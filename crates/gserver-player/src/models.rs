use std::collections::BTreeMap;

use crate::bag::InventoryState;
use crate::error::{PlayerError, Result};
use crate::formation::FormationState;
use crate::json::JsonValue;
use crate::wire_helpers::{
    decode_u32_map_lossy, decode_u64_map, decode_u64_map_lossy, encode_map,
};

/// Maximum integer representable without precision loss in Lua 5.1 IEEE-754 `double` (`2^53 - 1`).
pub const MAX_LUA_SAFE_INT: u64 = (1u64 << 53) - 1;

/// Protagonist gender / template selector (`CreateUserScene.lua`: `utid = 1` Male, `utid = 2` Female).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Male = 1,
    Female = 2,
}

impl Gender {
    pub fn from_utid(utid: u32) -> Result<Self> {
        match utid {
            1..=28 if utid % 2 == 1 => Ok(Gender::Male),
            1..=28 => Ok(Gender::Female),
            _ => Err(PlayerError::InvalidParameter(format!(
                "invalid protagonist utid={utid} (expected 1=Male or 2=Female)"
            ))),
        }
    }

    pub const fn default_utid(self) -> u32 {
        self as u32
    }
}

define_wire_enum! {
    /// All currencies tracked in `UserData.lua` (plus `JewelNum` for classic 1st-gen compatibility).
    pub enum CurrencyType {
        Gold => "gold",
        GoldBind => "gold_bind",
        Copper => "copper",
        HeroSoul => "hero_soul",
        Prestige => "prestige",
        Fame => "fame",
        Execution => "execution",
        Energy => "energy",
        JewelNum => "jewel_num",
        ArenaCoin => "arena_coin",
        TowerCoin => "tower_coin",
        BusinessCoin => "business_coin",
        RebelCoin => "rebel_coin",
        Stone => "stone",
        BossCoin => "boss_coin",
        AwakeArmCoin => "awakeArm_coin",
        LordwarCoin => "lordwar_coin",
        GodarmCoin => "godarm_coin",
        PetCoin => "pet_coin",
        GodanimalCoin => "godanimal_coin",
        GodAnimalExploreCoin => "god_animal_explore_coin",
        WorldTeamCoin => "world_team_coin",
        WorldPersonScore => "world_person_score",
        Grain => "grain",
        HeroLegendCoin => "hero_legend_coin",
        LegendBookCoin => "legend_book_coin",
        BrochureCoin => "brochure_coin",
        Strategy => "strategy",
        GuildPoint => "guild_point",
        SpiritCoin => "spirit_coin",
        ClassicWarCoin => "classic_war_coin",
        BingshuCoin => "bingshu_coin",
        DevilCoin => "devil_coin",
        CountryWarCoin => "country_war_coin",
        UnifyCoin => "unify_coin",
        FirstRideCoin => "first_ride_coin",
        SoaringCoin => "soaring_coin",
        QushuiyanCoin => "qushuiyan_coin",
        CrossGuildCoin => "cross_guild_coin",
        CrossGuildTechCoin => "cross_guild_tech_coin",
        VoucherCoin => "voucher_coin",
        TimeMachineVoucherCoin => "time_machine_voucher_coin",
        IronGoldCoin => "iron_gold_coin",
        HunshixilianCoin => "hunshixilian_coin",
        SecretPassScore => "secret_pass_score",
        SkincoinPurple => "skincoin_purple",
        SkincoinOrange => "skincoin_orange",
        SkincoinRed => "skincoin_red",
        SkincoinGold => "skincoin_gold",
        SkincoinDarkgold => "skincoin_darkgold",
    }
    aliases {
        "gold_num" => Gold,
        "silver_num" => Copper,
        "soul_num" => HeroSoul,
        "prestige_num" => Prestige,
        "stamina" => Energy,
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
            companion_heroes: vec![StarterHeroSeed {
                htid: 426,
                level: 1,
                evolve_level: 0,
                auto_deploy_slot: Some(2),
            }],
            starter_items: vec![
                StarterItemSeed { item_template_id: 10_001, count: 20 },
                StarterItemSeed { item_template_id: 20_001, count: 10 },
                StarterItemSeed { item_template_id: 101_101, count: 1 },
                StarterItemSeed { item_template_id: 180_101, count: 1 },
                StarterItemSeed { item_template_id: 50_101, count: 30 },
            ],
        }
    }
}

wire_record! {
    /// Account record persisted in `data/accounts.json` (maps HTTP `/phone/login` `pid` to per-server `uid`s).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Account {
        pid: String = req_str(),
        qid: String = opt_str(""),
        username: String = req_str(),
        auth_token: String = opt_str(""),
        /// Maps `group_id` (e.g. `"2900161"`) to character `uid`.
        characters: BTreeMap<String, u64> = custom(
            encode_map(&this.characters),
            decode_u64_map(r.opt_obj("characters")?, "invalid group_id", "invalid uid in Account.characters")?
        ),
        created_at: u64 = opt_u64(0),
        last_login_at: u64 = opt_u64(0),
        banned_until: u64 = opt_u64(0),
        ban_reason: Option<String> = opt_str_nullable(),
    }
    encode(this, m) => pub fn to_json;
    decode(r, "Account") => pub fn from_json;
}

impl Account {
    pub fn is_banned(&self, now_ts: u64) -> bool {
        self.banned_until > now_ts
    }
}

wire_record! {
    /// Complete hero instance matching `HeroInfo.lua` (`HerosData:parseData`).
    #[derive(Clone, Debug, PartialEq)]
    pub struct HeroInstance {
        hid: u64 = req_u64(),
        htid: u32 = req_u32(),
        dress_id: u32 = opt_u32(0),
        level: u32 = opt_u32(1),
        exp: u64 = opt_u64(0),
        evolve_level: u32 = opt_u32(0),
        upgrade_level: u32 = opt_u32(0),
        is_lock: u8 = opt_u8(0),
        hero_star_pos: u32 = opt_u32(0),
        hero_star_level: u32 = opt_u32(1),
        awake_rank: u32 = opt_u32(0),
        awake_level: u32 = opt_u32(0),
        gold_rank: u32 = opt_u32(0),
        gold_level: u32 = opt_u32(0),
        gold_status: u32 = opt_u32(0),
        dark_gold_rank: u32 = opt_u32(0),
        dark_gold_level: u32 = opt_u32(0),
        purple_gold_rank: u32 = opt_u32(0),
        purple_gold_level: u32 = opt_u32(0),
        color_gold_rank: u32 = opt_u32(0),
        color_gold_level: u32 = opt_u32(0),
        nirvana_gold_rank: u32 = opt_u32(0),
        nirvana_gold_level: u32 = opt_u32(0),
        gild_gold_rank: u32 = opt_u32(0),
        gild_gold_level: u32 = opt_u32(0),
        iron_gold_rank: u32 = opt_u32(0),
        iron_gold_level: u32 = opt_u32(0),
        hero_pill_rank: u32 = opt_u32(0),
        hero_pill_level: u32 = opt_u32(0),
        talent: Vec<u32> = vec_u32(),
        va_fail_reward: BTreeMap<String, JsonValue> = json_obj(),
        va_use_wash: BTreeMap<String, JsonValue> = json_obj(),
        va_last_wash: BTreeMap<String, JsonValue> = json_obj(),
        va_wash_count: BTreeMap<String, JsonValue> = json_obj(),
        va_inlay: BTreeMap<String, JsonValue> = json_obj(),
    }
    encode(this, m) => pub fn to_vnsg2_hero_info {
        m.insert("soul".into(), JsonValue::from(this.exp));
        for k in [
            "va_color_gold_gift",
            "va_nirvana_gold_gift",
            "va_gild_gold_gift",
            "va_iron_gold_gift",
            "equip",
        ] {
            m.insert(k.into(), JsonValue::empty_object());
        }
    };
    decode(r, "HeroInstance") => pub fn from_json;
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

    /// `current_config.db` defines protagonist template IDs as `1..=28` (male odd `1..27`, female even `2..28`).
    pub const fn is_protagonist(&self) -> bool {
        self.htid >= 1 && self.htid <= 28
    }
}

fn encode_storage_currencies(currencies: &BTreeMap<CurrencyType, u64>) -> JsonValue {
    let mut out = BTreeMap::new();
    for c in CurrencyType::ALL {
        let v = currencies.get(&c).copied().unwrap_or(0);
        if v > 0
            || matches!(
                c,
                CurrencyType::Gold
                    | CurrencyType::Copper
                    | CurrencyType::Execution
                    | CurrencyType::Energy
            )
        {
            out.insert(c.vnsg2_key().into(), JsonValue::from(v));
        }
    }
    JsonValue::Object(out)
}

fn decode_storage_currencies(
    opt: Option<&BTreeMap<String, JsonValue>>,
) -> Result<BTreeMap<CurrencyType, u64>> {
    let mut currencies: BTreeMap<CurrencyType, u64> =
        CurrencyType::ALL.into_iter().map(|c| (c, 0)).collect();
    if let Some(curr_obj) = opt {
        for (k, v) in curr_obj {
            if let Some(ct) = CurrencyType::from_key(k) {
                let amount = v
                    .as_u64()
                    .ok_or_else(|| PlayerError::Json(format!("invalid currency '{k}'")))?;
                currencies.insert(ct, amount);
            }
        }
    }
    Ok(currencies)
}

fn encode_heroes_map(heroes: &BTreeMap<u64, HeroInstance>) -> JsonValue {
    JsonValue::Object(
        heroes
            .iter()
            .map(|(hid, h)| (hid.to_string(), h.to_vnsg2_hero_info()))
            .collect(),
    )
}

fn decode_heroes_map(
    opt: Option<&BTreeMap<String, JsonValue>>,
) -> Result<BTreeMap<u64, HeroInstance>> {
    let heroes_obj = opt.ok_or_else(|| PlayerError::Json("missing heroes object".into()))?;
    let mut heroes = BTreeMap::new();
    for (k, h_val) in heroes_obj {
        let hid: u64 = k
            .parse()
            .map_err(|_| PlayerError::Json("invalid hero key".into()))?;
        let mut h = HeroInstance::from_json(h_val)?;
        h.hid = hid;
        heroes.insert(hid, h);
    }
    Ok(heroes)
}

wire_record! {
    /// Complete persistent player state (`UserData.lua` + `HerosData.lua` + `HeroFormationData.lua` + `BagData.lua` + `ConfigData.lua`).
    #[derive(Clone, Debug, PartialEq)]
    pub struct PlayerProfile {
        uid: u64 = req_u64(),
        pid: String = req_str(),
        server_id: u32 = opt_u32(2_900_161),
        group_id: String = opt_str("2900161"),
        uname: String = req_str(),
        utid: u32 = req_u32(),
        protagonist_hid: u64 = req_u64(),
        level: u32 = opt_u32(1),
        exp: u64 = opt_u64(0),
        vip: u32 = opt_u32(0),
        vip_exp: u64 = opt_u64(0),
        fc: u64 = opt_u64(0),
        fight_force_max: u64 = opt_u64(0),
        is_recharge: u8 = opt_u8(0),
        gold_recharge_accum_all: u64 = opt_u64(0),
        currencies: BTreeMap<CurrencyType, u64> = custom(
            encode_storage_currencies(&this.currencies),
            decode_storage_currencies(r.opt_obj("currencies")?)?
        ),
        execution_time: u64 = opt_u64(0),
        buy_execution_accum: u32 = opt_u32(0),
        buy_execution_time: u64 = opt_u64(0),
        energy_time: u64 = opt_u64(0),
        buy_copper_accum: u32 = opt_u32(0),
        buy_copper_time: u64 = opt_u64(0),
        dress_id: u32 = opt_u32(1),
        attr_dress_id: u32 = opt_u32(1),
        dress_info: BTreeMap<u32, u32> = custom(
            encode_map(&this.dress_info),
            decode_u32_map_lossy(r.opt_obj("dress_info")?, "invalid dress_info key", 1)?
        ),
        dress_expire: BTreeMap<u32, u64> = custom(
            encode_map(&this.dress_expire),
            decode_u64_map_lossy(r.opt_obj("dress_expire")?, "invalid dress_expire key", 0)?
        ),
        dress_hero_info: Vec<u32> = vec_u32(),
        avatar_id: u32 = opt_u32(0),
        va_avatar: Vec<u32> = vec_u32(),
        destiny: i32 = opt_i32(-1),
        destiny_star: u32 = opt_u32(0),
        camp: u8 = opt_u8(0),
        country: u8 = opt_u8(1),
        post: u32 = opt_u32(0),
        guild_id: u64 = opt_u64(0),
        town_id: u32 = opt_u32(1),
        hero_bag_buy_times: u32 = opt_u32(0),
        uname_change_num: u32 = opt_u32(0),
        create_time: u64 = opt_u64(0),
        login_time: u64 = opt_u64(0),
        last_login_time: u64 = opt_u64(0),
        online_accum_time: u64 = opt_u64(0),
        ban_chat_time: u64 = opt_u64(0),
        server_open_time: String = opt_str("20261124090000"),
        va_config: BTreeMap<String, JsonValue> = json_obj(),
        heroes: BTreeMap<u64, HeroInstance> = custom(
            encode_heroes_map(&this.heroes),
            decode_heroes_map(r.opt_obj("heroes")?)?
        ),
        formation: FormationState = custom(
            this.formation.to_storage_json(),
            FormationState::from_storage_json(r.req_val("formation")?)?
        ),
        inventory: InventoryState = custom(
            this.inventory.to_storage_json(),
            InventoryState::from_storage_json(r.req_val("inventory")?)?
        ),
        next_hid: u64 = opt_u64(10_000_100),
        next_item_id: u64 = opt_u64(1_000_100),
        revision: u64 = opt_u64(1),
    }
    encode(this, m) => pub fn to_storage_json {
        m.insert("schema_version".into(), JsonValue::from(1u32));
    };
    decode(r, "PlayerProfile") => pub fn from_storage_json (s) {
        if matches!(r.raw().get("dress_id"), None | Some(JsonValue::Null)) {
            s.dress_id = s.attr_dress_id;
        }
        s.dress_info.entry(s.attr_dress_id).or_insert(1);
    };
}

const USER_DATA_CLOSE_FLAGS: [&str; 22] = [
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
];

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

        let mut currencies: BTreeMap<CurrencyType, u64> =
            CurrencyType::ALL.into_iter().map(|c| (c, 0)).collect();
        for (ct, amount) in [
            (CurrencyType::Gold, starter.gold),
            (CurrencyType::GoldBind, starter.gold_bind),
            (CurrencyType::Copper, starter.copper),
            (CurrencyType::HeroSoul, starter.hero_soul),
            (CurrencyType::Prestige, starter.prestige),
            (CurrencyType::JewelNum, starter.jewel_num),
            (CurrencyType::Execution, starter.execution),
            (CurrencyType::Energy, starter.energy),
        ] {
            currencies.insert(ct, amount);
        }

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
            is_recharge: u8::from(starter.initial_vip > 0),
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
            heroes,
            formation: FormationState::with_protagonist(protagonist_hid),
            inventory: InventoryState::new(),
            next_hid: protagonist_hid + 1,
            next_item_id: 1_000_001,
            revision: 1,
        };

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
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(1);
                    let prog = item
                        .va_item_text
                        .get("progress_level")
                        .and_then(JsonValue::as_u64)
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
        wire_record!(@project m, self, copy: [uid, utid, level, vip], clone: [uname]);
        JsonValue::Object(m)
    }

    /// Character summary used by HTTP `/phone/loginrole/` (`live_capture_filtered.log`) and classic `user.getUsers`.
    pub fn to_character_summary(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        wire_record!(
            @project m, self,
            copy: [uid, utid, level, vip, dress_id, last_login_time],
            clone: [uname]
        );
        m.insert("htid".into(), JsonValue::from(self.utid));
        m.insert("quality".into(), JsonValue::from(5u32));
        m.insert("groupid".into(), JsonValue::from(self.group_id.clone()));
        JsonValue::Object(m)
    }

    /// Exact `user.getUser` payload (`UserData:parseData` in `UserData.lua`).
    pub fn to_vnsg2_get_user(&self) -> JsonValue {
        let mut m = BTreeMap::new();
        wire_record!(
            @project m, self,
            copy: [
                uid, server_id, utid, level, exp, vip, vip_exp, execution_time,
                buy_execution_accum, buy_execution_time, energy_time, buy_copper_accum,
                buy_copper_time, fc, is_recharge, gold_recharge_accum_all, dress_id,
                attr_dress_id, avatar_id, hero_bag_buy_times, uname_change_num, destiny,
                destiny_star, camp, country, post, guild_id, town_id, create_time,
                login_time, last_login_time, online_accum_time, ban_chat_time
            ],
            clone: [pid, group_id, uname, server_open_time],
            vec_u32: [dress_hero_info, va_avatar]
        );
        m.insert("htid".into(), JsonValue::from(self.utid));
        m.insert("exp_num".into(), JsonValue::from(self.exp));

        let copper = self.currency(CurrencyType::Copper);
        m.insert("copper".into(), JsonValue::from(copper.to_string()));
        m.insert("silver_num".into(), JsonValue::from(copper));
        for c in CurrencyType::ALL {
            if c != CurrencyType::Copper {
                m.insert(c.vnsg2_key().into(), JsonValue::from(self.currency(c)));
            }
        }
        for (alias, ct) in [
            ("gold_num", CurrencyType::Gold),
            ("soul_num", CurrencyType::HeroSoul),
            ("prestige_num", CurrencyType::Prestige),
            ("stamina", CurrencyType::Energy),
        ] {
            m.insert(alias.into(), JsonValue::from(self.currency(ct)));
        }
        m.insert("stamina_time".into(), JsonValue::from(self.energy_time));
        m.insert("fight_force".into(), JsonValue::from(self.fc));
        m.insert("fight_force_max".into(), JsonValue::from(self.fight_force_max.to_string()));

        let d_info = encode_map(&self.dress_info);
        m.insert("dress_info".into(), d_info.clone());
        m.insert("dress".into(), d_info);
        m.insert("dress_expire".into(), encode_map(&self.dress_expire));

        let mut va_recharge = BTreeMap::new();
        va_recharge.insert("first_recharge".into(), JsonValue::empty_object());
        va_recharge.insert("recommend_recharge".into(), JsonValue::empty_object());
        m.insert("va_recharge".into(), JsonValue::Object(va_recharge));

        let mut va_user = BTreeMap::new();
        va_user.insert("dress".into(), JsonValue::from(self.dress_id));
        va_user.insert("va_config".into(), JsonValue::Object(self.va_config.clone()));
        m.insert("va_user".into(), JsonValue::Object(va_user));

        m.insert("hero_bag_used".into(), JsonValue::from(self.heroes.len() as u32));
        for close_flag in USER_DATA_CLOSE_FLAGS {
            m.insert(close_flag.into(), JsonValue::from(false));
        }
        JsonValue::Object(m)
    }

    /// Exact `user.getVaConfig` payload (`ConfigData.lua`).
    pub fn to_vnsg2_va_config(&self) -> JsonValue {
        JsonValue::Object(self.va_config.clone())
    }
}

pub use crate::bag::BagItem as EquippedItem;
