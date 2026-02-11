use memchr::memchr_iter;
use rayon::prelude::*;

/// Minimum data size to use parallel processing (2MB).
/// Lower threshold lets us exploit 4 cores on smaller files.
const PARALLEL_THRESHOLD: usize = 2 * 1024 * 1024;

/// Results from counting a byte slice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WcCounts {
    pub lines: u64,
    pub words: u64,
    pub bytes: u64,
    pub chars: u64,
    pub max_line_length: u64,
}

/// Whitespace lookup table for branchless word boundary detection.
/// GNU wc uses C locale `isspace()`: space, tab, newline, CR, form feed, vertical tab.
const fn make_ws_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    t[0x09] = 1; // \t  horizontal tab
    t[0x0A] = 1; // \n  newline
    t[0x0B] = 1; // \v  vertical tab
    t[0x0C] = 1; // \f  form feed
    t[0x0D] = 1; // \r  carriage return
    t[0x20] = 1; //     space
    t
}

/// Precomputed whitespace lookup: `WS_TABLE[byte] == 1` if whitespace, `0` otherwise.
const WS_TABLE: [u8; 256] = make_ws_table();

/// Printable ASCII lookup table: 0x20 (space) through 0x7E (~) are printable.
const fn make_printable_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0x20u16;
    while i <= 0x7E {
        t[i as usize] = 1;
        i += 1;
    }
    t
}

const PRINTABLE_TABLE: [u8; 256] = make_printable_table();

/// Count newlines using SIMD-accelerated memchr.
/// GNU wc counts newline bytes (`\n`), not logical lines.
#[inline]
pub fn count_lines(data: &[u8]) -> u64 {
    memchr_iter(b'\n', data).count() as u64
}

/// Count bytes. Trivial but included for API consistency.
#[inline]
pub fn count_bytes(data: &[u8]) -> u64 {
    data.len() as u64
}

/// Count words using SIMD-accelerated whitespace detection + popcount.
///
/// A word is a maximal run of non-whitespace bytes (GNU wc definition).
/// Word starts are transitions from whitespace to non-whitespace.
///
/// On x86_64, uses SSE2 range comparisons to detect whitespace in 16-byte
/// vectors: `(0x09 <= b <= 0x0D) || (b == 0x20)`. Four vectors are processed
/// per iteration (64 bytes), with movemask combining into a 64-bit bitmask
/// for popcount-based word boundary detection.
///
/// Fallback: scalar 64-byte block bitmask approach with table lookup.
pub fn count_words(data: &[u8]) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        // SSE2 is always available on x86_64
        return unsafe { count_words_sse2(data) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        return count_words_scalar(data);
    }
}

/// SSE2-accelerated word counting. Processes 64 bytes per iteration using
/// 4 XMM registers for whitespace detection, then combines into a 64-bit
/// bitmask for word boundary detection via popcount.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn count_words_sse2(data: &[u8]) -> u64 {
    use std::arch::x86_64::*;

    unsafe {
        // Whitespace = (0x09 <= b <= 0x0D) || (b == 0x20)
        // Using signed comparison: cmpgt(b, 0x08) && cmpgt(0x0E, b) || cmpeq(b, 0x20)
        let min_ws = _mm_set1_epi8(0x08); // one below \t
        let max_ws = _mm_set1_epi8(0x0E); // one above \r
        let space = _mm_set1_epi8(0x20);

        let mut words = 0u64;
        let mut prev_ws_bit = 1u64; // treat start-of-data as whitespace

        let chunks = data.chunks_exact(64);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let ptr = chunk.as_ptr();

            // Load 4 x 16-byte vectors
            let v0 = _mm_loadu_si128(ptr as *const __m128i);
            let v1 = _mm_loadu_si128(ptr.add(16) as *const __m128i);
            let v2 = _mm_loadu_si128(ptr.add(32) as *const __m128i);
            let v3 = _mm_loadu_si128(ptr.add(48) as *const __m128i);

            // Detect whitespace in each vector: 3 comparisons + 1 AND + 1 OR
            macro_rules! detect_ws {
                ($v:expr) => {{
                    let ge_9 = _mm_cmpgt_epi8($v, min_ws);
                    let le_d = _mm_cmpgt_epi8(max_ws, $v);
                    let in_range = _mm_and_si128(ge_9, le_d);
                    let is_sp = _mm_cmpeq_epi8($v, space);
                    _mm_or_si128(in_range, is_sp)
                }};
            }

            let ws0 = detect_ws!(v0);
            let ws1 = detect_ws!(v1);
            let ws2 = detect_ws!(v2);
            let ws3 = detect_ws!(v3);

            // Combine 4 x 16-bit movemasks into one 64-bit whitespace mask
            let m0 = (_mm_movemask_epi8(ws0) as u16) as u64;
            let m1 = (_mm_movemask_epi8(ws1) as u16) as u64;
            let m2 = (_mm_movemask_epi8(ws2) as u16) as u64;
            let m3 = (_mm_movemask_epi8(ws3) as u16) as u64;
            let ws_mask = m0 | (m1 << 16) | (m2 << 32) | (m3 << 48);

            // Word starts: where previous byte was whitespace AND current is NOT
            let prev_mask = (ws_mask << 1) | prev_ws_bit;
            let word_starts = prev_mask & !ws_mask;
            words += word_starts.count_ones() as u64;

            prev_ws_bit = (ws_mask >> 63) & 1;
        }

        // Handle 16-byte sub-chunks of remainder
        let sub_chunks = remainder.chunks_exact(16);
        let sub_remainder = sub_chunks.remainder();
        let mut prev_ws_u32 = prev_ws_bit as u32;

        for chunk in sub_chunks {
            let v = _mm_loadu_si128(chunk.as_ptr() as *const __m128i);
            let ge_9 = _mm_cmpgt_epi8(v, min_ws);
            let le_d = _mm_cmpgt_epi8(max_ws, v);
            let in_range = _mm_and_si128(ge_9, le_d);
            let is_sp = _mm_cmpeq_epi8(v, space);
            let ws_vec = _mm_or_si128(in_range, is_sp);
            let ws_mask = _mm_movemask_epi8(ws_vec) as u32;

            let prev_mask = (ws_mask << 1) | prev_ws_u32;
            let word_starts = prev_mask & (!ws_mask & 0xFFFF);
            words += word_starts.count_ones() as u64;
            prev_ws_u32 = (ws_mask >> 15) & 1;
        }

        // Scalar for final <16 bytes
        let mut prev_ws = prev_ws_u32 as u8;
        for &b in sub_remainder {
            let curr_ws = WS_TABLE[b as usize];
            words += (prev_ws & (curr_ws ^ 1)) as u64;
            prev_ws = curr_ws;
        }
        words
    }
}

/// Scalar word counting fallback for non-x86 platforms.
/// Uses 64-byte block bitmask operations with table lookup + popcount.
#[cfg(not(target_arch = "x86_64"))]
fn count_words_scalar(data: &[u8]) -> u64 {
    let mut words = 0u64;
    let mut prev_ws_bit = 1u64;

    let chunks = data.chunks_exact(64);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let mut ws_mask = 0u64;
        let mut i = 0;
        while i + 7 < 64 {
            ws_mask |= (WS_TABLE[chunk[i] as usize] as u64) << i;
            ws_mask |= (WS_TABLE[chunk[i + 1] as usize] as u64) << (i + 1);
            ws_mask |= (WS_TABLE[chunk[i + 2] as usize] as u64) << (i + 2);
            ws_mask |= (WS_TABLE[chunk[i + 3] as usize] as u64) << (i + 3);
            ws_mask |= (WS_TABLE[chunk[i + 4] as usize] as u64) << (i + 4);
            ws_mask |= (WS_TABLE[chunk[i + 5] as usize] as u64) << (i + 5);
            ws_mask |= (WS_TABLE[chunk[i + 6] as usize] as u64) << (i + 6);
            ws_mask |= (WS_TABLE[chunk[i + 7] as usize] as u64) << (i + 7);
            i += 8;
        }

        let prev_mask = (ws_mask << 1) | prev_ws_bit;
        let word_starts = prev_mask & !ws_mask;
        words += word_starts.count_ones() as u64;
        prev_ws_bit = (ws_mask >> 63) & 1;
    }

    let mut prev_ws = prev_ws_bit as u8;
    for &b in remainder {
        let curr_ws = WS_TABLE[b as usize];
        words += (prev_ws & (curr_ws ^ 1)) as u64;
        prev_ws = curr_ws;
    }
    words
}

/// Count lines and words in a single pass using 64-byte bitmask blocks.
///
/// Eliminates the separate memchr line-counting pass by piggybacking newline
/// counting onto the whitespace bitmask already computed for word counting.
/// For each 64-byte block, we build both a whitespace mask and a newline mask,
/// then use popcount on each.
pub fn count_lines_words(data: &[u8]) -> (u64, u64) {
    let mut words = 0u64;
    let mut lines = 0u64;
    let mut prev_ws_bit = 1u64;

    let chunks = data.chunks_exact(64);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let mut ws_mask = 0u64;
        let mut nl_mask = 0u64;
        let mut i = 0;
        while i + 7 < 64 {
            let b0 = chunk[i];
            let b1 = chunk[i + 1];
            let b2 = chunk[i + 2];
            let b3 = chunk[i + 3];
            let b4 = chunk[i + 4];
            let b5 = chunk[i + 5];
            let b6 = chunk[i + 6];
            let b7 = chunk[i + 7];
            ws_mask |= (WS_TABLE[b0 as usize] as u64) << i;
            ws_mask |= (WS_TABLE[b1 as usize] as u64) << (i + 1);
            ws_mask |= (WS_TABLE[b2 as usize] as u64) << (i + 2);
            ws_mask |= (WS_TABLE[b3 as usize] as u64) << (i + 3);
            ws_mask |= (WS_TABLE[b4 as usize] as u64) << (i + 4);
            ws_mask |= (WS_TABLE[b5 as usize] as u64) << (i + 5);
            ws_mask |= (WS_TABLE[b6 as usize] as u64) << (i + 6);
            ws_mask |= (WS_TABLE[b7 as usize] as u64) << (i + 7);
            nl_mask |= ((b0 == b'\n') as u64) << i;
            nl_mask |= ((b1 == b'\n') as u64) << (i + 1);
            nl_mask |= ((b2 == b'\n') as u64) << (i + 2);
            nl_mask |= ((b3 == b'\n') as u64) << (i + 3);
            nl_mask |= ((b4 == b'\n') as u64) << (i + 4);
            nl_mask |= ((b5 == b'\n') as u64) << (i + 5);
            nl_mask |= ((b6 == b'\n') as u64) << (i + 6);
            nl_mask |= ((b7 == b'\n') as u64) << (i + 7);
            i += 8;
        }

        let prev_mask = (ws_mask << 1) | prev_ws_bit;
        let word_starts = prev_mask & !ws_mask;
        words += word_starts.count_ones() as u64;
        lines += nl_mask.count_ones() as u64;
        prev_ws_bit = (ws_mask >> 63) & 1;
    }

    let mut prev_ws = prev_ws_bit as u8;
    for &b in remainder {
        if b == b'\n' {
            lines += 1;
        }
        let curr_ws = WS_TABLE[b as usize];
        words += (prev_ws & (curr_ws ^ 1)) as u64;
        prev_ws = curr_ws;
    }
    (lines, words)
}

/// Count lines, words, and UTF-8 chars in a single pass using 64-byte bitmask blocks.
///
/// Builds whitespace, newline, and non-continuation-byte masks simultaneously
/// in one read of the data, then uses popcount on each mask. This is the most
/// memory-efficient approach when all three metrics are needed, as it touches
/// each cache line only once.
pub fn count_lines_words_chars_utf8(data: &[u8]) -> (u64, u64, u64) {
    let mut words = 0u64;
    let mut lines = 0u64;
    let mut chars = 0u64;
    let mut prev_ws_bit = 1u64;

    let chunks = data.chunks_exact(64);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let mut ws_mask = 0u64;
        let mut nl_mask = 0u64;
        let mut char_mask = 0u64;
        let mut i = 0;
        while i + 7 < 64 {
            let b0 = chunk[i];
            let b1 = chunk[i + 1];
            let b2 = chunk[i + 2];
            let b3 = chunk[i + 3];
            let b4 = chunk[i + 4];
            let b5 = chunk[i + 5];
            let b6 = chunk[i + 6];
            let b7 = chunk[i + 7];

            ws_mask |= (WS_TABLE[b0 as usize] as u64) << i
                | (WS_TABLE[b1 as usize] as u64) << (i + 1)
                | (WS_TABLE[b2 as usize] as u64) << (i + 2)
                | (WS_TABLE[b3 as usize] as u64) << (i + 3)
                | (WS_TABLE[b4 as usize] as u64) << (i + 4)
                | (WS_TABLE[b5 as usize] as u64) << (i + 5)
                | (WS_TABLE[b6 as usize] as u64) << (i + 6)
                | (WS_TABLE[b7 as usize] as u64) << (i + 7);

            nl_mask |= ((b0 == b'\n') as u64) << i
                | ((b1 == b'\n') as u64) << (i + 1)
                | ((b2 == b'\n') as u64) << (i + 2)
                | ((b3 == b'\n') as u64) << (i + 3)
                | ((b4 == b'\n') as u64) << (i + 4)
                | ((b5 == b'\n') as u64) << (i + 5)
                | ((b6 == b'\n') as u64) << (i + 6)
                | ((b7 == b'\n') as u64) << (i + 7);

            char_mask |= (((b0 & 0xC0) != 0x80) as u64) << i
                | (((b1 & 0xC0) != 0x80) as u64) << (i + 1)
                | (((b2 & 0xC0) != 0x80) as u64) << (i + 2)
                | (((b3 & 0xC0) != 0x80) as u64) << (i + 3)
                | (((b4 & 0xC0) != 0x80) as u64) << (i + 4)
                | (((b5 & 0xC0) != 0x80) as u64) << (i + 5)
                | (((b6 & 0xC0) != 0x80) as u64) << (i + 6)
                | (((b7 & 0xC0) != 0x80) as u64) << (i + 7);

            i += 8;
        }
        let prev_mask = (ws_mask << 1) | prev_ws_bit;
        let word_starts = prev_mask & !ws_mask;
        words += word_starts.count_ones() as u64;
        lines += nl_mask.count_ones() as u64;
        chars += char_mask.count_ones() as u64;
        prev_ws_bit = (ws_mask >> 63) & 1;
    }

    let mut prev_ws = prev_ws_bit as u8;
    for &b in remainder {
        if b == b'\n' {
            lines += 1;
        }
        let curr_ws = WS_TABLE[b as usize];
        words += (prev_ws & (curr_ws ^ 1)) as u64;
        prev_ws = curr_ws;
        chars += ((b & 0xC0) != 0x80) as u64;
    }
    (lines, words, chars)
}

/// Count UTF-8 characters by counting non-continuation bytes.
/// A continuation byte has the bit pattern `10xxxxxx` (0x80..0xBF).
/// Every other byte starts a new character (ASCII, multi-byte leader, or invalid).
///
/// Uses 64-byte block processing with popcount for ~4x throughput vs scalar.
pub fn count_chars_utf8(data: &[u8]) -> u64 {
    let mut count = 0u64;
    let chunks = data.chunks_exact(64);
    let remainder = chunks.remainder();

    for chunk in chunks {
        // Build 64-bit mask: bit i = 1 if chunk[i] is NOT a continuation byte
        let mut char_mask = 0u64;
        let mut i = 0;
        while i + 7 < 64 {
            char_mask |= (((chunk[i] & 0xC0) != 0x80) as u64) << i;
            char_mask |= (((chunk[i + 1] & 0xC0) != 0x80) as u64) << (i + 1);
            char_mask |= (((chunk[i + 2] & 0xC0) != 0x80) as u64) << (i + 2);
            char_mask |= (((chunk[i + 3] & 0xC0) != 0x80) as u64) << (i + 3);
            char_mask |= (((chunk[i + 4] & 0xC0) != 0x80) as u64) << (i + 4);
            char_mask |= (((chunk[i + 5] & 0xC0) != 0x80) as u64) << (i + 5);
            char_mask |= (((chunk[i + 6] & 0xC0) != 0x80) as u64) << (i + 6);
            char_mask |= (((chunk[i + 7] & 0xC0) != 0x80) as u64) << (i + 7);
            i += 8;
        }
        count += char_mask.count_ones() as u64;
    }

    for &b in remainder {
        count += ((b & 0xC0) != 0x80) as u64;
    }
    count
}

/// Count characters in C/POSIX locale (each byte is one character).
#[inline]
pub fn count_chars_c(data: &[u8]) -> u64 {
    data.len() as u64
}

/// Count characters, choosing behavior based on locale.
#[inline]
pub fn count_chars(data: &[u8], utf8: bool) -> u64 {
    if utf8 {
        count_chars_utf8(data)
    } else {
        count_chars_c(data)
    }
}

/// Detect if the current locale uses UTF-8 encoding.
pub fn is_utf8_locale() -> bool {
    for var in &["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                let lower = val.to_ascii_lowercase();
                return lower.contains("utf-8") || lower.contains("utf8");
            }
        }
    }
    false
}

/// Decode one UTF-8 character from a byte slice.
/// Returns (codepoint, byte_length). On invalid UTF-8, returns (byte as u32, 1).
#[inline]
fn decode_utf8(bytes: &[u8]) -> (u32, usize) {
    let b0 = bytes[0];
    if b0 < 0x80 {
        return (b0 as u32, 1);
    }
    if b0 < 0xC2 {
        // Continuation byte or overlong 2-byte — invalid as start
        return (b0 as u32, 1);
    }
    if b0 < 0xE0 {
        if bytes.len() < 2 || bytes[1] & 0xC0 != 0x80 {
            return (b0 as u32, 1);
        }
        let cp = ((b0 as u32 & 0x1F) << 6) | (bytes[1] as u32 & 0x3F);
        return (cp, 2);
    }
    if b0 < 0xF0 {
        if bytes.len() < 3 || bytes[1] & 0xC0 != 0x80 || bytes[2] & 0xC0 != 0x80 {
            return (b0 as u32, 1);
        }
        let cp = ((b0 as u32 & 0x0F) << 12)
            | ((bytes[1] as u32 & 0x3F) << 6)
            | (bytes[2] as u32 & 0x3F);
        return (cp, 3);
    }
    if b0 < 0xF5 {
        if bytes.len() < 4
            || bytes[1] & 0xC0 != 0x80
            || bytes[2] & 0xC0 != 0x80
            || bytes[3] & 0xC0 != 0x80
        {
            return (b0 as u32, 1);
        }
        let cp = ((b0 as u32 & 0x07) << 18)
            | ((bytes[1] as u32 & 0x3F) << 12)
            | ((bytes[2] as u32 & 0x3F) << 6)
            | (bytes[3] as u32 & 0x3F);
        return (cp, 4);
    }
    (b0 as u32, 1)
}

/// Check if a Unicode codepoint is an East Asian Wide/Fullwidth character (display width 2).
#[inline]
fn is_wide_char(cp: u32) -> bool {
    matches!(cp,
        0x1100..=0x115F   // Hangul Jamo
        | 0x231A..=0x231B // Watch, Hourglass
        | 0x2329..=0x232A // Angle Brackets
        | 0x23E9..=0x23F3 // Various symbols
        | 0x23F8..=0x23FA
        | 0x25FD..=0x25FE
        | 0x2614..=0x2615
        | 0x2648..=0x2653
        | 0x267F
        | 0x2693
        | 0x26A1
        | 0x26AA..=0x26AB
        | 0x26BD..=0x26BE
        | 0x26C4..=0x26C5
        | 0x26CE
        | 0x26D4
        | 0x26EA
        | 0x26F2..=0x26F3
        | 0x26F5
        | 0x26FA
        | 0x26FD
        | 0x2702
        | 0x2705
        | 0x2708..=0x270D
        | 0x270F
        | 0x2712
        | 0x2714
        | 0x2716
        | 0x271D
        | 0x2721
        | 0x2728
        | 0x2733..=0x2734
        | 0x2744
        | 0x2747
        | 0x274C
        | 0x274E
        | 0x2753..=0x2755
        | 0x2757
        | 0x2763..=0x2764
        | 0x2795..=0x2797
        | 0x27A1
        | 0x27B0
        | 0x27BF
        | 0x2934..=0x2935
        | 0x2B05..=0x2B07
        | 0x2B1B..=0x2B1C
        | 0x2B50
        | 0x2B55
        | 0x2E80..=0x303E  // CJK Radicals, Kangxi Radicals, Ideographic Description
        | 0x3041..=0x33BF  // Hiragana, Katakana, Bopomofo, Hangul Compat Jamo, Kanbun, CJK
        | 0x3400..=0x4DBF  // CJK Unified Ideographs Extension A
        | 0x4E00..=0xA4CF  // CJK Unified Ideographs, Yi
        | 0xA960..=0xA97C  // Hangul Jamo Extended-A
        | 0xAC00..=0xD7A3  // Hangul Syllables
        | 0xF900..=0xFAFF  // CJK Compatibility Ideographs
        | 0xFE10..=0xFE19  // Vertical Forms
        | 0xFE30..=0xFE6F  // CJK Compatibility Forms
        | 0xFF01..=0xFF60  // Fullwidth Latin, Halfwidth Katakana
        | 0xFFE0..=0xFFE6  // Fullwidth Signs
        | 0x1F004
        | 0x1F0CF
        | 0x1F170..=0x1F171
        | 0x1F17E..=0x1F17F
        | 0x1F18E
        | 0x1F191..=0x1F19A
        | 0x1F1E0..=0x1F1FF // Regional Indicators
        | 0x1F200..=0x1F202
        | 0x1F210..=0x1F23B
        | 0x1F240..=0x1F248
        | 0x1F250..=0x1F251
        | 0x1F260..=0x1F265
        | 0x1F300..=0x1F64F // Misc Symbols, Emoticons
        | 0x1F680..=0x1F6FF // Transport Symbols
        | 0x1F900..=0x1F9FF // Supplemental Symbols
        | 0x1FA00..=0x1FA6F
        | 0x1FA70..=0x1FAFF
        | 0x20000..=0x2FFFD // CJK Unified Ideographs Extension B-F
        | 0x30000..=0x3FFFD // CJK Unified Ideographs Extension G
    )
}

/// Compute maximum display width of any line (C/POSIX locale).
///
/// GNU wc -L behavior in C locale:
/// - `\n`: line terminator (records max, resets position)
/// - `\t`: advances to next tab stop (multiple of 8)
/// - `\r`: carriage return (resets position to 0, same line)
/// - `\f`: form feed (acts as line terminator like \n)
/// - Printable ASCII (0x20..0x7E): width 1
/// - Everything else (controls, high bytes): width 0
pub fn max_line_length_c(data: &[u8]) -> u64 {
    let mut max_len: u64 = 0;
    let mut line_len: u64 = 0; // max position seen on current line
    let mut linepos: u64 = 0; // current cursor position

    for &b in data {
        match b {
            b'\n' => {
                if line_len > max_len {
                    max_len = line_len;
                }
                linepos = 0;
                line_len = 0;
            }
            b'\t' => {
                linepos = (linepos + 8) & !7;
                if linepos > line_len {
                    line_len = linepos;
                }
            }
            b'\r' => {
                linepos = 0;
            }
            0x0C => {
                // Form feed: acts as line terminator
                if line_len > max_len {
                    max_len = line_len;
                }
                linepos = 0;
                line_len = 0;
            }
            _ => {
                if PRINTABLE_TABLE[b as usize] != 0 {
                    linepos += 1;
                    if linepos > line_len {
                        line_len = linepos;
                    }
                }
                // Non-printable: width 0
            }
        }
    }

    // Handle last line (may not end with \n)
    if line_len > max_len {
        max_len = line_len;
    }

    max_len
}

/// Compute maximum display width of any line (UTF-8 locale).
///
/// GNU wc -L in UTF-8 locale uses mbrtowc() + wcwidth() for display width.
/// East Asian Wide/Fullwidth characters get width 2, most others get width 1.
pub fn max_line_length_utf8(data: &[u8]) -> u64 {
    let mut max_len: u64 = 0;
    let mut line_len: u64 = 0;
    let mut linepos: u64 = 0;
    let mut i = 0;

    while i < data.len() {
        let b = data[i];

        // Fast path for common ASCII
        if b < 0x80 {
            match b {
                b'\n' => {
                    if line_len > max_len {
                        max_len = line_len;
                    }
                    linepos = 0;
                    line_len = 0;
                }
                b'\t' => {
                    linepos = (linepos + 8) & !7;
                    if linepos > line_len {
                        line_len = linepos;
                    }
                }
                b'\r' => {
                    linepos = 0;
                }
                0x0C => {
                    // Form feed: line terminator
                    if line_len > max_len {
                        max_len = line_len;
                    }
                    linepos = 0;
                    line_len = 0;
                }
                0x20..=0x7E => {
                    // Printable ASCII
                    linepos += 1;
                    if linepos > line_len {
                        line_len = linepos;
                    }
                }
                _ => {
                    // Non-printable ASCII control chars: width 0
                }
            }
            i += 1;
        } else {
            // Multibyte UTF-8
            let (cp, len) = decode_utf8(&data[i..]);

            // C1 control characters (0x80..0x9F): non-printable
            if cp <= 0x9F {
                // width 0
            } else if is_wide_char(cp) {
                linepos += 2;
                if linepos > line_len {
                    line_len = linepos;
                }
            } else {
                // Regular printable Unicode character: width 1
                linepos += 1;
                if linepos > line_len {
                    line_len = linepos;
                }
            }
            i += len;
        }
    }

    // Handle last line
    if line_len > max_len {
        max_len = line_len;
    }

    max_len
}

/// Compute maximum display width, choosing behavior based on locale.
#[inline]
pub fn max_line_length(data: &[u8], utf8: bool) -> u64 {
    if utf8 {
        max_line_length_utf8(data)
    } else {
        max_line_length_c(data)
    }
}

/// Count all metrics using optimized individual passes.
///
/// Each metric uses its own optimized algorithm:
/// - Lines: SIMD-accelerated memchr
/// - Words: branchless lookup table
/// - Chars: non-continuation byte counting (UTF-8) or byte counting (C locale)
/// - Max line length: locale-aware display width tracking
///
/// Multi-pass is faster than single-pass because each pass has a tight,
/// specialized loop. After the first pass, data is hot in L2/L3 cache,
/// making subsequent passes nearly free for memory bandwidth.
pub fn count_all(data: &[u8], utf8: bool) -> WcCounts {
    WcCounts {
        lines: count_lines(data),
        words: count_words(data),
        bytes: data.len() as u64,
        chars: count_chars(data, utf8),
        max_line_length: max_line_length(data, utf8),
    }
}

// ──────────────────────────────────────────────────
// Parallel counting for large files
// ──────────────────────────────────────────────────

/// Count newlines in parallel using SIMD memchr + rayon.
pub fn count_lines_parallel(data: &[u8]) -> u64 {
    if data.len() < PARALLEL_THRESHOLD {
        return count_lines(data);
    }

    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (data.len() / num_threads).max(1024 * 1024);

    data.par_chunks(chunk_size)
        .map(|chunk| memchr_iter(b'\n', chunk).count() as u64)
        .sum()
}

/// Count words in parallel with boundary adjustment.
///
/// Each chunk is counted independently, then boundaries are checked:
/// if chunk N ends with non-whitespace and chunk N+1 starts with non-whitespace,
/// a word was split across the boundary and double-counted — subtract 1.
pub fn count_words_parallel(data: &[u8]) -> u64 {
    if data.len() < PARALLEL_THRESHOLD {
        return count_words(data);
    }

    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (data.len() / num_threads).max(1024 * 1024);

    let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();

    // Each chunk produces: (word_count, first_byte_is_non_ws, last_byte_is_non_ws)
    let results: Vec<(u64, bool, bool)> = chunks
        .par_iter()
        .map(|chunk| {
            let words = count_words(chunk);
            let starts_non_ws = chunk
                .first()
                .is_some_and(|&b| WS_TABLE[b as usize] == 0);
            let ends_non_ws = chunk
                .last()
                .is_some_and(|&b| WS_TABLE[b as usize] == 0);
            (words, starts_non_ws, ends_non_ws)
        })
        .collect();

    let mut total = 0u64;
    for i in 0..results.len() {
        total += results[i].0;
        // If previous chunk ends with non-ws and this chunk starts with non-ws,
        // a word spans the boundary and was counted as two separate words.
        if i > 0 && results[i].1 && results[i - 1].2 {
            total -= 1;
        }
    }
    total
}

/// Count UTF-8 characters in parallel.
pub fn count_chars_parallel(data: &[u8], utf8: bool) -> u64 {
    if !utf8 {
        return data.len() as u64;
    }
    if data.len() < PARALLEL_THRESHOLD {
        return count_chars_utf8(data);
    }

    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (data.len() / num_threads).max(1024 * 1024);

    data.par_chunks(chunk_size)
        .map(count_chars_utf8)
        .sum()
}

/// Partial result from processing a chunk, used to combine results at boundaries.
struct ChunkResult {
    lines: u64,
    words: u64,
    chars: u64,
    starts_non_ws: bool,
    ends_non_ws: bool,
}

/// Combined parallel counting of lines + words + chars.
///
/// Uses SIMD-accelerated memchr for line counting plus optimized bitmask word
/// counting per chunk, with data staying cache-warm between passes.
/// On compute-bound CPUs (like Atom), the two-pass approach is faster than a
/// combined scalar single-pass because memchr uses SIMD (16+ bytes/cycle)
/// while the combined loop adds non-SIMD overhead to every byte.
pub fn count_lwc_parallel(data: &[u8], utf8: bool) -> (u64, u64, u64) {
    if data.len() < PARALLEL_THRESHOLD {
        // Small files: SIMD memchr + scalar word counting, sequential
        let lines = count_lines(data);
        let words = count_words(data);
        let chars = count_chars(data, utf8);
        return (lines, words, chars);
    }

    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (data.len() / num_threads).max(1024 * 1024);

    let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();

    let results: Vec<ChunkResult> = chunks
        .par_iter()
        .map(|chunk| {
            // Pass 1: SIMD-accelerated line counting (loads data into cache)
            let lines = memchr_iter(b'\n', chunk).count() as u64;
            // Pass 2: Bitmask word counting (data now cache-warm from pass 1)
            let words = count_words(chunk);
            // Pass 3: Char counting (fast with warm cache, or free for non-UTF8)
            let chars = if utf8 {
                count_chars_utf8(chunk)
            } else {
                chunk.len() as u64
            };
            let starts_non_ws = chunk
                .first()
                .is_some_and(|&b| WS_TABLE[b as usize] == 0);
            let ends_non_ws = chunk
                .last()
                .is_some_and(|&b| WS_TABLE[b as usize] == 0);
            ChunkResult {
                lines,
                words,
                chars,
                starts_non_ws,
                ends_non_ws,
            }
        })
        .collect();

    let mut total_lines = 0u64;
    let mut total_words = 0u64;
    let mut total_chars = 0u64;
    for i in 0..results.len() {
        total_lines += results[i].lines;
        total_words += results[i].words;
        total_chars += results[i].chars;
        if i > 0 && results[i].starts_non_ws && results[i - 1].ends_non_ws {
            total_words -= 1;
        }
    }
    (total_lines, total_words, total_chars)
}
