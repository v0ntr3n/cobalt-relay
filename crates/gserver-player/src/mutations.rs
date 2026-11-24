use std::collections::{BTreeMap, BTreeSet};

use crate::bag::{classify_template_id, BagItem};
use crate::error::{PlayerError, Result};
use crate::formation::EquipSlotType;
use crate::json::JsonValue;
use crate::models::{CurrencyType, HeroInstance, PlayerProfile, StarterItemSeed};

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

/// Summary returned by `PlayerProfile::use_item`.
#[derive(Clone, Debug, PartialEq)]
pub struct UseItemOutcome {
    pub consumed_template_id: u32,
    pub consumed_count: u32,
    pub remaining_in_grid: u32,
    pub daily_used_total: u32,
    pub summoned_hids: Vec<u64>,
    pub granted_items: Vec<BagItem>,
}

/// Summary returned by `PlayerProfile::level_up_hero` (`hero.heroLevelUp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeroLevelUpOutcome {
    pub hid: u64,
    pub old_level: u32,
    pub new_level: u32,
    pub exp: u64,
}

impl PlayerProfile {
    #[inline]
    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    // ========================================================================
    // 1. Currency, Stamina & Player EXP Progression
    // ========================================================================

    /// Grant `amount` of `currency` (saturating at `u64::MAX`), returning the new balance.
    pub fn grant_currency(&mut self, currency: CurrencyType, amount: u64, now_ts: u64) -> u64 {
        let entry = self.currencies.entry(currency).or_insert(0);
        *entry = entry.saturating_add(amount);
        let updated = *entry;
        if currency == CurrencyType::Execution && self.execution_time == 0 {
            self.execution_time = now_ts;
        } else if currency == CurrencyType::Energy && self.energy_time == 0 {
            self.energy_time = now_ts;
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
        if currency == CurrencyType::Execution && now_ts > 0 {
            self.execution_time = now_ts;
        } else if currency == CurrencyType::Energy && now_ts > 0 {
            self.energy_time = now_ts;
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
    // 2. Inventory / Bag Item Mutations
    // ========================================================================

    /// Add `count` items of `template_id` to the appropriate sub-bag (`ItemFactory.lua` routing).
    pub fn add_item(&mut self, template_id: u32, count: u32, now_ts: u64) -> Result<Vec<BagItem>> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "item count to add must be > 0".into(),
            ));
        }
        let (_, bag_type) = classify_template_id(template_id)?;

        if bag_type.is_stackable() {
            // Stack into an existing grid if present
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
                    self.bump_revision();
                    return Ok(vec![updated]);
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
            self.bump_revision();
            Ok(vec![item])
        } else {
            // Non-stackable equipment/pet/god_arm: verify enough free grids BEFORE mutating
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
                created.push(item);
            }
            self.bump_revision();
            Ok(created)
        }
    }

    /// Remove `count` items from a specific `gid` slot, deleting the grid entry when `item_num` hits `0`.
    pub fn remove_item_by_gid(&mut self, gid: u32, count: u32) -> Result<BagItem> {
        if count == 0 {
            return Err(PlayerError::InvalidParameter(
                "item count to remove must be > 0".into(),
            ));
        }
        let bag_type = crate::bag::BagType::from_gid(gid).ok_or_else(|| {
            PlayerError::ItemNotFound(format!("gid={gid} outside all bag ranges"))
        })?;
        let sub =
            self.inventory.bags.get_mut(&bag_type).ok_or_else(|| {
                PlayerError::ItemNotFound(format!("missing sub-bag for gid={gid}"))
            })?;
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
        if snapshot.item_num == 0 {
            sub.remove(&gid);
        }
        self.bump_revision();
        Ok(snapshot)
    }

    /// Remove `count` items matching `template_id` across grids in ascending `gid` order.
    /// Returns `Vec<(gid, remaining_num)>`.
    pub fn remove_item_by_template(
        &mut self,
        template_id: u32,
        count: u32,
    ) -> Result<Vec<(u32, u32)>> {
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
        for gid in matching_gids {
            if remaining_to_take == 0 {
                break;
            }
            if let Some(it) = sub.get_mut(&gid) {
                let take = it.item_num.min(remaining_to_take);
                it.item_num -= take;
                remaining_to_take -= take;
                let left = it.item_num;
                if left == 0 {
                    sub.remove(&gid);
                }
                affected.push((gid, left));
            }
        }
        self.bump_revision();
        Ok(affected)
    }

    /// Consume `count` items from `gid`, track `daily_use[tid]`, and apply `effect` (`bag.useItem`).
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

        // Pre-validate side-effect capacity before mutating the bag!
        if let UseItemEffect::SummonHero { htid } = &effect {
            if (1..=28).contains(htid) {
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

    // ========================================================================
    // 3. Hero Roster & Progression Mutations
    // ========================================================================

    /// Summon a new hero instance (`HerosData` / `re.hero.new`), returning the new `hid`.
    pub fn summon_hero(&mut self, htid: u32, initial_level: u32) -> Result<u64> {
        if htid == 0 {
            return Err(PlayerError::InvalidParameter("htid must be > 0".into()));
        }
        if (1..=28).contains(&htid) {
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

    /// Level up a hero (`hero.heroLevelUp`), deducting `copper_cost` and `soul_cost` atomically.
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

    /// Evolve a hero (`evolve_level + 1` via `CS_EVOLVE_DOEVOLVE`), optionally consuming duplicate `sacrifice_hids`.
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

    /// Upgrade / breakthrough a hero (`hero.upgrade`: `upgrade_level + 1`).
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

    // ========================================================================
    // 4. Squad / Formation & Equipment Mutations
    // ========================================================================

    /// Assign `hid` to active squad slot `1..=6` (`formation.setHeroToPosition`).
    /// Returns the previously deployed `hid` in that slot if replaced.
    pub fn set_squad_slot(&mut self, slot: u8, hid: u64) -> Result<Option<u64>> {
        if !(1..=6).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "squad slot must be in 1..=6, got {slot}"
            )));
        }
        let hero = self
            .heroes
            .get(&hid)
            .ok_or(PlayerError::HeroNotFound(hid))?;
        let target_htid = hero.htid;

        if self.formation.partner.values().any(|&h| h == hid)
            || self.formation.buddy.values().any(|&h| h == hid)
        {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "hero hid={hid} is currently in partner/buddy and must be removed first"
            )));
        }

        // Check if replacing protagonist without moving protagonist elsewhere
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

        // Enforce unique `htid` across squad (excluding `slot` and any old slot of `hid`) + partner
        for (&s, &other_hid) in &self.formation.position {
            if s != slot && other_hid != hid {
                if let Some(other) = self.heroes.get(&other_hid) {
                    if other.htid == target_htid {
                        return Err(PlayerError::DuplicateHeroTemplateInSquad {
                            htid: target_htid,
                        });
                    }
                }
            }
        }
        for &p_hid in self.formation.partner.values() {
            if p_hid != 0 && p_hid != hid {
                if let Some(other) = self.heroes.get(&p_hid) {
                    if other.htid == target_htid {
                        return Err(PlayerError::DuplicateHeroTemplateInSquad {
                            htid: target_htid,
                        });
                    }
                }
            }
        }

        // If `hid` was already in another squad slot, swap with `slot`
        if let Some(prev_slot) = self.formation.squad_slot_of(hid) {
            self.swap_squad_slots(prev_slot, slot)?;
            return Ok(self.formation.position.get(&prev_slot).copied());
        }

        let replaced = self.formation.position.insert(slot, hid);
        self.formation.sync_grid_with_squad();
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(replaced)
    }

    /// Remove the hero in active squad slot `1..=6` (`formation.unsetHeroFromPosition`).
    /// Protagonist cannot be removed from the active squad.
    pub fn unset_squad_slot(&mut self, slot: u8) -> Result<u64> {
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
        self.formation.position.remove(&slot);
        self.formation.sync_grid_with_squad();
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(hid)
    }

    /// Swap two squad slots `1..=6` (`formation.transHero`).
    pub fn swap_squad_slots(&mut self, slot_a: u8, slot_b: u8) -> Result<()> {
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
        self.formation.sync_grid_with_squad();
        self.recalculate_fight_force();
        self.bump_revision();
        Ok(())
    }

    /// Update the 3x3 battle matrix (`1..=9 -> hid` via `formation.set`).
    /// Validates that `grid_map` places the exact set of active squad heroes onto distinct `1..=9` cells.
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
        self.formation.grid_formation = grid_map;
        self.bump_revision();
        Ok(())
    }

    /// Unlock a LittleFriend (`partner`) slot `1..=6` (`formation.openPartner`).
    pub fn open_partner_slot(&mut self, slot: u8) -> Result<()> {
        if !(1..=6).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "partner slot must be in 1..=6, got {slot}"
            )));
        }
        self.formation.partner.entry(slot).or_insert(0);
        self.bump_revision();
        Ok(())
    }

    /// Set or clear (`hid == 0`) a LittleFriend (`partner`) slot `1..=6` (`formation.setPartner`).
    pub fn set_partner_slot(&mut self, slot: u8, hid: u64) -> Result<Option<u64>> {
        if !(1..=6).contains(&slot) {
            return Err(PlayerError::InvalidFormationSlot(format!(
                "partner slot must be in 1..=6, got {slot}"
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
        let target_htid = hero.htid;
        for &s_hid in self.formation.position.values() {
            if let Some(other) = self.heroes.get(&s_hid) {
                if other.htid == target_htid {
                    return Err(PlayerError::DuplicateHeroTemplateInSquad { htid: target_htid });
                }
            }
        }
        for (&p_slot, &p_hid) in &self.formation.partner {
            if p_slot != slot && p_hid != 0 && p_hid != hid {
                if let Some(other) = self.heroes.get(&p_hid) {
                    if other.htid == target_htid {
                        return Err(PlayerError::DuplicateHeroTemplateInSquad {
                            htid: target_htid,
                        });
                    }
                }
            }
        }
        let prev = self.formation.partner.insert(slot, hid);
        self.bump_revision();
        Ok(prev)
    }

    /// Equip an item from `bag_gid` onto `squad_slot` (`1..=6`) and `equip_slot` (`1..=9`) (`formation.addArming`).
    /// If another item was already in that slot, it is returned to the player's inventory automatically.
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

        // Remove 1 instance from bag first (frees a grid in case a previous item swaps back)
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

    /// Unequip the item at `(squad_slot, equip_slot)` back into the inventory (`formation.removeArming`).
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

        // Insert into bag BEFORE removing from arming so BagFull never loses the item!
        let bag_item = self.inventory.insert_instance(item)?;
        if let Some(slot_map) = self.formation.arming.get_mut(&squad_slot) {
            slot_map.remove(&(equip_slot as u8));
        }

        self.recalculate_fight_force();
        self.bump_revision();
        Ok(bag_item)
    }

    // ========================================================================
    // 5. Client `vaConfig` / `userConfig` Persistence (`ConfigData.lua`)
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
