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
    amf3_to_player_json, catalog_amf3_to_protocol, dto_push_at, dto_to_amf3,
    player_json_to_amf3, profile_to_gameplay_state, sync_gameplay_back_to_profile,
    to_gameplay_request, u64_to_amf3,
};
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

        let mut gameplay = GameplayEngine::new(0x4F4D_4733_515F_564E);
        let mut roles_by_pid: HashMap<String, Vec<RoleSummary>> = HashMap::new();
        for profile in Self::collect_all_persisted_profiles(&store) {
            gameplay.register_player(profile_to_gameplay_state(&profile));
            roles_by_pid
                .entry(profile.pid.clone())
                .or_default()
                .push(role_summary_from_profile(&profile, profile.group_id.clone()));
        }
        for (pid, roles) in roles_by_pid {
            mapi.set_roles_for_pid(pid, roles);
        }

        Ok(Arc::new(Self {
            config,
            mapi,
            data_store,
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
                if let Some(uid) = entry
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    if let Some(p) = store.get_player(uid) {
                        out.push(p.clone());
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
        let peer_envelopes: Vec<(u64, S2cEnvelope)> = {
            let Ok(mut guard) = self.inner.lock() else {
                return;
            };
            guard.peers.remove(&uid);
            let (_, bcasts) = guard.gameplay.town.leave_town(uid, now_ts);
            bcasts
                .into_iter()
                .map(|(target_uid, pkt)| {
                    let ts = pkt.time as i64;
                    (target_uid, dto_push_at(pkt, ts))
                })
                .collect()
        };
        self.broadcast_peer_pushes(peer_envelopes);
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
                    Some((guard.peers.get(&target_uid)?.clone(), env))
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

        let Ok(mut guard) = self.inner.lock() else {
            return Self::err_reply(req, "internal_lock_error", now_i64);
        };

        // Stage A: Pre-Login & Character Creation Handshake
        match req.method.as_str() {
            "user.login" => {
                let tbl = req.args.first();
                let pid = field_str_or_int(tbl, "pid")
                    .unwrap_or_else(|| "win32_guest_0001".to_string());
                let qid = field_str_or_int(tbl, "qid").unwrap_or_else(|| pid.clone());
                let hash = field_str(tbl, "hash", "");
                let raw_server_id = field_str(tbl, "serverID", "game2900161");
                let group_id = raw_server_id
                    .strip_prefix("game")
                    .unwrap_or(raw_server_id)
                    .to_string();

                if self.config.verify_tickets
                    && self.mapi.verify_issued_game_hash(hash, &pid).is_none()
                {
                    return Self::err_reply(req, "invalid_login_hash", now_i64);
                }

                if guard.store.get_account(&pid).is_none() {
                    let _ = guard.store.register_account(
                        pid.clone(),
                        qid.clone(),
                        format!("acct_{pid}"),
                        hash.to_string(),
                        now_ts,
                    );
                }

                guard.token_seq += 1;
                session.authenticated_pid = Some(pid);
                session.qid = qid;
                session.group_id = if group_id.is_empty() {
                    self.config.starter.default_group_id.clone()
                } else {
                    group_id
                };
                session.session_token = format!("tok_{}", guard.token_seq);
                return Self::ok_reply(req, session, Value::String("ok".into()), now_i64);
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
                if guard.gameplay.players.values().any(|p| p.uname == uname) {
                    return Self::err_reply(req, "uname_used", now_i64);
                }

                return match guard
                    .store
                    .create_character(&pid, &session.group_id, utid, &uname, now_ts)
                {
                    Ok(profile) => {
                        let uid = profile.uid;
                        let _ = guard.store.mutate_player(uid, now_ts, |p| {
                            p.set_user_config("guide_id", PlayerJson::Int(240))?;
                            p.set_user_config("is_first_fighten", PlayerJson::Bool(true))?;
                            p.set_user_config(
                                "opened_copy_chapters",
                                PlayerJson::Array(vec![PlayerJson::Int(1001)]),
                            )
                        });
                        let WorldInner {
                            ref store,
                            ref mut gameplay,
                            ..
                        } = *guard;
                        sync_player_into_gameplay(store, gameplay, uid);
                        let _ = gameplay.mail_center.send_system_mail(
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
                        Self::ok_reply(req, session, Value::String("ok".into()), now_i64)
                    }
                    Err(e) => Self::err_reply(req, &format!("create_failed:{e}"), now_i64),
                };
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

                return match guard.store.record_user_login(resolved_uid, now_ts) {
                    Ok(status_str) => {
                        if status_str == "ok" {
                            session.logged_in_uid = Some(resolved_uid);
                            if !guard.gameplay.players.contains_key(&resolved_uid) {
                                let WorldInner {
                                    ref store,
                                    ref mut gameplay,
                                    ..
                                } = *guard;
                                sync_player_into_gameplay(store, gameplay, resolved_uid);
                            }
                        }
                        Self::ok_reply(req, session, Value::String(status_str), now_i64)
                    }
                    Err(_) => Self::ok_reply(req, session, Value::String("fail".into()), now_i64),
                };
            }

            "user.stat" | "user.guideEvent" => {
                return Self::ok_reply(req, session, Value::String("ok".into()), now_i64);
            }

            _ => {}
        }

        // Stage B/C/D/E: Logged-In Bootstrap + Full Gameplay RPCs
        let Some(uid) = session.logged_in_uid else {
            return Self::err_reply(req, "user_not_logged_in", now_i64);
        };

        let WorldInner {
            ref mut store,
            ref mut gameplay,
            ..
        } = *guard;
        if let Some(player_res) = try_handle_player_rpc(store, gameplay, uid, req, now_ts) {
            return match player_res {
                Ok(ok) => {
                    if let Some(ref pid) = ok.refresh_roles_pid {
                        self.refresh_mapi_roles(store, pid);
                    }
                    let main_env = Self::stamp_session(
                        S2cEnvelope::ok_response(&req.callback_name, ok.ret),
                        session,
                        now_i64,
                    );
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
                    map.insert(
                        "fight_force_max".to_string(),
                        PlayerJson::UInt(profile.fight_force_max),
                    );
                    let unlock_levels = [1u32, 1, 10, 18, 26, 32];
                    let pos_map = unlock_levels
                        .iter()
                        .enumerate()
                        .map(|(i, &req_lv)| {
                            ((i + 1).to_string(), PlayerJson::Bool(profile.level >= req_lv))
                        })
                        .collect();
                    map.insert("hero_position_map".to_string(), PlayerJson::Object(pos_map));
                }
                Self::ok_reply(req, session, player_json_to_amf3(&user_json), now_i64)
            }

            "switchfunc.getSwitch" => {
                let unlocked = Value::dense_array(
                    [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16, 18, 20]
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
                let slots: Vec<Value> = (1..=6u8)
                    .map(|slot| {
                        u64_to_amf3(profile.formation.position.get(&slot).copied().unwrap_or(0))
                    })
                    .collect();
                let grid_entries: Vec<(String, Value)> = profile
                    .formation
                    .grid_formation
                    .iter()
                    .map(|(cell, &hid)| (cell.to_string(), u64_to_amf3(hid)))
                    .collect();
                let grid_val = if grid_entries.is_empty() {
                    Value::empty_array()
                } else {
                    Value::map(grid_entries)
                };
                let ret_val = Value::map([
                    ("formation", Value::dense_array(slots)),
                    ("grid_formation", grid_val),
                ]);
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "formation.getPositionInfo" => {
                let Some(profile) = guard.store.get_player(uid) else {
                    return Self::err_reply(req, "player_missing", now_i64);
                };
                let mut pos_json = profile.formation.to_vnsg2_position_info(&profile.heroes);
                if let PlayerJson::Object(ref mut map) = pos_json {
                    let slots: Vec<PlayerJson> = (1..=6u8)
                        .map(|slot| {
                            let hid = profile.formation.position.get(&slot).copied().unwrap_or(0);
                            PlayerJson::UInt(if profile.heroes.contains_key(&hid) {
                                hid
                            } else {
                                0
                            })
                        })
                        .collect();
                    map.insert("position".to_string(), PlayerJson::Array(slots));
                    let heros_map: BTreeMap<String, PlayerJson> = profile
                        .heroes
                        .iter()
                        .map(|(&hid, hero)| (hid.to_string(), hero.to_vnsg2_hero_info()))
                        .collect();
                    map.insert("allHeros".to_string(), PlayerJson::Object(heros_map));
                }
                Self::ok_reply(req, session, player_json_to_amf3(&pos_json), now_i64)
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
                let first_stage = format!("{chap_id}01");
                let mut va_copy_entries: Vec<(String, Value)> = gp_dto
                    .get("va_copy_info")
                    .and_then(|v| v.as_map())
                    .map(|info_map| {
                        info_map
                            .iter()
                            .map(|(stage_k, rec)| {
                                let stars =
                                    rec.get("star").and_then(|s| s.as_i64()).unwrap_or(3).max(1);
                                (stage_k.clone(), build_va_copy_stage(stars as i32))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !va_copy_entries.iter().any(|(k, _)| k == &first_stage) {
                    va_copy_entries.push((first_stage, build_va_copy_stage(3)));
                }
                if let Value::Object(ref mut obj) = ret_val {
                    obj.insert("va_copy", Value::map(va_copy_entries));
                }
                Self::ok_reply(req, session, ret_val, now_i64)
            }

            "allActivity.getActivityInfo"
            | "halidom.getInfo"
            | "opencarnival.getOpenAchieve"
            | "lordwar.getTempleInfo"
            | "crossClanBattle.getTempleInfo"
            | "crossClanBattle.getPromotionInfo"
            | "activityAnswer.getInfo"
            | "barrage.getBarrageRecord" => {
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

            _ => {
                let gp_req = to_gameplay_request(req);
                let outcome = guard.gameplay.dispatch(uid, &gp_req, now_ts);

                if outcome.response.err == "unsupported_method" {
                    if self.config.strict_rpc {
                        return Self::err_reply(req, "unsupported_method", now_i64);
                    }
                    return Self::ok_reply(req, session, Value::empty_array(), now_i64);
                }

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

                let raw_main = if outcome.response.err == "ok" {
                    S2cEnvelope::ok_response(
                        &req.callback_name,
                        dto_to_amf3(&outcome.response.ret, true),
                    )
                } else {
                    S2cEnvelope::error_response(&req.callback_name, outcome.response.err)
                };
                let mut caller_envelopes = Vec::with_capacity(1 + outcome.client_pushes.len());
                caller_envelopes.push(Self::stamp_session(raw_main, session, now_i64));
                caller_envelopes.extend(outcome.client_pushes.into_iter().map(|cp| {
                    let ts = cp.time as i64;
                    dto_push_at(cp, ts)
                }));
                let peer_envelopes = outcome
                    .peer_pushes
                    .into_iter()
                    .map(|(target_uid, pp)| {
                        let ts = pp.time as i64;
                        (target_uid, dto_push_at(pp, ts))
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
            let roles = acc
                .characters
                .iter()
                .filter_map(|(gid, &uid)| {
                    let p = store.get_player(uid)?;
                    Some(role_summary_from_profile(p, gid.clone()))
                })
                .collect();
            self.mapi.set_roles_for_pid(pid.to_string(), roles);
        }
    }

    fn stamp_session(
        mut env: S2cEnvelope,
        session: &ConnectionSession,
        now_i64: i64,
    ) -> S2cEnvelope {
        env = env.with_time(now_i64);
        if !session.session_token.is_empty() {
            env = env.with_token(session.session_token.clone());
        }
        env
    }

    fn ok_reply(
        req: &gserver_protocol::RpcRequest,
        session: &ConnectionSession,
        ret: Value,
        now_i64: i64,
    ) -> HandledRpc {
        HandledRpc {
            caller_envelopes: vec![Self::stamp_session(
                S2cEnvelope::ok_response(&req.callback_name, ret),
                session,
                now_i64,
            )],
            peer_envelopes: Vec::new(),
        }
    }

    fn err_reply(req: &gserver_protocol::RpcRequest, code: &str, now_i64: i64) -> HandledRpc {
        HandledRpc {
            caller_envelopes: vec![
                S2cEnvelope::error_response(&req.callback_name, code).with_time(now_i64),
            ],
            peer_envelopes: Vec::new(),
        }
    }
}

fn role_summary_from_profile(p: &PlayerProfile, groupid: String) -> RoleSummary {
    RoleSummary {
        uid: p.uid,
        uname: p.uname.clone(),
        level: p.level,
        quality: 6,
        groupname: format!("S161.Võ Xương ({groupid})"),
        groupid,
        maintainflag: 0,
        vip: p.vip,
    }
}

fn sync_player_into_gameplay(store: &PlayerStore, gameplay: &mut GameplayEngine, uid: u64) {
    if let Some(profile) = store.get_player(uid) {
        gameplay.register_player(profile_to_gameplay_state(profile));
    }
}

fn field_str_or_int(tbl: Option<&Value>, key: &str) -> Option<String> {
    let val = tbl?.get_field(key)?;
    val.as_str()
        .map(ToOwned::to_owned)
        .or_else(|| val.as_i64().map(|n| n.to_string()))
}

fn field_str<'a>(tbl: Option<&'a Value>, key: &str, default: &'a str) -> &'a str {
    tbl.and_then(|v| v.get_field(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn build_va_copy_stage(stars: i32) -> Value {
    Value::map([
        ("reset", Value::Int(0)),
        ("normal_star", Value::Int(stars)),
        ("star_status", Value::Int(0)),
        ("pass", Value::Int(1)),
        ("box_reward", Value::Int(0)),
    ])
}
