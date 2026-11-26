//! Stateful `Hoạt động`, Welfare, Card Pass, Fund, Sign-In, Month Card, and Universal 1-Click
//! `[tm]pay.instantBuy` RPC handlers backed by durable `PlayerStore` WAL + atomic JSON persistence.

use std::collections::BTreeMap;

use gserver_data::GameDataStore;
use gserver_gameplay::engine::GameplayEngine;
use gserver_gameplay::festival_conf::build_activity_conf_dto;
use gserver_player::{
    classify_template_id, BagDelta, CurrencyType, JsonValue as PlayerJson, PlayerError,
    PlayerProfile, PlayerStore,
};
use gserver_protocol::{RpcRequest, S2cEnvelope, Value};

use crate::bridge::{dto_to_amf3, player_json_to_amf3, u64_to_amf3};
use crate::player_rpcs::{
    arg_i64, build_bag_delta_env, build_new_heroes_env, build_user_update_env, map_player_err,
    parse_positive_u32_arg, resync_gameplay_with_store, val_as_i64_loose, PlayerRpcSuccess,
};

type Triplet = (u32, u32, u64);

// ============================================================================
// Exact SQLite Database Tables (`activity.db`, `recharge_dianquan.db`,
// `recharge_current.db`, `timeMachine_recharge_current.db`, `activeTaskShop.db`,
// `activity_eightYears.db`)
// ============================================================================

struct ActDailyRow {
    id: u32,
    goldprice: u64,
    reward1: &'static [Triplet],
    reward2: &'static [Triplet],
}

const ACT_DAILY_ROWS: &[ActDailyRow] = &[
    ActDailyRow {
        id: 1,
        goldprice: 0,
        reward1: &[(2, 0, 20)],
        reward2: &[(4, 22_102, 1), (4, 22_202, 1)],
    },
    ActDailyRow {
        id: 2,
        goldprice: 10,
        reward1: &[(4, 14_374, 1)],
        reward2: &[(2, 0, 100), (1, 0, 500_000)],
    },
    ActDailyRow {
        id: 3,
        goldprice: 20,
        reward1: &[(4, 14_374, 1)],
        reward2: &[(2, 0, 150), (1, 0, 1_000_000)],
    },
    ActDailyRow {
        id: 4,
        goldprice: 60,
        reward1: &[(4, 14_374, 1)],
        reward2: &[(2, 0, 200), (4, 21_045, 1), (4, 28_629, 1)],
    },
    ActDailyRow {
        id: 5,
        goldprice: 600,
        reward1: &[],
        reward2: &[],
    },
    ActDailyRow {
        id: 6,
        goldprice: 240,
        reward1: &[(4, 14_374, 5)],
        reward2: &[(2, 0, 300), (4, 21_045, 5), (4, 28_629, 3)],
    },
    ActDailyRow {
        id: 7,
        goldprice: 600,
        reward1: &[(4, 14_374, 10)],
        reward2: &[(2, 0, 600), (4, 21_045, 10), (4, 28_629, 5)],
    },
    ActDailyRow {
        id: 8,
        goldprice: 880,
        reward1: &[(4, 14_374, 15)],
        reward2: &[(2, 0, 900), (4, 21_045, 15), (4, 28_629, 7)],
    },
    ActDailyRow {
        id: 9,
        goldprice: 5_800,
        reward1: &[(4, 14_374, 50)],
        reward2: &[
            (2, 0, 6_000),
            (4, 21_045, 50),
            (4, 28_629, 15),
            (4, 28_672, 30),
        ],
    },
    ActDailyRow {
        id: 10,
        goldprice: 5_800,
        reward1: &[(4, 14_374, 50)],
        reward2: &[
            (2, 0, 6_000),
            (4, 21_045, 50),
            (4, 28_629, 15),
            (4, 21_046, 40),
        ],
    },
    ActDailyRow {
        id: 11,
        goldprice: 5_800,
        reward1: &[(4, 14_374, 50)],
        reward2: &[
            (2, 0, 6_000),
            (4, 21_045, 50),
            (4, 28_629, 15),
            (4, 28_070, 40),
        ],
    },
    ActDailyRow {
        id: 12,
        goldprice: 5_800,
        reward1: &[(4, 14_374, 50)],
        reward2: &[
            (2, 0, 6_000),
            (4, 21_045, 50),
            (4, 28_629, 15),
            (4, 28_660, 40),
        ],
    },
];

/// `(shop_id, item_tid, item_qty, ticket_14374_cost, monthly_limit)`
const ACT_DAILY_SHOP_ROWS: &[(u32, u32, u32, u32, u32)] = &[
    (1, 27_941, 1, 100, 1),
    (2, 28_030, 1, 200, 1),
    (3, 28_604, 1, 300, 1),
    (4, 28_070, 10, 15, 8),
    (5, 28_672, 10, 20, 6),
    (6, 28_682, 1, 30, 4),
    (7, 28_680, 10, 15, 8),
    (8, 28_070, 10, 20, 8),
    (9, 28_672, 10, 25, 6),
    (10, 28_682, 1, 40, 4),
    (11, 28_680, 10, 20, 8),
    (12, 28_622, 1, 5, 10),
    (13, 28_623, 1, 3, 10),
    (14, 28_622, 1, 4, 5),
    (15, 28_623, 1, 2, 5),
];

/// `(milestone_id, req_points, reward_item_tid)`
const ACT_DAILY_POINT_ROWS: &[(u32, u64, u32)] = &[
    (1, 300, 21_079),
    (2, 3_500, 21_083),
    (3, 10_000, 21_080),
    (4, 20_000, 21_081),
    (5, 30_000, 21_082),
];

struct DailyRecharge2Row {
    id: u32,
    goldprice: u64,
    limit: u32,
    reward: &'static [Triplet],
}

const DAILY_RECHARGE2_ROWS: &[DailyRecharge2Row] = &[
    DailyRecharge2Row {
        id: 1,
        goldprice: 880,
        limit: 1,
        reward: &[(4, 19_135, 10_000), (4, 19_136, 5), (2, 0, 880)],
    },
    DailyRecharge2Row {
        id: 2,
        goldprice: 5_800,
        limit: 20,
        reward: &[(4, 19_136, 99), (4, 19_144, 20), (2, 0, 5_800)],
    },
    DailyRecharge2Row {
        id: 3,
        goldprice: 5_800,
        limit: 10,
        reward: &[(4, 19_144, 50), (4, 4_700_110, 300), (2, 0, 5_800)],
    },
    DailyRecharge2Row {
        id: 4,
        goldprice: 5_800,
        limit: 2,
        reward: &[(4, 19_135, 50_000), (2, 0, 5_800)],
    },
    DailyRecharge2Row {
        id: 5,
        goldprice: 880,
        limit: 5,
        reward: &[(4, 19_155, 30), (4, 19_136, 5), (2, 0, 880)],
    },
];

struct WeekGiftRow {
    id: u32,
    min_lv: u32,
    max_lv: u32,
    tabs: u8,
    kind: u8, // 1 = gold/free, 2 = recharge
    gold_cost: u64,
    recharge_rmb: u64,
    buy_times: u32,
    reward: &'static [Triplet],
}

const ACT_WEEK_GIFT_ROWS: &[WeekGiftRow] = &[
    // Tab 1 (day_gift) Lv 1..100
    WeekGiftRow { id: 1, min_lv: 1, max_lv: 100, tabs: 1, kind: 1, gold_cost: 0, recharge_rmb: 0, buy_times: 1, reward: &[(4, 16_012, 2), (2, 0, 88), (4, 22_102, 4)] },
    WeekGiftRow { id: 2, min_lv: 1, max_lv: 100, tabs: 1, kind: 1, gold_cost: 128, recharge_rmb: 0, buy_times: 1, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 3, min_lv: 1, max_lv: 100, tabs: 1, kind: 1, gold_cost: 358, recharge_rmb: 0, buy_times: 2, reward: &[(4, 16_012, 2), (4, 22_102, 4), (2, 0, 88)] },
    WeekGiftRow { id: 4, min_lv: 1, max_lv: 100, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 6, buy_times: 4, reward: &[(4, 33_006, 1), (2, 0, 888), (4, 29_237, 1)] },
    WeekGiftRow { id: 5, min_lv: 1, max_lv: 100, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 98, buy_times: 3, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 6, min_lv: 1, max_lv: 100, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 418, buy_times: 1, reward: &[(4, 114_003, 1), (4, 16_012, 8), (2, 0, 1_688)] },
    // Tab 1 (day_gift) Lv 101..130
    WeekGiftRow { id: 7, min_lv: 101, max_lv: 130, tabs: 1, kind: 1, gold_cost: 0, recharge_rmb: 0, buy_times: 1, reward: &[(4, 27_600, 40), (4, 10_010, 888), (2, 0, 288)] },
    WeekGiftRow { id: 8, min_lv: 101, max_lv: 130, tabs: 1, kind: 1, gold_cost: 128, recharge_rmb: 0, buy_times: 1, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 9, min_lv: 101, max_lv: 130, tabs: 1, kind: 1, gold_cost: 358, recharge_rmb: 0, buy_times: 2, reward: &[(4, 27_403, 10), (4, 200_003, 10), (2, 0, 188)] },
    WeekGiftRow { id: 10, min_lv: 101, max_lv: 130, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 6, buy_times: 4, reward: &[(4, 27_600, 60), (4, 10_010, 1_888), (2, 0, 588)] },
    WeekGiftRow { id: 11, min_lv: 101, max_lv: 130, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 98, buy_times: 3, reward: &[(4, 28_006, 2), (4, 181_002, 10), (4, 10_012, 250)] },
    WeekGiftRow { id: 12, min_lv: 101, max_lv: 130, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 418, buy_times: 1, reward: &[(4, 27_403, 60), (4, 32_002, 1), (4, 200_003, 50)] },
    // Tab 1 (day_gift) Lv 131..999
    WeekGiftRow { id: 13, min_lv: 131, max_lv: 999, tabs: 1, kind: 1, gold_cost: 0, recharge_rmb: 0, buy_times: 1, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 14, min_lv: 131, max_lv: 999, tabs: 1, kind: 1, gold_cost: 128, recharge_rmb: 0, buy_times: 1, reward: &[(4, 200_003, 10), (4, 10_018, 10), (2, 0, 488)] },
    WeekGiftRow { id: 15, min_lv: 131, max_lv: 999, tabs: 1, kind: 1, gold_cost: 358, recharge_rmb: 0, buy_times: 2, reward: &[(4, 29_045, 4), (4, 22_102, 5), (2, 0, 288)] },
    WeekGiftRow { id: 16, min_lv: 131, max_lv: 999, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 6, buy_times: 4, reward: &[(4, 27_507, 30), (4, 10_020, 150), (4, 10_018, 50)] },
    WeekGiftRow { id: 17, min_lv: 131, max_lv: 999, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 98, buy_times: 3, reward: &[(4, 200_003, 50), (4, 10_018, 50), (2, 0, 1_288)] },
    WeekGiftRow { id: 18, min_lv: 131, max_lv: 999, tabs: 1, kind: 2, gold_cost: 0, recharge_rmb: 418, buy_times: 1, reward: &[(4, 29_040, 1), (4, 29_237, 1), (2, 0, 888)] },
    // Tab 2 (week_gift) Lv 1..100
    WeekGiftRow { id: 19, min_lv: 1, max_lv: 100, tabs: 2, kind: 1, gold_cost: 0, recharge_rmb: 0, buy_times: 1, reward: &[(4, 16_012, 2), (2, 0, 88), (4, 22_102, 4)] },
    WeekGiftRow { id: 20, min_lv: 1, max_lv: 100, tabs: 2, kind: 1, gold_cost: 1_288, recharge_rmb: 0, buy_times: 1, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 21, min_lv: 1, max_lv: 100, tabs: 2, kind: 1, gold_cost: 3_588, recharge_rmb: 0, buy_times: 2, reward: &[(4, 16_012, 2), (4, 22_102, 4), (2, 0, 88)] },
    WeekGiftRow { id: 22, min_lv: 1, max_lv: 100, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 30, buy_times: 5, reward: &[(4, 33_006, 1), (2, 0, 888), (4, 29_237, 1)] },
    WeekGiftRow { id: 23, min_lv: 1, max_lv: 100, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 68, buy_times: 4, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 24, min_lv: 1, max_lv: 100, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 168, buy_times: 3, reward: &[(4, 114_003, 1), (4, 16_012, 8), (2, 0, 1_688)] },
    WeekGiftRow { id: 25, min_lv: 1, max_lv: 100, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 268, buy_times: 2, reward: &[(4, 27_600, 40), (4, 10_010, 888), (2, 0, 288)] },
    WeekGiftRow { id: 26, min_lv: 1, max_lv: 100, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 648, buy_times: 1, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    // Tab 2 (week_gift) Lv 101..130
    WeekGiftRow { id: 27, min_lv: 101, max_lv: 130, tabs: 2, kind: 1, gold_cost: 0, recharge_rmb: 0, buy_times: 1, reward: &[(4, 27_403, 10), (4, 200_003, 10), (2, 0, 188)] },
    WeekGiftRow { id: 28, min_lv: 101, max_lv: 130, tabs: 2, kind: 1, gold_cost: 1_288, recharge_rmb: 0, buy_times: 1, reward: &[(4, 27_600, 60), (4, 10_010, 1_888), (2, 0, 588)] },
    WeekGiftRow { id: 29, min_lv: 101, max_lv: 130, tabs: 2, kind: 1, gold_cost: 3_588, recharge_rmb: 0, buy_times: 2, reward: &[(4, 28_006, 2), (4, 181_002, 10), (4, 10_012, 250)] },
    WeekGiftRow { id: 30, min_lv: 101, max_lv: 130, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 30, buy_times: 5, reward: &[(4, 27_403, 60), (4, 32_002, 1), (4, 200_003, 50)] },
    WeekGiftRow { id: 31, min_lv: 101, max_lv: 130, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 68, buy_times: 4, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 32, min_lv: 101, max_lv: 130, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 168, buy_times: 3, reward: &[(4, 200_003, 10), (4, 10_018, 10), (2, 0, 488)] },
    WeekGiftRow { id: 33, min_lv: 101, max_lv: 130, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 268, buy_times: 2, reward: &[(4, 29_045, 4), (4, 22_102, 5), (2, 0, 288)] },
    WeekGiftRow { id: 34, min_lv: 101, max_lv: 130, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 648, buy_times: 1, reward: &[(4, 27_507, 30), (4, 10_020, 150), (4, 10_018, 50)] },
    // Tab 2 (week_gift) Lv 131..999
    WeekGiftRow { id: 35, min_lv: 131, max_lv: 999, tabs: 2, kind: 1, gold_cost: 0, recharge_rmb: 0, buy_times: 1, reward: &[(4, 200_003, 50), (4, 10_018, 50), (2, 0, 1_288)] },
    WeekGiftRow { id: 36, min_lv: 131, max_lv: 999, tabs: 2, kind: 1, gold_cost: 1_288, recharge_rmb: 0, buy_times: 1, reward: &[(4, 29_040, 1), (4, 29_237, 1), (2, 0, 888)] },
    WeekGiftRow { id: 37, min_lv: 131, max_lv: 999, tabs: 2, kind: 1, gold_cost: 3_588, recharge_rmb: 0, buy_times: 2, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 38, min_lv: 131, max_lv: 999, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 30, buy_times: 5, reward: &[(4, 29_045, 4), (4, 22_102, 5), (2, 0, 288)] },
    WeekGiftRow { id: 39, min_lv: 131, max_lv: 999, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 68, buy_times: 4, reward: &[(4, 28_002, 4), (4, 22_102, 3), (4, 22_202, 3), (4, 181_002, 15)] },
    WeekGiftRow { id: 40, min_lv: 131, max_lv: 999, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 168, buy_times: 3, reward: &[(4, 200_003, 50), (4, 10_018, 50), (2, 0, 1_288)] },
    WeekGiftRow { id: 41, min_lv: 131, max_lv: 999, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 268, buy_times: 2, reward: &[(4, 29_040, 1), (4, 29_237, 1), (2, 0, 888)] },
    WeekGiftRow { id: 42, min_lv: 131, max_lv: 999, tabs: 2, kind: 2, gold_cost: 0, recharge_rmb: 648, buy_times: 1, reward: &[(4, 29_035, 80), (4, 19_007, 200), (4, 19_008, 400)] },
];

const QUARTER_REWARDS: [&[Triplet]; 4] = [
    &[(4, 21_150, 3), (4, 28_036, 1), (4, 20_706, 1)],
    &[(4, 28_036, 1), (4, 28_634, 3), (4, 20_911, 1)],
    &[(4, 29_035, 20), (4, 21_000, 5), (2, 0, 1_000)],
    &[(4, 27_895, 10), (4, 21_000, 5), (2, 0, 500)],
];

const FIRST_GIFT_TIERS: &[(u32, u64, &[Triplet])] = &[
    (1, 60, &[(4, 27_501, 1), (4, 22_202, 2), (4, 22_102, 2)]),
    (2, 980, &[(4, 115_003, 1), (4, 22_202, 5), (4, 17_004, 5)]),
    (3, 2_880, &[(4, 28_937, 1), (4, 29_004, 1), (4, 22_102, 5)]),
];

/// `(dianquan_id, voucher_coins, android_pay_id, ios_pay_id)`
const DIANQUAN_ROWS: &[(u32, u64, u32, u32)] = &[
    (1, 60, 2701, 2601),
    (2, 300, 2702, 2602),
    (3, 680, 2703, 2603),
    (4, 980, 2704, 2604),
    (5, 1_680, 2705, 2605),
    (6, 2_680, 2706, 2606),
    (7, 4_180, 2707, 2607),
    (8, 6_480, 2708, 2608),
    (9, 20_000, 2709, 2609),
];

const RECHARGE_CURRENT_ROWS: &[(u32, u64, &[Triplet])] = &[
    (1, 240, &[(4, 19_133, 1), (4, 19_135, 3_000), (2, 0, 240)]),
    (2, 1_500, &[(4, 19_133, 3), (4, 19_135, 15_000), (2, 0, 1_500)]),
    (3, 5_800, &[(4, 19_133, 3), (4, 19_136, 50), (2, 0, 5_800)]),
];

const TM_RECHARGE_CURRENT_ROWS: &[(u32, u64, &[Triplet])] = &[
    (1, 240, &[(4, 19_133, 1), (4, 5_209_005, 2)]),
    (2, 600, &[(4, 19_133, 3), (4, 5_209_005, 3)]),
    (3, 2_500, &[(4, 19_133, 3), (4, 5_209_005, 10)]),
];

/// `(ware_id, out_tid, out_qty, ticket_19903_cost, buy_limit)`
const ACTIVE_TASK_SHOP_WARES: &[(u32, u32, u32, u32, u32)] = &[
    (1, 19_903, 20, 10, 1),
    (2, 39_238, 10, 50, 1),
    (3, 19_144, 2, 10, 1),
    (4, 19_903, 30, 15, 1),
    (5, 39_238, 20, 100, 1),
    (6, 19_144, 4, 10, 1),
    (7, 4_700_110, 20, 80, 5),
    (8, 39_238, 40, 150, 1),
    (9, 19_144, 6, 10, 1),
    (10, 19_136, 5, 40, 2),
    (11, 39_238, 80, 200, 1),
    (12, 19_144, 8, 10, 1),
    (13, 19_136, 15, 120, 6),
    (14, 20_080, 80, 250, 1),
    (15, 19_144, 10, 10, 1),
];

/// `(gift_id, gold_cost_opt, recharge_nb, buy_times, reward)`
type ActiveTaskShopGiftRow = (u32, Option<u64>, u64, u32, &'static [Triplet]);
const ACTIVE_TASK_SHOP_GIFTS: &[ActiveTaskShopGiftRow] = &[
    (1, Some(1_888), 0, 1, &[(4, 19_903, 30)]),
    (2, None, 240, 1, &[(4, 19_903, 45), (2, 0, 240)]),
    (3, None, 880, 2, &[(4, 19_903, 90), (2, 0, 880)]),
    (4, None, 1_500, 3, &[(4, 19_903, 120), (2, 0, 1_500)]),
    (5, None, 2_500, 5, &[(4, 19_903, 170), (2, 0, 2_500)]),
];

const EIGHT_YEARS_CHARGES: &[(u32, u64, &[Triplet])] = &[
    (1, 880, &[]),
    (2, 240, &[]),
    (3, 1_500, &[]),
    (4, 0, &[(95, 0, 1), (2, 0, 188)]),
    (5, 240, &[(2, 0, 240), (95, 0, 6), (4, 20_911, 1)]),
    (6, 880, &[(2, 0, 880), (95, 0, 19), (4, 20_344, 1), (4, 19_129, 2), (4, 20_911, 3)]),
    (7, 1_500, &[(2, 0, 1_500), (95, 0, 33), (4, 20_344, 2), (4, 19_129, 4), (4, 20_911, 5)]),
    (8, 5_800, &[(2, 0, 5_800), (95, 0, 130), (4, 20_344, 6), (4, 19_129, 12), (4, 20_911, 20)]),
    (9, 240, &[(2, 0, 240), (97, 0, 1), (4, 20_911, 1)]),
    (10, 880, &[(2, 0, 880), (97, 0, 3), (4, 20_344, 1), (4, 19_129, 2), (4, 20_911, 3)]),
    (11, 1_500, &[(2, 0, 1_500), (97, 0, 5), (4, 20_344, 2), (4, 19_129, 4), (4, 20_911, 5)]),
    (12, 5_800, &[(2, 0, 5_800), (97, 0, 21), (4, 20_344, 6), (4, 19_129, 12), (4, 20_911, 20)]),
];

// ============================================================================
// Common Helpers (`PlayerJson` state access, Triplet Granting, AMF3 Triplets)
// ============================================================================

fn get_sub_obj(profile: &PlayerProfile, key: &str) -> BTreeMap<String, PlayerJson> {
    profile
        .event_state
        .get(key)
        .and_then(PlayerJson::as_object)
        .cloned()
        .unwrap_or_default()
}

fn set_sub_obj(profile: &mut PlayerProfile, key: &str, obj: BTreeMap<String, PlayerJson>) {
    profile.event_state.insert(key.to_owned(), PlayerJson::Object(obj));
}

fn obj_u64(map: &BTreeMap<String, PlayerJson>, key: &str, default: u64) -> u64 {
    map.get(key).and_then(PlayerJson::as_u64).unwrap_or(default)
}

fn obj_bool(map: &BTreeMap<String, PlayerJson>, key: &str, default: bool) -> bool {
    map.get(key).and_then(PlayerJson::as_bool).unwrap_or(default)
}

fn obj_nested(map: &BTreeMap<String, PlayerJson>, key: &str) -> BTreeMap<String, PlayerJson> {
    map.get(key)
        .and_then(PlayerJson::as_object)
        .cloned()
        .unwrap_or_default()
}

fn triplets_to_amf3(triplets: &[Triplet]) -> Value {
    Value::dense_array(
        triplets
            .iter()
            .map(|&(pt, tid, qty)| {
                Value::dense_array(vec![
                    Value::Int(pt as i32),
                    Value::Int(tid as i32),
                    u64_to_amf3(qty),
                ])
            })
            .collect(),
    )
}

fn add_pass_score(profile: &mut PlayerProfile, state_key: &str, add_score: u64) {
    let mut m = get_sub_obj(profile, state_key);
    let prev = obj_u64(&m, "score", 0);
    m.insert("score".into(), PlayerJson::from(prev.saturating_add(add_score)));
    set_sub_obj(profile, state_key, m);
}

fn add_eight_years_currency(profile: &mut PlayerProfile, field: &str, add_val: u64) {
    let mut m = get_sub_obj(profile, "eight_years");
    let prev = obj_u64(&m, field, 0);
    m.insert(field.into(), PlayerJson::from(prev.saturating_add(add_val)));
    set_sub_obj(profile, "eight_years", m);
}

/// Grants a slice of `(price_type, tid, qty)` reward triplets into `profile`, auto-expanding a sub-bag
/// if needed so IAP / event claims never fail on full sub-bags.
pub(crate) fn grant_triplets(
    profile: &mut PlayerProfile,
    triplets: &[Triplet],
    now_ts: u64,
) -> Result<(BagDelta, Vec<u64>), PlayerError> {
    let mut delta = BagDelta::new();
    let mut hids = Vec::new();
    for &(pt, tid, qty) in triplets {
        if qty == 0 {
            continue;
        }
        match pt {
            1 => {
                profile.grant_currency(CurrencyType::Copper, qty, now_ts);
            }
            2 => {
                profile.grant_currency(CurrencyType::Gold, qty, now_ts);
            }
            3 => {
                profile.grant_currency(CurrencyType::Execution, qty, now_ts);
            }
            4 => {
                if tid >= 10_000 {
                    if let Ok((_, bt)) = classify_template_id(tid) {
                        let cur_len = profile.inventory.bags.get(&bt).map_or(0, BTreeMap::len) as u32;
                        let cur_max = profile.inventory.grid_max_num.get(&bt).copied().unwrap_or_else(|| bt.default_max_grids());
                        if cur_len + 2 >= cur_max {
                            profile.inventory.grid_max_num.insert(bt, cur_max + 20);
                        }
                    }
                    let count_u32 = qty.min(u64::from(u32::MAX)) as u32;
                    let (_, d) = profile.add_item_with_delta(tid, count_u32, now_ts)?;
                    delta.merge(d);
                } else if (101..=999).contains(&tid) {
                    for _ in 0..qty.min(10) {
                        if !gserver_data::is_protagonist_htid(tid) {
                            if (profile.heroes.len() as u32) >= profile.max_hero_capacity() {
                                profile.hero_bag_buy_times = profile.hero_bag_buy_times.saturating_add(2);
                            }
                            let hid = profile.summon_hero(tid, 1)?;
                            hids.push(hid);
                        }
                    }
                }
            }
            5 | 100 => {
                if tid > 0 {
                    for _ in 0..qty.min(10) {
                        if !gserver_data::is_protagonist_htid(tid) {
                            if (profile.heroes.len() as u32) >= profile.max_hero_capacity() {
                                profile.hero_bag_buy_times = profile.hero_bag_buy_times.saturating_add(2);
                            }
                            let hid = profile.summon_hero(tid, 1)?;
                            hids.push(hid);
                        }
                    }
                }
            }
            6 => {
                profile.grant_currency(CurrencyType::Energy, qty, now_ts);
            }
            7 => {
                profile.grant_currency(CurrencyType::Fame, qty, now_ts);
            }
            8 => {
                profile.grant_currency(CurrencyType::GoldBind, qty, now_ts);
            }
            9 => {
                profile.grant_currency(CurrencyType::Prestige, qty, now_ts);
                profile.grant_currency(CurrencyType::ArenaCoin, qty, now_ts);
            }
            10 | 15 => {
                profile.grant_currency(CurrencyType::HeroSoul, qty, now_ts);
            }
            11 | 17 => {
                profile.grant_currency(CurrencyType::TowerCoin, qty, now_ts);
            }
            12 => {
                profile.grant_currency(CurrencyType::ClanContri, qty, now_ts);
                profile.grant_currency(CurrencyType::GuildPoint, qty, now_ts);
            }
            13 => {
                profile.grant_currency(CurrencyType::BossCoin, qty, now_ts);
            }
            14 | 23 => {
                profile.grant_currency(CurrencyType::AwakeArmCoin, qty, now_ts);
                profile.grant_currency(CurrencyType::JewelNum, qty, now_ts);
            }
            16 => {
                profile.grant_currency(CurrencyType::RebelCoin, qty, now_ts);
            }
            18 => {
                profile.grant_currency(CurrencyType::PetCoin, qty, now_ts);
            }
            19 => {
                profile.grant_currency(CurrencyType::WorldCoin, qty, now_ts);
            }
            20 => {
                profile.grant_currency(CurrencyType::LordwarCoin, qty, now_ts);
            }
            21 => {
                profile.grant_currency(CurrencyType::GodarmCoin, qty, now_ts);
            }
            22 => {
                profile.grant_currency(CurrencyType::GodanimalCoin, qty, now_ts);
            }
            73 => add_pass_score(profile, "act_weekly", qty),
            74 => add_pass_score(profile, "act_monthly", qty),
            87 => {
                profile.grant_currency(CurrencyType::VoucherCoin, qty, now_ts);
            }
            90 => add_pass_score(profile, "act_beauty", qty),
            94 => add_eight_years_currency(profile, "edict_score", qty),
            95 => add_eight_years_currency(profile, "lucky_ticket", qty),
            96 => add_eight_years_currency(profile, "year_score", qty),
            97 => add_eight_years_currency(profile, "wheel_ticket", qty),
            99 => {
                profile.grant_currency(CurrencyType::TimeMachineVoucherCoin, qty, now_ts);
            }
            _ => {
                profile.grant_currency(CurrencyType::Gold, qty, now_ts);
            }
        }
    }
    Ok((delta, hids))
}

/// Consume optional voucher ticket item from bag if player holds one (e.g. `moneytransferticket`),
/// returning any resulting `BagDelta`. Never fails if no physical ticket item is held.
fn try_consume_any_voucher_ticket(profile: &mut PlayerProfile, ticket_tids: &[u32], _now_ts: u64) -> BagDelta {
    let mut delta = BagDelta::new();
    for &tid in ticket_tids {
        if tid >= 10_000 && profile.inventory.count_by_template(tid) > 0 {
            if let Ok((_, d)) = profile.remove_item_by_template_with_delta(tid, 1) {
                delta.merge(d);
                break;
            }
        }
    }
    delta
}

/// Applies side-effect unlocks across WeekGift (`type == 2`), NewWeekFund, RechargeFund, and MonthFund
/// whenever a recharge of `gold_equiv` NB occurs!
fn record_full_recharge_with_festivals(profile: &mut PlayerProfile, gold_equiv: u64, now_ts: u64) {
    profile.record_recharge_nb(gold_equiv, now_ts);
    let rmb = (gold_equiv / 10).max(1);

    // Unlock matching `activityWeekGift` `type == 2` recharge packs for the player's level bracket
    unlock_week_gift_by_rmb(profile, rmb, now_ts);

    // Unlock `activity_week_fund` (`30` RMB = 300 NB, `98` RMB = 980 NB)
    let mut wf = get_sub_obj(profile, "week_fund");
    let mut va_open = obj_nested(&wf, "va_open_time");
    if gold_equiv >= 300 && obj_u64(&va_open, "1", 0) == 0 {
        va_open.insert("1".into(), PlayerJson::from(now_ts));
    }
    if gold_equiv >= 980 && obj_u64(&va_open, "2", 0) == 0 {
        va_open.insert("2".into(), PlayerJson::from(now_ts));
    }
    wf.insert("va_open_time".into(), PlayerJson::Object(va_open));
    set_sub_obj(profile, "week_fund", wf);

    // Unlock `activity_recharge_fund` (`30` / `98` / `328` RMB)
    let mut rf = get_sub_obj(profile, "recharge_fund");
    let mut va_buy = obj_nested(&rf, "va_buy");
    for (idx_k, req_nb) in [("1", 300u64), ("2", 980), ("3", 3_280)] {
        if gold_equiv >= req_nb && obj_u64(&va_buy, idx_k, 0) == 0 {
            va_buy.insert(idx_k.into(), PlayerJson::from(1u32));
        }
    }
    rf.insert("va_buy".into(), PlayerJson::Object(va_buy));
    set_sub_obj(profile, "recharge_fund", rf);

    // Update `activity_month_fund` if month/supreme card is active or threshold met
    refresh_month_fund_recharge_state(profile, gold_equiv, now_ts);
}

// ============================================================================
// Module State Builders & Mutators
// ============================================================================

// --- 1. `ActDaily` (`[tm]dailyDiscount.*`) ---

struct NormalizedDailyState {
    day_bucket: u64,
    week_bucket: u64,
    month_bucket: u64,
    daily_bought: BTreeMap<String, PlayerJson>,
    weekly_bought: BTreeMap<String, PlayerJson>,
    monthly_bought: BTreeMap<String, PlayerJson>,
    prebuy_until_day: u64,
    day_rec_num: u64,
    day_counted_bucket: u64,
    redeem: BTreeMap<String, PlayerJson>,
    recharge_sum: u64,
    point_claimed: BTreeMap<String, PlayerJson>,
}

impl NormalizedDailyState {
    fn load(profile: &PlayerProfile, now_ts: u64) -> Self {
        let day_bucket = now_ts / 86_400;
        let week_bucket = now_ts / (7 * 86_400);
        let month_bucket = now_ts / (30 * 86_400);
        let raw = get_sub_obj(profile, "daily_discount");

        let saved_day = obj_u64(&raw, "day_bucket", u64::MAX);
        let saved_week = obj_u64(&raw, "week_bucket", u64::MAX);
        let saved_month = obj_u64(&raw, "month_bucket", u64::MAX);

        let daily_bought = if saved_day == day_bucket {
            obj_nested(&raw, "daily_bought")
        } else {
            BTreeMap::new()
        };
        let weekly_bought = if saved_week == week_bucket {
            obj_nested(&raw, "weekly_bought")
        } else {
            BTreeMap::new()
        };
        let monthly_bought = if saved_month == month_bucket {
            obj_nested(&raw, "monthly_bought")
        } else {
            BTreeMap::new()
        };
        let redeem = if saved_month == month_bucket {
            obj_nested(&raw, "redeem")
        } else {
            BTreeMap::new()
        };
        let recharge_sum = if saved_month == month_bucket {
            obj_u64(&raw, "recharge_sum", 0)
        } else {
            0
        };
        let point_claimed = if saved_month == month_bucket {
            obj_nested(&raw, "point_claimed")
        } else {
            BTreeMap::new()
        };

        Self {
            day_bucket,
            week_bucket,
            month_bucket,
            daily_bought,
            weekly_bought,
            monthly_bought,
            prebuy_until_day: obj_u64(&raw, "prebuy_until_day", 0),
            day_rec_num: obj_u64(&raw, "day_rec_num", 0),
            day_counted_bucket: obj_u64(&raw, "day_counted_bucket", 0),
            redeem,
            recharge_sum,
            point_claimed,
        }
    }

    fn is_prebuy_active(&self) -> bool {
        self.prebuy_until_day > 0 && self.day_bucket <= self.prebuy_until_day
    }

    fn is_bought(&self, id: u32) -> bool {
        let k = id.to_string();
        match id {
            1..=4 => self.daily_bought.get(&k).and_then(PlayerJson::as_u64).unwrap_or(0) > 0,
            5 => self.is_prebuy_active(),
            6..=8 => self.weekly_bought.get(&k).and_then(PlayerJson::as_u64).unwrap_or(0) > 0,
            9..=12 => self.monthly_bought.get(&k).and_then(PlayerJson::as_u64).unwrap_or(0) > 0,
            _ => false,
        }
    }

    fn mark_bought(&mut self, id: u32) {
        let k = id.to_string();
        match id {
            1..=4 => {
                self.daily_bought.insert(k, PlayerJson::from(1u32));
            }
            5 => {
                self.prebuy_until_day = self.day_bucket + 7;
            }
            6..=8 => {
                self.weekly_bought.insert(k, PlayerJson::from(1u32));
            }
            9..=12 => {
                self.monthly_bought.insert(k, PlayerJson::from(1u32));
            }
            _ => {}
        }
    }

    fn maybe_bump_day_rec_num(&mut self, force_on_onekey: bool) {
        let all_daily_done = (1..=4).all(|id| self.is_bought(id));
        if (all_daily_done || force_on_onekey) && self.day_counted_bucket != self.day_bucket {
            self.day_rec_num = self.day_rec_num.saturating_add(1);
            self.day_counted_bucket = self.day_bucket;
        }
    }

    fn can_rec_map(&self) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        if !self.is_bought(1) {
            out.push(("1".into(), Value::Int(1)));
        }
        if self.is_prebuy_active() {
            for id in 2..=4u32 {
                if !self.is_bought(id) {
                    out.push((id.to_string(), Value::Int(1)));
                }
            }
        }
        out
    }

    fn save(&self, profile: &mut PlayerProfile) {
        let mut raw = BTreeMap::new();
        raw.insert("day_bucket".into(), PlayerJson::from(self.day_bucket));
        raw.insert("week_bucket".into(), PlayerJson::from(self.week_bucket));
        raw.insert("month_bucket".into(), PlayerJson::from(self.month_bucket));
        raw.insert("daily_bought".into(), PlayerJson::Object(self.daily_bought.clone()));
        raw.insert("weekly_bought".into(), PlayerJson::Object(self.weekly_bought.clone()));
        raw.insert("monthly_bought".into(), PlayerJson::Object(self.monthly_bought.clone()));
        raw.insert("prebuy_until_day".into(), PlayerJson::from(self.prebuy_until_day));
        raw.insert("day_rec_num".into(), PlayerJson::from(self.day_rec_num));
        raw.insert("day_counted_bucket".into(), PlayerJson::from(self.day_counted_bucket));
        raw.insert("redeem".into(), PlayerJson::Object(self.redeem.clone()));
        raw.insert("recharge_sum".into(), PlayerJson::from(self.recharge_sum));
        raw.insert("point_claimed".into(), PlayerJson::Object(self.point_claimed.clone()));
        set_sub_obj(profile, "daily_discount", raw);
    }

    fn to_get_info_value(&self) -> Value {
        let mut prod = Vec::new();
        for id in 1..=12u32 {
            if id != 5 && self.is_bought(id) {
                prod.push((id.to_string(), Value::Int(1)));
            }
        }
        let product_buy = if prod.is_empty() {
            Value::empty_array()
        } else {
            Value::map(prod)
        };

        let can_rec_entries = self.can_rec_map();
        let can_rec_val = if can_rec_entries.is_empty() {
            Value::empty_array()
        } else {
            Value::map(can_rec_entries)
        };

        let redeem_entries: Vec<(String, Value)> = self
            .redeem
            .iter()
            .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), Value::Int(n as i32))))
            .collect();
        let redeem_val = if redeem_entries.is_empty() {
            Value::empty_array()
        } else {
            Value::map(redeem_entries)
        };

        let mut pt_entries = Vec::with_capacity(5);
        for &(mid, req_pt, _) in ACT_DAILY_POINT_ROWS {
            let k = mid.to_string();
            let claimed = self.point_claimed.get(&k).and_then(PlayerJson::as_u64).unwrap_or(0) == 3;
            let st = if claimed {
                3
            } else if self.recharge_sum >= req_pt {
                2
            } else {
                1
            };
            pt_entries.push((k, Value::Int(st)));
        }

        Value::map([
            ("productBuy", product_buy),
            ("canRecDayPreBuy", can_rec_val),
            ("redeem", redeem_val),
            ("canDayPreBuy", Value::Bool(!self.is_prebuy_active())),
            ("dayRecNum", Value::Int(self.day_rec_num as i32)),
            ("dayPreBuyTomorrow", Value::Bool(false)),
            ("rechargeSum", u64_to_amf3(self.recharge_sum)),
            ("rechargePointRec", Value::map(pt_entries)),
        ])
    }
}

fn fulfill_act_daily_purchase(
    profile: &mut PlayerProfile,
    id: u32,
    vip_exp: u64,
    is_tm: bool,
    now_ts: u64,
) -> Result<(BagDelta, Vec<u64>, S2cEnvelope, Vec<Triplet>), PlayerError> {
    let Some(row) = ACT_DAILY_ROWS.iter().find(|r| r.id == id) else {
        return Err(PlayerError::InvalidParameter("invalid_daily_discount_id".into()));
    };
    let mut st = NormalizedDailyState::load(profile, now_ts);
    let currency_num = row.goldprice.max(vip_exp);
    st.recharge_sum = st.recharge_sum.saturating_add(currency_num);

    let now_i64 = now_ts as i64;
    if id == 5 {
        st.mark_bought(5);
        let can_rec = st.can_rec_map();
        st.save(profile);
        let cb = if is_tm {
            "tmDailyDiscount.preBuyUpdate"
        } else {
            "dailyDiscount.preBuyUpdate"
        };
        let can_rec_val = if can_rec.is_empty() {
            Value::empty_array()
        } else {
            Value::map(can_rec)
        };
        let push = S2cEnvelope::push(
            cb,
            Value::map([
                ("vipExp", u64_to_amf3(vip_exp)),
                ("currencyNum", u64_to_amf3(currency_num)),
                ("canRec", can_rec_val),
                ("dayPreBuyTomorrow", Value::Bool(false)),
                ("voucher", Value::Bool(false)),
            ]),
        )
        .with_time(now_i64);
        Ok((BagDelta::new(), Vec::new(), push, Vec::new()))
    } else {
        let mut combined = Vec::with_capacity(row.reward1.len() + row.reward2.len());
        combined.extend_from_slice(row.reward1);
        combined.extend_from_slice(row.reward2);
        let (delta, hids) = grant_triplets(profile, &combined, now_ts)?;
        st.mark_bought(id);
        st.maybe_bump_day_rec_num(false);
        st.save(profile);
        let cb = if is_tm {
            "tmDailyDiscount.normalUpdate"
        } else {
            "dailyDiscount.normalUpdate"
        };
        let push = S2cEnvelope::push(
            cb,
            Value::map([
                ("id", Value::Int(id as i32)),
                ("vipExp", u64_to_amf3(vip_exp)),
                ("currencyNum", u64_to_amf3(currency_num)),
                ("reward", triplets_to_amf3(&combined)),
                ("voucher", Value::Bool(false)),
            ]),
        )
        .with_time(now_i64);
        Ok((delta, hids, push, combined))
    }
}

// --- 2. `ActQuarter` (`[tm]activityQuarter.*`) ---

fn is_card_pass_recharged(profile: &PlayerProfile, state_key: &str) -> bool {
    let m = get_sub_obj(profile, state_key);
    obj_u64(&m, "recharge", 0) == 1
}

fn build_quarter_info_value(profile: &PlayerProfile, now_ts: u64) -> Value {
    let day_bucket = now_ts / 86_400;
    let q = get_sub_obj(profile, "act_quarter");
    let buy_time = obj_u64(&q, "buy_time", 0);
    let end_time = obj_u64(&q, "end_time", 0);
    let is_active = end_time > now_ts;
    let weekly_active = is_card_pass_recharged(profile, "act_weekly");
    let monthly_active = is_card_pass_recharged(profile, "act_monthly");

    let saved_day = obj_u64(&q, "rec_day_bucket", u64::MAX);
    let rec_map = if saved_day == day_bucket {
        obj_nested(&q, "reward_rec")
    } else {
        BTreeMap::new()
    };

    let mut status_entries = Vec::with_capacity(4);
    let mut rec_entries = Vec::new();
    for idx in 1..=4u32 {
        let k = idx.to_string();
        let unlocked = match idx {
            1 => is_active,
            2 => is_active && weekly_active && monthly_active,
            3 => is_active && monthly_active,
            _ => is_active && weekly_active,
        };
        let claimed_today = rec_map.get(&k).and_then(PlayerJson::as_bool).unwrap_or(false);
        if claimed_today {
            rec_entries.push((k.clone(), Value::Int(1)));
        }
        let st = if !unlocked {
            2
        } else if claimed_today {
            1
        } else {
            3
        };
        status_entries.push((k, Value::Int(st)));
    }
    let rec_val = if rec_entries.is_empty() {
        Value::empty_array()
    } else {
        Value::map(rec_entries)
    };

    Value::map([
        ("buyTime", u64_to_amf3(buy_time)),
        ("endTime", u64_to_amf3(end_time)),
        ("activeTime", u64_to_amf3(buy_time)),
        ("isActive", Value::Bool(is_active)),
        ("tag", Value::Int(1)),
        ("canWeekRec", Value::Bool(is_active && weekly_active)),
        ("canMonthRec", Value::Bool(is_active && monthly_active)),
        ("rewardStatus", Value::map(status_entries)),
        ("rewardRec", rec_val),
    ])
}

fn activate_quarter_pass(profile: &mut PlayerProfile, now_ts: u64) {
    let mut q = get_sub_obj(profile, "act_quarter");
    let prev_end = obj_u64(&q, "end_time", 0);
    let base = prev_end.max(now_ts);
    q.insert("buy_time".into(), PlayerJson::from(now_ts));
    q.insert("end_time".into(), PlayerJson::from(base + 30 * 86_400));
    set_sub_obj(profile, "act_quarter", q);
}

// --- 3. `ActWeekGift` (`[tm]activityWeekGift.*`) ---

fn ensure_week_gift_state(profile: &mut PlayerProfile, now_ts: u64) -> BTreeMap<String, PlayerJson> {
    let day_bucket = now_ts / 86_400;
    let week_bucket = now_ts / (7 * 86_400);
    let mut wg = get_sub_obj(profile, "act_week_gift");
    let lv = profile.level.max(1);

    if obj_u64(&wg, "day_bucket", u64::MAX) != day_bucket {
        wg.insert("day_bucket".into(), PlayerJson::from(day_bucket));
        wg.insert("day_level".into(), PlayerJson::from(lv));
        let mut dg = BTreeMap::new();
        for r in ACT_WEEK_GIFT_ROWS.iter().filter(|r| r.tabs == 1 && r.kind == 1 && r.gold_cost == 0) {
            if (r.min_lv..=r.max_lv).contains(&lv) {
                let mut cell = BTreeMap::new();
                cell.insert("received_num".into(), PlayerJson::from(0u32));
                cell.insert("total".into(), PlayerJson::from(1u32));
                dg.insert(r.id.to_string(), PlayerJson::Object(cell));
            }
        }
        wg.insert("day_gift".into(), PlayerJson::Object(dg));
    }

    if obj_u64(&wg, "week_bucket", u64::MAX) != week_bucket {
        wg.insert("week_bucket".into(), PlayerJson::from(week_bucket));
        wg.insert("week_level".into(), PlayerJson::from(lv));
        let mut wgm = BTreeMap::new();
        for r in ACT_WEEK_GIFT_ROWS.iter().filter(|r| r.tabs == 2 && r.kind == 1 && r.gold_cost == 0) {
            if (r.min_lv..=r.max_lv).contains(&lv) {
                let mut cell = BTreeMap::new();
                cell.insert("received_num".into(), PlayerJson::from(0u32));
                cell.insert("total".into(), PlayerJson::from(1u32));
                wgm.insert(r.id.to_string(), PlayerJson::Object(cell));
            }
        }
        wg.insert("week_gift".into(), PlayerJson::Object(wgm));
    }

    set_sub_obj(profile, "act_week_gift", wg.clone());
    wg
}

fn unlock_week_gift_by_rmb(profile: &mut PlayerProfile, rmb: u64, now_ts: u64) {
    let mut wg = ensure_week_gift_state(profile, now_ts);
    let day_lv = obj_u64(&wg, "day_level", u64::from(profile.level.max(1))) as u32;
    let week_lv = obj_u64(&wg, "week_level", u64::from(profile.level.max(1))) as u32;

    let mut dg = obj_nested(&wg, "day_gift");
    for r in ACT_WEEK_GIFT_ROWS.iter().filter(|r| r.tabs == 1 && r.kind == 2) {
        if (r.min_lv..=r.max_lv).contains(&day_lv) && rmb >= r.recharge_rmb {
            let k = r.id.to_string();
            let mut cell = obj_nested(&dg, &k);
            let rec = obj_u64(&cell, "received_num", 0);
            let tot = obj_u64(&cell, "total", 0);
            if tot < u64::from(r.buy_times) {
                cell.insert("received_num".into(), PlayerJson::from(rec));
                cell.insert("total".into(), PlayerJson::from(tot + 1));
                dg.insert(k, PlayerJson::Object(cell));
            }
        }
    }
    wg.insert("day_gift".into(), PlayerJson::Object(dg));

    let mut wgm = obj_nested(&wg, "week_gift");
    for r in ACT_WEEK_GIFT_ROWS.iter().filter(|r| r.tabs == 2 && r.kind == 2) {
        if (r.min_lv..=r.max_lv).contains(&week_lv) && rmb >= r.recharge_rmb {
            let k = r.id.to_string();
            let mut cell = obj_nested(&wgm, &k);
            let rec = obj_u64(&cell, "received_num", 0);
            let tot = obj_u64(&cell, "total", 0);
            if tot < u64::from(r.buy_times) {
                cell.insert("received_num".into(), PlayerJson::from(rec));
                cell.insert("total".into(), PlayerJson::from(tot + 1));
                wgm.insert(k, PlayerJson::Object(cell));
            }
        }
    }
    wg.insert("week_gift".into(), PlayerJson::Object(wgm));
    set_sub_obj(profile, "act_week_gift", wg);
}

fn build_week_gift_info_value(wg: &BTreeMap<String, PlayerJson>, fallback_lv: u32) -> Value {
    let day_lv = obj_u64(wg, "day_level", u64::from(fallback_lv)) as i32;
    let week_lv = obj_u64(wg, "week_level", u64::from(fallback_lv)) as i32;
    let dg = player_json_to_amf3(&PlayerJson::Object(obj_nested(wg, "day_gift")));
    let wgm = player_json_to_amf3(&PlayerJson::Object(obj_nested(wg, "week_gift")));
    Value::map([
        ("day_level", Value::Int(day_lv)),
        ("week_level", Value::Int(week_lv)),
        ("day_gift", dg.clone()),
        ("week_gift", wgm.clone()),
        ("1", dg),
        ("2", wgm),
    ])
}

// --- 4. `MonthCard` (`[tm]monthCard.*`) & `MonthFund` ---

fn has_active_month_card_slot(profile: &PlayerProfile, slot: u32, now_ts: u64) -> bool {
    let mc = get_sub_obj(profile, "month_cards");
    let slot_k = slot.to_string();
    if let Some(card) = mc.get(&slot_k).and_then(PlayerJson::as_object) {
        return obj_u64(card, "end_time", 0) > now_ts;
    }
    false
}

fn refresh_month_fund_recharge_state(profile: &mut PlayerProfile, gold_equiv: u64, now_ts: u64) {
    let mut mf = get_sub_obj(profile, "month_fund");
    let mut va_rt = obj_nested(&mf, "va_recharge_time");
    for (slot, req_nb) in [(1u32, 60u64), (2u32, 600u64)] {
        let k = slot.to_string();
        let cur = obj_u64(&va_rt, &k, 0);
        if cur == 3 {
            continue;
        }
        let card_bought = has_active_month_card_slot(profile, slot, now_ts);
        let rech_met = gold_equiv >= req_nb || profile.gold_recharge_accum_all >= req_nb;
        let next = if card_bought && rech_met {
            3
        } else if card_bought {
            2
        } else if rech_met {
            1
        } else {
            cur
        };
        if next > 0 {
            va_rt.insert(k, PlayerJson::from(next));
        }
    }
    mf.insert("va_recharge_time".into(), PlayerJson::Object(va_rt));
    set_sub_obj(profile, "month_fund", mf);
}

fn activate_month_card(
    profile: &mut PlayerProfile,
    slot: u32, // 1 = Month Card ("01"), 2 = Supreme Card ("02")
    actual_card_id: u32,
    days: u64,
    daily_reward: u64,
    is_tm: bool,
    now_ts: u64,
) -> S2cEnvelope {
    let mut mc = get_sub_obj(profile, "month_cards");
    let slot_k = slot.to_string();
    let prev = obj_nested(&mc, &slot_k);
    let prev_end = obj_u64(&prev, "end_time", 0);
    let receive_time = obj_u64(&prev, "receive_time", 0);
    let end_time = prev_end.max(now_ts).saturating_add(days.max(1) * 86_400);

    let mut entry = BTreeMap::new();
    entry.insert("slot".into(), PlayerJson::from(slot));
    entry.insert("card_id".into(), PlayerJson::from(actual_card_id));
    entry.insert("end_time".into(), PlayerJson::from(end_time));
    entry.insert("receive_time".into(), PlayerJson::from(receive_time));
    entry.insert("daily_reward".into(), PlayerJson::from(daily_reward));
    mc.insert(slot_k, PlayerJson::Object(entry));
    set_sub_obj(profile, "month_cards", mc);

    refresh_month_fund_recharge_state(profile, 0, now_ts);

    let cb = if is_tm {
        "tmRe.monthCard.updateCardInfo"
    } else {
        "re.monthCard.updateCardInfo"
    };
    S2cEnvelope::push(
        cb,
        Value::map([
            ("card_id", Value::Int(actual_card_id as i32)),
            ("end_time", u64_to_amf3(end_time)),
            ("receive_time", u64_to_amf3(receive_time)),
        ]),
    )
    .with_time(now_ts as i64)
}

fn build_month_card_info_value(profile: &PlayerProfile, now_ts: u64) -> Value {
    let mc = get_sub_obj(profile, "month_cards");
    let mut entries = Vec::new();
    for slot in [1u32, 2u32] {
        let slot_k = slot.to_string();
        if let Some(card) = mc.get(&slot_k).and_then(PlayerJson::as_object) {
            let end_time = obj_u64(card, "end_time", 0);
            if end_time > now_ts {
                let card_id = obj_u64(card, "card_id", u64::from(slot)) as i32;
                let receive_time = obj_u64(card, "receive_time", 0);
                entries.push((
                    card_id.to_string(),
                    Value::map([
                        ("card_id", Value::Int(card_id)),
                        ("end_time", u64_to_amf3(end_time)),
                        ("receive_time", u64_to_amf3(receive_time)),
                    ]),
                ));
            }
        }
    }
    if entries.is_empty() {
        Value::empty_array()
    } else {
        Value::map(entries)
    }
}

// --- 5. `allActivity` Sub-Tables (`loginReward`, `consume`, `recharge`, `singleRecharge`, `monthFund`, `weekFund`) ---

fn build_login_reward_value(profile: &PlayerProfile, state_key: &str) -> Value {
    let m = get_sub_obj(profile, state_key);
    let login_num = obj_u64(&m, "login_num", 7).max(1) as i32;
    let maked = m
        .get("maked")
        .and_then(PlayerJson::as_array)
        .map(|arr| {
            Value::dense_array(
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| Value::Int(n as i32)))
                    .collect(),
            )
        })
        .unwrap_or_else(Value::empty_array);
    Value::map([
        ("login_num", Value::Int(login_num)),
        ("maked", maked),
    ])
}

fn build_consume_info_value(profile: &PlayerProfile) -> Value {
    let consume_gold = profile
        .event_state
        .get("consume_gold_total")
        .and_then(PlayerJson::as_u64)
        .unwrap_or(0);
    let got = get_sub_obj(profile, "consume_got");
    let got_val = player_json_to_amf3(&PlayerJson::Object(got));
    Value::map([
        ("consume_gold", u64_to_amf3(consume_gold)),
        ("got_rewards", got_val),
    ])
}

fn build_accum_recharge_value(profile: &PlayerProfile) -> Value {
    let rmb = profile
        .event_state
        .get("act_recharge_rmb")
        .and_then(PlayerJson::as_u64)
        .unwrap_or(profile.gold_recharge_accum_all / 10);
    let got = get_sub_obj(profile, "recharge_got");
    Value::map([
        ("recharge_gold", u64_to_amf3(rmb)),
        ("got_rewards", player_json_to_amf3(&PlayerJson::Object(got))),
    ])
}

fn build_single_recharge_value(profile: &PlayerProfile) -> Value {
    let single = get_sub_obj(profile, "act_single_recharge");
    let mut entries = vec![("level".to_string(), Value::Int(profile.level.max(1) as i32))];
    for &req_rmb in &[6u64, 30, 98, 198, 328, 648] {
        let k = req_rmb.to_string();
        let sub = obj_nested(&single, &k);
        let max_num = obj_u64(&sub, "max_num", 0) as i32;
        let now_num = obj_u64(&sub, "now_num", 0) as i32;
        entries.push((
            k,
            Value::map([
                ("max_num", Value::Int(max_num)),
                ("now_num", Value::Int(now_num)),
            ]),
        ));
    }
    Value::map(entries)
}

fn build_month_fund_value(profile: &PlayerProfile) -> Value {
    let mf = get_sub_obj(profile, "month_fund");
    let va_rt = obj_nested(&mf, "va_recharge_time");
    let va_rec = obj_nested(&mf, "va_rec_info");
    let box_rw = obj_u64(&mf, "box_reward", 0) as i32;
    Value::map([
        ("va_recharge_time", player_json_to_amf3(&PlayerJson::Object(va_rt))),
        ("va_rec_info", player_json_to_amf3(&PlayerJson::Object(va_rec))),
        ("box_reward", Value::Int(box_rw)),
    ])
}

fn build_week_fund_value(profile: &PlayerProfile) -> Value {
    let wf = get_sub_obj(profile, "week_fund");
    let va_open = obj_nested(&wf, "va_open_time");
    let mut va_rew = obj_nested(&wf, "va_reward");
    va_rew.entry("1".into()).or_insert_with(PlayerJson::empty_object);
    va_rew.entry("2".into()).or_insert_with(PlayerJson::empty_object);
    let all_rw = obj_u64(&wf, "all_reward", 0) as i32;
    Value::map([
        ("va_open_time", player_json_to_amf3(&PlayerJson::Object(va_open))),
        ("va_reward", player_json_to_amf3(&PlayerJson::Object(va_rew))),
        ("all_reward", Value::Int(all_rw)),
    ])
}

// --- 6. Card Passes (`activityWeekly`, `activityMonthly`, `activityBeauty`) ---

fn build_card_pass_info(profile: &PlayerProfile, state_key: &str, is_weekly: bool) -> Value {
    let m = get_sub_obj(profile, state_key);
    let score = obj_u64(&m, "score", 0);
    let recharge = obj_u64(&m, "recharge", 0) as i32;
    let va_score = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "vaScore")));
    let va_task = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "vaTask")));
    if is_weekly {
        Value::map([
            ("score", u64_to_amf3(score)),
            ("daily_task_finish", Value::Int(5)),
            ("recharge", Value::Int(recharge)),
            ("vaScore", va_score),
            ("vaTask", va_task),
        ])
    } else {
        Value::map([
            ("score", u64_to_amf3(score)),
            (
                "va_activity_num",
                Value::dense_array(vec![Value::Int(100), Value::Int(100), Value::Int(100)]),
            ),
            ("recharge", Value::Int(recharge)),
            ("vaScore", va_score),
            ("vaTask", va_task),
        ])
    }
}

fn activate_card_pass(
    profile: &mut PlayerProfile,
    state_key: &str,
    push_cb: &str,
    vip_exp: u64,
    buy_reward: &[Triplet],
    now_ts: u64,
) -> Result<(BagDelta, Vec<u64>, S2cEnvelope), PlayerError> {
    let (delta, hids) = grant_triplets(profile, buy_reward, now_ts)?;
    let mut m = get_sub_obj(profile, state_key);
    m.insert("recharge".into(), PlayerJson::from(1u32));
    set_sub_obj(profile, state_key, m);
    let quarter_info = build_quarter_info_value(profile, now_ts);
    let push = S2cEnvelope::push(
        push_cb,
        Value::map([
            ("vipExp", u64_to_amf3(vip_exp)),
            ("reward", triplets_to_amf3(buy_reward)),
            ("voucher", Value::Bool(false)),
            ("quarterInfo", quarter_info),
        ]),
    )
    .with_time(now_ts as i64);
    Ok((delta, hids, push))
}

// --- 7. `Sign` & `SuperSign` (`[tm]sign.*` & `[tm]superSign.*`) ---

fn build_sign_info_value(profile: &PlayerProfile, now_ts: u64) -> Value {
    let day_bucket = now_ts / 86_400;
    let sign = get_sub_obj(profile, "sign");
    let sign_num = obj_u64(&sign, "sign_num", 0) as i32;
    let last_sign_time = obj_u64(&sign, "last_sign_time", 0);
    let last_vip_multi = obj_u64(&sign, "last_sign_vip_multi", 0) as i32;
    let rech_sign_num = obj_u64(&sign, "recharge_sign_num", 0) as i32;
    let last_rech_sign_time = obj_u64(&sign, "last_recharge_sign_time", 0);

    let saved_rech_day = obj_u64(&sign, "recharge_day_bucket", u64::MAX);
    let today_nb = if saved_rech_day == day_bucket {
        obj_u64(&sign, "today_recharge_nb", 0)
    } else {
        0
    };
    let effective_nb = today_nb.max(profile.gold_recharge_accum_all);
    let prev_va_rs = if saved_rech_day == day_bucket {
        obj_nested(&sign, "va_recharge_sign")
    } else {
        BTreeMap::new()
    };

    let mut va_rs_entries = Vec::new();
    for tier in [8u64, 40, 100] {
        let k = tier.to_string();
        if let Some(claimed) = prev_va_rs.get(&k).and_then(PlayerJson::as_bool) {
            va_rs_entries.push((k, Value::Bool(claimed)));
        } else if effective_nb >= tier {
            va_rs_entries.push((k, Value::Bool(false)));
        }
    }
    let va_rs_val = if va_rs_entries.is_empty() {
        Value::empty_array()
    } else {
        Value::map(va_rs_entries)
    };

    let acc_got = sign
        .get("acc_got")
        .and_then(PlayerJson::as_array)
        .map(|arr| {
            Value::dense_array(
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| Value::Int(n as i32)))
                    .collect(),
            )
        })
        .unwrap_or_else(Value::empty_array);

    let lv = profile.level.max(1) as i32;
    Value::map([
        ("uid", u64_to_amf3(profile.uid)),
        ("sign_num", Value::Int(sign_num)),
        ("acc_num", Value::Int(sign_num)),
        ("last_sign_time", u64_to_amf3(last_sign_time)),
        ("sign_time", u64_to_amf3(last_sign_time)),
        ("last_sign_vip_multi", Value::Int(last_vip_multi)),
        ("level_sign", Value::Int(lv)),
        ("level_recharge", Value::Int(lv)),
        ("recharge_sign_num", Value::Int(rech_sign_num)),
        ("last_recharge_sign_time", u64_to_amf3(last_rech_sign_time)),
        ("today_gold_all", u64_to_amf3(effective_nb)),
        ("acc_got", acc_got),
        ("va_recharge_sign", va_rs_val),
    ])
}

// --- 8. `ActiveTaskShop` & `Activity8Years` & `ActivityBattlePass` ---

fn build_active_task_shop_task_info(profile: &PlayerProfile) -> Value {
    let m = get_sub_obj(profile, "active_task_shop");
    let daily_list = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "daily_task_list")));
    let active_list = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "active_task_list")));
    Value::map([
        ("daily_execution", Value::Int(500)),
        ("active_point", Value::Int(1_000)),
        ("daily_task_list", daily_list),
        ("active_task_list", active_list),
    ])
}

fn build_active_task_shop_shop_info(profile: &PlayerProfile) -> Value {
    let m = get_sub_obj(profile, "active_task_shop");
    let cur_round = obj_u64(&m, "cur_round", 1).max(1) as i32;
    let ex_rounds = obj_nested(&m, "exchange_shop");
    let mut round_entries = Vec::new();
    for r in 1..=2u32 {
        let rk = r.to_string();
        let r_obj = obj_nested(&ex_rounds, &rk);
        let use_coin = obj_u64(&r_obj, "use_coin", 0) as i32;
        let goods = player_json_to_amf3(&PlayerJson::Object(obj_nested(&r_obj, "goods_list")));
        round_entries.push((
            rk,
            Value::map([
                ("use_coin", Value::Int(use_coin)),
                ("goods_list", goods),
            ]),
        ));
    }
    let active_shop = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "active_shop")));
    Value::map([
        ("cur_round", Value::Int(cur_round)),
        ("exchange_shop", Value::map(round_entries)),
        ("active_shop", active_shop),
    ])
}

fn build_eight_years_info(profile: &PlayerProfile, now_ts: u64) -> Value {
    let day_bucket = now_ts / 86_400;
    let m = get_sub_obj(profile, "eight_years");
    let mut sign_info_map = obj_nested(&m, "sign_info");
    for day in 1..=7u32 {
        sign_info_map.entry(day.to_string()).or_insert_with(|| {
            let mut row = BTreeMap::new();
            row.insert("sign".into(), PlayerJson::from(false));
            row.insert("rare".into(), PlayerJson::from(false));
            PlayerJson::Object(row)
        });
    }
    let sign_info = player_json_to_amf3(&PlayerJson::Object(sign_info_map));
    let edict_score = obj_u64(&m, "edict_score", 200) as i32;
    let mut edict_info_map = obj_nested(&m, "edict_info");
    for pid in 1..=15u32 {
        edict_info_map.entry(pid.to_string()).or_insert_with(|| {
            let mut row = BTreeMap::new();
            row.insert("normal".into(), PlayerJson::from(false));
            row.insert("rare".into(), PlayerJson::from(false));
            row.insert("super".into(), PlayerJson::from(false));
            PlayerJson::Object(row)
        });
    }
    let edict_info = player_json_to_amf3(&PlayerJson::Object(edict_info_map));
    let lucky_ticket = obj_u64(&m, "lucky_ticket", 5) as i32;
    let lucky_ticket_buy = obj_u64(&m, "lucky_ticket_buy", 0) as i32;
    let lucky_num = obj_u64(&m, "lucky_num", 3) as i32;
    let lucky_daily_rw = obj_bool(&m, "lucky_daily_reward", false);
    let lucky_total = obj_u64(&m, "lucky_ticket_total", 5) as i32;
    let lucky_rw_map = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "lucky_reawrd")));
    let year_score = obj_u64(&m, "year_score", 100) as i32;
    let year_ex = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "year_exchange_info")));
    let dinner_score = obj_u64(&m, "dinner_score", 50) as i32;
    let dinner_info = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "dinner_info")));
    let wheel_ticket = obj_u64(&m, "wheel_ticket", 2) as i32;
    let wheel_num = obj_u64(&m, "wheel_num", 0) as i32;
    let wheel_rw = player_json_to_amf3(&PlayerJson::Object(obj_nested(&m, "wheel_reward")));

    let saved_free_day = obj_u64(&m, "free_gift_day", u64::MAX);
    let mut gift_map = obj_nested(&m, "gift_info");
    if saved_free_day != day_bucket {
        gift_map.insert("4".into(), PlayerJson::from(0u32));
    }
    for gid in 1..=12u32 {
        gift_map
            .entry(gid.to_string())
            .or_insert_with(|| PlayerJson::from(0u32));
    }
    let gift_val = player_json_to_amf3(&PlayerJson::Object(gift_map));

    let task_claimed = obj_nested(&m, "task_claimed");
    let mut task_entries = Vec::new();
    for tid in 1..=15u32 {
        let tk = tid.to_string();
        let st = if obj_bool(&task_claimed, &tk, false) { 3 } else { 2 };
        task_entries.push((
            tk,
            Value::map([("rate", Value::Int(10)), ("status", Value::Int(st))]),
        ));
    }

    Value::map([
        ("act_day", Value::Int(7)),
        (
            "sign",
            Value::map([("act_day", Value::Int(7)), ("sign_info", sign_info)]),
        ),
        (
            "edict",
            Value::map([
                ("edict_score", Value::Int(edict_score)),
                ("edict_info", edict_info),
            ]),
        ),
        (
            "lucky",
            Value::map([
                ("lucky_draw_times", Value::Int(1)),
                ("lucky_ticket", Value::Int(lucky_ticket)),
                ("lucky_ticket_buy", Value::Int(lucky_ticket_buy)),
                ("lucky_num", Value::Int(lucky_num)),
                ("lucky_daily_reward", Value::Bool(lucky_daily_rw)),
            ]),
        ),
        (
            "lucky_reward",
            Value::map([
                ("lucky_ticket_total", Value::Int(lucky_total)),
                ("lucky_reawrd", lucky_rw_map),
            ]),
        ),
        (
            "year",
            Value::map([
                ("year_score", Value::Int(year_score)),
                ("year_exchange_info", year_ex),
            ]),
        ),
        (
            "dinner",
            Value::map([
                ("dinner_score", Value::Int(dinner_score)),
                ("world_score", Value::Int(10_000 + dinner_score)),
                ("dinner_info", dinner_info),
            ]),
        ),
        (
            "wheel",
            Value::map([
                ("wheel_ticket", Value::Int(wheel_ticket)),
                ("wheel_num", Value::Int(wheel_num)),
                ("wheel_reward", wheel_rw),
            ]),
        ),
        ("gift_info", gift_val),
        ("task_info", Value::map(task_entries)),
    ])
}

// ============================================================================
// Universal 1-Click `pay.instantBuy` (`tmPay.instantBuy`) Parser & Executor
// ============================================================================

struct InstantBuyParams {
    pay_id: u32,
    pay_price: u64,
    pay_type: String,
    buy_type: u32,
    item_id: u32,
    row_vip_exp: Option<u64>,
    row_base_reward: Option<u64>,
    row_extra_reward: Option<u64>,
    row_first_reward: Option<u64>,
    row_days: Option<u64>,
    row_daily_reward: Option<u64>,
    row_triplets: Vec<Triplet>,
}

fn parse_u64_field(v: Option<&Value>) -> Option<u64> {
    v.and_then(val_as_i64_loose)
        .filter(|&n| n >= 0)
        .map(|n| n as u64)
}

fn parse_triplets_from_value(v: Option<&Value>) -> Vec<Triplet> {
    let Some(Value::Array { dense, .. }) = v else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in dense {
        if let Value::Array { dense: tr, .. } = entry {
            if tr.len() >= 3 {
                let pt = val_as_i64_loose(&tr[0]).unwrap_or(0);
                let tid = val_as_i64_loose(&tr[1]).unwrap_or(0);
                let qty = val_as_i64_loose(&tr[2]).unwrap_or(0);
                if pt > 0 && qty > 0 && tid >= 0 {
                    out.push((pt as u32, tid as u32, qty as u64));
                }
            }
        }
    }
    out
}

fn parse_instant_buy_params(req: &RpcRequest) -> InstantBuyParams {
    let map_opt = req.args.first().or_else(|| req.arg_zero()).or(req.raw_args.as_ref());
    let get_f = |k: &str| map_opt.and_then(|m| m.get_field(k));

    let mut pay_id = parse_u64_field(get_f("pay_id")).unwrap_or(0) as u32;
    let pay_price = parse_u64_field(get_f("pay_price"))
        .or_else(|| parse_u64_field(get_f("consume_money")))
        .unwrap_or(0);
    let pay_type = get_f("pay_type")
        .or_else(|| get_f("price_type"))
        .and_then(|v| {
            v.as_str()
                .map(ToOwned::to_owned)
                .or_else(|| v.as_i64().map(|n| format!("{n:02}")))
        })
        .unwrap_or_else(|| "00".to_string());

    let mut buy_type = parse_u64_field(get_f("buy_type")).unwrap_or(0) as u32;
    let mut item_id = parse_u64_field(get_f("item_id")).unwrap_or(0) as u32;

    if let Some(extra) = get_f("extra_data") {
        match extra {
            Value::Array { dense, .. } => {
                if pay_type == "05" {
                    if item_id == 0 {
                        item_id = dense
                            .first()
                            .and_then(val_as_i64_loose)
                            .unwrap_or(0)
                            .max(0) as u32;
                    }
                    if pay_id == 0 {
                        pay_id = dense
                            .get(1)
                            .and_then(val_as_i64_loose)
                            .unwrap_or(0)
                            .max(0) as u32;
                    }
                } else {
                    if buy_type == 0 {
                        buy_type = dense
                            .first()
                            .and_then(val_as_i64_loose)
                            .unwrap_or(0)
                            .max(0) as u32;
                    }
                    if item_id == 0 {
                        item_id = dense
                            .get(1)
                            .and_then(val_as_i64_loose)
                            .unwrap_or(0)
                            .max(0) as u32;
                    }
                    if pay_id == 0 {
                        pay_id = dense
                            .get(2)
                            .and_then(val_as_i64_loose)
                            .unwrap_or(0)
                            .max(0) as u32;
                    }
                }
            }
            _ => {
                if item_id == 0 {
                    item_id = val_as_i64_loose(extra).unwrap_or(0).max(0) as u32;
                }
            }
        }
    }

    let pay_row = get_f("pay_row");
    let row_f = |k: &str| pay_row.and_then(|r| r.get_field(k));
    if pay_id == 0 {
        pay_id = parse_u64_field(row_f("id")).unwrap_or(0) as u32;
    }

    let row_vip_exp = parse_u64_field(row_f("vip_exp"))
        .or_else(|| parse_u64_field(row_f("goldprice")))
        .or_else(|| parse_u64_field(row_f("charge_num")));
    let row_base_reward = parse_u64_field(row_f("base_reward")).or_else(|| parse_u64_field(row_f("gold")));
    let row_extra_reward = parse_u64_field(row_f("extra_reward"));
    let row_first_reward = parse_u64_field(row_f("first_reward"));
    let row_days = parse_u64_field(row_f("days"));
    let row_daily_reward = parse_u64_field(row_f("daily_reward"));

    let mut row_triplets = Vec::new();
    for key in ["reward", "rewards", "buy_reward", "charge_reward_content", "item_id_num"] {
        let parsed = parse_triplets_from_value(row_f(key).or_else(|| get_f(key)));
        if !parsed.is_empty() {
            row_triplets = parsed;
            break;
        }
    }

    InstantBuyParams {
        pay_id,
        pay_price,
        pay_type,
        buy_type,
        item_id,
        row_vip_exp,
        row_base_reward,
        row_extra_reward,
        row_first_reward,
        row_days,
        row_daily_reward,
        row_triplets,
    }
}

fn vnd_or_rmb_to_nb(price: u64) -> u64 {
    match price {
        0 => 60,
        5_000 => 10,
        9_000 => 20,
        19_000..=29_000 => 60,
        45_000..=55_000 => 120,
        90_000..=110_000 => 240,
        190_000..=210_000 => 480,
        240_000..=260_000 => 600,
        340_000..=360_000 => 880,
        490_000..=520_000 => 1_280,
        590_000..=620_000 => 1_500,
        790_000..=820_000 => 2_000,
        990_000..=1_050_000 => 2_500,
        1_490_000..=1_550_000 => 3_880,
        1_990_000..=2_400_000 => 5_800,
        p if p >= 1_000 => ((p * 25) / 10_000).max(10),
        rmb => (rmb * 10).max(10),
    }
}

fn resolve_instant_buy_nb(params: &InstantBuyParams, is_tm: bool) -> u64 {
    if params.pay_type == "05" {
        if let Some(&(_, coins, _, _)) = DIANQUAN_ROWS.iter().find(|&&(id, _, and_id, ios_id)| {
            id == params.item_id || and_id == params.pay_id || ios_id == params.pay_id
        }) {
            return coins.max(10);
        }
    }
    if params.pay_type == "04" {
        match params.buy_type {
            4 => {
                if let Some(r) = ACT_DAILY_ROWS.iter().find(|r| r.id == params.item_id) {
                    if r.goldprice > 0 {
                        return r.goldprice;
                    }
                }
            }
            8 => return 5_800,
            10 if !is_tm => {
                if let Some(&(_, gp, _)) = RECHARGE_CURRENT_ROWS.iter().find(|&&(id, _, _)| id == params.item_id) {
                    return gp;
                }
            }
            10 | 27 => {
                if let Some(&(_, gp, _)) = TM_RECHARGE_CURRENT_ROWS.iter().find(|&&(id, _, _)| id == params.item_id) {
                    return gp;
                }
            }
            12 => {
                if let Some(r) = DAILY_RECHARGE2_ROWS.iter().find(|r| r.id == params.item_id) {
                    return r.goldprice;
                }
            }
            14 => {
                if let Some(&(_, _, nb, _, _)) = ACTIVE_TASK_SHOP_GIFTS.iter().find(|&&(id, _, _, _, _)| id == params.item_id) {
                    if nb > 0 {
                        return nb;
                    }
                }
            }
            21 => {
                if let Some(&(_, nb, _)) = EIGHT_YEARS_CHARGES.iter().find(|&&(id, _, _)| id == params.item_id) {
                    if nb > 0 {
                        return nb;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(ve) = params.row_vip_exp.filter(|&v| v > 0) {
        return ve;
    }
    if let Some(br) = params.row_base_reward.filter(|&v| v > 0) {
        return br;
    }
    vnd_or_rmb_to_nb(params.pay_price).max(10)
}

// ============================================================================
// Main Dispatcher Invoked by `player_rpcs::try_handle_player_rpc`
// ============================================================================

#[allow(clippy::too_many_arguments)]
pub fn try_handle_event_or_pay_rpc(
    store: &mut PlayerStore,
    gameplay: &mut GameplayEngine,
    data_store: &GameDataStore,
    uid: u64,
    req: &RpcRequest,
    m: &str,
    is_tm: bool,
    now_ts: u64,
) -> Option<Result<PlayerRpcSuccess, String>> {
    let now_i64 = now_ts as i64;

    match m {
        // ====================================================================
        // Active Official Event Lineup (`[tm]activity.getActivityConf` & `[tm]allActivity.getActivityInfo`)
        // ====================================================================
        "activity.getActivityConf" => {
            let ret = dto_to_amf3(&build_activity_conf_dto(now_ts), false);
            Some(Ok(PlayerRpcSuccess {
                ret,
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "allActivity.getActivityInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let ret = Value::map([
                ("activity_groupon", Value::empty_array()),
                ("activity_login_reward", build_login_reward_value(profile, "login_reward")),
                ("activity_recharge", build_accum_recharge_value(profile)),
                ("activity_recharge_reward", build_single_recharge_value(profile)),
                ("activity_consume", build_consume_info_value(profile)),
                ("activity_month_fund", build_month_fund_value(profile)),
                ("activity_week_fund", build_week_fund_value(profile)),
            ]);
            Some(Ok(PlayerRpcSuccess {
                ret,
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // Universal 1-Click `[tm]pay.instantBuy`
        // ====================================================================
        "pay.instantBuy" => {
            let params = parse_instant_buy_params(req);
            let gold_equiv = resolve_instant_buy_nb(&params, is_tm).max(1);
            let mut delta = BagDelta::new();
            let mut summoned_hids = Vec::new();
            let mut module_pushes = Vec::new();
            let mut show_gain_popup = false;
            let mut granted_triplets: Vec<Triplet> = Vec::new();

            let res = store.mutate_player(uid, now_ts, |p| {
                record_full_recharge_with_festivals(p, gold_equiv, now_ts);

                match params.pay_type.as_str() {
                    "01" | "02" => {
                        let slot = if params.pay_type == "02" { 2u32 } else { 1u32 };
                        let actual_card_id = if params.pay_id > 0 { params.pay_id } else { slot };
                        let days = params.row_days.unwrap_or(if slot == 2 { 3_650 } else { 30 });
                        let daily_rw = params.row_daily_reward.unwrap_or(if slot == 2 { 200 } else { 150 });
                        let base_nb = params.row_base_reward.unwrap_or(gold_equiv);
                        if base_nb > 0 {
                            p.grant_currency(CurrencyType::Gold, base_nb, now_ts);
                            granted_triplets.push((2, 0, base_nb));
                        }
                        let card_push = activate_month_card(
                            p,
                            slot,
                            actual_card_id,
                            days,
                            daily_rw,
                            is_tm,
                            now_ts,
                        );
                        module_pushes.push(card_push);
                    }
                    "05" => {
                        let (dq_id, coins) = DIANQUAN_ROWS
                            .iter()
                            .find(|&&(id, _, and_id, ios_id)| {
                                id == params.item_id || and_id == params.pay_id || ios_id == params.pay_id
                            })
                            .map(|&(id, c, _, _)| (id, c))
                            .unwrap_or((params.item_id.max(1), gold_equiv));
                        let ctype = if is_tm {
                            CurrencyType::TimeMachineVoucherCoin
                        } else {
                            CurrencyType::VoucherCoin
                        };
                        p.grant_currency(ctype, coins, now_ts);
                        granted_triplets.push((if is_tm { 99 } else { 87 }, 0, coins));
                        module_pushes.push(
                            S2cEnvelope::push(
                                "voucher.recharge",
                                Value::map([("id", Value::Int(dq_id as i32))]),
                            )
                            .with_time(now_i64),
                        );
                    }
                    "04" => {
                        let target_id = params.item_id.max(1);
                        match params.buy_type {
                            4 => {
                                let (d, hids, push, trs) = fulfill_act_daily_purchase(
                                    p,
                                    target_id,
                                    gold_equiv,
                                    is_tm,
                                    now_ts,
                                )?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                module_pushes.push(push);
                                granted_triplets.extend(trs);
                            }
                            12 => {
                                let row = DAILY_RECHARGE2_ROWS
                                    .iter()
                                    .find(|r| r.id == target_id)
                                    .unwrap_or(&DAILY_RECHARGE2_ROWS[0]);
                                let (d, hids) = grant_triplets(p, row.reward, now_ts)?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                granted_triplets.extend_from_slice(row.reward);

                                let day_bucket = now_ts / 86_400;
                                let mut m2 = get_sub_obj(p, "daily_recharge2");
                                if obj_u64(&m2, "day_bucket", u64::MAX) != day_bucket {
                                    m2.clear();
                                    m2.insert("day_bucket".into(), PlayerJson::from(day_bucket));
                                }
                                let mut bi = obj_nested(&m2, "buy_info");
                                let k = row.id.to_string();
                                let cur_n = obj_u64(&bi, &k, 0);
                                bi.insert(k, PlayerJson::from(cur_n + 1));
                                m2.insert("buy_info".into(), PlayerJson::Object(bi));
                                set_sub_obj(p, "daily_recharge2", m2);

                                let cb = if is_tm {
                                    "tmActivityDailyRecharge2.update"
                                } else {
                                    "activityDailyRecharge2.update"
                                };
                                module_pushes.push(
                                    S2cEnvelope::push(
                                        cb,
                                        Value::map([
                                            ("id", Value::Int(row.id as i32)),
                                            ("reward", triplets_to_amf3(row.reward)),
                                            ("vipExp", u64_to_amf3(gold_equiv)),
                                            ("voucher", Value::Bool(false)),
                                            ("voucherNum", Value::Int(0)),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            8 => {
                                activate_quarter_pass(p, now_ts);
                                let info = build_quarter_info_value(p, now_ts);
                                let cb = if is_tm {
                                    "tmActivityQuarter.update"
                                } else {
                                    "activityQuarter.update"
                                };
                                module_pushes.push(
                                    S2cEnvelope::push(
                                        cb,
                                        Value::map([
                                            ("vipExp", u64_to_amf3(gold_equiv)),
                                            ("voucher", Value::Bool(false)),
                                            ("info", info),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            1 | 2 | 15 => {
                                let (state_k, cb, rw): (&str, &str, &[Triplet]) = match params.buy_type {
                                    1 => (
                                        "act_weekly",
                                        if is_tm { "tmActivityWeekly.update" } else { "activityWeekly.update" },
                                        &[(2, 0, 300), (4, 22_102, 5)],
                                    ),
                                    2 => (
                                        "act_monthly",
                                        if is_tm { "tmActivityMonthly.update" } else { "activityMonthly.update" },
                                        &[(2, 0, 600), (4, 16_012, 5)],
                                    ),
                                    _ => (
                                        "act_beauty",
                                        if is_tm { "tmActivityBeauty.update" } else { "activityBeauty.update" },
                                        &[(2, 0, 600), (4, 22_102, 5)],
                                    ),
                                };
                                let (d, hids, push) = activate_card_pass(
                                    p, state_k, cb, gold_equiv, rw, now_ts,
                                )?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                module_pushes.push(push);
                                granted_triplets.extend_from_slice(rw);
                            }
                            10 | 27 => {
                                let use_tm_rows = is_tm || params.buy_type == 27;
                                let rows = if use_tm_rows {
                                    TM_RECHARGE_CURRENT_ROWS
                                } else {
                                    RECHARGE_CURRENT_ROWS
                                };
                                let &(prod_id, _, rw) = rows
                                    .iter()
                                    .find(|&&(id, _, _)| id == target_id)
                                    .unwrap_or(&rows[0]);
                                let (d, hids) = grant_triplets(p, rw, now_ts)?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                granted_triplets.extend_from_slice(rw);

                                let mut sm = get_sub_obj(p, "super_map_direct");
                                let k = prod_id.to_string();
                                let cur_n = obj_u64(&sm, &k, 0);
                                sm.insert(k, PlayerJson::from(cur_n + 1));
                                set_sub_obj(p, "super_map_direct", sm);

                                let cb = if is_tm {
                                    "re.tmCrossGuildWar.buyUpdate"
                                } else {
                                    "re.crossGuildWar.buyUpdate"
                                };
                                module_pushes.push(
                                    S2cEnvelope::push(
                                        cb,
                                        Value::map([
                                            ("product_id", Value::Int(prod_id as i32)),
                                            ("buy_num", Value::Int(1)),
                                            ("vip_exp", u64_to_amf3(gold_equiv)),
                                            ("voucher_num", Value::Int(0)),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            14 => {
                                let &(gid, _, _, _, rw) = ACTIVE_TASK_SHOP_GIFTS
                                    .iter()
                                    .find(|&&(id, _, _, _, _)| id == target_id)
                                    .unwrap_or(&ACTIVE_TASK_SHOP_GIFTS[1]);
                                let (d, hids) = grant_triplets(p, rw, now_ts)?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                granted_triplets.extend_from_slice(rw);

                                let mut ats = get_sub_obj(p, "active_task_shop");
                                let mut act_shop = obj_nested(&ats, "active_shop");
                                let k = gid.to_string();
                                let cur_n = obj_u64(&act_shop, &k, 0);
                                act_shop.insert(k, PlayerJson::from(cur_n + 1));
                                ats.insert("active_shop".into(), PlayerJson::Object(act_shop));
                                set_sub_obj(p, "active_task_shop", ats);

                                let cb = if is_tm {
                                    "tmActivityActiveTaskShop.pushNormalPay"
                                } else {
                                    "activityActiveTaskShop.pushNormalPay"
                                };
                                module_pushes.push(
                                    S2cEnvelope::push(
                                        cb,
                                        Value::map([
                                            ("id", Value::Int(gid as i32)),
                                            ("vipExp", u64_to_amf3(gold_equiv)),
                                            ("voucher", Value::Int(0)),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            21 => {
                                let &(cid, _, rw) = EIGHT_YEARS_CHARGES
                                    .iter()
                                    .find(|&&(id, _, _)| id == target_id)
                                    .unwrap_or(&EIGHT_YEARS_CHARGES[4]);
                                let (d, hids) = grant_triplets(p, rw, now_ts)?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                granted_triplets.extend_from_slice(rw);

                                let mut ey = get_sub_obj(p, "eight_years");
                                let mut gi = obj_nested(&ey, "gift_info");
                                let k = cid.to_string();
                                let cur_n = obj_u64(&gi, &k, 0);
                                gi.insert(k, PlayerJson::from(cur_n + 1));
                                ey.insert("gift_info".into(), PlayerJson::Object(gi));
                                if cid == 1 {
                                    ey.insert("high_sign_unlocked".into(), PlayerJson::Bool(true));
                                } else if cid == 2 {
                                    ey.insert("high_edict_unlocked".into(), PlayerJson::Bool(true));
                                } else if cid == 3 {
                                    ey.insert("super_edict_unlocked".into(), PlayerJson::Bool(true));
                                }
                                set_sub_obj(p, "eight_years", ey);

                                let cb = if is_tm {
                                    "tmActivity8Years.pushNormalPay"
                                } else {
                                    "activity8Years.pushNormalPay"
                                };
                                module_pushes.push(
                                    S2cEnvelope::push(
                                        cb,
                                        Value::map([
                                            ("id", Value::Int(cid as i32)),
                                            ("vipExp", u64_to_amf3(gold_equiv)),
                                            ("voucherNum", Value::Int(0)),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            29 => {
                                let pass_id = target_id.max(1);
                                let rw: Vec<Triplet> = if !params.row_triplets.is_empty() {
                                    params.row_triplets.clone()
                                } else {
                                    vec![(2, 0, gold_equiv), (4, 16_012, 5)]
                                };
                                let (d, hids) = grant_triplets(p, &rw, now_ts)?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                granted_triplets.extend(rw);

                                let mut bp = get_sub_obj(p, "battle_pass");
                                bp.insert("pass_id".into(), PlayerJson::from(pass_id));
                                let prev_sc = obj_u64(&bp, "pass_score", 500);
                                bp.insert("pass_score".into(), PlayerJson::from(prev_sc.max(500)));
                                set_sub_obj(p, "battle_pass", bp);

                                module_pushes.push(
                                    S2cEnvelope::push(
                                        "activityBattlePass.pushNormalPay",
                                        Value::map([
                                            ("id", Value::Int(pass_id as i32)),
                                            ("vipExp", u64_to_amf3(gold_equiv)),
                                            ("voucherNum", Value::Int(0)),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            6 => {
                                let fund_type = target_id.clamp(1, 4);
                                let stage = 1u32;
                                let mut mf = get_sub_obj(p, "multiple_funds");
                                mf.insert(format!("{fund_type}_{stage}_recharged"), PlayerJson::from(1u32));
                                set_sub_obj(p, "multiple_funds", mf);
                                p.grant_currency(CurrencyType::Gold, gold_equiv, now_ts);
                                granted_triplets.push((2, 0, gold_equiv));

                                let cb = if is_tm {
                                    "tmMultipleFunds.buy"
                                } else {
                                    "multipleFunds.buy"
                                };
                                module_pushes.push(
                                    S2cEnvelope::push(
                                        cb,
                                        Value::map([
                                            ("vipExp", u64_to_amf3(gold_equiv)),
                                            ("voucher", Value::Bool(false)),
                                            ("type", Value::Int(fund_type as i32)),
                                            ("stage", Value::Int(stage as i32)),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            23 => {
                                let rw: Vec<Triplet> = if !params.row_triplets.is_empty() {
                                    params.row_triplets.clone()
                                } else {
                                    vec![(2, 0, gold_equiv), (4, 16_012, 5)]
                                };
                                let (d, hids) = grant_triplets(p, &rw, now_ts)?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                granted_triplets.extend(rw);
                                module_pushes.push(
                                    S2cEnvelope::push(
                                        "timeMachine.pushNormalPay",
                                        Value::map([
                                            ("id", Value::Int(target_id as i32)),
                                            ("vipExp", u64_to_amf3(gold_equiv)),
                                            ("voucherNum", Value::Int(0)),
                                        ]),
                                    )
                                    .with_time(now_i64),
                                );
                            }
                            _ => {
                                let rw: Vec<Triplet> = if !params.row_triplets.is_empty() {
                                    params.row_triplets.clone()
                                } else {
                                    vec![(2, 0, gold_equiv)]
                                };
                                let (d, hids) = grant_triplets(p, &rw, now_ts)?;
                                delta.merge(d);
                                summoned_hids.extend(hids);
                                granted_triplets.extend(rw);
                                show_gain_popup = true;
                            }
                        }
                    }
                    _ => {
                        // Standard `"00"` Gold Pack + First-Recharge Bonus!
                        let base_nb = params.row_base_reward.unwrap_or(gold_equiv);
                        let pay_key = params.pay_id.max(1).to_string();
                        let mut bought_ids = p
                            .va_recharge
                            .get("bought_pay_ids")
                            .and_then(PlayerJson::as_object)
                            .cloned()
                            .unwrap_or_default();
                        let is_first_for_pack = !bought_ids.contains_key(&pay_key);
                        let bonus_nb = if is_first_for_pack {
                            params.row_first_reward.unwrap_or(base_nb)
                        } else {
                            params.row_extra_reward.unwrap_or(0)
                        };
                        bought_ids.insert(pay_key, PlayerJson::from(1u32));
                        p.va_recharge
                            .insert("bought_pay_ids".into(), PlayerJson::Object(bought_ids));

                        let total_gold = base_nb.saturating_add(bonus_nb);
                        p.grant_currency(CurrencyType::Gold, total_gold, now_ts);
                        granted_triplets.push((2, 0, total_gold));
                        show_gain_popup = true;
                    }
                }
                Ok(())
            });

            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }

            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();

            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &summoned_hids, is_tm, now_i64) {
                pushes.push(hp);
            }
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            pushes.extend(module_pushes);

            let ret = Value::map([
                ("ret", Value::String("ok".into())),
                ("pay_type", Value::String(params.pay_type)),
                ("pay_id", Value::Int(params.pay_id as i32)),
                ("buy_type", Value::Int(params.buy_type as i32)),
                ("item_id", Value::Int(params.item_id as i32)),
                ("vip_exp_added", u64_to_amf3(gold_equiv)),
                ("vip", Value::Int(updated.vip as i32)),
                ("vip_exp", u64_to_amf3(updated.vip_exp)),
                (
                    "gold_recharge_accum_all",
                    u64_to_amf3(updated.gold_recharge_accum_all),
                ),
                ("show_gain_popup", Value::Bool(show_gain_popup)),
                ("reward", triplets_to_amf3(&granted_triplets)),
            ]);
            Some(Ok(PlayerRpcSuccess {
                ret,
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // `Ưu Đãi Mỗi Ngày / Tuần / Tháng / Shop / Quà` (`[tm]dailyDiscount.*`)
        // ====================================================================
        "dailyDiscount.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let st = NormalizedDailyState::load(profile, now_ts);
            Some(Ok(PlayerRpcSuccess {
                ret: st.to_get_info_value(),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "dailyDiscount.receive" => {
            let id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let Some(row) = ACT_DAILY_ROWS.iter().find(|r| r.id == id && (1..=4).contains(&r.id)) else {
                return Some(Err("invalid_daily_id".into()));
            };
            let mut delta = BagDelta::new();
            let mut hids = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut st = NormalizedDailyState::load(p, now_ts);
                if st.is_bought(id) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                if id != 1 && !st.is_prebuy_active() {
                    return Err(PlayerError::InvalidParameter("prebuy_not_active".into()));
                }
                let mut combined = Vec::with_capacity(row.reward1.len() + row.reward2.len());
                combined.extend_from_slice(row.reward1);
                combined.extend_from_slice(row.reward2);
                let (d, hs) = grant_triplets(p, &combined, now_ts)?;
                delta.merge(d);
                hids.extend(hs);
                st.mark_bought(id);
                st.maybe_bump_day_rec_num(false);
                st.save(p);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &hids, is_tm, now_i64) {
                pushes.push(hp);
            }
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

        "dailyDiscount.receiveAllReward" => {
            let mut delta = BagDelta::new();
            let mut hids = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut st = NormalizedDailyState::load(p, now_ts);
                let max_id = if st.is_prebuy_active() { 4u32 } else { 1u32 };
                let mut combined = Vec::new();
                for id in 1..=max_id {
                    if !st.is_bought(id) {
                        if let Some(row) = ACT_DAILY_ROWS.iter().find(|r| r.id == id) {
                            combined.extend_from_slice(row.reward1);
                            combined.extend_from_slice(row.reward2);
                            st.mark_bought(id);
                        }
                    }
                }
                if combined.is_empty() {
                    return Err(PlayerError::InvalidParameter("nothing_to_claim".into()));
                }
                let (d, hs) = grant_triplets(p, &combined, now_ts)?;
                delta.merge(d);
                hids.extend(hs);
                st.maybe_bump_day_rec_num(true);
                st.save(p);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &hids, is_tm, now_i64) {
                pushes.push(hp);
            }
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

        "dailyDiscount.redeem" => {
            let shop_id = arg_i64(req, 0).unwrap_or(0).max(0) as u32;
            let num = arg_i64(req, 1).unwrap_or(1).clamp(1, 99) as u32;
            let Some(&(_, out_tid, out_unit_qty, ticket_cost, degree_limit)) =
                ACT_DAILY_SHOP_ROWS.iter().find(|&&(id, _, _, _, _)| id == shop_id)
            else {
                return Some(Err("invalid_shop_id".into()));
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut st = NormalizedDailyState::load(p, now_ts);
                let k = shop_id.to_string();
                let used = st.redeem.get(&k).and_then(PlayerJson::as_u64).unwrap_or(0) as u32;
                if used + num > degree_limit {
                    return Err(PlayerError::InvalidParameter("redeem_limit_exceeded".into()));
                }
                let (_, d_sub) = p.remove_item_by_template_with_delta(14_374, ticket_cost * num)?;
                delta.merge(d_sub);
                let (d_add, _) = grant_triplets(
                    p,
                    &[(4, out_tid, u64::from(out_unit_qty) * u64::from(num))],
                    now_ts,
                )?;
                delta.merge(d_add);
                st.redeem.insert(k, PlayerJson::from(used + num));
                st.save(p);
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

        "dailyDiscount.recRechargeReward" => {
            let mid = arg_i64(req, 0).unwrap_or(0).max(0) as u32;
            let Some(&(_, req_pt, reward_tid)) =
                ACT_DAILY_POINT_ROWS.iter().find(|&&(id, _, _)| id == mid)
            else {
                return Some(Err("invalid_point_id".into()));
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut st = NormalizedDailyState::load(p, now_ts);
                let k = mid.to_string();
                if st.point_claimed.get(&k).and_then(PlayerJson::as_u64).unwrap_or(0) == 3 {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                if st.recharge_sum < req_pt {
                    return Err(PlayerError::InvalidParameter("recharge_points_not_enough".into()));
                }
                let (d_add, _) = grant_triplets(p, &[(4, reward_tid, 1)], now_ts)?;
                delta.merge(d_add);
                st.point_claimed.insert(k, PlayerJson::from(3u32));
                st.save(p);
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

        "dailyDiscount.useVoucher" => {
            let id = arg_i64(req, 0).unwrap_or(2).clamp(2, 12) as u32;
            let vip_exp = ACT_DAILY_ROWS
                .iter()
                .find(|r| r.id == id)
                .map_or(60, |r| r.goldprice.max(10));
            let mut delta = BagDelta::new();
            let mut hids = Vec::new();
            let mut push_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902, 14_375], now_ts));
                record_full_recharge_with_festivals(p, vip_exp, now_ts);
                let (d, hs, push, _) = fulfill_act_daily_purchase(p, id, vip_exp, is_tm, now_ts)?;
                delta.merge(d);
                hids.extend(hs);
                push_opt = Some(push);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &hids, is_tm, now_i64) {
                pushes.push(hp);
            }
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            if let Some(mp) = push_opt {
                pushes.push(mp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // `Quà Ngày` (`[tm]activityDailyRecharge2.*`)
        // ====================================================================
        "activityDailyRecharge2.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let day_bucket = now_ts / 86_400;
            let m2 = get_sub_obj(profile, "daily_recharge2");
            let buy_info = if obj_u64(&m2, "day_bucket", u64::MAX) == day_bucket {
                player_json_to_amf3(&PlayerJson::Object(obj_nested(&m2, "buy_info")))
            } else {
                Value::empty_array()
            };
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([("buyInfo", buy_info)]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityDailyRecharge2.useVoucher" => {
            let id = arg_i64(req, 0).unwrap_or(1).clamp(1, 5) as u32;
            let row = DAILY_RECHARGE2_ROWS
                .iter()
                .find(|r| r.id == id)
                .unwrap_or(&DAILY_RECHARGE2_ROWS[0]);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let day_bucket = now_ts / 86_400;
                let mut m2 = get_sub_obj(p, "daily_recharge2");
                if obj_u64(&m2, "day_bucket", u64::MAX) != day_bucket {
                    m2.clear();
                    m2.insert("day_bucket".into(), PlayerJson::from(day_bucket));
                }
                let mut bi = obj_nested(&m2, "buy_info");
                let k = row.id.to_string();
                let cur_n = obj_u64(&bi, &k, 0) as u32;
                if cur_n >= row.limit {
                    return Err(PlayerError::InvalidParameter("daily_limit_reached".into()));
                }
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, row.goldprice, now_ts);
                let (d_add, _) = grant_triplets(p, row.reward, now_ts)?;
                delta.merge(d_add);
                bi.insert(k, PlayerJson::from(cur_n + 1));
                m2.insert("buy_info".into(), PlayerJson::Object(bi));
                set_sub_obj(p, "daily_recharge2", m2);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            let cb = if is_tm {
                "tmActivityDailyRecharge2.update"
            } else {
                "activityDailyRecharge2.update"
            };
            pushes.push(
                S2cEnvelope::push(
                    cb,
                    Value::map([
                        ("id", Value::Int(row.id as i32)),
                        ("reward", triplets_to_amf3(row.reward)),
                        ("vipExp", u64_to_amf3(row.goldprice)),
                        ("voucher", Value::Bool(false)),
                        ("voucherNum", Value::Int(0)),
                    ]),
                )
                .with_time(now_i64),
            );
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // `Quà Siêu Ưu Đãi` (`[tm]activityWeekGift.*`)
        // ====================================================================
        "activityWeekGift.getInfo" => {
            let mut wg_opt = None;
            let mut lv = 1u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                lv = p.level.max(1);
                wg_opt = Some(ensure_week_gift_state(p, now_ts));
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            let wg = wg_opt.expect("set above");
            Some(Ok(PlayerRpcSuccess {
                ret: build_week_gift_info_value(&wg, lv),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityWeekGift.buyGift" | "activityWeekGift.receiveReward" => {
            let id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let Some(row) = ACT_WEEK_GIFT_ROWS.iter().find(|r| r.id == id) else {
                return Some(Err("invalid_week_gift_id".into()));
            };
            let is_buy_call = m == "activityWeekGift.buyGift";
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut wg = ensure_week_gift_state(p, now_ts);
                let tab_key = if row.tabs == 1 { "day_gift" } else { "week_gift" };
                let mut tab_map = obj_nested(&wg, tab_key);
                let k = row.id.to_string();
                let mut cell = obj_nested(&tab_map, &k);
                let rec = obj_u64(&cell, "received_num", 0) as u32;
                let mut tot = obj_u64(&cell, "total", 0) as u32;

                if row.kind == 1 {
                    if rec >= row.buy_times {
                        return Err(PlayerError::InvalidParameter("already_bought".into()));
                    }
                    if row.gold_cost > 0 {
                        p.consume_currency(CurrencyType::Gold, row.gold_cost, now_ts)?;
                    }
                    let (d, _) = grant_triplets(p, row.reward, now_ts)?;
                    delta.merge(d);
                    cell.insert("received_num".into(), PlayerJson::from(rec + 1));
                    cell.insert("total".into(), PlayerJson::from((tot + 1).max(rec + 1)));
                } else {
                    if is_buy_call {
                        return Err(PlayerError::InvalidParameter("recharge_required".into()));
                    }
                    if tot == 0 && (p.gold_recharge_accum_all / 10) >= row.recharge_rmb {
                        tot = 1;
                    }
                    if rec >= tot || rec >= row.buy_times {
                        return Err(PlayerError::InvalidParameter("not_unlocked_or_claimed".into()));
                    }
                    let (d, _) = grant_triplets(p, row.reward, now_ts)?;
                    delta.merge(d);
                    cell.insert("received_num".into(), PlayerJson::from(rec + 1));
                    cell.insert("total".into(), PlayerJson::from(tot));
                }
                tab_map.insert(k, PlayerJson::Object(cell));
                wg.insert(tab_key.into(), PlayerJson::Object(tab_map));
                set_sub_obj(p, "act_week_gift", wg);
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
        // `Tam Quốc Chí Tôn Lệnh` (`[tm]activityQuarter.*`)
        // ====================================================================
        "activityQuarter.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_quarter_info_value(profile, now_ts),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityQuarter.recReward" => {
            let index = arg_i64(req, 0).unwrap_or(1);
            if !(1..=4).contains(&index) {
                return Some(Err("invalid_quarter_index".into()));
            }
            let idx_u32 = index as u32;
            let mut delta = BagDelta::new();
            let mut info_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                let day_bucket = now_ts / 86_400;
                let mut q = get_sub_obj(p, "act_quarter");
                let is_active = obj_u64(&q, "end_time", 0) > now_ts;
                let weekly_active = is_card_pass_recharged(p, "act_weekly");
                let monthly_active = is_card_pass_recharged(p, "act_monthly");
                let unlocked = match idx_u32 {
                    1 => is_active,
                    2 => is_active && weekly_active && monthly_active,
                    3 => is_active && monthly_active,
                    _ => is_active && weekly_active,
                };
                if !unlocked {
                    return Err(PlayerError::InvalidParameter("quarter_slot_locked".into()));
                }
                let mut rec_map = if obj_u64(&q, "rec_day_bucket", u64::MAX) == day_bucket {
                    obj_nested(&q, "reward_rec")
                } else {
                    BTreeMap::new()
                };
                let k = idx_u32.to_string();
                if rec_map.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                let (d, _) = grant_triplets(p, QUARTER_REWARDS[(idx_u32 - 1) as usize], now_ts)?;
                delta.merge(d);
                rec_map.insert(k, PlayerJson::Bool(true));
                q.insert("rec_day_bucket".into(), PlayerJson::from(day_bucket));
                q.insert("reward_rec".into(), PlayerJson::Object(rec_map));
                set_sub_obj(p, "act_quarter", q);
                info_opt = Some(build_quarter_info_value(p, now_ts));
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
                ret: info_opt.expect("set above"),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityQuarter.useVoucher" => {
            let mut delta = BagDelta::new();
            let mut info_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, 5_800, now_ts);
                activate_quarter_pass(p, now_ts);
                info_opt = Some(build_quarter_info_value(p, now_ts));
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let info = info_opt.expect("set above");
            let cb = if is_tm {
                "tmActivityQuarter.update"
            } else {
                "activityQuarter.update"
            };
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            pushes.push(
                S2cEnvelope::push(
                    cb,
                    Value::map([
                        ("vipExp", Value::Int(5_800)),
                        ("voucher", Value::Bool(false)),
                        ("info", info),
                    ]),
                )
                .with_time(now_i64),
            );
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // `Nạp lần đầu` (`[tm]firstGift.*`)
        // ====================================================================
        "firstGift.getRechargeGold" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: u64_to_amf3(profile.gold_recharge_accum_all),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "firstGift.receiveReward" => {
            let tier = arg_i64(req, 0).unwrap_or(1);
            let Some(&(_, req_nb, rw)) = FIRST_GIFT_TIERS.iter().find(|&&(t, _, _)| i64::from(t) == tier) else {
                return Some(Err("invalid_first_gift_tier".into()));
            };
            let mut delta = BagDelta::new();
            let mut hids = Vec::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                if p.gold_recharge_accum_all < req_nb {
                    return Err(PlayerError::InvalidParameter("recharge_not_enough".into()));
                }
                let mut fr = p
                    .va_recharge
                    .get("first_recharge")
                    .and_then(PlayerJson::as_object)
                    .cloned()
                    .unwrap_or_default();
                let k = tier.to_string();
                if fr.get(&k).and_then(PlayerJson::as_u64).unwrap_or(0) > 0 {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                let (d, hs) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
                hids.extend(hs);
                fr.insert(k, PlayerJson::from(1u32));
                p.va_recharge
                    .insert("first_recharge".into(), PlayerJson::Object(fr));
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let mut pushes = Vec::new();
            if let Some(hp) = build_new_heroes_env(updated, &hids, is_tm, now_i64) {
                pushes.push(hp);
            }
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
        // `Báo Danh` & `Báo Danh VIP` (`[tm]sign.*` & `[tm]superSign.*`)
        // ====================================================================
        "sign.getInfo" | "sign.getSignInfo" | "sign.getNormalInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_sign_info_value(profile, now_ts),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "sign.signToday" | "sign.gainNormalSignReward" => {
            let mut delta = BagDelta::new();
            let mut day_num = 1u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                let day_bucket = now_ts / 86_400;
                let mut sign = get_sub_obj(p, "sign");
                let last_ts = obj_u64(&sign, "last_sign_time", 0);
                if last_ts > 0 && (last_ts / 86_400) == day_bucket {
                    return Err(PlayerError::InvalidParameter("already_signed_today".into()));
                }
                day_num = obj_u64(&sign, "sign_num", 0) + 1;
                sign.insert("sign_num".into(), PlayerJson::from(day_num));
                sign.insert("last_sign_time".into(), PlayerJson::from(now_ts));
                sign.insert("last_sign_vip_multi".into(), PlayerJson::from(0u32));
                set_sub_obj(p, "sign", sign);
                let (d, _) = grant_triplets(p, &[(2, 0, 100), (4, 22_102, 1)], now_ts)?;
                delta.merge(d);
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
                    ("sign_num", u64_to_amf3(day_num)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "sign.gainVipSignReward" => {
            let req_vip = arg_i64(req, 0).unwrap_or(1).max(0) as u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                if p.vip < req_vip {
                    return Err(PlayerError::InvalidParameter("vip_too_low".into()));
                }
                let mut sign = get_sub_obj(p, "sign");
                if obj_u64(&sign, "last_sign_vip_multi", 0) > 0 {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                sign.insert("last_sign_vip_multi".into(), PlayerJson::from(1u32));
                set_sub_obj(p, "sign", sign);
                p.grant_currency(CurrencyType::Gold, 100, now_ts);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([("ret", Value::String("ok".into()))]),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "sign.gainAccSignReward" => {
            let req_days = arg_i64(req, 0).unwrap_or(1).max(1) as u64;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut sign = get_sub_obj(p, "sign");
                let sign_num = obj_u64(&sign, "sign_num", 1);
                if sign_num < req_days {
                    return Err(PlayerError::InvalidParameter("sign_days_not_enough".into()));
                }
                let mut acc = sign
                    .get("acc_got")
                    .and_then(PlayerJson::as_array)
                    .map(<[PlayerJson]>::to_vec)
                    .unwrap_or_default();
                if acc.iter().any(|v| v.as_u64() == Some(req_days)) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                acc.push(PlayerJson::from(req_days));
                sign.insert("acc_got".into(), PlayerJson::Array(acc));
                set_sub_obj(p, "sign", sign);
                let (d, _) = grant_triplets(p, &[(2, 0, 150), (4, 16_012, 1)], now_ts)?;
                delta.merge(d);
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
                ret: Value::map([("ret", Value::String("ok".into()))]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "sign.gainRechargeSignReward" => {
            let raw_tier = arg_i64(req, 0).unwrap_or(8);
            let req_nb: u64 = match raw_tier {
                1 | 8 => 8,
                2 | 40 => 40,
                3 | 100 => 100,
                other if other > 0 => other as u64,
                _ => return Some(Err("invalid_recharge_sign_tier".into())),
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let day_bucket = now_ts / 86_400;
                let mut sign = get_sub_obj(p, "sign");
                let saved_day = obj_u64(&sign, "recharge_day_bucket", u64::MAX);
                let today_nb = if saved_day == day_bucket {
                    obj_u64(&sign, "today_recharge_nb", 0)
                } else {
                    0
                };
                let effective_nb = today_nb.max(p.gold_recharge_accum_all);
                if effective_nb < req_nb {
                    return Err(PlayerError::InvalidParameter("recharge_not_enough".into()));
                }
                let mut va_rs = if saved_day == day_bucket {
                    obj_nested(&sign, "va_recharge_sign")
                } else {
                    BTreeMap::new()
                };
                let k = req_nb.to_string();
                if va_rs.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                va_rs.insert(k, PlayerJson::Bool(true));
                for t in [8u64, 40, 100] {
                    if effective_nb >= t {
                        va_rs.entry(t.to_string()).or_insert(PlayerJson::Bool(false));
                    }
                }
                sign.insert("recharge_day_bucket".into(), PlayerJson::from(day_bucket));
                sign.insert("today_recharge_nb".into(), PlayerJson::from(today_nb));
                sign.insert("va_recharge_sign".into(), PlayerJson::Object(va_rs));
                let prev_cnt = obj_u64(&sign, "recharge_sign_num", 0);
                sign.insert("recharge_sign_num".into(), PlayerJson::from(prev_cnt + 1));
                sign.insert("last_recharge_sign_time".into(), PlayerJson::from(now_ts));
                set_sub_obj(p, "sign", sign);

                let rw: &[Triplet] = match req_nb {
                    8 => &[(2, 0, 100), (4, 22_102, 2)],
                    40 => &[(2, 0, 200), (4, 16_012, 2)],
                    _ => &[(2, 0, 500), (4, 28_002, 1)],
                };
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
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
                    ("tier", u64_to_amf3(req_nb)),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "superSign.getInfo" | "superCheckin.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let ss = get_sub_obj(profile, "super_sign");
            let signed_days = player_json_to_amf3(&PlayerJson::Object(obj_nested(&ss, "signed")));
            let acc_claimed_map = obj_nested(&ss, "acc_claimed");
            let acc_num = acc_claimed_map.len() as i32;
            let acc_claimed = player_json_to_amf3(&PlayerJson::Object(acc_claimed_map));
            let sign_cnt = obj_u64(&ss, "sign_count", 0) as i32;
            let cur_day = (((now_ts / 86_400) % 30) + 1) as i32;
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("cur_day", Value::Int(cur_day)),
                    ("acc_num", Value::Int(acc_num)),
                    ("sign_num", Value::Int(sign_cnt)),
                    ("signed_days", signed_days.clone()),
                    ("va_sign", signed_days),
                    ("va_acc", acc_claimed),
                ]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "superSign.sign" | "superCheckin.sign" => {
            let default_day = (((now_ts / 86_400) % 30) + 1) as u32;
            let day = arg_i64(req, 0).unwrap_or(i64::from(default_day)).clamp(1, 31) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut ss = get_sub_obj(p, "super_sign");
                let mut signed = obj_nested(&ss, "signed");
                let k = day.to_string();
                if signed.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_signed".into()));
                }
                signed.insert(k, PlayerJson::Bool(true));
                let cnt = signed.len() as u64;
                ss.insert("signed".into(), PlayerJson::Object(signed));
                ss.insert("sign_count".into(), PlayerJson::from(cnt));
                set_sub_obj(p, "super_sign", ss);
                let (d, _) = grant_triplets(p, &[(2, 0, 100), (4, 22_102, 1)], now_ts)?;
                delta.merge(d);
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

        "superSign.receiveAccReward" | "superCheckin.fetchReward" => {
            let acc_id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut ss = get_sub_obj(p, "super_sign");
                let mut acc = obj_nested(&ss, "acc_claimed");
                let k = acc_id.to_string();
                if acc.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                acc.insert(k, PlayerJson::Bool(true));
                ss.insert("acc_claimed".into(), PlayerJson::Object(acc));
                set_sub_obj(p, "super_sign", ss);
                let (d, _) = grant_triplets(p, &[(2, 0, 200), (4, 16_012, 2)], now_ts)?;
                delta.merge(d);
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
        // `Thẻ Tháng / Thẻ Chí Tôn` (`[tm]monthCard.*`)
        // ====================================================================
        "monthCard.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_month_card_info_value(profile, now_ts),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "monthCard.receiveReward" => {
            let req_card_id = arg_i64(req, 0).unwrap_or(1).max(1) as u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                let day_bucket = now_ts / 86_400;
                let mut mc = get_sub_obj(p, "month_cards");
                let mut matched_slot = None;
                for slot in ["1", "2"] {
                    if let Some(card) = mc.get(slot).and_then(PlayerJson::as_object) {
                        let cid = obj_u64(card, "card_id", 0);
                        let sl = obj_u64(card, "slot", 0);
                        if cid == req_card_id || sl == req_card_id {
                            matched_slot = Some(slot.to_string());
                            break;
                        }
                    }
                }
                let Some(slot_k) = matched_slot else {
                    return Err(PlayerError::InvalidParameter("month_card_not_bought".into()));
                };
                let mut card = obj_nested(&mc, &slot_k);
                if obj_u64(&card, "end_time", 0) <= now_ts {
                    return Err(PlayerError::InvalidParameter("month_card_expired".into()));
                }
                let prev_rec = obj_u64(&card, "receive_time", 0);
                if prev_rec > 0 && (prev_rec / 86_400) == day_bucket {
                    return Err(PlayerError::InvalidParameter("already_claimed_today".into()));
                }
                let daily_rw = obj_u64(&card, "daily_reward", if slot_k == "2" { 200 } else { 150 });
                p.grant_currency(CurrencyType::Gold, daily_rw, now_ts);
                card.insert("receive_time".into(), PlayerJson::from(now_ts));
                mc.insert(slot_k, PlayerJson::Object(card));
                set_sub_obj(p, "month_cards", mc);
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

        // ====================================================================
        // Card Passes (`[tm]activityWeekly.*`, `[tm]activityMonthly.*`, `[tm]activityBeauty.*`)
        // ====================================================================
        "activityWeekly.getInfo" | "activityMonthly.getInfo" | "activityBeauty.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let (sk, is_w) = match m {
                "activityWeekly.getInfo" => ("act_weekly", true),
                "activityMonthly.getInfo" => ("act_monthly", false),
                _ => ("act_beauty", false),
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_card_pass_info(profile, sk, is_w),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityWeekly.getTodayTaskFinishNum" => Some(Ok(PlayerRpcSuccess {
            ret: Value::Int(5),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "activityMonthly.getActivePoint" | "activityBeauty.getActivePoint" => {
            Some(Ok(PlayerRpcSuccess {
                ret: Value::dense_array(vec![Value::Int(100), Value::Int(100), Value::Int(100)]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityWeekly.recTaskReward"
        | "activityMonthly.recTaskReward"
        | "activityBeauty.recTaskReward" => {
            let id = arg_i64(req, 0).unwrap_or(1).clamp(1, 3) as u32;
            let (sk, coin_pt, pill_tid) = match m {
                "activityWeekly.recTaskReward" => ("act_weekly", 73u32, 22_102u32),
                "activityMonthly.recTaskReward" => ("act_monthly", 74u32, 22_202u32),
                _ => ("act_beauty", 90u32, 16_012u32),
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut st = get_sub_obj(p, sk);
                let mut vt = obj_nested(&st, "vaTask");
                let k = id.to_string();
                if vt.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                vt.insert(k, PlayerJson::Bool(true));
                st.insert("vaTask".into(), PlayerJson::Object(vt));
                set_sub_obj(p, sk, st);
                let (d, _) = grant_triplets(p, &[(coin_pt, 0, 10), (4, pill_tid, 1)], now_ts)?;
                delta.merge(d);
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

        "activityWeekly.recScoreReward"
        | "activityMonthly.recScoreReward"
        | "activityBeauty.recScoreReward" => {
            let id = arg_i64(req, 0).unwrap_or(1).clamp(1, 3) as u32;
            let sk = match m {
                "activityWeekly.recScoreReward" => "act_weekly",
                "activityMonthly.recScoreReward" => "act_monthly",
                _ => "act_beauty",
            };
            let req_score = u64::from(id) * 10;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut st = get_sub_obj(p, sk);
                if obj_u64(&st, "recharge", 0) == 0 {
                    return Err(PlayerError::InvalidParameter("pass_not_recharged".into()));
                }
                if obj_u64(&st, "score", 0) < req_score {
                    return Err(PlayerError::InvalidParameter("score_not_enough".into()));
                }
                let mut vs = obj_nested(&st, "vaScore");
                let k = id.to_string();
                if vs.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                vs.insert(k, PlayerJson::Bool(true));
                st.insert("vaScore".into(), PlayerJson::Object(vs));
                set_sub_obj(p, sk, st);
                let rw: &[Triplet] = match id {
                    1 => &[(2, 0, 150)],
                    2 => &[(4, 16_012, 2)],
                    _ => &[(4, 28_002, 2)],
                };
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
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

        "activityWeekly.oneKeyRec" | "activityMonthly.oneKeyRec" | "activityBeauty.oneKeyRec" => {
            let (sk, coin_pt) = match m {
                "activityWeekly.oneKeyRec" => ("act_weekly", 73u32),
                "activityMonthly.oneKeyRec" => ("act_monthly", 74u32),
                _ => ("act_beauty", 90u32),
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut st = get_sub_obj(p, sk);
                let mut vt = obj_nested(&st, "vaTask");
                let mut rw = Vec::new();
                for id in 1..=3u32 {
                    let k = id.to_string();
                    if vt.get(&k).and_then(PlayerJson::as_bool) != Some(true) {
                        vt.insert(k, PlayerJson::Bool(true));
                        rw.push((coin_pt, 0, 10));
                        rw.push((4, 22_102, 1));
                    }
                }
                st.insert("vaTask".into(), PlayerJson::Object(vt));
                set_sub_obj(p, sk, st);
                if !rw.is_empty() {
                    let (d, _) = grant_triplets(p, &rw, now_ts)?;
                    delta.merge(d);
                }
                let mut st2 = get_sub_obj(p, sk);
                if obj_u64(&st2, "recharge", 0) == 1 {
                    let sc = obj_u64(&st2, "score", 0);
                    let mut vs = obj_nested(&st2, "vaScore");
                    let mut srw = Vec::new();
                    for id in 1..=3u32 {
                        let k = id.to_string();
                        if sc >= u64::from(id) * 10 && vs.get(&k).and_then(PlayerJson::as_bool) != Some(true) {
                            vs.insert(k, PlayerJson::Bool(true));
                            srw.push((2, 0, 100));
                        }
                    }
                    st2.insert("vaScore".into(), PlayerJson::Object(vs));
                    set_sub_obj(p, sk, st2);
                    if !srw.is_empty() {
                        let (d, _) = grant_triplets(p, &srw, now_ts)?;
                        delta.merge(d);
                    }
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
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityWeekly.useVoucher" | "activityMonthly.useVoucher" | "activityBeauty.useVoucher" => {
            let (sk, cb, vip_exp, rw): (&str, &str, u64, &[Triplet]) = match m {
                "activityWeekly.useVoucher" => (
                    "act_weekly",
                    if is_tm { "tmActivityWeekly.update" } else { "activityWeekly.update" },
                    240,
                    &[(2, 0, 300), (4, 22_102, 5)],
                ),
                "activityMonthly.useVoucher" => (
                    "act_monthly",
                    if is_tm { "tmActivityMonthly.update" } else { "activityMonthly.update" },
                    600,
                    &[(2, 0, 600), (4, 16_012, 5)],
                ),
                _ => (
                    "act_beauty",
                    if is_tm { "tmActivityBeauty.update" } else { "activityBeauty.update" },
                    600,
                    &[(2, 0, 600), (4, 22_102, 5)],
                ),
            };
            let mut delta = BagDelta::new();
            let mut push_opt = None;
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, vip_exp, now_ts);
                let (d, _, push) = activate_card_pass(p, sk, cb, vip_exp, rw, now_ts)?;
                delta.merge(d);
                push_opt = Some(push);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            if let Some(mp) = push_opt {
                pushes.push(mp);
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // VIP Benefits (`[tm]vipBenefit.*`, `[tm]weekGift.buy`)
        // ====================================================================
        "vipBenefit.getBenefitInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let day_bucket = now_ts / 86_400;
            let vb = get_sub_obj(profile, "vip_benefit");
            let daily = if obj_u64(&vb, "day_bucket", u64::MAX) == day_bucket {
                vb.get("daily")
                    .and_then(PlayerJson::as_array)
                    .map(|arr| {
                        Value::dense_array(
                            arr.iter()
                                .filter_map(|v| v.as_u64().map(|n| Value::Int(n as i32)))
                                .collect(),
                        )
                    })
                    .unwrap_or_else(Value::empty_array)
            } else {
                Value::empty_array()
            };
            let weekly = vb
                .get("weekly")
                .and_then(PlayerJson::as_array)
                .map(|arr| {
                    Value::dense_array(
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(|n| Value::Int(n as i32)))
                            .collect(),
                    )
                })
                .unwrap_or_else(Value::empty_array);
            let buy_info = player_json_to_amf3(&PlayerJson::Object(obj_nested(&vb, "week_gift_buy")));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("daily", daily),
                    ("weekly", weekly),
                    (
                        "weekGift",
                        Value::map([
                            ("time", u64_to_amf3(now_ts)),
                            ("buyInfo", buy_info),
                        ]),
                    ),
                ]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "vipBenefit.makeDailyBenefit" | "vipBenefit.makeWeeklyBenefit" => {
            let vip_req = arg_i64(req, 0).unwrap_or(0).clamp(0, 16) as u32;
            let is_daily = m == "vipBenefit.makeDailyBenefit";
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                if p.vip < vip_req {
                    return Err(PlayerError::InvalidParameter("vip_too_low".into()));
                }
                let day_bucket = now_ts / 86_400;
                let mut vb = get_sub_obj(p, "vip_benefit");
                let arr_key = if is_daily { "daily" } else { "weekly" };
                let mut arr = if !is_daily || obj_u64(&vb, "day_bucket", u64::MAX) == day_bucket {
                    vb.get(arr_key)
                        .and_then(PlayerJson::as_array)
                        .map(<[PlayerJson]>::to_vec)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                if arr.iter().any(|v| v.as_u64() == Some(u64::from(vip_req))) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                arr.push(PlayerJson::from(vip_req));
                if is_daily {
                    vb.insert("day_bucket".into(), PlayerJson::from(day_bucket));
                }
                vb.insert(arr_key.into(), PlayerJson::Array(arr));
                set_sub_obj(p, "vip_benefit", vb);

                let copper_gain = u64::from(vip_req + 1) * 10_000;
                let mut rw = vec![(1u32, 0u32, copper_gain)];
                if vip_req >= 1 {
                    rw.push((4, 16_001, u64::from(vip_req.min(10))));
                }
                let (d, _) = grant_triplets(p, &rw, now_ts)?;
                delta.merge(d);
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

        "weekGift.buy" => {
            let id = arg_i64(req, 0).unwrap_or(1).clamp(1, 13) as u32;
            let (cost_gold, rw): (u64, &[Triplet]) = match id {
                1 => (100, &[(1, 0, 200_000)]),
                2 => (125, &[(4, 22_202, 5)]),
                3 => (125, &[(4, 22_102, 5)]),
                4 => (240, &[(4, 17_004, 5)]),
                _ => (320, &[(4, 10_204, 20)]),
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, cost_gold, now_ts)?;
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
                let mut vb = get_sub_obj(p, "vip_benefit");
                let mut bi = obj_nested(&vb, "week_gift_buy");
                let k = id.to_string();
                let cur = obj_u64(&bi, &k, 0);
                bi.insert(k, PlayerJson::from(cur + 1));
                vb.insert("week_gift_buy".into(), PlayerJson::Object(bi));
                set_sub_obj(p, "vip_benefit", vb);
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
        // Growth & Funds (`growUp.*`, `growth.*`, `firstWeek.*`, `activityMonthFund.*`,
        // `activityWeekFund.*`, `activityRechargeFund.*`, `multipleFunds.*`)
        // ====================================================================
        "growUp.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let gu = get_sub_obj(profile, "grow_up");
            let claimed = gu
                .get("claimed")
                .and_then(PlayerJson::as_array)
                .map(|arr| {
                    Value::dense_array(
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(|n| Value::Int(n as i32)))
                            .collect(),
                    )
                })
                .unwrap_or_else(Value::empty_array);
            Some(Ok(PlayerRpcSuccess {
                ret: claimed,
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "growUp.make" => {
            let req_lv = arg_i64(req, 0).unwrap_or(7).max(1) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                if p.level < req_lv {
                    return Err(PlayerError::InvalidParameter("level_not_enough".into()));
                }
                let mut gu = get_sub_obj(p, "grow_up");
                let mut arr = gu
                    .get("claimed")
                    .and_then(PlayerJson::as_array)
                    .map(<[PlayerJson]>::to_vec)
                    .unwrap_or_default();
                if arr.iter().any(|v| v.as_u64() == Some(u64::from(req_lv))) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                arr.push(PlayerJson::from(req_lv));
                gu.insert("claimed".into(), PlayerJson::Array(arr));
                set_sub_obj(p, "grow_up", gu);
                let (d, _) = grant_triplets(p, &[(2, 0, 150), (1, 0, 50_000), (4, 22_102, 2)], now_ts)?;
                delta.merge(d);
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

        "growth.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let gr = get_sub_obj(profile, "growth_fund");
            let is_buy = obj_u64(&gr, "is_buy", 0) as i32;
            let user_num = 500 + is_buy;
            let va_gold = player_json_to_amf3(&PlayerJson::Object(obj_nested(&gr, "va_gold")));
            let va_benefit = player_json_to_amf3(&PlayerJson::Object(obj_nested(&gr, "va_benefit")));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("userNum", Value::Int(user_num)),
                    ("is_buy", Value::Int(is_buy)),
                    ("va_gold", va_gold),
                    ("va_benefit", va_benefit),
                ]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "growth.getNum" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let gr = get_sub_obj(profile, "growth_fund");
            let is_buy = obj_u64(&gr, "is_buy", 0) as i32;
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Int(500 + is_buy),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "growth.buy" => {
            let res = store.mutate_player(uid, now_ts, |p| {
                if p.vip < 3 {
                    return Err(PlayerError::InvalidParameter("vip_too_low".into()));
                }
                let mut gr = get_sub_obj(p, "growth_fund");
                if obj_u64(&gr, "is_buy", 0) == 1 {
                    return Err(PlayerError::InvalidParameter("already_bought".into()));
                }
                p.consume_currency(CurrencyType::Gold, 500, now_ts)?;
                gr.insert("is_buy".into(), PlayerJson::from(1u32));
                set_sub_obj(p, "growth_fund", gr);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            Some(Ok(PlayerRpcSuccess {
                ret: Value::Int(501),
                caller_pushes: vec![build_user_update_env(updated, is_tm, now_i64)],
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "growth.receiveReward" => {
            let rtype = arg_i64(req, 0).unwrap_or(1);
            let id = arg_i64(req, 1).unwrap_or(1).max(1) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut gr = get_sub_obj(p, "growth_fund");
                if rtype == 1 {
                    if obj_u64(&gr, "is_buy", 0) == 0 {
                        return Err(PlayerError::InvalidParameter("growth_not_bought".into()));
                    }
                    let mut vg = obj_nested(&gr, "va_gold");
                    let k = id.to_string();
                    if obj_u64(&vg, &k, 0) == 1 {
                        return Err(PlayerError::InvalidParameter("already_claimed".into()));
                    }
                    let gold_rw = match id {
                        1 => 500,
                        2 | 3 => 1_000,
                        4 | 5 => 1_500,
                        _ => 2_000,
                    };
                    p.grant_currency(CurrencyType::Gold, gold_rw, now_ts);
                    vg.insert(k, PlayerJson::from(1u32));
                    gr.insert("va_gold".into(), PlayerJson::Object(vg));
                } else {
                    let mut vb = obj_nested(&gr, "va_benefit");
                    let k = id.to_string();
                    if obj_u64(&vb, &k, 0) == 1 {
                        return Err(PlayerError::InvalidParameter("already_claimed".into()));
                    }
                    let (d, _) = grant_triplets(p, &[(2, 0, 200), (4, 22_102, 5)], now_ts)?;
                    delta.merge(d);
                    vb.insert(k, PlayerJson::from(1u32));
                    gr.insert("va_benefit".into(), PlayerJson::Object(vb));
                }
                set_sub_obj(p, "growth_fund", gr);
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

        "firstWeek.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let fw = get_sub_obj(profile, "first_week");
            let open_ts = obj_u64(&fw, "open_time", if profile.gold_recharge_accum_all >= 2_000 { profile.create_time.max(1) } else { 0 });
            let va_rw = player_json_to_amf3(&PlayerJson::Object(obj_nested(&fw, "va_reward")));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("open_time", u64_to_amf3(open_ts)),
                    ("va_reward", va_rw),
                ]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "firstWeek.recReward" => {
            let day = arg_i64(req, 0).unwrap_or(1).clamp(1, 7) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut fw = get_sub_obj(p, "first_week");
                let mut vr = obj_nested(&fw, "va_reward");
                let k = day.to_string();
                if obj_u64(&vr, &k, 0) > 0 {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                vr.insert(k, PlayerJson::from(1u32));
                fw.insert("va_reward".into(), PlayerJson::Object(vr));
                set_sub_obj(p, "first_week", fw);
                let (d, _) = grant_triplets(p, &[(4, 28_002, 2), (4, 16_001, 10)], now_ts)?;
                delta.merge(d);
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

        "activityMonthFund.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_month_fund_value(profile),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityMonthFund.recReward" => {
            let day = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let reward_id = arg_i64(req, 1).unwrap_or(1).clamp(1, 2) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut mf = get_sub_obj(p, "month_fund");
                let mut va_rec = obj_nested(&mf, "va_rec_info");
                let rk = reward_id.to_string();
                let mut sub = obj_nested(&va_rec, &rk);
                let dk = day.to_string();
                if sub.contains_key(&dk) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                sub.insert(dk, PlayerJson::from(day));
                va_rec.insert(rk, PlayerJson::Object(sub));
                mf.insert("va_rec_info".into(), PlayerJson::Object(va_rec));
                set_sub_obj(p, "month_fund", mf);
                let rw: &[Triplet] = if reward_id == 1 {
                    &[(2, 0, 100), (4, 22_102, 2)]
                } else {
                    &[(2, 0, 300), (4, 16_012, 2)]
                };
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
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

        "activityWeekFund.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_week_fund_value(profile),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityWeekFund.fetchReward" | "activityWeekFund.fetchAllReward" => {
            let ftype = arg_i64(req, 0).unwrap_or(1).clamp(1, 2) as u32;
            let day = arg_i64(req, 1).unwrap_or(1).clamp(1, 7) as u32;
            let is_box = m == "activityWeekFund.fetchAllReward";
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut wf = get_sub_obj(p, "week_fund");
                if is_box {
                    if obj_u64(&wf, "all_reward", 0) > 0 {
                        return Err(PlayerError::InvalidParameter("already_claimed".into()));
                    }
                    wf.insert("all_reward".into(), PlayerJson::from(1u32));
                } else {
                    let mut vr = obj_nested(&wf, "va_reward");
                    let tk = ftype.to_string();
                    let mut sub = obj_nested(&vr, &tk);
                    let dk = day.to_string();
                    if sub.contains_key(&dk) {
                        return Err(PlayerError::InvalidParameter("already_claimed".into()));
                    }
                    sub.insert(dk, PlayerJson::from(1u32));
                    vr.insert(tk, PlayerJson::Object(sub));
                    wf.insert("va_reward".into(), PlayerJson::Object(vr));
                }
                set_sub_obj(p, "week_fund", wf);
                let (d, _) = grant_triplets(p, &[(2, 0, 200), (4, 16_012, 2)], now_ts)?;
                delta.merge(d);
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

        "activityRechargeFund.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let rf = get_sub_obj(profile, "recharge_fund");
            let va_buy = player_json_to_amf3(&PlayerJson::Object(obj_nested(&rf, "va_buy")));
            let va_rec = player_json_to_amf3(&PlayerJson::Object(obj_nested(&rf, "va_receive")));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([("va_buy", va_buy), ("va_receive", va_rec)]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityRechargeFund.recieveReward" => {
            let id = arg_i64(req, 0).unwrap_or(1).clamp(1, 3) as u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut rf = get_sub_obj(p, "recharge_fund");
                let mut vr = obj_nested(&rf, "va_receive");
                let k = id.to_string();
                let cur = obj_u64(&vr, &k, 0);
                if cur >= 5 {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                vr.insert(k, PlayerJson::from(cur + 1));
                rf.insert("va_receive".into(), PlayerJson::Object(vr));
                set_sub_obj(p, "recharge_fund", rf);
                let gold_rw = match id {
                    1 => 100,
                    2 => 300,
                    _ => 800,
                };
                p.grant_currency(CurrencyType::Gold, gold_rw, now_ts);
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

        "multipleFunds.getRedDoc" => Some(Ok(PlayerRpcSuccess {
            ret: Value::map([
                ("1", Value::Int(0)),
                ("2", Value::Int(0)),
                ("3", Value::Int(0)),
                ("4", Value::Int(0)),
            ]),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "multipleFunds.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let fund_type = arg_i64(req, 0).unwrap_or(1).clamp(1, 4) as u32;
            let mf = get_sub_obj(profile, "multiple_funds");
            let mut stage_entries = Vec::new();
            for stage in 1..=6u32 {
                let is_rech = obj_u64(&mf, &format!("{fund_type}_{stage}_recharged"), 0) as i32;
                let free_info = player_json_to_amf3(&PlayerJson::Object(obj_nested(
                    &mf,
                    &format!("{fund_type}_{stage}_free"),
                )));
                let rech_info = player_json_to_amf3(&PlayerJson::Object(obj_nested(
                    &mf,
                    &format!("{fund_type}_{stage}_rech"),
                )));
                stage_entries.push((
                    stage.to_string(),
                    Value::map([
                        ("is_recharge", Value::Int(is_rech)),
                        ("va_free_info", free_info),
                        ("va_recharge_info", rech_info),
                    ]),
                ));
            }
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map(stage_entries),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "multipleFunds.receiveReward" | "multipleFunds.receiveAllReward" => {
            let fund_type = arg_i64(req, 0).unwrap_or(1).clamp(1, 4) as u32;
            let id = arg_i64(req, 1).unwrap_or(1).max(1) as u32;
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut mf = get_sub_obj(p, "multiple_funds");
                let fk = format!("{fund_type}_1_free");
                let mut fi = obj_nested(&mf, &fk);
                fi.insert(id.to_string(), PlayerJson::from(3u32));
                mf.insert(fk, PlayerJson::Object(fi));
                set_sub_obj(p, "multiple_funds", mf);
                p.grant_currency(CurrencyType::Gold, 200, now_ts);
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

        "multipleFunds.useVoucher" => {
            let fund_type = arg_i64(req, 0).unwrap_or(1).clamp(1, 4) as u32;
            let stage = arg_i64(req, 1).unwrap_or(1).clamp(1, 6) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, 600, now_ts);
                let mut mf = get_sub_obj(p, "multiple_funds");
                mf.insert(format!("{fund_type}_{stage}_recharged"), PlayerJson::from(1u32));
                set_sub_obj(p, "multiple_funds", mf);
                p.grant_currency(CurrencyType::Gold, 600, now_ts);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let cb = if is_tm {
                "tmMultipleFunds.buy"
            } else {
                "multipleFunds.buy"
            };
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            pushes.push(
                S2cEnvelope::push(
                    cb,
                    Value::map([
                        ("vipExp", Value::Int(600)),
                        ("voucher", Value::Bool(false)),
                        ("type", Value::Int(fund_type as i32)),
                        ("stage", Value::Int(stage as i32)),
                    ]),
                )
                .with_time(now_i64),
            );
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // Festival `Hoạt động` RPCs (`discount.*`, `loginReward.*`, `consume.*`,
        // `recharge.*`, `activityRechargeReward.*`)
        // ====================================================================
        "discount.getInfo" | "mergeDiscount.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let dc = get_sub_obj(profile, "festival_discount");
            Some(Ok(PlayerRpcSuccess {
                ret: player_json_to_amf3(&PlayerJson::Object(dc)),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "discount.buy" | "mergeDiscount.buy" => {
            let id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let num = arg_i64(req, 2)
                .or_else(|| arg_i64(req, 1))
                .unwrap_or(1)
                .clamp(1, 10) as u32;
            let (unit_gold, rw_tid, rw_qty, limit): (u64, u32, u32, u32) = match id {
                1 | 4 | 7 | 9 | 10 => (88, 16_012, 2, 5),
                2 | 5 => (188, 22_102, 5, 5),
                8 => (188, 22_202, 5, 5),
                _ => (388, 28_002, 2, 3),
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut dc = get_sub_obj(p, "festival_discount");
                let k = id.to_string();
                let bought = obj_u64(&dc, &k, 0) as u32;
                if bought + num > limit {
                    return Err(PlayerError::InvalidParameter("discount_limit_reached".into()));
                }
                p.consume_currency(CurrencyType::Gold, unit_gold * u64::from(num), now_ts)?;
                let (d, _) = grant_triplets(
                    p,
                    &[(4, rw_tid, u64::from(rw_qty) * u64::from(num))],
                    now_ts,
                )?;
                delta.merge(d);
                dc.insert(k, PlayerJson::from(bought + num));
                set_sub_obj(p, "festival_discount", dc);
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

        "loginReward.getInfo" | "loginReward_1.getInfo" | "mergeLoginReward.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_login_reward_value(profile, "login_reward"),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "loginReward.make" | "loginReward_1.make" | "mergeLoginReward.make" => {
            let day = arg_i64(req, 0).unwrap_or(1).clamp(1, 7) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut lr = get_sub_obj(p, "login_reward");
                let mut maked = lr
                    .get("maked")
                    .and_then(PlayerJson::as_array)
                    .map(<[PlayerJson]>::to_vec)
                    .unwrap_or_default();
                if maked.iter().any(|v| v.as_u64() == Some(u64::from(day))) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                maked.push(PlayerJson::from(day));
                lr.insert("login_num".into(), PlayerJson::from(7u32));
                lr.insert("maked".into(), PlayerJson::Array(maked));
                set_sub_obj(p, "login_reward", lr);

                let rw: &[Triplet] = match day {
                    1 => &[(2, 0, 100), (4, 22_102, 2)],
                    2 => &[(2, 0, 150), (4, 22_202, 2)],
                    3 => &[(2, 0, 200), (4, 16_012, 2)],
                    4 => &[(2, 0, 250), (4, 17_004, 5)],
                    5 => &[(2, 0, 300), (4, 10_204, 20)],
                    6 => &[(2, 0, 400), (4, 15_005, 200)],
                    _ => &[(2, 0, 500), (4, 28_002, 2)],
                };
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
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

        "consume.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_consume_info_value(profile),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "consume.recieveReward" => {
            let req_consume = arg_i64(req, 0).unwrap_or(200).max(1) as u64;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let spent = p
                    .event_state
                    .get("consume_gold_total")
                    .and_then(PlayerJson::as_u64)
                    .unwrap_or(0);
                if spent < req_consume {
                    return Err(PlayerError::InvalidParameter("consume_not_enough".into()));
                }
                let mut got = get_sub_obj(p, "consume_got");
                let k = req_consume.to_string();
                if got.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                got.insert(k, PlayerJson::Bool(true));
                set_sub_obj(p, "consume_got", got);
                let rw: &[Triplet] = match req_consume {
                    0..=200 => &[(4, 22_102, 2), (1, 0, 100_000)],
                    201..=500 => &[(4, 16_012, 2), (1, 0, 200_000)],
                    501..=1_000 => &[(4, 17_004, 5), (4, 22_202, 3)],
                    1_001..=3_000 => &[(4, 28_002, 2), (4, 10_204, 30)],
                    _ => &[(4, 27_501, 1), (4, 15_005, 500)],
                };
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
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

        "recharge.getInfo" | "activityRecharge2.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_accum_recharge_value(profile),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "recharge.recieveReward" | "activityRecharge2.recieveReward" => {
            let req_rmb = arg_i64(req, 0).unwrap_or(6).max(1) as u64;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let rmb = p
                    .event_state
                    .get("act_recharge_rmb")
                    .and_then(PlayerJson::as_u64)
                    .unwrap_or(p.gold_recharge_accum_all / 10);
                if rmb < req_rmb {
                    return Err(PlayerError::InvalidParameter("recharge_not_enough".into()));
                }
                let mut got = get_sub_obj(p, "recharge_got");
                let k = req_rmb.to_string();
                if got.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                got.insert(k, PlayerJson::Bool(true));
                set_sub_obj(p, "recharge_got", got);
                let rw: &[Triplet] = match req_rmb {
                    0..=6 => &[(2, 0, 100), (4, 22_102, 2)],
                    7..=30 => &[(2, 0, 300), (4, 16_012, 3)],
                    31..=98 => &[(2, 0, 888), (4, 28_002, 2)],
                    99..=328 => &[(2, 0, 1_888), (4, 27_501, 1)],
                    _ => &[(2, 0, 3_888), (4, 28_006, 1)],
                };
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
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

        "activityRechargeReward.getInfo"
        | "activityRechargeReward2.getInfo"
        | "activityRechargeRewardNew.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_single_recharge_value(profile),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityRechargeReward.recieveReward"
        | "activityRechargeReward2.recieveReward"
        | "activityRechargeRewardNew.recieveReward" => {
            let raw_arg = arg_i64(req, 0).unwrap_or(6).max(1) as u64;
            let req_rmb = match raw_arg {
                1 => 6,
                2 => 30,
                3 => 98,
                4 => 198,
                5 => 328,
                other => other,
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut single = get_sub_obj(p, "act_single_recharge");
                let k = req_rmb.to_string();
                let mut sub = obj_nested(&single, &k);
                let max_num = obj_u64(&sub, "max_num", 0);
                let now_num = obj_u64(&sub, "now_num", 0);
                if now_num >= max_num {
                    return Err(PlayerError::InvalidParameter("single_recharge_not_available".into()));
                }
                sub.insert("now_num".into(), PlayerJson::from(now_num + 1));
                single.insert(k, PlayerJson::Object(sub));
                set_sub_obj(p, "act_single_recharge", single);
                let rw: &[Triplet] = match req_rmb {
                    0..=6 => &[(2, 0, 100), (4, 16_012, 2)],
                    7..=30 => &[(2, 0, 300), (4, 22_102, 5)],
                    _ => &[(2, 0, 888), (4, 28_002, 2)],
                };
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
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
        // `Lạc Thưởng Cẩm Thành` (`[tm]activityActiveTaskShop.*`)
        // ====================================================================
        "activityActiveTaskShop.getTaskInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_active_task_shop_task_info(profile),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityActiveTaskShop.getShopInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_active_task_shop_shop_info(profile),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityActiveTaskShop.receiveTaskReward" => {
            let p_type = arg_i64(req, 0).unwrap_or(1);
            let p_id = arg_i64(req, 1).unwrap_or(101).max(1) as u32;
            let ticket_qty: u64 = match p_id {
                101 | 102 => 1,
                105 | 203 | 204 => 3,
                201 | 202 => 2,
                205 => 5,
                206 => 8,
                _ => 12,
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut ats = get_sub_obj(p, "active_task_shop");
                let list_key = if p_type == 1 {
                    "daily_task_list"
                } else {
                    "active_task_list"
                };
                let mut tl = obj_nested(&ats, list_key);
                let k = p_id.to_string();
                if tl.get(&k).and_then(PlayerJson::as_bool) == Some(true) {
                    return Err(PlayerError::InvalidParameter("already_claimed".into()));
                }
                tl.insert(k, PlayerJson::Bool(true));
                ats.insert(list_key.into(), PlayerJson::Object(tl));
                set_sub_obj(p, "active_task_shop", ats);
                let (d, _) = grant_triplets(p, &[(4, 19_903, ticket_qty)], now_ts)?;
                delta.merge(d);
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

        "activityActiveTaskShop.exchange" => {
            let ware_id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let num = arg_i64(req, 1).unwrap_or(1).clamp(1, 50) as u32;
            let Some(&(_, out_tid, out_qty, ticket_cost, limit)) =
                ACTIVE_TASK_SHOP_WARES.iter().find(|&&(id, _, _, _, _)| id == ware_id)
            else {
                return Some(Err("invalid_ware_id".into()));
            };
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut ats = get_sub_obj(p, "active_task_shop");
                let cur_round = obj_u64(&ats, "cur_round", 1).max(1);
                let rk = cur_round.to_string();
                let mut ex = obj_nested(&ats, "exchange_shop");
                let mut r_obj = obj_nested(&ex, &rk);
                let mut goods = obj_nested(&r_obj, "goods_list");
                let wk = ware_id.to_string();
                let used = obj_u64(&goods, &wk, 0) as u32;
                if used + num > limit {
                    return Err(PlayerError::InvalidParameter("ware_limit_exceeded".into()));
                }
                let total_cost = ticket_cost * num;
                let (_, d_sub) = p.remove_item_by_template_with_delta(19_903, total_cost)?;
                delta.merge(d_sub);
                let (d_add, _) = grant_triplets(
                    p,
                    &[(4, out_tid, u64::from(out_qty) * u64::from(num))],
                    now_ts,
                )?;
                delta.merge(d_add);

                goods.insert(wk, PlayerJson::from(used + num));
                let prev_use = obj_u64(&r_obj, "use_coin", 0);
                r_obj.insert("use_coin".into(), PlayerJson::from(prev_use + u64::from(total_cost)));
                r_obj.insert("goods_list".into(), PlayerJson::Object(goods));
                ex.insert(rk, PlayerJson::Object(r_obj));
                ats.insert("exchange_shop".into(), PlayerJson::Object(ex));
                set_sub_obj(p, "active_task_shop", ats);
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

        "activityActiveTaskShop.buy" => {
            let gift_id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let Some(&(_, gold_opt, _, limit, rw)) =
                ACTIVE_TASK_SHOP_GIFTS.iter().find(|&&(id, _, _, _, _)| id == gift_id)
            else {
                return Some(Err("invalid_gift_id".into()));
            };
            let gold_cost = gold_opt.unwrap_or(1_888);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut ats = get_sub_obj(p, "active_task_shop");
                let mut act_shop = obj_nested(&ats, "active_shop");
                let k = gift_id.to_string();
                let bought = obj_u64(&act_shop, &k, 0) as u32;
                if bought >= limit {
                    return Err(PlayerError::InvalidParameter("gift_limit_exceeded".into()));
                }
                p.consume_currency(CurrencyType::Gold, gold_cost, now_ts)?;
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
                act_shop.insert(k, PlayerJson::from(bought + 1));
                ats.insert("active_shop".into(), PlayerJson::Object(act_shop));
                set_sub_obj(p, "active_task_shop", ats);
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

        "activityActiveTaskShop.nextRound" => {
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut ats = get_sub_obj(p, "active_task_shop");
                let cur = obj_u64(&ats, "cur_round", 1);
                ats.insert("cur_round".into(), PlayerJson::from((cur + 1).min(2)));
                set_sub_obj(p, "active_task_shop", ats);
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

        "activityActiveTaskShop.useVoucher" => {
            let gid = arg_i64(req, 0).unwrap_or(2).clamp(2, 5) as u32;
            let &(_, _, vip_exp, _, rw) = ACTIVE_TASK_SHOP_GIFTS
                .iter()
                .find(|&&(id, _, _, _, _)| id == gid)
                .unwrap_or(&ACTIVE_TASK_SHOP_GIFTS[1]);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, vip_exp, now_ts);
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
                let mut ats = get_sub_obj(p, "active_task_shop");
                let mut act_shop = obj_nested(&ats, "active_shop");
                let k = gid.to_string();
                let cur = obj_u64(&act_shop, &k, 0);
                act_shop.insert(k, PlayerJson::from(cur + 1));
                ats.insert("active_shop".into(), PlayerJson::Object(act_shop));
                set_sub_obj(p, "active_task_shop", ats);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let cb = if is_tm {
                "tmActivityActiveTaskShop.pushNormalPay"
            } else {
                "activityActiveTaskShop.pushNormalPay"
            };
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            pushes.push(
                S2cEnvelope::push(
                    cb,
                    Value::map([
                        ("id", Value::Int(gid as i32)),
                        ("vipExp", u64_to_amf3(vip_exp)),
                        ("voucher", Value::Int(0)),
                    ]),
                )
                .with_time(now_i64),
            );
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // `8 Năm Lễ Hội` (`[tm]activity8Years.*`)
        // ====================================================================
        "activity8Years.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            Some(Ok(PlayerRpcSuccess {
                ret: build_eight_years_info(profile, now_ts),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activity8Years.buyFreeGift" => {
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let day_bucket = now_ts / 86_400;
                let ey = get_sub_obj(p, "eight_years");
                let mut gi = obj_nested(&ey, "gift_info");
                if obj_u64(&ey, "free_gift_day", u64::MAX) == day_bucket
                    && obj_u64(&gi, "4", 0) > 0
                {
                    return Err(PlayerError::InvalidParameter("already_claimed_today".into()));
                }
                let (d, _) = grant_triplets(p, &[(95, 0, 1), (2, 0, 188)], now_ts)?;
                delta.merge(d);
                let mut ey2 = get_sub_obj(p, "eight_years");
                gi.insert("4".into(), PlayerJson::from(1u32));
                ey2.insert("free_gift_day".into(), PlayerJson::from(day_bucket));
                ey2.insert("gift_info".into(), PlayerJson::Object(gi));
                set_sub_obj(p, "eight_years", ey2);
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

        "activity8Years.useVoucher" => {
            let cid = arg_i64(req, 0).unwrap_or(5).clamp(1, 12) as u32;
            let &(_, vip_exp, rw) = EIGHT_YEARS_CHARGES
                .iter()
                .find(|&&(id, _, _)| id == cid)
                .unwrap_or(&EIGHT_YEARS_CHARGES[4]);
            let eff_vip = vip_exp.max(240);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, eff_vip, now_ts);
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
                let mut ey = get_sub_obj(p, "eight_years");
                let mut gi = obj_nested(&ey, "gift_info");
                let k = cid.to_string();
                let cur = obj_u64(&gi, &k, 0);
                gi.insert(k, PlayerJson::from(cur + 1));
                ey.insert("gift_info".into(), PlayerJson::Object(gi));
                set_sub_obj(p, "eight_years", ey);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let cb = if is_tm {
                "tmActivity8Years.pushNormalPay"
            } else {
                "activity8Years.pushNormalPay"
            };
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            pushes.push(
                S2cEnvelope::push(
                    cb,
                    Value::map([
                        ("id", Value::Int(cid as i32)),
                        ("vipExp", u64_to_amf3(eff_vip)),
                        ("voucherNum", Value::Int(0)),
                    ]),
                )
                .with_time(now_i64),
            );
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        "activity8Years.sign"
        | "activity8Years.supSign"
        | "activity8Years.continueRecvSignReawrd"
        | "activity8Years.recvEdictReward"
        | "activity8Years.oneKeyRecvEdictReward"
        | "activity8Years.recvLuckyDailyReward"
        | "activity8Years.exchangeYearShop"
        | "activity8Years.commitDinnerScore"
        | "activity8Years.recvDinnerReward"
        | "activity8Years.recvLuckyReward"
        | "activity8Years.recvWheelReward"
        | "activity8Years.recvTaskReward" => {
            let arg_id = arg_i64(req, 0).unwrap_or(1).max(1) as u32;
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut ey = get_sub_obj(p, "eight_years");
                let k = arg_id.to_string();
                match m {
                    "activity8Years.sign" | "activity8Years.supSign" => {
                        let mut si = obj_nested(&ey, "sign_info");
                        let mut cell = obj_nested(&si, &k);
                        cell.insert("sign".into(), PlayerJson::Bool(true));
                        si.insert(k, PlayerJson::Object(cell));
                        ey.insert("sign_info".into(), PlayerJson::Object(si));
                    }
                    "activity8Years.recvTaskReward" => {
                        let mut tc = obj_nested(&ey, "task_claimed");
                        tc.insert(k, PlayerJson::Bool(true));
                        ey.insert("task_claimed".into(), PlayerJson::Object(tc));
                    }
                    _ => {}
                }
                set_sub_obj(p, "eight_years", ey);
                let (d, _) = grant_triplets(p, &[(2, 0, 100), (4, 22_102, 2)], now_ts)?;
                delta.merge(d);
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

        "activity8Years.getDinnerTotalScore" => Some(Ok(PlayerRpcSuccess {
            ret: Value::Int(10_050),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "activity8Years.luckyBuyTicket" => {
            let num = arg_i64(req, 0).unwrap_or(1).clamp(1, 10) as u64;
            let res = store.mutate_player(uid, now_ts, |p| {
                p.consume_currency(CurrencyType::Gold, 100 * num, now_ts)?;
                add_eight_years_currency(p, "lucky_ticket", num);
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

        "activity8Years.luckyLottery" | "activity8Years.wheelLottery" => {
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let (d, _) = grant_triplets(p, &[(4, 16_012, 1), (4, 22_102, 2)], now_ts)?;
                delta.merge(d);
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
                    ("reward", triplets_to_amf3(&[(4, 16_012, 1), (4, 22_102, 2)])),
                ]),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        // ====================================================================
        // `Chiến Lệnh Tự Chọn` (`[tm]activityBattlePass.*`)
        // ====================================================================
        "activityBattlePass.getInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let bp = get_sub_obj(profile, "battle_pass");
            let pass_id = obj_u64(&bp, "pass_id", 0) as i32;
            let pass_score = obj_u64(&bp, "pass_score", 500) as i32;
            let pass_info = player_json_to_amf3(&PlayerJson::Object(obj_nested(&bp, "pass_info")));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("pass_id", Value::Int(pass_id)),
                    ("pass_score", Value::Int(pass_score)),
                    ("pass_info", pass_info),
                ]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "activityBattlePass.recvReward" => {
            let id = parse_positive_u32_arg(req.args.first()).unwrap_or(1).clamp(1, 5);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                let mut bp = get_sub_obj(p, "battle_pass");
                let pass_id = obj_u64(&bp, "pass_id", 0);
                let pass_score = obj_u64(&bp, "pass_score", 500);
                if pass_score < u64::from(id) * 100 {
                    return Err(PlayerError::InvalidParameter("not enough".into()));
                }
                let mut pi = obj_nested(&bp, "pass_info");
                let k = id.to_string();
                let mut cell = obj_nested(&pi, &k);
                let normal_done = obj_bool(&cell, "normal", false);
                let high_done = obj_bool(&cell, "high", false);
                if normal_done && (pass_id == 0 || high_done) {
                    return Err(PlayerError::InvalidParameter("have received".into()));
                }
                let mut rw = Vec::new();
                if !normal_done {
                    cell.insert("normal".into(), PlayerJson::Bool(true));
                    match id {
                        1 => rw.push((2, 0, 100)),
                        2 => rw.push((4, 22_102, 2)),
                        3 => rw.push((4, 16_012, 2)),
                        4 => rw.push((2, 0, 200)),
                        _ => rw.push((4, 28_002, 2)),
                    }
                }
                if pass_id > 0 && !high_done {
                    cell.insert("high".into(), PlayerJson::Bool(true));
                    rw.push((2, 0, 300));
                }
                pi.insert(k, PlayerJson::Object(cell));
                bp.insert("pass_info".into(), PlayerJson::Object(pi));
                set_sub_obj(p, "battle_pass", bp);
                let (d, _) = grant_triplets(p, &rw, now_ts)?;
                delta.merge(d);
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

        "activityBattlePass.useVoucher" => {
            let prod_id = parse_positive_u32_arg(req.args.first()).unwrap_or(1);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, 600, now_ts);
                let (d, _) = grant_triplets(p, &[(2, 0, 600), (4, 16_012, 5)], now_ts)?;
                delta.merge(d);
                let mut bp = get_sub_obj(p, "battle_pass");
                bp.insert("pass_id".into(), PlayerJson::from(prod_id));
                bp.insert("pass_score".into(), PlayerJson::from(500u32));
                set_sub_obj(p, "battle_pass", bp);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            pushes.push(
                S2cEnvelope::push(
                    "activityBattlePass.pushNormalPay",
                    Value::map([
                        ("id", Value::Int(prod_id as i32)),
                        ("vipExp", Value::Int(600)),
                        ("voucherNum", Value::Int(0)),
                    ]),
                )
                .with_time(now_i64),
            );
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        // ====================================================================
        // `SuperMapDirectRecharge2` (`[tm]crossGuildWar.*`)
        // ====================================================================
        "crossGuildWar.getBaseInfo" => {
            let Some(profile) = store.get_player(uid) else {
                return Some(Err("player_missing".into()));
            };
            let sm = get_sub_obj(profile, "super_map_direct");
            let buy_info = player_json_to_amf3(&PlayerJson::Object(sm));
            Some(Ok(PlayerRpcSuccess {
                ret: Value::map([
                    ("status", Value::Int(1)),
                    ("stage", Value::Int(1)),
                    ("itemBuyNum", buy_info.clone()),
                    ("buy_info", buy_info),
                    ("guild_gift", Value::empty_array()),
                ]),
                caller_pushes: Vec::new(),
                peer_pushes: Vec::new(),
                refresh_roles_pid: None,
            }))
        }

        "crossGuildWar.tryBuyGuildGift" => Some(Ok(PlayerRpcSuccess {
            ret: Value::String("ok".into()),
            caller_pushes: Vec::new(),
            peer_pushes: Vec::new(),
            refresh_roles_pid: None,
        })),

        "crossGuildWar.useVoucher" => {
            let buy_id = arg_i64(req, 0).unwrap_or(1).clamp(1, 3) as u32;
            let rows = if is_tm {
                TM_RECHARGE_CURRENT_ROWS
            } else {
                RECHARGE_CURRENT_ROWS
            };
            let &(prod_id, vip_exp, rw) = rows
                .iter()
                .find(|&&(id, _, _)| id == buy_id)
                .unwrap_or(&rows[0]);
            let mut delta = BagDelta::new();
            let res = store.mutate_player(uid, now_ts, |p| {
                delta.merge(try_consume_any_voucher_ticket(p, &[19_901, 19_902], now_ts));
                record_full_recharge_with_festivals(p, vip_exp, now_ts);
                let (d, _) = grant_triplets(p, rw, now_ts)?;
                delta.merge(d);
                let mut sm = get_sub_obj(p, "super_map_direct");
                let k = prod_id.to_string();
                let cur = obj_u64(&sm, &k, 0);
                sm.insert(k, PlayerJson::from(cur + 1));
                set_sub_obj(p, "super_map_direct", sm);
                Ok(())
            });
            if let Err(e) = res {
                return Some(Err(map_player_err(e)));
            }
            resync_gameplay_with_store(store, gameplay, data_store, uid);
            let updated = store.get_player(uid).expect("exists");
            let pid = updated.pid.clone();
            let cb = if is_tm {
                "re.tmCrossGuildWar.buyUpdate"
            } else {
                "re.crossGuildWar.buyUpdate"
            };
            let mut pushes = Vec::new();
            if let Some(bp) = build_bag_delta_env(&delta, is_tm, now_i64) {
                pushes.push(bp);
            }
            pushes.push(build_user_update_env(updated, is_tm, now_i64));
            pushes.push(
                S2cEnvelope::push(
                    cb,
                    Value::map([
                        ("product_id", Value::Int(prod_id as i32)),
                        ("buy_num", Value::Int(1)),
                        ("vip_exp", u64_to_amf3(vip_exp)),
                        ("voucher_num", Value::Int(0)),
                    ]),
                )
                .with_time(now_i64),
            );
            Some(Ok(PlayerRpcSuccess {
                ret: Value::String("ok".into()),
                caller_pushes: pushes,
                peer_pushes: Vec::new(),
                refresh_roles_pid: Some(pid),
            }))
        }

        _ => None,
    }
}
