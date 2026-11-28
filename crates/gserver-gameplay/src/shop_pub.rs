//! Tavern Gacha (`raffle.*` from `RaffleGetPanel.lua`, `RaffleVipPanel.lua`, `raffle.db`)
//! and Shop subsystem (`shop.*` from `BaseShopData.lua`, `BaseShopProxy.lua`).

use std::collections::{BTreeMap, BTreeSet};

use crate::mail::{grant_rewards_to_player, RewardEntry};
use crate::player::{BagState, PlayerState};
use crate::protocol::{dto_map, dto_ok, push, DtoValue, PushPacket};

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

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.gen_below(items.len() as u32) as usize]
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
/// Red hero fragment/hero IDs from `raffle_red_hero`.
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

    pub fn get_info(&mut self, uid: u64, _now_ts: u64) -> DtoValue {
        let st = self.state_mut(uid);
        dto_map! {
            "warrior_free_cd" => st.warrior_free_ready_at,
            "warrior_free_num" => st.warrior_free_left_today,
            "divine_free_cd" => st.divine_free_ready_at,
            "hot_free_cd" => st.hot_free_ready_at,
            "warrior_pity" => 10 - (st.warrior_pity_counter % 10),
            "divine_pity" => 10 - (st.divine_pity_counter % 10),
            "first_ten_used" => u32::from(st.first_divine_ten_used),
            "luck" => st.divine_luck,
            "hot_luck" => st.hot_luck,
            "red_luck" => st.red_luck,
            "hot_index" => st.hot_spot_index,
            "hot_refresh_num" => st.hot_refresh_count,
        }
    }

    fn roll_warrior(rng: &mut Rng64, force_purple: bool) -> u32 {
        if force_purple {
            return rng.pick(PURPLE_HEROES);
        }
        let r = rng.gen_below(1000);
        rng.pick(if r < 75 {
            PURPLE_HEROES
        } else if r < 575 {
            BLUE_HEROES
        } else {
            GREEN_HEROES
        })
    }

    fn roll_divine(rng: &mut Rng64, force_orange: bool) -> u32 {
        if force_orange {
            return rng.pick(ORANGE_HEROES);
        }
        let r = rng.gen_below(1000);
        rng.pick(if r < 48 {
            ORANGE_HEROES
        } else if r < 348 {
            PURPLE_HEROES
        } else {
            BLUE_HEROES
        })
    }

    fn roll_luck_gain(rng: &mut Rng64) -> u32 {
        let r = rng.gen_below(10000);
        let mult = if r < 7000 { 1 } else if r < 9000 { 2 } else { 4 };
        10 * mult
    }

    /// Executes single or 10-pull recruitment on Pool 1 (`Warrior`) or Pool 2 (`Divine`).
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
                        st.warrior_free_ready_at = now_ts + 300;
                    } else if player.bag.count_by_tid(Self::TOKEN_WARRIOR) >= 1 {
                        bag_deltas.extend(player.bag.consume_item(Self::TOKEN_WARRIOR, 1)?);
                    } else {
                        return Err("not_enough_warrior_tokens");
                    }
                }
                (RafflePool::Warrior, 10) => {
                    if player.bag.count_by_tid(Self::TOKEN_WARRIOR) < 10 {
                        return Err("not_enough_warrior_tokens");
                    }
                    bag_deltas.extend(player.bag.consume_item(Self::TOKEN_WARRIOR, 10)?);
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

        let bonus_copper = match (pool, count) {
            (RafflePool::Warrior, 1) => 1_000,
            (RafflePool::Warrior, 10) => 10_000,
            (RafflePool::Divine, 1) => 2_000,
            _ => 20_000,
        };
        player.copper = (player.copper + bonus_copper).min(PlayerState::COPPER_CAP);

        let mut drawn_heroes_dto = Vec::with_capacity(count as usize);
        let mut luck_added = 0u32;
        for _ in 0..count {
            let st = self.states.entry(player.uid).or_default();
            let htid = match pool {
                RafflePool::Warrior => {
                    st.warrior_pity_counter += 1;
                    Self::roll_warrior(&mut self.rng, st.warrior_pity_counter % 10 == 0)
                }
                RafflePool::Divine => {
                    st.divine_pity_counter += 1;
                    let gain = Self::roll_luck_gain(&mut self.rng);
                    st.divine_luck = (st.divine_luck + gain).min(1500);
                    luck_added += gain;
                    Self::roll_divine(&mut self.rng, st.divine_pity_counter % 10 == 0)
                }
                _ => unreachable!(),
            };
            let (hero, hero_push) = player.roster.recruit_hero(htid, 1, now_ts);
            pushes.push(hero_push);
            drawn_heroes_dto.push(dto_map! {
                "hid" => hero.hid,
                "htid" => hero.htid,
                "quality" => hero.quality,
            });
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
        let ret = dto_ok! {
            "pool_id" => pool as u32,
            "heroes" => DtoValue::Array(drawn_heroes_dto),
            "luck" => cur_luck,
            "luck_add" => luck_added,
        };
        Ok((ret, pushes))
    }

    /// Executes Hot VIP (`raffle.raffleHot`, `raffle.multiRaffleHot`) or Red Hero (`raffle.raffleRed`) draws.
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
        let needed_gold = if count == 1 {
            if pool == RafflePool::HotLimit { 300 } else { 500 }
        } else {
            4000
        };

        let mut bag_deltas = Vec::new();
        let mut spent_gold = false;
        let st_free = self.state_mut(player.uid).hot_free_ready_at;

        if pool == RafflePool::HotLimit && count == 1 && now_ts >= st_free {
            self.state_mut(player.uid).hot_free_ready_at = now_ts + 172_800;
        } else if player.bag.count_by_tid(Self::TOKEN_RECRUIT_SEAL) >= needed_gold as u32 {
            bag_deltas.extend(player.bag.consume_item(Self::TOKEN_RECRUIT_SEAL, needed_gold as u32)?);
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
            let r = self.rng.gen_below(100);
            let (tid, qty) = if pool == RafflePool::HotLimit {
                st.hot_luck = (st.hot_luck + 10).min(1000);
                if r < 40 {
                    (self.rng.pick(ORANGE_HERO_FRAGS), 5)
                } else {
                    (self.rng.pick(BONUS_ITEMS), 2)
                }
            } else {
                st.red_luck = (st.red_luck + 10).min(1000);
                if r < 50 {
                    (self.rng.pick(RED_HERO_FRAGS), 5)
                } else if r < 70 {
                    (self.rng.pick(ORANGE_HERO_FRAGS), 5)
                } else {
                    (self.rng.pick(BONUS_ITEMS), 2)
                }
            };
            reward_entries.push(RewardEntry::Item { tid, qty });
            reward_dtos.push(dto_map! {
                "item" => dto_map! { tid.to_string() => qty },
            });
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

        let st = &self.states[&player.uid];
        let cur_luck = if pool == RafflePool::HotLimit { st.hot_luck } else { st.red_luck };
        Ok((
            dto_ok! {
                "reward" => DtoValue::Array(reward_dtos),
                "luck" => cur_luck,
            },
            pushes,
        ))
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
        Ok((dto_ok!(), pushes))
    }

    /// Claims a luck milestone box (`raffle.receiveLuckHeroGift`, `receiveHotLuckHeroGift`, `receiveRedLuckHeroGift`).
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
        let &req = thresholds
            .get(box_idx.saturating_sub(1) as usize)
            .ok_or("invalid_box_index")?;
        if cur_luck < req {
            return Err("luck_not_enough");
        }
        if !claimed.insert(box_idx) {
            return Err("box_already_claimed");
        }
        let pushes = grant_rewards_to_player(
            player,
            &[RewardEntry::Item { tid: reward_tid, qty: 5 }],
            now_ts,
        )?;
        Ok((dto_ok! { "box_idx" => box_idx }, pushes))
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
    const fn spec(
        idx: u32,
        id: u32,
        item_tid: u32,
        item_qty: u32,
        currency: ShopCurrency,
        unit_price: u64,
        max_buy: u32,
    ) -> Self {
        Self {
            idx,
            id,
            item_tid,
            item_qty,
            currency,
            unit_price,
            max_buy,
            bought: 0,
        }
    }

    pub fn to_dto(&self) -> DtoValue {
        dto_map! {
            "idx" => self.idx,
            "id" => self.id,
            "item_tid" => self.item_tid,
            "item_num" => self.item_qty,
            "price" => self.unit_price,
            "max_num" => self.max_buy,
            "buy_num" => self.bought,
        }
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

impl PlayerShopSheet {
    fn va_shop_dto(&self) -> DtoValue {
        DtoValue::Map(
            self.offers
                .iter()
                .map(|offer| (offer.idx.to_string(), offer.to_dto()))
                .collect(),
        )
    }
}

/// Multi-tab shop engine (`shop.getInfo`, `shop.buy`, `shop.refresh`, `shop.open`).
#[derive(Debug, Clone, Default)]
pub struct ShopEngine {
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
        let base = shop_type * 100;
        vec![
            ShopOffer::spec(1, base + 1, 16011, 1, ShopCurrency::Copper, 10_000, 5),
            ShopOffer::spec(2, base + 2, 16012, 1, currency, 250, 3),
            ShopOffer::spec(3, base + 3, 17002, 5, currency, 100, 10),
        ]
    }

    fn ensure_sheet(&mut self, uid: u64, shop_type: u32) -> &mut PlayerShopSheet {
        let seq = &mut self.seq;
        self.sheets.entry((uid, shop_type)).or_insert_with(|| {
            *seq += 1;
            PlayerShopSheet {
                shop_type,
                goods_key: format!("gk_{shop_type}_{}", *seq),
                refresh_free_num: 3,
                refresh_num: 0,
                offers: Self::default_offers_for_type(shop_type),
            }
        })
    }

    /// Returns exact `BaseShopData:parseData` table (`shop.getInfo`, `args = [shopType]`).
    pub fn get_info(&mut self, uid: u64, shop_type: u32, now_ts: u64) -> DtoValue {
        let sheet = self.ensure_sheet(uid, shop_type);
        let va_shop = sheet.va_shop_dto();
        dto_map! {
            "open" => true,
            "open_time" => now_ts,
            "refresh_time" => now_ts + 7_200,
            "refresh_free_num" => sheet.refresh_free_num,
            "refresh_num" => sheet.refresh_num,
            "refresh_item_num" => 0u32,
            "goods_key" => &sheet.goods_key,
            "free_num" => sheet.refresh_free_num,
            "last_free_num_time" => now_ts,
            "multi_tag" => 0u32,
            "va_shop" => va_shop,
            "va_quick_buy" => dto_map!(),
            "va_normal_auto_buy" => dto_map!(),
            "normal_auto_buy_gold_limit" => 0u32,
        }
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

        let delta_field = match currency {
            ShopCurrency::Copper => {
                if player.copper < total_cost as i64 {
                    return Err("not_enough_copper");
                }
                player.copper -= total_cost as i64;
                ("copper", DtoValue::from(player.copper))
            }
            ShopCurrency::Gold => {
                if player.gold < total_cost as i64 {
                    return Err("not_enough_gold");
                }
                player.gold -= total_cost as i64;
                ("gold", DtoValue::from(player.gold))
            }
            ShopCurrency::ArenaCoin => {
                if player.arena_coin < total_cost {
                    return Err("not_enough_arena_coin");
                }
                player.arena_coin -= total_cost;
                ("arena_coin", DtoValue::from(player.arena_coin))
            }
            ShopCurrency::HeroSoul => {
                if player.hero_soul < total_cost {
                    return Err("not_enough_hero_soul");
                }
                player.hero_soul -= total_cost;
                ("hero_soul", DtoValue::from(player.hero_soul))
            }
        };

        let bag_deltas = player.bag.add_item(item_tid, total_qty, now_ts)?;
        if let Some(offer) = self
            .ensure_sheet(player.uid, shop_type)
            .offers
            .iter_mut()
            .find(|o| o.idx == idx)
        {
            offer.bought += buy_num;
        }

        let mut pushes = vec![PlayerState::build_user_update_push([delta_field], now_ts)];
        if let Some(bp) = BagState::build_update_push(&bag_deltas, now_ts) {
            pushes.push(bp);
        }

        Ok((dto_ok!(), pushes))
    }

    /// Executes `shop.refresh` (`args = [shopType]` -> `{ ret = "ok", va_shop, goods_key }`).
    pub fn refresh(
        &mut self,
        player: &mut PlayerState,
        shop_type: u32,
        now_ts: u64,
    ) -> Result<(DtoValue, Vec<PushPacket>), &'static str> {
        let need_gold = self.ensure_sheet(player.uid, shop_type).refresh_free_num == 0;
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
        let new_key = format!("gk_{shop_type}_{}", self.seq);
        let sheet = self.ensure_sheet(player.uid, shop_type);
        if sheet.refresh_free_num > 0 {
            sheet.refresh_free_num -= 1;
        } else {
            sheet.refresh_num += 1;
        }
        sheet.goods_key = new_key.clone();
        sheet.offers = Self::default_offers_for_type(shop_type);
        let va_shop = sheet.va_shop_dto();

        Ok((
            dto_ok! {
                "goods_key" => &new_key,
                "va_shop" => va_shop,
            },
            pushes,
        ))
    }

    /// Emits `shop.open` push (`CS_SHOP_PUSH_UPDATE`) when a timed mystery shop triggers.
    pub fn build_mystery_shop_open_push(shop_type: u32, close_ts: u64, now_ts: u64) -> PushPacket {
        PushPacket::new(
            push::SHOP_OPEN,
            dto_map! {
                "shop_type" => shop_type,
                "open" => true,
                "close_time" => close_ts,
            },
            now_ts,
        )
    }
}
