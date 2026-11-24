//! Typed GServer (`vnsg2`) game configuration templates, lookup helpers, and
//! `LevelUpExpReader.lua`-compatible XP curve calculations.

use crate::generated_consts::{
    BAG_LIMITS, GEAR_SUITS, HEROES, ITEMS, LEVEL_EXP_CURVES, PROTAGONISTS, TOWNS,
};
use crate::json::{parse_json, JsonError, JsonValue};

/// Raw embedded `data/curated_templates.json` extracted from real GServer `.db` tables.
pub const EMBEDDED_CURATED_JSON: &str = include_str!("../data/curated_templates.json");

/// Parses the embedded `curated_templates.json` document into a [`JsonValue`] tree.
pub fn embedded_curated_json() -> Result<JsonValue, JsonError> {
    parse_json(EMBEDDED_CURATED_JSON)
}

/// GServer virtual reward currency / resource type IDs (first field of `type|id|count` triples).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RewardCurrencyType {
    Silver = 1,
    Gold = 2,
    Item = 4,
    Hero = 5,
    Prestige = 9,
    TowerCoin = 10,
    Stamina = 14,
    SoulJewel = 15,
    ClanContribution = 17,
    Energy = 18,
}

/// Three Kingdoms faction (`country` column in `hero.db` / `timeMachine_hero.db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kingdom {
    Protagonist = 0,
    Wei = 1,
    Shu = 2,
    Wu = 3,
    Qun = 4,
    Famous = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemCategory {
    StaminaPill,
    EnergyPill,
    GoldVoucher,
    RecruitToken,
    ExpItem,
    UpgradeMaterial,
    RedMaterial,
    GoldMaterial,
    EquipmentChest,
    HeroChest,
    FragmentChest,
    ChestKey,
    HeroFragment,
    StarterGear,
    PurpleGear,
    OrangeGear,
    RedGear,
    GoldGear,
    DarkGoldGear,
    PurpleGoldGear,
    ColorGoldGear,
    GildGoldGear,
    RedTreasure,
    GoldTreasure,
    ProtagonistJewel,
}

/// Male & Female Protagonist creation template (`CreateUserScene.lua` + `current_config.db` + `hero.db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtagonistTemplate {
    /// Character creation `utid` sent by `CreateUserProxy:createUser(utid, uname)` (`1` = Male, `2` = Female).
    pub utid: u32,
    /// Starter hero template ID (`htid`) placed in formation slot 1 (`1` = Male, `2` = Female).
    pub starter_htid: u32,
    pub name_vi: &'static str,
    pub name_en: &'static str,
    /// `1` = Male, `2` = Female.
    pub sex: u8,
    pub career: u8,
    /// `CreateUserScene.lua` `heroIndex` (`1` = Male, `0` = Female).
    pub hero_index_create_ui: u8,
    /// Spine model ID `1000000 + utid` (`1000001` Male, `1000002` Female).
    pub spine_res_id: u32,
    pub motion_id: &'static str,
    pub head_icon: &'static str,
    pub body_image: &'static str,
    pub init_quality: u8,
    pub qualification: u16,
    pub exp_curve_id: u32,
    pub destiny_treasure_id: u32,
    /// All quality/destiny evolution `htid`s for this protagonist gender.
    pub quality_htids: &'static [u32],
}

/// Authentic Three Kingdoms Hero template (`hero.db` / `timeMachine_hero.db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeroTemplate {
    pub htid: u32,
    pub name_vi: &'static str,
    pub name_en: &'static str,
    pub kingdom: Kingdom,
    pub sex: u8,
    pub career: u8,
    pub init_quality: u8,
    pub qualification: u16,
    pub exp_curve_id: u32,
    pub fragment_tid: u32,
    pub fragment_count: u16,
    pub motion_id: &'static str,
    pub awakened_htid: u32,
    pub red_evolved_htid: u32,
    pub gold_htid: u32,
    pub dark_gold_htid: u32,
    pub purple_gold_htid: u32,
    pub color_gold_htid: u32,
}

impl HeroTemplate {
    /// Returns `true` if `candidate_htid` matches this hero's base or any promoted tier ID.
    pub fn matches_any_tier(&self, candidate_htid: u32) -> bool {
        candidate_htid != 0
            && (self.htid == candidate_htid
                || self.awakened_htid == candidate_htid
                || self.red_evolved_htid == candidate_htid
                || self.gold_htid == candidate_htid
                || self.dark_gold_htid == candidate_htid
                || self.purple_gold_htid == candidate_htid
                || self.color_gold_htid == candidate_htid)
    }
}

/// Authentic Item template (`item_normal.db`, `item_direct`, `item_randgift`, `item_soul`, `item_arm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTemplate {
    pub item_tid: u32,
    pub category: ItemCategory,
    pub db_table: &'static str,
    pub name_vi: &'static str,
    pub name_en: &'static str,
    pub quality: u8,
    pub stack_limit: u32,
    pub desc_vi: &'static str,
}

/// Authentic 4-piece equipment set from `suit.db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GearSuitTemplate {
    pub suit_id: u32,
    pub name_vi: &'static str,
    pub quality: u8,
    pub max_lock: u8,
    pub suit_items: &'static [u32],
}

/// Authentic town / hub map template from `town.db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TownTemplate {
    pub town_id: u32,
    pub name_vi: &'static str,
    pub town_type: u8,
    pub born_x: i32,
    pub born_y: i32,
    pub in_level: u32,
    pub see_number: u32,
    pub call_number: u32,
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
    pub npc_ids: &'static [u32],
}

/// Authentic inventory bag limit configuration from `bag.db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BagLimitTemplate {
    pub bag_id: u32,
    pub name_vi: &'static str,
    pub db_names: &'static [&'static str],
    /// `1` = capped slot bag (expandable), `2` = uncapped stack/shard bag (`base_capacity == 0`).
    pub bag_type: u8,
    pub base_capacity: u32,
    pub buy_storage_limit: u32,
    pub buy_storage_step: u32,
}

impl BagLimitTemplate {
    /// Maximum capacity achievable after purchasing all allowed expansions (`0` = unlimited).
    pub const fn max_expandable_capacity(&self) -> u32 {
        if self.base_capacity == 0 {
            0
        } else {
            self.base_capacity + self.buy_storage_limit * self.buy_storage_step
        }
    }
}

/// Authentic 231-column (`exp_0..exp_230`) level-up XP curve from `level_up_exp.db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelExpCurveTemplate {
    pub curve_id: u32,
    pub name_vi: &'static str,
    pub max_level: u32,
    pub exp_by_index: [u64; 231],
}

impl LevelExpCurveTemplate {
    /// Exact port of `LevelUpExpReader:getNextLevelNeedExpById(id, level)`.
    ///
    /// `current_level` is the current character/hero level (e.g., `1` returns `exp_1 = 20` on curve `1`).
    pub fn next_level_need_exp(&self, current_level: u32) -> u64 {
        let idx = usize::try_from(current_level).unwrap_or(usize::MAX);
        self.exp_by_index.get(idx).copied().unwrap_or(0)
    }

    /// Exact port of `LevelUpExpReader:checkLevelCanUp(id, level)`.
    pub fn can_level_up(&self, current_level: u32) -> bool {
        current_level >= 1 && current_level < self.max_level && self.next_level_need_exp(current_level) > 0
    }

    /// Exact port of `LevelUpExpReader:getTotalExpByIdAndLevel(id, level)`:
    /// cumulative XP required from level 1 to reach `target_level`.
    pub fn total_exp_for_level(&self, target_level: u32) -> u64 {
        if target_level <= 1 {
            return 0;
        }
        let end_idx = usize::try_from(target_level - 1)
            .unwrap_or(self.exp_by_index.len() - 1)
            .min(self.exp_by_index.len() - 1);
        let mut total = 0u64;
        for &exp_val in self.exp_by_index.iter().take(end_idx + 1) {
            total = total.saturating_add(exp_val);
        }
        total
    }

    /// Exact port of `LevelUpExpReader:getLevelByTotalExp(totalExp, oldLevel, id)`.
    pub fn level_from_total_exp(&self, total_exp: u64, old_level: u32) -> u32 {
        let mut new_level = old_level.max(1);
        let mut target_level = new_level + 1;
        while total_exp >= self.total_exp_for_level(target_level) {
            if !self.can_level_up(new_level) {
                break;
            }
            new_level += 1;
            target_level += 1;
        }
        new_level
    }
}

/// Looks up a protagonist template by character-creation `utid` (`1` = Male, `2` = Female).
pub fn protagonist_by_utid(utid: u32) -> Option<&'static ProtagonistTemplate> {
    PROTAGONISTS.iter().find(|p| p.utid == utid)
}

/// Returns `true` if `htid` belongs to either the Male or Female protagonist evolution chain.
pub fn is_protagonist_htid(htid: u32) -> bool {
    PROTAGONISTS.iter().any(|p| p.quality_htids.contains(&htid))
}

/// Looks up a curated hero template by base or promoted `htid`.
pub fn hero_by_htid(htid: u32) -> Option<&'static HeroTemplate> {
    HEROES.iter().find(|h| h.matches_any_tier(htid))
}

/// Case-insensitive lookup by English or Vietnamese hero name (e.g. `"Zhao Yun"` or `"Triệu Vân"`).
pub fn hero_by_name(query: &str) -> Option<&'static HeroTemplate> {
    let q = query.trim().to_lowercase();
    HEROES
        .iter()
        .find(|h| h.name_en.to_lowercase() == q || h.name_vi.to_lowercase() == q)
}

/// Returns `true` if `htid` is a valid protagonist `htid` or curated hero `htid` (base or promoted).
pub fn is_valid_hero_htid(htid: u32) -> bool {
    is_protagonist_htid(htid) || hero_by_htid(htid).is_some()
}

/// Looks up a curated item template by `item_tid`.
pub fn item_by_tid(item_tid: u32) -> Option<&'static ItemTemplate> {
    ITEMS.iter().find(|i| i.item_tid == item_tid)
}

/// Returns all curated items matching `category`.
pub fn items_by_category(category: ItemCategory) -> Vec<&'static ItemTemplate> {
    ITEMS.iter().filter(|i| i.category == category).collect()
}

/// Returns `true` if `item_tid` exists in the curated item or suit tables.
pub fn is_valid_item_tid(item_tid: u32) -> bool {
    item_by_tid(item_tid).is_some()
        || GEAR_SUITS.iter().any(|s| s.suit_items.contains(&item_tid))
}

/// Looks up an equipment suit by `suit_id`.
pub fn gear_suit_by_id(suit_id: u32) -> Option<&'static GearSuitTemplate> {
    GEAR_SUITS.iter().find(|s| s.suit_id == suit_id)
}

/// Looks up a town template by `town_id`.
pub fn town_by_id(town_id: u32) -> Option<&'static TownTemplate> {
    TOWNS.iter().find(|t| t.town_id == town_id)
}

/// Returns `true` if `town_id` exists in `town.db`.
pub fn is_valid_town_id(town_id: u32) -> bool {
    town_by_id(town_id).is_some()
}

/// Looks up a bag capacity template by `bag_id`.
pub fn bag_by_id(bag_id: u32) -> Option<&'static BagLimitTemplate> {
    BAG_LIMITS.iter().find(|b| b.bag_id == bag_id)
}

/// Looks up a level-up XP curve by `curve_id` (`1` = Protagonist/Player, `2..=7` = Hero rarities, `8` = Counsellor).
pub fn exp_curve_by_id(curve_id: u32) -> Option<&'static LevelExpCurveTemplate> {
    LEVEL_EXP_CURVES.iter().find(|c| c.curve_id == curve_id)
}
