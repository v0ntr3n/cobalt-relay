use std::fmt;
use std::io;

/// Unified result alias for `gserver-player` operations.
pub type Result<T> = std::result::Result<T, PlayerError>;

/// Domain, serialization, and persistence errors produced by `gserver-player`.
#[derive(Debug)]
pub enum PlayerError {
    /// I/O failure during snapshot or WAL access.
    Io(io::Error),
    /// Malformed or unexpected JSON syntax/schema.
    Json(String),
    /// Corrupt WAL entry that could not be replayed.
    WalCorrupted { line: usize, reason: String },
    /// Invalid caller input or game invariant violation.
    InvalidParameter(String),
    /// Account with the given `pid` or `username` already exists.
    AccountAlreadyExists(String),
    /// Account with the given `pid` was not found.
    AccountNotFound(String),
    /// Account already owns a character in the target server group.
    CharacterAlreadyExists {
        pid: String,
        group_id: String,
        uid: u64,
    },
    /// Player profile with the given `uid` does not exist.
    CharacterNotFound(u64),
    /// Hero instance with the given `hid` does not exist on the player.
    HeroNotFound(u64),
    /// Currency balance is lower than the requested deduction.
    InsufficientCurrency {
        currency: &'static str,
        available: u64,
        required: u64,
    },
    /// Item grid or template was not found in the player's inventory.
    ItemNotFound(String),
    /// Total count of an item template or stack is lower than required.
    InsufficientItemCount {
        template_id: u32,
        available: u64,
        required: u64,
    },
    /// Sub-bag has reached its maximum unlocked grid capacity.
    BagFull {
        bag_type: &'static str,
        max_grids: u32,
    },
    /// Sub-bag or hero bag has reached its maximum expansion purchase count (`buy_storage_limit`).
    BagExpandLimitReached {
        bag_type: &'static str,
        max_open_times: u32,
    },
    /// Hero roster has reached `200 + 10 * hero_bag_buy_times`.
    HeroBagFull { max_heroes: u32 },
    /// Invalid squad, battle grid, partner, buddy, or equipment slot index.
    InvalidFormationSlot(String),
    /// Duplicate hero template (`htid`) cannot appear twice across squad + partner + buddy slots.
    DuplicateHeroTemplateInSquad { htid: u32 },
    /// Protagonist hero (`utid` / `htid` in protagonist set) must remain in the active squad.
    ProtagonistCannotBeRemoved { hid: u64 },
    /// Item class does not match the target equipment slot (`1..=9` or sub-system slot).
    EquipTypeMismatch {
        slot_type: u8,
        item_template_id: u32,
    },
    /// Item or hero is locked or currently deployed and cannot be decomposed/reborn/consumed.
    ResourceLockedOrDeployed(String),
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlayerError::Io(err) => write!(f, "I/O error: {err}"),
            PlayerError::Json(msg) => write!(f, "JSON error: {msg}"),
            PlayerError::WalCorrupted { line, reason } => {
                write!(f, "WAL corrupted at line {line}: {reason}")
            }
            PlayerError::InvalidParameter(msg) => write!(f, "invalid parameter: {msg}"),
            PlayerError::AccountAlreadyExists(id) => write!(f, "account already exists: {id}"),
            PlayerError::AccountNotFound(id) => write!(f, "account not found: {id}"),
            PlayerError::CharacterAlreadyExists { pid, group_id, uid } => write!(
                f,
                "character already exists for pid={pid} in group={group_id} (uid={uid})"
            ),
            PlayerError::CharacterNotFound(uid) => write!(f, "character not found: uid={uid}"),
            PlayerError::HeroNotFound(hid) => write!(f, "hero not found: hid={hid}"),
            PlayerError::InsufficientCurrency {
                currency,
                available,
                required,
            } => write!(
                f,
                "insufficient {currency}: available={available}, required={required}"
            ),
            PlayerError::ItemNotFound(detail) => write!(f, "item not found: {detail}"),
            PlayerError::InsufficientItemCount {
                template_id,
                available,
                required,
            } => write!(
                f,
                "insufficient item count for tid={template_id}: available={available}, required={required}"
            ),
            PlayerError::BagFull {
                bag_type,
                max_grids,
            } => write!(f, "sub-bag '{bag_type}' is full (max_grids={max_grids})"),
            PlayerError::BagExpandLimitReached {
                bag_type,
                max_open_times,
            } => write!(
                f,
                "sub-bag '{bag_type}' reached expansion limit (max_open_times={max_open_times})"
            ),
            PlayerError::HeroBagFull { max_heroes } => {
                write!(f, "hero bag is full (max_heroes={max_heroes})")
            }
            PlayerError::InvalidFormationSlot(msg) => write!(f, "invalid formation slot: {msg}"),
            PlayerError::DuplicateHeroTemplateInSquad { htid } => {
                write!(f, "duplicate hero template htid={htid} in squad/partner/buddy")
            }
            PlayerError::ProtagonistCannotBeRemoved { hid } => {
                write!(f, "protagonist hero hid={hid} cannot be removed or sacrificed")
            }
            PlayerError::EquipTypeMismatch {
                slot_type,
                item_template_id,
            } => write!(
                f,
                "item tid={item_template_id} cannot be equipped in arming slot type {slot_type}"
            ),
            PlayerError::ResourceLockedOrDeployed(msg) => {
                write!(f, "resource locked or deployed: {msg}")
            }
        }
    }
}

impl std::error::Error for PlayerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PlayerError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for PlayerError {
    fn from(err: io::Error) -> Self {
        PlayerError::Io(err)
    }
}
