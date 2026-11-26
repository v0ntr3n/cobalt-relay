//! Authentic per-hero battle skill mapping (`com_atk` -> `normal_skill_id`,
//! `angry_atk` -> `rage_skill_id`), skill-set cross-validation, diagnostic fallback
//! tracking, and programmatic validation reporting for GServer (`vnsg2`).

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::generated_consts::{HEROES, PROTAGONISTS};
use crate::generated_hero_skills::{HERO_SKILL_ENTRIES, VALIDATED_SKILL_IDS};

/// Verified fallback normal attack skill ID (`type == 2` in `skill.db`), retained strictly for
/// monster units, unknown custom test `htid`s, or templates explicitly lacking `com_atk`.
pub const FALLBACK_NORMAL_SKILL_ID: u32 = 1001;

/// Verified fallback rage attack skill ID (`type == 1` in `skill.db`), retained strictly for
/// monster units, unknown custom test `htid`s, or templates explicitly lacking `angry_atk`.
pub const FALLBACK_RAGE_SKILL_ID: u32 = 2001;

/// `awake_change_level = 5` in `timeMachine_current_config.db`: heroes with `awake_rank >= 5`
/// promote `com_atk` / `angry_atk` to `awake_com_atk` / `awake_angry_atk` when defined.
pub const AWAKE_SKILL_CHANGE_RANK: u32 = 5;

static HERO_SKILL_FALLBACK_TEMPLATES: AtomicU64 = AtomicU64::new(0);
static HERO_SKILL_FALLBACK_SLOTS: AtomicU64 = AtomicU64::new(0);
static MONSTER_DEFAULT_SKILL_UNITS: AtomicU64 = AtomicU64::new(0);
static LOGGED_FALLBACK_HTIDS: OnceLock<Mutex<BTreeSet<u32>>> = OnceLock::new();

/// Authentic hero skill configuration row extracted from `timeMachine_hero.db` (`HeroReader.lua`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeroSkillEntry {
    pub htid: u32,
    pub name_vi: &'static str,
    pub prototype_htid: u32,
    pub country: u8,
    pub init_quality: u8,
    pub is_protagonist: bool,
    pub is_show: bool,
    pub com_atk: Option<u32>,
    pub angry_atk: Option<u32>,
    pub awake_com_atk: Option<u32>,
    pub awake_angry_atk: Option<u32>,
    pub tray_gold_htid: u32,
    pub gild_gold_htid: u32,
}

/// Explicit contract violation when validating a raw skill reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillIdValidationError {
    Missing,
    Zero,
    Negative(i64),
    OutOfRange(i64),
    NonNumericType,
    Unvalidated(u32),
}

impl fmt::Display for SkillIdValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "missing/null skill reference"),
            Self::Zero => write!(f, "zero skill ID (0)"),
            Self::Negative(v) => write!(f, "negative skill ID ({v})"),
            Self::OutOfRange(v) => write!(f, "out-of-range u32 skill ID ({v})"),
            Self::NonNumericType => write!(f, "non-numeric skill field value"),
            Self::Unvalidated(id) => write!(f, "skill ID {id} not in validated skill set"),
        }
    }
}

/// Diagnostic reason why a hero resolution fell back to `1001` and/or `2001`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeroSkillFallbackReason {
    ZeroOrInvalidHtid {
        htid: u32,
    },
    UnknownHeroTemplate {
        htid: u32,
    },
    InvalidSkillReference {
        htid: u32,
        normal_error: Option<SkillIdValidationError>,
        rage_error: Option<SkillIdValidationError>,
    },
}

impl fmt::Display for HeroSkillFallbackReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroOrInvalidHtid { htid } => write!(f, "invalid hero htid={htid}"),
            Self::UnknownHeroTemplate { htid } => {
                write!(
                    f,
                    "unknown hero template htid={htid} (not in timeMachine_hero)"
                )
            }
            Self::InvalidSkillReference {
                htid,
                normal_error,
                rage_error,
            } => {
                write!(
                    f,
                    "hero htid={htid} invalid skill reference (com_atk={normal_error:?}, angry_atk={rage_error:?})"
                )
            }
        }
    }
}

/// Provenance of a resolved `(normal_skill_id, rage_skill_id)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeroSkillResolutionSource {
    /// Exact row in embedded `HERO_SKILL_ENTRIES` (`timeMachine_hero`).
    DirectTemplate { htid: u32, awakened_override: bool },
    /// Promoted `tray_gold` (`6xxx`), `gild_gold` (`7xxx`), or `iron_gold` (`8xxx`) tier
    /// resolved from the highest-quality validated row of its `prototype_htid` family.
    TierFamilyInherited {
        requested_htid: u32,
        resolved_from_htid: u32,
        awakened_override: bool,
    },
    /// Loaded dynamically at runtime from an exported client JSON table (`GameDataStore`).
    ExportedDbRow { htid: u32, awakened_override: bool },
    /// Fallback (`1001` and/or `2001`) applied due to an unknown template or missing/invalid skill reference.
    Fallback(HeroSkillFallbackReason),
}

/// Complete outcome of resolving a hero's battle skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedHeroBattleSkills {
    pub htid: u32,
    pub normal_skill_id: u32,
    pub rage_skill_id: u32,
    pub source: HeroSkillResolutionSource,
    pub used_normal_fallback: bool,
    pub used_rage_fallback: bool,
}

impl ResolvedHeroBattleSkills {
    pub const fn used_any_fallback(&self) -> bool {
        self.used_normal_fallback || self.used_rage_fallback
    }

    pub const fn fallback_slot_count(&self) -> u64 {
        (self.used_normal_fallback as u64) + (self.used_rage_fallback as u64)
    }
}

/// Detailed record for any hero template with missing or invalid skill references during validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHeroSkillDetail {
    pub htid: u32,
    pub name_vi: String,
    pub is_show: bool,
    pub normal_error: Option<SkillIdValidationError>,
    pub rage_error: Option<SkillIdValidationError>,
    pub resolved_normal_skill_id: u32,
    pub resolved_rage_skill_id: u32,
}

/// Programmatic validation summary across all `timeMachine_hero` rows and `curated_templates.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroSkillValidationReport {
    pub total_templates_inspected: usize,
    pub valid_normal_skill_count: usize,
    pub valid_rage_skill_count: usize,
    pub fully_valid_hero_count: usize,
    pub missing_or_invalid_template_count: usize,
    pub missing_or_invalid_skill_ref_count: usize,
    pub fallback_template_count: usize,
    pub fallback_skill_slot_count: usize,
    pub missing_or_invalid_details: Vec<InvalidHeroSkillDetail>,
    pub curated_protagonist_templates_count: usize,
    pub curated_protagonist_htids_inspected: usize,
    pub curated_protagonist_valid_normal_count: usize,
    pub curated_protagonist_valid_rage_count: usize,
    pub curated_hero_templates_count: usize,
    pub curated_hero_tier_htids_inspected: usize,
    pub curated_hero_tier_valid_normal_count: usize,
    pub curated_hero_tier_valid_rage_count: usize,
    pub validated_skill_set_size: usize,
}

/// Returns the complete slice of 801 embedded `timeMachine_hero` skill entries.
pub const fn all_hero_skill_entries() -> &'static [HeroSkillEntry] {
    HERO_SKILL_ENTRIES
}

/// Returns the sorted slice of all validated skill IDs.
pub const fn all_validated_skill_ids() -> &'static [u32] {
    VALIDATED_SKILL_IDS
}

/// Looks up an embedded `HeroSkillEntry` by `htid` in `O(log N)` time.
pub fn hero_skill_entry_by_htid(htid: u32) -> Option<&'static HeroSkillEntry> {
    HERO_SKILL_ENTRIES
        .binary_search_by_key(&htid, |e| e.htid)
        .ok()
        .map(|idx| &HERO_SKILL_ENTRIES[idx])
}

/// Checks whether `skill_id` is non-zero and exists in the embedded validated skill set.
pub fn is_validated_skill_id(skill_id: u32) -> bool {
    skill_id > 0 && VALIDATED_SKILL_IDS.binary_search(&skill_id).is_ok()
}

/// Validates a raw optional signed skill reference against the embedded validated skill set.
pub fn validate_raw_skill_id(raw: Option<i64>) -> Result<u32, SkillIdValidationError> {
    validate_raw_skill_id_with_extra(raw, None)
}

/// Validates a raw optional signed skill reference against the embedded skill set plus an
/// optional runtime skill-table predicate (`GameDataStore`).
pub fn validate_raw_skill_id_with_extra(
    raw: Option<i64>,
    extra_valid: Option<&dyn Fn(u32) -> bool>,
) -> Result<u32, SkillIdValidationError> {
    let Some(v) = raw else {
        return Err(SkillIdValidationError::Missing);
    };
    if v == 0 {
        return Err(SkillIdValidationError::Zero);
    }
    if v < 0 {
        return Err(SkillIdValidationError::Negative(v));
    }
    let id = u32::try_from(v).map_err(|_| SkillIdValidationError::OutOfRange(v))?;
    if is_validated_skill_id(id) || extra_valid.is_some_and(|pred| pred(id)) {
        Ok(id)
    } else {
        Err(SkillIdValidationError::Unvalidated(id))
    }
}

/// Cumulative count of hero skill resolutions that triggered a fallback (`1001` and/or `2001`).
pub fn hero_skill_fallback_template_count() -> u64 {
    HERO_SKILL_FALLBACK_TEMPLATES.load(Ordering::Relaxed)
}

/// Cumulative count of individual hero skill slots (`com_atk` or `angry_atk`) that used a fallback.
pub fn hero_skill_fallback_slot_count() -> u64 {
    HERO_SKILL_FALLBACK_SLOTS.load(Ordering::Relaxed)
}

/// Cumulative count of non-hero monster combat units initialized with default monster skills (`1001`/`2001`).
pub fn monster_default_skill_unit_count() -> u64 {
    MONSTER_DEFAULT_SKILL_UNITS.load(Ordering::Relaxed)
}

pub(crate) fn record_hero_skill_fallback(
    htid: u32,
    reason: HeroSkillFallbackReason,
    normal_skill_id: u32,
    rage_skill_id: u32,
    slots_used: u64,
) {
    HERO_SKILL_FALLBACK_TEMPLATES.fetch_add(1, Ordering::Relaxed);
    HERO_SKILL_FALLBACK_SLOTS.fetch_add(slots_used, Ordering::Relaxed);
    let set_mutex = LOGGED_FALLBACK_HTIDS.get_or_init(|| Mutex::new(BTreeSet::new()));
    if let Ok(mut seen) = set_mutex.lock() {
        if seen.insert(htid) {
            eprintln!(
                "[gserver-data] diagnostic: hero skill fallback ({reason}) -> normal_skill_id={normal_skill_id}, rage_skill_id={rage_skill_id}"
            );
        }
    }
}

/// Finds the highest-quality validated entry for promoted `tray_gold` (`6xxx`),
/// `gild_gold` (`7xxx`), or `iron_gold` (`8xxx`) hero IDs that share a prototype family.
pub fn find_inherited_tier_skill_entry(htid: u32) -> Option<&'static HeroSkillEntry> {
    if htid == 0 {
        return None;
    }

    // 1. Explicit `trayGold_id` or `gildGold_id` foreign key on any `timeMachine_hero` row
    let mut proto_target: Option<u32> = None;
    for entry in HERO_SKILL_ENTRIES {
        if (entry.tray_gold_htid != 0 && entry.tray_gold_htid == htid)
            || (entry.gild_gold_htid != 0 && entry.gild_gold_htid == htid)
        {
            proto_target = Some(entry.prototype_htid);
            break;
        }
    }

    // 2. Canonical `6xxx` (Nirvana/Tray Gold), `7xxx` (Gild Gold), `8xxx` (Iron Gold) tier offset
    if proto_target.is_none() && (6101..9000).contains(&htid) {
        let base_candidate = htid % 1000;
        if (101..=999).contains(&base_candidate)
            && HERO_SKILL_ENTRIES.iter().any(|e| {
                (e.prototype_htid == base_candidate || e.htid == base_candidate)
                    && e.com_atk.is_some()
                    && e.angry_atk.is_some()
            })
        {
            proto_target = Some(base_candidate);
        }
    }

    let proto = proto_target?;
    HERO_SKILL_ENTRIES
        .iter()
        .filter(|e| {
            (e.prototype_htid == proto || e.htid == proto)
                && e.com_atk.is_some_and(is_validated_skill_id)
                && e.angry_atk.is_some_and(is_validated_skill_id)
        })
        .max_by_key(|e| (e.init_quality, e.htid))
}

fn resolve_from_entry(
    requested_htid: u32,
    entry: &HeroSkillEntry,
    awake_rank: u32,
    inherited: bool,
    record_diagnostics: bool,
) -> ResolvedHeroBattleSkills {
    let use_awake_com = awake_rank >= AWAKE_SKILL_CHANGE_RANK && entry.awake_com_atk.is_some();
    let use_awake_angry = awake_rank >= AWAKE_SKILL_CHANGE_RANK && entry.awake_angry_atk.is_some();

    let raw_com = if use_awake_com {
        entry.awake_com_atk
    } else {
        entry.com_atk
    };
    let raw_angry = if use_awake_angry {
        entry.awake_angry_atk
    } else {
        entry.angry_atk
    };

    let com_res = validate_raw_skill_id(raw_com.map(i64::from));
    let angry_res = validate_raw_skill_id(raw_angry.map(i64::from));

    match (com_res, angry_res) {
        (Ok(normal_skill_id), Ok(rage_skill_id)) => {
            let awakened_override = use_awake_com || use_awake_angry;
            let source = if inherited {
                HeroSkillResolutionSource::TierFamilyInherited {
                    requested_htid,
                    resolved_from_htid: entry.htid,
                    awakened_override,
                }
            } else {
                HeroSkillResolutionSource::DirectTemplate {
                    htid: requested_htid,
                    awakened_override,
                }
            };
            ResolvedHeroBattleSkills {
                htid: requested_htid,
                normal_skill_id,
                rage_skill_id,
                source,
                used_normal_fallback: false,
                used_rage_fallback: false,
            }
        }
        (com_outcome, angry_outcome) => {
            let (normal_skill_id, used_normal_fallback, normal_error) = match com_outcome {
                Ok(id) => (id, false, None),
                Err(err) => (FALLBACK_NORMAL_SKILL_ID, true, Some(err)),
            };
            let (rage_skill_id, used_rage_fallback, rage_error) = match angry_outcome {
                Ok(id) => (id, false, None),
                Err(err) => (FALLBACK_RAGE_SKILL_ID, true, Some(err)),
            };
            let reason = HeroSkillFallbackReason::InvalidSkillReference {
                htid: requested_htid,
                normal_error,
                rage_error,
            };
            let slots = (used_normal_fallback as u64) + (used_rage_fallback as u64);
            if record_diagnostics && slots > 0 {
                record_hero_skill_fallback(
                    requested_htid,
                    reason,
                    normal_skill_id,
                    rage_skill_id,
                    slots,
                );
            }
            ResolvedHeroBattleSkills {
                htid: requested_htid,
                normal_skill_id,
                rage_skill_id,
                source: HeroSkillResolutionSource::Fallback(reason),
                used_normal_fallback,
                used_rage_fallback,
            }
        }
    }
}

/// Resolves authentic `(normal_skill_id, rage_skill_id)` (`com_atk`, `angry_atk`) for a hero `htid`.
pub fn resolve_hero_battle_skills(htid: u32) -> ResolvedHeroBattleSkills {
    resolve_hero_battle_skills_with_awake(htid, 0)
}

/// Resolves authentic `(normal_skill_id, rage_skill_id)` for a hero `htid` and `awake_rank`.
pub fn resolve_hero_battle_skills_with_awake(
    htid: u32,
    awake_rank: u32,
) -> ResolvedHeroBattleSkills {
    resolve_hero_battle_skills_internal(htid, awake_rank, true)
}

fn resolve_hero_battle_skills_internal(
    htid: u32,
    awake_rank: u32,
    record_diagnostics: bool,
) -> ResolvedHeroBattleSkills {
    if htid == 0 {
        let reason = HeroSkillFallbackReason::ZeroOrInvalidHtid { htid };
        if record_diagnostics {
            record_hero_skill_fallback(
                htid,
                reason,
                FALLBACK_NORMAL_SKILL_ID,
                FALLBACK_RAGE_SKILL_ID,
                2,
            );
        }
        return ResolvedHeroBattleSkills {
            htid,
            normal_skill_id: FALLBACK_NORMAL_SKILL_ID,
            rage_skill_id: FALLBACK_RAGE_SKILL_ID,
            source: HeroSkillResolutionSource::Fallback(reason),
            used_normal_fallback: true,
            used_rage_fallback: true,
        };
    }

    if let Some(entry) = hero_skill_entry_by_htid(htid) {
        return resolve_from_entry(htid, entry, awake_rank, false, record_diagnostics);
    }

    if let Some(donor) = find_inherited_tier_skill_entry(htid) {
        return resolve_from_entry(htid, donor, awake_rank, true, record_diagnostics);
    }

    let reason = HeroSkillFallbackReason::UnknownHeroTemplate { htid };
    if record_diagnostics {
        record_hero_skill_fallback(
            htid,
            reason,
            FALLBACK_NORMAL_SKILL_ID,
            FALLBACK_RAGE_SKILL_ID,
            2,
        );
    }
    ResolvedHeroBattleSkills {
        htid,
        normal_skill_id: FALLBACK_NORMAL_SKILL_ID,
        rage_skill_id: FALLBACK_RAGE_SKILL_ID,
        source: HeroSkillResolutionSource::Fallback(reason),
        used_normal_fallback: true,
        used_rage_fallback: true,
    }
}

/// Resolves `(normal_skill_id, rage_skill_id)` for an NPC/monster/defender unit:
/// - If `htid` belongs to a valid hero template (e.g., Arena guard `310` Tiểu Kiều or protagonist `1`/`2`),
///   returns that hero's authentic `(com_atk, angry_atk)`.
/// - Otherwise (e.g., Copy stronghold monster `5001..5011` or `50010111`), returns monster default
///   `(1001, 2001)` and increments [`monster_default_skill_unit_count`].
pub fn resolve_unit_or_monster_battle_skills(htid: u32) -> (u32, u32) {
    if htid > 0 {
        if let Some(entry) = hero_skill_entry_by_htid(htid) {
            let res = resolve_from_entry(htid, entry, 0, false, false);
            if !res.used_any_fallback() {
                return (res.normal_skill_id, res.rage_skill_id);
            }
        } else if let Some(donor) = find_inherited_tier_skill_entry(htid) {
            let res = resolve_from_entry(htid, donor, 0, true, false);
            if !res.used_any_fallback() {
                return (res.normal_skill_id, res.rage_skill_id);
            }
        }
    }
    MONSTER_DEFAULT_SKILL_UNITS.fetch_add(1, Ordering::Relaxed);
    (FALLBACK_NORMAL_SKILL_ID, FALLBACK_RAGE_SKILL_ID)
}

/// Runs a deterministic, side-effect-free validation pass across all 801 embedded
/// `timeMachine_hero` templates and all `curated_templates.json` protagonists & hero tiers.
pub fn validate_embedded_hero_skill_mappings() -> HeroSkillValidationReport {
    let mut valid_normal_skill_count = 0usize;
    let mut valid_rage_skill_count = 0usize;
    let mut fully_valid_hero_count = 0usize;
    let mut missing_or_invalid_template_count = 0usize;
    let mut missing_or_invalid_skill_ref_count = 0usize;
    let mut fallback_template_count = 0usize;
    let mut fallback_skill_slot_count = 0usize;
    let mut missing_or_invalid_details = Vec::new();

    for entry in HERO_SKILL_ENTRIES {
        let com_check = validate_raw_skill_id(entry.com_atk.map(i64::from));
        let angry_check = validate_raw_skill_id(entry.angry_atk.map(i64::from));
        let resolved = resolve_hero_battle_skills_internal(entry.htid, 0, false);

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
            missing_or_invalid_details.push(InvalidHeroSkillDetail {
                htid: entry.htid,
                name_vi: entry.name_vi.to_string(),
                is_show: entry.is_show,
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

    let mut curated_protagonist_htids_inspected = 0usize;
    let mut curated_protagonist_valid_normal_count = 0usize;
    let mut curated_protagonist_valid_rage_count = 0usize;
    for prot in PROTAGONISTS {
        for &q_htid in prot.quality_htids {
            curated_protagonist_htids_inspected += 1;
            let res = resolve_hero_battle_skills_internal(q_htid, 0, false);
            if !res.used_normal_fallback && is_validated_skill_id(res.normal_skill_id) {
                curated_protagonist_valid_normal_count += 1;
            }
            if !res.used_rage_fallback && is_validated_skill_id(res.rage_skill_id) {
                curated_protagonist_valid_rage_count += 1;
            }
        }
    }

    let mut seen_curated_tiers = BTreeSet::new();
    for hero in HEROES {
        for candidate in [
            hero.htid,
            hero.awakened_htid,
            hero.red_evolved_htid,
            hero.gold_htid,
            hero.dark_gold_htid,
            hero.purple_gold_htid,
            hero.color_gold_htid,
        ] {
            if candidate > 0 {
                seen_curated_tiers.insert(candidate);
            }
        }
    }

    let mut curated_hero_tier_valid_normal_count = 0usize;
    let mut curated_hero_tier_valid_rage_count = 0usize;
    for &tier_htid in &seen_curated_tiers {
        let res = resolve_hero_battle_skills_internal(tier_htid, 0, false);
        if !res.used_normal_fallback && is_validated_skill_id(res.normal_skill_id) {
            curated_hero_tier_valid_normal_count += 1;
        }
        if !res.used_rage_fallback && is_validated_skill_id(res.rage_skill_id) {
            curated_hero_tier_valid_rage_count += 1;
        }
    }

    HeroSkillValidationReport {
        total_templates_inspected: HERO_SKILL_ENTRIES.len(),
        valid_normal_skill_count,
        valid_rage_skill_count,
        fully_valid_hero_count,
        missing_or_invalid_template_count,
        missing_or_invalid_skill_ref_count,
        fallback_template_count,
        fallback_skill_slot_count,
        missing_or_invalid_details,
        curated_protagonist_templates_count: PROTAGONISTS.len(),
        curated_protagonist_htids_inspected,
        curated_protagonist_valid_normal_count,
        curated_protagonist_valid_rage_count,
        curated_hero_templates_count: HEROES.len(),
        curated_hero_tier_htids_inspected: seen_curated_tiers.len(),
        curated_hero_tier_valid_normal_count,
        curated_hero_tier_valid_rage_count,
        validated_skill_set_size: VALIDATED_SKILL_IDS.len(),
    }
}
