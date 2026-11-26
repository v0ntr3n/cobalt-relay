//! Tavern Gacha (`raffle.*`, `tmRaffle.*`, `recruitment.*`, `chat.getLastMainPop` from
//! `RaffleData.lua`, `RaffleProxy.lua`, `RaffleGetPanel.lua`, `RaffleVipPanel.lua`, `raffle.db`,
//! `raffle_hot.db`, `hot.db`, `vip.db`, `current_config.db`), Secondary Raffle Engines
//! (`godarmRaffle.*`, `heroLegendRaffle.*`, `activitySoulStoneRecruit.*`, `activityFamousRaffle.*`,
//! `activityTopAnimalRecruit.*`, `raffleArm.*`), and Unified & Companion Shop Engines
//! (`shop.*`, `tmShop.*`, `timeMachineTrader.*`, `babeltowerShop.*`, limit/weekend/score shops
//! from `BaseShopData.lua`, `BaseShopProxy.lua`, `ShopItem.lua`, `shop.db`, `timeMachine_shop.db`).

use std::collections::{BTreeMap, BTreeSet};

use crate::mail::{grant_rewards_to_player, RewardEntry};
use crate::player::{BagState, PlayerState};
use crate::protocol::{push, DtoValue, PushPacket};

/// Push topic emitted on every Pool 1 / Pool 2 Tavern draw (`ActRecruitmentProxy:onAddNum`).
pub const PUSH_RECRUITMENT_ADD_NUM: &str = "sc.recruitment.addnum";

/// Partitions S2C pushes around an RPC response per `vnsg2` client callback rules (`B-09`, `B-12`):
/// - **Pre-response pushes** (`re.hero.new`, `bag.update`, `sc.recruitment.addnum`, etc.) MUST arrive
///   BEFORE the RPC response so synchronous result panels (`RaffleRedHeroPanel`, `scanHandler`,
///   `resetAllCostTips`, `updateFragNum`) see updated `HerosData` and `BagData`.
/// - **Post-response pushes** (`user.updateUser` / `tmUser.updateUser`) MUST arrive AFTER the RPC
///   response so local client `userdata:addCopper` / `spendManager` adjustments do not double-apply
///   on top of absolute server currency totals.
pub fn partition_pushes_around_response(
    pushes: Vec<PushPacket>,
) -> (Vec<PushPacket>, Vec<PushPacket>) {
    let mut pre_response = Vec::new();
    let mut post_response = Vec::new();
    for pkt in pushes {
        if pkt.callback_name == push::USER_UPDATE || pkt.callback_name == "tmUser.updateUser" {
            post_response.push(pkt);
        } else {
            pre_response.push(pkt);
        }
    }
    (pre_response, post_response)
}

/// Deterministic `XorShift64*` PRNG (`#![forbid(unsafe_code)]`, zero external crates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng64 {
    state: u64,
}

impl Rng64 {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniformly samples in `0..bound` (`bound > 0`).
    pub fn gen_below(&mut self, bound: u32) -> u32 {
        (self.next_u64() % u64::from(bound.max(1))) as u32
    }
}

fn next_midnight_ts(now_ts: u64) -> u64 {
    ((now_ts / 86_400) + 1) * 86_400
}

// ============================================================================
// 1. Main Tavern Recruitment (`raffle.*` / `tmRaffle.*` / `recruitment.*`)
// ============================================================================

/// Tavern recruitment pool IDs matching `raffle.db` rows `1..=4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RafflePool {
    /// Pool 1 (`Rút Chiến Tướng`): Green 42.5%, Blue 50.0%, Purple 7.5%, 300s free CD, token `16011`.
    Warrior = 1,
    /// Pool 2 (`Rút Tướng` / Divine): Blue 65.2%, Purple 30.0%, Orange 4.8%, +10*boom luck/pull, token `16012` or `16013`.
    Divine = 2,
    /// Pool 3 (`Rút Hạn Giờ` / Hot VIP): Hot/Sub-hot Hero Soul Frags (`50xxx`) & Items, +10*boom luck/pull, token `16013`.
    HotLimit = 3,
    /// Pool 4 (`Rút Tướng Đỏ` / Red Hero): Red Hero Soul Frags (`50101..50402`), Orange Frags & Items, token `16013`.
    RedHero = 4,
}

impl RafflePool {
    pub fn from_u32(id: u32) -> Option<Self> {
        match id {
            1 => Some(Self::Warrior),
            2 => Some(Self::Divine),
            3 => Some(Self::HotLimit),
            4 => Some(Self::RedHero),
            _ => None,
        }
    }
}

// Verified hero template pools from `raffle.db` & `timeMachine_hero.db`:
const GREEN_HEROES: &[u32] = &[168, 169, 170, 263, 264, 365, 366, 464, 465];
const BLUE_HEROES: &[u32] = &[153, 154, 155, 251, 253, 356, 357, 454, 455];
const PURPLE_HEROES: &[u32] = &[
    121, 122, 123, 124, 125, 220, 221, 222, 223, 320, 321, 322, 419, 420, 421,
];
const ORANGE_HEROES: &[u32] = &[106, 406, 314, 113, 203, 313, 310, 114, 410, 112, 208];

/// Exact 36 choice heroes in `timeMachine_item_direct.json` row `29002` (Divine Luck thresholds `500` & `1000`).
pub const DIVINE_CHEST_29002_HEROES: &[u32] = &[
    108, 110, 111, 112, 113, 114, 115, 117, 209, 210, 211, 212, 213, 215, 216, 217, 308, 309, 310,
    311, 313, 314, 315, 317, 408, 410, 411, 412, 413, 414, 415, 417, 105, 214, 316, 418,
];

/// Exact 12 12-aptitude orange heroes in `timeMachine_item_direct.json` row `29003` (Divine Luck threshold `1500`).
pub const DIVINE_CHEST_29003_HEROES: &[u32] = &[
    106, 107, 109, 203, 207, 208, 306, 307, 312, 406, 407, 409,
];

/// Verified Red Hero Soul Fragment IDs (`raffle_hot.db` rows `13..=20` & `hot.db` `piece_id`).
/// Fixes **B-06** (`20001..20008` in `raffle_red_hero.db` are Spine Lihui offset IDs, NOT fragments!).
pub const RED_HERO_FRAGS: &[u32] = &[
    50102, 50202, 50302, 50402, 50101, 50201, 50301, 50401,
];

/// Verified Hot VIP Orange Hero Soul Fragment IDs (`raffle_hot.db` rows `1..=12` & `hot.db` `piece_id`).
/// Fixes **B-05** (`400xxx` does not exist and crashes `ItemFactory` Pet lookup!).
pub const ORANGE_HERO_FRAGS: &[u32] = &[
    50116, 50204, 50303, 50403, 50103, 50206, 50304, 50404, 50104, 50205, 50305, 50405,
];

const BONUS_ITEMS: &[u32] = &[17002, 22102, 22202];

/// Verified rotation entry from `raffle_hot.db` + `hot.db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotRotationEntry {
    pub raffle_hot_id: u32,
    pub main_htid: u32,
    pub main_frag_tid: u32,
    pub other1_frags: [u32; 3],
    pub other2_frags: [u32; 7],
}

/// Looks up a Hot VIP rotation entry (`pos = 0..=11` -> `raffle_hot_id = 1..=12`).
pub fn hot_rotation_entry(pos: u32) -> &'static HotRotationEntry {
    &HOT_ROTATION_TABLE[(pos as usize) % HOT_ROTATION_TABLE.len()]
}

/// Looks up a Red Hero rotation entry (`pos = 0..=7` -> `raffle_hot_id = 13..=20`).
pub fn red_rotation_entry(pos: u32) -> &'static HotRotationEntry {
    &RED_ROTATION_TABLE[(pos as usize) % RED_ROTATION_TABLE.len()]
}

const WEI_O1: [u32; 3] = [50106, 50107, 50109];
const SHU_O1: [u32; 3] = [50203, 50207, 50208];
const WU_O1: [u32; 3] = [50312, 50307, 50306];
const QUN_O1: [u32; 3] = [50409, 50406, 50407];

const WEI_O2: [u32; 7] = [50108, 50110, 50111, 50112, 50113, 50114, 50115];
const SHU_O2: [u32; 7] = [50209, 50210, 50211, 50212, 50213, 50215, 50216];
const WU_O2: [u32; 7] = [50308, 50309, 50310, 50311, 50313, 50314, 50315];
const QUN_O2: [u32; 7] = [50408, 50410, 50411, 50412, 50413, 50414, 50415];

/// Exact 12 Hot VIP entries (`raffle.db[3].hot = [1..=12]`, `pos = 0..=11`) from `raffle_hot.json` + `hot.json`.
const HOT_ROTATION_TABLE: [HotRotationEntry; 12] = [
    HotRotationEntry { raffle_hot_id: 1, main_htid: 116, main_frag_tid: 50116, other1_frags: WEI_O1, other2_frags: WEI_O2 },
    HotRotationEntry { raffle_hot_id: 2, main_htid: 204, main_frag_tid: 50204, other1_frags: SHU_O1, other2_frags: SHU_O2 },
    HotRotationEntry { raffle_hot_id: 3, main_htid: 303, main_frag_tid: 50303, other1_frags: WU_O1, other2_frags: WU_O2 },
    HotRotationEntry { raffle_hot_id: 4, main_htid: 403, main_frag_tid: 50403, other1_frags: QUN_O1, other2_frags: QUN_O2 },
    HotRotationEntry { raffle_hot_id: 5, main_htid: 103, main_frag_tid: 50103, other1_frags: WEI_O1, other2_frags: WEI_O2 },
    HotRotationEntry { raffle_hot_id: 6, main_htid: 206, main_frag_tid: 50206, other1_frags: SHU_O1, other2_frags: SHU_O2 },
    HotRotationEntry { raffle_hot_id: 7, main_htid: 304, main_frag_tid: 50304, other1_frags: WU_O1, other2_frags: WU_O2 },
    HotRotationEntry { raffle_hot_id: 8, main_htid: 404, main_frag_tid: 50404, other1_frags: QUN_O1, other2_frags: QUN_O2 },
    HotRotationEntry { raffle_hot_id: 9, main_htid: 104, main_frag_tid: 50104, other1_frags: WEI_O1, other2_frags: WEI_O2 },
    HotRotationEntry { raffle_hot_id: 10, main_htid: 205, main_frag_tid: 50205, other1_frags: SHU_O1, other2_frags: SHU_O2 },
    HotRotationEntry { raffle_hot_id: 11, main_htid: 305, main_frag_tid: 50305, other1_frags: WU_O1, other2_frags: WU_O2 },
    HotRotationEntry { raffle_hot_id: 12, main_htid: 405, main_frag_tid: 50405, other1_frags: QUN_O1, other2_frags: QUN_O2 },
];

/// Exact 8 Red Hero entries (`raffle.db[4].hot = [13..=20]`, `pos = 0..=7`) from `raffle_hot.json` + `hot.json`.
const RED_ROTATION_TABLE: [HotRotationEntry; 8] = [
    HotRotationEntry { raffle_hot_id: 13, main_htid: 102, main_frag_tid: 50102, other1_frags: WEI_O1, other2_frags: WEI_O2 },
    HotRotationEntry { raffle_hot_id: 14, main_htid: 202, main_frag_tid: 50202, other1_frags: SHU_O1, other2_frags: SHU_O2 },
    HotRotationEntry { raffle_hot_id: 15, main_htid: 302, main_frag_tid: 50302, other1_frags: WU_O1, other2_frags: WU_O2 },
    HotRotationEntry { raffle_hot_id: 16, main_htid: 402, main_frag_tid: 50402, other1_frags: QUN_O1, other2_frags: QUN_O2 },
    HotRotationEntry { raffle_hot_id: 17, main_htid: 101, main_frag_tid: 50101, other1_frags: WEI_O1, other2_frags: WEI_O2 },
    HotRotationEntry { raffle_hot_id: 18, main_htid: 201, main_frag_tid: 50201, other1_frags: SHU_O1, other2_frags: SHU_O2 },
    HotRotationEntry { raffle_hot_id: 19, main_htid: 301, main_frag_tid: 50301, other1_frags: WU_O1, other2_frags: WU_O2 },
    HotRotationEntry { raffle_hot_id: 20, main_htid: 401, main_frag_tid: 50401, other1_frags: QUN_O1, other2_frags: QUN_O2 },
];

/// `vip.db` `free_raffle_cd_release` subtracted from Pool 1's `300`s free draw cooldown (`B-17`).
pub fn vip_free_raffle_cd_release(vip: u32) -> u64 {
    match vip {
        0 | 1 => 0,
        2 | 3 => 60,
        4 | 5 => 120,
        6 | 7 => 180,
        8 | 9 => 240,
        10 | 11 => 360,
        _ => 480,
    }
}

/// `vip.db` `open_first_pick` (`0` for VIP 0..=4, `1` for VIP 5..=16) (`B-17`).
pub const fn vip_open_first_pick(vip: u32) -> bool {
    vip >= 5
}

/// `vip.db` `limit_raffle_times` (`(hot_daily_cap, red_daily_cap)`) (`B-14`).
pub const fn vip_limit_raffle_caps(vip: u32) -> (u32, u32) {
    match vip {
        0 => (30, 15),
        1 => (40, 20),
        2 => (50, 25),
        3 => (60, 30),
        4 => (70, 40),
        5 => (80, 45),
        6 => (90, 50),
        7 => (100, 55),
        8 => (110, 60),
        9 => (125, 65),
        10 => (140, 70),
        11 => (160, 75),
        12 => (180, 78),
        13 => (200, 80),
        _ => (200, 100),
    }
}

/// `vip.db` `goldarm_recruit_times` / `heroLegend_recruit_times` (`50..=100`).
pub const fn vip_secondary_recruit_cap(vip: u32) -> u32 {
    match vip {
        0 | 1 => 50,
        2 | 3 => 60,
        4 | 5 => 70,
        6 | 7 => 80,
        8 | 9 => 90,
        _ => 100,
    }
}

/// Marquee entry served by `chat.getLastMainPop` (`RaffleMessageItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaffleMarqueeEntry {
    pub id: u64,
    pub uname: String,
    pub time: u64,
    pub raffle_type: u32,
    pub hero_htid: Option<u32>,
    pub item_tid: Option<u32>,
}

/// Per-player tavern gacha progression (`raffle.getInfo` + `recruitment.getInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerRaffleState {
    pub day_bucket: u64,
    pub warrior_free_ready_at: u64,
    pub warrior_free_left_today: u32,
    pub warrior_get_free_time: u64,
    pub divine_free_ready_at: u64,
    pub divine_free_used_today: u32,
    pub divine_get_free_time: u64,
    pub hot_free_ready_at: u64,
    pub hot_free_used: u32,
    pub hot_get_free_time: u64,
    pub red_free_ready_at: u64,
    pub red_free_used: u32,
    pub red_get_free_time: u64,
    pub warrior_pity_counter: u32,
    pub divine_pity_counter: u32,
    pub first_divine_ten_used: bool,
    pub divine_luck: u32,
    pub hot_luck: u32,
    pub red_luck: u32,
    /// Claimed Divine luck thresholds (`500`, `1000`, `1500`).
    pub claimed_divine_boxes: BTreeSet<u32>,
    /// Claimed Hot VIP luck thresholds (`250`, `500`, `750`, `1000`).
    pub claimed_hot_boxes: BTreeSet<u32>,
    /// Claimed Red Hero luck thresholds (`250`, `500`, `750`, `1000`).
    pub claimed_red_boxes: BTreeSet<u32>,
    pub hot_spot_index: u32,
    pub hot_refresh_count: u32,
    pub hot_num_today: u32,
    pub red_num_today: u32,
    /// Two active Hot VIP slots: `(pos_0_to_11, [sub1_0_to_2, sub2_0_to_6])`.
    pub hot_va_hots: [(u32, [u32; 2]); 2],
    pub hot_next_pos: [u32; 2],
    /// Two active Red Hero slots: `(pos_0_to_7, [sub1_0_to_2, sub2_0_to_6])`.
    pub red_va_hots: [(u32, [u32; 2]); 2],
    pub red_next_pos: [u32; 2],
    /// Cumulative draws for `recruitment.getInfo` (`ActRecruitmentProxy`).
    pub recruitment_num: u32,
    pub claimed_recruitment_tiers: BTreeSet<u32>,
}

impl Default for PlayerRaffleState {
    fn default() -> Self {
        Self {
            day_bucket: 0,
            warrior_free_ready_at: 0,
            warrior_free_left_today: 5,
            warrior_get_free_time: 0,
            divine_free_ready_at: 0,
            divine_free_used_today: 0,
            divine_get_free_time: 0,
            hot_free_ready_at: 0,
            hot_free_used: 0,
            hot_get_free_time: 0,
            red_free_ready_at: 0,
            red_free_used: 0,
            red_get_free_time: 0,
            warrior_pity_counter: 0,
            divine_pity_counter: 0,
            first_divine_ten_used: false,
            divine_luck: 0,
            hot_luck: 0,
            red_luck: 0,
            claimed_divine_boxes: BTreeSet::new(),
            claimed_hot_boxes: BTreeSet::new(),
            claimed_red_boxes: BTreeSet::new(),
            hot_spot_index: 0,
            hot_refresh_count: 0,
            hot_num_today: 0,
            red_num_today: 0,
            hot_va_hots: [(0, [0, 0]), (1, [1, 1])],
            hot_next_pos: [2, 3],
            red_va_hots: [(0, [0, 0]), (1, [1, 1])],
            red_next_pos: [2, 3],
            recruitment_num: 0,
            claimed_recruitment_tiers: BTreeSet::new(),
        }
    }
}

impl PlayerRaffleState {
    /// Applies midnight daily reset & 48h (`172_800`s) Hot/Red free recovery ticks (`RaffleData.lua`).
    pub fn sync_timers(&mut self, now_ts: u64) {
        let cur_day = now_ts / 86_400;
        if self.day_bucket == 0 {
            self.day_bucket = cur_day;
            if self.warrior_get_free_time == 0 {
                self.warrior_get_free_time = now_ts;
            }
            if self.divine_get_free_time == 0 {
                self.divine_get_free_time = now_ts;
            }
        } else if cur_day > self.day_bucket {
            self.day_bucket = cur_day;
            self.warrior_free_left_today = 5;
            self.warrior_free_ready_at = 0;
            self.warrior_get_free_time = now_ts;
            self.divine_free_used_today = 0;
            self.divine_free_ready_at = 0;
            self.divine_get_free_time = now_ts;
            self.hot_num_today = 0;
            self.red_num_today = 0;
            self.hot_refresh_count = 0;
            // Advance daily Hot & Red rotations
            self.hot_va_hots[0].0 = self.hot_next_pos[0] % 12;
            self.hot_va_hots[1].0 = self.hot_next_pos[1] % 12;
            self.hot_next_pos = [
                (self.hot_va_hots[0].0 + 2) % 12,
                (self.hot_va_hots[1].0 + 2) % 12,
            ];
            self.red_va_hots[0].0 = self.red_next_pos[0] % 8;
            self.red_va_hots[1].0 = self.red_next_pos[1] % 8;
            self.red_next_pos = [
                (self.red_va_hots[0].0 + 2) % 8,
                (self.red_va_hots[1].0 + 2) % 8,
            ];
        }

        if self.divine_free_used_today > 0 && self.divine_free_ready_at > 0 && now_ts >= self.divine_free_ready_at {
            self.divine_free_used_today = 0;
            self.divine_free_ready_at = 0;
            self.divine_get_free_time = now_ts;
        }
        if self.hot_free_used > 0 && now_ts >= self.hot_free_ready_at {
            self.hot_free_used = 0;
            self.hot_free_ready_at = 0;
        }
        if self.red_free_used > 0 && now_ts >= self.red_free_ready_at {
            self.red_free_used = 0;
            self.red_free_ready_at = 0;
        }
    }
}

/// Tavern Gacha Engine (`raffle.*` / `tmRaffle.*` / `recruitment.*` / `chat.getLastMainPop`)
/// implementing all 28 fixes from `raffle_gacha_audit.md`.
#[derive(Debug, Clone)]
pub struct TavernRaffleEngine {
    rng: Rng64,
    states: BTreeMap<u64, PlayerRaffleState>,
    marquee_seq: u64,
    marquee_log: Vec<RaffleMarqueeEntry>,
}

impl TavernRaffleEngine {
    pub const TOKEN_WARRIOR: u32 = 16011;
    pub const TOKEN_DIVINE: u32 = 16012;
    pub const TOKEN_RECRUIT_SEAL: u32 = 16013;

    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng64::new(seed),
            states: BTreeMap::new(),
            marquee_seq: 1,
            marquee_log: Vec::new(),
        }
    }

    pub fn state_mut(&mut self, uid: u64) -> &mut PlayerRaffleState {
        self.states.entry(uid).or_default()
    }

    /// Exports `PlayerRaffleState` for WAL/profile persistence (`B-19`).
    pub fn export_state(&self, uid: u64) -> Option<PlayerRaffleState> {
        self.states.get(&uid).cloned()
    }

    /// Restores `PlayerRaffleState` from WAL/profile persistence (`B-19`).
    pub fn import_state(&mut self, uid: u64, state: PlayerRaffleState) {
        self.states.insert(uid, state);
    }

    fn record_marquee(
        &mut self,
        uname: &str,
        now_ts: u64,
        raffle_type: u32,
        hero_htid: Option<u32>,
        item_tid: Option<u32>,
    ) {
        let id = self.marquee_seq;
        self.marquee_seq += 1;
        self.marquee_log.push(RaffleMarqueeEntry {
            id,
            uname: uname.to_owned(),
            time: now_ts,
            raffle_type,
            hero_htid,
            item_tid,
        });
        if self.marquee_log.len() > 100 {
            let excess = self.marquee_log.len() - 100;
            self.marquee_log.drain(0..excess);
        }
    }

    /// Returns exact `RaffleData:setRaffleInfo(data)` payload (`B-03`, `B-04`):
    /// complete `raffle` subtable + `hot_raffle` + `red_raffle` with `hot_refresh_time > now_ts`.
    pub fn get_info(&mut self, uid: u64, now_ts: u64) -> DtoValue {
        let st = self.state_mut(uid);
        st.sync_timers(now_ts);

        let primary_used = 5u32.saturating_sub(st.warrior_free_left_today);
        let senior_used = if st.divine_free_used_today > 0 { 1u32 } else { 0u32 };
        let hot_used = if st.hot_free_used > 0 { 1u32 } else { 0u32 };
        let red_used = if st.red_free_used > 0 { 1u32 } else { 0u32 };
        let refresh_ts = next_midnight_ts(now_ts);

        let va_senior: Vec<DtoValue> = st
            .claimed_divine_boxes
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        let va_hot: Vec<DtoValue> = st
            .claimed_hot_boxes
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        let va_red: Vec<DtoValue> = st
            .claimed_red_boxes
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();

        let raffle_sub = DtoValue::map([
            ("primary_free_num", DtoValue::from(primary_used)),
            ("primary_free_time", DtoValue::from(st.warrior_free_ready_at)),
            ("primary_get_free_time", DtoValue::from(st.warrior_get_free_time)),
            ("senior_free_num", DtoValue::from(senior_used)),
            ("senior_free_time", DtoValue::from(st.divine_free_ready_at)),
            ("senior_get_free_time", DtoValue::from(st.divine_get_free_time)),
            ("primary_raffle_num", DtoValue::from(st.warrior_pity_counter)),
            ("senior_raffle_num", DtoValue::from(st.divine_pity_counter)),
            (
                "senior_first_muti_raffle",
                DtoValue::from(u32::from(st.first_divine_ten_used)),
            ),
            ("va_senior_luck_hero", DtoValue::Array(va_senior)),
            ("senior_luck_num", DtoValue::from(st.divine_luck)),
            ("hot_free_num", DtoValue::from(hot_used)),
            ("hot_free_time", DtoValue::from(0u32)),
            ("hot_get_free_time", DtoValue::from(st.hot_get_free_time)),
            ("hot_luck_num", DtoValue::from(st.hot_luck)),
            ("va_hot_luck_hero", DtoValue::Array(va_hot)),
            ("hot_num_today", DtoValue::from(st.hot_num_today)),
            ("red_free_num", DtoValue::from(red_used)),
            ("red_free_time", DtoValue::from(0u32)),
            ("red_get_free_time", DtoValue::from(st.red_get_free_time)),
            ("red_luck_num", DtoValue::from(st.red_luck)),
            ("va_red_luck_hero", DtoValue::Array(va_red)),
            ("red_num_today", DtoValue::from(st.red_num_today)),
        ]);

        let hot_raffle = DtoValue::map([
            (
                "va_hots",
                DtoValue::array(st.hot_va_hots.iter().map(|&(pos, hp)| {
                    DtoValue::map([
                        ("pos", DtoValue::from(pos % 12)),
                        (
                            "hot_pos",
                            DtoValue::array([DtoValue::from(hp[0] % 3), DtoValue::from(hp[1] % 7)]),
                        ),
                    ])
                })),
            ),
            (
                "next_hot_pos",
                DtoValue::array([
                    DtoValue::from(st.hot_next_pos[0] % 12),
                    DtoValue::from(st.hot_next_pos[1] % 12),
                ]),
            ),
            ("hot_refresh_time", DtoValue::from(refresh_ts)),
        ]);

        let red_raffle = DtoValue::map([
            (
                "va_hots",
                DtoValue::array(st.red_va_hots.iter().map(|&(pos, hp)| {
                    DtoValue::map([
                        ("pos", DtoValue::from(pos % 8)),
                        (
                            "hot_pos",
                            DtoValue::array([DtoValue::from(hp[0] % 3), DtoValue::from(hp[1] % 7)]),
                        ),
                    ])
                })),
            ),
            (
                "next_hot_pos",
                DtoValue::array([
                    DtoValue::from(st.red_next_pos[0] % 8),
                    DtoValue::from(st.red_next_pos[1] % 8),
                ]),
            ),
            ("hot_refresh_time", DtoValue::from(refresh_ts)),
        ]);

        DtoValue::map([
            ("raffle", raffle_sub),
            ("hot_raffle", hot_raffle),
            ("red_raffle", red_raffle),
        ])
    }

    fn roll_warrior(rng: &mut Rng64, force_purple: bool) -> u32 {
        if force_purple {
            return PURPLE_HEROES[rng.gen_below(PURPLE_HEROES.len() as u32) as usize];
        }
        // `raffle_probability` row 1: Purple 7.5% (75/1000), Blue 50.0% (500/1000), Green 42.5% (425/1000)
        let r = rng.gen_below(1000);
        if r < 75 {
            PURPLE_HEROES[rng.gen_below(PURPLE_HEROES.len() as u32) as usize]
        } else if r < 575 {
            BLUE_HEROES[rng.gen_below(BLUE_HEROES.len() as u32) as usize]
        } else {
            GREEN_HEROES[rng.gen_below(GREEN_HEROES.len() as u32) as usize]
        }
    }

    fn roll_divine(rng: &mut Rng64, force_orange: bool) -> u32 {
        if force_orange {
            return ORANGE_HEROES[rng.gen_below(ORANGE_HEROES.len() as u32) as usize];
        }
        // `raffle_probability` row 2: Orange 4.8% (48/1000), Purple 30.0% (300/1000), Blue 65.2% (652/1000)
        let r = rng.gen_below(1000);
        if r < 48 {
            ORANGE_HEROES[rng.gen_below(ORANGE_HEROES.len() as u32) as usize]
        } else if r < 348 {
            PURPLE_HEROES[rng.gen_below(PURPLE_HEROES.len() as u32) as usize]
        } else {
            BLUE_HEROES[rng.gen_below(BLUE_HEROES.len() as u32) as usize]
        }
    }

    /// Returns `(luck_gain, boom_mult)` from `raffle.db` row 2 `luck_boom = "1|7000, 2|2000, 4|1000"`.
    fn roll_divine_luck(rng: &mut Rng64) -> (u32, u32) {
        let r = rng.gen_below(10_000);
        let mult = if r < 7_000 {
            1
        } else if r < 9_000 {
            2
        } else {
            4
        };
        (10 * mult, mult)
    }

    /// Returns `(luck_gain, boom_mult)` from `raffle.db` row 3 / `godarm.db` / `heroLegend_raffle.db`
    /// `luck_boom = "1|8500, 2|1000, 4|500"`.
    fn roll_8500_luck(rng: &mut Rng64) -> (u32, u32) {
        let r = rng.gen_below(10_000);
        let mult = if r < 8_500 {
            1
        } else if r < 9_500 {
            2
        } else {
            4
        };
        (10 * mult, mult)
    }

    /// Executes `raffle.autoRaffle` (`args = [1]` at `level > 90`), consuming all `1..=5` remaining
    /// Warrior free draws at once (`B-02`).
    pub fn draw_auto_warrior(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let remaining = {
            let st = self.states.entry(player.uid).or_default();
            st.sync_timers(now_ts);
            if st.warrior_free_left_today == 0 || now_ts < st.warrior_free_ready_at {
                return Err("no_free_warrior_draws");
            }
            st.warrior_free_left_today
        };
        self.draw_standard(player, RafflePool::Warrior, remaining, true, now_ts)
    }

    /// Executes recruitment on Pool 1 (`Warrior`) or Pool 2 (`Divine`) supporting `1..=10` pulls
    /// (`raffle.singleRaffle`, `raffle.autoRaffle`, `raffle.multiRaffle` — `B-01`, `B-02`, `B-11`, `B-12`, `B-17`, `B-18`).
    pub fn draw_standard(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        count: u32,
        allow_free: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=10).contains(&count) {
            return Err("invalid_draw_count");
        }
        if !matches!(pool, RafflePool::Warrior | RafflePool::Divine) {
            return Err("unsupported_standard_pool");
        }

        let mut bag_deltas = Vec::new();
        let mut gold_spent = false;

        {
            let st = self.states.entry(player.uid).or_default();
            st.sync_timers(now_ts);
            match pool {
                RafflePool::Warrior => {
                    if allow_free
                        && st.warrior_free_left_today >= count
                        && now_ts >= st.warrior_free_ready_at
                    {
                        st.warrior_free_left_today -= count;
                        if count == 1 && st.warrior_free_left_today > 0 {
                            let cd = 300u64.saturating_sub(vip_free_raffle_cd_release(player.vip));
                            st.warrior_free_ready_at = now_ts + cd;
                        } else if st.warrior_free_left_today == 0 {
                            st.warrior_free_ready_at = next_midnight_ts(now_ts);
                        }
                    } else if player.bag.count_by_tid(Self::TOKEN_WARRIOR) >= count {
                        bag_deltas.extend(player.bag.consume_item(Self::TOKEN_WARRIOR, count)?);
                    } else {
                        return Err("not_enough_warrior_tokens");
                    }
                }
                RafflePool::Divine => {
                    if count == 1
                        && allow_free
                        && st.divine_free_used_today == 0
                        && now_ts >= st.divine_free_ready_at
                    {
                        st.divine_free_used_today = 1;
                        st.divine_free_ready_at = next_midnight_ts(now_ts);
                    } else if player.bag.count_by_tid(Self::TOKEN_DIVINE) >= count {
                        bag_deltas.extend(player.bag.consume_item(Self::TOKEN_DIVINE, count)?);
                    } else {
                        // `raffle.db` row 2: 1x = 300 `16013`, 10x = 900 `16013` (first 10x at VIP >= 5) else 2850 `16013`
                        let is_first_ten_discount =
                            count == 10 && !st.first_divine_ten_used && vip_open_first_pick(player.vip);
                        let seal_cost: u32 = if count == 1 {
                            300
                        } else if is_first_ten_discount {
                            900
                        } else if count == 10 {
                            2_850
                        } else {
                            300 * count
                        };

                        if player.bag.count_by_tid(Self::TOKEN_RECRUIT_SEAL) >= seal_cost {
                            bag_deltas.extend(
                                player
                                    .bag
                                    .consume_item(Self::TOKEN_RECRUIT_SEAL, seal_cost)?,
                            );
                            if count == 10 {
                                st.first_divine_ten_used = true;
                            }
                        } else if player.gold >= i64::from(seal_cost) {
                            player.gold -= i64::from(seal_cost);
                            gold_spent = true;
                            if count == 10 {
                                st.first_divine_ten_used = true;
                            }
                        } else {
                            return Err("not_enough_gold_or_tokens");
                        }
                    }
                }
                _ => unreachable!(),
            }
        }

        // Grant copper bonus from `raffle.db` (`copper_one = 1000/2000`, `copper_ten = 10000/20000`)
        let unit_copper = if pool == RafflePool::Warrior { 1_000 } else { 2_000 };
        let bonus_copper = unit_copper * i64::from(count);
        player.copper = (player.copper + bonus_copper).min(PlayerState::COPPER_CAP);

        let mut pre_pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pre_pushes.push(bp);
        }

        let mut reward_list = Vec::with_capacity(count as usize);
        let mut last_boom = 0u32;
        let mut boom_count = 0u32;
        let mut marquee_heroes = Vec::new();

        for _ in 0..count {
            let htid = {
                let st = self.states.entry(player.uid).or_default();
                st.recruitment_num = st.recruitment_num.saturating_add(1);
                match pool {
                    RafflePool::Warrior => {
                        st.warrior_pity_counter += 1;
                        // `base_count = 1`: `(primary_raffle_num + 1) % 10 == 0` guarantees purple!
                        let force = (st.warrior_pity_counter + 1).is_multiple_of(10)
                            || st.warrior_pity_counter.is_multiple_of(10);
                        Self::roll_warrior(&mut self.rng, force)
                    }
                    RafflePool::Divine => {
                        st.divine_pity_counter += 1;
                        // `base_count = 3`: `(senior_raffle_num + 3) % 10 == 0` OR 10th pull guarantees orange!
                        let force = (st.divine_pity_counter + 3).is_multiple_of(10)
                            || st.divine_pity_counter.is_multiple_of(10);
                        let (gain, mult) = Self::roll_divine_luck(&mut self.rng);
                        st.divine_luck = (st.divine_luck + gain).min(1_500);
                        last_boom = mult;
                        if mult > 1 {
                            boom_count += 1;
                        }
                        Self::roll_divine(&mut self.rng, force)
                    }
                    _ => unreachable!(),
                }
            };

            let (hero, hero_push) = player.roster.recruit_hero_to_bench(htid, 1, now_ts);
            if hero.quality >= 5 {
                marquee_heroes.push(hero.htid);
            }
            pre_pushes.push(hero_push);
            reward_list.push(DtoValue::map([(
                hero.htid.to_string(),
                DtoValue::from(hero.hid),
            )]));
        }

        let rtype = pool as u32;
        for m_htid in marquee_heroes {
            self.record_marquee(&player.uname, now_ts, rtype, Some(m_htid), None);
        }

        // Recruitment event counter delta push (`ActRecruitmentProxy:onAddNum`)
        pre_pushes.push(PushPacket::new(
            PUSH_RECRUITMENT_ADD_NUM,
            DtoValue::from(count),
            now_ts,
        ));

        // `user.updateUser` placed AFTER `re.hero.new` and `bag.update` (`B-12`)
        let mut user_fields = vec![
            ("copper", DtoValue::from(player.copper)),
            ("fight_force", DtoValue::from(player.fight_force())),
        ];
        if gold_spent {
            user_fields.push(("gold", DtoValue::from(player.gold)));
        }
        pre_pushes.push(PlayerState::build_user_update_push(user_fields, now_ts));

        let cur_luck = self.states.get(&player.uid).map_or(0, |s| s.divine_luck);
        let ret = if count == 1 {
            DtoValue::map([
                ("reward", DtoValue::Array(reward_list)),
                ("senior_luck_num", DtoValue::from(cur_luck)),
                ("luck_boom", DtoValue::from(last_boom)),
            ])
        } else {
            DtoValue::map([
                ("reward", DtoValue::Array(reward_list)),
                ("senior_luck_num", DtoValue::from(cur_luck)),
                ("luck_boom_num", DtoValue::from(boom_count)),
            ])
        };
        Ok((ret, pre_pushes))
    }

    /// Explicit Hot VIP / Red Hero draw taking 0-based `pos` (`0` or `1`) and `count` (`1` or `10`)
    /// (`raffle.raffleHot`, `raffle.multiRaffleHot`, `raffle.raffleRed` — `B-05`, `B-06`, `B-13`, `B-14`).
    pub fn draw_hot_or_red(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        pos: u32,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !matches!(pool, RafflePool::HotLimit | RafflePool::RedHero) || (count != 1 && count != 10)
        {
            return Err("invalid_limit_draw");
        }
        let slot_idx = (pos as usize) % 2;
        let single_cost: u32 = if pool == RafflePool::HotLimit { 300 } else { 500 };
        let needed_seal: u32 = if count == 1 { single_cost } else { 4_000 };
        let (hot_cap, red_cap) = vip_limit_raffle_caps(player.vip);

        let mut bag_deltas = Vec::new();
        let mut spent_gold = false;

        let active_entry = {
            let st = self.states.entry(player.uid).or_default();
            st.sync_timers(now_ts);

            let is_free = match (pool, count) {
                (RafflePool::HotLimit, 1) => st.hot_free_used == 0 && now_ts >= st.hot_free_ready_at,
                (RafflePool::RedHero, 1) => st.red_free_used == 0 && now_ts >= st.red_free_ready_at,
                _ => false,
            };

            if is_free {
                if pool == RafflePool::HotLimit {
                    st.hot_free_used = 1;
                    st.hot_get_free_time = now_ts;
                    st.hot_free_ready_at = now_ts + 172_800;
                } else {
                    st.red_free_used = 1;
                    st.red_get_free_time = now_ts;
                    st.red_free_ready_at = now_ts + 172_800;
                }
            } else {
                // Check VIP daily draw cap (`vip.db` `limit_raffle_times`)
                if pool == RafflePool::HotLimit && st.hot_num_today + count > hot_cap {
                    return Err("exceed_vip_hot_limit");
                }
                if pool == RafflePool::RedHero && st.red_num_today + count > red_cap {
                    return Err("exceed_vip_red_limit");
                }

                if player.bag.count_by_tid(Self::TOKEN_RECRUIT_SEAL) >= needed_seal {
                    bag_deltas.extend(
                        player
                            .bag
                            .consume_item(Self::TOKEN_RECRUIT_SEAL, needed_seal)?,
                    );
                } else if player.gold >= i64::from(needed_seal) {
                    player.gold -= i64::from(needed_seal);
                    spent_gold = true;
                } else {
                    return Err("not_enough_gold_or_seal");
                }

                if pool == RafflePool::HotLimit {
                    st.hot_num_today += count;
                } else {
                    st.red_num_today += count;
                }
            }

            if pool == RafflePool::HotLimit {
                let (hot_pos, sub_pos) = st.hot_va_hots[slot_idx];
                (HOT_ROTATION_TABLE[(hot_pos as usize) % 12], sub_pos)
            } else {
                let (red_pos, sub_pos) = st.red_va_hots[slot_idx];
                (RED_ROTATION_TABLE[(red_pos as usize) % 8], sub_pos)
            }
        };

        // Grant Copper bonus (`copper_one = 3000` for Hot single, `5000` for Red single)
        let bonus_copper = if count == 1 {
            if pool == RafflePool::HotLimit {
                3_000i64
            } else {
                5_000i64
            }
        } else {
            0i64
        };
        if bonus_copper > 0 {
            player.copper = (player.copper + bonus_copper).min(PlayerState::COPPER_CAP);
        }

        let (rot, sub_pos) = active_entry;
        let sub1_frag = rot.other1_frags[(sub_pos[0] as usize) % 3];
        let sub2_frag = rot.other2_frags[(sub_pos[1] as usize) % 7];

        let mut reward_entries = Vec::with_capacity(count as usize);
        let mut reward_dtos = Vec::with_capacity(count as usize);
        let mut last_boom = 1u32;
        let mut boom_count = 0u32;

        for _ in 0..count {
            let st = self.states.entry(player.uid).or_default();
            if pool == RafflePool::HotLimit {
                let (gain, mult) = Self::roll_8500_luck(&mut self.rng);
                st.hot_luck = (st.hot_luck + gain).min(1_000);
                last_boom = mult;
                if mult > 1 {
                    boom_count += 1;
                }
                let r = self.rng.gen_below(100);
                let (tid, qty) = if r < 35 {
                    (rot.main_frag_tid, 5)
                } else if r < 55 {
                    (sub1_frag, 3)
                } else if r < 75 {
                    (sub2_frag, 3)
                } else {
                    (BONUS_ITEMS[self.rng.gen_below(BONUS_ITEMS.len() as u32) as usize], 2)
                };
                reward_entries.push(RewardEntry::Item { tid, qty });
                reward_dtos.push(DtoValue::map([(
                    "item",
                    DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
                )]));
            } else {
                st.red_luck = (st.red_luck + 10).min(1_000);
                last_boom = 1;
                let r = self.rng.gen_below(100);
                let (tid, qty) = if r < 50 {
                    (rot.main_frag_tid, 5)
                } else if r < 65 {
                    (sub1_frag, 3)
                } else if r < 80 {
                    (sub2_frag, 3)
                } else {
                    (BONUS_ITEMS[self.rng.gen_below(BONUS_ITEMS.len() as u32) as usize], 2)
                };
                reward_entries.push(RewardEntry::Item { tid, qty });
                reward_dtos.push(DtoValue::map([(
                    "item",
                    DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
                )]));
            }
        }

        let granted_pushes = grant_rewards_to_player(player, &reward_entries, now_ts)?;
        let (mut pre_pushes, mut post_pushes) = partition_pushes_around_response(granted_pushes);
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pre_pushes.insert(0, bp);
        }
        if spent_gold || bonus_copper > 0 {
            let mut ufields = vec![("copper", DtoValue::from(player.copper))];
            if spent_gold {
                ufields.push(("gold", DtoValue::from(player.gold)));
            }
            post_pushes.push(PlayerState::build_user_update_push(ufields, now_ts));
        }
        pre_pushes.extend(post_pushes);

        let st = self.states.get(&player.uid).unwrap();
        let ret = match (pool, count) {
            (RafflePool::HotLimit, 1) => DtoValue::map([
                ("reward", DtoValue::Array(reward_dtos)),
                ("hot_luck_num", DtoValue::from(st.hot_luck)),
                ("luck_num", DtoValue::from(st.hot_luck)),
                ("luck_boom", DtoValue::from(last_boom)),
            ]),
            (RafflePool::HotLimit, _) => DtoValue::map([
                ("reward", DtoValue::Array(reward_dtos)),
                ("luck_num", DtoValue::from(st.hot_luck)),
                ("hot_luck_num", DtoValue::from(st.hot_luck)),
                ("luck_boom_num", DtoValue::from(boom_count)),
            ]),
            (RafflePool::RedHero, _) => DtoValue::map([
                ("reward", DtoValue::Array(reward_dtos)),
                ("red_luck_num", DtoValue::from(st.red_luck)),
                ("luck_num", DtoValue::from(st.red_luck)),
                ("luck_boom", DtoValue::from(last_boom)),
            ]),
            _ => unreachable!(),
        };
        Ok((ret, pre_pushes))
    }

    /// Backwards-compatible entry point called by `engine.rs` (`B-13`):
    /// - When `pool == RedHero` and `count_or_pos == 0`: interprets `0` as 0-based `pos = 0, count = 1`
    ///   (`RaffleVipPanelRed` sends `args = [0]` for slot 1 and `args = [1]` for slot 2!).
    /// - When `pool == RedHero` and `count_or_pos == 1`: `pos = 1, count = 1`.
    /// - When `count_or_pos == 10`: `pos = 0, count = 10`.
    pub fn draw_limit_or_red(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        count_or_pos: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        match (pool, count_or_pos) {
            (RafflePool::RedHero, 0) => self.draw_hot_or_red(player, pool, 0, 1, now_ts),
            (RafflePool::RedHero, 1) => self.draw_hot_or_red(player, pool, 1, 1, now_ts),
            (_, 1) => self.draw_hot_or_red(player, pool, 0, 1, now_ts),
            (_, 10) => self.draw_hot_or_red(player, pool, 0, 10, now_ts),
            _ => Err("invalid_limit_draw"),
        }
    }

    /// Buys recruitment seal tokens (`raffle.buyToken`, `args = [buy_num]` — `B-10`):
    /// `current_config.db` `buy_token_cost = [1, 16013]` -> **1 Gold = 1 `16013` (`Quân Hưởng`)**!
    pub fn buy_token(
        &mut self,
        player: &mut PlayerState,
        qty: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.buy_token_by_tid(player, Self::TOKEN_RECRUIT_SEAL, qty, now_ts)
    }

    /// Explicit 2-arg variant supporting `(tid, qty)` if a caller passes `{16013, qty}`.
    pub fn buy_token_by_tid(
        &mut self,
        player: &mut PlayerState,
        tid: u32,
        qty: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if qty == 0 {
            return Err("invalid_qty");
        }
        let target_tid = if tid == Self::TOKEN_DIVINE || tid == Self::TOKEN_RECRUIT_SEAL {
            tid
        } else {
            Self::TOKEN_RECRUIT_SEAL
        };
        let cost = i64::from(qty); // 1 Gold per token (`B-10`)
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let deltas = player.bag.add_item(target_tid, qty, now_ts)?;
        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&deltas, now_ts) {
            pushes.push(bp);
        }
        pushes.push(PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        ));
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Claims a luck milestone box (`raffle.receiveLuckHeroGift` `{500|1000|1500, choice_idx}`,
    /// `raffle.receiveHotLuckHeroGift` `{250|500|750|1000, hot_pos}`, or
    /// `raffle.receiveRedLuckHeroGift` `{250|500|750|1000, red_pos}` — `B-15`).
    ///
    /// Returns **`DtoValue::Int(new_luck_num)`** (plain AMF3 integer, as required by
    /// `RaffleProxy.lua:926, 945, 967`!). Also accepts legacy 1-based `1..=4` index for compatibility.
    pub fn receive_luck_box(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        luck_or_idx: u32,
        choice_or_pos: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let (threshold, tier_idx, hot_frag_and_qty) = {
            let st = self.states.entry(player.uid).or_default();
            st.sync_timers(now_ts);
            match pool {
                RafflePool::Divine => {
                    let thresholds = [500u32, 1000, 1500];
                    let (req, idx) = if let Some(pos) = thresholds.iter().position(|&t| t == luck_or_idx) {
                        (luck_or_idx, pos)
                    } else if (1..=3).contains(&luck_or_idx) {
                        let pos = (luck_or_idx - 1) as usize;
                        (thresholds[pos], pos)
                    } else {
                        return Err("invalid_box_index");
                    };
                    if st.divine_luck < req {
                        return Err("luck_not_enough");
                    }
                    if st.claimed_divine_boxes.contains(&req) {
                        return Err("box_already_claimed");
                    }
                    st.claimed_divine_boxes.insert(req);
                    (req, idx, None)
                }
                RafflePool::HotLimit => {
                    let thresholds = [250u32, 500, 750, 1000];
                    let qtys = [10u32, 20, 30, 40];
                    let (req, idx) = if let Some(pos) = thresholds.iter().position(|&t| t == luck_or_idx) {
                        (luck_or_idx, pos)
                    } else if (1..=4).contains(&luck_or_idx) {
                        let pos = (luck_or_idx - 1) as usize;
                        (thresholds[pos], pos)
                    } else {
                        return Err("invalid_box_index");
                    };
                    if st.hot_luck < req {
                        return Err("luck_not_enough");
                    }
                    if st.claimed_hot_boxes.contains(&req) {
                        return Err("box_already_claimed");
                    }
                    st.claimed_hot_boxes.insert(req);
                    let slot = (choice_or_pos as usize) % 2;
                    let hot_pos = (st.hot_va_hots[slot].0 as usize) % 12;
                    let frag_tid = HOT_ROTATION_TABLE[hot_pos].main_frag_tid;
                    (req, idx, Some((frag_tid, qtys[idx])))
                }
                RafflePool::RedHero => {
                    let thresholds = [250u32, 500, 750, 1000];
                    let qtys = [20u32, 35, 45, 60];
                    let (req, idx) = if let Some(pos) = thresholds.iter().position(|&t| t == luck_or_idx) {
                        (luck_or_idx, pos)
                    } else if (1..=4).contains(&luck_or_idx) {
                        let pos = (luck_or_idx - 1) as usize;
                        (thresholds[pos], pos)
                    } else {
                        return Err("invalid_box_index");
                    };
                    if st.red_luck < req {
                        return Err("luck_not_enough");
                    }
                    if st.claimed_red_boxes.contains(&req) {
                        return Err("box_already_claimed");
                    }
                    st.claimed_red_boxes.insert(req);
                    let slot = (choice_or_pos as usize) % 2;
                    let red_pos = (st.red_va_hots[slot].0 as usize) % 8;
                    let frag_tid = RED_ROTATION_TABLE[red_pos].main_frag_tid;
                    (req, idx, Some((frag_tid, qtys[idx])))
                }
                RafflePool::Warrior => return Err("pool_has_no_luck_box"),
            }
        };
        let _ = (threshold, tier_idx);

        let mut pushes = Vec::new();
        match pool {
            RafflePool::Divine => {
                // `29002` (36 choices) for 500 & 1000; `29003` (12 choices) for 1500 (`[5, htid, 1]`)
                let choice_pool = if threshold == 1500 {
                    DIVINE_CHEST_29003_HEROES
                } else {
                    DIVINE_CHEST_29002_HEROES
                };
                let chosen_htid = *choice_pool
                    .get(choice_or_pos as usize)
                    .ok_or("invalid_hero_choice_index")?;
                let (_, hero_push) = player.roster.recruit_hero_to_bench(chosen_htid, 1, now_ts);
                pushes.push(hero_push);
            }
            RafflePool::HotLimit | RafflePool::RedHero => {
                let (frag_tid, qty) = hot_frag_and_qty.unwrap();
                let deltas = player.bag.add_item(frag_tid, qty, now_ts)?;
                if let Some(bp) = BagState::build_update_push(&deltas, now_ts) {
                    pushes.push(bp);
                }
            }
            RafflePool::Warrior => unreachable!(),
        }

        // Cycle reset once all milestones in the cycle are claimed (`-1500` Divine, `-1000` Hot/Red)
        let new_luck = {
            let st = self.states.get_mut(&player.uid).unwrap();
            match pool {
                RafflePool::Divine => {
                    if st.claimed_divine_boxes.len() >= 3 {
                        st.divine_luck = st.divine_luck.saturating_sub(1_500);
                        st.claimed_divine_boxes.clear();
                    }
                    st.divine_luck
                }
                RafflePool::HotLimit => {
                    if st.claimed_hot_boxes.len() >= 4 {
                        st.hot_luck = st.hot_luck.saturating_sub(1_000);
                        st.claimed_hot_boxes.clear();
                    }
                    st.hot_luck
                }
                RafflePool::RedHero => {
                    if st.claimed_red_boxes.len() >= 4 {
                        st.red_luck = st.red_luck.saturating_sub(1_000);
                        st.claimed_red_boxes.clear();
                    }
                    st.red_luck
                }
                RafflePool::Warrior => 0,
            }
        };

        Ok((DtoValue::from(new_luck), pushes))
    }

    /// Backwards-compatible 1-arg wrapper delegating to [`Self::receive_luck_box`].
    pub fn claim_luck_box(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        box_idx: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.receive_luck_box(player, pool, box_idx, 0, now_ts)
    }

    /// Manual Hot VIP rotation (`raffle.refreshHotByHand` — `B-16`):
    /// Stepped cost `[[1, 100], [2, 200], [999, 300]]` Gold from `raffle.db` row 3.
    pub fn refresh_hot_by_hand(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let next_n = self
            .states
            .entry(player.uid)
            .or_default()
            .hot_refresh_count
            + 1;
        let cost: i64 = match next_n {
            1 => 100,
            2 => 200,
            _ => 300,
        };
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;

        {
            let st = self.states.entry(player.uid).or_default();
            st.sync_timers(now_ts);
            st.hot_refresh_count = next_n;
            let p0 = st.hot_next_pos[0] % 12;
            let p1 = st.hot_next_pos[1] % 12;
            let s10 = self.rng.gen_below(3);
            let s20 = self.rng.gen_below(7);
            let s11 = self.rng.gen_below(3);
            let s21 = self.rng.gen_below(7);
            st.hot_va_hots = [(p0, [s10, s20]), (p1, [s11, s21])];
            st.hot_next_pos = [(p0 + 2) % 12, (p1 + 2) % 12];
            st.hot_spot_index = p0;
        }

        let mut info = self.get_info(player.uid, now_ts);
        if let Some(hot_sub) = info.get("hot_raffle").cloned() {
            if let (Some(va_hots), Some(next_pos), DtoValue::Map(map)) = (
                hot_sub.get("va_hots").cloned(),
                hot_sub.get("next_hot_pos").cloned(),
                &mut info,
            ) {
                map.insert("va_hots".to_owned(), va_hots);
                map.insert("next_hot_pos".to_owned(), next_pos);
            }
        }
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((info, pushes))
    }

    /// `recruitment.getInfo` (`B-20`): `{ num = <int>, reward = [<claimed_1_to_9>] }`.
    pub fn get_recruitment_info(&mut self, uid: u64) -> DtoValue {
        let st = self.state_mut(uid);
        let claimed: Vec<DtoValue> = st
            .claimed_recruitment_tiers
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        DtoValue::map([
            ("num", DtoValue::from(st.recruitment_num)),
            ("reward", DtoValue::Array(claimed)),
        ])
    }

    /// `recruitment.receiveActiveReward` (`args = [tier_idx_1_to_9]` — `B-20`):
    /// Verified against `activity_recruitment.json` `condition = [10, 20, 40, 60, 80, 100, 130, 160, 200]`.
    pub fn receive_recruitment_reward(
        &mut self,
        player: &mut PlayerState,
        tier_idx: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        const CONDITIONS: [u32; 9] = [10, 20, 40, 60, 80, 100, 130, 160, 200];
        if !(1..=9).contains(&tier_idx) {
            return Err("invalid_recruitment_tier");
        }
        let req_num = CONDITIONS[(tier_idx - 1) as usize];
        {
            let st = self.state_mut(player.uid);
            if st.recruitment_num < req_num {
                return Err("recruitment_num_not_enough");
            }
            if st.claimed_recruitment_tiers.contains(&tier_idx) {
                return Err("recruitment_reward_already_claimed");
            }
            st.claimed_recruitment_tiers.insert(tier_idx);
        }

        let rewards: Vec<RewardEntry> = match tier_idx {
            1 => vec![RewardEntry::Gold(100), RewardEntry::Item { tid: 16012, qty: 1 }],
            2 => vec![RewardEntry::Gold(200), RewardEntry::Item { tid: 16012, qty: 2 }],
            _ => vec![RewardEntry::Item { tid: 33006, qty: 1 }],
        };
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        Ok((DtoValue::from("ok"), pushes))
    }

    /// `chat.getLastMainPop` (`args = [offset, limit]` — `B-21`).
    pub fn get_last_main_pop(&self, offset: usize, limit: usize) -> DtoValue {
        let cap = limit.clamp(1, 50);
        let items: Vec<DtoValue> = self
            .marquee_log
            .iter()
            .rev()
            .skip(offset)
            .take(cap)
            .map(|e| {
                let mut va_item = BTreeMap::new();
                if let Some(htid) = e.hero_htid {
                    va_item.insert("1".to_owned(), DtoValue::array([DtoValue::from(htid)]));
                }
                if let Some(tid) = e.item_tid {
                    va_item.insert("2".to_owned(), DtoValue::array([DtoValue::from(tid)]));
                }
                DtoValue::map([
                    ("id", DtoValue::from(e.id)),
                    ("uname", DtoValue::from(e.uname.as_str())),
                    ("time", DtoValue::from(e.time)),
                    ("raffle_type", DtoValue::from(e.raffle_type)),
                    ("va_item", DtoValue::Map(va_item)),
                ])
            })
            .collect();
        DtoValue::Array(items)
    }
}

// ============================================================================
// 2. Secondary Gacha / Raffle Engines (`B-22` through `B-27`)
// ============================================================================

/// Extracts `boxId` from either `{ { boxId = 1..5 } }` (AMF3 Map arg) or `{ 1..5 }` (Int arg).
pub fn extract_box_id_arg(args: &[DtoValue]) -> Option<u32> {
    let first = args.first()?;
    if let Some(id) = first.as_u32() {
        return Some(id);
    }
    first.get("boxId").and_then(DtoValue::as_u32)
}

// --- 2.1 GodArm Raffle (`godarmRaffle.*` / `tmGodarmRaffle.*` — `B-22`) ---

const GODARM_ORANGE_ITEMS: &[u32] = &[
    500401, 500402, 500403, 500404, 500405, 500406, 500407, 500408,
];
const GODARM_PURPLE_ITEMS: &[u32] = &[
    500301, 500302, 500304, 500305, 500306, 500307, 500308,
];
const GODARM_CHOICE_FRAGS: &[u32] = &[
    3_500_401, 3_500_402, 3_500_403, 3_500_404, 3_500_405, 3_500_406, 3_500_407, 3_500_408,
];

#[derive(Debug, Clone, Default)]
pub struct PlayerGodArmRaffleState {
    pub day_bucket: u64,
    pub raffle_num: u32,
    pub free_used: u32,
    pub get_free_time: u64,
    pub luck_num: u32,
    pub gold_num_today: u32,
    pub first_multi_used: bool,
    pub claimed_luck_scores: BTreeSet<u32>,
}

#[derive(Debug, Clone)]
pub struct GodArmRaffleEngine {
    rng: Rng64,
    states: BTreeMap<u64, PlayerGodArmRaffleState>,
}

impl GodArmRaffleEngine {
    pub const TOKEN_GODARM: u32 = 19009;

    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng64::new(seed ^ 0xA11C_E551),
            states: BTreeMap::new(),
        }
    }

    fn sync_state(&mut self, uid: u64, now_ts: u64) -> &mut PlayerGodArmRaffleState {
        let st = self.states.entry(uid).or_default();
        let day = now_ts / 86_400;
        if st.day_bucket == 0 {
            st.day_bucket = day;
            st.get_free_time = now_ts;
        } else if day > st.day_bucket {
            st.day_bucket = day;
            st.gold_num_today = 0;
        }
        // `godarm.db`: `refresh_day = 2` days (`172_800`s)
        if st.free_used > 0 && now_ts >= st.get_free_time + 172_800 {
            st.free_used = 0;
            st.get_free_time = now_ts;
        }
        st
    }

    pub fn get_info(&mut self, uid: u64, now_ts: u64) -> DtoValue {
        let st = self.sync_state(uid, now_ts);
        let claimed: Vec<DtoValue> = st
            .claimed_luck_scores
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        DtoValue::map([
            ("godarm_raffle_num", DtoValue::from(st.raffle_num)),
            ("godarm_free_num", DtoValue::from(st.free_used)),
            ("godarm_get_free_time", DtoValue::from(st.get_free_time)),
            ("godarm_luck_num", DtoValue::from(st.luck_num)),
            ("gold_num_today", DtoValue::from(st.gold_num_today)),
            (
                "godarm_first_muti_raffle",
                DtoValue::from(u32::from(st.first_multi_used)),
            ),
            ("va_godarm_luck_hero", DtoValue::Array(claimed)),
        ])
    }

    pub fn draw(
        &mut self,
        player: &mut PlayerState,
        is_multi: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        // `godarm.db`: `multi_raffle_number = 5`!
        let count: u32 = if is_multi { 5 } else { 1 };
        let gold_cost: i64 = if is_multi { 1_400 } else { 300 };
        let vip_cap = vip_secondary_recruit_cap(player.vip);

        let mut bag_deltas = Vec::new();
        let mut spent_gold = false;

        {
            let st = self.sync_state(player.uid, now_ts);
            if !is_multi && st.free_used == 0 {
                st.free_used = 1;
                st.get_free_time = now_ts;
            } else if player.bag.count_by_tid(Self::TOKEN_GODARM) >= count {
                bag_deltas.extend(player.bag.consume_item(Self::TOKEN_GODARM, count)?);
            } else {
                if st.gold_num_today + count > vip_cap {
                    return Err("exceed_vip_godarm_limit");
                }
                if player.gold < gold_cost {
                    return Err("not_enough_gold");
                }
                player.gold -= gold_cost;
                st.gold_num_today += count;
                if is_multi {
                    st.first_multi_used = true;
                }
                spent_gold = true;
            }
        }

        let bonus_copper = if is_multi { 10_000i64 } else { 2_000i64 };
        player.copper = (player.copper + bonus_copper).min(PlayerState::COPPER_CAP);

        let mut reward_dtos = Vec::with_capacity(count as usize);
        let mut last_boom = 1u32;
        let mut boom_count = 0u32;

        for _ in 0..count {
            let (tid, qty) = {
                let st = self.states.get_mut(&player.uid).unwrap();
                st.raffle_num += 1;
                let force_high = (st.raffle_num + 1).is_multiple_of(5) || st.raffle_num.is_multiple_of(5);
                let (gain, mult) = TavernRaffleEngine::roll_8500_luck(&mut self.rng);
                st.luck_num = (st.luck_num + gain).min(1_500);
                last_boom = mult;
                if mult > 1 {
                    boom_count += 1;
                }
                if force_high {
                    let idx = self.rng.gen_below(GODARM_ORANGE_ITEMS.len() as u32) as usize;
                    (GODARM_ORANGE_ITEMS[idx], 1u32)
                } else if self.rng.gen_below(100) < 40 {
                    let idx = self.rng.gen_below(GODARM_PURPLE_ITEMS.len() as u32) as usize;
                    (GODARM_PURPLE_ITEMS[idx], 1u32)
                } else {
                    let idx = self.rng.gen_below(GODARM_CHOICE_FRAGS.len() as u32) as usize;
                    (GODARM_CHOICE_FRAGS[idx], 5u32)
                }
            };
            bag_deltas.extend(player.bag.add_item(tid, qty, now_ts)?);
            reward_dtos.push(DtoValue::map([(
                "item",
                DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
            )]));
        }

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }
        let mut ufields = vec![("copper", DtoValue::from(player.copper))];
        if spent_gold {
            ufields.push(("gold", DtoValue::from(player.gold)));
        }
        pushes.push(PlayerState::build_user_update_push(ufields, now_ts));

        let cur_luck = self.states[&player.uid].luck_num;
        let ret = if is_multi {
            DtoValue::map([
                ("reward", DtoValue::Array(reward_dtos)),
                ("godarm_luck_num", DtoValue::from(cur_luck)),
                ("luck_boom_num", DtoValue::from(boom_count)),
            ])
        } else {
            DtoValue::map([
                ("reward", DtoValue::Array(reward_dtos)),
                ("godarm_luck_num", DtoValue::from(cur_luck)),
                ("luck_boom", DtoValue::from(last_boom)),
            ])
        };
        Ok((ret, pushes))
    }

    pub fn receive_luck_gift(
        &mut self,
        player: &mut PlayerState,
        luck_score: u32,
        choice_idx: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let req = match luck_score {
            500 | 1000 | 1500 => luck_score,
            1 => 500,
            2 => 1000,
            3 => 1500,
            _ => return Err("invalid_godarm_luck_score"),
        };
        let frag_tid = *GODARM_CHOICE_FRAGS
            .get(choice_idx as usize)
            .ok_or("invalid_godarm_choice")?;
        let qty = if req == 1500 { 40u32 } else { 30u32 };

        let new_luck = {
            let st = self.sync_state(player.uid, now_ts);
            if st.luck_num < req {
                return Err("luck_not_enough");
            }
            if st.claimed_luck_scores.contains(&req) {
                return Err("box_already_claimed");
            }
            st.claimed_luck_scores.insert(req);
            if st.claimed_luck_scores.len() >= 3 {
                st.luck_num = st.luck_num.saturating_sub(1_500);
                st.claimed_luck_scores.clear();
            }
            st.luck_num
        };

        let deltas = player.bag.add_item(frag_tid, qty, now_ts)?;
        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&deltas, now_ts) {
            pushes.push(bp);
        }
        Ok((DtoValue::from(new_luck), pushes))
    }
}

// --- 2.2 Hero Legend Raffle (`heroLegendRaffle.*` / `tmHeroLegendRaffle.*` — `B-23`) ---

#[derive(Debug, Clone)]
pub struct PlayerHeroLegendRaffleState {
    pub day_bucket: u64,
    pub free_remaining: u32,
    pub today_num: u32,
    pub lucky_num: u32,
    pub idx_to_pity: u32,
    pub claimed_boxes: BTreeSet<u32>,
}

impl Default for PlayerHeroLegendRaffleState {
    fn default() -> Self {
        Self {
            day_bucket: 0,
            free_remaining: 1, // `free_num` in `HeroLegendRaffleData` is REMAINING (`0..=1`)!
            today_num: 0,
            lucky_num: 0,
            idx_to_pity: 9,
            claimed_boxes: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeroLegendRaffleEngine {
    rng: Rng64,
    states: BTreeMap<u64, PlayerHeroLegendRaffleState>,
}

impl HeroLegendRaffleEngine {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng64::new(seed ^ 0xB22D_F662),
            states: BTreeMap::new(),
        }
    }

    fn sync_state(&mut self, uid: u64, now_ts: u64) -> &mut PlayerHeroLegendRaffleState {
        let st = self.states.entry(uid).or_default();
        let day = now_ts / 86_400;
        if st.day_bucket == 0 {
            st.day_bucket = day;
        } else if day > st.day_bucket {
            st.day_bucket = day;
            st.free_remaining = 1;
            st.today_num = 0;
        }
        st
    }

    pub fn get_info(&mut self, uid: u64, now_ts: u64) -> DtoValue {
        let st = self.sync_state(uid, now_ts);
        let lucky_box: Vec<DtoValue> = st
            .claimed_boxes
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        DtoValue::map([
            ("lucky_box", DtoValue::Array(lucky_box)),
            ("lucky_num", DtoValue::from(st.lucky_num)),
            ("idx", DtoValue::from(st.idx_to_pity)),
            ("free_num", DtoValue::from(st.free_remaining)),
            ("today_num", DtoValue::from(st.today_num)),
        ])
    }

    pub fn raffle(
        &mut self,
        player: &mut PlayerState,
        times: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if times != 1 && times != 5 {
            return Err("invalid_herolegend_times");
        }
        let all_cost: u64 = if times == 1 { 400 } else { 1_888 };
        let vip_cap = vip_secondary_recruit_cap(player.vip);
        let mut spent_currency = false;

        {
            let st = self.sync_state(player.uid, now_ts);
            if times == 1 && st.free_remaining > 0 {
                st.free_remaining -= 1;
            } else {
                if st.today_num + times > vip_cap {
                    return Err("exceed_vip_herolegend_limit");
                }
                let coin_part = player.legend_book_coin.min(all_cost);
                let gold_shortfall = (all_cost - coin_part) as i64;
                if player.gold < gold_shortfall {
                    return Err("not_enough_gold_or_legend_coin");
                }
                player.legend_book_coin -= coin_part;
                player.gold -= gold_shortfall;
                st.today_num += times;
                spent_currency = true;
            }
        }

        let bonus_copper = if times == 1 { 10_000i64 } else { 50_000i64 };
        player.copper = (player.copper + bonus_copper).min(PlayerState::COPPER_CAP);

        let mut bag_deltas = Vec::new();
        let mut reward_dtos = Vec::with_capacity(times as usize);
        let mut max_boom = 1u32;

        for _ in 0..times {
            let tid = {
                let st = self.states.get_mut(&player.uid).unwrap();
                let (gain, mult) = TavernRaffleEngine::roll_8500_luck(&mut self.rng);
                st.lucky_num = (st.lucky_num + gain).min(1_500);
                if mult > max_boom {
                    max_boom = mult;
                }
                let force_high = st.idx_to_pity == 0;
                st.idx_to_pity = if st.idx_to_pity == 0 {
                    9
                } else {
                    st.idx_to_pity - 1
                };
                if force_high {
                    if self.rng.gen_below(100) < 25 {
                        703_001 + self.rng.gen_below(8)
                    } else {
                        702_001 + self.rng.gen_below(61)
                    }
                } else {
                    701_001 + self.rng.gen_below(24)
                }
            };
            bag_deltas.extend(player.bag.add_item(tid, 1, now_ts)?);
            reward_dtos.push(DtoValue::map([(
                "item",
                DtoValue::map([(tid.to_string(), DtoValue::from(1u32))]),
            )]));
        }

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }
        let mut ufields = vec![("copper", DtoValue::from(player.copper))];
        if spent_currency {
            ufields.push(("gold", DtoValue::from(player.gold)));
            ufields.push(("legend_book_coin", DtoValue::from(player.legend_book_coin)));
        }
        pushes.push(PlayerState::build_user_update_push(ufields, now_ts));

        let lucky_num = self.states[&player.uid].lucky_num;
        let ret = DtoValue::map([
            ("reward", DtoValue::Array(reward_dtos)),
            ("lucky_num", DtoValue::from(lucky_num)),
            ("boom", DtoValue::from(max_boom)),
        ]);
        Ok((ret, pushes))
    }

    pub fn receive_reward(
        &mut self,
        player: &mut PlayerState,
        reward_id: u32,
        choose_idx: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let (req_luck, tid) = match reward_id {
            1 => {
                if choose_idx >= 61 {
                    return Err("invalid_herolegend_choice");
                }
                (500u32, 702_001 + choose_idx)
            }
            2 => {
                if choose_idx >= 61 {
                    return Err("invalid_herolegend_choice");
                }
                (1_000u32, 702_001 + choose_idx)
            }
            3 => {
                if choose_idx >= 8 {
                    return Err("invalid_herolegend_choice");
                }
                (1_500u32, 703_001 + choose_idx)
            }
            _ => return Err("invalid_herolegend_box"),
        };

        {
            let st = self.sync_state(player.uid, now_ts);
            if st.lucky_num < req_luck {
                return Err("luck_not_enough");
            }
            if st.claimed_boxes.contains(&reward_id) {
                return Err("box_already_claimed");
            }
            st.claimed_boxes.insert(reward_id);
            if st.claimed_boxes.len() >= 3 {
                st.lucky_num = st.lucky_num.saturating_sub(1_500);
                st.claimed_boxes.clear();
            }
        }

        let deltas = player.bag.add_item(tid, 1, now_ts)?;
        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&deltas, now_ts) {
            pushes.push(bp);
        }
        Ok((DtoValue::from("ok"), pushes))
    }
}

// --- 2.3 Activity Festival Recruit Engines (`SoulStone`, `FamousHero`, `TopAnimal` — `B-24..B-26`) ---

#[derive(Debug, Clone, Default)]
pub struct PlayerFestivalRaffleState {
    pub day_bucket: u64,
    pub gold_num: u32,
    pub score: u32,
    pub got_num: u32,
    pub claimed_boxes: BTreeSet<u32>,
    pub claimed_tasks: BTreeSet<u32>,
}

/// Shared engine shape for `activitySoulStoneRecruit.*`, `activityFamousRaffle.*`, and `activityTopAnimalRecruit.*`.
#[derive(Debug, Clone)]
pub struct ActivityRecruitEngine {
    rng: Rng64,
    soul_stone: BTreeMap<u64, PlayerFestivalRaffleState>,
    famous_hero: BTreeMap<u64, PlayerFestivalRaffleState>,
    top_animal: BTreeMap<u64, PlayerFestivalRaffleState>,
}

impl ActivityRecruitEngine {
    pub const TOKEN_FAMOUS: u32 = 19074;
    pub const TOKEN_SOUL_STONE: u32 = 19144;
    pub const TOKEN_TOP_ANIMAL: u32 = 19207;

    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng64::new(seed ^ 0xC33E_0773),
            soul_stone: BTreeMap::new(),
            famous_hero: BTreeMap::new(),
            top_animal: BTreeMap::new(),
        }
    }

    fn build_info_dto(st: &PlayerFestivalRaffleState, include_rounds: bool) -> DtoValue {
        let mut va_box = BTreeMap::new();
        for &b in &st.claimed_boxes {
            va_box.insert(b.to_string(), DtoValue::from(true));
        }
        let mut map = BTreeMap::new();
        map.insert("goldNum".to_owned(), DtoValue::from(st.gold_num));
        map.insert("score".to_owned(), DtoValue::from(st.score));
        map.insert("vaBox".to_owned(), DtoValue::Map(va_box));
        map.insert("rechargeNum".to_owned(), DtoValue::from(0u32));
        map.insert("todayRechargeNum".to_owned(), DtoValue::from(0u32));
        map.insert("totalActivePoint".to_owned(), DtoValue::from(100u32));
        map.insert("todayActivePoint".to_owned(), DtoValue::from(100u32));
        map.insert("taskRewardStatus".to_owned(), DtoValue::Map(BTreeMap::new()));
        if include_rounds {
            map.insert("gotNum".to_owned(), DtoValue::from(st.got_num));
            map.insert("vaBuy".to_owned(), DtoValue::Map(BTreeMap::new()));
        }
        DtoValue::Map(map)
    }

    // --- Soul Stone (`activitySoulStoneRecruit.*` — `B-24`) ---
    pub fn soul_stone_get_info(&mut self, uid: u64) -> DtoValue {
        let st = self.soul_stone.entry(uid).or_default();
        Self::build_info_dto(st, true)
    }

    pub fn soul_stone_raffle(
        &mut self,
        player: &mut PlayerState,
        _pos: u32,
        is_ten: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let count: u32 = if is_ten { 10 } else { 1 };
        let mut bag_deltas = if player.bag.count_by_tid(Self::TOKEN_SOUL_STONE) >= count {
            player.bag.consume_item(Self::TOKEN_SOUL_STONE, count)?
        } else if player.bag.count_by_tid(TavernRaffleEngine::TOKEN_RECRUIT_SEAL) >= count * 300 {
            player
                .bag
                .consume_item(TavernRaffleEngine::TOKEN_RECRUIT_SEAL, count * 300)?
        } else {
            return Err("not_enough_soul_stone_tokens");
        };

        let st = self.soul_stone.entry(player.uid).or_default();
        st.score = st.score.saturating_add(count);

        const HUNSHI_WHOLE: &[u32] = &[4_600_101, 4_600_102, 4_600_103, 4_600_104];
        const HUNSHI_FRAGS: &[u32] = &[4_700_110, 4_700_111, 4_700_112, 4_700_113, 4_700_401];
        let mut drops = Vec::with_capacity(count as usize);
        for i in 0..count {
            let (tid, qty) = if (is_ten && i == 9) || self.rng.gen_below(100) < 15 {
                (HUNSHI_WHOLE[self.rng.gen_below(HUNSHI_WHOLE.len() as u32) as usize], 1u32)
            } else {
                (HUNSHI_FRAGS[self.rng.gen_below(HUNSHI_FRAGS.len() as u32) as usize], 5u32)
            };
            bag_deltas.extend(player.bag.add_item(tid, qty, now_ts)?);
            drops.push(DtoValue::map([(
                "item",
                DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
            )]));
        }

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }
        Ok((DtoValue::Array(drops), pushes))
    }

    pub fn soul_stone_open_box(
        &mut self,
        player: &mut PlayerState,
        box_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        const THRESHOLDS: [u32; 5] = [50, 100, 200, 300, 400];
        if !(1..=5).contains(&box_id) {
            return Err("invalid_box_id");
        }
        let req = THRESHOLDS[(box_id - 1) as usize];
        {
            let st = self.soul_stone.entry(player.uid).or_default();
            if st.got_num >= 4 {
                return Err("box_round_limit_reached");
            }
            if st.score < req {
                return Err("score_not_enough");
            }
            if st.claimed_boxes.contains(&box_id) {
                return Err("box_already_claimed");
            }
            st.claimed_boxes.insert(box_id);
            if st.claimed_boxes.len() >= 5 {
                st.score = st.score.saturating_sub(400);
                st.got_num += 1;
                st.claimed_boxes.clear();
            }
        }
        // Exact `activity_hunshi_recruit.json` `box_reward1..5`
        let (extra_tid, extra_qty) = match box_id {
            1 => (39238u32, 10u32),
            2 => (39238, 20),
            3 => (39238, 40),
            4 => (39238, 80),
            _ => (20080, 80),
        };
        let pushes = grant_rewards_to_player(
            player,
            &[
                RewardEntry::Item {
                    tid: Self::TOKEN_SOUL_STONE,
                    qty: 10,
                },
                RewardEntry::Item {
                    tid: extra_tid,
                    qty: extra_qty,
                },
            ],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    // --- Famous Hero (`activityFamousRaffle.*` — `B-25`) ---
    pub fn famous_get_info(&mut self, uid: u64) -> DtoValue {
        let st = self.famous_hero.entry(uid).or_default();
        Self::build_info_dto(st, false)
    }

    pub fn famous_raffle(
        &mut self,
        player: &mut PlayerState,
        pos: u32,
        use_type: u32,
        is_ten: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let count: u32 = if is_ten { 10 } else { 1 };
        let mut bag_deltas = if use_type == 0 {
            let seal_cost = 300 * count;
            if player.bag.count_by_tid(TavernRaffleEngine::TOKEN_RECRUIT_SEAL) < seal_cost {
                return Err("not_enough_recruit_seal");
            }
            player
                .bag
                .consume_item(TavernRaffleEngine::TOKEN_RECRUIT_SEAL, seal_cost)?
        } else {
            if player.bag.count_by_tid(Self::TOKEN_FAMOUS) < count {
                return Err("not_enough_famous_tokens");
            }
            player.bag.consume_item(Self::TOKEN_FAMOUS, count)?
        };

        let st = self.famous_hero.entry(player.uid).or_default();
        st.score = st.score.saturating_add(count * 10);

        let featured_frag = RED_HERO_FRAGS[(pos.saturating_sub(1) as usize) % RED_HERO_FRAGS.len()];
        let mut drops = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let (tid, qty) = if self.rng.gen_below(100) < 45 {
                (featured_frag, 5u32)
            } else {
                (ORANGE_HERO_FRAGS[self.rng.gen_below(ORANGE_HERO_FRAGS.len() as u32) as usize], 5)
            };
            bag_deltas.extend(player.bag.add_item(tid, qty, now_ts)?);
            drops.push(DtoValue::map([(
                "item",
                DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
            )]));
        }

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }
        Ok((DtoValue::Array(drops), pushes))
    }

    pub fn famous_open_box(
        &mut self,
        player: &mut PlayerState,
        box_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        const THRESHOLDS: [u32; 4] = [100, 300, 600, 1000];
        if !(1..=4).contains(&box_id) {
            return Err("invalid_box_id");
        }
        let req = THRESHOLDS[(box_id - 1) as usize];
        {
            let st = self.famous_hero.entry(player.uid).or_default();
            if st.score < req {
                return Err("score_not_enough");
            }
            if st.claimed_boxes.contains(&box_id) {
                return Err("box_already_claimed");
            }
            st.claimed_boxes.insert(box_id);
        }
        let frag_tid = RED_HERO_FRAGS[(box_id as usize) % RED_HERO_FRAGS.len()];
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Item {
                tid: frag_tid,
                qty: box_id * 10,
            }],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    // --- Top Animal (`activityTopAnimalRecruit.*` — `B-26`) ---
    pub fn top_animal_get_info(&mut self, uid: u64) -> DtoValue {
        let st = self.top_animal.entry(uid).or_default();
        Self::build_info_dto(st, true)
    }

    pub fn top_animal_raffle(
        &mut self,
        player: &mut PlayerState,
        pos: u32,
        is_ten: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let count: u32 = if is_ten { 10 } else { 1 };
        let mut bag_deltas = if player.bag.count_by_tid(Self::TOKEN_TOP_ANIMAL) >= count {
            player.bag.consume_item(Self::TOKEN_TOP_ANIMAL, count)?
        } else if player.bag.count_by_tid(TavernRaffleEngine::TOKEN_RECRUIT_SEAL) >= count * 300 {
            player
                .bag
                .consume_item(TavernRaffleEngine::TOKEN_RECRUIT_SEAL, count * 300)?
        } else {
            return Err("not_enough_top_animal_tokens");
        };

        let st = self.top_animal.entry(player.uid).or_default();
        st.score = st.score.saturating_add(count);

        // Verified in `topAnimal/item_topAnimal.json` (`5400001` Tỳ Hưu, `5400002` Nhai Tí)
        // & `item_topAnimal_fragment.json` (`5500001`, `5500002`)
        let animal_idx = (pos.saturating_sub(1)) % 2;
        let whole_tid = 5_400_001 + animal_idx;
        let frag_tid = 5_500_001 + animal_idx;
        let mut drops = Vec::with_capacity(count as usize);
        for i in 0..count {
            let (tid, qty) = if (is_ten && i == 9) || self.rng.gen_below(100) < 10 {
                (whole_tid, 1u32)
            } else {
                (frag_tid, 5u32)
            };
            bag_deltas.extend(player.bag.add_item(tid, qty, now_ts)?);
            drops.push(DtoValue::map([(
                "item",
                DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
            )]));
        }

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }
        Ok((DtoValue::Array(drops), pushes))
    }

    pub fn top_animal_open_box(
        &mut self,
        player: &mut PlayerState,
        box_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        const THRESHOLDS: [u32; 5] = [50, 100, 200, 300, 400];
        if !(1..=5).contains(&box_id) {
            return Err("invalid_box_id");
        }
        let req = THRESHOLDS[(box_id - 1) as usize];
        {
            let st = self.top_animal.entry(player.uid).or_default();
            if st.score < req {
                return Err("score_not_enough");
            }
            if st.claimed_boxes.contains(&box_id) {
                return Err("box_already_claimed");
            }
            st.claimed_boxes.insert(box_id);
            if st.claimed_boxes.len() >= 5 {
                st.score = st.score.saturating_sub(400);
                st.got_num += 1;
                st.claimed_boxes.clear();
            }
        }
        let pushes = grant_rewards_to_player(
            player,
            &[
                RewardEntry::Item {
                    tid: Self::TOKEN_TOP_ANIMAL,
                    qty: 5,
                },
                RewardEntry::Item {
                    tid: 5_500_001,
                    qty: box_id * 5,
                },
            ],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    pub fn claim_task_reward(
        &mut self,
        player: &mut PlayerState,
        token_tid: u32,
        task_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let map = match token_tid {
            Self::TOKEN_SOUL_STONE => &mut self.soul_stone,
            Self::TOKEN_TOP_ANIMAL => &mut self.top_animal,
            _ => &mut self.famous_hero,
        };
        let st = map.entry(player.uid).or_default();
        if st.claimed_tasks.contains(&task_id) {
            return Err("task_already_claimed");
        }
        st.claimed_tasks.insert(task_id);
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Item {
                tid: token_tid,
                qty: 2,
            }],
            now_ts,
        )?;
        let fix_tb = DtoValue::array([DtoValue::array([
            DtoValue::from(4u32),
            DtoValue::from(token_tid),
            DtoValue::from(2u32),
        ])]);
        Ok((fix_tb, pushes))
    }
}

// --- 2.4 Limit Arm Recruit (`raffleArm.*` — `B-27`) ---

#[derive(Debug, Clone, Default)]
pub struct PlayerLimitArmState {
    pub count: u32,
    pub point: u32,
    pub claimed_boxes: BTreeSet<u32>,
}

#[derive(Debug, Clone)]
pub struct LimitArmRaffleEngine {
    rng: Rng64,
    states: BTreeMap<u64, PlayerLimitArmState>,
}

impl LimitArmRaffleEngine {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng64::new(seed ^ 0xD44F_1884),
            states: BTreeMap::new(),
        }
    }

    pub fn get_info(&mut self, uid: u64) -> DtoValue {
        let st = self.states.entry(uid).or_default();
        let boxes: Vec<DtoValue> = st
            .claimed_boxes
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        DtoValue::map([
            ("count", DtoValue::from(st.count)),
            ("point", DtoValue::from(st.point)),
            ("myRank", DtoValue::from(if st.point > 0 { 1u32 } else { 0u32 })),
            ("rank", DtoValue::Array(Vec::new())),
            ("boxReward", DtoValue::Array(boxes)),
        ])
    }

    pub fn raffle(
        &mut self,
        player: &mut PlayerState,
        num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if num != 1 && num != 10 {
            return Err("invalid_arm_raffle_num");
        }
        let cost: i64 = if num == 1 { 280 } else { 2_680 };
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;

        let st = self.states.entry(player.uid).or_default();
        st.count += num;
        st.point += num * 10;

        const ARM_FRAGS: &[u32] = &[3_145_001, 3_145_002, 3_145_003, 3_145_004];
        let mut item_map = BTreeMap::new();
        let mut bag_deltas = Vec::new();
        for _ in 0..num {
            let tid = ARM_FRAGS[self.rng.gen_below(ARM_FRAGS.len() as u32) as usize];
            bag_deltas.extend(player.bag.add_item(tid, 2, now_ts)?);
            let prev = item_map
                .get(&tid.to_string())
                .and_then(DtoValue::as_u32)
                .unwrap_or(0);
            item_map.insert(tid.to_string(), DtoValue::from(prev + 2));
        }

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }
        pushes.push(PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        ));
        let ret = DtoValue::map([
            ("myRank", DtoValue::from(1u32)),
            ("rank", DtoValue::Array(Vec::new())),
            ("drop", DtoValue::map([("item", DtoValue::Map(item_map))])),
        ]);
        Ok((ret, pushes))
    }

    pub fn get_box_reward(
        &mut self,
        player: &mut PlayerState,
        box_idx_0based: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        const REQ_POINTS: [u32; 4] = [50, 150, 300, 600];
        let &req = REQ_POINTS
            .get(box_idx_0based as usize)
            .ok_or("invalid_arm_box")?;
        {
            let st = self.states.entry(player.uid).or_default();
            if st.point < req {
                return Err("point_not_enough");
            }
            if st.claimed_boxes.contains(&box_idx_0based) {
                return Err("box_already_claimed");
            }
            st.claimed_boxes.insert(box_idx_0based);
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Item {
                tid: 3_145_001,
                qty: 5 * (box_idx_0based + 1),
            }],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }
}

/// Unified hub bundling all 6 secondary raffle engines (`B-22..B-28`) with a single RPC dispatcher.
#[derive(Debug, Clone)]
pub struct SecondaryRaffleHub {
    pub godarm: GodArmRaffleEngine,
    pub hero_legend: HeroLegendRaffleEngine,
    pub festival: ActivityRecruitEngine,
    pub limit_arm: LimitArmRaffleEngine,
}

impl SecondaryRaffleHub {
    pub fn new(seed: u64) -> Self {
        Self {
            godarm: GodArmRaffleEngine::new(seed),
            hero_legend: HeroLegendRaffleEngine::new(seed),
            festival: ActivityRecruitEngine::new(seed),
            limit_arm: LimitArmRaffleEngine::new(seed),
        }
    }

    /// Dispatches any `godarmRaffle.*`, `heroLegendRaffle.*`, `activitySoulStoneRecruit.*`,
    /// `activityFamousRaffle.*`, `activityTopAnimalRecruit.*`, or `raffleArm.*` (including `tm*` variants).
    pub fn dispatch_rpc(
        &mut self,
        method: &str,
        args: &[DtoValue],
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Option<Result<(DtoValue, Vec<PushPacket>), &'static str>> {
        match method {
            "godarmRaffle.getInfo" | "tmGodarmRaffle.getInfo" => {
                Some(Ok((self.godarm.get_info(player.uid, now_ts), Vec::new())))
            }
            "godarmRaffle.singleRaffle" | "tmGodarmRaffle.singleRaffle" => {
                Some(self.godarm.draw(player, false, now_ts))
            }
            "godarmRaffle.multiRaffle" | "tmGodarmRaffle.multiRaffle" => {
                Some(self.godarm.draw(player, true, now_ts))
            }
            "godarmRaffle.receiveLuckHeroGift" | "tmGodarmRaffle.receiveLuckHeroGift" => {
                let score = args.first().and_then(DtoValue::as_u32).unwrap_or(500);
                let idx = args.get(1).and_then(DtoValue::as_u32).unwrap_or(0);
                Some(self.godarm.receive_luck_gift(player, score, idx, now_ts))
            }

            "heroLegendRaffle.getInfo" | "tmHeroLegendRaffle.getInfo" => Some(Ok((
                self.hero_legend.get_info(player.uid, now_ts),
                Vec::new(),
            ))),
            "heroLegendRaffle.raffle" | "tmHeroLegendRaffle.raffle" => {
                let times = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                Some(self.hero_legend.raffle(player, times, now_ts))
            }
            "heroLegendRaffle.receiveReward" | "tmHeroLegendRaffle.receiveReward" => {
                let rid = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let cidx = args.get(1).and_then(DtoValue::as_u32).unwrap_or(0);
                Some(self.hero_legend.receive_reward(player, rid, cidx, now_ts))
            }

            "activitySoulStoneRecruit.getInfo" | "tmActivitySoulStoneRecruit.getInfo" => {
                Some(Ok((self.festival.soul_stone_get_info(player.uid), Vec::new())))
            }
            "activitySoulStoneRecruit.raffle" | "tmActivitySoulStoneRecruit.raffle" => {
                let pos = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let is_ten = args.get(1).and_then(DtoValue::as_bool).unwrap_or(false);
                Some(self.festival.soul_stone_raffle(player, pos, is_ten, now_ts))
            }
            "activitySoulStoneRecruit.openBox" | "tmActivitySoulStoneRecruit.openBox" => {
                let box_id = extract_box_id_arg(args).unwrap_or(1);
                Some(self.festival.soul_stone_open_box(player, box_id, now_ts))
            }
            "activitySoulStoneRecruit.getTaskReward" | "tmActivitySoulStoneRecruit.getTaskReward" => {
                let tid = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                Some(self.festival.claim_task_reward(
                    player,
                    ActivityRecruitEngine::TOKEN_SOUL_STONE,
                    tid,
                    now_ts,
                ))
            }
            "activitySoulStoneRecruit.useVoucher" | "tmActivitySoulStoneRecruit.useVoucher" => {
                Some(Ok((DtoValue::from("ok"), Vec::new())))
            }

            "activityFamousRaffle.getInfo" | "tmActivityFamousRaffle.getInfo" => {
                Some(Ok((self.festival.famous_get_info(player.uid), Vec::new())))
            }
            "activityFamousRaffle.raffle" | "tmActivityFamousRaffle.raffle" => {
                let pos = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let use_type = args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                let is_ten = args.get(2).and_then(DtoValue::as_bool).unwrap_or(false);
                Some(self.festival.famous_raffle(player, pos, use_type, is_ten, now_ts))
            }
            "activityFamousRaffle.openBox" | "tmActivityFamousRaffle.openBox" => {
                let box_id = extract_box_id_arg(args).unwrap_or(1);
                Some(self.festival.famous_open_box(player, box_id, now_ts))
            }
            "activityFamousRaffle.getTaskReward" | "tmActivityFamousRaffle.getTaskReward" => {
                let tid = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                Some(self.festival.claim_task_reward(
                    player,
                    ActivityRecruitEngine::TOKEN_FAMOUS,
                    tid,
                    now_ts,
                ))
            }

            "activityTopAnimalRecruit.getInfo" => {
                Some(Ok((self.festival.top_animal_get_info(player.uid), Vec::new())))
            }
            "activityTopAnimalRecruit.raffle" => {
                let pos = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let is_ten = args.get(1).and_then(DtoValue::as_bool).unwrap_or(false);
                Some(self.festival.top_animal_raffle(player, pos, is_ten, now_ts))
            }
            "activityTopAnimalRecruit.openBox" => {
                let box_id = extract_box_id_arg(args).unwrap_or(1);
                Some(self.festival.top_animal_open_box(player, box_id, now_ts))
            }
            "activityTopAnimalRecruit.getTaskReward" => {
                let tid = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                Some(self.festival.claim_task_reward(
                    player,
                    ActivityRecruitEngine::TOKEN_TOP_ANIMAL,
                    tid,
                    now_ts,
                ))
            }
            "activityTopAnimalRecruit.useVoucher" => Some(Ok((DtoValue::from("ok"), Vec::new()))),

            "raffleArm.getInfo" => Some(Ok((self.limit_arm.get_info(player.uid), Vec::new()))),
            "raffleArm.raffle" => {
                let num = args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                Some(self.limit_arm.raffle(player, num, now_ts))
            }
            "raffleArm.getBoxReward" => {
                let idx = args.first().and_then(DtoValue::as_u32).unwrap_or(0);
                Some(self.limit_arm.get_box_reward(player, idx, now_ts))
            }
            "raffleArm.reciveRankReward" | "raffleArm.enter" | "raffleArm.leave" => {
                Some(Ok((DtoValue::from("ok"), Vec::new())))
            }
            _ => None,
        }
    }
}

// ============================================================================
// 3. Unified `BaseShopData` Shop Subsystem (`shop.*` & `tmShop.*`, `shop_type` 1..=35)
// ============================================================================

/// Shop Refresh Token (`Lệnh Làm Mới`) from `shop_config.db` `refresh_item_id_num = [16001, 1]`.
pub const SHOP_REFRESH_TOKEN_TID: u32 = 16001;

/// `BaseShopProxy` `cos_type` constants (`ShopConst.lua`).
pub const SHOP_REFRESH_TYPE_FREE: u32 = 1;
pub const SHOP_REFRESH_TYPE_ITEM: u32 = 2;
pub const SHOP_REFRESH_TYPE_MONEY: u32 = 3;
pub const SHOP_REFRESH_TYPE_ADD_FREE: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopCurrency {
    Copper,
    Gold,
    ArenaCoin,
    HeroSoul,
    PriceType(u32),
}

impl ShopCurrency {
    pub const fn from_price_type(pt: u32) -> Self {
        match pt {
            1 => Self::Copper,
            2 => Self::Gold,
            9 => Self::ArenaCoin,
            15 => Self::HeroSoul,
            other => Self::PriceType(other),
        }
    }

    pub const fn price_type_id(self) -> u32 {
        match self {
            Self::Copper => 1,
            Self::Gold => 2,
            Self::ArenaCoin => 9,
            Self::HeroSoul => 15,
            Self::PriceType(pt) => pt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShopNeedItem {
    /// `4 = ITEM` (`BagData`), `5 = HERO` (`HerosData` bench sacrifice in `ExchangeShop 27`).
    pub kind: u32,
    pub tid: u32,
    pub qty: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopOffer {
    pub idx: u32,
    pub id: u32,
    /// `PriceTypeConst` reward kind: `4 = ITEM` (default), `5 = HERO`, `1 = COPPER`, `15 = HERO_SOUL`, `37 = STAR_SOUL`.
    pub reward_type: u32,
    pub item_tid: u32,
    pub item_qty: u32,
    pub currency: ShopCurrency,
    pub unit_price: u64,
    /// `u32::MAX` represents `-1` (unlimited purchases in `ware_buy_limit`).
    pub max_buy: u32,
    pub bought: u32,
    pub need_item: Option<ShopNeedItem>,
    pub increase_steps: &'static [(u32, u64)],
    pub vip_lv_inc: u32,
    pub vip_buy_table: &'static [(u32, u32)],
}

impl ShopOffer {
    /// Calculates effective purchase limit accounting for VIP (`vip_buy_increase` / `vip_lv_increase`)
    /// and 5x `multiRefresh` (`ShopItem:getCanBuyNum`, `ShopItem.lua:212-235`).
    pub fn effective_max_buy(&self, vip: u32, is_multi: bool) -> u32 {
        if self.max_buy == u32::MAX {
            return u32::MAX;
        }
        let mut limit = if is_multi {
            self.max_buy.saturating_mul(5)
        } else {
            self.max_buy
        };
        if !self.vip_buy_table.is_empty() {
            let mut extra = 0u32;
            for &(v_lv, add) in self.vip_buy_table {
                if vip <= v_lv {
                    extra = add;
                    break;
                }
                extra = add;
            }
            limit = limit.saturating_add(extra);
        } else if self.vip_lv_inc > 0 {
            limit = limit.saturating_add(vip.saturating_mul(self.vip_lv_inc));
        }
        limit
    }

    /// Computes exact total currency cost for buying `buy_num` more units starting from `self.bought`,
    /// applying `GoldShop` stepped `increase_array` (`ShopItem:getCostMoney`).
    pub fn total_cost_for(&self, buy_num: u32) -> u64 {
        if self.increase_steps.is_empty() {
            return self.unit_price.saturating_mul(u64::from(buy_num));
        }
        let mut sum = 0u64;
        for k in 1..=buy_num {
            let nth = self.bought.saturating_add(k);
            let mut add = self.increase_steps.last().map_or(0, |s| s.1);
            for &(up_to, step_add) in self.increase_steps {
                if nth <= up_to {
                    add = step_add;
                    break;
                }
            }
            sum = sum.saturating_add(self.unit_price.saturating_add(add));
        }
        sum
    }

    /// Emits exact `ShopItem:initShopData` wire fields (`idx`, `id`, and **`num`** — NOT only `buy_num`!).
    pub fn to_dto(&self) -> DtoValue {
        let wire_max: i64 = if self.max_buy == u32::MAX {
            -1
        } else {
            i64::from(self.max_buy)
        };
        DtoValue::map([
            ("idx", DtoValue::from(self.idx)),
            ("id", DtoValue::from(self.id)),
            ("num", DtoValue::from(self.bought)),
            ("buy_num", DtoValue::from(self.bought)),
            ("item_tid", DtoValue::from(self.item_tid)),
            ("item_num", DtoValue::from(self.item_qty)),
            ("price", DtoValue::from(self.unit_price)),
            ("max_num", DtoValue::from(wire_max)),
        ])
    }
}

/// Static DB ware row tuple: `(id, reward_type, tid, qty, price_type, base_price, limit, need_kind, need_tid, need_qty)`.
type StaticWareRow = (u32, u32, u32, u32, u32, u64, i32, u32, u32, u32);

const INC_STAMINA_PILL: &[(u32, u64)] = &[
    (3, 0), (5, 10), (7, 20), (9, 30), (11, 40), (13, 50), (15, 60), (17, 70), (999, 80),
];
const INC_EXP_BOOK: &[(u32, u64)] = &[
    (2, 0), (4, 10), (6, 20), (8, 30), (10, 40), (12, 50), (14, 60), (16, 70), (999, 80),
];
const INC_REVIVE: &[(u32, u64)] = &[
    (1, 0), (2, 10), (3, 20), (4, 30), (5, 40), (6, 50), (7, 60), (8, 70), (9, 80), (10, 90),
];
const INC_COMPASS: &[(u32, u64)] = &[
    (3, 0), (6, 10), (9, 20), (12, 30), (15, 40), (18, 50), (21, 60), (24, 70),
    (27, 80), (30, 90), (33, 100), (36, 110), (39, 120), (42, 130), (45, 140), (99_999, 150),
];
const INC_REBEL_TOKEN: &[(u32, u64)] = &[
    (5, 0), (10, 5), (20, 10), (30, 20), (40, 25), (999, 30),
];
const INC_PET_BOX: &[(u32, u64)] = &[(1, 0), (2, 20), (999, 40)];

const VIP_BUY_PILL: &[(u32, u32)] = &[
    (0, 5), (1, 14), (2, 17), (3, 18), (4, 19), (5, 20), (6, 22), (7, 24), (8, 26),
    (9, 28), (10, 30), (11, 32), (12, 34), (13, 36), (14, 38), (15, 40), (16, 42),
];
const VIP_BUY_GOLD_BOX: &[(u32, u32)] = &[
    (0, 0), (1, 1), (2, 3), (3, 5), (4, 7), (5, 9), (6, 11), (7, 13), (8, 15),
    (9, 17), (10, 19), (11, 21), (12, 23), (13, 25), (14, 27), (15, 29), (16, 31),
];
const VIP_BUY_EXP_BOOK: &[(u32, u32)] = &[
    (0, 0), (1, 2), (2, 4), (3, 6), (4, 8), (5, 10), (6, 12), (7, 14), (8, 16),
    (9, 18), (10, 20), (11, 22), (12, 24), (13, 26), (14, 28), (15, 30), (16, 32),
];
const VIP_BUY_BLUE_TREAS: &[(u32, u32)] = &[
    (0, 0), (1, 4), (2, 8), (3, 12), (4, 16), (5, 20), (6, 24), (7, 28), (8, 32),
    (9, 36), (10, 40), (11, 44), (12, 48), (13, 52), (14, 56), (15, 60), (16, 64),
];
const VIP_BUY_REBEL: &[(u32, u32)] = &[
    (0, 0), (1, 5), (2, 10), (3, 15), (4, 20), (5, 25), (6, 30), (7, 35), (8, 40),
    (9, 45), (10, 50), (11, 55), (12, 60), (13, 65), (14, 70), (15, 75), (16, 80),
];

/// Complete 31-item `GoldShop` (`shop_type = 8`, `gold_ware.json` rows `1..=31`).
fn build_gold_shop_offers() -> Vec<ShopOffer> {
    const GOLD_ROWS: &[StaticWareRow] = &[
        (1, 4, 22102, 1, 2, 20, 5, 0, 0, 0),
        (2, 4, 22202, 1, 2, 20, 5, 0, 0, 0),
        (3, 4, 30003, 1, 2, 50, 1, 0, 0, 0),
        (4, 4, 30002, 1, 2, 20, 10, 0, 0, 0),
        (5, 4, 30001, 1, 2, 10, -1, 0, 0, 0),
        (6, 4, 30006, 1, 2, 50, -1, 0, 0, 0),
        (7, 4, 30005, 1, 2, 20, -1, 0, 0, 0),
        (8, 4, 30004, 1, 2, 10, -1, 0, 0, 0),
        (9, 4, 17003, 1, 2, 20, 5, 0, 0, 0),
        (10, 4, 181002, 1, 2, 100, -1, 0, 0, 0),
        (11, 4, 181001, 1, 2, 20, 10, 0, 0, 0),
        (12, 4, 10012, 1, 2, 5, 500, 0, 0, 0),
        (13, 4, 10204, 1, 2, 20, 200, 0, 0, 0),
        (14, 4, 16001, 1, 2, 20, -1, 0, 0, 0),
        (15, 4, 15003, 1, 2, 10, -1, 0, 0, 0),
        (16, 4, 15004, 1, 2, 30, -1, 0, 0, 0),
        (17, 4, 15005, 10, 2, 20, 200, 0, 0, 0),
        (18, 4, 15006, 1, 2, 10, -1, 0, 0, 0),
        (19, 4, 15102, 1, 2, 10, 5, 0, 0, 0),
        (20, 4, 15007, 1, 2, 5, 10, 0, 0, 0),
        (21, 4, 10014, 10, 2, 5, 50, 0, 0, 0),
        (22, 4, 15008, 1, 2, 50, 50, 0, 0, 0),
        (23, 4, 36002, 1, 2, 60, 10, 0, 0, 0),
        (24, 4, 15103, 1, 2, 50, 100, 0, 0, 0),
        (25, 4, 29237, 1, 2, 1000, 1, 0, 0, 0),
        (26, 37, 0, 100, 2, 100, 8, 0, 0, 0),
        (27, 4, 28601, 1, 67, 50, 10, 0, 0, 0),
        (28, 4, 28602, 1, 68, 200, 10, 0, 0, 0),
        (29, 4, 28603, 1, 69, 300, 10, 0, 0, 0),
        (30, 4, 28604, 1, 70, 400, 10, 0, 0, 0),
        (31, 4, 28605, 1, 71, 500, 10, 0, 0, 0),
    ];

    GOLD_ROWS
        .iter()
        .enumerate()
        .map(|(i, &(id, rtype, tid, qty, pt, price, lim, _, _, _))| {
            let increase_steps: &'static [(u32, u64)] = match id {
                1 | 2 => INC_STAMINA_PILL,
                9 => INC_EXP_BOOK,
                18 => INC_REVIVE,
                19 => INC_COMPASS,
                20 => INC_REBEL_TOKEN,
                23 => INC_PET_BOX,
                _ => &[],
            };
            let vip_buy_table: &'static [(u32, u32)] = match id {
                1 | 2 => VIP_BUY_PILL,
                3 => VIP_BUY_GOLD_BOX,
                9 => VIP_BUY_EXP_BOOK,
                11 => VIP_BUY_BLUE_TREAS,
                20 => VIP_BUY_REBEL,
                _ => &[],
            };
            let vip_lv_inc = match id {
                4 => 5,
                19 => 3,
                _ => 0,
            };
            ShopOffer {
                idx: (i as u32) + 1,
                id,
                reward_type: rtype,
                item_tid: tid,
                item_qty: qty,
                currency: ShopCurrency::from_price_type(pt),
                unit_price: price,
                max_buy: if lim < 0 { u32::MAX } else { lim as u32 },
                bought: 0,
                need_item: None,
                increase_steps,
                vip_lv_inc,
                vip_buy_table,
            }
        })
        .collect()
}

// Verified DB ware catalogs extracted from `shop.db`, `devil.db`, `soaring.db`, `clanCrossServer.db`, `timeMachine_shop.db`:
const WARE_ARENA_2: &[StaticWareRow] = &[
    (1, 4, 50109, 5, 9, 1500, -1, 0, 0, 0),
    (2, 4, 50207, 5, 9, 1500, -1, 0, 0, 0),
    (3, 4, 50306, 5, 9, 1500, -1, 0, 0, 0),
    (4, 4, 50406, 5, 9, 1500, -1, 0, 0, 0),
    (5, 4, 22102, 1, 9, 500, 2, 0, 0, 0),
    (6, 4, 22202, 1, 9, 500, 2, 0, 0, 0),
    (7, 4, 15005, 10, 9, 200, 10, 0, 0, 0),
    (8, 4, 17003, 1, 9, 300, 5, 0, 0, 0),
];

const WARE_SOUL_3: &[StaticWareRow] = &[
    (10001, 4, 181001, 1, 2, 20, 1, 0, 0, 0),
    (10002, 4, 181001, 1, 15, 20, 1, 0, 0, 0),
    (10004, 4, 10010, 50, 2, 50, 1, 0, 0, 0),
    (10005, 4, 50106, 2, 15, 200, 1, 0, 0, 0),
    (10006, 4, 50203, 2, 15, 200, 1, 0, 0, 0),
    (10007, 4, 50312, 2, 15, 200, 1, 0, 0, 0),
    (10008, 4, 50406, 2, 15, 200, 1, 0, 0, 0),
    (10024, 4, 17002, 1, 15, 12, 1, 0, 0, 0),
    (40001, 5, 108, 1, 2, 2000, 1, 0, 0, 0),
    (40002, 5, 108, 1, 15, 2000, 1, 0, 0, 0),
];

const WARE_CLAN_5: &[StaticWareRow] = &[
    (1, 15, 0, 100, 17, 1000, 5, 0, 0, 0),
    (2, 4, 10010, 100, 17, 1000, 5, 0, 0, 0),
    (3, 4, 181002, 1, 17, 1000, 5, 0, 0, 0),
    (4, 4, 15005, 50, 17, 1000, 5, 0, 0, 0),
    (5, 4, 22102, 1, 17, 500, 1, 0, 0, 0),
    (6, 4, 22202, 1, 17, 500, 1, 0, 0, 0),
];

const WARE_TOWER_7: &[StaticWareRow] = &[
    (1, 4, 19002, 1, 10, 10, 10, 0, 0, 0),
    (2, 4, 10201, 1, 10, 20, 100, 0, 0, 0),
    (3, 4, 10202, 1, 10, 40, 60, 0, 0, 0),
    (4, 4, 10203, 1, 10, 100, 40, 0, 0, 0),
    (5, 4, 10204, 1, 10, 200, 20, 0, 0, 0),
    (30, 4, 3145001, 1, 10, 400, -1, 4, 19002, 2),
];

const WARE_WORLDBOSS_9: &[StaticWareRow] = &[
    (1, 4, 304001, 5, 2, 500, 1, 0, 0, 0),
    (2, 4, 304001, 5, 23, 500, 1, 0, 0, 0),
    (3, 4, 304002, 5, 2, 500, 1, 0, 0, 0),
    (4, 4, 304002, 5, 23, 500, 1, 0, 0, 0),
    (5, 4, 304003, 5, 23, 500, 1, 0, 0, 0),
    (6, 4, 304004, 5, 23, 500, 1, 0, 0, 0),
];

/// `AwakeShop` (`shop_type = 10`, `jewel_ware.json` — real IDs start at `365`, fixing the P0 `id=1..3` crash!).
const WARE_AWAKE_10: &[StaticWareRow] = &[
    (365, 4, 3305415, 5, 2, 162, 1, 0, 0, 0),
    (366, 4, 3305415, 5, 23, 1287, 1, 0, 0, 0),
    (367, 4, 3305415, 5, 2, 163, 1, 0, 0, 0),
    (368, 4, 3305415, 5, 23, 1288, 1, 0, 0, 0),
    (369, 4, 3305106, 5, 20, 800, 1, 0, 0, 0),
    (370, 4, 3305207, 5, 20, 800, 1, 0, 0, 0),
    (371, 4, 3305306, 5, 23, 1000, 1, 0, 0, 0),
    (372, 4, 3305406, 5, 20, 800, 1, 0, 0, 0),
];

const WARE_MYSTERY_12: &[StaticWareRow] = &[
    (1, 4, 50108, 20, 2, 500, 1, 0, 0, 0),
    (2, 4, 50108, 20, 15, 500, 1, 0, 0, 0),
    (3, 4, 50110, 20, 2, 500, 1, 0, 0, 0),
    (4, 4, 50110, 20, 15, 500, 1, 0, 0, 0),
    (5, 4, 50209, 20, 15, 500, 1, 0, 0, 0),
    (6, 4, 50308, 20, 15, 500, 1, 0, 0, 0),
    (7, 4, 50408, 20, 2, 500, 1, 0, 0, 0),
    (8, 4, 50213, 20, 15, 500, 1, 0, 0, 0),
];

const WARE_RENOWN_13: &[StaticWareRow] = &[
    (1, 4, 50116, 1, 27, 400, 20, 0, 0, 0),
    (2, 4, 50204, 1, 27, 400, 20, 0, 0, 0),
    (3, 4, 50304, 1, 27, 400, 20, 0, 0, 0),
    (4, 4, 50403, 1, 27, 400, 20, 0, 0, 0),
    (5, 4, 181001, 1, 27, 2000, 20, 0, 0, 0),
    (32, 4, 181001, 1, 9, 2000, 20, 0, 0, 0),
];

const WARE_PET_14: &[StaticWareRow] = &[
    (10001, 4, 10016, 2, 28, 4, 1, 0, 0, 0),
    (10002, 4, 10017, 2, 28, 10, 1, 0, 0, 0),
    (10003, 4, 10018, 1, 28, 20, 1, 0, 0, 0),
    (10004, 4, 10018, 2, 28, 40, 1, 0, 0, 0),
    (10005, 4, 10016, 5, 2, 20, 1, 0, 0, 0),
    (20001, 4, 3430001, 1, 28, 20, 1, 0, 0, 0),
    (20002, 4, 3430001, 2, 28, 40, 1, 0, 0, 0),
];

const WARE_HONOUR_15: &[StaticWareRow] = &[
    (1, 4, 50102, 1, 29, 50, 320, 0, 0, 0),
    (2, 4, 50202, 1, 29, 50, 320, 0, 0, 0),
    (3, 4, 50302, 1, 29, 50, 320, 0, 0, 0),
    (4, 4, 50402, 1, 29, 50, 320, 0, 0, 0),
    (5, 4, 50101, 1, 29, 50, 320, 0, 0, 0),
    (32, 15, 0, 50, 29, 50, 150, 0, 0, 0),
];

const WARE_SOLO_16: &[StaticWareRow] = &[
    (1, 4, 10021, 1, 32, 20, 50, 0, 0, 0),
    (2, 4, 28400, 1, 32, 100, 10, 0, 0, 0),
    (3, 4, 10026, 1, 32, 100, 100, 0, 0, 0),
    (4, 4, 10027, 1, 32, 100, 100, 0, 0, 0),
    (62, 4, 50412, 5, 32, 1500, -1, 0, 0, 0),
];

const WARE_CROSS_17: &[StaticWareRow] = &[
    (1, 4, 10021, 1, 31, 200, 300, 0, 0, 0),
    (2, 4, 10022, 1, 31, 1000, 20, 0, 0, 0),
    (3, 4, 10023, 1, 31, 1000, 20, 0, 0, 0),
    (4, 4, 10024, 1, 31, 1000, 20, 0, 0, 0),
    (39, 4, 19066, 1, 31, 500, 140, 0, 0, 0),
];

const WARE_GODARM_18: &[StaticWareRow] = &[
    (1, 4, 19008, 1, 33, 10, 100, 0, 0, 0),
    (2, 4, 19007, 1, 33, 20, 50, 0, 0, 0),
    (3, 4, 19011, 1, 33, 50, 50, 0, 0, 0),
    (4, 4, 3500102, 1, 33, 40, 10, 0, 0, 0),
    (5, 4, 3500401, 1, 33, 100, 10, 4, 19012, 1),
    (31, 4, 19235, 1, 33, 50, 20, 0, 0, 0),
];

const WARE_GODANIMAL_19: &[StaticWareRow] = &[
    (10001, 4, 3601001, 2, 34, 200, 1, 0, 0, 0),
    (10002, 4, 3601001, 5, 34, 500, 1, 0, 0, 0),
    (10003, 4, 3601001, 10, 34, 1000, 1, 0, 0, 0),
    (10004, 4, 3602001, 1, 34, 150, 1, 0, 0, 0),
    (10005, 4, 3602001, 2, 34, 300, 1, 0, 0, 0),
    (10006, 4, 3603001, 2, 34, 200, 1, 0, 0, 0),
    (10007, 4, 3603001, 5, 34, 500, 1, 0, 0, 0),
    (10024, 4, 3602001, 5, 2, 750, 1, 0, 0, 0),
];

const WARE_HEROLEGEND_20: &[StaticWareRow] = &[
    (10001, 4, 701001, 1, 39, 200, 5, 0, 0, 0),
    (10002, 4, 701002, 1, 39, 200, 5, 0, 0, 0),
    (10003, 4, 701003, 1, 39, 200, 5, 0, 0, 0),
    (10004, 4, 701004, 1, 39, 200, 5, 0, 0, 0),
    (10005, 4, 702001, 1, 39, 800, 2, 0, 0, 0),
    (10006, 4, 703001, 1, 39, 2000, 1, 0, 0, 0),
];

const WARE_BROCHURE_21: &[StaticWareRow] = &[
    (10001, 4, 10032, 1, 46, 100, 1, 0, 0, 0),
    (10002, 4, 10032, 2, 46, 200, 1, 0, 0, 0),
    (10003, 4, 10032, 5, 46, 500, 1, 0, 0, 0),
    (10004, 4, 10032, 1, 2, 100, 1, 0, 0, 0),
    (40001, 4, 3803001, 5, 46, 400, 1, 0, 0, 0),
    (40002, 4, 3803002, 5, 46, 400, 1, 0, 0, 0),
    (40003, 4, 3803001, 5, 2, 400, 1, 0, 0, 0),
];

const WARE_GEM_22: &[StaticWareRow] = &[
    (10001, 4, 10039, 1, 50, 20, 1, 0, 0, 0),
    (10002, 4, 10040, 1, 50, 50, 1, 0, 0, 0),
    (10003, 4, 10041, 1, 50, 100, 1, 0, 0, 0),
    (10004, 4, 10042, 5, 50, 25, 1, 0, 0, 0),
    (20001, 4, 10039, 1, 2, 20, 1, 0, 0, 0),
    (20002, 4, 10040, 1, 2, 50, 1, 0, 0, 0),
    (20003, 4, 10041, 1, 2, 100, 1, 0, 0, 0),
];

const WARE_BINGFU_23: &[StaticWareRow] = &[
    (10001, 4, 10046, 2, 2, 20, 1, 0, 0, 0),
    (10002, 4, 10046, 4, 2, 40, 1, 0, 0, 0),
    (10003, 4, 10046, 6, 2, 60, 1, 0, 0, 0),
    (10004, 4, 10047, 1, 2, 20, 1, 0, 0, 0),
    (10023, 4, 10046, 2, 53, 20, 1, 0, 0, 0),
    (10024, 4, 10048, 1, 53, 50, 1, 0, 0, 0),
    (10025, 4, 1301001, 5, 53, 200, 1, 0, 0, 0),
];

const WARE_CLASSICWAR_24: &[StaticWareRow] = &[
    (1, 4, 19044, 1, 61, 50, 100, 0, 0, 0),
    (2, 4, 19045, 1, 61, 100, 50, 0, 0, 0),
    (3, 4, 39229, 1, 61, 300, 50, 0, 0, 0),
    (4, 4, 39230, 1, 61, 300, 50, 0, 0, 0),
    (12, 4, 39229, 2, 61, 480, 2, 0, 0, 0),
];

const WARE_DEVIL_25: &[StaticWareRow] = &[
    (1, 4, 19062, 1, 64, 50, 450, 0, 0, 0),
    (2, 4, 19064, 1, 64, 300, 14, 0, 0, 0),
    (3, 4, 19071, 1, 64, 50, 300, 0, 0, 0),
    (4, 4, 19073, 1, 64, 300, 7, 0, 0, 0),
    (9, 4, 19224, 1, 64, 300, 7, 0, 0, 0),
];

const WARE_NATIONALWAR_26: &[StaticWareRow] = &[
    (1, 4, 19066, 1, 66, 500, 300, 0, 0, 0),
    (2, 4, 19069, 1, 66, 3000, 14, 0, 0, 0),
    (3, 4, 28040, 1, 66, 1000, 50, 0, 0, 0),
    (4, 4, 28041, 1, 66, 2000, 30, 0, 0, 0),
];

/// `ExchangeShop` (`shop_type = 27`, `exchange_ware.json` — hero sacrifices `1..=8` & item exchanges `9..=38`).
const WARE_EXCHANGE_27: &[StaticWareRow] = &[
    (1, 4, 19081, 1, 10, 0, -1, 5, 101, 1),
    (2, 4, 19081, 1, 10, 0, -1, 5, 102, 1),
    (3, 4, 19081, 1, 10, 0, -1, 5, 201, 1),
    (4, 4, 19081, 1, 10, 0, -1, 5, 202, 1),
    (5, 4, 19081, 1, 10, 0, -1, 5, 301, 1),
    (6, 4, 19081, 1, 10, 0, -1, 5, 302, 1),
    (7, 4, 19081, 1, 10, 0, -1, 5, 401, 1),
    (8, 4, 19081, 1, 10, 0, -1, 5, 402, 1),
    (9, 4, 50101, 80, 10, 0, -1, 4, 19081, 1),
    (10, 4, 50102, 80, 10, 0, -1, 4, 19081, 1),
    (38, 4, 19114, 1, 10, 0, -1, 4, 1850013, 60),
];

const WARE_UNIFY_28: &[StaticWareRow] = &[
    (1, 4, 19087, 1, 72, 75, 200, 0, 0, 0),
    (2, 4, 19088, 1, 72, 75, 200, 0, 0, 0),
    (3, 4, 28058, 1, 72, 75, 200, 0, 0, 0),
    (4, 4, 29033, 1, 72, 150, 150, 0, 0, 0),
    (9, 4, 27878, 1, 72, 315, 60, 0, 0, 0),
];

const WARE_FIRSTRIDE_29: &[StaticWareRow] = &[
    (1, 4, 19092, 1, 77, 50, 200, 0, 0, 0),
    (2, 4, 28686, 1, 77, 50, 200, 0, 0, 0),
    (3, 4, 19071, 1, 77, 50, 200, 0, 0, 0),
    (4, 4, 19102, 1, 77, 50, 200, 0, 0, 0),
    (11, 4, 27878, 1, 77, 150, 60, 0, 0, 0),
];

const WARE_SOARING_30: &[StaticWareRow] = &[
    (1, 4, 19107, 1, 78, 300, 7, 0, 0, 0),
    (2, 4, 19108, 1, 78, 50, 300, 0, 0, 0),
    (3, 4, 19110, 1, 78, 600, 7, 0, 0, 0),
    (4, 4, 19111, 1, 78, 50, 70, 0, 0, 0),
    (21, 4, 19237, 1, 78, 300, 10, 0, 0, 0),
];

const WARE_CROSS_CLAN_31: &[StaticWareRow] = &[
    (1, 4, 19144, 20, 81, 2500, 1, 0, 0, 0),
    (2, 4, 28653, 50, 81, 2500, 1, 0, 0, 0),
    (3, 4, 10010, 20000, 81, 2500, 1, 0, 0, 0),
    (4, 4, 24992, 50, 81, 3500, 1, 0, 0, 0),
    (16, 4, 5320001, 1, 81, 150, 25, 0, 0, 0),
];

const WARE_QUSHUIYAN_32: &[StaticWareRow] = &[
    (1, 4, 16012, 1, 82, 100, 10, 0, 0, 0),
    (2, 4, 22102, 2, 82, 50, 20, 0, 0, 0),
];

const WARE_TM_TREASURE_34: &[StaticWareRow] = &[
    (1, 4, 5201001, 1, 201, 50, 800, 0, 0, 0),
    (2, 4, 5201002, 1, 201, 100, 400, 0, 0, 0),
    (3, 4, 5201003, 1, 201, 15000, 4, 0, 0, 0),
    (4, 4, 5201004, 1, 201, 25000, 3, 0, 0, 0),
    (20, 4, 5201036, 1, 201, 100, -1, 0, 0, 0),
];

const WARE_IRONGOLD_35: &[StaticWareRow] = &[
    (1, 4, 19245, 1, 106, 50, 300, 0, 0, 0),
    (2, 4, 19247, 1, 106, 300, 10, 0, 0, 0),
    (3, 4, 19248, 1, 106, 50, 300, 0, 0, 0),
    (4, 4, 19250, 1, 106, 300, 10, 0, 0, 0),
];

const WARE_TM_TOWER_7: &[StaticWareRow] = &[
    (1, 4, 19002, 1, 10, 4000, -1, 0, 0, 0),
    (2, 4, 10201, 1, 10, 6000, -1, 0, 0, 0),
    (3, 4, 10202, 1, 10, 10000, -1, 0, 0, 0),
    (901, 4, 16012, 5, 10, 1, 1, 0, 0, 0),
    (920, 4, 29003, 1, 10, 1, 1, 0, 0, 0),
];

#[derive(Debug, Clone, Copy)]
struct ShopTypeConfig {
    is_random_refresh: bool,
    slot_count: usize,
    free_upper_limit: u32,
    free_cd_secs: u64,
    refresh_token_qty: u32,
    refresh_res_type: Option<u32>,
    refresh_price: u64,
    daily_refresh_limit: u32,
}

fn config_for_shop_type(shop_type: u32) -> ShopTypeConfig {
    match shop_type {
        1 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 3,
            free_cd_secs: 7_200,
            refresh_token_qty: 1,
            refresh_res_type: None,
            refresh_price: 20,
            daily_refresh_limit: 40,
        },
        3 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 10,
            free_cd_secs: 7_200,
            refresh_token_qty: 1,
            refresh_res_type: Some(15),
            refresh_price: 20,
            daily_refresh_limit: 40,
        },
        10 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 10,
            free_cd_secs: 7_200,
            refresh_token_qty: 1,
            refresh_res_type: Some(23),
            refresh_price: 200,
            daily_refresh_limit: 20,
        },
        12 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 0,
            free_cd_secs: 7_200,
            refresh_token_qty: 2,
            refresh_res_type: None,
            refresh_price: 100,
            daily_refresh_limit: 20,
        },
        14 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 10,
            free_cd_secs: 7_200,
            refresh_token_qty: 1,
            refresh_res_type: Some(28),
            refresh_price: 20,
            daily_refresh_limit: 18,
        },
        19 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 8,
            free_upper_limit: 10,
            free_cd_secs: 7_200,
            refresh_token_qty: 1,
            refresh_res_type: Some(34),
            refresh_price: 20,
            daily_refresh_limit: 15,
        },
        21 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 10,
            free_cd_secs: 7_200,
            refresh_token_qty: 1,
            refresh_res_type: Some(46),
            refresh_price: 20,
            daily_refresh_limit: 20,
        },
        22 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 10,
            free_cd_secs: 21_600,
            refresh_token_qty: 1,
            refresh_res_type: Some(50),
            refresh_price: 20,
            daily_refresh_limit: 20,
        },
        23 => ShopTypeConfig {
            is_random_refresh: true,
            slot_count: 6,
            free_upper_limit: 10,
            free_cd_secs: 21_600,
            refresh_token_qty: 1,
            refresh_res_type: Some(53),
            refresh_price: 20,
            daily_refresh_limit: 30,
        },
        _ => ShopTypeConfig {
            is_random_refresh: false,
            slot_count: 0,
            free_upper_limit: 0,
            free_cd_secs: 86_400,
            refresh_token_qty: 0,
            refresh_res_type: None,
            refresh_price: 20,
            daily_refresh_limit: 20,
        },
    }
}

fn offer_from_static_row(idx: u32, row: StaticWareRow) -> ShopOffer {
    let (id, rtype, tid, qty, pt, price, lim, nkind, ntid, nqty) = row;
    ShopOffer {
        idx,
        id,
        reward_type: rtype,
        item_tid: tid,
        item_qty: qty,
        currency: ShopCurrency::from_price_type(pt),
        unit_price: price,
        max_buy: if lim < 0 { u32::MAX } else { lim as u32 },
        bought: 0,
        need_item: if nkind > 0 {
            Some(ShopNeedItem {
                kind: nkind,
                tid: ntid,
                qty: nqty,
            })
        } else {
            None
        },
        increase_steps: &[],
        vip_lv_inc: 0,
        vip_buy_table: &[],
    }
}

fn catalog_rows_for_shop(shop_type: u32, is_time_machine: bool) -> &'static [StaticWareRow] {
    match (shop_type, is_time_machine) {
        (7, true) => WARE_TM_TOWER_7,
        (2, _) => WARE_ARENA_2,
        (3, _) => WARE_SOUL_3,
        (5, _) => WARE_CLAN_5,
        (7, false) => WARE_TOWER_7,
        (9, _) => WARE_WORLDBOSS_9,
        (10, _) => WARE_AWAKE_10,
        (11, _) | (18, _) => WARE_GODARM_18,
        (12, _) => WARE_MYSTERY_12,
        (13, _) => WARE_RENOWN_13,
        (14, _) => WARE_PET_14,
        (15, _) => WARE_HONOUR_15,
        (16, _) => WARE_SOLO_16,
        (17, _) => WARE_CROSS_17,
        (19, _) => WARE_GODANIMAL_19,
        (20, _) => WARE_HEROLEGEND_20,
        (21, _) => WARE_BROCHURE_21,
        (22, _) => WARE_GEM_22,
        (23, _) => WARE_BINGFU_23,
        (24, _) => WARE_CLASSICWAR_24,
        (25, _) => WARE_DEVIL_25,
        (26, _) => WARE_NATIONALWAR_26,
        (27, _) => WARE_EXCHANGE_27,
        (28, _) => WARE_UNIFY_28,
        (29, _) => WARE_FIRSTRIDE_29,
        (30, _) => WARE_SOARING_30,
        (31, _) => WARE_CROSS_CLAN_31,
        (32, _) => WARE_QUSHUIYAN_32,
        (34, _) => WARE_TM_TREASURE_34,
        (35, _) => WARE_IRONGOLD_35,
        _ => WARE_ARENA_2,
    }
}

#[derive(Debug, Clone)]
pub struct PlayerShopSheet {
    pub shop_type: u32,
    pub is_time_machine: bool,
    /// Active Lua `goods_key`: `0` on initial `getInfo` (`va_shop[1]`), `1` after any `refresh` (`va_shop[2]`).
    pub goods_key_int: u32,
    pub goods_key: String,
    pub refresh_epoch: u64,
    /// Timed accumulated free refreshes remaining (`free_num` in `BaseShopData.lua`).
    pub free_num: u32,
    /// Daily `const_free_times` already used (`refresh_free_num` in `BaseShopData.lua`, starts at `0`!).
    pub refresh_free_num: u32,
    /// Paid money/currency refreshes used today (`refresh_num`).
    pub refresh_num: u32,
    /// Token (`16001`) refreshes used today (`refresh_item_num`).
    pub refresh_item_num: u32,
    pub last_free_num_time: u64,
    pub is_multi: bool,
    pub normal_auto_buy_gold_limit: u32,
    pub quick_buy_items: Vec<u32>,
    pub quick_buy_heroes: Vec<u32>,
    pub offers: Vec<ShopOffer>,
}

/// Multi-tab unified shop engine (`shop.getInfo`, `shop.buy`, `shop.refresh`, `shop.multiRefresh`,
/// `shop.quickBuy`, `shop.normalShopAutoBuy`, `tmShop.*`, `shop.open`).
#[derive(Debug, Clone)]
pub struct ShopEngine {
    /// `(uid, encoded_shop_id) -> PlayerShopSheet`
    sheets: BTreeMap<(u64, u32), PlayerShopSheet>,
    rng: Rng64,
    seq: u64,
}

impl Default for ShopEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ShopEngine {
    pub fn new() -> Self {
        Self {
            sheets: BTreeMap::new(),
            rng: Rng64::new(0x5A09_CA7A_1065_BEEF),
            seq: 0,
        }
    }

    fn generate_offers(rng: &mut Rng64, shop_type: u32, is_time_machine: bool) -> Vec<ShopOffer> {
        if shop_type == 1 {
            // Synthetic E2E shop (`shop_type = 1`, unused by client UI) preserves `101..103`
            // plus authentic `ware.db` rows `1..=3` so E2E tests and DB lookups both succeed.
            return vec![
                ShopOffer {
                    idx: 1,
                    id: 101,
                    reward_type: 4,
                    item_tid: 16011,
                    item_qty: 1,
                    currency: ShopCurrency::Copper,
                    unit_price: 10_000,
                    max_buy: 5,
                    bought: 0,
                    need_item: None,
                    increase_steps: &[],
                    vip_lv_inc: 0,
                    vip_buy_table: &[],
                },
                ShopOffer {
                    idx: 2,
                    id: 102,
                    reward_type: 4,
                    item_tid: 16012,
                    item_qty: 1,
                    currency: ShopCurrency::Gold,
                    unit_price: 250,
                    max_buy: 3,
                    bought: 0,
                    need_item: None,
                    increase_steps: &[],
                    vip_lv_inc: 0,
                    vip_buy_table: &[],
                },
                ShopOffer {
                    idx: 3,
                    id: 103,
                    reward_type: 4,
                    item_tid: 17002,
                    item_qty: 5,
                    currency: ShopCurrency::Gold,
                    unit_price: 100,
                    max_buy: 10,
                    bought: 0,
                    need_item: None,
                    increase_steps: &[],
                    vip_lv_inc: 0,
                    vip_buy_table: &[],
                },
                offer_from_static_row(4, (1, 1, 0, 10000, 2, 10, 1, 0, 0, 0)),
                offer_from_static_row(5, (2, 1, 0, 30000, 2, 30, 1, 0, 0, 0)),
                offer_from_static_row(6, (3, 1, 0, 50000, 2, 50, 1, 0, 0, 0)),
            ];
        }

        if shop_type == 8 {
            return build_gold_shop_offers();
        }

        let cfg = config_for_shop_type(shop_type);
        let pool = catalog_rows_for_shop(shop_type, is_time_machine);
        if cfg.is_random_refresh && pool.len() > cfg.slot_count && cfg.slot_count > 0 {
            let start = rng.gen_below(pool.len() as u32) as usize;
            return (0..cfg.slot_count)
                .map(|i| {
                    let row = pool[(start + i) % pool.len()];
                    offer_from_static_row((i as u32) + 1, row)
                })
                .collect();
        }

        pool.iter()
            .enumerate()
            .map(|(i, &row)| offer_from_static_row((i as u32) + 1, row))
            .collect()
    }

    fn sheet_map_key(shop_type: u32, is_tm: bool) -> u32 {
        if is_tm {
            shop_type | 0x8000_0000
        } else {
            shop_type
        }
    }

    fn ensure_sheet_ext(
        &mut self,
        uid: u64,
        shop_type: u32,
        is_time_machine: bool,
        now_ts: u64,
    ) -> &mut PlayerShopSheet {
        let key = Self::sheet_map_key(shop_type, is_time_machine);
        let rng = &mut self.rng;
        let seq = &mut self.seq;
        self.sheets.entry((uid, key)).or_insert_with(|| {
            *seq += 1;
            let cfg = config_for_shop_type(shop_type);
            PlayerShopSheet {
                shop_type,
                is_time_machine,
                goods_key_int: 0,
                goods_key: "0".to_owned(),
                refresh_epoch: 0,
                free_num: cfg.free_upper_limit,
                refresh_free_num: 0,
                refresh_num: 0,
                refresh_item_num: 0,
                last_free_num_time: now_ts,
                is_multi: false,
                normal_auto_buy_gold_limit: 0,
                quick_buy_items: Vec::new(),
                quick_buy_heroes: Vec::new(),
                offers: Self::generate_offers(rng, shop_type, is_time_machine),
            }
        })
    }

    /// Encodes `va_shop` as the exact 1-based nested array expected by `BaseShopData:getShopData()`
    /// and `BaseShopData:updateData(va_shop, goods_key)`:
    /// - `goods_key == 0` (`getInfo`): `va_shop = [ offers ]` (`va_shop[1]`)
    /// - `goods_key == 1` (`refresh` / `multiRefresh`): `va_shop = [ [], offers ]` (`va_shop[2]`,
    ///   taking the `if goods_key == 1 then updateDataAll` branch in `BaseShopData.lua`!).
    fn encode_va_shop(offers: &[ShopOffer], goods_key_int: u32) -> DtoValue {
        let group: Vec<DtoValue> = offers.iter().map(ShopOffer::to_dto).collect();
        let mut groups = Vec::with_capacity((goods_key_int as usize) + 1);
        for _ in 0..goods_key_int {
            groups.push(DtoValue::Array(Vec::new()));
        }
        groups.push(DtoValue::Array(group));
        DtoValue::Array(groups)
    }

    /// Encodes string-keyed `goods_list` map (`{ "1": OfferDto, "2": OfferDto, ... }`).
    fn encode_goods_list_map(offers: &[ShopOffer]) -> DtoValue {
        let mut map = BTreeMap::new();
        for offer in offers {
            map.insert(offer.idx.to_string(), offer.to_dto());
        }
        DtoValue::Map(map)
    }

    /// Returns exact `BaseShopData:parseData` table (`shop.getInfo`, `args = [shopType]`).
    pub fn get_info(&mut self, uid: u64, shop_type: u32, now_ts: u64) -> DtoValue {
        self.get_info_ext(uid, shop_type, false, now_ts)
    }

    /// Time-Machine-aware `getInfo` (`tmShop.getInfo` sets `is_time_machine = true` for `TowerShop 7` & `TreasureShop 34`).
    pub fn get_info_ext(
        &mut self,
        uid: u64,
        shop_type: u32,
        is_time_machine: bool,
        now_ts: u64,
    ) -> DtoValue {
        let cfg = config_for_shop_type(shop_type);
        let sheet = self.ensure_sheet_ext(uid, shop_type, is_time_machine, now_ts);
        let va_shop = Self::encode_va_shop(&sheet.offers, sheet.goods_key_int);
        let goods_list = Self::encode_goods_list_map(&sheet.offers);
        // Preserve string `"0"` ONLY on synthetic `shop_type == 1` initial `getInfo` for legacy Rust E2E tests;
        // emit strict integer `0`/`1` on all real client shops (`2..=35`) per `BaseShopData.lua`!
        let wire_goods_key = if shop_type == 1 && sheet.refresh_epoch == 0 {
            DtoValue::from(sheet.goods_key.as_str())
        } else {
            DtoValue::from(sheet.goods_key_int)
        };
        let va_quick_buy = DtoValue::map([
            (
                "item",
                DtoValue::array(sheet.quick_buy_items.iter().copied().map(DtoValue::from)),
            ),
            (
                "hero",
                DtoValue::array(sheet.quick_buy_heroes.iter().copied().map(DtoValue::from)),
            ),
        ]);
        let refresh_cd = now_ts + cfg.free_cd_secs;

        DtoValue::map([
            ("open", DtoValue::from(true)),
            ("open_time", DtoValue::from(now_ts)),
            ("refresh_time", DtoValue::from(refresh_cd)),
            ("refresh_cd", DtoValue::from(refresh_cd)),
            ("refresh_free_num", DtoValue::from(sheet.refresh_free_num)),
            ("free_refresh_num", DtoValue::from(sheet.free_num)),
            ("refresh_num", DtoValue::from(sheet.refresh_num)),
            ("gold_refresh_num", DtoValue::from(sheet.refresh_num)),
            ("refresh_item_num", DtoValue::from(sheet.refresh_item_num)),
            ("goods_key", wire_goods_key),
            ("free_num", DtoValue::from(sheet.free_num)),
            ("last_free_num_time", DtoValue::from(sheet.last_free_num_time)),
            ("multi_tag", DtoValue::from(u32::from(sheet.is_multi))),
            ("point", DtoValue::from(0u32)),
            ("va_shop", va_shop),
            ("goods_list", goods_list),
            ("va_quick_buy", va_quick_buy),
            ("va_normal_auto_buy", DtoValue::Array(Vec::new())),
            (
                "normal_auto_buy_gold_limit",
                DtoValue::from(sheet.normal_auto_buy_gold_limit),
            ),
        ])
    }

    /// Executes `shop.buy` (`args = [shopType, goodsKey, idx, id, buyNum]` -> `{ ret = "ok", arr_del_hid? }`).
    #[allow(clippy::too_many_arguments)]
    pub fn buy(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        goods_key: &str,
        idx: u32,
        offer_id: u32,
        buy_num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.buy_ext(
            player,
            shop_type,
            false,
            goods_key,
            idx,
            offer_id,
            buy_num,
            now_ts,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn buy_ext(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        is_time_machine: bool,
        goods_key: &str,
        idx: u32,
        offer_id: u32,
        buy_num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if buy_num == 0 {
            return Err("invalid_buy_num");
        }
        let vip = player.vip;
        let (matched_idx, offer_snapshot) = {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
            if !goods_key.is_empty()
                && goods_key != sheet.goods_key
                && goods_key != sheet.goods_key_int.to_string()
            {
                return Err("stale_goods_key");
            }
            let found = sheet
                .offers
                .iter()
                .find(|o| {
                    (o.idx == idx && (offer_id == 0 || o.id == offer_id))
                        || (offer_id != 0 && idx == offer_id && o.id == offer_id)
                })
                .ok_or("offer_not_found")?;
            let max_buy = found.effective_max_buy(vip, sheet.is_multi);
            if max_buy != u32::MAX && found.bought.saturating_add(buy_num) > max_buy {
                return Err("exceed_buy_limit");
            }
            (found.idx, found.clone())
        };

        let total_cost = offer_snapshot.total_cost_for(buy_num);
        let price_type = offer_snapshot.currency.price_type_id();

        // Pre-flight check currency AND `need_item` BEFORE mutating anything!
        if total_cost > 0 && player.currency_by_price_type(price_type) < total_cost {
            return Err(match price_type {
                1 => "not_enough_copper",
                2 => "not_enough_gold",
                9 => "not_enough_arena_coin",
                15 => "not_enough_hero_soul",
                _ => "not_enough_currency",
            });
        }
        if let Some(need) = offer_snapshot.need_item {
            let req_qty = need.qty.saturating_mul(buy_num);
            if need.kind == 4 && player.bag.count_by_tid(need.tid) < req_qty {
                return Err("not_enough_need_items");
            }
            if need.kind == 5 {
                let bench_count = player
                    .roster
                    .heroes
                    .iter()
                    .filter(|(&hid, h)| h.htid == need.tid && !player.roster.formation.contains(&hid))
                    .count() as u32;
                if bench_count < req_qty {
                    return Err("not_enough_sacrifice_heroes");
                }
            }
        }

        let mut user_delta = Vec::new();
        let mut bag_deltas = Vec::new();
        let mut pre_pushes = Vec::new();
        let mut deleted_hids = Vec::new();

        if let Some(field) = player.spend_currency_by_price_type(price_type, total_cost)? {
            user_delta.push(field);
        }

        if let Some(need) = offer_snapshot.need_item {
            let req_qty = need.qty.saturating_mul(buy_num);
            if need.kind == 4 {
                bag_deltas.extend(player.bag.consume_item(need.tid, req_qty)?);
            } else if need.kind == 5 {
                deleted_hids = player.roster.remove_bench_heroes_by_htid(need.tid, req_qty)?;
            }
        }

        let total_qty = offer_snapshot.item_qty.saturating_mul(buy_num);
        match offer_snapshot.reward_type {
            4 => {
                bag_deltas.extend(player.bag.add_item(offer_snapshot.item_tid, total_qty, now_ts)?);
            }
            5 => {
                for _ in 0..total_qty {
                    let (_, hpkt) = player
                        .roster
                        .recruit_hero_to_bench(offer_snapshot.item_tid, 1, now_ts);
                    pre_pushes.push(hpkt);
                }
            }
            other_pt => {
                if let Some(field) =
                    player.add_currency_by_price_type(other_pt, u64::from(total_qty))
                {
                    user_delta.push(field);
                }
            }
        }

        let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
        if let Some(offer) = sheet.offers.iter_mut().find(|o| o.idx == matched_idx) {
            offer.bought = offer.bought.saturating_add(buy_num);
        }

        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pre_pushes.insert(0, bp);
        }
        if !user_delta.is_empty() {
            pre_pushes.push(PlayerState::build_user_update_push(user_delta, now_ts));
        }

        let ret = if deleted_hids.is_empty() {
            DtoValue::map([("ret", DtoValue::from("ok"))])
        } else {
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                (
                    "arr_del_hid",
                    DtoValue::array(deleted_hids.into_iter().map(DtoValue::from)),
                ),
            ])
        };
        Ok((ret, pre_pushes))
    }

    /// Executes a single refresh step, returning `(cos_type, bag_deltas, user_delta_opt)`.
    fn step_one_refresh(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        is_time_machine: bool,
        now_ts: u64,
    ) -> Result<(u32, Vec<crate::player::BagDelta>, Option<(&'static str, DtoValue)>), &'static str>
    {
        let cfg = config_for_shop_type(shop_type);
        let max_paid = cfg.daily_refresh_limit + player.vip * 10;
        let (has_add_free, used_paid) = {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
            (sheet.free_num > 0, sheet.refresh_num + sheet.refresh_item_num)
        };

        let mut bag_deltas = Vec::new();
        let mut user_field = None;

        let cos_type = if has_add_free {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
            sheet.free_num -= 1;
            sheet.last_free_num_time = now_ts;
            SHOP_REFRESH_TYPE_ADD_FREE
        } else {
            if used_paid >= max_paid {
                return Err("exceed_refresh_limit");
            }
            if cfg.refresh_token_qty > 0
                && player.bag.count_by_tid(SHOP_REFRESH_TOKEN_TID) >= cfg.refresh_token_qty
            {
                bag_deltas.extend(
                    player
                        .bag
                        .consume_item(SHOP_REFRESH_TOKEN_TID, cfg.refresh_token_qty)?,
                );
                let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
                sheet.refresh_item_num += 1;
                SHOP_REFRESH_TYPE_ITEM
            } else if let Some(res_pt) = cfg
                .refresh_res_type
                .filter(|&pt| player.currency_by_price_type(pt) >= cfg.refresh_price)
            {
                user_field = player.spend_currency_by_price_type(res_pt, cfg.refresh_price)?;
                let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
                sheet.refresh_num += 1;
                SHOP_REFRESH_TYPE_MONEY
            } else {
                // Fallback to Gold (`20` Gold standard)
                let gold_price = if shop_type == 10 { 20 } else { cfg.refresh_price };
                user_field = player.spend_currency_by_price_type(2, gold_price)?;
                let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
                sheet.refresh_num += 1;
                SHOP_REFRESH_TYPE_MONEY
            }
        };

        self.seq += 1;
        let next_seq = self.seq;
        let new_offers = Self::generate_offers(&mut self.rng, shop_type, is_time_machine);
        let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
        sheet.refresh_epoch = next_seq;
        // CRITICAL (`BaseShopData:updateData`): `goods_key == 1` (integer `1`) triggers `updateDataAll`
        // and sets `shopItemsMap = va_shop[2]` without ever wiping the UI!
        sheet.goods_key_int = 1;
        sheet.goods_key = if shop_type == 1 {
            next_seq.to_string()
        } else {
            "1".to_owned()
        };
        sheet.offers = new_offers;

        Ok((cos_type, bag_deltas, user_field))
    }

    /// Executes `shop.refresh` (`args = [shopType]` -> `{ ret = "ok", cos_type, goods_key = 1, va_shop }`).
    pub fn refresh(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.refresh_ext(player, shop_type, false, now_ts)
    }

    pub fn refresh_ext(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        is_time_machine: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let (cos_type, bag_deltas, user_field) =
            self.step_one_refresh(player, shop_type, is_time_machine, now_ts)?;
        let (va_shop, goods_list) = {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, is_time_machine, now_ts);
            sheet.is_multi = false;
            (
                Self::encode_va_shop(&sheet.offers, 1),
                Self::encode_goods_list_map(&sheet.offers),
            )
        };

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }
        if let Some(uf) = user_field {
            pushes.push(PlayerState::build_user_update_push([uf], now_ts));
        }

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("cos_type", DtoValue::from(cos_type)),
            ("goods_key", DtoValue::from(1u32)),
            ("va_shop", va_shop),
            ("goods_list", goods_list),
        ]);
        Ok((ret, pushes))
    }

    /// Executes 5x `shop.multiRefresh` (`args = [shopType]` -> `{ ret = "ok", cos_type = [nFree, nItem, nMoney, nAddFree], goods_key = 1, va_shop }`).
    pub fn multi_refresh(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let mut counts = [0u32; 4];
        let mut all_bag_deltas = Vec::new();
        let mut user_fields: BTreeMap<&'static str, DtoValue> = BTreeMap::new();

        for _ in 0..5 {
            let (ct, bdeltas, ufield) = self.step_one_refresh(player, shop_type, false, now_ts)?;
            if (1..=4).contains(&ct) {
                counts[(ct - 1) as usize] += 1;
            }
            all_bag_deltas.extend(bdeltas);
            if let Some((k, v)) = ufield {
                user_fields.insert(k, v);
            }
        }

        let (va_shop, goods_list) = {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, false, now_ts);
            sheet.is_multi = true;
            (
                Self::encode_va_shop(&sheet.offers, 1),
                Self::encode_goods_list_map(&sheet.offers),
            )
        };

        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&all_bag_deltas, now_ts) {
            pushes.push(bp);
        }
        if !user_fields.is_empty() {
            pushes.push(PlayerState::build_user_update_push(user_fields, now_ts));
        }

        let cos_type_arr = DtoValue::array(counts.into_iter().map(DtoValue::from));
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("cos_type", cos_type_arr),
            ("goods_key", DtoValue::from(1u32)),
            ("va_shop", va_shop),
            ("goods_list", goods_list),
        ]);
        Ok((ret, pushes))
    }

    /// Executes `shop.quickBuy` (`args = [shopType, itemTids, heroHtids, refreshMode, goldLimit, maxRefresh]`).
    #[allow(clippy::too_many_arguments)]
    pub fn quick_buy(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        target_items: &[u32],
        target_heroes: &[u32],
        refresh_mode: u32,
        gold_limit: u64,
        max_refresh: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, false, now_ts);
            sheet.quick_buy_items = target_items.to_vec();
            sheet.quick_buy_heroes = target_heroes.to_vec();
        }

        let mut cost_type_counts = [0u32; 4];
        let mut refresh_done = 0u32;
        let mut gold_spent_refresh = 0u64;
        let mut resource_spent: BTreeMap<u32, u64> = BTreeMap::new();
        let mut reward_items: BTreeMap<u32, u32> = BTreeMap::new();
        let mut reward_heroes: BTreeMap<u32, u32> = BTreeMap::new();
        let mut all_pushes = Vec::new();
        let cap = max_refresh.min(20);

        loop {
            // Buy matching offers on the current sheet
            let candidates: Vec<(u32, u32, u32, u32, u32, u64)> = {
                let sheet = self.ensure_sheet_ext(player.uid, shop_type, false, now_ts);
                sheet
                    .offers
                    .iter()
                    .filter(|o| {
                        let can_buy = o.bought < o.effective_max_buy(player.vip, sheet.is_multi);
                        let wanted = (o.reward_type == 4 && target_items.contains(&o.item_tid))
                            || (o.reward_type == 5 && target_heroes.contains(&o.item_tid));
                        can_buy && wanted
                    })
                    .map(|o| {
                        (
                            o.idx,
                            o.id,
                            o.reward_type,
                            o.item_tid,
                            o.item_qty,
                            o.currency.price_type_id() as u64,
                        )
                    })
                    .collect()
            };

            for (idx, id, rtype, tid, qty, pt_u64) in candidates {
                let pt = pt_u64 as u32;
                let before = player.currency_by_price_type(pt);
                if let Ok((_, bp)) = self.buy_ext(player, shop_type, false, "", idx, id, 1, now_ts) {
                    let after = player.currency_by_price_type(pt);
                    let diff = before.saturating_sub(after);
                    if pt == 2 {
                        gold_spent_refresh += diff;
                    } else if diff > 0 {
                        *resource_spent.entry(pt).or_insert(0) += diff;
                    }
                    if rtype == 5 {
                        *reward_heroes.entry(tid).or_insert(0) += qty;
                    } else {
                        *reward_items.entry(tid).or_insert(0) += qty;
                    }
                    all_pushes.extend(bp);
                }
            }

            if refresh_done >= cap {
                break;
            }
            // `refresh_mode == 2` means free/token only (no Gold spend)
            let sheet_has_free = self.ensure_sheet_ext(player.uid, shop_type, false, now_ts).free_num > 0;
            let has_token = player.bag.count_by_tid(SHOP_REFRESH_TOKEN_TID) >= 1;
            if !sheet_has_free && !has_token {
                if refresh_mode == 2 || gold_spent_refresh + 20 > gold_limit || player.gold < 20 {
                    break;
                }
            }
            let before_gold = player.gold;
            let Ok((ct, bdeltas, ufield)) = self.step_one_refresh(player, shop_type, false, now_ts) else {
                break;
            };
            refresh_done += 1;
            if (1..=4).contains(&ct) {
                cost_type_counts[(ct - 1) as usize] += 1;
            }
            let g_diff = (before_gold - player.gold).max(0) as u64;
            gold_spent_refresh += g_diff;
            if let Some(bp) = BagState::build_update_push(&bdeltas, now_ts) {
                all_pushes.push(bp);
            }
            if let Some(uf) = ufield {
                all_pushes.push(PlayerState::build_user_update_push([uf], now_ts));
            }
        }

        let va_shop = {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, false, now_ts);
            sheet.is_multi = false;
            sheet.goods_key_int = 1;
            Self::encode_va_shop(&sheet.offers, 1)
        };

        let mut res_map = BTreeMap::new();
        for (pt, amt) in resource_spent {
            res_map.insert(pt.to_string(), DtoValue::from(amt));
        }
        let mut reward_map = BTreeMap::new();
        if !reward_items.is_empty() {
            let mut m = BTreeMap::new();
            for (tid, q) in reward_items {
                m.insert(tid.to_string(), DtoValue::from(q));
            }
            reward_map.insert("4".to_owned(), DtoValue::Map(m));
        }
        if !reward_heroes.is_empty() {
            let mut m = BTreeMap::new();
            for (htid, q) in reward_heroes {
                m.insert(htid.to_string(), DtoValue::from(q));
            }
            reward_map.insert("5".to_owned(), DtoValue::Map(m));
        }

        let ret = DtoValue::map([
            (
                "costType",
                DtoValue::array(cost_type_counts.into_iter().map(DtoValue::from)),
            ),
            ("refreshNum", DtoValue::from(refresh_done)),
            ("goods_key", DtoValue::from(1u32)),
            ("va_shop", va_shop),
            (
                "cost",
                DtoValue::map([
                    ("resource", DtoValue::Map(res_map)),
                    ("gold", DtoValue::from(gold_spent_refresh)),
                ]),
            ),
            ("reward", DtoValue::Map(reward_map)),
        ]);
        Ok((ret, all_pushes))
    }

    /// Executes `shop.normalShopAutoBuy` (`args = [shopType, [[gid, dbId, num], ...], goldLimit]` -> `[[gid, boughtNum], ...]`).
    pub fn normal_shop_auto_buy(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        quick_tb: &[(u32, u32, u32)],
        gold_limit: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        {
            let sheet = self.ensure_sheet_ext(player.uid, shop_type, false, now_ts);
            sheet.normal_auto_buy_gold_limit = gold_limit;
        }
        let mut gold_spent = 0u64;
        let mut results = Vec::new();
        let mut all_pushes = Vec::new();

        for &(gid, db_id, want_num) in quick_tb {
            if want_num == 0 {
                continue;
            }
            let max_affordable = {
                let sheet = self.ensure_sheet_ext(player.uid, shop_type, false, now_ts);
                let Some(offer) = sheet.offers.iter().find(|o| o.idx == gid || o.id == db_id) else {
                    continue;
                };
                let rem_limit = offer
                    .effective_max_buy(player.vip, sheet.is_multi)
                    .saturating_sub(offer.bought);
                let capped = want_num.min(rem_limit);
                let mut allowed = 0u32;
                for k in 1..=capped {
                    let c = offer.total_cost_for(k);
                    if offer.currency == ShopCurrency::Gold
                        && gold_limit > 0
                        && gold_spent + c > u64::from(gold_limit)
                    {
                        break;
                    }
                    if player.currency_by_price_type(offer.currency.price_type_id()) < c {
                        break;
                    }
                    allowed = k;
                }
                (offer.idx, offer.id, offer.currency == ShopCurrency::Gold, allowed)
            };

            let (idx, id, is_gold, buy_cnt) = max_affordable;
            if buy_cnt == 0 {
                continue;
            }
            let before_gold = player.gold;
            if let Ok((_, bp)) = self.buy_ext(player, shop_type, false, "", idx, id, buy_cnt, now_ts) {
                if is_gold {
                    gold_spent += (before_gold - player.gold).max(0) as u64;
                }
                results.push(DtoValue::array([DtoValue::from(gid), DtoValue::from(buy_cnt)]));
                all_pushes.extend(bp);
            }
        }
        Ok((DtoValue::Array(results), all_pushes))
    }

    /// Emits `shop.open` push (`CS_SHOP_PUSH_UPDATE`) when a timed mystery shop (`shop_type = 12`) triggers.
    pub fn build_mystery_shop_open_push(shop_type: u32, close_ts: u64, now_ts: u64) -> PushPacket {
        PushPacket::new(
            push::SHOP_OPEN,
            DtoValue::map([
                ("shop_type", DtoValue::from(shop_type)),
                ("shopType", DtoValue::from(shop_type)),
                ("open_time", DtoValue::from(now_ts)),
                ("close_time", DtoValue::from(close_ts)),
            ]),
            now_ts,
        )
    }
}

// ============================================================================
// 4. Companion Non-`BaseShopData` Shops (`timeMachineTrader.*`, `babeltowerShop.*`,
//    Limit / Discount / Weekend / Score Exchange Shops)
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct PlayerTraderShopState {
    pub day_bucket: u64,
    pub fund_exp: u32,
    pub fund_high: bool,
    pub claimed_normal: BTreeSet<u32>,
    pub claimed_high: BTreeSet<u32>,
    pub daily_free_bought: u32,
}

/// Time Machine Trader Shop (`Hành Thương Thời Không` — `timeMachineTrader.*` from `TimeMachineTraderShopData.lua`).
#[derive(Debug, Clone, Default)]
pub struct TimeMachineTraderEngine {
    states: BTreeMap<u64, PlayerTraderShopState>,
}

impl TimeMachineTraderEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn sync_day(&mut self, uid: u64, now_ts: u64) -> &mut PlayerTraderShopState {
        let st = self.states.entry(uid).or_default();
        let day = now_ts / 86_400;
        if st.day_bucket == 0 {
            st.day_bucket = day;
            st.fund_exp = 100; // Level 1 unlocked by default
        } else if day > st.day_bucket {
            st.day_bucket = day;
            st.daily_free_bought = 0;
        }
        st
    }

    pub fn get_info(&mut self, uid: u64, now_ts: u64) -> DtoValue {
        let st = self.sync_day(uid, now_ts);
        let mut fund_info = BTreeMap::new();
        for id in 1..=30u32 {
            fund_info.insert(
                id.to_string(),
                DtoValue::map([
                    ("normal", DtoValue::from(st.claimed_normal.contains(&id))),
                    ("high", DtoValue::from(st.claimed_high.contains(&id))),
                ]),
            );
        }
        let mut gift_info = BTreeMap::new();
        gift_info.insert("1".to_owned(), DtoValue::from(st.daily_free_bought));

        DtoValue::map([
            ("fund_exp", DtoValue::from(st.fund_exp)),
            ("fund_high", DtoValue::from(st.fund_high)),
            ("fund_info", DtoValue::Map(fund_info)),
            ("gift_info", DtoValue::Map(gift_info)),
        ])
    }

    /// `timeMachineTrader.buyFreeGift` (`args = [1]`, `timeMachine_shop_charge.json` `id = 1` -> `22102 x25`).
    pub fn buy_free_gift(
        &mut self,
        player: &mut PlayerState,
        gift_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if gift_id != 1 {
            return Err("invalid_trader_gift");
        }
        {
            let st = self.sync_day(player.uid, now_ts);
            if st.daily_free_bought >= 1 {
                return Err("free_gift_already_claimed");
            }
            st.daily_free_bought = 1;
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Item { tid: 22102, qty: 25 }],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    /// `timeMachineTrader.recvFundReward` (`args = [id]` `1..=30`, `timeMachine_shop_fund.json`).
    pub fn recv_fund_reward(
        &mut self,
        player: &mut PlayerState,
        fund_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=30).contains(&fund_id) {
            return Err("invalid_fund_id");
        }
        let req_exp = fund_id * 100;
        let (grant_normal, grant_high) = {
            let st = self.sync_day(player.uid, now_ts);
            if st.fund_exp < req_exp {
                return Err("fund_exp_not_enough");
            }
            let gn = !st.claimed_normal.contains(&fund_id);
            let gh = st.fund_high && !st.claimed_high.contains(&fund_id);
            if !gn && !gh {
                return Err("fund_reward_already_claimed");
            }
            st.claimed_normal.insert(fund_id);
            if st.fund_high {
                st.claimed_high.insert(fund_id);
            }
            (gn, gh)
        };

        let mut entries = Vec::new();
        if grant_normal {
            match fund_id {
                1 | 5 => entries.push(RewardEntry::Copper(50_000_000)),
                2 => entries.push(RewardEntry::Item { tid: 17004, qty: 500 }),
                _ => entries.push(RewardEntry::Item { tid: 10010, qty: 1000 }),
            }
        }
        if grant_high {
            entries.push(RewardEntry::Item { tid: 19181, qty: 50 });
        }
        let pushes = grant_rewards_to_player(player, &entries, now_ts)?;
        Ok((DtoValue::from("ok"), pushes))
    }
}

/// Babel Tower Floor & Sweep Shop (`babeltowerShop.*` from `BabelTowerShopProxy.lua` & `babelTower_item.json`).
#[derive(Debug, Clone, Default)]
pub struct BabelTowerShopEngine {
    /// `uid -> (goods_id -> bought_count)`
    bought: BTreeMap<u64, BTreeMap<u32, u32>>,
    scores: BTreeMap<u64, u64>,
}

impl BabelTowerShopEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn item_price_and_score(goods_id: u32) -> (u64, u64) {
        match goods_id {
            1001..=1009 => (100, 100),
            2001..=2024 => (150, 150),
            3001..=3042 => (200, 200),
            _ => (100, 100),
        }
    }

    /// `babeltowerShop.getShopInfo` (`args = {}`).
    pub fn get_shop_info(&mut self, uid: u64) -> DtoValue {
        let user_bought = self.bought.entry(uid).or_default();
        let mut map = BTreeMap::new();
        for &id in &[1001u32, 1002, 2001, 2017, 3001, 3041, 3042] {
            let cnt = user_bought.get(&id).copied().unwrap_or(0);
            map.insert(id.to_string(), DtoValue::from(cnt));
        }
        DtoValue::Map(map)
    }

    /// `babeltowerShop.buy` (`args = [layer, floorIndex, goodsId, num]`).
    pub fn buy_floor_item(
        &mut self,
        player: &mut PlayerState,
        goods_id: u32,
        num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if num == 0 {
            return Err("invalid_buy_num");
        }
        let (unit_gold, unit_score) = Self::item_price_and_score(goods_id);
        let cost = (unit_gold * u64::from(num)) as i64;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        *self
            .bought
            .entry(player.uid)
            .or_default()
            .entry(goods_id)
            .or_insert(0) += num;
        *self.scores.entry(player.uid).or_insert(0) += unit_score * u64::from(num);
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::from("ok"), pushes))
    }

    /// `babeltowerShop.buyGoods` (`args = [{ [goodsIdStr] = qty, ... }]`).
    pub fn buy_sweep_goods(
        &mut self,
        player: &mut PlayerState,
        buy_tb: &[(u32, u32)],
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if buy_tb.is_empty() {
            return Err("empty_buy_table");
        }
        let mut total_gold = 0u64;
        let mut total_score = 0u64;
        for &(gid, qty) in buy_tb {
            let (ug, us) = Self::item_price_and_score(gid);
            total_gold += ug * u64::from(qty);
            total_score += us * u64::from(qty);
        }
        if player.gold < total_gold as i64 {
            return Err("not_enough_gold");
        }
        player.gold -= total_gold as i64;
        let map = self.bought.entry(player.uid).or_default();
        for &(gid, qty) in buy_tb {
            *map.entry(gid).or_insert(0) += qty;
        }
        *self.scores.entry(player.uid).or_insert(0) += total_score;
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::from("ok"), pushes))
    }
}

/// Activity Limit / Merge / Daily Discount, Weekend Gift & Score Exchange Shop Engine
/// (`discount.*`, `mergeDiscount.*`, `weekGift.*`, `activityWeekGift.*`, `scoreShop.*`).
#[derive(Debug, Clone, Default)]
pub struct ActivityDiscountShopEngine {
    /// `(uid, category, pack_id) -> bought_count`
    purchases: BTreeMap<(u64, &'static str, u32), u32>,
}

impl ActivityDiscountShopEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_info(&self, uid: u64, category: &'static str) -> DtoValue {
        let mut bought = BTreeMap::new();
        for (&(u, cat, pid), &cnt) in &self.purchases {
            if u == uid && cat == category {
                bought.insert(pid.to_string(), DtoValue::from(cnt));
            }
        }
        DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("buy_info", DtoValue::Map(bought)),
        ])
    }

    /// Buys a limit/weekend/score pack (`category` in `{"limit", "weekend", "score"}`).
    #[allow(clippy::too_many_arguments)]
    pub fn buy_pack(
        &mut self,
        player: &mut PlayerState,
        category: &'static str,
        pack_id: u32,
        qty: u32,
        unit_gold_cost: u64,
        max_limit: u32,
        reward_tid: u32,
        reward_qty: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if qty == 0 {
            return Err("invalid_qty");
        }
        let cur = self
            .purchases
            .get(&(player.uid, category, pack_id))
            .copied()
            .unwrap_or(0);
        if cur + qty > max_limit {
            return Err("exceed_pack_limit");
        }
        let total_gold = (unit_gold_cost * u64::from(qty)) as i64;
        if player.gold < total_gold {
            return Err("not_enough_gold");
        }
        player.gold -= total_gold;
        self.purchases
            .insert((player.uid, category, pack_id), cur + qty);

        let deltas = player
            .bag
            .add_item(reward_tid, reward_qty * qty, now_ts)?;
        let mut pushes = Vec::new();
        if let Some(bp) = BagState::build_update_push(&deltas, now_ts) {
            pushes.push(bp);
        }
        pushes.push(PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        ));
        Ok((DtoValue::from("ok"), pushes))
    }
}

// ============================================================================
// 5. Comprehensive Unit Tests (`B-01..B-28`, Secondary Raffles & All Shops)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_795_500_500;

    #[test]
    fn raffle_get_info_includes_hot_and_red_raffle_and_used_free_counts() {
        let mut eng = TavernRaffleEngine::new(7);
        let mut p = PlayerState::new(1, "Tester", 1);
        p.level = 95;
        p.vip = 10;

        let info0 = eng.get_info(1, NOW);
        let r0 = info0.get("raffle").unwrap();
        assert_eq!(r0.get("primary_free_num").and_then(DtoValue::as_u32), Some(0));
        assert_eq!(r0.get("senior_free_num").and_then(DtoValue::as_u32), Some(0));
        assert_eq!(r0.get("hot_free_num").and_then(DtoValue::as_u32), Some(0));
        assert_eq!(r0.get("red_free_num").and_then(DtoValue::as_u32), Some(0));
        assert!(info0.get("hot_raffle").and_then(|h| h.get("va_hots")).is_some());
        assert!(info0.get("red_raffle").and_then(|h| h.get("va_hots")).is_some());
        assert!(
            info0
                .get("hot_raffle")
                .and_then(|h| h.get("hot_refresh_time"))
                .and_then(DtoValue::as_u64)
                .unwrap()
                > NOW
        );

        // VIP 10 has 0s cooldown on Warrior free draws, and `autoRaffle` consumes all remaining free draws!
        let (single_ret, _) = eng
            .draw_standard(&mut p, RafflePool::Warrior, 1, true, NOW)
            .unwrap();
        assert_eq!(single_ret.get("reward").unwrap().as_array().unwrap().len(), 1);
        let (auto_ret, _) = eng.draw_auto_warrior(&mut p, NOW + 1).unwrap();
        assert_eq!(auto_ret.get("reward").unwrap().as_array().unwrap().len(), 4);

        let info1 = eng.get_info(1, NOW + 2);
        assert_eq!(
            info1
                .get("raffle")
                .unwrap()
                .get("primary_free_num")
                .and_then(DtoValue::as_u32),
            Some(5)
        );
    }

    #[test]
    fn raffle_buy_token_1_gold_and_divine_consumes_16013_without_double_charging_gold() {
        let mut eng = TavernRaffleEngine::new(99);
        let mut p = PlayerState::new(2, "Whale", 1);
        p.gold = 5_000;
        p.vip = 5; // Unlocks `open_first_pick` 900 discount!

        // Buy 900 `16013` seals at 1 Gold each (`B-10`)
        let (bt_ret, bt_pushes) = eng.buy_token(&mut p, 900, NOW).unwrap();
        assert_eq!(bt_ret.as_str(), Some("ok"));
        assert_eq!(p.gold, 4_100);
        assert_eq!(p.bag.count_by_tid(16013), 900);
        let (pre, post) = partition_pushes_around_response(bt_pushes);
        assert_eq!(pre[0].callback_name, push::BAG_UPDATE);
        assert_eq!(post[0].callback_name, push::USER_UPDATE);

        // Divine 10-pull consumes `900 x 16013` and does NOT deduct `p.gold` a second time (`B-11`)!
        let (ten_ret, ten_pushes) = eng
            .draw_standard(&mut p, RafflePool::Divine, 10, false, NOW)
            .unwrap();
        assert_eq!(p.gold, 4_100, "must consume 16013, not deduct gold twice");
        assert_eq!(p.bag.count_by_tid(16013), 0);
        let rewards = ten_ret.get("reward").unwrap().as_array().unwrap();
        assert_eq!(rewards.len(), 10);
        // Each reward entry is `{ [tostring(htid)] = hid }` (`B-01`)
        for r in rewards {
            let m = r.as_map().unwrap();
            assert_eq!(m.len(), 1);
        }
        assert!(ten_ret.get("senior_luck_num").and_then(DtoValue::as_u32).unwrap() >= 100);
        assert!(ten_ret.get("luck_boom_num").is_some());
        let (pre_draw, post_draw) = partition_pushes_around_response(ten_pushes);
        assert!(pre_draw.iter().any(|pk| pk.callback_name == push::HERO_NEW));
        assert!(pre_draw.iter().any(|pk| pk.callback_name == push::BAG_UPDATE));
        assert_eq!(post_draw[0].callback_name, push::USER_UPDATE);
    }

    #[test]
    fn raffle_hot_and_red_fragments_pos0_and_luck_box_plain_int_cycle_reset() {
        let mut eng = TavernRaffleEngine::new(123);
        let mut p = PlayerState::new(3, "RedCollector", 1);
        p.gold = 200_000;
        p.vip = 14;

        // `raffleRed({0})` (`pos = 0`) succeeds via `draw_limit_or_red(..., 0, ...)` (`B-13`)
        // and drops valid `50xxx` soul fragments (`B-05`, `B-06`)!
        let (red0, _) = eng
            .draw_limit_or_red(&mut p, RafflePool::RedHero, 0, NOW)
            .unwrap();
        let drop0 = &red0.get("reward").unwrap().as_array().unwrap()[0];
        let item_map = drop0.get("item").unwrap().as_map().unwrap();
        let (tid_str, _) = item_map.iter().next().unwrap();
        let tid: u32 = tid_str.parse().unwrap();
        assert!(
            (50_101..=50_499).contains(&tid) || BONUS_ITEMS.contains(&tid),
            "unexpected red draw tid {tid}"
        );
        assert_eq!(red0.get("red_luck_num").and_then(DtoValue::as_u32), Some(10));

        // Set Divine luck to 1500 and claim all 3 chests (500, 1000, 1500) -> plain integer + cycle reset!
        eng.state_mut(3).divine_luck = 1_500;
        let (c1, _) = eng
            .receive_luck_box(&mut p, RafflePool::Divine, 500, 0, NOW)
            .unwrap();
        assert_eq!(c1, DtoValue::from(1_500u32));
        let (c2, _) = eng
            .receive_luck_box(&mut p, RafflePool::Divine, 1_000, 1, NOW)
            .unwrap();
        assert_eq!(c2, DtoValue::from(1_500u32));
        let (c3, _) = eng
            .receive_luck_box(&mut p, RafflePool::Divine, 1_500, 11, NOW)
            .unwrap();
        assert_eq!(c3, DtoValue::from(0u32), "claiming all 3 chests resets cycle by -1500");

        // Hot refresh stepped pricing 100 -> 200 -> 300 (`B-16`)
        let g_before = p.gold;
        let _ = eng.refresh_hot_by_hand(&mut p, NOW).unwrap();
        assert_eq!(g_before - p.gold, 100);
        let _ = eng.refresh_hot_by_hand(&mut p, NOW).unwrap();
        assert_eq!(g_before - p.gold, 300);
    }

    #[test]
    fn secondary_raffle_hub_covers_godarm_herolegend_soulstone_famous_topanimal_and_arm() {
        let mut hub = SecondaryRaffleHub::new(555);
        let mut p = PlayerState::new(9, "AllModes", 1);
        p.gold = 100_000;
        p.legend_book_coin = 1_000;
        p.bag.add_item(19074, 2, NOW).unwrap();
        p.bag.add_item(19144, 12, NOW).unwrap();
        p.bag.add_item(19207, 2, NOW).unwrap();

        // 1. GodArm 5-pull (`multi_raffle_number = 5`)
        let (ga_info, _) = hub.dispatch_rpc("godarmRaffle.getInfo", &[], &mut p, NOW).unwrap().unwrap();
        assert_eq!(ga_info.get("godarm_free_num").and_then(DtoValue::as_u32), Some(0));
        let (ga_5, _) = hub.dispatch_rpc("godarmRaffle.multiRaffle", &[DtoValue::from(1u32)], &mut p, NOW).unwrap().unwrap();
        assert_eq!(ga_5.get("reward").unwrap().as_array().unwrap().len(), 5);

        // 2. HeroLegend 5-pull (`1888` cost = `1000` legend_book_coin + `888` gold shortfall!)
        let gold_pre_hl = p.gold;
        let (hl_5, _) = hub.dispatch_rpc("heroLegendRaffle.raffle", &[DtoValue::from(5u32)], &mut p, NOW).unwrap().unwrap();
        assert_eq!(p.legend_book_coin, 0);
        assert_eq!(gold_pre_hl - p.gold, 888);
        assert_eq!(hl_5.get("reward").unwrap().as_array().unwrap().len(), 5);

        // 3. SoulStone 10-pull + `{ boxId = 1 }` Map arg `openBox`
        let (ss_10, _) = hub
            .dispatch_rpc(
                "activitySoulStoneRecruit.raffle",
                &[DtoValue::from(1u32), DtoValue::from(true)],
                &mut p,
                NOW,
            )
            .unwrap()
            .unwrap();
        assert_eq!(ss_10.as_array().unwrap().len(), 10);
        hub.festival.soul_stone.get_mut(&9).unwrap().score = 60;
        let (ss_box, _) = hub
            .dispatch_rpc(
                "activitySoulStoneRecruit.openBox",
                &[DtoValue::map([("boxId", DtoValue::from(1u32))])],
                &mut p,
                NOW,
            )
            .unwrap()
            .unwrap();
        assert_eq!(ss_box.as_str(), Some("ok"));

        // 4. FamousHero, TopAnimal & LimitArm
        assert!(hub
            .dispatch_rpc(
                "activityFamousRaffle.raffle",
                &[DtoValue::from(1u32), DtoValue::from(1u32), DtoValue::from(false)],
                &mut p,
                NOW,
            )
            .unwrap()
            .is_ok());
        assert!(hub
            .dispatch_rpc(
                "activityTopAnimalRecruit.raffle",
                &[DtoValue::from(1u32), DtoValue::from(false)],
                &mut p,
                NOW,
            )
            .unwrap()
            .is_ok());
        assert!(hub
            .dispatch_rpc("raffleArm.raffle", &[DtoValue::from(1u32)], &mut p, NOW)
            .unwrap()
            .is_ok());
    }

    #[test]
    fn shop_engine_all_types_awake_365_gold_stepped_exchange_hero_and_refresh_key_1() {
        let mut shop = ShopEngine::new();
        let mut p = PlayerState::new(10, "Shopper", 1);
        p.gold = 50_000;
        p.vip = 2;
        p.rebel_coin = 10_000;
        p.awake_coin = 10_000;
        p.hero_soul = 10_000;

        // 1. `AwakeShop (10)`: real `jewel_ware` IDs >= 365, integer `goods_key = 0` on `getInfo`,
        //    integer `goods_key = 1` + `va_shop[2]` on `refresh`!
        let awake_info = shop.get_info(10, 10, NOW);
        assert_eq!(awake_info.get("goods_key"), Some(&DtoValue::from(0u32)));
        let group1 = awake_info.get("va_shop").unwrap().as_array().unwrap()[0]
            .as_array()
            .unwrap();
        assert_eq!(group1.len(), 6);
        assert!(group1.iter().all(|it| it.get("id").and_then(DtoValue::as_u32).unwrap() >= 365));

        let (ref1, _) = shop.refresh(&mut p, 10, NOW).unwrap();
        assert_eq!(ref1.get("goods_key"), Some(&DtoValue::from(1u32)));
        assert_eq!(ref1.get("cos_type"), Some(&DtoValue::from(SHOP_REFRESH_TYPE_ADD_FREE)));
        assert_eq!(ref1.get("va_shop").unwrap().as_array().unwrap().len(), 2);

        // 2. `GoldShop (8)` stepped pricing on Stamina Pill (`id = 1`, `22102`):
        //    Units 1..=3 cost 20, units 4..=5 cost 30 -> buying 5 costs `20*3 + 30*2 = 120` Gold!
        let _ = shop.get_info(10, 8, NOW);
        let gold_before = p.gold;
        let (_, _) = shop.buy(&mut p, 8, "0", 1, 1, 5, NOW).unwrap();
        assert_eq!(gold_before - p.gold, 120);
        assert_eq!(p.bag.count_by_tid(22102), 5);
        // Verify `num` (NOT only `buy_num`) is populated on subsequent `getInfo`!
        let gold_info2 = shop.get_info(10, 8, NOW);
        let pill_dto = &gold_info2.get("va_shop").unwrap().as_array().unwrap()[0]
            .as_array()
            .unwrap()[0];
        assert_eq!(pill_dto.get("num").and_then(DtoValue::as_u32), Some(5));

        // 3. `ExchangeShop (27)` sacrifices bench hero `htid = 101` and returns `arr_del_hid`!
        let (sac_hero, _) = p.roster.recruit_hero_to_bench(101, 1, NOW);
        let (ex_ret, _) = shop.buy(&mut p, 27, "", 1, 1, 1, NOW).unwrap();
        let del = ex_ret.get("arr_del_hid").unwrap().as_array().unwrap();
        assert_eq!(del[0].as_u64(), Some(sac_hero.hid));
        assert_eq!(p.bag.count_by_tid(19081), 1);

        // 4. `multiRefresh`, `quickBuy`, `normalShopAutoBuy`, `TimeMachineTraderEngine`, `BabelTowerShopEngine`
        let (mr, _) = shop.multi_refresh(&mut p, 3, NOW).unwrap();
        assert_eq!(mr.get("goods_key"), Some(&DtoValue::from(1u32)));
        let (qb, _) = shop
            .quick_buy(&mut p, 3, &[181001, 17002], &[108], 1, 200, 3, NOW)
            .unwrap();
        assert!(qb.get("costType").is_some());

        let (ab, _) = shop
            .normal_shop_auto_buy(&mut p, 8, &[(2, 2, 2)], 200, NOW)
            .unwrap();
        assert_eq!(ab.as_array().unwrap().len(), 1);
        assert_eq!(p.bag.count_by_tid(22202), 2);

        let mut trader = TimeMachineTraderEngine::new();
        assert!(trader.get_info(10, NOW).get("fund_info").is_some());
        let _ = trader.buy_free_gift(&mut p, 1, NOW).unwrap();
        assert_eq!(p.bag.count_by_tid(22102), 30);

        let mut babel = BabelTowerShopEngine::new();
        assert!(babel.get_shop_info(10).as_map().is_some());
        let _ = babel.buy_sweep_goods(&mut p, &[(1001, 2)], NOW).unwrap();

        let mut act_shop = ActivityDiscountShopEngine::new();
        let _ = act_shop
            .buy_pack(&mut p, "weekend", 1, 1, 188, 2, 16012, 5, NOW)
            .unwrap();
        assert_eq!(p.bag.count_by_tid(16012), 5);
    }
}
