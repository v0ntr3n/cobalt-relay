#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use gserver_data::bt_row::{
    parse_babeltime_data_row, parse_babeltime_schema_row, BtFieldValue, RewardTriple,
};
use gserver_data::{
    bag_by_id, bt_decrypt, bt_decrypt_strict, bt_decrypt_vec, bt_encrypt_vec,
    clear_runtime_key_tables, decrypt_babeltime_cell, decrypt_basic_in_place,
    embedded_curated_json, exp_curve_by_id, gear_suit_by_id, has_decrypt_keys, hero_by_htid,
    hero_by_name, is_bt_encrypted_buffer, is_protagonist_htid, is_valid_hero_htid,
    is_valid_item_tid, is_valid_town_id, item_by_tid, items_by_category, normalize_asset_path,
    parse_json, protagonist_by_utid, register_key_table, replace_file_extension,
    resolve_relative_asset_path, town_by_id, DecryptStatus, ExportedDatabaseDir, GameDataStore,
    ItemCategory, Kingdom, BAG_LIMITS, BT_DECRYPT_HEADER_SIZE, BT_DECRYPT_MAGIC,
    BT_DEFAULT_KEY_TABLES, GEAR_SUITS, HEROES, ITEMS, LEVEL_EXP_CURVES, PROTAGONISTS, TOWNS,
};

const REAL_ENC_LUA: &[u8] = include_bytes!("../data/test_vectors/MainTownSceneReader.lua.enc");
const REAL_DEC_LUA: &[u8] = include_bytes!("../data/test_vectors/MainTownSceneReader.lua.dec");

// 1. Verify all 16 compiled-in keys match 07_bt_decrypt_keys.inc lengths (KEY_04 = 10, others = 32)
#[test]
fn test_01_key_table_inventory_and_lengths() {
    assert!(has_decrypt_keys());
    assert_eq!(BT_DEFAULT_KEY_TABLES.len(), 16);
    for (idx, key) in BT_DEFAULT_KEY_TABLES.iter().enumerate() {
        if idx == 4 {
            assert_eq!(key.len(), 10, "KEY_04 (fibonacci) must be 10 bytes");
        } else {
            assert_eq!(key.len(), 32, "KEY_{idx:02X} must be 32 bytes");
        }
    }
}

// 2. Byte-for-byte verification of bt_decrypt against a REAL encrypted client Lua file!
#[test]
fn test_02_bt_decrypt_real_encrypted_client_lua_byte_for_byte() {
    assert!(is_bt_encrypted_buffer(REAL_ENC_LUA));
    assert_eq!(REAL_ENC_LUA.len(), 1729);
    assert_eq!(REAL_DEC_LUA.len(), 1697);

    let decrypted = bt_decrypt_strict(REAL_ENC_LUA).expect("real client Lua must decrypt cleanly");
    assert_eq!(decrypted, REAL_DEC_LUA, "byte-for-byte match against real client Lua");

    let text = std::str::from_utf8(&decrypted).expect("decrypted Lua is valid UTF-8");
    assert!(text.contains("MainTownSceneReader"));
    assert!(text.contains("databaseName = \"common\", tableName = \"mainTown_scene\""));
}

// 3. Verify in-place C-callback-style bt_decrypt strips 32-byte header and zeroes reclaimed tail
#[test]
fn test_03_bt_decrypt_in_place_strips_header_and_zeroes_tail() {
    let mut work = REAL_ENC_LUA.to_vec();
    let orig_cap_len = work.len();
    let mut sz = orig_cap_len;

    assert!(bt_decrypt(&mut work, &mut sz));
    assert_eq!(sz, REAL_DEC_LUA.len());
    assert_eq!(&work[..sz], REAL_DEC_LUA);
    // Reclaimed 32-byte tail must be zeroed (including NUL terminator at work[sz])
    assert!(work[sz..orig_cap_len].iter().all(|&b| b == 0));
}

// 4. Roundtrip encrypt -> decrypt across ALL 16 key slots (0..=15) and both algo modes (0 and 1)
#[test]
fn test_04_roundtrip_all_16_keys_even_and_odd_payload_lengths() {
    let sample_even = b"GCLIENT vnsg2 SQLite & Lua test payload (even len)!";
    let sample_odd = b"GCLIENT vnsg2 odd-length payload 12345";

    for algo in [0u8, 1u8] {
        for key_idx in 0u8..=15u8 {
            let enc_a = bt_encrypt_vec(sample_even, algo, key_idx).unwrap();
            assert!(is_bt_encrypted_buffer(&enc_a));
            assert_eq!(enc_a.len(), sample_even.len() + BT_DECRYPT_HEADER_SIZE);
            let dec_a = bt_decrypt_strict(&enc_a).unwrap();
            assert_eq!(dec_a.as_slice(), sample_even);

            let enc_b = bt_encrypt_vec(sample_odd, algo, key_idx).unwrap();
            let dec_b = bt_decrypt_strict(&enc_b).unwrap();
            assert_eq!(dec_b.as_slice(), sample_odd);
        }
    }
}

// 5. Verify 0-byte encrypted payload edge case
#[test]
fn test_05_zero_byte_encrypted_payload() {
    let enc = bt_encrypt_vec(b"", 1, 0).expect("0-byte encrypt");
    assert_eq!(enc.len(), 32);
    assert!(is_bt_encrypted_buffer(&enc));
    let dec = bt_decrypt_strict(&enc).expect("0-byte decrypt");
    assert!(dec.is_empty());
}

// 6. Verify plaintext pass-through on bt_decrypt_vec (e.g. unencrypted SQLite format 3 header)
#[test]
fn test_06_plaintext_sqlite_passthrough() {
    let sqlite_hdr = b"SQLite format 3\0some_unencrypted_page_bytes_here_1234567890";
    assert!(!is_bt_encrypted_buffer(sqlite_hdr));
    let out = bt_decrypt_vec(sqlite_hdr).expect("plaintext should pass through");
    assert_eq!(out.as_slice(), sqlite_hdr);
}

// 7. Reject truncated 0xFEFEFEFE headers (< 32 bytes) so raw magic never leaks
#[test]
fn test_07_reject_truncated_magic_header() {
    let mut bad = vec![0u8; 16];
    bad[..4].copy_from_slice(&BT_DECRYPT_MAGIC.to_le_bytes());
    let err = bt_decrypt_vec(&bad).expect_err("truncated 0xFEFEFEFE must fail");
    assert_eq!(err.status, DecryptStatus::TruncatedHeader);
}

// 8. Reject payload length mismatch, invalid algo_index, and out-of-range key_index
#[test]
fn test_08_header_validation_errors() {
    let mut framed = bt_encrypt_vec(b"hello world", 1, 3).unwrap();

    // Corrupt payload_len at [4..8]
    framed[4] ^= 0xFF;
    let err_len = decrypt_basic_in_place(&mut framed).expect_err("mismatch must fail");
    assert_eq!(err_len.status, DecryptStatus::PayloadLengthMismatch);
    framed[4] ^= 0xFF;

    // Unsupported algo_index = 2
    framed[8] = 2;
    let err_algo = bt_decrypt_strict(&framed).expect_err("algo 2 must fail");
    assert_eq!(err_algo.status, DecryptStatus::UnsupportedAlgoIndex);
    framed[8] = 1;

    // Out-of-range key_index = 16
    framed[9] = 16;
    let err_key = bt_decrypt_strict(&framed).expect_err("key 16 must fail");
    assert_eq!(err_key.status, DecryptStatus::InvalidKeyIndex);
}

// 9. Runtime key override registration & cleanup
#[test]
fn test_09_runtime_key_override_and_restore() {
    let custom_key = [0xAA, 0x55, 0xCC, 0x33, 0xF0, 0x0F];
    assert!(!register_key_table(16, &custom_key));
    assert!(!register_key_table(15, &[]));
    assert!(register_key_table(15, &custom_key));

    let msg = b"custom runtime key slot 15 check";
    let enc = bt_encrypt_vec(msg, 1, 15).unwrap();
    let dec = bt_decrypt_strict(&enc).unwrap();
    assert_eq!(dec.as_slice(), msg);

    clear_runtime_key_tables();
    // Verify default KEY_0F is active again
    let enc_default = bt_encrypt_vec(msg, 1, 15).unwrap();
    assert_ne!(enc, enc_default);
    assert_eq!(bt_decrypt_strict(&enc_default).unwrap().as_slice(), msg);
}

// 10. Real SQLite `babeltime` hex-encoded cell decryption from `online.db` (KEY_07)
#[test]
fn test_10_decrypt_real_sqlite_online_db_hex_cells() {
    // Exact row id='0' from online.db (`activity_open_online` schema row)
    let online_row0_hex =
        "FEFEFEFE22000000010700000000000000000000000000000000000000000000D665212CA96EB0693C65A5736363256E0A732E727577E1726C5C0C2C3C610272D379";
    let dec0 = decrypt_babeltime_cell(online_row0_hex).expect("online.db row 0 hex cell decrypt");
    assert_eq!(dec0, "des,online_seconds,reward\\~,,array");

    let schema = parse_babeltime_schema_row(&dec0);
    assert_eq!(schema.len(), 3);
    assert_eq!(schema[0].name, "des");
    assert_eq!(schema[1].name, "online_seconds");
    assert_eq!(schema[2].name, "reward");

    // Exact row id='1' from online.db
    let online_row1_hex =
        "FEFEFEFE2D000000010700000000000000000000000000000000000000000000136EEC696C65003560706CC3BE7460638FB37674FEE16B83BA6E04E1F6ADC22C6F30B02C367C11377030367C31";
    let dec1 = decrypt_babeltime_cell(online_row1_hex).expect("online.db row 1 hex cell decrypt");
    let parsed1 = parse_babeltime_data_row(&schema, &dec1);
    assert_eq!(parsed1.get("online_seconds").and_then(BtFieldValue::as_i64), Some(300));
}

// 11. Babeltime data row & RewardTriple parsing
#[test]
fn test_11_babeltime_data_row_and_reward_triple_parsing() {
    let schema = parse_babeltime_schema_row("des,online_seconds,reward\\~,,array");
    let row = parse_babeltime_data_row(&schema, "Online 5 phút có thể nhận,300,4|17002|5\\~1|0|150000");

    assert_eq!(row.get("online_seconds").and_then(BtFieldValue::as_i64), Some(300));
    match row.get("reward") {
        Some(BtFieldValue::Array(items)) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], vec!["4", "17002", "5"]);
        }
        other => panic!("unexpected reward field: {other:?}"),
    }

    let triple = RewardTriple::parse("4|22102|1").expect("valid reward triple");
    assert_eq!(triple.reward_type, 4);
    assert_eq!(triple.template_id, 22102);
    assert_eq!(triple.count, 1);
    assert!(is_valid_item_tid(triple.template_id));
}

// 12. Asset path normalization, relative resolution & extension replacement
#[test]
fn test_12_asset_path_utilities() {
    assert_eq!(
        normalize_asset_path("res\\database\\\\..\\database\\./town.db"),
        "res/database/town.db"
    );
    assert_eq!(
        normalize_asset_path("C:\\GCLIENT\\assets\\scripts\\..\\database\\hero.db"),
        "C:/GCLIENT/assets/database/hero.db"
    );
    assert_eq!(
        resolve_relative_asset_path("../dbReader/HeroReader.lua", "scripts/game/modules/HeroUtil.lua"),
        "scripts/game/dbReader/HeroReader.lua"
    );
    assert_eq!(
        replace_file_extension("res/database/town.db", ".json"),
        "res/database/town.json"
    );
}

// 13. Protagonist Male & Female `utid` / `htid` validation (`CreateUserProxy` / `CreateUserScene.lua`)
#[test]
fn test_13_protagonist_templates_male_and_female() {
    assert_eq!(PROTAGONISTS.len(), 2);

    let male = protagonist_by_utid(1).expect("utid=1 Male Protagonist");
    assert_eq!(male.starter_htid, 1);
    assert_eq!(male.sex, 1);
    assert_eq!(male.hero_index_create_ui, 1);
    assert_eq!(male.spine_res_id, 1_000_001);
    assert_eq!(male.exp_curve_id, 1);
    assert_eq!(male.destiny_treasure_id, 305010);
    assert!(is_protagonist_htid(1));
    assert!(is_protagonist_htid(35));

    let female = protagonist_by_utid(2).expect("utid=2 Female Protagonist");
    assert_eq!(female.starter_htid, 2);
    assert_eq!(female.sex, 2);
    assert_eq!(female.hero_index_create_ui, 0);
    assert_eq!(female.spine_res_id, 1_000_002);
    assert_eq!(female.exp_curve_id, 1);
    assert_eq!(female.destiny_treasure_id, 305020);
    assert!(is_protagonist_htid(2));
    assert!(is_protagonist_htid(36));

    assert!(protagonist_by_utid(0).is_none());
    assert!(protagonist_by_utid(3).is_none());
}

// 14. Authentic Three Kingdoms Famous Heroes (`DB_Heroes.getDataById` safety)
#[test]
fn test_14_famous_three_kingdoms_heroes_lookup() {
    assert!(HEROES.len() >= 55);

    let zhao_yun = hero_by_name("Zhao Yun").expect("Zhao Yun");
    assert_eq!(zhao_yun.htid, 201);
    assert_eq!(zhao_yun.name_vi, "Triệu Vân");
    assert_eq!(zhao_yun.kingdom, Kingdom::Shu);
    assert_eq!(zhao_yun.init_quality, 6);
    assert_eq!(zhao_yun.fragment_tid, 50201);
    assert_eq!(zhao_yun.fragment_count, 160);
    assert_eq!(zhao_yun.gold_htid, 2201);
    assert_eq!(zhao_yun.dark_gold_htid, 3201);
    assert_eq!(zhao_yun.purple_gold_htid, 4201);
    assert_eq!(zhao_yun.color_gold_htid, 5201);

    let lu_bu = hero_by_htid(401).expect("Lu Bu htid=401");
    assert_eq!(lu_bu.name_en, "Lu Bu");
    assert_eq!(lu_bu.name_vi, "Lữ Bố");
    assert_eq!(lu_bu.kingdom, Kingdom::Qun);
    assert_eq!(lu_bu.fragment_tid, 50401);

    let guan_yu = hero_by_name("Quan Vũ").expect("Guan Yu by Vietnamese name");
    assert_eq!(guan_yu.htid, 204);
    assert_eq!(guan_yu.gold_htid, 2204);

    let zhuge_liang = hero_by_htid(202).expect("Zhuge Liang htid=202");
    assert_eq!(zhuge_liang.name_en, "Zhuge Liang");

    let cao_cao = hero_by_htid(106).expect("Cao Cao htid=106");
    assert_eq!(cao_cao.kingdom, Kingdom::Wei);

    let zhou_yu = hero_by_htid(301).expect("Zhou Yu htid=301");
    assert_eq!(zhou_yu.kingdom, Kingdom::Wu);

    let diaochan = hero_by_htid(406).expect("Diaochan htid=406");
    assert_eq!(diaochan.name_vi, "Điêu Thuyền");

    // Promoted tier lookup (Gold Zhao Yun = 2201, Color-Gold Lu Bu = 5401)
    assert_eq!(hero_by_htid(2201).map(|h| h.htid), Some(201));
    assert_eq!(hero_by_htid(5401).map(|h| h.htid), Some(401));
    assert!(!is_valid_hero_htid(999_999));
}

// 15. Authentic Consumables, Stamina/Energy Pills & Recruit Tokens
#[test]
fn test_15_authentic_pills_and_recruit_tokens() {
    assert!(ITEMS.len() >= 120);

    let stamina = item_by_tid(22102).expect("Stamina Pill 22102");
    assert_eq!(stamina.name_vi, "Thể Lực Đơn");
    assert_eq!(stamina.category, ItemCategory::StaminaPill);

    let energy = item_by_tid(22202).expect("Energy Pill 22202");
    assert_eq!(energy.name_vi, "Hoạt Lực Đơn");
    assert_eq!(energy.category, ItemCategory::EnergyPill);

    let jr_recruit = item_by_tid(16011).expect("Junior Recruit Token 16011");
    assert_eq!(jr_recruit.category, ItemCategory::RecruitToken);
    let sr_recruit = item_by_tid(16012).expect("Senior Recruit Token 16012");
    assert_eq!(sr_recruit.name_vi, "Chiêu Mộ Lệnh-Cao");

    let tokens = items_by_category(ItemCategory::RecruitToken);
    assert!(tokens.len() >= 6);
}

// 16. Authentic Equipment Chests, Hero Fragments & Red/Gold Gear Suits
#[test]
fn test_16_chests_fragments_and_red_gold_gear_suits() {
    // Gold Chest & Key
    assert_eq!(item_by_tid(30003).unwrap().name_vi, "Quà Hoàng Kim");
    assert_eq!(item_by_tid(30006).unwrap().name_vi, "Hoàng Kim Lệnh");
    // Red Hero Choice Chest
    assert_eq!(item_by_tid(27506).unwrap().name_vi, "Túi Chọn Tướng Đỏ");
    // Zhao Yun & Lu Bu Shards
    assert_eq!(item_by_tid(50201).unwrap().category, ItemCategory::HeroFragment);
    assert_eq!(item_by_tid(50401).unwrap().category, ItemCategory::HeroFragment);

    // Red Gear Suits (601 = Nhiệt huyết bừng bừng, 602 = Long Hoàng Ngạo Thế)
    assert_eq!(GEAR_SUITS.len(), 23);
    let red_suit = gear_suit_by_id(602).expect("Red Suit 602");
    assert_eq!(red_suit.quality, 6);
    assert_eq!(red_suit.suit_items, &[116004, 126004, 136004, 146004]);
    for &arm_tid in red_suit.suit_items {
        assert_eq!(item_by_tid(arm_tid).unwrap().category, ItemCategory::RedGear);
    }

    // Gold Gear Suits (701 = Phượng Linh Thiên Hạ, 702 = Long Diệu Cửu Châu)
    let gold_suit = gear_suit_by_id(701).expect("Gold Suit 701");
    assert_eq!(gold_suit.quality, 7);
    assert_eq!(gold_suit.suit_items, &[117001, 127001, 137001, 147001]);
    for &arm_tid in gold_suit.suit_items {
        assert_eq!(item_by_tid(arm_tid).unwrap().category, ItemCategory::GoldGear);
    }
}

// 17. Authentic `town.db` map IDs & bounds
#[test]
fn test_17_town_templates_and_bounds() {
    assert_eq!(TOWNS.len(), 4);
    let starter = town_by_id(1).expect("Town 1 Đào Nguyên Thôn");
    assert_eq!(starter.name_vi, "Đào Nguyên Thôn");
    assert_eq!(starter.in_level, 0);
    assert_eq!((starter.born_x, starter.born_y), (40, 40));
    assert_eq!(starter.max_x, 2500);

    let luoyang = town_by_id(2).expect("Town 2 Lạc Dương");
    assert_eq!(luoyang.name_vi, "Lạc Dương");
    assert_eq!(luoyang.in_level, 5);

    let world_boss = town_by_id(101).expect("Town 101 Boss TG Trương Giác");
    assert_eq!(world_boss.town_type, 2);

    assert!(is_valid_town_id(1));
    assert!(is_valid_town_id(2));
    assert!(is_valid_town_id(3));
    assert!(is_valid_town_id(101));
    assert!(!is_valid_town_id(99));
}

// 18. Authentic `bag.db` initial capacities & expansion rules
#[test]
fn test_18_bag_capacities() {
    assert_eq!(BAG_LIMITS.len(), 42);

    let hero_bag = bag_by_id(1).expect("Hero Bag id=1");
    assert_eq!(hero_bag.base_capacity, 200);
    assert_eq!(hero_bag.buy_storage_limit, 40);
    assert_eq!(hero_bag.buy_storage_step, 10);
    assert_eq!(hero_bag.max_expandable_capacity(), 600);

    let arm_bag = bag_by_id(2).expect("Equipment Bag id=2");
    assert_eq!(arm_bag.base_capacity, 200);

    let prop_bag = bag_by_id(4).expect("Consumables Bag id=4");
    assert_eq!(prop_bag.bag_type, 2);
    assert_eq!(prop_bag.base_capacity, 0);
    assert_eq!(prop_bag.max_expandable_capacity(), 0);

    let jewel_bag = bag_by_id(7).expect("Jewel Bag id=7");
    assert_eq!(jewel_bag.base_capacity, 100);
    assert_eq!(jewel_bag.max_expandable_capacity(), 600);
}

// 19. Authentic `level_up_exp.db` curves & exact `LevelUpExpReader.lua` parity
#[test]
fn test_19_level_exp_curve_lua_parity() {
    assert_eq!(LEVEL_EXP_CURVES.len(), 8);

    let player_curve = exp_curve_by_id(1).expect("Protagonist curve id=1");
    assert_eq!(player_curve.max_level, 231);
    assert_eq!(player_curve.next_level_need_exp(1), 20);
    assert_eq!(player_curve.next_level_need_exp(2), 40);
    assert_eq!(player_curve.next_level_need_exp(10), 1000);
    assert_eq!(player_curve.next_level_need_exp(100), 3_100_000);
    assert_eq!(player_curve.next_level_need_exp(230), 57_700_070);

    // Cumulative XP: Lv1 = 0, Lv2 = 20, Lv3 = 20 + 40 = 60, Lv4 = 20 + 40 + 60 = 120
    assert_eq!(player_curve.total_exp_for_level(1), 0);
    assert_eq!(player_curve.total_exp_for_level(2), 20);
    assert_eq!(player_curve.total_exp_for_level(3), 60);
    assert_eq!(player_curve.total_exp_for_level(4), 120);

    // Level resolution from total XP
    assert_eq!(player_curve.level_from_total_exp(0, 1), 1);
    assert_eq!(player_curve.level_from_total_exp(19, 1), 1);
    assert_eq!(player_curve.level_from_total_exp(20, 1), 2);
    assert_eq!(player_curve.level_from_total_exp(59, 1), 2);
    assert_eq!(player_curve.level_from_total_exp(60, 1), 3);

    // Red Hero curve (id = 7)
    let red_hero_curve = exp_curve_by_id(7).expect("Red Hero curve id=7");
    assert_eq!(red_hero_curve.next_level_need_exp(1), 190);
    assert_eq!(red_hero_curve.next_level_need_exp(230), 491_420);
}

// 20. Embedded `curated_templates.json` parses cleanly with zero external crates
#[test]
fn test_20_embedded_curated_json_integrity() {
    let doc = embedded_curated_json().expect("embedded curated_templates.json must parse");
    assert_eq!(doc.get("game").and_then(|v| v.as_str()), Some("GServer (vnsg2)"));
    assert_eq!(doc.get("protagonists").and_then(|v| v.as_array()).map(|a| a.len()), Some(2));
    assert_eq!(doc.get("heroes").and_then(|v| v.as_array()).map(|a| a.len()), Some(HEROES.len()));
    assert_eq!(doc.get("items").and_then(|v| v.as_array()).map(|a| a.len()), Some(ITEMS.len()));
    assert_eq!(doc.get("towns").and_then(|v| v.as_array()).map(|a| a.len()), Some(4));
}

// 21. JSON parser Unicode escapes, surrogate pairs, negatives, floats, nested structures & errors
#[test]
#[allow(clippy::approx_constant)]
fn test_21_json_parser_edge_cases() {
    let raw = r#"{"vn":"Tri\u1ec7u V\u00e2n","emoji":"\uD83D\uDE80","neg":-42,"pi":3.1415,"arr":[true,false,null]}"#;
    let val = parse_json(raw).expect("valid RFC 8259 JSON");
    assert_eq!(val.get("vn").and_then(|v| v.as_str()), Some("Triệu Vân"));
    assert_eq!(val.get("emoji").and_then(|v| v.as_str()), Some("🚀"));
    assert_eq!(val.get("neg").and_then(|v| v.as_i64()), Some(-42));
    assert!((val.get("pi").and_then(|v| v.as_f64()).unwrap() - 3.1415).abs() < 1e-9);
    assert!(parse_json(r#"{"unclosed": [1, 2}"#).is_err());
    assert!(parse_json(r#"{"ok": 1} trailing"#).is_err());
}

// 22. End-to-end ExportedDatabaseDir + GameDataStore directory loader test
#[test]
fn test_22_exported_database_dir_and_game_data_store() {
    let tmp_dir: PathBuf = std::env::temp_dir().join(format!("gserver_loader_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(tmp_dir.join("town")).unwrap();
    fs::create_dir_all(tmp_dir.join("hero")).unwrap();

    let manifest = r#"{
      "format_version": 1,
      "generator": "gserver-data/export_db_to_json.py",
      "database_count": 2,
      "table_count": 2,
      "databases": [
        {"database": "town", "source_file": "town.db", "file_encrypted": false, "table_count": 1},
        {"database": "hero", "source_file": "hero.db", "file_encrypted": false, "table_count": 1}
      ],
      "tables": [
        {"database": "town", "table": "town", "relative_path": "town/town.json", "row_count": 1, "column_count": 2},
        {"database": "hero", "table": "heroes", "relative_path": "hero/heroes.json", "row_count": 1, "column_count": 2}
      ]
    }"#;
    fs::write(tmp_dir.join("manifest.json"), manifest).unwrap();

    let town_json = r#"{
      "database": "town",
      "table": "town",
      "columns": [{"name": "name", "kind": "scalar"}],
      "row_count": 1,
      "rows": [{"id": 777, "name": "Custom Admin Town"}]
    }"#;
    fs::write(tmp_dir.join("town/town.json"), town_json).unwrap();

    let hero_json = r#"{
      "database": "hero",
      "table": "heroes",
      "columns": [{"name": "name", "kind": "scalar"}],
      "row_count": 1,
      "rows": [{"id": 8888, "name": "Custom Admin Hero"}]
    }"#;
    fs::write(tmp_dir.join("hero/heroes.json"), hero_json).unwrap();

    let exported = ExportedDatabaseDir::load_eager(&tmp_dir).expect("load exported dir");
    assert_eq!(exported.loaded_table_count(), 2);
    assert_eq!(
        exported
            .get_row_by_id("town", "town", 777)
            .and_then(|r| r.get("name"))
            .and_then(|v| v.as_str()),
        Some("Custom Admin Town")
    );

    let store = GameDataStore::from_export_dir(&tmp_dir).expect("store from export dir");
    // Embedded templates work
    assert!(store.contains_hero_htid(201));
    assert!(store.contains_item_tid(22102));
    assert!(store.contains_town_id(1));
    // Custom exported DB IDs also work!
    assert!(store.contains_hero_htid(8888));
    assert!(store.contains_town_id(777));

    let _ = fs::remove_dir_all(&tmp_dir);
}

// 23. Cross-check that every Hero's fragment_tid & exp_curve_id is valid
#[test]
fn test_23_hero_exp_curves_and_fragments_consistency() {
    for hero in HEROES {
        assert!(
            exp_curve_by_id(hero.exp_curve_id).is_some(),
            "Hero {} ({}) references missing exp_curve_id={}",
            hero.htid,
            hero.name_en,
            hero.exp_curve_id
        );
        assert!(hero.fragment_tid >= 50101, "Hero {} missing authentic fragment_tid", hero.htid);
        assert!(hero.fragment_count >= 40, "Hero {} invalid fragment_count", hero.htid);
    }
}

// 24. Real SQLite `babeltime` hex-encoded cell decryption from `town.db` (KEY_08, Đào Nguyên Thôn)
#[test]
fn test_24_decrypt_real_sqlite_town_db_hex_cells() {
    let town_row0_hex =
        "FEFEFEFEA40000000108000000000000000000000000000000000000000000004261CB65936E0F633E6921737B741977315FC27989656F624072515F1D64AA69425FCA65C965132C1265205F39751B623A729A63986C2F5F417552621172AA6D4D78F979936D166E3E79696D3E6E2978736DD778A6786F6E4A614D5F1667D97249738A66DE722062065F3765242C056F2A6ED25F8B65302C5B6F486E2B70EA6F585FCF64CC5C012C007237612E2C5A612D72D779D52C6F2C032C132C582CAA615E72C779";
    let dec0 = decrypt_babeltime_cell(town_row0_hex).expect("town.db row 0 hex cell decrypt");
    let schema = parse_babeltime_schema_row(&dec0);
    assert!(schema.iter().any(|c| c.name == "name"));
    assert!(schema.iter().any(|c| c.name == "born_id"));
    assert!(schema.iter().any(|c| c.name == "in_level"));

    let town_row1_hex =
        "FEFEFEFE5C000000010800000000000000000000000000000000000000000000EB90FCA01B20C867597965AAD1202B68A2B42B2C6630465C21318730A57E73321E5C41304433DA7E1D30965CC1304F303D7E7530675C08306F309A31D534735C51340F2C442CB7300031962C8C354F2C512C752C65354630732C9A2C";
    let dec1 = decrypt_babeltime_cell(town_row1_hex).expect("town.db row 1 hex cell decrypt");
    let row1 = parse_babeltime_data_row(&schema, &dec1);
    assert_eq!(row1.get("name").and_then(BtFieldValue::as_str), Some("Đào Nguyên Thôn"));
    assert_eq!(row1.get("in_level").and_then(BtFieldValue::as_i64), Some(0));
    assert_eq!(row1.get("max_x").and_then(BtFieldValue::as_i64), Some(2500));
}

// 25. Plaintext cell pass-through & corrupted `FEFEFEFE` hex cell error handling (`CTable::getRowById` parity)
#[test]
fn test_25_babeltime_cell_plaintext_and_corrupted_header_errors() {
    // Unencrypted plaintext cells pass through unchanged
    assert_eq!(
        decrypt_babeltime_cell("Đào Nguyên Thôn,40|40").unwrap(),
        "Đào Nguyên Thôn,40|40"
    );

    // 32-byte `FEFEFEFE` header claiming 99 payload bytes when 0 payload bytes follow -> PayloadLengthMismatch
    let bad_len_hex = "FEFEFEFE63000000010700000000000000000000000000000000000000000000";
    let err_len = decrypt_babeltime_cell(bad_len_hex).expect_err("corrupt header payload_len must fail");
    assert_eq!(err_len.status, DecryptStatus::PayloadLengthMismatch);

    // 32-byte `FEFEFEFE` header with key_index = 0x20 (out of range) -> InvalidKeyIndex
    let bad_key_hex = "FEFEFEFE00000000012000000000000000000000000000000000000000000000";
    let err_key = decrypt_babeltime_cell(bad_key_hex).expect_err("invalid key_index must fail");
    assert_eq!(err_key.status, DecryptStatus::InvalidKeyIndex);
}

// 26. Programmatic validation report across all 801 `timeMachine_hero` rows & `curated_templates.json`
#[test]
fn test_26_validate_all_801_hero_skill_mappings_summary_report() {
    let report = gserver_data::validate_embedded_hero_skill_mappings();
    assert_eq!(report.total_templates_inspected, 801);
    assert_eq!(report.valid_normal_skill_count, 799);
    assert_eq!(report.valid_rage_skill_count, 799);
    assert_eq!(report.fully_valid_hero_count, 799);
    assert_eq!(report.missing_or_invalid_template_count, 2);
    assert_eq!(report.missing_or_invalid_skill_ref_count, 4);
    assert_eq!(report.fallback_template_count, 2);
    assert_eq!(report.fallback_skill_slot_count, 4);
    assert_eq!(report.validated_skill_set_size, 2270);

    // Exactly 2 templates in `timeMachine_hero` explicitly have null `com_atk`/`angry_atk`:
    // htid=667 (`Chân-Thủy Kính`, is_show=false) and htid=1002 (`Ma Long`, event dragon).
    let missing_ids: Vec<u32> = report
        .missing_or_invalid_details
        .iter()
        .map(|d| d.htid)
        .collect();
    assert_eq!(missing_ids, vec![667, 1002]);
    for d in &report.missing_or_invalid_details {
        assert_eq!(
            d.normal_error,
            Some(gserver_data::SkillIdValidationError::Missing)
        );
        assert_eq!(
            d.rage_error,
            Some(gserver_data::SkillIdValidationError::Missing)
        );
        assert_eq!(
            d.resolved_normal_skill_id,
            gserver_data::FALLBACK_NORMAL_SKILL_ID
        );
        assert_eq!(
            d.resolved_rage_skill_id,
            gserver_data::FALLBACK_RAGE_SKILL_ID
        );
    }

    // 100% of curated protagonists (`1..=36`) and curated hero tiers (330 distinct htids) are valid
    assert_eq!(report.curated_protagonist_templates_count, 2);
    assert_eq!(report.curated_protagonist_htids_inspected, 36);
    assert_eq!(report.curated_protagonist_valid_normal_count, 36);
    assert_eq!(report.curated_protagonist_valid_rage_count, 36);
    assert_eq!(report.curated_hero_templates_count, 59);
    assert_eq!(report.curated_hero_tier_htids_inspected, 330);
    assert_eq!(report.curated_hero_tier_valid_normal_count, 330);
    assert_eq!(report.curated_hero_tier_valid_rage_count, 330);

    // Verify `HERO_SKILL_ENTRIES` and `VALIDATED_SKILL_IDS` are strictly sorted for binary search
    let entries = gserver_data::all_hero_skill_entries();
    assert!(entries.windows(2).all(|w| w[0].htid < w[1].htid));
    let skill_ids = gserver_data::all_validated_skill_ids();
    assert!(skill_ids.windows(2).all(|w| w[0] < w[1]));
}

// 27. Table-driven representative hero & multi-tier skill resolution test
#[test]
fn test_27_table_driven_authentic_hero_and_tier_battle_skills() {
    // (htid, awake_rank, expected_normal, expected_rage, label)
    let cases: &[(u32, u32, u32, u32, &str)] = &[
        // Protagonist chains (`1..=36`)
        (1, 0, 102, 101, "Nam Chủ Tướng starter"),
        (2, 0, 104, 103, "Nữ Chủ Tướng starter"),
        (13, 0, 320, 323, "Nam Chủ Tướng Red"),
        (14, 0, 322, 324, "Nữ Chủ Tướng Red"),
        (17, 0, 335, 333, "Nam Chủ Tướng Gold"),
        (18, 0, 336, 334, "Nữ Chủ Tướng Gold"),
        (21, 0, 335, 339, "Nam Chủ Tướng Dark-Gold"),
        (23, 0, 337, 339, "Nam Chủ Tướng Dark-Gold II"),
        (25, 0, 341, 343, "Nam Chủ Tướng Purple-Gold"),
        (29, 0, 345, 347, "Nam Chủ Tướng Color-Gold"),
        (33, 0, 349, 351, "Nam Chủ Tướng Nirvana-Gold"),
        (35, 0, 349, 351, "Nam Chủ Tướng Nirvana-Gold II"),
        (36, 0, 350, 352, "Nữ Chủ Tướng Nirvana-Gold II"),
        // Base & promoted famous Three Kingdoms heroes
        (101, 0, 10102, 10101, "Trương Liêu base"),
        (
            101,
            5,
            5210102,
            5210101,
            "Trương Liêu awakened (awake_rank=5)",
        ),
        (601, 0, 5210102, 5210101, "Chân-Trương Liêu"),
        (1101, 0, 10102, 10101, "Trương Liêu Red"),
        (2101, 0, 5310102, 5310101, "Thánh-Trương Liêu (Gold)"),
        (3101, 0, 5410102, 5410101, "Thần-Trương Liêu (Dark-Gold)"),
        (4101, 0, 5510102, 5510101, "Ma-Trương Liêu (Purple-Gold)"),
        (5101, 0, 5610102, 5610101, "Tiên-Trương Liêu (Color-Gold)"),
        (6101, 0, 5610102, 5610101, "Tray-Gold Trương Liêu inherited"),
        (8101, 0, 5610102, 5610101, "Iron-Gold Trương Liêu inherited"),
        (102, 0, 10202, 10201, "Tư Mã Ý"),
        (103, 0, 10302, 10301, "Quách Gia base"),
        (1103, 0, 110302, 110301, "Quách Gia Red evolved"),
        (7103, 0, 5610302, 5610301, "Gild-Gold Quách Gia inherited"),
        (106, 0, 10602, 10601, "Tào Tháo"),
        (201, 0, 20102, 20101, "Triệu Vân base"),
        (2201, 0, 5320102, 5320101, "Thánh-Triệu Vân"),
        (5201, 0, 5620102, 5620101, "Tiên-Triệu Vân"),
        (202, 0, 20202, 20201, "Gia Cát Lượng"),
        (204, 0, 20402, 20401, "Quan Vũ"),
        (301, 0, 30102, 30101, "Chu Du"),
        (310, 0, 31002, 31001, "Tiểu Kiều"),
        (312, 0, 31202, 31201, "Tôn Quyền"),
        (401, 0, 40102, 40101, "Lữ Bố"),
        (5401, 0, 5640102, 5640101, "Tiên-Lữ Bố"),
        (402, 0, 40202, 40201, "Tả Từ"),
        (406, 0, 40602, 40601, "Điêu Thuyền"),
        (426, 0, 42602, 42601, "Tân Hiến Anh starter vanguard"),
        (901, 0, 90102, 90101, "Thủy Kính"),
        // Non-trivial irregular IDs proving table lookup vs arithmetic formula
        (154, 0, 600502, 600501, "Vương Lãng (Blue gacha)"),
        (168, 0, 5402, 5401, "Hạ Hầu Ân (Green gacha)"),
    ];

    for &(htid, awake_rank, exp_normal, exp_rage, label) in cases {
        let res = gserver_data::resolve_hero_battle_skills_with_awake(htid, awake_rank);
        assert!(
            !res.used_any_fallback(),
            "{label} (htid={htid}) must not use fallback"
        );
        assert_ne!(
            res.normal_skill_id, 1001,
            "{label} normal skill must not be generic 1001"
        );
        assert_ne!(
            res.rage_skill_id, 2001,
            "{label} rage skill must not be generic 2001"
        );
        assert_eq!(res.normal_skill_id, exp_normal, "{label} com_atk mismatch");
        assert_eq!(res.rage_skill_id, exp_rage, "{label} angry_atk mismatch");
        assert!(gserver_data::is_validated_skill_id(res.normal_skill_id));
        assert!(gserver_data::is_validated_skill_id(res.rage_skill_id));
    }

    // Verify ProtagonistTemplate and HeroTemplate struct fields match embedded skill table
    for p in PROTAGONISTS {
        let sh = gserver_data::resolve_hero_battle_skills(p.starter_htid);
        assert_eq!(
            (p.com_atk, p.angry_atk),
            (sh.normal_skill_id, sh.rage_skill_id)
        );
    }
    for h in HEROES {
        let bh = gserver_data::resolve_hero_battle_skills(h.htid);
        assert_eq!(
            (h.com_atk, h.angry_atk),
            (bh.normal_skill_id, bh.rage_skill_id)
        );
    }
}

// 28. Explicit validation of missing, zero, negative, overflow, unvalidated, and fallback diagnostics
#[test]
fn test_28_skill_validation_edge_cases_and_fallback_diagnostics() {
    use gserver_data::{
        hero_skill_fallback_slot_count, hero_skill_fallback_template_count,
        monster_default_skill_unit_count, resolve_hero_battle_skills,
        resolve_unit_or_monster_battle_skills, validate_raw_skill_id, HeroSkillFallbackReason,
        HeroSkillResolutionSource, SkillIdValidationError, FALLBACK_NORMAL_SKILL_ID,
        FALLBACK_RAGE_SKILL_ID,
    };

    assert_eq!(
        validate_raw_skill_id(None),
        Err(SkillIdValidationError::Missing)
    );
    assert_eq!(
        validate_raw_skill_id(Some(0)),
        Err(SkillIdValidationError::Zero)
    );
    assert_eq!(
        validate_raw_skill_id(Some(-7)),
        Err(SkillIdValidationError::Negative(-7))
    );
    assert_eq!(
        validate_raw_skill_id(Some(i64::from(u32::MAX) + 5)),
        Err(SkillIdValidationError::OutOfRange(i64::from(u32::MAX) + 5))
    );
    assert_eq!(
        validate_raw_skill_id(Some(999_999)),
        Err(SkillIdValidationError::Unvalidated(999_999))
    );
    assert_eq!(validate_raw_skill_id(Some(20102)), Ok(20102));

    let before_tpl = hero_skill_fallback_template_count();
    let before_slots = hero_skill_fallback_slot_count();

    let zero_res = resolve_hero_battle_skills(0);
    assert!(zero_res.used_any_fallback());
    assert_eq!(
        zero_res.source,
        HeroSkillResolutionSource::Fallback(HeroSkillFallbackReason::ZeroOrInvalidHtid { htid: 0 })
    );
    assert_eq!(
        (zero_res.normal_skill_id, zero_res.rage_skill_id),
        (FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID)
    );

    let unknown_res = resolve_hero_battle_skills(20_008);
    assert!(unknown_res.used_any_fallback());
    assert_eq!(
        unknown_res.source,
        HeroSkillResolutionSource::Fallback(HeroSkillFallbackReason::UnknownHeroTemplate {
            htid: 20_008
        })
    );

    let malong_res = resolve_hero_battle_skills(1002);
    assert!(malong_res.used_any_fallback());
    assert_eq!(
        malong_res.source,
        HeroSkillResolutionSource::Fallback(HeroSkillFallbackReason::InvalidSkillReference {
            htid: 1002,
            normal_error: Some(SkillIdValidationError::Missing),
            rage_error: Some(SkillIdValidationError::Missing),
        })
    );

    assert!(hero_skill_fallback_template_count() >= before_tpl + 3);
    assert!(hero_skill_fallback_slot_count() >= before_slots + 6);

    // Monster vs Hero unit resolution
    let before_mon = monster_default_skill_unit_count();
    assert_eq!(resolve_unit_or_monster_battle_skills(310), (31002, 31001));
    assert_eq!(
        resolve_unit_or_monster_battle_skills(5001),
        (FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID)
    );
    assert!(monster_default_skill_unit_count() > before_mon);
}

// 29. Runtime `GameDataStore` JSON table loading & validation for custom/invalid skill rows
#[test]
fn test_29_runtime_game_data_store_hero_skill_loading_and_validation() {
    use gserver_data::{
        HeroSkillFallbackReason, HeroSkillResolutionSource, SkillIdValidationError,
        FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID,
    };

    let tmp_dir: PathBuf =
        std::env::temp_dir().join(format!("gserver_skill_loader_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(tmp_dir.join("timeMachine_hero")).unwrap();
    fs::create_dir_all(tmp_dir.join("skill")).unwrap();

    let manifest = r#"{
      "format_version": 1,
      "generator": "gserver-data/export_db_to_json.py",
      "database_count": 2,
      "table_count": 2,
      "databases": [
        {"database": "timeMachine_hero", "source_file": "timeMachine_hero.db", "file_encrypted": false, "table_count": 1},
        {"database": "skill", "source_file": "skill.db", "file_encrypted": false, "table_count": 1}
      ],
      "tables": [
        {"database": "timeMachine_hero", "table": "timeMachine_hero", "relative_path": "timeMachine_hero/timeMachine_hero.json", "row_count": 4, "column_count": 4},
        {"database": "skill", "table": "skill", "relative_path": "skill/skill.json", "row_count": 2, "column_count": 1}
      ]
    }"#;
    fs::write(tmp_dir.join("manifest.json"), manifest).unwrap();

    let skill_json = r#"{
      "database": "skill",
      "table": "skill",
      "columns": [{"name": "name", "kind": "scalar"}],
      "row_count": 2,
      "rows": [
        {"id": 990002, "name": "Custom Mod Normal Skill"},
        {"id": 990001, "name": "Custom Mod Rage Skill"}
      ]
    }"#;
    fs::write(tmp_dir.join("skill/skill.json"), skill_json).unwrap();

    let tm_hero_json = r#"{
      "database": "timeMachine_hero",
      "table": "timeMachine_hero",
      "columns": [
        {"name": "name", "kind": "scalar"},
        {"name": "com_atk", "kind": "scalar"},
        {"name": "angry_atk", "kind": "scalar"}
      ],
      "row_count": 4,
      "rows": [
        {"id": 201, "name": "Triệu Vân", "com_atk": 20102, "angry_atk": 20101},
        {"id": 9501, "name": "Custom Mod Hero", "com_atk": 990002, "angry_atk": 990001},
        {"id": 9502, "name": "Zero & Negative Hero", "com_atk": 0, "angry_atk": -5},
        {"id": 9503, "name": "UnvalidatedRageHero", "com_atk": 20102, "angry_atk": 888888}
      ]
    }"#;
    fs::write(
        tmp_dir.join("timeMachine_hero/timeMachine_hero.json"),
        tm_hero_json,
    )
    .unwrap();

    let store = GameDataStore::from_export_dir(&tmp_dir).expect("load runtime skill store");
    assert!(store.contains_skill_id(990002));
    assert!(store.contains_skill_id(20102));
    assert!(!store.contains_skill_id(0));
    assert!(!store.contains_skill_id(888888));

    // Custom mod hero resolves custom validated skills from `skill/skill.json`
    let mod_res = store.resolve_hero_battle_skills(9501, 0);
    assert!(!mod_res.used_any_fallback());
    assert_eq!(
        (mod_res.normal_skill_id, mod_res.rage_skill_id),
        (990002, 990001)
    );
    assert_eq!(
        mod_res.source,
        HeroSkillResolutionSource::ExportedDbRow {
            htid: 9501,
            awakened_override: false,
        }
    );

    // Zero & negative skills fall back cleanly with exact error diagnostics
    let bad_zero_neg = store.resolve_hero_battle_skills(9502, 0);
    assert!(bad_zero_neg.used_normal_fallback && bad_zero_neg.used_rage_fallback);
    assert_eq!(
        (bad_zero_neg.normal_skill_id, bad_zero_neg.rage_skill_id),
        (FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID)
    );
    assert_eq!(
        bad_zero_neg.source,
        HeroSkillResolutionSource::Fallback(HeroSkillFallbackReason::InvalidSkillReference {
            htid: 9502,
            normal_error: Some(SkillIdValidationError::Zero),
            rage_error: Some(SkillIdValidationError::Negative(-5)),
        })
    );

    // Partial fallback: valid com_atk preserved (`20102`), invalid angry_atk falls back (`2001`)
    let partial = store.resolve_hero_battle_skills(9503, 0);
    assert!(!partial.used_normal_fallback && partial.used_rage_fallback);
    assert_eq!(partial.normal_skill_id, 20102);
    assert_eq!(partial.rage_skill_id, FALLBACK_RAGE_SKILL_ID);

    // Non-overridden embedded hero `401` (Lữ Bố) still works seamlessly
    let lubu = store.resolve_hero_battle_skills(401, 0);
    assert_eq!((lubu.normal_skill_id, lubu.rage_skill_id), (40102, 40101));

    let report = store.validate_hero_skill_mappings();
    assert_eq!(report.total_templates_inspected, 4);
    assert_eq!(report.valid_normal_skill_count, 3);
    assert_eq!(report.valid_rage_skill_count, 2);
    assert_eq!(report.fully_valid_hero_count, 2);
    assert_eq!(report.missing_or_invalid_template_count, 2);
    assert_eq!(report.missing_or_invalid_skill_ref_count, 3);
    assert_eq!(report.fallback_template_count, 2);
    assert_eq!(report.fallback_skill_slot_count, 3);

    let _ = fs::remove_dir_all(&tmp_dir);
}
