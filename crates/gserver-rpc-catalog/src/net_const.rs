//! Complete `NetConst` RPC catalog extracted verbatim from decrypted `vnsg2` `NetConst.lua`
//! (2,107 constants across 6,567 lines) and `NetConst.lua` Time-Machine mirror (`tm`, 2,021 constants),
//! plus inline RPC string literals invoked directly by `Network.lua`, `BarrageProxy.lua`, and `HalidomData.lua`.

#![allow(non_upper_case_globals)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RpcDirection {
    ClientToServer,
    ServerPush,
    RewardOrEventPush,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetConstEntry {
    pub const_name: &'static str,
    pub method: &'static str,
    pub line: u32,
    pub direction: RpcDirection,
}

const fn infer_direction(const_name: &str, method: &str) -> RpcDirection {
    let (n, m) = (const_name.as_bytes(), method.as_bytes());
    if n.len() >= 3 && n[0] == b'C' && n[1] == b'S' && n[2] == b'_' {
        RpcDirection::ClientToServer
    } else if m.len() >= 3 && m[0] == b'r' && m[1] == b'e' && m[2] == b'.' {
        RpcDirection::RewardOrEventPush
    } else if n.len() >= 3 && n[0] == b'S' && n[1] == b'C' && n[2] == b'_' {
        RpcDirection::ServerPush
    } else {
        RpcDirection::Other
    }
}

const fn entry(const_name: &'static str, method: &'static str, line: u32) -> NetConstEntry {
    NetConstEntry { const_name, method, line, direction: infer_direction(const_name, method) }
}

const fn build_catalog<const N: usize>(slices: &[&[NetConstEntry]]) -> [NetConstEntry; N] {
    let mut out = [entry("", "", 0); N];
    let (mut idx, mut s) = (0, 0);
    while s < slices.len() {
        let sl = slices[s];
        let mut k = 0;
        while k < sl.len() {
            out[idx] = sl[k];
            idx += 1;
            k += 1;
        }
        s += 1;
    }
    assert!(idx == N);
    let gaps = [701usize, 301, 132, 57, 23, 10, 4, 1];
    let mut g = 0;
    while g < gaps.len() {
        let gap = gaps[g];
        let mut i = gap;
        while i < N {
            let cur = out[i];
            let mut j = i;
            while j >= gap && out[j - gap].line > cur.line {
                out[j] = out[j - gap];
                j -= gap;
            }
            out[j] = cur;
            i += 1;
        }
        g += 1;
    }
    out
}

macro_rules! emit_prefixed_consts {
    ($p:literal : [ $( $id:ident($act:literal, $($rest:literal),+) ),+ $(,)? ]) => {
        $( pub const $id: &str = concat!($p, ".", $act); )+
    };
}

macro_rules! prefixed_entries {
    (norm $p:literal : [ $( $id:ident($act:literal, $nln:literal, $tln:literal) ),+ $(,)? ]) => {
        &[ $( entry(stringify!($id), concat!($p, ".", $act), $nln) ),+ ]
    };
    (tm $p:literal : [ $( $id:ident($act:literal, $nln:literal, $tln:literal) ),+ $(,)? ]) => {
        &[ $( entry(stringify!($id), concat!($p, ".", $act), $tln) ),+ ]
    };
    (single $p:literal : [ $( $id:ident($act:literal, $ln:literal) ),+ $(,)? ]) => {
        &[ $( entry(stringify!($id), concat!($p, ".", $act), $ln) ),+ ]
    };
}

macro_rules! define_rpc_consts {
    (
        paired_groups: [ $( ($np:literal, $tp:literal) => $pg:tt ),* $(,)? ],
        paired_raw: [ $( $pr_id:ident($pr_nm:literal, $pr_nln:literal, $pr_tm:literal, $pr_tln:literal) ),* $(,)? ],
        normal_groups: [ $( $ng_p:literal => $ng:tt ),* $(,)? ],
        normal_raw: [ $( $nr_id:ident($nr_m:literal, $nr_ln:literal) ),* $(,)? ],
        tm_groups: [ $( $tg_p:literal => $tg:tt ),* $(,)? ],
        tm_raw: [ $( $tr_id:ident($tr_m:literal, $tr_ln:literal) ),* $(,)? ],
    ) => {
        $( emit_prefixed_consts!($np : $pg); )*
        $( pub const $pr_id: &str = $pr_nm; )*
        $( emit_prefixed_consts!($ng_p : $ng); )*
        $( pub const $nr_id: &str = $nr_m; )*

        const SORTED_NET_CONSTS: [NetConstEntry; NET_CONST_COUNT] = build_catalog(&[
            $( prefixed_entries!(norm $np : $pg), )*
            &[ $( entry(stringify!($pr_id), $pr_nm, $pr_nln), )* ],
            $( prefixed_entries!(single $ng_p : $ng), )*
            &[ $( entry(stringify!($nr_id), $nr_m, $nr_ln), )* ],
        ]);
        pub const ALL_NET_CONSTS: &[NetConstEntry] = &SORTED_NET_CONSTS;

        pub mod tm {
            use super::{build_catalog, entry, NetConstEntry};
            pub const NET_CONST_TM_COUNT: usize = 2021;
            $( emit_prefixed_consts!($tp : $pg); )*
            $( pub const $pr_id: &str = $pr_tm; )*
            $( emit_prefixed_consts!($tg_p : $tg); )*
            $( pub const $tr_id: &str = $tr_m; )*

            const SORTED_TM_NET_CONSTS: [NetConstEntry; NET_CONST_TM_COUNT] = build_catalog(&[
                $( prefixed_entries!(tm $tp : $pg), )*
                &[ $( entry(stringify!($pr_id), $pr_tm, $pr_tln), )* ],
                $( prefixed_entries!(single $tg_p : $tg), )*
                &[ $( entry(stringify!($tr_id), $tr_m, $tr_ln), )* ],
            ]);
            pub const ALL_TM_NET_CONSTS: &[NetConstEntry] = &SORTED_TM_NET_CONSTS;
        }
    };
}

pub const NET_CONST_COUNT: usize = 2107;

define_rpc_consts! {
    paired_groups: [
        ("user", "tmUser") => [
            CS_USER_BEFORE_LOGIN_FIGHT("beforLoginBattle", 15, 14), SC_USER_UPDATEUSER("updateUser", 23, 23),
            CS_GET_USER_BEFRO_LOGIN("getUserBeforLogin", 27, 27), CS_USER_USERLOGIN("userLogin", 32, 32),
            CS_USER_CHANGE_U_NAME("changeUname", 47, 47), CS_USER_BUYCOPPER("buyCopper", 53, 53),
            CS_USER_CREATEUSER("createUser", 59, 59), CS_USER_BUYVIPGIFT("buyVipGift", 63, 63),
            CS_USER_VIPGIFTDATA("getVipGiftInfo", 67, 67), CS_USER_GET_RANDOM_NAME("getRandomName", 74, 74),
            CS_USER_OPENHEROBAG("openHeroBag", 80, 80)],
        ("user", "user") => [
            CS_GET_USER("getUser", 19, 19), CS_USER_SET_VA_CONFIG("setVaConfig", 37, 37),
            CS_USER_GET_VA_CONFIG("getVaConfig", 42, 42)],
        ("bag", "tmBag") => [
            CS_GET_BAG_DATA("bagInfo", 86, 86), CS_BAG_PUSH_UPDATE("update", 92, 93),
            CS_BAG_OPEN_GRID_BY_GOLD("openGridByGold", 97, 98), CS_BAG_SELL_ITEM("sellItems", 102, 103),
            CS_BAG_TAKE_TMP_BAG("takeTmpBag", 152, 151)],
        ("exchange", "tmExchange") => [
            CS_BAG_USE_ITEM("useItemInBag", 107, 108), CS_BAG_COMPOSE_HERO("composeHero", 112, 113),
            CS_BAG_COMPOSE_ARM("composeArm", 117, 118), CS_BAG_COMPOSE_GOD_ANIMAL("composeGodAnimal", 122, 123),
            CS_BAG_COMPOSE_GOD("composeGodArm", 129, 128), CS_BAG_COMPOSE_AWAKE("composeAwake", 134, 133),
            CS_BAG_COMPOSE_PET("composePet", 139, 138), CS_BAG_COMPOSE_TREASURE("composeTreasureByFragment", 144, 143),
            CS_BAG_COMPOSE_YANWU("composeYanWu", 147, 146), CS_EXCHANGE_EXCHANGEAWAKE("exchangeAwake", 539, 540),
            CS_SHOP_USE("useItemInBagByTpl", 928, 925), CS_ACTIVITY_GET_OPEN_BOX_INFO("getOpenBoxInfo", 1601, 1585),
            CS_ACTIVITY_EXCHANGE_OPEN_BOX_BY_KEY("openRandgiftBoxByKey", 1606, 1590),
            CS_ACTIVITY_EXCHANGE_OPEN_BOX_BY_GOLD("openRandgiftBoxByGold", 1611, 1595)],
        ("refinery", "tmRefinery") => [
            CS_DECOMPOSE_REBORNARM("rebornArm", 159, 158), CS_DECOMPOSE_REBORNTREASURE("rebornTreasure", 164, 163),
            CS_DECOMPOSE_REBORNHERO("rebornHero", 169, 168), CS_REFINERY_REFINERYARM("refineryArm", 174, 173),
            CS_REFINERY_REFINERYHERO("refineryHeros", 179, 178), CS_REFINERY_REFINERYAWAKE("refineryAwake", 184, 183),
            CS_REFINERY_REFINERYPET("refineryPet", 189, 188), CS_REFINERY_REFINERYSPIRIT("refinerySpirit", 194, 193),
            CS_REFINERY_REFINERYYANWU("refineryYanWu", 197, 196), CS_DECOMPOSE_REBORNPET("rebornPet", 202, 201),
            CS_DECOMPOSE_REBORNGOD("rebornGodArm", 207, 206), CS_DECOMPOSE_REFINERYGOD("refineryGodArm", 212, 211),
            CS_DECOMPOSE_REBORNAWAKE("rebornAwake", 217, 216), CS_DECOMPOSE_REBORNSPIRIT("rebornSpirit", 222, 221),
            CS_REFINERY_REFINERYARMFRAGMENT("refineryFragment", 227, 226), CS_REFINERY_REFINERYSOUL("refinerySoul", 232, 231),
            CS_DECOMPOSE_REFINERYGODANIMAL("refineryGodAnimal", 237, 236), CS_DECOMPOSE_REBORNGODANIMAL("rebornGodAnimal", 242, 241),
            SC_SPIRIT_REFINING("resolveSpirit", 4864, 4834), CS_SOULSKILL_REFINERY("refinerySoulSkill", 5229, 5199),
            CS_SOULSKILL_REBORN("rebornSoulSkill", 5230, 5200)],
        ("mail", "tmMail") => [
            CS_GET_MAIL_DATA("getMailBoxList", 257, 254), CS_MAIL_READALLMAIL("readAllMail", 261, 258),
            CS_SEND_MAIL("sendMail", 271, 268)],
        ("battle", "battle") => [
            CS_GET_BATTLEINFO("testGetBattleInfo", 303, 300), CS_BATTLE_GETRECORD("getRecord", 1822, 1789),
            CS_BATTLE_GET_ARENACS_RECORD("getWorldArenaRecord", 1827, 1794)],
        ("arena", "tmArena") => [
            CS_ARENA_GET_BASE_DATA("getBaseData", 312, 309), CS_ARENA_BUY_CHALLENGE_TIMES("buyChallengeTimes", 314, 311),
            CS_ARENA_CHALLENGE("challenge", 316, 313), CS_ARENA_ENTARN_ARENA("enterArena", 320, 317),
            CS_ARENA_QUICK_CHALLENGE("quickChallenge", 322, 319), CS_ARENA_SEND_RANK_REWARD("sendRankReward", 324, 321),
            CS_ARENA_GET_TOP_REPORTS("getTopReports", 328, 325), CS_ARENA_LEAVE_ARENA("leaveArena", 330, 327),
            CS_ARENA_FETCH_NUM_REWARD("fetchChallengeNumReward", 332, 329)],
        ("worldarena", "tmWorldarena") => [
            CS_ARENACS_GET_STATE("getWorldArenaState", 340, 337), CS_ARENACS_GET_BASE_INFO("getWorldArenaInfo", 344, 341),
            CS_ARENACS_SIGN_UP("signUp", 348, 345), CS_ARENACS_UPDATE_FMT("updateFmt", 352, 349),
            CS_ARENACS_ATTACK("attack", 356, 353), CS_ARENACS_BUY_ATK_NUM("buyAtkNum", 360, 357),
            CS_ARENACS_RESET("reset", 364, 361), CS_ARENACS_GET_RECORD_LIST("getRecordList", 368, 365),
            CS_ARENACS_GET_RANK_LIST("getRankList", 372, 369), CS_ARENACS_GET_ACCOUNT_DATA("getAccountData", 376, 373),
            CS_ARENACS_LEAVE_WORLD_ARENA("leaveWorldArena", 380, 377), CS_ARENACS_GET_ALL_RANK("getAllRank", 384, 381),
            CS_ARENACS_FETCH_REWARD("fetchReward", 388, 385), CS_ARENACS_GET_FORMATION("getFormation", 392, 389)],
        ("re.worldarena", "tmRe.worldarena") => [
            SC_ARENACS_SEND_MSG("sendMsg", 396, 393), SC_ARENACS_BE_DEFEATED("beDefeated", 400, 397)],
        ("robtreasure", "tmRobtreasure") => [
            CS_ROBTREASURE_GET_BASE_DATA("getBaseData", 405, 402),
            CS_ROBTREASURE_GET_ROB_TREASURE_LIST("getRobTreasureList", 407, 404),
            CS_ROBTREASURE_ROB_TREASURE("robTreasure", 409, 406), CS_ROBTREASURE_ROB_TREASURE_NUM("robTreasureNum", 411, 408),
            CS_ROBTREASURE_SEND_RESULT("sendResult", 413, 410), CS_ROBTREASURE_FUSE("fuse", 415, 412),
            CS_ROBTREASURE_REFUSE_WAR("refuseWar", 417, 414), CS_ROBTREASURE_QUICK_ROB("quickRob", 419, 416),
            SC_ROBTREASURE_ADD_FRAGMENT("addfragment", 425, 422)],
        ("hero", "tmHero") => [
            CS_GET_ALL_HERO("getAllHeroes", 432, 429), CS_HERO_ADD_ARMING("addArming", 435, 432),
            CS_HERO_ADD_MULTI_ARMING("addMultiArming", 438, 435), CS_HERO_REMOVE_ARMING("removeArming", 441, 438),
            CS_HERO_TRANSFER_ARMING("transferArming", 444, 441), CS_HERO_SKILL_LEVELUP("skillLevelUp", 447, 444),
            CS_HERO_LEVELUP("heroLevelUp", 451, 448)],
        ("evolve", "tmEvolve") => [CS_EVOLVE_DOEVOLVE("doEvolve", 454, 451), CS_EVOLVE_SURMOUNT("surmount", 457, 456)],
        ("herostar", "tmHerostar") => [
            CS_HERO_STAR_LEVELUP("levelUp", 461, 460), CS_HERO_STAR_AUTO_LEVELUP("oneKeylevelUp", 465, 464)],
        ("forge", "tmForge") => [
            CS_FORGE_REINFORCE("reinforceArm", 469, 468), CS_FORGE_AUTO_REINFORCE("autoReinforceArm", 472, 471),
            CS_FORGE_UPGRADE("upgradeArm", 475, 474), CS_FORGE_UPGRADE_LEVEL("upgradeArmUpLevel", 479, 478),
            CS_ARM_WASH("armWashRefresh", 484, 483), CS_ARM_WASH_CANCEL("armWashCancel", 489, 488),
            CS_ARM_WASH_APPLY("armWashApply", 494, 493), CS_ARM_TRANSLATE("armWashTransfer", 499, 498),
            CS_ARM_AUTO_WASH("armOneKeyWash", 503, 502), CS_ARM_LOCK("armRefineryLock", 508, 507),
            CS_ARM_UNLOCK("armRefineryUnLock", 513, 512), CS_FORGE_REINFORCE_TREASURE("reinforceTreasure", 518, 517),
            CS_FORGE_UPGRADE_TREASURE("upgradeTreasure", 522, 521), CS_FORGE_UPGRADE_AWAKE("upgradeAwake", 526, 526),
            CS_SPIRIT_REFINE("upgradeSpirit", 529, 530), CS_YANWU_REINFORCEYANWU("reinforceYanWu", 533, 534),
            CS_YANWU_PROGRESSYANWU("progressYanWu", 535, 536), CS_FORGE_REINFORCE_PET("reinforcePet", 3163, 3133),
            CS_FORGE_UPGRADE_PET("upgradePet", 3168, 3138), CS_FORGE_BREAK_PET("breakPet", 3173, 3143),
            CS_GOD_ANIMAL_REINFORCE("reinforceGodAnimal", 3187, 3157), CS_GOD_ANIMAL_STAR("upgradeStarGodAnimal", 3192, 3162),
            CS_GOD_ANIMAL_PROGRESS("upgradeGodAnimal", 3197, 3167), CS_GODARM_WASH("godArmWash", 3470, 3440),
            CS_GODARM_WASH_SAVE("godArmSaveWash", 3472, 3442), CS_GODARM_WASH_CANCEL("godArmCancelWash", 3474, 3444),
            CS_GODARM_WASH_ONEKEY("godArmOneKeyWash", 3476, 3446), CS_GODARM_UPGRADE("godArmUpgrade", 3478, 3448),
            SC_SPIRIT_ONE_KEY_LEVELUP("reinforceSpiritOneKey", 4866, 4836), CS_SOULSKILL_COMPOSE("composeSoulSkill", 5221, 5191),
            CS_SOULSKILL_LEVEL_UP("reinforceSoulSkill", 5225, 5195), CS_SOULSKILL_STAR_UP("upgradeSoulSkill", 5226, 5196),
            CS_SOULSKILL_AWAKE_UP("awakeSoulSkill", 5227, 5197), CS_SOULSTONE_UPGRADE("upgradeSoulStone", 6006, 6134),
            CS_SOULSTONE_YANGHUN("upgradeSoulStoneStar", 6008, 6136), CS_SOULSTONE_WASH("washSoulStone", 6009, 6137),
            CS_SOULSTONE_WASH_LOCK("lockSoulStoneAttr", 6010, 6138), CS_SOULSTONE_WASH_UNLOCK("unlockSoulStoneAttr", 6011, 6139),
            CS_SOULSTONE_STAR("upgradeSoulStoneStar", 6012, 6140), CS_SOULSTONE_DIRECT_WASH("directionalWashSoulStone", 6013, 6141),
            CS_SOULSTONE_SOULPOW_WASH("upgradeSoulPowerGift", 6014, 6142),
            CS_SOULSTONE_WASH_UNLOCK_BY_ITEM("unlockSoulStoneWashPositionByItem", 6015, 6143),
            CS_SOULJADE_UPGRADE("upgradeSoulJade", 6245, 6309), CS_SOULJADE_YANGHUN("upgradeSoulJadeStar", 6247, 6311),
            CS_SOULJADE_WASH("washSoulJade", 6248, 6312), CS_SOULJADE_WASH_LOCK("lockSoulJadeAttr", 6249, 6313),
            CS_SOULJADE_WASH_UNLOCK("unlockSoulJadeAttr", 6250, 6314), CS_SOULJADE_STAR("upgradeSoulJadeStar", 6251, 6315),
            CS_SOULJADE_DIRECT_WASH("directionalWashSoulJade", 6252, 6316),
            CS_SOULJADE_SOULPOW_WASH("upgradeSoulJadeQiGift", 6253, 6317),
            CS_SOULJADE_WASH_UNLOCK_BY_ITEM("unlockSoulJadeWashPositionByItem", 6254, 6318)],
        ("develop", "tmDevelop") => [
            CS_DEVELOP_GETDEVELOP("getDevelop", 546, 547), CS_DEVELOP_DODEVELOP("doDevelop", 549, 550),
            CS_DEVELOP_SAVE("save", 552, 553)],
        ("formation", "tmFormation") => [
            CS_FORMATION_GET("get", 559, 560), CS_FORMATION_SET("set", 563, 564), CS_FORMATION_GET_HEROS("getHeroes", 566, 567),
            CS_FORMATION_SET_HERO_TO_POSITION("setHeroToPosition", 569, 570),
            CS_FORMATION_GET_POSITION_INFO("getPositionInfo", 572, 573), CS_FORMATION_ADD_ARM("addArming", 575, 576),
            CS_FORMATION_ADD_MULTI_ARM("addMultiArming", 578, 579), CS_FORMATION_REMOVE_ARM("removeArming", 581, 582),
            CS_FORMATION_TRANSFER_ARM("transferArming", 584, 585), CS_FORMATION_SET_PARTNER("setPartner", 587, 588),
            CS_FORMATION_HEROENCOURAGE_QUICK_LEVELUP("morePowerAutoLvUp", 591, 592),
            CS_FORMATION_HEROENCOURAGE_UNLOCK("activateMorePower", 595, 596),
            CS_FORMATION_GET_OTHER_FORMATIONINFO("getOtherPositionInfo", 599, 600),
            CS_FORMATION_GOD_ANIMAL_ADD("addGodAnimal", 626, 627), CS_FORMATION_GOD_ANIMAL_REMOVE("removeGodAnimal", 631, 632),
            CS_FORMATION_REMOVE_ALL_PARTNERS("unsetMultiPartner", 636, 637),
            CS_GET_OTHER_SERVER_FORMATION("getOtherServerPositionInfo", 5893, 6145)],
        ("animal", "tmAnimal") => [
            CS_ANIMAL_CREATE_ANIMAL("createAnimalByGold", 603, 604), CS_ANIMAL_GET_ALL_ANIMAL("getAllAnimals", 606, 607),
            CS_ANIMAL_ATTACK_ANIMAL("attackAnimal", 609, 610), CS_ANIMAL_FOR_HELP("forHelp", 612, 613)],
        ("re.animal", "tmRe.animal") => [SC_ANIMAL_FOR_HELP("forHelp", 616, 617), SC_ANIMAL_CREATE_ANIMAL("createAnimal", 620, 621)],
        ("chat", "tmChat") => [
            CS_CHAT_PERSONAL_SEND("sendPersonal", 640, 641), CS_CHAT_PERSONAL_GET("getPersonalLastMsgs", 641, 642),
            CS_CHAT_WORLD_SEND("sendWorld", 644, 645), CS_CHAT_WORLD_GET("getWorldLastMsgs", 645, 646),
            CS_CHAT_GUILD_SEND("sendGuild", 648, 649), CS_CHAT_GUILD_GET("getGuildLastMsgs", 649, 650),
            CS_CHAT_GUILD_CROSS_SERVER_SEND("sendCrossGuild", 652, 653),
            CS_CHAT_GUILD_CROSS_SERVER_GET("getCrossGuildLastMsgs", 653, 654), CS_CHAT_COUNTRY_SEND("sendCountry", 660, 657),
            CS_CHAT_COUNTRY_GET("getCountryLastMsgs", 661, 658), CS_CHAT_CHAT_CROSS_SERVER_SEND("sendCrossGuildTeam", 6059, 6192),
            CS_CHAT_CHAT_CROSS_SERVER_GET("getCrossGuildTeamLastMsgs", 6060, 6193)],
        ("envelope", "tmEnvelope") => [CS_ENVELOPE_SEND("send", 671, 666), CS_ENVELOPE_OPEN("open", 674, 669)],
        ("playerTitle", "tmPlayerTitle") => [
            CS_PLAYER_TITLE_GET_ALL("getAllPlayerTitle", 681, 676), CS_PLAYER_TITLE_EQUIP("equip", 684, 679),
            CS_PLAYER_TITLE_EQUIP_ATTR("equipAttr", 687, 682), CS_PLAYER_TITLE_ACTIVATE("activate", 690, 685)],
        ("sc.achievements", "sc.tmAchievements") => [
            SC_PLAYER_TITLE_ACHIEVEMENT_CHG("getNewAchievement_player_title", 693, 688),
            SC_ACHIEVEMENT_UPDATE("getNewAchievement", 854, 851),
            SC_ACTIVITY_GETNEWACHIEVEMENT_OPEN_ONE("getNewAchievement_open_one", 1550, 1534)],
        ("heroTitle", "tmHeroTitle") => [
            CS_HERO_TITLE_GET_ALL("getAllHeroTitle", 697, 692), CS_HERO_TITLE_EQUIP("equip", 700, 695),
            CS_HERO_TITLE_EQUIP_ATTR("equipAttr", 703, 698), CS_HERO_TITLE_ACTIVATE("activate", 706, 701)],
        ("copy", "timeMachineCopy") => [
            CS_COPY_LIST("getList", 714, 709), CS_COPY_CHAPTERINFO("getCopy", 718, 713), CS_COPY_ATTACK("attack", 722, 717),
            CS_COPY_PASS("pass", 726, 721), CS_COPY_FAIL("battleFail", 730, 725), CS_COPY_SWEEP("sweep", 734, 729),
            CS_COPY_RESET_ATK_NUM("resetAtk", 738, 733), CS_COPY_ONE_KEY_RESET_ATK("oneKeyResetAtk", 743, 738),
            CS_COPY_GET_REWARD("fetchPrize", 748, 743), CS_COPY_GET_FIGHT_RECORD("getStronghRecord", 752, 747),
            CS_COPY_GET_POINT_BOX_REWARD("fetchBoxReward", 756, 751), CS_COPY_FETCH_FRESHER_REWARD("fetchFresherReward", 761, 756)],
        ("copy_daily", "tmCopyDaily") => [
            CS_DAILY_COPY_GET_INFO("getDailyCopyInfo", 766, 763), CS_DAILY_COPY_FIGHT("attack", 770, 767),
            CS_DAILY_COPY_PASS("pass", 774, 771), CS_DAILY_COPY_SWEEP("sweep", 783, 780)],
        ("task", "tmTask") => [
            CS_TASK_LIST("getAllTask", 792, 789), CS_TASK_COMPLETE("complete", 796, 793), SC_TASK_UPDATE("updateTask", 800, 797),
            CS_TASK_ACCEPT("accept", 804, 801), CS_TASK_DO("npcVisit", 808, 805)],
        ("dailytask", "tmDailytask") => [
            CS_DAYTASK_LIST("getInfo", 813, 810), CS_DAYTASK_COMPLETE("receiveTask", 817, 814),
            SC_DAYTASK_UPDATE("updateTask", 821, 818), CS_DAYTASK_RECEIVE_ACTIVE_POINT("receiveActivePoint", 825, 822),
            CS_DAYTASK_QUICK_FETCH("receiveAllTask", 830, 827)],
        ("achievements", "tmAchievements") => [
            CS_ACHIEVEMENT_INFO("getShowAchievements", 838, 835), CS_GET_ACHIEVEMENT_REWARD("receiveAchievementPrize", 842, 839),
            CS_ACHIEVEMENT_BOX_INFO("getShowAchievementsPointReward", 846, 843),
            CS_GET_ACHIEVEMENT_BOX_REWARD("receiveAchievementPointPrize", 850, 847)],
        ("friend", "tmFriend") => [
            CS_FRIEND_GETINFO("getFriendInfo", 859, 856), CS_FRIEND_ADDFRIEND("addFriend", 861, 858),
            CS_FRIEND_ADDBLACK("addBlack", 863, 860), CS_FRIEND_GETRECOMMENDFRIENDLIST("getRecommendFriendList", 865, 862),
            CS_FRIEND_GETREQUESTFRIENDLIST("getRequestFriendList", 867, 864), CS_FRIEND_DELFRIEND("delFriend", 869, 866),
            CS_FRIEND_REQUESTFRIEND("requestFriend", 871, 868), CS_FRIEND_REFUSEFRIEND("refuseFriend", 873, 870),
            CS_FRIEND_DELBLACK("delBlack", 875, 872), CS_FRIEND_GETBLACKLIST("getBlackList", 877, 874),
            CS_FRIEND_SENDMSG("sendMsg", 879, 876), CS_FRIEND_FINDFRIEND("findFriend", 881, 878),
            SC_FRIEND_NOTIFYRE("notifyrs", 883, 880), SC_FRIEND_NOTIFY("notify", 885, 882),
            CS_FRIEND_REQUEST_FRIEND_BY_UNAME("requestFriendByName", 899, 896), CS_FRIEND_USER_ATK("userAtk", 902, 899),
            CS_FRIEND_REFUSE_ALL_FRIEND("refuseAllFriend", 904, 901), CS_USER_ATK_OTHER_SERVER("userAtkOtherServer", 5894, 6146)],
        ("friendEnergy", "tmFriendEnergy") => [
            CS_FRIEND_GET_RECEIVE_ENERGY_LIST("getReceiveEnergyList", 889, 886), CS_FRIEND_PRESENT_ENERGY("presentEnergy", 891, 888),
            CS_FRIEND_PRESENT_ENERGY_FOR_LIST("presentEnergyForList", 893, 890), CS_FRIEND_RECEIVE_ENERGY("receiveEnergy", 895, 892),
            CS_FRIEND_ONE_KEY_RECEIVE_AND_PRESENT("oneKeyReceiveAndPresent", 897, 894)],
        ("shop", "tmShop") => [
            CS_SHOP_GETINFO("getInfo", 913, 910), CS_SHOP_BUY("buy", 918, 915), CS_SHOP_REFRESH("refresh", 923, 920),
            CS_SHOP_PUSH_UPDATE("open", 934, 931)],
        ("rob", "tmRob") => [
            CS_PLUNDER_GET_BASE_DATA("getBaseData", 958, 955), CS_PLUNDER_ENTER_CASTLE("enterCastle", 960, 957),
            CS_PLUNDER_LEAVE_CASTLE("leaveCastle", 962, 959), CS_PLUNDER_SEARCH("search", 964, 961),
            CS_PLUNDER_BUY_ROB_TIMES("buyRobTimes", 966, 963), CS_PLUNDER_START_ROB("startRob", 968, 965),
            CS_PLUNDER_REVENGE("revenge", 972, 969), CS_PLUNDER_GET_BATTLE_RECORD("getBattleRecord", 974, 971),
            SC_PLUNDER_NOTIFY("notify", 976, 973)],
        ("castle", "tmCastle") => [
            CS_TOWN_LIST("getInfo", 985, 982), CS_TOWN_BUILD_BUILDING("build", 989, 986),
            CS_TOWN_MOVE_BUILDING("setExtra", 993, 990), CS_TOWN_BUILDING_LEVEL_UP("levelUp", 997, 994),
            CS_TOWN_BUILDING_DESTROY("destroy", 1001, 998), CS_TOWN_BUILDING_FORCE_LEVEL_UP("forceLevelUp", 1005, 1002),
            CS_TOWN_BUILDING_CLEAR_LEVEL_UP_CD("buyLvupCD", 1009, 1006), CS_TOWN_BUILDING_BUY_QUEUE("buyBuildQueue", 1013, 1010)],
        ("hall", "tmHall") => [
            CS_TOWN_BUILDING_MAINBASE_INFO("getInfo", 1018, 1015), CS_TOWN_BUILDING_MAINBASE_GAIN("takeStone", 1022, 1019),
            CS_TOWN_BUILDING_MAINBASE_LEVY("tax", 1026, 1023), CS_TOWN_BUILDING_MAINBASE_FORCE_LEVY("forceTax", 1030, 1027),
            CS_TOWN_BUILDING_MAINBASE_AFFAIR_INFO("getChore", 1034, 1031),
            CS_TOWN_BUILDING_MAINBASE_DEAL_AFFAIR("dealChore", 1038, 1035)],
        ("workshop", "tmWorkshop") => [
            CS_TOWN_BUILDING_WORKSHOP_INFO("getInfo", 1043, 1040), CS_TOWN_BUILDING_WORKSHOP_SELL("sellItem", 1047, 1044),
            CS_TOWN_BUILDING_WORKSHOP_GAIN("takeItems", 1051, 1048),
            CS_TOWN_BUILDING_WORKSHOP_CHANG_PRODUCE_ITEM("changeProduceItem", 1055, 1052),
            CS_TOWN_BUILDING_WORKSHOP_PRODUCE("produce", 1059, 1056),
            CS_TOWN_BUILDING_WORKSHOP_FORCE_PRODUCE("forceProduce", 1063, 1060)],
        ("market", "tmMarket") => [
            CS_TOWN_BUILDING_MARKET_GET_INFO("getInfo", 1067, 1064), CS_TOWN_BUILDING_MARKET_GAIN("takeCopper", 1071, 1068)],
        ("smithy", "tmSmithy") => [
            CS_TOWN_BUILDING_SMITHY_INFO("getInfo", 1075, 1072), CS_TOWN_BUILDING_SMITHY_PAPER_MAKE("bluePrintProduce", 1079, 1076),
            CS_TOWN_BUILDING_SMITHY_FORGE("forgeProduce", 1083, 1080), CS_TOWN_BUILDING_SMITHY_BUY_MAKE_CD("buyCd", 1087, 1084)],
        ("reward", "tmReward") => [
            CS_REWARD_GETREWARDLIST("getRewardList", 1093, 1090), CS_REWARD_RECEIVEBYRIDARR("receiveByRidArr", 1097, 1094),
            CS_ACTIVITY_CODE_GIFTBYCODE("giftByCode", 1674, 1658)],
        ("celebrity", "tmCelebrity") => [
            CS_CELEBRITY_LIST("list", 1109, 1106), CS_CELEBRITY_VISITTIMES("getList", 1117, 1114),
            CS_CELEBRITY_VISITHERO("visit", 1121, 1118)],
        ("business", "tmBusiness") => [
            CS_BUSINESS_GET_BASE_DATA("getBaseData", 1129, 1126), CS_BUSINESS_ENTER("enter", 1133, 1130),
            CS_BUSINESS_LEAVE("leave", 1137, 1134), CS_BUSINESS_GOTOBUSINESS("gotoBusiness", 1142, 1139),
            CS_BUSINESS_BUYCARAVANS("buyCaravans", 1147, 1144), CS_BUSINESS_GETCARAVANSLIST("getCaravansList", 1152, 1149),
            CS_BUSINESS_INSPIRECARAVANS("inspireCaravans", 1156, 1153), CS_BUSINESS_BUYROBTIMES("buyRobTimes", 1160, 1157),
            CS_BUSINESS_BUYTRADETIMES("buyBusinessTimes", 1164, 1161), CS_BUSINESS_PROMPTLYEND("promptlyEnd", 1168, 1165),
            CS_BUSINESS_REFRESHCARAVANS("refreshCaravans", 1172, 1169), CS_BUSINESS_RESETROBCD("resetRobCD", 1176, 1173),
            CS_BUSINESS_ROB("rob", 1180, 1177), CS_BUSINESS_FRIENDHELP("friendHelp", 1184, 1181),
            CS_BUSINESS_GETREPORTS("getReports", 1188, 1185), CS_BUSINESS_GETUSERREPORTS("getUserReports", 1192, 1189)],
        ("re.business", "tmRe.business") => [
            SC_BUSINESS_ROBBED("robbed", 1196, 1193), SC_BUSINESS_TRADESTART("tradestart", 1200, 1197),
            SC_BUSINESS_TRADEEND("tradeend", 1204, 1201)],
        ("dota", "tmDota") => [
            CS_DOTA_INFO("getInfo", 1214, 1211), CS_DOTA_ATTACK_NORMAL_TOWER("attackNormalTower", 1219, 1216),
            CS_DOTA_RESULT_NORMAL_TOWER("resultNormalTower", 1224, 1221), CS_DOTA_RESET("resetNormalTower", 1229, 1226),
            CS_DOTA_OPEN_BOX("openBoxReward", 1234, 1231), CS_DOTA_ATTACK_ELITE("attackEliteTower", 1239, 1236),
            CS_DOTA_PASS_ELITE("passEliteTower", 1244, 1241), CS_DOTA_BUY_TIMES("buyEliteAttackNum", 1249, 1246),
            CS_DOTA_SWEEP("sweepNormalTower", 1254, 1251), CS_DOTA_NORMAL_BUY_TIMES("buyNormalAttackNum", 1259, 1256),
            CS_DOTA_SWEEP_ELITE("sweepEliteTower", 1264, 1261), CS_DOTA_QUICK_SWEEP("sweepNormalTowerQuick", 1269, 1266),
            CS_DOTA_SWEEP_NORMAL_BY_ASSISTANT("sweepNormalByAssistant", 1272, 1270),
            CS_DOTA_SWEEP_ELITE_BY_ASSISTANT("sweepEliteByAssistant", 1274, 1272)],
        ("discount", "tmDiscount") => [CS_ACTIVITY_DISCOUNT_INFO("getInfo", 1291, 1297), CS_ACTIVITY_DISCOUNT_BUY("buy", 1296, 1302)],
        ("activityRechargeRebate", "tmActivityRechargeRebate") => [
            CS_ACTIVITY_REBATE_INFO("getInfo", 1301, 1307), CS_ACTIVITY_REBATE_RECIEVE_REWARD("recieveReward", 1306, 1312),
            CS_ACTIVITY_REBATE_LOTTERY("getLotteryReward", 1311, 1317)],
        ("growUp", "tmGrowUp") => [CS_ACTIVITY_GROW_UP_GETINFO("getInfo", 1326, 1332), CS_ACTIVITY_GROW_UP_MAKE("make", 1331, 1337)],
        ("loginReward", "tmLoginReward") => [
            CS_ACTIVITY_LOGIN_REWARD_GETINFO("getInfo", 1336, 1342), CS_ACTIVITY_LOGIN_REWARD_MAKE("make", 1342, 1348)],
        ("returnServerSign", "tmReturnServerSign") => [
            CS_LOGIN_REWARD_RE_GETINFO("getInfo", 1347, 1353), CS_LOGIN_REWARD_RE_RECEIVE("make", 1352, 1358)],
        ("loginReward_1", "tmLoginReward_1") => [
            CS_ACTIVITY_LOGIN_REWARD_GETINFO_1("getInfo", 1357, 1363), CS_ACTIVITY_LOGIN_REWARD_MAKE_1("make", 1362, 1368)],
        ("online", "tmOnline") => [CS_ACTIVITY_ONLINE_GETINFO("getInfo", 1368, 1374), CS_ACTIVITY_ONLINE_FETCH("fetch", 1373, 1379)],
        ("consume", "tmConsume") => [
            CS_ACTIVITY_CONSUME_GETINFO("getInfo", 1378, 1384), CS_ACTIVITY_CONSUME_GET_REWARD("recieveReward", 1383, 1389)],
        ("recharge", "tmRecharge") => [
            CS_ACTIVITY_RECHARGE_GETINFO("getInfo", 1388, 1394), CS_ACTIVITY_RECHARGE_GET_REWARD("recieveReward", 1393, 1399)],
        ("growth", "tmGrowth") => [
            CS_ACTIVITY_GROWTH_GETINFO("getInfo", 1398, 1404), CS_ACTIVITY_GROWTH_BUY("buy", 1403, 1409),
            CS_ACTIVITY_GROWTH_RECEIVE_REWARD("receiveReward", 1408, 1414), CS_ACTIVITY_GROWTH_GET_NUM("getNum", 1413, 1419)],
        ("vipBenefit", "tmVipBenefit") => [
            CS_ACTIVITY_VIP_GET_BENEFIT_INFO("getBenefitInfo", 1418, 1424),
            CS_ACTIVITY_VIP_MAKE_DAILY_BENEFIT("makeDailyBenefit", 1422, 1428),
            CS_ACTIVITY_VIP_MAKE_WEEKLY_BENEFIT("makeWeeklyBenefit", 1428, 1434)],
        ("firstGift", "tmFirstGift") => [
            CS_ACTIVITY_FIRSTGIFT_RECEIVEREWARD("receiveReward", 1438, 1444),
            CS_ACTIVITY_FIRSTGIFT_INFO("getRechargeGold", 1440, 1446)],
        ("newYearGift", "tmNewYearGift") => [
            CS_ACTIVITY_NEW_YEAR_GIFT_GETINFO("getInfo", 1450, 1456), CS_NEWYEAR_NEWYEARGIFTCODE("newYearGiftCode", 3283, 3253)],
        ("activityRechargeReward", "tmActivityRechargeReward") => [
            CS_ACTIVITY_RECHARGEREWARD_GETINFO("getInfo", 1463, 1469),
            CS_ACTIVITY_RECHARGEREWARD_RECIEVEREWARD("recieveReward", 1471, 1477)],
        ("activityRechargeRewardNew", "tmActivityRechargeRewardNew") => [
            CS_ACTIVITY_RECHARGEREWARD_GETINFO_NEW("getInfo", 1508, 1492),
            CS_ACTIVITY_RECHARGEREWARD_RECIEVEREWARD_NEW("recieveReward", 1516, 1500)],
        ("opencarnival", "tmOpencarnival") => [
            CS_ACTIVITY_OPEN_CARNIVAL_GET_OPEN_ACHIEVE("getOpenAchieve", 1524, 1508),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_ACTIVE_REWARD("receiveActiveReward", 1529, 1513),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_DAILY_REWARD("receiveDailyReward", 1534, 1518),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_DISCOUNT_REWARD("receiveDiscountReward", 1539, 1523),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_RECHARGE_REWARD("receiveRechargeReward", 1544, 1528)],
        ("opencarnivaltwo", "tmOpencarnivaltwo") => [
            CS_ACTIVITY_OPEN_CARNIVAL_GET_OPEN_ACHIEVE_2("getOpenAchieve", 1559, 1543),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_ACTIVE_REWARD_2("receiveActiveReward", 1564, 1548),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_DAILY_REWARD_2("receiveDailyReward", 1569, 1553),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_DISCOUNT_REWARD_2("receiveDiscountReward", 1574, 1558),
            CS_ACTIVITY_OPEN_CARNIVAL_BUY_DISCOUNT("buyDiscount", 1579, 1563),
            CS_ACTIVITY_OPEN_CARNIVAL_RECEIVE_RECHARGE_REWARD_2("receiveRechargeReward", 1584, 1568)],
        ("share", "tmShare") => [
            CS_ACTIVITU_SHARE_GET_SHARED_INFO("getSharedInfo", 1591, 1575), CS_ACTIVITY_SHARE_RECEIVE("receive", 1596, 1580)],
        ("eatChicken", "tmEatChicken") => [
            CS_ACTIVITY_EATCHICKEN_GETINFO("getInfo", 1616, 1600), CS_ACTIVITY_EATCHICKEN_EAT("eat", 1621, 1605)],
        ("sign", "tmSign") => [
            CS_ACTIVITY_SIGN_GETINFO("getInfo", 1626, 1610), CS_ACTIVITY_SIGN_SIGNTODAY("signToday", 1631, 1615),
            CS_ACTIVITY_SIGN_GAIN_VIP_SIGN_REWARD("gainVipSignReward", 1636, 1620),
            CS_ACTIVITY_SIGN_GAIN_RECHARGE_SIGN_REWARD("gainRechargeSignReward", 1641, 1625)],
        ("activityLimit", "tmActivityLimit") => [
            CS_ACTIVITY_LIMIT_GETINFO("getInfo", 1646, 1630), CS_ACTIVITY_LIMIT_GETREWARD("getReward", 1651, 1635),
            SC_ACTIVITY_LIMIT_ADDNUM("addNum", 1657, 1641)],
        ("activitytask", "tmActivitytask") => [
            CS_ACTIVITY_MORETHAN_GETINFO("getInfo", 1662, 1646), CS_ACTIVITY_MORETHAN_GETREWARD("recieveReward", 1665, 1649)],
        ("monthCard", "tmMonthCard") => [
            CS_ACTIVITY_MONTH_CARD("getInfo", 1677, 1661), CS_ACTIVITY_MONTH_CARD_RECEIVE("receiveReward", 1680, 1664)],
        ("activityRechargeGift", "tmActivityRechargeGift") => [
            CS_ACTIVITY_RECHARGE_GIFT_GETINFO("getInfo", 1688, 1672),
            CS_ACTIVITY_RECHARGE_GIFT_GET_REWARD("receiveRechargeGift", 1693, 1677)],
        ("activityRechargeGiftOpen", "tmActivityRechargeGiftOpen") => [
            CS_ACTIVITY_RECHARGE_GIFT_OPEN_GETINFO("getInfo", 1703, 1687),
            CS_ACTIVITY_RECHARGE_GIFT_OPEN_GET_REWARD("receiveRechargeGiftOpen", 1708, 1692)],
        ("activitycontinuerecharge", "tmActivitycontinuerecharge") => [
            CS_ACTIVITY_CONTINUR_RECHARGE_GET_INFO("getInfo", 1713, 1697),
            CS_ACTIVITY_CONTINUR_RECHARGE_RECEIVE_GOLD("receiveGold", 1718, 1702)],
        ("activityRechargeDouble", "tmActivityRechargeDouble") => [
            CS_ACTIVITY_RECHARGE_DOUBLE_GETINFO("getInfo", 1740, 1707),
            CS_ACTIVITY_RECHARGE_DOUBLE_GETREWARD("receiveReward", 1745, 1712)],
        ("activityFamousRaffle", "tmActivityFamousRaffle") => [
            CS_ACTIVITY_ACTIVITY_FAMOUSRAFFLE_GETINFO("getInfo", 1750, 1717),
            CS_ACTIVITY_ACTIVITY_FAMOUSRAFFLE_RA("raffle", 1755, 1722),
            CS_ACTIVITY_ACTIVITY_FAMOUSRAFFLE_OPENBOX("openBox", 1760, 1727),
            CS_ACTIVITY_ACTIVITY_FAMOUSRAFFLE_TASK("getTaskReward", 1765, 1732)],
        ("activityPower", "tmActivityPower") => [
            CS_ACTIVITY_POWERIMPROVE_GETINFO("getInfo", 1772, 1739), CS_ACTIVITY_POWERIMPROVE_RECREWARD("recReward", 1777, 1744)],
        ("firstWeek", "tmFirstWeek") => [
            CS_ACTIVITY_WEEKFUND_GETINFO("getInfo", 1782, 1749), CS_ACTIVITY_WEEKFUND_RECREWARD("recReward", 1787, 1754)],
        ("activityHeroGift", "tmActivityHeroGift") => [
            CS_ACTIVITY_HEROGIFT_GETINFO("getInfo", 1792, 1759), CS_ACTIVITY_HEROGIFT_RECEIVE_GIFT("receiveGift", 1797, 1764),
            CS_ACTIVITY_HEROGIFT_ENTER("enter", 1802, 1769), CS_ACTIVITY_HEROGIFT_LEAVE("leave", 1807, 1774)],
        ("sworn", "tmSworn") => [
            CS_SWORN_SWORNINFO("getSwornInfo", 1833, 1800), CS_SWORN_DOSWORN("doSworn", 1837, 1804),
            CS_SWORN_LEVELUP("swornLvUp", 1841, 1808), CS_SWORN_DELSWORN("delSworn", 1845, 1812)],
        ("drink", "tmDrink") => [
            CS_DRINK_GETDRINKINFO("getDrinkInfo", 1852, 1819), CS_DRINK_DODRINK("doDrink", 1857, 1824),
            CS_DRINK_REFRESH("refresh", 1862, 1829)],
        ("post", "tmPost") => [
            CS_POST_GETINFO("getInfo", 1872, 1839), CS_POST_UPGRADE("upgrade", 1877, 1844),
            CS_POST_RECEIVESALARY("receiveSalary", 1882, 1849)],
        ("guild", "tmGuild") => [
            CS_LEGION_CREATE("createGuild", 1888, 1855), CS_LEGION_APPLY("applyGuild", 1892, 1859),
            CS_LEGION_CANCELAPPLY("cancelApply", 1896, 1863), CS_LEGION_AGREEAPPLY("agreeApply", 1900, 1867),
            CS_LEGION_REFUSEAPPLY("refuseApply", 1904, 1871), CS_LEGION_REFUSEALLAPPLY("refuseAllApply", 1908, 1875),
            CS_LEGION_QUIT("quitGuild", 1912, 1879), CS_LEGION_KICKMEMBER("kickMember", 1916, 1883),
            CS_LEGION_MODIFYICON("modifyIcon", 1920, 1887), CS_LEGION_MODIFYSLOGAN("modifySlogan", 1924, 1891),
            CS_LEGION_MODIFYPOST("modifyPost", 1928, 1895), CS_LEGION_DISMISS("dismiss", 1940, 1907),
            CS_LEGION_TRANSPRESIDENT("transPresident", 1944, 1911), CS_LEGION_SET_MEMBERTYPE("setMemberType", 1948, 1915),
            CS_LEGION_IMPEACH("impeach", 1953, 1920), CS_LEGION_CONTRIBUTE("contribute", 1957, 1924),
            CS_LEGION_UPGRADEGUILD("upgradeBuild", 1961, 1928), CS_LEGION_PROGRESS_REWARD("getReward", 1965, 1932),
            CS_LEGION_LOTTERY("lottery", 1969, 1936), CS_LEGION_FIGHT("fightEachOther", 1973, 1940),
            CS_LEGION_GETAPPLYLIST("getGuildApplyList", 1977, 1944), CS_LEGION_GETGUILDLIST("getGuildList", 1981, 1948),
            CS_LEGION_SEARCHGUILD("getGuildListByName", 1985, 1952), CS_LEGION_GETMEMBERLIST("getMemberList", 1989, 1956),
            CS_LEGION_MEMBERINFO("getMemberInfo", 1993, 1960), CS_LEGION_GETDYNAMICLIST("getDynamicList", 1997, 1964),
            CS_LEGION_GETGUILDINFO("getGuildInfo", 2001, 1968), CS_LEGION_MODIFY_NAME("modifyName", 2005, 1972),
            CS_LEGION_MODIFY_PASSWORD("modifyPassWd", 2009, 1976), CS_LEGION_SET_QQ("modifyQQ", 2013, 1980),
            CS_LEGION_SET_ICON("modifyIcon", 2017, 1984)],
        ("re", "tmRe") => [
            CS_LEGION_REFUSH_GUILD("refresh_member", 2021, 1988), CS_LEGION_REFUSH_GUILD_BUILD("refresh_guild", 2029, 1996),
            SC_MAINPOP("mainpop", 2697, 2667), SC_ACT_FIGHT_FORCE_REFRESH("refreshFightForce", 2747, 2717),
            CS_ACTPOWERWHEEL_PUSH_LOG("activity_power_wheel_log", 4067, 4037), SC_MILLIONQUIZ_REDBAG("noticeRedBag", 4487, 4457)],
        ("re.guild", "tmRe.guild") => [
            CS_LEGION_REFUSH_PROGRESS("contribute", 2025, 1992), CS_LEGION_REFUSH_GUILD_IMPEACH("impeach", 2033, 2000)],
        ("guildcopy", "tmGuildcopy") => [
            CS_LEGION_COPY_INFO("getGuildCopyInfo", 2038, 2005), CS_LEGION_COPY_STRONGINFO("getStrongInfo", 2042, 2009),
            CS_LEGION_COPY_CHOICE_COPY("choiceCopy", 2046, 2013), CS_LEGION_COPY_GET_BASE_REWARD("getBaseReward", 2050, 2017),
            CS_LEGION_COPY_GET_BOX_REWARD("getRewardBox", 2054, 2021),
            CS_LEGION_COPY_GET_LST_BOX_REWARD("getRewardBoxOld", 2058, 2025), CS_LEGION_COPY_BUY_ATK_NUM("buyAtkNum", 2062, 2029),
            CS_LEGION_COPY_GET_ATK_RANK("getAtkRank", 2066, 2033), CS_LEGION_COPY_ALL_FIGHT("allFight", 2070, 2037),
            CS_LEGION_COPY_FIGHT("attack", 2074, 2041), CS_LEGION_COPY_RESET("resetCopy", 2078, 2045),
            CS_LEGION_COPY_AUTO_RESET("autoReset", 2082, 2049)],
        ("re.guildCopy", "tmRe.guildCopy") => [
            CS_LEGION_COPY_PUSH_OPEN_BOX("rewardBox", 2086, 2053), CS_LEGION_COPY_PUSH_LST_BOX("rewardBoxOld", 2090, 2057),
            CS_LEGOIN_COPY_PUSH_ALL_FIGHT("allFight", 2095, 2062), CS_LEGION_COPY_PUSH_HP("refreshHp", 2099, 2066),
            CS_LEGION_COPY_PUSH_RESET("reset", 2103, 2070), CS_LEGION_COPY_PASS("pass", 2107, 2074)],
        ("guildWar", "tmGuildWar") => [
            CS_LEGION_BATTLE_SIGNUP("signUp", 2113, 2080), CS_LEGION_BATTLE_ARR_CITYLIST("getArrCitySignUpList", 2116, 2083),
            CS_LEGION_BATTLE_CITYLIST("getCityList", 2121, 2088), CS_LEGION_BATTLE_WARINFO("getWarInfo", 2125, 2092),
            CS_LEGION_BATTLE_ENTER("enter", 2129, 2096), CS_LEGION_BATTLE_GETINFO("getBattleInfo", 2133, 2100),
            CS_LEGION_BATTLE_LEAVE("leave", 2137, 2104), CS_LEGION_BATTLE_GUWU("inspirate", 2164, 2131),
            CS_LEGION_BATTLE_BUY_STREAK("buyStreakLimit", 2168, 2135), CS_LEGION_BATTLE_SIGN_INFO("getGuildSignUpList", 2171, 2138),
            CS_LEGION_BATTLE_SIGN("signUp", 2174, 2141), CS_LEGION_BATTLE_GET_SIGNUP_LIST("getSignUpList", 2177, 2144),
            CS_LEGION_BATTLE_GET_CITY_REWARD("receiveReward", 2185, 2152), CS_LEGION_BATTLE_GET_BASEINFO("getBaseData", 2189, 2156),
            CS_LEGION_BATTLE_ENTER_MAP("enterHall", 2199, 2166), CS_LEGION_BATTLE_LEAVE_MAP("leaveHall", 2203, 2170),
            CS_LEGION_BATTLE_GET_CITY_SIGN("getAllSignupNum", 2219, 2186), CS_LEGION_BATTLE_JIJIE("muster", 2223, 2190),
            CS_LEGION_BATTLE_GET_JIJIE_INFO("getMusterInfo", 2227, 2194),
            CS_LEGION_BATTLE_GET_REWARD_TYPE("getRewardType", 2231, 2198),
            CS_LEGION_BATTLE_GET_LST_WARINFO("getLastWarInfo", 2235, 2202),
            CS_LEGION_BATTLE_GET_BATTLE_QUEUE("getBattleQueue", 2239, 2206), CS_LEGION_BATTLE_GET_REPORT("getRepaly", 2243, 2210),
            CS_LEGION_BATTLE_CAPTURE_CITY("captureCity", 2247, 2214), CS_LEGION_BATTLE_CAPTURE_INFO("isHaveCaputured", 2251, 2218),
            CS_LEGION_BATTLE_ALL_SIGN_LIST("getCurGuildSignUpList", 2280, 2247),
            SC_CC_USER_ALLDATA("PushUserMapAllData", 5868, 5609), SC_CC_USER_ENTER_MAP("PushUserEnterMap", 5869, 5610),
            SC_CC_USER_CHANGE_POS("PushUserChangePos", 5870, 5611), SC_CC_USER_STATE_CHANGE("PushUserStateChange", 5871, 5612),
            SC_CC_USER_REVIVE("PushUserRevive", 5872, 5613), SC_CC_USER_MARCH("PushUserMarch", 5873, 5614),
            SC_CC_USER_ENERGY_CHANGE("PushUserEnergyChange", 5874, 5615),
            SC_CC_USER_POINTS_CHANGE("PushUserPointsChange", 5875, 5616), SC_CC_USER_POWER_CHANGE("PushUserPowerChange", 5876, 5617),
            SC_CC_BUILD_LIST_CHANGE("PushBuildListChange", 5878, 5619), SC_CC_DOBATTLE_BEGIN("PushDoBattleBegin", 5880, 5621),
            SC_CC_DOBATTLE_END("PushDoBattleEnd", 5881, 5622), SC_CC_SWITCH_BRAWL("PushSwitchToCrazyFight", 5884, 5625),
            SC_CC_BATTLE_RESULT("PushGuildWarBattleResult", 5887, 5628)],
        ("re.guildWar", "tmRe.guildWar") => [
            CS_LEGION_BATTLE_ADD_TEAM("enter", 2141, 2108), CS_LEGION_BATTLE_REMOVE_TEAM("leave", 2145, 2112),
            CS_LEGION_BATTLE_PUSH_QUEUE("battleQueue", 2149, 2116), CS_LEGION_BATTLE_PUSH_FIGHT("playRecord", 2153, 2120),
            CS_LEGION_BATTLE_FIGHT_END("battleResult", 2157, 2124), CS_LEGION_BATTLE_SIGN_ED("signup", 2160, 2127),
            CS_LEGION_BATTLE_PUSH_MAP("refreshCityList", 2194, 2161), CS_LEGION_BATTLE_ADDNUM("hallNumAdd", 2207, 2174),
            CS_LEGION_BATTLE_SUBNUM("hallNumSub", 2211, 2178), CS_LEGION_BATTLE_CLEAR_CITY("clearCity", 2215, 2182)],
        ("guildarmy", "tmGuildarmy") => [
            CS_MAP_GET_MAN_INFO("getInfo", 2256, 2223), CS_MAP_MAN_SWEEP("sweep", 2261, 2228),
            CS_MAP_MAN_NEW_ARMY("getNewArmy", 2266, 2233), CS_MAP_MAN_QUICK_SWEEP("sweepNew", 2271, 2238),
            CS_MAP_MAN_GET_ARMY_LIST("getArmyList", 2275, 2242)],
        ("top", "tmTop") => [
            CS_RANKLIST_LIST("get", 2294, 2261), CS_RANKLIST_PLAYERINFO("getUserInfo", 2298, 2265),
            CS_RANKLIST_GUILDINFO("getGuildInfo", 2302, 2269), CS_RANKLIST_OWNINFO("getSelf", 2306, 2273),
            CS_GET_GUILD_LEADER_INFO("getGuildPresidentInfo", 2318, 2285),
            CS_RANKLIST_GETFULL_INFO("getFullPlatformRank", 2323, 2290),
            CS_RANKLIST_COLLECT_INFO("getOtherCollectionInfo", 2325, 2292),
            CS_GET_PASS_GUILD_LEADER_INFO("getCrossGuildPresidentInfo", 2331, 2300), CS_ALLPLATFORM_RANK_FIGHT("attack", 4894, 4864)],
        ("praise", "tmPraise") => [
            CS_RANKLST_PRAISE_INFO("getPraiseInfo", 2310, 2277), CS_RANKLIST_PRAISE("doPraise", 2314, 2281),
            CS_RANKLIST_PRAISE_FPFF("doPraiseFPFF", 2327, 2294)],
        ("raffle", "tmRaffle") => [
            CS_RAFFLE_GET_INFO("getInfo", 2338, 2306), CS_RAFFLE_SIGNAL("singleRaffle", 2342, 2310),
            CS_RAFFLE_FREE_AUTO("autoRaffle", 2346, 2314), CS_RAFFLE_MULTI("multiRaffle", 2350, 2318),
            CS_RAFFLE_LIMIT("raffleHot", 2353, 2321), CS_RAFFLE_LIMIT_TEN("multiRaffleHot", 2356, 2324),
            CS_RAFFLE_RECEIVE_LUCK_HERO("receiveLuckHeroGift", 2359, 2327), CS_RAFFLE_LIMIT_REFRESH("refreshHotByHand", 2362, 2330),
            CS_RAFFLE_RECEIVE_LUCK_HERO_HOT("receiveHotLuckHeroGift", 2365, 2333),
            CS_RAFFLE_RECEIVE_LUCK_HERO_RED("receiveRedLuckHeroGift", 2368, 2336), CS_RAFFLE_RED("raffleRed", 2370, 2338),
            CS_RAFFLE_BUY_TOKEN("buyToken", 2373, 2341)],
        ("destiny", "tmDestiny") => [
            CS_ASTROLABE_GET_INFO("getDestinyInfo", 2379, 2347), CS_ASTROLABE_ADD_NODE("addNode", 2382, 2350)],
        ("newDestiny", "newDestiny") => [
            CE_DESTINY_GET_INFO("getDestinyInfo", 2390, 2360), CE_DESTINY_ADD_NODE("addNode", 2395, 2365)],
        ("mine", "tmMine") => [
            CS_MINE_GET_PAGE_INFO("getPageInfo", 2403, 2373), CS_MINE_GET_MY_PAGE_INFO("getMyPageInfo", 2407, 2377),
            CS_MINE_GET_SELF_INFO("getSelfInfo", 2411, 2381), CS_MINE_OPEN_TECH("openTech", 2415, 2385),
            CS_MINE_QUICK_SEARCH("quickSearch", 2419, 2389), CS_MINE_LEAVE_MINE("leaveMine", 2423, 2393),
            CS_MINE_ATTACK("attack", 2427, 2397), CS_MINE_SEND_RESULT("sendResult", 2431, 2401),
            CS_MINE_END_CAPTURE("endCapture", 2435, 2405), CS_MINE_EXTEND("extend", 2439, 2409),
            CS_MINE_GET_TECHINFO("getTechInfo", 2443, 2413), CS_MINE_GET_ENEMY_INFO("getEnemyInfo", 2447, 2417),
            CS_MINE_GET_GUILDMINE("getGuildMine", 2451, 2421), CS_MINE_GET_MINE_BY_UID("getMineByUid", 2455, 2425),
            CS_MINE_GET_ALL_REPORT("getAllReport", 2459, 2429)],
        ("re.mine", "tmRe.mine") => [
            SC_MINE_BE_ATTACKED("beAttacked", 2463, 2433), SC_MINE_NEW_ENEMY("newEnemy", 2467, 2437),
            SC_MINE_REFRESH_MINE("refreshMine", 2471, 2441)],
        ("worldMine", "tmWorldMine") => [
            CS_MINECS_GET_PAGE_INFO("getPageInfo", 2477, 2447), CS_MINECS_GET_MY_PAGE_INFO("getMyPageInfo", 2481, 2451),
            CS_MINECS_GET_SELF_INFO("getMyInfo", 2485, 2455), CS_MINECS_OPEN_TECH("openTech", 2489, 2459),
            CS_MINECS_QUICK_SEARCH("quickSearch", 2493, 2463), CS_MINECS_LEAVE_MINE("leaveMine", 2497, 2467),
            CS_MINECS_ATTACK("attack", 2501, 2471), CS_MINECS_SEND_RESULT("sendResult", 2505, 2475),
            CS_MINECS_END_CAPTURE("endCapture", 2509, 2479), CS_MINECS_EXTEND("extend", 2513, 2483),
            CS_MINECS_GET_TECHINFO("getTechInfo", 2517, 2487), CS_MINECS_GET_ENEMY_INFO("getEnemyInfo", 2521, 2491),
            CS_MINECS_GET_GUILDMINE("getGuildMine", 2525, 2495),
            CS_MINECS_GET_MINE_BY_SERVERID_AND_PID("getMineByServerIdAndPid", 2529, 2499),
            CS_MINECS_GET_ALL_REPORT("getAllReport", 2533, 2503), CS_MINECS_GET_FORMATION_DATA("getFormationData", 2537, 2507)],
        ("re.worldMine", "tmRe.worldMine") => [
            SC_MINECS_BE_ATTACKED("beAttacked", 2541, 2511), SC_MINECS_NEW_ENEMY("newEnemy", 2545, 2515),
            SC_MINECS_REFRESH_MINE("refreshMine", 2549, 2519)],
        ("rebel", "tmRebel") => [
            CS_REBEL_GET_INFO("getInfo", 2555, 2525), CS_REBEL_ENTER_REBEL("enterRebel", 2559, 2529),
            CS_REBEL_ATTACK("attack", 2563, 2533), CS_REBEL_GET_DAMAGE_REWARD("getDamageReward", 2567, 2537),
            CS_REBEL_GET_KILL_REWARD("getKillReward", 2571, 2541), CS_REBEL_GET_FIND_REWARD("getFindReward", 2575, 2545),
            CS_GET_REBEL_RANK("getRebelRank", 2579, 2549), CS_REBEL_THANK_FRIEND("thankFriend", 2583, 2553),
            CS_REBEL_FETCH_BOTH_REWARD("fetchBothReward", 2588, 2558), CS_REBEL_SHARE("share", 2592, 2562),
            CS_REBEL_BUY_ATK_NUM("buyAtkNum", 2596, 2566), CS_REBEL_GET_ALL_REBEL_REWARD("oneKeyGetRebelReward", 2616, 2586),
            CS_REBEL_GET_ALL_DAMAGE_REWARD("oneKeyGetDamageReward", 2620, 2590)],
        ("re.rebel", "tmRe.rebel") => [SC_REBEL_SHARE_REBEL("shareRebel", 2600, 2570), SC_REBEL_KILLED("killed", 2604, 2574)],
        ("bosselite", "tmBosselite") => [
            CS_BOSSELITE_GETBOSSELITE("getBosselite", 2627, 2597), CS_BOSSELITE_ATTACKBOSSELITE("attackBosselite", 2631, 2601),
            CS_BOSSELITE_PASSBOSSELITE("passBosselite", 2635, 2605),
            CS_BOSSELITE_BUY_NORMALATTACKTIMES("buyNormalAttackTimes", 2639, 2609)],
        ("discover", "tmDiscover") => [
            CS_DISCOVER_GETDISCOVER("getDiscover", 2645, 2615), CS_DISCOVER_ATTACK("attack", 2649, 2619),
            CS_DISCOVER_ATTACK_BOSS("attackBoss", 2653, 2623), CS_DISCOVER_PASS_BOSS("passBoss", 2657, 2627),
            CS_DISCOVER_GET_FIRST_REWARD("receivePassReward", 2661, 2631), CS_DISCOVER_GET_REWARD_CITY("getRewardCity", 2665, 2635),
            CS_DISCOVER_SWEEP("quickAttack", 2669, 2639), CS_DISCOVER_BOSS_SWEEP("attackBossBack", 2673, 2643),
            CS_AVENTRUE_GETAVENTURE("getAventure", 2680, 2650), CS_AVENTRUE_ATTACK("attackAventure", 2684, 2654),
            CS_AVENTRUE_FINISH("finishkAventure", 2688, 2658), CS_AVENTRUE_QUICK_FINISH("quickFinishkAventure", 2692, 2662)],
        ("onlineFestival", "tmOnlineFestival") => [
            CS_ONLINE_FESTIVAL_GETINFO("getInfo", 2703, 2673), CS_ONLINE_FESTIVAL_REVEIVE("receive", 2708, 2678)],
        ("openOnline", "tmOpenOnline") => [
            CS_ONLINE_OPEN_GETINFO("getInfo", 2713, 2683), CS_ONLINE_OPEN_REVEIVE("receive", 2718, 2688)],
        ("onlineFestivalReset", "tmOnlineFestivalReset") => [
            CS_ONLINE_FESTIVAL_RESET_GETINFO("getInfo", 2723, 2693), CS_ONLINE_FESTIVAL_RESET_REVEIVE("receive", 2728, 2698)],
        ("fightforce", "tmFightforce") => [
            CS_ACT_FIGHT_FORCE_GET_TOP("getTop", 2737, 2707), CS_ACT_FIGHT_FORCE_RECEIVE("receive", 2742, 2712)],
        ("levelRecharge", "tmLevelRecharge") => [
            CS_LEVEL_RECHARGE_GETINFO("getInfo", 2756, 2726), CS_LEVEL_RECHARGE_GETREWARD("getReward", 2761, 2731)],
        ("fiend", "tmFiend") => [
            CS_FIEND_GET_FIEND_INFO("getFiendInfo", 2770, 2740), CS_FIEND_ENTER("enter", 2775, 2745),
            CS_FIEND_LEAVE("leave", 2780, 2750), CS_FIEND_ATTACK("attack", 2785, 2755),
            CS_FIEND_GETTOTALTOP("getTotalTop", 2790, 2760), CS_FIEND_GETMAXTOP("getMaxTop", 2795, 2765),
            CS_FIEND_BUY_NUM("buyNum", 2816, 2786), CS_FIEND_SEND_REWARD("sendReward", 2821, 2791),
            CS_FIEND_OPEN_BOX("openBox", 2826, 2796), CS_FIEND_RESET_CD_TIME("resetCdTime", 2831, 2801)],
        ("re.fiend", "tmRe.fiend") => [
            SC_FIEND_MSG("msg", 2800, 2770), SC_FIEND_OPEN_FIEND("openFiend", 2805, 2775),
            SC_FIEND_FIEND_OVER("FiendOver", 2810, 2780), SC_FIEND_GET_GOOD_ITEM("getGoodItem", 2836, 2806),
            SC_FIEND_OVER_RANK("overRank", 2841, 2811), SC_BOSS_OVER_RANK("overRank", 3024, 2994)],
        ("recruitment", "tmRecruitment") => [
            CS_RECRUITMENT_GETINFO("getInfo", 2852, 2822), CS_RECRUITMENT_RECEIVEACTIVEREWARD("receiveActiveReward", 2858, 2828)],
        ("newswap", "tmNewswap") => [
            CS_SWAP_HERO("swapHero", 2871, 2841), CS_SWAP_TREASURE("swapTreasure", 2875, 2845),
            CS_SWAP_MULTI_TREASURE("swapMultiTreasure", 2879, 2849), CS_SWAP_AWAKEITEM("swapAwakeItem", 2883, 2853),
            CS_SWAP_CROSS_HERO("crossSwapHero", 2887, 2857), CS_SWAP_CROSS_AWAKEITEM("crossSwapAwakeItem", 2891, 2861),
            CS_SWAP_COUNTRY("swapCountry", 2895, 2865), CS_SWAP_COUNTRY_GET_NUM("getSwapCountryNum", 2899, 2869),
            CS_SWAP_GODARM("swapGodArmByTid", 2903, 2873), CS_SWAP_GODANIMAL("swapGodAnimal", 2907, 2877),
            CS_SWAP_BROCHURE("swapBrochure", 2911, 2881), CS_SWAPRED_HERO("swapRedHero", 3540, 3510),
            CS_SWAPRED_AWAKE("swapRedAwake", 3544, 3514), CS_SWAPRED_GODARM("swapGodArm", 3548, 3518)],
        ("envelopRain", "tmEnvelopRain") => [
            CS_ACTIVITY_STAR_RAIN_GETINFO("getInfo", 2917, 2887), CS_ACTIVITY_STAR_RAIN("receiveGold", 2919, 2889)],
        ("activityStarPlayOpen", "tmActivityStarPlayOpen") => [
            CS_ACTIVITY_STAR_CODE_OPEN_GETINFO("getInfo", 2929, 2899), CS_ACTIVITY_STAR_CODE_OPEN_RECEIVE("receiveGift", 2931, 2901)],
        ("activityStarPlaySignOpen", "tmActivityStarPlaySignOpen") => [
            CS_ACTIVITY_STAR_LEVEL_OPEN_GETINFO("getInfo", 2933, 2903),
            CS_ACTIVITY_STAR_LEVEL_OPEN_RECEIVE("receiveReward", 2935, 2905)],
        ("activityFestivalReturn", "tmActivityFestivalReturn") => [
            CS_ACTIVITY_RETURN_GETINFO("getInfo", 2938, 2908), CS_ACTIVITY_RETURN_RECEIVE("receiveReward", 2940, 2910)],
        ("activityFestivalValentine", "tmActivityFestivalValentine") => [
            CS_ACTIVITY_VALENTINE_GETINFO("getInfo", 2942, 2912), CS_ACTIVITY_VALENTINE_ONE("singleBuy", 2944, 2914),
            CS_ACTIVITY_VALENTINE_TEN("multiBuy", 2946, 2916), CS_ACTIVITY_VALENTINE_BOX("receiveBoxReward", 2948, 2918)],
        ("activityBinding", "tmActivityBinding") => [
            CS_ACTIVITY_BINDING_GETINFO("getInfo", 2955, 2925), CS_ACTIVITY_BINDING_GETBINDCODE("getBindCode", 2960, 2930),
            CS_ACTIVITY_BINDING_BINDPHONE("bindPhone", 2965, 2935), CS_ACTIVITY_BINDING_RECEIVEREWARD("receiveReward", 2970, 2940)],
        ("boss", "tmBoss") => [
            CS_BOSS_GET_FIEND_INFO("getFiendInfo", 2978, 2948), CS_BOSS_ENTER("enter", 2983, 2953),
            CS_BOSS_LEAVE("leave", 2988, 2958), CS_BOSS_ATTACK("attack", 2993, 2963), CS_BOSS_GETTOTALTOP("getTotalTop", 2998, 2968),
            CS_BOSS_GETMAXTOP("getMaxTop", 3003, 2973), CS_BOSS_RESETCDTIME("resetCdTime", 3008, 2978),
            CS_BOSS_OPENBOX("openBox", 3014, 2984), CS_BOSS_ATTACK_AND_RESETCD("attackAndResetCd", 3045, 3015),
            CS_BOSS_SEND_KILL_ENVELOPE("sendKillEnvelope", 3060, 3030), CS_BOSS_OPEN_KILL_ENVELOPE("openKillEnvelope", 3065, 3035),
            CS_BOSS_SET_OFFLINE("setIsOutLine", 3086, 3056)],
        ("re.boss", "tmRe.boss") => [
            SC_BOSS_OPEN_BOSS("openBoss", 3019, 2989), SC_BOSS_GET_GOOD_ITEM("getGoodItem", 3030, 3000),
            SC_BOSS_KILL_REWARD("killReward", 3040, 3010)],
        ("re.Boss", "tmRe.Boss") => [
            SC_BOSS_MSG("msg", 3035, 3005), SC_BOSS_TOP_FIVE("topFive", 3050, 3020),
            SC_BOSS_SENDENVELOPE("sendEnvelope", 3055, 3025), SC_BOSS_SEND_ENVELOPE_AUTO("sendEnvelopeAuto", 3071, 3041),
            SC_BOSS_OPEN_GOLD("openGold", 3076, 3046), SC_BOSS_KILL_INFO("killInfo", 3081, 3051)],
        ("activitySignInJoy", "tmActivitySignInJoy") => [
            CS_ACTIVITY_SIGN_IN_JOY_GET_INFO("getInfo", 3094, 3064), CS_ACTIVITY_SIGN_IN_JOY_SIGN("sign", 3098, 3068),
            CS_ACTIVITY_SIGN_IN_JOY_ADDUP_SIGN("addUpSign", 3102, 3072)],
        ("activityBusinessman", "tmActivityBusinessman") => [
            CS_ACTIVITYBUSINESSMAN_GETINFO("getInfo", 3106, 3076), CS_ACTIVITYBUSINESSMAN_BUY_DISCOUNT("buyDiscount", 3108, 3078),
            CS_ACTIVITYBUSINESSMAN_RECEIEVE_RECHARGE_GOLD("receiveRechargeGold", 3110, 3080),
            CS_ACTIVITYBUSINESSMAN_GETREWARD("receiveAllBenefit", 3112, 3082)],
        ("exploreSoul", "tmExploreSoul") => [
            CS_PREY_SPIRIT_GET_INFO("getInfo", 3118, 3088), CS_PREY_SPIRIT_PREY("explore", 3122, 3092),
            CS_PREY_SPIRIT_PREY_QUICK("quickExplore", 3126, 3096), CS_PREY_SPIRIT_ATTACK("attack", 3130, 3100),
            CS_PREY_SPIRIT_PASS("pass", 3134, 3104), CS_PREY_SPIRIT_CALL("call", 3138, 3108),
            CS_PREY_SPIRIT_PREY_NUM("exploreNums", 3142, 3112)],
        ("festivalActivity", "tmFestivalActivity") => [
            CS_FESTIVAL_ACTIVITY_GET_INFO("getInfo", 3148, 3118), CS_FESTIVAL_ACTIVITY_RECEIVE_REWARD("receiveReward", 3153, 3123)],
        ("book", "tmBook") => [
            CS_GOD_ANIMAL_GETBASEDATA("getBaseData", 3182, 3152), CS_BOOK_GETALLDATA("getAllData", 3319, 3289),
            CS_BOOK_GETBASEDATA("getBaseData", 3324, 3294), CS_BOOK_STARUP("promoteBook", 3329, 3299),
            CS_BOOK_RESET_STAR("resetBook", 3334, 3304)],
        ("passcity", "tmPasscity") => [
            CS_PASSCITY_GET_INFO("getInfo", 3204, 3174), CS_PASSCITY_GOTO_BATTLE("gotoBattle", 3209, 3179),
            CS_PASSCITY_ATTACK("attack", 3214, 3184), CS_PASSCITY_PASS("pass", 3219, 3189),
            CS_PASSCITY_CHANGE_POSITION("changePosition", 3224, 3194), CS_PASSCITY_OPEN_BOX("openBox", 3229, 3199),
            CS_PASSCITY_END_BATTLE("endBattle", 3234, 3204), CS_PASSCITY_RECOVER("recover", 3237, 3207),
            CS_PASSCITY_BUY_ATTACK("buyAttackNum", 3240, 3210), CS_PASSCITY_SWEEP("sweep", 3243, 3213),
            CS_PASSCITY_ATTACK_ELITE_NPC("attackEliteNPC", 3246, 3216), CS_PASSCITY_PASS_ELITE_NPC("passEliteNPC", 3249, 3219),
            CS_PASSCITY_SWEEP_ELITE_NPC("sweepEliteNPC", 3252, 3222),
            CS_PASSCITY_BUY_ELITE_ATTACK_NUM("buyEliteAttackNum", 3255, 3225)],
        ("worldperson", "tmWorldperson") => [
            CS_CROSSBATTLE_GET_INFO("getInfo", 3258, 3228), CS_CROSSBATTLE_ATTACK("attack", 3260, 3230),
            CS_CROSSBATTLE_BUY_ATTACK("buyAttackNum", 3262, 3232), CS_CROSSBATTLE_ANGRY("angry", 3264, 3234),
            CS_CROSSBATTLE_REFRESH_ENEMY("refreshEnemies", 3266, 3236), CS_CROSSBATTLE_RANKLIST("getRankList", 3268, 3238),
            CS_CROSSBATTLE_RECEIVEBOX("fetchWinReward", 3270, 3240), CS_CROSSBATTLE_GETFORMATION("getFormation", 3272, 3242),
            CS_CROSSBATTLE_RECEIVE_BOX("receiveBox", 3274, 3244)],
        ("activitySevenStar", "tmActivitySevenStar") => [
            CS_ACTIVITY_SEVENSTAR_GETINFO("getInfo", 3276, 3246), CS_ACTIVITY_SEVENSTAR_REC("recReward", 3278, 3248)],
        ("festivalYuanXiao", "tmFestivalYuanXiao") => [
            CS_YUANXIAO_GETINFO("getInfo", 3289, 3259), CS_YUANXIAO_GETMSG("getMsg", 3293, 3263),
            CS_YUANXIAO_MAKE("make", 3297, 3267), CS_YUANXIAO_BUY_ITEM("buy", 3301, 3271),
            CS_YUANXIAO_RECEIVE_BOX("receiveBox", 3305, 3275), CS_YUANXIAO_RECEIVE_RECHARGE("receiveRecharge", 3309, 3279)],
        ("raffleArm", "tmRaffleArm") => [
            CS_RAFFLEARM_GETINFO("getInfo", 3342, 3312), CS_RAFFLEARM_RAFFLE("raffle", 3347, 3317),
            CS_RAFFLEARM_GETBOXREWARD("getBoxReward", 3352, 3322), CS_RAFFLEARM_ENTER("enter", 3357, 3327),
            CS_RAFFLEARM_LEAVE("leave", 3362, 3332)],
        ("lordwar", "tmLordwar") => [
            CS_LORDWAR_GETLORDINFO("getLordInfo", 3369, 3339), CS_LORDWAR_REGISTER("register", 3374, 3344),
            CS_LORDWAR_UPDATE_FIGHTINFO("updateFightInfo", 3379, 3349), CS_LORDWAR_CLEARFMTCD("clearFmtCd", 3384, 3354),
            CS_LORDWAR_GETPROMOTIONINFO("getPromotionInfo", 3388, 3358), CS_LORDWAR_GETPROMOTIONBTL("getPromotionBtl", 3392, 3362),
            CS_LORDWAR_SUPPORT("support", 3396, 3366), CS_LORDWAR_GET_SUPPORT_TOP("getSupportSucTop", 3400, 3370),
            CS_LORDWAR_GET_MYSUPPORT("getMySupport", 3404, 3374), CS_LORDWAR_GET_MYTEAMINFO("getMyteamInfo", 3408, 3378),
            CS_LORDWAR_GET_MYRECORD("getMyRecord", 3412, 3382), CS_LORDWAR_MOBAI("worship", 3416, 3386),
            CS_LORDWAR_GET_TOP("getTempleInfo", 3420, 3390), CS_LORDWAR_GET_SERVER_REWARD("serverReward", 3424, 3394),
            CS_LORDWAR_GET_RECORD_AD("recordAdInfo", 3428, 3398)],
        ("re.lordwar", "tmRe.lordwar") => [
            CS_LORDWAR_REFRESH("refresh", 3433, 3403), CS_LORDWAR_UPDATERANK("updateRank", 3437, 3407),
            CS_LORDWAR_RECEIVE_REWARD("receiveReward", 3441, 3411)],
        ("messageBoard", "tmMessageBoard") => [
            CS_MSGBOARD_GET_MSG_RECORD("getMessageRecord", 3447, 3417), CS_MSGBOARD_SEND_MSG("sendMessage", 3451, 3421),
            CS_MSGBOARD_SEND_PERSONAL("sendCrossPersonal", 3455, 3425), CS_MSGBOARD_LEAVE("leave", 3459, 3429)],
        ("re.messageBoard", "tmRe.messageBoard") => [
            SC_MSGBOARD_GET_MSG("getMsg", 3463, 3433), SC_MSGBOARD_GET_PERSON_MSG("getPersonMsg", 3467, 3437)],
        ("godarmMonster", "tmGodarmMonster") => [
            CS_GODARM_COPY_GETINFO("getInfo", 3483, 3453), CS_GODARM_COPY_ATTACK("attack", 3486, 3456),
            CS_GODARM_COPY_FIGHTPASS("pass", 3489, 3459), CS_GODARM_COPY_BUYATTACKNUM("buyAttackNum", 3492, 3462),
            CS_GODARM_COPY_SWEEP("sweep", 3496, 3466), CS_GODARM_COPY_SWEEP_BY_ASSISTANT("sweepByAssistant", 3498, 3468)],
        ("activityBack", "tmActivityBack") => [
            CS_ACTIVITY_BACK_GETINFO("getInfo", 3504, 3474), CS_ACTIVITY_BACK_RECEIVELOGIN("receiveLogin", 3507, 3477),
            CS_ACTIVITY_BACK_RECEIVERECHARGE("receiveRecharge", 3510, 3480), CS_ACTIVITY_BACK_BUYGIFT("buyGift", 3513, 3483)],
        ("godarmRaffle", "tmGodarmRaffle") => [
            CS_GODARM_RAFFLE_GET_INFO("getInfo", 3519, 3489), CS_GODARM_RAFFLE_SIGNAL("singleRaffle", 3523, 3493),
            CS_GODARM_RAFFLE_MULTI("multiRaffle", 3527, 3497), CS_GODARM_RAFFLE_GET_LUCK("receiveLuckHeroGift", 3531, 3501)],
        ("awakeCopy", "tmAwakeCopy") => [
            CS_AWAKECOPY_GET_INFO("getAwakeCopyInfo", 3556, 3526), CS_AWAKECOPY_BUY_NUM("buyNum", 3560, 3530),
            CS_AWAKECOPY_CREATE_TEAM("createTeam", 3568, 3538), CS_AWAKECOPY_JOIN_TEAM("joinTeam", 3572, 3542),
            CS_AWAKECOPY_AUTO_JOIN_TEAM("joinTeamAuto", 3576, 3546), CS_AWAKECOPY_QUICK_INVITE("quickInvite", 3580, 3550),
            CS_AWAKECOPY_INVITE_FRIEND("getCanInviteFriend", 3584, 3554), CS_AWAKECOPY_INVITE("invite", 3588, 3558),
            CS_AWAKECOPY_READY("ready", 3592, 3562), CS_AWAKECOPY_ROOM_CLOSE("setTeamLimit", 3596, 3566),
            CS_AWAKECOPY_ROOM_OPEN("unsetTeamLimit", 3600, 3570), CS_AWAKECOPY_SET_MULTI("setIsMulti", 3604, 3574)],
        ("team", "tmTeam") => [
            CS_AWAKECOPY_GET_ROOMLIST("getRoomList", 3564, 3534), CS_AWAKECOPY_ENTER("enter", 3625, 3595),
            CS_AWAKECOPY_LEAVE("leave", 3629, 3599), CS_AWAKECOPY_TEAM_UPDATE("update", 3633, 3603),
            CS_AWAKECOPY_TEAM_QUIT("quit", 3637, 3607), CS_AWAKECOPY_TEAM_ADJUST("adjust", 3641, 3611),
            CS_AWAKECOPY_TEAM_KICK("kick", 3645, 3615), CS_AWAKECOPY_START_FIGHT("start", 3649, 3619)],
        ("re.awakeCopy", "tmRe.awakeCopy") => [
            CS_AWAKECOPY_PUSH_BEEN_INVITED("beInvited", 3608, 3578), CS_AWAKECOPY_PUSH_FIGHT_RESULT("donebattle", 3612, 3582),
            CS_AWAKECOPY_PUSH_FIGHT_ERROR("battleError", 3616, 3586), CS_AWAKECOPY_PUSH_MULTI_ERROR("multiError", 3620, 3590)],
        ("activityMonthFund", "tmActivityMonthFund") => [
            CS_ACTIVITY_MONTH_FUND_GETINFO("getInfo", 3656, 3626), CS_ACTIVITY_MONTH_FUND_RECREWARD("recReward", 3659, 3629)],
        ("activityRechargeFund", "tmActivityRechargeFund") => [
            CS_ACTIVITY_RECHARGE_FUND_GETINFO("getInfo", 3664, 3634),
            CS_ACTIVITY_RECHARGE_FUND_RECREWARD("recieveReward", 3667, 3637)],
        ("activityFestivalLanternriddle", "tmActivityFestivalLanternriddle") => [
            CS_ACTIVITY_LANTERN_RIDDLE_GETINFO("getInfo", 3672, 3642), CS_ACTIVITY_LANTERN_RIDDLE_ANSWER("answer", 3675, 3645)],
        ("juBaoPen", "tmJuBaoPen") => [
            CS_ACTIVITY_TREASURE_BOWL_GETINFO("getInfo", 3680, 3650),
            CS_ACTIVITY_TREASURE_BOWL_RECIEVE_REWARD("recieveReward", 3683, 3653)],
        ("activityTimeLimitGift", "tmActivityTimeLimitGift") => [
            CS_ACTIVITY_TIME_LIMIT_GIFT_GETINFO("getInfo", 3686, 3656),
            CS_ACTIVITY_TIME_LIMIT_GIFT_OPEN_BY_FRONT("openByFront", 3688, 3658),
            CS_ACTIVITY_TIME_LIMIT_GIFT_RECEIVE_REWARD("receiveReward", 3690, 3660)],
        ("activityWeekGift", "tmActivityWeekGift") => [
            CS_ACTIVITY_WEEK_GIFT_GETINFO("getInfo", 3695, 3665), CS_ACTIVITY_WEEK_GIFT_RECIEVE_REWARD("receiveReward", 3698, 3668),
            CS_ACTIVITY_WEEK_GIFT_BUY_GIFT("buyGift", 3701, 3671)],
        ("activityPhoto", "tmActivityPhoto") => [
            CS_ACTIVITY_PHOTO_GETINFO("getInfo", 3706, 3676), CS_ACTIVITY_PHOTO_RECIEVE_BOX("receiveBox", 3709, 3679),
            CS_ACTIVITY_PHOTO_TAKE_PHOTO("photo", 3712, 3682),
            CS_ACTIVITY_PHOTO_RECIEVE_SELF_REWARD("receiveSelfReward", 3715, 3685),
            CS_ACTIVITY_PHOTO_RECIEVE_SERVER_REWARD("receiveServerReward", 3718, 3688),
            CS_ACTIVITY_PHOTO_RECIEVE_LOGIN_REWARD("receiveLoginReward", 3721, 3691)],
        ("activityForwardPurchase", "tmActivityForwardPurchase") => [
            CS_ACTIVITY_FORWARD_PURCHASE_GETINFO("getInfo", 3726, 3696),
            CS_ACTIVITY_FORWARD_PURCHASE_USE_VOUCHER("useVoucher", 3729, 3699),
            SC_UPDATE_FORWARD_PURCHASE("updateForwardPurchase", 3732, 3702)],
        ("godAnimalAdventure", "tmGodAnimalAdventure") => [
            CS_GOD_ANIMAL_ADVENTURE_GETINFO("getInfo", 3735, 3705), CS_GOD_ANIMAL_ADVENTURE_ADVENTURE("adventure", 3737, 3707),
            CS_GOD_ANIMAL_ADVENTURE_FLUSH_MAP("flushMap", 3739, 3709),
            CS_GOD_ANIMAL_ADVENTURE_BUY_ADVENTURE_TIMES("buyAdventureTimes", 3741, 3711),
            CS_GOD_ANIMAL_ADVENTURE_GET_REWARD("getReward", 3743, 3713), CS_GOD_ANIMAL_ADVENTURE_INSPIRE("inspire", 3745, 3715),
            CS_GOD_ANIMAL_ADVENTURE_CANCEL_ADVENTURE("cancelAdventure", 3747, 3717)],
        ("countryWar", "tmCountryWar") => [
            CS_NATIONAL_WAR_GETINFO("getInfo", 3750, 3720), CS_NATIONAL_WAR_GET_CITY_INFO("getCityInfo", 3752, 3722),
            CS_NATIONAL_WAR_GET_COUNTRY_INFO("getCountryInfo", 3754, 3724), CS_NATIONAL_WAR_ATTACK("attack", 3756, 3726),
            CS_NATIONAL_WAR_DEFEND("defend", 3758, 3728), CS_NATIONAL_WAR_GET_WORLD_LOG("getWorldLog", 3760, 3730),
            CS_NATIONAL_WAR_GET_PERSONAL_LOG("getPersonalLog", 3762, 3732),
            CS_NATIONAL_WAR_ENTER_COUNTRY_WAR("enterCountryWar", 3764, 3734),
            CS_NATIONAL_WAR_LEAVE_COUNTRY_WAR("leaveCountryWar", 3766, 3736),
            CS_NATIONAL_WAR_GET_BATTLE_RECORD("getBattleRecord", 3768, 3738),
            CS_NATIONAL_WAR_GET_BATTLE_RECORD_IN_CITY("getBattleRecordInCity", 3770, 3740),
            CS_NATIONAL_WAR_GET_KILL_RANK_BY_RECORD_ID("getKillRankByRecordId", 3772, 3742),
            CS_NATIONAL_WAR_GET_DAMAGE_RANK_BY_RECORD_ID("getDamageRankByRecordId", 3774, 3744),
            CS_NATIONAL_WAR_TASK_GETINFO("getTaskInfo", 3776, 3746),
            CS_NATIONAL_WAR_TASK_GET_REWARD("receiveTaskReward", 3778, 3748),
            CS_NATIONAL_WAR_TASK_GET_ALL_REWARD("receiveTaskRewardOneKey", 3780, 3750),
            CS_NATIONAL_WAR_GETWORLDLASTMSGS("getWorldLastMsgs", 3782, 3752),
            CS_NATIONAL_WAR_SENDWORLDMESSAGE("sendWorldMessage", 3783, 3753),
            CS_NATIONAL_WAR_SENDPERSONMESSAGE("sendPersonMessage", 3784, 3754),
            CS_NATIONAL_WAR_ADJUSTPOSITION("adjustPosition", 3788, 3758)],
        ("worldRank", "tmWorldRank") => [
            CS_TOPKING_GET_INFO("getInfo", 3793, 3763), CS_TOPKING_GET_TASK_REWARD("recvTaskReward", 3797, 3767),
            CS_TOPKING_GET_DAILY_REWARD("recvDailyReward", 3801, 3771), CS_TOPKING_TEAM_INFO("getTeamInfo", 3805, 3775),
            CS_TOPKING_MATCH("match", 3809, 3779), CS_TOPKING_RANK("getRankList", 3813, 3783),
            CS_TOPKING_GET_TOP("getTopUser", 3817, 3787), CS_TOPKING_GET_HIS_TOP("getTopUserHis", 3821, 3791),
            CS_TOPKING_GET_SERVER_SKILL("getAllSkillInfo", 3825, 3795), CS_TOPKING_LEVELUP_SKILL("promoteSkill", 3829, 3799),
            CS_TOPKING_GET_TOP_BRID("getTopBrid", 3833, 3803), CS_TOPKING_SET_SKIP("setIsSkip", 3837, 3807)],
        ("animalCopy", "tmAnimalCopy") => [
            CS_GODANIMAL_GETINFO("getInfo", 3849, 3819), CS_GODANIMAL_GETLAYERENEMY("enterLayerEnemy", 3854, 3824),
            CS_GODANIMAL_ATTACK("attack", 3859, 3829), CS_GODANIMAL_OPENBOX("openBox", 3864, 3834),
            CS_GODANIMAL_RESET("reset", 3869, 3839), CS_GODANIMAL_BUYCHALLENGENUM("buyChallengeNum", 3874, 3844)],
        ("kingRoad", "tmKingRoad") => [
            CS_KINGROAD_GETINFO("getInfo", 3884, 3854), CS_KINGROAD_GETSTRONGHINFO("getStronghInfo", 3889, 3859),
            CS_KINGROAD_ATTACK("attack", 3894, 3864), CS_KINGROAD_PASS("pass", 3899, 3869),
            CS_KINGROAD_GETREWARD("getReward", 3904, 3874), CS_KINGROAD_GET_RANK("getRank", 3908, 3878)],
        ("activityMonsterNian", "tmActivityMonsterNian") => [
            CS_ACT_MONSTER_NIAN_GETINFO("getInfo", 3916, 3886), CS_ACT_MONSTER_NIAN_GETSTRONGHINFO("getStronghInfo", 3921, 3891),
            CS_ACT_MONSTER_NIAN_ATTACK("attack", 3926, 3896), CS_ACT_MONSTER_NIAN_PASS("pass", 3931, 3901),
            CS_ACT_MONSTER_NIAN_FETCHREWARD("fetchReward", 3936, 3906)],
        ("starSoul", "tmStarSoul") => [
            CS_STAR_SOUL_GETINFO("getStarSoul", 3944, 3914), CS_STAR_SOUL_UPGRADE("upgradeStarSoul", 3949, 3919),
            CS_STAR_SOUL_SETSKILL("setSkill", 3953, 3923), CS_STAR_SOUL_UNSETSKILL("unsetSkill", 3957, 3927)],
        ("clanRobGrain", "tmClanRobGrain") => [
            CS_ROBGRAIN_GET_MY_INFO("getMyInfo", 3965, 3935), CS_ROBGRAIN_GET_MAP_DATA("getMapData", 3969, 3939),
            CS_ROBGRAIN_REFRESH_MAP("refreshMap", 3973, 3943), CS_ROBGRAIN_GET_INVITE_LIST("getInviteList", 3977, 3947),
            CS_ROBGRAIN_ROB_GRAIN("robGrain", 3981, 3951), CS_ROBGRAIN_REFRESH_CAR("refreshCar", 3985, 3955),
            CS_ROBGRAIN_CALL_CAR("callCar", 3989, 3959), CS_ROBGRAIN_DEPART("depart", 3993, 3963),
            CS_ROBGRAIN_GET_RANKLIST_INFO("getRankListInfo", 3997, 3967),
            CS_ROBGRAIN_GET_CAN_OFFERLIST_INFO("getCanOfferListInfo", 4001, 3971),
            CS_ROBGRAIN_CAN_OFFERLIST_SEARCH("canOfferListSearch", 4005, 3975),
            CS_ROBGRAIN_GET_OFFERLIST("getOfferList", 4009, 3979), CS_ROBGRAIN_OFFER("offer", 4013, 3983),
            CS_ROBGRAIN_GET_CARINFO("getCarInfo", 4017, 3987), CS_ROBGRAIN_GET_GUILD_LEADER_INFO("getGuildPresidentInfo", 4029, 3999)],
        ("re.clanrobgrain", "tmRe.clanrobgrain") => [
            SC_ROBGRAIN_CONVEY_GRAIN_SUCCESS("conveyGrainSuccess", 4021, 3991), SC_ROBGRAIN_BEROBED("beRobed", 4025, 3995)],
        ("activityPowerWheel", "tmActivityPowerWheel") => [
            CS_ACTPOWERWHEEL_GETINFO("getInfo", 4035, 4005), CS_ACTPOWERWHEEL_SIGNAL_WHEEL("singleWheel", 4039, 4009),
            CS_ACTPOWERWHEEL_MULTI_WHEEL("multiWheel", 4043, 4013), CS_ACTPOWERWHEEL_GET_BOX("exchangeBox", 4047, 4017),
            CS_ACTPOWERWHEEL_GET_RANK("getRankInfo", 4051, 4021), CS_ACTPOWERWHEEL_BUY_ITEM("buyToken", 4055, 4025),
            CS_ACTPOWERWHEEL_ENTER("enterRoom", 4059, 4029), CS_ACTPOWERWHEEL_LEAVE("leaveRoom", 4063, 4033)],
        ("happyLottery", "tmHappyLottery") => [
            CS_ACTHAPPYLOTTERY_GETINFO("getInfo", 4073, 4043), CS_ACTHAPPYLOTTERY_DRAWLOTTERY("drawLottery", 4077, 4047),
            CS_ACTHAPPYLOTTERY_DRAWRETURN("drawReturn", 4081, 4051), CS_ACTHAPPYLOTTERY_RECEIVETICKET("receiveTicket", 4085, 4055),
            CS_ACTHAPPYLOTTERY_TICKETREWARDLOGIN("ticketRewardLogin", 4089, 4059),
            CS_ACTHAPPYLOTTERY_TICKETREWARD("ticketReward", 4094, 4064),
            CS_ACTHAPPYLOTTERY_REWARDEXCHANGE("rewardExchange", 4098, 4068),
            CS_ACTHAPPYLOTTERY_HAPPYLOTTERYSHOP("happyLotteryShop", 4102, 4072),
            SC_ACTHAPPYLOTTERY_FINISHTASK("addRewardTask", 4106, 4076)],
        ("protection", "tmProtection") => [
            CS_GODANIAML_ADD_PROTCTION("addGodAnimal", 4113, 4083), CS_GODANIAML_REMOVE_PROTCTION("removeGodAnimal", 4117, 4087),
            CS_GODANIAML_OPEN_PROTCTION("open", 4121, 4091)],
        ("heroLegendRaffle", "tmHeroLegendRaffle") => [
            CS_HEROLEGEND_RAFFLE_BUY_TOKEN("buyToken", 4128, 4098), CS_HEROLEGEND_RAFFLE_RAFFLE("raffle", 4132, 4102),
            CS_HEROLEGEND_RAFFLE_GET_INFO("getInfo", 4136, 4106), CS_HEROLEGEND_RAFFLE_RECEIVE_REWARD("receiveReward", 4140, 4110)],
        ("heroLegend", "tmHeroLegend") => [CS_HEROLEGEND_GET_INFO("getInfo", 4146, 4116), CS_HEROLEGEND_OPEN("open", 4150, 4120)],
        ("heroLegendCopy", "tmHeroLegendCopy") => [
            CS_HEROLEGENDCOPY_GET_INFO("getList", 4156, 4126), CS_HEROLEGENDCOPY_GET_STRONGHINFO("getCopy", 4160, 4130),
            CS_HEROLEGENDCOPY_ATTACK("attack", 4164, 4134), CS_HEROLEGENDCOPY_PASS("pass", 4168, 4138),
            CS_HEROLEGENDCOPY_GET_REWARD("fetchPrize", 4172, 4142),
            CS_HEROLEGENDCOPY_GET_STRONGH_REWARD("fetchBoxReward", 4176, 4146), CS_HEROLEGENDCOPY_SWEEP("sweep", 4180, 4150)],
        ("pillCopy", "tmPillCopy") => [
            CS_PILLCOPY_GET_INFO("getBaseInfo", 4186, 4156), CS_PILLCOPY_GET_STRONGHINFO("getCopy", 4190, 4160),
            CS_PILLCOPY_ATTACK("attack", 4194, 4164), CS_PILLCOPY_PASS("pass", 4198, 4168),
            CS_PILLCOPY_BUY_ATTACK_TIMES("buyAttackTimes", 4202, 4172), CS_PILLCOPY_GET_STAR_REWARD("fetchPrize", 4206, 4176),
            CS_PILLCOPY_GET_STRONGH_REWARD("fetchBoxReward", 4210, 4180), CS_PILLCOPY_SWEEP("sweep", 4214, 4184)],
        ("darkGoldCopy", "tmDarkGoldCopy") => [
            CS_DARKGOLDCOPY_GET_INFO("getBaseInfo", 4220, 4190), CS_DARKGOLDCOPY_GET_STRONGHINFO("getCopy", 4224, 4194),
            CS_DARKGOLDCOPY_ATTACK("attack", 4228, 4198), CS_DARKGOLDCOPY_PASS("pass", 4232, 4202),
            CS_DARKGOLDCOPY_BUY_ATTACK_TIMES("buyAttackTimes", 4236, 4206),
            CS_DARKGOLDCOPY_GET_STAR_REWARD("fetchPrize", 4240, 4210),
            CS_DARKGOLDCOPY_GET_STRONGH_REWARD("fetchBoxReward", 4244, 4214), CS_DARKGOLDCOPY_SWEEP("sweep", 4248, 4218)],
        ("jointSkillCopy", "tmJointSkillCopy") => [
            CS_JOINTSKILL_COPY_GET_INFO("getBaseInfo", 4254, 4224), CS_JOINTSKILL_COPY_GET_STRONGHINFO("getCopy", 4258, 4228),
            CS_JOINTSKILL_COPY_ATTACK("attack", 4262, 4232), CS_JOINTSKILL_COPY_PASS("pass", 4266, 4236),
            CS_JOINTSKILL_COPY_BUY_ATTACK_TIMES("buyAttackTimes", 4270, 4240),
            CS_JOINTSKILL_COPY_GET_STAR_REWARD("fetchPrize", 4274, 4244),
            CS_JOINTSKILL_COPY_GET_STRONGH_REWARD("fetchBoxReward", 4278, 4248), CS_JOINTSKILL_COPY_SWEEP("sweep", 4282, 4252)],
        ("crossClanBattle", "tmCrossClanBattle") => [
            CS_CROSSCLANBATTLE_ENTER("enter", 4290, 4260), CS_CROSSCLANBATTLE_LEAVE("leave", 4294, 4264),
            CS_CROSSCLANBATTLE_GETINFO("getMyInfo", 4298, 4268), CS_CROSSCLANBATTLE_SIGN("register", 4302, 4272),
            CS_CROSSCLANBATTLE_BATTLETEAM("getGuildMemberInfo", 4306, 4276),
            CS_CROSSCLANBATTLE_SAVE_BATTLETEAM("saveCombatants", 4310, 4280),
            CS_CROSSCLANBATTLE_MYRECORD("getMyGuildRecord", 4314, 4284),
            CS_CROSSCLANBATTLE_GET_MYGROUPRECORD("getRecordById", 4318, 4288),
            CS_CROSSCLANBATTLE_GET_MYDETAILRECORD("getBtlById", 4322, 4292),
            CS_CROSSCLANBATTLE_ROUNDINFO("getPromotionInfo", 4326, 4296),
            CS_CROSSCLANBATTLE_HISROUNDINFO("getHisPromotionInfo", 4330, 4300),
            CS_CROSSCLANBATTLE_UPDATEPOWER("updateFightInfo", 4334, 4304), CS_CROSSCLANBATTLE_SUPPORT("bet", 4338, 4308),
            CS_CROSSCLANBATTLE_MY_SUPPORT("getMySupportInfo", 4342, 4312),
            CS_CROSSCLANBATTLE_BATTLE_RECORD("getPromotionBtl", 4346, 4316), CS_CROSSCLANBATTLE_BUY_STREAK("buyStreak", 4350, 4320),
            CS_CROSSCLANBATTLE_INSPIRE("inspirate", 4354, 4324), CS_CROSSCLANBATTLE_GET_INSINFO("getGuildInsInfo", 4358, 4328),
            CS_CROSSCLANBATTLE_GET_TOP("getTempleInfo", 4362, 4332), CS_CROSSCLANBATTLE_MOBAI("worship", 4366, 4336),
            CS_CROSSCLANBATTLE_GETSUPPORTNUM("getBetNum", 4370, 4340), CS_CROSSCLANBATTLE_GET_SERVER("getServerList", 4382, 4352),
            CS_CROSSCLANBATTLE_GET_SERVERREWARD("serverReward", 4390, 4360),
            CS_CROSSCLANBATTLE_GET_BATTLETEAM("getPromotionTeamList", 4394, 4364),
            CS_CROSSCLANBATTLE_GET_CRASHREPORT("getCrashById", 4398, 4368)],
        ("re.crossclanbattle", "tmRe.crossclanbattle") => [
            CS_CROSSCLANBATTLE_PUSH_ROUND("refresh", 4374, 4344), CS_CROSSCLANBATTLE_PUSH_RANK("updateRank", 4378, 4348),
            CS_CROSSCLANBATTLE_PUSH_SERVERREWARD("receiveReward", 4386, 4356)],
        ("brochureCopy", "tmBrochureCopy") => [
            CS_BROCHURECOPY_GETINFO("getInfo", 4405, 4375), CS_BROCHURECOPY_GETRANK("getRank", 4409, 4379),
            CS_BROCHURECOPY_GETINFO_ENTER("getInfoEnter", 4413, 4383),
            CS_BROCHURECOPY_BUYCHALLENGENUM("buyChallengeNum", 4418, 4388), CS_BROCHURECOPY_ATTACK("attack", 4422, 4392),
            CS_BROCHURECOPY_REWARD_EVENT("getRewardEvent", 4426, 4396), CS_BROCHURECOPY_CHEST_EVENT("getChestEvent", 4430, 4400),
            CS_BROCHURECOPY_BROCHURE_EVENT("getBrochureEvent", 4434, 4404),
            CS_BROCHURECOPY_CHECK_FORMATION("checkFormation", 4438, 4408), CS_BROCHURECOPY_SWEEP("sweep", 4442, 4412)],
        ("activityAnswer", "tmActivityAnswer") => [
            CS_MILLIONQUIZ_GETINFO("getInfo", 4447, 4417), CS_MILLIONQUIZ_GET_FIRST_QUESTION("getFirstQuestion", 4451, 4421),
            CS_MILLIONQUIZ_ANSWER("answer", 4455, 4425), CS_MILLIONQUIZ_GET_REWARD("receiveReward", 4459, 4429),
            CS_MILLIONQUIZ_REVIVE("revive", 4463, 4433), CS_MILLIONQUIZ_ROB_REDBAG("robRedBag", 4467, 4437),
            CS_MILLIONQUIZ_ENTER("enter", 4471, 4441), CS_MILLIONQUIZ_LEAVE("leave", 4475, 4445)],
        ("mergeLoginReward", "tmMergeLoginReward") => [
            CS_MERGE_LOGINREWARD("getInfo", 4490, 4460), CS_ACTIVITY_MERGE_LOGIN_REWARD_MAKE("make", 4492, 4462)],
        ("mergeRecharge", "tmMergeRecharge") => [
            CS_MERGE_RECHARGE("getInfo", 4494, 4464), CS_ACTIVITY_MERGE_RECHARGE_GET_REWARD("recieveReward", 4496, 4466)],
        ("mergeDiscount", "tmMergeDiscount") => [
            CS_ACTIVITY_MERGE_DISCOUNT_INFO("getInfo", 4498, 4468), CS_ACTIVITY_MERGE_DISCOUNT_BUY("buy", 4500, 4470)],
        ("huntDeer", "tmHuntDeer") => [
            CS_HUNTDEER_GET_INFO("getInfo", 4506, 4476), CS_HUNTDEER_ENTER_HALL("enterHall", 4510, 4480),
            CS_HUNTDEER_LEAVE_HALL("leave", 4514, 4484), CS_HUNTDEER_ENTER_CITY("enterCity", 4518, 4488),
            CS_HUNTDEER_GET_PAGEINFO("getPositionInfo", 4522, 4492), CS_HUNTDEER_SET_CONVERGE("converge", 4526, 4496),
            CS_HUNTDEER_GET_TRACEINFO("getEnemyInfo", 4530, 4500), CS_HUNTDEER_REFRESH_TRACE("refreshEnemy", 4534, 4504),
            CS_HUNTDEER_ATTACK("attack", 4538, 4508), CS_HUNTDEER_GET_TASK_REWARD("receiveTaskReward", 4542, 4512),
            CS_HUNTDEER_ONE_KEY_GET_TASK_REWARD("oneKeyReceiveTaskReward", 4546, 4516),
            CS_HUNTDEER_GET_USERRANK("getUserRankList", 4550, 4520), CS_HUNTDEER_GET_GUILDRANK("getGuildRankList", 4554, 4524),
            CS_HUNTDEER_GET_CITYREPORT("getCityGuildInfo", 4558, 4528), CS_HUNTDEER_GET_MEMBERREPORT("getMemberInfo", 4562, 4532),
            CS_HUNTDEER_GET_MYREPORT("getSelfBr", 4566, 4536), CS_HUNTDEER_BUY_TRACE("buyChaseNum", 4570, 4540),
            CS_HUNTDEER_GET_ENDRANK("getTop", 4574, 4544), CS_HUNTDEER_GET_CITYLEGIONRANK("getCityGuildRankList", 4578, 4548),
            CS_HUNTDEER_QUICK_TRACE("sweepEnemy", 4586, 4556)],
        ("opencarnivalthree", "tmOpencarnivalthree") => [
            CS_ACT_THIRTY_DAY_GET_INFO("getOpenAchieve", 4593, 4563),
            CS_ACT_THIRTY_DAY_GET_ACTIVE_REWARD("receiveActiveReward", 4598, 4568),
            CS_ACT_THIRTY_DAY_GET_DAILY_REWARD("receiveDailyReward", 4603, 4573),
            CS_ACT_THIRTY_DAY_GET_DISCOUNT_REWARD("receiveDiscountReward", 4608, 4578),
            CS_ACT_THIRTY_DAY_BUY_DISCOUNT("buyDiscount", 4613, 4583),
            CS_ACT_THIRTY_DAY_GET_RECHARGE_REWARD("receiveRechargeReward", 4618, 4588)],
        ("heroGoldTask", "tmHeroGoldTask") => [
            CS_HERO_GOLD_TASK_GET_INFO("getInfo", 4625, 4595), CS_HERO_GOLD_TASK_GET_RANK_LIST("getRankList", 4630, 4600),
            CS_HERO_GOLD_TASK_GET_TOP3("getTop3", 4635, 4605), CS_HERO_GOLD_TASK_REFRESH_TASK("refreshTask", 4640, 4610),
            CS_HERO_GOLD_TASK_ACCEPT_TASK("acceptTask", 4645, 4615), CS_HERO_GOLD_TASK_CONTRIBUTE("contribute", 4650, 4620),
            CS_HERO_GOLD_TASK_GET_REWARD("fetchReward", 4655, 4625)],
        ("superSign", "tmSuperSign") => [
            CS_SUPER_CHECK_IN_GET_INFO("getInfo", 4664, 4634), CS_SUPER_CHECK_IN_SIGN("sign", 4666, 4636),
            CS_SUPER_CHECK_IN_FETCH_REWARD("receiveAccReward", 4668, 4638)],
        ("activityWeekFund", "tmActivityWeekFund") => [
            CS_NEW_WEEK_FUND_GET_INFO("getInfo", 4672, 4642), CS_NEW_WEEK_FUND_GET_REWARD("fetchReward", 4674, 4644),
            CS_NEW_WEEK_FUND_GET_BOX_REWARD("fetchAllReward", 4676, 4646)],
        ("activity3KCelebration", "tmActivity3KCelebration") => [
            CS_ACT_3K_CELEBRATION_GETINFO("getInfo", 4679, 4649), CS_ACT_3K_CELEBRATION_REFRESH_TASK("refreshTask", 4681, 4651),
            CS_ACT_3K_CELEBRATION_DONATE("donate", 4683, 4653), CS_ACT_3K_CELEBRATION_BUY_TASK("buyTask", 4685, 4655),
            CS_ACT_3K_CELEBRATION_COMPLETE_TASK("completeTask", 4687, 4657),
            CS_ACT_3K_CELEBRATION_OPEN_CHEST("openChest", 4689, 4659), CS_ACT_3K_CELEBRATION_SIGNIN("signIn", 4691, 4661),
            CS_ACT_3K_CELEBRATION_COMPLETE_RECHARGE("completeRecharge", 4693, 4663),
            CS_ACT_3K_CELEBRATION_EXCHANGE("exchange", 4695, 4665)],
        ("re.activity3KCelebration", "tmRe.activity3KCelebration") => [
            SC_ACT_3K_CELEBRATION_TASK_CHG("taskChg", 4697, 4667), SC_ACT_3K_CELEBRATION_RECHARGE_CHG("rechargeChg", 4699, 4669)],
        ("worldTrinity", "tmWorldTrinity") => [
            CS_TEAMBATTLE_CS_NOTICE_INFO("getNoticeInfo", 4702, 4672), CS_TEAMBATTLE_CS_GET_BASE_INFO("getBaseInfo", 4704, 4674),
            CS_TEAMBATTLE_CS_LEAVE("leave", 4706, 4676), CS_TEAMBATTLE_CS_GET_INVITE_LIST("getInviteList", 4708, 4678),
            CS_TEAMBATTLE_CS_GET_TEAMMEMBER_FORMATION("getUserFormationInfo", 4710, 4680),
            CS_TEAMBATTLE_CS_ONE_BTN_INVITE("autoInviteRobot", 4712, 4682), CS_TEAMBATTLE_CS_INVITE("invite", 4714, 4684),
            CS_TEAMBATTLE_CS_GET_FORMATION("getTeamFormation", 4716, 4686),
            CS_TEAMBATTLE_CS_GET_FORMATION_TID("getTeamFormationByTid", 4718, 4688),
            CS_TEAMBATTLE_CS_UPLOAD_FORMATION("updateBattleInfo", 4720, 4690),
            CS_TEAMBATTLE_CS_CHANGE_FORMATION("changeFormation", 4722, 4692), CS_TEAMBATTLE_CS_QUIT_TEAM("quitTeam", 4724, 4694),
            CS_TEAMBATTLE_CS_ENTER_TEAM("enterRoom", 4726, 4696), CS_TEAMBATTLE_CS_CREATE_TEAM("createTeam", 4728, 4698),
            CS_TEAMBATTLE_CS_READY("ready", 4730, 4700), CS_TEAMBATTLE_CS_KICK_OUT("kickOut", 4732, 4702),
            CS_TEAMBATTLE_CS_SIGN_UP("register", 4734, 4704), CS_TEAMBATTLE_CS_MATCH("matchEnemy", 4736, 4706),
            CS_TEAMBATTLE_CS_TEAM_CHAT("teamChat", 4738, 4708), CS_TEAMBATTLE_CS_GET_MY_REPRORT("getBattleList", 4740, 4710),
            CS_TEAMBATTLE_CS_GET_BATTLE_BY_ID("getBattleRecordByRecordId", 4742, 4712),
            CS_TEAMBATTLE_CS_GET_PROMOTION_REPORT("getPromotionReport", 4744, 4714),
            CS_TEAMBATTLE_CS_GET_PROMOTION_INFO("getPromotionInfo", 4746, 4716),
            CS_TEAMBATTLE_CS_GET_RANK_LIST("getRankList", 4748, 4718),
            CS_TEAMBATTLE_CS_GET_ZHUWEI_LIST("getSupportList", 4750, 4720), CS_TEAMBATTLE_CS_SEND_ZHUWEI("support", 4752, 4722),
            CS_TEAMBATTLE_CS_GET_OTHER_FORMATION("getOtherTeamFormation", 4754, 4724)],
        ("re.worldTrinity", "tmRe.worldTrinity") => [
            SC_TEAMBATTLE_CS_INVITED("invited", 4756, 4726), SC_TEAMBATTLE_CS_READY("ready", 4758, 4728),
            SC_TEAMBATTLE_CS_KICK_OUT("kickOut", 4760, 4730), SC_TEAMBATTLE_CS_SIGN_UP("register", 4762, 4732),
            SC_TEAMBATTLE_CS_TEAM_CHAT("teamChat", 4764, 4734), SC_TEAMBATTLE_CS_REFRESH_TEAM("refreshTeam", 4766, 4736),
            SC_TEAMBATTLE_CS_PROMOTION_STATE("promotionState", 4768, 4738)],
        ("activityFestivalCeremony", "tmActivityFestivalCeremony") => [
            CS_ACT_FESTIVAL_CEREMONY_GETINFO("getInfo", 4771, 4741),
            CS_ACT_FESTIVAL_CEREMONY_GETSCOREREWARD("fetchScoreReward", 4773, 4743),
            CS_ACT_FESTIVAL_CEREMONY_GETRECHARGEREWARD("fetchRechargeReward", 4775, 4745),
            CS_ACT_FESTIVAL_CEREMONY_ONEKEYFETCHREWARD("oneKeyFetchReward", 4777, 4747)],
        ("twinsBattle", "tmTwinsBattle") => [
            CS_TWINSBATTLE_GET_BASEINFO("getBaseInfo", 4780, 4750), CS_TWINSBATTLE_GET_ENTERINFO("getEnterInfo", 4782, 4752),
            CS_TWINSBATTLE_GET_RANKLIST("getRankList", 4784, 4754), CS_TWINSBATTLE_CREATE_TEAM("createTeam", 4786, 4756),
            CS_TWINSBATTLE_TEAM_CHAT("chat", 4788, 4758), CS_TWINSBATTLE_AUTO_INVITE_ROBO("autoInviteRobot", 4790, 4760),
            CS_TWINSBATTLE_READY("ready", 4792, 4762), CS_TWINSBATTLE_KICK("kick", 4794, 4764),
            CS_TWINSBATTLE_GET_INVITELIST("getInviteList", 4796, 4766), CS_TWINSBATTLE_INVITE("invite", 4798, 4768),
            CS_TWINSBATTLE_ENTER_TEAM("enterTeam", 4800, 4770), CS_TWINSBATTLE_LEAVE_TEAM("leaveTeam", 4802, 4772),
            CS_TWINSBATTLE_REG_TEAM("regTeam", 4804, 4774), CS_TWINSBATTLE_ATTACK("attack", 4806, 4776),
            CS_TWINSBATTLE_FETCH_TASKREWARD("fetchTaskReward", 4808, 4778),
            CS_TWINSBATTLE_FETCH_TASKREWARD_ONE_KEY("fetchTaskRewardOneKey", 4810, 4780),
            CS_TWINSBATTLE_USE_ITEM("useItem", 4812, 4782), CS_TWINSBATTLE_SWEEP("sweep", 4814, 4784),
            CS_TWINSBATTLE_CHANGE_FORMATION("changeFormation", 4816, 4786),
            CS_TWINSBATTLE_GET_PLAYER_FORMATION("getPlayerFormation", 4818, 4788),
            CS_TWINSBATTLE_GET_ENEMY_FORMATION("getEnemyFormation", 4820, 4790),
            CS_TWINSBATTLE_GET_REPORTS("getReports", 4822, 4792), CS_TWINSBATTLE_GET_REPORT_BY_BRID("getReportByBrid", 4824, 4794),
            CS_TWINSBATTLE_REFRESH_BATTLE_INFO("refreshBattleInfo", 4826, 4796),
            CS_TWINSBATTLE_LEAVE_TEAM_AFTER("leaveTeamAfterReg", 4828, 4798)],
        ("re.twinsBattle", "tmRe.twinsBattle") => [
            SC_TWINSBATTLE_RECEIVE_INVITE("receiveInvite", 4830, 4800), SC_TWINSBATTLE_TEAM_REFRESH("teamRefresh", 4832, 4802),
            SC_TWINSBATTLE_ON_KICKOUT("onKickOut", 4834, 4804), SC_TWINSBATTLE_ON_RECEIVE_CHAT("onRecieveChat", 4836, 4806),
            SC_TWINSBATTLE_ON_REG_TEAM("onRegTeam", 4838, 4808), SC_TWINSBATTLE_ON_COMPLETE_TASK("onCompleteTask", 4840, 4810)],
        ("activityTKCeremony", "tmActivityTKCeremony") => [
            CS_ACTIVITY_TKCEREMONY_GET_BASE_INFO("getBaseInfo", 4843, 4813), CS_ACTIVITY_TKCEREMONY_BUY_EXP("buyExp", 4845, 4815),
            CS_ACTIVITY_TKCEREMONY_FETCH_TASK_REWARD("fetchTaskReward", 4847, 4817),
            CS_ACTIVITY_TKCEREMONY_FETCH_ALL_TASK_REWARD("fetchAllTaskReward", 4849, 4819),
            CS_ACTIVITY_TKCEREMONY_FETCH_REWARD("fetchReward", 4851, 4821),
            CS_ACTIVITY_TKCEREMONY_FETCH_ALL_REWARD("fetchAllReward", 4853, 4823),
            CS_ACTIVITY_TKCEREMONY_FETCH_DISCOUNT_REWARD("fetchDiscountReward", 4855, 4825)],
        ("re.activityTKCeremony", "tmRe.activityTKCeremony") => [
            SC_ACTIVITY_TKCEREMONY_ADD_EXTRA_EXP("addExtraExp", 4857, 4827),
            SC_ACTIVITY_TKCEREMONY_ON_RECHARGE("onRecharge", 4859, 4829),
            SC_ACTIVITY_TKCEREMONY_REFRESH_TASK("refreshTask", 4861, 4831)],
        ("activityFestivalFireworks", "tmActivityFestivalFireworks") => [
            CS_ACTIVITY_FIREWORK_INFO("getInfo", 4871, 4841), CS_ACTIVITY_FIREWORK_EXCHANGE("exchangeFireworks", 4872, 4842),
            CS_ACTIVITY_FIREWORK_SET("setFireworks", 4873, 4843), CS_ACTIVITY_FIREWORK_RECEIVE("receiveFireworksReward", 4874, 4844),
            CS_ACTIVITY_FIREWORK_SEEKFIREWORKS("seekFireworks", 4877, 4847)],
        ("re.activity_festival_fireworks", "tmRe.activity_festival_fireworks") => [
            SC_ACTIVITY_FIREWORK_ONRECEIVE("onRecvReward", 4875, 4845), SC_ACTIVITY_FIREWORK_ONLOOK("onLookReward", 4876, 4846)],
        ("celebration", "tmCelebration") => [
            CS_ACTIVITY_CAKE_GETINFO("getInfo", 4882, 4852), CS_ACTIVITY_CAKE_ADDCREAM("addCream", 4884, 4854),
            CS_ACTIVITY_CAKE_BUYCREAM("buyCream", 4886, 4856), CS_ACTIVITY_CAKE_GETREWARD("getReward", 4888, 4858)],
        ("assistant", "tmAssistant") => [
            CS_ASSISTANT_GETINFO("getInfo", 4899, 4869), CS_ASSISTANT_SETOPEN("setOpen", 4901, 4871),
            CS_ASSISTANT_SETAUTOUSE("setAutoUse", 4903, 4873), CS_ASSISTANT_SETPARAMS("setParams", 4905, 4875),
            CS_ASSISTANT_RUNASSIST("runAssist", 4907, 4877)],
        ("imperialcity", "tmImperialcity") => [
            CS_ROYALCITY_ENTER("enter", 4912, 4882), CS_ROYALCITY_LEAVE("leave", 4915, 4885), CS_ROYALCITY_MOVE("move", 4918, 4888)],
        ("town", "tmTown") => [
            SC_ROYALCITY_ADDPLAYER("addPlayer", 4921, 4891), SC_ROYALCITY_DELPLAYER("delPlayer", 4924, 4894),
            SC_ROYALCITY_MOVE("move", 4927, 4897)],
        ("survival", "tmSurvival") => [
            CS_CHICKEN_CITY_GETINFO("getInfo", 4954, 4924), CS_CHICKEN_CITY_REGISTER("register", 4955, 4925),
            CS_CHICKEN_CITY_GETAREAINFO("getEnemyInfo", 4975, 4945), CS_CHICKEN_CITY_GETBUFF("getBuff", 4976, 4946),
            CS_CHICKEN_CITY_BUYCHALLENGENUM("buyChallengeNum", 4980, 4950),
            CS_CHICKEN_CITY_GETDETAILINFO("getDetailInfo", 4982, 4952), CS_CHICKEN_CITY_CHALLENGE("challenge", 4999, 4969),
            CS_CHICKEN_CITY_UPDATEFMT("updateFmt", 5001, 4971), CS_CHICKEN_CITY_ENTER("enterSurvival", 5003, 4973),
            CS_CHICKEN_CITY_EXITE("leaveSurvival", 5005, 4975)],
        ("re.survival", "tmRe.survival") => [
            SC_CHICKEN_CITY_GETMYPOSITION("challenge", 5007, 4977), SC_CHICKEN_CITY_NEWS("news", 5009, 4979),
            SC_CHICKEN_CITY_AUTO_REGISTER("autoRegister", 5011, 4981)],
        ("activityJigsaw", "activityJigsaw") => [
            CS_ACTIVITY_JIGSAW_GETINFO("getInfo", 5013, 4983), CS_ACTIVITY_JIGSAW_GIVELOVE("giveLove", 5014, 4984),
            CS_ACTIVITY_JIGSAW_RECIVEREWARD("recieveReward", 5015, 4985), CS_ACTIVITY_JIGSAW_OPENlETTER("openLetter", 5016, 4986)],
        ("babeltower", "tmBabeltower") => [CS_BABELTOWER_GETINFO("getInfo", 5021, 4991), CS_BABELTOWER_SWEEP("sweep", 5022, 4992)],
        ("babeltowerShop", "babeltowerShop") => [
            CS_BABELTOWER_SHOP_GETINFO("getShopInfo", 5023, 4993), CS_BABELTOWER_SHOP_BUYGOODS("buyGoods", 5024, 4994)],
        ("contest", "tmContest") => [
            CS_CONTEST_ENTER("enterContest", 5026, 4996), CS_CONTEST_EXIT("leaveContest", 5027, 4997),
            CS_CONTEST_GETINFO("getContestInfo", 5028, 4998), CS_CONTEST_GETPAGEINFO("getPageInfo", 5050, 5020),
            CS_CONTEST_UPDATEFIGHTINFO("updateFightInfo", 5052, 5022), CS_CONTEST_TEMPLEINFO("getTempleInfo", 5054, 5024),
            CS_CONTEST_SUPPORT("support", 5056, 5026), CS_CONTEST_MYSUPPORT("getMySupport", 5058, 5028),
            CS_CONTEST_GETAUDITIONRECORD("getAuditionRecord", 5060, 5030),
            CS_CONTEST_GETBATTLEDATA("getBattleDataOfUser", 5064, 5034), CS_CONTEST_CHALLENGE("challenge", 5066, 5036),
            CS_CONTEST_GETPROMOTIONiNFO("getPromotionInfo", 5068, 5038),
            CS_CONTEST_GETPROMOTIONREPORT("getPromotionReport", 5070, 5040)],
        ("activitySaveGoldExchange", "tmActivitySaveGoldExchange") => [
            CS_SAVE_GOLD_GETINFO("getInfo", 5075, 5045), CS_SAVE_GOLD_TASK_REWARD("getTaskReward", 5077, 5047),
            CS_SAVE_GOLD_EXCHANGE_REWARD("exchangeReward", 5079, 5049), CS_SAVE_GOLD_GET_REWARD("getReward", 5081, 5051)],
        ("chess", "tmChess") => [
            CS_CHESS_GETINGO("getInfo", 5086, 5056), CS_CHESS_MATCH("startMatch", 5088, 5058), CS_CHESS_READY("ready", 5095, 5065),
            CS_CHESS_FRESH_SHOP("refreshShop", 5103, 5073), CS_CHESS_BUYHERO("buy", 5105, 5075),
            CS_CHESS_CHANGE_FORMATION("changeFormation", 5107, 5077), CS_CHESS_FETCH_BATTLEINFO("fetchBattleInfo", 5109, 5079),
            CS_CHESS_BUY_EXP("buyExp", 5113, 5083), CS_CHESS_SHOP_LOCKED("lockShop", 5115, 5085),
            CS_CHESS_GETRANK("getRank", 5118, 5088), CS_CHESSBAG_GETBAGINFO("getBag", 5124, 5094),
            CS_CHESS_DRESSON_EQUIP("equip", 5128, 5098)],
        ("re.chess", "tmRe.chess") => [
            SC_CHESS_MATCH_SUCCESS("matchDone", 5091, 5061), SC_CHESS_MATCH_FAIL("matchFail", 5093, 5063),
            SC_CHESS_READY("ready", 5097, 5067), SC_CHESS_READY_TIMEOUT("readyTimeOut", 5099, 5069),
            SC_CHESS_READY_ALLSUCCESS("createTeam", 5101, 5071), SC_CHESS_BATTLE_DONE("battleDone", 5111, 5081)],
        ("chess", "chess") => [CS_CHESS_SELL_HERO("sell", 5120, 5090), CS_CHESS_POINT_RANKLIST("getRankList", 5122, 5092)],
        ("trialRoad", "tmTrialRoad") => [
            CS_TRIALROAD_GETINFO("getInfo", 5133, 5103), CS_TRIALROAD_GETENEMYINFO("getEnemyInfo", 5135, 5105),
            CS_TRIALROAD_ATTACK("attack", 5137, 5107), CS_TRIALROAD_REFRESH("refresh", 5139, 5109),
            CS_TRIALROAD_REPEL("repel", 5141, 5111), CS_TRIALROAD_GETTASKINFO("getTaskList", 5143, 5113),
            CS_TRIALROAD_RECIEVE_TASK_REWATD("recieveTaskReward", 5145, 5115),
            CS_TRIALROAD_SCORE_RECORD("getScoreRecord", 5147, 5117), CS_TRIALROAD_RANK_INFO("getRankList", 5149, 5119)],
        ("classicWar", "tmClassicWar") => [
            CS_CLASSICSWAR_GETINFO("getClassicWarInfo", 5154, 5124), CS_CLASSICSWAR_GETTEAMINFO("getMyTeamInfo", 5156, 5126),
            CS_CLASSICSWAR_CREATETEAM("createTeam", 5158, 5128), CS_CLASSICSWAR_MATCHTEAM("matchTeam", 5160, 5130),
            CS_CLASSICSWAR_QUITTEAM("quit", 5162, 5132), CS_CLASSICSWAR_ONEKEYINVITE("oneKeyInvite", 5164, 5134),
            CS_CLASSICSWAR_INVITE("invite", 5166, 5136), CS_CLASSICSWAR_KICKOUT("kick", 5168, 5138),
            CS_CLASSICSWAR_READY("ready", 5170, 5140), CS_CLASSICSWAR_CANCLEREADY("cancelReady", 5172, 5142),
            CS_CLASSICSWAR_SETOPENROOM("setOpenStatus", 5174, 5144), CS_CLASSICSWAR_STARTFIGHT("start", 5176, 5146),
            CS_CLASSICSWAR_TEAMFORMATION("getFormation", 5178, 5148), CS_CLASSICSWAR_BUYREWARDNUM("buyRewardNum", 5180, 5150),
            CS_CLASSICWAR_ADJUSET("adjust", 5184, 5154), CS_CLASSICWAR_GETEMEMBER("getGuildMember", 5186, 5156),
            CS_CLASSICWAR_GETRANKLIST("getRankList", 5188, 5158), CS_CLASSICWAR_CHART("sendInfo", 5190, 5160)],
        ("push.classicwar", "tmPush.classicwar") => [
            SC_CLASSICSWAR_JOIN("join", 5193, 5163), SC_CLASSICSWAR_QUIT("quit", 5195, 5165),
            SC_CLASSICSWAR_READY("ready", 5197, 5167), SC_CLASSICSWAR_STARTMATCH("startMatch", 5199, 5169),
            SC_CLASSICSWAR_STOPMATCH("stopMatch", 5201, 5171), SC_CLASSICSWAR_DONEBATTLE("doneBattle", 5203, 5173),
            SC_CLASSICSWAR_TEAMDISMISS("dismiss", 5207, 5177), SC_CLASSICSWAR_INVITE("invite", 5209, 5179),
            SC_CLASSICSWAR_LEAGERCHANGE("changeLeader", 5211, 5181), SC_CLASSICSWAR_CHAT("chat", 5216, 5186),
            SC_CLASSICSWAR_KICK("kick", 5218, 5188)],
        ("push.classicwar", "push.classicwar") => [
            SV_CLASSICSWAR_ADJUST("adjust", 5205, 5175), SC_CLASSICSWAR_OVERTIMEDISMISS("overtimeDismiss", 5213, 5183)],
        ("soulSkill", "tmSoulSkill") => [
            CS_SOULSKILL_ADD("addSoulSkill", 5222, 5192), CS_SOULSKILL_REMOVE("removeSoulSkill", 5223, 5193),
            CS_SOULSKILL_ADD_MULTI("addMultiSoulSkill", 5224, 5194)],
        ("activity4YearMission", "tmActivity4YearMission") => [
            CS_ACT4YEARMISSION_GETINFO("getInfo", 5234, 5204), CS_ACT4YEARMISSION_REFRESHTASK("refreshTask", 5236, 5206),
            CS_ACT4YEARMISSION_RECIEVETASKTEWARD("recieveTaskReward", 5238, 5208),
            CS_ACT4YEARMISSION_EXCHANGE("exchange", 5240, 5210), CS_ACT4YEARMISSION_OPENCHEST("openChest", 5242, 5212),
            CS_ACT4YEARMISSION_RECIEVERECHARGE("recieveRecharge", 5244, 5214)],
        ("statue", "tmStatue") => [
            CS_STATUE_GETINFO("getInfo", 5250, 5220), CS_STATUE_LIGHT("light", 5251, 5221), CS_STATUE_BREAK("break", 5252, 5222),
            CS_STATUE_UPGRADE("upgrade", 5253, 5223)],
        ("statueCopy", "tmStatueCopy") => [
            CS_STATUECOPY_GET_BASEINFO("getBaseInfo", 5256, 5226), CS_STATUECOPY_GETCOPY("getCopy", 5260, 5230),
            CS_STATUECOPY_ATTACK("attack", 5263, 5233), CS_STATUECOPY_PASS("pass", 5266, 5236),
            CS_STATUECOPY_BUY_ATTACK_TIMES("buyAttackTimes", 5269, 5239), CS_STATUECOPY_GET_STAR_REWARD("fetchPrize", 5272, 5242),
            CS_STATUECOPY_SWEEP("sweep", 5278, 5248)],
        ("bingshu", "tmBingshu") => [
            CS_BINGSHU_GETINFO("getInfo", 5281, 5251), CS_BINGSHU_COMPOSE("compose", 5282, 5252),
            CS_BINGSHU_LEVEL_UP("levelUp", 5283, 5253), CS_BINGSHU_UPGRADE("upgrade", 5284, 5254),
            CS_BINGSHU_EQUIP("equip", 5285, 5255), CS_BINGSHU_UNLOAD("unload", 5286, 5256), CS_BINGSHU_REBORN("reborn", 5287, 5257)],
        ("job", "tmJob") => [
            CS_JOB_GETINFO("getInfo", 5290, 5260), CS_JOB_UPGRADE("upgrade", 5291, 5261), CS_JOB_SETNAME("setName", 5292, 5262),
            CS_JOB_SKILL_LEVEL_UP("upgradeSkill", 5293, 5263), CS_JOB_SKILL_EQUIP("setSkill", 5294, 5264),
            CS_JOB_SKILL_UNLOAD("unLoad", 5295, 5265), CS_JOB_SKILL_REBORN("resetSkill", 5296, 5266),
            CS_JOB_SKILL_COMPOSE("getSkill", 5297, 5267)],
        ("wash", "tmWash") => [CS_HERO_WASH_GETINFO("getInfo", 5300, 5270), CS_HERO_WASH_GETBOX("getBox", 5301, 5271)],
        ("collection", "tmCollection") => [
            CS_COLLECTION_GET_INFO("getInfo", 5304, 5274), CS_COLLECTION_UNLOCK("unlock", 5305, 5275),
            CS_COLLECTION_GET_OTHER_INFO("getOtherInfo", 5306, 5276), CS_COLLECTION_LIKE("like", 5307, 5277),
            CS_COLLECTION_GET_SCORE_REWARD("getScoreReward", 5308, 5278), CS_COLLECTION_GET_LIKE_REWARD("getLikeReward", 5309, 5279)],
        ("activityWeekly", "tmActivityWeekly") => [
            CS_WEEKLY_GET_INFO("getInfo", 5312, 5282), CS_WEEKLY_REC_TASK_REWARD("recTaskReward", 5313, 5283),
            CS_WEEKLY_REC_SCORE_REWARD("recScoreReward", 5314, 5284), CS_WEEKLY_REC_KEY_REWARD("oneKeyRec", 5315, 5285),
            CS_WEEKLY_GET_TASK_NUM("getTodayTaskFinishNum", 5316, 5286), CS_WEEKLY_PUSH_UPDATE("update", 5317, 5287),
            CS_WEEKLY_USRE_VOUCHER("useVoucher", 5318, 5288)],
        ("activityMonthly", "tmActivityMonthly") => [
            CS_MONTHLY_GET_INFO("getInfo", 5321, 5291), CS_MONTHLY_REC_TASK_REWARD("recTaskReward", 5322, 5292),
            CS_MONTHLY_REC_SCORE_REWARD("recScoreReward", 5323, 5293), CS_MONTHLY_REC_KEY_REWARD("oneKeyRec", 5324, 5294),
            CS_MONTHLY_REC_GET_ACTIVITY("getActivePoint", 5325, 5295), CS_MONTHLY_PUSH_UPDATE("update", 5326, 5296),
            CS_MONTHLY_USRE_VOUCHER("useVoucher", 5327, 5297)],
        ("activityBeauty", "tmActivityBeauty") => [
            CS_BEAUTY_GET_INFO("getInfo", 5330, 5300), CS_BEAUTY_REC_TASK_REWARD("recTaskReward", 5331, 5301),
            CS_BEAUTY_REC_SCORE_REWARD("recScoreReward", 5332, 5302), CS_BEAUTY_REC_KEY_REWARD("oneKeyRec", 5333, 5303),
            CS_BEAUTY_REC_GET_ACTIVITY("getActivePoint", 5334, 5304), SC_BEAUTY_PUSH_UPDATE("update", 5335, 5305),
            CS_BEAUTY_USRE_VOUCHER("useVoucher", 5336, 5306)],
        ("unify", "tmUnify") => [
            CS_UNIFY_WORLD_GET_BASE_INFO("getBaseInfo", 5339, 5309), CS_UNIFY_WORLD_GET_INFO("getInfo", 5340, 5310),
            CS_UNIFY_WORLD_GET_TOP_INFO("getTopInfo", 5341, 5311), CS_UNIFY_WORLD_ATTACK("attack", 5342, 5312),
            CS_UNIFY_WORLD_GET_USER_RECORD("getUserRecord", 5343, 5313),
            CS_UNIFY_WORLD_GET_COUNTRY_RECORD("getCountryRecord", 5344, 5314), CS_UNIFY_WORLD_BUY_FOOD("buyFood", 5345, 5315),
            CS_UNIFY_WORLD_BUY_DECREE("buyDecree", 5346, 5316), CS_UNIFY_WORLD_ADD_POWER("addPower", 5347, 5317),
            CS_UNIFY_WORLD_GET_RANK("getRank", 5348, 5318), CS_UNIFY_WORLD_GET_COUNTRY_RANK("getCountryInfo", 5349, 5319),
            CS_UNIFY_WORLD_UPDATE_FIGHT_INFO("updateFightInfo", 5350, 5320), CS_UNIFY_WORLD_SOLO_APPLY("soloApply", 5351, 5321),
            CS_UNIFY_WORLD_GET_SOLO_APPLY_LIST("getSoloApplyList", 5352, 5322),
            CS_UNIFY_WORLD_GET_SOLO_INFO("getSoloInfo", 5353, 5323),
            CS_UNIFY_WORLD_GET_COUNTRY_HOME_USER("getCountryHomeUser", 5354, 5324),
            CS_UNIFY_WORLD_ENTER_UNIFY("enterUnify", 5355, 5325), CS_UNIFY_WORLD_LEAVE_UNIFY("leaveUnify", 5356, 5326),
            CS_UNIFY_WORLD_USE_VOUCHER("useVoucher", 5357, 5327), CS_UNIFY_WORLD_LIKE("like", 5358, 5328),
            CS_UNIFY_WORLD_GET_BUFF_RECORD("getBuffRecord", 5359, 5329), CS_UNIFY_WORLD_BUY_BUFF("buyBuff", 5360, 5330),
            CS_UNIFY_WORLD_GETWORLDLASTMSGS("getWorldLastMsgs", 5365, 5335),
            CS_UNIFY_WORLD_SENDWORLDMESSAGE("sendWorldMessage", 5366, 5336),
            CS_UNIFY_WORLD_SENDPERSONMESSAGE("sendPersonMessage", 5367, 5337)],
        ("beauty", "tmBeauty") => [
            CS_BEAUTY_GETINFO("getInfo", 5371, 5341), CS_BEAUTY_REEL_UP("upgradeReel", 5372, 5342),
            CS_BEAUTY_SKILL_UP("upgradeSkill", 5373, 5343), CS_BEAUTY_SET_FAVORITE("setBackgroud", 5374, 5344),
            CS_BEAUTY_RESET("resetSkill", 5375, 5345), CS_BEAUTYTOKEN_GET_INFO("getInfo", 5516, 5487),
            CS_BEAUTY_FORGE("forge", 5517, 5488), CS_BEAUTY_UPGRADE("upgrade", 5518, 5489), CS_BEAUTY_STAR("star", 5519, 5490),
            CS_BEAUTY_QUALITY("quality", 5520, 5491)],
        ("activityBingo", "tmActivityBingo") => [
            CS_BINGO_GET_INFO("getInfo", 5379, 5349), CS_BINGO_GET_TASK_INFO("getTaskInfo", 5380, 5350),
            CS_BINGO_LIFHT_GRID("lightGrid", 5381, 5351), CS_BINGO_REC_REWARD("receiveGridReward", 5382, 5352),
            CS_BINGO_REC_TASK_REWARD("receiveTaskReward", 5383, 5353), CS_BINGO_SINGLE_WHEEl("singleWheel", 5384, 5354),
            CS_BINGO_MUTI_WHEEl("multiWheel", 5385, 5355)],
        ("activityDoubleGift", "tmActivityDoubleGift") => [
            CS_DOUBLEGIFT_GET_INFO("getInfo", 5388, 5358), CS_DOUBLEGift_USRE_VOUCHER("useVoucher", 5389, 5359),
            CS_DOUBLEGift_SCORE_BUY("exchange", 5390, 5360), CS_DOUBLEGIFT_PUSH_NORMAl("pushNormalPay", 5391, 5361)],
        ("armHouse", "tmArmHouse") => [
            CS_EQUIPMENTSTORE_LEVELUP("levelUp", 5394, 5364), CS_EQUIPMENTSTORE_GETINFO("getInfo", 5395, 5365),
            CS_EQUIPMENTSTORE_ONEKEYLVUP("onekeyLvUp", 5396, 5366), CS_EQUIPMENTSTORE_RANKUP("rankUp", 5397, 5367),
            CS_EQUIPMENTSTORE_CHECKUSEFUL("checkUseful", 5398, 5368), CS_EQUIPMENTSTORE_FORCELVUP("forceLvUp", 5399, 5369),
            CS_EQUIPMENTSTORE_GETONEKEYLVUPITEM("getOneKeyLvUpItem", 5400, 5370)],
        ("activityDiagramsWheel", "tmActivityDiagramsWheel") => [
            CS_BAGUA_GETINFO("getInfo", 5403, 5373), CS_BAGUA_WHEEL("wheel", 5404, 5374),
            CS_BAGUA_RECEIVE_BOX_REWARD("receiveBoxReward", 5405, 5375)],
        ("activityActiveCelebration", "tmActivityActiveCelebration") => [
            CS_ACTIVE_CELEBRATION_GETINFO("getInfo", 5408, 5378), CS_ACTIVE_CELEBRATION_GET_REWARD("recReward", 5409, 5379),
            CS_ACTIVE_CELEBRATION_USE_VOUCHER("useVoucher", 5410, 5380),
            CS_ACTIVE_CELEBRATION_GET_ALL_REWARD("recAllReward", 5411, 5381), SC_ACTIVE_CELEBRATION_UPDATE("update", 5412, 5382)],
        ("dailyDiscount", "tmDailyDiscount") => [
            CS_DAILY_GET_INFO("getInfo", 5415, 5385), CS_DAILY_REC_KEY_REWARD("receiveAllReward", 5416, 5386),
            CS_DAILY_REC_REWARD("receive", 5417, 5387), CS_DAILY_REC_RED_REWARD("redeem", 5418, 5388),
            CS_DAILY_USRE_VOUCHER("useVoucher", 5419, 5389), CS_DAILY_REC_RECHARGE_REWARD("recRechargeReward", 5420, 5390),
            CS_DAILY_PUSH_NORMAl("normalUpdate", 5423, 5393), CS_DAILY_PUSH_PREBUY("preBuyUpdate", 5424, 5394)],
        ("firstRide", "tmFirstRide") => [
            CS_FIRST_RIDE_GET_BASE_INFO("getBaseInfo", 5428, 5398), CS_FIRST_RIDE_GET_INFO("getInfo", 5429, 5399),
            CS_FIRST_RIDE_CREATE_SQUAD("createSquad", 5430, 5400), CS_FIRST_RIDE_SEARCH_BY_UNAME("searchByUname", 5431, 5401),
            CS_FIRST_RIDE_INVITE("invite", 5432, 5402), CS_FIRST_RIDE_ACCEPT_INVITE("acceptInvite", 5433, 5403),
            CS_FIRST_RIDE_KICK("kick", 5434, 5404), CS_FIRST_RIDE_AUTO_ENROLL("autoEnroll", 5435, 5405),
            CS_FIRST_RIDE_READY("ready", 5436, 5406), CS_FIRST_RIDE_CANCEL_READY("cancelReady", 5437, 5407),
            CS_FIRST_RIDE_SQUAD_ENROLL("squadEnroll", 5438, 5408), CS_FIRST_RIDE_PERSONAL_ENROLL("personalEnroll", 5439, 5409),
            CS_FIRST_RIDE_LEAVE("leave", 5440, 5410), CS_FIRST_RIDE_ENTER_ROOM("enterRoom", 5441, 5411),
            CS_FIRST_RIDE_LEAVE_ROOM("leaveRoom", 5442, 5412), CS_FIRST_RIDE_GET_ROOM_INFO("getRoomInfo", 5443, 5413),
            CS_FIRST_RIDE_SEARCH_FRIEND("searchFriend", 5444, 5414),
            CS_FIRST_RIDE_SEARCH_GUILD_MEMBER("searchGuildMember", 5445, 5415),
            CS_FIRST_RIDE_GET_INVITE_LIST("getInviteList", 5446, 5416),
            CS_FIRST_RIDE_REFRESH_FIGHT_FORCE("refreshFightForce", 5447, 5417),
            CS_FIRST_RIDE_GET_WORSHIP_INFO("getWorshipInfo", 5448, 5418), CS_FIRST_RIDE_LIKE("like", 5449, 5419),
            CS_FIRST_RIDE_GET_ENCOURAGE_COUNTRY("getEncourageCountry", 5450, 5420),
            CS_FIRST_RIDE_GET_ENCOURAGE_PERSON("getEncouragePerson", 5451, 5421),
            CS_FIRST_RIDE_GET_MY_ENCOURAGE("getMyEncourage", 5452, 5422), CS_FIRST_RIDE_CHEER_COUNTRY("cheerCountry", 5453, 5423),
            CS_FIRST_RIDE_CHEER_PERSON("cheerPerson", 5454, 5424), CS_FIRST_RIDE_GET_HOST("getHost", 5455, 5425),
            CS_FIRST_RIDE_LOGIN("login", 5456, 5426), CS_FIRST_RIDE_GET_ROOM_LIST("getRoomList", 5457, 5427),
            CS_FIRST_RIDE_JOIN("join", 5458, 5428), CS_FIRST_RIDE_SUBCD("subCd", 5459, 5429),
            CS_FIRST_RIDE_GET_BUFF_RECORD("getBuffRecord", 5460, 5430), CS_FIRST_RIDE_BUY_BUFF("buyBuff", 5461, 5431),
            CS_FIRST_RIDE_GET_MY_RANK_AFTER_BATTLE("getMyRankAfterBattle", 5462, 5432),
            CS_FIRST_RIDE_REFUSE_ALL_INVITATION("refuseAllInvitation", 5463, 5433),
            CS_FIRST_RIDE_REFUSE_INVITE("refuseInvite", 5464, 5434), CS_FIRST_RIDE_GET_TASK_INFO("getTaskInfo", 5465, 5435),
            CS_FIRST_RIDE_RECEIVE_TASK_REWARD("receiveTaskReward", 5466, 5436),
            CS_FIRST_RIDE_RECEIVE_TASK_REWARD_ALL("receiveTaskRewardAll", 5467, 5437),
            CS_FIRST_RIDE_GETCAMPLASTMSGS("getRoomChat", 5478, 5449), CS_FIRST_RIDE_SENDCAMPMESSAGE("sendRoomChat", 5479, 5450),
            CS_FIRST_RIDE_GETSQUADLASTMSGS("getSquadChat", 5480, 5451), CS_FIRST_RIDE_SENDSQUADMESSAGE("sendSquadChat", 5481, 5452),
            CS_FIRST_RIDE_SENDPERSONMESSAGE("sendPrivateChat", 5482, 5453), CS_FIRST_RIDE_GETRANKLIST("getRankList", 5485, 5456),
            CS_FIRST_RIDE_GETALLRANKLIST("getAllRankList", 5486, 5457), CS_FIRST_RIDE_GETCAMPRANKLIST("getCampRankList", 5487, 5458),
            CS_FIRST_RIDE_GETBATTLERECORDLIST("getBattleRecordList", 5488, 5459),
            CS_FIRST_RIDE_GETBATTLERECORD("getBattleRecord", 5489, 5460), CS_FIRST_RIDE_GETMYRANK("getMyRank", 5490, 5461),
            CS_FIRST_RIDE_UPDATEBATTLEFMT("updateBattleFmt", 5491, 5462)],
        ("activityHeroesTreasure", "tmActivityHeroesTreasure") => [
            CS_HEROES_TREASURE_GETINFO("getInfo", 5526, 5497), CS_HEROES_TREASURE_SETAIMREWARD("setUltimatePrize", 5527, 5498),
            CS_HEROES_TREASURE_OPENBYKEY("openTreasureChestByKey", 5528, 5499),
            CS_HEROES_TREASURE_OPENBYGOLD("openTreasureChestByGold", 5529, 5500),
            CS_HEROES_TREASURE_QUICKOPEN("quickOpenTreasureChest", 5530, 5501),
            CS_HEROES_TREASURE_NEXTFLOOR("nextLayer", 5531, 5502), CS_HEROES_TREASURE_RESETCURFLOOR("resetLayer", 5532, 5503)],
        ("activityHeroesTreasure", "activityHeroesTreasure") => [
            CS_HEROES_TREASURE_RECEIVETASK("receiveTaskRewards", 5533, 5504),
            CS_HEROES_TREASURE_RECEIVERECHARGE("receiveRechargeRewards", 5534, 5505)],
        ("activityAnniversary6Fund", "tmActivityAnniversary6Fund") => [
            CS_SIXYEAR_FUND_GETINFO("getInfo", 5542, 5513), CS_SIXYEAR_FUND_RECREWARD("recReward", 5543, 5514),
            CS_SIXYEAR_FUND_RECALLBUYREWARD("recAllBuyRewrad", 5544, 5515)],
        ("activityAnniversary6Shop", "tmActivityAnniversary6Shop") => [
            CS_SIXYEAR_SHOP_GETINFO("getInfo", 5547, 5518), CS_SIXYEAR_SHOP_BUY("buy", 5548, 5519)],
        ("activityAnniversary6Feedback", "tmActivityAnniversary6Feedback") => [
            CS_SIXYEAR_VIP_GETINFO("getInfo", 5551, 5522), CS_SIXYEAR_VIP_BUY("buy", 5552, 5523)],
        ("activityAnniversary6SignIn", "tmActivityAnniversary6SignIn") => [
            CS_SIXYEAR_SIGN_GETINFO("getInfo", 5555, 5526), CS_SIXYEAR_SIGN_RECEIVE("receiveSignRewards", 5556, 5527)],
        ("activityQuarter", "tmActivityQuarter") => [
            CS_ACT_QUARTER_GET_INFO("getInfo", 5559, 5530), CS_ACT_QUARTER_REC_REWARD("recReward", 5560, 5531),
            CS_ACT_QUARTER_USE_VOUCHER("useVoucher", 5561, 5532), SC_ACT_QUARTER_UPDATE("update", 5562, 5533)],
        ("activityRareTreasure", "tmActivityRareTreasure") => [
            CS_ACT_RARE_TREASURE_GET_INFO("getInfo", 5565, 5536), CS_ACT_RARE_TREASURE_GET_RECORD("getRecord", 5566, 5537),
            CS_ACT_RARE_TREASURE_GENERATEPOOL("generatePool", 5567, 5538),
            CS_ACT_RARE_TREASURE_RECEIVE_REWARD("receiveReward", 5568, 5539), CS_ACT_RARE_TREASURE_RESET("reset", 5569, 5540),
            CS_ACT_RARE_TREASURE_EXCHANGE_BY_RECHARGE("exchangeByRecharge", 5570, 5541),
            CS_ACT_RARE_TREASURE_EXCHANGE_BY_DAILY_SCORE("exchangeByDailyScore", 5571, 5542)],
        ("activityCsrank", "tmActivityCsrank") => [
            CS_KINGCONTEND_GET_INFO("getInfo", 5576, 5547), CS_KINGCONTEND_GET_RANKLIST("getRankList", 5577, 5548),
            CS_KINGCONTEND_GET_POSLIST("getPosList", 5578, 5549), CS_KINGCONTEND_GET_ACHIVEMENT("recAchievement", 5579, 5550),
            CS_KINGCONTEND_GET_GIFTREWARD("recGiftReward", 5580, 5551), CS_KINGCONTEND_PUSH_TASK("pushTask", 5581, 5552),
            CS_KINGCONTEND_CHOOSE_REWARDGIFT("chooseRewardGift", 5582, 5553)],
        ("smallgame", "tmSmallgame") => [
            CS_SMALLGAME_GETINFO("getInfo", 5590, 5561), CS_SMALLGAME_ENTERPASS("enterPass", 5591, 5562),
            CS_SMALLGAME_FINISHPASS("finishPass", 5592, 5563), CS_SMALLGAME_LEAVEPASS("leavePass", 5593, 5564),
            CS_SMALLGAME_SMALLGAMEEVENT("smallGameEvent", 5594, 5565)],
        ("crossGuild", "tmCrossGuild") => [
            CS_LEGION_CROSS_SERVER_CREATE("createGuild", 5602, 5901), CS_LEGION_CROSS_SERVER_APPLY("applyGuild", 5606, 5905),
            CS_LEGION_CROSS_SERVER_CANCELAPPLY("cancelApply", 5610, 5911),
            CS_LEGION_CROSS_SERVER_AGREEAPPLY("agreeApply", 5614, 5915),
            CS_LEGION_CROSS_SERVER_REFUSEAPPLY("refuseApply", 5618, 5919),
            CS_LEGION_CROSS_SERVER_REFUSEALLAPPLY("refuseAllApply", 5622, 5923),
            CS_LEGION_CROSS_SERVER_QUIT("quitGuild", 5626, 5927), CS_LEGION_CROSS_SERVER_KICKMEMBER("kickMember", 5630, 5931),
            CS_LEGION_CROSS_SERVER_MODIFYICON("modifyIcon", 5634, 5935),
            CS_LEGION_CROSS_SERVER_MODIFYSLOGAN("modifySlogan", 5638, 5939),
            CS_LEGION_CROSS_SERVER_MODIFYPOST("modifyPost", 5642, 5943), CS_LEGION_CROSS_SERVER_DISMISS("dismiss", 5654, 5955),
            CS_LEGION_CROSS_SERVER_TRANSPRESIDENT("transPresident", 5658, 5959),
            CS_LEGION_CROSS_SERVER_SET_MEMBERTYPE("setMemberType", 5662, 5963),
            CS_LEGION_CROSS_SERVER_IMPEACH("impeach", 5667, 5968), CS_LEGION_CROSS_SERVER_CONTRIBUTE("contribute", 5671, 5972),
            CS_LEGION_CROSS_SERVER_UPGRADEGUILD("upgradeBuild", 5675, 5976),
            CS_LEGION_CROSS_SERVER_PROGRESS_REWARD("getReward", 5679, 5980), CS_LEGION_CROSS_SERVER_LOTTERY("lottery", 5683, 5984),
            CS_LEGION_CROSS_SERVER_FIGHT("fightEachOther", 5687, 5988),
            CS_LEGION_CROSS_SERVER_GETAPPLYLIST("getGuildApplyList", 5691, 5992),
            CS_LEGION_CROSS_SERVER_GETGUILDLIST("getGuildList", 5695, 5996),
            CS_LEGION_CROSS_SERVER_SEARCHGUILD("getGuildListByName", 5699, 6000),
            CS_LEGION_CROSS_SERVER_GETMEMBERLIST("getMemberList", 5703, 6004),
            CS_LEGION_CROSS_SERVER_MEMBERINFO("getMemberInfo", 5707, 6008),
            CS_LEGION_CROSS_SERVER_GETDYNAMICLIST("getDynamicList", 5711, 6012),
            CS_LEGION_CROSS_SERVER_GETGUILDINFO("getGuildInfo", 5715, 6016),
            CS_LEGION_CROSS_SERVER_MODIFY_NAME("modifyName", 5719, 6020),
            CS_LEGION_CROSS_SERVER_MODIFY_PASSWORD("modifyPassWd", 5723, 6024),
            CS_LEGION_CROSS_SERVER_SET_QQ("modifyQQ", 5727, 6028), CS_LEGION_CROSS_SERVER_SET_ICON("modifyIcon", 5731, 6032),
            CS_LEGION_CROSS_SERVER_SIGN("sign", 5733, 6034)],
        ("re.crossGuild", "re.tmCrossGuild") => [
            SC_LEGION_CROSS_SERVER_SIGN("sign", 5735, 6036), CS_LEGION_CROSS_SERVER_REFUSH_GUILD("refresh_member", 5739, 6040),
            CS_LEGION_CROSS_SERVER_REFUSH_PROGRESS("contribute", 5743, 6044),
            CS_LEGION_CROSS_SERVER_REFUSH_GUILD_IMPEACH("impeach", 5751, 6052)],
        ("crossGuildWar", "tmCrossGuildWar") => [
            CS_CC_USER_ENTER("userEnter", 5834, 5575), CS_CC_GET_BATTLE_LOG("GetGuildBattleLog", 5835, 5576),
            CS_CC_GET_MARCH_TIME("preMarch", 5836, 5577), CS_CC_GET_ONHOOK_INFO("getProceedInfo", 5838, 5579),
            CS_CC_RECEIVE_PORCEED("receivePorceed", 5839, 5580), CS_CC_USER_LEAVE("userLeave", 5842, 5583),
            CS_CC_USER_REGISTER("justPlay", 5843, 5584), CS_CC_MARCH("march", 5844, 5585),
            CS_CC_MARCH_SPEEDUP("marchSpeedUp", 5845, 5586), CS_CC_REBORN("reborn", 5846, 5587),
            CS_CC_CHALLENGE("challenge", 5847, 5588), CS_CC_REVIVE("revive", 5848, 5589),
            CS_CROSSGUILDWAR_GET_BASEINFO("getBaseInfo", 5850, 5591), CS_CROSSGUILDWAR_REGISTER("register", 5851, 5592),
            CS_CROSSGUILDWAR_GUILDRANK("getGuildRankList", 5852, 5593),
            CS_CROSSGUILDWAR_JOINMEMBERLIST("getJointMemberList", 5853, 5594),
            CS_CROSSGUILDWAR_GRTPROMOTED("getPromoted", 5854, 5595), CS_CROSSGUILDWAR_USEVOUCHER("useVoucher", 5856, 5597),
            CS_CROSSGUILDWAR_GETTASKINFO("getTaskInfo", 5861, 5602), CS_CROSSGUILDWAR_RECVTASKREWARD("recvTaskReward", 5862, 5603),
            CS_CROSSGUILDWAR_RECVGUILDGIFT("receiveGuildGift", 5864, 5605),
            CS_CROSSGUILDWAR_TRYBUYGUILDGIFT("tryBuyGuildGift", 5865, 5606), CS_CHAT_SUPER_MAP_SEND("sendChatMsg", 6043, 6189),
            CS_CHAT_SUPER_MAP_GET("getLastChatMsg", 6044, 6190)],
        ("re.crossGuildWar", "re.tmCrossGuildWar") => [
            SC_CROSSGUILDWAR_BUYUPDATA("buyUpdate", 5857, 5598), SC_CROSSGUILDWAR_BUYUGUILDGIFT("buyGuildGift", 5858, 5599),
            SC_CROSSGUILDWAR_TASKNOTIFY("taskNotify", 5860, 5601)],
        ("crossGuildWarDev", "tmCrossGuildWarDev") => [
            CS_CC_USER_ENTER_DEV("userEnter", 5900, 5638), CS_CC_GET_BATTLE_LOG_DEV("GetGuildBattleLog", 5901, 5639),
            CS_CC_GET_MARCH_TIME_DEV("preMarch", 5902, 5640), CS_CC_GET_ONHOOK_INFO_DEV("getProceedInfo", 5904, 5642),
            CS_CC_RECEIVE_PORCEED_DEV("receivePorceed", 5905, 5643), CS_CC_USER_LEAVE_DEV("userLeave", 5908, 5646),
            CS_CC_USER_REGISTER_DEV("justPlay", 5909, 5647), CS_CC_MARCH_DEV("march", 5910, 5648),
            CS_CC_MARCH_SPEEDUP_DEV("marchSpeedUp", 5911, 5649), CS_CC_REBORN_DEV("reborn", 5912, 5650),
            CS_CC_CHALLENGE_DEV("challenge", 5913, 5651), CS_CC_REVIVE_DEV("revive", 5914, 5652),
            CS_CROSSGUILDWAR_DEV_REGISTER("register", 5917, 5655), CS_CROSSGUILDWAR_DEV_GUILDRANK("getGuildRankList", 5918, 5656),
            CS_CROSSGUILDWAR_DEV_JOINMEMBERLIST("getJointMemberList", 5919, 5657),
            CS_CROSSGUILDWAR_DEV_GRTPROMOTED("getPromoted", 5920, 5658), CS_CROSSGUILDWAR_DEV_USEVOUCHER("useVoucher", 5922, 5660),
            CS_CROSSGUILDWAR_DEV_GETTASKINFO("getTaskInfo", 5925, 5663),
            CS_CROSSGUILDWAR_DEV_RECVTASKREWARD("recvTaskReward", 5926, 5664)],
        ("re.crossGuildWarDev", "re.tmCrossGuildWarDev") => [
            SC_CROSSGUILDWAR_DEV_BUYUPDATA("buyUpdate", 5923, 5661), SC_CROSSGUILDWAR_DEV_TASKNOTIFY("taskNotify", 5924, 5662)],
        ("crossCrazyWar", "tmCrossCrazyWar") => [
            CS_CC_USER_ENTER_SCUFFLE("userEnter", 5936, 5674), CS_CC_GET_BATTLE_LOG_SCUFFLE("GetGuildBattleLog", 5937, 5675),
            CS_CC_GET_MARCH_TIME_SCUFFLE("preMarch", 5938, 5676), CS_CC_GET_ONHOOK_INFO_SCUFFLE("getProceedInfo", 5940, 5678),
            CS_CC_RECEIVE_PORCEED_SCUFFLE("receivePorceed", 5941, 5679), CS_CC_USER_LEAVE_SCUFFLE("userLeave", 5944, 5682),
            CS_CC_USER_REGISTER_SCUFFLE("justPlay", 5945, 5683), CS_CC_MARCH_SCUFFLE("march", 5946, 5684),
            CS_CC_MARCH_SPEEDUP_SCUFFLE("marchSpeedUp", 5947, 5685), CS_CC_REBORN_SCUFFLE("reborn", 5948, 5686),
            CS_CC_CHALLENGE_SCUFFLE("challenge", 5949, 5687), CS_CC_REVIVE_SCUFFLE("revive", 5950, 5688),
            CS_CROSSGUILDWAR_SCUFFLE_REGISTER("register", 5953, 5691),
            CS_CROSSGUILDWAR_SCUFFLE_GUILDRANK("getGuildRankList", 5954, 5692),
            CS_CROSSGUILDWAR_SCUFFLE_JOINMEMBERLIST("getJointMemberList", 5955, 5693),
            CS_CROSSGUILDWAR_SCUFFLE_GRTPROMOTED("getPromoted", 5956, 5694),
            CS_CROSSGUILDWAR_SCUFFLE_GETTASKINFO("getTaskInfo", 5961, 5699),
            CS_CROSSGUILDWAR_SCUFFLE_RECVTASKREWARD("recvTaskReward", 5962, 5700)],
        ("crazyWar", "tmCrazyWar") => [
            SC_CC_USER_ALLDATA_SCUFFLE("PushUserMapAllData", 5965, 5703),
            SC_CC_USER_ENTER_MAP_SCUFFLE("PushUserEnterMap", 5966, 5704),
            SC_CC_USER_CHANGE_POS_SCUFFLE("PushUserChangePos", 5967, 5705),
            SC_CC_USER_STATE_CHANGE_SCUFFLE("PushUserStateChange", 5968, 5706),
            SC_CC_USER_REVIVE_SCUFFLE("PushUserRevive", 5969, 5707), SC_CC_USER_MARCH_SCUFFLE("PushUserMarch", 5970, 5708),
            SC_CC_USER_ENERGY_CHANGE_SCUFFLE("PushUserEnergyChange", 5971, 5709),
            SC_CC_USER_POINTS_CHANGE_SCUFFLE("PushUserPointsChange", 5972, 5710),
            SC_CC_USER_POWER_CHANGE_SCUFFLE("PushUserPowerChange", 5973, 5711),
            SC_CC_BUILD_LIST_CHANGE_SCUFFLE("PushBuildListChange", 5975, 5713),
            SC_CC_DOBATTLE_BEGIN_SCUFFLE("PushDoBattleBegin", 5977, 5715), SC_CC_DOBATTLE_END_SCUFFLE("PushDoBattleEnd", 5978, 5716),
            SC_CC_RANDOM_BUILD_OPEN_SCUFFLE("PushRandomBuildOpen", 5987, 5725),
            SC_CC_RANDOM_BUILD_CLOSE_SCUFFLE("PushRandomBuildClose", 5988, 5726),
            SC_CC_RANDOM_BUILD_CLOSE_UISER_RETURN_SCUFFLE("PushRandomBuildCloseUserReturn", 5989, 5727),
            SC_CC_BATTLE_RESULT_SCUFFLE("PushCrazyWarBattleResult", 5995, 5733)],
        ("soulStone", "tmSoulStone") => [
            CS_SOULSTONE_ADD("add", 6004, 6132), CS_SOULSTONE_REMOVE("remove", 6005, 6133),
            CS_SOULSTONE_TRANSFER("transfer", 6007, 6135)],
        ("crossGuildTech", "tmCrossGuildTech") => [
            CS_SOULTECH_GETINFO("getInfo", 6018, 6149), CS_SOULTECH_LEVELUP("levelUp", 6019, 6150)],
        ("activitySoulStoneRecruit", "tmActivitySoulStoneRecruit") => [
            CS_ACTIVITY_HUNSHI_RAFFLE_GETINFO("getInfo", 6022, 6153), CS_ACTIVITY_HUNSHI_RAFFLE_RAFFLE("raffle", 6024, 6155),
            CS_ACTIVITY_HUNSHI_RAFFLE_OPENBOX("openBox", 6026, 6157), CS_ACTIVITY_HUNSHI_RAFFLE_TASK("getTaskReward", 6028, 6159),
            CS_ACTIVITY_HUNSHI_RAFFLE_VOUCHER("useVoucher", 6030, 6161),
            SC_ACTIVITY_HUNSHI_RAFFLE_PAY_PUSH("pushNormalPay", 6032, 6163)],
        ("activityDailyRecharge2", "tmActivityDailyRecharge2") => [
            CS_DAILYGIFTBAG_GETINFO("getInfo", 6034, 6183), CS_DAILYGIFTBAG_USRE_VOUCHER("useVoucher", 6035, 6184),
            SC_DAILYGIFTBAG_PUSH_UPDATE("update", 6036, 6185)],
        ("re.crossGuild", "tmRe.crossGuild") => [
            SC_CROSSGUILD_BOXPROGRESS("boxProgress", 6038, 6165), SC_CROSSGUILD_CGWSCOREENOUGH("CGWScoreEnough", 6040, 6167)],
        ("activityDailyFundDraw", "tmActivityDailyFundDraw") => [
            CS_DAILYFUNDRAW_GETSIGNINFO("getSignInfo", 6047, 6170), CS_DAILYFUNDRAW_sign("sign", 6048, 6171),
            CS_DAILYFUNDRAW_RECEIVEFUND("receiveFund", 6049, 6172), CS_DAILYFUNDRAW_GETINFO("getInfo", 6050, 6173),
            CS_DAILYFUNDRAW_GETPOOL_INFO("getPoolInfo", 6051, 6174), CS_DAILYFUNDRAW_BUYTICKET("buyLotteryTicket", 6052, 6175),
            CS_DAILYFUNDRAW_CHOOSEITEM("chooseGridItem", 6053, 6176), CS_DAILYFUNDRAW_LOTTERY("lottery", 6054, 6177),
            CS_DAILYFUNDRAW_CHARGEINFO("getChargeInfo", 6055, 6178), CS_DAILYFUNDRAW_CHARGEUSE_VOUCHER("useVoucher", 6056, 6179),
            SC_DAILYFUNDRAW_PUSH_UPDATE("pushNormalPay", 6057, 6180)],
        ("activityActiveTaskShop", "tmActivityActiveTaskShop") => [
            CS_ACTIVETASKSHOP_GETTASKINFO("getTaskInfo", 6063, 6196), CS_ACTIVETASKSHOP_GETSHOPINFO("getShopInfo", 6064, 6197),
            CS_ACTIVETASKSHOP_RECEIVETASK("receiveTaskReward", 6065, 6198), CS_ACTIVETASKSHOP_BUY_EXC("exchange", 6066, 6199),
            CS_ACTIVETASKSHOP_BUY_ACT("buy", 6067, 6200), CS_ACTIVETASKSHOP_NEXTROUND("nextRound", 6068, 6201),
            CS_ACTIVETASKSHOP_USEVOUCHER("useVoucher", 6069, 6202), SC_ACTIVETASKSHOP_PUSH_PAY("pushNormalPay", 6070, 6203)],
        ("activityRechargeGiftDraw", "tmActivityRechargeGiftDraw") => [
            CS_ACTIVITYRECHARGEGIFTDRAW_GETINFO("getInfo", 6073, 6206),
            CS_ACTIVITYRECHARGEGIFTDRAW_REC_DAILY("receiveDailyBox", 6074, 6207),
            CS_ACTIVITYRECHARGEGIFTDRAW_REC_RECHARGE("receiveGiftReward", 6075, 6208),
            CS_ACTIVITYRECHARGEGIFTDRAW_REC_FINAL("receiveFinalReward", 6076, 6209)],
        ("activityHunShiXiLianAct", "tmActivityHunShiXiLianAct") => [
            CS_ACTIVITYHUNSHIXILIAN_ACT_GETINFO("getInfo", 6079, 6212),
            CS_ACTIVITYHUNSHIXILIAN_ACT_REC_SCOREBOX("recvScoreBox", 6080, 6213),
            CS_ACTIVITYHUNSHIXILIAN_ACT_REC_TASKREWARD("recvTaskReward", 6081, 6214),
            CS_ACTIVITYHUNSHIXILIAN_ACT_GET_RANKLIST("getRankList", 6082, 6215),
            CS_ACTIVITYHUNSHIXILIAN_ACT_USEVOUCHER("useVoucher", 6083, 6216),
            SC_ACTIVIT_YHUNSHIXILIAN_ACT_PUSH_PAY("pushNormalPay", 6084, 6217)],
        ("crossGuildCopy", "tmCrossGuildCopy") => [
            CS_CC_USER_ENTER_COPY("userEnter", 6089, 5821), CS_CC_GET_BATTLE_LOG_COPY("GetGuildBattleLog", 6090, 5822),
            CS_CC_GET_MARCH_TIME_COPY("preMarch", 6091, 5823), CS_CC_GET_ONHOOK_INFO_COPY("getProceedInfo", 6093, 5825),
            CS_CC_RECEIVE_PORCEED_COPY("receivePorceed", 6094, 5826), CS_CC_USER_LEAVE_COPY("userLeave", 6097, 5829),
            CS_CC_USER_REGISTER_COPY("justPlay", 6098, 5830), CS_CC_MARCH_COPY("march", 6099, 5831),
            CS_CC_MARCH_SPEEDUP_COPY("marchSpeedUp", 6100, 5832), CS_CC_REBORN_COPY("reborn", 6101, 5833),
            CS_CC_CHALLENGE_COPY("challenge", 6102, 5834), CS_CC_REVIVE_COPY("revive", 6103, 5835),
            CS_CROSSGUILDWAR_COPY_REGISTER("register", 6106, 5838), CS_CROSSGUILDWAR_COPY_GUILDRANK("getGuildRankList", 6107, 5839),
            CS_CROSSGUILDWAR_COPY_JOINMEMBERLIST("getJointMemberList", 6108, 5840),
            CS_CROSSGUILDWAR_COPY_GRTPROMOTED("getPromoted", 6109, 5841), CS_CROSSGUILDWAR_COPY_USEVOUCHER("useVoucher", 6111, 5843),
            CS_CROSSGUILDWAR_COPY_GETTASKINFO("getTaskInfo", 6114, 5846),
            CS_CROSSGUILDWAR_COPY_RECVTASKREWARD("recvTaskReward", 6115, 5847), CS_CROSSGUILDWAR_COPY_GETINFO("getInfo", 6116, 5848),
            CS_CROSSGUILDWAR_COPY_GETMONSTERFIGHT("getMonsterFightForce", 6117, 5849),
            CS_CROSSGUILDWAR_COPY_RECEIVEREWARD("receiveReward", 6118, 5850),
            CS_CROSSGUILDWAR_COPY_RECEIVEALL_REWARD("receiveAllReward", 6119, 5851),
            SC_CC_USER_ALLDATA_COPY("PushUserMapAllData", 6122, 5854), SC_CC_USER_ENTER_MAP_COPY("PushUserEnterMap", 6123, 5855),
            SC_CC_USER_CHANGE_POS_COPY("PushUserChangePos", 6124, 5856),
            SC_CC_USER_STATE_CHANGE_COPY("PushUserStateChange", 6125, 5857), SC_CC_USER_REVIVE_COPY("PushUserRevive", 6126, 5858),
            SC_CC_USER_MARCH_COPY("PushUserMarch", 6127, 5859), SC_CC_USER_ENERGY_CHANGE_COPY("PushUserEnergyChange", 6128, 5860),
            SC_CC_USER_POINTS_CHANGE_COPY("PushUserPointsChange", 6129, 5861),
            SC_CC_USER_POWER_CHANGE_COPY("PushUserPowerChange", 6130, 5862),
            SC_CC_BUILD_LIST_CHANGE_COPY("PushBuildListChange", 6132, 5864),
            SC_CC_DOBATTLE_BEGIN_COPY("PushDoBattleBegin", 6134, 5866), SC_CC_DOBATTLE_END_COPY("PushDoBattleEnd", 6135, 5867),
            SC_CC_BATTLE_RESULT_COPY("PushGuildWarBattleResult", 6136, 5868)],
        ("re.crossGuildCopy", "tmRe.crossGuildCopy") => [
            SC_CROSSGUILDWAR_COPY_BUYUPDATA("buyUpdate", 6112, 5844), SC_CROSSGUILDWAR_COPY_TASKNOTIFY("taskNotify", 6113, 5845)],
        ("generalBook", "tmGeneralBook") => [
            CS_GENERAL_BOOK_GETINFO("getInfo", 6142, 6220), CS_GENERAL_BOOK_LEVEL_BY_ITEM("levelUpUseItem", 6143, 6221),
            CS_GENERAL_BOOK_LEVEL_BY_STAR("levelUpByStar", 6144, 6222)],
        ("activityClanBattle", "tmActivityClanBattle") => [
            SC_CLEANBATTLE_BUYUPDATA("buy", 6147, 6225), SC_CLEANBATTLE_UPDATASCORE("updateScore", 6148, 6226)],
        ("CGClanBattle", "tmCGClanBattle") => [
            CS_CLEANBATTLE_GETBASEINFO("getBaseInfo", 6149, 6227), CS_CLEANBATTLE_GETINFO("getInfo", 6150, 6228),
            CS_CLEANBATTLE_RECEIVEREWARD("receiveScoreReward", 6151, 6229), CS_CLEANBATTLE_GETRANK("getRank", 6152, 6230),
            CS_CLEANBATTLE_USEVOUCHER("useVoucher", 6153, 6231), CS_CLEANBATTLE_GETMEMBERINFO("getOpponentMemberInfo", 6154, 6232),
            CS_CLEANBATTLE_GETALLRANK("getScoreRankAll", 6155, 6233), CS_CLEANBATTLE_GETTOPMVP("getTopMvp", 6156, 6234),
            CS_CLEANBATTLE_GETGUILDINFO("getGuildInfo", 6157, 6235)],
        ("crossServerRobGrain", "tmCrossServerRobGrain") => [
            CS_LEGIONCROSS_FREIGHT_GETINFO("getInfo", 6160, 6238),
            CS_LEGIONCROSS_FREIGHT_CLEAR_CART_INFO("clearCartInfo", 6161, 6239),
            CS_LEGIONCROSS_FREIGHT_GET_FORMATION("getBattleFormation", 6162, 6240),
            CS_LEGIONCROSS_FREIGHT_GETINVITELIST("getInviteList", 6164, 6242),
            CS_LEGIONCROSS_FREIGHT_INVITE_LEGION("groupCartInvite", 6165, 6243),
            CS_LEGIONCROSS_FREIGHT_INVITE_SPECIAL("fastCartInvite", 6166, 6244),
            CS_LEGIONCROSS_FREIGHT_INVITE_ROB("robInvite", 6167, 6245),
            CS_LEGIONCROSS_FREIGHT_REMOVE_LEGION("groupCartUnInvite", 6168, 6246),
            CS_LEGIONCROSS_FREIGHT_REMOVE_SPECIAL("fastCartUnInvite", 6169, 6247),
            CS_LEGIONCROSS_FREIGHT_REMOVE_ROB("robUnInvite", 6170, 6248),
            CS_LEGIONCROSS_FREIGHT_REFRESH_ALL_CART("refreshMapInfo", 6172, 6250),
            CS_LEGIONCROSS_FREIGHT_GET_ENEMY_CART("getCartListByCGID", 6173, 6251),
            CS_LEGIONCROSS_FREIGHT_CHECK_CART_AVAILABLE("checkDepart", 6174, 6252),
            CS_LEGIONCROSS_FREIGHT_REFRESH_CART_QUALITY("refreshCartQuality", 6175, 6253),
            CS_LEGIONCROSS_FREIGHT_START_FREIGHT_PERSONAL("departSelfCart", 6176, 6254),
            CS_LEGIONCROSS_FREIGHT_START_FREIGHT_MULTI("departMultipleCart", 6177, 6255),
            CS_LEGIONCROSS_FREIGHT_GET_CART_DATA("getCartInfo", 6178, 6256),
            SC_LEGIONCROSS_FREIGHT_SUCCESS_PERSONAL("selfCartArriveToFront", 6179, 6257),
            SC_LEGIONCROSS_FREIGHT_SUCCESS_LEGION("groupCartArriveToFront", 6180, 6258),
            SC_LEGIONCROSS_FREIGHT_SUCCESS_SPECIAL("fastCartArriveToFront", 6181, 6259),
            CS_LEGIONCROSS_FREIGHT_CALLTREASURECART("callTreasureCart", 6182, 6260),
            CS_LEGIONCROSS_FREIGHT_REFRESHTREASURECARTREWARD("refreshTreasureCartReward", 6183, 6261),
            CS_LEGIONCROSS_FREIGHT_CHECKTREASURECART("checkTreasureCart", 6184, 6262),
            CS_LEGIONCROSS_FREIGHT_GETTREASURECARTINVITELIST("getTreasureCartInviteList", 6185, 6263),
            CS_LEGIONCROSS_FREIGHT_TREASURECARTINVITE("treasureCartInvite", 6186, 6264),
            CS_LEGIONCROSS_FREIGHT_TREASURECARTUNINVITE("treasureCartUnInvite", 6187, 6265),
            CS_LEGIONCROSS_FREIGHT_CLEARTREASURECARTINFO("clearTreasureCartInfo", 6188, 6266),
            CS_LEGIONCROSS_FREIGHT_DEPARTTREASURECART("departTreasureCart", 6189, 6267),
            SC_LEGIONCROSS_FREIGHT_REASURECARTARRIVETOFRONT("treasureCartArriveToFront", 6190, 6268),
            CS_LEGIONCROSS_FREIGHT_ROBTREASURECART("robTreasureCart", 6191, 6269),
            SC_LEGIONCROSS_FREIGHT_AUTODEPARTTREASURECART("autoDepartTreasureCart", 6192, 6270),
            CS_LEGIONCROSS_FREIGHT_ROB_PERSONAL("robSelfCart", 6194, 6272),
            CS_LEGIONCROSS_FREIGHT_ROB_MULTI_CHECK("robMultipleCartCheck", 6195, 6273),
            CS_LEGIONCROSS_FREIGHT_ROB_MULTI("robMultipleCart", 6196, 6274),
            SC_LEGIONCROSS_FREIGHT_ROB_HELP_SUCCESS("refreshHelpLootNum", 6197, 6275),
            CS_LEGIONCROSS_FREIGHT_USEVOUCHER("useVoucher", 6199, 6277),
            CS_LEGIONCROSS_FREIGHT_USEVOUCHERBYWEEKLYCARD("useVoucherByWeeklyCard", 6200, 6278),
            CS_LEGIONCROSS_FREIGHT_RECVWEEKLYCARDDAILYREWARD("recvWeeklyCardDailyReward", 6201, 6279),
            SC_LEGIONCROSS_FREIGHT_PUSH_PAY("pushNormalPay", 6202, 6280),
            SC_LEGIONCROSS_FREIGHT_NO_PERMISSION("changeCrossGuildJob", 6203, 6281),
            CS_LEGIONCROSS_FREIGHT_GETREPORT_ROB("getRobReport", 6205, 6283),
            CS_LEGIONCROSS_FREIGHT_GETREPORT_MY_CART("getCartReport", 6206, 6284),
            CS_LEGIONCROSS_FREIGHT_GETINFO_REVENGE("robRevengeCheck", 6207, 6285),
            CS_LEGIONCROSS_FREIGHT_REPLAY_SINGLE("getselfBattleDataByRecordId", 6208, 6286),
            CS_LEGIONCROSS_FREIGHT_REPLAY_MULTI("getMultipleBattleDataByRecordId", 6209, 6287)],
        ("CGCBBoss", "tmCGCBBoss") => [
            CS_CLEANBATTLE_BOSS_GETBASEINFO("getBaseInfo", 6212, 6290),
            CS_CLEANBATTLE_BOSS_GETRANKLIST_PERSONAL("getUserRankList", 6213, 6291),
            CS_CLEANBATTLE_BOSS_GETRANKLIST_LEGION("getGuildRankList", 6214, 6292),
            CS_CLEANBATTLE_BOSS_BUY_NUM("buyNum", 6215, 6293), CS_CLEANBATTLE_BOSS_USEVOUCHER("useVoucher", 6216, 6294),
            CS_CLEANBATTLE_BOSS_USEGIFT("useBuyGift", 6218, 6296), CS_CLEANBATTLE_BOSS_CHALLENGE("attackNpc", 6219, 6297),
            CS_CLEANBATTLE_BOSS_PASS("pass", 6220, 6298), CS_CLEANBATTLE_BOSS_GETPVP_LIST("getPvpList", 6221, 6299),
            CS_CLEANBATTLE_BOSS_GETPVP_RECORD("getPvpRecords", 6222, 6300), CS_CLEANBATTLE_BOSS_ATTACK("attackUser", 6223, 6301),
            CS_CLEANBATTLE_BOSS_QUICKATTACK("quickAttack", 6224, 6302)],
        ("activitySecretEdict", "tmActivitySecretEdict") => [
            CS_ACTIVITYSECRETPASS_GETINFO("getInfo", 6227, 6321),
            CS_ACTIVITYSECRETPASS_RECVREWARD_PASS("recvEdictReward", 6228, 6322),
            CS_ACTIVITYSECRETPASS_RECVREWARD_PASS_ALL("oneKeyRecvEdictReward", 6229, 6323),
            CS_ACTIVITYSECRETPASS_SIGN("sign", 6230, 6324), CS_ACTIVITYSECRETPASS_RESIGN("supSign", 6231, 6325),
            CS_ACTIVITYSECRETPASS_SIGN_ADVANCED("continueRecvSignReawrd", 6232, 6326),
            CS_ACTIVITYSECRETPASS_RECVREWARD_TASK("recvTaskReward", 6233, 6327),
            CS_ACTIVITYSECRETPASS_RECVREWARD_RECHARGE("recvGiftReward", 6234, 6328),
            CS_ACTIVITYSECRETPASS_RECVREWARD_RECHARGE_ALL("oneKeyRecvGiftReawrd", 6235, 6329),
            CS_ACTIVITYSECRETPASS_USEVOUCHER("useVoucher", 6236, 6330), SC_ACTIVITYSECRETPASS_PUSH_PAY("pushNormalPay", 6237, 6331),
            SC_ACTIVITYSECRETPASS_REFRESH_TASK("updateTask", 6238, 6332)],
        ("soulJade", "tmSoulJade") => [
            CS_SOULJADE_ADD("add", 6243, 6307), CS_SOULJADE_REMOVE("remove", 6244, 6308),
            CS_SOULJADE_TRANSFER("transfer", 6246, 6310)],
        ("activity8Years", "tmActivity8Years") => [
            CS_ACT8YEARS_GETINFO("getInfo", 6257, 6335), CS_ACT8YEARS_SIGN("sign", 6259, 6337),
            CS_ACT8YEARS_SIGN_RE("supSign", 6260, 6338), CS_ACT8YEARS_SIGN_ADVANCED("continueRecvSignReawrd", 6261, 6339),
            CS_ACT8YEARS_PASS_RECV("recvEdictReward", 6263, 6341), CS_ACT8YEARS_PASS_RECV_ALL("oneKeyRecvEdictReward", 6264, 6342),
            CS_ACT8YEARS_LUCKY_LOTTERY("luckyLottery", 6266, 6344), CS_ACT8YEARS_LUCKY_BUYTICKET("luckyBuyTicket", 6267, 6345),
            CS_ACT8YEARS_LUCKY_RECREWARD("recvLuckyDailyReward", 6268, 6346), CS_ACT8YEARS_EXCHANGE("exchangeYearShop", 6270, 6348),
            CS_ACT8YEARS_PARTY_SUBMIT("commitDinnerScore", 6272, 6350), CS_ACT8YEARS_PARTY_RECV("recvDinnerReward", 6273, 6351),
            CS_ACT8YEARS_PARTY_GET_TOTALSCORE("getDinnerTotalScore", 6274, 6352),
            CS_ACT8YEARS_LUCKGIFT_RECV("recvLuckyReward", 6276, 6354), CS_ACT8YEARS_WHEEL_LOTTERY("wheelLottery", 6278, 6356),
            CS_ACT8YEARS_WHEEL_REWARD("recvWheelReward", 6279, 6357), CS_ACT8YEARS_RECHARGE_USEVOUCHER("useVoucher", 6281, 6359),
            SC_ACT8YEARS_RECHARGE_PUSH_PAY("pushNormalPay", 6282, 6360), CS_ACT8YEARS_RECHARGE_FREE("buyFreeGift", 6283, 6361),
            CS_ACT8YEARS_TASK_RECV("recvTaskReward", 6285, 6363), SC_ACT8YEARS_TASK_REFRESH("updateTask", 6286, 6364)],
        ("activityDream", "tmActivityDream") => [
            CS_ACTIVITYFANTASYTREASURE_GETINFO("getInfo", 6291, 6375),
            CS_ACTIVITYFANTASYTREASURE_RECEIVEREWARD("receiveReward", 6292, 6376),
            CS_ACTIVITYFANTASYTREASURE_USEVOUCHER("useVoucher", 6293, 6377),
            SC_ACTIVITYFANTASYTREASURE_PUSH_PAY("update", 6294, 6378)],
        ("ploy", "tmPloy") => [
            CS_PLOY_GETINFO("getInfo", 6360, 6455), CS_PLOY_COMPOSE("getPloy", 6361, 6456),
            CS_PLOY_LEVEL_UP("upgradePloy", 6362, 6457), CS_PLOY_EQUIP("equipPloy", 6363, 6458),
            CS_PLOY_REMOVE("removePloy", 6364, 6459), CS_PLOY_TRANS("transPloy", 6365, 6460)],
        ("greatSeal", "tmGreatSeal") => [
            CS_GREATSEAL_GETINFO("getInfo", 6451, 6469), CS_GREATSEAL_COMPOSE("compose", 6452, 6470),
            CS_GREATSEAL_LEVEL_UP("levelUp", 6453, 6471), CS_GREATSEAL_UPGRADE("addStar", 6454, 6472),
            CS_GREATSEAL_EQUIP("equip", 6455, 6473), CS_GREATSEAL_UNLOAD("unload", 6456, 6474),
            CS_GREATSEAL_TRANSFER("transfer", 6457, 6475), CS_GREATSEAL_REBORN("reborn", 6458, 6476)],
    ],
    paired_raw: [
        CS_RANKLIST_FULL_FORMATION("top.getFullPlatFormation", 2329, "tmTop.getFormation", 2297),
        CS_CHESS_TAKEOFF_EQUIP("", 5130, "tm", 5100),
        CS_LEGION_CROSS_SERVER_REFUSH_GUILD_BUILD("re.refresh_guild", 5747, "re.tmRefresh_guild", 6048),
        CS_COMPOSE_TREASURE("exchange.composeTreasure", 247, "emExchange.composeTreasure", 246),
        SC_NEWMAIL("re.mail.newMail", 266, "tmRe.mail.newMail", 263), CS_GET_USER_DATA("cs.userdata", 307, "tmCs.userdata", 304),
        SC_ARENA_DATA_REFRESH("re.arena.dataRefresh", 334, "tmRe.arena.dataRefresh", 331),
        SC_ROBTREASURE_FRAGMENT_CHG("re.robtreasure.fragmentChg", 423, "tmRe.robtreasure.fragmentChg", 420),
        SC_HERO_NEW("re.hero.new", 543, "re.tmHero.new", 544), RE_CHAT_MSG_PUSH("re.chat.getMsg", 664, "re.chat.getMsg", 661),
        SC_ENVELOPE_RECEIVE("re.envelope.receive", 677, "tmRe.envelope.receive", 672),
        SC_HERO_TITLE_ACHIEVEMENT_CHG("re.heroTitle.getNewHeroTitle", 709, "re.tmHeroTitle.getNewHeroTitle", 704),
        CS_DAILY_COPY_BUY_NUM("copy_daily.addNum", 778, "timeMachineCopy_daily.addNum", 775),
        SC_FRIENDENERGY_PRESENTENERGYRS("friendenergy.presentEnergyrs", 887, "tmFriendenergy.presentEnergyrs", 884),
        CS_OPENSWITCH_GETSWITCH("switchfunc.getSwitch", 954, "tmSwitchFunc.getSwitch", 951),
        SC_REWARD_NEW("re.reward.new", 1101, "re.tmReward.new", 1098),
        CS_CELEBRITY_PASSTIMES("copy.getFamousPassTimes", 1113, "tmCopy.getFamousPassTimes", 1110),
        CS_PURGATORY_SWEEP_BY_ASSISTANT("purgatory.sweepByAssistant", 1276, "tmPurgatory.sweepByAssistant", 1274),
        CS_ALL_ACTIVITY_GETACTIVITYINFO("allActivity.getActivityInfo", 1286, "tmAllActivity.getActivityInfo", 1292),
        CS_ACTIVITY_GETACTIVITYCONF("activity.getActivityConf", 1316, "tmActivity.getActivityConf", 1322),
        SC_ACTIVITY_NEWCONF("re.activity.newConf", 1321, "tmRe.activity.newConf", 1327),
        CS_ACTIVITY_VIP_WEEKGIFT_BUY("weekGift.buy", 1433, "tmWeekGift.buy", 1439),
        CS_ACTIVITY_SIGNIN_FESTIVAL_GETINFO("activitySignInFestival.getInfo", 1445, "tmActivitySignInFestival.getInfo", 1451),
        SC_ACTIVITY_MORETHAN_ADDNUM("activityTask.addNum", 1669, "tmActivityTask.addNum", 1653),
        SC_ACTIVITY_MONTH_CARD_UPDATE("re.monthCard.updateCardInfo", 1683, "tmRe.monthCard.updateCardInfo", 1667),
        SC_ACTIVITY_RECHARGEBACK("rechargeBack.receiveReward", 1698, "tmRechargeBack.receiveReward", 1682),
        SC_ACTIVITY_HEROGIFT_OTHER_RECEIVE("re.activityHeroGift.otherReceiveGift", 1812, "tmRe.activityHeroGift.otherReceiveGift", 1779),
        CS_LEGION_BATTLE_EXPRANK("GuildWeekExp.getWeekExpRank", 2181, "tmGuildWeekExp.getWeekExpRank", 2148),
        CS_COUNTRYCHOOSE_CHOOSE("countryChoose.choose", 2288, "tmCountryChoose.choose", 2255),
        SC_RECRUITMENT_ADDNUM("sc.recruitment.addnum", 2864, "tmSc.recruitment.addnum", 2834),
        SC_YUANXIAO_RECEIVE_MSG("re.festivalYuanXiao.receiveMsg", 3313, "tmRe.festivalYuanXiao.receiveMsg", 3283),
        SC_NATIONAL_WAR_CHATMESSAGE("re.countrywar.chatMessage", 3785, "tmRe.countrywar.chatMessage", 3755),
        CS_HEADFRAME_CHANGE("avatar.setAvatar", 3843, "tmAvatar.setAvatar", 3813),
        SC_MILLIONQUIZ_MESSAGE("re.activityAnswer.totalNum", 4479, "tmRe.activityAnswer.totalNum", 4449),
        CS_HUNTDEER_REFRESH_CITY("re.huntdeer.refreshCity", 4582, "tmRe.huntdeer.refreshCity", 4552),
        SC_HERO_GOLD_TASK_REFRESH_PROGRESS("re.heroGoldTask.refreshProgress", 4660, "tmRe.heroGoldTask.refreshProgress", 4630),
        CS_PILL_REBORN("heroPill.reborn", 4868, "tmHeroPill.reborn", 4838),
        SC_ACTIVITY_CAKE_ONWEIGHT("re.celebration.addCream", 4890, "tmRe.celebration.addCream", 4860),
        SC_ACTIVITY_JIGSAW_RECIVELOVE("re.activityJigsaw.reciveLove", 5017, "re.activityJigsaw.reciveLove", 4987),
        CS_CONTERT_GETRECORDBYID("contest.getBattleRecordByRecordId", 5062, "contest.getBattleRecordByRecordId", 5032),
        SC_CONTEST_CHANGE_POS_OF32("re.contest.challenge", 5072, "tmRe.contest.challenge", 5042),
        SC_SAVE_GOLD_ON_TASK_COMPLETE("re.activitySaveGoldExchange.completeTask", 5083, "re.tmActivitySaveGoldExchange.completeTask", 5053),
        SC_CHESSBAG_CHANGE("chessBag.update", 5126, "tmChessBag.update", 5096),
        CS_TRIALROAD_OPENFORMATION("trialRoad.getRankEnemyInfo", 5151, "trialRoad.getRankEnemyInfo", 5121),
        CS_CLASSICSWAR_STOP("classicWar.stop", 5182, "classicWar.stop", 5152),
        SC_ACT4YEARMISSION_PROGRESS("re.activity4YearMission.refreshProgress", 5246, "tmRe.activity4YearMission.refreshProgress", 5216),
        SC_UNIFY_WORLD_CHATMESSAGE("re.unify.chatMessage", 5368, "tmRe.unify.chatMessage", 5338),
        SC_FIRST_RIDE_CHATMESSAGE("re.firstRide.chat", 5483, "tmRe.firstRide.chat", 5454),
        SC_HEROES_TREASURE_TASK_FRESH("re.activityHeroesTreasure.completeTask", 5535, "tmRe.activityHeroesTreasure.completeTask", 5506),
        SC_HEROES_TREASURE_RECHARGE_FRESH("ActivityHeroesTreasure.pushNormalPay", 5536, "tmActivityHeroesTreasure.pushNormalPay", 5507),
        CS_CROSSGUILDWAR_DEV_GET_BASEINFO("crossGuildWarDev.getBaseInfo", 5916, "crossGuildWarDev.getBaseInfo", 5654),
        CS_CROSSGUILDWAR_SCUFFLE_GET_BASEINFO("crossCrazyWar.getBaseInfo", 5952, "crossCrazyWar.getBaseInfo", 5690),
        SC_CROSSGUILDWAR_SCUFFLE_TASKNOTIFY("re.crossCrazyWar.taskNotify", 5960, "tmRe.crossCrazyWar.taskNotify", 5698),
        CS_CROSSGUILDWAR_COPY_GET_BASEINFO("crossGuildCopy.getBaseInfo", 6105, "crossGuildCopy.getBaseInfo", 5837),
        SC_CLEANBATTLE_BOSS_PUSH_PAY("CrossGuildClanBattleBoss.buy", 6217, "tmCrossGuildClanBattleBoss.buy", 6295),
        SC_HOLIDAYCARD_RECEIVEREWARD("holidaycard.receiveReward", 6297, "tmHolidaycard.receiveReward", 6381),
        SC_PLOY_UNLOCK("re.ploy.pushUnlockSlot", 6366, "re.tmPloy.pushUnlockSlot", 6461),
    ],
    normal_groups: [
        "refinery" => [CS_DECOMPOSE_REFINERYTOPANIMAL("refineryTopAnimal", 250), CS_DECOMPOSE_REBORNTOPANIMAL("rebornTopAnimal", 251)],
        "tmChat" => [CS_CHAT_TIMEMACHINE_SEND("sendCrossGuild", 656), CS_CHAT_TIMEMACHINE_GET("getCrossGuildLastMsgs", 657)],
        "activityRechargeReward2" => [
            CS_ACTIVITY_RECHARGEREWARD_GETINFO_2("getInfo", 1476), CS_ACTIVITY_RECHARGEREWARD_RECIEVEREWARD_2("recieveReward", 1484)],
        "activityRecharge2" => [
            CS_ACTIVITY_RECHARGE_GETINFO_2("getInfo", 1489), CS_ACTIVITY_RECHARGE_GET_REWARD_2("recieveReward", 1494)],
        "shareInvite" => [
            CS_ACTIVITY_SHAREINVITE_GETINFO("getInfo", 1724), CS_ACTIVITY_SHAREINVITE_SHARE_RECEIVE("recShareReward", 1729),
            CS_ACTIVITY_SHAREINVITE_INVITE_RECEIVE("recInviteReward", 1734)],
        "guildcopy" => [
            CS_LEGION_CROSS_SERVER_COPY_INFO("getGuildCopyInfo", 5756),
            CS_LEGION_CROSS_SERVER_COPY_STRONGINFO("getStrongInfo", 5760),
            CS_LEGION_CROSS_SERVER_COPY_CHOICE_COPY("choiceCopy", 5764),
            CS_LEGION_CROSS_SERVER_COPY_GET_BASE_REWARD("getBaseReward", 5768),
            CS_LEGION_CROSS_SERVER_COPY_GET_BOX_REWARD("getRewardBox", 5772),
            CS_LEGION_CROSS_SERVER_COPY_GET_LST_BOX_REWARD("getRewardBoxOld", 5776),
            CS_LEGION_CROSS_SERVER_COPY_BUY_ATK_NUM("buyAtkNum", 5780), CS_LEGION_CROSS_SERVER_COPY_GET_ATK_RANK("getAtkRank", 5784),
            CS_LEGION_CROSS_SERVER_COPY_ALL_FIGHT("allFight", 5788), CS_LEGION_CROSS_SERVER_COPY_FIGHT("attack", 5792),
            CS_LEGION_CROSS_SERVER_COPY_RESET("resetCopy", 5796), CS_LEGION_CROSS_SERVER_COPY_AUTO_RESET("autoReset", 5800)],
        "re.guildCopy" => [
            CS_LEGION_CROSS_SERVER_COPY_PUSH_OPEN_BOX("rewardBox", 5804),
            CS_LEGION_CROSS_SERVER_COPY_PUSH_LST_BOX("rewardBoxOld", 5808),
            CS_LEGION_CROSS_SERVER_COPY_PUSH_ALL_FIGHT("allFight", 5813), CS_LEGION_CROSS_SERVER_COPY_PUSH_HP("refreshHp", 5817),
            CS_LEGION_CROSS_SERVER_COPY_PUSH_RESET("reset", 5821), CS_LEGION_CROSS_SERVER_COPY_PASS("pass", 5825)],
        "heroSkin" => [
            CS_HEROSKIN_GETALLHEROSKIN("getAllHeroSkin", 6300), CS_HEROSKIN_SETHEROSKIN("setHeroSkin", 6301),
            CS_HEROSKIN_UPGRADE("upgrade", 6302), CS_HEROSKIN_REBORN("reborn", 6303),
            CS_HEROSKIN_REBORN_SHOW("rebornCostShow", 6304), CS_HEROSKIN_REMOVE_ALL("removeAllUsed", 6305),
            CS_HEROSKIN_UPSTAR("upgradeStar", 6306)],
        "forge" => [
            CS_FORGE_TOGILDGOLDAWAKE("toGildGoldAwake", 6312), CS_FORGE_CHOOSEROUTEAWAKE("chooseGildGoldAwakeRoute", 6313),
            CS_FORGE_GIFTAWAKE("addGildGoldAwakeGift", 6314), CS_FORGE_RESETGIFTAWAKE("resetGildGoldAwakeGift", 6315),
            CS_FORGE_TOGILDGOLDPET("toGildGoldPet", 6318), CS_FORGE_CHOOSEROUTEPET("chooseGildGoldPetRoute", 6319),
            CS_FORGE_GIFTPETE("addGildGoldPetGift", 6320), CS_FORGE_RESETGIFTPET("resetGildGoldPetGift", 6321),
            CS_FORGE_WASHCOUNSELLOR("washCounsellor", 6341), CS_FORGE_EQUIPATTRTOCOUNSELLOR("equipAttrToCounsellor", 6342),
            CS_FORGE_UNLOCKCOUNSELLORWASHPOS("unlockCounsellorWashPos", 6343),
            CS_COUNSELLOR_DIRECT_WASH("directionalWashCounsellor", 6345), CS_FORGE_TOGILDGOLDTREASURE("toGildGoldTreasure", 6348),
            CS_FORGE_CHOOSEROUTETREASURE("chooseGildGoldTreasureRoute", 6349),
            CS_FORGE_GIFTTREASURE("addGildGoldTreasureGift", 6350), CS_FORGE_RESETGIFTTREASURE("resetGildGoldTreasureGift", 6351),
            CS_FORGE_TOGILDGOLDARM("toGildGoldArm", 6354), CS_FORGE_CHOOSEROUTEARM("chooseGildGoldArmRoute", 6355),
            CS_FORGE_GIFTARM("addGildGoldArmGift", 6356), CS_FORGE_RESETGIFTARM("resetGildGoldArmGift", 6357),
            CS_TOP_ANIMAL_REINFORCE("reinforceTopAnimal", 6441), CS_TOP_ANIMAL_STAR("upgradeStarTopAnimal", 6442),
            CS_FORGE_TOGILDGOLDBROCHURE("toGildGoldBrochure", 6445),
            CS_FORGE_CHOOSEROUTEROCHURE("chooseGildGoldBrochureRoute", 6446), CS_FORGE_GIFTROCHURE("addGildGoldBrochureGift", 6447),
            CS_FORGE_RESETGIFTROCHURE("resetGildGoldBrochureGift", 6448), CS_FORGE_TOIRONGOLDAWAKE("toIronGoldAwake", 6464),
            CS_FORGE_CHOOSEIRONROUTEAWAKE("chooseIronGoldAwakeRoute", 6465), CS_FORGE_IRONGIFTAWAKE("addIronGoldAwakeGift", 6466),
            CS_FORGE_RESETIRONGIFTAWAKE("resetIronGoldAwakeGift", 6467), CS_FORGE_TOIRONGOLDPET("toIronGoldPet", 6468),
            CS_FORGE_CHOOSEIRONROUTEPET("chooseIronGoldPetRoute", 6469), CS_FORGE_IRONGIFTPETE("addIronGoldPetGift", 6470),
            CS_FORGE_RESETIRONGIFTPET("resetIronGoldPetGift", 6471)],
        "activityCounsellorXiLian" => [
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_GETINFO("getInfo", 6327),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_TASHANDACH("getTaskAndAchList", 6328),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_REC_SCOREBOX("recvScoreBox", 6329),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_REC_TASKREWARD("recvTaskReward", 6330),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_FREEFIFT("buyFreeGift", 6331),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_USEVOUCHER("useVoucher", 6332),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_RECVFUNREWARD("recvFundReward", 6333),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_GET_RANKLIST("getRankList", 6334),
            CS_ACTIVITYCOUNSELLORXILIAN_ACT_REC_CHARGEREWARD("recvChargeReward", 6335),
            SC_ACTIVITYCOUNSELLORXILIAN_ACT_PUSH_PAY("pushNormalPay", 6336),
            SC_ACTIVITYCOUNSELLORXILIAN_ACT_FUN_PUSH_PAY("activityFundPay", 6337),
            SC_ACTIVITYCOUNSELLORXILIAN_ACT_TASK_PUSH_DATA("updateData", 6338)],
        "activity9Years" => [
            CS_ACTIVITY_9YEARS_GETINFO("getInfo", 6369), CS_ACTIVITY_9YEARS_RECVLOGINREWARD("recvLoginReward", 6370),
            CS_ACTIVITY_9YEARS_RECVEDICTREWARD("recvEdictReward", 6371),
            CS_ACTIVITY_9YEARS_RECVUNLIMITBOXREWARD("recvUnLimitBoxReward", 6372),
            CS_ACTIVITY_9YEARS_RRECVTASKREWARD("recvTaskReward", 6373),
            CS_ACTIVITY_9YEARS_RECVTOTALRECHARGEREWARD("recvTotalRechargeReward", 6375),
            CS_ACTIVITY_9YEARS_RRECVBOOK("recvBook", 6377), CS_ACTIVITY_9YEARS_RRECVCARD("recvCardBoxReward", 6379),
            CS_ACTIVITY_9YEARS_LUCKYLOTTERY("luckyLottery", 6381),
            CS_ACTIVITY_9YEARS_RECVLUCKYSCOREREWARD("recvLuckyScoreReward", 6382),
            CS_ACTIVITY_9YEARS_EXCHANGESHOP("exchangeShop", 6383), SC_ACTIVITY_9YEARS_PUSHNORMALPAY("pushNormalPay", 6384),
            CS_ACTIVITY_9YEARS_USEVOUCHER("useVoucher", 6385), CS_ACTIVITY_9YEARS_BUYFREEGIFT("buyFreeGift", 6386),
            SC_ACTIVITY_9YEARS_UPDATETASK("updateTask", 6387), CS_ACTIVITY_9YEARS_MAKECAKE("makeCake", 6389),
            CS_ACTIVITY_9YEARS_RECVTARGETREWARD("recvTargetReward", 6390), CS_ACTIVITY_9YEARS_GETRANKLIST("getRankList", 6391),
            CS_ACTIVITY_9YEARS_GETCAKESCOREINFO("getCakeScoreInfo", 6392), CS_ACTIVITY_9YEARS_CAKEEXCHANGE("cakeExchange", 6393),
            CS_ACTIVITY_9YEARS_OPENCAKEBAG("openCakeBag", 6394), CS_ACTIVITY_9YEARS_GETCAKEBAGSTATUS("getCakeBagStatus", 6395),
            SC_ACTIVITY_9YEARS_UPDATERESOURCE("updateResource", 6396)],
        "activityAllServerGift" => [
            CS_ACTIVITY_ALLSERVERGIFT_GETINFO("getInfo", 6399), CS_ACTIVITY_ALLSERVERGIFT_FREEREWARD("recvFreeReward", 6400),
            CS_ACTIVITY_ALLSERVERGIFT_ALLBUYREWARD("recvAllBuyReward", 6401),
            CS_ACTIVITY_ALLSERVERGIFT_TASKREWARD("recvTaskReward", 6402),
            CS_ACTIVITY_ALLSERVERGIFT_ALLTASKREWARD("oneKeyRecvTaskReward", 6403),
            CS_ACTIVITY_ALLSERVERGIFT_USERVOUCHER("userVoucher", 6404),
            CS_ACTIVITY_ALLSERVERGIFT_ALLBUYLIMITREWARD("recvAllBuyLimitReward", 6405)],
        "activityBattlePass" => [
            CS_ACTIVITY_BATTLE_PASS_GETINFO("getInfo", 6409), CS_ACTIVITY_BATTLE_PASS_RECVREWARD("recvReward", 6410),
            CS_ACTIVITY_BATTLE_PASS_USEVOUCHER("useVoucher", 6411), SC_ACTIVITY_BATTLE_PASS_PUSHNORMALPAY("pushNormalPay", 6412)],
        "halidomCollection" => [
            CS_HALIDOMCOLLECTION_GETINFO("getInfo", 6415), CS_HALIDOMCOLLECTION_EQUIPHALIDOM("equipHalidom", 6416),
            CS_HALIDOMCOLLECTION_UNLOADHALIDOM("unloadHalidom", 6417), CS_HALIDOMCOLLECTION_RECVREWARD("recvReward", 6418),
            CS_HALIDOMCOLLECTION_RECVCARDDAILYREWARD("recvCardDailyReward", 6419),
            CS_HALIDOMCOLLECTION_USEVOUCHER("useVoucher", 6420), SC_HALIDOMCOLLECTION_PUSH_PAY("pushNormalPay", 6421),
            CS_HALIDOMCOLLECTION_QUICKRECVREWARD("quickRecvReward", 6422),
            SC_HALIDOMCOLLECTION_AUTORECVREWARD("autoRecvReward", 6423)],
        "activityTopAnimalRecruit" => [
            CS_ACTIVITY_TOPANIMAL_RAFFLE_GETINFO("getInfo", 6426), CS_ACTIVITY_TOPANIMAL_RAFFLE_RAFFLE("raffle", 6428),
            CS_ACTIVITY_TOPANIMAL_RAFFLE_OPENBOX("openBox", 6430), CS_ACTIVITY_TOPANIMAL_RAFFLE_TASK("getTaskReward", 6432),
            CS_ACTIVITY_TOPANIMAL_RAFFLE_VOUCHER("useVoucher", 6434), SC_ACTIVITY_TOPANIMAL_RAFFLE_PAY_PUSH("pushNormalPay", 6436)],
        "formation" => [CS_FORMATION_TOP_ANIMAL_ADD("addTopAnimal", 6439), CS_FORMATION_TOP_ANIMAL_REMOVE("removeTopAnimal", 6440)],
        "activityTopAnimalRecharge" => [
            CS_ACTIVITY_TOP_ANIMAL_RECHARGE_GETINFO("getInfo", 6460), CS_ACTIVITY_TOP_ANIMAL_RECHARGE_USEVOUCHER("useVoucher", 6461),
            SC_ACTIVITY_TOP_ANIMAL_RECHARGE_UPDATE("update", 6462)],
        "fodderBattle" => [
            CS_CC_GETHOSTINFO_FODDER("getHostInfo", 6476), CS_CC_LOGININ_FODDER("login", 6477),
            CS_CC_USER_ENTER_FODDER("userEnter", 6478), CS_CC_GET_BATTLE_LOG_FODDER("getGuildBattleLog", 6480),
            CS_CC_GET_BATTLE_LOG_FODDER_OTHER("getBattleLogListMain", 6482), CS_CC_USER_LEAVE_FODDER("userLeave", 6488),
            CS_CC_USER_REGISTER_FODDER("justPlay", 6489), CS_CC_MARCH_FODDER("march", 6490),
            CS_CC_MARCH_CANCEL_FODDER("cancelMarch", 6491), CS_CC_REBORN_FODDER("reborn", 6493),
            CS_CC_CHALLENGE_FODDER("challenge", 6494), CS_CROSSGUILDWAR_REGISTER_FODDER("register", 6498),
            CS_CROSSGUILDWAR_JOINMEMBERLIST_FODDER("getJointMemberList", 6500), CS_FODDERBATTLE_GETINFO("getInfo", 6503),
            CS_FODDERBATTLE_GET_SCHEDULE("getSchedule", 6504), CS_FODDERBATTLE_GET_SCHEDULE_GUILDINFO("getBattleMember", 6505),
            CS_CROSSGUILDWAR_GETTASKINFO_FODDER("getTaskInfo", 6510), CS_CROSSGUILDWAR_RECVTASKREWARD_FODDER("recTaskReward", 6512),
            CS_CROSSGUILDWAR_RECVTASKREWARD_ALL_FODDER("recAllTaskReward", 6513), CS_ITEM_PICK_FODDER("pickItem", 6520),
            CS_ITEM_USE_FODDER("useItem", 6521), CS_CHANGE_BRANCH_FODDER("changeBranch", 6522),
            CS_GET_BATTLE_INFO("getBattleFormation", 6523), SC_CC_USER_ALLDATA_FODDER("PushUserMapAllData", 6526),
            SC_CC_USER_ENTER_MAP_FODDER("PushUserEnterMap", 6527), SC_CC_USER_CHANGE_POS_FODDER("PushUserChangePos", 6528),
            SC_CC_USER_STATE_CHANGE_FODDER("PushUserStateChange", 6529), SC_CC_USER_REVIVE_FODDER("PushUserRevive", 6530),
            SC_CC_USER_MARCH_FODDER("PushUserMarch", 6531), SC_CC_USER_MARCH_RATE_CHANGE_FODDER("PushUserMarchRate", 6532),
            SC_CC_USER_MARCH_CANCEL_FODDER("PushUserCancelMarch", 6533),
            SC_CC_USER_ENERGY_CHANGE_FODDER("PushUserEnergyChange", 6534),
            SC_CC_USER_POINTS_CHANGE_FODDER("PushUserPointsChange", 6535),
            SC_CC_USER_POWER_CHANGE_FODDER("PushUserPowerChange", 6536), SC_CC_BUILD_LIST_CHANGE_FODDER("PushBuildListChange", 6537),
            SC_CC_DOBATTLE_BEGIN_FODDER("PushDoBattleBegin", 6538), SC_CC_DOBATTLE_END_FODDER("PushDoBattleEnd", 6539),
            SC_CC_CAR_NEW_OPEN_FODDER("PushFodderBattleNewCart", 6542), SC_CC_CAR_MARCH_RATE_FODDER("PushCartMarchRate", 6543),
            SC_CC_CAR_ARRIVE_FODDER("PushFordeerBattleCartArrive", 6544),
            SC_CC_CAR_BATTLE_SET_FODDER("PushFordeerBattleCartSettle", 6545), SC_CC_CAR_POS_CHANGE_FODDER("PushCartPosChange", 6546),
            SC_CC_ITEM_NEW_FODDER("PushFodderBattleNewItem", 6547), SC_CC_ITEM_GET_FODDER("PushFodderBattleGetItem", 6548),
            SC_CC_ITEM_DROP_FODDER("PushFodderBattleDropItem", 6549), SC_CC_ITEM_USE_FODDER("PushFodderBattleUseItem", 6550),
            SC_CC_ITEM_PICK_FODDER("PushFodderBattlePickItem", 6551), SC_CC_CAR_OCCUPY_INFO("PushFodderBattleCartOccupyInfo", 6552),
            SC_CC_BATTLE_RESULT_FODDER("PushFodderBattleBattleResult", 6556),
            SC_CC_CAR_BATTLE_RESULT_FODDER("PushFodderBattleCartBattleStatue", 6557),
            SC_CC_CAR_BATTLE_BRANCH_CHANGE_FODDER("PushFodderBattleBranchChange", 6558),
            SC_CC_CAR_BATTLE_PATH_CHANGE_FODDER("PushFodderBattleCartPathChange", 6559),
            CS_CHAT_FODDER_BATTLE_SEND("sendChatMsg", 6562), CS_CHAT_FODDER_BATTLE_GET("getLastChatMsg", 6563)],
        "crossGuildWar" => [
            CS_CC_GET_MARCH_TIME_FODDER("preMarch", 6481), CS_CC_RECEIVE_PORCEED_FODDER("receivePorceed", 6485),
            CS_CC_MARCH_SPEEDUP_FODDER("marchSpeedUp", 6492), CS_CROSSGUILDWAR_GET_BASEINFO_FODDER("getBaseInfo", 6497),
            CS_CROSSGUILDWAR_GUILDRANK_FODDER("getGuildRankList", 6499), CS_CROSSGUILDWAR_GRTPROMOTED_FODDER("getPromoted", 6501)],
    ],
    normal_raw: [
        CS_USER_LOGIN("user.login", 12), CS_BAG_COMPOSE_TOP_ANIMAL("exchange.composeTopAnimal", 124),
        RE_TM_CHAT_MSG_PUSH("re.tmChat.getMsg", 666), SC_ACT8YEARS_TICKET_REFRESH("activity8Years.updateTicket", 6288),
        SC_HEROSKIN_PUSH_ADD("re.heroSkin.addSkin", 6307), CS_TM_GETINFO("tmTimeMachine.getInfo", 6309),
        SC_VOUCHER_RECHARGE("voucher.recharge", 6324), CS_COUNSELLOR_RECEIVERETURN("counsellor.receiveReturn", 6344),
        SC_ACTIVITY_ALLSERVERGIFT_BUYGIFT("re.activityAllServerGift.buyGift", 6406),
        SC_CROSSGUILDWAR_TASKCHANGE_FODDER("re.fodderBattle.taskNotify", 6511),
    ],
    tm_groups: [
        "tmForge" => [CS_FORGE_UPGRADE_TREASURE5("oneKeyUpgradeTreasure", 522), CS_FORGE_UPGRADE_AWAKE10("oneKeyUpgradeAwake", 527)],
        "timeMachineDotaTop" => [
            CS_TIME_MACHINE_TOWER_GETLAYERRANK("getLayerRank", 1278), CS_TIME_MACHINE_TOWER_GETHISTORYRANK("getRankHistory", 1279),
            CS_TIME_MACHINE_TOWER_GETTDREWARD("recTodayReward", 1280), CS_TIME_MACHINE_TOWER_RECRANKREWARD("recRankReward", 1281),
            CS_TM_DOTA_GETFORMATION("getFormation", 1282), CS_TM_DOTA_GETRANKINFO("getInfo", 1283)],
        "tmCrossCrazyWar" => [
            CS_CC_USER_ENTER_EMPER("userEnter", 5744), CS_CC_GET_BATTLE_LOG_EMPER("GetGuildBattleLog", 5745),
            CS_CC_GET_MARCH_TIME_EMPER("preMarch", 5746), CS_CC_GET_ONHOOK_INFO_EMPER("getProceedInfo", 5748),
            CS_CC_RECEIVE_PORCEED_EMPER("receivePorceed", 5749), CS_CC_USER_LEAVE_EMPER("userLeave", 5752),
            CS_CC_USER_REGISTER_EMPER("justPlay", 5753), CS_CC_MARCH_EMPER("march", 5754),
            CS_CC_MARCH_SPEEDUP_EMPER("marchSpeedUp", 5755), CS_CC_REBORN_EMPER("reborn", 5756),
            CS_CC_CHALLENGE_EMPER("challenge", 5757), CS_CC_REVIVE_EMPER("revive", 5758),
            CS_CROSSGUILDWAR_EMPER_REGISTER("register", 5761), CS_CROSSGUILDWAR_EMPER_GUILDRANK("getGuildRankList", 5762),
            CS_CROSSGUILDWAR_EMPER_JOINMEMBERLIST("getJointMemberList", 5763),
            CS_CROSSGUILDWAR_EMPER_GRTPROMOTED("getPromoted", 5764), CS_CROSSGUILDWAR_EMPER_GETTASKINFO("getTaskInfo", 5769),
            CS_CROSSGUILDWAR_EMPER_RECVTASKREWARD("recvTaskReward", 5770),
            CS_CROSSGUILDWAR_EMPER_RECVBOXREWARD("recvBoxReward", 5772)],
        "tmCrazyWar" => [
            SC_CC_USER_ALLDATA_EMPER("PushUserMapAllData", 5775), SC_CC_USER_ENTER_MAP_EMPER("PushUserEnterMap", 5776),
            SC_CC_USER_CHANGE_POS_EMPER("PushUserChangePos", 5777), SC_CC_USER_STATE_CHANGE_EMPER("PushUserStateChange", 5778),
            SC_CC_USER_REVIVE_EMPER("PushUserRevive", 5779), SC_CC_USER_MARCH_EMPER("PushUserMarch", 5780),
            SC_CC_USER_ENERGY_CHANGE_EMPER("PushUserEnergyChange", 5781),
            SC_CC_USER_POINTS_CHANGE_EMPER("PushUserPointsChange", 5782), SC_CC_USER_POWER_CHANGE_EMPER("PushUserPowerChange", 5783),
            SC_CC_USER_GEMNUM_CHANGE_EMPER("PushUserBaoZhuChange", 5784), SC_CC_BUILD_LIST_CHANGE_EMPER("PushBuildListChange", 5787),
            SC_CC_BOX_ADD_EMPER("PushBoxAdd", 5789), SC_CC_BOX_REMOVE_EMPER("PushBoxRemove", 5790),
            SC_CC_NPC_LIST_CHANGE_EMPER("PushNpcInfoChange", 5792), SC_CC_DOBATTLE_BEGIN_EMPER("PushDoBattleBegin", 5794),
            SC_CC_DOBATTLE_END_EMPER("PushDoBattleEnd", 5795), SC_CC_BATTLE_RESULT_EMPER("PushCrazyWarBattleResult", 5812)],
        "tmMultipleFunds" => [
            CS_MULTIPLEFUNDS_GETINFO("getInfo", 5876), CS_MULTIPLEFUNDS_RECEIVE("receiveReward", 5877),
            CS_MULTIPLEFUNDS_RECEIVEALL("receiveAllReward", 5878), CS_MULTIPLEFUNDS_HAVEREWARD("getRedDoc", 5879),
            SC_MULTIPLEFUNDS_BUY("buy", 5880), CS_MULTIPLEFUNDS_USEVOUCHER("useVoucher", 5881)],
        "tmActivityQushuiyan" => [
            CS_QUSHUIYAN_GETINFO("getInfo", 5884), CS_QUSHUIYAN_TASKINFO("getTaskInfo", 5885),
            CS_QUSHUIYAN_TASKRECEIVE("receiveTask", 5886), CS_QUSHUIYAN_TASKRECEIVEALL("receiveAllTask", 5887),
            CS_QUSHUIYAN_MORALIST("getMoralList", 5888), CS_QUSHUIYAN_EXCHAGEMORAL("exchangeMoral", 5889),
            CS_QUSHUIYAN_RECHARGELIST("getRechargeList", 5890), CS_QUSHUIYAN_RECHARGEREWARD("receiveRechargeReward", 5891),
            CS_QUSHUIYAN_RANKLIST("getRankList", 5892), CS_QUSHUIYAN_SINGLERAFFLE("singleRaffle", 5893),
            CS_QUSHUIYAN_MULTIRAFFLE("multiRaffle", 5894), SC_QUSHUIYAN_TASK_PUSH("updateTask", 5895)],
        "tmBeastTemple" => [
            CS_BEASTTEMPLE_GETINFO("getInfo", 6367), CS_BEASTTEMPLE_ACTIVE("active", 6368), CS_BEASTTEMPLE_LEVELUP("levelUp", 6369),
            CS_BEASTTEMPLE_STARUP("starUp", 6371), CS_BEASTTEMPLE_GIFT_LEVELUP("giftLevelUp", 6372)],
        "timeMachineArena" => [
            CS_TM_ARENA_GETINFO("getInfo", 6387), CS_TM_ARENA_ENTER("enter", 6388), CS_TM_ARENA_LEAVE("leave", 6389),
            CS_TM_ARENA_RECTODAYREWARD("recTodayReward", 6390), CS_TM_ARENA_CHALLENGEREWARD("recChallengeReward", 6391),
            CS_TM_ARENA_CHALLENGEWINNER("challengeWinner", 6392), CS_TM_ARENA_ATTACKMATCHPLAYER("attackMatchPlayer", 6393),
            CS_TM_ARENA_GETMATCHPLAYER("refreshMatchPlayer", 6394), CS_TM_ARENA_MATCH("match", 6395),
            CS_TM_ARENA_GETDEFENDRANK("getDefendRank", 6396), CS_TM_ARENA_RESULTRANK("getRankHistory", 6397),
            CS_TM_ARENA_RECRANKREWARD("recRankReward", 6398), CS_TM_ARENA_GETFORMATION("getFormation", 6399)],
        "tmWorldChampion" => [
            CS_TM_WORLD_CHAMPION_GETINFO("getInfo", 6403), CS_TM_WORLD_CHAMPION_GETPROMOTIONINFO("getPromotionInfo", 6404),
            CS_TM_WORLD_CHAMPION_GETPAGEINFO("getPageInfo", 6405), CS_TM_WORLD_CHAMPION_GETAUDITIONRECORD("getAuditionRecord", 6406),
            CS_TM_WORLD_CHAMPION_GETTEMPLEINFO("getTempleInfo", 6407),
            CS_TM_WORLD_CHAMPION_GETBATTLERECORDBYRECORDID("getBattleRecordByRecordId", 6408),
            CS_TM_WORLD_CHAMPION_SUPPORT("support", 6409), CS_TM_WORLD_CHAMPION_GETBATTLEDATAOFUSER("getBattleDataOfUser", 6410),
            CS_TM_WORLD_CHAMPION_GETPROMOTIONREPORT("getPromotionReport", 6411), CS_TM_WORLD_CHAMPION_DOPRAISE("doPraise", 6412),
            CS_TM_WORLD_CHAMPION_USERATTACK("userAttack", 6413), CS_TM_WORLD_CHAMPION_CHALLENGE("challenge", 6414),
            CS_TM_WORLD_CHAMPION_UPDATEFIGHTINFO("updateFightInfo", 6415),
            CS_TM_WORLD_CHAMPION_GETFORCERANK("getFightForceRank", 6417)],
        "timeMachineTrader" => [
            CS_TIME_MACHINE_TRADER_GETINFO("getInfo", 6435), CS_TIME_MACHINE_TRADER_RECVFUNDREWARD("recvFundReward", 6436),
            CS_TIME_MACHINE_TRADER_BUYFREEGIFT("buyFreeGift", 6437)],
        "timeMachineTreasureBox" => [
            CS_TIME_MACHINE_TREASUREBOX_GETINFO("getInfo", 6441), CS_TIME_MACHINE_TREASUREBOX_BOXLEVELUP("boxLevelUp", 6442),
            CS_TIME_MACHINE_TREASUREBOX_OPENBOX("openBox", 6443)],
        "tmProgress" => [CS_TREASURE_GETINFO("getInfo", 6447), CS_TREASURE_RECREWARD("receiveReward", 6448)],
        "tmCrossGuildNameActivity" => [
            CS_LEGIONCROSS_SERVER_RENAME_GETINFO("getInfo", 6464), CS_LEGIONCROSS_SERVER_RENAME_RECV_REWARD("recReward", 6465)],
    ],
    tm_raw: [
        CS_USER_ENTER("tmUser.enter", 11), CS_EVOLVE_DOEVOLVE_QUICK("tmEvolve.oneKeyEvolve", 453),
        CS_COPY_ONEKEYPASS("timeMachineCopy.oneKeySweepCopy", 758), CS_DOTA_ONEKEY_PASS("tmDota.oneKeySweepNormalTower", 1268),
        CS_ASTROLABE_ADD_NODE_QUICK("tmDestiny.oneKeyAddNode", 2352),
        CS_ALLPLATFORM_COLLECTION_GET_OTHER_INFO("tmTop.getOtherCollectionInfo", 4865),
        CS_FIRST_RIDE_RECEIVE_RANDOM_REWARD("tmFirstRide.receiveRandomReward", 5438),
        CS_CROSSGUILDWAR_EMPER_GET_BASEINFO("crossCrazyWar.getBaseInfo", 5760),
        SC_CROSSGUILDWAR_EMPER_TASKNOTIFY("tmRe.crossCrazyWar.taskNotify", 5768),
        SC_LEGION_CROSS_SERVER_APPLY("re.tmCrossGuild.apply", 5907), SC_TM_ARENA_REFRESHWINNER("re.timeMachineArena.newWinner", 6400),
        SC_TIME_MACHINE_PUSH_NORMAL_PAY("timeMachine.pushNormalPay", 6444), SC_TREASURE_DATACHANGE("re.tmProgress.dataChange", 6449),
        CS_TIME_MACHINE_PLOY_GIFT_GETINFO("tmPloyGift.getInfo", 6451), SC_TIME_MACHINE_PLOY_GIFT("re.TMPloyGift.pushNormalPay", 6452),
        SC_LEGIONCROSS_SERVER_RENAME_USEVOUCHER("re.tmCrossGuildNameActivity.numChange", 6466),
    ],
}

pub const INLINE_RPC_LITERALS: &[(&str, &str)] = &[
    ("INLINE_USER_STAT", "user.stat"),
    ("INLINE_USER_GUIDE_EVENT", "user.guideEvent"),
    ("INLINE_HALIDOM_GET_INFO", "halidom.getInfo"),
    ("INLINE_BARRAGE_GET", "barrage.get"),
    ("INLINE_BARRAGE_SET", "barrage.set"),
    ("INLINE_BARRAGE_ENTER_ROOM", "barrage.enterRoom"),
    ("INLINE_BARRAGE_LEAVE_ROOM", "barrage.leaveRoom"),
    ("INLINE_BARRAGE_SEND", "barrage.sendBarrage"),
    ("INLINE_BARRAGE_GET_RECORD", "barrage.getBarrageRecord"),
    ("INLINE_BARRAGE_SEND_GW", "barrage.sendGuildWarBarrage"),
    ("INLINE_BARRAGE_GET_GW_RECORD", "barrage.getGWBarrageRecord"),
    ("INLINE_TM_HERO_BOOK_GET_INFO", "tmHeroBook.getInfo"),
    ("INLINE_TM_TRANSFER_SEX_GET_INFO", "tmTransferSex.getInfo"),
    ("INLINE_PUSH_COPPER_OVER_MAX", "re.user.copperOverMax"),
    ("INLINE_PUSH_SERVER_NOTIFY", "SERVER_NOTIFY"),
    ("INLINE_PUSH_DUMMY_CALLBACK", "dummy"),
];

pub fn by_const_name(name: &str) -> Option<&'static NetConstEntry> {
    ALL_NET_CONSTS.iter().find(|e| e.const_name == name)
}
pub fn first_by_method(method: &str) -> Option<&'static NetConstEntry> {
    ALL_NET_CONSTS.iter().find(|e| e.method == method)
}
pub fn by_method(method: &str) -> Vec<&'static NetConstEntry> {
    ALL_NET_CONSTS.iter().filter(|e| e.method == method).collect()
}
pub fn by_module_prefix(prefix: &str) -> Vec<&'static NetConstEntry> {
    let pat = format!("{}.", prefix);
    ALL_NET_CONSTS.iter().filter(|e| e.method.starts_with(&pat)).collect()
}
pub fn is_known_rpc(method: &str) -> bool {
    ALL_NET_CONSTS.iter().any(|e| e.method == method)
        || INLINE_RPC_LITERALS.iter().any(|(_, m)| *m == method)
        || tm::ALL_TM_NET_CONSTS.iter().any(|e| e.method == method)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_counts_ordering_directions_and_lookups_match() {
        assert_eq!(ALL_NET_CONSTS.len(), NET_CONST_COUNT);
        assert_eq!(tm::ALL_TM_NET_CONSTS.len(), tm::NET_CONST_TM_COUNT);
        assert!(ALL_NET_CONSTS.windows(2).all(|w| w[0].line < w[1].line));
        assert!(tm::ALL_TM_NET_CONSTS.windows(2).all(|w| w[0].line < w[1].line));

        let count_dirs = |s: &[NetConstEntry]| {
            let (mut cs, mut sp, mut re, mut ot) = (0, 0, 0, 0);
            for e in s {
                match e.direction {
                    RpcDirection::ClientToServer => cs += 1,
                    RpcDirection::ServerPush => sp += 1,
                    RpcDirection::RewardOrEventPush => re += 1,
                    RpcDirection::Other => ot += 1,
                }
            }
            (cs, sp, re, ot)
        };
        assert_eq!(count_dirs(ALL_NET_CONSTS), (1853, 146, 105, 3));
        assert_eq!(count_dirs(tm::ALL_TM_NET_CONSTS), (1787, 213, 18, 3));

        assert_eq!(by_method("guildcopy.allFight").len(), 2);
        assert!(!by_module_prefix("user").is_empty());
        assert!(is_known_rpc(tm::CS_USER_ENTER) && is_known_rpc("barrage.enterRoom"));
    }
}
