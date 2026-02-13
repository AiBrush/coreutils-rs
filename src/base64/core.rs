use std::io::{self, Read, Write};

use base64_simd::AsOut;
use rayon::prelude::*;

const BASE64_ENGINE: &base64_simd::Base64 = &base64_simd::STANDARD;

/// Chunk size for no-wrap encoding: 32MB aligned to 3 bytes.
/// Larger chunks = fewer write() syscalls for big files.
const NOWRAP_CHUNK: usize = 32 * 1024 * 1024 - (32 * 1024 * 1024 % 3);

/// Minimum data size for parallel encoding (1MB).
/// Lowered from 4MB so 10MB benchmark workloads get multi-core processing.
const PARALLEL_ENCODE_THRESHOLD: usize = 1024 * 1024;

/// Minimum data size for parallel decoding (1MB of base64 data).
/// Lowered from 4MB for better parallelism on typical workloads.
const PARALLEL_DECODE_THRESHOLD: usize = 1024 * 1024;

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

/// Encode with line wrapping — fused encode+wrap in a single output buffer.
/// Encodes aligned input chunks, then interleaves newlines directly into
/// a single output buffer, eliminating the separate wrap pass.
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

    // Align input chunk to bytes_per_line for complete output lines.
    // Use 32MB chunks — large enough to process most files in a single pass,
    // reducing write() syscalls.
    let lines_per_chunk = (32 * 1024 * 1024) / bytes_per_line;
    let max_input_chunk = (lines_per_chunk * bytes_per_line).max(bytes_per_line);
    let input_chunk = max_input_chunk.min(data.len());

    let enc_max = BASE64_ENGINE.encoded_length(input_chunk);
    let mut encode_buf: Vec<u8> = Vec::with_capacity(enc_max);
    #[allow(clippy::uninit_vec)]
    unsafe {
        encode_buf.set_len(enc_max);
    }

    // Fused output buffer: holds encoded data with newlines interleaved
    let max_lines = enc_max / wrap_col + 2;
    let fused_max = enc_max + max_lines;
    let mut fused_buf: Vec<u8> = Vec::with_capacity(fused_max);
    #[allow(clippy::uninit_vec)]
    unsafe {
        fused_buf.set_len(fused_max);
    }

    for chunk in data.chunks(max_input_chunk.max(1)) {
        let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
        let encoded = BASE64_ENGINE.encode(chunk, encode_buf[..enc_len].as_out());

        // Fuse: copy encoded data into fused_buf with newlines interleaved
        let wp = fuse_wrap(encoded, wrap_col, &mut fused_buf);
        out.write_all(&fused_buf[..wp])?;
    }

    Ok(())
}

/// Parallel wrapped encoding: split at bytes_per_line boundaries, encode + wrap in parallel.
/// Requires bytes_per_line.is_multiple_of(3) so each chunk encodes without intermediate padding.
/// Uses write_vectored (writev) to send all encoded+wrapped chunks in a single syscall.
fn encode_wrapped_parallel(
    data: &[u8],
    wrap_col: usize,
    bytes_per_line: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let num_threads = rayon::current_num_threads().max(1);
    // Split at bytes_per_line boundaries for complete output lines per chunk
    let lines_per_chunk = (data.len() / bytes_per_line / num_threads).max(1);
    let chunk_size = lines_per_chunk * bytes_per_line;

    let chunks: Vec<&[u8]> = data.chunks(chunk_size.max(bytes_per_line)).collect();
    let encoded_chunks: Vec<Vec<u8>> = chunks
        .par_iter()
        .map(|chunk| {
            let enc_max = BASE64_ENGINE.encoded_length(chunk.len());
            let max_lines = enc_max / wrap_col + 2;
            // Single allocation with two non-overlapping regions:
            //   [0..fused_size) = fuse_wrap output region
            //   [fused_size..fused_size+enc_max) = encode region
            let fused_size = enc_max + max_lines;
            let total_size = fused_size + enc_max;
            let mut buf: Vec<u8> = Vec::with_capacity(total_size);
            #[allow(clippy::uninit_vec)]
            unsafe {
                buf.set_len(total_size);
            }
            // Encode into the second region [fused_size..fused_size+enc_max]
            let _ = BASE64_ENGINE.encode(chunk, buf[fused_size..fused_size + enc_max].as_out());
            // Use split_at_mut to get non-overlapping mutable/immutable refs
            let (fused_region, encode_region) = buf.split_at_mut(fused_size);
            let encoded = &encode_region[..enc_max];
            let wp = fuse_wrap(encoded, wrap_col, fused_region);
            buf.truncate(wp);
            buf
        })
        .collect();

    // Use write_vectored to send all chunks in a single syscall
    let iov: Vec<io::IoSlice> = encoded_chunks.iter().map(|c| io::IoSlice::new(c)).collect();
    write_all_vectored(out, &iov)
}

/// Fuse encoded base64 data with newlines in a single pass.
/// Uses ptr::copy_nonoverlapping with 8-line unrolling for max throughput.
/// Returns number of bytes written.
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

/// Strip all whitespace from a Vec in-place using the lookup table.
/// Single-pass compaction: uses NOT_WHITESPACE table to classify all whitespace
/// types simultaneously, avoiding the previous multi-scan approach.
fn strip_whitespace_inplace(data: &mut Vec<u8>) {
    // Quick check: any whitespace at all?
    let has_ws = data.iter().any(|&b| !NOT_WHITESPACE[b as usize]);
    if !has_ws {
        return;
    }

    // Single-pass in-place compaction using the lookup table.
    let ptr = data.as_ptr();
    let mut_ptr = data.as_mut_ptr();
    let len = data.len();
    let mut wp = 0usize;

    for i in 0..len {
        let b = unsafe { *ptr.add(i) };
        if NOT_WHITESPACE[b as usize] {
            unsafe { *mut_ptr.add(wp) = b };
            wp += 1;
        }
    }

    data.truncate(wp);
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
/// Uses memchr2 SIMD gap-copy for \n/\r (the dominant whitespace in base64),
/// then a fallback pass for rare whitespace types (tab, space, VT, FF).
fn decode_stripping_whitespace(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    // Quick check: any whitespace at all?  Use the lookup table for a single scan.
    let has_ws = data.iter().any(|&b| !NOT_WHITESPACE[b as usize]);
    if !has_ws {
        // No whitespace — decode directly from borrowed data
        return decode_borrowed_clean(out, data);
    }

    // SIMD gap-copy: use memchr2 to find \n and \r positions, then copy the
    // gaps between them. For typical base64 (76-char lines), newlines are ~1/77
    // of the data, so we process ~76 bytes per memchr hit instead of 1 per scalar.
    let mut clean: Vec<u8> = Vec::with_capacity(data.len());
    let dst = clean.as_mut_ptr();
    let mut wp = 0usize;
    let mut gap_start = 0usize;

    for pos in memchr::memchr2_iter(b'\n', b'\r', data) {
        let gap_len = pos - gap_start;
        if gap_len > 0 {
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
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(gap_start), dst.add(wp), tail_len);
        }
        wp += tail_len;
    }
    unsafe {
        clean.set_len(wp);
    }

    // Second pass for rare whitespace (tab, space, VT, FF) using lookup table.
    // In typical base64 streams this does nothing, but correctness requires it.
    let has_rare_ws = clean.iter().any(|&b| !NOT_WHITESPACE[b as usize]);
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

    decode_clean_slice(&mut clean, out)
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
/// Uses 3MB read chunks (aligned to 3 bytes for padding-free intermediate encoding).
/// 3MB is optimal for piped input: large enough for good throughput, small enough
/// that read_full() fills the buffer quickly from pipes (3 reads at 1MB pipe size).
pub fn encode_stream(
    reader: &mut impl Read,
    wrap_col: usize,
    writer: &mut impl Write,
) -> io::Result<()> {
    // 3MB aligned to 3 bytes — sweet spot for pipe throughput
    const STREAM_READ: usize = 3 * 1024 * 1024;
    let mut buf = vec![0u8; STREAM_READ];

    let encode_buf_size = BASE64_ENGINE.encoded_length(STREAM_READ);
    let mut encode_buf = vec![0u8; encode_buf_size];

    if wrap_col == 0 {
        // No wrapping: encode each chunk and write directly.
        loop {
            let n = read_full(reader, &mut buf)?;
            if n == 0 {
                break;
            }
            let enc_len = BASE64_ENGINE.encoded_length(n);
            let encoded = BASE64_ENGINE.encode(&buf[..n], encode_buf[..enc_len].as_out());
            writer.write_all(encoded)?;
        }
    } else {
        // Wrapping: fused encode+wrap into a single output buffer.
        let max_fused = encode_buf_size + (encode_buf_size / wrap_col + 2);
        let mut fused_buf = vec![0u8; max_fused];
        let mut col = 0usize;

        loop {
            let n = read_full(reader, &mut buf)?;
            if n == 0 {
                break;
            }
            let enc_len = BASE64_ENGINE.encoded_length(n);
            let encoded = BASE64_ENGINE.encode(&buf[..n], encode_buf[..enc_len].as_out());

            // Build fused output in a single buffer, then one write.
            let wp = build_fused_output(encoded, wrap_col, &mut col, &mut fused_buf);
            writer.write_all(&fused_buf[..wp])?;
        }

        if col > 0 {
            writer.write_all(b"\n")?;
        }
    }

    Ok(())
}

/// Build fused encode+wrap output into a pre-allocated buffer.
/// Returns the number of bytes written.
/// Uses unsafe ptr ops to avoid bounds checks in the hot loop.
#[inline]
fn build_fused_output(data: &[u8], wrap_col: usize, col: &mut usize, out_buf: &mut [u8]) -> usize {
    let mut rp = 0;
    let mut wp = 0;
    let len = data.len();
    let src = data.as_ptr();
    let dst = out_buf.as_mut_ptr();

    while rp < len {
        let space = wrap_col - *col;
        let avail = len - rp;

        if avail <= space {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(rp), dst.add(wp), avail);
            }
            wp += avail;
            *col += avail;
            if *col == wrap_col {
                unsafe { *dst.add(wp) = b'\n' };
                wp += 1;
                *col = 0;
            }
            break;
        } else {
            unsafe {
                std::ptr::copy_nonoverlapping(src.add(rp), dst.add(wp), space);
                *dst.add(wp + space) = b'\n';
            }
            wp += space + 1;
            rp += space;
            *col = 0;
        }
    }

    wp
}

/// Stream-decode from a reader to a writer. Used for stdin processing.
/// Fused single-pass: read chunk -> strip whitespace -> decode immediately.
/// Uses 16MB read buffer to reduce syscalls and memchr2-based SIMD whitespace
/// stripping for the common case (only \n and \r whitespace in base64 streams).
pub fn decode_stream(
    reader: &mut impl Read,
    ignore_garbage: bool,
    writer: &mut impl Write,
) -> io::Result<()> {
    const READ_CHUNK: usize = 16 * 1024 * 1024;
    let mut buf = vec![0u8; READ_CHUNK];
    // Pre-allocate clean buffer once and reuse across iterations.
    // Use Vec with set_len for zero-overhead reset instead of clear() + extend().
    let mut clean: Vec<u8> = Vec::with_capacity(READ_CHUNK + 4);
    let mut carry = [0u8; 4];
    let mut carry_len = 0usize;

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }

        // Copy carry bytes to start of clean buffer (0-3 bytes from previous chunk)
        unsafe {
            std::ptr::copy_nonoverlapping(carry.as_ptr(), clean.as_mut_ptr(), carry_len);
        }

        let chunk = &buf[..n];
        if ignore_garbage {
            // Scalar filter for ignore_garbage mode (rare path)
            let dst = unsafe { clean.as_mut_ptr().add(carry_len) };
            let mut wp = 0usize;
            for &b in chunk {
                if is_base64_char(b) {
                    unsafe { *dst.add(wp) = b };
                    wp += 1;
                }
            }
            unsafe { clean.set_len(carry_len + wp) };
        } else {
            // SIMD gap-copy: use memchr2 to find \n and \r positions, then copy
            // the gaps between them. For typical base64 (76-char lines), newlines
            // are ~1/77 of the data, so we process ~76 bytes per memchr hit
            // instead of 1 byte per scalar iteration.
            let dst = unsafe { clean.as_mut_ptr().add(carry_len) };
            let mut wp = 0usize;
            let mut gap_start = 0usize;

            for pos in memchr::memchr2_iter(b'\n', b'\r', chunk) {
                let gap_len = pos - gap_start;
                if gap_len > 0 {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            chunk.as_ptr().add(gap_start),
                            dst.add(wp),
                            gap_len,
                        );
                    }
                    wp += gap_len;
                }
                gap_start = pos + 1;
            }
            let tail_len = n - gap_start;
            if tail_len > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        chunk.as_ptr().add(gap_start),
                        dst.add(wp),
                        tail_len,
                    );
                }
                wp += tail_len;
            }
            let total_clean = carry_len + wp;
            unsafe { clean.set_len(total_clean) };

            // Second pass for rare whitespace (tab, space, VT, FF) using lookup table.
            // In typical base64 streams this does nothing, but we need correctness.
            let has_rare_ws = clean[carry_len..total_clean]
                .iter()
                .any(|&b| !NOT_WHITESPACE[b as usize]);
            if has_rare_ws {
                let ptr = clean.as_mut_ptr();
                let mut rp = carry_len;
                let mut cwp = carry_len;
                while rp < total_clean {
                    let b = unsafe { *ptr.add(rp) };
                    if NOT_WHITESPACE[b as usize] {
                        unsafe { *ptr.add(cwp) = b };
                        cwp += 1;
                    }
                    rp += 1;
                }
                clean.truncate(cwp);
            }
        }

        carry_len = 0;
        let is_last = n < READ_CHUNK;

        if is_last {
            // Last chunk: decode everything (including padding)
            decode_clean_slice(&mut clean, writer)?;
        } else {
            // Save incomplete base64 quadruplet for next iteration
            let clean_len = clean.len();
            let decode_len = (clean_len / 4) * 4;
            let leftover = clean_len - decode_len;
            if leftover > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        clean.as_ptr().add(decode_len),
                        carry.as_mut_ptr(),
                        leftover,
                    );
                }
                carry_len = leftover;
            }
            if decode_len > 0 {
                clean.truncate(decode_len);
                decode_clean_slice(&mut clean, writer)?;
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
