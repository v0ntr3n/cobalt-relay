//! Mail Box (`mail.getMailBoxList`, `mail.readAllMail`, `mail.sendMail`, `re.mail.newMail`)
//! and Reward Center (`reward.getRewardList`, `reward.receiveByRidArr`, `reward.giftByCode`, `re.reward.new`)
//! matching `NetConst.lua`, `RewardData.lua`, `RewardPanel.lua`, `RewardCell.lua`, and `mail_tmpl.db`.

use std::collections::{BTreeMap, BTreeSet};

use crate::player::{BagCategory, BagState, PlayerState};
use crate::protocol::{push, DtoValue, PushPacket};

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
            Self::Copper(amt) => DtoValue::array([DtoValue::from(1), DtoValue::from(0), DtoValue::from(amt)]),
            Self::Gold(amt) => DtoValue::array([DtoValue::from(2), DtoValue::from(0), DtoValue::from(amt)]),
            Self::Item { tid, qty } => {
                DtoValue::array([DtoValue::from(4), DtoValue::from(tid), DtoValue::from(qty)])
            }
            Self::HeroSoul(amt) => DtoValue::array([DtoValue::from(7), DtoValue::from(0), DtoValue::from(amt)]),
            Self::ArenaCoin(amt) => DtoValue::array([DtoValue::from(8), DtoValue::from(0), DtoValue::from(amt)]),
            Self::Hero { htid, count } => {
                DtoValue::array([DtoValue::from(10), DtoValue::from(htid), DtoValue::from(count)])
            }
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
        let mut extra = BTreeMap::new();
        for (k, v) in &self.va_extra {
            extra.insert(k.clone(), DtoValue::from(v.as_str()));
        }
        DtoValue::map([
            ("mid", DtoValue::from(self.mid)),
            ("template_id", DtoValue::from(self.template_id)),
            ("mail_type", DtoValue::from(self.mail_type as u32)),
            ("sender_uid", DtoValue::from(self.sender_uid)),
            ("sender_uname", DtoValue::from(self.sender_uname.as_str())),
            ("subject", DtoValue::from(self.subject.as_str())),
            ("content", DtoValue::from(self.content.as_str())),
            ("recv_time", DtoValue::from(self.recv_time)),
            ("status", DtoValue::from(if self.is_read { 1u32 } else { 0u32 })),
            ("va_mail", DtoValue::Map(extra)),
        ])
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
        let triplets = DtoValue::Array(
            self.items
                .iter()
                .copied()
                .map(RewardEntry::to_triplet_dto)
                .collect(),
        );
        DtoValue::map([
            ("rid", DtoValue::from(self.rid)),
            ("source", DtoValue::from(self.source)),
            ("title", DtoValue::from(self.title.as_str())),
            ("send_time", DtoValue::from(self.send_time)),
            ("expire_time", DtoValue::from(self.expire_time)),
            // `va_reward[1]` in Lua -> outer 1-element array containing the list of triplets!
            ("va_reward", DtoValue::array([triplets])),
        ])
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
                RewardEntry::Item { tid: 16011, qty: 10 },
            ],
        );
        Self {
            next_mid: 50_001,
            next_rid: 80_001,
            mails: BTreeMap::new(),
            rewards: BTreeMap::new(),
            claimed_rids: BTreeMap::new(),
            redeemed_codes: BTreeMap::new(),
            gift_codes,
        }
    }

    pub fn register_gift_code(&mut self, code: impl Into<String>, rewards: Vec<RewardEntry>) {
        self.gift_codes
            .insert(code.into().to_ascii_uppercase(), rewards);
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
        self.mails.entry(target_uid).or_default().insert(0, mail.clone());

        let mut pushes = vec![PushPacket::new(
            push::MAIL_NEW,
            DtoValue::array([mail.to_dto()]),
            now_ts,
        )];

        let bundle = if !attachments.is_empty() {
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
            self.rewards.entry(target_uid).or_default().insert(0, rb.clone());
            // `RewardData:onRewardNew(ret)` iterates `for _, reward in pairs(ret)`!
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
            sender_uname: sender.uname.clone(),
            subject: subject.into(),
            content: content.into(),
            recv_time: now_ts,
            is_read: false,
            va_extra: BTreeMap::new(),
        };
        self.mails.entry(target_uid).or_default().insert(0, mail.clone());
        let push_pkt = PushPacket::new(push::MAIL_NEW, DtoValue::array([mail.to_dto()]), now_ts);
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("mid", DtoValue::from(mid)),
        ]);
        (ret, push_pkt)
    }

    /// Lists player mails (`mail.getMailBoxList`), newest first.
    pub fn get_mail_list(&self, uid: u64) -> DtoValue {
        let list = self
            .mails
            .get(&uid)
            .map(|v| v.iter().map(MailMessage::to_dto).collect())
            .unwrap_or_default();
        DtoValue::map([
            ("mail_number", DtoValue::from(self.mails.get(&uid).map_or(0, Vec::len))),
            ("list", DtoValue::Array(list)),
        ])
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

    /// Lists unclaimed, non-expired Reward Center bundles (`reward.getRewardList`, `args = [offset, limit]`).
    /// Matches `RewardData:parseData(data)` which expects an array of reward dicts sorted by `send_time` DESC.
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

    /// Redeems a gift code (`reward.giftByCode`, `args = [code]`).
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
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("reward", items_dto),
        ]);
        Ok((ret, pushes))
    }
}
