use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gserver_player::{
    classify_template_id, crc32_ieee, to_vnsg2_other_position_info, Account, BagType, CurrencyType,
    EquipSlotType, GodArmAction, ItemClass, JsonValue, PlayerError, PlayerProfile, PlayerStore,
    RefineryDecomposeKind, RefineryRebornKind, RefinerySellKind, SellBagEntry, StarterHeroSeed,
    StarterItemSeed, StarterPackConfig, UseItemEffect, WalOp, WriteAheadLog, MAX_LUA_SAFE_INT,
    VALID_BATTLE_GRID_CELLS,
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
                htid: 103, // Quách Gia
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
    let mut p = PlayerProfile::new_character(
        52592,
        "636df6782106606d",
        "2900161",
        1,
        "Kaiaoaoa",
        1_795_500_450,
        &cfg,
    )
    .expect("create character");
    p.grant_currency(CurrencyType::ClanContri, 750, 1_795_500_460);
    p.grant_currency(CurrencyType::WorldCoin, 320, 1_795_500_460);
    p.grant_currency(CurrencyType::TowerCoin, 900, 1_795_500_460);
    p.grant_currency(CurrencyType::GodarmCoin, 450, 1_795_500_460);
    p.grant_currency(CurrencyType::PetCoin, 650, 1_795_500_460);
    p.grant_currency(CurrencyType::GodanimalCoin, 850, 1_795_500_460);

    let wire = p.to_vnsg2_get_user();
    let obj = wire.as_object().expect("UserData object");

    // 1. `copper` & `fight_force_max` MUST be numeric within Lua IEEE-754 double precision (`<= 2^53 - 1`)
    // so `UserData:onUpdateUser` + `getCopper() < cost` never crashes comparing string with number!
    assert_eq!(obj.get("copper").and_then(|v| v.as_u64()), Some(5_000_000));
    assert!(obj.get("copper").and_then(|v| v.as_u64()).unwrap() <= MAX_LUA_SAFE_INT);
    assert_eq!(
        obj.get("silver_num").and_then(|v| v.as_u64()),
        Some(5_000_000)
    );
    assert!(obj.get("fight_force_max").and_then(|v| v.as_u64()).is_some());

    // 2. All progression & shop currencies + aliases MUST be numeric
    assert_eq!(obj.get("gold").and_then(|v| v.as_u64()), Some(58_888));
    assert_eq!(obj.get("gold_num").and_then(|v| v.as_u64()), Some(58_888));
    assert_eq!(obj.get("gold_bind").and_then(|v| v.as_u64()), Some(18_888));
    assert_eq!(obj.get("hero_soul").and_then(|v| v.as_u64()), Some(20_000));
    assert_eq!(obj.get("soul_num").and_then(|v| v.as_u64()), Some(20_000));
    assert_eq!(obj.get("prestige").and_then(|v| v.as_u64()), Some(10_000));
    assert_eq!(obj.get("arena_coin").and_then(|v| v.as_u64()), Some(10_000));
    assert_eq!(obj.get("tower_num").and_then(|v| v.as_u64()), Some(900));
    assert_eq!(obj.get("jewel").and_then(|v| v.as_u64()), Some(5_000));
    assert_eq!(obj.get("awake_coin").and_then(|v| v.as_u64()), Some(5_000));
    assert_eq!(obj.get("clan_contri").and_then(|v| v.as_u64()), Some(750));
    assert_eq!(obj.get("godarm_coin").and_then(|v| v.as_u64()), Some(450));
    assert_eq!(obj.get("pet_coin").and_then(|v| v.as_u64()), Some(650));
    assert_eq!(
        obj.get("god_animal_coin").and_then(|v| v.as_u64()),
        Some(850)
    );
    assert_eq!(obj.get("world_coin").and_then(|v| v.as_u64()), Some(320));
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

    let upd = p.to_vnsg2_user_update();
    assert_eq!(upd.get("copper").and_then(|v| v.as_u64()), Some(5_000_000));

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
    assert_eq!(obj.get("exp").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("evolve_level").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("upgrade_level").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("destiny").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("awake_level").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("lock").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("skin_id").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(obj.get("hero_star_level").and_then(|v| v.as_u64()), Some(1));
    assert!(obj.get("pill").and_then(|v| v.as_object()).is_some());

    // Verify `va_hero = { star: {level: 1, progress: 0}, wash: {attr: {}, replace: {}, times: 0}, dress: ... }`
    // and strictly NO `arming` inside `va_hero`!
    let va_hero = obj
        .get("va_hero")
        .and_then(|v| v.as_object())
        .expect("va_hero object");
    assert!(
        !va_hero.contains_key("arming"),
        "va_hero must NEVER contain arming"
    );
    assert_eq!(
        va_hero
            .get("star")
            .and_then(|s| s.get("level"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        va_hero
            .get("star")
            .and_then(|s| s.get("progress"))
            .and_then(|v| v.as_u64()),
        Some(0)
    );
    let wash = va_hero.get("wash").and_then(|v| v.as_object()).unwrap();
    assert!(wash.get("attr").and_then(|v| v.as_object()).is_some());
    assert!(wash.get("replace").and_then(|v| v.as_object()).is_some());
    assert_eq!(wash.get("times").and_then(|v| v.as_u64()), Some(0));
    assert!(va_hero.get("dress").and_then(|v| v.as_object()).is_some());

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

    for arr_key in ["arming", "spirit", "yanWu", "brochure", "soulSkill"] {
        assert_eq!(
            obj.get(arr_key).and_then(|v| v.as_array()).unwrap().len(),
            6,
            "{arr_key} must be 6-element array"
        );
    }
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
        "soulStone",
        "soulJade",
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

    p.inventory.grid_max_num.insert(BagType::Awake, 2);
    p.add_item(300_101, 2, 100).unwrap();
    let err = p.add_item(300_101, 1, 100).unwrap_err();
    assert!(matches!(err, PlayerError::BagFull { .. }));

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

    let scroll_gid = p.add_item(30_201, 1, 110).unwrap()[0].gid;
    let out2 = p
        .use_item(scroll_gid, 1, UseItemEffect::SummonHero { htid: 304 }, 120)
        .unwrap();
    assert_eq!(out2.summoned_hids.len(), 1);
    assert!(p.heroes.contains_key(&out2.summoned_hids[0]));

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

    assert!(p.summon_hero(1, 1).is_err());
    assert!(p.summon_hero(2, 1).is_err());

    for i in 0..199 {
        p.summon_hero(100 + i, 1).unwrap();
    }
    assert_eq!(p.heroes.len(), 200);
    assert!(matches!(
        p.summon_hero(999, 1).unwrap_err(),
        PlayerError::HeroBagFull { max_heroes: 200 }
    ));

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

    p.set_hero_lock(fodder, true).unwrap();
    assert!(p.evolve_hero(main_gen, &[fodder], 1_000, 100).is_err());

    assert!(matches!(
        p.evolve_hero(main_gen, &[p.protagonist_hid], 1_000, 100)
            .unwrap_err(),
        PlayerError::ProtagonistCannotBeRemoved { .. }
    ));

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

    assert!(matches!(
        p.unset_squad_slot(1).unwrap_err(),
        PlayerError::ProtagonistCannotBeRemoved { .. }
    ));
    let h3 = p.summon_hero(301, 10).unwrap();
    assert!(matches!(
        p.set_squad_slot(1, h3).unwrap_err(),
        PlayerError::ProtagonistCannotBeRemoved { .. }
    ));

    p.swap_squad_slots(1, 2).unwrap();
    assert_eq!(p.formation.position.get(&2), Some(&p.protagonist_hid));

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

    let dup_426 = p.summon_hero(426, 1).unwrap();
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
fn test_26_battle_grid_6_cell_formation_sync_and_custom_layout_validation() {
    let cfg = StarterPackConfig::default();
    let mut p =
        PlayerProfile::new_character(52592, "pid_a", "2900161", 1, "TuânÚc", 1_700_000_000, &cfg)
            .unwrap();
    let comp_hid = p.formation.position.get(&2).copied().unwrap();

    // Default squad sync places slot 1 -> cell 3, slot 2 -> cell 6 (strictly inside `{2,3,5,6,8,9}`)
    for &cell in p.formation.grid_formation.keys() {
        assert!(VALID_BATTLE_GRID_CELLS.contains(&cell));
    }

    // New `set_battle_grid` strictly rejects invisible UI cells `1, 4, 7` (`FormationPanel.lua:34`)
    for bad_cell in [1u8, 4, 7, 10] {
        let mut invalid = BTreeMap::new();
        invalid.insert(bad_cell, p.protagonist_hid);
        invalid.insert(9, comp_hid);
        assert!(
            matches!(
                p.set_battle_grid(invalid).unwrap_err(),
                PlayerError::InvalidFormationSlot(_)
            ),
            "cell {bad_cell} must be rejected by set_battle_grid"
        );
    }

    // Valid `{2, 3, 5, 6, 8, 9}` layout succeeds
    let mut valid_grid = BTreeMap::new();
    valid_grid.insert(2, p.protagonist_hid);
    valid_grid.insert(9, comp_hid);
    p.set_battle_grid(valid_grid.clone()).unwrap();
    assert_eq!(p.formation.grid_formation, valid_grid);

    // Legacy `set_grid_formation` remaps `1, 4, 7` onto free `{2, 3, 5, 6, 8, 9}` cells cleanly
    let mut legacy_grid = BTreeMap::new();
    legacy_grid.insert(1, p.protagonist_hid);
    legacy_grid.insert(9, comp_hid);
    p.set_grid_formation(legacy_grid).unwrap();
    for &cell in p.formation.grid_formation.keys() {
        assert!(VALID_BATTLE_GRID_CELLS.contains(&cell));
    }
    let wire_grid = p.formation.to_vnsg2_formation_get();
    let f_obj = wire_grid.get("formation").unwrap().as_object().unwrap();
    assert!(!f_obj.contains_key("1"));
    assert!(!f_obj.contains_key("4"));
    assert!(!f_obj.contains_key("7"));

    // Omitting an active squad hero must fail
    let mut missing_hero = BTreeMap::new();
    missing_hero.insert(2, p.protagonist_hid);
    assert!(p.set_battle_grid(missing_hero).is_err());
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

    assert!(matches!(
        p.equip_item(1, EquipSlotType::Weapon, tr.gid).unwrap_err(),
        PlayerError::EquipTypeMismatch { .. }
    ));

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

    let swapped = p
        .equip_item(1, EquipSlotType::Weapon, w2.gid)
        .unwrap()
        .expect("previous weapon returned to bag");
    assert_eq!(swapped.item_id, w1.item_id);
    assert_eq!(p.inventory.count_by_template(101_101), 1);
    assert_eq!(p.inventory.count_by_template(101_102), 0);

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
            p.grant_currency(CurrencyType::Gold, 50_000, 1_700_000_010);
            p.consume_currency(CurrencyType::Copper, 999_999_999_999, 1_700_000_010)?;
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(err, PlayerError::InsufficientCurrency { .. }));

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
    }

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
    fs::remove_file(data_dir.join("wal").join("operations.wal")).unwrap();

    let mut reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    let status = reopened.record_user_login(p.uid, 1_700_000_200).unwrap();
    assert_eq!(status, "ban:1800000000:speedhack");

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

    let prot = profile.heroes.get(&profile.protagonist_hid).unwrap();
    assert_eq!(prot.battle_skill_ids(), (102, 101));
    assert!(!prot.resolved_battle_skills().used_any_fallback());

    let companion = profile.heroes.values().find(|h| h.htid == 426).unwrap();
    assert_eq!(companion.battle_skill_ids(), (42602, 42601));

    assert!(profile.summon_hero(35, 1).is_err());
    assert!(profile.summon_hero(36, 1).is_err());

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

#[test]
fn test_37_all_42_subbags_gid_ranges_template_classification_and_va_item_text_defaults() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70001, "pid_37", "2900161", 1, "BagAudit", 1_700_000_000, &cfg)
            .unwrap();

    // Verify all 42 sub-bag GID ranges are non-overlapping and roundtrip through BagType::from_gid
    let mut prev_end = 0u32;
    for bt in BagType::ALL {
        let (start, end) = bt.gid_range();
        assert!(start > prev_end, "{bt:?} range overlaps previous sub-bag");
        assert_eq!(BagType::from_gid(start), Some(bt));
        assert_eq!(BagType::from_gid(end), Some(bt));
        assert_eq!(BagType::from_key(bt.vnsg2_key()), Some(bt));
        prev_end = end;
    }
    assert_eq!(BagType::from_gid(0), None);
    assert_eq!(BagType::from_gid(999_999), None);

    // Every single ItemFactory template family across Normal + TimeMachine MUST allocate a valid GID
    let sample_tids: &[(u32, ItemClass, BagType)] = &[
        (10_010, ItemClass::Normal, BagType::Item),
        (22_102, ItemClass::Direct, BagType::Item),
        (30_001, ItemClass::RandGift, BagType::Item),
        (40_101, ItemClass::Dress, BagType::Item),
        (80_101, ItemClass::Dress, BagType::Item),
        (50_101, ItemClass::HeroFragment, BagType::Soul),
        (101_101, ItemClass::Arm, BagType::Arm),
        (182_001, ItemClass::Treasure, BagType::Treasure),
        (200_101, ItemClass::Spirit, BagType::Spirit),
        (300_101, ItemClass::Awake, BagType::Awake),
        (400_101, ItemClass::Pet, BagType::Pet),
        (5_000_101, ItemClass::Pet, BagType::Pet),
        (500_101, ItemClass::GodArm, BagType::GodArm),
        (600_101, ItemClass::GodAnimal, BagType::GodAnimal),
        (700_101, ItemClass::HeroLegend, BagType::HeroLegend),
        (800_101, ItemClass::Brochure, BagType::Brochure),
        (900_101, ItemClass::HalidomFragment, BagType::HalidomFragment),
        (1_000_101, ItemClass::Gem, BagType::Gem),
        (1_100_101, ItemClass::GodArmGem, BagType::Item),
        (1_200_101, ItemClass::Pill, BagType::Item),
        (1_300_101, ItemClass::BingFuFragment, BagType::BingFuFragment),
        (1_400_101, ItemClass::SoulSkill, BagType::SoulSkill),
        (1_500_101, ItemClass::Virtual, BagType::Item),
        (1_600_101, ItemClass::StatueFragment, BagType::StatueFragment),
        (1_700_101, ItemClass::BingShuFragment, BagType::BingShuFragment),
        (1_800_101, ItemClass::JobSkill, BagType::Item),
        (1_850_101, ItemClass::JobSkillFragment, BagType::JobSkillFragment),
        (1_900_101, ItemClass::SoulSkillFragment, BagType::Item),
        (3_100_101, ItemClass::ArmFragment, BagType::Fragment),
        (3_200_101, ItemClass::TreasureFragment, BagType::TreasureFragment),
        (400_000_101, ItemClass::TreasureFragment, BagType::TreasureFragment),
        (3_300_101, ItemClass::AwakeFragment, BagType::AwakeFragment),
        (3_400_101, ItemClass::PetFragment, BagType::PetFragment),
        (3_500_101, ItemClass::GodArmFragment, BagType::GodArmFragment),
        (3_600_101, ItemClass::GodAnimalFragment, BagType::GodAnimalFragment),
        (3_700_101, ItemClass::HeroLegendFragment, BagType::HeroLegendFragment),
        (3_800_101, ItemClass::BrochureFragment, BagType::BrochureFragment),
        (3_900_101, ItemClass::YanWu, BagType::YanWu),
        (4_000_101, ItemClass::YanWuFragment, BagType::YanWuFragment),
        (4_100_101, ItemClass::YanWuSoul, BagType::YanWuSoul),
        (4_200_101, ItemClass::CounsellorFragment, BagType::CounsellorFragment),
        (4_300_101, ItemClass::Counsellor, BagType::Counsellor),
        (4_400_101, ItemClass::CounsellorEquip, BagType::CounsellorEquip),
        (4_500_101, ItemClass::CounsellorEquipFragment, BagType::CounsellorEquipFragment),
        (4_600_101, ItemClass::SoulStone, BagType::SoulStone),
        (4_700_101, ItemClass::SoulStoneFragment, BagType::SoulStoneFragment),
        (4_800_101, ItemClass::SoulJade, BagType::SoulJade),
        (4_900_101, ItemClass::SoulJadeFragment, BagType::SoulJadeFragment),
        (5_200_001, ItemClass::WareHouse, BagType::WareHouse),
        (5_300_101, ItemClass::PloyFragment, BagType::PloyFragment),
        (5_400_101, ItemClass::TopAnimal, BagType::TopAnimal),
        (5_500_101, ItemClass::TopAnimalFragment, BagType::TopAnimalFragment),
        (5_600_101, ItemClass::GreatSealFragment, BagType::GreatSealFragment),
    ];

    for &(tid, expected_class, expected_bag) in sample_tids {
        assert_eq!(
            classify_template_id(tid).unwrap(),
            (expected_class, expected_bag),
            "mismatch on tid={tid}"
        );
        let added = p.add_item(tid, 1, 100).unwrap();
        assert_eq!(BagType::from_gid(added[0].gid), Some(expected_bag));
    }

    // Verify `Spirit` and `Dress` va_item_text defaults specifically
    let spirit = p.inventory.find_first_by_template(200_101).unwrap();
    for key in [
        "reinforce_level",
        "reinforce_exp",
        "progress_level",
        "total_exp",
        "gold_rank",
        "dark_gold_rank",
    ] {
        assert!(
            spirit.va_item_text.contains_key(key),
            "Spirit missing va_item_text.{key}"
        );
    }
    let dress = p.inventory.find_first_by_template(40_101).unwrap();
    assert_eq!(
        dress.va_item_text.get("dress_level").and_then(|v| v.as_u64()),
        Some(1)
    );
}

#[test]
fn test_38_bag_delta_emits_empty_json_object_on_zero_stack_or_deletion() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70002, "pid_38", "2900161", 1, "DeltaTest", 1_700_000_000, &cfg)
            .unwrap();

    let (items, add_delta) = p.add_item_with_delta(10_010, 5, 100).unwrap();
    let gid = items[0].gid;
    let gid_str = gid.to_string();

    let add_payload = add_delta.to_bag_update_payload();
    assert_eq!(
        add_payload.as_array().unwrap()[0]
            .get(&gid_str)
            .and_then(|v| v.get("item_num"))
            .and_then(|v| v.as_u64()),
        Some(5)
    );

    // Partial removal (5 -> 2) emits updated BagItem dict
    let (_, part_delta) = p.remove_item_by_gid_with_delta(gid, 3).unwrap();
    assert_eq!(
        part_delta.to_bag_update_payload().as_array().unwrap()[0]
            .get(&gid_str)
            .and_then(|v| v.get("item_num"))
            .and_then(|v| v.as_u64()),
        Some(2)
    );

    // Full removal (2 -> 0) MUST emit empty object `{}` at `[gidStr]` (`f.item_id == nil` in Lua)!
    let (_, del_delta) = p.remove_item_by_gid_with_delta(gid, 2).unwrap();
    let del_payload = del_delta.to_bag_update_payload();
    let slot_val = del_payload.as_array().unwrap()[0]
        .get(&gid_str)
        .expect("gid key present in deletion delta");
    assert_eq!(slot_val, &JsonValue::empty_object());
}

#[test]
fn test_39_formation_position_info_slot_bound_gear_godanimal_dicts_and_other_position_info() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70003, "pid_39", "2900161", 1, "PosSchema", 1_700_000_000, &cfg)
            .unwrap();

    let wep = p.add_item(101_101, 1, 100).unwrap()[0].clone();
    let sp = p.add_item(200_101, 1, 100).unwrap()[0].clone();
    let animal = p.add_item(600_101, 1, 100).unwrap()[0].clone();

    p.add_arming(1, EquipSlotType::Weapon, wep.item_id).unwrap();
    p.add_spirit(1, 1, sp.item_id).unwrap();
    p.formation.godanimal.insert(1, None); // unlocked empty -> `0`
    p.add_god_animal(2, animal.item_id).unwrap(); // occupied -> full BagItem wire dict!

    let pos_info = p.formation.to_vnsg2_position_info(&p.heroes);
    // `arming[0]` (slot 1): `"1"` has weapon, `"2".."9"` omitted
    let slot1_arm = pos_info
        .get("arming")
        .unwrap()
        .as_array()
        .unwrap()[0]
        .as_object()
        .unwrap();
    assert_eq!(slot1_arm.len(), 1);
    assert_eq!(
        slot1_arm
            .get("1")
            .and_then(|v| v.get("item_id"))
            .and_then(|v| v.as_u64()),
        Some(wep.item_id)
    );

    // `spirit[0]`: `"1"` is full BagItem dict, `"2".."6"` are `{}`
    let slot1_sp = pos_info
        .get("spirit")
        .unwrap()
        .as_array()
        .unwrap()[0]
        .as_object()
        .unwrap();
    assert_eq!(
        slot1_sp
            .get("1")
            .and_then(|v| v.get("item_id"))
            .and_then(|v| v.as_u64()),
        Some(sp.item_id)
    );
    assert_eq!(slot1_sp.get("2"), Some(&JsonValue::empty_object()));

    // `godanimal`: slot `"1"` is `0`, slot `"2"` is full BagItem dict (NEVER bare u64!)
    let god_map = pos_info.get("godanimal").unwrap().as_object().unwrap();
    assert_eq!(god_map.get("1").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(
        god_map
            .get("2")
            .and_then(|v| v.get("item_template_id"))
            .and_then(|v| v.as_u64()),
        Some(600_101)
    );

    // `to_vnsg2_other_position_info` embeds `userInfo`, `godanimal` dicts, and `equip`/`spirit` on `allHeros`
    let other = to_vnsg2_other_position_info(&p);
    assert!(other
        .get("userInfo")
        .and_then(|u| u.get(&p.uid.to_string()))
        .is_some());
    let prot_other = other
        .get("allHeros")
        .and_then(|h| h.get(&p.protagonist_hid.to_string()))
        .unwrap();
    assert_eq!(
        prot_other
            .get("equip")
            .and_then(|e| e.get("1"))
            .and_then(|w| w.get("item_id"))
            .and_then(|v| v.as_u64()),
        Some(wep.item_id)
    );
    assert!(prot_other.get("arrSkill").unwrap().as_object().is_some());
    assert!(prot_other.get("heroAttr").unwrap().as_object().is_some());
}

#[test]
fn test_40_sell_bag_items_and_refinery_sell_fragments_or_props() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70004, "pid_40", "2900161", 1, "Seller", 1_700_000_000, &cfg)
            .unwrap();

    let prop = p.add_item(10_010, 10, 100).unwrap()[0].clone();
    let arm = p.add_item(101_101, 1, 100).unwrap()[0].clone();
    let copper_start = p.currency(CurrencyType::Copper);

    // Lock guard prevents accidental sale
    p.inventory
        .get_mut_by_gid(arm.gid)
        .unwrap()
        .va_item_text
        .insert("refineryLock".into(), JsonValue::from(true));
    assert!(matches!(
        p.sell_bag_items(&[SellBagEntry::gid_only(arm.gid)], None, 110)
            .unwrap_err(),
        PlayerError::ResourceLockedOrDeployed(_)
    ));
    p.inventory
        .get_mut_by_gid(arm.gid)
        .unwrap()
        .va_item_text
        .insert("refineryLock".into(), JsonValue::from(false));

    // Parse wire JSON `[[gid, item_id, 4], [arm_gid]]`
    let raw_req = JsonValue::Array(vec![
        JsonValue::Array(vec![
            JsonValue::from(prop.gid),
            JsonValue::from(prop.item_id),
            JsonValue::from(4u32),
        ]),
        JsonValue::Array(vec![JsonValue::from(arm.gid)]),
    ]);
    let sell_out = p.sell_bag_items_json(&raw_req, None, 120).unwrap();
    assert!(sell_out.copper_added > 0);
    assert_eq!(
        p.currency(CurrencyType::Copper),
        copper_start + sell_out.copper_added
    );
    assert_eq!(p.inventory.count_by_template(10_010), 6);
    assert!(sell_out.bag_deltas.removed_gids.contains(&arm.gid));

    // Refinery `Bán` button for HeroFragment (`Soul`), `AwakeFragment`, `GodArmFragment`, and `HeroLegend`
    let soul_frag = p.add_item(50_101, 4, 130).unwrap()[0].clone();
    let soul_before = p.currency(CurrencyType::HeroSoul);
    let r_soul = p
        .refinery_sell_fragments_or_props(RefinerySellKind::Soul, &[soul_frag.item_id], None, 135)
        .unwrap();
    assert_eq!(r_soul.ret, "ok");
    assert!(p.currency(CurrencyType::HeroSoul) > soul_before);
    assert!(r_soul.bag_deltas.removed_gids.contains(&soul_frag.gid));

    let awake_frag = p.add_item(3_300_101, 2, 140).unwrap()[0].clone();
    let godarm_frag = p.add_item(3_500_101, 3, 140).unwrap()[0].clone();
    let r_frag = p
        .refinery_sell_fragments_or_props(
            RefinerySellKind::Fragment,
            &[awake_frag.item_id, godarm_frag.item_id],
            None,
            145,
        )
        .unwrap();
    assert_eq!(r_frag.rewards.get(&CurrencyType::AwakeArmCoin), Some(&50));
    assert_eq!(r_frag.rewards.get(&CurrencyType::GodarmCoin), Some(&75));

    let legend = p.add_item(700_101, 2, 150).unwrap()[0].clone();
    let r_leg = p
        .refinery_sell_fragments_or_props(RefinerySellKind::HeroLegend, &[legend.gid as u64], None, 155)
        .unwrap();
    assert_eq!(r_leg.rewards.get(&CurrencyType::HeroLegendCoin), Some(&100));
}

#[test]
fn test_41_use_item_in_bag_use_by_tpl_stamina_energy_choose_box_and_open_randgift_box() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70005, "pid_41", "2900161", 1, "BoxOpener", 1_700_000_000, &cfg)
            .unwrap();

    // 1. Stamina (`22102` -> +25, `22103` -> +50) & Energy (`22202` -> +10, `22203` -> +20)
    let st_small = p.add_item(22_102, 2, 100).unwrap()[0].clone();
    let st_big = p.add_item(22_103, 1, 100).unwrap()[0].clone();
    let en_small = p.add_item(22_202, 2, 100).unwrap()[0].clone();
    let en_big = p.add_item(22_203, 1, 100).unwrap()[0].clone();

    let exec_0 = p.currency(CurrencyType::Execution);
    let ener_0 = p.currency(CurrencyType::Energy);

    let u1 = p.use_item_in_bag(st_small.item_id, 2, None, None, 110).unwrap();
    assert_eq!(u1.drop.get("execution").and_then(|v| v.as_u64()), Some(50));
    let u2 = p.use_item_by_tpl(22_103, 1, None, None, 115).unwrap();
    assert_eq!(u2.drop.get("execution").and_then(|v| v.as_u64()), Some(50));
    assert_eq!(p.currency(CurrencyType::Execution), exec_0 + 100);
    assert!(u2.bag_deltas.removed_gids.contains(&st_big.gid));

    let u3 = p.use_item_in_bag(en_small.gid as u64, 2, None, None, 120).unwrap();
    assert_eq!(u3.drop.get("energy").and_then(|v| v.as_u64()), Some(20));
    let u4 = p.use_item_by_tpl(22_203, 1, None, None, 125).unwrap();
    assert_eq!(u4.drop.get("energy").and_then(|v| v.as_u64()), Some(20));
    assert_eq!(p.currency(CurrencyType::Energy), ener_0 + 40);
    assert!(u4.bag_deltas.removed_gids.contains(&en_big.gid));

    // 2. Choose-box (`option_idx`)
    let choose_box = p.add_item(20_005, 3, 130).unwrap()[0].clone();
    assert!(p.use_item_in_bag(choose_box.item_id, 1, Some(99), None, 131).is_err());
    let u_choose = p
        .use_item_in_bag(choose_box.item_id, 2, Some(1), None, 132)
        .unwrap();
    assert_eq!(
        u_choose
            .drop
            .get("item")
            .and_then(|m| m.get("50204"))
            .and_then(|v| v.as_u64()),
        Some(20)
    );
    assert_eq!(p.inventory.count_by_template(50_204), 20);

    // 3. OpenBox (`getOpenBoxInfo`, `openRandgiftBoxByKey`, `openRandgiftBoxByGold`)
    p.add_item(30_012, 3, 140).unwrap(); // Silver key for chest 33002
    let ob_key = p.open_randgift_box_by_key(33_002, 2, 145).unwrap();
    assert_eq!(ob_key.drops.len(), 2);
    assert_eq!(ob_key.gold_spent, 0);
    assert_eq!(p.inventory.count_by_template(30_012), 1);

    let gold_before = p.currency(CurrencyType::Gold);
    let ob_gold = p.open_randgift_box_by_gold(33_003, 2, 150).unwrap();
    // 33003 progressive Gold: 1st = 40, 2nd = 60 -> total 100 Gold
    assert_eq!(ob_gold.gold_spent, 100);
    assert_eq!(p.currency(CurrencyType::Gold), gold_before - 100);
    assert_eq!(
        p.get_open_box_info()
            .get("33003")
            .and_then(|m| m.get("gold_open_times"))
            .and_then(|v| v.as_u64()),
        Some(2)
    );
}

#[test]
fn test_42_open_bag_grid_by_gold_by_item_and_open_hero_bag_tiered_pricing_and_limits() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70006, "pid_42", "2900161", 1, "Expander", 1_700_000_000, &cfg)
            .unwrap();

    // Uncapped shard bag cannot be expanded
    assert!(p.open_bag_grid_by_gold(BagType::Soul, 1, 100).is_err());

    // Arm bag: +10 slots/step; 3 purchases = 10 + 20 + 50 = 80 Gold
    let gold_0 = p.currency(CurrencyType::Gold);
    let exp1 = p.open_bag_grid_by_gold(BagType::Arm, 3, 110).unwrap();
    assert_eq!(exp1.added_grids, 30);
    assert_eq!(exp1.new_max_grids, 230);
    assert_eq!(exp1.new_open_times, 3);
    assert_eq!(exp1.gold_spent, 80);
    assert_eq!(p.currency(CurrencyType::Gold), gold_0 - 80);

    // Expand GodArm bag (+5 slots/step) using `10028` bag expansion items
    p.add_item(10_028, 2, 115).unwrap();
    let exp2 = p
        .open_bag_grid_by_item(BagType::GodArm, Some(10_028), 2, 120)
        .unwrap();
    assert_eq!(exp2.added_grids, 10);
    assert_eq!(exp2.new_max_grids, 210);
    assert_eq!(p.inventory.count_by_template(10_028), 0);
    assert!(!exp2.bag_deltas.removed_gids.is_empty());

    // Exceeding `buy_storage_limit` returns `BagExpandLimitReached`
    assert!(matches!(
        p.open_bag_grid_by_gold(BagType::Arm, 38, 125).unwrap_err(),
        PlayerError::BagExpandLimitReached { .. }
    ));

    // Hero bag expansion (+10 heroes/step, 10 + 20 = 30 Gold for 2 steps)
    let (new_hero_cap, hero_gold) = p.open_hero_bag_by_gold(2, 130).unwrap();
    assert_eq!(new_hero_cap, 220);
    assert_eq!(hero_gold, 30);
}

#[test]
fn test_43_furnace_refinery_decompose_heroes_arm_treasure_awake_pet_spirit_godarm_soulskill() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70007, "pid_43", "2900161", 1, "Melter", 1_700_000_000, &cfg)
            .unwrap();

    // Protagonist and deployed companion cannot be decomposed
    let deployed_comp = p.formation.position[&2];
    assert!(matches!(
        p.refinery_decompose(RefineryDecomposeKind::Heroes, &[p.protagonist_hid], false, 100)
            .unwrap_err(),
        PlayerError::ProtagonistCannotBeRemoved { .. }
    ));
    assert!(matches!(
        p.refinery_decompose(RefineryDecomposeKind::Heroes, &[deployed_comp], false, 100)
            .unwrap_err(),
        PlayerError::ResourceLockedOrDeployed(_)
    ));

    // Summon general, invest levels & evolve, test preview_only then real decompose
    let fodder_hid = p.summon_hero(103, 15).unwrap();
    p.heroes.get_mut(&fodder_hid).unwrap().evolve_level = 2;

    let preview = p
        .refinery_decompose(RefineryDecomposeKind::Heroes, &[fodder_hid], true, 105)
        .unwrap();
    assert!(p.heroes.contains_key(&fodder_hid), "preview must not delete hero");
    assert!(preview.reward_dict.get("soul").and_then(|v| v.as_u64()).unwrap() > 0);

    let soul_0 = p.currency(CurrencyType::HeroSoul);
    let dec_hero = p
        .refinery_decompose(RefineryDecomposeKind::Heroes, &[fodder_hid], false, 110)
        .unwrap();
    assert!(!p.heroes.contains_key(&fodder_hid));
    assert!(p.currency(CurrencyType::HeroSoul) > soul_0);
    assert_eq!(p.inventory.count_by_template(10_010), 100); // 2 * 50 evolve pills refunded
    assert!(dec_hero.reward_dict.get("item").unwrap().as_object().is_some());

    // Decompose Arm, Spirit (`ResolveSpirit`), GodArm, and SoulSkill
    let arm = p.add_item(101_101, 1, 115).unwrap()[0].clone();
    p.reinforce_arm_vnsg2(arm.item_id, false, 116).unwrap();
    let dec_arm = p
        .refinery_decompose(RefineryDecomposeKind::Arm, &[arm.item_id], false, 120)
        .unwrap();
    assert!(dec_arm.bag_deltas.removed_gids.contains(&arm.gid));
    assert!(dec_arm.reward_currencies[&CurrencyType::Copper] > 5_000);

    let sp = p.add_item(200_101, 1, 125).unwrap()[0].clone();
    let dec_sp = p
        .refinery_decompose(RefineryDecomposeKind::ResolveSpirit, &[sp.item_id], false, 130)
        .unwrap();
    assert!(dec_sp.reward_currencies[&CurrencyType::SpiritCoin] >= 200);

    let ss = p.add_item(1_400_101, 1, 135).unwrap()[0].clone();
    let ss_prev = p
        .refinery_decompose(RefineryDecomposeKind::SoulSkill, &[ss.item_id], true, 136)
        .unwrap();
    assert!(p.inventory.get_by_gid(ss.gid).is_some());
    assert!(!ss_prev.reward_triplets.as_array().unwrap().is_empty());
    let ss_commit = p
        .refinery_decompose(RefineryDecomposeKind::SoulSkill, &[ss.item_id], false, 137)
        .unwrap();
    assert!(ss_commit.bag_deltas.removed_gids.contains(&ss.gid));
}

#[test]
fn test_44_furnace_refinery_reborn_hero_arm_treasure_awake_spirit_pet_godarm_pill_dress() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70008, "pid_44", "2900161", 1, "Reborner", 1_700_000_000, &cfg)
            .unwrap();

    // 1. Reborn invested Hero (`evolve_level = 4` refunds duplicate fragments `50105` + pills + silver, keeps Lv.1 hero!)
    let hid = p.summon_hero(105, 20).unwrap();
    {
        let h = p.heroes.get_mut(&hid).unwrap();
        h.evolve_level = 4;
        h.destiny = 3;
    }
    let gold_before = p.currency(CurrencyType::Gold);
    let rb_hero = p
        .refinery_reborn(RefineryRebornKind::Hero, hid, Some(true), false, 100)
        .unwrap();
    assert_eq!(rb_hero.gold_spent, 50);
    assert_eq!(p.currency(CurrencyType::Gold), gold_before - 50);
    let h_after = &p.heroes[&hid];
    assert_eq!(h_after.level, 1);
    assert_eq!(h_after.evolve_level, 0);
    assert_eq!(h_after.destiny, 3, "keep_destiny=Some(true) preserves destiny");
    assert_eq!(p.inventory.count_by_template(50_105), 80); // 1 duplicate copy of Xiahou Yuan (80 frags)

    // 2. Reborn reinforced & refined Arm keeps Lv.1 base item in bag and refunds stones + silver
    let arm = p.add_item(101_201, 1, 110).unwrap()[0].clone();
    p.reinforce_arm_vnsg2(arm.item_id, false, 111).unwrap();
    p.add_item(10_012, 10, 112).unwrap();
    let mut mats = BTreeMap::new();
    mats.insert(10_012, 5);
    p.upgrade_arm_vnsg2(arm.item_id, &mats, 113).unwrap();

    let rb_arm = p
        .refinery_reborn(RefineryRebornKind::Arm, arm.item_id, None, false, 115)
        .unwrap();
    assert_eq!(rb_arm.reset_item_id, Some(arm.item_id));
    let arm_reset = p.inventory.get_by_gid(arm.gid).unwrap();
    assert_eq!(
        arm_reset.va_item_text.get("reinforce_level").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        arm_reset.va_item_text.get("progress_level").and_then(|v| v.as_u64()),
        Some(0)
    );

    // 3. Reborn HeroPill and Dress
    p.heroes.get_mut(&hid).unwrap().hero_pill_rank = 2;
    let rb_pill = p
        .refinery_reborn(RefineryRebornKind::HeroPill, hid, None, false, 120)
        .unwrap();
    assert_eq!(p.heroes[&hid].hero_pill_rank, 0);
    assert_eq!(rb_pill.reward_items.get(&1_200_001), Some(&10));

    p.dress_info.insert(1, 4);
    let rb_dress = p
        .refinery_reborn(RefineryRebornKind::Dress, 1, None, false, 125)
        .unwrap();
    assert_eq!(p.dress_info[&1], 1);
    assert_eq!(rb_dress.reward_items.get(&10_015), Some(&15));
}

#[test]
fn test_45_fragment_synthesis_compose_hero_compose_item_quick_compose_and_treasure() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70009, "pid_45", "2900161", 1, "Crafter", 1_700_000_000, &cfg)
            .unwrap();

    // 1. Hero synthesis from `50105` (Xiahou Yuan, `fragment_count = 80` in `gserver_data::HEROES`)
    p.add_item(50_105, 85, 100).unwrap();
    let ch = p.compose_hero_from_frag(50_105, 1, None, 105).unwrap();
    assert_eq!(ch.consumed_frags, 80);
    assert_eq!(ch.heroes.len(), 1);
    assert_eq!(ch.heroes[0].htid, 105);
    assert_eq!(p.inventory.count_by_template(50_105), 5);

    // 2. Item synthesis (`ArmFragment`, `GodArmFragment`, `Gem` 3->1) & `quick_compose_from_frag`
    p.add_item(3_101_101, 40, 110).unwrap(); // 40 arm frags -> 2 weapons (`101101`)
    p.add_item(3_500_101, 40, 110).unwrap(); // 40 godarm frags -> 1 godarm (`500101`)
    let qc = p.quick_compose_from_frag(None, None, 115).unwrap();
    assert_eq!(qc.created_items.len(), 3);
    assert_eq!(p.inventory.count_by_template(101_101), 2);
    assert_eq!(p.inventory.count_by_template(500_101), 1);
    assert_eq!(p.inventory.count_by_template(3_101_101), 0);

    p.add_item(1_000_101, 6, 120).unwrap(); // 6 Lv1 gems -> 2 Lv2 gems (`1000102`)
    let gem_out = p.compose_item_from_frag(1_000_101, 2, None, 125).unwrap();
    assert_eq!(gem_out.created_items[0].item_template_id, 1_000_102);
    assert_eq!(p.inventory.count_by_template(1_000_102), 2);

    // 3. Horse/Book Treasure multi-part synthesis (`compose_treasure` & `one_key_compose_treasure`)
    // Treasure `182001` (offset `2001`) -> parts `3_220_011`, `3_220_012`, `3_220_013`, `3_220_014`
    for part in 1..=4u32 {
        p.add_item(3_200_000 + 2001 * 10 + part, 3, 130).unwrap();
    }
    let tr1 = p.compose_treasure(182_001, 1, None, 135).unwrap();
    assert_eq!(tr1.created_items.len(), 1);
    assert_eq!(tr1.created_items[0].item_template_id, 182_001);

    let tr_all = p.one_key_compose_treasure(182_001, None, 140).unwrap();
    assert_eq!(tr_all.created_items.len(), 2);
    assert_eq!(p.inventory.count_by_template(182_001), 3);
}

#[test]
fn test_46_squad_unset_returns_all_slot_gear_trans_hero_keeps_gear_partner_buddy_spirit_godanimal() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70010, "pid_46", "2900161", 1, "Commander", 1_700_000_000, &cfg)
            .unwrap();

    // Equip weapon, helmet, and 2 spirits onto slot 2 (`companion 426`)
    let w1 = p.add_item(101_101, 1, 100).unwrap()[0].clone();
    let h1 = p.add_item(102_101, 1, 100).unwrap()[0].clone();
    let sp1 = p.add_item(200_101, 1, 100).unwrap()[0].clone();
    let sp2 = p.add_item(200_102, 1, 100).unwrap()[0].clone();

    let (prev_ids, _) = p
        .add_multi_arming(
            2,
            &[(EquipSlotType::Weapon, w1.item_id), (EquipSlotType::Helmet, h1.item_id)],
        )
        .unwrap();
    assert_eq!(prev_ids, vec![None, None]);
    p.add_multi_spirit(2, &[(1, sp1.item_id), (2, sp2.item_id)])
        .unwrap();

    // `trans_hero(1, 2)` swaps protagonist & companion heroes while keeping gear anchored on slot 2!
    let comp_hid = p.formation.position[&2];
    p.trans_hero(1, 2).unwrap();
    assert_eq!(p.formation.position[&1], comp_hid);
    assert_eq!(p.formation.position[&2], p.protagonist_hid);
    assert_eq!(
        p.formation.arming[&2][&(EquipSlotType::Weapon as u8)].item_id,
        w1.item_id,
        "slot-bound gear must remain on slot 2 during trans_hero"
    );
    // Swap back so slot 2 holds non-protagonist `comp_hid`
    p.trans_hero(1, 2).unwrap();

    // `transfer_arming` and `transfer_spirit` move gear between slots cleanly
    p.transfer_arming(2, 1, EquipSlotType::Helmet).unwrap();
    assert_eq!(
        p.formation.arming[&1][&(EquipSlotType::Helmet as u8)].item_id,
        h1.item_id
    );
    p.transfer_spirit(2, 1, 2, 1).unwrap();
    assert_eq!(p.formation.spirit[&1][&1].item_id, sp2.item_id);

    // `unset_hero_from_position(2)` unsets `comp_hid` AND returns slot 2's remaining weapon + spirit to bag!
    let (unset_hid, unset_delta) = p.unset_hero_from_position(2).unwrap();
    assert_eq!(unset_hid, comp_hid);
    assert!(p.formation.arming[&2].is_empty());
    assert!(p.formation.spirit[&2].is_empty());
    assert_eq!(unset_delta.upserted.len(), 2);
    assert_eq!(p.inventory.count_by_template(101_101), 1);
    assert_eq!(p.inventory.count_by_template(200_101), 1);

    // Partner & Buddy lifecycle (`open_partner`, `unset_multi_partner`, `open_buddy`, `set_buddy`, `buddy_lv_up`, `reborn_buddy`)
    let p_hero = p.summon_hero(205, 1).unwrap();
    let b_hero = p.summon_hero(305, 1).unwrap();
    p.open_partner(2, 10, 110).unwrap();
    p.set_partner(2, p_hero).unwrap();
    let cleared = p.unset_multi_partner(None).unwrap();
    assert_eq!(cleared, vec![p_hero]);
    assert_eq!(p.formation.partner[&2], 0);

    p.open_buddy(1, 20, 115).unwrap();
    p.set_buddy(1, b_hero).unwrap();
    assert_eq!(p.buddy_lv_up(1, 5_000, 120).unwrap(), 2);
    let (reset_lv, refunded_cop) = p.reborn_buddy(1, 50, 125).unwrap();
    assert_eq!(reset_lv, 1);
    assert_eq!(refunded_cop, 10_000);

    // GodAnimal add & remove
    let ga = p.add_item(600_101, 1, 130).unwrap()[0].clone();
    p.add_god_animal(1, ga.item_id).unwrap();
    let (rem_ga, _) = p.remove_god_animal(1).unwrap();
    assert_eq!(rem_ga.item_id, ga.item_id);
    assert_eq!(p.formation.godanimal.get(&1), Some(&None));
}

#[test]
fn test_47_hero_progression_vnsg2_level_up_evolve_one_key_quality_awake_star_wash() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70011, "pid_47", "2900161", 1, "HeroMaster", 1_700_000_000, &cfg)
            .unwrap();
    p.add_player_exp(5_000); // Raise player level cap

    let hid = p.summon_hero(101, 1).unwrap(); // Zhang Liao

    // 1. `hero_level_up_vnsg2` consuming bag EXP pills `10011..10014` + copper
    p.add_item(10_014, 5, 100).unwrap();
    p.add_item(10_011, 10, 100).unwrap();
    let lv_out = p.hero_level_up_vnsg2(hid, 5, 105).unwrap();
    assert_eq!((lv_out.old_lv, lv_out.newlv), (1, 6));
    assert!(lv_out.exp_delta > 0);
    assert!(!lv_out.item_cost.is_empty());

    // 2. `evolve_hero_vnsg2` with `one_key = true`:
    // Ev 0->1, 1->2, 2->3 need only `10010` pills; Ev 3->4 needs 1 unlocked undeployed duplicate!
    p.add_item(10_010, 200, 110).unwrap();
    let locked_dup = p.summon_hero(101, 1).unwrap();
    p.set_hero_lock(locked_dup, true).unwrap();
    let free_dup = p.summon_hero(101, 1).unwrap();

    let ev_out = p.evolve_hero_vnsg2(hid, true, 115).unwrap();
    assert_eq!(ev_out.new_evolve_lv, 4); // Stops at +4 because locked_dup is protected!
    assert_eq!(ev_out.del_hero_hids, vec![free_dup]);
    assert!(p.heroes.contains_key(&locked_dup), "locked duplicate must never be consumed");

    // 3. Quality promotion (`101 -> 1101 -> 2101`), Awake, Star, and Wash
    let red_htid = p.upgrade_hero_quality(hid, 120).unwrap();
    assert_eq!(red_htid, 1101);
    let gold_htid = p.upgrade_hero_quality(hid, 121).unwrap();
    assert_eq!(gold_htid, 2101);

    let (aw_rank, aw_lv, _) = p.awake_hero(hid, 125).unwrap();
    assert_eq!((aw_rank, aw_lv), (1, 1));

    let (star_lv, star_pos, _) = p.upgrade_hero_star(hid, true, 130).unwrap();
    assert_eq!((star_lv, star_pos), (2, 0));

    let (wash_roll, _) = p.wash_hero(hid, 3, false, false, 135).unwrap();
    assert!(wash_roll.get("replace").unwrap().as_object().unwrap().contains_key("9"));
    let (wash_applied, _) = p.wash_hero(hid, 1, true, false, 136).unwrap();
    assert!(wash_applied.get("attr").unwrap().as_object().unwrap().contains_key("9"));
    assert!(wash_applied.get("replace").unwrap().as_object().unwrap().is_empty());
}

#[test]
fn test_48_forge_reinforce_arm_auto_reinforce_slots_1_to_4_only_upgrade_wash_treasure_spirit_awake_godarm_explore() {
    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70012, "pid_48", "2900161", 1, "Blacksmith", 1_700_000_000, &cfg)
            .unwrap();
    p.grant_currency(CurrencyType::SpiritCoin, 2_000, 100);
    p.grant_currency(CurrencyType::GodarmCoin, 2_000, 100);
    p.grant_currency(CurrencyType::AwakeArmCoin, 2_000, 100);

    // Equip slots 1 (Weapon), 2 (Helmet), AND slots 5 (AttackTreasure), 7 (Awake), 8 (Pet), 9 (GodArm) on squad slot 1
    let wep = p.add_item(101_101, 1, 100).unwrap()[0].clone();
    let helm = p.add_item(102_101, 1, 100).unwrap()[0].clone();
    let treas = p.add_item(182_001, 1, 100).unwrap()[0].clone();
    let awake = p.add_item(300_101, 1, 100).unwrap()[0].clone();
    let pet = p.add_item(400_101, 1, 100).unwrap()[0].clone();
    let godarm = p.add_item(500_101, 1, 100).unwrap()[0].clone();

    p.add_arming(1, EquipSlotType::Weapon, wep.item_id).unwrap();
    p.add_arming(1, EquipSlotType::Helmet, helm.item_id).unwrap();
    p.add_arming(1, EquipSlotType::AttackTreasure, treas.item_id)
        .unwrap();
    p.add_arming(1, EquipSlotType::Awake, awake.item_id).unwrap();
    p.add_arming(1, EquipSlotType::Pet, pet.item_id).unwrap();
    p.add_arming(1, EquipSlotType::GodArm, godarm.item_id)
        .unwrap();

    // Single +1 reinforce on weapon
    let r1 = p.reinforce_arm_vnsg2(wep.item_id, true, 105).unwrap();
    assert_eq!((r1.level, r1.up_level), (2, 1));

    // CRITICAL: `auto_reinforce_arm_vnsg2(1)` MUST reinforce ONLY slots `1..=4` (wep & helm),
    // NEVER touching slots `5..=9` (`treas`, `awake`, `pet`, `godarm`)!
    let auto_map = p.auto_reinforce_arm_vnsg2(1, 110).unwrap();
    assert_eq!(auto_map.len(), 2);
    assert!(auto_map.contains_key(&wep.item_id.to_string()));
    assert!(auto_map.contains_key(&helm.item_id.to_string()));
    assert!(!auto_map.contains_key(&treas.item_id.to_string()));
    assert!(!auto_map.contains_key(&awake.item_id.to_string()));
    assert!(!auto_map.contains_key(&pet.item_id.to_string()));
    assert!(!auto_map.contains_key(&godarm.item_id.to_string()));
    assert_eq!(
        p.formation.arming[&1][&(EquipSlotType::AttackTreasure as u8)]
            .va_item_text
            .get("reinforce_level")
            .and_then(|v| v.as_u64()),
        Some(1),
        "Treasure in slot 5 must remain untouched by auto_reinforce_arm_vnsg2"
    );

    // Upgrade & wash Arm
    p.add_item(10_014, 2, 115).unwrap();
    let mut mats = BTreeMap::new();
    mats.insert(10_014, 2);
    let arm_up = p.upgrade_arm_vnsg2(wep.item_id, &mats, 116).unwrap();
    assert_eq!(arm_up.level, 2);
    let (w_pend, _) = p.arm_wash(wep.item_id, false, 0, false, 117).unwrap();
    assert!(w_pend.get("replace").is_some());
    let (w_committed, _) = p.arm_wash(wep.item_id, true, 0, false, 118).unwrap();
    assert!(w_committed.get("attr").is_some());

    // Treasure reinforce & upgrade
    let fodder_tr = p.add_item(182_002, 1, 120).unwrap()[0].clone();
    let tr_reinf = p
        .reinforce_treasure_vnsg2(treas.item_id, &[fodder_tr.item_id], 121)
        .unwrap();
    assert_eq!(tr_reinf.level, 2);
    p.add_item(10_014, 10, 122).unwrap();
    let tr_up = p.upgrade_treasure_vnsg2(treas.item_id, true, 123).unwrap();
    assert_eq!(tr_up.progress_level, 2);

    // Awake upgrade & GodArm upgrade/wash/replace
    let aw_up = p.upgrade_awake_vnsg2(awake.item_id, false, 125).unwrap();
    assert_eq!(aw_up.reinforce_level, 2);

    p.godarm_wash_upgrade(godarm.item_id, GodArmAction::Upgrade, true, 130)
        .unwrap();
    p.godarm_wash_upgrade(godarm.item_id, GodArmAction::Evolve, false, 131)
        .unwrap();
    p.godarm_wash_upgrade(godarm.item_id, GodArmAction::Wash, false, 132)
        .unwrap();
    let (ga_va, _) = p
        .godarm_wash_upgrade(godarm.item_id, GodArmAction::ReplaceWash, false, 133)
        .unwrap();
    assert_eq!(ga_va.get("upgrade_level").and_then(|v| v.as_u64()), Some(10));
    assert_eq!(ga_va.get("progress_level").and_then(|v| v.as_u64()), Some(1));
    assert!(!ga_va.get("godArm_wash").unwrap().as_object().unwrap().is_empty());

    // Explore Soul (`spirit.exploreSoul`) & Spirit reinforce (`reinforce_spirit_vnsg2`)
    let exp_out = p.explore_soul(3, false, false, 140).unwrap();
    assert_eq!(exp_out.created_spirits.len(), 3);
    let main_sp = exp_out.created_spirits[0].item_id;
    let fodder_sp = exp_out.created_spirits[1].item_id;
    let sp_reinf = p
        .reinforce_spirit_vnsg2(main_sp, &[fodder_sp], 300, 145)
        .unwrap();
    assert!(sp_reinf.level >= 3);
    assert_eq!(sp_reinf.total_exp, 600);
}

#[test]
fn test_49_storage_and_wal_roundtrip_preserves_raffle_state_slot_gear_and_all_currencies() {
    let dir = TempTestDir::new("full_roundtrip_49");
    let data_dir = dir.path.join("data");

    let uid = {
        let mut store = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
        store
            .register_account("pid_49", "qid_49", "roundtrip_49", "tok", 1_700_000_000)
            .unwrap();
        let created = store
            .create_character("pid_49", "2900161", 1, "PersistAll", 1_700_000_010)
            .unwrap();

        store
            .mutate_player(created.uid, 1_700_000_050, |p| {
                p.grant_currency(CurrencyType::GodarmCoin, 333, 1_700_000_050);
                p.grant_currency(CurrencyType::PetCoin, 444, 1_700_000_050);
                p.grant_currency(CurrencyType::GodanimalCoin, 555, 1_700_000_050);
                p.grant_currency(CurrencyType::ClanContri, 666, 1_700_000_050);
                p.grant_currency(CurrencyType::WorldCoin, 777, 1_700_000_050);
                p.open_randgift_box_by_gold(33_002, 2, 1_700_000_050)?;
                let sp = p.add_item(200_101, 1, 1_700_000_050)?[0].clone();
                p.add_spirit(1, 1, sp.item_id)?;
                let ga = p.add_item(600_101, 1, 1_700_000_050)?[0].clone();
                p.add_god_animal(1, ga.item_id)?;
                Ok(())
            })
            .unwrap();
        created.uid
    };

    let reopened = PlayerStore::open(&data_dir, StarterPackConfig::default()).unwrap();
    let p = reopened.get_player(uid).unwrap();
    assert_eq!(p.currency(CurrencyType::GodarmCoin), 333);
    assert_eq!(p.currency(CurrencyType::PetCoin), 444);
    assert_eq!(p.currency(CurrencyType::GodanimalCoin), 555);
    assert_eq!(p.currency(CurrencyType::ClanContri), 666);
    assert_eq!(p.currency(CurrencyType::WorldCoin), 777);
    assert_eq!(
        p.get_open_box_info()
            .get("33002")
            .and_then(|m| m.get("gold_open_times"))
            .and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(p.formation.spirit[&1][&1].item_template_id, 200_101);
    assert_eq!(
        p.formation.godanimal[&1].as_ref().unwrap().item_template_id,
        600_101
    );
}

#[test]
fn test_50_game_data_store_backed_sell_direct_choose_randgift_require_ids_and_compose() {
    let dir = TempTestDir::new("db_store_50");
    let db_root = dir.path.join("dbs_out");
    fs::create_dir_all(db_root.join("timeMachine_item")).unwrap();
    fs::create_dir_all(db_root.join("timeMachine_hero")).unwrap();

    let manifest = r#"{
        "tables": [
            {
                "database": "timeMachine_item",
                "table": "timeMachine_item_normal",
                "relative_path": "timeMachine_item/timeMachine_item_normal.json",
                "row_count": 1,
                "column_count": 4
            },
            {
                "database": "timeMachine_item",
                "table": "timeMachine_item_direct",
                "relative_path": "timeMachine_item/timeMachine_item_direct.json",
                "row_count": 2,
                "column_count": 5
            },
            {
                "database": "timeMachine_item",
                "table": "timeMachine_item_randgift",
                "relative_path": "timeMachine_item/timeMachine_item_randgift.json",
                "row_count": 1,
                "column_count": 4
            },
            {
                "database": "timeMachine_hero",
                "table": "timeMachine_item_soul",
                "relative_path": "timeMachine_hero/timeMachine_item_soul.json",
                "row_count": 1,
                "column_count": 4
            }
        ]
    }"#;
    fs::write(db_root.join("manifest.json"), manifest).unwrap();

    fs::write(
        db_root
            .join("timeMachine_item")
            .join("timeMachine_item_normal.json"),
        r#"{
            "database": "timeMachine_item",
            "table": "timeMachine_item_normal",
            "columns": [],
            "rows": [
                { "id": 10055, "sell_num": 3456, "composite_after": 10056, "composite_number": 4 }
            ]
        }"#,
    )
    .unwrap();

    fs::write(
        db_root
            .join("timeMachine_item")
            .join("timeMachine_item_direct.json"),
        r#"{
            "database": "timeMachine_item",
            "table": "timeMachine_item_direct",
            "columns": [],
            "rows": [
                { "id": 20500, "reward": [[1, 0, 77777], [4, 10010, 3]] },
                { "id": 20501, "choose": [[2, 0, 250], [15, 0, 800]] }
            ]
        }"#,
    )
    .unwrap();

    fs::write(
        db_root
            .join("timeMachine_item")
            .join("timeMachine_item_randgift.json"),
        r#"{
            "database": "timeMachine_item",
            "table": "timeMachine_item_randgift",
            "columns": [],
            "rows": [
                { "id": 30001, "use_level_min": 1, "require_ids": [[30004, 1]] }
            ]
        }"#,
    )
    .unwrap();

    fs::write(
        db_root
            .join("timeMachine_hero")
            .join("timeMachine_item_soul.json"),
        r#"{
            "database": "timeMachine_hero",
            "table": "timeMachine_item_soul",
            "columns": [],
            "rows": [
                { "id": 50426, "composit_hero_id": 426, "need_part_num": 40, "hero_refinery_reward": [[15, 0, 12]] }
            ]
        }"#,
    )
    .unwrap();

    let ds = gserver_data::GameDataStore::from_export_dir(&db_root).expect("load fixture store");

    let mut cfg = StarterPackConfig::default();
    cfg.starter_items.clear();
    let mut p =
        PlayerProfile::new_character(70050, "pid_50", "2900161", 1, "DbDriven", 1_700_000_000, &cfg)
            .unwrap();

    // 1. DB `sell_num = 3456` & `composite_number = 4 -> 10056`
    let prop = p.add_item(10_055, 6, 100).unwrap()[0].clone();
    let sold = p
        .sell_bag_items(&[SellBagEntry::full(prop.gid, prop.item_id, 2)], Some(&ds), 105)
        .unwrap();
    assert_eq!(sold.copper_added, 6_912);
    let comp_norm = p.compose_item_from_frag(10_055, 1, Some(&ds), 106).unwrap();
    assert_eq!(comp_norm.created_items[0].item_template_id, 10_056);

    // 2. DB Direct `reward` & `choose`
    let dir_box = p.add_item(20_500, 2, 110).unwrap()[0].clone();
    let dir_out = p
        .use_item_in_bag(dir_box.item_id, 2, None, Some(&ds), 111)
        .unwrap();
    assert_eq!(
        dir_out.drop.get("copper").and_then(|v| v.as_u64()),
        Some(155_554)
    );
    assert_eq!(p.inventory.count_by_template(10_010), 6);

    let ch_box = p.add_item(20_501, 2, 112).unwrap()[0].clone();
    let ch_out = p
        .use_item_in_bag(ch_box.item_id, 2, Some(1), Some(&ds), 113)
        .unwrap();
    assert_eq!(
        ch_out.drop.get("hero_soul").and_then(|v| v.as_u64()),
        Some(1_600)
    );

    // 3. DB RandGift `require_ids: [[30004, 1]]`
    let rg = p.add_item(30_001, 1, 115).unwrap()[0].clone();
    assert!(matches!(
        p.use_item_in_bag(rg.item_id, 1, None, Some(&ds), 116)
            .unwrap_err(),
        PlayerError::InsufficientItemCount {
            template_id: 30_004,
            ..
        }
    ));
    p.add_item(30_004, 1, 117).unwrap();
    p.use_item_in_bag(rg.item_id, 1, None, Some(&ds), 118)
        .unwrap();
    assert_eq!(p.inventory.count_by_template(30_004), 0);

    // 4. DB Hero soul `50426 -> htid 426 (need 40)` & `hero_refinery_reward = 12 soul/shard`
    p.add_item(50_426, 45, 120).unwrap();
    let hc = p
        .compose_hero_from_frag(50_426, 1, Some(&ds), 121)
        .unwrap();
    assert_eq!(hc.heroes[0].htid, 426);
    assert_eq!(p.inventory.count_by_template(50_426), 5);
    let rem_gid = p.inventory.find_first_by_template(50_426).unwrap().gid;
    let rs = p
        .refinery_sell_fragments_or_props(RefinerySellKind::Soul, &[rem_gid as u64], Some(&ds), 122)
        .unwrap();
    assert_eq!(rs.rewards.get(&CurrencyType::HeroSoul), Some(&60));
}
