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
    // GNU supports both comma and space as separators
    for part in spec.split([',', ' ']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // Handle / prefix for repeating tab stops
        if let Some(rest) = part.strip_prefix('/') {
            let n: usize = rest.parse().map_err(|_| format!("'{}' is not a valid number", part))?;
            if n == 0 {
                return Err("tab size cannot be 0".to_string());
            }
            // This is a repeating interval from the last stop
            // GNU expand uses this as "every N after the last explicit stop"
            // For now, just generate stops up to a reasonable limit
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

/// Expand tabs to spaces.
/// If `initial_only` is true, only expand leading tabs (before any non-blank char).
pub fn expand_bytes(data: &[u8], tabs: &TabStops, initial_only: bool, out: &mut impl Write) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    // Output will be at least as large as input (tabs expand to spaces)
    let mut output = Vec::with_capacity(data.len() + data.len() / 8);
    let mut column: usize = 0;
    let mut in_initial = true; // tracking initial blanks on current line

    for &byte in data {
        match byte {
            b'\t' => {
                if initial_only && !in_initial {
                    // Past initial blanks: output tab literally
                    output.push(b'\t');
                    // Tab still advances column to next stop
                    column = tabs.next_tab_stop(column);
                } else {
                    let spaces = tabs.spaces_to_next(column);
                    output.extend(std::iter::repeat_n(b' ', spaces));
                    column += spaces;
                }
            }
            b'\n' => {
                output.push(b'\n');
                column = 0;
                in_initial = true;
            }
            b'\x08' => {
                // Backspace: GNU expand decrements column
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
pub fn unexpand_bytes(data: &[u8], tabs: &TabStops, all: bool, out: &mut impl Write) -> std::io::Result<()> {
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
                    // Only converting leading blanks, and we're past them
                    output.push(b' ');
                    column += 1;
                } else {
                    if space_start_col.is_none() {
                        space_start_col = Some(column);
                    }
                    column += 1;
                    // Check if we've reached a tab stop
                    if tabs.is_tab_stop(column) {
                        output.push(b'\t');
                        space_start_col = None;
                    }
                }
            }
            b'\t' => {
                // Tab: flush any pending spaces as a tab
                space_start_col = None;
                output.push(b'\t');
                column = tabs.next_tab_stop(column);
            }
            b'\n' => {
                // Flush pending spaces literally (don't convert trailing spaces to tabs)
                if let Some(start_col) = space_start_col.take() {
                    output.extend(std::iter::repeat_n(b' ', column - start_col));
                }
                output.push(b'\n');
                column = 0;
                in_initial = true;
            }
            b'\x08' => {
                // Backspace: flush pending spaces
                if let Some(start_col) = space_start_col.take() {
                    output.extend(std::iter::repeat_n(b' ', column - start_col));
                }
                output.push(b'\x08');
                if column > 0 {
                    column -= 1;
                }
            }
            _ => {
                // Flush pending spaces literally
                if let Some(start_col) = space_start_col.take() {
                    output.extend(std::iter::repeat_n(b' ', column - start_col));
                }
                if in_initial {
                    in_initial = false;
                }
                output.push(byte);
                column += 1;
            }
        }
    }

    // Flush trailing pending spaces
    if let Some(start_col) = space_start_col {
        output.extend(std::iter::repeat_n(b' ', column - start_col));
    }

    out.write_all(&output)
}
