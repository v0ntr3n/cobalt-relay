//! Babeltime (`vnsg2`) `babeltime` row schema & CSV field parser matching `BaseDBReader.lua`.
//!
//! In GServer SQLite `.db` tables:
//! - Row `id = "0"` holds `"col1,col2,...\\~type1,type2,..."` (where `type` is `"array"` or `""`).
//! - Data rows (`id != "0"`) hold comma-separated values where `\\~` separates array items
//!   and `|` separates tuple fields (such as `type|id|count` reward triples).

use std::collections::BTreeMap;

use gserver_protocol::bt_decrypt::{decrypt_babeltime_cell, DecryptResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtColumnKind {
    Scalar,
    Array,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtColumnSpec {
    pub name: String,
    pub kind: BtColumnKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtFieldValue {
    Empty,
    Scalar(String),
    Array(Vec<Vec<String>>),
}

impl BtFieldValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Scalar(s) => s.parse::<i64>().ok(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Scalar(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Parses the decrypted `babeltime` string of row `id = "0"` into column definitions.
pub fn parse_babeltime_schema_row(decrypted_row0: &str) -> Vec<BtColumnSpec> {
    let mut parts = decrypted_row0.split("\\~");
    let names_part = parts.next().unwrap_or("");
    let types_part = parts.next().unwrap_or("");
    if names_part.is_empty() {
        return Vec::new();
    }
    let kinds: Vec<&str> = types_part.split(',').collect();
    names_part
        .split(',')
        .enumerate()
        .map(|(i, name)| BtColumnSpec {
            name: name.to_string(),
            kind: if kinds.get(i).copied() == Some("array") {
                BtColumnKind::Array
            } else {
                BtColumnKind::Scalar
            },
        })
        .collect()
}

/// Parses a decrypted `babeltime` data row (`id != "0"`) using the schema from row `0`.
pub fn parse_babeltime_data_row(
    schema: &[BtColumnSpec],
    decrypted_row: &str,
) -> BTreeMap<String, BtFieldValue> {
    let raw_fields: Vec<&str> = decrypted_row.split(',').collect();
    let mut map = BTreeMap::new();
    for (idx, col) in schema.iter().enumerate() {
        if col.name.is_empty() {
            continue;
        }
        let raw = raw_fields.get(idx).copied().unwrap_or("");
        let val = match (col.kind, raw.is_empty()) {
            (BtColumnKind::Scalar, true) => BtFieldValue::Empty,
            (BtColumnKind::Scalar, false) => BtFieldValue::Scalar(raw.replace("\\~", ", ")),
            (BtColumnKind::Array, true) => BtFieldValue::Array(Vec::new()),
            (BtColumnKind::Array, false) => BtFieldValue::Array(
                raw.split("\\~")
                    .filter(|s| !s.is_empty())
                    .map(|item| item.split('|').map(ToString::to_string).collect())
                    .collect(),
            ),
        };
        map.insert(col.name.clone(), val);
    }
    map
}

/// Decrypts a raw SQLite `babeltime` cell via [`gserver_protocol::bt_decrypt::decrypt_babeltime_cell`]
/// and parses its fields against `schema`.
pub fn decrypt_and_parse_babeltime_row(
    schema: &[BtColumnSpec],
    raw_cell: &str,
) -> Result<BTreeMap<String, BtFieldValue>, DecryptResult> {
    let decrypted = decrypt_babeltime_cell(raw_cell)?;
    Ok(parse_babeltime_data_row(schema, &decrypted))
}

/// Reward triple `type|id|count` used across GServer tables (`copy.db`, `online.db`, `shop.db`, `item_direct`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewardTriple {
    pub reward_type: u32,
    pub template_id: u32,
    pub count: u64,
}

impl RewardTriple {
    /// Parses `"4|22102|1"` into `RewardTriple { reward_type: 4, template_id: 22102, count: 1 }`.
    pub fn parse(token: &str) -> Option<Self> {
        let mut it = token.split('|');
        let reward_type = it.next()?.parse::<u32>().ok()?;
        let template_id = it.next()?.parse::<u32>().ok()?;
        let count = it.next()?.parse::<u64>().ok()?;
        it.next().is_none().then_some(Self {
            reward_type,
            template_id,
            count,
        })
    }
}
