//! Complete persistent `PlayerProfile` + `GameplayEngine` RPC handlers covering:
//! - Character progression (`user.changeUname`, `user.buyCopper`, `user.buyExecution`, `user.addEnergy`,
//!   `user.openHeroBag`, `user.getVipGiftInfo`, `user.buyVipGift`)
//! - 42-sub-bag inventory mutations (`bag.openGridByGold`, `bag.useItem`, `exchange.useItemInBag`,
//!   `exchange.useItemInBagByTpl`, `bag.sellItems`, `bag.sellItem`, `bag.takeTmpBag`,
//!   `exchange.composeHero`, `exchange.composeArm`)
//! - Hero roster progression (`hero.getAllHeroes`, `hero.heroLevelUp`, `hero.skillLevelUp`,
//!   `evolve.doEvolve`, `evolve.surmount`, `hero.upgrade`, `hero.lock`, `hero.unLock`)
//! - 6-slot squad, 3x3 battle grid, LittleFriend `partner`, 9-slot `arming` & Forge (`formation.*`, `hero.*Arming`, `forge.*`)
//! - Extended Castle, Raffle, Copy, Arena & Sign endpoints (`castle.levelUp`, `raffle.refreshHotByHand`,
//!   `copy.oneKeyResetAtk`, `copy.fetchFresherReward`, `copy.getStronghRecord`, `copy.getFamousPassTimes`,
//!   `arena.quickChallenge`, `arena.buyChallengeTimes`, `arena.fetchChallengeNumReward`,
//!   `arena.getTopReports`, `sign.gainRechargeSignReward`)

use std::collections::BTreeMap;

use gserver_gameplay::engine::GameplayEngine;
use gserver_player::{
    BagItem, BagType, CurrencyType, EquipSlotType, JsonValue as PlayerJson, PlayerError,
    PlayerProfile, PlayerStore, StarterItemSeed, UseItemEffect,
};
use gserver_protocol::{RpcRequest, S2cEnvelope, Value};

use crate::bridge::{dto_to_amf3, player_json_to_amf3, profile_to_gameplay_state, u64_to_amf3};

pub struct PlayerRpcSuccess {
    pub ret: Value,
    pub caller_pushes: Vec<S2cEnvelope>,
    pub peer_pushes: Vec<(u64, S2cEnvelope)>,
    pub refresh_roles_pid: Option<String>,
}

fn map_player_err(err: PlayerError) -> String {
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
            } else {
                format!("invalid_parameter:{msg}")
            }
        }
        other => format!("player_error:{other}"),
    }
}

fn build_user_update_env(profile: &PlayerProfile, now_i64: i64) -> S2cEnvelope {
    let payload = Value::map([
        ("uname", Value::String(profile.uname.clone())),
        ("level", Value::Int(profile.level as i32)),
        ("exp", u64_to_amf3(profile.exp)),
        ("vip", Value::Int(profile.vip as i32)),
        ("vip_exp", u64_to_amf3(profile.vip_exp)),
        ("gold", u64_to_amf3(profile.currency(CurrencyType::Gold))),
        (
            "gold_bind",
            u64_to_amf3(profile.currency(CurrencyType::GoldBind)),
        ),
        (
            "copper",
            Value::String(profile.currency(CurrencyType::Copper).to_string()),
        ),
        (
            "hero_soul",
            u64_to_amf3(profile.currency(CurrencyType::HeroSoul)),
        ),
        (
            "prestige",
            u64_to_amf3(profile.currency(CurrencyType::Prestige)),
        ),
        (
            "execution",
            u64_to_amf3(profile.currency(CurrencyType::Execution)),
        ),
        ("execution_time", u64_to_amf3(profile.execution_time)),
        (
            "energy",
            u64_to_amf3(profile.currency(CurrencyType::Energy)),
        ),
        ("energy_time", u64_to_amf3(profile.energy_time)),
        ("fight_force", u64_to_amf3(profile.fc)),
        ("fight_force_max", u64_to_amf3(profile.fight_force_max)),
        (
            "hero_bag_buy_times",
            Value::Int(profile.hero_bag_buy_times as i32),
        ),
    ]);
    S2cEnvelope::push("user.updateUser", payload).with_time(now_i64)
}

fn build_bag_update_env(
    upserted: &[BagItem],
    removed_gids: &[u32],
    now_i64: i64,
) -> Option<S2cEnvelope> {
    if upserted.is_empty() && removed_gids.is_empty() {
        return None;
    }
    let mut entries: Vec<(String, Value)> = Vec::with_capacity(upserted.len() + removed_gids.len());
    for gid in removed_gids {
        entries.push((gid.to_string(), Value::empty_array()));
    }
    for item in upserted {
        entries.push((
            item.gid.to_string(),
            player_json_to_amf3(&item.to_wire_json()),
        ));
    }
    let payload = Value::dense_array(vec![Value::map(entries)]);
    Some(S2cEnvelope::push("bag.update", payload).with_time(now_i64))
}

fn build_new_heroes_env(
    profile: &PlayerProfile,
    hids: &[u64],
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
    Some(S2cEnvelope::push("re.hero.new", Value::map(entries)).with_time(now_i64))
}

fn resync_gameplay(store: &PlayerStore, gameplay: &mut GameplayEngine, uid: u64) {
    if let Some(profile) = store.get_player(uid) {
        let mut gp = profile_to_gameplay_state(profile);
        if let Some(prev) = gameplay.players.get(&uid) {
            gp.online_accum_secs = prev.online_accum_secs;
            gp.chat_ban_until = prev.chat_ban_until;
        }
        gameplay.register_player(gp);
    }
}

fn infer_item_effect(tid: u32) -> UseItemEffect {
    match tid {
        22102 => UseItemEffect::GrantCurrency {
            currency: CurrencyType::Execution,
            amount_per_item: 25,
        },
        22202 => UseItemEffect::GrantCurrency {
            currency: CurrencyType::Energy,
            amount_per_item: 10,
        },
        20_001..=25_000 => UseItemEffect::GrantCurrency {
            currency: CurrencyType::Execution,
            amount_per_item: 20,
        },
        25_001..=30_000 => UseItemEffect::AddPlayerExp { exp_per_item: 250 },
        30_001..=40_000 => UseItemEffect::GrantItems {
            items_per_use: vec![
                StarterItemSeed {
                    item_template_id: 16012,
                    count: 2,
                },
                StarterItemSeed {
                    item_template_id: 10_001,
                    count: 5,
                },
            ],
        },
        1_200_001..=1_300_000 => UseItemEffect::AddPlayerExp { exp_per_item: 500 },
        _ => UseItemEffect::GrantCurrency {
            currency: CurrencyType::Copper,
            amount_per_item: 10_000,
        },
    }
}

fn parse_squad_hid_and_slot(args: &[Value], allow_zero_hid: bool) -> Result<(u64, u8), String> {
    let a = args
        .first()
        .and_then(Value::as_i64)
        .ok_or_else(|| "missing_first_arg".to_string())?;
    let b = args
        .get(1)
        .and_then(Value::as_i64)
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

pub fn try_handle_player_rpc(
    store: &mut PlayerStore,
    gameplay: &mut GameplayEngine,
    uid: u64,
    req: &RpcRequest,
    now_ts: u64,
) -> Option<Result<PlayerRpcSuccess, String>> {
    let now_i64 = now_ts as i64;

    match req.method.as_str() {
        // ====================================================================
        // Domain 3: User Progression, Rename, Stamina/Copper & VIP Gifts
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
            // Enforce unique character name across online/loaded players in same group
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
            resync_gameplay(store, gameplay, uid);
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
            let push = build_user_update_env(updated, now_i64);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![push],
                peer_pushes,
                refresh_roles_pid: Some(pid),
            }))
        }

        "user.buyCopper" => {
            let count = req
                .args
                .first()
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 50) as u64;
            let gold_cost = 20 * count;
            let copper_gain = 100_000 * count;
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
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let push = build_user_update_env(updated, now_i64);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("copper", u64_to_amf3(copper_gain)),
                    ("crit", Value::Int(1)),
                ]),
                caller_pushes: vec![push],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.buyExecution" => {
            let count = req
                .args
                .first()
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 20) as u64;
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
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.addEnergy" => {
            let count = req
                .args
                .first()
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 20) as u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, 20 * count, now_ts)?;
                p.grant_currency(CurrencyType::Energy, 10 * count, now_ts);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.openHeroBag" => {
            let times = req
                .args
                .first()
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 10) as u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, u64::from(times) * 50, now_ts)?;
                p.hero_bag_buy_times = p.hero_bag_buy_times.saturating_add(times);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.getVipGiftInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let bought = read_u32_set_from_va(profile, "vip_gift_bought");
            let flags: Vec<Value> = (1..=12u32)
                .map(|vip_lv| Value::Int(i32::from(bought.contains(&vip_lv))))
                .collect();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::dense_array(flags),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "user.buyVipGift" => {
            let vip_lv = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if !(1..=15).contains(&vip_lv) {
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

            let mut added_items: Vec<BagItem> = Vec::new();
            let cost_gold = u64::from(vip_u32) * 50;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, cost_gold, now_ts)?;
                let added = p.add_item(16012, vip_u32 * 5, now_ts)?;
                added_items.extend(added);
                bought.push(vip_u32);
                write_u32_set_to_va(p, "vip_gift_bought", &bought);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(&added_items, &[], now_i64) {
                pushes.push(bp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Domain 4: Inventory / 42 Sub-Bags / Item Use / Sell / Compose
        // ====================================================================
        "bag.openGridByGold" => {
            let add_grids = req
                .args
                .first()
                .and_then(Value::as_i64)
                .unwrap_or(5)
                .clamp(1, 100) as u32;
            let bag_type = match req.args.get(1) {
                Some(v) if v.as_str().is_some() => {
                    BagType::from_key(v.as_str().unwrap()).unwrap_or(BagType::Item)
                }
                Some(v) if v.as_i64().is_some() => match v.as_i64().unwrap() {
                    1 => BagType::Arm,
                    2 => BagType::Item,
                    3 => BagType::Treasure,
                    4 => BagType::Soul,
                    _ => BagType::Item,
                },
                _ => BagType::Item,
            };
            let gold_cost = u64::from(add_grids) * 10;
            let mut new_max = 0u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
                let cur_max = p
                    .inventory
                    .grid_max_num
                    .get(&bag_type)
                    .copied()
                    .unwrap_or_else(|| bag_type.default_max_grids());
                new_max = cur_max.saturating_add(add_grids);
                p.inventory.grid_max_num.insert(bag_type, new_max);
                let opens = p
                    .inventory
                    .grid_open_times
                    .get(&bag_type)
                    .copied()
                    .unwrap_or(0);
                p.inventory
                    .grid_open_times
                    .insert(bag_type, opens.saturating_add(1));
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("bag_type", Value::String(bag_type.vnsg2_key().into())),
                    ("grid_max", Value::Int(new_max as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "bag.useItem" | "exchange.useItemInBag" => {
            let gid_i64 = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if gid_i64 <= 0 {
                return Some(Err("invalid_gid".to_string()));
            }
            let gid = gid_i64 as u32;
            let Some(existing) = store
                .get_player(uid)
                .and_then(|p| p.inventory.get_by_gid(gid).cloned())
            else {
                return Some(Err("item_not_found".to_string()));
            };

            let count_i64 = if req.args.len() >= 3 {
                let passed_item_id = req.args.get(1).and_then(Value::as_i64).unwrap_or(0);
                if passed_item_id > 0 && passed_item_id as u64 != existing.item_id {
                    return Some(Err("item_id_mismatch".to_string()));
                }
                req.args.get(2).and_then(Value::as_i64).unwrap_or(1)
            } else {
                req.args.get(1).and_then(Value::as_i64).unwrap_or(1)
            };
            if count_i64 <= 0 {
                return Some(Err("invalid_item_count".to_string()));
            }
            let count = count_i64 as u32;
            let effect = infer_item_effect(existing.item_template_id);

            let mut outcome_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.use_item(gid, count, effect.clone(), now_ts)?;
                outcome_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let out = outcome_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut upserted = out.granted_items.clone();
            let mut removed = Vec::new();
            if out.remaining_in_grid == 0 {
                removed.push(gid);
            } else if let Some(rem_item) = updated.inventory.get_by_gid(gid) {
                upserted.push(rem_item.clone());
            }

            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(&upserted, &removed, now_i64) {
                pushes.push(bp);
            }
            if let Some(hp) = build_new_heroes_env(updated, &out.summoned_hids, now_i64) {
                pushes.push(hp);
            }
            let pid = updated.pid.clone();
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("remaining", Value::Int(out.remaining_in_grid as i32)),
                    ("daily_used", Value::Int(out.daily_used_total as i32)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        "exchange.useItemInBagByTpl" => {
            let tid_i64 = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            let count_i64 = req.args.get(1).and_then(Value::as_i64).unwrap_or(1);
            if tid_i64 <= 0 || count_i64 <= 0 {
                return Some(Err("invalid_item_args".to_string()));
            }
            let tid = tid_i64 as u32;
            let count = count_i64 as u32;
            let Some(found_gid) = store.get_player(uid).and_then(|p| {
                for sub in p.inventory.bags.values() {
                    for it in sub.values() {
                        if it.item_template_id == tid {
                            return Some(it.gid);
                        }
                    }
                }
                None
            }) else {
                return Some(Err("item_not_found".to_string()));
            };

            let effect = infer_item_effect(tid);
            let mut outcome_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.use_item(found_gid, count, effect.clone(), now_ts)?;
                outcome_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let out = outcome_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut upserted = out.granted_items.clone();
            let mut removed = Vec::new();
            if out.remaining_in_grid == 0 {
                removed.push(found_gid);
            } else if let Some(rem_item) = updated.inventory.get_by_gid(found_gid) {
                upserted.push(rem_item.clone());
            }
            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(&upserted, &removed, now_i64) {
                pushes.push(bp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("remaining", Value::Int(out.remaining_in_grid as i32)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "bag.sellItems" | "bag.sellItem" => {
            // Accepts either `[[gid, item_id, num], ...]` (`bag.sellItems`) or `(gid, item_id, num)` (`bag.sellItem`)
            let mut sales: Vec<(u32, u32)> = Vec::new();
            if let Some(Value::Array { dense, .. }) = req.args.first() {
                for entry in dense {
                    if let Value::Array { dense: row, .. } = entry {
                        let gid = row.first().and_then(Value::as_i64).unwrap_or(0);
                        let num = row
                            .get(2)
                            .or_else(|| row.get(1))
                            .and_then(Value::as_i64)
                            .unwrap_or(1);
                        if gid <= 0 || num <= 0 {
                            return Some(Err("invalid_sell_entry".to_string()));
                        }
                        sales.push((gid as u32, num as u32));
                    }
                }
            } else {
                let gid = req.args.first().and_then(Value::as_i64).unwrap_or(0);
                let num = req
                    .args
                    .get(2)
                    .or_else(|| req.args.get(1))
                    .and_then(Value::as_i64)
                    .unwrap_or(1);
                if gid > 0 && num > 0 {
                    sales.push((gid as u32, num as u32));
                }
            }
            if sales.is_empty() {
                return Some(Err("empty_sell_list".to_string()));
            }

            let mut upserted = Vec::new();
            let mut removed = Vec::new();
            let mut total_copper = 0u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                for &(gid, num) in &sales {
                    let snap = p.remove_item_by_gid(gid, num)?;
                    total_copper = total_copper.saturating_add(u64::from(num) * 1_000);
                    if snap.item_num == 0 {
                        removed.push(gid);
                    } else {
                        upserted.push(snap);
                    }
                }
                p.grant_currency(CurrencyType::Copper, total_copper, now_ts);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(&upserted, &removed, now_i64) {
                pushes.push(bp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("copper", u64_to_amf3(total_copper)),
                ]),
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

        "exchange.composeHero" => {
            let raw_id = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            let count = req
                .args
                .get(1)
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 10) as u32;
            if raw_id <= 0 {
                return Some(Err("invalid_fragment_id".to_string()));
            }
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            // Resolve either fragment `gid` (`50_001..60_000`) or fragment `template_id` (`50_001..60_000`)
            let frag_tid = cur
                .inventory
                .get_by_gid(raw_id as u32)
                .map(|it| it.item_template_id)
                .unwrap_or(raw_id as u32);
            if !(50_001..=60_000).contains(&frag_tid) {
                return Some(Err("not_hero_fragment".to_string()));
            }
            let req_frags = 30 * count;
            let summoned_htid = 300 + ((frag_tid - 50_000) % 200).max(1);

            let mut affected_gids = Vec::new();
            let mut summoned_hids = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                if (p.heroes.len() as u32).saturating_add(count) > p.max_hero_capacity() {
                    return Err(PlayerError::HeroBagFull {
                        max_heroes: p.max_hero_capacity(),
                    });
                }
                affected_gids = p.remove_item_by_template(frag_tid, req_frags)?;
                for _ in 0..count {
                    summoned_hids.push(p.summon_hero(summoned_htid, 1)?);
                }
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut upserted = Vec::new();
            let mut removed = Vec::new();
            for (gid, left) in affected_gids {
                if left == 0 {
                    removed.push(gid);
                } else if let Some(it) = updated.inventory.get_by_gid(gid) {
                    upserted.push(it.clone());
                }
            }
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_update_env(&upserted, &removed, now_i64) {
                pushes.push(bp);
            }
            if let Some(hp) = build_new_heroes_env(updated, &summoned_hids, now_i64) {
                pushes.push(hp);
            }
            let first_hid = summoned_hids.first().copied().unwrap_or(0);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(first_hid)),
                    ("htid", Value::Int(summoned_htid as i32)),
                    (
                        "hids",
                        Value::dense_array(summoned_hids.into_iter().map(u64_to_amf3).collect()),
                    ),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "exchange.composeArm" => {
            let raw_id = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            let count = req
                .args
                .get(1)
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 10) as u32;
            if raw_id <= 0 {
                return Some(Err("invalid_fragment_id".to_string()));
            }
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let frag_tid = cur
                .inventory
                .get_by_gid(raw_id as u32)
                .map(|it| it.item_template_id)
                .unwrap_or(raw_id as u32);

            let (req_per_craft, out_tid) = if (3_100_001..=3_200_000).contains(&frag_tid) {
                (20u32, 101_102u32)
            } else if (400_000_001..=500_000_000).contains(&frag_tid) {
                (10u32, 180_102u32)
            } else {
                return Some(Err("not_equip_fragment".to_string()));
            };

            let mut affected_gids = Vec::new();
            let mut crafted = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                affected_gids = p.remove_item_by_template(frag_tid, req_per_craft * count)?;
                crafted = p.add_item(out_tid, count, now_ts)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut upserted = crafted.clone();
            let mut removed = Vec::new();
            for (gid, left) in affected_gids {
                if left == 0 {
                    removed.push(gid);
                } else if let Some(it) = updated.inventory.get_by_gid(gid) {
                    upserted.push(it.clone());
                }
            }
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_update_env(&upserted, &removed, now_i64) {
                pushes.push(bp);
            }
            let first_gid = crafted.first().map(|i| i.gid).unwrap_or(0);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("item_template_id", Value::Int(out_tid as i32)),
                    ("gid", Value::Int(first_gid as i32)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Domain 5: Hero Roster, Level-Up, Skill, Evolve, Breakthrough & Lock
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

        "hero.heroLevelUp" => {
            let hid = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let levels = req
                .args
                .get(1)
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 50) as u32;
            let copper_cost = u64::from(levels) * 1_000;
            let soul_cost = u64::from(levels) * 100;
            let mut out_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let out = p.level_up_hero(hid as u64, levels, copper_cost, soul_cost, now_ts)?;
                out_opt = Some(out);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let out = out_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(out.hid)),
                    ("level", Value::Int(out.new_level as i32)),
                    ("exp", u64_to_amf3(out.exp)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "hero.skillLevelUp" => {
            let hid = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let skill_id = req.args.get(1).and_then(Value::as_i64).unwrap_or(1001).max(1) as u32;
            let mut new_star_lv = 1u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                if !p.heroes.contains_key(&(hid as u64)) {
                    return Err(PlayerError::HeroNotFound(hid as u64));
                }
                p.consume_currency(CurrencyType::Copper, 2_000, now_ts)?;
                let hero = p.heroes.get_mut(&(hid as u64)).expect("checked");
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
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(hid as u64)),
                    ("skill_lv", Value::Int(new_star_lv as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "evolve.doEvolve" => {
            let hid = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let mut sac_hids: Vec<u64> = Vec::new();
            if let Some(Value::Array { dense, .. }) = req.args.get(1) {
                for v in dense {
                    if let Some(sh) = v.as_i64() {
                        if sh > 0 {
                            sac_hids.push(sh as u64);
                        }
                    }
                }
            }
            let mut new_ev = 0u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                new_ev = p.evolve_hero(hid as u64, &sac_hids, 5_000, now_ts)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(hid as u64)),
                    ("evolve_level", Value::Int(new_ev as i32)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "evolve.surmount" | "hero.upgrade" => {
            let hid = req.args.first().and_then(Value::as_i64).unwrap_or(0);
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
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("hid", u64_to_amf3(hid as u64)),
                    ("upgrade_level", Value::Int(new_up as i32)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "hero.lock" | "hero.unLock" => {
            let hid = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if hid <= 0 {
                return Some(Err("invalid_hid".to_string()));
            }
            let lock = req.method == "hero.lock";
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

        // ====================================================================
        // Domain 6: Squad, 3x3 Battle Grid, Partner, 9-Slot Arming & Forge
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

        "formation.getOtherPositionInfo" => {
            let target_uid = req.args.first().and_then(Value::as_i64).unwrap_or(0);
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
                p.set_squad_slot(slot, hid)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.unsetHeroFromPosition" => {
            let slot_i64 = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if !(1..=6).contains(&slot_i64) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                p.unset_squad_slot(slot_i64 as u8)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.transHero" => {
            let sa = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            let sb = req.args.get(1).and_then(Value::as_i64).unwrap_or(0);
            if !(1..=6).contains(&sa) || !(1..=6).contains(&sb) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                p.swap_squad_slots(sa as u8, sb as u8)
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: Vec::new(),
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
                        if let Some(hid) = v.as_i64() {
                            if hid > 0 {
                                grid_map.insert((idx + 1) as u8, hid as u64);
                            }
                        }
                    }
                    for (k, v) in assoc {
                        let cell: u8 = k
                            .parse()
                            .map_err(|_| "invalid_formation_slot".to_string())
                            .unwrap_or(0);
                        if let Some(hid) = v.as_i64() {
                            if hid > 0 {
                                grid_map.insert(cell, hid as u64);
                            }
                        }
                    }
                }
                Value::Object(obj) => {
                    for (k, v) in obj.iter() {
                        let cell: u8 = k
                            .parse()
                            .map_err(|_| "invalid_formation_slot".to_string())
                            .unwrap_or(0);
                        if let Some(hid) = v.as_i64() {
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
            let slot = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if !(1..=6).contains(&slot) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| p.open_partner_slot(slot as u8));
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
            let (hid, slot) = match parse_squad_hid_and_slot(&req.args, true) {
                Ok(pair) => pair,
                Err(code) => return Some(Err(code)),
            };
            let res = store.mutate_player(uid, now_ts, |p| {
                p.set_partner_slot(slot, hid)?;
                Ok(())
            });
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

        "formation.unsetMultiPartner" => {
            let mut slots: Vec<u8> = Vec::new();
            if let Some(Value::Array { dense, .. }) = req.args.first() {
                for v in dense {
                    if let Some(s) = v.as_i64() {
                        if !(1..=6).contains(&s) {
                            return Some(Err("invalid_formation_slot".to_string()));
                        }
                        slots.push(s as u8);
                    }
                }
            } else {
                slots.extend(1..=6u8);
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                for s in &slots {
                    if p.formation.partner.contains_key(s) {
                        p.set_partner_slot(*s, 0)?;
                    }
                }
                Ok(())
            });
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

        "formation.addArming" | "hero.addArming" => {
            let squad_slot = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            let equip_slot_raw = req.args.get(1).and_then(Value::as_i64).unwrap_or(0);
            let item_or_gid = req.args.get(2).and_then(Value::as_i64).unwrap_or(0);
            if !(1..=6).contains(&squad_slot) || !(1..=9).contains(&equip_slot_raw) || item_or_gid <= 0
            {
                return Some(Err("invalid_equip_args".to_string()));
            }
            let equip_slot = match EquipSlotType::from_u8(equip_slot_raw as u8) {
                Ok(es) => es,
                Err(e) => return Some(Err(map_player_err(e))),
            };
            let Some(bag_gid) = store
                .get_player(uid)
                .and_then(|p| resolve_bag_gid(p, item_or_gid as u64))
            else {
                return Some(Err("item_not_found".to_string()));
            };

            let mut returned_item: Option<BagItem> = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                returned_item = p.equip_item(squad_slot as u8, equip_slot, bag_gid)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let upserted: Vec<BagItem> = returned_item.into_iter().collect();
            let removed = if upserted.iter().any(|i| i.gid == bag_gid) {
                Vec::new()
            } else {
                vec![bag_gid]
            };
            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(&upserted, &removed, now_i64) {
                pushes.push(bp);
            }
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
            let squad_slot = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if !(1..=6).contains(&squad_slot) {
                return Some(Err("invalid_formation_slot".to_string()));
            }
            let mut pairs: Vec<(EquipSlotType, u64)> = Vec::new();
            if let Some(Value::Array { dense, assoc }) = req.args.get(1) {
                for (idx, v) in dense.iter().enumerate() {
                    if let Some(id) = v.as_i64() {
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
                    if let Some(id) = v.as_i64() {
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
                    if let Some(id) = v.as_i64() {
                        if id > 0 {
                            pairs.push((es, id as u64));
                        }
                    }
                }
            }
            if pairs.is_empty() {
                return Some(Err("empty_arming_list".to_string()));
            }
            let mut upserted = Vec::new();
            let mut removed = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                for &(es, raw_id) in &pairs {
                    let gid = resolve_bag_gid(p, raw_id)
                        .ok_or_else(|| PlayerError::ItemNotFound(format!("id={raw_id}")))?;
                    removed.push(gid);
                    if let Some(ret_it) = p.equip_item(squad_slot as u8, es, gid)? {
                        upserted.push(ret_it);
                    }
                }
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            removed.retain(|g| !upserted.iter().any(|u| u.gid == *g));
            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(&upserted, &removed, now_i64) {
                pushes.push(bp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "formation.removeArming" | "hero.removeArming" => {
            let squad_slot = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            let equip_slot_raw = req.args.get(1).and_then(Value::as_i64).unwrap_or(0);
            if !(1..=6).contains(&squad_slot) || !(1..=9).contains(&equip_slot_raw) {
                return Some(Err("invalid_equip_args".to_string()));
            }
            let equip_slot = match EquipSlotType::from_u8(equip_slot_raw as u8) {
                Ok(es) => es,
                Err(e) => return Some(Err(map_player_err(e))),
            };
            let mut bag_item_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let it = p.unequip_item(squad_slot as u8, equip_slot)?;
                bag_item_opt = Some(it);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let bag_item = bag_item_opt.expect("set above");
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(std::slice::from_ref(&bag_item), &[], now_i64) {
                pushes.push(bp);
            }
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
            let from_slot = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            let to_slot = req.args.get(1).and_then(Value::as_i64).unwrap_or(0);
            let equip_slot = req.args.get(2).and_then(Value::as_i64).unwrap_or(1);
            if !(1..=6).contains(&from_slot)
                || !(1..=6).contains(&to_slot)
                || !(1..=9).contains(&equip_slot)
            {
                return Some(Err("invalid_equip_args".to_string()));
            }
            let fs = from_slot as u8;
            let ts = to_slot as u8;
            let es = equip_slot as u8;
            let res = store.mutate_player(uid, now_ts, |p| {
                if fs == ts {
                    return Ok(());
                }
                let src_item = p
                    .formation
                    .arming
                    .get_mut(&fs)
                    .and_then(|m| m.remove(&es))
                    .ok_or_else(|| {
                        PlayerError::ItemNotFound(format!("no arming at slot={fs}, equip={es}"))
                    })?;
                let dst_old = p
                    .formation
                    .arming
                    .entry(ts)
                    .or_default()
                    .insert(es, src_item);
                if let Some(prev) = dst_old {
                    p.formation.arming.entry(fs).or_default().insert(es, prev);
                }
                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "forge.upgradeArm" | "forge.autoReinforceArm" => {
            let target_id = req.args.first().and_then(Value::as_i64).unwrap_or(0);
            if target_id <= 0 {
                return Some(Err("invalid_forge_target".to_string()));
            }
            let levels = if req.method == "forge.autoReinforceArm" {
                5u32
            } else {
                req.args
                    .get(1)
                    .and_then(Value::as_i64)
                    .unwrap_or(1)
                    .clamp(1, 20) as u32
            };
            let cost_copper = u64::from(levels) * 1_500;
            let mut new_reinforce_lv = 1u32;
            let mut bag_upserted: Vec<BagItem> = Vec::new();

            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Copper, cost_copper, now_ts)?;
                let max_lv = (p.level * 2).max(20);
                let tid_u64 = target_id as u64;

                // Search equipped arming first (by `item_id` OR `squad_slot` 1..=6 for `autoReinforceArm`)
                let mut found_equipped = false;
                if (1..=6).contains(&target_id) && req.method == "forge.autoReinforceArm" {
                    if let Some(slot_map) = p.formation.arming.get_mut(&(target_id as u8)) {
                        for item in slot_map.values_mut() {
                            let cur = item
                                .va_item_text
                                .get("reinforce_level")
                                .and_then(PlayerJson::as_u64)
                                .unwrap_or(1) as u32;
                            new_reinforce_lv = cur.saturating_add(levels).min(max_lv);
                            item.va_item_text.insert(
                                "reinforce_level".into(),
                                PlayerJson::from(new_reinforce_lv),
                            );
                            found_equipped = true;
                        }
                    }
                }
                if !found_equipped {
                    for slot_map in p.formation.arming.values_mut() {
                        for item in slot_map.values_mut() {
                            if item.item_id == tid_u64 {
                                let cur = item
                                    .va_item_text
                                    .get("reinforce_level")
                                    .and_then(PlayerJson::as_u64)
                                    .unwrap_or(1) as u32;
                                new_reinforce_lv = cur.saturating_add(levels).min(max_lv);
                                item.va_item_text.insert(
                                    "reinforce_level".into(),
                                    PlayerJson::from(new_reinforce_lv),
                                );
                                found_equipped = true;
                                break;
                            }
                        }
                        if found_equipped {
                            break;
                        }
                    }
                }

                // Otherwise search inventory bags by `gid` or `item_id`
                if !found_equipped {
                    let gid = resolve_bag_gid(p, tid_u64).ok_or_else(|| {
                        PlayerError::ItemNotFound(format!("forge target {tid_u64} not found"))
                    })?;
                    let bt = BagType::from_gid(gid)
                        .ok_or_else(|| PlayerError::ItemNotFound("bad gid".into()))?;
                    let sub = p
                        .inventory
                        .bags
                        .get_mut(&bt)
                        .ok_or_else(|| PlayerError::ItemNotFound("missing bag".into()))?;
                    let item = sub
                        .get_mut(&gid)
                        .ok_or_else(|| PlayerError::ItemNotFound("missing item".into()))?;
                    let cur = item
                        .va_item_text
                        .get("reinforce_level")
                        .and_then(PlayerJson::as_u64)
                        .unwrap_or(1) as u32;
                    new_reinforce_lv = cur.saturating_add(levels).min(max_lv);
                    item.va_item_text
                        .insert("reinforce_level".into(), PlayerJson::from(new_reinforce_lv));
                    bag_upserted.push(item.clone());
                }

                p.recalculate_fight_force();
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = vec![build_user_update_env(updated, now_i64)];
            if let Some(bp) = build_bag_update_env(&bag_upserted, &[], now_i64) {
                pushes.push(bp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("reinforce_level", Value::Int(new_reinforce_lv as i32)),
                    ("cost_copper", u64_to_amf3(cost_copper)),
                    ("fight_force", u64_to_amf3(updated.fc)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Domains 7–12: Extended Castle, Raffle, Copy, Arena & Sign RPCs
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
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("level", Value::Int(next_lv as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "raffle.refreshHotByHand" => {
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, 20, now_ts)?;
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            let info = dto_to_amf3(&gameplay.raffle.get_info(uid, now_ts), true);
            Some(Ok(PlayerRpcSuccess {
                ret: info,
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "copy.oneKeyResetAtk" => {
            let chap_id = req.args.first().and_then(Value::as_i64).unwrap_or(1001).max(1) as u32;
            let copy_id = req
                .args
                .get(1)
                .and_then(Value::as_i64)
                .unwrap_or(100101)
                .max(1) as u32;
            let Some(gp) = gameplay.players.get_mut(&uid) else {
                return Some(Err("user_not_found".to_string()));
            };
            match gameplay.copy.reset_attack_count(gp, chap_id, copy_id, now_ts) {
                Ok((ret_dto, pushes_dto)) => {
                    let gp_clone = gp.clone();
                    let _ = store.mutate_player(uid, now_ts, |p| {
                        crate::bridge::sync_gameplay_back_to_profile(&gp_clone, p, now_ts);
                        Ok(())
                    });
                    let caller_pushes = pushes_dto
                        .into_iter()
                        .map(|cp| {
                            S2cEnvelope::push(cp.callback_name, dto_to_amf3(&cp.ret, true))
                                .with_time(now_i64)
                        })
                        .collect();
                    Some(Ok(PlayerRpcSuccess {
                        ret: dto_to_amf3(&ret_dto, true),
                        caller_pushes,
                        peer_pushes: Vec::new(),
                        refresh_roles_pid: None,
                    }))
                }
                Err(code) => Some(Err(code.to_string())),
            }
        }

        "copy.fetchFresherReward" => {
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            if cur
                .va_config
                .get("fresher_reward_claimed")
                .and_then(|v| match v {
                    PlayerJson::Bool(b) => Some(*b),
                    _ => None,
                })
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
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("gold", Value::Int(500)),
                    ("copper", Value::Int(50_000)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "copy.getStronghRecord" | "copy.getFamousPassTimes" => Some(Ok(PlayerRpcSuccess {
            ret: Value::empty_array(),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "arena.quickChallenge" => {
            let target_pos = req
                .args
                .get(1)
                .or_else(|| req.args.first())
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .max(1) as u32;
            let Some(gp) = gameplay.players.get_mut(&uid) else {
                return Some(Err("user_not_found".to_string()));
            };
            match gameplay
                .arena
                .challenge(gp, target_pos, &mut gameplay.copy, now_ts)
            {
                Ok((ret_dto, sp, pp)) => {
                    let gp_clone = gp.clone();
                    let _ = store.mutate_player(uid, now_ts, |p| {
                        crate::bridge::sync_gameplay_back_to_profile(&gp_clone, p, now_ts);
                        Ok(())
                    });
                    let caller_pushes = sp
                        .into_iter()
                        .map(|cp| {
                            S2cEnvelope::push(cp.callback_name, dto_to_amf3(&cp.ret, true))
                                .with_time(now_i64)
                        })
                        .collect();
                    let peer_pushes = pp
                        .into_iter()
                        .map(|(tuid, pkt)| {
                            (
                                tuid,
                                S2cEnvelope::push(pkt.callback_name, dto_to_amf3(&pkt.ret, true))
                                    .with_time(now_i64),
                            )
                        })
                        .collect();
                    Some(Ok(PlayerRpcSuccess {
                        ret: dto_to_amf3(&ret_dto, true),
                        caller_pushes,
                        peer_pushes,
                        refresh_roles_pid: None,
                    }))
                }
                Err(code) => Some(Err(code.to_string())),
            }
        }

        "arena.buyChallengeTimes" => {
            let num = req
                .args
                .first()
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .clamp(1, 20) as u32;
            let cost = u64::from(num) * 20;
            let mut total_bought = 0u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, cost, now_ts)?;
                let cur = p
                    .va_config
                    .get("arena_buy_times")
                    .and_then(PlayerJson::as_u64)
                    .unwrap_or(0) as u32;
                total_bought = cur.saturating_add(num);
                p.set_user_config("arena_buy_times", PlayerJson::from(total_bought))
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("ret", Value::String("ok".into())),
                    ("buy_num", Value::Int(total_bought as i32)),
                ]),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "arena.fetchChallengeNumReward" => {
            let reward_id = req.args.first().and_then(Value::as_i64).unwrap_or(1).max(1) as u32;
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            let mut claimed = read_u32_set_from_va(cur, "arena_num_rewards");
            if claimed.contains(&reward_id) {
                return Some(Err("already_claimed".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                p.grant_currency(CurrencyType::Gold, 200, now_ts);
                p.grant_currency(CurrencyType::Prestige, 500, now_ts);
                claimed.push(reward_id);
                write_u32_set_to_va(p, "arena_num_rewards", &claimed);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
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

        "sign.gainRechargeSignReward" => {
            let reward_id = req.args.first().and_then(Value::as_i64).unwrap_or(1).max(1) as u32;
            let Some(cur) = store.get_player(uid) else {
                return Some(Err("player_missing".to_string()));
            };
            if cur.is_recharge == 0 && cur.vip == 0 {
                return Some(Err("recharge_required".to_string()));
            }
            let mut claimed = read_u32_set_from_va(cur, "recharge_sign_rewards");
            if claimed.contains(&reward_id) {
                return Some(Err("already_claimed".to_string()));
            }
            let res = store.mutate_player(uid, now_ts, |p| {
                p.grant_currency(CurrencyType::Gold, 300, now_ts);
                claimed.push(reward_id);
                write_u32_set_to_va(p, "recharge_sign_rewards", &claimed);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay(store, gameplay, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: vec![build_user_update_env(updated, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        _ => None,
    }
}
