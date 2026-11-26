use std::collections::BTreeMap;

use crate::bag::{classify_template_id, BagItem, ItemClass};
use crate::error::{PlayerError, Result};
use crate::json::{JsonObjReader, JsonValue};
use crate::models::{HeroInstance, PlayerProfile};

/// Default battle-grid slot mapping (`FormationData.lua`: `attack_sep_list = {3, 6, 9, 2, 5, 8}`).
/// Maps squad slot `1..=6` strictly onto the 6 visible battle grid cells `{3, 6, 9, 2, 5, 8}`
/// (`FormationPanel.lua`: `pos_list = {2, 3, 5, 6, 8, 9}`; cells `1, 4, 7` do not exist in UI!).
pub const DEFAULT_SQUAD_TO_GRID: [u8; 6] = [3, 6, 9, 2, 5, 8];

/// The ONLY valid battle grid positions in `FormationPanel.lua:20` (`pos_list = {2, 3, 5, 6, 8, 9}`).
pub const VALID_BATTLE_GRID_CELLS: [u8; 6] = [2, 3, 5, 6, 8, 9];

/// Returns `true` iff `cell` is one of the 6 visible battle grid cells `{2, 3, 5, 6, 8, 9}`.
#[inline]
pub const fn is_valid_battle_grid_cell(cell: u8) -> bool {
    matches!(cell, 2 | 3 | 5 | 6 | 8 | 9)
}

/// Equipment slot type inside `arming[pos]["1".."9"]` (`ArmInfo.lua`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EquipSlotType {
    Weapon = 1,
    Helmet = 2,
    Armor = 3,
    Belt = 4,
    AttackTreasure = 5,
    DefenseTreasure = 6,
    Awake = 7,
    Pet = 8,
    GodArm = 9,
}

impl EquipSlotType {
    pub fn from_u8(slot: u8) -> Result<Self> {
        match slot {
            1 => Ok(EquipSlotType::Weapon),
            2 => Ok(EquipSlotType::Helmet),
            3 => Ok(EquipSlotType::Armor),
            4 => Ok(EquipSlotType::Belt),
            5 => Ok(EquipSlotType::AttackTreasure),
            6 => Ok(EquipSlotType::DefenseTreasure),
            7 => Ok(EquipSlotType::Awake),
            8 => Ok(EquipSlotType::Pet),
            9 => Ok(EquipSlotType::GodArm),
            _ => Err(PlayerError::InvalidFormationSlot(format!(
                "equipment slot type must be in 1..=9, got {slot}"
            ))),
        }
    }

    /// Validate that `item_template_id` has a compatible `ItemClass` for this slot.
    pub fn validate_item(self, item_template_id: u32) -> Result<()> {
        let (class, _) = classify_template_id(item_template_id)?;
        let ok = match self {
            EquipSlotType::Weapon
            | EquipSlotType::Helmet
            | EquipSlotType::Armor
            | EquipSlotType::Belt => class == ItemClass::Arm,
            EquipSlotType::AttackTreasure | EquipSlotType::DefenseTreasure => {
                class == ItemClass::Treasure
            }
            EquipSlotType::Awake => class == ItemClass::Awake,
            EquipSlotType::Pet => class == ItemClass::Pet,
            EquipSlotType::GodArm => class == ItemClass::GodArm,
        };
        if !ok {
            return Err(PlayerError::EquipTypeMismatch {
                slot_type: self as u8,
                item_template_id,
            });
        }
        Ok(())
    }
}

/// `FormationBookInfo.lua` state embedded in `formation.getPositionInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormationBookState {
    pub cur_use: u32,
    pub lv_info: BTreeMap<u32, u32>,
    pub reset_num: u32,
}

impl FormationBookState {
    pub fn to_json(&self) -> JsonValue {
        let mut lv = BTreeMap::new();
        for (k, v) in &self.lv_info {
            lv.insert(k.to_string(), JsonValue::from(*v));
        }
        let mut m = BTreeMap::new();
        m.insert("cur_use".into(), JsonValue::from(self.cur_use));
        m.insert("lv_info".into(), JsonValue::Object(lv));
        m.insert("reset_num".into(), JsonValue::from(self.reset_num));
        JsonValue::Object(m)
    }

    pub fn from_json(val: &JsonValue) -> Result<Self> {
        let r = JsonObjReader::new(val, "FormationBookState")?;
        let mut lv_info = BTreeMap::new();
        if let Some(lv) = r.opt_obj("lv_info")? {
            for (k, v) in lv {
                let id: u32 = k
                    .parse()
                    .map_err(|_| PlayerError::Json("invalid formationBook lv_info key".into()))?;
                let level = v.as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
                lv_info.insert(id, level);
            }
        }
        Ok(Self {
            cur_use: r.opt_u32("cur_use", 0)?,
            lv_info,
            reset_num: r.opt_u32("reset_num", 0)?,
        })
    }
}

/// Complete squad, 6-cell battle matrix (`{2, 3, 5, 6, 8, 9}`), LittleFriend (`partner`),
/// `buddy`, and slot-bound equipment state (`HeroFormationData.lua` + `FormationData.lua`).
#[derive(Clone, Debug, PartialEq)]
pub struct FormationState {
    /// Active squad slots `1..=6 -> hid` (`hid > 0` ONLY; `HeroFormationData` crashes on `hid == 0`).
    pub position: BTreeMap<u8, u64>,
    /// 6-cell battle grid `{2, 3, 5, 6, 8, 9} -> hid` (`FormationData.lua`: `formation.get`; NEVER `1, 4, 7`).
    pub grid_formation: BTreeMap<u8, u64>,
    /// LittleFriend / 援军 slots `1..=6 -> hid` (`0` = unlocked empty slot; locked slots omitted).
    pub partner: BTreeMap<u8, u64>,
    /// Buddy / 助战军 slots `1..=8 -> hid` (`0` = unlocked empty slot; locked slots omitted).
    pub buddy: BTreeMap<u8, u64>,
    pub buddy_level_info: BTreeMap<u8, u32>,
    /// Per-squad-slot (`1..=6`) equipped items keyed by `EquipSlotType` (`1..=9`).
    pub arming: BTreeMap<u8, BTreeMap<u8, BagItem>>,
    /// Per-squad-slot (`1..=6`) equipped Spirits (`1..=8 -> BagItem`).
    pub spirit: BTreeMap<u8, BTreeMap<u8, BagItem>>,
    /// Per-squad-slot (`1..=6`) equipped YanWu (`1..=4 -> BagItem`).
    pub yan_wu: BTreeMap<u8, BTreeMap<u8, BagItem>>,
    /// Per-squad-slot (`1..=6`) equipped Brochures (`1..=6 -> BagItem`).
    pub brochure: BTreeMap<u8, BTreeMap<u8, BagItem>>,
    /// Per-squad-slot (`1..=6`) equipped SoulSkills (`1..=6 -> BagItem`).
    pub soul_skill: BTreeMap<u8, BTreeMap<u8, BagItem>>,
    /// Per-squad-slot (`1..=6`) equipped SoulStone (`BagItem`).
    pub soul_stone: BTreeMap<u8, BagItem>,
    /// Per-squad-slot (`1..=6`) equipped SoulJade (`BagItem`).
    pub soul_jade: BTreeMap<u8, BagItem>,
    pub bingfu: BTreeMap<u8, u32>,
    pub bingshu: BTreeMap<u8, u32>,
    /// Per-slot (`1..=6`) GodAnimal (`None` = unlocked empty `0`, `Some(BagItem)` = full wire dict).
    pub godanimal: BTreeMap<u8, Option<BagItem>>,
    /// Per-slot (`1..=6`) TopAnimal (`None` = unlocked empty `0`, `Some(BagItem)` = full wire dict).
    pub topanimal: BTreeMap<u8, Option<BagItem>>,
    /// Per-slot (`1..=6`) GodAnimal Protection (`None` = unlocked empty `0`, `Some(BagItem)` = full wire dict).
    pub protection: BTreeMap<u8, Option<BagItem>>,
    /// Per-squad-slot (`1..=6`) equipped attribute skin IDs (`attrSkins`).
    pub attr_skins: BTreeMap<u8, u32>,
    pub joint_skill: Vec<u32>,
    pub attr_extra: BTreeMap<u32, i64>,
    pub more_power_open_count: u32,
    pub formation_book: FormationBookState,
    pub soul_power: u32,
    pub soul_jade_qi: u32,
}

impl Default for FormationState {
    fn default() -> Self {
        Self::new()
    }
}

impl FormationState {
    pub fn new() -> Self {
        let mut arming = BTreeMap::new();
        let mut spirit = BTreeMap::new();
        let mut yan_wu = BTreeMap::new();
        let mut brochure = BTreeMap::new();
        let mut soul_skill = BTreeMap::new();
        for pos in 1..=6 {
            arming.insert(pos, BTreeMap::new());
            spirit.insert(pos, BTreeMap::new());
            yan_wu.insert(pos, BTreeMap::new());
            brochure.insert(pos, BTreeMap::new());
            soul_skill.insert(pos, BTreeMap::new());
        }
        let mut partner = BTreeMap::new();
        partner.insert(1, 0); // Slot 1 unlocked empty by default
        Self {
            position: BTreeMap::new(),
            grid_formation: BTreeMap::new(),
            partner,
            buddy: BTreeMap::new(),
            buddy_level_info: BTreeMap::new(),
            arming,
            spirit,
            yan_wu,
            brochure,
            soul_skill,
            soul_stone: BTreeMap::new(),
            soul_jade: BTreeMap::new(),
            bingfu: BTreeMap::new(),
            bingshu: BTreeMap::new(),
            godanimal: BTreeMap::new(),
            topanimal: BTreeMap::new(),
            protection: BTreeMap::new(),
            attr_skins: BTreeMap::new(),
            joint_skill: Vec::new(),
            attr_extra: BTreeMap::new(),
            more_power_open_count: 0,
            formation_book: FormationBookState::default(),
            soul_power: 0,
            soul_jade_qi: 0,
        }
    }

    pub fn with_protagonist(protagonist_hid: u64) -> Self {
        let mut f = Self::new();
        f.position.insert(1, protagonist_hid);
        f.grid_formation
            .insert(DEFAULT_SQUAD_TO_GRID[0], protagonist_hid);
        f
    }

    pub fn is_hero_deployed(&self, hid: u64) -> bool {
        if hid == 0 {
            return false;
        }
        self.position.values().any(|&h| h == hid)
            || self.partner.values().any(|&h| h == hid)
            || self.buddy.values().any(|&h| h == hid)
    }

    /// Returns `true` if an item with `item_id` is currently equipped anywhere on the formation.
    pub fn is_item_equipped(&self, item_id: u64) -> bool {
        if item_id == 0 {
            return false;
        }
        for m in self.arming.values() {
            if m.values().any(|it| it.item_id == item_id) {
                return true;
            }
        }
        for m in self.spirit.values() {
            if m.values().any(|it| it.item_id == item_id) {
                return true;
            }
        }
        for m in self.yan_wu.values() {
            if m.values().any(|it| it.item_id == item_id) {
                return true;
            }
        }
        for m in self.brochure.values() {
            if m.values().any(|it| it.item_id == item_id) {
                return true;
            }
        }
        for m in self.soul_skill.values() {
            if m.values().any(|it| it.item_id == item_id) {
                return true;
            }
        }
        if self.soul_stone.values().any(|it| it.item_id == item_id)
            || self.soul_jade.values().any(|it| it.item_id == item_id)
        {
            return true;
        }
        for entry in self
            .godanimal
            .values()
            .chain(self.topanimal.values())
            .chain(self.protection.values())
        {
            if let Some(it) = entry {
                if it.item_id == item_id {
                    return true;
                }
            }
        }
        false
    }

    pub fn squad_slot_of(&self, hid: u64) -> Option<u8> {
        self.position
            .iter()
            .find_map(|(&slot, &h)| if h == hid { Some(slot) } else { None })
    }

    /// Re-synchronize `grid_formation` so every hero in `position` occupies a unique cell
    /// strictly inside `{2, 3, 5, 6, 8, 9}` (`attack_sep_list = [3, 6, 9, 2, 5, 8]`; NEVER `1, 4, 7`).
    pub fn sync_grid_with_squad(&mut self) {
        let active_hids: Vec<u64> = self.position.values().copied().collect();
        self.grid_formation
            .retain(|&cell, hid| is_valid_battle_grid_cell(cell) && active_hids.contains(hid));

        // Deduplicate any hero accidentally mapped twice
        let mut seen_hids = Vec::with_capacity(6);
        self.grid_formation.retain(|_, hid| {
            if seen_hids.contains(hid) {
                false
            } else {
                seen_hids.push(*hid);
                true
            }
        });

        for (&slot, &hid) in &self.position {
            if hid == 0 {
                continue;
            }
            let already_placed = self.grid_formation.values().any(|&h| h == hid);
            if !already_placed {
                let preferred = DEFAULT_SQUAD_TO_GRID
                    .get(usize::from(slot.saturating_sub(1)))
                    .copied()
                    .unwrap_or(3);
                if is_valid_battle_grid_cell(preferred)
                    && !self.grid_formation.contains_key(&preferred)
                {
                    self.grid_formation.insert(preferred, hid);
                } else {
                    for g in DEFAULT_SQUAD_TO_GRID {
                        if let std::collections::btree_map::Entry::Vacant(e) =
                            self.grid_formation.entry(g)
                        {
                            e.insert(hid);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Exact `formation.get` payload (`FormationData.lua`), guaranteed to emit ONLY `{2, 3, 5, 6, 8, 9}`.
    pub fn to_vnsg2_formation_get(&self) -> JsonValue {
        let mut f_map = BTreeMap::new();
        for (&grid_pos, &hid) in &self.grid_formation {
            if hid > 0 && is_valid_battle_grid_cell(grid_pos) {
                f_map.insert(grid_pos.to_string(), JsonValue::from(hid));
            }
        }
        let mut root = BTreeMap::new();
        root.insert("formation".into(), JsonValue::Object(f_map));
        JsonValue::Object(root)
    }

    fn build_subslot_dict(
        map_opt: Option<&BTreeMap<u8, BagItem>>,
        min_slots: u8,
    ) -> JsonValue {
        let max_slot = map_opt
            .and_then(|m| m.keys().copied().max())
            .unwrap_or(min_slots)
            .max(min_slots);
        let mut inner = BTreeMap::new();
        for sub in 1..=max_slot {
            if let Some(item) = map_opt.and_then(|m| m.get(&sub)) {
                inner.insert(sub.to_string(), item.to_wire_json());
            } else {
                inner.insert(sub.to_string(), JsonValue::empty_object());
            }
        }
        JsonValue::Object(inner)
    }

    fn serialize_animal_or_protection_map(map: &BTreeMap<u8, Option<BagItem>>) -> JsonValue {
        let mut out = BTreeMap::new();
        for (&slot, entry) in map {
            match entry {
                Some(item) => {
                    out.insert(slot.to_string(), item.to_wire_json());
                }
                None => {
                    out.insert(slot.to_string(), JsonValue::from(0u32));
                }
            }
        }
        JsonValue::Object(out)
    }

    /// Exact `formation.getPositionInfo` payload (`HeroFormationData.lua`),
    /// with 1-based keys `"1".."6"`, full `BagItem` wire dicts for `arming`/`spirit`/`godanimal`/`topanimal`/`protection`,
    /// and classic compatibility fields (`squad`, `arrLittleFriend`, `attrExtra`).
    pub fn to_vnsg2_position_info(&self, heroes: &BTreeMap<u64, HeroInstance>) -> JsonValue {
        let mut root = BTreeMap::new();

        // CRITICAL: position ONLY contains occupied slots `"1".."6"` with hid > 0 present in allHeros!
        let mut pos_obj = BTreeMap::new();
        let mut classic_squad = Vec::with_capacity(6);
        for slot in 1..=6u8 {
            if let Some(&hid) = self.position.get(&slot) {
                if hid > 0 && heroes.contains_key(&hid) {
                    pos_obj.insert(slot.to_string(), JsonValue::from(hid));
                    classic_squad.push(JsonValue::from(hid));
                } else {
                    classic_squad.push(JsonValue::from(0u64));
                }
            } else {
                classic_squad.push(JsonValue::from(0u64));
            }
        }
        root.insert("position".into(), JsonValue::Object(pos_obj));

        // `partner`: unlocked empty = `0`, occupied = `hid`, locked = omitted
        let mut partner_obj = BTreeMap::new();
        let mut arr_little_friend = Vec::new();
        for (&slot, &hid) in &self.partner {
            if (1..=6).contains(&slot) {
                let valid_hid = if hid > 0 && heroes.contains_key(&hid) {
                    hid
                } else {
                    0
                };
                partner_obj.insert(slot.to_string(), JsonValue::from(valid_hid));
                arr_little_friend.push(JsonValue::from(valid_hid));
            }
        }
        root.insert("partner".into(), JsonValue::Object(partner_obj));

        // `buddy` & `buddyLevelInfo`: `"1".."8"`
        let mut buddy_obj = BTreeMap::new();
        for (&slot, &hid) in &self.buddy {
            if (1..=8).contains(&slot) {
                let valid_hid = if hid > 0 && heroes.contains_key(&hid) {
                    hid
                } else {
                    0
                };
                buddy_obj.insert(slot.to_string(), JsonValue::from(valid_hid));
            }
        }
        root.insert("buddy".into(), JsonValue::Object(buddy_obj));

        let mut buddy_lv_obj = BTreeMap::new();
        for slot in 1..=8u8 {
            if let Some(&lv) = self.buddy_level_info.get(&slot) {
                buddy_lv_obj.insert(slot.to_string(), JsonValue::from(lv));
            } else if self.buddy.contains_key(&slot) {
                buddy_lv_obj.insert(slot.to_string(), JsonValue::from(0u32));
            }
        }
        root.insert("buddyLevelInfo".into(), JsonValue::Object(buddy_lv_obj));

        // 6-element outer `arming` array (`1..6` in Lua): inner `"1".."9"` -> BagItem dict (empty sub-slots omitted!)
        let mut arming_arr = Vec::with_capacity(6);
        let mut spirit_arr = Vec::with_capacity(6);
        let mut yan_wu_arr = Vec::with_capacity(6);
        let mut brochure_arr = Vec::with_capacity(6);
        let mut soul_skill_arr = Vec::with_capacity(6);
        let mut soul_stone_obj = BTreeMap::new();
        let mut soul_jade_obj = BTreeMap::new();

        for slot in 1..=6u8 {
            let mut slot_arm = BTreeMap::new();
            if let Some(eq_map) = self.arming.get(&slot) {
                for (&eq_type, item) in eq_map {
                    if (1..=9).contains(&eq_type) {
                        slot_arm.insert(eq_type.to_string(), item.to_wire_json());
                    }
                }
            }
            arming_arr.push(JsonValue::Object(slot_arm));

            // `spirit`: `"1".."6"` -> full BagItem dict, empty `{}`
            spirit_arr.push(Self::build_subslot_dict(self.spirit.get(&slot), 6));
            // `yanWu`: `"1".."4"` -> full BagItem dict, empty `{}`
            yan_wu_arr.push(Self::build_subslot_dict(self.yan_wu.get(&slot), 4));
            // `brochure`: `"1".."6"` -> full BagItem dict, empty `{}`
            brochure_arr.push(Self::build_subslot_dict(self.brochure.get(&slot), 6));
            // `soulSkill`: `"1".."6"` -> full BagItem dict, empty `{}`
            soul_skill_arr.push(Self::build_subslot_dict(self.soul_skill.get(&slot), 6));

            // `soulStone` & `soulJade`: `"1".."6"` -> full BagItem dict or empty `{}`
            let ss_val = self
                .soul_stone
                .get(&slot)
                .map(BagItem::to_wire_json)
                .unwrap_or_else(JsonValue::empty_object);
            soul_stone_obj.insert(slot.to_string(), ss_val);

            let sj_val = self
                .soul_jade
                .get(&slot)
                .map(BagItem::to_wire_json)
                .unwrap_or_else(JsonValue::empty_object);
            soul_jade_obj.insert(slot.to_string(), sj_val);
        }

        root.insert("arming".into(), JsonValue::Array(arming_arr));
        root.insert("spirit".into(), JsonValue::Array(spirit_arr));
        root.insert("yanWu".into(), JsonValue::Array(yan_wu_arr));
        root.insert("brochure".into(), JsonValue::Array(brochure_arr));
        root.insert("soulSkill".into(), JsonValue::Array(soul_skill_arr));
        root.insert("soulStone".into(), JsonValue::Object(soul_stone_obj));
        root.insert("soulJade".into(), JsonValue::Object(soul_jade_obj));

        // `allHeros` map keyed by hid string
        let mut all_heros = BTreeMap::new();
        for (hid, hero) in heroes {
            all_heros.insert(hid.to_string(), hero.to_vnsg2_hero_info());
        }
        root.insert("allHeros".into(), JsonValue::Object(all_heros));

        // Mandatory non-nil tables in HeroFormationData:parseData
        let mut bingfu_obj = BTreeMap::new();
        for (&pos, &tid) in &self.bingfu {
            if (1..=6).contains(&pos) {
                bingfu_obj.insert(pos.to_string(), JsonValue::from(tid));
            }
        }
        root.insert("bingfu".into(), JsonValue::Object(bingfu_obj));

        let mut bingshu_obj = BTreeMap::new();
        for (&pos, &tid) in &self.bingshu {
            if (1..=6).contains(&pos) {
                bingshu_obj.insert(pos.to_string(), JsonValue::from(tid));
            }
        }
        root.insert("bingshu".into(), JsonValue::Object(bingshu_obj));

        root.insert("jobSkill".into(), JsonValue::empty_object());
        root.insert("arrCounsellor".into(), JsonValue::empty_object());

        let mut attr_skins_obj = BTreeMap::new();
        for (&pos, &skin_id) in &self.attr_skins {
            if (1..=6).contains(&pos) && skin_id > 0 {
                let mut sk = BTreeMap::new();
                sk.insert("skin_id".into(), JsonValue::from(skin_id));
                attr_skins_obj.insert(pos.to_string(), JsonValue::Object(sk));
            }
        }
        root.insert("attrSkins".into(), JsonValue::Object(attr_skins_obj));

        // `godanimal`, `topanimal`, `protection`: empty unlocked slots -> `0`, occupied -> full `BagItem` wire dicts!
        root.insert(
            "godanimal".into(),
            Self::serialize_animal_or_protection_map(&self.godanimal),
        );
        root.insert(
            "topanimal".into(),
            Self::serialize_animal_or_protection_map(&self.topanimal),
        );
        root.insert(
            "protection".into(),
            Self::serialize_animal_or_protection_map(&self.protection),
        );

        let js_arr: Vec<JsonValue> = self
            .joint_skill
            .iter()
            .map(|&id| JsonValue::from(id))
            .collect();
        root.insert("jointSkill".into(), JsonValue::Array(js_arr));

        root.insert(
            "morePowerOpenCount".into(),
            JsonValue::from(self.more_power_open_count),
        );
        root.insert("formationBook".into(), self.formation_book.to_json());
        root.insert("soulPower".into(), JsonValue::from(self.soul_power));
        root.insert("soulPowerGift".into(), JsonValue::empty_object());
        root.insert("soulJadeQi".into(), JsonValue::from(self.soul_jade_qi));
        root.insert("soulJadeQiGift".into(), JsonValue::empty_object());

        // Classic 1st-gen squad compatibility fields
        root.insert("squad".into(), JsonValue::Array(classic_squad));
        root.insert(
            "arrLittleFriend".into(),
            JsonValue::Array(arr_little_friend),
        );
        let mut attr_extra_obj = BTreeMap::new();
        for (&attr_id, &val) in &self.attr_extra {
            attr_extra_obj.insert(attr_id.to_string(), JsonValue::from(val));
        }
        root.insert("attrExtra".into(), JsonValue::Object(attr_extra_obj));

        JsonValue::Object(root)
    }

    /// Exact `formation.getOtherPositionInfo` payload (`game/struct/heroOther/HeroFormationInfo.lua`
    /// & `game/struct/heroOther/HeroInfo.lua`):
    /// `{ userInfo: { [uidStr]: ... }, position, partner, buddy, godanimal, topanimal, allHeros: { [hidStr]: HeroWithEmbeddedEquip } }`.
    pub fn to_vnsg2_other_position_info(&self, profile: &PlayerProfile) -> JsonValue {
        let mut root = BTreeMap::new();

        // 1. `userInfo: { [uidStr]: { uid, uname, utid, level, vip, dress_id, fc, fight_force, guild_id } }`
        let mut user_entry = BTreeMap::new();
        user_entry.insert("uid".into(), JsonValue::from(profile.uid));
        user_entry.insert("uname".into(), JsonValue::from(profile.uname.clone()));
        user_entry.insert("utid".into(), JsonValue::from(profile.utid));
        user_entry.insert("level".into(), JsonValue::from(profile.level));
        user_entry.insert("vip".into(), JsonValue::from(profile.vip));
        user_entry.insert("dress_id".into(), JsonValue::from(profile.dress_id));
        user_entry.insert("fc".into(), JsonValue::from(profile.fc));
        user_entry.insert("fight_force".into(), JsonValue::from(profile.fc));
        user_entry.insert("guild_id".into(), JsonValue::from(profile.guild_id));
        let mut user_info_map = BTreeMap::new();
        user_info_map.insert(profile.uid.to_string(), JsonValue::Object(user_entry));
        root.insert("userInfo".into(), JsonValue::Object(user_info_map));

        // Top-level convenience summary fields
        root.insert("uid".into(), JsonValue::from(profile.uid));
        root.insert("uname".into(), JsonValue::from(profile.uname.clone()));
        root.insert("level".into(), JsonValue::from(profile.level));
        root.insert("vip".into(), JsonValue::from(profile.vip));
        root.insert("fight_force".into(), JsonValue::from(profile.fc));

        // 2. `position`, `partner`, `buddy`
        let mut pos_obj = BTreeMap::new();
        for slot in 1..=6u8 {
            if let Some(&hid) = self.position.get(&slot) {
                if hid > 0 && profile.heroes.contains_key(&hid) {
                    pos_obj.insert(slot.to_string(), JsonValue::from(hid));
                }
            }
        }
        root.insert("position".into(), JsonValue::Object(pos_obj));

        let mut partner_obj = BTreeMap::new();
        for (&slot, &hid) in &self.partner {
            if (1..=6).contains(&slot) {
                let valid_hid = if hid > 0 && profile.heroes.contains_key(&hid) {
                    hid
                } else {
                    0
                };
                partner_obj.insert(slot.to_string(), JsonValue::from(valid_hid));
            }
        }
        root.insert("partner".into(), JsonValue::Object(partner_obj));

        let mut buddy_obj = BTreeMap::new();
        for (&slot, &hid) in &self.buddy {
            if (1..=8).contains(&slot) {
                let valid_hid = if hid > 0 && profile.heroes.contains_key(&hid) {
                    hid
                } else {
                    0
                };
                buddy_obj.insert(slot.to_string(), JsonValue::from(valid_hid));
            }
        }
        root.insert("buddy".into(), JsonValue::Object(buddy_obj));

        // 3. `godanimal` & `topanimal` (only occupied full BagItem wire dicts so HeroFormationInfo:initGodAnimalData succeeds)
        let mut god_obj = BTreeMap::new();
        for (&slot, entry) in &self.godanimal {
            if let Some(item) = entry {
                god_obj.insert(slot.to_string(), item.to_wire_json());
            }
        }
        root.insert("godanimal".into(), JsonValue::Object(god_obj));

        let mut top_obj = BTreeMap::new();
        for (&slot, entry) in &self.topanimal {
            if let Some(item) = entry {
                top_obj.insert(slot.to_string(), item.to_wire_json());
            }
        }
        root.insert("topanimal".into(), JsonValue::Object(top_obj));

        // 4. `allHeros`: embed slot `equip`, `spirit`, `yanWu`, `brochure`, `arrSkill`, and `heroAttr`
        let mut all_heros = BTreeMap::new();
        for (&hid, hero) in &profile.heroes {
            // Include deployed squad, partner, and buddy heroes (plus all heroes so union lookups succeed)
            let mut h_val = hero.to_vnsg2_hero_info();
            if let JsonValue::Object(ref mut hm) = h_val {
                let (normal_sk, rage_sk) = hero.battle_skill_ids();
                let mut arr_skill = BTreeMap::new();
                if normal_sk > 0 {
                    arr_skill.insert(normal_sk.to_string(), JsonValue::from(1u32));
                }
                if rage_sk > 0 {
                    arr_skill.insert(rage_sk.to_string(), JsonValue::from(1u32));
                }
                hm.insert("arrSkill".into(), JsonValue::Object(arr_skill));

                let base_hp = 1_000u64 + u64::from(hero.level) * 200 + u64::from(hero.evolve_level) * 500;
                let base_atk = 200u64 + u64::from(hero.level) * 40 + u64::from(hero.evolve_level) * 100;
                let base_def = 100u64 + u64::from(hero.level) * 20 + u64::from(hero.evolve_level) * 50;
                let mut hero_attr = BTreeMap::new();
                hero_attr.insert("1".into(), JsonValue::from(base_hp));
                hero_attr.insert("4".into(), JsonValue::from(base_def));
                hero_attr.insert("5".into(), JsonValue::from(base_def));
                hero_attr.insert("9".into(), JsonValue::from(base_atk));
                hm.insert("heroAttr".into(), JsonValue::Object(hero_attr));

                if let Some(slot) = self.squad_slot_of(hid) {
                    let mut equip_obj = BTreeMap::new();
                    if let Some(eq_map) = self.arming.get(&slot) {
                        for (&eq_type, item) in eq_map {
                            equip_obj.insert(eq_type.to_string(), item.to_wire_json());
                        }
                    }
                    hm.insert("equip".into(), JsonValue::Object(equip_obj));
                    hm.insert(
                        "spirit".into(),
                        Self::build_subslot_dict(self.spirit.get(&slot), 6),
                    );
                    hm.insert(
                        "yanWu".into(),
                        Self::build_subslot_dict(self.yan_wu.get(&slot), 4),
                    );
                    hm.insert(
                        "brochure".into(),
                        Self::build_subslot_dict(self.brochure.get(&slot), 6),
                    );
                } else {
                    hm.insert("equip".into(), JsonValue::empty_object());
                    hm.insert("spirit".into(), Self::build_subslot_dict(None, 6));
                    hm.insert("yanWu".into(), Self::build_subslot_dict(None, 4));
                    hm.insert("brochure".into(), Self::build_subslot_dict(None, 6));
                }
            }
            all_heros.insert(hid.to_string(), h_val);
        }
        root.insert("allHeros".into(), JsonValue::Object(all_heros));

        let mut ss_obj = BTreeMap::new();
        let mut sj_obj = BTreeMap::new();
        for slot in 1..=6u8 {
            ss_obj.insert(
                slot.to_string(),
                self.soul_stone
                    .get(&slot)
                    .map(BagItem::to_wire_json)
                    .unwrap_or_else(JsonValue::empty_object),
            );
            sj_obj.insert(
                slot.to_string(),
                self.soul_jade
                    .get(&slot)
                    .map(BagItem::to_wire_json)
                    .unwrap_or_else(JsonValue::empty_object),
            );
        }
        root.insert("soulStone".into(), JsonValue::Object(ss_obj));
        root.insert("soulJade".into(), JsonValue::Object(sj_obj));
        root.insert("bingfu".into(), JsonValue::empty_object());
        root.insert("bingshu".into(), JsonValue::empty_object());
        root.insert("jobSkill".into(), JsonValue::empty_object());
        root.insert("arrCounsellor".into(), JsonValue::empty_object());
        root.insert(
            "morePowerOpenCount".into(),
            JsonValue::from(self.more_power_open_count),
        );
        root.insert("morale".into(), JsonValue::from(0u32));

        JsonValue::Object(root)
    }

    pub fn to_storage_json(&self) -> JsonValue {
        let mut root = BTreeMap::new();

        let map_u8_u64 = |m: &BTreeMap<u8, u64>| -> JsonValue {
            let mut o = BTreeMap::new();
            for (k, v) in m {
                o.insert(k.to_string(), JsonValue::from(*v));
            }
            JsonValue::Object(o)
        };
        let map_u8_u32 = |m: &BTreeMap<u8, u32>| -> JsonValue {
            let mut o = BTreeMap::new();
            for (k, v) in m {
                o.insert(k.to_string(), JsonValue::from(*v));
            }
            JsonValue::Object(o)
        };
        let nested_items = |outer: &BTreeMap<u8, BTreeMap<u8, BagItem>>| -> JsonValue {
            let mut o = BTreeMap::new();
            for (&pos, sub) in outer {
                if !sub.is_empty() {
                    let mut s = BTreeMap::new();
                    for (&k, it) in sub {
                        s.insert(k.to_string(), it.to_wire_json());
                    }
                    o.insert(pos.to_string(), JsonValue::Object(s));
                }
            }
            JsonValue::Object(o)
        };
        let single_items = |m: &BTreeMap<u8, BagItem>| -> JsonValue {
            let mut o = BTreeMap::new();
            for (&pos, it) in m {
                o.insert(pos.to_string(), it.to_wire_json());
            }
            JsonValue::Object(o)
        };

        root.insert("position".into(), map_u8_u64(&self.position));
        root.insert("grid_formation".into(), map_u8_u64(&self.grid_formation));
        root.insert("partner".into(), map_u8_u64(&self.partner));
        root.insert("buddy".into(), map_u8_u64(&self.buddy));
        root.insert(
            "buddy_level_info".into(),
            map_u8_u32(&self.buddy_level_info),
        );

        root.insert("arming".into(), nested_items(&self.arming));
        root.insert("spirit".into(), nested_items(&self.spirit));
        root.insert("yan_wu".into(), nested_items(&self.yan_wu));
        root.insert("brochure".into(), nested_items(&self.brochure));
        root.insert("soul_skill".into(), nested_items(&self.soul_skill));
        root.insert("soul_stone".into(), single_items(&self.soul_stone));
        root.insert("soul_jade".into(), single_items(&self.soul_jade));

        root.insert("bingfu".into(), map_u8_u32(&self.bingfu));
        root.insert("bingshu".into(), map_u8_u32(&self.bingshu));
        root.insert("attr_skins".into(), map_u8_u32(&self.attr_skins));
        root.insert(
            "godanimal".into(),
            Self::serialize_animal_or_protection_map(&self.godanimal),
        );
        root.insert(
            "topanimal".into(),
            Self::serialize_animal_or_protection_map(&self.topanimal),
        );
        root.insert(
            "protection".into(),
            Self::serialize_animal_or_protection_map(&self.protection),
        );
        root.insert(
            "joint_skill".into(),
            JsonValue::Array(
                self.joint_skill
                    .iter()
                    .map(|&v| JsonValue::from(v))
                    .collect(),
            ),
        );

        let mut ae = BTreeMap::new();
        for (k, v) in &self.attr_extra {
            ae.insert(k.to_string(), JsonValue::from(*v));
        }
        root.insert("attr_extra".into(), JsonValue::Object(ae));
        root.insert(
            "more_power_open_count".into(),
            JsonValue::from(self.more_power_open_count),
        );
        root.insert("formation_book".into(), self.formation_book.to_json());
        root.insert("soul_power".into(), JsonValue::from(self.soul_power));
        root.insert("soul_jade_qi".into(), JsonValue::from(self.soul_jade_qi));

        JsonValue::Object(root)
    }

    pub fn from_storage_json(val: &JsonValue) -> Result<Self> {
        let mut state = Self::new();
        state.partner.clear();
        let r = JsonObjReader::new(val, "FormationState")?;

        let read_u8_u64 = |opt: Option<&BTreeMap<String, JsonValue>>| -> Result<BTreeMap<u8, u64>> {
            let mut out = BTreeMap::new();
            if let Some(m) = opt {
                for (k, v) in m {
                    let slot: u8 = k
                        .parse()
                        .map_err(|_| PlayerError::Json(format!("invalid slot key '{k}'")))?;
                    let id = v
                        .as_u64()
                        .ok_or_else(|| PlayerError::Json("invalid u64 in formation map".into()))?;
                    out.insert(slot, id);
                }
            }
            Ok(out)
        };
        let read_u8_u32 = |opt: Option<&BTreeMap<String, JsonValue>>| -> Result<BTreeMap<u8, u32>> {
            let mut out = BTreeMap::new();
            if let Some(m) = opt {
                for (k, v) in m {
                    let slot: u8 = k
                        .parse()
                        .map_err(|_| PlayerError::Json(format!("invalid slot key '{k}'")))?;
                    let id = v
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| PlayerError::Json("invalid u32 in formation map".into()))?;
                    out.insert(slot, id);
                }
            }
            Ok(out)
        };
        let read_nested_items = |opt: Option<&BTreeMap<String, JsonValue>>,
                                 target: &mut BTreeMap<u8, BTreeMap<u8, BagItem>>|
         -> Result<()> {
            if let Some(outer) = opt {
                for (pos_str, slot_val) in outer {
                    let pos: u8 = pos_str
                        .parse()
                        .map_err(|_| PlayerError::Json("invalid nested slot pos".into()))?;
                    let slot_map = slot_val.as_object().ok_or_else(|| {
                        PlayerError::Json("nested equip slot must be object".into())
                    })?;
                    let dest = target.entry(pos).or_default();
                    for (sub_str, item_val) in slot_map {
                        let sub: u8 = sub_str
                            .parse()
                            .map_err(|_| PlayerError::Json("invalid nested sub-slot".into()))?;
                        if item_val.as_object().is_some_and(|o| !o.is_empty()) {
                            dest.insert(sub, BagItem::from_json(item_val)?);
                        }
                    }
                }
            }
            Ok(())
        };
        let read_single_items =
            |opt: Option<&BTreeMap<String, JsonValue>>| -> Result<BTreeMap<u8, BagItem>> {
                let mut out = BTreeMap::new();
                if let Some(m) = opt {
                    for (pos_str, item_val) in m {
                        let pos: u8 = pos_str
                            .parse()
                            .map_err(|_| PlayerError::Json("invalid single item slot".into()))?;
                        if item_val.as_object().is_some_and(|o| !o.is_empty()) {
                            out.insert(pos, BagItem::from_json(item_val)?);
                        }
                    }
                }
                Ok(out)
            };
        let read_animal_map = |opt: Option<&BTreeMap<String, JsonValue>>,
                               default_tid: u32|
         -> Result<BTreeMap<u8, Option<BagItem>>> {
            let mut out = BTreeMap::new();
            if let Some(m) = opt {
                for (k, v) in m {
                    let slot: u8 = k
                        .parse()
                        .map_err(|_| PlayerError::Json(format!("invalid animal slot '{k}'")))?;
                    if v.as_object().is_some_and(|o| !o.is_empty()) {
                        out.insert(slot, Some(BagItem::from_json(v)?));
                    } else if let Some(n) = v.as_u64() {
                        if n == 0 {
                            out.insert(slot, None);
                        } else {
                            // Migrate legacy bare u64 item_id into a valid BagItem dict!
                            let migrated = BagItem::new(0, n, default_tid, 1, 0)?;
                            out.insert(slot, Some(migrated));
                        }
                    }
                }
            }
            Ok(out)
        };

        state.position = read_u8_u64(r.opt_obj("position")?)?;
        state.position.retain(|_, &mut hid| hid > 0);
        state.grid_formation = read_u8_u64(r.opt_obj("grid_formation")?)?;
        state.partner = read_u8_u64(r.opt_obj("partner")?)?;
        state.buddy = read_u8_u64(r.opt_obj("buddy")?)?;
        state.buddy_level_info = read_u8_u32(r.opt_obj("buddy_level_info")?)?;
        state.bingfu = read_u8_u32(r.opt_obj("bingfu")?)?;
        state.bingshu = read_u8_u32(r.opt_obj("bingshu")?)?;
        state.attr_skins = read_u8_u32(r.opt_obj("attr_skins")?)?;
        state.godanimal = read_animal_map(r.opt_obj("godanimal")?, 604_001)?;
        state.topanimal = read_animal_map(r.opt_obj("topanimal")?, 5_400_001)?;
        state.protection = read_animal_map(r.opt_obj("protection")?, 604_001)?;

        read_nested_items(r.opt_obj("arming")?, &mut state.arming)?;
        read_nested_items(r.opt_obj("spirit")?, &mut state.spirit)?;
        read_nested_items(r.opt_obj("yan_wu")?, &mut state.yan_wu)?;
        read_nested_items(r.opt_obj("brochure")?, &mut state.brochure)?;
        read_nested_items(r.opt_obj("soul_skill")?, &mut state.soul_skill)?;
        state.soul_stone = read_single_items(r.opt_obj("soul_stone")?)?;
        state.soul_jade = read_single_items(r.opt_obj("soul_jade")?)?;

        if let Some(js) = r.opt_arr("joint_skill")? {
            for item in js {
                if let Some(id) = item.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    state.joint_skill.push(id);
                }
            }
        }

        if let Some(ae) = r.opt_obj("attr_extra")? {
            for (k, v) in ae {
                let attr_id: u32 = k
                    .parse()
                    .map_err(|_| PlayerError::Json("invalid attr_extra key".into()))?;
                let val = v
                    .as_i64()
                    .ok_or_else(|| PlayerError::Json("invalid attr_extra value".into()))?;
                state.attr_extra.insert(attr_id, val);
            }
        }

        state.more_power_open_count = r.opt_u32("more_power_open_count", 0)?;
        if let Some(fb) = r.raw().get("formation_book") {
            state.formation_book = FormationBookState::from_json(fb)?;
        }
        state.soul_power = r.opt_u32("soul_power", 0)?;
        state.soul_jade_qi = r.opt_u32("soul_jade_qi", 0)?;

        // Normalize any legacy {1, 4, 7} cells in stored snapshots onto valid {2, 3, 5, 6, 8, 9} cells
        if state
            .grid_formation
            .keys()
            .any(|&c| !is_valid_battle_grid_cell(c))
        {
            state.sync_grid_with_squad();
        }

        Ok(state)
    }
}
