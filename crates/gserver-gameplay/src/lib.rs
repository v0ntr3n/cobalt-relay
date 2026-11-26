#![forbid(unsafe_code)]
//! `gserver-gameplay` — Zero-dependency pure-Rust gameplay & GM command engine for **GServer (`vnsg2`)**.
//!
//! Covers:
//! - `protocol`: AMF3 binary serializer/deserializer, 24-byte `LCPacket` wire framing, RPC/Push envelopes, and exact `NetConst.lua` method constants.
//! - `player`: `PlayerState`, `BagState` (`bag.bagInfo` & `bag.update`), `HeroRoster` & `HeroFormationData` (`re.hero.new`, `user.updateUser`, `MainTown` safety invariants).
//! - `town`: `MainTown` bootstrap validator + multiplayer walking rooms (`city.*`, `imperialcity.*`, `town.addPlayer`, `town.delPlayer`, `town.move`, `town.update`).
//! - `chat_gm`: Multi-channel chat (`chat.sendWorld`, `chat.sendPersonal`, `*LastMsgs` AMF3 blobs, `re.chat.getMsg`) + In-Game GM Slash-Command Processor (`/help`, `/gold`, `/silver`, `/vip`, `/level`, `/item`, `/hero`, `/mail`, `/heal`).
//! - `mail`: Notification `MailBox` (`mail.*`, `re.mail.newMail`) + `RewardCenter` (`reward.getRewardList`, `reward.receiveByRidArr`, `reward.giftByCode`, `re.reward.new`).
//! - `shop_pub`: Deterministic Tavern Gacha (`raffle.*`, `raffle.db` probabilities/pity/luck chests) + Multi-Currency `ShopEngine` (`shop.*`).
//! - `battle_copy`: Crash-proof Turn-Based Battle Simulator (`FightProxy` / `BaseUnit` / `PropertyKey`, `appraisal = 50` on win), PvE Campaign (`copy.*`), `ArenaEngine` (`arena.*`), and `SignAndOnlineService` (`sign.*`, `online.*`).
//! - `engine`: Stateful `GameplayEngine` RPC router returning responses + caller pushes + peer broadcasts.

pub mod battle_copy;
pub mod chat_gm;
pub mod engine;
pub mod mail;
pub mod player;
pub mod protocol;
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
        bundle_multi_envelope, decode_amf3, encode_amf3, push, rpc, DtoValue, LcPacketFrame,
        RpcRequest,
    };
    use crate::town::{validate_maintown_bootstrap, MainTownCrashRisk, TownMapConfig};

    const NOW: u64 = 1_795_500_500;

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
        let original = DtoValue::map([
            ("err", DtoValue::from("ok")),
            ("neg", DtoValue::from(-12345i64)),
            ("big", DtoValue::from(5_000_000_000i64)),
            ("pi", DtoValue::from(3.25f64)),
            ("flag", DtoValue::from(true)),
            ("nothing", DtoValue::Null),
            ("raw", DtoValue::Bytes(vec![1, 2, 3, 254])),
            (
                "arr",
                DtoValue::array([DtoValue::from(10), DtoValue::from("xin chao")]),
            ),
        ]);
        let bytes = encode_amf3(&original);
        let decoded = decode_amf3(&bytes).expect("AMF3 decode must succeed");
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
        let body = encode_amf3(&DtoValue::map([("hello", DtoValue::from(3u32))]));
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
        let req = RpcRequest::new(
            rpc::CHAT_SEND_WORLD,
            vec![DtoValue::from(1), DtoValue::from("/gold 777")],
            1,
        );
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
        let out = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::FORMATION_GET_POSITION_INFO, vec![], 2),
            NOW,
        );
        assert_eq!(out.response.err, "ok");
        let pos = out.response.ret.get("position").and_then(DtoValue::as_array).unwrap();
        assert_eq!(pos.len(), 6);
        assert_ne!(pos[0].as_u64().unwrap(), 0);
        let spirit = out.response.ret.get("spirit").and_then(DtoValue::as_array).unwrap();
        assert_eq!(spirit.len(), 6);
        let yanwu = out.response.ret.get("yanWu").and_then(DtoValue::as_array).unwrap();
        assert_eq!(yanwu.len(), 6);
    }

    // 7. Entering `city.enter` spawns player and broadcasts `town.addPlayer` to peers
    #[test]
    fn test_07_town_enter_and_peer_add_broadcast() {
        let mut eng = make_engine_with_two_players();
        let r1 = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(1u32)], 10),
            NOW,
        );
        assert_eq!(r1.response.err, "ok");
        assert!(r1.peer_pushes.is_empty());

        let r2 = eng.dispatch(
            1002,
            &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(1u32)], 11),
            NOW,
        );
        assert_eq!(r2.response.err, "ok");
        assert_eq!(r2.peer_pushes.len(), 1);
        assert_eq!(r2.peer_pushes[0].0, 1001);
        assert_eq!(r2.peer_pushes[0].1.callback_name, push::TOWN_ADD_PLAYER);
        let list = r2.response.ret.get("enter_list").and_then(DtoValue::as_array).unwrap();
        assert_eq!(list.len(), 2);
    }

    // 8. `imperialcity.move` clamps coordinates to `CCRect(50, 0, 2720, 342)` (`[50..2770] x [0..342]`)
    #[test]
    fn test_08_royalcity_move_clamps_bounds_and_sets_direction() {
        let mut eng = make_engine_with_two_players();
        eng.dispatch(1001, &RpcRequest::new(rpc::ROYALCITY_ENTER, vec![], 1), NOW);
        eng.dispatch(1002, &RpcRequest::new(rpc::ROYALCITY_ENTER, vec![], 2), NOW);

        // Move way past right/top bounds -> clamped to (2770, 342), dir = 1
        let mv1 = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::ROYALCITY_MOVE,
                vec![DtoValue::from(9999), DtoValue::from(999)],
                3,
            ),
            NOW,
        );
        assert_eq!(mv1.response.ret.get("x").and_then(DtoValue::as_i64), Some(2770));
        assert_eq!(mv1.response.ret.get("y").and_then(DtoValue::as_i64), Some(342));
        assert_eq!(mv1.response.ret.get("dir").and_then(DtoValue::as_i64), Some(1));
        assert_eq!(mv1.peer_pushes[0].1.callback_name, push::TOWN_MOVE);

        // Move past left/bottom bounds -> clamped to (50, 0), dir = -1
        let mv2 = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::ROYALCITY_MOVE,
                vec![DtoValue::from(-20), DtoValue::from(-5)],
                4,
            ),
            NOW,
        );
        assert_eq!(mv2.response.ret.get("x").and_then(DtoValue::as_i64), Some(50));
        assert_eq!(mv2.response.ret.get("y").and_then(DtoValue::as_i64), Some(0));
        assert_eq!(mv2.response.ret.get("dir").and_then(DtoValue::as_i64), Some(-1));
    }

    // 9. `city.leave` removes occupant and broadcasts `town.delPlayer`
    #[test]
    fn test_09_town_leave_broadcasts_del_player() {
        let mut eng = make_engine_with_two_players();
        eng.dispatch(1001, &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(1)], 1), NOW);
        eng.dispatch(1002, &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(1)], 2), NOW);

        let lv = eng.dispatch(1001, &RpcRequest::new(rpc::CITY_LEAVE, vec![], 3), NOW);
        assert_eq!(lv.peer_pushes.len(), 1);
        assert_eq!(lv.peer_pushes[0].0, 1002);
        assert_eq!(lv.peer_pushes[0].1.callback_name, push::TOWN_DEL_PLAYER);

        let enter_list = eng.dispatch(
            1002,
            &RpcRequest::new(rpc::CITY_GET_ENTER_LIST, vec![], 4),
            NOW,
        );
        assert_eq!(enter_list.response.ret.as_array().unwrap().len(), 1);
    }

    // 10. Town sharding automatically overflows player 11 into instance #2 for Đào Nguyên Thôn (`cap = 10`)
    #[test]
    fn test_10_town_room_capacity_sharding() {
        let mut eng = GameplayEngine::new(7);
        assert_eq!(TownMapConfig::preset(1).max_visible_players, 10);
        for i in 1..=11u64 {
            eng.register_player(PlayerState::new(i, format!("P{i}"), 106));
            let out = eng.dispatch(
                i,
                &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(1)], i as u32),
                NOW,
            );
            let inst = out.response.ret.get("instance_id").and_then(DtoValue::as_u64).unwrap();
            if i <= 10 {
                assert_eq!(inst, 1);
            } else {
                assert_eq!(inst, 2);
            }
        }
    }

    // 11. Town level requirement gate (`Lạc Dương` `town_id = 2` requires `level >= 5`)
    #[test]
    fn test_11_town_min_level_requirement() {
        let mut eng = make_engine_with_two_players();
        // Player 1001 is Lv.1 -> rejected from town 2
        let r1 = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(2)], 1),
            NOW,
        );
        assert_eq!(r1.response.err, "level_not_enough");

        // Player 1002 is Lv.50 -> allowed
        let r2 = eng.dispatch(
            1002,
            &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(2)], 2),
            NOW,
        );
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
        assert_eq!(
            parse_gm_command("/silver 100000"),
            Ok(GmCommand::Silver(100000))
        );
        assert_eq!(parse_gm_command("/vip 12"), Ok(GmCommand::Vip(12)));
        assert_eq!(parse_gm_command("/level 85"), Ok(GmCommand::Level(85)));
        assert_eq!(
            parse_gm_command("/item 16012 20"),
            Ok(GmCommand::Item {
                tid: 16012,
                qty: 20
            })
        );
        assert_eq!(
            parse_gm_command("/hero 20008"),
            Ok(GmCommand::Hero { htid: 20008 })
        );
        assert_eq!(
            parse_gm_command("/mail 17002 5"),
            Ok(GmCommand::Mail { tid: 17002, qty: 5 })
        );
        assert_eq!(parse_gm_command("/heal"), Ok(GmCommand::Heal));
        assert_eq!(
            parse_gm_command("hello world"),
            Err(GmParseError::NotAGmCommand)
        );
        assert!(matches!(
            parse_gm_command("/unknown"),
            Err(GmParseError::UnknownCommand(_))
        ));
    }

    // 14. GM `/gold` and `/silver` live `user.updateUser` + `re.user.copperOverMax` pushes
    #[test]
    fn test_14_gm_gold_and_silver_pushes_and_cap() {
        let mut eng = make_engine_with_two_players();
        let out_gold = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/gold 25000")],
                1,
            ),
            NOW,
        );
        assert_eq!(eng.players[&1001].gold, 35_000);
        assert!(out_gold
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::USER_UPDATE));
        assert!(out_gold
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::CHAT_GET_MSG));

        let out_silver = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/silver 2000000000")],
                2,
            ),
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
        eng.dispatch(1001, &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(1)], 1), NOW);
        eng.dispatch(1002, &RpcRequest::new(rpc::CITY_ENTER, vec![DtoValue::from(1)], 2), NOW);

        let out_vip = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/vip 10")],
                3,
            ),
            NOW,
        );
        assert_eq!(eng.players[&1001].vip, 10);
        assert_eq!(out_vip.peer_pushes.len(), 1);
        assert_eq!(out_vip.peer_pushes[0].1.callback_name, push::TOWN_UPDATE);

        let out_lv = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/level 60")],
                4,
            ),
            NOW,
        );
        assert_eq!(eng.players[&1001].level, 60);
        // At Lv.60 all 6 formation slots are unlocked!
        let pos_map = eng.players[&1001].hero_position_map();
        assert_eq!(pos_map.get("6").and_then(DtoValue::as_bool), Some(true));
        assert_eq!(out_lv.peer_pushes[0].1.callback_name, push::TOWN_UPDATE);
    }

    // 16. GM `/item`, `/hero`, `/mail`, and `/heal` emit exact client pushes
    #[test]
    fn test_16_gm_item_hero_mail_and_heal_pushes() {
        let mut eng = make_engine_with_two_players();
        // Drain execution & hero HP first
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

        let out_item = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/item 16012 15")],
                1,
            ),
            NOW,
        );
        assert!(out_item
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::BAG_UPDATE));
        assert_eq!(eng.players[&1001].bag.count_by_tid(16012), 15);

        let out_hero = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/hero 20008")],
                2,
            ),
            NOW,
        );
        assert!(out_hero
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::HERO_NEW));
        assert_eq!(eng.players[&1001].roster.heroes.len(), 2);

        let out_mail = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/mail 16011 8")],
                3,
            ),
            NOW,
        );
        assert!(out_mail
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::MAIL_NEW));
        assert!(out_mail
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::REWARD_NEW));

        let out_heal = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/heal")],
                4,
            ),
            NOW,
        );
        assert!(out_heal
            .client_pushes
            .iter()
            .any(|p| p.callback_name == push::USER_UPDATE));
        assert_eq!(eng.players[&1001].execution, 120);
        let healed_hp = eng.players[&1001].roster.heroes[&main_hid].cur_hp;
        let max_hp = eng.players[&1001].roster.heroes[&main_hid].max_hp;
        assert_eq!(healed_hp, max_hp);
    }

    // 17. World chat gate (`level < 40 && vip < 2` returns `10004` `NotOpen`) vs Lv.50 broadcast + AMF3 history
    #[test]
    fn test_17_world_chat_gate_broadcast_and_amf3_last_msgs() {
        let mut eng = make_engine_with_two_players();
        // Player 1001 is Lv.1 VIP 0 -> blocked with 10004
        let blocked = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("Hello world!")],
                1,
            ),
            NOW,
        );
        assert_eq!(
            blocked.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::NotOpen as i64)
        );

        // Player 1002 is Lv.50 VIP 5 -> succeeds and broadcasts `re.chat.getMsg` to 1001
        let sent1 = eng.dispatch(
            1002,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("First message")],
                2,
            ),
            NOW,
        );
        assert_eq!(
            sent1.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::Succ as i64)
        );
        assert_eq!(sent1.peer_pushes.len(), 1);
        assert_eq!(sent1.peer_pushes[0].0, 1001);
        assert_eq!(sent1.peer_pushes[0].1.callback_name, push::CHAT_GET_MSG);

        eng.dispatch(
            1002,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("Second message")],
                3,
            ),
            NOW + 1,
        );

        // `chat.getWorldLastMsgs` returns AMF3-encoded blobs in reverse chronological order!
        let hist = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::CHAT_GET_WORLD_LAST, vec![], 4),
            NOW + 2,
        );
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
            &RpcRequest::new(
                rpc::CHAT_SEND_PERSONAL,
                vec![
                    DtoValue::from("ChuaCongA"),
                    DtoValue::from(1),
                    DtoValue::from("Secret tip"),
                    DtoValue::from(false),
                ],
                1,
            ),
            NOW,
        );
        assert_eq!(
            ok_whisper.response.ret.get("error_code").and_then(DtoValue::as_i64),
            Some(ChatErrorCode::Succ as i64)
        );
        assert_eq!(ok_whisper.peer_pushes[0].0, 1001);

        let offline = eng.dispatch(
            1002,
            &RpcRequest::new(
                rpc::CHAT_SEND_PERSONAL,
                vec![
                    DtoValue::from("GhostUser"),
                    DtoValue::from(1),
                    DtoValue::from("Hi"),
                ],
                2,
            ),
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

        let list_out = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::REWARD_GET_LIST,
                vec![DtoValue::from(0), DtoValue::from(100)],
                1,
            ),
            NOW,
        );
        let rewards = list_out.response.ret.as_array().unwrap();
        assert_eq!(rewards.len(), 1);
        // Verify `va_reward[1]` nested array schema expected by `RewardPanel.lua` line 153!
        let va_reward = rewards[0].get("va_reward").and_then(DtoValue::as_array).unwrap();
        assert_eq!(va_reward.len(), 1);
        assert_eq!(va_reward[0].as_array().unwrap().len(), 2);

        let gold_before = eng.players[&1001].gold;
        let claim1 = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::REWARD_RECEIVE_BY_RID_ARR,
                vec![DtoValue::array([DtoValue::from(rid)])],
                2,
            ),
            NOW,
        );
        assert_eq!(claim1.response.err, "ok");
        assert_eq!(eng.players[&1001].gold, gold_before + 300);
        assert_eq!(eng.players[&1001].bag.count_by_tid(16012), 2);

        // Duplicate claim of same `rid` is rejected cleanly!
        let claim2 = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::REWARD_RECEIVE_BY_RID_ARR,
                vec![DtoValue::array([DtoValue::from(rid)])],
                3,
            ),
            NOW,
        );
        assert_eq!(claim2.response.err, "reward_not_found_or_claimed");

        let read_all = eng.dispatch(1001, &RpcRequest::new(rpc::MAIL_READ_ALL, vec![], 4), NOW);
        assert_eq!(
            read_all.response.ret.get("read_count").and_then(DtoValue::as_i64),
            Some(1)
        );
    }

    // 20. Gift code redemption (`reward.giftByCode`) grants rewards once and rejects duplicate reuse
    #[test]
    fn test_20_gift_code_redemption_once_per_player() {
        let mut eng = make_engine_with_two_players();
        let r1 = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::REWARD_GIFT_BY_CODE,
                vec![DtoValue::from("gservervip888")],
                1,
            ),
            NOW,
        );
        assert_eq!(r1.response.err, "ok");
        assert_eq!(eng.players[&1001].gold, 10_888);

        let r2 = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::REWARD_GIFT_BY_CODE,
                vec![DtoValue::from("GSERVERVIP888")],
                2,
            ),
            NOW,
        );
        assert_eq!(r2.response.err, "already_redeemed");
    }

    // 21. Tavern Divine 10-pull (`raffle.multiRaffle`) applies first-10 discount (900 gold), guarantees Orange hero, and accrues luck
    #[test]
    fn test_21_tavern_divine_multi_raffle_first_ten_and_orange_pity() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1001).unwrap().vip = 5;
        let out = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::RAFFLE_MULTI, vec![DtoValue::from(2u32)], 1),
            NOW,
        );
        assert_eq!(out.response.err, "ok");
        // First 10-pull at VIP >= 5 costs 900 gold (10_000 -> 9_100)
        assert_eq!(eng.players[&1001].gold, 9_100);
        let rewards = out.response.ret.get("reward").and_then(DtoValue::as_array).unwrap();
        assert_eq!(rewards.len(), 10);
        // At least the 10th pull must be quality 5 (Orange) in the player's roster!
        assert!(eng.players[&1001]
            .roster
            .heroes
            .values()
            .any(|h| h.quality == 5));
        assert!(out.response.ret.get("senior_luck_num").and_then(DtoValue::as_u64).unwrap() >= 100);

        // Second 10-pull costs 2850 gold (9_100 -> 6_250)
        let out2 = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::RAFFLE_MULTI, vec![DtoValue::from(2u32)], 2),
            NOW,
        );
        assert_eq!(out2.response.err, "ok");
        assert_eq!(eng.players[&1001].gold, 6_250);
    }

    // 22. Tavern Warrior free draw cooldown (`300s`) and token fallback
    #[test]
    fn test_22_tavern_warrior_free_cooldown_and_token() {
        let mut eng = make_engine_with_two_players();
        let free1 = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::RAFFLE_SINGLE, vec![DtoValue::from(1u32)], 1),
            NOW,
        );
        assert_eq!(free1.response.err, "ok");

        // Immediate second draw without token fails while on 300s cooldown
        let blocked = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::RAFFLE_SINGLE, vec![DtoValue::from(1u32)], 2),
            NOW + 10,
        );
        assert_eq!(blocked.response.err, "not_enough_warrior_tokens");

        // After 300s cooldown, free draw succeeds again!
        let free2 = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::RAFFLE_SINGLE, vec![DtoValue::from(1u32)], 3),
            NOW + 301,
        );
        assert_eq!(free2.response.err, "ok");
    }

    // 23. Tavern Hot VIP & Red Hero draws + Luck milestone chest (`raffle.receiveHotLuckHeroGift`)
    #[test]
    fn test_23_tavern_hot_and_red_draws_and_luck_chest() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1001).unwrap().gold = 200_000;
        for i in 0..3u32 {
            let r = eng.dispatch(
                1001,
                &RpcRequest::new(rpc::RAFFLE_MULTI_HOT, vec![], 10 + i),
                NOW,
            );
            assert_eq!(r.response.err, "ok");
        }
        // 30 pulls * 10 luck = 300 luck >= 250 box 1 threshold!
        let claim = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::RAFFLE_RECEIVE_LUCK_HOT, vec![DtoValue::from(1u32)], 20),
            NOW,
        );
        assert_eq!(claim.response.err, "ok");

        let red = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::RAFFLE_RED, vec![DtoValue::from(10u32)], 21),
            NOW,
        );
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
        let info = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::SHOP_GET_INFO, vec![DtoValue::from(1u32)], 1),
            NOW,
        );
        let gk = info
            .response
            .ret
            .get("goods_key")
            .and_then(DtoValue::as_str)
            .unwrap()
            .to_owned();

        let buy_ok = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::SHOP_BUY,
                vec![
                    DtoValue::from(1u32),
                    DtoValue::from(gk.as_str()),
                    DtoValue::from(2u32),
                    DtoValue::from(102u32),
                    DtoValue::from(2u32),
                ],
                2,
            ),
            NOW,
        );
        assert_eq!(buy_ok.response.err, "ok");
        assert_eq!(eng.players[&1001].bag.count_by_tid(16012), 2);

        // Refresh rotates `goods_key`; buying with old `gk` returns `stale_goods_key`
        let refreshed = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::SHOP_REFRESH, vec![DtoValue::from(1u32)], 3),
            NOW,
        );
        assert_eq!(refreshed.response.err, "ok");
        let stale_buy = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::SHOP_BUY,
                vec![
                    DtoValue::from(1u32),
                    DtoValue::from(gk.as_str()),
                    DtoValue::from(2u32),
                    DtoValue::from(102u32),
                    DtoValue::from(1u32),
                ],
                4,
            ),
            NOW,
        );
        assert_eq!(stale_buy.response.err, "stale_goods_key");
    }

    // 25. Battle simulator produces crash-proof `brid`, `userInfo[1..2]`, `arrTeam1`/`arrTeam2`, and `appraisal == 50`
    #[test]
    fn test_25_battle_simulator_crash_proof_dto_and_appraisal_50() {
        let p = PlayerState::new(1001, "Hero", 106);
        let t1 = vec![CombatUnit::from_hero(
            p.roster.heroes.values().next().unwrap(),
            0,
        )];
        let t2 = vec![CombatUnit::monster(9001, 5001, 0, 1)];
        let u1 = DtoValue::map([("uid", DtoValue::from(1001u64)), ("fight_force", DtoValue::from(5000u64)), ("dress_id", DtoValue::from(0u32))]);
        let u2 = DtoValue::map([("uid", DtoValue::from(0u64)), ("fight_force", DtoValue::from(1000u64)), ("dress_id", DtoValue::from(0u32))]);
        let sim = simulate_battle(888, false, &t1, &t2, u1, u2, &[]);
        assert!(sim.is_win);
        assert_eq!(sim.appraisal, 50, "FightProxy:isSuccessFight requires appraisal == 50");
        assert_eq!(sim.server_attack_dto.get("brid").and_then(DtoValue::as_u64), Some(888));
        assert_eq!(
            sim.server_attack_dto
                .get("userInfo")
                .and_then(DtoValue::as_array)
                .unwrap()
                .len(),
            2
        );
        let report_decoded = decode_amf3(&sim.to_amf3_report_bytes()).unwrap();
        assert_eq!(report_decoded.get("appraisal").and_then(DtoValue::as_u64), Some(50));
    }

    // 26. PvE Campaign Copy full flow: `copy.attack` -> `copy.pass` -> `copy.sweep` -> `copy.fetchBoxReward` -> `copy.fetchPrize`
    #[test]
    fn test_26_pve_copy_attack_pass_sweep_and_prizes() {
        let mut eng = make_engine_with_two_players();
        // Level up player 1001 so they crush chapter 1001 with 3 stars
        eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/level 20")],
                1,
            ),
            NOW,
        );

        let atk = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_ATTACK,
                vec![DtoValue::from(1001u32), DtoValue::from(100101u32)],
                2,
            ),
            NOW,
        );
        assert_eq!(atk.response.err, "ok");
        let brid = atk.response.ret.get("brid").and_then(DtoValue::as_u64).unwrap();
        assert!(brid > 0);

        let pass = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_PASS,
                vec![
                    DtoValue::from(1001u32),
                    DtoValue::from(100101u32),
                    DtoValue::Map(Default::default()),
                ],
                3,
            ),
            NOW,
        );
        assert_eq!(pass.response.err, "ok");
        assert_eq!(pass.response.ret.get("star").and_then(DtoValue::as_u64), Some(3));

        // Sweep x2 on 3-star stage
        let sweep = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_SWEEP,
                vec![
                    DtoValue::from(1001u32),
                    DtoValue::from(100101u32),
                    DtoValue::from(2u32),
                    DtoValue::from(0u32),
                ],
                4,
            ),
            NOW,
        );
        assert_eq!(sweep.response.err, "ok");

        // Claim stage box (`copy.fetchBoxReward` -> `"ok"`) and chapter star chest 0 (`3 stars`)
        let sbox = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_FETCH_BOX_REWARD,
                vec![DtoValue::from(1001u32), DtoValue::from(100101u32)],
                5,
            ),
            NOW,
        );
        assert_eq!(sbox.response.ret.as_str(), Some("ok"));

        let cprize = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_FETCH_PRIZE,
                vec![DtoValue::from(1001u32), DtoValue::from(0u32)],
                6,
            ),
            NOW,
        );
        assert_eq!(cprize.response.err, "ok");

        // Verify `battle.getRecord` retrieves the exact AMF3 replay blob
        let rec = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::BATTLE_GET_RECORD, vec![DtoValue::from(brid)], 7),
            NOW,
        );
        assert!(rec.response.ret.as_bytes().is_some());
    }

    // 27. Tutorial `user.beforLoginBattle` works at Lv.1 without consuming execution
    #[test]
    fn test_27_tutorial_before_login_battle() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1001).unwrap().execution = 0;
        let tut = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::USER_BEFORE_LOGIN_BATTLE, vec![], 1),
            NOW,
        );
        assert_eq!(tut.response.err, "ok");
        assert!(tut.response.ret.get("brid").and_then(DtoValue::as_u64).is_some());
    }

    // 28. Arena `enterArena` + `arena.challenge` swaps rank, emits AMF3 `battleRecord`, and pushes `re.arena.dataRefresh`
    #[test]
    fn test_28_arena_challenge_rank_swap_and_defender_refresh_push() {
        let mut eng = make_engine_with_two_players();
        // Register 1001 (Lv.1) first -> pos 16, then 1002 (Lv.50) -> pos 17
        let e1 = eng.dispatch(1001, &RpcRequest::new(rpc::ARENA_ENTER, vec![], 1), NOW);
        let pos1 = e1
            .response
            .ret
            .get("res")
            .and_then(|r| r.get("position"))
            .and_then(DtoValue::as_u64)
            .unwrap();
        assert_eq!(pos1, 16);

        let e2 = eng.dispatch(1002, &RpcRequest::new(rpc::ARENA_ENTER, vec![], 2), NOW);
        let pos2 = e2
            .response
            .ret
            .get("res")
            .and_then(|r| r.get("position"))
            .and_then(DtoValue::as_u64)
            .unwrap();
        assert_eq!(pos2, 17);

        // Player 1002 challenges Player 1001 at pos 16 -> wins, swaps to pos 16, pushes `re.arena.dataRefresh` to 1001!
        let ch = eng.dispatch(
            1002,
            &RpcRequest::new(
                rpc::ARENA_CHALLENGE,
                vec![
                    DtoValue::from(17u32),
                    DtoValue::from(16u32),
                    DtoValue::from(0u32),
                    DtoValue::from("ChuaCongA"),
                ],
                3,
            ),
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
        // Player 1002 has VIP 5
        let s1 = eng.dispatch(1002, &RpcRequest::new(rpc::SIGN_TODAY, vec![], 1), NOW);
        assert_eq!(s1.response.err, "ok");

        // Signing again on the same day is rejected
        let s_dup = eng.dispatch(1002, &RpcRequest::new(rpc::SIGN_TODAY, vec![], 2), NOW);
        assert_eq!(s_dup.response.err, "already_signed_today");

        // Claiming VIP bonus on the same day succeeds once
        let vip_bonus = eng.dispatch(
            1002,
            &RpcRequest::new(rpc::SIGN_GAIN_VIP, vec![DtoValue::from(2u32)], 3),
            NOW,
        );
        assert_eq!(vip_bonus.response.err, "ok");

        // Claiming `online.db` tier 2 (600s -> 150,000 copper) succeeds since `online_accum_secs = 1800`
        let copper_before = eng.players[&1002].copper;
        let online_claim = eng.dispatch(
            1002,
            &RpcRequest::new(rpc::OPEN_ONLINE_RECEIVE, vec![DtoValue::from(2u32)], 4),
            NOW,
        );
        assert_eq!(online_claim.response.err, "ok");
        assert_eq!(eng.players[&1002].copper, copper_before + 150_000);
    }

    // 30. Non-GM player cannot execute slash commands (`/gold` falls back to normal chat rules)
    #[test]
    fn test_30_non_gm_player_cannot_run_gm_commands() {
        let mut eng = make_engine_with_two_players();
        eng.players.get_mut(&1002).unwrap().gm_enabled = false;
        let gold_before = eng.players[&1002].gold;
        let out = eng.dispatch(
            1002,
            &RpcRequest::new(
                rpc::CHAT_SEND_WORLD,
                vec![DtoValue::from(1), DtoValue::from("/gold 999999")],
                1,
            ),
            NOW,
        );
        // Treated as ordinary chat message, gold untouched!
        assert_eq!(eng.players[&1002].gold, gold_before);
        assert_eq!(
            out.response
                .ret
                .get("error_code")
                .and_then(DtoValue::as_i64),
            Some(ChatErrorCode::Succ as i64)
        );
    }

    // 31. Copy & Arena battle execution emit authentic per-hero `arrSkill`, step skill IDs, and `rageQueue`
    #[test]
    fn test_31_copy_and_arena_battles_emit_authentic_per_hero_skills() {
        let mut eng = GameplayEngine::new(77);
        let mut p1 = PlayerState::new(1001, "ChuaCongNam", 1); // Nam Chủ Tướng (102 / 101)
        p1.level = 35;
        p1.roster.heroes.get_mut(&10_000_001).unwrap().level = 35;
        p1.roster
            .heroes
            .get_mut(&10_000_001)
            .unwrap()
            .recalculate_stats();
        let (zy, _) = p1.roster.recruit_hero(201, 35, NOW); // Triệu Vân (20102 / 20101)
        let (lb, _) = p1.roster.recruit_hero(401, 35, NOW); // Lữ Bố (40102 / 40101)
        let (dt, _) = p1.roster.recruit_hero(406, 35, NOW); // Điêu Thuyền (40602 / 40601)
        assert_eq!((zy.normal_skill_id, zy.rage_skill_id), (20102, 20101));
        assert_eq!((lb.normal_skill_id, lb.rage_skill_id), (40102, 40101));
        assert_eq!((dt.normal_skill_id, dt.rage_skill_id), (40602, 40601));

        let mut p2 = PlayerState::new(1002, "ChuaCongNu", 2); // Nữ Chủ Tướng (104 / 103)
        p2.level = 10;
        p2.roster.heroes.get_mut(&10_000_001).unwrap().level = 10;
        p2.roster
            .heroes
            .get_mut(&10_000_001)
            .unwrap()
            .recalculate_stats();

        eng.register_player(p1);
        eng.register_player(p2);

        // --- A. Copy Battle (`copy.attack` + `battle.getRecord`) ---
        let copy_atk = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_ATTACK,
                vec![DtoValue::from(1001u32), DtoValue::from(100101u32)],
                1,
            ),
            NOW,
        );
        assert_eq!(copy_atk.response.err, "ok");
        let team1_heroes = copy_atk
            .response
            .ret
            .get("battleInfo")
            .and_then(|b| b.get("arrTeam1"))
            .and_then(|t| t.get("1"))
            .and_then(|t| t.get("arrHero"))
            .and_then(DtoValue::as_map)
            .expect("arrTeam1.1.arrHero");
        assert_eq!(team1_heroes.len(), 4);

        let expected_team1_skills: &[(u32, u32, u32)] = &[
            (1, 102, 101),
            (201, 20102, 20101),
            (401, 40102, 40101),
            (406, 40602, 40601),
        ];
        for (idx, &(exp_htid, exp_com, exp_angry)) in expected_team1_skills.iter().enumerate() {
            let unit_dto = team1_heroes.get(&(idx + 1).to_string()).expect("unit slot");
            assert_eq!(
                unit_dto.get("htid").and_then(DtoValue::as_u32),
                Some(exp_htid)
            );
            let arr_skill = unit_dto
                .get("arrSkill")
                .and_then(DtoValue::as_array)
                .expect("arrSkill");
            let s0 = arr_skill[0].as_array().unwrap()[0].as_u32().unwrap();
            let s1 = arr_skill[1].as_array().unwrap()[0].as_u32().unwrap();
            assert_eq!((s0, s1), (exp_com, exp_angry));
            assert_ne!(s0, 1001);
            assert_ne!(s1, 2001);
        }

        // Copy stronghold monsters (`5002`, `5003`) retain monster default `(1001, 2001)`
        let team2_monsters = copy_atk
            .response
            .ret
            .get("battleInfo")
            .and_then(|b| b.get("arrTeam2"))
            .and_then(|t| t.get("1"))
            .and_then(|t| t.get("arrHero"))
            .and_then(DtoValue::as_map)
            .expect("arrTeam2.1.arrHero");
        for m_dto in team2_monsters.values() {
            let arr_skill = m_dto.get("arrSkill").and_then(DtoValue::as_array).unwrap();
            assert_eq!(arr_skill[0].as_array().unwrap()[0].as_u32(), Some(1001));
            assert_eq!(arr_skill[1].as_array().unwrap()[0].as_u32(), Some(2001));
        }

        // Verify Copy AMF3 replay report preserves `appraisal == 50`
        let brid = copy_atk
            .response
            .ret
            .get("brid")
            .and_then(DtoValue::as_u64)
            .unwrap();
        let rec_out = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::BATTLE_GET_RECORD, vec![DtoValue::from(brid)], 2),
            NOW,
        );
        let copy_report = decode_amf3(rec_out.response.ret.as_bytes().unwrap()).unwrap();
        assert_eq!(
            copy_report.get("appraisal").and_then(DtoValue::as_u32),
            Some(50)
        );

        // --- B. Arena Battle (`arena.challenge` vs Player 1002 & vs NPC Guard pos 15 Tiểu Kiều) ---
        let _ = eng.dispatch(1002, &RpcRequest::new(rpc::ARENA_ENTER, vec![], 3), NOW); // pos 16 (htid=2)
        let _ = eng.dispatch(1001, &RpcRequest::new(rpc::ARENA_ENTER, vec![], 4), NOW); // pos 17 (htid=1)

        // Challenge Player 1002 at pos 16 -> defender has Nữ Chủ Tướng (104 / 103), NOT 1001 / 2001!
        let arena_pvp = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::ARENA_CHALLENGE,
                vec![
                    DtoValue::from(17u32),
                    DtoValue::from(16u32),
                    DtoValue::from(0u32),
                    DtoValue::from("ChuaCongNu"),
                ],
                5,
            ),
            NOW,
        );
        assert_eq!(arena_pvp.response.err, "ok");
        let pvp_report = decode_amf3(
            arena_pvp
                .response
                .ret
                .get("battleRecord")
                .and_then(DtoValue::as_bytes)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            pvp_report.get("appraisal").and_then(DtoValue::as_u32),
            Some(50)
        );
        let pvp_def = pvp_report
            .get("serverData")
            .and_then(|s| s.get("battleInfo"))
            .and_then(|b| b.get("arrTeam2"))
            .and_then(|t| t.get("1"))
            .and_then(|t| t.get("arrHero"))
            .and_then(|h| h.get("1"))
            .expect("arena pvp defender unit");
        assert_eq!(pvp_def.get("htid").and_then(DtoValue::as_u32), Some(2));
        let pvp_def_skills = pvp_def
            .get("arrSkill")
            .and_then(DtoValue::as_array)
            .unwrap();
        assert_eq!(pvp_def_skills[0].as_array().unwrap()[0].as_u32(), Some(104));
        assert_eq!(pvp_def_skills[1].as_array().unwrap()[0].as_u32(), Some(103));

        // Challenge NPC Guard at pos 15 (`htid = 310` Tiểu Kiều -> 31002 / 31001) + multi-round rage check
        let mut p_tank = PlayerState::new(1003, "RageTester", 201); // Triệu Vân (20102 / 20101)
        p_tank.level = 15;
        p_tank.roster.heroes.get_mut(&10_000_001).unwrap().level = 15;
        p_tank
            .roster
            .heroes
            .get_mut(&10_000_001)
            .unwrap()
            .recalculate_stats();
        p_tank.roster.heroes.get_mut(&10_000_001).unwrap().atk = 400; // Ensures >= 2 rounds so rage triggers!
        eng.register_player(p_tank);
        let _ = eng.dispatch(1003, &RpcRequest::new(rpc::ARENA_ENTER, vec![], 6), NOW); // pos 18
        let arena_npc = eng.dispatch(
            1003,
            &RpcRequest::new(
                rpc::ARENA_CHALLENGE,
                vec![
                    DtoValue::from(18u32),
                    DtoValue::from(15u32),
                    DtoValue::from(15u32),
                    DtoValue::from("ArenaGuard_15"),
                ],
                7,
            ),
            NOW,
        );
        assert_eq!(arena_npc.response.err, "ok");
        let npc_report = decode_amf3(
            arena_npc
                .response
                .ret
                .get("battleRecord")
                .and_then(DtoValue::as_bytes)
                .unwrap(),
        )
        .unwrap();
        let npc_def = npc_report
            .get("serverData")
            .and_then(|s| s.get("battleInfo"))
            .and_then(|b| b.get("arrTeam2"))
            .and_then(|t| t.get("1"))
            .and_then(|t| t.get("arrHero"))
            .and_then(|h| h.get("1"))
            .expect("arena npc guard unit");
        assert_eq!(npc_def.get("htid").and_then(DtoValue::as_u32), Some(310));
        let npc_def_skills = npc_def
            .get("arrSkill")
            .and_then(DtoValue::as_array)
            .unwrap();
        assert_eq!(
            npc_def_skills[0].as_array().unwrap()[0].as_u32(),
            Some(31002)
        );
        assert_eq!(
            npc_def_skills[1].as_array().unwrap()[0].as_u32(),
            Some(31001)
        );

        // Round 2 rageQueue must record Triệu Vân's authentic rage skill `20101` (not 2001)
        let rage_q = npc_report
            .get("rageQueue")
            .and_then(DtoValue::as_array)
            .expect("rageQueue");
        assert!(
            rage_q
                .iter()
                .any(|entry| entry.get("skillId").and_then(DtoValue::as_u32) == Some(20101)),
            "rageQueue must contain Triệu Vân's authentic angry_atk 20101: {rage_q:?}"
        );
    }

    // 32. `copy.attack` skill IDs, `CopyProxy:groupDrops` drop schema, `pass_strongh` count & progress export/import
    #[test]
    fn test_32_copy_attack_skills_group_drops_and_progress_export_import() {
        let mut eng = make_engine_with_two_players();

        // Invalid stage rejected
        let bad = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_ATTACK,
                vec![DtoValue::from(0), DtoValue::from(0)],
                1,
            ),
            NOW,
        );
        assert_eq!(bad.response.err, "invalid_copy_stage");

        let atk = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_ATTACK,
                vec![DtoValue::from(1001u32), DtoValue::from(100_101u32)],
                2,
            ),
            NOW,
        );
        assert_eq!(atk.response.err, "ok");
        assert_eq!(
            atk.response.ret.get("appraisal").and_then(DtoValue::as_i64),
            Some(50)
        );
        assert!(atk
            .response
            .ret
            .get("fightRet")
            .and_then(DtoValue::as_bytes)
            .is_some());
        let drop = atk.response.ret.get("drop").unwrap();
        assert_eq!(drop.get("copper").and_then(DtoValue::as_i64), Some(1750));
        assert_eq!(
            drop.get("item")
                .and_then(|m| m.get("17002"))
                .and_then(DtoValue::as_i64),
            Some(1)
        );
        let unit = atk
            .response
            .ret
            .get("battleInfo")
            .and_then(|b| b.get("arrTeam1"))
            .and_then(|t| t.get("1"))
            .and_then(|t| t.get("arrHero"))
            .and_then(|h| h.get("1"))
            .unwrap();
        let unit_htid = unit.get("htid").and_then(DtoValue::as_i64).unwrap() as u32;
        let resolved = gserver_data::resolve_hero_battle_skills(unit_htid);
        assert!(!resolved.used_normal_fallback && !resolved.used_rage_fallback);
        let skills = unit.get("arrSkill").and_then(DtoValue::as_array).unwrap();
        assert_eq!(
            skills[0].as_array().unwrap()[0].as_i64(),
            Some(resolved.normal_skill_id as i64)
        );
        assert_eq!(
            skills[1].as_array().unwrap()[0].as_i64(),
            Some(resolved.rage_skill_id as i64)
        );

        // Before `copy.pass`, `pass_strongh` is 0
        let l0 = eng.dispatch(
            1001,
            &RpcRequest::new(rpc::COPY_GET_LIST, vec![DtoValue::from(1)], 3),
            NOW,
        );
        assert_eq!(
            l0.response
                .ret
                .get("copyData")
                .and_then(|c| c.get("1001"))
                .and_then(|c| c.get("pass_strongh"))
                .and_then(DtoValue::as_i64),
            Some(0)
        );

        let pass = eng.dispatch(
            1001,
            &RpcRequest::new(
                rpc::COPY_PASS,
                vec![
                    DtoValue::from(1001u32),
                    DtoValue::from(100_101u32),
                    DtoValue::Array(vec![]),
                ],
                4,
            ),
            NOW,
        );
        assert_eq!(pass.response.err, "ok");

        let exported = eng.copy.export_player_progress(1001);
        let mut eng2 = make_engine_with_two_players();
        eng2.copy.import_player_progress(1001, &exported);
        let l1 = eng2.dispatch(
            1001,
            &RpcRequest::new(rpc::COPY_GET_LIST, vec![DtoValue::from(1)], 5),
            NOW,
        );
        let c1001 = l1
            .response
            .ret
            .get("copyData")
            .and_then(|c| c.get("1001"))
            .unwrap();
        assert_eq!(c1001.get("star").and_then(DtoValue::as_i64), Some(3));
        assert_eq!(
            c1001.get("pass_strongh").and_then(DtoValue::as_i64),
            Some(1)
        );
    }
}
