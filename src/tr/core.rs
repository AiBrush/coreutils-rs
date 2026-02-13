use std::io::{self, Read, Write};

use rayon::prelude::*;

/// Maximum IoSlice entries per write_vectored batch.
/// Linux UIO_MAXIOV is 1024; we use that as our batch limit.
const MAX_IOV: usize = 1024;

/// Main processing buffer: 4MB (fits in L3 cache, avoids cache thrashing).
const BUF_SIZE: usize = 4 * 1024 * 1024;

/// Stream buffer: 8MB — larger buffer = fewer read/write syscalls for streaming.
const STREAM_BUF: usize = 8 * 1024 * 1024;

/// Minimum data size to engage rayon parallel processing for mmap paths.
/// Below this, single-threaded is faster due to thread pool overhead.
const PARALLEL_THRESHOLD: usize = 2 * 1024 * 1024;

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

/// Translate bytes in-place using a 256-byte lookup table.
#[inline(always)]
fn translate_inplace(data: &mut [u8], table: &[u8; 256]) {
    for b in data.iter_mut() {
        *b = unsafe { *table.get_unchecked(*b as usize) };
    }
}

/// Translate bytes from source to destination using a 256-byte lookup table.
#[inline(always)]
fn translate_to(src: &[u8], dst: &mut [u8], table: &[u8; 256]) {
    debug_assert!(dst.len() >= src.len());
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

/// SIMD-accelerated range translation for mmap'd data.
/// For tables where only a contiguous range [lo..=hi] is translated by a constant offset,
/// uses AVX2 (32 bytes/iter) or SSE2 (16 bytes/iter) vectorized arithmetic.
#[cfg(target_arch = "x86_64")]
fn translate_range_simd(src: &[u8], dst: &mut [u8], lo: u8, hi: u8, offset: i8) {
    if is_x86_feature_detected!("avx2") {
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
        let mut i = 0;

        while i + 32 <= len {
            let input = _mm256_loadu_si256(src.as_ptr().add(i) as *const _);
            let biased = _mm256_add_epi8(input, bias_v);
            // gt = 0xFF where biased > threshold (OUT of range)
            let gt = _mm256_cmpgt_epi8(biased, threshold_v);
            // mask = 0xFF where IN range (NOT gt)
            let mask = _mm256_cmpeq_epi8(gt, zero);
            let offset_masked = _mm256_and_si256(mask, offset_v);
            let result = _mm256_add_epi8(input, offset_masked);
            _mm256_storeu_si256(dst.as_mut_ptr().add(i) as *mut _, result);
            i += 32;
        }

        // SSE2 tail for 16-byte remainder
        if i + 16 <= len {
            let bias_v128 = _mm_set1_epi8(0x80u8.wrapping_sub(lo) as i8);
            let threshold_v128 = _mm_set1_epi8(0x80u8.wrapping_add(range) as i8);
            let offset_v128 = _mm_set1_epi8(offset);
            let zero128 = _mm_setzero_si128();

            let input = _mm_loadu_si128(src.as_ptr().add(i) as *const _);
            let biased = _mm_add_epi8(input, bias_v128);
            let gt = _mm_cmpgt_epi8(biased, threshold_v128);
            let mask = _mm_cmpeq_epi8(gt, zero128);
            let offset_masked = _mm_and_si128(mask, offset_v128);
            let result = _mm_add_epi8(input, offset_masked);
            _mm_storeu_si128(dst.as_mut_ptr().add(i) as *mut _, result);
            i += 16;
        }

        // Scalar tail
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
    if is_x86_feature_detected!("avx2") {
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

        while i + 32 <= len {
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
// Streaming functions (Read + Write)
// ============================================================================

pub fn translate(
    set1: &[u8],
    set2: &[u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);

    // Try SIMD fast path for range translations (in-place, single buffer)
    if let Some((lo, hi, offset)) = detect_range_offset(&table) {
        return translate_range_stream(lo, hi, offset, reader, writer);
    }

    // General case: use separate src/dst buffers with 8x-unrolled translate_to.
    // This avoids the read-modify-write cache penalty of in-place translation:
    // reading and writing the same cache line forces store-to-load forwarding stalls.
    // With separate buffers, the CPU can pipeline reads from src while writing to dst.
    let mut src = vec![0u8; STREAM_BUF];
    let mut dst = alloc_uninit_vec(STREAM_BUF);
    loop {
        let n = read_full(reader, &mut src)?;
        if n == 0 {
            break;
        }
        translate_to(&src[..n], &mut dst[..n], &table);
        writer.write_all(&dst[..n])?;
    }
    Ok(())
}

/// Streaming SIMD range translation — single buffer, in-place transform.
/// Saves 16MB allocation + memcpy vs separate src/dst buffers.
fn translate_range_stream(
    lo: u8,
    hi: u8,
    offset: i8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut buf = vec![0u8; STREAM_BUF];
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

/// Read as many bytes as possible into buf, retrying on partial reads.
#[inline]
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
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

    // Two-pass optimization for range translations:
    // Pass 1: SIMD range translate in-place (10x faster than scalar table lookup)
    // Pass 2: scalar squeeze (inherently sequential due to state dependency)
    // Even though it's two passes, the translate pass is so much faster with SIMD
    // that the total is still a net win.
    let range_info = detect_range_offset(&table);

    let mut buf = vec![0u8; STREAM_BUF];
    let mut last_squeezed: u16 = 256;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        // Pass 1: translate
        if let Some((lo, hi, offset)) = range_info {
            translate_range_simd_inplace(&mut buf[..n], lo, hi, offset);
        } else {
            translate_inplace(&mut buf[..n], &table);
        }
        // Pass 2: squeeze in-place
        let mut wp = 0;
        unsafe {
            let ptr = buf.as_mut_ptr();
            for i in 0..n {
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

    let member = build_member_set(delete_chars);
    let mut buf = vec![0u8; STREAM_BUF];

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

                if !is_member(&member, b0) {
                    *ptr.add(wp) = b0;
                    wp += 1;
                }
                if !is_member(&member, b1) {
                    *ptr.add(wp) = b1;
                    wp += 1;
                }
                if !is_member(&member, b2) {
                    *ptr.add(wp) = b2;
                    wp += 1;
                }
                if !is_member(&member, b3) {
                    *ptr.add(wp) = b3;
                    wp += 1;
                }
                if !is_member(&member, b4) {
                    *ptr.add(wp) = b4;
                    wp += 1;
                }
                if !is_member(&member, b5) {
                    *ptr.add(wp) = b5;
                    wp += 1;
                }
                if !is_member(&member, b6) {
                    *ptr.add(wp) = b6;
                    wp += 1;
                }
                if !is_member(&member, b7) {
                    *ptr.add(wp) = b7;
                    wp += 1;
                }
                i += 8;
            }
            while i < n {
                let b = *ptr.add(i);
                if !is_member(&member, b) {
                    *ptr.add(wp) = b;
                    wp += 1;
                }
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
    let mut buf = vec![0u8; STREAM_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
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
        writer.write_all(&buf[..wp])?;
    }
    Ok(())
}

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
        writer.write_all(&buf[..wp])?;
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
    let mut buf = vec![0u8; STREAM_BUF];
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
                *ptr.add(wp) = b;
                wp += 1;
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

    let member = build_member_set(squeeze_chars);
    let mut buf = vec![0u8; STREAM_BUF];
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

fn squeeze_single_stream(
    ch: u8,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut buf = vec![0u8; STREAM_BUF];
    let mut was_squeeze_char = false;

    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        let mut wp = 0;
        let mut i = 0;

        while i < n {
            if was_squeeze_char && buf[i] == ch {
                i += 1;
                while i < n && buf[i] == ch {
                    i += 1;
                }
                if i >= n {
                    break;
                }
            }

            match memchr::memchr(ch, &buf[i..n]) {
                Some(offset) => {
                    let run_len = offset;
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
                    i += run_len;

                    unsafe {
                        *buf.as_mut_ptr().add(wp) = ch;
                    }
                    wp += 1;
                    was_squeeze_char = true;
                    i += 1;
                    while i < n && buf[i] == ch {
                        i += 1;
                    }
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
                    was_squeeze_char = false;
                    break;
                }
            }
        }

        writer.write_all(&buf[..wp])?;
    }
    Ok(())
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

    let member = build_member_set(delete_chars);

    // Parallel path: pre-allocate a single output buffer of data.len() and have each
    // thread write to its non-overlapping slice, then do a single write_all.
    // This avoids per-chunk Vec allocations that the old approach had.
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        // Each thread deletes into its slice of outbuf and returns bytes written.
        let mut outbuf = alloc_uninit_vec(data.len());
        let chunk_lens: Vec<usize> = data
            .par_chunks(chunk_size)
            .zip(outbuf.par_chunks_mut(chunk_size))
            .map(|(src_chunk, dst_chunk)| delete_chunk_bitset_into(src_chunk, &member, dst_chunk))
            .collect();

        // Compact: move each chunk's output to be contiguous.
        // chunk_lens[i] is how many bytes thread i wrote into its slice.
        // We need to shift them together since each dst_chunk started at chunk_size offsets.
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

    // Single-write fast path: delete into one buffer, one write
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut outbuf = alloc_uninit_vec(data.len());
        let out_pos = delete_chunk_bitset_into(data, &member, &mut outbuf);
        return writer.write_all(&outbuf[..out_pos]);
    }

    // Chunked path for large data
    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = alloc_uninit_vec(buf_size);

    for chunk in data.chunks(buf_size) {
        let out_pos = delete_chunk_bitset_into(chunk, &member, &mut outbuf);
        writer.write_all(&outbuf[..out_pos])?;
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

fn delete_single_char_mmap(ch: u8, data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    // Parallel path for large data: each thread deletes from its chunk,
    // then use writev to write all results in one syscall batch.
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let results: Vec<Vec<u8>> = data
            .par_chunks(chunk_size)
            .map(|chunk| {
                let mut out = Vec::with_capacity(chunk.len());
                let mut last = 0;
                for pos in memchr::memchr_iter(ch, chunk) {
                    if pos > last {
                        out.extend_from_slice(&chunk[last..pos]);
                    }
                    last = pos + 1;
                }
                if last < chunk.len() {
                    out.extend_from_slice(&chunk[last..]);
                }
                out
            })
            .collect();

        // Use writev to batch all results into fewer syscalls
        let slices: Vec<std::io::IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| std::io::IoSlice::new(r))
            .collect();
        return write_ioslices(writer, &slices);
    }

    // Single-write fast path: collect all non-deleted spans into one buffer
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut outbuf = Vec::with_capacity(data.len());
        let mut last = 0;
        for pos in memchr::memchr_iter(ch, data) {
            if pos > last {
                outbuf.extend_from_slice(&data[last..pos]);
            }
            last = pos + 1;
        }
        if last < data.len() {
            outbuf.extend_from_slice(&data[last..]);
        }
        return writer.write_all(&outbuf);
    }

    // Chunked path for large data
    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = vec![0u8; buf_size];

    for chunk in data.chunks(buf_size) {
        let mut wp = 0;
        let mut last = 0;
        for pos in memchr::memchr_iter(ch, chunk) {
            if pos > last {
                let run = pos - last;
                outbuf[wp..wp + run].copy_from_slice(&chunk[last..pos]);
                wp += run;
            }
            last = pos + 1;
        }
        if last < chunk.len() {
            let run = chunk.len() - last;
            outbuf[wp..wp + run].copy_from_slice(&chunk[last..]);
            wp += run;
        }
        writer.write_all(&outbuf[..wp])?;
    }
    Ok(())
}

fn delete_multi_memchr_mmap(chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    let c0 = chars[0];
    let c1 = if chars.len() >= 2 { chars[1] } else { 0 };
    let c2 = if chars.len() >= 3 { chars[2] } else { 0 };
    let is_three = chars.len() >= 3;

    // Parallel path for large data
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let results: Vec<Vec<u8>> = data
            .par_chunks(chunk_size)
            .map(|chunk| {
                let mut out = Vec::with_capacity(chunk.len());
                let mut last = 0;
                if is_three {
                    for pos in memchr::memchr3_iter(c0, c1, c2, chunk) {
                        if pos > last {
                            out.extend_from_slice(&chunk[last..pos]);
                        }
                        last = pos + 1;
                    }
                } else {
                    for pos in memchr::memchr2_iter(c0, c1, chunk) {
                        if pos > last {
                            out.extend_from_slice(&chunk[last..pos]);
                        }
                        last = pos + 1;
                    }
                }
                if last < chunk.len() {
                    out.extend_from_slice(&chunk[last..]);
                }
                out
            })
            .collect();

        // Use writev to batch all results into fewer syscalls
        let slices: Vec<std::io::IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| std::io::IoSlice::new(r))
            .collect();
        return write_ioslices(writer, &slices);
    }

    // Single-write fast path: collect all non-deleted spans into one buffer
    if data.len() <= SINGLE_WRITE_LIMIT {
        let mut outbuf = Vec::with_capacity(data.len());
        let mut last = 0;
        if is_three {
            for pos in memchr::memchr3_iter(c0, c1, c2, data) {
                if pos > last {
                    outbuf.extend_from_slice(&data[last..pos]);
                }
                last = pos + 1;
            }
        } else {
            for pos in memchr::memchr2_iter(c0, c1, data) {
                if pos > last {
                    outbuf.extend_from_slice(&data[last..pos]);
                }
                last = pos + 1;
            }
        }
        if last < data.len() {
            outbuf.extend_from_slice(&data[last..]);
        }
        return writer.write_all(&outbuf);
    }

    // Chunked path for large data
    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = vec![0u8; buf_size];

    for chunk in data.chunks(buf_size) {
        let mut wp = 0;
        let mut last = 0;

        // Iterate directly over memchr iterator without collecting into Vec<usize>.
        // Positions are used exactly once in order, so no intermediate allocation needed.
        if is_three {
            for pos in memchr::memchr3_iter(c0, c1, c2, chunk) {
                if pos > last {
                    let run = pos - last;
                    outbuf[wp..wp + run].copy_from_slice(&chunk[last..pos]);
                    wp += run;
                }
                last = pos + 1;
            }
        } else {
            for pos in memchr::memchr2_iter(c0, c1, chunk) {
                if pos > last {
                    let run = pos - last;
                    outbuf[wp..wp + run].copy_from_slice(&chunk[last..pos]);
                    wp += run;
                }
                last = pos + 1;
            }
        }

        if last < chunk.len() {
            let run = chunk.len() - last;
            outbuf[wp..wp + run].copy_from_slice(&chunk[last..]);
            wp += run;
        }
        writer.write_all(&outbuf[..wp])?;
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

    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = vec![0u8; buf_size];
    let mut wp = 0;
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

    macro_rules! flush_and_copy {
        ($src:expr, $len:expr) => {
            if wp + $len > buf_size {
                writer.write_all(&outbuf[..wp])?;
                wp = 0;
            }
            if $len > buf_size {
                writer.write_all($src)?;
            } else {
                outbuf[wp..wp + $len].copy_from_slice($src);
                wp += $len;
            }
        };
    }

    while cursor < data.len() {
        match find_next!(&data[cursor..]) {
            Some(offset) => {
                let pos = cursor + offset;
                let b = data[pos];
                if pos > cursor {
                    let span = pos - cursor;
                    flush_and_copy!(&data[cursor..pos], span);
                    last_squeezed = 256;
                }
                if last_squeezed != b as u16 {
                    if wp >= buf_size {
                        writer.write_all(&outbuf[..wp])?;
                        wp = 0;
                    }
                    outbuf[wp] = b;
                    wp += 1;
                    last_squeezed = b as u16;
                }
                let mut skip = pos + 1;
                while skip < data.len() && data[skip] == b {
                    skip += 1;
                }
                cursor = skip;
            }
            None => {
                let remaining = data.len() - cursor;
                flush_and_copy!(&data[cursor..], remaining);
                break;
            }
        }
    }
    if wp > 0 {
        writer.write_all(&outbuf[..wp])?;
    }
    Ok(())
}

fn squeeze_single_mmap(ch: u8, data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if memchr::memmem::find(data, &[ch, ch]).is_none() {
        return writer.write_all(data);
    }

    // Parallel path: squeeze each chunk, fix boundaries
    if data.len() >= PARALLEL_THRESHOLD {
        let n_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() / n_threads).max(32 * 1024);

        let results: Vec<Vec<u8>> = data
            .par_chunks(chunk_size)
            .map(|chunk| {
                let mut out = Vec::with_capacity(chunk.len());
                let mut cursor = 0;
                while cursor < chunk.len() {
                    match memchr::memchr(ch, &chunk[cursor..]) {
                        Some(offset) => {
                            let pos = cursor + offset;
                            if pos > cursor {
                                out.extend_from_slice(&chunk[cursor..pos]);
                            }
                            out.push(ch);
                            cursor = pos + 1;
                            while cursor < chunk.len() && chunk[cursor] == ch {
                                cursor += 1;
                            }
                        }
                        None => {
                            out.extend_from_slice(&chunk[cursor..]);
                            break;
                        }
                    }
                }
                out
            })
            .collect();

        // Build IoSlice list, fixing boundary squeezability.
        // Use writev to minimize syscalls.
        let mut slices: Vec<std::io::IoSlice> = Vec::with_capacity(results.len());
        for (idx, result) in results.iter().enumerate() {
            if result.is_empty() {
                continue;
            }
            if idx > 0 {
                if let Some(&prev_last) = results[..idx].iter().rev().find_map(|r| r.last()) {
                    if prev_last == ch {
                        // Skip leading ch bytes in this chunk result
                        let skip = result.iter().take_while(|&&b| b == ch).count();
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

    let buf_size = data.len().min(BUF_SIZE);
    let mut outbuf = vec![0u8; buf_size];
    let len = data.len();
    let mut wp = 0;
    let mut cursor = 0;

    while cursor < len {
        match memchr::memchr(ch, &data[cursor..]) {
            Some(offset) => {
                let pos = cursor + offset;
                let gap = pos - cursor;
                if gap > 0 {
                    if wp + gap > buf_size {
                        writer.write_all(&outbuf[..wp])?;
                        wp = 0;
                    }
                    if gap > buf_size {
                        writer.write_all(&data[cursor..pos])?;
                    } else {
                        outbuf[wp..wp + gap].copy_from_slice(&data[cursor..pos]);
                        wp += gap;
                    }
                }
                if wp >= buf_size {
                    writer.write_all(&outbuf[..wp])?;
                    wp = 0;
                }
                outbuf[wp] = ch;
                wp += 1;
                cursor = pos + 1;
                while cursor < len && data[cursor] == ch {
                    cursor += 1;
                }
            }
            None => {
                let remaining = len - cursor;
                if remaining > 0 {
                    if wp + remaining > buf_size {
                        writer.write_all(&outbuf[..wp])?;
                        wp = 0;
                    }
                    if remaining > buf_size {
                        writer.write_all(&data[cursor..])?;
                    } else {
                        outbuf[wp..wp + remaining].copy_from_slice(&data[cursor..]);
                        wp += remaining;
                    }
                }
                break;
            }
        }
    }

    if wp > 0 {
        writer.write_all(&outbuf[..wp])?;
    }
    Ok(())
}
