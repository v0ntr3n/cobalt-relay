//! Crash-proof Turn-Based Battle Simulator (`FightProxy.lua`, `FightOperateLogic.lua`, `BaseUnit.lua`, `PropertyKey.lua`),
//! PvE Campaign Copy (`CopyData.lua`, `CopyProxy.lua`, `CopyFightProxy.lua`),
//! Arena (`ArenaProxy.lua`), and Daily Sign-In / Online Timed Rewards (`ActivitySignInCell.lua`, `online.db`).

use std::collections::{BTreeMap, BTreeSet};

use crate::mail::{grant_rewards_to_player, RewardEntry};
use crate::player::{HeroInstance, PlayerState};
use crate::protocol::{encode_amf3, push, DtoValue, PushPacket};

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
    pub max_hp: i64,
    pub cur_hp: i64,
    pub atk: i64,
    pub pdef: i64,
    pub mdef: i64,
    pub speed: i64,
    pub cur_rage: i32,
    pub max_rage: i32,
    pub rage_cost: i32,
    pub normal_skill_id: u32,
    pub rage_skill_id: u32,
    pub damage_dealt: i64,
}

impl CombatUnit {
    pub fn from_hero(hero: &HeroInstance, position: u32) -> Self {
        Self {
            hid: hero.hid,
            htid: hero.htid,
            position: position.min(5),
            level: hero.level,
            country: hero.country,
            dress_id: hero.dress_id,
            max_hp: hero.max_hp.max(1),
            cur_hp: hero.cur_hp.clamp(1, hero.max_hp.max(1)),
            atk: hero.atk.max(10),
            pdef: hero.pdef.max(0),
            mdef: hero.mdef.max(0),
            speed: hero.speed.max(1),
            cur_rage: 2,
            max_rage: 4,
            rage_cost: 4,
            normal_skill_id: hero.normal_skill_id,
            rage_skill_id: hero.rage_skill_id,
            damage_dealt: 0,
        }
    }

    pub fn monster(hid: u64, htid: u32, position: u32, level: u32) -> Self {
        let lv = i64::from(level.max(1));
        let max_hp = 800 + lv * 260;
        Self {
            hid,
            htid,
            position: position.min(5),
            level: level.max(1),
            country: 4,
            dress_id: 0,
            max_hp,
            cur_hp: max_hp,
            atk: 140 + lv * 40,
            pdef: 60 + lv * 18,
            mdef: 60 + lv * 18,
            speed: 85 + lv,
            cur_rage: 1,
            max_rage: 4,
            rage_cost: 4,
            normal_skill_id: 1001,
            rage_skill_id: 2001,
            damage_dealt: 0,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.cur_hp > 0
    }

    /// Serializes a unit into the exact `BaseUnit:init(data)` + `PropertyKey.lua` schema
    /// so `UnitFactory:creatUnit` never hits nil attributes or missing skills.
    pub fn to_unit_dto(&self) -> DtoValue {
        DtoValue::map([
            ("hid", DtoValue::from(self.hid)),
            ("htid", DtoValue::from(self.htid)),
            ("position", DtoValue::from(self.position)),
            ("level", DtoValue::from(self.level)),
            ("maxHp", DtoValue::from(self.max_hp)),
            ("curHp", DtoValue::from(self.cur_hp)),
            ("country", DtoValue::from(self.country)),
            ("maxRage", DtoValue::from(self.max_rage)),
            ("curRage", DtoValue::from(self.cur_rage)),
            ("dress_id", DtoValue::from(self.dress_id)),
            ("awake_rank", DtoValue::from(0u32)),
            ("awake_level", DtoValue::from(0u32)),
            ("rage_cost", DtoValue::from(self.rage_cost)),
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
            // `27`=initRage, `28`=rageTop, `34`=hit=10000)
            ("1", DtoValue::from(self.max_hp)),
            ("2", DtoValue::from(self.atk)),
            ("3", DtoValue::from(self.atk)),
            ("4", DtoValue::from(self.pdef)),
            ("5", DtoValue::from(self.mdef)),
            ("9", DtoValue::from(self.atk)),
            ("10", DtoValue::from(self.speed)),
            ("27", DtoValue::from(self.cur_rage)),
            ("28", DtoValue::from(self.max_rage)),
            ("34", DtoValue::from(10_000i32)),
        ])
    }
}

/// Action log step recorded in `rageQueue` / battle replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BattleStep {
    pub round: u32,
    pub attacker_hid: u64,
    pub defender_hid: u64,
    pub skill_id: u32,
    pub is_rage_skill: bool,
    pub damage: i64,
    pub defender_hp_after: i64,
}

/// Complete battle simulation result and `FightProxy` report structure.
#[derive(Debug, Clone)]
pub struct SimulatedBattle {
    pub brid: u64,
    pub is_win: bool,
    /// `FightProxy:isSuccessFight()` checks `appraisal == 50`! (`50` = Win, `10` = Loss)
    pub appraisal: u32,
    pub stars: u32,
    pub rounds: u32,
    pub steps: Vec<BattleStep>,
    pub server_attack_dto: DtoValue,
    pub fight_report_dto: DtoValue,
}

impl SimulatedBattle {
    pub fn to_amf3_report_bytes(&self) -> Vec<u8> {
        encode_amf3(&self.fight_report_dto)
    }
}

/// Builds the crash-proof `copy.attack` / `serverData` DTO verified from `FightProxy:parseBattleInfo`
/// and `FightOperateLogic:startFight` (`brid`, `drop`, `battleInfo`, `userInfo`).
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
    let build_team_dto = |units: &[CombatUnit]| -> DtoValue {
        let mut arr_hero = BTreeMap::new();
        for (idx, u) in units.iter().enumerate() {
            arr_hero.insert((idx + 1).to_string(), u.to_unit_dto());
        }
        DtoValue::map([
            ("curRage", DtoValue::from(0u32)),
            ("rageMax", DtoValue::from(100u32)),
            ("morale", DtoValue::from(0u32)),
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
    arr_team2.insert("1".to_owned(), build_team_dto(team2_units));

    let battle_info = DtoValue::map([
        ("isPVP", DtoValue::from(if is_pvp { 1u32 } else { 0u32 })),
        ("arrTeam1", DtoValue::Map(arr_team1)),
        ("arrTeam2", DtoValue::Map(arr_team2)),
        ("extraMonster", DtoValue::Map(BTreeMap::new())),
        ("skillRate", DtoValue::Map(BTreeMap::new())),
    ]);

    let drop_dto = DtoValue::Array(
        drops
            .iter()
            .copied()
            .map(RewardEntry::to_triplet_dto)
            .collect(),
    );

    DtoValue::map([
        ("brid", DtoValue::from(brid)),
        ("drop", drop_dto),
        ("battleInfo", battle_info),
        ("userInfo", DtoValue::array([user1_dto, user2_dto])),
    ])
}

/// Runs a deterministic turn-based battle (`round_limit = 20`) and constructs both the
/// `copy.attack` `serverData` DTO and the `FightProxy:decodeFightReport` DTO (`appraisal = 50` on win).
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
    let server_attack_dto = build_attack_server_dto(
        brid,
        is_pvp,
        initial_team1,
        initial_team2,
        user1_dto,
        user2_dto,
        drops,
    );

    let mut t1 = initial_team1.to_vec();
    let mut t2 = initial_team2.to_vec();
    let mut steps = Vec::new();
    let round_limit = 20u32;
    let mut round = 0u32;

    while round < round_limit
        && t1.iter().any(CombatUnit::is_alive)
        && t2.iter().any(CombatUnit::is_alive)
    {
        round += 1;
        // Alternate slot order 0..6: Team1 slot `s`, then Team2 slot `s`
        for slot in 0..6usize {
            // Team 1 unit at `slot` attacks first alive Team 2 unit
            if let Some(a_idx) = t1.iter().position(|u| u.is_alive() && u.position as usize == slot) {
                if let Some(d_idx) = t2.iter().position(CombatUnit::is_alive) {
                    let use_rage = t1[a_idx].cur_rage >= t1[a_idx].rage_cost;
                    let skill_id = if use_rage {
                        t1[a_idx].cur_rage -= t1[a_idx].rage_cost;
                        t1[a_idx].rage_skill_id
                    } else {
                        t1[a_idx].cur_rage = (t1[a_idx].cur_rage + 2).min(t1[a_idx].max_rage);
                        t1[a_idx].normal_skill_id
                    };
                    let base_dmg = (t1[a_idx].atk - t2[d_idx].pdef / 2).max(t1[a_idx].atk / 5).max(1);
                    let dmg = if use_rage { base_dmg * 22 / 10 } else { base_dmg };
                    t2[d_idx].cur_hp = (t2[d_idx].cur_hp - dmg).max(0);
                    t1[a_idx].damage_dealt += dmg;
                    steps.push(BattleStep {
                        round,
                        attacker_hid: t1[a_idx].hid,
                        defender_hid: t2[d_idx].hid,
                        skill_id,
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
            if let Some(a_idx) = t2.iter().position(|u| u.is_alive() && u.position as usize == slot) {
                if let Some(d_idx) = t1.iter().position(CombatUnit::is_alive) {
                    let use_rage = t2[a_idx].cur_rage >= t2[a_idx].rage_cost;
                    let skill_id = if use_rage {
                        t2[a_idx].cur_rage -= t2[a_idx].rage_cost;
                        t2[a_idx].rage_skill_id
                    } else {
                        t2[a_idx].cur_rage = (t2[a_idx].cur_rage + 2).min(t2[a_idx].max_rage);
                        t2[a_idx].normal_skill_id
                    };
                    let base_dmg = (t2[a_idx].atk - t1[d_idx].pdef / 2).max(t2[a_idx].atk / 5).max(1);
                    let dmg = if use_rage { base_dmg * 20 / 10 } else { base_dmg };
                    t1[d_idx].cur_hp = (t1[d_idx].cur_hp - dmg).max(0);
                    t2[a_idx].damage_dealt += dmg;
                    steps.push(BattleStep {
                        round,
                        attacker_hid: t2[a_idx].hid,
                        defender_hid: t1[d_idx].hid,
                        skill_id,
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

    let rage_queue = DtoValue::Array(
        steps
            .iter()
            .filter(|s| s.is_rage_skill)
            .map(|s| {
                DtoValue::map([
                    ("round", DtoValue::from(s.round)),
                    ("hid", DtoValue::from(s.attacker_hid)),
                    ("skillId", DtoValue::from(s.skill_id)),
                ])
            })
            .collect(),
    );

    let team_res_dto = |units: &[CombatUnit]| -> DtoValue {
        DtoValue::Array(
            units
                .iter()
                .map(|u| {
                    DtoValue::map([
                        ("hid", DtoValue::from(u.hid)),
                        ("curHp", DtoValue::from(u.cur_hp)),
                        ("maxHp", DtoValue::from(u.max_hp)),
                        ("curRage", DtoValue::from(u.cur_rage)),
                    ])
                })
                .collect(),
        )
    };

    let mut stat_map = BTreeMap::new();
    for u in t1.iter().chain(t2.iter()) {
        stat_map.insert(
            u.hid.to_string(),
            DtoValue::map([
                ("hid", DtoValue::from(u.hid)),
                ("damage", DtoValue::from(u.damage_dealt)),
            ]),
        );
    }

    let fight_base_data = DtoValue::map([
        ("bgPic", DtoValue::Array(Vec::new())),
        ("appearance_type", DtoValue::Array(Vec::new())),
        ("fightCondition", DtoValue::Array(Vec::new())),
        ("showHero", DtoValue::Array(Vec::new())),
        ("isJump", DtoValue::from(true)),
    ]);

    let fight_report_dto = DtoValue::map([
        ("brid", DtoValue::from(brid)),
        ("appraisal", DtoValue::from(appraisal)),
        ("round", DtoValue::from(round.max(1))),
        ("runTime", DtoValue::from(0.01f64)),
        ("fightBaseData", fight_base_data),
        ("serverData", server_attack_dto.clone()),
        ("rageQueue", rage_queue),
        (
            "heroResoultInfo",
            DtoValue::array([DtoValue::map([
                ("team1", team_res_dto(&t1)),
                ("team2", team_res_dto(&t2)),
            ])]),
        ),
        (
            "heroPercentInfo",
            DtoValue::map([
                ("team1", team_res_dto(&t1)),
                ("team2", team_res_dto(&t2)),
            ]),
        ),
        ("statisticalDataInfo", DtoValue::Map(stat_map)),
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
        units.push(CombatUnit::monster(10_000_001, player.utid, 0, player.level));
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
        ("level", DtoValue::from(level)),
        ("vip", DtoValue::from(vip)),
        ("fight_force", DtoValue::from(fight_force)),
        ("dress_id", DtoValue::from(dress_id)),
        ("soul_stone_power", DtoValue::from(0u32)),
        ("soulJadeQi", DtoValue::from(0u32)),
    ])
}

// ============================================================================
// PvE Campaign Copy Engine (`CopyData.lua`, `CopyProxy.lua`, `CopyFightProxy.lua`)
// ============================================================================

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

/// Manages Normal/Elite/Celebrity campaign progression, `copy.attack`, `copy.pass`, `copy.sweep`,
/// `copy.fetchPrize`, `copy.fetchBoxReward`, and `battle.getRecord`.
#[derive(Debug, Clone)]
pub struct CopyCampaignEngine {
    next_brid: u64,
    /// `(uid, copy_type) -> (chapter_id -> ChapterProgress)`
    progress: BTreeMap<(u64, u32), BTreeMap<u32, ChapterProgress>>,
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
    pub const MAX_DAILY_ATTACKS: u32 = 10;

    pub fn new() -> Self {
        Self {
            next_brid: 700_001,
            progress: BTreeMap::new(),
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

    pub fn get_record_blob(&self, brid: u64) -> Option<Vec<u8>> {
        self.battle_records
            .get(&brid)
            .map(SimulatedBattle::to_amf3_report_bytes)
    }

    fn ensure_player_chapters(&mut self, uid: u64, copy_type: u32) -> &mut BTreeMap<u32, ChapterProgress> {
        let chapters = self.progress.entry((uid, copy_type)).or_default();
        chapters.entry(1001).or_default();
        chapters
    }

    /// Returns `copy.getList` (`args = [copyType]`) matching `CopyData:parseData`.
    pub fn get_list(&mut self, uid: u64, copy_type: u32) -> DtoValue {
        let chapters = self.ensure_player_chapters(uid, copy_type);
        let mut copy_data = BTreeMap::new();
        for (&chap_id, chap) in chapters.iter() {
            let total_star: u32 = chap.stages.values().map(|s| s.stars).sum();
            let max_passed = chap
                .stages
                .iter()
                .filter(|(_, s)| s.stars > 0)
                .map(|(&cid, _)| cid)
                .max()
                .unwrap_or(0);
            let box_num = chap.stages.values().filter(|s| s.box_claimed).count() as u32;
            copy_data.insert(
                chap_id.to_string(),
                DtoValue::map([
                    ("star", DtoValue::from(total_star)),
                    ("prize", DtoValue::from(chap.prize_mask)),
                    ("pass_strongh", DtoValue::from(max_passed)),
                    ("box_reward_num", DtoValue::from(box_num)),
                ]),
            );
        }
        DtoValue::map([
            ("fresherRewardIndex", DtoValue::from(0u32)),
            ("copyData", DtoValue::Map(copy_data)),
        ])
    }

    /// Returns `copy.getCopy` (`args = [chapterId]`) stage details.
    pub fn get_chapter(&mut self, uid: u64, chapter_id: u32) -> DtoValue {
        let chapters = self.ensure_player_chapters(uid, 1);
        let chap = chapters.entry(chapter_id).or_default();
        let mut strongholds = BTreeMap::new();
        for (&copy_id, rec) in &chap.stages {
            strongholds.insert(
                copy_id.to_string(),
                DtoValue::map([
                    ("copy_id", DtoValue::from(copy_id)),
                    ("star", DtoValue::from(rec.stars)),
                    (
                        "less_num",
                        DtoValue::from(Self::MAX_DAILY_ATTACKS.saturating_sub(rec.attacks_today)),
                    ),
                    ("reset_num", DtoValue::from(rec.resets_today)),
                    (
                        "box_status",
                        DtoValue::from(if rec.box_claimed { 2u32 } else if rec.stars > 0 { 1u32 } else { 0u32 }),
                    ),
                ]),
            );
        }
        DtoValue::map([
            ("chapter_id", DtoValue::from(chapter_id)),
            ("prize", DtoValue::from(chap.prize_mask)),
            ("va_copy_info", DtoValue::Map(strongholds)),
        ])
    }

    /// Starts a PvE stage fight (`copy.attack` or `user.beforLoginBattle`), returning crash-proof
    /// `{ brid, drop, battleInfo, userInfo }`.
    pub fn attack_stage(
        &mut self,
        player: &PlayerState,
        chapter_id: u32,
        copy_id: u32,
        is_tutorial_before_login: bool,
    ) -> Result<DtoValue, &'static str> {
        if !is_tutorial_before_login {
            if player.execution < Self::EXEC_COST_PER_STAGE {
                return Err("not_enough_execution");
            }
            let chapters = self.ensure_player_chapters(player.uid, 1);
            let rec = chapters
                .entry(chapter_id)
                .or_default()
                .stages
                .entry(copy_id)
                .or_default();
            if rec.attacks_today >= Self::MAX_DAILY_ATTACKS {
                return Err("attack_limit_reached");
            }
        }

        let brid = self.alloc_brid();
        let stage_idx = (copy_id % 100).max(1);
        let monster_lv = if is_tutorial_before_login {
            1
        } else {
            ((chapter_id.saturating_sub(1001)) * 5 + stage_idx).max(1)
        };

        let team1 = extract_player_combat_units(player);
        let team2 = vec![
            CombatUnit::monster(900_001, 5001 + (stage_idx % 10), 0, monster_lv),
            CombatUnit::monster(900_002, 5002 + (stage_idx % 10), 1, monster_lv),
        ];

        let drops = vec![
            RewardEntry::Copper(1_500 + i64::from(stage_idx) * 250),
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
        let attack_dto = sim.server_attack_dto.clone();
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
    /// execution, awarding stars/exp/drops, unlocking the next chapter, and emitting live pushes.
    pub fn pass_stage(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let pending = match self.pending_battles.remove(&player.uid) {
            Some(p) if p.chapter_id == chapter_id && p.copy_id == copy_id => p,
            _ => {
                // Allow direct settlement if client re-sent `copy.pass`
                let _ = self.attack_stage(player, chapter_id, copy_id, false)?;
                self.pending_battles.remove(&player.uid).ok_or("no_active_battle")?
            }
        };

        if !pending.sim.is_win {
            return Ok((
                DtoValue::map([
                    ("battle_result", DtoValue::from(0u32)),
                    ("star", DtoValue::from(0u32)),
                    ("star_status", DtoValue::from(0u32)),
                    ("drop", DtoValue::Array(Vec::new())),
                ]),
                Vec::new(),
            ));
        }

        player.execution = player.execution.saturating_sub(Self::EXEC_COST_PER_STAGE);
        player.execution_time = now_ts;
        let exp_gain = u64::from(player.level.max(1)) * 60;
        player.exp += exp_gain;
        if player.exp >= u64::from(player.level) * 200 {
            player.exp = 0;
            player.level = (player.level + 1).min(200);
            let main_hid = player.roster.formation[0];
            if let Some(mh) = player.roster.heroes.get_mut(&main_hid) {
                mh.level = player.level;
                mh.recalculate_stats();
            }
        }

        let new_stars = pending.sim.stars.max(1);
        {
            let chapters = self.ensure_player_chapters(player.uid, 1);
            let chap = chapters.entry(chapter_id).or_default();
            let rec = chap.stages.entry(copy_id).or_default();
            rec.stars = rec.stars.max(new_stars);
            rec.attacks_today += 1;
            if (copy_id % 10) >= 5 {
                chapters.entry(chapter_id + 1).or_default();
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

        let drop_dto = DtoValue::Array(
            pending
                .drops
                .iter()
                .copied()
                .map(RewardEntry::to_triplet_dto)
                .collect(),
        );
        let ret = DtoValue::map([
            ("battle_result", DtoValue::from(1u32)),
            ("brid", DtoValue::from(pending.brid)),
            ("star", DtoValue::from(new_stars)),
            ("star_status", DtoValue::from(0u32)),
            ("drop", drop_dto),
            ("star_fragment", DtoValue::Map(BTreeMap::new())),
        ]);
        Ok((ret, pushes))
    }

    pub fn battle_fail(&mut self, uid: u64, _chapter_id: u32, _copy_id: u32) -> DtoValue {
        self.pending_battles.remove(&uid);
        DtoValue::map([("ret", DtoValue::from("ok"))])
    }

    /// Sweeps a 3-star cleared stage (`copy.sweep`, `args = [chapterId, copyId, num, isMultiple]`).
    pub fn sweep_stage(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        sweep_num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let n = sweep_num.clamp(1, 10);
        let needed_exec = Self::EXEC_COST_PER_STAGE * n;
        if player.execution < needed_exec {
            return Err("not_enough_execution");
        }
        {
            let chapters = self.ensure_player_chapters(player.uid, 1);
            let rec = chapters
                .get(&chapter_id)
                .and_then(|c| c.stages.get(&copy_id))
                .ok_or("stage_not_cleared")?;
            if rec.stars < 3 {
                return Err("requires_3_stars");
            }
            if rec.attacks_today + n > Self::MAX_DAILY_ATTACKS {
                return Err("attack_limit_reached");
            }
        }

        player.execution -= needed_exec;
        player.execution_time = now_ts;
        {
            let chapters = self.ensure_player_chapters(player.uid, 1);
            if let Some(rec) = chapters.entry(chapter_id).or_default().stages.get_mut(&copy_id) {
                rec.attacks_today += n;
            }
        }

        let mut all_rewards = Vec::new();
        let mut sweep_rounds = Vec::new();
        for _ in 0..n {
            let round_drop = [
                RewardEntry::Copper(1_800),
                RewardEntry::Item { tid: 17002, qty: 1 },
            ];
            all_rewards.extend_from_slice(&round_drop);
            sweep_rounds.push(DtoValue::Array(
                round_drop
                    .iter()
                    .copied()
                    .map(RewardEntry::to_triplet_dto)
                    .collect(),
            ));
        }

        let mut pushes = grant_rewards_to_player(player, &all_rewards, now_ts)?;
        pushes.insert(
            0,
            PlayerState::build_user_update_push(
                [
                    ("execution", DtoValue::from(player.execution)),
                    ("execution_time", DtoValue::from(player.execution_time)),
                ],
                now_ts,
            ),
        );

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("reward", DtoValue::Array(sweep_rounds)),
        ]);
        Ok((ret, pushes))
    }

    /// Resets daily attack count on a stage (`copy.resetAtk`, `args = [chapterId, copyId]`).
    pub fn reset_attack_count(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let cost = 20i64;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let chapters = self.ensure_player_chapters(player.uid, 1);
        let rec = chapters
            .entry(chapter_id)
            .or_default()
            .stages
            .entry(copy_id)
            .or_default();
        rec.attacks_today = 0;
        rec.resets_today += 1;

        let pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    /// Claims a chapter star chest (`copy.fetchPrize`, `args = [chapterId, boxIdx0Based]`).
    pub fn fetch_chapter_prize(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        box_idx_zero_based: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if box_idx_zero_based > 2 {
            return Err("invalid_box_idx");
        }
        let req_stars = [3u32, 6, 9][box_idx_zero_based as usize];
        let bit = 1u32 << box_idx_zero_based;

        {
            let chapters = self.ensure_player_chapters(player.uid, 1);
            let chap = chapters.entry(chapter_id).or_default();
            let cur_stars: u32 = chap.stages.values().map(|s| s.stars).sum();
            if cur_stars < req_stars {
                return Err("stars_not_enough");
            }
            if (chap.prize_mask & bit) != 0 {
                return Err("prize_already_claimed");
            }
            chap.prize_mask |= bit;
        }

        let reward = [
            RewardEntry::Gold(100 * i64::from(box_idx_zero_based + 1)),
            RewardEntry::Item { tid: 16012, qty: 1 },
        ];
        let pushes = grant_rewards_to_player(player, &reward, now_ts)?;
        let prize_mask = self
            .ensure_player_chapters(player.uid, 1)
            .get(&chapter_id)
            .map_or(bit, |c| c.prize_mask);
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("prize", DtoValue::from(prize_mask)),
            ]),
            pushes,
        ))
    }

    /// Claims a stage point chest (`copy.fetchBoxReward`, `args = [chapterId, copyId]` -> `"ok"`).
    pub fn fetch_stage_box_reward(
        &mut self,
        player: &mut PlayerState,
        chapter_id: u32,
        copy_id: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        {
            let chapters = self.ensure_player_chapters(player.uid, 1);
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
}

// ============================================================================
// Arena (`ArenaProxy.lua`) & Daily Sign / Online Rewards (`ActivitySignInCell.lua`, `online.db`)
// ============================================================================

#[derive(Debug, Clone)]
pub struct ArenaCompetitor {
    pub uid: u64,
    pub uname: String,
    pub level: u32,
    pub fight_force: u64,
    pub position: u32,
    pub is_npc: bool,
}

impl ArenaCompetitor {
    pub fn to_dto(&self) -> DtoValue {
        DtoValue::map([
            ("uid", DtoValue::from(self.uid)),
            ("uname", DtoValue::from(self.uname.as_str())),
            ("level", DtoValue::from(self.level)),
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
    attacks_used: BTreeMap<u64, u32>,
    attacks_bought: BTreeMap<u64, u32>,
}

impl Default for ArenaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ArenaEngine {
    pub fn new() -> Self {
        let mut ladder = BTreeMap::new();
        let mut uid_to_pos = BTreeMap::new();
        for pos in 1..=15u32 {
            let npc_uid = u64::from(pos);
            let comp = ArenaCompetitor {
                uid: npc_uid,
                uname: format!("ArenaGuard_{pos}"),
                level: 10 + pos * 2,
                fight_force: 3_000 + u64::from(16 - pos) * 1_200,
                position: pos,
                is_npc: true,
            };
            ladder.insert(pos, comp);
            uid_to_pos.insert(npc_uid, pos);
        }
        Self {
            ladder,
            uid_to_pos,
            attacks_used: BTreeMap::new(),
            attacks_bought: BTreeMap::new(),
        }
    }

    pub fn ensure_registered(&mut self, player: &PlayerState) -> u32 {
        if let Some(&pos) = self.uid_to_pos.get(&player.uid) {
            if let Some(entry) = self.ladder.get_mut(&pos) {
                entry.uname = player.uname.clone();
                entry.level = player.level;
                entry.fight_force = player.fight_force();
            }
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
        };
        self.ladder.insert(next_pos, comp);
        self.uid_to_pos.insert(player.uid, next_pos);
        next_pos
    }

    pub fn enter_arena(&mut self, player: &PlayerState) -> DtoValue {
        let my_pos = self.ensure_registered(player);
        let top10: Vec<DtoValue> = self
            .ladder
            .range(1..=10)
            .map(|(_, c)| c.to_dto())
            .collect();
        let start = my_pos.saturating_sub(4).max(1);
        let opponents: Vec<DtoValue> = self
            .ladder
            .range(start..=my_pos)
            .map(|(_, c)| c.to_dto())
            .collect();
        let used = self.attacks_used.get(&player.uid).copied().unwrap_or(0);
        let bought = self.attacks_bought.get(&player.uid).copied().unwrap_or(0);
        DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("is_have_challeged", DtoValue::from(0u32)),
            ("position", DtoValue::from(my_pos)),
            ("history_min_position", DtoValue::from(my_pos)),
            ("attack_num", DtoValue::from(used)),
            ("buy_num", DtoValue::from(bought)),
            ("win_num", DtoValue::from(0u32)),
            ("num_reward", DtoValue::Array(Vec::new())),
            (
                "res",
                DtoValue::map([
                    ("position", DtoValue::from(my_pos)),
                    ("attack_num", DtoValue::from(used)),
                    ("buy_num", DtoValue::from(bought)),
                    ("top10", DtoValue::Array(top10)),
                    ("opponents", DtoValue::Array(opponents)),
                ]),
            ),
        ])
    }

    /// Executes `arena.challenge` (`args = [myPos, targetPos, teamId, targetUname]`),
    /// running the deterministic simulator, swapping ranks on win, and returning
    /// `battleRecord` as an AMF3-encoded `FightReport` blob + `re.arena.dataRefresh`!
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
        let t2 = vec![CombatUnit::monster(
            defender.uid.max(800_001),
            310,
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
        let won = sim.is_win;
        copy_engine.record_battle(sim);

        *self.attacks_used.entry(player.uid).or_insert(0) += 1;

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
            let def_uid = def_entry.uid;
            let def_is_npc = def_entry.is_npc;
            self.ladder.insert(target_pos, my_entry);
            self.ladder.insert(my_pos, def_entry);

            if !def_is_npc {
                peer_pushes.push((
                    def_uid,
                    PushPacket::new(
                        push::ARENA_DATA_REFRESH,
                        DtoValue::map([
                            ("position", DtoValue::from(my_pos)),
                            ("top10", DtoValue::Array(Vec::new())),
                            ("opponents", DtoValue::Array(Vec::new())),
                        ]),
                        now_ts,
                    ),
                ));
            }
        }

        let coin_gain = if won { 50u64 } else { 25u64 };
        let self_pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::ArenaCoin(coin_gain), RewardEntry::Copper(5_000)],
            now_ts,
        )?;

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("result", DtoValue::from(if won { 1u32 } else { 0u32 })),
            ("position", DtoValue::from(new_pos)),
            ("oppo_fight_force", DtoValue::from(defender.fight_force)),
            ("battleRecord", DtoValue::Bytes(report_blob)),
        ]);
        Ok((ret, self_pushes, peer_pushes))
    }
}

/// Daily Sign-In (`sign.*` from `ActivitySignInCell.lua`) and Online Timed Rewards (`online.*`, `openOnline.*` from `online.db`).
#[derive(Debug, Clone, Default)]
pub struct SignAndOnlineService {
    sign_days: BTreeMap<u64, u32>,
    last_sign_day_bucket: BTreeMap<u64, u64>,
    last_sign_vip_claimed: BTreeMap<u64, bool>,
    claimed_online_tiers: BTreeMap<u64, BTreeSet<u32>>,
}

impl SignAndOnlineService {
    /// Exact tiers from decrypted `online.db` (`activity_open_online` rows `1..=5`):
    /// - 1: 300s -> `[4, 17002, 5]`
    /// - 2: 600s -> `[1, 0, 150000]`
    /// - 3: 600s -> `[4, 17002, 5]`
    /// - 4: 900s -> `[4, 22202, 2]`
    /// - 5: 1200s -> `[4, 22102, 2]`
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

    pub fn get_sign_info(&self, uid: u64, now_ts: u64) -> DtoValue {
        let day_bucket = now_ts / 86_400;
        let signed_today = self.last_sign_day_bucket.get(&uid).copied() == Some(day_bucket);
        let vip_claimed = self.last_sign_vip_claimed.get(&uid).copied().unwrap_or(false);
        DtoValue::map([
            ("uid", DtoValue::from(uid)),
            ("sign_num", DtoValue::from(self.sign_days.get(&uid).copied().unwrap_or(0))),
            ("signed_today", DtoValue::from(signed_today)),
            ("vip_multi", DtoValue::from(if vip_claimed { 1u32 } else { 0u32 })),
            ("rechargeRmb", DtoValue::from(0u32)),
            ("va_recharge_sign", DtoValue::Map(BTreeMap::new())),
            ("recharge_sign_num", DtoValue::from(0u32)),
            ("sign_time", DtoValue::from(0u64)),
            ("level_recharge", DtoValue::from(1u32)),
        ])
    }

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
            ]),
            pushes,
        ))
    }

    /// Claims VIP double bonus after signing today (`sign.gainVipSignReward` in `ActivitySignInCell.lua`).
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
        if self.last_sign_vip_claimed.get(&player.uid).copied().unwrap_or(false) {
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

    pub fn get_online_info(&self, player: &PlayerState) -> DtoValue {
        let claimed: Vec<DtoValue> = self
            .claimed_online_tiers
            .get(&player.uid)
            .map(|s| s.iter().copied().map(DtoValue::from).collect())
            .unwrap_or_default();
        DtoValue::map([
            ("accum_online_secs", DtoValue::from(player.online_accum_secs)),
            ("online_time", DtoValue::from(player.online_accum_secs)),
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
