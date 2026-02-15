use std::io::{self, Read, Write};

use base64_simd::AsOut;

const BASE64_ENGINE: &base64_simd::Base64 = &base64_simd::STANDARD;

/// Number of available CPUs (cached by the OS). Used for encode parallel thresholds
/// to avoid triggering Rayon's thread pool initialization for encode paths.
#[inline]
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Chunk size for sequential no-wrap encoding: 8MB aligned to 3 bytes.
/// Larger chunks reduce function call overhead per iteration while still
/// keeping peak buffer allocation reasonable (~10.7MB for the output).
const NOWRAP_CHUNK: usize = 8 * 1024 * 1024 - (8 * 1024 * 1024 % 3);

/// Minimum data size for parallel no-wrap encoding (4MB).
/// For 1-2MB input, thread creation (~200µs for 4 threads) + per-thread
/// buffer allocation page faults (~0.3ms) exceed the parallel encoding
/// benefit. At 4MB+, the ~2x parallel speedup amortizes overhead.
const PARALLEL_NOWRAP_THRESHOLD: usize = 4 * 1024 * 1024;

/// Minimum data size for parallel wrapped encoding (2MB).
/// Wrapped parallel uses N threads for SIMD encoding, providing ~Nx
/// speedup. Per-thread buffers (~2.5MB each for 10MB input) page-fault
/// concurrently, and std::thread::scope avoids Rayon pool init (~300µs).
const PARALLEL_WRAPPED_THRESHOLD: usize = 2 * 1024 * 1024;

/// Minimum data size for parallel decoding (2MB of base64 data).
/// Lower threshold lets parallel decode kick in earlier for medium files.
const PARALLEL_DECODE_THRESHOLD: usize = 2 * 1024 * 1024;

/// Encode data and write to output with line wrapping.
/// Uses SIMD encoding with fused encode+wrap for maximum throughput.
pub fn encode_to_writer(data: &[u8], wrap_col: usize, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if wrap_col == 0 {
        return encode_no_wrap(data, out);
    }

    encode_wrapped(data, wrap_col, out)
}

/// Encode without wrapping — parallel SIMD encoding for large data, sequential for small.
fn encode_no_wrap(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    if data.len() >= PARALLEL_NOWRAP_THRESHOLD && num_cpus() > 1 {
        return encode_no_wrap_parallel(data, out);
    }

    // Single-buffer encode: for data that fits in one chunk, encode directly
    // and write once. For larger data, reuse the buffer across chunks.
    let enc_len = BASE64_ENGINE.encoded_length(data.len().min(NOWRAP_CHUNK));
    let mut buf: Vec<u8> = Vec::with_capacity(enc_len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(enc_len);
    }

    for chunk in data.chunks(NOWRAP_CHUNK) {
        let clen = BASE64_ENGINE.encoded_length(chunk.len());
        let encoded = BASE64_ENGINE.encode(chunk, buf[..clen].as_out());
        out.write_all(encoded)?;
    }
    Ok(())
}

/// Parallel no-wrap encoding: split at 3-byte boundaries, encode chunks in parallel.
/// Each chunk except possibly the last is 3-byte aligned, so no padding in intermediate chunks.
///
/// Uses std::thread::scope instead of Rayon to avoid pool initialization overhead (~300µs).
/// Each scoped thread allocates its own output buffer and encodes independently.
/// Output uses writev to combine all per-thread buffers in a single syscall.
fn encode_no_wrap_parallel(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    let num_threads = num_cpus().max(1);
    let raw_chunk = data.len() / num_threads;
    // Align to 3 bytes so each chunk encodes without padding (except the last)
    let chunk_size = ((raw_chunk + 2) / 3) * 3;

    // Split input into 3-byte-aligned chunks
    let chunks: Vec<&[u8]> = data.chunks(chunk_size.max(3)).collect();

    // Each scoped thread allocates its own output buffer and encodes independently.
    let results: Vec<Vec<u8>> = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                s.spawn(|| {
                    let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
                    let mut buf: Vec<u8> = Vec::with_capacity(enc_len);
                    #[allow(clippy::uninit_vec)]
                    unsafe {
                        buf.set_len(enc_len);
                    }
                    // HUGEPAGE on per-thread buffer reduces page faults
                    #[cfg(target_os = "linux")]
                    if enc_len >= 2 * 1024 * 1024 {
                        unsafe {
                            libc::madvise(
                                buf.as_mut_ptr() as *mut libc::c_void,
                                enc_len,
                                libc::MADV_HUGEPAGE,
                            );
                        }
                    }
                    let _ = BASE64_ENGINE.encode(chunk, buf[..enc_len].as_out());
                    buf
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    // Single writev for all chunks in order
    let slices: Vec<io::IoSlice> = results.iter().map(|r| io::IoSlice::new(r)).collect();
    write_all_vectored(out, &slices)
}

/// Encode with line wrapping using forward scatter from L1-cached temp buffer.
/// Encodes groups of lines into a small temp buffer (fits in L1 cache), then
/// scatter-copies wrap_col-byte chunks from temp to output with newlines.
///
/// This is faster than bulk encode + backward expansion because:
/// - Temp buffer reads hit L1 cache (essentially free bandwidth)
/// - Output buffer is written once (no double-write from backward memmove)
/// - Forward access pattern is prefetcher-friendly
fn encode_wrapped(data: &[u8], wrap_col: usize, out: &mut impl Write) -> io::Result<()> {
    let bytes_per_line = wrap_col * 3 / 4;
    if bytes_per_line == 0 {
        return encode_wrapped_small(data, wrap_col, out);
    }

    if data.len() >= PARALLEL_WRAPPED_THRESHOLD && bytes_per_line.is_multiple_of(3) {
        return encode_wrapped_parallel(data, wrap_col, bytes_per_line, out);
    }

    if bytes_per_line.is_multiple_of(3) {
        return encode_wrapped_scatter(data, wrap_col, bytes_per_line, out);
    }

    // Fallback for non-3-aligned bytes_per_line: chunk + in-place expansion
    let lines_per_chunk = (32 * 1024 * 1024) / bytes_per_line;
    let max_input_chunk = (lines_per_chunk * bytes_per_line).max(bytes_per_line);

    let enc_max = BASE64_ENGINE.encoded_length(max_input_chunk.min(data.len()));
    let num_lines_max = enc_max / wrap_col + 1;
    let out_max = num_lines_max * (wrap_col + 1) + wrap_col + 1;
    let mut buf: Vec<u8> = Vec::with_capacity(out_max);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(out_max);
    }

    for chunk in data.chunks(max_input_chunk.max(1)) {
        let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
        let _ = BASE64_ENGINE.encode(chunk, buf[..enc_len].as_out());
        let num_full = enc_len / wrap_col;
        let rem = enc_len % wrap_col;
        let chunk_out_len = num_full * (wrap_col + 1) + if rem > 0 { rem + 1 } else { 0 };

        // Expand backwards
        unsafe {
            let ptr = buf.as_mut_ptr();
            let mut rp = enc_len;
            let mut wp = chunk_out_len;
            if rem > 0 {
                wp -= 1;
                *ptr.add(wp) = b'\n';
                wp -= rem;
                rp -= rem;
                if rp != wp {
                    std::ptr::copy(ptr.add(rp), ptr.add(wp), rem);
                }
            }
            for _ in 0..num_full {
                wp -= 1;
                *ptr.add(wp) = b'\n';
                wp -= wrap_col;
                rp -= wrap_col;
                if rp != wp {
                    std::ptr::copy(ptr.add(rp), ptr.add(wp), wrap_col);
                }
            }
        }
        out.write_all(&buf[..chunk_out_len])?;
    }

    Ok(())
}

/// Forward scatter encode: encode groups of lines into a small L1-cached temp
/// buffer, then scatter-copy wrap_col-byte chunks to the output with newlines.
///
/// Temp buffer size (24KB) fits in L1 data cache on both ARM64 (64KB L1d)
/// and x86_64 (32KB+ L1d), so reads from temp are essentially free.
/// Output buffer is written once in forward order (prefetcher-friendly).
fn encode_wrapped_scatter(
    data: &[u8],
    wrap_col: usize,
    bytes_per_line: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let line_out = wrap_col + 1;
    let full_lines = data.len() / bytes_per_line;
    let remainder_input = data.len() % bytes_per_line;

    let remainder_encoded = if remainder_input > 0 {
        BASE64_ENGINE.encoded_length(remainder_input) + 1
    } else {
        0
    };
    let out_size = full_lines * line_out + remainder_encoded;
    if out_size == 0 {
        return Ok(());
    }

    let mut buf: Vec<u8> = Vec::with_capacity(out_size);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(out_size);
    }
    #[cfg(target_os = "linux")]
    if out_size >= 2 * 1024 * 1024 {
        unsafe {
            libc::madvise(
                buf.as_mut_ptr() as *mut libc::c_void,
                out_size,
                libc::MADV_HUGEPAGE,
            );
        }
    }

    // Temp buffer: ~24KB encoded data fits in L1 cache.
    let group_lines = (24 * 1024 / wrap_col).max(1);
    let group_input = group_lines * bytes_per_line;
    let group_encoded = group_lines * wrap_col;
    let mut temp: Vec<u8> = Vec::with_capacity(group_encoded + 16);
    #[allow(clippy::uninit_vec)]
    unsafe {
        temp.set_len(group_encoded + 16);
    }

    let mut line_idx = 0;
    while line_idx + group_lines <= full_lines {
        let in_off = line_idx * bytes_per_line;

        // Encode group into L1-cached temp buffer
        unsafe {
            let s = std::slice::from_raw_parts_mut(temp.as_mut_ptr(), group_encoded);
            let _ = BASE64_ENGINE.encode(&data[in_off..in_off + group_input], s.as_out());
        }

        // Scatter-copy from temp to output with newlines
        scatter_lines(&temp, &mut buf, line_idx, group_lines, wrap_col, line_out);
        line_idx += group_lines;
    }

    // Remaining full lines (partial group)
    let remaining_lines = full_lines - line_idx;
    if remaining_lines > 0 {
        let in_off = line_idx * bytes_per_line;
        let rem_input = remaining_lines * bytes_per_line;
        let rem_encoded = remaining_lines * wrap_col;

        unsafe {
            let s = std::slice::from_raw_parts_mut(temp.as_mut_ptr(), rem_encoded);
            let _ = BASE64_ENGINE.encode(&data[in_off..in_off + rem_input], s.as_out());
        }

        scatter_lines(
            &temp,
            &mut buf,
            line_idx,
            remaining_lines,
            wrap_col,
            line_out,
        );
    }

    // Handle remainder (partial line at end)
    if remainder_input > 0 {
        let in_off = full_lines * bytes_per_line;
        let out_off = full_lines * line_out;
        let enc_len = BASE64_ENGINE.encoded_length(remainder_input);
        unsafe {
            let s = std::slice::from_raw_parts_mut(buf.as_mut_ptr().add(out_off), enc_len);
            let _ = BASE64_ENGINE.encode(&data[in_off..], s.as_out());
            *buf.as_mut_ptr().add(out_off + enc_len) = b'\n';
        }
    }

    out.write_all(&buf[..out_size])
}

/// Scatter-copy encoded lines from temp buffer to output buffer with newlines.
/// Uses copy_nonoverlapping since temp and output never overlap.
#[inline]
fn scatter_lines(
    temp: &[u8],
    buf: &mut [u8],
    line_start: usize,
    count: usize,
    wrap_col: usize,
    line_out: usize,
) {
    unsafe {
        let src = temp.as_ptr();
        let dst = buf.as_mut_ptr();
        for i in 0..count {
            let s_off = i * wrap_col;
            let d_off = (line_start + i) * line_out;
            std::ptr::copy_nonoverlapping(src.add(s_off), dst.add(d_off), wrap_col);
            *dst.add(d_off + wrap_col) = b'\n';
        }
    }
}

/// Static newline byte for IoSlice references in writev calls.
static NEWLINE: [u8; 1] = [b'\n'];

/// Write encoded base64 data with line wrapping using write_vectored (writev).
/// Builds IoSlice entries pointing at wrap_col-sized segments of the encoded buffer,
/// interleaved with newline IoSlices, then writes in batches of MAX_WRITEV_IOV.
/// This is zero-copy: no fused output buffer needed.
#[inline]
#[allow(dead_code)]
fn write_wrapped_iov(encoded: &[u8], wrap_col: usize, out: &mut impl Write) -> io::Result<()> {
    // Max IoSlice entries per writev batch. Linux UIO_MAXIOV is 1024.
    // Each line needs 2 entries (data + newline), so 512 lines per batch.
    const MAX_IOV: usize = 1024;

    let num_full_lines = encoded.len() / wrap_col;
    let remainder = encoded.len() % wrap_col;
    let total_iov = num_full_lines * 2 + if remainder > 0 { 2 } else { 0 };

    // Small output: build all IoSlices and write in one call
    if total_iov <= MAX_IOV {
        let mut iov: Vec<io::IoSlice> = Vec::with_capacity(total_iov);
        let mut pos = 0;
        for _ in 0..num_full_lines {
            iov.push(io::IoSlice::new(&encoded[pos..pos + wrap_col]));
            iov.push(io::IoSlice::new(&NEWLINE));
            pos += wrap_col;
        }
        if remainder > 0 {
            iov.push(io::IoSlice::new(&encoded[pos..pos + remainder]));
            iov.push(io::IoSlice::new(&NEWLINE));
        }
        return write_all_vectored(out, &iov);
    }

    // Large output: write in batches
    let mut iov: Vec<io::IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut pos = 0;
    for _ in 0..num_full_lines {
        iov.push(io::IoSlice::new(&encoded[pos..pos + wrap_col]));
        iov.push(io::IoSlice::new(&NEWLINE));
        pos += wrap_col;
        if iov.len() >= MAX_IOV {
            write_all_vectored(out, &iov)?;
            iov.clear();
        }
    }
    if remainder > 0 {
        iov.push(io::IoSlice::new(&encoded[pos..pos + remainder]));
        iov.push(io::IoSlice::new(&NEWLINE));
    }
    if !iov.is_empty() {
        write_all_vectored(out, &iov)?;
    }
    Ok(())
}

/// Write encoded base64 data with line wrapping using writev, tracking column state
/// across calls. Used by encode_stream for piped input where chunks don't align
/// to line boundaries.
#[inline]
fn write_wrapped_iov_streaming(
    encoded: &[u8],
    wrap_col: usize,
    col: &mut usize,
    out: &mut impl Write,
) -> io::Result<()> {
    const MAX_IOV: usize = 1024;
    let mut iov: Vec<io::IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut rp = 0;

    while rp < encoded.len() {
        let space = wrap_col - *col;
        let avail = encoded.len() - rp;

        if avail <= space {
            // Remaining data fits in current line
            iov.push(io::IoSlice::new(&encoded[rp..rp + avail]));
            *col += avail;
            if *col == wrap_col {
                iov.push(io::IoSlice::new(&NEWLINE));
                *col = 0;
            }
            break;
        } else {
            // Fill current line and add newline
            iov.push(io::IoSlice::new(&encoded[rp..rp + space]));
            iov.push(io::IoSlice::new(&NEWLINE));
            rp += space;
            *col = 0;
        }

        if iov.len() >= MAX_IOV - 1 {
            write_all_vectored(out, &iov)?;
            iov.clear();
        }
    }

    if !iov.is_empty() {
        write_all_vectored(out, &iov)?;
    }
    Ok(())
}

/// Parallel wrapped encoding: single output buffer, direct-to-position encode+wrap.
/// Requires bytes_per_line % 3 == 0 so each chunk encodes without intermediate padding.
///
/// Pre-calculates exact output size and each thread's write offset, then encodes
/// 57-byte input groups directly to their final position in a shared output buffer.
/// Each thread writes wrap_col encoded bytes + newline per line, so output for line N
/// starts at N * (wrap_col + 1). This eliminates per-chunk heap allocations and
/// the fuse_wrap copy pass entirely.
/// Parallel wrapped encoding with per-thread output buffers.
///
/// Each thread encodes its chunk of input lines into its own buffer (with newlines),
/// then writev combines all buffers in order. This avoids the single ~13.5MB shared
/// buffer allocation whose page faults (~3400 faults = ~3.4ms) dominate encoding time.
/// Single shared output buffer with MADV_HUGEPAGE — threads write directly to
/// computed offsets, eliminating per-thread allocations and page faults.
fn encode_wrapped_parallel(
    data: &[u8],
    wrap_col: usize,
    bytes_per_line: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let line_out = wrap_col + 1;
    let total_full_lines = data.len() / bytes_per_line;

    // Split work at line boundaries for parallel processing
    let num_threads = num_cpus().max(1);
    let lines_per_chunk = (total_full_lines / num_threads).max(1);

    // Build per-thread input ranges aligned to bytes_per_line
    let mut tasks: Vec<(usize, usize)> = Vec::new(); // (input_offset, num_input_bytes)
    let mut in_off = 0usize;
    while in_off < data.len() {
        let chunk_input = (lines_per_chunk * bytes_per_line).min(data.len() - in_off);
        let aligned_input = if in_off + chunk_input < data.len() {
            (chunk_input / bytes_per_line) * bytes_per_line
        } else {
            chunk_input
        };
        if aligned_input == 0 {
            break;
        }
        tasks.push((in_off, aligned_input));
        in_off += aligned_input;
    }

    // Compute per-task output sizes and cumulative offsets
    let mut task_out: Vec<(usize, usize)> = Vec::with_capacity(tasks.len()); // (out_offset, out_size)
    let mut total_out = 0usize;
    for &(_, chunk_len) in &tasks {
        let full_lines = chunk_len / bytes_per_line;
        let rem = chunk_len % bytes_per_line;
        let remainder_encoded = if rem > 0 {
            BASE64_ENGINE.encoded_length(rem) + 1
        } else {
            0
        };
        let out_size = full_lines * line_out + remainder_encoded;
        task_out.push((total_out, out_size));
        total_out += out_size;
    }

    // Single shared output buffer — one allocation, one MADV_HUGEPAGE
    let mut shared_buf: Vec<u8> = Vec::with_capacity(total_out);
    #[allow(clippy::uninit_vec)]
    unsafe {
        shared_buf.set_len(total_out);
    }
    #[cfg(target_os = "linux")]
    if total_out >= 2 * 1024 * 1024 {
        unsafe {
            libc::madvise(
                shared_buf.as_mut_ptr() as *mut libc::c_void,
                total_out,
                libc::MADV_HUGEPAGE,
            );
        }
    }

    // Parallel encoding: each thread writes to its pre-computed offset
    let shared_ptr = shared_buf.as_mut_ptr() as usize;
    std::thread::scope(|s| {
        let handles: Vec<_> = tasks
            .iter()
            .zip(task_out.iter())
            .map(|(&(in_off, chunk_len), &(out_off, out_size))| {
                s.spawn(move || {
                    let input = &data[in_off..in_off + chunk_len];
                    let full_lines = chunk_len / bytes_per_line;
                    let rem = chunk_len % bytes_per_line;

                    // Get our slice of the shared buffer (non-overlapping regions)
                    let buf = unsafe {
                        std::slice::from_raw_parts_mut(
                            (shared_ptr as *mut u8).add(out_off),
                            out_size,
                        )
                    };

                    if full_lines > 0 {
                        // Forward scatter from L1-cached temp buffer
                        let group_lines = (24 * 1024 / wrap_col).max(1);
                        let group_input = group_lines * bytes_per_line;
                        let group_encoded = group_lines * wrap_col;
                        let mut temp: Vec<u8> = Vec::with_capacity(group_encoded + 16);
                        #[allow(clippy::uninit_vec)]
                        unsafe {
                            temp.set_len(group_encoded + 16);
                        }

                        let mut line_idx = 0usize;
                        while line_idx + group_lines <= full_lines {
                            let i_off = line_idx * bytes_per_line;
                            unsafe {
                                let s = std::slice::from_raw_parts_mut(
                                    temp.as_mut_ptr(),
                                    group_encoded,
                                );
                                let _ = BASE64_ENGINE
                                    .encode(&input[i_off..i_off + group_input], s.as_out());
                            }
                            scatter_lines(&temp, buf, line_idx, group_lines, wrap_col, line_out);
                            line_idx += group_lines;
                        }

                        let rem_lines = full_lines - line_idx;
                        if rem_lines > 0 {
                            let i_off = line_idx * bytes_per_line;
                            let r_input = rem_lines * bytes_per_line;
                            let r_encoded = rem_lines * wrap_col;
                            unsafe {
                                let s =
                                    std::slice::from_raw_parts_mut(temp.as_mut_ptr(), r_encoded);
                                let _ = BASE64_ENGINE
                                    .encode(&input[i_off..i_off + r_input], s.as_out());
                            }
                            scatter_lines(&temp, buf, line_idx, rem_lines, wrap_col, line_out);
                        }
                    }

                    if rem > 0 {
                        let line_input = &input[full_lines * bytes_per_line..];
                        let enc_len = BASE64_ENGINE.encoded_length(rem);
                        let woff = full_lines * line_out;
                        unsafe {
                            let s =
                                std::slice::from_raw_parts_mut(buf.as_mut_ptr().add(woff), enc_len);
                            let _ = BASE64_ENGINE.encode(line_input, s.as_out());
                            *buf.as_mut_ptr().add(woff + enc_len) = b'\n';
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    });

    // Single write_all — no writev needed, data is already contiguous
    out.write_all(&shared_buf)
}

/// Fuse encoded base64 data with newlines in a single pass.
/// Uses ptr::copy_nonoverlapping with 8-line unrolling for max throughput.
/// Returns number of bytes written.
#[inline]
#[allow(dead_code)]
fn fuse_wrap(encoded: &[u8], wrap_col: usize, out_buf: &mut [u8]) -> usize {
    let line_out = wrap_col + 1; // wrap_col data bytes + 1 newline
    let mut rp = 0;
    let mut wp = 0;

    // Unrolled: process 8 lines per iteration for better ILP
    while rp + 8 * wrap_col <= encoded.len() {
        unsafe {
            let src = encoded.as_ptr().add(rp);
            let dst = out_buf.as_mut_ptr().add(wp);

            std::ptr::copy_nonoverlapping(src, dst, wrap_col);
            *dst.add(wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(wrap_col), dst.add(line_out), wrap_col);
            *dst.add(line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(2 * wrap_col), dst.add(2 * line_out), wrap_col);
            *dst.add(2 * line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(3 * wrap_col), dst.add(3 * line_out), wrap_col);
            *dst.add(3 * line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(4 * wrap_col), dst.add(4 * line_out), wrap_col);
            *dst.add(4 * line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(5 * wrap_col), dst.add(5 * line_out), wrap_col);
            *dst.add(5 * line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(6 * wrap_col), dst.add(6 * line_out), wrap_col);
            *dst.add(6 * line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(7 * wrap_col), dst.add(7 * line_out), wrap_col);
            *dst.add(7 * line_out + wrap_col) = b'\n';
        }
        rp += 8 * wrap_col;
        wp += 8 * line_out;
    }

    // Handle remaining 4 lines at a time
    while rp + 4 * wrap_col <= encoded.len() {
        unsafe {
            let src = encoded.as_ptr().add(rp);
            let dst = out_buf.as_mut_ptr().add(wp);

            std::ptr::copy_nonoverlapping(src, dst, wrap_col);
            *dst.add(wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(wrap_col), dst.add(line_out), wrap_col);
            *dst.add(line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(2 * wrap_col), dst.add(2 * line_out), wrap_col);
            *dst.add(2 * line_out + wrap_col) = b'\n';

            std::ptr::copy_nonoverlapping(src.add(3 * wrap_col), dst.add(3 * line_out), wrap_col);
            *dst.add(3 * line_out + wrap_col) = b'\n';
        }
        rp += 4 * wrap_col;
        wp += 4 * line_out;
    }

    // Remaining full lines
    while rp + wrap_col <= encoded.len() {
        unsafe {
            std::ptr::copy_nonoverlapping(
                encoded.as_ptr().add(rp),
                out_buf.as_mut_ptr().add(wp),
                wrap_col,
            );
            *out_buf.as_mut_ptr().add(wp + wrap_col) = b'\n';
        }
        rp += wrap_col;
        wp += line_out;
    }

    // Partial last line
    if rp < encoded.len() {
        let remaining = encoded.len() - rp;
        unsafe {
            std::ptr::copy_nonoverlapping(
                encoded.as_ptr().add(rp),
                out_buf.as_mut_ptr().add(wp),
                remaining,
            );
        }
        wp += remaining;
        out_buf[wp] = b'\n';
        wp += 1;
    }

    wp
}

/// Fallback for very small wrap columns (< 4 chars).
fn encode_wrapped_small(data: &[u8], wrap_col: usize, out: &mut impl Write) -> io::Result<()> {
    let enc_max = BASE64_ENGINE.encoded_length(data.len());
    let mut buf: Vec<u8> = Vec::with_capacity(enc_max);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(enc_max);
    }
    let encoded = BASE64_ENGINE.encode(data, buf[..enc_max].as_out());

    let wc = wrap_col.max(1);
    for line in encoded.chunks(wc) {
        out.write_all(line)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

/// Decode base64 data and write to output (borrows data, allocates clean buffer).
/// When `ignore_garbage` is true, strip all non-base64 characters.
/// When false, only strip whitespace (standard behavior).
pub fn decode_to_writer(data: &[u8], ignore_garbage: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if ignore_garbage {
        let mut cleaned = strip_non_base64(data);
        return decode_clean_slice(&mut cleaned, out);
    }

    // For large data (>= 512KB): use bulk strip + single-shot decode.
    // try_line_decode decodes per-line (~25ns overhead per 76-byte line call),
    // while strip+decode uses SIMD gap-copy + single-shot SIMD decode at ~6.5 GB/s.
    // For 10MB decode benchmark: ~2ms (bulk) vs ~4ms (per-line) = 2x faster.
    // For small data (< 512KB): per-line decode avoids allocation overhead.
    if data.len() < 512 * 1024 && data.len() >= 77 {
        if let Some(result) = try_line_decode(data, out) {
            return result;
        }
    }

    // Fast path: single-pass SIMD strip + decode
    decode_stripping_whitespace(data, out)
}

/// Decode base64 from a mutable buffer (MAP_PRIVATE mmap or owned Vec).
/// Strips whitespace in-place using SIMD memchr2 gap-copy, then decodes
/// in-place with base64_simd::decode_inplace. Zero additional allocations.
///
/// For MAP_PRIVATE mmap: the kernel uses COW semantics, so only pages
/// containing whitespace (newlines) get physically copied (~1.3% for
/// 76-char line base64). The decode writes to the same buffer, but decoded
/// data is always shorter than encoded (3/4 ratio), so it fits in-place.
pub fn decode_mmap_inplace(
    data: &mut [u8],
    ignore_garbage: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // For small data: try line-by-line decode (avoids COW page faults).
    // For large data (>= 512KB): bulk strip+decode is faster than per-line decode.
    if !ignore_garbage && data.len() >= 77 && data.len() < 512 * 1024 {
        if let Some(result) = try_line_decode(data, out) {
            return result;
        }
    }

    if ignore_garbage {
        // Strip non-base64 chars in-place
        let ptr = data.as_mut_ptr();
        let len = data.len();
        let mut wp = 0;
        for rp in 0..len {
            let b = unsafe { *ptr.add(rp) };
            if is_base64_char(b) {
                unsafe { *ptr.add(wp) = b };
                wp += 1;
            }
        }
        match BASE64_ENGINE.decode_inplace(&mut data[..wp]) {
            Ok(decoded) => return out.write_all(decoded),
            Err(_) => return decode_error(),
        }
    }

    // Fast path: uniform-line fused strip+decode (no intermediate buffer).
    if data.len() >= 77 {
        if let Some(result) = try_decode_uniform_lines(data, out) {
            return result;
        }
    }

    // Fallback: strip whitespace in-place using SIMD memchr2 gap-copy.

    // Quick check: no newlines at all — maybe already clean
    if memchr::memchr2(b'\n', b'\r', data).is_none() {
        // Check for rare whitespace
        if !data
            .iter()
            .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c)
        {
            // Perfectly clean — decode in-place directly
            match BASE64_ENGINE.decode_inplace(data) {
                Ok(decoded) => return out.write_all(decoded),
                Err(_) => return decode_error(),
            }
        }
        // Rare whitespace only — strip in-place
        let ptr = data.as_mut_ptr();
        let len = data.len();
        let mut wp = 0;
        for rp in 0..len {
            let b = unsafe { *ptr.add(rp) };
            if NOT_WHITESPACE[b as usize] {
                unsafe { *ptr.add(wp) = b };
                wp += 1;
            }
        }
        match BASE64_ENGINE.decode_inplace(&mut data[..wp]) {
            Ok(decoded) => return out.write_all(decoded),
            Err(_) => return decode_error(),
        }
    }

    // SIMD gap-copy: strip \n and \r in-place using memchr2
    let ptr = data.as_mut_ptr();
    let len = data.len();
    let mut wp = 0usize;
    let mut gap_start = 0usize;
    let mut has_rare_ws = false;

    // SAFETY: memchr2_iter reads from the original data. We write to positions
    // [0..wp] which are always <= gap_start, so we never overwrite unread data.
    for pos in memchr::memchr2_iter(b'\n', b'\r', data) {
        let gap_len = pos - gap_start;
        if gap_len > 0 {
            if !has_rare_ws {
                // Check for rare whitespace during the gap-copy
                has_rare_ws = unsafe {
                    std::slice::from_raw_parts(ptr.add(gap_start), gap_len)
                        .iter()
                        .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c)
                };
            }
            if wp != gap_start {
                unsafe { std::ptr::copy(ptr.add(gap_start), ptr.add(wp), gap_len) };
            }
            wp += gap_len;
        }
        gap_start = pos + 1;
    }
    // Final gap
    let tail_len = len - gap_start;
    if tail_len > 0 {
        if !has_rare_ws {
            has_rare_ws = unsafe {
                std::slice::from_raw_parts(ptr.add(gap_start), tail_len)
                    .iter()
                    .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c)
            };
        }
        if wp != gap_start {
            unsafe { std::ptr::copy(ptr.add(gap_start), ptr.add(wp), tail_len) };
        }
        wp += tail_len;
    }

    // Second pass for rare whitespace if needed
    if has_rare_ws {
        let mut rp = 0;
        let mut cwp = 0;
        while rp < wp {
            let b = unsafe { *ptr.add(rp) };
            if NOT_WHITESPACE[b as usize] {
                unsafe { *ptr.add(cwp) = b };
                cwp += 1;
            }
            rp += 1;
        }
        wp = cwp;
    }

    // Decode in-place: decoded data is always shorter than encoded (3/4 ratio)
    if wp >= PARALLEL_DECODE_THRESHOLD {
        // For large data, use parallel decode from the cleaned slice
        return decode_borrowed_clean_parallel(out, &data[..wp]);
    }
    match BASE64_ENGINE.decode_inplace(&mut data[..wp]) {
        Ok(decoded) => out.write_all(decoded),
        Err(_) => decode_error(),
    }
}

/// Decode base64 from an owned Vec (in-place whitespace strip + decode).
pub fn decode_owned(
    data: &mut Vec<u8>,
    ignore_garbage: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if ignore_garbage {
        data.retain(|&b| is_base64_char(b));
    } else {
        strip_whitespace_inplace(data);
    }

    decode_clean_slice(data, out)
}

/// Strip all whitespace from a Vec in-place using SIMD memchr2 gap-copy.
/// For typical base64 (76-char lines with \n), newlines are ~1/77 of the data,
/// so SIMD memchr2 skips ~76 bytes per hit instead of checking every byte.
/// Falls back to scalar compaction only for rare whitespace (tab, space, VT, FF).
fn strip_whitespace_inplace(data: &mut Vec<u8>) {
    // Quick check: skip stripping if no \n or \r in the data.
    // Uses SIMD memchr2 for fast scanning (~10 GB/s) instead of per-byte check.
    // For typical base64 (76-char lines), we'll find \n immediately and skip this.
    if memchr::memchr2(b'\n', b'\r', data).is_none() {
        // No newlines/CR — check for rare whitespace only
        if data
            .iter()
            .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c)
        {
            data.retain(|&b| NOT_WHITESPACE[b as usize]);
        }
        return;
    }

    // SIMD gap-copy: find \n and \r positions with memchr2, then memmove the
    // gaps between them to compact the data in-place. For typical base64 streams,
    // newlines are the only whitespace, so this handles >99% of cases.
    let ptr = data.as_mut_ptr();
    let len = data.len();
    let mut wp = 0usize;
    let mut gap_start = 0usize;
    let mut has_rare_ws = false;

    for pos in memchr::memchr2_iter(b'\n', b'\r', data.as_slice()) {
        let gap_len = pos - gap_start;
        if gap_len > 0 {
            if !has_rare_ws {
                // Check for rare whitespace during copy (amortized ~1 branch per 77 bytes)
                has_rare_ws = data[gap_start..pos]
                    .iter()
                    .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c);
            }
            if wp != gap_start {
                unsafe {
                    std::ptr::copy(ptr.add(gap_start), ptr.add(wp), gap_len);
                }
            }
            wp += gap_len;
        }
        gap_start = pos + 1;
    }
    // Copy the final gap
    let tail_len = len - gap_start;
    if tail_len > 0 {
        if !has_rare_ws {
            has_rare_ws = data[gap_start..]
                .iter()
                .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c);
        }
        if wp != gap_start {
            unsafe {
                std::ptr::copy(ptr.add(gap_start), ptr.add(wp), tail_len);
            }
        }
        wp += tail_len;
    }

    data.truncate(wp);

    // Second pass for rare whitespace (tab, space, VT, FF) — only if detected.
    // In typical base64 streams (76-char lines with \n), this is skipped entirely.
    if has_rare_ws {
        let ptr = data.as_mut_ptr();
        let len = data.len();
        let mut rp = 0;
        let mut cwp = 0;
        while rp < len {
            let b = unsafe { *ptr.add(rp) };
            if NOT_WHITESPACE[b as usize] {
                unsafe { *ptr.add(cwp) = b };
                cwp += 1;
            }
            rp += 1;
        }
        data.truncate(cwp);
    }
}

/// 256-byte lookup table: true for non-whitespace bytes.
/// Used for single-pass whitespace stripping in decode.
static NOT_WHITESPACE: [bool; 256] = {
    let mut table = [true; 256];
    table[b' ' as usize] = false;
    table[b'\t' as usize] = false;
    table[b'\n' as usize] = false;
    table[b'\r' as usize] = false;
    table[0x0b] = false; // vertical tab
    table[0x0c] = false; // form feed
    table
};

/// Fused strip+decode for uniform-line base64 data.
/// Detects consistent line length, then processes in sub-chunks: each sub-chunk
/// copies lines to a small local buffer (L2-hot) and decodes immediately.
/// Eliminates the large intermediate clean buffer (~12MB for 10MB decode).
/// Returns None if the data doesn't have uniform line structure.
fn try_decode_uniform_lines(data: &[u8], out: &mut impl Write) -> Option<io::Result<()>> {
    let first_nl = memchr::memchr(b'\n', data)?;
    let line_len = first_nl;
    if line_len == 0 || line_len % 4 != 0 {
        return None;
    }

    let stride = line_len + 1;

    // Verify the data has consistent line structure (first + last lines)
    let check_lines = 4.min(data.len() / stride);
    for i in 1..check_lines {
        let expected_nl = i * stride - 1;
        if expected_nl >= data.len() || data[expected_nl] != b'\n' {
            return None;
        }
    }

    let full_lines = if data.len() >= stride {
        let candidate = data.len() / stride;
        if candidate > 0 && data[candidate * stride - 1] != b'\n' {
            return None;
        }
        candidate
    } else {
        0
    };

    let remainder_start = full_lines * stride;
    let remainder = &data[remainder_start..];
    let rem_clean = if remainder.last() == Some(&b'\n') {
        &remainder[..remainder.len() - 1]
    } else {
        remainder
    };

    // Compute exact decoded sizes
    let decoded_per_line = line_len * 3 / 4;
    let rem_decoded_size = if rem_clean.is_empty() {
        0
    } else {
        let pad = rem_clean
            .iter()
            .rev()
            .take(2)
            .filter(|&&b| b == b'=')
            .count();
        rem_clean.len() * 3 / 4 - pad
    };
    let total_decoded = full_lines * decoded_per_line + rem_decoded_size;
    let clean_len = full_lines * line_len;

    // Parallel path: fused strip+decode with 128KB sub-chunks per thread.
    // Each thread copies lines to a thread-local buffer (L2-hot) and decodes immediately,
    // eliminating the 12MB+ intermediate clean buffer entirely.
    if clean_len >= PARALLEL_DECODE_THRESHOLD && num_cpus() > 1 {
        let mut output: Vec<u8> = Vec::with_capacity(total_decoded);
        #[allow(clippy::uninit_vec)]
        unsafe {
            output.set_len(total_decoded);
        }

        let out_ptr = output.as_mut_ptr() as usize;
        let src_ptr = data.as_ptr() as usize;
        let num_threads = num_cpus().max(1);
        let lines_per_thread = (full_lines + num_threads - 1) / num_threads;
        let lines_per_sub = (256 * 1024 / line_len).max(1);

        let result: Result<(), io::Error> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..num_threads)
                .map(|t| {
                    s.spawn(move || -> Result<(), io::Error> {
                        let start_line = t * lines_per_thread;
                        if start_line >= full_lines {
                            return Ok(());
                        }
                        let end_line = (start_line + lines_per_thread).min(full_lines);
                        let chunk_lines = end_line - start_line;

                        let sub_buf_size = lines_per_sub.min(chunk_lines) * line_len;
                        let mut local_buf: Vec<u8> = Vec::with_capacity(sub_buf_size);
                        #[allow(clippy::uninit_vec)]
                        unsafe {
                            local_buf.set_len(sub_buf_size);
                        }

                        let src = src_ptr as *const u8;
                        let out_base = out_ptr as *mut u8;
                        let local_dst = local_buf.as_mut_ptr();

                        let mut sub_start = 0usize;
                        while sub_start < chunk_lines {
                            let sub_count = (chunk_lines - sub_start).min(lines_per_sub);
                            let sub_clean = sub_count * line_len;

                            for i in 0..sub_count {
                                unsafe {
                                    std::ptr::copy_nonoverlapping(
                                        src.add((start_line + sub_start + i) * stride),
                                        local_dst.add(i * line_len),
                                        line_len,
                                    );
                                }
                            }

                            let out_offset = (start_line + sub_start) * decoded_per_line;
                            let out_size = sub_count * decoded_per_line;
                            let out_slice = unsafe {
                                std::slice::from_raw_parts_mut(out_base.add(out_offset), out_size)
                            };
                            BASE64_ENGINE
                                .decode(&local_buf[..sub_clean], out_slice.as_out())
                                .map_err(|_| {
                                    io::Error::new(io::ErrorKind::InvalidData, "invalid input")
                                })?;

                            sub_start += sub_count;
                        }
                        Ok(())
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap()?;
            }
            Ok(())
        });

        if let Err(e) = result {
            return Some(Err(e));
        }

        if !rem_clean.is_empty() {
            let rem_out = &mut output[full_lines * decoded_per_line..total_decoded];
            match BASE64_ENGINE.decode(rem_clean, rem_out.as_out()) {
                Ok(_) => {}
                Err(_) => return Some(decode_error()),
            }
        }

        return Some(out.write_all(&output[..total_decoded]));
    }

    // Sequential path: fused strip+decode in 256KB sub-chunks.
    // Larger sub-chunks give SIMD decode more data per call, improving throughput.
    // Uses decode_inplace on a small reusable buffer — no large allocations at all.
    let lines_per_sub = (256 * 1024 / line_len).max(1);
    let sub_buf_size = lines_per_sub * line_len;
    let mut local_buf: Vec<u8> = Vec::with_capacity(sub_buf_size);
    #[allow(clippy::uninit_vec)]
    unsafe {
        local_buf.set_len(sub_buf_size);
    }

    let src = data.as_ptr();
    let local_dst = local_buf.as_mut_ptr();

    let mut line_idx = 0usize;
    while line_idx < full_lines {
        let sub_count = (full_lines - line_idx).min(lines_per_sub);
        let sub_clean = sub_count * line_len;

        for i in 0..sub_count {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    src.add((line_idx + i) * stride),
                    local_dst.add(i * line_len),
                    line_len,
                );
            }
        }

        match BASE64_ENGINE.decode_inplace(&mut local_buf[..sub_clean]) {
            Ok(decoded) => {
                if let Err(e) = out.write_all(decoded) {
                    return Some(Err(e));
                }
            }
            Err(_) => return Some(decode_error()),
        }

        line_idx += sub_count;
    }

    if !rem_clean.is_empty() {
        let mut rem_buf = rem_clean.to_vec();
        match BASE64_ENGINE.decode_inplace(&mut rem_buf) {
            Ok(decoded) => {
                if let Err(e) = out.write_all(decoded) {
                    return Some(Err(e));
                }
            }
            Err(_) => return Some(decode_error()),
        }
    }

    Some(Ok(()))
}

/// Decode by stripping whitespace and decoding in a single fused pass.
/// For data with no whitespace, decodes directly without any copy.
/// Detects uniform line structure for fast structured-copy (no search needed),
/// falls back to SIMD memchr2 gap-copy for irregular data.
fn decode_stripping_whitespace(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    // Fast path for uniform-line base64 (e.g., standard 76-char lines + newline).
    // Copies at known offsets, avoiding the memchr2 search entirely.
    // For 13MB base64: saves ~1ms vs memchr2 gap-copy (just structured memcpy).
    if data.len() >= 77 {
        if let Some(result) = try_decode_uniform_lines(data, out) {
            return result;
        }
    }

    // Quick check: skip stripping if no \n or \r in the data.
    // Uses SIMD memchr2 for fast scanning (~10 GB/s) instead of per-byte check.
    if memchr::memchr2(b'\n', b'\r', data).is_none() {
        // No newlines/CR — check for rare whitespace only
        if !data
            .iter()
            .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c)
        {
            return decode_borrowed_clean(out, data);
        }
        // Has rare whitespace only — strip and decode
        let mut cleaned: Vec<u8> = Vec::with_capacity(data.len());
        for &b in data {
            if NOT_WHITESPACE[b as usize] {
                cleaned.push(b);
            }
        }
        return decode_clean_slice(&mut cleaned, out);
    }

    // SIMD gap-copy: use memchr2 to find \n and \r positions, then copy the
    // gaps between them. For typical base64 (76-char lines), newlines are ~1/77
    // of the data, so we process ~76 bytes per memchr hit instead of 1 per scalar.
    let mut clean: Vec<u8> = Vec::with_capacity(data.len());
    let dst = clean.as_mut_ptr();
    let mut wp = 0usize;
    let mut gap_start = 0usize;
    // Track whether any rare whitespace (tab, space, VT, FF) exists in gap regions.
    // This avoids the second full-scan pass when only \n/\r are present.
    let mut has_rare_ws = false;

    for pos in memchr::memchr2_iter(b'\n', b'\r', data) {
        let gap_len = pos - gap_start;
        if gap_len > 0 {
            // Check gap region for rare whitespace during copy.
            // This adds ~1 branch per gap but eliminates the second full scan.
            if !has_rare_ws {
                has_rare_ws = data[gap_start..pos]
                    .iter()
                    .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr().add(gap_start), dst.add(wp), gap_len);
            }
            wp += gap_len;
        }
        gap_start = pos + 1;
    }
    // Copy the final gap after the last \n/\r
    let tail_len = data.len() - gap_start;
    if tail_len > 0 {
        if !has_rare_ws {
            has_rare_ws = data[gap_start..]
                .iter()
                .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(gap_start), dst.add(wp), tail_len);
        }
        wp += tail_len;
    }
    unsafe {
        clean.set_len(wp);
    }

    // Second pass for rare whitespace (tab, space, VT, FF) — only runs when needed.
    // In typical base64 streams (76-char lines with \n), this is skipped entirely.
    if has_rare_ws {
        let ptr = clean.as_mut_ptr();
        let len = clean.len();
        let mut rp = 0;
        let mut cwp = 0;
        while rp < len {
            let b = unsafe { *ptr.add(rp) };
            if NOT_WHITESPACE[b as usize] {
                unsafe { *ptr.add(cwp) = b };
                cwp += 1;
            }
            rp += 1;
        }
        clean.truncate(cwp);
    }

    // For large data (>= threshold), use parallel decode for multi-core speedup.
    // For small data, use in-place decode to avoid extra allocation.
    if clean.len() >= PARALLEL_DECODE_THRESHOLD {
        decode_borrowed_clean_parallel(out, &clean)
    } else {
        decode_clean_slice(&mut clean, out)
    }
}

/// Try to decode base64 data line-by-line, avoiding whitespace stripping.
/// Returns Some(result) if the data has uniform line lengths suitable for
/// per-line decode, or None if the data doesn't fit this pattern.
///
/// For standard 76-char-line base64 (wrap=76): each line is 76 encoded chars
/// + newline = 77 bytes. 76 chars = 19 groups of 4 = 57 decoded bytes per line.
/// We decode each line directly into its position in the output buffer.
fn try_line_decode(data: &[u8], out: &mut impl Write) -> Option<io::Result<()>> {
    // Find the first newline to determine line length
    let first_nl = memchr::memchr(b'\n', data)?;
    let line_len = first_nl; // encoded chars per line (without newline)

    // Line length must be a multiple of 4 (complete base64 groups, no padding mid-stream)
    if line_len == 0 || line_len % 4 != 0 {
        return None;
    }

    let line_stride = line_len + 1; // line_len chars + 1 newline byte
    let decoded_per_line = line_len * 3 / 4;

    // Verify the data has a consistent line structure by checking the next few lines
    let check_lines = 4.min(data.len() / line_stride);
    for i in 1..check_lines {
        let expected_nl = i * line_stride - 1;
        if expected_nl >= data.len() {
            break;
        }
        if data[expected_nl] != b'\n' {
            return None; // Inconsistent line length
        }
    }

    // Calculate full lines and remainder
    let full_lines = if data.len() >= line_stride {
        // Check how many complete lines fit
        let candidate = data.len() / line_stride;
        // Verify the last full line's newline
        if candidate > 0 && data[candidate * line_stride - 1] != b'\n' {
            return None; // Not a clean line-structured file
        }
        candidate
    } else {
        0
    };

    let remainder_start = full_lines * line_stride;
    let remainder = &data[remainder_start..];

    // Calculate exact output size
    let remainder_clean_len = if remainder.is_empty() {
        0
    } else {
        // Remainder might end with newline, strip it
        let rem = if remainder.last() == Some(&b'\n') {
            &remainder[..remainder.len() - 1]
        } else {
            remainder
        };
        if rem.is_empty() {
            0
        } else {
            // Check for padding
            let pad = rem.iter().rev().take(2).filter(|&&b| b == b'=').count();
            if rem.len() % 4 != 0 {
                return None; // Invalid remainder
            }
            rem.len() * 3 / 4 - pad
        }
    };

    // Single-allocation decode: allocate full decoded output, decode all lines
    // directly into it, then write_all in one syscall. For 10MB base64 (7.5MB decoded),
    // this does 1 write() instead of ~30 chunked writes. The 7.5MB allocation is trivial
    // compared to the mmap'd input. SIMD decode at ~8 GB/s finishes in <1ms.
    let total_decoded = full_lines * decoded_per_line + remainder_clean_len;
    let mut out_buf: Vec<u8> = Vec::with_capacity(total_decoded);
    #[allow(clippy::uninit_vec)]
    unsafe {
        out_buf.set_len(total_decoded);
    }

    let dst = out_buf.as_mut_ptr();

    // Parallel line decode for large inputs (>= 4MB): split lines across threads.
    // Each thread decodes a contiguous block of lines directly to its final position
    // in the shared output buffer. SAFETY: non-overlapping output regions per thread.
    if data.len() >= PARALLEL_DECODE_THRESHOLD && full_lines >= 64 {
        let out_addr = dst as usize;
        let num_threads = num_cpus().max(1);
        let lines_per_chunk = (full_lines / num_threads).max(1);

        // Build per-thread task ranges: (start_line, end_line)
        let mut tasks: Vec<(usize, usize)> = Vec::new();
        let mut line_off = 0;
        while line_off < full_lines {
            let end = (line_off + lines_per_chunk).min(full_lines);
            tasks.push((line_off, end));
            line_off = end;
        }

        let decode_result: Result<(), io::Error> = std::thread::scope(|s| {
            let handles: Vec<_> = tasks
                .iter()
                .map(|&(start_line, end_line)| {
                    s.spawn(move || -> Result<(), io::Error> {
                        let out_ptr = out_addr as *mut u8;
                        let mut i = start_line;

                        while i + 4 <= end_line {
                            let in_base = i * line_stride;
                            let ob = i * decoded_per_line;
                            unsafe {
                                let s0 = std::slice::from_raw_parts_mut(
                                    out_ptr.add(ob),
                                    decoded_per_line,
                                );
                                if BASE64_ENGINE
                                    .decode(&data[in_base..in_base + line_len], s0.as_out())
                                    .is_err()
                                {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "invalid input",
                                    ));
                                }
                                let s1 = std::slice::from_raw_parts_mut(
                                    out_ptr.add(ob + decoded_per_line),
                                    decoded_per_line,
                                );
                                if BASE64_ENGINE
                                    .decode(
                                        &data[in_base + line_stride
                                            ..in_base + line_stride + line_len],
                                        s1.as_out(),
                                    )
                                    .is_err()
                                {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "invalid input",
                                    ));
                                }
                                let s2 = std::slice::from_raw_parts_mut(
                                    out_ptr.add(ob + 2 * decoded_per_line),
                                    decoded_per_line,
                                );
                                if BASE64_ENGINE
                                    .decode(
                                        &data[in_base + 2 * line_stride
                                            ..in_base + 2 * line_stride + line_len],
                                        s2.as_out(),
                                    )
                                    .is_err()
                                {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "invalid input",
                                    ));
                                }
                                let s3 = std::slice::from_raw_parts_mut(
                                    out_ptr.add(ob + 3 * decoded_per_line),
                                    decoded_per_line,
                                );
                                if BASE64_ENGINE
                                    .decode(
                                        &data[in_base + 3 * line_stride
                                            ..in_base + 3 * line_stride + line_len],
                                        s3.as_out(),
                                    )
                                    .is_err()
                                {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "invalid input",
                                    ));
                                }
                            }
                            i += 4;
                        }

                        while i < end_line {
                            let in_start = i * line_stride;
                            let out_off = i * decoded_per_line;
                            let out_slice = unsafe {
                                std::slice::from_raw_parts_mut(
                                    out_ptr.add(out_off),
                                    decoded_per_line,
                                )
                            };
                            if BASE64_ENGINE
                                .decode(&data[in_start..in_start + line_len], out_slice.as_out())
                                .is_err()
                            {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "invalid input",
                                ));
                            }
                            i += 1;
                        }

                        Ok(())
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap()?;
            }
            Ok(())
        });

        if decode_result.is_err() {
            return Some(decode_error());
        }
    } else {
        // Sequential decode with 4x unrolling for smaller inputs
        let mut i = 0;

        while i + 4 <= full_lines {
            let in_base = i * line_stride;
            let out_base = i * decoded_per_line;
            unsafe {
                let s0 = std::slice::from_raw_parts_mut(dst.add(out_base), decoded_per_line);
                if BASE64_ENGINE
                    .decode(&data[in_base..in_base + line_len], s0.as_out())
                    .is_err()
                {
                    return Some(decode_error());
                }

                let s1 = std::slice::from_raw_parts_mut(
                    dst.add(out_base + decoded_per_line),
                    decoded_per_line,
                );
                if BASE64_ENGINE
                    .decode(
                        &data[in_base + line_stride..in_base + line_stride + line_len],
                        s1.as_out(),
                    )
                    .is_err()
                {
                    return Some(decode_error());
                }

                let s2 = std::slice::from_raw_parts_mut(
                    dst.add(out_base + 2 * decoded_per_line),
                    decoded_per_line,
                );
                if BASE64_ENGINE
                    .decode(
                        &data[in_base + 2 * line_stride..in_base + 2 * line_stride + line_len],
                        s2.as_out(),
                    )
                    .is_err()
                {
                    return Some(decode_error());
                }

                let s3 = std::slice::from_raw_parts_mut(
                    dst.add(out_base + 3 * decoded_per_line),
                    decoded_per_line,
                );
                if BASE64_ENGINE
                    .decode(
                        &data[in_base + 3 * line_stride..in_base + 3 * line_stride + line_len],
                        s3.as_out(),
                    )
                    .is_err()
                {
                    return Some(decode_error());
                }
            }
            i += 4;
        }

        while i < full_lines {
            let in_start = i * line_stride;
            let in_end = in_start + line_len;
            let out_off = i * decoded_per_line;
            let out_slice =
                unsafe { std::slice::from_raw_parts_mut(dst.add(out_off), decoded_per_line) };
            match BASE64_ENGINE.decode(&data[in_start..in_end], out_slice.as_out()) {
                Ok(_) => {}
                Err(_) => return Some(decode_error()),
            }
            i += 1;
        }
    }

    // Decode remainder
    if remainder_clean_len > 0 {
        let rem = if remainder.last() == Some(&b'\n') {
            &remainder[..remainder.len() - 1]
        } else {
            remainder
        };
        let out_off = full_lines * decoded_per_line;
        let out_slice =
            unsafe { std::slice::from_raw_parts_mut(dst.add(out_off), remainder_clean_len) };
        match BASE64_ENGINE.decode(rem, out_slice.as_out()) {
            Ok(_) => {}
            Err(_) => return Some(decode_error()),
        }
    }

    // Single write_all for the entire decoded output
    Some(out.write_all(&out_buf[..total_decoded]))
}

/// Decode a clean (no whitespace) buffer in-place with SIMD.
fn decode_clean_slice(data: &mut [u8], out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    match BASE64_ENGINE.decode_inplace(data) {
        Ok(decoded) => out.write_all(decoded),
        Err(_) => decode_error(),
    }
}

/// Cold error path — keeps hot decode path tight by moving error construction out of line.
#[cold]
#[inline(never)]
fn decode_error() -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::InvalidData, "invalid input"))
}

/// Decode clean base64 data (no whitespace) from a borrowed slice.
fn decode_borrowed_clean(out: &mut impl Write, data: &[u8]) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    // Parallel decode for large data: split at 4-byte boundaries,
    // decode each chunk independently (base64 is context-free per 4-char group).
    if data.len() >= PARALLEL_DECODE_THRESHOLD {
        return decode_borrowed_clean_parallel(out, data);
    }
    // Pre-allocate exact output size to avoid decode_to_vec's reallocation.
    // Decoded size = data.len() * 3 / 4 minus padding.
    let pad = data.iter().rev().take(2).filter(|&&b| b == b'=').count();
    let decoded_size = data.len() * 3 / 4 - pad;
    let mut buf: Vec<u8> = Vec::with_capacity(decoded_size);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(decoded_size);
    }
    match BASE64_ENGINE.decode(data, buf[..decoded_size].as_out()) {
        Ok(decoded) => {
            out.write_all(decoded)?;
            Ok(())
        }
        Err(_) => decode_error(),
    }
}

/// Parallel decode: split at 4-byte boundaries, decode chunks in parallel.
/// Pre-allocates a single contiguous output buffer with exact decoded offsets computed
/// upfront, so each thread decodes directly to its final position. No compaction needed.
fn decode_borrowed_clean_parallel(out: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let num_threads = num_cpus().max(1);
    let raw_chunk = data.len() / num_threads;
    // Align to 4 bytes (each 4 base64 chars = 3 decoded bytes, context-free)
    let chunk_size = ((raw_chunk + 3) / 4) * 4;

    let chunks: Vec<&[u8]> = data.chunks(chunk_size.max(4)).collect();

    // Compute exact decoded sizes per chunk upfront to eliminate the compaction pass.
    let mut offsets: Vec<usize> = Vec::with_capacity(chunks.len() + 1);
    offsets.push(0);
    let mut total_decoded = 0usize;
    for (i, chunk) in chunks.iter().enumerate() {
        let decoded_size = if i == chunks.len() - 1 {
            let pad = chunk.iter().rev().take(2).filter(|&&b| b == b'=').count();
            chunk.len() * 3 / 4 - pad
        } else {
            chunk.len() * 3 / 4
        };
        total_decoded += decoded_size;
        offsets.push(total_decoded);
    }

    let mut output_buf: Vec<u8> = Vec::with_capacity(total_decoded);
    #[allow(clippy::uninit_vec)]
    unsafe {
        output_buf.set_len(total_decoded);
    }

    // Parallel decode: each thread decodes directly into its exact final position.
    // SAFETY: each thread writes to a non-overlapping region of the output buffer.
    let out_addr = output_buf.as_mut_ptr() as usize;
    let decode_result: Result<(), io::Error> = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                let offset = offsets[i];
                let expected_size = offsets[i + 1] - offset;
                s.spawn(move || -> Result<(), io::Error> {
                    // SAFETY: each thread writes to non-overlapping region
                    let out_slice = unsafe {
                        std::slice::from_raw_parts_mut(
                            (out_addr as *mut u8).add(offset),
                            expected_size,
                        )
                    };
                    let decoded = BASE64_ENGINE
                        .decode(chunk, out_slice.as_out())
                        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid input"))?;
                    debug_assert_eq!(decoded.len(), expected_size);
                    Ok(())
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap()?;
        }
        Ok(())
    });

    decode_result?;

    out.write_all(&output_buf[..total_decoded])
}

/// Strip non-base64 characters (for -i / --ignore-garbage).
fn strip_non_base64(data: &[u8]) -> Vec<u8> {
    data.iter()
        .copied()
        .filter(|&b| is_base64_char(b))
        .collect()
}

/// Check if a byte is a valid base64 alphabet character or padding.
#[inline]
fn is_base64_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='
}

/// Stream-encode from a reader to a writer. Used for stdin processing.
/// Dispatches to specialized paths for wrap_col=0 (no wrap) and wrap_col>0 (wrapping).
pub fn encode_stream(
    reader: &mut impl Read,
    wrap_col: usize,
    writer: &mut impl Write,
) -> io::Result<()> {
    if wrap_col == 0 {
        return encode_stream_nowrap(reader, writer);
    }
    encode_stream_wrapped(reader, wrap_col, writer)
}

/// Streaming encode with NO line wrapping — optimized fast path.
/// Read size is 24MB (divisible by 3): encoded output = 24MB * 4/3 = 32MB.
/// 24MB reads mean 10-18MB input is consumed in a single read() call,
/// and the encoded output writes in 1-2 write() calls.
fn encode_stream_nowrap(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    // 24MB aligned to 3 bytes: 24MB reads handle up to 24MB input in one pass.
    const NOWRAP_READ: usize = 24 * 1024 * 1024; // exactly divisible by 3

    // SAFETY: buf bytes are written by read_full before being processed.
    // encode_buf bytes are written by encode before being read.
    let mut buf: Vec<u8> = Vec::with_capacity(NOWRAP_READ);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(NOWRAP_READ);
    }
    let encode_buf_size = BASE64_ENGINE.encoded_length(NOWRAP_READ);
    let mut encode_buf: Vec<u8> = Vec::with_capacity(encode_buf_size);
    #[allow(clippy::uninit_vec)]
    unsafe {
        encode_buf.set_len(encode_buf_size);
    }

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let enc_len = BASE64_ENGINE.encoded_length(n);
        let encoded = BASE64_ENGINE.encode(&buf[..n], encode_buf[..enc_len].as_out());
        writer.write_all(encoded)?;
    }
    Ok(())
}

/// Streaming encode WITH line wrapping.
/// For the common case (wrap_col divides evenly into 3-byte input groups),
/// uses fuse_wrap to build a contiguous output buffer with newlines interleaved,
/// then writes it in a single write() call. This eliminates the overhead of
/// many writev() syscalls (one per ~512 lines via IoSlice).
///
/// For non-aligned wrap columns, falls back to the IoSlice/writev approach.
fn encode_stream_wrapped(
    reader: &mut impl Read,
    wrap_col: usize,
    writer: &mut impl Write,
) -> io::Result<()> {
    let bytes_per_line = wrap_col * 3 / 4;
    // For the common case (76-col wrapping, bytes_per_line=57 which is divisible by 3),
    // align the read buffer to bytes_per_line boundaries so each chunk produces
    // complete lines with no column carry-over between chunks.
    if bytes_per_line > 0 && bytes_per_line.is_multiple_of(3) {
        return encode_stream_wrapped_fused(reader, wrap_col, bytes_per_line, writer);
    }

    // Fallback: non-aligned wrap columns use IoSlice/writev with column tracking
    const STREAM_READ: usize = 12 * 1024 * 1024;
    let mut buf: Vec<u8> = Vec::with_capacity(STREAM_READ);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(STREAM_READ);
    }
    let encode_buf_size = BASE64_ENGINE.encoded_length(STREAM_READ);
    let mut encode_buf: Vec<u8> = Vec::with_capacity(encode_buf_size);
    #[allow(clippy::uninit_vec)]
    unsafe {
        encode_buf.set_len(encode_buf_size);
    }

    let mut col = 0usize;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let enc_len = BASE64_ENGINE.encoded_length(n);
        let encoded = BASE64_ENGINE.encode(&buf[..n], encode_buf[..enc_len].as_out());

        write_wrapped_iov_streaming(encoded, wrap_col, &mut col, writer)?;
    }

    if col > 0 {
        writer.write_all(b"\n")?;
    }

    Ok(())
}

/// Direct-to-position encode+wrap streaming: align reads to bytes_per_line boundaries,
/// encode each line directly into its final position with newline appended.
/// Eliminates the two-pass encode-then-fuse_wrap approach.
/// For 76-col wrapping (bytes_per_line=57): 12MB / 57 = ~210K complete lines per chunk.
/// Output = 210K * 77 bytes = ~16MB, one write() syscall per chunk.
fn encode_stream_wrapped_fused(
    reader: &mut impl Read,
    wrap_col: usize,
    bytes_per_line: usize,
    writer: &mut impl Write,
) -> io::Result<()> {
    // Align read size to bytes_per_line for complete output lines per chunk.
    // ~420K lines * 57 bytes = ~24MB input, ~32MB output.
    let lines_per_chunk = (24 * 1024 * 1024) / bytes_per_line;
    let read_size = lines_per_chunk * bytes_per_line;
    let line_out = wrap_col + 1; // wrap_col encoded bytes + 1 newline

    // SAFETY: buf bytes are written by read_full before being processed.
    // out_buf bytes are written by encode before being read.
    let mut buf: Vec<u8> = Vec::with_capacity(read_size);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(read_size);
    }
    // Output buffer: enough for all lines + remainder
    let max_output = lines_per_chunk * line_out + BASE64_ENGINE.encoded_length(bytes_per_line) + 2;
    let mut out_buf: Vec<u8> = Vec::with_capacity(max_output);
    #[allow(clippy::uninit_vec)]
    unsafe {
        out_buf.set_len(max_output);
    }

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }

        let full_lines = n / bytes_per_line;
        let remainder = n % bytes_per_line;

        // Encode each input line directly into its final output position.
        // Each 57-byte input line -> 76 encoded bytes + '\n' = 77 bytes at offset line_idx * 77.
        // This eliminates the separate encode + fuse_wrap copy entirely.
        let dst = out_buf.as_mut_ptr();
        let mut line_idx = 0;

        // 4-line unrolled loop for better ILP
        while line_idx + 4 <= full_lines {
            let in_base = line_idx * bytes_per_line;
            let out_base = line_idx * line_out;
            unsafe {
                let s0 = std::slice::from_raw_parts_mut(dst.add(out_base), wrap_col);
                let _ = BASE64_ENGINE.encode(&buf[in_base..in_base + bytes_per_line], s0.as_out());
                *dst.add(out_base + wrap_col) = b'\n';

                let s1 = std::slice::from_raw_parts_mut(dst.add(out_base + line_out), wrap_col);
                let _ = BASE64_ENGINE.encode(
                    &buf[in_base + bytes_per_line..in_base + 2 * bytes_per_line],
                    s1.as_out(),
                );
                *dst.add(out_base + line_out + wrap_col) = b'\n';

                let s2 = std::slice::from_raw_parts_mut(dst.add(out_base + 2 * line_out), wrap_col);
                let _ = BASE64_ENGINE.encode(
                    &buf[in_base + 2 * bytes_per_line..in_base + 3 * bytes_per_line],
                    s2.as_out(),
                );
                *dst.add(out_base + 2 * line_out + wrap_col) = b'\n';

                let s3 = std::slice::from_raw_parts_mut(dst.add(out_base + 3 * line_out), wrap_col);
                let _ = BASE64_ENGINE.encode(
                    &buf[in_base + 3 * bytes_per_line..in_base + 4 * bytes_per_line],
                    s3.as_out(),
                );
                *dst.add(out_base + 3 * line_out + wrap_col) = b'\n';
            }
            line_idx += 4;
        }

        // Remaining full lines
        while line_idx < full_lines {
            let in_base = line_idx * bytes_per_line;
            let out_base = line_idx * line_out;
            unsafe {
                let s = std::slice::from_raw_parts_mut(dst.add(out_base), wrap_col);
                let _ = BASE64_ENGINE.encode(&buf[in_base..in_base + bytes_per_line], s.as_out());
                *dst.add(out_base + wrap_col) = b'\n';
            }
            line_idx += 1;
        }

        let mut wp = full_lines * line_out;

        // Handle remainder (partial last line of this chunk)
        if remainder > 0 {
            let enc_len = BASE64_ENGINE.encoded_length(remainder);
            let line_input = &buf[full_lines * bytes_per_line..n];
            unsafe {
                let s = std::slice::from_raw_parts_mut(dst.add(wp), enc_len);
                let _ = BASE64_ENGINE.encode(line_input, s.as_out());
                *dst.add(wp + enc_len) = b'\n';
            }
            wp += enc_len + 1;
        }

        writer.write_all(&out_buf[..wp])?;
    }

    Ok(())
}

/// Stream-decode from a reader to a writer. Used for stdin processing.
/// In-place strip + decode: read chunk -> strip whitespace in-place in read buffer
/// -> decode in-place -> write. Eliminates separate clean buffer allocation (saves 32MB).
/// Uses 32MB read buffer for maximum pipe throughput — read_full retries to
/// fill the entire buffer from the pipe, and 32MB means even large inputs
/// (up to ~24MB after base64 encoding of 18MB raw) are read in a single syscall batch.
pub fn decode_stream(
    reader: &mut impl Read,
    ignore_garbage: bool,
    writer: &mut impl Write,
) -> io::Result<()> {
    const READ_CHUNK: usize = 32 * 1024 * 1024;
    // SAFETY: buf bytes are written by read_full before being processed.
    // The extra 4 bytes accommodate carry-over from previous chunk.
    let mut buf: Vec<u8> = Vec::with_capacity(READ_CHUNK + 4);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(READ_CHUNK + 4);
    }
    let mut carry = [0u8; 4];
    let mut carry_len = 0usize;

    loop {
        // Copy carry bytes to start of buffer, read new data after them
        if carry_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(carry.as_ptr(), buf.as_mut_ptr(), carry_len);
            }
        }
        let n = read_full(reader, &mut buf[carry_len..carry_len + READ_CHUNK])?;
        if n == 0 {
            break;
        }
        let total_raw = carry_len + n;

        // Strip whitespace in-place in the buffer itself.
        // This eliminates the separate clean buffer allocation (saves 16MB).
        let clean_len = if ignore_garbage {
            // Scalar filter for ignore_garbage mode (rare path)
            let ptr = buf.as_mut_ptr();
            let mut wp = 0usize;
            for i in 0..total_raw {
                let b = unsafe { *ptr.add(i) };
                if is_base64_char(b) {
                    unsafe { *ptr.add(wp) = b };
                    wp += 1;
                }
            }
            wp
        } else {
            // In-place SIMD gap-copy using memchr2 to find \n and \r positions.
            // For typical base64 (76-char lines), newlines are ~1/77 of the data,
            // so we process ~76 bytes per memchr hit.
            let ptr = buf.as_mut_ptr();
            let data = &buf[..total_raw];
            let mut wp = 0usize;
            let mut gap_start = 0usize;
            let mut has_rare_ws = false;

            for pos in memchr::memchr2_iter(b'\n', b'\r', data) {
                let gap_len = pos - gap_start;
                if gap_len > 0 {
                    if !has_rare_ws {
                        has_rare_ws = data[gap_start..pos]
                            .iter()
                            .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c);
                    }
                    if wp != gap_start {
                        unsafe {
                            std::ptr::copy(ptr.add(gap_start), ptr.add(wp), gap_len);
                        }
                    }
                    wp += gap_len;
                }
                gap_start = pos + 1;
            }
            let tail_len = total_raw - gap_start;
            if tail_len > 0 {
                if !has_rare_ws {
                    has_rare_ws = data[gap_start..total_raw]
                        .iter()
                        .any(|&b| b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c);
                }
                if wp != gap_start {
                    unsafe {
                        std::ptr::copy(ptr.add(gap_start), ptr.add(wp), tail_len);
                    }
                }
                wp += tail_len;
            }

            // Second pass for rare whitespace (tab, space, VT, FF) — only when detected.
            if has_rare_ws {
                let mut rp = 0;
                let mut cwp = 0;
                while rp < wp {
                    let b = unsafe { *ptr.add(rp) };
                    if NOT_WHITESPACE[b as usize] {
                        unsafe { *ptr.add(cwp) = b };
                        cwp += 1;
                    }
                    rp += 1;
                }
                cwp
            } else {
                wp
            }
        };

        carry_len = 0;
        let is_last = n < READ_CHUNK;

        if is_last {
            // Last chunk: decode everything (including padding)
            decode_clean_slice(&mut buf[..clean_len], writer)?;
        } else {
            // Save incomplete base64 quadruplet for next iteration
            let decode_len = (clean_len / 4) * 4;
            let leftover = clean_len - decode_len;
            if leftover > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        buf.as_ptr().add(decode_len),
                        carry.as_mut_ptr(),
                        leftover,
                    );
                }
                carry_len = leftover;
            }
            if decode_len > 0 {
                decode_clean_slice(&mut buf[..decode_len], writer)?;
            }
        }
    }

    // Handle any remaining carry-over bytes
    if carry_len > 0 {
        let mut carry_buf = carry[..carry_len].to_vec();
        decode_clean_slice(&mut carry_buf, writer)?;
    }

    Ok(())
}

/// Write all IoSlice entries using write_vectored (writev syscall).
/// Hot path: single write_vectored succeeds fully (common on Linux pipes/files).
/// Cold path: partial write handled out-of-line to keep hot path tight.
#[inline(always)]
fn write_all_vectored(out: &mut impl Write, slices: &[io::IoSlice]) -> io::Result<()> {
    if slices.is_empty() {
        return Ok(());
    }
    let total: usize = slices.iter().map(|s| s.len()).sum();
    let written = out.write_vectored(slices)?;
    if written >= total {
        return Ok(());
    }
    if written == 0 {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "write zero"));
    }
    write_all_vectored_slow(out, slices, written)
}

/// Handle partial write (cold path, never inlined).
#[cold]
#[inline(never)]
fn write_all_vectored_slow(
    out: &mut impl Write,
    slices: &[io::IoSlice],
    mut skip: usize,
) -> io::Result<()> {
    for slice in slices {
        let len = slice.len();
        if skip >= len {
            skip -= len;
            continue;
        }
        out.write_all(&slice[skip..])?;
        skip = 0;
    }
    Ok(())
}

/// Read as many bytes as possible into buf, retrying on partial reads.
/// Fast path: regular file reads usually return the full buffer on the first call,
/// avoiding the loop overhead entirely.
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
