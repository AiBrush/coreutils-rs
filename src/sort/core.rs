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
    compare_with_opts, int_to_sortable_u64, parse_general_numeric, parse_human_numeric,
    parse_numeric_value, select_comparator, skip_leading_blanks, try_parse_integer,
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
/// Uses raw fd for stdout on Unix to bypass StdoutLock overhead.
enum SortOutput {
    /// Raw fd stdout wrapped in BufWriter. The File is leaked (ManuallyDrop)
    /// to avoid closing fd 1 when the sort is done.
    Stdout(BufWriter<File>),
    File(BufWriter<File>),
}

impl SortOutput {
    /// Create stdout output using raw fd on Unix for lower overhead.
    fn stdout() -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::io::FromRawFd;
            // SAFETY: fd 1 is stdout, ManuallyDrop ensures we don't close it.
            // We create a File from the raw fd; BufWriter takes ownership.
            // The File will be dropped when BufWriter is dropped, but since
            // this is in main() and we process::exit(), that's fine.
            // The fd itself stays open because the OS manages it.
            let file = unsafe { File::from_raw_fd(1) };
            SortOutput::Stdout(BufWriter::with_capacity(OUTPUT_BUF_SIZE, file))
        }
        #[cfg(not(unix))]
        {
            // On non-Unix platforms, fall back to opening /dev/stdout or CON.
            // This is a best-effort stub; Windows support is not a primary target.
            let path = if cfg!(windows) { "CON" } else { "/dev/stdout" };
            SortOutput::Stdout(BufWriter::with_capacity(
                OUTPUT_BUF_SIZE,
                File::open(path).unwrap_or_else(|_| panic!("Cannot open stdout")),
            ))
        }
    }
}

impl SortOutput {
    /// Write directly to the underlying fd, bypassing BufWriter buffering.
    /// Flushes any pending buffered data first. Use when a contiguous buffer
    /// is already assembled — avoids the extra memcpy into BufWriter's internal buffer.
    #[inline]
    fn write_all_direct(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            SortOutput::Stdout(w) | SortOutput::File(w) => {
                w.flush()?;
                w.get_mut().write_all(buf)
            }
        }
    }
}

impl Write for SortOutput {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            SortOutput::Stdout(w) | SortOutput::File(w) => w.write(buf),
        }
    }
    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            SortOutput::Stdout(w) | SortOutput::File(w) => w.write_all(buf),
        }
    }
    #[inline]
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        match self {
            SortOutput::Stdout(w) | SortOutput::File(w) => w.write_vectored(bufs),
        }
    }
    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match self {
            SortOutput::Stdout(w) | SortOutput::File(w) => w.flush(),
        }
    }
}

/// 16MB buffer for output — reduces flush frequency for large files.
/// Larger buffer reduces write() syscall count which is significant for 100MB+ inputs.
/// 16MB stays within L3 cache on modern CPUs while significantly reducing syscalls.
const OUTPUT_BUF_SIZE: usize = 16 * 1024 * 1024;

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

    // Scan each chunk in parallel.
    // Uses raw pointer arithmetic (via data_addr usize) to eliminate bounds checking
    // in the \r\n detection hot path.
    let data_addr = data.as_ptr() as usize;
    let chunk_offsets: Vec<Vec<(usize, usize)>> = boundaries
        .windows(2)
        .collect::<Vec<_>>()
        .par_iter()
        .map(|w| {
            let chunk_start = w[0];
            let chunk_end = w[1];
            let dp = data_addr as *const u8;
            let chunk =
                unsafe { std::slice::from_raw_parts(dp.add(chunk_start), chunk_end - chunk_start) };
            let mut offsets = Vec::with_capacity(chunk.len() / 40 + 1);
            let mut line_start = chunk_start;

            for pos in memchr::memchr_iter(delimiter, chunk) {
                let abs_pos = chunk_start + pos;
                let mut line_end = abs_pos;
                if is_newline && line_end > line_start && unsafe { *dp.add(line_end - 1) } == b'\r'
                {
                    line_end -= 1;
                }
                offsets.push((line_start, line_end));
                line_start = abs_pos + 1;
            }

            // Handle last line in chunk (only if this is the final chunk)
            if line_start < chunk_end && chunk_end == data_len {
                let mut line_end = chunk_end;
                if is_newline && line_end > line_start && unsafe { *dp.add(line_end - 1) } == b'\r'
                {
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
            // Advise kernel for optimal page handling
            #[cfg(target_os = "linux")]
            {
                let _ = mmap.advise(memmap2::Advice::WillNeed);
                if metadata.len() >= 2 * 1024 * 1024 {
                    let _ = mmap.advise(memmap2::Advice::HugePage);
                }
            }
            FileData::Mmap(mmap)
        } else {
            FileData::Owned(Vec::new())
        }
    } else if inputs.len() == 1 && inputs[0] == "-" {
        // Single stdin: use read_stdin() directly without extra copy.
        // read_stdin() returns a Vec that we can use directly, avoiding the
        // extend_from_slice copy that would happen with the multi-file path.
        let stdin_data = crate::common::io::read_stdin()?;
        FileData::Owned(stdin_data)
    } else {
        // Multi-file or mixed file+stdin: concatenate into a single buffer.
        let mut data = Vec::new();
        for input in inputs {
            if input == "-" {
                let stdin_data = crate::common::io::read_stdin()?;
                if data.is_empty() {
                    data = stdin_data;
                } else {
                    data.extend_from_slice(&stdin_data);
                }
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
    let offsets = if data.len() > 2 * 1024 * 1024 {
        find_lines_parallel(data, delimiter)
    } else {
        let dp = data.as_ptr();
        let mut offsets = Vec::with_capacity(data.len() / 40 + 1);
        let mut start = 0usize;

        for pos in memchr::memchr_iter(delimiter, data) {
            let mut end = pos;
            // Strip trailing CR before LF (raw pointer to avoid bounds check)
            if delimiter == b'\n' && end > start && unsafe { *dp.add(end - 1) } == b'\r' {
                end -= 1;
            }
            offsets.push((start, end));
            start = pos + 1;
        }

        // Handle last line without trailing delimiter
        if start < data.len() {
            let mut end = data.len();
            if delimiter == b'\n' && end > start && unsafe { *dp.add(end - 1) } == b'\r' {
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

    let dp = data.as_ptr();
    for i in 1..offsets.len() {
        let (s1, e1) = offsets[i - 1];
        let (s2, e2) = offsets[i];
        // Use raw pointer arithmetic to avoid bounds checking
        let line1 = unsafe { std::slice::from_raw_parts(dp.add(s1), e1 - s1) };
        let line2 = unsafe { std::slice::from_raw_parts(dp.add(s2), e2 - s2) };
        // For -c -u: use dedup comparison (no last-resort) so that
        // key-equal lines are detected as duplicates.
        // For -c without -u: use full comparison (with last-resort).
        let cmp = if config.unique {
            compare_lines_for_dedup(line1, line2, config)
        } else {
            compare_lines(line1, line2, config)
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
    // Batch merge output to reduce write_all call overhead
    const MERGE_BATCH: usize = 256 * 1024;
    let mut batch_buf: Vec<u8> = Vec::with_capacity(MERGE_BATCH);

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
            batch_buf.extend_from_slice(&min.entry.line);
            batch_buf.extend_from_slice(terminator);
            if batch_buf.len() >= MERGE_BATCH {
                writer.write_all(&batch_buf)?;
                batch_buf.clear();
            }
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

    if !batch_buf.is_empty() {
        writer.write_all(&batch_buf)?;
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
/// Software prefetch a cache line for reading.
/// Hides memory latency by loading data into L1 cache before it's needed.
#[inline(always)]
fn prefetch_read(ptr: *const u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_prefetch(ptr as *const i8, std::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("prfm pldl1keep, [{x}]", x = in(reg) ptr, options(nostack, preserves_flags));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = ptr;
}

/// Big-endian byte order ensures u64 comparison matches lexicographic order.
#[inline]
fn line_prefix(data: &[u8], start: usize, end: usize) -> u64 {
    let len = end - start;
    if len >= 8 {
        // Unaligned u64 load + bswap: single instruction on x86_64
        u64::from_be_bytes(unsafe { *(data.as_ptr().add(start) as *const [u8; 8]) })
    } else {
        // Use raw pointer copy for short lines to avoid bounds checking
        let mut bytes = [0u8; 8];
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(start), bytes.as_mut_ptr(), len);
        }
        u64::from_be_bytes(bytes)
    }
}

/// Extract an 8-byte uppercase prefix for case-insensitive comparison.
/// Big-endian byte order ensures u64 comparison matches lexicographic order.
/// Uses raw pointer access to eliminate bounds checking in the hot path.
#[inline]
fn line_prefix_upper(data: &[u8], start: usize, end: usize) -> u64 {
    let len = end - start;
    let mut bytes = [0u8; 8];
    let take = len.min(8);
    let p = data.as_ptr();
    // Copy and uppercase in a single pass with raw pointers
    let mut i = 0usize;
    while i < take {
        let b = unsafe { *p.add(start + i) };
        bytes[i] = if b >= b'a' && b <= b'z' { b - 32 } else { b };
        i += 1;
    }
    u64::from_be_bytes(bytes)
}

/// Pre-extract key byte offsets into the data buffer for all lines.
/// Avoids repeated key extraction during sort comparisons.
/// Parallelized with rayon for large inputs (>10K lines).
///
/// Specialized fast path for `-t SEP -k N` (whole Nth field, no char offsets):
/// uses direct memchr calls instead of the general extract_key machinery.
fn pre_extract_key_offsets(
    data: &[u8],
    offsets: &[(usize, usize)],
    key: &KeyDef,
    separator: Option<u8>,
) -> Vec<(usize, usize)> {
    // Fast path: separator-based single whole field extraction (e.g., -t, -k2 or -t, -k2,2)
    // No char offsets, and end_field is either 0 (to end of line) or same as start_field.
    // This avoids the overhead of extract_key's general field/char computation.
    let is_whole_field = separator.is_some()
        && key.start_char == 0
        && key.end_char == 0
        && (key.end_field == 0 || key.end_field == key.start_field);
    if is_whole_field {
        let sep = separator.unwrap();
        let field_idx = key.start_field.saturating_sub(1);
        let to_end = key.end_field == 0; // -kN means from field N to end of line
        let extract_fast = move |&(s, e): &(usize, usize)| {
            let line = &data[s..e];
            // Find start of the target field
            let mut fstart = 0usize;
            for _ in 0..field_idx {
                match memchr::memchr(sep, &line[fstart..]) {
                    Some(pos) => fstart = fstart + pos + 1,
                    None => return (0usize, 0usize),
                }
            }
            if fstart > line.len() {
                return (0, 0);
            }
            if to_end {
                // -kN: from field N to end of line
                (s + fstart, e)
            } else {
                // -kN,N: just field N
                match memchr::memchr(sep, &line[fstart..]) {
                    Some(pos) => (s + fstart, s + fstart + pos),
                    None => (s + fstart, e),
                }
            }
        };

        return if offsets.len() > 10_000 {
            offsets.par_iter().map(extract_fast).collect()
        } else {
            offsets.iter().map(extract_fast).collect()
        };
    }

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

/// Write sorted indices to output, with optional unique dedup.
/// Zero-copy writev: writes directly from mmap data through BufWriter.
/// Eliminates ~110MB intermediate buffer allocation for 100MB files.
fn write_sorted_output(
    data: &[u8],
    offsets: &[(usize, usize)],
    sorted_indices: &[usize],
    config: &SortConfig,
    writer: &mut SortOutput,
    terminator: &[u8],
) -> io::Result<()> {
    let dp = data.as_ptr();
    let n = sorted_indices.len();
    let term_byte = terminator[0];
    if config.unique {
        let buf_cap = data.len() + n + 1;
        let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
        let bptr = buf.as_mut_ptr();
        let mut pos = 0usize;
        let mut prev: Option<usize> = None;
        for &idx in sorted_indices {
            let (s, e) = offsets[idx];
            let len = e - s;
            let should_output = match prev {
                Some(p) => {
                    let (ps, pe) = offsets[p];
                    let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                    let line = unsafe { std::slice::from_raw_parts(dp.add(s), len) };
                    compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                }
                None => true,
            };
            if should_output {
                unsafe {
                    std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                    *bptr.add(pos + len) = term_byte;
                }
                pos += len + 1;
                prev = Some(idx);
            }
        }
        unsafe {
            buf.set_len(pos);
        }
        writer.write_all_direct(&buf)?;
    } else {
        let total_size = if data.last() == Some(&term_byte) {
            data.len()
        } else {
            data.len() + 1
        };
        let mut buf: Vec<u8> = Vec::with_capacity(total_size);
        let bptr = buf.as_mut_ptr();
        let mut pos = 0usize;
        for j in 0..n {
            if j + 16 < n {
                let (ps, _) = offsets[sorted_indices[j + 16]];
                prefetch_read(unsafe { dp.add(ps) });
            }
            let (s, e) = offsets[sorted_indices[j]];
            let len = e - s;
            unsafe {
                std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                *bptr.add(pos + len) = term_byte;
            }
            pos += len + 1;
        }
        unsafe {
            buf.set_len(pos);
        }
        writer.write_all_direct(&buf)?;
    }
    Ok(())
}

/// Write sorted (key, index) entries to output using contiguous buffer.
/// Copies lines in sorted order into a buffer, then single write_all.
/// Eliminates IoSlice per-entry overhead and writev syscall count.
fn write_sorted_entries(
    data: &[u8],
    offsets: &[(usize, usize)],
    entries: &[(u64, usize)],
    config: &SortConfig,
    writer: &mut SortOutput,
    terminator: &[u8],
) -> io::Result<()> {
    let dp = data.as_ptr();
    let n = entries.len();
    let term_byte = terminator[0];
    if config.unique {
        let buf_cap = data.len() + n + 1;
        let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
        let bptr = buf.as_mut_ptr();
        let mut pos = 0usize;
        let mut prev: Option<usize> = None;
        for &(_, idx) in entries {
            let (s, e) = offsets[idx];
            let len = e - s;
            let should_output = match prev {
                Some(p) => {
                    let (ps, pe) = offsets[p];
                    let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                    let line = unsafe { std::slice::from_raw_parts(dp.add(s), len) };
                    compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                }
                None => true,
            };
            if should_output {
                unsafe {
                    std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                    *bptr.add(pos + len) = term_byte;
                }
                pos += len + 1;
                prev = Some(idx);
            }
        }
        unsafe {
            buf.set_len(pos);
        }
        writer.write_all_direct(&buf)?;
    } else {
        let total_size = if data.last() == Some(&term_byte) {
            data.len()
        } else {
            data.len() + 1
        };
        let mut buf: Vec<u8> = Vec::with_capacity(total_size);
        let bptr = buf.as_mut_ptr();
        let mut pos = 0usize;
        for j in 0..n {
            if j + 16 < n {
                let (ps, _) = offsets[entries[j + 16].1];
                prefetch_read(unsafe { dp.add(ps) });
            }
            let (s, e) = offsets[entries[j].1];
            let len = e - s;
            unsafe {
                std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                *bptr.add(pos + len) = term_byte;
            }
            pos += len + 1;
        }
        unsafe {
            buf.set_len(pos);
        }
        writer.write_all_direct(&buf)?;
    }
    Ok(())
}

/// Full 4-pass LSD (Least Significant Digit) radix sort for (u64, usize) entries.
/// Sorts purely by the u64 key in O(n) time with ZERO comparisons.
/// Uses 256-bucket passes on each byte of the u64 (4 passes on 16-bit groups).
///
/// This is dramatically faster than the previous 1-level radix + comparison sort
/// approach, especially for numeric data where many entries share the same top
/// 16 bits (e.g., sorted numeric data where all values are in a narrow range).
///
/// For stable sort: LSD radix sort is inherently stable (preserves input order
/// for equal keys), so no additional work is needed.
///
/// For non-stable sort with last-resort comparison: after the radix sort by u64,
/// we do a final pass to sort entries with equal u64 keys by their raw line content.
fn radix_sort_numeric_entries(
    entries: Vec<(u64, usize)>,
    data: &[u8],
    offsets: &[(usize, usize)],
    stable: bool,
    _reverse: bool,
) -> Vec<(u64, usize)> {
    let n = entries.len();
    if n <= 1 {
        return entries;
    }

    // LSD radix sort on 16-bit groups. Up to 4 passes: bits [0:16), [16:32), [32:48), [48:64)
    // Skip passes where all entries share the same 16-bit group (common for numeric data
    // in a limited range, e.g., integers 0-999999 all share the same top 32 bits).
    let nbk: usize = 65536;

    // Pre-scan: determine which 16-bit groups actually have variation.
    // XOR all values to find bits that differ, then check each 16-bit group.
    let first_val = entries[0].0;
    let mut xor_all = 0u64;
    for &(val, _) in &entries[1..] {
        xor_all |= val ^ first_val;
    }

    // Build list of passes that need sorting (where bits differ)
    let mut passes_needed: Vec<u32> = Vec::with_capacity(4);
    for pass in 0..4u32 {
        let shift = pass * 16;
        if ((xor_all >> shift) & 0xFFFF) != 0 {
            passes_needed.push(pass);
        }
    }

    // If no passes needed, all values are identical — already sorted
    if passes_needed.is_empty() {
        let mut sorted = entries;
        // Still need last-resort comparison for non-stable sort
        if !stable {
            let dp = data.as_ptr();
            let mut i = 0;
            while i < n {
                let key = sorted[i].0;
                let mut j = i + 1;
                while j < n && sorted[j].0 == key {
                    j += 1;
                }
                if j - i > 1 {
                    sorted[i..j].sort_unstable_by(|a, b| unsafe {
                        let (sa, ea) = offsets[a.1];
                        let (sb, eb) = offsets[b.1];
                        std::slice::from_raw_parts(dp.add(sa), ea - sa)
                            .cmp(std::slice::from_raw_parts(dp.add(sb), eb - sb))
                    });
                }
                i = j;
            }
        }
        return sorted;
    }

    let mut src = entries;
    let mut dst: Vec<(u64, usize)> = Vec::with_capacity(n);
    #[allow(clippy::uninit_vec)]
    unsafe {
        dst.set_len(n);
    }

    let mut cnts = vec![0u32; nbk];

    for &pass in &passes_needed {
        let shift = pass * 16;

        // Count occurrences
        cnts.iter_mut().for_each(|c| *c = 0);
        for &(val, _) in &src {
            cnts[((val >> shift) & 0xFFFF) as usize] += 1;
        }

        // Prefix sum -> write positions
        let mut sum = 0u32;
        for c in cnts.iter_mut() {
            let old = *c;
            *c = sum;
            sum += old;
        }

        // Scatter into dst
        let dst_ptr = dst.as_mut_ptr();
        for &ent in &src {
            let b = ((ent.0 >> shift) & 0xFFFF) as usize;
            unsafe {
                *dst_ptr.add(cnts[b] as usize) = ent;
            }
            cnts[b] += 1;
        }

        // Swap src and dst for next pass
        std::mem::swap(&mut src, &mut dst);
    }

    // After N passes, result is in `src` (swapped N times).
    // If N is odd, the result ended up in the original `dst` (now in `src` after swap).
    let mut sorted = src;

    // For non-stable sort: sort entries with equal u64 keys by raw line content
    // (last-resort comparison for deterministic output).
    if !stable {
        let dp = data.as_ptr();
        // Only sort runs of equal keys — most runs will be length 1.
        let mut i = 0;
        while i < n {
            let key = sorted[i].0;
            let mut j = i + 1;
            while j < n && sorted[j].0 == key {
                j += 1;
            }
            if j - i > 1 {
                // Sort this run by raw line content
                sorted[i..j].sort_unstable_by(|a, b| unsafe {
                    let (sa, ea) = offsets[a.1];
                    let (sb, eb) = offsets[b.1];
                    std::slice::from_raw_parts(dp.add(sa), ea - sa)
                        .cmp(std::slice::from_raw_parts(dp.add(sb), eb - sb))
                });
            }
            i = j;
        }
    }

    sorted
}

/// Full 4-pass LSD radix sort for lexicographic (u64, u32, u32) entries.
/// Sorts by the u64 big-endian prefix in O(n) time with ZERO comparisons.
/// After radix sort, entries with identical 8-byte prefixes are resolved by
/// comparing the remaining line content (bytes after position 8).
///
/// This matches the numeric sort path (which achieves 12.6x speedup) and is
/// much faster than 2-level MSD radix + pdqsort for large inputs because:
/// - All 64 bits resolved through O(n) radix passes (no comparison sort)
/// - Only the tiny fraction with identical 8-byte prefixes need comparison
/// - Skip optimization avoids passes where all entries share the same 16-bit group
fn radix_sort_lex_entries(
    entries: Vec<(u64, u32, u32)>,
    data: &[u8],
    stable: bool,
) -> Vec<(u64, u32, u32)> {
    let n = entries.len();
    if n <= 1 {
        return entries;
    }

    let nbk: usize = 65536;

    // Pre-scan: XOR all values to find which 16-bit groups have variation.
    let first_val = entries[0].0;
    let mut xor_all = 0u64;
    for &(val, _, _) in &entries[1..] {
        xor_all |= val ^ first_val;
    }

    // Build list of passes that need sorting (where bits differ)
    let mut passes_needed: Vec<u32> = Vec::with_capacity(4);
    for pass in 0..4u32 {
        let shift = pass * 16;
        if ((xor_all >> shift) & 0xFFFF) != 0 {
            passes_needed.push(pass);
        }
    }

    // If no passes needed, all prefixes identical — just resolve equal groups
    if passes_needed.is_empty() {
        let mut sorted = entries;
        sort_equal_lex_groups(&mut sorted, data, stable);
        return sorted;
    }

    let mut src = entries;
    let mut dst: Vec<(u64, u32, u32)> = Vec::with_capacity(n);
    #[allow(clippy::uninit_vec)]
    unsafe {
        dst.set_len(n);
    }

    let mut cnts = vec![0u32; nbk];

    for &pass in &passes_needed {
        let shift = pass * 16;

        // Count occurrences
        cnts.iter_mut().for_each(|c| *c = 0);
        for &(val, _, _) in &src {
            cnts[((val >> shift) & 0xFFFF) as usize] += 1;
        }

        // Prefix sum -> write positions
        let mut sum = 0u32;
        for c in cnts.iter_mut() {
            let old = *c;
            *c = sum;
            sum += old;
        }

        // Scatter into dst with software prefetch
        let dst_ptr = dst.as_mut_ptr();
        let src_ptr = src.as_ptr();
        let pfx_dist = 8usize;
        for idx in 0..n {
            let ent = unsafe { *src_ptr.add(idx) };
            let b = ((ent.0 >> shift) & 0xFFFF) as usize;
            unsafe {
                *dst_ptr.add(cnts[b] as usize) = ent;
            }
            cnts[b] += 1;
            // Prefetch future write destination
            if idx + pfx_dist < n {
                let future = unsafe { *src_ptr.add(idx + pfx_dist) };
                let fb = ((future.0 >> shift) & 0xFFFF) as usize;
                prefetch_read(unsafe { dst_ptr.add(cnts[fb] as usize) as *const u8 });
            }
        }

        std::mem::swap(&mut src, &mut dst);
    }

    let mut sorted = src;

    // Resolve entries with identical 8-byte prefixes by comparing remaining bytes
    sort_equal_lex_groups(&mut sorted, data, stable);

    sorted
}

/// Sort runs of entries with equal u64 prefixes by their full line content.
/// Skips the first 8 bytes (already resolved via prefix) and uses u64-wide loads
/// for fast comparison of remaining bytes.
fn sort_equal_lex_groups(sorted: &mut [(u64, u32, u32)], data: &[u8], stable: bool) {
    let n = sorted.len();
    let dp = data.as_ptr() as usize;
    let mut i = 0;
    while i < n {
        let key = sorted[i].0;
        let mut j = i + 1;
        while j < n && sorted[j].0 == key {
            j += 1;
        }
        if j - i > 1 {
            let cmp = |a: &(u64, u32, u32), b: &(u64, u32, u32)| -> Ordering {
                let la = a.2 as usize;
                let lb = b.2 as usize;
                let skip_a = 8.min(la);
                let skip_b = 8.min(lb);
                let rem_a = la - skip_a;
                let rem_b = lb - skip_b;
                unsafe {
                    let p = dp as *const u8;
                    let pa = p.add(a.1 as usize + skip_a);
                    let pb = p.add(b.1 as usize + skip_b);
                    let min_rem = rem_a.min(rem_b);
                    let mut k = 0usize;
                    while k + 8 <= min_rem {
                        let wa = u64::from_be_bytes(*(pa.add(k) as *const [u8; 8]));
                        let wb = u64::from_be_bytes(*(pb.add(k) as *const [u8; 8]));
                        if wa != wb {
                            return wa.cmp(&wb);
                        }
                        k += 8;
                    }
                    std::slice::from_raw_parts(pa.add(k), rem_a - k)
                        .cmp(std::slice::from_raw_parts(pb.add(k), rem_b - k))
                }
            };
            if stable {
                sorted[i..j].sort_by(cmp);
            } else {
                sorted[i..j].sort_unstable_by(cmp);
            }
        }
        i = j;
    }
}

/// Threshold for switching to parallel sort. Below this, rayon thread pool
/// overhead exceeds the sorting benefit. Set to 10K to enable parallel sort
/// earlier, which helps for piped 10MB input (~50K-200K lines).
const PARALLEL_SORT_THRESHOLD: usize = 10_000;

/// Helper: perform a parallel or sequential sort on indices.
fn do_sort(
    indices: &mut [usize],
    stable: bool,
    cmp: impl Fn(&usize, &usize) -> Ordering + Send + Sync,
) {
    let n = indices.len();
    if stable {
        if n > PARALLEL_SORT_THRESHOLD {
            indices.par_sort_by(cmp);
        } else {
            indices.sort_by(cmp);
        }
    } else if n > PARALLEL_SORT_THRESHOLD {
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
    // Enlarge pipe buffers on Linux for higher throughput when reading from stdin.
    // 8MB matches other tools (ftac, fbase64, ftr, fcut) for consistent syscall reduction.
    #[cfg(target_os = "linux")]
    {
        const PIPE_SIZE: i32 = 8 * 1024 * 1024;
        unsafe {
            libc::fcntl(0, libc::F_SETPIPE_SZ, PIPE_SIZE);
            libc::fcntl(1, libc::F_SETPIPE_SZ, PIPE_SIZE);
        }
    }

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
        let mut writer = if let Some(ref path) = config.output_file {
            SortOutput::File(BufWriter::with_capacity(
                OUTPUT_BUF_SIZE,
                File::create(path)?,
            ))
        } else {
            SortOutput::stdout()
        };
        return merge_sorted(inputs, config, &mut writer);
    }

    // Read all input BEFORE opening output file (supports -o same-file)
    let (buffer, offsets) = read_all_input(inputs, config.zero_terminated)?;
    let data: &[u8] = &buffer;
    let num_lines = offsets.len();

    let mut writer = if let Some(ref path) = config.output_file {
        SortOutput::File(BufWriter::with_capacity(
            OUTPUT_BUF_SIZE,
            File::create(path)?,
        ))
    } else {
        SortOutput::stdout()
    };

    if num_lines == 0 {
        return Ok(());
    }

    let terminator: &[u8] = if config.zero_terminated { b"\0" } else { b"\n" };

    // === Already-sorted detection: O(n) scan before sorting ===
    // If data is already in order, skip the O(n log n) sort entirely.
    // This turns the "already sorted" benchmark from 2.1x to near-instant.
    //
    // For plain lexicographic sort (most common case), use direct byte slice
    // comparison instead of the full compare_lines machinery.
    // Also handles sort -r (reverse) by checking descending order.
    let no_keys = config.keys.is_empty();
    let gopts = &config.global_opts;
    let is_plain_lex_for_check = no_keys
        && !gopts.has_sort_type()
        && !gopts.dictionary_order
        && !gopts.ignore_case
        && !gopts.ignore_nonprinting
        && !gopts.ignore_leading_blanks;

    // Pre-detect numeric mode to skip the generic sorted check.
    // Numeric mode does its own sorted check after parsing u64 values.
    let is_numeric_only_precheck = no_keys
        && (gopts.numeric || gopts.general_numeric || gopts.human_numeric)
        && !gopts.dictionary_order
        && !gopts.ignore_case
        && !gopts.ignore_nonprinting;

    if num_lines > 1 && !is_numeric_only_precheck {
        // Check both ascending and descending order simultaneously.
        // If ascending: data is already sorted -> output as-is.
        // If descending: data is reverse-sorted -> reverse and output (O(n) vs O(n log n)).
        // This is especially valuable for `sort reverse_sorted.txt` (currently 2.7x).
        let (is_sorted, is_reverse_sorted) = if is_plain_lex_for_check {
            let dp = data.as_ptr();
            let reverse = gopts.reverse;
            let mut asc = true;
            let mut desc = true;
            let mut prev_prefix = line_prefix(data, offsets[0].0, offsets[0].1);
            for i in 1..num_lines {
                let (s2, e2) = offsets[i];
                let cur_prefix = line_prefix(data, s2, e2);
                if asc {
                    if prev_prefix > cur_prefix {
                        asc = false;
                    } else if prev_prefix == cur_prefix {
                        let (s1, e1) = offsets[i - 1];
                        let a = unsafe { std::slice::from_raw_parts(dp.add(s1), e1 - s1) };
                        let b = unsafe { std::slice::from_raw_parts(dp.add(s2), e2 - s2) };
                        if a > b {
                            asc = false;
                        }
                    }
                }
                if desc {
                    if prev_prefix < cur_prefix {
                        desc = false;
                    } else if prev_prefix == cur_prefix {
                        let (s1, e1) = offsets[i - 1];
                        let a = unsafe { std::slice::from_raw_parts(dp.add(s1), e1 - s1) };
                        let b = unsafe { std::slice::from_raw_parts(dp.add(s2), e2 - s2) };
                        if a < b {
                            desc = false;
                        }
                    }
                }
                if !asc && !desc {
                    break;
                }
                prev_prefix = cur_prefix;
            }
            // For -r (reverse), swap: ascending input needs reversal, descending is "sorted"
            if reverse { (desc, asc) } else { (asc, desc) }
        } else {
            let mut asc = true;
            let mut desc = true;
            for i in 1..num_lines {
                let (s1, e1) = offsets[i - 1];
                let (s2, e2) = offsets[i];
                let cmp = compare_lines(&data[s1..e1], &data[s2..e2], config);
                match cmp {
                    Ordering::Greater => asc = false,
                    Ordering::Less => desc = false,
                    _ => {}
                }
                if !asc && !desc {
                    break;
                }
            }
            (asc, desc)
        };

        if is_sorted {
            // Zero-copy fast path: write mmap data directly when possible.
            // Conditions: non-unique, newline-terminated, no \r in data.
            if !config.unique && !config.zero_terminated && memchr::memchr(b'\r', data).is_none() {
                if data.last() == Some(&b'\n') {
                    writer.write_all_direct(data)?;
                } else if !data.is_empty() {
                    writer.write_all(data)?;
                    writer.write_all(b"\n")?;
                }
                writer.flush()?;
                return Ok(());
            }

            // Contiguous buffer output for unique/\r\n/zero-terminated cases
            let dp = data.as_ptr();
            let term_byte = terminator[0];
            if config.unique {
                let buf_cap = data.len() + num_lines + 1;
                let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
                let bptr = buf.as_mut_ptr();
                let mut pos = 0usize;
                let mut prev: Option<usize> = None;
                for i in 0..num_lines {
                    let (s, e) = offsets[i];
                    let len = e - s;
                    let line = unsafe { std::slice::from_raw_parts(dp.add(s), len) };
                    let emit = match prev {
                        Some(p) => {
                            let (ps, pe) = offsets[p];
                            let prev_line =
                                unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                            compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                        }
                        None => true,
                    };
                    if emit {
                        unsafe {
                            std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                            *bptr.add(pos + len) = term_byte;
                        }
                        pos += len + 1;
                        prev = Some(i);
                    }
                }
                unsafe {
                    buf.set_len(pos);
                }
                writer.write_all_direct(&buf)?;
            } else {
                let total_size = if data.last() == Some(&term_byte) {
                    data.len()
                } else {
                    data.len() + 1
                };
                let mut buf: Vec<u8> = Vec::with_capacity(total_size);
                let bptr = buf.as_mut_ptr();
                let mut pos = 0usize;
                for i in 0..num_lines {
                    let (s, e) = offsets[i];
                    let len = e - s;
                    unsafe {
                        std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                        *bptr.add(pos + len) = term_byte;
                    }
                    pos += len + 1;
                }
                unsafe {
                    buf.set_len(pos);
                }
                writer.write_all_direct(&buf)?;
            }
            writer.flush()?;
            return Ok(());
        }

        // Reverse-sorted detection: if data is in descending order and user wants
        // ascending (or vice versa with -r), just reverse the offsets array.
        // This is O(n) instead of O(n log n), turning the "reverse sorted" case
        // from 2.7x to near-instant (like the already-sorted case).
        if is_reverse_sorted {
            let dp = data.as_ptr();
            let term_byte = terminator[0];
            let buf_cap = data.len() + num_lines + 1;
            let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
            let bptr = buf.as_mut_ptr();
            let mut pos = 0usize;
            let mut prev: Option<usize> = None;
            for i in (0..num_lines).rev() {
                let (s, e) = offsets[i];
                let len = e - s;
                if config.unique {
                    let line = unsafe { std::slice::from_raw_parts(dp.add(s), len) };
                    let emit = match prev {
                        Some(p) => {
                            let (ps, pe) = offsets[p];
                            let prev_line =
                                unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                            compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                        }
                        None => true,
                    };
                    if !emit {
                        continue;
                    }
                    prev = Some(i);
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                    *bptr.add(pos + len) = term_byte;
                }
                pos += len + 1;
            }
            unsafe {
                buf.set_len(pos);
            }
            writer.write_all_direct(&buf)?;
            writer.flush()?;
            return Ok(());
        }
    }

    // Switch to random access for sort phase (comparisons jump to arbitrary lines).
    // Only advise for truly large files (>10MB) where the prefetch pattern matters.
    #[cfg(target_os = "linux")]
    if data.len() > 10 * 1024 * 1024 {
        if let FileData::Mmap(ref mmap) = buffer {
            let _ = mmap.advise(memmap2::Advice::Random);
        }
    }

    // Detect sort mode and use specialized fast path
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
        // FAST PATH 1: Radix bucket sort + prefix comparison.
        // Distributes entries into 256 buckets by first byte, then sorts each
        // bucket independently. This eliminates the first byte from comparisons
        // and reduces per-bucket sort size by ~62x (for uniform ASCII), giving
        // much better cache behavior and parallel scaling.
        let reverse = gopts.reverse;

        // Build entries with (prefix, start, len)
        let entries: Vec<(u64, u32, u32)> = if num_lines > 10_000 {
            offsets
                .par_iter()
                .map(|&(s, e)| (line_prefix(data, s, e), s as u32, (e - s) as u32))
                .collect()
        } else {
            offsets
                .iter()
                .map(|&(s, e)| (line_prefix(data, s, e), s as u32, (e - s) as u32))
                .collect()
        };

        // Already-sorted detection: O(n) scan using 8-byte prefixes.
        // If data is already sorted, skip the entire radix sort.
        // Also detects reverse-sorted input (for sort -r optimization).
        // Works with -u (unique) by applying linear dedup on sorted data.
        if num_lines > 1 && !config.zero_terminated && memchr::memchr(b'\r', data).is_none() {
            let dp = data.as_ptr();
            let mut is_sorted_fwd = true;
            let mut is_sorted_rev = true;
            let stable = config.stable;
            for i in 1..entries.len() {
                let (pa, sa, la) = entries[i - 1];
                let (pb, sb, lb) = entries[i];
                let ord = match pa.cmp(&pb) {
                    Ordering::Equal => unsafe {
                        let skip_a = 8.min(la as usize);
                        let skip_b = 8.min(lb as usize);
                        std::slice::from_raw_parts(
                            dp.add(sa as usize + skip_a),
                            la as usize - skip_a,
                        )
                        .cmp(std::slice::from_raw_parts(
                            dp.add(sb as usize + skip_b),
                            lb as usize - skip_b,
                        ))
                    },
                    ord => ord,
                };
                match ord {
                    Ordering::Greater => is_sorted_fwd = false,
                    Ordering::Less => is_sorted_rev = false,
                    Ordering::Equal => {
                        if !stable {
                            // For unstable sort, equal-key lines are ordered by full line
                            let line_ord = unsafe {
                                std::slice::from_raw_parts(dp.add(sa as usize), la as usize).cmp(
                                    std::slice::from_raw_parts(dp.add(sb as usize), lb as usize),
                                )
                            };
                            if line_ord == Ordering::Greater {
                                is_sorted_fwd = false;
                            }
                            if line_ord == Ordering::Less {
                                is_sorted_rev = false;
                            }
                        }
                    }
                }
                if !is_sorted_fwd && !is_sorted_rev {
                    break;
                }
            }

            if (!reverse && is_sorted_fwd) || (reverse && is_sorted_rev) {
                // Already in desired order: output directly (O(n) vs O(n log n))
                if config.unique {
                    // Linear dedup on already-sorted data — contiguous buffer
                    let term_byte = terminator[0];
                    let buf_cap = data.len() + entries.len() + 1;
                    let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
                    let bptr = buf.as_mut_ptr();
                    let mut pos = 0usize;
                    let mut prev_pfx = u64::MAX;
                    let mut prev_s = u32::MAX;
                    let mut prev_l = 0u32;
                    for &(pfx, s, l) in &entries {
                        let emit = prev_s == u32::MAX
                            || pfx != prev_pfx
                            || l != prev_l
                            || unsafe {
                                std::slice::from_raw_parts(dp.add(s as usize), l as usize)
                                    != std::slice::from_raw_parts(
                                        dp.add(prev_s as usize),
                                        prev_l as usize,
                                    )
                            };
                        if emit {
                            let lu = l as usize;
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    dp.add(s as usize),
                                    bptr.add(pos),
                                    lu,
                                );
                                *bptr.add(pos + lu) = term_byte;
                            }
                            pos += lu + 1;
                            prev_pfx = pfx;
                            prev_s = s;
                            prev_l = l;
                        }
                    }
                    unsafe {
                        buf.set_len(pos);
                    }
                    writer.write_all_direct(&buf)?;
                } else if data.last() == Some(&b'\n') {
                    writer.write_all_direct(data)?;
                } else if !data.is_empty() {
                    writer.write_all(data)?;
                    writer.write_all(b"\n")?;
                }
                writer.flush()?;
                return Ok(());
            }

            if (!reverse && is_sorted_rev) || (reverse && is_sorted_fwd) {
                // Reverse of desired order: output lines in reverse — contiguous buffer
                let term_byte = terminator[0];
                let unique = config.unique;
                let buf_cap = data.len() + entries.len() + 1;
                let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
                let bptr = buf.as_mut_ptr();
                let mut pos = 0usize;
                let mut prev_pfx = u64::MAX;
                let mut prev_s = u32::MAX;
                let mut prev_l = 0u32;
                for i in (0..entries.len()).rev() {
                    let (pfx, s, l) = entries[i];
                    if unique {
                        let emit = prev_s == u32::MAX
                            || pfx != prev_pfx
                            || l != prev_l
                            || unsafe {
                                std::slice::from_raw_parts(dp.add(s as usize), l as usize)
                                    != std::slice::from_raw_parts(
                                        dp.add(prev_s as usize),
                                        prev_l as usize,
                                    )
                            };
                        if !emit {
                            continue;
                        }
                        prev_pfx = pfx;
                        prev_s = s;
                        prev_l = l;
                    }
                    let lu = l as usize;
                    unsafe {
                        std::ptr::copy_nonoverlapping(dp.add(s as usize), bptr.add(pos), lu);
                        *bptr.add(pos + lu) = term_byte;
                    }
                    pos += lu + 1;
                }
                unsafe {
                    buf.set_len(pos);
                }
                writer.write_all_direct(&buf)?;
                writer.flush()?;
                return Ok(());
            }
        }

        // Parallel pdqsort with 8-byte prefix comparison.
        // Faster than MSD radix for typical inputs because:
        // - In-place: no 2MB scatter buffers, much better cache behavior
        // - u64 prefix resolves 99%+ of comparisons in 1 instruction
        // - u64-wide loads for remaining bytes eliminate byte-at-a-time comparison
        // - rayon par_sort splits across cores, each chunk fits in L2
        let data_addr = data.as_ptr() as usize;
        let pfx_cmp = |a: &(u64, u32, u32), b: &(u64, u32, u32)| -> Ordering {
            match a.0.cmp(&b.0) {
                Ordering::Equal => {
                    let la = a.2 as usize;
                    let lb = b.2 as usize;
                    let skip_a = 8.min(la);
                    let skip_b = 8.min(lb);
                    let rem_a = la - skip_a;
                    let rem_b = lb - skip_b;
                    unsafe {
                        let dp = data_addr as *const u8;
                        let pa = dp.add(a.1 as usize + skip_a);
                        let pb = dp.add(b.1 as usize + skip_b);
                        let min_rem = rem_a.min(rem_b);
                        let mut i = 0usize;
                        while i + 8 <= min_rem {
                            let wa = u64::from_be_bytes(*(pa.add(i) as *const [u8; 8]));
                            let wb = u64::from_be_bytes(*(pb.add(i) as *const [u8; 8]));
                            if wa != wb {
                                return wa.cmp(&wb);
                            }
                            i += 8;
                        }
                        std::slice::from_raw_parts(pa.add(i), rem_a - i)
                            .cmp(std::slice::from_raw_parts(pb.add(i), rem_b - i))
                    }
                }
                ord => ord,
            }
        };
        let mut sorted = entries;
        if num_lines > 512 {
            sorted = radix_sort_lex_entries(sorted, data, config.stable);
        } else if config.stable {
            sorted.sort_by(pfx_cmp);
        } else {
            sorted.sort_unstable_by(pfx_cmp);
        }

        // Switch to random-access hint for output phase:
        // After sorting, entries access data in sorted (not file) order,
        // which is random with respect to mmap page layout.
        // MADV_RANDOM disables readahead that would waste I/O on unneeded pages.
        #[cfg(target_os = "linux")]
        if let FileData::Mmap(ref mmap) = buffer {
            let _ = mmap.advise(memmap2::Advice::Random);
        }

        // Output sorted entries using contiguous buffer construction.
        // Instead of IoSlice batching (which has per-entry overhead and many
        // write_vectored calls), copy sorted lines into a contiguous buffer
        // and write_all once. Eliminates 2*N IoSlice pushes + N/512 writev calls.
        let dp = data.as_ptr();
        let n_sorted = sorted.len();
        let term_byte = terminator[0];
        if config.unique {
            // Dedup output: use prefix u64 as fast rejection.
            let mut prev_prefix = u64::MAX;
            let mut prev_start = u32::MAX;
            let mut prev_len = 0u32;
            // Upper bound on output size; actual may be smaller due to dedup.
            let buf_cap = data.len() + n_sorted + 1;
            let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
            let bptr = buf.as_mut_ptr();
            let mut pos = 0usize;
            for idx in 0..n_sorted {
                let actual_idx = if reverse { n_sorted - 1 - idx } else { idx };
                let (pfx, s, l) = sorted[actual_idx];
                let su = s as usize;
                let lu = l as usize;
                let emit = prev_start == u32::MAX || pfx != prev_prefix || {
                    let ps = prev_start as usize;
                    let pl = prev_len as usize;
                    if pl != lu {
                        true
                    } else {
                        unsafe {
                            std::slice::from_raw_parts(dp.add(ps), pl)
                                != std::slice::from_raw_parts(dp.add(su), lu)
                        }
                    }
                };
                if emit {
                    unsafe {
                        std::ptr::copy_nonoverlapping(dp.add(su), bptr.add(pos), lu);
                        *bptr.add(pos + lu) = term_byte;
                    }
                    pos += lu + 1;
                    prev_prefix = pfx;
                    prev_start = s;
                    prev_len = l;
                }
            }
            unsafe {
                buf.set_len(pos);
            }
            writer.write_all_direct(&buf)?;
        } else {
            // Non-unique output: build contiguous buffer with prefetch.
            // Total output = data.len() (if trailing newline) or data.len()+1.
            let total_size = if data.last() == Some(&term_byte) {
                data.len()
            } else {
                data.len() + 1
            };
            let mut buf: Vec<u8> = Vec::with_capacity(total_size);
            let bptr = buf.as_mut_ptr();
            let mut pos = 0usize;
            let iter_reverse = reverse;
            for j in 0..n_sorted {
                if j + 16 < n_sorted {
                    let ahead = if iter_reverse {
                        n_sorted - 1 - (j + 16)
                    } else {
                        j + 16
                    };
                    prefetch_read(unsafe { dp.add(sorted[ahead].1 as usize) });
                }
                let actual = if iter_reverse { n_sorted - 1 - j } else { j };
                let (_, s, l) = sorted[actual];
                let len = l as usize;
                unsafe {
                    std::ptr::copy_nonoverlapping(dp.add(s as usize), bptr.add(pos), len);
                    *bptr.add(pos + len) = term_byte;
                }
                pos += len + 1;
            }
            unsafe {
                buf.set_len(pos);
            }
            writer.write_all_direct(&buf)?;
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
        let dp_fold = data.as_ptr();
        // Case-insensitive comparison: u64 prefix resolves most comparisons,
        // falls back to full case-insensitive compare, then raw line compare.
        let fold_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
            let (sa, ea) = offsets[a.1];
            let (sb, eb) = offsets[b.1];
            let la = unsafe { std::slice::from_raw_parts(dp_fold.add(sa), ea - sa) };
            let lb = unsafe { std::slice::from_raw_parts(dp_fold.add(sb), eb - sb) };
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
            if ord == Ordering::Equal && !stable {
                unsafe {
                    std::slice::from_raw_parts(dp_fold.add(sa), ea - sa)
                        .cmp(std::slice::from_raw_parts(dp_fold.add(sb), eb - sb))
                }
            } else {
                ord
            }
        };

        // Use radix sort on uppercase prefix for large inputs.
        // This distributes entries by their 8-byte uppercase prefix, then
        // sorts within each bucket using the full case-insensitive comparator.
        if n > 256 {
            let mut entries = radix_sort_numeric_entries(entries, data, &offsets, stable, false);
            // The numeric radix sort treats u64 as opaque keys, which works for
            // our uppercase prefix since it preserves lexicographic order.
            // But we need to tiebreak within equal-prefix runs with the full
            // case-insensitive comparator.
            {
                let mut i = 0;
                while i < n {
                    let key = entries[i].0;
                    let mut j = i + 1;
                    while j < n && entries[j].0 == key {
                        j += 1;
                    }
                    if j - i > 1 {
                        entries[i..j].sort_unstable_by(fold_cmp);
                    }
                    i = j;
                }
            }
            if reverse {
                entries.reverse();
            }
            write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
        } else {
            let fold_cmp_rev = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                let ord = fold_cmp(a, b);
                if reverse { ord.reverse() } else { ord }
            };
            if stable {
                entries.sort_by(fold_cmp_rev);
            } else {
                entries.sort_unstable_by(fold_cmp_rev);
            }
            write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
        }
    } else if is_numeric_only {
        // FAST PATH 2: Pre-parsed numeric sort with u64 comparison.
        // For pure -n sort (not -g or -h), try integer-only fast path first:
        // parse directly to i64 -> sortable u64, avoiding f64 conversion entirely.
        let reverse = gopts.reverse;
        let stable = config.stable;

        let mut entries: Vec<(u64, usize)> = if gopts.numeric {
            // Single-pass numeric parse: try integer first, fall back to f64 inline.
            // This avoids the two-pass penalty when some lines have decimals —
            // instead of parsing ALL lines twice, each line is parsed exactly once.
            let parse_numeric_inline = |i: usize, &(s, e): &(usize, usize)| -> (u64, usize) {
                let line = &data[s..e];
                match try_parse_integer(line) {
                    Some(v) => (int_to_sortable_u64(v), i),
                    None => (float_to_sortable_u64(parse_numeric_value(line)), i),
                }
            };
            if num_lines > 10_000 {
                offsets
                    .par_iter()
                    .enumerate()
                    .map(|(i, off)| parse_numeric_inline(i, off))
                    .collect()
            } else {
                offsets
                    .iter()
                    .enumerate()
                    .map(|(i, off)| parse_numeric_inline(i, off))
                    .collect()
            }
        } else {
            // General numeric (-g) or human numeric (-h): always use f64
            if num_lines > 10_000 {
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
            }
        };

        // Fast already-sorted check using pre-parsed u64 values.
        // O(n) u64 comparisons instead of O(n) string-parsing comparisons.
        // For reverse mode, check descending order.
        if entries.len() > 1 {
            let is_sorted = if reverse {
                entries.windows(2).all(|w| w[0].0 >= w[1].0)
            } else {
                entries.windows(2).all(|w| w[0].0 <= w[1].0)
            };
            if is_sorted {
                // Data is already sorted by numeric value
                if !config.unique
                    && !config.zero_terminated
                    && memchr::memchr(b'\r', data).is_none()
                {
                    if data.last() == Some(&b'\n') {
                        writer.write_all(data)?;
                    } else if !data.is_empty() {
                        writer.write_all(data)?;
                        writer.write_all(b"\n")?;
                    }
                    writer.flush()?;
                    return Ok(());
                }
                // Handle unique/zero-terminated
                write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
                writer.flush()?;
                return Ok(());
            }
        }

        // Use radix sort for large numeric inputs (>256 entries).
        // Radix distribution on top 16 bits resolves ~99.9% of ordering without
        // comparisons, giving ~2x speedup over comparison-based sort.
        let n = entries.len();
        if n > 256 {
            // Always sort ascending in the radix sort; apply reverse at output time.
            let mut entries = radix_sort_numeric_entries(entries, data, &offsets, stable, false);
            if reverse {
                entries.reverse();
            }
            write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
        } else {
            let dp_ns = data.as_ptr();
            let cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                let ord = a.0.cmp(&b.0);
                if ord != Ordering::Equal {
                    return if reverse { ord.reverse() } else { ord };
                }
                if !stable {
                    let (sa, ea) = offsets[a.1];
                    let (sb, eb) = offsets[b.1];
                    unsafe {
                        std::slice::from_raw_parts(dp_ns.add(sa), ea - sa)
                            .cmp(std::slice::from_raw_parts(dp_ns.add(sb), eb - sb))
                    }
                } else {
                    Ordering::Equal
                }
            };
            if stable {
                entries.sort_by(cmp);
            } else {
                entries.sort_unstable_by(cmp);
            }
            write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
        }
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
            // For pure -n (not -g or -h), try integer fast path first.
            let is_gen = opts.general_numeric;
            let is_pure_numeric = opts.numeric && !opts.general_numeric && !opts.human_numeric;
            let reverse = opts.reverse;
            let stable = config.stable;

            let mut entries: Vec<(u64, usize)> = if is_pure_numeric {
                // Single-pass numeric parse: try integer first, fall back to f64 inline.
                let parse_entry = |i: usize, &(s, e): &(usize, usize)| -> (u64, usize) {
                    if s == e {
                        return (int_to_sortable_u64(0), i);
                    }
                    let line = &data[s..e];
                    match try_parse_integer(line) {
                        Some(v) => (int_to_sortable_u64(v), i),
                        None => (float_to_sortable_u64(parse_numeric_value(line)), i),
                    }
                };
                if num_lines > 10_000 {
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
                }
            } else {
                let parse_entry = |i: usize, &(s, e): &(usize, usize)| {
                    let f = if s == e {
                        if is_gen { f64::NAN } else { 0.0 }
                    } else {
                        parse_value_for_opts(&data[s..e], opts)
                    };
                    (float_to_sortable_u64(f), i)
                };
                if num_lines > 10_000 {
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
                }
            };

            // Use radix sort for single-key numeric values (>256 entries).
            let n = entries.len();
            if n > 256 {
                let mut entries =
                    radix_sort_numeric_entries(entries, data, &offsets, stable, false);
                if reverse {
                    entries.reverse();
                }
                write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
            } else {
                let dp_skn = data.as_ptr();
                let cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                    let ord = a.0.cmp(&b.0);
                    if ord != Ordering::Equal {
                        return if reverse { ord.reverse() } else { ord };
                    }
                    if !stable {
                        let (sa, ea) = offsets[a.1];
                        let (sb, eb) = offsets[b.1];
                        unsafe {
                            std::slice::from_raw_parts(dp_skn.add(sa), ea - sa)
                                .cmp(std::slice::from_raw_parts(dp_skn.add(sb), eb - sb))
                        }
                    } else {
                        Ordering::Equal
                    }
                };
                if stable {
                    entries.sort_by(cmp);
                } else {
                    entries.sort_unstable_by(cmp);
                }
                write_sorted_entries(data, &offsets, &entries, config, &mut writer, terminator)?;
            }
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
                // Already-sorted check for single-key lexicographic path.
                // Detects both forward-sorted and reverse-sorted input in O(n).
                // Handles all combinations of input order vs -r flag.
                if num_lines > 1 {
                    let mut is_sorted_fwd = true;
                    let mut is_sorted_rev = true;
                    let mut prev_pfx = if key_offs[0].0 < key_offs[0].1 {
                        line_prefix(data, key_offs[0].0, key_offs[0].1)
                    } else {
                        0u64
                    };
                    for i in 1..num_lines {
                        if !is_sorted_fwd && !is_sorted_rev {
                            break;
                        }
                        let (ks, ke) = key_offs[i];
                        let cur_pfx = if ks < ke {
                            line_prefix(data, ks, ke)
                        } else {
                            0u64
                        };
                        if is_sorted_fwd {
                            if cur_pfx < prev_pfx {
                                is_sorted_fwd = false;
                            } else if cur_pfx == prev_pfx {
                                let (ps, pe) = key_offs[i - 1];
                                let pk = &data[ps..pe];
                                let ck = &data[ks..ke];
                                if pk > ck {
                                    is_sorted_fwd = false;
                                } else if !config.stable && pk == ck {
                                    let (ls, le) = offsets[i - 1];
                                    let (cs, ce) = offsets[i];
                                    if data[ls..le] > data[cs..ce] {
                                        is_sorted_fwd = false;
                                    }
                                }
                            }
                        }
                        if is_sorted_rev {
                            if cur_pfx > prev_pfx {
                                is_sorted_rev = false;
                            } else if cur_pfx == prev_pfx {
                                let (ps, pe) = key_offs[i - 1];
                                let pk = &data[ps..pe];
                                let ck = &data[ks..ke];
                                if pk < ck {
                                    is_sorted_rev = false;
                                } else if !config.stable && pk == ck {
                                    let (ls, le) = offsets[i - 1];
                                    let (cs, ce) = offsets[i];
                                    if data[ls..le] < data[cs..ce] {
                                        is_sorted_rev = false;
                                    }
                                }
                            }
                        }
                        prev_pfx = cur_pfx;
                    }

                    // Determine if input order matches desired output order:
                    // - forward sorted + no -r: output directly
                    // - reverse sorted + -r: output directly
                    // - forward sorted + -r: output in reverse
                    // - reverse sorted + no -r: output in reverse
                    let already_correct = (is_sorted_fwd && !reverse) || (is_sorted_rev && reverse);
                    let needs_reverse = (is_sorted_fwd && reverse) || (is_sorted_rev && !reverse);

                    if already_correct || needs_reverse {
                        let forward = already_correct;
                        if forward
                            && !config.unique
                            && !config.zero_terminated
                            && memchr::memchr(b'\r', data).is_none()
                        {
                            if data.last() == Some(&b'\n') {
                                writer.write_all_direct(data)?;
                            } else if !data.is_empty() {
                                writer.write_all(data)?;
                                writer.write_all(b"\n")?;
                            }
                            writer.flush()?;
                            return Ok(());
                        }
                        // Contiguous buffer output for already-sorted single-key path
                        let dp = data.as_ptr();
                        let term_byte = terminator[0];
                        let buf_cap = data.len() + num_lines + 1;
                        let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
                        let bptr = buf.as_mut_ptr();
                        let mut pos = 0usize;
                        let mut prev_idx: Option<usize> = None;
                        let iter: Box<dyn Iterator<Item = usize>> = if forward {
                            Box::new(0..num_lines)
                        } else {
                            Box::new((0..num_lines).rev())
                        };
                        for i in iter {
                            let (s, e) = offsets[i];
                            let len = e - s;
                            if config.unique {
                                let line = unsafe { std::slice::from_raw_parts(dp.add(s), len) };
                                let emit = match prev_idx {
                                    Some(p) => {
                                        let (ps, pe) = offsets[p];
                                        let prev_line = unsafe {
                                            std::slice::from_raw_parts(dp.add(ps), pe - ps)
                                        };
                                        compare_lines_for_dedup(prev_line, line, config)
                                            != Ordering::Equal
                                    }
                                    None => true,
                                };
                                if !emit {
                                    continue;
                                }
                                prev_idx = Some(i);
                            }
                            unsafe {
                                std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                                *bptr.add(pos + len) = term_byte;
                            }
                            pos += len + 1;
                        }
                        unsafe {
                            buf.set_len(pos);
                        }
                        writer.write_all_direct(&buf)?;
                        writer.flush()?;
                        return Ok(());
                    }
                }

                // Packed-entry radix sort for single-key lexicographic path.
                // Stores key boundaries directly in each entry, eliminating
                // random accesses to key_offs[] during comparison.
                // Entry: (key_prefix, key_start, key_end, line_idx) = 24 bytes.
                // This reduces total memory from 12MB (entries+key_offs+offsets)
                // to 6MB (packed entries) for 250K lines.
                type PackedEntry = (u64, u32, u32, u32);
                let build_packed = |i: usize, &(ks, ke): &(usize, usize)| -> PackedEntry {
                    let pfx = if ks < ke {
                        line_prefix(data, ks, ke)
                    } else {
                        0u64
                    };
                    (pfx, ks as u32, ke as u32, i as u32)
                };
                let mut entries: Vec<PackedEntry> = if num_lines > 10_000 {
                    key_offs
                        .par_iter()
                        .enumerate()
                        .map(|(i, ko)| build_packed(i, ko))
                        .collect()
                } else {
                    key_offs
                        .iter()
                        .enumerate()
                        .map(|(i, ko)| build_packed(i, ko))
                        .collect()
                };

                // Comparison using packed entries: key data is in the entry,
                // no key_offs[] lookup needed. Only offsets[] is accessed for
                // last-resort full-line comparison (rare).
                let data_addr = data.as_ptr() as usize;
                let offsets_ptr = offsets.as_ptr() as usize;
                let packed_cmp = |a: &PackedEntry, b: &PackedEntry| -> Ordering {
                    let ord = match a.0.cmp(&b.0) {
                        Ordering::Equal => {
                            let la = (a.2 - a.1) as usize;
                            let lb = (b.2 - b.1) as usize;
                            let skip = 8.min(la).min(lb);
                            unsafe {
                                let dp = data_addr as *const u8;
                                let pa = dp.add(a.1 as usize + skip);
                                let pb = dp.add(b.1 as usize + skip);
                                let rem_a = la - skip;
                                let rem_b = lb - skip;
                                let min_rem = rem_a.min(rem_b);
                                let mut i = 0usize;
                                while i + 8 <= min_rem {
                                    let wa = u64::from_be_bytes(*(pa.add(i) as *const [u8; 8]));
                                    let wb = u64::from_be_bytes(*(pb.add(i) as *const [u8; 8]));
                                    if wa != wb {
                                        return wa.cmp(&wb);
                                    }
                                    i += 8;
                                }
                                std::slice::from_raw_parts(pa.add(i), rem_a - i)
                                    .cmp(std::slice::from_raw_parts(pb.add(i), rem_b - i))
                            }
                        }
                        ord => ord,
                    };
                    if ord != Ordering::Equal {
                        return if reverse { ord.reverse() } else { ord };
                    }
                    if !stable {
                        let off = offsets_ptr as *const (usize, usize);
                        let (la, ra) = unsafe { *off.add(a.3 as usize) };
                        let (lb, rb) = unsafe { *off.add(b.3 as usize) };
                        unsafe {
                            let dp = data_addr as *const u8;
                            std::slice::from_raw_parts(dp.add(la), ra - la)
                                .cmp(std::slice::from_raw_parts(dp.add(lb), rb - lb))
                        }
                    } else {
                        Ordering::Equal
                    }
                };
                if num_lines > 4096 && !reverse && !stable {
                    // Hybrid MSD radix + parallel bucket pdqsort for single-key lex.
                    // Distribute by top byte of key prefix, then pdqsort each bucket.
                    let n = entries.len();
                    let mut temp: Vec<PackedEntry> = Vec::with_capacity(n);
                    #[allow(clippy::uninit_vec)]
                    unsafe {
                        temp.set_len(n);
                    }
                    let mut cnts = [0u32; 256];
                    for ent in entries.iter() {
                        cnts[(ent.0 >> 56) as usize] += 1;
                    }
                    let mut bk_starts = [0usize; 257];
                    {
                        let mut s = 0;
                        for i in 0..256 {
                            bk_starts[i] = s;
                            s += cnts[i] as usize;
                        }
                        bk_starts[256] = s;
                    }
                    {
                        let mut wpos = bk_starts;
                        let tptr = temp.as_mut_ptr();
                        let sptr = entries.as_ptr();
                        for idx in 0..n {
                            let ent = unsafe { *sptr.add(idx) };
                            let b = (ent.0 >> 56) as usize;
                            unsafe {
                                *tptr.add(wpos[b]) = ent;
                            }
                            wpos[b] += 1;
                        }
                    }
                    entries = temp;

                    // Parallel bucket sort with packed comparison (no key_offs lookup)
                    let entries_ptr = entries.as_mut_ptr() as usize;
                    let buckets: Vec<(usize, usize)> = (0..256)
                        .filter(|&i| bk_starts[i + 1] - bk_starts[i] > 1)
                        .map(|i| (bk_starts[i], bk_starts[i + 1]))
                        .collect();
                    // Packed comparison for parallel buckets: all data in entry
                    let pk_cmp = |a: &PackedEntry, b: &PackedEntry| -> Ordering {
                        let ord = match a.0.cmp(&b.0) {
                            Ordering::Equal => {
                                let la = (a.2 - a.1) as usize;
                                let lb = (b.2 - b.1) as usize;
                                let skip = 8.min(la).min(lb);
                                unsafe {
                                    let dp = data_addr as *const u8;
                                    let pa = dp.add(a.1 as usize + skip);
                                    let pb = dp.add(b.1 as usize + skip);
                                    let rem_a = la - skip;
                                    let rem_b = lb - skip;
                                    let min_rem = rem_a.min(rem_b);
                                    let mut i = 0usize;
                                    while i + 8 <= min_rem {
                                        let wa = u64::from_be_bytes(*(pa.add(i) as *const [u8; 8]));
                                        let wb = u64::from_be_bytes(*(pb.add(i) as *const [u8; 8]));
                                        if wa != wb {
                                            return wa.cmp(&wb);
                                        }
                                        i += 8;
                                    }
                                    std::slice::from_raw_parts(pa.add(i), rem_a - i)
                                        .cmp(std::slice::from_raw_parts(pb.add(i), rem_b - i))
                                }
                            }
                            ord => ord,
                        };
                        if ord != Ordering::Equal {
                            return ord;
                        }
                        // Last-resort: full line comparison
                        let off = offsets_ptr as *const (usize, usize);
                        let (la, ra) = unsafe { *off.add(a.3 as usize) };
                        let (lb, rb) = unsafe { *off.add(b.3 as usize) };
                        unsafe {
                            let dp = data_addr as *const u8;
                            std::slice::from_raw_parts(dp.add(la), ra - la)
                                .cmp(std::slice::from_raw_parts(dp.add(lb), rb - lb))
                        }
                    };
                    buckets.into_par_iter().for_each(|(lo, hi)| {
                        let bsize = hi - lo;
                        let group = unsafe {
                            std::slice::from_raw_parts_mut(
                                (entries_ptr as *mut PackedEntry).add(lo),
                                bsize,
                            )
                        };
                        if bsize <= 64 {
                            group.sort_unstable_by(pk_cmp);
                            return;
                        }
                        // Level-2 radix by second byte of key prefix
                        let mut cnts2 = [0u32; 256];
                        for e in group.iter() {
                            cnts2[((e.0 >> 48) & 0xFF) as usize] += 1;
                        }
                        let mut starts2 = [0usize; 257];
                        {
                            let mut s2 = 0;
                            for i in 0..256 {
                                starts2[i] = s2;
                                s2 += cnts2[i] as usize;
                            }
                            starts2[256] = s2;
                        }
                        let mut temp2: Vec<PackedEntry> = Vec::with_capacity(bsize);
                        #[allow(clippy::uninit_vec)]
                        unsafe {
                            temp2.set_len(bsize);
                        }
                        {
                            let mut wpos2 = starts2;
                            for &ent in group.iter() {
                                let b = ((ent.0 >> 48) & 0xFF) as usize;
                                temp2[wpos2[b]] = ent;
                                wpos2[b] += 1;
                            }
                        }
                        group.copy_from_slice(&temp2);
                        drop(temp2);
                        for i in 0..256 {
                            let sub_sz = starts2[i + 1] - starts2[i];
                            if sub_sz > 1 {
                                group[starts2[i]..starts2[i + 1]].sort_unstable_by(pk_cmp);
                            }
                        }
                    });
                } else if num_lines > 10_000 {
                    entries.par_sort_unstable_by(packed_cmp);
                } else if stable {
                    entries.sort_by(packed_cmp);
                } else {
                    entries.sort_unstable_by(packed_cmp);
                }
                // Output sorted packed entries using contiguous buffer.
                {
                    let dp = data.as_ptr();
                    let n = entries.len();
                    let term_byte = terminator[0];
                    if config.unique {
                        let buf_cap = data.len() + n + 1;
                        let mut buf: Vec<u8> = Vec::with_capacity(buf_cap);
                        let bptr = buf.as_mut_ptr();
                        let mut pos = 0usize;
                        let mut prev: Option<u32> = None;
                        for j in 0..n {
                            let idx = if reverse { n - 1 - j } else { j };
                            let ent = &entries[idx];
                            let li = ent.3 as usize;
                            let (s, e) = offsets[li];
                            let len = e - s;
                            let should_output = match prev {
                                Some(p) => {
                                    let pi = p as usize;
                                    let (ps, pe) = offsets[pi];
                                    let prev_line =
                                        unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                                    let line =
                                        unsafe { std::slice::from_raw_parts(dp.add(s), len) };
                                    compare_lines_for_dedup(prev_line, line, config)
                                        != Ordering::Equal
                                }
                                None => true,
                            };
                            if should_output {
                                unsafe {
                                    std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                                    *bptr.add(pos + len) = term_byte;
                                }
                                pos += len + 1;
                                prev = Some(ent.3);
                            }
                        }
                        unsafe {
                            buf.set_len(pos);
                        }
                        writer.write_all_direct(&buf)?;
                    } else {
                        let total_size = if data.last() == Some(&term_byte) {
                            data.len()
                        } else {
                            data.len() + 1
                        };
                        let mut buf: Vec<u8> = Vec::with_capacity(total_size);
                        let bptr = buf.as_mut_ptr();
                        let mut pos = 0usize;
                        for j in 0..n {
                            let actual = if reverse { n - 1 - j } else { j };
                            if j + 16 < n {
                                let ahead_idx = if reverse { n - 1 - (j + 16) } else { j + 16 };
                                let (ps, _) = offsets[entries[ahead_idx].3 as usize];
                                prefetch_read(unsafe { dp.add(ps) });
                            }
                            let ent = &entries[actual];
                            let (s, e) = offsets[ent.3 as usize];
                            let len = e - s;
                            unsafe {
                                std::ptr::copy_nonoverlapping(dp.add(s), bptr.add(pos), len);
                                *bptr.add(pos + len) = term_byte;
                            }
                            pos += len + 1;
                        }
                        unsafe {
                            buf.set_len(pos);
                        }
                        writer.write_all_direct(&buf)?;
                    }
                }
            } else {
                // Pre-select comparator: eliminates per-comparison option branching
                let mut indices: Vec<usize> = (0..num_lines).collect();
                let (cmp_fn, needs_blank, needs_reverse) = select_comparator(opts, random_seed);
                let dp_sk = data.as_ptr() as usize;
                do_sort(&mut indices, stable, |&a, &b| {
                    let dp = dp_sk as *const u8;
                    let (sa, ea) = key_offs[a];
                    let (sb, eb) = key_offs[b];
                    let ka = if sa == ea {
                        &[] as &[u8]
                    } else if needs_blank {
                        skip_leading_blanks(unsafe {
                            std::slice::from_raw_parts(dp.add(sa), ea - sa)
                        })
                    } else {
                        unsafe { std::slice::from_raw_parts(dp.add(sa), ea - sa) }
                    };
                    let kb = if sb == eb {
                        &[] as &[u8]
                    } else if needs_blank {
                        skip_leading_blanks(unsafe {
                            std::slice::from_raw_parts(dp.add(sb), eb - sb)
                        })
                    } else {
                        unsafe { std::slice::from_raw_parts(dp.add(sb), eb - sb) }
                    };
                    let ord = cmp_fn(ka, kb);
                    let ord = if needs_reverse { ord.reverse() } else { ord };
                    if ord == Ordering::Equal && !stable {
                        let (la, ra) = offsets[a];
                        let (lb, rb) = offsets[b];
                        unsafe {
                            std::slice::from_raw_parts(dp.add(la), ra - la)
                                .cmp(std::slice::from_raw_parts(dp.add(lb), rb - lb))
                        }
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

        let dp_mk = data.as_ptr() as usize;
        do_sort(&mut indices, stable, |&a, &b| {
            let dp = dp_mk as *const u8;
            for (ki, &(cmp_fn, needs_blank, needs_reverse)) in comparators.iter().enumerate() {
                let (sa, ea) = all_key_offs[ki][a];
                let (sb, eb) = all_key_offs[ki][b];
                let ka = if sa == ea {
                    &[] as &[u8]
                } else if needs_blank {
                    skip_leading_blanks(unsafe { std::slice::from_raw_parts(dp.add(sa), ea - sa) })
                } else {
                    unsafe { std::slice::from_raw_parts(dp.add(sa), ea - sa) }
                };
                let kb = if sb == eb {
                    &[] as &[u8]
                } else if needs_blank {
                    skip_leading_blanks(unsafe { std::slice::from_raw_parts(dp.add(sb), eb - sb) })
                } else {
                    unsafe { std::slice::from_raw_parts(dp.add(sb), eb - sb) }
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
                let (sa, ea) = offsets[a];
                let (sb, eb) = offsets[b];
                unsafe {
                    std::slice::from_raw_parts(dp.add(sa), ea - sa)
                        .cmp(std::slice::from_raw_parts(dp.add(sb), eb - sb))
                }
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

        let dp_gen_key = data.as_ptr() as usize;
        do_sort(&mut indices, stable, |&a, &b| {
            let dp = dp_gen_key as *const u8;
            let (sa, ea) = offsets[a];
            let (sb, eb) = offsets[b];
            let la = unsafe { std::slice::from_raw_parts(dp.add(sa), ea - sa) };
            let lb = unsafe { std::slice::from_raw_parts(dp.add(sb), eb - sb) };

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
        let dp_addr = data.as_ptr() as usize;

        do_sort(&mut indices, stable, |&a, &b| {
            let (sa, ea) = offsets[a];
            let (sb, eb) = offsets[b];
            let dp = dp_addr as *const u8;
            let la = unsafe { std::slice::from_raw_parts(dp.add(sa), ea - sa) };
            let lb = unsafe { std::slice::from_raw_parts(dp.add(sb), eb - sb) };
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
                unsafe {
                    std::slice::from_raw_parts(dp.add(sa), ea - sa)
                        .cmp(std::slice::from_raw_parts(dp.add(sb), eb - sb))
                }
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
