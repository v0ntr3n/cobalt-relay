//! Directory loader for JSON tables exported by `tools/export_db_to_json.py`, plus
//! unified [`GameDataStore`] combining embedded templates with full client `database/` exports.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::json::{parse_json, JsonError, JsonValue};
use crate::templates::{is_valid_hero_htid, is_valid_item_tid, is_valid_town_id};

#[derive(Debug)]
pub enum LoaderError {
    Io { path: PathBuf, source: std::io::Error },
    Json { path: PathBuf, source: JsonError },
    InvalidFormat { path: PathBuf, detail: String },
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "I/O error reading '{}': {}", path.display(), source),
            Self::Json { path, source } => write!(f, "JSON error in '{}': {}", path.display(), source),
            Self::InvalidFormat { path, detail } => {
                write!(f, "Invalid GServer export format in '{}': {}", path.display(), detail)
            }
        }
    }
}

impl std::error::Error for LoaderError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableManifestEntry {
    pub database: String,
    pub table: String,
    pub relative_path: String,
    pub row_count: usize,
    pub column_count: usize,
}

#[derive(Debug, Clone)]
pub struct ExportedTable {
    pub database: String,
    pub table: String,
    pub columns: Vec<(String, String)>,
    pub rows_by_id: BTreeMap<String, JsonValue>,
}

impl ExportedTable {
    pub fn len(&self) -> usize {
        self.rows_by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows_by_id.is_empty()
    }

    /// Retrieves a row by integer ID (matching `CTable::getRowById` integer/string dual lookup).
    pub fn get_by_id(&self, id: u64) -> Option<&JsonValue> {
        self.rows_by_id.get(&id.to_string())
    }

    /// Retrieves a row by string key (e.g. `"xiahoudun_nh"` in `hero_model.db` or `"201"` in `hero.db`).
    pub fn get_by_key(&self, key: &str) -> Option<&JsonValue> {
        self.rows_by_id.get(key)
    }
}

/// Loader for an output folder produced by `tools/export_db_to_json.py`.
#[derive(Debug, Clone)]
pub struct ExportedDatabaseDir {
    root_dir: PathBuf,
    manifest_entries: Vec<TableManifestEntry>,
    loaded_tables: BTreeMap<(String, String), ExportedTable>,
}

impl ExportedDatabaseDir {
    /// Opens an exported directory, parses `manifest.json`, and eagerly loads all listed JSON tables.
    pub fn load_eager(root_dir: impl AsRef<Path>) -> Result<Self, LoaderError> {
        let mut dir = Self::open_manifest_only(root_dir)?;
        let keys: Vec<(String, String)> = dir
            .manifest_entries
            .iter()
            .map(|e| (e.database.clone(), e.table.clone()))
            .collect();
        for (db, tbl) in keys {
            dir.ensure_table_loaded(&db, &tbl)?;
        }
        Ok(dir)
    }

    /// Opens `manifest.json` without reading every table file upfront; tables can be loaded on demand.
    pub fn open_manifest_only(root_dir: impl AsRef<Path>) -> Result<Self, LoaderError> {
        let root = root_dir.as_ref().to_path_buf();
        let manifest_path = root.join("manifest.json");
        let text = fs::read_to_string(&manifest_path).map_err(|source| LoaderError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let doc = parse_json(&text).map_err(|source| LoaderError::Json {
            path: manifest_path.clone(),
            source,
        })?;

        let tables_arr = doc
            .get("tables")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| LoaderError::InvalidFormat {
                path: manifest_path.clone(),
                detail: "missing 'tables' array in manifest.json".to_string(),
            })?;

        let mut entries = Vec::with_capacity(tables_arr.len());
        for entry in tables_arr {
            let database = entry
                .get("database")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| LoaderError::InvalidFormat {
                    path: manifest_path.clone(),
                    detail: "table entry missing 'database'".to_string(),
                })?
                .to_string();
            let table = entry
                .get("table")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| LoaderError::InvalidFormat {
                    path: manifest_path.clone(),
                    detail: "table entry missing 'table'".to_string(),
                })?
                .to_string();
            let relative_path = entry
                .get("relative_path")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| LoaderError::InvalidFormat {
                    path: manifest_path.clone(),
                    detail: "table entry missing 'relative_path'".to_string(),
                })?
                .to_string();
            let row_count = entry
                .get("row_count")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as usize;
            let column_count = entry
                .get("column_count")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0) as usize;

            entries.push(TableManifestEntry {
                database,
                table,
                relative_path,
                row_count,
                column_count,
            });
        }

        Ok(Self {
            root_dir: root,
            manifest_entries: entries,
            loaded_tables: BTreeMap::new(),
        })
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn manifest_entries(&self) -> &[TableManifestEntry] {
        &self.manifest_entries
    }

    pub fn loaded_table_count(&self) -> usize {
        self.loaded_tables.len()
    }

    /// Loads `(database, table)` from disk if not yet cached and returns a reference to it.
    pub fn ensure_table_loaded(
        &mut self,
        database: &str,
        table: &str,
    ) -> Result<&ExportedTable, LoaderError> {
        let key = (database.to_string(), table.to_string());
        if !self.loaded_tables.contains_key(&key) {
            let rel = self
                .manifest_entries
                .iter()
                .find(|e| e.database == database && e.table == table)
                .map(|e| e.relative_path.clone())
                .unwrap_or_else(|| format!("{database}/{table}.json"));
            let table_path = self.root_dir.join(rel);
            let parsed = Self::read_table_file(&table_path)?;
            self.loaded_tables.insert(key.clone(), parsed);
        }
        Ok(self.loaded_tables.get(&key).expect("just inserted"))
    }

    pub fn get_loaded_table(&self, database: &str, table: &str) -> Option<&ExportedTable> {
        self.loaded_tables
            .get(&(database.to_string(), table.to_string()))
    }

    pub fn get_row_by_id(&self, database: &str, table: &str, id: u64) -> Option<&JsonValue> {
        self.get_loaded_table(database, table)?.get_by_id(id)
    }

    fn read_table_file(path: &Path) -> Result<ExportedTable, LoaderError> {
        let text = fs::read_to_string(path).map_err(|source| LoaderError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let doc = parse_json(&text).map_err(|source| LoaderError::Json {
            path: path.to_path_buf(),
            source,
        })?;

        let database = doc
            .get("database")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| LoaderError::InvalidFormat {
                path: path.to_path_buf(),
                detail: "missing 'database' string".to_string(),
            })?
            .to_string();
        let table = doc
            .get("table")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| LoaderError::InvalidFormat {
                path: path.to_path_buf(),
                detail: "missing 'table' string".to_string(),
            })?
            .to_string();

        let columns = doc
            .get("columns")
            .and_then(JsonValue::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let name = c.get("name")?.as_str()?.to_string();
                        let kind = c
                            .get("kind")
                            .and_then(JsonValue::as_str)
                            .unwrap_or("scalar")
                            .to_string();
                        Some((name, kind))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let rows_arr = doc
            .get("rows")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| LoaderError::InvalidFormat {
                path: path.to_path_buf(),
                detail: "missing 'rows' array".to_string(),
            })?;

        let mut rows_by_id = BTreeMap::new();
        for row in rows_arr {
            let key = match row.get("id") {
                Some(JsonValue::Int(n)) => n.to_string(),
                Some(JsonValue::Str(s)) => s.clone(),
                _ => continue,
            };
            rows_by_id.insert(key, row.clone());
        }

        Ok(ExportedTable {
            database,
            table,
            columns,
            rows_by_id,
        })
    }
}

/// High-level server template facade: serves zero-dependency embedded templates out of the box,
/// and seamlessly consults an optional [`ExportedDatabaseDir`] when pointed at an exported client `database/`.
#[derive(Debug, Clone, Default)]
pub struct GameDataStore {
    export_dir: Option<ExportedDatabaseDir>,
}

impl GameDataStore {
    pub const fn new_embedded() -> Self {
        Self { export_dir: None }
    }

    pub fn from_export_dir(root_dir: impl AsRef<Path>) -> Result<Self, LoaderError> {
        let export_dir = ExportedDatabaseDir::load_eager(root_dir)?;
        Ok(Self {
            export_dir: Some(export_dir),
        })
    }

    pub fn export_dir(&self) -> Option<&ExportedDatabaseDir> {
        self.export_dir.as_ref()
    }

    /// Checks whether a hero `htid` exists in either the exported `hero`/`timeMachine_hero` tables or embedded consts.
    pub fn contains_hero_htid(&self, htid: u32) -> bool {
        if is_valid_hero_htid(htid) {
            return true;
        }
        if let Some(dir) = &self.export_dir {
            let id64 = u64::from(htid);
            if dir.get_row_by_id("hero", "heroes", id64).is_some()
                || dir.get_row_by_id("hero", "hero", id64).is_some()
                || dir
                    .get_row_by_id("timeMachine_hero", "timeMachine_hero", id64)
                    .is_some()
            {
                return true;
            }
        }
        false
    }

    /// Checks whether an `item_tid` exists in either the exported item tables or embedded consts.
    pub fn contains_item_tid(&self, item_tid: u32) -> bool {
        if is_valid_item_tid(item_tid) {
            return true;
        }
        if let Some(dir) = &self.export_dir {
            let id64 = u64::from(item_tid);
            for (db, tbl) in [
                ("item_normal", "item_normal"),
                ("item", "item_normal"),
                ("item", "item_direct"),
                ("item", "item_randgift"),
                ("item", "item_arm"),
                ("item", "item_treasure"),
                ("item", "item_jewel"),
                ("timeMachine_item", "timeMachine_item_normal"),
                ("timeMachine_item", "timeMachine_item_direct"),
                ("timeMachine_item", "timeMachine_item_randgift"),
                ("timeMachine_hero", "timeMachine_item_soul"),
            ] {
                if dir.get_row_by_id(db, tbl, id64).is_some() {
                    return true;
                }
            }
        }
        false
    }

    /// Checks whether a `town_id` exists in either the exported `town.db` or embedded consts.
    pub fn contains_town_id(&self, town_id: u32) -> bool {
        if is_valid_town_id(town_id) {
            return true;
        }
        if let Some(dir) = &self.export_dir {
            if dir.get_row_by_id("town", "town", u64::from(town_id)).is_some() {
                return true;
            }
        }
        false
    }
}
