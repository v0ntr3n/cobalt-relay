//! Directory loader for JSON tables exported by `tools/export_db_to_json.py`, plus
//! unified [`GameDataStore`] combining embedded templates with full client `database/` exports.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::hero_skills::{
    is_validated_skill_id, record_hero_skill_fallback, resolve_hero_battle_skills_with_awake,
    validate_embedded_hero_skill_mappings, validate_raw_skill_id_with_extra,
    HeroSkillFallbackReason, HeroSkillResolutionSource, HeroSkillValidationReport,
    InvalidHeroSkillDetail, ResolvedHeroBattleSkills, SkillIdValidationError,
    AWAKE_SKILL_CHANGE_RANK, FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID,
};
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
            Self::Io { path, source } => {
                write!(f, "I/O error reading '{}': {}", path.display(), source)
            }
            Self::Json { path, source } => {
                write!(f, "JSON error in '{}': {}", path.display(), source)
            }
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

fn parse_skill_json_field(
    row: &JsonValue,
    field: &str,
    extra_valid: &dyn Fn(u32) -> bool,
) -> Result<u32, SkillIdValidationError> {
    match row.get(field) {
        None | Some(JsonValue::Null) => Err(SkillIdValidationError::Missing),
        Some(JsonValue::Int(v)) => validate_raw_skill_id_with_extra(Some(*v), Some(extra_valid)),
        Some(JsonValue::Str(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Err(SkillIdValidationError::Missing)
            } else if let Ok(v) = trimmed.parse::<i64>() {
                validate_raw_skill_id_with_extra(Some(v), Some(extra_valid))
            } else {
                Err(SkillIdValidationError::NonNumericType)
            }
        }
        _ => Err(SkillIdValidationError::NonNumericType),
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
            if dir
                .get_row_by_id("timeMachine_hero", "timeMachine_hero", id64)
                .is_some()
                || dir.get_row_by_id("hero", "hero", id64).is_some()
                || dir.get_row_by_id("hero", "heroes", id64).is_some()
            {
                return true;
            }
        }
        false
    }

    /// Checks whether a `skill_id` exists in the embedded validated skill set or any loaded runtime skill table.
    pub fn contains_skill_id(&self, skill_id: u32) -> bool {
        if is_validated_skill_id(skill_id) {
            return true;
        }
        if skill_id == 0 {
            return false;
        }
        if let Some(dir) = &self.export_dir {
            let id64 = u64::from(skill_id);
            for (db, tbl) in [
                ("skill", "skill"),
                ("timeMachine_skill", "timeMachine_skill"),
                ("skill_display", "skill_display"),
                ("skill_pet", "skill_pet"),
                ("jointSkill", "jointSkill"),
            ] {
                if dir.get_row_by_id(db, tbl, id64).is_some() {
                    return true;
                }
            }
        }
        false
    }

    /// Resolves authentic `(normal_skill_id, rage_skill_id)` for a hero `htid` and `awake_rank`,
    /// preferring any loaded `timeMachine_hero` / `hero` JSON table in [`Self::export_dir`] before
    /// falling back to the embedded 801-row `HERO_SKILL_ENTRIES` table.
    pub fn resolve_hero_battle_skills(
        &self,
        htid: u32,
        awake_rank: u32,
    ) -> ResolvedHeroBattleSkills {
        self.resolve_hero_battle_skills_internal(htid, awake_rank, true)
    }

    fn resolve_hero_battle_skills_internal(
        &self,
        htid: u32,
        awake_rank: u32,
        record_diagnostics: bool,
    ) -> ResolvedHeroBattleSkills {
        if htid > 0 {
            if let Some(dir) = &self.export_dir {
                let id64 = u64::from(htid);
                let row_opt = dir
                    .get_row_by_id("timeMachine_hero", "timeMachine_hero", id64)
                    .or_else(|| dir.get_row_by_id("hero", "hero", id64))
                    .or_else(|| dir.get_row_by_id("hero", "heroes", id64));
                if let Some(row) = row_opt {
                    let extra_check = |sid: u32| self.contains_skill_id(sid);
                    let use_awake_com = awake_rank >= AWAKE_SKILL_CHANGE_RANK
                        && matches!(row.get("awake_com_atk"), Some(JsonValue::Int(n)) if *n > 0);
                    let use_awake_angry = awake_rank >= AWAKE_SKILL_CHANGE_RANK
                        && matches!(row.get("awake_angry_atk"), Some(JsonValue::Int(n)) if *n > 0);

                    let com_col = if use_awake_com {
                        "awake_com_atk"
                    } else {
                        "com_atk"
                    };
                    let angry_col = if use_awake_angry {
                        "awake_angry_atk"
                    } else {
                        "angry_atk"
                    };

                    let com_res = parse_skill_json_field(row, com_col, &extra_check);
                    let angry_res = parse_skill_json_field(row, angry_col, &extra_check);

                    return match (com_res, angry_res) {
                        (Ok(normal_skill_id), Ok(rage_skill_id)) => ResolvedHeroBattleSkills {
                            htid,
                            normal_skill_id,
                            rage_skill_id,
                            source: HeroSkillResolutionSource::ExportedDbRow {
                                htid,
                                awakened_override: use_awake_com || use_awake_angry,
                            },
                            used_normal_fallback: false,
                            used_rage_fallback: false,
                        },
                        (com_outcome, angry_outcome) => {
                            let (normal_skill_id, used_normal_fallback, normal_error) =
                                match com_outcome {
                                    Ok(id) => (id, false, None),
                                    Err(e) => (FALLBACK_NORMAL_SKILL_ID, true, Some(e)),
                                };
                            let (rage_skill_id, used_rage_fallback, rage_error) =
                                match angry_outcome {
                                    Ok(id) => (id, false, None),
                                    Err(e) => (FALLBACK_RAGE_SKILL_ID, true, Some(e)),
                                };
                            let reason = HeroSkillFallbackReason::InvalidSkillReference {
                                htid,
                                normal_error,
                                rage_error,
                            };
                            let slots = (used_normal_fallback as u64) + (used_rage_fallback as u64);
                            if record_diagnostics && slots > 0 {
                                record_hero_skill_fallback(
                                    htid,
                                    reason,
                                    normal_skill_id,
                                    rage_skill_id,
                                    slots,
                                );
                            }
                            ResolvedHeroBattleSkills {
                                htid,
                                normal_skill_id,
                                rage_skill_id,
                                source: HeroSkillResolutionSource::Fallback(reason),
                                used_normal_fallback,
                                used_rage_fallback,
                            }
                        }
                    };
                }
            }
        }

        resolve_hero_battle_skills_with_awake(htid, awake_rank)
    }

    /// Validates hero skill mappings across the loaded exported `timeMachine_hero`/`hero` table
    /// when present, or across the embedded 801-row table when operating standalone.
    pub fn validate_hero_skill_mappings(&self) -> HeroSkillValidationReport {
        let Some(dir) = &self.export_dir else {
            return validate_embedded_hero_skill_mappings();
        };
        let Some(tbl) = dir
            .get_loaded_table("timeMachine_hero", "timeMachine_hero")
            .or_else(|| dir.get_loaded_table("hero", "hero"))
            .or_else(|| dir.get_loaded_table("hero", "heroes"))
        else {
            return validate_embedded_hero_skill_mappings();
        };

        let mut base = validate_embedded_hero_skill_mappings();
        let extra_check = |sid: u32| self.contains_skill_id(sid);

        let mut valid_normal_skill_count = 0usize;
        let mut valid_rage_skill_count = 0usize;
        let mut fully_valid_hero_count = 0usize;
        let mut missing_or_invalid_template_count = 0usize;
        let mut missing_or_invalid_skill_ref_count = 0usize;
        let mut fallback_template_count = 0usize;
        let mut fallback_skill_slot_count = 0usize;
        let mut details = Vec::new();

        let mut sorted_rows: Vec<(u32, &JsonValue)> = tbl
            .rows_by_id
            .iter()
            .filter_map(|(k, v)| k.parse::<u32>().ok().map(|id| (id, v)))
            .collect();
        sorted_rows.sort_by_key(|(id, _)| *id);

        for (htid, row) in &sorted_rows {
            let com_check = parse_skill_json_field(row, "com_atk", &extra_check);
            let angry_check = parse_skill_json_field(row, "angry_atk", &extra_check);
            let resolved = self.resolve_hero_battle_skills_internal(*htid, 0, false);

            if com_check.is_ok() {
                valid_normal_skill_count += 1;
            } else {
                missing_or_invalid_skill_ref_count += 1;
            }
            if angry_check.is_ok() {
                valid_rage_skill_count += 1;
            } else {
                missing_or_invalid_skill_ref_count += 1;
            }

            if com_check.is_ok() && angry_check.is_ok() {
                fully_valid_hero_count += 1;
            } else {
                missing_or_invalid_template_count += 1;
                let name_vi = row
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let is_show = row.get("is_show").and_then(JsonValue::as_i64) == Some(1);
                details.push(InvalidHeroSkillDetail {
                    htid: *htid,
                    name_vi,
                    is_show,
                    normal_error: com_check.err(),
                    rage_error: angry_check.err(),
                    resolved_normal_skill_id: resolved.normal_skill_id,
                    resolved_rage_skill_id: resolved.rage_skill_id,
                });
            }

            if resolved.used_any_fallback() {
                fallback_template_count += 1;
                fallback_skill_slot_count += resolved.fallback_slot_count() as usize;
            }
        }

        base.total_templates_inspected = sorted_rows.len();
        base.valid_normal_skill_count = valid_normal_skill_count;
        base.valid_rage_skill_count = valid_rage_skill_count;
        base.fully_valid_hero_count = fully_valid_hero_count;
        base.missing_or_invalid_template_count = missing_or_invalid_template_count;
        base.missing_or_invalid_skill_ref_count = missing_or_invalid_skill_ref_count;
        base.fallback_template_count = fallback_template_count;
        base.fallback_skill_slot_count = fallback_skill_slot_count;
        base.missing_or_invalid_details = details;
        base
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
