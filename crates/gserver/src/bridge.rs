//! Type-faithful bridges between `gserver-protocol::Value`, `gserver-player::JsonValue`,
//! `gserver-gameplay::protocol::DtoValue`, and `gserver-rpc-catalog::Amf3Value`, plus lossless
//! bidirectional state sync between persistent `PlayerProfile` and live `GameplayEngine` `PlayerState`
//! (fixing **B-08** authentic GID preservation and **B-19** `PlayerRaffleState` WAL persistence).

use std::collections::{BTreeMap, BTreeSet};

use gserver_gameplay::player::{
    BagCategory, BagItem as GameplayBagItem, HeroInstance as GameplayHero,
    PlayerState as GameplayPlayer,
};
use gserver_gameplay::protocol::{DtoValue, RpcRequest as GameplayRpcRequest};
use gserver_gameplay::shop_pub::PlayerRaffleState;
use gserver_player::{
    classify_template_id, BagItem as ProfileBagItem, BagType, CurrencyType, HeroInstance,
    JsonValue as PlayerJson, PlayerProfile,
};
use gserver_protocol::amf3::{AMF3_INT_MAX, AMF3_INT_MIN};
use gserver_protocol::{zlib_compress, zlib_decompress, Value};
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
        PlayerJson::Array(arr) => Value::dense_array(arr.iter().map(player_json_to_amf3).collect()),
        PlayerJson::Object(map) => {
            if map.is_empty() {
                Value::empty_array()
            } else {
                Value::map(map.iter().map(|(k, v)| (k.clone(), player_json_to_amf3(v))))
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

fn is_already_zlib_compressed(raw: &[u8]) -> bool {
    raw.len() >= 6 && raw[0] == 0x78 && zlib_decompress(raw).is_ok()
}

/// Converts `gserver_gameplay::protocol::DtoValue` into `gserver_protocol::Value`.
///
/// When `compress_blobs` is `true`, nested `DtoValue::Bytes` payloads (used in
/// `chat.get*LastMsgs`, `arena.challenge.battleRecord`, `babeltower.attack.fightRet`, and
/// `battle.getRecord`) are RFC 1950 Zlib-compressed (avoiding double-compression if already
/// zlib-wrapped) and emitted as `Value::ByteString` (`0x06`), matching `GameUtil:unzipData(blob)`.
pub fn dto_to_amf3(val: &DtoValue, compress_blobs: bool) -> Value {
    match val {
        DtoValue::Null => Value::Null,
        DtoValue::Bool(b) => Value::Bool(*b),
        DtoValue::Int(i) => i64_to_amf3(*i),
        DtoValue::Float(f) => Value::Double(*f),
        DtoValue::Str(s) => Value::String(s.clone()),
        DtoValue::Bytes(raw) => {
            let payload = if compress_blobs && !is_already_zlib_compressed(raw) {
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

/// Threads positional `args`, 0-indexed `arg_zero`, associative `named_args`, and `raw_args`
/// from `gserver_protocol::RpcRequest` into `gserver_gameplay::protocol::RpcRequest`.
pub fn to_gameplay_request(req: &gserver_protocol::RpcRequest) -> GameplayRpcRequest {
    GameplayRpcRequest {
        method: req.method.clone(),
        args: req.args.iter().map(amf3_to_dto).collect(),
        callback_name: req.callback_name.clone(),
        token: req.token.clone(),
        arg_zero: req.arg_zero().map(amf3_to_dto),
        named_args: req
            .named_args
            .iter()
            .map(|(k, v)| (k.clone(), amf3_to_dto(v)))
            .collect(),
        raw_args: req.raw_args.as_ref().map(amf3_to_dto),
    }
}

fn read_va_u64(profile: &PlayerProfile, key: &str) -> u64 {
    profile
        .va_config
        .get(key)
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

fn write_va_u64(profile: &mut PlayerProfile, key: &str, val: u64) {
    if val > 0 {
        profile
            .va_config
            .insert(key.to_owned(), PlayerJson::from(val));
    } else {
        profile.va_config.remove(key);
    }
}

/// Hydrates a `GameplayEngine` `PlayerState` from a persistent `PlayerProfile`.
pub fn profile_to_gameplay_state(profile: &PlayerProfile) -> GameplayPlayer {
    profile_to_gameplay_state_with_store(profile, None)
}

/// Hydrates a `GameplayEngine` `PlayerState` from a persistent `PlayerProfile`,
/// preserving authentic `10_001..=740_000` GIDs (`B-08`), all `PriceTypeConst` currencies,
/// expanded sub-bag capacities, and hero battle skills.
pub fn profile_to_gameplay_state_with_store(
    profile: &PlayerProfile,
    data_store: Option<&gserver_data::GameDataStore>,
) -> GameplayPlayer {
    let mut gp = GameplayPlayer::new(profile.uid, profile.uname.clone(), profile.utid);
    gp.pid = profile.pid.clone();
    gp.level = profile.level.max(1);
    gp.exp = profile.exp;
    gp.vip = profile.vip;
    gp.vip_exp = profile.vip_exp;
    gp.is_recharge = profile.is_recharge;
    gp.gold_recharge_accum_all = profile.gold_recharge_accum_all;

    // Core & Extended `PriceTypeConst` Currencies
    gp.gold = profile.currency(CurrencyType::Gold).min(i64::MAX as u64) as i64;
    gp.gold_bind = profile
        .currency(CurrencyType::GoldBind)
        .min(i64::MAX as u64) as i64;
    gp.copper = profile
        .currency(CurrencyType::Copper)
        .min(GameplayPlayer::COPPER_CAP as u64) as i64;
    gp.hero_soul = profile.currency(CurrencyType::HeroSoul);
    gp.prestige = profile.currency(CurrencyType::Prestige);
    gp.arena_coin = profile.currency(CurrencyType::ArenaCoin);
    gp.fame = profile.currency(CurrencyType::Fame);
    gp.execution = profile.currency(CurrencyType::Execution) as u32;
    gp.execution_time = profile.execution_time;
    gp.energy = profile.currency(CurrencyType::Energy) as u32;
    gp.energy_time = profile.energy_time;
    gp.tower_num = profile.currency(CurrencyType::TowerCoin);
    gp.business_coin = profile.currency(CurrencyType::BusinessCoin);
    gp.clan_contri = profile
        .currency(CurrencyType::ClanContri)
        .max(profile.currency(CurrencyType::GuildPoint));
    gp.boss_coin = profile.currency(CurrencyType::BossCoin);
    gp.awake_coin = profile
        .currency(CurrencyType::AwakeArmCoin)
        .max(profile.currency(CurrencyType::JewelNum));
    gp.rebel_coin = profile.currency(CurrencyType::RebelCoin);
    gp.pet_coin = profile.currency(CurrencyType::PetCoin);
    gp.world_coin = profile
        .currency(CurrencyType::WorldCoin)
        .max(profile.currency(CurrencyType::WorldTeamCoin));
    gp.world_person_score = profile.currency(CurrencyType::WorldPersonScore);
    gp.lordwar_coin = profile.currency(CurrencyType::LordwarCoin);
    gp.godarm_coin = profile.currency(CurrencyType::GodarmCoin);
    gp.god_animal_coin = profile.currency(CurrencyType::GodanimalCoin);
    gp.god_animal_explore_coin = profile.currency(CurrencyType::GodAnimalExploreCoin);
    gp.hero_legend_coin = profile.currency(CurrencyType::HeroLegendCoin);
    gp.legend_book_coin = profile.currency(CurrencyType::LegendBookCoin);
    gp.brochure_coin = profile.currency(CurrencyType::BrochureCoin);
    gp.classic_war_coin = profile.currency(CurrencyType::ClassicWarCoin);
    gp.bingshu_coin = profile.currency(CurrencyType::BingshuCoin);
    gp.devil_coin = profile.currency(CurrencyType::DevilCoin);
    gp.country_war_coin = profile.currency(CurrencyType::CountryWarCoin);
    gp.skincoin_purple = profile.currency(CurrencyType::SkincoinPurple);
    gp.skincoin_orange = profile.currency(CurrencyType::SkincoinOrange);
    gp.skincoin_red = profile.currency(CurrencyType::SkincoinRed);
    gp.skincoin_gold = profile.currency(CurrencyType::SkincoinGold);
    gp.skincoin_darkgold = profile.currency(CurrencyType::SkincoinDarkgold);
    gp.unify_coin = profile.currency(CurrencyType::UnifyCoin);
    gp.first_ride_coin = profile.currency(CurrencyType::FirstRideCoin);
    gp.soaring_coin = profile.currency(CurrencyType::SoaringCoin);
    gp.cross_guild_coin = profile.currency(CurrencyType::CrossGuildCoin);
    gp.qushuiyan_coin = profile.currency(CurrencyType::QushuiyanCoin);
    gp.voucher_coin = profile.currency(CurrencyType::VoucherCoin);
    gp.time_machine_voucher_coin = profile.currency(CurrencyType::TimeMachineVoucherCoin);
    gp.iron_gold_coin = profile.currency(CurrencyType::IronGoldCoin);

    // Extra activity coins persisted in `va_config`
    gp.star_soul = read_va_u64(profile, "_ext_star_soul");
    gp.huntdeer_coin = read_va_u64(profile, "_ext_huntdeer_coin");
    gp.bingfu_coin = read_va_u64(profile, "_ext_bingfu_coin");
    gp.darkgold_coin = read_va_u64(profile, "_ext_darkgold_coin");
    gp.babel_tower_coin = read_va_u64(profile, "_ext_babel_tower_coin");
    gp.tm_treasure_shop_coin = read_va_u64(profile, "_ext_tm_treasure_shop_coin");

    gp.dress_id = profile.dress_id;
    gp.avatar_id = profile.avatar_id;
    gp.online_accum_secs = profile.online_accum_time.max(gp.online_accum_secs);
    gp.chat_ban_until = profile.ban_chat_time;
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
        if let Some(ds) = data_store {
            let resolved = ds.resolve_hero_battle_skills(hero.htid, hero.awake_rank);
            gh.normal_skill_id = resolved.normal_skill_id;
            gh.rage_skill_id = resolved.rage_skill_id;
        }
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
    if !gp.roster.formation.contains(&profile.protagonist_hid) && gp.roster.formation[0] == 0 {
        gp.roster.formation[0] = profile.protagonist_hid;
    }

    // Sync sub-bag capacities & preserve authentic `item.gid` + `item.item_id` (`B-08`!)
    for (&bag_type, &max_grids) in &profile.inventory.grid_max_num {
        if let Some(cat) = BagCategory::from_gid(bag_type.grid_start()) {
            gp.bag.grid_max.insert(cat, max_grids);
        }
    }

    gp.bag.slots.clear();
    let mut max_item_id = profile.next_item_id;
    for (&bag_type, sub) in &profile.inventory.bags {
        let (range_start, range_end) = bag_type.gid_range();
        for (&gid, item) in sub {
            if item.item_id >= max_item_id {
                max_item_id = item.item_id + 1;
            }
            let effective_gid = if (range_start..=range_end).contains(&gid)
                && !gp.bag.slots.contains_key(&gid)
            {
                gid
            } else {
                // Fallback only if legacy test data had an out-of-range GID
                let mut alloc = range_start;
                while gp.bag.slots.contains_key(&alloc) && alloc < range_end {
                    alloc += 1;
                }
                alloc
            };
            gp.bag.slots.insert(
                effective_gid,
                GameplayBagItem {
                    gid: effective_gid,
                    item_id: item.item_id,
                    item_template_id: item.item_template_id,
                    item_num: item.item_num,
                    item_time: item.item_time,
                },
            );
        }
    }
    gp.bag.set_next_item_id(max_item_id.max(1_000_001));

    gp
}

/// Synchronizes mutations made inside `GameplayEngine` (GM commands, gacha, shop, battle drops)
/// back into the persistent `PlayerProfile` while preserving exact `gid`s, `item_id`s, `va_item_text`
/// progression, removed/recruited heroes, and all `PriceTypeConst` currencies.
pub fn sync_gameplay_back_to_profile(
    gp: &GameplayPlayer,
    profile: &mut PlayerProfile,
    now_ts: u64,
) {
    if !gp.uname.is_empty() {
        profile.uname = gp.uname.clone();
    }
    profile.level = gp.level.max(1);
    profile.exp = gp.exp;
    if gp.gold_recharge_accum_all > profile.gold_recharge_accum_all {
        let delta_nb = gp.gold_recharge_accum_all - profile.gold_recharge_accum_all;
        profile.record_recharge_nb(delta_nb, now_ts);
    }
    if gp.is_recharge > 0 {
        profile.is_recharge = 1;
    }
    profile.vip = gp.vip;
    profile.vip_exp = gp.vip_exp;

    // Sync coupled Prestige <-> ArenaCoin without `.max()` resurrecting spent ArenaCoin!
    let prev_arena = profile.currency(CurrencyType::ArenaCoin);
    let prev_prestige = profile.currency(CurrencyType::Prestige);
    let resolved_arena_prestige = if gp.arena_coin != prev_arena {
        gp.arena_coin
    } else if gp.prestige != prev_prestige {
        gp.prestige
    } else {
        gp.arena_coin.max(gp.prestige)
    };

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
        .insert(CurrencyType::Prestige, resolved_arena_prestige);
    profile
        .currencies
        .insert(CurrencyType::ArenaCoin, resolved_arena_prestige);
    profile.currencies.insert(CurrencyType::Fame, gp.fame);
    profile
        .currencies
        .insert(CurrencyType::Execution, u64::from(gp.execution));
    profile
        .currencies
        .insert(CurrencyType::Energy, u64::from(gp.energy));
    profile
        .currencies
        .insert(CurrencyType::TowerCoin, gp.tower_num);
    profile
        .currencies
        .insert(CurrencyType::BusinessCoin, gp.business_coin);
    profile
        .currencies
        .insert(CurrencyType::ClanContri, gp.clan_contri);
    profile
        .currencies
        .insert(CurrencyType::GuildPoint, gp.clan_contri);
    profile
        .currencies
        .insert(CurrencyType::BossCoin, gp.boss_coin);
    profile
        .currencies
        .insert(CurrencyType::AwakeArmCoin, gp.awake_coin);
    profile
        .currencies
        .insert(CurrencyType::JewelNum, gp.awake_coin);
    profile
        .currencies
        .insert(CurrencyType::RebelCoin, gp.rebel_coin);
    profile
        .currencies
        .insert(CurrencyType::PetCoin, gp.pet_coin);
    profile
        .currencies
        .insert(CurrencyType::WorldCoin, gp.world_coin);
    profile
        .currencies
        .insert(CurrencyType::WorldTeamCoin, gp.world_coin);
    profile
        .currencies
        .insert(CurrencyType::WorldPersonScore, gp.world_person_score);
    profile
        .currencies
        .insert(CurrencyType::LordwarCoin, gp.lordwar_coin);
    profile
        .currencies
        .insert(CurrencyType::GodarmCoin, gp.godarm_coin);
    profile
        .currencies
        .insert(CurrencyType::GodanimalCoin, gp.god_animal_coin);
    profile.currencies.insert(
        CurrencyType::GodAnimalExploreCoin,
        gp.god_animal_explore_coin,
    );
    profile
        .currencies
        .insert(CurrencyType::HeroLegendCoin, gp.hero_legend_coin);
    profile
        .currencies
        .insert(CurrencyType::LegendBookCoin, gp.legend_book_coin);
    profile
        .currencies
        .insert(CurrencyType::BrochureCoin, gp.brochure_coin);
    profile
        .currencies
        .insert(CurrencyType::ClassicWarCoin, gp.classic_war_coin);
    profile
        .currencies
        .insert(CurrencyType::BingshuCoin, gp.bingshu_coin);
    profile
        .currencies
        .insert(CurrencyType::DevilCoin, gp.devil_coin);
    profile
        .currencies
        .insert(CurrencyType::CountryWarCoin, gp.country_war_coin);
    profile
        .currencies
        .insert(CurrencyType::SkincoinPurple, gp.skincoin_purple);
    profile
        .currencies
        .insert(CurrencyType::SkincoinOrange, gp.skincoin_orange);
    profile
        .currencies
        .insert(CurrencyType::SkincoinRed, gp.skincoin_red);
    profile
        .currencies
        .insert(CurrencyType::SkincoinGold, gp.skincoin_gold);
    profile
        .currencies
        .insert(CurrencyType::SkincoinDarkgold, gp.skincoin_darkgold);
    profile
        .currencies
        .insert(CurrencyType::UnifyCoin, gp.unify_coin);
    profile
        .currencies
        .insert(CurrencyType::FirstRideCoin, gp.first_ride_coin);
    profile
        .currencies
        .insert(CurrencyType::SoaringCoin, gp.soaring_coin);
    profile
        .currencies
        .insert(CurrencyType::CrossGuildCoin, gp.cross_guild_coin);
    profile
        .currencies
        .insert(CurrencyType::QushuiyanCoin, gp.qushuiyan_coin);
    profile
        .currencies
        .insert(CurrencyType::VoucherCoin, gp.voucher_coin);
    profile.currencies.insert(
        CurrencyType::TimeMachineVoucherCoin,
        gp.time_machine_voucher_coin,
    );
    profile
        .currencies
        .insert(CurrencyType::IronGoldCoin, gp.iron_gold_coin);

    write_va_u64(profile, "_ext_star_soul", gp.star_soul);
    write_va_u64(profile, "_ext_huntdeer_coin", gp.huntdeer_coin);
    write_va_u64(profile, "_ext_bingfu_coin", gp.bingfu_coin);
    write_va_u64(profile, "_ext_darkgold_coin", gp.darkgold_coin);
    write_va_u64(profile, "_ext_babel_tower_coin", gp.babel_tower_coin);
    write_va_u64(
        profile,
        "_ext_tm_treasure_shop_coin",
        gp.tm_treasure_shop_coin,
    );

    profile.execution_time = gp.execution_time.max(now_ts);
    profile.energy_time = gp.energy_time.max(now_ts);
    profile.ban_chat_time = gp.chat_ban_until;

    // Sync removed/sacrificed heroes as well as newly summoned or leveled heroes
    profile
        .heroes
        .retain(|hid, _| *hid == profile.protagonist_hid || gp.roster.heroes.contains_key(hid));
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

    // Sync 6-slot squad lineup without resurrecting unset companion slots
    for (idx, &hid) in gp.roster.formation.iter().enumerate() {
        let slot = (idx + 1) as u8;
        if hid > 0 && profile.heroes.contains_key(&hid) {
            profile.formation.position.insert(slot, hid);
        } else if profile.formation.position.get(&slot).copied() != Some(profile.protagonist_hid) {
            profile.formation.position.remove(&slot);
        }
    }
    profile.formation.sync_grid_with_squad();

    // Sync bag slots by exact `(BagType, gid)` so authentic GIDs (`10_001..=740_000`), `item_id`s,
    // and existing equipment `va_item_text` levels/locks are 100% preserved (`B-08`)!
    let mut target_by_bag: BTreeMap<BagType, BTreeMap<u32, &GameplayBagItem>> = BTreeMap::new();
    let mut max_seen_item_id = gp.bag.next_item_id();
    for (&gid, gp_item) in &gp.bag.slots {
        if gp_item.item_num == 0 {
            continue;
        }
        if gp_item.item_id >= max_seen_item_id {
            max_seen_item_id = gp_item.item_id + 1;
        }
        let bag_type = BagType::from_gid(gid).or_else(|| {
            classify_template_id(gp_item.item_template_id)
                .ok()
                .map(|(_, bt)| bt)
        });
        if let Some(bt) = bag_type {
            target_by_bag.entry(bt).or_default().insert(gid, gp_item);
        }
    }

    for bt in BagType::ALL {
        let cur_sub = profile.inventory.bags.entry(bt).or_default();
        if let Some(target_sub) = target_by_bag.get(&bt) {
            cur_sub.retain(|gid, _| target_sub.contains_key(gid));
            for (&gid, gp_item) in target_sub {
                let keep_existing = cur_sub.get(&gid).is_some_and(|ex| {
                    ex.item_id == gp_item.item_id && ex.item_template_id == gp_item.item_template_id
                });
                if keep_existing {
                    if let Some(ex) = cur_sub.get_mut(&gid) {
                        ex.item_num = gp_item.item_num;
                        ex.item_time = gp_item.item_time;
                    }
                } else if let Ok(new_item) = ProfileBagItem::new(
                    gid,
                    gp_item.item_id,
                    gp_item.item_template_id,
                    gp_item.item_num,
                    gp_item.item_time,
                ) {
                    cur_sub.insert(gid, new_item);
                }
            }
        } else {
            cur_sub.clear();
        }
    }
    if max_seen_item_id > profile.next_item_id {
        profile.next_item_id = max_seen_item_id;
    }

    profile.recalculate_fight_force();
}

// ============================================================================
// `PlayerRaffleState <-> profile.raffle_state` WAL Persistence (`B-19`)
// ============================================================================

fn set_to_json_array(set: &BTreeSet<u32>) -> PlayerJson {
    PlayerJson::Array(set.iter().copied().map(PlayerJson::from).collect())
}

fn json_array_to_set(val: Option<&PlayerJson>) -> BTreeSet<u32> {
    val.and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect())
        .unwrap_or_default()
}

fn va_hots_to_json(slots: &[(u32, [u32; 2]); 2]) -> PlayerJson {
    PlayerJson::Array(
        slots
            .iter()
            .map(|&(p, [s0, s1])| {
                PlayerJson::Array(vec![
                    PlayerJson::from(p),
                    PlayerJson::from(s0),
                    PlayerJson::from(s1),
                ])
            })
            .collect(),
    )
}

fn json_to_va_hots(val: Option<&PlayerJson>, default: [(u32, [u32; 2]); 2]) -> [(u32, [u32; 2]); 2] {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return default;
    };
    if arr.len() < 2 {
        return default;
    }
    let mut out = default;
    for i in 0..2 {
        if let Some(sub) = arr[i].as_array() {
            if sub.len() >= 3 {
                let p = sub[0].as_u64().unwrap_or(0) as u32;
                let s0 = sub[1].as_u64().unwrap_or(0) as u32;
                let s1 = sub[2].as_u64().unwrap_or(0) as u32;
                out[i] = (p, [s0, s1]);
            }
        }
    }
    out
}

fn pair_to_json(pair: [u32; 2]) -> PlayerJson {
    PlayerJson::Array(vec![PlayerJson::from(pair[0]), PlayerJson::from(pair[1])])
}

fn json_to_pair(val: Option<&PlayerJson>, default: [u32; 2]) -> [u32; 2] {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return default;
    };
    if arr.len() >= 2 {
        [
            arr[0].as_u64().unwrap_or(u64::from(default[0])) as u32,
            arr[1].as_u64().unwrap_or(u64::from(default[1])) as u32,
        ]
    } else {
        default
    }
}

/// Serializes `TavernRaffleEngine`'s `PlayerRaffleState` into `profile.raffle_state["tavern"]`
/// without clobbering `open_box_counts` or other entries stored in `profile.raffle_state`!
pub fn export_raffle_state_to_profile(st: &PlayerRaffleState, profile: &mut PlayerProfile) {
    let mut m = BTreeMap::new();
    m.insert("day_bucket".into(), PlayerJson::from(st.day_bucket));
    m.insert(
        "warrior_free_ready_at".into(),
        PlayerJson::from(st.warrior_free_ready_at),
    );
    m.insert(
        "warrior_free_left_today".into(),
        PlayerJson::from(st.warrior_free_left_today),
    );
    m.insert(
        "warrior_get_free_time".into(),
        PlayerJson::from(st.warrior_get_free_time),
    );
    m.insert(
        "divine_free_ready_at".into(),
        PlayerJson::from(st.divine_free_ready_at),
    );
    m.insert(
        "divine_free_used_today".into(),
        PlayerJson::from(st.divine_free_used_today),
    );
    m.insert(
        "divine_get_free_time".into(),
        PlayerJson::from(st.divine_get_free_time),
    );
    m.insert(
        "hot_free_ready_at".into(),
        PlayerJson::from(st.hot_free_ready_at),
    );
    m.insert("hot_free_used".into(), PlayerJson::from(st.hot_free_used));
    m.insert(
        "hot_get_free_time".into(),
        PlayerJson::from(st.hot_get_free_time),
    );
    m.insert(
        "red_free_ready_at".into(),
        PlayerJson::from(st.red_free_ready_at),
    );
    m.insert("red_free_used".into(), PlayerJson::from(st.red_free_used));
    m.insert(
        "red_get_free_time".into(),
        PlayerJson::from(st.red_get_free_time),
    );
    m.insert(
        "warrior_pity_counter".into(),
        PlayerJson::from(st.warrior_pity_counter),
    );
    m.insert(
        "divine_pity_counter".into(),
        PlayerJson::from(st.divine_pity_counter),
    );
    m.insert(
        "first_divine_ten_used".into(),
        PlayerJson::from(st.first_divine_ten_used),
    );
    m.insert("divine_luck".into(), PlayerJson::from(st.divine_luck));
    m.insert("hot_luck".into(), PlayerJson::from(st.hot_luck));
    m.insert("red_luck".into(), PlayerJson::from(st.red_luck));
    m.insert(
        "claimed_divine_boxes".into(),
        set_to_json_array(&st.claimed_divine_boxes),
    );
    m.insert(
        "claimed_hot_boxes".into(),
        set_to_json_array(&st.claimed_hot_boxes),
    );
    m.insert(
        "claimed_red_boxes".into(),
        set_to_json_array(&st.claimed_red_boxes),
    );
    m.insert("hot_spot_index".into(), PlayerJson::from(st.hot_spot_index));
    m.insert(
        "hot_refresh_count".into(),
        PlayerJson::from(st.hot_refresh_count),
    );
    m.insert("hot_num_today".into(), PlayerJson::from(st.hot_num_today));
    m.insert("red_num_today".into(), PlayerJson::from(st.red_num_today));
    m.insert("hot_va_hots".into(), va_hots_to_json(&st.hot_va_hots));
    m.insert("hot_next_pos".into(), pair_to_json(st.hot_next_pos));
    m.insert("red_va_hots".into(), va_hots_to_json(&st.red_va_hots));
    m.insert("red_next_pos".into(), pair_to_json(st.red_next_pos));
    m.insert(
        "recruitment_num".into(),
        PlayerJson::from(st.recruitment_num),
    );
    m.insert(
        "claimed_recruitment_tiers".into(),
        set_to_json_array(&st.claimed_recruitment_tiers),
    );
    profile
        .raffle_state
        .insert("tavern".into(), PlayerJson::Object(m));
}

/// Restores `PlayerRaffleState` from `profile.raffle_state["tavern"]` if previously persisted.
pub fn import_raffle_state_from_profile(profile: &PlayerProfile) -> Option<PlayerRaffleState> {
    let m = profile.raffle_state.get("tavern")?.as_object()?;
    let def = PlayerRaffleState::default();
    let u32_of = |k: &str, d: u32| m.get(k).and_then(|v| v.as_u64()).map_or(d, |n| n as u32);
    let u64_of = |k: &str, d: u64| m.get(k).and_then(|v| v.as_u64()).unwrap_or(d);
    let bool_of = |k: &str, d: bool| m.get(k).and_then(|v| v.as_bool()).unwrap_or(d);

    Some(PlayerRaffleState {
        day_bucket: u64_of("day_bucket", def.day_bucket),
        warrior_free_ready_at: u64_of("warrior_free_ready_at", def.warrior_free_ready_at),
        warrior_free_left_today: u32_of("warrior_free_left_today", def.warrior_free_left_today),
        warrior_get_free_time: u64_of("warrior_get_free_time", def.warrior_get_free_time),
        divine_free_ready_at: u64_of("divine_free_ready_at", def.divine_free_ready_at),
        divine_free_used_today: u32_of("divine_free_used_today", def.divine_free_used_today),
        divine_get_free_time: u64_of("divine_get_free_time", def.divine_get_free_time),
        hot_free_ready_at: u64_of("hot_free_ready_at", def.hot_free_ready_at),
        hot_free_used: u32_of("hot_free_used", def.hot_free_used),
        hot_get_free_time: u64_of("hot_get_free_time", def.hot_get_free_time),
        red_free_ready_at: u64_of("red_free_ready_at", def.red_free_ready_at),
        red_free_used: u32_of("red_free_used", def.red_free_used),
        red_get_free_time: u64_of("red_get_free_time", def.red_get_free_time),
        warrior_pity_counter: u32_of("warrior_pity_counter", def.warrior_pity_counter),
        divine_pity_counter: u32_of("divine_pity_counter", def.divine_pity_counter),
        first_divine_ten_used: bool_of("first_divine_ten_used", def.first_divine_ten_used),
        divine_luck: u32_of("divine_luck", def.divine_luck),
        hot_luck: u32_of("hot_luck", def.hot_luck),
        red_luck: u32_of("red_luck", def.red_luck),
        claimed_divine_boxes: json_array_to_set(m.get("claimed_divine_boxes")),
        claimed_hot_boxes: json_array_to_set(m.get("claimed_hot_boxes")),
        claimed_red_boxes: json_array_to_set(m.get("claimed_red_boxes")),
        hot_spot_index: u32_of("hot_spot_index", def.hot_spot_index),
        hot_refresh_count: u32_of("hot_refresh_count", def.hot_refresh_count),
        hot_num_today: u32_of("hot_num_today", def.hot_num_today),
        red_num_today: u32_of("red_num_today", def.red_num_today),
        hot_va_hots: json_to_va_hots(m.get("hot_va_hots"), def.hot_va_hots),
        hot_next_pos: json_to_pair(m.get("hot_next_pos"), def.hot_next_pos),
        red_va_hots: json_to_va_hots(m.get("red_va_hots"), def.red_va_hots),
        red_next_pos: json_to_pair(m.get("red_next_pos"), def.red_next_pos),
        recruitment_num: u32_of("recruitment_num", def.recruitment_num),
        claimed_recruitment_tiers: json_array_to_set(m.get("claimed_recruitment_tiers")),
    })
}

/// Converts `DtoValue` into `gserver_player::JsonValue` for durable `PlayerProfile` storage.
pub fn dto_to_player_json(val: &DtoValue) -> PlayerJson {
    match val {
        DtoValue::Null => PlayerJson::Null,
        DtoValue::Bool(b) => PlayerJson::Bool(*b),
        DtoValue::Int(i) => {
            if *i >= 0 {
                PlayerJson::UInt(*i as u64)
            } else {
                PlayerJson::Int(*i)
            }
        }
        DtoValue::Float(f) => PlayerJson::Float(*f),
        DtoValue::Str(s) => PlayerJson::Str(s.clone()),
        DtoValue::Bytes(b) => PlayerJson::Str(String::from_utf8_lossy(b).into_owned()),
        DtoValue::Array(arr) => PlayerJson::Array(arr.iter().map(dto_to_player_json).collect()),
        DtoValue::Map(map) => {
            let mut m = BTreeMap::new();
            for (k, v) in map {
                m.insert(k.clone(), dto_to_player_json(v));
            }
            PlayerJson::Object(m)
        }
    }
}

/// Converts `gserver_player::JsonValue` back into `DtoValue` for `GameplayEngine` hydration.
pub fn player_json_to_dto(val: &PlayerJson) -> DtoValue {
    match val {
        PlayerJson::Null => DtoValue::Null,
        PlayerJson::Bool(b) => DtoValue::Bool(*b),
        PlayerJson::Int(i) => DtoValue::Int(*i),
        PlayerJson::UInt(u) => DtoValue::Int((*u).min(i64::MAX as u64) as i64),
        PlayerJson::Float(f) => DtoValue::Float(*f),
        PlayerJson::Str(s) => DtoValue::Str(s.clone()),
        PlayerJson::Array(arr) => DtoValue::Array(arr.iter().map(player_json_to_dto).collect()),
        PlayerJson::Object(map) => {
            let mut m = BTreeMap::new();
            for (k, v) in map {
                m.insert(k.clone(), player_json_to_dto(v));
            }
            DtoValue::Map(m)
        }
    }
}
