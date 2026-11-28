//! Complete TCP Connect -> `MainTown` (100% `MainLoading`) bootstrap RPC sequence, typed
//! response builders, and stateful [`BootstrapSession`] server simulator.

use std::collections::BTreeMap;

use crate::amf3::Amf3Value;
use crate::envelope::{RpcRequestEnvelope, RpcResponseEnvelope};
use crate::net_const;

/// Construct an [`Amf3Value`] literal (`{}` empty map, `[]` empty array, nested `{ k => v }`, `[...]`, or primitive).
macro_rules! amf3_val {
    ({}) => {
        Amf3Value::empty_map()
    };
    ([]) => {
        Amf3Value::empty_array()
    };
    ({ $($body:tt)+ }) => {
        amf3_map! { $($body)+ }
    };
    ([ $($elem:expr),+ $(,)? ]) => {
        Amf3Value::Array(vec![ $( Amf3Value::from($elem) ),+ ])
    };
    ($val:expr) => {
        Amf3Value::from($val)
    };
}

/// Construct a string-keyed [`Amf3Value::Map`] (`BTreeMap<String, Amf3Value>`), supporting both
/// `"key" => value` entries and `["k1", "k2", ...] => shared_value` batch initializers.
macro_rules! amf3_map {
    (@insert $m:ident) => {};
    (@insert $m:ident [ $($k:expr),+ $(,)? ] => {} $(, $($rest:tt)*)?) => {
        $( $m.insert(String::from($k), amf3_val!({})); )+
        $( amf3_map!(@insert $m $($rest)*); )?
    };
    (@insert $m:ident [ $($k:expr),+ $(,)? ] => { $($v:tt)+ } $(, $($rest:tt)*)?) => {{
        let shared = amf3_val!({ $($v)+ });
        $( $m.insert(String::from($k), shared.clone()); )+
        $( amf3_map!(@insert $m $($rest)*); )?
    }};
    (@insert $m:ident [ $($k:expr),+ $(,)? ] => $v:expr $(, $($rest:tt)*)?) => {
        $( $m.insert(String::from($k), Amf3Value::from($v)); )+
        $( amf3_map!(@insert $m $($rest)*); )?
    };
    (@insert $m:ident $k:tt => { $($v:tt)* } $(, $($rest:tt)*)?) => {
        $m.insert(String::from($k), amf3_val!({ $($v)* }));
        $( amf3_map!(@insert $m $($rest)*); )?
    };
    (@insert $m:ident $k:tt => [ $($v:tt)* ] $(, $($rest:tt)*)?) => {
        $m.insert(String::from($k), amf3_val!([ $($v)* ]));
        $( amf3_map!(@insert $m $($rest)*); )?
    };
    (@insert $m:ident $k:tt => $v:expr $(, $($rest:tt)*)?) => {
        $m.insert(String::from($k), Amf3Value::from($v));
        $( amf3_map!(@insert $m $($rest)*); )?
    };
    ($($body:tt)*) => {{
        let mut m = BTreeMap::<String, Amf3Value>::new();
        amf3_map!(@insert m $($body)*);
        Amf3Value::Map(m)
    }};
}

/// Declaratively build the `const` [`BOOTSTRAP_SEQUENCE`] table for both `NetConst` and `<inline>` RPCs.
macro_rules! bootstrap_specs {
    (
        $(
            ($step:expr, $phase:ident, $kind:ident $target:tt, $uncond:expr, $caller:expr, $args:expr, $ret:expr)
        ),* $(,)?
    ) => {
        &[
            $(
                BootstrapRpcSpec {
                    step_order: $step,
                    phase: BootstrapPhase::$phase,
                    net_const_name: bootstrap_specs!(@name $kind $target),
                    method: bootstrap_specs!(@method $kind $target),
                    unconditional_on_login: $uncond,
                    lua_caller: $caller,
                    request_args_schema: $args,
                    response_ret_schema: $ret,
                }
            ),*
        ]
    };
    (@name const $c:ident) => {
        stringify!($c)
    };
    (@name inline $m:literal) => {
        "<inline>"
    };
    (@method const $c:ident) => {
        net_const::$c
    };
    (@method inline $m:literal) => {
        $m
    };
}

/// High-level phase in the GServer (`vnsg2`) login-to-`MainTown` bootstrap pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BootstrapPhase {
    StageAAuthAndRole,
    StageBLoginInitAndFestival,
    StageCGuideAndCopyInit,
    StageDMainSceneToMainTown100,
    StageEPostMainTownBackground,
}

/// Static specification entry for a single RPC in the bootstrap sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapRpcSpec {
    pub step_order: u16,
    pub phase: BootstrapPhase,
    pub net_const_name: &'static str,
    pub method: &'static str,
    pub unconditional_on_login: bool,
    pub lua_caller: &'static str,
    pub request_args_schema: &'static str,
    pub response_ret_schema: &'static str,
}

/// Complete ordered bootstrap RPC specification table from TCP connect through `MainTown` 100%.
pub const BOOTSTRAP_SEQUENCE: &[BootstrapRpcSpec] = bootstrap_specs![
    (1, StageAAuthAndRole, const CS_USER_LOGIN, true,
     "LoginProxy:loginGame -> onUserLogin (LoginProxy.lua:971-1008)",
     "[ loginHashTable{AMF=3, hash, pid, qid, serverID='game<gid>', timestamp, offset, openDateTime, plname, plosgn, canCreateUser=1, canPay=1, mergeServerTime, uuid, record={}}, uuid:str, scrVersion:str ]",
     "\"ok\" (err=='ok' required; clears g.timestamp and calls getUserBeforLogin())"),
    (2, StageAAuthAndRole, const CS_GET_USER_BEFRO_LOGIN, true,
     "LoginProxy:getUserBeforLogin -> onGetUserBeforLogin (LoginProxy.lua:1010-1035)",
     "[]",
     "New account: {} (ret.uid == nil -> skips user.beforLoginBattle in vnsg2 CopyProxy.lua:1305 and enters CreateUserScene). Existing/created role: Map { uid:int, uname:str, utid:int(1|2), level:int } (MUST have top-level numeric uid!)"),
    (3, StageAAuthAndRole, const CS_USER_GET_RANDOM_NAME, false,
     "CreateUserScene:ctor -> CreateUserProxy:getRandomName (CreateUserScene.lua:102, 376-385)",
     "[ count:int(<=20, default 20), gender:int(1=male, 0=female) ]",
     "1-based Array [ { name:str }, ... ] (CreateUserScene indexes names[#names]['name'])"),
    (4, StageAAuthAndRole, inline "user.stat", false,
     "Stat:sendStat (Stat.lua / CreateUserScene.lua POINT_TIAOGUODONGHUA & POINT_ENTERGAME)",
     "[ statPointId:int ] (callbackName='user.stat' without _mId suffix)",
     "\"ok\" (fire-and-forget telemetry ack)"),
    (5, StageAAuthAndRole, const CS_USER_CREATEUSER, false,
     "CreateUserScene:_onEnterTap -> CreateUserProxy:createUser (CreateUserScene.lua:319-360)",
     "[ utid:int(1=male, 2=female), uname:str(1..=15 UTF-8 chars) ]",
     "\"ok\" (triggers second user.getUserBeforLogin() which now returns {uid, uname, utid, level})"),
    (6, StageAAuthAndRole, const CS_USER_USERLOGIN, true,
     "LoginProxy:loginUid -> onLoginUid (LoginProxy.lua:1058-1095)",
     "[ uid:int ]",
     "Literal string \"ok\" (\"fail\" raises error alert; \"ban:<unix_ts>:<reason>\" shows ban dialog)"),
    (7, StageBLoginInitAndFestival, const CS_GET_USER, true,
     "LoginInitDataProxy -> UserData:parseData (LoginProxy.lua:1098, UserData.lua:177-390)",
     "[]",
     "Map { uid:int, pid:str|int, uname:str, utid:int(1|2), level:int, vip:int, exp:int, copper:int|str, gold:int, gold_bind:int, gold_recharge_accum_all:int, execution:int, execution_time:int, buy_execution_accum:int, buy_execution_time:int, energy:int, energy_time:int, server_open_time:str('YYYYMMDDHHMMSS' 14 chars!), server_id:int, group_id:str, destiny:int(-1), fight_force_max:int, dress_info:{}, dress_expire:{}, dress_hero_info:{}, va_avatar:{}, ... }"),
    (8, StageBLoginInitAndFestival, const CS_OPENSWITCH_GETSWITCH, true,
     "LoginInitDataProxy -> OpenSwitchData:parseData (OpenSwitchData.lua:53-76)",
     "[]",
     "1-based Array [ open_node_id:int, ... ] (initializes self.dicAlreadyOpen={} required by LuaAssert across all proxies!)"),
    (9, StageBLoginInitAndFestival, const CS_USER_GET_VA_CONFIG, true,
     "LoginInitDataProxy -> ConfigData:parseData (ConfigData.lua:39-84, GuideManager.lua:159-250)",
     "[]",
     "Map { guide_id:int(240=GuideManager.END_ID, or 360 -> nextLeadId=240 per live_capture_filtered.log), guide_once_ids:int[], is_first_fighten:bool, opened_copy_chapters:int[] }"),
    (10, StageBLoginInitAndFestival, const CS_FORMATION_GET, true,
     "LoginInitDataProxy -> FormationData:parseData (FormationData.lua:33-52)",
     "[]",
     "Map { formation: 1-based Array [ major_hid:int, 0, 0, 0, 0, 0 ] }"),
    (11, StageBLoginInitAndFestival, const CS_ALL_ACTIVITY_GETACTIVITYINFO, true,
     "LoginProxy:dataAvialable -> FestivalManager:initDb / ActivityData:parseData (LoginProxy.lua:1109, ActivityData.lua)",
     "[]",
     "Map {} (or active festival CSV map; empty {} logs '<name>的csvdata是空的' and advances to GuideManager:run())"),
    (12, StageCGuideAndCopyInit, const CS_GET_BAG_DATA, true,
     "GuideManager:run -> CopyProxy:essentialData -> BagData:parseData (BagData.lua:412-565)",
     "[ {} ] (1-element array wrapping an empty table)",
     "Map { daily_use:{}, gridStart:{item,arm,treasure,fragment,soul,awake,awake_fragment,treasure_fragment,spirit,...}, gridMaxNum:{...}, gridOpenTimes:{arm,treasure,awake,spirit,...}, plus 41 BagConsts sub-bag maps={} }"),
    (13, StageCGuideAndCopyInit, const CS_FORMATION_GET_POSITION_INFO, true,
     "CopyProxy:essentialData -> HeroFormationData:parseData (HeroFormationData.lua:152-330, HeroInfo.lua:108-290)",
     "[]",
     "Map { position:[hid,0,0,0,0,0], partner:{}, buddy:{}, buddyLevelInfo:{}, godanimal:{}, topanimal:{}, protection:{}, allHeros:{ [tostring(hid)]: HeroDto{va_fail_reward={}, ...} }, jointSkill:{}, arming:[{}x6], spirit:[[{}x6]x6], yanWu:[[{}x4]x6], brochure:[[{}x6]x6], soulSkill:[[{}x6]x6], bingfu:{}, bingshu:{}, jobSkill:{}, arrCounsellor:{}, attrSkins:{}, morePowerOpenCount:0, formationBook:{cur_use:0, lv_info:{}, reset_num:0} }"),
    (14, StageCGuideAndCopyInit, inline "halidom.getInfo", true,
     "CopyProxy:essentialData -> HalidomData:parseData (HalidomData.lua:44-68)",
     "[]",
     "Map {} (HalidomData checks `if next(data) then ... end`, so empty {} safely skips)"),
    (15, StageCGuideAndCopyInit, const CS_COPY_LIST, true,
     "CopyProxy:essentialData -> CopyData:parseData (CopyData.lua:42-78)",
     "[ 1 ] (COPY_TYPE_1 normal chapters)",
     "Map { fresherRewardIndex:int(0), copyData:{ '1001': { star:3, prize:0, pass_strongh:1, box_reward_num:0 } } } (fresherRewardIndex + 1 requires non-nil number!)"),
    (16, StageCGuideAndCopyInit, const CS_COPY_CHAPTERINFO, true,
     "GuideManager:_dataAviable -> CopyInfoDataController:requestData (GuideManager.lua:204-219, MainScene.lua:568,1111)",
     "[ 1001 ] (chapter 1001)",
     "Map { va_copy: { '100101': { reset:0, normal_star:3, star_status:0, pass:1, box_reward:0 } } } (normal_star >= 1 ensures isFirstFight(100101)==false so MainLoading reaches 100% and enters MainTown!)"),
    (17, StageDMainSceneToMainTown100, const CS_SMALLGAME_GETINFO, true,
     "MainScene:run -> MainProxy:essentialData -> EnterViewData:parseData (EnterViewData.lua:27-68)",
     "[]",
     "Map { adopt_list: { '1':1, '2':1 } } (adopt_list MUST be a table; marking '1' & '2' adopted prevents enterViewMainPanel hijack)"),
    (18, StageDMainSceneToMainTown100, const CS_REWARD_GETREWARDLIST, true,
     "MainScene:run -> MainProxy:essentialData -> RewardData:parseData (RewardData.lua:34-65)",
     "[ 0, 100 ]",
     "1-based Array [] (or reward entries list)"),
    (19, StageDMainSceneToMainTown100, const CS_CHAT_WORLD_GET, true,
     "MainScene:run -> EnChatProxy:initData (ChatProxy.lua:55-96)",
     "[]",
     "1-based Array [] (list of compressed chat ByteArrays; empty [] initializes _worldData)"),
    (20, StageDMainSceneToMainTown100, const CS_CHAT_GUILD_GET, true,
     "MainScene:run -> EnChatProxy:initData (ChatProxy.lua:97-130)",
     "[]",
     "1-based Array [] (empty [] initializes _guildData and invokes EnCampaignProxy callback)"),
    (21, StageDMainSceneToMainTown100, const CS_ACTIVITY_OPEN_CARNIVAL_GET_OPEN_ACHIEVE, false,
     "MainScene:checkSevenDayState -> ActSevenDayProxy:checkDataAvialable (MainScene.lua:248-263)",
     "[] (sent when getOpenServerDay() <= reward_end_time)",
     "Map {} (seven-day carnival achievement map)"),
    (22, StageDMainSceneToMainTown100, inline "barrage.get", true,
     "MainScene:initBarrage -> MainTownBarrageProxy:getMainTownBarrageState -> BarrageProxy:getState (MainScene.lua:276, BarrageProxy.lua:324-339)",
     "[]",
     "Map { is_open:bool, send_time:int }"),
    (23, StageDMainSceneToMainTown100, inline "barrage.enterRoom", true,
     "MainScene:initBarrage -> MainTownBarrageProxy:enterRoom (MainScene.lua:277, BarrageProxy.lua:370-393)",
     "[ roomId:int ]",
     "Bool true / \"ok\" (callback invokes MainScene:init() -> MainTown:new() -> MainLoading:hide() sets progress bar to 100%!)"),
    (24, StageDMainSceneToMainTown100, inline "barrage.getBarrageRecord", true,
     "BarrageProxy:onEnterRoom -> getBarrageRecord (BarrageProxy.lua:53-71, 392)",
     "[ roomId_or_1:int ]",
     "1-based Array [] (iterated via `for _, data in pairs(ret)` — MUST be non-nil table!)"),
    (25, StageEPostMainTownBackground, const CS_LORDWAR_GET_TOP, true,
     "MainScene:init -> worldSoloData:sendGetTopUser (MainScene.lua:436)",
     "[]",
     "Map {} (top solo temple snapshot)"),
    (26, StageEPostMainTownBackground, const CS_CROSSCLANBATTLE_GET_TOP, true,
     "MainScene:init -> legionCrossData:sendGetTopUser (MainScene.lua:451)",
     "[]",
     "Map {} (cross-clan temple snapshot)"),
    (27, StageEPostMainTownBackground, const CS_CROSSCLANBATTLE_ROUNDINFO, true,
     "MainScene:init -> legionCrossData:sendGetPromotionInfo (MainScene.lua:452)",
     "[]",
     "Map {} (cross-clan promotion snapshot)"),
    (28, StageEPostMainTownBackground, const CS_MILLIONQUIZ_GETINFO, true,
     "MainScene:init 1s timer -> millionQuizData (MainScene.lua:455-461)",
     "[]",
     "Map {} (million-quiz state)"),
];

/// All 41 sub-bag table keys initialized in `BagData:parseData` (`BagConsts.lua` + `BagData.lua`).
pub const BAG_SUBBAG_KEYS: &[&str] = &[
    "item", "arm", "treasure", "fragment", "soul", "awake", "awake_fragment", "treasure_fragment",
    "spirit", "pet", "pet_fragment", "god_arm", "god_arm_fragment", "god_animal",
    "god_animal_fragment", "top_animal", "top_animal_fragment", "hero_legend",
    "hero_legend_fragment", "brochure", "brochure_fragment", "halidom_fragment",
    "bingfu_fragment", "gem", "soul_skill", "statue_fragment", "bingshu_fragment",
    "jobskill_fragment", "yanwu", "yanwu_fragment", "yanwu_soul", "counsellor",
    "counsellor_fragment", "counsellor_equip", "counsellor_equip_fragment", "soul_stone",
    "soul_stone_fragment", "soul_jade", "soul_jade_fragment", "ploy_fragment",
    "great_seal_fragment",
];

/// Required `gridStart` keys in `BagData:parseData` (first 9 have NO `or` fallback in Lua).
pub const BAG_GRID_START_DEFAULTS: &[(&str, i64)] = &[
    ("item", 10_001), ("arm", 20_001), ("treasure", 30_001), ("fragment", 40_001),
    ("soul", 50_001), ("awake", 60_001), ("awake_fragment", 70_001), ("treasure_fragment", 80_001),
    ("spirit", 90_001), ("pet", 100_001), ("pet_fragment", 200_001), ("god_arm", 210_001),
    ("god_arm_fragment", 220_001), ("halidom_fragment", 230_001), ("gem", 240_001),
    ("god_animal", 310_001), ("god_animal_fragment", 320_001), ("hero_legend", 420_001),
    ("hero_legend_fragment", 420_001), ("brochure", 510_001), ("brochure_fragment", 520_001),
    ("bingfu_fragment", 530_001), ("soul_skill", 540_001), ("yanwu", 580_001),
    ("yanwu_fragment", 590_001), ("yanwu_soul", 600_001), ("counsellor_equip", 630_001),
    ("top_animal", 710_001), ("top_animal_fragment", 720_001),
];

/// Default `gridMaxNum` capacities in `BagData:parseData`.
pub const BAG_GRID_MAX_DEFAULTS: &[(&str, i64)] = &[
    ("item", 10_000), ("arm", 200), ("treasure", 200), ("fragment", 10_000),
    ("soul", 10_000), ("awake", 200), ("awake_fragment", 10_000), ("treasure_fragment", 10_000),
    ("spirit", 200), ("pet", 200), ("pet_fragment", 10_000), ("god_arm", 200),
    ("god_arm_fragment", 10_000), ("halidom_fragment", 10_000), ("gem", 1_000),
    ("god_animal", 200), ("god_animal_fragment", 10_000), ("hero_legend", 500),
    ("hero_legend_fragment", 10_000), ("brochure", 500), ("brochure_fragment", 10_000),
    ("bingfu_fragment", 10_000), ("soul_skill", 500), ("yanwu", 500),
    ("yanwu_fragment", 10_000), ("yanwu_soul", 500), ("counsellor_equip", 500),
    ("top_animal", 200), ("top_animal_fragment", 10_000),
];

/// Required `gridOpenTimes` keys in `BagData:parseData` (first 4 have NO `or` fallback in Lua).
pub const BAG_GRID_OPEN_TIMES_DEFAULTS: &[(&str, i64)] = &[
    ("arm", 0), ("treasure", 0), ("awake", 0), ("spirit", 0), ("pet", 0), ("god_arm", 0),
    ("god_animal", 0), ("yanwu", 0), ("yanwu_soul", 0), ("gem", 0), ("counsellor_equip", 0),
    ("top_animal", 0),
];

fn slice_to_int_map(pairs: &[(&str, i64)]) -> Amf3Value {
    Amf3Value::Map(
        pairs
            .iter()
            .map(|&(k, v)| (k.to_owned(), Amf3Value::Int(v)))
            .collect(),
    )
}

fn ints_to_amf3_array(vals: &[i64]) -> Amf3Value {
    Amf3Value::Array(vals.iter().copied().map(Amf3Value::Int).collect())
}

/// Typed builder for `user.getUser` (`UserData:parseData`).
#[derive(Debug, Clone, PartialEq)]
pub struct UserProfile {
    pub uid: i64,
    pub pid: String,
    pub uname: String,
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
    pub server_open_time: String,
    pub server_id: i64,
    pub group_id: String,
    pub login_time: i64,
    pub last_login_time: i64,
    pub online_accum_time: i64,
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
        amf3_map! {
            "uid" => self.uid,
            "uname" => &self.uname,
            "utid" => self.utid,
            "level" => self.level,
            "vip" => self.vip,
            "dress_id" => 0,
        }
    }

    /// Build the full `user.getUser` dictionary expected by `UserData:parseData`.
    pub fn to_get_user_amf3(&self) -> Amf3Value {
        amf3_map! {
            "uid" => self.uid,
            "pid" => &self.pid,
            "uname" => &self.uname,
            "utid" => self.utid,
            "level" => self.level,
            "vip" => self.vip,
            "vip_exp" => self.vip_exp,
            "exp" => self.exp,
            "copper" => self.copper,
            "gold" => self.gold,
            "gold_bind" => self.gold_bind,
            "gold_recharge_accum_all" => self.gold_recharge_accum_all,
            "is_recharge" => self.is_recharge,
            "execution" => self.execution,
            "execution_time" => self.execution_time,
            "buy_execution_accum" => self.buy_execution_accum,
            "buy_execution_time" => self.buy_execution_time,
            "energy" => self.energy,
            "energy_time" => self.energy_time,
            "server_open_time" => &self.server_open_time,
            "server_id" => self.server_id,
            "group_id" => &self.group_id,
            "login_time" => self.login_time,
            "last_login_time" => self.last_login_time,
            "online_accum_time" => self.online_accum_time,
            "destiny" => self.destiny,
            "destiny_star" => self.destiny_star,
            "fc" => self.fc,
            "fight_force_max" => self.fight_force_max,
            [
                "dress_id", "attr_dress_id", "avatar_id", "guild_id", "prestige", "arena_coin",
                "godarm_coin", "tower_coin", "rebel_coin", "hero_soul", "pet_coin",
                "awakeArm_coin", "lordwar_coin", "world_team_coin", "world_person_score",
                "voucher_coin", "time_machine_voucher_coin", "iron_gold_coin",
            ] => 0,
            ["dress_info", "dress_expire", "dress_hero_info", "va_avatar"] => {},
            ["fiend_close", "cross_guild_close", "open_carnival2_close"] => false,
        }
    }
}

/// Typed builder for `user.getVaConfig` (`ConfigData:parseData` & `GuideManager`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaConfig {
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
        amf3_map! {
            "guide_id" => self.guide_id,
            "guide_once_ids" => ints_to_amf3_array(&self.guide_once_ids),
            "is_first_fighten" => self.is_first_fighten,
            "opened_copy_chapters" => ints_to_amf3_array(&self.opened_copy_chapters),
        }
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
        amf3_map! {
            "formation" => ints_to_amf3_array(&self.slots),
        }
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
        root.insert("gridStart".to_owned(), slice_to_int_map(BAG_GRID_START_DEFAULTS));
        root.insert("gridMaxNum".to_owned(), slice_to_int_map(BAG_GRID_MAX_DEFAULTS));
        root.insert(
            "gridOpenTimes".to_owned(),
            slice_to_int_map(BAG_GRID_OPEN_TIMES_DEFAULTS),
        );
        for &sub in BAG_SUBBAG_KEYS {
            let tbl = if sub == "item" && !self.items.is_empty() {
                Amf3Value::Map(self.items.clone())
            } else {
                Amf3Value::empty_map()
            };
            root.insert(sub.to_owned(), tbl);
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
        amf3_map! {
            "hid" => self.hid,
            "htid" => self.htid,
            "level" => self.level,
            "evolve_level" => self.evolve_level,
            "hero_star_level" => 1,
            [
                "exp", "dress_id", "upgrade_level", "is_lock", "awake_rank", "awake_level",
                "gold_rank", "gold_level", "gold_status", "dark_gold_rank", "dark_gold_level",
                "purple_gold_rank", "purple_gold_level", "color_gold_rank", "color_gold_level",
                "nirvana_gold_rank", "nirvana_gold_level", "gild_gold_rank", "gild_gold_level",
                "iron_gold_rank", "iron_gold_level", "hero_pill_rank", "hero_pill_level",
                "hero_star_pos",
            ] => 0,
            [
                "va_fail_reward", "va_use_wash", "va_last_wash", "va_wash_count",
                "va_color_gold_gift", "va_nirvana_gold_gift", "va_gild_gold_gift",
                "va_iron_gold_gift", "va_inlay",
            ] => {},
        }
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
        let all_heros: BTreeMap<String, Amf3Value> = self
            .heroes
            .iter()
            .map(|h| (h.hid.to_string(), h.to_amf3()))
            .collect();
        let six_empty_maps = || Amf3Value::Array(vec![Amf3Value::empty_map(); 6]);
        let six_by_n_empty =
            |n: usize| Amf3Value::Array(vec![Amf3Value::Array(vec![Amf3Value::empty_map(); n]); 6]);

        amf3_map! {
            "position" => ints_to_amf3_array(&self.position),
            "allHeros" => all_heros,
            "arming" => six_empty_maps(),
            "spirit" => six_by_n_empty(6),
            "yanWu" => six_by_n_empty(4),
            "brochure" => six_by_n_empty(6),
            "soulSkill" => six_by_n_empty(6),
            "morePowerOpenCount" => 0,
            "formationBook" => {
                "cur_use" => 0,
                "lv_info" => {},
                "reset_num" => 0,
            },
            [
                "partner", "buddy", "buddyLevelInfo", "godanimal", "topanimal", "protection",
                "jointSkill", "bingfu", "bingshu", "jobSkill", "arrCounsellor", "soulStone",
                "soulJade", "attrSkins",
            ] => {},
        }
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
        amf3_map! {
            "fresherRewardIndex" => self.fresher_reward_index,
            "copyData" => {
                "1001" => {
                    "star" => self.chapter_1001_stars,
                    "prize" => 0,
                    "pass_strongh" => 1,
                    "box_reward_num" => 0,
                },
            },
        }
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
        amf3_map! {
            "va_copy" => {
                (stage_key) => {
                    "reset" => 0,
                    "normal_star" => self.first_stage_normal_star,
                    "star_status" => 0,
                    "pass" => self.first_stage_pass,
                    "box_reward" => 0,
                },
            },
        }
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
        let adopt_list: BTreeMap<String, Amf3Value> = self
            .adopted_gates
            .iter()
            .map(|g| (g.to_string(), Amf3Value::Int(1)))
            .collect();
        amf3_map! { "adopt_list" => adopt_list }
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
        amf3_map! {
            "is_open" => self.is_open,
            "send_time" => self.send_time,
        }
    }
}

/// Build random candidate names for `user.getRandomName` (`CreateUserScene.lua:380`).
pub fn build_random_names_ret(count: usize, gender: i64) -> Amf3Value {
    const POOL_MALE: &[&str] = &["LongVương", "ChiếnThần", "TriệuVânVN", "QuanVũVN", "LữBốVN"];
    const POOL_FEMALE: &[&str] = &[
        "ĐiêuThuyền",
        "TiểuKiều",
        "ĐạiKiều",
        "TônThượngHương",
        "ChânCơ",
    ];
    let pool = if gender == 0 { POOL_FEMALE } else { POOL_MALE };
    let n = count.clamp(1, 20);
    Amf3Value::Array(
        (0..n)
            .map(|i| amf3_map! { "name" => format!("{}{}", pool[i % pool.len()], i + 1) })
            .collect(),
    )
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
            net_const::CS_USER_LOGIN | "user.stat" | "user.guideEvent" | net_const::CS_USER_USERLOGIN => {
                amf3_val!("ok")
            }
            net_const::CS_GET_USER_BEFRO_LOGIN => {
                if self.has_created_role {
                    self.profile.to_before_login_amf3()
                } else {
                    amf3_val!({})
                }
            }
            net_const::CS_USER_GET_RANDOM_NAME => {
                let count = req.args.first().and_then(Amf3Value::as_i64).unwrap_or(20) as usize;
                let gender = req.args.get(1).and_then(Amf3Value::as_i64).unwrap_or(1);
                build_random_names_ret(count, gender)
            }
            net_const::CS_USER_CREATEUSER => {
                if let Some(utid) = req.args.first().and_then(Amf3Value::as_i64) {
                    self.profile.utid = utid;
                }
                if let Some(uname) = req.args.get(1).and_then(Amf3Value::as_str) {
                    self.profile.uname = uname.to_owned();
                }
                self.has_created_role = true;
                amf3_val!("ok")
            }
            net_const::CS_GET_USER => self.profile.to_get_user_amf3(),
            net_const::CS_OPENSWITCH_GETSWITCH => ints_to_amf3_array(&self.open_switches),
            net_const::CS_USER_GET_VA_CONFIG => self.va_config.to_amf3(),
            net_const::CS_FORMATION_GET => {
                FormationSquad::with_major_hero(self.profile.major_hid).to_amf3()
            }
            net_const::CS_GET_BAG_DATA => self.bag.to_amf3(),
            net_const::CS_FORMATION_GET_POSITION_INFO => HeroFormationSnapshot::with_protagonist(
                self.profile.major_hid,
                self.profile.utid,
                self.profile.level,
            )
            .to_amf3(),
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
            net_const::CS_PLAYER_TITLE_GET_ALL => amf3_map! {
                "titleArr" => {},
                ["equipTitle", "equipAttrTitle"] => 0,
            },
            net_const::CS_HERO_TITLE_GET_ALL => amf3_map! {
                ["equipTitle", "equipAttrTitle", "titleData"] => {},
            },
            net_const::CS_LEGION_MEMBERINFO => amf3_val!("not_In_Guild"),
            net_const::CS_REWARD_GETREWARDLIST
            | net_const::CS_CHAT_WORLD_GET
            | net_const::CS_CHAT_GUILD_GET
            | net_const::CS_CHAT_GUILD_CROSS_SERVER_GET
            | net_const::CS_CHAT_CHAT_CROSS_SERVER_GET
            | net_const::CS_CHAT_TIMEMACHINE_GET
            | "barrage.getBarrageRecord" => amf3_val!([]),
            "barrage.get" => self.barrage_state.to_amf3(),
            "barrage.enterRoom" => {
                self.main_town_ready_100_percent = true;
                amf3_val!(true)
            }
            _ => amf3_val!({}),
        };

        RpcResponseEnvelope::ok(
            req.callback_name.clone(),
            ret,
            self.server_time,
            self.session_token.clone(),
        )
    }
}
