use std::io::Write;

/// Tab stop specification
#[derive(Clone, Debug)]
pub enum TabStops {
    /// Regular interval (default 8)
    Regular(usize),
    /// Explicit list of tab stop positions (0-indexed columns)
    List(Vec<usize>),
}

impl TabStops {
    /// Calculate the number of spaces to the next tab stop from the given column.
    #[inline]
    fn spaces_to_next(&self, column: usize) -> usize {
        match self {
            TabStops::Regular(n) => {
                if *n == 0 {
                    return 0;
                }
                *n - (column % *n)
            }
            TabStops::List(stops) => {
                // Find the first tab stop > current column
                match stops.binary_search(&(column + 1)) {
                    Ok(idx) => stops[idx] - column,
                    Err(idx) => {
                        if idx < stops.len() {
                            stops[idx] - column
                        } else {
                            // Past all tab stops: GNU uses 1 space
                            1
                        }
                    }
                }
            }
        }
    }

    /// Check if the given column is at a tab stop position.
    #[inline]
    fn is_tab_stop(&self, column: usize) -> bool {
        match self {
            TabStops::Regular(n) => {
                if *n == 0 {
                    return false;
                }
                column.is_multiple_of(*n)
            }
            TabStops::List(stops) => stops.binary_search(&column).is_ok(),
        }
    }

    /// Get the next tab stop position after the given column.
    #[inline]
    fn next_tab_stop(&self, column: usize) -> usize {
        column + self.spaces_to_next(column)
    }
}

/// Parse a tab specification string (e.g., "4", "4,8,12", "4 8 12").
pub fn parse_tab_stops(spec: &str) -> Result<TabStops, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(TabStops::Regular(8));
    }

    // Check if it's a single number (regular interval)
    if let Ok(n) = spec.parse::<usize>() {
        if n == 0 {
            return Err("tab size cannot be 0".to_string());
        }
        return Ok(TabStops::Regular(n));
    }

    // Parse as comma or space-separated list
    let mut stops: Vec<usize> = Vec::new();
    for part in spec.split([',', ' ']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // Handle / prefix for repeating tab stops
        if let Some(rest) = part.strip_prefix('/') {
            let n: usize = rest
                .parse()
                .map_err(|_| format!("'{}' is not a valid number", part))?;
            if n == 0 {
                return Err("tab size cannot be 0".to_string());
            }
            let last = stops.last().copied().unwrap_or(0);
            let mut pos = last + n;
            while pos < 10000 {
                stops.push(pos);
                pos += n;
            }
            continue;
        }
        match part.parse::<usize>() {
            Ok(n) => {
                if !stops.is_empty() && n <= *stops.last().unwrap() {
                    return Err("tab sizes must be ascending".to_string());
                }
                stops.push(n);
            }
            Err(_) => return Err(format!("'{}' is not a valid number", part)),
        }
    }

    if stops.is_empty() {
        return Err("tab specification is empty".to_string());
    }

    if stops.len() == 1 {
        return Ok(TabStops::Regular(stops[0]));
    }

    Ok(TabStops::List(stops))
}

// Pre-computed spaces buffer for fast tab expansion (avoids per-tab allocation)
const SPACES: [u8; 64] = [b' '; 64];

/// Write N spaces to a Vec efficiently using pre-computed buffer.
#[inline]
fn push_spaces(output: &mut Vec<u8>, n: usize) {
    let mut remaining = n;
    while remaining > 0 {
        let chunk = remaining.min(SPACES.len());
        output.extend_from_slice(&SPACES[..chunk]);
        remaining -= chunk;
    }
}

/// Write N spaces to a writer efficiently using pre-computed buffer.
#[inline]
fn write_spaces(out: &mut impl Write, n: usize) -> std::io::Result<()> {
    let mut remaining = n;
    while remaining > 0 {
        let chunk = remaining.min(SPACES.len());
        out.write_all(&SPACES[..chunk])?;
        remaining -= chunk;
    }
    Ok(())
}

/// Expand tabs to spaces using SIMD scanning.
/// Uses memchr2 to find tabs and newlines, bulk-copying everything between them.
pub fn expand_bytes(
    data: &[u8],
    tabs: &TabStops,
    initial_only: bool,
    out: &mut impl Write,
) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Fast path: no tabs in data → just copy through
    if memchr::memchr(b'\t', data).is_none() {
        return out.write_all(data);
    }

    // For regular tab stops with no -i flag, use the fast SIMD path
    if let TabStops::Regular(tab_size) = tabs {
        if !initial_only && memchr::memchr(b'\x08', data).is_none() {
            return expand_regular_fast(data, *tab_size, out);
        }
    }

    // Generic path for -i flag or tab lists
    expand_generic(data, tabs, initial_only, out)
}

/// Fast expand for regular tab stops without -i flag.
/// Streams directly to writer using memchr2 SIMD to find tabs and newlines.
/// Avoids allocating a large intermediate buffer.
fn expand_regular_fast(data: &[u8], tab_size: usize, out: &mut impl Write) -> std::io::Result<()> {
    let mut column: usize = 0;
    let mut pos: usize = 0;

    while pos < data.len() {
        match memchr::memchr2(b'\t', b'\n', &data[pos..]) {
            Some(offset) => {
                // Bulk write everything before the special byte
                if offset > 0 {
                    out.write_all(&data[pos..pos + offset])?;
                    column += offset;
                }
                let byte = data[pos + offset];
                pos += offset + 1;

                if byte == b'\n' {
                    out.write_all(b"\n")?;
                    column = 0;
                } else {
                    // Tab: write spaces directly to output
                    let spaces = tab_size - (column % tab_size);
                    write_spaces(out, spaces)?;
                    column += spaces;
                }
            }
            None => {
                out.write_all(&data[pos..])?;
                break;
            }
        }
    }

    Ok(())
}

/// Generic expand with support for -i flag and tab lists.
fn expand_generic(
    data: &[u8],
    tabs: &TabStops,
    initial_only: bool,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let mut output = Vec::with_capacity(data.len() + data.len() / 8);
    let mut column: usize = 0;
    let mut in_initial = true;

    for &byte in data {
        match byte {
            b'\t' => {
                if initial_only && !in_initial {
                    output.push(b'\t');
                    column = tabs.next_tab_stop(column);
                } else {
                    let spaces = tabs.spaces_to_next(column);
                    push_spaces(&mut output, spaces);
                    column += spaces;
                }
            }
            b'\n' => {
                output.push(b'\n');
                column = 0;
                in_initial = true;
            }
            b'\x08' => {
                output.push(b'\x08');
                if column > 0 {
                    column -= 1;
                }
            }
            _ => {
                if initial_only && in_initial && byte != b' ' {
                    in_initial = false;
                }
                output.push(byte);
                column += 1;
            }
        }
    }

    out.write_all(&output)
}

/// Unexpand spaces to tabs.
/// If `all` is true, convert all sequences of spaces; otherwise only leading spaces.
pub fn unexpand_bytes(
    data: &[u8],
    tabs: &TabStops,
    all: bool,
    out: &mut impl Write,
) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let mut output = Vec::with_capacity(data.len());
    let mut column: usize = 0;
    let mut space_start_col: Option<usize> = None;
    let mut in_initial = true;

    for &byte in data {
        match byte {
            b' ' => {
                if !all && !in_initial {
                    output.push(b' ');
                    column += 1;
                } else {
                    if space_start_col.is_none() {
                        space_start_col = Some(column);
                    }
                    column += 1;
                    if tabs.is_tab_stop(column) {
                        output.push(b'\t');
                        space_start_col = None;
                    }
                }
            }
            b'\t' => {
                space_start_col = None;
                output.push(b'\t');
                column = tabs.next_tab_stop(column);
            }
            b'\n' => {
                if let Some(start_col) = space_start_col.take() {
                    push_spaces(&mut output, column - start_col);
                }
                output.push(b'\n');
                column = 0;
                in_initial = true;
            }
            b'\x08' => {
                if let Some(start_col) = space_start_col.take() {
                    push_spaces(&mut output, column - start_col);
                }
                output.push(b'\x08');
                if column > 0 {
                    column -= 1;
                }
            }
            _ => {
                if let Some(start_col) = space_start_col.take() {
                    push_spaces(&mut output, column - start_col);
                }
                if in_initial {
                    in_initial = false;
                }
                output.push(byte);
                column += 1;
            }
        }
    }

    if let Some(start_col) = space_start_col {
        push_spaces(&mut output, column - start_col);
    }

    out.write_all(&output)
}
