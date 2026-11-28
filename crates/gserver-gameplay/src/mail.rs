//! Mail Box (`mail.getMailBoxList`, `mail.readAllMail`, `mail.sendMail`, `re.mail.newMail`)
//! and Reward Center (`reward.getRewardList`, `reward.receiveByRidArr`, `reward.giftByCode`, `re.reward.new`)
//! matching `NetConst.lua`, `RewardData.lua`, `RewardPanel.lua`, `RewardCell.lua`, and `mail_tmpl.db`.

use std::collections::{BTreeMap, BTreeSet};

use crate::player::{BagCategory, BagState, PlayerState};
use crate::protocol::{dto_arr, dto_map, dto_ok, push, DtoValue, PushPacket};

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
            Self::Copper(amt) => dto_arr![1, 0, amt],
            Self::Gold(amt) => dto_arr![2, 0, amt],
            Self::Item { tid, qty } => dto_arr![4, tid, qty],
            Self::HeroSoul(amt) => dto_arr![7, 0, amt],
            Self::ArenaCoin(amt) => dto_arr![8, 0, amt],
            Self::Hero { htid, count } => dto_arr![10, htid, count],
        }
    }

    pub fn slice_to_dto_array(entries: &[Self]) -> DtoValue {
        DtoValue::Array(entries.iter().copied().map(Self::to_triplet_dto).collect())
    }
}

/// Applies a batch of `RewardEntry` items atomically to `PlayerState`, returning live pushes
/// (`user.updateUser`, `bag.update`, `re.hero.new`). Checks bag capacity beforehand.
pub fn grant_rewards_to_player(
    player: &mut PlayerState,
    rewards: &[RewardEntry],
    now_ts: u64,
) -> Result<Vec<PushPacket>, &'static str> {
    for &entry in rewards {
        if let RewardEntry::Item { tid, qty } = entry {
            if qty > 0 && player.bag.count_by_tid(tid) == 0 {
                let cat = BagCategory::from_template_id(tid);
                if player.bag.is_category_full(cat) {
                    return Err("bag_full");
                }
            }
        }
    }

    let (mut gold_changed, mut copper_changed, mut copper_over_max) = (false, false, false);
    let (mut soul_changed, mut arena_changed) = (false, false);
    let mut bag_deltas = Vec::new();
    let mut pushes = Vec::new();

    for &entry in rewards {
        match entry {
            RewardEntry::Copper(amt) => {
                let next = player.copper.saturating_add(amt);
                copper_over_max |= next > PlayerState::COPPER_CAP;
                player.copper = next.min(PlayerState::COPPER_CAP);
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
                bag_deltas.extend(player.bag.add_item(tid, qty, now_ts)?);
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

/// Mail category matching `mail_tmpl.db` (`1`=System, `2`=Battle/Arena/Mine, `3`=Social/Friend/Guild).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailType {
    System = 1,
    Battle = 2,
    Social = 3,
}

/// Single notification mail entry (`mail.getMailBoxList` & `re.mail.newMail`).
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
    pub fn to_dto(&self) -> DtoValue {
        let extra: BTreeMap<String, DtoValue> = self
            .va_extra
            .iter()
            .map(|(k, v)| (k.clone(), DtoValue::from(v)))
            .collect();
        dto_map! {
            "mid" => self.mid,
            "template_id" => self.template_id,
            "mail_type" => self.mail_type as u32,
            "sender_uid" => self.sender_uid,
            "sender_uname" => &self.sender_uname,
            "subject" => &self.subject,
            "content" => &self.content,
            "recv_time" => self.recv_time,
            "status" => u32::from(self.is_read),
            "va_mail" => DtoValue::Map(extra),
        }
    }
}

/// Claimable attachment bundle in `RewardData.lua` / `RewardPanel.lua` (`reward.getRewardList`, `re.reward.new`).
///
/// Notice in `RewardPanel.lua` line 153: `v["va_reward"][1]` is the array of `[type, tid, num]` triplets!
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
        dto_map! {
            "rid" => self.rid,
            "source" => self.source,
            "title" => &self.title,
            "send_time" => self.send_time,
            "expire_time" => self.expire_time,
            "va_reward" => dto_arr![RewardEntry::slice_to_dto_array(&self.items)],
        }
    }
}

/// Per-player Mail Box + Reward Center + Gift Code redemption store.
#[derive(Debug, Clone)]
pub struct MailAndRewardCenter {
    next_mid: u64,
    next_rid: u64,
    mails: BTreeMap<u64, Vec<MailMessage>>,
    rewards: BTreeMap<u64, Vec<RewardBundle>>,
    claimed_rids: BTreeMap<u64, BTreeSet<u64>>,
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
        let mut center = Self {
            next_mid: 50_001,
            next_rid: 80_001,
            mails: BTreeMap::new(),
            rewards: BTreeMap::new(),
            claimed_rids: BTreeMap::new(),
            redeemed_codes: BTreeMap::new(),
            gift_codes: BTreeMap::new(),
        };
        center.register_gift_code(
            "GSERVERVIP888",
            vec![
                RewardEntry::Gold(888),
                RewardEntry::Copper(500_000),
                RewardEntry::Item { tid: 16012, qty: 5 },
            ],
        );
        center.register_gift_code(
            "TANTHU2026",
            vec![
                RewardEntry::Gold(500),
                RewardEntry::Item { tid: 16011, qty: 10 },
            ],
        );
        center
    }

    pub fn register_gift_code(&mut self, code: impl Into<String>, rewards: Vec<RewardEntry>) {
        self.gift_codes
            .insert(code.into().to_ascii_uppercase(), rewards);
    }

    fn insert_mail(&mut self, target_uid: u64, mail: MailMessage, now_ts: u64) -> PushPacket {
        let push_pkt = PushPacket::new(push::MAIL_NEW, dto_arr![mail.to_dto()], now_ts);
        self.mails.entry(target_uid).or_default().insert(0, mail);
        push_pkt
    }

    /// Pushes a notification mail (`re.mail.newMail`) and optional Reward Center attachment (`re.reward.new`).
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
        let mail = MailMessage {
            mid,
            template_id,
            mail_type: MailType::System,
            sender_uid: 0,
            sender_uname: "System".to_owned(),
            subject: subj.clone(),
            content: content.into(),
            recv_time: now_ts,
            is_read: false,
            va_extra: BTreeMap::new(),
        };
        let mut pushes = vec![self.insert_mail(target_uid, mail.clone(), now_ts)];

        let bundle = (!attachments.is_empty()).then(|| {
            let rid = self.next_rid;
            self.next_rid += 1;
            let rb = RewardBundle {
                rid,
                source: 1,
                title: subj,
                send_time: now_ts,
                expire_time: now_ts + Self::DEFAULT_TTL_SECS,
                items: attachments,
            };
            pushes.push(PushPacket::new(push::REWARD_NEW, dto_arr![rb.to_dto()], now_ts));
            self.rewards.entry(target_uid).or_default().insert(0, rb.clone());
            rb
        });

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
            mail_type: MailType::Social,
            sender_uid: sender.uid,
            sender_uname: player_name_or_default(&sender.uname),
            subject: subject.into(),
            content: content.into(),
            recv_time: now_ts,
            is_read: false,
            va_extra: BTreeMap::new(),
        };
        let push_pkt = self.insert_mail(target_uid, mail, now_ts);
        (dto_ok! { "mid" => mid }, push_pkt)
    }

    /// Lists player mails (`mail.getMailBoxList`), newest first.
    pub fn get_mail_list(&self, uid: u64) -> DtoValue {
        let list: Vec<DtoValue> = self
            .mails
            .get(&uid)
            .map(|v| v.iter().map(MailMessage::to_dto).collect())
            .unwrap_or_default();
        dto_map! {
            "mail_number" => list.len(),
            "list" => DtoValue::Array(list),
        }
    }

    /// Marks all mails read (`mail.readAllMail`).
    pub fn read_all_mail(&mut self, uid: u64) -> DtoValue {
        let mut marked = 0u32;
        if let Some(list) = self.mails.get_mut(&uid) {
            for m in list.iter_mut().filter(|m| !m.is_read) {
                m.is_read = true;
                marked += 1;
            }
        }
        dto_ok! { "read_count" => marked }
    }

    /// Lists unclaimed, non-expired Reward Center bundles (`reward.getRewardList`, `args = [offset, limit]`).
    pub fn get_reward_list(&self, uid: u64, offset: usize, limit: usize, now_ts: u64) -> DtoValue {
        let Some(list) = self.rewards.get(&uid) else {
            return dto_arr![];
        };
        let cap = if limit == 0 { 100 } else { limit };
        DtoValue::Array(
            list.iter()
                .filter(|r| !r.is_expired(now_ts))
                .skip(offset)
                .take(cap)
                .map(RewardBundle::to_dto)
                .collect(),
        )
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
        list.retain(|r| !r.is_expired(now_ts));

        let mut to_grant = Vec::new();
        let mut matched_rids = Vec::new();
        for &rid in rids {
            if !claimed_set.contains(&rid) {
                if let Some(bundle) = list.iter().find(|r| r.rid == rid) {
                    to_grant.extend_from_slice(&bundle.items);
                    matched_rids.push(rid);
                }
            }
        }
        if matched_rids.is_empty() {
            return Err("reward_not_found_or_claimed");
        }

        let pushes = grant_rewards_to_player(player, &to_grant, now_ts)?;
        claimed_set.extend(matched_rids.iter().copied());
        list.retain(|r| !matched_rids.contains(&r.rid));

        Ok((
            dto_ok! {
                "claimed_rids" => DtoValue::Array(matched_rids.into_iter().map(DtoValue::from).collect()),
            },
            pushes,
        ))
    }

    /// Redeems a gift code (`reward.giftByCode`, `args = [code]`).
    pub fn redeem_gift_code(
        &mut self,
        player: &mut PlayerState,
        code: &str,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let norm = code.trim().to_ascii_uppercase();
        let entries = self.gift_codes.get(&norm).cloned().ok_or("invalid_code")?;
        if self.redeemed_codes.entry(player.uid).or_default().contains(&norm) {
            return Err("already_redeemed");
        }
        let pushes = grant_rewards_to_player(player, &entries, now_ts)?;
        self.redeemed_codes.entry(player.uid).or_default().insert(norm);
        Ok((
            dto_ok! { "reward" => RewardEntry::slice_to_dto_array(&entries) },
            pushes,
        ))
    }
}

fn player_name_or_default(uname: &str) -> String {
    uname.to_owned()
}
