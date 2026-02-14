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

/// Hybrid MSD radix sort for lexicographic (u64, u32, u32) entries.
///
/// Two-level MSD (Most Significant Digit) radix:
/// 1. First pass: distribute by top 16 bits (bits 48-63) → 65536 buckets
/// 2. Second pass within each large bucket: distribute by next 16 bits (bits 32-47)
/// 3. Remaining sub-buckets: comparison sort (typically 0-2 entries)
///
/// This is more cache-friendly than full LSD radix (which makes 4 full O(n)
/// passes over all data with random scatter writes). MSD touches the data once
/// for the first pass, then only touches the entries in each bucket for the
/// second pass. Most buckets are small (≤30 entries for 2M lines), so the
/// second pass has excellent cache locality.
///
/// Key advantage over comparison sort alone: the first radix pass eliminates
/// the top 16 bits from ALL comparisons. The second radix pass eliminates
/// another 16 bits for larger buckets. Only the bottom 32 bits (4 bytes of
/// the prefix) need comparison sort, which resolves almost instantly.
fn radix_sort_entries(
    entries: Vec<(u64, u32, u32)>,
    data: &[u8],
    stable: bool,
    _reverse: bool,
) -> Vec<(u64, u32, u32)> {
    let n = entries.len();
    if n <= 1 {
        return entries;
    }

    // === PASS 1: MSD radix on top 8 bits (bits 56-63) ===
    // Using 256 buckets for cache-friendly scatter:
    // - wpos array (256 * 8B = 2KB) fits in L1 cache
    // - Consecutive writes to same bucket are ~256 entries apart,
    //   keeping cache lines warm in L3 for reuse
    // - The larger buckets (~n/256) are then sorted in parallel by Level 2
    let nbk: usize = 256;
    let mut cnts = vec![0u32; nbk];
    for &(pfx, _, _) in &entries {
        cnts[(pfx >> 56) as usize] += 1;
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
    // Scatter into sorted array with software prefetching.
    let mut sorted: Vec<(u64, u32, u32)> = Vec::with_capacity(n);
    #[allow(clippy::uninit_vec)]
    unsafe {
        sorted.set_len(n);
    }
    {
        let mut wpos = bk_starts.clone();
        let sptr = sorted.as_mut_ptr();
        let eptr = entries.as_ptr();
        let prefetch_dist = 8usize;
        for idx in 0..n {
            let ent = unsafe { *eptr.add(idx) };
            let b = (ent.0 >> 56) as usize;
            unsafe {
                *sptr.add(wpos[b]) = ent;
            }
            wpos[b] += 1;
            // Prefetch destination for an entry a few iterations ahead
            if idx + prefetch_dist < n {
                let future_ent = unsafe { *eptr.add(idx + prefetch_dist) };
                let future_b = (future_ent.0 >> 56) as usize;
                let future_pos = wpos[future_b];
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    std::arch::x86_64::_mm_prefetch(
                        sptr.add(future_pos) as *const i8,
                        std::arch::x86_64::_MM_HINT_T0,
                    );
                }
                // aarch64 prefetch requires nightly, skip it
                #[cfg(not(target_arch = "x86_64"))]
                let _ = future_pos;
            }
        }
    }
    drop(entries);
    drop(cnts);

    // === PASS 2: Within each bucket, uniform 8-bit MSD radix ===
    // Level 1 sorted byte 0 (bits 56-63). Level 2 sorts byte 1 (bits 48-55).
    // Level 3 sorts byte 2 (bits 40-47) if needed.
    // Each level uses 256 buckets: count array = 1KB (L1 cache),
    // and within-bucket scatter stays in L2 (80KB for ~5000 entries).
    {
        let sorted_ptr = sorted.as_mut_ptr() as usize;
        let data_addr = data.as_ptr() as usize;

        // Comparison function for tiebreaking within sub-buckets.
        // Starts comparison after the prefix bytes (skip first 8 or min(8,len) bytes).
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
                        let tail_a = std::slice::from_raw_parts(pa.add(i), rem_a - i);
                        let tail_b = std::slice::from_raw_parts(pb.add(i), rem_b - i);
                        tail_a.cmp(tail_b)
                    }
                }
                ord => ord,
            }
        };

        // Collect non-trivial buckets for parallel processing
        let mut slices: Vec<(usize, usize)> = Vec::new();
        for i in 0..nbk {
            let lo = bk_starts[i];
            let hi = bk_starts[i + 1];
            if hi - lo > 1 {
                slices.push((lo, hi));
            }
        }

        // Process buckets in parallel with rayon
        let chunk_fn = |(lo, hi): (usize, usize)| {
            let bucket = unsafe {
                std::slice::from_raw_parts_mut(
                    (sorted_ptr as *mut (u64, u32, u32)).add(lo),
                    hi - lo,
                )
            };
            let blen = bucket.len();

            // For large buckets (>64): second-level 8-bit MSD radix on byte 1 (bits 48-55).
            if blen > 64 {
                let check_shift = 48u32;
                let check_mask: u64 = 0xFF;

                // Check if byte 1 has variation
                let first_bits = (bucket[0].0 >> check_shift) & check_mask;
                let mut has_variation = false;
                for e in &bucket[1..] {
                    if ((e.0 >> check_shift) & check_mask) != first_bits {
                        has_variation = true;
                        break;
                    }
                }

                if has_variation {
                    let sub_nbk: usize = 256;
                    let shift = check_shift;
                    let mask = check_mask;

                    let mut sub_cnts = vec![0u32; sub_nbk];
                    for &(pfx, _, _) in bucket.iter() {
                        sub_cnts[((pfx >> shift) & mask) as usize] += 1;
                    }
                    let mut sub_starts = vec![0usize; sub_nbk + 1];
                    {
                        let mut s = 0usize;
                        for i in 0..sub_nbk {
                            sub_starts[i] = s;
                            s += sub_cnts[i] as usize;
                        }
                        sub_starts[sub_nbk] = s;
                    }
                    // Scatter within bucket using temp buffer
                    let mut temp: Vec<(u64, u32, u32)> = Vec::with_capacity(blen);
                    #[allow(clippy::uninit_vec)]
                    unsafe {
                        temp.set_len(blen);
                    }
                    {
                        let mut wpos = sub_starts.clone();
                        let temp_ptr = temp.as_mut_ptr();
                        for &ent in bucket.iter() {
                            let b = ((ent.0 >> shift) & mask) as usize;
                            unsafe {
                                *temp_ptr.add(wpos[b]) = ent;
                            }
                            wpos[b] += 1;
                        }
                    }
                    // Copy back and sort sub-buckets
                    bucket.copy_from_slice(&temp);
                    drop(temp);
                    // Sort each non-trivial sub-bucket
                    for sb in 0..sub_nbk {
                        let slo = sub_starts[sb];
                        let shi = sub_starts[sb + 1];
                        if shi - slo <= 1 {
                            continue;
                        }
                        let sub_slice = &mut bucket[slo..shi];
                        let sub_len = sub_slice.len();
                        // Check for all-identical content
                        let ref_s = sub_slice[0].1 as usize;
                        let ref_l = sub_slice[0].2 as usize;
                        let last_s = sub_slice[sub_len - 1].1 as usize;
                        let last_l = sub_slice[sub_len - 1].2 as usize;
                        let all_same = ref_l == last_l
                            && unsafe {
                                let ddp = data_addr as *const u8;
                                let a = std::slice::from_raw_parts(ddp.add(ref_s), ref_l);
                                let b = std::slice::from_raw_parts(ddp.add(last_s), last_l);
                                a == b
                            };
                        if all_same {
                            continue;
                        }

                        // 3rd-level 8-bit radix on byte 2 (bits 40-47)
                        if sub_len > 64 {
                            let r3_shift = 40u32;
                            let r3_mask: u64 = 0xFF;
                            let r3_first = (sub_slice[0].0 >> r3_shift) & r3_mask;
                            let mut r3_has_var = false;
                            for e in &sub_slice[1..] {
                                if ((e.0 >> r3_shift) & r3_mask) != r3_first {
                                    r3_has_var = true;
                                    break;
                                }
                            }
                            if r3_has_var {
                                let r3_nbk: usize = 256;
                                let r3_s = r3_shift;
                                let r3_m = r3_mask;
                                let mut r3_cnts = vec![0u32; r3_nbk];
                                for &(pfx, _, _) in sub_slice.iter() {
                                    r3_cnts[((pfx >> r3_s) & r3_m) as usize] += 1;
                                }
                                let mut r3_starts = vec![0usize; r3_nbk + 1];
                                {
                                    let mut s = 0usize;
                                    for i in 0..r3_nbk {
                                        r3_starts[i] = s;
                                        s += r3_cnts[i] as usize;
                                    }
                                    r3_starts[r3_nbk] = s;
                                }
                                let mut r3_temp: Vec<(u64, u32, u32)> = Vec::with_capacity(sub_len);
                                #[allow(clippy::uninit_vec)]
                                unsafe {
                                    r3_temp.set_len(sub_len);
                                }
                                {
                                    let mut wpos = r3_starts.clone();
                                    let tp = r3_temp.as_mut_ptr();
                                    for &ent in sub_slice.iter() {
                                        let b = ((ent.0 >> r3_s) & r3_m) as usize;
                                        unsafe {
                                            *tp.add(wpos[b]) = ent;
                                        }
                                        wpos[b] += 1;
                                    }
                                }
                                sub_slice.copy_from_slice(&r3_temp);
                                drop(r3_temp);
                                // 4th-level 8-bit radix on byte 3 (bits 32-39) for still-large buckets
                                for rb in 0..r3_nbk {
                                    let rlo = r3_starts[rb];
                                    let rhi = r3_starts[rb + 1];
                                    if rhi - rlo <= 1 {
                                        continue;
                                    }
                                    let rs = &mut sub_slice[rlo..rhi];
                                    if rs.len() > 64 {
                                        radix_sort_byte(rs, 32, stable, &cmp_fn);
                                    } else if rs.len() <= 16 {
                                        for k in 1..rs.len() {
                                            let mut pos = k;
                                            while pos > 0
                                                && cmp_fn(&rs[pos], &rs[pos - 1]) == Ordering::Less
                                            {
                                                rs.swap(pos, pos - 1);
                                                pos -= 1;
                                            }
                                        }
                                    } else if stable {
                                        rs.sort_by(cmp_fn);
                                    } else {
                                        rs.sort_unstable_by(cmp_fn);
                                    }
                                }
                                continue;
                            }
                        }

                        if sub_len <= 16 {
                            for k in 1..sub_len {
                                let mut pos = k;
                                while pos > 0
                                    && cmp_fn(&sub_slice[pos], &sub_slice[pos - 1])
                                        == Ordering::Less
                                {
                                    sub_slice.swap(pos, pos - 1);
                                    pos -= 1;
                                }
                            }
                        } else if stable {
                            sub_slice.sort_by(cmp_fn);
                        } else {
                            sub_slice.sort_unstable_by(cmp_fn);
                        }
                    }
                    return;
                }
            }

            // For small/medium buckets or no second-level variation:
            // Check for all-identical content first
            let ref_s = bucket[0].1 as usize;
            let ref_l = bucket[0].2 as usize;
            let last_s = bucket[blen - 1].1 as usize;
            let last_l = bucket[blen - 1].2 as usize;
            let all_same = ref_l == last_l
                && unsafe {
                    let ddp = data_addr as *const u8;
                    let a = std::slice::from_raw_parts(ddp.add(ref_s), ref_l);
                    let b = std::slice::from_raw_parts(ddp.add(last_s), last_l);
                    a == b
                };
            if all_same {
                return;
            }

            // Comparison sort (insertion sort for tiny buckets, pdqsort for larger)
            if blen <= 16 {
                for k in 1..blen {
                    let mut pos = k;
                    while pos > 0 && cmp_fn(&bucket[pos], &bucket[pos - 1]) == Ordering::Less {
                        bucket.swap(pos, pos - 1);
                        pos -= 1;
                    }
                }
            } else if stable {
                bucket.sort_by(cmp_fn);
            } else {
                bucket.sort_unstable_by(cmp_fn);
            }
        };

        if slices.len() > 4 {
            slices
                .into_par_iter()
                .for_each(|(lo, hi)| chunk_fn((lo, hi)));
        } else {
            for (lo, hi) in slices {
                chunk_fn((lo, hi));
            }
        }
    }

    sorted
}

/// Single-byte radix sort helper for Level 4+.
/// Sorts a slice of entries by an 8-bit radix at the given shift position,
/// then comparison-sorts any remaining large sub-buckets.
fn radix_sort_byte(
    slice: &mut [(u64, u32, u32)],
    shift: u32,
    stable: bool,
    cmp_fn: &impl Fn(&(u64, u32, u32), &(u64, u32, u32)) -> Ordering,
) {
    let slen = slice.len();
    let mask: u64 = 0xFF;

    let first_bits = (slice[0].0 >> shift) & mask;
    let mut has_var = false;
    for e in &slice[1..] {
        if ((e.0 >> shift) & mask) != first_bits {
            has_var = true;
            break;
        }
    }
    if !has_var {
        // All same at this byte level — fall through to comparison sort
        if stable {
            slice.sort_by(cmp_fn);
        } else {
            slice.sort_unstable_by(cmp_fn);
        }
        return;
    }

    let nbk: usize = 256;
    let mut cnts = vec![0u32; nbk];
    for &(pfx, _, _) in slice.iter() {
        cnts[((pfx >> shift) & mask) as usize] += 1;
    }
    let mut starts = vec![0usize; nbk + 1];
    {
        let mut s = 0usize;
        for i in 0..nbk {
            starts[i] = s;
            s += cnts[i] as usize;
        }
        starts[nbk] = s;
    }
    let mut temp: Vec<(u64, u32, u32)> = Vec::with_capacity(slen);
    #[allow(clippy::uninit_vec)]
    unsafe {
        temp.set_len(slen);
    }
    {
        let mut wpos = starts.clone();
        let tp = temp.as_mut_ptr();
        for &ent in slice.iter() {
            let b = ((ent.0 >> shift) & mask) as usize;
            unsafe {
                *tp.add(wpos[b]) = ent;
            }
            wpos[b] += 1;
        }
    }
    slice.copy_from_slice(&temp);
    drop(temp);
    for b in 0..nbk {
        let lo = starts[b];
        let hi = starts[b + 1];
        if hi - lo <= 1 {
            continue;
        }
        let rs = &mut slice[lo..hi];
        if rs.len() <= 16 {
            for k in 1..rs.len() {
                let mut pos = k;
                while pos > 0 && cmp_fn(&rs[pos], &rs[pos - 1]) == Ordering::Less {
                    rs.swap(pos, pos - 1);
                    pos -= 1;
                }
            }
        } else if stable {
            rs.sort_by(cmp_fn);
        } else {
            rs.sort_unstable_by(cmp_fn);
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
    writer: &mut impl Write,
    terminator: &[u8],
) -> io::Result<()> {
    let dp = data.as_ptr();
    if config.unique {
        let mut prev: Option<usize> = None;
        const UBATCH: usize = 512;
        let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(UBATCH * 2);
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
                slices.push(io::IoSlice::new(line));
                slices.push(io::IoSlice::new(terminator));
                if slices.len() >= UBATCH * 2 {
                    write_all_vectored(writer, &slices)?;
                    slices.clear();
                }
                prev = Some(idx);
            }
        }
        if !slices.is_empty() {
            write_all_vectored(writer, &slices)?;
        }
    } else {
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
        const UBATCH: usize = 512;
        let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(UBATCH * 2);
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
                slices.push(io::IoSlice::new(line));
                slices.push(io::IoSlice::new(terminator));
                if slices.len() >= UBATCH * 2 {
                    write_all_vectored(writer, &slices)?;
                    slices.clear();
                }
                prev = Some(idx);
            }
        }
        if !slices.is_empty() {
            write_all_vectored(writer, &slices)?;
        }
    } else {
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
                    writer.write_all(data)?;
                } else if !data.is_empty() {
                    writer.write_all(data)?;
                    writer.write_all(b"\n")?;
                }
                writer.flush()?;
                return Ok(());
            }

            // Line-by-line output for unique/\r\n/zero-terminated cases
            if config.unique {
                let dp = data.as_ptr();
                let mut prev: Option<usize> = None;
                const BATCH: usize = 512;
                let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
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
                        slices.push(io::IoSlice::new(line));
                        slices.push(io::IoSlice::new(terminator));
                        if slices.len() >= BATCH * 2 {
                            write_all_vectored(&mut writer, &slices)?;
                            slices.clear();
                        }
                        prev = Some(i);
                    }
                }
                if !slices.is_empty() {
                    write_all_vectored(&mut writer, &slices)?;
                }
            } else {
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

        // Reverse-sorted detection: if data is in descending order and user wants
        // ascending (or vice versa with -r), just reverse the offsets array.
        // This is O(n) instead of O(n log n), turning the "reverse sorted" case
        // from 2.7x to near-instant (like the already-sorted case).
        if is_reverse_sorted && !config.unique {
            // Zero-copy writev: output lines in reverse directly from mmap data.
            let dp = data.as_ptr();
            const BATCH: usize = 512;
            let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
            for i in (0..num_lines).rev() {
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
            writer.flush()?;
            return Ok(());
        }

        if is_reverse_sorted && config.unique {
            // Reverse-sorted with unique: output in reverse with dedup
            let dp = data.as_ptr();
            let mut prev: Option<usize> = None;
            const BATCH: usize = 512;
            let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
            for i in (0..num_lines).rev() {
                let (s, e) = offsets[i];
                let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
                let emit = match prev {
                    Some(p) => {
                        let (ps, pe) = offsets[p];
                        let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pe - ps) };
                        compare_lines_for_dedup(prev_line, line, config) != Ordering::Equal
                    }
                    None => true,
                };
                if emit {
                    slices.push(io::IoSlice::new(line));
                    slices.push(io::IoSlice::new(terminator));
                    if slices.len() >= BATCH * 2 {
                        write_all_vectored(&mut writer, &slices)?;
                        slices.clear();
                    }
                    prev = Some(i);
                }
            }
            if !slices.is_empty() {
                write_all_vectored(&mut writer, &slices)?;
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

        // Full LSD radix sort on 8-byte prefix + tiebreak for collisions
        let sorted = radix_sort_entries(entries, data, config.stable, reverse);

        // Switch to sequential for output phase
        #[cfg(target_os = "linux")]
        if let FileData::Mmap(ref mmap) = buffer {
            let _ = mmap.advise(memmap2::Advice::Sequential);
        }

        // Output sorted entries. For reverse, iterate buckets from high to low
        // (within each bucket, entries are already in reverse order from the sort).
        // For forward, iterate linearly through the sorted array.
        if config.unique {
            // After full LSD radix sort, entries are in ascending order.
            // For reverse, iterate backwards; for forward, iterate forwards.
            // Optimized dedup: use prefix u64 as first-pass rejection.
            // If prefixes differ, lines are definitely not equal (skip memcmp).
            // Uses write_vectored batching to reduce syscall overhead.
            let dp = data.as_ptr();
            let mut prev_prefix = u64::MAX;
            let mut prev_start = u32::MAX;
            let mut prev_len = 0u32;
            let n_sorted = sorted.len();
            let mut idx: usize = 0;
            const BATCH: usize = 512;
            let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
            while idx < n_sorted {
                let actual_idx = if reverse { n_sorted - 1 - idx } else { idx };
                let (pfx, s, l) = sorted[actual_idx];
                let su = s as usize;
                let lu = l as usize;
                let line = unsafe { std::slice::from_raw_parts(dp.add(su), lu) };
                let emit = prev_start == u32::MAX || pfx != prev_prefix || {
                    let ps = prev_start as usize;
                    let pl = prev_len as usize;
                    if pl != lu {
                        true
                    } else {
                        let prev_line = unsafe { std::slice::from_raw_parts(dp.add(ps), pl) };
                        prev_line != line
                    }
                };
                if emit {
                    slices.push(io::IoSlice::new(line));
                    slices.push(io::IoSlice::new(terminator));
                    if slices.len() >= BATCH * 2 {
                        write_all_vectored(&mut writer, &slices)?;
                        slices.clear();
                    }
                    prev_prefix = pfx;
                    prev_start = s;
                    prev_len = l;
                }
                idx += 1;
            }
            if !slices.is_empty() {
                write_all_vectored(&mut writer, &slices)?;
            }
        } else if reverse {
            // Reverse: zero-copy writev directly from mmap data in reverse order.
            // Eliminates ~110MB allocation for 100MB files.
            let dp = data.as_ptr();
            const BATCH: usize = 512;
            let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
            for &(_, s, l) in sorted.iter().rev() {
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
        } else {
            // Forward: zero-copy writev directly from mmap data.
            // Eliminates ~110MB allocation (out_offsets + out_buf) for 100MB files.
            // BufWriter(16MB) handles batching; writev syscall count is minimal.
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
                // Uses 8-byte prefix comparison on extracted keys for fast detection.
                // This is valuable for pre-sorted CSV data (e.g., sort -t, -k2 on already-sorted).
                if num_lines > 1 {
                    let mut is_sorted = true;
                    let mut prev_pfx = if key_offs[0].0 < key_offs[0].1 {
                        line_prefix(data, key_offs[0].0, key_offs[0].1)
                    } else {
                        0u64
                    };
                    for i in 1..num_lines {
                        let (ks, ke) = key_offs[i];
                        let cur_pfx = if ks < ke {
                            line_prefix(data, ks, ke)
                        } else {
                            0u64
                        };
                        if cur_pfx < prev_pfx {
                            is_sorted = false;
                            break;
                        }
                        if cur_pfx == prev_pfx {
                            let (ps, pe) = key_offs[i - 1];
                            let pk = &data[ps..pe];
                            let ck = &data[ks..ke];
                            if pk > ck {
                                is_sorted = false;
                                break;
                            }
                            // Last-resort whole-line comparison for equal keys
                            if !config.stable && pk == ck {
                                let (ls, le) = offsets[i - 1];
                                let (cs, ce) = offsets[i];
                                if data[ls..le] > data[cs..ce] {
                                    is_sorted = false;
                                    break;
                                }
                            }
                        }
                        prev_pfx = cur_pfx;
                    }

                    if is_sorted {
                        // Already sorted: output directly
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
                        // Line-by-line output for unique/special cases
                        let dp = data.as_ptr();
                        if config.unique {
                            let mut prev: Option<usize> = None;
                            const BATCH: usize = 512;
                            let mut slices: Vec<io::IoSlice<'_>> = Vec::with_capacity(BATCH * 2);
                            for i in 0..num_lines {
                                let (s, e) = offsets[i];
                                let line = unsafe { std::slice::from_raw_parts(dp.add(s), e - s) };
                                let emit = match prev {
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
                                if emit {
                                    slices.push(io::IoSlice::new(line));
                                    slices.push(io::IoSlice::new(terminator));
                                    if slices.len() >= BATCH * 2 {
                                        write_all_vectored(&mut writer, &slices)?;
                                        slices.clear();
                                    }
                                    prev = Some(i);
                                }
                            }
                            if !slices.is_empty() {
                                write_all_vectored(&mut writer, &slices)?;
                            }
                        } else {
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

                    // Sort each bucket with full key comparison.
                    // Uses raw pointer arithmetic to eliminate bounds checking in the
                    // comparison hot path. The 8-byte prefix is already resolved by
                    // the radix pass, so we only compare bytes after the prefix.
                    {
                        let sorted_ptr = sorted.as_mut_ptr() as usize;
                        let _sorted_len = sorted.len();
                        let data_addr = data.as_ptr() as usize;
                        let bucket_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                            let ord = match a.0.cmp(&b.0) {
                                Ordering::Equal => {
                                    let (sa, ea) = key_offs[a.1];
                                    let (sb, eb) = key_offs[b.1];
                                    let la = ea - sa;
                                    let lb = eb - sb;
                                    let skip = 8.min(la).min(lb);
                                    let rem_a = la - skip;
                                    let rem_b = lb - skip;
                                    unsafe {
                                        let dp = data_addr as *const u8;
                                        let pa = dp.add(sa + skip);
                                        let pb = dp.add(sb + skip);
                                        let min_rem = rem_a.min(rem_b);
                                        let mut i = 0usize;
                                        while i + 8 <= min_rem {
                                            let wa =
                                                u64::from_be_bytes(*(pa.add(i) as *const [u8; 8]));
                                            let wb =
                                                u64::from_be_bytes(*(pb.add(i) as *const [u8; 8]));
                                            if wa != wb {
                                                return wa.cmp(&wb);
                                            }
                                            i += 8;
                                        }
                                        let tail_a =
                                            std::slice::from_raw_parts(pa.add(i), rem_a - i);
                                        let tail_b =
                                            std::slice::from_raw_parts(pb.add(i), rem_b - i);
                                        tail_a.cmp(tail_b)
                                    }
                                }
                                ord => ord,
                            };
                            if ord != Ordering::Equal {
                                return ord;
                            }
                            if !stable {
                                let (la, ra) = offsets[a.1];
                                let (lb, rb) = offsets[b.1];
                                unsafe {
                                    let dp = data_addr as *const u8;
                                    std::slice::from_raw_parts(dp.add(la), ra - la)
                                        .cmp(std::slice::from_raw_parts(dp.add(lb), rb - lb))
                                }
                            } else {
                                Ordering::Equal
                            }
                        };
                        // Collect non-trivial buckets and process them.
                        // For large buckets (>64): second-level radix on bits 32-47.
                        let mut slices: Vec<(usize, usize)> = Vec::new();
                        for i in 0..nbk {
                            let lo = bk_starts[i];
                            let hi = bk_starts[i + 1];
                            if hi - lo > 1 {
                                slices.push((lo, hi));
                            }
                        }
                        let chunk_fn = |(lo, hi): (usize, usize)| {
                            let bucket = unsafe {
                                std::slice::from_raw_parts_mut(
                                    (sorted_ptr as *mut (u64, usize)).add(lo),
                                    hi - lo,
                                )
                            };
                            let blen = bucket.len();

                            // Second-level radix for large buckets.
                            // Uses 16-bit radix (65536 buckets) for very large buckets (>512),
                            // 8-bit radix (256 buckets) for medium buckets (65-512).
                            if blen > 64 {
                                let use_wide = blen > 512;
                                let check_shift = if use_wide { 32u32 } else { 40u32 };
                                let check_mask: u64 = if use_wide { 0xFFFF } else { 0xFF };
                                let first_bits = (bucket[0].0 >> check_shift) & check_mask;
                                let mut has_variation = false;
                                for e in &bucket[1..] {
                                    if ((e.0 >> check_shift) & check_mask) != first_bits {
                                        has_variation = true;
                                        break;
                                    }
                                }
                                if has_variation {
                                    let sub_nbk: usize = if use_wide { 65536 } else { 256 };
                                    let shift = check_shift;
                                    let mask = check_mask;
                                    let mut sub_cnts = vec![0u32; sub_nbk];
                                    for &(pfx, _) in bucket.iter() {
                                        sub_cnts[((pfx >> shift) & mask) as usize] += 1;
                                    }
                                    let mut sub_starts = vec![0usize; sub_nbk + 1];
                                    {
                                        let mut s = 0usize;
                                        for i in 0..sub_nbk {
                                            sub_starts[i] = s;
                                            s += sub_cnts[i] as usize;
                                        }
                                        sub_starts[sub_nbk] = s;
                                    }
                                    let mut temp: Vec<(u64, usize)> = Vec::with_capacity(blen);
                                    #[allow(clippy::uninit_vec)]
                                    unsafe {
                                        temp.set_len(blen);
                                    }
                                    {
                                        let mut wpos = sub_starts.clone();
                                        let temp_ptr = temp.as_mut_ptr();
                                        for &ent in bucket.iter() {
                                            let b = ((ent.0 >> shift) & mask) as usize;
                                            unsafe {
                                                *temp_ptr.add(wpos[b]) = ent;
                                            }
                                            wpos[b] += 1;
                                        }
                                    }
                                    bucket.copy_from_slice(&temp);
                                    drop(temp);
                                    for sb in 0..sub_nbk {
                                        let slo = sub_starts[sb];
                                        let shi = sub_starts[sb + 1];
                                        let sub_len = shi - slo;
                                        if sub_len <= 1 {
                                            continue;
                                        }
                                        let sub = &mut bucket[slo..shi];
                                        // 3rd-level radix on bits 16-31 for large sub-buckets
                                        if sub_len > 64 {
                                            let r3_shift = 16u32;
                                            let r3_mask: u64 = 0xFFFF;
                                            let r3_first = (sub[0].0 >> r3_shift) & r3_mask;
                                            let mut r3_has_var = false;
                                            for e in &sub[1..] {
                                                if ((e.0 >> r3_shift) & r3_mask) != r3_first {
                                                    r3_has_var = true;
                                                    break;
                                                }
                                            }
                                            if r3_has_var {
                                                let r3_nbk: usize = 256;
                                                let r3_s = 24u32;
                                                let r3_m: u64 = 0xFF;
                                                let mut r3_cnts = vec![0u32; r3_nbk];
                                                for &(pfx, _) in sub.iter() {
                                                    r3_cnts[((pfx >> r3_s) & r3_m) as usize] += 1;
                                                }
                                                let mut r3_starts = vec![0usize; r3_nbk + 1];
                                                {
                                                    let mut s = 0usize;
                                                    for i in 0..r3_nbk {
                                                        r3_starts[i] = s;
                                                        s += r3_cnts[i] as usize;
                                                    }
                                                    r3_starts[r3_nbk] = s;
                                                }
                                                let mut r3_temp: Vec<(u64, usize)> =
                                                    Vec::with_capacity(sub_len);
                                                #[allow(clippy::uninit_vec)]
                                                unsafe {
                                                    r3_temp.set_len(sub_len);
                                                }
                                                {
                                                    let mut wpos = r3_starts.clone();
                                                    let tp = r3_temp.as_mut_ptr();
                                                    for &ent in sub.iter() {
                                                        let b = ((ent.0 >> r3_s) & r3_m) as usize;
                                                        unsafe {
                                                            *tp.add(wpos[b]) = ent;
                                                        }
                                                        wpos[b] += 1;
                                                    }
                                                }
                                                sub.copy_from_slice(&r3_temp);
                                                drop(r3_temp);
                                                for rb in 0..r3_nbk {
                                                    let rlo = r3_starts[rb];
                                                    let rhi = r3_starts[rb + 1];
                                                    if rhi - rlo > 1 {
                                                        let rs = &mut sub[rlo..rhi];
                                                        if stable {
                                                            rs.sort_by(bucket_cmp);
                                                        } else {
                                                            rs.sort_unstable_by(bucket_cmp);
                                                        }
                                                    }
                                                }
                                                continue;
                                            }
                                        }
                                        if stable {
                                            sub.sort_by(bucket_cmp);
                                        } else {
                                            sub.sort_unstable_by(bucket_cmp);
                                        }
                                    }
                                    return;
                                }
                            }

                            // Comparison sort for small buckets or no variation
                            if blen <= 16 {
                                for k in 1..blen {
                                    let mut pos = k;
                                    while pos > 0
                                        && bucket_cmp(&bucket[pos], &bucket[pos - 1])
                                            == Ordering::Less
                                    {
                                        bucket.swap(pos, pos - 1);
                                        pos -= 1;
                                    }
                                }
                            } else if stable {
                                bucket.sort_by(bucket_cmp);
                            } else {
                                bucket.sort_unstable_by(bucket_cmp);
                            }
                        };

                        if slices.len() > 16 {
                            slices
                                .into_par_iter()
                                .for_each(|(lo, hi)| chunk_fn((lo, hi)));
                        } else {
                            for (lo, hi) in slices {
                                chunk_fn((lo, hi));
                            }
                        }
                    }

                    if reverse {
                        sorted.reverse();
                    }
                    write_sorted_entries(data, &offsets, &sorted, config, &mut writer, terminator)?;
                } else {
                    // Small input: comparison-based sort
                    let dp_small = data.as_ptr();
                    let prefix_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                        let ord = match a.0.cmp(&b.0) {
                            Ordering::Equal => {
                                let (sa, ea) = key_offs[a.1];
                                let (sb, eb) = key_offs[b.1];
                                let skip = 8.min(ea - sa).min(eb - sb);
                                unsafe {
                                    std::slice::from_raw_parts(
                                        dp_small.add(sa + skip),
                                        ea - sa - skip,
                                    )
                                    .cmp(
                                        std::slice::from_raw_parts(
                                            dp_small.add(sb + skip),
                                            eb - sb - skip,
                                        ),
                                    )
                                }
                            }
                            ord => ord,
                        };
                        if ord != Ordering::Equal {
                            return if reverse { ord.reverse() } else { ord };
                        }
                        if !stable {
                            let (la, ra) = offsets[a.1];
                            let (lb, rb) = offsets[b.1];
                            unsafe {
                                std::slice::from_raw_parts(dp_small.add(la), ra - la)
                                    .cmp(std::slice::from_raw_parts(dp_small.add(lb), rb - lb))
                            }
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
