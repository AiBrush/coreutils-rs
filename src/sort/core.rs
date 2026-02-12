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
use std::io::{self, BufRead, BufReader, BufWriter, IoSlice, Read, Write};
use std::sync::Arc;

use memmap2::Mmap;
use rayon::prelude::*;

use super::compare::{
    compare_with_opts, parse_general_numeric, parse_human_numeric, parse_numeric_value,
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

/// Compare two lines using the full key chain and global options.
#[inline]
pub fn compare_lines(a: &[u8], b: &[u8], config: &SortConfig) -> Ordering {
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

        // All keys equal: last-resort comparison (whole line) unless -s
        if !config.stable {
            return a.cmp(b);
        }

        Ordering::Equal
    } else {
        // No keys: compare whole line with global opts
        compare_with_opts(a, b, &config.global_opts, config.random_seed)
    }
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
        let file = File::open(&inputs[0])
            .map_err(|e| io::Error::new(e.kind(), format!("open failed: {}: {}", &inputs[0], e)))?;
        let metadata = file.metadata()?;
        if metadata.len() > 0 {
            let mmap = unsafe { Mmap::map(&file)? };
            // Start with Sequential for line scanning, caller switches to Random for sort
            #[cfg(target_os = "linux")]
            {
                let _ = mmap.advise(memmap2::Advice::Sequential);
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
                    io::Error::new(e.kind(), format!("open failed: {}: {}", input, e))
                })?;
                file.read_to_end(&mut data)?;
            }
        }
        FileData::Owned(data)
    };

    // Find line boundaries using SIMD-accelerated memchr
    let data = &*buffer;
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
            let file = File::open(input)
                .map_err(|e| io::Error::new(e.kind(), format!("open failed: {}: {}", input, e)))?;
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
        let cmp = compare_lines(&data[s1..e1], &data[s2..e2], config);
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
    let mut buf = Vec::with_capacity(OUTPUT_BUF_SIZE);

    while let Some(std::cmp::Reverse(min)) = heap.pop() {
        let should_output = if config.unique {
            match &prev_line {
                Some(prev) => compare_lines(prev, &min.entry.line, config) != Ordering::Equal,
                None => true,
            }
        } else {
            true
        };

        if should_output {
            buf.extend_from_slice(&min.entry.line);
            buf.extend_from_slice(terminator);
            if buf.len() >= OUTPUT_BUF_SIZE {
                writer.write_all(&buf)?;
                buf.clear();
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

    if !buf.is_empty() {
        writer.write_all(&buf)?;
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
        let slice = &data[start..start + 8];
        u64::from_be_bytes([
            slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
        ])
    } else {
        let mut bytes = [0u8; 8];
        bytes[..len].copy_from_slice(&data[start..end]);
        u64::from_be_bytes(bytes)
    }
}

/// Pre-extract key byte offsets into the data buffer for all lines.
/// Avoids repeated key extraction during sort comparisons.
fn pre_extract_key_offsets(
    data: &[u8],
    offsets: &[(usize, usize)],
    key: &KeyDef,
    separator: Option<u8>,
) -> Vec<(usize, usize)> {
    offsets
        .iter()
        .map(|&(s, e)| {
            let line = &data[s..e];
            let extracted = extract_key(line, key, separator);
            if extracted.is_empty() {
                (0, 0)
            } else {
                let offset_in_data =
                    unsafe { extracted.as_ptr().offset_from(data.as_ptr()) as usize };
                (offset_in_data, offset_in_data + extracted.len())
            }
        })
        .collect()
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

/// Maximum IoSlices per writev call (Linux IOV_MAX = 1024).
const IOV_BATCH: usize = 1024;

/// Write all IoSlices, handling partial writes and batching.
fn write_all_slices(out: &mut impl Write, slices: &[IoSlice<'_>]) -> io::Result<()> {
    let mut offset = 0;
    while offset < slices.len() {
        let end = (offset + IOV_BATCH).min(slices.len());
        let n = out.write_vectored(&slices[offset..end])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write any data",
            ));
        }
        let mut remaining = n;
        while offset < end && remaining >= slices[offset].len() {
            remaining -= slices[offset].len();
            offset += 1;
        }
        if remaining > 0 && offset < end {
            out.write_all(&slices[offset][remaining..])?;
            offset += 1;
        }
    }
    Ok(())
}

/// Write sorted indices to output, with optional unique dedup.
/// Uses writev (vectored I/O) to write directly from mmap — zero intermediate copies.
fn write_sorted_output(
    data: &[u8],
    offsets: &[(usize, usize)],
    sorted_indices: &[usize],
    config: &SortConfig,
    writer: &mut impl Write,
    terminator: &[u8],
) -> io::Result<()> {
    // Build IoSlice entries pointing directly into mmap data
    let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(sorted_indices.len() * 2);

    if config.unique {
        let mut prev: Option<usize> = None;
        for &idx in sorted_indices {
            let (s, e) = offsets[idx];
            let line = &data[s..e];
            let should_output = match prev {
                Some(p) => {
                    let (ps, pe) = offsets[p];
                    compare_lines(&data[ps..pe], line, config) != Ordering::Equal
                }
                None => true,
            };
            if should_output {
                slices.push(IoSlice::new(line));
                slices.push(IoSlice::new(terminator));
                prev = Some(idx);
            }
            // Flush batch to avoid excessive memory
            if slices.len() >= IOV_BATCH {
                write_all_slices(writer, &slices)?;
                slices.clear();
            }
        }
    } else {
        for &idx in sorted_indices {
            let (s, e) = offsets[idx];
            slices.push(IoSlice::new(&data[s..e]));
            slices.push(IoSlice::new(terminator));
            if slices.len() >= IOV_BATCH {
                write_all_slices(writer, &slices)?;
                slices.clear();
            }
        }
    }
    if !slices.is_empty() {
        write_all_slices(writer, &slices)?;
    }
    Ok(())
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
    let mut indices: Vec<usize> = (0..num_lines).collect();

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

    let is_numeric_only = no_keys
        && (gopts.numeric || gopts.general_numeric || gopts.human_numeric)
        && !gopts.dictionary_order
        && !gopts.ignore_case
        && !gopts.ignore_nonprinting;

    let is_single_key = config.keys.len() == 1;

    if is_plain_lex && num_lines > 256 {
        // FAST PATH 1: Prefix-based lexicographic sort
        let reverse = gopts.reverse;
        let mut entries: Vec<(u64, usize)> = offsets
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| (line_prefix(data, s, e), i))
            .collect();

        let prefix_cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
            let ord = match a.0.cmp(&b.0) {
                Ordering::Equal => {
                    let (sa, ea) = offsets[a.1];
                    let (sb, eb) = offsets[b.1];
                    data[sa..ea].cmp(&data[sb..eb])
                }
                ord => ord,
            };
            if reverse { ord.reverse() } else { ord }
        };

        let n = entries.len();
        if config.stable {
            if n > 10_000 {
                entries.par_sort_by(prefix_cmp);
            } else {
                entries.sort_by(prefix_cmp);
            }
        } else if n > 10_000 {
            entries.par_sort_unstable_by(prefix_cmp);
        } else {
            entries.sort_unstable_by(prefix_cmp);
        }

        // Output using writev — zero-copy from mmap
        // Switch to sequential for output phase
        #[cfg(target_os = "linux")]
        if let FileData::Mmap(ref mmap) = buffer {
            let _ = mmap.advise(memmap2::Advice::Sequential);
        }

        let mut slices: Vec<IoSlice<'_>> = Vec::with_capacity(entries.len().min(IOV_BATCH) * 2);
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
                    slices.push(IoSlice::new(line));
                    slices.push(IoSlice::new(terminator));
                    prev = Some(idx);
                }
                if slices.len() >= IOV_BATCH {
                    write_all_slices(&mut writer, &slices)?;
                    slices.clear();
                }
            }
        } else {
            for &(_, idx) in &entries {
                let (s, e) = offsets[idx];
                slices.push(IoSlice::new(&data[s..e]));
                slices.push(IoSlice::new(terminator));
                if slices.len() >= IOV_BATCH {
                    write_all_slices(&mut writer, &slices)?;
                    slices.clear();
                }
            }
        }
        if !slices.is_empty() {
            write_all_slices(&mut writer, &slices)?;
        }
    } else if is_numeric_only {
        // FAST PATH 2: Pre-parsed numeric sort with u64 comparison
        // float_to_sortable_u64 enables branchless u64::cmp instead of f64::partial_cmp
        let mut entries: Vec<(u64, usize)> = offsets
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| {
                (
                    float_to_sortable_u64(parse_value_for_opts(&data[s..e], gopts)),
                    i,
                )
            })
            .collect();
        let reverse = gopts.reverse;
        let stable = config.stable;

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
        if stable {
            if n > 10_000 {
                entries.par_sort_by(cmp);
            } else {
                entries.sort_by(cmp);
            }
        } else if n > 10_000 {
            entries.par_sort_unstable_by(cmp);
        } else {
            entries.sort_unstable_by(cmp);
        }

        for (i, &(_, idx)) in entries.iter().enumerate() {
            indices[i] = idx;
        }

        write_sorted_output(data, &offsets, &indices, config, &mut writer, terminator)?;
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
            let mut entries: Vec<(u64, usize)> = key_offs
                .iter()
                .enumerate()
                .map(|(i, &(s, e))| {
                    let f = if s == e {
                        if opts.general_numeric { f64::NAN } else { 0.0 }
                    } else {
                        parse_value_for_opts(&data[s..e], opts)
                    };
                    (float_to_sortable_u64(f), i)
                })
                .collect();
            let reverse = opts.reverse;
            let stable = config.stable;

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
            if stable {
                if n > 10_000 {
                    entries.par_sort_by(cmp);
                } else {
                    entries.sort_by(cmp);
                }
            } else if n > 10_000 {
                entries.par_sort_unstable_by(cmp);
            } else {
                entries.sort_unstable_by(cmp);
            }

            for (i, &(_, idx)) in entries.iter().enumerate() {
                indices[i] = idx;
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
                // Prefix-based sort: cache 8-byte key prefix as u64
                // Most comparisons resolve at the u64 level (no data[] access)
                let mut entries: Vec<(u64, usize)> = key_offs
                    .iter()
                    .enumerate()
                    .map(|(i, &(s, e))| (if s < e { line_prefix(data, s, e) } else { 0u64 }, i))
                    .collect();

                let cmp = |a: &(u64, usize), b: &(u64, usize)| -> Ordering {
                    let ord = match a.0.cmp(&b.0) {
                        Ordering::Equal => {
                            let (sa, ea) = key_offs[a.1];
                            let (sb, eb) = key_offs[b.1];
                            data[sa..ea].cmp(&data[sb..eb])
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

                let n = entries.len();
                if stable {
                    if n > 10_000 {
                        entries.par_sort_by(cmp);
                    } else {
                        entries.sort_by(cmp);
                    }
                } else if n > 10_000 {
                    entries.par_sort_unstable_by(cmp);
                } else {
                    entries.sort_unstable_by(cmp);
                }

                for (i, &(_, idx)) in entries.iter().enumerate() {
                    indices[i] = idx;
                }
            } else {
                do_sort(&mut indices, stable, |&a, &b| {
                    let (sa, ea) = key_offs[a];
                    let (sb, eb) = key_offs[b];
                    let ka = if sa == ea {
                        &[] as &[u8]
                    } else {
                        &data[sa..ea]
                    };
                    let kb = if sb == eb {
                        &[] as &[u8]
                    } else {
                        &data[sb..eb]
                    };
                    let ord = compare_with_opts(ka, kb, opts, random_seed);
                    if ord == Ordering::Equal && !stable {
                        data[offsets[a].0..offsets[a].1].cmp(&data[offsets[b].0..offsets[b].1])
                    } else {
                        ord
                    }
                });
            }
        }

        write_sorted_output(data, &offsets, &indices, config, &mut writer, terminator)?;
    } else if config.keys.len() > 1 {
        // FAST PATH 4: Multi-key sort with pre-extracted key offsets for ALL keys.
        // Eliminates per-comparison key extraction (O(n log n) calls to extract_key).
        let all_key_offs: Vec<Vec<(usize, usize)>> = config
            .keys
            .iter()
            .map(|key| pre_extract_key_offsets(data, &offsets, key, config.separator))
            .collect();

        let stable = config.stable;
        let random_seed = config.random_seed;
        let keys = &config.keys;
        let global_opts = &config.global_opts;

        do_sort(&mut indices, stable, |&a, &b| {
            for (ki, key) in keys.iter().enumerate() {
                let (sa, ea) = all_key_offs[ki][a];
                let (sb, eb) = all_key_offs[ki][b];
                let ka = if sa == ea {
                    &[] as &[u8]
                } else {
                    &data[sa..ea]
                };
                let kb = if sb == eb {
                    &[] as &[u8]
                } else {
                    &data[sb..eb]
                };

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

                let result = compare_with_opts(ka, kb, opts, random_seed);
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
    } else {
        // GENERAL PATH: Index-based sort with full comparison (fallback)
        do_sort(&mut indices, config.stable, |&a, &b| {
            let (sa, ea) = offsets[a];
            let (sb, eb) = offsets[b];
            compare_lines(&data[sa..ea], &data[sb..eb], config)
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
