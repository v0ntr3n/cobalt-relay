//! Crash-proof Turn-Based Battle Simulator (`FightProxy.lua`, `FightOperateLogic.lua`, `BaseUnit.lua`, `PropertyKey.lua`),
//! PvE Campaign Copy (`CopyData.lua`, `CopyProxy.lua`, `CopyInfoDataController.lua`, `EliteCopyData.lua`),
//! Daily Copy (`DailyCopyData.lua`, `DailyCopyProxy.lua`), Babel Tower (`BabelTowerData.lua`, `BabelTowerProxy.lua`),
//! Trial Tower & Purgatory (`DotaData.lua`, `DotaProxy.lua`), Awake Copy (`AwakeCopyData.lua`),
//! World Boss / Fiend / Rebel (`WorldBossData.lua`, `FiendData.lua`, `RebelData.lua`, `RebelProxy.lua`),
//! Territory Patrol (`DiscoverData.lua`), Devil Copy (`DevilCopyProxy.lua`), Soaring Copy (`SoaringCopyProxy.lua`),
//! PvP Arena & TimeMachine Arena (`ArenaData.lua`, `ArenaProxy.lua`, `TmArenaData.lua`, `TmArenaProxy.lua`),
//! Sign-In & Super Check-In (`SignInData.lua`, `ActivitySignInProxy.lua`, `SuperCheckInData.lua`),
//! Online Rewards (`ActOnlineData.lua`, `OnlineOpenProxy.lua`), Daily Quests & Tasks (`DayTaskData.lua`, `TaskData.lua`),
//! Achievements (`AchievementData.lua`), and Stamina Supply Banquet (`ActChickenData.lua`, `ActivityChickenProxy.lua`).

use std::collections::{BTreeMap, BTreeSet};

use crate::mail::{grant_rewards_to_player, RewardEntry};
use crate::player::{HeroInstance, PlayerState};
use crate::protocol::{decode_amf3, encode_amf3, push, DtoValue, PushPacket};

// ============================================================================
// Pure-Rust Safe RFC 1950 Zlib + AMF3 Binary Helpers (Zero External Crates)
// ============================================================================

const MOD_ADLER: u32 = 65_521;
const ADLER_NMAX: usize = 5_552;

/// Computes the RFC 1950 Adler-32 checksum of `data`.
pub fn adler32_checksum(data: &[u8]) -> u32 {
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for chunk in data.chunks(ADLER_NMAX) {
        for &b in chunk {
            s1 += u32::from(b);
            s2 += s1;
        }
        s1 %= MOD_ADLER;
        s2 %= MOD_ADLER;
    }
    (s2 << 16) | s1
}

/// Wraps `data` in a valid RFC 1950 Zlib stream (`0x78 0x01` + RFC 1951 Stored blocks + Adler-32)
/// so `GameUtil:unzipData(blob)` in Cocos2d-Lua and `gserver_protocol::zlib_decompress` decode it cleanly.
pub fn zlib_compress_rfc1950(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + (data.len() / 65_535 + 1) * 5 + 6);
    // CMF = 0x78 (deflate, 32K window), FLG = 0x01 ((0x78 * 256 + 0x01) % 31 == 0)
    out.push(0x78);
    out.push(0x01);

    if data.is_empty() {
        // Single final empty stored block: BFINAL=1, BTYPE=00 -> 0x01
        out.push(0x01);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
    } else {
        let chunks: Vec<&[u8]> = data.chunks(65_535).collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let is_final = i + 1 == chunks.len();
            out.push(if is_final { 0x01 } else { 0x00 });
            let len = chunk.len() as u16;
            let nlen = !len;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&nlen.to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    out.extend_from_slice(&adler32_checksum(data).to_be_bytes());
    out
}

/// Decompresses an RFC 1950 Zlib Stored-block stream produced by [`zlib_compress_rfc1950`],
/// verifying the RFC 1950 header and Adler-32 integrity trailer.
pub fn zlib_decompress_rfc1950(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() < 6 {
        return Err("zlib_eof");
    }
    let cmf = input[0];
    let flg = input[1];
    if (cmf & 0x0F) != 8 || ((u16::from(cmf) << 8 | u16::from(flg)) % 31) != 0 || (flg & 0x20) != 0 {
        return Err("zlib_bad_header");
    }
    let payload_end = input.len() - 4;
    let mut pos = 2usize;
    let mut out = Vec::new();
    loop {
        if pos >= payload_end {
            return Err("zlib_truncated_block");
        }
        let hdr = input[pos];
        pos += 1;
        let bfinal = (hdr & 0x01) != 0;
        let btype = (hdr >> 1) & 0x03;
        if btype != 0 {
            return Err("zlib_unsupported_block_type");
        }
        if pos + 4 > payload_end {
            return Err("zlib_truncated_stored_len");
        }
        let len = u16::from_le_bytes([input[pos], input[pos + 1]]);
        let nlen = u16::from_le_bytes([input[pos + 2], input[pos + 3]]);
        pos += 4;
        if len != !nlen {
            return Err("zlib_stored_len_mismatch");
        }
        let chunk_len = usize::from(len);
        if pos + chunk_len > payload_end {
            return Err("zlib_truncated_stored_data");
        }
        out.extend_from_slice(&input[pos..pos + chunk_len]);
        pos += chunk_len;
        if bfinal {
            break;
        }
    }
    let expected = u32::from_be_bytes([
        input[payload_end],
        input[payload_end + 1],
        input[payload_end + 2],
        input[payload_end + 3],
    ]);
    if adler32_checksum(&out) != expected {
        return Err("zlib_adler32_mismatch");
    }
    Ok(out)
}

/// Encodes `val` into AMF3 and compresses it into an RFC 1950 Zlib binary blob (`0x78...`)
/// matching `FightProxy:decodeFightReport(blob)` (`amf3.decode(GameUtil:unzipData(blob))`).
pub fn encode_zlib_amf3(val: &DtoValue) -> Vec<u8> {
    zlib_compress_rfc1950(&encode_amf3(val))
}

/// Decompresses an RFC 1950 Zlib binary blob and decodes the inner AMF3 value.
pub fn decode_zlib_amf3(blob: &[u8]) -> Result<DtoValue, &'static str> {
    let raw = zlib_decompress_rfc1950(blob)?;
    decode_amf3(&raw).map_err(|_| "amf3_decode_failed")
}

// ============================================================================
// Section 1: Battle Engine (`serverData` & `fightReport`)
// ============================================================================

/// Authentic monster `htid`s (`>= 100_001` in `monster.db` table `monster`),
/// guaranteeing `UnitFactory:creatUnit` routes to `MonsterReader:getRowById` and resolves real rows!
pub const VALID_MONSTER_HTIDS: &[u32] = &[
    1_010_111, // Chapter 1 Stronghold 101
    1_010_113,
    1_010_115,
    1_010_117,
    1_010_119,
    1_020_111, // Chapter 1 Stronghold 102
    1_020_113,
    1_020_115,
    1_030_111, // Chapter 1 Stronghold 103
    1_030_113,
    1_040_111, // Chapter 1 Stronghold 104
    1_040_113,
    1_050_111, // Chapter 1 Stronghold 105
    1_050_113,
    100_001,   // Opening battle monsters
    100_002,
    100_003,
    100_004,
    500_101,   // Special / Boss monsters
    600_101,
];

fn is_valid_hero_template_id(htid: u32) -> bool {
    htid > 0
        && htid <= 20_000
        && (gserver_data::hero_skill_entry_by_htid(htid).is_some()
            || gserver_data::find_inherited_tier_skill_entry(htid).is_some())
}

/// Resolves authentic monster `htid` pairs (`>= 100_001` in `monster.db` table `monster`) for a campaign stronghold.
pub fn stage_monster_htids(copy_id: u32) -> (u32, u32) {
    match copy_id {
        100_001 => (100_001, 100_002),
        100_101 => (1_010_111, 1_010_113),
        100_102 => (1_020_111, 1_020_113),
        100_103 => (1_030_111, 1_030_113),
        100_105 => (1_050_111, 1_050_113),
        100_106 => (1_040_111, 1_040_113),
        _ => {
            let idx1 = (copy_id as usize) % VALID_MONSTER_HTIDS.len();
            let idx2 = (idx1 + 1) % VALID_MONSTER_HTIDS.len();
            (VALID_MONSTER_HTIDS[idx1], VALID_MONSTER_HTIDS[idx2])
        }
    }
}

/// Combat unit snapshot used by the deterministic simulator and `battleInfo.arrTeam*` DTO builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombatUnit {
    pub hid: u64,
    pub htid: u32,
    /// Zero-based formation slot `0..=5` (`FightOperateLogic.lua`: `postionIndex = position + 1`).
    pub position: u32,
    pub level: u32,
    pub country: u32,
    pub dress_id: u32,
    pub star_lv: u32,
    pub evolve_level: u32,
    pub awake_rank: u32,
    pub awake_level: u32,
    pub max_hp: i64,
    pub cur_hp: i64,
    pub atk: i64,
    pub pdef: i64,
    pub mdef: i64,
    pub speed: i64,
    /// Standard `vnsg2` `BaseUnit` / `PropertyKey["27".."28"]` rage scale (`50` init, `100` max/cost).
    pub cur_rage: i32,
    pub max_rage: i32,
    pub rage_cost: i32,
    pub normal_skill_id: u32,
    pub rage_skill_id: u32,
    pub damage_dealt: i64,
    pub damage_taken: i64,
    pub kills: u32,
}

impl CombatUnit {
    pub fn from_hero(hero: &HeroInstance, position: u32) -> Self {
        let safe_htid = if is_valid_hero_template_id(hero.htid) || hero.htid > 20_000 {
            hero.htid
        } else {
            1 // Nam Chủ Tướng fallback if caller constructed an unmapped <=20000 htid
        };
        let canonical =
            gserver_data::resolve_hero_battle_skills_with_awake(safe_htid, hero.awake_rank);
        let has_stale_fallback = hero.normal_skill_id == gserver_data::FALLBACK_NORMAL_SKILL_ID
            && hero.rage_skill_id == gserver_data::FALLBACK_RAGE_SKILL_ID
            && !canonical.used_any_fallback();
        let normal_skill_id = if !has_stale_fallback
            && hero.normal_skill_id > 0
            && gserver_data::is_validated_skill_id(hero.normal_skill_id)
        {
            hero.normal_skill_id
        } else {
            canonical.normal_skill_id
        };
        let rage_skill_id = if !has_stale_fallback
            && hero.rage_skill_id > 0
            && gserver_data::is_validated_skill_id(hero.rage_skill_id)
        {
            hero.rage_skill_id
        } else {
            canonical.rage_skill_id
        };
        Self {
            hid: hero.hid,
            htid: safe_htid,
            position: position.min(5),
            level: hero.level.max(1),
            country: hero.country.clamp(1, 4),
            dress_id: 0, // Must be 0 unless validated against fashion.db
            star_lv: hero.quality.max(1),
            evolve_level: hero.evolve_level,
            awake_rank: hero.awake_rank,
            awake_level: hero.awake_level,
            max_hp: hero.max_hp.max(1),
            cur_hp: hero.cur_hp.clamp(1, hero.max_hp.max(1)),
            atk: hero.atk.max(10),
            pdef: hero.pdef.max(0),
            mdef: hero.mdef.max(0),
            speed: hero.speed.max(1),
            cur_rage: 50,
            max_rage: 100,
            rage_cost: 100,
            normal_skill_id,
            rage_skill_id,
            damage_dealt: 0,
            damage_taken: 0,
            kills: 0,
        }
    }

    pub fn monster(hid: u64, htid: u32, position: u32, level: u32) -> Self {
        // Preserve valid hero templates (e.g. Arena NPC guards 310 / PvP protagonist 2),
        // preserve valid monster.db IDs (>= 100_001), and remap any invalid < 100_001 non-hero ID
        // (such as legacy 5001..5010 or strongh icon_ids 20_001..20_024) to a verified monster.db ID!
        let safe_htid = if is_valid_hero_template_id(htid) || htid >= 100_001 {
            htid
        } else {
            VALID_MONSTER_HTIDS[(htid as usize) % VALID_MONSTER_HTIDS.len()]
        };
        let lv = i64::from(level.max(1));
        let max_hp = 800 + lv * 260;
        let (normal_skill_id, rage_skill_id) =
            gserver_data::resolve_unit_or_monster_battle_skills(safe_htid);
        let country = gserver_data::hero_skill_entry_by_htid(safe_htid)
            .filter(|e| e.country > 0)
            .map(|e| u32::from(e.country))
            .unwrap_or(4);
        Self {
            hid,
            htid: safe_htid,
            position: position.min(5),
            level: level.max(1),
            country,
            dress_id: 0,
            star_lv: 3,
            evolve_level: 0,
            awake_rank: 0,
            awake_level: 0,
            max_hp,
            cur_hp: max_hp,
            atk: 140 + lv * 40,
            pdef: 60 + lv * 18,
            mdef: 60 + lv * 18,
            speed: 85 + lv,
            cur_rage: 50,
            max_rage: 100,
            rage_cost: 100,
            normal_skill_id,
            rage_skill_id,
            damage_dealt: 0,
            damage_taken: 0,
            kills: 0,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.cur_hp > 0
    }

    /// Serializes a unit into the exact `BaseUnit:init(data)` + `PropertyKey.lua` schema
    /// (`vnsg2_playable_modes_audit.md` Section 1.3) so `UnitFactory:createUnit` never hits nil fields.
    pub fn to_unit_dto(&self) -> DtoValue {
        DtoValue::map([
            ("hid", DtoValue::from(self.hid)),
            ("htid", DtoValue::from(self.htid)),
            ("position", DtoValue::from(self.position.min(5))),
            ("level", DtoValue::from(self.level.max(1))),
            ("maxHp", DtoValue::from(self.max_hp.max(1))),
            ("curHp", DtoValue::from(self.cur_hp.max(0))),
            ("currHp", DtoValue::from(self.cur_hp.max(0))),
            ("country", DtoValue::from(self.country.clamp(1, 4))),
            ("maxRage", DtoValue::from(self.max_rage)),
            ("curRage", DtoValue::from(self.cur_rage)),
            ("rage", DtoValue::from(self.cur_rage)),
            ("rage_cost", DtoValue::from(self.rage_cost)),
            ("dress_id", DtoValue::from(self.dress_id)),
            ("dress", DtoValue::from(self.dress_id)),
            ("star_lv", DtoValue::from(self.star_lv)),
            ("evolve_level", DtoValue::from(self.evolve_level)),
            ("awake_rank", DtoValue::from(self.awake_rank)),
            ("awake_level", DtoValue::from(self.awake_level)),
            ("gold_rank", DtoValue::from(0u32)),
            ("gold_level", DtoValue::from(0u32)),
            ("normalSkill", DtoValue::from(self.normal_skill_id)),
            ("rageSkill", DtoValue::from(self.rage_skill_id)),
            (
                "arrSkill",
                DtoValue::array([
                    DtoValue::array([DtoValue::from(self.normal_skill_id), DtoValue::from(1u32)]),
                    DtoValue::array([DtoValue::from(self.rage_skill_id), DtoValue::from(1u32)]),
                ]),
            ),
            ("ImmuneIds", DtoValue::array(Vec::<DtoValue>::new())),
            // PropertyKey numeric IDs (`1`=hpBase, `2`=meleeAtkBase, `3`=magicAtkBase,
            // `4`=meleeDefBase, `5`=magicDefBase, `9`=generalAtkBase, `10`=speedBase,
            // `27`=initRage(50), `28`=rageTop(100), `32`=critRate, `34`=hitRate(10000=100%))
            ("1", DtoValue::from(self.max_hp.max(1))),
            ("2", DtoValue::from(self.atk)),
            ("3", DtoValue::from(self.atk)),
            ("4", DtoValue::from(self.pdef)),
            ("5", DtoValue::from(self.mdef)),
            ("9", DtoValue::from(self.atk)),
            ("10", DtoValue::from(self.speed)),
            ("27", DtoValue::from(self.cur_rage)),
            ("28", DtoValue::from(self.max_rage)),
            ("32", DtoValue::from(500i32)),
            ("34", DtoValue::from(10_000i32)),
        ])
    }
}

/// Action log step recorded in `arrReaction` & `rageQueue` battle replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BattleStep {
    pub round: u32,
    pub attack_round: u32,
    pub attacker_hid: u64,
    pub defender_hid: u64,
    pub skill_id: u32,
    /// `1` = normal attack (`com_atk`), `2` = rage attack (`angry_atk`).
    pub skill_index: u32,
    pub is_rage_skill: bool,
    pub damage: i64,
    pub defender_hp_after: i64,
}

/// Complete battle simulation result and `FightProxy` report structure (`BattleRecord`).
#[derive(Debug, Clone)]
pub struct SimulatedBattle {
    pub brid: u64,
    pub is_win: bool,
    /// `FightProxy:isSuccessFight()` checks `appraisal == 50`! (`50` = Win, `10` = Defeat)
    pub appraisal: u32,
    pub stars: u32,
    pub rounds: u32,
    pub steps: Vec<BattleStep>,
    pub server_attack_dto: DtoValue,
    pub fight_report_dto: DtoValue,
}

/// Alias matching `BattleRecord::to_report_dto` in audit documentation.
pub type BattleRecord = SimulatedBattle;

impl SimulatedBattle {
    /// Returns the decoded `fightReport` DTO table (`Section 1.4`).
    pub fn to_report_dto(&self) -> DtoValue {
        self.fight_report_dto.clone()
    }

    /// Encodes `fightReport` as raw AMF3 bytes (used inside `DtoValue::Bytes` when `gserver`
    /// `bridge::dto_to_amf3(..., true)` applies RFC 1950 Zlib compression at the wire boundary).
    pub fn to_amf3_report_bytes(&self) -> Vec<u8> {
        encode_amf3(&self.fight_report_dto)
    }

    /// Encodes `fightReport` directly into an RFC 1950 Zlib-compressed AMF3 binary blob (`0x78...`)
    /// for standalone Type B consumers (`FightProxy:decodeFightReport(blob)`).
    pub fn to_zlib_amf3_report_bytes(&self) -> Vec<u8> {
        encode_zlib_amf3(&self.fight_report_dto)
    }
}

/// Builds the `CopyProxy:groupDrops(data)` drop dictionary (`CopyProxy.lua:995-1008`):
/// `{ copper = <int>, soul = <int>, item = { ["<item_tid>"] = <num>, ... } }`.
pub fn build_copy_drop_dto(drops: &[RewardEntry]) -> DtoValue {
    let mut copper: i64 = 0;
    let mut soul: u64 = 0;
    let mut item_map: BTreeMap<String, DtoValue> = BTreeMap::new();
    for entry in drops {
        match *entry {
            RewardEntry::Copper(c) => copper = copper.saturating_add(c),
            RewardEntry::HeroSoul(s) => soul = soul.saturating_add(s),
            RewardEntry::Item { tid, qty } => {
                let prev = item_map
                    .get(&tid.to_string())
                    .and_then(DtoValue::as_u64)
                    .unwrap_or(0);
                item_map.insert(tid.to_string(), DtoValue::from(prev + u64::from(qty)));
            }
            _ => {}
        }
    }
    DtoValue::map([
        ("copper", DtoValue::from(copper)),
        ("soul", DtoValue::from(soul)),
        ("item", DtoValue::Map(item_map)),
    ])
}

fn ensure_disjoint_team_hids(team1: &[CombatUnit], team2: &[CombatUnit]) -> Vec<CombatUnit> {
    let used: BTreeSet<u64> = team1.iter().map(|u| u.hid).collect();
    let mut fixed = team2.to_vec();
    let mut seen = used;
    for (idx, u) in fixed.iter_mut().enumerate() {
        if u.hid == 0 || seen.contains(&u.hid) {
            let mut candidate = 90_000_001u64 + idx as u64;
            while seen.contains(&candidate) {
                candidate += 1;
            }
            u.hid = candidate;
        }
        seen.insert(u.hid);
    }
    fixed
}

/// Builds the crash-proof `*.attack` / `serverData` DTO verified from `FightProxy:parseBattleInfo`
/// and `FightOperateLogic:startFight` (`brid`, `arrSeed`, `atkTurn`, `drop`, `battleInfo`,
/// and string-keyed `userInfo = {"1": u1, "2": u2}`).
#[allow(clippy::too_many_arguments)]
pub fn build_attack_server_dto(
    brid: u64,
    is_pvp: bool,
    team1_units: &[CombatUnit],
    team2_units: &[CombatUnit],
    user1_dto: DtoValue,
    user2_dto: DtoValue,
    drops: &[RewardEntry],
) -> DtoValue {
    let safe_team2 = ensure_disjoint_team_hids(team1_units, team2_units);
    let build_team_dto = |units: &[CombatUnit]| -> DtoValue {
        let mut arr_hero = BTreeMap::new();
        for (idx, u) in units.iter().enumerate() {
            arr_hero.insert((idx + 1).to_string(), u.to_unit_dto());
        }
        DtoValue::map([
            ("curRage", DtoValue::from(0u32)),
            ("rageMax", DtoValue::from(100u32)),
            ("morale", DtoValue::from(0u32)),
            ("purpleGold_morale", DtoValue::from(0u32)),
            ("jobLv", DtoValue::from(0u32)),
            ("jobAttrs", DtoValue::Map(BTreeMap::new())),
            ("arrHero", DtoValue::Map(arr_hero)),
            ("arrGodAnimalInfo", DtoValue::Map(BTreeMap::new())),
            ("arrGodAnimalSkill", DtoValue::Map(BTreeMap::new())),
            ("arrTopAnimalInfo", DtoValue::Map(BTreeMap::new())),
            ("arrTopAnimalSkill", DtoValue::Map(BTreeMap::new())),
            ("beautySkill", DtoValue::Array(Vec::new())),
            ("schemeSkill", DtoValue::Map(BTreeMap::new())),
            ("militarySkill", DtoValue::Map(BTreeMap::new())),
        ])
    };

    let mut arr_team1 = BTreeMap::new();
    arr_team1.insert("1".to_owned(), build_team_dto(team1_units));
    let mut arr_team2 = BTreeMap::new();
    arr_team2.insert("1".to_owned(), build_team_dto(&safe_team2));

    let battle_info = DtoValue::map([
        ("isPVP", DtoValue::from(if is_pvp { 1u32 } else { 0u32 })),
        ("replaceInfo", DtoValue::array([DtoValue::from(0u32)])),
        ("arrTeam1", DtoValue::Map(arr_team1)),
        ("arrTeam2", DtoValue::Map(arr_team2)),
        ("extraMonster", DtoValue::Map(BTreeMap::new())),
        ("skillRate", DtoValue::Map(BTreeMap::new())),
    ]);

    let seed1 = ((brid.wrapping_mul(1_103_515_245).wrapping_add(12_345)) & 0x7FFF_FFFF).max(1);
    let seed2 = ((seed1.wrapping_mul(1_103_515_245).wrapping_add(54_321)) & 0x7FFF_FFFF).max(1);

    let mut user_info_map = BTreeMap::new();
    user_info_map.insert("1".to_owned(), user1_dto);
    user_info_map.insert("2".to_owned(), user2_dto);

    DtoValue::map([
        ("brid", DtoValue::from(brid)),
        ("atkTurn", DtoValue::from(1u32)),
        (
            "arrSeed",
            DtoValue::array([DtoValue::from(seed1), DtoValue::from(seed2)]),
        ),
        ("drop", build_copy_drop_dto(drops)),
        ("battleInfo", battle_info),
        ("userInfo", DtoValue::Map(user_info_map)),
    ])
}

/// Runs a deterministic turn-based battle (`round_limit = 20`) and constructs both the
/// Phase 1 `serverData` DTO and the Phase 2 / Type B `fightReport` DTO (`Section 1.3` & `1.4`).
#[allow(clippy::too_many_arguments)]
pub fn simulate_battle(
    brid: u64,
    is_pvp: bool,
    initial_team1: &[CombatUnit],
    initial_team2: &[CombatUnit],
    user1_dto: DtoValue,
    user2_dto: DtoValue,
    drops: &[RewardEntry],
) -> SimulatedBattle {
    let safe_initial_t2 = ensure_disjoint_team_hids(initial_team1, initial_team2);
    let server_attack_dto = build_attack_server_dto(
        brid,
        is_pvp,
        initial_team1,
        &safe_initial_t2,
        user1_dto,
        user2_dto,
        drops,
    );

    let mut t1 = initial_team1.to_vec();
    let mut t2 = safe_initial_t2;
    let mut steps = Vec::new();
    let round_limit = 20u32;
    let mut round = 0u32;
    let mut attack_round_counter = 0u32;

    while round < round_limit
        && t1.iter().any(CombatUnit::is_alive)
        && t2.iter().any(CombatUnit::is_alive)
    {
        round += 1;
        for slot in 0..6usize {
            // Team 1 unit at `slot` attacks first alive Team 2 unit
            if let Some(a_idx) = t1
                .iter()
                .position(|u| u.is_alive() && u.position as usize == slot)
            {
                if let Some(d_idx) = t2.iter().position(CombatUnit::is_alive) {
                    attack_round_counter += 1;
                    let use_rage = t1[a_idx].cur_rage >= t1[a_idx].rage_cost;
                    let (skill_id, skill_index) = if use_rage {
                        t1[a_idx].cur_rage -= t1[a_idx].rage_cost;
                        (t1[a_idx].rage_skill_id, 2u32)
                    } else {
                        t1[a_idx].cur_rage = (t1[a_idx].cur_rage + 50).min(t1[a_idx].max_rage);
                        (t1[a_idx].normal_skill_id, 1u32)
                    };
                    let base_dmg = (t1[a_idx].atk - t2[d_idx].pdef / 2)
                        .max(t1[a_idx].atk / 5)
                        .max(1);
                    let dmg = if use_rage {
                        base_dmg * 22 / 10
                    } else {
                        base_dmg
                    };
                    t2[d_idx].cur_hp = (t2[d_idx].cur_hp - dmg).max(0);
                    t1[a_idx].damage_dealt += dmg;
                    t2[d_idx].damage_taken += dmg;
                    if t2[d_idx].cur_hp == 0 {
                        t1[a_idx].kills += 1;
                    }
                    steps.push(BattleStep {
                        round,
                        attack_round: attack_round_counter,
                        attacker_hid: t1[a_idx].hid,
                        defender_hid: t2[d_idx].hid,
                        skill_id,
                        skill_index,
                        is_rage_skill: use_rage,
                        damage: dmg,
                        defender_hp_after: t2[d_idx].cur_hp,
                    });
                    if !t2.iter().any(CombatUnit::is_alive) {
                        break;
                    }
                }
            }

            // Team 2 unit at `slot` attacks first alive Team 1 unit
            if let Some(a_idx) = t2
                .iter()
                .position(|u| u.is_alive() && u.position as usize == slot)
            {
                if let Some(d_idx) = t1.iter().position(CombatUnit::is_alive) {
                    attack_round_counter += 1;
                    let use_rage = t2[a_idx].cur_rage >= t2[a_idx].rage_cost;
                    let (skill_id, skill_index) = if use_rage {
                        t2[a_idx].cur_rage -= t2[a_idx].rage_cost;
                        (t2[a_idx].rage_skill_id, 2u32)
                    } else {
                        t2[a_idx].cur_rage = (t2[a_idx].cur_rage + 50).min(t2[a_idx].max_rage);
                        (t2[a_idx].normal_skill_id, 1u32)
                    };
                    let base_dmg = (t2[a_idx].atk - t1[d_idx].pdef / 2)
                        .max(t2[a_idx].atk / 5)
                        .max(1);
                    let dmg = if use_rage {
                        base_dmg * 20 / 10
                    } else {
                        base_dmg
                    };
                    t1[d_idx].cur_hp = (t1[d_idx].cur_hp - dmg).max(0);
                    t2[a_idx].damage_dealt += dmg;
                    t1[d_idx].damage_taken += dmg;
                    if t1[d_idx].cur_hp == 0 {
                        t2[a_idx].kills += 1;
                    }
                    steps.push(BattleStep {
                        round,
                        attack_round: attack_round_counter,
                        attacker_hid: t2[a_idx].hid,
                        defender_hid: t1[d_idx].hid,
                        skill_id,
                        skill_index,
                        is_rage_skill: use_rage,
                        damage: dmg,
                        defender_hp_after: t1[d_idx].cur_hp,
                    });
                    if !t1.iter().any(CombatUnit::is_alive) {
                        break;
                    }
                }
            }
        }
    }

    let is_win = !t2.iter().any(CombatUnit::is_alive) && t1.iter().any(CombatUnit::is_alive);
    let appraisal = if is_win { 50 } else { 10 };
    let dead_allies = t1.iter().filter(|u| !u.is_alive()).count();
    let stars = if !is_win {
        0
    } else if dead_allies == 0 {
        3
    } else if dead_allies == 1 {
        2
    } else {
        1
    };

    let arr_reaction_log = DtoValue::Array(
        steps
            .iter()
            .map(|s| {
                DtoValue::map([
                    ("round", DtoValue::from(s.round)),
                    ("attackRound", DtoValue::from(s.attack_round)),
                    ("atk", DtoValue::from(s.attacker_hid)),
                    ("skillIndex", DtoValue::from(s.skill_index)),
                    ("skillId", DtoValue::from(s.skill_id)),
                    (
                        "arrReaction",
                        DtoValue::array([DtoValue::map([
                            ("defender", DtoValue::from(s.defender_hid)),
                            ("damage", DtoValue::from(s.damage)),
                            ("curHp", DtoValue::from(s.defender_hp_after)),
                        ])]),
                    ),
                ])
            })
            .collect(),
    );

    let rage_queue = DtoValue::Array(
        steps
            .iter()
            .filter(|s| s.is_rage_skill)
            .map(|s| {
                DtoValue::map([
                    ("round", DtoValue::from(s.round)),
                    ("attackRound", DtoValue::from(s.attack_round)),
                    ("attackRoundNum", DtoValue::from(s.attack_round)),
                    ("atk", DtoValue::from(s.attacker_hid)),
                    ("hid", DtoValue::from(s.attacker_hid)),
                    ("skillIndex", DtoValue::from(s.skill_index)),
                    ("skillId", DtoValue::from(s.skill_id)),
                    ("fireRageType", DtoValue::from(1u32)),
                    ("isUserJump", DtoValue::from(false)),
                    (
                        "arrReaction",
                        DtoValue::array([DtoValue::map([
                            ("defender", DtoValue::from(s.defender_hid)),
                            ("damage", DtoValue::from(s.damage)),
                            ("curHp", DtoValue::from(s.defender_hp_after)),
                        ])]),
                    ),
                ])
            })
            .collect(),
    );

    let team_res_dto = |units: &[CombatUnit]| -> DtoValue {
        DtoValue::Array(
            units
                .iter()
                .map(|u| {
                    let cost_hp = (u.max_hp - u.cur_hp).max(0);
                    DtoValue::map([
                        ("hid", DtoValue::from(u.hid)),
                        ("curHp", DtoValue::from(u.cur_hp)),
                        ("maxHp", DtoValue::from(u.max_hp)),
                        ("costHp", DtoValue::from(cost_hp)),
                        ("curRage", DtoValue::from(u.cur_rage)),
                        ("curGildedEnergy", DtoValue::from(0u32)),
                    ])
                })
                .collect(),
        )
    };

    // Section 1.4: `heroPercentInfo` MUST be a flat 1-indexed Array across all units!
    let hero_percent_info = DtoValue::Array(
        t1.iter()
            .chain(t2.iter())
            .map(|u| {
                let hp_pct = ((u.cur_hp.max(0) * 100) / u.max_hp.max(1)).clamp(0, 100);
                let rage_pct =
                    ((i64::from(u.cur_rage.max(0)) * 100) / i64::from(u.max_rage.max(1))).clamp(0, 100);
                DtoValue::map([
                    ("hid", DtoValue::from(u.hid)),
                    ("hp_percent", DtoValue::from(hp_pct)),
                    ("rage_percent", DtoValue::from(rage_pct)),
                ])
            })
            .collect(),
    );

    // Section 1.4: `statisticalDataInfo` MUST match `StatisticsPanel.lua:45-53`!
    let build_stat_team = |units: &[CombatUnit], is_enemy: bool| -> DtoValue {
        let all_damage: i64 = units.iter().map(|u| u.damage_dealt).sum();
        let info: Vec<DtoValue> = units
            .iter()
            .map(|u| {
                DtoValue::map([
                    ("isEnemy", DtoValue::from(is_enemy)),
                    ("hid", DtoValue::from(u.hid)),
                    ("htid", DtoValue::from(u.htid)),
                    ("dress_id", DtoValue::from(u.dress_id)),
                    ("awake_rank", DtoValue::from(u.awake_rank)),
                    ("awake_level", DtoValue::from(u.awake_level)),
                    ("numOfCure", DtoValue::from(0i64)),
                    ("numOfDamage", DtoValue::from(u.damage_dealt)),
                    ("numOfBeDamage", DtoValue::from(u.damage_taken)),
                    ("numOfKill", DtoValue::from(u.kills)),
                ])
            })
            .collect();
        DtoValue::map([
            ("allCure", DtoValue::from(0i64)),
            ("allDamage", DtoValue::from(all_damage)),
            ("info", DtoValue::Array(info)),
        ])
    };

    let statistical_data_info = DtoValue::map([
        ("team1", build_stat_team(&t1, false)),
        ("team2", build_stat_team(&t2, true)),
    ]);

    let fight_base_data = DtoValue::map([
        ("bgPic", DtoValue::array([DtoValue::from(1u32)])),
        ("appearance_type", DtoValue::Array(Vec::new())),
        ("fightCondition", DtoValue::Array(Vec::new())),
        ("showHero", DtoValue::Array(Vec::new())),
        ("isJump", DtoValue::from(true)),
    ]);

    let fight_report_dto = DtoValue::map([
        ("brid", DtoValue::from(brid)),
        ("appraisal", DtoValue::from(appraisal)),
        ("result", DtoValue::from(if is_win { 1u32 } else { 0u32 })),
        ("isWin", DtoValue::from(is_win)),
        ("round", DtoValue::from(round.max(1))),
        ("attackRound", DtoValue::from(attack_round_counter.max(1))),
        ("runTime", DtoValue::from(0.01f64)),
        ("svnVersion", DtoValue::from(0u32)),
        ("csvSvnVersion", DtoValue::from(0u32)),
        ("fightBaseData", fight_base_data),
        ("serverData", server_attack_dto.clone()),
        ("arrReaction", arr_reaction_log),
        ("rageQueue", rage_queue),
        (
            "heroResoultInfo",
            DtoValue::array([DtoValue::map([
                ("team1", team_res_dto(&t1)),
                ("team2", team_res_dto(&t2)),
            ])]),
        ),
        ("heroPercentInfo", hero_percent_info),
        ("statisticalDataInfo", statistical_data_info),
    ]);

    SimulatedBattle {
        brid,
        is_win,
        appraisal,
        stars,
        rounds: round.max(1),
        steps,
        server_attack_dto,
        fight_report_dto,
    }
}

pub fn extract_player_combat_units(player: &PlayerState) -> Vec<CombatUnit> {
    let mut units = Vec::new();
    for (slot, &hid) in player.roster.formation.iter().enumerate() {
        if hid != 0 {
            if let Some(hero) = player.roster.heroes.get(&hid) {
                units.push(CombatUnit::from_hero(hero, slot as u32));
            }
        }
    }
    if units.is_empty() {
        let fallback_protagonist =
            HeroInstance::from_template(10_000_001, player.utid.max(1), player.level.max(1));
        units.push(CombatUnit::from_hero(&fallback_protagonist, 0));
    }
    units
}

pub fn build_user_battle_dto(
    uid: u64,
    uname: &str,
    level: u32,
    vip: u32,
    fight_force: u64,
    dress_id: u32,
) -> DtoValue {
    DtoValue::map([
        ("uid", DtoValue::from(uid)),
        ("uname", DtoValue::from(uname)),
        ("utid", DtoValue::from(1u32)),
        ("level", DtoValue::from(level.max(1))),
        ("vip", DtoValue::from(vip)),
        ("fight_force", DtoValue::from(fight_force)),
        ("dress_id", DtoValue::from(dress_id)),
        ("quality", DtoValue::from(4u32)),
        ("soul_stone_power", DtoValue::from(0u32)),
        ("soulJadeQi", DtoValue::from(0u32)),
        ("generalBookAttrFightForce", DtoValue::from(0u32)),
    ])
}

// ============================================================================
// Section 2.1: Normal Copy, TimeMachine Copy & Elite Copy (`CopyCampaignEngine`)
// ============================================================================

/// Returns the exact stronghold IDs for a chapter from `copy/copy.json` & `copy_small/copy_elite.json`
/// (e.g. Chapter `1001` has 5 stages `[100101, 100102, 100103, 100105, 100106]` — `100104` does NOT exist!).
pub fn chapter_stage_ids(chapter_id: u32) -> Vec<u32> {
    match chapter_id {
        1000 => vec![100_001],
        1001 => vec![100_101, 100_102, 100_103, 100_105, 100_106],
        1002 => (1..=6).map(|i| 100_200 + i).collect(),
        1003 => (1..=8).map(|i| 100_300 + i).collect(),
        1111 => (1..=30).map(|i| 111_100 + i).collect(),
        1004..=1204 => (1..=10).map(|i| chapter_id * 100 + i).collect(),
        // Elite Copy (`copy_small/copy_elite.json`: `2001`..`2002` have 3 stages, `2003`..`2170` have 4 stages)
        2001 | 2002 => (1..=3).map(|i| chapter_id * 100 + i).collect(),
        2003..=2170 => (1..=4).map(|i| chapter_id * 100 + i).collect(),
        _ => Vec::new(),
    }
}

/// Returns the exact Time Machine stronghold IDs from `timeMachine_copy/timeMachine_copy.json` (`1001..=1150`, 10 stages each).
pub fn tm_chapter_stage_ids(chapter_id: u32) -> Vec<u32> {
    match chapter_id {
        1001..=1150 => (1..=10).map(|i| chapter_id * 100 + i).collect(),
        _ => chapter_stage_ids(chapter_id),
    }
}

/// Returns the exact `open_copy_id` unlocked upon clearing the final stronghold of `chapter_id`.
pub fn next_open_chapter_id(chapter_id: u32) -> Option<u32> {
    match chapter_id {
        1000..=1203 => Some(chapter_id + 1),
        1204 => None,
        2001..=2169 => Some(chapter_id + 1),
        2170 => None,
        _ => None,
    }
}

/// Returns the exact `score_require` star chest thresholds from `copy/copy.json` & `copy_elite.json`.
pub fn chapter_star_thresholds(chapter_id: u32) -> &'static [u32] {
    match chapter_id {
        1000 => &[],
        1001 => &[5, 10, 15],
        1002 => &[6, 12, 18],
        1003 => &[8, 16, 24],
        1111 => &[30, 60, 90],
        1004..=1204 => &[10, 20, 30],
        2001 | 2002 => &[9],
        2003..=2170 => &[12],
        _ => &[10, 20, 30],
    }
}

#[derive(Debug, Clone, Default)]
pub struct StageRecord {
    pub stars: u32,
    pub attacks_today: u32,
    pub resets_today: u32,
    pub box_claimed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChapterProgress {
    pub stages: BTreeMap<u32, StageRecord>,
    /// Bitmask of claimed chapter star chests (`box_idx` `0..=2`).
    pub prize_mask: u32,
}

#[derive(Debug, Clone)]
pub struct PendingStageBattle {
    pub brid: u64,
    pub chapter_id: u32,
    pub copy_id: u32,
    pub sim: SimulatedBattle,
    pub drops: Vec<RewardEntry>,
}

/// Manages Normal/Elite/Celebrity/TimeMachine campaign progression, `copy.attack`, `copy.pass`,
/// `copy.sweep`, `copy.fetchPrize`, `copy.fetchBoxReward`, `copy.resetAtk`, and `battle.getRecord`.
#[derive(Debug, Clone)]
pub struct CopyCampaignEngine {
    next_brid: u64,
    /// `(uid, copy_type) -> (chapter_id -> ChapterProgress)`
    progress: BTreeMap<(u64, u32), BTreeMap<u32, ChapterProgress>>,
    fresher_reward_index: BTreeMap<u64, u32>,
    famous_pass_times: BTreeMap<u64, u32>,
    pending_battles: BTreeMap<u64, PendingStageBattle>,
    battle_records: BTreeMap<u64, SimulatedBattle>,
}

impl Default for CopyCampaignEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyCampaignEngine {
    pub const EXEC_COST_PER_STAGE: u32 = 6;
    pub const ELITE_EXEC_COST_PER_STAGE: u32 = 12;
    pub const MAX_DAILY_ATTACKS: u32 = 10;
    pub const MAX_ELITE_DAILY_ATTACKS: u32 = 3;

    pub fn new() -> Self {
        Self {
            next_brid: 700_001,
            progress: BTreeMap::new(),
            fresher_reward_index: BTreeMap::new(),
            famous_pass_times: BTreeMap::new(),
            pending_battles: BTreeMap::new(),
            battle_records: BTreeMap::new(),
        }
    }

    pub fn alloc_brid(&mut self) -> u64 {
        let id = self.next_brid;
        self.next_brid += 1;
        id
    }

    pub fn record_battle(&mut self, sim: SimulatedBattle) {
        self.battle_records.insert(sim.brid, sim);
    }

    pub fn get_record(&self, brid: u64) -> Option<&SimulatedBattle> {
        self.battle_records.get(&brid)
    }

    pub fn get_record_blob(&self, brid: u64) -> Option<Vec<u8>> {
        self.battle_records
            .get(&brid)
            .map(SimulatedBattle::to_amf3_report_bytes)
    }

    pub fn get_zlib_record_blob(&self, brid: u64) -> Option<Vec<u8>> {
        self.battle_records
            .get(&brid)
            .map(SimulatedBattle::to_zlib_amf3_report_bytes)
    }

    fn copy_type_for_chapter(chapter_id: u32) -> u32 {
        if (2001..3000).contains(&chapter_id) {
            2
        } else if chapter_id >= 3000 {
            3
        } else {
            1
        }
    }

    fn max_daily_attacks_for_chapter(chapter_id: u32) -> u32 {
        if Self::copy_type_for_chapter(chapter_id) == 2 {
            Self::MAX_ELITE_DAILY_ATTACKS
        } else {
            Self::MAX_DAILY_ATTACKS
        }
    }

    fn exec_cost_for_chapter(chapter_id: u32) -> u32 {
        if Self::copy_type_for_chapter(chapter_id) == 2 {
            Self::ELITE_EXEC_COST_PER_STAGE
        } else {
            Self::EXEC_COST_PER_STAGE
        }
    }

    fn ensure_player_chapters(
        &mut self,
        uid: u64,
        copy_type: u32,
    ) -> &mut BTreeMap<u32, ChapterProgress> {
        let chapters = self.progress.entry((uid, copy_type)).or_default();
        match copy_type {
            1 => {
                chapters.entry(1001).or_default();
            }
            2 => {
                chapters.entry(2001).or_default();
            }
            _ => {}
        }
        chapters
    }

    /// Returns `copy.getList` (`args = [copyType]`) matching `CopyData:parseData` & `EliteCopyData:parseData`.
    pub fn get_list(&mut self, uid: u64, copy_type: u32) -> DtoValue {
        let fresher_idx = self.fresher_reward_index.get(&uid).copied().unwrap_or(0);
        let chapters = self.ensure_player_chapters(uid, copy_type);
        let mut copy_data = BTreeMap::new();
        for (&chap_id, chap) in chapters.iter() {
            let total_star: u32 = chap.stages.values().map(|s| s.stars).sum();
            let passed_count = chap.stages.values().filter(|s| s.stars > 0).count() as u32;
            let box_num = chap.stages.values().filter(|s| s.box_claimed).count() as u32;
            copy_data.insert(
                chap_id.to_string(),
                DtoValue::map([
                    ("star", DtoValue::from(total_star)),
                    ("score", DtoValue::from(total_star)),
                    ("prize", DtoValue::from(chap.prize_mask)),
                    ("prized_num", DtoValue::from(chap.prize_mask)),
                    ("pass_strongh", DtoValue::from(passed_count)),
                    ("box_reward_num", DtoValue::from(box_num)),
                ]),
            );
        }
        DtoValue::map([
            ("fresherRewardIndex", DtoValue::from(fresher_idx)),
            ("copyData", DtoValue::Map(copy_data)),
        ])
    }

    /// Returns `copy.getCopy` (`args = [chapterId]`) stage details matching `CopyInfoDataController:_initChapterData`.
    /// Emits accurate `va_copy` (`pass = attacks_today`, never fabricating fake 3-star clears on uncleared stages!)
    /// alongside `va_copy_info`, `score`, `prize`, and `prized_num`.
    pub fn get_chapter(&mut self, uid: u64, chapter_id: u32) -> DtoValue {
        let copy_type = Self::copy_type_for_chapter(chapter_id);
        let daily_cap = Self::max_daily_attacks_for_chapter(chapter_id);
        let chapters = self.ensure_player_chapters(uid, copy_type);
        let chap = chapters.entry(chapter_id).or_default();
        let total_score: u32 = chap.stages.values().map(|s| s.stars).sum();
        let mut strongholds = BTreeMap::new();
        let mut va_copy = BTreeMap::new();

        for (&copy_id, rec) in &chap.stages {
            let total_allowed = (rec.resets_today + 1) * daily_cap;
            let less_num = total_allowed.saturating_sub(rec.attacks_today);
            let star_status = if rec.stars >= 3 {
                7u32
            } else if rec.stars == 2 {
                3u32
            } else if rec.stars == 1 {
                1u32
            } else {
                0u32
            };
            strongholds.insert(
                copy_id.to_string(),
                DtoValue::map([
                    ("copy_id", DtoValue::from(copy_id)),
                    ("star", DtoValue::from(rec.stars)),
                    ("less_num", DtoValue::from(less_num)),
                    ("pass", DtoValue::from(rec.attacks_today)),
                    ("reset_num", DtoValue::from(rec.resets_today)),
                    (
                        "box_status",
                        DtoValue::from(if rec.box_claimed {
                            2u32
                        } else if rec.stars > 0 {
                            1u32
                        } else {
                            0u32
                        }),
                    ),
                ]),
            );
            va_copy.insert(
                copy_id.to_string(),
                DtoValue::map([
                    ("reset", DtoValue::from(rec.resets_today)),
                    ("normal_star", DtoValue::from(rec.stars)),
                    ("star_status", DtoValue::from(star_status)),
                    // `CopyInfoDataController`: `canDefeatNum = (reset + 1) * limit - pass`!
                    ("pass", DtoValue::from(rec.attacks_today)),
                    (
                        "box_reward",
                        DtoValue::from(if rec.box_claimed { 1u32 } else { 0u32 }),
                    ),
                ]),
            );
        }

        DtoValue::map([
            ("chapter_id", DtoValue::from(chapter_id)),
            ("score", DtoValue::from(total_score)),
            ("prize", DtoValue::from(chap.prize_mask)),
            ("prized_num", DtoValue::from(chap.prize_mask)),
            ("va_copy", DtoValue::Map(va_copy)),
            ("va_copy_info", DtoValue::Map(strongholds)),
        ])
    }

    fn validate_stage_in_chapter(chapter_id: u32, copy_id: u32, allow_tm: bool) -> bool {
        if chapter_id < 1000 || copy_id < 100_000 || copy_id / 100 != chapter_id {
            return false;
        }
        let valid_normal = chapter_stage_ids(chapter_id);
        if valid_normal.contains(&copy_id) {
            return true;
        }
        allow_tm && tm_chapter_stage_ids(chapter_id).contains(&copy_id)
    }

    /// Starts a PvE stage fight (`copy.attack`, `timeMachineCopy.attack`, or `user.beforLoginBattle`),
    /// returning crash-proof `serverData` (`brid`, `arrSeed`, `atkTurn`, `drop`, `battleInfo`, `userInfo` map).
    pub fn attack_stage(
        &mut self,
        player: &PlayerState,
        chapter_id: u32,
        copy_id: u32,
        is_tutorial_before_login: bool,
    ) -> Result<DtoValue, &'static str> {
        self.attack_stage_ext(player, chapter_id, copy_id, is_tutorial_before_login, false)
    }

    pub fn attack_tm_stage(
        &mut self,
        player: &PlayerState,
        chapter_id: u32,
        copy_id: u32,
    ) -> Result<DtoValue, &'static str> {
        self.attack_stage_ext(player, chapter_id, copy_id, false, true)
    }

    fn attack_stage_ext(
        &mut self,
        player: &PlayerState,
        chapter_id: u32,
        copy_id: u32,
        is_tutorial_before_login: bool,
        is_tm: bool,
    ) -> Result<DtoValue, &'static str> {
        if !Self::validate_stage_in_chapter(chapter_id, copy_id, is_tm) {
            return Err("invalid_copy_stage");
        }
        let copy_type = Self::copy_type_for_chapter(chapter_id);
        let exec_cost = Self::exec_cost_for_chapter(chapter_id);
        let daily_cap = Self::max_daily_attacks_for_chapter(chapter_id);

        if !is_tutorial_before_login {
            if player.execution < exec_cost {
                return Err("not_enough_execution");
            }
            let (attacks_today, resets_today) = self
                .progress
                .get(&(player.uid, copy_type))
                .and_then(|chaps| chaps.get(&chapter_id))
                .and_then(|chap| chap.stages.get(&copy_id))
                .map_or((0, 0), |rec| (rec.attacks_today, rec.resets_today));
            if attacks_today >= (resets_today + 1) * daily_cap {
                return Err("attack_limit_reached");
            }
        }

        let brid = self.alloc_brid();
        let stage_idx = (copy_id % 100).max(1);
        let base_chap = if copy_type == 2 { 2001 } else { 1001 };
        let monster_lv = if is_tutorial_before_login {
            1
        } else {
            ((chapter_id.saturating_sub(base_chap)) * 5 + stage_idx).max(1)
        };

        let team1 = extract_player_combat_units(player);
        let (m_htid1, m_htid2) = stage_monster_htids(copy_id);
        let team2 = vec![
            CombatUnit::monster(90_000_001, m_htid1, 0, monster_lv),
            CombatUnit::monster(90_000_002, m_htid2, 1, monster_lv),
        ];

        let drops = vec![
            RewardEntry::Copper(1_500 + i64::from(stage_idx) * 250),
            RewardEntry::HeroSoul(50 + u64::from(stage_idx) * 10),
            RewardEntry::Item { tid: 17002, qty: 1 },
        ];

        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(
            0,
            &format!("StageBoss_{copy_id}"),
            monster_lv,
            0,
            u64::from(monster_lv) * 1_500,
            0,
        );

        let sim = simulate_battle(brid, false, &team1, &team2, u1, u2, &drops);
        let mut attack_dto = sim.server_attack_dto.clone();
        if let DtoValue::Map(ref mut m) = attack_dto {
            m.insert("ret".to_owned(), DtoValue::from("ok"));
            m.insert("appraisal".to_owned(), DtoValue::from(sim.appraisal));
            m.insert(
                "fightRet".to_owned(),
                DtoValue::Bytes(sim.to_amf3_report_bytes()),
            );
        }
        self.battle_records.insert(brid, sim.clone());
        self.pending_battles.insert(
            player.uid,
            PendingStageBattle {
                brid,
                chapter_id,
                copy_id,
                sim,
                drops,
            },
        );
        Ok(attack_dto)
    }

    /// Settles a cleared stage (`copy.pass`, `args = [chapterId, copyId, fightInfo]`), deducting
    /// execution, awarding stars/exp/drops, unlocking `open_copy_id` ONLY when the chapter's real
    /// final stronghold is cleared (`100106`, `100206`, `100308`, `111130`, `XXXX10`), and emitting pushes.
    pub fn pass_stage(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.pass_stage_ext(player, chapter_id, copy_id, now_ts, false)
    }

    pub fn pass_tm_stage(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.pass_stage_ext(player, chapter_id, copy_id, now_ts, true)
    }

    fn pass_stage_ext(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
        is_tm: bool,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let pending = match self.pending_battles.remove(&player.uid) {
            Some(p) if p.chapter_id == chapter_id && p.copy_id == copy_id => p,
            _ => {
                let _ = self.attack_stage_ext(player, chapter_id, copy_id, false, is_tm)?;
                self.pending_battles
                    .remove(&player.uid)
                    .ok_or("no_active_battle")?
            }
        };

        if !pending.sim.is_win {
            return Ok((
                DtoValue::map([
                    ("battle_result", DtoValue::from(0u32)),
                    ("star", DtoValue::from(0u32)),
                    ("star_status", DtoValue::from(0u32)),
                    ("drop", build_copy_drop_dto(&[])),
                ]),
                Vec::new(),
            ));
        }

        let copy_type = Self::copy_type_for_chapter(chapter_id);
        let exec_cost = Self::exec_cost_for_chapter(chapter_id);
        player.execution = player.execution.saturating_sub(exec_cost);
        player.execution_time = now_ts;
        let exp_gain = u64::from(player.level.max(1)) * 60;
        player.exp += exp_gain;
        if player.exp >= u64::from(player.level.max(1)) * 200 {
            player.exp = 0;
            player.level = (player.level + 1).min(200);
            let main_hid = player.roster.formation[0];
            if let Some(mh) = player.roster.heroes.get_mut(&main_hid) {
                mh.level = player.level;
                mh.recalculate_stats();
            }
        }

        let new_stars = pending.sim.stars.max(1);
        let stage_list = if is_tm {
            tm_chapter_stage_ids(chapter_id)
        } else {
            chapter_stage_ids(chapter_id)
        };
        let is_final_stage = stage_list.last().copied() == Some(copy_id);

        let mut unlocked_chapters = Vec::new();
        let chap_score = {
            let chapters = self.ensure_player_chapters(player.uid, copy_type);
            let chap = chapters.entry(chapter_id).or_default();
            let rec = chap.stages.entry(copy_id).or_default();
            rec.stars = rec.stars.max(new_stars);
            rec.attacks_today += 1;
            let total_score: u32 = chap.stages.values().map(|s| s.stars).sum();

            if is_final_stage {
                if let Some(next_chap) = next_open_chapter_id(chapter_id) {
                    if !chapters.contains_key(&next_chap) {
                        chapters.entry(next_chap).or_default();
                        unlocked_chapters.push(next_chap);
                    }
                }
            }
            total_score
        };

        // If clearing Normal chapter `1001`..`1170` final stage and level >= 26, ensure matching Elite chapter can open
        if copy_type == 1 && is_final_stage && player.level >= 26 && (1001..=1170).contains(&chapter_id) {
            let elite_chap = chapter_id + 1000;
            let elite_chaps = self.ensure_player_chapters(player.uid, 2);
            if elite_chap == 2001
                || elite_chaps
                    .get(&(elite_chap - 1))
                    .and_then(|prev| {
                        chapter_stage_ids(elite_chap - 1)
                            .last()
                            .and_then(|last_s| prev.stages.get(last_s))
                    })
                    .is_some_and(|r| r.stars > 0)
            {
                elite_chaps.entry(elite_chap).or_default();
            }
        }

        let mut pushes = grant_rewards_to_player(player, &pending.drops, now_ts)?;
        pushes.insert(
            0,
            PlayerState::build_user_update_push(
                [
                    ("execution", DtoValue::from(player.execution)),
                    ("execution_time", DtoValue::from(player.execution_time)),
                    ("level", DtoValue::from(player.level)),
                    ("exp", DtoValue::from(player.exp)),
                    ("fight_force", DtoValue::from(player.fight_force())),
                ],
                now_ts,
            ),
        );

        let star_status = if new_stars >= 3 {
            7u32
        } else if new_stars == 2 {
            3u32
        } else {
            1u32
        };
        let drop_dto = build_copy_drop_dto(&pending.drops);
        let new_copy_dto = DtoValue::Array(unlocked_chapters.into_iter().map(DtoValue::from).collect());
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("battle_result", DtoValue::from(1u32)),
            ("brid", DtoValue::from(pending.brid)),
            ("star", DtoValue::from(new_stars)),
            ("score", DtoValue::from(chap_score)),
            ("star_status", DtoValue::from(star_status)),
            ("exp", DtoValue::from(exp_gain)),
            ("drop", drop_dto.clone()),
            ("reward", drop_dto),
            ("new_copy_or_base", new_copy_dto),
            ("star_fragment", DtoValue::Map(BTreeMap::new())),
        ]);
        Ok((ret, pushes))
    }

    pub fn battle_fail(&mut self, uid: u64, _chapter_id: u32, _copy_id: u32) -> DtoValue {
        self.pending_battles.remove(&uid);
        DtoValue::map([("ret", DtoValue::from("ok"))])
    }

    /// Sweeps a 3-star cleared stage (`copy.sweep` / `timeMachineCopy.sweep`, `args = [chapterId, copyId, num, isMultiple]`).
    /// Returns a **top-level 1-indexed `DtoValue::Array`** of per-sweep drop dictionaries so
    /// `CopySweepPanel.lua:79` (`if type(c) == "table" and c[1] then`) displays every sweep round!
    pub fn sweep_stage(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        sweep_num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let n = sweep_num.clamp(1, 10);
        let copy_type = Self::copy_type_for_chapter(chapter_id);
        let exec_cost = Self::exec_cost_for_chapter(chapter_id);
        let daily_cap = Self::max_daily_attacks_for_chapter(chapter_id);
        let needed_exec = exec_cost * n;
        if player.execution < needed_exec {
            return Err("not_enough_execution");
        }
        {
            let chapters = self.ensure_player_chapters(player.uid, copy_type);
            let rec = chapters
                .get(&chapter_id)
                .and_then(|c| c.stages.get(&copy_id))
                .ok_or("stage_not_cleared")?;
            if rec.stars < 3 {
                return Err("requires_3_stars");
            }
            if rec.attacks_today + n > (rec.resets_today + 1) * daily_cap {
                return Err("attack_limit_reached");
            }
        }

        player.execution -= needed_exec;
        player.execution_time = now_ts;
        let exp_per_round = u64::from(player.level.max(1)) * 60;
        player.exp += exp_per_round * u64::from(n);
        {
            let chapters = self.ensure_player_chapters(player.uid, copy_type);
            if let Some(rec) = chapters
                .entry(chapter_id)
                .or_default()
                .stages
                .get_mut(&copy_id)
            {
                rec.attacks_today += n;
            }
        }

        let mut all_rewards = Vec::new();
        let mut sweep_rounds = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let round_drop = [
                RewardEntry::Copper(1_800),
                RewardEntry::HeroSoul(50),
                RewardEntry::Item { tid: 17002, qty: 1 },
            ];
            all_rewards.extend_from_slice(&round_drop);
            let mut drop_map = match build_copy_drop_dto(&round_drop) {
                DtoValue::Map(m) => m,
                _ => BTreeMap::new(),
            };
            drop_map.insert("exp".to_owned(), DtoValue::from(exp_per_round));
            drop_map.insert("extra".to_owned(), DtoValue::Map(BTreeMap::new()));
            sweep_rounds.push(DtoValue::Map(drop_map));
        }

        let mut pushes = grant_rewards_to_player(player, &all_rewards, now_ts)?;
        pushes.insert(
            0,
            PlayerState::build_user_update_push(
                [
                    ("execution", DtoValue::from(player.execution)),
                    ("execution_time", DtoValue::from(player.execution_time)),
                    ("exp", DtoValue::from(player.exp)),
                ],
                now_ts,
            ),
        );

        Ok((DtoValue::Array(sweep_rounds), pushes))
    }

    /// Resets daily attack count on a stage (`copy.resetAtk` / `copy.resetCopyFightNum`, `args = [chapterId, copyId]`).
    pub fn reset_attack_count(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let copy_type = Self::copy_type_for_chapter(chapter_id);
        let cur_resets = self
            .progress
            .get(&(player.uid, copy_type))
            .and_then(|chaps| chaps.get(&chapter_id))
            .and_then(|c| c.stages.get(&copy_id))
            .map_or(0, |r| r.resets_today);
        let cost = match cur_resets {
            0 => 20i64,
            1 => 50i64,
            _ => 100i64,
        };
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let chapters = self.ensure_player_chapters(player.uid, copy_type);
        let rec = chapters
            .entry(chapter_id)
            .or_default()
            .stages
            .entry(copy_id)
            .or_default();
        rec.attacks_today = 0;
        rec.resets_today += 1;
        let resets_today = rec.resets_today;

        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("reset_num", DtoValue::from(resets_today)),
            ]),
            pushes,
        ))
    }

    pub fn reset_atk_num(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.reset_attack_count(player, chapter_id, copy_id, now_ts)
    }

    /// Claims a chapter star chest (`copy.fetchPrize`, `args = [chapterId, boxIdx0Based]`)
    /// using authentic `score_require` thresholds from `copy/copy.json` (`[5,10,15]`, `[6,12,18]`, `[8,16,24]`, `[10,20,30]`, `[30,60,90]`).
    pub fn fetch_chapter_prize(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        box_idx_zero_based: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let thresholds = chapter_star_thresholds(chapter_id);
        let Some(&req_stars) = thresholds.get(box_idx_zero_based as usize) else {
            return Err("invalid_box_idx");
        };
        let bit = 1u32 << box_idx_zero_based;
        let copy_type = Self::copy_type_for_chapter(chapter_id);

        let cur_stars = {
            let chapters = self.ensure_player_chapters(player.uid, copy_type);
            let chap = chapters.entry(chapter_id).or_default();
            let cur_stars: u32 = chap.stages.values().map(|s| s.stars).sum();
            if cur_stars < req_stars {
                return Err("stars_not_enough");
            }
            if (chap.prize_mask & bit) != 0 {
                return Err("prize_already_claimed");
            }
            chap.prize_mask |= bit;
            cur_stars
        };

        let reward = [
            RewardEntry::Gold(100 * i64::from(box_idx_zero_based + 1)),
            RewardEntry::Item { tid: 16012, qty: 1 },
        ];
        let pushes = grant_rewards_to_player(player, &reward, now_ts)?;
        let prize_mask = self
            .ensure_player_chapters(player.uid, copy_type)
            .get(&chapter_id)
            .map_or(bit, |c| c.prize_mask);
        let reward_dto = DtoValue::Array(reward.iter().copied().map(RewardEntry::to_triplet_dto).collect());
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("prize", DtoValue::from(prize_mask)),
                ("prized_num", DtoValue::from(prize_mask)),
                ("score", DtoValue::from(cur_stars)),
                ("reward", reward_dto),
            ]),
            pushes,
        ))
    }

    pub fn get_prize(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        box_idx_zero_based: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.fetch_chapter_prize(player, chapter_id, box_idx_zero_based, now_ts)
    }

    /// Claims a stage point chest (`copy.fetchBoxReward`, `args = [chapterId, copyId]` -> `"ok"`).
    pub fn fetch_stage_box_reward(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let copy_type = Self::copy_type_for_chapter(chapter_id);
        {
            let chapters = self.ensure_player_chapters(player.uid, copy_type);
            let rec = chapters
                .entry(chapter_id)
                .or_default()
                .stages
                .get_mut(&copy_id)
                .ok_or("stage_not_cleared")?;
            if rec.stars == 0 {
                return Err("stage_not_cleared");
            }
            if rec.box_claimed {
                return Err("box_already_claimed");
            }
            rec.box_claimed = true;
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(50), RewardEntry::Copper(10_000)],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Claims starter progression reward (`copy.fetchFresherReward` / `timeMachineCopy.fetchFresherReward`).
    pub fn fetch_fresher_reward(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let cur = self.fresher_reward_index.get(&player.uid).copied().unwrap_or(0);
        if cur >= 10 {
            return Err("already_claimed");
        }
        let next = cur + 1;
        self.fresher_reward_index.insert(player.uid, next);
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(100), RewardEntry::Item { tid: 22102, qty: 1 }],
            now_ts,
        )?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("fresherRewardIndex", DtoValue::from(next)),
            ]),
            pushes,
        ))
    }

    /// Returns celebrity copy pass counts (`copy.getFamousPassTimes`).
    pub fn get_famous_pass_times(&self, uid: u64) -> DtoValue {
        let passed = self.famous_pass_times.get(&uid).copied().unwrap_or(0);
        DtoValue::map([("pass_times", DtoValue::from(passed))])
    }

    /// Exports durable per-player campaign progress for WAL + `PlayerProfile` snapshot persistence.
    pub fn export_player_progress(&self, uid: u64) -> DtoValue {
        let mut by_type = BTreeMap::new();
        for (&(p_uid, copy_type), chapters) in &self.progress {
            if p_uid != uid {
                continue;
            }
            let mut chap_map = BTreeMap::new();
            let default_chap = if copy_type == 2 { 2001 } else { 1001 };
            for (&chap_id, chap) in chapters {
                let mut stages_map = BTreeMap::new();
                for (&copy_id, rec) in &chap.stages {
                    stages_map.insert(
                        copy_id.to_string(),
                        DtoValue::map([
                            ("stars", DtoValue::from(rec.stars)),
                            ("attacks_today", DtoValue::from(rec.attacks_today)),
                            ("resets_today", DtoValue::from(rec.resets_today)),
                            ("box_claimed", DtoValue::from(rec.box_claimed)),
                        ]),
                    );
                }
                if !stages_map.is_empty() || chap.prize_mask != 0 || chap_id != default_chap {
                    chap_map.insert(
                        chap_id.to_string(),
                        DtoValue::map([
                            ("prize_mask", DtoValue::from(chap.prize_mask)),
                            ("stages", DtoValue::Map(stages_map)),
                        ]),
                    );
                }
            }
            if !chap_map.is_empty() {
                by_type.insert(copy_type.to_string(), DtoValue::Map(chap_map));
            }
        }
        DtoValue::Map(by_type)
    }

    /// Hydrates durable per-player campaign progress from `PlayerProfile` on startup/login.
    pub fn import_player_progress(&mut self, uid: u64, data: &DtoValue) {
        let Some(by_type) = data.as_map() else {
            return;
        };
        for (type_k, chaps_val) in by_type {
            let Ok(copy_type) = type_k.parse::<u32>() else {
                continue;
            };
            let Some(chaps_map) = chaps_val.as_map() else {
                continue;
            };
            let dst_chaps = self.progress.entry((uid, copy_type)).or_default();
            if copy_type == 1 {
                dst_chaps.entry(1001).or_default();
            } else if copy_type == 2 {
                dst_chaps.entry(2001).or_default();
            }
            for (chap_k, chap_val) in chaps_map {
                let Ok(chap_id) = chap_k.parse::<u32>() else {
                    continue;
                };
                let dst_chap = dst_chaps.entry(chap_id).or_default();
                if let Some(pm) = chap_val.get("prize_mask").and_then(DtoValue::as_u32) {
                    dst_chap.prize_mask = pm;
                }
                if let Some(stages_map) = chap_val.get("stages").and_then(DtoValue::as_map) {
                    for (stage_k, rec_val) in stages_map {
                        let Ok(copy_id) = stage_k.parse::<u32>() else {
                            continue;
                        };
                        let rec = dst_chap.stages.entry(copy_id).or_default();
                        rec.stars = rec_val.get("stars").and_then(DtoValue::as_u32).unwrap_or(0);
                        rec.attacks_today = rec_val
                            .get("attacks_today")
                            .and_then(DtoValue::as_u32)
                            .unwrap_or(0);
                        rec.resets_today = rec_val
                            .get("resets_today")
                            .and_then(DtoValue::as_u32)
                            .unwrap_or(0);
                        rec.box_claimed = rec_val
                            .get("box_claimed")
                            .and_then(DtoValue::as_bool)
                            .unwrap_or(false);
                    }
                }
            }
        }
    }
}

/// Dedicated Elite Copy facade (`EliteCopyData.lua` / `ecopy.*` / `copy_elite.*` / `copy.getList({2})`).
#[derive(Debug, Clone, Default)]
pub struct EliteCopyEngine;

impl EliteCopyEngine {
    pub fn get_info(copy_engine: &mut CopyCampaignEngine, uid: u64) -> DtoValue {
        copy_engine.get_list(uid, 2)
    }

    pub fn get_elite_copy_info(copy_engine: &mut CopyCampaignEngine, uid: u64) -> DtoValue {
        copy_engine.get_list(uid, 2)
    }

    pub fn get_chapter(copy_engine: &mut CopyCampaignEngine, uid: u64, chapter_id: u32) -> DtoValue {
        copy_engine.get_chapter(uid, chapter_id.max(2001))
    }

    pub fn attack(
        copy_engine: &mut CopyCampaignEngine,
        player: &PlayerState,
        chapter_id: u32,
        copy_id: u32,
    ) -> Result<DtoValue, &'static str> {
        if !(2001..=2170).contains(&chapter_id) {
            return Err("invalid_elite_chapter");
        }
        copy_engine.attack_stage(player, chapter_id, copy_id, false)
    }

    pub fn pass(
        copy_engine: &mut CopyCampaignEngine,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(2001..=2170).contains(&chapter_id) {
            return Err("invalid_elite_chapter");
        }
        copy_engine.pass_stage(player, chapter_id, copy_id, now_ts)
    }

    pub fn sweep(
        copy_engine: &mut CopyCampaignEngine,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        sweep_num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(2001..=2170).contains(&chapter_id) {
            return Err("invalid_elite_chapter");
        }
        copy_engine.sweep_stage(player, chapter_id, copy_id, sweep_num, now_ts)
    }

    pub fn reset(
        copy_engine: &mut CopyCampaignEngine,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(2001..=2170).contains(&chapter_id) {
            return Err("invalid_elite_chapter");
        }
        copy_engine.reset_attack_count(player, chapter_id, copy_id, now_ts)
    }
}

// ============================================================================
// Section 2.2: Daily Copy (`copy_daily.*` / `tmCopyDaily.*`)
// ============================================================================

#[derive(Debug, Clone)]
pub struct DailyCopySlot {
    pub last_num: u32,
    pub buy_num: u32,
    pub passed_points: BTreeSet<u32>,
}

impl Default for DailyCopySlot {
    fn default() -> Self {
        Self {
            last_num: 2,
            buy_num: 0,
            passed_points: BTreeSet::new(),
        }
    }
}

/// Daily Resource Copy Engine (`DailyCopyData.lua`, `DailyCopyProxy.lua`, `daily_copy.json` IDs `1..=9`,
/// `daily_strongh.json` IDs `1001..=9020` and `strongh.json` `101..=922`).
#[derive(Debug, Clone, Default)]
pub struct DailyCopyEngine {
    /// `(uid, daily_copy_id) -> DailyCopySlot`
    slots: BTreeMap<(u64, u32), DailyCopySlot>,
    pending: BTreeMap<u64, (u32, u32, u64)>,
}

impl DailyCopyEngine {
    pub const DEFAULT_CHALLENGE_TIMES: u32 = 2;

    fn is_valid_point(copy_type_id: u32, point_id: u32) -> bool {
        if !(1..=9).contains(&copy_type_id) {
            return false;
        }
        let range_1000 = (copy_type_id * 1000 + 1)..=(copy_type_id * 1000 + 22);
        let range_100 = (copy_type_id * 100 + 1)..=(copy_type_id * 100 + 22);
        range_1000.contains(&point_id) || range_100.contains(&point_id)
    }

    fn point_rewards(copy_type_id: u32, point_id: u32) -> Vec<RewardEntry> {
        let tier = (point_id % 100).clamp(1, 22);
        match copy_type_id {
            1 => vec![RewardEntry::Copper(40_000 + i64::from(tier) * 20_000)],
            2 => vec![RewardEntry::Item {
                tid: 17002,
                qty: 2 + tier,
            }],
            3 => vec![RewardEntry::Item {
                tid: 181_001,
                qty: 1 + tier / 2,
            }],
            4 => vec![RewardEntry::Item {
                tid: 10010,
                qty: 50 + tier * 20,
            }],
            _ => vec![
                RewardEntry::Copper(30_000 + i64::from(tier) * 10_000),
                RewardEntry::HeroSoul(100 * u64::from(tier)),
            ],
        }
    }

    /// Handles `copy_daily.getInfo` / `tmCopyDaily.getInfo` (`args = [copyId]` or `[]`).
    /// Guarantees `ret.pass` is a non-nil `DtoValue::Array` so `ipairs(ret.pass)` in
    /// `DailyCopyData:_parseInfo` never crashes!
    pub fn get_info(&mut self, uid: u64, copy_type_id: Option<u32>) -> DtoValue {
        if let Some(cid) = copy_type_id.filter(|&c| (1..=9).contains(&c)) {
            let slot = self.slots.entry((uid, cid)).or_default();
            let pass_arr: Vec<DtoValue> =
                slot.passed_points.iter().copied().map(DtoValue::from).collect();
            return DtoValue::map([
                ("id", DtoValue::from(cid)),
                ("last_num", DtoValue::from(slot.last_num)),
                ("buy_num", DtoValue::from(slot.buy_num)),
                ("pass", DtoValue::Array(pass_arr)),
            ]);
        }

        let mut copy_map = BTreeMap::new();
        let mut all_passed = Vec::new();
        for cid in 1..=9u32 {
            let slot = self.slots.entry((uid, cid)).or_default();
            let max_pass = slot.passed_points.iter().copied().max().unwrap_or(0);
            for &pid in &slot.passed_points {
                all_passed.push(DtoValue::from(pid));
            }
            copy_map.insert(
                cid.to_string(),
                DtoValue::map([
                    ("atk_num", DtoValue::from(slot.last_num)),
                    ("last_num", DtoValue::from(slot.last_num)),
                    ("buy_num", DtoValue::from(slot.buy_num)),
                    ("max_pass", DtoValue::from(max_pass)),
                ]),
            );
        }
        let first_last = self.slots.get(&(uid, 1)).map_or(2, |s| s.last_num);
        DtoValue::map([
            ("last_num", DtoValue::from(first_last)),
            ("pass", DtoValue::Array(all_passed)),
            ("copy", DtoValue::Map(copy_map)),
        ])
    }

    /// Starts a Daily Copy fight (`copy_daily.attack`, `args = [copyId, pointId]`), returning Phase 1 `serverData`.
    pub fn attack(
        &mut self,
        player: &PlayerState,
        copy_type_id: u32,
        point_id: u32,
        copy_engine: &mut CopyCampaignEngine,
    ) -> Result<DtoValue, &'static str> {
        if !Self::is_valid_point(copy_type_id, point_id) {
            return Err("invalid_daily_copy_point");
        }
        let slot = self.slots.entry((player.uid, copy_type_id)).or_default();
        if slot.last_num == 0 && slot.passed_points.contains(&point_id) {
            return Err("daily_copy_no_attempts");
        }

        let brid = copy_engine.alloc_brid();
        let tier = (point_id % 100).max(1);
        let team1 = extract_player_combat_units(player);
        let (m1, m2) = stage_monster_htids(point_id);
        let team2 = vec![
            CombatUnit::monster(91_000_001, m1, 0, tier * 3),
            CombatUnit::monster(91_000_002, m2, 1, tier * 3),
        ];
        let drops = Self::point_rewards(copy_type_id, point_id);
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(
            0,
            &format!("DailyBoss_{point_id}"),
            tier * 3,
            0,
            u64::from(tier) * 2_000,
            0,
        );
        let sim = simulate_battle(brid, false, &team1, &team2, u1, u2, &drops);
        let server_dto = sim.server_attack_dto.clone();
        copy_engine.record_battle(sim);
        self.pending
            .insert(player.uid, (copy_type_id, point_id, brid));
        Ok(server_dto)
    }

    /// Settles a Daily Copy fight (`copy_daily.pass`, `args = [copyId, pointId, fightReport]`),
    /// matching `DailyCopyProxy:_fightSuccess` (`reward` bitmask + `is_pass = 1`).
    pub fn pass(
        &mut self,
        player: &mut PlayerState,
        copy_type_id: u32,
        point_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !Self::is_valid_point(copy_type_id, point_id) {
            return Err("invalid_daily_copy_point");
        }
        self.pending.remove(&player.uid);
        let slot = self.slots.entry((player.uid, copy_type_id)).or_default();
        let was_already_passed = !slot.passed_points.insert(point_id);
        if was_already_passed {
            if slot.last_num == 0 {
                return Err("daily_copy_no_attempts");
            }
            slot.last_num -= 1;
        }
        let rewards = Self::point_rewards(copy_type_id, point_id);
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("is_pass", DtoValue::from(1u32)),
            // Bit 0 set (`1` = reward1 tier unlocked; client BitArray reads bits 1..N)
            ("reward", DtoValue::from(1u32)),
            ("last_num", DtoValue::from(slot.last_num)),
            ("drop", build_copy_drop_dto(&rewards)),
        ]);
        Ok((ret, pushes))
    }

    /// Sweeps a cleared Daily Copy tier (`copy_daily.sweep`, `args = [copyId, pointId]`).
    pub fn sweep(
        &mut self,
        player: &mut PlayerState,
        copy_type_id: u32,
        point_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !Self::is_valid_point(copy_type_id, point_id) {
            return Err("invalid_daily_copy_point");
        }
        let slot = self.slots.entry((player.uid, copy_type_id)).or_default();
        if !slot.passed_points.contains(&point_id) {
            return Err("point_not_passed");
        }
        if slot.last_num == 0 {
            return Err("daily_copy_no_attempts");
        }
        slot.last_num -= 1;
        let rewards = Self::point_rewards(copy_type_id, point_id);
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("last_num", DtoValue::from(slot.last_num)),
            ("drop", build_copy_drop_dto(&rewards)),
        ]);
        Ok((ret, pushes))
    }

    /// Buys extra Daily Copy attempts (`copy_daily.addNum` / `timeMachineCopy_daily.addNum`).
    pub fn buy_atk_num(
        &mut self,
        player: &mut PlayerState,
        copy_type_id: u32,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=9).contains(&copy_type_id) || count == 0 {
            return Err("invalid_daily_copy_buy");
        }
        let cost = i64::from(count) * 20;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let slot = self.slots.entry((player.uid, copy_type_id)).or_default();
        slot.buy_num += count;
        slot.last_num += count;
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("last_num", DtoValue::from(slot.last_num)),
                ("buy_num", DtoValue::from(slot.buy_num)),
            ]),
            pushes,
        ))
    }
}

// ============================================================================
// Section 2.3: Babel Tower (`babeltower.*` / `tmBabeltower.*`)
// ============================================================================

#[derive(Debug, Clone)]
pub struct BabelPlayerState {
    pub floor: u32,
    pub last_max_floor: u32,
    pub attack_num: u32,
    pub buy_attack_num: u32,
    pub unused_buy_num: u32,
    pub score: u32,
    pub pass_tag: u32,
    pub idx: u32,
    pub is_sweep: u32,
    pub is_over: u32,
    pub passed_idx: BTreeSet<u32>,
    pub attr_buffs: BTreeMap<u32, u32>,
    pub claimed_floor_rewards: BTreeSet<u32>,
    pub claimed_score_rewards: BTreeSet<u32>,
    pub bag_items: BTreeMap<u32, u32>,
}

impl Default for BabelPlayerState {
    fn default() -> Self {
        Self {
            floor: 1,
            last_max_floor: 0,
            attack_num: 0,
            buy_attack_num: 0,
            unused_buy_num: 0,
            score: 0,
            pass_tag: 0,
            idx: 0,
            is_sweep: 0,
            is_over: 0,
            passed_idx: BTreeSet::new(),
            attr_buffs: BTreeMap::new(),
            claimed_floor_rewards: BTreeSet::new(),
            claimed_score_rewards: BTreeSet::new(),
            bag_items: BTreeMap::new(),
        }
    }
}

/// Babel Tower Engine (`BabelTowerData.lua`, `BabelTowerProxy.lua`, `BabelTowerNpcItem.lua`, `babelTower_*.json`).
#[derive(Debug, Clone, Default)]
pub struct BabelTowerEngine {
    players: BTreeMap<u64, BabelPlayerState>,
}

impl BabelTowerEngine {
    pub const MAX_FLOOR: u32 = 100;
    pub const SCORE_PER_FLOOR: u32 = 100;

    fn build_floor_info_dto(floor: u32) -> DtoValue {
        let mut npc_info = BTreeMap::new();
        // NPC 1: Type 1 (PvP fight, Trương Liêu npc=1)
        npc_info.insert(
            "1".to_owned(),
            DtoValue::map([
                ("type", DtoValue::from(1u32)),
                ("npc", DtoValue::from(1u32)),
                (
                    "extra",
                    DtoValue::map([
                        ("buff", DtoValue::array([DtoValue::from(1001u32)])),
                        (
                            "enemy",
                            DtoValue::map([
                                ("uid", DtoValue::from(900_100u64 + u64::from(floor))),
                                ("uname", DtoValue::from("Trương Liêu")),
                                ("utid", DtoValue::from(103u32)),
                                ("quality", DtoValue::from(5u32)),
                                ("fight_force", DtoValue::from(5_000u64 * u64::from(floor.max(1)))),
                                ("dress_id", DtoValue::from(0u32)),
                                ("formation", DtoValue::Array(Vec::new())),
                            ]),
                        ),
                    ]),
                ),
            ]),
        );
        // NPC 2: Type 2 (PvE fight, Triệu Vân npc=2)
        npc_info.insert(
            "2".to_owned(),
            DtoValue::map([
                ("type", DtoValue::from(2u32)),
                ("npc", DtoValue::from(2u32)),
                ("extra", DtoValue::Map(BTreeMap::new())),
            ]),
        );
        // NPC 3: Type 4 (Answer Quiz, npc=1011, question id=1 where right=1)
        npc_info.insert(
            "3".to_owned(),
            DtoValue::map([
                ("type", DtoValue::from(4u32)),
                ("npc", DtoValue::from(1011u32)),
                ("extra", DtoValue::from(1u32)),
            ]),
        );
        // NPC 4: Type 5 (Direct Pass Chest, npc=1013)
        npc_info.insert(
            "4".to_owned(),
            DtoValue::map([
                ("type", DtoValue::from(5u32)),
                ("npc", DtoValue::from(1013u32)),
                ("extra", DtoValue::Map(BTreeMap::new())),
            ]),
        );
        // NPC 5: Type 8 (Buff Shrine, npc=1021, buff_id=1001)
        npc_info.insert(
            "5".to_owned(),
            DtoValue::map([
                ("type", DtoValue::from(8u32)),
                ("npc", DtoValue::from(1021u32)),
                ("extra", DtoValue::from(1001u32)),
            ]),
        );

        DtoValue::map([
            ("addBuff", DtoValue::from(1001u32)),
            ("npcInfo", DtoValue::Map(npc_info)),
        ])
    }

    /// Returns `babeltower.getInfo` / `babeltower.getBabeltowerInfo` matching `BabelTowerData:parseData` verbatim!
    pub fn get_babeltower_info(&mut self, uid: u64, now_ts: u64) -> DtoValue {
        let st = self.players.entry(uid).or_default();
        let mut score_reward_map = BTreeMap::new();
        for &sc in &st.claimed_score_rewards {
            score_reward_map.insert(sc.to_string(), DtoValue::from(1u32));
        }
        let mut floor_reward_map = BTreeMap::new();
        for &fl in &st.claimed_floor_rewards {
            floor_reward_map.insert(fl.to_string(), DtoValue::from(1u32));
        }
        let attr_arr: Vec<DtoValue> = st
            .attr_buffs
            .iter()
            .map(|(&bid, &cnt)| DtoValue::map([(bid.to_string(), DtoValue::from(cnt))]))
            .collect();
        let passed_arr: Vec<DtoValue> = st.passed_idx.iter().copied().map(DtoValue::from).collect();

        DtoValue::map([
            ("end_time", DtoValue::from(now_ts + 7 * 86_400)),
            ("floor", DtoValue::from(st.floor)),
            ("attack_num", DtoValue::from(st.attack_num)),
            ("buy_attack_num", DtoValue::from(st.buy_attack_num)),
            ("unused_buy_num", DtoValue::from(st.unused_buy_num)),
            ("help_num", DtoValue::from(0u32)),
            ("score", DtoValue::from(st.score)),
            ("pass_tag", DtoValue::from(st.pass_tag)),
            ("idx", DtoValue::from(st.idx)),
            ("last_max_floor", DtoValue::from(st.last_max_floor)),
            ("is_sweep", DtoValue::from(st.is_sweep)),
            ("is_over", DtoValue::from(st.is_over)),
            ("va_score_reward", DtoValue::Map(score_reward_map)),
            ("va_floor_reward", DtoValue::Map(floor_reward_map)),
            (
                "va_buff",
                DtoValue::map([
                    ("gift", DtoValue::Map(BTreeMap::new())),
                    ("attr", DtoValue::Array(attr_arr)),
                ]),
            ),
            ("va_passed_idx", DtoValue::Array(passed_arr)),
            ("floorInfo", Self::build_floor_info_dto(st.floor)),
        ])
    }

    /// Returns `babeltowerBag.getInfo` (`BabelTowerBagData:parseData`).
    pub fn get_bag_info(&mut self, uid: u64) -> DtoValue {
        let st = self.players.entry(uid).or_default();
        let mut map = BTreeMap::new();
        for (&tid, &cnt) in &st.bag_items {
            if cnt > 0 {
                map.insert(tid.to_string(), DtoValue::from(cnt));
            }
        }
        DtoValue::Map(map)
    }

    /// Selects an NPC on the current floor (`babeltower.setIdx`, `args = [floor, idx]`).
    pub fn set_idx(&mut self, uid: u64, floor: u32, idx: u32) -> Result<DtoValue, &'static str> {
        if !(1..=5).contains(&idx) {
            return Err("invalid_npc_idx");
        }
        let st = self.players.entry(uid).or_default();
        if floor != 0 && floor != st.floor {
            return Err("floor_mismatch");
        }
        st.idx = idx;
        Ok(DtoValue::from("ok"))
    }

    /// Enters/attacks a Babel Tower NPC (`babeltower.enterLevel` / `babeltower.attack`, `args = [floor, idx]`),
    /// returning Type B `record` + `result = "success"` + `reward`.
    pub fn attack(
        &mut self,
        player: &mut PlayerState,
        floor: u32,
        idx: u32,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=5).contains(&idx) {
            return Err("invalid_npc_idx");
        }
        let cur_floor = {
            let st = self.players.entry(player.uid).or_default();
            if floor != 0 && floor != st.floor {
                return Err("floor_mismatch");
            }
            st.floor
        };

        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let npc_htid = if idx == 1 { 103 } else { 201 };
        let t2 = vec![CombatUnit::monster(
            92_000_000 + u64::from(cur_floor) * 10 + u64::from(idx),
            npc_htid,
            0,
            cur_floor * 2,
        )];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(
            900_100 + u64::from(cur_floor),
            "Thủ Vệ Thông Thiên",
            cur_floor * 2,
            0,
            u64::from(cur_floor) * 3_500,
            0,
        );
        let sim = simulate_battle(brid, idx == 1, &t1, &t2, u1, u2, &[]);
        let report_blob = sim.to_amf3_report_bytes();
        let is_win = sim.is_win;
        copy_engine.record_battle(sim);

        let st = self.players.entry(player.uid).or_default();
        let mut pushes = Vec::new();
        let result_str = if is_win {
            st.passed_idx.insert(idx);
            st.pass_tag = 1;
            st.last_max_floor = st.last_max_floor.max(cur_floor);
            *st.bag_items.entry(1001).or_insert(0) += 1;
            pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(10_000)], now_ts)?;
            "success"
        } else {
            st.attack_num += 1;
            st.idx = 0;
            st.attr_buffs.clear();
            "fail"
        };

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("result", DtoValue::from(result_str)),
            ("brid", DtoValue::from(brid)),
            ("record", DtoValue::Bytes(report_blob.clone())),
            ("fightRet", DtoValue::Bytes(report_blob)),
            (
                "reward",
                DtoValue::map([("copper", DtoValue::from(if is_win { 10_000i64 } else { 0 }))]),
            ),
        ]);
        Ok((ret, pushes))
    }

    pub fn enter_level(
        &mut self,
        player: &mut PlayerState,
        floor: u32,
        idx: u32,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.attack(player, floor, idx, copy_engine, now_ts)
    }

    /// Advances to the next floor (`babeltower.passFloor`, `args = [floor]`) after `pass_tag == 1`.
    pub fn pass_floor(&mut self, uid: u64, floor: u32) -> Result<DtoValue, &'static str> {
        let st = self.players.entry(uid).or_default();
        if floor != 0 && floor != st.floor {
            return Err("floor_mismatch");
        }
        if st.pass_tag == 0 {
            return Err("floor_not_passed");
        }
        st.score += Self::SCORE_PER_FLOOR;
        st.last_max_floor = st.last_max_floor.max(st.floor);
        *st.attr_buffs.entry(1001).or_insert(0) += 1;
        st.idx = 0;
        st.passed_idx.clear();

        if st.floor >= Self::MAX_FLOOR {
            st.is_over = 1;
            return Ok(DtoValue::Map(BTreeMap::new()));
        }
        st.floor += 1;
        st.pass_tag = 0;
        Ok(Self::build_floor_info_dto(st.floor))
    }

    /// Answers a quiz NPC (`babeltower.answer`, `args = [floor, idx, answerOption]`).
    pub fn answer(
        &mut self,
        player: &mut PlayerState,
        floor: u32,
        idx: u32,
        choice: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.players.entry(player.uid).or_default();
        if floor != 0 && floor != st.floor {
            return Err("floor_mismatch");
        }
        // Question id=1 ("Quan Vũ là người nơi nào?") has `right = 1` in `babelTower_answer.json`
        if choice == 1 {
            st.passed_idx.insert(idx);
            st.pass_tag = 1;
            st.last_max_floor = st.last_max_floor.max(st.floor);
            let pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(8_000)], now_ts)?;
            Ok((
                DtoValue::map([
                    ("result", DtoValue::from("success")),
                    ("reward", DtoValue::map([("copper", DtoValue::from(8_000i64))])),
                ]),
                pushes,
            ))
        } else {
            st.attack_num += 1;
            st.idx = 0;
            st.attr_buffs.clear();
            Ok((
                DtoValue::map([
                    ("result", DtoValue::from("fail")),
                    ("reward", DtoValue::Map(BTreeMap::new())),
                ]),
                Vec::new(),
            ))
        }
    }

    /// Resolves non-combat chest/shrine NPCs (`babeltower.specialEvent`, `args = [floor, idx]`).
    pub fn special_event(
        &mut self,
        player: &mut PlayerState,
        floor: u32,
        idx: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=5).contains(&idx) {
            return Err("invalid_npc_idx");
        }
        let st = self.players.entry(player.uid).or_default();
        if floor != 0 && floor != st.floor {
            return Err("floor_mismatch");
        }
        st.passed_idx.insert(idx);
        let mut pushes = Vec::new();
        if idx == 4 {
            // NPC 4 is Type 5 (Direct Pass Chest)
            st.pass_tag = 1;
            st.last_max_floor = st.last_max_floor.max(st.floor);
            pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(12_000)], now_ts)?;
        } else {
            // Buff Shrine (Type 8)
            *st.attr_buffs.entry(1001).or_insert(0) += 1;
            st.idx = 0;
        }
        Ok((
            DtoValue::map([
                ("result", DtoValue::from("success")),
                ("reward", DtoValue::map([("copper", DtoValue::from(12_000i64))])),
            ]),
            pushes,
        ))
    }

    /// Sweeps Babel Tower up to `last_max_floor` (`babeltower.sweep`).
    pub fn sweep(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.players.entry(player.uid).or_default();
        let target_floor = st.last_max_floor.max(1);
        st.floor = target_floor;
        st.pass_tag = 1;
        st.is_sweep = 1;
        let gained_score = target_floor * Self::SCORE_PER_FLOOR;
        st.score = st.score.max(gained_score);
        let copper_gain = i64::from(target_floor) * 10_000;
        *st.bag_items.entry(1001).or_insert(0) += target_floor;

        let pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(copper_gain)], now_ts)?;
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            (
                "babel_tower",
                DtoValue::map([("1001", DtoValue::from(target_floor))]),
            ),
            ("copper", DtoValue::from(copper_gain)),
            ("score", DtoValue::from(st.score)),
        ]);
        Ok((ret, pushes))
    }

    /// Resets current Babel Tower run (`babeltower.reset`).
    pub fn reset(&mut self, uid: u64, now_ts: u64) -> DtoValue {
        let st = self.players.entry(uid).or_default();
        st.floor = 1;
        st.pass_tag = 0;
        st.idx = 0;
        st.is_over = 0;
        st.passed_idx.clear();
        st.attr_buffs.clear();
        self.get_babeltower_info(uid, now_ts)
    }

    /// Buys extra Babel Tower challenge attempts (`babeltower.buyAtkNum`, `args = [count]` -> `"ok"`).
    pub fn buy_atk_num(
        &mut self,
        player: &mut PlayerState,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count == 0 {
            return Err("invalid_count");
        }
        let cost = i64::from(count) * 50;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let st = self.players.entry(player.uid).or_default();
        st.buy_attack_num += count;
        st.unused_buy_num += count;
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Claims a single floor (`reward_type = 1`) or score (`reward_type = 2`) chest (`babeltower.receiveReward`).
    pub fn receive_reward(
        &mut self,
        player: &mut PlayerState,
        reward_type: u32,
        target_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.players.entry(player.uid).or_default();
        match reward_type {
            1 => {
                let cleared = st.floor > target_id || (st.floor == target_id && st.pass_tag == 1);
                if !cleared || target_id == 0 {
                    return Err("floor_reward_locked");
                }
                if !st.claimed_floor_rewards.insert(target_id) {
                    return Err("already_claimed");
                }
            }
            2 => {
                if st.score < target_id || target_id == 0 {
                    return Err("score_reward_locked");
                }
                if !st.claimed_score_rewards.insert(target_id) {
                    return Err("already_claimed");
                }
            }
            _ => return Err("invalid_reward_type"),
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(50), RewardEntry::Copper(20_000)],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Claims all eligible floor (`1`) or score (`2`) chests (`babeltower.autoReceiveReward` -> `{ receive = [...] }`).
    pub fn auto_receive_reward(
        &mut self,
        player: &mut PlayerState,
        reward_type: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.players.entry(player.uid).or_default();
        let mut claimed_now = Vec::new();
        match reward_type {
            1 => {
                let max_cleared = if st.pass_tag == 1 {
                    st.floor
                } else {
                    st.floor.saturating_sub(1)
                };
                for fl in 1..=max_cleared {
                    if st.claimed_floor_rewards.insert(fl) {
                        claimed_now.push(fl);
                    }
                }
            }
            2 => {
                let max_step = st.score / 100;
                for step in 1..=max_step {
                    let req = step * 100;
                    if st.claimed_score_rewards.insert(req) {
                        claimed_now.push(req);
                    }
                }
            }
            _ => return Err("invalid_reward_type"),
        }
        let pushes = if claimed_now.is_empty() {
            Vec::new()
        } else {
            let n = claimed_now.len() as i64;
            grant_rewards_to_player(
                player,
                &[RewardEntry::Gold(50 * n), RewardEntry::Copper(20_000 * n)],
                now_ts,
            )?
        };
        let receive_arr = DtoValue::Array(claimed_now.into_iter().map(DtoValue::from).collect());
        Ok((DtoValue::map([("receive", receive_arr)]), pushes))
    }
}

// ============================================================================
// Section 2.4: Dota / Trial Tower & Purgatory (`dota.*`, `tmDota.*`, `purgatory.*`, `tmPurgatory.*`)
// ============================================================================

#[derive(Debug, Clone)]
pub struct DotaPlayerState {
    pub normal_layer: u32,
    pub normal_max_layer: u32,
    pub normal_max_attack_layer: u32,
    pub is_open: u32,
    pub defeat_num: u32,
    pub normal_reset_num: u32,
    pub normal_buy_attack_num: u32,
    pub normal_buy_attack_num_used: u32,
    pub elite_attack_num: u32,
    pub elite_max_layer: u32,
    pub elite_buy_attack_num: u32,
    pub elite_cur_layer: u32,
    pub purgatory_max_layer: u32,
    pub purgatory_buy_time: u32,
    pub purgatory_attack_time: u32,
    pub purgatory_last_layer: u32,
}

impl Default for DotaPlayerState {
    fn default() -> Self {
        Self {
            normal_layer: 0,
            normal_max_layer: 0,
            normal_max_attack_layer: 0,
            is_open: 0,
            defeat_num: 0,
            normal_reset_num: 0,
            normal_buy_attack_num: 0,
            normal_buy_attack_num_used: 0,
            elite_attack_num: 0,
            elite_max_layer: 0,
            elite_buy_attack_num: 0,
            elite_cur_layer: 1,
            purgatory_max_layer: 0,
            purgatory_buy_time: 0,
            purgatory_attack_time: 0,
            purgatory_last_layer: 1,
        }
    }
}

/// Trial Tower & Purgatory Engine (`DotaData.lua`, `DotaProxy.lua` Main & TM, `timeMachine_dota_*.json`).
#[derive(Debug, Clone, Default)]
pub struct DotaTowerEngine {
    players: BTreeMap<u64, DotaPlayerState>,
}

impl DotaTowerEngine {
    pub const MAX_NORMAL_LAYER: u32 = 310;
    pub const DEFEAT_LIMIT: u32 = 10;

    /// Returns `dota.getInfo` / `tmDota.getInfo` matching `DotaData:parseData` (`DotaData.lua:136-166`).
    pub fn get_info(&mut self, uid: u64) -> DtoValue {
        let st = self.players.entry(uid).or_default();
        DtoValue::map([
            ("normal_layer", DtoValue::from(st.normal_layer)),
            ("normal_max_layer", DtoValue::from(st.normal_max_layer)),
            (
                "normal_max_attack_layer",
                DtoValue::from(st.normal_max_attack_layer),
            ),
            ("is_open", DtoValue::from(st.is_open)),
            ("defeat_num", DtoValue::from(st.defeat_num)),
            ("normal_reset_num", DtoValue::from(st.normal_reset_num)),
            (
                "normal_buy_attack_num",
                DtoValue::from(st.normal_buy_attack_num),
            ),
            (
                "normal_buy_attack_num_used",
                DtoValue::from(st.normal_buy_attack_num_used),
            ),
            ("elite_attack_num", DtoValue::from(st.elite_attack_num)),
            ("elite_max_layer", DtoValue::from(st.elite_max_layer)),
            (
                "elite_buy_attack_num",
                DtoValue::from(st.elite_buy_attack_num),
            ),
            ("elite_cur_layer", DtoValue::from(st.elite_cur_layer.max(1))),
            (
                "purgatory_max_layer",
                DtoValue::from(st.purgatory_max_layer),
            ),
            ("purgatory_buy_time", DtoValue::from(st.purgatory_buy_time)),
            (
                "purgatory_attack_time",
                DtoValue::from(st.purgatory_attack_time),
            ),
            (
                "purgatory_last_layer",
                DtoValue::from(st.purgatory_last_layer.max(1)),
            ),
        ])
    }

    /// Starts a Trial Tower or Purgatory battle (`dota.attack`, `dota.attackElite`, `purgatory.attackPurgatory`),
    /// returning Type A `serverData`.
    pub fn attack(
        &mut self,
        player: &PlayerState,
        layer_id: u32,
        copy_engine: &mut CopyCampaignEngine,
    ) -> Result<DtoValue, &'static str> {
        if layer_id == 0 || layer_id > Self::MAX_NORMAL_LAYER {
            return Err("invalid_tower_layer");
        }
        let st = self.players.entry(player.uid).or_default();
        let lives = (Self::DEFEAT_LIMIT + st.normal_buy_attack_num)
            .saturating_sub(st.defeat_num + st.normal_buy_attack_num_used);
        if lives == 0 {
            return Err("dota_no_lives");
        }
        st.normal_max_attack_layer = st.normal_max_attack_layer.max(layer_id);

        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let (m1, m2) = stage_monster_htids(10_000 + layer_id);
        let t2 = vec![
            CombatUnit::monster(93_000_001, m1, 0, layer_id.max(1)),
            CombatUnit::monster(93_000_002, m2, 1, layer_id.max(1)),
        ];
        let drops = vec![
            RewardEntry::Copper(50_000 + i64::from(layer_id) * 5_000),
            RewardEntry::HeroSoul(100),
        ];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(
            0,
            &format!("TowerGuard_{layer_id}"),
            layer_id.max(1),
            0,
            u64::from(layer_id.max(1)) * 2_000,
            0,
        );
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &drops);
        let server_dto = sim.server_attack_dto.clone();
        copy_engine.record_battle(sim);
        Ok(server_dto)
    }

    /// Settles a Trial Tower / Elite / Purgatory floor (`dota.battleResult`, `dota.passElite`, `purgatory.passPurgatory`).
    /// `mode`: `0` = Normal, `1` = Elite, `2` = Purgatory.
    pub fn pass(
        &mut self,
        player: &mut PlayerState,
        mode: u32,
        layer_id: u32,
        is_win: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if layer_id == 0 {
            return Err("invalid_tower_layer");
        }
        let st = self.players.entry(player.uid).or_default();
        if !is_win {
            if mode == 0 {
                st.defeat_num += 1;
            }
            return Ok((DtoValue::from("fail"), Vec::new()));
        }
        match mode {
            0 => {
                st.normal_layer = st.normal_layer.max(layer_id);
                st.normal_max_layer = st.normal_max_layer.max(layer_id);
                st.is_open = 0;
            }
            1 => {
                st.elite_attack_num += 1;
                st.elite_cur_layer = layer_id;
                st.elite_max_layer = st.elite_max_layer.max(layer_id);
            }
            _ => {
                st.purgatory_attack_time += 1;
                st.purgatory_last_layer = layer_id;
                st.purgatory_max_layer = st.purgatory_max_layer.max(layer_id);
            }
        }
        let mut pushes = grant_rewards_to_player(
            player,
            &[
                RewardEntry::Copper(50_000 + i64::from(layer_id) * 5_000),
                RewardEntry::Item { tid: 17002, qty: 1 },
            ],
            now_ts,
        )?;
        if let Some(field) = player.add_currency_by_price_type(10, 500 + u64::from(layer_id) * 50) {
            pushes.push(PlayerState::build_user_update_push([field], now_ts));
        }
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Opens a chest floor in Trial Tower (`dota.openBox` / `tmDota.openBox`).
    pub fn open_box(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.players.entry(player.uid).or_default();
        st.is_open = 1;
        st.normal_layer = (st.normal_layer + 1).min(Self::MAX_NORMAL_LAYER);
        st.normal_max_layer = st.normal_max_layer.max(st.normal_layer);
        let mut pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(30), RewardEntry::Item { tid: 30002, qty: 1 }],
            now_ts,
        )?;
        if let Some(field) = player.add_currency_by_price_type(10, 1_000) {
            pushes.push(PlayerState::build_user_update_push([field], now_ts));
        }
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Sweeps Trial Tower / Elite / Purgatory (`dota.sweep`, `dota.sweepElite`, `purgatory.sweepPurgatory`).
    pub fn sweep(
        &mut self,
        player: &mut PlayerState,
        mode: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.players.entry(player.uid).or_default();
        match mode {
            0 => {
                if st.normal_layer >= st.normal_max_layer {
                    return Err("nothing_to_sweep");
                }
                st.normal_layer += 1;
            }
            1 => {
                if st.elite_max_layer == 0 {
                    return Err("nothing_to_sweep");
                }
                st.elite_attack_num += 1;
            }
            _ => {
                if st.purgatory_max_layer == 0 {
                    return Err("nothing_to_sweep");
                }
                st.purgatory_attack_time += 1;
            }
        }
        let mut pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(60_000)], now_ts)?;
        if let Some(field) = player.add_currency_by_price_type(10, 500) {
            pushes.push(PlayerState::build_user_update_push([field], now_ts));
        }
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Resets Normal Trial Tower back to floor 0 (`dota.reset` / `tmDota.reset`).
    pub fn reset(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.players.entry(player.uid).or_default();
        let cost = if st.normal_reset_num == 0 { 0i64 } else { 50i64 };
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        st.normal_reset_num += 1;
        st.normal_layer = 0;
        st.defeat_num = 0;
        st.is_open = 0;
        let pushes = if cost > 0 {
            vec![PlayerState::build_user_update_push(
                [("gold", DtoValue::from(player.gold))],
                now_ts,
            )]
        } else {
            Vec::new()
        };
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Buys extra attempts (`mode`: `0` = Normal, `1` = Elite, `2` = Purgatory).
    pub fn buy_atk_num(
        &mut self,
        player: &mut PlayerState,
        mode: u32,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count == 0 {
            return Err("invalid_count");
        }
        let unit_cost = match mode {
            0 => 50i64,
            1 => 30i64,
            _ => 100i64,
        };
        let cost = unit_cost * i64::from(count);
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let st = self.players.entry(player.uid).or_default();
        match mode {
            0 => st.normal_buy_attack_num += count,
            1 => st.elite_buy_attack_num += count,
            _ => st.purgatory_buy_time += count,
        }
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::from("ok"), pushes))
    }
}

// ============================================================================
// Section 2.5: Awake Copy, World Boss / Fiend / Rebel, Discover, Devil & Soaring
// ============================================================================

/// Awake Copy Co-op Engine (`AwakeCopyData.lua`, `awakeCopy.*`, `team.*`).
#[derive(Debug, Clone, Default)]
pub struct AwakeCopyEngine {
    passed: BTreeMap<u64, BTreeSet<u32>>,
    left_attack_num: BTreeMap<u64, u32>,
    left_help_num: BTreeMap<u64, u32>,
    buy_num: BTreeMap<u64, u32>,
}

impl AwakeCopyEngine {
    /// Returns `awakeCopy.getAwakeCopyInfo` / `tmAwakeCopy.getAwakeCopyInfo` (`AwakeCopyData.lua:90-102`).
    pub fn get_info(&mut self, uid: u64) -> DtoValue {
        let pass_arr: Vec<DtoValue> = self
            .passed
            .entry(uid)
            .or_default()
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        let atk = *self.left_attack_num.entry(uid).or_insert(3);
        let help = *self.left_help_num.entry(uid).or_insert(3);
        let buy = *self.buy_num.entry(uid).or_insert(0);
        DtoValue::map([
            ("pass_str", DtoValue::Array(pass_arr)),
            ("last_attack_num", DtoValue::from(atk)),
            ("last_help_num", DtoValue::from(help)),
            ("buy_num", DtoValue::from(buy)),
        ])
    }

    pub fn get_room_list() -> DtoValue {
        DtoValue::Array(Vec::new())
    }

    pub fn quick_battle(
        &mut self,
        player: &mut PlayerState,
        copy_id: u32,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let atk = self.left_attack_num.entry(player.uid).or_insert(3);
        if *atk == 0 {
            return Err("awake_copy_no_attempts");
        }
        *atk -= 1;
        self.passed
            .entry(player.uid)
            .or_default()
            .insert(copy_id.max(1));

        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let (m1, _) = stage_monster_htids(copy_id);
        let t2 = vec![CombatUnit::monster(94_000_001, m1, 0, player.level.max(1))];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, "Giác Tỉnh Thủ Lĩnh", player.level.max(1), 0, 10_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let report_blob = sim.to_amf3_report_bytes();
        copy_engine.record_battle(sim);

        let rewards = [RewardEntry::Copper(25_000), RewardEntry::Item { tid: 17002, qty: 2 }];
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("brid", DtoValue::from(brid)),
                ("fightRet", DtoValue::Bytes(report_blob)),
                ("drop", build_copy_drop_dto(&rewards)),
            ]),
            pushes,
        ))
    }

    pub fn buy_num(
        &mut self,
        player: &mut PlayerState,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count == 0 {
            return Err("invalid_count");
        }
        let cost = i64::from(count) * 30;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        *self.buy_num.entry(player.uid).or_insert(0) += count;
        *self.left_attack_num.entry(player.uid).or_insert(3) += count;
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::from("ok"), pushes))
    }
}

#[derive(Debug, Clone)]
pub struct RebelPlayerState {
    pub atk_count: u32,
    pub use_count: u32,
    pub damage: u64,
    pub max_damage: u64,
    pub share_count: u32,
    pub attack_share_count: u32,
    pub last_recovery_time: u64,
    pub damage_reward_claimed: BTreeSet<u32>,
}

impl Default for RebelPlayerState {
    fn default() -> Self {
        Self {
            atk_count: 10,
            use_count: 0,
            damage: 0,
            max_damage: 0,
            share_count: 0,
            attack_share_count: 0,
            last_recovery_time: 1_795_500_000,
            damage_reward_claimed: BTreeSet::new(),
        }
    }
}

/// World Boss, Fiend & Rebel Suppression Engine (`WorldBossData.lua`, `FiendData.lua`, `RebelData.lua`, `RebelProxy.lua`).
///
/// **CRITICAL SHOP DEPENDENCY**: All 26 `BaseShopProxy` shops require `RebelData` (`rebel.getRebelInfo` / `rebel.getInfo`)
/// to initialize without throwing in `RebelData:parseData(data)` (`RebelData.lua:53-122`).
#[derive(Debug, Clone)]
pub struct WorldBossAndRebelEngine {
    pub boss_max_hp: i64,
    pub boss_cur_hp: i64,
    pub fiend_max_hp: i64,
    pub fiend_cur_hp: i64,
    boss_damage: BTreeMap<u64, u64>,
    rebel_players: BTreeMap<u64, RebelPlayerState>,
}

impl Default for WorldBossAndRebelEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldBossAndRebelEngine {
    pub fn new() -> Self {
        Self {
            boss_max_hp: 10_000_000,
            boss_cur_hp: 10_000_000,
            fiend_max_hp: 5_000_000,
            fiend_cur_hp: 5_000_000,
            boss_damage: BTreeMap::new(),
            rebel_players: BTreeMap::new(),
        }
    }

    /// Exact `rebel.getRebelInfo` / `rebel.getInfo` / `tmRebel.getInfo` response required by `RebelData:parseData`
    /// (`RebelData.lua:53-122` & `:399-437`) so NONE of the 26 `BaseShopProxy` shops ever hang on `DataLoading`!
    pub fn get_rebel_info(&mut self, player: &PlayerState, now_ts: u64) -> DtoValue {
        let st = self.rebel_players.entry(player.uid).or_default();
        if st.last_recovery_time == 0 {
            st.last_recovery_time = now_ts;
        }
        let claimed_arr: Vec<DtoValue> = st
            .damage_reward_claimed
            .iter()
            .copied()
            .map(DtoValue::from)
            .collect();
        let self_rebel = DtoValue::map([
            ("killer", DtoValue::from(0u64)),
            (
                "va_reward",
                DtoValue::map([
                    ("find_drop", DtoValue::Map(BTreeMap::new())),
                    ("kill_drop", DtoValue::Map(BTreeMap::new())),
                ]),
            ),
        ]);

        DtoValue::map([
            ("level_record", DtoValue::from(player.level.max(1))),
            ("have_reward", DtoValue::from(false)),
            ("max_damage", DtoValue::from(st.max_damage)),
            ("damage", DtoValue::from(st.damage)),
            ("total_exploit", DtoValue::from(st.damage)),
            ("use_count", DtoValue::from(st.use_count)),
            ("atk_count", DtoValue::from(st.atk_count)),
            ("last_recovery_time", DtoValue::from(st.last_recovery_time)),
            ("share_count", DtoValue::from(st.share_count)),
            ("attack_share_count", DtoValue::from(st.attack_share_count)),
            ("is_have_rebel", DtoValue::from(false)),
            ("self_rebel", self_rebel),
            ("damage_reward", DtoValue::Array(claimed_arr)),
        ])
    }

    /// Returns `rebel.enter` (`RebelProxy:parseData`).
    pub fn enter_rebel(&mut self, player: &PlayerState) -> DtoValue {
        let mut extend_info = BTreeMap::new();
        extend_info.insert(
            player.uid.to_string(),
            DtoValue::map([
                ("uname", DtoValue::from(player.uname.as_str())),
                ("utid", DtoValue::from(player.utid.max(1))),
                ("dress_id", DtoValue::from(player.dress_id)),
                ("guild_name", DtoValue::from(player.guild_name.as_str())),
            ]),
        );
        DtoValue::map([
            ("rebel", DtoValue::Array(Vec::new())),
            ("extendInfo", DtoValue::Map(extend_info)),
            ("max_damage_rank", DtoValue::from(-1i32)),
            ("damage_rank", DtoValue::from(-1i32)),
            ("specialTimeIndex", DtoValue::from(1u32)),
        ])
    }

    /// Attacks a rebel army (`rebel.attack`, `args = [rebelId, attackMode]`).
    pub fn attack_rebel(
        &mut self,
        player: &mut PlayerState,
        rebel_id: u64,
        attack_mode: u32,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let cost_count = if attack_mode == 2 { 2u32 } else { 1u32 };
        {
            let st = self.rebel_players.entry(player.uid).or_default();
            if st.atk_count < cost_count {
                return Err("not_enough_rebel_atk");
            }
            st.atk_count -= cost_count;
            st.last_recovery_time = now_ts;
        }

        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(95_000_001, 500_101, 0, player.level.max(1))];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, "Quân Phản Loạn", player.level.max(1), 0, 8_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let dmg: u64 = sim
            .steps
            .iter()
            .filter(|s| t1.iter().any(|u| u.hid == s.attacker_hid))
            .map(|s| s.damage.max(0) as u64)
            .sum::<u64>()
            .max(1_000);
        let report_blob = sim.to_amf3_report_bytes();
        copy_engine.record_battle(sim);

        let st = self.rebel_players.entry(player.uid).or_default();
        st.damage = st.damage.saturating_add(dmg);
        st.max_damage = st.max_damage.max(dmg);

        let pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(15_000)], now_ts)?;
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("brid", DtoValue::from(brid)),
            ("damage", DtoValue::from(dmg)),
            ("cost_count", DtoValue::from(cost_count)),
            ("damage_rank", DtoValue::from(1u32)),
            ("max_damage_rank", DtoValue::from(1u32)),
            ("battle_record", DtoValue::Bytes(report_blob)),
            (
                "rebel",
                DtoValue::map([
                    ("id", DtoValue::from(rebel_id.max(1))),
                    ("killer", DtoValue::from(player.uid)),
                    ("hp", DtoValue::from(0u32)),
                    ("atk_count", DtoValue::from(1u32)),
                    ("die_time", DtoValue::from(now_ts)),
                    ("can_thank", DtoValue::from(0u32)),
                ]),
            ),
        ]);
        Ok((ret, pushes))
    }

    pub fn buy_rebel_atk_num(
        &mut self,
        player: &mut PlayerState,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count == 0 {
            return Err("invalid_count");
        }
        let mut pushes = Vec::new();
        if player.bag.count_by_tid(15007) >= count {
            let deltas = player.bag.consume_item(15007, count)?;
            if let Some(bp) = crate::player::BagState::build_update_push(&deltas, now_ts) {
                pushes.push(bp);
            }
        } else {
            let cost = i64::from(count) * 5;
            if player.gold < cost {
                return Err("not_enough_gold");
            }
            player.gold -= cost;
            pushes.push(PlayerState::build_user_update_push(
                [("gold", DtoValue::from(player.gold))],
                now_ts,
            ));
        }
        let st = self.rebel_players.entry(player.uid).or_default();
        st.atk_count += count;
        st.use_count += count;
        Ok((DtoValue::from("ok"), pushes))
    }

    /// Returns `boss.getBossData` (`WorldBossData:parseData` at `WorldBossData.lua:37-51`).
    pub fn get_boss_data(&self, player: &PlayerState, now_ts: u64) -> DtoValue {
        let atk_hp = self.boss_damage.get(&player.uid).copied().unwrap_or(0);
        DtoValue::map([
            (
                "atker",
                DtoValue::map([
                    ("uid", DtoValue::from(player.uid)),
                    ("uname", DtoValue::from(player.uname.as_str())),
                    ("rank", DtoValue::from(if atk_hp > 0 { 1u32 } else { 0u32 })),
                    ("atk_hp", DtoValue::from(atk_hp)),
                    ("inspire_num", DtoValue::from(0u32)),
                    ("revive_num", DtoValue::from(0u32)),
                    ("cd_time", DtoValue::from(0u64)),
                ]),
            ),
            (
                "boss",
                DtoValue::map([
                    ("id", DtoValue::from(1u32)),
                    ("level", DtoValue::from(50u32)),
                    ("hp", DtoValue::from(self.boss_cur_hp.max(0))),
                    ("maxHp", DtoValue::from(self.boss_max_hp)),
                    ("start_time", DtoValue::from(now_ts)),
                ]),
            ),
            (
                "shop",
                DtoValue::map([
                    ("maxHp", DtoValue::from(atk_hp)),
                    ("total_atk_num", DtoValue::from(u32::from(atk_hp > 0))),
                ]),
            ),
            ("top", DtoValue::Array(Vec::new())),
            ("killer", DtoValue::Map(BTreeMap::new())),
        ])
    }

    /// Attacks World Boss (`boss.attack`).
    pub fn attack_boss(
        &mut self,
        player: &mut PlayerState,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(95_100_001, 500_101, 0, 50)];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, "Trương Giác Ma Hóa", 50, 0, 500_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let dmg = (player.fight_force() / 2).max(5_000);
        let report_blob = sim.to_amf3_report_bytes();
        copy_engine.record_battle(sim);

        self.boss_cur_hp = (self.boss_cur_hp - dmg as i64).max(0);
        *self.boss_damage.entry(player.uid).or_insert(0) += dmg;
        let pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(25_000)], now_ts)?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("brid", DtoValue::from(brid)),
                ("damage", DtoValue::from(dmg)),
                ("boss_hp", DtoValue::from(self.boss_cur_hp)),
                ("fightRet", DtoValue::Bytes(report_blob)),
            ]),
            pushes,
        ))
    }

    /// Returns `fiend.getFiendInfo` / `fiend.enter` (`FiendData:parseData` at `FiendData.lua:44-57`).
    pub fn get_fiend_info(&self, now_ts: u64) -> DtoValue {
        DtoValue::map([
            ("is_open_box", DtoValue::from(false)),
            ("kill_uid", DtoValue::from(0u64)),
            (
                "magic",
                DtoValue::map([
                    ("cur_magic", DtoValue::from(0u32)),
                    ("max_magic", DtoValue::from(100u32)),
                    ("level", DtoValue::from(1u32)),
                    ("reset_time", DtoValue::from(now_ts)),
                ]),
            ),
            (
                "fiend",
                DtoValue::map([
                    ("id", DtoValue::from(1u32)),
                    ("fiend_id", DtoValue::from(1u32)),
                    ("hp", DtoValue::from(self.fiend_cur_hp.max(0))),
                    ("maxHp", DtoValue::from(self.fiend_max_hp)),
                    ("gen_time", DtoValue::from(now_ts)),
                ]),
            ),
            (
                "atker",
                DtoValue::map([
                    ("his_max_damage", DtoValue::from(1u64)),
                    ("cd_time", DtoValue::from(0u64)),
                    ("atk_num", DtoValue::from(5u32)),
                ]),
            ),
        ])
    }
}

/// Territory Patrol & Suppression Engine (`DiscoverData.lua`, `DiscoverProxy.lua`, `discover.*`).
#[derive(Debug, Clone, Default)]
pub struct DiscoverEngine {
    passed_cities: BTreeMap<u64, BTreeSet<u32>>,
    patrolling_cities: BTreeMap<(u64, u32), u64>,
    elite_max_layer: BTreeMap<u64, u32>,
    elite_attack_num: BTreeMap<u64, u32>,
    elite_buy_num: BTreeMap<u64, u32>,
}

impl DiscoverEngine {
    /// Returns `discover.getRewardCity` (`DiscoverData:parseData`).
    pub fn get_reward_city(&self, uid: u64) -> DtoValue {
        let pass_city: Vec<DtoValue> = self
            .passed_cities
            .get(&uid)
            .map(|s| s.iter().copied().map(DtoValue::from).collect())
            .unwrap_or_default();
        let elite_max = self.elite_max_layer.get(&uid).copied().unwrap_or(0);
        let elite_atk = self.elite_attack_num.get(&uid).copied().unwrap_or(0);
        let elite_buy = self.elite_buy_num.get(&uid).copied().unwrap_or(0);
        DtoValue::map([
            ("reward", DtoValue::Array(Vec::new())),
            ("pass_city", DtoValue::Array(pass_city)),
            ("kill_num", DtoValue::Array(Vec::new())),
            ("elite_max_layer", DtoValue::from(elite_max)),
            ("elite_cur_layer", DtoValue::from((elite_max + 1).max(1))),
            ("elite_attack_num", DtoValue::from(elite_atk)),
            ("elite_buy_attack_num", DtoValue::from(elite_buy)),
        ])
    }

    /// Returns `discover.getDiscover` (`args = [cityId]`) so clicking a city never hangs `DataLoading`!
    pub fn get_discover(&self, uid: u64, city_id: u32) -> DtoValue {
        let is_pass = self
            .passed_cities
            .get(&uid)
            .is_some_and(|s| s.contains(&city_id));
        DtoValue::map([
            ("city_id", DtoValue::from(city_id)),
            ("cur_hp", DtoValue::from(10_000i64)),
            ("max_hp", DtoValue::from(10_000i64)),
            ("monster_id", DtoValue::from(1_010_111u32)),
            ("attack_times", DtoValue::from(0u32)),
            ("kill_num", DtoValue::from(0u32)),
            ("is_pass", DtoValue::from(u32::from(is_pass))),
            ("have_first_reward", DtoValue::from(u32::from(is_pass))),
        ])
    }

    pub fn attack(
        &mut self,
        player: &PlayerState,
        city_id: u32,
        copy_engine: &mut CopyCampaignEngine,
    ) -> Result<DtoValue, &'static str> {
        if city_id == 0 {
            return Err("invalid_city");
        }
        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(96_000_001, 1_010_111, 0, player.level.max(1))];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, &format!("Thủ Quân_{city_id}"), player.level.max(1), 0, 5_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let dto = sim.server_attack_dto.clone();
        copy_engine.record_battle(sim);
        Ok(dto)
    }

    pub fn pass(
        &mut self,
        player: &mut PlayerState,
        city_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if city_id == 0 {
            return Err("invalid_city");
        }
        self.passed_cities.entry(player.uid).or_default().insert(city_id);
        let pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(20_000)], now_ts)?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                (
                    "new",
                    DtoValue::map([
                        ("cur_hp", DtoValue::from(10_000i64)),
                        ("monster_id", DtoValue::from(1_010_111u32)),
                    ]),
                ),
            ]),
            pushes,
        ))
    }

    pub fn patrol(&mut self, uid: u64, city_id: u32, now_ts: u64) -> Result<DtoValue, &'static str> {
        if city_id == 0 {
            return Err("invalid_city");
        }
        self.patrolling_cities.insert((uid, city_id), now_ts);
        Ok(DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("city_id", DtoValue::from(city_id)),
            ("start_time", DtoValue::from(now_ts)),
        ]))
    }

    pub fn receive_patrol_reward(
        &mut self,
        player: &mut PlayerState,
        city_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if self.patrolling_cities.remove(&(player.uid, city_id)).is_none() {
            return Err("not_patrolling");
        }
        let rewards = [
            RewardEntry::Gold(30),
            RewardEntry::Item { tid: 17002, qty: 2 },
        ];
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("drop", build_copy_drop_dto(&rewards)),
            ]),
            pushes,
        ))
    }
}

#[derive(Debug, Clone, Default)]
struct BossRushState {
    win_chapter: u32,
    defeat_num: u32,
    buy_num: u32,
    cleared_ids: BTreeSet<u32>,
}

fn build_boss_rush_va_copy(cleared: &BTreeSet<u32>, label: &str) -> DtoValue {
    let mut va_copy = BTreeMap::new();
    for id in 1..=10u32 {
        let max_hp = i64::from(id) * 50_000;
        let cur_hp = if cleared.contains(&id) { 0 } else { max_hp };
        va_copy.insert(
            id.to_string(),
            DtoValue::map([
                ("va_curhp", DtoValue::map([("1", DtoValue::from(cur_hp))])),
                ("va_maxhp", DtoValue::map([("1", DtoValue::from(max_hp))])),
                (
                    "va_player_extra",
                    DtoValue::map([
                        ("name", DtoValue::from(format!("{label}_{id}"))),
                        ("quality", DtoValue::from(6u32)),
                        ("fight_force", DtoValue::from(u64::from(id) * 50_000)),
                    ]),
                ),
            ]),
        );
    }
    DtoValue::Map(va_copy)
}

/// Devil Copy Engine (`DevilCopyData.lua`, `DevilCopyProxy.lua`, `devil/devil_copy.json` IDs `1..=10`).
#[derive(Debug, Clone, Default)]
pub struct DevilCopyEngine {
    states: BTreeMap<u64, BossRushState>,
}

impl DevilCopyEngine {
    pub fn get_login_info(&mut self, uid: u64) -> DtoValue {
        let st = self.states.entry(uid).or_default();
        DtoValue::map([
            ("win_chapter", DtoValue::from(st.win_chapter)),
            ("defeat_num", DtoValue::from(st.defeat_num)),
            ("buy_num", DtoValue::from(st.buy_num)),
        ])
    }

    pub fn get_info(&mut self, uid: u64) -> DtoValue {
        let st = self.states.entry(uid).or_default();
        DtoValue::map([
            ("win_chapter", DtoValue::from(st.win_chapter)),
            ("defeat_num", DtoValue::from(st.defeat_num)),
            ("buy_num", DtoValue::from(st.buy_num)),
            (
                "va_copy",
                build_boss_rush_va_copy(&st.cleared_ids, "MaHoaThuLinh"),
            ),
        ])
    }

    pub fn get_copy(&self, stage_id: u32) -> DtoValue {
        DtoValue::map([(
            "va_team",
            DtoValue::map([
                ("name", DtoValue::from(format!("MaHoaThuLinh_{stage_id}"))),
                ("quality", DtoValue::from(6u32)),
                ("fight_force", DtoValue::from(u64::from(stage_id.max(1)) * 50_000)),
            ]),
        )])
    }

    pub fn attack(
        &mut self,
        player: &PlayerState,
        stage_id: u32,
        copy_engine: &mut CopyCampaignEngine,
    ) -> Result<DtoValue, &'static str> {
        if !(1..=10).contains(&stage_id) {
            return Err("invalid_devil_stage");
        }
        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(97_000_001, 600_101, 0, stage_id * 10)];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, "Ma Hóa Cường Đạo", stage_id * 10, 0, 50_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let dto = sim.server_attack_dto.clone();
        copy_engine.record_battle(sim);
        Ok(dto)
    }

    pub fn pass(
        &mut self,
        player: &mut PlayerState,
        stage_id: u32,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=10).contains(&stage_id) {
            return Err("invalid_devil_stage");
        }
        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(97_000_002, 600_101, 0, stage_id)];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, "Ma Hóa Cường Đạo", stage_id, 0, 5_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let report_blob = sim.to_amf3_report_bytes();
        copy_engine.record_battle(sim);

        let st = self.states.entry(player.uid).or_default();
        st.cleared_ids.insert(stage_id);
        if stage_id == 10 {
            st.win_chapter = (st.win_chapter + 1).min(4);
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Item {
                tid: 19090,
                qty: 10 * stage_id,
            }],
            now_ts,
        )?;
        let pass_copy_reward = DtoValue::array([
            DtoValue::array([
                DtoValue::from(64u32),
                DtoValue::from(0u32),
                DtoValue::from(150u32 * stage_id),
            ]),
            DtoValue::array([
                DtoValue::from(4u32),
                DtoValue::from(19090u32),
                DtoValue::from(10u32 * stage_id),
            ]),
        ]);
        Ok((
            DtoValue::map([
                ("battle_result", DtoValue::from(1u32)),
                ("va_curhp", DtoValue::map([("1", DtoValue::from(0i64))])),
                ("va_copy", DtoValue::Map(BTreeMap::new())),
                ("passCopyReward", pass_copy_reward),
                ("fightReport", DtoValue::Bytes(report_blob)),
            ]),
            pushes,
        ))
    }

    pub fn buy_attack_times(
        &mut self,
        player: &mut PlayerState,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count == 0 {
            return Err("invalid_count");
        }
        let cost = i64::from(count) * 20;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        self.states.entry(player.uid).or_default().buy_num += count;
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::from("ok"), pushes))
    }
}

/// Soaring Copy Engine (`SoaringCopyData.lua`, `SoaringCopyProxy.lua`, `soaring/soaring_copy.json` IDs `1..=10`).
#[derive(Debug, Clone, Default)]
pub struct SoaringCopyEngine {
    states: BTreeMap<u64, BossRushState>,
}

impl SoaringCopyEngine {
    pub fn get_login_info(&mut self, uid: u64) -> DtoValue {
        let st = self.states.entry(uid).or_default();
        DtoValue::map([
            ("win_chapter", DtoValue::from(st.win_chapter)),
            ("defeat_num", DtoValue::from(st.defeat_num)),
            ("buy_num", DtoValue::from(st.buy_num)),
        ])
    }

    pub fn get_info(&mut self, uid: u64) -> DtoValue {
        let st = self.states.entry(uid).or_default();
        DtoValue::map([
            ("win_chapter", DtoValue::from(st.win_chapter)),
            ("defeat_num", DtoValue::from(st.defeat_num)),
            ("buy_num", DtoValue::from(st.buy_num)),
            (
                "va_copy",
                build_boss_rush_va_copy(&st.cleared_ids, "TienGioiThuLinh"),
            ),
        ])
    }

    pub fn get_copy(&self, stage_id: u32) -> DtoValue {
        DtoValue::map([(
            "va_team",
            DtoValue::map([
                ("name", DtoValue::from(format!("Lạc Tiên Nữ_{stage_id}"))),
                ("quality", DtoValue::from(6u32)),
                ("fight_force", DtoValue::from(u64::from(stage_id.max(1)) * 60_000)),
            ]),
        )])
    }

    pub fn attack(
        &mut self,
        player: &PlayerState,
        stage_id: u32,
        copy_engine: &mut CopyCampaignEngine,
    ) -> Result<DtoValue, &'static str> {
        if !(1..=10).contains(&stage_id) {
            return Err("invalid_soaring_stage");
        }
        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(98_000_001, 600_102, 0, stage_id * 10)];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, "Lạc Tiên Nữ", stage_id * 10, 0, 60_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let dto = sim.server_attack_dto.clone();
        copy_engine.record_battle(sim);
        Ok(dto)
    }

    pub fn pass(
        &mut self,
        player: &mut PlayerState,
        stage_id: u32,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=10).contains(&stage_id) {
            return Err("invalid_soaring_stage");
        }
        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(98_000_002, 600_102, 0, stage_id)];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(0, "Lạc Tiên Nữ", stage_id, 0, 6_000, 0);
        let sim = simulate_battle(brid, false, &t1, &t2, u1, u2, &[]);
        let report_blob = sim.to_amf3_report_bytes();
        copy_engine.record_battle(sim);

        let st = self.states.entry(player.uid).or_default();
        st.cleared_ids.insert(stage_id);
        if stage_id == 10 {
            st.win_chapter = (st.win_chapter + 1).min(4);
        }
        let pushes = grant_rewards_to_player(player, &[RewardEntry::Copper(30_000)], now_ts)?;
        let pass_copy_reward = DtoValue::array([
            DtoValue::array([
                DtoValue::from(78u32),
                DtoValue::from(0u32),
                DtoValue::from(300u32 * stage_id),
            ]),
            DtoValue::array([
                DtoValue::from(106u32),
                DtoValue::from(0u32),
                DtoValue::from(100u32 * stage_id),
            ]),
        ]);
        Ok((
            DtoValue::map([
                ("battle_result", DtoValue::from(1u32)),
                ("va_curhp", DtoValue::map([("1", DtoValue::from(0i64))])),
                ("va_copy", DtoValue::Map(BTreeMap::new())),
                ("passCopyReward", pass_copy_reward),
                ("fightReport", DtoValue::Bytes(report_blob)),
            ]),
            pushes,
        ))
    }

    pub fn buy_attack_times(
        &mut self,
        player: &mut PlayerState,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count == 0 {
            return Err("invalid_count");
        }
        let cost = i64::from(count) * 20;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        self.states.entry(player.uid).or_default().buy_num += count;
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::from("ok"), pushes))
    }
}

// ============================================================================
// Section 3: PvP Arena (`arena.*` & `timeMachineArena.*`)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ArenaCompetitor {
    pub uid: u64,
    pub uname: String,
    pub level: u32,
    pub fight_force: u64,
    pub position: u32,
    pub is_npc: bool,
    pub lead_htid: u32,
}

impl ArenaCompetitor {
    pub fn to_dto(&self) -> DtoValue {
        DtoValue::map([
            ("uid", DtoValue::from(self.uid)),
            ("uname", DtoValue::from(self.uname.as_str())),
            ("utid", DtoValue::from(self.lead_htid.max(1))),
            ("level", DtoValue::from(self.level)),
            ("quality", DtoValue::from(4u32)),
            ("dress_id", DtoValue::from(0u32)),
            ("fight_force", DtoValue::from(self.fight_force)),
            ("position", DtoValue::from(self.position)),
            ("teamId", DtoValue::from(if self.is_npc { self.position } else { 0 })),
        ])
    }
}

#[derive(Debug, Clone)]
pub struct ArenaEngine {
    ladder: BTreeMap<u32, ArenaCompetitor>,
    uid_to_pos: BTreeMap<u64, u32>,
    history_min_pos: BTreeMap<u64, u32>,
    attacks_used: BTreeMap<u64, u32>,
    attacks_bought: BTreeMap<u64, u32>,
    wins_today: BTreeMap<u64, u32>,
    claimed_num_rewards: BTreeMap<u64, BTreeSet<u32>>,
    last_reward_time: BTreeMap<u64, u64>,
    tm_atk_num: BTreeMap<u64, u32>,
    tm_rec_day: BTreeMap<u64, u32>,
    tm_rec_challenge: BTreeMap<u64, u32>,
}

impl Default for ArenaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ArenaEngine {
    const GUARD_HTIDS: [u32; 15] = [
        401, 201, 101, 301, 204, 106, 312, 406, 107, 207, 304, 403, 116, 314, 310,
    ];
    const DEFAULT_REWARD_TIME: u64 = 1_795_400_000;

    pub fn new() -> Self {
        let mut ladder = BTreeMap::new();
        let mut uid_to_pos = BTreeMap::new();
        for pos in 1..=15u32 {
            let npc_uid = u64::from(pos);
            let lead_htid = Self::GUARD_HTIDS[(pos - 1) as usize];
            let comp = ArenaCompetitor {
                uid: npc_uid,
                uname: format!("ArenaGuard_{pos}"),
                level: 10 + pos * 2,
                fight_force: 3_000 + u64::from(16 - pos) * 1_200,
                position: pos,
                is_npc: true,
                lead_htid,
            };
            ladder.insert(pos, comp);
            uid_to_pos.insert(npc_uid, pos);
        }
        Self {
            ladder,
            uid_to_pos,
            history_min_pos: BTreeMap::new(),
            attacks_used: BTreeMap::new(),
            attacks_bought: BTreeMap::new(),
            wins_today: BTreeMap::new(),
            claimed_num_rewards: BTreeMap::new(),
            last_reward_time: BTreeMap::new(),
            tm_atk_num: BTreeMap::new(),
            tm_rec_day: BTreeMap::new(),
            tm_rec_challenge: BTreeMap::new(),
        }
    }

    fn resolve_player_lead_htid(player: &PlayerState) -> u32 {
        player
            .roster
            .formation
            .iter()
            .copied()
            .find(|&hid| hid != 0)
            .and_then(|hid| player.roster.heroes.get(&hid))
            .map(|h| h.htid)
            .filter(|&htid| htid > 0)
            .unwrap_or_else(|| player.utid.max(1))
    }

    /// Replaces the default NPC ladder with real snapshot competitors `(uid, uname, level, fight_force, position)`.
    pub fn seed_competitors(&mut self, entries: Vec<(u64, String, u32, u64, u32)>) {
        let expanded = entries
            .into_iter()
            .map(|(uid, uname, level, fight_force, position)| {
                let fallback_htid =
                    Self::GUARD_HTIDS[((position.max(1) - 1) as usize) % Self::GUARD_HTIDS.len()];
                (uid, uname, level, fight_force, position, fallback_htid)
            })
            .collect();
        self.seed_competitors_with_htid(expanded);
    }

    /// Replaces the default NPC ladder with real snapshot competitors including `lead_htid`.
    pub fn seed_competitors_with_htid(&mut self, entries: Vec<(u64, String, u32, u64, u32, u32)>) {
        if entries.is_empty() {
            return;
        }
        self.ladder.clear();
        self.uid_to_pos.clear();
        for (uid, uname, level, fight_force, position, lead_htid) in entries {
            if position == 0 {
                continue;
            }
            let resolved_htid = if is_valid_hero_template_id(lead_htid) {
                lead_htid
            } else {
                Self::GUARD_HTIDS[((position - 1) as usize) % Self::GUARD_HTIDS.len()]
            };
            let comp = ArenaCompetitor {
                uid,
                uname,
                level: level.max(1),
                fight_force,
                position,
                is_npc: true,
                lead_htid: resolved_htid,
            };
            self.ladder.insert(position, comp);
            self.uid_to_pos.insert(uid, position);
        }
    }

    pub fn ensure_registered(&mut self, player: &PlayerState) -> u32 {
        let lead_htid = Self::resolve_player_lead_htid(player);
        if let Some(&pos) = self.uid_to_pos.get(&player.uid) {
            if let Some(entry) = self.ladder.get_mut(&pos) {
                entry.uname = player.uname.clone();
                entry.level = player.level;
                entry.fight_force = player.fight_force();
                entry.lead_htid = lead_htid;
            }
            self.history_min_pos
                .entry(player.uid)
                .and_modify(|m| *m = (*m).min(pos))
                .or_insert(pos);
            return pos;
        }
        let next_pos = self.ladder.keys().copied().max().unwrap_or(0) + 1;
        let comp = ArenaCompetitor {
            uid: player.uid,
            uname: player.uname.clone(),
            level: player.level,
            fight_force: player.fight_force(),
            position: next_pos,
            is_npc: false,
            lead_htid,
        };
        self.ladder.insert(next_pos, comp);
        self.uid_to_pos.insert(player.uid, next_pos);
        self.history_min_pos.insert(player.uid, next_pos);
        next_pos
    }

    fn top10_and_opponents(&self, my_pos: u32) -> (Vec<DtoValue>, Vec<DtoValue>) {
        let top10: Vec<DtoValue> = self
            .ladder
            .range(1..=10)
            .map(|(_, c)| c.to_dto())
            .collect();
        let start = my_pos.saturating_sub(4).max(1);
        let end = (my_pos + 2).max(start);
        let opponents: Vec<DtoValue> = self
            .ladder
            .range(start..=end)
            .map(|(_, c)| c.to_dto())
            .collect();
        (top10, opponents)
    }

    fn build_flop_draw_dto() -> DtoValue {
        DtoValue::map([
            ("real", DtoValue::map([("copper", DtoValue::from(5_000i64))])),
            ("show1", DtoValue::map([("gold", DtoValue::from(10i64))])),
            ("show2", DtoValue::map([("soul", DtoValue::from(50u64))])),
        ])
    }

    /// Returns `arena.enterArena` / `arena.getBaseData` / `arena.getArenaInfo` with `reward_time`
    /// in both root and `res` so `ArenaProxy:fetchReward()` (`ArenaProxy.lua:226`) never crashes!
    pub fn enter_arena(&mut self, player: &PlayerState) -> DtoValue {
        let my_pos = self.ensure_registered(player);
        let hist_min = self
            .history_min_pos
            .get(&player.uid)
            .copied()
            .unwrap_or(my_pos);
        let (top10, opponents) = self.top10_and_opponents(my_pos);
        let used = self.attacks_used.get(&player.uid).copied().unwrap_or(0);
        let bought = self.attacks_bought.get(&player.uid).copied().unwrap_or(0);
        let wins = self.wins_today.get(&player.uid).copied().unwrap_or(0);
        let reward_time = self
            .last_reward_time
            .get(&player.uid)
            .copied()
            .unwrap_or(Self::DEFAULT_REWARD_TIME);
        let num_reward: Vec<DtoValue> = self
            .claimed_num_rewards
            .get(&player.uid)
            .map(|s| s.iter().copied().map(DtoValue::from).collect())
            .unwrap_or_default();

        DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("is_have_challeged", DtoValue::from(u32::from(used > 0))),
            ("position", DtoValue::from(my_pos)),
            ("history_min_position", DtoValue::from(hist_min)),
            ("attack_num", DtoValue::from(used)),
            ("buy_num", DtoValue::from(bought)),
            ("win_num", DtoValue::from(wins)),
            ("reward_time", DtoValue::from(reward_time)),
            ("num_reward", DtoValue::Array(num_reward)),
            (
                "res",
                DtoValue::map([
                    ("position", DtoValue::from(my_pos)),
                    ("history_min_position", DtoValue::from(hist_min)),
                    ("attack_num", DtoValue::from(used)),
                    ("buy_num", DtoValue::from(bought)),
                    ("win_num", DtoValue::from(wins)),
                    ("reward_time", DtoValue::from(reward_time)),
                    ("my_fightCD", DtoValue::from(0u32)),
                    ("reset_cdnum", DtoValue::from(0u32)),
                    ("refresh_num", DtoValue::from(0u32)),
                    ("top10", DtoValue::Array(top10)),
                    ("opponents", DtoValue::Array(opponents)),
                ]),
            ),
        ])
    }

    pub fn get_arena_info(&mut self, player: &PlayerState) -> DtoValue {
        self.enter_arena(player)
    }

    pub fn get_rank_list(&self) -> DtoValue {
        DtoValue::Array(self.ladder.range(1..=50).map(|(_, c)| c.to_dto()).collect())
    }

    /// Executes `arena.challenge`, swapping ranks on win and returning `brid`, `top10`, `opponents`,
    /// `draw` / `flop` (3-card flop), `reward`, `serverData`, and `battleRecord` / `fightRet`!
    #[allow(clippy::type_complexity)]
    pub fn challenge(
        &mut self,
        player: &mut PlayerState,
        target_pos: u32,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>, Vec<(u64, PushPacket)>), &'static str> {
        let my_pos = self.ensure_registered(player);
        if my_pos == target_pos {
            return Err("position_err");
        }
        let Some(defender) = self.ladder.get(&target_pos).cloned() else {
            return Err("opponents_err");
        };

        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let def_htid = if defender.lead_htid > 0 {
            defender.lead_htid
        } else {
            310
        };
        let t2 = vec![CombatUnit::monster(
            defender.uid.max(800_001),
            def_htid,
            0,
            defender.level,
        )];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(
            defender.uid,
            &defender.uname,
            defender.level,
            0,
            defender.fight_force,
            0,
        );
        let sim = simulate_battle(brid, true, &t1, &t2, u1, u2, &[]);
        let report_blob = sim.to_amf3_report_bytes();
        let server_data_dto = sim.server_attack_dto.clone();
        let won = sim.is_win;
        copy_engine.record_battle(sim);

        *self.attacks_used.entry(player.uid).or_insert(0) += 1;
        if won {
            *self.wins_today.entry(player.uid).or_insert(0) += 1;
        }

        let mut peer_pushes = Vec::new();
        let mut new_pos = my_pos;
        if won && target_pos < my_pos {
            new_pos = target_pos;
            let mut my_entry = self.ladder.remove(&my_pos).unwrap();
            let mut def_entry = self.ladder.remove(&target_pos).unwrap();
            my_entry.position = target_pos;
            def_entry.position = my_pos;
            self.uid_to_pos.insert(player.uid, target_pos);
            self.uid_to_pos.insert(def_entry.uid, my_pos);
            self.history_min_pos
                .entry(player.uid)
                .and_modify(|m| *m = (*m).min(target_pos))
                .or_insert(target_pos);
            let def_uid = def_entry.uid;
            let def_is_npc = def_entry.is_npc;
            self.ladder.insert(target_pos, my_entry);
            self.ladder.insert(my_pos, def_entry);

            if !def_is_npc {
                let (def_top10, def_opps) = self.top10_and_opponents(my_pos);
                peer_pushes.push((
                    def_uid,
                    PushPacket::new(
                        push::ARENA_DATA_REFRESH,
                        DtoValue::map([
                            ("position", DtoValue::from(my_pos)),
                            ("top10", DtoValue::Array(def_top10)),
                            ("opponents", DtoValue::Array(def_opps)),
                        ]),
                        now_ts,
                    ),
                ));
            }
        }

        let coin_gain = if won { 50u64 } else { 25u64 };
        let rewards = [RewardEntry::ArenaCoin(coin_gain), RewardEntry::Copper(5_000)];
        let self_pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        let (top10, opponents) = self.top10_and_opponents(new_pos);
        let draw = Self::build_flop_draw_dto();

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("brid", DtoValue::from(brid)),
            ("result", DtoValue::from(if won { 1u32 } else { 0u32 })),
            ("position", DtoValue::from(new_pos)),
            ("rank", DtoValue::from(new_pos)),
            ("rank_change", DtoValue::from(my_pos.saturating_sub(new_pos))),
            ("oppo_fight_force", DtoValue::from(defender.fight_force)),
            ("top10", DtoValue::Array(top10)),
            ("opponents", DtoValue::Array(opponents)),
            ("draw", draw.clone()),
            ("flop", draw),
            ("reward", build_copy_drop_dto(&rewards)),
            ("serverData", server_data_dto),
            ("battleRecord", DtoValue::Bytes(report_blob.clone())),
            ("fightRet", DtoValue::Bytes(report_blob)),
        ]);
        Ok((ret, self_pushes, peer_pushes))
    }

    /// Executes `arena.quickChallenge` / sweep (`ArenaProxy:onQuickChallenge` at `ArenaProxy.lua:137-166`),
    /// returning `{ ret = "ok", res = { result = 1, draw = ... }, rounds = [...] }`!
    pub fn sweep(
        &mut self,
        player: &mut PlayerState,
        target_pos: u32,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let my_pos = self.ensure_registered(player);
        if my_pos == target_pos {
            return Err("position_err");
        }
        if !self.ladder.contains_key(&target_pos) {
            return Err("opponents_err");
        }
        let n = count.clamp(1, 5);
        *self.attacks_used.entry(player.uid).or_insert(0) += n;
        *self.wins_today.entry(player.uid).or_insert(0) += n;

        let draw = Self::build_flop_draw_dto();
        let mut rounds = Vec::with_capacity(n as usize);
        for _ in 0..n {
            rounds.push(DtoValue::map([
                ("result", DtoValue::from(1u32)),
                ("draw", draw.clone()),
            ]));
        }
        let rewards = [
            RewardEntry::ArenaCoin(50 * u64::from(n)),
            RewardEntry::Copper(5_000 * i64::from(n)),
        ];
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            (
                "res",
                DtoValue::map([
                    ("result", DtoValue::from(1u32)),
                    ("draw", draw),
                    ("rounds", DtoValue::Array(rounds.clone())),
                ]),
            ),
            ("rounds", DtoValue::Array(rounds)),
        ]);
        Ok((ret, pushes))
    }

    /// Buys extra Arena challenge attempts (`arena.buyChallengeTimes`), syncing `self.attacks_bought`!
    pub fn buy_atk_num(
        &mut self,
        player: &mut PlayerState,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count == 0 {
            return Err("invalid_count");
        }
        let cost = i64::from(count) * 20;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let bought = self.attacks_bought.entry(player.uid).or_insert(0);
        *bought += count;
        let cur_bought = *bought;
        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("buy_num", DtoValue::from(cur_bought)),
            ]),
            pushes,
        ))
    }

    /// Claims daily rank reward (`arena.fetchReward`, `ArenaProxy:onFetchReward`).
    pub fn receive_rank_reward(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let my_pos = self.ensure_registered(player);
        let coin_amt = (300u64).saturating_sub(u64::from(my_pos) * 5).max(50);
        let gold_amt = if my_pos <= 10 { 100i64 } else { 30i64 };
        self.last_reward_time.insert(player.uid, now_ts);
        let rewards = [
            RewardEntry::ArenaCoin(coin_amt),
            RewardEntry::Gold(gold_amt),
            RewardEntry::Copper(50_000),
        ];
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        let reward_triplets = DtoValue::Array(
            rewards
                .iter()
                .copied()
                .map(RewardEntry::to_triplet_dto)
                .collect(),
        );
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("reward_time", DtoValue::from(now_ts)),
                ("rewards", reward_triplets),
            ]),
            pushes,
        ))
    }

    /// Claims challenge-count milestone box (`arena.fetchChallengeNumReward`).
    pub fn fetch_challenge_num_reward(
        &mut self,
        player: &mut PlayerState,
        reward_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if reward_id == 0 {
            return Err("invalid_reward_id");
        }
        if !self
            .claimed_num_rewards
            .entry(player.uid)
            .or_default()
            .insert(reward_id)
        {
            return Err("already_claimed");
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::ArenaCoin(100), RewardEntry::Copper(20_000)],
            now_ts,
        )?;
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    // --- Time Machine Arena (`timeMachineArena.*`, `TmArenaData.lua` & `TmArenaProxy.lua`) ---

    pub fn tm_get_info(&mut self, player: &PlayerState) -> DtoValue {
        let _ = self.ensure_registered(player);
        let top_info: Vec<DtoValue> = self
            .ladder
            .range(1..=3)
            .map(|(&rank, c)| {
                DtoValue::map([
                    ("rank", DtoValue::from(rank)),
                    ("server_id", DtoValue::from(player.server_id.as_str())),
                    ("pid", DtoValue::from(format!("pid_{}", c.uid))),
                    ("utid", DtoValue::from(c.lead_htid.max(1))),
                    ("uname", DtoValue::from(c.uname.as_str())),
                    ("level", DtoValue::from(c.level)),
                    ("fight_force", DtoValue::from(c.fight_force)),
                    ("dress_id", DtoValue::from(0u32)),
                    ("avatar_id", DtoValue::from(c.lead_htid.max(1))),
                    ("quality", DtoValue::from(5u32)),
                    ("title_id", DtoValue::from(0u32)),
                    ("guild_name", DtoValue::from("")),
                ])
            })
            .collect();
        let atk_num = self.tm_atk_num.get(&player.uid).copied().unwrap_or(0);
        let is_rec_day = self.tm_rec_day.get(&player.uid).copied().unwrap_or(0);
        let is_rec_clg = self
            .tm_rec_challenge
            .get(&player.uid)
            .copied()
            .unwrap_or(0);

        DtoValue::map([
            ("err", DtoValue::from("ok")),
            ("atkNum", DtoValue::from(atk_num)),
            ("curStage", DtoValue::from(1u32)),
            ("isRecDay", DtoValue::from(is_rec_day)),
            ("isRecChallenge", DtoValue::from(is_rec_clg)),
            ("rankInfo", DtoValue::Array(Vec::new())),
            ("topInfo", DtoValue::Array(top_info)),
        ])
    }

    pub fn tm_challenge(
        &mut self,
        player: &mut PlayerState,
        copy_engine: &mut CopyCampaignEngine,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let brid = copy_engine.alloc_brid();
        let t1 = extract_player_combat_units(player);
        let t2 = vec![CombatUnit::monster(890_001, 401, 0, player.level.max(1))];
        let u1 = build_user_battle_dto(
            player.uid,
            &player.uname,
            player.level,
            player.vip,
            player.fight_force(),
            player.dress_id,
        );
        let u2 = build_user_battle_dto(1, "Lôi Chủ Thời Không", player.level.max(1), 0, 8_000, 0);
        let sim = simulate_battle(brid, true, &t1, &t2, u1, u2, &[]);
        let report_blob = sim.to_amf3_report_bytes();
        let is_win = sim.is_win;
        copy_engine.record_battle(sim);

        *self.tm_atk_num.entry(player.uid).or_insert(0) += 1;
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::ArenaCoin(60), RewardEntry::Copper(10_000)],
            now_ts,
        )?;
        Ok((
            DtoValue::map([
                ("err", DtoValue::from("ok")),
                ("isWin", DtoValue::from(is_win)),
                ("brid", DtoValue::from(brid)),
                ("battleRecord", DtoValue::Bytes(report_blob)),
            ]),
            pushes,
        ))
    }

    pub fn tm_rec_today_reward(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if self.tm_rec_day.get(&player.uid).copied() == Some(1) {
            return Ok((DtoValue::from("received"), Vec::new()));
        }
        self.tm_rec_day.insert(player.uid, 1);
        let pushes = grant_rewards_to_player(player, &[RewardEntry::Gold(50)], now_ts)?;
        Ok((DtoValue::from("ok"), pushes))
    }

    pub fn tm_rec_challenge_reward(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if self.tm_rec_challenge.get(&player.uid).copied() == Some(1) {
            return Ok((DtoValue::from("received"), Vec::new()));
        }
        self.tm_rec_challenge.insert(player.uid, 1);
        let pushes = grant_rewards_to_player(player, &[RewardEntry::ArenaCoin(100)], now_ts)?;
        Ok((DtoValue::from("ok"), pushes))
    }
}

// ============================================================================
// Section 4.4 & 4.5: Sign-In, Super Check-In, Online, Daily Quests, Achievements & EatChicken
// ============================================================================

/// Daily Sign-In (`sign.*`, `superCheckin.*`) and Online Timed Rewards (`online.*`, `openOnline.*`).
#[derive(Debug, Clone, Default)]
pub struct SignAndOnlineService {
    sign_days: BTreeMap<u64, u32>,
    last_sign_day_bucket: BTreeMap<u64, u64>,
    last_sign_timestamps: BTreeMap<u64, u64>,
    last_sign_vip_claimed: BTreeMap<u64, bool>,
    acc_sign_claimed: BTreeMap<u64, BTreeSet<u32>>,
    recharge_sign_claimed: BTreeMap<u64, BTreeSet<u32>>,
    super_checkin_days: BTreeMap<u64, BTreeMap<u32, u32>>,
    super_checkin_acc: BTreeMap<u64, u32>,
    claimed_online_tiers: BTreeMap<u64, BTreeSet<u32>>,
}

impl SignAndOnlineService {
    /// Exact tiers from decrypted `online.db` (`activity_open_online` rows `1..=5`).
    pub fn online_tier_spec(tier_id: u32) -> Option<(u64, RewardEntry)> {
        match tier_id {
            1 => Some((300, RewardEntry::Item { tid: 17002, qty: 5 })),
            2 => Some((600, RewardEntry::Copper(150_000))),
            3 => Some((600, RewardEntry::Item { tid: 17002, qty: 5 })),
            4 => Some((900, RewardEntry::Item { tid: 22202, qty: 2 })),
            5 => Some((1200, RewardEntry::Item { tid: 22102, qty: 2 })),
            _ => None,
        }
    }

    /// Returns `sign.getInfo` / `tmSign.getInfo` fixing all 4 Sign-In bugs (`Section 4.4`):
    /// includes `level_sign = 0`, `last_sign_vip_multi`, `month_sign_num`, `va_sign`, and real `last_sign_time`!
    pub fn get_sign_info(&self, uid: u64, now_ts: u64) -> DtoValue {
        self.get_sign_info_with_recharge(uid, 0, now_ts)
    }

    pub fn get_sign_info_with_recharge(
        &self,
        uid: u64,
        recharge_nb: u64,
        now_ts: u64,
    ) -> DtoValue {
        let day_bucket = now_ts / 86_400;
        let signed_today = self.last_sign_day_bucket.get(&uid).copied() == Some(day_bucket);
        let sign_num = self.sign_days.get(&uid).copied().unwrap_or(0);
        let last_sign_ts = self.last_sign_timestamps.get(&uid).copied().unwrap_or(0);
        let vip_claimed = self.last_sign_vip_claimed.get(&uid).copied().unwrap_or(false);
        let vip_multi_int = if vip_claimed { 1u32 } else { 0u32 };

        let va_sign: Vec<DtoValue> = (1..=sign_num).map(DtoValue::from).collect();
        let mut va_recharge_sign = BTreeMap::new();
        for tier in [8u32, 40, 100] {
            if recharge_nb >= u64::from(tier) {
                va_recharge_sign.insert(tier.to_string(), DtoValue::from(false));
            }
        }
        if let Some(claimed) = self.recharge_sign_claimed.get(&uid) {
            for &tier in claimed {
                va_recharge_sign.insert(tier.to_string(), DtoValue::from(true));
            }
        }

        DtoValue::map([
            ("uid", DtoValue::from(uid)),
            ("sign_num", DtoValue::from(sign_num)),
            ("month_sign_num", DtoValue::from(sign_num)),
            ("signed_today", DtoValue::from(signed_today)),
            ("level_sign", DtoValue::from(0u32)),
            ("level_recharge", DtoValue::from(1u32)),
            ("last_sign_vip_multi", DtoValue::from(vip_multi_int)),
            ("vip_multi", DtoValue::from(vip_multi_int)),
            ("rechargeRmb", DtoValue::from(0u32)),
            ("today_gold_all", DtoValue::from(0u32)),
            ("va_sign", DtoValue::Array(va_sign)),
            ("va_recharge_sign", DtoValue::Map(va_recharge_sign)),
            ("recharge_sign_num", DtoValue::from(0u32)),
            ("sign_time", DtoValue::from(last_sign_ts)),
            ("last_sign_time", DtoValue::from(last_sign_ts)),
            ("last_recharge_sign_time", DtoValue::from(0u64)),
            ("vip_sign_time", DtoValue::from(0u64)),
        ])
    }

    /// Signs in today (`sign.signToday` / `sign.gainNormalSignReward`), emitting `vip_multi = false`
    /// so `SignInProxy.lua` & `ActivitySignInProxy.lua` (`if b.vip_multi == false`) never double-render normal rewards!
    pub fn sign_today(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let day_bucket = now_ts / 86_400;
        if self.last_sign_day_bucket.get(&player.uid).copied() == Some(day_bucket) {
            return Err("already_signed_today");
        }
        let days = self.sign_days.entry(player.uid).or_insert(0);
        *days += 1;
        let cur_day = *days;
        self.last_sign_day_bucket.insert(player.uid, day_bucket);
        self.last_sign_timestamps.insert(player.uid, now_ts);
        self.last_sign_vip_claimed.insert(player.uid, false);

        let rewards = [
            RewardEntry::Gold(50 + i64::from(cur_day) * 10),
            RewardEntry::Copper(50_000),
        ];
        let pushes = grant_rewards_to_player(player, &rewards, now_ts)?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("sign_num", DtoValue::from(cur_day)),
                ("month_sign_num", DtoValue::from(cur_day)),
                ("last_sign_time", DtoValue::from(now_ts)),
                ("vip_multi", DtoValue::from(false)),
            ]),
            pushes,
        ))
    }

    pub fn gain_normal_sign_reward(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.sign_today(player, now_ts)
    }

    /// Claims cumulative sign-in milestone reward (`sign.gainAccSignReward`).
    pub fn gain_acc_sign_reward(
        &mut self,
        player: &mut PlayerState,
        req_days: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let cur_days = self.sign_days.get(&player.uid).copied().unwrap_or(0);
        if req_days == 0 || cur_days < req_days {
            return Err("sign_days_not_enough");
        }
        if !self
            .acc_sign_claimed
            .entry(player.uid)
            .or_default()
            .insert(req_days)
        {
            return Err("acc_reward_already_claimed");
        }
        let pushes = grant_rewards_to_player(
            player,
            &[
                RewardEntry::Gold(100),
                RewardEntry::Item { tid: 16012, qty: 1 },
            ],
            now_ts,
        )?;
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    /// Claims VIP double bonus after signing today (`sign.gainVipSignReward`).
    pub fn gain_vip_sign_reward(
        &mut self,
        player: &mut PlayerState,
        req_vip: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let day_bucket = now_ts / 86_400;
        if self.last_sign_day_bucket.get(&player.uid).copied() != Some(day_bucket) {
            return Err("not_signed_today");
        }
        if player.vip < req_vip.max(1) {
            return Err("vip_not_enough");
        }
        if self
            .last_sign_vip_claimed
            .get(&player.uid)
            .copied()
            .unwrap_or(false)
        {
            return Err("vip_bonus_already_claimed");
        }
        self.last_sign_vip_claimed.insert(player.uid, true);
        let cur_day = self.sign_days.get(&player.uid).copied().unwrap_or(1);
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(50 + i64::from(cur_day) * 10)],
            now_ts,
        )?;
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    /// Claims luxury recharge sign-in reward (`sign.gainRechargeSignReward`).
    pub fn gain_recharge_sign_reward(
        &mut self,
        player: &mut PlayerState,
        charge_tier: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if charge_tier == 0 {
            return Err("invalid_charge_tier");
        }
        if !self
            .recharge_sign_claimed
            .entry(player.uid)
            .or_default()
            .insert(charge_tier)
        {
            return Err("already_claimed");
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(100), RewardEntry::Item { tid: 22102, qty: 2 }],
            now_ts,
        )?;
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    // --- Super Check-In (`superCheckin.*`, `SuperCheckInData.lua` & `SuperCheckInProxy.lua`) ---

    pub fn get_super_checkin_info(&self, uid: u64, now_ts: u64) -> DtoValue {
        let cur_day = (((now_ts / 86_400) % 30) + 1) as u32;
        let acc_num = self.super_checkin_acc.get(&uid).copied().unwrap_or(0);
        let mut va_sign = BTreeMap::new();
        if let Some(days) = self.super_checkin_days.get(&uid) {
            for (&d, &state) in days {
                va_sign.insert(d.to_string(), DtoValue::from(state));
            }
        }
        DtoValue::map([
            ("cur_day", DtoValue::from(cur_day)),
            ("acc_num", DtoValue::from(acc_num)),
            ("va_sign", DtoValue::Map(va_sign)),
            ("va_reward", DtoValue::Array(Vec::new())),
        ])
    }

    pub fn super_checkin_sign(
        &mut self,
        player: &mut PlayerState,
        day: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !(1..=30).contains(&day) {
            return Err("invalid_day");
        }
        let cur_day = (((now_ts / 86_400) % 30) + 1) as u32;
        let days = self.super_checkin_days.entry(player.uid).or_default();
        if days.contains_key(&day) {
            return Err("already_checked_in");
        }
        let mut pushes = Vec::new();
        let state = if day < cur_day {
            if player.gold < 20 {
                return Err("not_enough_gold");
            }
            player.gold -= 20;
            pushes.push(PlayerState::build_user_update_push(
                [("gold", DtoValue::from(player.gold))],
                now_ts,
            ));
            2u32 // complement sign
        } else {
            1u32 // normal sign
        };
        days.insert(day, state);
        pushes.extend(grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(50), RewardEntry::Copper(30_000)],
            now_ts,
        )?);
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    pub fn super_checkin_fetch_reward(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let acc = self.super_checkin_acc.entry(player.uid).or_insert(0);
        *acc += 1;
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(150), RewardEntry::Item { tid: 16012, qty: 1 }],
            now_ts,
        )?;
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    // --- Online Rewards (`online.*` & `openOnline.*`, `ActOnlineData.lua` & `OnlineOpenProxy.lua`) ---

    /// Returns `{ step, begin_time, va_fetched = [], online_time, received = [], count_down }`
    /// so `#self.va_fetched` in `ActOnlineData.lua:69` NEVER throws a nil-length crash!
    pub fn get_online_info(&self, player: &PlayerState) -> DtoValue {
        let claimed_set = self.claimed_online_tiers.get(&player.uid);
        let claimed: Vec<DtoValue> = claimed_set
            .map(|s| s.iter().copied().map(DtoValue::from).collect())
            .unwrap_or_default();
        let step = claimed.len() as u32;
        let next_tier_secs = Self::online_tier_spec(step + 1).map_or(0, |(secs, _)| secs);
        let count_down = next_tier_secs.saturating_sub(player.online_accum_secs);

        DtoValue::map([
            ("step", DtoValue::from(step)),
            ("begin_time", DtoValue::from(player.execution_time)),
            ("last_login_time", DtoValue::from(player.execution_time)),
            ("total_online_time", DtoValue::from(player.online_accum_secs)),
            ("accum_online_secs", DtoValue::from(player.online_accum_secs)),
            ("online_time", DtoValue::from(player.online_accum_secs)),
            ("count_down", DtoValue::from(count_down)),
            ("va_fetched", DtoValue::Array(claimed.clone())),
            ("claimed_ids", DtoValue::Array(claimed.clone())),
            ("received", DtoValue::Array(claimed)),
        ])
    }

    pub fn fetch_online_reward(
        &mut self,
        player: &mut PlayerState,
        tier_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let (req_secs, entry) = Self::online_tier_spec(tier_id).ok_or("invalid_online_tier")?;
        if player.online_accum_secs < req_secs {
            return Err("online_time_not_enough");
        }
        let claimed = self.claimed_online_tiers.entry(player.uid).or_default();
        if claimed.contains(&tier_id) {
            return Err("online_tier_already_claimed");
        }
        claimed.insert(tier_id);
        let pushes = grant_rewards_to_player(player, &[entry], now_ts)?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("id", DtoValue::from(tier_id)),
            ]),
            pushes,
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct DailyTaskProgress {
    /// `0` = in progress, `1` = claimable, `2` = completed/claimed (`TaskConsts`).
    pub status: u32,
    pub num: u32,
    pub target: u32,
    pub score: u32,
}

/// Daily Quests & Main Tasks Engine (`DayTaskData.lua`, `TaskData.lua`, `TaskProxy.lua`, `dailytask.*`, `task.*`).
#[derive(Debug, Clone, Default)]
pub struct DailyTaskEngine {
    tasks: BTreeMap<u64, BTreeMap<u32, DailyTaskProgress>>,
    active_points: BTreeMap<u64, u32>,
    claimed_score_boxes: BTreeMap<u64, BTreeSet<u32>>,
    completed_main_tasks: BTreeMap<u64, BTreeSet<u32>>,
}

impl DailyTaskEngine {
    fn ensure_seeded(&mut self, uid: u64) {
        let map = self.tasks.entry(uid).or_default();
        if map.is_empty() {
            // Seed starter daily quests (`1`: Clear 1 Copy, `2`: Tavern Recruit 1, `3`: Arena 1)
            map.insert(
                1,
                DailyTaskProgress {
                    status: 1,
                    num: 1,
                    target: 1,
                    score: 20,
                },
            );
            map.insert(
                2,
                DailyTaskProgress {
                    status: 1,
                    num: 1,
                    target: 1,
                    score: 20,
                },
            );
            map.insert(
                3,
                DailyTaskProgress {
                    status: 0,
                    num: 0,
                    target: 1,
                    score: 20,
                },
            );
        }
    }

    pub fn record_progress(&mut self, uid: u64, task_id: u32, add_num: u32, target: u32) {
        self.ensure_seeded(uid);
        let entry = self
            .tasks
            .entry(uid)
            .or_default()
            .entry(task_id)
            .or_insert(DailyTaskProgress {
                status: 0,
                num: 0,
                target: target.max(1),
                score: 20,
            });
        if entry.status < 2 {
            entry.num = entry.num.saturating_add(add_num);
            if entry.num >= entry.target.max(1) {
                entry.status = 1;
            }
        }
    }

    /// Returns `dailytask.getInfo` / `tmDailytask.getInfo` matching `DayTaskData:parseData` (`DayTaskData.lua:95-125`).
    pub fn get_info(&mut self, player: &PlayerState) -> DtoValue {
        self.ensure_seeded(player.uid);
        let point = self.active_points.get(&player.uid).copied().unwrap_or(0);
        let mut dailytask_map = BTreeMap::new();
        let mut va_active_task = BTreeMap::new();
        for (&tid, prog) in &self.tasks[&player.uid] {
            dailytask_map.insert(
                tid.to_string(),
                DtoValue::map([
                    ("status", DtoValue::from(prog.status)),
                    ("num", DtoValue::from(prog.num)),
                ]),
            );
            va_active_task.insert(tid.to_string(), DtoValue::from(prog.num));
        }
        let mut va_receive_point = BTreeMap::new();
        let mut va_active_reward = Vec::new();
        if let Some(boxes) = self.claimed_score_boxes.get(&player.uid) {
            for &sc in boxes {
                va_receive_point.insert(sc.to_string(), DtoValue::from(1u32));
                va_active_reward.push(DtoValue::from(sc));
            }
        }
        DtoValue::map([
            ("point", DtoValue::from(point)),
            ("active_point", DtoValue::from(point)),
            ("super_checkin_score", DtoValue::from(point)),
            ("level", DtoValue::from(player.level.max(1))),
            ("va_receive_point", DtoValue::Map(va_receive_point)),
            (
                "va_active",
                DtoValue::map([
                    ("task", DtoValue::Map(va_active_task)),
                    ("reward", DtoValue::Array(va_active_reward)),
                ]),
            ),
            ("dailytask", DtoValue::Map(dailytask_map)),
        ])
    }

    /// Claims a completed daily quest (`dailytask.receiveTask` / `CS_DAYTASK_COMPLETE`, `args = [taskId]`).
    pub fn receive_task(
        &mut self,
        player: &mut PlayerState,
        task_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.ensure_seeded(player.uid);
        let prog = self
            .tasks
            .get_mut(&player.uid)
            .and_then(|m| m.get_mut(&task_id))
            .ok_or("task_not_found")?;
        if prog.status == 2 {
            return Err("task_already_claimed");
        }
        if prog.num < prog.target {
            return Err("task_not_finished");
        }
        prog.status = 2;
        let gained_score = prog.score.max(20);
        let new_pts = {
            let p = self.active_points.entry(player.uid).or_insert(0);
            *p += gained_score;
            *p
        };
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(20), RewardEntry::Copper(20_000)],
            now_ts,
        )?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("active_point", DtoValue::from(new_pts)),
            ]),
            pushes,
        ))
    }

    /// Claims an active-point chest (`dailytask.receiveActivePoint`, `args = [scoreThreshold]`).
    pub fn receive_active_point(
        &mut self,
        player: &mut PlayerState,
        score_threshold: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let cur_pts = self.active_points.get(&player.uid).copied().unwrap_or(0);
        if score_threshold == 0 || cur_pts < score_threshold {
            return Err("active_point_not_enough");
        }
        if !self
            .claimed_score_boxes
            .entry(player.uid)
            .or_default()
            .insert(score_threshold)
        {
            return Err("box_already_claimed");
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(50), RewardEntry::Item { tid: 22102, qty: 1 }],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    /// One-key claims all finished daily quests and eligible score boxes (`dailytask.receiveAllTask`).
    pub fn receive_all_task(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.ensure_seeded(player.uid);
        let mut claimed_tasks = 0u32;
        let mut added_score = 0u32;
        if let Some(map) = self.tasks.get_mut(&player.uid) {
            for prog in map.values_mut() {
                if prog.status < 2 && prog.num >= prog.target {
                    prog.status = 2;
                    claimed_tasks += 1;
                    added_score += prog.score.max(20);
                }
            }
        }
        let total_pts = {
            let p = self.active_points.entry(player.uid).or_insert(0);
            *p += added_score;
            *p
        };
        let mut claimed_boxes = 0u32;
        let boxes = self.claimed_score_boxes.entry(player.uid).or_default();
        for step in [20u32, 40, 60, 80, 100, 120] {
            if total_pts >= step && boxes.insert(step) {
                claimed_boxes += 1;
            }
        }
        let total_gold = i64::from(claimed_tasks * 20 + claimed_boxes * 50);
        let pushes = if total_gold > 0 {
            grant_rewards_to_player(
                player,
                &[
                    RewardEntry::Gold(total_gold),
                    RewardEntry::Copper(i64::from(claimed_tasks) * 20_000),
                ],
                now_ts,
            )?
        } else {
            Vec::new()
        };
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("active_point", DtoValue::from(total_pts)),
            ]),
            pushes,
        ))
    }

    /// Returns `task.getAllTask` / `tmTask.getAllTask` (`TaskData:parseData`).
    pub fn get_all_main_tasks(&self, uid: u64) -> DtoValue {
        let completed: Vec<DtoValue> = self
            .completed_main_tasks
            .get(&uid)
            .map(|s| s.iter().copied().map(DtoValue::from).collect())
            .unwrap_or_default();
        let _ = completed;
        DtoValue::Map(BTreeMap::new())
    }

    pub fn complete_main_task(
        &mut self,
        player: &mut PlayerState,
        task_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if task_id == 0 {
            return Err("invalid_task_id");
        }
        if !self
            .completed_main_tasks
            .entry(player.uid)
            .or_default()
            .insert(task_id)
        {
            return Err("task_already_completed");
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(50), RewardEntry::Copper(25_000)],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }
}

/// Achievements Engine (`AchievementData.lua`, `AchievementProxy.lua`, `achievements.*` / `tmAchievements.*`).
#[derive(Debug, Clone, Default)]
pub struct AchievementEngine {
    /// `uid -> (achieve_id -> (progress, reward_send))`
    progress: BTreeMap<u64, BTreeMap<u32, (u32, u32)>>,
    points: BTreeMap<u64, u32>,
    claimed_boxes: BTreeMap<u64, BTreeSet<u32>>,
}

impl AchievementEngine {
    pub fn record_achievement(&mut self, uid: u64, achieve_id: u32, progress_val: u32) {
        let entry = self
            .progress
            .entry(uid)
            .or_default()
            .entry(achieve_id)
            .or_insert((0, 0));
        entry.0 = entry.0.max(progress_val);
    }

    /// Returns `achievements.getShowAchievements` (`AchievementData:parseData`).
    pub fn get_show_achievements(&mut self, uid: u64) -> DtoValue {
        let map = self.progress.entry(uid).or_default();
        if map.is_empty() {
            // Seed with a valid achievement.db ID (100001); ID 1 does not exist in achievement.db.
            map.insert(100_001, (1, 0));
        }
        let achievements_arr: Vec<DtoValue> = map
            .iter()
            .map(|(&aid, &(data, reward_send))| {
                DtoValue::map([
                    ("achieve_id", DtoValue::from(aid)),
                    ("data", DtoValue::from(data)),
                    ("reward_send", DtoValue::from(reward_send)),
                ])
            })
            .collect();
        let have_receive: Vec<DtoValue> = self
            .claimed_boxes
            .get(&uid)
            .map(|s| s.iter().copied().map(DtoValue::from).collect())
            .unwrap_or_default();
        let point = self.points.get(&uid).copied().unwrap_or(0);

        DtoValue::map([
            ("point", DtoValue::from(point)),
            ("achievements", DtoValue::Array(achievements_arr)),
            ("have_receive", DtoValue::Array(have_receive)),
        ])
    }

    pub fn get_point_reward_info(&self, uid: u64) -> DtoValue {
        let have_receive: Vec<DtoValue> = self
            .claimed_boxes
            .get(&uid)
            .map(|s| s.iter().copied().map(DtoValue::from).collect())
            .unwrap_or_default();
        DtoValue::Array(have_receive)
    }

    pub fn receive_achievement_prize(
        &mut self,
        player: &mut PlayerState,
        achieve_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if achieve_id == 0 {
            return Err("invalid_achieve_id");
        }
        let entry = self
            .progress
            .entry(player.uid)
            .or_default()
            .entry(achieve_id)
            .or_insert((1, 0));
        if entry.1 == 1 {
            return Err("achievement_already_claimed");
        }
        entry.1 = 1;
        *self.points.entry(player.uid).or_insert(0) += 10;
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(100), RewardEntry::Copper(50_000)],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }

    pub fn receive_achievement_point_prize(
        &mut self,
        player: &mut PlayerState,
        box_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if box_id == 0 {
            return Err("invalid_box_id");
        }
        if !self
            .claimed_boxes
            .entry(player.uid)
            .or_default()
            .insert(box_id)
        {
            return Err("box_already_claimed");
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Gold(150), RewardEntry::Item { tid: 16012, qty: 2 }],
            now_ts,
        )?;
        Ok((DtoValue::from("ok"), pushes))
    }
}

/// Stamina Supply Banquet Service (`eatChicken.*` / `tmEatChicken.*` — `ActChickenData.lua`, `ActivityChickenProxy.lua`, `chicken.json`).
#[derive(Debug, Clone, Default)]
pub struct EatChickenService {
    /// Tracks `(uid, day_bucket, window_slot)` already claimed.
    eaten_slots: BTreeSet<(u64, u64, u32)>,
}

impl EatChickenService {
    pub const EXEC_GAIN: u32 = 50;
    pub const ENERGY_GAIN: u32 = 10;
    pub const BONUS_GOLD: i64 = 50;

    /// Maps a Unix timestamp to banquet window `1` (`12..14h`), `2` (`18..20h`), `3` (`21..23h`), or `0` (fallback).
    pub fn window_for_timestamp(now_ts: u64) -> u32 {
        let sec_of_day = now_ts % 86_400;
        let hour = sec_of_day / 3_600;
        match hour {
            12..=13 => 1,
            18..=19 => 2,
            21..=22 => 3,
            _ => 0,
        }
    }

    /// Returns `eatChicken.getInfo` (`ActChickenData:parseData`: `self.isEat = data` boolean — NEVER `{}`!).
    pub fn get_info(&self, uid: u64, now_ts: u64) -> DtoValue {
        let day_bucket = now_ts / 86_400;
        let slot = Self::window_for_timestamp(now_ts).max(1);
        let can_eat = !self.eaten_slots.contains(&(uid, day_bucket, slot));
        DtoValue::Bool(can_eat)
    }

    /// Claims stamina supply (`eatChicken.eat`, `ActivityChickenProxy:onEat`), granting `+50` execution,
    /// `+10` vitality/energy, and `+50` gold drop per `chicken.json`!
    pub fn eat_chicken(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let day_bucket = now_ts / 86_400;
        let slot = Self::window_for_timestamp(now_ts).max(1);
        if !self.eaten_slots.insert((player.uid, day_bucket, slot)) {
            return Err("already_eaten");
        }

        player.execution = player.execution.saturating_add(Self::EXEC_GAIN);
        player.execution_time = now_ts;
        player.energy = player.energy.saturating_add(Self::ENERGY_GAIN);
        player.energy_time = now_ts;
        player.gold = player.gold.saturating_add(Self::BONUS_GOLD);

        let pushes = vec![PlayerState::build_user_update_push(
            [
                ("execution", DtoValue::from(player.execution)),
                ("execution_time", DtoValue::from(player.execution_time)),
                ("energy", DtoValue::from(player.energy)),
                ("energy_time", DtoValue::from(player.energy_time)),
                ("gold", DtoValue::from(player.gold)),
            ],
            now_ts,
        )];

        let drop = DtoValue::map([
            ("execution", DtoValue::from(Self::EXEC_GAIN)),
            ("energy", DtoValue::from(Self::ENERGY_GAIN)),
            ("gold", DtoValue::from(Self::BONUS_GOLD)),
        ]);
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("power", DtoValue::from(Self::EXEC_GAIN)),
            ("vit", DtoValue::from(Self::ENERGY_GAIN)),
            ("drop", drop),
        ]);
        Ok((ret, pushes))
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    const TS: u64 = 1_795_521_600; // 12:00 UTC

    #[test]
    fn zlib_amf3_roundtrip_and_battle_schema_invariants() {
        let p = PlayerState::new(1001, "ChuaCong", 1);
        let t1 = extract_player_combat_units(&p);
        // Legacy broken 5001 monster ID must auto-remap to >20000 valid monster.db ID!
        let m = CombatUnit::monster(9001, 5001, 0, 5);
        assert!(m.htid > 20_000, "monster htid must be > 20000 in monster.db");
        assert_eq!((m.cur_rage, m.max_rage, m.rage_cost), (50, 100, 100));

        let u1 = build_user_battle_dto(1001, "ChuaCong", 10, 2, 15_000, 0);
        let u2 = build_user_battle_dto(0, "HoangCan", 5, 0, 3_000, 0);
        let sim = simulate_battle(
            700_001,
            false,
            &t1,
            &[m],
            u1,
            u2,
            &[RewardEntry::Copper(1_000)],
        );

        // Phase 1 `serverData` checks
        let seeds = sim
            .server_attack_dto
            .get("arrSeed")
            .and_then(DtoValue::as_array)
            .expect("arrSeed non-empty array");
        assert!(!seeds.is_empty());
        assert!(seeds[0].as_u64().unwrap() > 0);

        let user_info = sim
            .server_attack_dto
            .get("userInfo")
            .and_then(DtoValue::as_map)
            .expect("userInfo MUST be string-keyed Map {'1', '2'}");
        assert!(user_info.contains_key("1") && user_info.contains_key("2"));

        // Phase 2 `fightReport` + Zlib+AMF3 blob roundtrip
        let zblob = sim.to_zlib_amf3_report_bytes();
        assert_eq!(zblob[0], 0x78, "RFC 1950 Zlib header magic");
        let decoded = decode_zlib_amf3(&zblob).expect("decode zlib+AMF3 fightReport");
        assert_eq!(decoded.get("appraisal").and_then(DtoValue::as_u32), Some(50));
        assert_eq!(decoded.get("result").and_then(DtoValue::as_u32), Some(1));
        assert!(decoded
            .get("heroPercentInfo")
            .and_then(DtoValue::as_array)
            .is_some());
        let stats = decoded
            .get("statisticalDataInfo")
            .and_then(DtoValue::as_map)
            .expect("statisticalDataInfo map");
        assert!(stats.get("team1").and_then(|t| t.get("info")).is_some());
        assert!(stats.get("team2").and_then(|t| t.get("info")).is_some());
    }

    #[test]
    fn normal_and_elite_copy_exact_db_unlock_and_sweep_array() {
        let mut p = PlayerState::new(1001, "CopyHero", 106);
        p.level = 30;
        p.execution = 500;
        p.roster.heroes.get_mut(&10_000_001).unwrap().level = 30;
        p.roster
            .heroes
            .get_mut(&10_000_001)
            .unwrap()
            .recalculate_stats();

        let mut engine = CopyCampaignEngine::new();
        // Stage 100104 does NOT exist in strongh.db!
        assert_eq!(
            engine.attack_stage(&p, 1001, 100_104, false).unwrap_err(),
            "invalid_copy_stage"
        );

        // Clearing 100105 (4th stage of 1001) MUST NOT prematurely unlock chapter 1002!
        let _ = engine.pass_stage(&mut p, 1001, 100_101, TS).unwrap();
        let _ = engine.pass_stage(&mut p, 1001, 100_105, TS).unwrap();
        let list_mid = engine.get_list(1001, 1);
        assert!(
            list_mid
                .get("copyData")
                .and_then(|c| c.get("1002"))
                .is_none(),
            "Chapter 1002 must NOT unlock before clearing final boss 100106"
        );

        // With 6 stars >= 5 (`score_require = [5, 10, 15]`), box 0 succeeds while box 1 (10) rejects
        assert_eq!(
            engine.fetch_chapter_prize(&mut p, 1001, 1, TS).unwrap_err(),
            "stars_not_enough"
        );
        assert!(engine.fetch_chapter_prize(&mut p, 1001, 0, TS).is_ok());

        // Clearing 100106 (Tả Từ) unlocks chapter 1002 AND Elite 2001 (level >= 26)
        let _ = engine.pass_stage(&mut p, 1001, 100_106, TS).unwrap();
        let list_after = engine.get_list(1001, 1);
        assert!(list_after
            .get("copyData")
            .and_then(|c| c.get("1002"))
            .is_some());

        // Sweep returns top-level 1-indexed Array for CopySweepPanel.lua:79
        let (sweep_ret, _) = engine.sweep_stage(&mut p, 1001, 100_101, 3, TS).unwrap();
        assert_eq!(sweep_ret.as_array().expect("top-level sweep array").len(), 3);

        // Elite Copy list (`copy_type = 2`) returns 2001+, never polluted with 1001!
        let elite_list = EliteCopyEngine::get_info(&mut engine, 1001);
        let edata = elite_list
            .get("copyData")
            .and_then(DtoValue::as_map)
            .unwrap();
        assert!(edata.contains_key("2001") && !edata.contains_key("1001"));
    }

    #[test]
    fn daily_copy_babel_dota_rebel_discover_devil_soaring_engines() {
        let mut p = PlayerState::new(1001, "DungeonTester", 201);
        p.level = 60;
        p.gold = 5_000;
        p.roster.heroes.get_mut(&10_000_001).unwrap().level = 60;
        p.roster
            .heroes
            .get_mut(&10_000_001)
            .unwrap()
            .recalculate_stats();
        let mut copy_eng = CopyCampaignEngine::new();

        // 1. Daily Copy
        let mut daily = DailyCopyEngine::default();
        let d_info = daily.get_info(1001, Some(1));
        assert!(d_info.get("pass").and_then(DtoValue::as_array).is_some());
        let _ = daily.attack(&p, 1, 1001, &mut copy_eng).unwrap();
        let _ = daily.pass(&mut p, 1, 1001, TS).unwrap();
        let _ = daily.sweep(&mut p, 1, 1001, TS).unwrap();
        let _ = daily.buy_atk_num(&mut p, 1, 1, TS).unwrap();

        // 2. Babel Tower
        let mut babel = BabelTowerEngine::default();
        let b_info = babel.get_babeltower_info(1001, TS);
        assert_eq!(
            b_info
                .get("floorInfo")
                .and_then(|f| f.get("addBuff"))
                .and_then(DtoValue::as_u32),
            Some(1001)
        );
        babel.set_idx(1001, 1, 1).unwrap();
        let (b_atk, _) = babel.attack(&mut p, 1, 1, &mut copy_eng, TS).unwrap();
        assert_eq!(b_atk.get("result").and_then(DtoValue::as_str), Some("success"));
        let next_fl = babel.pass_floor(1001, 1).unwrap();
        assert!(next_fl.get("npcInfo").is_some());
        let (ans, _) = babel.answer(&mut p, 2, 3, 1, TS).unwrap();
        assert_eq!(ans.get("result").and_then(DtoValue::as_str), Some("success"));
        let (auto_rw, _) = babel.auto_receive_reward(&mut p, 1, TS).unwrap();
        assert_eq!(
            auto_rw
                .get("receive")
                .and_then(DtoValue::as_array)
                .unwrap()
                .len(),
            2
        );

        // 3. Dota & Purgatory
        let mut dota = DotaTowerEngine::default();
        assert!(dota.get_info(1001).get("normal_layer").is_some());
        let _ = dota.attack(&p, 1, &mut copy_eng).unwrap();
        let _ = dota.pass(&mut p, 0, 1, true, TS).unwrap();
        let _ = dota.reset(&mut p, TS).unwrap();
        let _ = dota.sweep(&mut p, 0, TS).unwrap();

        // 4. Rebel (`BaseShopProxy` dependency!) + Boss + Fiend
        let mut boss_rebel = WorldBossAndRebelEngine::new();
        let r_info = boss_rebel.get_rebel_info(&p, TS);
        assert_eq!(r_info.get("level_record").and_then(DtoValue::as_u32), Some(60));
        assert!(r_info
            .get("damage_reward")
            .and_then(DtoValue::as_array)
            .is_some());
        assert!(r_info
            .get("self_rebel")
            .and_then(|sr| sr.get("va_reward"))
            .is_some());
        let _ = boss_rebel.attack_rebel(&mut p, 1, 1, &mut copy_eng, TS).unwrap();
        assert!(boss_rebel.get_boss_data(&p, TS).get("atker").is_some());
        assert!(boss_rebel.get_fiend_info(TS).get("fiend").is_some());

        // 5. AwakeCopy, Discover, DevilCopy, SoaringCopy
        let mut awake = AwakeCopyEngine::default();
        assert!(awake.get_info(1001).get("pass_str").is_some());
        let _ = awake.quick_battle(&mut p, 1, &mut copy_eng, TS).unwrap();

        let mut disc = DiscoverEngine::default();
        assert!(disc.get_discover(1001, 1).get("cur_hp").is_some());
        disc.patrol(1001, 1, TS).unwrap();
        let _ = disc.receive_patrol_reward(&mut p, 1, TS + 60).unwrap();

        let mut devil = DevilCopyEngine::default();
        assert!(devil.get_info(1001).get("va_copy").is_some());
        let _ = devil.pass(&mut p, 1, &mut copy_eng, TS).unwrap();

        let mut soaring = SoaringCopyEngine::default();
        assert!(soaring.get_info(1001).get("va_copy").is_some());
        let _ = soaring.pass(&mut p, 1, &mut copy_eng, TS).unwrap();
    }

    #[test]
    fn arena_signin_online_dailytask_achievement_and_eatchicken() {
        let mut p = PlayerState::new(1001, "ActivityTester", 201);
        p.level = 50;
        p.vip = 4;
        p.gold = 5_000;
        p.roster.heroes.get_mut(&10_000_001).unwrap().level = 50;
        p.roster
            .heroes
            .get_mut(&10_000_001)
            .unwrap()
            .recalculate_stats();
        let mut copy_eng = CopyCampaignEngine::new();

        // Arena: reward_time, challenge draw/flop + top10/opponents, sweep res, buy_atk_num sync, TM Arena
        let mut arena = ArenaEngine::new();
        let enter = arena.enter_arena(&p);
        assert!(enter
            .get("res")
            .and_then(|r| r.get("reward_time"))
            .and_then(DtoValue::as_u64)
            .is_some());
        let (ch_ret, _, _) = arena.challenge(&mut p, 15, &mut copy_eng, TS).unwrap();
        assert!(ch_ret.get("draw").is_some());
        assert!(ch_ret.get("top10").and_then(DtoValue::as_array).is_some());
        let (sw_ret, _) = arena.sweep(&mut p, 14, 3, TS).unwrap();
        assert_eq!(
            sw_ret
                .get("res")
                .and_then(|r| r.get("result"))
                .and_then(DtoValue::as_u32),
            Some(1)
        );
        let _ = arena.buy_atk_num(&mut p, 2, TS).unwrap();
        assert_eq!(
            arena
                .enter_arena(&p)
                .get("buy_num")
                .and_then(DtoValue::as_u32),
            Some(2)
        );
        assert_eq!(
            arena.tm_get_info(&p).get("err").and_then(DtoValue::as_str),
            Some("ok")
        );

        // Sign-In & SuperCheckin & Online
        let mut sign = SignAndOnlineService::default();
        let s0 = sign.get_sign_info(1001, TS);
        assert_eq!(s0.get("level_sign").and_then(DtoValue::as_u32), Some(0));
        assert_eq!(
            s0.get("last_sign_vip_multi").and_then(DtoValue::as_u32),
            Some(0)
        );
        let (s_today, _) = sign.sign_today(&mut p, TS).unwrap();
        assert_eq!(s_today.get("vip_multi").and_then(DtoValue::as_bool), Some(false));
        assert_eq!(
            sign.get_sign_info(1001, TS)
                .get("last_sign_time")
                .and_then(DtoValue::as_u64),
            Some(TS)
        );
        assert!(sign
            .get_super_checkin_info(1001, TS)
            .get("va_sign")
            .is_some());
        let onl = sign.get_online_info(&p);
        assert!(onl.get("va_fetched").and_then(DtoValue::as_array).is_some());

        // DailyTask, Achievement & EatChicken
        let mut dt = DailyTaskEngine::default();
        assert!(dt.get_info(&p).get("dailytask").is_some());
        let _ = dt.receive_task(&mut p, 1, TS).unwrap();
        let _ = dt.receive_active_point(&mut p, 20, TS).unwrap();

        let mut ach = AchievementEngine::default();
        assert!(ach.get_show_achievements(1001).get("achievements").is_some());
        let _ = ach.receive_achievement_prize(&mut p, 1, TS).unwrap();

        let mut chicken = EatChickenService::default();
        assert_eq!(chicken.get_info(1001, TS), DtoValue::Bool(true));
        let exec_before = p.execution;
        let (eat_ret, _) = chicken.eat_chicken(&mut p, TS).unwrap();
        assert_eq!(eat_ret.get("power").and_then(DtoValue::as_u32), Some(50));
        assert_eq!(p.execution, exec_before + 50);
        assert_eq!(chicken.get_info(1001, TS), DtoValue::Bool(false));
    }
}
