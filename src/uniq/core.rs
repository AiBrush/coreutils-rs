use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

/// How to delimit groups when using --all-repeated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllRepeatedMethod {
    None,
    Prepend,
    Separate,
}

/// How to delimit groups when using --group
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMethod {
    Separate,
    Prepend,
    Append,
    Both,
}

/// Output mode for uniq
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Default: print unique lines and first of each duplicate group
    Default,
    /// -d: print only first line of duplicate groups
    RepeatedOnly,
    /// -D / --all-repeated: print ALL duplicate lines
    AllRepeated(AllRepeatedMethod),
    /// -u: print only lines that are NOT duplicated
    UniqueOnly,
    /// --group: show all items with group separators
    Group(GroupMethod),
}

/// Configuration for uniq processing
#[derive(Debug, Clone)]
pub struct UniqConfig {
    pub mode: OutputMode,
    pub count: bool,
    pub ignore_case: bool,
    pub skip_fields: usize,
    pub skip_chars: usize,
    pub check_chars: Option<usize>,
    pub zero_terminated: bool,
}

impl Default for UniqConfig {
    fn default() -> Self {
        Self {
            mode: OutputMode::Default,
            count: false,
            ignore_case: false,
            skip_fields: 0,
            skip_chars: 0,
            check_chars: None,
            zero_terminated: false,
        }
    }
}

/// Extract the comparison key from a line according to skip_fields, skip_chars, check_chars.
/// Matches GNU uniq field-skip semantics exactly: for each field, skip blanks then non-blanks.
#[inline(always)]
fn get_compare_slice<'a>(line: &'a [u8], config: &UniqConfig) -> &'a [u8] {
    let mut start = 0;
    let len = line.len();

    // Skip N fields (GNU: each field = run of blanks + run of non-blanks)
    for _ in 0..config.skip_fields {
        // Skip blanks (space and tab)
        while start < len && (line[start] == b' ' || line[start] == b'\t') {
            start += 1;
        }
        // Skip non-blanks (field content)
        while start < len && line[start] != b' ' && line[start] != b'\t' {
            start += 1;
        }
    }

    // Skip N characters
    if config.skip_chars > 0 {
        let remaining = len - start;
        let skip = config.skip_chars.min(remaining);
        start += skip;
    }

    let slice = &line[start..];

    // Limit comparison to N characters
    if let Some(w) = config.check_chars {
        if w < slice.len() {
            return &slice[..w];
        }
    }

    slice
}

/// Compare two lines (without terminators) using the config's comparison rules.
#[inline(always)]
fn lines_equal(a: &[u8], b: &[u8], config: &UniqConfig) -> bool {
    let sa = get_compare_slice(a, config);
    let sb = get_compare_slice(b, config);

    if config.ignore_case {
        sa.eq_ignore_ascii_case(sb)
    } else {
        sa == sb
    }
}

/// Check if config requires field/char skipping or char limiting.
#[inline(always)]
fn needs_key_extraction(config: &UniqConfig) -> bool {
    config.skip_fields > 0 || config.skip_chars > 0 || config.check_chars.is_some()
}

/// Fast path comparison: no field/char extraction needed, no case folding.
#[inline(always)]
fn lines_equal_fast(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// Write a count-prefixed line in GNU uniq format.
/// GNU format: "%7lu " — right-aligned in 7-char field, followed by space.
#[inline(always)]
fn write_count_line(out: &mut impl Write, count: u64, line: &[u8], term: u8) -> io::Result<()> {
    // Fast path for small counts (vast majority of cases)
    // Manually format to avoid write!() macro overhead
    let mut buf = [b' '; 20]; // Enough for u64 max
    let count_len = {
        let count_str = itoa_right_aligned(&mut buf, count);
        count_str.len()
    };
    let start = 20 - count_len;
    if count_len < 7 {
        // Pad with spaces to width 7
        let pad = 7 - count_len;
        out.write_all(&buf[..pad])?; // spaces
    }
    out.write_all(&buf[start..])?;
    out.write_all(b" ")?;
    out.write_all(line)?;
    out.write_all(&[term])?;
    Ok(())
}

/// Convert u64 to decimal string in a stack buffer, right-aligned.
/// Returns the slice of the buffer containing the decimal digits.
#[inline(always)]
fn itoa_right_aligned(buf: &mut [u8; 20], mut val: u64) -> &[u8] {
    if val == 0 {
        buf[19] = b'0';
        return &buf[19..20];
    }
    let mut pos = 20;
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    &buf[pos..20]
}

// ============================================================================
// High-performance mmap-based processing (for byte slices, zero-copy)
// ============================================================================

/// Process uniq from a byte slice (mmap'd file). Zero-copy, no per-line allocation.
pub fn process_uniq_bytes(data: &[u8], output: impl Write, config: &UniqConfig) -> io::Result<()> {
    let mut writer = BufWriter::with_capacity(4 * 1024 * 1024, output);
    let term = if config.zero_terminated { b'\0' } else { b'\n' };

    match config.mode {
        OutputMode::Group(method) => {
            process_group_bytes(data, &mut writer, config, method, term)?;
        }
        OutputMode::AllRepeated(method) => {
            process_all_repeated_bytes(data, &mut writer, config, method, term)?;
        }
        _ => {
            process_standard_bytes(data, &mut writer, config, term)?;
        }
    }

    writer.flush()?;
    Ok(())
}

/// Iterator over lines in a byte slice, yielding (line_without_terminator, has_terminator).
/// Uses memchr for SIMD-accelerated line boundary detection.
struct LineIter<'a> {
    data: &'a [u8],
    pos: usize,
    term: u8,
}

impl<'a> LineIter<'a> {
    #[inline(always)]
    fn new(data: &'a [u8], term: u8) -> Self {
        Self { data, pos: 0, term }
    }
}

impl<'a> Iterator for LineIter<'a> {
    /// (line content without terminator, full line including terminator for output)
    type Item = (&'a [u8], &'a [u8]);

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.data.len() {
            return None;
        }

        let remaining = &self.data[self.pos..];
        match memchr::memchr(self.term, remaining) {
            Some(idx) => {
                let line_start = self.pos;
                let line_end = self.pos + idx; // without terminator
                let full_end = self.pos + idx + 1; // with terminator
                self.pos = full_end;
                Some((
                    &self.data[line_start..line_end],
                    &self.data[line_start..full_end],
                ))
            }
            None => {
                // Last line without terminator
                let line_start = self.pos;
                self.pos = self.data.len();
                let line = &self.data[line_start..];
                Some((line, line))
            }
        }
    }
}

/// Batched output buffer size for uniq.
const UNIQ_BUF_SIZE: usize = 4 * 1024 * 1024;

/// Write a count-prefixed line into a Vec buffer (avoids per-line write_all).
#[inline(always)]
fn write_count_to_buf(buf: &mut Vec<u8>, count: u64, line: &[u8], term: u8) {
    let mut num_buf = [b' '; 20];
    let count_len = {
        let s = itoa_right_aligned(&mut num_buf, count);
        s.len()
    };
    let start = 20 - count_len;
    if count_len < 7 {
        // Pad with spaces to width 7
        let pad_bytes = &b"       "[..7 - count_len];
        buf.extend_from_slice(pad_bytes);
    }
    buf.extend_from_slice(&num_buf[start..]);
    buf.push(b' ');
    buf.extend_from_slice(line);
    buf.push(term);
}

/// Standard processing for Default, RepeatedOnly, UniqueOnly on byte slices.
fn process_standard_bytes(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let mut lines = LineIter::new(data, term);

    let (prev_content, prev_full) = match lines.next() {
        Some(v) => v,
        None => return Ok(()), // empty input
    };

    let fast = !needs_key_extraction(config) && !config.ignore_case;

    // Ultra-fast path: default mode, no count, no key extraction
    // Uses batched Vec output to avoid per-line write_all overhead
    if fast && !config.count && matches!(config.mode, OutputMode::Default) {
        let mut prev_content = prev_content;
        let mut out_buf = Vec::with_capacity(UNIQ_BUF_SIZE);
        out_buf.extend_from_slice(prev_full);
        if prev_full.len() == prev_content.len() {
            out_buf.push(term);
        }
        for (cur_content, cur_full) in lines {
            if !lines_equal_fast(prev_content, cur_content) {
                out_buf.extend_from_slice(cur_full);
                if cur_full.len() == cur_content.len() {
                    out_buf.push(term);
                }
                if out_buf.len() >= UNIQ_BUF_SIZE {
                    writer.write_all(&out_buf)?;
                    out_buf.clear();
                }
                prev_content = cur_content;
            }
        }
        if !out_buf.is_empty() {
            writer.write_all(&out_buf)?;
        }
        return Ok(());
    }

    // Fast batched path: default mode with count, no key extraction
    if fast && config.count && matches!(config.mode, OutputMode::Default) {
        let mut prev_content = prev_content;
        let mut prev_full = prev_full;
        let mut count: u64 = 1;
        let mut out_buf = Vec::with_capacity(UNIQ_BUF_SIZE);
        for (cur_content, cur_full) in lines {
            if lines_equal_fast(prev_content, cur_content) {
                count += 1;
            } else {
                write_count_to_buf(&mut out_buf, count, prev_content, term);
                if out_buf.len() >= UNIQ_BUF_SIZE {
                    writer.write_all(&out_buf)?;
                    out_buf.clear();
                }
                prev_content = cur_content;
                prev_full = cur_full;
                count = 1;
            }
        }
        write_count_to_buf(&mut out_buf, count, prev_content, term);
        if !out_buf.is_empty() {
            writer.write_all(&out_buf)?;
        }
        let _ = prev_full; // suppress unused warning
        return Ok(());
    }

    // Fast batched path: repeated-only or unique-only mode, no count, no key extraction
    if fast
        && !config.count
        && matches!(
            config.mode,
            OutputMode::RepeatedOnly | OutputMode::UniqueOnly
        )
    {
        let is_unique = matches!(config.mode, OutputMode::UniqueOnly);
        let mut prev_content = prev_content;
        let mut prev_full = prev_full;
        let mut count: u64 = 1;
        let mut out_buf = Vec::with_capacity(UNIQ_BUF_SIZE);
        for (cur_content, cur_full) in lines {
            if lines_equal_fast(prev_content, cur_content) {
                count += 1;
            } else {
                let emit = if is_unique { count == 1 } else { count > 1 };
                if emit {
                    out_buf.extend_from_slice(prev_full);
                    if prev_full.len() == prev_content.len() {
                        out_buf.push(term);
                    }
                    if out_buf.len() >= UNIQ_BUF_SIZE {
                        writer.write_all(&out_buf)?;
                        out_buf.clear();
                    }
                }
                prev_content = cur_content;
                prev_full = cur_full;
                count = 1;
            }
        }
        let emit = if is_unique { count == 1 } else { count > 1 };
        if emit {
            out_buf.extend_from_slice(prev_full);
            if prev_full.len() == prev_content.len() {
                out_buf.push(term);
            }
        }
        if !out_buf.is_empty() {
            writer.write_all(&out_buf)?;
        }
        return Ok(());
    }

    // General path with count tracking
    let mut prev_content = prev_content;
    let mut prev_full = prev_full;
    let mut count: u64 = 1;

    for (cur_content, cur_full) in lines {
        let equal = if fast {
            lines_equal_fast(prev_content, cur_content)
        } else {
            lines_equal(prev_content, cur_content, config)
        };

        if equal {
            count += 1;
        } else {
            // Output previous group
            output_group_bytes(writer, prev_content, prev_full, count, config, term)?;
            prev_content = cur_content;
            prev_full = cur_full;
            count = 1;
        }
    }

    // Output last group
    output_group_bytes(writer, prev_content, prev_full, count, config, term)?;
    Ok(())
}

/// Output a group for standard modes (bytes path).
#[inline(always)]
fn output_group_bytes(
    writer: &mut impl Write,
    content: &[u8],
    full: &[u8],
    count: u64,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let should_print = match config.mode {
        OutputMode::Default => true,
        OutputMode::RepeatedOnly => count > 1,
        OutputMode::UniqueOnly => count == 1,
        _ => true,
    };

    if should_print {
        if config.count {
            write_count_line(writer, count, content, term)?;
        } else {
            writer.write_all(full)?;
            // Add terminator if the original line didn't have one
            if full.len() == content.len() {
                writer.write_all(&[term])?;
            }
        }
    }

    Ok(())
}

/// Process --all-repeated / -D mode on byte slices.
fn process_all_repeated_bytes(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    method: AllRepeatedMethod,
    term: u8,
) -> io::Result<()> {
    let mut lines = LineIter::new(data, term);

    let first = match lines.next() {
        Some(v) => v,
        None => return Ok(()),
    };

    // Collect groups as (start_offset, line_count, first_line_content, lines_vec)
    // For all-repeated we need to buffer group lines since we only print if count > 1
    let mut group_lines: Vec<(&[u8], &[u8])> = Vec::with_capacity(64);
    group_lines.push(first);
    let mut first_group_printed = false;

    let fast = !needs_key_extraction(config) && !config.ignore_case;

    for (cur_content, cur_full) in lines {
        let prev_content = group_lines.last().unwrap().0;
        let equal = if fast {
            lines_equal_fast(prev_content, cur_content)
        } else {
            lines_equal(prev_content, cur_content, config)
        };

        if equal {
            group_lines.push((cur_content, cur_full));
        } else {
            // Flush group
            flush_all_repeated_bytes(writer, &group_lines, method, &mut first_group_printed, term)?;
            group_lines.clear();
            group_lines.push((cur_content, cur_full));
        }
    }

    // Flush last group
    flush_all_repeated_bytes(writer, &group_lines, method, &mut first_group_printed, term)?;

    Ok(())
}

/// Flush a group for --all-repeated mode (bytes path).
fn flush_all_repeated_bytes(
    writer: &mut impl Write,
    group: &[(&[u8], &[u8])],
    method: AllRepeatedMethod,
    first_group_printed: &mut bool,
    term: u8,
) -> io::Result<()> {
    if group.len() <= 1 {
        return Ok(()); // Not a duplicate group
    }

    match method {
        AllRepeatedMethod::Prepend => {
            writer.write_all(&[term])?;
        }
        AllRepeatedMethod::Separate => {
            if *first_group_printed {
                writer.write_all(&[term])?;
            }
        }
        AllRepeatedMethod::None => {}
    }

    for &(content, full) in group {
        writer.write_all(full)?;
        if full.len() == content.len() {
            writer.write_all(&[term])?;
        }
    }

    *first_group_printed = true;
    Ok(())
}

/// Process --group mode on byte slices.
fn process_group_bytes(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    method: GroupMethod,
    term: u8,
) -> io::Result<()> {
    let mut lines = LineIter::new(data, term);

    let (prev_content, prev_full) = match lines.next() {
        Some(v) => v,
        None => return Ok(()),
    };

    // Prepend/Both: separator before first group
    if matches!(method, GroupMethod::Prepend | GroupMethod::Both) {
        writer.write_all(&[term])?;
    }

    // Write first line
    writer.write_all(prev_full)?;
    if prev_full.len() == prev_content.len() {
        writer.write_all(&[term])?;
    }

    let mut prev_content = prev_content;
    let fast = !needs_key_extraction(config) && !config.ignore_case;

    for (cur_content, cur_full) in lines {
        let equal = if fast {
            lines_equal_fast(prev_content, cur_content)
        } else {
            lines_equal(prev_content, cur_content, config)
        };

        if !equal {
            // New group — write separator
            writer.write_all(&[term])?;
        }

        writer.write_all(cur_full)?;
        if cur_full.len() == cur_content.len() {
            writer.write_all(&[term])?;
        }

        prev_content = cur_content;
    }

    // Append/Both: separator after last group
    if matches!(method, GroupMethod::Append | GroupMethod::Both) {
        writer.write_all(&[term])?;
    }

    Ok(())
}

// ============================================================================
// Streaming processing (for stdin / pipe input)
// ============================================================================

/// Main streaming uniq processor.
/// Reads from `input`, writes to `output`.
pub fn process_uniq<R: Read, W: Write>(input: R, output: W, config: &UniqConfig) -> io::Result<()> {
    let reader = BufReader::with_capacity(4 * 1024 * 1024, input);
    let mut writer = BufWriter::with_capacity(4 * 1024 * 1024, output);
    let term = if config.zero_terminated { b'\0' } else { b'\n' };

    match config.mode {
        OutputMode::Group(method) => {
            process_group_stream(reader, &mut writer, config, method, term)?;
        }
        OutputMode::AllRepeated(method) => {
            process_all_repeated_stream(reader, &mut writer, config, method, term)?;
        }
        _ => {
            process_standard_stream(reader, &mut writer, config, term)?;
        }
    }

    writer.flush()?;
    Ok(())
}

/// Standard processing for Default, RepeatedOnly, UniqueOnly modes (streaming).
fn process_standard_stream<R: BufRead, W: Write>(
    mut reader: R,
    writer: &mut W,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let mut prev_line: Vec<u8> = Vec::with_capacity(4096);
    let mut current_line: Vec<u8> = Vec::with_capacity(4096);

    // Read first line
    if read_line_term(&mut reader, &mut prev_line, term)? == 0 {
        return Ok(()); // empty input
    }
    let mut count: u64 = 1;

    loop {
        current_line.clear();
        let bytes_read = read_line_term(&mut reader, &mut current_line, term)?;

        if bytes_read == 0 {
            // End of input — output the last group
            output_group_stream(writer, &prev_line, count, config, term)?;
            break;
        }

        if compare_lines_stream(&prev_line, &current_line, config, term) {
            count += 1;
        } else {
            output_group_stream(writer, &prev_line, count, config, term)?;
            std::mem::swap(&mut prev_line, &mut current_line);
            count = 1;
        }
    }

    Ok(())
}

/// Compare two lines (with terminators) in streaming mode.
#[inline(always)]
fn compare_lines_stream(a: &[u8], b: &[u8], config: &UniqConfig, term: u8) -> bool {
    let a_stripped = strip_term(a, term);
    let b_stripped = strip_term(b, term);
    lines_equal(a_stripped, b_stripped, config)
}

/// Strip terminator from end of line.
#[inline(always)]
fn strip_term(line: &[u8], term: u8) -> &[u8] {
    if line.last() == Some(&term) {
        &line[..line.len() - 1]
    } else {
        line
    }
}

/// Output a group in streaming mode.
#[inline(always)]
fn output_group_stream(
    writer: &mut impl Write,
    line: &[u8],
    count: u64,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let should_print = match config.mode {
        OutputMode::Default => true,
        OutputMode::RepeatedOnly => count > 1,
        OutputMode::UniqueOnly => count == 1,
        _ => true,
    };

    if should_print {
        let content = strip_term(line, term);
        if config.count {
            write_count_line(writer, count, content, term)?;
        } else {
            writer.write_all(content)?;
            writer.write_all(&[term])?;
        }
    }

    Ok(())
}

/// Process --all-repeated / -D mode (streaming).
fn process_all_repeated_stream<R: BufRead, W: Write>(
    mut reader: R,
    writer: &mut W,
    config: &UniqConfig,
    method: AllRepeatedMethod,
    term: u8,
) -> io::Result<()> {
    let mut group: Vec<Vec<u8>> = Vec::new();
    let mut current_line: Vec<u8> = Vec::with_capacity(4096);
    let mut first_group_printed = false;

    current_line.clear();
    if read_line_term(&mut reader, &mut current_line, term)? == 0 {
        return Ok(());
    }
    group.push(current_line.clone());

    loop {
        current_line.clear();
        let bytes_read = read_line_term(&mut reader, &mut current_line, term)?;

        if bytes_read == 0 {
            flush_all_repeated_stream(writer, &group, method, &mut first_group_printed, term)?;
            break;
        }

        if compare_lines_stream(group.last().unwrap(), &current_line, config, term) {
            group.push(current_line.clone());
        } else {
            flush_all_repeated_stream(writer, &group, method, &mut first_group_printed, term)?;
            group.clear();
            group.push(current_line.clone());
        }
    }

    Ok(())
}

/// Flush a group for --all-repeated mode (streaming).
fn flush_all_repeated_stream(
    writer: &mut impl Write,
    group: &[Vec<u8>],
    method: AllRepeatedMethod,
    first_group_printed: &mut bool,
    term: u8,
) -> io::Result<()> {
    if group.len() <= 1 {
        return Ok(());
    }

    match method {
        AllRepeatedMethod::Prepend => {
            writer.write_all(&[term])?;
        }
        AllRepeatedMethod::Separate => {
            if *first_group_printed {
                writer.write_all(&[term])?;
            }
        }
        AllRepeatedMethod::None => {}
    }

    for line in group {
        let content = strip_term(line, term);
        writer.write_all(content)?;
        writer.write_all(&[term])?;
    }

    *first_group_printed = true;
    Ok(())
}

/// Process --group mode (streaming).
fn process_group_stream<R: BufRead, W: Write>(
    mut reader: R,
    writer: &mut W,
    config: &UniqConfig,
    method: GroupMethod,
    term: u8,
) -> io::Result<()> {
    let mut prev_line: Vec<u8> = Vec::with_capacity(4096);
    let mut current_line: Vec<u8> = Vec::with_capacity(4096);

    if read_line_term(&mut reader, &mut prev_line, term)? == 0 {
        return Ok(());
    }

    // Prepend/Both: separator before first group
    if matches!(method, GroupMethod::Prepend | GroupMethod::Both) {
        writer.write_all(&[term])?;
    }

    let content = strip_term(&prev_line, term);
    writer.write_all(content)?;
    writer.write_all(&[term])?;

    loop {
        current_line.clear();
        let bytes_read = read_line_term(&mut reader, &mut current_line, term)?;

        if bytes_read == 0 {
            if matches!(method, GroupMethod::Append | GroupMethod::Both) {
                writer.write_all(&[term])?;
            }
            break;
        }

        if !compare_lines_stream(&prev_line, &current_line, config, term) {
            writer.write_all(&[term])?;
        }

        let content = strip_term(&current_line, term);
        writer.write_all(content)?;
        writer.write_all(&[term])?;

        std::mem::swap(&mut prev_line, &mut current_line);
    }

    Ok(())
}

/// Read a line terminated by the given byte (newline or NUL).
/// Returns number of bytes read (0 = EOF).
#[inline(always)]
fn read_line_term<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>, term: u8) -> io::Result<usize> {
    reader.read_until(term, buf)
}
