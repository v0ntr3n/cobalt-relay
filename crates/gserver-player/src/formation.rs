use std::collections::BTreeMap;

use crate::bag::{classify_template_id, BagItem, ItemClass};
use crate::error::{PlayerError, Result};
use crate::json::{JsonObjReader, JsonValue};
use crate::models::HeroInstance;

/// Default battle-grid slot mapping (`FormationData.lua`: `attack_sep_list = {3, 6, 9, 2, 5, 8}`).
/// Maps squad slot `1..=6` to 3x3 battle grid position `1..=9`.
pub const DEFAULT_SQUAD_TO_GRID: [u8; 6] = [3, 6, 9, 2, 5, 8];

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

/// Complete squad, 9-grid battle matrix, LittleFriend (`partner`), `buddy`, and 6-slot equipment state
/// (`HeroFormationData.lua` + `FormationData.lua`).
#[derive(Clone, Debug, PartialEq)]
pub struct FormationState {
    /// Active squad slots `1..=6 -> hid` (`hid > 0` ONLY; `HeroFormationData` crashes on `hid == 0`).
    pub position: BTreeMap<u8, u64>,
    /// 3x3 battle grid `1..=9 -> hid` (`FormationData.lua`: `formation.get`).
    pub grid_formation: BTreeMap<u8, u64>,
    /// LittleFriend /援军 slots `1..=6 -> hid` (`0` = unlocked empty slot).
    pub partner: BTreeMap<u8, u64>,
    /// Buddy /助战军 slots `1..=8 -> hid` (`0` = unlocked empty slot).
    pub buddy: BTreeMap<u8, u64>,
    pub buddy_level_info: BTreeMap<u8, u32>,
    /// Per-squad-slot (`1..=6`) equipped items keyed by `EquipSlotType` (`1..=9`).
    pub arming: BTreeMap<u8, BTreeMap<u8, BagItem>>,
    pub bingfu: BTreeMap<u8, u32>,
    pub godanimal: BTreeMap<u8, u64>,
    pub topanimal: BTreeMap<u8, u64>,
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
        for pos in 1..=6 {
            arming.insert(pos, BTreeMap::new());
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
            bingfu: BTreeMap::new(),
            godanimal: BTreeMap::new(),
            topanimal: BTreeMap::new(),
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

    pub fn squad_slot_of(&self, hid: u64) -> Option<u8> {
        self.position
            .iter()
            .find_map(|(&slot, &h)| if h == hid { Some(slot) } else { None })
    }

    /// Re-synchronize `grid_formation` so every hero in `position` occupies exactly one `1..=9` grid cell.
    pub fn sync_grid_with_squad(&mut self) {
        let active_hids: Vec<u64> = self.position.values().copied().collect();
        self.grid_formation
            .retain(|_, hid| active_hids.contains(hid));

        for (&slot, &hid) in &self.position {
            let already_placed = self.grid_formation.values().any(|&h| h == hid);
            if !already_placed {
                let preferred = DEFAULT_SQUAD_TO_GRID
                    .get(usize::from(slot.saturating_sub(1)))
                    .copied()
                    .unwrap_or(slot);
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.grid_formation.entry(preferred)
                {
                    e.insert(hid);
                } else {
                    for g in 1..=9 {
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

    /// Exact `formation.get` payload (`FormationData.lua`).
    pub fn to_vnsg2_formation_get(&self) -> JsonValue {
        let mut f_map = BTreeMap::new();
        for (&grid_pos, &hid) in &self.grid_formation {
            if hid > 0 {
                f_map.insert(grid_pos.to_string(), JsonValue::from(hid));
            }
        }
        let mut root = BTreeMap::new();
        root.insert("formation".into(), JsonValue::Object(f_map));
        JsonValue::Object(root)
    }

    /// Exact `formation.getPositionInfo` payload (`HeroFormationData.lua`),
    /// plus classic Babeltime 1st-gen compatibility aliases (`squad`, `arrLittleFriend`, `attrExtra`).
    pub fn to_vnsg2_position_info(&self, heroes: &BTreeMap<u64, HeroInstance>) -> JsonValue {
        let mut root = BTreeMap::new();

        // CRITICAL: position ONLY contains occupied slots with hid > 0 present in allHeros!
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

        let mut partner_obj = BTreeMap::new();
        let mut arr_little_friend = Vec::new();
        for (&slot, &hid) in &self.partner {
            partner_obj.insert(slot.to_string(), JsonValue::from(hid));
            arr_little_friend.push(JsonValue::from(hid));
        }
        root.insert("partner".into(), JsonValue::Object(partner_obj));

        let mut buddy_obj = BTreeMap::new();
        for (&slot, &hid) in &self.buddy {
            buddy_obj.insert(slot.to_string(), JsonValue::from(hid));
        }
        root.insert("buddy".into(), JsonValue::Object(buddy_obj));

        let mut buddy_lv_obj = BTreeMap::new();
        for (&slot, &lv) in &self.buddy_level_info {
            buddy_lv_obj.insert(slot.to_string(), JsonValue::from(lv));
        }
        root.insert("buddyLevelInfo".into(), JsonValue::Object(buddy_lv_obj));

        // 6-element arming array: slots 1..=6 -> {"1".."9": BagItem}
        let mut arming_arr = Vec::with_capacity(6);
        for slot in 1..=6u8 {
            let mut slot_obj = BTreeMap::new();
            if let Some(eq_map) = self.arming.get(&slot) {
                for (&eq_type, item) in eq_map {
                    slot_obj.insert(eq_type.to_string(), item.to_wire_json());
                }
            }
            arming_arr.push(JsonValue::Object(slot_obj));
        }
        root.insert("arming".into(), JsonValue::Array(arming_arr));

        // 6x6 empty slot arrays for spirit, brochure, soulSkill, soulStone, soulJade; 6x4 for yanWu
        let six_by_six = || {
            JsonValue::Array(
                (0..6)
                    .map(|_| JsonValue::Array((0..6).map(|_| JsonValue::empty_object()).collect()))
                    .collect(),
            )
        };
        let six_by_four = || {
            JsonValue::Array(
                (0..6)
                    .map(|_| JsonValue::Array((0..4).map(|_| JsonValue::empty_object()).collect()))
                    .collect(),
            )
        };
        root.insert("spirit".into(), six_by_six());
        root.insert("yanWu".into(), six_by_four());
        root.insert("brochure".into(), six_by_six());
        root.insert("soulSkill".into(), six_by_six());
        root.insert("soulStone".into(), six_by_six());
        root.insert("soulJade".into(), six_by_six());

        // allHeros map keyed by hid string
        let mut all_heros = BTreeMap::new();
        for (hid, hero) in heroes {
            all_heros.insert(hid.to_string(), hero.to_vnsg2_hero_info());
        }
        root.insert("allHeros".into(), JsonValue::Object(all_heros));

        // Mandatory non-nil tables in HeroFormationData:parseData
        let mut bingfu_obj = BTreeMap::new();
        for (&pos, &tid) in &self.bingfu {
            bingfu_obj.insert(pos.to_string(), JsonValue::from(tid));
        }
        root.insert("bingfu".into(), JsonValue::Object(bingfu_obj));
        root.insert("bingshu".into(), JsonValue::empty_object());
        root.insert("jobSkill".into(), JsonValue::empty_object());
        root.insert("arrCounsellor".into(), JsonValue::empty_object());
        root.insert("attrSkins".into(), JsonValue::empty_object());
        root.insert("protection".into(), JsonValue::empty_object());

        let mut godanimal_obj = BTreeMap::new();
        for (&pos, &id) in &self.godanimal {
            godanimal_obj.insert(pos.to_string(), JsonValue::from(id));
        }
        root.insert("godanimal".into(), JsonValue::Object(godanimal_obj));

        let mut topanimal_obj = BTreeMap::new();
        for (&pos, &id) in &self.topanimal {
            topanimal_obj.insert(pos.to_string(), JsonValue::from(id));
        }
        root.insert("topanimal".into(), JsonValue::Object(topanimal_obj));

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

        root.insert("position".into(), map_u8_u64(&self.position));
        root.insert("grid_formation".into(), map_u8_u64(&self.grid_formation));
        root.insert("partner".into(), map_u8_u64(&self.partner));
        root.insert("buddy".into(), map_u8_u64(&self.buddy));
        root.insert(
            "buddy_level_info".into(),
            map_u8_u32(&self.buddy_level_info),
        );

        let mut arming_obj = BTreeMap::new();
        for (&pos, eq_map) in &self.arming {
            if !eq_map.is_empty() {
                let mut slot_obj = BTreeMap::new();
                for (&eq_type, item) in eq_map {
                    slot_obj.insert(eq_type.to_string(), item.to_wire_json());
                }
                arming_obj.insert(pos.to_string(), JsonValue::Object(slot_obj));
            }
        }
        root.insert("arming".into(), JsonValue::Object(arming_obj));
        root.insert("bingfu".into(), map_u8_u32(&self.bingfu));
        root.insert("godanimal".into(), map_u8_u64(&self.godanimal));
        root.insert("topanimal".into(), map_u8_u64(&self.topanimal));
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

        state.position = read_u8_u64(r.opt_obj("position")?)?;
        state.position.retain(|_, &mut hid| hid > 0);
        state.grid_formation = read_u8_u64(r.opt_obj("grid_formation")?)?;
        state.partner = read_u8_u64(r.opt_obj("partner")?)?;
        state.buddy = read_u8_u64(r.opt_obj("buddy")?)?;
        state.buddy_level_info = read_u8_u32(r.opt_obj("buddy_level_info")?)?;
        state.bingfu = read_u8_u32(r.opt_obj("bingfu")?)?;
        state.godanimal = read_u8_u64(r.opt_obj("godanimal")?)?;
        state.topanimal = read_u8_u64(r.opt_obj("topanimal")?)?;

        if let Some(arming_obj) = r.opt_obj("arming")? {
            for (pos_str, slot_val) in arming_obj {
                let pos: u8 = pos_str
                    .parse()
                    .map_err(|_| PlayerError::Json("invalid arming pos".into()))?;
                let slot_map = slot_val
                    .as_object()
                    .ok_or_else(|| PlayerError::Json("arming slot must be object".into()))?;
                let target = state.arming.entry(pos).or_default();
                for (eq_str, item_val) in slot_map {
                    let eq_type: u8 = eq_str
                        .parse()
                        .map_err(|_| PlayerError::Json("invalid arming eq_type".into()))?;
                    target.insert(eq_type, BagItem::from_json(item_val)?);
                }
            }
        }

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

        Ok(state)
    }
}
