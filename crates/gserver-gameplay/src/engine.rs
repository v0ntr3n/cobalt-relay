//! Unified stateful RPC router (`GameplayEngine`) wiring together Player, Town, Chat/GM,
//! Mail/Reward Center, Shop/Tavern Raffle, PvE Copy/Battle, Arena, and Sign/Online modules.

use std::collections::BTreeMap;

use crate::battle_copy::{ArenaEngine, CopyCampaignEngine, SignAndOnlineService};
use crate::chat_gm::{ChatChannel, ChatMsgType, ChatService};
use crate::mail::MailAndRewardCenter;
use crate::player::PlayerState;
use crate::protocol::{rpc, DtoValue, PushPacket, RpcRequest, RpcResponse};
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

        let res: Result<DtoValue, &'static str> = match req.method.as_str() {
            // --- User, Bag, Hero & Formation ---
            rpc::USER_GET_USER => Ok(self.players[&uid].to_user_dto()),
            rpc::BAG_INFO => Ok(self.players[&uid].bag.to_bag_info_dto()),
            rpc::HERO_GET_ALL => Ok(self.players[&uid].roster.to_all_heroes_dto()),
            rpc::FORMATION_GET_POSITION_INFO => {
                Ok(self.players[&uid].roster.to_formation_position_info_dto())
            }

            // --- Town, City, RoyalCity & Castle ---
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
            rpc::ROYALCITY_ENTER => {
                let x = req.args.first().and_then(DtoValue::as_i64).map(|v| v as i32);
                let y = req.args.get(1).and_then(DtoValue::as_i64).map(|v| v as i32);
                self.town
                    .enter_town(&self.players[&uid], 999, x, y, now_ts)
                    .map(|(ret, bcasts)| {
                        peer_pushes.extend(bcasts);
                        ret
                    })
            }
            rpc::CITY_LEAVE | rpc::ROYALCITY_LEAVE => {
                let (ret, bcasts) = self.town.leave_town(uid, now_ts);
                peer_pushes.extend(bcasts);
                Ok(ret)
            }
            rpc::CITY_GET_ENTER_LIST => Ok(self.town.get_enter_list(uid)),
            rpc::TOWN_MOVE | rpc::ROYALCITY_MOVE => {
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
                // `ChatProxy:sendPersonMessage` sends `args = [targetUname, msgType, msg, ignoreFilter]`
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
            rpc::CHAT_GET_COUNTRY_LAST => Ok(self.chat.get_channel_last_msgs(ChatChannel::Country)),
            rpc::TM_CHAT_GET_CROSS_GUILD_LAST => {
                Ok(self.chat.get_channel_last_msgs(ChatChannel::TimeMachine))
            }
            rpc::CHAT_GET_PERSONAL_LAST => Ok(self.chat.get_personal_last_msgs(uid)),

            // --- Mail & Reward Center ---
            rpc::MAIL_GET_LIST => Ok(self.mail_center.get_mail_list(uid)),
            rpc::MAIL_READ_ALL => Ok(self.mail_center.read_all_mail(uid)),
            rpc::MAIL_SEND => {
                let target_uid = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0);
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
            rpc::REWARD_GET_LIST => {
                let offset = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0) as usize;
                let limit = req.args.get(1).and_then(DtoValue::as_u64).unwrap_or(100) as usize;
                Ok(self.mail_center.get_reward_list(uid, offset, limit, now_ts))
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
            rpc::REWARD_GIFT_BY_CODE => {
                let code = req.args.first().and_then(DtoValue::as_str).unwrap_or("");
                let player = self.players.get_mut(&uid).unwrap();
                self.mail_center
                    .redeem_gift_code(player, code, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }

            // --- Shop & Tavern Raffle ---
            rpc::SHOP_GET_INFO => {
                let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                Ok(self.shop.get_info(uid, shop_type, now_ts))
            }
            rpc::SHOP_BUY => {
                let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let goods_key = req.args.get(1).and_then(DtoValue::as_str).unwrap_or("");
                let idx = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1);
                let id = req.args.get(3).and_then(DtoValue::as_u32).unwrap_or(0);
                let buy_num = req.args.get(4).and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.shop
                    .buy(player, shop_type, goods_key, idx, id, buy_num, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::SHOP_REFRESH => {
                let shop_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.shop
                    .refresh(player, shop_type, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_GET_INFO => Ok(self.raffle.get_info(uid, now_ts)),
            rpc::RAFFLE_SINGLE | rpc::RAFFLE_AUTO => {
                let pool_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2);
                let pool = RafflePool::from_u32(pool_id).unwrap_or(RafflePool::Divine);
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .draw_standard(player, pool, 1, true, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_MULTI => {
                let pool_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(2);
                let pool = RafflePool::from_u32(pool_id).unwrap_or(RafflePool::Divine);
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .draw_standard(player, pool, 10, false, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_HOT => {
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .draw_limit_or_red(player, RafflePool::HotLimit, 1, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_MULTI_HOT => {
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .draw_limit_or_red(player, RafflePool::HotLimit, 10, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_RED => {
                let count = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .draw_limit_or_red(player, RafflePool::RedHero, count, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_BUY_TOKEN => {
                let qty = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .buy_token(player, qty, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_RECEIVE_LUCK => {
                let box_idx = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .claim_luck_box(player, RafflePool::Divine, box_idx, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_RECEIVE_LUCK_HOT => {
                let box_idx = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .claim_luck_box(player, RafflePool::HotLimit, box_idx, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::RAFFLE_RECEIVE_LUCK_RED => {
                let box_idx = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.raffle
                    .claim_luck_box(player, RafflePool::RedHero, box_idx, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }

            // --- PvE Campaign Copy & Battle Record ---
            rpc::COPY_GET_LIST => {
                let copy_type = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                Ok(self.copy.get_list(uid, copy_type))
            }
            rpc::COPY_GET_COPY => {
                let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                Ok(self.copy.get_chapter(uid, chapter_id))
            }
            rpc::USER_BEFORE_LOGIN_BATTLE => {
                self.copy.attack_stage(&self.players[&uid], 1001, 100101, true)
            }
            rpc::COPY_ATTACK => {
                let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                self.copy
                    .attack_stage(&self.players[&uid], chapter_id, copy_id, false)
            }
            rpc::COPY_PASS => {
                let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                let player = self.players.get_mut(&uid).unwrap();
                self.copy
                    .pass_stage(player, chapter_id, copy_id, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::COPY_FAIL => {
                let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                Ok(self.copy.battle_fail(uid, chapter_id, copy_id))
            }
            rpc::COPY_SWEEP => {
                let chapter_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1001);
                let copy_id = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(100101);
                let num = req.args.get(2).and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.copy
                    .sweep_stage(player, chapter_id, copy_id, num, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::COPY_RESET_ATK => {
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
            rpc::COPY_FETCH_PRIZE => {
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
            rpc::COPY_FETCH_BOX_REWARD => {
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
            rpc::BATTLE_GET_RECORD => {
                let brid = req.args.first().and_then(DtoValue::as_u64).unwrap_or(0);
                self.copy
                    .get_record_blob(brid)
                    .map(DtoValue::Bytes)
                    .ok_or("battle_record_not_found")
            }

            // --- Arena ---
            rpc::ARENA_ENTER | rpc::ARENA_GET_BASE_DATA => {
                Ok(self.arena.enter_arena(&self.players[&uid]))
            }
            rpc::ARENA_LEAVE => Ok(DtoValue::map([("ret", DtoValue::from("ok"))])),
            rpc::ARENA_CHALLENGE => {
                let target_pos = req.args.get(1).and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.arena
                    .challenge(player, target_pos, &mut self.copy, now_ts)
                    .map(|(ret, sp, pp)| {
                        client_pushes.extend(sp);
                        peer_pushes.extend(pp);
                        ret
                    })
            }

            // --- Sign, Online & Activity ---
            rpc::SIGN_GET_INFO => Ok(self.sign_online.get_sign_info(uid, now_ts)),
            rpc::SIGN_TODAY => {
                let player = self.players.get_mut(&uid).unwrap();
                self.sign_online
                    .sign_today(player, now_ts)
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
            rpc::ONLINE_GET_INFO | rpc::OPEN_ONLINE_GET_INFO => {
                Ok(self.sign_online.get_online_info(&self.players[&uid]))
            }
            rpc::ONLINE_FETCH | rpc::OPEN_ONLINE_RECEIVE => {
                let tier_id = req.args.first().and_then(DtoValue::as_u32).unwrap_or(1);
                let player = self.players.get_mut(&uid).unwrap();
                self.sign_online
                    .fetch_online_reward(player, tier_id, now_ts)
                    .map(|(ret, pushes)| {
                        client_pushes.extend(pushes);
                        ret
                    })
            }
            rpc::ACTIVITY_GET_CONF => Ok(DtoValue::map([
                ("version", DtoValue::from(1u32)),
                ("arrData", DtoValue::Map(BTreeMap::new())),
            ])),

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
