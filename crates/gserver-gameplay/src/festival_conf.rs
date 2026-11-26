//! Active official festival event schedule (`[tm]activity.getActivityConf` & `FestivalManager.lua`)
//! strictly obeying `BaseCsvReader.lua` (>= 4 non-empty lines, RFC-4180 quoted `array` / `array3` cells)
//! and `ActRedPointManager.lua` safety invariants (excluding nil-proxy `activity_recharge_double`).

use std::collections::BTreeMap;

use crate::protocol::DtoValue;

/// Active festival keys opened in `activity.getActivityConf` / `tmActivity.getActivityConf`.
pub const ACTIVE_FESTIVAL_KEYS: &[&str] = &[
    "activity_time_machine",
    "discount",
    "activity_login_reward",
    "activity_recharge_reward",
    "activity_recharge",
    "activity_consume",
    "activity_month_fund",
    "activity_week_fund",
    "activity_recharge_fund",
    "activity_weekly",
    "activity_monthly",
    "activity_beauty",
    "activity_activeTaskShop",
    "activity_8years",
    "battlePass_optional",
];

fn build_festival_entry(csv_data: &str, start_time: u64, end_time: u64) -> DtoValue {
    DtoValue::map([
        ("version", DtoValue::from(1u32)),
        ("start_time", DtoValue::from(start_time)),
        ("end_time", DtoValue::from(end_time)),
        ("need_open_time", DtoValue::from(2_147_483_647u64)),
        ("data", DtoValue::from(csv_data)),
    ])
}

/// CSV for `"activity_time_machine"` (`TimeMachineData.lua`, `TimeMachineNotifyPanel.lua`, `SelectHeroPanel.lua`).
/// Note: `activityTime[2]` MUST be 6 digits >= `100000` so `tonumber("100000")` stays 6 chars under `string.sub`!
pub const CSV_ACTIVITY_TIME_MACHINE: &str = "\
id,activityTime,ShowTime,Choose_hero\n\
id,activityTime,ShowTime,Choose_hero\n\
0,array,0,array\n\
1,\"0,100000\",86400,\"101,201,301,401\"";

/// CSV for `"discount"` (`ActivityDiscountData.lua`, `ActivityDiscountCell.lua`).
/// Includes rows across days `1..=5` so `cur_day` always resolves active discounted packs.
pub const CSV_DISCOUNT: &str = "\
id,day,level_limit,ware_buy_type,ware_buy_limit,item_discount,item_price,item_id_num\n\
id,day,level_limit,ware_buy_type,ware_buy_limit,item_discount,item_price,item_id_num\n\
0,0,array,0,0,0,array,array\n\
1,1,\"1,200\",1,5,5,2|88,4|16012|2\n\
2,1,\"1,200\",1,5,5,2|188,4|22102|5\n\
3,1,\"1,200\",1,3,5,2|388,4|28002|2\n\
4,2,\"1,200\",1,5,5,2|88,4|16012|2\n\
5,2,\"1,200\",1,5,5,2|188,4|22102|5\n\
6,2,\"1,200\",1,3,5,2|388,4|28002|2\n\
7,3,\"1,200\",1,5,5,2|88,4|16012|2\n\
8,3,\"1,200\",1,5,5,2|188,4|22202|5\n\
9,4,\"1,200\",1,5,5,2|88,4|16012|2\n\
10,5,\"1,200\",1,5,5,2|88,4|16012|2";

/// CSV for `"activity_login_reward"` (`ActivityLoginRewardItem.lua`, `ActivityLoginRewardCell.lua`).
pub const CSV_LOGIN_REWARD: &str = "\
id,days,ware_buy_type,des_spec,reward\n\
id,days,ware_buy_type,des_spec,reward\n\
0,0,0,0,array\n\
1,1,1,LoginDay1,\"2|0|100,4|22102|2\"\n\
2,2,1,LoginDay2,\"2|0|150,4|22202|2\"\n\
3,3,1,LoginDay3,\"2|0|200,4|16012|2\"\n\
4,4,1,LoginDay4,\"2|0|250,4|17004|5\"\n\
5,5,1,LoginDay5,\"2|0|300,4|10204|20\"\n\
6,6,1,LoginDay6,\"2|0|400,4|15005|200\"\n\
7,7,1,LoginDay7,\"2|0|500,4|28002|2\"";

/// CSV for `"activity_recharge_reward"` (`ActSingleRechargeProxy.lua`, `ActSingleRechargeCell.lua`).
pub const CSV_SINGLE_RECHARGE: &str = "\
id,lv_range,recharge_class,charge_require,recharge_num,ware_buy_type,discount,recharge_reward_1,recharge_reward_2,recharge_reward_3\n\
id,lv_range,recharge_class,charge_require,recharge_num,ware_buy_type,discount,recharge_reward_1,recharge_reward_2,recharge_reward_3\n\
0,array,array,array,array,0,array,array,array,array\n\
1,\"0,200\",\"1|2|3,1|2|3\",\"6,30,98\",\"3,3,3\",1,\"300,350,400\",\"2|0|100,4|16012|2\",\"2|0|300,4|22102|5\",\"2|0|888,4|28002|2\"";

/// CSV for `"activity_recharge"` (`ActivityRechargeItem.lua`, `ActivityRechargeCell.lua`).
pub const CSV_ACCUM_RECHARGE: &str = "\
id,recharge,ware_buy_type,discount,reward\n\
id,recharge,ware_buy_type,discount,reward\n\
0,0,0,0,array\n\
1,6,1,300,\"2|0|100,4|22102|2\"\n\
2,30,1,350,\"2|0|300,4|16012|3\"\n\
3,98,1,400,\"2|0|888,4|28002|2\"\n\
4,328,1,500,\"2|0|1888,4|27501|1\"\n\
5,648,1,600,\"2|0|3888,4|28006|1\"";

/// CSV for `"activity_consume"` (`ActConsumeData.lua`, `ActivityConsumeCell.lua`).
pub const CSV_CONSUME_GOLD: &str = "\
id,consume,open_lv,reward\n\
id,consume,open_lv,reward\n\
0,0,0,array\n\
1,200,1,\"4|22102|2,1|0|100000\"\n\
2,500,1,\"4|16012|2,1|0|200000\"\n\
3,1000,1,\"4|17004|5,4|22202|3\"\n\
4,3000,1,\"4|28002|2,4|10204|30\"\n\
5,5000,1,\"4|27501|1,4|15005|500\"";

/// CSV for `"activity_month_fund"` (`ActFundPanel.lua`, `ActFundGetItem.lua`, `ActFundProxy.lua`).
pub const CSV_MONTH_FUND: &str = "\
id,day,condition,level1,level2,demand_name1,demand_name2,price_name1,price_name2,recharge1,recharge2,vip_lv1,vip_factor1,dis_reward1,vip_lv2,vip_factor2,dis_reward2,allbuy_reward\n\
id,day,condition,level1,level2,demand_name1,demand_name2,price_name1,price_name2,recharge1,recharge2,vip_lv1,vip_factor1,dis_reward1,vip_lv2,vip_factor2,dis_reward2,allbuy_reward\n\
0,0,0,0,0,0,0,0,0,0,0,0,0,array,0,0,array,array\n\
1,90,1,1,2,TheThang,TheChiTon,60NB,600NB,60,600,0,0,\"2|0|100,4|22102|2\",0,0,\"2|0|300,4|16012|2\",\"2|0|500,4|28002|2\"\n\
2,90,1,1,2,TheThang,TheChiTon,60NB,600NB,60,600,0,0,\"2|0|100,4|22202|2\",0,0,\"2|0|300,4|17004|5\",\"2|0|500,4|28002|2\"\n\
3,90,1,1,2,TheThang,TheChiTon,60NB,600NB,60,600,0,0,\"2|0|150,4|16012|1\",0,0,\"2|0|400,4|10204|20\",\"2|0|500,4|28002|2\"";

/// CSV for `"activity_week_fund"` (`ActNewWeekFundPanel.lua`, `ActNewWeekFundProxy.lua`).
pub const CSV_NEW_WEEK_FUND: &str = "\
id,price,charge_time,time,type,allbuy_reward,reward1,reward2,reward3,reward4,reward5,reward6,reward7\n\
id,price,charge_time,time,type,allbuy_reward,reward1,reward2,reward3,reward4,reward5,reward6,reward7\n\
0,0,0,0,array,array,array,array,array,array,array,array,array\n\
1,30,90,7,\"1,1,1,1,1,1,1\",\"2|0|500,4|16012|3\",2|0|100,4|22102|2,4|22202|2,4|16012|2,4|17004|5,4|10204|20,2|0|300\n\
2,98,90,7,\"1,1,1,1,1,1,1\",\"2|0|500,4|16012|3\",2|0|300,4|22102|5,4|22202|5,4|16012|5,4|28002|2,4|15005|300,2|0|888";

/// CSV for `"activity_recharge_fund"` (`ActRechargeFundData.lua`, `ActRechargeFundPanel.lua`).
pub const CSV_RECHARGE_FUND: &str = "\
id,price,maxBuy_times,buy_time,total_time,return_times,way\n\
id,price,maxBuy_times,buy_time,total_time,return_times,way\n\
0,0,0,0,0,0,array\n\
1,30,1,30,60,5,\"100,100,100,100,100\"\n\
2,98,1,30,60,5,\"300,300,300,300,300\"\n\
3,328,1,30,60,5,\"800,800,800,800,800\"";

/// CSV for `"activity_weekly"` (`ActWeeklyData.lua`, `ActWeeklyProxy.lua`, `WEELY_COIN = 73`).
pub const CSV_ACTIVITY_WEEKLY: &str = "\
id,vngandroid,vngios,android,task_score,accumulates,allbuy_reward1,giftID1,giftID2,giftID3,reward1,reward2,reward3\n\
id,vngandroid,vngios,android,task_score,accumulates,allbuy_reward1,giftID1,giftID2,giftID3,reward1,reward2,reward3\n\
0,0,0,0,array,array,array,array,array,array,array,array,array\n\
1,1601,1701,1601,\"1,3,5\",\"10,20,30\",\"2|0|300,4|22102|5\",\"73|0|10,4|22102|1\",\"73|0|10,4|22202|1\",\"73|0|10,4|16012|1\",2|0|100,4|16012|2,4|28002|2";

/// CSV for `"activity_monthly"` (`ActMonthlyData.lua`, `ActMonthlyProxy.lua`, `MONTHLY_COIN = 74`).
pub const CSV_ACTIVITY_MONTHLY: &str = "\
id,vngandroid,vngios,android,task_activity,accumulates,allbuy_reward1,giftID1,giftID2,giftID3,reward1,reward2,reward3\n\
id,vngandroid,vngios,android,task_activity,accumulates,allbuy_reward1,giftID1,giftID2,giftID3,reward1,reward2,reward3\n\
0,0,0,0,array,array,array,array,array,array,array,array,array\n\
1,1602,1702,1602,\"1|20,2|40,3|60\",\"10,20,30\",\"2|0|600,4|16012|5\",\"74|0|10,4|22102|2\",\"74|0|10,4|22202|2\",\"74|0|10,4|16012|2\",2|0|200,4|16012|3,4|27501|1";

/// CSV for `"activity_beauty"` (`ActBeautyData.lua`, `ActBeautyProxy.lua`, `BEAUTY_COIN = 90`).
pub const CSV_ACTIVITY_BEAUTY: &str = "\
id,vngandroid,vngios,android,task_activity,accumulates,allbuy_reward1,giftID1,giftID2,giftID3,reward1,reward2,reward3\n\
id,vngandroid,vngios,android,task_activity,accumulates,allbuy_reward1,giftID1,giftID2,giftID3,reward1,reward2,reward3\n\
0,0,0,0,array,array,array,array,array,array,array,array,array\n\
1,1630,1730,1630,\"1|20,2|40,3|60\",\"10,20,30\",\"2|0|600,4|22102|5\",\"90|0|10,4|22102|2\",\"90|0|10,4|22202|2\",\"90|0|10,4|16012|2\",2|0|200,4|16012|3,4|28006|1";

/// CSV for `"activity_activeTaskShop"` (`ActiveTaskShopData.lua`).
pub const CSV_ACTIVE_TASK_SHOP: &str = "\
id,floor,turn,showItemShelf,shelfFloor1,shelfFloor2,shelfFloor3,floorUnlockTurn1,floorUnlockTurn2\n\
id,floor,turn,showItemShelf,shelfFloor1,shelfFloor2,shelfFloor3,floorUnlockTurn1,floorUnlockTurn2\n\
0,0,0,0,array,array,array,array,array\n\
1,3,2,14,\"1,2,3,4,5\",\"6,7,8,9,10\",\"11,12,13,14,15\",\"4|19903|0,4|19903|50,4|19903|150\",\"4|19903|0,4|19903|50,4|19903|150\"";

/// CSV for `"activity_8years"` (`ActEightYearsData.lua`, `ActEightYearsBaseData.lua`, `ActEightYearsLuckEggData.lua`).
pub const CSV_ACTIVITY_8YEARS: &str = "\
id,tab_openID,seven_open,sign_spend,luckdraw_common,luckdraw_num,egg_use,yuanbao_price,egg_num\n\
id,tab_openID,seven_open,sign_spend,luckdraw_common,luckdraw_num,egg_use,yuanbao_price,egg_num\n\
0,array,array,array,array,array,array,array,0\n\
1,\"1,2,3,4,5,6,7,8,9\",\"1,2,3,4,5,6,7,8,9\",2|0|100,\"4|16012|1,4|22102|2\",1|10,4|19133|1,2|0|100,10";

/// CSV for `"battlePass_optional"` (`ActBattlePassData.lua`, `ActBattlePassProxy.lua`).
pub const CSV_BATTLEPASS_OPTIONAL: &str = "\
id,integral,normal_reward\n\
id,integral,normal_reward\n\
0,0,array\n\
1,100,2|0|100\n\
2,200,4|22102|2\n\
3,300,4|16012|2\n\
4,400,2|0|200\n\
5,500,4|28002|2";

/// Builds the complete `[tm]activity.getActivityConf` response (`{"version": 1, "arrData": ...}`).
pub fn build_activity_conf_dto(now_ts: u64) -> DtoValue {
    let start_time = now_ts.saturating_sub(86_400);
    let end_time = now_ts.saturating_add(90 * 86_400);

    let specs: &[(&str, &str)] = &[
        ("activity_time_machine", CSV_ACTIVITY_TIME_MACHINE),
        ("discount", CSV_DISCOUNT),
        ("activity_login_reward", CSV_LOGIN_REWARD),
        ("activity_recharge_reward", CSV_SINGLE_RECHARGE),
        ("activity_recharge", CSV_ACCUM_RECHARGE),
        ("activity_consume", CSV_CONSUME_GOLD),
        ("activity_month_fund", CSV_MONTH_FUND),
        ("activity_week_fund", CSV_NEW_WEEK_FUND),
        ("activity_recharge_fund", CSV_RECHARGE_FUND),
        ("activity_weekly", CSV_ACTIVITY_WEEKLY),
        ("activity_monthly", CSV_ACTIVITY_MONTHLY),
        ("activity_beauty", CSV_ACTIVITY_BEAUTY),
        ("activity_activeTaskShop", CSV_ACTIVE_TASK_SHOP),
        ("activity_8years", CSV_ACTIVITY_8YEARS),
        ("battlePass_optional", CSV_BATTLEPASS_OPTIONAL),
    ];

    let mut arr_data = BTreeMap::new();
    for &(fid, csv) in specs {
        arr_data.insert(
            fid.to_owned(),
            build_festival_entry(csv, start_time, end_time),
        );
    }

    DtoValue::map([
        ("version", DtoValue::from(1u32)),
        ("arrData", DtoValue::Map(arr_data)),
    ])
}

/// Pure-Rust line-for-line port of `BaseCsvReader.lua:parseCsvData` + `splitCsv` to verify
/// every emitted festival CSV obeys the 4-non-empty-line, RFC-4180 quote, and `id >= 1` rules.
pub fn validate_base_csv_reader(csv: &str) -> Result<usize, String> {
    if csv.len() < 2 {
        return Err("csv shorter than 2 bytes".into());
    }
    let lines: Vec<&str> = csv
        .split(['\n', '\r'])
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 4 {
        return Err(format!(
            "BaseCsvReader requires >= 4 non-empty lines, got {}",
            lines.len()
        ));
    }
    let col_names = split_csv_line(lines[1])?;
    let deal_funcs = split_csv_line(lines[2])?;
    if col_names.is_empty() || col_names.len() != deal_funcs.len() {
        return Err(format!(
            "column/dealFunc length mismatch: {} vs {}",
            col_names.len(),
            deal_funcs.len()
        ));
    }
    if col_names[0] != "id" {
        return Err(format!("first column must be 'id', got '{}'", col_names[0]));
    }
    for f in &deal_funcs {
        if !matches!(f.as_str(), "" | "0" | "array" | "array3") {
            return Err(format!("unsupported BaseCsvReader dealFunc '{f}'"));
        }
    }
    let mut row_count = 0usize;
    for (idx, &row_line) in lines.iter().enumerate().skip(3) {
        let cells = split_csv_line(row_line)?;
        if cells.len() != col_names.len() {
            return Err(format!(
                "line {} has {} cells, expected {}",
                idx + 1,
                cells.len(),
                col_names.len()
            ));
        }
        let row_id: i64 = cells[0]
            .parse()
            .map_err(|_| format!("line {} invalid numeric id '{}'", idx + 1, cells[0]))?;
        if row_id <= 0 {
            return Err(format!("line {} id must be > 0, got {row_id}", idx + 1));
        }
        row_count += 1;
    }
    Ok(row_count)
}

fn split_csv_line(line: &str) -> Result<Vec<String>, String> {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut in_quote = false;
    let mut cur = String::new();
    let mut out = Vec::new();
    while i < bytes.len() {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();
        if b == b',' && !in_quote {
            out.push(std::mem::take(&mut cur));
        } else if b == b'"' {
            if in_quote {
                if next == Some(b'"') {
                    cur.push('"');
                    i += 1;
                } else {
                    in_quote = false;
                }
            } else {
                in_quote = true;
            }
        } else {
            cur.push(b as char);
        }
        i += 1;
    }
    if in_quote {
        return Err("unclosed quote in CSV line".into());
    }
    out.push(cur);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_active_festival_csvs_pass_base_csv_reader_and_exclude_nil_proxies() {
        let now = 1_795_500_000u64;
        let dto = build_activity_conf_dto(now);
        assert_eq!(dto.get("version").and_then(DtoValue::as_u32), Some(1));
        let arr = dto
            .get("arrData")
            .and_then(DtoValue::as_map)
            .expect("arrData map");
        assert!(
            !arr.contains_key("activity_recharge_double"),
            "must never include nil-proxy activity_recharge_double"
        );
        assert_eq!(arr.len(), ACTIVE_FESTIVAL_KEYS.len());
        for &fid in ACTIVE_FESTIVAL_KEYS {
            let entry = arr.get(fid).unwrap_or_else(|| panic!("missing {fid}"));
            assert_eq!(
                entry.get("start_time").and_then(DtoValue::as_u64),
                Some(now - 86_400)
            );
            assert_eq!(
                entry.get("end_time").and_then(DtoValue::as_u64),
                Some(now + 90 * 86_400)
            );
            assert_eq!(
                entry.get("need_open_time").and_then(DtoValue::as_u64),
                Some(2_147_483_647)
            );
            let csv = entry
                .get("data")
                .and_then(DtoValue::as_str)
                .expect("csv string");
            let rows = validate_base_csv_reader(csv)
                .unwrap_or_else(|e| panic!("festival {fid} invalid CSV: {e}"));
            assert!(rows >= 1, "{fid} must contain at least 1 data row");
        }
    }
}
