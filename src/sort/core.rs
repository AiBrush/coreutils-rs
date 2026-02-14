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

/// Radix-distribute entries into 65536 buckets by top 2 bytes of prefix,
/// then sort each bucket independently using rayon. Returns sorted array
/// and bucket boundary table. This reduces per-bucket sort size by ~60x
/// for uniform ASCII data, greatly improving cache behavior.
fn radix_sort_entries(
    entries: Vec<(u64, u32, u32)>,
    data: &[u8],
    stable: bool,
    reverse: bool,
) -> (Vec<(u64, u32, u32)>, Vec<usize>) {
    let nbk: usize = 65536;
    let mut cnts = vec![0u32; nbk];
    for &(pfx, _, _) in &entries {
        cnts[(pfx >> 48) as usize] += 1;
    }
    let mut bk_starts = vec![0usize; nbk + 1];
    {
        let mut s = 0usize;
        for i in 0..nbk {
            bk_starts[i] = s;
            s += cnts[i] as usize;
        }
        bk_starts[nbk] = s;
    }
    // Allocate without zero-init: the scatter writes every position exactly once
    // (each of the N entries maps to a unique position via the prefix sum).
    // Avoids ~15ms of page faults for 40MB zero-fill on large inputs.
    let mut sorted: Vec<(u64, u32, u32)> = Vec::with_capacity(entries.len());
    // SAFETY: every position [0..entries.len()) is written exactly once by the scatter loop below.
    #[allow(clippy::uninit_vec)]
    unsafe {
        sorted.set_len(entries.len());
    }
    {
        let mut wpos = bk_starts.clone();
        for &ent in &entries {
            let b = (ent.0 >> 48) as usize;
            unsafe {
                *sorted.as_mut_ptr().add(wpos[b]) = ent;
            }
            wpos[b] += 1;
        }
    }
    drop(entries);
    drop(cnts);

    // Sort each non-trivial bucket independently with rayon.
    // Optimized comparison: u64 prefix comparison resolves most entries,
    // only falls through to word-at-a-time for prefix collisions.
    // The skip=min(8,la,lb) optimization avoids re-comparing the prefix bytes.
    // Word-at-a-time comparison uses u64 loads for the suffix, which is ~8x faster
    // than byte-by-byte comparison on modern CPUs (single u64 cmp vs 8 byte cmps).
    {
        let sorted_ptr = sorted.as_mut_ptr();
        let sorted_len = sorted.len();
        let data_addr = data.as_ptr() as usize;
        let cmp_fn = |a: &(u64, u32, u32), b: &(u64, u32, u32)| -> Ordering {
            match a.0.cmp(&b.0) {
                Ordering::Equal => {
                    let sa = a.1 as usize;
                    let la = a.2 as usize;
                    let sb = b.1 as usize;
                    let lb = b.2 as usize;
                    let skip = 8.min(la).min(lb);
                    let rem_a = la - skip;
                    let rem_b = lb - skip;
                    unsafe {
                        let dp = data_addr as *const u8;
                        let pa = dp.add(sa + skip);
                        let pb = dp.add(sb + skip);
                        // Word-at-a-time comparison: compare 8 bytes at a time
                        // using big-endian u64 loads. This resolves most suffix
                        // collisions in 1-2 iterations instead of 8-16 byte cmps.
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
                        // Compare remaining bytes (< 8)
                        let tail_a = std::slice::from_raw_parts(pa.add(i), rem_a - i);
                        let tail_b = std::slice::from_raw_parts(pb.add(i), rem_b - i);
                        tail_a.cmp(tail_b)
                    }
                }
                ord => ord,
            }
        };
        let mut slices: Vec<&mut [(u64, u32, u32)]> = Vec::new();
        for i in 0..nbk {
            let lo = bk_starts[i];
            let hi = bk_starts[i + 1];
            if hi - lo > 1 {
                debug_assert!(hi <= sorted_len);
                slices.push(unsafe { std::slice::from_raw_parts_mut(sorted_ptr.add(lo), hi - lo) });
            }
        }
        if stable {
            if !reverse {
                slices.into_par_iter().for_each(|sl| sl.sort_by(cmp_fn));
            } else {
                slices
                    .into_par_iter()
                    .for_each(|sl| sl.sort_by(|a, b| cmp_fn(a, b).reverse()));
            }
        } else if !reverse {
            slices
                .into_par_iter()
                .for_each(|sl| sl.sort_unstable_by(cmp_fn));
        } else {
            slices
                .into_par_iter()
                .for_each(|sl| sl.sort_unstable_by(|a, b| cmp_fn(a, b).reverse()));
        }
    }

    (sorted, bk_starts)
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
        let dp = data.as_ptr();
        let mut prev: Option<usize> = None;
        for &idx in sorted_indices {
            let (s, e) = offsets[idx];
            let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
            let should_output = match prev {
                Some(p) => {
                    let (ps, pe) = offsets[p];
                    let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                    compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                }
                None => true,
            };
            if should_output {
                writer.write_all(line)?;
                writer.write_all(terminator)?;
                prev = Some(idx);
            }
        }
    } else {
        // Batch line+terminator pairs via write_vectored to reduce function call
        // overhead (2N write_all calls -> N/BATCH write_vectored calls).
        let dp = data.as_ptr();
        const BATCH: usize = 512;
        let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
        for &idx in sorted_indices {
            let (s, e) = offsets[idx];
            let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
            slices.push(io::IoSlice::new(line));
            slices.push(io::IoSlice::new(terminator));
            if slices.len() >= BATCH * 2 {
                write_all_vectored(writer, &slices)?;
                slices.clear();
            }
        }
        if !slices.is_empty() {
            write_all_vectored(writer, &slices)?;
        }
    }
    Ok(())
}

/// Write all IoSlices to the writer, handling partial writes correctly.
/// Advances past fully-consumed slices on each iteration.
fn write_all_vectored(writer: &mut impl Write, slices: &[io::IoSlice<'_>]) -> io::Result<()> {
    // Fast path: single write_vectored call usually consumes everything
    // when backed by BufWriter with a large buffer.
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
    // Find which slices were fully consumed and where the partial one starts.
    let mut consumed = n;
    for slice in slices {
        if consumed == 0 {
            writer.write_all(slice)?;
        } else if consumed >= slice.len() {
            consumed -= slice.len();
        } else {
            // Partial slice: write remaining portion
            writer.write_all(&slice[consumed..])?;
            consumed = 0;
        }
    }
    Ok(())
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
        let dp = data.as_ptr();
        let mut prev: Option<usize> = None;
        for &(_, idx) in entries {
            let (s, e) = offsets[idx];
            let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
            let should_output = match prev {
                Some(p) => {
                    let (ps, pe) = offsets[p];
                    let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                    compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                }
                None => true,
            };
            if should_output {
                writer.write_all(line)?;
                writer.write_all(terminator)?;
                prev = Some(idx);
            }
        }
    } else {
        // Batch line+terminator pairs via write_vectored to reduce function call
        // overhead (2N write_all calls -> N/BATCH write_vectored calls).
        let dp = data.as_ptr();
        const BATCH: usize = 512;
        let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
        for &(_, idx) in entries {
            let (s, e) = offsets[idx];
            let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
            slices.push(io::IoSlice::new(line));
            slices.push(io::IoSlice::new(terminator));
            if slices.len() >= BATCH * 2 {
                write_all_vectored(writer, &slices)?;
                slices.clear();
            }
        }
        if !slices.is_empty() {
            write_all_vectored(writer, &slices)?;
        }
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
            let mut i = 0;
            while i < n {
                let key = sorted[i].0;
                let mut j = i + 1;
                while j < n && sorted[j].0 == key {
                    j += 1;
                }
                if j - i > 1 {
                    sorted[i..j].sort_unstable_by(|a, b| {
                        data[offsets[a.1].0..offsets[a.1].1]
                            .cmp(&data[offsets[b.1].0..offsets[b.1].1])
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
                sorted[i..j].sort_unstable_by(|a, b| {
                    data[offsets[a.1].0..offsets[a.1].1].cmp(&data[offsets[b.1].0..offsets[b.1].1])
                });
            }
            i = j;
        }
    }

    sorted
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
    let no_keys = config.keys.is_empty();
    let gopts = &config.global_opts;
    let is_plain_lex_for_check = no_keys
        && !gopts.has_sort_type()
        && !gopts.dictionary_order
        && !gopts.ignore_case
        && !gopts.ignore_nonprinting
        && !gopts.ignore_leading_blanks
        && !gopts.reverse;

    // Pre-detect numeric mode to skip the generic sorted check.
    // Numeric mode does its own sorted check after parsing u64 values.
    let is_numeric_only_precheck = no_keys
        && (gopts.numeric || gopts.general_numeric || gopts.human_numeric)
        && !gopts.dictionary_order
        && !gopts.ignore_case
        && !gopts.ignore_nonprinting;

    if num_lines > 1 && !is_numeric_only_precheck {
        let is_sorted = if is_plain_lex_for_check {
            // Fast path: 8-byte prefix comparison for plain lexicographic sort.
            // Most lines can be resolved with a single u64 comparison, avoiding
            // full memcmp. Only falls back to full comparison on prefix collision.
            let dp = data.as_ptr();
            let mut sorted = true;
            let mut prev_prefix = line_prefix(data, offsets[0].0, offsets[0].1);
            for i in 1..num_lines {
                let (s2, e2) = offsets[i];
                let cur_prefix = line_prefix(data, s2, e2);
                if prev_prefix > cur_prefix {
                    sorted = false;
                    break;
                }
                if prev_prefix == cur_prefix {
                    // Prefix collision — full comparison needed
                    let (s1, e1) = offsets[i - 1];
                    let a = unsafe { std::slice::from_raw_parts(dp.add(s1), e1 - s1) };
                    let b = unsafe { std::slice::from_raw_parts(dp.add(s2), e2 - s2) };
                    if a > b {
                        sorted = false;
                        break;
                    }
                }
                prev_prefix = cur_prefix;
            }
            sorted
        } else {
            let mut sorted = true;
            for i in 1..num_lines {
                let (s1, e1) = offsets[i - 1];
                let (s2, e2) = offsets[i];
                let cmp = compare_lines(&data[s1..e1], &data[s2..e2], config);
                if cmp == Ordering::Greater {
                    sorted = false;
                    break;
                }
            }
            sorted
        };
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
                let dp = data.as_ptr();
                let mut prev: Option<usize> = None;
                for i in 0..num_lines {
                    let (s, e) = offsets[i];
                    let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
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
                        writer.write_all(line)?;
                        writer.write_all(terminator)?;
                        prev = Some(i);
                    }
                }
            } else {
                // Already sorted: write lines directly using write_vectored batching.
                // Zero-copy: IoSlice entries point into mmap data, no output buffer allocation.
                let dp = data.as_ptr();
                const BATCH: usize = 512;
                let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
                for i in 0..num_lines {
                    let (s, e) = offsets[i];
                    let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
                    slices.push(io::IoSlice::new(line));
                    slices.push(io::IoSlice::new(terminator));
                    if slices.len() >= BATCH * 2 {
                        write_all_vectored(&mut writer, &slices)?;
                        slices.clear();
                    }
                }
                if !slices.is_empty() {
                    write_all_vectored(&mut writer, &slices)?;
                }
            }
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

        // Two-level radix distribution + per-bucket sort via helper function
        let (sorted, bk_starts) = radix_sort_entries(entries, data, config.stable, reverse);
        let nbk = bk_starts.len() - 1;

        // Switch to sequential for output phase
        #[cfg(target_os = "linux")]
        if let FileData::Mmap(ref mmap) = buffer {
            let _ = mmap.advise(memmap2::Advice::Sequential);
        }

        // Output sorted entries. For reverse, iterate buckets from high to low
        // (within each bucket, entries are already in reverse order from the sort).
        // For forward, iterate linearly through the sorted array.
        if config.unique {
            let dp = data.as_ptr();
            let mut prev_start = u32::MAX;
            let mut prev_len = 0u32;
            if reverse {
                for bi in (0..nbk).rev() {
                    for &(_, s, l) in &sorted[bk_starts[bi]..bk_starts[bi + 1]] {
                        let su = s as usize;
                        let lu = l as usize;
                        let line = unsafe { std::slice::from_raw_parts(dp.add(su), lu) };
                        let emit = prev_start == u32::MAX || {
                            let ps = prev_start as usize;
                            let pl = prev_len as usize;
                            let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pl) };
                            compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                        };
                        if emit {
                            writer.write_all(line)?;
                            writer.write_all(terminator)?;
                            prev_start = s;
                            prev_len = l;
                        }
                    }
                }
            } else {
                for &(_, s, l) in &sorted {
                    let su = s as usize;
                    let lu = l as usize;
                    let line = unsafe { std::slice::from_raw_parts(dp.add(su), lu) };
                    let emit = prev_start == u32::MAX || {
                        let ps = prev_start as usize;
                        let pl = prev_len as usize;
                        let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pl) };
                        compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                    };
                    if emit {
                        writer.write_all(line)?;
                        writer.write_all(terminator)?;
                        prev_start = s;
                        prev_len = l;
                    }
                }
            }
        } else if reverse {
            // Reverse: write buckets from highest to lowest using write_vectored.
            // Zero-copy: IoSlice entries point directly into mmap data.
            // Avoids per-bucket Vec<u8> buffer allocations entirely.
            let dp = data.as_ptr();
            const BATCH: usize = 512;
            let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
            for bi in (0..nbk).rev() {
                for &(_, s, l) in &sorted[bk_starts[bi]..bk_starts[bi + 1]] {
                    let line =
                        unsafe { std::slice::from_raw_parts(dp.add(s as usize), l as usize) };
                    slices.push(io::IoSlice::new(line));
                    slices.push(io::IoSlice::new(terminator));
                    if slices.len() >= BATCH * 2 {
                        write_all_vectored(&mut writer, &slices)?;
                        slices.clear();
                    }
                }
            }
            if !slices.is_empty() {
                write_all_vectored(&mut writer, &slices)?;
            }
        } else {
            // Forward: write_vectored batching from sorted entries.
            // Zero-copy: IoSlice entries point directly into mmap data.
            // Eliminates the ~100MB contiguous output buffer allocation that
            // the parallel fill approach required.
            let dp = data.as_ptr();
            const BATCH: usize = 512;
            let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
            for &(_, s, l) in &sorted {
                let line = unsafe { std::slice::from_raw_parts(dp.add(s as usize), l as usize) };
                slices.push(io::IoSlice::new(line));
                slices.push(io::IoSlice::new(terminator));
                if slices.len() >= BATCH * 2 {
                    write_all_vectored(&mut writer, &slices)?;
                    slices.clear();
                }
            }
            if !slices.is_empty() {
                write_all_vectored(&mut writer, &slices)?;
            }
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
                let cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                    let ord = a.0.cmp(&b.0);
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
                // Radix + prefix sort for single-key lexicographic path.
                // Uses radix distribution on (key_prefix, line_index) pairs.
                // Resolves most ordering via the 8-byte prefix radix buckets.
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

                let n = entries.len();

                if n > 256 {
                    // Radix sort on key prefix: distributes into 65536 buckets,
                    // then sorts each bucket with full key comparison.
                    // For `-t: -k2` this gives ~1.5x speedup over comparison sort
                    // because most keys are resolved by the 2-byte radix distribution.
                    let nbk: usize = 65536;
                    let mut cnts = vec![0u32; nbk];
                    for &(pfx, _) in &entries {
                        cnts[(pfx >> 48) as usize] += 1;
                    }
                    let mut bk_starts = vec![0usize; nbk + 1];
                    {
                        let mut s = 0usize;
                        for i in 0..nbk {
                            bk_starts[i] = s;
                            s += cnts[i] as usize;
                        }
                        bk_starts[nbk] = s;
                    }
                    let mut sorted: Vec<(u64, usize)> = Vec::with_capacity(n);
                    #[allow(clippy::uninit_vec)]
                    unsafe {
                        sorted.set_len(n);
                    }
                    {
                        let mut wpos = bk_starts.clone();
                        for &ent in &entries {
                            let b = (ent.0 >> 48) as usize;
                            unsafe {
                                *sorted.as_mut_ptr().add(wpos[b]) = ent;
                            }
                            wpos[b] += 1;
                        }
                    }
                    drop(entries);

                    // Sort each bucket with full key comparison
                    {
                        let sorted_ptr = sorted.as_mut_ptr();
                        let sorted_len = sorted.len();
                        let bucket_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                            let ord = match a.0.cmp(&b.0) {
                                Ordering::Equal => {
                                    let (sa, ea) = key_offs[a.1];
                                    let (sb, eb) = key_offs[b.1];
                                    let skip = 8.min(ea - sa).min(eb - sb);
                                    data[sa + skip..ea].cmp(&data[sb + skip..eb])
                                }
                                ord => ord,
                            };
                            if ord != Ordering::Equal {
                                return ord;
                            }
                            if !stable {
                                data[offsets[a.1].0..offsets[a.1].1]
                                    .cmp(&data[offsets[b.1].0..offsets[b.1].1])
                            } else {
                                Ordering::Equal
                            }
                        };
                        let mut slices: Vec<&mut [(u64, usize)]> = Vec::new();
                        for i in 0..nbk {
                            let lo = bk_starts[i];
                            let hi = bk_starts[i + 1];
                            if hi - lo > 1 {
                                debug_assert!(hi <= sorted_len);
                                slices.push(unsafe {
                                    std::slice::from_raw_parts_mut(sorted_ptr.add(lo), hi - lo)
                                });
                            }
                        }
                        if stable {
                            slices.into_par_iter().for_each(|sl| sl.sort_by(bucket_cmp));
                        } else {
                            slices
                                .into_par_iter()
                                .for_each(|sl| sl.sort_unstable_by(bucket_cmp));
                        }
                    }

                    if reverse {
                        sorted.reverse();
                    }
                    write_sorted_entries(data, &offsets, &sorted, config, &mut writer, terminator)?;
                } else {
                    // Small input: comparison-based sort
                    let prefix_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                        let ord = match a.0.cmp(&b.0) {
                            Ordering::Equal => {
                                let (sa, ea) = key_offs[a.1];
                                let (sb, eb) = key_offs[b.1];
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
                    if stable {
                        entries.sort_by(prefix_cmp);
                    } else {
                        entries.sort_unstable_by(prefix_cmp);
                    }
                    write_sorted_entries(
                        data,
                        &offsets,
                        &entries,
                        config,
                        &mut writer,
                        terminator,
                    )?;
                }
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
