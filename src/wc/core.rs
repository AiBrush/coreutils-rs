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

// ──────────────────────────────────────────────────
// 3-state byte classification for word counting
// ──────────────────────────────────────────────────
//
// GNU wc uses mbrtowc() + iswspace() + iswprint() with 3-state logic:
//   0 = printable (word content): starts or continues a word
//   1 = space (word break): ends any current word
//   2 = transparent (unchanged): non-printable, non-space — does NOT change in_word
//
// The critical difference from 2-state is that transparent characters
// (NUL, control chars, invalid UTF-8) do NOT break words.
// Example: "hello\x00world" is 1 word (NUL is transparent), not 2.

/// 3-state byte classification for C/POSIX locale.
/// In C locale, mbrtowc() fails for bytes >= 0x80, making them transparent.
/// Only printable ASCII (0x21-0x7E) forms words.
const fn make_byte_class_c() -> [u8; 256] {
    let mut t = [2u8; 256]; // default: transparent
    // Spaces: iswspace() returns true
    t[0x09] = 1; // \t
    t[0x0A] = 1; // \n
    t[0x0B] = 1; // \v
    t[0x0C] = 1; // \f
    t[0x0D] = 1; // \r
    t[0x20] = 1; // space
    // Printable ASCII (0x21-0x7E): word content
    let mut i = 0x21u16;
    while i <= 0x7E {
        t[i as usize] = 0;
        i += 1;
    }
    t
}

const BYTE_CLASS_C: [u8; 256] = make_byte_class_c();

/// 3-state single-byte classification for UTF-8 locale.
/// Multi-byte UTF-8 sequences are handled by the state machine separately.
const fn make_byte_class_utf8() -> [u8; 256] {
    let mut t = [2u8; 256]; // default: transparent
    // Spaces
    t[0x09] = 1; // \t
    t[0x0A] = 1; // \n
    t[0x0B] = 1; // \v
    t[0x0C] = 1; // \f
    t[0x0D] = 1; // \r
    t[0x20] = 1; // space
    // Printable ASCII (0x21-0x7E): word content
    let mut i = 0x21u16;
    while i <= 0x7E {
        t[i as usize] = 0;
        i += 1;
    }
    t
}

const BYTE_CLASS_UTF8: [u8; 256] = make_byte_class_utf8();

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

// ──────────────────────────────────────────────────
// Unicode character classification helpers
// ──────────────────────────────────────────────────

/// Check if a Unicode codepoint is a whitespace character (matching glibc iswspace).
/// Only covers multi-byte Unicode spaces; ASCII spaces are handled by the byte table.
#[inline]
fn is_unicode_space(cp: u32) -> bool {
    matches!(
        cp,
        0x00A0 |           // No-Break Space
        0x1680 |           // Ogham Space Mark
        0x2000
            ..=0x200A |  // En Quad through Hair Space
        0x2028 |           // Line Separator
        0x2029 |           // Paragraph Separator
        0x202F |           // Narrow No-Break Space
        0x205F |           // Medium Mathematical Space
        0x3000 // Ideographic Space
    )
}

/// Check if a Unicode codepoint (>= 0x80) is printable (matching glibc iswprint).
/// C1 control characters (U+0080-U+009F) are not printable.
/// Most characters >= U+00A0 are printable.
#[inline]
fn is_unicode_printable(cp: u32) -> bool {
    cp >= 0xA0
}

// ──────────────────────────────────────────────────
// Core counting functions
// ──────────────────────────────────────────────────

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

/// Count words using locale-aware 3-state logic (default: UTF-8).
pub fn count_words(data: &[u8]) -> u64 {
    count_words_locale(data, true)
}

/// Count words with explicit locale control using 3-state logic.
///
/// GNU wc classifies each character as:
///   - space (iswspace=true): sets in_word=false
///   - printable (iswprint=true): sets in_word=true, increments word count on transition
///   - transparent (neither): leaves in_word unchanged
pub fn count_words_locale(data: &[u8], utf8: bool) -> u64 {
    if utf8 {
        count_words_utf8(data)
    } else {
        count_words_c(data)
    }
}

/// Count words in C/POSIX locale using 3-state scalar logic.
/// Only printable ASCII (0x21-0x7E) forms words.
/// Bytes >= 0x80 and non-printable ASCII controls are transparent.
fn count_words_c(data: &[u8]) -> u64 {
    let mut words = 0u64;
    let mut in_word = false;
    for &b in data {
        let class = BYTE_CLASS_C[b as usize];
        if class == 1 {
            // Space: break word
            in_word = false;
        } else if class == 0 {
            // Printable: start/continue word
            if !in_word {
                in_word = true;
                words += 1;
            }
        }
        // class == 2: transparent — in_word unchanged
    }
    words
}

/// Count words in a C locale chunk, returning word count plus boundary info.
/// Used by parallel word counting.
/// Returns (word_count, first_active_is_printable, ends_in_word).
fn count_words_c_chunk(data: &[u8]) -> (u64, bool, bool) {
    let mut words = 0u64;
    let mut in_word = false;
    let mut first_active_is_printable = false;
    let mut seen_active = false;

    for &b in data {
        let class = BYTE_CLASS_C[b as usize];
        if class == 1 {
            if !seen_active {
                seen_active = true;
                // first_active_is_printable stays false
            }
            in_word = false;
        } else if class == 0 {
            if !seen_active {
                seen_active = true;
                first_active_is_printable = true;
            }
            if !in_word {
                in_word = true;
                words += 1;
            }
        }
    }
    (words, first_active_is_printable, in_word)
}

/// Count words in UTF-8 locale using a state machine with 3-state logic.
///
/// Handles:
/// - ASCII spaces (0x09-0x0D, 0x20): word break
/// - ASCII printable (0x21-0x7E): word content
/// - ASCII non-printable (0x00-0x08, 0x0E-0x1F, 0x7F): transparent
/// - Valid UTF-8 multi-byte → check Unicode space/printable
/// - Invalid UTF-8: transparent (GNU wc skips invalid bytes without changing state)
fn count_words_utf8(data: &[u8]) -> u64 {
    let mut words = 0u64;
    let mut in_word = false;
    let mut i = 0;

    while i < data.len() {
        let b = data[i];

        if b < 0x80 {
            // ASCII: use 3-state lookup table
            let class = BYTE_CLASS_UTF8[b as usize];
            if class == 1 {
                in_word = false;
            } else if class == 0 {
                if !in_word {
                    in_word = true;
                    words += 1;
                }
            }
            // class == 2: transparent
            i += 1;
        } else if b < 0xC2 {
            // 0x80-0xBF: standalone continuation byte (invalid UTF-8)
            // 0xC0-0xC1: overlong encoding (invalid UTF-8)
            // Transparent: don't change in_word
            i += 1;
        } else if b < 0xE0 {
            // 2-byte sequence: need 1 continuation byte
            if i + 1 < data.len() && (data[i + 1] & 0xC0) == 0x80 {
                let cp = ((b as u32 & 0x1F) << 6) | (data[i + 1] as u32 & 0x3F);
                if is_unicode_space(cp) {
                    in_word = false;
                } else if is_unicode_printable(cp) {
                    if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
                // else: non-printable (e.g., C1 controls U+0080-U+009F) → transparent
                i += 2;
            } else {
                // Invalid sequence: transparent
                i += 1;
            }
        } else if b < 0xF0 {
            // 3-byte sequence: need 2 continuation bytes
            if i + 2 < data.len() && (data[i + 1] & 0xC0) == 0x80 && (data[i + 2] & 0xC0) == 0x80 {
                let cp = ((b as u32 & 0x0F) << 12)
                    | ((data[i + 1] as u32 & 0x3F) << 6)
                    | (data[i + 2] as u32 & 0x3F);
                if is_unicode_space(cp) {
                    in_word = false;
                } else if is_unicode_printable(cp) {
                    if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
                i += 3;
            } else {
                // Invalid: transparent
                i += 1;
            }
        } else if b < 0xF5 {
            // 4-byte sequence: need 3 continuation bytes
            if i + 3 < data.len()
                && (data[i + 1] & 0xC0) == 0x80
                && (data[i + 2] & 0xC0) == 0x80
                && (data[i + 3] & 0xC0) == 0x80
            {
                let cp = ((b as u32 & 0x07) << 18)
                    | ((data[i + 1] as u32 & 0x3F) << 12)
                    | ((data[i + 2] as u32 & 0x3F) << 6)
                    | (data[i + 3] as u32 & 0x3F);
                if is_unicode_space(cp) {
                    in_word = false;
                } else if is_unicode_printable(cp) {
                    if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
                i += 4;
            } else {
                // Invalid: transparent
                i += 1;
            }
        } else {
            // 0xF5-0xFF: invalid UTF-8 — transparent
            i += 1;
        }
    }

    words
}

/// Count lines and words using optimized strategies per locale.
/// UTF-8: fused single-pass for lines+words to avoid extra data traversal.
/// C locale: single scalar pass with 3-state logic.
pub fn count_lines_words(data: &[u8], utf8: bool) -> (u64, u64) {
    if utf8 {
        count_lines_words_utf8_fused(data)
    } else {
        let mut lines = 0u64;
        let mut words = 0u64;
        let mut in_word = false;
        for &b in data {
            if b == b'\n' {
                lines += 1;
            }
            let class = BYTE_CLASS_C[b as usize];
            if class == 1 {
                in_word = false;
            } else if class == 0 {
                if !in_word {
                    in_word = true;
                    words += 1;
                }
            }
        }
        (lines, words)
    }
}

/// Fused lines+words counting in UTF-8 mode (single pass).
/// Avoids separate memchr pass for newlines by counting them inline with words.
fn count_lines_words_utf8_fused(data: &[u8]) -> (u64, u64) {
    let mut lines = 0u64;
    let mut words = 0u64;
    let mut in_word = false;
    let mut i = 0;

    while i < data.len() {
        let b = data[i];

        if b < 0x80 {
            // ASCII fast path: combined newline + word counting
            if b == b'\n' {
                lines += 1;
                in_word = false;
            } else {
                let class = BYTE_CLASS_UTF8[b as usize];
                if class == 1 {
                    in_word = false;
                } else if class == 0 {
                    if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
            }
            i += 1;
        } else if b < 0xC2 {
            i += 1;
        } else if b < 0xE0 {
            if i + 1 < data.len() && (data[i + 1] & 0xC0) == 0x80 {
                let cp = ((b as u32 & 0x1F) << 6) | (data[i + 1] as u32 & 0x3F);
                if is_unicode_space(cp) {
                    in_word = false;
                } else if is_unicode_printable(cp) {
                    if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
                i += 2;
            } else {
                i += 1;
            }
        } else if b < 0xF0 {
            if i + 2 < data.len() && (data[i + 1] & 0xC0) == 0x80 && (data[i + 2] & 0xC0) == 0x80 {
                let cp = ((b as u32 & 0x0F) << 12)
                    | ((data[i + 1] as u32 & 0x3F) << 6)
                    | (data[i + 2] as u32 & 0x3F);
                if is_unicode_space(cp) {
                    in_word = false;
                } else if is_unicode_printable(cp) {
                    if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
                i += 3;
            } else {
                i += 1;
            }
        } else if b < 0xF5 {
            if i + 3 < data.len()
                && (data[i + 1] & 0xC0) == 0x80
                && (data[i + 2] & 0xC0) == 0x80
                && (data[i + 3] & 0xC0) == 0x80
            {
                let cp = ((b as u32 & 0x07) << 18)
                    | ((data[i + 1] as u32 & 0x3F) << 12)
                    | ((data[i + 2] as u32 & 0x3F) << 6)
                    | (data[i + 3] as u32 & 0x3F);
                if is_unicode_space(cp) {
                    in_word = false;
                } else if is_unicode_printable(cp) {
                    if !in_word {
                        in_word = true;
                        words += 1;
                    }
                }
                i += 4;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    (lines, words)
}

/// Count lines, words, and chars using optimized strategies per locale.
pub fn count_lines_words_chars(data: &[u8], utf8: bool) -> (u64, u64, u64) {
    if utf8 {
        // Fused single-pass for lines+words, then fast char-counting pass
        let (lines, words) = count_lines_words_utf8_fused(data);
        let chars = count_chars_utf8(data);
        (lines, words, chars)
    } else {
        // C locale: single pass for lines + words, chars = byte count
        let mut lines = 0u64;
        let mut words = 0u64;
        let mut in_word = false;
        for &b in data {
            if b == b'\n' {
                lines += 1;
            }
            let class = BYTE_CLASS_C[b as usize];
            if class == 1 {
                in_word = false;
            } else if class == 0 {
                if !in_word {
                    in_word = true;
                    words += 1;
                }
            }
        }
        (lines, words, data.len() as u64)
    }
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
        let cp =
            ((b0 as u32 & 0x0F) << 12) | ((bytes[1] as u32 & 0x3F) << 6) | (bytes[2] as u32 & 0x3F);
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
    matches!(
        cp,
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
/// - Words: 3-state scalar/state-machine (locale-dependent)
/// - Chars: non-continuation byte counting (UTF-8) or byte counting (C locale)
/// - Max line length: locale-aware display width tracking
///
/// Multi-pass is faster than single-pass because each pass has a tight,
/// specialized loop. After the first pass, data is hot in L2/L3 cache,
/// making subsequent passes nearly free for memory bandwidth.
pub fn count_all(data: &[u8], utf8: bool) -> WcCounts {
    if utf8 {
        let (lines, words) = count_lines_words_utf8_fused(data);
        WcCounts {
            lines,
            words,
            bytes: data.len() as u64,
            chars: count_chars_utf8(data),
            max_line_length: max_line_length_utf8(data),
        }
    } else {
        WcCounts {
            lines: count_lines(data),
            words: count_words_locale(data, false),
            bytes: data.len() as u64,
            chars: data.len() as u64,
            max_line_length: max_line_length_c(data),
        }
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
pub fn count_words_parallel(data: &[u8], utf8: bool) -> u64 {
    if utf8 || data.len() < PARALLEL_THRESHOLD {
        // UTF-8: state machine can't be trivially parallelized
        // (multi-byte sequences may span chunk boundaries).
        return count_words_locale(data, utf8);
    }

    // C locale: parallel 3-state word counting with boundary adjustment
    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (data.len() / num_threads).max(1024 * 1024);

    let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();

    // Each chunk returns (word_count, first_active_is_printable, ends_in_word)
    let results: Vec<(u64, bool, bool)> = chunks
        .par_iter()
        .map(|chunk| count_words_c_chunk(chunk))
        .collect();

    let mut total = 0u64;
    for i in 0..results.len() {
        total += results[i].0;
        // Boundary adjustment: if previous chunk ended in_word AND
        // current chunk's first non-transparent byte is printable,
        // the word was split across chunks — subtract the overcount.
        if i > 0 && results[i - 1].2 && results[i].1 {
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

    data.par_chunks(chunk_size).map(count_chars_utf8).sum()
}

/// Count lines + words + bytes in a single fused pass (the default wc mode).
/// Avoids separate passes entirely — combines newline counting with word detection.
pub fn count_lwb(data: &[u8], utf8: bool) -> (u64, u64, u64) {
    let (lines, words) = count_lines_words(data, utf8);
    (lines, words, data.len() as u64)
}

/// Parallel counting of lines + words + bytes only (no chars).
/// Optimized for the default `wc` mode: avoids unnecessary char-counting pass.
pub fn count_lwb_parallel(data: &[u8], utf8: bool) -> (u64, u64, u64) {
    if data.len() < PARALLEL_THRESHOLD {
        // Small file: use fused single-pass
        return count_lwb(data, utf8);
    }

    // Word counting must be sequential for UTF-8 (state machine across chunks)
    // But we use the fused lines+words approach to avoid a separate memchr pass
    let (lines, words) = if utf8 {
        count_lines_words_utf8_fused(data)
    } else {
        // C locale: parallel 3-state word counting with boundary adjustment
        let num_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / num_threads).max(1024 * 1024);

        let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();
        let results: Vec<(u64, bool, bool)> = chunks
            .par_iter()
            .map(|chunk| count_words_c_chunk(chunk))
            .collect();

        let mut word_total = 0u64;
        for i in 0..results.len() {
            word_total += results[i].0;
            if i > 0 && results[i - 1].2 && results[i].1 {
                word_total -= 1;
            }
        }

        let line_total: u64 = data
            .par_chunks(chunk_size)
            .map(|chunk| memchr_iter(b'\n', chunk).count() as u64)
            .sum();

        (line_total, word_total)
    };

    (lines, words, data.len() as u64)
}

/// Combined parallel counting of lines + words + chars.
pub fn count_lwc_parallel(data: &[u8], utf8: bool) -> (u64, u64, u64) {
    if data.len() < PARALLEL_THRESHOLD {
        let lines = count_lines(data);
        let words = count_words_locale(data, utf8);
        let chars = count_chars(data, utf8);
        return (lines, words, chars);
    }

    // Word counting: sequential for UTF-8 (state machine), parallel for C locale
    let words = count_words_parallel(data, utf8);

    // Lines and chars can always be parallelized safely
    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = (data.len() / num_threads).max(1024 * 1024);

    let lines: u64 = data
        .par_chunks(chunk_size)
        .map(|chunk| memchr_iter(b'\n', chunk).count() as u64)
        .sum();

    let chars = if utf8 {
        data.par_chunks(chunk_size).map(count_chars_utf8).sum()
    } else {
        data.len() as u64
    };

    (lines, words, chars)
}
