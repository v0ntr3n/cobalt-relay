#![forbid(unsafe_code)]

//! Deterministic SplitMix64 + Xoshiro256** PRNG fuzzing engine (`#![forbid(unsafe_code)]`,
//! zero external dependencies) for structured and mutational protocol fuzzing across:
//! - `Amf3Encoder` / `Amf3Decoder` (round-trip fidelity, allocation-bomb resistance, depth limits)
//! - `zlib_compress` / `zlib_decompress_bounded` (corrupt headers, bit-flips, truncation, zip-bombs)
//! - `ServerRxStreamDecoder` & `Win32ClientSimulator` (arbitrary chunking, random mutation)
//! - `route_sdk_http_request` & `parse_tolerant_query_params`

use std::collections::HashMap;
use std::sync::Mutex;

use crate::amf3::{
    Amf3Decoder, Amf3Encoder, Amf3Value, AMF3_INT_MAX, AMF3_INT_MIN,
};
use crate::crypto::compute_babeltime_md5;
use crate::framing::{
    encode_client_lc_packet, encode_server_single_frame, ServerRxStreamDecoder,
    Win32ClientSimulator, CLIENT_LC_HEADER_LEN,
};
use crate::http_gateway::{route_sdk_http_request, HttpGatewayConfig, HttpGatewayMetrics};
use crate::zlib::{zlib_compress, zlib_decompress_bounded};

/// Canonical deterministic seed (`"GCLIENTVNS"`).
pub const DEFAULT_FUZZ_SEED: u64 = 0x4F4D_4733_5156_4E53;

/// Reproducible Xoshiro256** PRNG seeded via SplitMix64.
#[derive(Debug, Clone)]
pub struct DeterministicPrng {
    s: [u64; 4],
}

impl DeterministicPrng {
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let mut next_sm = || -> u64 {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self {
            s: [next_sm(), next_sm(), next_sm(), next_sm()],
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = (self.s[1].wrapping_mul(5)).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;

        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];

        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    pub fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    pub fn gen_range(&mut self, upper_exclusive: usize) -> usize {
        if upper_exclusive <= 1 {
            0
        } else {
            (self.next_u64() as usize) % upper_exclusive
        }
    }

    pub fn gen_bool(&mut self) -> bool {
        (self.next_u64() & 1) != 0
    }

    pub fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut i = 0;
        while i + 8 <= buf.len() {
            let w = self.next_u64().to_le_bytes();
            buf[i..i + 8].copy_from_slice(&w);
            i += 8;
        }
        while i < buf.len() {
            buf[i] = self.next_u8();
            i += 1;
        }
    }

    /// Generates a random valid `Amf3Value` tree exercising string deduplication, boundary U29s,
    /// large timestamps, binary strings, ByteArrays, empty `{}` tables, mixed arrays, and objects.
    pub fn gen_amf3_value(&mut self, depth: usize, max_depth: usize) -> Amf3Value {
        const POOL_STRINGS: &[&[u8]] = &[
            b"",
            b"err",
            b"ok",
            b"ret",
            b"callback",
            b"callbackName",
            b"token",
            b"time",
            b"user.login_2",
            b"vnsg2",
            b"\x00\xFF\xFE\x80raw_lua_binary\x00",
        ];
        const BOUNDARY_INTS: &[i64] = &[
            -268_435_457,
            AMF3_INT_MIN as i64,
            -2_097_152,
            -16_384,
            -128,
            -1,
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            2_097_151,
            2_097_152,
            AMF3_INT_MAX as i64,
            268_435_456,
            1_795_500_450,
            -1_795_500_450,
        ];

        let choice = if depth >= max_depth {
            self.gen_range(8)
        } else {
            self.gen_range(11)
        };

        match choice {
            0 => Amf3Value::Undefined,
            1 => Amf3Value::Null,
            2 => Amf3Value::Bool(self.gen_bool()),
            3 => {
                if self.gen_bool() {
                    let b = BOUNDARY_INTS[self.gen_range(BOUNDARY_INTS.len())];
                    Amf3Value::from_i64_auto(b)
                } else {
                    let raw = (self.next_u32() as i32) >> 3;
                    Amf3Value::from_i64_auto(raw as i64)
                }
            }
            4 => {
                if self.gen_range(6) == 0 {
                    Amf3Value::Double(-0.0)
                } else {
                    let f = (self.next_u64() as i64 as f64) / 1024.5;
                    Amf3Value::Double(f)
                }
            }
            5 => {
                if self.gen_bool() {
                    Amf3Value::String(POOL_STRINGS[self.gen_range(POOL_STRINGS.len())].to_vec())
                } else {
                    let len = self.gen_range(24);
                    let mut b = vec![0u8; len];
                    self.fill_bytes(&mut b);
                    Amf3Value::String(b)
                }
            }
            6 => Amf3Value::Date((self.next_u32() as f64) * 1000.0),
            7 => {
                let len = self.gen_range(20);
                let mut b = vec![0u8; len];
                self.fill_bytes(&mut b);
                Amf3Value::ByteArray(b)
            }
            8 => {
                if self.gen_range(3) == 0 {
                    Amf3Value::empty_lua_table()
                } else {
                    let assoc_n = self.gen_range(3);
                    let dense_n = self.gen_range(5);
                    let mut assoc = Vec::with_capacity(assoc_n);
                    for idx in 0..assoc_n {
                        let key = format!("k{}_{}", idx, self.gen_range(4)).into_bytes();
                        assoc.push((key, self.gen_amf3_value(depth + 1, max_depth)));
                    }
                    let mut dense = Vec::with_capacity(dense_n);
                    for _ in 0..dense_n {
                        dense.push(self.gen_amf3_value(depth + 1, max_depth));
                    }
                    Amf3Value::Array { assoc, dense }
                }
            }
            9 => {
                let n = self.gen_range(5);
                let mut members = Vec::with_capacity(n);
                for idx in 0..n {
                    let key = match idx {
                        0 => b"err".to_vec(),
                        1 => b"ret".to_vec(),
                        _ => format!("f{}", idx).into_bytes(),
                    };
                    members.push((key, self.gen_amf3_value(depth + 1, max_depth)));
                }
                Amf3Value::Object {
                    class_name: Vec::new(),
                    dynamic: true,
                    sealed: Vec::new(),
                    dynamic_members: members,
                    externalizable: None,
                }
            }
            _ => {
                let class_tag = self.s[0] ^ self.s[2];
                let sealed_n = 1 + self.gen_range(3);
                let mut sealed = Vec::with_capacity(sealed_n);
                for idx in 0..sealed_n {
                    sealed.push((
                        format!("s{}", idx).into_bytes(),
                        self.gen_amf3_value(depth + 1, max_depth),
                    ));
                }
                Amf3Value::Object {
                    class_name: format!("bt.Model.{class_tag:016x}").into_bytes(),
                    dynamic: false,
                    sealed,
                    dynamic_members: Vec::new(),
                    externalizable: None,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FuzzCampaignSummary {
    pub structured_roundtrips: usize,
    pub mutated_amf3_cases: usize,
    pub mutated_zlib_cases: usize,
    pub mutated_frame_cases: usize,
    pub mutated_http_cases: usize,
}

/// Executes a deterministic, reproducible fuzz campaign across all protocol parsers and codecs,
/// asserting 100% round-trip exactness on valid trees and zero panics/OOMs on hostile mutations.
pub fn run_deterministic_fuzz_campaign(seed: u64, iterations: usize) -> FuzzCampaignSummary {
    let mut rng = DeterministicPrng::new(seed);
    let mut summary = FuzzCampaignSummary::default();
    let http_cfg = HttpGatewayConfig::default();
    let http_metrics = HttpGatewayMetrics::default();
    let http_sessions = Mutex::new(HashMap::new());

    for _ in 0..iterations {
        // 1. Structured AMF3 + Zlib + Framing round-trip
        let val = rng.gen_amf3_value(0, 4);
        let encoded = Amf3Encoder::encode(&val).expect("structured Amf3Value must encode");
        let (decoded, end_pos) =
            Amf3Decoder::decode_at(&encoded, 0).expect("encoded AMF3 must decode");
        assert_eq!(end_pos, encoded.len(), "AMF3 decoder must consume exact byte count");
        assert_amf3_bits_eq(&val, &decoded);

        let compressed = zlib_compress(&encoded);
        let decompressed =
            zlib_decompress_bounded(&compressed, 1024 * 1024).expect("zlib round-trip must succeed");
        assert_eq!(decompressed, encoded);

        let lc_pkt = encode_client_lc_packet(&encoded, rng.gen_bool());
        let mut srv_dec = ServerRxStreamDecoder::new(65536, 65536);
        let out_pkts = srv_dec.push_bytes(&lc_pkt).expect("valid LCPacket must decode");
        assert_eq!(out_pkts.len(), 1);
        assert_amf3_bits_eq(&val, &out_pkts[0].value);
        summary.structured_roundtrips += 1;

        // 2. Hostile mutational AMF3 fuzzing (random bit-flips, U29 length bombs, truncation, random bytes)
        let mut mutated = if rng.gen_bool() {
            encoded.clone()
        } else {
            let len = rng.gen_range(96);
            let mut raw = vec![0u8; len];
            rng.fill_bytes(&mut raw);
            raw
        };
        if !mutated.is_empty() {
            let flips = 1 + rng.gen_range(4);
            for _ in 0..flips {
                let idx = rng.gen_range(mutated.len());
                mutated[idx] ^= 1u8 << rng.gen_range(8);
            }
            if rng.gen_bool() {
                let trunc = rng.gen_range(mutated.len());
                mutated.truncate(trunc);
            }
        }
        let _ = Amf3Decoder::decode(&mutated);
        summary.mutated_amf3_cases += 1;

        // 3. Hostile mutational Zlib fuzzing (capped at 4 KiB output to catch zip-bombs fast)
        let mut zmut = compressed;
        if !zmut.is_empty() {
            let idx = rng.gen_range(zmut.len());
            zmut[idx] = rng.next_u8();
            if rng.gen_range(4) == 0 {
                let trunc = rng.gen_range(zmut.len());
                zmut.truncate(trunc);
            }
        }
        let _ = zlib_decompress_bounded(&zmut, 4096);
        summary.mutated_zlib_cases += 1;

        // 4. Hostile mutational framing fuzzing (both ServerRxStreamDecoder & Win32ClientSimulator)
        let mut fmut = lc_pkt;
        if fmut.len() >= CLIENT_LC_HEADER_LEN && rng.gen_bool() {
            // Recompute valid BabelTime MD5 over mutated body so fuzzer penetrates PAST the MD5 gate!
            let body_idx = CLIENT_LC_HEADER_LEN + rng.gen_range(fmut.len() - CLIENT_LC_HEADER_LEN);
            fmut[body_idx] ^= rng.next_u8().max(1);
            let new_md5 = compute_babeltime_md5(&fmut[CLIENT_LC_HEADER_LEN..]);
            fmut[8..24].copy_from_slice(&new_md5);
        } else if !fmut.is_empty() {
            let idx = rng.gen_range(fmut.len());
            fmut[idx] ^= rng.next_u8();
        }
        let mut fuzz_srv = ServerRxStreamDecoder::new(8192, 8192);
        let _ = fuzz_srv.push_bytes(&fmut);

        if let Ok(mut srv_frame) = encode_server_single_frame(&val, rng.gen_bool()) {
            if !srv_frame.is_empty() {
                let idx = rng.gen_range(srv_frame.len());
                srv_frame[idx] ^= rng.next_u8();
            }
            let mut sim = Win32ClientSimulator::new();
            let _ = sim.receive_server_bytes(&srv_frame, 1_795_500_450);
        }
        summary.mutated_frame_cases += 1;

        // 5. Hostile HTTP query fuzzing (`/phone/gethash/?&&pid=%ZZ...`)
        let mut qbytes = vec![0u8; rng.gen_range(64)];
        rng.fill_bytes(&mut qbytes);
        let noisy_q: String = qbytes
            .iter()
            .map(|&b| match b % 8 {
                0 => '&',
                1 => '=',
                2 => '%',
                3 => '?',
                _ => (b'a' + (b % 26)) as char,
            })
            .collect();
        let target = format!("/phone/gethash/?&{}", noisy_q);
        let (status, _, body) =
            route_sdk_http_request(&target, Some("PHPSESSID=fuzz;;="), &http_cfg, &http_metrics, &http_sessions);
        assert_eq!(status, 200);
        assert!(body.starts_with('{'));
        summary.mutated_http_cases += 1;
    }

    summary
}

/// Bit-exact comparison handling `-0.0` vs `+0.0` and `NaN` bit patterns.
pub fn assert_amf3_bits_eq(a: &Amf3Value, b: &Amf3Value) {
    match (a, b) {
        (Amf3Value::Double(da), Amf3Value::Double(db)) => {
            assert_eq!(da.to_bits(), db.to_bits(), "Double bits mismatch: {} vs {}", da, db);
        }
        (Amf3Value::Date(da), Amf3Value::Date(db)) => {
            assert_eq!(da.to_bits(), db.to_bits(), "Date bits mismatch: {} vs {}", da, db);
        }
        (
            Amf3Value::Array {
                assoc: aa,
                dense: da,
            },
            Amf3Value::Array {
                assoc: ab,
                dense: db,
            },
        ) => {
            assert_eq!(aa.len(), ab.len());
            for ((ka, va), (kb, vb)) in aa.iter().zip(ab.iter()) {
                assert_eq!(ka, kb);
                assert_amf3_bits_eq(va, vb);
            }
            assert_eq!(da.len(), db.len());
            for (va, vb) in da.iter().zip(db.iter()) {
                assert_amf3_bits_eq(va, vb);
            }
        }
        (
            Amf3Value::Object {
                class_name: ca,
                dynamic: dyna,
                sealed: sa,
                dynamic_members: dma,
                externalizable: ea,
            },
            Amf3Value::Object {
                class_name: cb,
                dynamic: dynb,
                sealed: sb,
                dynamic_members: dmb,
                externalizable: eb,
            },
        ) => {
            assert_eq!(ca, cb);
            assert_eq!(dyna, dynb);
            assert_eq!(sa.len(), sb.len());
            for ((ka, va), (kb, vb)) in sa.iter().zip(sb.iter()) {
                assert_eq!(ka, kb);
                assert_amf3_bits_eq(va, vb);
            }
            assert_eq!(dma.len(), dmb.len());
            for ((ka, va), (kb, vb)) in dma.iter().zip(dmb.iter()) {
                assert_eq!(ka, kb);
                assert_amf3_bits_eq(va, vb);
            }
            match (ea, eb) {
                (Some(va), Some(vb)) => assert_amf3_bits_eq(va, vb),
                (None, None) => {}
                _ => panic!("externalizable mismatch"),
            }
        }
        _ => assert_eq!(a, b),
    }
}
