# `gserver-player`

Zero-dependency, pure-Rust (`#![forbid(unsafe_code)]`) player state, Lua wire-schema projection, progression mutation, and crash-safe persistence crate for **GServer (`vnsg2`)**.

## Highlights

- **100% Safe Rust, Zero External Dependencies**: Built strictly on the Rust standard library (`std`) so offline builds (`cargo test --offline`) work out of the box with zero third-party crates.
- **Audited `vnsg2` Lua Data Model Schemas**:
  - **Login & `UserData`** (`LoginProxy.lua`, `UserData.lua`, `ConfigData.lua`):
    - `user.getUserBeforLogin`: `{}` when no character exists (`ret.uid == nil` triggers `CreateUserScene`), or `{uid, uname, utid, level, vip}`.
    - `user.userLogin`: `"ok"` on success or `"ban:<until_ts>:<reason>"` on suspension.
    - `user.getUser`: `copper` and `fight_force_max` emitted as `tonumber(...)`-safe strings; direct arithmetic fields (`gold`, `gold_bind`, `hero_soul`, `prestige`, `execution`, `execution_time`, `energy`, `energy_time`, `hero_bag_buy_times`, shop coins) emitted as numbers; 14-char `"YYYYMMDDHHMMSS"` `server_open_time`; valid `dress_id` / `attr_dress_id` guaranteed present in `dress_info` (`default_fashion_id = 1`); plus classic 1st-gen field aliases (`gold_num`, `silver_num`, `soul_num`, `jewel_num`, `prestige_num`, `stamina`, `stamina_time`, `fight_force`, `dress`, `va_user`, `create_time`).
  - **Hero Roster** (`HerosData.lua`, `HeroInfo.lua`): Numeric `hid`, `htid` (`1` Male / `2` Female protagonist), `level`, `exp`, `evolve_level`, `upgrade_level`, `hero_star_pos`, `hero_star_level`, promotion ranks/levels, and guaranteed non-nil `va_fail_reward` & wash tables.
  - **Squad & Formation** (`HeroFormationData.lua`, `FormationData.lua`, `ArmInfo.lua`): `position` containing only occupied `hid > 0` entries present in `allHeros`, 3x3 battle grid `formation` (`attack_sep_list = {3, 6, 9, 2, 5, 8}`), `partner` (`LittleFriend`), `buddy`, 6-slot `arming` (`"1"`..`"9"`), and mandatory non-nil tables (`bingfu`, `jointSkill`, `arrCounsellor`, `godanimal`, `topanimal`, `formationBook`).
  - **Inventory / Bag** (`BagData.lua`, `BagConsts.lua`, `ItemFactory.lua`, `ArmItem.lua`, `TreasureItem.lua`, `PetItem.lua`, `GodItem.lua`, `AwakeItem.lua`): Non-nil `daily_use`, `gridStart`, `gridMaxNum`, `gridOpenTimes`, all 42 `vnsg2` sub-bag tables (`gid` ranges `10001..740000`), classic sub-bag aliases (`props`, `heroFrag`, `treas`, `armFrag`, `godarm`, `dress`, `rune`), and per-class `va_item_text` defaults.
- **Crash-Safe Persistence (`PlayerStore` + `WriteAheadLog`)**:
  - `data/wal/operations.wal`: Append-only single-line JSONL records protected by IEEE 802.3 CRC32 checksums and `File::sync_data()`.
  - `data/accounts.json` & `data/players/<uid>.json`: Written via sibling `.tmp.<pid>.<seq>` + `File::sync_all()` + POSIX `fs::rename` + parent directory `sync_all()`.
  - Startup recovery automatically truncates torn WAL tails, cleans orphaned `.tmp` files, and replays committed WAL entries that had not yet been snapshotted before power loss.

## Running Offline Tests

```bash
cargo test --offline
```
