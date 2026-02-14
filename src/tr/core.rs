use std::io::{self, Read, Write};

use rayon::prelude::*;

/// Maximum IoSlice entries per write_vectored batch.
/// Linux UIO_MAXIOV is 1024; we use that as our batch limit.
const MAX_IOV: usize = 1024;

/// Main processing buffer: 4MB (fits in L3 cache, avoids cache thrashing).
const BUF_SIZE: usize = 4 * 1024 * 1024;

/// Stream buffer: 16MB — tr streaming operations (translate, delete, squeeze)
/// are compute-light (single table lookup or bitset check per byte), so the
/// bottleneck is I/O syscalls, not cache pressure. 16MB buffer means only
/// 1 read()/write() syscall pair for a 10MB input, minimizing syscall overhead.
/// For piped input, read_full retries partial reads to fill the entire buffer.
/// This applies to ALL streaming modes (delete, squeeze, translate).
const STREAM_BUF: usize = 16 * 1024 * 1024;

/// Minimum data size to engage rayon parallel processing for mmap paths.
/// AVX2 translation runs at ~10 GB/s per core. At 4MB, parallel
/// processing with 2 cores provides ~1.5x speedup (2MB/core in ~0.2ms)
/// while rayon overhead is ~100us (thread pool is pre-warmed after first use).
/// Lowered from 16MB to benefit 4-10MB inputs common in benchmarks.
const PARALLEL_THRESHOLD: usize = 4 * 1024 * 1024;

/// Write multiple IoSlice buffers using write_vectored, batching into MAX_IOV-sized groups.
/// Falls back to write_all per slice for partial writes.
#[inline]
fn write_ioslices(writer: &mut impl Write, slices: &[std::io::IoSlice]) -> io::Result<()> {
    if slices.is_empty() {
        return Ok(());
    }
    for batch in slices.chunks(MAX_IOV) {
        let total: usize = batch.iter().map(|s| s.len()).sum();
        match writer.write_vectored(batch) {
            Ok(n) if n >= total => continue,
            Ok(mut written) => {
                // Partial write: fall back to write_all per remaining slice
                for slice in batch {
                    let slen = slice.len();
                    if written >= slen {
                        written -= slen;
                        continue;
                    }
                    if written > 0 {
                        writer.write_all(&slice[written..])?;
                        written = 0;
                    } else {
                        writer.write_all(slice)?;
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Allocate a Vec<u8> of given length without zero-initialization.
/// SAFETY: Caller must write all bytes before reading them.
#[inline]
#[allow(clippy::uninit_vec)]
fn alloc_uninit_vec(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    // SAFETY: u8 has no drop, no invalid bit patterns; caller will overwrite before reading
    unsafe {
        v.set_len(len);
    }
    v
}

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

/// Cached SIMD capability level for x86_64.
/// 0 = unchecked, 1 = scalar only, 2 = SSSE3, 3 = AVX2
#[cfg(target_arch = "x86_64")]
static SIMD_LEVEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn get_simd_level() -> u8 {
    let level = SIMD_LEVEL.load(std::sync::atomic::Ordering::Relaxed);
    if level != 0 {
        return level;
    }
    let detected = if is_x86_feature_detected!("avx2") {
        3
    } else if is_x86_feature_detected!("ssse3") {
        2
    } else {
        1
    };
    SIMD_LEVEL.store(detected, std::sync::atomic::Ordering::Relaxed);
    detected
}

/// Count how many entries in the translate table are non-identity.
#[cfg(target_arch = "x86_64")]
#[inline]
fn count_non_identity(table: &[u8; 256]) -> usize {
    table
        .iter()
        .enumerate()
        .filter(|&(i, &v)| v != i as u8)
        .count()
}

/// Translate bytes in-place using a 256-byte lookup table.
/// For sparse translations (few bytes change), uses SIMD skip-ahead:
/// compare 32 bytes at a time against identity, skip unchanged chunks.
/// For dense translations, uses full SIMD nibble decomposition.
/// Falls back to 8x-unrolled scalar on non-x86_64 platforms.
#[inline(always)]
fn translate_inplace(data: &mut [u8], table: &[u8; 256]) {
    #[cfg(target_arch = "x86_64")]
    {
        let level = get_simd_level();
        if level >= 3 {
            // For sparse translations (<=16 non-identity entries), the skip-ahead
            // approach is faster: load 32 bytes, do a full nibble lookup, compare
            // against input, skip store if identical. This avoids writing to pages
            // that don't change (important for MAP_PRIVATE COW mmap).
            let non_id = count_non_identity(table);
            if non_id > 0 && non_id <= 16 {
                unsafe { translate_inplace_avx2_sparse(data, table) };
                return;
            }
            unsafe { translate_inplace_avx2_table(data, table) };
            return;
        }
        if level >= 2 {
            unsafe { translate_inplace_ssse3_table(data, table) };
            return;
        }
    }
    translate_inplace_scalar(data, table);
}

/// Sparse AVX2 translate: skip unchanged 32-byte chunks.
/// For each chunk: perform full nibble lookup, compare result vs input.
/// If identical (no bytes changed), skip the store entirely.
/// This reduces memory bandwidth and avoids COW page faults for
/// MAP_PRIVATE mmaps when most bytes are unchanged.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_inplace_avx2_sparse(data: &mut [u8], table: &[u8; 256]) {
    use std::arch::x86_64::*;

    unsafe {
        let len = data.len();
        let ptr = data.as_mut_ptr();

        // Pre-build 16 lookup vectors (same as full nibble decomposition)
        let mut lut = [_mm256_setzero_si256(); 16];
        for h in 0u8..16 {
            let base = (h as usize) * 16;
            let row: [u8; 16] = std::array::from_fn(|i| *table.get_unchecked(base + i));
            let row128 = _mm_loadu_si128(row.as_ptr() as *const _);
            lut[h as usize] = _mm256_broadcastsi128_si256(row128);
        }

        let lo_mask = _mm256_set1_epi8(0x0F);

        let mut i = 0;
        while i + 32 <= len {
            let input = _mm256_loadu_si256(ptr.add(i) as *const _);
            let lo_nibble = _mm256_and_si256(input, lo_mask);
            let hi_nibble = _mm256_and_si256(_mm256_srli_epi16(input, 4), lo_mask);

            let mut result = _mm256_setzero_si256();
            macro_rules! do_nibble {
                ($h:expr) => {
                    let h_val = _mm256_set1_epi8($h as i8);
                    let mask = _mm256_cmpeq_epi8(hi_nibble, h_val);
                    let looked_up = _mm256_shuffle_epi8(lut[$h], lo_nibble);
                    result = _mm256_or_si256(result, _mm256_and_si256(mask, looked_up));
                };
            }
            do_nibble!(0);
            do_nibble!(1);
            do_nibble!(2);
            do_nibble!(3);
            do_nibble!(4);
            do_nibble!(5);
            do_nibble!(6);
            do_nibble!(7);
            do_nibble!(8);
            do_nibble!(9);
            do_nibble!(10);
            do_nibble!(11);
            do_nibble!(12);
            do_nibble!(13);
            do_nibble!(14);
            do_nibble!(15);

            // Only store if result differs from input (skip unchanged chunks)
            let diff = _mm256_xor_si256(input, result);
            if _mm256_testz_si256(diff, diff) == 0 {
                _mm256_storeu_si256(ptr.add(i) as *mut _, result);
            }
            i += 32;
        }

        // Scalar tail
        while i < len {
            *ptr.add(i) = *table.get_unchecked(*ptr.add(i) as usize);
            i += 1;
        }
    }
}

/// Scalar fallback: 8x-unrolled table lookup.
#[inline(always)]
fn translate_inplace_scalar(data: &mut [u8], table: &[u8; 256]) {
    let len = data.len();
    let ptr = data.as_mut_ptr();
    let mut i = 0;
    unsafe {
        while i + 8 <= len {
            *ptr.add(i) = *table.get_unchecked(*ptr.add(i) as usize);
            *ptr.add(i + 1) = *table.get_unchecked(*ptr.add(i + 1) as usize);
            *ptr.add(i + 2) = *table.get_unchecked(*ptr.add(i + 2) as usize);
            *ptr.add(i + 3) = *table.get_unchecked(*ptr.add(i + 3) as usize);
            *ptr.add(i + 4) = *table.get_unchecked(*ptr.add(i + 4) as usize);
            *ptr.add(i + 5) = *table.get_unchecked(*ptr.add(i + 5) as usize);
            *ptr.add(i + 6) = *table.get_unchecked(*ptr.add(i + 6) as usize);
            *ptr.add(i + 7) = *table.get_unchecked(*ptr.add(i + 7) as usize);
            i += 8;
        }
        while i < len {
            *ptr.add(i) = *table.get_unchecked(*ptr.add(i) as usize);
            i += 1;
        }
    }
}

// ============================================================================
// SIMD arbitrary table lookup using pshufb nibble decomposition (x86_64)
// ============================================================================
//
// For an arbitrary 256-byte lookup table, we decompose each byte into
// high nibble (bits 7-4) and low nibble (bits 3-0). We pre-build 16
// SIMD vectors, one for each high nibble value h (0..15), containing
// the 16 table entries table[h*16+0..h*16+15]. Then for each input
// vector we:
//   1. Extract low nibble (AND 0x0F) -> used as pshufb index
//   2. Extract high nibble (shift right 4) -> used to select which table
//   3. For each of the 16 high nibble values, create a mask where
//      the high nibble equals that value, pshufb the corresponding
//      table, and accumulate results
//
// AVX2 processes 32 bytes/iteration; SSSE3 processes 16 bytes/iteration.
// With instruction-level parallelism, this achieves much higher throughput
// than scalar table lookups which have serial data dependencies.

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_inplace_avx2_table(data: &mut [u8], table: &[u8; 256]) {
    use std::arch::x86_64::*;

    unsafe {
        let len = data.len();
        let ptr = data.as_mut_ptr();

        // Pre-build 16 lookup vectors, one per high nibble value.
        // Each vector holds 32 bytes = 2x 128-bit lanes, each lane has the same
        // 16 table entries for pshufb indexing by low nibble.
        let mut lut = [_mm256_setzero_si256(); 16];
        for h in 0u8..16 {
            let base = (h as usize) * 16;
            let row: [u8; 16] = std::array::from_fn(|i| *table.get_unchecked(base + i));
            // Broadcast the 128-bit row to both lanes of the 256-bit vector
            let row128 = _mm_loadu_si128(row.as_ptr() as *const _);
            lut[h as usize] = _mm256_broadcastsi128_si256(row128);
        }

        let lo_mask = _mm256_set1_epi8(0x0F);

        let mut i = 0;

        // 2x unrolled: process 64 bytes (2x32) per iteration for better ILP.
        // The CPU can overlap load/compute of the second vector while the first
        // is in the nibble decomposition pipeline.
        while i + 64 <= len {
            let input0 = _mm256_loadu_si256(ptr.add(i) as *const _);
            let input1 = _mm256_loadu_si256(ptr.add(i + 32) as *const _);

            let lo0 = _mm256_and_si256(input0, lo_mask);
            let hi0 = _mm256_and_si256(_mm256_srli_epi16(input0, 4), lo_mask);
            let lo1 = _mm256_and_si256(input1, lo_mask);
            let hi1 = _mm256_and_si256(_mm256_srli_epi16(input1, 4), lo_mask);

            let mut r0 = _mm256_setzero_si256();
            let mut r1 = _mm256_setzero_si256();

            macro_rules! do_nibble2 {
                ($h:expr) => {
                    let h_val = _mm256_set1_epi8($h as i8);
                    let m0 = _mm256_cmpeq_epi8(hi0, h_val);
                    let l0 = _mm256_shuffle_epi8(lut[$h], lo0);
                    r0 = _mm256_or_si256(r0, _mm256_and_si256(m0, l0));
                    let m1 = _mm256_cmpeq_epi8(hi1, h_val);
                    let l1 = _mm256_shuffle_epi8(lut[$h], lo1);
                    r1 = _mm256_or_si256(r1, _mm256_and_si256(m1, l1));
                };
            }
            do_nibble2!(0);
            do_nibble2!(1);
            do_nibble2!(2);
            do_nibble2!(3);
            do_nibble2!(4);
            do_nibble2!(5);
            do_nibble2!(6);
            do_nibble2!(7);
            do_nibble2!(8);
            do_nibble2!(9);
            do_nibble2!(10);
            do_nibble2!(11);
            do_nibble2!(12);
            do_nibble2!(13);
            do_nibble2!(14);
            do_nibble2!(15);

            _mm256_storeu_si256(ptr.add(i) as *mut _, r0);
            _mm256_storeu_si256(ptr.add(i + 32) as *mut _, r1);
            i += 64;
        }

        // Remaining 32-byte chunk
        if i + 32 <= len {
            let input = _mm256_loadu_si256(ptr.add(i) as *const _);
            let lo_nibble = _mm256_and_si256(input, lo_mask);
            let hi_nibble = _mm256_and_si256(_mm256_srli_epi16(input, 4), lo_mask);

            let mut result = _mm256_setzero_si256();

            macro_rules! do_nibble {
                ($h:expr) => {
                    let h_val = _mm256_set1_epi8($h as i8);
                    let mask = _mm256_cmpeq_epi8(hi_nibble, h_val);
                    let looked_up = _mm256_shuffle_epi8(lut[$h], lo_nibble);
                    result = _mm256_or_si256(result, _mm256_and_si256(mask, looked_up));
                };
            }
            do_nibble!(0);
            do_nibble!(1);
            do_nibble!(2);
            do_nibble!(3);
            do_nibble!(4);
            do_nibble!(5);
            do_nibble!(6);
            do_nibble!(7);
            do_nibble!(8);
            do_nibble!(9);
            do_nibble!(10);
            do_nibble!(11);
            do_nibble!(12);
            do_nibble!(13);
            do_nibble!(14);
            do_nibble!(15);

            _mm256_storeu_si256(ptr.add(i) as *mut _, result);
            i += 32;
        }

        // SSE/SSSE3 tail for remaining 16-byte chunk
        if i + 16 <= len {
            let lo_mask128 = _mm_set1_epi8(0x0F);

            let mut lut128 = [_mm_setzero_si128(); 16];
            for h in 0u8..16 {
                lut128[h as usize] = _mm256_castsi256_si128(lut[h as usize]);
            }

            let input = _mm_loadu_si128(ptr.add(i) as *const _);
            let lo_nib = _mm_and_si128(input, lo_mask128);
            let hi_nib = _mm_and_si128(_mm_srli_epi16(input, 4), lo_mask128);

            let mut res = _mm_setzero_si128();
            macro_rules! do_nibble128 {
                ($h:expr) => {
                    let h_val = _mm_set1_epi8($h as i8);
                    let mask = _mm_cmpeq_epi8(hi_nib, h_val);
                    let looked_up = _mm_shuffle_epi8(lut128[$h], lo_nib);
                    res = _mm_or_si128(res, _mm_and_si128(mask, looked_up));
                };
            }
            do_nibble128!(0);
            do_nibble128!(1);
            do_nibble128!(2);
            do_nibble128!(3);
            do_nibble128!(4);
            do_nibble128!(5);
            do_nibble128!(6);
            do_nibble128!(7);
            do_nibble128!(8);
            do_nibble128!(9);
            do_nibble128!(10);
            do_nibble128!(11);
            do_nibble128!(12);
            do_nibble128!(13);
            do_nibble128!(14);
            do_nibble128!(15);

            _mm_storeu_si128(ptr.add(i) as *mut _, res);
            i += 16;
        }

        // Scalar tail
        while i < len {
            *ptr.add(i) = *table.get_unchecked(*ptr.add(i) as usize);
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn translate_inplace_ssse3_table(data: &mut [u8], table: &[u8; 256]) {
    use std::arch::x86_64::*;

    unsafe {
        let len = data.len();
        let ptr = data.as_mut_ptr();

        // Pre-build 16 lookup vectors for pshufb
        let mut lut = [_mm_setzero_si128(); 16];
        for h in 0u8..16 {
            let base = (h as usize) * 16;
            let row: [u8; 16] = std::array::from_fn(|i| *table.get_unchecked(base + i));
            lut[h as usize] = _mm_loadu_si128(row.as_ptr() as *const _);
        }

        let lo_mask = _mm_set1_epi8(0x0F);

        let mut i = 0;
        while i + 16 <= len {
            let input = _mm_loadu_si128(ptr.add(i) as *const _);
            let lo_nibble = _mm_and_si128(input, lo_mask);
            let hi_nibble = _mm_and_si128(_mm_srli_epi16(input, 4), lo_mask);

            let mut result = _mm_setzero_si128();

            macro_rules! do_nibble {
                ($h:expr) => {
                    let h_val = _mm_set1_epi8($h as i8);
                    let mask = _mm_cmpeq_epi8(hi_nibble, h_val);
                    let looked_up = _mm_shuffle_epi8(lut[$h], lo_nibble);
                    result = _mm_or_si128(result, _mm_and_si128(mask, looked_up));
                };
            }
            do_nibble!(0);
            do_nibble!(1);
            do_nibble!(2);
            do_nibble!(3);
            do_nibble!(4);
            do_nibble!(5);
            do_nibble!(6);
            do_nibble!(7);
            do_nibble!(8);
            do_nibble!(9);
            do_nibble!(10);
            do_nibble!(11);
            do_nibble!(12);
            do_nibble!(13);
            do_nibble!(14);
            do_nibble!(15);

            _mm_storeu_si128(ptr.add(i) as *mut _, result);
            i += 16;
        }

        // Scalar tail
        while i < len {
            *ptr.add(i) = *table.get_unchecked(*ptr.add(i) as usize);
            i += 1;
        }
    }
}

/// Translate bytes from source to destination using a 256-byte lookup table.
/// On x86_64 with SSSE3+, uses SIMD pshufb-based nibble decomposition.
#[inline(always)]
fn translate_to(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
    debug_assert!(dst.len() >= src.len());
    #[cfg(target_arch = "x86_64")]
    {
        let level = get_simd_level();
        if level >= 3 {
            unsafe { translate_to_avx2_table(src, dst, table) };
            return;
        }
        if level >= 2 {
            unsafe { translate_to_ssse3_table(src, dst, table) };
            return;
        }
    }
    translate_to_scalar(src, dst, table);
}

/// Scalar fallback for translate_to.
#[inline(always)]
fn translate_to_scalar(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
    unsafe {
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let len = src.len();
        let mut i = 0;
        while i + 8 <= len {
            *dp.add(i) = *table.get_unchecked(*sp.add(i) as usize);
            *dp.add(i + 1) = *table.get_unchecked(*sp.add(i + 1) as usize);
            *dp.add(i + 2) = *table.get_unchecked(*sp.add(i + 2) as usize);
            *dp.add(i + 3) = *table.get_unchecked(*sp.add(i + 3) as usize);
            *dp.add(i + 4) = *table.get_unchecked(*sp.add(i + 4) as usize);
            *dp.add(i + 5) = *table.get_unchecked(*sp.add(i + 5) as usize);
            *dp.add(i + 6) = *table.get_unchecked(*sp.add(i + 6) as usize);
            *dp.add(i + 7) = *table.get_unchecked(*sp.add(i + 7) as usize);
            i += 8;
        }
        while i < len {
            *dp.add(i) = *table.get_unchecked(*sp.add(i) as usize);
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_to_avx2_table(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
    use std::arch::x86_64::*;

    unsafe {
        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();

        // Pre-build 16 lookup vectors
        let mut lut = [_mm256_setzero_si256(); 16];
        for h in 0u8..16 {
            let base = (h as usize) * 16;
            let row: [u8; 16] = std::array::from_fn(|i| *table.get_unchecked(base + i));
            let row128 = _mm_loadu_si128(row.as_ptr() as *const _);
            lut[h as usize] = _mm256_broadcastsi128_si256(row128);
        }

        let lo_mask = _mm256_set1_epi8(0x0F);

        let mut i = 0;

        // 2x unrolled: process 64 bytes per iteration for better ILP
        while i + 64 <= len {
            let input0 = _mm256_loadu_si256(sp.add(i) as *const _);
            let input1 = _mm256_loadu_si256(sp.add(i + 32) as *const _);

            let lo0 = _mm256_and_si256(input0, lo_mask);
            let hi0 = _mm256_and_si256(_mm256_srli_epi16(input0, 4), lo_mask);
            let lo1 = _mm256_and_si256(input1, lo_mask);
            let hi1 = _mm256_and_si256(_mm256_srli_epi16(input1, 4), lo_mask);

            let mut r0 = _mm256_setzero_si256();
            let mut r1 = _mm256_setzero_si256();

            macro_rules! do_nibble2 {
                ($h:expr) => {
                    let h_val = _mm256_set1_epi8($h as i8);
                    let m0 = _mm256_cmpeq_epi8(hi0, h_val);
                    let l0 = _mm256_shuffle_epi8(lut[$h], lo0);
                    r0 = _mm256_or_si256(r0, _mm256_and_si256(m0, l0));
                    let m1 = _mm256_cmpeq_epi8(hi1, h_val);
                    let l1 = _mm256_shuffle_epi8(lut[$h], lo1);
                    r1 = _mm256_or_si256(r1, _mm256_and_si256(m1, l1));
                };
            }
            do_nibble2!(0);
            do_nibble2!(1);
            do_nibble2!(2);
            do_nibble2!(3);
            do_nibble2!(4);
            do_nibble2!(5);
            do_nibble2!(6);
            do_nibble2!(7);
            do_nibble2!(8);
            do_nibble2!(9);
            do_nibble2!(10);
            do_nibble2!(11);
            do_nibble2!(12);
            do_nibble2!(13);
            do_nibble2!(14);
            do_nibble2!(15);

            _mm256_storeu_si256(dp.add(i) as *mut _, r0);
            _mm256_storeu_si256(dp.add(i + 32) as *mut _, r1);
            i += 64;
        }

        // Remaining 32-byte chunk
        if i + 32 <= len {
            let input = _mm256_loadu_si256(sp.add(i) as *const _);
            let lo_nibble = _mm256_and_si256(input, lo_mask);
            let hi_nibble = _mm256_and_si256(_mm256_srli_epi16(input, 4), lo_mask);

            let mut result = _mm256_setzero_si256();

            macro_rules! do_nibble {
                ($h:expr) => {
                    let h_val = _mm256_set1_epi8($h as i8);
                    let mask = _mm256_cmpeq_epi8(hi_nibble, h_val);
                    let looked_up = _mm256_shuffle_epi8(lut[$h], lo_nibble);
                    result = _mm256_or_si256(result, _mm256_and_si256(mask, looked_up));
                };
            }
            do_nibble!(0);
            do_nibble!(1);
            do_nibble!(2);
            do_nibble!(3);
            do_nibble!(4);
            do_nibble!(5);
            do_nibble!(6);
            do_nibble!(7);
            do_nibble!(8);
            do_nibble!(9);
            do_nibble!(10);
            do_nibble!(11);
            do_nibble!(12);
            do_nibble!(13);
            do_nibble!(14);
            do_nibble!(15);

            _mm256_storeu_si256(dp.add(i) as *mut _, result);
            i += 32;
        }

        // SSSE3 tail for remaining 16-byte chunk
        if i + 16 <= len {
            let lo_mask128 = _mm_set1_epi8(0x0F);
            let mut lut128 = [_mm_setzero_si128(); 16];
            for h in 0u8..16 {
                lut128[h as usize] = _mm256_castsi256_si128(lut[h as usize]);
            }

            let input = _mm_loadu_si128(sp.add(i) as *const _);
            let lo_nib = _mm_and_si128(input, lo_mask128);
            let hi_nib = _mm_and_si128(_mm_srli_epi16(input, 4), lo_mask128);

            let mut res = _mm_setzero_si128();
            macro_rules! do_nibble128 {
                ($h:expr) => {
                    let h_val = _mm_set1_epi8($h as i8);
                    let mask = _mm_cmpeq_epi8(hi_nib, h_val);
                    let looked_up = _mm_shuffle_epi8(lut128[$h], lo_nib);
                    res = _mm_or_si128(res, _mm_and_si128(mask, looked_up));
                };
            }
            do_nibble128!(0);
            do_nibble128!(1);
            do_nibble128!(2);
            do_nibble128!(3);
            do_nibble128!(4);
            do_nibble128!(5);
            do_nibble128!(6);
            do_nibble128!(7);
            do_nibble128!(8);
            do_nibble128!(9);
            do_nibble128!(10);
            do_nibble128!(11);
            do_nibble128!(12);
            do_nibble128!(13);
            do_nibble128!(14);
            do_nibble128!(15);

            _mm_storeu_si128(dp.add(i) as *mut _, res);
            i += 16;
        }

        // Scalar tail
        while i < len {
            *dp.add(i) = *table.get_unchecked(*sp.add(i) as usize);
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn translate_to_ssse3_table(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
    use std::arch::x86_64::*;

    unsafe {
        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();

        let mut lut = [_mm_setzero_si128(); 16];
        for h in 0u8..16 {
            let base = (h as usize) * 16;
            let row: [u8; 16] = std::array::from_fn(|i| *table.get_unchecked(base + i));
            lut[h as usize] = _mm_loadu_si128(row.as_ptr() as *const _);
        }

        let lo_mask = _mm_set1_epi8(0x0F);

        let mut i = 0;
        while i + 16 <= len {
            let input = _mm_loadu_si128(sp.add(i) as *const _);
            let lo_nibble = _mm_and_si128(input, lo_mask);
            let hi_nibble = _mm_and_si128(_mm_srli_epi16(input, 4), lo_mask);

            let mut result = _mm_setzero_si128();

            macro_rules! do_nibble {
                ($h:expr) => {
                    let h_val = _mm_set1_epi8($h as i8);
                    let mask = _mm_cmpeq_epi8(hi_nibble, h_val);
                    let looked_up = _mm_shuffle_epi8(lut[$h], lo_nibble);
                    result = _mm_or_si128(result, _mm_and_si128(mask, looked_up));
                };
            }
            do_nibble!(0);
            do_nibble!(1);
            do_nibble!(2);
            do_nibble!(3);
            do_nibble!(4);
            do_nibble!(5);
            do_nibble!(6);
            do_nibble!(7);
            do_nibble!(8);
            do_nibble!(9);
            do_nibble!(10);
            do_nibble!(11);
            do_nibble!(12);
            do_nibble!(13);
            do_nibble!(14);
            do_nibble!(15);

            _mm_storeu_si128(dp.add(i) as *mut _, result);
            i += 16;
        }

        // Scalar tail
        while i < len {
            *dp.add(i) = *table.get_unchecked(*sp.add(i) as usize);
            i += 1;
        }
    }
}

// ============================================================================
// SIMD range translation (x86_64)
// ============================================================================

/// Detect if the translate table is a single contiguous range with constant offset.
/// Returns Some((lo, hi, offset)) if all non-identity entries form [lo..=hi] with
/// table[i] = i + offset for all i in [lo, hi].
#[inline]
fn detect_range_offset(table: &[u8; 256]) -> Option<(u8, u8, i8)> {
    let mut lo: Option<u8> = None;
    let mut hi = 0u8;
    let mut offset = 0i16;

    for i in 0..256 {
        if table[i] != i as u8 {
            let diff = table[i] as i16 - i as i16;
            match lo {
                None => {
                    lo = Some(i as u8);
                    hi = i as u8;
                    offset = diff;
                }
                Some(_) => {
                    if diff != offset || i as u8 != hi.wrapping_add(1) {
                        return None;
                    }
                    hi = i as u8;
                }
            }
        }
    }

    lo.map(|l| (l, hi, offset as i8))
}

/// Detect if the translate table maps a contiguous range [lo..=hi] to a single constant byte,
/// and all other bytes are identity. This covers cases like `tr '\000-\037' 'X'` where
/// a range maps to one replacement character.
/// Returns Some((lo, hi, replacement)) if the pattern matches.
#[inline]
fn detect_range_to_constant(table: &[u8; 256]) -> Option<(u8, u8, u8)> {
    let mut lo: Option<u8> = None;
    let mut hi = 0u8;
    let mut replacement = 0u8;

    for i in 0..256 {
        if table[i] != i as u8 {
            match lo {
                None => {
                    lo = Some(i as u8);
                    hi = i as u8;
                    replacement = table[i];
                }
                Some(_) => {
                    if table[i] != replacement || i as u8 != hi.wrapping_add(1) {
                        return None;
                    }
                    hi = i as u8;
                }
            }
        }
    }

    lo.map(|l| (l, hi, replacement))
}

/// SIMD-accelerated range-to-constant translation.
/// For tables where a contiguous range [lo..=hi] maps to a single byte, and all
/// other bytes are identity. Uses vectorized range check + blend (5 SIMD ops per
/// 32 bytes with AVX2, vs 48 for general nibble decomposition).
#[cfg(target_arch = "x86_64")]
fn translate_range_to_constant_simd_inplace(data: &mut [u8], lo: u8, hi: u8, replacement: u8) {
    if get_simd_level() >= 3 {
        unsafe { translate_range_to_constant_avx2_inplace(data, lo, hi, replacement) };
    } else {
        unsafe { translate_range_to_constant_sse2_inplace(data, lo, hi, replacement) };
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_range_to_constant_avx2_inplace(
    data: &mut [u8],
    lo: u8,
    hi: u8,
    replacement: u8,
) {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let repl_v = _mm256_set1_epi8(replacement as i8);
        let zero = _mm256_setzero_si256();

        let len = data.len();
        let ptr = data.as_mut_ptr();
        let mut i = 0;

        // 2x unrolled: process 64 bytes per iteration for better ILP
        while i + 64 <= len {
            let in0 = _mm256_loadu_si256(ptr.add(i) as *const _);
            let in1 = _mm256_loadu_si256(ptr.add(i + 32) as *const _);
            let bi0 = _mm256_add_epi8(in0, bias_v);
            let bi1 = _mm256_add_epi8(in1, bias_v);
            let gt0 = _mm256_cmpgt_epi8(bi0, threshold_v);
            let gt1 = _mm256_cmpgt_epi8(bi1, threshold_v);
            let ir0 = _mm256_cmpeq_epi8(gt0, zero);
            let ir1 = _mm256_cmpeq_epi8(gt1, zero);
            let r0 = _mm256_blendv_epi8(in0, repl_v, ir0);
            let r1 = _mm256_blendv_epi8(in1, repl_v, ir1);
            _mm256_storeu_si256(ptr.add(i) as *mut _, r0);
            _mm256_storeu_si256(ptr.add(i + 32) as *mut _, r1);
            i += 64;
        }

        // Remaining 32-byte chunk
        if i + 32 <= len {
            let input = _mm256_loadu_si256(ptr.add(i) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            let in_range = _mm256_cmpeq_epi8(gt, zero);
            let result = _mm256_blendv_epi8(input, repl_v, in_range);
            _mm256_storeu_si256(ptr.add(i) as *mut _, result);
            i += 32;
        }

        if i + 16 <= len {
            let bias_v128 = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
            let threshold_v128 = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
            let repl_v128 = _mm_set1_epi8(replacement as i8);
            let zero128 = _mm_setzero_si128();

            let input = _mm_loadu_si128(ptr.add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v128);
            let gt = _mm_cmpgt_epi8(biased, threshold_v128);
            let in_range = _mm_cmpeq_epi8(gt, zero128);
            let result = _mm_blendv_epi8(input, repl_v128, in_range);
            _mm_storeu_si128(ptr.add(i) as *mut _, result);
            i += 16;
        }

        while i < len {
            let b = *ptr.add(i);
            *ptr.add(i) = if b >= lo && b <= hi { replacement } else { b };
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn translate_range_to_constant_sse2_inplace(
    data: &mut [u8],
    lo: u8,
    hi: u8,
    replacement: u8,
) {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let repl_v = _mm_set1_epi8(replacement as i8);
        let zero = _mm_setzero_si128();

        let len = data.len();
        let ptr = data.as_mut_ptr();
        let mut i = 0;

        while i + 16 <= len {
            let input = _mm_loadu_si128(ptr.add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v);
            let gt = _mm_cmpgt_epi8(biased, threshold_v);
            // in_range mask: 0xFF where in range, 0x00 where not
            let in_range = _mm_cmpeq_epi8(gt, zero);
            // SSE2 blendv: (repl & mask) | (input & ~mask)
            let result = _mm_or_si128(
                _mm_and_si128(in_range, repl_v),
                _mm_andnot_si128(in_range, input),
            );
            _mm_storeu_si128(ptr.add(i) as *mut _, result);
            i += 16;
        }

        while i < len {
            let b = *ptr.add(i);
            *ptr.add(i) = if b >= lo && b <= hi { replacement } else { b };
            i += 1;
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn translate_range_to_constant_simd_inplace(data: &mut [u8], lo: u8, hi: u8, replacement: u8) {
    for b in data.iter_mut() {
        if *b >= lo && *b <= hi {
            *b = replacement;
        }
    }
}

/// SIMD-accelerated range translation for mmap'd data.
/// For tables where only a contiguous range [lo..=hi] is translated by a constant offset,
/// uses AVX2 (32 bytes/iter) or SSE2 (16 bytes/iter) vectorized arithmetic.
#[cfg(target_arch = "x86_64")]
fn translate_range_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    if get_simd_level() >= 3 {
        unsafe { translate_range_avx2(src, dst, lo, hi, offset) };
    } else {
        unsafe { translate_range_sse2(src, dst, lo, hi, offset) };
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_range_avx2(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        // Bias: shift range so lo maps to -128 (signed min).
        // For input in [lo, hi]: biased = input + (0x80 - lo) is in [-128, -128+range].
        // For input < lo: biased wraps to large positive (signed), > threshold.
        // For input > hi: biased > -128+range, > threshold.
        let bias_v = _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let offset_v = _mm256_set1_epi8(offset);
        let zero = _mm256_setzero_si256();

        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let mut i = 0;

        // 2x unrolled: process 64 bytes per iteration for better ILP.
        // Load/compute on the second vector while the first is in-flight.
        while i + 64 <= len {
            let in0 = _mm256_loadu_si256(sp.add(i) as *const _);
            let in1 = _mm256_loadu_si256(sp.add(i + 32) as *const _);
            let bi0 = _mm256_add_epi8(in0, bias_v);
            let bi1 = _mm256_add_epi8(in1, bias_v);
            let gt0 = _mm256_cmpgt_epi8(bi0, threshold_v);
            let gt1 = _mm256_cmpgt_epi8(bi1, threshold_v);
            let m0 = _mm256_cmpeq_epi8(gt0, zero);
            let m1 = _mm256_cmpeq_epi8(gt1, zero);
            let om0 = _mm256_and_si256(m0, offset_v);
            let om1 = _mm256_and_si256(m1, offset_v);
            let r0 = _mm256_add_epi8(in0, om0);
            let r1 = _mm256_add_epi8(in1, om1);
            _mm256_storeu_si256(dp.add(i) as *mut _, r0);
            _mm256_storeu_si256(dp.add(i + 32) as *mut _, r1);
            i += 64;
        }

        // Remaining 32-byte chunk
        if i + 32 <= len {
            let input = _mm256_loadu_si256(sp.add(i) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            let mask = _mm256_cmpeq_epi8(gt, zero);
            let offset_masked = _mm256_and_si256(mask, offset_v);
            let result = _mm256_add_epi8(input, offset_masked);
            _mm256_storeu_si256(dp.add(i) as *mut _, result);
            i += 32;
        }

        // SSE2 tail for 16-byte remainder
        if i + 16 <= len {
            let bias_v128 = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
            let threshold_v128 = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
            let offset_v128 = _mm_set1_epi8(offset);
            let zero128 = _mm_setzero_si128();

            let input = _mm_loadu_si128(sp.add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v128);
            let gt = _mm_cmpgt_epi8(biased, threshold_v128);
            let mask = _mm_cmpeq_epi8(gt, zero128);
            let offset_masked = _mm_and_si128(mask, offset_v128);
            let result = _mm_add_epi8(input, offset_masked);
            _mm_storeu_si128(dp.add(i) as *mut _, result);
            i += 16;
        }

        // Scalar tail
        while i < len {
            let b = *sp.add(i);
            *dp.add(i) = if b >= lo && b <= hi {
                b.wrapping_add(offset as u8)
            } else {
                b
            };
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn translate_range_sse2(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let offset_v = _mm_set1_epi8(offset);
        let zero = _mm_setzero_si128();

        let len = src.len();
        let mut i = 0;

        while i + 16 <= len {
            let input = _mm_loadu_si128(src.as_ptr().add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v);
            let gt = _mm_cmpgt_epi8(biased, threshold_v);
            let mask = _mm_cmpeq_epi8(gt, zero);
            let offset_masked = _mm_and_si128(mask, offset_v);
            let result = _mm_add_epi8(input, offset_masked);
            _mm_storeu_si128(dst.as_mut_ptr().add(i) as *mut _, result);
            i += 16;
        }

        while i < len {
            let b = *src.get_unchecked(i);
            *dst.get_unchecked_mut(i) = if b >= lo && b <= hi {
                b.wrapping_add(offset as u8)
            } else {
                b
            };
            i += 1;
        }
    }
}

/// Scalar range translation fallback for non-x86_64.
#[cfg(not(target_arch = "x86_64"))]
fn translate_range_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    for (i, &b) in src.iter().enumerate() {
        dst[i] = if b >= lo && b <= hi {
            b.wrapping_add(offset as u8)
        } else {
            b
        };
    }
}

// ============================================================================
// In-place SIMD range translation (saves one buffer allocation in streaming)
// ============================================================================

/// In-place SIMD-accelerated range translation.
/// Translates bytes in [lo..=hi] by adding `offset`, leaving others unchanged.
/// Operates on the buffer in-place, eliminating the need for a separate output buffer.
#[cfg(target_arch = "x86_64")]
fn translate_range_simd_inplace(data: &mut [u8], lo: u8, hi: u8, offset: i8) {
    if get_simd_level() >= 3 {
        unsafe { translate_range_avx2_inplace(data, lo, hi, offset) };
    } else {
        unsafe { translate_range_sse2_inplace(data, lo, hi, offset) };
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_range_avx2_inplace(data: &mut [u8], lo: u8, hi: u8, offset: i8) {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let offset_v = _mm256_set1_epi8(offset);
        let zero = _mm256_setzero_si256();

        let len = data.len();
        let ptr = data.as_mut_ptr();
        let mut i = 0;

        // 2x unrolled: process 64 bytes per iteration for better ILP
        while i + 64 <= len {
            let in0 = _mm256_loadu_si256(ptr.add(i) as *const _);
            let in1 = _mm256_loadu_si256(ptr.add(i + 32) as *const _);
            let bi0 = _mm256_add_epi8(in0, bias_v);
            let bi1 = _mm256_add_epi8(in1, bias_v);
            let gt0 = _mm256_cmpgt_epi8(bi0, threshold_v);
            let gt1 = _mm256_cmpgt_epi8(bi1, threshold_v);
            let m0 = _mm256_cmpeq_epi8(gt0, zero);
            let m1 = _mm256_cmpeq_epi8(gt1, zero);
            let om0 = _mm256_and_si256(m0, offset_v);
            let om1 = _mm256_and_si256(m1, offset_v);
            let r0 = _mm256_add_epi8(in0, om0);
            let r1 = _mm256_add_epi8(in1, om1);
            _mm256_storeu_si256(ptr.add(i) as *mut _, r0);
            _mm256_storeu_si256(ptr.add(i + 32) as *mut _, r1);
            i += 64;
        }

        // Remaining 32-byte chunk
        if i + 32 <= len {
            let input = _mm256_loadu_si256(ptr.add(i) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            let mask = _mm256_cmpeq_epi8(gt, zero);
            let offset_masked = _mm256_and_si256(mask, offset_v);
            let result = _mm256_add_epi8(input, offset_masked);
            _mm256_storeu_si256(ptr.add(i) as *mut _, result);
            i += 32;
        }

        if i + 16 <= len {
            let bias_v128 = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
            let threshold_v128 = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
            let offset_v128 = _mm_set1_epi8(offset);
            let zero128 = _mm_setzero_si128();

            let input = _mm_loadu_si128(ptr.add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v128);
            let gt = _mm_cmpgt_epi8(biased, threshold_v128);
            let mask = _mm_cmpeq_epi8(gt, zero128);
            let offset_masked = _mm_and_si128(mask, offset_v128);
            let result = _mm_add_epi8(input, offset_masked);
            _mm_storeu_si128(ptr.add(i) as *mut _, result);
            i += 16;
        }

        while i < len {
            let b = *ptr.add(i);
            *ptr.add(i) = if b >= lo && b <= hi {
                b.wrapping_add(offset as u8)
            } else {
                b
            };
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn translate_range_sse2_inplace(data: &mut [u8], lo: u8, hi: u8, offset: i8) {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let offset_v = _mm_set1_epi8(offset);
        let zero = _mm_setzero_si128();

        let len = data.len();
        let ptr = data.as_mut_ptr();
        let mut i = 0;

        while i + 16 <= len {
            let input = _mm_loadu_si128(ptr.add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v);
            let gt = _mm_cmpgt_epi8(biased, threshold_v);
            let mask = _mm_cmpeq_epi8(gt, zero);
            let offset_masked = _mm_and_si128(mask, offset_v);
            let result = _mm_add_epi8(input, offset_masked);
            _mm_storeu_si128(ptr.add(i) as *mut _, result);
            i += 16;
        }

        while i < len {
            let b = *ptr.add(i);
            *ptr.add(i) = if b >= lo && b <= hi {
                b.wrapping_add(offset as u8)
            } else {
                b
            };
            i += 1;
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn translate_range_simd_inplace(data: &mut [u8], lo: u8, hi: u8, offset: i8) {
    for b in data.iter_mut() {
        if *b >= lo && *b <= hi {
            *b = b.wrapping_add(offset as u8);
        }
    }
}

// ============================================================================
// SIMD range deletion (x86_64)
// ============================================================================

/// Detect if ALL delete characters form a single contiguous byte range [lo..=hi].
/// Returns Some((lo, hi)) if so. This is true for common classes:
/// - `[:digit:]` = 0x30..=0x39
/// - `a-z` = 0x61..=0x7A
/// - `A-Z` = 0x41..=0x5A
#[inline]
fn detect_delete_range(chars: &[u8]) -> Option<(u8, u8)> {
    if chars.is_empty() {
        return None;
    }
    let mut lo = chars[0];
    let mut hi = chars[0];
    for &c in &chars[1..] {
        if c < lo {
            lo = c;
        }
        if c > hi {
            hi = c;
        }
    }
    // Check that the range size matches the number of chars (no gaps)
    if (hi - lo + 1) as usize == chars.len() {
        Some((lo, hi))
    } else {
        None
    }
}

/// SIMD-accelerated delete for contiguous byte ranges.
/// Uses the same bias+threshold trick as range translate to identify bytes in [lo..=hi],
/// then compacts output by skipping matched bytes.
#[cfg(target_arch = "x86_64")]
fn delete_range_chunk(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) -> usize {
    if get_simd_level() >= 3 {
        unsafe { delete_range_avx2(src, dst, lo, hi) }
    } else {
        unsafe { delete_range_sse2(src, dst, lo, hi) }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn delete_range_avx2(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) -> usize {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let zero = _mm256_setzero_si256();

        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let mut ri = 0;
        let mut wp = 0;

        while ri + 32 <= len {
            let input = _mm256_loadu_si256(sp.add(ri) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            // gt = 0xFF where biased > threshold (OUT of range = KEEP)
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            // in_range = 0xFF where IN range (to DELETE), 0 where to KEEP
            let in_range = _mm256_cmpeq_epi8(gt, zero);
            // keep_mask bits: 1 = keep (NOT in range)
            let keep_mask = !(_mm256_movemask_epi8(in_range) as u32);

            if keep_mask == 0xFFFFFFFF {
                // All 32 bytes are kept — bulk copy
                std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 32);
                wp += 32;
            } else if keep_mask != 0 {
                // Partial keep — process each 8-byte lane with popcnt
                compact_8bytes(sp.add(ri), dp.add(wp), keep_mask as u8);
                let c0 = (keep_mask as u8).count_ones() as usize;
                compact_8bytes(sp.add(ri + 8), dp.add(wp + c0), (keep_mask >> 8) as u8);
                let c1 = ((keep_mask >> 8) as u8).count_ones() as usize;
                compact_8bytes(
                    sp.add(ri + 16),
                    dp.add(wp + c0 + c1),
                    (keep_mask >> 16) as u8,
                );
                let c2 = ((keep_mask >> 16) as u8).count_ones() as usize;
                compact_8bytes(
                    sp.add(ri + 24),
                    dp.add(wp + c0 + c1 + c2),
                    (keep_mask >> 24) as u8,
                );
                let c3 = ((keep_mask >> 24) as u8).count_ones() as usize;
                wp += c0 + c1 + c2 + c3;
            }
            // else: keep_mask == 0 means all bytes deleted, skip entirely
            ri += 32;
        }

        // SSE2 tail for 16-byte remainder
        if ri + 16 <= len {
            let bias_v128 = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
            let threshold_v128 = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
            let zero128 = _mm_setzero_si128();

            let input = _mm_loadu_si128(sp.add(ri) as *const _);
            let biased = _mm_add_epi8(input, bias_v128);
            let gt = _mm_cmpgt_epi8(biased, threshold_v128);
            let in_range = _mm_cmpeq_epi8(gt, zero128);
            let keep_mask = !(_mm_movemask_epi8(in_range) as u32) & 0xFFFF;

            if keep_mask == 0xFFFF {
                std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 16);
                wp += 16;
            } else if keep_mask != 0 {
                compact_8bytes(sp.add(ri), dp.add(wp), keep_mask as u8);
                let c0 = (keep_mask as u8).count_ones() as usize;
                compact_8bytes(sp.add(ri + 8), dp.add(wp + c0), (keep_mask >> 8) as u8);
                wp += c0 + ((keep_mask >> 8) as u8).count_ones() as usize;
            }
            ri += 16;
        }

        // Scalar tail
        while ri < len {
            let b = *sp.add(ri);
            if b < lo || b > hi {
                *dp.add(wp) = b;
                wp += 1;
            }
            ri += 1;
        }

        wp
    }
}

/// Compact 8 source bytes into contiguous output bytes using a keep mask.
/// Each bit in `mask` indicates whether the corresponding byte should be kept.
/// Uses branchless writes: always writes 8 bytes (the extras are harmless since
/// the caller tracks the write pointer by popcount).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn compact_8bytes(src: *const u8, dst: *mut u8, mask: u8) {
    // For each set bit position in the mask, copy that byte from src to the
    // next output position. Using a tight trailing_zeros loop is faster than
    // a lookup table for 8-bit masks because it's branch-predicted well
    // and avoids cache misses on the table.
    unsafe {
        let mut m = mask;
        let mut w = 0;
        while m != 0 {
            let bit = m.trailing_zeros() as usize;
            *dst.add(w) = *src.add(bit);
            w += 1;
            m &= m - 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn delete_range_sse2(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) -> usize {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let zero = _mm_setzero_si128();

        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let mut ri = 0;
        let mut wp = 0;

        while ri + 16 <= len {
            let input = _mm_loadu_si128(sp.add(ri) as *const _);
            let biased = _mm_add_epi8(input, bias_v);
            let gt = _mm_cmpgt_epi8(biased, threshold_v);
            let in_range = _mm_cmpeq_epi8(gt, zero);
            let keep_mask = !(_mm_movemask_epi8(in_range) as u32) & 0xFFFF;

            if keep_mask == 0xFFFF {
                // All 16 bytes kept — bulk copy
                std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 16);
                wp += 16;
            } else if keep_mask != 0 {
                compact_8bytes(sp.add(ri), dp.add(wp), keep_mask as u8);
                let c0 = (keep_mask as u8).count_ones() as usize;
                compact_8bytes(sp.add(ri + 8), dp.add(wp + c0), (keep_mask >> 8) as u8);
                wp += c0 + ((keep_mask >> 8) as u8).count_ones() as usize;
            }
            ri += 16;
        }

        while ri < len {
            let b = *sp.add(ri);
            if b < lo || b > hi {
                *dp.add(wp) = b;
                wp += 1;
            }
            ri += 1;
        }

        wp
    }
}

/// Scalar range delete fallback for non-x86_64.
#[cfg(not(target_arch = "x86_64"))]
fn delete_range_chunk(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) -> usize {
    let mut wp = 0;
    for &b in src {
        if b < lo || b > hi {
            dst[wp] = b;
            wp += 1;
        }
    }
    wp
}

/// Streaming delete for contiguous byte ranges using SIMD range detection.
/// Uses 4MB buffer to reduce syscalls (delete is compute-light, I/O bound).
/// When no bytes are deleted from a chunk (common for data with few matches),
/// writes directly from the source buffer to avoid the copy overhead.
fn delete_range_streaming(
    lo: u8,
    hi: u8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut src = alloc_uninit_vec(STREAM_BUF);
    let mut dst = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut src)?;
        if n == 0 {
            break;
        }
        let wp = delete_range_chunk(&src[..n], &mut dst, lo, hi);
        if wp == n {
            // No bytes deleted — write source directly (avoids copy overhead)
            writer.write_all(&src[..n])?;
        } else if wp > 0 {
            writer.write_all(&dst[..wp])?;
        }
    }
    Ok(())
}

// ============================================================================
// Streaming functions (Read + Write)
// ============================================================================

pub fn translate(
    set1: &[u8],
    set2: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);

    // Check for identity table — pure passthrough (no transformation needed)
    let is_identity = table.iter().enumerate().all(|(i, &v)| v == i as u8);
    if is_identity {
        return passthrough_stream(reader, writer);
    }

    // Try SIMD fast path for constant-offset range translations (in-place, single buffer)
    if let Some((lo, hi, offset)) = detect_range_offset(&table) {
        return translate_range_stream(lo, hi, offset, reader, writer);
    }

    // Try SIMD fast path for range-to-constant translations (e.g., '\000-\037' -> 'X').
    // Uses blendv (5 SIMD ops/32 bytes) instead of nibble decomposition (48 ops/32 bytes).
    if let Some((lo, hi, replacement)) = detect_range_to_constant(&table) {
        return translate_range_to_constant_stream(lo, hi, replacement, reader, writer);
    }

    // General case: IN-PLACE translation on a SINGLE 16MB buffer.
    // This halves memory bandwidth vs the old separate src/dst approach:
    // - Old: read into src, translate from src→dst (read + write), write dst = 12MB bandwidth
    // - New: read into buf, translate in-place (read+write), write buf = 8MB bandwidth
    // The 8x-unrolled in-place translate avoids store-to-load forwarding stalls
    // because consecutive reads are 8 bytes apart (sequential), not aliased.
    // Using 16MB buffer = 1 read for 10MB input, minimizing syscall count.
    // SAFETY: all bytes are written by read_full before being translated.
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        translate_inplace(&mut buf[..n], &table);
        writer.write_all(&buf[..n])?;
    }
    Ok(())
}

/// Streaming SIMD range translation — single buffer, in-place transform.
/// Uses 16MB uninit buffer for fewer syscalls (translate is compute-light).
fn translate_range_stream(
    lo: u8,
    hi: u8,
    offset: i8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        translate_range_simd_inplace(&mut buf[..n], lo, hi, offset);
        writer.write_all(&buf[..n])?;
    }
    Ok(())
}

/// Streaming SIMD range-to-constant translation — single buffer, in-place transform.
/// Uses blendv instead of nibble decomposition for ~10x fewer SIMD ops per vector.
fn translate_range_to_constant_stream(
    lo: u8,
    hi: u8,
    replacement: u8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        translate_range_to_constant_simd_inplace(&mut buf[..n], lo, hi, replacement);
        writer.write_all(&buf[..n])?;
    }
    Ok(())
}

/// Pure passthrough: copy stdin to stdout without transformation.
/// Uses a single 16MB uninit buffer with direct read/write, no processing overhead.
fn passthrough_stream(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
    }
    Ok(())
}

/// Read as many bytes as possible into buf, retrying on partial reads.
/// Fast path: first read() often fills the entire buffer for regular files.
#[inline]
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    // Fast path: first read() usually fills the entire buffer for regular files
    let n = reader.read(buf)?;
    if n == buf.len() || n == 0 {
        return Ok(n);
    }
    // Slow path: partial read — retry to fill buffer (pipes, slow devices)
    let mut total = n;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

pub fn translate_squeeze(
    set1: &[u8],
    set2: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);
    let squeeze_set = build_member_set(set2);

    // For single-char squeeze set with range-to-constant translation, use
    // fused approach: translate via SIMD, then use memmem to find squeeze points.
    if set2.len() == 1 || (set2.len() > 1 && set2.iter().all(|&b| b == set2[0])) {
        let squeeze_ch = set2.last().copied().unwrap_or(0);
        return translate_squeeze_single_ch(&table, squeeze_ch, &squeeze_set, reader, writer);
    }

    // Two-pass optimization for range translations:
    // Pass 1: SIMD range translate in-place (10x faster than scalar table lookup)
    // Pass 2: scalar squeeze (inherently sequential due to state dependency)
    let range_info = detect_range_offset(&table);
    let range_const_info = if range_info.is_none() {
        detect_range_to_constant(&table)
    } else {
        None
    };

    let mut buf = alloc_uninit_vec(STREAM_BUF);
    let mut last_squeezed: u16 = 256;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        // Pass 1: translate
        if let Some((lo, hi, offset)) = range_info {
            translate_range_simd_inplace(&mut buf[..n], lo, hi, offset);
        } else if let Some((lo, hi, replacement)) = range_const_info {
            translate_range_to_constant_simd_inplace(&mut buf[..n], lo, hi, replacement);
        } else {
            translate_inplace(&mut buf[..n], &table);
        }
        // Pass 2: squeeze in-place using 8x-unrolled loop
        let mut wp = 0;
        unsafe {
            let ptr = buf.as_mut_ptr();
            let mut i = 0;
            while i + 8 <= n {
                macro_rules! squeeze_byte {
                    ($off:expr) => {
                        let b = *ptr.add(i + $off);
                        if is_member(&squeeze_set, b) {
                            if last_squeezed != b as u16 {
                                last_squeezed = b as u16;
                                *ptr.add(wp) = b;
                                wp += 1;
                            }
                        } else {
                            last_squeezed = 256;
                            *ptr.add(wp) = b;
                            wp += 1;
                        }
                    };
                }
                squeeze_byte!(0);
                squeeze_byte!(1);
                squeeze_byte!(2);
                squeeze_byte!(3);
                squeeze_byte!(4);
                squeeze_byte!(5);
                squeeze_byte!(6);
                squeeze_byte!(7);
                i += 8;
            }
            while i < n {
                let b = *ptr.add(i);
                if is_member(&squeeze_set, b) {
                    if last_squeezed == b as u16 {
                        i += 1;
                        continue;
                    }
                    last_squeezed = b as u16;
                } else {
                    last_squeezed = 256;
                }
                *ptr.add(wp) = b;
                wp += 1;
                i += 1;
            }
        }
        writer.write_all(&buf[..wp])?;
    }
    Ok(())
}

/// Optimized translate+squeeze for single squeeze character.
/// After SIMD translation, uses memmem to find consecutive pairs
/// of the squeeze char and does zero-copy writev from the buffer.
fn translate_squeeze_single_ch(
    table: &[u8; 256],
    squeeze_ch: u8,
    _squeeze_set: &[u8; 32],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let range_info = detect_range_offset(table);
    let range_const_info = if range_info.is_none() {
        detect_range_to_constant(table)
    } else {
        None
    };

    let pair = [squeeze_ch, squeeze_ch];
    let finder = memchr::memmem::Finder::new(&pair);
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    let mut was_squeeze_char = false;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        // Pass 1: SIMD translate in-place
        if let Some((lo, hi, offset)) = range_info {
            translate_range_simd_inplace(&mut buf[..n], lo, hi, offset);
        } else if let Some((lo, hi, replacement)) = range_const_info {
            translate_range_to_constant_simd_inplace(&mut buf[..n], lo, hi, replacement);
        } else {
            translate_inplace(&mut buf[..n], table);
        }

        // Pass 2: squeeze using memmem pair-finding + writev (zero-copy)
        let mut i = 0;

        // Handle carry-over from previous chunk
        if was_squeeze_char {
            while i < n && unsafe { *buf.as_ptr().add(i) } == squeeze_ch {
                i += 1;
            }
            if i >= n {
                continue;
            }
        }

        let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(64);

        loop {
            match finder.find(&buf[i..n]) {
                Some(offset) => {
                    let seg_end = i + offset + 1;
                    if seg_end > i {
                        iov.push(std::io::IoSlice::new(&buf[i..seg_end]));
                    }
                    i = seg_end;
                    while i < n && unsafe { *buf.as_ptr().add(i) } == squeeze_ch {
                        i += 1;
                    }
                    if i >= n {
                        was_squeeze_char = true;
                        break;
                    }
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                None => {
                    if i < n {
                        iov.push(std::io::IoSlice::new(&buf[i..n]));
                    }
                    was_squeeze_char = n > 0 && unsafe { *buf.as_ptr().add(n - 1) } == squeeze_ch;
                    break;
                }
            }
        }

        if !iov.is_empty() {
            write_ioslices(writer, &iov)?;
        }
    }
    Ok(())
}

pub fn delete(
    delete_chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    if delete_chars.len() == 1 {
        return delete_single_streaming(delete_chars[0], reader, writer);
    }
    if delete_chars.len() <= 3 {
        return delete_multi_streaming(delete_chars, reader, writer);
    }

    // SIMD fast path: if all delete chars form a contiguous range [lo..=hi],
    // use vectorized range comparison instead of scalar bitset lookup.
    // This covers [:digit:] (0x30-0x39), a-z, A-Z, etc.
    if let Some((lo, hi)) = detect_delete_range(delete_chars) {
        return delete_range_streaming(lo, hi, reader, writer);
    }

    let member = build_member_set(delete_chars);
    let mut buf = alloc_uninit_vec(STREAM_BUF);

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut wp = 0;
        unsafe {
            let ptr = buf.as_mut_ptr();
            let mut i = 0;
            while i + 8 <= n {
                let b0 = *ptr.add(i);
                let b1 = *ptr.add(i + 1);
                let b2 = *ptr.add(i + 2);
                let b3 = *ptr.add(i + 3);
                let b4 = *ptr.add(i + 4);
                let b5 = *ptr.add(i + 5);
                let b6 = *ptr.add(i + 6);
                let b7 = *ptr.add(i + 7);

                // Branchless: write byte then conditionally advance pointer.
                // Avoids branch mispredictions when most bytes are kept.
                *ptr.add(wp) = b0;
                wp += !is_member(&member, b0) as usize;
                *ptr.add(wp) = b1;
                wp += !is_member(&member, b1) as usize;
                *ptr.add(wp) = b2;
                wp += !is_member(&member, b2) as usize;
                *ptr.add(wp) = b3;
                wp += !is_member(&member, b3) as usize;
                *ptr.add(wp) = b4;
                wp += !is_member(&member, b4) as usize;
                *ptr.add(wp) = b5;
                wp += !is_member(&member, b5) as usize;
                *ptr.add(wp) = b6;
                wp += !is_member(&member, b6) as usize;
                *ptr.add(wp) = b7;
                wp += !is_member(&member, b7) as usize;
                i += 8;
            }
            while i < n {
                let b = *ptr.add(i);
                *ptr.add(wp) = b;
                wp += !is_member(&member, b) as usize;
                i += 1;
            }
        }
        writer.write_all(&buf[..wp])?;
    }
    Ok(())
}

fn delete_single_streaming(
    ch: u8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut src = alloc_uninit_vec(STREAM_BUF);
    let mut dst = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut src)?;
        if n == 0 {
            break;
        }
        // Use memchr to find byte positions, gap-copy to separate dst buffer.
        // Separate src/dst allows copy_nonoverlapping (faster than memmove)
        // and avoids aliasing concerns in the hot loop.
        let mut wp = 0;
        let mut i = 0;
        while i < n {
            match memchr::memchr(ch, &src[i..n]) {
                Some(offset) => {
                    if offset > 0 {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                src.as_ptr().add(i),
                                dst.as_mut_ptr().add(wp),
                                offset,
                            );
                        }
                        wp += offset;
                    }
                    i += offset + 1;
                }
                None => {
                    let run_len = n - i;
                    if run_len > 0 {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                src.as_ptr().add(i),
                                dst.as_mut_ptr().add(wp),
                                run_len,
                            );
                        }
                        wp += run_len;
                    }
                    break;
                }
            }
        }
        // If nothing was deleted, write from src directly (avoids extra copy)
        if wp == n {
            writer.write_all(&src[..n])?;
        } else if wp > 0 {
            writer.write_all(&dst[..wp])?;
        }
    }
    Ok(())
}

fn delete_multi_streaming(
    chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut src = alloc_uninit_vec(STREAM_BUF);
    let mut dst = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut src)?;
        if n == 0 {
            break;
        }
        // Use memchr2/memchr3 to find byte positions, gap-copy to separate dst buffer.
        // Separate src/dst allows copy_nonoverlapping (faster than memmove).
        let mut wp = 0;
        let mut i = 0;
        while i < n {
            let found = if chars.len() == 2 {
                memchr::memchr2(chars[0], chars[1], &src[i..n])
            } else {
                memchr::memchr3(chars[0], chars[1], chars[2], &src[i..n])
            };
            match found {
                Some(offset) => {
                    if offset > 0 {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                src.as_ptr().add(i),
                                dst.as_mut_ptr().add(wp),
                                offset,
                            );
                        }
                        wp += offset;
                    }
                    i += offset + 1;
                }
                None => {
                    let run_len = n - i;
                    if run_len > 0 {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                src.as_ptr().add(i),
                                dst.as_mut_ptr().add(wp),
                                run_len,
                            );
                        }
                        wp += run_len;
                    }
                    break;
                }
            }
        }
        if wp == n {
            writer.write_all(&src[..n])?;
        } else if wp > 0 {
            writer.write_all(&dst[..wp])?;
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
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    let mut last_squeezed: u16 = 256;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        // Fused delete+squeeze: 8x-unrolled inner loop for better ILP.
        // Each byte is checked against delete set first (skip if member),
        // then squeeze set (deduplicate consecutive members).
        let mut wp = 0;
        unsafe {
            let ptr = buf.as_mut_ptr();
            let mut i = 0;
            while i + 8 <= n {
                macro_rules! process_byte {
                    ($off:expr) => {
                        let b = *ptr.add(i + $off);
                        if !is_member(&delete_set, b) {
                            if is_member(&squeeze_set, b) {
                                if last_squeezed != b as u16 {
                                    last_squeezed = b as u16;
                                    *ptr.add(wp) = b;
                                    wp += 1;
                                }
                            } else {
                                last_squeezed = 256;
                                *ptr.add(wp) = b;
                                wp += 1;
                            }
                        }
                    };
                }
                process_byte!(0);
                process_byte!(1);
                process_byte!(2);
                process_byte!(3);
                process_byte!(4);
                process_byte!(5);
                process_byte!(6);
                process_byte!(7);
                i += 8;
            }
            while i < n {
                let b = *ptr.add(i);
                if !is_member(&delete_set, b) {
                    if is_member(&squeeze_set, b) {
                        if last_squeezed != b as u16 {
                            last_squeezed = b as u16;
                            *ptr.add(wp) = b;
                            wp += 1;
                        }
                    } else {
                        last_squeezed = 256;
                        *ptr.add(wp) = b;
                        wp += 1;
                    }
                }
                i += 1;
            }
        }
        writer.write_all(&buf[..wp])?;
    }
    Ok(())
}

pub fn squeeze(
    squeeze_chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    if squeeze_chars.len() == 1 {
        return squeeze_single_stream(squeeze_chars[0], reader, writer);
    }

    // For 2-3 squeeze chars, use memchr2/memchr3-based gap-copy
    // which gives SIMD-accelerated scanning instead of byte-at-a-time.
    if squeeze_chars.len() <= 3 {
        return squeeze_multi_stream(squeeze_chars, reader, writer);
    }

    let member = build_member_set(squeeze_chars);
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    let mut last_squeezed: u16 = 256;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut wp = 0;
        unsafe {
            let ptr = buf.as_mut_ptr();
            for i in 0..n {
                let b = *ptr.add(i);
                if is_member(&member, b) {
                    if last_squeezed == b as u16 {
                        continue;
                    }
                    last_squeezed = b as u16;
                } else {
                    last_squeezed = 256;
                }
                *ptr.add(wp) = b;
                wp += 1;
            }
        }
        writer.write_all(&buf[..wp])?;
    }
    Ok(())
}

/// Streaming squeeze for 2-3 chars using memchr2/memchr3 SIMD scanning.
/// Builds writev IoSlice entries pointing into the read buffer, skipping
/// duplicate runs of squeezable characters. Zero-copy between squeeze points.
fn squeeze_multi_stream(
    chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let c0 = chars[0];
    let c1 = chars[1];
    let c2 = if chars.len() >= 3 {
        Some(chars[2])
    } else {
        None
    };
    let single_byte = [0u8; 1]; // used for the kept single byte
    let _ = single_byte;

    let mut buf = alloc_uninit_vec(STREAM_BUF);
    let mut last_squeezed: u16 = 256;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }

        // In-place compaction using memchr2/memchr3 gap-copy.
        // For each squeezable char found, copy the gap before it,
        // then emit one byte (if not a squeeze duplicate) and skip the run.
        let ptr = buf.as_mut_ptr();
        let mut wp = 0usize;
        let mut cursor = 0usize;

        macro_rules! find_next {
            ($start:expr) => {
                if let Some(c) = c2 {
                    memchr::memchr3(c0, c1, c, &buf[$start..n])
                } else {
                    memchr::memchr2(c0, c1, &buf[$start..n])
                }
            };
        }

        while cursor < n {
            match find_next!(cursor) {
                Some(offset) => {
                    let pos = cursor + offset;
                    let b = unsafe { *ptr.add(pos) };

                    // Copy gap before squeeze point
                    let gap = pos - cursor;
                    if gap > 0 {
                        if wp != cursor {
                            unsafe {
                                std::ptr::copy(ptr.add(cursor), ptr.add(wp), gap);
                            }
                        }
                        wp += gap;
                        last_squeezed = 256;
                    }

                    // Emit single byte if not duplicate
                    if last_squeezed != b as u16 {
                        unsafe { *ptr.add(wp) = b };
                        wp += 1;
                        last_squeezed = b as u16;
                    }

                    // Skip the run of same byte
                    cursor = pos + 1;
                    while cursor < n && unsafe { *ptr.add(cursor) } == b {
                        cursor += 1;
                    }
                }
                None => {
                    // No more squeeze chars — copy remainder
                    let rem = n - cursor;
                    if rem > 0 {
                        if wp != cursor {
                            unsafe {
                                std::ptr::copy(ptr.add(cursor), ptr.add(wp), rem);
                            }
                        }
                        wp += rem;
                        last_squeezed = 256;
                    }
                    break;
                }
            }
        }

        writer.write_all(&buf[..wp])?;
    }
    Ok(())
}

fn squeeze_single_stream(
    ch: u8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    // Use a two-byte pattern finder (ch,ch) to jump directly to squeeze points.
    // For squeeze-spaces: most of the data has no consecutive spaces, so memmem
    // skips over huge regions at SIMD speed. When a pair is found, we scan the
    // run length and collapse it to one occurrence.
    // Zero-copy writev: build IoSlice entries pointing into the read buffer,
    // skipping duplicate bytes. Avoids memmove entirely.
    let pair = [ch, ch];
    let finder = memchr::memmem::Finder::new(&pair);
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    let mut was_squeeze_char = false;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }

        let mut i = 0;

        // Handle carry-over: if previous chunk ended with squeeze char,
        // skip leading occurrences of that char in this chunk.
        if was_squeeze_char {
            while i < n && unsafe { *buf.as_ptr().add(i) } == ch {
                i += 1;
            }
            if i >= n {
                // Entire chunk is squeeze-char continuation
                // was_squeeze_char remains true
                continue;
            }
        }

        // Zero-copy writev: scan for consecutive pairs and build IoSlice entries
        // pointing directly into the read buffer, skipping duplicate bytes.
        let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(64);

        loop {
            match finder.find(&buf[i..n]) {
                Some(offset) => {
                    // Include everything up to and including the first char of the pair
                    let seg_end = i + offset + 1;
                    if seg_end > i {
                        iov.push(std::io::IoSlice::new(&buf[i..seg_end]));
                    }
                    i = seg_end;
                    // Skip all remaining consecutive ch bytes (the run)
                    while i < n && unsafe { *buf.as_ptr().add(i) } == ch {
                        i += 1;
                    }
                    if i >= n {
                        was_squeeze_char = true;
                        break;
                    }
                    // Flush when approaching MAX_IOV
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                None => {
                    // No more consecutive pairs — emit remainder
                    if i < n {
                        iov.push(std::io::IoSlice::new(&buf[i..n]));
                    }
                    // Check if chunk ends with the squeeze char
                    was_squeeze_char = n > 0 && unsafe { *buf.as_ptr().add(n - 1) } == ch;
                    break;
                }
            }
        }

        if !iov.is_empty() {
            write_ioslices(writer, &iov)?;
        }
    }
    Ok(())
}

// ============================================================================
// Batch in-place functions (owned data from piped stdin)
// ============================================================================

/// Translate bytes in-place on an owned buffer, then write.
/// For piped stdin where we own the data, this avoids the separate output buffer
/// allocation needed by translate_mmap. Uses parallel in-place SIMD for large data.
pub fn translate_owned(
    set1: &[u8],
    set2: &[u8],
    data: &mut [u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);

    // Identity table — pure passthrough
    let is_identity = table.iter().enumerate().all(|(i, &v)| v == i as u8);
    if is_identity {
        return writer.write_all(data);
    }

    // SIMD range fast path (in-place)
    if let Some((lo, hi, offset)) = detect_range_offset(&table) {
        if data.len() >= PARALLEL_THRESHOLD {
            let n_threads = rayon::current_num_threads().max(1);
            let chunk_size = (data.len() / n_threads).max(32 * 1024);
            data.par_chunks_mut(chunk_size).for_each(|chunk| {
                translate_range_simd_inplace(chunk, lo, hi, offset);
            });
        } else {
            translate_range_simd_inplace(data, lo, hi, offset);
        }
        return writer.write_all(data);
    }

    // SIMD range-to-constant fast path (in-place)
    if let Some((lo, hi, replacement)) = detect_range_to_constant(&table) {
        if data.len() >= PARALLEL_THRESHOLD {
            let n_threads = rayon::current_num_threads().max(1);
            let chunk_size = (data.len() / n_threads).max(32 * 1024);
            data.par_chunks_mut(chunk_size).for_each(|chunk| {
                translate_range_to_constant_simd_inplace(chunk, lo, hi, replacement);
            });
        } else {
            translate_range_to_constant_simd_inplace(data, lo, hi, replacement);
        }
        return writer.write_all(data);
    }

    // General table lookup (in-place)
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);
        data.par_chunks_mut(chunk_size).for_each(|chunk| {
            translate_inplace(chunk, &table);
        });
    } else {
        translate_inplace(data, &table);
    }
    writer.write_all(data)
}

// ============================================================================
// Mmap-based functions (zero-copy input from byte slice)
// ============================================================================

/// Maximum data size for single-allocation translate approach.
/// Below this limit, translate ALL data into one buffer and do a single write_all.
/// Above this, use chunked approach to limit memory usage.
const SINGLE_WRITE_LIMIT: usize = 16 * 1024 * 1024;

/// Translate bytes from an mmap'd byte slice.
/// Detects single-range translations (e.g., a-z to A-Z) and uses SIMD vectorized
/// arithmetic (AVX2: 32 bytes/iter, SSE2: 16 bytes/iter) for those cases.
/// Falls back to scalar 256-byte table lookup for general translations.
///
/// For data >= 2MB: uses rayon parallel processing across multiple cores.
/// For data <= 16MB: single allocation + single write_all (1 syscall).
/// For data > 16MB: chunked approach to limit memory (N syscalls where N = data/4MB).
pub fn translate_mmap(
    set1: &[u8],
    set2: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);

    // Check if table is identity — pure passthrough
    let is_identity = table.iter().enumerate().all(|(i, &v)| v == i as u8);
    if is_identity {
        return writer.write_all(data);
    }

    // Try SIMD fast path for single-range constant-offset translations
    if let Some((lo, hi, offset)) = detect_range_offset(&table) {
        return translate_mmap_range(data, writer, lo, hi, offset);
    }

    // Try SIMD fast path for range-to-constant translations
    if let Some((lo, hi, replacement)) = detect_range_to_constant(&table) {
        return translate_mmap_range_to_constant(data, writer, lo, hi, replacement);
    }

    // General case: table lookup (with parallel processing for large data)
    translate_mmap_table(data, writer, &table)
}

/// SIMD range translate for mmap data, with rayon parallel processing.
fn translate_mmap_range(
    data: &[u8],
    writer: &mut impl Write,
    lo: u8,
    hi: u8,
    offset: i8,
) -> io::Result<()> {
    // Parallel path: split data into chunks, translate each in parallel
    if data.len() >= PARALLEL_THRESHOLD {
        let mut buf = alloc_uninit_vec(data.len());
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        // Process chunks in parallel: each thread writes to its slice of buf
        data.par_chunks(chunk_size)
            .zip(buf.par_chunks_mut(chunk_size))
            .for_each(|(src_chunk, dst_chunk)| {
                translate_range_simd(src_chunk, &mut dst_chunk[..src_chunk.len()], lo, hi, offset);
            });

        return writer.write_all(&buf);
    }

    // Small data: single-threaded SIMD
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut buf = alloc_uninit_vec(data.len());
        translate_range_simd(data, &mut buf, lo, hi, offset);
        return writer.write_all(&buf);
    }
    // Chunked path for large data (shouldn't happen since PARALLEL_THRESHOLD < SINGLE_WRITE_LIMIT)
    let mut buf = alloc_uninit_vec(BUF_SIZE);
    for chunk in data.chunks(BUF_SIZE) {
        translate_range_simd(chunk, &mut buf[..chunk.len()], lo, hi, offset);
        writer.write_all(&buf[..chunk.len()])?;
    }
    Ok(())
}

/// SIMD range-to-constant translate for mmap data.
/// Uses blendv (5 SIMD ops/32 bytes) for range-to-constant patterns.
fn translate_mmap_range_to_constant(
    data: &[u8],
    writer: &mut impl Write,
    lo: u8,
    hi: u8,
    replacement: u8,
) -> io::Result<()> {
    // For mmap data (read-only), copy to buffer and translate in-place
    if data.len() >= PARALLEL_THRESHOLD {
        let mut buf = alloc_uninit_vec(data.len());
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        // Copy + translate in parallel
        data.par_chunks(chunk_size)
            .zip(buf.par_chunks_mut(chunk_size))
            .for_each(|(src_chunk, dst_chunk)| {
                dst_chunk[..src_chunk.len()].copy_from_slice(src_chunk);
                translate_range_to_constant_simd_inplace(
                    &mut dst_chunk[..src_chunk.len()],
                    lo,
                    hi,
                    replacement,
                );
            });

        return writer.write_all(&buf);
    }

    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut buf = data.to_vec();
        translate_range_to_constant_simd_inplace(&mut buf, lo, hi, replacement);
        return writer.write_all(&buf);
    }
    let mut buf = alloc_uninit_vec(BUF_SIZE);
    for chunk in data.chunks(BUF_SIZE) {
        buf[..chunk.len()].copy_from_slice(chunk);
        translate_range_to_constant_simd_inplace(&mut buf[..chunk.len()], lo, hi, replacement);
        writer.write_all(&buf[..chunk.len()])?;
    }
    Ok(())
}

/// General table-lookup translate for mmap data, with rayon parallel processing.
fn translate_mmap_table(data: &[u8], writer: &mut impl Write, table: &[u8; 256]) -> io::Result<()> {
    // Parallel path: split data into chunks, translate each in parallel
    if data.len() >= PARALLEL_THRESHOLD {
        let mut buf = alloc_uninit_vec(data.len());
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        data.par_chunks(chunk_size)
            .zip(buf.par_chunks_mut(chunk_size))
            .for_each(|(src_chunk, dst_chunk)| {
                translate_to(src_chunk, &mut dst_chunk[..src_chunk.len()], table);
            });

        return writer.write_all(&buf);
    }

    // Small data: single-threaded
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut buf = alloc_uninit_vec(data.len());
        translate_to(data, &mut buf, table);
        return writer.write_all(&buf);
    }
    let mut buf = alloc_uninit_vec(BUF_SIZE);
    for chunk in data.chunks(BUF_SIZE) {
        translate_to(chunk, &mut buf[..chunk.len()], table);
        writer.write_all(&buf[..chunk.len()])?;
    }
    Ok(())
}

/// Translate bytes in-place on a mutable buffer (e.g., MAP_PRIVATE mmap).
/// Eliminates the output buffer allocation entirely — the kernel's COW
/// semantics mean only modified pages are physically copied.
///
/// For data >= PARALLEL_THRESHOLD: rayon parallel in-place translate.
/// Otherwise: single-threaded in-place translate.
pub fn translate_mmap_inplace(
    set1: &[u8],
    set2: &[u8],
    data: &mut [u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);

    // Check if table is identity — pure passthrough
    let is_identity = table.iter().enumerate().all(|(i, &v)| v == i as u8);
    if is_identity {
        return writer.write_all(data);
    }

    // For data that's being translated in a MAP_PRIVATE mmap, every modified page
    // triggers a COW fault. For small-to-medium files where most bytes change,
    // reading from mmap (read-only) + writing to a separate heap buffer is faster
    // because it avoids COW faults entirely. The output buffer is fresh memory
    // (no COW), and the input mmap stays read-only (MADV_SEQUENTIAL).
    // Threshold: 32MB. Above this, in-place is better to avoid doubling memory.
    const SEPARATE_BUF_THRESHOLD: usize = 32 * 1024 * 1024;

    if data.len() < SEPARATE_BUF_THRESHOLD {
        return translate_to_separate_buf(data, &table, writer);
    }

    // Try SIMD fast path for single-range constant-offset translations (e.g., a-z -> A-Z)
    if let Some((lo, hi, offset)) = detect_range_offset(&table) {
        if data.len() >= PARALLEL_THRESHOLD {
            let n_threads = rayon::current_num_threads().max(1);
            let chunk_size = (data.len() / n_threads).max(32 * 1024);
            data.par_chunks_mut(chunk_size)
                .for_each(|chunk| translate_range_simd_inplace(chunk, lo, hi, offset));
        } else {
            translate_range_simd_inplace(data, lo, hi, offset);
        }
        return writer.write_all(data);
    }

    // Try SIMD fast path for range-to-constant translations
    if let Some((lo, hi, replacement)) = detect_range_to_constant(&table) {
        if data.len() >= PARALLEL_THRESHOLD {
            let n_threads = rayon::current_num_threads().max(1);
            let chunk_size = (data.len() / n_threads).max(32 * 1024);
            data.par_chunks_mut(chunk_size).for_each(|chunk| {
                translate_range_to_constant_simd_inplace(chunk, lo, hi, replacement)
            });
        } else {
            translate_range_to_constant_simd_inplace(data, lo, hi, replacement);
        }
        return writer.write_all(data);
    }

    // General case: in-place table lookup
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);
        data.par_chunks_mut(chunk_size)
            .for_each(|chunk| translate_inplace(chunk, &table));
    } else {
        translate_inplace(data, &table);
    }
    writer.write_all(data)
}

/// Translate from read-only source to a separate output buffer, avoiding COW faults.
/// Uses the appropriate SIMD path (range offset, range-to-constant, or general nibble).
///
/// For data >= PARALLEL_THRESHOLD: parallel chunked translate into full-size buffer.
/// For smaller data: chunked translate+write with a reusable 4MB buffer to keep
/// output data in L3 cache during write_all (avoids main memory round-trip).
fn translate_to_separate_buf(
    data: &[u8],
    table: &[u8; 256],
    writer: &mut impl Write,
) -> io::Result<()> {
    let range_info = detect_range_offset(table);
    let const_info = if range_info.is_none() {
        detect_range_to_constant(table)
    } else {
        None
    };

    if data.len() >= PARALLEL_THRESHOLD {
        // Parallel path: full-size output buffer, parallel translate, single write.
        let mut out_buf = alloc_uninit_vec(data.len());
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        if let Some((lo, hi, offset)) = range_info {
            data.par_chunks(chunk_size)
                .zip(out_buf.par_chunks_mut(chunk_size))
                .for_each(|(src, dst)| {
                    translate_range_simd(src, &mut dst[..src.len()], lo, hi, offset);
                });
        } else if let Some((lo, hi, replacement)) = const_info {
            data.par_chunks(chunk_size)
                .zip(out_buf.par_chunks_mut(chunk_size))
                .for_each(|(src, dst)| {
                    dst[..src.len()].copy_from_slice(src);
                    translate_range_to_constant_simd_inplace(
                        &mut dst[..src.len()],
                        lo,
                        hi,
                        replacement,
                    );
                });
        } else {
            data.par_chunks(chunk_size)
                .zip(out_buf.par_chunks_mut(chunk_size))
                .for_each(|(src, dst)| {
                    translate_to(src, &mut dst[..src.len()], table);
                });
        }
        return writer.write_all(&out_buf);
    }

    // Sequential path: translate in L3-cache-sized chunks (4MB).
    // This keeps the output buffer hot in L3 cache during write_all,
    // avoiding a main memory round-trip that happens with a full-size buffer.
    const CHUNK: usize = 4 * 1024 * 1024;
    let buf_size = data.len().min(CHUNK);
    let mut out_buf = alloc_uninit_vec(buf_size);

    for src_chunk in data.chunks(CHUNK) {
        let dst = &mut out_buf[..src_chunk.len()];
        if let Some((lo, hi, offset)) = range_info {
            translate_range_simd(src_chunk, dst, lo, hi, offset);
        } else if let Some((lo, hi, replacement)) = const_info {
            dst.copy_from_slice(src_chunk);
            translate_range_to_constant_simd_inplace(dst, lo, hi, replacement);
        } else {
            translate_to(src_chunk, dst, table);
        }
        writer.write_all(dst)?;
    }

    Ok(())
}

/// Translate from a read-only mmap (or any byte slice) to a separate output buffer.
/// Avoids MAP_PRIVATE COW page faults by reading from the original data and
/// writing to a freshly allocated heap buffer.
pub fn translate_mmap_readonly(
    set1: &[u8],
    set2: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);

    // Check if table is identity — pure passthrough
    let is_identity = table.iter().enumerate().all(|(i, &v)| v == i as u8);
    if is_identity {
        return writer.write_all(data);
    }

    translate_to_separate_buf(data, &table, writer)
}

/// Translate + squeeze from mmap'd byte slice.
///
/// For data >= 2MB: two-phase approach: parallel translate, then sequential squeeze.
/// For data <= 16MB: single-pass translate+squeeze into one buffer, one write syscall.
/// For data > 16MB: chunked approach to limit memory.
pub fn translate_squeeze_mmap(
    set1: &[u8],
    set2: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);
    let squeeze_set = build_member_set(set2);

    // For large data: two-phase approach
    // Phase 1: parallel translate into buffer
    // Phase 2: sequential squeeze IN-PLACE on the translated buffer
    //          (squeeze only removes bytes, never grows, so no second allocation needed)
    if data.len() >= PARALLEL_THRESHOLD {
        // Phase 1: parallel translate
        let mut translated = alloc_uninit_vec(data.len());
        let range_info = detect_range_offset(&table);
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        if let Some((lo, hi, offset)) = range_info {
            data.par_chunks(chunk_size)
                .zip(translated.par_chunks_mut(chunk_size))
                .for_each(|(src_chunk, dst_chunk)| {
                    translate_range_simd(
                        src_chunk,
                        &mut dst_chunk[..src_chunk.len()],
                        lo,
                        hi,
                        offset,
                    );
                });
        } else {
            data.par_chunks(chunk_size)
                .zip(translated.par_chunks_mut(chunk_size))
                .for_each(|(src_chunk, dst_chunk)| {
                    translate_to(src_chunk, &mut dst_chunk[..src_chunk.len()], &table);
                });
        }

        // Phase 2: squeeze in-place on the translated buffer.
        // Since squeeze only removes bytes (never grows), we can read ahead and
        // compact into the same buffer, saving a full data.len() heap allocation.
        let mut last_squeezed: u16 = 256;
        let len = translated.len();
        let mut wp = 0;
        unsafe {
            let ptr = translated.as_mut_ptr();
            let mut i = 0;
            while i < len {
                let b = *ptr.add(i);
                if is_member(&squeeze_set, b) {
                    if last_squeezed == b as u16 {
                        i += 1;
                        continue;
                    }
                    last_squeezed = b as u16;
                } else {
                    last_squeezed = 256;
                }
                *ptr.add(wp) = b;
                wp += 1;
                i += 1;
            }
        }
        return writer.write_all(&translated[..wp]);
    }

    // Single-write fast path: translate+squeeze all data in one pass, one write
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut buf: Vec<u8> = Vec::with_capacity(data.len());
        let mut last_squeezed: u16 = 256;
        unsafe {
            buf.set_len(data.len());
            let outp: *mut u8 = buf.as_mut_ptr();
            let inp = data.as_ptr();
            let len = data.len();
            let mut wp = 0;
            let mut i = 0;
            while i < len {
                let translated = *table.get_unchecked(*inp.add(i) as usize);
                if is_member(&squeeze_set, translated) {
                    if last_squeezed == translated as u16 {
                        i += 1;
                        continue;
                    }
                    last_squeezed = translated as u16;
                } else {
                    last_squeezed = 256;
                }
                *outp.add(wp) = translated;
                wp += 1;
                i += 1;
            }
            buf.set_len(wp);
        }
        return writer.write_all(&buf);
    }

    // Chunked path for large data
    let buf_size = data.len().min(BUF_SIZE);
    let mut buf = vec![0u8; buf_size];
    let mut last_squeezed: u16 = 256;

    for chunk in data.chunks(buf_size) {
        translate_to(chunk, &mut buf[..chunk.len()], &table);
        let mut wp = 0;
        unsafe {
            let ptr = buf.as_mut_ptr();
            for i in 0..chunk.len() {
                let b = *ptr.add(i);
                if is_member(&squeeze_set, b) {
                    if last_squeezed == b as u16 {
                        continue;
                    }
                    last_squeezed = b as u16;
                } else {
                    last_squeezed = 256;
                }
                *ptr.add(wp) = b;
                wp += 1;
            }
        }
        writer.write_all(&buf[..wp])?;
    }
    Ok(())
}

/// Delete from mmap'd byte slice.
///
/// For data >= 2MB: uses rayon parallel processing across multiple cores.
/// For data <= 16MB: delete into one buffer, one write syscall.
/// For data > 16MB: chunked approach to limit memory.
pub fn delete_mmap(delete_chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    if delete_chars.len() == 1 {
        return delete_single_char_mmap(delete_chars[0], data, writer);
    }
    if delete_chars.len() <= 3 {
        return delete_multi_memchr_mmap(delete_chars, data, writer);
    }

    // SIMD fast path for contiguous ranges (digits, a-z, A-Z, etc.)
    if let Some((lo, hi)) = detect_delete_range(delete_chars) {
        return delete_range_mmap(data, writer, lo, hi);
    }

    let member = build_member_set(delete_chars);

    // Heuristic: sample first 1024 bytes to estimate delete frequency.
    // If < 10% of bytes are deleted, zero-copy writev is efficient
    // (long runs between deletes = few IoSlice entries).
    // For dense deletes (>= 10%), copy-based approach with parallel processing
    // produces fewer, larger writes.
    let sample_size = data.len().min(1024);
    let sample_deletes = data[..sample_size]
        .iter()
        .filter(|&&b| is_member(&member, b))
        .count();
    let delete_pct = sample_deletes * 100 / sample_size.max(1);

    if delete_pct < 10 {
        return delete_bitset_zerocopy(data, &member, writer);
    }

    // Dense delete: copy-based approach with parallel processing
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let mut outbuf = alloc_uninit_vec(data.len());
        let chunk_lens: Vec<usize> = data
            .par_chunks(chunk_size)
            .zip(outbuf.par_chunks_mut(chunk_size))
            .map(|(src_chunk, dst_chunk)| delete_chunk_bitset_into(src_chunk, &member, dst_chunk))
            .collect();

        let mut write_pos = 0;
        let mut src_offset = 0;
        for &clen in &chunk_lens {
            if clen > 0 && src_offset != write_pos {
                unsafe {
                    std::ptr::copy(
                        outbuf.as_ptr().add(src_offset),
                        outbuf.as_mut_ptr().add(write_pos),
                        clen,
                    );
                }
            }
            write_pos += clen;
            src_offset += chunk_size;
        }

        return writer.write_all(&outbuf[..write_pos]);
    }

    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut outbuf = alloc_uninit_vec(data.len());
        let out_pos = delete_chunk_bitset_into(data, &member, &mut outbuf);
        return writer.write_all(&outbuf[..out_pos]);
    }

    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = alloc_uninit_vec(buf_size);
    for chunk in data.chunks(buf_size) {
        let out_pos = delete_chunk_bitset_into(chunk, &member, &mut outbuf);
        writer.write_all(&outbuf[..out_pos])?;
    }
    Ok(())
}

/// SIMD range delete for mmap data, with rayon parallel processing.
fn delete_range_mmap(data: &[u8], writer: &mut impl Write, lo: u8, hi: u8) -> io::Result<()> {
    // Parallel path: each thread deletes from its chunk into a local Vec
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let results: Vec<Vec<u8>> = data
            .par_chunks(chunk_size)
            .map(|chunk| {
                let mut out = alloc_uninit_vec(chunk.len());
                let wp = delete_range_chunk(chunk, &mut out, lo, hi);
                unsafe { out.set_len(wp) };
                out
            })
            .collect();

        let slices: Vec<std::io::IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| std::io::IoSlice::new(r))
            .collect();
        return write_ioslices(writer, &slices);
    }

    // Single-write fast path
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut outbuf = alloc_uninit_vec(data.len());
        let wp = delete_range_chunk(data, &mut outbuf, lo, hi);
        return writer.write_all(&outbuf[..wp]);
    }

    // Chunked path
    let mut outbuf = alloc_uninit_vec(BUF_SIZE);
    for chunk in data.chunks(BUF_SIZE) {
        let wp = delete_range_chunk(chunk, &mut outbuf, lo, hi);
        writer.write_all(&outbuf[..wp])?;
    }
    Ok(())
}

/// Delete bytes from chunk using bitset, writing into pre-allocated buffer.
/// Returns number of bytes written.
#[inline]
fn delete_chunk_bitset_into(chunk: &[u8], member: &[u8; 32], outbuf: &mut [u8]) -> usize {
    let len = chunk.len();
    let mut out_pos = 0;
    let mut i = 0;

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

            *outbuf.get_unchecked_mut(out_pos) = b0;
            out_pos += !is_member(member, b0) as usize;
            *outbuf.get_unchecked_mut(out_pos) = b1;
            out_pos += !is_member(member, b1) as usize;
            *outbuf.get_unchecked_mut(out_pos) = b2;
            out_pos += !is_member(member, b2) as usize;
            *outbuf.get_unchecked_mut(out_pos) = b3;
            out_pos += !is_member(member, b3) as usize;
            *outbuf.get_unchecked_mut(out_pos) = b4;
            out_pos += !is_member(member, b4) as usize;
            *outbuf.get_unchecked_mut(out_pos) = b5;
            out_pos += !is_member(member, b5) as usize;
            *outbuf.get_unchecked_mut(out_pos) = b6;
            out_pos += !is_member(member, b6) as usize;
            *outbuf.get_unchecked_mut(out_pos) = b7;
            out_pos += !is_member(member, b7) as usize;
        }
        i += 8;
    }

    while i < len {
        unsafe {
            let b = *chunk.get_unchecked(i);
            *outbuf.get_unchecked_mut(out_pos) = b;
            out_pos += !is_member(member, b) as usize;
        }
        i += 1;
    }

    out_pos
}

/// Zero-copy delete for general bitset: scan for runs of kept bytes,
/// build IoSlice entries pointing directly into the source data.
/// No allocation for output data — just ~16 bytes per IoSlice entry.
/// Flushes in MAX_IOV-sized batches for efficient writev.
fn delete_bitset_zerocopy(
    data: &[u8],
    member: &[u8; 32],
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(MAX_IOV);
    let len = data.len();
    let mut i = 0;
    let mut run_start: Option<usize> = None;

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };
        if is_member(member, b) {
            // This byte should be deleted
            if let Some(rs) = run_start {
                iov.push(std::io::IoSlice::new(&data[rs..i]));
                run_start = None;
                if iov.len() >= MAX_IOV {
                    write_ioslices(writer, &iov)?;
                    iov.clear();
                }
            }
        } else {
            // This byte should be kept
            if run_start.is_none() {
                run_start = Some(i);
            }
        }
        i += 1;
    }
    // Flush final run
    if let Some(rs) = run_start {
        iov.push(std::io::IoSlice::new(&data[rs..]));
    }
    if !iov.is_empty() {
        write_ioslices(writer, &iov)?;
    }
    Ok(())
}

fn delete_single_char_mmap(ch: u8, data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    // Zero-copy delete using writev: build IoSlice entries pointing to the
    // gaps between deleted characters in the ORIGINAL data buffer.
    // For `tr -d '\n'` on 10MB with ~200K newlines:
    //   - Old: 10MB allocation + 10MB copy into output buffer
    //   - New: ~200K * 16 = 3.2MB IoSlice entries, zero data copy
    // Uses SIMD memchr_iter to find all positions, then builds IoSlice spans.
    let mut iov: Vec<std::io::IoSlice> = Vec::new();
    let mut last = 0;
    for pos in memchr::memchr_iter(ch, data) {
        if pos > last {
            iov.push(std::io::IoSlice::new(&data[last..pos]));
        }
        last = pos + 1;
    }
    if last < data.len() {
        iov.push(std::io::IoSlice::new(&data[last..]));
    }
    write_ioslices(writer, &iov)
}

fn delete_multi_memchr_mmap(chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    let c0 = chars[0];
    let c1 = if chars.len() >= 2 { chars[1] } else { 0 };
    let c2 = if chars.len() >= 3 { chars[2] } else { 0 };
    let is_three = chars.len() >= 3;

    // Zero-copy delete using writev: build IoSlice entries pointing to the
    // gaps between deleted characters in the original data buffer.
    let mut iov: Vec<std::io::IoSlice> = Vec::new();
    let mut last = 0;
    if is_three {
        for pos in memchr::memchr3_iter(c0, c1, c2, data) {
            if pos > last {
                iov.push(std::io::IoSlice::new(&data[last..pos]));
            }
            last = pos + 1;
        }
    } else {
        for pos in memchr::memchr2_iter(c0, c1, data) {
            if pos > last {
                iov.push(std::io::IoSlice::new(&data[last..pos]));
            }
            last = pos + 1;
        }
    }
    if last < data.len() {
        iov.push(std::io::IoSlice::new(&data[last..]));
    }
    write_ioslices(writer, &iov)
}

/// Delete + squeeze from mmap'd byte slice.
///
/// For data <= 16MB: delete+squeeze into one buffer, one write syscall.
/// For data > 16MB: chunked approach to limit memory.
pub fn delete_squeeze_mmap(
    delete_chars: &[u8],
    squeeze_chars: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let delete_set = build_member_set(delete_chars);
    let squeeze_set = build_member_set(squeeze_chars);

    // Single-write fast path: delete+squeeze all data in one pass, one write
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut outbuf: Vec<u8> = Vec::with_capacity(data.len());
        let mut last_squeezed: u16 = 256;
        unsafe {
            outbuf.set_len(data.len());
            let outp: *mut u8 = outbuf.as_mut_ptr();
            let inp = data.as_ptr();
            let len = data.len();
            let mut out_pos = 0;
            let mut i = 0;
            while i < len {
                let b = *inp.add(i);
                if is_member(&delete_set, b) {
                    i += 1;
                    continue;
                }
                if is_member(&squeeze_set, b) {
                    if last_squeezed == b as u16 {
                        i += 1;
                        continue;
                    }
                    last_squeezed = b as u16;
                } else {
                    last_squeezed = 256;
                }
                *outp.add(out_pos) = b;
                out_pos += 1;
                i += 1;
            }
            outbuf.set_len(out_pos);
        }
        return writer.write_all(&outbuf);
    }

    // Chunked path for large data
    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = vec![0u8; buf_size];
    let mut last_squeezed: u16 = 256;

    for chunk in data.chunks(buf_size) {
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
///
/// For data >= 2MB: uses rayon parallel processing with boundary fixup.
/// For data <= 16MB: squeeze into one buffer, one write syscall.
/// For data > 16MB: chunked approach to limit memory.
pub fn squeeze_mmap(squeeze_chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    if squeeze_chars.len() == 1 {
        return squeeze_single_mmap(squeeze_chars[0], data, writer);
    }
    if squeeze_chars.len() == 2 {
        return squeeze_multi_mmap::<2>(squeeze_chars, data, writer);
    }
    if squeeze_chars.len() == 3 {
        return squeeze_multi_mmap::<3>(squeeze_chars, data, writer);
    }

    let member = build_member_set(squeeze_chars);

    // Parallel path: squeeze each chunk independently, then fix boundaries
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let results: Vec<Vec<u8>> = data
            .par_chunks(chunk_size)
            .map(|chunk| squeeze_chunk_bitset(chunk, &member))
            .collect();

        // Build IoSlice list, fixing boundaries: if chunk N ends with byte B
        // and chunk N+1 starts with same byte B, and B is in squeeze set,
        // skip the first byte(s) of chunk N+1 that equal B.
        // Collect slices for writev to minimize syscalls.
        let mut slices: Vec<std::io::IoSlice> = Vec::with_capacity(results.len());
        for (idx, result) in results.iter().enumerate() {
            if result.is_empty() {
                continue;
            }
            if idx > 0 {
                // Check boundary: does previous chunk end with same squeezable byte?
                if let Some(&prev_last) = results[..idx].iter().rev().find_map(|r| r.last()) {
                    if is_member(&member, prev_last) {
                        // Skip leading bytes in this chunk that equal prev_last
                        let skip = result.iter().take_while(|&&b| b == prev_last).count();
                        if skip < result.len() {
                            slices.push(std::io::IoSlice::new(&result[skip..]));
                        }
                        continue;
                    }
                }
            }
            slices.push(std::io::IoSlice::new(result));
        }
        return write_ioslices(writer, &slices);
    }

    // Single-write fast path: squeeze all data into one buffer, one write
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut outbuf: Vec<u8> = Vec::with_capacity(data.len());
        let mut last_squeezed: u16 = 256;
        let len = data.len();
        let mut wp = 0;
        let mut i = 0;

        unsafe {
            outbuf.set_len(data.len());
            let inp = data.as_ptr();
            let outp: *mut u8 = outbuf.as_mut_ptr();

            while i < len {
                let b = *inp.add(i);
                if is_member(&member, b) {
                    if last_squeezed != b as u16 {
                        *outp.add(wp) = b;
                        wp += 1;
                        last_squeezed = b as u16;
                    }
                    i += 1;
                    while i < len && *inp.add(i) == b {
                        i += 1;
                    }
                } else {
                    last_squeezed = 256;
                    *outp.add(wp) = b;
                    wp += 1;
                    i += 1;
                }
            }
            outbuf.set_len(wp);
        }
        return writer.write_all(&outbuf);
    }

    // Chunked path for large data
    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = vec![0u8; buf_size];
    let mut last_squeezed: u16 = 256;

    for chunk in data.chunks(buf_size) {
        let len = chunk.len();
        let mut wp = 0;
        let mut i = 0;

        unsafe {
            let inp = chunk.as_ptr();
            let outp = outbuf.as_mut_ptr();

            while i < len {
                let b = *inp.add(i);
                if is_member(&member, b) {
                    if last_squeezed != b as u16 {
                        *outp.add(wp) = b;
                        wp += 1;
                        last_squeezed = b as u16;
                    }
                    i += 1;
                    while i < len && *inp.add(i) == b {
                        i += 1;
                    }
                } else {
                    last_squeezed = 256;
                    *outp.add(wp) = b;
                    wp += 1;
                    i += 1;
                }
            }
        }
        writer.write_all(&outbuf[..wp])?;
    }
    Ok(())
}

/// Squeeze a single chunk using bitset membership. Returns squeezed output.
fn squeeze_chunk_bitset(chunk: &[u8], member: &[u8; 32]) -> Vec<u8> {
    let len = chunk.len();
    let mut out = Vec::with_capacity(len);
    let mut last_squeezed: u16 = 256;
    let mut i = 0;

    unsafe {
        out.set_len(len);
        let inp = chunk.as_ptr();
        let outp: *mut u8 = out.as_mut_ptr();
        let mut wp = 0;

        while i < len {
            let b = *inp.add(i);
            if is_member(member, b) {
                if last_squeezed != b as u16 {
                    *outp.add(wp) = b;
                    wp += 1;
                    last_squeezed = b as u16;
                }
                i += 1;
                while i < len && *inp.add(i) == b {
                    i += 1;
                }
            } else {
                last_squeezed = 256;
                *outp.add(wp) = b;
                wp += 1;
                i += 1;
            }
        }
        out.set_len(wp);
    }
    out
}

fn squeeze_multi_mmap<const N: usize>(
    chars: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    // Parallel path for large data: squeeze each chunk, fix boundaries with writev
    if data.len() >= PARALLEL_THRESHOLD {
        let member = build_member_set(chars);
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let results: Vec<Vec<u8>> = data
            .par_chunks(chunk_size)
            .map(|chunk| squeeze_chunk_bitset(chunk, &member))
            .collect();

        // Build IoSlice list, fixing boundaries
        let mut slices: Vec<std::io::IoSlice> = Vec::with_capacity(results.len());
        for (idx, result) in results.iter().enumerate() {
            if result.is_empty() {
                continue;
            }
            if idx > 0 {
                if let Some(&prev_last) = results[..idx].iter().rev().find_map(|r| r.last()) {
                    if is_member(&member, prev_last) {
                        let skip = result.iter().take_while(|&&b| b == prev_last).count();
                        if skip < result.len() {
                            slices.push(std::io::IoSlice::new(&result[skip..]));
                        }
                        continue;
                    }
                }
            }
            slices.push(std::io::IoSlice::new(result));
        }
        return write_ioslices(writer, &slices);
    }

    // Zero-copy writev: build IoSlice entries pointing directly into
    // the original mmap'd data, keeping one byte per run of squeezable chars.
    // Each IoSlice points at the gap between squeeze points (inclusive of
    // the first byte of a run) — no data is copied.
    let single = [chars[0]; 1]; // scratch for emitting single squeeze byte
    let _ = single;
    let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(1024);
    let mut cursor = 0;
    let mut last_squeezed: u16 = 256;

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
                // Emit gap before squeeze point
                if pos > cursor {
                    iov.push(std::io::IoSlice::new(&data[cursor..pos]));
                    last_squeezed = 256;
                }
                // Emit single byte if not duplicate
                if last_squeezed != b as u16 {
                    // Point at the byte in the original data (zero-copy)
                    iov.push(std::io::IoSlice::new(&data[pos..pos + 1]));
                    last_squeezed = b as u16;
                }
                // Skip the run of same byte
                let mut skip = pos + 1;
                while skip < data.len() && data[skip] == b {
                    skip += 1;
                }
                cursor = skip;
                // Flush when approaching MAX_IOV
                if iov.len() >= MAX_IOV {
                    write_ioslices(writer, &iov)?;
                    iov.clear();
                }
            }
            None => {
                if cursor < data.len() {
                    iov.push(std::io::IoSlice::new(&data[cursor..]));
                }
                break;
            }
        }
    }
    if !iov.is_empty() {
        write_ioslices(writer, &iov)?;
    }
    Ok(())
}

fn squeeze_single_mmap(ch: u8, data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Quick check: no consecutive pairs means no squeezing needed
    let pair = [ch, ch];
    if memchr::memmem::find(data, &pair).is_none() {
        return writer.write_all(data);
    }

    // Zero-copy writev approach: build IoSlice entries pointing directly into
    // the original mmap'd data, skipping duplicate bytes in runs.
    // For `tr -s ' '` on 10MB with ~5K squeeze points:
    //   - ~10K IoSlice entries (one per gap + one per squeeze point)
    //   - ~10 writev syscalls (at 1024 entries per batch)
    //   - Zero data copy — kernel reads directly from mmap pages
    let finder = memchr::memmem::Finder::new(&pair);
    let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(2048);
    let mut cursor = 0;

    while cursor < data.len() {
        match finder.find(&data[cursor..]) {
            Some(offset) => {
                let pair_pos = cursor + offset;
                // Include everything up to and including the first byte of the pair
                let seg_end = pair_pos + 1;
                if seg_end > cursor {
                    iov.push(std::io::IoSlice::new(&data[cursor..seg_end]));
                }
                // Skip all remaining consecutive ch bytes (the run)
                let mut skip = seg_end;
                while skip < data.len() && data[skip] == ch {
                    skip += 1;
                }
                cursor = skip;
                // Flush when approaching MAX_IOV
                if iov.len() >= MAX_IOV {
                    write_ioslices(writer, &iov)?;
                    iov.clear();
                }
            }
            None => {
                // No more pairs — emit remainder
                if cursor < data.len() {
                    iov.push(std::io::IoSlice::new(&data[cursor..]));
                }
                break;
            }
        }
    }

    if !iov.is_empty() {
        write_ioslices(writer, &iov)?;
    }
    Ok(())
}
