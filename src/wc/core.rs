use memchr::memchr_iter;
use rayon::prelude::*;

/// Minimum data size to use parallel processing (1MB).
/// Rayon overhead is ~5-10μs per task; at 1MB with memchr SIMD (~10 GB/s),
/// each chunk takes ~100μs, so overhead is < 10%.
const PARALLEL_THRESHOLD: usize = 1024 * 1024;

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

/// Byte classification for C/POSIX locale word counting.
/// GNU wc in C locale uses mbrtowc() + iswspace() + iswprint() with 3-state logic:
///   0 = printable (word content): starts or continues a word
///   1 = space (word break): ends any current word
///   2 = transparent (unchanged): non-printable, non-space — does NOT change in_word
///
/// In C locale, mbrtowc returns each byte as-is, then:
///   - iswspace: 0x09-0x0D, 0x20 -> true (space)
///   - iswprint: 0x20-0x7E -> true (printable); note: 0x20 is both space and printable,
///     but space takes priority in GNU wc's logic
///   - Bytes 0x00-0x08, 0x0E-0x1F, 0x7F, 0x80-0xFF: neither space nor printable -> transparent
///
/// This matches the UTF-8 byte table for single-byte classification.
const BYTE_CLASS_C: [u8; 256] = BYTE_CLASS_UTF8;

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

/// Count words in C/POSIX locale using 3-state logic matching GNU wc.
/// GNU wc in C locale uses mbrtowc() + iswspace() + iswprint():
///   - class 0 (printable: 0x20-0x7E minus spaces, i.e. 0x21-0x7E): word content
///   - class 1 (space: 0x09-0x0D, 0x20): word break
///   - class 2 (transparent: everything else): leaves in_word unchanged
///
/// This means bytes >= 0x80 and control characters do NOT start or break words.
///
/// Optimized with printable ASCII run skipping.
fn count_words_c(data: &[u8]) -> u64 {
    let mut words = 0u64;
    let mut in_word = false;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };
        if b >= 0x21 && b <= 0x7E {
            // Printable ASCII — word content (most common for text)
            if !in_word {
                in_word = true;
                words += 1;
            }
            i += 1;
            // Skip remaining printable ASCII
            while i < len {
                let b = unsafe { *data.get_unchecked(i) };
                if b >= 0x21 && b <= 0x7E {
                    i += 1;
                } else {
                    break;
                }
            }
        } else {
            let class = unsafe { *BYTE_CLASS_C.get_unchecked(b as usize) };
            if class == 1 {
                // Space — break word
                in_word = false;
            }
            // class == 2 (transparent): leave in_word unchanged
            i += 1;
        }
    }
    words
}

/// AVX2-accelerated fused line+word counter for C locale chunks.
/// Processes 32 bytes per iteration using 3-state logic:
///   - Printable ASCII (0x21-0x7E): word content (starts/continues words)
///   - Space bytes (0x09-0x0D, 0x20): word breaks
///   - Everything else: transparent (does NOT affect word state)
/// Word transitions are detected via bitmask of printable positions.
/// GNU wc C locale: only printable ASCII starts words; high bytes are transparent.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn count_lw_c_chunk_avx2(data: &[u8]) -> (u64, u64, bool, bool) {
    use std::arch::x86_64::*;

    let len = data.len();
    let ptr = data.as_ptr();
    let mut i = 0usize;
    let mut total_lines = 0u64;
    let mut total_words = 0u64;
    // 3-state: 0 = neutral (not in word), 1 = in word, 2 = transparent (no change)
    // We track prev_state as an enum: 0 = not in word, 1 = in word
    let mut prev_in_word = false;

    unsafe {
        let nl_byte = _mm256_set1_epi8(b'\n' as i8);
        let zero = _mm256_setzero_si256();
        let ones = _mm256_set1_epi8(1);
        // Printable ASCII: 0x21-0x7E  (word content)
        let lo_print = _mm256_set1_epi8(0x20i8); // 0x21 - 1
        let hi_print = _mm256_set1_epi8(0x7Fi8); // 0x7E + 1
        // Space detection: 0x09-0x0D and 0x20
        let space_char = _mm256_set1_epi8(0x20i8);
        let tab_lo = _mm256_set1_epi8(0x08i8);
        let tab_hi = _mm256_set1_epi8(0x0Ei8);

        let mut line_acc = _mm256_setzero_si256();
        let mut batch = 0u32;

        while i + 32 <= len {
            let v = _mm256_loadu_si256(ptr.add(i) as *const __m256i);
            let is_nl = _mm256_cmpeq_epi8(v, nl_byte);
            line_acc = _mm256_add_epi8(line_acc, _mm256_and_si256(is_nl, ones));

            // is_printable = (v > 0x20) && (v < 0x7F) — printable ASCII range
            let gt_20 = _mm256_cmpgt_epi8(v, lo_print);
            let lt_7f = _mm256_cmpgt_epi8(hi_print, v);
            let is_printable = _mm256_and_si256(gt_20, lt_7f);

            // is_space = (v == 0x20) | (v > 0x08 && v < 0x0E)
            let is_sp = _mm256_cmpeq_epi8(v, space_char);
            let gt_08 = _mm256_cmpgt_epi8(v, tab_lo);
            let lt_0e = _mm256_cmpgt_epi8(tab_hi, v);
            let is_tab_range = _mm256_and_si256(gt_08, lt_0e);
            let is_space = _mm256_or_si256(is_sp, is_tab_range);

            let print_mask = _mm256_movemask_epi8(is_printable) as u32;
            let space_mask = _mm256_movemask_epi8(is_space) as u32;

            // Fast path: if all bytes are printable or space (no transparent bytes),
            // use the 2-state bitmask approach. This handles pure ASCII text.
            if (print_mask | space_mask) == 0xFFFF_FFFF {
                // 2-state: printable = word content, space = break
                let prev_mask = (print_mask << 1) | (prev_in_word as u32);
                total_words += (print_mask & !prev_mask).count_ones() as u64;
                prev_in_word = (print_mask >> 31) & 1 == 1;
            } else {
                // 3-state: bit-by-bit propagation for transparent bytes
                let mut local_words = 0u32;
                let mut in_word = prev_in_word;
                let mut pm = print_mask;
                let mut sm = space_mask;
                for _ in 0..32 {
                    let p = pm & 1;
                    let s = sm & 1;
                    if s != 0 {
                        in_word = false;
                    } else if p != 0 {
                        if !in_word {
                            local_words += 1;
                            in_word = true;
                        }
                    }
                    pm >>= 1;
                    sm >>= 1;
                }
                total_words += local_words as u64;
                prev_in_word = in_word;
            }

            batch += 1;
            if batch >= 255 {
                let sad = _mm256_sad_epu8(line_acc, zero);
                let hi = _mm256_extracti128_si256(sad, 1);
                let lo = _mm256_castsi256_si128(sad);
                let s = _mm_add_epi64(lo, hi);
                let h64 = _mm_unpackhi_epi64(s, s);
                let t = _mm_add_epi64(s, h64);
                total_lines += _mm_cvtsi128_si64(t) as u64;
                line_acc = _mm256_setzero_si256();
                batch = 0;
            }
            i += 32;
        }

        if batch > 0 {
            let sad = _mm256_sad_epu8(line_acc, zero);
            let hi = _mm256_extracti128_si256(sad, 1);
            let lo = _mm256_castsi256_si128(sad);
            let s = _mm_add_epi64(lo, hi);
            let h64 = _mm_unpackhi_epi64(s, s);
            let t = _mm_add_epi64(s, h64);
            total_lines += _mm_cvtsi128_si64(t) as u64;
        }

        // Scalar tail using 3-state logic
        while i < len {
            let b = *ptr.add(i);
            if b == b'\n' {
                total_lines += 1;
                prev_in_word = false;
            } else if b >= 0x21 && b <= 0x7E {
                // Printable ASCII — word content
                if !prev_in_word {
                    total_words += 1;
                    prev_in_word = true;
                }
            } else if *BYTE_CLASS_C.get_unchecked(b as usize) == 1 {
                // Other space byte
                prev_in_word = false;
            }
            // else: transparent, leave prev_in_word unchanged
            i += 1;
        }
    }

    let first_is_word = !data.is_empty() && BYTE_CLASS_C[data[0] as usize] == 0;
    (total_lines, total_words, first_is_word, prev_in_word)
}

/// SSE2-accelerated fused line+word counter for C locale chunks.
/// Same 3-state algorithm as AVX2 but processes 16 bytes per iteration.
/// Available on all x86_64 CPUs (SSE2 is baseline for x86_64).
/// GNU wc C locale: only printable ASCII (0x21-0x7E) starts words.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn count_lw_c_chunk_sse2(data: &[u8]) -> (u64, u64, bool, bool) {
    use std::arch::x86_64::*;

    let len = data.len();
    let ptr = data.as_ptr();
    let mut i = 0usize;
    let mut total_lines = 0u64;
    let mut total_words = 0u64;
    let mut prev_in_word = false;

    unsafe {
        let nl_byte = _mm_set1_epi8(b'\n' as i8);
        let zero = _mm_setzero_si128();
        let ones = _mm_set1_epi8(1);
        // Printable ASCII: 0x21-0x7E
        let lo_print = _mm_set1_epi8(0x20i8);
        let hi_print = _mm_set1_epi8(0x7Fi8);
        // Space detection: 0x09-0x0D and 0x20
        let space_char = _mm_set1_epi8(0x20i8);
        let tab_lo = _mm_set1_epi8(0x08i8);
        let tab_hi = _mm_set1_epi8(0x0Ei8);

        let mut line_acc = _mm_setzero_si128();
        let mut batch = 0u32;

        while i + 16 <= len {
            let v = _mm_loadu_si128(ptr.add(i) as *const __m128i);
            let is_nl = _mm_cmpeq_epi8(v, nl_byte);
            line_acc = _mm_add_epi8(line_acc, _mm_and_si128(is_nl, ones));

            // is_printable = (v > 0x20) && (v < 0x7F)
            let gt_20 = _mm_cmpgt_epi8(v, lo_print);
            let lt_7f = _mm_cmpgt_epi8(hi_print, v);
            let is_printable = _mm_and_si128(gt_20, lt_7f);

            // is_space = (v == 0x20) | (v > 0x08 && v < 0x0E)
            let is_sp = _mm_cmpeq_epi8(v, space_char);
            let gt_08 = _mm_cmpgt_epi8(v, tab_lo);
            let lt_0e = _mm_cmpgt_epi8(tab_hi, v);
            let is_tab_range = _mm_and_si128(gt_08, lt_0e);
            let is_space = _mm_or_si128(is_sp, is_tab_range);

            let print_mask = _mm_movemask_epi8(is_printable) as u32;
            let space_mask = _mm_movemask_epi8(is_space) as u32;

            // Fast path: if all bytes are printable or space (no transparent bytes),
            // use the 2-state bitmask approach for pure ASCII text.
            if (print_mask | space_mask) == 0xFFFF {
                let prev_mask = (print_mask << 1) | (prev_in_word as u32);
                total_words += (print_mask & !prev_mask).count_ones() as u64;
                prev_in_word = (print_mask >> 15) & 1 == 1;
            } else {
                // 3-state: bit-by-bit propagation for transparent bytes
                let mut local_words = 0u32;
                let mut in_word = prev_in_word;
                let mut pm = print_mask;
                let mut sm = space_mask;
                for _ in 0..16 {
                    let p = pm & 1;
                    let s = sm & 1;
                    if s != 0 {
                        in_word = false;
                    } else if p != 0 {
                        if !in_word {
                            local_words += 1;
                            in_word = true;
                        }
                    }
                    pm >>= 1;
                    sm >>= 1;
                }
                total_words += local_words as u64;
                prev_in_word = in_word;
            }

            batch += 1;
            if batch >= 255 {
                let sad = _mm_sad_epu8(line_acc, zero);
                let hi = _mm_unpackhi_epi64(sad, sad);
                let t = _mm_add_epi64(sad, hi);
                total_lines += _mm_cvtsi128_si64(t) as u64;
                line_acc = _mm_setzero_si128();
                batch = 0;
            }
            i += 16;
        }

        if batch > 0 {
            let sad = _mm_sad_epu8(line_acc, zero);
            let hi = _mm_unpackhi_epi64(sad, sad);
            let t = _mm_add_epi64(sad, hi);
            total_lines += _mm_cvtsi128_si64(t) as u64;
        }

        // Scalar tail using 3-state logic
        while i < len {
            let b = *ptr.add(i);
            if b == b'\n' {
                total_lines += 1;
                prev_in_word = false;
            } else if b >= 0x21 && b <= 0x7E {
                if !prev_in_word {
                    total_words += 1;
                    prev_in_word = true;
                }
            } else if *BYTE_CLASS_C.get_unchecked(b as usize) == 1 {
                prev_in_word = false;
            }
            i += 1;
        }
    }

    let first_is_word = !data.is_empty() && BYTE_CLASS_C[data[0] as usize] == 0;
    (total_lines, total_words, first_is_word, prev_in_word)
}

/// Dispatch to AVX2, SSE2, or scalar chunk counter.
#[inline]
fn count_lw_c_chunk_fast(data: &[u8]) -> (u64, u64, bool, bool) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && data.len() >= 64 {
            return unsafe { count_lw_c_chunk_avx2(data) };
        }
        if data.len() >= 32 {
            return unsafe { count_lw_c_chunk_sse2(data) };
        }
    }
    count_lw_c_chunk(data)
}

/// Count words + lines in a C locale chunk using 3-state logic, returning
/// counts plus boundary info for parallel chunk merging.
/// Returns (line_count, word_count, first_is_printable, ends_in_word).
/// GNU wc C locale: only printable ASCII (0x21-0x7E) starts words;
/// bytes >= 0x80 and control chars are transparent.
fn count_lw_c_chunk(data: &[u8]) -> (u64, u64, bool, bool) {
    let mut lines = 0u64;
    let mut words = 0u64;
    let mut in_word = false;
    let mut first_is_printable = false;
    let mut seen_first_non_transparent = false;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };
        if b >= 0x21 && b <= 0x7E {
            // Printable ASCII — word content
            if !seen_first_non_transparent {
                seen_first_non_transparent = true;
                first_is_printable = true;
            }
            if !in_word {
                in_word = true;
                words += 1;
            }
            i += 1;
            // Skip remaining printable ASCII
            while i < len {
                let b = unsafe { *data.get_unchecked(i) };
                if b >= 0x21 && b <= 0x7E {
                    i += 1;
                } else {
                    break;
                }
            }
        } else {
            let class = unsafe { *BYTE_CLASS_C.get_unchecked(b as usize) };
            if class == 1 {
                // Space byte — break word
                if !seen_first_non_transparent {
                    seen_first_non_transparent = true;
                    // first_is_printable stays false
                }
                if b == b'\n' {
                    lines += 1;
                }
                in_word = false;
            }
            // class == 2: transparent — don't change in_word or seen_first
            i += 1;
        }
    }
    (lines, words, first_is_printable, in_word)
}

/// Count words in UTF-8 locale using a state machine with 3-state logic.
///
/// Handles:
/// - ASCII spaces (0x09-0x0D, 0x20): word break
/// - ASCII printable (0x21-0x7E): word content
/// - ASCII non-printable (0x00-0x08, 0x0E-0x1F, 0x7F): transparent
/// - Valid UTF-8 multi-byte → check Unicode space/printable
/// - Invalid UTF-8: transparent (GNU wc skips invalid bytes without changing state)
///
/// Optimized with ASCII run skipping: when a word starts, skips remaining
/// printable ASCII bytes without per-byte table lookups (~4x fewer state checks
/// for English text with 5-char average word length).
fn count_words_utf8(data: &[u8]) -> u64 {
    let mut words = 0u64;
    let mut in_word = false;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };

        if b >= 0x21 && b <= 0x7E {
            // Printable ASCII (most common case for text) — word content
            if !in_word {
                in_word = true;
                words += 1;
            }
            i += 1;
            // Skip remaining printable ASCII (they don't change state)
            while i < len {
                let b = unsafe { *data.get_unchecked(i) };
                if b >= 0x21 && b <= 0x7E {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if b < 0x80 {
            // Non-printable ASCII: space/tab/newline/controls
            let class = unsafe { *BYTE_CLASS_UTF8.get_unchecked(b as usize) };
            if class == 1 {
                in_word = false;
            }
            // class == 2: transparent (controls 0x00-0x08, 0x0E-0x1F, 0x7F)
            i += 1;
        } else if b < 0xC2 {
            i += 1;
        } else if b < 0xE0 {
            if i + 1 < len && (unsafe { *data.get_unchecked(i + 1) } & 0xC0) == 0x80 {
                let cp = ((b as u32 & 0x1F) << 6)
                    | (unsafe { *data.get_unchecked(i + 1) } as u32 & 0x3F);
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
            if i + 2 < len
                && (unsafe { *data.get_unchecked(i + 1) } & 0xC0) == 0x80
                && (unsafe { *data.get_unchecked(i + 2) } & 0xC0) == 0x80
            {
                let cp = ((b as u32 & 0x0F) << 12)
                    | ((unsafe { *data.get_unchecked(i + 1) } as u32 & 0x3F) << 6)
                    | (unsafe { *data.get_unchecked(i + 2) } as u32 & 0x3F);
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
            if i + 3 < len
                && (unsafe { *data.get_unchecked(i + 1) } & 0xC0) == 0x80
                && (unsafe { *data.get_unchecked(i + 2) } & 0xC0) == 0x80
                && (unsafe { *data.get_unchecked(i + 3) } & 0xC0) == 0x80
            {
                let cp = ((b as u32 & 0x07) << 18)
                    | ((unsafe { *data.get_unchecked(i + 1) } as u32 & 0x3F) << 12)
                    | ((unsafe { *data.get_unchecked(i + 2) } as u32 & 0x3F) << 6)
                    | (unsafe { *data.get_unchecked(i + 3) } as u32 & 0x3F);
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

    words
}

/// Count lines and words using optimized strategies per locale.
/// UTF-8: fused single-pass for lines+words to avoid extra data traversal.
/// C locale: AVX2 SIMD fused counter when available, scalar fallback otherwise.
pub fn count_lines_words(data: &[u8], utf8: bool) -> (u64, u64) {
    if utf8 {
        count_lines_words_utf8_fused(data)
    } else {
        let (lines, words, _, _) = count_lw_c_chunk_fast(data);
        (lines, words)
    }
}

/// Fused lines+words counting in UTF-8 mode (single pass).
/// Avoids separate memchr pass for newlines by counting them inline with words.
///
/// Key optimization: ASCII run skipping. Once a word starts (printable ASCII byte),
/// we skip remaining printable ASCII bytes without any per-byte state checks.
/// For English text (avg word ~5 chars), this reduces state transitions by ~4x.
fn count_lines_words_utf8_fused(data: &[u8]) -> (u64, u64) {
    let mut lines = 0u64;
    let mut words = 0u64;
    let mut in_word = false;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };

        if b >= 0x21 && b <= 0x7E {
            // Printable ASCII (most common) — word content
            if !in_word {
                in_word = true;
                words += 1;
            }
            i += 1;
            // Skip remaining printable ASCII (they don't change state or count lines)
            while i < len {
                let b = unsafe { *data.get_unchecked(i) };
                if b >= 0x21 && b <= 0x7E {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if b == b'\n' {
            lines += 1;
            in_word = false;
            i += 1;
        } else if b == b' ' {
            in_word = false;
            i += 1;
        } else if b < 0x80 {
            // Other ASCII: \t, \r, \v, \f, controls
            let class = unsafe { *BYTE_CLASS_UTF8.get_unchecked(b as usize) };
            if class == 1 {
                in_word = false;
            }
            // class == 2: transparent
            i += 1;
        } else if b < 0xC2 {
            i += 1;
        } else if b < 0xE0 {
            if i + 1 < len && (unsafe { *data.get_unchecked(i + 1) } & 0xC0) == 0x80 {
                let cp = ((b as u32 & 0x1F) << 6)
                    | (unsafe { *data.get_unchecked(i + 1) } as u32 & 0x3F);
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
            if i + 2 < len
                && (unsafe { *data.get_unchecked(i + 1) } & 0xC0) == 0x80
                && (unsafe { *data.get_unchecked(i + 2) } & 0xC0) == 0x80
            {
                let cp = ((b as u32 & 0x0F) << 12)
                    | ((unsafe { *data.get_unchecked(i + 1) } as u32 & 0x3F) << 6)
                    | (unsafe { *data.get_unchecked(i + 2) } as u32 & 0x3F);
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
            if i + 3 < len
                && (unsafe { *data.get_unchecked(i + 1) } & 0xC0) == 0x80
                && (unsafe { *data.get_unchecked(i + 2) } & 0xC0) == 0x80
                && (unsafe { *data.get_unchecked(i + 3) } & 0xC0) == 0x80
            {
                let cp = ((b as u32 & 0x07) << 18)
                    | ((unsafe { *data.get_unchecked(i + 1) } as u32 & 0x3F) << 12)
                    | ((unsafe { *data.get_unchecked(i + 2) } as u32 & 0x3F) << 6)
                    | (unsafe { *data.get_unchecked(i + 3) } as u32 & 0x3F);
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
        // C locale: use optimized fused lines+words, chars = byte count
        let (lines, words) = count_lines_words(data, false);
        (lines, words, data.len() as u64)
    }
}

/// Count UTF-8 characters by counting non-continuation bytes.
/// A continuation byte has the bit pattern `10xxxxxx` (0x80..0xBF).
/// Every other byte starts a new character (ASCII, multi-byte leader, or invalid).
///
/// Uses AVX2 SIMD on x86_64 for ~32 bytes per cycle throughput.
/// Falls back to 64-byte block processing with popcount on other architectures.
pub fn count_chars_utf8(data: &[u8]) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { count_chars_utf8_avx2(data) };
        }
    }
    count_chars_utf8_scalar(data)
}

/// AVX2 SIMD character counter: counts non-continuation bytes using
/// vectorized AND+CMP with batched horizontal reduction via PSADBW.
/// Processes 32 bytes per ~3 instructions, with horizontal sum every 255 iterations.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn count_chars_utf8_avx2(data: &[u8]) -> u64 {
    unsafe {
        use std::arch::x86_64::*;

        let mask_c0 = _mm256_set1_epi8(0xC0u8 as i8);
        let val_80 = _mm256_set1_epi8(0x80u8 as i8);
        let ones = _mm256_set1_epi8(1);
        let zero = _mm256_setzero_si256();

        let mut total = 0u64;
        let len = data.len();
        let ptr = data.as_ptr();
        let mut i = 0;
        let mut acc = _mm256_setzero_si256();
        let mut batch = 0u32;

        while i + 32 <= len {
            let v = _mm256_loadu_si256(ptr.add(i) as *const __m256i);
            let masked = _mm256_and_si256(v, mask_c0);
            let is_cont = _mm256_cmpeq_epi8(masked, val_80);
            let non_cont = _mm256_andnot_si256(is_cont, ones);
            acc = _mm256_add_epi8(acc, non_cont);

            batch += 1;
            if batch >= 255 {
                // Horizontal sum via PSADBW: sum u8 differences against zero
                let sad = _mm256_sad_epu8(acc, zero);
                let hi = _mm256_extracti128_si256(sad, 1);
                let lo = _mm256_castsi256_si128(sad);
                let sum = _mm_add_epi64(lo, hi);
                let hi64 = _mm_unpackhi_epi64(sum, sum);
                let t = _mm_add_epi64(sum, hi64);
                total += _mm_cvtsi128_si64(t) as u64;
                acc = _mm256_setzero_si256();
                batch = 0;
            }
            i += 32;
        }

        // Final horizontal sum
        if batch > 0 {
            let sad = _mm256_sad_epu8(acc, zero);
            let hi = _mm256_extracti128_si256(sad, 1);
            let lo = _mm256_castsi256_si128(sad);
            let sum = _mm_add_epi64(lo, hi);
            let hi64 = _mm_unpackhi_epi64(sum, sum);
            let t = _mm_add_epi64(sum, hi64);
            total += _mm_cvtsi128_si64(t) as u64;
        }

        while i < len {
            total += ((*ptr.add(i) & 0xC0) != 0x80) as u64;
            i += 1;
        }

        total
    }
}

/// Scalar fallback for count_chars_utf8.
fn count_chars_utf8_scalar(data: &[u8]) -> u64 {
    let mut count = 0u64;
    let chunks = data.chunks_exact(64);
    let remainder = chunks.remainder();

    for chunk in chunks {
        // Fast path: if all bytes are ASCII (< 0x80), every byte is a character
        let mut any_high = 0u8;
        let mut i = 0;
        while i + 8 <= 64 {
            unsafe {
                any_high |= *chunk.get_unchecked(i);
                any_high |= *chunk.get_unchecked(i + 1);
                any_high |= *chunk.get_unchecked(i + 2);
                any_high |= *chunk.get_unchecked(i + 3);
                any_high |= *chunk.get_unchecked(i + 4);
                any_high |= *chunk.get_unchecked(i + 5);
                any_high |= *chunk.get_unchecked(i + 6);
                any_high |= *chunk.get_unchecked(i + 7);
            }
            i += 8;
        }
        if any_high < 0x80 {
            count += 64;
            continue;
        }

        let mut char_mask = 0u64;
        i = 0;
        while i + 7 < 64 {
            unsafe {
                char_mask |= (((*chunk.get_unchecked(i) & 0xC0) != 0x80) as u64) << i;
                char_mask |= (((*chunk.get_unchecked(i + 1) & 0xC0) != 0x80) as u64) << (i + 1);
                char_mask |= (((*chunk.get_unchecked(i + 2) & 0xC0) != 0x80) as u64) << (i + 2);
                char_mask |= (((*chunk.get_unchecked(i + 3) & 0xC0) != 0x80) as u64) << (i + 3);
                char_mask |= (((*chunk.get_unchecked(i + 4) & 0xC0) != 0x80) as u64) << (i + 4);
                char_mask |= (((*chunk.get_unchecked(i + 5) & 0xC0) != 0x80) as u64) << (i + 5);
                char_mask |= (((*chunk.get_unchecked(i + 6) & 0xC0) != 0x80) as u64) << (i + 6);
                char_mask |= (((*chunk.get_unchecked(i + 7) & 0xC0) != 0x80) as u64) << (i + 7);
            }
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

/// Check if a Unicode codepoint is a zero-width character (combining mark, etc.).
/// GNU wc uses wcwidth() which returns 0 for these. We must match.
#[inline]
fn is_zero_width(cp: u32) -> bool {
    matches!(
        cp,
        0x0300..=0x036F   // Combining Diacritical Marks
        | 0x0483..=0x0489 // Cyrillic combining marks
        | 0x0591..=0x05BD // Hebrew combining marks
        | 0x05BF
        | 0x05C1..=0x05C2
        | 0x05C4..=0x05C5
        | 0x05C7
        | 0x0600..=0x0605 // Arabic number signs
        | 0x0610..=0x061A // Arabic combining marks
        | 0x064B..=0x065F // Arabic combining marks
        | 0x0670
        | 0x06D6..=0x06DD
        | 0x06DF..=0x06E4
        | 0x06E7..=0x06E8
        | 0x06EA..=0x06ED
        | 0x070F
        | 0x0711
        | 0x0730..=0x074A
        | 0x07A6..=0x07B0
        | 0x07EB..=0x07F3
        | 0x07FD
        | 0x0816..=0x0819
        | 0x081B..=0x0823
        | 0x0825..=0x0827
        | 0x0829..=0x082D
        | 0x0859..=0x085B
        | 0x08D3..=0x08E1
        | 0x08E3..=0x0902
        | 0x093A
        | 0x093C
        | 0x0941..=0x0948
        | 0x094D
        | 0x0951..=0x0957
        | 0x0962..=0x0963
        | 0x0981
        | 0x09BC
        | 0x09C1..=0x09C4
        | 0x09CD
        | 0x09E2..=0x09E3
        | 0x09FE
        | 0x0A01..=0x0A02
        | 0x0A3C
        | 0x0A41..=0x0A42
        | 0x0A47..=0x0A48
        | 0x0A4B..=0x0A4D
        | 0x0A51
        | 0x0A70..=0x0A71
        | 0x0A75
        | 0x0A81..=0x0A82
        | 0x0ABC
        | 0x0AC1..=0x0AC5
        | 0x0AC7..=0x0AC8
        | 0x0ACD
        | 0x0AE2..=0x0AE3
        | 0x0AFA..=0x0AFF
        | 0x0B01
        | 0x0B3C
        | 0x0B3F
        | 0x0B41..=0x0B44
        | 0x0B4D
        | 0x0B56
        | 0x0B62..=0x0B63
        | 0x0B82
        | 0x0BC0
        | 0x0BCD
        | 0x0C00
        | 0x0C04
        | 0x0C3E..=0x0C40
        | 0x0C46..=0x0C48
        | 0x0C4A..=0x0C4D
        | 0x0C55..=0x0C56
        | 0x0C62..=0x0C63
        | 0x0C81
        | 0x0CBC
        | 0x0CBF
        | 0x0CC6
        | 0x0CCC..=0x0CCD
        | 0x0CE2..=0x0CE3
        | 0x0D00..=0x0D01
        | 0x0D3B..=0x0D3C
        | 0x0D41..=0x0D44
        | 0x0D4D
        | 0x0D62..=0x0D63
        | 0x0DCA
        | 0x0DD2..=0x0DD4
        | 0x0DD6
        | 0x0E31
        | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E
        | 0x0EB1
        | 0x0EB4..=0x0EBC
        | 0x0EC8..=0x0ECD
        | 0x0F18..=0x0F19
        | 0x0F35
        | 0x0F37
        | 0x0F39
        | 0x0F71..=0x0F7E
        | 0x0F80..=0x0F84
        | 0x0F86..=0x0F87
        | 0x0F8D..=0x0F97
        | 0x0F99..=0x0FBC
        | 0x0FC6
        | 0x102D..=0x1030
        | 0x1032..=0x1037
        | 0x1039..=0x103A
        | 0x103D..=0x103E
        | 0x1058..=0x1059
        | 0x105E..=0x1060
        | 0x1071..=0x1074
        | 0x1082
        | 0x1085..=0x1086
        | 0x108D
        | 0x109D
        | 0x1160..=0x11FF // Hangul Jamo medial vowels and final consonants
        | 0x135D..=0x135F
        | 0x1712..=0x1714
        | 0x1732..=0x1734
        | 0x1752..=0x1753
        | 0x1772..=0x1773
        | 0x17B4..=0x17B5
        | 0x17B7..=0x17BD
        | 0x17C6
        | 0x17C9..=0x17D3
        | 0x17DD
        | 0x180B..=0x180D
        | 0x1885..=0x1886
        | 0x18A9
        | 0x1920..=0x1922
        | 0x1927..=0x1928
        | 0x1932
        | 0x1939..=0x193B
        | 0x1A17..=0x1A18
        | 0x1A1B
        | 0x1A56
        | 0x1A58..=0x1A5E
        | 0x1A60
        | 0x1A62
        | 0x1A65..=0x1A6C
        | 0x1A73..=0x1A7C
        | 0x1A7F
        | 0x1AB0..=0x1ABE
        | 0x1B00..=0x1B03
        | 0x1B34
        | 0x1B36..=0x1B3A
        | 0x1B3C
        | 0x1B42
        | 0x1B6B..=0x1B73
        | 0x1B80..=0x1B81
        | 0x1BA2..=0x1BA5
        | 0x1BA8..=0x1BA9
        | 0x1BAB..=0x1BAD
        | 0x1BE6
        | 0x1BE8..=0x1BE9
        | 0x1BED
        | 0x1BEF..=0x1BF1
        | 0x1C2C..=0x1C33
        | 0x1C36..=0x1C37
        | 0x1CD0..=0x1CD2
        | 0x1CD4..=0x1CE0
        | 0x1CE2..=0x1CE8
        | 0x1CED
        | 0x1CF4
        | 0x1CF8..=0x1CF9
        | 0x1DC0..=0x1DF9
        | 0x1DFB..=0x1DFF
        | 0x200B..=0x200F // Zero-width space, ZWNJ, ZWJ, LRM, RLM
        | 0x202A..=0x202E // Bidi control chars
        | 0x2060..=0x2064 // Word joiner, invisible operators
        | 0x2066..=0x206F // Bidi isolates
        | 0x20D0..=0x20F0 // Combining marks for symbols
        | 0xFE00..=0xFE0F // Variation Selectors
        | 0xFE20..=0xFE2F // Combining Half Marks
        | 0xFEFF          // Zero Width No-Break Space (BOM)
        | 0xFFF9..=0xFFFB // Interlinear annotation anchors
        | 0x1D167..=0x1D169
        | 0x1D173..=0x1D182
        | 0x1D185..=0x1D18B
        | 0x1D1AA..=0x1D1AD
        | 0x1D242..=0x1D244
        | 0xE0001
        | 0xE0020..=0xE007F
        | 0xE0100..=0xE01EF // Variation Selectors Supplement
    )
}

/// Check if a Unicode codepoint is an East Asian Wide/Fullwidth character (display width 2).
/// Matches glibc wcwidth() behavior for maximum GNU compatibility.
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
        | 0x3040..=0x33BF  // Hiragana, Katakana, Bopomofo, Hangul Compat Jamo, Kanbun, CJK
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
///
/// Optimized with printable ASCII run counting: for runs of bytes in
/// 0x21-0x7E (no space/tab/newline), counts the entire run length at once.
pub fn max_line_length_c(data: &[u8]) -> u64 {
    let mut max_len: u64 = 0;
    let mut line_len: u64 = 0;
    let mut linepos: u64 = 0;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };
        if b >= 0x21 && b <= 0x7E {
            // Printable non-space ASCII — count run length
            i += 1;
            let mut run = 1u64;
            while i < len {
                let b = unsafe { *data.get_unchecked(i) };
                if b >= 0x21 && b <= 0x7E {
                    run += 1;
                    i += 1;
                } else {
                    break;
                }
            }
            linepos += run;
            if linepos > line_len {
                line_len = linepos;
            }
        } else {
            match b {
                b' ' => {
                    linepos += 1;
                    if linepos > line_len {
                        line_len = linepos;
                    }
                }
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
                    if line_len > max_len {
                        max_len = line_len;
                    }
                    linepos = 0;
                    line_len = 0;
                }
                _ => {} // Non-printable: width 0
            }
            i += 1;
        }
    }

    if line_len > max_len {
        max_len = line_len;
    }

    max_len
}

/// Compute maximum display width of any line (UTF-8 locale).
///
/// GNU wc -L in UTF-8 locale uses mbrtowc() + wcwidth() for display width.
/// East Asian Wide/Fullwidth characters get width 2, most others get width 1.
///
/// Optimized with printable ASCII run counting for common text.
pub fn max_line_length_utf8(data: &[u8]) -> u64 {
    let mut max_len: u64 = 0;
    let mut line_len: u64 = 0;
    let mut linepos: u64 = 0;
    let mut i = 0;
    let len = data.len();

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };

        if b >= 0x21 && b <= 0x7E {
            // Printable non-space ASCII (most common) — count run length
            i += 1;
            let mut run = 1u64;
            while i < len {
                let b = unsafe { *data.get_unchecked(i) };
                if b >= 0x21 && b <= 0x7E {
                    run += 1;
                    i += 1;
                } else {
                    break;
                }
            }
            linepos += run;
            if linepos > line_len {
                line_len = linepos;
            }
        } else if b < 0x80 {
            // Other ASCII: space, tab, newline, controls
            match b {
                b' ' => {
                    linepos += 1;
                    if linepos > line_len {
                        line_len = linepos;
                    }
                }
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
                    if line_len > max_len {
                        max_len = line_len;
                    }
                    linepos = 0;
                    line_len = 0;
                }
                _ => {} // Non-printable: width 0
            }
            i += 1;
        } else {
            // Multibyte UTF-8
            let (cp, len) = decode_utf8(&data[i..]);

            // C1 control characters (0x80..0x9F): non-printable, width 0
            if cp <= 0x9F {
                // width 0
            } else if is_zero_width(cp) {
                // Combining marks, zero-width chars: width 0
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

/// Quick check if data is likely all-ASCII by sampling three regions.
/// Checks first 256 bytes, middle 256 bytes, and last 256 bytes.
/// If any byte >= 0x80 is found, returns false.
#[inline]
fn check_ascii_sample(data: &[u8]) -> bool {
    let len = data.len();
    if len == 0 {
        return true;
    }

    // Check in 8-byte blocks using OR-accumulation for speed
    let check_region = |start: usize, end: usize| -> bool {
        let mut or_acc = 0u8;
        let region = &data[start..end];
        let mut i = 0;
        while i + 8 <= region.len() {
            unsafe {
                or_acc |= *region.get_unchecked(i);
                or_acc |= *region.get_unchecked(i + 1);
                or_acc |= *region.get_unchecked(i + 2);
                or_acc |= *region.get_unchecked(i + 3);
                or_acc |= *region.get_unchecked(i + 4);
                or_acc |= *region.get_unchecked(i + 5);
                or_acc |= *region.get_unchecked(i + 6);
                or_acc |= *region.get_unchecked(i + 7);
            }
            i += 8;
        }
        while i < region.len() {
            or_acc |= region[i];
            i += 1;
        }
        or_acc < 0x80
    };

    let sample = 256.min(len);

    // Check beginning
    if !check_region(0, sample) {
        return false;
    }
    // Check middle
    if len > sample * 2 {
        let mid = len / 2;
        let mid_start = mid.saturating_sub(sample / 2);
        if !check_region(mid_start, (mid_start + sample).min(len)) {
            return false;
        }
    }
    // Check end
    if len > sample {
        if !check_region(len - sample, len) {
            return false;
        }
    }

    true
}

// ──────────────────────────────────────────────────
// Parallel counting for large files
// ──────────────────────────────────────────────────

/// Split data into chunks at newline boundaries for parallel processing.
/// Returns slices where each slice (except possibly the last) ends with `\n`.
/// Splitting at newlines guarantees word boundaries in any locale,
/// enabling safe parallel word counting without boundary adjustment.
fn split_at_newlines(data: &[u8], num_chunks: usize) -> Vec<&[u8]> {
    if data.is_empty() || num_chunks <= 1 {
        return vec![data];
    }
    let chunk_size = data.len() / num_chunks;
    let mut chunks = Vec::with_capacity(num_chunks);
    let mut pos = 0;

    for _ in 0..num_chunks - 1 {
        let target = pos + chunk_size;
        if target >= data.len() {
            break;
        }
        let boundary = memchr::memchr(b'\n', &data[target..])
            .map(|p| target + p + 1)
            .unwrap_or(data.len());
        if boundary > pos {
            chunks.push(&data[pos..boundary]);
        }
        pos = boundary;
    }
    if pos < data.len() {
        chunks.push(&data[pos..]);
    }
    chunks
}

/// Count newlines in parallel using SIMD memchr + rayon.
/// Each thread gets at least 1MB (to amortize rayon scheduling overhead).
pub fn count_lines_parallel(data: &[u8]) -> u64 {
    if data.len() < PARALLEL_THRESHOLD {
        return count_lines(data);
    }

    let num_threads = rayon::current_num_threads().max(1);
    // Ensure chunks are large enough to amortize SIMD setup overhead
    let chunk_size = (data.len() / num_threads).max(2 * 1024 * 1024);

    data.par_chunks(chunk_size)
        .map(|chunk| memchr_iter(b'\n', chunk).count() as u64)
        .sum()
}

/// Count words in parallel with boundary adjustment.
pub fn count_words_parallel(data: &[u8], utf8: bool) -> u64 {
    if data.len() < PARALLEL_THRESHOLD {
        return count_words_locale(data, utf8);
    }

    let num_threads = rayon::current_num_threads().max(1);

    if utf8 {
        // UTF-8: split at newline boundaries for safe parallel word counting.
        // Newlines are always word boundaries, so no boundary adjustment needed.
        let chunks = split_at_newlines(data, num_threads);
        chunks.par_iter().map(|chunk| count_words_utf8(chunk)).sum()
    } else {
        // C locale: parallel 3-state word counting with boundary adjustment
        let chunk_size = (data.len() / num_threads).max(1024 * 1024);

        let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();

        // Each chunk returns (lines, word_count, first_active_is_printable, ends_in_word)
        let results: Vec<(u64, u64, bool, bool)> = chunks
            .par_iter()
            .map(|chunk| count_lw_c_chunk(chunk))
            .collect();

        let mut total = 0u64;
        for i in 0..results.len() {
            total += results[i].1;
            // Boundary adjustment: if previous chunk ended in_word AND
            // current chunk's first non-transparent byte is printable,
            // the word was split across chunks — subtract the overcount.
            if i > 0 && results[i - 1].3 && results[i].2 {
                total -= 1;
            }
        }
        total
    }
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
/// C locale: single fused pass per chunk counts BOTH lines and words.
/// UTF-8: checks ASCII first for C locale fast path, else splits at newlines
/// for safe parallel UTF-8 word counting.
pub fn count_lwb_parallel(data: &[u8], utf8: bool) -> (u64, u64, u64) {
    if data.len() < PARALLEL_THRESHOLD {
        // Small file: use fused single-pass
        return count_lwb(data, utf8);
    }

    let num_threads = rayon::current_num_threads().max(1);

    let (lines, words) = if !utf8 {
        // C locale: FUSED parallel lines+words counting — single pass per chunk
        let chunk_size = (data.len() / num_threads).max(1024 * 1024);

        let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();
        let results: Vec<(u64, u64, bool, bool)> = chunks
            .par_iter()
            .map(|chunk| count_lw_c_chunk_fast(chunk))
            .collect();

        let mut line_total = 0u64;
        let mut word_total = 0u64;
        for i in 0..results.len() {
            line_total += results[i].0;
            word_total += results[i].1;
            if i > 0 && results[i - 1].3 && results[i].2 {
                word_total -= 1;
            }
        }

        (line_total, word_total)
    } else {
        // UTF-8 locale: check if ASCII for faster C locale path
        let is_ascii = check_ascii_sample(data);
        if is_ascii {
            // Pure ASCII: use C locale parallel path (arbitrary chunks OK)
            let chunk_size = (data.len() / num_threads).max(1024 * 1024);
            let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();
            let results: Vec<(u64, u64, bool, bool)> = chunks
                .par_iter()
                .map(|chunk| count_lw_c_chunk_fast(chunk))
                .collect();

            let mut line_total = 0u64;
            let mut word_total = 0u64;
            for i in 0..results.len() {
                line_total += results[i].0;
                word_total += results[i].1;
                if i > 0 && results[i - 1].3 && results[i].2 {
                    word_total -= 1;
                }
            }
            (line_total, word_total)
        } else {
            // Non-ASCII UTF-8: split at newline boundaries for safe parallel
            // word counting. Newlines always break words, so no adjustment needed.
            let chunks = split_at_newlines(data, num_threads);
            let results: Vec<(u64, u64)> = chunks
                .par_iter()
                .map(|chunk| count_lines_words_utf8_fused(chunk))
                .collect();
            let mut line_total = 0u64;
            let mut word_total = 0u64;
            for (l, w) in results {
                line_total += l;
                word_total += w;
            }
            (line_total, word_total)
        }
    };

    (lines, words, data.len() as u64)
}

/// Combined parallel counting of lines + words + chars.
/// UTF-8: splits at newline boundaries for fused lines+words+chars per chunk.
/// C locale: fused parallel lines+words with boundary adjustment + parallel chars.
pub fn count_lwc_parallel(data: &[u8], utf8: bool) -> (u64, u64, u64) {
    if data.len() < PARALLEL_THRESHOLD {
        let lines = count_lines(data);
        let words = count_words_locale(data, utf8);
        let chars = count_chars(data, utf8);
        return (lines, words, chars);
    }

    let num_threads = rayon::current_num_threads().max(1);

    if utf8 {
        // UTF-8: fused parallel lines+words+chars per chunk (split at newlines)
        let chunks = split_at_newlines(data, num_threads);
        let results: Vec<(u64, u64, u64)> = chunks
            .par_iter()
            .map(|chunk| {
                let (lines, words) = count_lines_words_utf8_fused(chunk);
                let chars = count_chars_utf8(chunk);
                (lines, words, chars)
            })
            .collect();
        let mut lines = 0u64;
        let mut words = 0u64;
        let mut chars = 0u64;
        for (l, w, c) in results {
            lines += l;
            words += w;
            chars += c;
        }
        (lines, words, chars)
    } else {
        // C locale: fused parallel lines+words + parallel chars (= byte count)
        let chunk_size = (data.len() / num_threads).max(1024 * 1024);
        let chunks: Vec<&[u8]> = data.chunks(chunk_size).collect();
        let results: Vec<(u64, u64, bool, bool)> = chunks
            .par_iter()
            .map(|chunk| count_lw_c_chunk_fast(chunk))
            .collect();
        let mut lines = 0u64;
        let mut words = 0u64;
        for i in 0..results.len() {
            lines += results[i].0;
            words += results[i].1;
            if i > 0 && results[i - 1].3 && results[i].2 {
                words -= 1;
            }
        }
        (lines, words, data.len() as u64)
    }
}

/// Parallel max line length computation.
/// Splits at newline boundaries so each chunk independently computes correct
/// max line width (since newlines reset position tracking).
pub fn max_line_length_parallel(data: &[u8], utf8: bool) -> u64 {
    if data.len() < PARALLEL_THRESHOLD {
        return max_line_length(data, utf8);
    }
    let num_threads = rayon::current_num_threads().max(1);
    let chunks = split_at_newlines(data, num_threads);
    chunks
        .par_iter()
        .map(|chunk| {
            if utf8 {
                max_line_length_utf8(chunk)
            } else {
                max_line_length_c(chunk)
            }
        })
        .max()
        .unwrap_or(0)
}

/// Parallel counting of all metrics at once.
/// Splits at newline boundaries for safe parallel word + max_line_length counting.
/// Each chunk computes all metrics in a single traversal group, maximizing cache reuse.
pub fn count_all_parallel(data: &[u8], utf8: bool) -> WcCounts {
    if data.len() < PARALLEL_THRESHOLD {
        return count_all(data, utf8);
    }

    let num_threads = rayon::current_num_threads().max(1);
    let chunks = split_at_newlines(data, num_threads);

    if utf8 {
        let results: Vec<(u64, u64, u64, u64)> = chunks
            .par_iter()
            .map(|chunk| {
                let (lines, words) = count_lines_words_utf8_fused(chunk);
                let chars = count_chars_utf8(chunk);
                let max_ll = max_line_length_utf8(chunk);
                (lines, words, chars, max_ll)
            })
            .collect();

        let mut counts = WcCounts {
            bytes: data.len() as u64,
            ..Default::default()
        };
        for (l, w, c, m) in results {
            counts.lines += l;
            counts.words += w;
            counts.chars += c;
            if m > counts.max_line_length {
                counts.max_line_length = m;
            }
        }
        counts
    } else {
        // C locale: fused lines+words per chunk + max_line_length per chunk
        let results: Vec<(u64, u64, u64)> = chunks
            .par_iter()
            .map(|chunk| {
                let (lines, words) = count_lines_words(chunk, false);
                let max_ll = max_line_length_c(chunk);
                (lines, words, max_ll)
            })
            .collect();

        let mut counts = WcCounts {
            bytes: data.len() as u64,
            chars: data.len() as u64,
            ..Default::default()
        };
        for (l, w, m) in &results {
            counts.lines += l;
            counts.words += w;
            if *m > counts.max_line_length {
                counts.max_line_length = *m;
            }
        }
        counts
    }
}
