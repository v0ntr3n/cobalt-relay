//! Strict Lua `parseData` / callback response schema validators.
//!
//! Validates every bootstrap RPC response envelope and `ret` payload against the exact Lua runtime
//! expectations extracted from `Network.lua`, `LoginProxy.lua`, `CreateUserScene.lua`,
//! `UserData.lua`, `OpenSwitchData.lua`, `ConfigData.lua`, `FormationData.lua`, `BagData.lua`,
//! `HeroFormationData.lua`, `HeroInfo.lua`, `CopyData.lua`, `GuideManager.lua`, `MainScene.lua`,
//! `EnterViewData.lua`, `ChatProxy.lua`, and `BarrageProxy.lua`.

use crate::amf3::Amf3Value;
use crate::bootstrap::{BAG_GRID_OPEN_TIMES_DEFAULTS, BAG_GRID_START_DEFAULTS, BAG_SUBBAG_KEYS};
use crate::envelope::RpcResponseEnvelope;
use crate::net_const;

/// Detailed schema violation explaining which Lua file/line would fail at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaSchemaViolation {
    pub method: String,
    pub field_path: String,
    pub reason: String,
}

impl LuaSchemaViolation {
    fn new(method: &str, field_path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            method: method.to_owned(),
            field_path: field_path.into(),
            reason: reason.into(),
        }
    }
}

/// Validate an [`RpcResponseEnvelope`] for `method` (given the client's `request_args`).
///
/// When `require_main_town_bypass` is `true`, also enforces tutorial/first-fight completion
/// invariants (`guide_id == 240 || guide_id == 360` and `100101.normal_star >= 1`) so
/// `MainLoading` reaches 100% and enters `MainTown` directly without forcing battle `100101`.
pub fn validate_response_for_method(
    method: &str,
    request_args: &[Amf3Value],
    resp: &RpcResponseEnvelope,
    require_main_town_bypass: bool,
) -> Result<(), Vec<LuaSchemaViolation>> {
    let mut errs = Vec::new();

    // 1. Common Network.lua + TimeManager.lua envelope checks
    if resp.err != "ok" {
        errs.push(LuaSchemaViolation::new(
            method,
            "envelope.err",
            format!("Network.lua requires err == 'ok', got '{}'", resp.err),
        ));
    }
    if resp.callback_name.is_empty() {
        errs.push(LuaSchemaViolation::new(
            method,
            "envelope.callback.callbackName",
            "callback.callbackName must be non-empty",
        ));
    }
    if resp.time <= 0 {
        errs.push(LuaSchemaViolation::new(
            method,
            "envelope.time",
            "TimeManager.lua requires positive Unix timestamp in seconds",
        ));
    }
    if resp.token.is_empty() {
        errs.push(LuaSchemaViolation::new(
            method,
            "envelope.token",
            "Network.lua requires non-empty string session token",
        ));
    }

    let ret = &resp.ret;

    // 2. Method-specific Lua callback / BindableData:parseData checks
    match method {
        net_const::CS_USER_LOGIN => {
            if matches!(ret, Amf3Value::Null | Amf3Value::Undefined) {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "LoginProxy:onUserLogin expects 'ok' or non-nil acknowledgement",
                ));
            }
        }
        net_const::CS_GET_USER_BEFRO_LOGIN => {
            if !ret.is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "LoginProxy:onGetUserBeforLogin indexes `ret.uid`; `ret` must be a Lua table",
                ));
            } else if let Some(map) = ret.as_map() {
                if let Some(uid_val) = map.get("uid") {
                    if uid_val.as_i64().is_none() {
                        errs.push(LuaSchemaViolation::new(
                            method,
                            "ret.uid",
                            "existing role `ret.uid` must be numeric for LoginProxy:loginUid(ret.uid)",
                        ));
                    }
                    match map.get("utid").and_then(Amf3Value::as_i64) {
                        Some(1 | 2) => {}
                        _ => errs.push(LuaSchemaViolation::new(
                            method,
                            "ret.utid",
                            "existing role `ret.utid` must be 1 (male) or 2 (female)",
                        )),
                    }
                }
            } else if let Some(arr) = ret.as_array() {
                if !arr.is_empty() {
                    errs.push(LuaSchemaViolation::new(
                        method,
                        "ret",
                        "CRITICAL TRAP: non-empty 1-based array in getUserBeforLogin has `ret.uid == nil` in LoginProxy.lua:1018 and misroutes existing role into CreateUserScene! Return a string-keyed Map with top-level `uid`.",
                    ));
                }
            }
        }
        net_const::CS_USER_GET_RANDOM_NAME => {
            match ret.as_array() {
                Some(items) if !items.is_empty() => {
                    for (idx, item) in items.iter().enumerate() {
                        match item.get_field("name").and_then(Amf3Value::as_str) {
                            Some(s) if !s.is_empty() => {}
                            _ => errs.push(LuaSchemaViolation::new(
                                method,
                                format!("ret[{}].name", idx + 1),
                                "CreateUserScene.lua:380 indexes `names[#names]['name']`; each element must have non-empty string `name`",
                            )),
                        }
                    }
                }
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "CreateUserScene expects non-empty 1-based AMF3 Array of `{ name = <string> }`",
                )),
            }
        }
        net_const::CS_USER_CREATEUSER => {
            if ret.as_str() != Some("ok") {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "CreateUserScene expects `ret == 'ok'` on successful character creation",
                ));
            }
        }
        net_const::CS_USER_USERLOGIN => {
            if ret.as_str() != Some("ok") {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "LoginProxy:onLoginUid requires literal string `ret == 'ok'` ('fail' or 'ban:...' aborts bootstrap)",
                ));
            }
        }
        net_const::CS_GET_USER => {
            validate_get_user(method, ret, &mut errs);
        }
        net_const::CS_OPENSWITCH_GETSWITCH => {
            if !ret.is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "OpenSwitchData:parseData expects 1-based array / table of unlocked switch IDs",
                ));
            }
            if let Some(arr) = ret.as_array() {
                for (i, item) in arr.iter().enumerate() {
                    if item.as_i64().is_none() {
                        errs.push(LuaSchemaViolation::new(
                            method,
                            format!("ret[{}]", i + 1),
                            "switch node ID must be an integer",
                        ));
                    }
                }
            }
        }
        net_const::CS_USER_GET_VA_CONFIG => {
            if !ret.is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "ConfigData:parseData expects a string-keyed Lua table",
                ));
            } else if require_main_town_bypass {
                match ret.get_field("guide_id").and_then(Amf3Value::as_i64) {
                    Some(240 | 360) => {}
                    other => errs.push(LuaSchemaViolation::new(
                        method,
                        "ret.guide_id",
                        format!(
                            "MainTown direct entry requires guide_id == 240 (GuideManager.END_ID) or 360 (-> 240), got {:?}",
                            other
                        ),
                    )),
                }
            }
        }
        net_const::CS_FORMATION_GET => {
            match ret.get_field("formation") {
                Some(Amf3Value::Map(m))
                    if !m.is_empty()
                        && m.values().all(|v| v.as_i64().is_some_and(|hid| hid > 0)) => {}
                Some(Amf3Value::Array(arr))
                    if !arr.is_empty() && arr.len() <= 6 && arr.iter().all(|v| v.as_i64().is_some()) => {}
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.formation",
                    "FormationData:parseData requires non-empty occupied `ret.formation` map or array",
                )),
            }
        }
        net_const::CS_ALL_ACTIVITY_GETACTIVITYINFO => {
            if !ret.is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "ActivityData:parseData / FestivalManager:initDb expects a Lua table",
                ));
            }
        }
        net_const::CS_GET_BAG_DATA => {
            if request_args.len() != 1 || !request_args[0].is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "request.args",
                    "BagData:getParam sends `{ {} }` (1-element array containing an empty table)",
                ));
            }
            validate_bag_info(method, ret, &mut errs);
        }
        net_const::CS_FORMATION_GET_POSITION_INFO => {
            validate_position_info(method, ret, &mut errs);
        }
        "halidom.getInfo" => {
            if !ret.is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "HalidomData:parseData calls `next(data)` which requires a Lua table",
                ));
            }
        }
        net_const::CS_COPY_LIST => {
            if request_args.first().and_then(Amf3Value::as_i64).is_none() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "request.args[1]",
                    "CopyData requests `copy.getList` with `[ 1 ]` (COPY_TYPE_1)",
                ));
            }
            if ret.get_field("fresherRewardIndex").and_then(Amf3Value::as_i64).is_none() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.fresherRewardIndex",
                    "CopyData.lua:46 computes `data.fresherRewardIndex + 1` without nil check",
                ));
            }
            match ret.get_field("copyData") {
                Some(v) if v.is_lua_table() => {}
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.copyData",
                    "CopyData:parseData expects table `ret.copyData`",
                )),
            }
        }
        net_const::CS_COPY_CHAPTERINFO => {
            let chap = request_args.first().and_then(Amf3Value::as_i64).unwrap_or(1001);
            match ret.get_field("va_copy") {
                Some(vc) if vc.is_lua_table() => {
                    if chap == 1001 && require_main_town_bypass {
                        let s100101 = vc.get_field("100101");
                        let stars = s100101
                            .and_then(|s| s.get_field("normal_star"))
                            .and_then(Amf3Value::as_i64)
                            .unwrap_or(0);
                        if stars < 1 {
                            errs.push(LuaSchemaViolation::new(
                                method,
                                "ret.va_copy['100101'].normal_star",
                                "MainScene:isGuideFirstFight checks getCopyStarNum(100101) < 1; normal_star must be >= 1 to enter MainTown at 100% without forcing copyPanel!",
                            ));
                        }
                    }
                }
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.va_copy",
                    "CopyInfoDataController expects `ret.va_copy` map",
                )),
            }
        }
        net_const::CS_SMALLGAME_GETINFO => {
            match ret.get_field("adopt_list") {
                Some(al) if al.is_lua_table() => {
                    if require_main_town_bypass
                        && al.get_field("1").and_then(Amf3Value::as_i64) != Some(1)
                    {
                        errs.push(LuaSchemaViolation::new(
                            method,
                            "ret.adopt_list['1']",
                            "adopt_list['1']=1 prevents EnterViewProxy gate 1 from hijacking MainTown",
                        ));
                    }
                }
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.adopt_list",
                    "EnterViewData:checkGateHasAdopt indexes `self.adopt_gate[tostring(gate_id)]`; `adopt_list` must be a non-nil table",
                )),
            }
        }
        net_const::CS_REWARD_GETREWARDLIST
        | net_const::CS_CHAT_WORLD_GET
        | net_const::CS_CHAT_GUILD_GET
        | "barrage.getBarrageRecord" => {
            if !ret.is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "Callback iterates `pairs(ret)` / `#ret`; `ret` must be a non-nil Lua table/array",
                ));
            }
        }
        "barrage.get" => {
            if ret.get_field("is_open").and_then(Amf3Value::as_bool).is_none() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.is_open",
                    "BarrageProxy:onGetState reads boolean `ret['is_open']`",
                ));
            }
            if ret.get_field("send_time").and_then(Amf3Value::as_i64).is_none() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.send_time",
                    "BarrageProxy:onGetState reads numeric `ret['send_time']`",
                ));
            }
        }
        "barrage.enterRoom" => {
            if matches!(ret, Amf3Value::Null | Amf3Value::Undefined) {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "BarrageProxy:onEnterRoom expects non-nil ack before invoking MainScene:init()",
                ));
            }
        }
        net_const::CS_COPY_ATTACK => {
            if ret.get_field("brid").and_then(Amf3Value::as_i64).unwrap_or(0) <= 0 {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.brid",
                    "FightProxy:parseBattleInfo requires positive integer `brid`",
                ));
            }
            match ret.get_field("drop") {
                Some(d) if d.is_lua_table() => {
                    if d.get_field("copper").and_then(Amf3Value::as_i64).is_none() {
                        errs.push(LuaSchemaViolation::new(
                            method,
                            "ret.drop.copper",
                            "CopyProxy:groupDrops reads numeric `data.copper`",
                        ));
                    }
                    if !d.get_field("item").map(Amf3Value::is_lua_table).unwrap_or(false) {
                        errs.push(LuaSchemaViolation::new(
                            method,
                            "ret.drop.item",
                            "CopyProxy:groupDrops iterates `pairs(data.item)`",
                        ));
                    }
                }
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.drop",
                    "CopyFightProxy:getAttackDataFromServer stores `ret['drop']` table",
                )),
            }
            match ret
                .get_field("battleInfo")
                .and_then(|bi| bi.get_field("arrTeam1"))
                .and_then(|t1| t1.as_array().and_then(|a| a.first()).or_else(|| t1.get_field("1")))
                .and_then(|w1| w1.get_field("arrHero"))
                .and_then(Amf3Value::as_map)
            {
                Some(heroes) if !heroes.is_empty() => {
                    for (slot_k, unit) in heroes {
                        match unit.get_field("arrSkill").and_then(Amf3Value::as_array) {
                            Some(skills)
                                if skills.len() >= 2
                                    && skills.iter().all(|sk| {
                                        sk.as_array()
                                            .and_then(|pair| pair.first())
                                            .and_then(Amf3Value::as_i64)
                                            .is_some_and(|sid| sid > 0)
                                    }) => {}
                            _ => errs.push(LuaSchemaViolation::new(
                                method,
                                format!("ret.battleInfo.arrTeam1['1'].arrHero['{}'].arrSkill", slot_k),
                                "SkillFactory:creatSkill requires positive normal & rage skill IDs",
                            )),
                        }
                    }
                }
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.battleInfo.arrTeam1['1'].arrHero",
                    "FightProxy:parseBattleInfo requires non-empty `arrTeam1['1'].arrHero` map",
                )),
            }
            match ret.get_field("userInfo") {
                Some(Amf3Value::Map(m)) if m.contains_key("1") && m.contains_key("2") => {}
                Some(Amf3Value::Array(u)) if u.len() >= 2 => {}
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.userInfo",
                    "FightProxy:parseBattleInfo requires `userInfo` table with keys '1' & '2' (or 2-element array)",
                )),
            }
        }
        net_const::CS_COPY_PASS => {
            if ret.get_field("battle_result").and_then(Amf3Value::as_i64).is_none() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.battle_result",
                    "CopyProxy:fightEnd checks `tonumber(ret.battle_result)`",
                ));
            }
            if !ret.get_field("drop").map(Amf3Value::is_lua_table).unwrap_or(false) {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.drop",
                    "CopyProxy:fightEnd passes `ret['drop']` to `groupDrops`",
                ));
            }
        }
        net_const::CS_ACTIVITY_STAR_CODE_OPEN_GETINFO => {
            if !ret.is_lua_table() {
                errs.push(LuaSchemaViolation::new(
                    method,
                    "ret",
                    "ActStarCodeData:parseData indexes `net_info[tostring(i)] == true`",
                ));
            } else if let Some(map) = ret.as_map() {
                for (k, v) in map {
                    if v.as_bool().is_none() {
                        errs.push(LuaSchemaViolation::new(
                            method,
                            format!("ret['{}']", k),
                            "ActStarCodeData:parseData checks `net_info[tostring(i)] == true`",
                        ));
                    }
                }
            }
        }
        net_const::CS_ACTIVITY_STAR_CODE_OPEN_RECEIVE => {
            match ret.get_field("reward").and_then(Amf3Value::as_array) {
                Some(arr) if !arr.is_empty() => {
                    for (idx, item) in arr.iter().enumerate() {
                        match item.as_array() {
                            Some(tr) if tr.len() == 3 => {
                                let ptype = tr[0].as_i64().unwrap_or(0);
                                let tid = tr[1].as_i64().unwrap_or(0);
                                let qty = tr[2].as_i64().unwrap_or(0);
                                if ptype <= 0 || qty <= 0 || (ptype == 4 && tid <= 0) {
                                    errs.push(LuaSchemaViolation::new(
                                        method,
                                        format!("ret.reward[{}]", idx + 1),
                                        "GainItemListInfoPanel requires valid `[priceType, id, num]` triplet (`id > 0` when `type == 4`)",
                                    ));
                                }
                            }
                            _ => errs.push(LuaSchemaViolation::new(
                                method,
                                format!("ret.reward[{}]", idx + 1),
                                "RewardManager:addRewardsByFixTb expects 3-element `[type, id, num]` array",
                            )),
                        }
                    }
                }
                _ => errs.push(LuaSchemaViolation::new(
                    method,
                    "ret.reward",
                    "ActStar gift claim must return non-empty `reward` triplet array",
                )),
            }
        }
        _ => {}
    }

    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn validate_get_user(method: &str, ret: &Amf3Value, errs: &mut Vec<LuaSchemaViolation>) {
    let Some(map) = ret.as_map() else {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret",
            "UserData:parseData requires string-keyed AMF3 Object map",
        ));
        return;
    };

    for num_field in [
        "uid",
        "utid",
        "level",
        "vip",
        "exp",
        "gold",
        "gold_bind",
        "gold_recharge_accum_all",
        "execution",
        "execution_time",
        "buy_execution_accum",
        "buy_execution_time",
        "energy",
        "energy_time",
        "server_id",
        "destiny",
        "fc",
        "fight_force_max",
    ] {
        if map.get(num_field).and_then(Amf3Value::as_f64).is_none() {
            errs.push(LuaSchemaViolation::new(
                method,
                format!("ret.{}", num_field),
                format!("UserData:parseData requires numeric `{}` (arithmetic / db lookup without nil fallback)", num_field),
            ));
        }
    }

    if !matches!(map.get("utid").and_then(Amf3Value::as_i64), Some(1 | 2)) {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret.utid",
            "UserData protagonist `utid` must be 1 (male) or 2 (female) for `heroReader` lookup",
        ));
    }

    if map
        .get("copper")
        .map(|v| !v.is_lua_tonumber_compatible())
        .unwrap_or(true)
    {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret.copper",
            "UserData calls `tonumber(data.copper)`; must be number or numeric string",
        ));
    }

    match map.get("uname").and_then(Amf3Value::as_str) {
        Some(s) if !s.is_empty() => {}
        _ => errs.push(LuaSchemaViolation::new(
            method,
            "ret.uname",
            "UserData requires non-empty string `uname`",
        )),
    }

    match map.get("pid") {
        Some(v) if v.as_str().is_some() || v.as_i64().is_some() => {}
        _ => errs.push(LuaSchemaViolation::new(
            method,
            "ret.pid",
            "LoginProxy:dataAvialable reads `g.mainData.userdata.pid`",
        )),
    }

    match map.get("server_open_time").and_then(Amf3Value::as_str) {
        Some(s) if s.len() == 14 && s.bytes().all(|b| b.is_ascii_digit()) => {}
        _ => errs.push(LuaSchemaViolation::new(
            method,
            "ret.server_open_time",
            "MainProxy:ctor parses `string.sub(server_open_time, 3, 14)` via `TimeUtil:getSecondByString`; MUST be 14-digit 'YYYYMMDDHHMMSS' string!",
        )),
    }

    for tbl_field in ["dress_info", "dress_expire", "dress_hero_info", "va_avatar"] {
        if !map
            .get(tbl_field)
            .map(Amf3Value::is_lua_table)
            .unwrap_or(false)
        {
            errs.push(LuaSchemaViolation::new(
                method,
                format!("ret.{}", tbl_field),
                format!("UserData requires table `{}`", tbl_field),
            ));
        }
    }
}

fn validate_bag_info(method: &str, ret: &Amf3Value, errs: &mut Vec<LuaSchemaViolation>) {
    let Some(map) = ret.as_map() else {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret",
            "BagData:parseData requires string-keyed AMF3 Object map",
        ));
        return;
    };

    if !map
        .get("daily_use")
        .map(Amf3Value::is_lua_table)
        .unwrap_or(false)
    {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret.daily_use",
            "BagData:parseData calls `pairs(data.daily_use)` without a nil guard",
        ));
    }

    match map.get("gridStart").and_then(Amf3Value::as_map) {
        Some(gs) => {
            for &(key, _) in &BAG_GRID_START_DEFAULTS[..9] {
                if gs.get(key).and_then(Amf3Value::as_i64).is_none() {
                    errs.push(LuaSchemaViolation::new(
                        method,
                        format!("ret.gridStart.{}", key),
                        "BagData.lua:421-446 reads first 9 `gridStart` keys without `or` default",
                    ));
                }
            }
        }
        None => errs.push(LuaSchemaViolation::new(
            method,
            "ret.gridStart",
            "BagData:parseData indexes `data.gridStart` directly",
        )),
    }

    if map.get("gridMaxNum").and_then(Amf3Value::as_map).is_none() {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret.gridMaxNum",
            "BagData:parseData indexes `data.gridMaxNum` directly",
        ));
    }

    match map.get("gridOpenTimes").and_then(Amf3Value::as_map) {
        Some(go) => {
            for &(key, _) in &BAG_GRID_OPEN_TIMES_DEFAULTS[..4] {
                if go.get(key).and_then(Amf3Value::as_i64).is_none() {
                    errs.push(LuaSchemaViolation::new(
                        method,
                        format!("ret.gridOpenTimes.{}", key),
                        "BagData.lua:449-452 reads arm/treasure/awake/spirit `gridOpenTimes` without `or` default",
                    ));
                }
            }
        }
        None => errs.push(LuaSchemaViolation::new(
            method,
            "ret.gridOpenTimes",
            "BagData:parseData indexes `data.gridOpenTimes` directly",
        )),
    }

    for &sub in BAG_SUBBAG_KEYS {
        if !map.get(sub).map(Amf3Value::is_lua_table).unwrap_or(false) {
            errs.push(LuaSchemaViolation::new(
                method,
                format!("ret.{}", sub),
                format!("BagData expects sub-bag table `{}`", sub),
            ));
        }
    }
}

fn validate_position_info(method: &str, ret: &Amf3Value, errs: &mut Vec<LuaSchemaViolation>) {
    let Some(map) = ret.as_map() else {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret",
            "HeroFormationData:parseData requires string-keyed AMF3 Object map",
        ));
        return;
    };

    let major_hid = match map.get("position") {
        Some(Amf3Value::Map(pos_map))
            if !pos_map.is_empty()
                && pos_map
                    .values()
                    .all(|v| v.as_i64().is_some_and(|hid| hid > 0)) =>
        {
            pos_map.get("1").and_then(Amf3Value::as_i64).unwrap_or(0)
        }
        Some(Amf3Value::Array(pos))
            if !pos.is_empty() && pos.len() <= 6 && pos.iter().all(|v| v.as_i64().is_some()) =>
        {
            pos[0].as_i64().unwrap_or(0)
        }
        _ => {
            errs.push(LuaSchemaViolation::new(
                method,
                "ret.position",
                "HeroFormationData requires non-empty occupied `position` map or array",
            ));
            0
        }
    };

    match map.get("allHeros").and_then(Amf3Value::as_map) {
        Some(heros) if !heros.is_empty() => {
            if major_hid > 0 && !heros.contains_key(&major_hid.to_string()) {
                errs.push(LuaSchemaViolation::new(
                    method,
                    format!("ret.allHeros['{}']", major_hid),
                    "HeroFormationData looks up deployed hid via `tostring(hid)` in `allHeros`",
                ));
            }
            for (hid_key, hero_val) in heros {
                let Some(hm) = hero_val.as_map() else {
                    errs.push(LuaSchemaViolation::new(
                        method,
                        format!("ret.allHeros['{}']", hid_key),
                        "each hero entry must be an AMF3 Object map",
                    ));
                    continue;
                };
                if !hm
                    .get("va_fail_reward")
                    .map(Amf3Value::is_lua_table)
                    .unwrap_or(false)
                {
                    errs.push(LuaSchemaViolation::new(
                        method,
                        format!("ret.allHeros['{}'].va_fail_reward", hid_key),
                        "HeroInfo:cloneTable calls `pairs(va_fail_reward)` without nil guard!",
                    ));
                }
                for req_num in ["hid", "htid", "level", "evolve_level"] {
                    if hm.get(req_num).and_then(Amf3Value::as_i64).is_none() {
                        errs.push(LuaSchemaViolation::new(
                            method,
                            format!("ret.allHeros['{}'].{}", hid_key, req_num),
                            format!("HeroInfo requires integer `{}`", req_num),
                        ));
                    }
                }
            }
        }
        _ => errs.push(LuaSchemaViolation::new(
            method,
            "ret.allHeros",
            "HeroFormationData requires non-empty string-keyed `allHeros` map (1-based array breaks `tostring(hid)` lookup!)",
        )),
    }

    if !map
        .get("bingfu")
        .map(Amf3Value::is_lua_table)
        .unwrap_or(false)
    {
        errs.push(LuaSchemaViolation::new(
            method,
            "ret.bingfu",
            "HeroFormationData:initBingFu calls `pairs(data.bingfu)` without nil guard!",
        ));
    }

    for slot_arr in ["arming", "spirit", "yanWu", "brochure", "soulSkill"] {
        match map.get(slot_arr).and_then(Amf3Value::as_array) {
            Some(a) if a.len() == 6 => {}
            _ => errs.push(LuaSchemaViolation::new(
                method,
                format!("ret.{}", slot_arr),
                format!(
                    "HeroFormationData requires 6-formation-slot array `{}`",
                    slot_arr
                ),
            )),
        }
    }

    match map.get("formationBook").and_then(Amf3Value::as_map) {
        Some(fb)
            if fb.get("cur_use").and_then(Amf3Value::as_i64).is_some()
                && fb
                    .get("lv_info")
                    .map(Amf3Value::is_lua_table)
                    .unwrap_or(false) => {}
        _ => errs.push(LuaSchemaViolation::new(
            method,
            "ret.formationBook",
            "FormationBookInfo requires `{ cur_use:int, lv_info:table, reset_num:int }`",
        )),
    }
}
