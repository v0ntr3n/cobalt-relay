//! Thread-safe `ServerWorld` coordinating `MapiService` (HTTP SDK), `PlayerStore` (WAL + atomic JSON),
//! `GameplayEngine` (towns, chat/GM, gacha, shop, mail/rewards, PvE copy, arena), and live TCP peer pushes.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use gserver_data::{protagonist_by_utid, GameDataStore};
use gserver_gameplay::engine::GameplayEngine;
use gserver_gameplay::mail::RewardEntry;
use gserver_http::{MapiService, RoleSummary};
use gserver_player::{JsonValue as PlayerJson, PlayerProfile, PlayerStore};
use gserver_protocol::{encode_s2c_single_frame, S2cEnvelope, ServerTxEncoder, Value};
use gserver_rpc_catalog::bootstrap::{
    build_random_names_ret, BarrageState, EnterViewSnapshot, VaConfig,
};

use crate::bridge::{
    catalog_amf3_to_protocol, dto_to_amf3, player_json_to_amf3,
    profile_to_gameplay_state_with_store, sync_gameplay_back_to_profile, to_gameplay_request,
    u64_to_amf3,
};
use crate::bxh_snapshot::BxhSnapshot;
use crate::config::DaemonConfig;
use crate::player_rpcs::try_handle_player_rpc;

/// Per-TCP-connection session state tracked across RPCs.
#[derive(Debug, Default, Clone)]
pub struct ConnectionSession {
    pub authenticated_pid: Option<String>,
    pub qid: String,
    pub group_id: String,
    pub logged_in_uid: Option<u64>,
    pub session_token: String,
}

/// Output envelopes produced by handling one client `RpcRequest`.
pub struct HandledRpc {
    /// Envelopes to send back to the calling TCP connection (response first, followed by caller pushes).
    pub caller_envelopes: Vec<S2cEnvelope>,
    /// Asynchronous `(target_uid, S2cEnvelope)` pushes to deliver to other connected peers.
    pub peer_envelopes: Vec<(u64, S2cEnvelope)>,
}

struct WorldInner {
    store: PlayerStore,
    gameplay: GameplayEngine,
    peers: HashMap<u64, Arc<Mutex<TcpStream>>>,
    token_seq: u64,
}

/// Shared dual-protocol server state bridged across the HTTP MAPI listener and TCP Game Server.
pub struct ServerWorld {
    config: DaemonConfig,
    mapi: Arc<MapiService>,
    data_store: GameDataStore,
    bxh_snapshot: BxhSnapshot,
    tx_encoder: ServerTxEncoder,
    inner: Mutex<WorldInner>,
}

impl ServerWorld {
    pub fn open(config: DaemonConfig, mapi: Arc<MapiService>) -> Result<Arc<Self>, String> {
        let store = PlayerStore::open(&config.data_dir, config.starter.clone())
            .map_err(|e| format!("failed to open PlayerStore at {:?}: {e}", config.data_dir))?;

        let data_store = if let Some(ref export_dir) = config.db_export_dir {
            GameDataStore::from_export_dir(export_dir)
                .map_err(|e| format!("failed to load exported DB dir {:?}: {e}", export_dir))?
        } else {
            GameDataStore::new_embedded()
        };

        let bxh_snapshot = if let Some(ref snap_path) = config.bxh_snapshot_path {
            BxhSnapshot::load_optional(snap_path).unwrap_or_else(|e| {
                eprintln!("[gserver] warning: ignoring BXH snapshot {:?}: {e}", snap_path);
                BxhSnapshot::default()
            })
        } else {
            BxhSnapshot::default()
        };

        let mut gameplay = GameplayEngine::new(0x4F4D_4733_515F_564E);
        let snap_arena = bxh_snapshot.extract_arena_players();
        if !snap_arena.is_empty() {
            gameplay.arena.seed_competitors_with_htid(
                snap_arena
                    .into_iter()
                    .map(|p| (p.uid, p.uname, p.level, p.fight_force, p.position, p.htid))
                    .collect(),
            );
        }

        // Hydrate MapiService role summaries & GameplayEngine from persisted PlayerStore!
        let mut roles_by_pid: HashMap<String, Vec<RoleSummary>> = HashMap::new();
        for profile in Self::collect_all_persisted_profiles(&store) {
            gp_register_and_role(&profile, &data_store, &mut gameplay, &mut roles_by_pid);
        }
        for (pid, roles) in roles_by_pid {
            mapi.set_roles_for_pid(pid, roles);
        }

        Ok(Arc::new(Self {
            config,
            mapi,
            data_store,
            bxh_snapshot,
            tx_encoder: ServerTxEncoder::new(),
            inner: Mutex::new(WorldInner {
                store,
                gameplay,
                peers: HashMap::new(),
                token_seq: 1000,
            }),
        }))
    }

    fn collect_all_persisted_profiles(store: &PlayerStore) -> Vec<PlayerProfile> {
        let mut out = Vec::new();
        let players_dir = store.data_dir().join("players");
        if let Ok(entries) = std::fs::read_dir(players_dir) {
            for entry in entries.flatten() {
                if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                    if let Ok(uid) = stem.parse::<u64>() {
                        if let Some(p) = store.get_player(uid) {
                            out.push(p.clone());
                        }
                    }
                }
            }
        }
        out
    }

    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    pub fn mapi(&self) -> &Arc<MapiService> {
        &self.mapi
    }

    pub fn data_store(&self) -> &GameDataStore {
        &self.data_store
    }

    pub fn now_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1_795_500_500)
    }

    pub fn online_player_count(&self) -> usize {
        self.inner.lock().map(|g| g.peers.len()).unwrap_or(0)
    }

    /// Registers a live TCP writer for `uid` so peer broadcasts reach this client immediately.
    pub fn bind_peer_stream(&self, uid: u64, writer: Arc<Mutex<TcpStream>>) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.peers.insert(uid, writer);
        }
    }

    /// Removes a disconnected client from `peers` and broadcasts `town.delPlayer` if in a town room.
    pub fn on_disconnect(&self, session: &ConnectionSession) {
        let Some(uid) = session.logged_in_uid else {
            return;
        };
        let now_ts = Self::now_unix_secs();
        let deliveries: Vec<(Arc<Mutex<TcpStream>>, S2cEnvelope)> = {
            let Ok(mut guard) = self.inner.lock() else {
                return;
            };
            guard.peers.remove(&uid);
            let (_, bcasts) = guard.gameplay.town.leave_town(uid, now_ts);
            bcasts
                .into_iter()
                .filter_map(|(target_uid, pkt)| {
                    let stream = guard.peers.get(&target_uid)?.clone();
                    let env = S2cEnvelope::push(pkt.callback_name, dto_to_amf3(&pkt.ret, true))
                        .with_time(pkt.time as i64);
                    Some((stream, env))
                })
                .collect()
        };

        for (stream_arc, env) in deliveries {
            let _ = self.write_envelope(&stream_arc, &env);
        }
    }

    /// Writes a single S2C 8-byte framed envelope to a client socket, auto-compressing large payloads.
    pub fn write_envelope(
        &self,
        stream_arc: &Arc<Mutex<TcpStream>>,
        env: &S2cEnvelope,
    ) -> Result<(), String> {
        let raw_preview = env.to_amf3_bytes().map_err(|e| e.to_string())?;
        let compress = raw_preview.len() >= self.config.compress_s2c_threshold;
        let bytes =
            encode_s2c_single_frame(&self.tx_encoder, env, compress).map_err(|e| e.to_string())?;
        let mut guard = stream_arc
            .lock()
            .map_err(|_| "poisoned TCP stream mutex".to_string())?;
        guard.write_all(&bytes).map_err(|e| e.to_string())?;
        guard.flush().map_err(|e| e.to_string())
    }

    /// Delivers queued peer pushes across active TCP connections.
    pub fn broadcast_peer_pushes(&self, peer_envelopes: Vec<(u64, S2cEnvelope)>) {
        if peer_envelopes.is_empty() {
            return;
        }
        let targets: Vec<(Arc<Mutex<TcpStream>>, S2cEnvelope)> = {
            let Ok(guard) = self.inner.lock() else {
                return;
            };
            peer_envelopes
                .into_iter()
                .filter_map(|(target_uid, env)| {
                    let s = guard.peers.get(&target_uid)?.clone();
                    Some((s, env))
                })
                .collect()
        };
        for (stream_arc, env) in targets {
            let _ = self.write_envelope(&stream_arc, &env);
        }
    }

    /// Routes any GServer (`vnsg2`) RPC request through auth, bootstrap, persistence, and gameplay.
    pub fn handle_rpc(
        &self,
        session: &mut ConnectionSession,
        req: &gserver_protocol::RpcRequest,
    ) -> HandledRpc {
        let now_ts = Self::now_unix_secs();
        let now_i64 = now_ts as i64;

        if req.method == "ping" || req.method == "user.ping" {
            return HandledRpc {
                caller_envelopes: vec![S2cEnvelope::ping().with_time(now_i64)],
                peer_envelopes: Vec::new(),
            };
        }

        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => {
                return Self::err_reply(req, "internal_lock_error", now_i64);
            }
        };

        // ------------------------------------------------------------------
        // Stage A: Pre-Login & Character Creation Handshake
        // ------------------------------------------------------------------
        match req.method.as_str() {
            "user.login" => {
                let login_tbl = req.args.first();
                let pid = login_tbl
                    .and_then(|v| v.get_field("pid"))
                    .and_then(|v| {
                        v.as_str()
                            .map(ToOwned::to_owned)
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                    .unwrap_or_else(|| "win32_guest_0001".to_string());
                let qid = login_tbl
                    .and_then(|v| v.get_field("qid"))
                    .and_then(|v| {
                        v.as_str()
                            .map(ToOwned::to_owned)
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                    .unwrap_or_else(|| pid.clone());
                let hash = login_tbl
                    .and_then(|v| v.get_field("hash"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let raw_server_id = login_tbl
                    .and_then(|v| v.get_field("serverID"))
                    .and_then(Value::as_str)
                    .unwrap_or("game2900161");
                let group_id = raw_server_id
                    .strip_prefix("game")
                    .unwrap_or(raw_server_id)
                    .to_string();

                if self.config.verify_tickets
                    && self.mapi.verify_issued_game_hash(hash, &pid).is_none()
                {
                    return Self::err_reply(req, "invalid_login_hash", now_i64);
                }

                // Ensure account exists in persistent store
                if guard.store.get_account(&pid).is_none() {
                    let uname = format!("acct_{pid}");
                    let _ = guard.store.register_account(
                        pid.clone(),
                        qid.clone(),
                        uname,
                        hash.to_string(),
                        now_ts,
                    );
                }

                guard.token_seq += 1;
                let tok = format!("tok_{}", guard.token_seq);
                session.authenticated_pid = Some(pid);
                session.qid = qid;
                session.group_id = if group_id.is_empty() {
                    self.config.starter.default_group_id.clone()
                } else {
                    group_id
                };
                session.session_token = tok.clone();

                let env = S2cEnvelope::ok_response(&req.callback_name, Value::String("ok".into()))
                    .with_token(tok)
                    .with_time(now_i64);
                return HandledRpc {
                    caller_envelopes: vec![env],
                    peer_envelopes: Vec::new(),
                };
            }

            "user.getUserBeforLogin" => {
                let Some(ref pid) = session.authenticated_pid else {
                    return Self::err_reply(req, "not_authenticated", now_i64);
                };
                let ret_val = match guard.store.get_player_for_account(pid, &session.group_id) {
                    Some(profile) => player_json_to_amf3(&profile.to_vnsg2_user_before_login()),
                    None => Value::empty_array(),
                };
                return Self::ok_reply(req, session, ret_val, now_i64);
            }

            "user.getRandomName" => {
                let count = req
                    .args
                    .first()
                    .and_then(Value::as_i64)
                    .unwrap_or(10)
                    .clamp(1, 20) as usize;
                let gender = req.args.get(1).and_then(Value::as_i64).unwrap_or(1);
                let ret_val = catalog_amf3_to_protocol(&build_random_names_ret(count, gender));
                return Self::ok_reply(req, session, ret_val, now_i64);
            }

            "user.createUser" => {
                let Some(pid) = session.authenticated_pid.clone() else {
                    return Self::err_reply(req, "not_authenticated", now_i64);
                };
                let utid = req.args.first().and_then(Value::as_i64).unwrap_or(1) as u32;
                if protagonist_by_utid(utid).is_none() {
                    return Self::err_reply(req, "invalid_utid", now_i64);
                }
                let uname = req
                    .args
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if guard
                    .gameplay
                    .players
                    .values()
                    .any(|p| p.uname == uname)
                {
                    return Self::err_reply(req, "uname_used", now_i64);
                }

                match guard
                    .store
                    .create_character(&pid, &session.group_id, utid, &uname, now_ts)
                {
                    Ok(profile) => {
                        let uid = profile.uid;
                        // Persist default tutorial completion (`guide_id = 240`) so `MainLoading` reaches 100% cleanly!
                        let _ = guard.store.mutate_player(uid, now_ts, |p| {
                            p.set_user_config("guide_id", PlayerJson::Int(240))?;
                            p.set_user_config("is_first_fighten", PlayerJson::Bool(true))?;
                            p.set_user_config(
                                "opened_copy_chapters",
                                PlayerJson::Array(vec![PlayerJson::Int(1001)]),
                            )
                        });
                        if let Some(updated) = guard.store.get_player(uid) {
                            let gp = profile_to_gameplay_state_with_store(
                                updated,
                                Some(&self.data_store),
                            );
                            guard.gameplay.register_player(gp);
                        }
                        // Seed welcome starter gift in MailBox & Reward Center (`reward.getRewardList`)
                        let _ = guard.gameplay.mail_center.send_system_mail(
                            uid,
                            1,
                            "Quà Chào Mừng Chúa Công GServer",
                            "Chúc Chúa Công chinh phạt Tam Quốc bách chiến bách thắng!",
                            vec![
                                RewardEntry::Gold(10_000),
                                RewardEntry::Item { tid: 16012, qty: 10 },
                            ],
                            now_ts,
                        );
                        self.refresh_mapi_roles(&guard.store, &pid);
                        return Self::ok_reply(
                            req,
                            session,
                            Value::String("ok".into()),
                            now_i64,
                        );
                    }
                    Err(e) => {
                        return Self::err_reply(req, &format!("create_failed:{e}"), now_i64);
                    }
                }
            }

            "user.userLogin" => {
                let Some(pid) = session.authenticated_pid.clone() else {
                    return Self::err_reply(req, "not_authenticated", now_i64);
                };
                let requested_uid = req.args.first().and_then(Value::as_i64).unwrap_or(0) as u64;
                let resolved_uid = if requested_uid > 0 {
                    requested_uid
                } else {
                    guard
                        .store
                        .get_player_for_account(&pid, &session.group_id)
                        .map(|p| p.uid)
                        .unwrap_or(0)
                };

                match guard.store.record_user_login(resolved_uid, now_ts) {
                    Ok(status_str) => {
                        if status_str == "ok" {
                            session.logged_in_uid = Some(resolved_uid);
                            if !guard.gameplay.players.contains_key(&resolved_uid) {
                                let gp_opt = guard.store.get_player(resolved_uid).map(|p| {
                                    profile_to_gameplay_state_with_store(p, Some(&self.data_store))
                                });
                                if let Some(gp) = gp_opt {
                                    guard.gameplay.register_player(gp);
                                }
                            }
                        }
                        return Self::ok_reply(req, session, Value::String(status_str), now_i64);
                    }
                    Err(_) => {
                        return Self::ok_reply(
                            req,
                            session,
                            Value::String("fail".into()),
                            now_i64,
                        );
                    }
                }
            }

            "user.stat" | "user.guideEvent" => {
                return Self::ok_reply(req, session, Value::String("ok".into()), now_i64);
            }

            _ => {}
        }

        // ------------------------------------------------------------------
        // Stage B/C/D/E: Logged-In Bootstrap + Full Gameplay RPCs
        // ------------------------------------------------------------------
        let Some(uid) = session.logged_in_uid else {
            return Self::err_reply(req, "user_not_logged_in", now_i64);
        };

        // Check `PlayerStore`-backed progression, bag, hero, squad, arming, forge & extended RPCs
        let WorldInner {
            ref mut store,
            ref mut gameplay,
            ..
        } = *guard;
        if let Some(player_res) =
            try_handle_player_rpc(store, gameplay, &self.data_store, uid, req, now_ts)
        {
            if req.method == "formation.getOtherPositionInfo" {
                if let Err(ref code) = player_res {
                    if code == "target_player_not_found" {
                        if let Some(target_uid) = req.args.first().and_then(Value::as_i64) {
                            if target_uid > 0 {
                                if let Some(snap_form) =
                                    self.bxh_snapshot.get_other_formation(target_uid as u64)
                                {
                                    return Self::ok_reply(req, session, snap_form, now_i64);
                                }
                            }
                        }
                    }
                }
            }
            return match player_res {
                Ok(ok) => {
                    if let Some(ref pid) = ok.refresh_roles_pid {
                        self.refresh_mapi_roles(store, pid);
                    }
                    let mut main_env =
                        S2cEnvelope::ok_response(&req.callback_name, ok.ret).with_time(now_i64);
                    if !session.session_token.is_empty() {
                        main_env = main_env.with_token(session.session_token.clone());
                    }
                    let mut caller_envelopes = Vec::with_capacity(1 + ok.caller_pushes.len());
                    caller_envelopes.push(main_env);
                    caller_envelopes.extend(ok.caller_pushes);
                    HandledRpc {
                        caller_envelopes,
                        peer_envelopes: ok.peer_pushes,
                    }
                }
                Err(code) => Self::err_reply(req, &code, now_i64),
            };
        }

        match req.method.as_str() {
            "user.getUser" => {
                let Some(profile) = guard.store.get_player(uid) else {
                    return Self::err_reply(req, "player_missing", now_i64);
                };
                let mut user_json = profile.to_vnsg2_get_user();
                if let PlayerJson::Object(ref mut map) = user_json {
                    // Emit `fight_force_max` as numeric so both Lua arithmetic and `tonumber()` succeed!
                    map.insert(
                        "fight_force_max".to_string(),
                        PlayerJson::UInt(profile.fight_force_max),
                    );
                    // `hero_position_map` and `hero_partner_map` are client-side config tables
                    // built by `UserData:initConfig()` with integer keys `1..6` from
                    // `current_config.db` and must not be sent/clobbered by the server.
                    map.remove("hero_position_map");
                    map.remove("hero_partner_map");
                }
                let ret_val = player_json_to_amf3(&user_json);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "switchfunc.getSwitch" => {
                // All function_node IDs with require_lv <= 10 plus starter activity/UI switches
                let unlocked = Value::dense_array(
                    [
                        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16, 18, 20, 24, 25, 30, 31,
                        32, 35, 36, 44, 48, 49, 51, 56, 57, 58, 62, 67, 129, 163, 168, 186, 200,
                        203, 220, 237, 239, 242, 244, 246,
                    ]
                    .into_iter()
                    .map(Value::Int)
                    .collect(),
                );
                Self::ok_reply(req, session, unlocked, now_i64)
            }

            "user.getVaConfig" => {
                let Some(profile) = guard.store.get_player(uid) else {
                    return Self::err_reply(req, "player_missing", now_i64);
                };
                // Start from `VaConfig::default()` (`guide_id = 240`, `is_first_fighten = true`, etc.)
                // and overlay any custom keys stored in `profile.va_config`!
                let mut base = catalog_amf3_to_protocol(&VaConfig::default().to_amf3());
                if let Value::Object(ref mut obj) = base {
                    for (k, v) in &profile.va_config {
                        obj.insert(k.clone(), player_json_to_amf3(v));
                    }
                }
                Self::ok_reply(req, session, base, now_i64)
            }

            "user.setVaConfig" => {
                let Some(k) = req.args.first().and_then(Value::as_str) else {
                    return Self::err_reply(req, "missing_vaconfig_key", now_i64);
                };
                let Some(v) = req.args.get(1) else {
                    return Self::err_reply(req, "missing_vaconfig_val", now_i64);
                };
                let key_owned = k.to_string();
                let pj = amf3_to_player_json(v);
                if let Err(e) = guard
                    .store
                    .mutate_player(uid, now_ts, |p| p.set_user_config(key_owned, pj))
                {
                    return Self::err_reply(req, &format!("vaconfig_failed:{e}"), now_i64);
                }
                Self::ok_reply(req, session, Value::String("ok".into()), now_i64)
            }

            "formation.get" => {
                let Some(profile) = guard.store.get_player(uid) else {
                    return Self::err_reply(req, "player_missing", now_i64);
                };
                // `FormationData:parseData` iterates `pairs(toarray(data["formation"]))` and
                // `FormationItem:addArmature` requires every hid in `formation` to exist in `allHeros` (no `0`s!).
                // Battle grid cells follow `attack_sep_list = {3, 6, 9, 2, 5, 8}`.
                const DEFAULT_CELLS: [u8; 6] = [3, 6, 9, 2, 5, 8];
                let mut grid_entries: Vec<(String, Value)> = profile
                    .formation
                    .grid_formation
                    .iter()
                    .filter(|(_, &hid)| hid > 0 && profile.heroes.contains_key(&hid))
                    .map(|(cell, &hid)| (cell.to_string(), u64_to_amf3(hid)))
                    .collect();
                if grid_entries.is_empty() {
                    for (idx, slot) in (1..=6u8).enumerate() {
                        if let Some(&hid) = profile.formation.position.get(&slot) {
                            if hid > 0 && profile.heroes.contains_key(&hid) {
                                grid_entries.push((DEFAULT_CELLS[idx].to_string(), u64_to_amf3(hid)));
                            }
                        }
                    }
                }
                let formation_val = if grid_entries.is_empty() {
                    Value::empty_array()
                } else {
                    Value::map(grid_entries.clone())
                };
                let grid_val = if grid_entries.is_empty() {
                    Value::empty_array()
                } else {
                    Value::map(grid_entries)
                };
                let ret_val = Value::map([
                    ("formation", formation_val),
                    ("grid_formation", grid_val),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "formation.getPositionInfo" => {
                let Some(profile) = guard.store.get_player(uid) else {
                    return Self::err_reply(req, "player_missing", now_i64);
                };
                let mut pos_json = profile.formation.to_vnsg2_position_info(&profile.heroes);
                // Keep `position` occupied-only (no 0 entries, because HeroFormationData.lua:185-188
                // iterates `pairs(self["position"])` without skipping hid == 0)
                // AND ensure `allHeros` contains every hero in `profile.heroes`!
                if let PlayerJson::Object(ref mut map) = pos_json {
                    let mut heros_map = BTreeMap::new();
                    for (&hid, hero) in &profile.heroes {
                        heros_map.insert(hid.to_string(), hero.to_vnsg2_hero_info());
                    }
                    map.insert("allHeros".to_string(), PlayerJson::Object(heros_map));
                }
                let ret_val = player_json_to_amf3(&pos_json);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "bag.bagInfo" => {
                let Some(profile) = guard.store.get_player(uid) else {
                    return Self::err_reply(req, "player_missing", now_i64);
                };
                let ret_val = player_json_to_amf3(&profile.inventory.to_vnsg2_bag_info());
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "copy.getCopy" => {
                let chap_id = req
                    .args
                    .first()
                    .and_then(Value::as_i64)
                    .unwrap_or(1001)
                    .max(1) as u32;
                let gp_dto = guard.gameplay.copy.get_chapter(uid, chap_id);
                let mut ret_val = dto_to_amf3(&gp_dto, true);
                // Populate `va_copy` required by `CopyInfoDataController` & `MainScene:isGuideFirstFight`!
                let first_stage = format!("{chap_id}01");
                let mut va_copy_entries: Vec<(String, Value)> = Vec::new();
                if let Some(info_map) = gp_dto.get("va_copy_info").and_then(|v| v.as_map()) {
                    for (stage_k, rec) in info_map {
                        let stars = rec.get("star").and_then(|s| s.as_i64()).unwrap_or(3).max(1);
                        va_copy_entries.push((
                            stage_k.clone(),
                            Value::map([
                                ("reset", Value::Int(0)),
                                ("normal_star", Value::Int(stars as i32)),
                                ("star_status", Value::Int(0)),
                                ("pass", Value::Int(1)),
                                ("box_reward", Value::Int(0)),
                            ]),
                        ));
                    }
                }
                if !va_copy_entries.iter().any(|(k, _)| k == &first_stage) {
                    va_copy_entries.push((
                        first_stage,
                        Value::map([
                            ("reset", Value::Int(0)),
                            ("normal_star", Value::Int(3)),
                            ("star_status", Value::Int(0)),
                            ("pass", Value::Int(1)),
                            ("box_reward", Value::Int(0)),
                        ]),
                    ));
                }
                if let Value::Object(ref mut obj) = ret_val {
                    obj.insert("va_copy", Value::map(va_copy_entries));
                }
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "halidom.getInfo" => {
                let ret_val = Value::map([
                    ("current", Value::Int(1)),
                    ("halidomInfo", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "allActivity.getActivityInfo" => {
                let ret_val = Value::map([("activity_groupon", Value::empty_array())]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "opencarnival.getOpenAchieve"
            | "lordwar.getTempleInfo"
            | "crossClanBattle.getTempleInfo"
            | "crossClanBattle.getPromotionInfo"
            | "activityAnswer.getInfo" => {
                Self::ok_reply(req, session, Value::empty_array(), now_i64)
            }

            "smallgame.getInfo" => {
                let ret_val = catalog_amf3_to_protocol(&EnterViewSnapshot::default().to_amf3());
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "barrage.get" => {
                let ret_val = catalog_amf3_to_protocol(&BarrageState::default().to_amf3());
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "barrage.enterRoom" => Self::ok_reply(req, session, Value::Bool(true), now_i64),

            "barrage.getBarrageRecord" => {
                Self::ok_reply(req, session, Value::empty_array(), now_i64)
            }

            "dota.getInfo" => {
                let ret_val = Value::map([
                    ("normal_layer", Value::Int(0)),
                    ("normal_max_layer", Value::Int(0)),
                    ("is_open", Value::Int(0)),
                    ("defeat_num", Value::Int(0)),
                    ("normal_reset_num", Value::Int(0)),
                    ("normal_buy_attack_num", Value::Int(0)),
                    ("normal_buy_attack_num_used", Value::Int(0)),
                    ("normal_max_attack_layer", Value::Int(0)),
                    ("elite_attack_num", Value::Int(3)),
                    ("elite_max_layer", Value::Int(0)),
                    ("elite_buy_attack_num", Value::Int(0)),
                    ("elite_cur_layer", Value::Int(0)),
                    ("purgatory_max_layer", Value::Int(0)),
                    ("purgatory_buy_time", Value::Int(0)),
                    ("purgatory_attack_time", Value::Int(0)),
                    ("purgatory_last_layer", Value::Int(1)),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "achievements.getShowAchievements" => {
                let ret_val = Value::map([
                    ("point", Value::Int(0)),
                    ("achievements", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "discover.getRewardCity" => {
                let ret_val = Value::map([
                    ("reward", Value::empty_array()),
                    ("pass_city", Value::empty_array()),
                    ("kill_num", Value::empty_array()),
                    ("elite_max_layer", Value::Int(0)),
                    ("elite_cur_layer", Value::Int(0)),
                    ("elite_attack_num", Value::Int(3)),
                    ("elite_buy_attack_num", Value::Int(0)),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "praise.getPraiseInfo" => {
                let ret_val = Value::map([
                    ("praise_num", Value::Int(0)),
                    ("va_praise", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "lordwar.getPromotionInfo" => {
                let ret_val = Value::map([
                    ("round", Value::Int(0)),
                    ("winSubround", Value::Int(0)),
                    ("loseSubround", Value::Int(0)),
                    ("status", Value::Int(0)),
                    ("loseLord", Value::empty_array()),
                    ("winLord", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "awakeCopy.getAwakeCopyInfo" => {
                let ret_val = Value::map([
                    ("pass_str", Value::empty_array()),
                    ("last_attack_num", Value::Int(3)),
                    ("last_help_num", Value::Int(3)),
                    ("buy_num", Value::Int(0)),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "activityRechargeRebate.getInfo" => Self::ok_reply(
                req,
                session,
                Value::String("activity_close".to_string()),
                now_i64,
            ),

            "brochureStrategy.getInfo" => {
                let ret_val = Value::map([
                    ("brochure_strategy_level", Value::Int(0)),
                    ("va_brochure_strategy", Value::empty_array()),
                    ("va_get_brochure_strategy", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "collection.getInfo" => {
                let score_reward_status = Value::map([
                    ("page_1", Value::empty_array()),
                    ("page_2", Value::empty_array()),
                    ("page_3", Value::empty_array()),
                    ("page_4", Value::empty_array()),
                    ("page_5", Value::empty_array()),
                ]);
                let ret_val = Value::map([
                    ("1", Value::empty_array()),
                    ("2", Value::empty_array()),
                    ("3", Value::empty_array()),
                    ("4", Value::empty_array()),
                    ("5", Value::empty_array()),
                    ("6", Value::empty_array()),
                    ("7", Value::empty_array()),
                    ("8", Value::empty_array()),
                    ("9", Value::empty_array()),
                    ("10", Value::empty_array()),
                    ("scoreRewardStatus", score_reward_status),
                    ("likedRewardStatus", Value::empty_array()),
                    ("likedNum", Value::Int(0)),
                    ("todayLikeNum", Value::Int(0)),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "dailyDiscount.getInfo" => {
                let ret_val = Value::map([
                    ("productBuy", Value::empty_array()),
                    ("canRecDayPreBuy", Value::empty_array()),
                    ("redeem", Value::empty_array()),
                    ("canDayPreBuy", Value::Bool(false)),
                    ("dayRecNum", Value::Int(0)),
                    ("dayPreBuyTomorrow", Value::Bool(false)),
                    ("rechargeSum", Value::Int(0)),
                    ("rechargePointRec", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "generalBook.getInfo" => {
                let book_entry = || {
                    Value::map([
                        ("level", Value::Int(1)),
                        ("total_exp", Value::Int(0)),
                        ("star_exp", Value::empty_array()),
                    ])
                };
                let ret_val = Value::map([
                    ("1", book_entry()),
                    ("2", book_entry()),
                    ("3", book_entry()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "dailytask.getInfo" => {
                let level = guard
                    .store
                    .get_player(uid)
                    .map(|p| p.level as i32)
                    .unwrap_or(1);
                let ret_val = Value::map([
                    ("super_checkin_score", Value::Int(0)),
                    ("active_point", Value::Int(0)),
                    ("level", Value::Int(level)),
                    ("va_receive_point", Value::empty_array()),
                    ("dailytask", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "friend.getFriendInfo" => {
                let ret_val = Value::map([
                    ("friendList", Value::empty_array()),
                    ("blackList", Value::empty_array()),
                    ("todayPresentUids", Value::empty_array()),
                    ("todayReceiveTimes", Value::Int(0)),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "growth.getInfo" => {
                let ret_val = Value::map([
                    ("userNum", Value::Int(0)),
                    ("is_buy", Value::Int(0)),
                    ("va_gold", Value::empty_array()),
                    ("va_benefit", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "growth.getNum" => Self::ok_reply(req, session, Value::Int(0), now_i64),

            "envelopRain.getInfo" => Self::ok_reply(req, session, Value::Bool(false), now_i64),

            "activityStarPlaySignOpen.getInfo" => {
                let ret_val = Value::map([("maked", Value::empty_array())]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "job.getInfo" => {
                let ret_val = Value::map([
                    ("job", Value::Int(1)),
                    ("level", Value::Int(1)),
                    ("name", Value::Int(1)),
                    ("va_skill", Value::empty_array()),
                    ("va_equip", Value::empty_array()),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "recruitment.getInfo" => {
                let ret_val = Value::map([
                    ("reward", Value::empty_array()),
                    ("num", Value::Int(0)),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "firstGift.getRechargeGold" => Self::ok_reply(req, session, Value::Int(0), now_i64),

            "dressBrightBook.getInfo" => {
                let ret_val = Value::map([
                    ("attr_list", Value::empty_array()),
                    (
                        "book_list",
                        Value::map((1..=12).map(|id| (id.to_string(), Value::empty_array()))),
                    ),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "activityDailyRecharge2.getInfo" => {
                let ret_val = Value::map([("buyInfo", Value::empty_array())]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "top.get" => {
                let tab = req
                    .args
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("fight_force_max");
                let self_profile = guard.store.get_player(uid).cloned();
                let local_profiles = Self::collect_all_persisted_profiles(&guard.store);
                let ret_val = self.bxh_snapshot.build_top_get_reply(
                    tab,
                    self_profile.as_ref(),
                    &local_profiles,
                );
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "top.getWorldFightForceRank" => {
                let self_profile = guard.store.get_player(uid).cloned();
                let local_profiles = Self::collect_all_persisted_profiles(&guard.store);
                let ret_val = self.bxh_snapshot.build_world_fight_force_reply(
                    self_profile.as_ref(),
                    &local_profiles,
                );
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "top.getUserInfo" => {
                let target_uid = req.args.first().and_then(Value::as_i64).unwrap_or(0) as u64;
                if let Some(info) = self.bxh_snapshot.get_user_info(target_uid) {
                    return Self::ok_reply(req, session, info, now_i64);
                }
                if let Some(p) = guard.store.get_player(target_uid) {
                    let ret_val = Value::map([
                        ("uid", u64_to_amf3(p.uid)),
                        ("uname", Value::String(p.uname.clone())),
                        ("level", Value::Int(p.level as i32)),
                        ("vip", Value::Int(p.vip as i32)),
                        ("fight_force", u64_to_amf3(p.fc)),
                        ("utid", Value::Int(p.utid as i32)),
                    ]);
                    return Self::ok_reply(req, session, ret_val, now_i64);
                }
                Self::ok_reply(req, session, Value::empty_array(), now_i64)
            }

            "top.getSelf" => {
                let tab = req
                    .args
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("fight_force_max");
                let self_profile = guard.store.get_player(uid).cloned();
                let local_profiles = Self::collect_all_persisted_profiles(&guard.store);
                let full = self.bxh_snapshot.build_top_get_reply(
                    tab,
                    self_profile.as_ref(),
                    &local_profiles,
                );
                let ret_val = full
                    .get_field("self")
                    .cloned()
                    .unwrap_or_else(Value::empty_array);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            _ => {
                // Route all Town, Chat/GM, Mail, Reward, Raffle, Shop, Copy/Battle, Arena, Sign & Online RPCs!
                let gp_req = to_gameplay_request(req);
                let outcome = guard.gameplay.dispatch(uid, &gp_req, now_ts);

                if outcome.response.err == "unsupported_method" {
                    if self.config.strict_rpc {
                        return Self::err_reply(req, "unsupported_method", now_i64);
                    }
                    return Self::ok_reply(req, session, Value::empty_array(), now_i64);
                }

                // Persist state changes back to `PlayerStore` (CRC32 WAL + atomic player JSON)
                if outcome.response.err == "ok" {
                    if let Some(gp_state) = guard.gameplay.players.get(&uid).cloned() {
                        let pid_opt = guard.store.get_player(uid).map(|p| p.pid.clone());
                        let _ = guard.store.mutate_player(uid, now_ts, |profile| {
                            sync_gameplay_back_to_profile(&gp_state, profile, now_ts);
                            Ok(())
                        });
                        if let Some(ref pid) = pid_opt {
                            self.refresh_mapi_roles(&guard.store, pid);
                        }
                    }
                }

                let mut caller_envelopes = Vec::with_capacity(1 + outcome.client_pushes.len());
                let main_env = if outcome.response.err == "ok" {
                    S2cEnvelope::ok_response(
                        &req.callback_name,
                        dto_to_amf3(&outcome.response.ret, true),
                    )
                } else {
                    S2cEnvelope::error_response(&req.callback_name, outcome.response.err)
                }
                .with_token(session.session_token.clone())
                .with_time(now_i64);
                caller_envelopes.push(main_env);

                for cp in outcome.client_pushes {
                    caller_envelopes.push(
                        S2cEnvelope::push(cp.callback_name, dto_to_amf3(&cp.ret, true))
                            .with_time(cp.time as i64),
                    );
                }

                let peer_envelopes = outcome
                    .peer_pushes
                    .into_iter()
                    .map(|(target_uid, pp)| {
                        (
                            target_uid,
                            S2cEnvelope::push(pp.callback_name, dto_to_amf3(&pp.ret, true))
                                .with_time(pp.time as i64),
                        )
                    })
                    .collect();

                HandledRpc {
                    caller_envelopes,
                    peer_envelopes,
                }
            }
        }
    }

    fn refresh_mapi_roles(&self, store: &PlayerStore, pid: &str) {
        if let Some(acc) = store.get_account(pid) {
            let mut roles = Vec::new();
            for (gid, &uid) in &acc.characters {
                if let Some(p) = store.get_player(uid) {
                    roles.push(RoleSummary {
                        uid: p.uid,
                        uname: p.uname.clone(),
                        level: p.level,
                        quality: 6,
                        groupid: gid.clone(),
                        groupname: format!("S161.Võ Xương ({gid})"),
                        maintainflag: 0,
                        vip: p.vip,
                    });
                }
            }
            self.mapi.set_roles_for_pid(pid.to_string(), roles);
        }
    }

    fn ok_reply(
        req: &gserver_protocol::RpcRequest,
        session: &ConnectionSession,
        ret: Value,
        now_i64: i64,
    ) -> HandledRpc {
        let mut env = S2cEnvelope::ok_response(&req.callback_name, ret).with_time(now_i64);
        if !session.session_token.is_empty() {
            env = env.with_token(session.session_token.clone());
        }
        HandledRpc {
            caller_envelopes: vec![env],
            peer_envelopes: Vec::new(),
        }
    }

    fn err_reply(req: &gserver_protocol::RpcRequest, code: &str, now_i64: i64) -> HandledRpc {
        HandledRpc {
            caller_envelopes: vec![
                S2cEnvelope::error_response(&req.callback_name, code).with_time(now_i64)
            ],
            peer_envelopes: Vec::new(),
        }
    }
}

fn gp_register_and_role(
    profile: &PlayerProfile,
    data_store: &GameDataStore,
    gameplay: &mut GameplayEngine,
    roles_by_pid: &mut HashMap<String, Vec<RoleSummary>>,
) {
    gameplay.register_player(profile_to_gameplay_state_with_store(
        profile,
        Some(data_store),
    ));
    roles_by_pid
        .entry(profile.pid.clone())
        .or_default()
        .push(RoleSummary {
            uid: profile.uid,
            uname: profile.uname.clone(),
            level: profile.level,
            quality: 6,
            groupid: profile.group_id.clone(),
            groupname: format!("S161.Võ Xương ({})", profile.group_id),
            maintainflag: 0,
            vip: profile.vip,
        });
}

fn amf3_to_player_json(val: &Value) -> PlayerJson {
    match val {
        Value::Undefined | Value::Null => PlayerJson::Null,
        Value::Bool(b) => PlayerJson::Bool(*b),
        Value::Int(i) => PlayerJson::Int(i64::from(*i)),
        Value::Double(d) => {
            if d.fract() == 0.0 && *d >= 0.0 && *d <= (u64::MAX as f64) {
                PlayerJson::UInt(*d as u64)
            } else {
                PlayerJson::Float(*d)
            }
        }
        Value::String(s) => PlayerJson::Str(s.clone()),
        Value::ByteString(b) | Value::ByteArray(b) | Value::Xml(b) | Value::XmlDoc(b) => {
            PlayerJson::Str(String::from_utf8_lossy(b).into_owned())
        }
        Value::Date(ms) => PlayerJson::Float(*ms),
        Value::Array { dense, assoc } => {
            if assoc.is_empty() {
                PlayerJson::Array(dense.iter().map(amf3_to_player_json).collect())
            } else {
                let mut m = BTreeMap::new();
                for (i, v) in dense.iter().enumerate() {
                    m.insert((i + 1).to_string(), amf3_to_player_json(v));
                }
                for (k, v) in assoc {
                    m.insert(k.clone(), amf3_to_player_json(v));
                }
                PlayerJson::Object(m)
            }
        }
        Value::Object(obj) => {
            let mut m = BTreeMap::new();
            for (k, v) in obj.iter() {
                m.insert(k.to_string(), amf3_to_player_json(v));
            }
            PlayerJson::Object(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gserver_http::ServerConfig as HttpMapiConfig;
    use gserver_protocol::RpcRequest;

    #[test]
    fn user_get_user_excludes_hero_position_map_and_hero_partner_map() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "gserver_world_get_user_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).expect("create temp test dir");

        let cfg = DaemonConfig {
            data_dir: tmp_dir.clone(),
            verify_tickets: false,
            ..DaemonConfig::default()
        };
        let mapi = Arc::new(MapiService::new(HttpMapiConfig::default()));
        let world = ServerWorld::open(cfg, mapi).expect("open ServerWorld");

        let mut session = ConnectionSession::default();
        let login = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "user.login",
                2,
                vec![Value::map([
                    ("pid", Value::String("pid_userdata_932".into())),
                    ("serverID", Value::String("game2900161".into())),
                ])],
                "0",
            ),
        );
        assert_eq!(login.caller_envelopes[0].err, "ok");

        let tok = session.session_token.clone();
        let create = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "user.createUser",
                3,
                vec![Value::Int(1), Value::String("UserDataGuard".into())],
                tok.as_str(),
            ),
        );
        assert_eq!(create.caller_envelopes[0].err, "ok");

        let before = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id("user.getUserBeforLogin", 4, vec![], tok.as_str()),
        );
        let uid = before.caller_envelopes[0]
            .ret
            .get_field("uid")
            .and_then(Value::as_i64)
            .expect("created uid");

        let ulogin = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "user.userLogin",
                5,
                vec![Value::Int(uid as i32)],
                tok.as_str(),
            ),
        );
        assert_eq!(ulogin.caller_envelopes[0].ret.as_str(), Some("ok"));

        let get_user = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id("user.getUser", 6, vec![], tok.as_str()),
        );
        assert_eq!(get_user.caller_envelopes[0].err, "ok");
        let ret = &get_user.caller_envelopes[0].ret;
        assert_eq!(ret.get_field("uid").and_then(Value::as_i64), Some(uid));
        assert!(
            ret.get_field("hero_position_map").is_none(),
            "user.getUser must not contain hero_position_map (clobbers UserData:initConfig integer keys at UserData.lua:931-932)"
        );
        assert!(
            ret.get_field("hero_partner_map").is_none(),
            "user.getUser must not contain hero_partner_map (client-side UserData:initConfig table)"
        );

        let gm_lv = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "chat.sendWorld",
                7,
                vec![Value::Int(1), Value::String("/level 55".into())],
                tok.as_str(),
            ),
        );
        for env in &gm_lv.caller_envelopes {
            assert!(
                env.ret.get_field("hero_position_map").is_none(),
                "push {:?} must not contain hero_position_map",
                env.callback_name
            );
            assert!(
                env.ret.get_field("hero_partner_map").is_none(),
                "push {:?} must not contain hero_partner_map",
                env.callback_name
            );
        }

        drop(world);
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn server_world_copy_arena_and_skill_level_up_use_authentic_hero_skills() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "gserver_world_hero_skills_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).expect("create temp test dir");

        let cfg = DaemonConfig {
            data_dir: tmp_dir.clone(),
            verify_tickets: false,
            ..DaemonConfig::default()
        };
        let mapi = Arc::new(MapiService::new(HttpMapiConfig::default()));
        let world = ServerWorld::open(cfg, mapi).expect("open ServerWorld");

        let mut session = ConnectionSession::default();
        let _ = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "user.login",
                1,
                vec![Value::map([
                    ("pid", Value::String("pid_skill_e2e".into())),
                    ("serverID", Value::String("game2900161".into())),
                ])],
                "0",
            ),
        );
        let tok = session.session_token.clone();
        let _ = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "user.createUser",
                2,
                vec![Value::Int(2), Value::String("ChuaCongNu".into())],
                tok.as_str(),
            ),
        );
        let before = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id("user.getUserBeforLogin", 3, vec![], tok.as_str()),
        );
        let uid = before.caller_envelopes[0]
            .ret
            .get_field("uid")
            .and_then(Value::as_i64)
            .unwrap();
        let _ = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "user.userLogin",
                4,
                vec![Value::Int(uid as i32)],
                tok.as_str(),
            ),
        );

        // `hero.skillLevelUp` with omitted skill_id defaults to hero's authentic `com_atk` (not 1001!)
        let heroes_ret = world
            .handle_rpc(
                &mut session,
                &RpcRequest::with_seq_id("hero.getAllHeroes", 5, vec![], tok.as_str()),
            )
            .caller_envelopes[0]
            .ret
            .clone();
        let mut zhaoyun_hid = 0i32;
        if let Value::Object(ref map) = heroes_ret {
            for (_k, v) in map.iter() {
                if v.get_field("htid").and_then(Value::as_i64) == Some(201) {
                    zhaoyun_hid = v.get_field("hid").and_then(Value::as_i64).unwrap() as i32;
                }
            }
        }
        assert!(
            zhaoyun_hid > 0,
            "DaemonConfig::default grants starter Triệu Vân (htid=201)"
        );

        let sk_up = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "hero.skillLevelUp",
                6,
                vec![Value::Int(zhaoyun_hid)],
                tok.as_str(),
            ),
        );
        assert_eq!(sk_up.caller_envelopes[0].err, "ok");

        let heroes_after = world
            .handle_rpc(
                &mut session,
                &RpcRequest::with_seq_id("hero.getAllHeroes", 7, vec![], tok.as_str()),
            )
            .caller_envelopes[0]
            .ret
            .clone();
        let zy_dto = heroes_after
            .get_field(&zhaoyun_hid.to_string())
            .expect("Triệu Vân DTO");
        if let Some(Value::Array { dense, .. }) = zy_dto.get_field("talent") {
            assert_eq!(dense, &vec![Value::Int(20102)]);
        } else {
            panic!("expected talent array with authentic com_atk 20102");
        }

        // `copy.attack` over ServerWorld emits authentic `arrSkill` for Nữ Chủ Tướng (104/103), Triệu Vân (20102/20101), Điêu Thuyền (40602/40601)
        let atk = world.handle_rpc(
            &mut session,
            &RpcRequest::with_seq_id(
                "copy.attack",
                8,
                vec![Value::Int(1001), Value::Int(100101)],
                tok.as_str(),
            ),
        );
        assert_eq!(atk.caller_envelopes[0].err, "ok");
        let arr_hero = atk.caller_envelopes[0]
            .ret
            .get_field("battleInfo")
            .and_then(|b| b.get_field("arrTeam1"))
            .and_then(|t| t.get_field("1"))
            .and_then(|t| t.get_field("arrHero"))
            .expect("copy.attack arrTeam1.1.arrHero");
        for (slot_key, exp_htid, exp_com, exp_angry) in [
            ("1", 2, 104, 103),
            ("2", 201, 20102, 20101),
            ("3", 406, 40602, 40601),
        ] {
            let u = arr_hero.get_field(slot_key).expect("deployed hero unit");
            assert_eq!(u.get_field("htid").and_then(Value::as_i64), Some(exp_htid));
            if let Some(Value::Array { dense, .. }) = u.get_field("arrSkill") {
                if let (
                    Some(Value::Array { dense: s0, .. }),
                    Some(Value::Array { dense: s1, .. }),
                ) = (dense.first(), dense.get(1))
                {
                    assert_eq!(s0.first().and_then(Value::as_i64), Some(exp_com));
                    assert_eq!(s1.first().and_then(Value::as_i64), Some(exp_angry));
                } else {
                    panic!("invalid nested arrSkill entries");
                }
            } else {
                panic!("missing arrSkill array");
            }
        }

        drop(world);
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
