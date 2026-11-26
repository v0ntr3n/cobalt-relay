//! Player state, `BagData` inventory grids, `HerosData` roster, `HeroFormationData` lineup,
//! and live `user.updateUser` / `bag.update` / `re.hero.new` delta push builders.
//!
//! GID ranges (`10_001..=740_000`), item template classifications, hero DTO fields, and
//! currency keys (`PriceTypeConst` -> `UserData`) are verified directly against
//! `BagData.lua`, `BagConsts.lua`, `ItemFactory.lua`, `HeroInfo.lua`, and `UserData.lua`.

use std::collections::BTreeMap;

use crate::protocol::{push, DtoValue, PushPacket};

/// All sub-bag grid categories verified against `BagData:getBagTypeByGid` (`BagData.lua`)
/// and `BagConsts.lua` (`10_001..=740_000`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BagCategory {
    Treasure,
    Arm,
    Item,
    ArmFrag,
    HeroFrag,
    Awake,
    AwakeFrag,
    TreasFrag,
    Spirit,
    Pet,
    PetFrag,
    GodArm,
    GodArmFrag,
    HalidomFrag,
    Gem,
    GodAnimal,
    GodAnimalFrag,
    HeroLegend,
    HeroLegendFrag,
    Brochure,
    BrochureFrag,
    BingFuFrag,
    SoulSkill,
    StatueFrag,
    BingShuFrag,
    JobSkillFrag,
    YanWu,
    YanWuFrag,
    YanWuSoul,
    Counsellor,
    CounsellorFrag,
    CounsellorEquip,
    CounsellorEquipFrag,
    SoulStone,
    SoulStoneFrag,
    SoulJade,
    SoulJadeFrag,
    WareHouse,
    PloyFrag,
    TopAnimal,
    TopAnimalFrag,
    GreatSealFrag,
}

impl BagCategory {
    pub const ALL: [BagCategory; 42] = [
        Self::Treasure,
        Self::Arm,
        Self::Item,
        Self::ArmFrag,
        Self::HeroFrag,
        Self::Awake,
        Self::AwakeFrag,
        Self::TreasFrag,
        Self::Spirit,
        Self::Pet,
        Self::PetFrag,
        Self::GodArm,
        Self::GodArmFrag,
        Self::HalidomFrag,
        Self::Gem,
        Self::GodAnimal,
        Self::GodAnimalFrag,
        Self::HeroLegend,
        Self::HeroLegendFrag,
        Self::Brochure,
        Self::BrochureFrag,
        Self::BingFuFrag,
        Self::SoulSkill,
        Self::StatueFrag,
        Self::BingShuFrag,
        Self::JobSkillFrag,
        Self::YanWu,
        Self::YanWuFrag,
        Self::YanWuSoul,
        Self::Counsellor,
        Self::CounsellorFrag,
        Self::CounsellorEquip,
        Self::CounsellorEquipFrag,
        Self::SoulStone,
        Self::SoulStoneFrag,
        Self::SoulJade,
        Self::SoulJadeFrag,
        Self::WareHouse,
        Self::PloyFrag,
        Self::TopAnimal,
        Self::TopAnimalFrag,
        Self::GreatSealFrag,
    ];

    /// Exact Lua sub-bag key in `BagConsts.lua` / `bag.bagInfo`.
    pub const fn key_name(self) -> &'static str {
        match self {
            Self::Treasure => "treasure",
            Self::Arm => "arm",
            Self::Item => "item",
            Self::ArmFrag => "fragment",
            Self::HeroFrag => "soul",
            Self::Awake => "awake",
            Self::AwakeFrag => "awake_fragment",
            Self::TreasFrag => "treasure_fragment",
            Self::Spirit => "spirit",
            Self::Pet => "pet",
            Self::PetFrag => "pet_fragment",
            Self::GodArm => "god_arm",
            Self::GodArmFrag => "god_arm_fragment",
            Self::HalidomFrag => "halidom_fragment",
            Self::Gem => "gem",
            Self::GodAnimal => "god_animal",
            Self::GodAnimalFrag => "god_animal_fragment",
            Self::HeroLegend => "hero_legend",
            Self::HeroLegendFrag => "hero_legend_fragment",
            Self::Brochure => "brochure",
            Self::BrochureFrag => "brochure_fragment",
            Self::BingFuFrag => "bingfu_fragment",
            Self::SoulSkill => "soul_skill",
            Self::StatueFrag => "statue_fragment",
            Self::BingShuFrag => "bingshu_fragment",
            Self::JobSkillFrag => "jobskill_fragment",
            Self::YanWu => "yanwu",
            Self::YanWuFrag => "yanwu_fragment",
            Self::YanWuSoul => "yanwu_soul",
            Self::Counsellor => "counsellor",
            Self::CounsellorFrag => "counsellor_fragment",
            Self::CounsellorEquip => "counsellor_equip",
            Self::CounsellorEquipFrag => "counsellor_equip_fragment",
            Self::SoulStone => "soul_stone",
            Self::SoulStoneFrag => "soul_stone_fragment",
            Self::SoulJade => "soul_jade",
            Self::SoulJadeFrag => "soul_jade_fragment",
            Self::WareHouse => "ware_house",
            Self::PloyFrag => "ploy_fragment",
            Self::TopAnimal => "top_animal",
            Self::TopAnimalFrag => "top_animal_fragment",
            Self::GreatSealFrag => "great_seal_fragment",
        }
    }

    /// Exact inclusive `(grid_start, grid_end)` range enforced by `BagData:getBagTypeByGid` (`BagData.lua`).
    pub const fn gid_range(self) -> (u32, u32) {
        match self {
            Self::Treasure => (10_001, 20_000),
            Self::Arm => (20_001, 30_000),
            Self::Item => (30_001, 40_000),
            Self::ArmFrag => (40_001, 50_000),
            Self::HeroFrag => (50_001, 60_000),
            Self::Awake => (60_001, 70_000),
            Self::AwakeFrag => (70_001, 80_000),
            Self::TreasFrag => (80_001, 90_000),
            Self::Spirit => (90_001, 100_000),
            Self::Pet => (100_001, 110_000),
            Self::PetFrag => (200_001, 210_000),
            Self::GodArm => (210_001, 220_000),
            Self::GodArmFrag => (220_001, 230_000),
            Self::HalidomFrag => (230_001, 240_000),
            Self::Gem => (240_001, 250_000),
            Self::GodAnimal => (310_001, 320_000),
            Self::GodAnimalFrag => (320_001, 330_000),
            Self::HeroLegend => (410_001, 420_000),
            Self::HeroLegendFrag => (420_001, 430_000),
            Self::Brochure => (510_001, 520_000),
            Self::BrochureFrag => (520_001, 530_000),
            Self::BingFuFrag => (530_001, 540_000),
            Self::SoulSkill => (540_001, 550_000),
            Self::StatueFrag => (550_001, 560_000),
            Self::BingShuFrag => (560_001, 570_000),
            Self::JobSkillFrag => (570_001, 580_000),
            Self::YanWu => (580_001, 590_000),
            Self::YanWuFrag => (590_001, 600_000),
            Self::YanWuSoul => (600_001, 610_000),
            Self::Counsellor => (610_001, 620_000),
            Self::CounsellorFrag => (620_001, 630_000),
            Self::CounsellorEquip => (630_001, 640_000),
            Self::CounsellorEquipFrag => (640_001, 650_000),
            Self::SoulStone => (650_001, 660_000),
            Self::SoulStoneFrag => (660_001, 670_000),
            Self::SoulJade => (670_001, 680_000),
            Self::SoulJadeFrag => (680_001, 690_000),
            Self::WareHouse => (690_001, 700_000),
            Self::PloyFrag => (700_001, 710_000),
            Self::TopAnimal => (710_001, 720_000),
            Self::TopAnimalFrag => (720_001, 730_000),
            Self::GreatSealFrag => (730_001, 740_000),
        }
    }

    pub const fn grid_start(self) -> u32 {
        self.gid_range().0
    }

    pub const fn grid_end(self) -> u32 {
        self.gid_range().1
    }

    pub const fn default_max_grids(self) -> u32 {
        match self {
            Self::Item => 300,
            Self::Arm
            | Self::Spirit
            | Self::Pet
            | Self::GodArm
            | Self::Gem
            | Self::YanWu
            | Self::TopAnimal => 200,
            Self::Treasure => 150,
            Self::Awake | Self::GodAnimal => 100,
            _ => 10_000,
        }
    }

    pub const fn is_stackable(self) -> bool {
        !matches!(
            self,
            Self::Arm
                | Self::Treasure
                | Self::Awake
                | Self::Spirit
                | Self::Pet
                | Self::GodArm
                | Self::GodAnimal
                | Self::YanWu
                | Self::Counsellor
                | Self::CounsellorEquip
                | Self::SoulStone
                | Self::SoulJade
                | Self::TopAnimal
        )
    }

    /// Resolves a `gid` back to its owning sub-bag (`BagData:getBagTypeByGid`).
    pub fn from_gid(gid: u32) -> Option<Self> {
        for cat in Self::ALL {
            let (lo, hi) = cat.gid_range();
            if gid >= lo && gid <= hi {
                return Some(cat);
            }
        }
        None
    }

    /// Classifies an item template ID into its exact `vnsg2` bag sub-table (`ItemFactory.lua:278-374`).
    pub fn from_template_id(tid: u32) -> Self {
        match tid {
            10_001..=40_000 => Self::Item,
            50_001..=60_000 => Self::HeroFrag,
            100_001..=180_000 => Self::Arm,
            180_001..=200_000 => Self::Treasure,
            200_001..=300_000 => Self::Spirit,
            300_001..=400_000 => Self::Awake,
            400_001..=500_000 | 5_000_001..=5_200_000 => Self::Pet,
            500_001..=600_000 => Self::GodArm,
            600_001..=700_000 => Self::GodAnimal,
            700_001..=800_000 => Self::HeroLegend,
            800_001..=900_000 => Self::Brochure,
            900_001..=1_000_000 => Self::HalidomFrag,
            1_000_001..=1_100_000 => Self::Gem,
            1_100_001..=1_300_000 | 1_500_001..=1_600_000 | 1_800_001..=1_850_000 => Self::Item,
            1_300_001..=1_400_000 => Self::BingFuFrag,
            1_400_001..=1_500_000 => Self::SoulSkill,
            1_600_001..=1_700_000 => Self::StatueFrag,
            1_700_001..=1_800_000 => Self::BingShuFrag,
            1_850_001..=1_900_000 => Self::JobSkillFrag,
            3_100_001..=3_200_000 => Self::ArmFrag,
            3_300_001..=3_400_000 => Self::AwakeFrag,
            3_400_001..=3_500_000 => Self::PetFrag,
            3_500_001..=3_600_000 => Self::GodArmFrag,
            3_600_001..=3_700_000 => Self::GodAnimalFrag,
            3_700_001..=3_800_000 => Self::HeroLegendFrag,
            3_800_001..=3_900_000 => Self::BrochureFrag,
            3_900_001..=4_000_000 => Self::YanWu,
            4_000_001..=4_100_000 => Self::YanWuFrag,
            4_100_001..=4_200_000 => Self::YanWuSoul,
            4_200_001..=4_300_000 => Self::CounsellorFrag,
            4_300_001..=4_400_000 => Self::Counsellor,
            4_400_001..=4_500_000 => Self::CounsellorEquip,
            4_500_001..=4_600_000 => Self::CounsellorEquipFrag,
            4_600_001..=4_700_000 => Self::SoulStone,
            4_700_001..=4_800_000 => Self::SoulStoneFrag,
            4_800_001..=4_900_000 => Self::SoulJade,
            4_900_001..=5_000_000 => Self::SoulJadeFrag,
            5_300_001..=5_400_000 => Self::PloyFrag,
            5_400_001..=5_500_000 => Self::TopAnimal,
            5_500_001..=5_600_000 => Self::TopAnimalFrag,
            5_600_001..=5_700_000 => Self::GreatSealFrag,
            400_000_001..=500_000_000 => Self::TreasFrag,
            _ => Self::Item,
        }
    }
}

/// Builds non-nil `va_item_text` defaults required by `ArmItem.lua`, `TreasureItem.lua`,
/// `AwakeItem.lua`, `PetItem.lua`, and `GodItem.lua`.
fn default_va_item_text_dto(cat: BagCategory) -> DtoValue {
    match cat {
        BagCategory::Arm
        | BagCategory::Treasure
        | BagCategory::Awake
        | BagCategory::Pet
        | BagCategory::GodArm
        | BagCategory::GodAnimal
        | BagCategory::TopAnimal
        | BagCategory::Spirit => DtoValue::map([
            ("reinforce_level", DtoValue::from(1u32)),
            ("reinforce_exp", DtoValue::from(0u32)),
            ("reinforce_cost", DtoValue::from(0u32)),
            ("progress_level", DtoValue::from(0u32)),
            ("progress_exp", DtoValue::from(0u32)),
            ("star_level", DtoValue::from(0u32)),
            ("rune_id", DtoValue::from(0u32)),
        ]),
        _ => DtoValue::Map(BTreeMap::new()),
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
        let cat = BagCategory::from_template_id(self.item_template_id);
        DtoValue::map([
            ("item_id", DtoValue::from(self.item_id)),
            ("item_template_id", DtoValue::from(self.item_template_id)),
            ("item_num", DtoValue::from(self.item_num)),
            ("item_time", DtoValue::from(self.item_time)),
            ("va_item_text", default_va_item_text_dto(cat)),
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
        for cat in BagCategory::ALL {
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
        let (start, end) = cat.gid_range();
        self.slots.range(start..=end).count() as u32
    }

    pub fn is_category_full(&self, cat: BagCategory) -> bool {
        let max = self
            .grid_max
            .get(&cat)
            .copied()
            .unwrap_or(cat.default_max_grids());
        self.used_grids(cat) >= max
    }

    fn allocate_single_slot(
        &mut self,
        cat: BagCategory,
        tid: u32,
        qty: u32,
        now_ts: u64,
    ) -> Result<BagItem, &'static str> {
        if self.is_category_full(cat) {
            return Err("bag_full");
        }
        let (start, range_end) = cat.gid_range();
        let max = self
            .grid_max
            .get(&cat)
            .copied()
            .unwrap_or(cat.default_max_grids());
        let cap_end = start.saturating_add(max.saturating_sub(1)).min(range_end);
        let mut chosen_gid = None;
        for candidate in start..=cap_end {
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
        Ok(item)
    }

    /// Adds `qty` of `tid`, stacking onto an existing slot for stackable items/fragments or allocating new `gid`(s)
    /// within the authentic `BagData:getBagTypeByGid` sub-bag range.
    pub fn add_item(&mut self, tid: u32, qty: u32, now_ts: u64) -> Result<Vec<BagDelta>, &'static str> {
        if qty == 0 {
            return Ok(Vec::new());
        }
        let cat = BagCategory::from_template_id(tid);
        if cat.is_stackable() {
            let (start, end) = cat.gid_range();
            if let Some((_, existing)) = self
                .slots
                .range_mut(start..=end)
                .find(|(_, it)| it.item_template_id == tid)
            {
                existing.item_num = existing.item_num.saturating_add(qty);
                existing.item_time = now_ts;
                return Ok(vec![BagDelta::Upsert(existing.clone())]);
            }
            let item = self.allocate_single_slot(cat, tid, qty, now_ts)?;
            return Ok(vec![BagDelta::Upsert(item)]);
        }

        // Non-stackable equipment (`Arm`, `Treasure`, `GodArm`, `SoulStone`, `TopAnimal`, etc.):
        // check space up front so allocation is atomic, then allocate one grid per instance.
        let max = self
            .grid_max
            .get(&cat)
            .copied()
            .unwrap_or(cat.default_max_grids());
        if self.used_grids(cat).saturating_add(qty) > max {
            return Err("bag_full");
        }
        let mut deltas = Vec::with_capacity(qty as usize);
        for _ in 0..qty {
            let item = self.allocate_single_slot(cat, tid, 1, now_ts)?;
            deltas.push(BagDelta::Upsert(item));
        }
        Ok(deltas)
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
            let name = cat.key_name().to_owned();
            let (start, end) = cat.gid_range();
            let max = self
                .grid_max
                .get(&cat)
                .copied()
                .unwrap_or(cat.default_max_grids());
            grid_start.insert(name.clone(), DtoValue::from(start));
            grid_max_num.insert(name.clone(), DtoValue::from(max));
            grid_open_times.insert(name.clone(), DtoValue::from(0u32));

            let mut sub_map = BTreeMap::new();
            for (&gid, item) in self.slots.range(start..=end) {
                if item.item_num > 0 {
                    sub_map.insert(gid.to_string(), item.to_dto());
                }
            }
            root.insert(name, DtoValue::Map(sub_map));
        }

        root.insert("daily_use".to_owned(), DtoValue::Map(BTreeMap::new()));
        root.insert("gridStart".to_owned(), DtoValue::Map(grid_start));
        root.insert("gridMaxNum".to_owned(), DtoValue::Map(grid_max_num));
        root.insert("gridOpenTimes".to_owned(), DtoValue::Map(grid_open_times));
        DtoValue::Map(root)
    }

    /// Builds the live `bag.update` push packet (`ret = [ { [gid] = ItemDto_or_Empty } ]`)
    /// matching `BagData:onpushReceiveData(ret)` -> `self:updateItemTb(ret[1])`.
    /// When a stack reaches `0` (either via `BagDelta::Remove` or `Upsert` with `item_num == 0`),
    /// emits an empty map `{}` at `[gidStr]` so `BagData:updateItemTb` sees `item_id == nil` and deletes the slot.
    pub fn build_update_push(deltas: &[BagDelta], now_ts: u64) -> Option<PushPacket> {
        if deltas.is_empty() {
            return None;
        }
        let mut map = BTreeMap::new();
        for delta in deltas {
            match delta {
                BagDelta::Upsert(item) if item.item_num > 0 => {
                    map.insert(item.gid.to_string(), item.to_dto());
                }
                BagDelta::Upsert(item) => {
                    map.insert(item.gid.to_string(), DtoValue::Map(BTreeMap::new()));
                }
                BagDelta::Remove { gid } => {
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

/// Single hero instance matching `HerosData.lua` (`HeroInfo.lua:109-151`) and `BaseUnit.lua`.
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
        Self::from_template_with_awake(hid, htid, level, 0)
    }

    pub fn from_template_with_awake(hid: u64, htid: u32, level: u32, awake_rank: u32) -> Self {
        let country = gserver_data::hero_skill_entry_by_htid(htid)
            .filter(|e| e.country > 0)
            .map(|e| u32::from(e.country))
            .unwrap_or_else(|| match htid / 100 {
                1 | 101 => 1, // Wei
                2 | 102 => 2, // Shu
                3 | 103 => 3, // Wu
                4 | 104 => 4, // Qun
                _ => (htid % 4) + 1,
            });
        let quality = gserver_data::hero_by_htid(htid)
            .map(|tpl| u32::from(tpl.init_quality))
            .unwrap_or_else(|| {
                if htid >= 20_000
                    || matches!(htid, 101 | 102 | 201 | 202 | 301 | 302 | 401 | 402)
                {
                    6 // Red
                } else if (103..=418).contains(&htid) && (htid % 100) < 20 {
                    5 // Orange
                } else if (htid % 100) < 55 {
                    4 // Purple
                } else if (htid % 100) < 62 {
                    3 // Blue
                } else {
                    2 // Green
                }
            });
        let q_mult = i64::from(quality);
        let lv = i64::from(level.max(1));
        let max_hp = 1_200 * q_mult + lv * 350 * q_mult;
        let atk = 220 * q_mult + lv * 55 * q_mult;
        let pdef = 110 * q_mult + lv * 28 * q_mult;
        let mdef = 110 * q_mult + lv * 28 * q_mult;
        let speed = 100 + q_mult * 15 + lv * 2;
        let resolved_skills = gserver_data::resolve_hero_battle_skills_with_awake(htid, awake_rank);
        Self {
            hid,
            htid,
            level: level.max(1),
            exp: 0,
            evolve_level: 0,
            upgrade_level: 0,
            awake_rank,
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
            normal_skill_id: resolved_skills.normal_skill_id,
            rage_skill_id: resolved_skills.rage_skill_id,
        }
    }

    pub fn recalculate_stats(&mut self) {
        let fresh =
            Self::from_template_with_awake(self.hid, self.htid, self.level, self.awake_rank);
        self.max_hp = fresh.max_hp;
        self.cur_hp = fresh.max_hp;
        self.atk = fresh.atk;
        self.pdef = fresh.pdef;
        self.mdef = fresh.mdef;
        self.speed = fresh.speed;
        self.normal_skill_id = fresh.normal_skill_id;
        self.rage_skill_id = fresh.rage_skill_id;
    }

    pub fn fight_force(&self) -> u64 {
        (self.max_hp as u64 / 5)
            + (self.atk as u64 * 4)
            + (self.pdef as u64 * 2)
            + (self.mdef as u64 * 2)
            + (self.speed as u64 * 3)
    }

    /// Builds exact `HeroInfoDto` expected by `HeroInfo:init` (`scripts/game/struct/hero/HeroInfo.lua:108-151`):
    /// - `hero_star_level = 1` (MUST be `1`, NOT `0`, because in Lua `0 or 1` evaluates to `0` -> `star_level = -1`!)
    /// - `is_lock = 0` (`data["is_lock"]`)
    /// - complete `va_hero` & `va_*` subtables so wash/star/gold/inlay readers never hit `nil`.
    pub fn to_hero_dto(&self) -> DtoValue {
        let va_hero = DtoValue::map([
            ("skillBook", DtoValue::Map(BTreeMap::new())),
            ("lock", DtoValue::from(0u32)),
            ("wash", DtoValue::Map(BTreeMap::new())),
            ("star", DtoValue::Map(BTreeMap::new())),
        ]);
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
            ("hero_star_level", DtoValue::from(1u32)),
            ("gold_rank", DtoValue::from(0u32)),
            ("gold_level", DtoValue::from(0u32)),
            ("gold_status", DtoValue::from(0u32)),
            ("dark_gold_rank", DtoValue::from(0u32)),
            ("dark_gold_level", DtoValue::from(0u32)),
            ("purple_gold_rank", DtoValue::from(0u32)),
            ("purple_gold_level", DtoValue::from(0u32)),
            ("color_gold_rank", DtoValue::from(0u32)),
            ("color_gold_level", DtoValue::from(0u32)),
            ("nirvana_gold_rank", DtoValue::from(0u32)),
            ("nirvana_gold_level", DtoValue::from(0u32)),
            ("gild_gold_rank", DtoValue::from(0u32)),
            ("gild_gold_level", DtoValue::from(0u32)),
            ("iron_gold_rank", DtoValue::from(0u32)),
            ("iron_gold_level", DtoValue::from(0u32)),
            ("hero_pill_rank", DtoValue::from(0u32)),
            ("hero_pill_level", DtoValue::from(0u32)),
            ("is_lock", DtoValue::from(0u32)),
            ("va_hero", va_hero),
            ("va_fail_reward", DtoValue::Map(BTreeMap::new())),
            ("va_use_wash", DtoValue::Map(BTreeMap::new())),
            ("va_last_wash", DtoValue::Map(BTreeMap::new())),
            ("va_wash_count", DtoValue::Map(BTreeMap::new())),
            ("va_inlay", DtoValue::Map(BTreeMap::new())),
            ("va_color_gold_gift", DtoValue::Map(BTreeMap::new())),
            ("va_nirvana_gold_gift", DtoValue::Map(BTreeMap::new())),
            ("va_gild_gold_gift", DtoValue::Map(BTreeMap::new())),
            ("va_iron_gold_gift", DtoValue::Map(BTreeMap::new())),
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

    /// Recruits a hero into `HerosData` bench ONLY (`re.hero.new` push) without mutating `formation`
    /// (`B-18`: Tavern Raffle & Shop hero summons never auto-deploy into active squad slots!).
    pub fn recruit_hero_to_bench(
        &mut self,
        htid: u32,
        level: u32,
        now_ts: u64,
    ) -> (HeroInstance, PushPacket) {
        let hid = self.next_hid;
        self.next_hid += 1;
        let hero = HeroInstance::from_template(hid, htid, level);
        self.heroes.insert(hid, hero.clone());
        let mut push_map = BTreeMap::new();
        push_map.insert(hid.to_string(), hero.to_hero_dto());
        let pkt = PushPacket::new(push::HERO_NEW, DtoValue::Map(push_map), now_ts);
        (hero, pkt)
    }

    /// Recruits a hero and places into an open formation slot if available (used by GM `/hero` & campaign helpers).
    pub fn recruit_hero(&mut self, htid: u32, level: u32, now_ts: u64) -> (HeroInstance, PushPacket) {
        let (hero, pkt) = self.recruit_hero_to_bench(htid, level, now_ts);
        if !self.formation.contains(&hero.hid) {
            if let Some(empty_slot) = self.formation.iter_mut().skip(1).find(|&&mut h| h == 0) {
                *empty_slot = hero.hid;
            }
        }
        (hero, pkt)
    }

    /// Sacrifices `count` unequipped bench heroes with `htid` (used by `ExchangeShop (27)` `need_item = [[5, htid, qty]]`),
    /// returning their deleted `hid`s for `ret.arr_del_hid`.
    pub fn remove_bench_heroes_by_htid(
        &mut self,
        htid: u32,
        count: u32,
    ) -> Result<Vec<u64>, &'static str> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let candidates: Vec<u64> = self
            .heroes
            .iter()
            .filter(|(&hid, h)| h.htid == htid && !self.formation.contains(&hid))
            .take(count as usize)
            .map(|(&hid, _)| hid)
            .collect();
        if candidates.len() < count as usize {
            return Err("not_enough_sacrifice_heroes");
        }
        for &hid in &candidates {
            self.heroes.remove(&hid);
        }
        Ok(candidates)
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

/// Complete player session state (`UserData.lua`, `BagData.lua`, `HerosData.lua`),
/// including all `PriceTypeConst` shop currencies verified in `priceicon.db` and `UserData.lua`.
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
    /// `PriceTypeConst 10 = TOWER_COIN` (`UserData.tower_coin`)
    pub tower_num: u64,
    /// `PriceTypeConst 16 = TRADE_COIN` (`UserData.business_coin`)
    pub business_coin: u64,
    /// `PriceTypeConst 17 = GUILD_CONTRIBUTION` (`LegionData.guild_point`)
    pub clan_contri: u64,
    /// `PriceTypeConst 19 = WORLDBOSS` (`UserData.boss_coin`)
    pub boss_coin: u64,
    /// `PriceTypeConst 20 = AWAKEARM_COIN` (`UserData.awakeArm_coin` / `jewel_num`)
    pub awake_coin: u64,
    /// `PriceTypeConst 23 = REBEL_COIN` (`UserData.rebel_coin`)
    pub rebel_coin: u64,
    /// `PriceTypeConst 28 = PET_COIN` (`UserData.pet_coin`)
    pub pet_coin: u64,
    /// `PriceTypeConst 29 = TEAM_COIN` (`UserData.world_team_coin`)
    pub world_coin: u64,
    /// `PriceTypeConst 31 = CROSS_COIN` (`UserData.world_person_score`)
    pub world_person_score: u64,
    /// `PriceTypeConst 32 = LORDWAR_COIN` (`UserData.lordwar_coin`)
    pub lordwar_coin: u64,
    /// `PriceTypeConst 33 = GODARM_COIN` (`UserData.godarm_coin`)
    pub godarm_coin: u64,
    /// `PriceTypeConst 34 = GODANIMAL_COIN` (`UserData.godanimal_coin`)
    pub god_animal_coin: u64,
    /// `PriceTypeConst 65` (`UserData.god_animal_explore_coin`)
    pub god_animal_explore_coin: u64,
    /// `PriceTypeConst 37 = STAR_SOUL_COIN` (`UserData.star_soul`)
    pub star_soul: u64,
    /// `PriceTypeConst 39 = HERO_LEGEND_COIN` (`UserData.hero_legend_coin`)
    pub hero_legend_coin: u64,
    /// `HeroLegendRaffleData` currency (`UserData.legend_book_coin`)
    pub legend_book_coin: u64,
    /// `PriceTypeConst 46 = BROCHURE_COIN` (`UserData.brochure_coin`)
    pub brochure_coin: u64,
    /// `PriceTypeConst 50 = HUNT_DEER_COIN` (`UserData.huntdeer_coin`)
    pub huntdeer_coin: u64,
    /// `PriceTypeConst 53 = BINGFU_COIN` (`UserData.bingfu_coin`)
    pub bingfu_coin: u64,
    /// `PriceTypeConst 57 = DARK_GOLD_COIN` (`UserData.darkgold_coin`)
    pub darkgold_coin: u64,
    /// `PriceTypeConst 61 = CLASSICWAR_COIN` (`UserData.classic_war_coin`)
    pub classic_war_coin: u64,
    /// `PriceTypeConst 63 = BINGSHU_YANBINGYU` (`UserData.bingshu_coin`)
    pub bingshu_coin: u64,
    /// `PriceTypeConst 64 = DEVIL_COIN` (`UserData.devil_coin`)
    pub devil_coin: u64,
    /// `PriceTypeConst 66 = NATIONALWAR_COIN` (`UserData.country_war_coin`)
    pub country_war_coin: u64,
    /// `PriceTypeConst 67..71 = SKINCOIN_*` (`GoldShop` Fashion tab `27..31`)
    pub skincoin_purple: u64,
    pub skincoin_orange: u64,
    pub skincoin_red: u64,
    pub skincoin_gold: u64,
    pub skincoin_darkgold: u64,
    /// `PriceTypeConst 72 = UNIFY_WORLD_COIN` (`UserData.unify_coin`)
    pub unify_coin: u64,
    /// `PriceTypeConst 77 = FIRST_RIDE_COIN` (`UserData.first_ride_coin`)
    pub first_ride_coin: u64,
    /// `PriceTypeConst 78 = SOARING_COIN` (`UserData.soaring_coin`)
    pub soaring_coin: u64,
    /// `PriceTypeConst 81 = CROSS_GUILD_COIN` (`UserData.cross_guild_coin`)
    pub cross_guild_coin: u64,
    /// `PriceTypeConst 82 = QUSHUIYAN_SCORE` (`UserData.qushuiyan_coin`)
    pub qushuiyan_coin: u64,
    /// `PriceTypeConst 87 = VOUCHER_COIN` (`UserData.voucher_coin`)
    pub voucher_coin: u64,
    /// `PriceTypeConst 99 = TIME_MACHINE_VOUCHER_COIN` (`UserData.time_machine_voucher_coin`)
    pub time_machine_voucher_coin: u64,
    /// `PriceTypeConst 100 = BABEL_TOWER_ITEM`
    pub babel_tower_coin: u64,
    /// `PriceTypeConst 106 = IRON_GOLD_COIN` (`UserData.iron_gold_coin`)
    pub iron_gold_coin: u64,
    /// `PriceTypeConst 201 = TIME_MACHINE_TREASURE_COIN` (`UserData_TM.tm_treasure_shop_coin`)
    pub tm_treasure_shop_coin: u64,
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
            tower_num: 0,
            business_coin: 0,
            clan_contri: 0,
            boss_coin: 0,
            awake_coin: 0,
            rebel_coin: 0,
            pet_coin: 0,
            world_coin: 0,
            world_person_score: 0,
            lordwar_coin: 0,
            godarm_coin: 0,
            god_animal_coin: 0,
            god_animal_explore_coin: 0,
            star_soul: 0,
            hero_legend_coin: 0,
            legend_book_coin: 0,
            brochure_coin: 0,
            huntdeer_coin: 0,
            bingfu_coin: 0,
            darkgold_coin: 0,
            classic_war_coin: 0,
            bingshu_coin: 0,
            devil_coin: 0,
            country_war_coin: 0,
            skincoin_purple: 0,
            skincoin_orange: 0,
            skincoin_red: 0,
            skincoin_gold: 0,
            skincoin_darkgold: 0,
            unify_coin: 0,
            first_ride_coin: 0,
            soaring_coin: 0,
            cross_guild_coin: 0,
            qushuiyan_coin: 0,
            voucher_coin: 0,
            time_machine_voucher_coin: 0,
            babel_tower_coin: 0,
            iron_gold_coin: 0,
            tm_treasure_shop_coin: 0,
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

    /// Reads current balance for any `PriceTypeConst` currency ID (`priceicon.db` / `PriceTypeConst.lua`).
    pub fn currency_by_price_type(&self, price_type: u32) -> u64 {
        match price_type {
            1 => self.copper.max(0) as u64,
            2 => self.gold.max(0) as u64,
            3 => self.gold_bind.max(0) as u64,
            7 => self.prestige,
            9 => self.arena_coin,
            10 => self.tower_num,
            14 => u64::from(self.execution),
            15 => self.hero_soul,
            16 => self.business_coin,
            17 | 38 => self.clan_contri,
            18 => u64::from(self.energy),
            19 => self.boss_coin,
            20 => self.awake_coin,
            23 => self.rebel_coin,
            27 => self.fame,
            28 => self.pet_coin,
            29 => self.world_coin,
            31 => self.world_person_score,
            32 => self.lordwar_coin,
            33 => self.godarm_coin,
            34 => self.god_animal_coin,
            37 => self.star_soul,
            39 => self.hero_legend_coin,
            46 => self.brochure_coin,
            50 => self.huntdeer_coin,
            53 => self.bingfu_coin,
            57 => self.darkgold_coin,
            61 => self.classic_war_coin,
            63 => self.bingshu_coin,
            64 => self.devil_coin,
            65 => self.god_animal_explore_coin,
            66 => self.country_war_coin,
            67 => self.skincoin_purple,
            68 => self.skincoin_orange,
            69 => self.skincoin_red,
            70 => self.skincoin_gold,
            71 => self.skincoin_darkgold,
            72 => self.unify_coin,
            77 => self.first_ride_coin,
            78 => self.soaring_coin,
            81 => self.cross_guild_coin,
            82 => self.qushuiyan_coin,
            87 => self.voucher_coin,
            99 => self.time_machine_voucher_coin,
            100 => self.babel_tower_coin,
            106 => self.iron_gold_coin,
            201 => self.tm_treasure_shop_coin,
            _ => 0,
        }
    }

    /// Deducts `amount` of `price_type`, returning the updated `(wire_key, DtoValue)` pair for `user.updateUser`.
    pub fn spend_currency_by_price_type(
        &mut self,
        price_type: u32,
        amount: u64,
    ) -> Result<Option<(&'static str, DtoValue)>, &'static str> {
        if amount == 0 {
            return Ok(None);
        }
        if self.currency_by_price_type(price_type) < amount {
            return Err(match price_type {
                1 => "not_enough_copper",
                2 => "not_enough_gold",
                9 => "not_enough_arena_coin",
                10 => "not_enough_tower_coin",
                15 => "not_enough_hero_soul",
                17 => "not_enough_clan_contri",
                20 => "not_enough_awake_coin",
                23 => "not_enough_rebel_coin",
                27 => "not_enough_fame",
                28 => "not_enough_pet_coin",
                29 => "not_enough_world_coin",
                33 => "not_enough_godarm_coin",
                34 => "not_enough_god_animal_coin",
                39 => "not_enough_hero_legend_coin",
                _ => "not_enough_currency",
            });
        }
        let field = match price_type {
            1 => {
                self.copper -= amount as i64;
                ("copper", DtoValue::from(self.copper))
            }
            2 => {
                self.gold -= amount as i64;
                ("gold", DtoValue::from(self.gold))
            }
            3 => {
                self.gold_bind -= amount as i64;
                ("gold_bind", DtoValue::from(self.gold_bind))
            }
            7 => {
                self.prestige -= amount;
                ("prestige", DtoValue::from(self.prestige))
            }
            9 => {
                self.arena_coin -= amount;
                ("arena_coin", DtoValue::from(self.arena_coin))
            }
            10 => {
                self.tower_num -= amount;
                ("tower_coin", DtoValue::from(self.tower_num))
            }
            14 => {
                self.execution = self.execution.saturating_sub(amount as u32);
                ("execution", DtoValue::from(self.execution))
            }
            15 => {
                self.hero_soul -= amount;
                ("hero_soul", DtoValue::from(self.hero_soul))
            }
            16 => {
                self.business_coin -= amount;
                ("business_coin", DtoValue::from(self.business_coin))
            }
            17 | 38 => {
                self.clan_contri -= amount;
                ("guild_point", DtoValue::from(self.clan_contri))
            }
            18 => {
                self.energy = self.energy.saturating_sub(amount as u32);
                ("energy", DtoValue::from(self.energy))
            }
            19 => {
                self.boss_coin -= amount;
                ("boss_coin", DtoValue::from(self.boss_coin))
            }
            20 => {
                self.awake_coin -= amount;
                ("awakeArm_coin", DtoValue::from(self.awake_coin))
            }
            23 => {
                self.rebel_coin -= amount;
                ("rebel_coin", DtoValue::from(self.rebel_coin))
            }
            27 => {
                self.fame -= amount;
                ("fame", DtoValue::from(self.fame))
            }
            28 => {
                self.pet_coin -= amount;
                ("pet_coin", DtoValue::from(self.pet_coin))
            }
            29 => {
                self.world_coin -= amount;
                ("world_team_coin", DtoValue::from(self.world_coin))
            }
            31 => {
                self.world_person_score -= amount;
                ("world_person_score", DtoValue::from(self.world_person_score))
            }
            32 => {
                self.lordwar_coin -= amount;
                ("lordwar_coin", DtoValue::from(self.lordwar_coin))
            }
            33 => {
                self.godarm_coin -= amount;
                ("godarm_coin", DtoValue::from(self.godarm_coin))
            }
            34 => {
                self.god_animal_coin -= amount;
                ("godanimal_coin", DtoValue::from(self.god_animal_coin))
            }
            37 => {
                self.star_soul -= amount;
                ("star_soul", DtoValue::from(self.star_soul))
            }
            39 => {
                self.hero_legend_coin -= amount;
                ("hero_legend_coin", DtoValue::from(self.hero_legend_coin))
            }
            46 => {
                self.brochure_coin -= amount;
                ("brochure_coin", DtoValue::from(self.brochure_coin))
            }
            50 => {
                self.huntdeer_coin -= amount;
                ("huntdeer_coin", DtoValue::from(self.huntdeer_coin))
            }
            53 => {
                self.bingfu_coin -= amount;
                ("bingfu_coin", DtoValue::from(self.bingfu_coin))
            }
            57 => {
                self.darkgold_coin -= amount;
                ("darkgold_coin", DtoValue::from(self.darkgold_coin))
            }
            61 => {
                self.classic_war_coin -= amount;
                ("classic_war_coin", DtoValue::from(self.classic_war_coin))
            }
            63 => {
                self.bingshu_coin -= amount;
                ("bingshu_coin", DtoValue::from(self.bingshu_coin))
            }
            64 => {
                self.devil_coin -= amount;
                ("devil_coin", DtoValue::from(self.devil_coin))
            }
            65 => {
                self.god_animal_explore_coin -= amount;
                (
                    "god_animal_explore_coin",
                    DtoValue::from(self.god_animal_explore_coin),
                )
            }
            66 => {
                self.country_war_coin -= amount;
                ("country_war_coin", DtoValue::from(self.country_war_coin))
            }
            67 => {
                self.skincoin_purple -= amount;
                ("skincoin_purple", DtoValue::from(self.skincoin_purple))
            }
            68 => {
                self.skincoin_orange -= amount;
                ("skincoin_orange", DtoValue::from(self.skincoin_orange))
            }
            69 => {
                self.skincoin_red -= amount;
                ("skincoin_red", DtoValue::from(self.skincoin_red))
            }
            70 => {
                self.skincoin_gold -= amount;
                ("skincoin_gold", DtoValue::from(self.skincoin_gold))
            }
            71 => {
                self.skincoin_darkgold -= amount;
                ("skincoin_darkgold", DtoValue::from(self.skincoin_darkgold))
            }
            72 => {
                self.unify_coin -= amount;
                ("unify_coin", DtoValue::from(self.unify_coin))
            }
            77 => {
                self.first_ride_coin -= amount;
                ("first_ride_coin", DtoValue::from(self.first_ride_coin))
            }
            78 => {
                self.soaring_coin -= amount;
                ("soaring_coin", DtoValue::from(self.soaring_coin))
            }
            81 => {
                self.cross_guild_coin -= amount;
                ("cross_guild_coin", DtoValue::from(self.cross_guild_coin))
            }
            82 => {
                self.qushuiyan_coin -= amount;
                ("qushuiyan_coin", DtoValue::from(self.qushuiyan_coin))
            }
            87 => {
                self.voucher_coin -= amount;
                ("voucher_coin", DtoValue::from(self.voucher_coin))
            }
            99 => {
                self.time_machine_voucher_coin -= amount;
                (
                    "time_machine_voucher_coin",
                    DtoValue::from(self.time_machine_voucher_coin),
                )
            }
            100 => {
                self.babel_tower_coin -= amount;
                ("babel_tower_coin", DtoValue::from(self.babel_tower_coin))
            }
            106 => {
                self.iron_gold_coin -= amount;
                ("iron_gold_coin", DtoValue::from(self.iron_gold_coin))
            }
            201 => {
                self.tm_treasure_shop_coin -= amount;
                (
                    "tm_treasure_shop_coin",
                    DtoValue::from(self.tm_treasure_shop_coin),
                )
            }
            _ => return Err("unsupported_currency"),
        };
        Ok(Some(field))
    }

    /// Grants `amount` of `price_type` (used when shop offers grant currency like `1=Copper`, `15=HeroSoul`, `37=StarSoul`).
    pub fn add_currency_by_price_type(
        &mut self,
        price_type: u32,
        amount: u64,
    ) -> Option<(&'static str, DtoValue)> {
        if amount == 0 {
            return None;
        }
        Some(match price_type {
            1 => {
                self.copper = self
                    .copper
                    .saturating_add(amount as i64)
                    .min(Self::COPPER_CAP);
                ("copper", DtoValue::from(self.copper))
            }
            2 => {
                self.gold = self.gold.saturating_add(amount as i64);
                ("gold", DtoValue::from(self.gold))
            }
            3 => {
                self.gold_bind = self.gold_bind.saturating_add(amount as i64);
                ("gold_bind", DtoValue::from(self.gold_bind))
            }
            7 => {
                self.prestige = self.prestige.saturating_add(amount);
                ("prestige", DtoValue::from(self.prestige))
            }
            9 => {
                self.arena_coin = self.arena_coin.saturating_add(amount);
                ("arena_coin", DtoValue::from(self.arena_coin))
            }
            10 => {
                self.tower_num = self.tower_num.saturating_add(amount);
                ("tower_coin", DtoValue::from(self.tower_num))
            }
            14 => {
                self.execution = self.execution.saturating_add(amount as u32);
                ("execution", DtoValue::from(self.execution))
            }
            15 => {
                self.hero_soul = self.hero_soul.saturating_add(amount);
                ("hero_soul", DtoValue::from(self.hero_soul))
            }
            17 | 38 => {
                self.clan_contri = self.clan_contri.saturating_add(amount);
                ("guild_point", DtoValue::from(self.clan_contri))
            }
            18 => {
                self.energy = self.energy.saturating_add(amount as u32);
                ("energy", DtoValue::from(self.energy))
            }
            20 => {
                self.awake_coin = self.awake_coin.saturating_add(amount);
                ("awakeArm_coin", DtoValue::from(self.awake_coin))
            }
            23 => {
                self.rebel_coin = self.rebel_coin.saturating_add(amount);
                ("rebel_coin", DtoValue::from(self.rebel_coin))
            }
            27 => {
                self.fame = self.fame.saturating_add(amount);
                ("fame", DtoValue::from(self.fame))
            }
            28 => {
                self.pet_coin = self.pet_coin.saturating_add(amount);
                ("pet_coin", DtoValue::from(self.pet_coin))
            }
            29 => {
                self.world_coin = self.world_coin.saturating_add(amount);
                ("world_team_coin", DtoValue::from(self.world_coin))
            }
            33 => {
                self.godarm_coin = self.godarm_coin.saturating_add(amount);
                ("godarm_coin", DtoValue::from(self.godarm_coin))
            }
            34 => {
                self.god_animal_coin = self.god_animal_coin.saturating_add(amount);
                ("godanimal_coin", DtoValue::from(self.god_animal_coin))
            }
            37 => {
                self.star_soul = self.star_soul.saturating_add(amount);
                ("star_soul", DtoValue::from(self.star_soul))
            }
            39 => {
                self.hero_legend_coin = self.hero_legend_coin.saturating_add(amount);
                ("hero_legend_coin", DtoValue::from(self.hero_legend_coin))
            }
            _ => return None,
        })
    }

    /// Server-side helper computing unlocked formation positions (`1..=6`) from player `level`.
    /// Note: never emitted over the wire in `user.getUser` or `user.updateUser`, because
    /// `UserData:initConfig()` populates `self.hero_position_map` with integer keys `1..6`.
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

    /// Builds a full currency synchronization `user.updateUser` push reflecting all `UserData` balances.
    pub fn build_full_currency_update_push(&self, now_ts: u64) -> PushPacket {
        Self::build_user_update_push(
            [
                ("gold", DtoValue::from(self.gold)),
                ("gold_bind", DtoValue::from(self.gold_bind)),
                ("copper", DtoValue::from(self.copper)),
                ("execution", DtoValue::from(self.execution)),
                ("energy", DtoValue::from(self.energy)),
                ("prestige", DtoValue::from(self.prestige)),
                ("arena_coin", DtoValue::from(self.arena_coin)),
                ("hero_soul", DtoValue::from(self.hero_soul)),
                ("fame", DtoValue::from(self.fame)),
                ("tower_coin", DtoValue::from(self.tower_num)),
                ("business_coin", DtoValue::from(self.business_coin)),
                ("guild_point", DtoValue::from(self.clan_contri)),
                ("boss_coin", DtoValue::from(self.boss_coin)),
                ("awakeArm_coin", DtoValue::from(self.awake_coin)),
                ("jewel_num", DtoValue::from(self.awake_coin)),
                ("rebel_coin", DtoValue::from(self.rebel_coin)),
                ("pet_coin", DtoValue::from(self.pet_coin)),
                ("world_team_coin", DtoValue::from(self.world_coin)),
                ("world_person_score", DtoValue::from(self.world_person_score)),
                ("lordwar_coin", DtoValue::from(self.lordwar_coin)),
                ("godarm_coin", DtoValue::from(self.godarm_coin)),
                ("godanimal_coin", DtoValue::from(self.god_animal_coin)),
                (
                    "god_animal_explore_coin",
                    DtoValue::from(self.god_animal_explore_coin),
                ),
                ("star_soul", DtoValue::from(self.star_soul)),
                ("hero_legend_coin", DtoValue::from(self.hero_legend_coin)),
                ("legend_book_coin", DtoValue::from(self.legend_book_coin)),
                ("brochure_coin", DtoValue::from(self.brochure_coin)),
                ("huntdeer_coin", DtoValue::from(self.huntdeer_coin)),
                ("bingfu_coin", DtoValue::from(self.bingfu_coin)),
                ("darkgold_coin", DtoValue::from(self.darkgold_coin)),
                ("classic_war_coin", DtoValue::from(self.classic_war_coin)),
                ("bingshu_coin", DtoValue::from(self.bingshu_coin)),
                ("devil_coin", DtoValue::from(self.devil_coin)),
                ("country_war_coin", DtoValue::from(self.country_war_coin)),
                ("skincoin_purple", DtoValue::from(self.skincoin_purple)),
                ("skincoin_orange", DtoValue::from(self.skincoin_orange)),
                ("skincoin_red", DtoValue::from(self.skincoin_red)),
                ("skincoin_gold", DtoValue::from(self.skincoin_gold)),
                ("skincoin_darkgold", DtoValue::from(self.skincoin_darkgold)),
                ("unify_coin", DtoValue::from(self.unify_coin)),
                ("first_ride_coin", DtoValue::from(self.first_ride_coin)),
                ("soaring_coin", DtoValue::from(self.soaring_coin)),
                ("cross_guild_coin", DtoValue::from(self.cross_guild_coin)),
                ("qushuiyan_coin", DtoValue::from(self.qushuiyan_coin)),
                ("voucher_coin", DtoValue::from(self.voucher_coin)),
                (
                    "time_machine_voucher_coin",
                    DtoValue::from(self.time_machine_voucher_coin),
                ),
                ("iron_gold_coin", DtoValue::from(self.iron_gold_coin)),
                (
                    "tm_treasure_shop_coin",
                    DtoValue::from(self.tm_treasure_shop_coin),
                ),
                ("fight_force", DtoValue::from(self.fight_force())),
            ],
            now_ts,
        )
    }

    /// Builds full `user.getUser` DTO (`UserData:parseData`), guaranteeing all MainTown and Shop invariants.
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
            ("tower_coin", DtoValue::from(self.tower_num)),
            ("business_coin", DtoValue::from(self.business_coin)),
            ("guild_point", DtoValue::from(self.clan_contri)),
            ("boss_coin", DtoValue::from(self.boss_coin)),
            ("awakeArm_coin", DtoValue::from(self.awake_coin)),
            ("jewel_num", DtoValue::from(self.awake_coin)),
            ("rebel_coin", DtoValue::from(self.rebel_coin)),
            ("pet_coin", DtoValue::from(self.pet_coin)),
            ("world_team_coin", DtoValue::from(self.world_coin)),
            ("world_person_score", DtoValue::from(self.world_person_score)),
            ("lordwar_coin", DtoValue::from(self.lordwar_coin)),
            ("godarm_coin", DtoValue::from(self.godarm_coin)),
            ("godanimal_coin", DtoValue::from(self.god_animal_coin)),
            (
                "god_animal_explore_coin",
                DtoValue::from(self.god_animal_explore_coin),
            ),
            ("star_soul", DtoValue::from(self.star_soul)),
            ("hero_legend_coin", DtoValue::from(self.hero_legend_coin)),
            ("legend_book_coin", DtoValue::from(self.legend_book_coin)),
            ("brochure_coin", DtoValue::from(self.brochure_coin)),
            ("huntdeer_coin", DtoValue::from(self.huntdeer_coin)),
            ("bingfu_coin", DtoValue::from(self.bingfu_coin)),
            ("darkgold_coin", DtoValue::from(self.darkgold_coin)),
            ("classic_war_coin", DtoValue::from(self.classic_war_coin)),
            ("bingshu_coin", DtoValue::from(self.bingshu_coin)),
            ("devil_coin", DtoValue::from(self.devil_coin)),
            ("country_war_coin", DtoValue::from(self.country_war_coin)),
            ("skincoin_purple", DtoValue::from(self.skincoin_purple)),
            ("skincoin_orange", DtoValue::from(self.skincoin_orange)),
            ("skincoin_red", DtoValue::from(self.skincoin_red)),
            ("skincoin_gold", DtoValue::from(self.skincoin_gold)),
            ("skincoin_darkgold", DtoValue::from(self.skincoin_darkgold)),
            ("unify_coin", DtoValue::from(self.unify_coin)),
            ("first_ride_coin", DtoValue::from(self.first_ride_coin)),
            ("soaring_coin", DtoValue::from(self.soaring_coin)),
            ("cross_guild_coin", DtoValue::from(self.cross_guild_coin)),
            ("qushuiyan_coin", DtoValue::from(self.qushuiyan_coin)),
            ("voucher_coin", DtoValue::from(self.voucher_coin)),
            (
                "time_machine_voucher_coin",
                DtoValue::from(self.time_machine_voucher_coin),
            ),
            ("iron_gold_coin", DtoValue::from(self.iron_gold_coin)),
            (
                "tm_treasure_shop_coin",
                DtoValue::from(self.tm_treasure_shop_coin),
            ),
            ("pet_close", DtoValue::from(false)),
            ("godArm_close", DtoValue::from(false)),
            ("yanwu_close", DtoValue::from(false)),
            ("halidom_close", DtoValue::from(false)),
            ("fight_force", DtoValue::from(self.fight_force())),
            ("dress_id", DtoValue::from(self.dress_id)),
            ("title_id", DtoValue::from(self.title_id)),
            ("server_id", DtoValue::from(self.server_id.as_str())),
            ("server_open_time", DtoValue::from(self.server_open_time.as_str())),
            ("dress_info", DtoValue::Map(BTreeMap::new())),
            ("dress_expire", DtoValue::Map(BTreeMap::new())),
            ("va_user", DtoValue::Map(BTreeMap::new())),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bag_category_gids_match_bagdata_lua_get_bag_type_by_gid() {
        assert_eq!(BagCategory::Treasure.gid_range(), (10_001, 20_000));
        assert_eq!(BagCategory::Arm.gid_range(), (20_001, 30_000));
        assert_eq!(BagCategory::Item.gid_range(), (30_001, 40_000));
        assert_eq!(BagCategory::ArmFrag.gid_range(), (40_001, 50_000));
        assert_eq!(BagCategory::HeroFrag.gid_range(), (50_001, 60_000));
        assert_eq!(BagCategory::Awake.gid_range(), (60_001, 70_000));
        assert_eq!(BagCategory::AwakeFrag.gid_range(), (70_001, 80_000));
        assert_eq!(BagCategory::TreasFrag.gid_range(), (80_001, 90_000));
        assert_eq!(BagCategory::Spirit.gid_range(), (90_001, 100_000));
        assert_eq!(BagCategory::Pet.gid_range(), (100_001, 110_000));
        assert_eq!(BagCategory::GodArm.gid_range(), (210_001, 220_000));
        assert_eq!(BagCategory::GodArmFrag.gid_range(), (220_001, 230_000));
        assert_eq!(BagCategory::HeroLegend.gid_range(), (410_001, 420_000));
        assert_eq!(BagCategory::HeroLegendFrag.gid_range(), (420_001, 430_000));
        assert_eq!(BagCategory::SoulStone.gid_range(), (650_001, 660_000));
        assert_eq!(BagCategory::SoulStoneFrag.gid_range(), (660_001, 670_000));
        assert_eq!(BagCategory::TopAnimal.gid_range(), (710_001, 720_000));
        assert_eq!(BagCategory::TopAnimalFrag.gid_range(), (720_001, 730_000));

        // Verify template ID routing across key Gacha & Shop items
        assert_eq!(BagCategory::from_template_id(16_013), BagCategory::Item);
        assert_eq!(BagCategory::from_template_id(29_002), BagCategory::Item);
        assert_eq!(BagCategory::from_template_id(50_101), BagCategory::HeroFrag);
        assert_eq!(BagCategory::from_template_id(50_116), BagCategory::HeroFrag);
        assert_eq!(BagCategory::from_template_id(101_001), BagCategory::Arm);
        assert_eq!(BagCategory::from_template_id(181_001), BagCategory::Treasure);
        assert_eq!(BagCategory::from_template_id(400_000_001), BagCategory::TreasFrag);
        assert_eq!(BagCategory::from_template_id(500_401), BagCategory::GodArm);
        assert_eq!(BagCategory::from_template_id(3_500_401), BagCategory::GodArmFrag);
        assert_eq!(BagCategory::from_template_id(702_001), BagCategory::HeroLegend);
        assert_eq!(BagCategory::from_template_id(4_600_101), BagCategory::SoulStone);
        assert_eq!(BagCategory::from_template_id(4_700_110), BagCategory::SoulStoneFrag);
        assert_eq!(BagCategory::from_template_id(5_400_001), BagCategory::TopAnimal);
        assert_eq!(BagCategory::from_template_id(5_500_001), BagCategory::TopAnimalFrag);
    }

    #[test]
    fn bag_update_push_emits_authentic_gid_and_empty_map_on_zero_stack() {
        let mut bag = BagState::new();
        let d1 = bag.add_item(16_013, 300, 1_700_000_000).unwrap();
        assert_eq!(
            BagCategory::from_gid(30_001),
            Some(BagCategory::Item),
            "16013 must allocate inside 30001..40000"
        );
        match &d1[0] {
            BagDelta::Upsert(it) => assert_eq!(it.gid, 30_001),
            _ => panic!("expected upsert"),
        }

        let d_frag = bag.add_item(50_101, 20, 1_700_000_000).unwrap();
        match &d_frag[0] {
            BagDelta::Upsert(it) => assert_eq!(it.gid, 50_001),
            _ => panic!("expected soul frag at 50001"),
        }

        let d_rem = bag.consume_item(16_013, 300).unwrap();
        let pkt = BagState::build_update_push(&d_rem, 1_700_000_001).unwrap();
        let slot_dto = pkt.ret.as_array().unwrap()[0]
            .get("30001")
            .expect("must emit string gid '30001'")
            .as_map()
            .expect("empty map deletes stack in BagData:updateItemTb");
        assert!(slot_dto.is_empty());
    }

    #[test]
    fn hero_dto_and_bench_recruitment_preserve_maintown_and_star_invariants() {
        let mut p = PlayerState::new(1001, "MainHero", 1);
        let (_, hero_push) = p.roster.recruit_hero_to_bench(102, 1, 1_700_000_000);
        // Bench recruitment MUST NOT silently auto-equip into formation[1..5]
        assert_eq!(p.roster.formation[1..], [0, 0, 0, 0, 0]);
        let dto = hero_push.ret.get("10000002").expect("new hero dto");
        assert_eq!(dto.get("hero_star_level").and_then(DtoValue::as_u32), Some(1));
        assert_eq!(dto.get("is_lock").and_then(DtoValue::as_u32), Some(0));
        assert!(dto.get("va_hero").and_then(DtoValue::as_map).is_some());

        // Currency round-trip via PriceTypeConst
        p.tower_num = 5_000;
        p.awake_coin = 2_000;
        p.godarm_coin = 800;
        let (_, val) = p
            .spend_currency_by_price_type(10, 1_200)
            .unwrap()
            .unwrap();
        assert_eq!(val.as_u64(), Some(3_800));
        let user_dto = p.to_user_dto();
        assert_eq!(user_dto.get("tower_coin").and_then(DtoValue::as_u64), Some(3_800));
        assert_eq!(user_dto.get("awakeArm_coin").and_then(DtoValue::as_u64), Some(2_000));
        assert_eq!(user_dto.get("godarm_coin").and_then(DtoValue::as_u64), Some(800));
    }
}
