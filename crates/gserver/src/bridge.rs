//! Type-faithful bridges between `gserver-protocol::Value`, `gserver-player::JsonValue`,
//! `gserver-gameplay::protocol::DtoValue`, and `gserver-rpc-catalog::Amf3Value`, plus state sync
//! between persistent `PlayerProfile` and live `GameplayEngine` `PlayerState`.

use std::collections::BTreeMap;

use gserver_gameplay::player::{
    BagCategory, BagItem as GameplayBagItem, HeroInstance as GameplayHero,
    PlayerState as GameplayPlayer,
};
use gserver_gameplay::protocol::{DtoValue, RpcRequest as GameplayRpcRequest};
use gserver_player::{CurrencyType, HeroInstance, JsonValue as PlayerJson, PlayerProfile};
use gserver_protocol::amf3::{AMF3_INT_MAX, AMF3_INT_MIN};
use gserver_protocol::{zlib_compress, Value};
use gserver_rpc_catalog::Amf3Value as CatalogAmf3;

#[inline]
pub fn i64_to_amf3(v: i64) -> Value {
    if (i64::from(AMF3_INT_MIN)..=i64::from(AMF3_INT_MAX)).contains(&v) {
        Value::Int(v as i32)
    } else {
        Value::Double(v as f64)
    }
}

#[inline]
pub fn u64_to_amf3(v: u64) -> Value {
    if v <= AMF3_INT_MAX as u64 {
        Value::Int(v as i32)
    } else {
        Value::Double(v as f64)
    }
}

/// Converts `gserver_player::JsonValue` wire projections into `gserver_protocol::Value`.
///
/// Empty objects `{}` map to `Value::empty_array()` (`[0x09, 0x01, 0x01]`), matching how
/// `Amf3Module.cpp` encodes empty Lua tables `{}`.
pub fn player_json_to_amf3(val: &PlayerJson) -> Value {
    match val {
        PlayerJson::Null => Value::Null,
        PlayerJson::Bool(b) => Value::Bool(*b),
        PlayerJson::Int(i) => i64_to_amf3(*i),
        PlayerJson::UInt(u) => u64_to_amf3(*u),
        PlayerJson::Float(f) => Value::Double(*f),
        PlayerJson::Str(s) => Value::String(s.clone()),
        PlayerJson::Array(arr) => {
            Value::dense_array(arr.iter().map(player_json_to_amf3).collect())
        }
        PlayerJson::Object(map) => {
            if map.is_empty() {
                Value::empty_array()
            } else {
                Value::map(
                    map.iter()
                        .map(|(k, v)| (k.clone(), player_json_to_amf3(v))),
                )
            }
        }
    }
}

/// Converts `gserver_rpc_catalog::Amf3Value` into `gserver_protocol::Value`.
pub fn catalog_amf3_to_protocol(val: &CatalogAmf3) -> Value {
    match val {
        CatalogAmf3::Undefined => Value::Undefined,
        CatalogAmf3::Null => Value::Null,
        CatalogAmf3::Bool(b) => Value::Bool(*b),
        CatalogAmf3::Int(i) => i64_to_amf3(*i),
        CatalogAmf3::Double(d) => Value::Double(*d),
        CatalogAmf3::String(s) => Value::String(s.clone()),
        CatalogAmf3::Date(ms) => Value::Date(*ms),
        CatalogAmf3::ByteArray(b) => Value::ByteString(b.clone()),
        CatalogAmf3::Array(arr) => {
            Value::dense_array(arr.iter().map(catalog_amf3_to_protocol).collect())
        }
        CatalogAmf3::Map(entries) => {
            if entries.is_empty() {
                Value::empty_array()
            } else {
                Value::map(
                    entries
                        .iter()
                        .map(|(k, v)| (k.clone(), catalog_amf3_to_protocol(v))),
                )
            }
        }
    }
}

/// Converts `gserver_protocol::Value` into `gserver_rpc_catalog::Amf3Value` for Lua invariant checks.
pub fn protocol_to_catalog_amf3(val: &Value) -> CatalogAmf3 {
    match val {
        Value::Undefined => CatalogAmf3::Undefined,
        Value::Null => CatalogAmf3::Null,
        Value::Bool(b) => CatalogAmf3::Bool(*b),
        Value::Int(i) => CatalogAmf3::Int(i64::from(*i)),
        Value::Double(d) => CatalogAmf3::Double(*d),
        Value::String(s) => CatalogAmf3::String(s.clone()),
        Value::ByteString(b) | Value::ByteArray(b) | Value::Xml(b) | Value::XmlDoc(b) => {
            CatalogAmf3::ByteArray(b.clone())
        }
        Value::Date(ms) => CatalogAmf3::Date(*ms),
        Value::Array { dense, assoc } => {
            if assoc.is_empty() {
                CatalogAmf3::Array(dense.iter().map(protocol_to_catalog_amf3).collect())
            } else {
                let mut m = BTreeMap::new();
                for (idx, v) in dense.iter().enumerate() {
                    m.insert((idx + 1).to_string(), protocol_to_catalog_amf3(v));
                }
                for (k, v) in assoc {
                    m.insert(k.clone(), protocol_to_catalog_amf3(v));
                }
                CatalogAmf3::Map(m)
            }
        }
        Value::Object(obj) => {
            let mut m = BTreeMap::new();
            for (k, v) in obj.iter() {
                m.insert(k.to_string(), protocol_to_catalog_amf3(v));
            }
            CatalogAmf3::Map(m)
        }
    }
}

/// Converts `gserver_protocol::Value` into `gserver_gameplay::protocol::DtoValue`.
pub fn amf3_to_dto(val: &Value) -> DtoValue {
    match val {
        Value::Undefined | Value::Null => DtoValue::Null,
        Value::Bool(b) => DtoValue::Bool(*b),
        Value::Int(i) => DtoValue::Int(i64::from(*i)),
        Value::Double(d) => {
            if d.fract() == 0.0 && *d >= (i64::MIN as f64) && *d <= (i64::MAX as f64) {
                DtoValue::Int(*d as i64)
            } else {
                DtoValue::Float(*d)
            }
        }
        Value::String(s) => DtoValue::Str(s.clone()),
        Value::ByteString(b) | Value::ByteArray(b) => match std::str::from_utf8(b) {
            Ok(s) => DtoValue::Str(s.to_string()),
            Err(_) => DtoValue::Bytes(b.clone()),
        },
        Value::Xml(b) | Value::XmlDoc(b) => DtoValue::Bytes(b.clone()),
        Value::Date(ms) => DtoValue::Float(*ms),
        Value::Array { dense, assoc } => {
            if assoc.is_empty() {
                DtoValue::Array(dense.iter().map(amf3_to_dto).collect())
            } else {
                let mut m = BTreeMap::new();
                for (i, item) in dense.iter().enumerate() {
                    m.insert((i + 1).to_string(), amf3_to_dto(item));
                }
                for (k, item) in assoc {
                    m.insert(k.clone(), amf3_to_dto(item));
                }
                DtoValue::Map(m)
            }
        }
        Value::Object(obj) => {
            let mut m = BTreeMap::new();
            for (k, v) in obj.iter() {
                m.insert(k.to_string(), amf3_to_dto(v));
            }
            DtoValue::Map(m)
        }
    }
}

/// Converts `gserver_gameplay::protocol::DtoValue` into `gserver_protocol::Value`.
///
/// When `compress_blobs` is `true`, nested `DtoValue::Bytes` payloads (used in
/// `chat.get*LastMsgs`, `arena.challenge.battleRecord`, and `battle.getRecord`) are
/// RFC 1950 Zlib-compressed and emitted as `Value::ByteString` (`0x06`), matching
/// `GameUtil:unzipData(blob)` in `ChatProxy.lua` and `FightProxy.lua`.
pub fn dto_to_amf3(val: &DtoValue, compress_blobs: bool) -> Value {
    match val {
        DtoValue::Null => Value::Null,
        DtoValue::Bool(b) => Value::Bool(*b),
        DtoValue::Int(i) => i64_to_amf3(*i),
        DtoValue::Float(f) => Value::Double(*f),
        DtoValue::Str(s) => Value::String(s.clone()),
        DtoValue::Bytes(raw) => {
            let payload = if compress_blobs {
                zlib_compress(raw)
            } else {
                raw.clone()
            };
            Value::ByteString(payload)
        }
        DtoValue::Array(arr) => {
            Value::dense_array(arr.iter().map(|v| dto_to_amf3(v, compress_blobs)).collect())
        }
        DtoValue::Map(map) => {
            if map.is_empty() {
                Value::empty_array()
            } else {
                Value::map(
                    map.iter()
                        .map(|(k, v)| (k.clone(), dto_to_amf3(v, compress_blobs))),
                )
            }
        }
    }
}

pub fn to_gameplay_request(req: &gserver_protocol::RpcRequest) -> GameplayRpcRequest {
    GameplayRpcRequest {
        method: req.method.clone(),
        args: req.args.iter().map(amf3_to_dto).collect(),
        callback_name: req.callback_name.clone(),
        token: req.token.clone(),
    }
}

/// Hydrates a `GameplayEngine` `PlayerState` from a persistent `PlayerProfile`.
pub fn profile_to_gameplay_state(profile: &PlayerProfile) -> GameplayPlayer {
    let mut gp = GameplayPlayer::new(profile.uid, profile.uname.clone(), profile.utid);
    gp.pid = profile.pid.clone();
    gp.level = profile.level;
    gp.exp = profile.exp;
    gp.vip = profile.vip;
    gp.vip_exp = profile.vip_exp;
    gp.gold = profile.currency(CurrencyType::Gold).min(i64::MAX as u64) as i64;
    gp.gold_bind = profile
        .currency(CurrencyType::GoldBind)
        .min(i64::MAX as u64) as i64;
    gp.copper = profile
        .currency(CurrencyType::Copper)
        .min(GameplayPlayer::COPPER_CAP as u64) as i64;
    gp.hero_soul = profile.currency(CurrencyType::HeroSoul);
    gp.prestige = profile.currency(CurrencyType::Prestige);
    gp.arena_coin = profile.currency(CurrencyType::Prestige);
    gp.execution = profile.currency(CurrencyType::Execution) as u32;
    gp.execution_time = profile.execution_time;
    gp.energy = profile.currency(CurrencyType::Energy) as u32;
    gp.energy_time = profile.energy_time;
    gp.dress_id = profile.dress_id;
    gp.server_id = format!("game{}", profile.group_id);
    gp.server_open_time = profile.server_open_time.clone();
    gp.gm_enabled = true;

    // Sync hero roster & 6-slot formation
    gp.roster.heroes.clear();
    gp.roster.formation = [0; 6];
    let mut max_hid = profile.protagonist_hid;
    for (&hid, hero) in &profile.heroes {
        if hid > max_hid {
            max_hid = hid;
        }
        let mut gh = GameplayHero::from_template(hid, hero.htid, hero.level.max(1));
        if let Some(tpl) = gserver_data::hero_by_htid(hero.htid) {
            gh.quality = u32::from(tpl.init_quality);
        }
        gh.exp = hero.exp;
        gh.evolve_level = hero.evolve_level;
        gh.upgrade_level = hero.upgrade_level;
        gh.awake_rank = hero.awake_rank;
        gh.awake_level = hero.awake_level;
        gh.dress_id = hero.dress_id;
        gh.recalculate_stats();
        gp.roster.heroes.insert(hid, gh);
    }
    gp.roster.set_next_hid(max_hid.max(profile.next_hid) + 1);

    for slot in 1..=6u8 {
        if let Some(&hid) = profile.formation.position.get(&slot) {
            if hid > 0 && gp.roster.heroes.contains_key(&hid) {
                gp.roster.formation[(slot - 1) as usize] = hid;
            }
        }
    }
    if gp.roster.formation[0] == 0 {
        gp.roster.formation[0] = profile.protagonist_hid;
    }

    // Sync inventory items into `GameplayPlayer` bag ranges
    gp.bag.slots.clear();
    let mut next_cat_gid: BTreeMap<BagCategory, u32> = BTreeMap::new();
    for sub in profile.inventory.bags.values() {
        for item in sub.values() {
            let cat = BagCategory::from_template_id(item.item_template_id);
            let gid = next_cat_gid.entry(cat).or_insert_with(|| cat.grid_start());
            let assigned_gid = *gid;
            *gid += 1;
            gp.bag.slots.insert(
                assigned_gid,
                GameplayBagItem {
                    gid: assigned_gid,
                    item_id: item.item_id,
                    item_template_id: item.item_template_id,
                    item_num: item.item_num,
                    item_time: item.item_time,
                },
            );
        }
    }

    gp
}

/// Synchronizes mutations made inside `GameplayEngine` (GM commands, gacha, shop, battle drops)
/// back into the persistent `PlayerProfile`.
pub fn sync_gameplay_back_to_profile(
    gp: &GameplayPlayer,
    profile: &mut PlayerProfile,
    now_ts: u64,
) {
    profile.level = gp.level.max(1);
    profile.exp = gp.exp;
    profile.vip = gp.vip;
    profile.vip_exp = gp.vip_exp;
    profile
        .currencies
        .insert(CurrencyType::Gold, gp.gold.max(0) as u64);
    profile
        .currencies
        .insert(CurrencyType::GoldBind, gp.gold_bind.max(0) as u64);
    profile
        .currencies
        .insert(CurrencyType::Copper, gp.copper.max(0) as u64);
    profile
        .currencies
        .insert(CurrencyType::HeroSoul, gp.hero_soul);
    profile
        .currencies
        .insert(CurrencyType::Prestige, gp.arena_coin.max(gp.prestige));
    profile
        .currencies
        .insert(CurrencyType::Execution, u64::from(gp.execution));
    profile
        .currencies
        .insert(CurrencyType::Energy, u64::from(gp.energy));
    profile.execution_time = gp.execution_time.max(now_ts);
    profile.energy_time = gp.energy_time.max(now_ts);

    // Sync protagonist + newly summoned or leveled heroes
    for (&hid, gh) in &gp.roster.heroes {
        let entry = profile
            .heroes
            .entry(hid)
            .or_insert_with(|| HeroInstance::new(hid, gh.htid, gh.level.max(1)));
        entry.htid = gh.htid;
        entry.level = gh.level.max(1);
        entry.exp = gh.exp;
        entry.evolve_level = gh.evolve_level;
        entry.upgrade_level = gh.upgrade_level;
        entry.awake_rank = gh.awake_rank;
        entry.awake_level = gh.awake_level;
        if hid >= profile.next_hid {
            profile.next_hid = hid + 1;
        }
    }

    // Sync 6-slot squad lineup
    for (idx, &hid) in gp.roster.formation.iter().enumerate() {
        let slot = (idx + 1) as u8;
        if hid > 0 && profile.heroes.contains_key(&hid) {
            profile.formation.position.insert(slot, hid);
        }
    }
    profile.formation.sync_grid_with_squad();

    // Sync bag item counts by template delta so `InventoryState` sub-bag invariants stay exact
    let mut gp_counts: BTreeMap<u32, u64> = BTreeMap::new();
    for item in gp.bag.slots.values() {
        *gp_counts.entry(item.item_template_id).or_insert(0) += u64::from(item.item_num);
    }

    let mut cur_counts: BTreeMap<u32, u64> = BTreeMap::new();
    for sub in profile.inventory.bags.values() {
        for item in sub.values() {
            *cur_counts.entry(item.item_template_id).or_insert(0) += u64::from(item.item_num);
        }
    }

    for (&tid, &target_qty) in &gp_counts {
        let cur_qty = cur_counts.get(&tid).copied().unwrap_or(0);
        if target_qty > cur_qty {
            let add_n = (target_qty - cur_qty).min(u64::from(u32::MAX)) as u32;
            let _ = profile.add_item(tid, add_n, now_ts);
        } else if target_qty < cur_qty {
            let sub_n = (cur_qty - target_qty).min(u64::from(u32::MAX)) as u32;
            let _ = profile.remove_item_by_template(tid, sub_n);
        }
    }
    for (&tid, &cur_qty) in &cur_counts {
        if !gp_counts.contains_key(&tid) && cur_qty > 0 {
            let _ = profile.remove_item_by_template(tid, cur_qty.min(u64::from(u32::MAX)) as u32);
        }
    }

    profile.recalculate_fight_force();
}
