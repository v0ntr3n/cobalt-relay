//! Complete persistent `PlayerProfile` + `GameplayEngine` RPC handlers covering BOTH Base (`vnsg2`)
//! and Time Machine (`game_TM` / `tm*`) namespaces, plus client typo `emExchange.composeTreasure`:
//! - Economy, Rename, Copper/Stamina & VIP `0..=16` Gifts (`[tm]User.*`)
//! - Selling (`[tm]Bag.sellItems`, `[tm]Refinery.{refineryFragment,refinerySoul,refineryHeroLegend}`)
//! - Item Use & OpenBox (`[tm]Exchange.{useItemInBag,useItemInBagByTpl,getOpenBoxInfo,openRandgiftBoxByKey,openRandgiftBoxByGold}`)
//! - Sub-Bag & Hero Bag Expansion (`[tm]Bag.{openGridByGold,openGridByItem}`, `[tm]User.{openHeroBag,openHeroGrid}`)
//! - Furnace Decompose & Reborn (`[tm]Refinery.refinery*`, `[tm]Refinery.reborn*`, `resolveSpirit`, `[tm]SoulSkill.*`, `[tm]HeroPill.reborn`, `[tm]Dress.rebornDress`, `[tm]HeroSkin.*`)
//! - Fragment & Treasure Synthesis (`[tm]Exchange.compose*`, `quickCompose*`, `emExchange.composeTreasure`, `oneKeycomposeTreasure`)
//! - 6-Slot Squad, 3x3 Battle Grid, Partner, Buddy, Arming, Spirit, GodAnimal, TopAnimal & MorePower (`[tm]Formation.*`, `[tm]Spirit.*`)
//! - Hero Progression, Star, Wash, Pill, Dress, Forge, Spirit Explore & Quality Promotions (`[tm]Hero.*`, `[tm]Evolve.*`, `[tm]Herostar.*`, `[tm]Wash.*`, `[tm]Forge.*`, `[tm]ExploreSoul.*`, `*Gold.*`)

use std::collections::BTreeMap;

use gserver_data::GameDataStore;
use gserver_gameplay::engine::GameplayEngine;
use gserver_player::{
    BagDelta, BagItem, BagType, CurrencyType, EquipSlotType, GodArmAction, JsonValue as PlayerJson,
    PlayerError, PlayerProfile, PlayerStore, RefineryDecomposeKind, RefineryRebornKind,
    RefinerySellKind, SellBagEntry,
};
use gserver_protocol::{RpcRequest, S2cEnvelope, Value};

use crate::bridge::{
    dto_to_amf3, player_json_to_amf3, profile_to_gameplay_state_with_store, u64_to_amf3,
};

pub struct PlayerRpcSuccess {
    pub ret: Value,
    pub caller_pushes: Vec<S2cEnvelope>,
    pub peer_pushes: Vec<(u64, S2cEnvelope)>,
    pub refresh_roles_pid: Option<String>,
}

/// Normalizes `tm*` method prefixes (`tmFormation.*` -> `formation.*`, `tmHero.*` -> `hero.*`, etc.)
/// and the Lua client typo `emExchange.*` -> `exchange.*`, returning `(canonical_method, is_tm)`.
pub fn normalize_player_method(method: &str) -> (String, bool) {
    if let Some(rest) = method.strip_prefix("emExchange.") {
        return (format!("exchange.{rest}"), false);
    }
    if let Some(rest) = method.strip_prefix("tmEmExchange.") {
        return (format!("exchange.{rest}"), true);
    }
    if matches!(
        method,
        "tmActivityStarPlayOpen.getInfo"
            | "tmActivityStarPlayOpen.receiveGift"
            | "tmActivityStarPlaySignOpen.getInfo"
            | "tmActivityStarPlaySignOpen.receiveReward"
    ) {
        let stripped = method.strip_prefix("tm").unwrap();
        let mut chars = stripped.chars();
        let first = chars.next().unwrap().to_ascii_lowercase();
        return (format!("{first}{}", chars.as_str()), true);
    }
    if let Some(rest) = method.strip_prefix("tm") {
        let mut chars = rest.chars();
        if let Some(first) = chars.next() {
            if first.is_ascii_uppercase() {
                let mut out = String::with_capacity(rest.len());
                out.push(first.to_ascii_lowercase());
                out.push_str(chars.as_str());
                return (out, true);
            }
        }
    }
    (method.to_owned(), false)
}

pub(crate) fn map_player_err(err: PlayerError) -> String {
    match err {
        PlayerError::InsufficientCurrency { currency, .. } => {
            format!("insufficient_currency:{currency}")
        }
        PlayerError::InsufficientItemCount { .. } => "insufficient_item_count".to_string(),
        PlayerError::ItemNotFound(_) => "item_not_found".to_string(),
        PlayerError::BagFull { bag_type, .. } => format!("bag_full:{bag_type}"),
        PlayerError::HeroNotFound(_) => "hero_not_found".to_string(),
        PlayerError::HeroBagFull { .. } => "hero_bag_full".to_string(),
        PlayerError::ProtagonistCannotBeRemoved { .. } => {
            "protagonist_cannot_be_removed".to_string()
        }
        PlayerError::DuplicateHeroTemplateInSquad { .. } => "duplicate_htid_in_squad".to_string(),
        PlayerError::InvalidFormationSlot(_) => "invalid_formation_slot".to_string(),
        PlayerError::EquipTypeMismatch { .. } => "equip_type_mismatch".to_string(),
        PlayerError::InvalidParameter(msg) => {
            if msg.contains("protagonist level") {
                "protagonist_cannot_level_directly".to_string()
            } else if msg.contains("locked hero") {
                "locked_hero_cannot_sacrifice".to_string()
            } else if msg.contains("deployed hero") {
                "deployed_hero_cannot_sacrifice".to_string()
            } else if msg.contains("locked") {
                "item_is_locked".to_string()
            } else {
                format!("invalid_parameter:{msg}")
            }
        }
        other => format!("player_error:{other}"),
    }
}

pub(crate) fn build_user_update_env(profile: &PlayerProfile, is_tm: bool, now_i64: i64) -> S2cEnvelope {
    let cb = if is_tm {
        "tmUser.updateUser"
    } else {
        "user.updateUser"
    };
    let payload = player_json_to_amf3(&profile.to_vnsg2_user_update());
    S2cEnvelope::push(cb, payload).with_time(now_i64)
}

pub(crate) fn build_bag_delta_env(delta: &BagDelta, is_tm: bool, now_i64: i64) -> Option<S2cEnvelope> {
    if delta.is_empty() {
        return None;
    }
    let cb = if is_tm { "tmBag.update" } else { "bag.update" };
    let payload = player_json_to_amf3(&delta.to_bag_update_payload());
    Some(S2cEnvelope::push(cb, payload).with_time(now_i64))
}

fn build_bag_update_env(
    upserted: &[BagItem],
    removed_gids: &[u32],
    is_tm: bool,
    now_i64: i64,
) -> Option<S2cEnvelope> {
    if upserted.is_empty() && removed_gids.is_empty() {
        return None;
    }
    let mut delta = BagDelta::new();
    for &gid in removed_gids {
        delta.record_remove(gid);
    }
    delta.record_items(upserted.iter().cloned());
    build_bag_delta_env(&delta, is_tm, now_i64)
}

pub(crate) fn build_new_heroes_env(
    profile: &PlayerProfile,
    hids: &[u64],
    is_tm: bool,
    now_i64: i64,
) -> Option<S2cEnvelope> {
    if hids.is_empty() {
        return None;
    }
    let mut entries = Vec::new();
    for &hid in hids {
        if let Some(hero) = profile.heroes.get(&hid) {
            entries.push((
                hid.to_string(),
                player_json_to_amf3(&hero.to_vnsg2_hero_info()),
            ));
        }
    }
    if entries.is_empty() {
        return None;
    }
    let cb = if is_tm { "re.tmHero.new" } else { "re.hero.new" };
    Some(S2cEnvelope::push(cb, Value::map(entries)).with_time(now_i64))
}

pub(crate) fn resync_gameplay_with_store(
    store: &PlayerStore,
    gameplay: &mut GameplayEngine,
    data_store: &GameDataStore,
    uid: u64,
) {
    if let Some(profile) = store.get_player(uid) {
        let mut gp = profile_to_gameplay_state_with_store(profile, Some(data_store));
        if let Some(prev) = gameplay.players.get(&uid) {
            gp.online_accum_secs = prev.online_accum_secs;
            gp.chat_ban_until = prev.chat_ban_until;
        }
        gameplay.register_player(gp);
    }
}

/// Reads an integer argument trying 1-based `args[idx]` first, or `arg_zero` when `idx == 0` and `args` is empty.
pub(crate) fn arg_i64(req: &RpcRequest, idx: usize) -> Option<i64> {
    if let Some(v) = req.args.get(idx) {
        if let Some(n) = val_as_i64_loose(v) {
            return Some(n);
        }
    }
    if idx == 0 {
        if let Some(z) = req.arg_zero() {
            return val_as_i64_loose(z);
        }
    }
    req.arg_at_lua0(idx + 1).and_then(val_as_i64_loose)
}

pub(crate) fn val_as_i64_loose(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

/// Extracts all positive `u64` IDs from a Lua 0-indexed table `{[0]=id0, [1]=id1}`, 1-based array `[id1, id2]`,
/// nested array `[[id1], [id2]]`, or scalar `id`!
fn collect_u64_ids_from_value(val: &Value, out: &mut Vec<u64>) {
    if let Some(n) = val_as_i64_loose(val) {
        if n > 0 {
            out.push(n as u64);
        }
        return;
    }
    match val {
        Value::Array { dense, assoc } => {
            // Preserve 0-index first if present
            if let Some((_, v0)) = assoc.iter().find(|(k, _)| k == "0") {
                collect_u64_ids_from_value(v0, out);
            }
            for item in dense {
                collect_u64_ids_from_value(item, out);
            }
            let mut numeric_assoc: Vec<(usize, &Value)> = assoc
                .iter()
                .filter(|(k, _)| k.as_str() != "0")
                .filter_map(|(k, v)| k.parse::<usize>().ok().map(|idx| (idx, v)))
                .collect();
            numeric_assoc.sort_by_key(|(idx, _)| *idx);
            for (_, v) in numeric_assoc {
                collect_u64_ids_from_value(v, out);
            }
        }
        Value::Object(obj) => {
            let mut keyed: Vec<(usize, &Value)> = obj
                .iter()
                .filter_map(|(k, v)| k.parse::<usize>().ok().map(|idx| (idx, v)))
                .collect();
            keyed.sort_by_key(|(idx, _)| *idx);
            if !keyed.is_empty() {
                for (_, v) in keyed {
                    collect_u64_ids_from_value(v, out);
                }
            } else {
                for (_, v) in obj.iter() {
                    collect_u64_ids_from_value(v, out);
                }
            }
        }
        _ => {}
    }
}

fn parse_target_id_list(req: &RpcRequest) -> Vec<u64> {
    let mut ids = Vec::new();
    if let Some(z) = req.arg_zero() {
        collect_u64_ids_from_value(z, &mut ids);
    }
    for v in &req.args {
        collect_u64_ids_from_value(v, &mut ids);
    }
    if ids.is_empty() {
        if let Some(raw) = &req.raw_args {
            collect_u64_ids_from_value(raw, &mut ids);
        }
    }
    ids
}

/// Parses a single sell tuple `{ [0]=gid, [1]=item_id, [2]=num }` OR `[gid, item_id, num]` OR `[gid]`.
fn parse_single_sell_entry(val: &Value) -> Option<SellBagEntry> {
    if let Some(gid) = val_as_i64_loose(val).filter(|&g| g > 0) {
        return Some(SellBagEntry::gid_only(gid as u32));
    }
    match val {
        Value::Array { dense, assoc } => {
            if let Some((_, v0)) = assoc.iter().find(|(k, _)| k == "0") {
                let gid = val_as_i64_loose(v0).filter(|&g| g > 0)? as u32;
                let item_id = dense
                    .first()
                    .or_else(|| assoc.iter().find(|(k, _)| k == "1").map(|(_, v)| v))
                    .and_then(val_as_i64_loose)
                    .filter(|&id| id > 0)
                    .map(|id| id as u64);
                let num = dense
                    .get(1)
                    .or_else(|| assoc.iter().find(|(k, _)| k == "2").map(|(_, v)| v))
                    .and_then(val_as_i64_loose)
                    .filter(|&n| n > 0)
                    .map(|n| n as u32);
                Some(SellBagEntry { gid, item_id, num })
            } else if !dense.is_empty() {
                let gid = val_as_i64_loose(&dense[0]).filter(|&g| g > 0)? as u32;
                let (item_id, num) = if dense.len() >= 3 {
                    (
                        val_as_i64_loose(&dense[1])
                            .filter(|&id| id > 0)
                            .map(|id| id as u64),
                        val_as_i64_loose(&dense[2])
                            .filter(|&n| n > 0)
                            .map(|n| n as u32),
                    )
                } else if dense.len() == 2 {
                    (
                        None,
                        val_as_i64_loose(&dense[1])
                            .filter(|&n| n > 0)
                            .map(|n| n as u32),
                    )
                } else {
                    (None, None)
                };
                Some(SellBagEntry { gid, item_id, num })
            } else {
                None
            }
        }
        Value::Object(obj) => {
            if let Some(v0) = obj.get("0") {
                let gid = val_as_i64_loose(v0).filter(|&g| g > 0)? as u32;
                let item_id = obj
                    .get("1")
                    .and_then(val_as_i64_loose)
                    .filter(|&id| id > 0)
                    .map(|id| id as u64);
                let num = obj
                    .get("2")
                    .and_then(val_as_i64_loose)
                    .filter(|&n| n > 0)
                    .map(|n| n as u32);
                Some(SellBagEntry { gid, item_id, num })
            } else if let Some(v1) = obj.get("1") {
                let gid = val_as_i64_loose(v1).filter(|&g| g > 0)? as u32;
                let item_id = obj
                    .get("2")
                    .and_then(val_as_i64_loose)
                    .filter(|&id| id > 0)
                    .map(|id| id as u64);
                let num = obj
                    .get("3")
                    .and_then(val_as_i64_loose)
                    .filter(|&n| n > 0)
                    .map(|n| n as u32);
                Some(SellBagEntry { gid, item_id, num })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_sell_bag_entries(req: &RpcRequest) -> Result<Vec<SellBagEntry>, String> {
    let mut entries = Vec::new();

    // Case 1: 0-indexed flat `{ [0] = gid, [1] = item_id, [2] = num }` directly on `args`
    if let Some(z) = req.arg_zero() {
        if let Some(gid) = val_as_i64_loose(z).filter(|&g| g > 0) {
            let item_id = req
                .arg_at_lua0(1)
                .and_then(val_as_i64_loose)
                .filter(|&id| id > 0)
                .map(|id| id as u64);
            let num = req
                .arg_at_lua0(2)
                .and_then(val_as_i64_loose)
                .filter(|&n| n > 0)
                .map(|n| n as u32);
            return Ok(vec![SellBagEntry {
                gid: gid as u32,
                item_id,
                num,
            }]);
        }
        if let Some(e) = parse_single_sell_entry(z) {
            entries.push(e);
        }
    }

    // Case 2: `args[0]` is a table of sell entries `[[gid, item_id, num], ...]` or `{[0]=entry0, [1]=entry1}`
    if let Some(first) = req.args.first() {
        match first {
            Value::Array { dense, assoc } => {
                if let Some((_, v0)) = assoc.iter().find(|(k, _)| k == "0") {
                    if let Some(e0) = parse_single_sell_entry(v0) {
                        if val_as_i64_loose(v0).is_none() {
                            entries.push(e0);
                        } else if let Some(single) = parse_single_sell_entry(first) {
                            entries.push(single);
                            return Ok(entries);
                        }
                    }
                }
                for item in dense {
                    let e = parse_single_sell_entry(item)
                        .ok_or_else(|| "invalid_sell_entry".to_string())?;
                    entries.push(e);
                }
                if !entries.is_empty() {
                    return Ok(entries);
                }
            }
            Value::Object(obj) => {
                if obj.get("0").and_then(val_as_i64_loose).is_some() {
                    if let Some(single) = parse_single_sell_entry(first) {
                        entries.push(single);
                        return Ok(entries);
                    }
                }
                let mut keyed: Vec<(usize, &Value)> = obj
                    .iter()
                    .filter_map(|(k, v)| k.parse::<usize>().ok().map(|idx| (idx, v)))
                    .collect();
                keyed.sort_by_key(|(idx, _)| *idx);
                for (_, item) in keyed {
                    let e = parse_single_sell_entry(item)
                        .ok_or_else(|| "invalid_sell_entry".to_string())?;
                    entries.push(e);
                }
                if !entries.is_empty() {
                    return Ok(entries);
                }
            }
            _ => {
                // Scalar `(gid, item_id, num)` (`bag.sellItem`)
                let gid = val_as_i64_loose(first).unwrap_or(0);
                let num = req
                    .args
                    .get(2)
                    .or_else(|| req.args.get(1))
                    .and_then(val_as_i64_loose)
                    .unwrap_or(1);
                if gid <= 0 || num <= 0 {
                    return Err("invalid_sell_entry".to_string());
                }
                let item_id = if req.args.len() >= 3 {
                    req.args
                        .get(1)
                        .and_then(val_as_i64_loose)
                        .filter(|&id| id > 0)
                        .map(|id| id as u64)
                } else {
                    None
                };
                entries.push(SellBagEntry {
                    gid: gid as u32,
                    item_id,
                    num: Some(num as u32),
                });
            }
        }
    }

    if entries.is_empty() {
        Err("empty_sell_list".to_string())
    } else {
        Ok(entries)
    }
}

fn resolve_bag_type_from_val(v: &Value) -> Option<BagType> {
    if let Some(s) = v.as_str() {
        return BagType::from_key(s);
    }
    let id = val_as_i64_loose(v)?;
    if id <= 0 {
        return None;
    }
    let u = id as u32;
    // Match `bag_v2/bag.json` row ID (`2..=43`) or legacy `1..=4`
    if let Some(bt) = BagType::ALL.into_iter().find(|b| b.bag_db_id() == u) {
        return Some(bt);
    }
    match u {
        1 => Some(BagType::Arm),
        _ => None,
    }
}

/// Parses `(BagType, steps, optional_item_tid)` for `bag.openGridByGold` / `bag.openGridByItem`,
/// supporting BOTH `BagProxy.lua` (`[bag_db_id_or_key, open_steps_or_grids]`) AND legacy `[add_grids, bag_key]`.
fn parse_bag_expand_args(req: &RpcRequest) -> (BagType, u32, Option<u32>) {
    let a0 = req.arg_zero().or_else(|| req.args.first());
    let a1 = if req.arg_zero().is_some() {
        req.arg_at_lua0(1)
    } else {
        req.args.get(1)
    };
    let a2 = if req.arg_zero().is_some() {
        req.arg_at_lua0(2)
    } else {
        req.args.get(2)
    };

    // Legacy `[add_grids, "arm"]` when `a1` is a string bag key
    if let Some(bt) = a1.and_then(|v| v.as_str().and_then(BagType::from_key)) {
        let raw_n = a0.and_then(val_as_i64_loose).unwrap_or(5).max(1) as u32;
        let step_sz = bt.buy_storage_step().unwrap_or(5).max(1);
        let steps = if raw_n >= step_sz {
            (raw_n / step_sz).max(1)
        } else {
            raw_n
        };
        let item_tid = a2
            .and_then(val_as_i64_loose)
            .filter(|&t| t >= 10_000)
            .map(|t| t as u32);
        return (bt, steps, item_tid);
    }

    // Standard `BagProxy.lua`: `a0` = `bag_type` (`"arm"` or `bag_db_id` `2..=43`), `a1` = `steps` (or `[item_tid, steps]`)
    if let Some(bt) = a0.and_then(resolve_bag_type_from_val) {
        let n1 = a1.and_then(val_as_i64_loose).unwrap_or(1).max(1) as u32;
        if n1 >= 10_000 {
            let steps = a2.and_then(val_as_i64_loose).unwrap_or(1).max(1) as u32;
            return (bt, steps, Some(n1));
        }
        return (bt, n1.clamp(1, 20), None);
    }

    // Fallback `[add_grids, numeric_bag_code]`
    let bt = a1
        .and_then(resolve_bag_type_from_val)
        .unwrap_or(BagType::Arm);
    let raw_n = a0.and_then(val_as_i64_loose).unwrap_or(1).max(1) as u32;
    let step_sz = bt.buy_storage_step().unwrap_or(5).max(1);
    let steps = if raw_n >= step_sz {
        (raw_n / step_sz).max(1)
    } else {
        raw_n
    };
    (bt, steps, None)
}

fn extract_option_idx(val: Option<&Value>) -> Option<usize> {
    let v = val?;
    if let Some(n) = val_as_i64_loose(v) {
        if n >= 0 {
            return Some(n as usize);
        }
    }
    match v {
        Value::Array { dense, assoc } => {
            if let Some((_, v0)) = assoc.iter().find(|(k, _)| k == "0") {
                return val_as_i64_loose(v0)
                    .filter(|&n| n >= 0)
                    .map(|n| n as usize);
            }
            dense
                .first()
                .and_then(val_as_i64_loose)
                .filter(|&n| n >= 0)
                .map(|n| n as usize)
        }
        Value::Object(obj) => obj
            .get("0")
            .or_else(|| obj.get("1"))
            .or_else(|| obj.get("chooseIdx"))
            .and_then(val_as_i64_loose)
            .filter(|&n| n >= 0)
            .map(|n| n as usize),
        _ => None,
    }
}

fn parse_squad_hid_and_slot(args: &[Value], allow_zero_hid: bool) -> Result<(u64, u8), String> {
    let a = args
        .first()
        .and_then(val_as_i64_loose)
        .ok_or_else(|| "missing_first_arg".to_string())?;
    let b = args
        .get(1)
        .and_then(val_as_i64_loose)
        .ok_or_else(|| "missing_second_arg".to_string())?;
    if a < 0 || b < 0 {
        return Err("negative_formation_arg".to_string());
    }
    let ua = a as u64;
    let ub = b as u64;

    if (1..=6).contains(&ub) && (ua > 6 || (allow_zero_hid && ua == 0)) {
        Ok((ua, ub as u8))
    } else if (1..=6).contains(&ua) && (ub > 6 || (allow_zero_hid && ub == 0)) {
        Ok((ub, ua as u8))
    } else {
        Err("invalid_formation_slot".to_string())
    }
}

fn parse_partner_hid_and_slot(args: &[Value], allow_zero_hid: bool) -> Result<(u64, u8), String> {
    let a = args
        .first()
        .and_then(val_as_i64_loose)
        .ok_or_else(|| "missing_first_arg".to_string())?;
    let b = args
        .get(1)
        .and_then(val_as_i64_loose)
        .ok_or_else(|| "missing_second_arg".to_string())?;
    if a < 0 || b < 0 {
        return Err("negative_formation_arg".to_string());
    }
    let ua = a as u64;
    let ub = b as u64;
    if (1..=12).contains(&ub) && (ua > 12 || (allow_zero_hid && ua == 0)) {
        Ok((ua, ub as u8))
    } else if (1..=12).contains(&ua) && (ub > 12 || (allow_zero_hid && ub == 0)) {
        Ok((ub, ua as u8))
    } else {
        Err("invalid_formation_slot".to_string())
    }
}

fn parse_buddy_hid_and_slot(args: &[Value], allow_zero_hid: bool) -> Result<(u64, u8), String> {
    let a = args
        .first()
        .and_then(val_as_i64_loose)
        .ok_or_else(|| "missing_first_arg".to_string())?;
    let b = args
        .get(1)
        .and_then(val_as_i64_loose)
        .ok_or_else(|| "missing_second_arg".to_string())?;
    if a < 0 || b < 0 {
        return Err("negative_formation_arg".to_string());
    }
    let ua = a as u64;
    let ub = b as u64;
    if (1..=8).contains(&ub) && (ua > 8 || (allow_zero_hid && ua == 0)) {
        Ok((ua, ub as u8))
    } else if (1..=8).contains(&ua) && (ub > 8 || (allow_zero_hid && ub == 0)) {
        Ok((ub, ua as u8))
    } else {
        Err("invalid_formation_slot".to_string())
    }
}

fn resolve_bag_gid(profile: &PlayerProfile, gid_or_item_id: u64) -> Option<u32> {
    if let Ok(gid_u32) = u32::try_from(gid_or_item_id) {
        if profile.inventory.get_by_gid(gid_u32).is_some() {
            return Some(gid_u32);
        }
    }
    profile
        .inventory
        .get_by_item_id(gid_or_item_id)
        .map(|(_, it)| it.gid)
}

fn resolve_any_item_id(profile: &PlayerProfile, item_id_or_gid: u64) -> Option<u64> {
    // Check equipped items first
    for m in profile.formation.arming.values() {
        for it in m.values() {
            if it.item_id == item_id_or_gid || u64::from(it.gid) == item_id_or_gid {
                return Some(it.item_id);
            }
        }
    }
    for m in profile.formation.spirit.values() {
        for it in m.values() {
            if it.item_id == item_id_or_gid || u64::from(it.gid) == item_id_or_gid {
                return Some(it.item_id);
            }
        }
    }
    if let Some((_, it)) = profile.inventory.get_by_item_id(item_id_or_gid) {
        return Some(it.item_id);
    }
    if let Ok(gid) = u32::try_from(item_id_or_gid) {
        if let Some(it) = profile.inventory.get_by_gid(gid) {
            return Some(it.item_id);
        }
    }
    None
}

fn read_u32_set_from_va(profile: &PlayerProfile, key: &str) -> Vec<u32> {
    profile
        .va_config
        .get(key)
        .and_then(|v| match v {
            PlayerJson::Array(arr) => Some(
                arr.iter()
                    .filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn write_u32_set_to_va(profile: &mut PlayerProfile, key: &str, vals: &[u32]) {
    let arr = vals.iter().map(|&n| PlayerJson::from(n)).collect();
    let _ = profile.set_user_config(key, PlayerJson::Array(arr));
}

pub fn vip_gift_gold_cost(vip_lv: u32) -> u64 {
    match vip_lv {
        0 => 88,
        1 => 188,
        2 => 288,
        3 => 388,
        4 => 488,
        5 => 688,
        6 => 888,
        7 => 1_088,
        8 => 1_288,
        9 => 1_688,
        10 => 2_088,
        11 => 2_688,
        12 => 3_288,
        13 => 3_888,
        14 => 4_688,
        15 => 5_688,
        _ => 6_688,
    }
}

/// Authentic `vip.db` `vip_gift` `(price_type, tid, qty)` reward rows for VIP `0..=16`.
pub fn vip_gift_reward_triplets(vip_lv: u32) -> &'static [(u32, u32, u64)] {
    match vip_lv {
        0 => &[(4, 33_011, 1), (4, 22_102, 1), (4, 22_202, 1), (1, 0, 50_000)],
        1 => &[(4, 16_012, 2), (4, 22_202, 2), (1, 0, 100_000)],
        2 => &[(4, 124_003, 1), (4, 22_102, 2), (4, 17_003, 5), (1, 0, 150_000)],
        3 => &[(4, 134_003, 1), (4, 22_202, 3), (4, 181_001, 5), (1, 0, 200_000)],
        4 => &[(4, 144_003, 1), (4, 22_102, 3), (4, 17_004, 10), (1, 0, 300_000)],
        5 => &[(4, 114_003, 1), (4, 22_202, 5), (4, 181_001, 10), (1, 0, 500_000)],
        6 => &[(4, 27_215, 1), (4, 22_102, 5), (4, 10_014, 200), (4, 33_002, 5)],
        7 => &[(4, 125_001, 1), (4, 22_202, 8), (4, 10_012, 200), (1, 0, 800_000)],
        8 => &[(4, 135_001, 1), (4, 22_102, 8), (4, 10_014, 400), (4, 33_002, 10)],
        9 => &[(4, 115_001, 1), (4, 145_001, 1), (4, 22_202, 10), (4, 10_012, 400), (1, 0, 1_000_000)],
        10 => &[(4, 27_501, 1), (4, 28_006, 1), (4, 22_102, 10), (4, 10_204, 50), (4, 33_003, 10)],
        11 => &[(4, 28_202, 1), (4, 22_202, 12), (4, 15_005, 2_000), (1, 0, 1_500_000)],
        12 => &[(4, 135_004, 1), (4, 125_004, 1), (4, 22_102, 12), (4, 10_204, 100), (4, 33_003, 20)],
        13 => &[(4, 145_004, 1), (4, 115_004, 1), (4, 22_202, 15), (4, 15_005, 3_000), (1, 0, 2_000_000)],
        14 => &[(4, 27_506, 2), (4, 28_006, 1), (4, 22_102, 15), (4, 10_014, 5_000), (1, 0, 3_000_000)],
        15 => &[(4, 204_019, 1), (4, 28_006, 1), (4, 22_202, 20), (4, 181_002, 30), (1, 0, 4_000_000)],
        _ => &[(4, 28_301, 1), (4, 22_102, 20), (4, 10_014, 8_000), (1, 0, 5_000_000)],
    }
}

/// Configured `starPlay_gift` (`Quà Điêu Thuyền` / `ActStarCodeData.lua` + `ActCodeCell.lua`) tier.
struct ActStarGiftTier {
    id: u32,
    req_level: u32,
    gold: u64,
    copper: u64,
    items: &'static [(u32, u32)],
}

const ACT_STAR_GIFT_TIERS: &[ActStarGiftTier] = &[
    ActStarGiftTier {
        id: 1,
        req_level: 1,
        gold: 188,
        copper: 50_000,
        items: &[(10_010, 20), (50_406, 10)],
    },
    ActStarGiftTier {
        id: 2,
        req_level: 10,
        gold: 288,
        copper: 100_000,
        items: &[(22_102, 2), (50_406, 15)],
    },
    ActStarGiftTier {
        id: 3,
        req_level: 20,
        gold: 388,
        copper: 150_000,
        items: &[(16_012, 3), (50_406, 20)],
    },
    ActStarGiftTier {
        id: 4,
        req_level: 30,
        gold: 588,
        copper: 200_000,
        items: &[(22_202, 3), (50_406, 25)],
    },
    ActStarGiftTier {
        id: 5,
        req_level: 40,
        gold: 888,
        copper: 300_000,
        items: &[(17_002, 10), (50_406, 30)],
    },
    ActStarGiftTier {
        id: 6,
        req_level: 50,
        gold: 1_288,
        copper: 500_000,
        items: &[(30_003, 2), (15_201, 5)],
    },
    ActStarGiftTier {
        id: 7,
        req_level: 60,
        gold: 1_888,
        copper: 800_000,
        items: &[(31_001, 1), (14_085, 5)],
    },
];

const ACT_STAR_SIGN_LEVELS: &[u32] = &[5, 10, 20, 30, 40, 50, 60];

pub(crate) fn parse_positive_u32_arg(val: Option<&Value>) -> Option<u32> {
    let v = val?;
    val_as_i64_loose(v)
        .filter(|&n| n > 0)
        .and_then(|n| u32::try_from(n).ok())
}

fn decompose_kind_for_method(m: &str) -> Option<(RefineryDecomposeKind, bool, bool)> {
    let (base, preview) = if let Some(stripped) = m.strip_suffix("Preview") {
        (stripped, true)
    } else {
        (m, false)
    };
    let (kind, ret_triplets) = match base {
        "refinery.refineryHeroes" | "refinery.refineryHeros" | "refinery.refineryHero" => {
            (RefineryDecomposeKind::Heroes, false)
        }
        "refinery.refineryArm" => (RefineryDecomposeKind::Arm, false),
        "refinery.refineryTreasure" => (RefineryDecomposeKind::Treasure, false),
        "refinery.refineryAwake" | "refinery.refineryAwakeItem" => {
            (RefineryDecomposeKind::Awake, false)
        }
        "refinery.refineryPet" => (RefineryDecomposeKind::Pet, false),
        "refinery.refinerySpirit" => (RefineryDecomposeKind::Spirit, false),
        "refinery.resolveSpirit" | "spirit.resolveSpirit" => {
            (RefineryDecomposeKind::ResolveSpirit, false)
        }
        "refinery.refineryGodArm" => (RefineryDecomposeKind::GodArm, false),
        "refinery.refineryGodAnimal" | "refinery.refineryTopAnimal" => {
            (RefineryDecomposeKind::GodAnimal, false)
        }
        "refinery.refineryBrochures" | "refinery.refineryBrochure" => {
            (RefineryDecomposeKind::Brochures, false)
        }
        "refinery.refineryYanWu" => (RefineryDecomposeKind::YanWu, false),
        "soulSkill.refinery" | "refinery.refinerySoulSkill" => {
            (RefineryDecomposeKind::SoulSkill, true)
        }
        _ => return None,
    };
    Some((kind, preview, ret_triplets))
}

fn reborn_kind_for_method(m: &str) -> Option<(RefineryRebornKind, bool, bool)> {
    let (base, preview) = if let Some(s) = m.strip_suffix("Preview") {
        (s, true)
    } else if let Some(s) = m.strip_suffix("CostShow") {
        (s, true)
    } else {
        (m, false)
    };
    let (kind, ret_triplets) = match base {
        "refinery.rebornHero" => (RefineryRebornKind::Hero, false),
        "refinery.rebornOrangeHero" => (RefineryRebornKind::OrangeHero, false),
        "refinery.rebornArm" => (RefineryRebornKind::Arm, false),
        "refinery.rebornTreasure" => (RefineryRebornKind::Treasure, false),
        "refinery.rebornAwake" | "refinery.rebornAwakeItem" => (RefineryRebornKind::Awake, false),
        "refinery.rebornSpirit" => (RefineryRebornKind::Spirit, false),
        "refinery.rebornPet" => (RefineryRebornKind::Pet, false),
        "refinery.rebornGodArm" => (RefineryRebornKind::GodArm, false),
        "refinery.rebornGodAnimal" => (RefineryRebornKind::GodAnimal, false),
        "refinery.rebornTopAnimal" => (RefineryRebornKind::TopAnimal, false),
        "refinery.rebornBrochure" => (RefineryRebornKind::Brochure, false),
        "refinery.rebornCounsellorEquip" => (RefineryRebornKind::CounsellorEquip, false),
        "refinery.rebornSoulStone" => (RefineryRebornKind::SoulStone, false),
        "refinery.rebornSoulJade" => (RefineryRebornKind::SoulJade, false),
        "refinery.rebornRune" | "refinery.rebornGem" => (RefineryRebornKind::Rune, false),
        "heroPill.reborn" | "refinery.rebornHeroPill" => (RefineryRebornKind::HeroPill, true),
        "dress.rebornDress" | "heroSkin.reborn" | "refinery.rebornDress" => {
            (RefineryRebornKind::Dress, true)
        }
        "soulSkill.reborn" | "refinery.rebornSoulSkill" => (RefineryRebornKind::SoulSkill, true),
        _ => return None,
    };
    Some((kind, preview, ret_triplets))
}

pub fn try_handle_player_rpc(
    store: &mut PlayerStore,
    gameplay: &mut GameplayEngine,
    data_store: &GameDataStore,
    uid: u64,
    req: &RpcRequest,
    now_ts: u64,
) -> Option<Result<PlayerRpcSuccess, String>> {
    let now_i64 = now_ts as i64;
    let (norm_method, is_tm) = normalize_player_method(&req.method);
    let m = norm_method.as_str();

    // Check Furnace Decompose & Reborn tables first!
    if let Some((decomp_kind, preview_only, ret_triplets)) = decompose_kind_for_method(m) {
        let target_ids = parse_target_id_list(req);
        if target_ids.is_empty() {
            return Some(Err("empty_refinery_targets".to_string()));
        }
        let mut out_opt = None;
        let res = if preview_only {
            let Some(mut cloned) = store.get_player(uid).cloned() else {
                return Some(Err("player_missing".to_string()));
            };
            cloned
                .refinery_decompose(decomp_kind, &target_ids, true, now_ts)
                .map(|o| {
                    out_opt = Some(o);
                })
        } else {
            store.mutate_player(uid, now_ts, |p| {
                let o = p.refinery_decompose(decomp_kind, &target_ids, false, now_ts)?;
                out_opt = Some(o);
                Ok(())
            })
        };
        if let Err(e) = res {
            return Some(Err(map_player_err(e)));
        }
        let out = out_opt.expect("set above");
        let mut pushes = Vec::new();
        if !preview_only {
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
        }
        let ret = if ret_triplets {
            player_json_to_amf3(&out.reward_triplets)
        } else {
            player_json_to_amf3(&out.reward_dict)
        };
        return Some(Ok(PlayerRpcSuccess {
            ret,
            caller_pushes: pushes,
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        }));
    }

    if let Some((reborn_kind, preview_only, ret_triplets)) = reborn_kind_for_method(m) {
        let target_ids = parse_target_id_list(req);
        let Some(&target_id) = target_ids.first() else {
            return Some(Err("invalid_reborn_target".to_string()));
        };
        let keep_destiny = req
            .args
            .get(1)
            .and_then(|v| match v { Value::Bool(b) => Some(*b), _ => v.as_i64().map(|n| n != 0) });
        let mut out_opt = None;
        let res = if preview_only {
            let Some(mut cloned) = store.get_player(uid).cloned() else {
                return Some(Err("player_missing".to_string()));
            };
            cloned
                .refinery_reborn(reborn_kind, target_id, keep_destiny, true, now_ts)
                .map(|o| {
                    out_opt = Some(o);
                })
        } else {
            store.mutate_player(uid, now_ts, |p| {
                let mut first_out =
                    p.refinery_reborn(reborn_kind, target_id, keep_destiny, false, now_ts)?;
                for &extra_id in target_ids.iter().skip(1) {
                    let next_out =
                        p.refinery_reborn(reborn_kind, extra_id, keep_destiny, false, now_ts)?;
                    first_out.bag_deltas.merge(next_out.bag_deltas);
                }
                out_opt = Some(first_out);
                Ok(())
            })
        };
        if let Err(e) = res {
            return Some(Err(map_player_err(e)));
        }
        let out = out_opt.expect("set above");
        let mut pushes = Vec::new();
        if !preview_only {
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
        }
        let ret = if ret_triplets {
            player_json_to_amf3(&out.reward_triplets)
        } else {
            player_json_to_amf3(&out.reward_dict)
        };
        return Some(Ok(PlayerRpcSuccess {
            ret,
            caller_pushes: pushes,
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        }));
    }

    match m {
        // ====================================================================
        // Economy, Rename, BuyCopper, Stamina & VIP Gifts (`[tm]User.*`)
        // ====================================================================
        "user.changeUname" => {
            let raw_name = req
                .args
                .first()
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if raw_name.is_empty() || raw_name.chars().count() > 15 {
                return Some(Err("invalid_uname".to_string()));
            }
            let Some(cur_profile) = store.get_player(uid).cloned() else {
                return Some(Err("player_missing".to_string()));
            };
            if cur_profile.uname == raw_name {
                return Some(Err("uname_unchanged".to_string()));
            }
            if gameplay
                .players
                .values()
                .any(|p| p.uid != uid && p.uname == raw_name)
            {
                return Some(Err("uname_used".to_string()));
            }

            let res = store.mutate_player(uid, now_ts, |p| {
                if p.uname_change_num > 0 {
                    p.consume_currency(CurrencyType::Gold, 100, now_ts)?;
                }
                p.uname = raw_name.clone();
                p.uname_change_num = p.uname_change_num.saturating_add(1);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let mut peer_pushes = Vec::new();
            if let Some(gp) = gameplay.players.get(&uid) {
                for (target_uid, pkt) in gameplay.town.sync_player_appearance(gp, now_ts) {
                    peer_pushes.push((
                        target_uid,
                        S2cEnvelope::push(pkt.callback_name, dto_to_amf3(&pkt.ret, true))
                            .with_time(now_i64),
                    ));
                }
            }
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let push = build_user_update_env(updated, is_tm, now_i64);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![push],
                peer_pushes,
                refresh_roles_pid: Some(pid),
            }))
        }

        "user.buyCopper" => {
            let count = arg_i64(req, 0).unwrap_or(1).clamp(1, 50) as u64;
            let unit_copper = 100_000u64;
            let gold_cost = 20 * count;
            let copper_gain = unit_copper * count;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
                p.grant_currency(CurrencyType::Copper, copper_gain, now_ts);
                p.buy_copper_accum = p.buy_copper_accum.saturating_add(count as u32);
                p.buy_copper_time = now_ts;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let push = build_user_update_env(updated, is_tm, now_i64);
            // `BuyCoinPanel.lua:187`: `for k, v in ipairs(data) do userdata:addCopper(v[2]) end`
            // MUST return a 1-based dense array of `[critMult, copperGain]` pairs!
            let pairs: Vec<Value> = (0..count)
                .map(|_| Value::dense_array(vec![Value::Int(1), u64_to_amf3(unit_copper)]))
                .collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::dense_array(pairs),
                caller_pushes: vec![push],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.buyExecution" => {
            let count = arg_i64(req, 0).unwrap_or(1).clamp(1, 20) as u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, 20 * count, now_ts)?;
                p.grant_currency(CurrencyType::Execution, 25 * count, now_ts);
                p.buy_execution_accum = p.buy_execution_accum.saturating_add(count as u32);
                p.buy_execution_time = now_ts;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.addEnergy" | "user.buyStamina" => {
            let count = arg_i64(req, 0).unwrap_or(1).clamp(1, 20) as u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, 20 * count, now_ts)?;
                p.grant_currency(CurrencyType::Energy, 10 * count, now_ts);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.openHeroBag" | "user.openHeroGrid" => {
            let times = arg_i64(req, 0).unwrap_or(1).clamp(1, 10) as u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.open_hero_bag_by_gold(times, now_ts)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Bool(true),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.getVipGiftInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            // `VipData.lua:83`: `for k, v in pairs(data) do self._vip_gift_data[tonumber(v)] = true end`
            let mut bought = read_u32_set_from_va(profile, "vip_gift_bought");
            bought.sort_unstable();
            bought.dedup();
            let list: Vec<Value> = bought.into_iter().map(|v| Value::Int(v as i32)).collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::dense_array(list),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.buyVipGift" => {
            let vip_lv = arg_i64(req, 0).unwrap_or(-1);
            if !(0..=16).contains(&vip_lv) {
                return Some(Err("invalid_vip_level".to_string()));
            }
            let vip_u32 = vip_lv as u32;
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            if cur.vip < vip_u32 {
                return Some(Err("vip_too_low".to_string()));
            }
            let mut bought = read_u32_set_from_va(cur, "vip_gift_bought");
            if bought.contains(&vip_u32) {
                return Some(Err("vip_gift_already_bought".to_string()));
            }

            let cost_gold = vip_gift_gold_cost(vip_u32);
            let gift_triplets = vip_gift_reward_triplets(vip_u32);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, cost_gold, now_ts)?;
                let (d_gift, _) = crate::event_pay_rpcs::grant_triplets(p, gift_triplets, now_ts)?;
                delta.merge(d_gift);
                bought.push(vip_u32);
                bought.sort_unstable();
                bought.dedup();
                write_u32_set_to_va(p, "vip_gift_bought", &bought);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Bag Expansion, Selling, Item Use, OpenBox & Fragment Synthesis
        // ====================================================================
        "bag.openGridByGold" => {
            let (bag_type, steps, _) = parse_bag_expand_args(req);
            let res = store.mutate_player(uid, now_ts, |p| {
                p.open_bag_grid_by_gold(bag_type, steps, now_ts)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "bag.openGridByItem" => {
            let (bag_type, steps, item_tid) = parse_bag_expand_args(req);
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.open_bag_grid_by_item(bag_type, item_tid, steps, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "bag.sellItems" | "bag.sellItem" => {
            let entries = match parse_sell_bag_entries(req) {
                Ok(list) => list,
                Err(code) => return Some(Err(code)),
            };
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.sell_bag_items(&entries, Some(data_store), now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "refinery.refineryFragment" | "refinery.refinerySoul" | "refinery.refineryHeroLegend" => {
            let sell_kind = match m {
                "refinery.refinerySoul" => RefinerySellKind::Soul,
                "refinery.refineryHeroLegend" => RefinerySellKind::HeroLegend,
                _ => RefinerySellKind::Fragment,
            };
            let ids = parse_target_id_list(req);
            if ids.is_empty() {
                return Some(Err("empty_sell_list".to_string()));
            }
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out =
                    p.refinery_sell_fragments_or_props(sell_kind, &ids, Some(data_store), now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "bag.useItem" | "exchange.useItemInBag" | "shop.use" => {
            let target_raw = arg_i64(req, 0).unwrap_or(0);
            if target_raw <= 0 {
                return Some(Err("invalid_gid".to_string()));
            }
            let target_id = target_raw as u64;
            let Some(cur_prof) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let existing_item = u32::try_from(target_id)
                .ok()
                .and_then(|g| cur_prof.inventory.get_by_gid(g))
                .or_else(|| {
                    cur_prof
                        .inventory
                        .get_by_item_id(target_id)
                        .map(|(_, it)| it)
                });
            let Some(existing) = existing_item.cloned() else {
                return Some(Err("item_not_found".to_string()));
            };

            // Support BOTH `[item_id_or_gid, count, choose_idx]` (`BagProxy.lua`) AND legacy `[gid, item_id, count]`
            let a1_val = if req.arg_zero().is_some() {
                req.arg_at_lua0(1)
            } else {
                req.args.get(1)
            };
            let a2_val = if req.arg_zero().is_some() {
                req.arg_at_lua0(2)
            } else {
                req.args.get(2)
            };
            let a3_val = if req.arg_zero().is_some() {
                req.arg_at_lua0(3)
            } else {
                req.args.get(3)
            };

            let (count_i64, option_idx) = if let Some(a1_num) = a1_val.and_then(val_as_i64_loose) {
                if a1_num as u64 == existing.item_id
                    || (a1_num >= 1_000_000 && a2_val.and_then(val_as_i64_loose).is_some())
                {
                    if a1_num as u64 != existing.item_id {
                        return Some(Err("item_id_mismatch".to_string()));
                    }
                    let c = a2_val.and_then(val_as_i64_loose).unwrap_or(1);
                    (c, extract_option_idx(a3_val))
                } else {
                    (a1_num, extract_option_idx(a2_val))
                }
            } else {
                (1, extract_option_idx(a2_val))
            };

            if count_i64 <= 0 {
                return Some(Err("invalid_item_count".to_string()));
            }
            let count = count_i64 as u32;

            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.use_item_in_bag(
                    u64::from(existing.gid),
                    count,
                    option_idx,
                    Some(data_store),
                    now_ts,
                )?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &out.summoned_hids, is_tm, now_i64) {
                pushes.push(hp);
            }
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            let pid = updated.pid.clone();
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&out.drop),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        "exchange.useItemInBagByTpl" => {
            let tid_i64 = arg_i64(req, 0).unwrap_or(0);
            let count_i64 = arg_i64(req, 1).unwrap_or(1);
            if tid_i64 <= 0 || count_i64 <= 0 {
                return Some(Err("invalid_item_args".to_string()));
            }
            let option_idx = extract_option_idx(req.args.get(2));
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.use_item_by_tpl(
                    tid_i64 as u32,
                    count_i64 as u32,
                    option_idx,
                    Some(data_store),
                    now_ts,
                )?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &out.summoned_hids, is_tm, now_i64) {
                pushes.push(hp);
            }
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&out.drop),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exchange.getOpenBoxInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&profile.get_open_box_info()),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exchange.openRandgiftBoxByKey" | "exchange.openRandgiftBoxByGold" => {
            let chest_tid = arg_i64(req, 0).unwrap_or(33_001).max(1) as u32;
            let num = arg_i64(req, 1).unwrap_or(1).clamp(1, 50) as u32;
            let by_gold = m == "exchange.openRandgiftBoxByGold";
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = if by_gold {
                    p.open_randgift_box_by_gold(chest_tid, num, now_ts)?
                } else {
                    p.open_randgift_box_by_key(chest_tid, num, now_ts)?
                };
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            let drops_arr: Vec<Value> = out.drops.iter().map(player_json_to_amf3).collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::dense_array(drops_arr),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "bag.takeTmpBag" => Some(Ok(PlayerRpcSuccess {
            ret: Value::String("ok".into()),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        // ====================================================================
        // Synthesis (`[tm]Exchange.compose*`, `emExchange.composeTreasure`, `quickCompose*`)
        // ====================================================================
        "exchange.composeHero" => {
            let raw_id = arg_i64(req, 0).unwrap_or(0);
            let count = arg_i64(req, 1).unwrap_or(1).clamp(1, 20) as u32;
            if raw_id <= 0 {
                return Some(Err("invalid_fragment_id".to_string()));
            }
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let frag_tid = if let Some(cand_tid) = u32::try_from(raw_id)
                .ok()
                .filter(|&tid| (50_001..=60_000).contains(&tid) && cur.inventory.count_by_template(tid) > 0)
            {
                cand_tid
            } else {
                u32::try_from(raw_id)
                    .ok()
                    .and_then(|g| {
                        cur.inventory
                            .get_by_gid(g)
                            .map(|it| it.item_template_id)
                            .filter(|&tid| (50_001..=60_000).contains(&tid))
                    })
                    .or_else(|| {
                        cur.inventory
                            .get_by_item_id(raw_id as u64)
                            .map(|(_, it)| it.item_template_id)
                    })
                    .unwrap_or(raw_id as u32)
            };
            if !(50_001..=60_000).contains(&frag_tid) {
                return Some(Err("not_hero_fragment".to_string()));
            }

            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out =
                    p.compose_hero_from_frag(frag_tid, count, Some(data_store), now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &out.summoned_hids, is_tm, now_i64) {
                pushes.push(hp);
            }
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            let first_hid = out.summoned_hids.first().copied().unwrap_or(0);
            let first_htid = out.heroes.first().map(|h| h.htid).unwrap_or(0);
            let heroes_map: Vec<(String, Value)> = out
                .heroes
                .iter()
                .map(|h| (h.hid.to_string(), player_json_to_amf3(&h.to_vnsg2_hero_info())))
                .collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(first_hid)),
                    ("htid", Value::Int(first_htid as i32)),
                    (
                        "hids",
                        Value::dense_array(
                            out.summoned_hids.into_iter().map(u64_to_amf3).collect(),
                        ),
                    ),
                    ("heroes", Value::map(heroes_map)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exchange.composeArm"
        | "exchange.composeTreasureByFragment"
        | "exchange.composeAwake"
        | "exchange.composeGodArm"
        | "exchange.composePet"
        | "exchange.composeNormal"
        | "exchange.composeGem"
        | "exchange.composeGodAnimal"
        | "exchange.composeTopAnimal"
        | "exchange.composeBrochure"
        | "exchange.composeYanWu"
        | "exchange.composeCounsellor"
        | "exchange.composeCounsellorEquip"
        | "exchange.composeSoulStone"
        | "exchange.composeSoulJade"
        | "forge.composeSoulSkill" => {
            let raw_id = arg_i64(req, 0).unwrap_or(0);
            let count = arg_i64(req, 1).unwrap_or(1).clamp(1, 50) as u32;
            if raw_id <= 0 {
                return Some(Err("invalid_fragment_id".to_string()));
            }
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let frag_tid = if let Some(cand_tid) = u32::try_from(raw_id)
                .ok()
                .filter(|&tid| {
                    PlayerProfile::resolve_item_frag_recipe(tid, Some(data_store)).is_ok()
                        && cur.inventory.count_by_template(tid) > 0
                })
            {
                cand_tid
            } else {
                u32::try_from(raw_id)
                    .ok()
                    .and_then(|g| {
                        cur.inventory
                            .get_by_gid(g)
                            .map(|it| it.item_template_id)
                            .filter(|&tid| {
                                PlayerProfile::resolve_item_frag_recipe(tid, Some(data_store))
                                    .is_ok()
                            })
                    })
                    .or_else(|| {
                        cur.inventory
                            .get_by_item_id(raw_id as u64)
                            .map(|(_, it)| it.item_template_id)
                    })
                    .unwrap_or(raw_id as u32)
            };

            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out =
                    p.compose_item_from_frag(frag_tid, count, Some(data_store), now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exchange.composeTreasure" => {
            let raw_id = arg_i64(req, 0).unwrap_or(0);
            let count = arg_i64(req, 1).unwrap_or(1).clamp(1, 50) as u32;
            if raw_id <= 0 {
                return Some(Err("invalid_treasure_id".to_string()));
            }
            let tid = raw_id as u32;
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = if (400_000_001..=500_000_000).contains(&tid) {
                    p.compose_item_from_frag(tid, count, Some(data_store), now_ts)?
                } else {
                    p.compose_treasure(tid, count, Some(data_store), now_ts)?
                };
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            let first_tid = out.created_items.first().map(|i| i.item_template_id).unwrap_or(0);
            let first_gid = out.created_items.first().map(|i| i.gid).unwrap_or(0);
            let first_item_id = out.created_items.first().map(|i| i.item_id).unwrap_or(0);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("item_template_id", Value::Int(first_tid as i32)),
                    ("gid", Value::Int(first_gid as i32)),
                    ("item_id", u64_to_amf3(first_item_id)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exchange.oneKeycomposeTreasure" | "exchange.oneKeyComposeTreasure" => {
            let tid = arg_i64(req, 0).unwrap_or(0);
            if tid <= 0 {
                return Some(Err("invalid_treasure_id".to_string()));
            }
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out =
                    p.one_key_compose_treasure(tid as u32, Some(data_store), now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            let count = out.created_items.len() as i32;
            let first_gid = out.created_items.first().map(|i| i.gid).unwrap_or(0);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("num", Value::Int(count)),
                    ("gid", Value::Int(first_gid as i32)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exchange.quickComposeArm"
        | "exchange.quickComposeTreasure"
        | "exchange.quickComposeAwake"
        | "exchange.quickComposeGodArm"
        | "exchange.quickComposeGem"
        | "exchange.quickCompose" => {
            let filter = match m {
                "exchange.quickComposeArm" => Some(BagType::Fragment),
                "exchange.quickComposeTreasure" => Some(BagType::TreasureFragment),
                "exchange.quickComposeAwake" => Some(BagType::AwakeFragment),
                "exchange.quickComposeGodArm" => Some(BagType::GodArmFragment),
                "exchange.quickComposeGem" => Some(BagType::Gem),
                _ => None,
            };
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.quick_compose_from_frag(filter, Some(data_store), now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            let crafted_arr: Vec<Value> = out
                .created_items
                .iter()
                .map(|i| Value::Int(i.item_template_id as i32))
                .collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("items", Value::dense_array(crafted_arr)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Hero Roster, LevelUp, Skill, Evolve, Awake, Star, Wash, Pill & Dress
        // ====================================================================
        "hero.getAllHeroes" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let entries: Vec<(String, Value)> = profile
                .heroes
                .iter()
                .map(|(hid, hero)| {
                    (
                        hid.to_string(),
                        player_json_to_amf3(&hero.to_vnsg2_hero_info()),
                    )
                })
                .collect();
            let ret = if entries.is_empty() {
                Value::empty_array()
            } else {
                Value::map(entries)
            };
            Some(Ok(PlayerRpcSuccess {
                ret,
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "hero.heroLevelUp" | "hero.levelUp" => {
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let levels = arg_i64(req, 1).unwrap_or(1).clamp(1, 50) as u32;
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.hero_level_up_vnsg2(hid as u64, levels, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            let item_cost_val = if out.item_cost.is_empty() {
                Value::empty_array()
            } else {
                Value::map(
                    out.item_cost
                        .iter()
                        .map(|(tid, cnt)| (tid.to_string(), Value::Int(*cnt as i32))),
                )
            };
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(out.hid)),
                    ("oldLv", Value::Int(out.old_lv as i32)),
                    ("newlv", Value::Int(out.newlv as i32)),
                    ("level", Value::Int(out.newlv as i32)),
                    ("exp", u64_to_amf3(out.exp)),
                    ("itemCost", item_cost_val),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "hero.skillLevelUp" => {
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let explicit_skill_id = arg_i64(req, 1)
                .filter(|&v| v > 0)
                .and_then(|v| u32::try_from(v).ok());
            let mut new_star_lv = 1u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                if !p.heroes.contains_key(&(hid as u64)) {
                    return Err(PlayerError::HeroNotFound(hid as u64));
                }
                p.consume_currency(CurrencyType::Copper, 2_000, now_ts)?;
                let hero = p.heroes.get_mut(&(hid as u64)).expect("checked");
                let skill_id = explicit_skill_id.unwrap_or_else(|| {
                    data_store
                        .resolve_hero_battle_skills(hero.htid, hero.awake_rank)
                        .normal_skill_id
                });
                if !hero.talent.contains(&skill_id) {
                    hero.talent.push(skill_id);
                }
                hero.hero_star_level = hero.hero_star_level.saturating_add(1);
                new_star_lv = hero.hero_star_level;
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(hid as u64)),
                    ("skill_lv", Value::Int(new_star_lv as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "evolve.doEvolve" | "evolve.oneKeyEvolve" => {
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let one_key = m == "evolve.oneKeyEvolve";
            let mut explicit_sac_hids: Vec<u64> = Vec::new();
            if let Some(second) = req.args.get(1) {
                collect_u64_ids_from_value(second, &mut explicit_sac_hids);
            }

            let mut new_ev = 0u32;
            let mut del_hids = Vec::new();
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                if !explicit_sac_hids.is_empty() {
                    new_ev = p.evolve_hero(hid as u64, &explicit_sac_hids, 5_000, now_ts)?;
                    del_hids = explicit_sac_hids.clone();
                } else {
                    let out = p.evolve_hero_vnsg2(hid as u64, one_key, now_ts)?;
                    new_ev = out.new_evolve_lv;
                    del_hids = out.del_hero_hids;
                    delta = out.bag_deltas;
                }
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            let del_arr: Vec<Value> = del_hids.into_iter().map(u64_to_amf3).collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(hid as u64)),
                    ("newEvolvelv", Value::Int(new_ev as i32)),
                    ("evolve_level", Value::Int(new_ev as i32)),
                    ("delHeroHids", Value::dense_array(del_arr)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "evolve.surmount" | "hero.upgrade" => {
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let mut new_up = 0u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                new_up = p.upgrade_hero(hid as u64, 5_000, now_ts)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(hid as u64)),
                    ("upgrade_level", Value::Int(new_up as i32)),
                    ("delHeroHids", Value::empty_array()),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "hero.awake" => {
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let mut out_pair = (0u32, 0u32);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (rk, lv, d) = p.awake_hero(hid as u64, now_ts)?;
                out_pair = (rk, lv);
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(hid as u64)),
                    ("awake_rank", Value::Int(out_pair.0 as i32)),
                    ("awake_level", Value::Int(out_pair.1 as i32)),
                    ("delHeroHids", Value::empty_array()),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "hero.lock" | "hero.unLock" | "hero.unlock" => {
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let lock = m == "hero.lock";
            let res = store.mutate_player(uid, now_ts, |p| p.set_hero_lock(hid as u64, lock));
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "herostar.levelUp"
        | "herostar.oneKeylevelUp"
        | "herostar.oneKeyLevelUp"
        | "herostar.levelUpToMax"
        | "hero.upgradeStar" => {
            let hid = arg_i64(req, 0).unwrap_or_else(|| {
                store
                    .get_player(uid)
                    .map_or(0, |p| p.protagonist_hid as i64)
            });
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let is_auto = m != "herostar.levelUp";
            let mut star_lv = 1u32;
            let mut star_pos = 0u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (lv, pos, d) = p.upgrade_hero_star(hid as u64, is_auto, now_ts)?;
                star_lv = lv;
                star_pos = pos;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            let ret = if !is_auto {
                Value::Bool(true)
            } else {
                Value::map([
                    ("curPos", Value::Int(star_pos as i32)),
                    ("curLevel", Value::Int(star_lv as i32)),
                    ("failPos", Value::empty_array()),
                ])
            };
            Some(Ok(PlayerRpcSuccess {
                ret,
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "wash.getInfo" => Some(Ok(PlayerRpcSuccess {
            ret: Value::empty_array(),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "wash.doWash"
        | "wash.wash"
        | "wash.oneKeyWash"
        | "wash.save"
        | "wash.replaceWash"
        | "wash.cancel"
        | "wash.transferWash"
        | "wash.getBox" => {
            if m == "wash.cancel" || m == "wash.getBox" {
                return Some(Ok(PlayerRpcSuccess {
                    ret: Value::String("ok".into()),
                    caller_pushes: Vec::new(),
                    peer_pushes: Vec::new(),
                    refresh_roles_pid: None,
                }));
            }
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let apply_replace = matches!(m, "wash.save" | "wash.replaceWash");
            let times = if m == "wash.oneKeyWash" {
                5u32
            } else {
                arg_i64(req, 1).unwrap_or(1).clamp(1, 10) as u32
            };
            let use_gold = arg_i64(req, 2).unwrap_or(0) != 0;
            let mut wash_json = PlayerJson::Null;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (wj, d) = p.wash_hero(hid as u64, times, apply_replace, use_gold, now_ts)?;
                wash_json = wj;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&wash_json),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "heroPill.addPill" | "heroPill.oneKeyAddPill" | "heroPill.levelUp" => {
            let hid = arg_i64(req, 0).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let mut pill_lv = 1u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                let h = p
                    .heroes
                    .get_mut(&(hid as u64))
                    .ok_or(PlayerError::HeroNotFound(hid as u64))?;
                h.hero_pill_level = h.hero_pill_level.saturating_add(1);
                if h.hero_pill_level >= 10 {
                    h.hero_pill_level = 0;
                    h.hero_pill_rank = h.hero_pill_rank.saturating_add(1);
                }
                pill_lv = h.hero_pill_rank * 10 + h.hero_pill_level;
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("pill_lv", Value::Int(pill_lv as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "dress.wear" | "dress.setDress" | "dress.strengthen" | "dress.getInfo" => {
            if m == "dress.getInfo" {
                return Some(Ok(PlayerRpcSuccess {
                    ret: Value::empty_array(),
                    caller_pushes: Vec::new(),
                    peer_pushes: Vec::new(),
                    refresh_roles_pid: None,
                }));
            }
            let dress_id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.dress_id = dress_id;
                p.attr_dress_id = dress_id;
                let lv = p.dress_info.entry(dress_id).or_insert(0);
                *lv = (*lv).saturating_add(1);
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // Quality promotions (`heroGold.*`, `darkGold.*`, `purpleGold.*`, `colorGold.*`, `nirvanaGold.*`, `gildGold.*`, `ironGold.*`, `forge.to*`)
        _ if m.starts_with("heroGold.")
            || m.starts_with("darkGold.")
            || m.starts_with("purpleGold.")
            || m.starts_with("colorGold.")
            || m.starts_with("nirvanaGold.")
            || m.starts_with("gildGold.")
            || m.starts_with("ironGold.")
            || m.starts_with("forge.to") =>
        {
            let target = arg_i64(req, 0).unwrap_or(0);
            if target <= 0 {
                return Some(Err("invalid_promotion_target".to_string()));
            }
            let tid_u64 = target as u64;
            let mut bag_upserted = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Copper, 10_000, now_ts)?;
                if let Some(h) = p.heroes.get_mut(&tid_u64) {
                    if m.starts_with("darkGold.") {
                        h.dark_gold_rank = h.dark_gold_rank.max(1);
                        h.dark_gold_level = h.dark_gold_level.saturating_add(1);
                    } else if m.starts_with("purpleGold.") {
                        h.purple_gold_rank = h.purple_gold_rank.max(1);
                        h.purple_gold_level = h.purple_gold_level.saturating_add(1);
                    } else if m.starts_with("colorGold.") {
                        h.color_gold_rank = h.color_gold_rank.max(1);
                        h.color_gold_level = h.color_gold_level.saturating_add(1);
                    } else if m.starts_with("nirvanaGold.") {
                        h.nirvana_gold_rank = h.nirvana_gold_rank.max(1);
                        h.nirvana_gold_level = h.nirvana_gold_level.saturating_add(1);
                    } else if m.starts_with("gildGold.") {
                        h.gild_gold_rank = h.gild_gold_rank.max(1);
                        h.gild_gold_level = h.gild_gold_level.saturating_add(1);
                    } else if m.starts_with("ironGold.") {
                        h.iron_gold_rank = h.iron_gold_rank.max(1);
                        h.iron_gold_level = h.iron_gold_level.saturating_add(1);
                    } else {
                        h.gold_rank = h.gold_rank.max(1);
                        h.gold_level = h.gold_level.saturating_add(1);
                    }
                    p.recalculate_fight_force();
                    return Ok(());
                }
                // Equipment quality promotion
                if let Some(gid) = resolve_bag_gid(p, tid_u64) {
                    if let Some(bt) = BagType::from_gid(gid) {
                        if let Some(it) = p.inventory.bags.get_mut(&bt).and_then(|s| s.get_mut(&gid))
                        {
                            it.va_item_text
                                .insert("gold_rank".into(), PlayerJson::from(1u32));
                            bag_upserted.push(it.clone());
                        }
                    }
                } else {
                    for sm in p.formation.arming.values_mut() {
                        for it in sm.values_mut() {
                            if it.item_id == tid_u64 {
                                it.va_item_text
                                    .insert("gold_rank".into(), PlayerJson::from(1u32));
                            }
                        }
                    }
                }
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_update_env(&bag_upserted, &[], is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Squad, Battle Grid, Partner, Buddy, Arming, Spirit, Animals & MorePower
        // ====================================================================
        "formation.getHeroes" | "formation.getSquad" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let slots: Vec<Value> = (1..=6u8)
                .map(|slot| {
                    let hid = profile.formation.position.get(&slot).copied().unwrap_or(0);
                    u64_to_amf3(hid)
                })
                .collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::dense_array(slots),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.getOtherPositionInfo" | "formation.getOtherServerPositionInfo" => {
            let target_uid = arg_i64(req, 0).unwrap_or(0);
            if target_uid <= 0 {
                return Some(Err("invalid_target_uid".to_string()));
            }
            let Some(other) = store.get_player(target_uid as u64) else {
                return Some(Err("target_player_not_found".to_string()));
            };
            let mut pos_json = other.formation.to_vnsg2_position_info(&other.heroes);
            if let PlayerJson::Object(ref mut map) = pos_json {
                map.insert("uid".into(), PlayerJson::from(other.uid));
                map.insert("uname".into(), PlayerJson::from(other.uname.clone()));
                map.insert("level".into(), PlayerJson::from(other.level));
                map.insert("vip".into(), PlayerJson::from(other.vip));
                map.insert("fight_force".into(), PlayerJson::from(other.fc));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&pos_json),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.setHeroToPosition" => {
            let (hid, slot) = match parse_squad_hid_and_slot(&req.args, false) {
                Ok(pair) => pair,
                Err(code) => return Some(Err(code)),
            };
            let res = store.mutate_player(uid, now_ts, |p| {
                p.set_hero_to_position(slot, hid)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Bool(true),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.unsetHeroFromPosition" => {
            let slot_i64 = arg_i64(req, 0).unwrap_or(0);
            if !(1..=6).contains(&slot_i64) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.unset_hero_from_position(slot_i64 as u8)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Bool(true),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.transHero" => {
            let sa = arg_i64(req, 0).unwrap_or(0);
            let sb = arg_i64(req, 1).unwrap_or(0);
            if !(1..=6).contains(&sa) || !(1..=6).contains(&sb) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| p.trans_hero(sa as u8, sb as u8));
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Bool(true),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.set" => {
            let Some(first) = req.args.first() else {
                return Some(Err("missing_grid_formation".to_string()));
            };
            let mut grid_map: BTreeMap<u8, u64> = BTreeMap::new();
            match first {
                Value::Array { dense, assoc } => {
                    for (idx, v) in dense.iter().enumerate() {
                        if let Some(hid) = val_as_i64_loose(v) {
                            if hid > 0 {
                                grid_map.insert((idx + 1) as u8, hid as u64);
                            }
                        }
                    }
                    for (k, v) in assoc {
                        let cell: u8 = k.parse().unwrap_or(0);
                        if let Some(hid) = val_as_i64_loose(v) {
                            if hid > 0 {
                                grid_map.insert(cell, hid as u64);
                            }
                        }
                    }
                }
                Value::Object(obj) => {
                    for (k, v) in obj.iter() {
                        let cell: u8 = k.parse().unwrap_or(0);
                        if let Some(hid) = val_as_i64_loose(v) {
                            if hid > 0 {
                                grid_map.insert(cell, hid as u64);
                            }
                        }
                    }
                }
                _ => return Some(Err("invalid_grid_formation".to_string())),
            }

            let res = store.mutate_player(uid, now_ts, |p| p.set_grid_formation(grid_map.clone()));
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.openPartner" => {
            let slot = arg_i64(req, 0).unwrap_or(0);
            if !(1..=12).contains(&slot) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| p.open_partner(slot as u8, 0, now_ts));
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.setPartner" => {
            let (hid, slot) = match parse_partner_hid_and_slot(&req.args, true) {
                Ok(pair) => pair,
                Err(code) => return Some(Err(code)),
            };
            let res = store.mutate_player(uid, now_ts, |p| {
                p.set_partner(slot, hid)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Bool(true),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.unsetMultiPartner" => {
            let mut slots: Vec<u8> = Vec::new();
            if let Some(Value::Array { dense, .. }) = req.args.first() {
                for v in dense {
                    if let Some(s) = val_as_i64_loose(v) {
                        if !(1..=12).contains(&s) {
                            return Some(Err("invalid_formation_slot".to_string()));
                        }
                        slots.push(s as u8);
                    }
                }
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                let opt = if slots.is_empty() {
                    None
                } else {
                    Some(slots.as_slice())
                };
                p.unset_multi_partner(opt)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Bool(true),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.openBuddy" => {
            let slot = arg_i64(req, 0).unwrap_or(0);
            if !(1..=8).contains(&slot) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| p.open_buddy(slot as u8, 0, now_ts));
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.setBuddy" => {
            let (hid, slot) = match parse_buddy_hid_and_slot(&req.args, true) {
                Ok(pair) => pair,
                Err(code) => return Some(Err(code)),
            };
            let res = store.mutate_player(uid, now_ts, |p| {
                p.set_buddy(slot, hid)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Bool(true),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.buddyLvUp" => {
            let slot = arg_i64(req, 0).unwrap_or(1);
            if !(1..=8).contains(&slot) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let mut new_lv = 1u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                new_lv = p.buddy_lv_up(slot as u8, 10_000, now_ts)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("level", Value::Int(new_lv as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.rebornBuddy" => {
            let slot = arg_i64(req, 0).unwrap_or(1);
            if !(1..=8).contains(&slot) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let mut refund = 0u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, r) = p.reborn_buddy(slot as u8, 20, now_ts)?;
                refund = r;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("copper", u64_to_amf3(refund)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.activateMorePower" | "formation.morePowerAutoLvUp" => {
            let mut book_lv = 1u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Copper, 5_000, now_ts)?;
                p.formation.more_power_open_count =
                    p.formation.more_power_open_count.saturating_add(1).min(12);
                book_lv = p.formation.more_power_open_count;
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("morePowerOpenCount", Value::Int(book_lv as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.addArming" | "hero.addArming" => {
            let squad_slot = arg_i64(req, 0).unwrap_or(0);
            let equip_slot_raw = arg_i64(req, 1).unwrap_or(0);
            let item_or_gid = arg_i64(req, 2).unwrap_or(0);
            if !(1..=6).contains(&squad_slot) || !(1..=9).contains(&equip_slot_raw) || item_or_gid <= 0
            {
                return Some(Err("invalid_equip_args".to_string()));
            }
            let equip_slot = match EquipSlotType::from_u8(equip_slot_raw as u8) {
                Ok(es) => es,
                Err(e) => return Some(Err(map_player_err(e))),
            };

            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.add_arming(squad_slot as u8, equip_slot, item_or_gid as u64)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.addMultiArming" | "hero.addMultiArming" => {
            let squad_slot = arg_i64(req, 0).unwrap_or(0);
            if !(1..=6).contains(&squad_slot) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let mut pairs: Vec<(EquipSlotType, u64)> = Vec::new();
            if let Some(Value::Array { dense, assoc }) = req.args.get(1) {
                for (idx, v) in dense.iter().enumerate() {
                    if let Some(id) = val_as_i64_loose(v) {
                        if id > 0 {
                            let es = match EquipSlotType::from_u8((idx + 1) as u8) {
                                Ok(e) => e,
                                Err(err) => return Some(Err(map_player_err(err))),
                            };
                            pairs.push((es, id as u64));
                        }
                    }
                }
                for (k, v) in assoc {
                    let s_u8: u8 = k.parse().unwrap_or(0);
                    let es = match EquipSlotType::from_u8(s_u8) {
                        Ok(e) => e,
                        Err(err) => return Some(Err(map_player_err(err))),
                    };
                    if let Some(id) = val_as_i64_loose(v) {
                        if id > 0 {
                            pairs.push((es, id as u64));
                        }
                    }
                }
            } else if let Some(Value::Object(obj)) = req.args.get(1) {
                for (k, v) in obj.iter() {
                    let s_u8: u8 = k.parse().unwrap_or(0);
                    let es = match EquipSlotType::from_u8(s_u8) {
                        Ok(e) => e,
                        Err(err) => return Some(Err(map_player_err(err))),
                    };
                    if let Some(id) = val_as_i64_loose(v) {
                        if id > 0 {
                            pairs.push((es, id as u64));
                        }
                    }
                }
            }
            if pairs.is_empty() {
                return Some(Err("empty_arming_list".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.add_multi_arming(squad_slot as u8, &pairs)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.removeArming" | "hero.removeArming" => {
            let squad_slot = arg_i64(req, 0).unwrap_or(0);
            let equip_slot_raw = arg_i64(req, 1).unwrap_or(0);
            if !(1..=6).contains(&squad_slot) || !(1..=9).contains(&equip_slot_raw) {
                return Some(Err("invalid_equip_args".to_string()));
            }
            let equip_slot = match EquipSlotType::from_u8(equip_slot_raw as u8) {
                Ok(es) => es,
                Err(e) => return Some(Err(map_player_err(e))),
            };
            let mut bag_item_opt = None;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (it, d) = p.remove_arming(squad_slot as u8, equip_slot)?;
                bag_item_opt = Some(it);
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let bag_item = bag_item_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("gid", Value::Int(bag_item.gid as i32)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.transferArming" | "hero.transferArming" => {
            let from_slot = arg_i64(req, 0).unwrap_or(0);
            let to_slot = arg_i64(req, 1).unwrap_or(0);
            let equip_slot_raw = arg_i64(req, 2).unwrap_or(1);
            if !(1..=6).contains(&from_slot)
                || !(1..=6).contains(&to_slot)
                || !(1..=9).contains(&equip_slot_raw)
            {
                return Some(Err("invalid_equip_args".to_string()));
            }
            let equip_slot = match EquipSlotType::from_u8(equip_slot_raw as u8) {
                Ok(es) => es,
                Err(e) => return Some(Err(map_player_err(e))),
            };
            let res = store.mutate_player(uid, now_ts, |p| {
                p.transfer_arming(from_slot as u8, to_slot as u8, equip_slot)
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "spirit.addSpirit" | "formation.addSpirit" => {
            let sq = arg_i64(req, 0).unwrap_or(0);
            let pos = arg_i64(req, 1).unwrap_or(0);
            let id = arg_i64(req, 2).unwrap_or(0);
            if !(1..=6).contains(&sq) || !(1..=8).contains(&pos) || id <= 0 {
                return Some(Err("invalid_spirit_args".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.add_spirit(sq as u8, pos as u8, id as u64)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "spirit.addMultiSpirit" | "formation.addMultiSpirit" => {
            let sq = arg_i64(req, 0).unwrap_or(0);
            if !(1..=6).contains(&sq) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let mut pairs: Vec<(u8, u64)> = Vec::new();
            if let Some(Value::Array { dense, assoc }) = req.args.get(1) {
                for (idx, v) in dense.iter().enumerate() {
                    if let Some(id) = val_as_i64_loose(v).filter(|&x| x > 0) {
                        pairs.push(((idx + 1) as u8, id as u64));
                    }
                }
                for (k, v) in assoc {
                    if let (Ok(pos), Some(id)) =
                        (k.parse::<u8>(), val_as_i64_loose(v).filter(|&x| x > 0))
                    {
                        pairs.push((pos, id as u64));
                    }
                }
            } else if let Some(Value::Object(obj)) = req.args.get(1) {
                for (k, v) in obj.iter() {
                    if let (Ok(pos), Some(id)) =
                        (k.parse::<u8>(), val_as_i64_loose(v).filter(|&x| x > 0))
                    {
                        pairs.push((pos, id as u64));
                    }
                }
            }
            if pairs.is_empty() {
                return Some(Err("empty_spirit_list".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.add_multi_spirit(sq as u8, &pairs)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "spirit.removeSpirit" | "formation.removeSpirit" => {
            let sq = arg_i64(req, 0).unwrap_or(0);
            let pos = arg_i64(req, 1).unwrap_or(0);
            if !(1..=6).contains(&sq) || !(1..=8).contains(&pos) {
                return Some(Err("invalid_spirit_args".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.remove_spirit(sq as u8, pos as u8)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "spirit.transferSpirit" | "formation.transferSpirit" => {
            let from_sq = arg_i64(req, 0).unwrap_or(0);
            let to_sq = arg_i64(req, 1).unwrap_or(0);
            let from_pos = arg_i64(req, 2).unwrap_or(1);
            let to_pos = arg_i64(req, 3).unwrap_or(from_pos);
            if !(1..=6).contains(&from_sq) || !(1..=6).contains(&to_sq) {
                return Some(Err("invalid_spirit_args".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                p.transfer_spirit(from_sq as u8, to_sq as u8, from_pos as u8, to_pos as u8)
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.addGodAnimal" | "godanimal.addGodAnimal" => {
            let slot = arg_i64(req, 0).unwrap_or(0);
            let id = arg_i64(req, 1).unwrap_or(0);
            if !(1..=6).contains(&slot) || id <= 0 {
                return Some(Err("invalid_god_animal_args".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.add_god_animal(slot as u8, id as u64)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.removeGodAnimal" | "godanimal.removeGodAnimal" => {
            let slot = arg_i64(req, 0).unwrap_or(0);
            if !(1..=6).contains(&slot) {
                return Some(Err("invalid_god_animal_args".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (_, d) = p.remove_god_animal(slot as u8)?;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.addTopAnimal" | "topanimal.addTopAnimal" => {
            let slot = arg_i64(req, 0).unwrap_or(0);
            let id = arg_i64(req, 1).unwrap_or(0);
            if !(1..=6).contains(&slot) || id <= 0 {
                return Some(Err("invalid_top_animal_args".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let gid = resolve_bag_gid(p, id as u64)
                    .ok_or_else(|| PlayerError::ItemNotFound(format!("id={id}")))?;
                let item = p.inventory.get_by_gid(gid).cloned().unwrap();
                let (_, d_rem) = p.remove_item_by_gid_with_delta(gid, 1)?;
                delta.merge(d_rem);
                if let Some(Some(prev)) = p.formation.topanimal.insert(slot as u8, Some(item)) {
                    let returned = p.inventory.insert_instance(prev)?;
                    delta.record_item(returned);
                }
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.removeTopAnimal" | "topanimal.removeTopAnimal" => {
            let slot = arg_i64(req, 0).unwrap_or(0);
            if !(1..=6).contains(&slot) {
                return Some(Err("invalid_top_animal_args".to_string()));
            }
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let Some(Some(prev)) = p.formation.topanimal.insert(slot as u8, None) else {
                    return Err(PlayerError::ItemNotFound("empty topanimal slot".into()));
                };
                let returned = p.inventory.insert_instance(prev)?;
                delta.record_item(returned);
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Forge (`[tm]Forge.*`) & Spirit Explore (`[tm]ExploreSoul.*`)
        // ====================================================================
        "forge.reinforceArm" | "forge.reinforce" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            if raw_target <= 0 {
                return Some(Err("invalid_forge_target".to_string()));
            }
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };
            let is_once = req
                .args
                .get(1)
                .and_then(|v| match v { Value::Bool(b) => Some(*b), _ => v.as_i64().map(|n| n <= 1) })
                .unwrap_or(true);
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.reinforce_arm_vnsg2(item_id, is_once, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("level", Value::Int(out.level as i32)),
                    ("upLevel", Value::Int(out.up_level as i32)),
                    ("up_level", Value::Int(out.up_level as i32)),
                    ("reinforce_level", Value::Int(out.level as i32)),
                    ("cost_copper", u64_to_amf3(out.cost_copper)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.autoReinforceArm" | "forge.autoReinforceAll" => {
            let squad_pos = arg_i64(req, 0).unwrap_or(1);
            if !(1..=6).contains(&squad_pos) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let mut steps_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let steps = p.auto_reinforce_arm_vnsg2(squad_pos as u8, now_ts)?;
                steps_opt = Some(steps);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let steps = steps_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            // `ArmProxy:onAutoReinforceArmResponse`: `for item_id, step in pairs(ret) do ... end`
            let ret_val = if steps.is_empty() {
                Value::empty_array()
            } else {
                Value::map(
                    steps
                        .into_iter()
                        .map(|(id_str, st)| (id_str, player_json_to_amf3(&st.to_wire_json()))),
                )
            };
            Some(Ok(PlayerRpcSuccess {
                ret: ret_val,
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.upgradeArm" | "forge.upgradeArmUpLevel" | "forge.upgrade" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            if raw_target <= 0 {
                return Some(Err("invalid_forge_target".to_string()));
            }
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };

            // Check if `args[1]` is a refine-stone material table `{ [tid]: count }` (`ArmProxy.lua`)
            let mut mat_map: BTreeMap<u32, u32> = BTreeMap::new();
            if let Some(second) = req.args.get(1) {
                match second {
                    Value::Object(obj) => {
                        for (k, v) in obj.iter() {
                            if let (Ok(tid), Some(cnt)) =
                                (k.parse::<u32>(), val_as_i64_loose(v).filter(|&n| n > 0))
                            {
                                mat_map.insert(tid, cnt as u32);
                            }
                        }
                    }
                    Value::Array { assoc, .. } if !assoc.is_empty() => {
                        for (k, v) in assoc {
                            if let (Ok(tid), Some(cnt)) =
                                (k.parse::<u32>(), val_as_i64_loose(v).filter(|&n| n > 0))
                            {
                                mat_map.insert(tid, cnt as u32);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if !mat_map.is_empty() {
                let mut out_opt = None;
                let res = store.mutate_player(uid, now_ts, |p| {
                    let out = p.upgrade_arm_vnsg2(item_id, &mat_map, now_ts)?;
                    out_opt = Some(out);
                    Ok(())
                });
                if let Err(e) = res {
                    return Some(Err(map_player_err(e)));
                }
                resync_gameplay_with_store(store, gameplay, data_store, uid);
                let out = out_opt.expect("set above");
                let updated = store.get_player(uid).expect("exists");
                let mut pushes = Vec::new();
                if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                    pushes.push(bp);
                }
                pushes.push(build_user_update_env(updated, is_tm, now_i64));
                let consume_val = Value::map(
                    mat_map
                        .iter()
                        .map(|(tid, cnt)| (tid.to_string(), Value::Int(*cnt as i32))),
                );
                return Some(Ok(PlayerRpcSuccess {
                    ret: Value::map([
                        ("ret", Value::String("ok".into())),
                        ("level", Value::Int(out.level as i32)),
                        ("exp", Value::Int(out.exp as i32)),
                        ("consume", consume_val),
                        ("fight_force", u64_to_amf3(updated.fc)),
                    ]),
                    caller_pushes: pushes,
                    peer_pushes: Vec::new(),
                    refresh_roles_pid: None,
                }));
            }

            // Fallback multi-step reinforce when `args[1]` is an integer step count (`[gid_or_item_id, levels]`)
            let levels = arg_i64(req, 1).unwrap_or(1).clamp(1, 20) as u32;
            let cost_copper = u64::from(levels) * 1_500;
            let mut new_reinforce_lv = 1u32;
            let mut bag_upserted: Vec<BagItem> = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Copper, cost_copper, now_ts)?;
                let max_lv = (p.level * 2).max(20);
                let mut found = false;
                for sm in p.formation.arming.values_mut() {
                    for it in sm.values_mut() {
                        if it.item_id == item_id {
                            let cur = it
                                .va_item_text
                                .get("reinforce_level")
                                .and_then(PlayerJson::as_u64)
                                .unwrap_or(1) as u32;
                            new_reinforce_lv = cur.saturating_add(levels).min(max_lv);
                            it.va_item_text.insert(
                                "reinforce_level".into(),
                                PlayerJson::from(new_reinforce_lv),
                            );
                            found = true;
                        }
                    }
                }
                if !found {
                    if let Some((bt, it_ref)) = p.inventory.get_by_item_id(item_id) {
                        let gid = it_ref.gid;
                        if let Some(it) = p.inventory.bags.get_mut(&bt).and_then(|s| s.get_mut(&gid))
                        {
                            let cur = it
                                .va_item_text
                                .get("reinforce_level")
                                .and_then(PlayerJson::as_u64)
                                .unwrap_or(1) as u32;
                            new_reinforce_lv = cur.saturating_add(levels).min(max_lv);
                            it.va_item_text.insert(
                                "reinforce_level".into(),
                                PlayerJson::from(new_reinforce_lv),
                            );
                            bag_upserted.push(it.clone());
                        }
                    }
                }
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_update_env(&bag_upserted, &[], is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("level", Value::Int(new_reinforce_lv as i32)),
                    ("reinforce_level", Value::Int(new_reinforce_lv as i32)),
                    ("cost_copper", u64_to_amf3(cost_copper)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.armWashRefresh"
        | "forge.armWashApply"
        | "forge.armWashCancel"
        | "forge.armOneKeyWash"
        | "forge.armWashTransfer"
        | "forge.wash" => {
            if m == "forge.armWashCancel" || m == "forge.armWashTransfer" {
                return Some(Ok(PlayerRpcSuccess {
                    ret: Value::String("ok".into()),
                    caller_pushes: Vec::new(),
                    peer_pushes: Vec::new(),
                    refresh_roles_pid: None,
                }));
            }
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };
            let apply = m == "forge.armWashApply";
            let lock_mask = arg_i64(req, 1).unwrap_or(0).max(0) as u32;
            let use_gold = arg_i64(req, 2).unwrap_or(0) != 0 || m == "forge.armOneKeyWash";
            let mut wash_json = PlayerJson::Null;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (wj, d) = p.arm_wash(item_id, apply, lock_mask, use_gold, now_ts)?;
                wash_json = wj;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&wash_json),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.armRefineryLock" | "forge.armRefineryUnLock" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            let lock = m == "forge.armRefineryLock";
            let mut bag_upserted = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let Some(item_id) = resolve_any_item_id(p, raw_target as u64) else {
                    return Err(PlayerError::ItemNotFound("lock target missing".into()));
                };
                if let Some((bt, it_ref)) = p.inventory.get_by_item_id(item_id) {
                    let gid = it_ref.gid;
                    if let Some(it) = p.inventory.bags.get_mut(&bt).and_then(|s| s.get_mut(&gid)) {
                        it.va_item_text
                            .insert("refineryLock".into(), PlayerJson::from(lock));
                        bag_upserted.push(it.clone());
                    }
                }
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_update_env(&bag_upserted, &[], is_tm, now_i64) {
                pushes.push(bp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.reinforceTreasure" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };
            let mut mat_ids = Vec::new();
            if let Some(second) = req.args.get(1) {
                collect_u64_ids_from_value(second, &mut mat_ids);
            }
            // Resolve any GIDs in `mat_ids` into `item_id`s
            if let Some(cur) = store.get_player(uid) {
                mat_ids = mat_ids
                    .into_iter()
                    .filter_map(|id| resolve_any_item_id(cur, id))
                    .collect();
            }
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.reinforce_treasure_vnsg2(item_id, &mat_ids, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("level", Value::Int(out.level as i32)),
                    ("exp", Value::Int(out.exp as i32)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.upgradeTreasure" | "forge.oneKeyUpgradeTreasure" | "forge.evolveTreasure" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };
            let one_key = m == "forge.oneKeyUpgradeTreasure";
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.upgrade_treasure_vnsg2(item_id, one_key, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("progress_level", Value::Int(out.progress_level as i32)),
                    ("level", Value::Int(out.progress_level as i32)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.upgradeAwake" | "forge.oneKeyUpgradeAwake" | "awake.upgrade" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };
            let one_key = m == "forge.oneKeyUpgradeAwake";
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.upgrade_awake_vnsg2(item_id, one_key, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("reinforce_level", Value::Int(out.reinforce_level as i32)),
                    ("progress_level", Value::Int(out.progress_level as i32)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.godArmUpgrade"
        | "forge.godArmEvolve"
        | "forge.godArmWash"
        | "forge.godArmOneKeyWash"
        | "forge.godArmReplaceWash"
        | "forge.godArmSaveWash"
        | "forge.godArmCancelWash"
        | "godarm.upgrade"
        | "godarm.evolve"
        | "godarm.wash"
        | "godarm.replaceWash" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };
            let action = match m {
                "forge.godArmEvolve" | "godarm.evolve" => GodArmAction::Evolve,
                "forge.godArmWash" | "forge.godArmOneKeyWash" | "godarm.wash" => GodArmAction::Wash,
                "forge.godArmReplaceWash" | "forge.godArmSaveWash" | "godarm.replaceWash" => {
                    GodArmAction::ReplaceWash
                }
                "forge.godArmCancelWash" => return Some(Ok(PlayerRpcSuccess {
                    ret: Value::String("ok".into()),
                    caller_pushes: Vec::new(),
                    peer_pushes: Vec::new(),
                    refresh_roles_pid: None,
                })),
                _ => GodArmAction::Upgrade,
            };
            let is_ten = m == "forge.godArmOneKeyWash" || arg_i64(req, 1).unwrap_or(1) >= 10;
            let mut res_json = PlayerJson::Null;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (j, d) = p.godarm_wash_upgrade(item_id, action, is_ten, now_ts)?;
                res_json = j;
                delta = d;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&res_json),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.reinforceSpirit"
        | "forge.reinforceSpiritOneKey"
        | "forge.upgradeSpirit"
        | "spirit.reinforce"
        | "spirit.upgrade" => {
            let raw_target = arg_i64(req, 0).unwrap_or(0);
            let Some(item_id) = store
                .get_player(uid)
                .and_then(|p| resolve_any_item_id(p, raw_target as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };
            let mut mat_ids = Vec::new();
            if let Some(second) = req.args.get(1) {
                collect_u64_ids_from_value(second, &mut mat_ids);
            }
            if let Some(cur) = store.get_player(uid) {
                mat_ids = mat_ids
                    .into_iter()
                    .filter_map(|id| resolve_any_item_id(cur, id))
                    .collect();
            }
            let spirit_coin_exp = if mat_ids.is_empty() {
                arg_i64(req, 1).unwrap_or(100).max(1) as u64
            } else {
                0
            };
            let prev_lv = store
                .get_player(uid)
                .and_then(|p| p.peek_item_anywhere(item_id).ok())
                .and_then(|(it, _)| it.va_item_text.get("reinforce_level").and_then(|v| v.as_u64()).map(|n| n as u32))
                .unwrap_or(0);
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.reinforce_spirit_vnsg2(item_id, &mat_ids, spirit_coin_exp, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let up_lv = out.level.saturating_sub(prev_lv);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("level", Value::Int(out.level as i32)),
                    ("upLevel", Value::Int(up_lv as i32)),
                    ("exp", Value::Int(out.exp as i32)),
                    ("total_exp", Value::Int(out.total_exp as i32)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exploreSoul.getInfo" => Some(Ok(PlayerRpcSuccess {
            ret: Value::map([("npc", Value::Int(1)), ("gold_times", Value::Int(0))]),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "exploreSoul.explore"
        | "exploreSoul.quickExplore"
        | "exploreSoul.huntSoul"
        | "exploreSoul.callHighNpc" => {
            let quick = m == "exploreSoul.quickExplore";
            let use_gold = m == "exploreSoul.callHighNpc";
            let times = if quick {
                5u32
            } else {
                arg_i64(req, 0).unwrap_or(1).clamp(1, 10) as u32
            };
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.explore_soul(times, quick, use_gold, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&out.bag_deltas, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            let items_arr: Vec<Value> = out
                .created_spirits
                .iter()
                .map(|i| Value::Int(i.item_template_id as i32))
                .collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("npc", Value::Int(out.npc_stage as i32)),
                    ("spirit_coin", u64_to_amf3(out.spirit_coin_gained)),
                    ("items", Value::dense_array(items_arr)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Castle, Fresher Reward, Arena Top Reports & ActStar Event
        // ====================================================================
        "castle.levelUp" => {
            let mut next_lv = 2u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Copper, 5_000, now_ts)?;
                let cur = p
                    .va_config
                    .get("castle_level")
                    .and_then(PlayerJson::as_u64)
                    .unwrap_or(1) as u32;
                next_lv = cur.saturating_add(1);
                p.set_user_config("castle_level", PlayerJson::from(next_lv))
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("level", Value::Int(next_lv as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "copy.fetchFresherReward" => {
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            if cur
                .va_config
                .get("fresher_reward_claimed")
                .and_then(PlayerJson::as_bool)
                == Some(true)
            {
                return Some(Err("already_claimed".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                p.grant_currency(CurrencyType::Gold, 500, now_ts);
                p.grant_currency(CurrencyType::Copper, 50_000, now_ts);
                p.set_user_config("fresher_reward_claimed", PlayerJson::Bool(true))
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("gold", Value::Int(500)),
                    ("copper", Value::Int(50_000)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "arena.getTopReports" => Some(Ok(PlayerRpcSuccess {
            ret: Value::empty_array(),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "activityStarPlayOpen.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let ret = if profile.act_star_claimed.is_empty() {
                Value::empty_array()
            } else {
                Value::map(
                    profile
                        .act_star_claimed
                        .iter()
                        .map(|&id| (id.to_string(), Value::Bool(true))),
                )
            };
            Some(Ok(PlayerRpcSuccess {
                ret,
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityStarPlayOpen.receiveGift" => {
            let Some(gift_id) = parse_positive_u32_arg(req.args.first()) else {
                return Some(Err("invalid_gift_id".to_string()));
            };
            let Some(tier) = ACT_STAR_GIFT_TIERS.iter().find(|t| t.id == gift_id) else {
                return Some(Err("invalid_gift_id".to_string()));
            };
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            if cur.act_star_claimed.contains(&gift_id) {
                return Some(Err("already_claimed".to_string()));
            }
            if cur.level < tier.req_level {
                return Some(Err("level_not_enough".to_string()));
            }

            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                if p.act_star_claimed.contains(&gift_id) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                if p.level < tier.req_level {
                    return Err(PlayerError::InvalidParameter("level_not_enough".into()));
                }
                if tier.gold > 0 {
                    p.grant_currency(CurrencyType::Gold, tier.gold, now_ts);
                }
                if tier.copper > 0 {
                    p.grant_currency(CurrencyType::Copper, tier.copper, now_ts);
                }
                for &(tid, count) in tier.items {
                    let (_, d) = p.add_item_with_delta(tid, count, now_ts)?;
                    delta.merge(d);
                }
                p.act_star_claimed.push(gift_id);
                p.act_star_claimed.sort_unstable();
                p.act_star_claimed.dedup();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }

            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));

            let mut reward_triplets: Vec<Value> = Vec::new();
            if tier.gold > 0 {
                reward_triplets.push(Value::dense_array(vec![
                    Value::Int(2),
                    Value::Int(0),
                    u64_to_amf3(tier.gold),
                ]));
            }
            if tier.copper > 0 {
                reward_triplets.push(Value::dense_array(vec![
                    Value::Int(1),
                    Value::Int(0),
                    u64_to_amf3(tier.copper),
                ]));
            }
            let mut item_drop_entries: Vec<(String, Value)> = Vec::new();
            for &(tid, count) in tier.items {
                reward_triplets.push(Value::dense_array(vec![
                    Value::Int(4),
                    Value::Int(tid as i32),
                    Value::Int(count as i32),
                ]));
                item_drop_entries.push((tid.to_string(), Value::Int(count as i32)));
            }

            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("id", Value::Int(gift_id as i32)),
                    ("reward", Value::dense_array(reward_triplets)),
                    (
                        "drop",
                        Value::map([
                            ("gold", u64_to_amf3(tier.gold)),
                            ("copper", u64_to_amf3(tier.copper)),
                            ("item", Value::map(item_drop_entries)),
                        ]),
                    ),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityStarPlaySignOpen.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let maked = if profile.act_star_sign_claimed.is_empty() {
                Value::empty_array()
            } else {
                Value::map(
                    profile
                        .act_star_sign_claimed
                        .iter()
                        .map(|&lv| (lv.to_string(), Value::Bool(true))),
                )
            };
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([("maked", maked)]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityStarPlaySignOpen.receiveReward" => {
            let Some(req_lv) = parse_positive_u32_arg(req.args.first()) else {
                return Some(Err("invalid_level_tier".to_string()));
            };
            if !ACT_STAR_SIGN_LEVELS.contains(&req_lv) {
                return Some(Err("invalid_level_tier".to_string()));
            }
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            if cur.act_star_sign_claimed.contains(&req_lv) {
                return Some(Err("already_claimed".to_string()));
            }
            if cur.level < req_lv {
                return Some(Err("level_not_enough".to_string()));
            }

            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                p.grant_currency(CurrencyType::Gold, 100, now_ts);
                let (_, d) = p.add_item_with_delta(17_002, 5, now_ts)?;
                delta.merge(d);
                p.act_star_sign_claimed.push(req_lv);
                p.act_star_sign_claimed.sort_unstable();
                p.act_star_sign_claimed.dedup();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }

            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("lv", Value::Int(req_lv as i32)),
                    (
                        "reward",
                        Value::dense_array(vec![
                            Value::dense_array(vec![Value::Int(2), Value::Int(0), Value::Int(100)]),
                            Value::dense_array(vec![
                                Value::Int(4),
                                Value::Int(17_002),
                                Value::Int(5),
                            ]),
                        ]),
                    ),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        _ => crate::event_pay_rpcs::try_handle_event_or_pay_rpc(
            store, gameplay, data_store, uid, req, m, is_tm, now_ts,
        ),
    }
}
