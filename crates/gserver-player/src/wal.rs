use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{PlayerError, Result};
use crate::json::{JsonObjReader, JsonValue};
use crate::models::{Account, PlayerProfile};

/// Pure-Rust IEEE 802.3 CRC32 checksum (`0xEDB88320` reflected polynomial).
pub fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Payload of a single committed Write-Ahead Log entry.
#[derive(Clone, Debug, PartialEq)]
pub enum WalOp {
    UpsertAccount(Account),
    UpsertPlayer(Box<PlayerProfile>),
    Checkpoint { seq: u64 },
}

impl WalOp {
    pub fn op_tag(&self) -> &'static str {
        match self {
            WalOp::UpsertAccount(_) => "upsert_account",
            WalOp::UpsertPlayer(_) => "upsert_player",
            WalOp::Checkpoint { .. } => "checkpoint",
        }
    }

    pub fn to_payload_json(&self) -> JsonValue {
        match self {
            WalOp::UpsertAccount(acc) => acc.to_json(),
            WalOp::UpsertPlayer(player) => player.to_storage_json(),
            WalOp::Checkpoint { seq } => {
                let mut m = BTreeMap::new();
                m.insert("checkpoint_seq".into(), JsonValue::from(*seq));
                JsonValue::Object(m)
            }
        }
    }

    pub fn from_tag_and_payload(tag: &str, payload: &JsonValue) -> Result<Self> {
        match tag {
            "upsert_account" => Ok(WalOp::UpsertAccount(Account::from_json(payload)?)),
            "upsert_player" => Ok(WalOp::UpsertPlayer(Box::new(
                PlayerProfile::from_storage_json(payload)?,
            ))),
            "checkpoint" => {
                let r = JsonObjReader::new(payload, "WalCheckpoint")?;
                Ok(WalOp::Checkpoint {
                    seq: r.req_u64("checkpoint_seq")?,
                })
            }
            other => Err(PlayerError::Json(format!("unknown WAL op '{other}'"))),
        }
    }
}

/// Checksummed single-line JSONL record written to `data/wal/operations.wal`.
#[derive(Clone, Debug, PartialEq)]
pub struct WalRecord {
    pub seq: u64,
    pub timestamp: u64,
    pub op: WalOp,
    pub crc32: u32,
}

impl WalRecord {
    pub fn new(seq: u64, timestamp: u64, op: WalOp) -> Self {
        let payload_json = op.to_payload_json();
        let canonical = Self::canonical_signing_string(seq, timestamp, op.op_tag(), &payload_json);
        let crc32 = crc32_ieee(canonical.as_bytes());
        Self {
            seq,
            timestamp,
            op,
            crc32,
        }
    }

    fn canonical_signing_string(
        seq: u64,
        timestamp: u64,
        op_tag: &str,
        payload_json: &JsonValue,
    ) -> String {
        format!(
            "{seq}:{timestamp}:{op_tag}:{}",
            payload_json.to_json_string()
        )
    }

    pub fn to_jsonl_line(&self) -> String {
        let payload_json = self.op.to_payload_json();
        let mut m = BTreeMap::new();
        m.insert("seq".into(), JsonValue::from(self.seq));
        m.insert("ts".into(), JsonValue::from(self.timestamp));
        m.insert("op".into(), JsonValue::from(self.op.op_tag()));
        m.insert("crc32".into(), JsonValue::from(self.crc32));
        m.insert("payload".into(), payload_json);
        let mut line = JsonValue::Object(m).to_json_string();
        line.push('\n');
        line
    }

    pub fn parse_and_verify(line_str: &str, line_no: usize) -> Result<Self> {
        let val = JsonValue::parse(line_str).map_err(|e| PlayerError::WalCorrupted {
            line: line_no,
            reason: e.to_string(),
        })?;
        let r = JsonObjReader::new(&val, "WalRecord").map_err(|e| PlayerError::WalCorrupted {
            line: line_no,
            reason: e.to_string(),
        })?;
        let seq = r.req_u64("seq").map_err(|e| PlayerError::WalCorrupted {
            line: line_no,
            reason: e.to_string(),
        })?;
        let timestamp = r.req_u64("ts").map_err(|e| PlayerError::WalCorrupted {
            line: line_no,
            reason: e.to_string(),
        })?;
        let op_tag = r.req_str("op").map_err(|e| PlayerError::WalCorrupted {
            line: line_no,
            reason: e.to_string(),
        })?;
        let expected_crc = r.req_u32("crc32").map_err(|e| PlayerError::WalCorrupted {
            line: line_no,
            reason: e.to_string(),
        })?;
        let payload = r
            .req_val("payload")
            .map_err(|e| PlayerError::WalCorrupted {
                line: line_no,
                reason: e.to_string(),
            })?;

        let signing = Self::canonical_signing_string(seq, timestamp, &op_tag, payload);
        let actual_crc = crc32_ieee(signing.as_bytes());
        if actual_crc != expected_crc {
            return Err(PlayerError::WalCorrupted {
                line: line_no,
                reason: format!(
                    "CRC32 mismatch: expected 0x{expected_crc:08X}, computed 0x{actual_crc:08X}"
                ),
            });
        }

        let op = WalOp::from_tag_and_payload(&op_tag, payload).map_err(|e| {
            PlayerError::WalCorrupted {
                line: line_no,
                reason: e.to_string(),
            }
        })?;

        Ok(Self {
            seq,
            timestamp,
            op,
            crc32: expected_crc,
        })
    }
}

/// Append-only Write-Ahead Log writer + crash-recovery scanner.
pub struct WriteAheadLog {
    path: PathBuf,
    file: Option<File>,
    next_seq: u64,
}

/// Outcome of scanning `operations.wal` during startup recovery.
#[derive(Clone, Debug, Default)]
pub struct WalScanOutcome {
    pub records: Vec<WalRecord>,
    pub truncated_corrupt_tail_bytes: u64,
    pub max_seq: u64,
}

impl WriteAheadLog {
    /// Open or create `operations.wal`, recovering from any torn/incomplete tail write caused by power loss.
    pub fn open_and_recover(path: impl AsRef<Path>) -> Result<(Self, WalScanOutcome)> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut outcome = WalScanOutcome::default();
        if path.exists() {
            let mut raw = Vec::new();
            File::open(&path)?.read_to_end(&mut raw)?;

            let mut valid_len = 0usize;
            let mut line_no = 0usize;
            let mut cursor = 0usize;

            while cursor < raw.len() {
                line_no += 1;
                let rel_end = raw[cursor..].iter().position(|&b| b == b'\n');
                let Some(nl_offset) = rel_end else {
                    // Incomplete trailing line with no newline -> torn tail from crash mid-append!
                    break;
                };
                let line_end = cursor + nl_offset + 1;
                let line_bytes = &raw[cursor..line_end - 1];
                if line_bytes.iter().all(|b| b.is_ascii_whitespace()) {
                    valid_len = line_end;
                    cursor = line_end;
                    continue;
                }

                let Ok(line_str) = std::str::from_utf8(line_bytes) else {
                    if line_end == raw.len() {
                        break;
                    }
                    return Err(PlayerError::WalCorrupted {
                        line: line_no,
                        reason: "invalid UTF-8 in middle of WAL".into(),
                    });
                };

                match WalRecord::parse_and_verify(line_str, line_no) {
                    Ok(rec) => {
                        if rec.seq > outcome.max_seq {
                            outcome.max_seq = rec.seq;
                        }
                        outcome.records.push(rec);
                        valid_len = line_end;
                        cursor = line_end;
                    }
                    Err(err) => {
                        // Torn/corrupted final line at EOF -> truncate cleanly to last committed entry
                        if line_end == raw.len()
                            || raw[line_end..]
                                .iter()
                                .all(|b| *b == 0 || b.is_ascii_whitespace())
                        {
                            break;
                        }
                        return Err(err);
                    }
                }
            }

            if valid_len < raw.len() {
                outcome.truncated_corrupt_tail_bytes = (raw.len() - valid_len) as u64;
                let f = OpenOptions::new().write(true).open(&path)?;
                f.set_len(valid_len as u64)?;
                f.sync_all()?;
            }
        }

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        file.seek(SeekFrom::End(0))?;

        let next_seq = outcome.max_seq + 1;
        Ok((
            Self {
                path,
                file: Some(file),
                next_seq,
            },
            outcome,
        ))
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Append a `WalOp`, `write_all` + `sync_data`, and advance `next_seq`.
    pub fn append(&mut self, timestamp: u64, op: WalOp) -> Result<WalRecord> {
        let seq = self.next_seq;
        let record = WalRecord::new(seq, timestamp, op);
        let line = record.to_jsonl_line();
        let fh = self.file.as_mut().ok_or_else(|| {
            PlayerError::Io(std::io::Error::other("WAL file handle closed"))
        })?;
        fh.write_all(line.as_bytes())?;
        fh.sync_data()?;
        self.next_seq += 1;
        Ok(record)
    }

    /// Compact/truncate the WAL down to a fresh checkpoint marker once all snapshots are synced on disk.
    pub fn reset_with_checkpoint(&mut self, timestamp: u64) -> Result<u64> {
        let cp_seq = self.next_seq;
        let cp_record = WalRecord::new(cp_seq, timestamp, WalOp::Checkpoint { seq: cp_seq });
        let line = cp_record.to_jsonl_line();

        let tmp_path = self.path.with_extension("wal.ckpt.tmp");
        {
            let mut tmp = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)?;
            tmp.write_all(line.as_bytes())?;
            tmp.sync_all()?;
        }
        // Close open WAL handle before renaming so Windows NTFS MoveFileExW succeeds.
        let _ = self.file.take();
        fs::rename(&tmp_path, &self.path)?;
        if let Some(parent) = self.path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        let mut reopened = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)?;
        reopened.seek(SeekFrom::End(0))?;
        self.file = Some(reopened);
        self.next_seq = cp_seq + 1;
        Ok(cp_seq)
    }
}
