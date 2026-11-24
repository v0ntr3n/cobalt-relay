use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{PlayerError, Result};
use crate::json::{JsonObjReader, JsonValue};
use crate::models::{Account, PlayerProfile, StarterPackConfig};
use crate::wal::{WalOp, WriteAheadLog};

static TMP_FILE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Crash-safe atomic file replacement:
/// 1. Writes complete payload to `<target>.tmp.<pid>.<seq>` in the same directory.
/// 2. Calls `File::sync_all()` so data blocks reach stable storage before rename.
/// 3. Atomically replaces `<target>` via `fs::rename`.
/// 4. Calls `sync_all()` on the parent directory so the directory entry update is durable.
pub fn atomic_write_bytes(target: &Path, bytes: &[u8]) -> Result<()> {
    let parent = target.parent().ok_or_else(|| {
        PlayerError::InvalidParameter(format!(
            "path '{}' has no parent directory",
            target.display()
        ))
    })?;
    fs::create_dir_all(parent)?;

    let seq = TMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("snapshot.json");
    let tmp_path = parent.join(format!(".{file_name}.tmp.{pid}.{seq}"));

    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }

    if let Err(err) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(PlayerError::Io(err));
    }

    if let Ok(dir_handle) = File::open(parent) {
        let _ = dir_handle.sync_all();
    }
    Ok(())
}

/// Summary of startup integrity checks and crash recovery executed by `PlayerStore::open`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub loaded_accounts: usize,
    pub loaded_players: usize,
    pub replayed_wal_accounts: usize,
    pub replayed_wal_players: usize,
    pub cleaned_orphan_tmp_files: usize,
    pub truncated_corrupt_wal_bytes: u64,
}

/// Crash-safe file + WAL persistence engine managing:
/// - `<root>/accounts.json`
/// - `<root>/players/<uid>.json`
/// - `<root>/wal/operations.wal`
pub struct PlayerStore {
    data_dir: PathBuf,
    players_dir: PathBuf,
    accounts_path: PathBuf,
    starter_config: StarterPackConfig,
    accounts: BTreeMap<String, Account>,
    players: BTreeMap<u64, PlayerProfile>,
    next_uid: u64,
    wal: WriteAheadLog,
    last_recovery: RecoveryReport,
}

impl PlayerStore {
    /// Open or initialize `<data_dir>`, sweeping orphaned `.tmp` files and replaying any un-snapshotted WAL entries.
    pub fn open(data_dir: impl AsRef<Path>, starter_config: StarterPackConfig) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let players_dir = data_dir.join("players");
        let wal_dir = data_dir.join("wal");
        fs::create_dir_all(&players_dir)?;
        fs::create_dir_all(&wal_dir)?;

        let mut report = RecoveryReport::default();
        report.cleaned_orphan_tmp_files += sweep_orphan_tmp_files(&data_dir)?;
        report.cleaned_orphan_tmp_files += sweep_orphan_tmp_files(&players_dir)?;
        report.cleaned_orphan_tmp_files += sweep_orphan_tmp_files(&wal_dir)?;

        let accounts_path = data_dir.join("accounts.json");
        let mut accounts = BTreeMap::new();
        let mut next_uid = 50_001u64;

        if accounts_path.exists() {
            let txt = fs::read_to_string(&accounts_path)?;
            let val = JsonValue::parse(&txt)?;
            let r = JsonObjReader::new(&val, "accounts.json")?;
            next_uid = r.opt_u64("next_uid", 50_001)?;
            if let Some(acc_obj) = r.opt_obj("accounts")? {
                for (pid, a_val) in acc_obj {
                    let mut acc = Account::from_json(a_val)?;
                    acc.pid = pid.clone();
                    for &uid in acc.characters.values() {
                        if uid >= next_uid {
                            next_uid = uid + 1;
                        }
                    }
                    accounts.insert(pid.clone(), acc);
                }
            }
        }
        report.loaded_accounts = accounts.len();

        let mut players = BTreeMap::new();
        for entry in fs::read_dir(&players_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let txt = fs::read_to_string(&path)?;
            let val = JsonValue::parse(&txt)?;
            let player = PlayerProfile::from_storage_json(&val)?;
            if player.uid >= next_uid {
                next_uid = player.uid + 1;
            }
            players.insert(player.uid, player);
        }
        report.loaded_players = players.len();

        let (wal, wal_scan) = WriteAheadLog::open_and_recover(wal_dir.join("operations.wal"))?;
        report.truncated_corrupt_wal_bytes = wal_scan.truncated_corrupt_tail_bytes;

        // Replay WAL records that were durably committed before a crash prevented snapshot rename
        let mut dirty_accounts = false;
        let mut dirty_player_uids = Vec::new();

        for rec in wal_scan.records {
            match rec.op {
                WalOp::UpsertAccount(acc) => {
                    for &uid in acc.characters.values() {
                        if uid >= next_uid {
                            next_uid = uid + 1;
                        }
                    }
                    let needs_apply = accounts.get(&acc.pid) != Some(&acc);
                    if needs_apply {
                        accounts.insert(acc.pid.clone(), acc);
                        dirty_accounts = true;
                        report.replayed_wal_accounts += 1;
                    }
                }
                WalOp::UpsertPlayer(player) => {
                    let player = *player;
                    if player.uid >= next_uid {
                        next_uid = player.uid + 1;
                    }
                    let needs_apply = match players.get(&player.uid) {
                        None => true,
                        Some(existing) => {
                            player.revision > existing.revision || existing != &player
                        }
                    };
                    if needs_apply {
                        let uid = player.uid;
                        players.insert(uid, player);
                        if !dirty_player_uids.contains(&uid) {
                            dirty_player_uids.push(uid);
                        }
                        report.replayed_wal_players += 1;
                    }
                }
                WalOp::Checkpoint { .. } => {}
            }
        }

        let store = Self {
            data_dir,
            players_dir,
            accounts_path,
            starter_config,
            accounts,
            players,
            next_uid,
            wal,
            last_recovery: report,
        };

        if dirty_accounts || !store.accounts_path.exists() {
            store.write_accounts_snapshot()?;
        }
        for uid in dirty_player_uids {
            if let Some(player) = store.players.get(&uid) {
                store.write_player_snapshot(player)?;
            }
        }

        Ok(store)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn accounts_path(&self) -> &Path {
        &self.accounts_path
    }

    pub fn player_path(&self, uid: u64) -> PathBuf {
        self.players_dir.join(format!("{uid}.json"))
    }

    pub fn starter_config(&self) -> &StarterPackConfig {
        &self.starter_config
    }

    pub fn last_recovery_report(&self) -> &RecoveryReport {
        &self.last_recovery
    }

    pub fn get_account(&self, pid: &str) -> Option<&Account> {
        self.accounts.get(pid)
    }

    pub fn find_account_by_username(&self, username: &str) -> Option<&Account> {
        self.accounts.values().find(|a| a.username == username)
    }

    pub fn get_player(&self, uid: u64) -> Option<&PlayerProfile> {
        self.players.get(&uid)
    }

    pub fn get_player_for_account(&self, pid: &str, group_id: &str) -> Option<&PlayerProfile> {
        let acc = self.accounts.get(pid)?;
        let uid = acc.characters.get(group_id).copied()?;
        self.players.get(&uid)
    }

    /// Return `user.getUserBeforLogin` payload (`{}` if account has no character in `group_id`, else `{uid, ...}`).
    pub fn get_user_before_login(&self, pid: &str, group_id: &str) -> Result<JsonValue> {
        let acc = self
            .accounts
            .get(pid)
            .ok_or_else(|| PlayerError::AccountNotFound(pid.to_string()))?;
        if let Some(&uid) = acc.characters.get(group_id) {
            if let Some(player) = self.players.get(&uid) {
                return Ok(player.to_vnsg2_user_before_login());
            }
        }
        Ok(JsonValue::empty_object())
    }

    /// Return HTTP `/phone/loginrole/` / `user.getUsers` role summaries across all groups for `pid`.
    pub fn list_character_summaries(&self, pid: &str) -> Result<Vec<JsonValue>> {
        let acc = self
            .accounts
            .get(pid)
            .ok_or_else(|| PlayerError::AccountNotFound(pid.to_string()))?;
        let mut out = Vec::new();
        for &uid in acc.characters.values() {
            if let Some(p) = self.players.get(&uid) {
                out.push(p.to_character_summary());
            }
        }
        Ok(out)
    }

    /// Register a new account in `accounts.json` + WAL.
    pub fn register_account(
        &mut self,
        pid: impl Into<String>,
        qid: impl Into<String>,
        username: impl Into<String>,
        auth_token: impl Into<String>,
        now_ts: u64,
    ) -> Result<Account> {
        let pid = pid.into();
        let username = username.into();
        if pid.trim().is_empty() || username.trim().is_empty() {
            return Err(PlayerError::InvalidParameter(
                "account pid and username must be non-empty".into(),
            ));
        }
        if self.accounts.contains_key(&pid) {
            return Err(PlayerError::AccountAlreadyExists(pid));
        }
        if self.find_account_by_username(&username).is_some() {
            return Err(PlayerError::AccountAlreadyExists(username));
        }

        let acc = Account {
            pid: pid.clone(),
            qid: qid.into(),
            username,
            auth_token: auth_token.into(),
            characters: BTreeMap::new(),
            created_at: now_ts,
            last_login_at: now_ts,
            banned_until: 0,
            ban_reason: None,
        };

        self.wal.append(now_ts, WalOp::UpsertAccount(acc.clone()))?;
        self.accounts.insert(pid, acc.clone());
        self.write_accounts_snapshot()?;
        Ok(acc)
    }

    /// Create a new character (`user.createUser`) with starter gift pack, writing WAL first and then
    /// atomically persisting `data/players/<uid>.json` and `data/accounts.json`.
    pub fn create_character(
        &mut self,
        pid: &str,
        group_id: &str,
        utid: u32,
        uname: &str,
        now_ts: u64,
    ) -> Result<PlayerProfile> {
        let resolved_group = if group_id.is_empty() {
            self.starter_config.default_group_id.clone()
        } else {
            group_id.to_string()
        };

        let acc = self
            .accounts
            .get(pid)
            .ok_or_else(|| PlayerError::AccountNotFound(pid.to_string()))?;
        if let Some(&existing_uid) = acc.characters.get(&resolved_group) {
            return Err(PlayerError::CharacterAlreadyExists {
                pid: pid.to_string(),
                group_id: resolved_group,
                uid: existing_uid,
            });
        }

        // Reject duplicate character name within the same server group
        if self
            .players
            .values()
            .any(|p| p.group_id == resolved_group && p.uname == uname.trim())
        {
            return Err(PlayerError::InvalidParameter(format!(
                "character uname '{}' is already taken in group {}",
                uname.trim(),
                resolved_group
            )));
        }

        let uid = self.next_uid;
        let profile = PlayerProfile::new_character(
            uid,
            pid,
            resolved_group.clone(),
            utid,
            uname,
            now_ts,
            &self.starter_config,
        )?;

        let mut updated_acc = acc.clone();
        updated_acc.characters.insert(resolved_group, uid);
        updated_acc.last_login_at = now_ts;

        // Commit both records to WAL BEFORE updating snapshots on disk
        self.wal
            .append(now_ts, WalOp::UpsertPlayer(Box::new(profile.clone())))?;
        self.wal
            .append(now_ts, WalOp::UpsertAccount(updated_acc.clone()))?;

        self.next_uid = uid + 1;
        self.players.insert(uid, profile.clone());
        self.accounts.insert(pid.to_string(), updated_acc);

        self.write_player_snapshot(&profile)?;
        self.write_accounts_snapshot()?;
        Ok(profile)
    }

    /// Execute `CS_USER_USERLOGIN` (`user.userLogin`): returns `"ok"` or `"ban:<until>:<reason>"`
    /// while updating login timestamps and natural stamina recovery.
    pub fn record_user_login(&mut self, uid: u64, now_ts: u64) -> Result<String> {
        let pid = self
            .players
            .get(&uid)
            .map(|p| p.pid.clone())
            .ok_or(PlayerError::CharacterNotFound(uid))?;

        if let Some(acc) = self.accounts.get(&pid) {
            if acc.is_banned(now_ts) {
                let reason = acc.ban_reason.as_deref().unwrap_or("suspended");
                return Ok(format!("ban:{}:{reason}", acc.banned_until));
            }
        }

        self.mutate_player(uid, now_ts, |player| {
            player.tick_stamina_recovery(now_ts);
            player.last_login_time = player.login_time;
            player.login_time = now_ts;
            player.revision = player.revision.saturating_add(1);
            Ok(())
        })?;

        Ok("ok".to_string())
    }

    /// Transactional player mutation:
    /// 1. Clones `PlayerProfile` so any error inside `f` leaves in-memory and on-disk state untouched!
    /// 2. Appends + `sync_data`s the post-mutation state to `operations.wal`.
    /// 3. Atomically writes `data/players/<uid>.json` via temp-file + `sync_all` + `fs::rename`.
    pub fn mutate_player<F, R>(&mut self, uid: u64, now_ts: u64, f: F) -> Result<R>
    where
        F: FnOnce(&mut PlayerProfile) -> Result<R>,
    {
        let mut working = self
            .players
            .get(&uid)
            .cloned()
            .ok_or(PlayerError::CharacterNotFound(uid))?;
        let prev_rev = working.revision;
        let out = f(&mut working)?;
        if working.revision <= prev_rev {
            working.revision = prev_rev + 1;
        }

        self.wal
            .append(now_ts, WalOp::UpsertPlayer(Box::new(working.clone())))?;
        self.write_player_snapshot(&working)?;
        self.players.insert(uid, working);
        Ok(out)
    }

    /// Flush all snapshots and compact `operations.wal` to a single checkpoint entry.
    pub fn checkpoint(&mut self, now_ts: u64) -> Result<u64> {
        self.write_accounts_snapshot()?;
        for player in self.players.values() {
            self.write_player_snapshot(player)?;
        }
        self.wal.reset_with_checkpoint(now_ts)
    }

    fn write_accounts_snapshot(&self) -> Result<()> {
        let mut acc_map = BTreeMap::new();
        for (pid, acc) in &self.accounts {
            acc_map.insert(pid.clone(), acc.to_json());
        }
        let mut root = BTreeMap::new();
        root.insert("schema_version".into(), JsonValue::from(1u32));
        root.insert("next_uid".into(), JsonValue::from(self.next_uid));
        root.insert("accounts".into(), JsonValue::Object(acc_map));
        let pretty = JsonValue::Object(root).to_json_pretty();
        atomic_write_bytes(&self.accounts_path, pretty.as_bytes())
    }

    fn write_player_snapshot(&self, player: &PlayerProfile) -> Result<()> {
        let path = self.player_path(player.uid);
        let pretty = player.to_storage_json().to_json_pretty();
        atomic_write_bytes(&path, pretty.as_bytes())
    }
}

fn sweep_orphan_tmp_files(dir: &Path) -> Result<usize> {
    let mut removed = 0usize;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name.contains(".tmp.") || name.ends_with(".ckpt.tmp") {
                fs::remove_file(&path)?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}
