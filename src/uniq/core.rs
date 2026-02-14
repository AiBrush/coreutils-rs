use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

/// Write a large contiguous buffer, retrying on partial writes.
#[inline]
fn write_all_raw(writer: &mut impl Write, buf: &[u8]) -> io::Result<()> {
    writer.write_all(buf)
}

/// Write all IoSlices to the writer, handling partial writes correctly.
fn write_all_vectored(writer: &mut impl Write, slices: &[io::IoSlice<'_>]) -> io::Result<()> {
    let n = writer.write_vectored(slices)?;
    let expected: usize = slices.iter().map(|s| s.len()).sum();
    if n >= expected {
        return Ok(());
    }
    if n == 0 && expected > 0 {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "write_vectored returned 0",
        ));
    }
    // Slow path: partial write — fall back to write_all per remaining slice.
    let mut consumed = n;
    for slice in slices {
        if consumed == 0 {
            writer.write_all(slice)?;
        } else if consumed >= slice.len() {
            consumed -= slice.len();
        } else {
            writer.write_all(&slice[consumed..])?;
            consumed = 0;
        }
    }
    Ok(())
}

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

/// Fast case-insensitive comparison: no field/char extraction, just case-insensitive.
/// Uses length check + 8-byte prefix rejection before full comparison.
#[inline(always)]
fn lines_equal_case_insensitive(a: &[u8], b: &[u8]) -> bool {
    let alen = a.len();
    if alen != b.len() {
        return false;
    }
    if alen == 0 {
        return true;
    }
    a.eq_ignore_ascii_case(b)
}

/// Check if config requires field/char skipping or char limiting.
#[inline(always)]
fn needs_key_extraction(config: &UniqConfig) -> bool {
    config.skip_fields > 0 || config.skip_chars > 0 || config.check_chars.is_some()
}

/// Fast path comparison: no field/char extraction needed, no case folding.
/// Uses pointer+length equality shortcut and multi-word prefix rejection.
/// For short lines (<= 32 bytes, common in many-dups data), avoids the
/// full memcmp call overhead by doing direct word comparisons.
/// For medium lines (33-256 bytes), uses a tight u64 loop covering the
/// full line without falling through to memcmp.
#[inline(always)]
fn lines_equal_fast(a: &[u8], b: &[u8]) -> bool {
    let alen = a.len();
    if alen != b.len() {
        return false;
    }
    if alen == 0 {
        return true;
    }
    // Short-line fast path: compare via word loads to avoid memcmp call overhead
    if alen <= 8 {
        // For < 8 bytes: byte-by-byte via slice (compiler vectorizes this)
        return a == b;
    }
    unsafe {
        let ap = a.as_ptr();
        let bp = b.as_ptr();
        // 8-byte prefix check: reject most non-equal lines without full memcmp
        let a8 = (ap as *const u64).read_unaligned();
        let b8 = (bp as *const u64).read_unaligned();
        if a8 != b8 {
            return false;
        }
        // Check last 8 bytes (overlapping for 9-16 byte lines, eliminating full memcmp)
        if alen <= 16 {
            let a_tail = (ap.add(alen - 8) as *const u64).read_unaligned();
            let b_tail = (bp.add(alen - 8) as *const u64).read_unaligned();
            return a_tail == b_tail;
        }
        // For 17-32 bytes: check first 16 + last 16 (overlapping) to avoid memcmp
        if alen <= 32 {
            let a16 = (ap.add(8) as *const u64).read_unaligned();
            let b16 = (bp.add(8) as *const u64).read_unaligned();
            if a16 != b16 {
                return false;
            }
            let a_tail = (ap.add(alen - 8) as *const u64).read_unaligned();
            let b_tail = (bp.add(alen - 8) as *const u64).read_unaligned();
            return a_tail == b_tail;
        }
        // For 33-256 bytes: tight u64 loop covering the full line.
        // Compare 32 bytes per iteration (4 u64 loads), then handle tail.
        // This avoids the function call overhead of memcmp for medium lines.
        if alen <= 256 {
            let mut off = 8usize; // first 8 bytes already compared
            // Compare 32 bytes at a time
            while off + 32 <= alen {
                let a0 = (ap.add(off) as *const u64).read_unaligned();
                let b0 = (bp.add(off) as *const u64).read_unaligned();
                let a1 = (ap.add(off + 8) as *const u64).read_unaligned();
                let b1 = (bp.add(off + 8) as *const u64).read_unaligned();
                let a2 = (ap.add(off + 16) as *const u64).read_unaligned();
                let b2 = (bp.add(off + 16) as *const u64).read_unaligned();
                let a3 = (ap.add(off + 24) as *const u64).read_unaligned();
                let b3 = (bp.add(off + 24) as *const u64).read_unaligned();
                // XOR all pairs and OR together: zero if all equal
                if (a0 ^ b0) | (a1 ^ b1) | (a2 ^ b2) | (a3 ^ b3) != 0 {
                    return false;
                }
                off += 32;
            }
            // Compare remaining 8 bytes at a time
            while off + 8 <= alen {
                let aw = (ap.add(off) as *const u64).read_unaligned();
                let bw = (bp.add(off) as *const u64).read_unaligned();
                if aw != bw {
                    return false;
                }
                off += 8;
            }
            // Compare tail (overlapping last 8 bytes)
            if off < alen {
                let a_tail = (ap.add(alen - 8) as *const u64).read_unaligned();
                let b_tail = (bp.add(alen - 8) as *const u64).read_unaligned();
                return a_tail == b_tail;
            }
            return true;
        }
    }
    // Longer lines (>256): prefix passed, fall through to full memcmp
    a == b
}

/// Write a count-prefixed line in GNU uniq format.
/// GNU format: "%7lu " — right-aligned in 7-char field, followed by space.
/// Combines prefix + line + term into a single write for short lines (< 240 bytes).
#[inline(always)]
fn write_count_line(out: &mut impl Write, count: u64, line: &[u8], term: u8) -> io::Result<()> {
    // Build prefix "     N " in a stack buffer (max 21 bytes for u64 + spaces)
    let mut prefix = [b' '; 28]; // Enough for u64 max + padding + space
    let digits = itoa_right_aligned_into(&mut prefix, count);
    let width = digits.max(7); // minimum 7 chars
    let prefix_len = width + 1; // +1 for trailing space
    prefix[width] = b' ';

    // Single write for short lines (common case) — avoids 3 separate BufWriter calls
    let total = prefix_len + line.len() + 1;
    if total <= 256 {
        let mut buf = [0u8; 256];
        buf[..prefix_len].copy_from_slice(&prefix[..prefix_len]);
        buf[prefix_len..prefix_len + line.len()].copy_from_slice(line);
        buf[prefix_len + line.len()] = term;
        out.write_all(&buf[..total])
    } else {
        out.write_all(&prefix[..prefix_len])?;
        out.write_all(line)?;
        out.write_all(&[term])
    }
}

/// Write u64 decimal right-aligned into prefix buffer.
/// Buffer is pre-filled with spaces. Returns number of digits written.
#[inline(always)]
fn itoa_right_aligned_into(buf: &mut [u8; 28], mut val: u64) -> usize {
    if val == 0 {
        buf[6] = b'0';
        return 7; // 6 spaces + '0' = 7 chars
    }
    // Write digits right-to-left from position 27 (leaving room for trailing space)
    let mut pos = 27;
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    let num_digits = 27 - pos;
    if num_digits >= 7 {
        // Number is wide enough, shift to front
        buf.copy_within(pos..27, 0);
        num_digits
    } else {
        // Right-align in 7-char field: spaces then digits
        let pad = 7 - num_digits;
        buf.copy_within(pos..27, pad);
        // buf[0..pad] is already spaces from initialization
        7
    }
}

// ============================================================================
// High-performance mmap-based processing (for byte slices, zero-copy)
// ============================================================================

/// Process uniq from a byte slice (mmap'd file). Zero-copy, no per-line allocation.
pub fn process_uniq_bytes(data: &[u8], output: impl Write, config: &UniqConfig) -> io::Result<()> {
    // 16MB output buffer — optimal for L3 cache utilization on modern CPUs.
    // 32MB can cause cache thrashing; 16MB stays within L3 while still
    // reducing write() syscall count significantly.
    let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, output);
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

/// Get line content (without terminator) from pre-computed positions.
/// `content_end` is the end of actual content (excludes trailing terminator if present).
#[inline(always)]
fn line_content_at<'a>(
    data: &'a [u8],
    line_starts: &[usize],
    idx: usize,
    content_end: usize,
) -> &'a [u8] {
    let start = line_starts[idx];
    let end = if idx + 1 < line_starts.len() {
        line_starts[idx + 1] - 1 // exclude terminator
    } else {
        content_end // last line: pre-computed to exclude trailing terminator
    };
    &data[start..end]
}

/// Get full line (with terminator) from pre-computed positions.
#[inline(always)]
fn line_full_at<'a>(data: &'a [u8], line_starts: &[usize], idx: usize) -> &'a [u8] {
    let start = line_starts[idx];
    let end = if idx + 1 < line_starts.len() {
        line_starts[idx + 1] // include terminator
    } else {
        data.len()
    };
    &data[start..end]
}

/// Linear scan for the end of a duplicate group.
/// Returns the index of the first line that differs from line_starts[group_start].
/// Must use linear scan (not binary search) because uniq input may NOT be sorted --
/// equal lines can appear in non-adjacent groups separated by different lines.
/// Caches key length for fast length-mismatch rejection.
#[inline]
fn linear_scan_group_end(
    data: &[u8],
    line_starts: &[usize],
    group_start: usize,
    num_lines: usize,
    content_end: usize,
) -> usize {
    let key = line_content_at(data, line_starts, group_start, content_end);
    let key_len = key.len();
    let mut i = group_start + 1;
    while i < num_lines {
        let candidate = line_content_at(data, line_starts, i, content_end);
        if candidate.len() != key_len || !lines_equal_fast(key, candidate) {
            return i;
        }
        i += 1;
    }
    i
}

/// Standard processing for Default, RepeatedOnly, UniqueOnly on byte slices.
/// Ultra-fast path: single-pass inline scanning with memchr, no line_starts Vec.
/// General path: pre-computed line positions with binary search for groups.
fn process_standard_bytes(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let fast = !needs_key_extraction(config) && !config.ignore_case;
    let fast_ci = !needs_key_extraction(config) && config.ignore_case;

    // Ultra-fast path: default mode, no count, no key extraction.
    // Single-pass: scan with memchr, compare adjacent lines inline.
    // Avoids the 20MB+ line_starts allocation + cache misses from random access.
    if fast && !config.count && matches!(config.mode, OutputMode::Default) {
        return process_default_fast_singlepass(data, writer, term);
    }

    // Ultra-fast path: repeated-only or unique-only, no count, no key extraction
    if fast
        && !config.count
        && matches!(
            config.mode,
            OutputMode::RepeatedOnly | OutputMode::UniqueOnly
        )
    {
        return process_filter_fast_singlepass(data, writer, config, term);
    }

    // Ultra-fast path: count mode with no key extraction.
    // Single-pass: scan with memchr, count groups inline, emit count-prefixed lines.
    // Avoids the line_starts Vec allocation (20MB+ for large files).
    if fast && config.count {
        return process_count_fast_singlepass(data, writer, config, term);
    }

    // Fast path for case-insensitive (-i) mode with no key extraction.
    // Single-pass: scan with memchr, compare adjacent lines with eq_ignore_ascii_case.
    // Avoids the general path's line_starts Vec allocation.
    if fast_ci && !config.count && matches!(config.mode, OutputMode::Default) {
        return process_default_ci_singlepass(data, writer, term);
    }

    if fast_ci
        && !config.count
        && matches!(
            config.mode,
            OutputMode::RepeatedOnly | OutputMode::UniqueOnly
        )
    {
        return process_filter_ci_singlepass(data, writer, config, term);
    }

    if fast_ci && config.count {
        return process_count_ci_singlepass(data, writer, config, term);
    }

    // General path: pre-computed line positions for binary search on groups
    let estimated_lines = (data.len() / 40).max(64);
    let mut line_starts: Vec<usize> = Vec::with_capacity(estimated_lines);
    line_starts.push(0);
    for pos in memchr::memchr_iter(term, data) {
        if pos + 1 < data.len() {
            line_starts.push(pos + 1);
        }
    }
    let num_lines = line_starts.len();
    if num_lines == 0 {
        return Ok(());
    }

    // Pre-compute content end: if data ends with terminator, exclude it for last line
    let content_end = if data.last() == Some(&term) {
        data.len() - 1
    } else {
        data.len()
    };

    // Ultra-fast path: default mode, no count, no key extraction
    if fast && !config.count && matches!(config.mode, OutputMode::Default) {
        // Write first line
        let first_full = line_full_at(data, &line_starts, 0);
        let first_content = line_content_at(data, &line_starts, 0, content_end);
        write_all_raw(writer, first_full)?;
        if first_full.len() == first_content.len() {
            writer.write_all(&[term])?;
        }

        let mut i = 1;
        while i < num_lines {
            let prev = line_content_at(data, &line_starts, i - 1, content_end);
            let cur = line_content_at(data, &line_starts, i, content_end);

            if lines_equal_fast(prev, cur) {
                // Duplicate detected — linear scan for end of group
                let group_end =
                    linear_scan_group_end(data, &line_starts, i - 1, num_lines, content_end);
                i = group_end;
                continue;
            }

            // Unique line — write it
            let cur_full = line_full_at(data, &line_starts, i);
            write_all_raw(writer, cur_full)?;
            if cur_full.len() == cur.len() {
                writer.write_all(&[term])?;
            }
            i += 1;
        }
        return Ok(());
    }

    // General path with count tracking
    let mut i = 0;
    while i < num_lines {
        let content = line_content_at(data, &line_starts, i, content_end);
        let full = line_full_at(data, &line_starts, i);

        let group_end = if fast
            && i + 1 < num_lines
            && lines_equal_fast(
                content,
                line_content_at(data, &line_starts, i + 1, content_end),
            ) {
            // Duplicate detected — linear scan for end
            linear_scan_group_end(data, &line_starts, i, num_lines, content_end)
        } else if !fast
            && i + 1 < num_lines
            && lines_equal(
                content,
                line_content_at(data, &line_starts, i + 1, content_end),
                config,
            )
        {
            // Slow path linear scan with key extraction
            let mut j = i + 2;
            while j < num_lines {
                if !lines_equal(
                    content,
                    line_content_at(data, &line_starts, j, content_end),
                    config,
                ) {
                    break;
                }
                j += 1;
            }
            j
        } else {
            i + 1
        };

        let count = (group_end - i) as u64;
        output_group_bytes(writer, content, full, count, config, term)?;
        i = group_end;
    }

    Ok(())
}

/// Ultra-fast single-pass default mode: scan with memchr, compare adjacent lines inline.
/// No pre-computed positions, no binary search, no Vec allocation.
/// Outputs each line that differs from the previous.
///
/// For large files (>4MB), uses parallel chunk processing: each chunk is deduplicated
/// independently, then cross-chunk boundaries are resolved.
fn process_default_fast_singlepass(
    data: &[u8],
    writer: &mut impl Write,
    term: u8,
) -> io::Result<()> {
    // Parallel path for large files — kick in at 4MB.
    // Lower thresholds (e.g. 2MB) hurt performance on 10MB files because
    // the parallel overhead dominates for smaller chunks.
    if data.len() >= 4 * 1024 * 1024 {
        return process_default_parallel(data, writer, term);
    }

    process_default_sequential(data, writer, term)
}

/// Sequential single-pass dedup with zero-copy output.
/// Instead of copying data to a buffer, tracks contiguous output runs and writes
/// directly from the original data. For all-unique data, this is a single write_all.
///
/// Optimized for the "many duplicates" case: caches the previous line's length
/// and first-8-byte prefix for fast rejection of non-duplicates without
/// calling the full comparison function.
///
/// Uses raw pointer arithmetic throughout to avoid bounds checking in the hot loop.
fn process_default_sequential(data: &[u8], writer: &mut impl Write, term: u8) -> io::Result<()> {
    let data_len = data.len();
    let base = data.as_ptr();
    let mut prev_start: usize = 0;

    // Find end of first line
    let first_end: usize = match memchr::memchr(term, data) {
        Some(pos) => pos,
        None => {
            // Single line, no terminator
            writer.write_all(data)?;
            return writer.write_all(&[term]);
        }
    };

    // Cache previous line metadata for fast comparison
    let mut prev_len = first_end - prev_start;
    let mut prev_prefix: u64 = if prev_len >= 8 {
        unsafe { (base.add(prev_start) as *const u64).read_unaligned() }
    } else {
        0
    };

    // run_start tracks the beginning of the current contiguous output region.
    // When a duplicate is found, we flush the run up to the duplicate and skip it.
    let mut run_start: usize = 0;
    let mut cur_start = first_end + 1;
    let mut last_output_end = first_end + 1; // exclusive end including terminator

    while cur_start < data_len {
        // Speculative line-end detection: if the previous line had length L,
        // check if data[cur_start + L] is the terminator. This avoids the
        // memchr SIMD call for repetitive data where all lines have the same length.
        // Falls back to memchr if the speculation is wrong.
        let cur_end = {
            let speculative = cur_start + prev_len;
            if speculative < data_len && unsafe { *base.add(speculative) } == term {
                speculative
            } else {
                match memchr::memchr(term, unsafe {
                    std::slice::from_raw_parts(base.add(cur_start), data_len - cur_start)
                }) {
                    Some(offset) => cur_start + offset,
                    None => data_len,
                }
            }
        };

        let cur_len = cur_end - cur_start;

        // Fast reject: if lengths differ, lines are definitely not equal.
        // This branch structure is ordered by frequency: length mismatch is
        // most common for unique data, prefix mismatch next, full compare last.
        let is_dup = if cur_len != prev_len {
            false
        } else if cur_len == 0 {
            true
        } else if cur_len >= 8 {
            // Compare cached 8-byte prefix first
            let cur_prefix = unsafe { (base.add(cur_start) as *const u64).read_unaligned() };
            if cur_prefix != prev_prefix {
                false
            } else if cur_len <= 8 {
                true // prefix covers entire line
            } else if cur_len <= 16 {
                // Check last 8 bytes (overlapping)
                unsafe {
                    let a_tail =
                        (base.add(prev_start + prev_len - 8) as *const u64).read_unaligned();
                    let b_tail = (base.add(cur_start + cur_len - 8) as *const u64).read_unaligned();
                    a_tail == b_tail
                }
            } else if cur_len <= 32 {
                // Check bytes 8-16 and last 8 bytes
                unsafe {
                    let a16 = (base.add(prev_start + 8) as *const u64).read_unaligned();
                    let b16 = (base.add(cur_start + 8) as *const u64).read_unaligned();
                    if a16 != b16 {
                        false
                    } else {
                        let a_tail =
                            (base.add(prev_start + prev_len - 8) as *const u64).read_unaligned();
                        let b_tail =
                            (base.add(cur_start + cur_len - 8) as *const u64).read_unaligned();
                        a_tail == b_tail
                    }
                }
            } else if cur_len <= 256 {
                // 33-256 bytes: tight u64 loop with XOR-OR batching.
                // Compares 32 bytes per iteration (4 u64 loads), reducing
                // branch mispredictions vs individual comparisons.
                unsafe {
                    let ap = base.add(prev_start);
                    let bp = base.add(cur_start);
                    let mut off = 8usize; // first 8 bytes already compared via prefix
                    let mut eq = true;
                    while off + 32 <= cur_len {
                        let a0 = (ap.add(off) as *const u64).read_unaligned();
                        let b0 = (bp.add(off) as *const u64).read_unaligned();
                        let a1 = (ap.add(off + 8) as *const u64).read_unaligned();
                        let b1 = (bp.add(off + 8) as *const u64).read_unaligned();
                        let a2 = (ap.add(off + 16) as *const u64).read_unaligned();
                        let b2 = (bp.add(off + 16) as *const u64).read_unaligned();
                        let a3 = (ap.add(off + 24) as *const u64).read_unaligned();
                        let b3 = (bp.add(off + 24) as *const u64).read_unaligned();
                        if (a0 ^ b0) | (a1 ^ b1) | (a2 ^ b2) | (a3 ^ b3) != 0 {
                            eq = false;
                            break;
                        }
                        off += 32;
                    }
                    if eq {
                        while off + 8 <= cur_len {
                            let aw = (ap.add(off) as *const u64).read_unaligned();
                            let bw = (bp.add(off) as *const u64).read_unaligned();
                            if aw != bw {
                                eq = false;
                                break;
                            }
                            off += 8;
                        }
                    }
                    if eq && off < cur_len {
                        let a_tail = (ap.add(cur_len - 8) as *const u64).read_unaligned();
                        let b_tail = (bp.add(cur_len - 8) as *const u64).read_unaligned();
                        eq = a_tail == b_tail;
                    }
                    eq
                }
            } else {
                // For longer lines (>256), use unsafe slice comparison
                unsafe {
                    let a = std::slice::from_raw_parts(base.add(prev_start), prev_len);
                    let b = std::slice::from_raw_parts(base.add(cur_start), cur_len);
                    a == b
                }
            }
        } else {
            // Short line < 8 bytes — direct byte comparison
            unsafe {
                let a = std::slice::from_raw_parts(base.add(prev_start), prev_len);
                let b = std::slice::from_raw_parts(base.add(cur_start), cur_len);
                a == b
            }
        };

        if is_dup {
            // Duplicate — flush the current run up to this line, then skip it
            if run_start < cur_start {
                writer.write_all(&data[run_start..cur_start])?;
            }
            // Start new run after this duplicate
            run_start = if cur_end < data_len {
                cur_end + 1
            } else {
                cur_end
            };
        } else {
            // Different line — update cached comparison state
            prev_start = cur_start;
            prev_len = cur_len;
            prev_prefix = if cur_len >= 8 {
                unsafe { (base.add(cur_start) as *const u64).read_unaligned() }
            } else {
                0
            };
            last_output_end = if cur_end < data_len {
                cur_end + 1
            } else {
                cur_end
            };
        }

        if cur_end < data_len {
            cur_start = cur_end + 1;
        } else {
            break;
        }
    }

    // Flush remaining run
    if run_start < data_len {
        writer.write_all(&data[run_start..last_output_end.max(run_start)])?;
    }

    // Ensure trailing terminator
    if data_len > 0 && unsafe { *base.add(data_len - 1) } != term {
        writer.write_all(&[term])?;
    }

    Ok(())
}

/// Parallel zero-copy dedup for large files: split into chunks, find duplicate
/// positions in each chunk in parallel, then write output runs directly from
/// the original data. No per-chunk buffer allocation needed.
fn process_default_parallel(data: &[u8], writer: &mut impl Write, term: u8) -> io::Result<()> {
    use rayon::prelude::*;

    let num_threads = rayon::current_num_threads().max(1);
    let chunk_target = data.len() / num_threads;

    // Find chunk boundaries aligned to line terminators
    let mut boundaries = Vec::with_capacity(num_threads + 1);
    boundaries.push(0usize);
    for i in 1..num_threads {
        let target = i * chunk_target;
        if target >= data.len() {
            break;
        }
        if let Some(p) = memchr::memchr(term, &data[target..]) {
            let b = target + p + 1;
            if b > *boundaries.last().unwrap() && b <= data.len() {
                boundaries.push(b);
            }
        }
    }
    boundaries.push(data.len());

    let n_chunks = boundaries.len() - 1;
    if n_chunks <= 1 {
        return process_default_sequential(data, writer, term);
    }

    // Each chunk produces: output runs (zero-copy refs to data) + first/last line info
    struct ChunkResult {
        /// Byte ranges in the original data to output (contiguous runs)
        runs: Vec<(usize, usize)>,
        /// First line in chunk (absolute offsets into data, content without term)
        first_line_start: usize,
        first_line_end: usize,
        /// Last *output* line in chunk (content without term)
        last_line_start: usize,
        last_line_end: usize,
    }

    let results: Vec<ChunkResult> = boundaries
        .windows(2)
        .collect::<Vec<_>>()
        .par_iter()
        .map(|w| {
            let chunk_start = w[0];
            let chunk_end = w[1];
            let chunk = &data[chunk_start..chunk_end];

            let first_term = match memchr::memchr(term, chunk) {
                Some(pos) => pos,
                None => {
                    return ChunkResult {
                        runs: vec![(chunk_start, chunk_end)],
                        first_line_start: chunk_start,
                        first_line_end: chunk_end,
                        last_line_start: chunk_start,
                        last_line_end: chunk_end,
                    };
                }
            };

            let first_line_start = chunk_start;
            let first_line_end = chunk_start + first_term;

            let mut runs: Vec<(usize, usize)> = Vec::new();
            let mut run_start = chunk_start;
            let mut prev_start = 0usize;
            let mut prev_end = first_term;
            let mut last_out_start = chunk_start;
            let mut last_out_end = first_line_end;

            let mut prev_len = first_term;
            let chunk_base = chunk.as_ptr();
            let chunk_len = chunk.len();
            let mut cur_start = first_term + 1;
            while cur_start < chunk_len {
                // Speculative line-end: check if next line has same length
                let cur_end = {
                    let spec = cur_start + prev_len;
                    if spec < chunk_len && unsafe { *chunk_base.add(spec) } == term {
                        spec
                    } else {
                        match memchr::memchr(term, &chunk[cur_start..]) {
                            Some(offset) => cur_start + offset,
                            None => chunk_len,
                        }
                    }
                };

                if lines_equal_fast(&chunk[prev_start..prev_end], &chunk[cur_start..cur_end]) {
                    // Duplicate — flush current run up to this line
                    let abs_cur = chunk_start + cur_start;
                    if run_start < abs_cur {
                        runs.push((run_start, abs_cur));
                    }
                    // New run starts after this duplicate
                    run_start = chunk_start
                        + if cur_end < chunk_len {
                            cur_end + 1
                        } else {
                            cur_end
                        };
                } else {
                    last_out_start = chunk_start + cur_start;
                    last_out_end = chunk_start + cur_end;
                    prev_len = cur_end - cur_start;
                }
                prev_start = cur_start;
                prev_end = cur_end;

                if cur_end < chunk_len {
                    cur_start = cur_end + 1;
                } else {
                    break;
                }
            }

            // Close final run
            if run_start < chunk_end {
                runs.push((run_start, chunk_end));
            }

            ChunkResult {
                runs,
                first_line_start,
                first_line_end,
                last_line_start: last_out_start,
                last_line_end: last_out_end,
            }
        })
        .collect();

    // Write results, adjusting cross-chunk boundaries.
    // Batch output runs via write_vectored to reduce syscall count.
    const BATCH: usize = 256;
    let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH);
    for (i, result) in results.iter().enumerate() {
        let skip_first = if i > 0 {
            let prev = &results[i - 1];
            let prev_last = &data[prev.last_line_start..prev.last_line_end];
            let cur_first = &data[result.first_line_start..result.first_line_end];
            lines_equal_fast(prev_last, cur_first)
        } else {
            false
        };

        let skip_end = if skip_first {
            // Skip bytes up to and including the first line's terminator
            result.first_line_end + 1
        } else {
            0
        };

        for &(rs, re) in &result.runs {
            let actual_start = rs.max(skip_end);
            if actual_start < re {
                slices.push(io::IoSlice::new(&data[actual_start..re]));
                if slices.len() >= BATCH {
                    write_all_vectored(writer, &slices)?;
                    slices.clear();
                }
            }
        }
    }
    if !slices.is_empty() {
        write_all_vectored(writer, &slices)?;
    }

    // Ensure trailing terminator
    if !data.is_empty() && *data.last().unwrap() != term {
        writer.write_all(&[term])?;
    }

    Ok(())
}

/// Fast single-pass for RepeatedOnly (-d) and UniqueOnly (-u) modes.
/// Batched output: accumulates output lines in a 64KB buffer before flushing,
/// reducing the number of write() syscalls.
/// Uses cached prefix comparison for fast duplicate detection.
/// Uses raw pointer arithmetic throughout to avoid bounds checking.
fn process_filter_fast_singlepass(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let repeated = matches!(config.mode, OutputMode::RepeatedOnly);
    let data_len = data.len();
    let base = data.as_ptr();

    let first_term = match memchr::memchr(term, data) {
        Some(pos) => pos,
        None => {
            // Single line: unique (count=1)
            if !repeated {
                writer.write_all(data)?;
                writer.write_all(&[term])?;
            }
            return Ok(());
        }
    };

    // Write directly to the BufWriter (already 16MB) — no batch_buf needed.
    // This is zero-copy: we write slices from the original mmap data.
    let mut prev_start: usize = 0;
    let mut prev_end: usize = first_term;
    let mut prev_len = prev_end;
    let mut count: u64 = 1;
    let mut cur_start = first_term + 1;

    while cur_start < data_len {
        let cur_end = match memchr::memchr(term, unsafe {
            std::slice::from_raw_parts(base.add(cur_start), data_len - cur_start)
        }) {
            Some(offset) => cur_start + offset,
            None => data_len,
        };

        let cur_len = cur_end - cur_start;
        let is_dup = cur_len == prev_len
            && unsafe {
                let a = std::slice::from_raw_parts(base.add(prev_start), prev_len);
                let b = std::slice::from_raw_parts(base.add(cur_start), cur_len);
                lines_equal_fast(a, b)
            };

        if is_dup {
            count += 1;
        } else {
            // Output previous group directly from mmap data
            let should_print = if repeated { count > 1 } else { count == 1 };
            if should_print {
                writer.write_all(&data[prev_start..prev_end])?;
                writer.write_all(&[term])?;
            }
            prev_start = cur_start;
            prev_end = cur_end;
            prev_len = cur_len;
            count = 1;
        }

        if cur_end < data_len {
            cur_start = cur_end + 1;
        } else {
            break;
        }
    }

    // Output last group
    let should_print = if repeated { count > 1 } else { count == 1 };
    if should_print {
        writer.write_all(&data[prev_start..prev_end])?;
        writer.write_all(&[term])?;
    }

    Ok(())
}

/// Fast single-pass for count mode (-c) with all standard output modes.
/// Zero line_starts allocation: scans with memchr, counts groups inline,
/// and writes count-prefixed lines directly.
/// Uses cached length comparison for fast duplicate rejection.
/// Uses raw pointer arithmetic to avoid bounds checking.
///
/// Batched output: accumulates count-prefixed lines in a 64KB buffer and
/// flushes periodically, reducing the number of write() syscalls from
/// one-per-group to one-per-64KB.
fn process_count_fast_singlepass(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let data_len = data.len();
    let base = data.as_ptr();
    let is_default = matches!(config.mode, OutputMode::Default);

    let first_term = match memchr::memchr(term, data) {
        Some(pos) => pos,
        None => {
            // Single line: count=1
            let should_print = match config.mode {
                OutputMode::Default => true,
                OutputMode::RepeatedOnly => false,
                OutputMode::UniqueOnly => true,
                _ => true,
            };
            if should_print {
                write_count_line(writer, 1, data, term)?;
            }
            return Ok(());
        }
    };

    let mut prev_start: usize = 0;
    let mut prev_end: usize = first_term;
    let mut prev_len = prev_end;
    let mut count: u64 = 1;
    let mut cur_start = first_term + 1;

    while cur_start < data_len {
        let cur_end = match memchr::memchr(term, unsafe {
            std::slice::from_raw_parts(base.add(cur_start), data_len - cur_start)
        }) {
            Some(offset) => cur_start + offset,
            None => data_len,
        };

        let cur_len = cur_end - cur_start;
        let is_dup = cur_len == prev_len
            && unsafe {
                let a = std::slice::from_raw_parts(base.add(prev_start), prev_len);
                let b = std::slice::from_raw_parts(base.add(cur_start), cur_len);
                lines_equal_fast(a, b)
            };

        if is_dup {
            count += 1;
        } else {
            // Output previous group with count — directly via BufWriter
            let should_print = if is_default {
                true
            } else {
                match config.mode {
                    OutputMode::RepeatedOnly => count > 1,
                    OutputMode::UniqueOnly => count == 1,
                    _ => true,
                }
            };
            if should_print {
                write_count_line(writer, count, &data[prev_start..prev_end], term)?;
            }
            prev_start = cur_start;
            prev_end = cur_end;
            prev_len = cur_len;
            count = 1;
        }

        if cur_end < data_len {
            cur_start = cur_end + 1;
        } else {
            break;
        }
    }

    // Output last group
    let should_print = if is_default {
        true
    } else {
        match config.mode {
            OutputMode::RepeatedOnly => count > 1,
            OutputMode::UniqueOnly => count == 1,
            _ => true,
        }
    };
    if should_print {
        write_count_line(writer, count, &data[prev_start..prev_end], term)?;
    }

    Ok(())
}

/// Fast single-pass for case-insensitive (-i) default mode.
/// Same logic as process_default_sequential but uses eq_ignore_ascii_case.
fn process_default_ci_singlepass(data: &[u8], writer: &mut impl Write, term: u8) -> io::Result<()> {
    let mut prev_start: usize = 0;
    let mut prev_end: usize;

    match memchr::memchr(term, data) {
        Some(pos) => {
            prev_end = pos;
        }
        None => {
            writer.write_all(data)?;
            return writer.write_all(&[term]);
        }
    }

    // Write first line
    writer.write_all(&data[..prev_end + 1])?;

    let mut cur_start = prev_end + 1;

    while cur_start < data.len() {
        let cur_end = match memchr::memchr(term, &data[cur_start..]) {
            Some(offset) => cur_start + offset,
            None => data.len(),
        };

        let prev_content = &data[prev_start..prev_end];
        let cur_content = &data[cur_start..cur_end];

        if !lines_equal_case_insensitive(prev_content, cur_content) {
            // Different line — write it
            if cur_end < data.len() {
                writer.write_all(&data[cur_start..cur_end + 1])?;
            } else {
                writer.write_all(&data[cur_start..cur_end])?;
                writer.write_all(&[term])?;
            }
            prev_start = cur_start;
            prev_end = cur_end;
        }

        if cur_end < data.len() {
            cur_start = cur_end + 1;
        } else {
            break;
        }
    }

    Ok(())
}

/// Fast single-pass for case-insensitive (-i) repeated/unique-only modes.
/// Zero-copy: writes directly from mmap data through BufWriter.
fn process_filter_ci_singlepass(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let repeated = matches!(config.mode, OutputMode::RepeatedOnly);

    let first_term = match memchr::memchr(term, data) {
        Some(pos) => pos,
        None => {
            if !repeated {
                writer.write_all(data)?;
                writer.write_all(&[term])?;
            }
            return Ok(());
        }
    };

    let mut prev_start: usize = 0;
    let mut prev_end: usize = first_term;
    let mut count: u64 = 1;
    let mut cur_start = first_term + 1;

    while cur_start < data.len() {
        let cur_end = match memchr::memchr(term, &data[cur_start..]) {
            Some(offset) => cur_start + offset,
            None => data.len(),
        };

        if lines_equal_case_insensitive(&data[prev_start..prev_end], &data[cur_start..cur_end]) {
            count += 1;
        } else {
            let should_print = if repeated { count > 1 } else { count == 1 };
            if should_print {
                writer.write_all(&data[prev_start..prev_end])?;
                writer.write_all(&[term])?;
            }
            prev_start = cur_start;
            prev_end = cur_end;
            count = 1;
        }

        if cur_end < data.len() {
            cur_start = cur_end + 1;
        } else {
            break;
        }
    }

    let should_print = if repeated { count > 1 } else { count == 1 };
    if should_print {
        writer.write_all(&data[prev_start..prev_end])?;
        writer.write_all(&[term])?;
    }

    Ok(())
}

/// Fast single-pass for case-insensitive (-i) count (-c) mode.
/// Writes directly to BufWriter — no batch_buf allocation needed.
fn process_count_ci_singlepass(
    data: &[u8],
    writer: &mut impl Write,
    config: &UniqConfig,
    term: u8,
) -> io::Result<()> {
    let first_term = match memchr::memchr(term, data) {
        Some(pos) => pos,
        None => {
            let should_print = match config.mode {
                OutputMode::Default => true,
                OutputMode::RepeatedOnly => false,
                OutputMode::UniqueOnly => true,
                _ => true,
            };
            if should_print {
                write_count_line(writer, 1, data, term)?;
            }
            return Ok(());
        }
    };

    let is_default = matches!(config.mode, OutputMode::Default);

    let mut prev_start: usize = 0;
    let mut prev_end: usize = first_term;
    let mut count: u64 = 1;
    let mut cur_start = first_term + 1;

    while cur_start < data.len() {
        let cur_end = match memchr::memchr(term, &data[cur_start..]) {
            Some(offset) => cur_start + offset,
            None => data.len(),
        };

        if lines_equal_case_insensitive(&data[prev_start..prev_end], &data[cur_start..cur_end]) {
            count += 1;
        } else {
            let should_print = if is_default {
                true
            } else {
                match config.mode {
                    OutputMode::RepeatedOnly => count > 1,
                    OutputMode::UniqueOnly => count == 1,
                    _ => true,
                }
            };
            if should_print {
                write_count_line(writer, count, &data[prev_start..prev_end], term)?;
            }
            prev_start = cur_start;
            prev_end = cur_end;
            count = 1;
        }

        if cur_end < data.len() {
            cur_start = cur_end + 1;
        } else {
            break;
        }
    }

    let should_print = if is_default {
        true
    } else {
        match config.mode {
            OutputMode::RepeatedOnly => count > 1,
            OutputMode::UniqueOnly => count == 1,
            _ => true,
        }
    };
    if should_print {
        write_count_line(writer, count, &data[prev_start..prev_end], term)?;
    }

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
    let reader = BufReader::with_capacity(8 * 1024 * 1024, input);
    let mut writer = BufWriter::with_capacity(32 * 1024 * 1024, output);
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
