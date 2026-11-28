//! Structured error types for AMF3 serialization, RFC 1950/1951 Zlib compression,
//! TCP packet framing, RPC envelopes, and asset decryption.

/// Declarative error enum generator deriving `Debug`, `Clone`, `PartialEq`, `Eq`,
/// [`core::fmt::Display`], [`std::error::Error`] (with optional `#[source]` chaining),
/// and [`core::convert::From`] conversions for `#[source]` tuple variants.
#[macro_export]
macro_rules! define_error {
    ($( $(#[$emeta:meta])* $vis:vis enum $ename:ident { $($body:tt)* } )+) => {
        $( $crate::define_error!(@m fmt_out [$(#[$emeta])*] [$vis] $ename [] [] [] [] $($body)*); )+
    };
    (@m $f:ident [$(#[$emeta:meta])*] [$vis:vis] $ename:ident [$($defs:tt)*] [$($disp:tt)*] [$($src:tt)*] [$($froms:tt)*]) => {
        $(#[$emeta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        $vis enum $ename { $($defs)* }

        impl ::core::fmt::Display for $ename {
            #[allow(unused_variables)]
            fn fmt(&self, $f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self { $($disp)* }
            }
        }

        impl ::std::error::Error for $ename {
            fn source(&self) -> ::core::option::Option<&(dyn ::std::error::Error + 'static)> {
                match self {
                    $($src)*
                    #[allow(unreachable_patterns)]
                    _ => ::core::option::Option::None,
                }
            }
        }

        $($froms)*
    };
    (@m $f:ident $em:tt $vis:tt $en:ident [$($d:tt)*] [$($p:tt)*] $s:tt $fr:tt
        $(#[$vm:meta])* $vn:ident => $fmt:literal $(, $($rest:tt)*)?) => {
        $crate::define_error!(@m $f $em $vis $en [$($d)* $(#[$vm])* $vn,]
            [$($p)* Self::$vn => ::core::write!($f, $fmt),] $s $fr $($($rest)*)?);
    };
    (@m $f:ident $em:tt $vis:tt $en:ident [$($d:tt)*] [$($p:tt)*] $s:tt $fr:tt
        $(#[$vm:meta])* $vn:ident { $($fnm:ident : $fty:ty),* $(,)? } => $fmt:literal $(, $($rest:tt)*)?) => {
        $crate::define_error!(@m $f $em $vis $en [$($d)* $(#[$vm])* $vn { $($fnm : $fty),* },]
            [$($p)* Self::$vn { $($fnm),* } => ::core::write!($f, $fmt),] $s $fr $($($rest)*)?);
    };
    (@m $f:ident $em:tt $vis:tt $en:ident [$($d:tt)*] [$($p:tt)*] [$($s:tt)*] [$($fr:tt)*]
        $(#[$vm:meta])* $vn:ident ( #[source] $pn:ident : $pty:ty $(,)? ) => $fmt:literal $(, $($rest:tt)*)?) => {
        $crate::define_error!(@m $f $em $vis $en [$($d)* $(#[$vm])* $vn($pty),]
            [$($p)* Self::$vn($pn) => ::core::write!($f, $fmt),]
            [$($s)* Self::$vn($pn) => ::core::option::Option::Some($pn),]
            [$($fr)* impl ::core::convert::From<$pty> for $en { fn from(v: $pty) -> Self { Self::$vn(v) } }]
            $($($rest)*)?);
    };
    (@m $f:ident $em:tt $vis:tt $en:ident [$($d:tt)*] [$($p:tt)*] $s:tt $fr:tt
        $(#[$vm:meta])* $vn:ident ( $($pn:ident : $pty:ty),+ $(,)? ) => $fmt:literal $(, $($rest:tt)*)?) => {
        $crate::define_error!(@m $f $em $vis $en [$($d)* $(#[$vm])* $vn($($pty),+),]
            [$($p)* Self::$vn($($pn),+) => ::core::write!($f, $fmt),] $s $fr $($($rest)*)?);
    };
    (@m $f:ident $em:tt $vis:tt $en:ident [$($d:tt)*] [$($p:tt)*] $s:tt $fr:tt
        $(#[$vm:meta])* $vn:ident ( $($pty:ty),+ $(,)? ) => $fmt:literal $(, $($rest:tt)*)?) => {
        $crate::define_error!(@m $f $em $vis $en [$($d)* $(#[$vm])* $vn($($pty),+),]
            [$($p)* Self::$vn(..) => ::core::write!($f, $fmt),] $s $fr $($($rest)*)?);
    };
}

define_error! {
    /// Unified protocol error covering all layers of `gserver-protocol`.
    pub enum ProtocolError {
        /// AMF3 encoding or decoding failure.
        Amf3(#[source] e: Amf3Error) => "{e}",
        /// RFC 1950 Zlib or RFC 1951 Deflate failure.
        Zlib(#[source] e: ZlibError) => "{e}",
        /// C2S / S2C TCP packet framing or integrity failure.
        Frame(#[source] e: FrameError) => "{e}",
        /// High-level RPC or server push envelope schema mismatch.
        Envelope(#[source] e: EnvelopeError) => "{e}",
        /// `0xFEFEFEFE` `bt_decrypt` asset decryption error.
        BtDecrypt(#[source] e: BtDecryptError) => "{e}",
    }

    /// AMF3 serialization and deserialization errors.
    pub enum Amf3Error {
        UnexpectedEof { offset: usize, needed: usize, remaining: usize } => "AMF3 unexpected EOF at offset {offset}: needed {needed} bytes, {remaining} remaining",
        InvalidMarker { marker: u8, offset: usize } => "AMF3 unsupported type marker 0x{marker:02X} at offset {offset}",
        InvalidStringRef { index: usize, table_len: usize, offset: usize } => "AMF3 invalid string ref {index} (table len {table_len}) at offset {offset}",
        InvalidObjectRef { index: usize, table_len: usize, offset: usize } => "AMF3 invalid object ref {index} (table len {table_len}) at offset {offset}",
        InvalidTraitRef { index: usize, table_len: usize, offset: usize } => "AMF3 invalid trait ref {index} (table len {table_len}) at offset {offset}",
        CyclicObjectRef { index: usize, offset: usize } => "AMF3 cyclic self-reference to in-progress object ref {index} at offset {offset}",
        RecursionLimitExceeded { max_depth: usize, offset: usize } => "AMF3 nesting depth exceeded limit ({max_depth}) at offset {offset}",
        AllocationLimitExceeded { kind: &'static str, requested: usize, limit: usize, offset: usize } => "AMF3 {kind} size {requested} exceeds limit {limit} at offset {offset}",
        LengthOverflow { kind: &'static str, len: usize } => "AMF3 {kind} length {len} exceeds 28-bit U29 maximum",
        InvalidUtf8 { offset: usize } => "AMF3 string is not valid UTF-8 at offset {offset}",
    }

    /// RFC 1950 Zlib and RFC 1951 Deflate decompression/compression errors.
    pub enum ZlibError {
        UnexpectedEof => "zlib unexpected end of stream",
        InvalidZlibHeader { cmf: u8, flg: u8 } => "invalid RFC 1950 zlib header: CMF=0x{cmf:02X}, FLG=0x{flg:02X}",
        PresetDictionaryUnsupported => "RFC 1950 FDICT preset dictionary is not supported",
        InvalidBlockType => "invalid RFC 1951 deflate block type 0b11",
        StoredLengthMismatch { len: u16, nlen: u16 } => "deflate stored block LEN/NLEN mismatch: LEN=0x{len:04X}, NLEN=0x{nlen:04X}",
        InvalidDynamicHeader { hlit: usize, hdist: usize, hclen: usize } => "invalid deflate dynamic Huffman counts: HLIT={hlit}, HDIST={hdist}, HCLEN={hclen}",
        InvalidHuffmanTable(ctx: &'static str) => "invalid Huffman table ({ctx})",
        InvalidHuffmanCode => "invalid or unmapped Huffman code in bitstream",
        InvalidCodeLengthRepeat => "invalid repeat sequence in dynamic Huffman code-length table",
        InvalidBackReference { distance: usize, output_len: usize } => "LZ77 distance {distance} exceeds current output length {output_len}",
        OutputLimitExceeded { max_output_size: usize } => "decompressed output exceeded maximum limit of {max_output_size} bytes",
        Adler32Mismatch { expected: u32, actual: u32 } => "zlib Adler-32 checksum mismatch: expected 0x{expected:08X}, computed 0x{actual:08X}",
    }

    /// TCP packet framing and BabelTime `LCPacket` integrity errors.
    pub enum FrameError {
        ZeroBodyLength => "TCP frame declared invalid bodyLen == 0",
        BodyTooLarge { body_len: u32, max_body_len: u32 } => "TCP frame bodyLen {body_len} exceeds limit {max_body_len}",
        PayloadOverflow { len: usize } => "TCP frame payload length {len} exceeds maximum allowed frame size",
        InvalidLcFlags(flags: [u8; 4]) => "invalid LCPacket flags {flags:?} (expected [0|1, 1, 1, 1])",
        Md5Mismatch { expected: [u8; 16], actual: [u8; 16] } => "BabelTime LCPacket 16-byte MD5 integrity mismatch",
        UnrecognizedC2sHeader(hdr: [u8; 8]) => "unrecognized C2S frame header prefix {hdr:02X?}",
        InvalidMultiSubPacket { offset: usize, declared_len: usize, remaining: usize } => "invalid multi sub-packet at offset {offset}: len {declared_len} > remaining {remaining}",
    }

    /// High-level GServer RPC request / response envelope errors.
    pub enum EnvelopeError {
        ExpectedMap => "expected AMF3 Object or associative Array envelope",
        MissingField(field: &'static str) => "missing required envelope field '{field}'",
        InvalidFieldType { field: &'static str, expected: &'static str } => "envelope field '{field}' expected {expected}",
        EmptyMultiBatch => "multi S2C envelope must contain at least 1 item",
    }

    /// `0xFEFEFEFE` `bt_decrypt` asset errors (ported from `BtDecryptHook.h`).
    pub enum BtDecryptError {
        TruncatedHeader { len: usize } => "truncated 0xFEFEFEFE bt_decrypt header ({len} < 32 bytes)",
        PayloadLengthMismatch { header_payload_size: u32, actual_payload_size: usize } => "bt_decrypt payload mismatch: header={header_payload_size}, actual={actual_payload_size}",
        UnsupportedAlgoIndex(algo: u8) => "unsupported bt_decrypt algoIndex {algo} (expected 0 or 1)",
        InvalidKeyIndex(idx: u8) => "bt_decrypt keyIndex {idx} out of range (expected 0..=15)",
    }
}
