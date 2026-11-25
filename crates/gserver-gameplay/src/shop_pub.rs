//! Tavern Gacha (`raffle.*` from `RaffleGetPanel.lua`, `RaffleVipPanel.lua`, `raffle.db`)
//! and Shop subsystem (`shop.*` from `BaseShopData.lua`, `BaseShopProxy.lua`).

use std::collections::{BTreeMap, BTreeSet};

use crate::mail::{grant_rewards_to_player, RewardEntry};
use crate::player::{BagState, PlayerState};
use crate::protocol::{push, DtoValue, PushPacket};

/// Deterministic `XorShift64*` PRNG (`#![forbid(unsafe_code)]`, zero external crates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng64 {
    state: u64,
}

impl Rng64 {
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniformly samples in `0..bound` (`bound > 0`).
    pub fn gen_below(&mut self, bound: u32) -> u32 {
        (self.next_u64() % u64::from(bound.max(1))) as u32
    }
}

/// Tavern recruitment pool IDs matching `raffle.db` rows `1..=4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RafflePool {
    /// Pool 1 (`Rút Chiến Tướng`): Green 42.5%, Blue 50.0%, Purple 7.5%, 300s free CD, token `16011`.
    Warrior = 1,
    /// Pool 2 (`Rút Tướng` / Divine): Blue 65.2%, Purple 30.0%, Orange 4.8%, +10 luck/pull, token `16012` or `300`/`2850` gold.
    Divine = 2,
    /// Pool 3 (`Rút Hạn Giờ` / Hot VIP): Orange Frag 40%, Item 60%, +10 luck/pull, token `16013` or `300`/`4000` gold.
    HotLimit = 3,
    /// Pool 4 (`Rút Tướng Đỏ` / Red Hero): Red Frag 50%, Orange Frag 20%, Item 30%, token `16013` or `500`/`4000` gold.
    RedHero = 4,
}

impl RafflePool {
    pub fn from_u32(id: u32) -> Option<Self> {
        match id {
            1 => Some(Self::Warrior),
            2 => Some(Self::Divine),
            3 => Some(Self::HotLimit),
            4 => Some(Self::RedHero),
            _ => None,
        }
    }
}

// Hero template pools extracted directly from decrypted `raffle.db`:
const GREEN_HEROES: &[u32] = &[168, 169, 170, 263, 264, 365, 366, 464, 465];
const BLUE_HEROES: &[u32] = &[153, 154, 155, 251, 253, 356, 357, 454, 455];
const PURPLE_HEROES: &[u32] = &[121, 122, 123, 220, 221, 320, 321, 419, 420];
const ORANGE_HEROES: &[u32] = &[106, 406, 314, 113, 203, 313, 310, 114, 410, 112, 208];
/// Red hero fragment/hero IDs from `raffle_red_hero` (`Trương Liêu 20005`, `Tư Mã Ý 20003`, `Triệu Vân 20006`, `Gia Cát Lượng 20001`, `Chu Du 20007`, `Tôn Sách 20002`, `Lữ Bố 20008`, `Tả Từ 20004`).
const RED_HERO_FRAGS: &[u32] = &[20005, 20003, 20006, 20001, 20007, 20002, 20008, 20004];
const ORANGE_HERO_FRAGS: &[u32] = &[400106, 400203, 400310, 400406];
const BONUS_ITEMS: &[u32] = &[17002, 22102, 22202];

/// Per-player tavern gacha progression (`raffle.getInfo`).
#[derive(Debug, Clone)]
pub struct PlayerRaffleState {
    pub warrior_free_ready_at: u64,
    pub warrior_free_left_today: u32,
    pub divine_free_ready_at: u64,
    pub hot_free_ready_at: u64,
    pub warrior_pity_counter: u32,
    pub divine_pity_counter: u32,
    pub first_divine_ten_used: bool,
    pub divine_luck: u32,
    pub hot_luck: u32,
    pub red_luck: u32,
    pub claimed_divine_boxes: BTreeSet<u32>,
    pub claimed_hot_boxes: BTreeSet<u32>,
    pub claimed_red_boxes: BTreeSet<u32>,
    pub hot_spot_index: u32,
    pub hot_refresh_count: u32,
}

impl Default for PlayerRaffleState {
    fn default() -> Self {
        Self {
            warrior_free_ready_at: 0,
            warrior_free_left_today: 5,
            divine_free_ready_at: 0,
            hot_free_ready_at: 0,
            warrior_pity_counter: 0,
            divine_pity_counter: 0,
            first_divine_ten_used: false,
            divine_luck: 0,
            hot_luck: 0,
            red_luck: 0,
            claimed_divine_boxes: BTreeSet::new(),
            claimed_hot_boxes: BTreeSet::new(),
            claimed_red_boxes: BTreeSet::new(),
            hot_spot_index: 0,
            hot_refresh_count: 0,
        }
    }
}

/// Tavern Gacha Engine (`raffle.*`) faithfully implementing `raffle.db` rates, pity, tokens, and luck chests.
#[derive(Debug, Clone)]
pub struct TavernRaffleEngine {
    rng: Rng64,
    states: BTreeMap<u64, PlayerRaffleState>,
}

impl TavernRaffleEngine {
    pub const TOKEN_WARRIOR: u32 = 16011;
    pub const TOKEN_DIVINE: u32 = 16012;
    pub const TOKEN_RECRUIT_SEAL: u32 = 16013;

    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng64::new(seed),
            states: BTreeMap::new(),
        }
    }

    pub fn state_mut(&mut self, uid: u64) -> &mut PlayerRaffleState {
        self.states.entry(uid).or_default()
    }

    pub fn get_info(&mut self, uid: u64, now_ts: u64) -> DtoValue {
        let st = self.state_mut(uid);
        let raffle_sub = DtoValue::map([
            ("primary_free_num", DtoValue::from(0u32)),
            ("primary_free_time", DtoValue::from(st.warrior_free_ready_at)),
            ("primary_get_free_time", DtoValue::from(now_ts)),
            ("senior_free_num", DtoValue::from(0u32)),
            ("senior_free_time", DtoValue::from(st.divine_free_ready_at)),
            ("senior_get_free_time", DtoValue::from(now_ts)),
            ("primary_raffle_num", DtoValue::from(st.warrior_pity_counter)),
            ("senior_raffle_num", DtoValue::from(st.divine_pity_counter)),
            (
                "senior_first_muti_raffle",
                DtoValue::from(if st.first_divine_ten_used { 1u32 } else { 0u32 }),
            ),
            ("va_senior_luck_hero", DtoValue::Array(Vec::new())),
            ("senior_luck_num", DtoValue::from(st.divine_luck)),
        ]);
        DtoValue::map([
            ("raffle", raffle_sub),
            ("warrior_free_cd", DtoValue::from(st.warrior_free_ready_at)),
            ("warrior_free_num", DtoValue::from(st.warrior_free_left_today)),
            ("divine_free_cd", DtoValue::from(st.divine_free_ready_at)),
            ("hot_free_cd", DtoValue::from(st.hot_free_ready_at)),
            ("warrior_pity", DtoValue::from(10 - (st.warrior_pity_counter % 10))),
            ("divine_pity", DtoValue::from(10 - (st.divine_pity_counter % 10))),
            ("first_ten_used", DtoValue::from(if st.first_divine_ten_used { 1u32 } else { 0u32 })),
            ("luck", DtoValue::from(st.divine_luck)),
            ("hot_luck", DtoValue::from(st.hot_luck)),
            ("red_luck", DtoValue::from(st.red_luck)),
            ("hot_index", DtoValue::from(st.hot_spot_index)),
            ("hot_refresh_num", DtoValue::from(st.hot_refresh_count)),
        ])
    }

    fn roll_warrior(rng: &mut Rng64, force_purple: bool) -> u32 {
        if force_purple {
            return PURPLE_HEROES[rng.gen_below(PURPLE_HEROES.len() as u32) as usize];
        }
        // `raffle_probability` row 1: Purple 7.5% (75/1000), Blue 50.0% (500/1000), Green 42.5% (425/1000)
        let r = rng.gen_below(1000);
        if r < 75 {
            PURPLE_HEROES[rng.gen_below(PURPLE_HEROES.len() as u32) as usize]
        } else if r < 575 {
            BLUE_HEROES[rng.gen_below(BLUE_HEROES.len() as u32) as usize]
        } else {
            GREEN_HEROES[rng.gen_below(GREEN_HEROES.len() as u32) as usize]
        }
    }

    fn roll_divine(rng: &mut Rng64, force_orange: bool) -> u32 {
        if force_orange {
            return ORANGE_HEROES[rng.gen_below(ORANGE_HEROES.len() as u32) as usize];
        }
        // `raffle_probability` row 2: Orange 4.8% (48/1000), Purple 30.0% (300/1000), Blue 65.2% (652/1000)
        let r = rng.gen_below(1000);
        if r < 48 {
            ORANGE_HEROES[rng.gen_below(ORANGE_HEROES.len() as u32) as usize]
        } else if r < 348 {
            PURPLE_HEROES[rng.gen_below(PURPLE_HEROES.len() as u32) as usize]
        } else {
            BLUE_HEROES[rng.gen_below(BLUE_HEROES.len() as u32) as usize]
        }
    }

    fn roll_luck_gain(rng: &mut Rng64) -> u32 {
        // `raffle` row 2 `luck_boom`: 1x (70%), 2x (20%), 4x (10%) * base `get_luck = 10`
        let r = rng.gen_below(10000);
        let mult = if r < 7000 {
            1
        } else if r < 9000 {
            2
        } else {
            4
        };
        10 * mult
    }

    /// Executes single or 10-pull recruitment on Pool 1 (`Warrior`) or Pool 2 (`Divine`)
    /// (`raffle.singleRaffle`, `raffle.autoRaffle`, `raffle.multiRaffle`).
    pub fn draw_standard(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        count: u32,
        allow_free: bool,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if count != 1 && count != 10 {
            return Err("invalid_draw_count");
        }
        let mut pushes = Vec::new();
        let mut bag_deltas = Vec::new();
        let mut gold_spent = false;

        {
            let st = self.states.entry(player.uid).or_default();
            match (pool, count) {
                (RafflePool::Warrior, 1) => {
                    if allow_free && st.warrior_free_left_today > 0 && now_ts >= st.warrior_free_ready_at {
                        st.warrior_free_left_today -= 1;
                        st.warrior_free_ready_at = now_ts + 300; // 300s CD from `raffle.db`
                    } else if player.bag.count_by_tid(Self::TOKEN_WARRIOR) >= 1 {
                        bag_deltas.extend(player.bag.consume_item(Self::TOKEN_WARRIOR, 1)?);
                    } else {
                        return Err("not_enough_warrior_tokens");
                    }
                }
                (RafflePool::Warrior, 10) => {
                    if player.bag.count_by_tid(Self::TOKEN_WARRIOR) >= 10 {
                        bag_deltas.extend(player.bag.consume_item(Self::TOKEN_WARRIOR, 10)?);
                    } else {
                        return Err("not_enough_warrior_tokens");
                    }
                }
                (RafflePool::Divine, 1) => {
                    if allow_free && now_ts >= st.divine_free_ready_at {
                        st.divine_free_ready_at = now_ts + 86_400;
                    } else if player.bag.count_by_tid(Self::TOKEN_DIVINE) >= 1 {
                        bag_deltas.extend(player.bag.consume_item(Self::TOKEN_DIVINE, 1)?);
                    } else if player.gold >= 300 {
                        player.gold -= 300;
                        gold_spent = true;
                    } else {
                        return Err("not_enough_gold_or_tokens");
                    }
                }
                (RafflePool::Divine, 10) => {
                    let gold_cost = if !st.first_divine_ten_used { 900 } else { 2850 };
                    if player.bag.count_by_tid(Self::TOKEN_DIVINE) >= 10 {
                        bag_deltas.extend(player.bag.consume_item(Self::TOKEN_DIVINE, 10)?);
                    } else if player.gold >= gold_cost {
                        player.gold -= gold_cost;
                        gold_spent = true;
                        st.first_divine_ten_used = true;
                    } else {
                        return Err("not_enough_gold_or_tokens");
                    }
                }
                _ => return Err("unsupported_standard_pool"),
            }
        }

        // Grant copper bonus from `raffle.db` (`copper_one` / `copper_ten`)
        let bonus_copper = match (pool, count) {
            (RafflePool::Warrior, 1) => 1_000,
            (RafflePool::Warrior, 10) => 10_000,
            (RafflePool::Divine, 1) => 2_000,
            _ => 20_000,
        };
        player.copper = (player.copper + bonus_copper).min(PlayerState::COPPER_CAP);

        let mut drawn_heroes_dto = Vec::new();
        let mut luck_added = 0u32;
        for _ in 0..count {
            let st = self.states.entry(player.uid).or_default();
            let htid = match pool {
                RafflePool::Warrior => {
                    st.warrior_pity_counter += 1;
                    let force = st.warrior_pity_counter.is_multiple_of(10);
                    Self::roll_warrior(&mut self.rng, force)
                }
                RafflePool::Divine => {
                    st.divine_pity_counter += 1;
                    let force = st.divine_pity_counter.is_multiple_of(10);
                    let gain = Self::roll_luck_gain(&mut self.rng);
                    st.divine_luck = (st.divine_luck + gain).min(1500);
                    luck_added += gain;
                    Self::roll_divine(&mut self.rng, force)
                }
                _ => unreachable!(),
            };
            let (hero, hero_push) = player.roster.recruit_hero(htid, 1, now_ts);
            pushes.push(hero_push);
            drawn_heroes_dto.push(DtoValue::map([
                ("hid", DtoValue::from(hero.hid)),
                ("htid", DtoValue::from(hero.htid)),
                ("quality", DtoValue::from(hero.quality)),
            ]));
        }

        let mut user_fields = vec![
            ("copper", DtoValue::from(player.copper)),
            ("fight_force", DtoValue::from(player.fight_force())),
        ];
        if gold_spent {
            user_fields.push(("gold", DtoValue::from(player.gold)));
        }
        pushes.insert(0, PlayerState::build_user_update_push(user_fields, now_ts));
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }

        let cur_luck = self.states.get(&player.uid).map_or(0, |s| s.divine_luck);
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("pool_id", DtoValue::from(pool as u32)),
            ("heroes", DtoValue::Array(drawn_heroes_dto)),
            ("luck", DtoValue::from(cur_luck)),
            ("luck_add", DtoValue::from(luck_added)),
        ]);
        Ok((ret, pushes))
    }

    /// Executes Hot VIP (`raffle.raffleHot`, `raffle.multiRaffleHot`) or Red Hero (`raffle.raffleRed`) draws
    /// matching `RaffleVipPanel.lua` (`cbRaffleSucess`: `item` / `hero` reward dicts) and `raffle.db` rows 3 & 4.
    pub fn draw_limit_or_red(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        count: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if !matches!(pool, RafflePool::HotLimit | RafflePool::RedHero) || (count != 1 && count != 10) {
            return Err("invalid_limit_draw");
        }
        let single_gold = if pool == RafflePool::HotLimit { 300 } else { 500 };
        let ten_gold = 4000;
        let needed_gold = if count == 1 { single_gold } else { ten_gold };

        let mut bag_deltas = Vec::new();
        let mut spent_gold = false;
        let st_free = self
            .states
            .entry(player.uid)
            .or_default()
            .hot_free_ready_at;

        if pool == RafflePool::HotLimit && count == 1 && now_ts >= st_free {
            self.states.entry(player.uid).or_default().hot_free_ready_at = now_ts + 172_800;
        } else if player.bag.count_by_tid(Self::TOKEN_RECRUIT_SEAL) >= needed_gold as u32 {
            bag_deltas.extend(
                player
                    .bag
                    .consume_item(Self::TOKEN_RECRUIT_SEAL, needed_gold as u32)?,
            );
        } else if player.gold >= needed_gold {
            player.gold -= needed_gold;
            spent_gold = true;
        } else {
            return Err("not_enough_gold_or_seal");
        }

        let mut reward_entries = Vec::with_capacity(count as usize);
        let mut reward_dtos = Vec::with_capacity(count as usize);

        for _ in 0..count {
            let st = self.states.entry(player.uid).or_default();
            if pool == RafflePool::HotLimit {
                st.hot_luck = (st.hot_luck + 10).min(1000);
                // Row 3: 40% Orange Hero Frag, 60% Bonus Item
                let r = self.rng.gen_below(100);
                let (tid, qty) = if r < 40 {
                    (ORANGE_HERO_FRAGS[self.rng.gen_below(ORANGE_HERO_FRAGS.len() as u32) as usize], 5)
                } else {
                    (BONUS_ITEMS[self.rng.gen_below(BONUS_ITEMS.len() as u32) as usize], 2)
                };
                reward_entries.push(RewardEntry::Item { tid, qty });
                reward_dtos.push(DtoValue::map([(
                    "item",
                    DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
                )]));
            } else {
                st.red_luck = (st.red_luck + 10).min(1000);
                // Row 4: 50% Red Hero Frag, 20% Orange Frag, 30% Item
                let r = self.rng.gen_below(100);
                let (tid, qty) = if r < 50 {
                    (RED_HERO_FRAGS[self.rng.gen_below(RED_HERO_FRAGS.len() as u32) as usize], 5)
                } else if r < 70 {
                    (ORANGE_HERO_FRAGS[self.rng.gen_below(ORANGE_HERO_FRAGS.len() as u32) as usize], 5)
                } else {
                    (BONUS_ITEMS[self.rng.gen_below(BONUS_ITEMS.len() as u32) as usize], 2)
                };
                reward_entries.push(RewardEntry::Item { tid, qty });
                reward_dtos.push(DtoValue::map([(
                    "item",
                    DtoValue::map([(tid.to_string(), DtoValue::from(qty))]),
                )]));
            }
        }

        let mut pushes = grant_rewards_to_player(player, &reward_entries, now_ts)?;
        if spent_gold {
            pushes.insert(
                0,
                PlayerState::build_user_update_push([("gold", DtoValue::from(player.gold))], now_ts),
            );
        }
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }

        let st = self.states.get(&player.uid).unwrap();
        let cur_luck = if pool == RafflePool::HotLimit {
            st.hot_luck
        } else {
            st.red_luck
        };
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("reward", DtoValue::Array(reward_dtos)),
            ("luck", DtoValue::from(cur_luck)),
        ]);
        Ok((ret, pushes))
    }

    /// Buys recruitment seal tokens (`raffle.buyToken`, `args = [num]`).
    pub fn buy_token(
        &mut self,
        player: &mut PlayerState,
        qty: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if qty == 0 {
            return Err("invalid_qty");
        }
        let cost = i64::from(qty) * 10;
        if player.gold < cost {
            return Err("not_enough_gold");
        }
        player.gold -= cost;
        let deltas = player.bag.add_item(Self::TOKEN_RECRUIT_SEAL, qty, now_ts)?;
        let mut pushes = vec![PlayerState::build_user_update_push(
            [("gold", DtoValue::from(player.gold))],
            now_ts,
        )];
        if let Some(bp) = BagState::build_update_push(&deltas, now_ts) {
            pushes.push(bp);
        }
        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    /// Claims a luck milestone box (`raffle.receiveLuckHeroGift` `[500, 1000, 1500]`
    /// or `raffle.receiveHotLuckHeroGift` / `raffle.receiveRedLuckHeroGift` `[250, 500, 750, 1000]`).
    pub fn claim_luck_box(
        &mut self,
        player: &mut PlayerState,
        pool: RafflePool,
        box_idx: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let st = self.states.entry(player.uid).or_default();
        let (thresholds, cur_luck, claimed, reward_tid): (&[u32], u32, &mut BTreeSet<u32>, u32) = match pool {
            RafflePool::Divine => (&[500, 1000, 1500], st.divine_luck, &mut st.claimed_divine_boxes, 29002),
            RafflePool::HotLimit => (&[250, 500, 750, 1000], st.hot_luck, &mut st.claimed_hot_boxes, 400106),
            RafflePool::RedHero => (&[250, 500, 750, 1000], st.red_luck, &mut st.claimed_red_boxes, 20008),
            RafflePool::Warrior => return Err("pool_has_no_luck_box"),
        };
        let i = box_idx.saturating_sub(1) as usize;
        let &req = thresholds.get(i).ok_or("invalid_box_index")?;
        if cur_luck < req {
            return Err("luck_not_enough");
        }
        if claimed.contains(&box_idx) {
            return Err("box_already_claimed");
        }
        claimed.insert(box_idx);
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Item {
                tid: reward_tid,
                qty: 5,
            }],
            now_ts,
        )?;
        Ok((
            DtoValue::map([
                ("ret", DtoValue::from("ok")),
                ("box_idx", DtoValue::from(box_idx)),
            ]),
            pushes,
        ))
    }
}

// ============================================================================
// Shop Subsystem (`BaseShopData.lua`, `BaseShopProxy.lua`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopCurrency {
    Copper,
    Gold,
    ArenaCoin,
    HeroSoul,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopOffer {
    pub idx: u32,
    pub id: u32,
    pub item_tid: u32,
    pub item_qty: u32,
    pub currency: ShopCurrency,
    pub unit_price: u64,
    pub max_buy: u32,
    pub bought: u32,
}

impl ShopOffer {
    pub fn to_dto(&self) -> DtoValue {
        DtoValue::map([
            ("idx", DtoValue::from(self.idx)),
            ("id", DtoValue::from(self.id)),
            ("item_tid", DtoValue::from(self.item_tid)),
            ("item_num", DtoValue::from(self.item_qty)),
            ("price", DtoValue::from(self.unit_price)),
            ("max_num", DtoValue::from(self.max_buy)),
            ("buy_num", DtoValue::from(self.bought)),
        ])
    }
}

#[derive(Debug, Clone)]
pub struct PlayerShopSheet {
    pub shop_type: u32,
    pub goods_key: String,
    pub refresh_free_num: u32,
    pub refresh_num: u32,
    pub offers: Vec<ShopOffer>,
}

/// Multi-tab shop engine (`shop.getInfo`, `shop.buy`, `shop.refresh`, `shop.open`).
#[derive(Debug, Clone, Default)]
pub struct ShopEngine {
    /// `(uid, shop_type) -> PlayerShopSheet`
    sheets: BTreeMap<(u64, u32), PlayerShopSheet>,
    seq: u64,
}

impl ShopEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn default_offers_for_type(shop_type: u32) -> Vec<ShopOffer> {
        let currency = match shop_type {
            2 => ShopCurrency::ArenaCoin,
            3 => ShopCurrency::HeroSoul,
            _ => ShopCurrency::Gold,
        };
        // Keep `101..103` for shop_type 1 (tested by E2E), and use real DB ware IDs (`1..3` / `10001..10003`)
        // for all other shops so `ShopItem:initShopData` finds valid rows in `shop/*.db`.
        let base_id = match shop_type {
            1 => 100,
            3 | 14 | 19 | 20 | 21 | 22 | 23 => 10_000,
            _ => 0,
        };
        vec![
            ShopOffer {
                idx: 1,
                id: base_id + 1,
                item_tid: 16011,
                item_qty: 1,
                currency: ShopCurrency::Copper,
                unit_price: 10_000,
                max_buy: 5,
                bought: 0,
            },
            ShopOffer {
                idx: 2,
                id: base_id + 2,
                item_tid: 16012,
                item_qty: 1,
                currency,
                unit_price: 250,
                max_buy: 3,
                bought: 0,
            },
            ShopOffer {
                idx: 3,
                id: base_id + 3,
                item_tid: 17002,
                item_qty: 5,
                currency,
                unit_price: 100,
                max_buy: 10,
                bought: 0,
            },
        ]
    }

    fn ensure_sheet(&mut self, uid: u64, shop_type: u32) -> &mut PlayerShopSheet {
        let seq = &mut self.seq;
        self.sheets.entry((uid, shop_type)).or_insert_with(|| {
            *seq += 1;
            PlayerShopSheet {
                shop_type,
                // Numeric string `"0"` so Lua `self.shopItemsGroupMap[self.goods_key + 1]` resolves to index 1
                // while remaining a valid string token for `shop.buy` anti-stale checks.
                goods_key: "0".to_string(),
                refresh_free_num: 3,
                refresh_num: 0,
                offers: Self::default_offers_for_type(shop_type),
            }
        })
    }

    /// Encodes `va_shop` as the 2-level nested array expected by `BaseShopData:getShopData()`
    /// (`for key, group in pairs(self.shopData) do for _, item in pairs(group) do ... end end`).
    fn encode_va_shop(offers: &[ShopOffer], goods_key: &str) -> DtoValue {
        let group: Vec<DtoValue> = offers.iter().map(ShopOffer::to_dto).collect();
        let group_idx = goods_key.parse::<usize>().unwrap_or(0);
        let mut groups = Vec::with_capacity(group_idx + 1);
        for _ in 0..group_idx {
            groups.push(DtoValue::Array(Vec::new()));
        }
        groups.push(DtoValue::Array(group));
        DtoValue::Array(groups)
    }

    /// Returns exact `BaseShopData:parseData` table (`shop.getInfo`, `args = [shopType]`).
    pub fn get_info(&mut self, uid: u64, shop_type: u32, now_ts: u64) -> DtoValue {
        let sheet = self.ensure_sheet(uid, shop_type);
        let va_shop = Self::encode_va_shop(&sheet.offers, &sheet.goods_key);
        DtoValue::map([
            ("open", DtoValue::from(true)),
            ("open_time", DtoValue::from(now_ts)),
            ("refresh_time", DtoValue::from(now_ts + 7_200)),
            ("refresh_free_num", DtoValue::from(sheet.refresh_free_num)),
            ("refresh_num", DtoValue::from(sheet.refresh_num)),
            ("refresh_item_num", DtoValue::from(0u32)),
            ("goods_key", DtoValue::from(sheet.goods_key.as_str())),
            ("free_num", DtoValue::from(sheet.refresh_free_num)),
            ("last_free_num_time", DtoValue::from(now_ts)),
            ("multi_tag", DtoValue::from(0u32)),
            ("va_shop", va_shop),
            ("va_quick_buy", DtoValue::Map(BTreeMap::new())),
            ("va_normal_auto_buy", DtoValue::Map(BTreeMap::new())),
            ("normal_auto_buy_gold_limit", DtoValue::from(0u32)),
        ])
    }

    /// Executes `shop.buy` (`args = [shopType, goodsKey, idx, id, buyNum]` -> `{ ret = "ok" }`).
    #[allow(clippy::too_many_arguments)]
    pub fn buy(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        goods_key: &str,
        idx: u32,
        offer_id: u32,
        buy_num: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        if buy_num == 0 {
            return Err("invalid_buy_num");
        }
        let (item_tid, total_qty, currency, total_cost) = {
            let sheet = self.ensure_sheet(player.uid, shop_type);
            if !goods_key.is_empty() && sheet.goods_key != goods_key {
                return Err("stale_goods_key");
            }
            let offer = sheet
                .offers
                .iter()
                .find(|o| o.idx == idx && (offer_id == 0 || o.id == offer_id))
                .ok_or("offer_not_found")?;
            if offer.bought + buy_num > offer.max_buy {
                return Err("exceed_buy_limit");
            }
            (
                offer.item_tid,
                offer.item_qty * buy_num,
                offer.currency,
                offer.unit_price * u64::from(buy_num),
            )
        };

        let mut user_delta = Vec::new();
        match currency {
            ShopCurrency::Copper => {
                if player.copper < total_cost as i64 {
                    return Err("not_enough_copper");
                }
                player.copper -= total_cost as i64;
                user_delta.push(("copper", DtoValue::from(player.copper)));
            }
            ShopCurrency::Gold => {
                if player.gold < total_cost as i64 {
                    return Err("not_enough_gold");
                }
                player.gold -= total_cost as i64;
                user_delta.push(("gold", DtoValue::from(player.gold)));
            }
            ShopCurrency::ArenaCoin => {
                if player.arena_coin < total_cost {
                    return Err("not_enough_arena_coin");
                }
                player.arena_coin -= total_cost;
                user_delta.push(("arena_coin", DtoValue::from(player.arena_coin)));
            }
            ShopCurrency::HeroSoul => {
                if player.hero_soul < total_cost {
                    return Err("not_enough_hero_soul");
                }
                player.hero_soul -= total_cost;
                user_delta.push(("hero_soul", DtoValue::from(player.hero_soul)));
            }
        }

        let bag_deltas = player.bag.add_item(item_tid, total_qty, now_ts)?;
        let sheet = self.ensure_sheet(player.uid, shop_type);
        if let Some(offer) = sheet.offers.iter_mut().find(|o| o.idx == idx) {
            offer.bought += buy_num;
        }

        let mut pushes = vec![PlayerState::build_user_update_push(user_delta, now_ts)];
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }

        Ok((DtoValue::map([("ret", DtoValue::from("ok"))]), pushes))
    }

    /// Executes `shop.refresh` (`args = [shopType]` -> `{ ret = "ok", va_shop, goods_key }`).
    pub fn refresh(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let need_gold = {
            let sheet = self.ensure_sheet(player.uid, shop_type);
            sheet.refresh_free_num == 0
        };
        let mut pushes = Vec::new();
        if need_gold {
            if player.gold < 20 {
                return Err("not_enough_gold");
            }
            player.gold -= 20;
            pushes.push(PlayerState::build_user_update_push(
                [("gold", DtoValue::from(player.gold))],
                now_ts,
            ));
        }
        self.seq += 1;
        let new_key = self.seq.to_string();
        let sheet = self.ensure_sheet(player.uid, shop_type);
        if sheet.refresh_free_num > 0 {
            sheet.refresh_free_num -= 1;
        } else {
            sheet.refresh_num += 1;
        }
        sheet.goods_key = new_key.clone();
        sheet.offers = Self::default_offers_for_type(shop_type);

        let va_shop = Self::encode_va_shop(&sheet.offers, &new_key);
        let ret = DtoValue::map([
            ("ret", DtoValue::from("ok")),
            ("goods_key", DtoValue::from(new_key.as_str())),
            ("va_shop", va_shop),
        ]);
        Ok((ret, pushes))
    }

    /// Emits `shop.open` push (`CS_SHOP_PUSH_UPDATE`) when a timed mystery shop triggers.
    pub fn build_mystery_shop_open_push(shop_type: u32, close_ts: u64, now_ts: u64) -> PushPacket {
        PushPacket::new(
            push::SHOP_OPEN,
            DtoValue::map([
                ("shop_type", DtoValue::from(shop_type)),
                ("open", DtoValue::from(true)),
                ("close_time", DtoValue::from(close_ts)),
            ]),
            now_ts,
        )
    }
}
