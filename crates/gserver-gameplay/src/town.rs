//! `MainTown` bootstrap validator and multiplayer walking town room manager
//! (`city.enter`, `city.leave`, `city.getEnterList`, `town.move`, `imperialcity.*`,
//! and server pushes `town.addPlayer`, `town.delPlayer`, `town.move`, `town.update`).

use std::collections::BTreeMap;

use crate::player::PlayerState;
use crate::protocol::{dto_arr, dto_map, dto_ok, push, DtoValue, PushPacket};

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
    #[allow(clippy::too_many_arguments)]
    const fn spec(
        town_id: u32,
        name: &'static str,
        min_level: u32,
        max_visible_players: usize,
        spawn: (i32, i32),
        x_bounds: (i32, i32),
        y_bounds: (i32, i32),
    ) -> Self {
        Self {
            town_id,
            name,
            min_level,
            max_visible_players,
            spawn_x: spawn.0,
            spawn_y: spawn.1,
            min_x: x_bounds.0,
            max_x: x_bounds.1,
            min_y: y_bounds.0,
            max_y: y_bounds.1,
            walk_speed: 200,
        }
    }

    /// Preset town definitions from `town.db` (`id = 1, 2, 3, 101`) + `RoyalCityHero.lua` (`id = 999`).
    pub fn preset(town_id: u32) -> Self {
        match town_id {
            1 => Self::spec(1, "Dao Nguyen Thon", 0, 10, (40, 40), (0, 2500), (0, 350)),
            2 => Self::spec(2, "Lac Duong", 5, 20, (40, 40), (0, 2500), (0, 400)),
            3 => Self::spec(3, "Truong An", 0, 40, (120, 120), (0, 2772), (0, 768)),
            101 => Self::spec(101, "Boss TG Truong Giac", 0, 20, (40, 40), (0, 1250), (0, 300)),
            // RoyalCity (`imperialcity.*`) bounds verified from `RoyalCityHero.lua`:
            // `self.m_checkRect = CCRect(50, 0, 2720, 342)` -> x in [50..2770], y in [0..342], speed = 200.
            _ => Self::spec(999, "Hoang Thanh (RoyalCity)", 1, 30, (260, 160), (50, 2770), (0, 342)),
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

    /// Serializes to the `npcData` / player entry schema read by `RoyalCityHero:setData`.
    pub fn to_dto(&self) -> DtoValue {
        dto_map! {
            "uid" => self.uid,
            "icuid" => &self.icuid,
            "uname" => &self.uname,
            "utid" => self.utid,
            "dress_id" => self.dress_id,
            "title_id" => self.title_id,
            "level" => self.level,
            "vip" => self.vip,
            "fight_force" => self.fight_force,
            "x" => self.x,
            "y" => self.y,
            "dir" => self.dir,
            "extra" => dto_map! {
                "icuid" => &self.icuid,
                "uid" => self.uid,
                "uname" => &self.uname,
                "utid" => self.utid,
                "dress_id" => self.dress_id,
                "title_id" => self.title_id,
                "server_id" => &self.server_id,
                "fight_force" => self.fight_force,
                "vip" => self.vip,
                "level" => self.level,
            },
        }
    }
}

pub type PeerPushList = Vec<(u64, PushPacket)>;

#[derive(Debug, Clone)]
struct TownInstance {
    config: TownMapConfig,
    occupants: BTreeMap<u64, TownPlayerPresence>,
}

impl TownInstance {
    fn broadcast_except(&self, exclude_uid: u64, pkt: &PushPacket, out: &mut PeerPushList) {
        out.extend(
            self.occupants
                .keys()
                .copied()
                .filter(|&peer| peer != exclude_uid)
                .map(|peer| (peer, pkt.clone())),
        );
    }
}

/// Manages sharded multiplayer town rooms (`city.*` and `imperialcity.*`) and peer broadcasts.
#[derive(Debug, Clone, Default)]
pub struct TownRoomManager {
    instances: BTreeMap<(u32, u32), TownInstance>,
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

        let target_inst_id = (1u32..)
            .find(|&inst_id| {
                self.instances
                    .get(&(cfg.town_id, inst_id))
                    .map_or(true, |inst| inst.occupants.len() < cfg.max_visible_players)
            })
            .unwrap_or(1);

        let (x, y) = cfg.clamp_point(req_x.unwrap_or(cfg.spawn_x), req_y.unwrap_or(cfg.spawn_y));
        let presence = TownPlayerPresence::from_player(player, cfg.town_id, target_inst_id, x, y);

        let inst = self
            .instances
            .entry((cfg.town_id, target_inst_id))
            .or_insert_with(|| TownInstance {
                config: cfg.clone(),
                occupants: BTreeMap::new(),
            });

        let add_pkt = PushPacket::new(push::TOWN_ADD_PLAYER, presence.to_dto(), now_ts);
        inst.broadcast_except(player.uid, &add_pkt, &mut broadcasts);

        inst.occupants.insert(player.uid, presence.clone());
        self.player_loc.insert(player.uid, (cfg.town_id, target_inst_id));

        let peers: Vec<DtoValue> = inst
            .occupants
            .values()
            .map(TownPlayerPresence::to_dto)
            .collect();

        let ret = dto_ok! {
            "town_id" => cfg.town_id,
            "instance_id" => target_inst_id,
            "self_icuid" => &presence.icuid,
            "x" => x,
            "y" => y,
            "speed" => cfg.walk_speed,
            "enter_list" => DtoValue::Array(peers),
        };
        Ok((ret, broadcasts))
    }

    /// Removes `uid` from their current room (`city.leave` / `imperialcity.leave`) and broadcasts `town.delPlayer`.
    pub fn leave_town(&mut self, uid: u64, now_ts: u64) -> (DtoValue, PeerPushList) {
        let mut broadcasts = Vec::new();
        if let Some((town_id, inst_id)) = self.player_loc.remove(&uid) {
            if let Some(inst) = self.instances.get_mut(&(town_id, inst_id)) {
                if let Some(removed) = inst.occupants.remove(&uid) {
                    let pkt = PushPacket::new(
                        push::TOWN_DEL_PLAYER,
                        dto_map! { "uid" => uid, "icuid" => &removed.icuid },
                        now_ts,
                    );
                    inst.broadcast_except(uid, &pkt, &mut broadcasts);
                }
            }
        }
        (dto_ok!(), broadcasts)
    }

    /// Returns visible peers in the caller's current town shard (`city.getEnterList`).
    pub fn get_enter_list(&self, uid: u64) -> DtoValue {
        let Some(&(town_id, inst_id)) = self.player_loc.get(&uid) else {
            return dto_arr![];
        };
        let Some(inst) = self.instances.get(&(town_id, inst_id)) else {
            return dto_arr![];
        };
        DtoValue::Array(inst.occupants.values().map(TownPlayerPresence::to_dto).collect())
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

        let pkt = PushPacket::new(
            push::TOWN_MOVE,
            dto_map! {
                "uid" => uid,
                "icuid" => &icuid,
                "x" => clamped_x,
                "y" => clamped_y,
                "dir" => dir,
            },
            now_ts,
        );
        let mut broadcasts = Vec::new();
        inst.broadcast_except(uid, &pkt, &mut broadcasts);

        Ok((
            dto_ok! {
                "x" => clamped_x,
                "y" => clamped_y,
                "dir" => dir,
            },
            broadcasts,
        ))
    }

    /// Syncs updated player appearance/stats (`vip`, `level`, `dress_id`, `title_id`, `fight_force`)
    /// to peers in the same town room via `town.update` (`SC_TOWN_PLAYER_UPDATE`).
    pub fn sync_player_appearance(&mut self, player: &PlayerState, now_ts: u64) -> PeerPushList {
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
        let pkt = PushPacket::new(push::TOWN_UPDATE, occ.to_dto(), now_ts);
        let mut broadcasts = Vec::new();
        inst.broadcast_except(player.uid, &pkt, &mut broadcasts);
        broadcasts
    }

    /// Safe response for castle/fief building info RPCs (`castle.getInfo`, `hall.getInfo`, etc.).
    pub fn castle_building_info_dto(building_kind: &str) -> DtoValue {
        dto_ok! {
            "kind" => building_kind,
            "level" => 1u32,
            "cd_time" => 0u32,
            "va_extra" => dto_map!(),
        }
    }
}
