use std::io::{self, Read, Write};

use base64_simd::AsOut;

const BASE64_ENGINE: &base64_simd::Base64 = &base64_simd::STANDARD;

/// Streaming encode chunk: 24MB aligned to 3 bytes.
/// Larger chunks = fewer loop iterations = fewer encode setup calls.
const STREAM_ENCODE_CHUNK: usize = 24 * 1024 * 1024 - (24 * 1024 * 1024 % 3);

/// Chunk size for no-wrap encoding: 24MB aligned to 3 bytes.
const NOWRAP_CHUNK: usize = 24 * 1024 * 1024 - (24 * 1024 * 1024 % 3);

/// Encode data and write to output with line wrapping.
/// Uses SIMD encoding with reusable buffers for maximum throughput.
pub fn encode_to_writer(data: &[u8], wrap_col: usize, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if wrap_col == 0 {
        return encode_no_wrap(data, out);
    }

    encode_wrapped(data, wrap_col, out)
}

/// Encode without wrapping — sequential SIMD encoding in chunks.
fn encode_no_wrap(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    // Size buffer for actual data, not max chunk size.
    let actual_chunk = NOWRAP_CHUNK.min(data.len());
    let enc_max = BASE64_ENGINE.encoded_length(actual_chunk);
    let mut buf = vec![0u8; enc_max];

    for chunk in data.chunks(NOWRAP_CHUNK) {
        let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
        let encoded = BASE64_ENGINE.encode(chunk, buf[..enc_len].as_out());
        out.write_all(encoded)?;
    }
    Ok(())
}

/// Encode with line wrapping.
/// Encodes in 6MB input chunks aligned to line boundaries, then wraps
/// and writes each chunk. 6MB input -> ~8MB encoded+wrapped output.
fn encode_wrapped(data: &[u8], wrap_col: usize, out: &mut impl Write) -> io::Result<()> {
    // Calculate bytes_per_line: the input bytes that produce exactly wrap_col encoded chars.
    // wrap_col base64 chars = (wrap_col * 3 / 4) input bytes (when wrap_col is divisible by 4).
    // For the default wrap_col=76: 76*3/4 = 57 bytes per line.
    let bytes_per_line = wrap_col * 3 / 4;

    // Encode in 6MB chunks aligned to bytes_per_line so each chunk produces
    // complete lines, avoiding column tracking across chunks.
    // 6MB input -> ~8MB encoded -> ~8.1MB wrapped output.
    let lines_per_chunk = (6 * 1024 * 1024) / bytes_per_line.max(1);
    let max_input_chunk = lines_per_chunk * bytes_per_line.max(1);
    let input_chunk = max_input_chunk.max(bytes_per_line.max(1)).min(data.len());

    let enc_max = BASE64_ENGINE.encoded_length(input_chunk);
    let mut encode_buf = vec![0u8; enc_max];

    // Output buffer: encoded + newlines. One newline per wrap_col chars.
    let max_lines = enc_max / wrap_col + 2;
    let wrapped_max = enc_max + max_lines;
    let mut wrap_buf = vec![0u8; wrapped_max];

    for chunk in data.chunks(max_input_chunk.max(1)) {
        let enc_len = BASE64_ENGINE.encoded_length(chunk.len());
        let encoded = BASE64_ENGINE.encode(chunk, encode_buf[..enc_len].as_out());
        let wp = wrap_encoded(encoded, wrap_col, &mut wrap_buf);
        out.write_all(&wrap_buf[..wp])?;
    }

    Ok(())
}

/// Wrap encoded base64 data with newlines at `wrap_col` columns.
/// Returns number of bytes written to `wrap_buf`.
/// Uses unsafe ptr::copy_nonoverlapping for maximum throughput,
/// with 4-line unrolling to reduce loop overhead.
#[inline]
fn wrap_encoded(encoded: &[u8], wrap_col: usize, wrap_buf: &mut [u8]) -> usize {
    let line_out = wrap_col + 1;
    let mut rp = 0;
    let mut wp = 0;

    // Unrolled: process 4 lines per iteration
    while rp + 4 * wrap_col <= encoded.len() {
        unsafe {
            let src = encoded.as_ptr().add(rp);
            let dst = wrap_buf.as_mut_ptr().add(wp);

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
                wrap_buf.as_mut_ptr().add(wp),
                wrap_col,
            );
        }
        wp += wrap_col;
        wrap_buf[wp] = b'\n';
        wp += 1;
        rp += wrap_col;
    }

    // Partial last line
    if rp < encoded.len() {
        let remaining = encoded.len() - rp;
        wrap_buf[wp..wp + remaining].copy_from_slice(&encoded[rp..rp + remaining]);
        wp += remaining;
        wrap_buf[wp] = b'\n';
        wp += 1;
    }

    wp
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
        return decode_owned_clean(&mut cleaned, out);
    }

    // Fast path: strip newlines with memchr (SIMD), then SIMD decode
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

    decode_owned_clean(data, out)
}

/// Strip all whitespace from a Vec in-place using SIMD memchr for newlines.
fn strip_whitespace_inplace(data: &mut Vec<u8>) {
    // Quick check for newlines using SIMD
    if memchr::memchr(b'\n', data).is_none() {
        if data.iter().any(|&b| is_whitespace(b)) {
            data.retain(|&b| !is_whitespace(b));
        }
        return;
    }

    // In-place compaction using raw pointers to avoid borrow conflict.
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

/// Decode by stripping all whitespace from the entire input at once,
/// then performing a single SIMD decode pass.
fn decode_stripping_whitespace(data: &[u8], out: &mut impl Write) -> io::Result<()> {
    // Quick check: any whitespace at all?
    if memchr::memchr2(b'\n', b'\r', data).is_none()
        && !data.iter().any(|&b| b == b' ' || b == b'\t')
    {
        // No whitespace — decode directly from borrowed data
        return decode_borrowed_clean(out, data);
    }

    // Strip newlines from entire input in a single pass using SIMD memchr.
    let mut clean = Vec::with_capacity(data.len());
    let mut last = 0;
    for pos in memchr::memchr_iter(b'\n', data) {
        if pos > last {
            clean.extend_from_slice(&data[last..pos]);
        }
        last = pos + 1;
    }
    if last < data.len() {
        clean.extend_from_slice(&data[last..]);
    }

    // Handle rare non-newline whitespace (CR, tab, etc.)
    if clean.iter().any(|&b| is_whitespace(b)) {
        clean.retain(|&b| !is_whitespace(b));
    }

    decode_owned_clean(&mut clean, out)
}

/// Decode a clean (no whitespace) owned buffer in-place with SIMD.
fn decode_owned_clean(data: &mut [u8], out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    match BASE64_ENGINE.decode_inplace(data) {
        Ok(decoded) => out.write_all(decoded),
        Err(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid input")),
    }
}

/// Decode clean base64 data (no whitespace) from a borrowed slice.
fn decode_borrowed_clean(out: &mut impl Write, data: &[u8]) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    match BASE64_ENGINE.decode_to_vec(data) {
        Ok(decoded) => {
            out.write_all(&decoded)?;
            Ok(())
        }
        Err(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid input")),
    }
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
        // No wrapping: encode each 4MB chunk and write directly.
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
        // Wrapping: batch wrapped output into a pre-allocated buffer.
        let max_wrapped = encode_buf_size + (encode_buf_size / wrap_col + 2);
        let mut wrap_buf = vec![0u8; max_wrapped];
        let mut col = 0usize;

        loop {
            let n = read_full(reader, &mut buf)?;
            if n == 0 {
                break;
            }
            let enc_len = BASE64_ENGINE.encoded_length(n);
            let encoded = BASE64_ENGINE.encode(&buf[..n], encode_buf[..enc_len].as_out());

            // Build wrapped output in wrap_buf, then single write.
            let wp = build_wrapped_output(encoded, wrap_col, &mut col, &mut wrap_buf);
            writer.write_all(&wrap_buf[..wp])?;
        }

        if col > 0 {
            writer.write_all(b"\n")?;
        }
    }

    Ok(())
}

/// Build wrapped output into a pre-allocated buffer.
/// Returns the number of bytes written to wrap_buf.
#[inline]
fn build_wrapped_output(
    data: &[u8],
    wrap_col: usize,
    col: &mut usize,
    wrap_buf: &mut [u8],
) -> usize {
    let mut rp = 0;
    let mut wp = 0;

    while rp < data.len() {
        let space = wrap_col - *col;
        let avail = data.len() - rp;

        if avail <= space {
            wrap_buf[wp..wp + avail].copy_from_slice(&data[rp..rp + avail]);
            wp += avail;
            *col += avail;
            if *col == wrap_col {
                wrap_buf[wp] = b'\n';
                wp += 1;
                *col = 0;
            }
            break;
        } else {
            wrap_buf[wp..wp + space].copy_from_slice(&data[rp..rp + space]);
            wp += space;
            wrap_buf[wp] = b'\n';
            wp += 1;
            rp += space;
            *col = 0;
        }
    }

    wp
}

/// Stream-decode from a reader to a writer. Used for stdin processing.
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

        // Build clean buffer: carry-over + stripped chunk
        clean.clear();
        clean.extend_from_slice(&carry);
        carry.clear();

        let chunk = &buf[..n];
        if ignore_garbage {
            clean.extend(chunk.iter().copied().filter(|&b| is_base64_char(b)));
        } else {
            // Strip newlines using SIMD memchr
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
            decode_owned_clean(&mut clean, writer)?;
        } else {
            // Save incomplete base64 quadruplet for next iteration
            let decode_len = (clean.len() / 4) * 4;
            if decode_len < clean.len() {
                carry.extend_from_slice(&clean[decode_len..]);
            }
            if decode_len > 0 {
                clean.truncate(decode_len);
                decode_owned_clean(&mut clean, writer)?;
            }
        }
    }

    // Handle any remaining carry-over bytes
    if !carry.is_empty() {
        decode_owned_clean(&mut carry, writer)?;
    }

    Ok(())
}

/// Read as many bytes as possible into buf, retrying on partial reads.
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
