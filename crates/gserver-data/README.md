# `gserver-data`

Zero-dependency, `#![forbid(unsafe_code)]` pure-Rust asset decryptor (`bt_decrypt`) and authentic game template database loader for **GServer (`vnsg2` / Babeltime)**.

## Features

- **`bt_decrypt` (`0xFEFEFEFE` Container & SQLite Cell Decryptor)**:
  - Exact 100% safe Rust port of `bt_decrypt` (`0x340ef4`) and `decrypt_basic` (`0x340e4c`) from `libcocos2dlua.so` using all 16 real key slots from `07_bt_decrypt_keys.inc`.
  - Hex-encoded `babeltime` SQLite cell decryption matching `CConfigManager.cpp` (`CTable::getRowById`).
  - Cocos2d-x POSIX asset path normalization (`normalize_asset_path`, `resolve_relative_asset_path`, `replace_file_extension`).
- **Embedded Authentic `vnsg2` Templates (Zero External Dependencies)**:
  - `PROTAGONISTS`: Male (`utid = 1`, `htid = 1`, spine `1000001`) and Female (`utid = 2`, `htid = 2`, spine `1000002`) protagonists + all 18 quality evolution `htid`s.
  - `HEROES`: 59 authentic Three Kingdoms heroes across Wei, Shu, Wu, Qun, and Famous factions with real `htid`, `fragment_tid`, `fragment_count`, `exp_curve_id`, and Awakened / Red / Gold / Dark-Gold / Purple-Gold / Color-Gold promotion `htid`s.
  - `ITEMS` & `GEAR_SUITS`: 127 authentic items (Stamina/Energy pills, EXP books, Recruit tokens, Hero & Equipment chests, Hero shards, Starter/Purple/Orange/Red/Gold/Dark-Gold/Purple-Gold/Color-Gold/Gild-Gold gear) + all 23 equipment sets from `suit.db`.
  - `TOWNS`, `BAG_LIMITS`, `LEVEL_EXP_CURVES`: Real `town.db` maps (`1, 2, 3, 101`), all 42 `bag.db` inventory capacities, and full 231-level (`exp_0..exp_230`) `level_up_exp.db` curves with exact `LevelUpExpReader.lua` math.
- **Full Client `database/` Directory Exporter & Loader**:
  - Run `python3 tools/export_db_to_json.py --db-dir /path/to/client/database --out-dir ./exported_db` to decrypt and export every SQLite `.db` table into structured JSON + `manifest.json`.
  - Load at server startup via `GameDataStore::from_export_dir("./exported_db")` or `ExportedDatabaseDir::load_eager("./exported_db")`.

## Build & Test Offline

```bash
cargo test --offline
```
