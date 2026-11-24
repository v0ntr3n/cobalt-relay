//! `MainTown` bootstrap validator and multiplayer walking town room manager
//! (`city.enter`, `city.leave`, `city.getEnterList`, `town.move`, `imperialcity.*`,
//! and server pushes `town.addPlayer`, `town.delPlayer`, `town.move`, `town.update`).

use std::collections::BTreeMap;

use crate::player::PlayerState;
use crate::protocol::{push, DtoValue, PushPacket};

/// Town map bounds and sharding limits extracted directly from decrypted `town.db` and `RoyalCityHero.lua`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TownMapConfig {
    pub town_id: u32,
    pub name: &'static str,
    pub min_level: u32,
    pub max_visible_players: usize,
    pub spawn_x: i32,
    pub spawn_y: i32,
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
    pub walk_speed: u32,
}

impl TownMapConfig {
    /// Preset town definitions from `town.db` (`id = 1, 2, 3, 101`) + `RoyalCityHero.lua` (`id = 999`).
    pub fn preset(town_id: u32) -> Self {
        match town_id {
            1 => Self {
                town_id: 1,
                name: "Dao Nguyen Thon",
                min_level: 0,
                max_visible_players: 10,
                spawn_x: 40,
                spawn_y: 40,
                min_x: 0,
                max_x: 2500,
                min_y: 0,
                max_y: 350,
                walk_speed: 200,
            },
            2 => Self {
                town_id: 2,
                name: "Lac Duong",
                min_level: 5,
                max_visible_players: 20,
                spawn_x: 40,
                spawn_y: 40,
                min_x: 0,
                max_x: 2500,
                min_y: 0,
                max_y: 400,
                walk_speed: 200,
            },
            3 => Self {
                town_id: 3,
                name: "Truong An",
                min_level: 0,
                max_visible_players: 40,
                spawn_x: 120,
                spawn_y: 120,
                min_x: 0,
                max_x: 2772,
                min_y: 0,
                max_y: 768,
                walk_speed: 200,
            },
            101 => Self {
                town_id: 101,
                name: "Boss TG Truong Giac",
                min_level: 0,
                max_visible_players: 20,
                spawn_x: 40,
                spawn_y: 40,
                min_x: 0,
                max_x: 1250,
                min_y: 0,
                max_y: 300,
                walk_speed: 200,
            },
            // RoyalCity (`imperialcity.*`) bounds verified from `RoyalCityHero.lua`:
            // `self.m_checkRect = CCRect(50, 0, 2720, 342)` -> x in [50..2770], y in [0..342], speed = 200.
            _ => Self {
                town_id: 999,
                name: "Hoang Thanh (RoyalCity)",
                min_level: 1,
                max_visible_players: 30,
                spawn_x: 260,
                spawn_y: 160,
                min_x: 50,
                max_x: 50 + 2720,
                min_y: 0,
                max_y: 342,
                walk_speed: 200,
            },
        }
    }

    pub fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        (x.clamp(self.min_x, self.max_x), y.clamp(self.min_y, self.max_y))
    }
}

/// Validates every data invariant required by `MainScene.lua` -> `MainProxy.lua` -> `MainTown.lua`
/// so entering the home `MainTown` scene never triggers a Lua nil-index crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainTownCrashRisk {
    /// `UserData:parseData` calls `string.sub(self.server_open_time, 3, 14)` and `MainProxy:ctor`
    /// parses `"YYYYMMDDHHMMSS"`. Strings shorter than 14 digits crash `MainTown` immediately!
    InvalidServerOpenTime(String),
    /// `MainTown:initLineUp` requires protagonist hero in formation slot 0 (`formation[0] != 0`)
    /// and present in `allHeros`.
    MissingProtagonistFormation,
    /// `HeroFormationData` references a `hid` in `formation` that is missing from `allHeros`.
    DanglingFormationHero(u64),
}

pub fn validate_maintown_bootstrap(player: &PlayerState) -> Result<(), MainTownCrashRisk> {
    if player.server_open_time.len() < 14
        || !player.server_open_time.bytes().take(14).all(|b| b.is_ascii_digit())
    {
        return Err(MainTownCrashRisk::InvalidServerOpenTime(
            player.server_open_time.clone(),
        ));
    }
    let main_hid = player.roster.formation[0];
    if main_hid == 0 || !player.roster.heroes.contains_key(&main_hid) {
        return Err(MainTownCrashRisk::MissingProtagonistFormation);
    }
    for &hid in &player.roster.formation {
        if hid != 0 && !player.roster.heroes.contains_key(&hid) {
            return Err(MainTownCrashRisk::DanglingFormationHero(hid));
        }
    }
    Ok(())
}

/// Presence snapshot of a player walking inside a `city.*` or `imperialcity.*` instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TownPlayerPresence {
    pub uid: u64,
    pub icuid: String,
    pub uname: String,
    pub utid: u32,
    pub dress_id: u32,
    pub title_id: u32,
    pub server_id: String,
    pub fight_force: u64,
    pub vip: u32,
    pub level: u32,
    pub x: i32,
    pub y: i32,
    pub dir: i32,
    pub town_id: u32,
    pub instance_id: u32,
}

impl TownPlayerPresence {
    pub fn from_player(player: &PlayerState, town_id: u32, instance_id: u32, x: i32, y: i32) -> Self {
        Self {
            uid: player.uid,
            icuid: format!("{}_{}", player.server_id, player.uid),
            uname: player.uname.clone(),
            utid: player.utid,
            dress_id: player.dress_id,
            title_id: player.title_id,
            server_id: player.server_id.clone(),
            fight_force: player.fight_force(),
            vip: player.vip,
            level: player.level,
            x,
            y,
            dir: 1,
            town_id,
            instance_id,
        }
    }

    /// Serializes to the `npcData` / player entry schema read by `RoyalCityHero:setData`
    /// (`npcData.extra.{icuid, uid, uname, utid, dress_id, title_id, server_id, fight_force, vip, level}`).
    pub fn to_dto(&self) -> DtoValue {
        let extra = DtoValue::map([
            ("icuid", DtoValue::from(self.icuid.as_str())),
            ("uid", DtoValue::from(self.uid)),
            ("uname", DtoValue::from(self.uname.as_str())),
            ("utid", DtoValue::from(self.utid)),
            ("dress_id", DtoValue::from(self.dress_id)),
            ("title_id", DtoValue::from(self.title_id)),
            ("server_id", DtoValue::from(self.server_id.as_str())),
            ("fight_force", DtoValue::from(self.fight_force)),
            ("vip", DtoValue::from(self.vip)),
            ("level", DtoValue::from(self.level)),
        ]);
        DtoValue::map([
            ("uid", DtoValue::from(self.uid)),
            ("icuid", DtoValue::from(self.icuid.as_str())),
            ("uname", DtoValue::from(self.uname.as_str())),
            ("utid", DtoValue::from(self.utid)),
            ("dress_id", DtoValue::from(self.dress_id)),
            ("title_id", DtoValue::from(self.title_id)),
            ("level", DtoValue::from(self.level)),
            ("vip", DtoValue::from(self.vip)),
            ("fight_force", DtoValue::from(self.fight_force)),
            ("x", DtoValue::from(self.x)),
            ("y", DtoValue::from(self.y)),
            ("dir", DtoValue::from(self.dir)),
            ("extra", extra),
        ])
    }
}

pub type PeerPushList = Vec<(u64, PushPacket)>;

#[derive(Debug, Clone)]
struct TownInstance {
    config: TownMapConfig,
    occupants: BTreeMap<u64, TownPlayerPresence>,
}

/// Manages sharded multiplayer town rooms (`city.*` and `imperialcity.*`) and peer broadcasts.
#[derive(Debug, Clone, Default)]
pub struct TownRoomManager {
    /// `(town_id, instance_id) -> TownInstance`
    instances: BTreeMap<(u32, u32), TownInstance>,
    /// `uid -> (town_id, instance_id)`
    player_loc: BTreeMap<u64, (u32, u32)>,
}

impl TownRoomManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn locate_player(&self, uid: u64) -> Option<&TownPlayerPresence> {
        let &(town_id, inst_id) = self.player_loc.get(&uid)?;
        self.instances.get(&(town_id, inst_id))?.occupants.get(&uid)
    }

    /// Enters `town_id` (`city.enter` or `imperialcity.enter`), auto-leaving any prior room,
    /// clamping spawn coordinates, sharding rooms by `max_visible_players`, and returning
    /// `(enter_response_dto, peer_broadcasts[(peer_uid, PushPacket)])`.
    pub fn enter_town(
        &mut self,
        player: &PlayerState,
        town_id: u32,
        req_x: Option<i32>,
        req_y: Option<i32>,
        now_ts: u64,
    ) -> Result<(DtoValue, PeerPushList), &'static str> {
        validate_maintown_bootstrap(player).map_err(|_| "maintown_bootstrap_invalid")?;
        let cfg = TownMapConfig::preset(town_id);
        if player.level < cfg.min_level {
            return Err("level_not_enough");
        }

        let mut broadcasts = Vec::new();
        if self.player_loc.contains_key(&player.uid) {
            let (_, leave_bcasts) = self.leave_town(player.uid, now_ts);
            broadcasts.extend(leave_bcasts);
        }

        // Find first instance with free capacity or create a new shard
        let mut target_inst_id = 1u32;
        loop {
            match self.instances.get(&(cfg.town_id, target_inst_id)) {
                Some(inst) if inst.occupants.len() >= cfg.max_visible_players => {
                    target_inst_id += 1;
                }
                _ => break,
            }
        }

        let (x, y) = cfg.clamp_point(
            req_x.unwrap_or(cfg.spawn_x),
            req_y.unwrap_or(cfg.spawn_y),
        );
        let presence = TownPlayerPresence::from_player(player, cfg.town_id, target_inst_id, x, y);

        let inst = self
            .instances
            .entry((cfg.town_id, target_inst_id))
            .or_insert_with(|| TownInstance {
                config: cfg.clone(),
                occupants: BTreeMap::new(),
            });

        let add_pkt = PushPacket::new(push::TOWN_ADD_PLAYER, presence.to_dto(), now_ts);
        for &peer_uid in inst.occupants.keys() {
            if peer_uid != player.uid {
                broadcasts.push((peer_uid, add_pkt.clone()));
            }
        }

        inst.occupants.insert(player.uid, presence.clone());
        self.player_loc.insert(player.uid, (cfg.town_id, target_inst_id));

        let peers: Vec<DtoValue> = inst
            .occupants
            .values()
            .map(TownPlayerPresence::to_dto)
            .collect();

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("town_id", DtoValue::from(cfg.town_id)),
            ("instance_id", DtoValue::from(target_inst_id)),
            ("self_icuid", DtoValue::from(presence.icuid.as_str())),
            ("x", DtoValue::from(x)),
            ("y", DtoValue::from(y)),
            ("speed", DtoValue::from(cfg.walk_speed)),
            ("enter_list", DtoValue::Array(peers)),
        ]);

        Ok((ret, broadcasts))
    }

    /// Removes `uid` from their current room (`city.leave` / `imperialcity.leave`) and broadcasts `town.delPlayer`.
    pub fn leave_town(&mut self, uid: u64, now_ts: u64) -> (DtoValue, Vec<(u64, PushPacket)>) {
        let mut broadcasts = Vec::new();
        if let Some((town_id, inst_id)) = self.player_loc.remove(&uid) {
            if let Some(inst) = self.instances.get_mut(&(town_id, inst_id)) {
                if let Some(removed) = inst.occupants.remove(&uid) {
                    let del_dto = DtoValue::map([
                        ("uid", DtoValue::from(uid)),
                        ("icuid", DtoValue::from(removed.icuid.as_str())),
                    ]);
                    let pkt = PushPacket::new(push::TOWN_DEL_PLAYER, del_dto, now_ts);
                    for &peer_uid in inst.occupants.keys() {
                        broadcasts.push((peer_uid, pkt.clone()));
                    }
                }
            }
        }
        (DtoValue::map([("ret", DtoValue::from("ok"))]), broadcasts)
    }

    /// Returns visible peers in the caller's current town shard (`city.getEnterList`).
    pub fn get_enter_list(&self, uid: u64) -> DtoValue {
        let Some(&(town_id, inst_id)) = self.player_loc.get(&uid) else {
            return DtoValue::Array(Vec::new());
        };
        let Some(inst) = self.instances.get(&(town_id, inst_id)) else {
            return DtoValue::Array(Vec::new());
        };
        DtoValue::Array(
            inst.occupants
                .values()
                .map(TownPlayerPresence::to_dto)
                .collect(),
        )
    }

    /// Moves `uid` inside their town shard (`town.move` / `imperialcity.move`), clamping to the
    /// town's `CCRect` (`RoyalCityHero:checkHeroPos`), updating facing direction (`1` or `-1`),
    /// and broadcasting `town.move` to all peers in the shard.
    pub fn move_player(
        &mut self,
        uid: u64,
        dest_x: i32,
        dest_y: i32,
        now_ts: u64,
    ) -> Result<(DtoValue, PeerPushList), &'static str> {
        let &(town_id, inst_id) = self.player_loc.get(&uid).ok_or("not_in_town")?;
        let inst = self.instances.get_mut(&(town_id, inst_id)).ok_or("not_in_town")?;
        let (clamped_x, clamped_y) = inst.config.clamp_point(dest_x, dest_y);

        let occupant = inst.occupants.get_mut(&uid).ok_or("not_in_town")?;
        let dx = clamped_x - occupant.x;
        if dx > 0 {
            occupant.dir = 1;
        } else if dx < 0 {
            occupant.dir = -1;
        }
        occupant.x = clamped_x;
        occupant.y = clamped_y;
        let dir = occupant.dir;
        let icuid = occupant.icuid.clone();

        let move_dto = DtoValue::map([
            ("uid", DtoValue::from(uid)),
            ("icuid", DtoValue::from(icuid.as_str())),
            ("x", DtoValue::from(clamped_x)),
            ("y", DtoValue::from(clamped_y)),
            ("dir", DtoValue::from(dir)),
        ]);
        let pkt = PushPacket::new(push::TOWN_MOVE, move_dto.clone(), now_ts);

        let mut broadcasts = Vec::new();
        for &peer_uid in inst.occupants.keys() {
            if peer_uid != uid {
                broadcasts.push((peer_uid, pkt.clone()));
            }
        }

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("x", DtoValue::from(clamped_x)),
            ("y", DtoValue::from(clamped_y)),
            ("dir", DtoValue::from(dir)),
        ]);
        Ok((ret, broadcasts))
    }

    /// Syncs updated player appearance/stats (`vip`, `level`, `dress_id`, `title_id`, `fight_force`)
    /// to peers in the same town room via `town.update` (`SC_TOWN_PLAYER_UPDATE`).
    pub fn sync_player_appearance(
        &mut self,
        player: &PlayerState,
        now_ts: u64,
    ) -> Vec<(u64, PushPacket)> {
        let Some(&(town_id, inst_id)) = self.player_loc.get(&player.uid) else {
            return Vec::new();
        };
        let Some(inst) = self.instances.get_mut(&(town_id, inst_id)) else {
            return Vec::new();
        };
        let Some(occ) = inst.occupants.get_mut(&player.uid) else {
            return Vec::new();
        };
        occ.uname = player.uname.clone();
        occ.dress_id = player.dress_id;
        occ.title_id = player.title_id;
        occ.vip = player.vip;
        occ.level = player.level;
        occ.fight_force = player.fight_force();
        let dto = occ.to_dto();
        let pkt = PushPacket::new(push::TOWN_UPDATE, dto, now_ts);

        inst.occupants
            .keys()
            .copied()
            .filter(|&peer| peer != player.uid)
            .map(|peer| (peer, pkt.clone()))
            .collect()
    }

    /// Safe response for castle/fief building info RPCs (`castle.getInfo`, `hall.getInfo`, etc.).
    pub fn castle_building_info_dto(building_kind: &str) -> DtoValue {
        DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("kind", DtoValue::from(building_kind)),
            ("level", DtoValue::from(1u32)),
            ("cd_time", DtoValue::from(0u32)),
            ("va_extra", DtoValue::Map(BTreeMap::new())),
        ])
    }
}
