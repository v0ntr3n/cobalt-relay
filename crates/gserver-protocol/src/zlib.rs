//! Pure-Rust Safe RFC 1950 (Zlib) + RFC 1951 (Deflate) decompressor and compressor
//! with Adler-32 integrity verification.
//!
//! Zero external crates and `#![forbid(unsafe_code)]`.
//! - Decompressor supports Stored (`BTYPE=00`), Fixed Huffman (`BTYPE=01`), and
//!   Dynamic Huffman (`BTYPE=10`) blocks, full LZ77 back-references, output size caps,
//!   and RFC 1950 header + Adler-32 validation.
//! - Compressor supports Stored (`BTYPE=00`) and Fixed Huffman (`BTYPE=01`, both literal-only
//!   and LZ77 hash-chain compressed) modes with RFC 1950 header + Adler-32 trailer.

use crate::error::ZlibError;

/// Default maximum decompressed output size (64 MiB, matching `LCPacket.h`).
pub const DEFAULT_MAX_DECOMPRESSED_SIZE: usize = 64 * 1024 * 1024;

const MOD_ADLER: u32 = 65521;
const ADLER_NMAX: usize = 5552;
const MAX_BITS: usize = 15;
const MAX_LIT_CODES: usize = 288;
const MAX_DIST_CODES: usize = 32;

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];

const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Compression strategy for [`zlib_compress_with_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionMode {
    /// RFC 1951 Stored (`BTYPE=00`) uncompressed blocks inside an RFC 1950 Zlib stream.
    Stored,
    /// RFC 1951 Fixed Huffman (`BTYPE=01`) encoding literals without LZ77 search.
    FixedLiteralsOnly,
    /// RFC 1951 Fixed Huffman (`BTYPE=01`) with 32 KiB LZ77 hash-chain matching (default).
    #[default]
    FixedHuffmanLz77,
}

/// Computes the RFC 1950 Adler-32 checksum of `data`.
pub fn adler32(data: &[u8]) -> u32 {
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;

    for chunk in data.chunks(ADLER_NMAX) {
        for &b in chunk {
            s1 += u32::from(b);
            s2 += s1;
        }
        s1 %= MOD_ADLER;
        s2 %= MOD_ADLER;
    }

    (s2 << 16) | s1
}

/// Decompresses an RFC 1950 Zlib stream using [`DEFAULT_MAX_DECOMPRESSED_SIZE`] (64 MiB).
pub fn zlib_decompress(input: &[u8]) -> Result<Vec<u8>, ZlibError> {
    zlib_decompress_with_limit(input, DEFAULT_MAX_DECOMPRESSED_SIZE)
}

/// Decompresses an RFC 1950 Zlib stream while enforcing `max_output_size` bytes.
pub fn zlib_decompress_with_limit(
    input: &[u8],
    max_output_size: usize,
) -> Result<Vec<u8>, ZlibError> {
    if input.len() < 6 {
        return Err(ZlibError::UnexpectedEof);
    }

    let cmf = input[0];
    let flg = input[1];
    let cm = cmf & 0x0F;
    let cinfo = (cmf >> 4) & 0x0F;
    let hdr_u16 = (u16::from(cmf) << 8) | u16::from(flg);

    if cm != 8 || cinfo > 7 || (hdr_u16 % 31) != 0 {
        return Err(ZlibError::InvalidZlibHeader { cmf, flg });
    }
    if (flg & 0x20) != 0 {
        return Err(ZlibError::PresetDictionaryUnsupported);
    }

    let mut reader = BitReader::new(&input[2..]);
    let mut output = Vec::new();
    inflate_stream(&mut reader, &mut output, max_output_size)?;

    reader.align_to_byte();
    let trailer = reader.read_aligned_bytes(4)?;
    let expected = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let actual = adler32(&output);
    if expected != actual {
        return Err(ZlibError::Adler32Mismatch { expected, actual });
    }

    Ok(output)
}

/// Compresses `input` into a valid RFC 1950 Zlib stream using [`CompressionMode::FixedHuffmanLz77`].
pub fn zlib_compress(input: &[u8]) -> Vec<u8> {
    zlib_compress_with_mode(input, CompressionMode::FixedHuffmanLz77)
}

/// Compresses `input` into a valid RFC 1950 Zlib stream with the chosen [`CompressionMode`].
pub fn zlib_compress_with_mode(input: &[u8], mode: CompressionMode) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len().saturating_add(16));
    // RFC 1950 header: CM=8 (Deflate), CINFO=7 (32K window) => CMF=0x78.
    // FLG=0x01 for Stored (FLEVEL=0, 0x7801 % 31 == 0), FLG=0x9C for default compression (0x789C % 31 == 0).
    let flg = match mode {
        CompressionMode::Stored => 0x01u8,
        CompressionMode::FixedLiteralsOnly | CompressionMode::FixedHuffmanLz77 => 0x9Cu8,
    };
    out.push(0x78);
    out.push(flg);

    match mode {
        CompressionMode::Stored => deflate_stored(input, &mut out),
        CompressionMode::FixedLiteralsOnly => deflate_fixed_literals(input, &mut out),
        CompressionMode::FixedHuffmanLz77 => deflate_fixed_lz77(input, &mut out),
    }

    out.extend_from_slice(&adler32(input).to_be_bytes());
    out
}

// ============================================================================
// LSB-first BitReader & BitWriter
// ============================================================================

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_buf: u32,
    bit_count: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    #[inline]
    fn read_bit(&mut self) -> Result<u32, ZlibError> {
        if self.bit_count == 0 {
            if self.byte_pos >= self.data.len() {
                return Err(ZlibError::UnexpectedEof);
            }
            self.bit_buf = u32::from(self.data[self.byte_pos]);
            self.byte_pos += 1;
            self.bit_count = 8;
        }
        let bit = self.bit_buf & 1;
        self.bit_buf >>= 1;
        self.bit_count -= 1;
        Ok(bit)
    }

    #[inline]
    fn read_bits(&mut self, count: u8) -> Result<u32, ZlibError> {
        if count == 0 {
            return Ok(0);
        }
        while self.bit_count < count {
            if self.byte_pos >= self.data.len() {
                return Err(ZlibError::UnexpectedEof);
            }
            self.bit_buf |= u32::from(self.data[self.byte_pos]) << self.bit_count;
            self.byte_pos += 1;
            self.bit_count += 8;
        }
        let mask = (1u32 << count) - 1;
        let val = self.bit_buf & mask;
        self.bit_buf >>= count;
        self.bit_count -= count;
        Ok(val)
    }

    fn align_to_byte(&mut self) {
        self.bit_buf = 0;
        self.bit_count = 0;
    }

    fn read_aligned_bytes(&mut self, len: usize) -> Result<&'a [u8], ZlibError> {
        debug_assert_eq!(self.bit_count, 0);
        if len > self.data.len() || self.byte_pos > self.data.len() - len {
            return Err(ZlibError::UnexpectedEof);
        }
        let slice = &self.data[self.byte_pos..self.byte_pos + len];
        self.byte_pos += len;
        Ok(slice)
    }
}

struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    bit_buf: u32,
    bit_count: u8,
}

impl<'a> BitWriter<'a> {
    fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    #[inline]
    fn write_bits(&mut self, val: u32, count: u8) {
        if count == 0 {
            return;
        }
        self.bit_buf |= (val & ((1u32 << count) - 1)) << self.bit_count;
        self.bit_count += count;
        while self.bit_count >= 8 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
    }

    #[inline]
    fn write_huffman_msb(&mut self, code: u16, len: u8) {
        for shift in (0..len).rev() {
            let bit = u32::from((code >> shift) & 1);
            self.write_bits(bit, 1);
        }
    }

    fn finish_align(&mut self) {
        if self.bit_count > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }
}

// ============================================================================
// Canonical Huffman Decoder (RFC 1951 §3.2.2)
// ============================================================================

struct CanonicalHuffman {
    counts: [u16; MAX_BITS + 1],
    symbols: [u16; MAX_LIT_CODES],
    max_len: usize,
    total_symbols: usize,
}

impl CanonicalHuffman {
    fn from_lengths(
        lengths: &[u8],
        allow_empty_or_incomplete_single: bool,
        table_name: &'static str,
    ) -> Result<Self, ZlibError> {
        let mut counts = [0u16; MAX_BITS + 1];
        let mut max_len = 0usize;
        let mut total_symbols = 0usize;

        for &len in lengths {
            let l = usize::from(len);
            if l > MAX_BITS {
                return Err(ZlibError::InvalidHuffmanTable(table_name));
            }
            if l > 0 {
                counts[l] += 1;
                total_symbols += 1;
                if l > max_len {
                    max_len = l;
                }
            }
        }

        if total_symbols == 0 {
            if allow_empty_or_incomplete_single {
                return Ok(Self {
                    counts,
                    symbols: [0u16; MAX_LIT_CODES],
                    max_len: 0,
                    total_symbols: 0,
                });
            }
            return Err(ZlibError::InvalidHuffmanTable(table_name));
        }

        // Verify Kraft inequality across bit lengths 1..=MAX_BITS.
        let mut left: i32 = 1;
        for &cnt in &counts[1..=MAX_BITS] {
            left = (left << 1) - i32::from(cnt);
            if left < 0 {
                return Err(ZlibError::InvalidHuffmanTable(table_name));
            }
        }

        // Multi-symbol trees must be complete (`left == 0`). Single-symbol trees
        // (`total_symbols == 1`) are legitimately incomplete in zlib/Deflate.
        if left != 0 && !(allow_empty_or_incomplete_single && total_symbols == 1) {
            return Err(ZlibError::InvalidHuffmanTable(table_name));
        }

        let mut offsets = [0usize; MAX_BITS + 1];
        let mut accum = 0usize;
        for bits in 1..=max_len {
            offsets[bits] = accum;
            accum += usize::from(counts[bits]);
        }

        let mut symbols = [0u16; MAX_LIT_CODES];
        for (sym, &len) in lengths.iter().enumerate() {
            let l = usize::from(len);
            if l > 0 {
                symbols[offsets[l]] = sym as u16;
                offsets[l] += 1;
            }
        }

        Ok(Self {
            counts,
            symbols,
            max_len,
            total_symbols,
        })
    }

    fn fixed_literal_tree() -> Result<Self, ZlibError> {
        let mut lengths = [0u8; MAX_LIT_CODES];
        lengths[0..=143].fill(8);
        lengths[144..=255].fill(9);
        lengths[256..=279].fill(7);
        lengths[280..=287].fill(8);
        Self::from_lengths(&lengths, false, "fixed literal/length")
    }

    fn fixed_distance_tree() -> Result<Self, ZlibError> {
        let lengths = [5u8; MAX_DIST_CODES];
        Self::from_lengths(&lengths, false, "fixed distance")
    }

    #[inline]
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, ZlibError> {
        if self.total_symbols == 0 {
            return Err(ZlibError::InvalidHuffmanCode);
        }
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: usize = 0;

        for len in 1..=self.max_len {
            code |= reader.read_bit()? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                let pos = index + (code - first) as usize;
                return Ok(self.symbols[pos]);
            }
            index += count as usize;
            first = (first + count) << 1;
            code <<= 1;
        }

        Err(ZlibError::InvalidHuffmanCode)
    }
}

// ============================================================================
// RFC 1951 Inflate Engine
// ============================================================================

fn inflate_stream(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    max_output_size: usize,
) -> Result<(), ZlibError> {
    loop {
        let bfinal = reader.read_bits(1)?;
        let btype = reader.read_bits(2)?;

        match btype {
            0 => inflate_stored_block(reader, output, max_output_size)?,
            1 => {
                let lit_tree = CanonicalHuffman::fixed_literal_tree()?;
                let dist_tree = CanonicalHuffman::fixed_distance_tree()?;
                inflate_huffman_block(reader, output, max_output_size, &lit_tree, &dist_tree)?;
            }
            2 => {
                let (lit_tree, dist_tree) = decode_dynamic_trees(reader)?;
                inflate_huffman_block(reader, output, max_output_size, &lit_tree, &dist_tree)?;
            }
            _ => return Err(ZlibError::InvalidBlockType),
        }

        if bfinal == 1 {
            break;
        }
    }
    Ok(())
}

fn inflate_stored_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    max_output_size: usize,
) -> Result<(), ZlibError> {
    reader.align_to_byte();
    let hdr = reader.read_aligned_bytes(4)?;
    let len = u16::from_le_bytes([hdr[0], hdr[1]]);
    let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
    if len != !nlen {
        return Err(ZlibError::StoredLengthMismatch { len, nlen });
    }

    let block_len = usize::from(len);
    if block_len > max_output_size || output.len() > max_output_size - block_len {
        return Err(ZlibError::OutputLimitExceeded { max_output_size });
    }

    let raw = reader.read_aligned_bytes(block_len)?;
    output.extend_from_slice(raw);
    Ok(())
}

fn decode_dynamic_trees(
    reader: &mut BitReader<'_>,
) -> Result<(CanonicalHuffman, CanonicalHuffman), ZlibError> {
    let hlit = reader.read_bits(5)? as usize + 257;
    let hdist = reader.read_bits(5)? as usize + 1;
    let hclen = reader.read_bits(4)? as usize + 4;

    if hlit > 286 || hdist > 30 {
        return Err(ZlibError::InvalidDynamicHeader { hlit, hdist, hclen });
    }

    let mut code_lengths = [0u8; 19];
    for i in 0..hclen {
        code_lengths[CODE_LENGTH_ORDER[i]] = reader.read_bits(3)? as u8;
    }

    let cl_tree = CanonicalHuffman::from_lengths(&code_lengths, true, "code-length alphabet")?;

    let total_codes = hlit + hdist;
    let mut combined = [0u8; 286 + 30];
    let mut idx = 0usize;

    while idx < total_codes {
        let sym = cl_tree.decode(reader)?;
        match sym {
            0..=15 => {
                combined[idx] = sym as u8;
                idx += 1;
            }
            16 => {
                if idx == 0 {
                    return Err(ZlibError::InvalidCodeLengthRepeat);
                }
                let repeat = reader.read_bits(2)? as usize + 3;
                if idx + repeat > total_codes {
                    return Err(ZlibError::InvalidCodeLengthRepeat);
                }
                let prev = combined[idx - 1];
                for dst in &mut combined[idx..idx + repeat] {
                    *dst = prev;
                }
                idx += repeat;
            }
            17 => {
                let repeat = reader.read_bits(3)? as usize + 3;
                if idx + repeat > total_codes {
                    return Err(ZlibError::InvalidCodeLengthRepeat);
                }
                for dst in &mut combined[idx..idx + repeat] {
                    *dst = 0;
                }
                idx += repeat;
            }
            18 => {
                let repeat = reader.read_bits(7)? as usize + 11;
                if idx + repeat > total_codes {
                    return Err(ZlibError::InvalidCodeLengthRepeat);
                }
                for dst in &mut combined[idx..idx + repeat] {
                    *dst = 0;
                }
                idx += repeat;
            }
            _ => return Err(ZlibError::InvalidCodeLengthRepeat),
        }
    }

    // End-of-block symbol 256 MUST be present in every dynamic literal/length tree.
    if combined[256] == 0 {
        return Err(ZlibError::InvalidHuffmanTable("missing EOB 256 in dynamic tree"));
    }

    let lit_tree =
        CanonicalHuffman::from_lengths(&combined[..hlit], true, "dynamic literal/length")?;
    let dist_tree =
        CanonicalHuffman::from_lengths(&combined[hlit..total_codes], true, "dynamic distance")?;

    Ok((lit_tree, dist_tree))
}

fn inflate_huffman_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    max_output_size: usize,
    lit_tree: &CanonicalHuffman,
    dist_tree: &CanonicalHuffman,
) -> Result<(), ZlibError> {
    loop {
        let sym = lit_tree.decode(reader)?;
        if sym < 256 {
            if output.len() >= max_output_size {
                return Err(ZlibError::OutputLimitExceeded { max_output_size });
            }
            output.push(sym as u8);
        } else if sym == 256 {
            break;
        } else if sym <= 285 {
            let len_idx = (sym - 257) as usize;
            let extra_len_bits = LENGTH_EXTRA[len_idx];
            let match_len =
                usize::from(LENGTH_BASE[len_idx]) + reader.read_bits(extra_len_bits)? as usize;

            if match_len > max_output_size || output.len() > max_output_size - match_len {
                return Err(ZlibError::OutputLimitExceeded { max_output_size });
            }

            let dist_sym = dist_tree.decode(reader)? as usize;
            if dist_sym >= 30 {
                return Err(ZlibError::InvalidHuffmanCode);
            }
            let extra_dist_bits = DIST_EXTRA[dist_sym];
            let distance =
                usize::from(DIST_BASE[dist_sym]) + reader.read_bits(extra_dist_bits)? as usize;

            if distance == 0 || distance > output.len() {
                return Err(ZlibError::InvalidBackReference {
                    distance,
                    output_len: output.len(),
                });
            }

            output.reserve(match_len);
            for _ in 0..match_len {
                let b = output[output.len() - distance];
                output.push(b);
            }
        } else {
            return Err(ZlibError::InvalidHuffmanCode);
        }
    }
    Ok(())
}

// ============================================================================
// RFC 1951 Deflate Compressors (Stored & Fixed Huffman + LZ77)
// ============================================================================

fn deflate_stored(input: &[u8], out: &mut Vec<u8>) {
    if input.is_empty() {
        out.push(0x01); // BFINAL=1, BTYPE=00
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
        return;
    }

    let mut offset = 0usize;
    while offset < input.len() {
        let rem = input.len() - offset;
        let chunk_len = rem.min(65535);
        let is_final = offset + chunk_len == input.len();
        out.push(if is_final { 0x01 } else { 0x00 });
        let len_u16 = chunk_len as u16;
        let nlen_u16 = !len_u16;
        out.extend_from_slice(&len_u16.to_le_bytes());
        out.extend_from_slice(&nlen_u16.to_le_bytes());
        out.extend_from_slice(&input[offset..offset + chunk_len]);
        offset += chunk_len;
    }
}

#[inline]
fn write_fixed_literal(writer: &mut BitWriter<'_>, lit: u8) {
    let sym = u16::from(lit);
    if sym <= 143 {
        writer.write_huffman_msb(0x30 + sym, 8);
    } else {
        writer.write_huffman_msb(0x190 + (sym - 144), 9);
    }
}

#[inline]
fn write_fixed_symbol(writer: &mut BitWriter<'_>, sym: u16) {
    if sym <= 143 {
        writer.write_huffman_msb(0x30 + sym, 8);
    } else if sym <= 255 {
        writer.write_huffman_msb(0x190 + (sym - 144), 9);
    } else if sym <= 279 {
        writer.write_huffman_msb(sym - 256, 7);
    } else {
        writer.write_huffman_msb(0xC0 + (sym - 280), 8);
    }
}

#[inline]
fn write_fixed_length_distance(writer: &mut BitWriter<'_>, length: usize, distance: usize) {
    debug_assert!((3..=258).contains(&length));
    debug_assert!((1..=32768).contains(&distance));

    let mut len_idx = 0usize;
    for (i, &base) in LENGTH_BASE.iter().enumerate().rev() {
        if length >= usize::from(base) {
            len_idx = i;
            break;
        }
    }
    write_fixed_symbol(writer, (257 + len_idx) as u16);
    let len_extra = LENGTH_EXTRA[len_idx];
    if len_extra > 0 {
        writer.write_bits((length - usize::from(LENGTH_BASE[len_idx])) as u32, len_extra);
    }

    let mut dist_idx = 0usize;
    for (i, &base) in DIST_BASE.iter().enumerate().rev() {
        if distance >= usize::from(base) {
            dist_idx = i;
            break;
        }
    }
    writer.write_huffman_msb(dist_idx as u16, 5);
    let dist_extra = DIST_EXTRA[dist_idx];
    if dist_extra > 0 {
        writer.write_bits((distance - usize::from(DIST_BASE[dist_idx])) as u32, dist_extra);
    }
}

fn deflate_fixed_literals(input: &[u8], out: &mut Vec<u8>) {
    let mut writer = BitWriter::new(out);
    writer.write_bits(1, 1); // BFINAL = 1
    writer.write_bits(1, 2); // BTYPE = 01 (Fixed Huffman)
    for &b in input {
        write_fixed_literal(&mut writer, b);
    }
    write_fixed_symbol(&mut writer, 256); // EOB
    writer.finish_align();
}

fn deflate_fixed_lz77(input: &[u8], out: &mut Vec<u8>) {
    const WINDOW_SIZE: usize = 32768;
    const HASH_BITS: usize = 14;
    const HASH_SIZE: usize = 1 << HASH_BITS;
    const MAX_CHAIN_STEPS: usize = 32;

    let mut writer = BitWriter::new(out);
    writer.write_bits(1, 1); // BFINAL = 1
    writer.write_bits(1, 2); // BTYPE = 01 (Fixed Huffman)

    if input.len() < 3 {
        for &b in input {
            write_fixed_literal(&mut writer, b);
        }
        write_fixed_symbol(&mut writer, 256);
        writer.finish_align();
        return;
    }

    let mut head = vec![usize::MAX; HASH_SIZE];
    let mut prev = vec![usize::MAX; WINDOW_SIZE];

    let hash3 = |slice: &[u8]| -> usize {
        let v = (u32::from(slice[0]) << 16) | (u32::from(slice[1]) << 8) | u32::from(slice[2]);
        ((v.wrapping_mul(0x1E35_A7BD)) >> (32 - HASH_BITS)) as usize
    };

    let mut pos = 0usize;
    while pos < input.len() {
        if pos + 3 > input.len() {
            write_fixed_literal(&mut writer, input[pos]);
            pos += 1;
            continue;
        }

        let h = hash3(&input[pos..pos + 3]);
        let mut cand = head[h];
        prev[pos & (WINDOW_SIZE - 1)] = cand;
        head[h] = pos;

        let max_len = (input.len() - pos).min(258);
        let min_cand = pos.saturating_sub(WINDOW_SIZE);
        let mut best_len = 2usize;
        let mut best_dist = 0usize;
        let mut steps = 0usize;

        while cand != usize::MAX && cand >= min_cand && steps < MAX_CHAIN_STEPS {
            steps += 1;
            if input[cand + best_len.min(max_len - 1)] == input[pos + best_len.min(max_len - 1)]
                && input[cand..cand + 3] == input[pos..pos + 3]
            {
                let mut mlen = 3usize;
                while mlen < max_len && input[cand + mlen] == input[pos + mlen] {
                    mlen += 1;
                }
                if mlen > best_len {
                    best_len = mlen;
                    best_dist = pos - cand;
                    if best_len == max_len {
                        break;
                    }
                }
            }
            let next_cand = prev[cand & (WINDOW_SIZE - 1)];
            if next_cand >= cand {
                break;
            }
            cand = next_cand;
        }

        if best_len >= 3 {
            write_fixed_length_distance(&mut writer, best_len, best_dist);
            for step in 1..best_len {
                let p = pos + step;
                if p + 3 <= input.len() {
                    let sh = hash3(&input[p..p + 3]);
                    prev[p & (WINDOW_SIZE - 1)] = head[sh];
                    head[sh] = p;
                }
            }
            pos += best_len;
        } else {
            write_fixed_literal(&mut writer, input[pos]);
            pos += 1;
        }
    }

    write_fixed_symbol(&mut writer, 256); // EOB
    writer.finish_align();
}
