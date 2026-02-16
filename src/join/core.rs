use std::cmp::Ordering;
use std::io::{self, Write};

/// How to handle sort-order checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderCheck {
    Default,
    Strict,
    None,
}

/// An output field specification from -o format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSpec {
    /// Field 0: the join field
    JoinField,
    /// (file_index 0-based, field_index 0-based)
    FileField(usize, usize),
}

/// Configuration for the join command.
pub struct JoinConfig {
    /// Join field for file 1 (0-indexed)
    pub field1: usize,
    /// Join field for file 2 (0-indexed)
    pub field2: usize,
    /// Also print unpairable lines from file 1 (-a 1)
    pub print_unpaired1: bool,
    /// Also print unpairable lines from file 2 (-a 2)
    pub print_unpaired2: bool,
    /// Print ONLY unpairable lines from file 1 (-v 1)
    pub only_unpaired1: bool,
    /// Print ONLY unpairable lines from file 2 (-v 2)
    pub only_unpaired2: bool,
    /// Replace missing fields with this string (-e)
    pub empty_filler: Option<Vec<u8>>,
    /// Ignore case in key comparison (-i)
    pub case_insensitive: bool,
    /// Output format (-o)
    pub output_format: Option<Vec<OutputSpec>>,
    /// Auto output format (-o auto)
    pub auto_format: bool,
    /// Field separator (-t). None = whitespace mode.
    pub separator: Option<u8>,
    /// Order checking
    pub order_check: OrderCheck,
    /// Treat first line as header (--header)
    pub header: bool,
    /// Use NUL as line delimiter (-z)
    pub zero_terminated: bool,
}

impl Default for JoinConfig {
    fn default() -> Self {
        Self {
            field1: 0,
            field2: 0,
            print_unpaired1: false,
            print_unpaired2: false,
            only_unpaired1: false,
            only_unpaired2: false,
            empty_filler: None,
            case_insensitive: false,
            output_format: None,
            auto_format: false,
            separator: None,
            order_check: OrderCheck::Default,
            header: false,
            zero_terminated: false,
        }
    }
}

/// Split data into lines by delimiter using SIMD scanning.
fn split_lines<'a>(data: &'a [u8], delim: u8) -> Vec<&'a [u8]> {
    if data.is_empty() {
        return Vec::new();
    }
    let count = memchr::memchr_iter(delim, data).count();
    let has_trailing = data.last() == Some(&delim);
    let cap = if has_trailing { count } else { count + 1 };
    let mut lines = Vec::with_capacity(cap);
    let mut start = 0;
    for pos in memchr::memchr_iter(delim, data) {
        lines.push(&data[start..pos]);
        start = pos + 1;
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    lines
}

/// Split a line into fields by whitespace (runs of space/tab).
fn split_fields_whitespace<'a>(line: &'a [u8]) -> Vec<&'a [u8]> {
    let mut fields = Vec::new();
    let mut i = 0;
    let len = line.len();
    while i < len {
        // Skip whitespace
        while i < len && (line[i] == b' ' || line[i] == b'\t') {
            i += 1;
        }
        if i >= len {
            break;
        }
        let start = i;
        while i < len && line[i] != b' ' && line[i] != b'\t' {
            i += 1;
        }
        fields.push(&line[start..i]);
    }
    fields
}

/// Split a line into fields by exact single character.
fn split_fields_char<'a>(line: &'a [u8], sep: u8) -> Vec<&'a [u8]> {
    let count = memchr::memchr_iter(sep, line).count();
    let mut fields = Vec::with_capacity(count + 1);
    let mut start = 0;
    for pos in memchr::memchr_iter(sep, line) {
        fields.push(&line[start..pos]);
        start = pos + 1;
    }
    fields.push(&line[start..]);
    fields
}

/// Split a line into fields based on the separator setting.
#[inline]
fn split_fields<'a>(line: &'a [u8], separator: Option<u8>) -> Vec<&'a [u8]> {
    if let Some(sep) = separator {
        split_fields_char(line, sep)
    } else {
        split_fields_whitespace(line)
    }
}

/// Extract a single field from a line without allocating a Vec.
#[inline]
fn extract_field<'a>(line: &'a [u8], field_index: usize, separator: Option<u8>) -> &'a [u8] {
    if let Some(sep) = separator {
        let mut count = 0;
        let mut start = 0;
        for pos in memchr::memchr_iter(sep, line) {
            if count == field_index {
                return &line[start..pos];
            }
            count += 1;
            start = pos + 1;
        }
        if count == field_index {
            return &line[start..];
        }
        b""
    } else {
        let mut count = 0;
        let mut i = 0;
        let len = line.len();
        while i < len {
            while i < len && (line[i] == b' ' || line[i] == b'\t') {
                i += 1;
            }
            if i >= len {
                break;
            }
            let start = i;
            while i < len && line[i] != b' ' && line[i] != b'\t' {
                i += 1;
            }
            if count == field_index {
                return &line[start..i];
            }
            count += 1;
        }
        b""
    }
}

/// Compare two keys, optionally case-insensitive.
#[inline]
fn compare_keys(a: &[u8], b: &[u8], case_insensitive: bool) -> Ordering {
    if case_insensitive {
        for (&ca, &cb) in a.iter().zip(b.iter()) {
            match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                Ordering::Equal => continue,
                other => return other,
            }
        }
        a.len().cmp(&b.len())
    } else {
        a.cmp(b)
    }
}

/// Write a paired output line (default format: join_key + other fields).
fn write_paired_default(
    fields1: &[&[u8]],
    fields2: &[&[u8]],
    join_key: &[u8],
    field1: usize,
    field2: usize,
    out_sep: u8,
    delim: u8,
    buf: &mut Vec<u8>,
) {
    buf.extend_from_slice(join_key);
    for (i, f) in fields1.iter().enumerate() {
        if i == field1 {
            continue;
        }
        buf.push(out_sep);
        buf.extend_from_slice(f);
    }
    for (i, f) in fields2.iter().enumerate() {
        if i == field2 {
            continue;
        }
        buf.push(out_sep);
        buf.extend_from_slice(f);
    }
    buf.push(delim);
}

/// Write a paired output line with -o format.
fn write_paired_format(
    fields1: &[&[u8]],
    fields2: &[&[u8]],
    join_key: &[u8],
    specs: &[OutputSpec],
    empty: &[u8],
    out_sep: u8,
    delim: u8,
    buf: &mut Vec<u8>,
) {
    for (i, spec) in specs.iter().enumerate() {
        if i > 0 {
            buf.push(out_sep);
        }
        match spec {
            OutputSpec::JoinField => buf.extend_from_slice(join_key),
            OutputSpec::FileField(file_num, field_idx) => {
                let fields = if *file_num == 0 { fields1 } else { fields2 };
                if let Some(f) = fields.get(*field_idx) {
                    buf.extend_from_slice(f);
                } else {
                    buf.extend_from_slice(empty);
                }
            }
        }
    }
    buf.push(delim);
}

/// Write an unpaired output line (default format).
fn write_unpaired_default(
    fields: &[&[u8]],
    join_field: usize,
    out_sep: u8,
    delim: u8,
    buf: &mut Vec<u8>,
) {
    let key = fields.get(join_field).copied().unwrap_or(b"");
    buf.extend_from_slice(key);
    for (i, f) in fields.iter().enumerate() {
        if i == join_field {
            continue;
        }
        buf.push(out_sep);
        buf.extend_from_slice(f);
    }
    buf.push(delim);
}

/// Write an unpaired output line with -o format.
fn write_unpaired_format(
    fields: &[&[u8]],
    file_num: usize,
    join_field: usize,
    specs: &[OutputSpec],
    empty: &[u8],
    out_sep: u8,
    delim: u8,
    buf: &mut Vec<u8>,
) {
    let key = fields.get(join_field).copied().unwrap_or(b"");
    for (i, spec) in specs.iter().enumerate() {
        if i > 0 {
            buf.push(out_sep);
        }
        match spec {
            OutputSpec::JoinField => buf.extend_from_slice(key),
            OutputSpec::FileField(fnum, fidx) => {
                if *fnum == file_num {
                    if let Some(f) = fields.get(*fidx) {
                        buf.extend_from_slice(f);
                    } else {
                        buf.extend_from_slice(empty);
                    }
                } else {
                    buf.extend_from_slice(empty);
                }
            }
        }
    }
    buf.push(delim);
}

/// Run the join merge algorithm on two sorted inputs.
pub fn join(
    data1: &[u8],
    data2: &[u8],
    config: &JoinConfig,
    tool_name: &str,
    out: &mut impl Write,
) -> io::Result<bool> {
    let delim = if config.zero_terminated { b'\0' } else { b'\n' };
    let out_sep = config.separator.unwrap_or(b' ');
    let empty = config.empty_filler.as_deref().unwrap_or(b"");
    let ci = config.case_insensitive;

    let print_paired = !config.only_unpaired1 && !config.only_unpaired2;
    let show_unpaired1 = config.print_unpaired1 || config.only_unpaired1;
    let show_unpaired2 = config.print_unpaired2 || config.only_unpaired2;

    let lines1 = split_lines(data1, delim);
    let lines2 = split_lines(data2, delim);

    let mut i1 = 0usize;
    let mut i2 = 0usize;
    let mut had_order_error = false;
    let mut warned1 = false;
    let mut warned2 = false;

    let mut buf = Vec::with_capacity((data1.len() + data2.len()) / 2);

    // Handle -o auto: build format from first lines
    let auto_specs: Option<Vec<OutputSpec>> = if config.auto_format {
        let fc1 = if !lines1.is_empty() {
            split_fields(lines1[0], config.separator).len()
        } else {
            1
        };
        let fc2 = if !lines2.is_empty() {
            split_fields(lines2[0], config.separator).len()
        } else {
            1
        };
        let mut specs = Vec::new();
        specs.push(OutputSpec::JoinField);
        for i in 0..fc1 {
            if i != config.field1 {
                specs.push(OutputSpec::FileField(0, i));
            }
        }
        for i in 0..fc2 {
            if i != config.field2 {
                specs.push(OutputSpec::FileField(1, i));
            }
        }
        Some(specs)
    } else {
        None
    };

    let format = config.output_format.as_deref().or(auto_specs.as_deref());

    // Handle --header: join first lines without sort check
    if config.header && !lines1.is_empty() && !lines2.is_empty() {
        let fields1 = split_fields(lines1[0], config.separator);
        let fields2 = split_fields(lines2[0], config.separator);
        let key = fields1.get(config.field1).copied().unwrap_or(b"");

        if let Some(specs) = format {
            write_paired_format(
                &fields1, &fields2, key, specs, empty, out_sep, delim, &mut buf,
            );
        } else {
            write_paired_default(
                &fields1,
                &fields2,
                key,
                config.field1,
                config.field2,
                out_sep,
                delim,
                &mut buf,
            );
        }
        i1 = 1;
        i2 = 1;
    } else if config.header {
        // One or both files empty — skip header
        if !lines1.is_empty() {
            i1 = 1;
        }
        if !lines2.is_empty() {
            i2 = 1;
        }
    }

    while i1 < lines1.len() && i2 < lines2.len() {
        let key1 = extract_field(lines1[i1], config.field1, config.separator);
        let key2 = extract_field(lines2[i2], config.field2, config.separator);

        // Order checks
        if config.order_check != OrderCheck::None {
            if !warned1 && i1 > (if config.header { 1 } else { 0 }) {
                let prev_key = extract_field(lines1[i1 - 1], config.field1, config.separator);
                if compare_keys(key1, prev_key, ci) == Ordering::Less {
                    had_order_error = true;
                    warned1 = true;
                    eprintln!(
                        "{}: input:{}: is not sorted: {}",
                        tool_name,
                        i1 + 1,
                        String::from_utf8_lossy(lines1[i1])
                    );
                    if config.order_check == OrderCheck::Strict {
                        out.write_all(&buf)?;
                        return Ok(true);
                    }
                }
            }
            if !warned2 && i2 > (if config.header { 1 } else { 0 }) {
                let prev_key = extract_field(lines2[i2 - 1], config.field2, config.separator);
                if compare_keys(key2, prev_key, ci) == Ordering::Less {
                    had_order_error = true;
                    warned2 = true;
                    eprintln!(
                        "{}: input:{}: is not sorted: {}",
                        tool_name,
                        i2 + 1,
                        String::from_utf8_lossy(lines2[i2])
                    );
                    if config.order_check == OrderCheck::Strict {
                        out.write_all(&buf)?;
                        return Ok(true);
                    }
                }
            }
        }

        match compare_keys(key1, key2, ci) {
            Ordering::Less => {
                if show_unpaired1 {
                    let fields1 = split_fields(lines1[i1], config.separator);
                    if let Some(specs) = format {
                        write_unpaired_format(
                            &fields1,
                            0,
                            config.field1,
                            specs,
                            empty,
                            out_sep,
                            delim,
                            &mut buf,
                        );
                    } else {
                        write_unpaired_default(&fields1, config.field1, out_sep, delim, &mut buf);
                    }
                }
                i1 += 1;
            }
            Ordering::Greater => {
                if show_unpaired2 {
                    let fields2 = split_fields(lines2[i2], config.separator);
                    if let Some(specs) = format {
                        write_unpaired_format(
                            &fields2,
                            1,
                            config.field2,
                            specs,
                            empty,
                            out_sep,
                            delim,
                            &mut buf,
                        );
                    } else {
                        write_unpaired_default(&fields2, config.field2, out_sep, delim, &mut buf);
                    }
                }
                i2 += 1;
            }
            Ordering::Equal => {
                // Find all consecutive file2 lines with the same key
                let group_start = i2;
                let current_key = key2;
                i2 += 1;
                while i2 < lines2.len() {
                    let next_key = extract_field(lines2[i2], config.field2, config.separator);
                    if compare_keys(next_key, current_key, ci) != Ordering::Equal {
                        break;
                    }
                    i2 += 1;
                }

                // For each file1 line with the same key, cross-product with file2 group
                loop {
                    if print_paired {
                        let fields1 = split_fields(lines1[i1], config.separator);
                        let key = fields1.get(config.field1).copied().unwrap_or(b"");
                        for j in group_start..i2 {
                            let fields2 = split_fields(lines2[j], config.separator);
                            if let Some(specs) = format {
                                write_paired_format(
                                    &fields1, &fields2, key, specs, empty, out_sep, delim, &mut buf,
                                );
                            } else {
                                write_paired_default(
                                    &fields1,
                                    &fields2,
                                    key,
                                    config.field1,
                                    config.field2,
                                    out_sep,
                                    delim,
                                    &mut buf,
                                );
                            }
                        }
                    }
                    i1 += 1;
                    if i1 >= lines1.len() {
                        break;
                    }
                    let next_key = extract_field(lines1[i1], config.field1, config.separator);
                    let cmp = compare_keys(next_key, current_key, ci);
                    if cmp != Ordering::Equal {
                        // Check order: next_key should be > current_key
                        if config.order_check != OrderCheck::None
                            && !warned1
                            && cmp == Ordering::Less
                        {
                            had_order_error = true;
                            warned1 = true;
                            eprintln!(
                                "{}: input:{}: is not sorted: {}",
                                tool_name,
                                i1 + 1,
                                String::from_utf8_lossy(lines1[i1])
                            );
                            if config.order_check == OrderCheck::Strict {
                                out.write_all(&buf)?;
                                return Ok(true);
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    // Drain remaining from file 1
    while i1 < lines1.len() {
        if show_unpaired1 {
            let fields1 = split_fields(lines1[i1], config.separator);
            if let Some(specs) = format {
                write_unpaired_format(
                    &fields1,
                    0,
                    config.field1,
                    specs,
                    empty,
                    out_sep,
                    delim,
                    &mut buf,
                );
            } else {
                write_unpaired_default(&fields1, config.field1, out_sep, delim, &mut buf);
            }
        }
        i1 += 1;
    }

    // Drain remaining from file 2
    while i2 < lines2.len() {
        if show_unpaired2 {
            let fields2 = split_fields(lines2[i2], config.separator);
            if let Some(specs) = format {
                write_unpaired_format(
                    &fields2,
                    1,
                    config.field2,
                    specs,
                    empty,
                    out_sep,
                    delim,
                    &mut buf,
                );
            } else {
                write_unpaired_default(&fields2, config.field2, out_sep, delim, &mut buf);
            }
        }
        i2 += 1;
    }

    out.write_all(&buf)?;
    Ok(had_order_error)
}
