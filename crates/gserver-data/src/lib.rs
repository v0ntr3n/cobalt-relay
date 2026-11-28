#![forbid(unsafe_code)]
//! # `gserver-data`
//!
//! Pure-Rust asset decryptor (`bt_decrypt`, re-exported from [`gserver_protocol`]) and authentic
//! game configuration template loader for **GServer (`vnsg2` / Babeltime Sanguo 2)**.

pub mod bt_decrypt {
    pub use gserver_protocol::bt_decrypt::{
        bt_decrypt_in_place, bt_decrypt_in_place as bt_decrypt, bt_decrypt_or_passthrough,
        bt_decrypt_strict, bt_decrypt_vec, bt_encrypt, bt_encrypt_vec, clear_runtime_key_tables,
        decrypt_babeltime_cell, decrypt_basic_in_place, decrypt_buffer_in_place, has_bt_magic,
        has_decrypt_keys, is_bt_encrypted_buffer, normalize_asset_path, register_key_table,
        replace_file_extension, resolve_relative_asset_path, DecryptResult, DecryptStatus, AP_1,
        BT_DECRYPT_HEADER_SIZE, BT_DECRYPT_MAGIC, BT_DEFAULT_KEY_TABLES, BT_KEY_TABLES,
        BT_KEY_TABLE_COUNT, BT_MAX_KEY_INDEX, BT_MAX_KEY_LENGTH, KEY_00, KEY_01, KEY_02, KEY_03,
        KEY_04, KEY_05, KEY_06, KEY_07, KEY_08, KEY_09, KEY_0A, KEY_0B, KEY_0C, KEY_0D, KEY_0E,
        KEY_0F, PRIME_INDICES,
    };
}
pub mod bt_row;
pub mod generated_consts;
pub use gserver_protocol::json;
pub mod loader;
pub mod templates;

pub use bt_decrypt::{
    bt_decrypt, bt_decrypt_in_place, bt_decrypt_strict, bt_decrypt_vec, bt_encrypt_vec,
    clear_runtime_key_tables, decrypt_babeltime_cell, decrypt_basic_in_place,
    decrypt_buffer_in_place, has_decrypt_keys, is_bt_encrypted_buffer, normalize_asset_path,
    register_key_table, replace_file_extension, resolve_relative_asset_path, DecryptResult,
    DecryptStatus, BT_DECRYPT_HEADER_SIZE, BT_DECRYPT_MAGIC, BT_DEFAULT_KEY_TABLES,
    BT_KEY_TABLE_COUNT, BT_MAX_KEY_INDEX, BT_MAX_KEY_LENGTH,
};
pub use generated_consts::{
    BAG_LIMITS, CURRENT_VERSION_LEVEL_LIMIT, DEFAULT_STARTER_TOWN_ID, GEAR_SUITS,
    GLOBAL_ENERGY_LIMIT, GLOBAL_STAMINA_LIMIT, HEROES, ITEMS, LEVEL_EXP_CURVES, LUOYANG_TOWN_ID,
    PROTAGONISTS, TOWNS,
};
pub use json::{parse_json, JsonError, JsonNumber, JsonObject, JsonParseError, JsonValue};
pub use loader::{
    ExportedDatabaseDir, ExportedTable, GameDataStore, LoaderError, TableManifestEntry,
};
pub use templates::{
    bag_by_id, embedded_curated_json, exp_curve_by_id, gear_suit_by_id, hero_by_htid,
    hero_by_name, is_protagonist_htid, is_valid_hero_htid, is_valid_item_tid, is_valid_town_id,
    item_by_tid, items_by_category, protagonist_by_utid, town_by_id, BagLimitTemplate,
    GearSuitTemplate, HeroTemplate, ItemCategory, ItemTemplate, Kingdom, LevelExpCurveTemplate,
    ProtagonistTemplate, RewardCurrencyType, TownTemplate,
};
