use memchr::memchr_iter;
use rayon::prelude::*;
use std::io::{self, BufRead, IoSlice, Write};

/// Minimum file size for parallel processing (512KB).
/// Lowered to benefit from parallel chunk processing on smaller piped inputs.
/// At 512KB with 2+ threads, the per-thread chunk is ~256KB which still
/// amortizes the rayon overhead (~100-200us) well.
const PARALLEL_THRESHOLD: usize = 512 * 1024;

/// Max iovec entries per writev call (Linux default).
const MAX_IOV: usize = 1024;

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
            return false;
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

// ── Unsafe buffer helpers (skip bounds checks in hot loops) ──────────────

/// Append a slice to buf without capacity checks.
/// Caller MUST ensure buf has enough remaining capacity.
#[inline(always)]
unsafe fn buf_extend(buf: &mut Vec<u8>, data: &[u8]) {
    unsafe {
        let len = buf.len();
        std::ptr::copy_nonoverlapping(data.as_ptr(), buf.as_mut_ptr().add(len), data.len());
        buf.set_len(len + data.len());
    }
}

/// Append a single byte to buf without capacity checks.
/// Caller MUST ensure buf has enough remaining capacity.
#[inline(always)]
unsafe fn buf_push(buf: &mut Vec<u8>, b: u8) {
    unsafe {
        let len = buf.len();
        *buf.as_mut_ptr().add(len) = b;
        buf.set_len(len + 1);
    }
}

/// Write multiple IoSlice buffers using write_vectored (writev syscall).
/// Batches into MAX_IOV-sized groups. Falls back to write_all per slice for partial writes.
#[inline]
fn write_ioslices(out: &mut impl Write, slices: &[IoSlice]) -> io::Result<()> {
    if slices.is_empty() {
        return Ok(());
    }
    for batch in slices.chunks(MAX_IOV) {
        let total: usize = batch.iter().map(|s| s.len()).sum();
        match out.write_vectored(batch) {
            Ok(n) if n >= total => continue,
            Ok(mut written) => {
                // Partial write: fall back to write_all per remaining slice
                for slice in batch {
                    let slen = slice.len();
                    if written >= slen {
                        written -= slen;
                        continue;
                    }
                    if written > 0 {
                        out.write_all(&slice[written..])?;
                        written = 0;
                    } else {
                        out.write_all(slice)?;
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ── Chunk splitting for parallel processing ──────────────────────────────

/// Split data into chunks aligned to line boundaries for parallel processing.
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
        let boundary = memchr::memchr(line_delim, &data[target..])
            .map(|p| target + p + 1)
            .unwrap_or(data.len());
        if boundary > pos {
            chunks.push(&data[pos..boundary]);
        }
        pos = boundary;
    }

    if pos < data.len() {
        chunks.push(&data[pos..]);
    }

    chunks
}

// ── Fast path: multi-field non-contiguous extraction ─────────────────────

/// Multi-field non-contiguous extraction (e.g., `cut -d, -f1,3,5`).
/// Pre-collects delimiter positions per line into a stack-allocated array,
/// then directly indexes into them for each selected field.
/// This is O(max_field) per line instead of O(num_fields * scan_length).
fn process_fields_multi_select(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    ranges: &[Range],
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    let max_field = ranges.last().map_or(0, |r| r.end);

    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                multi_select_chunk(
                    chunk, delim, line_delim, ranges, max_field, suppress, &mut buf,
                );
                buf
            })
            .collect();
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        let mut buf = Vec::with_capacity(data.len());
        multi_select_chunk(
            data, delim, line_delim, ranges, max_field, suppress, &mut buf,
        );
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for multi-field extraction using two-level scanning with early exit.
/// Outer loop: memchr for newlines (SIMD scan for line boundaries).
/// Inner loop: memchr_iter for delimiters with early exit at max_field.
/// This avoids scanning delimiters past max_field on each line, saving ~40% of
/// per-hit processing for lines with many columns but small field selections
/// (e.g., -f1,3,5 on 20-column CSV stops scanning after field 5).
fn multi_select_chunk(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    ranges: &[Range],
    max_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    buf.reserve(data.len());

    // Pre-compute field selection mask for O(1) field lookup
    let field_mask = compute_field_mask(ranges, false);

    let base = data.as_ptr();
    let mut start = 0;

    for end_pos in memchr_iter(line_delim, data) {
        let line = unsafe { std::slice::from_raw_parts(base.add(start), end_pos - start) };
        multi_select_line(
            line, delim, line_delim, ranges, max_field, field_mask, suppress, buf,
        );
        start = end_pos + 1;
    }
    // Handle last line without trailing line_delim
    if start < data.len() {
        let line = unsafe { std::slice::from_raw_parts(base.add(start), data.len() - start) };
        multi_select_line(
            line, delim, line_delim, ranges, max_field, field_mask, suppress, buf,
        );
    }
}

/// Extract selected fields from a single line using delimiter position scanning.
/// Scans delimiters only up to max_field (early exit), then extracts selected fields
/// by indexing into the collected positions.
#[inline(always)]
fn multi_select_line(
    line: &[u8],
    delim: u8,
    line_delim: u8,
    ranges: &[Range],
    max_field: usize,
    field_mask: u64,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    if len == 0 {
        if !suppress {
            unsafe { buf_push(buf, line_delim) };
        }
        return;
    }

    buf.reserve(len + 1);
    let base = line.as_ptr();

    // Collect delimiter positions up to max_field (early exit).
    // Stack array for up to 64 delimiter positions.
    let mut delim_pos = [0usize; 64];
    let mut num_delims: usize = 0;
    let max_delims = max_field.min(64); // need max_field-1 delimiters to identify max_field fields

    for pos in memchr_iter(delim, line) {
        if num_delims < max_delims {
            delim_pos[num_delims] = pos;
            num_delims += 1;
            if num_delims >= max_delims {
                break; // Early exit: found enough delimiters
            }
        }
    }

    if num_delims == 0 {
        // No delimiter in line
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
        return;
    }

    // Extract selected fields using delimiter positions.
    // Field N has content between delim_pos[N-2]+1 and delim_pos[N-1] (or line start/end).
    let total_fields = num_delims + 1;
    let mut first_output = true;

    for r in ranges {
        let range_start = r.start;
        let range_end = r.end.min(total_fields);
        if range_start > total_fields {
            break; // No more fields available
        }
        for field_num in range_start..=range_end {
            if field_num > total_fields {
                break;
            }
            if !is_selected(field_num, field_mask, ranges, false) {
                continue;
            }

            // Get field boundaries
            let field_start = if field_num == 1 {
                0
            } else if field_num - 2 < num_delims {
                delim_pos[field_num - 2] + 1
            } else {
                continue;
            };
            let field_end = if field_num <= num_delims {
                delim_pos[field_num - 1]
            } else {
                len
            };

            if !first_output {
                unsafe { buf_push(buf, delim) };
            }
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(field_start), field_end - field_start),
                );
            }
            first_output = false;
        }
    }

    if first_output {
        // No fields were output but line had delimiters — output empty line
        unsafe { buf_push(buf, line_delim) };
    } else {
        unsafe { buf_push(buf, line_delim) };
    }
}

// ── Fast path: field extraction with batched output ──────────────────────

/// Optimized field extraction with early exit and batched output.
fn process_fields_fast(data: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    let delim = cfg.delim;
    let line_delim = cfg.line_delim;
    let ranges = cfg.ranges;
    let complement = cfg.complement;
    let output_delim = cfg.output_delim;
    let suppress = cfg.suppress_no_delim;

    // NOTE: Removed the full-file `memchr(delim, data).is_none()` scan.
    // That scan was O(N) over the entire file just to check an edge case
    // (no delimiter in any line). The per-line processing already handles
    // lines without delimiters correctly, so the scan was pure overhead
    // for files that DO contain delimiters (the common case).

    // Ultra-fast path: single field extraction (e.g., cut -f5)
    if !complement && ranges.len() == 1 && ranges[0].start == ranges[0].end {
        return process_single_field(data, delim, line_delim, ranges[0].start, suppress, out);
    }

    // Fast path: complement of single field or contiguous range with default output delimiter.
    if complement
        && ranges.len() == 1
        && output_delim.len() == 1
        && output_delim[0] == delim
        && ranges[0].start == ranges[0].end
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

    // Fast path: complement of contiguous range (e.g., --complement -f3-5 = output fields 1,2,6+).
    // This is equivalent to outputting a prefix and a suffix, skipping the middle range.
    if complement
        && ranges.len() == 1
        && ranges[0].start > 1
        && ranges[0].end < usize::MAX
        && output_delim.len() == 1
        && output_delim[0] == delim
    {
        return process_complement_range(
            data,
            delim,
            line_delim,
            ranges[0].start,
            ranges[0].end,
            suppress,
            out,
        );
    }

    // Fast path: contiguous from-start field range (e.g., cut -f1-5)
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
    if !complement
        && ranges.len() == 1
        && ranges[0].end == usize::MAX
        && ranges[0].start > 1
        && output_delim.len() == 1
        && output_delim[0] == delim
    {
        return process_fields_suffix(data, delim, line_delim, ranges[0].start, suppress, out);
    }

    // Fast path: contiguous field range with start > 1 (e.g., cut -f2-4)
    if !complement
        && ranges.len() == 1
        && ranges[0].start > 1
        && ranges[0].end < usize::MAX
        && output_delim.len() == 1
        && output_delim[0] == delim
    {
        return process_fields_mid_range(
            data,
            delim,
            line_delim,
            ranges[0].start,
            ranges[0].end,
            suppress,
            out,
        );
    }

    // Fast path: multi-field non-contiguous extraction (e.g., cut -f1,3,5)
    // Uses delimiter position caching: find all delimiter positions per line,
    // then directly index into them for each selected field.
    // This is faster than the general extract_fields_to_buf which re-checks
    // is_selected() for every field encountered.
    if !complement
        && ranges.len() > 1
        && ranges.last().map_or(false, |r| r.end < usize::MAX)
        && output_delim.len() == 1
        && output_delim[0] == delim
        && delim != line_delim
    {
        return process_fields_multi_select(data, delim, line_delim, ranges, suppress, out);
    }

    // General field extraction
    let max_field = if complement {
        usize::MAX
    } else {
        ranges.last().map(|r| r.end).unwrap_or(0)
    };
    let field_mask = compute_field_mask(ranges, complement);

    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
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
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        let mut buf = Vec::with_capacity(data.len());
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
/// When `delim != line_delim`, uses a single-pass memchr2_iter scan to find both
/// delimiters and line terminators in one SIMD pass, eliminating per-line memchr_iter
/// setup overhead. When `delim == line_delim`, falls back to the two-level approach.
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
    // When delim != line_delim and max_field is bounded, use two-level approach:
    // outer memchr for newlines, inner memchr_iter for delimiters with early exit.
    // This avoids scanning past max_field on each line (significant for lines with
    // many columns but small field selection like -f1,3,5 on 20-column CSV).
    // For complement or unbounded ranges, use single-pass memchr2_iter which
    // needs to process all delimiters anyway.
    if delim != line_delim && max_field < usize::MAX && !complement {
        buf.reserve(data.len());
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
        return;
    }

    // Single-pass path for complement or unbounded ranges: memchr2_iter for both
    // delimiter and line_delim in one SIMD scan.
    // Uses raw pointer arithmetic to eliminate bounds checking in the hot loop.
    if delim != line_delim {
        buf.reserve(data.len());

        let data_len = data.len();
        let base = data.as_ptr();
        let mut line_start: usize = 0;
        let mut field_start: usize = 0;
        let mut field_num: usize = 1;
        let mut first_output = true;
        let mut has_delim = false;

        for pos in memchr::memchr2_iter(delim, line_delim, data) {
            let byte = unsafe { *base.add(pos) };

            if byte == line_delim {
                // End of line: flush final field and emit line delimiter
                if (field_num <= max_field || complement)
                    && has_delim
                    && is_selected(field_num, field_mask, ranges, complement)
                {
                    if !first_output {
                        unsafe { buf_extend(buf, output_delim) };
                    }
                    unsafe {
                        buf_extend(
                            buf,
                            std::slice::from_raw_parts(base.add(field_start), pos - field_start),
                        )
                    };
                    first_output = false;
                }

                if !first_output {
                    unsafe { buf_push(buf, line_delim) };
                } else if !has_delim {
                    if !suppress {
                        unsafe {
                            buf_extend(
                                buf,
                                std::slice::from_raw_parts(base.add(line_start), pos - line_start),
                            );
                            buf_push(buf, line_delim);
                        }
                    }
                } else {
                    unsafe { buf_push(buf, line_delim) };
                }

                // Reset state for next line
                line_start = pos + 1;
                field_start = pos + 1;
                field_num = 1;
                first_output = true;
                has_delim = false;
            } else {
                // Field delimiter hit
                has_delim = true;

                if is_selected(field_num, field_mask, ranges, complement) {
                    if !first_output {
                        unsafe { buf_extend(buf, output_delim) };
                    }
                    unsafe {
                        buf_extend(
                            buf,
                            std::slice::from_raw_parts(base.add(field_start), pos - field_start),
                        )
                    };
                    first_output = false;
                }

                field_num += 1;
                field_start = pos + 1;
            }
        }

        // Handle last line without trailing line_delim
        if line_start < data_len {
            if line_start < data_len {
                if (field_num <= max_field || complement)
                    && has_delim
                    && is_selected(field_num, field_mask, ranges, complement)
                {
                    if !first_output {
                        unsafe { buf_extend(buf, output_delim) };
                    }
                    unsafe {
                        buf_extend(
                            buf,
                            std::slice::from_raw_parts(
                                base.add(field_start),
                                data_len - field_start,
                            ),
                        )
                    };
                    first_output = false;
                }

                if !first_output {
                    unsafe { buf_push(buf, line_delim) };
                } else if !has_delim {
                    if !suppress {
                        unsafe {
                            buf_extend(
                                buf,
                                std::slice::from_raw_parts(
                                    base.add(line_start),
                                    data_len - line_start,
                                ),
                            );
                            buf_push(buf, line_delim);
                        }
                    }
                } else {
                    unsafe { buf_push(buf, line_delim) };
                }
            }
        }

        return;
    }

    // Fallback: when delim == line_delim, use the two-level scan approach
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
/// Uses combined memchr2_iter SIMD scan when delim != line_delim for a single
/// pass over the data (vs. nested loops: outer newline scan + inner delim scan).
fn process_single_field(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    target: usize,
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    let target_idx = target - 1;

    // Combined SIMD scan: single pass using memchr2 for any target field.
    if delim != line_delim {
        if data.len() >= PARALLEL_THRESHOLD {
            let chunks = split_into_chunks(data, line_delim);
            let results: Vec<Vec<u8>> = chunks
                .par_iter()
                .map(|chunk| {
                    let mut buf = Vec::with_capacity(chunk.len());
                    process_nth_field_combined(
                        chunk, delim, line_delim, target_idx, suppress, &mut buf,
                    );
                    buf
                })
                .collect();
            // Use write_vectored (writev) to batch N writes into fewer syscalls
            let slices: Vec<IoSlice> = results
                .iter()
                .filter(|r| !r.is_empty())
                .map(|r| IoSlice::new(r))
                .collect();
            write_ioslices(out, &slices)?;
        } else if target_idx == 0 && !suppress {
            // Zero-copy fast path for field 1 (most common case):
            // For each line, either truncate at the first delimiter, or pass through.
            // Since most lines have a delimiter, and field 1 is a prefix of each line,
            // we can write contiguous runs directly from the source data.
            single_field1_zerocopy(data, delim, line_delim, out)?;
        } else {
            // Single-pass SIMD scan using memchr2_iter for both delimiter and
            // line_delim simultaneously. For large files this is faster than the
            // two-pass approach (outer newline scan + inner scalar field scan)
            // because it processes the entire file in one SIMD sweep.
            let mut buf = Vec::with_capacity(data.len().min(4 * 1024 * 1024));
            process_nth_field_combined(data, delim, line_delim, target_idx, suppress, &mut buf);
            if !buf.is_empty() {
                out.write_all(&buf)?;
            }
        }
        return Ok(());
    }

    // Fallback for delim == line_delim: nested loop approach
    if data.len() >= PARALLEL_THRESHOLD {
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
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        let mut buf = Vec::with_capacity(data.len() / 4);
        process_single_field_chunk(data, delim, target_idx, line_delim, suppress, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Complement range extraction: skip fields start..=end, output rest (e.g., --complement -f3-5).
/// For each line: output fields 1..start-1, then fields end+1..EOF, skipping fields start..end.
fn process_complement_range(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    skip_start: usize,
    skip_end: usize,
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                complement_range_chunk(
                    chunk, delim, skip_start, skip_end, line_delim, suppress, &mut buf,
                );
                buf
            })
            .collect();
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        let mut buf = Vec::with_capacity(data.len());
        complement_range_chunk(
            data, delim, skip_start, skip_end, line_delim, suppress, &mut buf,
        );
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for complement range extraction.
fn complement_range_chunk(
    data: &[u8],
    delim: u8,
    skip_start: usize,
    skip_end: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        complement_range_line(line, delim, skip_start, skip_end, line_delim, suppress, buf);
        start = end_pos + 1;
    }
    if start < data.len() {
        complement_range_line(
            &data[start..],
            delim,
            skip_start,
            skip_end,
            line_delim,
            suppress,
            buf,
        );
    }
}

/// Extract all fields except skip_start..=skip_end from one line.
/// Outputs fields 1..skip_start-1, then fields skip_end+1..EOF.
///
/// Optimized: only scans for enough delimiters to find the skip region boundaries.
/// For `--complement -f3-5` with 20 fields, this finds delimiter 2 and 5, then
/// does a single copy of prefix + suffix, avoiding scanning past field 5.
#[inline(always)]
fn complement_range_line(
    line: &[u8],
    delim: u8,
    skip_start: usize,
    skip_end: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    buf.reserve(len + 1);
    let base = line.as_ptr();

    // 1-based field numbers. To skip fields skip_start..=skip_end:
    // - prefix_end = position of (skip_start-1)th delimiter (exclusive; end of prefix fields)
    // - suffix_start = position after skip_end-th delimiter (inclusive; start of suffix fields)
    //
    // Find the first (skip_start - 1) delimiters to locate prefix_end,
    // then the next (skip_end - skip_start + 1) delimiters to locate suffix_start.

    let need_prefix_delims = skip_start - 1; // number of delimiters before the skip region
    let need_skip_delims = skip_end - skip_start + 1; // delimiters within the skip region
    let total_need = need_prefix_delims + need_skip_delims;

    // Find delimiter positions up to total_need
    let mut delim_count: usize = 0;
    let mut prefix_end_pos: usize = usize::MAX; // byte position of (skip_start-1)th delim
    let mut suffix_start_pos: usize = usize::MAX; // byte position after skip_end-th delim

    for pos in memchr_iter(delim, line) {
        delim_count += 1;
        if delim_count == need_prefix_delims {
            prefix_end_pos = pos;
        }
        if delim_count == total_need {
            suffix_start_pos = pos + 1;
            break;
        }
    }

    if delim_count == 0 {
        // No delimiter at all
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
        return;
    }

    // Case analysis:
    // 1. Not enough delims to reach skip_start: all fields are before skip region, output all
    // 2. Enough to reach skip_start but not skip_end: prefix + no suffix
    // 3. Enough to reach skip_end: prefix + delim + suffix

    if delim_count < need_prefix_delims {
        // Not enough fields to reach skip region — output entire line
        unsafe {
            buf_extend(buf, line);
            buf_push(buf, line_delim);
        }
        return;
    }

    let has_prefix = need_prefix_delims > 0;
    let has_suffix = suffix_start_pos != usize::MAX && suffix_start_pos < len;

    if has_prefix && has_suffix {
        // Output: prefix (up to prefix_end_pos) + delim + suffix (from suffix_start_pos)
        unsafe {
            buf_extend(buf, std::slice::from_raw_parts(base, prefix_end_pos));
            buf_push(buf, delim);
            buf_extend(
                buf,
                std::slice::from_raw_parts(base.add(suffix_start_pos), len - suffix_start_pos),
            );
            buf_push(buf, line_delim);
        }
    } else if has_prefix {
        // Only prefix, no suffix (skip region extends to end of line)
        unsafe {
            buf_extend(buf, std::slice::from_raw_parts(base, prefix_end_pos));
            buf_push(buf, line_delim);
        }
    } else if has_suffix {
        // No prefix (skip_start == 1), only suffix
        unsafe {
            buf_extend(
                buf,
                std::slice::from_raw_parts(base.add(suffix_start_pos), len - suffix_start_pos),
            );
            buf_push(buf, line_delim);
        }
    } else {
        // All fields skipped
        unsafe { buf_push(buf, line_delim) };
    }
}

/// Complement single-field extraction: skip one field, output rest unchanged.
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
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
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
/// Optimized: finds only the delimiters bounding the skip field (skip_idx-th
/// and (skip_idx+1)-th delimiters), then copies prefix + suffix in 2 bulk
/// copies instead of iterating through all fields.
#[inline(always)]
fn complement_single_field_line(
    line: &[u8],
    delim: u8,
    skip_idx: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    buf.reserve(len + 1);
    let base = line.as_ptr();

    // Find the delimiters bounding the skip field:
    // - We need skip_idx delimiters to find where the skip field starts
    // - We need one more delimiter to find where it ends
    // For skip_idx == 0 (skip field 1): skip field starts at 0, ends at first delimiter
    // For skip_idx == 1 (skip field 2): skip field starts after 1st delim, ends at 2nd delim
    let need_before = skip_idx; // delimiters before skip field
    let need_total = skip_idx + 1; // delimiters to find end of skip field

    let mut delim_count: usize = 0;
    let mut skip_start_pos: usize = 0; // byte start of skip field
    let mut skip_end_pos: usize = len; // byte position of delimiter after skip field (or EOL)
    let mut found_end = false;

    for pos in memchr_iter(delim, line) {
        delim_count += 1;
        if delim_count == need_before {
            skip_start_pos = pos + 1;
        }
        if delim_count == need_total {
            skip_end_pos = pos;
            found_end = true;
            break;
        }
    }

    if delim_count == 0 {
        // No delimiter in line
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
        return;
    }

    // Not enough delimiters to reach the skip field: output entire line
    if delim_count < need_before {
        unsafe {
            buf_extend(buf, line);
            buf_push(buf, line_delim);
        }
        return;
    }

    // skip field is at positions skip_start_pos..skip_end_pos
    // Output prefix (before skip field) + suffix (after skip field)
    let has_prefix = skip_idx > 0 && skip_start_pos > 0;
    let has_suffix = found_end && skip_end_pos < len;

    if has_prefix && has_suffix {
        // prefix = line[0..skip_start_pos-1] (before the delimiter that starts skip field)
        // suffix = line[skip_end_pos+1..] (after the delimiter that ends skip field)
        unsafe {
            buf_extend(buf, std::slice::from_raw_parts(base, skip_start_pos - 1));
            buf_push(buf, delim);
            buf_extend(
                buf,
                std::slice::from_raw_parts(base.add(skip_end_pos + 1), len - skip_end_pos - 1),
            );
            buf_push(buf, line_delim);
        }
    } else if has_prefix {
        // Only prefix (skip field is the last field)
        unsafe {
            buf_extend(buf, std::slice::from_raw_parts(base, skip_start_pos - 1));
            buf_push(buf, line_delim);
        }
    } else if has_suffix {
        // No prefix (skip field is the first field)
        unsafe {
            buf_extend(
                buf,
                std::slice::from_raw_parts(base.add(skip_end_pos + 1), len - skip_end_pos - 1),
            );
            buf_push(buf, line_delim);
        }
    } else {
        // Skip field is the only field (or entire line)
        unsafe { buf_push(buf, line_delim) };
    }
}

/// Contiguous from-start field range extraction (e.g., `cut -f1-5`).
/// Zero-copy for the non-parallel path: identifies the truncation point per line
/// and writes contiguous runs directly from the source data.
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
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else if !suppress {
        // Zero-copy fast path: scan for truncation points, write runs from source.
        // When suppress is false, every line is output (with or without delimiter).
        // Most lines have enough fields, so the output is often identical to input.
        fields_prefix_zerocopy(data, delim, line_delim, last_field, out)?;
    } else {
        let mut buf = Vec::with_capacity(data.len());
        fields_prefix_chunk(data, delim, line_delim, last_field, suppress, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Zero-copy field-prefix extraction using writev: builds IoSlice entries pointing
/// directly into the source data, flushing in MAX_IOV-sized batches.
/// For lines where the Nth delimiter exists, we truncate at that point.
/// For lines with fewer fields, we output them unchanged (contiguous run).
/// Lines without any delimiter are output unchanged (suppress=false assumed).
#[inline]
fn fields_prefix_zerocopy(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    last_field: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let newline_buf: [u8; 1] = [line_delim];
    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut start = 0;
    let mut run_start: usize = 0;

    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        let mut field_count = 1;
        let mut truncate_at: Option<usize> = None;
        for dpos in memchr_iter(delim, line) {
            if field_count >= last_field {
                truncate_at = Some(start + dpos);
                break;
            }
            field_count += 1;
        }

        if let Some(trunc_pos) = truncate_at {
            if run_start < start {
                iov.push(IoSlice::new(&data[run_start..start]));
            }
            iov.push(IoSlice::new(&data[start..trunc_pos]));
            iov.push(IoSlice::new(&newline_buf));
            run_start = end_pos + 1;

            if iov.len() >= MAX_IOV - 2 {
                write_ioslices(out, &iov)?;
                iov.clear();
            }
        }
        start = end_pos + 1;
    }
    // Handle last line without terminator
    if start < data.len() {
        let line = &data[start..];
        let mut field_count = 1;
        let mut truncate_at: Option<usize> = None;
        for dpos in memchr_iter(delim, line) {
            if field_count >= last_field {
                truncate_at = Some(start + dpos);
                break;
            }
            field_count += 1;
        }
        if let Some(trunc_pos) = truncate_at {
            if run_start < start {
                iov.push(IoSlice::new(&data[run_start..start]));
            }
            iov.push(IoSlice::new(&data[start..trunc_pos]));
            iov.push(IoSlice::new(&newline_buf));
            if !iov.is_empty() {
                write_ioslices(out, &iov)?;
            }
            return Ok(());
        }
    }
    // Flush remaining contiguous run
    if run_start < data.len() {
        iov.push(IoSlice::new(&data[run_start..]));
        if !data.is_empty() && *data.last().unwrap() != line_delim {
            iov.push(IoSlice::new(&newline_buf));
        }
    }
    if !iov.is_empty() {
        write_ioslices(out, &iov)?;
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
/// Uses memchr SIMD for delimiter scanning on all line sizes.
#[inline(always)]
fn fields_prefix_line(
    line: &[u8],
    delim: u8,
    line_delim: u8,
    last_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    buf.reserve(len + 1);
    let base = line.as_ptr();

    let mut field_count = 1usize;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        if field_count >= last_field {
            unsafe {
                buf_extend(buf, std::slice::from_raw_parts(base, pos));
                buf_push(buf, line_delim);
            }
            return;
        }
        field_count += 1;
    }

    if !has_delim {
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
        return;
    }

    unsafe {
        buf_extend(buf, line);
        buf_push(buf, line_delim);
    }
}

/// Open-ended field suffix extraction (e.g., `cut -f3-`).
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
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
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
/// Uses memchr SIMD for delimiter scanning on all line sizes.
#[inline(always)]
fn fields_suffix_line(
    line: &[u8],
    delim: u8,
    line_delim: u8,
    start_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    buf.reserve(len + 1);
    let base = line.as_ptr();

    let skip_delims = start_field - 1;
    let mut delim_count = 0usize;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        delim_count += 1;
        if delim_count >= skip_delims {
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(pos + 1), len - pos - 1),
                );
                buf_push(buf, line_delim);
            }
            return;
        }
    }

    if !has_delim {
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
        return;
    }

    // Fewer delimiters than needed
    unsafe { buf_push(buf, line_delim) };
}

/// Contiguous mid-range field extraction (e.g., `cut -f2-4`).
/// Optimized: skip to start_field using memchr, then output until end_field.
fn process_fields_mid_range(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    start_field: usize,
    end_field: usize,
    suppress: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.len() >= PARALLEL_THRESHOLD {
        let chunks = split_into_chunks(data, line_delim);
        let results: Vec<Vec<u8>> = chunks
            .par_iter()
            .map(|chunk| {
                let mut buf = Vec::with_capacity(chunk.len());
                fields_mid_range_chunk(
                    chunk,
                    delim,
                    line_delim,
                    start_field,
                    end_field,
                    suppress,
                    &mut buf,
                );
                buf
            })
            .collect();
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        let mut buf = Vec::with_capacity(data.len());
        fields_mid_range_chunk(
            data,
            delim,
            line_delim,
            start_field,
            end_field,
            suppress,
            &mut buf,
        );
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk for contiguous mid-range field extraction.
fn fields_mid_range_chunk(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    start_field: usize,
    end_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = &data[start..end_pos];
        fields_mid_range_line(
            line,
            delim,
            line_delim,
            start_field,
            end_field,
            suppress,
            buf,
        );
        start = end_pos + 1;
    }
    if start < data.len() {
        fields_mid_range_line(
            &data[start..],
            delim,
            line_delim,
            start_field,
            end_field,
            suppress,
            buf,
        );
    }
}

/// Extract fields start_field..=end_field from one line.
/// Uses scalar byte scanning for short lines, memchr_iter for longer.
/// Raw pointer arithmetic to eliminate bounds checking.
#[inline(always)]
fn fields_mid_range_line(
    line: &[u8],
    delim: u8,
    line_delim: u8,
    start_field: usize,
    end_field: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    buf.reserve(len + 1);
    let base = line.as_ptr();

    // Count delimiters to find start_field and end_field boundaries
    let skip_before = start_field - 1; // delimiters to skip before start_field
    let field_span = end_field - start_field; // additional delimiters within the range
    let target_end_delim = skip_before + field_span + 1;
    let mut delim_count = 0;
    let mut range_start = 0;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        delim_count += 1;
        if delim_count == skip_before {
            range_start = pos + 1;
        }
        if delim_count == target_end_delim {
            if skip_before == 0 {
                range_start = 0;
            }
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(range_start), pos - range_start),
                );
                buf_push(buf, line_delim);
            }
            return;
        }
    }

    if !has_delim {
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
        return;
    }

    // Line has delimiters but fewer fields than end_field
    if delim_count >= skip_before {
        // We have at least start_field, output from range_start to end
        if skip_before == 0 {
            range_start = 0;
        }
        unsafe {
            buf_extend(
                buf,
                std::slice::from_raw_parts(base.add(range_start), len - range_start),
            );
            buf_push(buf, line_delim);
        }
    } else {
        // Not enough fields even for start_field — output empty line
        unsafe { buf_push(buf, line_delim) };
    }
}

/// Combined SIMD scan for arbitrary single field extraction.
/// Uses memchr2_iter(delim, line_delim) to scan for both bytes in a single SIMD pass.
/// This is faster than the nested approach (outer: find newlines, inner: find delimiters)
/// because it eliminates one full SIMD scan and improves cache locality.
///
/// For target_idx == 0 (field 1), after finding the target field we skip remaining
/// delimiters on the line by scanning directly for line_delim.
fn process_nth_field_combined(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    target_idx: usize,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    buf.reserve(data.len());

    let data_len = data.len();
    let base = data.as_ptr();
    let mut line_start: usize = 0;
    let mut field_start: usize = 0;
    let mut field_idx: usize = 0;
    let mut has_delim = false;
    let mut emitted = false;

    for pos in memchr::memchr2_iter(delim, line_delim, data) {
        let byte = unsafe { *base.add(pos) };

        if byte == line_delim {
            // End of line
            if !emitted {
                if has_delim && field_idx == target_idx {
                    // Last field matches target
                    unsafe {
                        buf_extend(
                            buf,
                            std::slice::from_raw_parts(base.add(field_start), pos - field_start),
                        );
                        buf_push(buf, line_delim);
                    }
                } else if has_delim {
                    // Target field doesn't exist (fewer fields)
                    unsafe {
                        buf_push(buf, line_delim);
                    }
                } else if !suppress {
                    // No delimiter in line — output unchanged
                    unsafe {
                        buf_extend(
                            buf,
                            std::slice::from_raw_parts(base.add(line_start), pos - line_start),
                        );
                        buf_push(buf, line_delim);
                    }
                }
            }
            // Reset for next line
            line_start = pos + 1;
            field_start = pos + 1;
            field_idx = 0;
            has_delim = false;
            emitted = false;
        } else {
            // Delimiter found
            has_delim = true;
            if field_idx == target_idx {
                unsafe {
                    buf_extend(
                        buf,
                        std::slice::from_raw_parts(base.add(field_start), pos - field_start),
                    );
                    buf_push(buf, line_delim);
                }
                emitted = true;
            }
            field_idx += 1;
            field_start = pos + 1;
        }
    }

    // Handle last line without trailing newline
    if line_start < data_len && !emitted {
        if has_delim && field_idx == target_idx {
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(field_start), data_len - field_start),
                );
                buf_push(buf, line_delim);
            }
        } else if has_delim {
            unsafe {
                buf_push(buf, line_delim);
            }
        } else if !suppress {
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(line_start), data_len - line_start),
                );
                buf_push(buf, line_delim);
            }
        }
    }
}

/// Zero-copy field-1 extraction using writev: builds IoSlice entries pointing
/// directly into the source data, flushing in MAX_IOV-sized batches.
/// For each line: if delimiter exists, output field1 + newline; otherwise pass through.
/// Uses memchr2 to scan for both delimiter and line terminator in a single SIMD pass.
///
/// For lines without delimiter: the entire line (including its newline) is a single
/// contiguous IoSlice. For lines with delimiter: two IoSlices (field1 data + newline byte).
/// Contiguous runs of unmodified lines are coalesced into a single IoSlice.
#[inline]
fn single_field1_zerocopy(
    data: &[u8],
    delim: u8,
    line_delim: u8,
    out: &mut impl Write,
) -> io::Result<()> {
    // Static newline byte for IoSlice references
    let newline_buf: [u8; 1] = [line_delim];

    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut line_start: usize = 0;
    let mut run_start: usize = 0;
    let mut first_delim: Option<usize> = None;

    for pos in memchr::memchr2_iter(delim, line_delim, data) {
        let byte = unsafe { *data.get_unchecked(pos) };

        if byte == line_delim {
            // End of line
            if let Some(dp) = first_delim {
                // Line has delimiter — truncate at first delimiter.
                // Flush current contiguous run, then add truncated line + newline.
                if run_start < line_start {
                    iov.push(IoSlice::new(&data[run_start..line_start]));
                }
                iov.push(IoSlice::new(&data[line_start..dp]));
                iov.push(IoSlice::new(&newline_buf));
                run_start = pos + 1;

                // Flush when we're near the iovec limit
                if iov.len() >= MAX_IOV - 2 {
                    write_ioslices(out, &iov)?;
                    iov.clear();
                }
            }
            // else: no delimiter in line, output unchanged (stays in run)
            line_start = pos + 1;
            first_delim = None;
        } else {
            // Delimiter found
            if first_delim.is_none() {
                first_delim = Some(pos);
            }
        }
    }

    // Handle last line (no trailing line_delim)
    if line_start < data.len() {
        if let Some(dp) = first_delim {
            if run_start < line_start {
                iov.push(IoSlice::new(&data[run_start..line_start]));
            }
            iov.push(IoSlice::new(&data[line_start..dp]));
            iov.push(IoSlice::new(&newline_buf));
            if !iov.is_empty() {
                write_ioslices(out, &iov)?;
            }
            return Ok(());
        }
    }

    // Flush remaining contiguous run
    if run_start < data.len() {
        iov.push(IoSlice::new(&data[run_start..]));
        if !data.is_empty() && *data.last().unwrap() != line_delim {
            iov.push(IoSlice::new(&newline_buf));
        }
    }
    if !iov.is_empty() {
        write_ioslices(out, &iov)?;
    }
    Ok(())
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
    if start < data.len() {
        extract_single_field_line(&data[start..], delim, target_idx, line_delim, suppress, buf);
    }
}

/// Extract a single field from one line.
/// For short lines (< 256 bytes), uses direct scalar scanning to avoid memchr overhead.
/// For longer lines, uses memchr for SIMD-accelerated scanning.
/// Raw pointer arithmetic eliminates per-field bounds checking.
#[inline(always)]
fn extract_single_field_line(
    line: &[u8],
    delim: u8,
    target_idx: usize,
    line_delim: u8,
    suppress: bool,
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    // Ensure capacity for worst case (full line + newline)
    buf.reserve(len + 1);

    let base = line.as_ptr();

    // Ultra-fast path for first field: single memchr
    if target_idx == 0 {
        match memchr::memchr(delim, line) {
            Some(pos) => unsafe {
                buf_extend(buf, std::slice::from_raw_parts(base, pos));
                buf_push(buf, line_delim);
            },
            None => {
                if !suppress {
                    unsafe {
                        buf_extend(buf, line);
                        buf_push(buf, line_delim);
                    }
                }
            }
        }
        return;
    }

    // Use memchr SIMD for all line sizes (faster than scalar even for short lines)
    let mut field_start = 0;
    let mut field_idx = 0;
    let mut has_delim = false;

    for pos in memchr_iter(delim, line) {
        has_delim = true;
        if field_idx == target_idx {
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(field_start), pos - field_start),
                );
                buf_push(buf, line_delim);
            }
            return;
        }
        field_idx += 1;
        field_start = pos + 1;
    }

    if !has_delim {
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
        return;
    }

    if field_idx == target_idx {
        unsafe {
            buf_extend(
                buf,
                std::slice::from_raw_parts(base.add(field_start), len - field_start),
            );
            buf_push(buf, line_delim);
        }
    } else {
        unsafe { buf_push(buf, line_delim) };
    }
}

/// Extract fields from a single line into the output buffer.
/// Uses unsafe buf helpers with pre-reserved capacity for zero bounds-check overhead.
/// Raw pointer arithmetic eliminates per-field bounds checking.
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

    if len == 0 {
        if !suppress {
            buf.push(line_delim);
        }
        return;
    }

    // Only reserve if remaining capacity is insufficient. The caller pre-sizes the
    // buffer to data.len(), so this check avoids redundant reserve() calls per line.
    let needed = len + output_delim.len() * 16 + 1;
    if buf.capacity() - buf.len() < needed {
        buf.reserve(needed);
    }

    let base = line.as_ptr();
    let mut field_num: usize = 1;
    let mut field_start: usize = 0;
    let mut first_output = true;
    let mut has_delim = false;

    // Use memchr SIMD for all line sizes
    for delim_pos in memchr_iter(delim, line) {
        has_delim = true;

        if is_selected(field_num, field_mask, ranges, complement) {
            if !first_output {
                unsafe { buf_extend(buf, output_delim) };
            }
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(field_start), delim_pos - field_start),
                )
            };
            first_output = false;
        }

        field_num += 1;
        field_start = delim_pos + 1;

        if field_num > max_field {
            break;
        }
    }

    // Last field
    if (field_num <= max_field || complement)
        && has_delim
        && is_selected(field_num, field_mask, ranges, complement)
    {
        if !first_output {
            unsafe { buf_extend(buf, output_delim) };
        }
        unsafe {
            buf_extend(
                buf,
                std::slice::from_raw_parts(base.add(field_start), len - field_start),
            )
        };
        first_output = false;
    }

    if !first_output {
        unsafe { buf_push(buf, line_delim) };
    } else if !has_delim {
        if !suppress {
            unsafe {
                buf_extend(buf, line);
                buf_push(buf, line_delim);
            }
        }
    } else {
        unsafe { buf_push(buf, line_delim) };
    }
}

// ── Fast path: byte/char extraction with batched output ──────────────────

/// Ultra-fast path for `cut -b1-N`: single from-start byte range.
/// Zero-copy: writes directly from the source data using output runs.
/// For lines shorter than max_bytes, the output is identical to the input,
/// so we emit contiguous runs directly. Only lines exceeding max_bytes need truncation.
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
                // Estimate output size without scanning: assume average line
                // is at least (max_bytes+1) bytes (otherwise no truncation).
                // For cut -b1-5 on 50-char lines: output ~ chunk.len() * 6/51 ~ chunk/8.
                // Using chunk.len()/4 as initial capacity handles most cases
                // without reallocation, while avoiding the extra memchr scan.
                let est_out = (chunk.len() / 4).max(max_bytes + 2);
                let mut buf = Vec::with_capacity(est_out.min(chunk.len()));
                bytes_from_start_chunk(chunk, max_bytes, line_delim, &mut buf);
                buf
            })
            .collect();
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        // For small max_bytes, the buffer path is faster than writev zero-copy
        // because every line gets truncated, creating 3 IoSlice entries per line.
        // Copying max_bytes+1 bytes into a contiguous buffer is cheaper than
        // managing millions of IoSlice entries through the kernel.
        if max_bytes <= 64 {
            // Estimate output size without scanning: output <= data.len(),
            // typically ~data.len()/4 for short max_bytes on longer lines.
            let est_out = (data.len() / 4).max(max_bytes + 2);
            let mut buf = Vec::with_capacity(est_out.min(data.len()));
            bytes_from_start_chunk(data, max_bytes, line_delim, &mut buf);
            if !buf.is_empty() {
                out.write_all(&buf)?;
            }
        } else {
            // Zero-copy path: track contiguous output runs and write directly from source.
            // For lines <= max_bytes, we include them as-is (no copy needed).
            // For lines > max_bytes, we flush the run, write the truncated line, start new run.
            bytes_from_start_zerocopy(data, max_bytes, line_delim, out)?;
        }
    }
    Ok(())
}

/// Zero-copy byte-prefix extraction using writev: builds IoSlice entries pointing
/// directly into the source data, flushing in MAX_IOV-sized batches.
/// Lines shorter than max_bytes stay in contiguous runs. Lines needing truncation
/// produce two IoSlices (truncated data + newline).
#[inline]
fn bytes_from_start_zerocopy(
    data: &[u8],
    max_bytes: usize,
    line_delim: u8,
    out: &mut impl Write,
) -> io::Result<()> {
    let newline_buf: [u8; 1] = [line_delim];
    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut start = 0;
    let mut run_start: usize = 0;

    for pos in memchr_iter(line_delim, data) {
        let line_len = pos - start;
        if line_len > max_bytes {
            // This line needs truncation
            if run_start < start {
                iov.push(IoSlice::new(&data[run_start..start]));
            }
            iov.push(IoSlice::new(&data[start..start + max_bytes]));
            iov.push(IoSlice::new(&newline_buf));
            run_start = pos + 1;

            if iov.len() >= MAX_IOV - 2 {
                write_ioslices(out, &iov)?;
                iov.clear();
            }
        }
        start = pos + 1;
    }
    // Handle last line without terminator
    if start < data.len() {
        let line_len = data.len() - start;
        if line_len > max_bytes {
            if run_start < start {
                iov.push(IoSlice::new(&data[run_start..start]));
            }
            iov.push(IoSlice::new(&data[start..start + max_bytes]));
            iov.push(IoSlice::new(&newline_buf));
            if !iov.is_empty() {
                write_ioslices(out, &iov)?;
            }
            return Ok(());
        }
    }
    // Flush remaining contiguous run
    if run_start < data.len() {
        iov.push(IoSlice::new(&data[run_start..]));
        if !data.is_empty() && *data.last().unwrap() != line_delim {
            iov.push(IoSlice::new(&newline_buf));
        }
    }
    if !iov.is_empty() {
        write_ioslices(out, &iov)?;
    }
    Ok(())
}

/// Process a chunk for from-start byte range extraction (parallel path).
/// Uses unsafe appends to eliminate bounds checking in the hot loop.
/// When max_bytes is small, the output per line is tiny so we fuse the
/// data copy + delimiter into a single tight loop with minimal overhead.
#[inline]
fn bytes_from_start_chunk(data: &[u8], max_bytes: usize, line_delim: u8, buf: &mut Vec<u8>) {
    // Pre-reserve enough capacity: output is at most data.len() bytes
    // (when all lines are <= max_bytes, output equals input).
    // This eliminates per-iteration capacity checks in the hot loop.
    buf.reserve(data.len().min(buf.capacity().max(max_bytes + 2)));

    let mut start = 0;
    let data_ptr = data.as_ptr();

    for pos in memchr_iter(line_delim, data) {
        let line_len = pos - start;
        let take = line_len.min(max_bytes);
        // Check capacity only when we might exceed it (rare with good pre-reserve)
        let needed = take + 1;
        if buf.len() + needed > buf.capacity() {
            buf.reserve(needed.max(data.len() - pos));
        }
        unsafe {
            let dst = buf.as_mut_ptr().add(buf.len());
            std::ptr::copy_nonoverlapping(data_ptr.add(start), dst, take);
            *dst.add(take) = line_delim;
            buf.set_len(buf.len() + take + 1);
        }
        start = pos + 1;
    }
    // Handle last line without terminator
    if start < data.len() {
        let line_len = data.len() - start;
        let take = line_len.min(max_bytes);
        let needed = take + 1;
        if buf.len() + needed > buf.capacity() {
            buf.reserve(needed);
        }
        unsafe {
            let dst = buf.as_mut_ptr().add(buf.len());
            std::ptr::copy_nonoverlapping(data_ptr.add(start), dst, take);
            *dst.add(take) = line_delim;
            buf.set_len(buf.len() + take + 1);
        }
    }
}

/// Fast path for `cut -bN-`: skip first N-1 bytes per line.
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
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        // Zero-copy: write suffix of each line directly from source
        bytes_from_offset_zerocopy(data, skip_bytes, line_delim, out)?;
    }
    Ok(())
}

/// Zero-copy byte-offset extraction: writes suffix of each line directly from source data.
/// Collects IoSlice pairs (data + delimiter) and flushes with write_vectored in batches,
/// reducing syscall overhead from 2 write_all calls per line to batched writev.
#[inline]
fn bytes_from_offset_zerocopy(
    data: &[u8],
    skip_bytes: usize,
    line_delim: u8,
    out: &mut impl Write,
) -> io::Result<()> {
    let delim_buf = [line_delim];
    let mut iov: Vec<IoSlice> = Vec::with_capacity(256);

    let mut start = 0;
    for pos in memchr_iter(line_delim, data) {
        let line_len = pos - start;
        if line_len > skip_bytes {
            iov.push(IoSlice::new(&data[start + skip_bytes..pos]));
        }
        iov.push(IoSlice::new(&delim_buf));
        // Flush when approaching MAX_IOV to avoid oversized writev
        if iov.len() >= MAX_IOV - 1 {
            write_ioslices(out, &iov)?;
            iov.clear();
        }
        start = pos + 1;
    }
    if start < data.len() {
        let line_len = data.len() - start;
        if line_len > skip_bytes {
            iov.push(IoSlice::new(&data[start + skip_bytes..data.len()]));
        }
        iov.push(IoSlice::new(&delim_buf));
    }
    if !iov.is_empty() {
        write_ioslices(out, &iov)?;
    }
    Ok(())
}

/// Process a chunk for from-offset byte range extraction.
/// Uses unsafe appends to eliminate bounds checking in the hot loop.
#[inline]
fn bytes_from_offset_chunk(data: &[u8], skip_bytes: usize, line_delim: u8, buf: &mut Vec<u8>) {
    buf.reserve(data.len());

    let mut start = 0;
    for pos in memchr_iter(line_delim, data) {
        let line_len = pos - start;
        if line_len > skip_bytes {
            unsafe {
                buf_extend(buf, &data[start + skip_bytes..pos]);
            }
        }
        unsafe {
            buf_push(buf, line_delim);
        }
        start = pos + 1;
    }
    if start < data.len() {
        let line_len = data.len() - start;
        if line_len > skip_bytes {
            unsafe {
                buf_extend(buf, &data[start + skip_bytes..data.len()]);
            }
        }
        unsafe {
            buf_push(buf, line_delim);
        }
    }
}

/// Optimized byte/char extraction with batched output and parallel processing.
fn process_bytes_fast(data: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    let line_delim = cfg.line_delim;
    let ranges = cfg.ranges;
    let complement = cfg.complement;
    let output_delim = cfg.output_delim;

    // Ultra-fast path: single range from byte 1 (e.g., cut -b1-10, cut -b-20)
    if !complement && ranges.len() == 1 && ranges[0].start == 1 && output_delim.is_empty() {
        let max_bytes = ranges[0].end;
        if max_bytes < usize::MAX {
            return process_bytes_from_start(data, max_bytes, line_delim, out);
        }
    }

    // Fast path: single open-ended range from byte N (e.g., cut -b5-)
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
                let mut buf = Vec::with_capacity(chunk.len());
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
        // Use write_vectored (writev) to batch N writes into fewer syscalls
        let slices: Vec<IoSlice> = results
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| IoSlice::new(r))
            .collect();
        write_ioslices(out, &slices)?;
    } else {
        let mut buf = Vec::with_capacity(data.len());
        process_bytes_chunk(data, ranges, complement, output_delim, line_delim, &mut buf);
        if !buf.is_empty() {
            out.write_all(&buf)?;
        }
    }
    Ok(())
}

/// Process a chunk of data for byte/char extraction.
/// Uses raw pointer arithmetic for the newline scan.
fn process_bytes_chunk(
    data: &[u8],
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    line_delim: u8,
    buf: &mut Vec<u8>,
) {
    buf.reserve(data.len());
    let base = data.as_ptr();
    let mut start = 0;
    for end_pos in memchr_iter(line_delim, data) {
        let line = unsafe { std::slice::from_raw_parts(base.add(start), end_pos - start) };
        cut_bytes_to_buf(line, ranges, complement, output_delim, buf);
        unsafe { buf_push(buf, line_delim) };
        start = end_pos + 1;
    }
    if start < data.len() {
        let line = unsafe { std::slice::from_raw_parts(base.add(start), data.len() - start) };
        cut_bytes_to_buf(line, ranges, complement, output_delim, buf);
        unsafe { buf_push(buf, line_delim) };
    }
}

/// Extract byte ranges from a line into the output buffer.
/// Uses unsafe buf helpers for zero bounds-check overhead in hot loops.
/// Raw pointer arithmetic eliminates per-range bounds checking.
#[inline(always)]
fn cut_bytes_to_buf(
    line: &[u8],
    ranges: &[Range],
    complement: bool,
    output_delim: &[u8],
    buf: &mut Vec<u8>,
) {
    let len = line.len();
    let base = line.as_ptr();
    let mut first_range = true;

    // Reserve worst case: full line + delimiters between ranges
    let needed = len + output_delim.len() * ranges.len() + 1;
    if buf.capacity() - buf.len() < needed {
        buf.reserve(needed);
    }

    if complement {
        let mut pos: usize = 1;
        for r in ranges {
            let rs = r.start;
            let re = r.end.min(len);
            if pos < rs {
                if !first_range && !output_delim.is_empty() {
                    unsafe { buf_extend(buf, output_delim) };
                }
                unsafe { buf_extend(buf, std::slice::from_raw_parts(base.add(pos - 1), rs - pos)) };
                first_range = false;
            }
            pos = re + 1;
            if pos > len {
                break;
            }
        }
        if pos <= len {
            if !first_range && !output_delim.is_empty() {
                unsafe { buf_extend(buf, output_delim) };
            }
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(pos - 1), len - pos + 1),
                )
            };
        }
    } else if output_delim.is_empty() && ranges.len() == 1 {
        // Ultra-fast path: single range, no output delimiter
        let start = ranges[0].start.saturating_sub(1);
        let end = ranges[0].end.min(len);
        if start < len {
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(start), end - start),
                )
            };
        }
    } else {
        for r in ranges {
            let start = r.start.saturating_sub(1);
            let end = r.end.min(len);
            if start >= len {
                break;
            }
            if !first_range && !output_delim.is_empty() {
                unsafe { buf_extend(buf, output_delim) };
            }
            unsafe {
                buf_extend(
                    buf,
                    std::slice::from_raw_parts(base.add(start), end - start),
                )
            };
            first_range = false;
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────────

/// Cut fields from a line using a delimiter. Writes to `out`.
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
    if memchr::memchr(delim, line).is_none() {
        if !suppress_no_delim {
            out.write_all(line)?;
            return Ok(true);
        }
        return Ok(false);
    }

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
pub fn process_cut_data(data: &[u8], cfg: &CutConfig, out: &mut impl Write) -> io::Result<()> {
    match cfg.mode {
        CutMode::Fields => process_fields_fast(data, cfg, out),
        CutMode::Bytes | CutMode::Characters => process_bytes_fast(data, cfg, out),
    }
}

/// Process input from a reader (for stdin).
/// Uses batch reading: reads large chunks (16MB), then processes them in batch
/// using the fast mmap-based paths, avoiding per-line read_until syscall overhead.
/// 16MB chunks mean a 10MB piped input is consumed in a single batch.
pub fn process_cut_reader<R: BufRead>(
    mut reader: R,
    cfg: &CutConfig,
    out: &mut impl Write,
) -> io::Result<()> {
    const CHUNK_SIZE: usize = 16 * 1024 * 1024; // 16MB read chunks
    let line_delim = cfg.line_delim;

    // Read large chunks and process in batch.
    // We keep a buffer; after processing complete lines, we shift leftover to the front.
    let mut buf = Vec::with_capacity(CHUNK_SIZE + 4096);

    loop {
        // Read up to CHUNK_SIZE bytes
        buf.reserve(CHUNK_SIZE);
        let read_start = buf.len();
        unsafe { buf.set_len(read_start + CHUNK_SIZE) };
        let n = read_fully(&mut reader, &mut buf[read_start..])?;
        buf.truncate(read_start + n);

        if buf.is_empty() {
            break;
        }

        if n == 0 {
            // EOF with leftover data (last line without terminator)
            process_cut_data(&buf, cfg, out)?;
            break;
        }

        // Find the last line delimiter in the buffer so we process complete lines
        let process_end = match memchr::memrchr(line_delim, &buf) {
            Some(pos) => pos + 1,
            None => {
                // No line delimiter found — keep accumulating
                continue;
            }
        };

        // Process the complete lines using the fast batch path
        process_cut_data(&buf[..process_end], cfg, out)?;

        // Shift leftover to the front for next iteration
        let leftover_len = buf.len() - process_end;
        if leftover_len > 0 {
            buf.copy_within(process_end.., 0);
        }
        buf.truncate(leftover_len);
    }

    Ok(())
}

/// Read as many bytes as possible into buf, retrying on partial reads.
#[inline]
fn read_fully<R: BufRead>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let n = reader.read(buf)?;
    if n == buf.len() || n == 0 {
        return Ok(n);
    }
    // Slow path: partial read — retry to fill buffer
    let mut total = n;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// Cut operation mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CutMode {
    Bytes,
    Characters,
    Fields,
}
