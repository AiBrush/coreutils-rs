use std::io::Write;

/// Fold (wrap) lines to a given width.
///
/// Modes:
/// - `bytes` mode (-b): count bytes, break at byte boundaries
/// - default mode: count columns (tab = advance to next tab stop, backspace = decrement)
///
/// If `spaces` (-s): break at the last space within the width instead of mid-word.
pub fn fold_bytes(
    data: &[u8],
    width: usize,
    count_bytes: bool,
    break_at_spaces: bool,
    out: &mut impl Write,
) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if width == 0 {
        return fold_width_zero(data, out);
    }

    // Fast path: byte mode without -s, use SIMD-accelerated scanning
    if count_bytes && !break_at_spaces {
        return fold_byte_fast(data, width, out);
    }

    let mut output = Vec::with_capacity(data.len() + data.len() / width);

    if count_bytes {
        fold_byte_mode(data, width, break_at_spaces, &mut output);
    } else {
        fold_column_mode(data, width, break_at_spaces, &mut output);
    }

    out.write_all(&output)
}

/// Width 0: GNU fold behavior — each byte becomes a newline.
fn fold_width_zero(data: &[u8], out: &mut impl Write) -> std::io::Result<()> {
    let output = vec![b'\n'; data.len()];
    out.write_all(&output)
}

/// Fast fold by byte count without -s flag.
/// Uses memchr to find newlines, bulk-copies runs, inserts breaks at exact positions.
fn fold_byte_fast(data: &[u8], width: usize, out: &mut impl Write) -> std::io::Result<()> {
    // Each line can have at most one extra newline inserted
    let mut output = Vec::with_capacity(data.len() + data.len() / width + 1);
    let mut pos: usize = 0;

    while pos < data.len() {
        // Find the next newline within the remaining data
        let remaining = &data[pos..];

        match memchr::memchr(b'\n', remaining) {
            Some(nl_offset) => {
                // Process the segment up to (and including) the newline
                let segment = &data[pos..pos + nl_offset + 1];
                fold_segment_bytes(&mut output, segment, width);
                pos += nl_offset + 1;
            }
            None => {
                // No more newlines: process the rest
                fold_segment_bytes(&mut output, &data[pos..], width);
                break;
            }
        }
    }

    out.write_all(&output)
}

/// Fold a single line segment (no internal newlines except possibly trailing) by bytes.
#[inline]
fn fold_segment_bytes(output: &mut Vec<u8>, segment: &[u8], width: usize) {
    let mut start = 0;
    while start + width < segment.len() {
        // Check if the character at start+width is a newline (end of line)
        if segment[start + width] == b'\n' {
            output.extend_from_slice(&segment[start..start + width + 1]);
            return;
        }
        output.extend_from_slice(&segment[start..start + width]);
        output.push(b'\n');
        start += width;
    }
    // Remaining bytes
    if start < segment.len() {
        output.extend_from_slice(&segment[start..]);
    }
}

/// Fold by byte count with -s (break at spaces).
fn fold_byte_mode(data: &[u8], width: usize, break_at_spaces: bool, output: &mut Vec<u8>) {
    let mut col: usize = 0;
    let mut last_space_out_pos: Option<usize> = None;

    for &byte in data {
        if byte == b'\n' {
            output.push(b'\n');
            col = 0;
            last_space_out_pos = None;
            continue;
        }

        if col >= width {
            if break_at_spaces {
                if let Some(sp_pos) = last_space_out_pos {
                    let after_space = output[sp_pos + 1..].to_vec();
                    output.truncate(sp_pos + 1);
                    output.push(b'\n');
                    output.extend_from_slice(&after_space);
                    col = after_space.len();
                    last_space_out_pos = None;
                } else {
                    output.push(b'\n');
                    col = 0;
                }
            } else {
                output.push(b'\n');
                col = 0;
            }
        }

        if break_at_spaces && (byte == b' ' || byte == b'\t') {
            last_space_out_pos = Some(output.len());
        }

        output.push(byte);
        col += 1;
    }
}

/// Fold by column count (default mode, handles tabs and backspaces).
fn fold_column_mode(data: &[u8], width: usize, break_at_spaces: bool, output: &mut Vec<u8>) {
    let mut col: usize = 0;
    let mut last_space_out_pos: Option<usize> = None;

    for &byte in data {
        if byte == b'\n' {
            output.push(b'\n');
            col = 0;
            last_space_out_pos = None;
            continue;
        }

        // Calculate display width of this byte
        let char_width = if byte == b'\t' {
            let next_stop = ((col / 8) + 1) * 8;
            next_stop - col
        } else if byte == b'\x08' || byte < 0x20 || byte == 0x7f {
            // Backspace and other control chars: 0 width
            0
        } else {
            1
        };

        // Handle backspace
        if byte == b'\x08' {
            output.push(byte);
            if col > 0 {
                col -= 1;
            }
            continue;
        }

        // Check if adding this character would exceed width
        if col + char_width > width && char_width > 0 {
            if break_at_spaces {
                if let Some(sp_pos) = last_space_out_pos {
                    let after_space = output[sp_pos + 1..].to_vec();
                    output.truncate(sp_pos + 1);
                    output.push(b'\n');
                    col = recalc_column(&after_space);
                    output.extend_from_slice(&after_space);
                    last_space_out_pos = None;
                } else {
                    output.push(b'\n');
                    col = 0;
                }
            } else {
                output.push(b'\n');
                col = 0;
            }
        }

        if break_at_spaces && (byte == b' ' || byte == b'\t') {
            last_space_out_pos = Some(output.len());
        }

        output.push(byte);
        col += char_width;
    }
}

/// Recalculate column position for a segment of output.
fn recalc_column(data: &[u8]) -> usize {
    let mut col = 0;
    for &b in data {
        if b == b'\t' {
            col = ((col / 8) + 1) * 8;
        } else if b == b'\x08' {
            if col > 0 {
                col -= 1;
            }
        } else if b >= 0x20 && b != 0x7f {
            col += 1;
        }
    }
    col
}
