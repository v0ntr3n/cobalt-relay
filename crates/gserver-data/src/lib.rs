#![forbid(unsafe_code)]
//! # `gserver-data`
//!
//! Pure-Rust, zero-dependency asset decryptor (`bt_decrypt`) and authentic game
//! configuration template loader for **GServer (`vnsg2` / Babeltime Sanguo 2)**.
//!
//! ## Key Capabilities
//! 1. **100% Safe `bt_decrypt` Port (`0xFEFEFEFE` Container & SQLite Cell Decryptor)**:
//!    - Exact port of `bt_decrypt` (`0x340ef4`) and `decrypt_basic` (`0x340e4c`) from
//!      `libcocos2dlua.so` using all 16 real key slots from `07_bt_decrypt_keys.inc`.
//!    - Supports in-place header-stripping decryption, byte-slice decryption, symmetric
//!      encryption for roundtrip verification, and SQLite `babeltime` hex-cell decryption
//!      matching `CConfigManager.cpp` (`CTable::getRowById`).
//! 2. **Authentic Extracted Starter, Core & Battle Skill Templates**:
//!    - Zero-allocation `const` tables (`PROTAGONISTS`, `HEROES`, `HERO_SKILL_ENTRIES`,
//!      `VALIDATED_SKILL_IDS`, `ITEMS`, `GEAR_SUITS`, `TOWNS`, `BAG_LIMITS`, `LEVEL_EXP_CURVES`)
//!      plus embedded `curated_templates.json` extracted from the real `vnsg2` client SQLite
//!      `.db` files so private servers never send invalid `utid`, `htid`, `arrSkill`,
//!      `item_template_id`, `town_id`, or `exp_id` values.
//! 3. **Runtime Client `database/` Export Directory Loader**:
//!    - Companion `tools/export_db_to_json.py` decrypts and exports any client `database/`
//!      folder into structured JSON tables + `manifest.json`, which [`ExportedDatabaseDir`]
//!      and [`GameDataStore`] load with zero external Rust crate dependencies.

pub mod bt_decrypt;
pub mod bt_row;
pub mod generated_consts;
pub mod generated_hero_skills;
pub mod hero_skills;
pub mod json;
pub mod loader;
pub mod templates;

pub use bt_decrypt::{
    bt_decrypt, bt_decrypt_strict, bt_decrypt_vec, bt_encrypt_vec, clear_runtime_key_tables,
    decrypt_babeltime_cell, decrypt_basic_in_place, decrypt_buffer_in_place, has_decrypt_keys,
    is_bt_encrypted_buffer, normalize_asset_path, register_key_table, replace_file_extension,
    resolve_relative_asset_path, DecryptResult, DecryptStatus, BT_DECRYPT_HEADER_SIZE,
    BT_DECRYPT_MAGIC, BT_DEFAULT_KEY_TABLES, BT_KEY_TABLE_COUNT, BT_MAX_KEY_INDEX,
    BT_MAX_KEY_LENGTH,
};
pub use generated_consts::{
    BAG_LIMITS, CURRENT_VERSION_LEVEL_LIMIT, DEFAULT_STARTER_TOWN_ID, GEAR_SUITS,
    GLOBAL_ENERGY_LIMIT, GLOBAL_STAMINA_LIMIT, HEROES, ITEMS, LEVEL_EXP_CURVES, LUOYANG_TOWN_ID,
    PROTAGONISTS, TOWNS,
};
pub use generated_hero_skills::{HERO_SKILL_ENTRIES, VALIDATED_SKILL_IDS};
pub use hero_skills::{
    all_hero_skill_entries, all_validated_skill_ids, find_inherited_tier_skill_entry,
    hero_skill_entry_by_htid, hero_skill_fallback_slot_count, hero_skill_fallback_template_count,
    is_validated_skill_id, monster_default_skill_unit_count, resolve_hero_battle_skills,
    resolve_hero_battle_skills_with_awake, resolve_unit_or_monster_battle_skills,
    validate_embedded_hero_skill_mappings, validate_raw_skill_id, validate_raw_skill_id_with_extra,
    HeroSkillEntry, HeroSkillFallbackReason, HeroSkillResolutionSource, HeroSkillValidationReport,
    InvalidHeroSkillDetail, ResolvedHeroBattleSkills, SkillIdValidationError,
    AWAKE_SKILL_CHANGE_RANK, FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID,
};
pub use json::{parse_json, JsonError, JsonValue};
pub use loader::{ExportedDatabaseDir, ExportedTable, GameDataStore, LoaderError, TableManifestEntry};
pub use templates::{
    bag_by_id, embedded_curated_json, exp_curve_by_id, gear_suit_by_id, hero_by_htid,
    hero_by_name, is_protagonist_htid, is_valid_hero_htid, is_valid_item_tid, is_valid_town_id,
    item_by_tid, items_by_category, protagonist_by_utid, town_by_id, BagLimitTemplate,
    GearSuitTemplate, HeroTemplate, ItemCategory, ItemTemplate, Kingdom, LevelExpCurveTemplate,
    ProtagonistTemplate, RewardCurrencyType, TownTemplate,
};
