use std::collections::{BTreeMap, BTreeSet};

use crate::error::{PlayerError, Result};
use crate::json::{JsonObjReader, JsonValue};

/// All 42 sub-bag tables defined in GServer (`vnsg2`) `BagConsts.lua`, `BagData.lua`, and `bag_v2/bag.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BagType {
    Treasure,
    Arm,
    Item,
    Fragment,
    Soul,
    Awake,
    AwakeFragment,
    TreasureFragment,
    Spirit,
    Pet,
    PetFragment,
    GodArm,
    GodArmFragment,
    HalidomFragment,
    Gem,
    GodAnimal,
    GodAnimalFragment,
    HeroLegend,
    HeroLegendFragment,
    Brochure,
    BrochureFragment,
    BingFuFragment,
    SoulSkill,
    StatueFragment,
    BingShuFragment,
    JobSkillFragment,
    YanWu,
    YanWuFragment,
    YanWuSoul,
    Counsellor,
    CounsellorFragment,
    CounsellorEquip,
    CounsellorEquipFragment,
    SoulStone,
    SoulStoneFragment,
    SoulJade,
    SoulJadeFragment,
    WareHouse,
    PloyFragment,
    TopAnimal,
    TopAnimalFragment,
    GreatSealFragment,
}

impl BagType {
    /// Complete ordered list of all 42 `vnsg2` sub-bags iterated by `BagData:parseData`.
    pub const ALL: [BagType; 42] = [
        BagType::Treasure,
        BagType::Arm,
        BagType::Item,
        BagType::Fragment,
        BagType::Soul,
        BagType::Awake,
        BagType::AwakeFragment,
        BagType::TreasureFragment,
        BagType::Spirit,
        BagType::Pet,
        BagType::PetFragment,
        BagType::GodArm,
        BagType::GodArmFragment,
        BagType::HalidomFragment,
        BagType::Gem,
        BagType::GodAnimal,
        BagType::GodAnimalFragment,
        BagType::HeroLegend,
        BagType::HeroLegendFragment,
        BagType::Brochure,
        BagType::BrochureFragment,
        BagType::BingFuFragment,
        BagType::SoulSkill,
        BagType::StatueFragment,
        BagType::BingShuFragment,
        BagType::JobSkillFragment,
        BagType::YanWu,
        BagType::YanWuFragment,
        BagType::YanWuSoul,
        BagType::Counsellor,
        BagType::CounsellorFragment,
        BagType::CounsellorEquip,
        BagType::CounsellorEquipFragment,
        BagType::SoulStone,
        BagType::SoulStoneFragment,
        BagType::SoulJade,
        BagType::SoulJadeFragment,
        BagType::WareHouse,
        BagType::PloyFragment,
        BagType::TopAnimal,
        BagType::TopAnimalFragment,
        BagType::GreatSealFragment,
    ];

    /// Exact Lua sub-bag key in `BagConsts.lua` / `bag.bagInfo`.
    pub const fn vnsg2_key(self) -> &'static str {
        match self {
            BagType::Treasure => "treasure",
            BagType::Arm => "arm",
            BagType::Item => "item",
            BagType::Fragment => "fragment",
            BagType::Soul => "soul",
            BagType::Awake => "awake",
            BagType::AwakeFragment => "awake_fragment",
            BagType::TreasureFragment => "treasure_fragment",
            BagType::Spirit => "spirit",
            BagType::Pet => "pet",
            BagType::PetFragment => "pet_fragment",
            BagType::GodArm => "god_arm",
            BagType::GodArmFragment => "god_arm_fragment",
            BagType::HalidomFragment => "halidom_fragment",
            BagType::Gem => "gem",
            BagType::GodAnimal => "god_animal",
            BagType::GodAnimalFragment => "god_animal_fragment",
            BagType::HeroLegend => "hero_legend",
            BagType::HeroLegendFragment => "hero_legend_fragment",
            BagType::Brochure => "brochure",
            BagType::BrochureFragment => "brochure_fragment",
            BagType::BingFuFragment => "bingfu_fragment",
            BagType::SoulSkill => "soul_skill",
            BagType::StatueFragment => "statue_fragment",
            BagType::BingShuFragment => "bingshu_fragment",
            BagType::JobSkillFragment => "jobskill_fragment",
            BagType::YanWu => "yanwu",
            BagType::YanWuFragment => "yanwu_fragment",
            BagType::YanWuSoul => "yanwu_soul",
            BagType::Counsellor => "counsellor",
            BagType::CounsellorFragment => "counsellor_fragment",
            BagType::CounsellorEquip => "counsellor_equip",
            BagType::CounsellorEquipFragment => "counsellor_equip_fragment",
            BagType::SoulStone => "soul_stone",
            BagType::SoulStoneFragment => "soul_stone_fragment",
            BagType::SoulJade => "soul_jade",
            BagType::SoulJadeFragment => "soul_jade_fragment",
            BagType::WareHouse => "ware_house",
            BagType::PloyFragment => "ploy_fragment",
            BagType::TopAnimal => "top_animal",
            BagType::TopAnimalFragment => "top_animal_fragment",
            BagType::GreatSealFragment => "great_seal_fragment",
        }
    }

    /// Resolve a `BagType` from either canonical `vnsg2` snake_case keys or classic camelCase aliases.
    pub fn from_key(key: &str) -> Option<Self> {
        if let Some(found) = Self::ALL.into_iter().find(|b| b.vnsg2_key() == key) {
            return Some(found);
        }
        match key {
            "props" | "normal" | "dress" => Some(BagType::Item),
            "treas" => Some(BagType::Treasure),
            "armFrag" => Some(BagType::Fragment),
            "heroFrag" | "soul_fragment" => Some(BagType::Soul),
            "awakeFrag" => Some(BagType::AwakeFragment),
            "treasureFrag" | "treasFrag" => Some(BagType::TreasureFragment),
            "petFrag" => Some(BagType::PetFragment),
            "godarm" | "god" => Some(BagType::GodArm),
            "godarmFrag" | "godFrag" => Some(BagType::GodArmFragment),
            "halidomFrag" => Some(BagType::HalidomFragment),
            "rune" => Some(BagType::Gem),
            "godAnimal" => Some(BagType::GodAnimal),
            "godAnimalFrag" => Some(BagType::GodAnimalFragment),
            "heroLegend" => Some(BagType::HeroLegend),
            "heroLegendFrag" => Some(BagType::HeroLegendFragment),
            "brochureFrag" => Some(BagType::BrochureFragment),
            "bingfu" | "bingfuFrag" => Some(BagType::BingFuFragment),
            "bingshu" | "bingshuFrag" => Some(BagType::BingShuFragment),
            "soulSkill" | "soulSkillFrag" => Some(BagType::SoulSkill),
            "statueFrag" => Some(BagType::StatueFragment),
            "jobskill" | "jobskillFrag" | "jobSkillFrag" => Some(BagType::JobSkillFragment),
            "yanWu" => Some(BagType::YanWu),
            "yanWuFrag" => Some(BagType::YanWuFragment),
            "yanWuSoul" => Some(BagType::YanWuSoul),
            "counsellorFrag" => Some(BagType::CounsellorFragment),
            "counsellorEquip" | "counsellor_arm" => Some(BagType::CounsellorEquip),
            "counsellorEquipFrag" | "counsellor_arm_fragment" => {
                Some(BagType::CounsellorEquipFragment)
            }
            "soulStone" => Some(BagType::SoulStone),
            "soulStoneFrag" => Some(BagType::SoulStoneFragment),
            "soulJade" => Some(BagType::SoulJade),
            "soulJadeFrag" => Some(BagType::SoulJadeFragment),
            "wareHouse" => Some(BagType::WareHouse),
            "ployFrag" => Some(BagType::PloyFragment),
            "topAnimal" => Some(BagType::TopAnimal),
            "topAnimalFrag" => Some(BagType::TopAnimalFragment),
            "greatSealFrag" => Some(BagType::GreatSealFragment),
            _ => None,
        }
    }

    /// Resolve `bag_v2/bag.json` numeric row ID (`2..=43`; row `1` is Hero roster).
    pub const fn bag_db_id(self) -> u32 {
        match self {
            BagType::Arm => 2,
            BagType::Treasure => 3,
            BagType::Item => 4,
            BagType::Soul => 5,
            BagType::Fragment => 6,
            BagType::Awake => 7,
            BagType::AwakeFragment => 8,
            BagType::TreasureFragment => 9,
            BagType::Spirit => 10,
            BagType::Pet => 11,
            BagType::PetFragment => 12,
            BagType::GodArm => 13,
            BagType::GodArmFragment => 14,
            BagType::GodAnimal => 15,
            BagType::GodAnimalFragment => 16,
            BagType::HeroLegend => 17,
            BagType::HeroLegendFragment => 18,
            BagType::Brochure => 19,
            BagType::BrochureFragment => 20,
            BagType::HalidomFragment => 21,
            BagType::Gem => 22,
            BagType::BingFuFragment => 23,
            BagType::SoulSkill => 24,
            BagType::StatueFragment => 25,
            BagType::BingShuFragment => 26,
            BagType::JobSkillFragment => 27,
            BagType::YanWu => 28,
            BagType::YanWuFragment => 29,
            BagType::YanWuSoul => 30,
            BagType::Counsellor => 31,
            BagType::CounsellorFragment => 32,
            BagType::CounsellorEquip => 33,
            BagType::CounsellorEquipFragment => 34,
            BagType::SoulStone => 35,
            BagType::SoulStoneFragment => 36,
            BagType::SoulJade => 37,
            BagType::SoulJadeFragment => 38,
            BagType::WareHouse => 39,
            BagType::PloyFragment => 40,
            BagType::TopAnimal => 41,
            BagType::TopAnimalFragment => 42,
            BagType::GreatSealFragment => 43,
        }
    }

    /// Inclusive `(grid_start, grid_end)` range enforced by `BagData:getBagTypeByGid`.
    pub const fn gid_range(self) -> (u32, u32) {
        match self {
            BagType::Treasure => (10_001, 20_000),
            BagType::Arm => (20_001, 30_000),
            BagType::Item => (30_001, 40_000),
            BagType::Fragment => (40_001, 50_000),
            BagType::Soul => (50_001, 60_000),
            BagType::Awake => (60_001, 70_000),
            BagType::AwakeFragment => (70_001, 80_000),
            BagType::TreasureFragment => (80_001, 90_000),
            BagType::Spirit => (90_001, 100_000),
            BagType::Pet => (100_001, 110_000),
            BagType::PetFragment => (200_001, 210_000),
            BagType::GodArm => (210_001, 220_000),
            BagType::GodArmFragment => (220_001, 230_000),
            BagType::HalidomFragment => (230_001, 240_000),
            BagType::Gem => (240_001, 250_000),
            BagType::GodAnimal => (310_001, 320_000),
            BagType::GodAnimalFragment => (320_001, 330_000),
            BagType::HeroLegend => (410_001, 420_000),
            BagType::HeroLegendFragment => (420_001, 430_000),
            BagType::Brochure => (510_001, 520_000),
            BagType::BrochureFragment => (520_001, 530_000),
            BagType::BingFuFragment => (530_001, 540_000),
            BagType::SoulSkill => (540_001, 550_000),
            BagType::StatueFragment => (550_001, 560_000),
            BagType::BingShuFragment => (560_001, 570_000),
            BagType::JobSkillFragment => (570_001, 580_000),
            BagType::YanWu => (580_001, 590_000),
            BagType::YanWuFragment => (590_001, 600_000),
            BagType::YanWuSoul => (600_001, 610_000),
            BagType::Counsellor => (610_001, 620_000),
            BagType::CounsellorFragment => (620_001, 630_000),
            BagType::CounsellorEquip => (630_001, 640_000),
            BagType::CounsellorEquipFragment => (640_001, 650_000),
            BagType::SoulStone => (650_001, 660_000),
            BagType::SoulStoneFragment => (660_001, 670_000),
            BagType::SoulJade => (670_001, 680_000),
            BagType::SoulJadeFragment => (680_001, 690_000),
            BagType::WareHouse => (690_001, 700_000),
            BagType::PloyFragment => (700_001, 710_000),
            BagType::TopAnimal => (710_001, 720_000),
            BagType::TopAnimalFragment => (720_001, 730_000),
            BagType::GreatSealFragment => (730_001, 740_000),
        }
    }

    pub const fn grid_start(self) -> u32 {
        self.gid_range().0
    }

    pub const fn grid_end(self) -> u32 {
        self.gid_range().1
    }

    /// Default `gridMaxNum` extracted from `bag_v2/bag.json` + `BagData.lua`.
    pub const fn default_max_grids(self) -> u32 {
        match self {
            BagType::Arm
            | BagType::Treasure
            | BagType::Spirit
            | BagType::Pet
            | BagType::GodArm
            | BagType::Gem
            | BagType::YanWuSoul
            | BagType::Counsellor
            | BagType::CounsellorEquip => 200,
            BagType::Awake | BagType::GodAnimal | BagType::TopAnimal => 100,
            _ => 10_000,
        }
    }

    /// Slots unlocked per expansion purchase (`buy_storage` in `bag_v2/bag.json`).
    /// Returns `None` if the sub-bag is an uncapped shard/prop bag (`type == 2` without expansion).
    pub const fn buy_storage_step(self) -> Option<u32> {
        match self {
            BagType::Arm
            | BagType::Treasure
            | BagType::Spirit
            | BagType::Pet
            | BagType::CounsellorEquip => Some(10),
            BagType::Awake
            | BagType::GodArm
            | BagType::GodAnimal
            | BagType::Gem
            | BagType::YanWuSoul
            | BagType::Counsellor
            | BagType::TopAnimal => Some(5),
            _ => None,
        }
    }

    /// Maximum number of expansion purchases (`buy_storage_limit` in `bag_v2/bag.json`).
    pub const fn buy_storage_limit(self) -> Option<u32> {
        match self {
            BagType::Arm
            | BagType::Treasure
            | BagType::Spirit
            | BagType::Pet
            | BagType::Gem
            | BagType::YanWuSoul
            | BagType::Counsellor
            | BagType::CounsellorEquip => Some(40),
            BagType::Awake => Some(100),
            BagType::GodArm => Some(80),
            BagType::GodAnimal | BagType::TopAnimal => Some(20),
            _ => None,
        }
    }

    /// Exact tiered Gold price for the `next_open_index`-th (`1`-based!) purchase from `bag_v2/bag.json`:
    /// `[[1, 10], [2, 20], [3, 50], [4, 100], [limit, 200]]`.
    pub const fn buy_storage_gold_cost(next_open_index_1based: u32) -> u64 {
        match next_open_index_1based {
            0 | 1 => 10,
            2 => 20,
            3 => 50,
            4 => 100,
            _ => 200,
        }
    }

    /// Returns `true` when identical `item_template_id` items stack in one grid.
    /// Unique equipment instances (`Arm`, `Treasure`, `Awake`, `Spirit`, `Pet`, `GodArm`,
    /// `GodAnimal`, `Brochure`, `SoulSkill`, `YanWu`, `Counsellor`, `CounsellorEquip`,
    /// `SoulStone`, `SoulJade`, `TopAnimal`) allocate one grid per instance so independent
    /// `va_item_text` levels/wash states stay isolated.
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
                | BagType::Brochure
                | BagType::SoulSkill
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
        for bag in Self::ALL {
            let (lo, hi) = bag.gid_range();
            if gid >= lo && gid <= hi {
                return Some(bag);
            }
        }
        None
    }
}

/// Item class routed by `ItemFactory:getItemTypeByTid` in Normal & TimeMachine `ItemFactory.lua`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemClass {
    Normal,
    Direct,
    RandGift,
    Dress,
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
    SoulSkillFragment,
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
    WareHouse,
    PloyFragment,
    TopAnimal,
    TopAnimalFragment,
    GreatSealFragment,
    TreasureFragment,
}

/// Classify `item_template_id` across Normal (`game/struct/item/ItemFactory.lua`),
/// TimeMachine (`game/game_TM/struct_TM/item/ItemFactory.lua`), and classic Dress/Fragment ranges.
/// Every returned `BagType` is guaranteed to allocate GIDs strictly inside `BagData:getBagTypeByGid`
/// (`10_001..=740_000`) so Lua never encounters a `nil` bag type.
pub fn classify_template_id(tid: u32) -> Result<(ItemClass, BagType)> {
    let pair = match tid {
        10_001..=20_000 => (ItemClass::Normal, BagType::Item),
        20_001..=30_000 => (ItemClass::Direct, BagType::Item),
        30_001..=40_000 => (ItemClass::RandGift, BagType::Item),
        40_001..=50_000 | 80_001..=90_000 => (ItemClass::Dress, BagType::Item),
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
        1_900_001..=2_000_000 => (ItemClass::SoulSkillFragment, BagType::Item),
        3_100_001..=3_200_000 => (ItemClass::ArmFragment, BagType::Fragment),
        3_200_001..=3_300_000 => (ItemClass::TreasureFragment, BagType::TreasureFragment),
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
        5_200_001..=5_300_000 => (ItemClass::WareHouse, BagType::WareHouse),
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

/// Build non-nil `va_item_text` defaults required by all `vnsg2` item structs (`ArmItem.lua`,
/// `TreasureItem.lua`, `AwakeItem.lua`, `PetItem.lua`, `GodItem.lua`, `SpiritItem.lua`,
/// `GodAnimalItem.lua`, `TopAnimalItem.lua`, `BrochureItem.lua`, `SoulSkillItem.lua`,
/// `YanWuItem.lua`, `CounsellorItem.lua`, `CounsellorArmItem.lua`, `SoulStoneItem.lua`,
/// `SoulJadeItem.lua`, and `Dress`).
pub fn default_va_item_text(class: ItemClass) -> BTreeMap<String, JsonValue> {
    let mut m = BTreeMap::new();
    match class {
        ItemClass::Arm => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("reinforce_cost".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("progress_exp".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            m.insert("rune_id".into(), JsonValue::from(0u32));
            m.insert("refineryLock".into(), JsonValue::from(false));
            m.insert("wash".into(), JsonValue::empty_object());
            insert_gold_tiers(&mut m);
        }
        ItemClass::Treasure => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("reinforce_cost".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("progress_exp".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            insert_gold_tiers(&mut m);
        }
        ItemClass::Awake => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("progress_exp".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            insert_gold_tiers(&mut m);
        }
        ItemClass::Pet => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("reinforce_cost".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(1u32));
            m.insert("progress_exp".into(), JsonValue::from(0u32));
            m.insert("break_level".into(), JsonValue::from(0u32));
            insert_gold_tiers(&mut m);
        }
        ItemClass::GodArm => {
            m.insert("upgrade_level".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("godArm_wash".into(), JsonValue::empty_object());
            m.insert("godArm_last_wash".into(), JsonValue::empty_object());
            m.insert("godArm_wash_cost".into(), JsonValue::empty_object());
            m.insert("godArm_real_wash".into(), JsonValue::empty_object());
            m.insert("godArm_real_wash_count".into(), JsonValue::empty_object());
            m.insert(
                "godArm_real_wash_last_info".into(),
                JsonValue::empty_object(),
            );
            m.insert("godArm_lock".into(), JsonValue::from(0u32));
            m.insert("godArm_red_rank".into(), JsonValue::from(0u32));
            m.insert("godArm_gold_rank".into(), JsonValue::from(0u32));
            m.insert("godArm_va_gem".into(), JsonValue::empty_object());
            insert_gold_tiers(&mut m);
        }
        ItemClass::Spirit => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("total_exp".into(), JsonValue::from(0u32));
            m.insert("gold_rank".into(), JsonValue::from(0u32));
            m.insert("dark_gold_rank".into(), JsonValue::from(0u32));
            m.insert("purple_gold_rank".into(), JsonValue::from(0u32));
        }
        ItemClass::Dress => {
            m.insert("dress_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("exp".into(), JsonValue::from(0u32));
        }
        ItemClass::GodAnimal => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("reinforce_cost".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("progress_exp".into(), JsonValue::from(0u32));
            m.insert("godAnimal_red_rank".into(), JsonValue::from(0u32));
            m.insert("godAnimal_red_level".into(), JsonValue::from(0u32));
            insert_gold_tiers(&mut m);
        }
        ItemClass::TopAnimal => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("reinforce_cost".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("progress_exp".into(), JsonValue::from(0u32));
            m.insert("topAnimal_red_rank".into(), JsonValue::from(0u32));
            m.insert("topAnimal_red_level".into(), JsonValue::from(0u32));
            insert_gold_tiers(&mut m);
        }
        ItemClass::Brochure => {
            m.insert("brochure_lock".into(), JsonValue::from(0u32));
            m.insert("reinforce_level".into(), JsonValue::from(0u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("red_rank".into(), JsonValue::from(0u32));
            m.insert("red_level".into(), JsonValue::from(0u32));
            insert_gold_tiers(&mut m);
        }
        ItemClass::SoulSkill => {
            m.insert("reinforce_level".into(), JsonValue::from(1u32));
            m.insert("reinforce_exp".into(), JsonValue::from(0u32));
            m.insert("reinforce_cost".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(0u32));
        }
        ItemClass::YanWu => {
            m.insert("reinforce_level".into(), JsonValue::from(0u32));
            m.insert("progress_level".into(), JsonValue::from(1u32));
            m.insert("yanwu_soul".into(), JsonValue::empty_object());
        }
        ItemClass::Counsellor => {
            m.insert("progress_level".into(), JsonValue::from(1u32));
            m.insert("total_exp".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            m.insert("counsellor_awake".into(), JsonValue::from(0u32));
            m.insert("counsellor_wash".into(), JsonValue::empty_object());
        }
        ItemClass::CounsellorEquip => {
            m.insert("progress_level".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
        }
        ItemClass::SoulStone => {
            m.insert("upgrade_level".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            m.insert("wash_num".into(), JsonValue::from(0u32));
            m.insert("item_unlock".into(), JsonValue::empty_object());
            m.insert("wash_attr".into(), JsonValue::empty_object());
            m.insert("halo_attr".into(), JsonValue::empty_object());
            m.insert("soul_stone_power".into(), JsonValue::from(0u32));
        }
        ItemClass::SoulJade => {
            m.insert("upgrade_level".into(), JsonValue::from(0u32));
            m.insert("star_level".into(), JsonValue::from(0u32));
            m.insert("wash_num".into(), JsonValue::from(0u32));
            m.insert("item_unlock".into(), JsonValue::empty_object());
            m.insert("wash_attr".into(), JsonValue::empty_object());
            m.insert("halo_attr".into(), JsonValue::empty_object());
            m.insert("soul_jade_qi".into(), JsonValue::from(0u32));
        }
        _ => {}
    }
    m
}

fn insert_gold_tiers(m: &mut BTreeMap<String, JsonValue>) {
    for k in [
        "gold_rank",
        "gold_level",
        "dark_gold_rank",
        "dark_gold_level",
        "purple_gold_rank",
        "purple_gold_level",
        "color_gold_rank",
        "color_gold_level",
        "nirvana_gold_rank",
        "nirvana_gold_level",
        "gild_gold_rank",
        "gild_gold_level",
        "iron_gold_rank",
        "iron_gold_level",
    ] {
        m.insert(k.into(), JsonValue::from(0u32));
    }
}

/// Single item entry in a sub-bag or equipped slot (`BaseItem.lua`).
#[derive(Clone, Debug, PartialEq)]
pub struct BagItem {
    pub gid: u32,
    pub item_id: u64,
    pub item_template_id: u32,
    pub item_num: u32,
    pub item_time: u64,
    pub item_bind: u8,
    pub va_item_text: BTreeMap<String, JsonValue>,
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

    /// Returns `true` when any item-level lock (`refineryLock`, `godArm_lock`, `brochure_lock`) is active.
    pub fn is_locked(&self) -> bool {
        if self
            .va_item_text
            .get("refineryLock")
            .and_then(JsonValue::as_bool)
            == Some(true)
        {
            return true;
        }
        for k in ["godArm_lock", "brochure_lock"] {
            if self
                .va_item_text
                .get(k)
                .and_then(JsonValue::as_u64)
                .unwrap_or(0)
                != 0
            {
                return true;
            }
        }
        false
    }

    pub fn to_wire_json(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("gid".into(), JsonValue::from(self.gid));
        m.insert("item_id".into(), JsonValue::from(self.item_id));
        m.insert(
            "item_template_id".into(),
            JsonValue::from(self.item_template_id),
        );
        m.insert("item_num".into(), JsonValue::from(self.item_num));
        m.insert("item_time".into(), JsonValue::from(self.item_time));
        m.insert("item_bind".into(), JsonValue::from(self.item_bind));
        m.insert(
            "va_item_text".into(),
            JsonValue::Object(self.va_item_text.clone()),
        );
        JsonValue::Object(m)
    }

    pub fn from_json(val: &JsonValue) -> Result<Self> {
        let r = JsonObjReader::new(val, "BagItem")?;
        let va_item_text = r.opt_obj("va_item_text")?.cloned().unwrap_or_default();
        Ok(Self {
            gid: r.opt_u32("gid", 0)?,
            item_id: r.req_u64("item_id")?,
            item_template_id: r.req_u32("item_template_id")?,
            item_num: r.req_u32("item_num")?,
            item_time: r.opt_u64("item_time", 0)?,
            item_bind: r.opt_u8("item_bind", 0)?,
            va_item_text,
        })
    }
}

/// Incremental S2C `bag.update` / `tmBag.update` delta tracker (`BagData:updateItemTb`).
///
/// Guarantees that whenever an item stack reaches `0` or is removed from a grid slot,
/// `[gid_str]` is emitted as an **empty JSON object `{}`** (`f.item_id == nil` in Lua),
/// preventing ghost items (`nil`) and zombie `x0` stacks (`item_num = 0`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BagDelta {
    /// Upserted or modified bag items with `item_num >= 1`, keyed by `gid`.
    pub upserted: BTreeMap<u32, BagItem>,
    /// Deleted grid IDs whose slots must be cleared via `{}`.
    pub removed_gids: BTreeSet<u32>,
}

impl BagDelta {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.upserted.is_empty() && self.removed_gids.is_empty()
    }

    /// Record an updated or newly inserted item. If `item.item_num == 0`, records a deletion (`{}`).
    pub fn record_item(&mut self, item: BagItem) {
        if item.gid == 0 {
            return;
        }
        if item.item_num == 0 {
            self.record_remove(item.gid);
        } else {
            self.removed_gids.remove(&item.gid);
            self.upserted.insert(item.gid, item);
        }
    }

    /// Record multiple updated or newly inserted items.
    pub fn record_items<I: IntoIterator<Item = BagItem>>(&mut self, items: I) {
        for item in items {
            self.record_item(item);
        }
    }

    /// Record an explicit slot deletion at `gid` (emitted as `{}`).
    pub fn record_remove(&mut self, gid: u32) {
        if gid == 0 {
            return;
        }
        self.upserted.remove(&gid);
        self.removed_gids.insert(gid);
    }

    /// Merge another `BagDelta` in chronological order.
    pub fn merge(&mut self, other: BagDelta) {
        for gid in other.removed_gids {
            self.record_remove(gid);
        }
        for (_, item) in other.upserted {
            self.record_item(item);
        }
    }

    /// Build the inner `{ [gid_str]: BagItem | {} }` map consumed by `BagData:updateItemTb`.
    pub fn to_gid_map_json(&self) -> JsonValue {
        let mut map = BTreeMap::new();
        for &gid in &self.removed_gids {
            map.insert(gid.to_string(), JsonValue::empty_object());
        }
        for (&gid, item) in &self.upserted {
            if item.item_num == 0 {
                map.insert(gid.to_string(), JsonValue::empty_object());
            } else {
                map.insert(gid.to_string(), item.to_wire_json());
            }
        }
        JsonValue::Object(map)
    }

    /// Build the exact 1-element array `[ { [gid_str]: BagItem | {} } ]` required by
    /// `BagData:onpushReceiveData(b) -> self:updateItemTb(b[1])` (`bag.update` / `tmBag.update`).
    pub fn to_bag_update_payload(&self) -> JsonValue {
        JsonValue::Array(vec![self.to_gid_map_json()])
    }
}

/// Complete player bag state matching `bag.bagInfo` / `tmBag.bagInfo` (`BagData.lua`).
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

    pub fn get_mut_by_gid(&mut self, gid: u32) -> Option<&mut BagItem> {
        let bag_type = BagType::from_gid(gid)?;
        self.bags.get_mut(&bag_type)?.get_mut(&gid)
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

    pub fn get_mut_by_item_id(&mut self, item_id: u64) -> Option<(BagType, &mut BagItem)> {
        for (bt, sub) in &mut self.bags {
            for item in sub.values_mut() {
                if item.item_id == item_id {
                    return Some((*bt, item));
                }
            }
        }
        None
    }

    /// Find the first grid slot containing `template_id` (ascending `gid`).
    pub fn find_first_by_template(&self, template_id: u32) -> Option<&BagItem> {
        let (_, bag_type) = classify_template_id(template_id).ok()?;
        self.bags
            .get(&bag_type)?
            .values()
            .find(|it| it.item_template_id == template_id)
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
        if bag_type.is_stackable() {
            if let Some(sub) = self.bags.get_mut(&bag_type) {
                if let Some(existing) = sub
                    .values_mut()
                    .find(|it| it.item_template_id == item.item_template_id)
                {
                    existing.item_num = existing.item_num.saturating_add(item.item_num.max(1));
                    return Ok(existing.clone());
                }
            }
        }
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

    /// Convenience wrapper over `BagDelta::to_bag_update_payload`.
    pub fn to_bag_update_payload(delta: &BagDelta) -> JsonValue {
        delta.to_bag_update_payload()
    }

    /// Serialize to exact `vnsg2` `bag.bagInfo` schema (`BagData:parseData`),
    /// plus classic Babeltime 1st-gen sub-bag alias keys (`props`, `heroFrag`, `treas`, `armFrag`, `godarm`, `dress`, `rune`)
    /// so every sub-bag table Lua iterates over is guaranteed non-nil.
    pub fn to_vnsg2_bag_info(&self) -> JsonValue {
        let mut root = BTreeMap::new();

        let mut daily = BTreeMap::new();
        for (tid, count) in &self.daily_use {
            daily.insert(tid.to_string(), JsonValue::from(*count));
        }
        root.insert("daily_use".into(), JsonValue::Object(daily));

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

        // Also populate classic 1st-gen sub-bag aliases so dual-protocol tools never see nil:
        root.insert(
            "props".into(),
            root.get("item")
                .cloned()
                .unwrap_or_else(JsonValue::empty_object),
        );
        root.insert(
            "heroFrag".into(),
            root.get("soul")
                .cloned()
                .unwrap_or_else(JsonValue::empty_object),
        );
        root.insert(
            "treas".into(),
            root.get("treasure")
                .cloned()
                .unwrap_or_else(JsonValue::empty_object),
        );
        root.insert(
            "armFrag".into(),
            root.get("fragment")
                .cloned()
                .unwrap_or_else(JsonValue::empty_object),
        );
        root.insert(
            "godarm".into(),
            root.get("god_arm")
                .cloned()
                .unwrap_or_else(JsonValue::empty_object),
        );
        root.insert("dress".into(), JsonValue::empty_object());
        root.insert(
            "rune".into(),
            root.get("gem")
                .cloned()
                .unwrap_or_else(JsonValue::empty_object),
        );

        root.insert("gridStart".into(), JsonValue::Object(grid_start));
        root.insert("gridMaxNum".into(), JsonValue::Object(grid_max));
        root.insert("gridOpenTimes".into(), JsonValue::Object(grid_open));

        JsonValue::Object(root)
    }

    /// Serialize TimeMachine `tmBag.bagInfo` schema (`game_TM/gamedata_TM/BagData.lua`).
    pub fn to_vnsg2_tm_bag_info(&self) -> JsonValue {
        self.to_vnsg2_bag_info()
    }

    pub fn to_storage_json(&self) -> JsonValue {
        let mut root = BTreeMap::new();

        let mut daily = BTreeMap::new();
        for (tid, count) in &self.daily_use {
            daily.insert(tid.to_string(), JsonValue::from(*count));
        }
        root.insert("daily_use".into(), JsonValue::Object(daily));

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

        if let Some(daily) = r.opt_obj("daily_use")? {
            for (k, v) in daily {
                let tid: u32 = k
                    .parse()
                    .map_err(|_| PlayerError::Json(format!("invalid daily_use key '{k}'")))?;
                let num = v
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| PlayerError::Json("invalid daily_use count".into()))?;
                state.daily_use.insert(tid, num);
            }
        }

        if let Some(gm) = r.opt_obj("grid_max_num")? {
            for (k, v) in gm {
                if let Some(bt) = BagType::from_key(k) {
                    if let Some(n) = v.as_u64().and_then(|x| u32::try_from(x).ok()) {
                        state.grid_max_num.insert(bt, n);
                    }
                }
            }
        }

        if let Some(go) = r.opt_obj("grid_open_times")? {
            for (k, v) in go {
                if let Some(bt) = BagType::from_key(k) {
                    if let Some(n) = v.as_u64().and_then(|x| u32::try_from(x).ok()) {
                        state.grid_open_times.insert(bt, n);
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
