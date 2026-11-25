//! Chat subsystem (`chat.sendWorld`, `chat.sendPersonal`, `chat.sendGuild`, `*LastMsgs`,
//! `re.chat.getMsg`) and In-Game GM Slash-Command Processor (`/help`, `/gold`, `/silver`,
//! `/vip`, `/level`, `/item`, `/hero`, `/mail`, `/heal`).

use std::collections::{BTreeMap, VecDeque};

use crate::mail::{MailAndRewardCenter, RewardEntry};
use crate::player::{BagState, PlayerState};
use crate::protocol::{encode_amf3, push, DtoValue, PushPacket};
use crate::town::TownRoomManager;

/// Chat channel IDs verified from `ChatConst.lua` / `ChatProxy.lua`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChatChannel {
    World = 1,
    System = 2,
    Guild = 3,
    CrossGuild = 4,
    Private = 5,
    CrossTeam = 8,
    TimeMachine = 10,
    Country = 11,
}

/// Chat message payload types verified from `ChatConst.lua`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatMsgType {
    Normal = 1,
    Voice = 3,
    BattleReport = 4,
    RedEnvelope = 5,
}

impl ChatMsgType {
    pub fn from_i64(v: i64) -> Self {
        match v {
            3 => Self::Voice,
            4 => Self::BattleReport,
            5 => Self::RedEnvelope,
            _ => Self::Normal,
        }
    }
}

/// Exact chat return error codes verified from `ChatConst.lua` / `ChatProxy.lua`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatErrorCode {
    Succ = 10000,
    Offline = 10001,
    BlackName = 10002,
    NoTeam = 10003,
    NotOpen = 10004,
    Illegal = 10100,
    NoName = 10111,
    Ban = 11111,
}

/// Parsed GM slash command invoked from the in-game chat bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmCommand {
    Help,
    Gold(i64),
    Silver(i64),
    Vip(u32),
    Level(u32),
    Item { tid: u32, qty: u32 },
    Hero { htid: u32 },
    Mail { tid: u32, qty: u32 },
    Heal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmParseError {
    NotAGmCommand,
    UnknownCommand(String),
    MissingArg(&'static str),
    InvalidNumber(&'static str),
}

/// Parses GM slash commands:
/// `/help`, `/gold <amt>`, `/silver <amt>`, `/vip <lvl>`, `/level <lvl>`,
/// `/item <tid> <qty>`, `/hero <htid>`, `/mail [tid] [qty]`, `/heal`.
pub fn parse_gm_command(input: &str) -> Result<GmCommand, GmParseError> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Err(GmParseError::NotAGmCommand);
    }
    let mut parts = trimmed[1..].split_whitespace();
    let Some(cmd_raw) = parts.next() else {
        return Err(GmParseError::UnknownCommand(String::new()));
    };
    let cmd = cmd_raw.to_ascii_lowercase();
    match cmd.as_str() {
        "help" | "h" | "?" => Ok(GmCommand::Help),
        "gold" => {
            let s = parts.next().ok_or(GmParseError::MissingArg("/gold <amt>"))?;
            let amt = s.parse::<i64>().map_err(|_| GmParseError::InvalidNumber("gold amount"))?;
            Ok(GmCommand::Gold(amt))
        }
        "silver" | "copper" => {
            let s = parts.next().ok_or(GmParseError::MissingArg("/silver <amt>"))?;
            let amt = s.parse::<i64>().map_err(|_| GmParseError::InvalidNumber("silver amount"))?;
            Ok(GmCommand::Silver(amt))
        }
        "vip" => {
            let s = parts.next().ok_or(GmParseError::MissingArg("/vip <lvl>"))?;
            let lvl = s.parse::<u32>().map_err(|_| GmParseError::InvalidNumber("vip level"))?;
            Ok(GmCommand::Vip(lvl.min(18)))
        }
        "level" | "lv" => {
            let s = parts.next().ok_or(GmParseError::MissingArg("/level <lvl>"))?;
            let lvl = s.parse::<u32>().map_err(|_| GmParseError::InvalidNumber("player level"))?;
            if lvl == 0 {
                return Err(GmParseError::InvalidNumber("level must be >= 1"));
            }
            Ok(GmCommand::Level(lvl.min(200)))
        }
        "item" => {
            let tid_s = parts.next().ok_or(GmParseError::MissingArg("/item <tid> <qty>"))?;
            let qty_s = parts.next().unwrap_or("1");
            let tid = tid_s.parse::<u32>().map_err(|_| GmParseError::InvalidNumber("item tid"))?;
            let qty = qty_s.parse::<u32>().map_err(|_| GmParseError::InvalidNumber("item qty"))?;
            if qty == 0 {
                return Err(GmParseError::InvalidNumber("item qty must be >= 1"));
            }
            Ok(GmCommand::Item { tid, qty })
        }
        "hero" => {
            let htid_s = parts.next().ok_or(GmParseError::MissingArg("/hero <htid>"))?;
            let htid = htid_s.parse::<u32>().map_err(|_| GmParseError::InvalidNumber("hero htid"))?;
            Ok(GmCommand::Hero { htid })
        }
        "mail" => {
            let tid = match parts.next() {
                Some(s) => s.parse::<u32>().map_err(|_| GmParseError::InvalidNumber("mail item tid"))?,
                None => 16012, // Default Divine Recruitment Token
            };
            let qty = match parts.next() {
                Some(s) => s.parse::<u32>().map_err(|_| GmParseError::InvalidNumber("mail item qty"))?,
                None => 10,
            };
            Ok(GmCommand::Mail { tid, qty: qty.max(1) })
        }
        "heal" => Ok(GmCommand::Heal),
        _ => Err(GmParseError::UnknownCommand(cmd)),
    }
}

/// Result of executing a GM slash command against live player, mail, and town state.
#[derive(Debug, Clone)]
pub struct GmExecutionResult {
    pub summary: String,
    pub client_pushes: Vec<PushPacket>,
    pub town_broadcasts: Vec<(u64, PushPacket)>,
}

/// Executes a parsed `GmCommand` and generates live `user.updateUser`, `bag.update`,
/// `re.hero.new`, `re.mail.newMail`, `re.reward.new`, `town.update`, and system `re.chat.getMsg` pushes.
pub fn execute_gm_command(
    cmd: GmCommand,
    player: &mut PlayerState,
    mail_center: &mut MailAndRewardCenter,
    town_mgr: &mut TownRoomManager,
    now_ts: u64,
) -> Result<GmExecutionResult, &'static str> {
    if !player.gm_enabled {
        return Err("gm_permission_denied");
    }

    let mut client_pushes = Vec::new();
    let mut town_broadcasts = Vec::new();

    let summary = match cmd {
        GmCommand::Help => {
            "GM Commands: /help | /gold <amt> | /silver <amt> | /vip <0..18> | /level <1..200> | /item <tid> <qty> | /hero <htid> | /mail [tid] [qty] | /heal".to_owned()
        }
        GmCommand::Gold(delta) => {
            player.gold = (player.gold + delta).max(0);
            client_pushes.push(PlayerState::build_user_update_push(
                [("gold", DtoValue::from(player.gold))],
                now_ts,
            ));
            format!("[GM] Gold updated -> {}", player.gold)
        }
        GmCommand::Silver(delta) => {
            let raw = (player.copper + delta).max(0);
            let capped = raw > PlayerState::COPPER_CAP;
            player.copper = raw.min(PlayerState::COPPER_CAP);
            client_pushes.push(PlayerState::build_user_update_push(
                [("copper", DtoValue::from(player.copper))],
                now_ts,
            ));
            if capped {
                client_pushes.push(PushPacket::new(
                    push::USER_COPPER_OVER_MAX,
                    DtoValue::from(1u32),
                    now_ts,
                ));
            }
            format!("[GM] Silver/Copper updated -> {}", player.copper)
        }
        GmCommand::Vip(vip_lv) => {
            player.vip = vip_lv;
            player.vip_exp = u64::from(vip_lv) * 1_000;
            client_pushes.push(PlayerState::build_user_update_push(
                [
                    ("vip", DtoValue::from(player.vip)),
                    ("vip_exp", DtoValue::from(player.vip_exp)),
                ],
                now_ts,
            ));
            town_broadcasts.extend(town_mgr.sync_player_appearance(player, now_ts));
            format!("[GM] VIP level set -> VIP {}", player.vip)
        }
        GmCommand::Level(lv) => {
            player.level = lv;
            player.exp = 0;
            let main_hid = player.roster.formation[0];
            if let Some(main_hero) = player.roster.heroes.get_mut(&main_hid) {
                main_hero.level = lv;
                main_hero.recalculate_stats();
            }
            let ff = player.fight_force();
            client_pushes.push(PlayerState::build_user_update_push(
                [
                    ("level", DtoValue::from(player.level)),
                    ("exp", DtoValue::from(player.exp)),
                    ("fight_force", DtoValue::from(ff)),
                ],
                now_ts,
            ));
            town_broadcasts.extend(town_mgr.sync_player_appearance(player, now_ts));
            format!("[GM] Level set -> Lv.{} (FightForce={})", player.level, ff)
        }
        GmCommand::Item { tid, qty } => {
            let deltas = player.bag.add_item(tid, qty, now_ts)?;
            if let Some(bag_push) = BagState::build_update_push(&deltas, now_ts) {
                client_pushes.push(bag_push);
            }
            let total = player.bag.count_by_tid(tid);
            format!("[GM] Added item tid={tid} x{qty} (bag total={total})")
        }
        GmCommand::Hero { htid } => {
            let (hero, hero_push) = player.roster.recruit_hero(htid, player.level.max(1), now_ts);
            client_pushes.push(hero_push);
            let ff = player.fight_force();
            client_pushes.push(PlayerState::build_user_update_push(
                [("fight_force", DtoValue::from(ff))],
                now_ts,
            ));
            town_broadcasts.extend(town_mgr.sync_player_appearance(player, now_ts));
            format!(
                "[GM] Recruited hero htid={} -> hid={} (quality={})",
                htid, hero.hid, hero.quality
            )
        }
        GmCommand::Mail { tid, qty } => {
            let (mail, bundle, mail_pushes) = mail_center.send_system_mail(
                player.uid,
                0,
                "GM Gift Delivery",
                format!("GM dispatched item {tid} x{qty} + 500 Gold to your Reward Center."),
                vec![RewardEntry::Gold(500), RewardEntry::Item { tid, qty }],
                now_ts,
            );
            client_pushes.extend(mail_pushes);
            let rid = bundle.map_or(0, |b| b.rid);
            format!("[GM] Sent system mail mid={} & reward rid={}", mail.mid, rid)
        }
        GmCommand::Heal => {
            let healed_heroes = player.roster.heal_all();
            player.execution = player.execution_max;
            player.energy = player.energy_max;
            player.execution_time = now_ts;
            player.energy_time = now_ts;
            client_pushes.push(PlayerState::build_user_update_push(
                [
                    ("execution", DtoValue::from(player.execution)),
                    ("execution_time", DtoValue::from(player.execution_time)),
                    ("energy", DtoValue::from(player.energy)),
                    ("energy_time", DtoValue::from(player.energy_time)),
                ],
                now_ts,
            ));
            format!(
                "[GM] Healed {} injured heroes; restored execution={} & energy={}",
                healed_heroes, player.execution, player.energy
            )
        }
    };

    // Push GM feedback into the player's chat window via `re.chat.getMsg`
    let feedback_dto = build_chat_push_dto(
        ChatChannel::Private,
        ChatMsgType::Normal,
        &summary,
        0,
        "[GM]",
        1,
        99,
        15,
        6,
        0,
        &player.uname,
        player.quality,
        now_ts,
    );
    client_pushes.push(PushPacket::new(push::CHAT_GET_MSG, feedback_dto, now_ts));

    Ok(GmExecutionResult {
        summary,
        client_pushes,
        town_broadcasts,
    })
}

/// Builds the exact `re.chat.getMsg` DTO decoded by `ChatProxy:onMsgPush(ret)`:
/// `{ message = { channel, message, type }, user_info = { uid, uname, utid, level, vip, quality, fight_force, dress_id, avatar_id, title_id, guild_name, time, toName, toQuality } }`.
#[allow(clippy::too_many_arguments)]
pub fn build_chat_push_dto(
    channel: ChatChannel,
    msg_type: ChatMsgType,
    text: &str,
    uid: u64,
    uname: &str,
    utid: u32,
    level: u32,
    vip: u32,
    quality: u32,
    fight_force: u64,
    to_name: &str,
    to_quality: u32,
    now_ts: u64,
) -> DtoValue {
    let message = DtoValue::map([
        ("channel", DtoValue::from(channel as u32)),
        ("message", DtoValue::from(text)),
        ("type", DtoValue::from(msg_type as u32)),
    ]);
    let user_info = DtoValue::map([
        ("uid", DtoValue::from(uid)),
        ("uname", DtoValue::from(uname)),
        ("utid", DtoValue::from(utid)),
        ("level", DtoValue::from(level)),
        ("vip", DtoValue::from(vip)),
        ("quality", DtoValue::from(quality)),
        ("fight_force", DtoValue::from(fight_force)),
        ("dress_id", DtoValue::from(0u32)),
        ("avatar_id", DtoValue::from(utid)),
        ("title_id", DtoValue::from(0u32)),
        ("guild_name", DtoValue::from("")),
        ("time", DtoValue::from(now_ts)),
        ("toName", DtoValue::from(to_name)),
        ("toQuality", DtoValue::from(to_quality)),
    ]);
    DtoValue::map([("message", message), ("user_info", user_info)])
}

/// Manages channel history ring buffers (`*LastMsgs` AMF3 blobs) and chat routing + GM interception.
#[derive(Debug, Clone)]
pub struct ChatService {
    pub max_history_per_channel: usize,
    pub max_char_len: usize,
    history: BTreeMap<ChatChannel, VecDeque<DtoValue>>,
    personal_history: BTreeMap<u64, VecDeque<DtoValue>>,
}

impl Default for ChatService {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatService {
    pub fn new() -> Self {
        Self {
            max_history_per_channel: 50,
            max_char_len: 120,
            history: BTreeMap::new(),
            personal_history: BTreeMap::new(),
        }
    }

    pub fn send_response_dto(code: ChatErrorCode, filtered_msg: &str, ban_end: u64) -> DtoValue {
        DtoValue::map([
            ("error_code", DtoValue::from(code as u32)),
            ("message", DtoValue::from(filtered_msg)),
            ("ban_end_time", DtoValue::from(ban_end)),
        ])
    }

    /// Sends a public/guild/cross-server chat message (or executes a GM `/command` if prefixed with `/`).
    #[allow(clippy::too_many_arguments)]
    pub fn send_channel_message(
        &mut self,
        channel: ChatChannel,
        msg_type: ChatMsgType,
        raw_text: &str,
        sender: &mut PlayerState,
        all_online_uids: &[u64],
        mail_center: &mut MailAndRewardCenter,
        town_mgr: &mut TownRoomManager,
        now_ts: u64,
    ) -> (DtoValue, Vec<PushPacket>, Vec<(u64, PushPacket)>) {
        // 1. Intercept GM slash commands before level/VIP chat gates!
        if raw_text.trim_start().starts_with('/') && sender.gm_enabled {
            match parse_gm_command(raw_text) {
                Ok(gm_cmd) => match execute_gm_command(gm_cmd, sender, mail_center, town_mgr, now_ts) {
                    Ok(exec) => {
                        let resp = Self::send_response_dto(ChatErrorCode::Succ, &exec.summary, 0);
                        return (resp, exec.client_pushes, exec.town_broadcasts);
                    }
                    Err(err_msg) => {
                        let err_push = PushPacket::new(
                            push::CHAT_GET_MSG,
                            build_chat_push_dto(
                                ChatChannel::Private,
                                ChatMsgType::Normal,
                                &format!("[GM Error] {err_msg}"),
                                0,
                                "[GM]",
                                1,
                                99,
                                15,
                                6,
                                0,
                                &sender.uname,
                                sender.quality,
                                now_ts,
                            ),
                            now_ts,
                        );
                        let resp = Self::send_response_dto(ChatErrorCode::Succ, err_msg, 0);
                        return (resp, vec![err_push], Vec::new());
                    }
                },
                Err(parse_err) => {
                    let msg = format!("[GM Syntax] {parse_err:?} (try /help)");
                    let err_push = PushPacket::new(
                        push::CHAT_GET_MSG,
                        build_chat_push_dto(
                            ChatChannel::Private,
                            ChatMsgType::Normal,
                            &msg,
                            0,
                            "[GM]",
                            1,
                            99,
                            15,
                            6,
                            0,
                            &sender.uname,
                            sender.quality,
                            now_ts,
                        ),
                        now_ts,
                    );
                    let resp = Self::send_response_dto(ChatErrorCode::Succ, &msg, 0);
                    return (resp, vec![err_push], Vec::new());
                }
            }
        }

        // 2. Standard chat validation (`ChatProxy.lua`: mute check, length check, level>=40 or vip>=2 for non-guild)
        if sender.chat_ban_until > now_ts {
            return (
                Self::send_response_dto(ChatErrorCode::Ban, "", sender.chat_ban_until),
                Vec::new(),
                Vec::new(),
            );
        }
        let text = raw_text.trim();
        if text.is_empty() || text.chars().count() > self.max_char_len {
            return (
                Self::send_response_dto(ChatErrorCode::Illegal, "", 0),
                Vec::new(),
                Vec::new(),
            );
        }
        if channel != ChatChannel::Guild && sender.level < 40 && sender.vip < 2 {
            return (
                Self::send_response_dto(ChatErrorCode::NotOpen, "", 0),
                Vec::new(),
                Vec::new(),
            );
        }
        if channel == ChatChannel::Guild && sender.guild_name.is_empty() {
            return (
                Self::send_response_dto(ChatErrorCode::NoTeam, "", 0),
                Vec::new(),
                Vec::new(),
            );
        }

        let push_dto = build_chat_push_dto(
            channel,
            msg_type,
            text,
            sender.uid,
            &sender.uname,
            sender.utid,
            sender.level,
            sender.vip,
            sender.quality,
            sender.fight_force(),
            "",
            0,
            now_ts,
        );

        let q = self.history.entry(channel).or_default();
        if q.len() >= self.max_history_per_channel {
            q.pop_front();
        }
        q.push_back(push_dto.clone());

        let push_name = if channel == ChatChannel::TimeMachine {
            push::TM_CHAT_GET_MSG
        } else {
            push::CHAT_GET_MSG
        };
        let pkt = PushPacket::new(push_name, push_dto, now_ts);

        let mut self_pushes = vec![pkt.clone()];
        let mut peer_broadcasts = Vec::new();
        for &uid in all_online_uids {
            if uid != sender.uid {
                peer_broadcasts.push((uid, pkt.clone()));
            }
        }
        let _ = &mut self_pushes;

        (
            Self::send_response_dto(ChatErrorCode::Succ, text, 0),
            self_pushes,
            peer_broadcasts,
        )
    }

    /// Sends a private whisper (`chat.sendPersonal`, `args = [targetUname, msgType, msg, ignoreFilter]`).
    pub fn send_personal_message(
        &mut self,
        sender: &PlayerState,
        target: Option<(&u64, &str, u32)>,
        msg_type: ChatMsgType,
        raw_text: &str,
        now_ts: u64,
    ) -> (DtoValue, Vec<PushPacket>, Vec<(u64, PushPacket)>) {
        if sender.chat_ban_until > now_ts {
            return (
                Self::send_response_dto(ChatErrorCode::Ban, "", sender.chat_ban_until),
                Vec::new(),
                Vec::new(),
            );
        }
        if sender.level < 40 && sender.vip < 2 {
            return (
                Self::send_response_dto(ChatErrorCode::NotOpen, "", 0),
                Vec::new(),
                Vec::new(),
            );
        }
        let text = raw_text.trim();
        if text.is_empty() || text.chars().count() > self.max_char_len {
            return (
                Self::send_response_dto(ChatErrorCode::Illegal, "", 0),
                Vec::new(),
                Vec::new(),
            );
        }
        let Some((&target_uid, target_uname, target_quality)) = target else {
            return (
                Self::send_response_dto(ChatErrorCode::Offline, "", 0),
                Vec::new(),
                Vec::new(),
            );
        };

        let dto = build_chat_push_dto(
            ChatChannel::Private,
            msg_type,
            text,
            sender.uid,
            &sender.uname,
            sender.utid,
            sender.level,
            sender.vip,
            sender.quality,
            sender.fight_force(),
            target_uname,
            target_quality,
            now_ts,
        );

        for uid in [sender.uid, target_uid] {
            let q = self.personal_history.entry(uid).or_default();
            if q.len() >= self.max_history_per_channel {
                q.pop_front();
            }
            q.push_back(dto.clone());
        }

        let pkt = PushPacket::new(push::CHAT_GET_MSG, dto, now_ts);
        let self_pushes = vec![pkt.clone()];
        let peer_pushes = if target_uid != sender.uid {
            vec![(target_uid, pkt)]
        } else {
            Vec::new()
        };

        (
            Self::send_response_dto(ChatErrorCode::Succ, text, 0),
            self_pushes,
            peer_pushes,
        )
    }

    /// Returns channel history (`*LastMsgs`) in **reverse chronological order** (`tmp[len - k]`)
    /// with each entry encoded as an AMF3 `Bytes` blob, matching `ChatProxy:get*LastMsgs`!
    pub fn get_channel_last_msgs(&self, channel: ChatChannel) -> DtoValue {
        let Some(q) = self.history.get(&channel) else {
            return DtoValue::Array(Vec::new());
        };
        let blobs: Vec<DtoValue> = q
            .iter()
            .rev()
            .map(|dto| DtoValue::Bytes(encode_amf3(dto)))
            .collect();
        DtoValue::Array(blobs)
    }

    /// Returns personal chat history (`chat.getPersonalLastMsgs`) in reverse chronological AMF3 blobs.
    pub fn get_personal_last_msgs(&self, uid: u64) -> DtoValue {
        let Some(q) = self.personal_history.get(&uid) else {
            return DtoValue::Array(Vec::new());
        };
        let blobs: Vec<DtoValue> = q
            .iter()
            .rev()
            .map(|dto| DtoValue::Bytes(encode_amf3(dto)))
            .collect();
        DtoValue::Array(blobs)
    }
}
