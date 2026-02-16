use std::io::Write;

/// Reverse each line in the input data and write to output.
/// Lines are delimited by newline (b'\n').
/// ASCII lines are reversed byte-by-byte (fast path).
/// Non-ASCII lines are reversed by Unicode characters.
pub fn rev_bytes(data: &[u8], out: &mut impl Write) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Pre-allocate output buffer same size as input
    // Use 256KB flush threshold for batched output
    const FLUSH_THRESHOLD: usize = 256 * 1024;
    let mut output = Vec::with_capacity(data.len().min(FLUSH_THRESHOLD * 2));
    let mut start = 0;

    for pos in memchr::memchr_iter(b'\n', data) {
        let line = &data[start..pos];
        reverse_line(line, &mut output);
        output.push(b'\n');
        start = pos + 1;

        // Flush in batches for large files
        if output.len() >= FLUSH_THRESHOLD {
            out.write_all(&output)?;
            output.clear();
        }
    }

    // Handle last line without trailing newline
    if start < data.len() {
        let line = &data[start..];
        reverse_line(line, &mut output);
    }

    if !output.is_empty() {
        out.write_all(&output)?;
    }
    Ok(())
}

/// Reverse a single line (without the newline delimiter).
/// Fast path for pure ASCII, slow path for UTF-8 multibyte.
#[inline]
fn reverse_line(line: &[u8], output: &mut Vec<u8>) {
    if line.is_empty() {
        return;
    }

    // Check if all bytes are ASCII (< 128)
    if is_ascii(line) {
        // ASCII fast path: reverse bytes directly
        let start = output.len();
        output.extend_from_slice(line);
        output[start..].reverse();
    } else {
        // UTF-8 path: reverse by characters without intermediate Vec
        match std::str::from_utf8(line) {
            Ok(s) => {
                // Pre-reserve space
                output.reserve(line.len());
                // Write reversed chars directly
                for ch in s.chars().rev() {
                    let mut buf = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut buf);
                    output.extend_from_slice(encoded.as_bytes());
                }
            }
            Err(_) => {
                // Invalid UTF-8: reverse bytes (same as GNU rev behavior)
                let start = output.len();
                output.extend_from_slice(line);
                output[start..].reverse();
            }
        }
    }
}

/// Check if all bytes in the slice are ASCII (< 128).
/// Uses word-at-a-time trick for SIMD-like speed.
#[inline]
fn is_ascii(data: &[u8]) -> bool {
    // Process 8 bytes at a time
    let chunks = data.chunks_exact(8);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let word = u64::from_ne_bytes(chunk.try_into().unwrap());
        if word & 0x8080808080808080 != 0 {
            return false;
        }
    }

    for &b in remainder {
        if b & 0x80 != 0 {
            return false;
        }
    }

    true
}
