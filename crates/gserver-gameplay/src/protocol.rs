//! Wire framing, AMF3-compatible DTO values, RPC envelopes, and exact `NetConst.lua` method constants.

use std::collections::BTreeMap;
use std::fmt;

/// Exact Client-to-Server RPC method names verified from `NetConst.lua` and module proxies.
pub mod rpc {
    // User & Bag
    pub const USER_LOGIN: &str = "user.login";
    pub const USER_USER_LOGIN: &str = "user.userLogin";
    pub const USER_GET_USER: &str = "user.getUser";
    pub const USER_CREATE_USER: &str = "user.createUser";
    pub const USER_BEFORE_LOGIN_BATTLE: &str = "user.beforLoginBattle";
    pub const BAG_INFO: &str = "bag.bagInfo";
    pub const BAG_SELL_ITEMS: &str = "bag.sellItems";
    pub const BAG_OPEN_GRID_BY_GOLD: &str = "bag.openGridByGold";
    pub const HERO_GET_ALL: &str = "hero.getAllHeroes";
    pub const FORMATION_GET_POSITION_INFO: &str = "formation.getPositionInfo";

    // Town, City, RoyalCity & Castle
    pub const CITY_ENTER: &str = "city.enter";
    pub const CITY_LEAVE: &str = "city.leave";
    pub const CITY_GET_ENTER_LIST: &str = "city.getEnterList";
    pub const CITY_ENTER_SHADOW_INFO: &str = "city.enterShadowInfo";
    pub const TOWN_MOVE: &str = "town.move";
    pub const ROYALCITY_ENTER: &str = "imperialcity.enter";
    pub const ROYALCITY_LEAVE: &str = "imperialcity.leave";
    pub const ROYALCITY_MOVE: &str = "imperialcity.move";
    pub const IMPERIALCITY_HOST: &str = "imperialcity.getImperialCityHost";
    pub const IMPERIALCITY_LOGIN: &str = "imperialcity.login";
    pub const CASTLE_GET_INFO: &str = "castle.getInfo";
    pub const HALL_GET_INFO: &str = "hall.getInfo";
    pub const MARKET_GET_INFO: &str = "market.getInfo";
    pub const WORKSHOP_GET_INFO: &str = "workshop.getInfo";
    pub const SMITHY_GET_INFO: &str = "smithy.getInfo";

    // Chat
    pub const CHAT_SEND_WORLD: &str = "chat.sendWorld";
    pub const CHAT_GET_WORLD_LAST: &str = "chat.getWorldLastMsgs";
    pub const CHAT_SEND_PERSONAL: &str = "chat.sendPersonal";
    pub const CHAT_GET_PERSONAL_LAST: &str = "chat.getPersonalLastMsgs";
    pub const CHAT_SEND_GUILD: &str = "chat.sendGuild";
    pub const CHAT_GET_GUILD_LAST: &str = "chat.getGuildLastMsgs";
    pub const CHAT_SEND_CROSS_GUILD: &str = "chat.sendCrossGuild";
    pub const CHAT_GET_CROSS_GUILD_LAST: &str = "chat.getCrossGuildLastMsgs";
    pub const CHAT_SEND_CROSS_TEAM: &str = "chat.sendCrossGuildTeam";
    pub const CHAT_GET_CROSS_TEAM_LAST: &str = "chat.getCrossGuildTeamLastMsgs";
    pub const CHAT_SEND_COUNTRY: &str = "chat.sendCountry";
    pub const CHAT_GET_COUNTRY_LAST: &str = "chat.getCountryLastMsgs";
    pub const CHAT_GET_LAST_MAIN_POP: &str = "chat.getLastMainPop";
    pub const TM_CHAT_SEND_CROSS_GUILD: &str = "tmChat.sendCrossGuild";
    pub const TM_CHAT_GET_CROSS_GUILD_LAST: &str = "tmChat.getCrossGuildLastMsgs";

    // Mail & Reward Center
    pub const MAIL_GET_LIST: &str = "mail.getMailBoxList";
    pub const MAIL_READ_ALL: &str = "mail.readAllMail";
    pub const MAIL_SEND: &str = "mail.sendMail";
    pub const MAIL_DRAW_ITEMS: &str = "mail.drawMailItems";
    pub const MAIL_DRAW_ALL_ITEMS: &str = "mail.drawAllMailItems";
    pub const REWARD_GET_LIST: &str = "reward.getRewardList";
    pub const REWARD_RECEIVE: &str = "reward.receiveReward";
    pub const REWARD_RECEIVE_BY_RID_ARR: &str = "reward.receiveByRidArr";
    pub const REWARD_GIFT_BY_CODE: &str = "reward.giftByCode";

    // Shop & Tavern Raffle
    pub const SHOP_GET_INFO: &str = "shop.getInfo";
    pub const SHOP_BUY: &str = "shop.buy";
    pub const SHOP_REFRESH: &str = "shop.refresh";
    pub const SHOP_MULTI_REFRESH: &str = "shop.multiRefresh";
    pub const SHOP_QUICK_BUY: &str = "shop.quickBuy";
    pub const SHOP_AUTO_BUY: &str = "shop.normalShopAutoBuy";
    pub const RAFFLE_GET_INFO: &str = "raffle.getInfo";
    pub const RAFFLE_SINGLE: &str = "raffle.singleRaffle";
    pub const RAFFLE_AUTO: &str = "raffle.autoRaffle";
    pub const RAFFLE_MULTI: &str = "raffle.multiRaffle";
    pub const RAFFLE_HOT: &str = "raffle.raffleHot";
    pub const RAFFLE_MULTI_HOT: &str = "raffle.multiRaffleHot";
    pub const RAFFLE_RED: &str = "raffle.raffleRed";
    pub const RAFFLE_BUY_TOKEN: &str = "raffle.buyToken";
    pub const RAFFLE_RECEIVE_LUCK: &str = "raffle.receiveLuckHeroGift";
    pub const RAFFLE_RECEIVE_LUCK_HOT: &str = "raffle.receiveHotLuckHeroGift";
    pub const RAFFLE_RECEIVE_LUCK_RED: &str = "raffle.receiveRedLuckHeroGift";
    pub const RAFFLE_REFRESH_HOT: &str = "raffle.refreshHotByHand";
    pub const RECRUITMENT_GET_INFO: &str = "recruitment.getInfo";
    pub const RECRUITMENT_RECEIVE_REWARD: &str = "recruitment.receiveActiveReward";

    // PvE Campaign Copy & Battle Record
    pub const COPY_GET_LIST: &str = "copy.getList";
    pub const COPY_GET_COPY: &str = "copy.getCopy";
    pub const COPY_ATTACK: &str = "copy.attack";
    pub const COPY_PASS: &str = "copy.pass";
    pub const COPY_FAIL: &str = "copy.battleFail";
    pub const COPY_SWEEP: &str = "copy.sweep";
    pub const COPY_RESET_ATK: &str = "copy.resetAtk";
    pub const COPY_ONE_KEY_RESET_ATK: &str = "copy.oneKeyResetAtk";
    pub const COPY_FETCH_PRIZE: &str = "copy.fetchPrize";
    pub const COPY_GET_PRIZE: &str = "copy.getPrize";
    pub const COPY_FETCH_BOX_REWARD: &str = "copy.fetchBoxReward";
    pub const COPY_FETCH_FRESHER_REWARD: &str = "copy.fetchFresherReward";
    pub const COPY_GET_FAMOUS_PASS_TIMES: &str = "copy.getFamousPassTimes";
    pub const COPY_GET_STRONGH_RECORD: &str = "copy.getStronghRecord";
    pub const BATTLE_GET_RECORD: &str = "battle.getRecord";

    // Arena
    pub const ARENA_GET_BASE_DATA: &str = "arena.getBaseData";
    pub const ARENA_GET_ARENA_INFO: &str = "arena.getArenaInfo";
    pub const ARENA_ENTER: &str = "arena.enterArena";
    pub const ARENA_LEAVE: &str = "arena.leaveArena";
    pub const ARENA_GET_RANK_LIST: &str = "arena.getRankList";
    pub const ARENA_CHALLENGE: &str = "arena.challenge";
    pub const ARENA_QUICK_CHALLENGE: &str = "arena.quickChallenge";
    pub const ARENA_SWEEP: &str = "arena.sweep";
    pub const ARENA_BUY_TIMES: &str = "arena.buyChallengeTimes";
    pub const ARENA_FETCH_REWARD: &str = "arena.fetchReward";
    pub const ARENA_FETCH_NUM_REWARD: &str = "arena.fetchChallengeNumReward";

    // Sign, Online & Activity
    pub const SIGN_GET_INFO: &str = "sign.getInfo";
    pub const SIGN_TODAY: &str = "sign.signToday";
    pub const SIGN_GAIN_NORMAL: &str = "sign.gainNormalSignReward";
    pub const SIGN_GAIN_ACC: &str = "sign.gainAccSignReward";
    pub const SIGN_GAIN_VIP: &str = "sign.gainVipSignReward";
    pub const SIGN_GAIN_RECHARGE: &str = "sign.gainRechargeSignReward";
    pub const SUPER_CHECKIN_GET_INFO: &str = "superCheckin.getInfo";
    pub const SUPER_CHECKIN_SIGN: &str = "superCheckin.sign";
    pub const SUPER_CHECKIN_FETCH: &str = "superCheckin.fetchReward";
    pub const ONLINE_GET_INFO: &str = "online.getInfo";
    pub const ONLINE_FETCH: &str = "online.fetch";
    pub const OPEN_ONLINE_GET_INFO: &str = "openOnline.getInfo";
    pub const OPEN_ONLINE_RECEIVE: &str = "openOnline.receive";
    pub const ACTIVITY_GET_CONF: &str = "activity.getActivityConf";
}

/// Exact Server-to-Client push callback names verified from `NetConst.lua` & `Network.lua`.
///
/// Note the critical `vnsg2` protocol nuance: `user.updateUser`, `bag.update`, `shop.open`,
/// and `town.*` do NOT carry the `re.` prefix!
pub mod push {
    pub const USER_UPDATE: &str = "user.updateUser";
    pub const TM_USER_UPDATE: &str = "tmUser.updateUser";
    pub const USER_COPPER_OVER_MAX: &str = "re.user.copperOverMax";
    pub const SERVER_NOTIFY: &str = "SERVER_NOTIFY";
    pub const BAG_UPDATE: &str = "bag.update";
    pub const TM_BAG_UPDATE: &str = "tmBag.update";
    pub const HERO_NEW: &str = "re.hero.new";
    pub const TM_HERO_NEW: &str = "re.tmHero.new";
    pub const CHAT_GET_MSG: &str = "re.chat.getMsg";
    pub const TM_CHAT_GET_MSG: &str = "re.tmChat.getMsg";
    pub const MAIL_NEW: &str = "re.mail.newMail";
    pub const TM_MAIL_NEW: &str = "tmRe.mail.newMail";
    pub const REWARD_NEW: &str = "re.reward.new";
    pub const TM_REWARD_NEW: &str = "re.tmReward.new";
    pub const SHOP_OPEN: &str = "shop.open";
    pub const TM_SHOP_OPEN: &str = "tmShop.open";
    pub const TOWN_ADD_PLAYER: &str = "town.addPlayer";
    pub const TOWN_DEL_PLAYER: &str = "town.delPlayer";
    pub const TOWN_MOVE: &str = "town.move";
    pub const TOWN_UPDATE: &str = "town.update";
    pub const ARENA_DATA_REFRESH: &str = "re.arena.dataRefresh";
    pub const TM_ARENA_DATA_REFRESH: &str = "tmRe.arena.dataRefresh";
    pub const ACTIVITY_NEW_CONF: &str = "re.activity.newConf";
}

/// AMF3-compatible value representation used across GServer RPC tables and binary blobs.
#[derive(Clone, PartialEq)]
pub enum DtoValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Array(Vec<DtoValue>),
    Map(BTreeMap<String, DtoValue>),
}

impl fmt::Debug for DtoValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "Null"),
            Self::Bool(b) => write!(f, "Bool({b})"),
            Self::Int(i) => write!(f, "Int({i})"),
            Self::Float(v) => write!(f, "Float({v})"),
            Self::Str(s) => write!(f, "Str({s:?})"),
            Self::Bytes(b) => write!(f, "Bytes(len={})", b.len()),
            Self::Array(a) => f.debug_list().entries(a.iter()).finish(),
            Self::Map(m) => f.debug_map().entries(m.iter()).finish(),
        }
    }
}

impl DtoValue {
    pub fn map(entries: impl IntoIterator<Item = (impl Into<String>, DtoValue)>) -> Self {
        let mut m = BTreeMap::new();
        for (k, v) in entries {
            m.insert(k.into(), v);
        }
        Self::Map(m)
    }

    pub fn array(items: impl IntoIterator<Item = DtoValue>) -> Self {
        Self::Array(items.into_iter().collect())
    }

    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Float(v) => Some(*v as i64),
            Self::Str(s) => s.parse::<i64>().ok(),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|v| u32::try_from(v).ok())
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_i64().and_then(|v| u64::try_from(v).ok())
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Int(v) => Some(*v != 0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b.as_slice()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[DtoValue]> {
        match self {
            Self::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, DtoValue>> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&DtoValue> {
        self.as_map()?.get(key)
    }
}

impl From<bool> for DtoValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<i32> for DtoValue {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}
impl From<i64> for DtoValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}
impl From<u32> for DtoValue {
    fn from(v: u32) -> Self {
        Self::Int(i64::from(v))
    }
}
impl From<u64> for DtoValue {
    fn from(v: u64) -> Self {
        Self::Int(v as i64)
    }
}
impl From<usize> for DtoValue {
    fn from(v: usize) -> Self {
        Self::Int(v as i64)
    }
}
impl From<f64> for DtoValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}
impl From<&str> for DtoValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_owned())
    }
}
impl From<String> for DtoValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}

/// Decoded client RPC request (`{ method, args, token, callback = { callbackName } }`),
/// preserving Lua 0-indexed `args[0]`, named map entries (`{ boxId = 1 }`), and raw AMF3 `args`.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcRequest {
    pub method: String,
    pub args: Vec<DtoValue>,
    pub callback_name: String,
    pub token: String,
    pub arg_zero: Option<DtoValue>,
    pub named_args: Vec<(String, DtoValue)>,
    pub raw_args: Option<DtoValue>,
}

impl RpcRequest {
    pub fn new(method: impl Into<String>, args: Vec<DtoValue>, req_id: u32) -> Self {
        let method_str = method.into();
        let callback_name = format!("{method_str}_{req_id}");
        let raw_args = Some(DtoValue::Array(args.clone()));
        Self {
            method: method_str,
            args,
            callback_name,
            token: req_id.to_string(),
            arg_zero: None,
            named_args: Vec::new(),
            raw_args,
        }
    }

    /// Returns Lua 0-indexed `args[0]` when sent via associative key `"0"`.
    pub fn arg_zero(&self) -> Option<&DtoValue> {
        self.arg_zero
            .as_ref()
            .or_else(|| self.raw_args.as_ref().and_then(|r| r.get("0")))
    }

    /// Looks up a Lua argument using exact 0-based / 1-based Lua table indexing:
    /// - `lua_idx == 0`: returns Lua `args[0]` (`arg_zero` / `raw_args["0"]`).
    /// - `lua_idx >= 1`: returns Lua `args[lua_idx]` (`args[lua_idx - 1]` or `raw_args[tostring(lua_idx)]`).
    pub fn arg_at_lua0(&self, lua_idx: usize) -> Option<&DtoValue> {
        if lua_idx == 0 {
            return self.arg_zero();
        }
        if let Some(v) = self.args.get(lua_idx - 1) {
            if !matches!(v, DtoValue::Null) {
                return Some(v);
            }
        }
        let key = lua_idx.to_string();
        self.raw_args
            .as_ref()
            .and_then(|r| r.get(&key))
            .or_else(|| self.args.get(lua_idx - 1))
    }

    /// Looks up a named/associative argument on `args` (e.g. `boxId`, `brid`, `token`).
    pub fn named_arg(&self, key: &str) -> Option<&DtoValue> {
        self.named_args
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .or_else(|| self.raw_args.as_ref().and_then(|r| r.get(key)))
    }

    pub fn to_dto(&self) -> DtoValue {
        let args_dto = match &self.raw_args {
            Some(raw) => raw.clone(),
            None if self.arg_zero.is_none() && self.named_args.is_empty() => {
                DtoValue::Array(self.args.clone())
            }
            None => {
                let mut m = BTreeMap::new();
                if let Some(z) = &self.arg_zero {
                    m.insert("0".to_owned(), z.clone());
                }
                for (idx, v) in self.args.iter().enumerate() {
                    m.insert((idx + 1).to_string(), v.clone());
                }
                for (k, v) in &self.named_args {
                    m.insert(k.clone(), v.clone());
                }
                DtoValue::Map(m)
            }
        };
        DtoValue::map([
            ("method", DtoValue::from(self.method.as_str())),
            ("args", args_dto),
            ("token", DtoValue::from(self.token.as_str())),
            (
                "callback",
                DtoValue::map([(
                    "callbackName",
                    DtoValue::from(self.callback_name.as_str()),
                )]),
            ),
        ])
    }
}

/// Server RPC response envelope (`{ err = "ok", callback = { callbackName }, ret, time, token }`).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcResponse {
    pub err: String,
    pub callback_name: String,
    pub ret: DtoValue,
    pub time: u64,
    pub token: String,
}

impl RpcResponse {
    pub fn ok(req: &RpcRequest, ret: DtoValue, time: u64) -> Self {
        Self {
            err: "ok".to_owned(),
            callback_name: req.callback_name.clone(),
            ret,
            time,
            token: req.token.clone(),
        }
    }

    pub fn error(req: &RpcRequest, err_code: impl Into<String>, time: u64) -> Self {
        Self {
            err: err_code.into(),
            callback_name: req.callback_name.clone(),
            ret: DtoValue::Null,
            time,
            token: req.token.clone(),
        }
    }

    pub fn to_dto(&self) -> DtoValue {
        DtoValue::map([
            ("err", DtoValue::from(self.err.as_str())),
            (
                "callback",
                DtoValue::map([(
                    "callbackName",
                    DtoValue::from(self.callback_name.as_str()),
                )]),
            ),
            ("ret", self.ret.clone()),
            ("time", DtoValue::from(self.time)),
            ("token", DtoValue::from(self.token.as_str())),
        ])
    }
}

/// Server-to-client asynchronous push envelope (`callbackName` = push channel, `err = "ok"`).
#[derive(Debug, Clone, PartialEq)]
pub struct PushPacket {
    pub callback_name: String,
    pub ret: DtoValue,
    pub time: u64,
}

impl PushPacket {
    pub fn new(callback_name: impl Into<String>, ret: DtoValue, time: u64) -> Self {
        Self {
            callback_name: callback_name.into(),
            ret,
            time,
        }
    }

    pub fn to_dto(&self) -> DtoValue {
        DtoValue::map([
            ("err", DtoValue::from("ok")),
            (
                "callback",
                DtoValue::map([(
                    "callbackName",
                    DtoValue::from(self.callback_name.as_str()),
                )]),
            ),
            ("ret", self.ret.clone()),
            ("time", DtoValue::from(self.time)),
        ])
    }
}

/// Bundles an RPC response + 0..N live pushes into a `multi = 1` batch envelope
/// supported natively by `Network:onRead` (`flag[3] != 0`).
///
/// State-sync pushes (`re.[tm]Hero.new`, `[tm]Bag.update`, `sc.recruitment.addnum`) are placed
/// **BEFORE** `response` (`B-09`), while `[tm]User.updateUser` is placed **AFTER** `response` (`B-12`).
pub fn bundle_multi_envelope(
    response: &RpcResponse,
    pushes: &[PushPacket],
    time: u64,
) -> DtoValue {
    let mut items = Vec::with_capacity(1 + pushes.len());
    let mut post_items = Vec::new();
    for p in pushes {
        if matches!(p.callback_name.as_str(), "user.updateUser" | "tmUser.updateUser") {
            post_items.push(p.to_dto());
        } else {
            items.push(p.to_dto());
        }
    }
    items.push(response.to_dto());
    items.extend(post_items);
    DtoValue::map([
        ("err", DtoValue::from("ok")),
        ("ret", DtoValue::Array(items)),
        ("time", DtoValue::from(time)),
    ])
}

// ============================================================================
// Pure-Rust AMF3 Serializer & Deserializer (matching Amf3Module.cpp)
// ============================================================================

const AMF3_NULL: u8 = 0x01;
const AMF3_FALSE: u8 = 0x02;
const AMF3_TRUE: u8 = 0x03;
const AMF3_INTEGER: u8 = 0x04;
const AMF3_DOUBLE: u8 = 0x05;
const AMF3_STRING: u8 = 0x06;
const AMF3_ARRAY: u8 = 0x09;
const AMF3_OBJECT: u8 = 0x0A;
const AMF3_BYTE_ARRAY: u8 = 0x0C;

const AMF3_INT29_MIN: i64 = -(1 << 28);
const AMF3_INT29_MAX: i64 = (1 << 28) - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    UnexpectedEof,
    UnsupportedMarker(u8),
    InvalidUtf8,
    InvalidHeader,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected EOF while decoding AMF3"),
            Self::UnsupportedMarker(m) => write!(f, "unsupported AMF3 type marker 0x{m:02X}"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 string in AMF3 stream"),
            Self::InvalidHeader => write!(f, "invalid LCPacket 24-byte header"),
        }
    }
}

fn write_u29(out: &mut Vec<u8>, val: u32) {
    let v = val & 0x1FFF_FFFF;
    if v < 0x80 {
        out.push(v as u8);
    } else if v < 0x4000 {
        out.push(((v >> 7) as u8 & 0x7F) | 0x80);
        out.push((v & 0x7F) as u8);
    } else if v < 0x20_0000 {
        out.push(((v >> 14) as u8 & 0x7F) | 0x80);
        out.push(((v >> 7) as u8 & 0x7F) | 0x80);
        out.push((v & 0x7F) as u8);
    } else {
        out.push(((v >> 22) as u8 & 0x7F) | 0x80);
        out.push(((v >> 15) as u8 & 0x7F) | 0x80);
        out.push(((v >> 8) as u8 & 0x7F) | 0x80);
        out.push((v & 0xFF) as u8);
    }
}

fn read_u29(buf: &[u8], pos: &mut usize) -> Result<u32, CodecError> {
    let mut acc: u32 = 0;
    for i in 0..4 {
        if *pos >= buf.len() {
            return Err(CodecError::UnexpectedEof);
        }
        let b = buf[*pos];
        *pos += 1;
        if i < 3 {
            acc = (acc << 7) | u32::from(b & 0x7F);
            if (b & 0x80) == 0 {
                return Ok(acc);
            }
        } else {
            acc = (acc << 8) | u32::from(b);
            return Ok(acc);
        }
    }
    Ok(acc)
}

fn write_amf3_str_raw(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    write_u29(out, ((bytes.len() as u32) << 1) | 1);
    out.extend_from_slice(bytes);
}

fn read_amf3_str_raw(buf: &[u8], pos: &mut usize) -> Result<String, CodecError> {
    let header = read_u29(buf, pos)?;
    let len = (header >> 1) as usize;
    if *pos + len > buf.len() {
        return Err(CodecError::UnexpectedEof);
    }
    let slice = &buf[*pos..*pos + len];
    *pos += len;
    core::str::from_utf8(slice)
        .map(str::to_owned)
        .map_err(|_| CodecError::InvalidUtf8)
}

/// Encodes a `DtoValue` into a standalone AMF3 byte buffer (no reference table aliasing).
pub fn encode_amf3(val: &DtoValue) -> Vec<u8> {
    let mut out = Vec::new();
    encode_amf3_inner(val, &mut out);
    out
}

fn encode_amf3_inner(val: &DtoValue, out: &mut Vec<u8>) {
    match val {
        DtoValue::Null => out.push(AMF3_NULL),
        DtoValue::Bool(false) => out.push(AMF3_FALSE),
        DtoValue::Bool(true) => out.push(AMF3_TRUE),
        DtoValue::Int(i) if (*i >= AMF3_INT29_MIN && *i <= AMF3_INT29_MAX) => {
            out.push(AMF3_INTEGER);
            let u = (*i as u32) & 0x1FFF_FFFF;
            write_u29(out, u);
        }
        DtoValue::Int(i) => {
            out.push(AMF3_DOUBLE);
            out.extend_from_slice(&(*i as f64).to_be_bytes());
        }
        DtoValue::Float(f) => {
            out.push(AMF3_DOUBLE);
            out.extend_from_slice(&f.to_be_bytes());
        }
        DtoValue::Str(s) => {
            out.push(AMF3_STRING);
            write_amf3_str_raw(out, s);
        }
        DtoValue::Bytes(b) => {
            out.push(AMF3_BYTE_ARRAY);
            write_u29(out, ((b.len() as u32) << 1) | 1);
            out.extend_from_slice(b);
        }
        DtoValue::Array(items) => {
            out.push(AMF3_ARRAY);
            write_u29(out, ((items.len() as u32) << 1) | 1);
            out.push(0x01); // empty associative portion
            for item in items {
                encode_amf3_inner(item, out);
            }
        }
        DtoValue::Map(map) => {
            out.push(AMF3_OBJECT);
            // U29O-traits: inline, dynamic, 0 sealed members -> 0b1011 = 0x0B
            write_u29(out, 0x0B);
            write_amf3_str_raw(out, ""); // anonymous class name
            for (k, v) in map {
                write_amf3_str_raw(out, k);
                encode_amf3_inner(v, out);
            }
            write_amf3_str_raw(out, ""); // end of dynamic members
        }
    }
}

/// Decodes a standalone AMF3 byte buffer produced by `encode_amf3` or GServer `amf3.encode`.
pub fn decode_amf3(buf: &[u8]) -> Result<DtoValue, CodecError> {
    let mut pos = 0;
    decode_amf3_inner(buf, &mut pos)
}

fn decode_amf3_inner(buf: &[u8], pos: &mut usize) -> Result<DtoValue, CodecError> {
    if *pos >= buf.len() {
        return Err(CodecError::UnexpectedEof);
    }
    let marker = buf[*pos];
    *pos += 1;
    match marker {
        0x00 | AMF3_NULL => Ok(DtoValue::Null),
        AMF3_FALSE => Ok(DtoValue::Bool(false)),
        AMF3_TRUE => Ok(DtoValue::Bool(true)),
        AMF3_INTEGER => {
            let raw = read_u29(buf, pos)?;
            let signed = if (raw & 0x1000_0000) != 0 {
                (raw | 0xE000_0000) as i32
            } else {
                raw as i32
            };
            Ok(DtoValue::Int(i64::from(signed)))
        }
        AMF3_DOUBLE => {
            if *pos + 8 > buf.len() {
                return Err(CodecError::UnexpectedEof);
            }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buf[*pos..*pos + 8]);
            *pos += 8;
            Ok(DtoValue::Float(f64::from_be_bytes(bytes)))
        }
        AMF3_STRING => Ok(DtoValue::Str(read_amf3_str_raw(buf, pos)?)),
        AMF3_BYTE_ARRAY => {
            let header = read_u29(buf, pos)?;
            let len = (header >> 1) as usize;
            if *pos + len > buf.len() {
                return Err(CodecError::UnexpectedEof);
            }
            let data = buf[*pos..*pos + len].to_vec();
            *pos += len;
            Ok(DtoValue::Bytes(data))
        }
        AMF3_ARRAY => {
            let header = read_u29(buf, pos)?;
            let dense_len = (header >> 1) as usize;
            let mut assoc = BTreeMap::new();
            loop {
                let key = read_amf3_str_raw(buf, pos)?;
                if key.is_empty() {
                    break;
                }
                let val = decode_amf3_inner(buf, pos)?;
                assoc.insert(key, val);
            }
            let mut dense = Vec::with_capacity(dense_len);
            for _ in 0..dense_len {
                dense.push(decode_amf3_inner(buf, pos)?);
            }
            if assoc.is_empty() {
                Ok(DtoValue::Array(dense))
            } else {
                for (idx, item) in dense.into_iter().enumerate() {
                    assoc.insert((idx + 1).to_string(), item);
                }
                Ok(DtoValue::Map(assoc))
            }
        }
        AMF3_OBJECT => {
            let traits = read_u29(buf, pos)?;
            let is_dynamic = (traits & 0x08) != 0;
            let sealed_count = (traits >> 4) as usize;
            let _class_name = read_amf3_str_raw(buf, pos)?;
            let mut sealed_names = Vec::with_capacity(sealed_count);
            for _ in 0..sealed_count {
                sealed_names.push(read_amf3_str_raw(buf, pos)?);
            }
            let mut map = BTreeMap::new();
            for name in sealed_names {
                let val = decode_amf3_inner(buf, pos)?;
                map.insert(name, val);
            }
            if is_dynamic {
                loop {
                    let key = read_amf3_str_raw(buf, pos)?;
                    if key.is_empty() {
                        break;
                    }
                    let val = decode_amf3_inner(buf, pos)?;
                    map.insert(key, val);
                }
            }
            Ok(DtoValue::Map(map))
        }
        other => Err(CodecError::UnsupportedMarker(other)),
    }
}

/// 24-byte `LCPacket` wire header (`LCPacket.h` / `Network.lua`).
///
/// Layout:
/// - `[0..4]`: Big-endian `u32` body length (excluding the 24-byte header unless `include_header` mode).
/// - `[4..8]`: 4 flag bytes `[encrypted, 1, zipped, multi]`.
/// - `[8..24]`: 16-byte signature / checksum tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LcPacketFrame {
    pub flags: [u8; 4],
    pub checksum: [u8; 16],
    pub payload: Vec<u8>,
}

impl LcPacketFrame {
    pub fn uncompressed(payload: Vec<u8>, multi: bool) -> Self {
        Self {
            flags: [0, 1, 0, u8::from(multi)],
            checksum: [0u8; 16],
            payload,
        }
    }

    pub fn to_wire_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24 + self.payload.len());
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.flags);
        out.extend_from_slice(&self.checksum);
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() < 24 {
            return Err(CodecError::InvalidHeader);
        }
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&bytes[0..4]);
        let body_len = u32::from_be_bytes(len_bytes) as usize;
        if bytes.len() < 24 + body_len {
            return Err(CodecError::UnexpectedEof);
        }
        let mut flags = [0u8; 4];
        flags.copy_from_slice(&bytes[4..8]);
        let mut checksum = [0u8; 16];
        checksum.copy_from_slice(&bytes[8..24]);
        let payload = bytes[24..24 + body_len].to_vec();
        Ok(Self {
            flags,
            checksum,
            payload,
        })
    }
}
