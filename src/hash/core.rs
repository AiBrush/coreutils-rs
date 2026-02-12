use std::cell::RefCell;
use std::fs::{self, File};
use std::io::{self, BufRead, Read, Write};
use std::path::Path;

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

use digest::Digest;
use md5::Md5;

/// Supported hash algorithms.
#[derive(Debug, Clone, Copy)]
pub enum HashAlgorithm {
    Sha256,
    Md5,
    Blake2b,
}

impl HashAlgorithm {
    pub fn name(self) -> &'static str {
        match self {
            HashAlgorithm::Sha256 => "SHA256",
            HashAlgorithm::Md5 => "MD5",
            HashAlgorithm::Blake2b => "BLAKE2b",
        }
    }
}

// ── Generic hash helpers ────────────────────────────────────────────

fn hash_digest<D: Digest>(data: &[u8]) -> String {
    hex_encode(&D::digest(data))
}

/// Streaming hash using thread-local 1MB buffer for optimal L2 cache behavior.
/// 1MB fits in L2 cache on most CPUs, keeping data hot during hash update.
/// Uses read_full to ensure each update() gets a full buffer, minimizing
/// per-chunk hasher overhead and maximizing SIMD-friendly aligned updates.
fn hash_reader_impl<D: Digest>(mut reader: impl Read) -> io::Result<String> {
    STREAM_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let mut hasher = D::new();
        loop {
            let n = read_full(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex_encode(&hasher.finalize()))
    })
}

// ── Public hashing API ──────────────────────────────────────────────

/// Buffer size for streaming hash I/O.
/// 2MB gives fewer syscalls while still fitting in L3 cache.
/// With fadvise(SEQUENTIAL) the kernel prefetches ahead, so the next
/// 2MB is already in page cache by the time we finish hashing the current chunk.
/// ring's SHA-NI processes at ~3-4 GB/s, so 2MB takes ~0.5ms — fast enough
/// that I/O latency dominates and larger buffers just waste memory.
const HASH_READ_BUF: usize = 2 * 1024 * 1024;

/// Threshold below which we read the entire file + single-shot hash.
/// Files up to 8MB fit comfortably in memory and benefit from contiguous
/// data access — the CPU hardware prefetcher works best on large contiguous
/// buffers. Also avoids per-chunk hasher.update() call overhead.
const SINGLE_SHOT_THRESHOLD: u64 = 8 * 1024 * 1024;

// Thread-local reusable buffer for streaming hash I/O.
// Allocated once per thread, reused across all hash_reader calls.
thread_local! {
    static STREAM_BUF: RefCell<Vec<u8>> = RefCell::new(vec![0u8; HASH_READ_BUF]);
}

// ── SHA-256: ring on non-Apple, sha2 fallback on Apple ───────────────

/// Single-shot SHA-256 using ring's BoringSSL assembly (Linux/Windows).
#[cfg(not(target_vendor = "apple"))]
fn sha256_bytes(data: &[u8]) -> String {
    hex_encode(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

/// Single-shot SHA-256 using sha2 crate (macOS fallback).
#[cfg(target_vendor = "apple")]
fn sha256_bytes(data: &[u8]) -> String {
    hash_digest::<sha2::Sha256>(data)
}

/// Streaming SHA-256 using ring's BoringSSL assembly (Linux/Windows).
#[cfg(not(target_vendor = "apple"))]
fn sha256_reader(mut reader: impl Read) -> io::Result<String> {
    STREAM_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        loop {
            let n = read_full(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            ctx.update(&buf[..n]);
        }
        Ok(hex_encode(ctx.finish().as_ref()))
    })
}

/// Streaming SHA-256 using sha2 crate (macOS fallback).
#[cfg(target_vendor = "apple")]
fn sha256_reader(reader: impl Read) -> io::Result<String> {
    hash_reader_impl::<sha2::Sha256>(reader)
}

/// Compute hash of a byte slice directly (zero-copy fast path).
pub fn hash_bytes(algo: HashAlgorithm, data: &[u8]) -> String {
    match algo {
        HashAlgorithm::Sha256 => sha256_bytes(data),
        HashAlgorithm::Md5 => hash_digest::<Md5>(data),
        HashAlgorithm::Blake2b => {
            let hash = blake2b_simd::blake2b(data);
            hex_encode(hash.as_bytes())
        }
    }
}

/// Compute hash of data from a reader, returning hex string.
pub fn hash_reader<R: Read>(algo: HashAlgorithm, reader: R) -> io::Result<String> {
    match algo {
        HashAlgorithm::Sha256 => sha256_reader(reader),
        HashAlgorithm::Md5 => hash_reader_impl::<Md5>(reader),
        HashAlgorithm::Blake2b => blake2b_hash_reader(reader, 64),
    }
}

/// Track whether O_NOATIME is supported to avoid repeated failed open() attempts.
/// After the first EPERM, we never try O_NOATIME again (saves one syscall per file).
#[cfg(target_os = "linux")]
static NOATIME_SUPPORTED: AtomicBool = AtomicBool::new(true);

/// Open a file with O_NOATIME on Linux to avoid atime update overhead.
/// Caches whether O_NOATIME works to avoid double-open on every file.
#[cfg(target_os = "linux")]
fn open_noatime(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    if NOATIME_SUPPORTED.load(Ordering::Relaxed) {
        match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOATIME)
            .open(path)
        {
            Ok(f) => return Ok(f),
            Err(ref e) if e.raw_os_error() == Some(libc::EPERM) => {
                // O_NOATIME requires file ownership or CAP_FOWNER — disable globally
                NOATIME_SUPPORTED.store(false, Ordering::Relaxed);
            }
            Err(e) => return Err(e), // Real error, propagate
        }
    }
    File::open(path)
}

#[cfg(not(target_os = "linux"))]
fn open_noatime(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Hint the kernel for sequential read access. Non-blocking.
#[cfg(target_os = "linux")]
#[inline]
fn fadvise_sequential(file: &File, len: u64) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, len as i64, libc::POSIX_FADV_SEQUENTIAL);
    }
}

#[cfg(not(target_os = "linux"))]
#[inline]
fn fadvise_sequential(_file: &File, _len: u64) {}

/// Hash a file by path. Single open + fstat to minimize syscalls.
/// Uses read() for small files, streaming read+hash for large files.
/// Replaced mmap with read()+fadvise for better cache behavior:
/// read() keeps data hot in L2/L3 cache, while mmap suffers page table
/// and TLB overhead for sequential single-pass workloads.
pub fn hash_file(algo: HashAlgorithm, path: &Path) -> io::Result<String> {
    // Single open — reuse fd for fstat + read (saves separate stat + open)
    let file = open_noatime(path)?;
    let metadata = file.metadata()?; // fstat on existing fd, cheaper than stat(path)
    let len = metadata.len();
    let is_regular = metadata.file_type().is_file();

    if is_regular && len == 0 {
        return Ok(hash_bytes(algo, &[]));
    }

    if is_regular && len > 0 {
        // Files up to 8MB: read entirely + single-shot hash.
        // Contiguous data enables optimal CPU prefetching and avoids
        // per-chunk hasher.update() overhead.
        // Use read_full into exact-size buffer instead of read_to_end
        // to avoid the grow-and-probe loop (saves 1-2 extra read() syscalls).
        if len <= SINGLE_SHOT_THRESHOLD {
            fadvise_sequential(&file, len);
            let mut buf = vec![0u8; len as usize];
            let n = read_full(&mut &file, &mut buf)?;
            return Ok(hash_bytes(algo, &buf[..n]));
        }

        // Large files (>8MB): streaming read with kernel readahead hint.
        // fadvise(SEQUENTIAL) enables aggressive readahead (2x default).
        fadvise_sequential(&file, len);
        return hash_reader(algo, file);
    }

    // Fallback: streaming read (special files, pipes, etc.) — fd already open
    hash_reader(algo, file)
}

/// Hash stdin. Uses fadvise for file redirects, streaming for pipes.
pub fn hash_stdin(algo: HashAlgorithm) -> io::Result<String> {
    let stdin = io::stdin();
    // Hint kernel for sequential access if stdin is a regular file (redirect)
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let fd = stdin.as_raw_fd();
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } == 0
            && (stat.st_mode & libc::S_IFMT) == libc::S_IFREG
            && stat.st_size > 0
        {
            unsafe {
                libc::posix_fadvise(fd, 0, stat.st_size, libc::POSIX_FADV_SEQUENTIAL);
            }
        }
    }
    // Streaming hash — works for both pipe and file-redirect stdin
    hash_reader(algo, stdin.lock())
}

/// Estimate total file size for parallel/sequential decision.
/// Uses a quick heuristic: samples first file and extrapolates.
/// Returns 0 if estimation fails.
pub fn estimate_total_size(paths: &[&Path]) -> u64 {
    if paths.is_empty() {
        return 0;
    }
    // Sample first file to estimate
    if let Ok(meta) = fs::metadata(paths[0]) {
        meta.len().saturating_mul(paths.len() as u64)
    } else {
        0
    }
}

/// Check if parallel hashing is worthwhile for the given file paths.
/// Only uses rayon when files are individually large enough for the hash
/// computation to dominate over rayon overhead (thread pool init + work stealing).
/// For many small files (e.g., 100 × 100KB), sequential is much faster.
pub fn should_use_parallel(paths: &[&Path]) -> bool {
    if paths.len() < 2 {
        return false;
    }
    let total = estimate_total_size(paths);
    let avg = total / paths.len() as u64;
    // Only parallelize when average file size >= 1MB.
    // Below this, rayon overhead exceeds the benefit of parallel hashing.
    avg >= 1024 * 1024
}

/// Issue readahead hints for a list of file paths to warm the page cache.
/// Uses POSIX_FADV_WILLNEED which is non-blocking and batches efficiently.
#[cfg(target_os = "linux")]
pub fn readahead_files(paths: &[&Path]) {
    use std::os::unix::io::AsRawFd;
    for path in paths {
        if let Ok(file) = open_noatime(path) {
            if let Ok(meta) = file.metadata() {
                let len = meta.len();
                if meta.file_type().is_file() && len > 0 {
                    unsafe {
                        libc::posix_fadvise(
                            file.as_raw_fd(),
                            0,
                            len as i64,
                            libc::POSIX_FADV_WILLNEED,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn readahead_files(_paths: &[&Path]) {
    // No-op on non-Linux
}

// --- BLAKE2b variable-length functions (using blake2b_simd) ---

/// Hash raw data with BLAKE2b variable output length.
/// `output_bytes` is the output size in bytes (e.g., 32 for 256-bit).
pub fn blake2b_hash_data(data: &[u8], output_bytes: usize) -> String {
    let hash = blake2b_simd::Params::new()
        .hash_length(output_bytes)
        .hash(data);
    hex_encode(hash.as_bytes())
}

/// Hash a reader with BLAKE2b variable output length.
/// Uses thread-local 1MB buffer for cache-friendly streaming.
pub fn blake2b_hash_reader<R: Read>(mut reader: R, output_bytes: usize) -> io::Result<String> {
    STREAM_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let mut state = blake2b_simd::Params::new()
            .hash_length(output_bytes)
            .to_state();
        loop {
            let n = read_full(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            state.update(&buf[..n]);
        }
        Ok(hex_encode(state.finalize().as_bytes()))
    })
}

/// Hash a file with BLAKE2b variable output length. Single open + fstat.
/// Uses read() for small files, streaming read+hash for large.
pub fn blake2b_hash_file(path: &Path, output_bytes: usize) -> io::Result<String> {
    // Single open — reuse fd for fstat + read
    let file = open_noatime(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len();
    let is_regular = metadata.file_type().is_file();

    if is_regular && len == 0 {
        return Ok(blake2b_hash_data(&[], output_bytes));
    }

    if is_regular && len > 0 {
        // Files up to 8MB: read entirely + single-shot hash.
        if len <= SINGLE_SHOT_THRESHOLD {
            fadvise_sequential(&file, len);
            let mut buf = vec![0u8; len as usize];
            let n = read_full(&mut &file, &mut buf)?;
            return Ok(blake2b_hash_data(&buf[..n], output_bytes));
        }

        // Large files (>8MB): streaming read with kernel readahead hint
        fadvise_sequential(&file, len);
        return blake2b_hash_reader(file, output_bytes);
    }

    // Fallback: streaming read — fd already open
    blake2b_hash_reader(file, output_bytes)
}

/// Hash stdin with BLAKE2b variable output length.
/// Tries fadvise if stdin is a regular file (shell redirect), then streams.
pub fn blake2b_hash_stdin(output_bytes: usize) -> io::Result<String> {
    let stdin = io::stdin();
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let fd = stdin.as_raw_fd();
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } == 0
            && (stat.st_mode & libc::S_IFMT) == libc::S_IFREG
            && stat.st_size > 0
        {
            unsafe {
                libc::posix_fadvise(fd, 0, stat.st_size, libc::POSIX_FADV_SEQUENTIAL);
            }
        }
    }
    blake2b_hash_reader(stdin.lock(), output_bytes)
}

/// Print hash result in GNU format: "hash  filename\n"
pub fn print_hash(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    binary: bool,
) -> io::Result<()> {
    let mode_char = if binary { '*' } else { ' ' };
    writeln!(out, "{} {}{}", hash, mode_char, filename)
}

/// Print hash in GNU format with NUL terminator instead of newline.
pub fn print_hash_zero(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    binary: bool,
) -> io::Result<()> {
    let mode_char = if binary { '*' } else { ' ' };
    write!(out, "{} {}{}\0", hash, mode_char, filename)
}

/// Print hash result in BSD tag format: "ALGO (filename) = hash\n"
pub fn print_hash_tag(
    out: &mut impl Write,
    algo: HashAlgorithm,
    hash: &str,
    filename: &str,
) -> io::Result<()> {
    writeln!(out, "{} ({}) = {}", algo.name(), filename, hash)
}

/// Print hash in BSD tag format with NUL terminator.
pub fn print_hash_tag_zero(
    out: &mut impl Write,
    algo: HashAlgorithm,
    hash: &str,
    filename: &str,
) -> io::Result<()> {
    write!(out, "{} ({}) = {}\0", algo.name(), filename, hash)
}

/// Print hash in BSD tag format with BLAKE2b length info:
/// "BLAKE2b (filename) = hash" for 512-bit, or
/// "BLAKE2b-256 (filename) = hash" for other lengths.
pub fn print_hash_tag_b2sum(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    bits: usize,
) -> io::Result<()> {
    if bits == 512 {
        writeln!(out, "BLAKE2b ({}) = {}", filename, hash)
    } else {
        writeln!(out, "BLAKE2b-{} ({}) = {}", bits, filename, hash)
    }
}

/// Print hash in BSD tag format with BLAKE2b length info and NUL terminator.
pub fn print_hash_tag_b2sum_zero(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    bits: usize,
) -> io::Result<()> {
    if bits == 512 {
        write!(out, "BLAKE2b ({}) = {}\0", filename, hash)
    } else {
        write!(out, "BLAKE2b-{} ({}) = {}\0", bits, filename, hash)
    }
}

/// Options for check mode.
pub struct CheckOptions {
    pub quiet: bool,
    pub status_only: bool,
    pub strict: bool,
    pub warn: bool,
    pub ignore_missing: bool,
    /// Prefix for per-line format warnings, e.g., "fmd5sum: checksums.txt".
    /// When non-empty, warnings use GNU format: "{prefix}: {line}: message".
    /// When empty, uses generic format: "line {line}: message".
    pub warn_prefix: String,
}

/// Result of check mode verification.
pub struct CheckResult {
    pub ok: usize,
    pub mismatches: usize,
    pub format_errors: usize,
    pub read_errors: usize,
    /// Number of files skipped because they were missing and --ignore-missing was set.
    pub ignored_missing: usize,
}

/// Verify checksums from a check file.
/// Each line should be "hash  filename" or "hash *filename" or "ALGO (filename) = hash".
pub fn check_file<R: BufRead>(
    algo: HashAlgorithm,
    reader: R,
    opts: &CheckOptions,
    out: &mut impl Write,
    err_out: &mut impl Write,
) -> io::Result<CheckResult> {
    let quiet = opts.quiet;
    let status_only = opts.status_only;
    let warn = opts.warn;
    let ignore_missing = opts.ignore_missing;
    let mut ok_count = 0;
    let mut mismatch_count = 0;
    let mut format_errors = 0;
    let mut read_errors = 0;
    let mut ignored_missing_count = 0;
    let mut line_num = 0;

    for line_result in reader.lines() {
        line_num += 1;
        let line = line_result?;
        let line = line.trim_end();

        if line.is_empty() {
            continue;
        }

        // Parse "hash  filename" or "hash *filename" or "ALGO (file) = hash"
        let (expected_hash, filename) = match parse_check_line(line) {
            Some(v) => v,
            None => {
                format_errors += 1;
                if warn {
                    out.flush()?;
                    if opts.warn_prefix.is_empty() {
                        writeln!(
                            err_out,
                            "line {}: improperly formatted {} checksum line",
                            line_num,
                            algo.name()
                        )?;
                    } else {
                        writeln!(
                            err_out,
                            "{}: {}: improperly formatted {} checksum line",
                            opts.warn_prefix,
                            line_num,
                            algo.name()
                        )?;
                    }
                }
                continue;
            }
        };

        // Compute actual hash
        let actual = match hash_file(algo, Path::new(filename)) {
            Ok(h) => h,
            Err(e) => {
                if ignore_missing && e.kind() == io::ErrorKind::NotFound {
                    ignored_missing_count += 1;
                    continue;
                }
                read_errors += 1;
                if !status_only {
                    out.flush()?;
                    writeln!(err_out, "{}: {}", filename, e)?;
                    writeln!(out, "{}: FAILED open or read", filename)?;
                }
                continue;
            }
        };

        if actual.eq_ignore_ascii_case(expected_hash) {
            ok_count += 1;
            if !quiet && !status_only {
                writeln!(out, "{}: OK", filename)?;
            }
        } else {
            mismatch_count += 1;
            if !status_only {
                writeln!(out, "{}: FAILED", filename)?;
            }
        }
    }

    Ok(CheckResult {
        ok: ok_count,
        mismatches: mismatch_count,
        format_errors,
        read_errors,
        ignored_missing: ignored_missing_count,
    })
}

/// Parse a checksum line in any supported format.
pub fn parse_check_line(line: &str) -> Option<(&str, &str)> {
    // Try BSD tag format: "ALGO (filename) = hash"
    let rest = line
        .strip_prefix("MD5 (")
        .or_else(|| line.strip_prefix("SHA256 ("))
        .or_else(|| line.strip_prefix("BLAKE2b ("))
        .or_else(|| {
            // Handle BLAKE2b-NNN (filename) = hash
            if line.starts_with("BLAKE2b-") {
                let after = &line["BLAKE2b-".len()..];
                if let Some(sp) = after.find(" (") {
                    if after[..sp].bytes().all(|b| b.is_ascii_digit()) {
                        return Some(&after[sp + 2..]);
                    }
                }
            }
            None
        });
    if let Some(rest) = rest {
        if let Some(paren_idx) = rest.find(") = ") {
            let filename = &rest[..paren_idx];
            let hash = &rest[paren_idx + 4..];
            return Some((hash, filename));
        }
    }

    // Handle backslash-escaped lines (leading '\')
    let line = line.strip_prefix('\\').unwrap_or(line);

    // Standard format: "hash  filename"
    if let Some(idx) = line.find("  ") {
        let hash = &line[..idx];
        let rest = &line[idx + 2..];
        return Some((hash, rest));
    }
    // Binary mode: "hash *filename"
    if let Some(idx) = line.find(" *") {
        let hash = &line[..idx];
        let rest = &line[idx + 2..];
        return Some((hash, rest));
    }
    None
}

/// Parse a BSD-style tag line: "ALGO (filename) = hash"
/// Returns (expected_hash, filename, optional_bits).
/// `bits` is the hash length parsed from the algo name (e.g., BLAKE2b-256 -> Some(256)).
pub fn parse_check_line_tag(line: &str) -> Option<(&str, &str, Option<usize>)> {
    let paren_start = line.find(" (")?;
    let algo_part = &line[..paren_start];
    let rest = &line[paren_start + 2..];
    let paren_end = rest.find(") = ")?;
    let filename = &rest[..paren_end];
    let hash = &rest[paren_end + 4..];

    // Parse optional bit length from algo name (e.g., "BLAKE2b-256" -> Some(256))
    let bits = if let Some(dash_pos) = algo_part.rfind('-') {
        algo_part[dash_pos + 1..].parse::<usize>().ok()
    } else {
        None
    };

    Some((hash, filename, bits))
}

/// Read as many bytes as possible into buf, retrying on partial reads.
/// Ensures each hash update gets a full buffer (fewer update calls = less overhead).
#[inline]
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
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

/// Compile-time generated 2-byte hex pair lookup table.
/// Each byte maps directly to its 2-char hex representation — single lookup per byte.
const fn generate_hex_table() -> [[u8; 2]; 256] {
    let hex = b"0123456789abcdef";
    let mut table = [[0u8; 2]; 256];
    let mut i = 0;
    while i < 256 {
        table[i] = [hex[i >> 4], hex[i & 0xf]];
        i += 1;
    }
    table
}

const HEX_TABLE: [[u8; 2]; 256] = generate_hex_table();

/// Fast hex encoding using 2-byte pair lookup table — one lookup per input byte.
/// Uses String directly instead of Vec<u8> to avoid the from_utf8 conversion overhead.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let len = bytes.len() * 2;
    let mut hex = String::with_capacity(len);
    // SAFETY: We write exactly `len` valid ASCII hex bytes into the String's buffer.
    unsafe {
        let buf = hex.as_mut_vec();
        buf.set_len(len);
        let ptr = buf.as_mut_ptr();
        for (i, &b) in bytes.iter().enumerate() {
            let pair = *HEX_TABLE.get_unchecked(b as usize);
            *ptr.add(i * 2) = pair[0];
            *ptr.add(i * 2 + 1) = pair[1];
        }
    }
    hex
}
