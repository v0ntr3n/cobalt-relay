use std::collections::BTreeMap;

use crate::bag::{classify_template_id, BagItem, ItemClass};
use crate::error::{PlayerError, Result};
use crate::json::{JsonObjReader, JsonValue};
use crate::models::HeroInstance;
use crate::wire_helpers::{
    decode_u32_map, decode_u32_map_lossy, decode_u32_vec, decode_u64_map, encode_map,
    encode_u32_slice,
};

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

wire_record! {
    /// `FormationBookInfo.lua` state embedded in `formation.getPositionInfo`.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct FormationBookState {
        cur_use: u32 = opt_u32(0),
        lv_info: BTreeMap<u32, u32> = custom(
            encode_map(&this.lv_info),
            decode_u32_map_lossy(r.opt_obj("lv_info")?, "invalid formationBook lv_info key", 0)?
        ),
        reset_num: u32 = opt_u32(0),
    }
    encode(this, m) => pub fn to_json;
    decode(r, "FormationBookState") => pub fn from_json;
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
        hid != 0
            && (self.position.values().any(|&h| h == hid)
                || self.partner.values().any(|&h| h == hid)
                || self.buddy.values().any(|&h| h == hid))
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
            if !self.grid_formation.values().any(|&h| h == hid) {
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
            if let Some(&hid) = self.position.get(&slot).filter(|&&h| h > 0 && heroes.contains_key(&h)) {
                pos_obj.insert(slot.to_string(), JsonValue::from(hid));
                classic_squad.push(JsonValue::from(hid));
            } else {
                classic_squad.push(JsonValue::from(0u64));
            }
        }
        root.insert("position".into(), JsonValue::Object(pos_obj));

        let arr_little_friend: Vec<JsonValue> =
            self.partner.values().map(|&hid| JsonValue::from(hid)).collect();
        root.insert("partner".into(), encode_map(&self.partner));
        root.insert("buddy".into(), encode_map(&self.buddy));
        root.insert("buddyLevelInfo".into(), encode_map(&self.buddy_level_info));

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

        let empty_grid = |cols: usize| {
            JsonValue::Array(
                (0..6)
                    .map(|_| JsonValue::Array((0..cols).map(|_| JsonValue::empty_object()).collect()))
                    .collect(),
            )
        };
        for k in ["spirit", "brochure", "soulSkill", "soulStone", "soulJade"] {
            root.insert(k.into(), empty_grid(6));
        }
        root.insert("yanWu".into(), empty_grid(4));

        let all_heros = heroes
            .iter()
            .map(|(hid, hero)| (hid.to_string(), hero.to_vnsg2_hero_info()))
            .collect();
        root.insert("allHeros".into(), JsonValue::Object(all_heros));

        root.insert("bingfu".into(), encode_map(&self.bingfu));
        for k in [
            "bingshu",
            "jobSkill",
            "arrCounsellor",
            "attrSkins",
            "protection",
            "soulPowerGift",
            "soulJadeQiGift",
        ] {
            root.insert(k.into(), JsonValue::empty_object());
        }
        root.insert("godanimal".into(), encode_map(&self.godanimal));
        root.insert("topanimal".into(), encode_map(&self.topanimal));
        root.insert("jointSkill".into(), encode_u32_slice(&self.joint_skill));
        root.insert("morePowerOpenCount".into(), JsonValue::from(self.more_power_open_count));
        root.insert("formationBook".into(), self.formation_book.to_json());
        root.insert("soulPower".into(), JsonValue::from(self.soul_power));
        root.insert("soulJadeQi".into(), JsonValue::from(self.soul_jade_qi));
        root.insert("squad".into(), JsonValue::Array(classic_squad));
        root.insert("arrLittleFriend".into(), JsonValue::Array(arr_little_friend));
        root.insert("attrExtra".into(), encode_map(&self.attr_extra));

        JsonValue::Object(root)
    }

    pub fn to_storage_json(&self) -> JsonValue {
        let mut root = BTreeMap::new();
        root.insert("position".into(), encode_map(&self.position));
        root.insert("grid_formation".into(), encode_map(&self.grid_formation));
        root.insert("partner".into(), encode_map(&self.partner));
        root.insert("buddy".into(), encode_map(&self.buddy));
        root.insert("buddy_level_info".into(), encode_map(&self.buddy_level_info));

        let mut arming_obj = BTreeMap::new();
        for (&pos, eq_map) in &self.arming {
            if !eq_map.is_empty() {
                let slot_obj = eq_map
                    .iter()
                    .map(|(&eq_type, item)| (eq_type.to_string(), item.to_wire_json()))
                    .collect();
                arming_obj.insert(pos.to_string(), JsonValue::Object(slot_obj));
            }
        }
        root.insert("arming".into(), JsonValue::Object(arming_obj));
        root.insert("bingfu".into(), encode_map(&self.bingfu));
        root.insert("godanimal".into(), encode_map(&self.godanimal));
        root.insert("topanimal".into(), encode_map(&self.topanimal));
        root.insert("joint_skill".into(), encode_u32_slice(&self.joint_skill));
        root.insert("attr_extra".into(), encode_map(&self.attr_extra));
        root.insert("more_power_open_count".into(), JsonValue::from(self.more_power_open_count));
        root.insert("formation_book".into(), self.formation_book.to_json());
        root.insert("soul_power".into(), JsonValue::from(self.soul_power));
        root.insert("soul_jade_qi".into(), JsonValue::from(self.soul_jade_qi));
        JsonValue::Object(root)
    }

    pub fn from_storage_json(val: &JsonValue) -> Result<Self> {
        let mut state = Self::new();
        state.partner.clear();
        let r = JsonObjReader::new(val, "FormationState")?;
        let read_u8_u64 = |key: &str| -> Result<BTreeMap<u8, u64>> {
            decode_u64_map(r.opt_obj(key)?, "invalid slot key", "invalid u64 in formation map")
        };
        let read_u8_u32 = |key: &str| -> Result<BTreeMap<u8, u32>> {
            decode_u32_map(r.opt_obj(key)?, "invalid slot key", "invalid u32 in formation map")
        };

        state.position = read_u8_u64("position")?;
        state.position.retain(|_, &mut hid| hid > 0);
        state.grid_formation = read_u8_u64("grid_formation")?;
        state.partner = read_u8_u64("partner")?;
        state.buddy = read_u8_u64("buddy")?;
        state.buddy_level_info = read_u8_u32("buddy_level_info")?;
        state.bingfu = read_u8_u32("bingfu")?;
        state.godanimal = read_u8_u64("godanimal")?;
        state.topanimal = read_u8_u64("topanimal")?;

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

        state.joint_skill = decode_u32_vec(r.opt_arr("joint_skill")?);
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
