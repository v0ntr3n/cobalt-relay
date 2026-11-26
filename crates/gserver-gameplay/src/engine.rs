//! Unified stateful RPC router (`GameplayEngine`) wiring together Player, Town/ImperialCity,
//! Chat/GM, Mail/Reward Center, Shop/Tavern & Secondary Raffles, PvE Copy/Dungeons/WorldBoss/Rebel,
//! Arena (`arena.*` & `timeMachineArena.*`), and Sign/Online/DailyTask/Achievement/EatChicken modules.

use std::collections::BTreeMap;

use crate::battle_copy::{
    AchievementEngine, ArenaEngine, AwakeCopyEngine, BabelTowerEngine, CopyCampaignEngine,
    DailyCopyEngine, DailyTaskEngine, DevilCopyEngine, DiscoverEngine, DotaTowerEngine,
    EatChickenService, EliteCopyEngine, SignAndOnlineService, SoaringCopyEngine,
    WorldBossAndRebelEngine,
};
use crate::chat_gm::{ChatChannel, ChatMsgType, ChatService};
use crate::mail::MailAndRewardCenter;
use crate::player::PlayerState;
use crate::protocol::{rpc, DtoValue, PushPacket, RpcRequest, RpcResponse};
use crate::shop_pub::{
    ActivityDiscountShopEngine, BabelTowerShopEngine, RafflePool, SecondaryRaffleHub, ShopEngine,
    TavernRaffleEngine, TimeMachineTraderEngine,
};
use crate::town::TownRoomManager;

/// Outcome of routing a single client `RpcRequest` through `GameplayEngine`.
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    pub response: RpcResponse,
    /// Asynchronous pushes queued for the calling client (`user.updateUser`, `bag.update`, `re.hero.new`, etc.).
    pub client_pushes: Vec<PushPacket>,
    /// Asynchronous pushes queued for other connected players `(target_uid, PushPacket)`.
    pub peer_pushes: Vec<(u64, PushPacket)>,
}

/// Complete server-side gameplay coordinator for GServer (`vnsg2` + `game_TM`).
#[derive(Debug, Clone)]
pub struct GameplayEngine {
    pub players: BTreeMap<u64, PlayerState>,
    pub town: TownRoomManager,
    pub chat: ChatService,
    pub mail_center: MailAndRewardCenter,
    pub raffle: TavernRaffleEngine,
    pub secondary_raffle: SecondaryRaffleHub,
    pub shop: ShopEngine,
    pub trader_shop: TimeMachineTraderEngine,
    pub babel_shop: BabelTowerShopEngine,
    pub activity_shop: ActivityDiscountShopEngine,
    pub copy: CopyCampaignEngine,
    pub daily_copy: DailyCopyEngine,
    pub babel_tower: BabelTowerEngine,
    pub dota_tower: DotaTowerEngine,
    pub awake_copy: AwakeCopyEngine,
    pub world_boss_rebel: WorldBossAndRebelEngine,
    pub discover: DiscoverEngine,
    pub devil_copy: DevilCopyEngine,
    pub soaring_copy: SoaringCopyEngine,
    pub arena: ArenaEngine,
    pub sign_online: SignAndOnlineService,
    pub daily_task: DailyTaskEngine,
    pub achievements: AchievementEngine,
    pub eat_chicken: EatChickenService,
}

impl Default for GameplayEngine {
    fn default() -> Self {
        Self::new(0x564E_5347_325F_3351)
    }
}

/// Rewrites Base-mode S2C push callback names into `game_TM` (`consts_TM/NetConst.lua`) push names
/// when a `tm*` / `timeMachine*` RPC triggered the mutation.
pub fn rewrite_push_for_tm(pkt: &mut PushPacket) {
    let mapped = match pkt.callback_name.as_str() {
        "user.updateUser" => Some("tmUser.updateUser"),
        "bag.update" => Some("tmBag.update"),
        "re.hero.new" => Some("re.tmHero.new"),
        "re.mail.newMail" => Some("tmRe.mail.newMail"),
        "re.reward.new" => Some("re.tmReward.new"),
        "shop.open" => Some("tmShop.open"),
        "sc.recruitment.addnum" => Some("tmSc.recruitment.addnum"),
        "re.arena.dataRefresh" => Some("tmRe.arena.dataRefresh"),
        _ => None,
    };
    if let Some(target) = mapped {
        pkt.callback_name = target.to_owned();
    }
}

/// Returns `(canonical_method, is_tm)` for `tm*` / `timeMachine*` gameplay RPCs, preserving
/// explicit TM methods (`tmMail.*`, `tmReward.*`, `timeMachineCopy.*`, `timeMachineArena.*`,
/// `timeMachineTrader.*`, `tmChat.sendCrossGuild`, `tmChat.getCrossGuildLastMsgs`) that have
/// dedicated TM state handlers.
fn normalize_gameplay_method(method: &str) -> (String, bool) {
    if method.starts_with("timeMachine") {
        // Normalize `timeMachineCopy_daily.*` -> `copy_daily.*` while keeping `is_tm = true`
        if let Some(rest) = method.strip_prefix("timeMachineCopy_daily.") {
            return (format!("copy_daily.{rest}"), true);
        }
        return (method.to_owned(), true);
    }
    if matches!(
        method,
        "tmChat.sendCrossGuild" | "tmChat.getCrossGuildLastMsgs"
    ) {
        return (method.to_owned(), true);
    }
    if method.starts_with("tmMail.")
        || method.starts_with("tmReward.")
        || method.starts_with("tmArena.")
    {
        return (method.to_owned(), true);
    }
    if let Some(rest) = method.strip_prefix("tm") {
        let mut chars = rest.chars();
        if let Some(first) = chars.next() {
            if first.is_ascii_uppercase() {
                let mut norm = String::with_capacity(rest.len());
                norm.push(first.to_ascii_lowercase());
                norm.push_str(chars.as_str());
                return (norm, true);
            }
        }
    }
    (method.to_owned(), false)
}

fn dto_to_key_string(val: Option<&DtoValue>) -> String {
    match val {
        Some(DtoValue::Str(s)) => s.clone(),
        Some(DtoValue::Int(n)) => n.to_string(),
        Some(DtoValue::Float(f)) => (*f as i64).to_string(),
        _ => String::new(),
    }
}

fn dto_to_u32_vec(val: Option<&DtoValue>) -> Vec<u32> {
    match val {
        Some(DtoValue::Array(arr)) => arr.iter().filter_map(DtoValue::as_u32).collect(),
        Some(DtoValue::Map(m)) => m.values().filter_map(DtoValue::as_u32).collect(),
        _ => Vec::new(),
    }
}

fn parse_pos_and_count(req: &RpcRequest, default_count: u32) -> (u32, u32) {
    if let Some(z) = req.arg_zero() {
        let pos = z.as_u32().unwrap_or(0);
        let cnt = req
            .arg_at_lua0(1)
            .and_then(DtoValue::as_u32)
            .filter(|&n| n == 1 || n == 10)
            .unwrap_or(default_count);
        (pos, cnt)
    } else if req.args.len() == 1 && req.args[0].as_u32() == Some(10) {
        // Legacy 1-arg 10x Red draw (`args = [10]`); real Lua `RaffleProxy` sends `pos` in `{0, 1}`.
        (0, 10)
    } else {
        let pos = req.args.first().and_then(DtoValue::as_u32).unwrap_or(0);
        let cnt = req
            .args
            .get(1)
            .and_then(DtoValue::as_u32)
            .filter(|&n| n == 1 || n == 10)
            .unwrap_or(default_count);
        (pos, cnt)
    }
}

impl GameplayEngine {
    pub fn new(rng_seed: u64) -> Self {
        Self {
            players: BTreeMap::new(),
            town: TownRoomManager::new(),
            chat: ChatService::new(),
            mail_center: MailAndRewardCenter::new(),
            raffle: TavernRaffleEngine::new(rng_seed),
            secondary_raffle: SecondaryRaffleHub::new(rng_seed ^ 0xA5A5_5A5A_F0F0_0F0F),
            shop: ShopEngine::new(),
            trader_shop: TimeMachineTraderEngine::new(),
            babel_shop: BabelTowerShopEngine::new(),
            activity_shop: ActivityDiscountShopEngine::new(),
            copy: CopyCampaignEngine::new(),
            daily_copy: DailyCopyEngine::default(),
            babel_tower: BabelTowerEngine::default(),
            dota_tower: DotaTowerEngine::default(),
            awake_copy: AwakeCopyEngine::default(),
            world_boss_rebel: WorldBossAndRebelEngine::new(),
            discover: DiscoverEngine::default(),
            devil_copy: DevilCopyEngine::default(),
            soaring_copy: SoaringCopyEngine::default(),
            arena: ArenaEngine::new(),
            sign_online: SignAndOnlineService::default(),
            daily_task: DailyTaskEngine::default(),
            achievements: AchievementEngine::default(),
            eat_chicken: EatChickenService::default(),
        }
    }

    pub fn register_player(&mut self, player: PlayerState) {
        self.players.insert(player.uid, player);
    }

    /// Dispatches any supported `vnsg2` or `game_TM` gameplay RPC method for `uid`.
    pub fn dispatch(&mut self, uid: u64, req: &RpcRequest, now_ts: u64) -> DispatchOutcome {
        if !self.players.contains_key(&uid) {
            return DispatchOutcome {
                response: RpcResponse::error(req, "user_not_found", now_ts),
                client_pushes: Vec::new(),
                peer_pushes: Vec::new(),
            };
        }

        let (norm_method, is_tm) = normalize_gameplay_method(&req.method);
        let m = norm_method.as_str();

        let mut client_pushes = Vec::new();
        let mut peer_pushes = Vec::new();

        // Check SecondaryRaffleHub first (`godarmRaffle.*`, `heroLegendRaffle.*`, `activity*Recruit.*`, `raffleArm.*`)
        let synthesized_named_args;
        let sec_args: &[DtoValue] = if req.args.is_empty() && !req.named_args.is_empty() {
            synthesized_named_args = vec![DtoValue::map(req.named_args.iter().cloned())];
            &synthesized_named_args[..]
        } else {
            &req.args[..]
        };

        let sec_res = {
            let player = self.players.get_mut(&uid).unwrap();
            self.secondary_raffle
                .dispatch_rpc(&req.method, sec_args, player, now_ts)
                .or_else(|| self.secondary_raffle.dispatch_rpc(m, sec_args, player, now_ts))
        };

        let res: Result<DtoValue, &'static str> = if let Some(sr) = sec_res {
            sr.map(|(ret, pushes)| {
                client_pushes.extend(pushes);
                ret
            })
        } else {
            match m {
                // --- User, Bag, Hero & Formation ---
                rpc::USER_GET_USER => Ok(self.players[&uid].to_user_dto()),
                rpc::BAG_INFO => Ok(self.players[&uid].bag.to_bag_info_dto()),
                rpc::HERO_GET_ALL => Ok(self.players[&uid].roster.to_all_heroes_dto()),
                rpc::FORMATION_GET_POSITION_INFO => {
                    Ok(self.players[&uid].roster.to_formation_position_info_dto())
                }

                // --- Town, City, RoyalCity, ImperialCity & Castle ---
                rpc::CITY_ENTER => {
                    let town_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let x = req.args.get(1).and_then(DtoValue::as_i64).map(|v| v as i32);
                    let y = req.args.get(2).and_then(DtoValue::as_i64).map(|v| v as i32);
                    self.town
                        .enter_town(&self.players[&uid], town_id, x, y, now_ts)
                        .map(|(ret, bcasts)| {
                            peer_pushes.extend(bcasts);
                            ret
                        })
                }
                rpc::CITY_ENTER_SHADOW_INFO => {
                    let town_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    Ok(self.town.enter_shadow_info(&self.players[&uid], town_id))
                }
                rpc::ROYALCITY_ENTER | "royalcity.enter" => {
                    let x = req.args.first().and_then(DtoValue::as_i64).map(|v| v as i32);
                    let y = req.args.get(1).and_then(DtoValue::as_i64).map(|v| v as i32);
                    self.town
                        .enter_town(&self.players[&uid], 999, x, y, now_ts)
                        .map(|(ret, bcasts)| {
                            peer_pushes.extend(bcasts);
                            ret
                        })
                }
                rpc::IMPERIALCITY_HOST | "imperialcity.getEnterInfo" => Ok(TownRoomManager::imperial_city_host_dto(
                    "127.0.0.1",
                    9001,
                    &req.token,
                )),
                rpc::IMPERIALCITY_LOGIN => Ok(TownRoomManager::imperial_city_login_dto(
                    uid,
                    &self.players[&uid].server_id,
                )),
                rpc::CITY_LEAVE | rpc::ROYALCITY_LEAVE | "royalcity.leave" => {
                    let (ret, bcasts) = self.town.leave_town(uid, now_ts);
                    peer_pushes.extend(bcasts);
                    Ok(ret)
                }
                rpc::CITY_GET_ENTER_LIST => Ok(self.town.get_enter_list(uid)),
                rpc::TOWN_MOVE | rpc::ROYALCITY_MOVE | "royalcity.move" => {
                    let x = req.args.first().and_then(DtoValue::as_i64).unwrap_or(100) as i32;
                    let y = req.args.get(1).and_then(DtoValue::as_i64).unwrap_or(100) as i32;
                    self.town.move_player(uid, x, y, now_ts).map(|(ret, bcasts)| {
                        peer_pushes.extend(bcasts);
                        ret
                    })
                }
                rpc::CASTLE_GET_INFO
                | rpc::HALL_GET_INFO
                | rpc::MARKET_GET_INFO
                | rpc::WORKSHOP_GET_INFO
                | rpc::SMITHY_GET_INFO => Ok(TownRoomManager::castle_building_info_dto(m)),

                // --- Chat & In-Game GM Slash Commands ---
                rpc::CHAT_SEND_WORLD
                | rpc::CHAT_SEND_GUILD
                | rpc::CHAT_SEND_CROSS_GUILD
                | rpc::CHAT_SEND_CROSS_TEAM
                | rpc::CHAT_SEND_COUNTRY
                | rpc::TM_CHAT_SEND_CROSS_GUILD => {
                    let channel = match m {
                        rpc::CHAT_SEND_GUILD => ChatChannel::Guild,
                        rpc::CHAT_SEND_CROSS_GUILD => ChatChannel::CrossGuild,
                        rpc::CHAT_SEND_CROSS_TEAM => ChatChannel::CrossTeam,
                        rpc::CHAT_SEND_COUNTRY => ChatChannel::Country,
                        rpc::TM_CHAT_SEND_CROSS_GUILD => ChatChannel::TimeMachine,
                        _ if is_tm => ChatChannel::TimeMachine,
                        _ => ChatChannel::World,
                    };
                    let msg_type = req
                        .args
                        .first()
                        .and_then(DtoValue::as_i64)
                        .map_or(ChatMsgType::Normal, ChatMsgType::from_i64);
                    let text = req
                        .args
                        .get(1)
                        .and_then(DtoValue::as_str)
                        .or_else(|| req.args.first().and_then(DtoValue::as_str))
                        .unwrap_or("");
                    let online_uids: Vec<u64> = self.players.keys().copied().collect();
                    let mut sender = self.players.remove(&uid).unwrap();
                    let (ret, sp, pp) = self.chat.send_channel_message(
                        channel,
                        msg_type,
                        text,
                        &mut sender,
                        &online_uids,
                        &mut self.mail_center,
                        &mut self.town,
                        now_ts,
                    );
                    self.players.insert(uid, sender);
                    client_pushes.extend(sp);
                    peer_pushes.extend(pp);
                    Ok(ret)
                }
                rpc::CHAT_SEND_PERSONAL => {
                    let target_name = req.args.first().and_then(DtoValue::as_str).unwrap_or("");
                    let msg_type = req
                        .args
                        .get(1)
                        .and_then(DtoValue::as_i64)
                        .map_or(ChatMsgType::Normal, ChatMsgType::from_i64);
                    let text = req.args.get(2).and_then(DtoValue::as_str).unwrap_or("");

                    if text.trim_start().starts_with('/') && self.players[&uid].gm_enabled {
                        let online_uids: Vec<u64> = self.players.keys().copied().collect();
                        let mut sender = self.players.remove(&uid).unwrap();
                        let (ret, sp, pp) = self.chat.send_channel_message(
                            ChatChannel::Private,
                            msg_type,
                            text,
                            &mut sender,
                            &online_uids,
                            &mut self.mail_center,
                            &mut self.town,
                            now_ts,
                        );
                        self.players.insert(uid, sender);
                        client_pushes.extend(sp);
                        peer_pushes.extend(pp);
                        Ok(ret)
                    } else {
                        let target_lookup = self
                            .players
                            .values()
                            .find(|p| p.uname == target_name)
                            .map(|p| (p.uid, p.uname.clone(), p.quality));
                        let sender = &self.players[&uid];
                        let target_ref = target_lookup
                            .as_ref()
                            .map(|(tuid, tname, tq)| (tuid, tname.as_str(), *tq));
                        let (ret, sp, pp) = self.chat.send_personal_message(
                            sender,
                            target_ref,
                            msg_type,
                            text,
                            now_ts,
                        );
                        client_pushes.extend(sp);
                        peer_pushes.extend(pp);
                        Ok(ret)
                    }
                }
                rpc::CHAT_GET_WORLD_LAST => Ok(self.chat.get_channel_last_msgs(ChatChannel::World)),
                rpc::CHAT_GET_GUILD_LAST => Ok(self.chat.get_channel_last_msgs(ChatChannel::Guild)),
                rpc::CHAT_GET_CROSS_GUILD_LAST => {
                    Ok(self.chat.get_channel_last_msgs(ChatChannel::CrossGuild))
                }
                rpc::CHAT_GET_CROSS_TEAM_LAST => {
                    Ok(self.chat.get_channel_last_msgs(ChatChannel::CrossTeam))
                }
                rpc::CHAT_GET_COUNTRY_LAST => {
                    Ok(self.chat.get_channel_last_msgs(ChatChannel::Country))
                }
                rpc::TM_CHAT_GET_CROSS_GUILD_LAST => {
                    Ok(self.chat.get_channel_last_msgs(ChatChannel::TimeMachine))
                }
                rpc::CHAT_GET_PERSONAL_LAST => Ok(self.chat.get_personal_last_msgs(uid)),
                rpc::CHAT_GET_LAST_MAIN_POP => {
                    let offset = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0) as usize;
                    let limit = req.args.get(1).and_then(DtoValue::as_u64).unwrap_or(20) as usize;
                    let raffle_pop = self.raffle.get_last_main_pop(offset, limit);
                    if raffle_pop.as_array().is_some_and(|a| !a.is_empty()) {
                        Ok(raffle_pop)
                    } else {
                        Ok(self.chat.get_last_main_pop(offset, limit))
                    }
                }

                // --- Mail & Reward Center (Base & `tmMail.*` / `tmReward.*`) ---
                rpc::MAIL_GET_LIST | "mail.getMailList" => Ok(self.mail_center.get_mail_list(uid)),
                "tmMail.getMailBoxList" | "tmMail.getMailList" => {
                    let tm_list = self.mail_center.get_tm_mail_list(uid);
                    if tm_list.as_array().is_some_and(|a| !a.is_empty()) {
                        Ok(tm_list)
                    } else {
                        Ok(self.mail_center.get_mail_list(uid))
                    }
                }
                rpc::MAIL_READ_ALL => Ok(self.mail_center.read_all_mail(uid)),
                "tmMail.readAllMail" => Ok(self.mail_center.read_all_tm_mail(uid)),
                rpc::MAIL_SEND => {
                    let target_uid = req.args.first().and_then(DtoValue::as_u64).unwrap_or(uid);
                    let subject = req.args.get(1).and_then(DtoValue::as_str).unwrap_or("Mail");
                    let content = req.args.get(2).and_then(DtoValue::as_str).unwrap_or("");
                    let (ret, pkt) = self.mail_center.send_player_mail(
                        &self.players[&uid],
                        target_uid,
                        subject,
                        content,
                        now_ts,
                    );
                    if target_uid == uid {
                        client_pushes.push(pkt);
                    } else {
                        peer_pushes.push((target_uid, pkt));
                    }
                    Ok(ret)
                }
                "tmMail.sendMail" => {
                    let target_uid = req.args.first().and_then(DtoValue::as_u64).unwrap_or(uid);
                    let subject = req.args.get(1).and_then(DtoValue::as_str).unwrap_or("Mail");
                    let content = req.args.get(2).and_then(DtoValue::as_str).unwrap_or("");
                    let (ret, pkt) = self.mail_center.send_tm_player_mail(
                        &self.players[&uid],
                        target_uid,
                        subject,
                        content,
                        now_ts,
                    );
                    if target_uid == uid {
                        client_pushes.push(pkt);
                    } else {
                        peer_pushes.push((target_uid, pkt));
                    }
                    Ok(ret)
                }
                rpc::MAIL_DRAW_ITEMS | "mail.fetchItem" | "tmMail.drawMailItems" | "tmMail.fetchItem" => {
                    let mid = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.mail_center
                        .draw_mail_items(player, mid, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::MAIL_DRAW_ALL_ITEMS
                | "mail.fetchAllItems"
                | "tmMail.drawAllMailItems"
                | "tmMail.fetchAllItems" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.mail_center
                        .draw_all_mail_items(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::REWARD_GET_LIST => {
                    let offset = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0) as usize;
                    let limit = req.args.get(1).and_then(DtoValue::as_u64).unwrap_or(100) as usize;
                    Ok(self.mail_center.get_reward_list(uid, offset, limit, now_ts))
                }
                "tmReward.getRewardList" => {
                    let offset = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0) as usize;
                    let limit = req.args.get(1).and_then(DtoValue::as_u64).unwrap_or(100) as usize;
                    let tm_res = self.mail_center.get_tm_reward_list(uid, offset, limit, now_ts);
                    let has_tm = tm_res
                        .get("list")
                        .and_then(DtoValue::as_array)
                        .is_some_and(|a| !a.is_empty());
                    if has_tm {
                        Ok(tm_res)
                    } else {
                        Ok(self.mail_center.get_reward_list(uid, offset, limit, now_ts))
                    }
                }
                rpc::REWARD_RECEIVE | "tmReward.receiveReward" => {
                    let rid = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.mail_center
                        .receive_reward(player, rid, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::REWARD_RECEIVE_BY_RID_ARR => {
                    let rids: Vec<u64> = req
                        .args
                        .first()
                        .and_then(DtoValue::as_array)
                        .map(|arr| arr.iter().filter_map(DtoValue::as_u64).collect())
                        .unwrap_or_default();
                    let player = self.players.get_mut(&uid).unwrap();
                    self.mail_center
                        .receive_by_rid_arr(player, &rids, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "tmReward.receiveByRidArr" => {
                    let rids: Vec<u64> = req
                        .args
                        .first()
                        .and_then(DtoValue::as_array)
                        .map(|arr| arr.iter().filter_map(DtoValue::as_u64).collect())
                        .unwrap_or_default();
                    let player = self.players.get_mut(&uid).unwrap();
                    self.mail_center
                        .receive_tm_by_rid_arr(player, &rids, now_ts)
                        .or_else(|_| self.mail_center.receive_by_rid_arr(player, &rids, now_ts))
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::REWARD_GIFT_BY_CODE | "tmReward.giftByCode" => {
                    let code = req.args.first().and_then(DtoValue::as_str).unwrap_or("");
                    let player = self.players.get_mut(&uid).unwrap();
                    self.mail_center
                        .redeem_gift_code(player, code, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Shops (`shop.*`, `tmShop.*`, `timeMachineTrader.*`, `babeltowerShop.*`, `limitShop.*`, `scoreShop.*`, `weekendShop.*`) ---
                rpc::SHOP_GET_INFO => {
                    let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    Ok(self.shop.get_info_ext(uid, shop_type, is_tm, now_ts))
                }
                rpc::SHOP_BUY => {
                    let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let goods_key = dto_to_key_string(req.args.get(1));
                    let idx = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1);
                    let id = req.args.get(3).and_then(DtoValue::as_u32).unwrap_or(0);
                    let buy_num = req.args.get(4).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self
                        .shop
                        .buy_ext(player, shop_type, is_tm, &goods_key, idx, id, buy_num, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 4, 1, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::SHOP_REFRESH => {
                    let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.shop
                        .refresh_ext(player, shop_type, is_tm, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SHOP_MULTI_REFRESH => {
                    let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.shop
                        .multi_refresh(player, shop_type, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SHOP_QUICK_BUY => {
                    let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let items = dto_to_u32_vec(req.args.get(1));
                    let heroes = dto_to_u32_vec(req.args.get(2));
                    let refresh_mode = req.args.get(3).and_then(DtoValue::as_u32).unwrap_or(1);
                    let gold_limit = req.args.get(4).and_then(DtoValue::as_u64).unwrap_or(500);
                    let max_refresh = req.args.get(5).and_then(DtoValue::as_u32).unwrap_or(5);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.shop
                        .quick_buy(
                            player,
                            shop_type,
                            &items,
                            &heroes,
                            refresh_mode,
                            gold_limit,
                            max_refresh,
                            now_ts,
                        )
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SHOP_AUTO_BUY => {
                    let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let mut quick_tb = Vec::new();
                    if let Some(DtoValue::Array(arr)) = req.args.get(1) {
                        for entry in arr {
                            if let Some(sub) = entry.as_array() {
                                let g = sub.first().and_then(DtoValue::as_u32).unwrap_or(1);
                                let d = sub.get(1).and_then(DtoValue::as_u32).unwrap_or(0);
                                let n = sub.get(2).and_then(DtoValue::as_u32).unwrap_or(1);
                                quick_tb.push((g, d, n));
                            }
                        }
                    }
                    let gold_limit = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1_000);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.shop
                        .normal_shop_auto_buy(player, shop_type, &quick_tb, gold_limit, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "timeMachineTrader.getInfo" | "traderShop.getInfo" => {
                    Ok(self.trader_shop.get_info(uid, now_ts))
                }
                "timeMachineTrader.buyFreeGift"
                | "traderShop.buyFreeGift"
                | "traderShop.buy" => {
                    let gift_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.trader_shop
                        .buy_free_gift(player, gift_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "timeMachineTrader.recvFundReward" | "traderShop.recvFundReward" => {
                    let fund_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.trader_shop
                        .recv_fund_reward(player, fund_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltowerShop.getShopInfo" | "babeltowerShop.getInfo" => {
                    Ok(self.babel_shop.get_shop_info(uid))
                }
                "babeltowerShop.buy" | "babeltower.buy" => {
                    // `babeltowerShop.buy` passes `[layer, floorIndex, goodsId, num]` or `[goodsId, num]`
                    let (goods_id, num) = if req.args.len() >= 4 {
                        (
                            req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1001),
                            req.args.get(3).and_then(DtoValue::as_u32).unwrap_or(1),
                        )
                    } else {
                        (
                            req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001),
                            req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1),
                        )
                    };
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_shop
                        .buy_floor_item(player, goods_id, num, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltowerShop.buyGoods" => {
                    let mut buy_tb = Vec::new();
                    if let Some(DtoValue::Map(map)) = req.args.first() {
                        for (k, v) in map {
                            if let (Ok(gid), Some(qty)) = (k.parse::<u32>(), v.as_u32()) {
                                if qty > 0 {
                                    buy_tb.push((gid, qty));
                                }
                            }
                        }
                    } else if let Some(gid) = req.args.first().and_then(DtoValue::as_u32) {
                        let qty = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                        buy_tb.push((gid, qty));
                    }
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_shop
                        .buy_sweep_goods(player, &buy_tb, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "limitShop.getInfo"
                | "discount.getInfo"
                | "mergeDiscount.getInfo"
                | "dailyDiscount.getInfo"
                | "activityBusinessman.getInfo" => Ok(self.activity_shop.get_info(uid, "limit")),
                "weekendShop.getInfo" | "weekGift.getInfo" | "activityWeekGift.getInfo" => {
                    Ok(self.activity_shop.get_info(uid, "weekend"))
                }
                "scoreShop.getInfo"
                | "activityActiveTaskShop.getInfo"
                | "activityAnniversary6Shop.getInfo" => {
                    Ok(self.activity_shop.get_info(uid, "score"))
                }
                "limitShop.buy"
                | "discount.buy"
                | "mergeDiscount.buy"
                | "dailyDiscount.buy"
                | "activityBusinessman.buy" => {
                    let pack_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let qty = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.activity_shop
                        .buy_pack(player, "limit", pack_id, qty, 100, 10, 16012, 1, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "weekendShop.buy" | "weekGift.buy" | "activityWeekGift.buy" => {
                    let pack_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let qty = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.activity_shop
                        .buy_pack(player, "weekend", pack_id, qty, 150, 5, 22102, 2, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "scoreShop.buy"
                | "scoreShop.exchange"
                | "activityActiveTaskShop.buy"
                | "activityAnniversary6Shop.buy"
                | "activity8Years.exchangeYearShop"
                | "activity9Years.exchangeShop"
                | "activity9Years.cakeExchange"
                | "happyLottery.happyLotteryShop"
                | "activitySaveGoldExchange.exchangeReward"
                | "activityRareTreasure.exchangeByDailyScore"
                | "activityDoubleGift.exchange" => {
                    let pack_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let qty = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.activity_shop
                        .buy_pack(player, "score", pack_id, qty, 50, 20, 17002, 5, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Tavern Raffle & Recruitment ---
                rpc::RAFFLE_GET_INFO => Ok(self.raffle.get_info(uid, now_ts)),
                rpc::RAFFLE_SINGLE | "raffle.raffle" => {
                    let pool_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2);
                    let pool = RafflePool::from_u32(pool_id).unwrap_or(RafflePool::Divine);
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.raffle.draw_standard(player, pool, 1, true, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 2, 1, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::RAFFLE_AUTO => {
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.raffle.draw_auto_warrior(player, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 2, 1, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::RAFFLE_MULTI => {
                    let pool_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2);
                    let pool = RafflePool::from_u32(pool_id).unwrap_or(RafflePool::Divine);
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.raffle.draw_standard(player, pool, 10, false, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 2, 10, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::RAFFLE_HOT => {
                    let (pos, count) = parse_pos_and_count(req, 1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .draw_hot_or_red(player, RafflePool::HotLimit, pos, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::RAFFLE_MULTI_HOT => {
                    let (pos, _) = parse_pos_and_count(req, 10);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .draw_hot_or_red(player, RafflePool::HotLimit, pos, 10, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::RAFFLE_RED => {
                    let (pos, count) = parse_pos_and_count(req, 1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .draw_hot_or_red(player, RafflePool::RedHero, pos, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::RAFFLE_BUY_TOKEN | "raffle.buyRaffleToken" => {
                    let a0 = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    let res = if req.args.len() >= 2 && a0 >= 10_000 {
                        let qty = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                        self.raffle.buy_token_by_tid(player, a0, qty, now_ts)
                    } else {
                        self.raffle.buy_token(player, a0, now_ts)
                    };
                    res.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::RAFFLE_RECEIVE_LUCK => {
                    let luck_or_idx = req.args.first().and_then(DtoValue::as_u32).unwrap_or(500);
                    let choice_idx = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(0);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .receive_luck_box(player, RafflePool::Divine, luck_or_idx, choice_idx, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::RAFFLE_RECEIVE_LUCK_HOT => {
                    let luck_or_idx = req.args.first().and_then(DtoValue::as_u32).unwrap_or(250);
                    let hot_pos = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(0);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .receive_luck_box(player, RafflePool::HotLimit, luck_or_idx, hot_pos, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::RAFFLE_RECEIVE_LUCK_RED => {
                    let luck_or_idx = req.args.first().and_then(DtoValue::as_u32).unwrap_or(250);
                    let red_pos = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(0);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .receive_luck_box(player, RafflePool::RedHero, luck_or_idx, red_pos, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::RAFFLE_REFRESH_HOT => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .refresh_hot_by_hand(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::RECRUITMENT_GET_INFO => Ok(self.raffle.get_recruitment_info(uid)),
                rpc::RECRUITMENT_RECEIVE_REWARD | "recruitment.receiveReward" => {
                    let tier_idx = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.raffle
                        .receive_recruitment_reward(player, tier_idx, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- PvE Campaign Copy, TimeMachineCopy & Battle Record ---
                rpc::COPY_GET_LIST => {
                    let copy_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    Ok(self.copy.get_list(uid, copy_type))
                }
                "timeMachineCopy.getList" => {
                    let copy_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    Ok(self.copy.get_list(uid, copy_type))
                }
                rpc::COPY_GET_COPY => {
                    let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                    Ok(self.copy.get_chapter(uid, chapter_id))
                }
                "timeMachineCopy.getCopy" => {
                    let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                    Ok(self.copy.get_chapter(uid, chapter_id))
                }
                rpc::USER_BEFORE_LOGIN_BATTLE => {
                    self.copy
                        .attack_stage(&self.players[&uid], 1001, 100101, true)
                }
                rpc::COPY_ATTACK => {
                    let chapter_id = match req.args.first() {
                        Some(v) => v.as_i64().filter(|&n| n > 0).map_or(0, |n| n as u32),
                        None => 1001,
                    };
                    let copy_id = match req.args.get(1) {
                        Some(v) => v.as_i64().filter(|&n| n > 0).map_or(0, |n| n as u32),
                        None => 100101,
                    };
                    self.copy
                        .attack_stage(&self.players[&uid], chapter_id, copy_id, false)
                }
                "timeMachineCopy.attack" => {
                    let (chapter_id, copy_id) = if req.args.len() >= 2 {
                        (
                            req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001),
                            req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100_101),
                        )
                    } else {
                        let cid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(100_101);
                        ((cid / 100).max(1001), cid)
                    };
                    self.copy
                        .attack_tm_stage(&self.players[&uid], chapter_id, copy_id)
                }
                rpc::COPY_PASS => {
                    let chapter_id = match req.args.first() {
                        Some(v) => v.as_i64().filter(|&n| n > 0).map_or(0, |n| n as u32),
                        None => 1001,
                    };
                    let copy_id = match req.args.get(1) {
                        Some(v) => v.as_i64().filter(|&n| n > 0).map_or(0, |n| n as u32),
                        None => 100101,
                    };
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.copy.pass_stage(player, chapter_id, copy_id, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 1, 1, 1);
                        self.achievements.record_achievement(uid, 100_001, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                "timeMachineCopy.pass" => {
                    let (chapter_id, copy_id) = if req.args.len() >= 2 {
                        (
                            req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001),
                            req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100_101),
                        )
                    } else {
                        let cid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(100_101);
                        ((cid / 100).max(1001), cid)
                    };
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.copy.pass_tm_stage(player, chapter_id, copy_id, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 1, 1, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::COPY_FAIL
                | "copy.fail"
                | "timeMachineCopy.fail"
                | "timeMachineCopy.battleFail" => {
                    let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                    let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                    Ok(self.copy.battle_fail(uid, chapter_id, copy_id))
                }
                rpc::COPY_SWEEP | "timeMachineCopy.sweep" => {
                    let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                    let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                    let num = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.copy.sweep_stage(player, chapter_id, copy_id, num, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 1, num, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::COPY_RESET_ATK
                | "copy.resetAtkNum"
                | rpc::COPY_ONE_KEY_RESET_ATK
                | "timeMachineCopy.resetAtk"
                | "timeMachineCopy.resetAtkNum" => {
                    let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                    let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.copy
                        .reset_attack_count(player, chapter_id, copy_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::COPY_FETCH_PRIZE
                | rpc::COPY_GET_PRIZE
                | "timeMachineCopy.fetchPrize"
                | "timeMachineCopy.getPrize" => {
                    let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                    let box_idx = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(0);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.copy
                        .fetch_chapter_prize(player, chapter_id, box_idx, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::COPY_FETCH_BOX_REWARD | "timeMachineCopy.fetchBoxReward" => {
                    let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                    let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.copy
                        .fetch_stage_box_reward(player, chapter_id, copy_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::COPY_FETCH_FRESHER_REWARD => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.copy
                        .fetch_fresher_reward(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::COPY_GET_FAMOUS_PASS_TIMES => Ok(self.copy.get_famous_pass_times(uid)),
                rpc::COPY_GET_STRONGH_RECORD => Ok(DtoValue::Array(Vec::new())),
                rpc::BATTLE_GET_RECORD => {
                    let brid = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0);
                    self.copy
                        .get_record_blob(brid)
                        .map(DtoValue::Bytes)
                        .ok_or("battle_record_not_found")
                }

                // --- Elite Copy (`ecopy.*` / `copy_elite.*`) ---
                "ecopy.getEliteCopyInfo"
                | "ecopy.getInfo"
                | "copy_elite.getEliteCopyInfo"
                | "copy_elite.getInfo" => Ok(EliteCopyEngine::get_elite_copy_info(&mut self.copy, uid)),
                "ecopy.getCopy" | "copy_elite.getCopy" => {
                    let ch = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2001);
                    Ok(EliteCopyEngine::get_chapter(&mut self.copy, uid, ch))
                }
                "ecopy.attack" | "copy_elite.attack" => {
                    let ch = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2001);
                    let stage = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(200_101);
                    EliteCopyEngine::attack(&mut self.copy, &self.players[&uid], ch, stage)
                }
                "ecopy.pass" | "copy_elite.pass" => {
                    let ch = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2001);
                    let stage = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(200_101);
                    let player = self.players.get_mut(&uid).unwrap();
                    EliteCopyEngine::pass(&mut self.copy, player, ch, stage, now_ts).map(
                        |(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        },
                    )
                }
                "ecopy.sweep" | "copy_elite.sweep" => {
                    let ch = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2001);
                    let stage = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(200_101);
                    let num = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    EliteCopyEngine::sweep(&mut self.copy, player, ch, stage, num, now_ts).map(
                        |(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        },
                    )
                }
                "ecopy.reset"
                | "ecopy.resetAtkNum"
                | "copy_elite.reset"
                | "copy_elite.resetAtkNum" => {
                    let ch = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2001);
                    let stage = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(200_101);
                    let player = self.players.get_mut(&uid).unwrap();
                    EliteCopyEngine::reset(&mut self.copy, player, ch, stage, now_ts).map(
                        |(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        },
                    )
                }

                // --- Daily Copy (`copy_daily.*` / `acopy.*`) ---
                "copy_daily.getDailyCopyInfo"
                | "copy_daily.getInfo"
                | "copyDaily.getInfo"
                | "acopy.getCopyList"
                | "acopy.getInfo" => {
                    let cid = req.args.first().and_then(DtoValue::as_u32);
                    Ok(self.daily_copy.get_info(uid, cid))
                }
                "copy_daily.attack" | "copyDaily.attack" | "acopy.attack" => {
                    let cid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let pid = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(101);
                    self.daily_copy
                        .attack(&self.players[&uid], cid, pid, &mut self.copy)
                }
                "copy_daily.pass" | "copyDaily.pass" | "acopy.pass" => {
                    let cid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let pid = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(101);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.daily_copy
                        .pass(player, cid, pid, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "copy_daily.sweep" | "copyDaily.sweep" | "acopy.sweep" => {
                    let cid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let pid = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(101);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.daily_copy
                        .sweep(player, cid, pid, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "copy_daily.buyAtkNum"
                | "copy_daily.addNum"
                | "copyDaily.buyAtkNum"
                | "acopy.buyAtkNum" => {
                    let cid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let count = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.daily_copy
                        .buy_atk_num(player, cid, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Babel Tower (`babeltower.*` — all 12 RPCs) ---
                "babeltower.getBabeltowerInfo" | "babeltower.getInfo" => {
                    Ok(self.babel_tower.get_babeltower_info(uid, now_ts))
                }
                "babeltower.getBagInfo" => Ok(self.babel_tower.get_bag_info(uid)),
                "babeltower.setIdx" => {
                    let floor = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let idx = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    self.babel_tower.set_idx(uid, floor, idx)
                }
                "babeltower.attack" | "babeltower.enterLevel" => {
                    let floor = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let idx = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_tower
                        .attack(player, floor, idx, &mut self.copy, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltower.passFloor" | "babeltower.pass" => {
                    let floor = req.args.first().and_then(DtoValue::as_u32).unwrap_or(0);
                    self.babel_tower.pass_floor(uid, floor)
                }
                "babeltower.answer" => {
                    let floor = req.args.first().and_then(DtoValue::as_u32).unwrap_or(0);
                    let idx = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(2);
                    let choice = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_tower
                        .answer(player, floor, idx, choice, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltower.specialEvent" => {
                    let floor = req.args.first().and_then(DtoValue::as_u32).unwrap_or(0);
                    let idx = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(4);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_tower
                        .special_event(player, floor, idx, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltower.sweep" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_tower
                        .sweep(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltower.reset" => Ok(self.babel_tower.reset(uid, now_ts)),
                "babeltower.buyAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_tower
                        .buy_atk_num(player, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltower.receiveReward" => {
                    let reward_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let target_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_tower
                        .receive_reward(player, reward_type, target_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "babeltower.autoReceiveReward" => {
                    let reward_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.babel_tower
                        .auto_receive_reward(player, reward_type, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Trial Tower & Purgatory (`dota.*` & `purgatory.*`) ---
                "dota.getInfo" | "dota.getDotaInfo" | "purgatory.getInfo" | "purgatory.getPurgatoryInfo" => {
                    Ok(self.dota_tower.get_info(uid))
                }
                "dota.attack"
                | "dota.attackElite"
                | "purgatory.attack"
                | "purgatory.attackPurgatory" => {
                    let layer = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    self.dota_tower
                        .attack(&self.players[&uid], layer, &mut self.copy)
                }
                "dota.pass" | "dota.battleResult" => {
                    let layer = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let is_win = req.args.get(1).and_then(DtoValue::as_bool).unwrap_or(true);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower
                        .pass(player, 0, layer, is_win, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "dota.passElite" => {
                    let layer = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower
                        .pass(player, 1, layer, true, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "purgatory.pass" | "purgatory.passPurgatory" => {
                    let layer = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower
                        .pass(player, 2, layer, true, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "dota.openBox" | "purgatory.openBox" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower.open_box(player, now_ts).map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                "dota.sweep" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower.sweep(player, 0, now_ts).map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                "dota.sweepElite" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower.sweep(player, 1, now_ts).map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                "purgatory.sweep" | "purgatory.sweepPurgatory" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower.sweep(player, 2, now_ts).map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                "dota.reset" | "purgatory.reset" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower.reset(player, now_ts).map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                "dota.buyAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower
                        .buy_atk_num(player, 0, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "dota.buyEliteAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower
                        .buy_atk_num(player, 1, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "purgatory.buyAtkNum" | "purgatory.buyTime" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.dota_tower
                        .buy_atk_num(player, 2, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Awake Copy (`awakeCopy.*`) ---
                "awakeCopy.getAwakeCopyInfo" | "awakeCopy.getInfo" => {
                    Ok(self.awake_copy.get_info(uid))
                }
                "awakeCopy.getRoomList" | "team.getRoomList" => {
                    Ok(AwakeCopyEngine::get_room_list())
                }
                "awakeCopy.quickBattle" | "awakeCopy.attack" => {
                    let copy_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.awake_copy
                        .quick_battle(player, copy_id, &mut self.copy, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "awakeCopy.buyNum" | "awakeCopy.buyAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.awake_copy
                        .buy_num(player, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Rebel Army, World Boss & Fiend (`rebel.*`, `boss.*`, `fiend.*`) ---
                "rebel.getRebelInfo" | "rebel.getInfo" => {
                    Ok(self.world_boss_rebel.get_rebel_info(&self.players[&uid], now_ts))
                }
                "rebel.enterRebel" | "rebel.enter" => {
                    Ok(self.world_boss_rebel.enter_rebel(&self.players[&uid]))
                }
                "rebel.attackRebel" | "rebel.attack" => {
                    let rebel_id = req.args.first().and_then(DtoValue::as_u64).unwrap_or(1);
                    let mode = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.world_boss_rebel
                        .attack_rebel(player, rebel_id, mode, &mut self.copy, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "rebel.buyRebelAtkNum" | "rebel.buyAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.world_boss_rebel
                        .buy_rebel_atk_num(player, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "boss.getBossData" | "boss.getInfo" => {
                    Ok(self.world_boss_rebel.get_boss_data(&self.players[&uid], now_ts))
                }
                "boss.attack" | "boss.attackBoss" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.world_boss_rebel
                        .attack_boss(player, &mut self.copy, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "boss.getFiendInfo" | "fiend.getFiendInfo" | "fiend.enter" => {
                    Ok(self.world_boss_rebel.get_fiend_info(now_ts))
                }

                // --- Territory Exploration (`discover.*`) ---
                "discover.getRewardCity" | "discover.getInfo" => {
                    Ok(self.discover.get_reward_city(uid))
                }
                "discover.getDiscover" => {
                    let city_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    Ok(self.discover.get_discover(uid, city_id))
                }
                "discover.attack" => {
                    let city_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    self.discover
                        .attack(&self.players[&uid], city_id, &mut self.copy)
                }
                "discover.pass" => {
                    let city_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.discover
                        .pass(player, city_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "discover.patrol" => {
                    let city_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    self.discover.patrol(uid, city_id, now_ts)
                }
                "discover.receivePatrolReward" => {
                    let city_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.discover
                        .receive_patrol_reward(player, city_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Devil Copy & Soaring Copy (`devilCopy.*` & `soaringCopy.*`) ---
                "devilCopy.getLoginInfo" => Ok(self.devil_copy.get_login_info(uid)),
                "devilCopy.getInfo" => Ok(self.devil_copy.get_info(uid)),
                "devilCopy.getCopy" => {
                    let sid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    Ok(self.devil_copy.get_copy(sid))
                }
                "devilCopy.attack" => {
                    let sid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    self.devil_copy
                        .attack(&self.players[&uid], sid, &mut self.copy)
                }
                "devilCopy.pass" => {
                    let sid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.devil_copy
                        .pass(player, sid, &mut self.copy, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "devilCopy.buyAttackTimes" | "devilCopy.buyAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.devil_copy
                        .buy_attack_times(player, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                "soaringCopy.getLoginInfo" => Ok(self.soaring_copy.get_login_info(uid)),
                "soaringCopy.getInfo" => Ok(self.soaring_copy.get_info(uid)),
                "soaringCopy.getCopy" => {
                    let sid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    Ok(self.soaring_copy.get_copy(sid))
                }
                "soaringCopy.attack" => {
                    let sid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    self.soaring_copy
                        .attack(&self.players[&uid], sid, &mut self.copy)
                }
                "soaringCopy.pass" => {
                    let sid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.soaring_copy
                        .pass(player, sid, &mut self.copy, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "soaringCopy.buyAttackTimes" | "soaringCopy.buyAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.soaring_copy
                        .buy_attack_times(player, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Arena (`arena.*`, `timeMachineArena.*`, `tmArena.*`) ---
                rpc::ARENA_ENTER | rpc::ARENA_GET_BASE_DATA | rpc::ARENA_GET_ARENA_INFO => {
                    Ok(self.arena.enter_arena(&self.players[&uid]))
                }
                rpc::ARENA_GET_RANK_LIST => Ok(self.arena.get_rank_list()),
                rpc::ARENA_LEAVE | "tmArena.leaveArena" | "timeMachineArena.leaveArena" => {
                    Ok(DtoValue::map([("ret", DtoValue::from("ok"))]))
                }
                rpc::ARENA_CHALLENGE => {
                    let target_pos = req
                        .args
                        .get(1)
                        .or_else(|| req.args.first())
                        .and_then(DtoValue::as_u32)
                        .unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self
                        .arena
                        .challenge(player, target_pos, &mut self.copy, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 3, 1, 1);
                    }
                    out.map(|(ret, sp, pp)| {
                        client_pushes.extend(sp);
                        peer_pushes.extend(pp);
                        ret
                    })
                }
                rpc::ARENA_QUICK_CHALLENGE
                | rpc::ARENA_SWEEP
                | "tmArena.quickChallenge"
                | "tmArena.sweep" => {
                    let my_pos = self.arena.ensure_registered(&self.players[&uid]);
                    let a0 = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let (target_pos, count) = if a0 == my_pos && req.args.len() >= 2 {
                        let tp = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                        let cnt = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(5);
                        (tp, cnt)
                    } else {
                        let cnt = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(5);
                        (a0, cnt)
                    };
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.arena.sweep(player, target_pos, count, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 3, count, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                rpc::ARENA_BUY_TIMES
                | "arena.buyAtkNum"
                | "tmArena.buyChallengeTimes"
                | "tmArena.buyAtkNum" => {
                    let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.arena
                        .buy_atk_num(player, count, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::ARENA_FETCH_REWARD
                | "arena.receiveRankReward"
                | "tmArena.fetchReward"
                | "tmArena.receiveRankReward" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.arena
                        .receive_rank_reward(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::ARENA_FETCH_NUM_REWARD | "tmArena.fetchChallengeNumReward" => {
                    let reward_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.arena
                        .fetch_challenge_num_reward(player, reward_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "timeMachineArena.getArenaInfo"
                | "timeMachineArena.getBaseData"
                | "timeMachineArena.enterArena"
                | "tmArena.getArenaInfo"
                | "tmArena.getBaseData"
                | "tmArena.enterArena" => Ok(self.arena.tm_get_info(&self.players[&uid])),
                "timeMachineArena.challenge" | "tmArena.challenge" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    let out = self.arena.tm_challenge(player, &mut self.copy, now_ts);
                    if out.is_ok() {
                        self.daily_task.record_progress(uid, 3, 1, 1);
                    }
                    out.map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
                }
                "timeMachineArena.recTodayReward" | "tmArena.recTodayReward" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.arena
                        .tm_rec_today_reward(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "timeMachineArena.recChallengeReward" | "tmArena.recChallengeReward" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.arena
                        .tm_rec_challenge_reward(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }

                // --- Sign, SuperCheckin, Online, DailyTask, MainTask, Achievements & EatChicken ---
                rpc::SIGN_GET_INFO | "sign.getSignInfo" | "sign.getNormalInfo" => {
                    let rech_nb = self
                        .players
                        .get(&uid)
                        .map_or(0, |p| p.gold_recharge_accum_all);
                    Ok(self
                        .sign_online
                        .get_sign_info_with_recharge(uid, rech_nb, now_ts))
                }
                rpc::SIGN_TODAY | rpc::SIGN_GAIN_NORMAL => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.sign_online
                        .sign_today(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SIGN_GAIN_ACC => {
                    let req_days = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.sign_online
                        .gain_acc_sign_reward(player, req_days, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SIGN_GAIN_VIP => {
                    let req_vip = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.sign_online
                        .gain_vip_sign_reward(player, req_vip, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SIGN_GAIN_RECHARGE => {
                    let tier = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.sign_online
                        .gain_recharge_sign_reward(player, tier, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SUPER_CHECKIN_GET_INFO | "superSign.getInfo" => {
                    Ok(self.sign_online.get_super_checkin_info(uid, now_ts))
                }
                rpc::SUPER_CHECKIN_SIGN | "superSign.sign" => {
                    let default_day = (((now_ts / 86_400) % 30) + 1) as u32;
                    let day = req
                        .args
                        .first()
                        .and_then(DtoValue::as_u32)
                        .unwrap_or(default_day);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.sign_online
                        .super_checkin_sign(player, day, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::SUPER_CHECKIN_FETCH | "superSign.receiveAccReward" | "superCheckin.fetch" | "superCheckin.gainReward" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.sign_online
                        .super_checkin_fetch_reward(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::ONLINE_GET_INFO | rpc::OPEN_ONLINE_GET_INFO | "online.getOnlineInfo" => {
                    Ok(self.sign_online.get_online_info(&self.players[&uid]))
                }
                rpc::ONLINE_FETCH | rpc::OPEN_ONLINE_RECEIVE | "online.fetchOnlineReward" | "online.recv" => {
                    let tier_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.sign_online
                        .fetch_online_reward(player, tier_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "dailytask.getInfo" | "dailytask.getDailyTaskInfo" => {
                    Ok(self.daily_task.get_info(&self.players[&uid]))
                }
                "dailytask.receiveTask" | "dailytask.complete" | "dailytask.getTaskPrize" => {
                    let task_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.daily_task
                        .receive_task(player, task_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "dailytask.receiveActivePoint" | "dailytask.fetchReward" | "dailytask.receiveActivePrize" => {
                    let score = req.args.first().and_then(DtoValue::as_u32).unwrap_or(20);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.daily_task
                        .receive_active_point(player, score, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "dailytask.receiveAllTask" | "dailytask.oneKeyReceive" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.daily_task
                        .receive_all_task(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "task.getAllTask" | "task.getAllMainTasks" | "task.getInfo" => {
                    Ok(self.daily_task.get_all_main_tasks(uid))
                }
                "task.completeMainTask" | "task.complete" => {
                    let tid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.daily_task
                        .complete_main_task(player, tid, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "achievements.getShowAchievements" | "achievements.getInfo" => {
                    Ok(self.achievements.get_show_achievements(uid))
                }
                "achievements.getPointRewardInfo" | "achievements.getShowAchievementsPointReward" => {
                    Ok(self.achievements.get_point_reward_info(uid))
                }
                "achievements.receiveAchievementPrize" | "achievements.receivePrize" | "achievements.fetchPrize" => {
                    let aid = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.achievements
                        .receive_achievement_prize(player, aid, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "achievements.receiveAchievementPointPrize" | "achievements.receivePointPrize" => {
                    let box_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                    let player = self.players.get_mut(&uid).unwrap();
                    self.achievements
                        .receive_achievement_point_prize(player, box_id, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                "eatChicken.getInfo" => Ok(self.eat_chicken.get_info(uid, now_ts)),
                "eatChicken.eat" | "eatChicken.eatChicken" => {
                    let player = self.players.get_mut(&uid).unwrap();
                    self.eat_chicken
                        .eat_chicken(player, now_ts)
                        .map(|(ret, pushes)| {
                            client_pushes.extend(pushes);
                            ret
                        })
                }
                rpc::ACTIVITY_GET_CONF => Ok(crate::festival_conf::build_activity_conf_dto(now_ts)),

                _ => Err("unsupported_method"),
            }
        };

        if is_tm {
            for pkt in &mut client_pushes {
                rewrite_push_for_tm(pkt);
            }
        }

        let response = match res {
            Ok(ret) => RpcResponse::ok(req, ret, now_ts),
            Err(code) => RpcResponse::error(req, code, now_ts),
        };

        DispatchOutcome {
            response,
            client_pushes,
            peer_pushes,
        }
    }
}
