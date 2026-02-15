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
    if data.is_empty() || width == 0 {
        // width 0: GNU fold outputs nothing per line but passes newlines
        if width == 0 {
            return fold_width_zero(data, out);
        }
        return Ok(());
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
    let mut output = Vec::with_capacity(data.len());
    for &_b in data {
        output.push(b'\n');
    }
    out.write_all(&output)
}

/// Fold by byte count (-b flag).
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
                    // Break at the last space: insert newline after the space
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

        if break_at_spaces && byte == b' ' {
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
            // Tab: advance to next tab stop (every 8)
            let next_stop = ((col / 8) + 1) * 8;
            next_stop - col
        } else if byte == b'\x08' {
            // Backspace: decrement column (can't go below 0)
            0 // handled specially below
        } else if byte < 0x20 || byte == 0x7f {
            // Control chars: 0 width in GNU fold
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
                    // Recalculate column for moved content
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

        if break_at_spaces && byte == b' ' {
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
