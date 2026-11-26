use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gserver_player::{
    classify_template_id, crc32_ieee, Account, BagType, CurrencyType, EquipSlotType, ItemClass,
    JsonValue, PlayerError, PlayerProfile, PlayerStore, StarterHeroSeed, StarterItemSeed,
    StarterPackConfig, UseItemEffect, WalOp, WriteAheadLog,
};

static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempTestDir {
    path: PathBuf,
}

impl TempTestDir {
    fn new(label: &str) -> Self {
        let id = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("gserver_player_test_{label}_{pid}_{id}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp test dir");
        Self { path }
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_01_json_roundtrip_primitives_escapes_and_surrogate_pairs() {
    let raw = r#"{
        "null_v": null,
        "bool_t": true,
        "bool_f": false,
        "neg": -9876543210,
        "big_u64": 18446744073709551610,
        "float_v": 12.5e2,
        "escaped": "line1\nline2\t\"quote\"\\slash\/",
        "vietnamese": "Chúa Công Thiếu Niên 3Q",
        "emoji_surrogate": "\uD83D\uDE80"
    }"#;
    let parsed = JsonValue::parse(raw).expect("valid JSON");
    assert_eq!(parsed.get("null_v"), Some(&JsonValue::Null));
    assert_eq!(parsed.get("bool_t").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(parsed.get("bool_f").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        parsed.get("neg").and_then(|v| v.as_i64()),
        Some(-9_876_543_210)
    );
    assert_eq!(
        parsed.get("big_u64").and_then(|v| v.as_u64()),
        Some(18_446_744_073_709_551_610)
    );
    assert_eq!(parsed.get("float_v").and_then(|v| v.as_f64()), Some(1250.0));
    assert_eq!(
        parsed.get("emoji_surrogate").and_then(|v| v.as_str()),
        Some("🚀")
    );

    let compact = parsed.to_json_string();
    let reparsed = JsonValue::parse(&compact).expect("reparse compact");
    assert_eq!(parsed, reparsed);

    let pretty = parsed.to_json_pretty();
    let reparsed_pretty = JsonValue::parse(&pretty).expect("reparse pretty");
    assert_eq!(parsed, reparsed_pretty);
}

#[test]
fn test_02_json_rejects_malformed_inputs() {
    assert!(JsonValue::parse("").is_err());
    assert!(JsonValue::parse("01").is_err(), "leading zero must fail");
    assert!(
        JsonValue::parse(r#"{"a": 1} trailing"#).is_err(),
        "trailing tokens must fail"
    );
    assert!(
        JsonValue::parse(r#""\uD800""#).is_err(),
        "unpaired high surrogate must fail"
    );
    assert!(
        JsonValue::parse(r#""\uDC00""#).is_err(),
        "unpaired low surrogate must fail"
    );
    assert!(
        JsonValue::parse("{\"unclosed\": [1, 2").is_err(),
        "unclosed array must fail"
    );
}

#[test]
fn test_03_json_deterministic_key_sorting_and_depth_guard() {
    let mut m = BTreeMap::new();
    m.insert("z_key".to_string(), JsonValue::from(1u32));
    m.insert("a_key".to_string(), JsonValue::from(2u32));
    m.insert("m_key".to_string(), JsonValue::from(3u32));
    let s = JsonValue::Object(m).to_json_string();
    assert_eq!(s, r#"{"a_key":2,"m_key":3,"z_key":1}"#);

    let deep = "[".repeat(140) + &"]".repeat(140);
    assert!(
        JsonValue::parse(&deep).is_err(),
        "excessive nesting must be rejected cleanly"
    );
}

#[test]
fn test_04_crc32_ieee_standard_check_vector() {
    assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
    assert_ne!(crc32_ieee(b"123456789"), crc32_ieee(b"123456780"));
}

#[test]
fn test_05_starter_character_creation_male_and_female_utid_validation() {
    let cfg = StarterPackConfig::default();
    let male = PlayerProfile::new_character(
        52592,
        "pid_01",
        "2900161",
        1,
        "TriệuVân",
        1_700_000_000,
        &cfg,
    )
    .expect("male character");
    assert_eq!(male.utid, 1);
    assert_eq!(male.uname, "TriệuVân");
    assert!(male
        .heroes
        .get(&male.protagonist_hid)
        .unwrap()
        .is_protagonist());

    let female = PlayerProfile::new_character(
        52593,
        "pid_02",
        "2900161",
        2,
        "ĐiêuThuyền",
        1_700_000_000,
        &cfg,
    )
    .expect("female character");
    assert_eq!(female.utid, 2);

    // Invalid utid and invalid uname length
    assert!(PlayerProfile::new_character(
        52594,
        "pid_03",
        "2900161",
        999,
        "BadUtid",
        1_700_000_000,
        &cfg
    )
    .is_err());
    assert!(PlayerProfile::new_character(
        52595,
        "pid_04",
        "2900161",
        1,
        "   ",
        1_700_000_000,
        &cfg
    )
    .is_err());
    assert!(PlayerProfile::new_character(
        52596,
        "pid_05",
        "2900161",
        1,
        "NameExceedingFifteenChars",
        1_700_000_000,
        &cfg
    )
    .is_err());
}

#[test]
fn test_06_starter_pack_custom_vip_gold_silver_heroes_and_items() {
    let cfg = StarterPackConfig {
        initial_vip: 12,
        gold: 999_999,
        copper: 88_000_000,
        companion_heroes: vec![
            StarterHeroSeed {
                htid: 103, // Trương Liêu /橙将
                level: 10,
                evolve_level: 2,
                auto_deploy_slot: Some(2),
            },
            StarterHeroSeed {
                htid: 204, // Triệu Vân
                level: 10,
                evolve_level: 1,
                auto_deploy_slot: Some(3),
            },
        ],
        starter_items: vec![
            StarterItemSeed {
                item_template_id: 500_101, // GodArm
                count: 1,
            },
            StarterItemSeed {
                item_template_id: 400_101, // Pet
                count: 1,
            },
        ],
        ..StarterPackConfig::default()
    };

    let p = PlayerProfile::new_character(
        60001,
        "pid_vip",
        "2900161",
        1,
        "ChúaCôngBT",
        1_700_000_100,
        &cfg,
    )
    .expect("custom starter pack");
    assert_eq!(p.vip, 12);
    assert_eq!(p.currency(CurrencyType::Gold), 999_999);
    assert_eq!(p.currency(CurrencyType::Copper), 88_000_000);
    assert_eq!(p.heroes.len(), 3); // Protagonist + 2 companions
    assert_eq!(p.formation.position.len(), 3);
    assert_eq!(p.inventory.count_by_template(500_101), 1);
    assert_eq!(p.inventory.count_by_template(400_101), 1);
}

#[test]
fn test_07_vnsg2_get_user_wire_schema_invariants() {
    let cfg = StarterPackConfig::default();
    let p = PlayerProfile::new_character(
        52592,
        "636df6782106606d",
        "2900161",
        1,
        "Kaiaoaoa",
        1_795_500_450,
        &cfg,
    )
    .expect("create character");
    let wire = p.to_vnsg2_get_user();
    let obj = wire.as_object().expect("UserData object");

    // 1. `copper` MUST be a string for `tonumber(self.copper)` big-num safety, while `silver_num` is numeric
    assert_eq!(obj.get("copper").and_then(|v| v.as_str()), Some("5000000"));
    assert_eq!(
        obj.get("silver_num").and_then(|v| v.as_u64()),
        Some(5_000_000)
    );

    // 2. Arithmetic currencies & timers MUST be numbers (NOT strings!)
    assert_eq!(obj.get("gold").and_then(|v| v.as_u64()), Some(58_888));
    assert_eq!(obj.get("gold_num").and_then(|v| v.as_u64()), Some(58_888));
    assert_eq!(obj.get("gold_bind").and_then(|v| v.as_u64()), Some(18_888));
    assert_eq!(obj.get("hero_soul").and_then(|v| v.as_u64()), Some(20_000));
    assert_eq!(obj.get("soul_num").and_then(|v| v.as_u64()), Some(20_000));
    assert_eq!(obj.get("prestige").and_then(|v| v.as_u64()), Some(10_000));
    assert_eq!(obj.get("execution").and_then(|v| v.as_u64()), Some(120));
    assert_eq!(
        obj.get("execution_time").and_then(|v| v.as_u64()),
        Some(1_795_500_450)
    );
    assert_eq!(obj.get("energy").and_then(|v| v.as_u64()), Some(50));
    assert_eq!(obj.get("stamina").and_then(|v| v.as_u64()), Some(50));
    assert_eq!(
        obj.get("hero_bag_buy_times").and_then(|v| v.as_u64()),
        Some(0)
    );

    // 3. `server_open_time` MUST be 14-char string (`string.sub(self.server_open_time, 3, 14)`)
    let sot = obj
        .get("server_open_time")
        .and_then(|v| v.as_str())
        .expect("server_open_time string");
    assert_eq!(sot.len(), 14);

    // 4. `attr_dress_id` MUST exist in `dress_info` (`self.dress_info[self.attr_dress_id] - 1`)
    let attr_dress_id = obj
        .get("attr_dress_id")
        .and_then(|v| v.as_u64())
        .expect("attr_dress_id");
    let dress_info = obj
        .get("dress_info")
        .and_then(|v| v.as_object())
        .expect("dress_info object");
    assert!(
        dress_info
            .get(&attr_dress_id.to_string())
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 1
    );
    assert!(obj
        .get("dress_expire")
        .and_then(|v| v.as_object())
        .is_some());
    assert!(obj
        .get("dress_hero_info")
        .and_then(|v| v.as_array())
        .is_some());
    assert_eq!(obj.get("pet_close").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        obj.get("godArm_close").and_then(|v| v.as_bool()),
        Some(false)
    );
}

#[test]
fn test_08_vnsg2_user_before_login_and_character_summary_schemas() {
    let cfg = StarterPackConfig::default();
    let p = PlayerProfile::new_character(
        52592,
        "pid_a",
        "2900161",
        2,
        "TiểuKiều",
        1_700_000_000,
        &cfg,
    )
    .unwrap();

    let before = p.to_vnsg2_user_before_login();
    assert_eq!(before.get("uid").and_then(|v| v.as_u64()), Some(52592));

    let summary = p.to_character_summary();
    assert_eq!(summary.get("uid").and_then(|v| v.as_u64()), Some(52592));
    assert_eq!(
        summary.get("uname").and_then(|v| v.as_str()),
        Some("TiểuKiều")
    );
    assert_eq!(summary.get("utid").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(
        summary.get("groupid").and_then(|v| v.as_str()),
        Some("2900161")
    );
}

#[test]
fn test_09_vnsg2_hero_info_wire_schema_non_nil_tables() {
    let cfg = StarterPackConfig::default();
    let p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "QuanVũ", 1_700_000_000, &cfg)
            .unwrap();
    let hero = p.heroes.get(&p.protagonist_hid).unwrap();
    let wire = hero.to_vnsg2_hero_info();
    let obj = wire.as_object().unwrap();

    assert_eq!(
        obj.get("hid").and_then(|v| v.as_u64()),
        Some(p.protagonist_hid)
    );
    assert_eq!(obj.get("htid").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(obj.get("level").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(obj.get("evolve_level").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("hero_star_level").and_then(|v| v.as_u64()), Some(1));

    // Mandatory non-nil tables (`HeroInfo:cloneTable(data["va_fail_reward"])` crashes if nil!)
    for table_key in [
        "va_fail_reward",
        "va_use_wash",
        "va_last_wash",
        "va_wash_count",
        "va_inlay",
        "va_color_gold_gift",
        "va_nirvana_gold_gift",
        "va_gild_gold_gift",
        "va_iron_gold_gift",
    ] {
        assert!(
            obj.get(table_key).and_then(|v| v.as_object()).is_some(),
            "HeroInfo.{table_key} must be non-nil table"
        );
    }
}

#[test]
fn test_10_vnsg2_formation_position_info_no_zero_hids_and_non_nil_subtables() {
    let cfg = StarterPackConfig::default();
    let p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "LưuBị", 1_700_000_000, &cfg)
            .unwrap();
    let pos_info = p.formation.to_vnsg2_position_info(&p.heroes);
    let obj = pos_info.as_object().unwrap();

    // `position` MUST contain ONLY occupied slots with hid > 0 present in allHeros!
    let pos_map = obj.get("position").and_then(|v| v.as_object()).unwrap();
    let all_heros = obj.get("allHeros").and_then(|v| v.as_object()).unwrap();
    assert_eq!(pos_map.len(), 2); // Protagonist (slot 1) + companion 426 (slot 2)
    for hid_val in pos_map.values() {
        let hid = hid_val.as_u64().unwrap();
        assert!(hid > 0, "position must never contain hid == 0");
        assert!(
            all_heros.contains_key(&hid.to_string()),
            "every hid in position must exist in allHeros"
        );
    }

    assert_eq!(
        obj.get("arming").and_then(|v| v.as_array()).unwrap().len(),
        6
    );
    for required_table in [
        "partner",
        "buddy",
        "buddyLevelInfo",
        "bingfu",
        "bingshu",
        "jobSkill",
        "arrCounsellor",
        "godanimal",
        "topanimal",
        "formationBook",
        "attrSkins",
        "protection",
    ] {
        assert!(
            obj.get(required_table)
                .and_then(|v| v.as_object())
                .is_some(),
            "HeroFormationData.{required_table} must be non-nil object"
        );
    }
    assert!(obj.get("jointSkill").and_then(|v| v.as_array()).is_some());
    assert_eq!(
        obj.get("squad").and_then(|v| v.as_array()).unwrap().len(),
        6
    );

    let f_get = p.formation.to_vnsg2_formation_get();
    let grid = f_get.get("formation").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        grid.get("3").and_then(|v| v.as_u64()),
        Some(p.protagonist_hid)
    );
}

#[test]
fn test_11_vnsg2_bag_info_all_42_subbags_daily_use_and_grid_metadata_non_nil() {
    let cfg = StarterPackConfig::default();
    let p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "TàoTháo", 1_700_000_000, &cfg)
            .unwrap();
    let bag_info = p.inventory.to_vnsg2_bag_info();
    let obj = bag_info.as_object().unwrap();

    assert!(obj.get("daily_use").and_then(|v| v.as_object()).is_some());
    let gs = obj.get("gridStart").and_then(|v| v.as_object()).unwrap();
    let gm = obj.get("gridMaxNum").and_then(|v| v.as_object()).unwrap();
    let go = obj
        .get("gridOpenTimes")
        .and_then(|v| v.as_object())
        .unwrap();

    for bt in BagType::ALL {
        let key = bt.vnsg2_key();
        assert!(
            obj.get(key).and_then(|v| v.as_object()).is_some(),
            "missing sub-bag '{key}'"
        );
        assert_eq!(
            gs.get(key).and_then(|v| v.as_u64()),
            Some(u64::from(bt.grid_start()))
        );
        assert!(gm.get(key).and_then(|v| v.as_u64()).unwrap() >= 100);
        assert_eq!(go.get(key).and_then(|v| v.as_u64()), Some(0));
    }

    // Classic sub-bag compatibility keys
    for alias in [
        "props", "heroFrag", "treas", "armFrag", "godarm", "dress", "rune",
    ] {
        assert!(
            obj.get(alias).and_then(|v| v.as_object()).is_some(),
            "missing classic bag alias '{alias}'"
        );
    }
}

#[test]
fn test_12_item_factory_template_id_classification_across_all_categories() {
    assert_eq!(
        classify_template_id(10_001).unwrap(),
        (ItemClass::Normal, BagType::Item)
    );
    assert_eq!(
        classify_template_id(20_500).unwrap(),
        (ItemClass::Direct, BagType::Item)
    );
    assert_eq!(
        classify_template_id(30_100).unwrap(),
        (ItemClass::RandGift, BagType::Item)
    );
    assert_eq!(
        classify_template_id(50_201).unwrap(),
        (ItemClass::HeroFragment, BagType::Soul)
    );
    assert_eq!(
        classify_template_id(101_101).unwrap(),
        (ItemClass::Arm, BagType::Arm)
    );
    assert_eq!(
        classify_template_id(185_001).unwrap(),
        (ItemClass::Treasure, BagType::Treasure)
    );
    assert_eq!(
        classify_template_id(305_001).unwrap(),
        (ItemClass::Awake, BagType::Awake)
    );
    assert_eq!(
        classify_template_id(405_001).unwrap(),
        (ItemClass::Pet, BagType::Pet)
    );
    assert_eq!(
        classify_template_id(5_100_001).unwrap(),
        (ItemClass::Pet, BagType::Pet)
    );
    assert_eq!(
        classify_template_id(505_001).unwrap(),
        (ItemClass::GodArm, BagType::GodArm)
    );
    assert_eq!(
        classify_template_id(400_000_001).unwrap(),
        (ItemClass::TreasureFragment, BagType::TreasureFragment)
    );
    assert!(classify_template_id(999_999_999).is_err());
}

#[test]
fn test_13_item_class_va_item_text_defaults_arm_treasure_awake_pet_godarm() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p = PlayerProfile::new_character(
        52592,
        "pid_a",
        "2900161",
        1,
        "TônQuyền",
        1_700_000_000,
        &cfg,
    )
    .unwrap();

    let arm = &p.add_item(101_101, 1, 100).unwrap()[0];
    assert_eq!(
        arm.va_item_text
            .get("reinforce_level")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        arm.va_item_text
            .get("refineryLock")
            .and_then(|v| v.as_bool()),
        Some(false)
    );

    let pet = &p.add_item(400_101, 1, 100).unwrap()[0];
    assert_eq!(
        pet.va_item_text
            .get("progress_level")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        pet.va_item_text.get("break_level").and_then(|v| v.as_u64()),
        Some(0)
    );

    let god = &p.add_item(500_101, 1, 100).unwrap()[0];
    assert!(god
        .va_item_text
        .get("godArm_wash")
        .unwrap()
        .as_object()
        .is_some());
    assert!(god
        .va_item_text
        .get("godArm_va_gem")
        .unwrap()
        .as_object()
        .is_some());
}

#[test]
fn test_14_currency_grant_consume_and_insufficient_balance_atomicity() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "LữBố", 1_700_000_000, &cfg)
            .unwrap();
    let start_gold = p.currency(CurrencyType::Gold);
    assert_eq!(
        p.grant_currency(CurrencyType::Gold, 1_112, 1_700_000_010),
        start_gold + 1_112
    );
    assert_eq!(
        p.consume_currency(CurrencyType::Gold, 10_000, 1_700_000_020)
            .unwrap(),
        start_gold + 1_112 - 10_000
    );

    let rev_before = p.revision;
    let err = p
        .consume_currency(CurrencyType::Gold, 999_999_999, 1_700_000_030)
        .unwrap_err();
    assert!(matches!(err, PlayerError::InsufficientCurrency { .. }));
    assert_eq!(p.revision, rev_before);
}

#[test]
fn test_15_stamina_and_energy_natural_time_recovery_and_caps() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "ChuDu", 1_700_000_000, &cfg)
            .unwrap();
    p.consume_currency(CurrencyType::Execution, 100, 1_700_000_000)
        .unwrap(); // 20 left
    p.consume_currency(CurrencyType::Energy, 30, 1_700_000_000)
        .unwrap(); // 20 left

    // Advance 1800 seconds => +5 execution (1800/360) and +2 energy (1800/900)
    p.tick_stamina_recovery(1_700_001_800);
    assert_eq!(p.currency(CurrencyType::Execution), 25);
    assert_eq!(p.currency(CurrencyType::Energy), 22);

    // Advance 10 days => caps cleanly at 120 execution and 50 energy
    p.tick_stamina_recovery(1_700_864_000);
    assert_eq!(p.currency(CurrencyType::Execution), 120);
    assert_eq!(p.currency(CurrencyType::Energy), 50);
}

#[test]
fn test_16_player_exp_level_up_syncs_protagonist_hero_level_and_fight_force() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "GiaCát", 1_700_000_000, &cfg)
            .unwrap();
    let fc_before = p.fc;
    // Lv1 -> Lv2 needs 100, Lv2 -> Lv3 needs 200 => 350 EXP reaches Lv3 with 50 leftover EXP
    let (old_lv, new_lv) = p.add_player_exp(350);
    assert_eq!((old_lv, new_lv), (1, 3));
    assert_eq!(p.exp, 50);
    let protag = p.heroes.get(&p.protagonist_hid).unwrap();
    assert_eq!(protag.level, 3);
    assert_eq!(protag.exp, 50);
    assert!(p.fc > fc_before);
}

#[test]
fn test_17_bag_add_stackable_vs_non_stackable_equipment_unique_instances() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "TưMãÝ", 1_700_000_000, &cfg)
            .unwrap();

    // Stackable prop (10001..20000) merges into a single gid (30001)
    let b1 = p.add_item(10_005, 5, 100).unwrap();
    let b2 = p.add_item(10_005, 7, 105).unwrap();
    assert_eq!(b1[0].gid, 30_001);
    assert_eq!(b2[0].gid, 30_001);
    assert_eq!(b2[0].item_num, 12);

    // Non-stackable equipment (100001..180000) creates 3 distinct grids (20001, 20002, 20003)
    let arms = p.add_item(101_201, 3, 110).unwrap();
    assert_eq!(arms.len(), 3);
    assert_eq!(arms[0].gid, 20_001);
    assert_eq!(arms[1].gid, 20_002);
    assert_eq!(arms[2].gid, 20_003);
    assert_ne!(arms[0].item_id, arms[1].item_id);
}

#[test]
fn test_18_bag_remove_by_gid_and_by_template_atomic_rollback_on_shortfall() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p = PlayerProfile::new_character(
        52592,
        "pid_a",
        "2900161",
        1,
        "HoàngTrung",
        1_700_000_000,
        &cfg,
    )
    .unwrap();

    p.add_item(101_301, 3, 100).unwrap();
    assert_eq!(p.inventory.count_by_template(101_301), 3);

    // Asking to remove 4 when only 3 exist must fail without removing any of the 3!
    let err = p.remove_item_by_template(101_301, 4).unwrap_err();
    assert!(matches!(err, PlayerError::InsufficientItemCount { .. }));
    assert_eq!(p.inventory.count_by_template(101_301), 3);

    let removed = p.remove_item_by_template(101_301, 2).unwrap();
    assert_eq!(removed.len(), 2);
    assert_eq!(p.inventory.count_by_template(101_301), 1);
}

#[test]
fn test_19_bag_capacity_enforcement_and_grid_expansion() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "MãSiêu", 1_700_000_000, &cfg)
            .unwrap();

    // Set Awake bag capacity to 2 for fast boundary verification
    p.inventory.grid_max_num.insert(BagType::Awake, 2);
    p.add_item(300_101, 2, 100).unwrap();
    let err = p.add_item(300_101, 1, 100).unwrap_err();
    assert!(matches!(err, PlayerError::BagFull { .. }));

    // Expand by 5 grids (`bag.db`: Awake buy_storage = 5)
    p.inventory.expand_bag_grids(BagType::Awake, 5, 1);
    let added = p.add_item(300_101, 3, 100).unwrap();
    assert_eq!(added.len(), 3);
}

#[test]
fn test_20_use_item_effects_currency_exp_summon_giftbox_and_daily_use_counter() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p = PlayerProfile::new_character(
        52592,
        "pid_a",
        "2900161",
        1,
        "BàngThống",
        1_700_000_000,
        &cfg,
    )
    .unwrap();

    // 1. Direct gold card
    let card_gid = p.add_item(20_101, 5, 100).unwrap()[0].gid;
    let out1 = p
        .use_item(
            card_gid,
            2,
            UseItemEffect::GrantCurrency {
                currency: CurrencyType::Gold,
                amount_per_item: 500,
            },
            110,
        )
        .unwrap();
    assert_eq!(out1.remaining_in_grid, 3);
    assert_eq!(out1.daily_used_total, 2);
    assert_eq!(p.currency(CurrencyType::Gold), cfg.gold + 1_000);

    // 2. Hero summon scroll
    let scroll_gid = p.add_item(30_201, 1, 110).unwrap()[0].gid;
    let out2 = p
        .use_item(scroll_gid, 1, UseItemEffect::SummonHero { htid: 304 }, 120)
        .unwrap();
    assert_eq!(out2.summoned_hids.len(), 1);
    assert!(p.heroes.contains_key(&out2.summoned_hids[0]));

    // 3. Gift box granting multiple items
    let box_gid = p.add_item(30_301, 2, 120).unwrap()[0].gid;
    let out3 = p
        .use_item(
            box_gid,
            2,
            UseItemEffect::GrantItems {
                items_per_use: vec![StarterItemSeed {
                    item_template_id: 50_105,
                    count: 10,
                }],
            },
            130,
        )
        .unwrap();
    assert_eq!(out3.remaining_in_grid, 0);
    assert_eq!(p.inventory.count_by_template(50_105), 20);
}

#[test]
fn test_21_summon_hero_and_hero_bag_capacity_enforcement() {
    let mut cfg = StarterPackConfig::default();
    cfg.companion_heroes.clear();
    let mut p = PlayerProfile::new_character(
        52592,
        "pid_a",
        "2900161",
        1,
        "TrươngPhi",
        1_700_000_000,
        &cfg,
    )
    .unwrap();

    // Protagonist template IDs 1..=28 cannot be summoned as duplicates
    assert!(p.summon_hero(1, 1).is_err());
    assert!(p.summon_hero(2, 1).is_err());

    // Base capacity is 200 (1 protagonist + 199 summons)
    for i in 0..199 {
        p.summon_hero(100 + i, 1).unwrap();
    }
    assert_eq!(p.heroes.len(), 200);
    assert!(matches!(
        p.summon_hero(999, 1).unwrap_err(),
        PlayerError::HeroBagFull { max_heroes: 200 }
    ));

    // Buy 1 hero bag expansion (+10 slots)
    p.hero_bag_buy_times = 1;
    let hid = p.summon_hero(999, 1).unwrap();
    assert!(p.heroes.contains_key(&hid));
}

#[test]
fn test_22_level_up_hero_consumes_copper_and_soul_and_rejects_protagonist() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "CamNinh", 1_700_000_000, &cfg)
            .unwrap();

    // Protagonist cannot be leveled via hero.heroLevelUp
    assert!(p
        .level_up_hero(p.protagonist_hid, 5, 100, 100, 100)
        .is_err());

    let comp_hid = p.formation.position.get(&2).copied().unwrap();
    let fc_before = p.fc;
    let res = p.level_up_hero(comp_hid, 9, 5_000, 1_000, 100).unwrap();
    assert_eq!((res.old_level, res.new_level), (1, 10));
    assert_eq!(p.currency(CurrencyType::Copper), cfg.copper - 5_000);
    assert_eq!(p.currency(CurrencyType::HeroSoul), cfg.hero_soul - 1_000);
    assert!(p.fc > fc_before);
}

#[test]
fn test_23_evolve_and_upgrade_hero_with_duplicate_sacrifice_and_safety_guards() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "LữMông", 1_700_000_000, &cfg)
            .unwrap();

    let main_gen = p.summon_hero(107, 20).unwrap();
    let fodder = p.summon_hero(107, 1).unwrap();

    // Locked fodder must be rejected
    p.set_hero_lock(fodder, true).unwrap();
    assert!(p.evolve_hero(main_gen, &[fodder], 1_000, 100).is_err());

    // Protagonist sacrifice must be rejected
    assert!(matches!(
        p.evolve_hero(main_gen, &[p.protagonist_hid], 1_000, 100)
            .unwrap_err(),
        PlayerError::ProtagonistCannotBeRemoved { .. }
    ));

    // Unlock fodder -> evolve succeeds and removes fodder
    p.set_hero_lock(fodder, false).unwrap();
    let ev = p.evolve_hero(main_gen, &[fodder], 10_000, 100).unwrap();
    assert_eq!(ev, 1);
    assert!(!p.heroes.contains_key(&fodder));

    let up = p.upgrade_hero(main_gen, 5_000, 110).unwrap();
    assert_eq!(up, 1);
}

#[test]
fn test_24_squad_set_unset_swap_and_protagonist_irremovable_invariant() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "TônSách", 1_700_000_000, &cfg)
            .unwrap();

    // Protagonist in slot 1 cannot be unset or overwritten by an undeployed hero
    assert!(matches!(
        p.unset_squad_slot(1).unwrap_err(),
        PlayerError::ProtagonistCannotBeRemoved { .. }
    ));
    let h3 = p.summon_hero(301, 10).unwrap();
    assert!(matches!(
        p.set_squad_slot(1, h3).unwrap_err(),
        PlayerError::ProtagonistCannotBeRemoved { .. }
    ));

    // Protagonist CAN swap slots (e.g. slot 1 <-> slot 2)
    p.swap_squad_slots(1, 2).unwrap();
    assert_eq!(p.formation.position.get(&2), Some(&p.protagonist_hid));

    // Deploy h3 into slot 3, then unset slot 3
    p.set_squad_slot(3, h3).unwrap();
    assert_eq!(p.formation.position.get(&3), Some(&h3));
    assert_eq!(p.unset_squad_slot(3).unwrap(), h3);
    assert!(!p.formation.position.contains_key(&3));
}

#[test]
fn test_25_squad_and_partner_reject_duplicate_hero_template_htid() {
    let cfg = StarterPackConfig::default();
    let mut p = PlayerProfile::new_character(
        52592,
        "pid_a",
        "2900161",
        1,
        "QuáchGia",
        1_700_000_000,
        &cfg,
    )
    .unwrap();

    let dup_426 = p.summon_hero(426, 1).unwrap(); // 426 is already in squad slot 2!
    assert!(matches!(
        p.set_squad_slot(3, dup_426).unwrap_err(),
        PlayerError::DuplicateHeroTemplateInSquad { htid: 426 }
    ));
    assert!(matches!(
        p.set_partner_slot(1, dup_426).unwrap_err(),
        PlayerError::DuplicateHeroTemplateInSquad { htid: 426 }
    ));

    let unique_partner = p.summon_hero(205, 1).unwrap();
    p.open_partner_slot(2).unwrap();
    p.set_partner_slot(2, unique_partner).unwrap();
    assert_eq!(p.formation.partner.get(&2), Some(&unique_partner));
}

#[test]
fn test_26_battle_grid_9_cell_formation_sync_and_custom_layout_validation() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "TuânÚc", 1_700_000_000, &cfg)
            .unwrap();
    let comp_hid = p.formation.position.get(&2).copied().unwrap();

    // Move protagonist to cell 1 and companion to cell 9
    let mut custom_grid = BTreeMap::new();
    custom_grid.insert(1, p.protagonist_hid);
    custom_grid.insert(9, comp_hid);
    p.set_grid_formation(custom_grid.clone()).unwrap();
    assert_eq!(p.formation.grid_formation, custom_grid);

    // Omitting an active squad hero or using out-of-range cell 10 must fail
    let mut bad_grid = BTreeMap::new();
    bad_grid.insert(1, p.protagonist_hid);
    assert!(p.set_grid_formation(bad_grid).is_err());

    let mut oob_grid = custom_grid;
    oob_grid.remove(&9);
    oob_grid.insert(10, comp_hid);
    assert!(p.set_grid_formation(oob_grid).is_err());
}

#[test]
fn test_27_equip_swap_and_unequip_items_across_slots() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "ĐiểnVi", 1_700_000_000, &cfg)
            .unwrap();

    let w1 = p.add_item(101_101, 1, 100).unwrap()[0].clone();
    let w2 = p.add_item(101_102, 1, 100).unwrap()[0].clone();
    let tr = p.add_item(180_101, 1, 100).unwrap()[0].clone();
    let pet = p.add_item(400_101, 1, 100).unwrap()[0].clone();
    let god = p.add_item(500_101, 1, 100).unwrap()[0].clone();

    // Type mismatch: Treasure cannot go into Weapon slot 1
    assert!(matches!(
        p.equip_item(1, EquipSlotType::Weapon, tr.gid).unwrap_err(),
        PlayerError::EquipTypeMismatch { .. }
    ));

    // Equip w1 -> Weapon, tr -> AttackTreasure, pet -> Pet, god -> GodArm
    assert!(p
        .equip_item(1, EquipSlotType::Weapon, w1.gid)
        .unwrap()
        .is_none());
    assert!(p
        .equip_item(1, EquipSlotType::AttackTreasure, tr.gid)
        .unwrap()
        .is_none());
    assert!(p
        .equip_item(1, EquipSlotType::Pet, pet.gid)
        .unwrap()
        .is_none());
    assert!(p
        .equip_item(1, EquipSlotType::GodArm, god.gid)
        .unwrap()
        .is_none());

    // Equipping w2 into Weapon slot swaps w1 back into the Arm bag!
    let swapped = p
        .equip_item(1, EquipSlotType::Weapon, w2.gid)
        .unwrap()
        .expect("previous weapon returned to bag");
    assert_eq!(swapped.item_id, w1.item_id);
    assert_eq!(p.inventory.count_by_template(101_101), 1);
    assert_eq!(p.inventory.count_by_template(101_102), 0);

    // Unequip w2 back to bag
    let unequipped = p.unequip_item(1, EquipSlotType::Weapon).unwrap();
    assert_eq!(unequipped.item_id, w2.item_id);
    assert_eq!(p.inventory.count_by_template(101_102), 1);
}

#[test]
fn test_28_user_va_config_set_update_remove_and_wire_projection() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "HứaChử", 1_700_000_000, &cfg)
            .unwrap();

    p.set_user_config("music_mute", JsonValue::from(true))
        .unwrap();
    p.set_user_config("battle_speed", JsonValue::from(3u32))
        .unwrap();
    let mut guide_step = BTreeMap::new();
    guide_step.insert("step".into(), JsonValue::from(42u32));
    p.set_user_config("guide_state", JsonValue::Object(guide_step))
        .unwrap();

    let va = p.to_vnsg2_va_config();
    assert_eq!(va.get("music_mute").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(va.get("battle_speed").and_then(|v| v.as_u64()), Some(3));
    assert_eq!(
        va.get("guide_state")
            .and_then(|v| v.get("step"))
            .and_then(|v| v.as_u64()),
        Some(42)
    );

    assert!(p.remove_user_config("music_mute").is_some());
    assert!(p.to_vnsg2_va_config().get("music_mute").is_none());
}

#[test]
fn test_29_store_end_to_end_persistence_reload_accounts_and_players_json() {
    let dir = TempTestDir::new("e2e_persist");
    let data_dir = dir.path.join("data");

    let created_uid = {
        let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
        store
            .register_account(
                "636df6782106606d",
                "124383197",
                "kaiaoaoa",
                "tok_xyz",
                1_700_000_000,
            )
            .unwrap();
        assert_eq!(
            store
                .get_user_before_login("636df6782106606d", "2900161")
                .unwrap(),
            JsonValue::empty_object()
        );

        let player = store
            .create_character("636df6782106606d", "2900161", 1, "Kaiaoaoa", 1_700_000_010)
            .unwrap();
        store
            .mutate_player(player.uid, 1_700_000_020, |p| {
                p.grant_currency(CurrencyType::Gold, 10_000, 1_700_000_020);
                p.set_user_config("skip_anim", JsonValue::from(1u32))?;
                Ok(())
            })
            .unwrap();

        assert!(data_dir.join("accounts.json").exists());
        assert!(data_dir
            .join("players")
            .join(format!("{}.json", player.uid))
            .exists());
        player.uid
    };

    // Reopen store from disk and verify exact state
    let reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    let loaded = reopened.get_player(created_uid).expect("reloaded player");
    assert_eq!(loaded.uname, "Kaiaoaoa");
    assert_eq!(loaded.currency(CurrencyType::Gold), 68_888);
    assert_eq!(
        loaded.va_config.get("skip_anim").and_then(|v| v.as_u64()),
        Some(1)
    );
    let summaries = reopened
        .list_character_summaries("636df6782106606d")
        .unwrap();
    assert_eq!(summaries.len(), 1);
}

#[test]
fn test_30_store_transactional_rollback_leaves_disk_and_memory_untouched_on_error() {
    let dir = TempTestDir::new("tx_rollback");
    let data_dir = dir.path.join("data");
    let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    store
        .register_account("pid_tx", "qid_1", "tx_user", "tok", 1_700_000_000)
        .unwrap();
    let player = store
        .create_character("pid_tx", "2900161", 1, "TxHero", 1_700_000_001)
        .unwrap();

    let gold_before = player.currency(CurrencyType::Gold);
    let err = store
        .mutate_player(player.uid, 1_700_000_010, |p| {
            // Mutate first, then fail midway inside closure!
            p.grant_currency(CurrencyType::Gold, 50_000, 1_700_000_010);
            p.consume_currency(CurrencyType::Copper, 999_999_999_999, 1_700_000_010)?;
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(err, PlayerError::InsufficientCurrency { .. }));

    // Neither memory nor on-disk snapshot may observe the uncommitted +50,000 gold!
    assert_eq!(
        store
            .get_player(player.uid)
            .unwrap()
            .currency(CurrencyType::Gold),
        gold_before
    );
    let reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    assert_eq!(
        reopened
            .get_player(player.uid)
            .unwrap()
            .currency(CurrencyType::Gold),
        gold_before
    );
}

#[test]
fn test_31_crash_recovery_replays_committed_wal_when_player_snapshot_missed_write() {
    let dir = TempTestDir::new("wal_replay");
    let data_dir = dir.path.join("data");

    let uid = {
        let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
        store
            .register_account("pid_wal", "qid_wal", "wal_user", "tok", 1_700_000_000)
            .unwrap();
        let p = store
            .create_character("pid_wal", "2900161", 1, "WalGeneral", 1_700_000_005)
            .unwrap();
        p.uid
    };

    // Simulate a crash AFTER WAL append + sync_data, BEFORE `players/<uid>.json` atomic rename!
    {
        let stale_disk_txt =
            fs::read_to_string(data_dir.join("players").join(format!("{uid}.json"))).unwrap();
        let mut advanced_player =
            PlayerProfile::from_storage_json(&JsonValue::parse(&stale_disk_txt).unwrap()).unwrap();
        advanced_player.grant_currency(CurrencyType::Gold, 777_000, 1_700_000_999);
        let new_hid = advanced_player.summon_hero(207, 50).unwrap();
        assert!(new_hid > 0);

        let wal_path = data_dir.join("wal").join("operations.wal");
        let (mut wal, _) = WriteAheadLog::open_and_recover(&wal_path).unwrap();
        wal.append(
            1_700_000_999,
            WalOp::UpsertPlayer(Box::new(advanced_player)),
        )
        .unwrap();
        // Deliberately do NOT write `players/<uid>.json` (simulates power cut right before rename)!
    }

    // Reopen store: WAL replay must advance the player snapshot and persist the recovered file!
    let recovered = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    assert!(recovered.last_recovery_report().replayed_wal_players >= 1);
    let p = recovered.get_player(uid).unwrap();
    assert_eq!(p.currency(CurrencyType::Gold), 58_888 + 777_000);
    assert!(p.heroes.values().any(|h| h.htid == 207 && h.level == 50));
}

#[test]
fn test_32_crash_recovery_truncates_torn_wal_tail_and_cleans_orphan_tmp_files() {
    let dir = TempTestDir::new("torn_wal");
    let data_dir = dir.path.join("data");

    let uid = {
        let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
        store
            .register_account("pid_torn", "qid_torn", "torn_user", "tok", 1_700_000_000)
            .unwrap();
        store
            .create_character("pid_torn", "2900161", 2, "TornTest", 1_700_000_010)
            .unwrap()
            .uid
    };

    // Leave orphaned `.tmp.` files in `data/` and `data/players/`, and append a half-written torn JSON line to WAL
    let orphan_a = data_dir.join(".accounts.json.tmp.9999.1");
    let orphan_b = data_dir
        .join("players")
        .join(format!(".{uid}.json.tmp.9999.2"));
    fs::write(&orphan_a, b"{\"partial_garbage\":").unwrap();
    fs::write(&orphan_b, b"{\"partial_player\":").unwrap();

    let wal_path = data_dir.join("wal").join("operations.wal");
    {
        let mut wf = OpenOptions::new().append(true).open(&wal_path).unwrap();
        wf.write_all(b"{\"seq\":999,\"ts\":1700000999,\"op\":\"upsert_player\",\"crc32\":123")
            .unwrap();
        wf.sync_all().unwrap();
    }

    let reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    let rep = reopened.last_recovery_report();
    assert_eq!(rep.cleaned_orphan_tmp_files, 2);
    assert!(rep.truncated_corrupt_wal_bytes > 0);
    assert!(!orphan_a.exists());
    assert!(!orphan_b.exists());
    assert_eq!(reopened.get_player(uid).unwrap().uname, "TornTest");
}

#[test]
fn test_33_wal_checkpoint_compaction_and_subsequent_mutations() {
    let dir = TempTestDir::new("wal_ckpt");
    let data_dir = dir.path.join("data");
    let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    store
        .register_account("pid_cp", "qid_cp", "cp_user", "tok", 1_700_000_000)
        .unwrap();
    let p = store
        .create_character("pid_cp", "2900161", 1, "CheckpointHero", 1_700_000_001)
        .unwrap();

    for i in 0..10 {
        store
            .mutate_player(p.uid, 1_700_000_010 + i, |pl| {
                pl.grant_currency(CurrencyType::Gold, 100, 1_700_000_010 + i);
                Ok(())
            })
            .unwrap();
    }

    let wal_path = data_dir.join("wal").join("operations.wal");
    let size_before = fs::metadata(&wal_path).unwrap().len();
    let cp_seq = store.checkpoint(1_700_000_500).unwrap();
    assert!(cp_seq >= 12);
    let size_after = fs::metadata(&wal_path).unwrap().len();
    assert!(
        size_after < size_before,
        "checkpoint must compact multi-record WAL"
    );

    // Subsequent mutations after checkpoint must continue cleanly
    store
        .mutate_player(p.uid, 1_700_000_600, |pl| {
            pl.grant_currency(CurrencyType::Gold, 2_000, 1_700_000_600);
            Ok(())
        })
        .unwrap();

    drop(store);
    let reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    assert_eq!(
        reopened
            .get_player(p.uid)
            .unwrap()
            .currency(CurrencyType::Gold),
        58_888 + 1_000 + 2_000
    );
}

#[test]
fn test_34_user_login_ok_and_banned_account_response_format() {
    let dir = TempTestDir::new("login_ban");
    let data_dir = dir.path.join("data");
    let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    store
        .register_account("pid_ban", "qid_ban", "ban_user", "tok", 1_700_000_000)
        .unwrap();
    let p = store
        .create_character("pid_ban", "2900161", 1, "BanCheck", 1_700_000_005)
        .unwrap();

    assert_eq!(store.record_user_login(p.uid, 1_700_000_100).unwrap(), "ok");

    // Simulate banned account in accounts.json (`LoginProxy.lua` expects `"ban:<ts>:<reason>"`)
    let acc_txt = fs::read_to_string(store.accounts_path()).unwrap();
    let mut acc_json = JsonValue::parse(&acc_txt).unwrap();
    {
        let accounts_obj = acc_json
            .as_object_mut()
            .unwrap()
            .get_mut("accounts")
            .unwrap()
            .as_object_mut()
            .unwrap();
        let entry = accounts_obj
            .get_mut("pid_ban")
            .unwrap()
            .as_object_mut()
            .unwrap();
        entry.insert("banned_until".into(), JsonValue::from(1_800_000_000u64));
        entry.insert("ban_reason".into(), JsonValue::from("speedhack"));
    }
    let accounts_path = store.accounts_path().to_path_buf();
    drop(store);
    fs::write(&accounts_path, acc_json.to_json_pretty()).unwrap();
    // Clear WAL after dropping open store handle so Windows DeleteFileW succeeds
    fs::remove_file(data_dir.join("wal").join("operations.wal")).unwrap();

    let mut reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    let status = reopened.record_user_login(p.uid, 1_700_000_200).unwrap();
    assert_eq!(status, "ban:1800000000:speedhack");

    // Verify Account::from_json roundtrip
    let acc_ref: &Account = reopened.get_account("pid_ban").unwrap();
    assert!(acc_ref.is_banned(1_700_000_200));
    assert!(!acc_ref.is_banned(1_900_000_000));
}

#[test]
fn test_35_player_hero_instance_authentic_battle_skill_resolution() {
    let cfg = StarterPackConfig::default();
    let mut profile = PlayerProfile::new_character(
        60_001,
        "pid_sk",
        "2900161",
        1,
        "SkillHero",
        1_700_000_000,
        &cfg,
    )
    .expect("create character");

    // Starter protagonist (htid=1) -> (102, 101)
    let prot = profile.heroes.get(&profile.protagonist_hid).unwrap();
    assert_eq!(prot.battle_skill_ids(), (102, 101));
    assert!(!prot.resolved_battle_skills().used_any_fallback());

    // Starter companion Tân Hiến Anh (htid=426) -> (42602, 42601)
    let companion = profile.heroes.values().find(|h| h.htid == 426).unwrap();
    assert_eq!(companion.battle_skill_ids(), (42602, 42601));

    // Quality 11 protagonist `htid = 35` / `36` recognized as protagonist and blocked from summon_hero
    assert!(profile.summon_hero(35, 1).is_err());
    assert!(profile.summon_hero(36, 1).is_err());

    // Summoned Trương Liêu (`101`) transitions to `awake_com_atk`/`awake_angry_atk` at `awake_rank = 5`
    let zl_hid = profile.summon_hero(101, 80).unwrap();
    assert_eq!(profile.heroes[&zl_hid].battle_skill_ids(), (10102, 10101));
    profile.heroes.get_mut(&zl_hid).unwrap().awake_rank = 5;
    assert_eq!(
        profile.heroes[&zl_hid].battle_skill_ids(),
        (5210102, 5210101)
    );
}

#[test]
fn test_36_copy_progress_and_act_star_claimed_wal_and_json_roundtrip() {
    let dir = TempTestDir::new("copy_actstar_wal");
    let data_dir = dir.path.join("data");
    let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    store
        .register_account("pid_star", "qid_star", "star_user", "tok", 1_700_000_000)
        .unwrap();
    let p = store
        .create_character("pid_star", "2900161", 1, "ĐiêuThuyềnA6", 1_700_000_005)
        .unwrap();

    store
        .mutate_player(p.uid, 1_700_000_050, |pl| {
            pl.act_star_claimed = vec![1, 3];
            pl.act_star_sign_claimed = vec![15, 25];
            let mut chap_map = BTreeMap::new();
            chap_map.insert("prize_mask".to_string(), JsonValue::from(1u64));
            pl.copy_progress
                .insert("1".to_string(), JsonValue::Object(chap_map));
            Ok(())
        })
        .unwrap();

    // Ensure internal fields do not pollute user-facing `va_config`
    let in_mem = store.get_player(p.uid).unwrap();
    assert!(!in_mem.va_config.contains_key("copy_progress"));
    assert!(!in_mem.va_config.contains_key("act_star_claimed"));

    drop(store);
    let reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    let restored = reopened.get_player(p.uid).unwrap();
    assert_eq!(restored.act_star_claimed, vec![1, 3]);
    assert_eq!(restored.act_star_sign_claimed, vec![15, 25]);
    assert_eq!(
        restored
            .copy_progress
            .get("1")
            .and_then(|v| v.get("prize_mask"))
            .and_then(JsonValue::as_u64),
        Some(1)
    );
}
