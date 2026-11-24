#![forbid(unsafe_code)]

//! Pure-Rust, zero-dependency RFC 1950 (zlib + Adler-32) and RFC 1951 (DEFLATE)
//! bounded decompressor and LZ77 + Fixed-Huffman compressor.
//!
//! Designed specifically for hostile QA testing of `LCPacket::zlibCompress` / `zlibDecompress`
//! and `GameUtil:unzipData`:
//! - Full inflate support for `BTYPE=00` (Stored), `BTYPE=01` (Fixed Huffman), and
//!   `BTYPE=10` (Dynamic Huffman), plus strict rejection of `BTYPE=11`, invalid zlib headers,
//!   preset dictionaries (`FDICT=1`), oversubscribed Huffman trees, out-of-window distances,
//!   truncated bitstreams, corrupted Adler-32 trailers, and **zlib zip-bombs** exceeding `max_output`.

use std::fmt;

const ADLER_MOD: u32 = 65521;

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
    35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
    3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193,
    257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6,
    7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZlibError {
    TruncatedHeader,
    InvalidCompressionMethod(u8),
    InvalidWindowSize(u8),
    HeaderChecksumFailed,
    PresetDictUnsupported,
    UnexpectedEof,
    ReservedBlockType,
    StoredLengthComplementMismatch,
    InvalidHuffmanTree,
    InvalidSymbol,
    InvalidDistanceCode(u16),
    DistanceBeforeStart { dist: usize, available: usize },
    OutputLimitExceeded { limit: usize },
    Adler32Mismatch { expected: u32, actual: u32 },
}

impl fmt::Display for ZlibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader => write!(f, "zlib stream shorter than 6-byte header+trailer"),
            Self::InvalidCompressionMethod(cm) => write!(f, "invalid zlib CM={} (expected 8)", cm),
            Self::InvalidWindowSize(ci) => write!(f, "invalid zlib CINFO={} (expected <= 7)", ci),
            Self::HeaderChecksumFailed => write!(f, "zlib CMF/FLG FCHECK modulo-31 failed"),
            Self::PresetDictUnsupported => write!(f, "zlib FDICT preset dictionary unsupported"),
            Self::UnexpectedEof => write!(f, "unexpected EOF in DEFLATE bitstream"),
            Self::ReservedBlockType => write!(f, "reserved DEFLATE block type BTYPE=11"),
            Self::StoredLengthComplementMismatch => write!(f, "stored block LEN != !NLEN"),
            Self::InvalidHuffmanTree => write!(f, "invalid or oversubscribed Huffman code lengths"),
            Self::InvalidSymbol => write!(f, "unassigned Huffman code encountered"),
            Self::InvalidDistanceCode(code) => write!(f, "reserved DEFLATE distance code {}", code),
            Self::DistanceBeforeStart { dist, available } => write!(
                f,
                "LZ77 back-reference distance {} exceeds output length {}",
                dist, available
            ),
            Self::OutputLimitExceeded { limit } => {
                write!(f, "decompressed output exceeds limit {} bytes", limit)
            }
            Self::Adler32Mismatch { expected, actual } => write!(
                f,
                "zlib Adler-32 mismatch: expected 0x{:08x}, computed 0x{:08x}",
                expected, actual
            ),
        }
    }
}

/// Computes RFC 1950 Adler-32 checksum.
pub fn adler32(data: &[u8]) -> u32 {
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for chunk in data.chunks(5552) {
        for &b in chunk {
            s1 += b as u32;
            s2 += s1;
        }
        s1 %= ADLER_MOD;
        s2 %= ADLER_MOD;
    }
    (s2 << 16) | s1
}

// ============================================================================
// Canonical Huffman Decoder (LSB-first DEFLATE bit ordering)
// ============================================================================

struct HuffmanTable {
    max_bits: u8,
    counts: [u16; 16],
    symbols: Vec<u16>,
    offsets: [u16; 16],
}

impl HuffmanTable {
    fn from_lengths(lengths: &[u8]) -> Result<Self, ZlibError> {
        let mut counts = [0u16; 16];
        let mut max_bits = 0u8;
        for &len in lengths {
            if len > 15 {
                return Err(ZlibError::InvalidHuffmanTree);
            }
            if len > 0 {
                counts[len as usize] += 1;
                if len > max_bits {
                    max_bits = len;
                }
            }
        }

        // Check Kraft inequality so oversubscribed trees never misdecode.
        if max_bits > 0 {
            let mut left: i32 = 1;
            for &cnt in counts.iter().skip(1) {
                left <<= 1;
                left -= i32::from(cnt);
                if left < 0 {
                    return Err(ZlibError::InvalidHuffmanTree);
                }
            }
        }

        let mut offsets = [0u16; 16];
        let mut total = 0u16;
        for bits in 1..=15 {
            offsets[bits] = total;
            total += counts[bits];
        }

        let mut next_slot = offsets;
        let mut symbols = vec![0u16; total as usize];
        for (sym, &len) in lengths.iter().enumerate() {
            if len > 0 {
                let slot = next_slot[len as usize] as usize;
                symbols[slot] = sym as u16;
                next_slot[len as usize] += 1;
            }
        }

        Ok(Self {
            max_bits,
            counts,
            symbols,
            offsets,
        })
    }

    fn fixed_lit_len() -> Self {
        let mut lengths = [0u8; 288];
        lengths[0..144].fill(8);
        lengths[144..256].fill(9);
        lengths[256..280].fill(7);
        lengths[280..288].fill(8);
        Self::from_lengths(&lengths).expect("valid RFC 1951 fixed lit/len table")
    }

    fn fixed_dist() -> Self {
        let lengths = [5u8; 32];
        Self::from_lengths(&lengths).expect("valid RFC 1951 fixed dist table")
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buf: u64,
    bit_count: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn fill(&mut self, need: u8) -> Result<(), ZlibError> {
        while self.bit_count < need {
            if self.pos >= self.data.len() {
                return Err(ZlibError::UnexpectedEof);
            }
            self.bit_buf |= (self.data[self.pos] as u64) << self.bit_count;
            self.pos += 1;
            self.bit_count += 8;
        }
        Ok(())
    }

    fn read_bits(&mut self, count: u8) -> Result<u32, ZlibError> {
        if count == 0 {
            return Ok(0);
        }
        self.fill(count)?;
        let mask = (1u64 << count) - 1;
        let val = (self.bit_buf & mask) as u32;
        self.bit_buf >>= count;
        self.bit_count -= count;
        Ok(val)
    }

    fn align_to_byte(&mut self) {
        let drop_bits = self.bit_count & 7;
        self.bit_buf >>= drop_bits;
        self.bit_count -= drop_bits;
    }

    fn read_aligned_bytes(&mut self, len: usize) -> Result<&'a [u8], ZlibError> {
        self.align_to_byte();
        let buffered_bytes = (self.bit_count / 8) as usize;
        // Rewind buffered whole bytes into slice cursor for zero-copy slice read
        self.pos -= buffered_bytes;
        self.bit_buf = 0;
        self.bit_count = 0;

        if self.pos + len > self.data.len() {
            return Err(ZlibError::UnexpectedEof);
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn decode_huffman(&mut self, table: &HuffmanTable) -> Result<u16, ZlibError> {
        if table.max_bits == 0 {
            return Err(ZlibError::InvalidSymbol);
        }
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        for len in 1..=(table.max_bits as usize) {
            let bit = self.read_bits(1)? as i32;
            code |= bit;
            let count = table.counts[len] as i32;
            if code - count < first {
                let idx = (table.offsets[len] as i32 + (code - first)) as usize;
                return Ok(table.symbols[idx]);
            }
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(ZlibError::InvalidSymbol)
    }
}

/// Decompresses an RFC 1950 zlib stream with strict `max_output` zip-bomb enforcement.
pub fn zlib_decompress_bounded(input: &[u8], max_output: usize) -> Result<Vec<u8>, ZlibError> {
    if input.len() < 6 {
        return Err(ZlibError::TruncatedHeader);
    }
    let cmf = input[0];
    let flg = input[1];
    let cm = cmf & 0x0F;
    let cinfo = cmf >> 4;
    if cm != 8 {
        return Err(ZlibError::InvalidCompressionMethod(cm));
    }
    if cinfo > 7 {
        return Err(ZlibError::InvalidWindowSize(cinfo));
    }
    if (((cmf as u16) << 8) | (flg as u16)) % 31 != 0 {
        return Err(ZlibError::HeaderChecksumFailed);
    }
    if (flg & 0x20) != 0 {
        return Err(ZlibError::PresetDictUnsupported);
    }

    let mut reader = BitReader::new(input, 2);
    // Cap initial capacity to avoid pre-allocation bombs while staying fast for normal RPCs.
    let mut out = Vec::with_capacity(input.len().saturating_mul(2).min(max_output).min(65536));

    loop {
        let bfinal = reader.read_bits(1)?;
        let btype = reader.read_bits(2)?;
        match btype {
            0 => {
                let hdr = reader.read_aligned_bytes(4)?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]);
                let nlen = u16::from_le_bytes([hdr[2], hdr[3]]);
                if len != !nlen {
                    return Err(ZlibError::StoredLengthComplementMismatch);
                }
                let block_len = len as usize;
                if out.len().saturating_add(block_len) > max_output {
                    return Err(ZlibError::OutputLimitExceeded { limit: max_output });
                }
                let raw = reader.read_aligned_bytes(block_len)?;
                out.extend_from_slice(raw);
            }
            1 => {
                let lit_table = HuffmanTable::fixed_lit_len();
                let dist_table = HuffmanTable::fixed_dist();
                inflate_huffman_block(&mut reader, &lit_table, &dist_table, &mut out, max_output)?;
            }
            2 => {
                let hlit = reader.read_bits(5)? as usize + 257;
                let hdist = reader.read_bits(5)? as usize + 1;
                let hclen = reader.read_bits(4)? as usize + 4;
                if hlit > 286 || hdist > 30 {
                    return Err(ZlibError::InvalidHuffmanTree);
                }

                let mut cl_lengths = [0u8; 19];
                for i in 0..hclen {
                    cl_lengths[CL_ORDER[i]] = reader.read_bits(3)? as u8;
                }
                let cl_table = HuffmanTable::from_lengths(&cl_lengths)?;

                let total_codes = hlit + hdist;
                let mut lengths = Vec::with_capacity(total_codes);
                while lengths.len() < total_codes {
                    let sym = reader.decode_huffman(&cl_table)?;
                    match sym {
                        0..=15 => lengths.push(sym as u8),
                        16 => {
                            let prev = *lengths.last().ok_or(ZlibError::InvalidHuffmanTree)?;
                            let repeat = reader.read_bits(2)? as usize + 3;
                            if lengths.len() + repeat > total_codes {
                                return Err(ZlibError::InvalidHuffmanTree);
                            }
                            lengths.resize(lengths.len() + repeat, prev);
                        }
                        17 => {
                            let repeat = reader.read_bits(3)? as usize + 3;
                            if lengths.len() + repeat > total_codes {
                                return Err(ZlibError::InvalidHuffmanTree);
                            }
                            lengths.resize(lengths.len() + repeat, 0);
                        }
                        18 => {
                            let repeat = reader.read_bits(7)? as usize + 11;
                            if lengths.len() + repeat > total_codes {
                                return Err(ZlibError::InvalidHuffmanTree);
                            }
                            lengths.resize(lengths.len() + repeat, 0);
                        }
                        _ => return Err(ZlibError::InvalidHuffmanTree),
                    }
                }

                // End-of-block symbol 256 must have non-zero code length.
                if lengths[256] == 0 {
                    return Err(ZlibError::InvalidHuffmanTree);
                }
                let lit_table = HuffmanTable::from_lengths(&lengths[..hlit])?;
                let dist_table = HuffmanTable::from_lengths(&lengths[hlit..])?;
                inflate_huffman_block(&mut reader, &lit_table, &dist_table, &mut out, max_output)?;
            }
            _ => return Err(ZlibError::ReservedBlockType),
        }

        if bfinal == 1 {
            break;
        }
    }

    let trailer = reader.read_aligned_bytes(4)?;
    let expected_adler = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let actual_adler = adler32(&out);
    if expected_adler != actual_adler {
        return Err(ZlibError::Adler32Mismatch {
            expected: expected_adler,
            actual: actual_adler,
        });
    }

    Ok(out)
}

fn inflate_huffman_block(
    reader: &mut BitReader<'_>,
    lit_table: &HuffmanTable,
    dist_table: &HuffmanTable,
    out: &mut Vec<u8>,
    max_output: usize,
) -> Result<(), ZlibError> {
    loop {
        let sym = reader.decode_huffman(lit_table)?;
        if sym < 256 {
            if out.len() >= max_output {
                return Err(ZlibError::OutputLimitExceeded { limit: max_output });
            }
            out.push(sym as u8);
        } else if sym == 256 {
            break;
        } else if sym <= 285 {
            let len_idx = (sym - 257) as usize;
            let extra_bits = LENGTH_EXTRA[len_idx];
            let match_len = (LENGTH_BASE[len_idx] as usize) + (reader.read_bits(extra_bits)? as usize);

            let dist_sym = reader.decode_huffman(dist_table)?;
            if dist_sym >= 30 {
                return Err(ZlibError::InvalidDistanceCode(dist_sym));
            }
            let d_idx = dist_sym as usize;
            let dist = (DIST_BASE[d_idx] as usize) + (reader.read_bits(DIST_EXTRA[d_idx])? as usize);
            if dist == 0 || dist > out.len() {
                return Err(ZlibError::DistanceBeforeStart {
                    dist,
                    available: out.len(),
                });
            }
            if out.len().saturating_add(match_len) > max_output {
                return Err(ZlibError::OutputLimitExceeded { limit: max_output });
            }

            let start = out.len() - dist;
            if dist >= match_len {
                out.extend_from_within(start..start + match_len);
            } else {
                for i in 0..match_len {
                    let b = out[start + i];
                    out.push(b);
                }
            }
        } else {
            return Err(ZlibError::InvalidSymbol);
        }
    }
    Ok(())
}

// ============================================================================
// Pure-Rust RFC 1950 Zlib Compressor (LZ77 + Fixed Huffman `BTYPE=01`)
// ============================================================================

struct BitWriter {
    out: Vec<u8>,
    bit_buf: u64,
    bit_count: u8,
}

impl BitWriter {
    fn with_capacity(cap: usize) -> Self {
        Self {
            out: Vec::with_capacity(cap),
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn write_bits(&mut self, val: u32, count: u8) {
        if count == 0 {
            return;
        }
        self.bit_buf |= ((val as u64) & ((1u64 << count) - 1)) << self.bit_count;
        self.bit_count += count;
        while self.bit_count >= 8 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
    }

    /// Writes an MSB-first Huffman codeword in DEFLATE's reversed bit order.
    fn write_huffman_msb(&mut self, code: u16, bits: u8) {
        let rev = code.reverse_bits() >> (16 - bits);
        self.write_bits(rev as u32, bits);
    }

    fn flush_align(&mut self) {
        if self.bit_count > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }
}

fn write_fixed_sym(bw: &mut BitWriter, sym: u16) {
    match sym {
        0..=143 => bw.write_huffman_msb(0x30 + sym, 8),
        144..=255 => bw.write_huffman_msb(0x190 + (sym - 144), 9),
        256..=279 => bw.write_huffman_msb(sym - 256, 7),
        280..=287 => bw.write_huffman_msb(0xC0 + (sym - 280), 8),
        _ => unreachable!(),
    }
}

fn encode_length(bw: &mut BitWriter, match_len: usize) {
    let mut idx = 0usize;
    for i in (0..LENGTH_BASE.len()).rev() {
        if match_len >= LENGTH_BASE[i] as usize {
            idx = i;
            break;
        }
    }
    write_fixed_sym(bw, 257 + idx as u16);
    let extra = LENGTH_EXTRA[idx];
    if extra > 0 {
        let diff = (match_len - LENGTH_BASE[idx] as usize) as u32;
        bw.write_bits(diff, extra);
    }
}

fn encode_distance(bw: &mut BitWriter, dist: usize) {
    let mut idx = 0usize;
    for i in (0..DIST_BASE.len()).rev() {
        if dist >= DIST_BASE[i] as usize {
            idx = i;
            break;
        }
    }
    bw.write_huffman_msb(idx as u16, 5);
    let extra = DIST_EXTRA[idx];
    if extra > 0 {
        let diff = (dist - DIST_BASE[idx] as usize) as u32;
        bw.write_bits(diff, extra);
    }
}

/// Compresses `input` into a standard RFC 1950 zlib stream (`0x78 0x9C` + LZ77 Fixed-Huffman
/// DEFLATE + Big-Endian Adler-32), 100% compatible with C `zlib` (`LCPacket::zlibDecompress`
/// and `GameUtil:unzipData`).
pub fn zlib_compress(input: &[u8]) -> Vec<u8> {
    let mut bw = BitWriter::with_capacity(input.len() + 16);
    // Standard RFC 1950 header: CM=8, CINFO=7 (32K window), FLEVEL=2, FDICT=0, FCHECK=28 -> 0x78, 0x9C
    bw.out.push(0x78);
    bw.out.push(0x9C);

    // Single final Fixed-Huffman DEFLATE block: BFINAL=1, BTYPE=01
    bw.write_bits(1, 1);
    bw.write_bits(1, 2);

    // Hash-chained LZ77 matcher (32 KiB sliding window, max match 258)
    const HASH_BITS: usize = 13;
    const HASH_SIZE: usize = 1 << HASH_BITS;
    let mut head = vec![usize::MAX; HASH_SIZE];
    let mut prev = vec![usize::MAX; input.len().clamp(1, 32768)];
    let prev_mask = if prev.len().is_power_of_two() {
        prev.len() - 1
    } else {
        usize::MAX
    };

    let hash3 = |b0: u8, b1: u8, b2: u8| -> usize {
        ((((b0 as usize) << 10) ^ ((b1 as usize) << 5) ^ (b2 as usize)).wrapping_mul(2654435761))
            & (HASH_SIZE - 1)
    };

    let mut pos = 0usize;
    while pos < input.len() {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;

        if pos + 3 <= input.len() {
            let h = hash3(input[pos], input[pos + 1], input[pos + 2]);
            let mut cand = head[h];
            let slot = if prev_mask != usize::MAX {
                pos & prev_mask
            } else {
                pos % prev.len()
            };
            prev[slot] = cand;
            head[h] = pos;

            let max_len = (input.len() - pos).min(258);
            let mut steps = 0usize;
            while cand != usize::MAX && steps < 24 {
                let dist = pos - cand;
                if dist == 0 || dist > 32768 {
                    break;
                }
                if input[cand + best_len.min(max_len - 1)]
                    == input[pos + best_len.min(max_len - 1)]
                    && input[cand] == input[pos]
                    && input[cand + 1] == input[pos + 1]
                    && input[cand + 2] == input[pos + 2]
                {
                    let mut mlen = 3usize;
                    while mlen < max_len && input[cand + mlen] == input[pos + mlen] {
                        mlen += 1;
                    }
                    if mlen > best_len {
                        best_len = mlen;
                        best_dist = dist;
                        if best_len == 258 {
                            break;
                        }
                    }
                }
                let cslot = if prev_mask != usize::MAX {
                    cand & prev_mask
                } else {
                    cand % prev.len()
                };
                let next_cand = prev[cslot];
                if next_cand >= cand {
                    break;
                }
                cand = next_cand;
                steps += 1;
            }
        }

        if best_len >= 3 {
            encode_length(&mut bw, best_len);
            encode_distance(&mut bw, best_dist);
            for step in 1..best_len {
                let p = pos + step;
                if p + 3 <= input.len() {
                    let h = hash3(input[p], input[p + 1], input[p + 2]);
                    let slot = if prev_mask != usize::MAX {
                        p & prev_mask
                    } else {
                        p % prev.len()
                    };
                    prev[slot] = head[h];
                    head[h] = p;
                }
            }
            pos += best_len;
        } else {
            write_fixed_sym(&mut bw, input[pos] as u16);
            pos += 1;
        }
    }

    // End-of-block symbol 256
    write_fixed_sym(&mut bw, 256);
    bw.flush_align();

    let checksum = adler32(input);
    bw.out.extend_from_slice(&checksum.to_be_bytes());
    bw.out
}

/// Builds a compact RFC 1950 zlib zip-bomb that expands to `target_zeros` zero bytes
/// using minimum DEFLATE stored/RLE blocks — used exclusively by our adversarial QA suite
/// to prove `ServerRxStreamDecoder` and `zlib_decompress_bounded` reject decompression bombs
/// in O(limit) memory!
pub fn craft_zlib_zero_bomb(target_zeros: usize) -> Vec<u8> {
    if target_zeros == 0 {
        return zlib_compress(&[]);
    }
    let mut bw = BitWriter::with_capacity(target_zeros / 128 + 32);
    bw.out.push(0x78);
    bw.out.push(0x9C);
    bw.write_bits(1, 1); // BFINAL=1
    bw.write_bits(1, 2); // BTYPE=01 (Fixed Huffman)

    // Emit literal 0x00, then repeat distance=1, length=258 (`sym=285`, 0 extra bits; `dist=0`, 0 extra bits)!
    // Each 258-byte zero run takes ONLY 8 + 5 = 13 bits (~158x compression ratio in a single pass)!
    write_fixed_sym(&mut bw, 0);
    let mut rem = target_zeros - 1;
    while rem >= 258 {
        write_fixed_sym(&mut bw, 285); // length 258
        bw.write_huffman_msb(0, 5);    // distance 1
        rem -= 258;
    }
    if rem >= 3 {
        encode_length(&mut bw, rem);
        encode_distance(&mut bw, 1);
    } else {
        for _ in 0..rem {
            write_fixed_sym(&mut bw, 0);
        }
    }
    write_fixed_sym(&mut bw, 256);
    bw.flush_align();

    // Adler-32 of `N` zero bytes: s1 = 1, s2 = N % 65521!
    let s1: u32 = 1;
    let s2: u32 = ((target_zeros as u64) % (ADLER_MOD as u64)) as u32;
    let checksum = (s2 << 16) | s1;
    bw.out.extend_from_slice(&checksum.to_be_bytes());
    bw.out
}
