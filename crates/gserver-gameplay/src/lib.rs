#![forbid(unsafe_code)]
//! `gserver-gameplay` — Pure-Rust gameplay & GM command engine for **GServer (`vnsg2`)**.
//!
//! Covers:
//! - `protocol`: Delegated AMF3/Zlib/MD5/`LCPacket` wire codecs (`gserver-protocol`), `NetConst.lua` method aliases (`gserver-rpc-catalog`), `dto_map!`/`dto_arr!`/`dto_ok!` macros, and typed `RpcRequest` extractors.
//! - `player`: `PlayerState`, `BagState` (`bag.bagInfo` & `bag.update`), `HeroRoster` & `HeroFormationData` (`re.hero.new`, `user.updateUser`, `MainTown` safety invariants).
//! - `town`: `MainTown` bootstrap validator + multiplayer walking rooms (`city.*`, `imperialcity.*`, `town.addPlayer`, `town.delPlayer`, `town.move`, `town.update`).
//! - `chat_gm`: Multi-channel chat (`chat.sendWorld`, `chat.sendPersonal`, `*LastMsgs` AMF3 blobs, `re.chat.getMsg`) + In-Game GM Slash-Command Processor (`/help`, `/gold`, `/silver`, `/vip`, `/level`, `/item`, `/hero`, `/mail`, `/heal`).
//! - `mail`: Notification `MailBox` (`mail.*`, `re.mail.newMail`) + `RewardCenter` (`reward.getRewardList`, `reward.receiveByRidArr`, `reward.giftByCode`, `re.reward.new`).
//! - `shop_pub`: Deterministic Tavern Gacha (`raffle.*`, `raffle.db` probabilities/pity/luck chests) + Multi-Currency `ShopEngine` (`shop.*`).
//! - `battle_copy`: Crash-proof Turn-Based Battle Simulator (`FightProxy` / `BaseUnit` / `PropertyKey`, `appraisal = 50` on win), PvE Campaign (`copy.*`), `ArenaEngine` (`arena.*`), and `SignAndOnlineService` (`sign.*`, `online.*`).
//! - `engine`: Stateful `GameplayEngine` RPC router returning responses + caller pushes + peer broadcasts.

pub mod protocol;
pub mod battle_copy;
pub mod chat_gm;
pub mod engine;
pub mod mail;
pub mod player;
pub mod shop_pub;
pub mod town;

#[cfg(test)]
mod tests {
    use crate::battle_copy::{simulate_battle, CombatUnit};
    use crate::chat_gm::{parse_gm_command, ChatErrorCode, GmCommand, GmParseError};
    use crate::engine::GameplayEngine;
    use crate::mail::RewardEntry;
    use crate::player::{BagCategory, BagDelta, PlayerState};
    use crate::protocol::{
        bundle_multi_envelope, decode_amf3, dto_arr, dto_map, dto_ok, encode_amf3, push, rpc,
        CodecError, DtoValue, LcPacketFrame, RpcRequest,
    };
    use crate::town::{validate_maintown_bootstrap, MainTownCrashRisk, TownMapConfig};

    const NOW: u64 = 1_795_500_500;

    macro_rules! rpc_call {
        ($method:expr, $id:expr $(,)?) => {
            RpcRequest::new($method, Vec::new(), $id)
        };
        ($method:expr, [$($arg:expr),+ $(,)?], $id:expr $(,)?) => {
            RpcRequest::new($method, vec![$(DtoValue::from($arg)),+], $id)
        };
    }

    fn make_engine_with_two_players() -> GameplayEngine {
        let mut eng = GameplayEngine::new(42);
        let mut p1 = PlayerState::new(1001, "ChuaCongA", 106);
        p1.gold = 10_000;
        p1.copper = 500_000;
        let mut p2 = PlayerState::new(1002, "ChuaCongB", 203);
        p2.level = 50;
        p2.vip = 5;
        eng.register_player(p1);
        eng.register_player(p2);
        eng
    }

    // 1. AMF3 roundtrip over nested maps, arrays, ints, floats, booleans, strings, and byte blobs
    #[test]
    fn test_01_amf3_nested_roundtrip() {
        let original = dto_map! {
            "err" => "ok",
            "neg" => -12345i64,
            "big" => 5_000_000_000i64,
            "pi" => 3.25f64,
            "flag" => true,
            "nothing" => DtoValue::Null,
            "raw" => DtoValue::Bytes(vec![1, 2, 3, 254]),
            "arr" => dto_arr![10, "xin chao"],
        };
        let bytes = encode_amf3(&original);
        let decoded = decode_amf3(&bytes).expect("AMF3 decode must succeed");
        assert_eq!(decoded, original);
        assert_eq!(decoded.get("err").and_then(DtoValue::as_str), Some("ok"));
        assert_eq!(decoded.get("neg").and_then(DtoValue::as_i64), Some(-12345));
        assert_eq!(
            decoded.get("big").and_then(DtoValue::as_i64),
            Some(5_000_000_000)
        );
        assert_eq!(decoded.get("flag").and_then(DtoValue::as_bool), Some(true));
        assert_eq!(
            decoded.get("raw").and_then(DtoValue::as_bytes),
            Some(&[1u8, 2, 3, 254][..])
        );
    }

    // 2. LCPacket 24-byte wire framing roundtrip
    #[test]
    fn test_02_lcpacket_24byte_header_roundtrip() {
        let body = encode_amf3(&dto_map! { "hello" => 3u32 });
        let frame = LcPacketFrame::uncompressed(body.clone(), true);
        let wire = frame.to_wire_bytes();
        assert_eq!(wire.len(), 24 + body.len());
        let parsed = LcPacketFrame::from_wire_bytes(&wire).expect("valid 24-byte LCPacket");
        assert_eq!(parsed.flags, [0, 1, 0, 1]);
        assert_eq!(parsed.payload, body);
    }

    // 3. Multi-envelope bundling (`flag[3] != 0`) combines RPC reply + pushes cleanly
    #[test]
    fn test_03_multi_envelope_bundling() {
        let mut eng = make_engine_with_two_players();
        let req = rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/gold 777"], 1);
        let out = eng.dispatch(1001, &req, NOW);
        let bundled = bundle_multi_envelope(&out.response, &out.client_pushes, NOW);
        let arr = bundled.get("ret").and_then(DtoValue::as_array).unwrap();
        assert_eq!(arr.len(), 1 + out.client_pushes.len());
    }

    // 4. MainTown bootstrap validator catches short `server_open_time` (< 14 digits)
    #[test]
    fn test_04_maintown_validator_catches_bad_server_open_time() {
        let mut p = PlayerState::new(1, "Tester", 106);
        assert!(validate_maintown_bootstrap(&p).is_ok());
        p.server_open_time = "2026-01".to_owned();
        assert_eq!(
            validate_maintown_bootstrap(&p),
            Err(MainTownCrashRisk::InvalidServerOpenTime("2026-01".to_owned()))
        );
    }

    // 5. MainTown bootstrap validator catches missing protagonist or dangling formation hid
    #[test]
    fn test_05_maintown_validator_catches_dangling_formation_hid() {
        let mut p = PlayerState::new(1, "Tester", 106);
        p.roster.formation[1] = 999_999;
        assert_eq!(
            validate_maintown_bootstrap(&p),
            Err(MainTownCrashRisk::DanglingFormationHero(999_999))
        );
        p.roster.formation[0] = 0;
        assert_eq!(
            validate_maintown_bootstrap(&p),
            Err(MainTownCrashRisk::MissingProtagonistFormation)
        );
    }

    // 6. `formation.getPositionInfo` returns 6-slot position + 6x6 spirit + 6x4 yanWu tables
    #[test]
    fn test_06_formation_position_info_dto_schema() {
        let mut eng = make_engine_with_two_players();
        let out = eng.dispatch(1001, &rpc_call!(rpc::FORMATION_GET_POSITION_INFO, 2), NOW);
        assert_eq!(out.response.err, "ok");
        let pos = out.response.ret.get("position").and_then(DtoValue::as_array).unwrap();
        assert_eq!(pos.len(), 6);
        assert_ne!(pos[0].as_u64().unwrap(), 0);
        assert_eq!(out.response.ret.get("spirit").and_then(DtoValue::as_array).unwrap().len(), 6);
        assert_eq!(out.response.ret.get("yanWu").and_then(DtoValue::as_array).unwrap().len(), 6);
    }

    // 7. Entering `city.enter` spawns player and broadcasts `town.addPlayer` to peers
    #[test]
    fn test_07_town_enter_and_peer_add_broadcast() {
        let mut eng = make_engine_with_two_players();
        let r1 = eng.dispatch(1001, &rpc_call!(rpc::CITY_ENTER, [1u32], 10), NOW);
        assert_eq!(r1.response.err, "ok");
        assert!(r1.peer_pushes.is_empty());

        let r2 = eng.dispatch(1002, &rpc_call!(rpc::CITY_ENTER, [1u32], 11), NOW);
        assert_eq!(r2.response.err, "ok");
        assert_eq!(r2.peer_pushes.len(), 1);
        assert_eq!(r2.peer_pushes[0].0, 1001);
        assert_eq!(r2.peer_pushes[0].1.callback_name, push::TOWN_ADD_PLAYER);
        assert_eq!(r2.response.ret.get("enter_list").and_then(DtoValue::as_array).unwrap().len(), 2);
    }

    // 8. `imperialcity.move` clamps coordinates to `CCRect(50, 0, 2720, 342)` (`[50..2770] x [0..342]`)
    #[test]
    fn test_08_royalcity_move_clamps_bounds_and_sets_direction() {
        let mut eng = make_engine_with_two_players();
        eng.dispatch(1001, &rpc_call!(rpc::ROYALCITY_ENTER, 1), NOW);
        eng.dispatch(1002, &rpc_call!(rpc::ROYALCITY_ENTER, 2), NOW);

        let mv1 = eng.dispatch(1001, &rpc_call!(rpc::ROYALCITY_MOVE, [9999, 999], 3), NOW);
        assert_eq!(mv1.response.ret.get("x").and_then(DtoValue::as_i64), Some(2770));
        assert_eq!(mv1.response.ret.get("y").and_then(DtoValue::as_i64), Some(342));
        assert_eq!(mv1.response.ret.get("dir").and_then(DtoValue::as_i64), Some(1));
        assert_eq!(mv1.peer_pushes[0].1.callback_name, push::TOWN_MOVE);

        let mv2 = eng.dispatch(1001, &rpc_call!(rpc::ROYALCITY_MOVE, [-20, -5], 4), NOW);
        assert_eq!(mv2.response.ret.get("x").and_then(DtoValue::as_i64), Some(50));
        assert_eq!(mv2.response.ret.get("y").and_then(DtoValue::as_i64), Some(0));
        assert_eq!(mv2.response.ret.get("dir").and_then(DtoValue::as_i64), Some(-1));
    }

    // 9. `city.leave` removes occupant and broadcasts `town.delPlayer`
    #[test]
    fn test_09_town_leave_broadcasts_del_player() {
        let mut eng = make_engine_with_two_players();
        eng.dispatch(1001, &rpc_call!(rpc::CITY_ENTER, [1], 1), NOW);
        eng.dispatch(1002, &rpc_call!(rpc::CITY_ENTER, [1], 2), NOW);

        let lv = eng.dispatch(1001, &rpc_call!(rpc::CITY_LEAVE, 3), NOW);
        assert_eq!(lv.peer_pushes.len(), 1);
        assert_eq!(lv.peer_pushes[0].0, 1002);
        assert_eq!(lv.peer_pushes[0].1.callback_name, push::TOWN_DEL_PLAYER);

        let enter_list = eng.dispatch(1002, &rpc_call!(rpc::CITY_GET_ENTER_LIST, 4), NOW);
        assert_eq!(enter_list.response.ret.as_array().unwrap().len(), 1);
    }

    // 10. Town sharding automatically overflows player 11 into instance #2 for Đào Nguyên Thôn (`cap = 10`)
    #[test]
    fn test_10_town_room_capacity_sharding() {
        let mut eng = GameplayEngine::new(7);
        assert_eq!(TownMapConfig::preset(1).max_visible_players, 10);
        for i in 1..=11u64 {
            eng.register_player(PlayerState::new(i, format!("P{i}"), 106));
            let out = eng.dispatch(i, &rpc_call!(rpc::CITY_ENTER, [1], i as u32), NOW);
            let inst = out.response.ret.get("instance_id").and_then(DtoValue::as_u64).unwrap();
            assert_eq!(inst, if i <= 10 { 1 } else { 2 });
        }
    }

    // 11. Town level requirement gate (`Lạc Dương` `town_id = 2` requires `level >= 5`)
    #[test]
    fn test_11_town_min_level_requirement() {
        let mut eng = make_engine_with_two_players();
        let r1 = eng.dispatch(1001, &rpc_call!(rpc::CITY_ENTER, [2], 1), NOW);
        assert_eq!(r1.response.err, "level_not_enough");
        let r2 = eng.dispatch(1002, &rpc_call!(rpc::CITY_ENTER, [2], 2), NOW);
        assert_eq!(r2.response.err, "ok");
    }

    // 12. Bag item stacking, consumption, and empty-dict deletion push (`bag.update`)
    #[test]
    fn test_12_bag_add_stack_consume_and_delete_push() {
        let mut p = PlayerState::new(1, "BagUser", 106);
        let d1 = p.bag.add_item(16012, 3, NOW).unwrap();
        let d2 = p.bag.add_item(16012, 2, NOW).unwrap();
        assert_eq!(p.bag.count_by_tid(16012), 5);
        assert_eq!(p.bag.used_grids(BagCategory::Item), 1);
        assert!(matches!(d1[0], BagDelta::Upsert(_)));
        assert!(matches!(d2[0], BagDelta::Upsert(_)));

        let d3 = p.bag.consume_item(16012, 5).unwrap();
        assert_eq!(p.bag.count_by_tid(16012), 0);
        assert!(matches!(d3[0], BagDelta::Remove { .. }));

        let push_pkt = crate::player::BagState::build_update_push(&d3, NOW).unwrap();
        assert_eq!(push_pkt.callback_name, push::BAG_UPDATE);
        let first_arg = &push_pkt.ret.as_array().unwrap()[0];
        let deleted_slot = first_arg.get("30001").unwrap().as_map().unwrap();
        assert!(deleted_slot.is_empty(), "deleted gid must push empty map");
    }

    // 13. GM parser unit tests covering all 9 commands + error cases
    #[test]
    fn test_13_gm_command_parser_all_variants() {
        assert_eq!(parse_gm_command("/help"), Ok(GmCommand::Help));
        assert_eq!(parse_gm_command("/gold 5000"), Ok(GmCommand::Gold(5000)));
        assert_eq!(parse_gm_command("/silver 100000"), Ok(GmCommand::Silver(100000)));
        assert_eq!(parse_gm_command("/vip 12"), Ok(GmCommand::Vip(12)));
        assert_eq!(parse_gm_command("/level 85"), Ok(GmCommand::Level(85)));
        assert_eq!(
            parse_gm_command("/item 16012 20"),
            Ok(GmCommand::Item { tid: 16012, qty: 20 })
        );
        assert_eq!(parse_gm_command("/hero 20008"), Ok(GmCommand::Hero { htid: 20008 }));
        assert_eq!(
            parse_gm_command("/mail 17002 5"),
            Ok(GmCommand::Mail { tid: 17002, qty: 5 })
        );
        assert_eq!(parse_gm_command("/heal"), Ok(GmCommand::Heal));
        assert_eq!(parse_gm_command("hello world"), Err(GmParseError::NotAGmCommand));
        assert!(matches!(
            parse_gm_command("/unknown"),
            Err(GmParseError::UnknownCommand(_))
        ));
    }

    // 14. GM `/gold` and `/silver` live `user.updateUser` + `re.user.copperOverMax` pushes
    #[test]
    fn test_14_gm_gold_and_silver_pushes_and_cap() {
        let mut eng = make_engine_with_two_players();
        let out_gold = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/gold 25000"], 1), NOW);
        assert_eq!(eng.players[&1001].gold, 35_000);
        assert!(out_gold.client_pushes.iter().any(|p| p.callback_name == push::USER_UPDATE));
        assert!(out_gold.client_pushes.iter().any(|p| p.callback_name == push::CHAT_GET_MSG));

        let out_silver = eng.dispatch(
            1001,
            &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/silver 2000000000"], 2),
            NOW,
        );
        assert_eq!(eng.players[&1001].copper, PlayerState::COPPER_CAP);
        assert!(out_silver
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::USER_COPPER_OVER_MAX));
    }

    // 15. GM `/vip` and `/level` update formation slots AND broadcast `town.update` to town peers
    #[test]
    fn test_15_gm_vip_and_level_sync_town_peers() {
        let mut eng = make_engine_with_two_players();
        eng.dispatch(1001, &rpc_call!(rpc::CITY_ENTER, [1], 1), NOW);
        eng.dispatch(1002, &rpc_call!(rpc::CITY_ENTER, [1], 2), NOW);

        let out_vip = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/vip 10"], 3), NOW);
        assert_eq!(eng.players[&1001].vip, 10);
        assert_eq!(out_vip.peer_pushes.len(), 1);
        assert_eq!(out_vip.peer_pushes[0].1.callback_name, push::TOWN_UPDATE);

        let out_lv = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/level 60"], 4), NOW);
        assert_eq!(eng.players[&1001].level, 60);
        let pos_map = eng.players[&1001].hero_position_map();
        assert_eq!(pos_map.get("6").and_then(DtoValue::as_bool), Some(true));
        assert_eq!(out_lv.peer_pushes[0].1.callback_name, push::TOWN_UPDATE);
    }

    // 16. GM `/item`, `/hero`, `/mail`, and `/heal` emit exact client pushes
    #[test]
    fn test_16_gm_item_hero_mail_and_heal_pushes() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1001).unwrap().execution = 5;
        let main_hid = eng.players[&1001].roster.formation[0];
        eng.players
            .get_mut(&1001)
            .unwrap()
            .roster
            .heroes
            .get_mut(&main_hid)
            .unwrap()
            .cur_hp = 10;

        let out_item = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/item 16012 15"], 1), NOW);
        assert!(out_item.client_pushes.iter().any(|p| p.callback_name == push::BAG_UPDATE));
        assert_eq!(eng.players[&1001].bag.count_by_tid(16012), 15);

        let out_hero = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/hero 20008"], 2), NOW);
        assert!(out_hero.client_pushes.iter().any(|p| p.callback_name == push::HERO_NEW));
        assert_eq!(eng.players[&1001].roster.heroes.len(), 2);

        let out_mail = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/mail 16011 8"], 3), NOW);
        assert!(out_mail.client_pushes.iter().any(|p| p.callback_name == push::MAIL_NEW));
        assert!(out_mail.client_pushes.iter().any(|p| p.callback_name == push::REWARD_NEW));

        let out_heal = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/heal"], 4), NOW);
        assert!(out_heal.client_pushes.iter().any(|p| p.callback_name == push::USER_UPDATE));
        assert_eq!(eng.players[&1001].execution, 120);
        assert_eq!(
            eng.players[&1001].roster.heroes[&main_hid].cur_hp,
            eng.players[&1001].roster.heroes[&main_hid].max_hp
        );
    }

    // 17. World chat gate (`level < 40 && vip < 2` returns `10004` `NotOpen`) vs Lv.50 broadcast + AMF3 history
    #[test]
    fn test_17_world_chat_gate_broadcast_and_amf3_last_msgs() {
        let mut eng = make_engine_with_two_players();
        let blocked = eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "Hello world!"], 1), NOW);
        assert_eq!(
            blocked.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::NotOpen as i64)
        );

        let sent1 = eng.dispatch(1002, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "First message"], 2), NOW);
        assert_eq!(
            sent1.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::Succ as i64)
        );
        assert_eq!(sent1.peer_pushes.len(), 1);
        assert_eq!(sent1.peer_pushes[0].0, 1001);
        assert_eq!(sent1.peer_pushes[0].1.callback_name, push::CHAT_GET_MSG);

        eng.dispatch(1002, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "Second message"], 3), NOW + 1);

        let hist = eng.dispatch(1001, &rpc_call!(rpc::CHAT_GET_WORLD_LAST, 4), NOW + 2);
        let blobs = hist.response.ret.as_array().unwrap();
        assert_eq!(blobs.len(), 2);
        let newest = decode_amf3(blobs[0].as_bytes().unwrap()).unwrap();
        assert_eq!(
            newest
                .get("message")
                .and_then(|m| m.get("message"))
                .and_then(DtoValue::as_str),
            Some("Second message")
        );
    }

    // 18. Private whisper (`chat.sendPersonal`) handles online delivery and offline `10001` code
    #[test]
    fn test_18_personal_chat_online_and_offline() {
        let mut eng = make_engine_with_two_players();
        let ok_whisper = eng.dispatch(
            1002,
            &rpc_call!(rpc::CHAT_SEND_PERSONAL, ["ChuaCongA", 1, "Secret tip", false], 1),
            NOW,
        );
        assert_eq!(
            ok_whisper.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::Succ as i64)
        );
        assert_eq!(ok_whisper.peer_pushes[0].0, 1001);

        let offline = eng.dispatch(
            1002,
            &rpc_call!(rpc::CHAT_SEND_PERSONAL, ["GhostUser", 1, "Hi"], 2),
            NOW,
        );
        assert_eq!(
            offline.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::Offline as i64)
        );
    }

    // 19. Mail list, readAllMail, Reward Center `va_reward[1]` schema, and idempotent `receiveByRidArr`
    #[test]
    fn test_19_mail_and_reward_center_idempotent_claim() {
        let mut eng = make_engine_with_two_players();
        let (_, bundle, _) = eng.mail_center.send_system_mail(
            1001,
            5,
            "Arena Daily Settlement",
            "Rank #1 reward",
            vec![
                RewardEntry::Gold(300),
                RewardEntry::Item { tid: 16012, qty: 2 },
            ],
            NOW,
        );
        let rid = bundle.unwrap().rid;

        let list_out = eng.dispatch(1001, &rpc_call!(rpc::REWARD_GET_LIST, [0, 100], 1), NOW);
        let rewards = list_out.response.ret.as_array().unwrap();
        assert_eq!(rewards.len(), 1);
        let va_reward = rewards[0].get("va_reward").and_then(DtoValue::as_array).unwrap();
        assert_eq!(va_reward.len(), 1);
        assert_eq!(va_reward[0].as_array().unwrap().len(), 2);

        let gold_before = eng.players[&1001].gold;
        let claim1 = eng.dispatch(
            1001,
            &rpc_call!(rpc::REWARD_RECEIVE_BY_RID_ARR, [dto_arr![rid]], 2),
            NOW,
        );
        assert_eq!(claim1.response.err, "ok");
        assert_eq!(eng.players[&1001].gold, gold_before + 300);
        assert_eq!(eng.players[&1001].bag.count_by_tid(16012), 2);

        let claim2 = eng.dispatch(
            1001,
            &rpc_call!(rpc::REWARD_RECEIVE_BY_RID_ARR, [dto_arr![rid]], 3),
            NOW,
        );
        assert_eq!(claim2.response.err, "reward_not_found_or_claimed");

        let read_all = eng.dispatch(1001, &rpc_call!(rpc::MAIL_READ_ALL, 4), NOW);
        assert_eq!(
            read_all.response.ret.get("read_count").and_then(DtoValue::as_i64),
            Some(1)
        );
    }

    // 20. Gift code redemption (`reward.giftByCode`) grants rewards once and rejects duplicate reuse
    #[test]
    fn test_20_gift_code_redemption_once_per_player() {
        let mut eng = make_engine_with_two_players();
        let r1 = eng.dispatch(1001, &rpc_call!(rpc::REWARD_GIFT_BY_CODE, ["gservervip888"], 1), NOW);
        assert_eq!(r1.response.err, "ok");
        assert_eq!(eng.players[&1001].gold, 10_888);

        let r2 = eng.dispatch(1001, &rpc_call!(rpc::REWARD_GIFT_BY_CODE, ["GSERVERVIP888"], 2), NOW);
        assert_eq!(r2.response.err, "already_redeemed");
    }

    // 21. Tavern Divine 10-pull (`raffle.multiRaffle`) applies first-10 discount (900 gold), guarantees Orange hero, and accrues luck
    #[test]
    fn test_21_tavern_divine_multi_raffle_first_ten_and_orange_pity() {
        let mut eng = make_engine_with_two_players();
        let out = eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_MULTI, [2u32], 1), NOW);
        assert_eq!(out.response.err, "ok");
        assert_eq!(eng.players[&1001].gold, 9_100);
        let heroes = out.response.ret.get("heroes").and_then(DtoValue::as_array).unwrap();
        assert_eq!(heroes.len(), 10);
        assert!(heroes
            .iter()
            .any(|h| h.get("quality").and_then(DtoValue::as_u64) == Some(5)));
        assert!(out.response.ret.get("luck").and_then(DtoValue::as_u64).unwrap() >= 100);

        let out2 = eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_MULTI, [2u32], 2), NOW);
        assert_eq!(out2.response.err, "ok");
        assert_eq!(eng.players[&1001].gold, 6_250);
    }

    // 22. Tavern Warrior free draw cooldown (`300s`) and token fallback
    #[test]
    fn test_22_tavern_warrior_free_cooldown_and_token() {
        let mut eng = make_engine_with_two_players();
        assert_eq!(eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_SINGLE, [1u32], 1), NOW).response.err, "ok");
        assert_eq!(
            eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_SINGLE, [1u32], 2), NOW + 10)
                .response
                .err,
            "not_enough_warrior_tokens"
        );
        assert_eq!(
            eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_SINGLE, [1u32], 3), NOW + 301)
                .response
                .err,
            "ok"
        );
    }

    // 23. Tavern Hot VIP & Red Hero draws + Luck milestone chest (`raffle.receiveHotLuckHeroGift`)
    #[test]
    fn test_23_tavern_hot_and_red_draws_and_luck_chest() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1001).unwrap().gold = 200_000;
        for i in 0..3u32 {
            assert_eq!(eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_MULTI_HOT, 10 + i), NOW).response.err, "ok");
        }
        assert_eq!(
            eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_RECEIVE_LUCK_HOT, [1u32], 20), NOW)
                .response
                .err,
            "ok"
        );

        let red = eng.dispatch(1001, &rpc_call!(rpc::RAFFLE_RED, [10u32], 21), NOW);
        assert_eq!(red.response.err, "ok");
        assert_eq!(
            red.response.ret.get("reward").and_then(DtoValue::as_array).unwrap().len(),
            10
        );
    }

    // 24. Shop `getInfo`, `buy` with `goods_key` anti-stale validation, and `refresh`
    #[test]
    fn test_24_shop_buy_goods_key_validation_and_refresh() {
        let mut eng = make_engine_with_two_players();
        let info = eng.dispatch(1001, &rpc_call!(rpc::SHOP_GET_INFO, [1u32], 1), NOW);
        let gk = info
            .response
            .ret
            .get("goods_key")
            .and_then(DtoValue::as_str)
            .unwrap()
            .to_owned();

        let buy_ok = eng.dispatch(
            1001,
            &rpc_call!(rpc::SHOP_BUY, [1u32, gk.as_str(), 2u32, 102u32, 2u32], 2),
            NOW,
        );
        assert_eq!(buy_ok.response.err, "ok");
        assert_eq!(eng.players[&1001].bag.count_by_tid(16012), 2);

        assert_eq!(eng.dispatch(1001, &rpc_call!(rpc::SHOP_REFRESH, [1u32], 3), NOW).response.err, "ok");
        let stale_buy = eng.dispatch(
            1001,
            &rpc_call!(rpc::SHOP_BUY, [1u32, gk.as_str(), 2u32, 102u32, 1u32], 4),
            NOW,
        );
        assert_eq!(stale_buy.response.err, "stale_goods_key");
    }

    // 25. Battle simulator produces crash-proof `brid`, `userInfo[1..2]`, `arrTeam1`/`arrTeam2`, and `appraisal == 50`
    #[test]
    fn test_25_battle_simulator_crash_proof_dto_and_appraisal_50() {
        let p = PlayerState::new(1001, "Hero", 106);
        let t1 = vec![CombatUnit::from_hero(p.roster.heroes.values().next().unwrap(), 0)];
        let t2 = vec![CombatUnit::monster(9001, 5001, 0, 1)];
        let u1 = dto_map! { "uid" => 1001u64, "fight_force" => 5000u64, "dress_id" => 0u32 };
        let u2 = dto_map! { "uid" => 0u64, "fight_force" => 1000u64, "dress_id" => 0u32 };
        let sim = simulate_battle(888, false, &t1, &t2, u1, u2, &[]);
        assert!(sim.is_win);
        assert_eq!(sim.appraisal, 50, "FightProxy:isSuccessFight requires appraisal == 50");
        assert_eq!(sim.server_attack_dto.get("brid").and_then(DtoValue::as_u64), Some(888));
        assert_eq!(
            sim.server_attack_dto.get("userInfo").and_then(DtoValue::as_array).unwrap().len(),
            2
        );
        let report_decoded = decode_amf3(&sim.to_amf3_report_bytes()).unwrap();
        assert_eq!(report_decoded.get("appraisal").and_then(DtoValue::as_u64), Some(50));
    }

    // 26. PvE Campaign Copy full flow: `copy.attack` -> `copy.pass` -> `copy.sweep` -> `copy.fetchBoxReward` -> `copy.fetchPrize`
    #[test]
    fn test_26_pve_copy_attack_pass_sweep_and_prizes() {
        let mut eng = make_engine_with_two_players();
        eng.dispatch(1001, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/level 20"], 1), NOW);

        let atk = eng.dispatch(1001, &rpc_call!(rpc::COPY_ATTACK, [1001u32, 100101u32], 2), NOW);
        assert_eq!(atk.response.err, "ok");
        let brid = atk.response.ret.get("brid").and_then(DtoValue::as_u64).unwrap();
        assert!(brid > 0);

        let pass = eng.dispatch(
            1001,
            &rpc_call!(rpc::COPY_PASS, [1001u32, 100101u32, dto_map!()], 3),
            NOW,
        );
        assert_eq!(pass.response.err, "ok");
        assert_eq!(pass.response.ret.get("star").and_then(DtoValue::as_u64), Some(3));

        assert_eq!(
            eng.dispatch(1001, &rpc_call!(rpc::COPY_SWEEP, [1001u32, 100101u32, 2u32, 0u32], 4), NOW)
                .response
                .err,
            "ok"
        );
        assert_eq!(
            eng.dispatch(1001, &rpc_call!(rpc::COPY_FETCH_BOX_REWARD, [1001u32, 100101u32], 5), NOW)
                .response
                .ret
                .as_str(),
            Some("ok")
        );
        assert_eq!(
            eng.dispatch(1001, &rpc_call!(rpc::COPY_FETCH_PRIZE, [1001u32, 0u32], 6), NOW)
                .response
                .err,
            "ok"
        );
        assert!(eng
            .dispatch(1001, &rpc_call!(rpc::BATTLE_GET_RECORD, [brid], 7), NOW)
            .response
            .ret
            .as_bytes()
            .is_some());
    }

    // 27. Tutorial `user.beforLoginBattle` works at Lv.1 without consuming execution
    #[test]
    fn test_27_tutorial_before_login_battle() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1001).unwrap().execution = 0;
        let tut = eng.dispatch(1001, &rpc_call!(rpc::USER_BEFORE_LOGIN_BATTLE, 1), NOW);
        assert_eq!(tut.response.err, "ok");
        assert!(tut.response.ret.get("brid").and_then(DtoValue::as_u64).is_some());
    }

    // 28. Arena `enterArena` + `arena.challenge` swaps rank, emits AMF3 `battleRecord`, and pushes `re.arena.dataRefresh`
    #[test]
    fn test_28_arena_challenge_rank_swap_and_defender_refresh_push() {
        let mut eng = make_engine_with_two_players();
        let e1 = eng.dispatch(1001, &rpc_call!(rpc::ARENA_ENTER, 1), NOW);
        assert_eq!(
            e1.response.ret.get("res").and_then(|r| r.get("position")).and_then(DtoValue::as_u64),
            Some(16)
        );
        let e2 = eng.dispatch(1002, &rpc_call!(rpc::ARENA_ENTER, 2), NOW);
        assert_eq!(
            e2.response.ret.get("res").and_then(|r| r.get("position")).and_then(DtoValue::as_u64),
            Some(17)
        );

        let ch = eng.dispatch(
            1002,
            &rpc_call!(rpc::ARENA_CHALLENGE, [17u32, 16u32, 0u32, "ChuaCongA"], 3),
            NOW,
        );
        assert_eq!(ch.response.err, "ok");
        assert_eq!(ch.response.ret.get("position").and_then(DtoValue::as_u64), Some(16));
        assert!(ch.response.ret.get("battleRecord").and_then(DtoValue::as_bytes).is_some());
        assert_eq!(ch.peer_pushes.len(), 1);
        assert_eq!(ch.peer_pushes[0].0, 1001);
        assert_eq!(ch.peer_pushes[0].1.callback_name, push::ARENA_DATA_REFRESH);
    }

    // 29. Daily Sign-In (`sign.signToday` + `sign.gainVipSignReward`) and Online Timed Rewards (`online.db` tiers)
    #[test]
    fn test_29_daily_signin_vip_double_and_online_tiers() {
        let mut eng = make_engine_with_two_players();
        assert_eq!(eng.dispatch(1002, &rpc_call!(rpc::SIGN_TODAY, 1), NOW).response.err, "ok");
        assert_eq!(
            eng.dispatch(1002, &rpc_call!(rpc::SIGN_TODAY, 2), NOW).response.err,
            "already_signed_today"
        );
        assert_eq!(
            eng.dispatch(1002, &rpc_call!(rpc::SIGN_GAIN_VIP, [2u32], 3), NOW).response.err,
            "ok"
        );

        let copper_before = eng.players[&1002].copper;
        assert_eq!(
            eng.dispatch(1002, &rpc_call!(rpc::OPEN_ONLINE_RECEIVE, [2u32], 4), NOW)
                .response
                .err,
            "ok"
        );
        assert_eq!(eng.players[&1002].copper, copper_before + 150_000);
    }

    // 30. Non-GM player cannot execute slash commands (`/gold` falls back to normal chat rules)
    #[test]
    fn test_30_non_gm_player_cannot_run_gm_commands() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1002).unwrap().gm_enabled = false;
        let gold_before = eng.players[&1002].gold;
        let out = eng.dispatch(1002, &rpc_call!(rpc::CHAT_SEND_WORLD, [1, "/gold 999999"], 1), NOW);
        assert_eq!(eng.players[&1002].gold, gold_before);
        assert_eq!(
            out.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::Succ as i64)
        );
    }

    // 31. Lossless `DtoValue <-> gserver_protocol::Value` delegation & compressed LCPacket + BabelTime MD5
    #[test]
    fn test_31_protocol_delegation_zlib_md5_and_netconst_aliases() {
        let dto = dto_ok! {
            "empty_map" => dto_map!(),
            "empty_arr" => dto_arr![],
            "ascii_bytes" => DtoValue::Bytes(b"ASCII_BLOB".to_vec()),
            "i29_min" => i64::from(gserver_protocol::amf3::AMF3_INT_MIN),
            "i29_max" => i64::from(gserver_protocol::amf3::AMF3_INT_MAX),
            "u64_ts" => 1_795_500_500i64,
            "float_int" => 42.0f64,
            "repeat" => dto_arr![7u32; 4],
        };
        let amf = encode_amf3(&dto);
        assert_eq!(decode_amf3(&amf).unwrap(), dto);

        // Compressed LCPacket roundtrip + BabelTime MD5 tamper check
        let zframe = LcPacketFrame::compressed(&amf, true);
        assert_eq!(zframe.flags, [0, 1, 1, 1]);
        let mut wire = zframe.to_wire_bytes();
        let parsed = LcPacketFrame::from_wire_bytes(&wire).unwrap();
        assert_eq!(parsed.decompress_payload().unwrap(), amf);

        wire[8] ^= 0xFF;
        assert_eq!(LcPacketFrame::from_wire_bytes(&wire), Err(CodecError::ChecksumMismatch));

        // C2S LCPacket roundtrip via gserver_protocol::frame
        let c2s = LcPacketFrame::encode_c2s(&amf, true).unwrap();
        assert_eq!(LcPacketFrame::decode_c2s(&c2s).unwrap(), amf);

        // NetConst alias parity
        assert_eq!(rpc::USER_GET_USER, gserver_rpc_catalog::net_const::CS_GET_USER);
        assert_eq!(rpc::COPY_ATTACK, gserver_rpc_catalog::net_const::CS_COPY_ATTACK);
        assert_eq!(push::USER_UPDATE, gserver_rpc_catalog::net_const::SC_USER_UPDATEUSER);
        assert_eq!(push::BAG_UPDATE, gserver_rpc_catalog::net_const::CS_BAG_PUSH_UPDATE);
    }

    // 32. Typed `RpcRequest` argument extractors
    #[test]
    fn test_32_rpc_request_typed_extractors() {
        let req = RpcRequest::new(
            rpc::SHOP_BUY,
            vec![
                DtoValue::from(2u32),
                DtoValue::from("gk_2_1"),
                DtoValue::from(-15i32),
                DtoValue::from(true),
                dto_arr![80001u64, 80002u64],
            ],
            99,
        );
        assert_eq!(req.arg_u32_or(0, 1), 2);
        assert_eq!(req.arg_str_or(1, ""), "gk_2_1");
        assert_eq!(req.arg_i32_or(2, 0), -15);
        assert!(req.arg_bool_or(3, false));
        assert_eq!(req.arg_u64_vec(4), vec![80001, 80002]);
        assert_eq!(req.arg_u32_or(9, 77), 77);
    }
}
