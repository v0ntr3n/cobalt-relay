//! Player state, `BagData` inventory grids, `HerosData` roster, `HeroFormationData` lineup,
//! and live `user.updateUser` / `bag.update` / `re.hero.new` delta push builders.

use std::collections::BTreeMap;

use crate::protocol::{dto_arr, dto_map, push, DtoValue, PushPacket};

/// Bag sub-container grid ranges verified from `BagData.lua:getBagTypeByGid` & `gserver_player::BagType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BagCategory {
    Treasure,
    Arm,
    Item,
    ArmFrag,
    HeroFrag,
    TreasFrag,
}

impl BagCategory {
    /// Complete ordered list of active gameplay bag categories.
    pub const ALL: [Self; 6] = [
        Self::Treasure,
        Self::Arm,
        Self::Item,
        Self::ArmFrag,
        Self::HeroFrag,
        Self::TreasFrag,
    ];

    pub const fn key_name(self) -> &'static str {
        match self {
            Self::Treasure => "treasure",
            Self::Arm => "arm",
            Self::Item => "item",
            Self::ArmFrag => "armFrag",
            Self::HeroFrag => "heroFrag",
            Self::TreasFrag => "treasFrag",
        }
    }

    /// Canonical `vnsg2` sub-bag key matching `BagConsts.lua` & `gserver_player::BagType::vnsg2_key`.
    pub const fn vnsg2_key(self) -> &'static str {
        match self {
            Self::Treasure => "treasure",
            Self::Arm => "arm",
            Self::Item => "item",
            Self::ArmFrag => "fragment",
            Self::HeroFrag => "soul",
            Self::TreasFrag => "treasure_fragment",
        }
    }

    /// Inclusive `(grid_start, grid_end)` range enforced by `BagData:getBagTypeByGid` (`gserver_player::BagType::gid_range`).
    pub const fn gid_range(self) -> (u32, u32) {
        match self {
            Self::Treasure => (10_001, 20_000),
            Self::Arm => (20_001, 30_000),
            Self::Item => (30_001, 40_000),
            Self::ArmFrag => (40_001, 50_000),
            Self::HeroFrag => (50_001, 60_000),
            Self::TreasFrag => (80_001, 90_000),
        }
    }

    pub const fn grid_start(self) -> u32 {
        self.gid_range().0
    }

    pub const fn default_max_grids(self) -> u32 {
        match self {
            Self::Item => 300,
            Self::Arm | Self::ArmFrag | Self::HeroFrag => 200,
            Self::Treasure | Self::TreasFrag => 150,
        }
    }

    pub const fn is_stackable(self) -> bool {
        !matches!(self, Self::Arm | Self::Treasure)
    }

    /// Classifies an item template ID into its GServer bag sub-table (`ItemFactory.lua`).
    pub fn from_template_id(tid: u32) -> Self {
        match tid {
            100_001..=180_000 => Self::Arm,
            180_001..=200_000 => Self::Treasure,
            3_100_001..=3_200_000 => Self::ArmFrag,
            50_001..=60_000 => Self::HeroFrag,
            400_000_001..=500_000_000 => Self::TreasFrag,
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
        dto_map! {
            "item_id" => self.item_id,
            "item_template_id" => self.item_template_id,
            "item_num" => self.item_num,
            "item_time" => self.item_time,
            "va_item_text" => dto_map!(),
        }
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
    const EXTRA_BAG_KEYS: [&'static str; 20] = [
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
        "daily_use",
    ];

    pub fn new() -> Self {
        let grid_max = BagCategory::ALL
            .into_iter()
            .map(|cat| (cat, cat.default_max_grids()))
            .collect();
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
        let (start, end) = cat.gid_range();
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
        if cat.is_stackable() {
            if let Some((_, existing)) = self
                .slots
                .iter_mut()
                .find(|(_, it)| it.item_template_id == tid)
            {
                existing.item_num = existing.item_num.saturating_add(qty);
                existing.item_time = now_ts;
                return Ok(vec![BagDelta::Upsert(existing.clone())]);
            }
        }
        if self.is_category_full(cat) {
            return Err("bag_full");
        }
        let (start, hard_end) = cat.gid_range();
        let max = self.grid_max.get(&cat).copied().unwrap_or(100);
        let cap_end = start.saturating_add(max.saturating_sub(1)).min(hard_end);
        let gid = (start..=cap_end)
            .find(|candidate| !self.slots.contains_key(candidate))
            .ok_or("bag_full")?;
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
        let mut grid_start = BTreeMap::new();
        let mut grid_max_num = BTreeMap::new();
        let mut grid_open_times = BTreeMap::new();
        let mut root = BTreeMap::new();

        for cat in BagCategory::ALL {
            let (start, end) = cat.gid_range();
            let max = self.grid_max.get(&cat).copied().unwrap_or(cat.default_max_grids());
            let sub_map: BTreeMap<String, DtoValue> = self
                .slots
                .range(start..=end)
                .map(|(&gid, item)| (gid.to_string(), item.to_dto()))
                .collect();

            for key in [cat.key_name(), cat.vnsg2_key()] {
                grid_start.insert(key.to_owned(), DtoValue::from(start));
                grid_max_num.insert(key.to_owned(), DtoValue::from(max));
                grid_open_times.insert(key.to_owned(), DtoValue::from(0u32));
                root.insert(key.to_owned(), DtoValue::Map(sub_map.clone()));
            }
        }

        for extra in Self::EXTRA_BAG_KEYS {
            root.insert(extra.to_owned(), dto_map!());
        }
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
        let map: BTreeMap<String, DtoValue> = deltas
            .iter()
            .map(|delta| match delta {
                BagDelta::Upsert(item) => (item.gid.to_string(), item.to_dto()),
                // In BagData:updateItemTb, `itemInfo.item_id == nil` deletes the gid!
                BagDelta::Remove { gid } => (gid.to_string(), dto_map!()),
            })
            .collect();
        Some(PushPacket::new(
            push::BAG_UPDATE,
            dto_arr![DtoValue::Map(map)],
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
        dto_map! {
            "hid" => self.hid,
            "htid" => self.htid,
            "level" => self.level,
            "exp" => self.exp,
            "soul" => 0u32,
            "evolve_level" => self.evolve_level,
            "upgrade_level" => self.upgrade_level,
            "awake_rank" => self.awake_rank,
            "awake_level" => self.awake_level,
            "dress_id" => self.dress_id,
            "hero_star_pos" => 0u32,
            "hero_star_level" => 0u32,
            "gold_rank" => 0u32,
            "gold_level" => 0u32,
            "lock" => 0u32,
        }
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
        heroes.insert(main_hid, HeroInstance::from_template(main_hid, protagonist_htid, level));
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
        if !self.formation.contains(&hid) {
            if let Some(empty_slot) = self.formation.iter_mut().skip(1).find(|&&mut h| h == 0) {
                *empty_slot = hid;
            }
        }
        let pkt = PushPacket::new(
            push::HERO_NEW,
            dto_map! { hid.to_string() => hero.to_hero_dto() },
            now_ts,
        );
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
        DtoValue::Map(
            self.heroes
                .iter()
                .map(|(&hid, hero)| (hid.to_string(), hero.to_hero_dto()))
                .collect(),
        )
    }

    /// Builds a crash-proof `formation.getPositionInfo` DTO matching `HeroFormationData:parseData`.
    pub fn to_formation_position_info_dto(&self) -> DtoValue {
        dto_map! {
            "position" => DtoValue::array(self.formation.iter().copied().map(DtoValue::from)),
            "partner" => dto_arr![0u32; 8],
            "buddy" => dto_arr![],
            "buddyLevelInfo" => dto_arr![],
            "godanimal" => dto_arr![0u32; 6],
            "topanimal" => dto_arr![0u32; 6],
            "allHeros" => self.to_all_heroes_dto(),
            "jointSkill" => dto_map!(),
            "arming" => dto_arr![dto_map!(); 6],
            "spirit" => dto_arr![dto_arr![dto_map!(); 6]; 6],
            "yanWu" => dto_arr![dto_arr![dto_map!(); 4]; 6],
            "brochure" => dto_map!(),
            "soulSkill" => dto_map!(),
            "bingfu" => dto_map!(),
            "bingshu" => dto_map!(),
            "jobSkill" => dto_map!(),
            "arrCounsellor" => dto_arr![],
            "soulStone" => dto_map!(),
            "soulJade" => dto_map!(),
            "soulPower" => 0u32,
            "soulJadeQi" => 0u32,
        }
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
        [1u32, 1, 10, 18, 26, 32]
            .iter()
            .enumerate()
            .map(|(idx, &req_lv)| ((idx + 1).to_string(), DtoValue::from(self.level >= req_lv)))
            .collect()
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
        dto_map! {
            "uid" => self.uid,
            "pid" => &self.pid,
            "uname" => &self.uname,
            "utid" => self.utid,
            "htid" => self.utid,
            "level" => self.level,
            "exp" => self.exp,
            "vip" => self.vip,
            "vip_exp" => self.vip_exp,
            "gold" => self.gold,
            "gold_bind" => self.gold_bind,
            "copper" => self.copper,
            "execution" => self.execution,
            "execution_time" => self.execution_time,
            "energy" => self.energy,
            "energy_time" => self.energy_time,
            "prestige" => self.prestige,
            "arena_coin" => self.arena_coin,
            "hero_soul" => self.hero_soul,
            "fame" => self.fame,
            "fight_force" => self.fight_force(),
            "dress_id" => self.dress_id,
            "title_id" => self.title_id,
            "server_id" => &self.server_id,
            "server_open_time" => &self.server_open_time,
            "hero_position_map" => DtoValue::Map(self.hero_position_map()),
            "dress_info" => dto_map!(),
            "dress_expire" => dto_map!(),
            "va_user" => dto_map!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BagCategory, BagDelta, BagState};
    use gserver_player::BagType;

    #[test]
    fn test_bag_category_grid_start_parity_with_gserver_player() {
        let expected_pairs = [
            (BagCategory::Treasure, BagType::Treasure, 10_001),
            (BagCategory::Arm, BagType::Arm, 20_001),
            (BagCategory::Item, BagType::Item, 30_001),
            (BagCategory::ArmFrag, BagType::Fragment, 40_001),
            (BagCategory::HeroFrag, BagType::Soul, 50_001),
            (BagCategory::TreasFrag, BagType::TreasureFragment, 80_001),
        ];
        assert_eq!(expected_pairs.len(), BagCategory::ALL.len());

        for (cat, bag_type, expected_start) in expected_pairs {
            assert_eq!(
                BagType::from_key(cat.vnsg2_key()),
                Some(bag_type),
                "BagCategory::{cat:?} vnsg2_key must resolve to {bag_type:?}"
            );
            assert_eq!(
                cat.grid_start(),
                expected_start,
                "BagCategory::{cat:?} grid_start mismatch"
            );
            assert_eq!(
                cat.grid_start(),
                bag_type.grid_start(),
                "BagCategory::{cat:?} grid_start drifted from gserver_player::BagType::{bag_type:?}"
            );
            assert_eq!(
                cat.gid_range(),
                bag_type.gid_range(),
                "BagCategory::{cat:?} gid_range drifted from gserver_player::BagType::{bag_type:?}"
            );
            assert_eq!(
                cat.is_stackable(),
                bag_type.is_stackable(),
                "BagCategory::{cat:?} stackable semantics drifted from gserver_player::BagType::{bag_type:?}"
            );
            let (lo, hi) = cat.gid_range();
            assert_eq!(BagType::from_gid(lo), Some(bag_type));
            assert_eq!(BagType::from_gid(hi), Some(bag_type));
        }
    }

    #[test]
    fn test_bag_state_allocates_disjoint_canonical_gids_across_all_categories() {
        let mut bag = BagState::new();
        let samples = [
            (182_001u32, BagCategory::Treasure, BagType::Treasure),
            (112_001u32, BagCategory::Arm, BagType::Arm),
            (16_012u32, BagCategory::Item, BagType::Item),
            (3_113_001u32, BagCategory::ArmFrag, BagType::Fragment),
            (50_101u32, BagCategory::HeroFrag, BagType::Soul),
            (400_000_001u32, BagCategory::TreasFrag, BagType::TreasureFragment),
        ];

        for (tid, cat, bag_type) in samples {
            let deltas = bag.add_item(tid, 1, 1_795_500_000).expect("add sample item");
            let BagDelta::Upsert(ref item) = deltas[0] else {
                panic!("expected Upsert delta");
            };
            assert_eq!(item.gid, cat.grid_start());
            assert_eq!(BagType::from_gid(item.gid), Some(bag_type));
        }

        for (_, cat, _) in samples {
            assert_eq!(
                bag.used_grids(cat),
                1,
                "category {cat:?} range must remain strictly disjoint"
            );
        }

        let dto = bag.to_bag_info_dto();
        let gs = dto.get("gridStart").and_then(|v| v.as_map()).expect("gridStart map");
        for cat in BagCategory::ALL {
            assert_eq!(
                gs.get(cat.key_name()).and_then(|v| v.as_i64()),
                Some(i64::from(cat.grid_start()))
            );
            assert_eq!(
                gs.get(cat.vnsg2_key()).and_then(|v| v.as_i64()),
                Some(i64::from(cat.grid_start()))
            );
        }
    }
}
