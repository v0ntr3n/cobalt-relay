#![forbid(unsafe_code)]
//! # `gserver-player`
//!
//! Zero-dependency, pure-Rust player state, Lua wire-schema projection, progression
//! mutation, and crash-safe persistence engine for **GServer (`vnsg2`)**.
//!
//! ## Core Guarantees
//! - **`#![forbid(unsafe_code)]` & Zero External Dependencies**: Built strictly on Rust `std`,
//!   including a deterministic RFC 8259 JSON parser/serializer and IEEE 802.3 CRC32 WAL verifier.
//! - **Exact `vnsg2` Lua Model Compatibility**:
//!   - `UserData.lua`: `copper` & `fight_force_max` serialized as numeric values within Lua double
//!     precision (`<= 2^53 - 1`); all arithmetic currencies (`gold`, `gold_bind`, `hero_soul`/`soul_num`,
//!     `prestige`/`arena_coin`, `tower_num`, `jewel`/`awake_coin`, `clan_contri`, `godarm_coin`,
//!     `pet_coin`, `god_animal_coin`, `world_coin`, `execution`, `energy`/`stamina`) and recovery
//!     timestamps emitted as numbers; 14-char `"YYYYMMDDHHMMSS"` `server_open_time`; valid `dress_id` /
//!     `attr_dress_id` present inside `dress_info`; persistent `raffle_state`.
//!   - `HeroInfo.lua`: numeric `hid`, `htid`, `level`, `exp`, `evolve_level`, `upgrade_level`,
//!     `destiny`, `awake_level`, `lock` (`0`/`1`), `pill`, `skin_id`, plus `va_hero = { star, wash, dress }`
//!     (without `arming` inside `va_hero`).
//!   - `HeroFormationData.lua` & `FormationData.lua`: `position` containing only occupied `hid > 0`
//!     entries, 6-cell battle `formation` (`{2, 3, 5, 6, 8, 9}`), `partner`, `buddy`, `buddyLevelInfo`,
//!     6-slot `arming` (`"1"`..`"9"`), `spirit`, `yanWu`, `brochure`, `soulSkill`, `soulStone`,
//!     `soulJade`, full `BagItem` wire dicts for `godanimal`/`topanimal`/`protection`, and
//!     `to_vnsg2_other_position_info`.
//!   - `BagData.lua` & `ItemFactory.lua`: `daily_use`, `gridStart`, `gridMaxNum`, `gridOpenTimes`,
//!     all 42 sub-bag tables (`10001..740000`), complete `va_item_text` defaults, and `{}` empty-map
//!     `BagDelta` deletion payloads.
//! - **Crash-Safe Persistence**: Append-only CRC32-checksummed JSONL Write-Ahead Log
//!   (`data/wal/operations.wal`) paired with atomic temp-file + `File::sync_all` + `fs::rename`
//!   snapshots (`data/accounts.json`, `data/players/<uid>.json`).

pub mod bag;
pub mod error;
pub mod formation;
pub mod json;
pub mod models;
pub mod mutations;
pub mod store;
pub mod wal;

pub use bag::{
    classify_template_id, default_va_item_text, BagDelta, BagItem, BagType, InventoryState,
    ItemClass,
};
pub use error::{PlayerError, Result};
pub use formation::{
    is_valid_battle_grid_cell, EquipSlotType, FormationBookState, FormationState,
    DEFAULT_SQUAD_TO_GRID, VALID_BATTLE_GRID_CELLS,
};
pub use json::{JsonObjReader, JsonValue};
pub use models::{
    to_vnsg2_other_position_info, vip_level_from_exp, Account, CurrencyType, EquippedItem, Gender,
    HeroInstance, PlayerProfile, StarterHeroSeed, StarterItemSeed, StarterPackConfig,
    MAX_LUA_SAFE_INT, VIP_RECHARGE_REQUIRE,
};
pub use mutations::{
    ArmReinforceOutcome, ArmReinforceStep, ArmUpgradeOutcome, AwakeUpgradeOutcome,
    BagExpandOutcome, ComposeHeroOutcome, ComposeItemOutcome, ExploreSoulOutcome, GodArmAction,
    HeroEvolveVnsg2Outcome, HeroLevelUpOutcome, HeroLevelUpVnsg2Outcome, OpenBoxOutcome,
    RefineryDecomposeKind, RefineryOutcome, RefineryRebornKind, RefinerySellKind,
    RefinerySellOutcome, SellBagEntry, SellBagOutcome, SpiritReinforceOutcome,
    TreasureReinforceOutcome, TreasureUpgradeOutcome, UseBagItemOutcome, UseItemEffect,
    UseItemOutcome,
};
pub use store::{atomic_write_bytes, PlayerStore, RecoveryReport};
pub use wal::{crc32_ieee, WalOp, WalRecord, WalScanOutcome, WriteAheadLog};
