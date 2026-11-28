//! Declarative persistent `PlayerProfile` + `GameplayEngine` RPC router covering character
//! progression, 42-sub-bag inventory mutations, hero roster growth, 6-slot squad/grid/partner/arming,
//! equipment forge, castle, raffle, PvE copy, arena, and recharge sign rewards.

use std::collections::BTreeMap;

use gserver_gameplay::battle_copy::{ArenaEngine, CopyCampaignEngine};
use gserver_gameplay::engine::GameplayEngine;
use gserver_gameplay::player::PlayerState as GameplayPlayer;
use gserver_gameplay::protocol::{DtoValue, PushPacket};
use gserver_player::{
    BagItem, BagType, CurrencyType, EquipSlotType, JsonValue as PlayerJson, PlayerError,
    PlayerProfile, PlayerStore, StarterItemSeed, UseItemEffect, UseItemOutcome,
};
use gserver_protocol::{RpcRequest, S2cEnvelope, Value};

use crate::bridge::{
    dto_push_at, dto_to_amf3, player_json_to_amf3, profile_to_gameplay_state,
    sync_gameplay_back_to_profile, u64_to_amf3,
};

pub struct PlayerRpcSuccess {
    pub ret: Value,
    pub caller_pushes: Vec<S2cEnvelope>,
    pub peer_pushes: Vec<(u64, S2cEnvelope)>,
    pub refresh_roles_pid: Option<String>,
}

impl PlayerRpcSuccess {
    pub fn ret_only(ret: Value) -> Self {
        Self { ret, caller_pushes: Vec::new(), peer_pushes: Vec::new(), refresh_roles_pid: None }
    }
}

fn ok_map<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let mut entries = Vec::with_capacity(N + 1);
    entries.push(("ret", Value::String("ok".into())));
    entries.extend(fields);
    Value::map(entries)
}

/// Positional AMF3 argument cursor providing clamped, ranged, and required extractors.
#[derive(Clone, Copy, Debug)]
pub struct ArgReader<'a> {
    args: &'a [Value],
}

impl<'a> ArgReader<'a> {
    pub fn new(args: &'a [Value]) -> Self { Self { args } }
    pub fn len(&self) -> usize { self.args.len() }
    pub fn is_empty(&self) -> bool { self.args.is_empty() }
    pub fn get(&self, idx: usize) -> Option<&'a Value> { self.args.get(idx) }
    pub fn raw(&self) -> &'a [Value] { self.args }
    pub fn opt_i64(&self, idx: usize, default: i64) -> i64 {
        self.get(idx).and_then(Value::as_i64).unwrap_or(default)
    }
    pub fn req_i64(&self, idx: usize, err: &str) -> Result<i64, String> {
        self.get(idx).and_then(Value::as_i64).ok_or_else(|| err.to_string())
    }
    pub fn pos_u64(&self, idx: usize, err: &str) -> Result<u64, String> {
        let v = self.opt_i64(idx, 0);
        (v > 0).then_some(v as u64).ok_or_else(|| err.to_string())
    }
    pub fn pos_u32(&self, idx: usize, err: &str) -> Result<u32, String> {
        self.pos_u64(idx, err).map(|v| v as u32)
    }
    pub fn clamped_u64(&self, idx: usize, default: i64, min: i64, max: i64) -> u64 {
        self.opt_i64(idx, default).clamp(min, max) as u64
    }
    pub fn clamped_u32(&self, idx: usize, default: i64, min: i64, max: i64) -> u32 {
        self.opt_i64(idx, default).clamp(min, max) as u32
    }
    pub fn min_u32(&self, idx: usize, default: i64, min: i64) -> u32 {
        self.opt_i64(idx, default).max(min) as u32
    }
    pub fn in_range_u8(&self, idx: usize, lo: i64, hi: i64, err: &str) -> Result<u8, String> {
        let v = self.opt_i64(idx, 0);
        (lo..=hi).contains(&v).then_some(v as u8).ok_or_else(|| err.to_string())
    }
    pub fn in_range_u32(&self, idx: usize, lo: i64, hi: i64, err: &str) -> Result<u32, String> {
        let v = self.opt_i64(idx, 0);
        (lo..=hi).contains(&v).then_some(v as u32).ok_or_else(|| err.to_string())
    }
    pub fn trimmed_string(&self, idx: usize) -> String {
        self.get(idx).and_then(Value::as_str).unwrap_or("").trim().to_string()
    }
    pub fn equip_target(&self) -> Result<(u8, EquipSlotType), MutationError> {
        let squad_slot = self.in_range_u8(0, 1, 6, "invalid_equip_args")?;
        let es_raw = self.in_range_u8(1, 1, 9, "invalid_equip_args")?;
        Ok((squad_slot, EquipSlotType::from_u8(es_raw)?))
    }
}

/// Typed argument decoder built on [`ArgReader`].
pub trait FromAmf3Args<'a>: Sized {
    fn from_amf3_args(reader: ArgReader<'a>) -> Result<Self, String>;
}

impl<'a> FromAmf3Args<'a> for () {
    fn from_amf3_args(_: ArgReader<'a>) -> Result<Self, String> { Ok(()) }
}
impl<'a> FromAmf3Args<'a> for ArgReader<'a> {
    fn from_amf3_args(reader: ArgReader<'a>) -> Result<Self, String> { Ok(reader) }
}

pub struct ClampedU64<const DEF: i64, const MAX: i64>(pub u64);
impl<'a, const DEF: i64, const MAX: i64> FromAmf3Args<'a> for ClampedU64<DEF, MAX> {
    fn from_amf3_args(r: ArgReader<'a>) -> Result<Self, String> { Ok(Self(r.clamped_u64(0, DEF, 1, MAX))) }
}

pub struct ClampedU32<const DEF: i64, const MAX: i64>(pub u32);
impl<'a, const DEF: i64, const MAX: i64> FromAmf3Args<'a> for ClampedU32<DEF, MAX> {
    fn from_amf3_args(r: ArgReader<'a>) -> Result<Self, String> { Ok(Self(r.clamped_u32(0, DEF, 1, MAX))) }
}

pub struct MinU32<const DEF: i64>(pub u32);
impl<'a, const DEF: i64> FromAmf3Args<'a> for MinU32<DEF> {
    fn from_amf3_args(r: ArgReader<'a>) -> Result<Self, String> { Ok(Self(r.min_u32(0, DEF, 1))) }
}

macro_rules! amf3_newtype_arg {
    ($($vis:vis struct $name:ident($inner:ty) => |$r:ident| $expr:expr;)*) => {$(
        $vis struct $name(pub $inner);
        impl<'a> FromAmf3Args<'a> for $name {
            fn from_amf3_args($r: ArgReader<'a>) -> Result<Self, String> { ($expr).map(Self) }
        }
    )*};
}

amf3_newtype_arg! {
    pub struct HeroHidArg(u64) => |r| r.pos_u64(0, "invalid_hid");
    pub struct SquadSlotArg(u8) => |r| r.in_range_u8(0, 1, 6, "invalid_formation_slot");
    pub struct RenameArg(String) => |r| {
        let s = r.trimmed_string(0);
        (!s.is_empty() && s.chars().count() <= 15).then_some(s).ok_or_else(|| "invalid_uname".to_string())
    };
    pub struct ComposeArgs((u32, u32)) => |r| Ok((r.pos_u32(0, "invalid_fragment_id")?, r.clamped_u32(1, 1, 1, 10)));
}

/// Unified mutation error propagating either domain [`PlayerError`]s or explicit RPC error codes.
pub enum MutationError {
    Rpc(String),
    Player(PlayerError),
}
impl From<PlayerError> for MutationError { fn from(e: PlayerError) -> Self { Self::Player(e) } }
impl From<&'static str> for MutationError { fn from(s: &'static str) -> Self { Self::Rpc(s.to_string()) } }
impl From<String> for MutationError { fn from(s: String) -> Self { Self::Rpc(s) } }

/// Side-effect descriptor returned by a `PlayerProfile` mutation inside [`execute_player_mutation`].
pub struct MutationEffects {
    pub ret: Value,
    pub resync: bool,
    pub push_user: bool,
    pub bag_upserted: Vec<BagItem>,
    pub bag_removed: Vec<u32>,
    pub new_heroes: Vec<u64>,
    pub refresh_roles: bool,
    pub sync_town_appearance: bool,
}

impl MutationEffects {
    pub fn ok(ret: Value) -> Self {
        Self {
            ret, resync: true, push_user: true,
            bag_upserted: Vec::new(), bag_removed: Vec::new(), new_heroes: Vec::new(),
            refresh_roles: false, sync_town_appearance: false,
        }
    }
    pub fn ok_str() -> Self { Self::ok(Value::String("ok".into())) }
    pub fn quiet_ok() -> Self { Self { resync: false, push_user: false, ..Self::ok_str() } }
    pub fn without_user_push(mut self) -> Self { self.push_user = false; self }
    pub fn with_refresh_roles(mut self) -> Self { self.refresh_roles = true; self }
    pub fn with_town_appearance(mut self) -> Self { self.sync_town_appearance = true; self }
    pub fn with_bag_added(mut self, items: impl IntoIterator<Item = BagItem>) -> Self {
        self.bag_upserted.extend(items);
        self
    }
    pub fn record_bag_slot_delta(&mut self, p: &PlayerProfile, gid: u32, remaining: u32) {
        if remaining == 0 {
            self.bag_removed.push(gid);
        } else if let Some(it) = p.inventory.get_by_gid(gid) {
            self.bag_upserted.push(it.clone());
        }
    }
    pub fn record_bag_template_deltas(&mut self, p: &PlayerProfile, affected: &[(u32, u32)]) {
        for &(gid, left) in affected {
            self.record_bag_slot_delta(p, gid, left);
        }
    }
    pub fn record_use_outcome(&mut self, p: &PlayerProfile, gid: u32, out: &UseItemOutcome) {
        self.bag_upserted.extend(out.granted_items.iter().cloned());
        self.record_bag_slot_delta(p, gid, out.remaining_in_grid);
        self.new_heroes.extend(out.summoned_hids.iter().copied());
    }
    pub fn record_equip_swap(&mut self, equipped_gid: u32, returned: Option<BagItem>) {
        match returned {
            Some(it) => {
                if it.gid != equipped_gid { self.bag_removed.push(equipped_gid); }
                self.bag_upserted.push(it);
            }
            None => self.bag_removed.push(equipped_gid),
        }
    }
}

fn map_player_err(err: PlayerError) -> String {
    match err {
        PlayerError::InsufficientCurrency { currency, .. } => format!("insufficient_currency:{currency}"),
        PlayerError::InsufficientItemCount { .. } => "insufficient_item_count".to_string(),
        PlayerError::ItemNotFound(_) => "item_not_found".to_string(),
        PlayerError::BagFull { bag_type, .. } => format!("bag_full:{bag_type}"),
        PlayerError::HeroNotFound(_) => "hero_not_found".to_string(),
        PlayerError::HeroBagFull { .. } => "hero_bag_full".to_string(),
        PlayerError::ProtagonistCannotBeRemoved { .. } => "protagonist_cannot_be_removed".to_string(),
        PlayerError::DuplicateHeroTemplateInSquad { .. } => "duplicate_htid_in_squad".to_string(),
        PlayerError::InvalidFormationSlot(_) => "invalid_formation_slot".to_string(),
        PlayerError::EquipTypeMismatch { .. } => "equip_type_mismatch".to_string(),
        PlayerError::InvalidParameter(m) if m.contains("protagonist level") => "protagonist_cannot_level_directly".into(),
        PlayerError::InvalidParameter(m) if m.contains("locked hero") => "locked_hero_cannot_sacrifice".into(),
        PlayerError::InvalidParameter(m) if m.contains("deployed hero") => "deployed_hero_cannot_sacrifice".into(),
        PlayerError::InvalidParameter(m) => format!("invalid_parameter:{m}"),
        other => format!("player_error:{other}"),
    }
}

fn build_user_update_env(p: &PlayerProfile, now_i64: i64) -> S2cEnvelope {
    let c = |ct| u64_to_amf3(p.currency(ct));
    let payload = Value::map([
        ("uname", Value::String(p.uname.clone())),
        ("level", Value::Int(p.level as i32)),
        ("exp", u64_to_amf3(p.exp)),
        ("vip", Value::Int(p.vip as i32)),
        ("vip_exp", u64_to_amf3(p.vip_exp)),
        ("gold", c(CurrencyType::Gold)),
        ("gold_bind", c(CurrencyType::GoldBind)),
        ("copper", Value::String(p.currency(CurrencyType::Copper).to_string())),
        ("hero_soul", c(CurrencyType::HeroSoul)),
        ("prestige", c(CurrencyType::Prestige)),
        ("execution", c(CurrencyType::Execution)),
        ("execution_time", u64_to_amf3(p.execution_time)),
        ("energy", c(CurrencyType::Energy)),
        ("energy_time", u64_to_amf3(p.energy_time)),
        ("fight_force", u64_to_amf3(p.fc)),
        ("fight_force_max", u64_to_amf3(p.fight_force_max)),
        ("hero_bag_buy_times", Value::Int(p.hero_bag_buy_times as i32)),
    ]);
    S2cEnvelope::push("user.updateUser", payload).with_time(now_i64)
}

fn build_bag_update_env(upserted: &[BagItem], removed_gids: &[u32], now_i64: i64) -> Option<S2cEnvelope> {
    if upserted.is_empty() && removed_gids.is_empty() {
        return None;
    }
    let mut entries: Vec<(String, Value)> = Vec::with_capacity(upserted.len() + removed_gids.len());
    entries.extend(removed_gids.iter().map(|gid| (gid.to_string(), Value::empty_array())));
    entries.extend(upserted.iter().map(|it| (it.gid.to_string(), player_json_to_amf3(&it.to_wire_json()))));
    Some(S2cEnvelope::push("bag.update", Value::dense_array(vec![Value::map(entries)])).with_time(now_i64))
}

fn build_new_heroes_env(profile: &PlayerProfile, hids: &[u64], now_i64: i64) -> Option<S2cEnvelope> {
    let entries: Vec<(String, Value)> = hids
        .iter()
        .filter_map(|&hid| Some((hid.to_string(), player_json_to_amf3(&profile.heroes.get(&hid)?.to_vnsg2_hero_info()))))
        .collect();
    (!entries.is_empty()).then(|| S2cEnvelope::push("re.hero.new", Value::map(entries)).with_time(now_i64))
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

/// Runs a transactional `PlayerStore` WAL + snapshot mutation, resynchronizes `GameplayEngine`,
/// and emits `user.updateUser`, `bag.update`, `re.hero.new`, and town appearance peer pushes.
pub fn execute_player_mutation<F>(
    store: &mut PlayerStore,
    gameplay: &mut GameplayEngine,
    uid: u64,
    now_ts: u64,
    mutate_fn: F,
) -> Result<PlayerRpcSuccess, String>
where
    F: FnOnce(&mut PlayerProfile) -> Result<MutationEffects, MutationError>,
{
    if store.get_player(uid).is_none() {
        return Err("player_missing".to_string());
    }
    let now_i64 = now_ts as i64;
    let mut custom_rpc_err: Option<String> = None;
    let effects = store
        .mutate_player(uid, now_ts, |p| match mutate_fn(p) {
            Ok(eff) => Ok(eff),
            Err(MutationError::Player(pe)) => Err(pe),
            Err(MutationError::Rpc(code)) => {
                custom_rpc_err = Some(code);
                Err(PlayerError::CharacterNotFound(uid))
            }
        })
        .map_err(|e| custom_rpc_err.unwrap_or_else(|| map_player_err(e)))?;

    if effects.resync {
        resync_gameplay(store, gameplay, uid);
    }
    let updated = store.get_player(uid).expect("validated above");

    let mut caller_pushes = Vec::new();
    if effects.push_user {
        caller_pushes.push(build_user_update_env(updated, now_i64));
    }
    caller_pushes.extend(build_bag_update_env(&effects.bag_upserted, &effects.bag_removed, now_i64));
    caller_pushes.extend(build_new_heroes_env(updated, &effects.new_heroes, now_i64));

    let mut peer_pushes = Vec::new();
    if effects.sync_town_appearance {
        if let Some(gp) = gameplay.players.get(&uid) {
            peer_pushes.extend(
                gameplay.town.sync_player_appearance(gp, now_ts).into_iter().map(|(t, pkt)| (t, dto_push_at(pkt, now_i64))),
            );
        }
    }

    Ok(PlayerRpcSuccess {
        ret: effects.ret,
        caller_pushes,
        peer_pushes,
        refresh_roles_pid: effects.refresh_roles.then(|| updated.pid.clone()),
    })
}

/// Runs a `GameplayEngine` mutation (`copy.*`, `arena.*`), persists state back to `PlayerStore` WAL,
/// and converts `DtoValue` responses + `PushPacket`s into wire `S2cEnvelope`s.
pub fn execute_gameplay_rpc<F>(
    store: &mut PlayerStore,
    gameplay: &mut GameplayEngine,
    uid: u64,
    now_ts: u64,
    op: F,
) -> Result<PlayerRpcSuccess, String>
where
    F: FnOnce(&mut GameplayPlayer, &mut CopyCampaignEngine, &mut ArenaEngine)
        -> Result<(DtoValue, Vec<PushPacket>, Vec<(u64, PushPacket)>), &'static str>,
{
    let now_i64 = now_ts as i64;
    let GameplayEngine { ref mut players, ref mut copy, ref mut arena, .. } = *gameplay;
    let gp = players.get_mut(&uid).ok_or_else(|| "user_not_found".to_string())?;
    let (ret_dto, caller_pkts, peer_pkts) = op(gp, copy, arena).map_err(str::to_string)?;
    let gp_clone = gp.clone();
    let _ = store.mutate_player(uid, now_ts, |p| {
        sync_gameplay_back_to_profile(&gp_clone, p, now_ts);
        Ok(())
    });

    Ok(PlayerRpcSuccess {
        ret: dto_to_amf3(&ret_dto, true),
        caller_pushes: caller_pkts.into_iter().map(|cp| dto_push_at(cp, now_i64)).collect(),
        peer_pushes: peer_pkts.into_iter().map(|(tuid, pkt)| (tuid, dto_push_at(pkt, now_i64))).collect(),
        refresh_roles_pid: None,
    })
}

fn infer_item_effect(tid: u32) -> UseItemEffect {
    match tid {
        22_102 => UseItemEffect::GrantCurrency { currency: CurrencyType::Execution, amount_per_item: 25 },
        22_202 => UseItemEffect::GrantCurrency { currency: CurrencyType::Energy, amount_per_item: 10 },
        20_001..=25_000 => UseItemEffect::GrantCurrency { currency: CurrencyType::Execution, amount_per_item: 20 },
        25_001..=30_000 => UseItemEffect::AddPlayerExp { exp_per_item: 250 },
        30_001..=40_000 => UseItemEffect::GrantItems {
            items_per_use: vec![
                StarterItemSeed { item_template_id: 16_012, count: 2 },
                StarterItemSeed { item_template_id: 10_001, count: 5 },
            ],
        },
        1_200_001..=1_300_000 => UseItemEffect::AddPlayerExp { exp_per_item: 500 },
        _ => UseItemEffect::GrantCurrency { currency: CurrencyType::Copper, amount_per_item: 10_000 },
    }
}

fn parse_squad_hid_and_slot(args: &[Value], allow_zero_hid: bool) -> Result<(u64, u8), String> {
    let r = ArgReader::new(args);
    let (a, b) = (r.req_i64(0, "missing_first_arg")?, r.req_i64(1, "missing_second_arg")?);
    if a < 0 || b < 0 {
        return Err("negative_formation_arg".to_string());
    }
    let (ua, ub) = (a as u64, b as u64);
    if (1..=6).contains(&ub) && (ua > 6 || (allow_zero_hid && ua == 0)) {
        Ok((ua, ub as u8))
    } else if (1..=6).contains(&ua) && (ub > 6 || (allow_zero_hid && ub == 0)) {
        Ok((ub, ua as u8))
    } else {
        Err("invalid_formation_slot".to_string())
    }
}

fn resolve_bag_gid(profile: &PlayerProfile, gid_or_item_id: u64) -> Option<u32> {
    u32::try_from(gid_or_item_id)
        .ok()
        .filter(|&g| profile.inventory.get_by_gid(g).is_some())
        .or_else(|| profile.inventory.get_by_item_id(gid_or_item_id).map(|(_, it)| it.gid))
}

fn parse_bag_type_arg(val: Option<&Value>) -> BagType {
    if let Some(s) = val.and_then(Value::as_str) {
        return BagType::from_key(s).unwrap_or(BagType::Item);
    }
    match val.and_then(Value::as_i64) {
        Some(1) => BagType::Arm,
        Some(3) => BagType::Treasure,
        Some(4) => BagType::Soul,
        _ => BagType::Item,
    }
}

fn read_u32_set_from_va(profile: &PlayerProfile, key: &str) -> Vec<u32> {
    match profile.va_config.get(key) {
        Some(PlayerJson::Array(arr)) => arr.iter().filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok())).collect(),
        _ => Vec::new(),
    }
}

fn claim_va_set_entry(p: &mut PlayerProfile, key: &str, entry_id: u32, dup_err: &'static str) -> Result<(), MutationError> {
    let mut set = read_u32_set_from_va(p, key);
    if set.contains(&entry_id) {
        return Err(dup_err.into());
    }
    set.push(entry_id);
    p.set_user_config(key, PlayerJson::Array(set.into_iter().map(PlayerJson::from).collect()))?;
    Ok(())
}

fn bump_va_u32(p: &mut PlayerProfile, key: &str, default: u32, add: u32) -> Result<u32, PlayerError> {
    let cur = p.va_config.get(key).and_then(PlayerJson::as_u64).unwrap_or(u64::from(default)) as u32;
    let next = cur.saturating_add(add);
    p.set_user_config(key, PlayerJson::from(next))?;
    Ok(next)
}

fn bump_reinforce_level(item: &mut BagItem, levels: u32, max_lv: u32) -> u32 {
    let cur = item.va_item_text.get("reinforce_level").and_then(PlayerJson::as_u64).unwrap_or(1) as u32;
    let next = cur.saturating_add(levels).min(max_lv);
    item.va_item_text.insert("reinforce_level".into(), PlayerJson::from(next));
    next
}

fn pos_kv(k: &str, v: &Value) -> Option<(u8, u64)> {
    v.as_i64().filter(|&n| n > 0).map(|n| (k.parse::<u8>().unwrap_or(0), n as u64))
}

fn parse_positive_indexed_map(val: &Value) -> Result<Vec<(u8, u64)>, String> {
    match val {
        Value::Array { dense, assoc } => Ok(dense
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_i64().filter(|&n| n > 0).map(|n| ((i + 1) as u8, n as u64)))
            .chain(assoc.iter().filter_map(|(k, v)| pos_kv(k, v)))
            .collect()),
        Value::Object(obj) => Ok(obj.iter().filter_map(|(k, v)| pos_kv(k, v)).collect()),
        _ => Err("invalid_table_arg".to_string()),
    }
}

fn parse_sell_entries(r: ArgReader<'_>) -> Result<Vec<(u32, u32)>, String> {
    let mut sales = Vec::new();
    if let Some(Value::Array { dense, .. }) = r.get(0) {
        for entry in dense {
            if let Value::Array { dense: row, .. } = entry {
                let gid = row.first().and_then(Value::as_i64).unwrap_or(0);
                let num = row.get(2).or_else(|| row.get(1)).and_then(Value::as_i64).unwrap_or(1);
                if gid <= 0 || num <= 0 {
                    return Err("invalid_sell_entry".to_string());
                }
                sales.push((gid as u32, num as u32));
            }
        }
    } else {
        let gid = r.opt_i64(0, 0);
        let num = r.get(2).or_else(|| r.get(1)).and_then(Value::as_i64).unwrap_or(1);
        if gid > 0 && num > 0 {
            sales.push((gid as u32, num as u32));
        }
    }
    (!sales.is_empty()).then_some(sales).ok_or_else(|| "empty_sell_list".to_string())
}

fn handle_forge_upgrade(
    p: &mut PlayerProfile,
    target_id: u64,
    levels: u32,
    is_auto: bool,
    now_ts: u64,
) -> Result<MutationEffects, MutationError> {
    let cost_copper = u64::from(levels) * 1_500;
    p.consume_currency(CurrencyType::Copper, cost_copper, now_ts)?;
    let max_lv = (p.level * 2).max(20);
    let mut new_reinforce_lv = 1u32;
    let mut bag_upserted = Vec::new();
    let mut found_equipped = false;

    if (1..=6).contains(&target_id) && is_auto {
        if let Some(slot_map) = p.formation.arming.get_mut(&(target_id as u8)) {
            for item in slot_map.values_mut() {
                new_reinforce_lv = bump_reinforce_level(item, levels, max_lv);
                found_equipped = true;
            }
        }
    }
    if !found_equipped {
        if let Some(item) = p.formation.arming.values_mut().flat_map(|m| m.values_mut()).find(|it| it.item_id == target_id) {
            new_reinforce_lv = bump_reinforce_level(item, levels, max_lv);
            found_equipped = true;
        }
    }
    if !found_equipped {
        let gid = resolve_bag_gid(p, target_id)
            .ok_or_else(|| PlayerError::ItemNotFound(format!("forge target {target_id} not found")))?;
        let bt = BagType::from_gid(gid).ok_or_else(|| PlayerError::ItemNotFound("bad gid".into()))?;
        let item = p
            .inventory
            .bags
            .get_mut(&bt)
            .and_then(|sub| sub.get_mut(&gid))
            .ok_or_else(|| PlayerError::ItemNotFound("missing item".into()))?;
        new_reinforce_lv = bump_reinforce_level(item, levels, max_lv);
        bag_upserted.push(item.clone());
    }

    p.recalculate_fight_force();
    Ok(MutationEffects::ok(ok_map([
        ("reinforce_level", Value::Int(new_reinforce_lv as i32)),
        ("cost_copper", u64_to_amf3(cost_copper)),
        ("fight_force", u64_to_amf3(p.fc)),
    ]))
    .with_bag_added(bag_upserted))
}

macro_rules! player_rpc_routes {
    (
        ($store:ident, $gameplay:ident, $uid:ident, $req:ident, $now_ts:ident);
        $( $($pat:literal)|+ => $kind:ident $spec:tt => $body:block ),* $(,)?
    ) => {{
        let reader = ArgReader::new(&$req.args);
        match $req.method.as_str() {
            $( $($pat)|+ => Some(player_rpc_routes!(@arm $kind, $store, $gameplay, $uid, $req, $now_ts, reader, $spec, $body)), )*
            _ => None,
        }
    }};
    (@arm const_val, $s:ident, $gp:ident, $u:ident, $req:ident, $now:ident, $r:ident, (), $body:block) => {
        Ok(PlayerRpcSuccess::ret_only($body))
    };
    (@arm query, $s:ident, $gp:ident, $u:ident, $req:ident, $now:ident, $r:ident, ($p:ident), $body:block) => {
        $s.get_player($u).ok_or_else(|| "player_missing".to_string()).map(|$p| PlayerRpcSuccess::ret_only($body))
    };
    (@arm store_query, $s:ident, $gp:ident, $u:ident, $req:ident, $now:ident, $r:ident, ($apat:pat, $aty:ty), $body:block) => {
        <$aty as FromAmf3Args>::from_amf3_args($r).and_then(|$apat| $body.map(PlayerRpcSuccess::ret_only))
    };
    (@arm mutate, $s:ident, $gp:ident, $u:ident, $req:ident, $now:ident, $r:ident, ($p:ident, $apat:pat, $aty:ty), $body:block) => {
        <$aty as FromAmf3Args>::from_amf3_args($r)
            .and_then(|$apat| execute_player_mutation($s, $gp, $u, $now, |$p: &mut PlayerProfile| $body))
    };
    (@arm mutate_args, $s:ident, $gp:ident, $u:ident, $req:ident, $now:ident, $r:ident, ($p:ident, $r_arg:ident), $body:block) => {
        <ArgReader<'_> as FromAmf3Args>::from_amf3_args($r)
            .and_then(|$r_arg| execute_player_mutation($s, $gp, $u, $now, |$p: &mut PlayerProfile| $body))
    };
    (@arm gameplay, $s:ident, $gp:ident, $u:ident, $req:ident, $now:ident, $r:ident, ($player:ident, $copy:ident, $arena:ident, $r_arg:ident), $body:block) => {
        <ArgReader<'_> as FromAmf3Args>::from_amf3_args($r)
            .and_then(|$r_arg| execute_gameplay_rpc($s, $gp, $u, $now, |$player, $copy, $arena| $body))
    };
    (@arm custom, $s:ident, $gp:ident, $u:ident, $req:ident, $now:ident, $r:ident, ($apat:pat, $aty:ty), $body:block) => {
        <$aty as FromAmf3Args>::from_amf3_args($r).and_then(|$apat| $body)
    };
}

pub fn try_handle_player_rpc(
    store: &mut PlayerStore,
    gameplay: &mut GameplayEngine,
    uid: u64,
    req: &RpcRequest,
    now_ts: u64,
) -> Option<Result<PlayerRpcSuccess, String>> {
    player_rpc_routes! {
        (store, gameplay, uid, req, now_ts);

        // Domain 3: User Progression, Rename, Stamina/Copper & VIP Gifts
        "user.changeUname" => custom(RenameArg(raw_name), RenameArg) => {
            let cur = store.get_player(uid).ok_or_else(|| "player_missing".to_string())?;
            if cur.uname == raw_name { return Err("uname_unchanged".to_string()); }
            if gameplay.players.values().any(|p| p.uid != uid && p.uname == raw_name) {
                return Err("uname_used".to_string());
            }
            execute_player_mutation(store, gameplay, uid, now_ts, |p| {
                if p.uname_change_num > 0 { p.consume_currency(CurrencyType::Gold, 100, now_ts)?; }
                p.uname = raw_name;
                p.uname_change_num = p.uname_change_num.saturating_add(1);
                Ok(MutationEffects::ok_str().with_refresh_roles().with_town_appearance())
            })
        },
        "user.buyCopper" => mutate(p, ClampedU64(count), ClampedU64<1, 50>) => {
            let copper_gain = 100_000 * count;
            p.consume_currency(CurrencyType::Gold, 20 * count, now_ts)?;
            p.grant_currency(CurrencyType::Copper, copper_gain, now_ts);
            p.buy_copper_accum = p.buy_copper_accum.saturating_add(count as u32);
            p.buy_copper_time = now_ts;
            Ok(MutationEffects::ok(ok_map([("copper", u64_to_amf3(copper_gain)), ("crit", Value::Int(1))])))
        },
        "user.buyExecution" => mutate(p, ClampedU64(count), ClampedU64<1, 20>) => {
            p.consume_currency(CurrencyType::Gold, 20 * count, now_ts)?;
            p.grant_currency(CurrencyType::Execution, 25 * count, now_ts);
            p.buy_execution_accum = p.buy_execution_accum.saturating_add(count as u32);
            p.buy_execution_time = now_ts;
            Ok(MutationEffects::ok_str())
        },
        "user.addEnergy" => mutate(p, ClampedU64(count), ClampedU64<1, 20>) => {
            p.consume_currency(CurrencyType::Gold, 20 * count, now_ts)?;
            p.grant_currency(CurrencyType::Energy, 10 * count, now_ts);
            Ok(MutationEffects::ok_str())
        },
        "user.openHeroBag" => mutate(p, ClampedU32(times), ClampedU32<1, 10>) => {
            p.consume_currency(CurrencyType::Gold, u64::from(times) * 50, now_ts)?;
            p.hero_bag_buy_times = p.hero_bag_buy_times.saturating_add(times);
            Ok(MutationEffects::ok_str())
        },
        "user.getVipGiftInfo" => query(profile) => {
            let bought = read_u32_set_from_va(profile, "vip_gift_bought");
            Value::dense_array((1..=12u32).map(|lv| Value::Int(i32::from(bought.contains(&lv)))).collect())
        },
        "user.buyVipGift" => mutate_args(p, r) => {
            let vip_lv = r.in_range_u32(0, 1, 15, "invalid_vip_level")?;
            if p.vip < vip_lv { return Err("vip_too_low".into()); }
            claim_va_set_entry(p, "vip_gift_bought", vip_lv, "vip_gift_already_bought")?;
            p.consume_currency(CurrencyType::Gold, u64::from(vip_lv) * 50, now_ts)?;
            Ok(MutationEffects::ok_str().with_bag_added(p.add_item(16_012, vip_lv * 5, now_ts)?))
        },

        // Domain 4: Inventory / 42 Sub-Bags / Item Use / Sell / Compose
        "bag.openGridByGold" => mutate_args(p, r) => {
            let (add_grids, bag_type) = (r.clamped_u32(0, 5, 1, 100), parse_bag_type_arg(r.get(1)));
            p.consume_currency(CurrencyType::Gold, u64::from(add_grids) * 10, now_ts)?;
            let cur_max = p.inventory.grid_max_num.get(&bag_type).copied().unwrap_or_else(|| bag_type.default_max_grids());
            let new_max = cur_max.saturating_add(add_grids);
            p.inventory.grid_max_num.insert(bag_type, new_max);
            let opens = p.inventory.grid_open_times.get(&bag_type).copied().unwrap_or(0);
            p.inventory.grid_open_times.insert(bag_type, opens.saturating_add(1));
            Ok(MutationEffects::ok(ok_map([
                ("bag_type", Value::String(bag_type.vnsg2_key().into())),
                ("grid_max", Value::Int(new_max as i32)),
            ])))
        },
        "bag.useItem" | "exchange.useItemInBag" => mutate_args(p, r) => {
            let gid = r.pos_u32(0, "invalid_gid")?;
            let existing = p.inventory.get_by_gid(gid).ok_or_else(|| MutationError::from("item_not_found"))?;
            let count_i64 = if r.len() >= 3 {
                let passed_id = r.opt_i64(1, 0);
                if passed_id > 0 && passed_id as u64 != existing.item_id { return Err("item_id_mismatch".into()); }
                r.opt_i64(2, 1)
            } else {
                r.opt_i64(1, 1)
            };
            if count_i64 <= 0 { return Err("invalid_item_count".into()); }
            let out = p.use_item(gid, count_i64 as u32, infer_item_effect(existing.item_template_id), now_ts)?;
            let mut eff = MutationEffects::ok(ok_map([
                ("remaining", Value::Int(out.remaining_in_grid as i32)),
                ("daily_used", Value::Int(out.daily_used_total as i32)),
            ]))
            .with_refresh_roles();
            eff.record_use_outcome(p, gid, &out);
            Ok(eff)
        },
        "exchange.useItemInBagByTpl" => mutate_args(p, r) => {
            let (tid_i64, count_i64) = (r.opt_i64(0, 0), r.opt_i64(1, 1));
            if tid_i64 <= 0 || count_i64 <= 0 { return Err("invalid_item_args".into()); }
            let tid = tid_i64 as u32;
            let found_gid = p.inventory.bags.values().flat_map(|sub| sub.values())
                .find(|it| it.item_template_id == tid)
                .map(|it| it.gid)
                .ok_or_else(|| MutationError::from("item_not_found"))?;
            let out = p.use_item(found_gid, count_i64 as u32, infer_item_effect(tid), now_ts)?;
            let mut eff = MutationEffects::ok(ok_map([("remaining", Value::Int(out.remaining_in_grid as i32))]));
            eff.bag_upserted.extend(out.granted_items.iter().cloned());
            eff.record_bag_slot_delta(p, found_gid, out.remaining_in_grid);
            Ok(eff)
        },
        "bag.sellItems" | "bag.sellItem" => mutate_args(p, r) => {
            let sales = parse_sell_entries(r)?;
            let mut total_copper = 0u64;
            let mut eff = MutationEffects::ok_str();
            for (gid, num) in sales {
                let snap = p.remove_item_by_gid(gid, num)?;
                total_copper = total_copper.saturating_add(u64::from(num) * 1_000);
                if snap.item_num == 0 { eff.bag_removed.push(gid); } else { eff.bag_upserted.push(snap); }
            }
            p.grant_currency(CurrencyType::Copper, total_copper, now_ts);
            eff.ret = ok_map([("copper", u64_to_amf3(total_copper))]);
            Ok(eff)
        },
        "bag.takeTmpBag" => const_val() => { Value::String("ok".into()) },
        "exchange.composeHero" => mutate(p, ComposeArgs((raw_id, count)), ComposeArgs) => {
            let frag_tid = p.inventory.get_by_gid(raw_id).map_or(raw_id, |it| it.item_template_id);
            if !(50_001..=60_000).contains(&frag_tid) { return Err("not_hero_fragment".into()); }
            if (p.heroes.len() as u32).saturating_add(count) > p.max_hero_capacity() {
                return Err(PlayerError::HeroBagFull { max_heroes: p.max_hero_capacity() }.into());
            }
            let summoned_htid = 300 + ((frag_tid - 50_000) % 200).max(1);
            let affected = p.remove_item_by_template(frag_tid, 30 * count)?;
            let summoned_hids: Vec<u64> = (0..count).map(|_| p.summon_hero(summoned_htid, 1)).collect::<Result<_, _>>()?;
            let first_hid = summoned_hids.first().copied().unwrap_or(0);
            let mut eff = MutationEffects::ok(ok_map([
                ("hid", u64_to_amf3(first_hid)),
                ("htid", Value::Int(summoned_htid as i32)),
                ("hids", Value::dense_array(summoned_hids.iter().copied().map(u64_to_amf3).collect())),
            ]))
            .without_user_push();
            eff.record_bag_template_deltas(p, &affected);
            eff.new_heroes = summoned_hids;
            Ok(eff)
        },
        "exchange.composeArm" => mutate(p, ComposeArgs((raw_id, count)), ComposeArgs) => {
            let frag_tid = p.inventory.get_by_gid(raw_id).map_or(raw_id, |it| it.item_template_id);
            let (req_per_craft, out_tid) = if (3_100_001..=3_200_000).contains(&frag_tid) {
                (20u32, 101_102u32)
            } else if (400_000_001..=500_000_000).contains(&frag_tid) {
                (10u32, 180_102u32)
            } else {
                return Err("not_equip_fragment".into());
            };
            let affected = p.remove_item_by_template(frag_tid, req_per_craft * count)?;
            let crafted = p.add_item(out_tid, count, now_ts)?;
            let first_gid = crafted.first().map_or(0, |i| i.gid);
            let mut eff = MutationEffects::ok(ok_map([
                ("item_template_id", Value::Int(out_tid as i32)),
                ("gid", Value::Int(first_gid as i32)),
            ]))
            .without_user_push()
            .with_bag_added(crafted);
            eff.record_bag_template_deltas(p, &affected);
            Ok(eff)
        },

        // Domain 5: Hero Roster, Level-Up, Skill, Evolve, Breakthrough & Lock
        "hero.getAllHeroes" => query(profile) => {
            let entries: Vec<(String, Value)> = profile.heroes.iter()
                .map(|(hid, hero)| (hid.to_string(), player_json_to_amf3(&hero.to_vnsg2_hero_info())))
                .collect();
            if entries.is_empty() { Value::empty_array() } else { Value::map(entries) }
        },
        "hero.heroLevelUp" => mutate_args(p, r) => {
            let (hid, levels) = (r.pos_u64(0, "invalid_hid")?, r.clamped_u32(1, 1, 1, 50));
            let out = p.level_up_hero(hid, levels, u64::from(levels) * 1_000, u64::from(levels) * 100, now_ts)?;
            Ok(MutationEffects::ok(ok_map([
                ("hid", u64_to_amf3(out.hid)),
                ("level", Value::Int(out.new_level as i32)),
                ("exp", u64_to_amf3(out.exp)),
                ("fight_force", u64_to_amf3(p.fc)),
            ])))
        },
        "hero.skillLevelUp" => mutate_args(p, r) => {
            let (hid, skill_id) = (r.pos_u64(0, "invalid_hid")?, r.min_u32(1, 1001, 1));
            if !p.heroes.contains_key(&hid) { return Err(PlayerError::HeroNotFound(hid).into()); }
            p.consume_currency(CurrencyType::Copper, 2_000, now_ts)?;
            let hero = p.heroes.get_mut(&hid).expect("checked");
            if !hero.talent.contains(&skill_id) { hero.talent.push(skill_id); }
            hero.hero_star_level = hero.hero_star_level.saturating_add(1);
            let new_star_lv = hero.hero_star_level;
            p.recalculate_fight_force();
            Ok(MutationEffects::ok(ok_map([("hid", u64_to_amf3(hid)), ("skill_lv", Value::Int(new_star_lv as i32))])))
        },
        "evolve.doEvolve" => mutate_args(p, r) => {
            let hid = r.pos_u64(0, "invalid_hid")?;
            let sac_hids: Vec<u64> = match r.get(1) {
                Some(Value::Array { dense, .. }) => dense.iter().filter_map(|v| v.as_i64().filter(|&n| n > 0).map(|n| n as u64)).collect(),
                _ => Vec::new(),
            };
            let new_ev = p.evolve_hero(hid, &sac_hids, 5_000, now_ts)?;
            Ok(MutationEffects::ok(ok_map([
                ("hid", u64_to_amf3(hid)),
                ("evolve_level", Value::Int(new_ev as i32)),
                ("fight_force", u64_to_amf3(p.fc)),
            ])))
        },
        "evolve.surmount" | "hero.upgrade" => mutate(p, HeroHidArg(hid), HeroHidArg) => {
            let new_up = p.upgrade_hero(hid, 5_000, now_ts)?;
            Ok(MutationEffects::ok(ok_map([
                ("hid", u64_to_amf3(hid)),
                ("upgrade_level", Value::Int(new_up as i32)),
                ("fight_force", u64_to_amf3(p.fc)),
            ])))
        },
        "hero.lock" | "hero.unLock" => mutate(p, HeroHidArg(hid), HeroHidArg) => {
            p.set_hero_lock(hid, req.method == "hero.lock")?;
            Ok(MutationEffects::quiet_ok())
        },

        // Domain 6: Squad, 3x3 Battle Grid, Partner, 9-Slot Arming & Forge
        "formation.getHeroes" | "formation.getSquad" => query(profile) => {
            Value::dense_array((1..=6u8).map(|slot| u64_to_amf3(profile.formation.position.get(&slot).copied().unwrap_or(0))).collect())
        },
        "formation.getOtherPositionInfo" => store_query(r, ArgReader<'_>) => {
            let other = store.get_player(r.pos_u64(0, "invalid_target_uid")?).ok_or_else(|| "target_player_not_found".to_string())?;
            let mut pos_json = other.formation.to_vnsg2_position_info(&other.heroes);
            if let PlayerJson::Object(ref mut map) = pos_json {
                for (k, v) in [
                    ("uid", PlayerJson::from(other.uid)),
                    ("uname", PlayerJson::from(other.uname.clone())),
                    ("level", PlayerJson::from(other.level)),
                    ("vip", PlayerJson::from(other.vip)),
                    ("fight_force", PlayerJson::from(other.fc)),
                ] {
                    map.insert(k.into(), v);
                }
            }
            Ok(player_json_to_amf3(&pos_json))
        },
        "formation.setHeroToPosition" => mutate_args(p, r) => {
            let (hid, slot) = parse_squad_hid_and_slot(r.raw(), false)?;
            p.set_squad_slot(slot, hid)?;
            Ok(MutationEffects::ok_str())
        },
        "formation.unsetHeroFromPosition" => mutate(p, SquadSlotArg(slot), SquadSlotArg) => {
            p.unset_squad_slot(slot)?;
            Ok(MutationEffects::ok_str())
        },
        "formation.transHero" => mutate_args(p, r) => {
            p.swap_squad_slots(r.in_range_u8(0, 1, 6, "invalid_formation_slot")?, r.in_range_u8(1, 1, 6, "invalid_formation_slot")?)?;
            Ok(MutationEffects::ok_str().without_user_push())
        },
        "formation.set" => mutate_args(p, r) => {
            let first = r.get(0).ok_or_else(|| MutationError::from("missing_grid_formation"))?;
            let grid_map: BTreeMap<u8, u64> = parse_positive_indexed_map(first)
                .map_err(|_| MutationError::from("invalid_grid_formation"))?
                .into_iter()
                .collect();
            p.set_grid_formation(grid_map)?;
            Ok(MutationEffects::quiet_ok())
        },
        "formation.openPartner" => mutate(p, SquadSlotArg(slot), SquadSlotArg) => {
            p.open_partner_slot(slot)?;
            Ok(MutationEffects::quiet_ok())
        },
        "formation.setPartner" => mutate_args(p, r) => {
            let (hid, slot) = parse_squad_hid_and_slot(r.raw(), true)?;
            p.set_partner_slot(slot, hid)?;
            Ok(MutationEffects::quiet_ok())
        },
        "formation.unsetMultiPartner" => mutate_args(p, r) => {
            let slots: Vec<u8> = match r.get(0) {
                Some(Value::Array { dense, .. }) => dense.iter().filter_map(Value::as_i64)
                    .map(|s| (1..=6).contains(&s).then_some(s as u8).ok_or_else(|| MutationError::from("invalid_formation_slot")))
                    .collect::<Result<_, _>>()?,
                _ => (1..=6u8).collect(),
            };
            for s in slots {
                if p.formation.partner.contains_key(&s) { p.set_partner_slot(s, 0)?; }
            }
            Ok(MutationEffects::quiet_ok())
        },
        "formation.addArming" | "hero.addArming" => mutate_args(p, r) => {
            let (squad_slot, equip_slot) = r.equip_target()?;
            let item_or_gid = r.pos_u64(2, "invalid_equip_args")?;
            let bag_gid = resolve_bag_gid(p, item_or_gid).ok_or_else(|| MutationError::from("item_not_found"))?;
            let returned = p.equip_item(squad_slot, equip_slot, bag_gid)?;
            let mut eff = MutationEffects::ok(ok_map([("fight_force", u64_to_amf3(p.fc))]));
            eff.record_equip_swap(bag_gid, returned);
            Ok(eff)
        },
        "formation.addMultiArming" | "hero.addMultiArming" => mutate_args(p, r) => {
            let squad_slot = r.in_range_u8(0, 1, 6, "invalid_formation_slot")?;
            let mut pairs = Vec::new();
            if let Some(Ok(raw_pairs)) = r.get(1).map(parse_positive_indexed_map) {
                for (s_u8, id) in raw_pairs { pairs.push((EquipSlotType::from_u8(s_u8)?, id)); }
            }
            if pairs.is_empty() { return Err("empty_arming_list".into()); }
            let mut eff = MutationEffects::ok_str();
            for (es, raw_id) in pairs {
                let gid = resolve_bag_gid(p, raw_id).ok_or_else(|| PlayerError::ItemNotFound(format!("id={raw_id}")))?;
                eff.bag_removed.push(gid);
                if let Some(ret_it) = p.equip_item(squad_slot, es, gid)? { eff.bag_upserted.push(ret_it); }
            }
            eff.bag_removed.retain(|g| !eff.bag_upserted.iter().any(|u| u.gid == *g));
            Ok(eff)
        },
        "formation.removeArming" | "hero.removeArming" => mutate_args(p, r) => {
            let (squad_slot, equip_slot) = r.equip_target()?;
            let bag_item = p.unequip_item(squad_slot, equip_slot)?;
            let gid = bag_item.gid;
            Ok(MutationEffects::ok(ok_map([("gid", Value::Int(gid as i32)), ("fight_force", u64_to_amf3(p.fc))]))
                .with_bag_added([bag_item]))
        },
        "formation.transferArming" | "hero.transferArming" => mutate_args(p, r) => {
            let fs = r.in_range_u8(0, 1, 6, "invalid_equip_args")?;
            let ts = r.in_range_u8(1, 1, 6, "invalid_equip_args")?;
            let es_i64 = r.opt_i64(2, 1);
            if !(1..=9).contains(&es_i64) { return Err("invalid_equip_args".into()); }
            let es = es_i64 as u8;
            if fs != ts {
                let src_item = p.formation.arming.get_mut(&fs).and_then(|m| m.remove(&es))
                    .ok_or_else(|| PlayerError::ItemNotFound(format!("no arming at slot={fs}, equip={es}")))?;
                if let Some(prev) = p.formation.arming.entry(ts).or_default().insert(es, src_item) {
                    p.formation.arming.entry(fs).or_default().insert(es, prev);
                }
                p.recalculate_fight_force();
            }
            Ok(MutationEffects::ok_str())
        },
        "forge.upgradeArm" | "forge.autoReinforceArm" => mutate_args(p, r) => {
            let target_id = r.pos_u64(0, "invalid_forge_target")?;
            let is_auto = req.method == "forge.autoReinforceArm";
            let levels = if is_auto { 5 } else { r.clamped_u32(1, 1, 1, 20) };
            handle_forge_upgrade(p, target_id, levels, is_auto, now_ts)
        },

        // Domains 7–12: Extended Castle, Raffle, Copy, Arena & Sign RPCs
        "castle.levelUp" => mutate(p, (), ()) => {
            p.consume_currency(CurrencyType::Copper, 5_000, now_ts)?;
            let next_lv = bump_va_u32(p, "castle_level", 1, 1)?;
            Ok(MutationEffects::ok(ok_map([("level", Value::Int(next_lv as i32))])))
        },
        "raffle.refreshHotByHand" => custom((), ()) => {
            let mut ok = execute_player_mutation(store, gameplay, uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, 20, now_ts)?;
                Ok(MutationEffects::ok_str())
            })?;
            ok.ret = dto_to_amf3(&gameplay.raffle.get_info(uid, now_ts), true);
            Ok(ok)
        },
        "copy.oneKeyResetAtk" => gameplay(gp, copy, _arena, r) => {
            copy.reset_attack_count(gp, r.min_u32(0, 1001, 1), r.min_u32(1, 100_101, 1), now_ts)
                .map(|(ret, pushes)| (ret, pushes, Vec::new()))
        },
        "copy.fetchFresherReward" => mutate(p, (), ()) => {
            if matches!(p.va_config.get("fresher_reward_claimed"), Some(PlayerJson::Bool(true))) {
                return Err("already_claimed".into());
            }
            p.grant_currency(CurrencyType::Gold, 500, now_ts);
            p.grant_currency(CurrencyType::Copper, 50_000, now_ts);
            p.set_user_config("fresher_reward_claimed", PlayerJson::Bool(true))?;
            Ok(MutationEffects::ok(ok_map([("gold", Value::Int(500)), ("copper", Value::Int(50_000))])))
        },
        "copy.getStronghRecord" | "copy.getFamousPassTimes" | "arena.getTopReports" => const_val() => {
            Value::empty_array()
        },
        "arena.quickChallenge" => gameplay(gp, copy, arena, r) => {
            let pos = r.get(1).or_else(|| r.get(0)).and_then(Value::as_i64).unwrap_or(1).max(1) as u32;
            arena.challenge(gp, pos, copy, now_ts)
        },
        "arena.buyChallengeTimes" => mutate(p, ClampedU32(num), ClampedU32<1, 20>) => {
            p.consume_currency(CurrencyType::Gold, u64::from(num) * 20, now_ts)?;
            let total_bought = bump_va_u32(p, "arena_buy_times", 0, num)?;
            Ok(MutationEffects::ok(ok_map([("buy_num", Value::Int(total_bought as i32))])))
        },
        "arena.fetchChallengeNumReward" => mutate(p, MinU32(reward_id), MinU32<1>) => {
            claim_va_set_entry(p, "arena_num_rewards", reward_id, "already_claimed")?;
            p.grant_currency(CurrencyType::Gold, 200, now_ts);
            p.grant_currency(CurrencyType::Prestige, 500, now_ts);
            Ok(MutationEffects::ok_str())
        },
        "sign.gainRechargeSignReward" => mutate(p, MinU32(reward_id), MinU32<1>) => {
            if p.is_recharge == 0 && p.vip == 0 { return Err("recharge_required".into()); }
            claim_va_set_entry(p, "recharge_sign_rewards", reward_id, "already_claimed")?;
            p.grant_currency(CurrencyType::Gold, 300, now_ts);
            Ok(MutationEffects::ok_str())
        },
    }
}
