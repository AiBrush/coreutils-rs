use memchr::memchr_iter;
use rayon::prelude::*;
use std::io::{self, BufRead, Write};

/// Minimum file size for parallel processing (1MB).
/// Rayon overhead is ~5-10μs per task; at 1MB per chunk,
/// each chunk takes ~100μs+ to process, so overhead is < 10%.
const PARALLEL_THRESHOLD: usize = 1024 * 1024;

/// Configuration for cut operations.
pub struct CutConfig<'a> {
    pub mode: CutMode,
    pub ranges: &'a [Range],
    pub complement: bool,
    pub delim: u8,
    pub output_delim: &'a [u8],
    pub suppress_no_delim: bool,
    pub line_delim: u8,
}

/// A range specification like 1, 3-5, -3, 4-
#[derive(Debug, Clone)]
pub struct Range {
    pub start: usize, // 1-based, 0 means "from beginning"
    pub end: usize,   // 1-based, usize::MAX means "to end"
}

/// Parse a LIST specification like "1,3-5,7-" into ranges.
/// Each range is 1-based. Returns sorted, merged ranges.
pub fn parse_ranges(spec: &str) -> Result<Vec<Range>, String> {
    let mut ranges = Vec::new();

    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some(idx) = part.find('-') {
            let left = &part[..idx];
            let right = &part[idx + 1..];

            let start = if left.is_empty() {
                1
            } else {
                left.parse::<usize>()
                    .map_err(|_| format!("invalid range: '{}'", part))?
            };

            let end = if right.is_empty() {
                usize::MAX
            } else {
                right
                    .parse::<usize>()
                    .map_err(|_| format!("invalid range: '{}'", part))?
            };

            if start == 0 {
                return Err("fields and positions are numbered from 1".to_string());
            }
            if start > end {
                return Err(format!("invalid decreasing range: '{}'", part));
            }

            ranges.push(Range { start, end });
        } else {
            let n = part
                .parse::<usize>()
                .map_err(|_| format!("invalid field: '{}'", part))?;
            if n == 0 {
                return Err("fields and positions are numbered from 1".to_string());
            }
            ranges.push(Range { start: n, end: n });
        }
    }

    if ranges.is_empty() {
        return Err("you must specify a list of bytes, characters, or fields".to_string());
    }

    // Sort and merge overlapping ranges
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut merged = vec![ranges[0].clone()];
    for r in &ranges[1..] {
        let last = merged.last_mut().unwrap();
        if r.start <= last.end.saturating_add(1) {
            last.end = last.end.max(r.end);
        } else {
            merged.push(r.clone());
        }
    }

    Ok(merged)
}

/// Check if a 1-based position is in any range.
/// Ranges must be sorted. Uses early exit since ranges are sorted.
#[inline(always)]
fn in_ranges(ranges: &[Range], pos: usize) -> bool {
    for r in ranges {
        if pos < r.start {
            return false; // ranges are sorted, no point checking further
        }
        if pos <= r.end {
            return true;
        }
    }
    false
}

/// Pre-compute a 64-bit mask for field selection.
/// Bit i-1 is set if field i should be output.
#[inline]
fn compute_field_mask(ranges: &[Range], complement: bool) -> u64 {
    let mut mask: u64 = 0;
    for i in 1..=64u32 {
        let in_range = in_ranges(ranges, i as usize);
        if in_range != complement {
            mask |= 1u64 << (i - 1);
        }
    }
    mask
}

/// Check if a field should be selected, using bitset for first 64 fields.
#[inline(always)]
fn is_selected(field_num: usize, mask: u64, ranges: &[Range], complement: bool) -> bool {
    if field_num <= 64 {
        (mask >> (field_num - 1)) & 1 == 1
    } else {
        in_ranges(ranges, field_num) != complement
    }
}

// ── Chunk splitting for parallel processing ──────────────────────────────

/// Split data into chunks aligned to line boundaries for parallel processing.
/// Returns slices that each end on a line_delim boundary (except possibly the last).
fn split_into_chunks<'a>(data: &'a [u8], line_delim: u8) -> Vec<&'a [u8]> {
    let num_threads = rayon::current_num_threads().max(1);
    if data.len() < PARALLEL_THRESHOLD || num_threads <= 1 {
        return vec![data];
    }

    let chunk_size = data.len() / num_threads;
    let mut chunks = Vec::with_capacity(num_threads);
    let mut pos = 0;

    for _ in 0..num_threads - 1 {
        let target = pos + chunk_size;
        if target >= data.len() {
            break;
        }
        // Align to next line boundary
        let boundary = memchr::memchr(line_delim, &data[target..])
            .map(|p| target + p + 1)
            .unwrap_or(data.len());
        if boundary > pos {
            chunks.push(&data[pos..boundary]);
        }
        pos = boundary;
    }

    // Last chunk gets the remainder
    if pos < data.len() {
        chunks.push(&data[pos..]);
    }

    chunks
}

// ── Fast path: field extraction with batched output ──────────────────────

/// Optimized field extraction with early exit and batched output.
/// Uses SIMD newline scanning + inline byte scan per line.
fn process_fields_fast(data: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    let delim = cfg.delim;
    let line_delim = cfg.line_delim;
    let ranges = cfg.ranges;
    let complement = cfg.complement;
    let output_delim = cfg.output_delim;
    let suppress = cfg.suppress_no_delim;

    // Ultra-fast path: single field extraction (e.g., cut -f5)
    if !complement && ranges.len() == 1 && ranges[0].start == ranges[0].end {
        return process_single_field(data, delim, line_delim, ranges[0].start, suppress, out);
    }

    // Fast path: complement of single field with default output delimiter.
    // e.g., cut --complement -f1: skip first field, output rest unchanged.
    if complement
        && ranges.len() == 1
        && ranges[0].start == ranges[0].end
        && output_delim.len() == 1
        && output_delim[0] == delim
    {
        return process_complement_single_field(
            data,
            delim,
            line_delim,
            ranges[0].start,
            suppress,
            out,
        );
    }

    // Fast path: contiguous from-start field range (e.g., cut -f1-5, cut -f-3)
    // with default output delimiter. Just find the N-th delimiter and copy.
    if !complement
        && ranges.len() == 1
        && ranges[0].start == 1
        && output_delim.len() == 1
        && output_delim[0] == delim
        && ranges[0].end < usize::MAX
    {
        return process_fields_prefix(data, delim, line_delim, ranges[0].end, suppress, out);
    }

    // Fast path: open-ended field range from field N (e.g., cut -f3-)
    // Skip first N-1 delimiters per line instead of per-field selection.
    if !complement
        && ranges.len() == 1
        && ranges[0].end == usize::MAX
        && ranges[0].start > 1
        && output_delim.len() == 1
        && output_delim[0] == delim
    {
        return process_fields_suffix(data, delim, line_delim, ranges[0].start, suppress, out);
    }

    // Pre-compute for general field extraction
    let max_field = if complement {
        usize::MAX
    } else {
        ranges.last().map(|r| r.end).unwrap_or(0)
    };
    let field_mask = compute_field_mask(ranges, complement);

    if data.len() >= PARALLEL_THRESHOLD {
        // Parallel path
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len() / 2);
                process_fields_chunk(
                    chunk,
                    delim,
                    ranges,
                    output_delim,
                    suppress,
                    max_field,
                    field_mask,
                    line_delim,
                    complement,
                    &mut buf,
                );
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        // Sequential path
        let mut buf = Vec::with_capacity(data.len() / 2);
        process_fields_chunk(
            data,
            delim,
            ranges,
            output_delim,
            suppress,
            max_field,
            field_mask,
            line_delim,
            complement,
            &mut buf,
        );
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk of data for general field extraction.
fn process_fields_chunk(
    data: &[u8],
    delim: u8,
    ranges: &[Range],
    output_delim: &[u8],
    suppress: bool,
    max_field: usize,
    field_mask: u64,
    line_delim: u8,
    complement: bool,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        extract_fields_to_buf(
            line,
            delim,
            ranges,
            output_delim,
            suppress,
            max_field,
            field_mask,
            line_delim,
            buf,
            complement,
        );
        start = end_pos + 1;
    }
    if start < data.len() {
        extract_fields_to_buf(
            &data[start..],
            delim,
            ranges,
            output_delim,
            suppress,
            max_field,
            field_mask,
            line_delim,
            buf,
            complement,
        );
    }
}

// ── Ultra-fast single field extraction ───────────────────────────────────

/// Specialized path for extracting exactly one field (e.g., `cut -f5`).
/// Uses SIMD memchr for newline scanning + minimal inline byte scan per line.
/// For large files, splits work across multiple threads with rayon.
fn process_single_field(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    target: usize,
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    let target_idx = target - 1;

    // Ultra-fast path: first field with combined delimiter+newline scan.
    // Uses memchr2_iter to find both delimiter and newline in a single SIMD pass,
    // eliminating the per-line memchr call overhead.
    if target_idx == 0 && delim != line_delim {
        if data.len() >= PARALLEL_THRESHOLD {
            let chunks = split_into_chunks(data, line_delim);
            let results: Vec<Vec<u8>> = chunks
                .par_iter()
                .map(|chunk| {
                    let mut buf = Vec::with_capacity(chunk.len() / 4);
                    process_first_field_combined(chunk, delim, line_delim, suppress, &mut buf);
                    buf
                })
                .collect();
            for result in &results {
                if !result.is_empty() {
                    out.write_all(result)?;
                }
            }
        } else {
            let mut buf = Vec::with_capacity(data.len() / 4);
            process_first_field_combined(data, delim, line_delim, suppress, &mut buf);
            if !buf.is_empty() {
                out.write_all(&buf)?;
            }
        }
        return Ok(());
    }

    if data.len() >= PARALLEL_THRESHOLD {
        // Parallel path: split into chunks aligned to line boundaries
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len() / 4);
                process_single_field_chunk(
                    chunk, delim, target_idx, line_delim, suppress, &mut buf,
                );
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        // Sequential path for small data
        let mut buf = Vec::with_capacity(data.len() / 4);
        process_single_field_chunk(data, delim, target_idx, line_delim, suppress, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Complement single-field extraction: skip one field, output rest unchanged.
/// For `--complement -f1`: find first delimiter, output everything after.
/// For `--complement -fN`: output fields 1..N-1, skip N, output N+1...
/// Uses combined SIMD scan for first-field complement (most common case).
fn process_complement_single_field(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    skip_field: usize,
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    let skip_idx = skip_field - 1;

    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                complement_single_field_chunk(
                    chunk, delim, skip_idx, line_delim, suppress, &mut buf,
                );
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        let mut buf = Vec::with_capacity(data.len());
        complement_single_field_chunk(data, delim, skip_idx, line_delim, suppress, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for complement single-field extraction.
fn complement_single_field_chunk(
    data: &[u8],
    delim: u8,
    skip_idx: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        complement_single_field_line(line, delim, skip_idx, line_delim, suppress, buf);
        start = end_pos + 1;
    }
    if start < data.len() {
        complement_single_field_line(&data[start..], delim, skip_idx, line_delim, suppress, buf);
    }
}

/// Extract all fields except skip_idx from one line.
#[inline(always)]
fn complement_single_field_line(
    line: &[u8],
    delim: u8,
    skip_idx: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    if line.is_empty() {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    // Find all delimiter positions
    let mut field_idx = 0;
    let mut field_start = 0;
    let mut first_output = true;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        if field_idx != skip_idx {
            if !first_output {
                buf.push(delim);
            }
            buf.extend_from_slice(&line[field_start..pos]);
            first_output = false;
        }
        field_idx += 1;
        field_start = pos + 1;
    }

    if !has_delim {
        if !suppress {
            buf.extend_from_slice(line);
            buf.push(line_delim);
        }
        return;
    }

    // Last field
    if field_idx != skip_idx {
        if !first_output {
            buf.push(delim);
        }
        buf.extend_from_slice(&line[field_start..]);
    }

    if !first_output || field_idx != skip_idx {
        buf.push(line_delim);
    } else {
        // Only the skipped field existed — output empty line
        buf.push(line_delim);
    }
}

/// Contiguous from-start field range extraction (e.g., `cut -f1-5`).
/// Finds the N-th delimiter and copies everything before it (preserving original delimiters).
/// Much faster than per-field selection: one memchr_iter scan with early exit.
fn process_fields_prefix(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    last_field: usize,
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                fields_prefix_chunk(chunk, delim, line_delim, last_field, suppress, &mut buf);
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        let mut buf = Vec::with_capacity(data.len());
        fields_prefix_chunk(data, delim, line_delim, last_field, suppress, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for contiguous from-start field range extraction.
fn fields_prefix_chunk(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    last_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        fields_prefix_line(line, delim, line_delim, last_field, suppress, buf);
        start = end_pos + 1;
    }
    if start < data.len() {
        fields_prefix_line(&data[start..], delim, line_delim, last_field, suppress, buf);
    }
}

/// Extract first N fields from one line (contiguous from-start range).
/// Finds the N-th delimiter using memchr_iter with early exit.
#[inline(always)]
fn fields_prefix_line(
    line: &[u8],
    delim: u8,
    line_delim: u8,
    last_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    if line.is_empty() {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    // Count delimiters; stop at last_field (we need fields 1..last_field)
    let mut field_count = 1;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        if field_count >= last_field {
            // Found enough fields — output up to this delimiter
            buf.extend_from_slice(&line[..pos]);
            buf.push(line_delim);
            return;
        }
        field_count += 1;
    }

    if !has_delim {
        if !suppress {
            buf.extend_from_slice(line);
            buf.push(line_delim);
        }
        return;
    }

    // Fewer fields than requested — output the entire line
    // (all fields are within range since we have < last_field fields)
    buf.extend_from_slice(line);
    buf.push(line_delim);
}

/// Open-ended field suffix extraction (e.g., `cut -f3-`).
/// Skip the first N-1 delimiters, then output everything from field N to end.
fn process_fields_suffix(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    start_field: usize,
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                fields_suffix_chunk(chunk, delim, line_delim, start_field, suppress, &mut buf);
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        let mut buf = Vec::with_capacity(data.len());
        fields_suffix_chunk(data, delim, line_delim, start_field, suppress, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for open-ended field suffix extraction.
fn fields_suffix_chunk(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    start_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        fields_suffix_line(line, delim, line_delim, start_field, suppress, buf);
        start = end_pos + 1;
    }
    if start < data.len() {
        fields_suffix_line(
            &data[start..],
            delim,
            line_delim,
            start_field,
            suppress,
            buf,
        );
    }
}

/// Extract fields from start_field to end from one line.
/// Skips the first (start_field - 1) delimiters, outputs the rest.
#[inline(always)]
fn fields_suffix_line(
    line: &[u8],
    delim: u8,
    line_delim: u8,
    start_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    if line.is_empty() {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    let skip_delims = start_field - 1;
    let mut delim_count = 0;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        delim_count += 1;
        if delim_count >= skip_delims {
            // Output from after this delimiter to end of line
            buf.extend_from_slice(&line[pos + 1..]);
            buf.push(line_delim);
            return;
        }
    }

    if !has_delim {
        // No delimiter — output whole line or suppress
        if !suppress {
            buf.extend_from_slice(line);
            buf.push(line_delim);
        }
        return;
    }

    // Fewer delimiters than needed — field doesn't exist, output empty
    buf.push(line_delim);
}

/// First-field extraction using combined delimiter+newline SIMD scan.
/// Single memchr2_iter pass finds both delimiter and newline positions,
/// eliminating per-line memchr overhead (saves ~250K function calls for 10MB).
fn process_first_field_combined(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let mut line_start = 0;
    let mut found_delim = false; // true if we already found delimiter for current line

    for pos in memchr::memchr2_iter(delim, line_delim, data) {
        let byte = data[pos];
        if byte == line_delim {
            // End of line
            if !found_delim {
                // No delimiter on this line — output whole line or suppress
                if !suppress {
                    buf.extend_from_slice(&data[line_start..pos]);
                    buf.push(line_delim);
                }
            }
            line_start = pos + 1;
            found_delim = false;
        } else {
            // Delimiter found
            if !found_delim {
                // First delimiter — output field before it
                buf.extend_from_slice(&data[line_start..pos]);
                buf.push(line_delim);
                found_delim = true;
            }
            // Subsequent delimiters on same line — ignore
        }
    }

    // Handle last line without trailing newline
    if line_start < data.len() {
        if !found_delim {
            // No delimiter found — output whole line or suppress
            if !suppress {
                // Check if there's a delimiter in the remaining data
                match memchr::memchr(delim, &data[line_start..]) {
                    Some(offset) => {
                        buf.extend_from_slice(&data[line_start..line_start + offset]);
                        buf.push(line_delim);
                    }
                    None => {
                        buf.extend_from_slice(&data[line_start..]);
                        buf.push(line_delim);
                    }
                }
            }
        }
    }
}

/// Process a chunk of data for single-field extraction.
fn process_single_field_chunk(
    data: &[u8],
    delim: u8,
    target_idx: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        extract_single_field_line(line, delim, target_idx, line_delim, suppress, buf);
        start = end_pos + 1;
    }
    // Handle final line without terminator
    if start < data.len() {
        extract_single_field_line(&data[start..], delim, target_idx, line_delim, suppress, buf);
    }
}

/// Extract a single field from one line.
/// For target_idx == 0 (first field), uses single memchr instead of memchr_iter.
/// For other fields, uses SIMD memchr_iter with early exit.
#[inline(always)]
fn extract_single_field_line(
    line: &[u8],
    delim: u8,
    target_idx: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    if line.is_empty() {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    // Ultra-fast path for first field (target_idx == 0): single memchr
    if target_idx == 0 {
        match memchr::memchr(delim, line) {
            Some(pos) => {
                buf.extend_from_slice(&line[..pos]);
                buf.push(line_delim);
            }
            None => {
                // No delimiter — output whole line or suppress
                if !suppress {
                    buf.extend_from_slice(line);
                    buf.push(line_delim);
                }
            }
        }
        return;
    }

    let mut field_start = 0;
    let mut field_idx = 0;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        if field_idx == target_idx {
            // Found end of target field — output and return
            buf.extend_from_slice(&line[field_start..pos]);
            buf.push(line_delim);
            return;
        }
        field_idx += 1;
        field_start = pos + 1;
    }

    if !has_delim {
        // No delimiters found — output whole line or suppress
        if !suppress {
            buf.extend_from_slice(line);
            buf.push(line_delim);
        }
        return;
    }

    if field_idx == target_idx {
        // Target is the last field (no trailing delimiter)
        buf.extend_from_slice(&line[field_start..]);
        buf.push(line_delim);
    } else {
        // Not enough fields — output empty line
        buf.push(line_delim);
    }
}

/// Extract fields from a single line into the output buffer.
/// Uses inline byte scanning with early exit for maximum performance.
#[inline(always)]
fn extract_fields_to_buf(
    line: &[u8],
    delim: u8,
    ranges: &[Range],
    output_delim: &[u8],
    suppress: bool,
    max_field: usize,
    field_mask: u64,
    line_delim: u8,
    buf: &mut Vec<u8>,
    complement: bool,
) {
    let len = line.len();

    // Empty line: no delimiter possible
    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    let mut field_num: usize = 1;
    let mut field_start: usize = 0;
    let mut first_output = true;
    let mut has_delim = false;

    // SIMD-accelerated delimiter scanning with early exit
    for delim_pos in memchr_iter(delim, line) {
        has_delim = true;

        if is_selected(field_num, field_mask, ranges, complement) {
            if !first_output {
                buf.extend_from_slice(output_delim);
            }
            buf.extend_from_slice(&line[field_start..delim_pos]);
            first_output = false;
        }

        field_num += 1;
        field_start = delim_pos + 1;

        // Early exit: past the last needed field
        if field_num > max_field {
            break;
        }
    }

    // Last field (only if we didn't early-exit past it)
    if (field_num <= max_field || complement)
        && has_delim
        && is_selected(field_num, field_mask, ranges, complement)
    {
        if !first_output {
            buf.extend_from_slice(output_delim);
        }
        buf.extend_from_slice(&line[field_start..len]);
        first_output = false;
    }

    // Output line terminator
    if !first_output {
        // Had output — add line delimiter
        buf.push(line_delim);
    } else if !has_delim {
        // No delimiter found — output whole line or suppress
        if !suppress {
            buf.extend_from_slice(line);
            buf.push(line_delim);
        }
    } else {
        // Had delimiter but no selected field — output empty line (GNU compat)
        buf.push(line_delim);
    }
}

// ── Fast path: byte/char extraction with batched output ──────────────────

/// Ultra-fast path for `cut -b1-N`: single from-start byte range.
/// Scans for newlines with SIMD memchr, truncates each line to N bytes.
/// Avoids per-line function call + range check overhead entirely.
fn process_bytes_from_start(
    data: &[u8],
    max_bytes: usize,
    line_delim: u8,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                bytes_from_start_chunk(chunk, max_bytes, line_delim, &mut buf);
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        let mut buf = Vec::with_capacity(data.len());
        bytes_from_start_chunk(data, max_bytes, line_delim, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for from-start byte range extraction.
/// For each line, outputs min(line_len, max_bytes) bytes + line delimiter.
#[inline]
fn bytes_from_start_chunk(data: &[u8], max_bytes: usize, line_delim: u8, buf: &mut Vec<u8>) {
    let mut start = 0;
    for pos in memchr_iter(line_delim, data) {
        let line_len = pos - start;
        let take = line_len.min(max_bytes);
        buf.extend_from_slice(&data[start..start + take]);
        buf.push(line_delim);
        start = pos + 1;
    }
    // Handle last line without terminator
    if start < data.len() {
        let line_len = data.len() - start;
        let take = line_len.min(max_bytes);
        buf.extend_from_slice(&data[start..start + take]);
        buf.push(line_delim);
    }
}

/// Fast path for `cut -bN-`: skip first N-1 bytes per line.
/// For each line, outputs bytes from position N to end of line.
fn process_bytes_from_offset(
    data: &[u8],
    skip_bytes: usize,
    line_delim: u8,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                bytes_from_offset_chunk(chunk, skip_bytes, line_delim, &mut buf);
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        let mut buf = Vec::with_capacity(data.len());
        bytes_from_offset_chunk(data, skip_bytes, line_delim, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for from-offset byte range extraction.
#[inline]
fn bytes_from_offset_chunk(data: &[u8], skip_bytes: usize, line_delim: u8, buf: &mut Vec<u8>) {
    let mut start = 0;
    for pos in memchr_iter(line_delim, data) {
        let line_len = pos - start;
        if line_len > skip_bytes {
            buf.extend_from_slice(&data[start + skip_bytes..pos]);
        }
        buf.push(line_delim);
        start = pos + 1;
    }
    // Handle last line without terminator
    if start < data.len() {
        let line_len = data.len() - start;
        if line_len > skip_bytes {
            buf.extend_from_slice(&data[start + skip_bytes..data.len()]);
        }
        buf.push(line_delim);
    }
}

/// Optimized byte/char extraction with batched output and parallel processing.
fn process_bytes_fast(data: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    let line_delim = cfg.line_delim;
    let ranges = cfg.ranges;
    let complement = cfg.complement;
    let output_delim = cfg.output_delim;

    // Ultra-fast path: single range from byte 1 (e.g., cut -b1-10, cut -b-20)
    // Avoids per-line function call overhead by scanning newlines and truncating inline.
    if !complement && ranges.len() == 1 && ranges[0].start == 1 && output_delim.is_empty() {
        let max_bytes = ranges[0].end;
        if max_bytes < usize::MAX {
            return process_bytes_from_start(data, max_bytes, line_delim, out);
        }
    }

    // Fast path: single open-ended range from byte N (e.g., cut -b5-)
    // Skip first N-1 bytes per line instead of per-byte range checking.
    if !complement && ranges.len() == 1 && ranges[0].end == usize::MAX && output_delim.is_empty() {
        let skip_bytes = ranges[0].start.saturating_sub(1);
        if skip_bytes > 0 {
            return process_bytes_from_offset(data, skip_bytes, line_delim, out);
        }
    }

    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len() / 2);
                process_bytes_chunk(
                    chunk,
                    ranges,
                    complement,
                    output_delim,
                    line_delim,
                    &mut buf,
                );
                buf
            })
            .collect();
        for result in &results {
            if !result.is_empty() {
                out.write_all(result)?;
            }
        }
    } else {
        let mut buf = Vec::with_capacity(data.len() / 2);
        process_bytes_chunk(data, ranges, complement, output_delim, line_delim, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk of data for byte/char extraction.
fn process_bytes_chunk(
    data: &[u8],
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    line_delim: u8,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        cut_bytes_to_buf(line, ranges, complement, output_delim, buf);
        buf.push(line_delim);
        start = end_pos + 1;
    }
    if start < data.len() {
        cut_bytes_to_buf(&data[start..], ranges, complement, output_delim, buf);
        buf.push(line_delim);
    }
}

/// Extract byte ranges from a line into the output buffer.
/// For the common non-complement case with contiguous ranges, uses bulk copy.
#[inline(always)]
fn cut_bytes_to_buf(
    line: &[u8],
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    let mut first_range = true;

    if complement {
        let mut pos: usize = 1;
        for r in ranges {
            let rs = r.start;
            let re = r.end.min(len);
            if pos < rs {
                if !first_range && !output_delim.is_empty() {
                    buf.extend_from_slice(output_delim);
                }
                buf.extend_from_slice(&line[pos - 1..rs - 1]);
                first_range = false;
            }
            pos = re + 1;
            if pos > len {
                break;
            }
        }
        if pos <= len {
            if !first_range && !output_delim.is_empty() {
                buf.extend_from_slice(output_delim);
            }
            buf.extend_from_slice(&line[pos - 1..len]);
        }
    } else if output_delim.is_empty() && ranges.len() == 1 {
        // Ultra-fast path: single range, no output delimiter
        let start = ranges[0].start.saturating_sub(1);
        let end = ranges[0].end.min(len);
        if start < len {
            buf.extend_from_slice(&line[start..end]);
        }
    } else {
        for r in ranges {
            let start = r.start.saturating_sub(1);
            let end = r.end.min(len);
            if start >= len {
                break;
            }
            if !first_range && !output_delim.is_empty() {
                buf.extend_from_slice(output_delim);
            }
            buf.extend_from_slice(&line[start..end]);
            first_range = false;
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────────

/// Cut fields from a line using a delimiter. Writes to `out`.
/// Returns true if any output was written, false if suppressed.
/// Used by process_cut_reader (stdin path) and unit tests.
#[inline]
pub fn cut_fields(
    line: &[u8],
    delim: u8,
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    suppress_no_delim: bool,
    out: &mut impl Write,
) -> io::Result<bool> {
    // Check if line contains delimiter at all
    if memchr::memchr(delim, line).is_none() {
        if !suppress_no_delim {
            out.write_all(line)?;
            return Ok(true);
        }
        return Ok(false); // suppressed
    }

    // Walk through fields using memchr, output selected ones
    let mut field_num: usize = 1;
    let mut field_start: usize = 0;
    let mut first_output = true;

    for delim_pos in memchr_iter(delim, line) {
        let selected = in_ranges(ranges, field_num) != complement;
        if selected {
            if !first_output {
                out.write_all(output_delim)?;
            }
            out.write_all(&line[field_start..delim_pos])?;
            first_output = false;
        }
        field_start = delim_pos + 1;
        field_num += 1;
    }

    // Last field (after last delimiter)
    let selected = in_ranges(ranges, field_num) != complement;
    if selected {
        if !first_output {
            out.write_all(output_delim)?;
        }
        out.write_all(&line[field_start..])?;
    }

    Ok(true)
}

/// Cut bytes/chars from a line. Writes selected bytes to `out`.
/// Used by process_cut_reader (stdin path) and unit tests.
#[inline]
pub fn cut_bytes(
    line: &[u8],
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    out: &mut impl Write,
) -> io::Result<bool> {
    let mut first_range = true;

    if complement {
        let len = line.len();
        let mut comp_ranges = Vec::new();
        let mut pos: usize = 1;
        for r in ranges {
            let rs = r.start;
            let re = r.end.min(len);
            if pos < rs {
                comp_ranges.push((pos, rs - 1));
            }
            pos = re + 1;
            if pos > len {
                break;
            }
        }
        if pos <= len {
            comp_ranges.push((pos, len));
        }
        for &(s, e) in &comp_ranges {
            if !first_range && !output_delim.is_empty() {
                out.write_all(output_delim)?;
            }
            out.write_all(&line[s - 1..e])?;
            first_range = false;
        }
    } else {
        for r in ranges {
            let start = r.start.saturating_sub(1);
            let end = r.end.min(line.len());
            if start >= line.len() {
                break;
            }
            if !first_range && !output_delim.is_empty() {
                out.write_all(output_delim)?;
            }
            out.write_all(&line[start..end])?;
            first_range = false;
        }
    }
    Ok(true)
}

/// Process a full data buffer (from mmap or read) with cut operation.
/// Dispatches to optimized fast paths for field and byte modes.
pub fn process_cut_data(data: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    match cfg.mode {
        CutMode::Fields => process_fields_fast(data, cfg, out),
        CutMode::Bytes | CutMode::Characters => process_bytes_fast(data, cfg, out),
    }
}

/// Process input from a reader (for stdin).
pub fn process_cut_reader<R: BufRead>(
    mut reader: R,
    cfg: &CutConfig,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let n = reader.read_until(cfg.line_delim, &mut buf)?;
        if n == 0 {
            break;
        }

        let has_line_delim = buf.last() == Some(&cfg.line_delim);
        let line = if has_line_delim {
            &buf[..buf.len() - 1]
        } else {
            &buf[..]
        };

        let wrote = process_one_line(line, cfg, out)?;

        // GNU always terminates output lines, even if input had no trailing delimiter
        if wrote {
            out.write_all(&[cfg.line_delim])?;
        }
    }

    Ok(())
}

/// Process one line according to the cut config (used by stdin reader path).
#[inline]
fn process_one_line(line: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<bool> {
    match cfg.mode {
        CutMode::Fields => cut_fields(
            line,
            cfg.delim,
            cfg.ranges,
            cfg.complement,
            cfg.output_delim,
            cfg.suppress_no_delim,
            out,
        ),
        CutMode::Bytes | CutMode::Characters => {
            cut_bytes(line, cfg.ranges, cfg.complement, cfg.output_delim, out)
        }
    }
}

/// Cut operation mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CutMode {
    Bytes,
    Characters,
    Fields,
}
