//! Official BXH & Arena Snapshot Loader (`official_bxh_snapshot.json`).
//!
//! Loads public leaderboard snapshots captured from the official server (`top.get`,
//! `top.getWorldFightForceRank`, `arena.enterArena`, and `formation.getOtherPositionInfo`)
//! so local private-server BXH tabs, Luận Võ opponents, and "Xem đội hình" work offline
//! with authentic player entries while dynamically injecting the local player's `"self"` row.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use gserver_player::json::JsonValue as PlayerJson;
use gserver_player::PlayerProfile;
use gserver_protocol::amf3::Value;

use crate::bridge::player_json_to_amf3;

/// Parsed snapshot of official leaderboard & top player formations.
#[derive(Clone, Debug, Default)]
pub struct BxhSnapshot {
    pub top_tabs: HashMap<String, PlayerJson>,
    pub world_fight_force: Option<PlayerJson>,
    pub arena: Option<PlayerJson>,
    pub formations: HashMap<u64, PlayerJson>,
    pub players_by_uid: HashMap<u64, PlayerJson>,
}

/// Summary of an arena opponent extracted from the snapshot for seeding `ArenaService`.
#[derive(Clone, Debug)]
pub struct SnapshotArenaPlayer {
    pub uid: u64,
    pub uname: String,
    pub level: u32,
    pub fight_force: u64,
    pub position: u32,
    pub htid: u32,
}

impl BxhSnapshot {
    /// Loads a snapshot JSON file if present; returns an empty snapshot when missing.
    pub fn load_optional(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .map_err(|e| format!("failed to read BXH snapshot {:?}: {e}", path))?;
        Self::from_json_str(&raw)
    }

    pub fn from_json_str(raw: &str) -> Result<Self, String> {
        let root = PlayerJson::parse(raw).map_err(|e| format!("invalid BXH snapshot JSON: {e}"))?;
        let mut snap = Self::default();

        if let Some(top_obj) = root.get("top").and_then(PlayerJson::as_object) {
            for (tab, tab_val) in top_obj {
                // Index every player entry by uid for `top.getUserInfo`
                if let Some(rank_map) = tab_val.get("rank").and_then(PlayerJson::as_object) {
                    for entry in rank_map.values() {
                        if let Some(uid) = json_as_u64(entry.get("uid")) {
                            snap.players_by_uid.entry(uid).or_insert_with(|| entry.clone());
                        }
                    }
                } else if let Some(rank_arr) = tab_val.get("rank").and_then(PlayerJson::as_array) {
                    for entry in rank_arr {
                        if let Some(uid) = json_as_u64(entry.get("uid")) {
                            snap.players_by_uid.entry(uid).or_insert_with(|| entry.clone());
                        }
                    }
                }
                snap.top_tabs.insert(tab.clone(), tab_val.clone());
            }
        }

        if let Some(wf) = root.get("world_fight_force") {
            if !matches!(wf, PlayerJson::Null) {
                snap.world_fight_force = Some(wf.clone());
            }
        }

        if let Some(arena) = root.get("arena") {
            if !matches!(arena, PlayerJson::Null) {
                snap.arena = Some(arena.clone());
            }
        }

        if let Some(fmap) = root.get("formations").and_then(PlayerJson::as_object) {
            for (uid_str, fval) in fmap {
                if let Ok(uid) = uid_str.parse::<u64>() {
                    snap.formations.insert(uid, fval.clone());
                }
            }
        }

        Ok(snap)
    }

    pub fn is_loaded(&self) -> bool {
        !self.top_tabs.is_empty() || !self.formations.is_empty() || self.arena.is_some()
    }

    /// Serves `top.get(tab)` using captured official rank entries + dynamic local `"self"` row.
    pub fn build_top_get_reply(
        &self,
        tab: &str,
        self_profile: Option<&PlayerProfile>,
        local_profiles: &[PlayerProfile],
    ) -> Value {
        if let Some(tab_val) = self.top_tabs.get(tab) {
            let mut out = BTreeMap::new();
            if let Some(rank_val) = tab_val.get("rank") {
                out.insert("rank".to_string(), rank_val.clone());
            } else {
                out.insert("rank".to_string(), PlayerJson::Object(BTreeMap::new()));
            }
            if let Some(p) = self_profile {
                out.insert("self".to_string(), build_self_rank_entry(p, None));
            }
            return player_json_to_amf3(&PlayerJson::Object(out));
        }

        // Fallback: build rank list dynamically from local PlayerStore profiles
        let mut sorted: Vec<&PlayerProfile> = local_profiles.iter().collect();
        match tab {
            "level" => sorted.sort_by(|a, b| b.level.cmp(&a.level).then_with(|| b.fc.cmp(&a.fc))),
            _ => sorted.sort_by(|a, b| b.fc.cmp(&a.fc).then_with(|| b.level.cmp(&a.level))),
        }

        let mut rank_obj = BTreeMap::new();
        let mut self_rank: Option<u32> = None;
        for (idx, prof) in sorted.iter().take(50).enumerate() {
            let r = (idx + 1) as u32;
            if self_profile.is_some_and(|sp| sp.uid == prof.uid) {
                self_rank = Some(r);
            }
            rank_obj.insert(r.to_string(), build_self_rank_entry(prof, Some(r)));
        }

        let mut out = BTreeMap::new();
        out.insert("rank".to_string(), PlayerJson::Object(rank_obj));
        if let Some(p) = self_profile {
            out.insert("self".to_string(), build_self_rank_entry(p, self_rank));
        }
        player_json_to_amf3(&PlayerJson::Object(out))
    }

    /// Serves `top.getWorldFightForceRank`.
    pub fn build_world_fight_force_reply(
        &self,
        self_profile: Option<&PlayerProfile>,
        local_profiles: &[PlayerProfile],
    ) -> Value {
        if let Some(ref wf) = self.world_fight_force {
            let mut out = BTreeMap::new();
            if let Some(rank_val) = wf.get("rank") {
                out.insert("rank".to_string(), rank_val.clone());
            } else {
                out.insert("rank".to_string(), PlayerJson::Object(BTreeMap::new()));
            }
            if let Some(p) = self_profile {
                out.insert("self".to_string(), build_self_rank_entry(p, None));
            }
            return player_json_to_amf3(&PlayerJson::Object(out));
        }
        self.build_top_get_reply("fight_force_max", self_profile, local_profiles)
    }

    /// Serves `top.getUserInfo(uid)` from the captured snapshot.
    pub fn get_user_info(&self, target_uid: u64) -> Option<Value> {
        self.players_by_uid
            .get(&target_uid)
            .map(player_json_to_amf3)
    }

    /// Serves `formation.getOtherPositionInfo(uid)` from the captured snapshot.
    pub fn get_other_formation(&self, target_uid: u64) -> Option<Value> {
        self.formations.get(&target_uid).map(player_json_to_amf3)
    }

    /// Extracts arena ladder entries from `top.arena` or `arena.res.top10` for `ArenaService`.
    pub fn extract_arena_players(&self) -> Vec<SnapshotArenaPlayer> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Prefer `top.arena.rank` (up to 50 ranked players)
        if let Some(arena_tab) = self.top_tabs.get("arena") {
            if let Some(rank_map) = arena_tab.get("rank").and_then(PlayerJson::as_object) {
                for (pos_str, entry) in rank_map {
                    if let (Ok(pos), Some(uid)) = (pos_str.parse::<u32>(), json_as_u64(entry.get("uid"))) {
                        if pos > 0 && seen.insert(uid) {
                            let uname = entry
                                .get("uname")
                                .and_then(PlayerJson::as_str)
                                .unwrap_or("Hero")
                                .to_string();
                            let level = json_as_u64(entry.get("level")).unwrap_or(1) as u32;
                            let fc = json_as_u64(entry.get("fight_force")).unwrap_or(10_000);
                            let htid = json_as_u64(entry.get("htid"))
                                .or_else(|| json_as_u64(entry.get("utid")))
                                .or_else(|| {
                                    self.players_by_uid.get(&uid).and_then(|p| {
                                        json_as_u64(p.get("htid"))
                                            .or_else(|| json_as_u64(p.get("utid")))
                                    })
                                })
                                .unwrap_or(310) as u32;
                            out.push(SnapshotArenaPlayer {
                                uid,
                                uname,
                                level,
                                fight_force: fc,
                                position: pos,
                                htid,
                            });
                        }
                    }
                }
            }
        }

        // Also check `arena.res.top10` / `arena.res.opponents`
        if let Some(res) = self.arena.as_ref().and_then(|a| a.get("res")) {
            for key in ["top10", "opponents"] {
                if let Some(arr) = res.get(key).and_then(PlayerJson::as_array) {
                    for entry in arr {
                        let uid = json_as_u64(entry.get("uid")).unwrap_or(0);
                        let pos = json_as_u64(entry.get("position")).unwrap_or(0) as u32;
                        if uid > 0 && pos > 0 && seen.insert(uid) {
                            let uname = entry
                                .get("uname")
                                .and_then(PlayerJson::as_str)
                                .unwrap_or("Hero")
                                .to_string();
                            let level = json_as_u64(entry.get("level")).unwrap_or(1) as u32;
                            let fc = json_as_u64(entry.get("fight_force")).unwrap_or(10_000);
                            let htid = json_as_u64(entry.get("htid"))
                                .or_else(|| json_as_u64(entry.get("utid")))
                                .or_else(|| {
                                    self.players_by_uid.get(&uid).and_then(|p| {
                                        json_as_u64(p.get("htid"))
                                            .or_else(|| json_as_u64(p.get("utid")))
                                    })
                                })
                                .unwrap_or(310) as u32;
                            out.push(SnapshotArenaPlayer {
                                uid,
                                uname,
                                level,
                                fight_force: fc,
                                position: pos,
                                htid,
                            });
                        }
                    }
                }
            }
        }

        out.sort_by_key(|p| p.position);
        out
    }
}

fn json_as_u64(v: Option<&PlayerJson>) -> Option<u64> {
    match v? {
        PlayerJson::UInt(n) => Some(*n),
        PlayerJson::Int(n) if *n >= 0 => Some(*n as u64),
        PlayerJson::Float(f) if *f >= 0.0 => Some(*f as u64),
        PlayerJson::Str(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn build_self_rank_entry(p: &PlayerProfile, rank: Option<u32>) -> PlayerJson {
    let mut m = BTreeMap::new();
    if let Some(r) = rank {
        m.insert("rank".to_string(), PlayerJson::from(r));
    }
    m.insert("uid".to_string(), PlayerJson::from(p.uid));
    m.insert("pid".to_string(), PlayerJson::from(p.pid.clone()));
    m.insert("uname".to_string(), PlayerJson::from(p.uname.clone()));
    m.insert("utid".to_string(), PlayerJson::from(p.utid));
    m.insert("htid".to_string(), PlayerJson::from(p.utid));
    m.insert("level".to_string(), PlayerJson::from(p.level));
    m.insert("vip".to_string(), PlayerJson::from(p.vip));
    m.insert("fight_force".to_string(), PlayerJson::from(p.fc));
    m.insert("quality".to_string(), PlayerJson::from(2u32));
    m.insert("dress_id".to_string(), PlayerJson::from(1u32));
    m.insert("avatar_id".to_string(), PlayerJson::from(0u32));
    m.insert("title_id".to_string(), PlayerJson::from(0u32));
    m.insert("praised_num".to_string(), PlayerJson::from(0u32));
    m.insert("guild_id".to_string(), PlayerJson::from(p.guild_id));
    m.insert("guild_name".to_string(), PlayerJson::from(""));
    m.insert("star_normal".to_string(), PlayerJson::from(0u32));
    m.insert("normal".to_string(), PlayerJson::from(0u32));
    m.insert("normal_id".to_string(), PlayerJson::from(0u32));
    m.insert("normal_layer".to_string(), PlayerJson::from(0u32));
    m.insert("max_layer".to_string(), PlayerJson::from(0u32));
    PlayerJson::Object(m)
}
