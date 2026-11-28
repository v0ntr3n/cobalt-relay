//! Unified stateful RPC router (`GameplayEngine`) wiring together Player, Town, Chat/GM,
//! Mail/Reward Center, Shop/Tavern Raffle, PvE Copy/Battle, Arena, and Sign/Online modules.

use std::collections::BTreeMap;

use crate::battle_copy::{ArenaEngine, CopyCampaignEngine, SignAndOnlineService};
use crate::chat_gm::{ChatChannel, ChatMsgType, ChatService};
use crate::mail::MailAndRewardCenter;
use crate::player::PlayerState;
use crate::protocol::{dto_map, dto_ok, rpc, DtoValue, PushPacket, RpcRequest, RpcResponse};
use crate::shop_pub::{RafflePool, ShopEngine, TavernRaffleEngine};
use crate::town::TownRoomManager;

/// Outcome of routing a single client `RpcRequest` through `GameplayEngine`.
#[derive(Debug, Clone)]
pub struct DispatchOutcome {
    pub response: RpcResponse,
    /// Asynchronous pushes queued for the calling client (`user.updateUser`, `bag.update`, `re.chat.getMsg`, etc.).
    pub client_pushes: Vec<PushPacket>,
    /// Asynchronous pushes queued for other connected players `(target_uid, PushPacket)`.
    pub peer_pushes: Vec<(u64, PushPacket)>,
}

/// Complete server-side gameplay coordinator for GServer (`vnsg2`).
#[derive(Debug, Clone)]
pub struct GameplayEngine {
    pub players: BTreeMap<u64, PlayerState>,
    pub town: TownRoomManager,
    pub chat: ChatService,
    pub mail_center: MailAndRewardCenter,
    pub raffle: TavernRaffleEngine,
    pub shop: ShopEngine,
    pub copy: CopyCampaignEngine,
    pub arena: ArenaEngine,
    pub sign_online: SignAndOnlineService,
}

impl Default for GameplayEngine {
    fn default() -> Self {
        Self::new(0x564E_5347_325F_3351)
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
            shop: ShopEngine::new(),
            copy: CopyCampaignEngine::new(),
            arena: ArenaEngine::new(),
            sign_online: SignAndOnlineService::default(),
        }
    }

    pub fn register_player(&mut self, player: PlayerState) {
        self.players.insert(player.uid, player);
    }

    /// Dispatches any supported `vnsg2` gameplay RPC method for `uid`.
    pub fn dispatch(&mut self, uid: u64, req: &RpcRequest, now_ts: u64) -> DispatchOutcome {
        if !self.players.contains_key(&uid) {
            return DispatchOutcome {
                response: RpcResponse::error(req, "user_not_found", now_ts),
                client_pushes: Vec::new(),
                peer_pushes: Vec::new(),
            };
        }

        let mut client_pushes = Vec::new();
        let mut peer_pushes = Vec::new();
        let mut with_pushes = |res: Result<(DtoValue, Vec<PushPacket>), &'static str>| {
            res.map(|(ret, p)| {
                client_pushes.extend(p);
                ret
            })
        };

        let res: Result<DtoValue, &'static str> = match req.method.as_str() {
            // --- User, Bag, Hero & Formation ---
            rpc::USER_GET_USER => Ok(self.players[&uid].to_user_dto()),
            rpc::BAG_INFO => Ok(self.players[&uid].bag.to_bag_info_dto()),
            rpc::HERO_GET_ALL => Ok(self.players[&uid].roster.to_all_heroes_dto()),
            rpc::FORMATION_GET_POSITION_INFO => {
                Ok(self.players[&uid].roster.to_formation_position_info_dto())
            }

            // --- Town, City, RoyalCity & Castle ---
            rpc::CITY_ENTER => self
                .town
                .enter_town(
                    &self.players[&uid],
                    req.arg_u32_or(0, 1),
                    req.arg_i32(1),
                    req.arg_i32(2),
                    now_ts,
                )
                .map(|(ret, bcasts)| {
                    peer_pushes.extend(bcasts);
                    ret
                }),
            rpc::ROYALCITY_ENTER | "royalcity.enter" => self
                .town
                .enter_town(&self.players[&uid], 999, req.arg_i32(0), req.arg_i32(1), now_ts)
                .map(|(ret, bcasts)| {
                    peer_pushes.extend(bcasts);
                    ret
                }),
            rpc::CITY_LEAVE | rpc::ROYALCITY_LEAVE | "royalcity.leave" => {
                let (ret, bcasts) = self.town.leave_town(uid, now_ts);
                peer_pushes.extend(bcasts);
                Ok(ret)
            }
            rpc::CITY_GET_ENTER_LIST => Ok(self.town.get_enter_list(uid)),
            rpc::TOWN_MOVE | rpc::ROYALCITY_MOVE | "royalcity.move" => self
                .town
                .move_player(uid, req.arg_i32_or(0, 100), req.arg_i32_or(1, 100), now_ts)
                .map(|(ret, bcasts)| {
                    peer_pushes.extend(bcasts);
                    ret
                }),
            rpc::CASTLE_GET_INFO
            | rpc::HALL_GET_INFO
            | rpc::MARKET_GET_INFO
            | rpc::WORKSHOP_GET_INFO
            | rpc::SMITHY_GET_INFO => Ok(TownRoomManager::castle_building_info_dto(&req.method)),

            // --- Chat & In-Game GM Slash Commands ---
            rpc::CHAT_SEND_WORLD
            | rpc::CHAT_SEND_GUILD
            | rpc::CHAT_SEND_CROSS_GUILD
            | rpc::CHAT_SEND_CROSS_TEAM
            | rpc::CHAT_SEND_COUNTRY
            | rpc::TM_CHAT_SEND_CROSS_GUILD => {
                let channel = match req.method.as_str() {
                    rpc::CHAT_SEND_GUILD => ChatChannel::Guild,
                    rpc::CHAT_SEND_CROSS_GUILD => ChatChannel::CrossGuild,
                    rpc::CHAT_SEND_CROSS_TEAM => ChatChannel::CrossTeam,
                    rpc::CHAT_SEND_COUNTRY => ChatChannel::Country,
                    rpc::TM_CHAT_SEND_CROSS_GUILD => ChatChannel::TimeMachine,
                    _ => ChatChannel::World,
                };
                let msg_type = req
                    .arg_i64(0)
                    .map_or(ChatMsgType::Normal, ChatMsgType::from_i64);
                let text = req.arg_str(1).or_else(|| req.arg_str(0)).unwrap_or("");
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
                let target_name = req.arg_str_or(0, "");
                let msg_type = req
                    .arg_i64(1)
                    .map_or(ChatMsgType::Normal, ChatMsgType::from_i64);
                let text = req.arg_str_or(2, "");

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
                    let target_ref = target_lookup
                        .as_ref()
                        .map(|(tuid, tname, tq)| (tuid, tname.as_str(), *tq));
                    let (ret, sp, pp) = self.chat.send_personal_message(
                        &self.players[&uid],
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
            rpc::CHAT_GET_COUNTRY_LAST => Ok(self.chat.get_channel_last_msgs(ChatChannel::Country)),
            rpc::TM_CHAT_GET_CROSS_GUILD_LAST => {
                Ok(self.chat.get_channel_last_msgs(ChatChannel::TimeMachine))
            }
            rpc::CHAT_GET_PERSONAL_LAST => Ok(self.chat.get_personal_last_msgs(uid)),

            // --- Mail & Reward Center ---
            rpc::MAIL_GET_LIST | "mail.getMailList" => Ok(self.mail_center.get_mail_list(uid)),
            rpc::MAIL_READ_ALL => Ok(self.mail_center.read_all_mail(uid)),
            rpc::MAIL_SEND => {
                let target_uid = req.arg_u64_or(0, 0);
                let (ret, pkt) = self.mail_center.send_player_mail(
                    &self.players[&uid],
                    target_uid,
                    req.arg_str_or(1, "Mail"),
                    req.arg_str_or(2, ""),
                    now_ts,
                );
                if target_uid == uid {
                    client_pushes.push(pkt);
                } else {
                    peer_pushes.push((target_uid, pkt));
                }
                Ok(ret)
            }
            rpc::REWARD_GET_LIST => Ok(self.mail_center.get_reward_list(
                uid,
                req.arg_usize_or(0, 0),
                req.arg_usize_or(1, 100),
                now_ts,
            )),
            rpc::REWARD_RECEIVE_BY_RID_ARR => with_pushes(
                self.mail_center.receive_by_rid_arr(
                    self.players.get_mut(&uid).unwrap(),
                    &req.arg_u64_vec(0),
                    now_ts,
                ),
            ),
            rpc::REWARD_GIFT_BY_CODE => with_pushes(self.mail_center.redeem_gift_code(
                self.players.get_mut(&uid).unwrap(),
                req.arg_str_or(0, ""),
                now_ts,
            )),

            // --- Shop & Tavern Raffle ---
            rpc::SHOP_GET_INFO => Ok(self.shop.get_info(uid, req.arg_u32_or(0, 1), now_ts)),
            rpc::SHOP_BUY => with_pushes(self.shop.buy(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1),
                req.arg_str_or(1, ""),
                req.arg_u32_or(2, 1),
                req.arg_u32_or(3, 0),
                req.arg_u32_or(4, 1),
                now_ts,
            )),
            rpc::SHOP_REFRESH => with_pushes(self.shop.refresh(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1),
                now_ts,
            )),
            rpc::RAFFLE_GET_INFO => Ok(self.raffle.get_info(uid, now_ts)),
            rpc::RAFFLE_SINGLE | rpc::RAFFLE_AUTO | "raffle.raffle" | rpc::RAFFLE_MULTI => {
                let is_multi = req.method == rpc::RAFFLE_MULTI;
                let pool = RafflePool::from_u32(req.arg_u32_or(0, 2)).unwrap_or(RafflePool::Divine);
                with_pushes(self.raffle.draw_standard(
                    self.players.get_mut(&uid).unwrap(),
                    pool,
                    if is_multi { 10 } else { 1 },
                    !is_multi,
                    now_ts,
                ))
            }
            rpc::RAFFLE_HOT | rpc::RAFFLE_MULTI_HOT | rpc::RAFFLE_RED => {
                let (pool, count) = match req.method.as_str() {
                    rpc::RAFFLE_HOT => (RafflePool::HotLimit, 1),
                    rpc::RAFFLE_MULTI_HOT => (RafflePool::HotLimit, 10),
                    _ => (RafflePool::RedHero, req.arg_u32_or(0, 1)),
                };
                with_pushes(self.raffle.draw_limit_or_red(
                    self.players.get_mut(&uid).unwrap(),
                    pool,
                    count,
                    now_ts,
                ))
            }
            rpc::RAFFLE_BUY_TOKEN | "raffle.buyRaffleToken" => with_pushes(self.raffle.buy_token(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1),
                now_ts,
            )),
            rpc::RAFFLE_RECEIVE_LUCK
            | rpc::RAFFLE_RECEIVE_LUCK_HOT
            | rpc::RAFFLE_RECEIVE_LUCK_RED => {
                let pool = match req.method.as_str() {
                    rpc::RAFFLE_RECEIVE_LUCK_HOT => RafflePool::HotLimit,
                    rpc::RAFFLE_RECEIVE_LUCK_RED => RafflePool::RedHero,
                    _ => RafflePool::Divine,
                };
                with_pushes(self.raffle.claim_luck_box(
                    self.players.get_mut(&uid).unwrap(),
                    pool,
                    req.arg_u32_or(0, 1),
                    now_ts,
                ))
            }

            // --- PvE Campaign Copy & Battle Record ---
            rpc::COPY_GET_LIST => Ok(self.copy.get_list(uid, req.arg_u32_or(0, 1))),
            rpc::COPY_GET_COPY => Ok(self.copy.get_chapter(uid, req.arg_u32_or(0, 1001))),
            rpc::USER_BEFORE_LOGIN_BATTLE => {
                self.copy.attack_stage(&self.players[&uid], 1001, 100101, true)
            }
            rpc::COPY_ATTACK => self.copy.attack_stage(
                &self.players[&uid],
                req.arg_u32_or(0, 1001),
                req.arg_u32_or(1, 100101),
                false,
            ),
            rpc::COPY_PASS => with_pushes(self.copy.pass_stage(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1001),
                req.arg_u32_or(1, 100101),
                now_ts,
            )),
            rpc::COPY_FAIL => Ok(self.copy.battle_fail(
                uid,
                req.arg_u32_or(0, 1001),
                req.arg_u32_or(1, 100101),
            )),
            rpc::COPY_SWEEP => with_pushes(self.copy.sweep_stage(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1001),
                req.arg_u32_or(1, 100101),
                req.arg_u32_or(2, 1),
                now_ts,
            )),
            rpc::COPY_RESET_ATK | "copy.resetAtkNum" => with_pushes(self.copy.reset_attack_count(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1001),
                req.arg_u32_or(1, 100101),
                now_ts,
            )),
            rpc::COPY_FETCH_PRIZE => with_pushes(self.copy.fetch_chapter_prize(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1001),
                req.arg_u32_or(1, 0),
                now_ts,
            )),
            rpc::COPY_FETCH_BOX_REWARD => with_pushes(self.copy.fetch_stage_box_reward(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1001),
                req.arg_u32_or(1, 100101),
                now_ts,
            )),
            rpc::BATTLE_GET_RECORD => self
                .copy
                .get_record_blob(req.arg_u64_or(0, 0))
                .map(DtoValue::Bytes)
                .ok_or("battle_record_not_found"),

            // --- Arena ---
            rpc::ARENA_ENTER | rpc::ARENA_GET_BASE_DATA => {
                Ok(self.arena.enter_arena(&self.players[&uid]))
            }
            rpc::ARENA_LEAVE => Ok(dto_ok!()),
            rpc::ARENA_CHALLENGE => self
                .arena
                .challenge(
                    self.players.get_mut(&uid).unwrap(),
                    req.arg_u32_or(1, 1),
                    &mut self.copy,
                    now_ts,
                )
                .map(|(ret, sp, pp)| {
                    client_pushes.extend(sp);
                    peer_pushes.extend(pp);
                    ret
                }),

            // --- Sign, Online & Activity ---
            rpc::SIGN_GET_INFO | "sign.getSignInfo" => {
                Ok(self.sign_online.get_sign_info(uid, now_ts))
            }
            rpc::SIGN_TODAY => {
                with_pushes(self.sign_online.sign_today(self.players.get_mut(&uid).unwrap(), now_ts))
            }
            rpc::SIGN_GAIN_VIP => with_pushes(self.sign_online.gain_vip_sign_reward(
                self.players.get_mut(&uid).unwrap(),
                req.arg_u32_or(0, 1),
                now_ts,
            )),
            rpc::ONLINE_GET_INFO | rpc::OPEN_ONLINE_GET_INFO | "online.getOnlineInfo" => {
                Ok(self.sign_online.get_online_info(&self.players[&uid]))
            }
            rpc::ONLINE_FETCH | rpc::OPEN_ONLINE_RECEIVE | "online.fetchOnlineReward" => {
                with_pushes(self.sign_online.fetch_online_reward(
                    self.players.get_mut(&uid).unwrap(),
                    req.arg_u32_or(0, 1),
                    now_ts,
                ))
            }
            rpc::ACTIVITY_GET_CONF => Ok(dto_map! { "version" => 1u32, "arrData" => dto_map!() }),

            _ => Err("unsupported_method"),
        };

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
