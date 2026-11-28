use std::fmt;
use std::io;

/// Unified result alias for `gserver-player` operations.
pub type Result<T> = std::result::Result<T, PlayerError>;

macro_rules! define_player_error {
    (@accum [$fmt_f:ident; $(#[$emeta:meta])* $vis:vis enum $ename:ident] [$($variants:tt)*] [$($arms:tt)*]) => {
        $(#[$emeta])*
        $vis enum $ename { $($variants)* }

        impl fmt::Display for $ename {
            fn fmt(&self, $fmt_f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self { $($arms)* }
            }
        }
    };
    (@accum [$fmt_f:ident; $($hdr:tt)*] [$($variants:tt)*] [$($arms:tt)*]
        $(#[$meta:meta])* $vname:ident ($fname:ident : $fty:ty) => $fmt:literal, $($rest:tt)*
    ) => {
        define_player_error!(@accum [$fmt_f; $($hdr)*]
            [$($variants)* $(#[$meta])* $vname($fty),]
            [$($arms)* Self::$vname($fname) => write!($fmt_f, $fmt),]
            $($rest)*
        );
    };
    (@accum [$fmt_f:ident; $($hdr:tt)*] [$($variants:tt)*] [$($arms:tt)*]
        $(#[$meta:meta])* $vname:ident { $($fname:ident : $fty:ty),+ $(,)? } => $fmt:literal, $($rest:tt)*
    ) => {
        define_player_error!(@accum [$fmt_f; $($hdr)*]
            [$($variants)* $(#[$meta])* $vname { $($fname: $fty),+ },]
            [$($arms)* Self::$vname { $($fname),+ } => write!($fmt_f, $fmt),]
            $($rest)*
        );
    };
    ($(#[$emeta:meta])* $vis:vis enum $ename:ident { $($body:tt)* }) => {
        define_player_error!(@accum [f; $(#[$emeta])* $vis enum $ename] [] [] $($body)*);
    };
}

define_player_error! {
    /// Domain, serialization, and persistence errors produced by `gserver-player`.
    #[derive(Debug)]
    pub enum PlayerError {
        /// I/O failure during snapshot or WAL access.
        Io(err: io::Error) => "I/O error: {err}",
        /// Malformed or unexpected JSON syntax/schema.
        Json(msg: String) => "JSON error: {msg}",
        /// Corrupt WAL entry that could not be replayed.
        WalCorrupted { line: usize, reason: String } => "WAL corrupted at line {line}: {reason}",
        /// Invalid caller input or game invariant violation.
        InvalidParameter(msg: String) => "invalid parameter: {msg}",
        /// Account with the given `pid` or `username` already exists.
        AccountAlreadyExists(id: String) => "account already exists: {id}",
        /// Account with the given `pid` was not found.
        AccountNotFound(id: String) => "account not found: {id}",
        /// Account already owns a character in the target server group.
        CharacterAlreadyExists { pid: String, group_id: String, uid: u64 } =>
            "character already exists for pid={pid} in group={group_id} (uid={uid})",
        /// Player profile with the given `uid` does not exist.
        CharacterNotFound(uid: u64) => "character not found: uid={uid}",
        /// Hero instance with the given `hid` does not exist on the player.
        HeroNotFound(hid: u64) => "hero not found: hid={hid}",
        /// Currency balance is lower than the requested deduction.
        InsufficientCurrency { currency: &'static str, available: u64, required: u64 } =>
            "insufficient {currency}: available={available}, required={required}",
        /// Item grid or template was not found in the player's inventory.
        ItemNotFound(detail: String) => "item not found: {detail}",
        /// Total count of an item template or stack is lower than required.
        InsufficientItemCount { template_id: u32, available: u64, required: u64 } =>
            "insufficient item count for tid={template_id}: available={available}, required={required}",
        /// Sub-bag has reached its maximum unlocked grid capacity.
        BagFull { bag_type: &'static str, max_grids: u32 } =>
            "sub-bag '{bag_type}' is full (max_grids={max_grids})",
        /// Hero roster has reached `200 + 10 * hero_bag_buy_times`.
        HeroBagFull { max_heroes: u32 } => "hero bag is full (max_heroes={max_heroes})",
        /// Invalid squad, battle grid, partner, buddy, or equipment slot index.
        InvalidFormationSlot(msg: String) => "invalid formation slot: {msg}",
        /// Duplicate hero template (`htid`) cannot appear twice across squad + partner slots.
        DuplicateHeroTemplateInSquad { htid: u32 } =>
            "duplicate hero template htid={htid} in squad/partner",
        /// Protagonist hero (`utid` / `htid` in protagonist set) must remain in the active squad.
        ProtagonistCannotBeRemoved { hid: u64 } =>
            "protagonist hero hid={hid} cannot be removed or sacrificed",
        /// Item class does not match the target equipment slot (`1..=9`).
        EquipTypeMismatch { slot_type: u8, item_template_id: u32 } =>
            "item tid={item_template_id} cannot be equipped in arming slot type {slot_type}",
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
