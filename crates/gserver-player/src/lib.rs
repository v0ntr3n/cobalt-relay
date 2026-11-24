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
//!   - `UserData.lua`: `copper` & `fight_force_max` serialized as `tonumber(...)`-safe numeric strings;
//!     all arithmetic currencies (`gold`, `gold_bind`, `hero_soul`, `prestige`, `execution`, `energy`,
//!     shop coins) and recovery timestamps (`execution_time`, `energy_time`, `hero_bag_buy_times`)
//!     emitted as numbers; 14-char `"YYYYMMDDHHMMSS"` `server_open_time`; valid `dress_id` /
//!     `attr_dress_id` present inside `dress_info`.
//!   - `HeroInfo.lua`: numeric `hid`, `htid`, `level`, `exp`, `evolve_level`, `upgrade_level`, plus
//!     guaranteed non-nil `va_fail_reward`, wash tables, and promotion rank/level fields.
//!   - `HeroFormationData.lua` & `FormationData.lua`: `position` containing only occupied `hid > 0`
//!     entries, 9-cell battle `formation`, `partner` (`LittleFriend`), `buddy`, 6-slot `arming`
//!     (`"1"`..`"9"`), and mandatory non-nil tables (`bingfu`, `jointSkill`, `godanimal`, `topanimal`,
//!     `formationBook`).
//!   - `BagData.lua` & `ItemFactory.lua`: `daily_use`, `gridStart`, `gridMaxNum`, `gridOpenTimes`,
//!     all 42 sub-bag tables (`10001..740000`) plus classic sub-bag aliases so Lua `pairs(...)`
//!     never encounters `nil`.
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
    classify_template_id, default_va_item_text, BagItem, BagType, InventoryState, ItemClass,
};
pub use error::{PlayerError, Result};
pub use formation::{EquipSlotType, FormationBookState, FormationState, DEFAULT_SQUAD_TO_GRID};
pub use json::{JsonObjReader, JsonValue};
pub use models::{
    Account, CurrencyType, Gender, HeroInstance, PlayerProfile, StarterHeroSeed, StarterItemSeed,
    StarterPackConfig,
};
pub use mutations::{HeroLevelUpOutcome, UseItemEffect, UseItemOutcome};
pub use store::{atomic_write_bytes, PlayerStore, RecoveryReport};
pub use wal::{crc32_ieee, WalOp, WalRecord, WalScanOutcome, WriteAheadLog};
