use std::io::{self, Read, Write};

use rayon::prelude::*;

const BUF_SIZE: usize = 1024 * 1024; // 1MB — fits L2/L3 cache for locality

/// Stream buffer: 256KB — process data immediately after each read().
/// Stays in L2 cache, matches typical kernel pipe buffer size.
/// Unlike fill_buf (which loops to accumulate 8MB = ~128 syscalls for pipes),
/// we read once and process immediately, matching GNU tr's approach.
const STREAM_BUF: usize = 256 * 1024;

/// Minimum size for parallel translation.
/// Lowered to 4MB since rayon overhead is small (~10μs) compared to
/// translation time per chunk (~400μs at 10GB/s).
const PARALLEL_TRANSLATE_THRESHOLD: usize = 4 * 1024 * 1024;

/// Build a 256-byte lookup table mapping set1[i] -> set2[i].
#[inline]
fn build_translate_table(set1: &[u8], set2: &[u8]) -> [u8; 256] {
    let mut table: [u8; 256] = std::array::from_fn(|i| i as u8);
    let last = set2.last().copied();
    for (i, &from) in set1.iter().enumerate() {
        table[from as usize] = if i < set2.len() {
            set2[i]
        } else {
            last.unwrap_or(from)
        };
    }
    table
}

/// Build a 256-bit (32-byte) membership set for O(1) byte lookup.
#[inline]
fn build_member_set(chars: &[u8]) -> [u8; 32] {
    let mut set = [0u8; 32];
    for &ch in chars {
        set[ch as usize >> 3] |= 1 << (ch & 7);
    }
    set
}

#[inline(always)]
fn is_member(set: &[u8; 32], ch: u8) -> bool {
    unsafe { (*set.get_unchecked(ch as usize >> 3) & (1 << (ch & 7))) != 0 }
}

// ============================================================================
// Table analysis for SIMD fast paths
// ============================================================================

/// Classification of a translation table for SIMD optimization.
#[allow(dead_code)] // Fields used only on x86_64 (AVX2 SIMD path)
enum TranslateKind {
    /// Table is identity — no translation needed.
    Identity,
    /// A contiguous range [lo, hi] with constant wrapping-add delta; identity elsewhere.
    RangeDelta { lo: u8, hi: u8, delta: u8 },
    /// Arbitrary translation — use general lookup table.
    General,
}

/// Analyze a translation table to detect SIMD-optimizable patterns.
fn analyze_table(table: &[u8; 256]) -> TranslateKind {
    let mut first_delta: Option<u8> = None;
    let mut lo: u8 = 0;
    let mut hi: u8 = 0;
    let mut count: u32 = 0;

    for i in 0..256u16 {
        let actual = table[i as usize];
        if actual != i as u8 {
            let delta = actual.wrapping_sub(i as u8);
            count += 1;
            match first_delta {
                None => {
                    first_delta = Some(delta);
                    lo = i as u8;
                    hi = i as u8;
                }
                Some(d) if d == delta => {
                    hi = i as u8;
                }
                _ => return TranslateKind::General,
            }
        }
    }

    match (count, first_delta) {
        (0, _) => TranslateKind::Identity,
        (c, Some(delta)) if c == (hi as u32 - lo as u32 + 1) => {
            TranslateKind::RangeDelta { lo, hi, delta }
        }
        _ => TranslateKind::General,
    }
}

// ============================================================================
// SIMD translation (x86_64 AVX2)
// ============================================================================

#[cfg(target_arch = "x86_64")]
mod simd_tr {
    /// Translate bytes in range [lo, hi] by adding `delta` (wrapping), leave others unchanged.
    /// Processes 128 bytes per iteration (4x unroll) using AVX2.
    ///
    /// SAFETY: Caller must ensure AVX2 is available and out.len() >= data.len().
    #[target_feature(enable = "avx2")]
    pub unsafe fn range_delta(data: &[u8], out: &mut [u8], lo: u8, hi: u8, delta: u8) {
        unsafe {
            use std::arch::x86_64::*;

            let lo_vec = _mm256_set1_epi8(lo as i8);
            let range_vec = _mm256_set1_epi8((hi - lo) as i8);
            let delta_vec = _mm256_set1_epi8(delta as i8);

            let len = data.len();
            let inp = data.as_ptr();
            let outp = out.as_mut_ptr();
            let mut i = 0usize;

            // 4x unrolled: process 128 bytes per iteration for better ILP
            while i + 128 <= len {
                let v0 = _mm256_loadu_si256(inp.add(i) as *const __m256i);
                let v1 = _mm256_loadu_si256(inp.add(i + 32) as *const __m256i);
                let v2 = _mm256_loadu_si256(inp.add(i + 64) as *const __m256i);
                let v3 = _mm256_loadu_si256(inp.add(i + 96) as *const __m256i);

                let d0 = _mm256_sub_epi8(v0, lo_vec);
                let d1 = _mm256_sub_epi8(v1, lo_vec);
                let d2 = _mm256_sub_epi8(v2, lo_vec);
                let d3 = _mm256_sub_epi8(v3, lo_vec);

                let m0 = _mm256_cmpeq_epi8(_mm256_min_epu8(d0, range_vec), d0);
                let m1 = _mm256_cmpeq_epi8(_mm256_min_epu8(d1, range_vec), d1);
                let m2 = _mm256_cmpeq_epi8(_mm256_min_epu8(d2, range_vec), d2);
                let m3 = _mm256_cmpeq_epi8(_mm256_min_epu8(d3, range_vec), d3);

                let r0 = _mm256_add_epi8(v0, _mm256_and_si256(m0, delta_vec));
                let r1 = _mm256_add_epi8(v1, _mm256_and_si256(m1, delta_vec));
                let r2 = _mm256_add_epi8(v2, _mm256_and_si256(m2, delta_vec));
                let r3 = _mm256_add_epi8(v3, _mm256_and_si256(m3, delta_vec));

                _mm256_storeu_si256(outp.add(i) as *mut __m256i, r0);
                _mm256_storeu_si256(outp.add(i + 32) as *mut __m256i, r1);
                _mm256_storeu_si256(outp.add(i + 64) as *mut __m256i, r2);
                _mm256_storeu_si256(outp.add(i + 96) as *mut __m256i, r3);
                i += 128;
            }

            while i + 32 <= len {
                let v = _mm256_loadu_si256(inp.add(i) as *const __m256i);
                let diff = _mm256_sub_epi8(v, lo_vec);
                let mask = _mm256_cmpeq_epi8(_mm256_min_epu8(diff, range_vec), diff);
                let result = _mm256_add_epi8(v, _mm256_and_si256(mask, delta_vec));
                _mm256_storeu_si256(outp.add(i) as *mut __m256i, result);
                i += 32;
            }

            while i < len {
                let b = *inp.add(i);
                *outp.add(i) = if b.wrapping_sub(lo) <= (hi - lo) {
                    b.wrapping_add(delta)
                } else {
                    b
                };
                i += 1;
            }
        }
    }

    /// General 256-byte table lookup using 16-way vpshufb (nibble decomposition).
    /// Splits each input byte into high nibble (selects one of 16 shuffle tables)
    /// and low nibble (index within the shuffle table). Processes 64 bytes per
    /// iteration (2x unrolled) for instruction-level parallelism.
    ///
    /// SAFETY: Caller must ensure AVX2 is available and out.len() >= data.len().
    #[target_feature(enable = "avx2")]
    pub unsafe fn general_lookup(data: &[u8], out: &mut [u8], table: &[u8; 256]) {
        unsafe {
            use std::arch::x86_64::*;

            // Build 16 vpshufb LUTs from the 256-byte translation table.
            // LUT[h] covers table[h*16..h*16+16], broadcast to both 128-bit lanes.
            let tp = table.as_ptr();
            let mut luts = [_mm256_setzero_si256(); 16];
            let mut h = 0;
            while h < 16 {
                luts[h] =
                    _mm256_broadcastsi128_si256(_mm_loadu_si128(tp.add(h * 16) as *const __m128i));
                h += 1;
            }

            let lo_mask = _mm256_set1_epi8(0x0F);
            let len = data.len();
            let inp = data.as_ptr();
            let outp = out.as_mut_ptr();
            let mut i = 0;

            // 2x unrolled: process 64 bytes per iteration
            while i + 64 <= len {
                let v0 = _mm256_loadu_si256(inp.add(i) as *const __m256i);
                let v1 = _mm256_loadu_si256(inp.add(i + 32) as *const __m256i);

                let lo0 = _mm256_and_si256(v0, lo_mask);
                let lo1 = _mm256_and_si256(v1, lo_mask);
                let hi0 = _mm256_and_si256(_mm256_srli_epi16(v0, 4), lo_mask);
                let hi1 = _mm256_and_si256(_mm256_srli_epi16(v1, 4), lo_mask);

                let mut r0 = _mm256_setzero_si256();
                let mut r1 = _mm256_setzero_si256();

                // Process all 16 high-nibble values.
                // For each h, bytes where high_nibble == h get their result from luts[h].
                macro_rules! do_nib {
                    ($h:literal) => {
                        let hv = _mm256_set1_epi8($h);
                        let lut = luts[$h as usize];
                        let m0 = _mm256_cmpeq_epi8(hi0, hv);
                        let m1 = _mm256_cmpeq_epi8(hi1, hv);
                        r0 = _mm256_or_si256(
                            r0,
                            _mm256_and_si256(_mm256_shuffle_epi8(lut, lo0), m0),
                        );
                        r1 = _mm256_or_si256(
                            r1,
                            _mm256_and_si256(_mm256_shuffle_epi8(lut, lo1), m1),
                        );
                    };
                }

                do_nib!(0);
                do_nib!(1);
                do_nib!(2);
                do_nib!(3);
                do_nib!(4);
                do_nib!(5);
                do_nib!(6);
                do_nib!(7);
                do_nib!(8);
                do_nib!(9);
                do_nib!(10);
                do_nib!(11);
                do_nib!(12);
                do_nib!(13);
                do_nib!(14);
                do_nib!(15);

                _mm256_storeu_si256(outp.add(i) as *mut __m256i, r0);
                _mm256_storeu_si256(outp.add(i + 32) as *mut __m256i, r1);
                i += 64;
            }

            // Single vector tail
            while i + 32 <= len {
                let v = _mm256_loadu_si256(inp.add(i) as *const __m256i);
                let lo = _mm256_and_si256(v, lo_mask);
                let hi = _mm256_and_si256(_mm256_srli_epi16(v, 4), lo_mask);

                let mut result = _mm256_setzero_si256();
                let mut hh = 0u8;
                while hh < 16 {
                    let hv = _mm256_set1_epi8(hh as i8);
                    let m = _mm256_cmpeq_epi8(hi, hv);
                    result = _mm256_or_si256(
                        result,
                        _mm256_and_si256(_mm256_shuffle_epi8(luts[hh as usize], lo), m),
                    );
                    hh += 1;
                }

                _mm256_storeu_si256(outp.add(i) as *mut __m256i, result);
                i += 32;
            }

            // Scalar tail
            while i < len {
                *outp.add(i) = *table.get_unchecked(*inp.add(i) as usize);
                i += 1;
            }
        }
    }

    /// In-place general 256-byte table lookup using 16-way vpshufb.
    ///
    /// SAFETY: Caller must ensure AVX2 is available.
    #[target_feature(enable = "avx2")]
    pub unsafe fn general_lookup_inplace(data: &mut [u8], table: &[u8; 256]) {
        unsafe {
            use std::arch::x86_64::*;

            let tp = table.as_ptr();
            let mut luts = [_mm256_setzero_si256(); 16];
            let mut h = 0;
            while h < 16 {
                luts[h] =
                    _mm256_broadcastsi128_si256(_mm_loadu_si128(tp.add(h * 16) as *const __m128i));
                h += 1;
            }

            let lo_mask = _mm256_set1_epi8(0x0F);
            let len = data.len();
            let ptr = data.as_mut_ptr();
            let mut i = 0;

            while i + 64 <= len {
                let v0 = _mm256_loadu_si256(ptr.add(i) as *const __m256i);
                let v1 = _mm256_loadu_si256(ptr.add(i + 32) as *const __m256i);

                let lo0 = _mm256_and_si256(v0, lo_mask);
                let lo1 = _mm256_and_si256(v1, lo_mask);
                let hi0 = _mm256_and_si256(_mm256_srli_epi16(v0, 4), lo_mask);
                let hi1 = _mm256_and_si256(_mm256_srli_epi16(v1, 4), lo_mask);

                let mut r0 = _mm256_setzero_si256();
                let mut r1 = _mm256_setzero_si256();

                macro_rules! do_nib {
                    ($h:literal) => {
                        let hv = _mm256_set1_epi8($h);
                        let lut = luts[$h as usize];
                        let m0 = _mm256_cmpeq_epi8(hi0, hv);
                        let m1 = _mm256_cmpeq_epi8(hi1, hv);
                        r0 = _mm256_or_si256(
                            r0,
                            _mm256_and_si256(_mm256_shuffle_epi8(lut, lo0), m0),
                        );
                        r1 = _mm256_or_si256(
                            r1,
                            _mm256_and_si256(_mm256_shuffle_epi8(lut, lo1), m1),
                        );
                    };
                }

                do_nib!(0);
                do_nib!(1);
                do_nib!(2);
                do_nib!(3);
                do_nib!(4);
                do_nib!(5);
                do_nib!(6);
                do_nib!(7);
                do_nib!(8);
                do_nib!(9);
                do_nib!(10);
                do_nib!(11);
                do_nib!(12);
                do_nib!(13);
                do_nib!(14);
                do_nib!(15);

                _mm256_storeu_si256(ptr.add(i) as *mut __m256i, r0);
                _mm256_storeu_si256(ptr.add(i + 32) as *mut __m256i, r1);
                i += 64;
            }

            while i + 32 <= len {
                let v = _mm256_loadu_si256(ptr.add(i) as *const __m256i);
                let lo = _mm256_and_si256(v, lo_mask);
                let hi = _mm256_and_si256(_mm256_srli_epi16(v, 4), lo_mask);

                let mut result = _mm256_setzero_si256();
                let mut hh = 0u8;
                while hh < 16 {
                    let hv = _mm256_set1_epi8(hh as i8);
                    let m = _mm256_cmpeq_epi8(hi, hv);
                    result = _mm256_or_si256(
                        result,
                        _mm256_and_si256(_mm256_shuffle_epi8(luts[hh as usize], lo), m),
                    );
                    hh += 1;
                }

                _mm256_storeu_si256(ptr.add(i) as *mut __m256i, result);
                i += 32;
            }

            while i < len {
                let b = *ptr.add(i);
                *ptr.add(i) = *table.get_unchecked(b as usize);
                i += 1;
            }
        }
    }

    /// In-place SIMD translate for stdin path.
    ///
    /// SAFETY: Caller must ensure AVX2 is available.
    #[target_feature(enable = "avx2")]
    pub unsafe fn range_delta_inplace(data: &mut [u8], lo: u8, hi: u8, delta: u8) {
        unsafe {
            use std::arch::x86_64::*;

            let lo_vec = _mm256_set1_epi8(lo as i8);
            let range_vec = _mm256_set1_epi8((hi - lo) as i8);
            let delta_vec = _mm256_set1_epi8(delta as i8);

            let len = data.len();
            let ptr = data.as_mut_ptr();
            let mut i = 0usize;

            while i + 128 <= len {
                let v0 = _mm256_loadu_si256(ptr.add(i) as *const __m256i);
                let v1 = _mm256_loadu_si256(ptr.add(i + 32) as *const __m256i);
                let v2 = _mm256_loadu_si256(ptr.add(i + 64) as *const __m256i);
                let v3 = _mm256_loadu_si256(ptr.add(i + 96) as *const __m256i);

                let d0 = _mm256_sub_epi8(v0, lo_vec);
                let d1 = _mm256_sub_epi8(v1, lo_vec);
                let d2 = _mm256_sub_epi8(v2, lo_vec);
                let d3 = _mm256_sub_epi8(v3, lo_vec);

                let m0 = _mm256_cmpeq_epi8(_mm256_min_epu8(d0, range_vec), d0);
                let m1 = _mm256_cmpeq_epi8(_mm256_min_epu8(d1, range_vec), d1);
                let m2 = _mm256_cmpeq_epi8(_mm256_min_epu8(d2, range_vec), d2);
                let m3 = _mm256_cmpeq_epi8(_mm256_min_epu8(d3, range_vec), d3);

                let r0 = _mm256_add_epi8(v0, _mm256_and_si256(m0, delta_vec));
                let r1 = _mm256_add_epi8(v1, _mm256_and_si256(m1, delta_vec));
                let r2 = _mm256_add_epi8(v2, _mm256_and_si256(m2, delta_vec));
                let r3 = _mm256_add_epi8(v3, _mm256_and_si256(m3, delta_vec));

                _mm256_storeu_si256(ptr.add(i) as *mut __m256i, r0);
                _mm256_storeu_si256(ptr.add(i + 32) as *mut __m256i, r1);
                _mm256_storeu_si256(ptr.add(i + 64) as *mut __m256i, r2);
                _mm256_storeu_si256(ptr.add(i + 96) as *mut __m256i, r3);
                i += 128;
            }

            while i + 32 <= len {
                let v = _mm256_loadu_si256(ptr.add(i) as *const __m256i);
                let diff = _mm256_sub_epi8(v, lo_vec);
                let mask = _mm256_cmpeq_epi8(_mm256_min_epu8(diff, range_vec), diff);
                let result = _mm256_add_epi8(v, _mm256_and_si256(mask, delta_vec));
                _mm256_storeu_si256(ptr.add(i) as *mut __m256i, result);
                i += 32;
            }

            while i < len {
                let b = *ptr.add(i);
                *ptr.add(i) = if b.wrapping_sub(lo) <= (hi - lo) {
                    b.wrapping_add(delta)
                } else {
                    b
                };
                i += 1;
            }
        }
    }
}

/// AVX2 nibble-based set membership classifier.
/// Uses vpshufb to test 32 bytes at a time for membership in a byte set.
/// Returns a `__m256i` where each byte is 0xFF if the byte is NOT in the set, 0x00 if it IS.
/// Check if AVX2 is available at runtime.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn has_avx2() -> bool {
    is_x86_feature_detected!("avx2")
}

/// Translate a chunk of bytes using a lookup table — unrolled 8-byte inner loop.
#[inline(always)]
fn translate_chunk(chunk: &[u8], out: &mut [u8], table: &[u8; 256]) {
    let len = chunk.len();
    let mut i = 0;
    while i + 8 <= len {
        unsafe {
            *out.get_unchecked_mut(i) = *table.get_unchecked(*chunk.get_unchecked(i) as usize);
            *out.get_unchecked_mut(i + 1) =
                *table.get_unchecked(*chunk.get_unchecked(i + 1) as usize);
            *out.get_unchecked_mut(i + 2) =
                *table.get_unchecked(*chunk.get_unchecked(i + 2) as usize);
            *out.get_unchecked_mut(i + 3) =
                *table.get_unchecked(*chunk.get_unchecked(i + 3) as usize);
            *out.get_unchecked_mut(i + 4) =
                *table.get_unchecked(*chunk.get_unchecked(i + 4) as usize);
            *out.get_unchecked_mut(i + 5) =
                *table.get_unchecked(*chunk.get_unchecked(i + 5) as usize);
            *out.get_unchecked_mut(i + 6) =
                *table.get_unchecked(*chunk.get_unchecked(i + 6) as usize);
            *out.get_unchecked_mut(i + 7) =
                *table.get_unchecked(*chunk.get_unchecked(i + 7) as usize);
        }
        i += 8;
    }
    while i < len {
        unsafe {
            *out.get_unchecked_mut(i) = *table.get_unchecked(*chunk.get_unchecked(i) as usize);
        }
        i += 1;
    }
}

/// In-place translate for stdin path — avoids separate output buffer.
#[inline(always)]
fn translate_inplace(data: &mut [u8], table: &[u8; 256]) {
    let len = data.len();
    let ptr = data.as_mut_ptr();
    let tab = table.as_ptr();

    unsafe {
        let mut i = 0;
        while i + 8 <= len {
            *ptr.add(i) = *tab.add(*ptr.add(i) as usize);
            *ptr.add(i + 1) = *tab.add(*ptr.add(i + 1) as usize);
            *ptr.add(i + 2) = *tab.add(*ptr.add(i + 2) as usize);
            *ptr.add(i + 3) = *tab.add(*ptr.add(i + 3) as usize);
            *ptr.add(i + 4) = *tab.add(*ptr.add(i + 4) as usize);
            *ptr.add(i + 5) = *tab.add(*ptr.add(i + 5) as usize);
            *ptr.add(i + 6) = *tab.add(*ptr.add(i + 6) as usize);
            *ptr.add(i + 7) = *tab.add(*ptr.add(i + 7) as usize);
            i += 8;
        }
        while i < len {
            *ptr.add(i) = *tab.add(*ptr.add(i) as usize);
            i += 1;
        }
    }
}

// ============================================================================
// Dispatch: choose SIMD or scalar based on table analysis
// ============================================================================

/// Translate a chunk using the best available method.
/// `use_simd` is cached at call site to avoid per-chunk atomic loads.
#[inline]
fn translate_chunk_dispatch(
    chunk: &[u8],
    out: &mut [u8],
    table: &[u8; 256],
    kind: &TranslateKind,
    _use_simd: bool,
) {
    match kind {
        TranslateKind::Identity => {
            out[..chunk.len()].copy_from_slice(chunk);
        }
        #[cfg(target_arch = "x86_64")]
        TranslateKind::RangeDelta { lo, hi, delta } => {
            if _use_simd {
                unsafe { simd_tr::range_delta(chunk, out, *lo, *hi, *delta) };
                return;
            }
            translate_chunk(chunk, out, table);
        }
        #[cfg(not(target_arch = "x86_64"))]
        TranslateKind::RangeDelta { .. } => {
            translate_chunk(chunk, out, table);
        }
        #[cfg(target_arch = "x86_64")]
        TranslateKind::General => {
            if _use_simd {
                unsafe { simd_tr::general_lookup(chunk, out, table) };
                return;
            }
            translate_chunk(chunk, out, table);
        }
        #[cfg(not(target_arch = "x86_64"))]
        TranslateKind::General => {
            translate_chunk(chunk, out, table);
        }
    }
}

/// In-place translate dispatch.
/// `use_simd` is cached at call site to avoid per-chunk atomic loads.
#[inline]
fn translate_inplace_dispatch(
    data: &mut [u8],
    table: &[u8; 256],
    kind: &TranslateKind,
    _use_simd: bool,
) {
    match kind {
        TranslateKind::Identity => {}
        #[cfg(target_arch = "x86_64")]
        TranslateKind::RangeDelta { lo, hi, delta } => {
            if _use_simd {
                unsafe { simd_tr::range_delta_inplace(data, *lo, *hi, *delta) };
                return;
            }
            translate_inplace(data, table);
        }
        #[cfg(not(target_arch = "x86_64"))]
        TranslateKind::RangeDelta { .. } => {
            translate_inplace(data, table);
        }
        #[cfg(target_arch = "x86_64")]
        TranslateKind::General => {
            if _use_simd {
                unsafe { simd_tr::general_lookup_inplace(data, table) };
                return;
            }
            translate_inplace(data, table);
        }
        #[cfg(not(target_arch = "x86_64"))]
        TranslateKind::General => {
            translate_inplace(data, table);
        }
    }
}

// ============================================================================
// Streaming functions (Read + Write)
// Process data immediately after each read() — no fill_buf accumulation.
// Uses 256KB buffer (L2-friendly) instead of 8MB.
// ============================================================================

pub fn translate(
    set1: &[u8],
    set2: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);
    let kind = analyze_table(&table);
    #[cfg(target_arch = "x86_64")]
    let use_simd = has_avx2();
    #[cfg(not(target_arch = "x86_64"))]
    let use_simd = false;

    let mut buf = vec![0u8; STREAM_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        translate_inplace_dispatch(&mut buf[..n], &table, &kind, use_simd);
        writer.write_all(&buf[..n])?;
    }
    Ok(())
}

pub fn translate_squeeze(
    set1: &[u8],
    set2: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);
    let squeeze_set = build_member_set(set2);
    let mut outbuf = vec![0u8; STREAM_BUF];
    let mut inbuf = vec![0u8; STREAM_BUF];
    let mut last_squeezed: u16 = 256;

    loop {
        let n = match reader.read(&mut inbuf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let mut out_pos = 0;
        for &b in &inbuf[..n] {
            let translated = unsafe { *table.get_unchecked(b as usize) };
            if is_member(&squeeze_set, translated) {
                if last_squeezed == translated as u16 {
                    continue;
                }
                last_squeezed = translated as u16;
            } else {
                last_squeezed = 256;
            }
            unsafe {
                *outbuf.get_unchecked_mut(out_pos) = translated;
            }
            out_pos += 1;
        }
        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

pub fn delete(
    delete_chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    // Fast path: single character delete using SIMD memchr
    if delete_chars.len() == 1 {
        return delete_single_streaming(delete_chars[0], reader, writer);
    }

    // Fast paths: 2-3 char delete using SIMD memchr2/memchr3
    if delete_chars.len() <= 3 {
        return delete_multi_streaming(delete_chars, reader, writer);
    }

    let member = build_member_set(delete_chars);
    let mut outbuf = vec![0u8; STREAM_BUF];
    let mut inbuf = vec![0u8; STREAM_BUF];

    loop {
        let n = match reader.read(&mut inbuf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let mut out_pos = 0;
        for &b in &inbuf[..n] {
            if !is_member(&member, b) {
                unsafe {
                    *outbuf.get_unchecked_mut(out_pos) = b;
                }
                out_pos += 1;
            }
        }
        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

/// Single-character delete from a reader using SIMD memchr scanning.
fn delete_single_streaming(
    ch: u8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut buf = vec![0u8; STREAM_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let chunk = &buf[..n];
        let mut last = 0;
        for pos in memchr::memchr_iter(ch, chunk) {
            if pos > last {
                writer.write_all(&chunk[last..pos])?;
            }
            last = pos + 1;
        }
        if last < n {
            writer.write_all(&chunk[last..n])?;
        }
    }
    Ok(())
}

/// Multi-character delete (2-3 chars) from a reader using SIMD memchr2/memchr3.
fn delete_multi_streaming(
    chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut buf = vec![0u8; STREAM_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let chunk = &buf[..n];
        let mut last = 0;
        if chars.len() == 2 {
            for pos in memchr::memchr2_iter(chars[0], chars[1], chunk) {
                if pos > last {
                    writer.write_all(&chunk[last..pos])?;
                }
                last = pos + 1;
            }
        } else {
            for pos in memchr::memchr3_iter(chars[0], chars[1], chars[2], chunk) {
                if pos > last {
                    writer.write_all(&chunk[last..pos])?;
                }
                last = pos + 1;
            }
        }
        if last < n {
            writer.write_all(&chunk[last..n])?;
        }
    }
    Ok(())
}

pub fn delete_squeeze(
    delete_chars: &[u8],
    squeeze_chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let delete_set = build_member_set(delete_chars);
    let squeeze_set = build_member_set(squeeze_chars);
    let mut outbuf = vec![0u8; STREAM_BUF];
    let mut inbuf = vec![0u8; STREAM_BUF];
    let mut last_squeezed: u16 = 256;

    loop {
        let n = match reader.read(&mut inbuf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let mut out_pos = 0;
        for &b in &inbuf[..n] {
            if is_member(&delete_set, b) {
                continue;
            }
            if is_member(&squeeze_set, b) {
                if last_squeezed == b as u16 {
                    continue;
                }
                last_squeezed = b as u16;
            } else {
                last_squeezed = 256;
            }
            unsafe {
                *outbuf.get_unchecked_mut(out_pos) = b;
            }
            out_pos += 1;
        }
        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

pub fn squeeze(
    squeeze_chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    // Fast path: single squeeze char — bulk copy non-match runs
    if squeeze_chars.len() == 1 {
        return squeeze_single_stream(squeeze_chars[0], reader, writer);
    }

    let member = build_member_set(squeeze_chars);
    let mut outbuf = vec![0u8; STREAM_BUF];
    let mut inbuf = vec![0u8; STREAM_BUF];
    let mut last_squeezed: u16 = 256;

    loop {
        let n = match reader.read(&mut inbuf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let mut out_pos = 0;
        for &b in &inbuf[..n] {
            if is_member(&member, b) {
                if last_squeezed == b as u16 {
                    continue;
                }
                last_squeezed = b as u16;
            } else {
                last_squeezed = 256;
            }
            unsafe {
                *outbuf.get_unchecked_mut(out_pos) = b;
            }
            out_pos += 1;
        }
        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

/// Squeeze a single character from a stream — optimized with bulk copy.
fn squeeze_single_stream(
    ch: u8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut inbuf = vec![0u8; STREAM_BUF];
    let mut outbuf = vec![0u8; STREAM_BUF];
    let mut was_squeeze_char = false;

    loop {
        let n = match reader.read(&mut inbuf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let chunk = &inbuf[..n];
        let mut out_pos = 0;
        let mut i = 0;

        while i < n {
            if chunk[i] == ch {
                if !was_squeeze_char {
                    // First in run — keep it
                    unsafe {
                        *outbuf.get_unchecked_mut(out_pos) = ch;
                    }
                    out_pos += 1;
                    was_squeeze_char = true;
                }
                // Skip rest of run
                i += 1;
            } else {
                was_squeeze_char = false;
                // Find next squeeze char using SIMD memchr
                let remaining = &chunk[i..];
                let run_end = memchr::memchr(ch, remaining).unwrap_or(remaining.len());
                // Bulk copy the non-match run
                outbuf[out_pos..out_pos + run_end].copy_from_slice(&chunk[i..i + run_end]);
                out_pos += run_end;
                i += run_end;
            }
        }

        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

// ============================================================================
// Mmap-based functions (zero-copy input from byte slice)
// ============================================================================

/// Translate bytes from an mmap'd byte slice — zero syscall reads.
/// Uses SIMD AVX2 for range-delta patterns (e.g., a-z → A-Z).
/// For large inputs, translates in parallel using rayon for maximum throughput.
pub fn translate_mmap(
    set1: &[u8],
    set2: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);
    let kind = analyze_table(&table);
    #[cfg(target_arch = "x86_64")]
    let use_simd = has_avx2();
    #[cfg(not(target_arch = "x86_64"))]
    let use_simd = false;

    if matches!(kind, TranslateKind::Identity) {
        return writer.write_all(data);
    }

    // Parallel translation for very large inputs — single shared output buffer
    if data.len() >= PARALLEL_TRANSLATE_THRESHOLD {
        let mut out = vec![0u8; data.len()];
        let num_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() + num_threads - 1) / num_threads;
        // Align to BUF_SIZE boundaries for cache efficiency
        let chunk_size = ((chunk_size + BUF_SIZE - 1) / BUF_SIZE) * BUF_SIZE;

        // Translate in parallel into shared buffer — no per-chunk allocation
        data.par_chunks(chunk_size)
            .zip(out.par_chunks_mut(chunk_size))
            .for_each(|(inp, outp)| {
                translate_chunk_dispatch(inp, &mut outp[..inp.len()], &table, &kind, use_simd);
            });

        // Single write of entire result
        writer.write_all(&out)?;
        return Ok(());
    }

    // Single-allocation path: translate entire data into one buffer, single write
    if data.len() <= BUF_SIZE {
        let mut out = vec![0u8; data.len()];
        translate_chunk_dispatch(data, &mut out, &table, &kind, use_simd);
        writer.write_all(&out)?;
        return Ok(());
    }

    // Chunked path for larger data — reuses buffer across chunks
    let mut out = vec![0u8; BUF_SIZE];
    for chunk in data.chunks(BUF_SIZE) {
        translate_chunk_dispatch(chunk, &mut out[..chunk.len()], &table, &kind, use_simd);
        writer.write_all(&out[..chunk.len()])?;
    }
    Ok(())
}

/// Translate + squeeze from mmap'd byte slice.
/// Uses a two-pass approach for the common case where few bytes are squeezed:
/// translate first (using SIMD when possible), then squeeze.
pub fn translate_squeeze_mmap(
    set1: &[u8],
    set2: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);
    let squeeze_set = build_member_set(set2);
    let kind = analyze_table(&table);
    #[cfg(target_arch = "x86_64")]
    let use_simd = has_avx2();
    #[cfg(not(target_arch = "x86_64"))]
    let use_simd = false;

    // Pre-allocate output buffer for translation
    let mut trans_buf = vec![0u8; BUF_SIZE];
    let mut outbuf = vec![0u8; BUF_SIZE];
    let mut last_squeezed: u16 = 256;

    for chunk in data.chunks(BUF_SIZE) {
        // Phase 1: Translate (may use SIMD)
        translate_chunk_dispatch(
            chunk,
            &mut trans_buf[..chunk.len()],
            &table,
            &kind,
            use_simd,
        );

        // Phase 2: Squeeze
        let mut out_pos = 0;
        for i in 0..chunk.len() {
            let translated = unsafe { *trans_buf.get_unchecked(i) };
            if is_member(&squeeze_set, translated) {
                if last_squeezed == translated as u16 {
                    continue;
                }
                last_squeezed = translated as u16;
            } else {
                last_squeezed = 256;
            }
            unsafe {
                *outbuf.get_unchecked_mut(out_pos) = translated;
            }
            out_pos += 1;
        }
        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

/// Delete from mmap'd byte slice.
/// Uses SIMD memchr for single-character delete (common case).
/// For multi-char delete, uses 8-byte unrolled scan with bitset lookup.
pub fn delete_mmap(delete_chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    // Fast path: single character delete uses SIMD memchr
    if delete_chars.len() == 1 {
        return delete_single_char_mmap(delete_chars[0], data, writer);
    }

    // Fast path: 2-char delete uses SIMD memchr2 (bulk copy between matches)
    if delete_chars.len() == 2 {
        return delete_multi_memchr_mmap::<2>(delete_chars, data, writer);
    }

    // Fast path: 3-char delete uses SIMD memchr3 (bulk copy between matches)
    if delete_chars.len() == 3 {
        return delete_multi_memchr_mmap::<3>(delete_chars, data, writer);
    }

    let member = build_member_set(delete_chars);
    let mut outbuf = vec![0u8; BUF_SIZE];

    for chunk in data.chunks(BUF_SIZE) {
        let mut out_pos = 0;
        let len = chunk.len();
        let mut i = 0;

        // 8-byte unrolled scan for better ILP
        while i + 8 <= len {
            unsafe {
                let b0 = *chunk.get_unchecked(i);
                let b1 = *chunk.get_unchecked(i + 1);
                let b2 = *chunk.get_unchecked(i + 2);
                let b3 = *chunk.get_unchecked(i + 3);
                let b4 = *chunk.get_unchecked(i + 4);
                let b5 = *chunk.get_unchecked(i + 5);
                let b6 = *chunk.get_unchecked(i + 6);
                let b7 = *chunk.get_unchecked(i + 7);

                if !is_member(&member, b0) {
                    *outbuf.get_unchecked_mut(out_pos) = b0;
                    out_pos += 1;
                }
                if !is_member(&member, b1) {
                    *outbuf.get_unchecked_mut(out_pos) = b1;
                    out_pos += 1;
                }
                if !is_member(&member, b2) {
                    *outbuf.get_unchecked_mut(out_pos) = b2;
                    out_pos += 1;
                }
                if !is_member(&member, b3) {
                    *outbuf.get_unchecked_mut(out_pos) = b3;
                    out_pos += 1;
                }
                if !is_member(&member, b4) {
                    *outbuf.get_unchecked_mut(out_pos) = b4;
                    out_pos += 1;
                }
                if !is_member(&member, b5) {
                    *outbuf.get_unchecked_mut(out_pos) = b5;
                    out_pos += 1;
                }
                if !is_member(&member, b6) {
                    *outbuf.get_unchecked_mut(out_pos) = b6;
                    out_pos += 1;
                }
                if !is_member(&member, b7) {
                    *outbuf.get_unchecked_mut(out_pos) = b7;
                    out_pos += 1;
                }
            }
            i += 8;
        }

        while i < len {
            unsafe {
                let b = *chunk.get_unchecked(i);
                if !is_member(&member, b) {
                    *outbuf.get_unchecked_mut(out_pos) = b;
                    out_pos += 1;
                }
            }
            i += 1;
        }

        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

/// Multi-character delete (2-3 chars) using SIMD memchr2/memchr3.
/// Bulk-copies runs of non-matching bytes, far faster than byte-at-a-time.
fn delete_multi_memchr_mmap<const N: usize>(
    chars: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut last = 0;
    if N == 2 {
        for pos in memchr::memchr2_iter(chars[0], chars[1], data) {
            if pos > last {
                writer.write_all(&data[last..pos])?;
            }
            last = pos + 1;
        }
    } else {
        for pos in memchr::memchr3_iter(chars[0], chars[1], chars[2], data) {
            if pos > last {
                writer.write_all(&data[last..pos])?;
            }
            last = pos + 1;
        }
    }
    if last < data.len() {
        writer.write_all(&data[last..])?;
    }
    Ok(())
}

/// Single-character delete from mmap using SIMD memchr.
/// Copies runs of non-matching bytes in bulk (memcpy), far faster than byte-at-a-time.
fn delete_single_char_mmap(ch: u8, data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    let mut last = 0;
    for pos in memchr::memchr_iter(ch, data) {
        if pos > last {
            writer.write_all(&data[last..pos])?;
        }
        last = pos + 1;
    }
    if last < data.len() {
        writer.write_all(&data[last..])?;
    }
    Ok(())
}

/// Delete + squeeze from mmap'd byte slice.
pub fn delete_squeeze_mmap(
    delete_chars: &[u8],
    squeeze_chars: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let delete_set = build_member_set(delete_chars);
    let squeeze_set = build_member_set(squeeze_chars);
    let mut outbuf = vec![0u8; BUF_SIZE];
    let mut last_squeezed: u16 = 256;

    for chunk in data.chunks(BUF_SIZE) {
        let mut out_pos = 0;
        for &b in chunk {
            if is_member(&delete_set, b) {
                continue;
            }
            if is_member(&squeeze_set, b) {
                if last_squeezed == b as u16 {
                    continue;
                }
                last_squeezed = b as u16;
            } else {
                last_squeezed = 256;
            }
            unsafe {
                *outbuf.get_unchecked_mut(out_pos) = b;
            }
            out_pos += 1;
        }
        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

/// Squeeze from mmap'd byte slice.
/// Uses a two-pass approach: find runs of squeezable bytes with memchr,
/// then copy non-squeezed content in bulk.
pub fn squeeze_mmap(squeeze_chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    // Fast path: single squeeze character — use SIMD memchr to find runs
    if squeeze_chars.len() == 1 {
        return squeeze_single_mmap(squeeze_chars[0], data, writer);
    }

    // Fast path: 2-3 squeeze chars — use memchr2/memchr3 for SIMD scanning
    if squeeze_chars.len() == 2 {
        return squeeze_multi_mmap::<2>(squeeze_chars, data, writer);
    }
    if squeeze_chars.len() == 3 {
        return squeeze_multi_mmap::<3>(squeeze_chars, data, writer);
    }

    // General path: 8-byte unrolled member check with bulk non-member span copy
    let member = build_member_set(squeeze_chars);
    let mut last_squeezed: u16 = 256;
    let mut span_start = 0; // start of current non-squeezed span in data
    let len = data.len();
    let mut i = 0;

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };
        if is_member(&member, b) {
            // Write non-member span before this member byte
            if i > span_start {
                writer.write_all(&data[span_start..i])?;
            }
            if last_squeezed != b as u16 {
                // First occurrence of this member — write it
                writer.write_all(&[b])?;
                last_squeezed = b as u16;
            }
            i += 1;
            // Skip consecutive duplicates of same byte
            while i < len && data[i] == b {
                i += 1;
            }
            span_start = i;
        } else {
            last_squeezed = 256;
            i += 1;
        }
    }

    // Write remaining non-member span
    if span_start < len {
        writer.write_all(&data[span_start..])?;
    }
    Ok(())
}

/// Squeeze with 2-3 char sets using SIMD memchr2/memchr3 for fast scanning.
/// Writes non-member spans directly from mmap (zero-copy).
fn squeeze_multi_mmap<const N: usize>(
    chars: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut last_squeezed: u16 = 256;
    let mut cursor = 0;

    macro_rules! find_next {
        ($data:expr) => {
            if N == 2 {
                memchr::memchr2(chars[0], chars[1], $data)
            } else {
                memchr::memchr3(chars[0], chars[1], chars[2], $data)
            }
        };
    }

    while cursor < data.len() {
        match find_next!(&data[cursor..]) {
            Some(offset) => {
                let pos = cursor + offset;
                let b = data[pos];
                // Write non-member span
                if pos > cursor {
                    writer.write_all(&data[cursor..pos])?;
                    last_squeezed = 256;
                }
                if last_squeezed != b as u16 {
                    writer.write_all(&[b])?;
                    last_squeezed = b as u16;
                }
                // Skip consecutive duplicates of same byte
                let mut skip = pos + 1;
                while skip < data.len() && data[skip] == b {
                    skip += 1;
                }
                cursor = skip;
            }
            None => {
                writer.write_all(&data[cursor..])?;
                break;
            }
        }
    }
    Ok(())
}

/// Squeeze a single repeated character from mmap'd data.
/// Zero-copy: writes directly from mmap data, no intermediate buffer.
/// Uses SIMD memchr for bulk-skip between occurrences of the squeeze char.
fn squeeze_single_mmap(ch: u8, data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    let mut cursor = 0;

    while cursor < data.len() {
        // Find next occurrence of squeeze char using SIMD memchr
        match memchr::memchr(ch, &data[cursor..]) {
            Some(offset) => {
                let pos = cursor + offset;
                // Write everything from cursor to pos + 1 (including first occurrence)
                writer.write_all(&data[cursor..pos + 1])?;
                // Skip consecutive duplicates
                let mut skip = pos + 1;
                while skip < data.len() && data[skip] == ch {
                    skip += 1;
                }
                cursor = skip;
            }
            None => {
                // No more occurrences — write remaining data
                writer.write_all(&data[cursor..])?;
                break;
            }
        }
    }
    Ok(())
}
