use std::collections::BTreeMap;

use crate::bag::InventoryState;
use crate::error::{PlayerError, Result};
use crate::formation::FormationState;
use crate::json::{JsonObjReader, JsonValue};

/// Protagonist gender / template selector (`CreateUserScene.lua`: `utid = 1` Male, `utid = 2` Female).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Male = 1,
    Female = 2,
}

impl Gender {
    pub fn from_utid(utid: u32) -> Result<Self> {
        match utid {
            1 => Ok(Gender::Male),
            2 => Ok(Gender::Female),
            _ if gserver_data::is_protagonist_htid(utid) => {
                if utid % 2 == 1 {
                    Ok(Gender::Male)
                } else {
                    Ok(Gender::Female)
                }
            }
            _ => Err(PlayerError::InvalidParameter(format!(
                "invalid protagonist utid={utid} (expected 1=Male or 2=Female)"
            ))),
        }
    }

    pub const fn default_utid(self) -> u32 {
        self as u32
    }
}

/// All currencies tracked in `UserData.lua` (plus `JewelNum` for classic 1st-gen compatibility).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CurrencyType {
    Gold,
    GoldBind,
    Copper,
    HeroSoul,
    Prestige,
    Fame,
    Execution,
    Energy,
    JewelNum,
    ArenaCoin,
    TowerCoin,
    BusinessCoin,
    RebelCoin,
    Stone,
    BossCoin,
    AwakeArmCoin,
    LordwarCoin,
    GodarmCoin,
    PetCoin,
    GodanimalCoin,
    GodAnimalExploreCoin,
    WorldTeamCoin,
    WorldPersonScore,
    Grain,
    HeroLegendCoin,
    LegendBookCoin,
    BrochureCoin,
    Strategy,
    GuildPoint,
    SpiritCoin,
    ClassicWarCoin,
    BingshuCoin,
    DevilCoin,
    CountryWarCoin,
    UnifyCoin,
    FirstRideCoin,
    SoaringCoin,
    QushuiyanCoin,
    CrossGuildCoin,
    CrossGuildTechCoin,
    VoucherCoin,
    TimeMachineVoucherCoin,
    IronGoldCoin,
    HunshixilianCoin,
    SecretPassScore,
    SkincoinPurple,
    SkincoinOrange,
    SkincoinRed,
    SkincoinGold,
    SkincoinDarkgold,
}

impl CurrencyType {
    pub const ALL: [CurrencyType; 50] = [
        CurrencyType::Gold,
        CurrencyType::GoldBind,
        CurrencyType::Copper,
        CurrencyType::HeroSoul,
        CurrencyType::Prestige,
        CurrencyType::Fame,
        CurrencyType::Execution,
        CurrencyType::Energy,
        CurrencyType::JewelNum,
        CurrencyType::ArenaCoin,
        CurrencyType::TowerCoin,
        CurrencyType::BusinessCoin,
        CurrencyType::RebelCoin,
        CurrencyType::Stone,
        CurrencyType::BossCoin,
        CurrencyType::AwakeArmCoin,
        CurrencyType::LordwarCoin,
        CurrencyType::GodarmCoin,
        CurrencyType::PetCoin,
        CurrencyType::GodanimalCoin,
        CurrencyType::GodAnimalExploreCoin,
        CurrencyType::WorldTeamCoin,
        CurrencyType::WorldPersonScore,
        CurrencyType::Grain,
        CurrencyType::HeroLegendCoin,
        CurrencyType::LegendBookCoin,
        CurrencyType::BrochureCoin,
        CurrencyType::Strategy,
        CurrencyType::GuildPoint,
        CurrencyType::SpiritCoin,
        CurrencyType::ClassicWarCoin,
        CurrencyType::BingshuCoin,
        CurrencyType::DevilCoin,
        CurrencyType::CountryWarCoin,
        CurrencyType::UnifyCoin,
        CurrencyType::FirstRideCoin,
        CurrencyType::SoaringCoin,
        CurrencyType::QushuiyanCoin,
        CurrencyType::CrossGuildCoin,
        CurrencyType::CrossGuildTechCoin,
        CurrencyType::VoucherCoin,
        CurrencyType::TimeMachineVoucherCoin,
        CurrencyType::IronGoldCoin,
        CurrencyType::HunshixilianCoin,
        CurrencyType::SecretPassScore,
        CurrencyType::SkincoinPurple,
        CurrencyType::SkincoinOrange,
        CurrencyType::SkincoinRed,
        CurrencyType::SkincoinGold,
        CurrencyType::SkincoinDarkgold,
    ];

    /// Exact field name on `UserData` (`UserData.lua`).
    pub const fn vnsg2_key(self) -> &'static str {
        match self {
            CurrencyType::Gold => "gold",
            CurrencyType::GoldBind => "gold_bind",
            CurrencyType::Copper => "copper",
            CurrencyType::HeroSoul => "hero_soul",
            CurrencyType::Prestige => "prestige",
            CurrencyType::Fame => "fame",
            CurrencyType::Execution => "execution",
            CurrencyType::Energy => "energy",
            CurrencyType::JewelNum => "jewel_num",
            CurrencyType::ArenaCoin => "arena_coin",
            CurrencyType::TowerCoin => "tower_coin",
            CurrencyType::BusinessCoin => "business_coin",
            CurrencyType::RebelCoin => "rebel_coin",
            CurrencyType::Stone => "stone",
            CurrencyType::BossCoin => "boss_coin",
            CurrencyType::AwakeArmCoin => "awakeArm_coin",
            CurrencyType::LordwarCoin => "lordwar_coin",
            CurrencyType::GodarmCoin => "godarm_coin",
            CurrencyType::PetCoin => "pet_coin",
            CurrencyType::GodanimalCoin => "godanimal_coin",
            CurrencyType::GodAnimalExploreCoin => "god_animal_explore_coin",
            CurrencyType::WorldTeamCoin => "world_team_coin",
            CurrencyType::WorldPersonScore => "world_person_score",
            CurrencyType::Grain => "grain",
            CurrencyType::HeroLegendCoin => "hero_legend_coin",
            CurrencyType::LegendBookCoin => "legend_book_coin",
            CurrencyType::BrochureCoin => "brochure_coin",
            CurrencyType::Strategy => "strategy",
            CurrencyType::GuildPoint => "guild_point",
            CurrencyType::SpiritCoin => "spirit_coin",
            CurrencyType::ClassicWarCoin => "classic_war_coin",
            CurrencyType::BingshuCoin => "bingshu_coin",
            CurrencyType::DevilCoin => "devil_coin",
            CurrencyType::CountryWarCoin => "country_war_coin",
            CurrencyType::UnifyCoin => "unify_coin",
            CurrencyType::FirstRideCoin => "first_ride_coin",
            CurrencyType::SoaringCoin => "soaring_coin",
            CurrencyType::QushuiyanCoin => "qushuiyan_coin",
            CurrencyType::CrossGuildCoin => "cross_guild_coin",
            CurrencyType::CrossGuildTechCoin => "cross_guild_tech_coin",
            CurrencyType::VoucherCoin => "voucher_coin",
            CurrencyType::TimeMachineVoucherCoin => "time_machine_voucher_coin",
            CurrencyType::IronGoldCoin => "iron_gold_coin",
            CurrencyType::HunshixilianCoin => "hunshixilian_coin",
            CurrencyType::SecretPassScore => "secret_pass_score",
            CurrencyType::SkincoinPurple => "skincoin_purple",
            CurrencyType::SkincoinOrange => "skincoin_orange",
            CurrencyType::SkincoinRed => "skincoin_red",
            CurrencyType::SkincoinGold => "skincoin_gold",
            CurrencyType::SkincoinDarkgold => "skincoin_darkgold",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "gold_num" => Some(CurrencyType::Gold),
            "silver_num" => Some(CurrencyType::Copper),
            "soul_num" => Some(CurrencyType::HeroSoul),
            "prestige_num" => Some(CurrencyType::Prestige),
            "stamina" => Some(CurrencyType::Energy),
            _ => Self::ALL.into_iter().find(|c| c.vnsg2_key() == key),
        }
    }
}

/// Seed item granted to newly created characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StarterItemSeed {
    pub item_template_id: u32,
    pub count: u32,
}

/// Seed companion hero granted to newly created characters alongside the protagonist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StarterHeroSeed {
    pub htid: u32,
    pub level: u32,
    pub evolve_level: u32,
    pub auto_deploy_slot: Option<u8>,
}

/// Configurable starter gift pack applied when creating a new character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StarterPackConfig {
    pub initial_level: u32,
    pub initial_vip: u32,
    pub initial_vip_exp: u64,
    pub gold: u64,
    pub gold_bind: u64,
    pub copper: u64,
    pub hero_soul: u64,
    pub prestige: u64,
    pub jewel_num: u64,
    pub execution: u64,
    pub energy: u64,
    /// `default_fashion_map` in `current_config.db` maps protagonist qualities 2..5 -> fashion ID `1`.
    pub default_fashion_id: u32,
    /// 14-char `"YYYYMMDDHHMMSS"` string required by `UserData.lua` (`string.sub(self.server_open_time, 3, 14)`).
    pub server_open_time: String,
    pub server_id: u32,
    pub default_group_id: String,
    pub companion_heroes: Vec<StarterHeroSeed>,
    pub starter_items: Vec<StarterItemSeed>,
}

impl Default for StarterPackConfig {
    fn default() -> Self {
        Self {
            initial_level: 1,
            initial_vip: 6,
            initial_vip_exp: 10_000,
            gold: 58_888,
            gold_bind: 18_888,
            copper: 5_000_000,
            hero_soul: 20_000,
            prestige: 10_000,
            jewel_num: 5_000,
            execution: 120,
            energy: 50,
            default_fashion_id: 1,
            server_open_time: "20261124090000".to_string(),
            server_id: 2_900_161,
            default_group_id: "2900161".to_string(),
            companion_heroes: vec![
                // Starter companion vanguard general (deployed in slot 2, matching first_copy_hero_LineUp = 426|2)
                StarterHeroSeed {
                    htid: 426,
                    level: 1,
                    evolve_level: 0,
                    auto_deploy_slot: Some(2),
                },
            ],
            starter_items: vec![
                // Normal consumable prop (item_normal: 10010 = Tiến Cấp Đơn -> bag "item")
                StarterItemSeed {
                    item_template_id: 10_010,
                    count: 20,
                },
                // Direct-use stamina pill (item_direct: 22102 = Thể Lực Đơn -> bag "item")
                StarterItemSeed {
                    item_template_id: 22_102,
                    count: 10,
                },
                // Starter weapon equipment (item_arm: 112001 = Bách Luyện Kiếm, quality=2 -> bag "arm")
                StarterItemSeed {
                    item_template_id: 112_001,
                    count: 1,
                },
                // Starter accessory / treasure (item_treasure: 182001 = Đại Uyển -> bag "treasure")
                StarterItemSeed {
                    item_template_id: 182_001,
                    count: 1,
                },
                // Starter hero soul fragments (item_soul: 50101 = Mảnh Trương Liêu -> bag "soul")
                StarterItemSeed {
                    item_template_id: 50_101,
                    count: 30,
                },
            ],
        }
    }
}

/// Account record persisted in `data/accounts.json` (maps HTTP `/phone/login` `pid` to per-server `uid`s).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub pid: String,
    pub qid: String,
    pub username: String,
    pub auth_token: String,
    /// Maps `group_id` (e.g. `"2900161"`) to character `uid`.
    pub characters: BTreeMap<String, u64>,
    pub created_at: u64,
    pub last_login_at: u64,
    pub banned_until: u64,
    pub ban_reason: Option<String>,
}

impl Account {
    pub fn is_banned(&self, now_ts: u64) -> bool {
        self.banned_until > now_ts
    }

    pub fn to_json(&self) -> JsonValue {
        let mut chars = BTreeMap::new();
        for (gid, uid) in &self.characters {
            chars.insert(gid.clone(), JsonValue::from(*uid));
        }
        let mut m = BTreeMap::new();
        m.insert("pid".into(), JsonValue::from(self.pid.clone()));
        m.insert("qid".into(), JsonValue::from(self.qid.clone()));
        m.insert("username".into(), JsonValue::from(self.username.clone()));
        m.insert(
            "auth_token".into(),
            JsonValue::from(self.auth_token.clone()),
        );
        m.insert("characters".into(), JsonValue::Object(chars));
        m.insert("created_at".into(), JsonValue::from(self.created_at));
        m.insert("last_login_at".into(), JsonValue::from(self.last_login_at));
        m.insert("banned_until".into(), JsonValue::from(self.banned_until));
        match &self.ban_reason {
            Some(r) => m.insert("ban_reason".into(), JsonValue::from(r.clone())),
            None => m.insert("ban_reason".into(), JsonValue::Null),
        };
        JsonValue::Object(m)
    }

    pub fn from_json(val: &JsonValue) -> Result<Self> {
        let r = JsonObjReader::new(val, "Account")?;
        let mut characters = BTreeMap::new();
        if let Some(c_map) = r.opt_obj("characters")? {
            for (k, v) in c_map {
                let uid = v
                    .as_u64()
                    .ok_or_else(|| PlayerError::Json("invalid uid in Account.characters".into()))?;
                characters.insert(k.clone(), uid);
            }
        }
        Ok(Self {
            pid: r.req_str("pid")?,
            qid: r.opt_str("qid")?.unwrap_or_default(),
            username: r.req_str("username")?,
            auth_token: r.opt_str("auth_token")?.unwrap_or_default(),
            characters,
            created_at: r.opt_u64("created_at", 0)?,
            last_login_at: r.opt_u64("last_login_at", 0)?,
            banned_until: r.opt_u64("banned_until", 0)?,
            ban_reason: r.opt_str("ban_reason")?,
        })
    }
}

/// Complete hero instance matching `HeroInfo.lua` (`HerosData:parseData`).
#[derive(Clone, Debug, PartialEq)]
pub struct HeroInstance {
    pub hid: u64,
    pub htid: u32,
    pub dress_id: u32,
    pub level: u32,
    pub exp: u64,
    pub evolve_level: u32,
    pub upgrade_level: u32,
    pub is_lock: u8,
    pub hero_star_pos: u32,
    pub hero_star_level: u32,
    pub awake_rank: u32,
    pub awake_level: u32,
    pub gold_rank: u32,
    pub gold_level: u32,
    pub gold_status: u32,
    pub dark_gold_rank: u32,
    pub dark_gold_level: u32,
    pub purple_gold_rank: u32,
    pub purple_gold_level: u32,
    pub color_gold_rank: u32,
    pub color_gold_level: u32,
    pub nirvana_gold_rank: u32,
    pub nirvana_gold_level: u32,
    pub gild_gold_rank: u32,
    pub gild_gold_level: u32,
    pub iron_gold_rank: u32,
    pub iron_gold_level: u32,
    pub hero_pill_rank: u32,
    pub hero_pill_level: u32,
    pub talent: Vec<u32>,
    pub va_fail_reward: BTreeMap<String, JsonValue>,
    pub va_use_wash: BTreeMap<String, JsonValue>,
    pub va_last_wash: BTreeMap<String, JsonValue>,
    pub va_wash_count: BTreeMap<String, JsonValue>,
    pub va_inlay: BTreeMap<String, JsonValue>,
}

impl HeroInstance {
    pub fn new(hid: u64, htid: u32, level: u32) -> Self {
        Self {
            hid,
            htid,
            dress_id: 0,
            level: level.max(1),
            exp: 0,
            evolve_level: 0,
            upgrade_level: 0,
            is_lock: 0,
            hero_star_pos: 0,
            hero_star_level: 1,
            awake_rank: 0,
            awake_level: 0,
            gold_rank: 0,
            gold_level: 0,
            gold_status: 0,
            dark_gold_rank: 0,
            dark_gold_level: 0,
            purple_gold_rank: 0,
            purple_gold_level: 0,
            color_gold_rank: 0,
            color_gold_level: 0,
            nirvana_gold_rank: 0,
            nirvana_gold_level: 0,
            gild_gold_rank: 0,
            gild_gold_level: 0,
            iron_gold_rank: 0,
            iron_gold_level: 0,
            hero_pill_rank: 0,
            hero_pill_level: 0,
            talent: Vec::new(),
            va_fail_reward: BTreeMap::new(),
            va_use_wash: BTreeMap::new(),
            va_last_wash: BTreeMap::new(),
            va_wash_count: BTreeMap::new(),
            va_inlay: BTreeMap::new(),
        }
    }

    /// `timeMachine_current_config.db` defines protagonist template IDs as `1..=36` (male odd `1..35`, female even `2..36`).
    pub const fn is_protagonist(&self) -> bool {
        self.htid >= 1 && self.htid <= 36
    }

    /// Resolves authentic `(normal_skill_id, rage_skill_id)` (`com_atk`, `angry_atk`) from `gserver-data`.
    pub fn resolved_battle_skills(&self) -> gserver_data::ResolvedHeroBattleSkills {
        gserver_data::resolve_hero_battle_skills_with_awake(self.htid, self.awake_rank)
    }

    /// Returns `(normal_skill_id, rage_skill_id)` for this hero instance.
    pub fn battle_skill_ids(&self) -> (u32, u32) {
        let res = self.resolved_battle_skills();
        (res.normal_skill_id, res.rage_skill_id)
    }

    /// Exact `HeroInfo:init(data)` wire dictionary (`HeroInfo.lua`), plus classic `soul` / `talent` / `equip` compatibility keys.
    pub fn to_vnsg2_hero_info(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("hid".into(), JsonValue::from(self.hid));
        m.insert("htid".into(), JsonValue::from(self.htid));
        m.insert("dress_id".into(), JsonValue::from(self.dress_id));
        m.insert("level".into(), JsonValue::from(self.level));
        m.insert("exp".into(), JsonValue::from(self.exp));
        m.insert("soul".into(), JsonValue::from(self.exp)); // Classic 1st-gen alias
        m.insert("evolve_level".into(), JsonValue::from(self.evolve_level));
        m.insert("upgrade_level".into(), JsonValue::from(self.upgrade_level));
        m.insert("is_lock".into(), JsonValue::from(self.is_lock));
        m.insert("hero_star_pos".into(), JsonValue::from(self.hero_star_pos));
        m.insert(
            "hero_star_level".into(),
            JsonValue::from(self.hero_star_level),
        );
        m.insert("awake_rank".into(), JsonValue::from(self.awake_rank));
        m.insert("awake_level".into(), JsonValue::from(self.awake_level));
        m.insert("gold_rank".into(), JsonValue::from(self.gold_rank));
        m.insert("gold_level".into(), JsonValue::from(self.gold_level));
        m.insert("gold_status".into(), JsonValue::from(self.gold_status));
        m.insert(
            "dark_gold_rank".into(),
            JsonValue::from(self.dark_gold_rank),
        );
        m.insert(
            "dark_gold_level".into(),
            JsonValue::from(self.dark_gold_level),
        );
        m.insert(
            "purple_gold_rank".into(),
            JsonValue::from(self.purple_gold_rank),
        );
        m.insert(
            "purple_gold_level".into(),
            JsonValue::from(self.purple_gold_level),
        );
        m.insert(
            "color_gold_rank".into(),
            JsonValue::from(self.color_gold_rank),
        );
        m.insert(
            "color_gold_level".into(),
            JsonValue::from(self.color_gold_level),
        );
        m.insert("va_color_gold_gift".into(), JsonValue::empty_object());
        m.insert(
            "nirvana_gold_rank".into(),
            JsonValue::from(self.nirvana_gold_rank),
        );
        m.insert(
            "nirvana_gold_level".into(),
            JsonValue::from(self.nirvana_gold_level),
        );
        m.insert("va_nirvana_gold_gift".into(), JsonValue::empty_object());
        m.insert(
            "gild_gold_rank".into(),
            JsonValue::from(self.gild_gold_rank),
        );
        m.insert(
            "gild_gold_level".into(),
            JsonValue::from(self.gild_gold_level),
        );
        m.insert("va_gild_gold_gift".into(), JsonValue::empty_object());
        m.insert(
            "iron_gold_rank".into(),
            JsonValue::from(self.iron_gold_rank),
        );
        m.insert(
            "iron_gold_level".into(),
            JsonValue::from(self.iron_gold_level),
        );
        m.insert("va_iron_gold_gift".into(), JsonValue::empty_object());
        m.insert(
            "hero_pill_rank".into(),
            JsonValue::from(self.hero_pill_rank),
        );
        m.insert(
            "hero_pill_level".into(),
            JsonValue::from(self.hero_pill_level),
        );

        // CRITICAL: va_fail_reward MUST be a non-nil table (HeroInfo:cloneTable iterates pairs(t) directly!)
        m.insert(
            "va_fail_reward".into(),
            JsonValue::Object(self.va_fail_reward.clone()),
        );
        m.insert(
            "va_use_wash".into(),
            JsonValue::Object(self.va_use_wash.clone()),
        );
        m.insert(
            "va_last_wash".into(),
            JsonValue::Object(self.va_last_wash.clone()),
        );
        m.insert(
            "va_wash_count".into(),
            JsonValue::Object(self.va_wash_count.clone()),
        );
        m.insert("va_inlay".into(), JsonValue::Object(self.va_inlay.clone()));
        m.insert(
            "talent".into(),
            JsonValue::Array(self.talent.iter().map(|&t| JsonValue::from(t)).collect()),
        );
        m.insert("equip".into(), JsonValue::empty_object());
        JsonValue::Object(m)
    }

    pub fn from_json(val: &JsonValue) -> Result<Self> {
        let r = JsonObjReader::new(val, "HeroInstance")?;
        let mut talent = Vec::new();
        if let Some(arr) = r.opt_arr("talent")? {
            for v in arr {
                if let Some(id) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    talent.push(id);
                }
            }
        }
        Ok(Self {
            hid: r.req_u64("hid")?,
            htid: r.req_u32("htid")?,
            dress_id: r.opt_u32("dress_id", 0)?,
            level: r.opt_u32("level", 1)?,
            exp: r.opt_u64("exp", 0)?,
            evolve_level: r.opt_u32("evolve_level", 0)?,
            upgrade_level: r.opt_u32("upgrade_level", 0)?,
            is_lock: r.opt_u8("is_lock", 0)?,
            hero_star_pos: r.opt_u32("hero_star_pos", 0)?,
            hero_star_level: r.opt_u32("hero_star_level", 1)?,
            awake_rank: r.opt_u32("awake_rank", 0)?,
            awake_level: r.opt_u32("awake_level", 0)?,
            gold_rank: r.opt_u32("gold_rank", 0)?,
            gold_level: r.opt_u32("gold_level", 0)?,
            gold_status: r.opt_u32("gold_status", 0)?,
            dark_gold_rank: r.opt_u32("dark_gold_rank", 0)?,
            dark_gold_level: r.opt_u32("dark_gold_level", 0)?,
            purple_gold_rank: r.opt_u32("purple_gold_rank", 0)?,
            purple_gold_level: r.opt_u32("purple_gold_level", 0)?,
            color_gold_rank: r.opt_u32("color_gold_rank", 0)?,
            color_gold_level: r.opt_u32("color_gold_level", 0)?,
            nirvana_gold_rank: r.opt_u32("nirvana_gold_rank", 0)?,
            nirvana_gold_level: r.opt_u32("nirvana_gold_level", 0)?,
            gild_gold_rank: r.opt_u32("gild_gold_rank", 0)?,
            gild_gold_level: r.opt_u32("gild_gold_level", 0)?,
            iron_gold_rank: r.opt_u32("iron_gold_rank", 0)?,
            iron_gold_level: r.opt_u32("iron_gold_level", 0)?,
            hero_pill_rank: r.opt_u32("hero_pill_rank", 0)?,
            hero_pill_level: r.opt_u32("hero_pill_level", 0)?,
            talent,
            va_fail_reward: r.opt_obj("va_fail_reward")?.cloned().unwrap_or_default(),
            va_use_wash: r.opt_obj("va_use_wash")?.cloned().unwrap_or_default(),
            va_last_wash: r.opt_obj("va_last_wash")?.cloned().unwrap_or_default(),
            va_wash_count: r.opt_obj("va_wash_count")?.cloned().unwrap_or_default(),
            va_inlay: r.opt_obj("va_inlay")?.cloned().unwrap_or_default(),
        })
    }
}

/// Complete persistent player state (`UserData.lua` + `HerosData.lua` + `HeroFormationData.lua` + `BagData.lua` + `ConfigData.lua`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerProfile {
    pub uid: u64,
    pub pid: String,
    pub server_id: u32,
    pub group_id: String,
    pub uname: String,
    pub utid: u32,
    pub protagonist_hid: u64,
    pub level: u32,
    pub exp: u64,
    pub vip: u32,
    pub vip_exp: u64,
    pub fc: u64,
    pub fight_force_max: u64,
    pub is_recharge: u8,
    pub gold_recharge_accum_all: u64,
    pub currencies: BTreeMap<CurrencyType, u64>,
    pub execution_time: u64,
    pub buy_execution_accum: u32,
    pub buy_execution_time: u64,
    pub energy_time: u64,
    pub buy_copper_accum: u32,
    pub buy_copper_time: u64,
    pub dress_id: u32,
    pub attr_dress_id: u32,
    pub dress_info: BTreeMap<u32, u32>,
    pub dress_expire: BTreeMap<u32, u64>,
    pub dress_hero_info: Vec<u32>,
    pub avatar_id: u32,
    pub va_avatar: Vec<u32>,
    pub destiny: i32,
    pub destiny_star: u32,
    pub camp: u8,
    pub country: u8,
    pub post: u32,
    pub guild_id: u64,
    pub town_id: u32,
    pub hero_bag_buy_times: u32,
    pub uname_change_num: u32,
    pub create_time: u64,
    pub login_time: u64,
    pub last_login_time: u64,
    pub online_accum_time: u64,
    pub ban_chat_time: u64,
    pub server_open_time: String,
    pub va_config: BTreeMap<String, JsonValue>,
    pub copy_progress: BTreeMap<String, JsonValue>,
    pub act_star_claimed: Vec<u32>,
    pub act_star_sign_claimed: Vec<u32>,
    pub heroes: BTreeMap<u64, HeroInstance>,
    pub formation: FormationState,
    pub inventory: InventoryState,
    pub next_hid: u64,
    pub next_item_id: u64,
    pub revision: u64,
}

impl PlayerProfile {
    /// Construct a new character initialized from `StarterPackConfig` (`user.createUser`).
    pub fn new_character(
        uid: u64,
        pid: impl Into<String>,
        group_id: impl Into<String>,
        utid: u32,
        uname: impl Into<String>,
        now_ts: u64,
        starter: &StarterPackConfig,
    ) -> Result<Self> {
        let _gender = Gender::from_utid(utid)?;
        let uname_str = uname.into();
        let trimmed = uname_str.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 15 {
            return Err(PlayerError::InvalidParameter(
                "character uname must be 1..=15 Unicode characters".into(),
            ));
        }
        if starter.server_open_time.len() < 14 {
            return Err(PlayerError::InvalidParameter(
                "server_open_time must be at least 14 chars (YYYYMMDDHHMMSS)".into(),
            ));
        }

        let protagonist_hid = 10_000_001u64;
        let protagonist = HeroInstance::new(protagonist_hid, utid, starter.initial_level.max(1));
        let mut heroes = BTreeMap::new();
        heroes.insert(protagonist_hid, protagonist);

        let mut currencies = BTreeMap::new();
        for c in CurrencyType::ALL {
            currencies.insert(c, 0);
        }
        currencies.insert(CurrencyType::Gold, starter.gold);
        currencies.insert(CurrencyType::GoldBind, starter.gold_bind);
        currencies.insert(CurrencyType::Copper, starter.copper);
        currencies.insert(CurrencyType::HeroSoul, starter.hero_soul);
        currencies.insert(CurrencyType::Prestige, starter.prestige);
        currencies.insert(CurrencyType::JewelNum, starter.jewel_num);
        currencies.insert(CurrencyType::Execution, starter.execution);
        currencies.insert(CurrencyType::Energy, starter.energy);

        let fashion_id = starter.default_fashion_id.max(1);
        let mut dress_info = BTreeMap::new();
        dress_info.insert(fashion_id, 1);

        let group_str = group_id.into();
        let resolved_group = if group_str.is_empty() {
            starter.default_group_id.clone()
        } else {
            group_str
        };

        let mut profile = Self {
            uid,
            pid: pid.into(),
            server_id: starter.server_id,
            group_id: resolved_group,
            uname: trimmed.to_string(),
            utid,
            protagonist_hid,
            level: starter.initial_level.max(1),
            exp: 0,
            vip: starter.initial_vip,
            vip_exp: starter.initial_vip_exp,
            fc: 0,
            fight_force_max: 0,
            is_recharge: if starter.initial_vip > 0 { 1 } else { 0 },
            gold_recharge_accum_all: starter.initial_vip_exp,
            currencies,
            execution_time: now_ts,
            buy_execution_accum: 0,
            buy_execution_time: now_ts,
            energy_time: now_ts,
            buy_copper_accum: 0,
            buy_copper_time: now_ts,
            dress_id: fashion_id,
            attr_dress_id: fashion_id,
            dress_info,
            dress_expire: BTreeMap::new(),
            dress_hero_info: Vec::new(),
            avatar_id: 0,
            va_avatar: Vec::new(),
            destiny: -1,
            destiny_star: 0,
            camp: 0,
            country: 1,
            post: 0,
            guild_id: 0,
            town_id: 1,
            hero_bag_buy_times: 0,
            uname_change_num: 0,
            create_time: now_ts,
            login_time: now_ts,
            last_login_time: now_ts,
            online_accum_time: 0,
            ban_chat_time: 0,
            server_open_time: starter.server_open_time.clone(),
            va_config: BTreeMap::new(),
            copy_progress: BTreeMap::new(),
            act_star_claimed: Vec::new(),
            act_star_sign_claimed: Vec::new(),
            heroes,
            formation: FormationState::with_protagonist(protagonist_hid),
            inventory: InventoryState::new(),
            next_hid: protagonist_hid + 1,
            next_item_id: 1_000_001,
            revision: 1,
        };

        // Grant starter companion heroes
        for seed in &starter.companion_heroes {
            let hid = profile.summon_hero(seed.htid, seed.level)?;
            if seed.evolve_level > 0 {
                if let Some(h) = profile.heroes.get_mut(&hid) {
                    h.evolve_level = seed.evolve_level;
                }
            }
            if let Some(slot) = seed.auto_deploy_slot {
                profile.set_squad_slot(slot, hid)?;
            }
        }

        // Grant starter items
        for item_seed in &starter.starter_items {
            if item_seed.count > 0 {
                profile.add_item(item_seed.item_template_id, item_seed.count, now_ts)?;
            }
        }

        profile.recalculate_fight_force();
        profile.revision = 1;
        Ok(profile)
    }

    pub fn currency(&self, currency: CurrencyType) -> u64 {
        self.currencies.get(&currency).copied().unwrap_or(0)
    }

    /// Maximum hero roster capacity (`bag.db` row 1: `base_bag_storage = 200`, `buy_storage = 10`).
    pub const fn max_hero_capacity(&self) -> u32 {
        200 + 10 * self.hero_bag_buy_times
    }

    /// Recompute synthetic combat power (`fc` & `fight_force_max`) from deployed heroes and equipment.
    pub fn recalculate_fight_force(&mut self) {
        let mut total_fc = 0u64;
        for (&slot, &hid) in &self.formation.position {
            if let Some(hero) = self.heroes.get(&hid) {
                let hero_power = 500u64
                    + u64::from(hero.level) * 120
                    + u64::from(hero.evolve_level) * 850
                    + u64::from(hero.upgrade_level) * 600
                    + u64::from(hero.awake_rank) * 1_500
                    + u64::from(hero.gold_rank) * 2_000;
                total_fc = total_fc.saturating_add(hero_power);
            }
            if let Some(eq_map) = self.formation.arming.get(&slot) {
                for item in eq_map.values() {
                    let reinf = item
                        .va_item_text
                        .get("reinforce_level")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1);
                    let prog = item
                        .va_item_text
                        .get("progress_level")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    total_fc = total_fc.saturating_add(250 + reinf * 45 + prog * 180);
                }
            }
        }
        self.fc = total_fc;
        if self.fc > self.fight_force_max {
            self.fight_force_max = self.fc;
        }
    }

    /// Wire payload for `user.getUserBeforLogin` (`LoginProxy.lua`: checks `if ret.uid == nil`).
    pub fn to_vnsg2_user_before_login(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("uid".into(), JsonValue::from(self.uid));
        m.insert("uname".into(), JsonValue::from(self.uname.clone()));
        m.insert("utid".into(), JsonValue::from(self.utid));
        m.insert("level".into(), JsonValue::from(self.level));
        m.insert("vip".into(), JsonValue::from(self.vip));
        JsonValue::Object(m)
    }

    /// Character summary used by HTTP `/phone/loginrole/` (`live_capture_filtered.log`) and classic `user.getUsers`.
    pub fn to_character_summary(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("uid".into(), JsonValue::from(self.uid));
        m.insert("uname".into(), JsonValue::from(self.uname.clone()));
        m.insert("utid".into(), JsonValue::from(self.utid));
        m.insert("htid".into(), JsonValue::from(self.utid));
        m.insert("level".into(), JsonValue::from(self.level));
        m.insert("vip".into(), JsonValue::from(self.vip));
        m.insert("quality".into(), JsonValue::from(5u32));
        m.insert("groupid".into(), JsonValue::from(self.group_id.clone()));
        m.insert("dress_id".into(), JsonValue::from(self.dress_id));
        m.insert(
            "last_login_time".into(),
            JsonValue::from(self.last_login_time),
        );
        JsonValue::Object(m)
    }

    /// Exact `user.getUser` payload (`UserData:parseData` in `UserData.lua`),
    /// respecting `tonumber(copper)` string encoding, direct numeric arithmetic fields,
    /// 14-char `server_open_time`, non-nil `dress_info` / `dress_expire` / `dress_hero_info`,
    /// and classic 1st-gen field aliases (`gold_num`, `silver_num`, `soul_num`, `jewel_num`, `prestige_num`, `stamina`, `fight_force`, `va_user`).
    pub fn to_vnsg2_get_user(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("uid".into(), JsonValue::from(self.uid));
        m.insert("pid".into(), JsonValue::from(self.pid.clone()));
        m.insert("server_id".into(), JsonValue::from(self.server_id));
        m.insert("group_id".into(), JsonValue::from(self.group_id.clone()));
        m.insert("uname".into(), JsonValue::from(self.uname.clone()));
        m.insert("utid".into(), JsonValue::from(self.utid));
        m.insert("htid".into(), JsonValue::from(self.utid));
        m.insert("level".into(), JsonValue::from(self.level));
        m.insert("exp".into(), JsonValue::from(self.exp));
        m.insert("exp_num".into(), JsonValue::from(self.exp));
        m.insert("vip".into(), JsonValue::from(self.vip));
        m.insert("vip_exp".into(), JsonValue::from(self.vip_exp));

        // Big-number string field parsed via `self.copper = tonumber(self.copper)` in UserData.lua:1932
        let copper = self.currency(CurrencyType::Copper);
        m.insert("copper".into(), JsonValue::from(copper.to_string()));
        m.insert("silver_num".into(), JsonValue::from(copper));

        // Direct arithmetic number currencies (MUST be JSON/AMF numbers, NOT strings!)
        for c in CurrencyType::ALL {
            if c != CurrencyType::Copper {
                m.insert(c.vnsg2_key().into(), JsonValue::from(self.currency(c)));
            }
        }

        // Classic 1st-gen field aliases requested by downstream protocol adapters
        m.insert(
            "gold_num".into(),
            JsonValue::from(self.currency(CurrencyType::Gold)),
        );
        m.insert(
            "soul_num".into(),
            JsonValue::from(self.currency(CurrencyType::HeroSoul)),
        );
        m.insert(
            "prestige_num".into(),
            JsonValue::from(self.currency(CurrencyType::Prestige)),
        );
        m.insert(
            "stamina".into(),
            JsonValue::from(self.currency(CurrencyType::Energy)),
        );
        m.insert("stamina_time".into(), JsonValue::from(self.energy_time));

        // Timers & purchase accumulators (MUST be numbers for initExecutionTime / initEnergyTime)
        m.insert(
            "execution_time".into(),
            JsonValue::from(self.execution_time),
        );
        m.insert(
            "buy_execution_accum".into(),
            JsonValue::from(self.buy_execution_accum),
        );
        m.insert(
            "buy_execution_time".into(),
            JsonValue::from(self.buy_execution_time),
        );
        m.insert("energy_time".into(), JsonValue::from(self.energy_time));
        m.insert(
            "buy_copper_accum".into(),
            JsonValue::from(self.buy_copper_accum),
        );
        m.insert(
            "buy_copper_time".into(),
            JsonValue::from(self.buy_copper_time),
        );

        // Fight force (`fc` number + `fight_force_max` string/number via tonumber + `fight_force` alias)
        m.insert("fc".into(), JsonValue::from(self.fc));
        m.insert("fight_force".into(), JsonValue::from(self.fc));
        m.insert(
            "fight_force_max".into(),
            JsonValue::from(self.fight_force_max.to_string()),
        );

        m.insert("is_recharge".into(), JsonValue::from(self.is_recharge));
        m.insert(
            "gold_recharge_accum_all".into(),
            JsonValue::from(self.gold_recharge_accum_all),
        );

        // Dress / Fashion tables (MUST contain attr_dress_id inside dress_info!)
        m.insert("dress_id".into(), JsonValue::from(self.dress_id));
        m.insert("attr_dress_id".into(), JsonValue::from(self.attr_dress_id));
        let mut d_info = BTreeMap::new();
        for (&fid, &lv) in &self.dress_info {
            d_info.insert(fid.to_string(), JsonValue::from(lv));
        }
        m.insert("dress_info".into(), JsonValue::Object(d_info.clone()));
        m.insert("dress".into(), JsonValue::Object(d_info));

        let mut d_exp = BTreeMap::new();
        for (&fid, &ts) in &self.dress_expire {
            d_exp.insert(fid.to_string(), JsonValue::from(ts));
        }
        m.insert("dress_expire".into(), JsonValue::Object(d_exp));
        m.insert(
            "dress_hero_info".into(),
            JsonValue::Array(
                self.dress_hero_info
                    .iter()
                    .map(|&id| JsonValue::from(id))
                    .collect(),
            ),
        );

        m.insert("avatar_id".into(), JsonValue::from(self.avatar_id));
        m.insert(
            "va_avatar".into(),
            JsonValue::Array(
                self.va_avatar
                    .iter()
                    .map(|&id| JsonValue::from(id))
                    .collect(),
            ),
        );

        // Recharge & va_user nested tables
        let mut va_recharge = BTreeMap::new();
        va_recharge.insert("first_recharge".into(), JsonValue::empty_object());
        va_recharge.insert("recommend_recharge".into(), JsonValue::empty_object());
        m.insert("va_recharge".into(), JsonValue::Object(va_recharge));

        let mut va_user = BTreeMap::new();
        va_user.insert("dress".into(), JsonValue::from(self.dress_id));
        va_user.insert(
            "va_config".into(),
            JsonValue::Object(self.va_config.clone()),
        );
        m.insert("va_user".into(), JsonValue::Object(va_user));

        // Meta & capacity fields (`hero_bag_buy_times` MUST be number for HerosData:getMaxNum!)
        m.insert(
            "hero_bag_used".into(),
            JsonValue::from(self.heroes.len() as u32),
        );
        m.insert(
            "hero_bag_buy_times".into(),
            JsonValue::from(self.hero_bag_buy_times),
        );
        m.insert(
            "uname_change_num".into(),
            JsonValue::from(self.uname_change_num),
        );
        m.insert("destiny".into(), JsonValue::from(self.destiny));
        m.insert("destiny_star".into(), JsonValue::from(self.destiny_star));
        m.insert("camp".into(), JsonValue::from(self.camp));
        m.insert("country".into(), JsonValue::from(self.country));
        m.insert("post".into(), JsonValue::from(self.post));
        m.insert("guild_id".into(), JsonValue::from(self.guild_id));
        m.insert("town_id".into(), JsonValue::from(self.town_id));
        m.insert("create_time".into(), JsonValue::from(self.create_time));
        m.insert("login_time".into(), JsonValue::from(self.login_time));
        m.insert(
            "last_login_time".into(),
            JsonValue::from(self.last_login_time),
        );
        m.insert(
            "online_accum_time".into(),
            JsonValue::from(self.online_accum_time),
        );
        m.insert("ban_chat_time".into(), JsonValue::from(self.ban_chat_time));
        m.insert(
            "activity_login_give_gold_open_time".into(),
            JsonValue::from(0u64),
        );

        // CRITICAL 14-char string: string.sub(self.server_open_time, 3, 14)
        m.insert(
            "server_open_time".into(),
            JsonValue::from(self.server_open_time.clone()),
        );

        // Feature open/close switches (false = open in UserData.lua)
        for close_flag in [
            "boss_close",
            "fiend_close",
            "spirit_close",
            "yanwu_close",
            "swap_close",
            "guildwar_close",
            "guildmanwar_close",
            "pet_close",
            "passcity_close",
            "swap_country_close",
            "world_team_close",
            "world_mine_close",
            "godArm_close",
            "awake_close",
            "awake_copy_close",
            "halidom_close",
            "huntdeer_close",
            "classic_war_close",
            "country_war_close",
            "unify_close",
            "first_ride_close",
            "soaring_copy_close",
        ] {
            m.insert(close_flag.into(), JsonValue::from(false));
        }

        JsonValue::Object(m)
    }

    /// Exact `user.getVaConfig` payload (`ConfigData.lua`).
    pub fn to_vnsg2_va_config(&self) -> JsonValue {
        JsonValue::Object(self.va_config.clone())
    }

    /// Full canonical JSON snapshot persisted to `data/players/<uid>.json`.
    pub fn to_storage_json(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("schema_version".into(), JsonValue::from(1u32));
        m.insert("revision".into(), JsonValue::from(self.revision));
        m.insert("uid".into(), JsonValue::from(self.uid));
        m.insert("pid".into(), JsonValue::from(self.pid.clone()));
        m.insert("server_id".into(), JsonValue::from(self.server_id));
        m.insert("group_id".into(), JsonValue::from(self.group_id.clone()));
        m.insert("uname".into(), JsonValue::from(self.uname.clone()));
        m.insert("utid".into(), JsonValue::from(self.utid));
        m.insert(
            "protagonist_hid".into(),
            JsonValue::from(self.protagonist_hid),
        );
        m.insert("level".into(), JsonValue::from(self.level));
        m.insert("exp".into(), JsonValue::from(self.exp));
        m.insert("vip".into(), JsonValue::from(self.vip));
        m.insert("vip_exp".into(), JsonValue::from(self.vip_exp));
        m.insert("fc".into(), JsonValue::from(self.fc));
        m.insert(
            "fight_force_max".into(),
            JsonValue::from(self.fight_force_max),
        );
        m.insert("is_recharge".into(), JsonValue::from(self.is_recharge));
        m.insert(
            "gold_recharge_accum_all".into(),
            JsonValue::from(self.gold_recharge_accum_all),
        );

        let mut curr_obj = BTreeMap::new();
        for c in CurrencyType::ALL {
            let v = self.currency(c);
            if v > 0
                || matches!(
                    c,
                    CurrencyType::Gold
                        | CurrencyType::Copper
                        | CurrencyType::Execution
                        | CurrencyType::Energy
                )
            {
                curr_obj.insert(c.vnsg2_key().into(), JsonValue::from(v));
            }
        }
        m.insert("currencies".into(), JsonValue::Object(curr_obj));

        m.insert(
            "execution_time".into(),
            JsonValue::from(self.execution_time),
        );
        m.insert(
            "buy_execution_accum".into(),
            JsonValue::from(self.buy_execution_accum),
        );
        m.insert(
            "buy_execution_time".into(),
            JsonValue::from(self.buy_execution_time),
        );
        m.insert("energy_time".into(), JsonValue::from(self.energy_time));
        m.insert(
            "buy_copper_accum".into(),
            JsonValue::from(self.buy_copper_accum),
        );
        m.insert(
            "buy_copper_time".into(),
            JsonValue::from(self.buy_copper_time),
        );
        m.insert("dress_id".into(), JsonValue::from(self.dress_id));
        m.insert("attr_dress_id".into(), JsonValue::from(self.attr_dress_id));

        let mut d_info = BTreeMap::new();
        for (&fid, &lv) in &self.dress_info {
            d_info.insert(fid.to_string(), JsonValue::from(lv));
        }
        m.insert("dress_info".into(), JsonValue::Object(d_info));

        let mut d_exp = BTreeMap::new();
        for (&fid, &ts) in &self.dress_expire {
            d_exp.insert(fid.to_string(), JsonValue::from(ts));
        }
        m.insert("dress_expire".into(), JsonValue::Object(d_exp));
        m.insert(
            "dress_hero_info".into(),
            JsonValue::Array(
                self.dress_hero_info
                    .iter()
                    .map(|&id| JsonValue::from(id))
                    .collect(),
            ),
        );
        m.insert("avatar_id".into(), JsonValue::from(self.avatar_id));
        m.insert(
            "va_avatar".into(),
            JsonValue::Array(
                self.va_avatar
                    .iter()
                    .map(|&id| JsonValue::from(id))
                    .collect(),
            ),
        );
        m.insert("destiny".into(), JsonValue::from(self.destiny));
        m.insert("destiny_star".into(), JsonValue::from(self.destiny_star));
        m.insert("camp".into(), JsonValue::from(self.camp));
        m.insert("country".into(), JsonValue::from(self.country));
        m.insert("post".into(), JsonValue::from(self.post));
        m.insert("guild_id".into(), JsonValue::from(self.guild_id));
        m.insert("town_id".into(), JsonValue::from(self.town_id));
        m.insert(
            "hero_bag_buy_times".into(),
            JsonValue::from(self.hero_bag_buy_times),
        );
        m.insert(
            "uname_change_num".into(),
            JsonValue::from(self.uname_change_num),
        );
        m.insert("create_time".into(), JsonValue::from(self.create_time));
        m.insert("login_time".into(), JsonValue::from(self.login_time));
        m.insert(
            "last_login_time".into(),
            JsonValue::from(self.last_login_time),
        );
        m.insert(
            "online_accum_time".into(),
            JsonValue::from(self.online_accum_time),
        );
        m.insert("ban_chat_time".into(), JsonValue::from(self.ban_chat_time));
        m.insert(
            "activity_login_give_gold_open_time".into(),
            JsonValue::from(0u64),
        );
        m.insert(
            "server_open_time".into(),
            JsonValue::from(self.server_open_time.clone()),
        );
        m.insert(
            "va_config".into(),
            JsonValue::Object(self.va_config.clone()),
        );
        if !self.copy_progress.is_empty() {
            m.insert(
                "copy_progress".into(),
                JsonValue::Object(self.copy_progress.clone()),
            );
        }
        if !self.act_star_claimed.is_empty() {
            m.insert(
                "act_star_claimed".into(),
                JsonValue::Array(
                    self.act_star_claimed
                        .iter()
                        .map(|&id| JsonValue::from(id))
                        .collect(),
                ),
            );
        }
        if !self.act_star_sign_claimed.is_empty() {
            m.insert(
                "act_star_sign_claimed".into(),
                JsonValue::Array(
                    self.act_star_sign_claimed
                        .iter()
                        .map(|&lv| JsonValue::from(lv))
                        .collect(),
                ),
            );
        }

        let mut heroes_obj = BTreeMap::new();
        for (hid, h) in &self.heroes {
            heroes_obj.insert(hid.to_string(), h.to_vnsg2_hero_info());
        }
        m.insert("heroes".into(), JsonValue::Object(heroes_obj));
        m.insert("formation".into(), self.formation.to_storage_json());
        m.insert("inventory".into(), self.inventory.to_storage_json());
        m.insert("next_hid".into(), JsonValue::from(self.next_hid));
        m.insert("next_item_id".into(), JsonValue::from(self.next_item_id));

        JsonValue::Object(m)
    }

    pub fn from_storage_json(val: &JsonValue) -> Result<Self> {
        let r = JsonObjReader::new(val, "PlayerProfile")?;
        let mut currencies = BTreeMap::new();
        for c in CurrencyType::ALL {
            currencies.insert(c, 0);
        }
        if let Some(curr_obj) = r.opt_obj("currencies")? {
            for (k, v) in curr_obj {
                if let Some(ct) = CurrencyType::from_key(k) {
                    let amount = v
                        .as_u64()
                        .ok_or_else(|| PlayerError::Json(format!("invalid currency '{k}'")))?;
                    currencies.insert(ct, amount);
                }
            }
        }

        let mut dress_info = BTreeMap::new();
        if let Some(di) = r.opt_obj("dress_info")? {
            for (k, v) in di {
                let fid: u32 = k
                    .parse()
                    .map_err(|_| PlayerError::Json("invalid dress_info key".into()))?;
                let lv = v.as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(1);
                dress_info.insert(fid, lv);
            }
        }
        let attr_dress_id = r.opt_u32("attr_dress_id", 1)?;
        dress_info.entry(attr_dress_id).or_insert(1);

        let mut dress_expire = BTreeMap::new();
        if let Some(de) = r.opt_obj("dress_expire")? {
            for (k, v) in de {
                let fid: u32 = k
                    .parse()
                    .map_err(|_| PlayerError::Json("invalid dress_expire key".into()))?;
                let ts = v.as_u64().unwrap_or(0);
                dress_expire.insert(fid, ts);
            }
        }

        let mut dress_hero_info = Vec::new();
        if let Some(dhi) = r.opt_arr("dress_hero_info")? {
            for v in dhi {
                if let Some(id) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    dress_hero_info.push(id);
                }
            }
        }

        let mut va_avatar = Vec::new();
        if let Some(va) = r.opt_arr("va_avatar")? {
            for v in va {
                if let Some(id) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    va_avatar.push(id);
                }
            }
        }

        let mut heroes = BTreeMap::new();
        let heroes_obj = r
            .opt_obj("heroes")?
            .ok_or_else(|| PlayerError::Json("missing heroes object".into()))?;
        for (k, h_val) in heroes_obj {
            let hid: u64 = k
                .parse()
                .map_err(|_| PlayerError::Json("invalid hero key".into()))?;
            let mut h = HeroInstance::from_json(h_val)?;
            h.hid = hid;
            heroes.insert(hid, h);
        }

        let formation = FormationState::from_storage_json(r.req_val("formation")?)?;
        let inventory = InventoryState::from_storage_json(r.req_val("inventory")?)?;
        let copy_progress = r.opt_obj("copy_progress")?.cloned().unwrap_or_default();
        let mut act_star_claimed = Vec::new();
        if let Some(arr) = r.opt_arr("act_star_claimed")? {
            for v in arr {
                if let Some(id) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    if !act_star_claimed.contains(&id) {
                        act_star_claimed.push(id);
                    }
                }
            }
        }
        act_star_claimed.sort_unstable();
        let mut act_star_sign_claimed = Vec::new();
        if let Some(arr) = r.opt_arr("act_star_sign_claimed")? {
            for v in arr {
                if let Some(lv) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    if !act_star_sign_claimed.contains(&lv) {
                        act_star_sign_claimed.push(lv);
                    }
                }
            }
        }
        act_star_sign_claimed.sort_unstable();

        Ok(Self {
            uid: r.req_u64("uid")?,
            pid: r.req_str("pid")?,
            server_id: r.opt_u32("server_id", 2_900_161)?,
            group_id: r.opt_str("group_id")?.unwrap_or_else(|| "2900161".into()),
            uname: r.req_str("uname")?,
            utid: r.req_u32("utid")?,
            protagonist_hid: r.req_u64("protagonist_hid")?,
            level: r.opt_u32("level", 1)?,
            exp: r.opt_u64("exp", 0)?,
            vip: r.opt_u32("vip", 0)?,
            vip_exp: r.opt_u64("vip_exp", 0)?,
            fc: r.opt_u64("fc", 0)?,
            fight_force_max: r.opt_u64("fight_force_max", 0)?,
            is_recharge: r.opt_u8("is_recharge", 0)?,
            gold_recharge_accum_all: r.opt_u64("gold_recharge_accum_all", 0)?,
            currencies,
            execution_time: r.opt_u64("execution_time", 0)?,
            buy_execution_accum: r.opt_u32("buy_execution_accum", 0)?,
            buy_execution_time: r.opt_u64("buy_execution_time", 0)?,
            energy_time: r.opt_u64("energy_time", 0)?,
            buy_copper_accum: r.opt_u32("buy_copper_accum", 0)?,
            buy_copper_time: r.opt_u64("buy_copper_time", 0)?,
            dress_id: r.opt_u32("dress_id", attr_dress_id)?,
            attr_dress_id,
            dress_info,
            dress_expire,
            dress_hero_info,
            avatar_id: r.opt_u32("avatar_id", 0)?,
            va_avatar,
            destiny: r.opt_i32("destiny", -1)?,
            destiny_star: r.opt_u32("destiny_star", 0)?,
            camp: r.opt_u8("camp", 0)?,
            country: r.opt_u8("country", 1)?,
            post: r.opt_u32("post", 0)?,
            guild_id: r.opt_u64("guild_id", 0)?,
            town_id: r.opt_u32("town_id", 1)?,
            hero_bag_buy_times: r.opt_u32("hero_bag_buy_times", 0)?,
            uname_change_num: r.opt_u32("uname_change_num", 0)?,
            create_time: r.opt_u64("create_time", 0)?,
            login_time: r.opt_u64("login_time", 0)?,
            last_login_time: r.opt_u64("last_login_time", 0)?,
            online_accum_time: r.opt_u64("online_accum_time", 0)?,
            ban_chat_time: r.opt_u64("ban_chat_time", 0)?,
            server_open_time: r
                .opt_str("server_open_time")?
                .unwrap_or_else(|| "20261124090000".into()),
            va_config: r.opt_obj("va_config")?.cloned().unwrap_or_default(),
            copy_progress,
            act_star_claimed,
            act_star_sign_claimed,
            heroes,
            formation,
            inventory,
            next_hid: r.opt_u64("next_hid", 10_000_100)?,
            next_item_id: r.opt_u64("next_item_id", 1_000_100)?,
            revision: r.opt_u64("revision", 1)?,
        })
    }
}

// Re-export BagItem for convenience when working with models
pub use crate::bag::BagItem as EquippedItem;
