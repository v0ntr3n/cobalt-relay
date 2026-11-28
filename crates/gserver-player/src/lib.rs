#![forbid(unsafe_code)]
//! # `gserver-player`
//!
//! Pure-Rust player state, Lua wire-schema projection, progression mutation, and crash-safe
//! persistence engine for **GServer (`vnsg2`)**, backed by `gserver-protocol::json`.

macro_rules! define_wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $($variant:ident => $key:literal),+ $(,)? }
        $(aliases { $($alias:literal => $target:ident),* $(,)? })?
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name { $($variant,)+ }

        impl $name {
            pub const ALL: [$name; <[()]>::len(&[$({ let _ = stringify!($variant); }),+])] = [
                $(Self::$variant,)+
            ];

            pub const fn vnsg2_key(self) -> &'static str {
                match self { $(Self::$variant => $key,)+ }
            }

            pub fn from_key(key: &str) -> Option<Self> {
                match key {
                    $($($alias => Some(Self::$target),)*)?
                    $($key => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

macro_rules! wire_record {
    (@enc $s:ident, $m:ident, $f:ident, opt_str_nullable ()) => {
        $m.insert(
            stringify!($f).into(),
            $s.$f.as_ref().map_or(crate::json::JsonValue::Null, |v| v.clone().into()),
        );
    };
    (@enc $s:ident, $m:ident, $f:ident, json_obj ()) => {
        $m.insert(stringify!($f).into(), crate::json::JsonValue::Object($s.$f.clone()));
    };
    (@enc $s:ident, $m:ident, $f:ident, vec_u32 ()) => {
        $m.insert(stringify!($f).into(), crate::wire_helpers::encode_u32_slice(&$s.$f));
    };
    (@enc $s:ident, $m:ident, $f:ident, custom ($enc:expr, $dec:expr)) => {
        $m.insert(stringify!($f).into(), $enc);
    };
    (@enc $s:ident, $m:ident, $f:ident, req_str ()) => {
        $m.insert(stringify!($f).into(), crate::json::JsonValue::from($s.$f.clone()));
    };
    (@enc $s:ident, $m:ident, $f:ident, opt_str ($d:expr)) => {
        $m.insert(stringify!($f).into(), crate::json::JsonValue::from($s.$f.clone()));
    };
    (@enc $s:ident, $m:ident, $f:ident, $scalar_kind:ident ($($args:tt)*)) => {
        $m.insert(stringify!($f).into(), crate::json::JsonValue::from($s.$f));
    };

    (@dec $r:ident, $f:ident, req_u64 ()) => { $r.req_u64(stringify!($f))? };
    (@dec $r:ident, $f:ident, req_u32 ()) => { $r.req_u32(stringify!($f))? };
    (@dec $r:ident, $f:ident, req_str ()) => { $r.req_str(stringify!($f))? };
    (@dec $r:ident, $f:ident, opt_u64 ($d:expr)) => { $r.opt_u64(stringify!($f), $d)? };
    (@dec $r:ident, $f:ident, opt_u32 ($d:expr)) => { $r.opt_u32(stringify!($f), $d)? };
    (@dec $r:ident, $f:ident, opt_u8 ($d:expr)) => { $r.opt_u8(stringify!($f), $d)? };
    (@dec $r:ident, $f:ident, opt_i32 ($d:expr)) => { $r.opt_i32(stringify!($f), $d)? };
    (@dec $r:ident, $f:ident, opt_str ($d:expr)) => {
        $r.opt_str(stringify!($f))?.unwrap_or_else(|| ($d).into())
    };
    (@dec $r:ident, $f:ident, opt_str_nullable ()) => { $r.opt_str(stringify!($f))? };
    (@dec $r:ident, $f:ident, json_obj ()) => {
        $r.opt_obj(stringify!($f))?.cloned().unwrap_or_default()
    };
    (@dec $r:ident, $f:ident, vec_u32 ()) => {
        crate::wire_helpers::decode_u32_vec($r.opt_arr(stringify!($f))?)
    };
    (@dec $r:ident, $f:ident, custom ($enc:expr, $dec:expr)) => { $dec };

    (
        @project $map:ident, $self_id:expr
        $(, copy: [$($cf:ident),* $(,)?])?
        $(, clone: [$($clf:ident),* $(,)?])?
        $(, vec_u32: [$($vf:ident),* $(,)?])?
    ) => {
        $($( $map.insert(stringify!($cf).into(), crate::json::JsonValue::from($self_id.$cf)); )*)?
        $($( $map.insert(stringify!($clf).into(), crate::json::JsonValue::from($self_id.$clf.clone())); )*)?
        $($( $map.insert(stringify!($vf).into(), crate::wire_helpers::encode_u32_slice(&$self_id.$vf)); )*)?
    };

    (
        $(#[$smeta:meta])*
        $vis:vis struct $sname:ident {
            $($(#[$fmeta:meta])* $fname:ident : $fty:ty = $kind:ident ( $($args:tt)* ),)+
        }
        encode($self_id:ident, $map_id:ident) => $enc_vis:vis fn $enc_fn:ident $( $enc_extra:block )?;
        decode($r_id:ident, $ctx:literal) => $dec_vis:vis fn $dec_fn:ident $( ($out_id:ident) $dec_extra:block )?;
    ) => {
        $(#[$smeta])*
        $vis struct $sname { $($(#[$fmeta])* pub $fname: $fty,)+ }

        impl $sname {
            $enc_vis fn $enc_fn(&self) -> crate::json::JsonValue {
                let $self_id = self;
                let mut $map_id = std::collections::BTreeMap::new();
                $( wire_record!(@enc $self_id, $map_id, $fname, $kind ( $($args)* )); )+
                $( $enc_extra )?
                crate::json::JsonValue::Object($map_id)
            }

            $dec_vis fn $dec_fn(val: &crate::json::JsonValue) -> crate::error::Result<Self> {
                let $r_id = crate::json::JsonObjReader::new(val, $ctx)?;
                #[allow(unused_mut)]
                let mut out = Self {
                    $( $fname: wire_record!(@dec $r_id, $fname, $kind ( $($args)* )), )+
                };
                $( let $out_id = &mut out; $dec_extra )?
                Ok(out)
            }
        }
    };
}

pub(crate) mod wire_helpers {
    use std::collections::BTreeMap;
    use std::fmt::Display;
    use std::str::FromStr;

    use crate::error::{PlayerError, Result};
    use crate::json::JsonValue;

    pub fn encode_u32_slice(items: &[u32]) -> JsonValue {
        JsonValue::Array(items.iter().map(|&v| JsonValue::from(v)).collect())
    }

    pub fn decode_u32_vec(opt: Option<&[JsonValue]>) -> Vec<u32> {
        opt.into_iter()
            .flatten()
            .filter_map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
            .collect()
    }

    pub fn encode_map<K: Display, V: Copy + Into<JsonValue>>(m: &BTreeMap<K, V>) -> JsonValue {
        JsonValue::Object(m.iter().map(|(k, v)| (k.to_string(), (*v).into())).collect())
    }

    pub fn decode_map_with<K: Ord + FromStr, V>(
        opt: Option<&BTreeMap<String, JsonValue>>,
        mut key_err: impl FnMut(&str) -> String,
        mut parse_val: impl FnMut(&JsonValue) -> Result<V>,
    ) -> Result<BTreeMap<K, V>> {
        let mut out = BTreeMap::new();
        if let Some(m) = opt {
            for (k, v) in m {
                let parsed_k = k.parse::<K>().map_err(|_| PlayerError::Json(key_err(k)))?;
                out.insert(parsed_k, parse_val(v)?);
            }
        }
        Ok(out)
    }

    pub fn decode_u64_map<K: Ord + FromStr>(
        opt: Option<&BTreeMap<String, JsonValue>>,
        key_err: &str,
        val_err: &str,
    ) -> Result<BTreeMap<K, u64>> {
        decode_map_with(
            opt,
            |k| format!("{key_err} '{k}'"),
            |v| v.as_u64().ok_or_else(|| PlayerError::Json(val_err.into())),
        )
    }

    pub fn decode_u32_map<K: Ord + FromStr>(
        opt: Option<&BTreeMap<String, JsonValue>>,
        key_err: &str,
        val_err: &str,
    ) -> Result<BTreeMap<K, u32>> {
        decode_map_with(
            opt,
            |k| format!("{key_err} '{k}'"),
            |v| {
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| PlayerError::Json(val_err.into()))
            },
        )
    }

    pub fn decode_u32_map_lossy<K: Ord + FromStr>(
        opt: Option<&BTreeMap<String, JsonValue>>,
        key_err: &str,
        default_val: u32,
    ) -> Result<BTreeMap<K, u32>> {
        decode_map_with(
            opt,
            |_| key_err.into(),
            |v| Ok(v.as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(default_val)),
        )
    }

    pub fn decode_u64_map_lossy<K: Ord + FromStr>(
        opt: Option<&BTreeMap<String, JsonValue>>,
        key_err: &str,
        default_val: u64,
    ) -> Result<BTreeMap<K, u64>> {
        decode_map_with(opt, |_| key_err.into(), |v| Ok(v.as_u64().unwrap_or(default_val)))
    }
}

pub mod bag;
pub mod error;
pub mod formation;
pub mod json;
pub mod models;
pub mod mutations;
pub mod store;
pub mod wal;

pub use bag::{
    classify_template_id, default_va_item_text, BagItem, BagType, InventoryState, ItemClass,
};
pub use error::{PlayerError, Result};
pub use formation::{EquipSlotType, FormationBookState, FormationState, DEFAULT_SQUAD_TO_GRID};
pub use json::{JsonObjReader, JsonValue};
pub use models::{
    Account, CurrencyType, Gender, HeroInstance, PlayerProfile, StarterHeroSeed, StarterItemSeed,
    StarterPackConfig,
};
pub use mutations::{HeroLevelUpOutcome, UseItemEffect, UseItemOutcome};
pub use store::{atomic_write_bytes, PlayerStore, RecoveryReport};
pub use wal::{crc32_ieee, WalOp, WalRecord, WalScanOutcome, WriteAheadLog};
