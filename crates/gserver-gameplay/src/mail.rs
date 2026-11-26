//! Mail Box (`mail.getMailBoxList`, `mail.getMailList`, `mail.readAllMail`, `mail.sendMail`,
//! `mail.drawMailItems`, `mail.drawAllMailItems`, `tmMail.*`, `re.mail.newMail`, `tmRe.mail.newMail`)
//! and Reward Center (`reward.getRewardList`, `reward.receiveReward`, `reward.receiveByRidArr`,
//! `reward.giftByCode`, `tmReward.*`, `re.reward.new`, `re.tmReward.new`)
//! matching `MailConst.lua`, `MailData.lua`, `MailItemUI.lua`, `RewardData.lua`, and `RewardProxy.lua`.

use std::collections::{BTreeMap, BTreeSet};

use crate::player::{BagCategory, BagState, PlayerState};
use crate::protocol::{push, DtoValue, PushPacket};

/// Push callback name for Time Machine new mail (`MailProxy_TM.lua`).
pub const PUSH_TM_MAIL_NEW: &str = "tmRe.mail.newMail";
/// Push callback name for Time Machine new reward (`RewardProxy_TM.lua`).
pub const PUSH_TM_REWARD_NEW: &str = "re.tmReward.new";

/// Standard GServer reward 3-tuple `[price_type, template_id, amount]` (`PriceTypeConst` / `raffle.db` / `online.db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewardEntry {
    /// Type `1`: Silver / Copper (`[1, 0, amount]`).
    Copper(i64),
    /// Type `2`: Gold (`[2, 0, amount]`).
    Gold(i64),
    /// Type `4`: Inventory Item / Fragment (`[4, item_tid, qty]`).
    Item { tid: u32, qty: u32 },
    /// Type `7`: Hero Soul (`[7, 0, amount]`).
    HeroSoul(u64),
    /// Type `8`: Arena Prestige / Coin (`[8, 0, amount]`).
    ArenaCoin(u64),
    /// Type `10`: Direct Hero Card (`[10, htid, qty]`).
    Hero { htid: u32, count: u32 },
}

impl RewardEntry {
    pub fn to_triplet_dto(self) -> DtoValue {
        match self {
            Self::Copper(amt) => {
                DtoValue::array([DtoValue::from(1), DtoValue::from(0), DtoValue::from(amt)])
            }
            Self::Gold(amt) => {
                DtoValue::array([DtoValue::from(2), DtoValue::from(0), DtoValue::from(amt)])
            }
            Self::Item { tid, qty } => {
                DtoValue::array([DtoValue::from(4), DtoValue::from(tid), DtoValue::from(qty)])
            }
            Self::HeroSoul(amt) => {
                DtoValue::array([DtoValue::from(7), DtoValue::from(0), DtoValue::from(amt)])
            }
            Self::ArenaCoin(amt) => {
                DtoValue::array([DtoValue::from(8), DtoValue::from(0), DtoValue::from(amt)])
            }
            Self::Hero { htid, count } => DtoValue::array([
                DtoValue::from(10),
                DtoValue::from(htid),
                DtoValue::from(count),
            ]),
        }
    }
}

/// Applies a batch of `RewardEntry` items atomically to `PlayerState`, returning live pushes
/// (`user.updateUser`, `bag.update`, `re.hero.new`). Checks bag capacity beforehand.
pub fn grant_rewards_to_player(
    player: &mut PlayerState,
    rewards: &[RewardEntry],
    now_ts: u64,
) -> Result<Vec<PushPacket>, &'static str> {
    // Pre-flight capacity check for non-existing item templates
    for entry in rewards {
        if let RewardEntry::Item { tid, qty } = *entry {
            if qty > 0 && player.bag.count_by_tid(tid) == 0 {
                let cat = BagCategory::from_template_id(tid);
                if player.bag.is_category_full(cat) {
                    return Err("bag_full");
                }
            }
        }
    }

    let mut gold_changed = false;
    let mut copper_changed = false;
    let mut copper_over_max = false;
    let mut soul_changed = false;
    let mut arena_changed = false;
    let mut bag_deltas = Vec::new();
    let mut pushes = Vec::new();

    for &entry in rewards {
        match entry {
            RewardEntry::Copper(amt) => {
                let next = player.copper.saturating_add(amt);
                if next > PlayerState::COPPER_CAP {
                    player.copper = PlayerState::COPPER_CAP;
                    copper_over_max = true;
                } else {
                    player.copper = next;
                }
                copper_changed = true;
            }
            RewardEntry::Gold(amt) => {
                player.gold = player.gold.saturating_add(amt);
                gold_changed = true;
            }
            RewardEntry::HeroSoul(amt) => {
                player.hero_soul = player.hero_soul.saturating_add(amt);
                soul_changed = true;
            }
            RewardEntry::ArenaCoin(amt) => {
                player.arena_coin = player.arena_coin.saturating_add(amt);
                arena_changed = true;
            }
            RewardEntry::Item { tid, qty } => {
                let deltas = player.bag.add_item(tid, qty, now_ts)?;
                bag_deltas.extend(deltas);
            }
            RewardEntry::Hero { htid, count } => {
                for _ in 0..count {
                    let (_, hero_push) = player.roster.recruit_hero(htid, 1, now_ts);
                    pushes.push(hero_push);
                }
            }
        }
    }

    let mut user_fields = Vec::new();
    if gold_changed {
        user_fields.push(("gold", DtoValue::from(player.gold)));
    }
    if copper_changed {
        user_fields.push(("copper", DtoValue::from(player.copper)));
    }
    if soul_changed {
        user_fields.push(("hero_soul", DtoValue::from(player.hero_soul)));
    }
    if arena_changed {
        user_fields.push(("arena_coin", DtoValue::from(player.arena_coin)));
    }
    if !user_fields.is_empty() {
        pushes.insert(0, PlayerState::build_user_update_push(user_fields, now_ts));
    }
    if copper_over_max {
        pushes.push(PushPacket::new(
            push::USER_COPPER_OVER_MAX,
            DtoValue::from(1u32),
            now_ts,
        ));
    }
    if let Some(bag_push) = BagState::build_update_push(&bag_deltas, now_ts) {
        pushes.push(bag_push);
    }

    Ok(pushes)
}

/// Mail category matching `MailConst.lua` (`MAIL_PLAYER = 1`, `MAIL_SYSTEM = 2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailType {
    /// `MailConst.MAIL_PLAYER = 1` (player-to-player / social message).
    Player = 1,
    /// `MailConst.MAIL_SYSTEM = 2` (system / template mail — looked up in `mail.db` by `template_id`).
    System = 2,
    /// Battle / Arena / Mine report mail (`mail.db` battle template; wire `mail_type = 2`).
    Battle = 3,
}

impl MailType {
    /// Backwards-compatible alias for player-to-player social mail (`MailConst.MAIL_PLAYER = 1`).
    #[allow(non_upper_case_globals)]
    pub const Social: Self = Self::Player;

    /// Returns the exact wire `mail_type` expected by `MailConst.lua` / `MailData.lua` (`1` or `2`).
    pub fn wire_code(self) -> u32 {
        match self {
            Self::Player => 1,
            Self::System | Self::Battle => 2,
        }
    }
}

/// Single notification mail entry (`mail.getMailBoxList`, `mail.getMailList`, & `re.mail.newMail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMessage {
    pub mid: u64,
    pub template_id: u32,
    pub mail_type: MailType,
    pub sender_uid: u64,
    pub sender_uname: String,
    pub subject: String,
    pub content: String,
    pub recv_time: u64,
    pub is_read: bool,
    pub va_extra: BTreeMap<String, String>,
}

impl MailMessage {
    /// Serializes to the exact `MailData.lua` & `MailItemUI.lua` schema:
    /// - `mail_type`: `1` (`MAIL_PLAYER`) or `2` (`MAIL_SYSTEM`)
    /// - `template_id`: guaranteed `>= 1` when `mail_type == 2` so `mailReader:getRowById(template_id)` never returns `nil`
    /// - `read_time`: `0` when unread (`MailData:isNewMail` checks `v.read_time == 0`), `> 0` when read
    /// - `va_extra`: `{ data = [ { k = v, ... } ], color = [] }` (`MailItemUI:fillContent` iterates `ipairs(va_extra.data)` then `pairs(v)` and `ipairs(va_extra.color)`)
    pub fn to_dto(&self) -> DtoValue {
        let wire_mail_type = self.mail_type.wire_code();
        let resolved_template_id = if wire_mail_type == 2 && self.template_id == 0 {
            1
        } else {
            self.template_id
        };
        let read_time = if self.is_read {
            self.recv_time.max(1)
        } else {
            0
        };

        let mut kv_map = BTreeMap::new();
        for (k, v) in &self.va_extra {
            kv_map.insert(k.clone(), DtoValue::from(v.as_str()));
        }
        let data_arr = if kv_map.is_empty() {
            DtoValue::Array(Vec::new())
        } else {
            DtoValue::array([DtoValue::Map(kv_map.clone())])
        };
        let va_extra_dto = DtoValue::map([
            ("data", data_arr),
            ("color", DtoValue::Array(Vec::new())),
        ]);

        DtoValue::map([
            ("mid", DtoValue::from(self.mid)),
            ("template_id", DtoValue::from(resolved_template_id)),
            ("mail_type", DtoValue::from(wire_mail_type)),
            ("sender_uid", DtoValue::from(self.sender_uid)),
            ("sender_uname", DtoValue::from(self.sender_uname.as_str())),
            ("subject", DtoValue::from(self.subject.as_str())),
            ("content", DtoValue::from(self.content.as_str())),
            ("recv_time", DtoValue::from(self.recv_time)),
            ("read_time", DtoValue::from(read_time)),
            ("status", DtoValue::from(if self.is_read { 1u32 } else { 0u32 })),
            ("vip", DtoValue::from(0u32)),
            ("va_extra", va_extra_dto.clone()),
            ("va_mail", va_extra_dto),
        ])
    }
}

/// Claimable attachment bundle in `RewardData.lua` / `RewardProxy.lua` (`reward.getRewardList`, `re.reward.new`).
///
/// Verified against `RewardProxy.lua`:
/// - `va_reward[1]` is the 1-indexed array of `[type, tid, num]` reward triplets.
/// - `va_reward[2]` is the metadata map (`description`, `msg`, `rank`, `day`, `num`, `layer`, `time`, `cityName`)
///   formatted by `RewardProxy:getDesBySourceAndReward(source, va_reward)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewardBundle {
    pub rid: u64,
    pub source: u32,
    pub title: String,
    pub send_time: u64,
    pub expire_time: u64,
    pub items: Vec<RewardEntry>,
}

impl RewardBundle {
    pub fn is_expired(&self, now_ts: u64) -> bool {
        self.expire_time > 0 && now_ts >= self.expire_time
    }

    pub fn to_dto(&self) -> DtoValue {
        let triplets = DtoValue::Array(
            self.items
                .iter()
                .copied()
                .map(RewardEntry::to_triplet_dto)
                .collect(),
        );
        let meta = DtoValue::map([
            ("description", DtoValue::from(self.title.as_str())),
            ("msg", DtoValue::from(self.title.as_str())),
            ("rank", DtoValue::from(1u32)),
            ("day", DtoValue::from(1u32)),
            ("num", DtoValue::from(1u32)),
            ("layer", DtoValue::from(1u32)),
            ("time", DtoValue::from(self.send_time)),
            ("cityName", DtoValue::from("")),
        ]);
        DtoValue::map([
            ("rid", DtoValue::from(self.rid)),
            ("source", DtoValue::from(self.source)),
            ("title", DtoValue::from(self.title.as_str())),
            ("send_time", DtoValue::from(self.send_time)),
            ("expire_time", DtoValue::from(self.expire_time)),
            ("va_reward", DtoValue::array([triplets, meta])),
        ])
    }
}

/// Per-player Mail Box + Reward Center + Gift Code redemption store (Main & Time Machine).
#[derive(Debug, Clone)]
pub struct MailAndRewardCenter {
    next_mid: u64,
    next_rid: u64,
    mails: BTreeMap<u64, Vec<MailMessage>>,
    mail_attachments: BTreeMap<(u64, u64), Vec<RewardEntry>>,
    drawn_mids: BTreeMap<u64, BTreeSet<u64>>,
    rewards: BTreeMap<u64, Vec<RewardBundle>>,
    claimed_rids: BTreeMap<u64, BTreeSet<u64>>,
    tm_mails: BTreeMap<u64, Vec<MailMessage>>,
    tm_rewards: BTreeMap<u64, Vec<RewardBundle>>,
    tm_claimed_rids: BTreeMap<u64, BTreeSet<u64>>,
    redeemed_codes: BTreeMap<u64, BTreeSet<String>>,
    gift_codes: BTreeMap<String, Vec<RewardEntry>>,
}

impl Default for MailAndRewardCenter {
    fn default() -> Self {
        Self::new()
    }
}

impl MailAndRewardCenter {
    /// Default retention window (14 days, matching `RewardPanel.lua` `reward_4`).
    pub const DEFAULT_TTL_SECS: u64 = 14 * 86_400;

    pub fn new() -> Self {
        let mut gift_codes = BTreeMap::new();
        gift_codes.insert(
            "GSERVERVIP888".to_owned(),
            vec![
                RewardEntry::Gold(888),
                RewardEntry::Copper(500_000),
                RewardEntry::Item { tid: 16012, qty: 5 },
            ],
        );
        gift_codes.insert(
            "TANTHU2026".to_owned(),
            vec![
                RewardEntry::Gold(500),
                RewardEntry::Item {
                    tid: 16011,
                    qty: 10,
                },
            ],
        );
        Self {
            next_mid: 50_001,
            next_rid: 80_001,
            mails: BTreeMap::new(),
            mail_attachments: BTreeMap::new(),
            drawn_mids: BTreeMap::new(),
            rewards: BTreeMap::new(),
            claimed_rids: BTreeMap::new(),
            tm_mails: BTreeMap::new(),
            tm_rewards: BTreeMap::new(),
            tm_claimed_rids: BTreeMap::new(),
            redeemed_codes: BTreeMap::new(),
            gift_codes,
        }
    }

    pub fn register_gift_code(&mut self, code: impl Into<String>, rewards: Vec<RewardEntry>) {
        self.gift_codes
            .insert(code.into().to_ascii_uppercase(), rewards);
    }

    /// Pushes a notification mail (`re.mail.newMail` as a direct Map, NOT an array!)
    /// and optional Reward Center attachment (`re.reward.new` as `[rb_dto]`).
    pub fn send_system_mail(
        &mut self,
        target_uid: u64,
        template_id: u32,
        subject: impl Into<String>,
        content: impl Into<String>,
        attachments: Vec<RewardEntry>,
        now_ts: u64,
    ) -> (MailMessage, Option<RewardBundle>, Vec<PushPacket>) {
        let mid = self.next_mid;
        self.next_mid += 1;
        let subj = subject.into();
        let resolved_tpl = if template_id == 0 { 1 } else { template_id };
        let mail = MailMessage {
            mid,
            template_id: resolved_tpl,
            mail_type: MailType::System,
            sender_uid: 0,
            sender_uname: "System".to_owned(),
            subject: subj.clone(),
            content: content.into(),
            recv_time: now_ts,
            is_read: false,
            va_extra: BTreeMap::new(),
        };
        self.mails
            .entry(target_uid)
            .or_default()
            .insert(0, mail.clone());

        // `MailData:newMailHandler(ret)` executes `table.insert(self.list_original, ret)` and sorts by `ret.recv_time`:
        // `re.mail.newMail` MUST push `mail.to_dto()` directly as a Map!
        let mut pushes = vec![PushPacket::new(push::MAIL_NEW, mail.to_dto(), now_ts)];

        let bundle = if !attachments.is_empty() {
            self.mail_attachments
                .insert((target_uid, mid), attachments.clone());
            let rid = self.next_rid;
            self.next_rid += 1;
            let rb = RewardBundle {
                rid,
                source: 2, // `RewardProxy.SYSTEM_GENERAL = 2` (uses `va_reward[2].description`)
                title: subj,
                send_time: now_ts,
                expire_time: now_ts + Self::DEFAULT_TTL_SECS,
                items: attachments,
            };
            self.rewards
                .entry(target_uid)
                .or_default()
                .insert(0, rb.clone());
            // `RewardData:onRewardNew(ret)` iterates `for _, reward in pairs(ret)` on `[rb.to_dto()]`!
            pushes.push(PushPacket::new(
                push::REWARD_NEW,
                DtoValue::array([rb.to_dto()]),
                now_ts,
            ));
            Some(rb)
        } else {
            None
        };

        (mail, bundle, pushes)
    }

    /// Pushes a Time Machine system mail (`tmRe.mail.newMail`) and optional TM reward (`re.tmReward.new`).
    pub fn send_tm_system_mail(
        &mut self,
        target_uid: u64,
        template_id: u32,
        subject: impl Into<String>,
        content: impl Into<String>,
        attachments: Vec<RewardEntry>,
        now_ts: u64,
    ) -> (MailMessage, Option<RewardBundle>, Vec<PushPacket>) {
        let mid = self.next_mid;
        self.next_mid += 1;
        let subj = subject.into();
        let resolved_tpl = if template_id == 0 { 1 } else { template_id };
        let mail = MailMessage {
            mid,
            template_id: resolved_tpl,
            mail_type: MailType::System,
            sender_uid: 0,
            sender_uname: "System".to_owned(),
            subject: subj.clone(),
            content: content.into(),
            recv_time: now_ts,
            is_read: false,
            va_extra: BTreeMap::new(),
        };
        self.tm_mails
            .entry(target_uid)
            .or_default()
            .insert(0, mail.clone());

        let mut pushes = vec![PushPacket::new(PUSH_TM_MAIL_NEW, mail.to_dto(), now_ts)];

        let bundle = if !attachments.is_empty() {
            let rid = self.next_rid;
            self.next_rid += 1;
            let rb = RewardBundle {
                rid,
                source: 2,
                title: subj,
                send_time: now_ts,
                expire_time: now_ts + Self::DEFAULT_TTL_SECS,
                items: attachments,
            };
            self.tm_rewards
                .entry(target_uid)
                .or_default()
                .insert(0, rb.clone());
            pushes.push(PushPacket::new(
                PUSH_TM_REWARD_NEW,
                DtoValue::array([rb.to_dto()]),
                now_ts,
            ));
            Some(rb)
        } else {
            None
        };

        (mail, bundle, pushes)
    }

    /// Sends a player-to-player social mail (`mail.sendMail`).
    pub fn send_player_mail(
        &mut self,
        sender: &PlayerState,
        target_uid: u64,
        subject: impl Into<String>,
        content: impl Into<String>,
        now_ts: u64,
    ) -> (DtoValue, PushPacket) {
        let mid = self.next_mid;
        self.next_mid += 1;
        let mail = MailMessage {
            mid,
            template_id: 0,
            mail_type: MailType::Player,
            sender_uid: sender.uid,
            sender_uname: sender.uname.clone(),
            subject: subject.into(),
            content: content.into(),
            recv_time: now_ts,
            is_read: false,
            va_extra: BTreeMap::new(),
        };
        self.mails
            .entry(target_uid)
            .or_default()
            .insert(0, mail.clone());
        let push_pkt = PushPacket::new(push::MAIL_NEW, mail.to_dto(), now_ts);
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("mid", DtoValue::from(mid)),
        ]);
        (ret, push_pkt)
    }

    /// Sends a Time Machine player-to-player social mail (`tmMail.sendMail`).
    pub fn send_tm_player_mail(
        &mut self,
        sender: &PlayerState,
        target_uid: u64,
        subject: impl Into<String>,
        content: impl Into<String>,
        now_ts: u64,
    ) -> (DtoValue, PushPacket) {
        let mid = self.next_mid;
        self.next_mid += 1;
        let mail = MailMessage {
            mid,
            template_id: 0,
            mail_type: MailType::Player,
            sender_uid: sender.uid,
            sender_uname: sender.uname.clone(),
            subject: subject.into(),
            content: content.into(),
            recv_time: now_ts,
            is_read: false,
            va_extra: BTreeMap::new(),
        };
        self.tm_mails
            .entry(target_uid)
            .or_default()
            .insert(0, mail.clone());
        let push_pkt = PushPacket::new(PUSH_TM_MAIL_NEW, mail.to_dto(), now_ts);
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("mid", DtoValue::from(mid)),
        ]);
        (ret, push_pkt)
    }

    /// Lists player mails (`mail.getMailBoxList` / `mail.getMailList`), newest first.
    ///
    /// Matches `MailData:parseData(data)` (`MailData.lua:54-58`) which sets `self.list_original = data`
    /// and checks `if #self.list_original > 0` — MUST return a top-level `DtoValue::Array`!
    pub fn get_mail_list(&self, uid: u64) -> DtoValue {
        let list = self
            .mails
            .get(&uid)
            .map(|v| v.iter().map(MailMessage::to_dto).collect())
            .unwrap_or_default();
        DtoValue::Array(list)
    }

    /// Lists Time Machine player mails (`tmMail.getMailBoxList` / `tmMail.getMailList`) as a top-level Array.
    pub fn get_tm_mail_list(&self, uid: u64) -> DtoValue {
        let list = self
            .tm_mails
            .get(&uid)
            .map(|v| v.iter().map(MailMessage::to_dto).collect())
            .unwrap_or_default();
        DtoValue::Array(list)
    }

    /// Marks all mails read (`mail.readAllMail`).
    pub fn read_all_mail(&mut self, uid: u64) -> DtoValue {
        let mut marked = 0u32;
        if let Some(list) = self.mails.get_mut(&uid) {
            for m in list {
                if !m.is_read {
                    m.is_read = true;
                    marked += 1;
                }
            }
        }
        DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("read_count", DtoValue::from(marked)),
        ])
    }

    /// Marks all Time Machine mails read (`tmMail.readAllMail`).
    pub fn read_all_tm_mail(&mut self, uid: u64) -> DtoValue {
        let mut marked = 0u32;
        if let Some(list) = self.tm_mails.get_mut(&uid) {
            for m in list {
                if !m.is_read {
                    m.is_read = true;
                    marked += 1;
                }
            }
        }
        DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("read_count", DtoValue::from(marked)),
        ])
    }

    /// Draws items attached to a specific mail (`mail.drawMailItems`) idempotently.
    pub fn draw_mail_items(
        &mut self,
        player: &mut PlayerState,
        mid: u64,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let uid = player.uid;
        if self.drawn_mids.entry(uid).or_default().contains(&mid) {
            return Err("mail_items_already_drawn");
        }
        let Some(items) = self.mail_attachments.get(&(uid, mid)).cloned() else {
            return Err("mail_items_not_found");
        };
        let pushes = grant_rewards_to_player(player, &items, now_ts)?;
        self.drawn_mids.entry(uid).or_default().insert(mid);
        if let Some(list) = self.mails.get_mut(&uid) {
            if let Some(m) = list.iter_mut().find(|m| m.mid == mid) {
                m.is_read = true;
            }
        }
        if let Some(list) = self.tm_mails.get_mut(&uid) {
            if let Some(m) = list.iter_mut().find(|m| m.mid == mid) {
                m.is_read = true;
            }
        }
        let reward_arr = DtoValue::Array(items.into_iter().map(RewardEntry::to_triplet_dto).collect());
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("mid", DtoValue::from(mid)),
            ("reward", reward_arr),
        ]);
        Ok((ret, pushes))
    }

    /// Draws all unclaimed mail attachments (`mail.drawAllMailItems`) idempotently.
    pub fn draw_all_mail_items(
        &mut self,
        player: &mut PlayerState,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let uid = player.uid;
        let drawn = self.drawn_mids.entry(uid).or_default().clone();
        let pending: Vec<(u64, Vec<RewardEntry>)> = self
            .mail_attachments
            .iter()
            .filter(|&(&(u, mid), _)| u == uid && !drawn.contains(&mid))
            .map(|(&(_, mid), items)| (mid, items.clone()))
            .collect();

        if pending.is_empty() {
            return Ok((
                DtoValue::map([
                    ("ret", DtoValue::from("ok")),
                    ("drawn_mids", DtoValue::Array(Vec::new())),
                    ("reward", DtoValue::Array(Vec::new())),
                ]),
                Vec::new(),
            ));
        }

        let mut all_items = Vec::new();
        let mut mids = Vec::new();
        for (mid, items) in pending {
            mids.push(mid);
            all_items.extend(items);
        }
        let pushes = grant_rewards_to_player(player, &all_items, now_ts)?;
        let drawn_mut = self.drawn_mids.entry(uid).or_default();
        for &mid in &mids {
            drawn_mut.insert(mid);
        }
        if let Some(list) = self.mails.get_mut(&uid) {
            for m in list {
                if mids.contains(&m.mid) {
                    m.is_read = true;
                }
            }
        }
        if let Some(list) = self.tm_mails.get_mut(&uid) {
            for m in list {
                if mids.contains(&m.mid) {
                    m.is_read = true;
                }
            }
        }
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            (
                "drawn_mids",
                DtoValue::Array(mids.into_iter().map(DtoValue::from).collect()),
            ),
            (
                "reward",
                DtoValue::Array(all_items.into_iter().map(RewardEntry::to_triplet_dto).collect()),
            ),
        ]);
        Ok((ret, pushes))
    }

    /// Lists unclaimed, non-expired Reward Center bundles (`reward.getRewardList`, `args = [offset, limit]`).
    /// Matches `RewardData:setRewards(data)` which expects an array of reward dicts sorted by `send_time` DESC.
    pub fn get_reward_list(&self, uid: u64, offset: usize, limit: usize, now_ts: u64) -> DtoValue {
        let Some(list) = self.rewards.get(&uid) else {
            return DtoValue::Array(Vec::new());
        };
        let cap = if limit == 0 { 100 } else { limit };
        let items: Vec<DtoValue> = list
            .iter()
            .filter(|r| !r.is_expired(now_ts))
            .skip(offset)
            .take(cap)
            .map(RewardBundle::to_dto)
            .collect();
        DtoValue::Array(items)
    }

    /// Lists unclaimed, non-expired Time Machine Reward Center bundles (`tmReward.getRewardList`).
    pub fn get_tm_reward_list(&self, uid: u64, offset: usize, limit: usize, now_ts: u64) -> DtoValue {
        let Some(list) = self.tm_rewards.get(&uid) else {
            return DtoValue::Array(Vec::new());
        };
        let cap = if limit == 0 { 100 } else { limit };
        let items: Vec<DtoValue> = list
            .iter()
            .filter(|r| !r.is_expired(now_ts))
            .skip(offset)
            .take(cap)
            .map(RewardBundle::to_dto)
            .collect();
        DtoValue::Array(items)
    }

    /// Claims a single Reward Center bundle (`reward.receiveReward`, `args = [rid]`).
    pub fn receive_reward(
        &mut self,
        player: &mut PlayerState,
        rid: u64,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        self.receive_by_rid_arr(player, &[rid], now_ts)
    }

    /// Claims one or more Reward Center bundles (`reward.receiveByRidArr`, `args = [[rid1, rid2, ...]]`)
    /// idempotently, granting currencies/items/heroes and emitting live update pushes.
    pub fn receive_by_rid_arr(
        &mut self,
        player: &mut PlayerState,
        rids: &[u64],
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let uid = player.uid;
        let claimed_set = self.claimed_rids.entry(uid).or_default();
        let list = self.rewards.entry(uid).or_default();

        // Purge expired first
        list.retain(|r| !r.is_expired(now_ts));

        let mut to_grant = Vec::new();
        let mut matched_rids = Vec::new();
        for &rid in rids {
            if claimed_set.contains(&rid) {
                continue;
            }
            if let Some(bundle) = list.iter().find(|r| r.rid == rid) {
                to_grant.extend_from_slice(&bundle.items);
                matched_rids.push(rid);
            }
        }

        if matched_rids.is_empty() {
            return Err("reward_not_found_or_claimed");
        }

        let pushes = grant_rewards_to_player(player, &to_grant, now_ts)?;
        for &rid in &matched_rids {
            claimed_set.insert(rid);
        }
        list.retain(|r| !matched_rids.contains(&r.rid));

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            (
                "claimed_rids",
                DtoValue::Array(matched_rids.into_iter().map(DtoValue::from).collect()),
            ),
        ]);
        Ok((ret, pushes))
    }

    /// Claims one or more Time Machine Reward Center bundles (`tmReward.receiveByRidArr`).
    pub fn receive_tm_by_rid_arr(
        &mut self,
        player: &mut PlayerState,
        rids: &[u64],
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let uid = player.uid;
        let claimed_set = self.tm_claimed_rids.entry(uid).or_default();
        let list = self.tm_rewards.entry(uid).or_default();

        list.retain(|r| !r.is_expired(now_ts));

        let mut to_grant = Vec::new();
        let mut matched_rids = Vec::new();
        for &rid in rids {
            if claimed_set.contains(&rid) {
                continue;
            }
            if let Some(bundle) = list.iter().find(|r| r.rid == rid) {
                to_grant.extend_from_slice(&bundle.items);
                matched_rids.push(rid);
            }
        }

        if matched_rids.is_empty() {
            return Err("reward_not_found_or_claimed");
        }

        let pushes = grant_rewards_to_player(player, &to_grant, now_ts)?;
        for &rid in &matched_rids {
            claimed_set.insert(rid);
        }
        list.retain(|r| !matched_rids.contains(&r.rid));

        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            (
                "claimed_rids",
                DtoValue::Array(matched_rids.into_iter().map(DtoValue::from).collect()),
            ),
        ]);
        Ok((ret, pushes))
    }

    /// Redeems a gift code (`reward.giftByCode` / `tmReward.giftByCode`, `args = [code]`).
    pub fn redeem_gift_code(
        &mut self,
        player: &mut PlayerState,
        code: &str,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let norm = code.trim().to_ascii_uppercase();
        let Some(entries) = self.gift_codes.get(&norm).cloned() else {
            return Err("invalid_code");
        };
        let used = self.redeemed_codes.entry(player.uid).or_default();
        if used.contains(&norm) {
            return Err("already_redeemed");
        }
        let pushes = grant_rewards_to_player(player, &entries, now_ts)?;
        used.insert(norm);
        let items_dto = DtoValue::Array(
            entries
                .into_iter()
                .map(RewardEntry::to_triplet_dto)
                .collect(),
        );
        let ret = DtoValue::map([("ret", DtoValue::from("ok")), ("reward", items_dto)]);
        Ok((ret, pushes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_list_is_top_level_array_and_new_mail_push_is_map() {
        let mut center = MailAndRewardCenter::new();
        let mut p = PlayerState::new(1001, "ChuaCong", 1);
        let (mail, bundle, pushes) = center.send_system_mail(
            1001,
            0,
            "Quà Hệ Thống",
            "Thưởng đăng nhập",
            vec![RewardEntry::Gold(200)],
            1_700_000_000,
        );

        // 1. `re.mail.newMail` MUST push a direct Map (NOT an array), with `mail_type == 2`, `template_id >= 1`, and `read_time == 0`!
        let mail_push = pushes
            .iter()
            .find(|pk| pk.callback_name == push::MAIL_NEW)
            .expect("re.mail.newMail push");
        assert!(mail_push.ret.as_map().is_some(), "re.mail.newMail must be Map");
        assert_eq!(mail_push.ret.get("mail_type").and_then(DtoValue::as_u32), Some(2));
        assert_eq!(mail_push.ret.get("template_id").and_then(DtoValue::as_u32), Some(1));
        assert_eq!(mail_push.ret.get("read_time").and_then(DtoValue::as_u64), Some(0));
        let va_extra = mail_push.ret.get("va_extra").expect("va_extra");
        assert!(va_extra.get("data").and_then(DtoValue::as_array).is_some());
        assert!(va_extra.get("color").and_then(DtoValue::as_array).is_some());

        // 2. `mail.getMailBoxList` MUST return a top-level Array!
        let inbox = center.get_mail_list(1001);
        let arr = inbox.as_array().expect("top-level array for MailData:parseData");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].get("mid").and_then(DtoValue::as_u64), Some(mail.mid));

        // 3. `readAllMail` sets `read_time > 0`
        center.read_all_mail(1001);
        let inbox_after = center.get_mail_list(1001);
        assert!(
            inbox_after.as_array().unwrap()[0]
                .get("read_time")
                .and_then(DtoValue::as_u64)
                .unwrap()
                > 0
        );

        // 4. Player mail uses `MAIL_PLAYER = 1`
        let (_, p_push) = center.send_player_mail(&p, 1001, "Hi", "Xin chao", 1_700_000_010);
        assert_eq!(p_push.ret.get("mail_type").and_then(DtoValue::as_u32), Some(1));

        // 5. `RewardBundle` includes `[triplets, metadata_map]` in `va_reward` and `receive_reward` works
        let rb = bundle.expect("reward bundle");
        let dto = rb.to_dto();
        let va_reward = dto.get("va_reward").and_then(DtoValue::as_array).unwrap();
        assert_eq!(va_reward.len(), 2);
        assert_eq!(
            va_reward[1].get("description").and_then(DtoValue::as_str),
            Some("Quà Hệ Thống")
        );
        let gold_before = p.gold;
        let (claim_ret, _) = center.receive_reward(&mut p, rb.rid, 1_700_000_020).unwrap();
        assert_eq!(claim_ret.get("ret").and_then(DtoValue::as_str), Some("ok"));
        assert_eq!(p.gold, gold_before + 200);
    }

    #[test]
    fn draw_mail_items_and_tm_mail_reward_work_idempotently() {
        let mut center = MailAndRewardCenter::new();
        let mut p = PlayerState::new(1002, "Tester", 2);
        let (m1, _, _) = center.send_system_mail(
            1002,
            1,
            "Gift 1",
            "Body",
            vec![RewardEntry::Gold(150)],
            1_700_000_100,
        );
        let gold0 = p.gold;
        let (draw1, _) = center.draw_mail_items(&mut p, m1.mid, 1_700_000_101).unwrap();
        assert_eq!(draw1.get("ret").and_then(DtoValue::as_str), Some("ok"));
        assert_eq!(p.gold, gold0 + 150);
        assert_eq!(
            center.draw_mail_items(&mut p, m1.mid, 1_700_000_102).unwrap_err(),
            "mail_items_already_drawn"
        );

        let (_, tm_rb, tm_pushes) = center.send_tm_system_mail(
            1002,
            1,
            "TM Gift",
            "TM Body",
            vec![RewardEntry::Gold(300)],
            1_700_000_200,
        );
        assert_eq!(tm_pushes[0].callback_name, PUSH_TM_MAIL_NEW);
        assert_eq!(center.get_tm_mail_list(1002).as_array().unwrap().len(), 1);
        let rid = tm_rb.unwrap().rid;
        let (tm_claim, _) = center
            .receive_tm_by_rid_arr(&mut p, &[rid], 1_700_000_205)
            .unwrap();
        assert_eq!(tm_claim.get("ret").and_then(DtoValue::as_str), Some("ok"));
        assert_eq!(p.gold, gold0 + 450);
    }
}
