/// Core sorting logic for fsort.
/// High-performance implementation using single-buffer + index sorting.
///
/// Key optimizations:
/// - Single contiguous buffer for all input (mmap for files, Vec for stdin)
/// - Sort lightweight index pairs instead of moving line data
/// - par_sort_unstable_by (pdqsort) for non-stable sort
/// - memchr SIMD for line boundary detection
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::sync::Arc;

use memmap2::Mmap;
use rayon::prelude::*;

use crate::common::io_error_msg;

use super::compare::{
    compare_with_opts, parse_general_numeric, parse_human_numeric, parse_numeric_value,
    select_comparator, skip_leading_blanks,
};
use super::key::{KeyDef, KeyOpts, extract_key};

/// Buffer that holds file data, either memory-mapped or heap-allocated.
enum FileData {
    Mmap(Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for FileData {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            FileData::Mmap(m) => m,
            FileData::Owned(v) => v,
        }
    }
}

/// Output writer enum to avoid Box<dyn Write> vtable dispatch overhead.
enum SortOutput<'a> {
    Stdout(BufWriter<io::StdoutLock<'a>>),
    File(BufWriter<File>),
}

impl Write for SortOutput<'_> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            SortOutput::Stdout(w) => w.write(buf),
            SortOutput::File(w) => w.write(buf),
        }
    }
    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            SortOutput::Stdout(w) => w.write_all(buf),
            SortOutput::File(w) => w.write_all(buf),
        }
    }
    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        match self {
            SortOutput::Stdout(w) => w.write_vectored(bufs),
            SortOutput::File(w) => w.write_vectored(bufs),
        }
    }
    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match self {
            SortOutput::Stdout(w) => w.flush(),
            SortOutput::File(w) => w.flush(),
        }
    }
}

/// 4MB buffer for output — reduces flush frequency for large files.
const OUTPUT_BUF_SIZE: usize = 4 * 1024 * 1024;

/// Configuration for a sort operation.
#[derive(Debug, Clone)]
pub struct SortConfig {
    pub keys: Vec<KeyDef>,
    pub separator: Option<u8>,
    pub global_opts: KeyOpts,
    pub unique: bool,
    pub stable: bool,
    pub reverse: bool,
    pub check: CheckMode,
    pub merge: bool,
    pub output_file: Option<String>,
    pub zero_terminated: bool,
    pub parallel: Option<usize>,
    pub buffer_size: Option<usize>,
    pub temp_dir: Option<String>,
    pub random_seed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckMode {
    None,
    Diagnose,
    Quiet,
}

impl Default for SortConfig {
    fn default() -> Self {
        SortConfig {
            keys: Vec::new(),
            separator: None,
            global_opts: KeyOpts::default(),
            unique: false,
            stable: false,
            reverse: false,
            check: CheckMode::None,
            merge: false,
            output_file: None,
            zero_terminated: false,
            parallel: None,
            buffer_size: None,
            temp_dir: None,
            random_seed: 0,
        }
    }
}

/// Compare two lines using the full key chain, global options, and last-resort.
/// Used for sorting (determines order of all lines).
#[inline]
pub fn compare_lines(a: &[u8], b: &[u8], config: &SortConfig) -> Ordering {
    compare_lines_inner(a, b, config, false)
}

/// Compare two lines for dedup (-u): skip last-resort comparison.
/// GNU sort -u considers lines equal when the sort keys match, even if the
/// raw bytes differ (e.g., "Apple" == "apple" with -f, "01" == "1" with -n).
#[inline]
pub fn compare_lines_for_dedup(a: &[u8], b: &[u8], config: &SortConfig) -> Ordering {
    compare_lines_inner(a, b, config, true)
}

#[inline]
fn compare_lines_inner(
    a: &[u8],
    b: &[u8],
    config: &SortConfig,
    skip_last_resort: bool,
) -> Ordering {
    let stable = config.stable || skip_last_resort;
    if !config.keys.is_empty() {
        for key in &config.keys {
            let ka = extract_key(a, key, config.separator);
            let kb = extract_key(b, key, config.separator);

            let opts = if key.opts.has_sort_type()
                || key.opts.ignore_case
                || key.opts.dictionary_order
                || key.opts.ignore_nonprinting
                || key.opts.ignore_leading_blanks
                || key.opts.reverse
            {
                &key.opts
            } else {
                &config.global_opts
            };

            let result = compare_with_opts(ka, kb, opts, config.random_seed);

            if result != Ordering::Equal {
                return result;
            }
        }

        // All keys equal: last-resort comparison (whole line) unless -s or dedup
        if !stable {
            return a.cmp(b);
        }

        Ordering::Equal
    } else {
        // No keys: compare whole line with global opts
        let result = compare_with_opts(a, b, &config.global_opts, config.random_seed);
        // Last-resort whole-line comparison for deterministic order (unless -s or dedup)
        if result == Ordering::Equal && !stable {
            a.cmp(b)
        } else {
            result
        }
    }
}

/// Parallel line boundary detection for large files (>4MB).
/// Splits data into thread-count chunks aligned at delimiter boundaries,
/// then scans each chunk concurrently with SIMD memchr.
fn find_lines_parallel(data: &[u8], delimiter: u8) -> Vec<(usize, usize)> {
    let num_threads = rayon::current_num_threads().max(1);
    let chunk_size = data.len() / num_threads;

    // Find chunk boundaries aligned to delimiter positions
    let mut boundaries = Vec::with_capacity(num_threads + 1);
    boundaries.push(0usize);
    for i in 1..num_threads {
        let target = i * chunk_size;
        if target >= data.len() {
            break;
        }
        if let Some(p) = memchr::memchr(delimiter, &data[target..]) {
            let boundary = target + p + 1;
            if boundary > *boundaries.last().unwrap() && boundary <= data.len() {
                boundaries.push(boundary);
            }
        }
    }
    boundaries.push(data.len());

    let is_newline = delimiter == b'\n';
    let data_len = data.len();

    // Scan each chunk in parallel
    let chunk_offsets: Vec<Vec<(usize, usize)>> = boundaries
        .windows(2)
        .collect::<Vec<_>>()
        .par_iter()
        .map(|w| {
            let chunk_start = w[0];
            let chunk_end = w[1];
            let chunk = &data[chunk_start..chunk_end];
            let mut offsets = Vec::with_capacity(chunk.len() / 40 + 1);
            let mut line_start = chunk_start;

            for pos in memchr::memchr_iter(delimiter, chunk) {
                let abs_pos = chunk_start + pos;
                let mut line_end = abs_pos;
                if is_newline && line_end > line_start && data[line_end - 1] == b'\r' {
                    line_end -= 1;
                }
                offsets.push((line_start, line_end));
                line_start = abs_pos + 1;
            }

            // Handle last line in chunk (only if this is the final chunk)
            if line_start < chunk_end && chunk_end == data_len {
                let mut line_end = chunk_end;
                if is_newline && line_end > line_start && data[line_end - 1] == b'\r' {
                    line_end -= 1;
                }
                offsets.push((line_start, line_end));
            }

            offsets
        })
        .collect();

    let total: usize = chunk_offsets.iter().map(|v| v.len()).sum();
    let mut offsets = Vec::with_capacity(total);
    for chunk in chunk_offsets {
        offsets.extend_from_slice(&chunk);
    }
    offsets
}

/// Read all input into a single contiguous buffer and compute line offsets.
/// Uses mmap for single-file input (zero-copy), Vec for stdin/multi-file.
fn read_all_input(
    inputs: &[String],
    zero_terminated: bool,
) -> io::Result<(FileData, Vec<(usize, usize)>)> {
    let delimiter = if zero_terminated { b'\0' } else { b'\n' };

    // Single file (non-stdin): use mmap directly for zero-copy
    let buffer = if inputs.len() == 1 && inputs[0] != "-" {
        let file = File::open(&inputs[0]).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("open failed: {}: {}", &inputs[0], io_error_msg(&e)),
            )
        })?;
        let metadata = file.metadata()?;
        if metadata.len() > 0 {
            let mmap = unsafe { memmap2::MmapOptions::new().populate().map(&file)? };
            // Sequential for line scanning; caller switches to Random for sort phase
            #[cfg(target_os = "linux")]
            {
                let _ = mmap.advise(memmap2::Advice::Sequential);
                if metadata.len() >= 2 * 1024 * 1024 {
                    let _ = mmap.advise(memmap2::Advice::HugePage);
                }
            }
            FileData::Mmap(mmap)
        } else {
            FileData::Owned(Vec::new())
        }
    } else {
        let mut data = Vec::new();
        for input in inputs {
            if input == "-" {
                io::stdin().lock().read_to_end(&mut data)?;
            } else {
                let mut file = File::open(input).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("open failed: {}: {}", input, io_error_msg(&e)),
                    )
                })?;
                file.read_to_end(&mut data)?;
            }
        }
        FileData::Owned(data)
    };

    // Find line boundaries using SIMD-accelerated memchr
    // Use parallel scanning for large files (>4MB) to leverage multiple cores
    let data = &*buffer;
    let offsets = if data.len() > 4 * 1024 * 1024 {
        find_lines_parallel(data, delimiter)
    } else {
        let mut offsets = Vec::with_capacity(data.len() / 40 + 1);
        let mut start = 0usize;

        for pos in memchr::memchr_iter(delimiter, data) {
            let mut end = pos;
            // Strip trailing CR before LF
            if delimiter == b'\n' && end > start && data[end - 1] == b'\r' {
                end -= 1;
            }
            offsets.push((start, end));
            start = pos + 1;
        }

        // Handle last line without trailing delimiter
        if start < data.len() {
            let mut end = data.len();
            if delimiter == b'\n' && end > start && data[end - 1] == b'\r' {
                end -= 1;
            }
            offsets.push((start, end));
        }
        offsets
    };

    Ok((buffer, offsets))
}

/// Read all lines from inputs (legacy API, used by merge_sorted).
pub fn read_lines(inputs: &[String], zero_terminated: bool) -> io::Result<Vec<Vec<u8>>> {
    let delimiter = if zero_terminated { b'\0' } else { b'\n' };
    let mut lines = Vec::new();

    for input in inputs {
        if input == "-" {
            let stdin = io::stdin();
            let reader = BufReader::new(stdin.lock());
            read_delimited_lines(reader, delimiter, &mut lines)?;
        } else {
            let file = File::open(input).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("open failed: {}: {}", input, io_error_msg(&e)),
                )
            })?;
            let reader = BufReader::with_capacity(256 * 1024, file);
            read_delimited_lines(reader, delimiter, &mut lines)?;
        }
    }

    Ok(lines)
}

fn read_delimited_lines<R: Read>(
    mut reader: BufReader<R>,
    delimiter: u8,
    lines: &mut Vec<Vec<u8>>,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        let n = reader.read_until(delimiter, &mut buf)?;
        if n == 0 {
            break;
        }
        if buf.last() == Some(&delimiter) {
            buf.pop();
        }
        if delimiter == b'\n' && buf.last() == Some(&b'\r') {
            buf.pop();
        }
        lines.push(buf.clone());
    }
    Ok(())
}

/// Check if input is sorted using the optimized buffer path.
pub fn check_sorted(inputs: &[String], config: &SortConfig) -> io::Result<bool> {
    let (buffer, offsets) = read_all_input(inputs, config.zero_terminated)?;
    let data = &*buffer;

    for i in 1..offsets.len() {
        let (s1, e1) = offsets[i - 1];
        let (s2, e2) = offsets[i];
        // For -c -u: use dedup comparison (no last-resort) so that
        // key-equal lines are detected as duplicates.
        // For -c without -u: use full comparison (with last-resort).
        let cmp = if config.unique {
            compare_lines_for_dedup(&data[s1..e1], &data[s2..e2], config)
        } else {
            compare_lines(&data[s1..e1], &data[s2..e2], config)
        };
        let bad = if config.unique {
            cmp != Ordering::Less
        } else {
            cmp == Ordering::Greater
        };
        if bad {
            if config.check == CheckMode::Diagnose {
                let line_display = String::from_utf8_lossy(&data[s2..e2]);
                let filename = if inputs.is_empty() || inputs[0] == "-" {
                    "-"
                } else {
                    &inputs[0]
                };
                eprintln!("sort: {}:{}: disorder: {}", filename, i + 1, line_display);
            }
            return Ok(false);
        }
    }
    Ok(true)
}

/// Entry in the merge BinaryHeap. Stores the line, file index, and a reference
/// to the config for comparison. BinaryHeap is a max-heap, so we reverse the
/// comparison to get a min-heap (smallest element first).
struct MergeEntry {
    line: Vec<u8>,
    file_idx: usize,
    /// Sequence number for stable merge (preserves input order for equal elements).
    seq: u64,
}

/// Merge already-sorted files using a BinaryHeap for O(n log k) performance.
/// Previous implementation used O(k) linear scan per output line.
pub fn merge_sorted(
    inputs: &[String],
    config: &SortConfig,
    writer: &mut impl Write,
) -> io::Result<()> {
    let delimiter = if config.zero_terminated { b'\0' } else { b'\n' };
    let terminator: &[u8] = if config.zero_terminated { b"\0" } else { b"\n" };

    // Open readers for all files
    let mut readers: Vec<Box<dyn BufRead>> = Vec::with_capacity(inputs.len());
    for input in inputs {
        if input == "-" {
            readers.push(Box::new(BufReader::with_capacity(
                256 * 1024,
                io::stdin().lock(),
            )));
        } else {
            let file = File::open(input)?;
            readers.push(Box::new(BufReader::with_capacity(256 * 1024, file)));
        }
    }

    // Helper to read next line from a reader
    let read_next = |reader: &mut dyn BufRead, delim: u8| -> io::Result<Option<Vec<u8>>> {
        let mut buf = Vec::with_capacity(256);
        let n = reader.read_until(delim, &mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        if buf.last() == Some(&delim) {
            buf.pop();
        }
        if delim == b'\n' && buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Ok(Some(buf))
    };

    // Initialize heap with first line from each file
    // BinaryHeap is a max-heap, so we wrap comparison in Reverse-like logic
    let mut seq: u64 = 0;
    let mut heap: BinaryHeap<std::cmp::Reverse<MergeEntryOrd>> =
        BinaryHeap::with_capacity(inputs.len());
    let config_arc = Arc::new(config.clone());

    for (i, reader) in readers.iter_mut().enumerate() {
        if let Some(line) = read_next(reader.as_mut(), delimiter)? {
            heap.push(std::cmp::Reverse(MergeEntryOrd {
                entry: MergeEntry {
                    line,
                    file_idx: i,
                    seq,
                },
                config: Arc::clone(&config_arc),
            }));
            seq += 1;
        }
    }

    let mut prev_line: Option<Vec<u8>> = None;

    while let Some(std::cmp::Reverse(min)) = heap.pop() {
        let should_output = if config.unique {
            match &prev_line {
                Some(prev) => {
                    compare_lines_for_dedup(prev, &min.entry.line, config) != Ordering::Equal
                }
                None => true,
            }
        } else {
            true
        };

        if should_output {
            writer.write_all(&min.entry.line)?;
            writer.write_all(terminator)?;
            if config.unique {
                prev_line = Some(min.entry.line.clone());
            }
        }

        let file_idx = min.entry.file_idx;
        let entry_config = Arc::clone(&min.config);
        if let Some(next_line) = read_next(readers[file_idx].as_mut(), delimiter)? {
            heap.push(std::cmp::Reverse(MergeEntryOrd {
                entry: MergeEntry {
                    line: next_line,
                    file_idx,
                    seq,
                },
                config: entry_config,
            }));
            seq += 1;
        }
    }

    Ok(())
}

/// Wrapper that implements Ord for MergeEntry using SortConfig.
struct MergeEntryOrd {
    entry: MergeEntry,
    config: Arc<SortConfig>,
}

impl PartialEq for MergeEntryOrd {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for MergeEntryOrd {}

impl PartialOrd for MergeEntryOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MergeEntryOrd {
    fn cmp(&self, other: &Self) -> Ordering {
        let ord = compare_lines(&self.entry.line, &other.entry.line, &self.config);
        match ord {
            Ordering::Equal => self.entry.seq.cmp(&other.entry.seq),
            _ => ord,
        }
    }
}

/// Extract an 8-byte prefix from a line for cache-friendly comparison.
/// Big-endian byte order ensures u64 comparison matches lexicographic order.
#[inline]
fn line_prefix(data: &[u8], start: usize, end: usize) -> u64 {
    let len = end - start;
    if len >= 8 {
        // Unaligned u64 load + bswap: single instruction on x86_64
        u64::from_be_bytes(unsafe { *(data.as_ptr().add(start) as *const [u8; 8]) })
    } else {
        let mut bytes = [0u8; 8];
        bytes[..len].copy_from_slice(&data[start..end]);
        u64::from_be_bytes(bytes)
    }
}

/// Extract an 8-byte uppercase prefix for case-insensitive comparison.
/// Big-endian byte order ensures u64 comparison matches lexicographic order.
#[inline]
fn line_prefix_upper(data: &[u8], start: usize, end: usize) -> u64 {
    let len = end - start;
    let mut bytes = [0u8; 8];
    let take = len.min(8);
    for i in 0..take {
        bytes[i] = data[start + i].to_ascii_uppercase();
    }
    u64::from_be_bytes(bytes)
}

/// Pre-extract key byte offsets into the data buffer for all lines.
/// Avoids repeated key extraction during sort comparisons.
/// Parallelized with rayon for large inputs (>10K lines).
fn pre_extract_key_offsets(
    data: &[u8],
    offsets: &[(usize, usize)],
    key: &KeyDef,
    separator: Option<u8>,
) -> Vec<(usize, usize)> {
    let extract = |&(s, e): &(usize, usize)| {
        let line = &data[s..e];
        let extracted = extract_key(line, key, separator);
        if extracted.is_empty() {
            (0, 0)
        } else {
            let offset_in_data = unsafe { extracted.as_ptr().offset_from(data.as_ptr()) as usize };
            (offset_in_data, offset_in_data + extracted.len())
        }
    };

    if offsets.len() > 10_000 {
        offsets.par_iter().map(extract).collect()
    } else {
        offsets.iter().map(extract).collect()
    }
}

/// Select the right numeric parser for pre-parsing.
fn parse_value_for_opts(slice: &[u8], opts: &KeyOpts) -> f64 {
    if opts.general_numeric {
        parse_general_numeric(slice)
    } else if opts.human_numeric {
        parse_human_numeric(slice)
    } else {
        parse_numeric_value(slice)
    }
}

/// Convert f64 to a u64 whose natural ordering matches float ordering.
/// This enables branchless u64::cmp instead of f64::partial_cmp.
/// NaN sorts before all other values (for -g compatibility).
#[inline]
fn float_to_sortable_u64(f: f64) -> u64 {
    if f.is_nan() {
        return 0; // NaN sorts first
    }
    let bits = f.to_bits();
    if (bits >> 63) == 0 {
        bits ^ 0x8000000000000000 // positive: flip sign bit
    } else {
        !bits // negative: flip all bits
    }
}

/// Minimum output size for single-buffer construction.
/// Below this, per-line writes through BufWriter are fine.
const SINGLE_BUF_THRESHOLD: usize = 1024 * 1024; // 1MB

/// Write sorted indices to output, with optional unique dedup.
/// For large outputs (>1MB), builds a single contiguous output buffer and writes
/// it in one call, eliminating per-line write_all overhead (~10M function calls
/// for a 100MB file). The parallel copy phase uses rayon to fill the buffer.
fn write_sorted_output(
    data: &[u8],
    offsets: &[(usize, usize)],
    sorted_indices: &[usize],
    config: &SortConfig,
    writer: &mut impl Write,
    terminator: &[u8],
) -> io::Result<()> {
    if config.unique {
        let mut prev: Option<usize> = None;
        for &idx in sorted_indices {
            let (s, e) = offsets[idx];
            let line = &data[s..e];
            let should_output = match prev {
                Some(p) => {
                    let (ps, pe) = offsets[p];
                    compare_lines_for_dedup(&data[ps..pe], line, config) != Ordering::Equal
                }
                None => true,
            };
            if should_output {
                writer.write_all(line)?;
                writer.write_all(terminator)?;
                prev = Some(idx);
            }
        }
    } else if data.len() >= SINGLE_BUF_THRESHOLD {
        write_sorted_single_buf_idx(data, offsets, sorted_indices, terminator, writer)?;
    } else {
        for &idx in sorted_indices {
            let (s, e) = offsets[idx];
            writer.write_all(&data[s..e])?;
            writer.write_all(terminator)?;
        }
    }
    Ok(())
}

/// Build output from sorted indices using single allocation + parallel fill.
/// One allocation instead of N per-thread allocations reduces mmap/brk overhead.
fn write_sorted_single_buf_idx(
    data: &[u8],
    offsets: &[(usize, usize)],
    sorted_indices: &[usize],
    terminator: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let tl = terminator.len();
    let n = sorted_indices.len();

    if n > 50_000 {
        let num_threads = rayon::current_num_threads().max(1);
        let chunk_size = (n + num_threads - 1) / num_threads;

        // Compute per-chunk sizes in parallel
        let chunk_sizes: Vec<usize> = sorted_indices
            .par_chunks(chunk_size)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|&idx| (offsets[idx].1 - offsets[idx].0) + tl)
                    .sum()
            })
            .collect();

        // Per-thread sub-buffers with pre-computed sizes
        let buffers: Vec<Vec<u8>> = sorted_indices
            .par_chunks(chunk_size)
            .enumerate()
            .map(|(ci, chunk)| {
                let sz = chunk_sizes[ci];
                let mut buf = vec![0u8; sz];
                let buf_ptr = buf.as_mut_ptr();
                let data_ptr = data.as_ptr();
                let mut wp = 0usize;
                for &idx in chunk {
                    let (s, e) = offsets[idx];
                    let ll = e - s;
                    unsafe {
                        std::ptr::copy_nonoverlapping(data_ptr.add(s), buf_ptr.add(wp), ll);
                        wp += ll;
                        std::ptr::copy_nonoverlapping(terminator.as_ptr(), buf_ptr.add(wp), tl);
                        wp += tl;
                    }
                }
                buf
            })
            .collect();

        for buf in &buffers {
            writer.write_all(buf)?;
        }
        return Ok(());
    }

    // Small output: single sequential buffer
    let total: usize = sorted_indices
        .iter()
        .map(|&idx| (offsets[idx].1 - offsets[idx].0) + tl)
        .sum();
    let mut output = vec![0u8; total];
    let out_ptr = output.as_mut_ptr();
    let data_ptr = data.as_ptr();
    let mut wp = 0usize;
    for &idx in sorted_indices {
        let (s, e) = offsets[idx];
        let ll = e - s;
        unsafe {
            std::ptr::copy_nonoverlapping(data_ptr.add(s), out_ptr.add(wp), ll);
            wp += ll;
            std::ptr::copy_nonoverlapping(terminator.as_ptr(), out_ptr.add(wp), tl);
            wp += tl;
        }
    }
    writer.write_all(&output)
}

/// Write sorted (key, index) entries to output. Like write_sorted_output but
/// iterates (u64, usize) entries directly, avoiding the O(n) copy-back to indices.
/// For large outputs: builds a single buffer with parallel memcpy.
fn write_sorted_entries(
    data: &[u8],
    offsets: &[(usize, usize)],
    entries: &[(u64, usize)],
    config: &SortConfig,
    writer: &mut impl Write,
    terminator: &[u8],
) -> io::Result<()> {
    if config.unique {
        let mut prev: Option<usize> = None;
        for &(_, idx) in entries {
            let (s, e) = offsets[idx];
            let line = &data[s..e];
            let should_output = match prev {
                Some(p) => {
                    let (ps, pe) = offsets[p];
                    compare_lines_for_dedup(&data[ps..pe], line, config) != Ordering::Equal
                }
                None => true,
            };
            if should_output {
                writer.write_all(line)?;
                writer.write_all(terminator)?;
                prev = Some(idx);
            }
        }
    } else if data.len() >= SINGLE_BUF_THRESHOLD {
        write_sorted_single_buf_entries(data, offsets, entries, terminator, writer)?;
    } else {
        for &(_, idx) in entries {
            let (s, e) = offsets[idx];
            writer.write_all(&data[s..e])?;
            writer.write_all(terminator)?;
        }
    }
    Ok(())
}

/// Build output from sorted (key, index) entries using single allocation + parallel fill.
/// One allocation instead of N per-thread allocations reduces mmap/brk overhead.
/// Parallel fill via rayon uses prefix-sum offsets for each thread's write region.
fn write_sorted_single_buf_entries(
    data: &[u8],
    offsets: &[(usize, usize)],
    entries: &[(u64, usize)],
    terminator: &[u8],
    writer: &mut impl Write,
) -> io::Result<()> {
    let tl = terminator.len();
    let n = entries.len();

    if n > 50_000 {
        let num_threads = rayon::current_num_threads().max(1);
        let chunk_size = (n + num_threads - 1) / num_threads;

        // Compute per-chunk sizes in parallel
        let chunk_sizes: Vec<usize> = entries
            .par_chunks(chunk_size)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|&(_, idx)| (offsets[idx].1 - offsets[idx].0) + tl)
                    .sum()
            })
            .collect();

        // Per-thread sub-buffers with pre-computed sizes
        let buffers: Vec<Vec<u8>> = entries
            .par_chunks(chunk_size)
            .enumerate()
            .map(|(ci, chunk)| {
                let sz = chunk_sizes[ci];
                let mut buf = vec![0u8; sz];
                let buf_ptr = buf.as_mut_ptr();
                let data_ptr = data.as_ptr();
                let mut wp = 0usize;
                for &(_, idx) in chunk {
                    let (s, e) = offsets[idx];
                    let ll = e - s;
                    unsafe {
                        std::ptr::copy_nonoverlapping(data_ptr.add(s), buf_ptr.add(wp), ll);
                        wp += ll;
                        std::ptr::copy_nonoverlapping(terminator.as_ptr(), buf_ptr.add(wp), tl);
                        wp += tl;
                    }
                }
                buf
            })
            .collect();

        for buf in &buffers {
            writer.write_all(buf)?;
        }
        return Ok(());
    }

    // Small output: single sequential buffer
    let total: usize = entries
        .iter()
        .map(|&(_, idx)| (offsets[idx].1 - offsets[idx].0) + tl)
        .sum();
    let mut output = vec![0u8; total];
    let out_ptr = output.as_mut_ptr();
    let data_ptr = data.as_ptr();
    let mut wp = 0usize;
    for &(_, idx) in entries {
        let (s, e) = offsets[idx];
        let ll = e - s;
        unsafe {
            std::ptr::copy_nonoverlapping(data_ptr.add(s), out_ptr.add(wp), ll);
            wp += ll;
            std::ptr::copy_nonoverlapping(terminator.as_ptr(), out_ptr.add(wp), tl);
            wp += tl;
        }
    }
    writer.write_all(&output)
}

/// Helper: perform a parallel or sequential sort on indices.
fn do_sort(
    indices: &mut [usize],
    stable: bool,
    cmp: impl Fn(&usize, &usize) -> Ordering + Send + Sync,
) {
    let n = indices.len();
    if stable {
        if n > 10_000 {
            indices.par_sort_by(cmp);
        } else {
            indices.sort_by(cmp);
        }
    } else if n > 10_000 {
        indices.par_sort_unstable_by(cmp);
    } else {
        indices.sort_unstable_by(cmp);
    }
}

/// Main sort entry point — high-performance path with specialized fast paths.
///
/// Fast paths:
/// 1. Default lexicographic: prefix-based sort (caches first 8 bytes inline)
/// 2. Numeric/general-numeric/human-numeric (no keys): pre-parses all values
/// 3. Single-key sorts: pre-extracts key offsets + optional numeric pre-parse
/// 4. General: index-based sort with full comparison function
pub fn sort_and_output(inputs: &[String], config: &SortConfig) -> io::Result<()> {
    if let Some(n) = config.parallel {
        let n = n.max(1);
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .ok();
    }

    if config.check != CheckMode::None {
        let sorted = check_sorted(inputs, config)?;
        if !sorted {
            std::process::exit(1);
        }
        return Ok(());
    }

    if config.merge {
        let stdout = io::stdout();
        let mut writer = if let Some(ref path) = config.output_file {
            SortOutput::File(BufWriter::with_capacity(
                OUTPUT_BUF_SIZE,
                File::create(path)?,
            ))
        } else {
            SortOutput::Stdout(BufWriter::with_capacity(OUTPUT_BUF_SIZE, stdout.lock()))
        };
        return merge_sorted(inputs, config, &mut writer);
    }

    // Read all input BEFORE opening output file (supports -o same-file)
    let (buffer, offsets) = read_all_input(inputs, config.zero_terminated)?;
    let data: &[u8] = &buffer;
    let num_lines = offsets.len();

    let stdout = io::stdout();
    let mut writer = if let Some(ref path) = config.output_file {
        SortOutput::File(BufWriter::with_capacity(
            OUTPUT_BUF_SIZE,
            File::create(path)?,
        ))
    } else {
        SortOutput::Stdout(BufWriter::with_capacity(OUTPUT_BUF_SIZE, stdout.lock()))
    };

    if num_lines == 0 {
        return Ok(());
    }

    let terminator: &[u8] = if config.zero_terminated { b"\0" } else { b"\n" };

    // === Already-sorted detection: O(n) scan before sorting ===
    // If data is already in order, skip the O(n log n) sort entirely.
    // This turns the "already sorted" benchmark from 2.1x to near-instant.
    if num_lines > 1 {
        let mut is_sorted = true;
        for i in 1..num_lines {
            let (s1, e1) = offsets[i - 1];
            let (s2, e2) = offsets[i];
            let cmp = compare_lines(&data[s1..e1], &data[s2..e2], config);
            if cmp == Ordering::Greater {
                is_sorted = false;
                break;
            }
        }
        if is_sorted {
            // Zero-copy fast path: write mmap data directly when possible.
            // Conditions: non-unique, newline-terminated, no \r in data.
            if !config.unique && !config.zero_terminated && memchr::memchr(b'\r', data).is_none() {
                if data.last() == Some(&b'\n') {
                    writer.write_all(data)?;
                } else if !data.is_empty() {
                    writer.write_all(data)?;
                    writer.write_all(b"\n")?;
                }
                writer.flush()?;
                return Ok(());
            }

            // Line-by-line output for unique/\r\n/zero-terminated cases
            // Write directly to BufWriter — it already handles batching.
            if config.unique {
                let mut prev: Option<usize> = None;
                for i in 0..num_lines {
                    let (s, e) = offsets[i];
                    let emit = match prev {
                        Some(p) => {
                            let (ps, pe) = offsets[p];
                            compare_lines_for_dedup(&data[ps..pe], &data[s..e], config)
                                != Ordering::Equal
                        }
                        None => true,
                    };
                    if emit {
                        writer.write_all(&data[s..e])?;
                        writer.write_all(terminator)?;
                        prev = Some(i);
                    }
                }
            } else {
                for i in 0..num_lines {
                    let (s, e) = offsets[i];
                    writer.write_all(&data[s..e])?;
                    writer.write_all(terminator)?;
                }
            }
            writer.flush()?;
            return Ok(());
        }
    }

    // Switch to random access for sort phase (comparisons jump to arbitrary lines)
    #[cfg(target_os = "linux")]
    if let FileData::Mmap(ref mmap) = buffer {
        let _ = mmap.advise(memmap2::Advice::Random);
    }

    // Detect sort mode and use specialized fast path
    let no_keys = config.keys.is_empty();
    let gopts = &config.global_opts;

    let is_plain_lex = no_keys
        && !gopts.has_sort_type()
        && !gopts.dictionary_order
        && !gopts.ignore_case
        && !gopts.ignore_nonprinting
        && !gopts.ignore_leading_blanks;

    // Case-insensitive lexicographic (sort -f, optionally with -b)
    let is_fold_case_lex = no_keys
        && !gopts.has_sort_type()
        && !gopts.dictionary_order
        && gopts.ignore_case
        && !gopts.ignore_nonprinting;

    let is_numeric_only = no_keys
        && (gopts.numeric || gopts.general_numeric || gopts.human_numeric)
        && !gopts.dictionary_order
        && !gopts.ignore_case
        && !gopts.ignore_nonprinting;

    let is_single_key = config.keys.len() == 1;

    if is_plain_lex && num_lines > 256 {
        // FAST PATH 1: Prefix-based lexicographic sort
        let reverse = gopts.reverse;
        let mut entries: Vec<(u64, usize)> = if num_lines > 10_000 {
            offsets
                .par_iter()
                .enumerate()
                .map(|(i, &(s, e))| (line_prefix(data, s, e), i))
                .collect()
        } else {
            offsets
                .iter()
                .enumerate()
                .map(|(i, &(s, e))| (line_prefix(data, s, e), i))
                .collect()
        };

        // Parallel prefix-comparison sort: uses all CPU cores via rayon.
        // The u64 prefix resolves ~95% of comparisons without touching line data,
        // so the effective comparison cost is ~2ns. On 8 cores with 5M lines:
        // ~5M * 22 / 8 = ~14M comparisons/core * 2.4ns = ~34ms total.
        // This beats sequential radix sort (~200ms for 4 passes with random writes).
        let prefix_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
            let ord = match a.0.cmp(&b.0) {
                Ordering::Equal => {
                    let (sa, ea) = offsets[a.1];
                    let (sb, eb) = offsets[b.1];
                    // Skip first 8 bytes (already compared via u64 prefix)
                    let skip = 8.min(ea - sa).min(eb - sb);
                    data[sa + skip..ea].cmp(&data[sb + skip..eb])
                }
                ord => ord,
            };
            if reverse { ord.reverse() } else { ord }
        };
        let n = entries.len();
        if n > 10_000 {
            entries.par_sort_unstable_by(prefix_cmp);
        } else {
            entries.sort_unstable_by(prefix_cmp);
        }

        // Switch to sequential for output phase
        #[cfg(target_os = "linux")]
        if let FileData::Mmap(ref mmap) = buffer {
            let _ = mmap.advise(memmap2::Advice::Sequential);
        }

        // Output phase: parallel buffer construction for large, per-line for small
        if config.unique {
            let mut prev: Option<usize> = None;
            for &(_, idx) in &entries {
                let (s, e) = offsets[idx];
                let line = &data[s..e];
                let emit = match prev {
                    Some(p) => {
                        let (ps, pe) = offsets[p];
                        data[ps..pe] != *line
                    }
                    None => true,
                };
                if emit {
                    writer.write_all(line)?;
                    writer.write_all(terminator)?;
                    prev = Some(idx);
                }
            }
        } else {
            write_sorted_single_buf_entries(data, &offsets, &entries, terminator, &mut writer)?;
        }
    } else if is_fold_case_lex && num_lines > 256 {
        // FAST PATH 1b: Case-insensitive prefix sort (sort -f)
        // Pre-computes uppercase 8-byte prefix, radix sorts on it.
        // Avoids per-comparison case folding in O(n log n) comparisons.
        let reverse = gopts.reverse;
        let needs_blank = gopts.ignore_leading_blanks;
        let mut entries: Vec<(u64, usize)> = if num_lines > 10_000 {
            offsets
                .par_iter()
                .enumerate()
                .map(|(i, &(s, e))| {
                    let (s, e) = if needs_blank {
                        let trimmed = super::compare::skip_leading_blanks(&data[s..e]);
                        let new_s = unsafe { trimmed.as_ptr().offset_from(data.as_ptr()) as usize };
                        (new_s, new_s + trimmed.len())
                    } else {
                        (s, e)
                    };
                    (line_prefix_upper(data, s, e), i)
                })
                .collect()
        } else {
            offsets
                .iter()
                .enumerate()
                .map(|(i, &(s, e))| {
                    let (s, e) = if needs_blank {
                        let trimmed = super::compare::skip_leading_blanks(&data[s..e]);
                        let new_s = unsafe { trimmed.as_ptr().offset_from(data.as_ptr()) as usize };
                        (new_s, new_s + trimmed.len())
                    } else {
                        (s, e)
                    };
                    (line_prefix_upper(data, s, e), i)
                })
                .collect()
        };

        let n = entries.len();
        let stable = config.stable;
        // Parallel case-insensitive sort: u64 prefix resolves most comparisons,
        // falls back to full case-insensitive compare, then raw line compare.
        let fold_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
            let la = &data[offsets[a.1].0..offsets[a.1].1];
            let lb = &data[offsets[b.1].0..offsets[b.1].1];
            let la = if needs_blank {
                super::compare::skip_leading_blanks(la)
            } else {
                la
            };
            let lb = if needs_blank {
                super::compare::skip_leading_blanks(lb)
            } else {
                lb
            };
            let ord = match a.0.cmp(&b.0) {
                Ordering::Equal => super::compare::compare_ignore_case(la, lb),
                ord => ord,
            };
            let ord = if reverse { ord.reverse() } else { ord };
            if ord == Ordering::Equal && !stable {
                data[offsets[a.1].0..offsets[a.1].1].cmp(&data[offsets[b.1].0..offsets[b.1].1])
            } else {
                ord
            }
        };
        if n > 10_000 {
            if stable {
                entries.par_sort_by(fold_cmp);
            } else {
                entries.par_sort_unstable_by(fold_cmp);
            }
        } else if stable {
            entries.sort_by(fold_cmp);
        } else {
            entries.sort_unstable_by(fold_cmp);
        }

        write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
    } else if is_numeric_only {
        // FAST PATH 2: Pre-parsed numeric sort with u64 comparison
        // float_to_sortable_u64 enables branchless u64::cmp instead of f64::partial_cmp
        // Parallelize numeric parsing for large inputs (parsing is CPU-bound)
        let mut entries: Vec<(u64, usize)> = if num_lines > 10_000 {
            offsets
                .par_iter()
                .enumerate()
                .map(|(i, &(s, e))| {
                    (
                        float_to_sortable_u64(parse_value_for_opts(&data[s..e], gopts)),
                        i,
                    )
                })
                .collect()
        } else {
            offsets
                .iter()
                .enumerate()
                .map(|(i, &(s, e))| {
                    (
                        float_to_sortable_u64(parse_value_for_opts(&data[s..e], gopts)),
                        i,
                    )
                })
                .collect()
        };
        let reverse = gopts.reverse;
        let stable = config.stable;

        // Parallel sort on pre-parsed numeric values (u64-encoded floats).
        // par_sort scales across cores; radix sort's random writes hurt cache.
        let cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
            let ord = a.0.cmp(&b.0);
            if ord != Ordering::Equal {
                return if reverse { ord.reverse() } else { ord };
            }
            if !stable {
                data[offsets[a.1].0..offsets[a.1].1].cmp(&data[offsets[b.1].0..offsets[b.1].1])
            } else {
                Ordering::Equal
            }
        };
        let n = entries.len();
        if n > 10_000 {
            if stable {
                entries.par_sort_by(cmp);
            } else {
                entries.par_sort_unstable_by(cmp);
            }
        } else if stable {
            entries.sort_by(cmp);
        } else {
            entries.sort_unstable_by(cmp);
        }

        write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
    } else if is_single_key {
        // FAST PATH 3: Single-key sort with pre-extracted key offsets
        let key = &config.keys[0];
        let opts = if key.opts.has_sort_type()
            || key.opts.ignore_case
            || key.opts.dictionary_order
            || key.opts.ignore_nonprinting
            || key.opts.ignore_leading_blanks
            || key.opts.reverse
        {
            &key.opts
        } else {
            gopts
        };

        let key_offs = pre_extract_key_offsets(data, &offsets, key, config.separator);
        let is_key_numeric = opts.numeric || opts.general_numeric || opts.human_numeric;

        if is_key_numeric {
            // Single key, numeric: u64-based branchless comparison
            // Parallelize numeric parsing for large inputs
            let is_gen = opts.general_numeric;
            let parse_entry = |i: usize, &(s, e): &(usize, usize)| {
                let f = if s == e {
                    if is_gen { f64::NAN } else { 0.0 }
                } else {
                    parse_value_for_opts(&data[s..e], opts)
                };
                (float_to_sortable_u64(f), i)
            };
            let mut entries: Vec<(u64, usize)> = if num_lines > 10_000 {
                key_offs
                    .par_iter()
                    .enumerate()
                    .map(|(i, ko)| parse_entry(i, ko))
                    .collect()
            } else {
                key_offs
                    .iter()
                    .enumerate()
                    .map(|(i, ko)| parse_entry(i, ko))
                    .collect()
            };
            let reverse = opts.reverse;
            let stable = config.stable;

            // Parallel sort on single-key numeric values
            let n = entries.len();
            let cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                let ord = a.0.cmp(&b.0);
                if ord != Ordering::Equal {
                    return if reverse { ord.reverse() } else { ord };
                }
                if !stable {
                    data[offsets[a.1].0..offsets[a.1].1].cmp(&data[offsets[b.1].0..offsets[b.1].1])
                } else {
                    Ordering::Equal
                }
            };
            if n > 10_000 {
                if stable {
                    entries.par_sort_by(cmp);
                } else {
                    entries.par_sort_unstable_by(cmp);
                }
            } else if stable {
                entries.sort_by(cmp);
            } else {
                entries.sort_unstable_by(cmp);
            }

            write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
        } else {
            // Single key, non-numeric: direct comparison of pre-extracted keys
            let stable = config.stable;
            let reverse = opts.reverse;
            let random_seed = config.random_seed;
            let has_flags = opts.dictionary_order
                || opts.ignore_case
                || opts.ignore_nonprinting
                || opts.ignore_leading_blanks
                || opts.has_sort_type();

            if !has_flags {
                // Prefix-based sort: cache 8-byte key prefix as u64
                // Most comparisons resolve at the u64 level (no data[] access)
                let pfx = |i: usize, &(s, e): &(usize, usize)| -> (u64, usize) {
                    (if s < e { line_prefix(data, s, e) } else { 0u64 }, i)
                };
                let mut entries: Vec<(u64, usize)> = if num_lines > 10_000 {
                    key_offs
                        .par_iter()
                        .enumerate()
                        .map(|(i, ko)| pfx(i, ko))
                        .collect()
                } else {
                    key_offs
                        .iter()
                        .enumerate()
                        .map(|(i, ko)| pfx(i, ko))
                        .collect()
                };

                // Parallel prefix-comparison sort for single-key path
                let n = entries.len();
                let prefix_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                    let ord = match a.0.cmp(&b.0) {
                        Ordering::Equal => {
                            let (sa, ea) = key_offs[a.1];
                            let (sb, eb) = key_offs[b.1];
                            // Skip first 8 bytes (already compared via u64 prefix)
                            let skip = 8.min(ea - sa).min(eb - sb);
                            data[sa + skip..ea].cmp(&data[sb + skip..eb])
                        }
                        ord => ord,
                    };
                    if ord != Ordering::Equal {
                        return if reverse { ord.reverse() } else { ord };
                    }
                    if !stable {
                        data[offsets[a.1].0..offsets[a.1].1]
                            .cmp(&data[offsets[b.1].0..offsets[b.1].1])
                    } else {
                        Ordering::Equal
                    }
                };
                if n > 10_000 {
                    if stable {
                        entries.par_sort_by(prefix_cmp);
                    } else {
                        entries.par_sort_unstable_by(prefix_cmp);
                    }
                } else if stable {
                    entries.sort_by(prefix_cmp);
                } else {
                    entries.sort_unstable_by(prefix_cmp);
                }

                write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
            } else {
                // Pre-select comparator: eliminates per-comparison option branching
                let mut indices: Vec<usize> = (0..num_lines).collect();
                let (cmp_fn, needs_blank, needs_reverse) = select_comparator(opts, random_seed);
                do_sort(&mut indices, stable, |&a, &b| {
                    let (sa, ea) = key_offs[a];
                    let (sb, eb) = key_offs[b];
                    let ka = if sa == ea {
                        &[] as &[u8]
                    } else if needs_blank {
                        skip_leading_blanks(&data[sa..ea])
                    } else {
                        &data[sa..ea]
                    };
                    let kb = if sb == eb {
                        &[] as &[u8]
                    } else if needs_blank {
                        skip_leading_blanks(&data[sb..eb])
                    } else {
                        &data[sb..eb]
                    };
                    let ord = cmp_fn(ka, kb);
                    let ord = if needs_reverse { ord.reverse() } else { ord };
                    if ord == Ordering::Equal && !stable {
                        data[offsets[a].0..offsets[a].1].cmp(&data[offsets[b].0..offsets[b].1])
                    } else {
                        ord
                    }
                });
                write_sorted_output(data, &offsets, &indices, config, &mut writer, terminator)?;
            }
        }
    } else if config.keys.len() > 1 {
        // FAST PATH 4: Multi-key sort with pre-extracted key offsets for ALL keys.
        // Eliminates per-comparison key extraction (O(n log n) calls to extract_key).
        let mut indices: Vec<usize> = (0..num_lines).collect();
        let all_key_offs: Vec<Vec<(usize, usize)>> = if config.keys.len() > 1 && num_lines > 10_000
        {
            config
                .keys
                .par_iter()
                .map(|key| pre_extract_key_offsets(data, &offsets, key, config.separator))
                .collect()
        } else {
            config
                .keys
                .iter()
                .map(|key| pre_extract_key_offsets(data, &offsets, key, config.separator))
                .collect()
        };

        let stable = config.stable;
        let random_seed = config.random_seed;
        let keys = &config.keys;
        let global_opts = &config.global_opts;

        // Pre-select comparators: eliminates per-comparison option branching
        let comparators: Vec<_> = keys
            .iter()
            .map(|key| {
                let opts = if key.opts.has_sort_type()
                    || key.opts.ignore_case
                    || key.opts.dictionary_order
                    || key.opts.ignore_nonprinting
                    || key.opts.ignore_leading_blanks
                    || key.opts.reverse
                {
                    &key.opts
                } else {
                    global_opts
                };
                select_comparator(opts, random_seed)
            })
            .collect();

        do_sort(&mut indices, stable, |&a, &b| {
            for (ki, &(cmp_fn, needs_blank, needs_reverse)) in comparators.iter().enumerate() {
                let (sa, ea) = all_key_offs[ki][a];
                let (sb, eb) = all_key_offs[ki][b];
                let ka = if sa == ea {
                    &[] as &[u8]
                } else if needs_blank {
                    skip_leading_blanks(&data[sa..ea])
                } else {
                    &data[sa..ea]
                };
                let kb = if sb == eb {
                    &[] as &[u8]
                } else if needs_blank {
                    skip_leading_blanks(&data[sb..eb])
                } else {
                    &data[sb..eb]
                };

                let result = cmp_fn(ka, kb);
                let result = if needs_reverse {
                    result.reverse()
                } else {
                    result
                };
                if result != Ordering::Equal {
                    return result;
                }
            }

            // All keys equal: last-resort whole-line comparison unless stable
            if !stable {
                data[offsets[a].0..offsets[a].1].cmp(&data[offsets[b].0..offsets[b].1])
            } else {
                Ordering::Equal
            }
        });

        write_sorted_output(data, &offsets, &indices, config, &mut writer, terminator)?;
    } else if !config.keys.is_empty() {
        // GENERAL PATH: Key-based sort with pre-selected comparators (fallback for unusual configs)
        let mut indices: Vec<usize> = (0..num_lines).collect();
        let stable = config.stable;
        let random_seed = config.random_seed;
        let keys = &config.keys;
        let global_opts = &config.global_opts;

        let comparators: Vec<_> = keys
            .iter()
            .map(|key| {
                let opts = if key.opts.has_any_option() {
                    &key.opts
                } else {
                    global_opts
                };
                select_comparator(opts, random_seed)
            })
            .collect();

        do_sort(&mut indices, stable, |&a, &b| {
            let la = &data[offsets[a].0..offsets[a].1];
            let lb = &data[offsets[b].0..offsets[b].1];

            for (ki, &(cmp_fn, needs_blank, needs_reverse)) in comparators.iter().enumerate() {
                let ka = extract_key(la, &keys[ki], config.separator);
                let kb = extract_key(lb, &keys[ki], config.separator);
                let ka = if needs_blank {
                    skip_leading_blanks(ka)
                } else {
                    ka
                };
                let kb = if needs_blank {
                    skip_leading_blanks(kb)
                } else {
                    kb
                };
                let result = cmp_fn(ka, kb);
                let result = if needs_reverse {
                    result.reverse()
                } else {
                    result
                };
                if result != Ordering::Equal {
                    return result;
                }
            }
            if !stable { la.cmp(lb) } else { Ordering::Equal }
        });

        write_sorted_output(data, &offsets, &indices, config, &mut writer, terminator)?;
    } else {
        // GENERAL PATH: No keys, non-lex sort with pre-selected comparator
        let mut indices: Vec<usize> = (0..num_lines).collect();
        let (cmp_fn, needs_blank, needs_reverse) =
            select_comparator(&config.global_opts, config.random_seed);
        let stable = config.stable;

        do_sort(&mut indices, stable, |&a, &b| {
            let la = &data[offsets[a].0..offsets[a].1];
            let lb = &data[offsets[b].0..offsets[b].1];
            let la = if needs_blank {
                skip_leading_blanks(la)
            } else {
                la
            };
            let lb = if needs_blank {
                skip_leading_blanks(lb)
            } else {
                lb
            };
            let ord = cmp_fn(la, lb);
            let ord = if needs_reverse { ord.reverse() } else { ord };
            // Last-resort whole-line comparison for deterministic order (unless -s)
            if ord == Ordering::Equal && !stable {
                data[offsets[a].0..offsets[a].1].cmp(&data[offsets[b].0..offsets[b].1])
            } else {
                ord
            }
        });

        write_sorted_output(data, &offsets, &indices, config, &mut writer, terminator)?;
    }

    writer.flush()?;
    Ok(())
}

/// Parse a buffer size string like "10K", "1M", "1G".
pub fn parse_buffer_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty buffer size".to_string());
    }

    let (num_part, suffix) = if s.ends_with(|c: char| c.is_ascii_alphabetic()) {
        let (n, s) = s.split_at(s.len() - 1);
        (n, s.chars().next())
    } else {
        (s, None)
    };

    let base: usize = num_part
        .parse()
        .map_err(|_| format!("invalid buffer size: {}", s))?;

    let multiplier = match suffix {
        Some('K') | Some('k') => 1024,
        Some('M') | Some('m') => 1024 * 1024,
        Some('G') | Some('g') => 1024 * 1024 * 1024,
        Some('T') | Some('t') => 1024usize.pow(4),
        Some('b') => 512,
        Some(c) => return Err(format!("invalid suffix '{}' in buffer size", c)),
        None => 1,
    };

    Ok(base * multiplier)
}
