/// Key definition parsing and field extraction for `sort -k`.
///
/// KEYDEF format: FIELD[.CHAR][OPTS],[FIELD[.CHAR][OPTS]]
/// Fields and characters are 1-indexed.

/// Per-key ordering options that can override global options.
#[derive(Debug, Clone, Default)]
pub struct KeyOpts {
    pub numeric: bool,
    pub general_numeric: bool,
    pub human_numeric: bool,
    pub month: bool,
    pub version: bool,
    pub random: bool,
    pub reverse: bool,
    pub ignore_leading_blanks: bool,
    pub dictionary_order: bool,
    pub ignore_case: bool,
    pub ignore_nonprinting: bool,
}

impl KeyOpts {
    /// Returns true if any sort-type option is set.
    pub fn has_sort_type(&self) -> bool {
        self.numeric
            || self.general_numeric
            || self.human_numeric
            || self.month
            || self.version
            || self.random
    }

    /// Returns true if any option at all is set (for key inheritance logic).
    pub fn has_any_option(&self) -> bool {
        self.has_sort_type()
            || self.ignore_case
            || self.dictionary_order
            || self.ignore_nonprinting
            || self.ignore_leading_blanks
            || self.reverse
    }

    /// Parse single-letter option flags from a string.
    pub fn parse_flags(&mut self, flags: &str) {
        for c in flags.chars() {
            match c {
                'b' => self.ignore_leading_blanks = true,
                'd' => self.dictionary_order = true,
                'f' => self.ignore_case = true,
                'g' => self.general_numeric = true,
                'h' => self.human_numeric = true,
                'i' => self.ignore_nonprinting = true,
                'M' => self.month = true,
                'n' => self.numeric = true,
                'R' => self.random = true,
                'r' => self.reverse = true,
                'V' => self.version = true,
                _ => {}
            }
        }
    }
}

/// A parsed key specification from `-k START[,END]`.
#[derive(Debug, Clone)]
pub struct KeyDef {
    pub start_field: usize,
    pub start_char: usize,
    pub end_field: usize,
    pub end_char: usize,
    pub opts: KeyOpts,
}

impl KeyDef {
    /// Parse a KEYDEF string like "2,2n" or "1.3,1.5" or "3,3rn".
    pub fn parse(spec: &str) -> Result<KeyDef, String> {
        let parts: Vec<&str> = spec.splitn(2, ',').collect();

        let (start_field, start_char, start_opts) = parse_field_spec(parts[0])?;

        let (end_field, end_char, end_opts) = if parts.len() > 1 {
            parse_field_spec(parts[1])?
        } else {
            (0, 0, String::new())
        };

        let mut opts = KeyOpts::default();
        opts.parse_flags(&start_opts);
        opts.parse_flags(&end_opts);

        if start_field == 0 {
            return Err("field number is zero: invalid field specification".to_string());
        }

        Ok(KeyDef {
            start_field,
            start_char,
            end_field,
            end_char,
            opts,
        })
    }
}

/// Parse a single field spec like "2" or "1.3" or "2n" or "1.3bf".
fn parse_field_spec(s: &str) -> Result<(usize, usize, String), String> {
    let mut field_str = String::new();
    let mut char_str = String::new();
    let mut opts = String::new();
    let mut in_char = false;

    for c in s.chars() {
        if c == '.' && !in_char && opts.is_empty() {
            in_char = true;
        } else if c.is_ascii_digit() && opts.is_empty() {
            if in_char {
                char_str.push(c);
            } else {
                field_str.push(c);
            }
        } else if c.is_ascii_alphabetic() {
            opts.push(c);
        } else {
            return Err(format!("invalid character '{}' in key spec", c));
        }
    }

    let field = if field_str.is_empty() {
        0
    } else {
        field_str
            .parse::<usize>()
            .map_err(|_| "invalid field number".to_string())?
    };

    let char_pos = if char_str.is_empty() {
        0
    } else {
        char_str
            .parse::<usize>()
            .map_err(|_| "invalid character position".to_string())?
    };

    Ok((field, char_pos, opts))
}

/// Find the byte range of the Nth field (0-indexed) in a line.
/// Returns (start, end) byte offsets. Allocation-free.
/// Uses SIMD memchr for separator-based field finding.
#[inline]
fn find_nth_field(line: &[u8], n: usize, separator: Option<u8>) -> (usize, usize) {
    match separator {
        Some(sep) => {
            // Use memchr_iter for SIMD-accelerated separator scanning
            let mut field = 0;
            let mut start = 0;
            for pos in memchr::memchr_iter(sep, line) {
                if field == n {
                    return (start, pos);
                }
                field += 1;
                start = pos + 1;
            }
            if field == n {
                (start, line.len())
            } else {
                (line.len(), line.len())
            }
        }
        None => {
            let mut field = 0;
            let mut i = 0;
            let len = line.len();

            while i < len {
                let field_start = i;
                // Skip blanks (part of this field)
                while i < len && is_blank(line[i]) {
                    i += 1;
                }
                // Consume non-blanks
                while i < len && !is_blank(line[i]) {
                    i += 1;
                }
                if field == n {
                    return (field_start, i);
                }
                field += 1;
            }

            (line.len(), line.len())
        }
    }
}

#[inline]
fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Extract the key portion of a line based on a KeyDef.
/// Allocation-free: uses find_nth_field instead of collecting all fields.
pub fn extract_key<'a>(line: &'a [u8], key: &KeyDef, separator: Option<u8>) -> &'a [u8] {
    let sf = key.start_field.saturating_sub(1);
    let (sf_start, sf_end) = find_nth_field(line, sf, separator);

    if sf_start >= line.len() {
        return b"";
    }

    let start_byte = if key.start_char > 0 {
        let field_len = sf_end - sf_start;
        let char_offset = (key.start_char - 1).min(field_len);
        sf_start + char_offset
    } else {
        sf_start
    };

    let end_byte = if key.end_field > 0 {
        let ef = key.end_field.saturating_sub(1);
        let (ef_start, ef_end) = find_nth_field(line, ef, separator);
        if key.end_char > 0 {
            let field_len = ef_end - ef_start;
            let char_offset = key.end_char.min(field_len);
            ef_start + char_offset
        } else {
            ef_end
        }
    } else {
        line.len()
    };

    if start_byte >= end_byte || start_byte >= line.len() {
        return b"";
    }

    &line[start_byte..end_byte.min(line.len())]
}
