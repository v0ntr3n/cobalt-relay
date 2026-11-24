//! Player state, `BagData` inventory grids, `HerosData` roster, `HeroFormationData` lineup,
//! and live `user.updateUser` / `bag.update` / `re.hero.new` delta push builders.

use std::collections::BTreeMap;

use crate::protocol::{push, DtoValue, PushPacket};

/// Bag sub-container grid ranges verified from `BagData.lua` / `BaseBagProxy.lua`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BagCategory {
    Arm,
    Item,
    Treasure,
    HeroFrag,
    TreasFrag,
}

impl BagCategory {
    pub fn key_name(self) -> &'static str {
        match self {
            Self::Arm => "arm",
            Self::Item => "item",
            Self::Treasure => "treasure",
            Self::HeroFrag => "heroFrag",
            Self::TreasFrag => "treasFrag",
        }
    }

    pub fn grid_start(self) -> u32 {
        match self {
            Self::Arm => 2_000_001,
            Self::Item => 3_000_001,
            Self::HeroFrag => 4_000_001,
            Self::Treasure => 5_000_001,
            Self::TreasFrag => 6_000_001,
        }
    }

    pub fn default_max_grids(self) -> u32 {
        match self {
            Self::Item => 300,
            Self::Arm => 200,
            Self::HeroFrag => 200,
            Self::Treasure => 150,
            Self::TreasFrag => 150,
        }
    }

    /// Classifies an item template ID into its GServer bag sub-table.
    pub fn from_template_id(tid: u32) -> Self {
        match tid {
            100_000..=199_999 => Self::Arm,
            400_000..=499_999 | 20_000..=20_999 => Self::HeroFrag,
            500_000..=599_999 => Self::Treasure,
            600_000..=699_999 => Self::TreasFrag,
            _ => Self::Item,
        }
    }
}

/// Single inventory slot item matching `BagData.lua` (`item_id`, `item_template_id`, `item_num`, `item_time`, `va_item_text`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BagItem {
    pub gid: u32,
    pub item_id: u64,
    pub item_template_id: u32,
    pub item_num: u32,
    pub item_time: u64,
}

impl BagItem {
    pub fn to_dto(&self) -> DtoValue {
        DtoValue::map([
            ("item_id", DtoValue::from(self.item_id)),
            ("item_template_id", DtoValue::from(self.item_template_id)),
            ("item_num", DtoValue::from(self.item_num)),
            ("item_time", DtoValue::from(self.item_time)),
            ("va_item_text", DtoValue::map(Vec::<(&str, DtoValue)>::new())),
        ])
    }
}

/// Delta entry emitted in `bag.update` (`CS_BAG_PUSH_UPDATE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BagDelta {
    Upsert(BagItem),
    Remove { gid: u32 },
}

/// Player inventory state supporting stacking, consumption, `bag.bagInfo`, and `bag.update` pushes.
#[derive(Debug, Clone)]
pub struct BagState {
    pub slots: BTreeMap<u32, BagItem>,
    pub grid_max: BTreeMap<BagCategory, u32>,
    next_item_id: u64,
}

impl Default for BagState {
    fn default() -> Self {
        Self::new()
    }
}

impl BagState {
    pub fn new() -> Self {
        let mut grid_max = BTreeMap::new();
        for cat in [
            BagCategory::Arm,
            BagCategory::Item,
            BagCategory::Treasure,
            BagCategory::HeroFrag,
            BagCategory::TreasFrag,
        ] {
            grid_max.insert(cat, cat.default_max_grids());
        }
        Self {
            slots: BTreeMap::new(),
            grid_max,
            next_item_id: 90_000_001,
        }
    }

    pub fn count_by_tid(&self, tid: u32) -> u32 {
        self.slots
            .values()
            .filter(|it| it.item_template_id == tid)
            .map(|it| it.item_num)
            .sum()
    }

    pub fn used_grids(&self, cat: BagCategory) -> u32 {
        let start = cat.grid_start();
        let end = start + 999_999;
        self.slots.range(start..=end).count() as u32
    }

    pub fn is_category_full(&self, cat: BagCategory) -> bool {
        let max = self.grid_max.get(&cat).copied().unwrap_or(100);
        self.used_grids(cat) >= max
    }

    /// Adds `qty` of `tid`, stacking onto an existing slot for stackable items/fragments or allocating a new `gid`.
    pub fn add_item(&mut self, tid: u32, qty: u32, now_ts: u64) -> Result<Vec<BagDelta>, &'static str> {
        if qty == 0 {
            return Ok(Vec::new());
        }
        let cat = BagCategory::from_template_id(tid);
        let stackable = matches!(cat, BagCategory::Item | BagCategory::HeroFrag | BagCategory::TreasFrag);
        if stackable {
            if let Some((&gid, existing)) = self
                .slots
                .iter_mut()
                .find(|(_, it)| it.item_template_id == tid)
            {
                existing.item_num = existing.item_num.saturating_add(qty);
                existing.item_time = now_ts;
                let updated = existing.clone();
                let _ = gid;
                return Ok(vec![BagDelta::Upsert(updated)]);
            }
        }
        if self.is_category_full(cat) {
            return Err("bag_full");
        }
        let start = cat.grid_start();
        let max = self.grid_max.get(&cat).copied().unwrap_or(100);
        let mut chosen_gid = None;
        for offset in 0..max {
            let candidate = start + offset;
            if !self.slots.contains_key(&candidate) {
                chosen_gid = Some(candidate);
                break;
            }
        }
        let gid = chosen_gid.ok_or("bag_full")?;
        let item_id = self.next_item_id;
        self.next_item_id += 1;
        let item = BagItem {
            gid,
            item_id,
            item_template_id: tid,
            item_num: qty,
            item_time: now_ts,
        };
        self.slots.insert(gid, item.clone());
        Ok(vec![BagDelta::Upsert(item)])
    }

    /// Consumes `qty` of `tid` across matching slots, emitting `BagDelta::Upsert` or `BagDelta::Remove`.
    pub fn consume_item(&mut self, tid: u32, qty: u32) -> Result<Vec<BagDelta>, &'static str> {
        if qty == 0 {
            return Ok(Vec::new());
        }
        if self.count_by_tid(tid) < qty {
            return Err("not_enough_items");
        }
        let mut remaining = qty;
        let matching_gids: Vec<u32> = self
            .slots
            .iter()
            .filter(|(_, it)| it.item_template_id == tid)
            .map(|(&gid, _)| gid)
            .collect();
        let mut deltas = Vec::new();
        for gid in matching_gids {
            if remaining == 0 {
                break;
            }
            if let Some(slot) = self.slots.get_mut(&gid) {
                if slot.item_num <= remaining {
                    remaining -= slot.item_num;
                    self.slots.remove(&gid);
                    deltas.push(BagDelta::Remove { gid });
                } else {
                    slot.item_num -= remaining;
                    remaining = 0;
                    deltas.push(BagDelta::Upsert(slot.clone()));
                }
            }
        }
        Ok(deltas)
    }

    /// Builds the full `bag.bagInfo` response table expected by `BagData:parseData`.
    pub fn to_bag_info_dto(&self) -> DtoValue {
        let categories = [
            BagCategory::Arm,
            BagCategory::Item,
            BagCategory::Treasure,
            BagCategory::HeroFrag,
            BagCategory::TreasFrag,
        ];
        let mut grid_start = BTreeMap::new();
        let mut grid_max_num = BTreeMap::new();
        let mut grid_open_times = BTreeMap::new();
        let mut root = BTreeMap::new();

        for cat in categories {
            let name = cat.key_name().to_owned();
            let start = cat.grid_start();
            let max = self.grid_max.get(&cat).copied().unwrap_or(cat.default_max_grids());
            grid_start.insert(name.clone(), DtoValue::from(start));
            grid_max_num.insert(name.clone(), DtoValue::from(max));
            grid_open_times.insert(name.clone(), DtoValue::from(0u32));

            let mut sub_map = BTreeMap::new();
            for (&gid, item) in self.slots.range(start..=(start + 999_999)) {
                sub_map.insert(gid.to_string(), item.to_dto());
            }
            root.insert(name, DtoValue::Map(sub_map));
        }

        // Empty auxiliary sub-bags accessed by `BagData:parseData` in vnsg2
        for extra in [
            "awake",
            "awakeFrag",
            "spirit",
            "pet",
            "petFrag",
            "god",
            "godFrag",
            "godAnimal",
            "godAnimalFrag",
            "topAnimal",
            "topAnimalFrag",
            "brochure",
            "soulSkill",
            "yanWu",
            "yanWuSoul",
            "counsellor",
            "counsellorFrag",
            "soulStone",
            "soulJade",
        ] {
            root.insert(extra.to_owned(), DtoValue::Map(BTreeMap::new()));
        }

        root.insert("daily_use".to_owned(), DtoValue::Map(BTreeMap::new()));
        root.insert("gridStart".to_owned(), DtoValue::Map(grid_start));
        root.insert("gridMaxNum".to_owned(), DtoValue::Map(grid_max_num));
        root.insert("gridOpenTimes".to_owned(), DtoValue::Map(grid_open_times));
        DtoValue::Map(root)
    }

    /// Builds the live `bag.update` push packet (`ret = [ { [gid] = ItemDto_or_Empty } ]`)
    /// matching `BagData:onpushReceiveData(ret)` -> `self:updateItemTb(ret[1])`.
    pub fn build_update_push(deltas: &[BagDelta], now_ts: u64) -> Option<PushPacket> {
        if deltas.is_empty() {
            return None;
        }
        let mut map = BTreeMap::new();
        for delta in deltas {
            match delta {
                BagDelta::Upsert(item) => {
                    map.insert(item.gid.to_string(), item.to_dto());
                }
                BagDelta::Remove { gid } => {
                    // In BagData:updateItemTb, `itemInfo.item_id == nil` deletes the gid!
                    map.insert(gid.to_string(), DtoValue::Map(BTreeMap::new()));
                }
            }
        }
        Some(PushPacket::new(
            push::BAG_UPDATE,
            DtoValue::array([DtoValue::Map(map)]),
            now_ts,
        ))
    }
}

/// Single hero instance matching `HerosData.lua` (`HeroInfo`) and `BaseUnit.lua`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroInstance {
    pub hid: u64,
    pub htid: u32,
    pub level: u32,
    pub exp: u64,
    pub evolve_level: u32,
    pub upgrade_level: u32,
    pub awake_rank: u32,
    pub awake_level: u32,
    pub dress_id: u32,
    pub country: u32,
    pub quality: u32,
    pub max_hp: i64,
    pub cur_hp: i64,
    pub atk: i64,
    pub pdef: i64,
    pub mdef: i64,
    pub speed: i64,
    pub normal_skill_id: u32,
    pub rage_skill_id: u32,
}

impl HeroInstance {
    pub fn from_template(hid: u64, htid: u32, level: u32) -> Self {
        let country = match htid / 100 {
            1 | 101 => 1, // Wei
            2 | 102 => 2, // Shu
            3 | 103 => 3, // Wu
            4 | 104 => 4, // Qun
            _ => (htid % 4) + 1,
        };
        let quality = if htid >= 20_000 {
            6 // Red
        } else if (105..=418).contains(&htid) && (htid % 100) < 20 {
            5 // Orange
        } else if (htid % 100) < 55 {
            4 // Purple
        } else {
            3 // Blue
        };
        let q_mult = i64::from(quality);
        let lv = i64::from(level.max(1));
        let max_hp = 1_200 * q_mult + lv * 350 * q_mult;
        let atk = 220 * q_mult + lv * 55 * q_mult;
        let pdef = 110 * q_mult + lv * 28 * q_mult;
        let mdef = 110 * q_mult + lv * 28 * q_mult;
        let speed = 100 + q_mult * 15 + lv * 2;
        Self {
            hid,
            htid,
            level: level.max(1),
            exp: 0,
            evolve_level: 0,
            upgrade_level: 0,
            awake_rank: 0,
            awake_level: 0,
            dress_id: 0,
            country,
            quality,
            max_hp,
            cur_hp: max_hp,
            atk,
            pdef,
            mdef,
            speed,
            normal_skill_id: 1000 + (htid % 1000),
            rage_skill_id: 2000 + (htid % 1000),
        }
    }

    pub fn recalculate_stats(&mut self) {
        let fresh = Self::from_template(self.hid, self.htid, self.level);
        self.max_hp = fresh.max_hp;
        self.cur_hp = fresh.max_hp;
        self.atk = fresh.atk;
        self.pdef = fresh.pdef;
        self.mdef = fresh.mdef;
        self.speed = fresh.speed;
    }

    pub fn fight_force(&self) -> u64 {
        (self.max_hp as u64 / 5)
            + (self.atk as u64 * 4)
            + (self.pdef as u64 * 2)
            + (self.mdef as u64 * 2)
            + (self.speed as u64 * 3)
    }

    pub fn to_hero_dto(&self) -> DtoValue {
        DtoValue::map([
            ("hid", DtoValue::from(self.hid)),
            ("htid", DtoValue::from(self.htid)),
            ("level", DtoValue::from(self.level)),
            ("exp", DtoValue::from(self.exp)),
            ("soul", DtoValue::from(0u32)),
            ("evolve_level", DtoValue::from(self.evolve_level)),
            ("upgrade_level", DtoValue::from(self.upgrade_level)),
            ("awake_rank", DtoValue::from(self.awake_rank)),
            ("awake_level", DtoValue::from(self.awake_level)),
            ("dress_id", DtoValue::from(self.dress_id)),
            ("hero_star_pos", DtoValue::from(0u32)),
            ("hero_star_level", DtoValue::from(0u32)),
            ("gold_rank", DtoValue::from(0u32)),
            ("gold_level", DtoValue::from(0u32)),
            ("lock", DtoValue::from(0u32)),
        ])
    }
}

/// Player hero roster and 6-slot active formation (`HeroFormationData.lua`).
#[derive(Debug, Clone)]
pub struct HeroRoster {
    pub heroes: BTreeMap<u64, HeroInstance>,
    /// 6-slot lineup (`0` = empty, otherwise `hid`). Slot 0 is always the protagonist hero!
    pub formation: [u64; 6],
    next_hid: u64,
}

impl HeroRoster {
    pub fn set_next_hid(&mut self, next_hid: u64) {
        if next_hid > self.next_hid {
            self.next_hid = next_hid;
        }
    }

    pub fn new_with_protagonist(protagonist_htid: u32, level: u32) -> Self {
        let mut heroes = BTreeMap::new();
        let main_hid = 10_000_001;
        let main_hero = HeroInstance::from_template(main_hid, protagonist_htid, level);
        heroes.insert(main_hid, main_hero);
        Self {
            heroes,
            formation: [main_hid, 0, 0, 0, 0, 0],
            next_hid: main_hid + 1,
        }
    }

    pub fn recruit_hero(&mut self, htid: u32, level: u32, now_ts: u64) -> (HeroInstance, PushPacket) {
        let hid = self.next_hid;
        self.next_hid += 1;
        let hero = HeroInstance::from_template(hid, htid, level);
        self.heroes.insert(hid, hero.clone());
        // Auto-place into an open unlocked slot if fewer than 6 heroes are on formation
        if !self.formation.contains(&hid) {
            if let Some(empty_slot) = self.formation.iter_mut().skip(1).find(|&&mut h| h == 0) {
                *empty_slot = hid;
            }
        }
        let mut push_map = BTreeMap::new();
        push_map.insert(hid.to_string(), hero.to_hero_dto());
        let pkt = PushPacket::new(push::HERO_NEW, DtoValue::Map(push_map), now_ts);
        (hero, pkt)
    }

    pub fn heal_all(&mut self) -> usize {
        let mut count = 0;
        for h in self.heroes.values_mut() {
            if h.cur_hp < h.max_hp {
                count += 1;
            }
            h.cur_hp = h.max_hp;
        }
        count
    }

    pub fn total_fight_force(&self) -> u64 {
        self.formation
            .iter()
            .filter_map(|&hid| (hid != 0).then(|| self.heroes.get(&hid)).flatten())
            .map(HeroInstance::fight_force)
            .sum()
    }

    pub fn to_all_heroes_dto(&self) -> DtoValue {
        let mut m = BTreeMap::new();
        for (&hid, hero) in &self.heroes {
            m.insert(hid.to_string(), hero.to_hero_dto());
        }
        DtoValue::Map(m)
    }

    /// Builds a crash-proof `formation.getPositionInfo` DTO matching `HeroFormationData:parseData`.
    pub fn to_formation_position_info_dto(&self) -> DtoValue {
        let empty_obj = || DtoValue::Map(BTreeMap::new());
        let spirit_6x6 = DtoValue::array((0..6).map(|_| {
            DtoValue::array((0..6).map(|_| empty_obj()))
        }));
        let yanwu_6x4 = DtoValue::array((0..6).map(|_| {
            DtoValue::array((0..4).map(|_| empty_obj()))
        }));
        let arming_6x4 = DtoValue::array((0..6).map(|_| empty_obj()));

        DtoValue::map([
            (
                "position",
                DtoValue::array(self.formation.iter().copied().map(DtoValue::from)),
            ),
            ("partner", DtoValue::array((0..8).map(|_| DtoValue::from(0u32)))),
            ("buddy", DtoValue::array(Vec::<DtoValue>::new())),
            ("buddyLevelInfo", DtoValue::array(Vec::<DtoValue>::new())),
            ("godanimal", DtoValue::array((0..6).map(|_| DtoValue::from(0u32)))),
            ("topanimal", DtoValue::array((0..6).map(|_| DtoValue::from(0u32)))),
            ("allHeros", self.to_all_heroes_dto()),
            ("jointSkill", empty_obj()),
            ("arming", arming_6x4),
            ("spirit", spirit_6x6),
            ("yanWu", yanwu_6x4),
            ("brochure", empty_obj()),
            ("soulSkill", empty_obj()),
            ("bingfu", empty_obj()),
            ("bingshu", empty_obj()),
            ("jobSkill", empty_obj()),
            ("arrCounsellor", DtoValue::array(Vec::<DtoValue>::new())),
            ("soulStone", empty_obj()),
            ("soulJade", empty_obj()),
            ("soulPower", DtoValue::from(0u32)),
            ("soulJadeQi", DtoValue::from(0u32)),
        ])
    }
}

/// Complete player session state (`UserData.lua`, `BagData.lua`, `HerosData.lua`).
#[derive(Debug, Clone)]
pub struct PlayerState {
    pub uid: u64,
    pub pid: String,
    pub uname: String,
    pub utid: u32,
    pub level: u32,
    pub exp: u64,
    pub vip: u32,
    pub vip_exp: u64,
    pub gold: i64,
    pub gold_bind: i64,
    pub copper: i64,
    pub execution: u32,
    pub execution_max: u32,
    pub execution_time: u64,
    pub energy: u32,
    pub energy_max: u32,
    pub energy_time: u64,
    pub prestige: u64,
    pub arena_coin: u64,
    pub hero_soul: u64,
    pub fame: u64,
    pub dress_id: u32,
    pub title_id: u32,
    pub avatar_id: u32,
    pub quality: u32,
    pub guild_name: String,
    pub server_id: String,
    /// Must be 14-digit `"YYYYMMDDHHMMSS"` so `UserData:parseData` & `MainProxy:ctor` never crash!
    pub server_open_time: String,
    pub gm_enabled: bool,
    pub chat_ban_until: u64,
    pub online_accum_secs: u64,
    pub bag: BagState,
    pub roster: HeroRoster,
}

impl PlayerState {
    pub const COPPER_CAP: i64 = 2_000_000_000;

    pub fn new(uid: u64, uname: impl Into<String>, utid: u32) -> Self {
        let level = 1;
        Self {
            uid,
            pid: format!("pid_{uid}"),
            uname: uname.into(),
            utid,
            level,
            exp: 0,
            vip: 0,
            vip_exp: 0,
            gold: 500,
            gold_bind: 0,
            copper: 100_000,
            execution: 120,
            execution_max: 120,
            execution_time: 1_795_500_000,
            energy: 30,
            energy_max: 30,
            energy_time: 1_795_500_000,
            prestige: 0,
            arena_coin: 0,
            hero_soul: 0,
            fame: 0,
            dress_id: 0,
            title_id: 0,
            avatar_id: utid,
            quality: 4,
            guild_name: String::new(),
            server_id: "game2900161".to_owned(),
            server_open_time: "20260101090000".to_owned(),
            gm_enabled: true,
            chat_ban_until: 0,
            online_accum_secs: 1_800,
            bag: BagState::new(),
            roster: HeroRoster::new_with_protagonist(utid, level),
        }
    }

    pub fn fight_force(&self) -> u64 {
        self.roster.total_fight_force()
    }

    /// Computes unlocked formation positions (`1..=6`) from player `level` (`MainTown:initLineUp`).
    pub fn hero_position_map(&self) -> BTreeMap<String, DtoValue> {
        let unlock_levels = [1u32, 1, 10, 18, 26, 32];
        let mut map = BTreeMap::new();
        for (idx, &req_lv) in unlock_levels.iter().enumerate() {
            map.insert((idx + 1).to_string(), DtoValue::from(self.level >= req_lv));
        }
        map
    }

    /// Builds a live `user.updateUser` delta push (`SC_USER_UPDATEUSER = "user.updateUser"`).
    pub fn build_user_update_push(
        fields: impl IntoIterator<Item = (&'static str, DtoValue)>,
        now_ts: u64,
    ) -> PushPacket {
        PushPacket::new(push::USER_UPDATE, DtoValue::map(fields), now_ts)
    }

    /// Builds full `user.getUser` DTO (`UserData:parseData`), guaranteeing all MainTown invariants.
    pub fn to_user_dto(&self) -> DtoValue {
        DtoValue::map([
            ("uid", DtoValue::from(self.uid)),
            ("pid", DtoValue::from(self.pid.as_str())),
            ("uname", DtoValue::from(self.uname.as_str())),
            ("utid", DtoValue::from(self.utid)),
            ("htid", DtoValue::from(self.utid)),
            ("level", DtoValue::from(self.level)),
            ("exp", DtoValue::from(self.exp)),
            ("vip", DtoValue::from(self.vip)),
            ("vip_exp", DtoValue::from(self.vip_exp)),
            ("gold", DtoValue::from(self.gold)),
            ("gold_bind", DtoValue::from(self.gold_bind)),
            ("copper", DtoValue::from(self.copper)),
            ("execution", DtoValue::from(self.execution)),
            ("execution_time", DtoValue::from(self.execution_time)),
            ("energy", DtoValue::from(self.energy)),
            ("energy_time", DtoValue::from(self.energy_time)),
            ("prestige", DtoValue::from(self.prestige)),
            ("arena_coin", DtoValue::from(self.arena_coin)),
            ("hero_soul", DtoValue::from(self.hero_soul)),
            ("fame", DtoValue::from(self.fame)),
            ("fight_force", DtoValue::from(self.fight_force())),
            ("dress_id", DtoValue::from(self.dress_id)),
            ("title_id", DtoValue::from(self.title_id)),
            ("server_id", DtoValue::from(self.server_id.as_str())),
            ("server_open_time", DtoValue::from(self.server_open_time.as_str())),
            ("hero_position_map", DtoValue::Map(self.hero_position_map())),
            ("dress_info", DtoValue::Map(BTreeMap::new())),
            ("dress_expire", DtoValue::Map(BTreeMap::new())),
            ("va_user", DtoValue::Map(BTreeMap::new())),
        ])
    }
}
