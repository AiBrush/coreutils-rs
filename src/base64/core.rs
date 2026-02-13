use std::io::{self, Read, Write};

use base64_simd::AsOut;
use rayon::prelude::*;

const BASE64_ENGINE: &base64_simd::Base64 = &base64_simd::STANDARD;

/// Streaming encode chunk: 24MB aligned to 3 bytes.
const STREAM_ENCODE_CHUNK: usize = 24 * 1024 * 1024 - (24 * 1024 * 1024 % 3);

/// Chunk size for no-wrap encoding: 32MB aligned to 3 bytes.
/// Larger chunks = fewer write() syscalls for big files.
const NOWRAP_CHUNK: usize = 32 * 1024 * 1024 - (32 * 1024 * 1024 % 3);

/// Minimum data size for parallel encoding (4MB).
/// Below this, single-threaded SIMD is faster than rayon overhead.
const PARALLEL_ENCODE_THRESHOLD: usize = 4 * 1024 * 1024;

/// Minimum data size for parallel decoding (4MB of base64 data).
const PARALLEL_DECODE_THRESHOLD: usize = 4 * 1024 * 1024;

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

    for chunk in &encoded_chunks {
        out.write_all(chunk)?;
    }
    Ok(())
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
            let mut encode_buf: Vec<u8> = Vec::with_capacity(enc_max);
            #[allow(clippy::uninit_vec)]
            unsafe {
                encode_buf.set_len(enc_max);
            }
            let encoded = BASE64_ENGINE.encode(chunk, encode_buf[..enc_max].as_out());
            let max_lines = enc_max / wrap_col + 2;
            let mut fused: Vec<u8> = Vec::with_capacity(enc_max + max_lines);
            #[allow(clippy::uninit_vec)]
            unsafe {
                fused.set_len(enc_max + max_lines);
            }
            let wp = fuse_wrap(encoded, wrap_col, &mut fused);
            fused.truncate(wp);
            fused
        })
        .collect();

    for chunk in &encoded_chunks {
        out.write_all(chunk)?;
    }
    Ok(())
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

/// Strip all whitespace from a Vec in-place using SIMD memchr.
/// Single-pass compaction: scan for newlines (most common whitespace in base64),
/// compact non-newline segments, then handle rare other whitespace.
fn strip_whitespace_inplace(data: &mut Vec<u8>) {
    // Quick check: no newlines at all?
    if memchr::memchr(b'\n', data).is_none() {
        if data.iter().any(|&b| is_whitespace(b)) {
            data.retain(|&b| !is_whitespace(b));
        }
        return;
    }

    // In-place compaction using raw pointers
    let ptr = data.as_ptr();
    let mut_ptr = data.as_mut_ptr();
    let len = data.len();
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };

    let mut wp = 0usize;
    let mut rp = 0usize;

    for pos in memchr::memchr_iter(b'\n', slice) {
        if pos > rp {
            let seg = pos - rp;
            unsafe {
                std::ptr::copy(ptr.add(rp), mut_ptr.add(wp), seg);
            }
            wp += seg;
        }
        rp = pos + 1;
    }

    if rp < len {
        let seg = len - rp;
        unsafe {
            std::ptr::copy(ptr.add(rp), mut_ptr.add(wp), seg);
        }
        wp += seg;
    }

    data.truncate(wp);

    // Handle rare non-newline whitespace (CR, tab, etc.)
    if data.iter().any(|&b| is_whitespace(b)) {
        data.retain(|&b| !is_whitespace(b));
    }
}

/// Decode by stripping whitespace and decoding in a single fused pass.
/// For data with no whitespace, decodes directly without any copy.
fn decode_stripping_whitespace(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    // Quick check: any whitespace at all?
    if memchr::memchr2(b'\n', b'\r', data).is_none()
        && !data.iter().any(|&b| b == b' ' || b == b'\t')
    {
        // No whitespace — decode directly from borrowed data
        return decode_borrowed_clean(out, data);
    }

    // Fused strip+collect: use SIMD memchr to find newlines,
    // copy non-newline segments via raw pointers (skip bounds checks).
    let mut clean: Vec<u8> = Vec::with_capacity(data.len());
    let mut wp = 0usize;
    let mut last = 0;
    for pos in memchr::memchr_iter(b'\n', data) {
        if pos > last {
            let seg = pos - last;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(last),
                    clean.as_mut_ptr().add(wp),
                    seg,
                );
            }
            wp += seg;
        }
        last = pos + 1;
    }
    if last < data.len() {
        let seg = data.len() - last;
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(last), clean.as_mut_ptr().add(wp), seg);
        }
        wp += seg;
    }
    unsafe {
        clean.set_len(wp);
    }

    // Handle rare non-newline whitespace (CR, tab, etc.)
    if clean.iter().any(|&b| is_whitespace(b)) {
        clean.retain(|&b| !is_whitespace(b));
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
fn decode_borrowed_clean_parallel(out: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let num_threads = rayon::current_num_threads().max(1);
    let raw_chunk = data.len() / num_threads;
    // Align to 4 bytes (each 4 base64 chars = 3 decoded bytes, context-free)
    let chunk_size = ((raw_chunk + 3) / 4) * 4;

    let chunks: Vec<&[u8]> = data.chunks(chunk_size.max(4)).collect();
    let decoded_chunks: Result<Vec<Vec<u8>>, io::Error> = chunks
        .par_iter()
        .map(|chunk| {
            BASE64_ENGINE
                .decode_to_vec(chunk)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid input"))
        })
        .collect();

    for chunk in &decoded_chunks? {
        out.write_all(chunk)?;
    }
    Ok(())
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

/// Check if a byte is ASCII whitespace.
#[inline]
fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Stream-encode from a reader to a writer. Used for stdin processing.
pub fn encode_stream(
    reader: &mut impl Read,
    wrap_col: usize,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut buf = vec![0u8; STREAM_ENCODE_CHUNK];

    let encode_buf_size = BASE64_ENGINE.encoded_length(STREAM_ENCODE_CHUNK);
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
#[inline]
fn build_fused_output(data: &[u8], wrap_col: usize, col: &mut usize, out_buf: &mut [u8]) -> usize {
    let mut rp = 0;
    let mut wp = 0;

    while rp < data.len() {
        let space = wrap_col - *col;
        let avail = data.len() - rp;

        if avail <= space {
            out_buf[wp..wp + avail].copy_from_slice(&data[rp..rp + avail]);
            wp += avail;
            *col += avail;
            if *col == wrap_col {
                out_buf[wp] = b'\n';
                wp += 1;
                *col = 0;
            }
            break;
        } else {
            out_buf[wp..wp + space].copy_from_slice(&data[rp..rp + space]);
            wp += space;
            out_buf[wp] = b'\n';
            wp += 1;
            rp += space;
            *col = 0;
        }
    }

    wp
}

/// Stream-decode from a reader to a writer. Used for stdin processing.
/// Fused single-pass: read chunk → strip whitespace in-place → decode immediately.
pub fn decode_stream(
    reader: &mut impl Read,
    ignore_garbage: bool,
    writer: &mut impl Write,
) -> io::Result<()> {
    const READ_CHUNK: usize = 4 * 1024 * 1024;
    let mut buf = vec![0u8; READ_CHUNK];
    let mut clean = Vec::with_capacity(READ_CHUNK);
    let mut carry: Vec<u8> = Vec::with_capacity(4);

    loop {
        let n = read_full(reader, &mut buf)?;
        if n == 0 {
            break;
        }

        // Fused: build clean buffer from carry-over + stripped chunk in one pass
        clean.clear();
        clean.extend_from_slice(&carry);
        carry.clear();

        let chunk = &buf[..n];
        if ignore_garbage {
            clean.extend(chunk.iter().copied().filter(|&b| is_base64_char(b)));
        } else {
            // Strip newlines using SIMD memchr — single pass
            let mut last = 0;
            for pos in memchr::memchr_iter(b'\n', chunk) {
                if pos > last {
                    clean.extend_from_slice(&chunk[last..pos]);
                }
                last = pos + 1;
            }
            if last < n {
                clean.extend_from_slice(&chunk[last..]);
            }
            // Handle rare non-newline whitespace
            if clean.iter().any(|&b| is_whitespace(b) && b != b'\n') {
                clean.retain(|&b| !is_whitespace(b));
            }
        }

        let is_last = n < READ_CHUNK;

        if is_last {
            // Last chunk: decode everything (including padding)
            decode_clean_slice(&mut clean, writer)?;
        } else {
            // Save incomplete base64 quadruplet for next iteration
            let decode_len = (clean.len() / 4) * 4;
            if decode_len < clean.len() {
                carry.extend_from_slice(&clean[decode_len..]);
            }
            if decode_len > 0 {
                clean.truncate(decode_len);
                decode_clean_slice(&mut clean, writer)?;
            }
        }
    }

    // Handle any remaining carry-over bytes
    if !carry.is_empty() {
        decode_clean_slice(&mut carry, writer)?;
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
