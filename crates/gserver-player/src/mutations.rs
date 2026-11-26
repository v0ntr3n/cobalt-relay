use std::collections::{BTreeMap, BTreeSet};

use crate::bag::{classify_template_id, default_va_item_text, BagDelta, BagItem, BagType, ItemClass};
use crate::error::{PlayerError, Result};
use crate::formation::{
    is_valid_battle_grid_cell, EquipSlotType, DEFAULT_SQUAD_TO_GRID,
};
use crate::json::JsonValue;
use crate::models::{
    CurrencyType, HeroInstance, PlayerProfile, StarterItemSeed, MAX_LUA_SAFE_INT,
};

/// Declarative effect applied when consuming an item via `PlayerProfile::use_item`.
#[derive(Clone, Debug, PartialEq)]
pub enum UseItemEffect {
    /// Consume the item with no extra side effect (for quest/script turn-ins).
    NoOp,
    /// Grant `amount_per_item * count` of `currency`.
    GrantCurrency {
        currency: CurrencyType,
        amount_per_item: u64,
    },
    /// Add `exp_per_item * count` player EXP (leveling up player & protagonist).
    AddPlayerExp { exp_per_item: u64 },
    /// Summon `count` instances of hero `htid` at level 1.
    SummonHero { htid: u32 },
    /// Open a gift box granting fixed items per box consumed.
    GrantItems { items_per_use: Vec<StarterItemSeed> },
}

/// Summary returned by legacy `PlayerProfile::use_item`.
#[derive(Clone, Debug, PartialEq)]
pub struct UseItemOutcome {
    pub consumed_template_id: u32,
    pub consumed_count: u32,
    pub remaining_in_grid: u32,
    pub daily_used_total: u32,
    pub summoned_hids: Vec<u64>,
    pub granted_items: Vec<BagItem>,
}

/// Summary returned by legacy `PlayerProfile::level_up_hero` (`hero.heroLevelUp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeroLevelUpOutcome {
    pub hid: u64,
    pub old_level: u32,
    pub new_level: u32,
    pub exp: u64,
}

/// Single sell specification accepted by `PlayerProfile::sell_bag_items` (`[tm]Bag.sellItems`).
/// Supports both `[gid, item_id, num]` triplets and `[gid]` / scalar `gid` shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SellBagEntry {
    pub gid: u32,
    pub item_id: Option<u64>,
    pub num: Option<u32>,
}

impl SellBagEntry {
    pub const fn gid_only(gid: u32) -> Self {
        Self {
            gid,
            item_id: None,
            num: None,
        }
    }

    pub const fn full(gid: u32, item_id: u64, num: u32) -> Self {
        Self {
            gid,
            item_id: Some(item_id),
            num: Some(num),
        }
    }

    /// Parse a single entry from AMF3/JSON (`[gid, item_id, num]`, `[gid]`, or integer `gid`).
    pub fn from_json(val: &JsonValue) -> Result<Self> {
        if let Some(gid_u64) = val.as_u64() {
            let gid = u32::try_from(gid_u64)
                .map_err(|_| PlayerError::InvalidParameter("sell gid overflow".into()))?;
            return Ok(Self::gid_only(gid));
        }
        if let Some(arr) = val.as_array() {
            if arr.is_empty() {
                return Err(PlayerError::InvalidParameter(
                    "empty sell entry array".into(),
                ));
            }
            let gid_u64 = arr[0]
                .as_u64()
                .or_else(|| arr[0].as_str().and_then(|s| s.parse::<u64>().ok()))
                .ok_or_else(|| PlayerError::InvalidParameter("invalid sell gid".into()))?;
            let gid = u32::try_from(gid_u64)
                .map_err(|_| PlayerError::InvalidParameter("sell gid overflow".into()))?;
            let item_id = arr
                .get(1)
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok())))
                .filter(|&id| id > 0);
            let num = arr
                .get(2)
                .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok())))
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n > 0);
            return Ok(Self { gid, item_id, num });
        }
        Err(PlayerError::InvalidParameter(
            "sell entry must be [gid, item_id, num], [gid], or gid".into(),
        ))
    }
}

/// Outcome returned by `PlayerProfile::sell_bag_items` (`[tm]Bag.sellItems`).
#[derive(Clone, Debug, PartialEq)]
pub struct SellBagOutcome {
    pub copper_added: u64,
    pub bag_deltas: BagDelta,
}

/// Target endpoint selector for `PlayerProfile::refinery_sell_fragments_or_props`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefinerySellKind {
    /// `[tm]Refinery.refineryFragment` (props & equipment/awake/pet/godarm/godanimal/brochure shards).
    Fragment,
    /// `[tm]Refinery.refinerySoul` (`soul` hero fragments -> `hero_soul`).
    Soul,
    /// `[tm]Refinery.refineryHeroLegend` (`hero_legend` & `hero_legend_fragment`).
    HeroLegend,
}

/// Outcome returned by `PlayerProfile::refinery_sell_fragments_or_props`.
#[derive(Clone, Debug, PartialEq)]
pub struct RefinerySellOutcome {
    pub ret: &'static str,
    pub rewards: BTreeMap<CurrencyType, u64>,
    pub reward_json: JsonValue,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::use_item_in_bag` & `PlayerProfile::use_item_by_tpl`
/// (`[tm]Exchange.useItemInBag` / `useItemInBagByTpl`).
#[derive(Clone, Debug, PartialEq)]
pub struct UseBagItemOutcome {
    pub consumed_template_id: u32,
    pub consumed_count: u32,
    /// Exact `Util:parseDropsToTable` / `BagProxy.lua` drop dictionary (`{ execution, energy, copper, gold, item, hero, ... }`).
    pub drop: JsonValue,
    pub granted_items: Vec<BagItem>,
    pub summoned_hids: Vec<u64>,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::open_randgift_box` (`openRandgiftBoxByKey` / `openRandgiftBoxByGold`).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenBoxOutcome {
    pub chest_tid: u32,
    pub open_count: u32,
    pub drops: Vec<JsonValue>,
    pub gold_spent: u64,
    pub bag_deltas: BagDelta,
    pub open_box_info: JsonValue,
}

/// Outcome returned by `PlayerProfile::open_bag_grid_by_gold` & `open_bag_grid_by_item`.
#[derive(Clone, Debug, PartialEq)]
pub struct BagExpandOutcome {
    pub bag_type: BagType,
    pub added_grids: u32,
    pub new_max_grids: u32,
    pub new_open_times: u32,
    pub gold_spent: u64,
    pub bag_deltas: BagDelta,
}

/// Target decompose category for `PlayerProfile::refinery_decompose` (`Lò Luyện` — `[tm]Refinery.*` & `soulSkill.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefineryDecomposeKind {
    Heroes,
    Arm,
    Treasure,
    Awake,
    Pet,
    Spirit,
    GodArm,
    GodAnimal,
    Brochures,
    YanWu,
    ResolveSpirit,
    SoulSkill,
}

/// Target reborn category for `PlayerProfile::refinery_reborn` (`Trùng Sinh` — `[tm]Refinery.reborn*`, `tmHeroPill.reborn`, `dress.rebornDress`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefineryRebornKind {
    Hero,
    OrangeHero,
    Arm,
    Treasure,
    Awake,
    Spirit,
    Pet,
    GodArm,
    GodAnimal,
    TopAnimal,
    Brochure,
    CounsellorEquip,
    SoulStone,
    SoulJade,
    Rune,
    HeroPill,
    Dress,
    SoulSkill,
}

/// Unified outcome returned by `refinery_decompose` and `refinery_reborn`.
#[derive(Clone, Debug, PartialEq)]
pub struct RefineryOutcome {
    pub gold_spent: u64,
    pub reward_currencies: BTreeMap<CurrencyType, u64>,
    pub reward_items: BTreeMap<u32, u32>,
    /// Exact Lua reward dictionary (`{ silver, copper, soul, hero_soul, jewel, awakeArm_coin, godarm_coin, pet_coin, spirit_coin, godanimal_coin, brochure_coin, item: { [tidStr]: cnt } }`).
    pub reward_dict: JsonValue,
    /// Array of `[price_type, tpl_id, amount]` triplets used by `soulSkill.refinery` / `dress.rebornDress`.
    pub reward_triplets: JsonValue,
    pub removed_hids: Vec<u64>,
    pub reset_hid: Option<u64>,
    pub reset_item_id: Option<u64>,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::compose_hero_from_frag` (`[tm]Exchange.composeHero`).
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeHeroOutcome {
    pub frag_tid: u32,
    pub consumed_frags: u32,
    pub heroes: Vec<HeroInstance>,
    pub summoned_hids: Vec<u64>,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `compose_item_from_frag`, `quick_compose_from_frag`, `compose_treasure`, & `one_key_compose_treasure`.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeItemOutcome {
    pub created_items: Vec<BagItem>,
    pub consumed_fragments: BTreeMap<u32, u32>,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::hero_level_up_vnsg2` (`[tm]Hero.levelUp` / `heroLevelUp`).
#[derive(Clone, Debug, PartialEq)]
pub struct HeroLevelUpVnsg2Outcome {
    pub hid: u64,
    pub old_lv: u32,
    pub newlv: u32,
    pub exp: u64,
    pub exp_delta: u64,
    pub copper_spent: u64,
    pub item_cost: BTreeMap<u32, u32>,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::evolve_hero_vnsg2` (`evolve.doEvolve` / `oneKeyEvolve`).
#[derive(Clone, Debug, PartialEq)]
pub struct HeroEvolveVnsg2Outcome {
    pub hid: u64,
    pub old_evolve_lv: u32,
    pub new_evolve_lv: u32,
    pub del_hero_hids: Vec<u64>,
    pub pills_used: u32,
    pub copper_spent: u64,
    pub bag_deltas: BagDelta,
}

/// Per-item reinforce step returned by `reinforce_arm_vnsg2` and `auto_reinforce_arm_vnsg2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmReinforceStep {
    pub level: u32,
    pub up_level: u32,
    pub cost_copper: u64,
}

impl ArmReinforceStep {
    pub fn to_wire_json(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("level".into(), JsonValue::from(self.level));
        m.insert("up_level".into(), JsonValue::from(self.up_level));
        m.insert("upLevel".into(), JsonValue::from(self.up_level));
        m.insert("cost_num".into(), JsonValue::from(self.cost_copper));
        JsonValue::Object(m)
    }
}

/// Outcome returned by `PlayerProfile::reinforce_arm_vnsg2` (`forge.reinforce`).
#[derive(Clone, Debug, PartialEq)]
pub struct ArmReinforceOutcome {
    pub item_id: u64,
    pub level: u32,
    pub up_level: u32,
    pub cost_copper: u64,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::upgrade_arm_vnsg2` (`forge.upgrade` / refine).
#[derive(Clone, Debug, PartialEq)]
pub struct ArmUpgradeOutcome {
    pub item_id: u64,
    pub level: u32,
    pub exp: u32,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::reinforce_treasure_vnsg2` (`forge.reinforceTreasure`).
#[derive(Clone, Debug, PartialEq)]
pub struct TreasureReinforceOutcome {
    pub item_id: u64,
    pub level: u32,
    pub exp: u32,
    pub copper_spent: u64,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::upgrade_treasure_vnsg2` (`forge.evolveTreasure` / `upgradeTreasure`).
#[derive(Clone, Debug, PartialEq)]
pub struct TreasureUpgradeOutcome {
    pub item_id: u64,
    pub progress_level: u32,
    pub copper_spent: u64,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::reinforce_spirit_vnsg2` (`spirit.upgrade` / `reinforce`).
#[derive(Clone, Debug, PartialEq)]
pub struct SpiritReinforceOutcome {
    pub item_id: u64,
    pub level: u32,
    pub exp: u32,
    pub total_exp: u32,
    pub bag_deltas: BagDelta,
}

/// Outcome returned by `PlayerProfile::upgrade_awake_vnsg2` (`awake.upgrade` / `progress`).
#[derive(Clone, Debug, PartialEq)]
pub struct AwakeUpgradeOutcome {
    pub item_id: u64,
    pub reinforce_level: u32,
    pub progress_level: u32,
    pub bag_deltas: BagDelta,
}

/// Sub-action for `PlayerProfile::godarm_wash_upgrade` (`godarm.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GodArmAction {
    Upgrade,
    Evolve,
    Wash,
    ReplaceWash,
}

/// Outcome returned by `PlayerProfile::explore_soul` (`spirit.exploreSoul` / `huntSoul`).
#[derive(Clone, Debug, PartialEq)]
pub struct ExploreSoulOutcome {
    pub created_spirits: Vec<BagItem>,
    pub spirit_coin_gained: u64,
    pub copper_spent: u64,
    pub gold_spent: u64,
    pub npc_stage: u32,
    pub bag_deltas: BagDelta,
}

// ============================================================================
// Internal Data-Driven Helpers
// ============================================================================

fn lookup_item_db_row(
    store: &gserver_data::GameDataStore,
    tid: u32,
) -> Option<&gserver_data::JsonValue> {
    let dir = store.export_dir()?;
    let id64 = u64::from(tid);
    for (db, tbl) in [
        ("timeMachine_item", "timeMachine_item_direct"),
        ("timeMachine_item", "timeMachine_item_normal"),
        ("timeMachine_item", "timeMachine_item_randgift"),
        ("timeMachine_hero", "timeMachine_item_soul"),
        ("item_normal", "item_normal"),
        ("item", "item_direct"),
        ("item", "item_normal"),
        ("item", "item_randgift"),
        ("item", "item_arm"),
        ("item", "item_treasure"),
        ("hunshi", "item_hunshi_fragment"),
        ("hunshi", "item_hunshi"),
    ] {
        if let Some(row) = dir.get_row_by_id(db, tbl, id64) {
            return Some(row);
        }
    }
    None
}

fn parse_db_u64(val: Option<&gserver_data::JsonValue>) -> Option<u64> {
    let v = val?;
    match v {
        gserver_data::JsonValue::Int(n) if *n >= 0 => Some(*n as u64),
        gserver_data::JsonValue::Str(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_db_reward_triplets(val: Option<&gserver_data::JsonValue>) -> Vec<(u32, u32, u64)> {
    let mut out = Vec::new();
    let Some(v) = val else {
        return out;
    };
    match v {
        gserver_data::JsonValue::Array(arr) => {
            for elem in arr {
                if let gserver_data::JsonValue::Array(trip) = elem {
                    if trip.len() >= 3 {
                        let ptype = parse_db_u64(trip.first()).unwrap_or(0) as u32;
                        let tpl = parse_db_u64(trip.get(1)).unwrap_or(0) as u32;
                        let amt = parse_db_u64(trip.get(2)).unwrap_or(0);
                        if ptype > 0 && amt > 0 {
                            out.push((ptype, tpl, amt));
                        }
                    }
                }
            }
        }
        gserver_data::JsonValue::Str(s) => {
            for part in s.split(',') {
                let segs: Vec<&str> = part.trim().split('|').collect();
                if segs.len() >= 3 {
                    if let (Ok(pt), Ok(id), Ok(am)) = (
                        segs[0].trim().parse::<u32>(),
                        segs[1].trim().parse::<u32>(),
                        segs[2].trim().parse::<u64>(),
                    ) {
                        if pt > 0 && am > 0 {
                            out.push((pt, id, am));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

fn default_sell_copper_per_unit(tid: u32, class: ItemClass) -> u64 {
    match class {
        ItemClass::Arm => {
            if tid >= 150_000 {
                20_000
            } else if tid >= 130_000 {
                10_000
            } else {
                5_000
            }
        }
        ItemClass::Treasure | ItemClass::Awake | ItemClass::Pet | ItemClass::GodArm => 10_000,
        ItemClass::GodAnimal | ItemClass::TopAnimal | ItemClass::SoulStone | ItemClass::SoulJade => {
            15_000
        }
        ItemClass::Spirit => 3_000,
        ItemClass::HeroFragment => 2_000,
        _ => 1_000,
    }
}

fn currency_to_price_type(c: CurrencyType) -> u32 {
    match c {
        CurrencyType::Copper => 1,
        CurrencyType::Gold => 2,
        CurrencyType::GoldBind => 3,
        CurrencyType::Prestige | CurrencyType::ArenaCoin => 9,
        CurrencyType::TowerCoin => 10,
        CurrencyType::Stone => 13,
        CurrencyType::Execution => 14,
        CurrencyType::HeroSoul => 15,
        CurrencyType::GuildPoint | CurrencyType::ClanContri => 17,
        CurrencyType::Energy => 18,
        CurrencyType::AwakeArmCoin | CurrencyType::JewelNum => 20,
        CurrencyType::PetCoin => 28,
        CurrencyType::GodarmCoin => 33,
        CurrencyType::GodanimalCoin => 34,
        CurrencyType::HeroLegendCoin => 39,
        CurrencyType::BrochureCoin => 46,
        CurrencyType::SpiritCoin => 56,
        CurrencyType::HunshixilianCoin => 89,
        _ => 1,
    }
}

fn build_refinery_reward_views(
    currencies: &BTreeMap<CurrencyType, u64>,
    items: &BTreeMap<u32, u32>,
) -> (JsonValue, JsonValue) {
    let mut dict = BTreeMap::new();
    let mut triplets = Vec::new();

    for (&c, &amt) in currencies {
        if amt == 0 {
            continue;
        }
        triplets.push(JsonValue::Array(vec![
            JsonValue::from(currency_to_price_type(c)),
            JsonValue::from(0u32),
            JsonValue::from(amt),
        ]));
        match c {
            CurrencyType::Copper => {
                dict.insert("silver".into(), JsonValue::from(amt));
                dict.insert("copper".into(), JsonValue::from(amt));
            }
            CurrencyType::HeroSoul => {
                dict.insert("soul".into(), JsonValue::from(amt));
                dict.insert("hero_soul".into(), JsonValue::from(amt));
            }
            CurrencyType::JewelNum | CurrencyType::AwakeArmCoin => {
                let prev = dict
                    .get("jewel")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0)
                    .max(amt);
                dict.insert("jewel".into(), JsonValue::from(prev));
                dict.insert("awakeArm_coin".into(), JsonValue::from(prev));
            }
            other => {
                dict.insert(other.vnsg2_key().into(), JsonValue::from(amt));
            }
        }
    }

    let mut item_obj = BTreeMap::new();
    for (&tid, &cnt) in items {
        if cnt > 0 {
            item_obj.insert(tid.to_string(), JsonValue::from(cnt));
            triplets.push(JsonValue::Array(vec![
                JsonValue::from(4u32),
                JsonValue::from(tid),
                JsonValue::from(cnt),
            ]));
        }
    }
    dict.insert("item".into(), JsonValue::Object(item_obj));

    (JsonValue::Object(dict), JsonValue::Array(triplets))
}

fn split_exp_into_pills(mut exp: u64) -> BTreeMap<u32, u32> {
    let mut pills = BTreeMap::new();
    for (pill_tid, val) in [
        (10_014u32, 10_000u64),
        (10_013u32, 5_000u64),
        (10_012u32, 1_500u64),
        (10_011u32, 300u64),
    ] {
        if exp >= val {
            let cnt = (exp / val) as u32;
            if cnt > 0 {
                pills.insert(pill_tid, cnt);
                exp %= val;
            }
        }
    }
    if exp > 0 {
        *pills.entry(10_011).or_insert(0) += 1;
    }
    pills
}

impl PlayerProfile {
    #[inline]
    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    // ========================================================================
    // 1. Currency, Stamina & Player EXP Progression
    // ========================================================================

    /// Grant `amount` of `currency` (capping `Copper` at `MAX_LUA_SAFE_INT`), returning the new balance.
    pub fn grant_currency(&mut self, currency: CurrencyType, amount: u64, now_ts: u64) -> u64 {
        let entry = self.currencies.entry(currency).or_insert(0);
        let raw_next = entry.saturating_add(amount);
        *entry = if currency == CurrencyType::Copper {
            raw_next.min(MAX_LUA_SAFE_INT)
        } else {
            raw_next
        };
        let updated = *entry;

        // Keep coupled classic/vnsg2 currency pairs synchronized
        match currency {
            CurrencyType::Prestige => {
                self.currencies.insert(CurrencyType::ArenaCoin, updated);
            }
            CurrencyType::ArenaCoin => {
                self.currencies.insert(CurrencyType::Prestige, updated);
            }
            CurrencyType::JewelNum => {
                self.currencies.insert(CurrencyType::AwakeArmCoin, updated);
            }
            CurrencyType::AwakeArmCoin => {
                self.currencies.insert(CurrencyType::JewelNum, updated);
            }
            CurrencyType::ClanContri => {
                self.currencies.insert(CurrencyType::GuildPoint, updated);
            }
            CurrencyType::GuildPoint => {
                self.currencies.insert(CurrencyType::ClanContri, updated);
            }
            CurrencyType::WorldCoin => {
                self.currencies.insert(CurrencyType::WorldTeamCoin, updated);
            }
            CurrencyType::WorldTeamCoin => {
                self.currencies.insert(CurrencyType::WorldCoin, updated);
            }
            CurrencyType::Execution if self.execution_time == 0 => {
                self.execution_time = now_ts;
            }
            CurrencyType::Energy if self.energy_time == 0 => {
                self.energy_time = now_ts;
            }
            _ => {}
        }

        self.bump_revision();
        updated
    }

    /// Deduct `amount` of `currency`, failing atomically with `InsufficientCurrency` if underfunded.
    pub fn consume_currency(
        &mut self,
        currency: CurrencyType,
        amount: u64,
        now_ts: u64,
    ) -> Result<u64> {
        let available = self.currency(currency);
        if available < amount {
            return Err(PlayerError::InsufficientCurrency {
                currency: currency.vnsg2_key(),
                available,
                required: amount,
            });
        }
        let next = available - amount;
        self.currencies.insert(currency, next);
        match currency {
            CurrencyType::Prestige => {
                self.currencies.insert(CurrencyType::ArenaCoin, next);
            }
            CurrencyType::ArenaCoin => {
                self.currencies.insert(CurrencyType::Prestige, next);
            }
            CurrencyType::JewelNum => {
                self.currencies.insert(CurrencyType::AwakeArmCoin, next);
            }
            CurrencyType::AwakeArmCoin => {
                self.currencies.insert(CurrencyType::JewelNum, next);
            }
            CurrencyType::ClanContri => {
                self.currencies.insert(CurrencyType::GuildPoint, next);
            }
            CurrencyType::GuildPoint => {
                self.currencies.insert(CurrencyType::ClanContri, next);
            }
            CurrencyType::WorldCoin => {
                self.currencies.insert(CurrencyType::WorldTeamCoin, next);
            }
            CurrencyType::WorldTeamCoin => {
                self.currencies.insert(CurrencyType::WorldCoin, next);
            }
            CurrencyType::Execution if now_ts > 0 => {
                self.execution_time = now_ts;
            }
            CurrencyType::Energy if now_ts > 0 => {
                self.energy_time = now_ts;
            }
            _ => {}
        }
        self.bump_revision();
        Ok(next)
    }

    /// Recover natural `execution` (every 360s up to cap 120) and `energy` (every 900s up to cap 50)
    /// matching `UserData:initExecutionTime` and `UserData:initEnergyTime` + `current_config.db`.
    pub fn tick_stamina_recovery(&mut self, now_ts: u64) {
        const EXEC_INTERVAL: u64 = 360;
        const EXEC_CAP: u64 = 120;
        const ENERGY_INTERVAL: u64 = 900;
        const ENERGY_CAP: u64 = 50;

        let mut changed = false;

        if now_ts > self.execution_time && self.execution_time > 0 {
            let cur_exec = self.currency(CurrencyType::Execution);
            if cur_exec < EXEC_CAP {
                let ticks = (now_ts - self.execution_time) / EXEC_INTERVAL;
                if ticks > 0 {
                    let next_exec = cur_exec.saturating_add(ticks).min(EXEC_CAP);
                    self.currencies.insert(CurrencyType::Execution, next_exec);
                    self.execution_time = if next_exec >= EXEC_CAP {
                        now_ts
                    } else {
                        self.execution_time + ticks * EXEC_INTERVAL
                    };
                    changed = true;
                }
            } else {
                self.execution_time = now_ts;
            }
        }

        if now_ts > self.energy_time && self.energy_time > 0 {
            let cur_energy = self.currency(CurrencyType::Energy);
            if cur_energy < ENERGY_CAP {
                let ticks = (now_ts - self.energy_time) / ENERGY_INTERVAL;
                if ticks > 0 {
                    let next_energy = cur_energy.saturating_add(ticks).min(ENERGY_CAP);
                    self.currencies.insert(CurrencyType::Energy, next_energy);
                    self.energy_time = if next_energy >= ENERGY_CAP {
                        now_ts
                    } else {
                        self.energy_time + ticks * ENERGY_INTERVAL
                    };
                    changed = true;
                }
            } else {
                self.energy_time = now_ts;
            }
        }

        if changed {
            self.bump_revision();
        }
    }

    /// Add protagonist/account EXP (`100 * level` curve) and keep the protagonist `HeroInstance` level synced.
    pub fn add_player_exp(&mut self, exp_gain: u64) -> (u32, u32) {
        let old_lv = self.level;
        self.exp = self.exp.saturating_add(exp_gain);
        while self.level < 200 {
            let needed = u64::from(self.level) * 100;
            if self.exp >= needed {
                self.exp -= needed;
                self.level += 1;
            } else {
                break;
            }
        }
        if let Some(protag) = self.heroes.get_mut(&self.protagonist_hid) {
            protag.level = self.level;
            protag.exp = self.exp;
        }
        self.recalculate_fight_force();
        self.bump_revision();
        (old_lv, self.level)
    }

    // ========================================================================
    // 2. Inventory / Bag Item Mutations, Selling, Use & Slot Expansion
    // ========================================================================

    /// Add `count` items of `template_id` to the appropriate sub-bag (`ItemFactory.lua` routing).
    pub fn add_item(&mut self, template_id: u32, count: u32, now_ts: u64) -> Result<Vec<BagItem>> {
        let (items, _) = self.add_item_with_delta(template_id, count, now_ts)?;
        Ok(items)
    }

    /// Add `count` items of `template_id` and return `(created_or_updated_items, BagDelta)`.
    pub fn add_item_with_delta(
        &mut self,
        template_id: u32,
        count: u32,
        now_ts: u64,
    ) -> Result<(Vec<BagItem>, BagDelta)> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "item count to add must be > 0".into(),
            ));
        }
        let (_, bag_type) = classify_template_id(template_id)?;
        let mut delta = BagDelta::new();

        if bag_type.is_stackable() {
            if let Some(sub) = self.inventory.bags.get_mut(&bag_type) {
                if let Some(existing) = sub
                    .values_mut()
                    .find(|it| it.item_template_id == template_id)
                {
                    existing.item_num = existing.item_num.saturating_add(count);
                    if now_ts > 0 {
                        existing.item_time = now_ts;
                    }
                    let updated = existing.clone();
                    delta.record_item(updated.clone());
                    self.bump_revision();
                    return Ok((vec![updated], delta));
                }
            }
            let gid = self.inventory.next_free_gid(bag_type)?;
            let item_id = self.next_item_id;
            self.next_item_id += 1;
            let item = BagItem::new(gid, item_id, template_id, count, now_ts)?;
            self.inventory
                .bags
                .entry(bag_type)
                .or_default()
                .insert(gid, item.clone());
            delta.record_item(item.clone());
            self.bump_revision();
            Ok((vec![item], delta))
        } else {
            let max_grids = self
                .inventory
                .grid_max_num
                .get(&bag_type)
                .copied()
                .unwrap_or_else(|| bag_type.default_max_grids());
            let used = self
                .inventory
                .bags
                .get(&bag_type)
                .map(|m| m.len() as u32)
                .unwrap_or(0);
            if used.saturating_add(count) > max_grids {
                return Err(PlayerError::BagFull {
                    bag_type: bag_type.vnsg2_key(),
                    max_grids,
                });
            }
            let mut created = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let gid = self.inventory.next_free_gid(bag_type)?;
                let item_id = self.next_item_id;
                self.next_item_id += 1;
                let item = BagItem::new(gid, item_id, template_id, 1, now_ts)?;
                self.inventory
                    .bags
                    .entry(bag_type)
                    .or_default()
                    .insert(gid, item.clone());
                delta.record_item(item.clone());
                created.push(item);
            }
            self.bump_revision();
            Ok((created, delta))
        }
    }

    /// Remove `count` items from a specific `gid` slot, deleting the grid entry when `item_num` hits `0`.
    pub fn remove_item_by_gid(&mut self, gid: u32, count: u32) -> Result<BagItem> {
        let (snapshot, _) = self.remove_item_by_gid_with_delta(gid, count)?;
        Ok(snapshot)
    }

    /// Remove `count` items from `gid` and emit a `BagDelta` (recording `{}` when stack hits `0`).
    pub fn remove_item_by_gid_with_delta(
        &mut self,
        gid: u32,
        count: u32,
    ) -> Result<(BagItem, BagDelta)> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "item count to remove must be > 0".into(),
            ));
        }
        let bag_type = BagType::from_gid(gid).ok_or_else(|| {
            PlayerError::ItemNotFound(format!("gid={gid} outside all bag ranges"))
        })?;
        let sub = self
            .inventory
            .bags
            .get_mut(&bag_type)
            .ok_or_else(|| PlayerError::ItemNotFound(format!("missing sub-bag for gid={gid}")))?;
        let item = sub
            .get_mut(&gid)
            .ok_or_else(|| PlayerError::ItemNotFound(format!("no item at gid={gid}")))?;

        if item.item_num < count {
            return Err(PlayerError::InsufficientItemCount {
                template_id: item.item_template_id,
                available: u64::from(item.item_num),
                required: u64::from(count),
            });
        }

        item.item_num -= count;
        let snapshot = item.clone();
        let mut delta = BagDelta::new();
        if snapshot.item_num == 0 {
            sub.remove(&gid);
            delta.record_remove(gid);
        } else {
            delta.record_item(snapshot.clone());
        }
        self.bump_revision();
        Ok((snapshot, delta))
    }

    /// Remove `count` items matching `template_id` across grids in ascending `gid` order.
    /// Returns `Vec<(gid, remaining_num)>`.
    pub fn remove_item_by_template(
        &mut self,
        template_id: u32,
        count: u32,
    ) -> Result<Vec<(u32, u32)>> {
        let (affected, _) = self.remove_item_by_template_with_delta(template_id, count)?;
        Ok(affected)
    }

    /// Remove `count` items matching `template_id` and return `(affected_gids, BagDelta)`.
    pub fn remove_item_by_template_with_delta(
        &mut self,
        template_id: u32,
        count: u32,
    ) -> Result<(Vec<(u32, u32)>, BagDelta)> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "item count to remove must be > 0".into(),
            ));
        }
        let available = self.inventory.count_by_template(template_id);
        if available < u64::from(count) {
            return Err(PlayerError::InsufficientItemCount {
                template_id,
                available,
                required: u64::from(count),
            });
        }
        let (_, bag_type) = classify_template_id(template_id)?;
        let sub = self.inventory.bags.entry(bag_type).or_default();

        let matching_gids: Vec<u32> = sub
            .iter()
            .filter_map(|(&gid, it)| {
                if it.item_template_id == template_id {
                    Some(gid)
                } else {
                    None
                }
            })
            .collect();

        let mut remaining_to_take = count;
        let mut affected = Vec::new();
        let mut delta = BagDelta::new();
        for gid in matching_gids {
            if remaining_to_take == 0 {
                break;
            }
            if let Some(it) = sub.get_mut(&gid) {
                let take = it.item_num.min(remaining_to_take);
                it.item_num -= take;
                remaining_to_take -= take;
                let snapshot = it.clone();
                let left = snapshot.item_num;
                if left == 0 {
                    sub.remove(&gid);
                    delta.record_remove(gid);
                } else {
                    delta.record_item(snapshot);
                }
                affected.push((gid, left));
            }
        }
        self.bump_revision();
        Ok((affected, delta))
    }

    /// Resolve an inventory item by either `item_id` (checked first per `BagItemProxy:useGoods`)
    /// or `gid` (`10_001..=740_000`).
    fn resolve_bag_gid_by_id_or_gid(&self, item_id_or_gid: u64) -> Result<u32> {
        if let Some((_, item)) = self.inventory.get_by_item_id(item_id_or_gid) {
            return Ok(item.gid);
        }
        if let Ok(gid) = u32::try_from(item_id_or_gid) {
            if self.inventory.get_by_gid(gid).is_some() {
                return Ok(gid);
            }
        }
        Err(PlayerError::ItemNotFound(format!(
            "no bag item matching item_id_or_gid={item_id_or_gid}"
        )))
    }

    /// Sell bag items for copper (`[tm]Bag.sellItems`), supporting `[gid, item_id, num]` and `[gid]` entries.
    pub fn sell_bag_items(
        &mut self,
        entries: &[SellBagEntry],
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<SellBagOutcome> {
        if entries.is_empty() {
            return Err(PlayerError::InvalidParameter(
                "sell_bag_items requires at least one entry".into(),
            ));
        }

        // Aggregate per-gid sell counts and pre-validate all entries before mutating
        let mut requested_by_gid: BTreeMap<u32, (Option<u64>, u32)> = BTreeMap::new();
        for entry in entries {
            let item = self.inventory.get_by_gid(entry.gid).ok_or_else(|| {
                PlayerError::ItemNotFound(format!("no item at gid={}", entry.gid))
            })?;
            if item.is_locked() {
                return Err(PlayerError::ResourceLockedOrDeployed(format!(
                    "item gid={} (item_id={}) is locked",
                    entry.gid, item.item_id
                )));
            }
            if let Some(exp_id) = entry.item_id {
                if exp_id != 0 && exp_id != item.item_id {
                    return Err(PlayerError::InvalidParameter(format!(
                        "sell item_id mismatch at gid={}: expected {}, got {exp_id}",
                        entry.gid, item.item_id
                    )));
                }
            }
            let add_num = entry.num.unwrap_or(item.item_num).max(1);
            let slot = requested_by_gid
                .entry(entry.gid)
                .or_insert((entry.item_id, 0));
            slot.1 = slot.1.saturating_add(add_num);
            if slot.1 > item.item_num {
                return Err(PlayerError::InsufficientItemCount {
                    template_id: item.item_template_id,
                    available: u64::from(item.item_num),
                    required: u64::from(slot.1),
                });
            }
        }

        let mut total_copper = 0u64;
        let mut bag_deltas = BagDelta::new();

        for (gid, (_, sell_num)) in requested_by_gid {
            let item = self
                .inventory
                .get_by_gid(gid)
                .cloned()
                .expect("pre-validated above");
            let (class, _) = classify_template_id(item.item_template_id)?;
            let unit_price = data_store
                .and_then(|ds| lookup_item_db_row(ds, item.item_template_id))
                .and_then(|row| {
                    parse_db_u64(row.get("sell_num"))
                        .or_else(|| parse_db_u64(row.get("sell_price")))
                        .or_else(|| parse_db_u64(row.get("count")))
                        .filter(|&v| v > 0)
                })
                .unwrap_or_else(|| default_sell_copper_per_unit(item.item_template_id, class));

            // Also refund any invested reinforce copper on sold equipment
            let invested_copper = item
                .va_item_text
                .get("reinforce_cost")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);

            let (_, delta) = self.remove_item_by_gid_with_delta(gid, sell_num)?;
            bag_deltas.merge(delta);
            total_copper = total_copper
                .saturating_add(unit_price.saturating_mul(u64::from(sell_num)))
                .saturating_add(invested_copper);
        }

        if total_copper > 0 {
            self.grant_currency(CurrencyType::Copper, total_copper, now_ts);
        }

        Ok(SellBagOutcome {
            copper_added: total_copper,
            bag_deltas,
        })
    }

    /// Convenience JSON wrapper for `sell_bag_items` accepting `[[gid, item_id, num], [gid], ...]`.
    pub fn sell_bag_items_json(
        &mut self,
        raw_entries: &JsonValue,
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<SellBagOutcome> {
        let arr = raw_entries.as_array().ok_or_else(|| {
            PlayerError::InvalidParameter("sellItems arg must be an array".into())
        })?;
        let mut parsed = Vec::with_capacity(arr.len());
        for v in arr {
            parsed.push(SellBagEntry::from_json(v)?);
        }
        self.sell_bag_items(&parsed, data_store, now_ts)
    }

    /// Sell props or fragments via the Bag `Bán` button (`[tm]Refinery.refineryFragment`,
    /// `refinerySoul`, `refineryHeroLegend`), granting shop/soul/silver currencies per DB
    /// `refinery_reward` / `hero_refinery_reward` and emitting `{}` deletion deltas.
    pub fn refinery_sell_fragments_or_props(
        &mut self,
        kind: RefinerySellKind,
        item_ids_or_gids: &[u64],
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<RefinerySellOutcome> {
        if item_ids_or_gids.is_empty() {
            return Err(PlayerError::InvalidParameter(
                "refinery_sell requires at least one item".into(),
            ));
        }

        let mut gids = Vec::with_capacity(item_ids_or_gids.len());
        let mut seen = BTreeSet::new();
        for &raw_id in item_ids_or_gids {
            let gid = self.resolve_bag_gid_by_id_or_gid(raw_id)?;
            if !seen.insert(gid) {
                return Err(PlayerError::InvalidParameter(format!(
                    "duplicate item gid={gid} in refinery sell"
                )));
            }
            let item = self.inventory.get_by_gid(gid).expect("resolved");
            if item.is_locked() {
                return Err(PlayerError::ResourceLockedOrDeployed(format!(
                    "item gid={gid} is locked"
                )));
            }
            let (class, _) = classify_template_id(item.item_template_id)?;
            match kind {
                RefinerySellKind::Soul if class != ItemClass::HeroFragment => {
                    return Err(PlayerError::InvalidParameter(format!(
                        "refinerySoul expects HeroFragment, got {:?}",
                        class
                    )));
                }
                RefinerySellKind::HeroLegend
                    if !matches!(
                        class,
                        ItemClass::HeroLegend | ItemClass::HeroLegendFragment
                    ) =>
                {
                    return Err(PlayerError::InvalidParameter(format!(
                        "refineryHeroLegend expects HeroLegend/HeroLegendFragment, got {:?}",
                        class
                    )));
                }
                _ => {}
            }
            gids.push(gid);
        }

        let mut rewards: BTreeMap<CurrencyType, u64> = BTreeMap::new();
        let mut bag_deltas = BagDelta::new();

        for gid in gids {
            let item = self.inventory.get_by_gid(gid).cloned().expect("verified");
            let count = u64::from(item.item_num);
            let (class, _) = classify_template_id(item.item_template_id)?;

            // Check DB `refinery_reward` / `hero_refinery_reward` first
            let mut db_triplets = Vec::new();
            if let Some(ds) = data_store {
                if let Some(row) = lookup_item_db_row(ds, item.item_template_id) {
                    db_triplets = parse_db_reward_triplets(
                        row.get("refinery_reward")
                            .or_else(|| row.get("hero_refinery_reward")),
                    );
                }
            }

            if !db_triplets.is_empty() {
                for (ptype, _tpl, amt) in db_triplets {
                    if let Some(ct) = CurrencyType::from_price_type(ptype) {
                        *rewards.entry(ct).or_insert(0) += amt.saturating_mul(count);
                    }
                }
            } else {
                match class {
                    ItemClass::HeroFragment => {
                        let htid = item.item_template_id.saturating_sub(50_000);
                        let per_shard = match gserver_data::hero_by_htid(htid)
                            .map(|h| h.init_quality)
                            .unwrap_or(3)
                        {
                            6..=u8::MAX => 50,
                            5 => 25,
                            4 => 20,
                            _ => 5,
                        };
                        *rewards.entry(CurrencyType::HeroSoul).or_insert(0) += per_shard * count;
                    }
                    ItemClass::AwakeFragment => {
                        *rewards.entry(CurrencyType::AwakeArmCoin).or_insert(0) += 25 * count;
                    }
                    ItemClass::GodArmFragment => {
                        *rewards.entry(CurrencyType::GodarmCoin).or_insert(0) += 25 * count;
                    }
                    ItemClass::PetFragment => {
                        *rewards.entry(CurrencyType::PetCoin).or_insert(0) += 25 * count;
                    }
                    ItemClass::GodAnimalFragment | ItemClass::TopAnimalFragment => {
                        *rewards.entry(CurrencyType::GodanimalCoin).or_insert(0) += 25 * count;
                    }
                    ItemClass::BrochureFragment => {
                        *rewards.entry(CurrencyType::BrochureCoin).or_insert(0) += 20 * count;
                    }
                    ItemClass::HeroLegend | ItemClass::HeroLegendFragment => {
                        *rewards.entry(CurrencyType::HeroLegendCoin).or_insert(0) += 50 * count;
                        *rewards.entry(CurrencyType::Copper).or_insert(0) += 2_000 * count;
                    }
                    ItemClass::ArmFragment | ItemClass::TreasureFragment => {
                        *rewards.entry(CurrencyType::Copper).or_insert(0) += 2_000 * count;
                    }
                    _ => {
                        let unit = default_sell_copper_per_unit(item.item_template_id, class);
                        *rewards.entry(CurrencyType::Copper).or_insert(0) += unit * count;
                    }
                }
            }

            let (_, delta) = self.remove_item_by_gid_with_delta(gid, item.item_num)?;
            bag_deltas.merge(delta);
        }

        for (&ct, &amt) in &rewards {
            if amt > 0 {
                self.grant_currency(ct, amt, now_ts);
            }
        }

        let (mut reward_json, _) = build_refinery_reward_views(&rewards, &BTreeMap::new());
        if let JsonValue::Object(ref mut m) = reward_json {
            m.insert("ret".into(), JsonValue::from("ok"));
        }

        Ok(RefinerySellOutcome {
            ret: "ok",
            rewards,
            reward_json,
            bag_deltas,
        })
    }

    /// Consume `count` items from `gid`, track `daily_use[tid]`, and apply `effect` (legacy `bag.useItem`).
    pub fn use_item(
        &mut self,
        gid: u32,
        count: u32,
        effect: UseItemEffect,
        now_ts: u64,
    ) -> Result<UseItemOutcome> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "use_item count must be > 0".into(),
            ));
        }
        let existing = self
            .inventory
            .get_by_gid(gid)
            .cloned()
            .ok_or_else(|| PlayerError::ItemNotFound(format!("no item at gid={gid}")))?;
        if existing.item_num < count {
            return Err(PlayerError::InsufficientItemCount {
                template_id: existing.item_template_id,
                available: u64::from(existing.item_num),
                required: u64::from(count),
            });
        }

        if let UseItemEffect::SummonHero { htid } = &effect {
            if gserver_data::is_protagonist_htid(*htid) {
                return Err(PlayerError::InvalidParameter(
                    "cannot summon additional protagonist template via item".into(),
                ));
            }
            let next_len = (self.heroes.len() as u32).saturating_add(count);
            if next_len > self.max_hero_capacity() {
                return Err(PlayerError::HeroBagFull {
                    max_heroes: self.max_hero_capacity(),
                });
            }
        }

        let after_remove = self.remove_item_by_gid(gid, count)?;
        let daily_entry = self
            .inventory
            .daily_use
            .entry(existing.item_template_id)
            .or_insert(0);
        *daily_entry = daily_entry.saturating_add(count);
        let daily_used_total = *daily_entry;

        let mut summoned_hids = Vec::new();
        let mut granted_items = Vec::new();

        match effect {
            UseItemEffect::NoOp => {}
            UseItemEffect::GrantCurrency {
                currency,
                amount_per_item,
            } => {
                let total = amount_per_item.saturating_mul(u64::from(count));
                self.grant_currency(currency, total, now_ts);
            }
            UseItemEffect::AddPlayerExp { exp_per_item } => {
                let total = exp_per_item.saturating_mul(u64::from(count));
                self.add_player_exp(total);
            }
            UseItemEffect::SummonHero { htid } => {
                for _ in 0..count {
                    summoned_hids.push(self.summon_hero(htid, 1)?);
                }
            }
            UseItemEffect::GrantItems { items_per_use } => {
                for seed in items_per_use {
                    let total_cnt = seed.count.saturating_mul(count);
                    if total_cnt > 0 {
                        let added = self.add_item(seed.item_template_id, total_cnt, now_ts)?;
                        granted_items.extend(added);
                    }
                }
            }
        }

        Ok(UseItemOutcome {
            consumed_template_id: existing.item_template_id,
            consumed_count: count,
            remaining_in_grid: after_remove.item_num,
            daily_used_total,
            summoned_hids,
            granted_items,
        })
    }

    /// Use an item from the bag by `item_id` or `gid` (`[tm]Exchange.useItemInBag`),
    /// handling stamina/energy pills (`22102` +25, `22103` +50, `22202` +10, `22203` +20),
    /// silver/gold/soul chests, direct items, choose-boxes (`option_idx`), and random giftboxes.
    pub fn use_item_in_bag(
        &mut self,
        item_id_or_gid: u64,
        count: u32,
        option_idx: Option<usize>,
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<UseBagItemOutcome> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "use_item_in_bag count must be > 0".into(),
            ));
        }
        let gid = self.resolve_bag_gid_by_id_or_gid(item_id_or_gid)?;
        let item = self.inventory.get_by_gid(gid).cloned().expect("resolved");
        if item.item_num < count {
            return Err(PlayerError::InsufficientItemCount {
                template_id: item.item_template_id,
                available: u64::from(item.item_num),
                required: u64::from(count),
            });
        }
        let plan = self.plan_bag_item_use(item.item_template_id, count, option_idx, data_store)?;
        let (_, mut bag_deltas) = self.remove_item_by_gid_with_delta(gid, count)?;
        self.apply_bag_item_use_plan(item.item_template_id, count, plan, &mut bag_deltas, now_ts)
    }

    /// Use `count` items by `template_id` across stacks (`[tm]Exchange.useItemInBagByTpl`).
    pub fn use_item_by_tpl(
        &mut self,
        template_id: u32,
        count: u32,
        option_idx: Option<usize>,
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<UseBagItemOutcome> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "use_item_by_tpl count must be > 0".into(),
            ));
        }
        if self.inventory.count_by_template(template_id) < u64::from(count) {
            return Err(PlayerError::InsufficientItemCount {
                template_id,
                available: self.inventory.count_by_template(template_id),
                required: u64::from(count),
            });
        }
        let plan = self.plan_bag_item_use(template_id, count, option_idx, data_store)?;
        let (_, mut bag_deltas) = self.remove_item_by_template_with_delta(template_id, count)?;
        self.apply_bag_item_use_plan(template_id, count, plan, &mut bag_deltas, now_ts)
    }

    #[allow(clippy::type_complexity)]
    fn plan_bag_item_use(
        &self,
        tid: u32,
        count: u32,
        option_idx: Option<usize>,
        data_store: Option<&gserver_data::GameDataStore>,
    ) -> Result<(Vec<(u32, u32, u64)>, Vec<(u32, u32)>)> {
        let (class, _) = classify_template_id(tid)?;
        let mut required_extra_items = Vec::new();

        // Check level & required paired key items when DB row exists
        if let Some(ds) = data_store {
            if let Some(row) = lookup_item_db_row(ds, tid) {
                let min_lv = parse_db_u64(row.get("use_level_min").or_else(|| row.get("level")))
                    .unwrap_or(1) as u32;
                if self.level < min_lv {
                    return Err(PlayerError::InvalidParameter(format!(
                        "player level {} < required level {min_lv} for item {tid}",
                        self.level
                    )));
                }
                if let Some(gserver_data::JsonValue::Array(req_arr)) = row.get("require_ids") {
                    for pair in req_arr {
                        if let gserver_data::JsonValue::Array(p) = pair {
                            if p.len() >= 2 {
                                let req_tid = parse_db_u64(p.first()).unwrap_or(0) as u32;
                                let req_cnt = parse_db_u64(p.get(1)).unwrap_or(0) as u32;
                                if req_tid > 0 && req_cnt > 0 {
                                    let need = req_cnt.saturating_mul(count);
                                    let have = self.inventory.count_by_template(req_tid);
                                    if have < u64::from(need) {
                                        return Err(PlayerError::InsufficientItemCount {
                                            template_id: req_tid,
                                            available: have,
                                            required: u64::from(need),
                                        });
                                    }
                                    required_extra_items.push((req_tid, need));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 1. Canonical stamina / energy pills & silver/gold/soul chests
        let count64 = u64::from(count);
        let built_in: Option<Vec<(u32, u32, u64)>> = match tid {
            22_101 => Some(vec![(14, 0, 10 * count64)]),
            22_102 => Some(vec![(14, 0, 25 * count64)]),
            22_103 => Some(vec![(14, 0, 50 * count64)]),
            22_201 => Some(vec![(18, 0, 5 * count64)]),
            22_202 => Some(vec![(18, 0, 10 * count64)]),
            22_203 => Some(vec![(18, 0, 20 * count64)]),
            22_301 => Some(vec![(1, 0, 100_000 * count64)]),
            22_401 => Some(vec![(2, 0, 100 * count64)]),
            22_501 => Some(vec![(15, 0, 200 * count64)]),
            _ => None,
        };
        if let Some(triplets) = built_in {
            return Ok((triplets, required_extra_items));
        }

        // 2. DB-driven `choose` or `reward` triplets
        if let Some(ds) = data_store {
            if let Some(row) = lookup_item_db_row(ds, tid) {
                let choose_list = parse_db_reward_triplets(row.get("choose"));
                if !choose_list.is_empty() {
                    let idx = option_idx.ok_or_else(|| {
                        PlayerError::InvalidParameter(format!(
                            "choose-box item {tid} requires option_idx"
                        ))
                    })?;
                    let &(pt, tpl, amt) = choose_list.get(idx).ok_or_else(|| {
                        PlayerError::InvalidParameter(format!(
                            "option_idx {idx} out of range (len={}) for item {tid}",
                            choose_list.len()
                        ))
                    })?;
                    return Ok((vec![(pt, tpl, amt.saturating_mul(count64))], required_extra_items));
                }
                let reward_list = parse_db_reward_triplets(row.get("reward"));
                if !reward_list.is_empty() {
                    let scaled = reward_list
                        .into_iter()
                        .map(|(pt, tpl, amt)| (pt, tpl, amt.saturating_mul(count64)))
                        .collect();
                    return Ok((scaled, required_extra_items));
                }
            }
        }

        // 3. Choose-box fallback (`option_idx` provided on `Direct` box e.g. `20005..20999`)
        if let Some(idx) = option_idx {
            let fallback_options: [(u32, u32, u64); 4] = [
                (4, 50_101, 10), // Wei Zhang Liao shards
                (4, 50_204, 10), // Shu Zhao Yun shards
                (4, 50_301, 10), // Wu Zhou Yu shards
                (4, 50_401, 10), // Qun Lu Bu shards
            ];
            let &(pt, tpl, amt) = fallback_options.get(idx).ok_or_else(|| {
                PlayerError::InvalidParameter(format!(
                    "choose-box option_idx={idx} out of bounds (0..4)"
                ))
            })?;
            return Ok((vec![(pt, tpl, amt.saturating_mul(count64))], required_extra_items));
        }

        // 4. Class-based deterministic rewards for Direct, RandGift, and Normal chests
        let triplets = match class {
            ItemClass::Direct => match tid {
                20_001 => vec![(1, 0, 10_000 * count64)],
                20_002 | 20_101 => vec![(2, 0, 100 * count64)],
                20_003 => vec![(15, 0, 200 * count64)],
                _ => vec![(1, 0, 25_000 * count64)],
            },
            ItemClass::RandGift => {
                // Deterministic pseudo-random table indexed by uid/revision/tid
                let seed = (self.uid ^ self.revision ^ u64::from(tid)) as usize;
                let pool: [(u32, u32, u64); 4] = [
                    (4, 10_010, 5),
                    (4, 10_012, 3),
                    (4, 50_101, 5),
                    (1, 0, 50_000),
                ];
                let (pt, tpl, amt) = pool[seed % pool.len()];
                vec![(pt, tpl, amt.saturating_mul(count64))]
            }
            ItemClass::Dress => {
                vec![(1, 0, 0)]
            }
            _ => {
                return Err(PlayerError::InvalidParameter(format!(
                    "item tid={tid} ({class:?}) cannot be directly used via useItemInBag"
                )))
            }
        };

        Ok((triplets, required_extra_items))
    }

    #[allow(clippy::type_complexity)]
    fn apply_bag_item_use_plan(
        &mut self,
        tid: u32,
        count: u32,
        plan: (Vec<(u32, u32, u64)>, Vec<(u32, u32)>),
        bag_deltas: &mut BagDelta,
        now_ts: u64,
    ) -> Result<UseBagItemOutcome> {
        let (triplets, extra_reqs) = plan;
        for (req_tid, req_cnt) in extra_reqs {
            let (_, d) = self.remove_item_by_template_with_delta(req_tid, req_cnt)?;
            bag_deltas.merge(d);
        }

        let daily_entry = self.inventory.daily_use.entry(tid).or_insert(0);
        *daily_entry = daily_entry.saturating_add(count);

        if let Ok((ItemClass::Dress, _)) = classify_template_id(tid) {
            self.dress_info.entry(tid).or_insert(1);
        }

        let mut drop_map: BTreeMap<String, JsonValue> = BTreeMap::new();
        let mut drop_items: BTreeMap<String, u64> = BTreeMap::new();
        let mut drop_treas_frags: BTreeMap<String, u64> = BTreeMap::new();
        let mut drop_heroes: BTreeMap<String, u64> = BTreeMap::new();
        let mut granted_items = Vec::new();
        let mut summoned_hids = Vec::new();

        for (ptype, tpl_id, amount) in triplets {
            if amount == 0 {
                continue;
            }
            if ptype == 4 && tpl_id > 0 {
                let add_cnt = u32::try_from(amount).unwrap_or(u32::MAX);
                let (added, d) = self.add_item_with_delta(tpl_id, add_cnt, now_ts)?;
                bag_deltas.merge(d);
                if matches!(
                    classify_template_id(tpl_id),
                    Ok((ItemClass::TreasureFragment, _))
                ) {
                    *drop_treas_frags.entry(tpl_id.to_string()).or_insert(0) += amount;
                }
                *drop_items.entry(tpl_id.to_string()).or_insert(0) += amount;
                granted_items.extend(added);
            } else if (ptype == 5 || ptype == 6) && tpl_id > 0 {
                for _ in 0..amount {
                    let hid = self.summon_hero(tpl_id, 1)?;
                    summoned_hids.push(hid);
                }
                *drop_heroes.entry(tpl_id.to_string()).or_insert(0) += amount;
            } else if ptype == 11 {
                self.add_player_exp(amount);
                let prev = drop_map
                    .get("master_exp")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0);
                drop_map.insert("master_exp".into(), JsonValue::from(prev + amount));
            } else if let Some(ct) = CurrencyType::from_price_type(ptype) {
                self.grant_currency(ct, amount, now_ts);
                let key = ct.vnsg2_key();
                let prev = drop_map.get(key).and_then(JsonValue::as_u64).unwrap_or(0);
                drop_map.insert(key.into(), JsonValue::from(prev + amount));
                if ct == CurrencyType::Copper {
                    drop_map.insert("silver".into(), JsonValue::from(prev + amount));
                }
            }
        }

        if !drop_items.is_empty() {
            let mut m = BTreeMap::new();
            for (k, v) in drop_items {
                m.insert(k, JsonValue::from(v));
            }
            drop_map.insert("item".into(), JsonValue::Object(m));
        }
        if !drop_treas_frags.is_empty() {
            let mut m = BTreeMap::new();
            for (k, v) in drop_treas_frags {
                m.insert(k, JsonValue::from(v));
            }
            drop_map.insert("treasure_fragment".into(), JsonValue::Object(m));
        }
        if !drop_heroes.is_empty() {
            let mut m = BTreeMap::new();
            for (k, v) in drop_heroes {
                m.insert(k, JsonValue::from(v));
            }
            drop_map.insert("hero".into(), JsonValue::Object(m));
        }

        self.bump_revision();
        Ok(UseBagItemOutcome {
            consumed_template_id: tid,
            consumed_count: count,
            drop: JsonValue::Object(drop_map),
            granted_items,
            summoned_hids,
            bag_deltas: bag_deltas.clone(),
        })
    }

    /// Return current OpenBox state (`[tm]Exchange.getOpenBoxInfo`), backed by `self.raffle_state["open_box"]`.
    pub fn get_open_box_info(&self) -> JsonValue {
        let mut out = BTreeMap::new();
        for default_tid in [33_001u32, 33_002, 33_003] {
            let mut entry = BTreeMap::new();
            entry.insert("open_times".into(), JsonValue::from(0u32));
            entry.insert("gold_open_times".into(), JsonValue::from(0u32));
            entry.insert("last_gold_open_time".into(), JsonValue::from(0u64));
            out.insert(default_tid.to_string(), JsonValue::Object(entry));
        }
        if let Some(JsonValue::Object(saved)) = self.raffle_state.get("open_box") {
            for (k, v) in saved {
                out.insert(k.clone(), v.clone());
            }
        }
        JsonValue::Object(out)
    }

    /// Open `num` random treasure/gift boxes by key (`[tm]Exchange.openRandgiftBoxByKey`).
    pub fn open_randgift_box_by_key(
        &mut self,
        chest_tid: u32,
        num: u32,
        now_ts: u64,
    ) -> Result<OpenBoxOutcome> {
        self.open_randgift_box(chest_tid, num, false, now_ts)
    }

    /// Open `num` random treasure/gift boxes by Gold (`[tm]Exchange.openRandgiftBoxByGold`).
    pub fn open_randgift_box_by_gold(
        &mut self,
        chest_tid: u32,
        num: u32,
        now_ts: u64,
    ) -> Result<OpenBoxOutcome> {
        self.open_randgift_box(chest_tid, num, true, now_ts)
    }

    /// Unified OpenBox mutation (`openRandgiftBoxByKey` / `openRandgiftBoxByGold`), persisting
    /// `open_times` & `gold_open_times` inside `self.raffle_state["open_box"]`.
    pub fn open_randgift_box(
        &mut self,
        chest_tid: u32,
        num: u32,
        by_gold: bool,
        now_ts: u64,
    ) -> Result<OpenBoxOutcome> {
        if num == 0 || num > 50 {
            return Err(PlayerError::InvalidParameter(
                "open_randgift_box num must be in 1..=50".into(),
            ));
        }

        let cur_info = self.get_open_box_info();
        let chest_key = chest_tid.to_string();
        let (mut open_times, mut gold_open_times) = cur_info
            .get(&chest_key)
            .and_then(JsonValue::as_object)
            .map(|m| {
                let ot = m.get("open_times").and_then(JsonValue::as_u64).unwrap_or(0) as u32;
                let gt = m
                    .get("gold_open_times")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0) as u32;
                (ot, gt)
            })
            .unwrap_or((0, 0));

        let mut bag_deltas = BagDelta::new();
        let mut gold_spent = 0u64;

        if by_gold {
            // Tiered `activity/box.json` `increase_array`:
            // 33003+ (gold chest): base 40, +20/step, cap 200; 33001/33002: base 20, +10/step, cap 100
            let (base, step, cap) = if chest_tid >= 33_003 {
                (40u64, 20u64, 200u64)
            } else {
                (20u64, 10u64, 100u64)
            };
            for i in 0..num {
                let idx = u64::from(gold_open_times + i);
                gold_spent += (base + idx * step).min(cap);
            }
            self.consume_currency(CurrencyType::Gold, gold_spent, now_ts)?;
            gold_open_times = gold_open_times.saturating_add(num);
        } else {
            // Consume paired key (`30011..30013` / `chest_tid + 3`) or the chest item itself from bag
            let paired_key = match chest_tid {
                33_001 => 30_011,
                33_002 => 30_012,
                33_003 => 30_013,
                30_001..=30_003 => chest_tid + 3,
                _ => chest_tid,
            };
            if self.inventory.count_by_template(chest_tid) >= u64::from(num)
                && self.inventory.count_by_template(paired_key) >= u64::from(num)
                && paired_key != chest_tid
            {
                let (_, d1) = self.remove_item_by_template_with_delta(chest_tid, num)?;
                let (_, d2) = self.remove_item_by_template_with_delta(paired_key, num)?;
                bag_deltas.merge(d1);
                bag_deltas.merge(d2);
            } else if self.inventory.count_by_template(paired_key) >= u64::from(num) {
                let (_, d) = self.remove_item_by_template_with_delta(paired_key, num)?;
                bag_deltas.merge(d);
            } else {
                let (_, d) = self.remove_item_by_template_with_delta(chest_tid, num)?;
                bag_deltas.merge(d);
            }
        }

        let mut drops = Vec::with_capacity(num as usize);
        let pool: [(u32, u32); 4] = [
            (10_010, 5),   // Tiến Cấp Đơn
            (10_012, 3),   // Tinh Luyện Thạch / EXP pill
            (3_100_001, 5), // Arm fragment
            (50_101, 2),   // Hero fragment
        ];
        for i in 0..num {
            let idx = ((self.uid as usize) + (open_times as usize) + (i as usize)) % pool.len();
            let (reward_tid, reward_cnt) = pool[idx];
            let (_, d) = self.add_item_with_delta(reward_tid, reward_cnt, now_ts)?;
            bag_deltas.merge(d);

            let mut item_m = BTreeMap::new();
            item_m.insert(reward_tid.to_string(), JsonValue::from(reward_cnt));
            let mut drop_obj = BTreeMap::new();
            drop_obj.insert("item".into(), JsonValue::Object(item_m));
            drops.push(JsonValue::Object(drop_obj));
        }

        open_times = open_times.saturating_add(num);
        let mut box_entry = BTreeMap::new();
        box_entry.insert("open_times".into(), JsonValue::from(open_times));
        box_entry.insert("gold_open_times".into(), JsonValue::from(gold_open_times));
        box_entry.insert(
            "last_gold_open_time".into(),
            JsonValue::from(if by_gold { now_ts } else { 0 }),
        );

        let mut ob_root = match self.raffle_state.remove("open_box") {
            Some(JsonValue::Object(m)) => m,
            _ => BTreeMap::new(),
        };
        ob_root.insert(chest_key, JsonValue::Object(box_entry));
        self.raffle_state
            .insert("open_box".into(), JsonValue::Object(ob_root));

        let open_box_info = self.get_open_box_info();
        self.bump_revision();

        Ok(OpenBoxOutcome {
            chest_tid,
            open_count: num,
            drops,
            gold_spent,
            bag_deltas,
            open_box_info,
        })
    }

    /// Expand a sub-bag's slot capacity using Gold (`bag.openGridByGold`), enforcing
    /// `buy_storage_step`, `buy_storage_limit`, and tiered `10/20/50/100/200` Gold pricing.
    pub fn open_bag_grid_by_gold(
        &mut self,
        bag_type: BagType,
        steps: u32,
        now_ts: u64,
    ) -> Result<BagExpandOutcome> {
        if steps == 0 {
            return Err(PlayerError::InvalidParameter(
                "open_bag_grid_by_gold steps must be > 0".into(),
            ));
        }
        let step_size = bag_type.buy_storage_step().ok_or_else(|| {
            PlayerError::InvalidParameter(format!(
                "sub-bag '{}' is an uncapped stack bag and cannot be expanded",
                bag_type.vnsg2_key()
            ))
        })?;
        let limit = bag_type.buy_storage_limit().unwrap_or(40);
        let cur_times = self
            .inventory
            .grid_open_times
            .get(&bag_type)
            .copied()
            .unwrap_or(0);
        if cur_times.saturating_add(steps) > limit {
            return Err(PlayerError::BagExpandLimitReached {
                bag_type: bag_type.vnsg2_key(),
                max_open_times: limit,
            });
        }

        let mut gold_cost = 0u64;
        for i in 1..=steps {
            gold_cost += BagType::buy_storage_gold_cost(cur_times + i);
        }

        self.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
        let added_grids = step_size.saturating_mul(steps);
        self.inventory.expand_bag_grids(bag_type, added_grids, steps);
        let new_max_grids = self.inventory.grid_max_num[&bag_type];
        let new_open_times = self.inventory.grid_open_times[&bag_type];
        self.bump_revision();

        Ok(BagExpandOutcome {
            bag_type,
            added_grids,
            new_max_grids,
            new_open_times,
            gold_spent: gold_cost,
            bag_deltas: BagDelta::new(),
        })
    }

    /// Expand a sub-bag's slot capacity by consuming bag expansion tokens (`bag.openGridByItem`, default `10028`).
    pub fn open_bag_grid_by_item(
        &mut self,
        bag_type: BagType,
        expand_item_tid: Option<u32>,
        steps: u32,
        _now_ts: u64,
    ) -> Result<BagExpandOutcome> {
        if steps == 0 {
            return Err(PlayerError::InvalidParameter(
                "open_bag_grid_by_item steps must be > 0".into(),
            ));
        }
        let step_size = bag_type.buy_storage_step().ok_or_else(|| {
            PlayerError::InvalidParameter(format!(
                "sub-bag '{}' is an uncapped stack bag and cannot be expanded",
                bag_type.vnsg2_key()
            ))
        })?;
        let limit = bag_type.buy_storage_limit().unwrap_or(40);
        let cur_times = self
            .inventory
            .grid_open_times
            .get(&bag_type)
            .copied()
            .unwrap_or(0);
        if cur_times.saturating_add(steps) > limit {
            return Err(PlayerError::BagExpandLimitReached {
                bag_type: bag_type.vnsg2_key(),
                max_open_times: limit,
            });
        }

        let token_tid = match expand_item_tid {
            Some(tid) => tid,
            None => {
                if self.inventory.count_by_template(10_028) >= u64::from(steps) {
                    10_028
                } else if self.inventory.count_by_template(10_029) >= u64::from(steps) {
                    10_029
                } else {
                    10_028
                }
            }
        };

        let (_, bag_deltas) = self.remove_item_by_template_with_delta(token_tid, steps)?;
        let added_grids = step_size.saturating_mul(steps);
        self.inventory.expand_bag_grids(bag_type, added_grids, steps);
        let new_max_grids = self.inventory.grid_max_num[&bag_type];
        let new_open_times = self.inventory.grid_open_times[&bag_type];
        self.bump_revision();

        Ok(BagExpandOutcome {
            bag_type,
            added_grids,
            new_max_grids,
            new_open_times,
            gold_spent: 0,
            bag_deltas,
        })
    }

    /// Expand Hero roster capacity (`user.openHeroBag`, row 1 of `bag_v2/bag.json`: +10 slots/purchase, limit 40).
    pub fn open_hero_bag_by_gold(&mut self, steps: u32, now_ts: u64) -> Result<(u32, u64)> {
        if steps == 0 {
            return Err(PlayerError::InvalidParameter(
                "open_hero_bag_by_gold steps must be > 0".into(),
            ));
        }
        const HERO_BAG_LIMIT: u32 = 40;
        if self.hero_bag_buy_times.saturating_add(steps) > HERO_BAG_LIMIT {
            return Err(PlayerError::BagExpandLimitReached {
                bag_type: "hero",
                max_open_times: HERO_BAG_LIMIT,
            });
        }
        let mut gold_cost = 0u64;
        for i in 1..=steps {
            gold_cost += BagType::buy_storage_gold_cost(self.hero_bag_buy_times + i);
        }
        self.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
        self.hero_bag_buy_times += steps;
        self.bump_revision();
        Ok((self.max_hero_capacity(), gold_cost))
    }

    // ========================================================================
    // 3. Furnace Decompose (`Lò Luyện`) & Reborn (`Trùng Sinh`)
    // ========================================================================

    /// Decompose unlocked, undeployed heroes or items (`[tm]Refinery.refinery*`, `resolveSpirit`, `soulSkill.refinery`),
    /// refunding base shop currencies + 100% invested silver/pills/stones and emitting `{}` deletion deltas.
    pub fn refinery_decompose(
        &mut self,
        kind: RefineryDecomposeKind,
        target_ids: &[u64],
        preview_only: bool,
        now_ts: u64,
    ) -> Result<RefineryOutcome> {
        if target_ids.is_empty() {
            return Err(PlayerError::InvalidParameter(
                "refinery_decompose requires at least one target".into(),
            ));
        }

        let mut seen = BTreeSet::new();
        for &id in target_ids {
            if !seen.insert(id) {
                return Err(PlayerError::InvalidParameter(format!(
                    "duplicate target id={id} in refinery_decompose"
                )));
            }
        }

        let mut reward_currencies: BTreeMap<CurrencyType, u64> = BTreeMap::new();
        let mut reward_items: BTreeMap<u32, u32> = BTreeMap::new();
        let mut removed_hids = Vec::new();
        let mut bag_deltas = BagDelta::new();

        if kind == RefineryDecomposeKind::Heroes {
            for &hid in target_ids {
                let hero = self.heroes.get(&hid).ok_or(PlayerError::HeroNotFound(hid))?;
                if hero.is_protagonist() {
                    return Err(PlayerError::ProtagonistCannotBeRemoved { hid });
                }
                if hero.is_lock != 0 {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "hero hid={hid} is locked"
                    )));
                }
                if self.formation.is_hero_deployed(hid) {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "hero hid={hid} is currently deployed in formation"
                    )));
                }

                let quality = gserver_data::hero_by_htid(hero.htid)
                    .map(|h| h.init_quality)
                    .unwrap_or(4);
                let base_soul = match quality {
                    6..=u8::MAX => 5_000u64,
                    5 => 2_500,
                    4 => 1_000,
                    _ => 250,
                };
                *reward_currencies.entry(CurrencyType::HeroSoul).or_insert(0) +=
                    base_soul + u64::from(hero.evolve_level) * (base_soul / 2);

                if hero.level > 1 || hero.exp > 0 {
                    let invested_exp = u64::from(hero.level - 1) * 1_000 + hero.exp;
                    *reward_currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(hero.level - 1) * 1_000;
                    for (pill_tid, cnt) in split_exp_into_pills(invested_exp) {
                        *reward_items.entry(pill_tid).or_insert(0) += cnt;
                    }
                }
                if hero.evolve_level > 0 {
                    *reward_items.entry(10_010).or_insert(0) += hero.evolve_level * 50;
                }
                if hero.awake_rank > 0 || hero.awake_level > 0 {
                    *reward_currencies
                        .entry(CurrencyType::AwakeArmCoin)
                        .or_insert(0) +=
                        u64::from(hero.awake_rank * 500 + hero.awake_level * 100);
                }
                removed_hids.push(hid);
            }

            if !preview_only {
                for &hid in &removed_hids {
                    self.heroes.remove(&hid);
                }
            }
        } else {
            let mut gids = Vec::with_capacity(target_ids.len());
            for &raw_id in target_ids {
                let gid = self.resolve_bag_gid_by_id_or_gid(raw_id)?;
                let item = self.inventory.get_by_gid(gid).expect("resolved");
                if item.is_locked() {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "item gid={gid} (item_id={}) is locked",
                        item.item_id
                    )));
                }
                if self.formation.is_item_equipped(item.item_id) {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "item item_id={} is currently equipped",
                        item.item_id
                    )));
                }
                let (class, _) = classify_template_id(item.item_template_id)?;
                let matches_kind = match kind {
                    RefineryDecomposeKind::Arm => class == ItemClass::Arm,
                    RefineryDecomposeKind::Treasure => class == ItemClass::Treasure,
                    RefineryDecomposeKind::Awake => class == ItemClass::Awake,
                    RefineryDecomposeKind::Pet => class == ItemClass::Pet,
                    RefineryDecomposeKind::Spirit | RefineryDecomposeKind::ResolveSpirit => {
                        class == ItemClass::Spirit
                    }
                    RefineryDecomposeKind::GodArm => class == ItemClass::GodArm,
                    RefineryDecomposeKind::GodAnimal => matches!(class, ItemClass::GodAnimal | ItemClass::TopAnimal),
                    RefineryDecomposeKind::Brochures => class == ItemClass::Brochure,
                    RefineryDecomposeKind::YanWu => class == ItemClass::YanWu,
                    RefineryDecomposeKind::SoulSkill => class == ItemClass::SoulSkill,
                    RefineryDecomposeKind::Heroes => false,
                };
                if !matches_kind {
                    return Err(PlayerError::InvalidParameter(format!(
                        "item tid={} ({class:?}) does not match decompose kind {kind:?}",
                        item.item_template_id
                    )));
                }
                gids.push(gid);
            }

            for gid in gids {
                let item = self.inventory.get_by_gid(gid).cloned().expect("verified");
                self.accumulate_item_refund(&item, true, &mut reward_currencies, &mut reward_items);
                if !preview_only {
                    let (_, d) = self.remove_item_by_gid_with_delta(gid, item.item_num)?;
                    bag_deltas.merge(d);
                }
            }
        }

        if !preview_only {
            for (&ct, &amt) in &reward_currencies {
                if amt > 0 {
                    self.grant_currency(ct, amt, now_ts);
                }
            }
            for (&tid, &cnt) in &reward_items {
                if cnt > 0 {
                    let (_, d) = self.add_item_with_delta(tid, cnt, now_ts)?;
                    bag_deltas.merge(d);
                }
            }
            self.recalculate_fight_force();
            self.bump_revision();
        }

        let (reward_dict, reward_triplets) =
            build_refinery_reward_views(&reward_currencies, &reward_items);

        Ok(RefineryOutcome {
            gold_spent: 0,
            reward_currencies,
            reward_items,
            reward_dict,
            reward_triplets,
            removed_hids,
            reset_hid: None,
            reset_item_id: None,
            bag_deltas,
        })
    }

    /// Reborn (`Trùng Sinh`) a hero, equipment, treasure, awake, spirit, pet, godarm, animal,
    /// brochure, soulStone, soulJade, rune, heroPill, or dress (`[tm]Refinery.reborn*`),
    /// deducting 50 Gold, resetting target to Lv.1 in-place, and refunding 100% invested resources!
    pub fn refinery_reborn(
        &mut self,
        kind: RefineryRebornKind,
        target_id: u64,
        keep_destiny: Option<bool>,
        preview_only: bool,
        now_ts: u64,
    ) -> Result<RefineryOutcome> {
        const REBORN_GOLD_COST: u64 = 50;

        if !preview_only && self.currency(CurrencyType::Gold) < REBORN_GOLD_COST {
            return Err(PlayerError::InsufficientCurrency {
                currency: CurrencyType::Gold.vnsg2_key(),
                available: self.currency(CurrencyType::Gold),
                required: REBORN_GOLD_COST,
            });
        }

        let mut reward_currencies: BTreeMap<CurrencyType, u64> = BTreeMap::new();
        let mut reward_items: BTreeMap<u32, u32> = BTreeMap::new();
        let mut reset_hid = None;
        let mut reset_item_id = None;
        let mut bag_deltas = BagDelta::new();

        match kind {
            RefineryRebornKind::Hero | RefineryRebornKind::OrangeHero => {
                let hero = self
                    .heroes
                    .get(&target_id)
                    .ok_or(PlayerError::HeroNotFound(target_id))?;
                if hero.is_protagonist() {
                    return Err(PlayerError::ProtagonistCannotBeRemoved { hid: target_id });
                }
                if hero.is_lock != 0 {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "hero hid={target_id} is locked"
                    )));
                }
                if self.formation.is_hero_deployed(target_id) {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "hero hid={target_id} is currently deployed in formation"
                    )));
                }
                let preserve_dest = keep_destiny.unwrap_or(false);
                let has_investment = hero.level > 1
                    || hero.exp > 0
                    || hero.evolve_level > 0
                    || hero.upgrade_level > 0
                    || hero.awake_rank > 0
                    || hero.awake_level > 0
                    || hero.hero_star_level > 1
                    || (!preserve_dest && hero.destiny > 0);
                if !has_investment {
                    return Err(PlayerError::InvalidParameter(format!(
                        "hero hid={target_id} is already Lv.1 with zero invested resources"
                    )));
                }

                if hero.level > 1 || hero.exp > 0 {
                    let invested_exp = u64::from(hero.level - 1) * 1_000 + hero.exp;
                    *reward_currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(hero.level - 1) * 1_000;
                    for (pill_tid, cnt) in split_exp_into_pills(invested_exp) {
                        *reward_items.entry(pill_tid).or_insert(0) += cnt;
                    }
                }
                if hero.evolve_level > 0 {
                    *reward_items.entry(10_010).or_insert(0) += hero.evolve_level * 50;
                    *reward_currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(hero.evolve_level) * 10_000;
                    // Refund duplicate copies as hero soul shards (`htid + 50_000`) when evolve >= 4
                    if hero.evolve_level >= 4 && (100..10_000).contains(&hero.htid) {
                        let frag_tid = 50_000 + (hero.htid % 10_000);
                        if classify_template_id(frag_tid).is_ok() {
                            let frag_per_copy = gserver_data::hero_by_htid(hero.htid)
                                .map(|h| u32::from(h.fragment_count))
                                .unwrap_or(80);
                            *reward_items.entry(frag_tid).or_insert(0) +=
                                (hero.evolve_level - 3) * frag_per_copy;
                        }
                    }
                }
                if hero.upgrade_level > 0 {
                    *reward_currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(hero.upgrade_level) * 20_000;
                }
                if hero.awake_rank > 0 || hero.awake_level > 0 {
                    *reward_currencies
                        .entry(CurrencyType::AwakeArmCoin)
                        .or_insert(0) +=
                        u64::from(hero.awake_rank * 500 + hero.awake_level * 100);
                }
                if !preserve_dest && hero.destiny > 0 {
                    *reward_items.entry(10_015).or_insert(0) += hero.destiny * 10;
                }

                if !preview_only {
                    self.consume_currency(CurrencyType::Gold, REBORN_GOLD_COST, now_ts)?;
                    let h = self.heroes.get_mut(&target_id).expect("verified");
                    h.level = 1;
                    h.exp = 0;
                    h.evolve_level = 0;
                    h.upgrade_level = 0;
                    h.awake_rank = 0;
                    h.awake_level = 0;
                    h.hero_star_level = 1;
                    h.hero_star_pos = 0;
                    if !preserve_dest {
                        h.destiny = 0;
                    }
                }
                reset_hid = Some(target_id);
            }
            RefineryRebornKind::HeroPill => {
                let hero = self
                    .heroes
                    .get(&target_id)
                    .ok_or(PlayerError::HeroNotFound(target_id))?;
                let pill_cnt = hero.hero_pill_rank * 5
                    + hero.hero_pill_level
                    + (hero.va_inlay.len() as u32);
                if pill_cnt == 0 {
                    return Err(PlayerError::InvalidParameter(
                        "hero has no inlaid pills to reborn".into(),
                    ));
                }
                *reward_items.entry(1_200_001).or_insert(0) += pill_cnt;
                *reward_currencies.entry(CurrencyType::Copper).or_insert(0) +=
                    u64::from(pill_cnt) * 5_000;
                if !preview_only {
                    self.consume_currency(CurrencyType::Gold, REBORN_GOLD_COST, now_ts)?;
                    let h = self.heroes.get_mut(&target_id).expect("verified");
                    h.hero_pill_rank = 0;
                    h.hero_pill_level = 0;
                    h.va_inlay.clear();
                }
                reset_hid = Some(target_id);
            }
            RefineryRebornKind::Dress => {
                // Can target either a bag Dress item OR an equipped/unlocked `dress_info` entry
                if let Ok(gid) = self.resolve_bag_gid_by_id_or_gid(target_id) {
                    let item = self.inventory.get_by_gid(gid).cloned().expect("resolved");
                    let lv = item
                        .va_item_text
                        .get("dress_level")
                        .or_else(|| item.va_item_text.get("reinforce_level"))
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(1) as u32;
                    if lv <= 1 {
                        return Err(PlayerError::InvalidParameter(
                            "dress item is already Lv.1".into(),
                        ));
                    }
                    *reward_items.entry(10_015).or_insert(0) += (lv - 1) * 5;
                    *reward_currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(lv - 1) * 10_000;
                    if !preview_only {
                        self.consume_currency(CurrencyType::Gold, REBORN_GOLD_COST, now_ts)?;
                        if let Some(it) = self.inventory.get_mut_by_gid(gid) {
                            it.va_item_text = default_va_item_text(ItemClass::Dress);
                            bag_deltas.record_item(it.clone());
                            reset_item_id = Some(it.item_id);
                        }
                    }
                } else {
                    let fid = u32::try_from(target_id).map_err(|_| {
                        PlayerError::ItemNotFound(format!("dress id={target_id} not found"))
                    })?;
                    let lv = self.dress_info.get(&fid).copied().ok_or_else(|| {
                        PlayerError::ItemNotFound(format!("dress {fid} not in dress_info"))
                    })?;
                    if lv <= 1 {
                        return Err(PlayerError::InvalidParameter(
                            "dress is already Lv.1".into(),
                        ));
                    }
                    *reward_items.entry(10_015).or_insert(0) += (lv - 1) * 5;
                    *reward_currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(lv - 1) * 10_000;
                    if !preview_only {
                        self.consume_currency(CurrencyType::Gold, REBORN_GOLD_COST, now_ts)?;
                        self.dress_info.insert(fid, 1);
                    }
                }
            }
            _ => {
                let gid = self.resolve_bag_gid_by_id_or_gid(target_id)?;
                let item = self.inventory.get_by_gid(gid).cloned().expect("resolved");
                if item.is_locked() {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "item item_id={} is locked",
                        item.item_id
                    )));
                }
                if self.formation.is_item_equipped(item.item_id) {
                    return Err(PlayerError::ResourceLockedOrDeployed(format!(
                        "item item_id={} is currently equipped",
                        item.item_id
                    )));
                }
                let (class, _) = classify_template_id(item.item_template_id)?;
                self.accumulate_item_refund(&item, false, &mut reward_currencies, &mut reward_items);
                if reward_currencies.is_empty() && reward_items.is_empty() {
                    return Err(PlayerError::InvalidParameter(format!(
                        "item item_id={} is already at base level with zero invested resources",
                        item.item_id
                    )));
                }
                reset_item_id = Some(item.item_id);
                if !preview_only {
                    self.consume_currency(CurrencyType::Gold, REBORN_GOLD_COST, now_ts)?;
                    if let Some(it) = self.inventory.get_mut_by_gid(gid) {
                        it.va_item_text = default_va_item_text(class);
                        bag_deltas.record_item(it.clone());
                    }
                }
            }
        }

        let gold_spent = if preview_only { 0 } else { REBORN_GOLD_COST };
        if !preview_only {
            for (&ct, &amt) in &reward_currencies {
                if amt > 0 {
                    self.grant_currency(ct, amt, now_ts);
                }
            }
            for (&tid, &cnt) in &reward_items {
                if cnt > 0 {
                    let (_, d) = self.add_item_with_delta(tid, cnt, now_ts)?;
                    bag_deltas.merge(d);
                }
            }
            self.recalculate_fight_force();
            self.bump_revision();
        }

        let (reward_dict, reward_triplets) =
            build_refinery_reward_views(&reward_currencies, &reward_items);

        Ok(RefineryOutcome {
            gold_spent,
            reward_currencies,
            reward_items,
            reward_dict,
            reward_triplets,
            removed_hids: Vec::new(),
            reset_hid,
            reset_item_id,
            bag_deltas,
        })
    }

    fn accumulate_item_refund(
        &self,
        item: &BagItem,
        include_base_melt_value: bool,
        currencies: &mut BTreeMap<CurrencyType, u64>,
        items: &mut BTreeMap<u32, u32>,
    ) {
        let Ok((class, _)) = classify_template_id(item.item_template_id) else {
            return;
        };

        let reinf_lv = item
            .va_item_text
            .get("reinforce_level")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1) as u32;
        let reinf_cost = item
            .va_item_text
            .get("reinforce_cost")
            .and_then(JsonValue::as_u64)
            .unwrap_or_else(|| u64::from(reinf_lv.saturating_sub(1)) * 2_000);
        let prog_lv = item
            .va_item_text
            .get("progress_level")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as u32;
        let prog_exp = item
            .va_item_text
            .get("progress_exp")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as u32;
        let up_lv = item
            .va_item_text
            .get("upgrade_level")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as u32;
        let star_lv = item
            .va_item_text
            .get("star_level")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as u32;
        let total_exp = item
            .va_item_text
            .get("total_exp")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);

        if reinf_cost > 0 {
            *currencies.entry(CurrencyType::Copper).or_insert(0) += reinf_cost;
        }

        match class {
            ItemClass::Arm => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        default_sell_copper_per_unit(item.item_template_id, class);
                    *items.entry(10_012).or_insert(0) += 2;
                }
                if prog_lv > 0 || prog_exp > 0 {
                    *items.entry(10_012).or_insert(0) += prog_lv * 5 + (prog_exp / 100).max(1);
                }
                if star_lv > 0 {
                    *currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(star_lv) * 10_000;
                }
            }
            ItemClass::Treasure => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::Copper).or_insert(0) += 15_000;
                }
                let reinf_exp = item
                    .va_item_text
                    .get("reinforce_exp")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0);
                if reinf_lv > 1 || reinf_exp > 0 {
                    *currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(reinf_lv.saturating_sub(1)) * 5_000 + reinf_exp * 10;
                }
                if prog_lv > 0 {
                    *items.entry(10_014).or_insert(0) += prog_lv * 10;
                }
            }
            ItemClass::Awake => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::AwakeArmCoin).or_insert(0) += 500;
                }
                if reinf_lv > 1 || prog_lv > 0 {
                    *currencies.entry(CurrencyType::AwakeArmCoin).or_insert(0) +=
                        u64::from(reinf_lv.saturating_sub(1)) * 100 + u64::from(prog_lv) * 300;
                    if prog_lv > 0 {
                        *items.entry(10_015).or_insert(0) += prog_lv * 5;
                    }
                }
            }
            ItemClass::Pet => {
                let eff_prog = prog_lv.saturating_sub(1);
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::PetCoin).or_insert(0) += 500;
                }
                if reinf_lv > 1 || eff_prog > 0 {
                    *currencies.entry(CurrencyType::PetCoin).or_insert(0) +=
                        u64::from(reinf_lv.saturating_sub(1)) * 100 + u64::from(eff_prog) * 300;
                }
            }
            ItemClass::Spirit => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::SpiritCoin).or_insert(0) += 200;
                }
                let exp_refund = total_exp.max(u64::from(reinf_lv.saturating_sub(1)) * 250);
                if exp_refund > 0 || prog_lv > 0 {
                    *currencies.entry(CurrencyType::SpiritCoin).or_insert(0) +=
                        exp_refund + u64::from(prog_lv) * 500;
                }
            }
            ItemClass::GodArm => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::GodarmCoin).or_insert(0) += 500;
                }
                if up_lv > 0 || prog_lv > 0 {
                    *currencies.entry(CurrencyType::GodarmCoin).or_insert(0) +=
                        u64::from(up_lv) * 150 + u64::from(prog_lv) * 400;
                }
            }
            ItemClass::GodAnimal | ItemClass::TopAnimal => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::GodanimalCoin).or_insert(0) += 500;
                }
                if reinf_lv > 1 || prog_lv > 0 || star_lv > 0 {
                    *currencies.entry(CurrencyType::GodanimalCoin).or_insert(0) +=
                        u64::from(reinf_lv.saturating_sub(1)) * 150
                            + u64::from(prog_lv + star_lv) * 400;
                }
            }
            ItemClass::Brochure => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::BrochureCoin).or_insert(0) += 300;
                }
                if reinf_lv > 0 || prog_lv > 0 {
                    *currencies.entry(CurrencyType::BrochureCoin).or_insert(0) +=
                        u64::from(reinf_lv + prog_lv) * 200;
                }
            }
            ItemClass::SoulStone | ItemClass::SoulJade => {
                let wash_num = item
                    .va_item_text
                    .get("wash_num")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0);
                if up_lv > 0 || star_lv > 0 || wash_num > 0 {
                    *currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(up_lv + star_lv) * 20_000;
                    *currencies
                        .entry(CurrencyType::HunshixilianCoin)
                        .or_insert(0) += wash_num * 30 + u64::from(up_lv) * 20;
                }
            }
            _ => {
                if include_base_melt_value {
                    *currencies.entry(CurrencyType::Copper).or_insert(0) += 5_000;
                }
                if reinf_lv > 1 || prog_lv > 0 || up_lv > 0 || star_lv > 0 {
                    *currencies.entry(CurrencyType::Copper).or_insert(0) +=
                        u64::from(reinf_lv.saturating_sub(1) + prog_lv + up_lv + star_lv) * 5_000;
                }
            }
        }
    }

    // ========================================================================
    // 4. Fragment Synthesis (`[tm]Exchange.*` & `emExchange.*Treasure`)
    // ========================================================================

    /// Synthesize `count` heroes from hero soul fragments (`[tm]Exchange.composeHero`),
    /// consulting `GameDataStore` / `gserver_data::HEROES` or quality-tier defaults (`40`/`80`/`100`/`120`/`160`).
    pub fn compose_hero_from_frag(
        &mut self,
        frag_tid: u32,
        count: u32,
        data_store: Option<&gserver_data::GameDataStore>,
        _now_ts: u64,
    ) -> Result<ComposeHeroOutcome> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "compose_hero_from_frag count must be > 0".into(),
            ));
        }
        let (class, _) = classify_template_id(frag_tid)?;
        if class != ItemClass::HeroFragment {
            return Err(PlayerError::InvalidParameter(format!(
                "tid={frag_tid} is {class:?}, expected HeroFragment (50001..=60000)"
            )));
        }

        // 1. Resolve `(htid, frag_need)` from DB row, curated `HEROES`, or `frag_tid - 50_000`
        let mut resolved: Option<(u32, u32)> = None;
        if let Some(ds) = data_store {
            if let Some(row) = lookup_item_db_row(ds, frag_tid) {
                let htid = parse_db_u64(
                    row.get("composit_hero_id")
                        .or_else(|| row.get("aimItem"))
                        .or_else(|| row.get("composit_id")),
                )
                .unwrap_or(0) as u32;
                let need = parse_db_u64(
                    row.get("need_part_num")
                        .or_else(|| row.get("frag_count"))
                        .or_else(|| row.get("maxStack"))
                        .or_else(|| row.get("top_limit")),
                )
                .unwrap_or(0) as u32;
                if htid > 0 && (1..=500).contains(&need) {
                    resolved = Some((htid, need));
                }
            }
        }
        if resolved.is_none() {
            if let Some(hero_tpl) = gserver_data::HEROES
                .iter()
                .find(|h| h.fragment_tid == frag_tid)
            {
                resolved = Some((hero_tpl.htid, u32::from(hero_tpl.fragment_count.max(1))));
            }
        }
        let (htid, frag_need) = resolved.unwrap_or_else(|| {
            let htid = frag_tid - 50_000;
            let default_need = match gserver_data::hero_by_htid(htid).map(|h| h.init_quality) {
                Some(6..=u8::MAX) => 120,
                Some(5) => 80,
                Some(4) => 40,
                _ => {
                    if htid < 400 {
                        80
                    } else {
                        40
                    }
                }
            };
            (htid, default_need)
        });

        if gserver_data::is_protagonist_htid(htid) || htid == 0 {
            return Err(PlayerError::InvalidParameter(format!(
                "invalid synthesized hero htid={htid} for fragment {frag_tid}"
            )));
        }
        if (self.heroes.len() as u32).saturating_add(count) > self.max_hero_capacity() {
            return Err(PlayerError::HeroBagFull {
                max_heroes: self.max_hero_capacity(),
            });
        }

        let total_frags = frag_need.saturating_mul(count);
        let (_, bag_deltas) = self.remove_item_by_template_with_delta(frag_tid, total_frags)?;

        let mut summoned_hids = Vec::with_capacity(count as usize);
        let mut heroes = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let hid = self.summon_hero(htid, 1)?;
            summoned_hids.push(hid);
            heroes.push(self.heroes[&hid].clone());
        }

        Ok(ComposeHeroOutcome {
            frag_tid,
            consumed_frags: total_frags,
            heroes,
            summoned_hids,
            bag_deltas,
        })
    }

    pub fn resolve_item_frag_recipe(
        frag_tid: u32,
        data_store: Option<&gserver_data::GameDataStore>,
    ) -> Result<(u32, u32)> {
        if let Some(ds) = data_store {
            if let Some(row) = lookup_item_db_row(ds, frag_tid) {
                let aim = parse_db_u64(
                    row.get("aimItem")
                        .or_else(|| row.get("composit_id"))
                        .or_else(|| row.get("composit_item_id"))
                        .or_else(|| row.get("composite_after")),
                )
                .unwrap_or(0) as u32;
                let need = parse_db_u64(
                    row.get("need_part_num")
                        .or_else(|| row.get("frag_count"))
                        .or_else(|| row.get("composite_number"))
                        .or_else(|| row.get("maxStack")),
                )
                .unwrap_or(0) as u32;
                if aim > 0 && need > 0 && classify_template_id(aim).is_ok() {
                    return Ok((aim, need));
                }
            }
        }

        let (class, _) = classify_template_id(frag_tid)?;
        let (aim, need) = match class {
            ItemClass::ArmFragment => {
                let offset = (frag_tid - 3_100_000).clamp(1, 79_999);
                (100_000 + offset, 20)
            }
            ItemClass::TreasureFragment => {
                let offset = if frag_tid >= 400_000_001 {
                    ((frag_tid - 400_000_000) % 19_999).max(1)
                } else {
                    (frag_tid - 3_200_000).clamp(1, 19_999)
                };
                (180_000 + offset, 20)
            }
            ItemClass::AwakeFragment => (300_000 + (frag_tid - 3_300_000), 20),
            ItemClass::PetFragment => (400_000 + (frag_tid - 3_400_000), 40),
            ItemClass::GodArmFragment => (500_000 + (frag_tid - 3_500_000), 40),
            ItemClass::GodAnimalFragment => (600_000 + (frag_tid - 3_600_000), 50),
            ItemClass::TopAnimalFragment => (5_400_000 + (frag_tid - 5_500_000), 50),
            ItemClass::HeroLegendFragment => (700_000 + (frag_tid - 3_700_000), 30),
            ItemClass::BrochureFragment => (800_000 + (frag_tid - 3_800_000), 30),
            ItemClass::YanWuFragment => (3_900_000 + (frag_tid - 4_000_000), 20),
            ItemClass::YanWuSoul if frag_tid < 4_200_000 => (frag_tid + 1, 3),
            ItemClass::CounsellorFragment => (4_300_000 + (frag_tid - 4_200_000), 50),
            ItemClass::CounsellorEquipFragment => (4_400_000 + (frag_tid - 4_500_000), 30),
            ItemClass::SoulStoneFragment => (4_600_000 + (frag_tid - 4_700_000), 40),
            ItemClass::SoulJadeFragment => (4_800_000 + (frag_tid - 4_900_000), 40),
            ItemClass::SoulSkillFragment => (1_400_000 + (frag_tid - 1_900_000), 40),
            ItemClass::Gem if frag_tid < 1_100_000 => (frag_tid + 1, 3),
            ItemClass::GodArmGem if frag_tid < 1_200_000 => (frag_tid + 1, 2),
            ItemClass::Normal if frag_tid < 20_000 => (frag_tid + 1, 5),
            _ => {
                return Err(PlayerError::InvalidParameter(format!(
                    "tid={frag_tid} ({class:?}) is not a synthesizable item fragment"
                )))
            }
        };
        classify_template_id(aim)?;
        Ok((aim, need))
    }

    /// Synthesize `count` equipment/treasure/awake/godarm/pet/gem/normal/animal items from fragments.
    pub fn compose_item_from_frag(
        &mut self,
        frag_tid: u32,
        count: u32,
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<ComposeItemOutcome> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "compose_item_from_frag count must be > 0".into(),
            ));
        }
        let (aim_tid, need_num) = Self::resolve_item_frag_recipe(frag_tid, data_store)?;
        let total_frags = need_num.saturating_mul(count);

        // Pre-verify target bag capacity for non-stackable gear BEFORE consuming fragments!
        let (_, aim_bag) = classify_template_id(aim_tid)?;
        if !aim_bag.is_stackable() {
            let max_grids = self
                .inventory
                .grid_max_num
                .get(&aim_bag)
                .copied()
                .unwrap_or_else(|| aim_bag.default_max_grids());
            let used = self
                .inventory
                .bags
                .get(&aim_bag)
                .map(|m| m.len() as u32)
                .unwrap_or(0);
            if used.saturating_add(count) > max_grids {
                return Err(PlayerError::BagFull {
                    bag_type: aim_bag.vnsg2_key(),
                    max_grids,
                });
            }
        }

        let (_, mut bag_deltas) = self.remove_item_by_template_with_delta(frag_tid, total_frags)?;
        let (created_items, add_delta) = self.add_item_with_delta(aim_tid, count, now_ts)?;
        bag_deltas.merge(add_delta);

        let mut consumed_fragments = BTreeMap::new();
        consumed_fragments.insert(frag_tid, total_frags);

        Ok(ComposeItemOutcome {
            created_items,
            consumed_fragments,
            bag_deltas,
        })
    }

    /// One-click fragment synthesis (`quickCompose*`) across `bag_filter` (or all fragment sub-bags).
    pub fn quick_compose_from_frag(
        &mut self,
        bag_filter: Option<BagType>,
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<ComposeItemOutcome> {
        let candidate_bags: Vec<BagType> = match bag_filter {
            Some(bt) => vec![bt],
            None => vec![
                BagType::Fragment,
                BagType::TreasureFragment,
                BagType::AwakeFragment,
                BagType::PetFragment,
                BagType::GodArmFragment,
                BagType::GodAnimalFragment,
                BagType::TopAnimalFragment,
                BagType::HeroLegendFragment,
                BagType::BrochureFragment,
                BagType::YanWuFragment,
                BagType::CounsellorFragment,
                BagType::CounsellorEquipFragment,
                BagType::SoulStoneFragment,
                BagType::SoulJadeFragment,
            ],
        };

        let mut frag_tids = BTreeSet::new();
        for bt in candidate_bags {
            if let Some(sub) = self.inventory.bags.get(&bt) {
                for item in sub.values() {
                    frag_tids.insert(item.item_template_id);
                }
            }
        }

        let mut created_items = Vec::new();
        let mut consumed_fragments = BTreeMap::new();
        let mut bag_deltas = BagDelta::new();

        for frag_tid in frag_tids {
            let Ok((aim_tid, need_num)) = Self::resolve_item_frag_recipe(frag_tid, data_store) else {
                continue;
            };
            let have = self.inventory.count_by_template(frag_tid) as u32;
            let mut max_make = have / need_num;
            if max_make == 0 {
                continue;
            }
            let (_, aim_bag) = classify_template_id(aim_tid)?;
            if !aim_bag.is_stackable() {
                let max_grids = self
                    .inventory
                    .grid_max_num
                    .get(&aim_bag)
                    .copied()
                    .unwrap_or_else(|| aim_bag.default_max_grids());
                let used = self
                    .inventory
                    .bags
                    .get(&aim_bag)
                    .map(|m| m.len() as u32)
                    .unwrap_or(0);
                max_make = max_make.min(max_grids.saturating_sub(used));
            }
            if max_make == 0 {
                continue;
            }
            let out = self.compose_item_from_frag(frag_tid, max_make, data_store, now_ts)?;
            created_items.extend(out.created_items);
            for (k, v) in out.consumed_fragments {
                *consumed_fragments.entry(k).or_insert(0) += v;
            }
            bag_deltas.merge(out.bag_deltas);
        }

        Ok(ComposeItemOutcome {
            created_items,
            consumed_fragments,
            bag_deltas,
        })
    }

    /// Resolve the Horse/Book treasure fragment recipe (`emExchange.composeTreasure`).
    /// Supports multi-part fragments (`3_200_000 + offset * 10 + 1..=parts` or `400_000_000 + treasure_tid * 10 + 1..=parts`,
    /// 3..=6 parts) as well as single-stack `3_200_000 + offset` shards (`20` shards).
    fn resolve_treasure_compose_parts(
        &self,
        treasure_tid: u32,
        _data_store: Option<&gserver_data::GameDataStore>,
    ) -> Result<Vec<(u32, u32)>> {
        let (class, _) = classify_template_id(treasure_tid)?;
        if class != ItemClass::Treasure {
            return Err(PlayerError::InvalidParameter(format!(
                "tid={treasure_tid} is {class:?}, expected Treasure (180001..=200000)"
            )));
        }

        let offset = treasure_tid - 180_000;
        // Check classic `400_000_000 + offset * 10 + part` or `3_200_000 + offset * 10 + part` multi-part sets in bag
        for base in [
            400_000_000u32.saturating_add(offset.saturating_mul(10)),
            3_200_000u32.saturating_add(offset.saturating_mul(10)),
        ] {
            let mut present_parts = Vec::new();
            for p in 1..=6u32 {
                let cand = base.saturating_add(p);
                if classify_template_id(cand).is_ok() && self.inventory.count_by_template(cand) > 0
                {
                    present_parts.push((cand, 1));
                }
            }
            // Horse/Book treasures use at least 3 distinct fragments (3 purple, 4/5 orange, 5/6 red)
            if present_parts.len() >= 3 {
                return Ok(present_parts);
            }
        }

        // Fallback: check direct `3_200_000 + offset` single shard stack (`20` shards) or standard 3-part recipe
        let direct_frag = 3_200_000 + offset;
        if self.inventory.count_by_template(direct_frag) > 0 {
            return Ok(vec![(direct_frag, 20)]);
        }

        // Default canonical 3-part Horse/Book fragment IDs so missing-fragment errors report exact TID
        let default_base = 3_200_000 + offset * 10;
        Ok(vec![
            (default_base + 1, 1),
            (default_base + 2, 1),
            (default_base + 3, 1),
        ])
    }

    /// Synthesize `count` Horse/Book treasures (`emExchange.composeTreasure`) by consuming required treasure fragments.
    pub fn compose_treasure(
        &mut self,
        treasure_tid: u32,
        count: u32,
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<ComposeItemOutcome> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "compose_treasure count must be > 0".into(),
            ));
        }
        let parts = self.resolve_treasure_compose_parts(treasure_tid, data_store)?;
        for &(frag_tid, per_unit) in &parts {
            let req = u64::from(per_unit.saturating_mul(count));
            let avail = self.inventory.count_by_template(frag_tid);
            if avail < req {
                return Err(PlayerError::InsufficientItemCount {
                    template_id: frag_tid,
                    available: avail,
                    required: req,
                });
            }
        }

        let max_grids = self
            .inventory
            .grid_max_num
            .get(&BagType::Treasure)
            .copied()
            .unwrap_or_else(|| BagType::Treasure.default_max_grids());
        let used = self
            .inventory
            .bags
            .get(&BagType::Treasure)
            .map(|m| m.len() as u32)
            .unwrap_or(0);
        if used.saturating_add(count) > max_grids {
            return Err(PlayerError::BagFull {
                bag_type: BagType::Treasure.vnsg2_key(),
                max_grids,
            });
        }

        let mut bag_deltas = BagDelta::new();
        let mut consumed_fragments = BTreeMap::new();
        for (frag_tid, per_unit) in parts {
            let take = per_unit * count;
            let (_, d) = self.remove_item_by_template_with_delta(frag_tid, take)?;
            bag_deltas.merge(d);
            *consumed_fragments.entry(frag_tid).or_insert(0) += take;
        }

        let (created_items, add_d) = self.add_item_with_delta(treasure_tid, count, now_ts)?;
        bag_deltas.merge(add_d);

        Ok(ComposeItemOutcome {
            created_items,
            consumed_fragments,
            bag_deltas,
        })
    }

    /// One-click synthesize all craftable copies of `treasure_tid` (`emExchange.oneKeycomposeTreasure`).
    pub fn one_key_compose_treasure(
        &mut self,
        treasure_tid: u32,
        data_store: Option<&gserver_data::GameDataStore>,
        now_ts: u64,
    ) -> Result<ComposeItemOutcome> {
        let parts = self.resolve_treasure_compose_parts(treasure_tid, data_store)?;
        let mut max_count = u32::MAX;
        for &(frag_tid, per_unit) in &parts {
            let avail = self.inventory.count_by_template(frag_tid) as u32;
            max_count = max_count.min(avail / per_unit.max(1));
        }
        let max_grids = self
            .inventory
            .grid_max_num
            .get(&BagType::Treasure)
            .copied()
            .unwrap_or_else(|| BagType::Treasure.default_max_grids());
        let used = self
            .inventory
            .bags
            .get(&BagType::Treasure)
            .map(|m| m.len() as u32)
            .unwrap_or(0);
        max_count = max_count.min(max_grids.saturating_sub(used));
        if max_count == 0 {
            let (first_frag, per_unit) = parts[0];
            return Err(PlayerError::InsufficientItemCount {
                template_id: first_frag,
                available: self.inventory.count_by_template(first_frag),
                required: u64::from(per_unit),
            });
        }
        self.compose_treasure(treasure_tid, max_count, data_store, now_ts)
    }

    // ========================================================================
    // 5. Squad (`Đội Hình`), Battle Grid (`{2,3,5,6,8,9}`), Partner, Buddy & Gear
    // ========================================================================

    fn ensure_unique_htid_for_deployment(&self, hid: u64, ignore_squad_slot: Option<u8>, ignore_partner_slot: Option<u8>, ignore_buddy_slot: Option<u8>) -> Result<()> {
        let target_htid = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?
            .htid;

        for (&s, &other_hid) in &self.formation.position {
            if Some(s) != ignore_squad_slot && other_hid != hid {
                if let Some(other) = self.heroes.get(&other_hid) {
                    if other.htid == target_htid {
                        return Err(PlayerError::DuplicateHeroTemplateInSquad { htid: target_htid });
                    }
                }
            }
        }
        for (&s, &p_hid) in &self.formation.partner {
            if Some(s) != ignore_partner_slot && p_hid != 0 && p_hid != hid {
                if let Some(other) = self.heroes.get(&p_hid) {
                    if other.htid == target_htid {
                        return Err(PlayerError::DuplicateHeroTemplateInSquad { htid: target_htid });
                    }
                }
            }
        }
        for (&s, &b_hid) in &self.formation.buddy {
            if Some(s) != ignore_buddy_slot && b_hid != 0 && b_hid != hid {
                if let Some(other) = self.heroes.get(&b_hid) {
                    if other.htid == target_htid {
                        return Err(PlayerError::DuplicateHeroTemplateInSquad { htid: target_htid });
                    }
                }
            }
        }
        Ok(())
    }

    /// Assign `hid` to active squad slot `1..=6` (`formation.setHeroToPosition`).
    pub fn set_hero_to_position(&mut self, slot: u8, hid: u64) -> Result<Option<u64>> {
        self.set_squad_slot(slot, hid)
    }

    /// Assign `hid` to active squad slot `1..=6` (`formation.setHeroToPosition`).
    /// Returns the previously deployed `hid` in that slot if replaced.
    pub fn set_squad_slot(&mut self, slot: u8, hid: u64) -> Result<Option<u64>> {
        if !(1..=6).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "squad slot must be in 1..=6, got {slot}"
            )));
        }
        if !self.heroes.contains_key(&hid) {
            return Err(PlayerError::HeroNotFound(hid));
        }

        if self.formation.partner.values().any(|&h| h == hid)
            || self.formation.buddy.values().any(|&h| h == hid)
        {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "hero hid={hid} is currently in partner/buddy and must be removed first"
            )));
        }

        if let Some(&old_hid) = self.formation.position.get(&slot) {
            if old_hid == hid {
                return Ok(Some(old_hid));
            }
            if old_hid == self.protagonist_hid
                && !self.formation.position.values().any(|&h| h == hid)
            {
                return Err(PlayerError::ProtagonistCannotBeRemoved { hid: old_hid });
            }
        }

        self.ensure_unique_htid_for_deployment(hid, Some(slot), None, None)?;

        if let Some(prev_slot) = self.formation.squad_slot_of(hid) {
            self.trans_hero(prev_slot, slot)?;
            return Ok(self.formation.position.get(&prev_slot).copied());
        }

        let prev_hid = self.formation.position.insert(slot, hid);
        if let Some(old_h) = prev_hid {
            // Preserve the exact battle grid cell of the replaced hero!
            for cell_hid in self.formation.grid_formation.values_mut() {
                if *cell_hid == old_h {
                    *cell_hid = hid;
                }
            }
        }
        self.formation.sync_grid_with_squad();
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(prev_hid)
    }

    /// Remove the hero in active squad slot `1..=6` (`formation.unsetHeroFromPosition`) AND
    /// return ALL slot-bound gear (`arming`, `spirit`, `yan_wu`, `brochure`, `soul_skill`,
    /// `soul_stone`, `soul_jade`) back to the player's inventory!
    pub fn unset_hero_from_position(&mut self, slot: u8) -> Result<(u64, BagDelta)> {
        if !(1..=6).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "squad slot must be in 1..=6, got {slot}"
            )));
        }
        let hid = self.formation.position.get(&slot).copied().ok_or_else(|| {
            PlayerError::InvalidFormationSlot(format!("squad slot {slot} is already empty"))
        })?;
        if hid == self.protagonist_hid {
            return Err(PlayerError::ProtagonistCannotBeRemoved { hid });
        }

        // Collect all slot-bound items so we can verify bag space before mutating
        let mut to_return: Vec<BagItem> = Vec::new();
        if let Some(m) = self.formation.arming.get(&slot) {
            to_return.extend(m.values().cloned());
        }
        if let Some(m) = self.formation.spirit.get(&slot) {
            to_return.extend(m.values().cloned());
        }
        if let Some(m) = self.formation.yan_wu.get(&slot) {
            to_return.extend(m.values().cloned());
        }
        if let Some(m) = self.formation.brochure.get(&slot) {
            to_return.extend(m.values().cloned());
        }
        if let Some(m) = self.formation.soul_skill.get(&slot) {
            to_return.extend(m.values().cloned());
        }
        if let Some(it) = self.formation.soul_stone.get(&slot) {
            to_return.push(it.clone());
        }
        if let Some(it) = self.formation.soul_jade.get(&slot) {
            to_return.push(it.clone());
        }

        // Pre-check bag capacity per sub-bag
        let mut needed_per_bag: BTreeMap<BagType, u32> = BTreeMap::new();
        for item in &to_return {
            let (_, bt) = classify_template_id(item.item_template_id)?;
            if !bt.is_stackable() {
                *needed_per_bag.entry(bt).or_insert(0) += 1;
            }
        }
        for (bt, need) in needed_per_bag {
            let max_grids = self
                .inventory
                .grid_max_num
                .get(&bt)
                .copied()
                .unwrap_or_else(|| bt.default_max_grids());
            let used = self
                .inventory
                .bags
                .get(&bt)
                .map(|m| m.len() as u32)
                .unwrap_or(0);
            if used.saturating_add(need) > max_grids {
                return Err(PlayerError::BagFull {
                    bag_type: bt.vnsg2_key(),
                    max_grids,
                });
            }
        }

        let mut delta = BagDelta::new();
        for item in to_return {
            let returned = self.inventory.insert_instance(item)?;
            delta.record_item(returned);
        }

        if let Some(m) = self.formation.arming.get_mut(&slot) {
            m.clear();
        }
        if let Some(m) = self.formation.spirit.get_mut(&slot) {
            m.clear();
        }
        if let Some(m) = self.formation.yan_wu.get_mut(&slot) {
            m.clear();
        }
        if let Some(m) = self.formation.brochure.get_mut(&slot) {
            m.clear();
        }
        if let Some(m) = self.formation.soul_skill.get_mut(&slot) {
            m.clear();
        }
        self.formation.soul_stone.remove(&slot);
        self.formation.soul_jade.remove(&slot);
        self.formation.attr_skins.remove(&slot);

        self.formation.position.remove(&slot);
        self.formation.sync_grid_with_squad();
        self.recalculate_fight_force();
        self.bump_revision();
        Ok((hid, delta))
    }

    /// Legacy wrapper around `unset_hero_from_position` returning `Result<u64>`.
    pub fn unset_squad_slot(&mut self, slot: u8) -> Result<u64> {
        let (hid, _) = self.unset_hero_from_position(slot)?;
        Ok(hid)
    }

    /// Swap two heroes between squad slots `slot_a` and `slot_b` (`formation.transHero`),
    /// keeping ALL slot-bound equipment (`arming`, `spirit`, etc.) anchored on their original slots!
    pub fn trans_hero(&mut self, slot_a: u8, slot_b: u8) -> Result<()> {
        if !(1..=6).contains(&slot_a) || !(1..=6).contains(&slot_b) {
            return Err(PlayerError::InvalidFormationSlot(
                "squad slots must be in 1..=6".into(),
            ));
        }
        if slot_a == slot_b {
            return Ok(());
        }
        let a = self.formation.position.remove(&slot_a);
        let b = self.formation.position.remove(&slot_b);
        if let Some(hid_a) = a {
            self.formation.position.insert(slot_b, hid_a);
        }
        if let Some(hid_b) = b {
            self.formation.position.insert(slot_a, hid_b);
        }
        // If both slots had heroes, swap their battle grid positions cleanly
        if let (Some(hid_a), Some(hid_b)) = (a, b) {
            for val in self.formation.grid_formation.values_mut() {
                if *val == hid_a {
                    *val = hid_b;
                } else if *val == hid_b {
                    *val = hid_a;
                }
            }
        }
        self.formation.sync_grid_with_squad();
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(())
    }

    /// Legacy alias for `trans_hero`.
    pub fn swap_squad_slots(&mut self, slot_a: u8, slot_b: u8) -> Result<()> {
        self.trans_hero(slot_a, slot_b)
    }

    /// Strictly set the 6-cell battle matrix (`{2, 3, 5, 6, 8, 9} -> hid` via `formation.set`).
    /// Rejects invisible cells `1, 4, 7` (`FormationPanel.lua:34`) with `InvalidFormationSlot`.
    pub fn set_battle_grid(&mut self, grid_map: BTreeMap<u8, u64>) -> Result<()> {
        let mut placed_hids = BTreeSet::new();
        for (&grid_pos, &hid) in &grid_map {
            if !is_valid_battle_grid_cell(grid_pos) {
                return Err(PlayerError::InvalidFormationSlot(format!(
                    "battle grid cell must be in {{2, 3, 5, 6, 8, 9}}, got {grid_pos}"
                )));
            }
            if hid == 0 {
                return Err(PlayerError::InvalidFormationSlot(
                    "battle grid map must omit empty cells instead of storing hid=0".into(),
                ));
            }
            if !placed_hids.insert(hid) {
                return Err(PlayerError::InvalidFormationSlot(format!(
                    "duplicate hid={hid} in battle grid"
                )));
            }
        }
        let active_hids: BTreeSet<u64> = self.formation.position.values().copied().collect();
        if placed_hids != active_hids {
            return Err(PlayerError::InvalidFormationSlot(
                "grid_formation must contain the exact set of active squad hids".into(),
            ));
        }
        self.formation.grid_formation = grid_map;
        self.bump_revision();
        Ok(())
    }

    /// Legacy `set_grid_formation`: accepts `1..=9` from older callers while automatically
    /// remapping invisible cells `1, 4, 7` onto free `{2, 3, 5, 6, 8, 9}` cells so `FormationPanel.lua`
    /// never receives invisible battle grid coordinates!
    pub fn set_grid_formation(&mut self, grid_map: BTreeMap<u8, u64>) -> Result<()> {
        let mut placed_hids = BTreeSet::new();
        for (&grid_pos, &hid) in &grid_map {
            if !(1..=9).contains(&grid_pos) {
                return Err(PlayerError::InvalidFormationSlot(format!(
                    "battle grid cell must be in 1..=9, got {grid_pos}"
                )));
            }
            if hid == 0 {
                return Err(PlayerError::InvalidFormationSlot(
                    "battle grid map must omit empty cells instead of storing hid=0".into(),
                ));
            }
            if !placed_hids.insert(hid) {
                return Err(PlayerError::InvalidFormationSlot(format!(
                    "duplicate hid={hid} in battle grid"
                )));
            }
        }
        let active_hids: BTreeSet<u64> = self.formation.position.values().copied().collect();
        if placed_hids != active_hids {
            return Err(PlayerError::InvalidFormationSlot(
                "grid_formation must contain the exact set of active squad hids".into(),
            ));
        }

        // Keep valid `{2, 3, 5, 6, 8, 9}` cells first, then remap any `1, 4, 7` onto free valid cells
        let mut sanitized = BTreeMap::new();
        let mut needs_remap = Vec::new();
        for (cell, hid) in grid_map {
            if is_valid_battle_grid_cell(cell) {
                sanitized.insert(cell, hid);
            } else {
                needs_remap.push((cell, hid));
            }
        }
        for (old_cell, hid) in needs_remap {
            let preferred = match old_cell {
                1 => 2,
                4 => 5,
                7 => 8,
                _ => 3,
            };
            if let std::collections::btree_map::Entry::Vacant(e) = sanitized.entry(preferred) {
                e.insert(hid);
            } else {
                for g in DEFAULT_SQUAD_TO_GRID {
                    if let std::collections::btree_map::Entry::Vacant(e) = sanitized.entry(g) {
                        e.insert(hid);
                        break;
                    }
                }
            }
        }
        self.formation.grid_formation = sanitized;
        self.bump_revision();
        Ok(())
    }

    /// Unlock a LittleFriend (`partner`) slot `1..=6` (`formation.openPartner`), deducting optional Gold.
    pub fn open_partner(&mut self, slot: u8, gold_cost: u64, now_ts: u64) -> Result<()> {
        if !(1..=12).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "partner slot must be in 1..=12, got {slot}"
            )));
        }
        if gold_cost > 0 {
            self.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
        }
        self.formation.partner.entry(slot).or_insert(0);
        self.bump_revision();
        Ok(())
    }

    /// Legacy wrapper around `open_partner(slot, 0, 0)`.
    pub fn open_partner_slot(&mut self, slot: u8) -> Result<()> {
        self.open_partner(slot, 0, 0)
    }

    /// Set or clear (`hid == 0`) a LittleFriend (`partner`) slot `1..=6` (`formation.setPartner`).
    pub fn set_partner(&mut self, slot: u8, hid: u64) -> Result<Option<u64>> {
        self.set_partner_slot(slot, hid)
    }

    /// Set or clear (`hid == 0`) a LittleFriend (`partner`) slot `1..=6` (`formation.setPartner`).
    pub fn set_partner_slot(&mut self, slot: u8, hid: u64) -> Result<Option<u64>> {
        if !(1..=12).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "partner slot must be in 1..=12, got {slot}"
            )));
        }
        if hid == 0 {
            let prev = self.formation.partner.insert(slot, 0);
            self.bump_revision();
            return Ok(prev);
        }
        let hero = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        if hero.is_protagonist() {
            return Err(PlayerError::ProtagonistCannotBeRemoved { hid });
        }
        if self.formation.position.values().any(|&h| h == hid)
            || self.formation.buddy.values().any(|&h| h == hid)
        {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "hero hid={hid} is already deployed in squad or buddy"
            )));
        }
        self.ensure_unique_htid_for_deployment(hid, None, Some(slot), None)?;
        let prev = self.formation.partner.insert(slot, hid);
        self.bump_revision();
        Ok(prev)
    }

    /// Clear multiple (or all when `None`) unlocked `partner` slots (`formation.unsetMultiPartner`).
    pub fn unset_multi_partner(&mut self, slots: Option<&[u8]>) -> Result<Vec<u64>> {
        let targets: Vec<u8> = match slots {
            Some(list) => list.to_vec(),
            None => self.formation.partner.keys().copied().collect(),
        };
        let mut removed = Vec::new();
        for s in targets {
            if !(1..=12).contains(&s) {
                return Err(PlayerError::InvalidFormationSlot(format!(
                    "partner slot must be in 1..=12, got {s}"
                )));
            }
            if let Some(val) = self.formation.partner.get_mut(&s) {
                if *val > 0 {
                    removed.push(*val);
                    *val = 0;
                }
            }
        }
        self.bump_revision();
        Ok(removed)
    }

    /// Unlock a `buddy` (助战军) slot `1..=8` (`formation.openBuddy`).
    pub fn open_buddy(&mut self, slot: u8, gold_cost: u64, now_ts: u64) -> Result<()> {
        if !(1..=8).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "buddy slot must be in 1..=8, got {slot}"
            )));
        }
        if gold_cost > 0 {
            self.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
        }
        self.formation.buddy.entry(slot).or_insert(0);
        self.formation.buddy_level_info.entry(slot).or_insert(1);
        self.bump_revision();
        Ok(())
    }

    /// Set or clear (`hid == 0`) a `buddy` slot `1..=8` (`formation.setBuddy`).
    pub fn set_buddy(&mut self, slot: u8, hid: u64) -> Result<Option<u64>> {
        if !(1..=8).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "buddy slot must be in 1..=8, got {slot}"
            )));
        }
        if hid == 0 {
            let prev = self.formation.buddy.insert(slot, 0);
            self.bump_revision();
            return Ok(prev);
        }
        let hero = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        if hero.is_protagonist() {
            return Err(PlayerError::ProtagonistCannotBeRemoved { hid });
        }
        if self.formation.position.values().any(|&h| h == hid)
            || self.formation.partner.values().any(|&h| h == hid)
        {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "hero hid={hid} is already deployed in squad or partner"
            )));
        }
        self.ensure_unique_htid_for_deployment(hid, None, None, Some(slot))?;
        let prev = self.formation.buddy.insert(slot, hid);
        self.formation.buddy_level_info.entry(slot).or_insert(1);
        self.bump_revision();
        Ok(prev)
    }

    /// Upgrade a `buddy` slot's level (`formation.buddyLvUp`).
    pub fn buddy_lv_up(&mut self, slot: u8, copper_cost: u64, now_ts: u64) -> Result<u32> {
        if !(1..=8).contains(&slot) || !self.formation.buddy.contains_key(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "buddy slot {slot} is locked or invalid"
            )));
        }
        if copper_cost > 0 {
            self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;
        }
        let entry = self.formation.buddy_level_info.entry(slot).or_insert(1);
        *entry = entry.saturating_add(1);
        let new_lv = *entry;
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(new_lv)
    }

    /// Reborn a `buddy` slot's level back to `1`, refunding invested copper (`formation.rebornBuddy`).
    pub fn reborn_buddy(&mut self, slot: u8, gold_cost: u64, now_ts: u64) -> Result<(u32, u64)> {
        if !(1..=8).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "buddy slot must be in 1..=8, got {slot}"
            )));
        }
        let cur_lv = self
            .formation
            .buddy_level_info
            .get(&slot)
            .copied()
            .unwrap_or(1);
        if cur_lv <= 1 {
            return Err(PlayerError::InvalidParameter(format!(
                "buddy slot {slot} is already Lv.1"
            )));
        }
        if gold_cost > 0 {
            self.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
        }
        let copper_refund = u64::from(cur_lv - 1) * 10_000;
        self.formation.buddy_level_info.insert(slot, 1);
        self.grant_currency(CurrencyType::Copper, copper_refund, now_ts);
        self.recalculate_fight_force();
        self.bump_revision();
        Ok((1, copper_refund))
    }

    /// Equip an `arming` item (`1..=9`) onto `squad_slot` (`1..=6`) by `item_id` or `gid`
    /// (`hero.addArming` / `formation.addArming`), returning `(prev_item_id, BagDelta)`.
    pub fn add_arming(
        &mut self,
        squad_slot: u8,
        equip_slot: EquipSlotType,
        item_id_or_gid: u64,
    ) -> Result<(Option<u64>, BagDelta)> {
        if !(1..=6).contains(&squad_slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "squad slot must be in 1..=6, got {squad_slot}"
            )));
        }
        let gid = self.resolve_bag_gid_by_id_or_gid(item_id_or_gid)?;
        let item = self.inventory.get_by_gid(gid).cloned().expect("resolved");
        equip_slot.validate_item(item.item_template_id)?;

        let (_, mut delta) = self.remove_item_by_gid_with_delta(gid, 1)?;
        let mut equipped_item = item;
        equipped_item.item_num = 1;
        equipped_item.gid = 0;

        let prev_equipped = self
            .formation
            .arming
            .entry(squad_slot)
            .or_default()
            .insert(equip_slot as u8, equipped_item);

        let prev_item_id = if let Some(old_item) = prev_equipped {
            let old_id = old_item.item_id;
            let returned = self.inventory.insert_instance(old_item)?;
            delta.record_item(returned);
            Some(old_id)
        } else {
            None
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok((prev_item_id, delta))
    }

    /// Batch-equip multiple `arming` slots on `squad_slot` (`hero.addMultiArming`).
    pub fn add_multi_arming(
        &mut self,
        squad_slot: u8,
        equips: &[(EquipSlotType, u64)],
    ) -> Result<(Vec<Option<u64>>, BagDelta)> {
        let mut prev_ids = Vec::with_capacity(equips.len());
        let mut total_delta = BagDelta::new();
        for &(slot_type, item_id_or_gid) in equips {
            let (prev_id, d) = self.add_arming(squad_slot, slot_type, item_id_or_gid)?;
            prev_ids.push(prev_id);
            total_delta.merge(d);
        }
        Ok((prev_ids, total_delta))
    }

    /// Unequip an `arming` slot back into the bag (`hero.removeArming`), returning `(BagItem, BagDelta)`.
    pub fn remove_arming(
        &mut self,
        squad_slot: u8,
        equip_slot: EquipSlotType,
    ) -> Result<(BagItem, BagDelta)> {
        let bag_item = self.unequip_item(squad_slot, equip_slot)?;
        let mut delta = BagDelta::new();
        delta.record_item(bag_item.clone());
        Ok((bag_item, delta))
    }

    /// Transfer/swap an `arming` slot directly between two squad slots (`formation.transferArming`).
    pub fn transfer_arming(
        &mut self,
        from_slot: u8,
        to_slot: u8,
        equip_slot: EquipSlotType,
    ) -> Result<()> {
        if !(1..=6).contains(&from_slot) || !(1..=6).contains(&to_slot) {
            return Err(PlayerError::InvalidFormationSlot(
                "squad slots must be in 1..=6".into(),
            ));
        }
        if from_slot == to_slot {
            return Ok(());
        }
        let key = equip_slot as u8;
        let from_item = self
            .formation
            .arming
            .get_mut(&from_slot)
            .and_then(|m| m.remove(&key))
            .ok_or_else(|| {
                PlayerError::ItemNotFound(format!(
                    "no item at from_slot={from_slot}, equip_slot={key}"
                ))
            })?;
        let to_item = self
            .formation
            .arming
            .get_mut(&to_slot)
            .and_then(|m| m.remove(&key));

        self.formation
            .arming
            .entry(to_slot)
            .or_default()
            .insert(key, from_item);
        if let Some(swapped) = to_item {
            self.formation
                .arming
                .entry(from_slot)
                .or_default()
                .insert(key, swapped);
        }
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(())
    }

    /// Legacy `equip_item` wrapper by `bag_gid`.
    pub fn equip_item(
        &mut self,
        squad_slot: u8,
        equip_slot: EquipSlotType,
        bag_gid: u32,
    ) -> Result<Option<BagItem>> {
        if !(1..=6).contains(&squad_slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "squad slot must be in 1..=6, got {squad_slot}"
            )));
        }
        let item = self
            .inventory
            .get_by_gid(bag_gid)
            .cloned()
            .ok_or_else(|| PlayerError::ItemNotFound(format!("no item at gid={bag_gid}")))?;
        equip_slot.validate_item(item.item_template_id)?;

        self.remove_item_by_gid(bag_gid, 1)?;
        let mut equipped_item = item;
        equipped_item.item_num = 1;
        equipped_item.gid = 0;

        let prev_equipped = self
            .formation
            .arming
            .entry(squad_slot)
            .or_default()
            .insert(equip_slot as u8, equipped_item);

        let returned = if let Some(old_item) = prev_equipped {
            Some(self.inventory.insert_instance(old_item)?)
        } else {
            None
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(returned)
    }

    /// Legacy `unequip_item` returning `BagItem`.
    pub fn unequip_item(&mut self, squad_slot: u8, equip_slot: EquipSlotType) -> Result<BagItem> {
        if !(1..=6).contains(&squad_slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "squad slot must be in 1..=6, got {squad_slot}"
            )));
        }
        let item = self
            .formation
            .arming
            .get(&squad_slot)
            .and_then(|m| m.get(&(equip_slot as u8)))
            .cloned()
            .ok_or_else(|| {
                PlayerError::ItemNotFound(format!(
                    "no item equipped at squad_slot={squad_slot}, equip_slot={}",
                    equip_slot as u8
                ))
            })?;

        let bag_item = self.inventory.insert_instance(item)?;
        if let Some(slot_map) = self.formation.arming.get_mut(&squad_slot) {
            slot_map.remove(&(equip_slot as u8));
        }

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(bag_item)
    }

    /// Equip a Spirit (`Chiến Hồn`) onto `(squad_slot, spirit_pos)` (`1..=8`).
    pub fn add_spirit(
        &mut self,
        squad_slot: u8,
        spirit_pos: u8,
        item_id_or_gid: u64,
    ) -> Result<(Option<u64>, BagDelta)> {
        if !(1..=6).contains(&squad_slot) || !(1..=8).contains(&spirit_pos) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "invalid spirit slot ({squad_slot}, {spirit_pos})"
            )));
        }
        let gid = self.resolve_bag_gid_by_id_or_gid(item_id_or_gid)?;
        let item = self.inventory.get_by_gid(gid).cloned().expect("resolved");
        let (class, _) = classify_template_id(item.item_template_id)?;
        if class != ItemClass::Spirit {
            return Err(PlayerError::EquipTypeMismatch {
                slot_type: spirit_pos,
                item_template_id: item.item_template_id,
            });
        }

        let (_, mut delta) = self.remove_item_by_gid_with_delta(gid, 1)?;
        let mut eq_spirit = item;
        eq_spirit.item_num = 1;
        eq_spirit.gid = 0;

        let prev = self
            .formation
            .spirit
            .entry(squad_slot)
            .or_default()
            .insert(spirit_pos, eq_spirit);

        let prev_id = if let Some(old_sp) = prev {
            let oid = old_sp.item_id;
            let ret = self.inventory.insert_instance(old_sp)?;
            delta.record_item(ret);
            Some(oid)
        } else {
            None
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok((prev_id, delta))
    }

    /// Batch-equip multiple Spirits onto `squad_slot` (`spirit.addMultiSpirit`).
    pub fn add_multi_spirit(
        &mut self,
        squad_slot: u8,
        spirits: &[(u8, u64)],
    ) -> Result<(Vec<Option<u64>>, BagDelta)> {
        let mut prev_ids = Vec::with_capacity(spirits.len());
        let mut total_delta = BagDelta::new();
        for &(pos, id_or_gid) in spirits {
            let (prev, d) = self.add_spirit(squad_slot, pos, id_or_gid)?;
            prev_ids.push(prev);
            total_delta.merge(d);
        }
        Ok((prev_ids, total_delta))
    }

    /// Unequip a Spirit from `(squad_slot, spirit_pos)` back to the `spirit` bag.
    pub fn remove_spirit(
        &mut self,
        squad_slot: u8,
        spirit_pos: u8,
    ) -> Result<(BagItem, BagDelta)> {
        if !(1..=6).contains(&squad_slot) || !(1..=8).contains(&spirit_pos) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "invalid spirit slot ({squad_slot}, {spirit_pos})"
            )));
        }
        let item = self
            .formation
            .spirit
            .get(&squad_slot)
            .and_then(|m| m.get(&spirit_pos))
            .cloned()
            .ok_or_else(|| {
                PlayerError::ItemNotFound(format!(
                    "no spirit equipped at ({squad_slot}, {spirit_pos})"
                ))
            })?;

        let returned = self.inventory.insert_instance(item)?;
        if let Some(m) = self.formation.spirit.get_mut(&squad_slot) {
            m.remove(&spirit_pos);
        }
        let mut delta = BagDelta::new();
        delta.record_item(returned.clone());
        self.recalculate_fight_force();
        self.bump_revision();
        Ok((returned, delta))
    }

    /// Transfer/swap a Spirit between `(from_slot, from_pos)` and `(to_slot, to_pos)`.
    pub fn transfer_spirit(
        &mut self,
        from_slot: u8,
        to_slot: u8,
        from_pos: u8,
        to_pos: u8,
    ) -> Result<()> {
        if !(1..=6).contains(&from_slot)
            || !(1..=6).contains(&to_slot)
            || !(1..=8).contains(&from_pos)
            || !(1..=8).contains(&to_pos)
        {
            return Err(PlayerError::InvalidFormationSlot(
                "invalid spirit transfer coordinates".into(),
            ));
        }
        if from_slot == to_slot && from_pos == to_pos {
            return Ok(());
        }
        let a = self
            .formation
            .spirit
            .get_mut(&from_slot)
            .and_then(|m| m.remove(&from_pos))
            .ok_or_else(|| {
                PlayerError::ItemNotFound(format!(
                    "no spirit at ({from_slot}, {from_pos})"
                ))
            })?;
        let b = self
            .formation
            .spirit
            .get_mut(&to_slot)
            .and_then(|m| m.remove(&to_pos));
        self.formation
            .spirit
            .entry(to_slot)
            .or_default()
            .insert(to_pos, a);
        if let Some(swapped) = b {
            self.formation
                .spirit
                .entry(from_slot)
                .or_default()
                .insert(from_pos, swapped);
        }
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(())
    }

    /// Equip a `GodAnimal` (`Thần Thú`) into formation `godanimal` slot `1..=6` (`godanimal.addGodAnimal`),
    /// storing the full `BagItem` wire dict so `HeroFormationData.lua:121` never sees a bare ID!
    pub fn add_god_animal(
        &mut self,
        slot: u8,
        item_id_or_gid: u64,
    ) -> Result<(Option<u64>, BagDelta)> {
        if !(1..=6).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "god_animal slot must be in 1..=6, got {slot}"
            )));
        }
        let gid = self.resolve_bag_gid_by_id_or_gid(item_id_or_gid)?;
        let item = self.inventory.get_by_gid(gid).cloned().expect("resolved");
        let (class, _) = classify_template_id(item.item_template_id)?;
        if class != ItemClass::GodAnimal {
            return Err(PlayerError::EquipTypeMismatch {
                slot_type: slot,
                item_template_id: item.item_template_id,
            });
        }

        let (_, mut delta) = self.remove_item_by_gid_with_delta(gid, 1)?;
        let mut eq_animal = item;
        eq_animal.item_num = 1;
        eq_animal.gid = 0;

        let prev = self
            .formation
            .godanimal
            .insert(slot, Some(eq_animal))
            .flatten();

        let prev_id = if let Some(old_an) = prev {
            let oid = old_an.item_id;
            let ret = self.inventory.insert_instance(old_an)?;
            delta.record_item(ret);
            Some(oid)
        } else {
            None
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok((prev_id, delta))
    }

    /// Unequip a `GodAnimal` from `slot` (`1..=6`) back to the `god_animal` bag, leaving the unlocked slot as `0` (`None`).
    pub fn remove_god_animal(&mut self, slot: u8) -> Result<(BagItem, BagDelta)> {
        if !(1..=6).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "god_animal slot must be in 1..=6, got {slot}"
            )));
        }
        let item = self
            .formation
            .godanimal
            .get(&slot)
            .and_then(|o| o.clone())
            .ok_or_else(|| {
                PlayerError::ItemNotFound(format!("no god_animal equipped in slot {slot}"))
            })?;

        let returned = self.inventory.insert_instance(item)?;
        self.formation.godanimal.insert(slot, None);
        let mut delta = BagDelta::new();
        delta.record_item(returned.clone());
        self.recalculate_fight_force();
        self.bump_revision();
        Ok((returned, delta))
    }

    // ========================================================================
    // 6. Hero, Arm, Treasure, Spirit, Awake & GodArm Progression
    // ========================================================================

    /// Summon a new hero instance (`HerosData` / `re.hero.new`), returning the new `hid`.
    pub fn summon_hero(&mut self, htid: u32, initial_level: u32) -> Result<u64> {
        if htid == 0 {
            return Err(PlayerError::InvalidParameter("htid must be > 0".into()));
        }
        if gserver_data::is_protagonist_htid(htid) {
            return Err(PlayerError::InvalidParameter(format!(
                "protagonist htid={htid} cannot be summoned via summon_hero"
            )));
        }
        if (self.heroes.len() as u32) >= self.max_hero_capacity() {
            return Err(PlayerError::HeroBagFull {
                max_heroes: self.max_hero_capacity(),
            });
        }
        let hid = self.next_hid;
        self.next_hid += 1;
        let hero = HeroInstance::new(hid, htid, initial_level.max(1));
        self.heroes.insert(hid, hero);
        self.bump_revision();
        Ok(hid)
    }

    /// Legacy `level_up_hero` (`hero.heroLevelUp`), deducting explicit `copper_cost` and `soul_cost`.
    pub fn level_up_hero(
        &mut self,
        hid: u64,
        levels: u32,
        copper_cost: u64,
        soul_cost: u64,
        now_ts: u64,
    ) -> Result<HeroLevelUpOutcome> {
        if levels == 0 {
            return Err(PlayerError::InvalidParameter("levels must be > 0".into()));
        }
        let hero = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        if hero.is_protagonist() {
            return Err(PlayerError::InvalidParameter(
                "protagonist level tracks PlayerProfile.level via add_player_exp".into(),
            ));
        }

        if self.currency(CurrencyType::Copper) < copper_cost {
            return Err(PlayerError::InsufficientCurrency {
                currency: CurrencyType::Copper.vnsg2_key(),
                available: self.currency(CurrencyType::Copper),
                required: copper_cost,
            });
        }
        if self.currency(CurrencyType::HeroSoul) < soul_cost {
            return Err(PlayerError::InsufficientCurrency {
                currency: CurrencyType::HeroSoul.vnsg2_key(),
                available: self.currency(CurrencyType::HeroSoul),
                required: soul_cost,
            });
        }

        if copper_cost > 0 {
            self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;
        }
        if soul_cost > 0 {
            self.consume_currency(CurrencyType::HeroSoul, soul_cost, now_ts)?;
        }

        let hero_mut = self.heroes.get_mut(&hid).expect("verified above");
        let old_level = hero_mut.level;
        hero_mut.level = hero_mut.level.saturating_add(levels);
        let new_level = hero_mut.level;
        let exp = hero_mut.exp;

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(HeroLevelUpOutcome {
            hid,
            old_level,
            new_level,
            exp,
        })
    }

    /// Authentic `vnsg2` hero level-up (`hero_level_up_vnsg2`), consuming bag EXP pills
    /// (`10011..=10014` and `17001..=17004`: `300 / 1_500 / 5_000 / 10_000` EXP) + silver (`copper`),
    /// capped at `self.level.max(20)` (or `210` max curve level), returning `{ newlv, old_lv, exp_delta, bag_deltas }`.
    pub fn hero_level_up_vnsg2(
        &mut self,
        hid: u64,
        add_lv_count: u32,
        now_ts: u64,
    ) -> Result<HeroLevelUpVnsg2Outcome> {
        if add_lv_count == 0 {
            return Err(PlayerError::InvalidParameter(
                "add_lv_count must be > 0".into(),
            ));
        }
        let hero = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        if hero.is_protagonist() {
            return Err(PlayerError::InvalidParameter(
                "protagonist level tracks PlayerProfile.level via add_player_exp".into(),
            ));
        }

        let old_lv = hero.level;
        let old_exp = hero.exp;
        let max_lv = self.level.max(old_lv + add_lv_count).min(210);
        let target_lv = old_lv.saturating_add(add_lv_count).min(max_lv);
        if target_lv <= old_lv {
            return Err(PlayerError::InvalidParameter(
                "hero has already reached the level cap".into(),
            ));
        }

        let curve_id = gserver_data::hero_by_htid(hero.htid)
            .map(|h| h.exp_curve_id)
            .unwrap_or(4);
        let needed_exp = if let Some(curve) = gserver_data::exp_curve_by_id(curve_id) {
            let cur_tot = curve.total_exp_for_level(old_lv).saturating_add(old_exp);
            let dst_tot = curve.total_exp_for_level(target_lv);
            dst_tot.saturating_sub(cur_tot).max(u64::from(target_lv - old_lv) * 300)
        } else {
            u64::from(target_lv - old_lv) * 600
        };

        // Gather available EXP pills (`10011..=10014`, `17001..=17004`) smallest-to-largest or greedy
        let pill_defs: [(u32, u64); 8] = [
            (10_014, 10_000),
            (17_004, 10_000),
            (10_013, 5_000),
            (17_003, 5_000),
            (10_012, 1_500),
            (17_002, 1_500),
            (10_011, 300),
            (17_001, 300),
        ];

        let mut total_avail_exp = 0u64;
        for &(ptid, pval) in &pill_defs {
            total_avail_exp = total_avail_exp
                .saturating_add(self.inventory.count_by_template(ptid).saturating_mul(pval));
        }
        if total_avail_exp < needed_exp {
            return Err(PlayerError::InsufficientItemCount {
                template_id: 10_011,
                available: total_avail_exp,
                required: needed_exp,
            });
        }

        let mut rem_exp = needed_exp;
        let mut to_consume: Vec<(u32, u32, u64)> = Vec::new();
        // First pass: exact/floor consumption from high-tier down
        for &(ptid, pval) in &pill_defs {
            if rem_exp == 0 {
                break;
            }
            let avail = self.inventory.count_by_template(ptid);
            if avail == 0 {
                continue;
            }
            let take = (rem_exp / pval).min(avail);
            if take > 0 {
                to_consume.push((ptid, take as u32, pval));
                rem_exp -= take * pval;
            }
        }
        // Second pass: if remainder > 0, top off with smallest available pill
        if rem_exp > 0 {
            for &(ptid, pval) in pill_defs.iter().rev() {
                let already_taken = to_consume
                    .iter()
                    .find(|&&(t, _, _)| t == ptid)
                    .map(|&(_, c, _)| u64::from(c))
                    .unwrap_or(0);
                let avail = self.inventory.count_by_template(ptid).saturating_sub(already_taken);
                if avail > 0 {
                    let take = rem_exp.div_ceil(pval).min(avail);
                    if let Some(entry) = to_consume.iter_mut().find(|e| e.0 == ptid) {
                        entry.1 += take as u32;
                    } else {
                        to_consume.push((ptid, take as u32, pval));
                    }
                    rem_exp = rem_exp.saturating_sub(take * pval);
                    if rem_exp == 0 {
                        break;
                    }
                }
            }
        }

        let total_exp_fed: u64 = to_consume
            .iter()
            .map(|&(_, c, pval)| u64::from(c) * pval)
            .sum();
        let copper_cost = total_exp_fed; // 1 copper per EXP point (`enhance_provide_cost_copper`)
        self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;

        let mut bag_deltas = BagDelta::new();
        let mut item_cost = BTreeMap::new();
        for (ptid, cnt, _) in to_consume {
            let (_, d) = self.remove_item_by_template_with_delta(ptid, cnt)?;
            bag_deltas.merge(d);
            *item_cost.entry(ptid).or_insert(0) += cnt;
        }

        let leftover_exp = total_exp_fed.saturating_sub(needed_exp);
        let hero_mut = self.heroes.get_mut(&hid).expect("verified");
        hero_mut.level = target_lv;
        hero_mut.exp = leftover_exp;

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(HeroLevelUpVnsg2Outcome {
            hid,
            old_lv,
            newlv: target_lv,
            exp: leftover_exp,
            exp_delta: total_exp_fed,
            copper_spent: copper_cost,
            item_cost,
            bag_deltas,
        })
    }

    /// Legacy `evolve_hero` (`evolve_level + 1`), consuming caller-supplied `sacrifice_hids`.
    pub fn evolve_hero(
        &mut self,
        hid: u64,
        sacrifice_hids: &[u64],
        copper_cost: u64,
        now_ts: u64,
    ) -> Result<u32> {
        if !self.heroes.contains_key(&hid) {
            return Err(PlayerError::HeroNotFound(hid));
        }

        let mut seen = BTreeSet::new();
        for &sac_hid in sacrifice_hids {
            if sac_hid == hid {
                return Err(PlayerError::InvalidParameter(
                    "cannot sacrifice target hero into itself".into(),
                ));
            }
            if !seen.insert(sac_hid) {
                return Err(PlayerError::InvalidParameter(format!(
                    "duplicate sacrifice hid={sac_hid}"
                )));
            }
            let sac = self
                .heroes
                .get(&sac_hid)
                .ok_or(PlayerError::HeroNotFound(sac_hid))?;
            if sac.is_protagonist() {
                return Err(PlayerError::ProtagonistCannotBeRemoved { hid: sac_hid });
            }
            if sac.is_lock != 0 {
                return Err(PlayerError::InvalidParameter(format!(
                    "locked hero hid={sac_hid} cannot be sacrificed"
                )));
            }
            if self.formation.is_hero_deployed(sac_hid) {
                return Err(PlayerError::InvalidParameter(format!(
                    "deployed hero hid={sac_hid} cannot be sacrificed"
                )));
            }
        }

        if copper_cost > 0 {
            self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;
        }

        for &sac_hid in sacrifice_hids {
            self.heroes.remove(&sac_hid);
        }

        let new_evolve = {
            let target = self.heroes.get_mut(&hid).expect("verified above");
            target.evolve_level = target.evolve_level.saturating_add(1);
            target.evolve_level
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(new_evolve)
    }

    /// Authentic `vnsg2` hero evolve (`evolve_hero_vnsg2`), auto-picking unlocked, undeployed
    /// duplicate heroes (`htid` match when `evolve_level >= 3`) + `10010` (`Tiến Cấp Đơn`) pills + silver,
    /// returning `{ new_evolve_lv, del_hero_hids, bag_deltas }`.
    pub fn evolve_hero_vnsg2(
        &mut self,
        hid: u64,
        one_key: bool,
        now_ts: u64,
    ) -> Result<HeroEvolveVnsg2Outcome> {
        let hero = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        let old_ev = hero.evolve_level;
        let target_htid = hero.htid;
        let is_protag = hero.is_protagonist();

        let mut cur_ev = old_ev;
        let mut del_hero_hids = Vec::new();
        let mut pills_used = 0u32;
        let mut copper_spent = 0u64;
        let mut bag_deltas = BagDelta::new();

        loop {
            if cur_ev >= 25 {
                break;
            }
            let step_pills = (cur_ev + 1) * 5;
            let step_copper = u64::from(cur_ev + 1) * 5_000;
            let need_dup = if !is_protag && cur_ev >= 3 { 1usize } else { 0usize };

            // Find unlocked, undeployed duplicate hero(es) if required
            let mut step_dups = Vec::new();
            if need_dup > 0 {
                for (&cand_hid, cand) in &self.heroes {
                    if cand_hid != hid
                        && !del_hero_hids.contains(&cand_hid)
                        && cand.htid == target_htid
                        && !cand.is_protagonist()
                        && cand.is_lock == 0
                        && !self.formation.is_hero_deployed(cand_hid)
                    {
                        step_dups.push(cand_hid);
                        if step_dups.len() == need_dup {
                            break;
                        }
                    }
                }
                if step_dups.len() < need_dup {
                    if cur_ev == old_ev {
                        return Err(PlayerError::InvalidParameter(format!(
                            "evolve_level {} -> {} requires {need_dup} unlocked undeployed duplicate hero(es) of htid={target_htid}",
                            cur_ev,
                            cur_ev + 1
                        )));
                    }
                    break;
                }
            }

            if self.inventory.count_by_template(10_010) < u64::from(step_pills) {
                if cur_ev == old_ev {
                    return Err(PlayerError::InsufficientItemCount {
                        template_id: 10_010,
                        available: self.inventory.count_by_template(10_010),
                        required: u64::from(step_pills),
                    });
                }
                break;
            }
            if self.currency(CurrencyType::Copper) < step_copper {
                if cur_ev == old_ev {
                    return Err(PlayerError::InsufficientCurrency {
                        currency: CurrencyType::Copper.vnsg2_key(),
                        available: self.currency(CurrencyType::Copper),
                        required: step_copper,
                    });
                }
                break;
            }

            self.consume_currency(CurrencyType::Copper, step_copper, now_ts)?;
            let (_, d) = self.remove_item_by_template_with_delta(10_010, step_pills)?;
            bag_deltas.merge(d);

            for dup_hid in step_dups {
                self.heroes.remove(&dup_hid);
                del_hero_hids.push(dup_hid);
            }

            cur_ev += 1;
            pills_used += step_pills;
            copper_spent += step_copper;

            if !one_key {
                break;
            }
        }

        if cur_ev == old_ev {
            return Err(PlayerError::InvalidParameter(
                "hero has reached maximum evolve_level".into(),
            ));
        }

        self.heroes.get_mut(&hid).expect("verified").evolve_level = cur_ev;
        self.recalculate_fight_force();
        self.bump_revision();

        Ok(HeroEvolveVnsg2Outcome {
            hid,
            old_evolve_lv: old_ev,
            new_evolve_lv: cur_ev,
            del_hero_hids,
            pills_used,
            copper_spent,
            bag_deltas,
        })
    }

    /// Legacy `upgrade_hero` (`hero.upgrade`: `upgrade_level + 1`).
    pub fn upgrade_hero(&mut self, hid: u64, copper_cost: u64, now_ts: u64) -> Result<u32> {
        if !self.heroes.contains_key(&hid) {
            return Err(PlayerError::HeroNotFound(hid));
        }
        if copper_cost > 0 {
            self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;
        }
        let next = {
            let h = self.heroes.get_mut(&hid).expect("verified above");
            h.upgrade_level = h.upgrade_level.saturating_add(1);
            h.upgrade_level
        };
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(next)
    }

    /// Quality promotion (`upgrade_hero_quality` / `evolve.surmount`): promotes `htid` along
    /// curated `red_evolved_htid -> gold_htid -> dark_gold_htid -> purple_gold_htid -> color_gold_htid`
    /// (or increments `upgrade_level` & `gold_rank`), returning the updated `htid`.
    pub fn upgrade_hero_quality(&mut self, hid: u64, now_ts: u64) -> Result<u32> {
        let hero = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        let cur_htid = hero.htid;
        let copper_cost = 50_000u64;
        self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;

        let h = self.heroes.get_mut(&hid).expect("verified");
        if h.is_protagonist() {
            if let Some(prot) = gserver_data::PROTAGONISTS
                .iter()
                .find(|p| p.quality_htids.contains(&cur_htid))
            {
                if let Some(pos) = prot.quality_htids.iter().position(|&id| id == cur_htid) {
                    if let Some(&next_htid) = prot.quality_htids.get(pos + 1) {
                        h.htid = next_htid;
                        self.utid = next_htid;
                    }
                }
            }
        } else if let Some(tpl) = gserver_data::hero_by_htid(cur_htid) {
            let next_htid = if cur_htid == tpl.htid && tpl.red_evolved_htid > 0 {
                tpl.red_evolved_htid
            } else if cur_htid == tpl.red_evolved_htid && tpl.gold_htid > 0 {
                h.gold_rank = h.gold_rank.max(1);
                tpl.gold_htid
            } else if cur_htid == tpl.gold_htid && tpl.dark_gold_htid > 0 {
                h.dark_gold_rank = h.dark_gold_rank.max(1);
                tpl.dark_gold_htid
            } else if cur_htid == tpl.dark_gold_htid && tpl.purple_gold_htid > 0 {
                h.purple_gold_rank = h.purple_gold_rank.max(1);
                tpl.purple_gold_htid
            } else if cur_htid == tpl.purple_gold_htid && tpl.color_gold_htid > 0 {
                h.color_gold_rank = h.color_gold_rank.max(1);
                tpl.color_gold_htid
            } else {
                cur_htid
            };
            h.htid = next_htid;
        }
        h.upgrade_level = h.upgrade_level.saturating_add(1);
        let new_htid = h.htid;

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(new_htid)
    }

    /// Awaken a hero (`hero.awake` / `awake_hero`), advancing `awake_level` (`0..9 -> +1 awake_rank`).
    /// Consumes `AwakeArmCoin` (`100`) + `Copper` (`10_000`) (or bag `10_015` awake stones if present).
    pub fn awake_hero(&mut self, hid: u64, now_ts: u64) -> Result<(u32, u32, BagDelta)> {
        if !self.heroes.contains_key(&hid) {
            return Err(PlayerError::HeroNotFound(hid));
        }
        let mut delta = BagDelta::new();
        if self.inventory.count_by_template(10_015) >= 5 {
            let (_, d) = self.remove_item_by_template_with_delta(10_015, 5)?;
            delta.merge(d);
        } else {
            self.consume_currency(CurrencyType::AwakeArmCoin, 100, now_ts)?;
        }
        self.consume_currency(CurrencyType::Copper, 10_000, now_ts)?;

        let h = self.heroes.get_mut(&hid).expect("verified");
        h.awake_level += 1;
        if h.awake_level >= 10 {
            h.awake_level = 0;
            h.awake_rank = h.awake_rank.saturating_add(1);
        } else if h.awake_rank == 0 {
            h.awake_rank = 1;
        }
        let res = (h.awake_rank, h.awake_level, delta);
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(res)
    }

    /// Lock or unlock a hero (`hero.lock` / `hero.unLock`).
    pub fn set_hero_lock(&mut self, hid: u64, locked: bool) -> Result<()> {
        let hero = self
            .heroes
            .get_mut(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        hero.is_lock = u8::from(locked);
        self.bump_revision();
        Ok(())
    }

    /// Advance hero star chart (`hero.upgradeStar` / `va_hero.star`), returning `(star_level, star_pos, BagDelta)`.
    pub fn upgrade_hero_star(
        &mut self,
        hid: u64,
        auto_steps: bool,
        now_ts: u64,
    ) -> Result<(u32, u32, BagDelta)> {
        if !self.heroes.contains_key(&hid) {
            return Err(PlayerError::HeroNotFound(hid));
        }
        let steps = if auto_steps { 6u32 } else { 1u32 };
        let copper_cost = u64::from(steps) * 5_000;
        self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;

        let mut delta = BagDelta::new();
        if self.inventory.count_by_template(10_016) >= u64::from(steps) {
            let (_, d) = self.remove_item_by_template_with_delta(10_016, steps)?;
            delta.merge(d);
        }

        let h = self.heroes.get_mut(&hid).expect("verified");
        for _ in 0..steps {
            h.hero_star_pos += 1;
            if h.hero_star_pos >= 6 {
                h.hero_star_pos = 0;
                h.hero_star_level = h.hero_star_level.max(1) + 1;
            }
        }
        let out = (h.hero_star_level.max(1), h.hero_star_pos, delta);
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(out)
    }

    /// Wash / cultivate hero attributes (`hero.wash` / `hero.replaceWash`), updating `va_hero.wash`.
    pub fn wash_hero(
        &mut self,
        hid: u64,
        times: u32,
        apply_replace: bool,
        use_gold: bool,
        now_ts: u64,
    ) -> Result<(JsonValue, BagDelta)> {
        if !self.heroes.contains_key(&hid) {
            return Err(PlayerError::HeroNotFound(hid));
        }
        let mut delta = BagDelta::new();

        if apply_replace {
            let h = self.heroes.get_mut(&hid).expect("verified");
            for (k, v) in h.va_last_wash.clone() {
                h.va_use_wash.insert(k, v);
            }
            h.va_last_wash.clear();
            let info = h.to_vnsg2_hero_info();
            let wash = info.get("va_hero").and_then(|v| v.get("wash")).cloned().unwrap_or_else(JsonValue::empty_object);
            self.recalculate_fight_force();
            self.bump_revision();
            return Ok((wash, delta));
        }

        let steps = times.clamp(1, 10);
        if use_gold {
            self.consume_currency(CurrencyType::Gold, u64::from(steps) * 10, now_ts)?;
        } else if self.inventory.count_by_template(10_017) >= u64::from(steps) {
            let (_, d) = self.remove_item_by_template_with_delta(10_017, steps)?;
            delta.merge(d);
        } else {
            self.consume_currency(CurrencyType::Copper, u64::from(steps) * 5_000, now_ts)?;
        }

        let h = self.heroes.get_mut(&hid).expect("verified");
        h.wash_times = h.wash_times.saturating_add(steps);
        let bonus_base = i64::from(steps) * 15;
        let mut cand = BTreeMap::new();
        cand.insert("1".into(), JsonValue::from(bonus_base * 5)); // HP
        cand.insert("9".into(), JsonValue::from(bonus_base));     // ATK
        cand.insert("4".into(), JsonValue::from(bonus_base / 2 + 1)); // PDEF
        cand.insert("5".into(), JsonValue::from(bonus_base / 2 + 1)); // MDEF
        h.va_last_wash = cand;

        let info = h.to_vnsg2_hero_info();
        let wash = info
            .get("va_hero")
            .and_then(|v| v.get("wash"))
            .cloned()
            .unwrap_or_else(JsonValue::empty_object);
        self.bump_revision();
        Ok((wash, delta))
    }

    /// Locate a mutable `BagItem` by `item_id` across EITHER equipped formation slots OR inventory sub-bags.
    /// Returns `(item_mut, is_in_bag)`.
    fn find_item_mut_anywhere(&mut self, item_id: u64) -> Result<(&mut BagItem, bool)> {
        for m in self.formation.arming.values_mut() {
            for it in m.values_mut() {
                if it.item_id == item_id {
                    return Ok((it, false));
                }
            }
        }
        for m in self.formation.spirit.values_mut() {
            for it in m.values_mut() {
                if it.item_id == item_id {
                    return Ok((it, false));
                }
            }
        }
        for it in self
            .formation
            .godanimal
            .values_mut()
            .chain(self.formation.topanimal.values_mut())
            .flatten()
        {
            if it.item_id == item_id {
                return Ok((it, false));
            }
        }
        if let Some((_, it)) = self.inventory.get_mut_by_item_id(item_id) {
            return Ok((it, true));
        }
        Err(PlayerError::ItemNotFound(format!(
            "no equipped or bag item with item_id={item_id}"
        )))
    }

    pub fn peek_item_anywhere(&self, item_id: u64) -> Result<(BagItem, bool)> {
        for m in self.formation.arming.values() {
            for it in m.values() {
                if it.item_id == item_id {
                    return Ok((it.clone(), false));
                }
            }
        }
        for m in self.formation.spirit.values() {
            for it in m.values() {
                if it.item_id == item_id {
                    return Ok((it.clone(), false));
                }
            }
        }
        for it in self
            .formation
            .godanimal
            .values()
            .chain(self.formation.topanimal.values())
            .flatten()
        {
            if it.item_id == item_id {
                return Ok((it.clone(), false));
            }
        }
        if let Some((_, it)) = self.inventory.get_by_item_id(item_id) {
            return Ok((it.clone(), true));
        }
        Err(PlayerError::ItemNotFound(format!(
            "no equipped or bag item with item_id={item_id}"
        )))
    }

    /// Reinforce an `Arm` item (`forge.reinforce` / `reinforce_arm_vnsg2`):
    /// `is_once = true` -> `+1` level; `is_once = false` -> `+5` levels, capped at `(player.level * 2).max(20)`.
    pub fn reinforce_arm_vnsg2(
        &mut self,
        item_id: u64,
        is_once: bool,
        now_ts: u64,
    ) -> Result<ArmReinforceOutcome> {
        let (item_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(item_snap.item_template_id)?;
        if class != ItemClass::Arm {
            return Err(PlayerError::InvalidParameter(format!(
                "reinforce_arm_vnsg2 requires Arm item, got {class:?}"
            )));
        }

        let cur_lv = item_snap
            .va_item_text
            .get("reinforce_level")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1) as u32;
        let cap = (self.level.saturating_mul(2)).max(20);
        if cur_lv >= cap {
            return Err(PlayerError::InvalidParameter(format!(
                "arm item_id={item_id} already at reinforce cap {cap}"
            )));
        }

        let want_add = if is_once { 1 } else { 5 };
        let up_level = want_add.min(cap - cur_lv);
        let mut cost_copper = 0u64;
        for step in 0..up_level {
            cost_copper += u64::from(cur_lv + step) * 500;
        }
        self.consume_currency(CurrencyType::Copper, cost_copper, now_ts)?;

        let new_lv = cur_lv + up_level;
        let mut bag_deltas = BagDelta::new();
        {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            let prev_cost = it_mut
                .va_item_text
                .get("reinforce_cost")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            it_mut
                .va_item_text
                .insert("reinforce_level".into(), JsonValue::from(new_lv));
            it_mut.va_item_text.insert(
                "reinforce_cost".into(),
                JsonValue::from(prev_cost + cost_copper),
            );
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
        }

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(ArmReinforceOutcome {
            item_id,
            level: new_lv,
            up_level,
            cost_copper,
            bag_deltas,
        })
    }

    /// Auto-reinforce all equipped `Arm` pieces on `squad_pos` (`forge.autoReinforceAll` / `auto_reinforce_arm_vnsg2`).
    ///
    /// **CRITICAL**: Reinforces **ONLY** `arming` sub-slots `1..=4` (`Weapon`, `Helmet`, `Armor`, `Belt`),
    /// NEVER touching sub-slots `5..=9` (`AttackTreasure`, `DefenseTreasure`, `Awake`, `Pet`, `GodArm`)!
    /// Returns `{ [item_id_str]: ArmReinforceStep { level, up_level, cost_copper } }`.
    pub fn auto_reinforce_arm_vnsg2(
        &mut self,
        squad_pos: u8,
        now_ts: u64,
    ) -> Result<BTreeMap<String, ArmReinforceStep>> {
        if !(1..=6).contains(&squad_pos) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "squad_pos must be in 1..=6, got {squad_pos}"
            )));
        }

        let cap = (self.level.saturating_mul(2)).max(20);
        let mut slot_items: Vec<(u8, u64, u32)> = Vec::new();
        if let Some(eq_map) = self.formation.arming.get(&squad_pos) {
            for sub_slot in 1..=4u8 {
                if let Some(it) = eq_map.get(&sub_slot) {
                    if matches!(classify_template_id(it.item_template_id), Ok((ItemClass::Arm, _))) {
                        let lv = it
                            .va_item_text
                            .get("reinforce_level")
                            .and_then(JsonValue::as_u64)
                            .unwrap_or(1) as u32;
                        slot_items.push((sub_slot, it.item_id, lv));
                    }
                }
            }
        }

        if slot_items.is_empty() {
            return Ok(BTreeMap::new());
        }

        // Balanced round-robin leveling across slots 1..=4 so equipment Master resonance stays synchronized!
        let mut levels: Vec<u32> = slot_items.iter().map(|x| x.2).collect();
        let mut costs: Vec<u64> = vec![0; slot_items.len()];
        let mut avail_copper = self.currency(CurrencyType::Copper);

        loop {
            // Pick the lowest-level arm below cap
            let mut min_idx = None;
            let mut min_lv = u32::MAX;
            for (idx, &lv) in levels.iter().enumerate() {
                if lv < cap && lv < min_lv {
                    min_lv = lv;
                    min_idx = Some(idx);
                }
            }
            let Some(idx) = min_idx else {
                break;
            };
            let step_cost = u64::from(levels[idx]) * 500;
            if avail_copper < step_cost {
                break;
            }
            avail_copper -= step_cost;
            levels[idx] += 1;
            costs[idx] += step_cost;
        }

        let total_spent: u64 = costs.iter().sum();
        if total_spent > 0 {
            self.consume_currency(CurrencyType::Copper, total_spent, now_ts)?;
        }

        let mut result = BTreeMap::new();
        if let Some(eq_map) = self.formation.arming.get_mut(&squad_pos) {
            for (idx, &(sub_slot, item_id, old_lv)) in slot_items.iter().enumerate() {
                let new_lv = levels[idx];
                let up_lv = new_lv.saturating_sub(old_lv);
                if up_lv > 0 {
                    if let Some(it) = eq_map.get_mut(&sub_slot) {
                        let prev_cost = it
                            .va_item_text
                            .get("reinforce_cost")
                            .and_then(JsonValue::as_u64)
                            .unwrap_or(0);
                        it.va_item_text
                            .insert("reinforce_level".into(), JsonValue::from(new_lv));
                        it.va_item_text.insert(
                            "reinforce_cost".into(),
                            JsonValue::from(prev_cost + costs[idx]),
                        );
                    }
                    result.insert(
                        item_id.to_string(),
                        ArmReinforceStep {
                            level: new_lv,
                            up_level: up_lv,
                            cost_copper: costs[idx],
                        },
                    );
                }
            }
        }

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(result)
    }

    /// Refine / upgrade an `Arm` item (`forge.upgrade` / `upgrade_arm_vnsg2`), consuming refine stones
    /// from `mat_map: { [stone_tid]: count }` (`100 exp` per stone, `500 exp` per `progress_level`),
    /// returning `{ level, exp, bag_deltas }`.
    pub fn upgrade_arm_vnsg2(
        &mut self,
        item_id: u64,
        mat_map: &BTreeMap<u32, u32>,
        _now_ts: u64,
    ) -> Result<ArmUpgradeOutcome> {
        if mat_map.is_empty() {
            return Err(PlayerError::InvalidParameter(
                "upgrade_arm_vnsg2 mat_map cannot be empty".into(),
            ));
        }
        let (item_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(item_snap.item_template_id)?;
        if class != ItemClass::Arm {
            return Err(PlayerError::InvalidParameter(format!(
                "upgrade_arm_vnsg2 requires Arm item, got {class:?}"
            )));
        }

        for (&tid, &cnt) in mat_map {
            if cnt == 0 {
                return Err(PlayerError::InvalidParameter(
                    "material count must be > 0".into(),
                ));
            }
            let avail = self.inventory.count_by_template(tid);
            if avail < u64::from(cnt) {
                return Err(PlayerError::InsufficientItemCount {
                    template_id: tid,
                    available: avail,
                    required: u64::from(cnt),
                });
            }
        }

        let mut bag_deltas = BagDelta::new();
        let mut added_exp = 0u32;
        for (&tid, &cnt) in mat_map {
            let (_, d) = self.remove_item_by_template_with_delta(tid, cnt)?;
            bag_deltas.merge(d);
            let stone_exp = match tid {
                10_014 => 500,
                10_013 => 250,
                _ => 100,
            };
            added_exp = added_exp.saturating_add(cnt.saturating_mul(stone_exp));
        }

        let (new_lv, new_exp) = {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            let mut lv = it_mut
                .va_item_text
                .get("progress_level")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32;
            let mut exp = it_mut
                .va_item_text
                .get("progress_exp")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32;
            exp = exp.saturating_add(added_exp);
            while exp >= 500 && lv < 100 {
                exp -= 500;
                lv += 1;
            }
            it_mut
                .va_item_text
                .insert("progress_level".into(), JsonValue::from(lv));
            it_mut
                .va_item_text
                .insert("progress_exp".into(), JsonValue::from(exp));
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
            (lv, exp)
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(ArmUpgradeOutcome {
            item_id,
            level: new_lv,
            exp: new_exp,
            bag_deltas,
        })
    }

    /// Wash / polish an `Arm` item (`forge.wash` / `arm_wash`), updating `va_item_text["wash"]`.
    pub fn arm_wash(
        &mut self,
        item_id: u64,
        apply_replace: bool,
        lock_mask: u32,
        use_gold: bool,
        now_ts: u64,
    ) -> Result<(JsonValue, BagDelta)> {
        let (item_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(item_snap.item_template_id)?;
        if class != ItemClass::Arm {
            return Err(PlayerError::InvalidParameter(format!(
                "arm_wash requires Arm item, got {class:?}"
            )));
        }

        let mut bag_deltas = BagDelta::new();
        if !apply_replace {
            let locked_lines = lock_mask.count_ones();
            if use_gold || locked_lines > 0 {
                self.consume_currency(
                    CurrencyType::Gold,
                    10 + u64::from(locked_lines) * 5,
                    now_ts,
                )?;
            } else if self.inventory.count_by_template(10_018) >= 2 {
                let (_, d) = self.remove_item_by_template_with_delta(10_018, 2)?;
                bag_deltas.merge(d);
            } else {
                self.consume_currency(CurrencyType::Copper, 10_000, now_ts)?;
            }
        }

        let wash_val = {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            let mut wash_obj = it_mut
                .va_item_text
                .get("wash")
                .and_then(JsonValue::as_object)
                .cloned()
                .unwrap_or_default();
            if apply_replace {
                if let Some(rep) = wash_obj.remove("replace") {
                    wash_obj.insert("attr".into(), rep);
                }
            } else {
                let mut rep = BTreeMap::new();
                rep.insert("1".into(), JsonValue::from(250u32));
                rep.insert("9".into(), JsonValue::from(60u32));
                wash_obj.insert("replace".into(), JsonValue::Object(rep));
            }
            let val = JsonValue::Object(wash_obj);
            it_mut.va_item_text.insert("wash".into(), val.clone());
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
            val
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok((wash_val, bag_deltas))
    }

    /// Reinforce a `Treasure` (`forge.reinforceTreasure` / `reinforce_treasure_vnsg2`) by consuming
    /// unlocked bag treasures / EXP treasures (`material_item_ids`) + copper.
    pub fn reinforce_treasure_vnsg2(
        &mut self,
        item_id: u64,
        material_item_ids: &[u64],
        now_ts: u64,
    ) -> Result<TreasureReinforceOutcome> {
        if material_item_ids.is_empty() {
            return Err(PlayerError::InvalidParameter(
                "reinforce_treasure_vnsg2 requires at least one material item".into(),
            ));
        }
        let (target_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(target_snap.item_template_id)?;
        if class != ItemClass::Treasure {
            return Err(PlayerError::InvalidParameter(format!(
                "reinforce_treasure_vnsg2 requires Treasure, got {class:?}"
            )));
        }

        let mut mat_gids = Vec::with_capacity(material_item_ids.len());
        let mut seen = BTreeSet::new();
        for &mid in material_item_ids {
            if mid == item_id {
                return Err(PlayerError::InvalidParameter(
                    "cannot consume target treasure into itself".into(),
                ));
            }
            let gid = self.resolve_bag_gid_by_id_or_gid(mid)?;
            if !seen.insert(gid) {
                return Err(PlayerError::InvalidParameter(format!(
                    "duplicate material gid={gid}"
                )));
            }
            let mat = self.inventory.get_by_gid(gid).expect("resolved");
            if mat.is_locked() || self.formation.is_item_equipped(mat.item_id) {
                return Err(PlayerError::ResourceLockedOrDeployed(format!(
                    "material item_id={} is locked or equipped",
                    mat.item_id
                )));
            }
            mat_gids.push(gid);
        }

        let copper_cost = (mat_gids.len() as u64) * 2_000;
        self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;

        let mut bag_deltas = BagDelta::new();
        let mut exp_gain = 0u32;
        for gid in mat_gids {
            let mat = self.inventory.get_by_gid(gid).cloned().expect("verified");
            let base_exp = mat
                .va_item_text
                .get("reinforce_exp")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32
                + 500;
            exp_gain = exp_gain.saturating_add(base_exp);
            let (_, d) = self.remove_item_by_gid_with_delta(gid, 1)?;
            bag_deltas.merge(d);
        }

        let (new_lv, new_exp) = {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            let mut lv = it_mut
                .va_item_text
                .get("reinforce_level")
                .and_then(JsonValue::as_u64)
                .unwrap_or(1) as u32;
            let mut exp = it_mut
                .va_item_text
                .get("reinforce_exp")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32;
            let prev_cost = it_mut
                .va_item_text
                .get("reinforce_cost")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            exp = exp.saturating_add(exp_gain);
            while exp >= 500 && lv < 100 {
                exp -= 500;
                lv += 1;
            }
            it_mut
                .va_item_text
                .insert("reinforce_level".into(), JsonValue::from(lv));
            it_mut
                .va_item_text
                .insert("reinforce_exp".into(), JsonValue::from(exp));
            it_mut.va_item_text.insert(
                "reinforce_cost".into(),
                JsonValue::from(prev_cost + copper_cost),
            );
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
            (lv, exp)
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(TreasureReinforceOutcome {
            item_id,
            level: new_lv,
            exp: new_exp,
            copper_spent: copper_cost,
            bag_deltas,
        })
    }

    /// Refine / evolve a `Treasure` (`forge.evolveTreasure` / `upgrade_treasure_vnsg2`),
    /// consuming treasure refine stones (`10_014`, 5 per step) + copper (`10_000` per step).
    pub fn upgrade_treasure_vnsg2(
        &mut self,
        item_id: u64,
        one_key: bool,
        now_ts: u64,
    ) -> Result<TreasureUpgradeOutcome> {
        let (target_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(target_snap.item_template_id)?;
        if class != ItemClass::Treasure {
            return Err(PlayerError::InvalidParameter(format!(
                "upgrade_treasure_vnsg2 requires Treasure, got {class:?}"
            )));
        }

        let max_steps = if one_key {
            let by_stone = (self.inventory.count_by_template(10_014) / 5) as u32;
            let by_copper = (self.currency(CurrencyType::Copper) / 10_000) as u32;
            by_stone.min(by_copper).min(20)
        } else {
            1
        };
        if max_steps == 0 || self.inventory.count_by_template(10_014) < 5 {
            return Err(PlayerError::InsufficientItemCount {
                template_id: 10_014,
                available: self.inventory.count_by_template(10_014),
                required: 5,
            });
        }
        let copper_cost = u64::from(max_steps) * 10_000;
        self.consume_currency(CurrencyType::Copper, copper_cost, now_ts)?;
        let (_, mut bag_deltas) =
            self.remove_item_by_template_with_delta(10_014, max_steps * 5)?;

        let prog_lv = {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            let next = it_mut
                .va_item_text
                .get("progress_level")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32
                + max_steps;
            it_mut
                .va_item_text
                .insert("progress_level".into(), JsonValue::from(next));
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
            next
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(TreasureUpgradeOutcome {
            item_id,
            progress_level: prog_lv,
            copper_spent: copper_cost,
            bag_deltas,
        })
    }

    /// Reinforce a `Spirit` (`Chiến Hồn` — `spirit.upgrade` / `reinforce_spirit_vnsg2`),
    /// consuming fodder Spirits (`material_item_ids`) and/or `SpiritCoin` (`spirit_coin_exp`),
    /// updating `reinforce_level`, `reinforce_exp`, and `total_exp`.
    pub fn reinforce_spirit_vnsg2(
        &mut self,
        item_id: u64,
        material_item_ids: &[u64],
        spirit_coin_exp: u64,
        now_ts: u64,
    ) -> Result<SpiritReinforceOutcome> {
        if material_item_ids.is_empty() && spirit_coin_exp == 0 {
            return Err(PlayerError::InvalidParameter(
                "reinforce_spirit_vnsg2 requires material spirits or spirit_coin_exp > 0".into(),
            ));
        }
        let (target_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(target_snap.item_template_id)?;
        if class != ItemClass::Spirit {
            return Err(PlayerError::InvalidParameter(format!(
                "reinforce_spirit_vnsg2 requires Spirit, got {class:?}"
            )));
        }

        let mut mat_gids = Vec::with_capacity(material_item_ids.len());
        let mut seen = BTreeSet::new();
        for &mid in material_item_ids {
            if mid == item_id {
                return Err(PlayerError::InvalidParameter(
                    "cannot consume target spirit into itself".into(),
                ));
            }
            let gid = self.resolve_bag_gid_by_id_or_gid(mid)?;
            if !seen.insert(gid) {
                return Err(PlayerError::InvalidParameter(format!(
                    "duplicate spirit material gid={gid}"
                )));
            }
            let mat = self.inventory.get_by_gid(gid).expect("resolved");
            if mat.is_locked() || self.formation.is_item_equipped(mat.item_id) {
                return Err(PlayerError::ResourceLockedOrDeployed(format!(
                    "spirit material item_id={} is locked or equipped",
                    mat.item_id
                )));
            }
            mat_gids.push(gid);
        }

        if spirit_coin_exp > 0 {
            self.consume_currency(CurrencyType::SpiritCoin, spirit_coin_exp, now_ts)?;
        }

        let mut bag_deltas = BagDelta::new();
        let mut exp_gain = u32::try_from(spirit_coin_exp).unwrap_or(u32::MAX);
        for gid in mat_gids {
            let mat = self.inventory.get_by_gid(gid).cloned().expect("verified");
            let fodder_exp = mat
                .va_item_text
                .get("total_exp")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32
                + 300;
            exp_gain = exp_gain.saturating_add(fodder_exp);
            let (_, d) = self.remove_item_by_gid_with_delta(gid, 1)?;
            bag_deltas.merge(d);
        }

        let (new_lv, new_exp, new_total) = {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            let mut lv = it_mut
                .va_item_text
                .get("reinforce_level")
                .and_then(JsonValue::as_u64)
                .unwrap_or(1) as u32;
            let mut exp = it_mut
                .va_item_text
                .get("reinforce_exp")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32;
            let total = (it_mut
                .va_item_text
                .get("total_exp")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32)
                .saturating_add(exp_gain);
            exp = exp.saturating_add(exp_gain);
            while exp >= 300 && lv < 60 {
                exp -= 300;
                lv += 1;
            }
            it_mut
                .va_item_text
                .insert("reinforce_level".into(), JsonValue::from(lv));
            it_mut
                .va_item_text
                .insert("reinforce_exp".into(), JsonValue::from(exp));
            it_mut
                .va_item_text
                .insert("total_exp".into(), JsonValue::from(total));
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
            (lv, exp, total)
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(SpiritReinforceOutcome {
            item_id,
            level: new_lv,
            exp: new_exp,
            total_exp: new_total,
            bag_deltas,
        })
    }

    /// Upgrade an `Awake` equipment piece (`awake.upgrade` / `upgrade_awake_vnsg2`),
    /// consuming `AwakeArmCoin` (`100` per step) + `Copper` (`5_000` per step).
    pub fn upgrade_awake_vnsg2(
        &mut self,
        item_id: u64,
        one_key: bool,
        now_ts: u64,
    ) -> Result<AwakeUpgradeOutcome> {
        let (target_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(target_snap.item_template_id)?;
        if class != ItemClass::Awake {
            return Err(PlayerError::InvalidParameter(format!(
                "upgrade_awake_vnsg2 requires Awake item, got {class:?}"
            )));
        }
        let steps = if one_key {
            let by_coin = (self.currency(CurrencyType::AwakeArmCoin) / 100) as u32;
            let by_cop = (self.currency(CurrencyType::Copper) / 5_000) as u32;
            by_coin.min(by_cop).clamp(1, 5)
        } else {
            1
        };
        self.consume_currency(CurrencyType::AwakeArmCoin, u64::from(steps) * 100, now_ts)?;
        self.consume_currency(CurrencyType::Copper, u64::from(steps) * 5_000, now_ts)?;

        let mut bag_deltas = BagDelta::new();
        let (reinf_lv, prog_lv) = {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            let mut r_lv = it_mut
                .va_item_text
                .get("reinforce_level")
                .and_then(JsonValue::as_u64)
                .unwrap_or(1) as u32;
            let mut p_lv = it_mut
                .va_item_text
                .get("progress_level")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as u32;
            r_lv += steps;
            if r_lv >= (p_lv + 1) * 5 {
                p_lv += 1;
            }
            it_mut
                .va_item_text
                .insert("reinforce_level".into(), JsonValue::from(r_lv));
            it_mut
                .va_item_text
                .insert("progress_level".into(), JsonValue::from(p_lv));
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
            (r_lv, p_lv)
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(AwakeUpgradeOutcome {
            item_id,
            reinforce_level: reinf_lv,
            progress_level: prog_lv,
            bag_deltas,
        })
    }

    /// Upgrade, evolve, wash, or replace-wash a `GodArm` (`Thần Binh` — `godarm.*`).
    pub fn godarm_wash_upgrade(
        &mut self,
        item_id: u64,
        action: GodArmAction,
        is_ten: bool,
        now_ts: u64,
    ) -> Result<(JsonValue, BagDelta)> {
        let (target_snap, in_bag) = self.peek_item_anywhere(item_id)?;
        let (class, _) = classify_template_id(target_snap.item_template_id)?;
        if class != ItemClass::GodArm {
            return Err(PlayerError::InvalidParameter(format!(
                "godarm_wash_upgrade requires GodArm, got {class:?}"
            )));
        }

        let steps = if is_ten { 10u32 } else { 1u32 };
        let mut bag_deltas = BagDelta::new();

        match action {
            GodArmAction::Upgrade => {
                self.consume_currency(
                    CurrencyType::GodarmCoin,
                    u64::from(steps) * 50,
                    now_ts,
                )?;
                self.consume_currency(CurrencyType::Copper, u64::from(steps) * 5_000, now_ts)?;
            }
            GodArmAction::Evolve => {
                self.consume_currency(CurrencyType::GodarmCoin, 200, now_ts)?;
                self.consume_currency(CurrencyType::Copper, 25_000, now_ts)?;
            }
            GodArmAction::Wash => {
                self.consume_currency(
                    CurrencyType::GodarmCoin,
                    u64::from(steps) * 20,
                    now_ts,
                )?;
            }
            GodArmAction::ReplaceWash => {}
        }

        let va_json = {
            let (it_mut, _) = self.find_item_mut_anywhere(item_id)?;
            match action {
                GodArmAction::Upgrade => {
                    let next = it_mut
                        .va_item_text
                        .get("upgrade_level")
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(0) as u32
                        + steps;
                    it_mut
                        .va_item_text
                        .insert("upgrade_level".into(), JsonValue::from(next));
                }
                GodArmAction::Evolve => {
                    let next = it_mut
                        .va_item_text
                        .get("progress_level")
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(0) as u32
                        + 1;
                    it_mut
                        .va_item_text
                        .insert("progress_level".into(), JsonValue::from(next));
                }
                GodArmAction::Wash => {
                    let mut cand = BTreeMap::new();
                    cand.insert("1".into(), JsonValue::from(u64::from(steps) * 120));
                    cand.insert("9".into(), JsonValue::from(u64::from(steps) * 30));
                    it_mut
                        .va_item_text
                        .insert("godArm_last_wash".into(), JsonValue::Object(cand));
                }
                GodArmAction::ReplaceWash => {
                    if let Some(last) = it_mut.va_item_text.get("godArm_last_wash").cloned() {
                        it_mut.va_item_text.insert("godArm_wash".into(), last);
                        it_mut
                            .va_item_text
                            .insert("godArm_last_wash".into(), JsonValue::empty_object());
                    }
                }
            }
            if in_bag {
                bag_deltas.record_item(it_mut.clone());
            }
            JsonValue::Object(it_mut.va_item_text.clone())
        };

        self.recalculate_fight_force();
        self.bump_revision();
        Ok((va_json, bag_deltas))
    }

    /// Explore / hunt Spirits (`Chiến Hồn` — `spirit.exploreSoul` / `huntSoul`),
    /// advancing NPC stage in `raffle_state["spirit_npc"]` and granting `Spirit` items + `SpiritCoin`.
    pub fn explore_soul(
        &mut self,
        times: u32,
        quick: bool,
        use_gold: bool,
        now_ts: u64,
    ) -> Result<ExploreSoulOutcome> {
        let count = if quick { times.max(10) } else { times.max(1) }.min(50);

        let max_grids = self
            .inventory
            .grid_max_num
            .get(&BagType::Spirit)
            .copied()
            .unwrap_or_else(|| BagType::Spirit.default_max_grids());
        let used = self
            .inventory
            .bags
            .get(&BagType::Spirit)
            .map(|m| m.len() as u32)
            .unwrap_or(0);
        if used.saturating_add(count) > max_grids {
            return Err(PlayerError::BagFull {
                bag_type: BagType::Spirit.vnsg2_key(),
                max_grids,
            });
        }

        let (copper_spent, gold_spent) = if use_gold {
            let gc = u64::from(count) * 20;
            self.consume_currency(CurrencyType::Gold, gc, now_ts)?;
            (0, gc)
        } else {
            let cc = u64::from(count) * 8_000;
            self.consume_currency(CurrencyType::Copper, cc, now_ts)?;
            (cc, 0)
        };

        let mut npc_stage = self
            .raffle_state
            .get("spirit_npc")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1) as u32;

        let mut created_spirits = Vec::with_capacity(count as usize);
        let mut bag_deltas = BagDelta::new();
        let spirit_pool: [u32; 5] = [200_101, 200_102, 200_201, 200_301, 200_401];

        for i in 0..count {
            let idx = if use_gold || npc_stage >= 4 {
                3 + ((i as usize) % 2)
            } else {
                ((npc_stage as usize) + (i as usize)) % spirit_pool.len()
            };
            let sp_tid = spirit_pool[idx];
            let (added, d) = self.add_item_with_delta(sp_tid, 1, now_ts)?;
            bag_deltas.merge(d);
            created_spirits.extend(added);
            npc_stage = if npc_stage >= 5 { 1 } else { npc_stage + 1 };
        }

        let spirit_coin_gained = u64::from(count) * 50;
        self.grant_currency(CurrencyType::SpiritCoin, spirit_coin_gained, now_ts);
        self.raffle_state
            .insert("spirit_npc".into(), JsonValue::from(npc_stage));
        self.bump_revision();

        Ok(ExploreSoulOutcome {
            created_spirits,
            spirit_coin_gained,
            copper_spent,
            gold_spent,
            npc_stage,
            bag_deltas,
        })
    }

    // ========================================================================
    // 7. Client `vaConfig` / `userConfig` Persistence (`ConfigData.lua`)
    // ========================================================================

    /// Save a client preference key-value pair (`user.setVaConfig`).
    pub fn set_user_config(&mut self, key: impl Into<String>, value: JsonValue) -> Result<()> {
        let k = key.into();
        if k.is_empty() || k.len() > 128 {
            return Err(PlayerError::InvalidParameter(
                "vaConfig key must be 1..=128 bytes".into(),
            ));
        }
        self.va_config.insert(k, value);
        self.bump_revision();
        Ok(())
    }

    /// Remove a key from `vaConfig`.
    pub fn remove_user_config(&mut self, key: &str) -> Option<JsonValue> {
        let removed = self.va_config.remove(key);
        if removed.is_some() {
            self.bump_revision();
        }
        removed
    }
}
