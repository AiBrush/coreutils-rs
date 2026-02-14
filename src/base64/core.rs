use std::io::{self, Read, Write};

use base64_simd::AsOut;
use rayon::prelude::*;

const BASE64_ENGINE: &base64_simd::Base64 = &base64_simd::STANDARD;

/// Chunk size for no-wrap encoding: 32MB aligned to 3 bytes.
/// Larger chunks = fewer write() syscalls for big files.
const NOWRAP_CHUNK: usize = 32 * 1024 * 1024 - (32 * 1024 * 1024 % 3);

/// Minimum data size for parallel encoding (16MB).
/// base64_simd SIMD encoding runs at ~8 GB/s per core, processing 10MB
/// in ~1.25ms. Rayon thread pool creation + sync costs ~100-500us.
/// For 10MB benchmark workloads, single-threaded is faster.
const PARALLEL_ENCODE_THRESHOLD: usize = 16 * 1024 * 1024;

/// Minimum data size for parallel decoding (32MB of base64 data).
/// base64_simd decode runs at ~4-8 GB/s per core. For 13.5MB (the typical
/// decode benchmark), single-threaded decode takes ~2-3ms while parallel
/// decode adds ~1ms for allocation + rayon overhead, making it slower on
/// 2-core CI runners. Use single-threaded in-place decode for data under 32MB.
const PARALLEL_DECODE_THRESHOLD: usize = 32 * 1024 * 1024;

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
    if data.len() >= PARALLEL_ENCODE_THRESHOLD {
        return encode_no_wrap_parallel(data, out);
    }

    let actual_chunk = NOWRAP_CHUNK.min(data.len());
    let enc_max = BASE64_ENGINE.encoded_length(actual_chunk);
    // SAFETY: encode() writes exactly enc_len bytes before we read them.
    let mut buf: Vec<u8> = Vec::with_capacity(enc_max);
    #[allow(clippy::uninit_vec)]
    unsafe {
        buf.set_len(enc_max);
    }

    for chunk in data.chunks(NOWRAP_CHUNK) {
        let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
        let encoded = BASE64_ENGINE.encode(chunk, buf[..enc_len].as_out());
        out.write_all(encoded)?;
    }
    Ok(())
}

/// Parallel no-wrap encoding: split at 3-byte boundaries, encode chunks in parallel.
/// Each chunk except possibly the last is 3-byte aligned, so no padding in intermediate chunks.
/// Uses write_vectored (writev) to send all encoded chunks in a single syscall.
fn encode_no_wrap_parallel(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    let num_threads = rayon::current_num_threads().max(1);
    let raw_chunk = data.len() / num_threads;
    // Align to 3 bytes so each chunk encodes without padding (except the last)
    let chunk_size = ((raw_chunk + 2) / 3) * 3;

    let chunks: Vec<&[u8]> = data.chunks(chunk_size.max(3)).collect();
    let encoded_chunks: Vec<Vec<u8>> = chunks
        .par_iter()
        .map(|chunk| {
            let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
            let mut buf: Vec<u8> = Vec::with_capacity(enc_len);
            #[allow(clippy::uninit_vec)]
            unsafe {
                buf.set_len(enc_len);
            }
            let _ = BASE64_ENGINE.encode(chunk, buf[..enc_len].as_out());
            buf
        })
        .collect();

    // Use write_vectored to send all chunks in a single syscall
    let iov: Vec<io::IoSlice> = encoded_chunks.iter().map(|c| io::IoSlice::new(c)).collect();
    write_all_vectored(out, &iov)
}

/// Encode with line wrapping — uses writev to interleave encoded segments
/// with newlines without copying data. For each wrap_col-sized segment of
/// encoded output, we create an IoSlice pointing directly at the encode buffer,
/// interleaved with IoSlice entries pointing at a static newline byte.
fn encode_wrapped(data: &[u8], wrap_col: usize, out: &mut impl Write) -> io::Result<()> {
    // Calculate bytes_per_line: input bytes that produce exactly wrap_col encoded chars.
    // For default wrap_col=76: 76*3/4 = 57 bytes per line.
    let bytes_per_line = wrap_col * 3 / 4;
    if bytes_per_line == 0 {
        // Degenerate case: wrap_col < 4, fall back to byte-at-a-time
        return encode_wrapped_small(data, wrap_col, out);
    }

    // Parallel encoding for large data when bytes_per_line is a multiple of 3.
    // This guarantees each chunk encodes to complete base64 without padding.
    if data.len() >= PARALLEL_ENCODE_THRESHOLD && bytes_per_line.is_multiple_of(3) {
        return encode_wrapped_parallel(data, wrap_col, bytes_per_line, out);
    }

    // Bulk-encode-then-insert-newlines: encode large chunks (many lines at once)
    // with a single SIMD encode() call, then copy the encoded output with newline
    // insertions into the final buffer. This reduces the number of encode() calls
    // by ~256x (from ~175K to ~684 for a 10MB file), amortizing the per-call SIMD
    // setup overhead. Each chunk encodes 256 lines = 14592 bytes input -> 19456 bytes
    // encoded, then inserts 256 newlines during the copy to the output buffer.
    //
    // For default wrap=76: bytes_per_line=57, wrap_col=76.
    // Chunk: 256 * 57 = 14592 bytes input -> 19456 bytes raw encoded.
    // Output: 256 * 77 = 19712 bytes (76 encoded + 1 newline per line).
    if bytes_per_line.is_multiple_of(3) {
        let line_out = wrap_col + 1; // wrap_col encoded bytes + 1 newline
        let total_full_lines = data.len() / bytes_per_line;
        let remainder_input = data.len() % bytes_per_line;

        // Calculate exact output size
        let remainder_encoded = if remainder_input > 0 {
            BASE64_ENGINE.encoded_length(remainder_input) + 1 // +1 for trailing newline
        } else {
            0
        };
        let total_output = total_full_lines * line_out + remainder_encoded;

        // Pre-allocate single contiguous output buffer
        let mut out_buf: Vec<u8> = Vec::with_capacity(total_output);
        #[allow(clippy::uninit_vec)]
        unsafe {
            out_buf.set_len(total_output);
        }

        // Chunk size: 256 lines per encode call to amortize SIMD setup.
        // 256 * 57 = 14592 bytes input, fits in L1 cache.
        const LINES_PER_CHUNK: usize = 256;

        let chunk_input = LINES_PER_CHUNK * bytes_per_line;
        let chunk_encoded = BASE64_ENGINE.encoded_length(chunk_input);
        // Temporary buffer for bulk encoding (reused across chunks)
        let mut enc_buf: Vec<u8> = Vec::with_capacity(chunk_encoded);
        #[allow(clippy::uninit_vec)]
        unsafe {
            enc_buf.set_len(chunk_encoded);
        }

        let dst = out_buf.as_mut_ptr();
        let mut line_idx = 0usize;

        // Process full chunks of LINES_PER_CHUNK lines
        while line_idx + LINES_PER_CHUNK <= total_full_lines {
            let in_start = line_idx * bytes_per_line;
            let in_end = in_start + chunk_input;
            let encoded =
                BASE64_ENGINE.encode(&data[in_start..in_end], enc_buf[..chunk_encoded].as_out());

            // Copy encoded data with newline insertions.
            // Source: contiguous encoded bytes (wrap_col per line, no gaps).
            // Destination: wrap_col bytes + '\n' per line.
            let out_start = line_idx * line_out;
            unsafe {
                let sp = encoded.as_ptr();
                let dp = dst.add(out_start);
                for li in 0..LINES_PER_CHUNK {
                    std::ptr::copy_nonoverlapping(
                        sp.add(li * wrap_col),
                        dp.add(li * line_out),
                        wrap_col,
                    );
                    *dp.add(li * line_out + wrap_col) = b'\n';
                }
            }
            line_idx += LINES_PER_CHUNK;
        }

        // Remaining full lines (< LINES_PER_CHUNK)
        let remaining_lines = total_full_lines - line_idx;
        if remaining_lines > 0 {
            let in_start = line_idx * bytes_per_line;
            let in_end = in_start + remaining_lines * bytes_per_line;
            let enc_len = BASE64_ENGINE.encoded_length(in_end - in_start);
            let encoded =
                BASE64_ENGINE.encode(&data[in_start..in_end], enc_buf[..enc_len].as_out());

            let out_start = line_idx * line_out;
            unsafe {
                let sp = encoded.as_ptr();
                let dp = dst.add(out_start);
                for li in 0..remaining_lines {
                    std::ptr::copy_nonoverlapping(
                        sp.add(li * wrap_col),
                        dp.add(li * line_out),
                        wrap_col,
                    );
                    *dp.add(li * line_out + wrap_col) = b'\n';
                }
            }
        }

        // Handle remainder (last partial line)
        if remainder_input > 0 {
            let in_base = total_full_lines * bytes_per_line;
            let out_base = total_full_lines * line_out;
            let enc_len = BASE64_ENGINE.encoded_length(remainder_input);
            unsafe {
                let s = std::slice::from_raw_parts_mut(dst.add(out_base), enc_len);
                let _ = BASE64_ENGINE.encode(&data[in_base..], s.as_out());
                *dst.add(out_base + enc_len) = b'\n';
            }
        }

        return out.write_all(&out_buf[..total_output]);
    }

    // Fallback for non-3-aligned bytes_per_line: use writev
    let lines_per_chunk = (32 * 1024 * 1024) / bytes_per_line;
    let max_input_chunk = (lines_per_chunk * bytes_per_line).max(bytes_per_line);
    let input_chunk = max_input_chunk.min(data.len());

    let enc_max = BASE64_ENGINE.encoded_length(input_chunk);
    let mut encode_buf: Vec<u8> = Vec::with_capacity(enc_max);
    #[allow(clippy::uninit_vec)]
    unsafe {
        encode_buf.set_len(enc_max);
    }

    for chunk in data.chunks(max_input_chunk.max(1)) {
        let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
        let encoded = BASE64_ENGINE.encode(chunk, encode_buf[..enc_len].as_out());
        write_wrapped_iov(encoded, wrap_col, out)?;
    }

    Ok(())
}

/// Static newline byte for IoSlice references in writev calls.
static NEWLINE: [u8; 1] = [b'\n'];

/// Write encoded base64 data with line wrapping using write_vectored (writev).
/// Builds IoSlice entries pointing at wrap_col-sized segments of the encoded buffer,
/// interleaved with newline IoSlices, then writes in batches of MAX_WRITEV_IOV.
/// This is zero-copy: no fused output buffer needed.
#[inline]
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
fn encode_wrapped_parallel(
    data: &[u8],
    wrap_col: usize,
    bytes_per_line: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let line_out = wrap_col + 1; // wrap_col data + 1 newline per line
    let total_full_lines = data.len() / bytes_per_line;
    let remainder_input = data.len() % bytes_per_line;

    // Calculate exact output size
    let remainder_encoded = if remainder_input > 0 {
        BASE64_ENGINE.encoded_length(remainder_input) + 1 // +1 for trailing newline
    } else {
        0
    };
    let total_output = total_full_lines * line_out + remainder_encoded;

    // Pre-allocate single contiguous output buffer
    let mut outbuf: Vec<u8> = Vec::with_capacity(total_output);
    #[allow(clippy::uninit_vec)]
    unsafe {
        outbuf.set_len(total_output);
    }

    // Split work at line boundaries for parallel processing
    let num_threads = rayon::current_num_threads().max(1);
    let lines_per_chunk = (total_full_lines / num_threads).max(1);
    let input_chunk = lines_per_chunk * bytes_per_line;

    // Compute per-chunk metadata: (input_offset, output_offset, num_input_bytes)
    let mut tasks: Vec<(usize, usize, usize)> = Vec::new();
    let mut in_off = 0usize;
    let mut out_off = 0usize;
    while in_off < data.len() {
        let chunk_input = input_chunk.min(data.len() - in_off);
        // Align to bytes_per_line except for the very last chunk
        let aligned_input = if in_off + chunk_input < data.len() {
            (chunk_input / bytes_per_line) * bytes_per_line
        } else {
            chunk_input
        };
        if aligned_input == 0 {
            break;
        }
        let full_lines = aligned_input / bytes_per_line;
        let rem = aligned_input % bytes_per_line;
        let chunk_output = full_lines * line_out
            + if rem > 0 {
                BASE64_ENGINE.encoded_length(rem) + 1
            } else {
                0
            };
        tasks.push((in_off, out_off, aligned_input));
        in_off += aligned_input;
        out_off += chunk_output;
    }

    // Parallel encode: each thread encodes lines directly into the final
    // output buffer, eliminating per-thread buffer allocation and the
    // scatter copy phase entirely. Each 57-byte input line encodes to
    // exactly 76 encoded bytes + 1 newline = 77 bytes at a known offset.
    // base64_simd handles the SIMD encoding even for 57-byte inputs.
    // SAFETY: tasks have non-overlapping output regions.
    let out_addr = outbuf.as_mut_ptr() as usize;

    tasks.par_iter().for_each(|&(in_off, out_off, chunk_len)| {
        let input = &data[in_off..in_off + chunk_len];
        let full_lines = chunk_len / bytes_per_line;
        let rem = chunk_len % bytes_per_line;

        let out_ptr = out_addr as *mut u8;

        // Encode each line directly into its final position in the output buffer.
        // No thread-local buffer needed — each 57-byte input -> 76 encoded bytes
        // written directly at out_off + line_idx * 77.
        if full_lines > 0 {
            let dst = unsafe { out_ptr.add(out_off) };
            let mut line_idx = 0;

            // 4-line unrolled loop for ILP
            while line_idx + 4 <= full_lines {
                let in_base = line_idx * bytes_per_line;
                let out_base = line_idx * line_out;
                unsafe {
                    let s0 = std::slice::from_raw_parts_mut(dst.add(out_base), wrap_col);
                    let _ = BASE64_ENGINE
                        .encode(&input[in_base..in_base + bytes_per_line], s0.as_out());
                    *dst.add(out_base + wrap_col) = b'\n';

                    let s1 = std::slice::from_raw_parts_mut(dst.add(out_base + line_out), wrap_col);
                    let _ = BASE64_ENGINE.encode(
                        &input[in_base + bytes_per_line..in_base + 2 * bytes_per_line],
                        s1.as_out(),
                    );
                    *dst.add(out_base + line_out + wrap_col) = b'\n';

                    let s2 =
                        std::slice::from_raw_parts_mut(dst.add(out_base + 2 * line_out), wrap_col);
                    let _ = BASE64_ENGINE.encode(
                        &input[in_base + 2 * bytes_per_line..in_base + 3 * bytes_per_line],
                        s2.as_out(),
                    );
                    *dst.add(out_base + 2 * line_out + wrap_col) = b'\n';

                    let s3 =
                        std::slice::from_raw_parts_mut(dst.add(out_base + 3 * line_out), wrap_col);
                    let _ = BASE64_ENGINE.encode(
                        &input[in_base + 3 * bytes_per_line..in_base + 4 * bytes_per_line],
                        s3.as_out(),
                    );
                    *dst.add(out_base + 3 * line_out + wrap_col) = b'\n';
                }
                line_idx += 4;
            }

            // Remaining lines one at a time
            while line_idx < full_lines {
                let in_base = line_idx * bytes_per_line;
                let out_base = line_idx * line_out;
                unsafe {
                    let s = std::slice::from_raw_parts_mut(dst.add(out_base), wrap_col);
                    let _ =
                        BASE64_ENGINE.encode(&input[in_base..in_base + bytes_per_line], s.as_out());
                    *dst.add(out_base + wrap_col) = b'\n';
                }
                line_idx += 1;
            }
        }

        // Handle remainder (last partial line of this chunk)
        if rem > 0 {
            let line_input = &input[full_lines * bytes_per_line..];
            let enc_len = BASE64_ENGINE.encoded_length(rem);
            let woff = out_off + full_lines * line_out;
            // Encode directly into final output position
            let out_slice =
                unsafe { std::slice::from_raw_parts_mut(out_ptr.add(woff), enc_len + 1) };
            let _ = BASE64_ENGINE.encode(line_input, out_slice[..enc_len].as_out());
            out_slice[enc_len] = b'\n';
        }
    });

    out.write_all(&outbuf[..total_output])
}

/// Fuse encoded base64 data with newlines in a single pass.
/// Uses ptr::copy_nonoverlapping with 8-line unrolling for max throughput.
/// Returns number of bytes written.
#[allow(dead_code)]
#[inline]
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

    // Fast path: single-pass strip + decode
    decode_stripping_whitespace(data, out)
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

/// Decode by stripping whitespace and decoding in a single fused pass.
/// For data with no whitespace, decodes directly without any copy.
///
/// For standard base64 with regular newlines (every 76 chars), uses a fast
/// fixed-stride copy that skips memchr scanning entirely. Falls back to SIMD
/// memchr2 gap-copy for irregular newline patterns or mixed whitespace.
fn decode_stripping_whitespace(data: &[u8], out: &mut impl Write) -> io::Result<()> {
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

    // Detect regular line pattern: if the first newline is at a consistent stride,
    // use fixed-stride copy (no memchr scanning needed). This covers standard base64
    // output with -w 76 (stride 77) or any other fixed wrap width.
    if let Some(stride) = detect_regular_newline_stride(data) {
        return decode_fixed_stride(data, stride, out);
    }

    // Fallback: SIMD gap-copy for irregular newline patterns.
    decode_stripping_whitespace_memchr(data, out)
}

/// Detect if data has newlines at a regular stride (every `stride` bytes).
/// Returns Some(stride) if the first 3 newlines are evenly spaced and the
/// stride matches the expected line width (no rare whitespace in between).
/// Returns None if the pattern is irregular.
#[inline]
fn detect_regular_newline_stride(data: &[u8]) -> Option<usize> {
    // Find first newline
    let pos1 = memchr::memchr(b'\n', data)?;
    if pos1 == 0 || pos1 >= data.len() - 1 {
        return None;
    }
    let stride = pos1 + 1; // expected line stride (e.g. 77 for wrap=76)

    // Verify second newline is at the expected position
    if stride >= data.len() {
        // Only one line — treat as regular with this stride
        // Check no rare whitespace in the line
        if data[..pos1]
            .iter()
            .any(|&b| b == b' ' || b == b'\t' || b == b'\r' || b == 0x0b || b == 0x0c)
        {
            return None;
        }
        return Some(stride);
    }
    if data.get(stride - 1).copied() != Some(b'\n') {
        return None;
    }

    // Verify third newline if data is large enough
    if 2 * stride <= data.len() {
        if data.get(2 * stride - 1).copied() != Some(b'\n') {
            return None;
        }
    }

    // Check for rare whitespace in the first line (representative sample)
    if data[..pos1]
        .iter()
        .any(|&b| b == b' ' || b == b'\t' || b == b'\r' || b == 0x0b || b == 0x0c)
    {
        return None;
    }

    Some(stride)
}

/// Fast fixed-stride decode for regular base64 output.
/// Bulk-strips newlines from chunks of 256 lines (19456 bytes) into an L1-resident
/// temp buffer, then decodes each chunk in a single SIMD call. This reduces the
/// number of decode() calls by ~256x (from ~175K to ~684 for a 10MB decode) and
/// avoids allocating a 13.3MB intermediate clean buffer.
///
/// Instead of: alloc 13.3MB clean buffer + 175K memcpy + 1 decode call
/// We do: reuse 20KB temp buffer + 684 bulk memcpy + 684 decode calls
///
/// The 20KB temp buffer stays hot in L1 cache, and each decode call processes
/// enough data (19456 bytes) for efficient SIMD utilization.
fn decode_fixed_stride(data: &[u8], stride: usize, out: &mut impl Write) -> io::Result<()> {
    let line_len = stride - 1; // data bytes per line (e.g. 76 for stride 77)
    let num_full_lines = data.len() / stride;
    let remainder = data.len() % stride;

    // Calculate exact decoded output size
    let clean_len = num_full_lines * line_len
        + if remainder > 0 {
            let tail = if data[data.len() - 1] == b'\n' {
                remainder - 1
            } else {
                remainder.min(line_len)
            };
            tail
        } else {
            0
        };
    // Each 4 base64 chars decode to 3 bytes, plus padding adjustment
    let decoded_size = clean_len * 3 / 4 + 3; // +3 for rounding

    // Pre-allocate output buffer for all decoded data
    let mut out_buf: Vec<u8> = Vec::with_capacity(decoded_size);
    #[allow(clippy::uninit_vec)]
    unsafe {
        out_buf.set_len(decoded_size);
    }
    let out_ptr = out_buf.as_mut_ptr();
    let mut out_wp = 0usize;

    // Temp buffer for bulk newline stripping: 256 lines * 76 bytes = 19456 bytes.
    // Stays hot in L1 cache (typically 32-48 KB).
    const LINES_PER_CHUNK: usize = 256;
    let chunk_clean = LINES_PER_CHUNK * line_len;
    let mut temp: Vec<u8> = Vec::with_capacity(chunk_clean + line_len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        temp.set_len(chunk_clean + line_len);
    }

    let src = data.as_ptr();
    let tmp = temp.as_mut_ptr();
    let mut line = 0usize;

    // Process full chunks of LINES_PER_CHUNK lines
    while line + LINES_PER_CHUNK <= num_full_lines {
        // Bulk copy 256 lines into temp buffer, skipping newlines
        unsafe {
            for li in 0..LINES_PER_CHUNK {
                std::ptr::copy_nonoverlapping(
                    src.add((line + li) * stride),
                    tmp.add(li * line_len),
                    line_len,
                );
            }
        }
        // Decode the chunk directly into the output buffer
        let dec_len = match BASE64_ENGINE.decode(
            &temp[..chunk_clean],
            unsafe { std::slice::from_raw_parts_mut(out_ptr.add(out_wp), decoded_size - out_wp) }
                .as_out(),
        ) {
            Ok(decoded) => decoded.len(),
            Err(_) => return decode_error(),
        };
        out_wp += dec_len;
        line += LINES_PER_CHUNK;
    }

    // Remaining full lines (< LINES_PER_CHUNK)
    let remaining_lines = num_full_lines - line;
    if remaining_lines > 0 {
        unsafe {
            for li in 0..remaining_lines {
                std::ptr::copy_nonoverlapping(
                    src.add((line + li) * stride),
                    tmp.add(li * line_len),
                    line_len,
                );
            }
        }
        let rem_clean = remaining_lines * line_len;
        // If there's no remainder, this is the last chunk — decode including any padding
        let dec_len = match BASE64_ENGINE.decode(
            &temp[..rem_clean],
            unsafe { std::slice::from_raw_parts_mut(out_ptr.add(out_wp), decoded_size - out_wp) }
                .as_out(),
        ) {
            Ok(decoded) => decoded.len(),
            Err(_) => return decode_error(),
        };
        out_wp += dec_len;
    }

    // Handle remainder (last partial line)
    if remainder > 0 {
        let actual_tail = if data[data.len() - 1] == b'\n' {
            remainder - 1
        } else {
            remainder.min(line_len)
        };
        if actual_tail > 0 {
            let tail_start = num_full_lines * stride;
            let dec_len = match BASE64_ENGINE.decode(
                &data[tail_start..tail_start + actual_tail],
                unsafe {
                    std::slice::from_raw_parts_mut(out_ptr.add(out_wp), decoded_size - out_wp)
                }
                .as_out(),
            ) {
                Ok(decoded) => decoded.len(),
                Err(_) => return decode_error(),
            };
            out_wp += dec_len;
        }
    }

    out.write_all(&out_buf[..out_wp])
}

/// SIMD memchr2 gap-copy whitespace strip for irregular newline patterns.
/// Falls back from the fixed-stride fast path when newlines aren't regular.
fn decode_stripping_whitespace_memchr(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    let mut clean: Vec<u8> = Vec::with_capacity(data.len());
    let dst = clean.as_mut_ptr();
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
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr().add(gap_start), dst.add(wp), gap_len);
            }
            wp += gap_len;
        }
        gap_start = pos + 1;
    }
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

    if clean.len() >= PARALLEL_DECODE_THRESHOLD {
        decode_borrowed_clean_parallel(out, &clean)
    } else {
        decode_clean_slice(&mut clean, out)
    }
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
    match BASE64_ENGINE.decode_to_vec(data) {
        Ok(decoded) => {
            out.write_all(&decoded)?;
            Ok(())
        }
        Err(_) => decode_error(),
    }
}

/// Parallel decode: split at 4-byte boundaries, decode chunks in parallel via rayon.
/// Pre-allocates a single contiguous output buffer with exact decoded offsets computed
/// upfront, so each thread decodes directly to its final position. No compaction needed.
fn decode_borrowed_clean_parallel(out: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let num_threads = rayon::current_num_threads().max(1);
    let raw_chunk = data.len() / num_threads;
    // Align to 4 bytes (each 4 base64 chars = 3 decoded bytes, context-free)
    let chunk_size = ((raw_chunk + 3) / 4) * 4;

    let chunks: Vec<&[u8]> = data.chunks(chunk_size.max(4)).collect();

    // Compute exact decoded sizes per chunk upfront to eliminate the compaction pass.
    // For all chunks except the last, decoded size is exactly chunk.len() * 3 / 4.
    // For the last chunk, account for '=' padding bytes.
    let mut offsets: Vec<usize> = Vec::with_capacity(chunks.len() + 1);
    offsets.push(0);
    let mut total_decoded = 0usize;
    for (i, chunk) in chunks.iter().enumerate() {
        let decoded_size = if i == chunks.len() - 1 {
            // Last chunk: count '=' padding to get exact decoded size
            let pad = chunk.iter().rev().take(2).filter(|&&b| b == b'=').count();
            chunk.len() * 3 / 4 - pad
        } else {
            // Non-last chunks: 4-byte aligned, no padding, exact 3/4 ratio
            chunk.len() * 3 / 4
        };
        total_decoded += decoded_size;
        offsets.push(total_decoded);
    }

    // Pre-allocate contiguous output buffer with exact total size
    let mut output_buf: Vec<u8> = Vec::with_capacity(total_decoded);
    #[allow(clippy::uninit_vec)]
    unsafe {
        output_buf.set_len(total_decoded);
    }

    // Parallel decode: each thread decodes directly into its exact final position.
    // No compaction pass needed since offsets are computed from exact decoded sizes.
    // SAFETY: each thread writes to a non-overlapping region of the output buffer.
    // Use usize representation of the pointer for Send+Sync compatibility with rayon.
    let out_addr = output_buf.as_mut_ptr() as usize;
    let decode_result: Result<Vec<()>, io::Error> = chunks
        .par_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let offset = offsets[i];
            let expected_size = offsets[i + 1] - offset;
            // SAFETY: each thread writes to non-overlapping region [offset..offset+expected_size]
            let out_slice = unsafe {
                std::slice::from_raw_parts_mut((out_addr as *mut u8).add(offset), expected_size)
            };
            let decoded = BASE64_ENGINE
                .decode(chunk, out_slice.as_out())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid input"))?;
            debug_assert_eq!(decoded.len(), expected_size);
            Ok(())
        })
        .collect();

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
/// Read size is 12MB (divisible by 3): encoded output = 12MB * 4/3 = 16MB.
/// 12MB reads mean 10MB input is consumed in a single read() call,
/// and the 16MB encoded output writes in 1-2 write() calls.
fn encode_stream_nowrap(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    // 12MB aligned to 3 bytes: encoded output = 12MB * 4/3 = 16MB.
    // For 10MB input: 1 read (10MB) instead of 2 reads.
    const NOWRAP_READ: usize = 12 * 1024 * 1024; // exactly divisible by 3

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
    // ~210K lines * 57 bytes = ~12MB input, ~16MB output.
    let lines_per_chunk = (12 * 1024 * 1024) / bytes_per_line;
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

    // Temporary buffer for bulk encoding (reused across read iterations)
    const STREAM_LINES_PER_CHUNK: usize = 256;
    let chunk_input = STREAM_LINES_PER_CHUNK * bytes_per_line;
    let chunk_encoded = BASE64_ENGINE.encoded_length(chunk_input);
    let mut enc_buf: Vec<u8> = Vec::with_capacity(chunk_encoded);
    #[allow(clippy::uninit_vec)]
    unsafe {
        enc_buf.set_len(chunk_encoded);
    }

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }

        let full_lines = n / bytes_per_line;
        let remainder = n % bytes_per_line;

        // Bulk-encode-then-insert-newlines: encode chunks of 256 lines at once
        // to amortize SIMD encode setup, then copy with newline insertions.
        let dst = out_buf.as_mut_ptr();
        let mut line_idx = 0usize;

        // Process full chunks of STREAM_LINES_PER_CHUNK lines
        while line_idx + STREAM_LINES_PER_CHUNK <= full_lines {
            let in_start = line_idx * bytes_per_line;
            let in_end = in_start + chunk_input;
            let encoded =
                BASE64_ENGINE.encode(&buf[in_start..in_end], enc_buf[..chunk_encoded].as_out());

            let out_start = line_idx * line_out;
            unsafe {
                let sp = encoded.as_ptr();
                let dp = dst.add(out_start);
                for li in 0..STREAM_LINES_PER_CHUNK {
                    std::ptr::copy_nonoverlapping(
                        sp.add(li * wrap_col),
                        dp.add(li * line_out),
                        wrap_col,
                    );
                    *dp.add(li * line_out + wrap_col) = b'\n';
                }
            }
            line_idx += STREAM_LINES_PER_CHUNK;
        }

        // Remaining full lines (< STREAM_LINES_PER_CHUNK)
        let remaining_lines = full_lines - line_idx;
        if remaining_lines > 0 {
            let in_start = line_idx * bytes_per_line;
            let in_end = in_start + remaining_lines * bytes_per_line;
            let enc_len = BASE64_ENGINE.encoded_length(in_end - in_start);
            let encoded = BASE64_ENGINE.encode(&buf[in_start..in_end], enc_buf[..enc_len].as_out());

            let out_start = line_idx * line_out;
            unsafe {
                let sp = encoded.as_ptr();
                let dp = dst.add(out_start);
                for li in 0..remaining_lines {
                    std::ptr::copy_nonoverlapping(
                        sp.add(li * wrap_col),
                        dp.add(li * line_out),
                        wrap_col,
                    );
                    *dp.add(li * line_out + wrap_col) = b'\n';
                }
            }
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
/// -> decode in-place -> write. Eliminates separate clean buffer allocation (saves 16MB).
/// Uses 16MB read buffer for maximum pipe throughput — read_full retries to
/// fill the entire buffer from the pipe, and 16MB means the entire 10MB
/// benchmark input is read in a single syscall batch, minimizing overhead.
pub fn decode_stream(
    reader: &mut impl Read,
    ignore_garbage: bool,
    writer: &mut impl Write,
) -> io::Result<()> {
    const READ_CHUNK: usize = 16 * 1024 * 1024;
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
/// Falls back to write_all per slice on partial writes.
fn write_all_vectored(out: &mut impl Write, slices: &[io::IoSlice]) -> io::Result<()> {
    if slices.is_empty() {
        return Ok(());
    }
    let total: usize = slices.iter().map(|s| s.len()).sum();

    // Try write_vectored first — often writes everything in one syscall
    let written = match out.write_vectored(slices) {
        Ok(n) if n >= total => return Ok(()),
        Ok(n) => n,
        Err(e) => return Err(e),
    };

    // Partial write fallback
    let mut skip = written;
    for slice in slices {
        let slen = slice.len();
        if skip >= slen {
            skip -= slen;
            continue;
        }
        if skip > 0 {
            out.write_all(&slice[skip..])?;
            skip = 0;
        } else {
            out.write_all(slice)?;
        }
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
