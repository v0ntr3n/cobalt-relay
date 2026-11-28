use std::collections::BTreeMap;

use crate::error::{PlayerError, Result};
use crate::json::{JsonObjReader, JsonValue};
use crate::wire_helpers::{decode_u32_map, encode_map};

define_wire_enum! {
    /// All 42 sub-bag tables defined in GServer (`vnsg2`) `BagConsts.lua` & `BagData.lua`.
    pub enum BagType {
        Treasure => "treasure",
        Arm => "arm",
        Item => "item",
        Fragment => "fragment",
        Soul => "soul",
        Awake => "awake",
        AwakeFragment => "awake_fragment",
        TreasureFragment => "treasure_fragment",
        Spirit => "spirit",
        Pet => "pet",
        PetFragment => "pet_fragment",
        GodArm => "god_arm",
        GodArmFragment => "god_arm_fragment",
        HalidomFragment => "halidom_fragment",
        Gem => "gem",
        GodAnimal => "god_animal",
        GodAnimalFragment => "god_animal_fragment",
        HeroLegend => "hero_legend",
        HeroLegendFragment => "hero_legend_fragment",
        Brochure => "brochure",
        BrochureFragment => "brochure_fragment",
        BingFuFragment => "bingfu_fragment",
        SoulSkill => "soul_skill",
        StatueFragment => "statue_fragment",
        BingShuFragment => "bingshu_fragment",
        JobSkillFragment => "jobskill_fragment",
        YanWu => "yanwu",
        YanWuFragment => "yanwu_fragment",
        YanWuSoul => "yanwu_soul",
        Counsellor => "counsellor",
        CounsellorFragment => "counsellor_fragment",
        CounsellorEquip => "counsellor_equip",
        CounsellorEquipFragment => "counsellor_equip_fragment",
        SoulStone => "soul_stone",
        SoulStoneFragment => "soul_stone_fragment",
        SoulJade => "soul_jade",
        SoulJadeFragment => "soul_jade_fragment",
        WareHouse => "ware_house",
        PloyFragment => "ploy_fragment",
        TopAnimal => "top_animal",
        TopAnimalFragment => "top_animal_fragment",
        GreatSealFragment => "great_seal_fragment",
    }
}

const BAG_GRID_STARTS: [u32; 42] = [
    10_001, 20_001, 30_001, 40_001, 50_001, 60_001, 70_001, 80_001, 90_001, 100_001, 200_001,
    210_001, 220_001, 230_001, 240_001, 310_001, 320_001, 410_001, 420_001, 510_001, 520_001,
    530_001, 540_001, 550_001, 560_001, 570_001, 580_001, 590_001, 600_001, 610_001, 620_001,
    630_001, 640_001, 650_001, 660_001, 670_001, 680_001, 690_001, 700_001, 710_001, 720_001,
    730_001,
];

impl BagType {
    /// Inclusive `(grid_start, grid_end)` range enforced by `BagData:getBagTypeByGid`.
    pub const fn gid_range(self) -> (u32, u32) {
        let start = BAG_GRID_STARTS[self as usize];
        (start, start + 9_999)
    }

    pub const fn grid_start(self) -> u32 {
        BAG_GRID_STARTS[self as usize]
    }

    /// Default `gridMaxNum` extracted from `bag.db` + `BagData.lua`.
    pub const fn default_max_grids(self) -> u32 {
        match self {
            BagType::Arm
            | BagType::Treasure
            | BagType::Spirit
            | BagType::Pet
            | BagType::GodArm
            | BagType::Gem
            | BagType::YanWu
            | BagType::TopAnimal => 200,
            BagType::Awake | BagType::GodAnimal => 100,
            _ => 10_000,
        }
    }

    /// Returns `true` when identical `item_template_id` items stack in one grid.
    pub const fn is_stackable(self) -> bool {
        !matches!(
            self,
            BagType::Arm
                | BagType::Treasure
                | BagType::Awake
                | BagType::Spirit
                | BagType::Pet
                | BagType::GodArm
                | BagType::GodAnimal
                | BagType::YanWu
                | BagType::Counsellor
                | BagType::CounsellorEquip
                | BagType::SoulStone
                | BagType::SoulJade
                | BagType::TopAnimal
        )
    }

    /// Resolve a `gid` back to its owning sub-bag (`BagData:getBagTypeByGid`).
    pub fn from_gid(gid: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|bag| {
            let (lo, hi) = bag.gid_range();
            gid >= lo && gid <= hi
        })
    }
}

/// Item class routed by `ItemFactory:getItemTypeByTid` in `ItemFactory.lua`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemClass {
    Normal,
    Direct,
    RandGift,
    HeroFragment,
    Arm,
    Treasure,
    Spirit,
    Awake,
    Pet,
    GodArm,
    GodAnimal,
    HeroLegend,
    Brochure,
    HalidomFragment,
    Gem,
    GodArmGem,
    Pill,
    BingFuFragment,
    SoulSkill,
    Virtual,
    StatueFragment,
    BingShuFragment,
    JobSkill,
    JobSkillFragment,
    ArmFragment,
    AwakeFragment,
    PetFragment,
    GodArmFragment,
    GodAnimalFragment,
    HeroLegendFragment,
    BrochureFragment,
    YanWu,
    YanWuFragment,
    YanWuSoul,
    CounsellorFragment,
    Counsellor,
    CounsellorEquip,
    CounsellorEquipFragment,
    SoulStone,
    SoulStoneFragment,
    SoulJade,
    SoulJadeFragment,
    PloyFragment,
    TopAnimal,
    TopAnimalFragment,
    GreatSealFragment,
    TreasureFragment,
}

/// Classify `item_template_id` using the exact ranges in `ItemFactory.lua` (`getItemTypeByTid`).
pub fn classify_template_id(tid: u32) -> Result<(ItemClass, BagType)> {
    let pair = match tid {
        10_001..=20_000 => (ItemClass::Normal, BagType::Item),
        20_001..=30_000 => (ItemClass::Direct, BagType::Item),
        30_001..=40_000 => (ItemClass::RandGift, BagType::Item),
        50_001..=60_000 => (ItemClass::HeroFragment, BagType::Soul),
        100_001..=180_000 => (ItemClass::Arm, BagType::Arm),
        180_001..=200_000 => (ItemClass::Treasure, BagType::Treasure),
        200_001..=300_000 => (ItemClass::Spirit, BagType::Spirit),
        300_001..=400_000 => (ItemClass::Awake, BagType::Awake),
        400_001..=500_000 | 5_000_001..=5_200_000 => (ItemClass::Pet, BagType::Pet),
        500_001..=600_000 => (ItemClass::GodArm, BagType::GodArm),
        600_001..=700_000 => (ItemClass::GodAnimal, BagType::GodAnimal),
        700_001..=800_000 => (ItemClass::HeroLegend, BagType::HeroLegend),
        800_001..=900_000 => (ItemClass::Brochure, BagType::Brochure),
        900_001..=1_000_000 => (ItemClass::HalidomFragment, BagType::HalidomFragment),
        1_000_001..=1_100_000 => (ItemClass::Gem, BagType::Gem),
        1_100_001..=1_200_000 => (ItemClass::GodArmGem, BagType::Item),
        1_200_001..=1_300_000 => (ItemClass::Pill, BagType::Item),
        1_300_001..=1_400_000 => (ItemClass::BingFuFragment, BagType::BingFuFragment),
        1_400_001..=1_500_000 => (ItemClass::SoulSkill, BagType::SoulSkill),
        1_500_001..=1_600_000 => (ItemClass::Virtual, BagType::Item),
        1_600_001..=1_700_000 => (ItemClass::StatueFragment, BagType::StatueFragment),
        1_700_001..=1_800_000 => (ItemClass::BingShuFragment, BagType::BingShuFragment),
        1_800_001..=1_850_000 => (ItemClass::JobSkill, BagType::Item),
        1_850_001..=1_900_000 => (ItemClass::JobSkillFragment, BagType::JobSkillFragment),
        3_100_001..=3_200_000 => (ItemClass::ArmFragment, BagType::Fragment),
        3_300_001..=3_400_000 => (ItemClass::AwakeFragment, BagType::AwakeFragment),
        3_400_001..=3_500_000 => (ItemClass::PetFragment, BagType::PetFragment),
        3_500_001..=3_600_000 => (ItemClass::GodArmFragment, BagType::GodArmFragment),
        3_600_001..=3_700_000 => (ItemClass::GodAnimalFragment, BagType::GodAnimalFragment),
        3_700_001..=3_800_000 => (ItemClass::HeroLegendFragment, BagType::HeroLegendFragment),
        3_800_001..=3_900_000 => (ItemClass::BrochureFragment, BagType::BrochureFragment),
        3_900_001..=4_000_000 => (ItemClass::YanWu, BagType::YanWu),
        4_000_001..=4_100_000 => (ItemClass::YanWuFragment, BagType::YanWuFragment),
        4_100_001..=4_200_000 => (ItemClass::YanWuSoul, BagType::YanWuSoul),
        4_200_001..=4_300_000 => (ItemClass::CounsellorFragment, BagType::CounsellorFragment),
        4_300_001..=4_400_000 => (ItemClass::Counsellor, BagType::Counsellor),
        4_400_001..=4_500_000 => (ItemClass::CounsellorEquip, BagType::CounsellorEquip),
        4_500_001..=4_600_000 => (
            ItemClass::CounsellorEquipFragment,
            BagType::CounsellorEquipFragment,
        ),
        4_600_001..=4_700_000 => (ItemClass::SoulStone, BagType::SoulStone),
        4_700_001..=4_800_000 => (ItemClass::SoulStoneFragment, BagType::SoulStoneFragment),
        4_800_001..=4_900_000 => (ItemClass::SoulJade, BagType::SoulJade),
        4_900_001..=5_000_000 => (ItemClass::SoulJadeFragment, BagType::SoulJadeFragment),
        5_300_001..=5_400_000 => (ItemClass::PloyFragment, BagType::PloyFragment),
        5_400_001..=5_500_000 => (ItemClass::TopAnimal, BagType::TopAnimal),
        5_500_001..=5_600_000 => (ItemClass::TopAnimalFragment, BagType::TopAnimalFragment),
        5_600_001..=5_700_000 => (ItemClass::GreatSealFragment, BagType::GreatSealFragment),
        400_000_001..=500_000_000 => (ItemClass::TreasureFragment, BagType::TreasureFragment),
        _ => {
            return Err(PlayerError::InvalidParameter(format!(
                "item_template_id {tid} is outside all ItemFactory.lua ranges"
            )))
        }
    };
    Ok(pair)
}

/// Build non-nil `va_item_text` defaults required by `ArmItem.lua`, `TreasureItem.lua`,
/// `AwakeItem.lua`, `PetItem.lua`, and `GodItem.lua`.
pub fn default_va_item_text(class: ItemClass) -> BTreeMap<String, JsonValue> {
    let mut m = BTreeMap::new();
    let put_u32s = |map: &mut BTreeMap<String, JsonValue>, pairs: &[(&str, u32)]| {
        for &(k, v) in pairs {
            map.insert(k.into(), JsonValue::from(v));
        }
    };
    match class {
        ItemClass::Arm => {
            put_u32s(
                &mut m,
                &[
                    ("reinforce_level", 1),
                    ("reinforce_exp", 0),
                    ("reinforce_cost", 0),
                    ("progress_level", 0),
                    ("progress_exp", 0),
                    ("star_level", 0),
                    ("rune_id", 0),
                ],
            );
            m.insert("refineryLock".into(), JsonValue::from(false));
            insert_gold_tiers(&mut m);
        }
        ItemClass::Treasure => {
            put_u32s(
                &mut m,
                &[
                    ("reinforce_level", 1),
                    ("reinforce_exp", 0),
                    ("progress_level", 0),
                    ("progress_exp", 0),
                ],
            );
            insert_gold_tiers(&mut m);
        }
        ItemClass::Awake => {
            put_u32s(&mut m, &[("progress_level", 0), ("progress_exp", 0)]);
            insert_gold_tiers(&mut m);
        }
        ItemClass::Pet => {
            put_u32s(
                &mut m,
                &[
                    ("reinforce_level", 1),
                    ("reinforce_exp", 0),
                    ("progress_level", 1),
                    ("progress_exp", 0),
                    ("break_level", 0),
                    ("gold_rank", 0),
                    ("gold_level", 0),
                    ("dark_gold_rank", 0),
                    ("dark_gold_level", 0),
                    ("purple_gold_rank", 0),
                    ("purple_gold_level", 0),
                ],
            );
        }
        ItemClass::GodArm => {
            put_u32s(
                &mut m,
                &[
                    ("upgrade_level", 0),
                    ("godArm_lock", 0),
                    ("godArm_red_rank", 0),
                    ("godArm_gold_rank", 0),
                ],
            );
            for k in [
                "godArm_wash",
                "godArm_last_wash",
                "godArm_wash_cost",
                "godArm_real_wash",
                "godArm_real_wash_count",
                "godArm_real_wash_last_info",
                "godArm_va_gem",
            ] {
                m.insert(k.into(), JsonValue::empty_object());
            }
        }
        ItemClass::Spirit => put_u32s(&mut m, &[("level", 1), ("exp", 0)]),
        _ => {}
    }
    m
}

fn insert_gold_tiers(m: &mut BTreeMap<String, JsonValue>) {
    for prefix in [
        "gold",
        "dark_gold",
        "purple_gold",
        "color_gold",
        "nirvana_gold",
        "gild_gold",
        "iron_gold",
    ] {
        m.insert(format!("{prefix}_rank"), JsonValue::from(0u32));
        m.insert(format!("{prefix}_level"), JsonValue::from(0u32));
    }
}

wire_record! {
    /// Single item entry in a sub-bag or equipped slot (`BaseItem.lua`).
    #[derive(Clone, Debug, PartialEq)]
    pub struct BagItem {
        gid: u32 = opt_u32(0),
        item_id: u64 = req_u64(),
        item_template_id: u32 = req_u32(),
        item_num: u32 = req_u32(),
        item_time: u64 = opt_u64(0),
        item_bind: u8 = opt_u8(0),
        va_item_text: BTreeMap<String, JsonValue> = json_obj(),
    }
    encode(this, m) => pub fn to_wire_json;
    decode(r, "BagItem") => pub fn from_json;
}

impl BagItem {
    pub fn new(
        gid: u32,
        item_id: u64,
        item_template_id: u32,
        item_num: u32,
        item_time: u64,
    ) -> Result<Self> {
        let (class, _) = classify_template_id(item_template_id)?;
        Ok(Self {
            gid,
            item_id,
            item_template_id,
            item_num,
            item_time,
            item_bind: 0,
            va_item_text: default_va_item_text(class),
        })
    }
}

/// Complete player bag state matching `bag.bagInfo` (`BagData.lua`).
#[derive(Clone, Debug, PartialEq)]
pub struct InventoryState {
    pub daily_use: BTreeMap<u32, u32>,
    pub grid_max_num: BTreeMap<BagType, u32>,
    pub grid_open_times: BTreeMap<BagType, u32>,
    pub bags: BTreeMap<BagType, BTreeMap<u32, BagItem>>,
}

impl Default for InventoryState {
    fn default() -> Self {
        Self::new()
    }
}

impl InventoryState {
    pub fn new() -> Self {
        let mut grid_max_num = BTreeMap::new();
        let mut grid_open_times = BTreeMap::new();
        let mut bags = BTreeMap::new();
        for bag in BagType::ALL {
            grid_max_num.insert(bag, bag.default_max_grids());
            grid_open_times.insert(bag, 0);
            bags.insert(bag, BTreeMap::new());
        }
        Self {
            daily_use: BTreeMap::new(),
            grid_max_num,
            grid_open_times,
            bags,
        }
    }

    /// Total count of items matching `template_id` across its sub-bag.
    pub fn count_by_template(&self, template_id: u32) -> u64 {
        let Ok((_, bag_type)) = classify_template_id(template_id) else {
            return 0;
        };
        self.bags
            .get(&bag_type)
            .map(|sub| {
                sub.values()
                    .filter(|it| it.item_template_id == template_id)
                    .map(|it| u64::from(it.item_num))
                    .sum()
            })
            .unwrap_or(0)
    }

    pub fn get_by_gid(&self, gid: u32) -> Option<&BagItem> {
        let bag_type = BagType::from_gid(gid)?;
        self.bags.get(&bag_type)?.get(&gid)
    }

    pub fn get_by_item_id(&self, item_id: u64) -> Option<(BagType, &BagItem)> {
        for (bt, sub) in &self.bags {
            for item in sub.values() {
                if item.item_id == item_id {
                    return Some((*bt, item));
                }
            }
        }
        None
    }

    /// Allocate the lowest free `gid` inside `[grid_start, grid_start + max_grids - 1]`.
    pub fn next_free_gid(&self, bag_type: BagType) -> Result<u32> {
        let (start, hard_end) = bag_type.gid_range();
        let max_grids = self
            .grid_max_num
            .get(&bag_type)
            .copied()
            .unwrap_or_else(|| bag_type.default_max_grids());
        let sub = self
            .bags
            .get(&bag_type)
            .ok_or_else(|| PlayerError::InvalidParameter("missing sub-bag".into()))?;
        if sub.len() as u32 >= max_grids {
            return Err(PlayerError::BagFull {
                bag_type: bag_type.vnsg2_key(),
                max_grids,
            });
        }
        let cap_end = start
            .saturating_add(max_grids.saturating_sub(1))
            .min(hard_end);
        for candidate in start..=cap_end {
            if !sub.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
        Err(PlayerError::BagFull {
            bag_type: bag_type.vnsg2_key(),
            max_grids,
        })
    }

    /// Insert an existing `BagItem` instance (for example when unequipping an item back to bag).
    pub fn insert_instance(&mut self, mut item: BagItem) -> Result<BagItem> {
        let (_, bag_type) = classify_template_id(item.item_template_id)?;
        let gid = self.next_free_gid(bag_type)?;
        item.gid = gid;
        self.bags
            .entry(bag_type)
            .or_default()
            .insert(gid, item.clone());
        Ok(item)
    }

    /// Expand a sub-bag's capacity (`bag.openGridByGold` / `bag.openGridByItem`).
    pub fn expand_bag_grids(&mut self, bag_type: BagType, add_grids: u32, add_times: u32) {
        let (_, hard_end) = bag_type.gid_range();
        let hard_cap = hard_end - bag_type.grid_start() + 1;
        let max_entry = self
            .grid_max_num
            .entry(bag_type)
            .or_insert_with(|| bag_type.default_max_grids());
        *max_entry = max_entry.saturating_add(add_grids).min(hard_cap);
        let times_entry = self.grid_open_times.entry(bag_type).or_insert(0);
        *times_entry = times_entry.saturating_add(add_times);
    }

    /// Serialize to exact `vnsg2` `bag.bagInfo` schema (`BagData:parseData`),
    /// plus classic Babeltime 1st-gen sub-bag alias keys.
    pub fn to_vnsg2_bag_info(&self) -> JsonValue {
        let mut root = BTreeMap::new();
        root.insert("daily_use".into(), encode_map(&self.daily_use));

        let mut grid_start = BTreeMap::new();
        let mut grid_max = BTreeMap::new();
        let mut grid_open = BTreeMap::new();

        for bag in BagType::ALL {
            let key = bag.vnsg2_key();
            grid_start.insert(key.into(), JsonValue::from(bag.grid_start()));
            let max_n = self
                .grid_max_num
                .get(&bag)
                .copied()
                .unwrap_or_else(|| bag.default_max_grids());
            let open_n = self.grid_open_times.get(&bag).copied().unwrap_or(0);
            grid_max.insert(key.into(), JsonValue::from(max_n));
            grid_open.insert(key.into(), JsonValue::from(open_n));

            let mut sub_obj = BTreeMap::new();
            if let Some(sub) = self.bags.get(&bag) {
                for (gid, item) in sub {
                    sub_obj.insert(gid.to_string(), item.to_wire_json());
                }
            }
            root.insert(key.into(), JsonValue::Object(sub_obj));
        }

        for (alias, src) in [
            ("props", "item"),
            ("heroFrag", "soul"),
            ("treas", "treasure"),
            ("armFrag", "fragment"),
            ("godarm", "god_arm"),
            ("rune", "gem"),
        ] {
            let val = root.get(src).cloned().unwrap_or_else(JsonValue::empty_object);
            root.insert(alias.into(), val);
        }
        root.insert("dress".into(), JsonValue::empty_object());
        root.insert("gridStart".into(), JsonValue::Object(grid_start));
        root.insert("gridMaxNum".into(), JsonValue::Object(grid_max));
        root.insert("gridOpenTimes".into(), JsonValue::Object(grid_open));
        JsonValue::Object(root)
    }

    pub fn to_storage_json(&self) -> JsonValue {
        let mut root = BTreeMap::new();
        root.insert("daily_use".into(), encode_map(&self.daily_use));

        let mut grid_max = BTreeMap::new();
        let mut grid_open = BTreeMap::new();
        let mut bags_obj = BTreeMap::new();

        for bag in BagType::ALL {
            let key = bag.vnsg2_key();
            grid_max.insert(
                key.into(),
                JsonValue::from(
                    self.grid_max_num
                        .get(&bag)
                        .copied()
                        .unwrap_or_else(|| bag.default_max_grids()),
                ),
            );
            grid_open.insert(
                key.into(),
                JsonValue::from(self.grid_open_times.get(&bag).copied().unwrap_or(0)),
            );

            if let Some(sub) = self.bags.get(&bag) {
                if !sub.is_empty() {
                    let mut sub_obj = BTreeMap::new();
                    for (gid, item) in sub {
                        sub_obj.insert(gid.to_string(), item.to_wire_json());
                    }
                    bags_obj.insert(key.into(), JsonValue::Object(sub_obj));
                }
            }
        }

        root.insert("grid_max_num".into(), JsonValue::Object(grid_max));
        root.insert("grid_open_times".into(), JsonValue::Object(grid_open));
        root.insert("bags".into(), JsonValue::Object(bags_obj));
        JsonValue::Object(root)
    }

    pub fn from_storage_json(val: &JsonValue) -> Result<Self> {
        let mut state = Self::new();
        let r = JsonObjReader::new(val, "InventoryState")?;
        state.daily_use = decode_u32_map(
            r.opt_obj("daily_use")?,
            "invalid daily_use key",
            "invalid daily_use count",
        )?;

        for (field, target) in [
            ("grid_max_num", &mut state.grid_max_num),
            ("grid_open_times", &mut state.grid_open_times),
        ] {
            if let Some(obj) = r.opt_obj(field)? {
                for (k, v) in obj {
                    if let (Some(bt), Some(n)) = (
                        BagType::from_key(k),
                        v.as_u64().and_then(|x| u32::try_from(x).ok()),
                    ) {
                        target.insert(bt, n);
                    }
                }
            }
        }

        if let Some(bags_obj) = r.opt_obj("bags")? {
            for (k, sub_val) in bags_obj {
                let bt = BagType::from_key(k)
                    .ok_or_else(|| PlayerError::Json(format!("unknown sub-bag key '{k}'")))?;
                let sub_map = sub_val
                    .as_object()
                    .ok_or_else(|| PlayerError::Json(format!("sub-bag '{k}' must be object")))?;
                let target = state.bags.entry(bt).or_default();
                for (gid_str, item_val) in sub_map {
                    let gid: u32 = gid_str
                        .parse()
                        .map_err(|_| PlayerError::Json(format!("invalid gid '{gid_str}'")))?;
                    let mut item = BagItem::from_json(item_val)?;
                    item.gid = gid;
                    target.insert(gid, item);
                }
            }
        }

        Ok(state)
    }
}
