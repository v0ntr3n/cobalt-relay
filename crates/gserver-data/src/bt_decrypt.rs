//! Exact 100% safe Rust port of `BtDecryptHook.cpp` + `07_bt_decrypt_keys.inc` +
//! `CConfigManager.cpp` hex-cell decryption from GServer (`vnsg2` / `libcocos2dlua.so`).
//!
//! ### Binary Header Layout (32 bytes, little-endian)
//! - `[0..4]`: `u32` magic == `0xFEFE_FEFE`
//! - `[4..8]`: `u32` `payload_size` == `total_size - 32`
//! - `[8]`: `u8` `algo_index` (`0` or `1` => `decrypt_basic`)
//! - `[9]`: `u8` `key_index` (`0..=15` => `off_CC01A0[key_index]`)
//! - `[10..32]`: `u8[22]` reserved (`0x00`)

use std::sync::{OnceLock, RwLock};

pub const BT_DECRYPT_MAGIC: u32 = 0xFEFE_FEFE;
pub const BT_DECRYPT_HEADER_SIZE: usize = 32;
pub const BT_MAX_KEY_INDEX: u8 = 15;
pub const BT_KEY_TABLE_COUNT: usize = 16;
pub const BT_MAX_KEY_LENGTH: usize = 508;

/// Base 128-entry seed array `aP_1` from `07_bt_decrypt_keys.inc` (`libcocos2dlua.so`).
pub const AP_1: [u32; 128] = [
    0x70, 0x96, 0xB2, 0x9D, 0xD0, 0x52, 0x19, 0x9A, 0x79, 0xC6, 0xD9, 0x6C, 0x6B, 0xBD, 0xDC, 0x4C,
    0x51, 0x52, 0xAC, 0xFA, 0x99, 0x5C, 0x65, 0x71, 0x9B, 0x06, 0x80, 0x4A, 0x02, 0x6E, 0x20, 0x02,
    0x40, 0x10, 0x04, 0x80, 0x04, 0x08, 0x01, 0x40, 0x99, 0x72, 0x05, 0x4C, 0x10, 0x56, 0x9E, 0x29,
    0xF0, 0x97, 0x70, 0x4A, 0x04, 0x5B, 0x87, 0xE0, 0x27, 0x58, 0xB2, 0x53, 0x53, 0x4D, 0xAF, 0x38,
    0xBE, 0xCB, 0x3E, 0x3F, 0x16, 0x40, 0xAD, 0x2F, 0xB3, 0x33, 0x7C, 0x43, 0x89, 0x1B, 0xEC, 0xF9,
    0x33, 0x5D, 0x44, 0xB6, 0x38, 0x4B, 0x98, 0x5F, 0xA3, 0xCA, 0xB3, 0x76, 0x18, 0x63, 0xAF, 0x57,
    0xAE, 0x6D, 0x16, 0x45, 0x2E, 0x43, 0x74, 0x61, 0x76, 0xF0, 0xA4, 0x7F, 0x8C, 0x91, 0x79, 0xBF,
    0x6E, 0x3D, 0x76, 0xA6, 0x88, 0x8E, 0x07, 0x2C, 0x12, 0xED, 0x22, 0x86, 0xCE, 0x98, 0xF7, 0x74,
];

/// Prime index lookup table from `07_bt_decrypt_keys.inc`.
pub const PRIME_INDICES: [u32; 32] = [
    2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89,
    93, 97, 101, 103, 107, 109, 113, 119,
];

/// `[0]` `encrypt_hash_header` (`len = 32`)
pub const KEY_00: [u8; 32] = [
    0x70, 0x96, 0xB2, 0x9D, 0xD0, 0x52, 0x19, 0x9A, 0x79, 0xC6, 0xD9, 0x6C, 0x6B, 0xBD, 0xDC, 0x4C,
    0x51, 0x52, 0xAC, 0xFA, 0x99, 0x5C, 0x65, 0x71, 0x9B, 0x06, 0x80, 0x4A, 0x02, 0x6E, 0x20, 0x02,
];

/// `[1]` `encrypt_hash_tail` (`len = 32`)
pub const KEY_01: [u8; 32] = [
    0x57, 0xAE, 0x6D, 0x16, 0x45, 0x2E, 0x43, 0x74, 0x61, 0x76, 0xF0, 0xA4, 0x7F, 0x8C, 0x91, 0x79,
    0xBF, 0x6E, 0x3D, 0x76, 0xA6, 0x88, 0x8E, 0x07, 0x2C, 0x12, 0xED, 0x22, 0x86, 0xCE, 0x98, 0xF7,
];

/// `[2]` `encrypt_hash_odd` (`len = 32`)
pub const KEY_02: [u8; 32] = [
    0x96, 0x9D, 0x52, 0x9A, 0xC6, 0x6C, 0xBD, 0x4C, 0x52, 0xFA, 0x5C, 0x71, 0x06, 0x4A, 0x6E, 0x02,
    0x10, 0x80, 0x08, 0x40, 0x72, 0x4C, 0x56, 0x29, 0x97, 0x4A, 0x5B, 0xE0, 0x58, 0x53, 0x4D, 0x38,
];

/// `[3]` `encrypt_hash_even` (`len = 32`)
pub const KEY_03: [u8; 32] = [
    0x70, 0xB2, 0xD0, 0x19, 0x79, 0xD9, 0x6B, 0xDC, 0x51, 0xAC, 0x99, 0x65, 0x9B, 0x80, 0x02, 0x20,
    0x40, 0x04, 0x04, 0x01, 0x99, 0x05, 0x10, 0x9E, 0xF0, 0x70, 0x04, 0x87, 0x27, 0xB2, 0x53, 0xAF,
];

/// `[4]` `encrypt_hash_fibonacci` (`len = 10`)
pub const KEY_04: [u8; 10] = [0x96, 0xB2, 0x9D, 0x52, 0x79, 0xBD, 0x5C, 0x04, 0xE0, 0xCA];

/// `[5]` `encrypt_hash_prime` (`len = 32`)
pub const KEY_05: [u8; 32] = [
    0xB2, 0x9D, 0x52, 0x9A, 0x6C, 0xBD, 0x52, 0xFA, 0x71, 0x6E, 0x02, 0x08, 0x72, 0x4C, 0x29, 0x5B,
    0x53, 0x4D, 0x3F, 0x2F, 0x33, 0xF9, 0xB6, 0xCA, 0x63, 0x6D, 0x43, 0x61, 0x7F, 0x91, 0x3D, 0x2C,
];

/// `[6]` `encrypt_hash_odds` (`len = 32`)
pub const KEY_06: [u8; 32] = [
    0x9D, 0x19, 0x79, 0xD9, 0x6B, 0xBD, 0x51, 0x99, 0x65, 0x71, 0x9B, 0x01, 0x99, 0x05, 0x29, 0x97,
    0x5B, 0x87, 0x27, 0x53, 0x53, 0x4D, 0xAF, 0xCB, 0x3F, 0xAD, 0x2F, 0xB3, 0x33, 0x43, 0x89, 0x1B,
];

/// `[7]` `encrypt_hash_evens` (`len = 32`)
pub const KEY_07: [u8; 32] = [
    0x70, 0x96, 0xB2, 0xD0, 0x52, 0x9A, 0xC6, 0x6C, 0xDC, 0x4C, 0x52, 0xAC, 0xFA, 0x5C, 0x06, 0x80,
    0x4A, 0x02, 0x6E, 0x20, 0x02, 0x40, 0x10, 0x04, 0x80, 0x04, 0x08, 0x40, 0x72, 0x4C, 0x10, 0x56,
];

/// `[8]` `encrypt_hash_tail_odd` (`len = 32`)
pub const KEY_08: [u8; 32] = [
    0x74, 0x98, 0x86, 0xED, 0x2C, 0x8E, 0xA6, 0x3D, 0xBF, 0x91, 0x7F, 0xF0, 0x61, 0x43, 0x45, 0x6D,
    0x57, 0x63, 0x76, 0xCA, 0x5F, 0x4B, 0xB6, 0x5D, 0xF9, 0x1B, 0x43, 0x33, 0x2F, 0x40, 0x3F, 0xCB,
];

/// `[9]` `encrypt_hash_tail_even` (`len = 32`)
pub const KEY_09: [u8; 32] = [
    0xF7, 0xCE, 0x22, 0x12, 0x07, 0x88, 0x76, 0x6E, 0x79, 0x8C, 0xA4, 0x76, 0x74, 0x2E, 0x16, 0xAE,
    0xAF, 0x18, 0xB3, 0xA3, 0x98, 0x38, 0x44, 0x33, 0xEC, 0x89, 0x7C, 0xB3, 0xAD, 0x16, 0x3E, 0xBE,
];

/// `[10]` `encrypt_hash_tail_odds` (`len = 32`)
pub const KEY_0A: [u8; 32] = [
    0xF7, 0xED, 0x07, 0x3D, 0xBF, 0x79, 0x91, 0x7F, 0x61, 0x43, 0x45, 0x6D, 0x57, 0xAF, 0x63, 0xB3,
    0xA3, 0x5F, 0x4B, 0x5D, 0x33, 0xF9, 0x1B, 0x89, 0x43, 0x33, 0xB3, 0x2F, 0xAD, 0x3F, 0xCB, 0xAF,
];

/// `[11]` `encrypt_hash_tail_evens` (`len = 32`)
pub const KEY_0B: [u8; 32] = [
    0x74, 0x98, 0xCE, 0x86, 0x22, 0x12, 0x2C, 0x8E, 0x88, 0xA6, 0x76, 0x6E, 0x8C, 0xA4, 0xF0, 0x76,
    0x74, 0x2E, 0x16, 0xAE, 0x18, 0x76, 0xCA, 0x98, 0x38, 0xB6, 0x44, 0xEC, 0x7C, 0x40, 0x16, 0x3E,
];

/// `[12]` `encrypt_hash_odd_sub` (`len = 32`)
pub const KEY_0C: [u8; 32] = [
    0xDA, 0x15, 0x7E, 0x7F, 0xB3, 0x6D, 0xAE, 0x90, 0xFF, 0xB2, 0x3D, 0xF4, 0x95, 0x36, 0x94, 0x1E,
    0x30, 0x84, 0xFC, 0xC1, 0x27, 0xB9, 0xBA, 0x75, 0x59, 0x26, 0xA9, 0xA7, 0xCF, 0x5F, 0x06, 0x77,
];

/// `[13]` `encrypt_hash_even_sub` (`len = 32`)
pub const KEY_0D: [u8; 32] = [
    0xE4, 0xCD, 0x39, 0x21, 0xED, 0x01, 0xE1, 0xFB, 0xA6, 0x61, 0xF7, 0xD6, 0x86, 0x48, 0x4E, 0xC2,
    0x0C, 0x7C, 0x07, 0xA7, 0x6D, 0x3C, 0xB8, 0x39, 0x27, 0x46, 0xD4, 0xB9, 0xA6, 0x00, 0x9E, 0x7A,
];

/// `[14]` `encrypt_hash_odd_add` (`len = 32`)
pub const KEY_0E: [u8; 32] = [
    0x06, 0x4F, 0x22, 0xB3, 0x3F, 0x45, 0x28, 0x28, 0xA3, 0xA6, 0xF5, 0xD6, 0xA1, 0xCA, 0x70, 0x22,
    0x50, 0x84, 0x0C, 0x41, 0x0B, 0x51, 0x66, 0xC7, 0x87, 0xBA, 0x5F, 0x67, 0x7F, 0x05, 0xA0, 0xE7,
];

/// `[15]` `encrypt_hash_even_add` (`len = 32`)
pub const KEY_0F: [u8; 32] = [
    0x48, 0x6D, 0x6B, 0x13, 0x9F, 0xD7, 0x99, 0x9D, 0xFE, 0x93, 0xC1, 0x0C, 0x86, 0x4C, 0x8E, 0x42,
    0x14, 0x84, 0x09, 0xD9, 0x77, 0x5C, 0xF4, 0x19, 0x07, 0x4E, 0xE2, 0x07, 0x0A, 0xA6, 0xFC, 0xF6,
];

/// Complete 16-slot default key table matching `kBtDefaultKeyTables[16]`.
pub const BT_DEFAULT_KEY_TABLES: [&[u8]; BT_KEY_TABLE_COUNT] = [
    &KEY_00, &KEY_01, &KEY_02, &KEY_03, &KEY_04, &KEY_05, &KEY_06, &KEY_07, &KEY_08, &KEY_09,
    &KEY_0A, &KEY_0B, &KEY_0C, &KEY_0D, &KEY_0E, &KEY_0F,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DecryptStatus {
    Ok = 0,
    NotEncrypted = 1,
    InvalidBuffer = 2,
    TruncatedHeader = 3,
    PayloadLengthMismatch = 4,
    UnsupportedAlgoIndex = 5,
    InvalidKeyIndex = 6,
    MissingKeyTable = 7,
}

impl DecryptStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::NotEncrypted => "NOT_ENCRYPTED",
            Self::InvalidBuffer => "INVALID_BUFFER",
            Self::TruncatedHeader => "TRUNCATED_HEADER",
            Self::PayloadLengthMismatch => "PAYLOAD_LENGTH_MISMATCH",
            Self::UnsupportedAlgoIndex => "UNSUPPORTED_ALGO_INDEX",
            Self::InvalidKeyIndex => "INVALID_KEY_INDEX",
            Self::MissingKeyTable => "MISSING_KEY_TABLE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptResult {
    pub status: DecryptStatus,
    pub magic: u32,
    pub algo_index: u8,
    pub key_index: u8,
    pub header_payload_size: u32,
    pub actual_payload_size: usize,
    pub message: String,
}

impl DecryptResult {
    pub const fn ok(&self) -> bool {
        matches!(self.status, DecryptStatus::Ok)
    }
}

impl std::fmt::Display for DecryptResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "status={} magic=0x{:08X} algo={} key_index={} header_payload={} actual_payload={} detail=\"{}\"",
            self.status.as_str(),
            self.magic,
            self.algo_index,
            self.key_index,
            self.header_payload_size,
            self.actual_payload_size,
            self.message
        )
    }
}

impl std::error::Error for DecryptResult {}

fn runtime_keys() -> &'static RwLock<[Option<Vec<u8>>; BT_KEY_TABLE_COUNT]> {
    static RUNTIME_KEYS: OnceLock<RwLock<[Option<Vec<u8>>; BT_KEY_TABLE_COUNT]>> = OnceLock::new();
    RUNTIME_KEYS.get_or_init(|| RwLock::new(std::array::from_fn(|_| None)))
}

#[inline]
fn read_le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Returns `true` if built-in or runtime `bt_decrypt` key tables are available.
pub fn has_decrypt_keys() -> bool {
    if let Ok(guard) = runtime_keys().read() {
        if guard.iter().any(|slot| slot.as_ref().is_some_and(|v| !v.is_empty())) {
            return true;
        }
    }
    BT_DEFAULT_KEY_TABLES.iter().any(|k| !k.is_empty())
}

/// Optional thread-safe runtime override for a key slot (`0..=15`).
pub fn register_key_table(key_index: u8, key_data: &[u8]) -> bool {
    if key_index > BT_MAX_KEY_INDEX || key_data.is_empty() || key_data.len() > BT_MAX_KEY_LENGTH {
        return false;
    }
    if let Ok(mut guard) = runtime_keys().write() {
        guard[usize::from(key_index)] = Some(key_data.to_vec());
        true
    } else {
        false
    }
}

/// Clears all runtime key overrides, restoring the compiled-in `07_bt_decrypt_keys.inc` slots.
pub fn clear_runtime_key_tables() {
    if let Ok(mut guard) = runtime_keys().write() {
        for slot in guard.iter_mut() {
            *slot = None;
        }
    }
}

fn with_resolved_key<R>(key_index: u8, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    if key_index > BT_MAX_KEY_INDEX {
        return None;
    }
    let idx = usize::from(key_index);
    if let Ok(guard) = runtime_keys().read() {
        if let Some(custom) = guard[idx].as_deref() {
            if !custom.is_empty() {
                return Some(f(custom));
            }
        }
    }
    let default_key = BT_DEFAULT_KEY_TABLES[idx];
    if default_key.is_empty() {
        None
    } else {
        Some(f(default_key))
    }
}

/// Checks whether a buffer starts with a complete 32-byte `0xFEFEFEFE` header.
pub fn is_bt_encrypted_buffer(data: &[u8]) -> bool {
    data.len() >= BT_DECRYPT_HEADER_SIZE && read_le_u32(&data[..4]) == BT_DECRYPT_MAGIC
}

/// Direct port of `decrypt_basic` (`0x340e4c` in `libcocos2dlua.so`).
///
/// Operates in-place on `data[32..]` and does **not** strip the 32-byte header.
pub fn decrypt_basic_in_place(data: &mut [u8]) -> Result<DecryptResult, DecryptResult> {
    if data.len() < BT_DECRYPT_HEADER_SIZE {
        let magic = if data.len() >= 4 { read_le_u32(&data[..4]) } else { 0 };
        return Err(DecryptResult {
            status: DecryptStatus::TruncatedHeader,
            magic,
            algo_index: 0,
            key_index: 0,
            header_payload_size: 0,
            actual_payload_size: 0,
            message: "Buffer smaller than 32-byte GServer header".to_string(),
        });
    }

    let magic = read_le_u32(&data[0..4]);
    let header_payload_size = read_le_u32(&data[4..8]);
    let algo_index = data[8];
    let key_index = data[9];
    let payload_len = data.len() - BT_DECRYPT_HEADER_SIZE;

    if magic != BT_DECRYPT_MAGIC {
        return Err(DecryptResult {
            status: DecryptStatus::NotEncrypted,
            magic,
            algo_index,
            key_index,
            header_payload_size,
            actual_payload_size: payload_len,
            message: "No 0xFEFEFEFE header; plaintext asset".to_string(),
        });
    }

    if payload_len != usize::try_from(header_payload_size).unwrap_or(usize::MAX) {
        return Err(DecryptResult {
            status: DecryptStatus::PayloadLengthMismatch,
            magic,
            algo_index,
            key_index,
            header_payload_size,
            actual_payload_size: payload_len,
            message: "Header payload size does not match (totalSize - 32)".to_string(),
        });
    }

    if algo_index != 0 && algo_index != 1 {
        return Err(DecryptResult {
            status: DecryptStatus::UnsupportedAlgoIndex,
            magic,
            algo_index,
            key_index,
            header_payload_size,
            actual_payload_size: payload_len,
            message: "Unsupported encryption algorithm index in header[8]".to_string(),
        });
    }

    if key_index > BT_MAX_KEY_INDEX {
        return Err(DecryptResult {
            status: DecryptStatus::InvalidKeyIndex,
            magic,
            algo_index,
            key_index,
            header_payload_size,
            actual_payload_size: payload_len,
            message: "Key index out of range (expected 0..=15)".to_string(),
        });
    }

    let applied = with_resolved_key(key_index, |key| {
        if payload_len > 0 {
            let payload = &mut data[BT_DECRYPT_HEADER_SIZE..];
            let key_len = key.len();
            let offset = payload_len % key_len;
            let mut i = 0usize;
            while i < payload_len {
                let idx = (i + offset) % key_len;
                payload[i] ^= key[idx];
                i += 2;
            }
        }
    });

    if applied.is_none() {
        return Err(DecryptResult {
            status: DecryptStatus::MissingKeyTable,
            magic,
            algo_index,
            key_index,
            header_payload_size,
            actual_payload_size: payload_len,
            message: "Key table slot is empty".to_string(),
        });
    }

    Ok(DecryptResult {
        status: DecryptStatus::Ok,
        magic,
        algo_index,
        key_index,
        header_payload_size,
        actual_payload_size: payload_len,
        message: "Decrypted successfully".to_string(),
    })
}

/// Full in-place decrypt + 32-byte header strip + NUL termination matching `BtDecryptHook::DecryptBuffer`.
pub fn decrypt_buffer_in_place(
    data: &mut [u8],
    size: &mut usize,
) -> Result<DecryptResult, DecryptResult> {
    if *size > data.len() {
        return Err(DecryptResult {
            status: DecryptStatus::InvalidBuffer,
            magic: 0,
            algo_index: 0,
            key_index: 0,
            header_payload_size: 0,
            actual_payload_size: 0,
            message: "Declared size exceeds slice capacity".to_string(),
        });
    }

    let active_len = *size;
    if active_len < BT_DECRYPT_HEADER_SIZE {
        if active_len >= 4 && read_le_u32(&data[..4]) == BT_DECRYPT_MAGIC {
            return Err(DecryptResult {
                status: DecryptStatus::TruncatedHeader,
                magic: BT_DECRYPT_MAGIC,
                algo_index: 0,
                key_index: 0,
                header_payload_size: 0,
                actual_payload_size: 0,
                message: "Truncated 0xFEFEFEFE header (< 32 bytes)".to_string(),
            });
        }
        return Err(DecryptResult {
            status: DecryptStatus::NotEncrypted,
            magic: 0,
            algo_index: 0,
            key_index: 0,
            header_payload_size: 0,
            actual_payload_size: active_len,
            message: "Buffer shorter than 32-byte header; treating as plaintext".to_string(),
        });
    }

    let diag = decrypt_basic_in_place(&mut data[..active_len])?;
    let payload_len = active_len - BT_DECRYPT_HEADER_SIZE;
    if payload_len > 0 {
        data.copy_within(BT_DECRYPT_HEADER_SIZE..active_len, 0);
    }
    data[payload_len..active_len].fill(0);
    *size = payload_len;
    Ok(diag)
}

/// Convenience callback matching `bool bt_decrypt(unsigned char* data, unsigned long* size)`.
pub fn bt_decrypt(data: &mut [u8], size: &mut usize) -> bool {
    decrypt_buffer_in_place(data, size).is_ok()
}

/// Decrypts `raw` if it carries a `0xFEFEFEFE` header, or returns `raw.to_vec()` unchanged for
/// unencrypted plaintext assets (while still rejecting corrupted/truncated `0xFEFEFEFE` containers).
pub fn bt_decrypt_vec(raw: &[u8]) -> Result<Vec<u8>, DecryptResult> {
    if raw.len() >= 4 && raw.len() < BT_DECRYPT_HEADER_SIZE && read_le_u32(&raw[..4]) == BT_DECRYPT_MAGIC {
        return Err(DecryptResult {
            status: DecryptStatus::TruncatedHeader,
            magic: BT_DECRYPT_MAGIC,
            algo_index: 0,
            key_index: 0,
            header_payload_size: 0,
            actual_payload_size: 0,
            message: "Truncated 0xFEFEFEFE header (< 32 bytes)".to_string(),
        });
    }
    if !is_bt_encrypted_buffer(raw) {
        return Ok(raw.to_vec());
    }
    bt_decrypt_strict(raw)
}

/// Strictly decrypts a `0xFEFEFEFE` container into a freshly allocated payload `Vec<u8>`.
pub fn bt_decrypt_strict(raw: &[u8]) -> Result<Vec<u8>, DecryptResult> {
    let mut work = raw.to_vec();
    let mut sz = work.len();
    decrypt_buffer_in_place(&mut work, &mut sz)?;
    work.truncate(sz);
    Ok(work)
}

/// Symmetric `0xFEFEFEFE` container encoder (for roundtrip tests and tooling).
pub fn bt_encrypt_vec(
    plaintext: &[u8],
    algo_index: u8,
    key_index: u8,
) -> Result<Vec<u8>, DecryptResult> {
    if algo_index != 0 && algo_index != 1 {
        return Err(DecryptResult {
            status: DecryptStatus::UnsupportedAlgoIndex,
            magic: BT_DECRYPT_MAGIC,
            algo_index,
            key_index,
            header_payload_size: 0,
            actual_payload_size: plaintext.len(),
            message: "Unsupported encryption algorithm index".to_string(),
        });
    }
    let Ok(payload_u32) = u32::try_from(plaintext.len()) else {
        return Err(DecryptResult {
            status: DecryptStatus::PayloadLengthMismatch,
            magic: BT_DECRYPT_MAGIC,
            algo_index,
            key_index,
            header_payload_size: u32::MAX,
            actual_payload_size: plaintext.len(),
            message: "Plaintext exceeds u32::MAX".to_string(),
        });
    };

    let mut out = Vec::with_capacity(BT_DECRYPT_HEADER_SIZE + plaintext.len());
    out.extend_from_slice(&BT_DECRYPT_MAGIC.to_le_bytes());
    out.extend_from_slice(&payload_u32.to_le_bytes());
    out.push(algo_index);
    out.push(key_index);
    out.extend_from_slice(&[0u8; 22]);
    out.extend_from_slice(plaintext);

    // Since decrypt_basic is an involution (even-byte XOR), running it over the framed buffer encrypts!
    decrypt_basic_in_place(&mut out)?;
    Ok(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn hex_decode(trimmed: &[u8]) -> Option<Vec<u8>> {
    if trimmed.len() < 2 || !trimmed.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let mut i = 0usize;
    while i < trimmed.len() {
        let hi = hex_nibble(trimmed[i])?;
        let lo = hex_nibble(trimmed[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

/// Exact port of `CTable::getRowById` cell decryption in `CConfigManager.cpp`:
/// trims trailing ASCII whitespace, hex-decodes even-length hex strings starting with
/// `FEFEFEFE`, runs `bt_decrypt`, and returns UTF-8 text (or returns plaintext cells as-is).
pub fn decrypt_babeltime_cell(raw_col: &str) -> Result<String, DecryptResult> {
    let trimmed = raw_col.trim_end_matches(['\r', '\n', ' ', '\t']);
    if trimmed.len() >= 64 && trimmed.len().is_multiple_of(2) {
        if let Some(mut decoded) = hex_decode(trimmed.as_bytes()) {
            if is_bt_encrypted_buffer(&decoded) {
                let mut sz = decoded.len();
                decrypt_buffer_in_place(&mut decoded, &mut sz)?;
                decoded.truncate(sz);
                return Ok(String::from_utf8_lossy(&decoded).into_owned());
            }
        }
    }
    Ok(raw_col.to_string())
}

/// Canonical POSIX `'/'` path normalization matching `BtDecryptHook::NormalizeAssetPath`.
pub fn normalize_asset_path(raw_path: &str) -> String {
    if raw_path.is_empty() {
        return String::new();
    }

    let unified: String = raw_path
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    let bytes = unified.as_bytes();

    let mut prefix = String::new();
    let mut pos = 0usize;
    let is_absolute: bool;
    let mut min_protected_segments = 0usize;

    if bytes.len() >= 2 && bytes[0] == b'/' && bytes[1] == b'/' && (bytes.len() == 2 || bytes[2] != b'/') {
        prefix.push_str("//");
        pos = 2;
        is_absolute = true;
        min_protected_segments = 2;
    } else if bytes[0] == b'/' {
        prefix.push('/');
        is_absolute = true;
        while pos < bytes.len() && bytes[pos] == b'/' {
            pos += 1;
        }
    } else if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        if bytes.len() >= 3 && bytes[2] == b'/' {
            prefix.push_str(&unified[..3]);
            pos = 3;
            is_absolute = true;
            while pos < bytes.len() && bytes[pos] == b'/' {
                pos += 1;
            }
        } else {
            prefix.push_str(&unified[..2]);
            pos = 2;
            is_absolute = false;
        }
    } else {
        is_absolute = false;
    }

    let had_trailing_slash = pos < bytes.len() && bytes.last() == Some(&b'/');
    let mut segments: Vec<&str> = Vec::new();

    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos] == b'/' {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let seg_end = unified[pos..]
            .find('/')
            .map_or(bytes.len(), |rel| pos + rel);
        let seg = &unified[pos..seg_end];
        pos = seg_end;

        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            if segments.len() > min_protected_segments && segments.last().copied() != Some("..") {
                segments.pop();
            } else if !is_absolute {
                segments.push(seg);
            }
        } else {
            segments.push(seg);
        }
    }

    if segments.is_empty() {
        if !prefix.is_empty() {
            return prefix;
        }
        if unified == "." {
            return ".".to_string();
        }
        return String::new();
    }

    let mut out = prefix;
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(seg);
    }
    if had_trailing_slash && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// Resolves a relative asset path against a base file path using POSIX `'/'` separators.
pub fn resolve_relative_asset_path(relative_path: &str, base_file_path: &str) -> String {
    let norm_rel = normalize_asset_path(relative_path);
    let norm_base = normalize_asset_path(base_file_path);

    if norm_rel.is_empty() {
        return norm_base;
    }
    let rel_bytes = norm_rel.as_bytes();
    if rel_bytes[0] == b'/' || (rel_bytes.len() >= 2 && rel_bytes[1] == b':') {
        return norm_rel;
    }
    match norm_base.rfind('/') {
        None => norm_rel,
        Some(slash_pos) => normalize_asset_path(&format!("{}{}", &norm_base[..=slash_pos], norm_rel)),
    }
}

/// Replaces or appends a file extension on a normalized POSIX path.
pub fn replace_file_extension(path: &str, new_ext: &str) -> String {
    let norm = normalize_asset_path(path);
    let slash_pos = norm.rfind('/');
    let dot_pos = norm.rfind('.');

    match (dot_pos, slash_pos) {
        (None, _) => format!("{norm}{new_ext}"),
        (Some(d), Some(s)) if d < s => format!("{norm}{new_ext}"),
        (Some(d), _) => format!("{}{}", &norm[..d], new_ext),
    }
}
