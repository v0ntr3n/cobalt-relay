//! Complete TCP Connect -> `MainTown` (100% `MainLoading`) bootstrap RPC sequence, typed
//! response builders, and stateful [`BootstrapSession`] server simulator.

use std::collections::BTreeMap;

use crate::amf3::Amf3Value;
use crate::envelope::{RpcRequestEnvelope, RpcResponseEnvelope};
use crate::net_const;

/// High-level phase in the GServer (`vnsg2`) login-to-`MainTown` bootstrap pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BootstrapPhase {
    /// Stage A: TCP connect, `user.login`, `user.getUserBeforLogin`, optional `CreateUserScene`
    /// (`user.getRandomName` -> `user.stat` -> `user.createUser` -> `user.getUserBeforLogin`),
    /// and `user.userLogin`.
    StageAAuthAndRole,
    /// Stage B: `LoginInitDataProxy:checkDataAvialable` (`user.getUser`, `switchfunc.getSwitch`,
    /// `user.getVaConfig`, `formation.get`) + `FestivalManager:initDb` (`allActivity.getActivityInfo`).
    StageBLoginInitAndFestival,
    /// Stage C: `GuideManager:run()` -> `CopyProxy:checkDataAvialable` (`bag.bagInfo`,
    /// `formation.getPositionInfo`, `halidom.getInfo`, `copy.getList`) -> `GuideManager:_dataAviable`
    /// (`copy.getCopy` for chapter `1001`).
    StageCGuideAndCopyInit,
    /// Stage D: `MainScene:run()` -> `MainProxy:checkDataAvialable` -> `EnChatProxy:initData` ->
    /// `EnCampaignProxy` -> `ActSevenDayProxy` / `ActMoreGoldProxy` -> `MainTownBarrageProxy`
    /// (`barrage.get`, `barrage.enterRoom` -> `MainScene:init()` -> `MainTown:new()` +
    /// `MainLoading:hide()` 100% -> `barrage.getBarrageRecord`).
    StageDMainSceneToMainTown100,
    /// Stage E: Asynchronous post-`MainTown` background queries fired inside `MainScene:init()`
    /// (`lordwar.getTempleInfo`, `crossClanBattle.getTempleInfo`, `crossClanBattle.getPromotionInfo`,
    /// and 1-second timer `activityAnswer.getInfo`).
    StageEPostMainTownBackground,
}

/// Static specification entry for a single RPC in the bootstrap sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapRpcSpec {
    pub step_order: u16,
    pub phase: BootstrapPhase,
    /// Verbatim `NetConst.lua` constant name, or `"<inline>"` when sent as a raw string literal.
    pub net_const_name: &'static str,
    /// Verbatim wire `method` string.
    pub method: &'static str,
    /// Whether this RPC is unconditionally sent on every existing-role login (`true`) or conditional
    /// (new-account role creation or `openSwitchData`-gated module).
    pub unconditional_on_login: bool,
    /// Originating Lua module / function.
    pub lua_caller: &'static str,
    /// Exact Lua `args` array layout.
    pub request_args_schema: &'static str,
    /// Exact Lua `ret` payload schema and critical nil/type invariants.
    pub response_ret_schema: &'static str,
}

/// Complete ordered bootstrap RPC specification table from TCP connect through `MainTown` 100%.
pub const BOOTSTRAP_SEQUENCE: &[BootstrapRpcSpec] = &[
    BootstrapRpcSpec {
        step_order: 1,
        phase: BootstrapPhase::StageAAuthAndRole,
        net_const_name: "CS_USER_LOGIN",
        method: net_const::CS_USER_LOGIN,
        unconditional_on_login: true,
        lua_caller: "LoginProxy:loginGame -> onUserLogin (LoginProxy.lua:971-1008)",
        request_args_schema: "[ loginHashTable{AMF=3, hash, pid, qid, serverID='game<gid>', timestamp, offset, openDateTime, plname, plosgn, canCreateUser=1, canPay=1, mergeServerTime, uuid, record={}}, uuid:str, scrVersion:str ]",
        response_ret_schema: "\"ok\" (err=='ok' required; clears g.timestamp and calls getUserBeforLogin())",
    },
    BootstrapRpcSpec {
        step_order: 2,
        phase: BootstrapPhase::StageAAuthAndRole,
        net_const_name: "CS_GET_USER_BEFRO_LOGIN",
        method: net_const::CS_GET_USER_BEFRO_LOGIN,
        unconditional_on_login: true,
        lua_caller: "LoginProxy:getUserBeforLogin -> onGetUserBeforLogin (LoginProxy.lua:1010-1035)",
        request_args_schema: "[]",
        response_ret_schema: "New account: {} (ret.uid == nil -> skips user.beforLoginBattle in vnsg2 CopyProxy.lua:1305 and enters CreateUserScene). Existing/created role: Map { uid:int, uname:str, utid:int(1|2), level:int } (MUST have top-level numeric uid!)",
    },
    BootstrapRpcSpec {
        step_order: 3,
        phase: BootstrapPhase::StageAAuthAndRole,
        net_const_name: "CS_USER_GET_RANDOM_NAME",
        method: net_const::CS_USER_GET_RANDOM_NAME,
        unconditional_on_login: false,
        lua_caller: "CreateUserScene:ctor -> CreateUserProxy:getRandomName (CreateUserScene.lua:102, 376-385)",
        request_args_schema: "[ count:int(<=20, default 20), gender:int(1=male, 0=female) ]",
        response_ret_schema: "1-based Array [ { name:str }, ... ] (CreateUserScene indexes names[#names]['name'])",
    },
    BootstrapRpcSpec {
        step_order: 4,
        phase: BootstrapPhase::StageAAuthAndRole,
        net_const_name: "<inline>",
        method: "user.stat",
        unconditional_on_login: false,
        lua_caller: "Stat:sendStat (Stat.lua / CreateUserScene.lua POINT_TIAOGUODONGHUA & POINT_ENTERGAME)",
        request_args_schema: "[ statPointId:int ] (callbackName='user.stat' without _mId suffix)",
        response_ret_schema: "\"ok\" (fire-and-forget telemetry ack)",
    },
    BootstrapRpcSpec {
        step_order: 5,
        phase: BootstrapPhase::StageAAuthAndRole,
        net_const_name: "CS_USER_CREATEUSER",
        method: net_const::CS_USER_CREATEUSER,
        unconditional_on_login: false,
        lua_caller: "CreateUserScene:_onEnterTap -> CreateUserProxy:createUser (CreateUserScene.lua:319-360)",
        request_args_schema: "[ utid:int(1=male, 2=female), uname:str(1..=15 UTF-8 chars) ]",
        response_ret_schema: "\"ok\" (triggers second user.getUserBeforLogin() which now returns {uid, uname, utid, level})",
    },
    BootstrapRpcSpec {
        step_order: 6,
        phase: BootstrapPhase::StageAAuthAndRole,
        net_const_name: "CS_USER_USERLOGIN",
        method: net_const::CS_USER_USERLOGIN,
        unconditional_on_login: true,
        lua_caller: "LoginProxy:loginUid -> onLoginUid (LoginProxy.lua:1058-1095)",
        request_args_schema: "[ uid:int ]",
        response_ret_schema: "Literal string \"ok\" (\"fail\" raises error alert; \"ban:<unix_ts>:<reason>\" shows ban dialog)",
    },
    BootstrapRpcSpec {
        step_order: 7,
        phase: BootstrapPhase::StageBLoginInitAndFestival,
        net_const_name: "CS_GET_USER",
        method: net_const::CS_GET_USER,
        unconditional_on_login: true,
        lua_caller: "LoginInitDataProxy -> UserData:parseData (LoginProxy.lua:1098, UserData.lua:177-390)",
        request_args_schema: "[]",
        response_ret_schema: "Map { uid:int, pid:str|int, uname:str, utid:int(1|2), level:int, vip:int, exp:int, copper:int|str, gold:int, gold_bind:int, gold_recharge_accum_all:int, execution:int, execution_time:int, buy_execution_accum:int, buy_execution_time:int, energy:int, energy_time:int, server_open_time:str('YYYYMMDDHHMMSS' 14 chars!), server_id:int, group_id:str, destiny:int(-1), fight_force_max:int, dress_info:{}, dress_expire:{}, dress_hero_info:{}, va_avatar:{}, ... }",
    },
    BootstrapRpcSpec {
        step_order: 8,
        phase: BootstrapPhase::StageBLoginInitAndFestival,
        net_const_name: "CS_OPENSWITCH_GETSWITCH",
        method: net_const::CS_OPENSWITCH_GETSWITCH,
        unconditional_on_login: true,
        lua_caller: "LoginInitDataProxy -> OpenSwitchData:parseData (OpenSwitchData.lua:53-76)",
        request_args_schema: "[]",
        response_ret_schema: "1-based Array [ open_node_id:int, ... ] (initializes self.dicAlreadyOpen={} required by LuaAssert across all proxies!)",
    },
    BootstrapRpcSpec {
        step_order: 9,
        phase: BootstrapPhase::StageBLoginInitAndFestival,
        net_const_name: "CS_USER_GET_VA_CONFIG",
        method: net_const::CS_USER_GET_VA_CONFIG,
        unconditional_on_login: true,
        lua_caller: "LoginInitDataProxy -> ConfigData:parseData (ConfigData.lua:39-84, GuideManager.lua:159-250)",
        request_args_schema: "[]",
        response_ret_schema: "Map { guide_id:int(240=GuideManager.END_ID, or 360 -> nextLeadId=240 per live_capture_filtered.log), guide_once_ids:int[], is_first_fighten:bool, opened_copy_chapters:int[] }",
    },
    BootstrapRpcSpec {
        step_order: 10,
        phase: BootstrapPhase::StageBLoginInitAndFestival,
        net_const_name: "CS_FORMATION_GET",
        method: net_const::CS_FORMATION_GET,
        unconditional_on_login: true,
        lua_caller: "LoginInitDataProxy -> FormationData:parseData (FormationData.lua:33-52)",
        request_args_schema: "[]",
        response_ret_schema: "Map { formation: 1-based Array [ major_hid:int, 0, 0, 0, 0, 0 ] }",
    },
    BootstrapRpcSpec {
        step_order: 11,
        phase: BootstrapPhase::StageBLoginInitAndFestival,
        net_const_name: "CS_ALL_ACTIVITY_GETACTIVITYINFO",
        method: net_const::CS_ALL_ACTIVITY_GETACTIVITYINFO,
        unconditional_on_login: true,
        lua_caller: "LoginProxy:dataAvialable -> FestivalManager:initDb / ActivityData:parseData (LoginProxy.lua:1109, ActivityData.lua)",
        request_args_schema: "[]",
        response_ret_schema: "Map {} (or active festival CSV map; empty {} logs '<name>的csvdata是空的' and advances to GuideManager:run())",
    },
    BootstrapRpcSpec {
        step_order: 12,
        phase: BootstrapPhase::StageCGuideAndCopyInit,
        net_const_name: "CS_GET_BAG_DATA",
        method: net_const::CS_GET_BAG_DATA,
        unconditional_on_login: true,
        lua_caller: "GuideManager:run -> CopyProxy:essentialData -> BagData:parseData (BagData.lua:412-565)",
        request_args_schema: "[ {} ] (1-element array wrapping an empty table)",
        response_ret_schema: "Map { daily_use:{}, gridStart:{item,arm,treasure,fragment,soul,awake,awake_fragment,treasure_fragment,spirit,...}, gridMaxNum:{...}, gridOpenTimes:{arm,treasure,awake,spirit,...}, plus 41 BagConsts sub-bag maps={} }",
    },
    BootstrapRpcSpec {
        step_order: 13,
        phase: BootstrapPhase::StageCGuideAndCopyInit,
        net_const_name: "CS_FORMATION_GET_POSITION_INFO",
        method: net_const::CS_FORMATION_GET_POSITION_INFO,
        unconditional_on_login: true,
        lua_caller: "CopyProxy:essentialData -> HeroFormationData:parseData (HeroFormationData.lua:152-330, HeroInfo.lua:108-290)",
        request_args_schema: "[]",
        response_ret_schema: "Map { position:[hid,0,0,0,0,0], partner:{}, buddy:{}, buddyLevelInfo:{}, godanimal:{}, topanimal:{}, protection:{}, allHeros:{ [tostring(hid)]: HeroDto{va_fail_reward={}, ...} }, jointSkill:{}, arming:[{}x6], spirit:[[{}x6]x6], yanWu:[[{}x4]x6], brochure:[[{}x6]x6], soulSkill:[[{}x6]x6], bingfu:{}, bingshu:{}, jobSkill:{}, arrCounsellor:{}, attrSkins:{}, morePowerOpenCount:0, formationBook:{cur_use:0, lv_info:{}, reset_num:0} }",
    },
    BootstrapRpcSpec {
        step_order: 14,
        phase: BootstrapPhase::StageCGuideAndCopyInit,
        net_const_name: "<inline>",
        method: "halidom.getInfo",
        unconditional_on_login: true,
        lua_caller: "CopyProxy:essentialData -> HalidomData:parseData (HalidomData.lua:44-68)",
        request_args_schema: "[]",
        response_ret_schema: "Map {} (HalidomData checks `if next(data) then ... end`, so empty {} safely skips)",
    },
    BootstrapRpcSpec {
        step_order: 15,
        phase: BootstrapPhase::StageCGuideAndCopyInit,
        net_const_name: "CS_COPY_LIST",
        method: net_const::CS_COPY_LIST,
        unconditional_on_login: true,
        lua_caller: "CopyProxy:essentialData -> CopyData:parseData (CopyData.lua:42-78)",
        request_args_schema: "[ 1 ] (COPY_TYPE_1 normal chapters)",
        response_ret_schema: "Map { fresherRewardIndex:int(0), copyData:{ '1001': { star:3, prize:0, pass_strongh:1, box_reward_num:0 } } } (fresherRewardIndex + 1 requires non-nil number!)",
    },
    BootstrapRpcSpec {
        step_order: 16,
        phase: BootstrapPhase::StageCGuideAndCopyInit,
        net_const_name: "CS_COPY_CHAPTERINFO",
        method: net_const::CS_COPY_CHAPTERINFO,
        unconditional_on_login: true,
        lua_caller: "GuideManager:_dataAviable -> CopyInfoDataController:requestData (GuideManager.lua:204-219, MainScene.lua:568,1111)",
        request_args_schema: "[ 1001 ] (chapter 1001)",
        response_ret_schema: "Map { va_copy: { '100101': { reset:0, normal_star:3, star_status:0, pass:1, box_reward:0 } } } (normal_star >= 1 ensures isFirstFight(100101)==false so MainLoading reaches 100% and enters MainTown!)",
    },
    BootstrapRpcSpec {
        step_order: 17,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "CS_SMALLGAME_GETINFO",
        method: net_const::CS_SMALLGAME_GETINFO,
        unconditional_on_login: true,
        lua_caller: "MainScene:run -> MainProxy:essentialData -> EnterViewData:parseData (EnterViewData.lua:27-68)",
        request_args_schema: "[]",
        response_ret_schema: "Map { adopt_list: { '1':1, '2':1 } } (adopt_list MUST be a table; marking '1' & '2' adopted prevents enterViewMainPanel hijack)",
    },
    BootstrapRpcSpec {
        step_order: 18,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "CS_REWARD_GETREWARDLIST",
        method: net_const::CS_REWARD_GETREWARDLIST,
        unconditional_on_login: true,
        lua_caller: "MainScene:run -> MainProxy:essentialData -> RewardData:parseData (RewardData.lua:34-65)",
        request_args_schema: "[ 0, 100 ]",
        response_ret_schema: "1-based Array [] (or reward entries list)",
    },
    BootstrapRpcSpec {
        step_order: 19,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "CS_CHAT_WORLD_GET",
        method: net_const::CS_CHAT_WORLD_GET,
        unconditional_on_login: true,
        lua_caller: "MainScene:run -> EnChatProxy:initData (ChatProxy.lua:55-96)",
        request_args_schema: "[]",
        response_ret_schema: "1-based Array [] (list of compressed chat ByteArrays; empty [] initializes _worldData)",
    },
    BootstrapRpcSpec {
        step_order: 20,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "CS_CHAT_GUILD_GET",
        method: net_const::CS_CHAT_GUILD_GET,
        unconditional_on_login: true,
        lua_caller: "MainScene:run -> EnChatProxy:initData (ChatProxy.lua:97-130)",
        request_args_schema: "[]",
        response_ret_schema: "1-based Array [] (empty [] initializes _guildData and invokes EnCampaignProxy callback)",
    },
    BootstrapRpcSpec {
        step_order: 21,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "CS_ACTIVITY_OPEN_CARNIVAL_GET_OPEN_ACHIEVE",
        method: net_const::CS_ACTIVITY_OPEN_CARNIVAL_GET_OPEN_ACHIEVE,
        unconditional_on_login: false,
        lua_caller: "MainScene:checkSevenDayState -> ActSevenDayProxy:checkDataAvialable (MainScene.lua:248-263)",
        request_args_schema: "[] (sent when getOpenServerDay() <= reward_end_time)",
        response_ret_schema: "Map {} (seven-day carnival achievement map)",
    },
    BootstrapRpcSpec {
        step_order: 22,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "<inline>",
        method: "barrage.get",
        unconditional_on_login: true,
        lua_caller: "MainScene:initBarrage -> MainTownBarrageProxy:getMainTownBarrageState -> BarrageProxy:getState (MainScene.lua:276, BarrageProxy.lua:324-339)",
        request_args_schema: "[]",
        response_ret_schema: "Map { is_open:bool, send_time:int }",
    },
    BootstrapRpcSpec {
        step_order: 23,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "<inline>",
        method: "barrage.enterRoom",
        unconditional_on_login: true,
        lua_caller: "MainScene:initBarrage -> MainTownBarrageProxy:enterRoom (MainScene.lua:277, BarrageProxy.lua:370-393)",
        request_args_schema: "[ roomId:int ]",
        response_ret_schema: "Bool true / \"ok\" (callback invokes MainScene:init() -> MainTown:new() -> MainLoading:hide() sets progress bar to 100%!)",
    },
    BootstrapRpcSpec {
        step_order: 24,
        phase: BootstrapPhase::StageDMainSceneToMainTown100,
        net_const_name: "<inline>",
        method: "barrage.getBarrageRecord",
        unconditional_on_login: true,
        lua_caller: "BarrageProxy:onEnterRoom -> getBarrageRecord (BarrageProxy.lua:53-71, 392)",
        request_args_schema: "[ roomId_or_1:int ]",
        response_ret_schema: "1-based Array [] (iterated via `for _, data in pairs(ret)` — MUST be non-nil table!)",
    },
    BootstrapRpcSpec {
        step_order: 25,
        phase: BootstrapPhase::StageEPostMainTownBackground,
        net_const_name: "CS_LORDWAR_GET_TOP",
        method: net_const::CS_LORDWAR_GET_TOP,
        unconditional_on_login: true,
        lua_caller: "MainScene:init -> worldSoloData:sendGetTopUser (MainScene.lua:436)",
        request_args_schema: "[]",
        response_ret_schema: "Map {} (top solo temple snapshot)",
    },
    BootstrapRpcSpec {
        step_order: 26,
        phase: BootstrapPhase::StageEPostMainTownBackground,
        net_const_name: "CS_CROSSCLANBATTLE_GET_TOP",
        method: net_const::CS_CROSSCLANBATTLE_GET_TOP,
        unconditional_on_login: true,
        lua_caller: "MainScene:init -> legionCrossData:sendGetTopUser (MainScene.lua:451)",
        request_args_schema: "[]",
        response_ret_schema: "Map {} (cross-clan temple snapshot)",
    },
    BootstrapRpcSpec {
        step_order: 27,
        phase: BootstrapPhase::StageEPostMainTownBackground,
        net_const_name: "CS_CROSSCLANBATTLE_ROUNDINFO",
        method: net_const::CS_CROSSCLANBATTLE_ROUNDINFO,
        unconditional_on_login: true,
        lua_caller: "MainScene:init -> legionCrossData:sendGetPromotionInfo (MainScene.lua:452)",
        request_args_schema: "[]",
        response_ret_schema: "Map {} (cross-clan promotion snapshot)",
    },
    BootstrapRpcSpec {
        step_order: 28,
        phase: BootstrapPhase::StageEPostMainTownBackground,
        net_const_name: "CS_MILLIONQUIZ_GETINFO",
        method: net_const::CS_MILLIONQUIZ_GETINFO,
        unconditional_on_login: true,
        lua_caller: "MainScene:init 1s timer -> millionQuizData (MainScene.lua:455-461)",
        request_args_schema: "[]",
        response_ret_schema: "Map {} (million-quiz state)",
    },
];

/// All 41 sub-bag table keys initialized in `BagData:parseData` (`BagConsts.lua` + `BagData.lua`).
pub const BAG_SUBBAG_KEYS: &[&str] = &[
    "item",
    "arm",
    "treasure",
    "fragment",
    "soul",
    "awake",
    "awake_fragment",
    "treasure_fragment",
    "spirit",
    "pet",
    "pet_fragment",
    "god_arm",
    "god_arm_fragment",
    "god_animal",
    "god_animal_fragment",
    "top_animal",
    "top_animal_fragment",
    "hero_legend",
    "hero_legend_fragment",
    "brochure",
    "brochure_fragment",
    "halidom_fragment",
    "bingfu_fragment",
    "gem",
    "soul_skill",
    "statue_fragment",
    "bingshu_fragment",
    "jobskill_fragment",
    "yanwu",
    "yanwu_fragment",
    "yanwu_soul",
    "counsellor",
    "counsellor_fragment",
    "counsellor_equip",
    "counsellor_equip_fragment",
    "soul_stone",
    "soul_stone_fragment",
    "soul_jade",
    "soul_jade_fragment",
    "ploy_fragment",
    "great_seal_fragment",
];

/// Required `gridStart` keys in `BagData:parseData` (first 9 have NO `or` fallback in Lua).
pub const BAG_GRID_START_DEFAULTS: &[(&str, i64)] = &[
    ("item", 10_001),
    ("arm", 20_001),
    ("treasure", 30_001),
    ("fragment", 40_001),
    ("soul", 50_001),
    ("awake", 60_001),
    ("awake_fragment", 70_001),
    ("treasure_fragment", 80_001),
    ("spirit", 90_001),
    ("pet", 100_001),
    ("pet_fragment", 200_001),
    ("god_arm", 210_001),
    ("god_arm_fragment", 220_001),
    ("halidom_fragment", 230_001),
    ("gem", 240_001),
    ("god_animal", 310_001),
    ("god_animal_fragment", 320_001),
    ("hero_legend", 420_001),
    ("hero_legend_fragment", 420_001),
    ("brochure", 510_001),
    ("brochure_fragment", 520_001),
    ("bingfu_fragment", 530_001),
    ("soul_skill", 540_001),
    ("yanwu", 580_001),
    ("yanwu_fragment", 590_001),
    ("yanwu_soul", 600_001),
    ("counsellor_equip", 630_001),
    ("top_animal", 710_001),
    ("top_animal_fragment", 720_001),
];

/// Default `gridMaxNum` capacities in `BagData:parseData`.
pub const BAG_GRID_MAX_DEFAULTS: &[(&str, i64)] = &[
    ("item", 10_000),
    ("arm", 200),
    ("treasure", 200),
    ("fragment", 10_000),
    ("soul", 10_000),
    ("awake", 200),
    ("awake_fragment", 10_000),
    ("treasure_fragment", 10_000),
    ("spirit", 200),
    ("pet", 200),
    ("pet_fragment", 10_000),
    ("god_arm", 200),
    ("god_arm_fragment", 10_000),
    ("halidom_fragment", 10_000),
    ("gem", 1_000),
    ("god_animal", 200),
    ("god_animal_fragment", 10_000),
    ("hero_legend", 500),
    ("hero_legend_fragment", 10_000),
    ("brochure", 500),
    ("brochure_fragment", 10_000),
    ("bingfu_fragment", 10_000),
    ("soul_skill", 500),
    ("yanwu", 500),
    ("yanwu_fragment", 10_000),
    ("yanwu_soul", 500),
    ("counsellor_equip", 500),
    ("top_animal", 200),
    ("top_animal_fragment", 10_000),
];

/// Required `gridOpenTimes` keys in `BagData:parseData` (first 4 have NO `or` fallback in Lua).
pub const BAG_GRID_OPEN_TIMES_DEFAULTS: &[(&str, i64)] = &[
    ("arm", 0),
    ("treasure", 0),
    ("awake", 0),
    ("spirit", 0),
    ("pet", 0),
    ("god_arm", 0),
    ("god_animal", 0),
    ("yanwu", 0),
    ("yanwu_soul", 0),
    ("gem", 0),
    ("counsellor_equip", 0),
    ("top_animal", 0),
];

/// Typed builder for `user.getUser` (`UserData:parseData`).
#[derive(Debug, Clone, PartialEq)]
pub struct UserProfile {
    pub uid: i64,
    pub pid: String,
    pub uname: String,
    /// `1` = male protagonist (`heroReader` ID `1`), `2` = female protagonist (`heroReader` ID `2`).
    pub utid: i64,
    pub level: i64,
    pub vip: i64,
    pub vip_exp: i64,
    pub exp: i64,
    pub copper: i64,
    pub gold: i64,
    pub gold_bind: i64,
    pub gold_recharge_accum_all: i64,
    pub is_recharge: i64,
    pub execution: i64,
    pub execution_time: i64,
    pub buy_execution_accum: i64,
    pub buy_execution_time: i64,
    pub energy: i64,
    pub energy_time: i64,
    /// MUST be a 14-character `"YYYYMMDDHHMMSS"` string (`MainProxy:ctor` slices `string.sub(..., 3, 14)`).
    pub server_open_time: String,
    pub server_id: i64,
    pub group_id: String,
    pub login_time: i64,
    pub last_login_time: i64,
    pub online_accum_time: i64,
    /// `-1` when no destiny node is unlocked yet.
    pub destiny: i64,
    pub destiny_star: i64,
    pub fc: i64,
    pub fight_force_max: i64,
    pub major_hid: i64,
}

impl Default for UserProfile {
    fn default() -> Self {
        let now = 1_795_496_852_i64;
        Self {
            uid: 100_001,
            pid: "636df6782106606d".to_owned(),
            uname: "ChúaCôngVN".to_owned(),
            utid: 1,
            level: 10,
            vip: 0,
            vip_exp: 0,
            exp: 0,
            copper: 100_000,
            gold: 500,
            gold_bind: 0,
            gold_recharge_accum_all: 0,
            is_recharge: 0,
            execution: 120,
            execution_time: now,
            buy_execution_accum: 0,
            buy_execution_time: now,
            energy: 30,
            energy_time: now,
            server_open_time: "20260101000000".to_owned(),
            server_id: 1,
            group_id: "game1".to_owned(),
            login_time: now,
            last_login_time: now,
            online_accum_time: 0,
            destiny: -1,
            destiny_star: 0,
            fc: 12_500,
            fight_force_max: 12_500,
            major_hid: 10_000_001,
        }
    }
}

impl UserProfile {
    /// Build the pre-login summary map returned by `user.getUserBeforLogin` when the role exists.
    pub fn to_before_login_amf3(&self) -> Amf3Value {
        Amf3Value::map_from([
            ("uid", Amf3Value::Int(self.uid)),
            ("uname", Amf3Value::String(self.uname.clone())),
            ("utid", Amf3Value::Int(self.utid)),
            ("level", Amf3Value::Int(self.level)),
            ("vip", Amf3Value::Int(self.vip)),
            ("dress_id", Amf3Value::Int(0)),
        ])
    }

    /// Build the full `user.getUser` dictionary expected by `UserData:parseData`.
    pub fn to_get_user_amf3(&self) -> Amf3Value {
        let mut m = BTreeMap::new();
        m.insert("uid".to_owned(), Amf3Value::Int(self.uid));
        m.insert("pid".to_owned(), Amf3Value::String(self.pid.clone()));
        m.insert("uname".to_owned(), Amf3Value::String(self.uname.clone()));
        m.insert("utid".to_owned(), Amf3Value::Int(self.utid));
        m.insert("level".to_owned(), Amf3Value::Int(self.level));
        m.insert("vip".to_owned(), Amf3Value::Int(self.vip));
        m.insert("vip_exp".to_owned(), Amf3Value::Int(self.vip_exp));
        m.insert("exp".to_owned(), Amf3Value::Int(self.exp));
        m.insert("copper".to_owned(), Amf3Value::Int(self.copper));
        m.insert("gold".to_owned(), Amf3Value::Int(self.gold));
        m.insert("gold_bind".to_owned(), Amf3Value::Int(self.gold_bind));
        m.insert(
            "gold_recharge_accum_all".to_owned(),
            Amf3Value::Int(self.gold_recharge_accum_all),
        );
        m.insert("is_recharge".to_owned(), Amf3Value::Int(self.is_recharge));
        m.insert("execution".to_owned(), Amf3Value::Int(self.execution));
        m.insert(
            "execution_time".to_owned(),
            Amf3Value::Int(self.execution_time),
        );
        m.insert(
            "buy_execution_accum".to_owned(),
            Amf3Value::Int(self.buy_execution_accum),
        );
        m.insert(
            "buy_execution_time".to_owned(),
            Amf3Value::Int(self.buy_execution_time),
        );
        m.insert("energy".to_owned(), Amf3Value::Int(self.energy));
        m.insert("energy_time".to_owned(), Amf3Value::Int(self.energy_time));
        m.insert(
            "server_open_time".to_owned(),
            Amf3Value::String(self.server_open_time.clone()),
        );
        m.insert("server_id".to_owned(), Amf3Value::Int(self.server_id));
        m.insert(
            "group_id".to_owned(),
            Amf3Value::String(self.group_id.clone()),
        );
        m.insert("login_time".to_owned(), Amf3Value::Int(self.login_time));
        m.insert(
            "last_login_time".to_owned(),
            Amf3Value::Int(self.last_login_time),
        );
        m.insert(
            "online_accum_time".to_owned(),
            Amf3Value::Int(self.online_accum_time),
        );
        m.insert("destiny".to_owned(), Amf3Value::Int(self.destiny));
        m.insert("destiny_star".to_owned(), Amf3Value::Int(self.destiny_star));
        m.insert("fc".to_owned(), Amf3Value::Int(self.fc));
        m.insert(
            "fight_force_max".to_owned(),
            Amf3Value::Int(self.fight_force_max),
        );
        m.insert("dress_id".to_owned(), Amf3Value::Int(0));
        m.insert("attr_dress_id".to_owned(), Amf3Value::Int(0));
        m.insert("dress_info".to_owned(), Amf3Value::empty_map());
        m.insert("dress_expire".to_owned(), Amf3Value::empty_map());
        m.insert("dress_hero_info".to_owned(), Amf3Value::empty_map());
        m.insert("va_avatar".to_owned(), Amf3Value::empty_map());
        m.insert("avatar_id".to_owned(), Amf3Value::Int(0));
        m.insert("guild_id".to_owned(), Amf3Value::Int(0));

        for coin_key in [
            "prestige",
            "arena_coin",
            "godarm_coin",
            "tower_coin",
            "rebel_coin",
            "hero_soul",
            "pet_coin",
            "awakeArm_coin",
            "lordwar_coin",
            "world_team_coin",
            "world_person_score",
            "voucher_coin",
            "time_machine_voucher_coin",
            "iron_gold_coin",
        ] {
            m.insert(coin_key.to_owned(), Amf3Value::Int(0));
        }

        for close_flag in ["fiend_close", "cross_guild_close", "open_carnival2_close"] {
            m.insert(close_flag.to_owned(), Amf3Value::Bool(false));
        }
        Amf3Value::Map(m)
    }
}

/// Typed builder for `user.getVaConfig` (`ConfigData:parseData` & `GuideManager`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaConfig {
    /// `GuideManager.END_ID = 240` (or `360`, whose `leadStep[360].nextLeadId == 240`, producing
    /// `getCurStepDto===== true 240` as seen in `live_capture_filtered.log`).
    pub guide_id: i64,
    pub guide_once_ids: Vec<i64>,
    pub is_first_fighten: bool,
    pub opened_copy_chapters: Vec<i64>,
}

impl Default for VaConfig {
    fn default() -> Self {
        Self {
            guide_id: 240,
            guide_once_ids: vec![10_001, 10_010, 10_101, 10_201, 10_301, 10_401, 10_501],
            is_first_fighten: true,
            opened_copy_chapters: vec![1001],
        }
    }
}

impl VaConfig {
    pub fn to_amf3(&self) -> Amf3Value {
        Amf3Value::map_from([
            ("guide_id", Amf3Value::Int(self.guide_id)),
            (
                "guide_once_ids",
                Amf3Value::Array(
                    self.guide_once_ids
                        .iter()
                        .copied()
                        .map(Amf3Value::Int)
                        .collect(),
                ),
            ),
            ("is_first_fighten", Amf3Value::Bool(self.is_first_fighten)),
            (
                "opened_copy_chapters",
                Amf3Value::Array(
                    self.opened_copy_chapters
                        .iter()
                        .copied()
                        .map(Amf3Value::Int)
                        .collect(),
                ),
            ),
        ])
    }
}

/// Typed builder for `formation.get` (`FormationData:parseData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormationSquad {
    pub slots: [i64; 6],
}

impl FormationSquad {
    pub fn with_major_hero(major_hid: i64) -> Self {
        Self {
            slots: [major_hid, 0, 0, 0, 0, 0],
        }
    }

    pub fn to_amf3(&self) -> Amf3Value {
        Amf3Value::map_from([(
            "formation",
            Amf3Value::Array(self.slots.iter().copied().map(Amf3Value::Int).collect()),
        )])
    }
}

/// Typed builder for `bag.bagInfo` (`BagData:parseData`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BagSnapshot {
    pub items: BTreeMap<String, Amf3Value>,
}

impl BagSnapshot {
    pub fn to_amf3(&self) -> Amf3Value {
        let mut root = BTreeMap::new();
        root.insert("daily_use".to_owned(), Amf3Value::empty_map());

        let mut grid_start = BTreeMap::new();
        for &(k, v) in BAG_GRID_START_DEFAULTS {
            grid_start.insert(k.to_owned(), Amf3Value::Int(v));
        }
        root.insert("gridStart".to_owned(), Amf3Value::Map(grid_start));

        let mut grid_max = BTreeMap::new();
        for &(k, v) in BAG_GRID_MAX_DEFAULTS {
            grid_max.insert(k.to_owned(), Amf3Value::Int(v));
        }
        root.insert("gridMaxNum".to_owned(), Amf3Value::Map(grid_max));

        let mut grid_open = BTreeMap::new();
        for &(k, v) in BAG_GRID_OPEN_TIMES_DEFAULTS {
            grid_open.insert(k.to_owned(), Amf3Value::Int(v));
        }
        root.insert("gridOpenTimes".to_owned(), Amf3Value::Map(grid_open));

        for &sub in BAG_SUBBAG_KEYS {
            if sub == "item" && !self.items.is_empty() {
                root.insert(sub.to_owned(), Amf3Value::Map(self.items.clone()));
            } else {
                root.insert(sub.to_owned(), Amf3Value::empty_map());
            }
        }

        Amf3Value::Map(root)
    }
}

/// Hero DTO inside `formation.getPositionInfo` (`HeroInfo.lua`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroDto {
    pub hid: i64,
    pub htid: i64,
    pub level: i64,
    pub evolve_level: i64,
}

impl HeroDto {
    pub fn protagonist(hid: i64, utid: i64, level: i64) -> Self {
        Self {
            hid,
            htid: utid,
            level,
            evolve_level: 0,
        }
    }

    pub fn to_amf3(&self) -> Amf3Value {
        let mut m = BTreeMap::new();
        m.insert("hid".to_owned(), Amf3Value::Int(self.hid));
        m.insert("htid".to_owned(), Amf3Value::Int(self.htid));
        m.insert("level".to_owned(), Amf3Value::Int(self.level));
        m.insert("exp".to_owned(), Amf3Value::Int(0));
        m.insert("evolve_level".to_owned(), Amf3Value::Int(self.evolve_level));
        m.insert("dress_id".to_owned(), Amf3Value::Int(0));
        m.insert("upgrade_level".to_owned(), Amf3Value::Int(0));
        // CRITICAL: HeroInfo:cloneTable calls `pairs(va_fail_reward)` without a nil guard!
        m.insert("va_fail_reward".to_owned(), Amf3Value::empty_map());
        m.insert("va_use_wash".to_owned(), Amf3Value::empty_map());
        m.insert("va_last_wash".to_owned(), Amf3Value::empty_map());
        m.insert("va_wash_count".to_owned(), Amf3Value::empty_map());
        m.insert("is_lock".to_owned(), Amf3Value::Int(0));
        m.insert("awake_rank".to_owned(), Amf3Value::Int(0));
        m.insert("awake_level".to_owned(), Amf3Value::Int(0));
        m.insert("gold_rank".to_owned(), Amf3Value::Int(0));
        m.insert("gold_level".to_owned(), Amf3Value::Int(0));
        m.insert("gold_status".to_owned(), Amf3Value::Int(0));
        m.insert("dark_gold_rank".to_owned(), Amf3Value::Int(0));
        m.insert("dark_gold_level".to_owned(), Amf3Value::Int(0));
        m.insert("purple_gold_rank".to_owned(), Amf3Value::Int(0));
        m.insert("purple_gold_level".to_owned(), Amf3Value::Int(0));
        m.insert("color_gold_rank".to_owned(), Amf3Value::Int(0));
        m.insert("color_gold_level".to_owned(), Amf3Value::Int(0));
        m.insert("va_color_gold_gift".to_owned(), Amf3Value::empty_map());
        m.insert("nirvana_gold_rank".to_owned(), Amf3Value::Int(0));
        m.insert("nirvana_gold_level".to_owned(), Amf3Value::Int(0));
        m.insert("va_nirvana_gold_gift".to_owned(), Amf3Value::empty_map());
        m.insert("gild_gold_rank".to_owned(), Amf3Value::Int(0));
        m.insert("gild_gold_level".to_owned(), Amf3Value::Int(0));
        m.insert("va_gild_gold_gift".to_owned(), Amf3Value::empty_map());
        m.insert("iron_gold_rank".to_owned(), Amf3Value::Int(0));
        m.insert("iron_gold_level".to_owned(), Amf3Value::Int(0));
        m.insert("va_iron_gold_gift".to_owned(), Amf3Value::empty_map());
        m.insert("hero_pill_rank".to_owned(), Amf3Value::Int(0));
        m.insert("hero_pill_level".to_owned(), Amf3Value::Int(0));
        m.insert("va_inlay".to_owned(), Amf3Value::empty_map());
        m.insert("hero_star_pos".to_owned(), Amf3Value::Int(0));
        m.insert("hero_star_level".to_owned(), Amf3Value::Int(1));
        Amf3Value::Map(m)
    }
}

/// Typed builder for `formation.getPositionInfo` (`HeroFormationData:parseData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroFormationSnapshot {
    pub position: [i64; 6],
    pub heroes: Vec<HeroDto>,
}

impl HeroFormationSnapshot {
    pub fn with_protagonist(major_hid: i64, utid: i64, level: i64) -> Self {
        Self {
            position: [major_hid, 0, 0, 0, 0, 0],
            heroes: vec![HeroDto::protagonist(major_hid, utid, level)],
        }
    }

    pub fn to_amf3(&self) -> Amf3Value {
        let mut all_heros = BTreeMap::new();
        for h in &self.heroes {
            all_heros.insert(h.hid.to_string(), h.to_amf3());
        }

        let six_empty_maps = || Amf3Value::Array((0..6).map(|_| Amf3Value::empty_map()).collect());
        let six_by_n_empty = |n: usize| {
            Amf3Value::Array(
                (0..6)
                    .map(|_| Amf3Value::Array((0..n).map(|_| Amf3Value::empty_map()).collect()))
                    .collect(),
            )
        };

        let mut root = BTreeMap::new();
        root.insert(
            "position".to_owned(),
            Amf3Value::Array(self.position.iter().copied().map(Amf3Value::Int).collect()),
        );
        root.insert("partner".to_owned(), Amf3Value::empty_map());
        root.insert("buddy".to_owned(), Amf3Value::empty_map());
        root.insert("buddyLevelInfo".to_owned(), Amf3Value::empty_map());
        root.insert("godanimal".to_owned(), Amf3Value::empty_map());
        root.insert("topanimal".to_owned(), Amf3Value::empty_map());
        root.insert("protection".to_owned(), Amf3Value::empty_map());
        root.insert("allHeros".to_owned(), Amf3Value::Map(all_heros));
        root.insert("jointSkill".to_owned(), Amf3Value::empty_map());
        root.insert("arming".to_owned(), six_empty_maps());
        root.insert("spirit".to_owned(), six_by_n_empty(6));
        root.insert("yanWu".to_owned(), six_by_n_empty(4));
        root.insert("brochure".to_owned(), six_by_n_empty(6));
        root.insert("soulSkill".to_owned(), six_by_n_empty(6));
        // CRITICAL: HeroFormationData:initBingFu calls `pairs(data.bingfu)` without a nil guard!
        root.insert("bingfu".to_owned(), Amf3Value::empty_map());
        root.insert("bingshu".to_owned(), Amf3Value::empty_map());
        root.insert("jobSkill".to_owned(), Amf3Value::empty_map());
        root.insert("arrCounsellor".to_owned(), Amf3Value::empty_map());
        root.insert("soulStone".to_owned(), Amf3Value::empty_map());
        root.insert("soulJade".to_owned(), Amf3Value::empty_map());
        root.insert("attrSkins".to_owned(), Amf3Value::empty_map());
        root.insert("morePowerOpenCount".to_owned(), Amf3Value::Int(0));
        root.insert(
            "formationBook".to_owned(),
            Amf3Value::map_from([
                ("cur_use", Amf3Value::Int(0)),
                ("lv_info", Amf3Value::empty_map()),
                ("reset_num", Amf3Value::Int(0)),
            ]),
        );
        Amf3Value::Map(root)
    }
}

/// Typed builder for `copy.getList` (`CopyData:parseData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyListSnapshot {
    pub fresher_reward_index: i64,
    pub chapter_1001_stars: i64,
}

impl Default for CopyListSnapshot {
    fn default() -> Self {
        Self {
            fresher_reward_index: 0,
            chapter_1001_stars: 3,
        }
    }
}

impl CopyListSnapshot {
    pub fn to_amf3(&self) -> Amf3Value {
        Amf3Value::map_from([
            (
                "fresherRewardIndex",
                Amf3Value::Int(self.fresher_reward_index),
            ),
            (
                "copyData",
                Amf3Value::map_from([(
                    "1001",
                    Amf3Value::map_from([
                        ("star", Amf3Value::Int(self.chapter_1001_stars)),
                        ("prize", Amf3Value::Int(0)),
                        ("pass_strongh", Amf3Value::Int(1)),
                        ("box_reward_num", Amf3Value::Int(0)),
                    ]),
                )]),
            ),
        ])
    }
}

/// Typed builder for `copy.getCopy` (`CopyInfoDataController` / `GuideManager:_dataAviable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyChapterSnapshot {
    pub chapter_id: i64,
    pub first_stage_normal_star: i64,
    pub first_stage_pass: i64,
}

impl Default for CopyChapterSnapshot {
    fn default() -> Self {
        Self {
            chapter_id: 1001,
            first_stage_normal_star: 3,
            first_stage_pass: 1,
        }
    }
}

impl CopyChapterSnapshot {
    pub fn to_amf3(&self) -> Amf3Value {
        let stage_key = format!("{}01", self.chapter_id);
        Amf3Value::map_from([(
            "va_copy",
            Amf3Value::map_from([(
                stage_key.as_str(),
                Amf3Value::map_from([
                    ("reset", Amf3Value::Int(0)),
                    ("normal_star", Amf3Value::Int(self.first_stage_normal_star)),
                    ("star_status", Amf3Value::Int(0)),
                    ("pass", Amf3Value::Int(self.first_stage_pass)),
                    ("box_reward", Amf3Value::Int(0)),
                ]),
            )]),
        )])
    }
}

/// Typed builder for `smallgame.getInfo` (`EnterViewData:parseData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterViewSnapshot {
    pub adopted_gates: Vec<i64>,
}

impl Default for EnterViewSnapshot {
    fn default() -> Self {
        Self {
            adopted_gates: vec![1, 2],
        }
    }
}

impl EnterViewSnapshot {
    pub fn to_amf3(&self) -> Amf3Value {
        let mut adopt_list = BTreeMap::new();
        for g in &self.adopted_gates {
            adopt_list.insert(g.to_string(), Amf3Value::Int(1));
        }
        Amf3Value::map_from([("adopt_list", Amf3Value::Map(adopt_list))])
    }
}

/// Typed builder for `barrage.get` (`BarrageProxy:onGetState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BarrageState {
    pub is_open: bool,
    pub send_time: i64,
}

impl BarrageState {
    pub fn to_amf3(&self) -> Amf3Value {
        Amf3Value::map_from([
            ("is_open", Amf3Value::Bool(self.is_open)),
            ("send_time", Amf3Value::Int(self.send_time)),
        ])
    }
}

/// Build random candidate names for `user.getRandomName` (`CreateUserScene.lua:380`).
pub fn build_random_names_ret(count: usize, gender: i64) -> Amf3Value {
    let pool_male = ["LongVương", "ChiếnThần", "TriệuVânVN", "QuanVũVN", "LữBốVN"];
    let pool_female = [
        "ĐiêuThuyền",
        "TiểuKiều",
        "ĐạiKiều",
        "TônThượngHương",
        "ChânCơ",
    ];
    let pool = if gender == 0 {
        &pool_female[..]
    } else {
        &pool_male[..]
    };
    let n = count.clamp(1, 20);
    let mut arr = Vec::with_capacity(n);
    for i in 0..n {
        let candidate = format!("{}{}", pool[i % pool.len()], i + 1);
        arr.push(Amf3Value::map_from([(
            "name",
            Amf3Value::String(candidate),
        )]));
    }
    Amf3Value::Array(arr)
}

/// Stateful mock server session capable of driving a real or simulated `vnsg2` client from
/// TCP connect (`user.login`) to `MainTown` at 100% `MainLoading`.
#[derive(Debug, Clone)]
pub struct BootstrapSession {
    pub has_created_role: bool,
    pub profile: UserProfile,
    pub open_switches: Vec<i64>,
    pub va_config: VaConfig,
    pub bag: BagSnapshot,
    pub copy_list: CopyListSnapshot,
    pub copy_chapter_1001: CopyChapterSnapshot,
    pub enter_view: EnterViewSnapshot,
    pub barrage_state: BarrageState,
    pub server_time: i64,
    pub session_token: String,
    pub main_town_ready_100_percent: bool,
    pub handled_methods: Vec<String>,
}

impl BootstrapSession {
    pub fn new_account(server_time: i64) -> Self {
        Self {
            has_created_role: false,
            profile: UserProfile {
                login_time: server_time,
                last_login_time: server_time,
                execution_time: server_time,
                buy_execution_time: server_time,
                energy_time: server_time,
                ..UserProfile::default()
            },
            open_switches: Vec::new(),
            va_config: VaConfig::default(),
            bag: BagSnapshot::default(),
            copy_list: CopyListSnapshot::default(),
            copy_chapter_1001: CopyChapterSnapshot::default(),
            enter_view: EnterViewSnapshot::default(),
            barrage_state: BarrageState::default(),
            server_time,
            session_token: "vnsg2_tok_0001".to_owned(),
            main_town_ready_100_percent: false,
            handled_methods: Vec::new(),
        }
    }

    pub fn existing_role(profile: UserProfile, server_time: i64) -> Self {
        let mut s = Self::new_account(server_time);
        s.has_created_role = true;
        s.profile = profile;
        s
    }

    /// Dispatch a single client RPC request and produce the exact Lua-compatible response envelope.
    pub fn handle_request(&mut self, req: &RpcRequestEnvelope) -> RpcResponseEnvelope {
        self.handled_methods.push(req.method.clone());
        let ret = match req.method.as_str() {
            net_const::CS_USER_LOGIN => Amf3Value::string("ok"),
            net_const::CS_GET_USER_BEFRO_LOGIN => {
                if self.has_created_role {
                    self.profile.to_before_login_amf3()
                } else {
                    Amf3Value::empty_map()
                }
            }
            net_const::CS_USER_GET_RANDOM_NAME => {
                let count = req.args.first().and_then(Amf3Value::as_i64).unwrap_or(20) as usize;
                let gender = req.args.get(1).and_then(Amf3Value::as_i64).unwrap_or(1);
                build_random_names_ret(count, gender)
            }
            "user.stat" | "user.guideEvent" => Amf3Value::string("ok"),
            net_const::CS_USER_CREATEUSER => {
                if let Some(utid) = req.args.first().and_then(Amf3Value::as_i64) {
                    self.profile.utid = utid;
                }
                if let Some(uname) = req.args.get(1).and_then(Amf3Value::as_str) {
                    self.profile.uname = uname.to_owned();
                }
                self.has_created_role = true;
                Amf3Value::string("ok")
            }
            net_const::CS_USER_USERLOGIN => Amf3Value::string("ok"),
            net_const::CS_GET_USER => self.profile.to_get_user_amf3(),
            net_const::CS_OPENSWITCH_GETSWITCH => Amf3Value::Array(
                self.open_switches
                    .iter()
                    .copied()
                    .map(Amf3Value::Int)
                    .collect(),
            ),
            net_const::CS_USER_GET_VA_CONFIG => self.va_config.to_amf3(),
            net_const::CS_FORMATION_GET => {
                FormationSquad::with_major_hero(self.profile.major_hid).to_amf3()
            }
            net_const::CS_ALL_ACTIVITY_GETACTIVITYINFO => Amf3Value::empty_map(),
            net_const::CS_GET_BAG_DATA => self.bag.to_amf3(),
            net_const::CS_FORMATION_GET_POSITION_INFO => HeroFormationSnapshot::with_protagonist(
                self.profile.major_hid,
                self.profile.utid,
                self.profile.level,
            )
            .to_amf3(),
            "halidom.getInfo" => Amf3Value::empty_map(),
            net_const::CS_COPY_LIST => self.copy_list.to_amf3(),
            net_const::CS_COPY_CHAPTERINFO => {
                let chap = req.args.first().and_then(Amf3Value::as_i64).unwrap_or(1001);
                if chap == 1001 {
                    self.copy_chapter_1001.to_amf3()
                } else {
                    CopyChapterSnapshot {
                        chapter_id: chap,
                        first_stage_normal_star: 0,
                        first_stage_pass: 0,
                    }
                    .to_amf3()
                }
            }
            net_const::CS_SMALLGAME_GETINFO => self.enter_view.to_amf3(),
            net_const::CS_REWARD_GETREWARDLIST => Amf3Value::empty_array(),
            net_const::CS_GET_MAIL_DATA => Amf3Value::empty_map(),
            net_const::CS_FRIEND_GETINFO => Amf3Value::empty_map(),
            net_const::CS_PLAYER_TITLE_GET_ALL => Amf3Value::map_from([
                ("titleArr", Amf3Value::empty_map()),
                ("equipTitle", Amf3Value::Int(0)),
                ("equipAttrTitle", Amf3Value::Int(0)),
            ]),
            net_const::CS_HERO_TITLE_GET_ALL => Amf3Value::map_from([
                ("equipTitle", Amf3Value::empty_map()),
                ("equipAttrTitle", Amf3Value::empty_map()),
                ("titleData", Amf3Value::empty_map()),
            ]),
            net_const::CS_LEGION_MEMBERINFO => Amf3Value::string("not_In_Guild"),
            net_const::CS_CHAT_WORLD_GET
            | net_const::CS_CHAT_GUILD_GET
            | net_const::CS_CHAT_GUILD_CROSS_SERVER_GET
            | net_const::CS_CHAT_CHAT_CROSS_SERVER_GET
            | net_const::CS_CHAT_TIMEMACHINE_GET => Amf3Value::empty_array(),
            net_const::CS_ACTIVITY_OPEN_CARNIVAL_GET_OPEN_ACHIEVE => Amf3Value::empty_map(),
            "barrage.get" => self.barrage_state.to_amf3(),
            "barrage.enterRoom" => {
                // BarrageProxy:onEnterRoom invokes MainScene:init() -> MainTown:new() -> MainLoading:hide() (100%)
                self.main_town_ready_100_percent = true;
                Amf3Value::Bool(true)
            }
            "barrage.getBarrageRecord" => Amf3Value::empty_array(),
            net_const::CS_LORDWAR_GET_TOP
            | net_const::CS_CROSSCLANBATTLE_GET_TOP
            | net_const::CS_CROSSCLANBATTLE_ROUNDINFO
            | net_const::CS_MILLIONQUIZ_GETINFO => Amf3Value::empty_map(),
            _ => Amf3Value::empty_map(),
        };

        RpcResponseEnvelope::ok(
            req.callback_name.clone(),
            ret,
            self.server_time,
            self.session_token.clone(),
        )
    }
}
