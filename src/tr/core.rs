use std::io::{self, Read, Write};

use rayon::prelude::*;

const BUF_SIZE: usize = 8 * 1024 * 1024; // 8MB — mmap chunk processing

/// Stream buffer: 256KB — process data immediately after each read().
/// Stays in L2 cache, matches typical kernel pipe buffer size.
/// Unlike fill_buf (which loops to accumulate 8MB = ~128 syscalls for pipes),
/// we read once and process immediately, matching GNU tr's approach.
const STREAM_BUF: usize = 256 * 1024;

/// Minimum size for parallel translation.
/// AVX2 SIMD translation is so fast (~16 GB/s) that parallelism only helps for very large inputs.
const PARALLEL_TRANSLATE_THRESHOLD: usize = 256 * 1024 * 1024;

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

    // Parallel translation for large inputs — each chunk is independent
    if data.len() >= PARALLEL_TRANSLATE_THRESHOLD {
        let num_threads = rayon::current_num_threads().max(1);
        let chunk_size = (data.len() + num_threads - 1) / num_threads;
        // Align to BUF_SIZE boundaries for cache efficiency
        let chunk_size = ((chunk_size + BUF_SIZE - 1) / BUF_SIZE) * BUF_SIZE;

        // Translate all chunks in parallel
        let translated: Vec<Vec<u8>> = data
            .par_chunks(chunk_size)
            .map(|chunk| {
                let mut out = vec![0u8; chunk.len()];
                translate_chunk_dispatch(chunk, &mut out, &table, &kind, use_simd);
                out
            })
            .collect();

        // Write sequentially to preserve order
        for chunk in &translated {
            writer.write_all(chunk)?;
        }
        return Ok(());
    }

    // Sequential path for smaller data
    let mut out = vec![0u8; BUF_SIZE];
    for chunk in data.chunks(BUF_SIZE) {
        translate_chunk_dispatch(chunk, &mut out[..chunk.len()], &table, &kind, use_simd);
        writer.write_all(&out[..chunk.len()])?;
    }
    Ok(())
}

/// Translate + squeeze from mmap'd byte slice.
pub fn translate_squeeze_mmap(
    set1: &[u8],
    set2: &[u8],
    data: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let table = build_translate_table(set1, set2);
    let squeeze_set = build_member_set(set2);
    let mut outbuf = vec![0u8; BUF_SIZE];
    let mut last_squeezed: u16 = 256;

    for chunk in data.chunks(BUF_SIZE) {
        let mut out_pos = 0;
        for &b in chunk {
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

/// Delete from mmap'd byte slice.
/// Uses SIMD memchr for single-character delete (common case).
pub fn delete_mmap(delete_chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    // Fast path: single character delete uses SIMD memchr
    if delete_chars.len() == 1 {
        return delete_single_char_mmap(delete_chars[0], data, writer);
    }

    let member = build_member_set(delete_chars);
    let mut outbuf = vec![0u8; BUF_SIZE];

    for chunk in data.chunks(BUF_SIZE) {
        let mut out_pos = 0;
        for &b in chunk {
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
pub fn squeeze_mmap(squeeze_chars: &[u8], data: &[u8], writer: &mut impl Write) -> io::Result<()> {
    let member = build_member_set(squeeze_chars);
    let mut outbuf = vec![0u8; BUF_SIZE];
    let mut last_squeezed: u16 = 256;

    for chunk in data.chunks(BUF_SIZE) {
        let mut out_pos = 0;
        for &b in chunk {
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
