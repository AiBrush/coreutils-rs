use std::io::{self, Read, Write};

use rayon::prelude::*;

/// Maximum IoSlice entries per write_vectored batch.
/// Linux UIO_MAXIOV is 1024; we use that as our batch limit.
const MAX_IOV: usize = 1024;

/// Stream buffer: 4MB — sized to fit entirely in L3 cache (~8-16MB on modern
/// CPUs). Data passes through the buffer 3 times (read→translate→write), so
/// keeping it cache-warm gives ~3-5x throughput vs spilling to DRAM.
/// For 10MB file: 3 rounds × 4MB buffer at L3 speed (~100 GB/s) ≈ 300µs,
/// vs 1 round × 16MB buffer at DRAM speed (~20 GB/s) ≈ 1000µs.
/// For piped input with 8MB pipe buffer: 2 reads per chunk, ~5µs extra syscalls.
const STREAM_BUF: usize = 4 * 1024 * 1024;

/// Minimum data size to engage rayon parallel processing for mmap paths.
/// AVX2 translation runs at ~10 GB/s per core. For 10MB benchmarks,
/// rayon overhead (~100-200us for spawn+join) dominates the ~1ms
/// single-core translate time. Only use parallel for genuinely large files
/// where the parallel speedup outweighs rayon overhead.
const PARALLEL_THRESHOLD: usize = 32 * 1024 * 1024;

/// 256-entry lookup table for byte compaction: for each 8-bit keep mask,
/// stores the bit positions of set bits (indices of bytes to keep).
/// Used by compact_8bytes to replace the serial trailing_zeros loop with
/// unconditional indexed stores, eliminating the tzcnt→blsr dependency chain.
/// Total size: 256 * 8 = 2KB — fits entirely in L1 cache.
#[cfg(target_arch = "x86_64")]
static COMPACT_LUT: [[u8; 8]; 256] = {
    let mut lut = [[0u8; 8]; 256];
    let mut mask: u16 = 0;
    while mask < 256 {
        let mut idx: usize = 0;
        let mut bit: u8 = 0;
        while bit < 8 {
            if (mask >> bit) & 1 != 0 {
                lut[mask as usize][idx] = bit;
                idx += 1;
            }
            bit += 1;
        }
        mask += 1;
    }
    lut
};

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
/// Uses MADV_HUGEPAGE on Linux for buffers >= 2MB to reduce TLB misses.
/// SAFETY: Caller must write all bytes before reading them.
#[inline]
#[allow(clippy::uninit_vec)]
fn alloc_uninit_vec(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    // SAFETY: u8 has no drop, no invalid bit patterns; caller will overwrite before reading
    unsafe {
        v.set_len(len);
    }
    #[cfg(target_os = "linux")]
    if len >= 2 * 1024 * 1024 {
        unsafe {
            libc::madvise(
                v.as_mut_ptr() as *mut libc::c_void,
                len,
                libc::MADV_HUGEPAGE,
            );
        }
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
#[cfg(not(target_arch = "aarch64"))]
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

/// ARM64 NEON table lookup using nibble decomposition (same algorithm as x86 pshufb).
/// Uses vqtbl1q_u8 for 16-byte table lookups, processes 16 bytes per iteration.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn translate_inplace_scalar(data: &mut [u8], table: &[u8; 256]) {
    unsafe { translate_inplace_neon_table(data, table) };
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn translate_inplace_neon_table(data: &mut [u8], table: &[u8; 256]) {
    use std::arch::aarch64::*;

    unsafe {
        let len = data.len();
        let ptr = data.as_mut_ptr();

        // Pre-build 16 NEON lookup vectors (one per high nibble)
        let mut lut: [uint8x16_t; 16] = [vdupq_n_u8(0); 16];
        for h in 0u8..16 {
            let base = (h as usize) * 16;
            lut[h as usize] = vld1q_u8(table.as_ptr().add(base));
        }

        let lo_mask = vdupq_n_u8(0x0F);
        let mut i = 0;

        while i + 16 <= len {
            let input = vld1q_u8(ptr.add(i));
            let lo_nibble = vandq_u8(input, lo_mask);
            let hi_nibble = vandq_u8(vshrq_n_u8(input, 4), lo_mask);

            let mut result = vdupq_n_u8(0);
            macro_rules! do_nibble {
                ($h:expr) => {
                    let h_val = vdupq_n_u8($h);
                    let mask = vceqq_u8(hi_nibble, h_val);
                    let looked_up = vqtbl1q_u8(lut[$h as usize], lo_nibble);
                    result = vorrq_u8(result, vandq_u8(mask, looked_up));
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

            vst1q_u8(ptr.add(i), result);
            i += 16;
        }

        // Scalar tail
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
            // Use nontemporal stores when dst is 32-byte aligned (large Vec allocations)
            if dst.as_ptr() as usize & 31 == 0 {
                unsafe { translate_to_avx2_table_nt(src, dst, table) };
            } else {
                unsafe { translate_to_avx2_table(src, dst, table) };
            }
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
#[cfg(not(target_arch = "aarch64"))]
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

/// ARM64 NEON table-lookup translate_to using nibble decomposition.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn translate_to_scalar(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
    unsafe { translate_to_neon_table(src, dst, table) };
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn translate_to_neon_table(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
    use std::arch::aarch64::*;

    unsafe {
        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();

        let mut lut: [uint8x16_t; 16] = [vdupq_n_u8(0); 16];
        for h in 0u8..16 {
            lut[h as usize] = vld1q_u8(table.as_ptr().add((h as usize) * 16));
        }

        let lo_mask = vdupq_n_u8(0x0F);
        let mut i = 0;

        while i + 16 <= len {
            let input = vld1q_u8(sp.add(i));
            let lo_nibble = vandq_u8(input, lo_mask);
            let hi_nibble = vandq_u8(vshrq_n_u8(input, 4), lo_mask);

            let mut result = vdupq_n_u8(0);
            macro_rules! do_nibble {
                ($h:expr) => {
                    let h_val = vdupq_n_u8($h);
                    let mask = vceqq_u8(hi_nibble, h_val);
                    let looked_up = vqtbl1q_u8(lut[$h as usize], lo_nibble);
                    result = vorrq_u8(result, vandq_u8(mask, looked_up));
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

            vst1q_u8(dp.add(i), result);
            i += 16;
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

/// Nontemporal variant of translate_to_avx2_table: uses _mm256_stream_si256 for stores.
/// Avoids RFO cache traffic for the destination buffer in streaming translate operations.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_to_avx2_table_nt(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
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

        // 2x unrolled with nontemporal stores
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

            _mm256_stream_si256(dp.add(i) as *mut _, r0);
            _mm256_stream_si256(dp.add(i + 32) as *mut _, r1);
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

            _mm256_stream_si256(dp.add(i) as *mut _, result);
            i += 32;
        }

        // SSSE3 tail for remaining 16-byte chunk (regular store)
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

        // Fence: ensure nontemporal stores are visible before write() syscall
        _mm_sfence();
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

#[cfg(target_arch = "aarch64")]
fn translate_range_to_constant_simd_inplace(data: &mut [u8], lo: u8, hi: u8, replacement: u8) {
    unsafe { translate_range_to_constant_neon_inplace(data, lo, hi, replacement) };
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn translate_range_to_constant_neon_inplace(
    data: &mut [u8],
    lo: u8,
    hi: u8,
    replacement: u8,
) {
    use std::arch::aarch64::*;

    unsafe {
        let len = data.len();
        let ptr = data.as_mut_ptr();
        let lo_v = vdupq_n_u8(lo);
        let hi_v = vdupq_n_u8(hi);
        let repl_v = vdupq_n_u8(replacement);
        let mut i = 0;

        while i + 32 <= len {
            let in0 = vld1q_u8(ptr.add(i));
            let in1 = vld1q_u8(ptr.add(i + 16));
            let ge0 = vcgeq_u8(in0, lo_v);
            let le0 = vcleq_u8(in0, hi_v);
            let mask0 = vandq_u8(ge0, le0);
            let ge1 = vcgeq_u8(in1, lo_v);
            let le1 = vcleq_u8(in1, hi_v);
            let mask1 = vandq_u8(ge1, le1);
            // bsl: select repl where mask, keep input where not
            vst1q_u8(ptr.add(i), vbslq_u8(mask0, repl_v, in0));
            vst1q_u8(ptr.add(i + 16), vbslq_u8(mask1, repl_v, in1));
            i += 32;
        }

        if i + 16 <= len {
            let input = vld1q_u8(ptr.add(i));
            let ge = vcgeq_u8(input, lo_v);
            let le = vcleq_u8(input, hi_v);
            let mask = vandq_u8(ge, le);
            vst1q_u8(ptr.add(i), vbslq_u8(mask, repl_v, input));
            i += 16;
        }

        while i < len {
            let b = *ptr.add(i);
            *ptr.add(i) = if b >= lo && b <= hi { replacement } else { b };
            i += 1;
        }
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn translate_range_to_constant_simd_inplace(data: &mut [u8], lo: u8, hi: u8, replacement: u8) {
    for b in data.iter_mut() {
        if *b >= lo && *b <= hi {
            *b = replacement;
        }
    }
}

/// SIMD range-to-constant translation from src to dst (no intermediate copy needed).
/// Reads from src, writes translated result to dst in a single pass.
#[cfg(target_arch = "x86_64")]
fn translate_range_to_constant_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, replacement: u8) {
    if get_simd_level() >= 3 {
        unsafe { translate_range_to_constant_avx2(src, dst, lo, hi, replacement) };
    } else {
        unsafe { translate_range_to_constant_sse2(src, dst, lo, hi, replacement) };
    }
}

#[cfg(target_arch = "aarch64")]
fn translate_range_to_constant_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, replacement: u8) {
    unsafe { translate_range_to_constant_neon(src, dst, lo, hi, replacement) };
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn translate_range_to_constant_neon(
    src: &[u8],
    dst: &mut [u8],
    lo: u8,
    hi: u8,
    replacement: u8,
) {
    use std::arch::aarch64::*;

    unsafe {
        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let lo_v = vdupq_n_u8(lo);
        let hi_v = vdupq_n_u8(hi);
        let repl_v = vdupq_n_u8(replacement);
        let mut i = 0;

        while i + 32 <= len {
            let in0 = vld1q_u8(sp.add(i));
            let in1 = vld1q_u8(sp.add(i + 16));
            let mask0 = vandq_u8(vcgeq_u8(in0, lo_v), vcleq_u8(in0, hi_v));
            let mask1 = vandq_u8(vcgeq_u8(in1, lo_v), vcleq_u8(in1, hi_v));
            vst1q_u8(dp.add(i), vbslq_u8(mask0, repl_v, in0));
            vst1q_u8(dp.add(i + 16), vbslq_u8(mask1, repl_v, in1));
            i += 32;
        }

        if i + 16 <= len {
            let input = vld1q_u8(sp.add(i));
            let mask = vandq_u8(vcgeq_u8(input, lo_v), vcleq_u8(input, hi_v));
            vst1q_u8(dp.add(i), vbslq_u8(mask, repl_v, input));
            i += 16;
        }

        while i < len {
            let b = *sp.add(i);
            *dp.add(i) = if b >= lo && b <= hi { replacement } else { b };
            i += 1;
        }
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn translate_range_to_constant_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, replacement: u8) {
    for (i, &b) in src.iter().enumerate() {
        unsafe {
            *dst.get_unchecked_mut(i) = if b >= lo && b <= hi { replacement } else { b };
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_range_to_constant_avx2(
    src: &[u8],
    dst: &mut [u8],
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
        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let mut i = 0;
        while i + 64 <= len {
            let in0 = _mm256_loadu_si256(sp.add(i) as *const _);
            let in1 = _mm256_loadu_si256(sp.add(i + 32) as *const _);
            let bi0 = _mm256_add_epi8(in0, bias_v);
            let bi1 = _mm256_add_epi8(in1, bias_v);
            let gt0 = _mm256_cmpgt_epi8(bi0, threshold_v);
            let gt1 = _mm256_cmpgt_epi8(bi1, threshold_v);
            let ir0 = _mm256_cmpeq_epi8(gt0, zero);
            let ir1 = _mm256_cmpeq_epi8(gt1, zero);
            let r0 = _mm256_blendv_epi8(in0, repl_v, ir0);
            let r1 = _mm256_blendv_epi8(in1, repl_v, ir1);
            _mm256_storeu_si256(dp.add(i) as *mut _, r0);
            _mm256_storeu_si256(dp.add(i + 32) as *mut _, r1);
            i += 64;
        }
        if i + 32 <= len {
            let input = _mm256_loadu_si256(sp.add(i) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            let in_range = _mm256_cmpeq_epi8(gt, zero);
            let result = _mm256_blendv_epi8(input, repl_v, in_range);
            _mm256_storeu_si256(dp.add(i) as *mut _, result);
            i += 32;
        }
        while i < len {
            let b = *sp.add(i);
            *dp.add(i) = if b >= lo && b <= hi { replacement } else { b };
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn translate_range_to_constant_sse2(
    src: &[u8],
    dst: &mut [u8],
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
        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let mut i = 0;
        while i + 16 <= len {
            let input = _mm_loadu_si128(sp.add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v);
            let gt = _mm_cmpgt_epi8(biased, threshold_v);
            let in_range = _mm_cmpeq_epi8(gt, zero);
            let result = _mm_or_si128(
                _mm_and_si128(in_range, repl_v),
                _mm_andnot_si128(in_range, input),
            );
            _mm_storeu_si128(dp.add(i) as *mut _, result);
            i += 16;
        }
        while i < len {
            let b = *sp.add(i);
            *dp.add(i) = if b >= lo && b <= hi { replacement } else { b };
            i += 1;
        }
    }
}

/// SIMD-accelerated range translation for mmap'd data.
/// For tables where only a contiguous range [lo..=hi] is translated by a constant offset,
/// uses AVX2 (32 bytes/iter) or SSE2 (16 bytes/iter) vectorized arithmetic.
/// When dst is 32-byte aligned (true for large Vec allocations from mmap), uses
/// nontemporal stores to bypass cache, avoiding read-for-ownership overhead and
/// reducing memory traffic by ~33% for streaming writes.
#[cfg(target_arch = "x86_64")]
fn translate_range_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    if get_simd_level() >= 3 {
        // Use nontemporal stores when dst is 32-byte aligned (typical for large allocs)
        if dst.as_ptr() as usize & 31 == 0 {
            unsafe { translate_range_avx2_nt(src, dst, lo, hi, offset) };
        } else {
            unsafe { translate_range_avx2(src, dst, lo, hi, offset) };
        }
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

/// Nontemporal variant of translate_range_avx2: uses _mm256_stream_si256 for stores.
/// This bypasses the cache for writes, avoiding read-for-ownership (RFO) traffic on
/// the destination buffer. For streaming translate (src → dst, dst not read again),
/// this reduces memory traffic by ~33% (10MB input: 20MB vs 30MB total traffic).
/// Requires dst to be 32-byte aligned (guaranteed for large Vec/mmap allocations).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn translate_range_avx2_nt(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let offset_v = _mm256_set1_epi8(offset);
        let zero = _mm256_setzero_si256();

        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let mut i = 0;

        // 2x unrolled with nontemporal stores
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
            _mm256_stream_si256(dp.add(i) as *mut _, r0);
            _mm256_stream_si256(dp.add(i + 32) as *mut _, r1);
            i += 64;
        }

        // Remaining 32-byte chunk (still nontemporal if aligned)
        if i + 32 <= len {
            let input = _mm256_loadu_si256(sp.add(i) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            let mask = _mm256_cmpeq_epi8(gt, zero);
            let offset_masked = _mm256_and_si256(mask, offset_v);
            let result = _mm256_add_epi8(input, offset_masked);
            _mm256_stream_si256(dp.add(i) as *mut _, result);
            i += 32;
        }

        // SSE2 tail for 16-byte remainder (regular store — only 16 bytes)
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

        // Fence: ensure nontemporal stores are visible before write() syscall
        _mm_sfence();
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

/// ARM64 NEON-accelerated range translation.
/// Processes 16 bytes per iteration using vectorized range check + conditional add.
#[cfg(target_arch = "aarch64")]
fn translate_range_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    unsafe { translate_range_neon(src, dst, lo, hi, offset) };
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn translate_range_neon(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    use std::arch::aarch64::*;

    unsafe {
        let len = src.len();
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let lo_v = vdupq_n_u8(lo);
        let hi_v = vdupq_n_u8(hi);
        let offset_v = vdupq_n_s8(offset);
        let mut i = 0;

        // 2x unrolled: process 32 bytes per iteration
        while i + 32 <= len {
            let in0 = vld1q_u8(sp.add(i));
            let in1 = vld1q_u8(sp.add(i + 16));
            // Range check: (b >= lo) & (b <= hi)
            let ge0 = vcgeq_u8(in0, lo_v);
            let le0 = vcleq_u8(in0, hi_v);
            let mask0 = vandq_u8(ge0, le0);
            let ge1 = vcgeq_u8(in1, lo_v);
            let le1 = vcleq_u8(in1, hi_v);
            let mask1 = vandq_u8(ge1, le1);
            // Conditional add: in + (offset & mask)
            let off0 = vandq_u8(mask0, vreinterpretq_u8_s8(offset_v));
            let off1 = vandq_u8(mask1, vreinterpretq_u8_s8(offset_v));
            let r0 = vaddq_u8(in0, off0);
            let r1 = vaddq_u8(in1, off1);
            vst1q_u8(dp.add(i), r0);
            vst1q_u8(dp.add(i + 16), r1);
            i += 32;
        }

        if i + 16 <= len {
            let input = vld1q_u8(sp.add(i));
            let ge = vcgeq_u8(input, lo_v);
            let le = vcleq_u8(input, hi_v);
            let mask = vandq_u8(ge, le);
            let off = vandq_u8(mask, vreinterpretq_u8_s8(offset_v));
            vst1q_u8(dp.add(i), vaddq_u8(input, off));
            i += 16;
        }

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

/// Scalar range translation fallback for non-x86_64, non-aarch64 platforms.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn translate_range_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    let offset_u8 = offset as u8;
    let range = hi.wrapping_sub(lo);
    unsafe {
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let len = src.len();
        let mut i = 0;
        while i + 8 <= len {
            macro_rules! do_byte {
                ($off:expr) => {{
                    let b = *sp.add(i + $off);
                    let in_range = b.wrapping_sub(lo) <= range;
                    *dp.add(i + $off) = if in_range {
                        b.wrapping_add(offset_u8)
                    } else {
                        b
                    };
                }};
            }
            do_byte!(0);
            do_byte!(1);
            do_byte!(2);
            do_byte!(3);
            do_byte!(4);
            do_byte!(5);
            do_byte!(6);
            do_byte!(7);
            i += 8;
        }
        while i < len {
            let b = *sp.add(i);
            let in_range = b.wrapping_sub(lo) <= range;
            *dp.add(i) = if in_range {
                b.wrapping_add(offset_u8)
            } else {
                b
            };
            i += 1;
        }
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

#[cfg(target_arch = "aarch64")]
fn translate_range_simd_inplace(data: &mut [u8], lo: u8, hi: u8, offset: i8) {
    unsafe { translate_range_neon_inplace(data, lo, hi, offset) };
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn translate_range_neon_inplace(data: &mut [u8], lo: u8, hi: u8, offset: i8) {
    use std::arch::aarch64::*;

    unsafe {
        let len = data.len();
        let ptr = data.as_mut_ptr();
        let lo_v = vdupq_n_u8(lo);
        let hi_v = vdupq_n_u8(hi);
        let offset_v = vdupq_n_s8(offset);
        let mut i = 0;

        while i + 32 <= len {
            let in0 = vld1q_u8(ptr.add(i));
            let in1 = vld1q_u8(ptr.add(i + 16));
            let ge0 = vcgeq_u8(in0, lo_v);
            let le0 = vcleq_u8(in0, hi_v);
            let mask0 = vandq_u8(ge0, le0);
            let ge1 = vcgeq_u8(in1, lo_v);
            let le1 = vcleq_u8(in1, hi_v);
            let mask1 = vandq_u8(ge1, le1);
            let off0 = vandq_u8(mask0, vreinterpretq_u8_s8(offset_v));
            let off1 = vandq_u8(mask1, vreinterpretq_u8_s8(offset_v));
            vst1q_u8(ptr.add(i), vaddq_u8(in0, off0));
            vst1q_u8(ptr.add(i + 16), vaddq_u8(in1, off1));
            i += 32;
        }

        if i + 16 <= len {
            let input = vld1q_u8(ptr.add(i));
            let ge = vcgeq_u8(input, lo_v);
            let le = vcleq_u8(input, hi_v);
            let mask = vandq_u8(ge, le);
            let off = vandq_u8(mask, vreinterpretq_u8_s8(offset_v));
            vst1q_u8(ptr.add(i), vaddq_u8(input, off));
            i += 16;
        }

        while i < len {
            let b = *ptr.add(i);
            if b >= lo && b <= hi {
                *ptr.add(i) = b.wrapping_add(offset as u8);
            }
            i += 1;
        }
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn translate_range_simd_inplace(data: &mut [u8], lo: u8, hi: u8, offset: i8) {
    let offset_u8 = offset as u8;
    let range = hi.wrapping_sub(lo);
    for b in data.iter_mut() {
        if b.wrapping_sub(lo) <= range {
            *b = b.wrapping_add(offset_u8);
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
    // Cast to usize before +1 to avoid u8 overflow when hi=255, lo=0 (range=256)
    if (hi as usize - lo as usize + 1) == chars.len() {
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
                // Partial keep — per-lane processing with all-keep fast paths.
                // For 4% delete rate, ~72% of 8-byte lanes are all-keep even
                // within partial 32-byte blocks. The per-lane check avoids
                // the LUT compact overhead for these clean lanes.
                let m0 = keep_mask as u8;
                let m1 = (keep_mask >> 8) as u8;
                let m2 = (keep_mask >> 16) as u8;
                let m3 = (keep_mask >> 24) as u8;

                if m0 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 8);
                } else if m0 != 0 {
                    compact_8bytes_simd(sp.add(ri), dp.add(wp), m0);
                }
                let c0 = m0.count_ones() as usize;

                if m1 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri + 8), dp.add(wp + c0), 8);
                } else if m1 != 0 {
                    compact_8bytes_simd(sp.add(ri + 8), dp.add(wp + c0), m1);
                }
                let c1 = m1.count_ones() as usize;

                if m2 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri + 16), dp.add(wp + c0 + c1), 8);
                } else if m2 != 0 {
                    compact_8bytes_simd(sp.add(ri + 16), dp.add(wp + c0 + c1), m2);
                }
                let c2 = m2.count_ones() as usize;

                if m3 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri + 24), dp.add(wp + c0 + c1 + c2), 8);
                } else if m3 != 0 {
                    compact_8bytes_simd(sp.add(ri + 24), dp.add(wp + c0 + c1 + c2), m3);
                }
                let c3 = m3.count_ones() as usize;
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
                let m0 = keep_mask as u8;
                let m1 = (keep_mask >> 8) as u8;
                if m0 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 8);
                } else if m0 != 0 {
                    compact_8bytes_simd(sp.add(ri), dp.add(wp), m0);
                }
                let c0 = m0.count_ones() as usize;
                if m1 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri + 8), dp.add(wp + c0), 8);
                } else if m1 != 0 {
                    compact_8bytes_simd(sp.add(ri + 8), dp.add(wp + c0), m1);
                }
                wp += c0 + m1.count_ones() as usize;
            }
            ri += 16;
        }

        // Scalar tail — branchless: always store, advance wp only for kept bytes
        while ri < len {
            let b = *sp.add(ri);
            *dp.add(wp) = b;
            wp += (b < lo || b > hi) as usize;
            ri += 1;
        }

        wp
    }
}

/// Compact 8 source bytes into contiguous output bytes using a keep mask.
/// Each bit in `mask` indicates whether the corresponding byte should be kept.
/// Uses a precomputed LUT: for each 8-bit mask, the LUT stores indices of set bits.
/// Always performs 8 unconditional stores (extra stores past popcount are harmless
/// since the write pointer only advances by popcount, and subsequent lanes overwrite).
/// This eliminates the serial tzcnt→blsr dependency chain (~28 cycles) in favor of
/// independent indexed loads and stores (~15 cycles).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn compact_8bytes(src: *const u8, dst: *mut u8, mask: u8) {
    unsafe {
        let idx = COMPACT_LUT.get_unchecked(mask as usize);
        *dst = *src.add(*idx.get_unchecked(0) as usize);
        *dst.add(1) = *src.add(*idx.get_unchecked(1) as usize);
        *dst.add(2) = *src.add(*idx.get_unchecked(2) as usize);
        *dst.add(3) = *src.add(*idx.get_unchecked(3) as usize);
        *dst.add(4) = *src.add(*idx.get_unchecked(4) as usize);
        *dst.add(5) = *src.add(*idx.get_unchecked(5) as usize);
        *dst.add(6) = *src.add(*idx.get_unchecked(6) as usize);
        *dst.add(7) = *src.add(*idx.get_unchecked(7) as usize);
    }
}

/// SSSE3 pshufb-based byte compaction. Loads 8 source bytes into an XMM register,
/// shuffles kept bytes to the front using COMPACT_LUT + _mm_shuffle_epi8, stores 8 bytes.
/// ~4x faster than scalar compact_8bytes: 1 pshufb vs 8 individual indexed byte copies.
/// Requires SSSE3; safe to call from AVX2 functions (which imply SSSE3).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
#[inline]
unsafe fn compact_8bytes_simd(src: *const u8, dst: *mut u8, mask: u8) {
    use std::arch::x86_64::*;
    unsafe {
        let src_v = _mm_loadl_epi64(src as *const _);
        let shuf = _mm_loadl_epi64(COMPACT_LUT.get_unchecked(mask as usize).as_ptr() as *const _);
        let out_v = _mm_shuffle_epi8(src_v, shuf);
        _mm_storel_epi64(dst as *mut _, out_v);
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
                let m0 = keep_mask as u8;
                let m1 = (keep_mask >> 8) as u8;
                if m0 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 8);
                } else if m0 != 0 {
                    compact_8bytes(sp.add(ri), dp.add(wp), m0);
                }
                let c0 = m0.count_ones() as usize;
                if m1 == 0xFF {
                    std::ptr::copy_nonoverlapping(sp.add(ri + 8), dp.add(wp + c0), 8);
                } else if m1 != 0 {
                    compact_8bytes(sp.add(ri + 8), dp.add(wp + c0), m1);
                }
                wp += c0 + m1.count_ones() as usize;
            }
            ri += 16;
        }

        // Scalar tail — branchless
        while ri < len {
            let b = *sp.add(ri);
            *dp.add(wp) = b;
            wp += (b < lo || b > hi) as usize;
            ri += 1;
        }

        wp
    }
}

/// Branchless range delete fallback for non-x86_64 (ARM64, etc.).
/// Unconditional store + conditional pointer advance eliminates branch
/// mispredictions. Unrolled 8x for better ILP on out-of-order cores.
#[cfg(not(target_arch = "x86_64"))]
fn delete_range_chunk(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) -> usize {
    let len = src.len();
    let sp = src.as_ptr();
    let dp = dst.as_mut_ptr();
    let mut wp: usize = 0;
    let mut i: usize = 0;

    // Unrolled branchless loop — 8 bytes per iteration
    while i + 8 <= len {
        unsafe {
            let b0 = *sp.add(i);
            *dp.add(wp) = b0;
            wp += (b0 < lo || b0 > hi) as usize;
            let b1 = *sp.add(i + 1);
            *dp.add(wp) = b1;
            wp += (b1 < lo || b1 > hi) as usize;
            let b2 = *sp.add(i + 2);
            *dp.add(wp) = b2;
            wp += (b2 < lo || b2 > hi) as usize;
            let b3 = *sp.add(i + 3);
            *dp.add(wp) = b3;
            wp += (b3 < lo || b3 > hi) as usize;
            let b4 = *sp.add(i + 4);
            *dp.add(wp) = b4;
            wp += (b4 < lo || b4 > hi) as usize;
            let b5 = *sp.add(i + 5);
            *dp.add(wp) = b5;
            wp += (b5 < lo || b5 > hi) as usize;
            let b6 = *sp.add(i + 6);
            *dp.add(wp) = b6;
            wp += (b6 < lo || b6 > hi) as usize;
            let b7 = *sp.add(i + 7);
            *dp.add(wp) = b7;
            wp += (b7 < lo || b7 > hi) as usize;
        }
        i += 8;
    }

    // Scalar tail
    while i < len {
        unsafe {
            let b = *sp.add(i);
            *dp.add(wp) = b;
            wp += (b < lo || b > hi) as usize;
        }
        i += 1;
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
    // Single-buffer in-place delete: eliminates the 16MB dst allocation
    // and its ~4000 page faults. For 10MB piped input, saves ~1.2ms.
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_once(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let wp = delete_range_inplace(&mut buf, n, lo, hi);
        if wp > 0 {
            writer.write_all(&buf[..wp])?;
        }
    }
    Ok(())
}

/// In-place range delete: SIMD scan for all-keep blocks + branchless scalar compaction.
/// Uses a single buffer — reads at position ri, writes at position wp (wp <= ri always).
#[inline]
fn delete_range_inplace(buf: &mut [u8], n: usize, lo: u8, hi: u8) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        let level = get_simd_level();
        if level >= 3 {
            return unsafe { delete_range_inplace_avx2(buf, n, lo, hi) };
        }
    }
    // Scalar fallback: branchless in-place delete
    let ptr = buf.as_mut_ptr();
    let mut ri = 0;
    let mut wp = 0;
    unsafe {
        while ri + 8 <= n {
            let b0 = *ptr.add(ri);
            let b1 = *ptr.add(ri + 1);
            let b2 = *ptr.add(ri + 2);
            let b3 = *ptr.add(ri + 3);
            let b4 = *ptr.add(ri + 4);
            let b5 = *ptr.add(ri + 5);
            let b6 = *ptr.add(ri + 6);
            let b7 = *ptr.add(ri + 7);
            *ptr.add(wp) = b0;
            wp += (b0 < lo || b0 > hi) as usize;
            *ptr.add(wp) = b1;
            wp += (b1 < lo || b1 > hi) as usize;
            *ptr.add(wp) = b2;
            wp += (b2 < lo || b2 > hi) as usize;
            *ptr.add(wp) = b3;
            wp += (b3 < lo || b3 > hi) as usize;
            *ptr.add(wp) = b4;
            wp += (b4 < lo || b4 > hi) as usize;
            *ptr.add(wp) = b5;
            wp += (b5 < lo || b5 > hi) as usize;
            *ptr.add(wp) = b6;
            wp += (b6 < lo || b6 > hi) as usize;
            *ptr.add(wp) = b7;
            wp += (b7 < lo || b7 > hi) as usize;
            ri += 8;
        }
        while ri < n {
            let b = *ptr.add(ri);
            *ptr.add(wp) = b;
            wp += (b < lo || b > hi) as usize;
            ri += 1;
        }
    }
    wp
}

/// AVX2 in-place range delete: scan 32 bytes at a time, skip all-keep blocks,
/// branchless scalar compaction for mixed blocks.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn delete_range_inplace_avx2(buf: &mut [u8], n: usize, lo: u8, hi: u8) -> usize {
    use std::arch::x86_64::*;

    unsafe {
        let range = hi - lo;
        let bias_v = _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let zero = _mm256_setzero_si256();

        let ptr = buf.as_mut_ptr();
        let mut ri = 0;
        let mut wp = 0;

        while ri + 32 <= n {
            let input = _mm256_loadu_si256(ptr.add(ri) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            let in_range = _mm256_cmpeq_epi8(gt, zero);
            let del_mask = _mm256_movemask_epi8(in_range) as u32;

            if del_mask == 0 {
                // All 32 bytes kept
                if wp != ri {
                    std::ptr::copy(ptr.add(ri), ptr.add(wp), 32);
                }
                wp += 32;
            } else if del_mask != 0xFFFFFFFF {
                // Mixed block: pshufb-based 8-byte compaction.
                // Process 4 × 8-byte sub-chunks using COMPACT_LUT + pshufb.
                // Each sub-chunk: load 8 bytes into register (safe for overlap),
                // shuffle kept bytes to front, store. 4 SIMD ops vs 32 scalar.
                let keep_mask = !del_mask;
                let m0 = keep_mask as u8;
                let m1 = (keep_mask >> 8) as u8;
                let m2 = (keep_mask >> 16) as u8;
                let m3 = (keep_mask >> 24) as u8;

                let c0 = m0.count_ones() as usize;
                let c1 = m1.count_ones() as usize;
                let c2 = m2.count_ones() as usize;
                let c3 = m3.count_ones() as usize;

                // Sub-chunk 0: bytes 0-7
                if m0 == 0xFF {
                    std::ptr::copy(ptr.add(ri), ptr.add(wp), 8);
                } else if m0 != 0 {
                    let src_v = _mm_loadl_epi64(ptr.add(ri) as *const _);
                    let shuf = _mm_loadl_epi64(COMPACT_LUT[m0 as usize].as_ptr() as *const _);
                    let out_v = _mm_shuffle_epi8(src_v, shuf);
                    _mm_storel_epi64(ptr.add(wp) as *mut _, out_v);
                }

                // Sub-chunk 1: bytes 8-15
                if m1 == 0xFF {
                    std::ptr::copy(ptr.add(ri + 8), ptr.add(wp + c0), 8);
                } else if m1 != 0 {
                    let src_v = _mm_loadl_epi64(ptr.add(ri + 8) as *const _);
                    let shuf = _mm_loadl_epi64(COMPACT_LUT[m1 as usize].as_ptr() as *const _);
                    let out_v = _mm_shuffle_epi8(src_v, shuf);
                    _mm_storel_epi64(ptr.add(wp + c0) as *mut _, out_v);
                }

                // Sub-chunk 2: bytes 16-23
                if m2 == 0xFF {
                    std::ptr::copy(ptr.add(ri + 16), ptr.add(wp + c0 + c1), 8);
                } else if m2 != 0 {
                    let src_v = _mm_loadl_epi64(ptr.add(ri + 16) as *const _);
                    let shuf = _mm_loadl_epi64(COMPACT_LUT[m2 as usize].as_ptr() as *const _);
                    let out_v = _mm_shuffle_epi8(src_v, shuf);
                    _mm_storel_epi64(ptr.add(wp + c0 + c1) as *mut _, out_v);
                }

                // Sub-chunk 3: bytes 24-31
                if m3 == 0xFF {
                    std::ptr::copy(ptr.add(ri + 24), ptr.add(wp + c0 + c1 + c2), 8);
                } else if m3 != 0 {
                    let src_v = _mm_loadl_epi64(ptr.add(ri + 24) as *const _);
                    let shuf = _mm_loadl_epi64(COMPACT_LUT[m3 as usize].as_ptr() as *const _);
                    let out_v = _mm_shuffle_epi8(src_v, shuf);
                    _mm_storel_epi64(ptr.add(wp + c0 + c1 + c2) as *mut _, out_v);
                }

                wp += c0 + c1 + c2 + c3;
            }
            // del_mask == 0xFFFFFFFF: all deleted, skip entirely
            ri += 32;
        }

        // Scalar tail
        while ri < n {
            let b = *ptr.add(ri);
            *ptr.add(wp) = b;
            wp += (b < lo || b > hi) as usize;
            ri += 1;
        }

        wp
    }
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
    // SAFETY: all bytes are written by read_once before being translated.
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_once(reader, &mut buf)?;
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
        let n = read_once(reader, &mut buf)?;
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
        let n = read_once(reader, &mut buf)?;
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
        let n = read_once(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
    }
    Ok(())
}

/// Single-read for pipelining: process data immediately after first read()
/// instead of blocking to fill the entire buffer. This enables cat|ftr
/// pipelining: while ftr processes the first chunk, cat continues writing
/// to the pipe. For 10MB piped input with 8MB pipe buffer, this saves
/// ~0.5-1ms by overlapping cat's final writes with ftr's processing.
#[inline]
fn read_once(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match reader.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
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
        let n = read_once(reader, &mut buf)?;
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
/// and compacts in-place with a single write_all per chunk.
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
        let n = read_once(reader, &mut buf)?;
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

        // Pass 2: in-place squeeze compaction
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

        let ptr = buf.as_mut_ptr();
        let mut wp = 0usize;

        loop {
            match finder.find(&buf[i..n]) {
                Some(offset) => {
                    let seg_end = i + offset + 1;
                    let gap = seg_end - i;
                    if gap > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(ptr.add(i) as *const u8, ptr.add(wp), gap);
                            }
                        }
                        wp += gap;
                    }
                    i = seg_end;
                    while i < n && unsafe { *buf.as_ptr().add(i) } == squeeze_ch {
                        i += 1;
                    }
                    if i >= n {
                        was_squeeze_char = true;
                        break;
                    }
                }
                None => {
                    let rem = n - i;
                    if rem > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(ptr.add(i) as *const u8, ptr.add(wp), rem);
                            }
                        }
                        wp += rem;
                    }
                    was_squeeze_char = n > 0 && unsafe { *buf.as_ptr().add(n - 1) } == squeeze_ch;
                    break;
                }
            }
        }

        if wp > 0 {
            writer.write_all(&buf[..wp])?;
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
        let n = read_once(reader, &mut buf)?;
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
    // Single-buffer in-place delete: memchr finds delete positions,
    // gap-copy backward in the same buffer. Saves 16MB dst allocation.
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_once(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut wp = 0;
        let mut i = 0;
        while i < n {
            match memchr::memchr(ch, &buf[i..n]) {
                Some(offset) => {
                    if offset > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(
                                    buf.as_ptr().add(i),
                                    buf.as_mut_ptr().add(wp),
                                    offset,
                                );
                            }
                        }
                        wp += offset;
                    }
                    i += offset + 1;
                }
                None => {
                    let run_len = n - i;
                    if run_len > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(
                                    buf.as_ptr().add(i),
                                    buf.as_mut_ptr().add(wp),
                                    run_len,
                                );
                            }
                        }
                        wp += run_len;
                    }
                    break;
                }
            }
        }
        if wp > 0 {
            writer.write_all(&buf[..wp])?;
        }
    }
    Ok(())
}

fn delete_multi_streaming(
    chars: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    // Single-buffer in-place delete: memchr2/memchr3 finds delete positions,
    // gap-copy backward in the same buffer. Saves 16MB dst allocation.
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_once(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut wp = 0;
        let mut i = 0;
        while i < n {
            let found = if chars.len() == 2 {
                memchr::memchr2(chars[0], chars[1], &buf[i..n])
            } else {
                memchr::memchr3(chars[0], chars[1], chars[2], &buf[i..n])
            };
            match found {
                Some(offset) => {
                    if offset > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(
                                    buf.as_ptr().add(i),
                                    buf.as_mut_ptr().add(wp),
                                    offset,
                                );
                            }
                        }
                        wp += offset;
                    }
                    i += offset + 1;
                }
                None => {
                    let run_len = n - i;
                    if run_len > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(
                                    buf.as_ptr().add(i),
                                    buf.as_mut_ptr().add(wp),
                                    run_len,
                                );
                            }
                        }
                        wp += run_len;
                    }
                    break;
                }
            }
        }
        if wp > 0 {
            writer.write_all(&buf[..wp])?;
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
        let n = read_once(reader, &mut buf)?;
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
        let n = read_once(reader, &mut buf)?;
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
        let n = read_once(reader, &mut buf)?;
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
    // In-place compaction: memmem finds consecutive pairs, then gap-copy
    // in the same buffer to remove duplicates. Single write_all per chunk
    // eliminates writev overhead (saves ~5-10 syscalls for 10MB input).
    let pair = [ch, ch];
    let finder = memchr::memmem::Finder::new(&pair);
    let mut buf = alloc_uninit_vec(STREAM_BUF);
    let mut was_squeeze_char = false;

    loop {
        let n = read_once(reader, &mut buf)?;
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
                continue;
            }
        }

        // In-place compaction: scan for consecutive pairs and remove duplicates.
        let ptr = buf.as_mut_ptr();
        let mut wp = 0usize;

        loop {
            match finder.find(&buf[i..n]) {
                Some(offset) => {
                    // Copy everything up to and including the first char of the pair
                    let seg_end = i + offset + 1;
                    let gap = seg_end - i;
                    if gap > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(ptr.add(i) as *const u8, ptr.add(wp), gap);
                            }
                        }
                        wp += gap;
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
                }
                None => {
                    // No more consecutive pairs — copy remainder
                    let rem = n - i;
                    if rem > 0 {
                        if wp != i {
                            unsafe {
                                std::ptr::copy(ptr.add(i) as *const u8, ptr.add(wp), rem);
                            }
                        }
                        wp += rem;
                    }
                    was_squeeze_char = n > 0 && unsafe { *buf.as_ptr().add(n - 1) } == ch;
                    break;
                }
            }
        }

        if wp > 0 {
            writer.write_all(&buf[..wp])?;
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

    // Chunked SIMD translate: 256KB buffer fits in L2 cache.
    const CHUNK: usize = 256 * 1024;
    let buf_size = data.len().min(CHUNK);
    let mut buf = alloc_uninit_vec(buf_size);
    for chunk in data.chunks(CHUNK) {
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

    // Chunked translate: 256KB buffer fits in L2 cache.
    const CHUNK: usize = 256 * 1024;
    let buf_size = data.len().min(CHUNK);
    let mut buf = alloc_uninit_vec(buf_size);
    for chunk in data.chunks(CHUNK) {
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

    // Chunked translate: 256KB buffer fits in L2 cache.
    const CHUNK: usize = 256 * 1024;
    let buf_size = data.len().min(CHUNK);
    let mut buf = alloc_uninit_vec(buf_size);
    for chunk in data.chunks(CHUNK) {
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
    // Threshold: 64MB. For benchmark-sized files (10MB), avoid COW entirely.
    const SEPARATE_BUF_THRESHOLD: usize = 64 * 1024 * 1024;

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
/// For smaller data: single full-size allocation + single write_all for minimum
/// syscall overhead. At 10MB, the allocation is cheap and a single write() is faster
/// than multiple 4MB chunked writes.
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
                    translate_range_to_constant_simd(
                        src,
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

    // Single-allocation translate: full-size output buffer, single translate, single write.
    // For 10MB data, this does 1 write() instead of 40 chunked writes, eliminating
    // 39 write() syscalls. SIMD translate streams through src and dst sequentially,
    // so the L2 cache argument for 256KB chunks doesn't apply (src data doesn't fit
    // in L2 anyway). The reduced syscall overhead more than compensates.
    let mut out_buf = alloc_uninit_vec(data.len());
    if let Some((lo, hi, offset)) = range_info {
        translate_range_simd(data, &mut out_buf, lo, hi, offset);
    } else if let Some((lo, hi, replacement)) = const_info {
        translate_range_to_constant_simd(data, &mut out_buf, lo, hi, replacement);
    } else {
        translate_to(data, &mut out_buf, table);
    }
    writer.write_all(&out_buf)
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

    // Single-allocation translate+squeeze: full-size buffer, single write_all.
    // For 10MB data, this does 1 write() instead of ~40 chunked writes.
    let mut buf = alloc_uninit_vec(data.len());
    translate_to(data, &mut buf, &table);
    let mut last_squeezed: u16 = 256;
    let mut wp = 0;
    unsafe {
        let ptr = buf.as_mut_ptr();
        for i in 0..data.len() {
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
    writer.write_all(&buf[..wp])
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

    // Heuristic: estimate total delete positions. Zero-copy writev is only efficient
    // when all gaps fit in a single writev call (< MAX_IOV/2 entries). With uniform
    // distribution, each delete creates an IoSlice entry. For many deletes (> 512),
    // multiple writev calls are needed, and the compact approach is faster.
    let sample_size = data.len().min(1024);
    let sample_deletes = data[..sample_size]
        .iter()
        .filter(|&&b| is_member(&member, b))
        .count();
    let estimated_deletes = if sample_size > 0 {
        data.len() * sample_deletes / sample_size
    } else {
        data.len()
    };

    if estimated_deletes < MAX_IOV / 2 {
        return delete_bitset_zerocopy(data, &member, writer);
    }

    // Dense delete: parallel compact with writev (avoids scatter-gather copy)
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let mut outbuf = alloc_uninit_vec(data.len());
        let chunk_lens: Vec<usize> = data
            .par_chunks(chunk_size)
            .zip(outbuf.par_chunks_mut(chunk_size))
            .map(|(src_chunk, dst_chunk)| delete_chunk_bitset_into(src_chunk, &member, dst_chunk))
            .collect();

        // Use writev to write each chunk at its original position, avoiding
        // the O(N) scatter-gather memmove. With ~4 threads, that's 4 IoSlice
        // entries — far below MAX_IOV.
        let slices: Vec<std::io::IoSlice> = chunk_lens
            .iter()
            .enumerate()
            .filter(|&(_, &len)| len > 0)
            .map(|(i, &len)| std::io::IoSlice::new(&outbuf[i * chunk_size..i * chunk_size + len]))
            .collect();
        return write_ioslices(writer, &slices);
    }

    // Streaming compact: 256KB output buffer reduces page fault overhead.
    // For 10MB data: ~64 page faults instead of ~2500, with ~40 write_all calls.
    const COMPACT_BUF: usize = 256 * 1024;
    let mut outbuf = alloc_uninit_vec(COMPACT_BUF);

    for chunk in data.chunks(COMPACT_BUF) {
        let out_pos = delete_chunk_bitset_into(chunk, &member, &mut outbuf);
        if out_pos > 0 {
            writer.write_all(&outbuf[..out_pos])?;
        }
    }
    Ok(())
}

/// SIMD range delete for mmap data.
/// Uses a density heuristic: for sparse deletes (< 15%), uses zero-copy writev
/// directly from mmap data (no output buffer allocation). For dense deletes,
/// uses SIMD compact into a pre-allocated buffer.
fn delete_range_mmap(data: &[u8], writer: &mut impl Write, lo: u8, hi: u8) -> io::Result<()> {
    // Sample first 1024 bytes to estimate delete density
    let sample_size = data.len().min(1024);
    let sample_deletes = data[..sample_size]
        .iter()
        .filter(|&&b| b >= lo && b <= hi)
        .count();
    // Estimate expected number of delete positions (IoSlice entries for zero-copy).
    // Each delete creates an IoSlice entry. With MAX_IOV=1024 per writev,
    // if estimated_deletes > MAX_IOV/2, the writev overhead from multiple syscalls
    // exceeds the compact approach cost. Only use zero-copy when all gaps fit in
    // a single writev call.
    let estimated_deletes = if sample_size > 0 {
        data.len() * sample_deletes / sample_size
    } else {
        data.len()
    };
    if estimated_deletes < MAX_IOV / 2 {
        return delete_range_mmap_zerocopy(data, writer, lo, hi);
    }

    // Dense deletes: parallel compact with writev (avoids scatter-gather copy)
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let mut outbuf = alloc_uninit_vec(data.len());
        let chunk_lens: Vec<usize> = data
            .par_chunks(chunk_size)
            .zip(outbuf.par_chunks_mut(chunk_size))
            .map(|(src_chunk, dst_chunk)| delete_range_chunk(src_chunk, dst_chunk, lo, hi))
            .collect();

        // Use writev to write each chunk at its original position, avoiding
        // the O(N) scatter-gather memmove.
        let slices: Vec<std::io::IoSlice> = chunk_lens
            .iter()
            .enumerate()
            .filter(|&(_, &len)| len > 0)
            .map(|(i, &len)| std::io::IoSlice::new(&outbuf[i * chunk_size..i * chunk_size + len]))
            .collect();
        return write_ioslices(writer, &slices);
    }

    // Streaming compact: use 256KB output buffer instead of full data.len() buffer.
    // This reduces page fault overhead from ~2500 faults (10MB) to ~64 faults (256KB).
    // The extra write_all calls (~40 for 10MB) are negligible cost.
    const COMPACT_BUF: usize = 256 * 1024;
    let mut outbuf = alloc_uninit_vec(COMPACT_BUF);

    #[cfg(target_arch = "x86_64")]
    {
        let mut wp = 0;
        let level = get_simd_level();
        let len = data.len();
        let sp = data.as_ptr();
        let dp = outbuf.as_mut_ptr();
        let mut ri = 0;

        if level >= 3 {
            use std::arch::x86_64::*;
            let range = hi - lo;
            let bias_v = unsafe { _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8) };
            let threshold_v = unsafe { _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8) };
            let zero = unsafe { _mm256_setzero_si256() };

            while ri + 32 <= len {
                // Flush when output buffer is nearly full
                if wp + 32 > COMPACT_BUF {
                    writer.write_all(&outbuf[..wp])?;
                    wp = 0;
                }

                let input = unsafe { _mm256_loadu_si256(sp.add(ri) as *const _) };
                let biased = unsafe { _mm256_add_epi8(input, bias_v) };
                let gt = unsafe { _mm256_cmpgt_epi8(biased, threshold_v) };
                let in_range = unsafe { _mm256_cmpeq_epi8(gt, zero) };
                let keep_mask = !(unsafe { _mm256_movemask_epi8(in_range) } as u32);

                if keep_mask == 0xFFFFFFFF {
                    unsafe { std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 32) };
                    wp += 32;
                } else if keep_mask != 0 {
                    let m0 = keep_mask as u8;
                    let m1 = (keep_mask >> 8) as u8;
                    let m2 = (keep_mask >> 16) as u8;
                    let m3 = (keep_mask >> 24) as u8;

                    if m0 == 0xFF {
                        unsafe { std::ptr::copy_nonoverlapping(sp.add(ri), dp.add(wp), 8) };
                    } else if m0 != 0 {
                        unsafe { compact_8bytes_simd(sp.add(ri), dp.add(wp), m0) };
                    }
                    let c0 = m0.count_ones() as usize;

                    if m1 == 0xFF {
                        unsafe {
                            std::ptr::copy_nonoverlapping(sp.add(ri + 8), dp.add(wp + c0), 8)
                        };
                    } else if m1 != 0 {
                        unsafe { compact_8bytes_simd(sp.add(ri + 8), dp.add(wp + c0), m1) };
                    }
                    let c1 = m1.count_ones() as usize;

                    if m2 == 0xFF {
                        unsafe {
                            std::ptr::copy_nonoverlapping(sp.add(ri + 16), dp.add(wp + c0 + c1), 8)
                        };
                    } else if m2 != 0 {
                        unsafe { compact_8bytes_simd(sp.add(ri + 16), dp.add(wp + c0 + c1), m2) };
                    }
                    let c2 = m2.count_ones() as usize;

                    if m3 == 0xFF {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                sp.add(ri + 24),
                                dp.add(wp + c0 + c1 + c2),
                                8,
                            )
                        };
                    } else if m3 != 0 {
                        unsafe {
                            compact_8bytes_simd(sp.add(ri + 24), dp.add(wp + c0 + c1 + c2), m3)
                        };
                    }
                    let c3 = m3.count_ones() as usize;
                    wp += c0 + c1 + c2 + c3;
                }
                ri += 32;
            }
        }

        // Scalar tail
        while ri < len {
            if wp + 1 > COMPACT_BUF {
                writer.write_all(&outbuf[..wp])?;
                wp = 0;
            }
            let b = unsafe { *sp.add(ri) };
            unsafe { *dp.add(wp) = b };
            wp += (b < lo || b > hi) as usize;
            ri += 1;
        }

        if wp > 0 {
            writer.write_all(&outbuf[..wp])?;
        }
        return Ok(());
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // Non-x86 fallback: chunk the source and process with delete_range_chunk
        for chunk in data.chunks(COMPACT_BUF) {
            let clen = delete_range_chunk(chunk, &mut outbuf, lo, hi);
            if clen > 0 {
                writer.write_all(&outbuf[..clen])?;
            }
        }
        return Ok(());
    }

    #[allow(unreachable_code)]
    Ok(())
}

/// Zero-copy range delete for mmap data: SIMD-scans for bytes in [lo..=hi],
/// builds IoSlice entries pointing to the gaps between deleted ranges in the
/// original mmap data, and writes using writev. No output buffer allocation.
/// For 10MB text with 4% digits: ~1.5ms vs ~4ms for the compact approach.
fn delete_range_mmap_zerocopy(
    data: &[u8],
    writer: &mut impl Write,
    lo: u8,
    hi: u8,
) -> io::Result<()> {
    #[cfg(target_arch = "x86_64")]
    {
        if get_simd_level() >= 3 {
            return unsafe { delete_range_zerocopy_avx2(data, writer, lo, hi) };
        }
        if get_simd_level() >= 2 {
            return unsafe { delete_range_zerocopy_sse2(data, writer, lo, hi) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { delete_range_zerocopy_neon(data, writer, lo, hi) };
    }

    // Scalar fallback: byte-by-byte scan with IoSlice batching
    #[allow(unreachable_code)]
    delete_range_zerocopy_scalar(data, writer, lo, hi)
}

/// Scalar zero-copy range delete: byte-by-byte scan with IoSlice batching.
/// Used as fallback when SIMD is unavailable.
fn delete_range_zerocopy_scalar(
    data: &[u8],
    writer: &mut impl Write,
    lo: u8,
    hi: u8,
) -> io::Result<()> {
    let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(MAX_IOV);
    let len = data.len();
    let mut run_start: usize = 0;
    let mut i: usize = 0;

    while i < len {
        let b = unsafe { *data.get_unchecked(i) };
        if b >= lo && b <= hi {
            if i > run_start {
                iov.push(std::io::IoSlice::new(&data[run_start..i]));
                if iov.len() >= MAX_IOV {
                    write_ioslices(writer, &iov)?;
                    iov.clear();
                }
            }
            run_start = i + 1;
        }
        i += 1;
    }
    if run_start < len {
        iov.push(std::io::IoSlice::new(&data[run_start..]));
    }
    if !iov.is_empty() {
        write_ioslices(writer, &iov)?;
    }
    Ok(())
}

/// AVX2 zero-copy range delete: scans 32 bytes at a time using SIMD range
/// comparison, then iterates only the delete positions from the bitmask.
/// Blocks with no deletes (common for sparse data) skip with zero per-byte work.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn delete_range_zerocopy_avx2(
    data: &[u8],
    writer: &mut impl Write,
    lo: u8,
    hi: u8,
) -> io::Result<()> {
    use std::arch::x86_64::*;

    unsafe {
        let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(MAX_IOV);
        let len = data.len();
        let mut run_start: usize = 0;
        let mut ri: usize = 0;

        let range = hi - lo;
        let bias_v = _mm256_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm256_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let zero = _mm256_setzero_si256();

        while ri + 32 <= len {
            let input = _mm256_loadu_si256(data.as_ptr().add(ri) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            let in_range = _mm256_cmpeq_epi8(gt, zero);
            let del_mask = _mm256_movemask_epi8(in_range) as u32;

            if del_mask == 0 {
                // No bytes to delete — run continues
                ri += 32;
                continue;
            }

            // Process each deleted byte position from the bitmask
            let mut m = del_mask;
            while m != 0 {
                let bit = m.trailing_zeros() as usize;
                let abs_pos = ri + bit;
                if abs_pos > run_start {
                    iov.push(std::io::IoSlice::new(&data[run_start..abs_pos]));
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                run_start = abs_pos + 1;
                m &= m - 1; // clear lowest set bit (blsr)
            }

            ri += 32;
        }

        // Scalar tail
        while ri < len {
            let b = *data.get_unchecked(ri);
            if b >= lo && b <= hi {
                if ri > run_start {
                    iov.push(std::io::IoSlice::new(&data[run_start..ri]));
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                run_start = ri + 1;
            }
            ri += 1;
        }

        if run_start < len {
            iov.push(std::io::IoSlice::new(&data[run_start..]));
        }
        if !iov.is_empty() {
            write_ioslices(writer, &iov)?;
        }
        Ok(())
    }
}

/// SSE2 zero-copy range delete: same approach as AVX2 but with 16-byte blocks.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn delete_range_zerocopy_sse2(
    data: &[u8],
    writer: &mut impl Write,
    lo: u8,
    hi: u8,
) -> io::Result<()> {
    use std::arch::x86_64::*;

    unsafe {
        let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(MAX_IOV);
        let len = data.len();
        let mut run_start: usize = 0;
        let mut ri: usize = 0;

        let range = hi - lo;
        let bias_v = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
        let threshold_v = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
        let zero = _mm_setzero_si128();

        while ri + 16 <= len {
            let input = _mm_loadu_si128(data.as_ptr().add(ri) as *const _);
            let biased = _mm_add_epi8(input, bias_v);
            let gt = _mm_cmpgt_epi8(biased, threshold_v);
            let in_range = _mm_cmpeq_epi8(gt, zero);
            let del_mask = _mm_movemask_epi8(in_range) as u32 & 0xFFFF;

            if del_mask == 0 {
                ri += 16;
                continue;
            }

            let mut m = del_mask;
            while m != 0 {
                let bit = m.trailing_zeros() as usize;
                let abs_pos = ri + bit;
                if abs_pos > run_start {
                    iov.push(std::io::IoSlice::new(&data[run_start..abs_pos]));
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                run_start = abs_pos + 1;
                m &= m - 1;
            }

            ri += 16;
        }

        while ri < len {
            let b = *data.get_unchecked(ri);
            if b >= lo && b <= hi {
                if ri > run_start {
                    iov.push(std::io::IoSlice::new(&data[run_start..ri]));
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                run_start = ri + 1;
            }
            ri += 1;
        }

        if run_start < len {
            iov.push(std::io::IoSlice::new(&data[run_start..]));
        }
        if !iov.is_empty() {
            write_ioslices(writer, &iov)?;
        }
        Ok(())
    }
}

/// NEON zero-copy range delete for aarch64: scans 16 bytes at a time using
/// NEON unsigned comparison, creates bitmask via pairwise narrowing, then
/// iterates delete positions from the bitmask.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn delete_range_zerocopy_neon(
    data: &[u8],
    writer: &mut impl Write,
    lo: u8,
    hi: u8,
) -> io::Result<()> {
    use std::arch::aarch64::*;

    unsafe {
        let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(MAX_IOV);
        let len = data.len();
        let mut run_start: usize = 0;
        let mut ri: usize = 0;

        let lo_v = vdupq_n_u8(lo);
        let hi_v = vdupq_n_u8(hi);
        // Bit position mask for extracting bitmask from comparison results
        let bit_mask: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];
        let bit_mask_v = vld1q_u8(bit_mask.as_ptr());

        while ri + 16 <= len {
            let input = vld1q_u8(data.as_ptr().add(ri));
            // in_range = 0xFF where lo <= byte <= hi
            let ge_lo = vcgeq_u8(input, lo_v);
            let le_hi = vcleq_u8(input, hi_v);
            let in_range = vandq_u8(ge_lo, le_hi);

            // Create 16-bit bitmask: reduce 16 bytes to 2 bytes
            let bits = vandq_u8(in_range, bit_mask_v);
            let pair = vpaddlq_u8(bits); // u8→u16 pairwise add
            let quad = vpaddlq_u16(pair); // u16→u32
            let octet = vpaddlq_u32(quad); // u32→u64
            let mask_lo = vgetq_lane_u64::<0>(octet) as u8;
            let mask_hi = vgetq_lane_u64::<1>(octet) as u8;
            let del_mask = (mask_hi as u16) << 8 | mask_lo as u16;

            if del_mask == 0 {
                // No bytes to delete — run continues
                ri += 16;
                continue;
            }

            // Process each deleted byte position
            let mut m = del_mask;
            while m != 0 {
                let bit = m.trailing_zeros() as usize;
                let abs_pos = ri + bit;
                if abs_pos > run_start {
                    iov.push(std::io::IoSlice::new(&data[run_start..abs_pos]));
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                run_start = abs_pos + 1;
                m &= m - 1;
            }

            ri += 16;
        }

        // Scalar tail
        while ri < len {
            let b = *data.get_unchecked(ri);
            if b >= lo && b <= hi {
                if ri > run_start {
                    iov.push(std::io::IoSlice::new(&data[run_start..ri]));
                    if iov.len() >= MAX_IOV {
                        write_ioslices(writer, &iov)?;
                        iov.clear();
                    }
                }
                run_start = ri + 1;
            }
            ri += 1;
        }

        if run_start < len {
            iov.push(std::io::IoSlice::new(&data[run_start..]));
        }
        if !iov.is_empty() {
            write_ioslices(writer, &iov)?;
        }
        Ok(())
    }
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
    // Streaming zero-copy delete using writev: build IoSlice batches of MAX_IOV
    // pointing to gaps between deleted characters, write each batch immediately.
    // Avoids allocating the full Vec<IoSlice> for all positions.
    let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut last = 0;
    for pos in memchr::memchr_iter(ch, data) {
        if pos > last {
            iov.push(std::io::IoSlice::new(&data[last..pos]));
            if iov.len() >= MAX_IOV {
                write_ioslices(writer, &iov)?;
                iov.clear();
            }
        }
        last = pos + 1;
    }
    if last < data.len() {
        iov.push(std::io::IoSlice::new(&data[last..]));
    }
    if !iov.is_empty() {
        write_ioslices(writer, &iov)?;
    }
    Ok(())
}

fn delete_multi_memchr_mmap(chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    let c0 = chars[0];
    let c1 = if chars.len() >= 2 { chars[1] } else { 0 };
    let c2 = if chars.len() >= 3 { chars[2] } else { 0 };
    let is_three = chars.len() >= 3;

    // Streaming zero-copy delete: batch IoSlice entries and write in groups of MAX_IOV.
    let mut iov: Vec<std::io::IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut last = 0;

    macro_rules! process_pos {
        ($pos:expr) => {
            if $pos > last {
                iov.push(std::io::IoSlice::new(&data[last..$pos]));
                if iov.len() >= MAX_IOV {
                    write_ioslices(writer, &iov)?;
                    iov.clear();
                }
            }
            last = $pos + 1;
        };
    }

    if is_three {
        for pos in memchr::memchr3_iter(c0, c1, c2, data) {
            process_pos!(pos);
        }
    } else {
        for pos in memchr::memchr2_iter(c0, c1, data) {
            process_pos!(pos);
        }
    }
    if last < data.len() {
        iov.push(std::io::IoSlice::new(&data[last..]));
    }
    if !iov.is_empty() {
        write_ioslices(writer, &iov)?;
    }
    Ok(())
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

    // Single-allocation delete+squeeze: full-size buffer, single write_all.
    let mut outbuf = alloc_uninit_vec(data.len());
    let mut last_squeezed: u16 = 256;
    let mut out_pos = 0;

    for &b in data.iter() {
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
    writer.write_all(&outbuf[..out_pos])
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

    // Single-allocation squeeze: full-size buffer, single write_all.
    let mut outbuf = alloc_uninit_vec(data.len());
    let len = data.len();
    let mut wp = 0;
    let mut i = 0;
    let mut last_squeezed: u16 = 256;

    unsafe {
        let inp = data.as_ptr();
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
    writer.write_all(&outbuf[..wp])
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
