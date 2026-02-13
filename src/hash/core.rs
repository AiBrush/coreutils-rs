use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::path::Path;

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_os = "linux"))]
use digest::Digest;
#[cfg(not(target_os = "linux"))]
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

/// Single-shot hash using the Digest trait (non-Linux fallback).
#[cfg(not(target_os = "linux"))]
fn hash_digest<D: Digest>(data: &[u8]) -> String {
    hex_encode(&D::digest(data))
}

/// Streaming hash using thread-local buffer (non-Linux fallback).
#[cfg(not(target_os = "linux"))]
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
/// 8MB: amortizes syscall overhead while still fitting in L3 cache on modern CPUs.
/// Larger buffer means fewer read() calls per file (e.g., 13 reads for 100MB vs 25).
const HASH_READ_BUF: usize = 8 * 1024 * 1024;

// Thread-local reusable buffer for streaming hash I/O.
// Allocated once per thread, reused across all hash_reader calls.
thread_local! {
    static STREAM_BUF: RefCell<Vec<u8>> = RefCell::new(vec![0u8; HASH_READ_BUF]);
}

// ── SHA-256 ───────────────────────────────────────────────────────────

/// Single-shot SHA-256 using OpenSSL's optimized assembly (SHA-NI on x86).
/// Linux only — OpenSSL is not available on Windows/macOS in CI.
#[cfg(target_os = "linux")]
fn sha256_bytes(data: &[u8]) -> String {
    let digest = openssl::hash::hash(openssl::hash::MessageDigest::sha256(), data)
        .expect("SHA256 hash failed");
    hex_encode(&digest)
}

/// Single-shot SHA-256 using ring's BoringSSL assembly (Windows and other non-Apple).
#[cfg(all(not(target_vendor = "apple"), not(target_os = "linux")))]
fn sha256_bytes(data: &[u8]) -> String {
    hex_encode(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

/// Single-shot SHA-256 using sha2 crate (macOS fallback — ring doesn't compile on Apple Silicon).
#[cfg(target_vendor = "apple")]
fn sha256_bytes(data: &[u8]) -> String {
    hash_digest::<sha2::Sha256>(data)
}

/// Streaming SHA-256 using OpenSSL's optimized assembly.
/// Linux only — OpenSSL is not available on Windows/macOS in CI.
#[cfg(target_os = "linux")]
fn sha256_reader(mut reader: impl Read) -> io::Result<String> {
    STREAM_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let mut hasher = openssl::hash::Hasher::new(openssl::hash::MessageDigest::sha256())
            .map_err(|e| io::Error::other(e))?;
        loop {
            let n = read_full(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]).map_err(|e| io::Error::other(e))?;
        }
        let digest = hasher.finish().map_err(|e| io::Error::other(e))?;
        Ok(hex_encode(&digest))
    })
}

/// Streaming SHA-256 using ring's BoringSSL assembly (Windows and other non-Apple).
#[cfg(all(not(target_vendor = "apple"), not(target_os = "linux")))]
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
        HashAlgorithm::Md5 => md5_bytes(data),
        HashAlgorithm::Blake2b => {
            let hash = blake2b_simd::blake2b(data);
            hex_encode(hash.as_bytes())
        }
    }
}

// ── MD5 ─────────────────────────────────────────────────────────────

/// Single-shot MD5 using OpenSSL's optimized assembly (Linux).
#[cfg(target_os = "linux")]
fn md5_bytes(data: &[u8]) -> String {
    let digest =
        openssl::hash::hash(openssl::hash::MessageDigest::md5(), data).expect("MD5 hash failed");
    hex_encode(&digest)
}

/// Single-shot MD5 using md-5 crate (non-Linux fallback).
#[cfg(not(target_os = "linux"))]
fn md5_bytes(data: &[u8]) -> String {
    hash_digest::<Md5>(data)
}

/// Compute hash of data from a reader, returning hex string.
pub fn hash_reader<R: Read>(algo: HashAlgorithm, reader: R) -> io::Result<String> {
    match algo {
        HashAlgorithm::Sha256 => sha256_reader(reader),
        HashAlgorithm::Md5 => md5_reader(reader),
        HashAlgorithm::Blake2b => blake2b_hash_reader(reader, 64),
    }
}

/// Streaming MD5 using OpenSSL's optimized assembly (Linux).
#[cfg(target_os = "linux")]
fn md5_reader(mut reader: impl Read) -> io::Result<String> {
    STREAM_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let mut hasher = openssl::hash::Hasher::new(openssl::hash::MessageDigest::md5())
            .map_err(|e| io::Error::other(e))?;
        loop {
            let n = read_full(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]).map_err(|e| io::Error::other(e))?;
        }
        let digest = hasher.finish().map_err(|e| io::Error::other(e))?;
        Ok(hex_encode(&digest))
    })
}

/// Streaming MD5 using md-5 crate (non-Linux fallback).
#[cfg(not(target_os = "linux"))]
fn md5_reader(reader: impl Read) -> io::Result<String> {
    hash_reader_impl::<Md5>(reader)
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
        match std::fs::OpenOptions::new()
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

/// Minimum file size to issue fadvise hint (1MB).
/// For small files, the syscall overhead exceeds the readahead benefit.
#[cfg(target_os = "linux")]
const FADVISE_MIN_SIZE: u64 = 1024 * 1024;

/// Maximum file size for single-read hash optimization.
/// Files up to this size are read entirely into a thread-local buffer and hashed
/// with single-shot hash (avoids Hasher allocation + streaming overhead).
const SMALL_FILE_LIMIT: u64 = 1024 * 1024;

// Thread-local reusable buffer for small-file single-read hash.
// Avoids repeated allocation for many small files (e.g., 100 files of 1KB each).
thread_local! {
    static SMALL_FILE_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(64 * 1024));
}

/// Hash a file by path. Uses mmap for large files (zero-copy, no read() syscalls),
/// single-read + single-shot hash for small files, and streaming read as fallback.
pub fn hash_file(algo: HashAlgorithm, path: &Path) -> io::Result<String> {
    let file = open_noatime(path)?;
    let metadata = file.metadata()?;
    let file_size = metadata.len();

    if metadata.file_type().is_file() && file_size == 0 {
        return Ok(hash_bytes(algo, &[]));
    }

    if file_size > 0 && metadata.file_type().is_file() {
        // mmap for large files — zero-copy, eliminates multiple read() syscalls
        if file_size >= SMALL_FILE_LIMIT {
            #[cfg(target_os = "linux")]
            if file_size >= FADVISE_MIN_SIZE {
                use std::os::unix::io::AsRawFd;
                unsafe {
                    libc::posix_fadvise(
                        file.as_raw_fd(),
                        0,
                        file_size as i64,
                        libc::POSIX_FADV_SEQUENTIAL,
                    );
                }
            }
            if let Ok(mmap) = unsafe { memmap2::MmapOptions::new().populate().map(&file) } {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
                    if file_size >= 2 * 1024 * 1024 {
                        let _ = mmap.advise(memmap2::Advice::HugePage);
                    }
                }
                return Ok(hash_bytes(algo, &mmap));
            }
        }
        // Small files: single read into thread-local buffer, then single-shot hash.
        // This avoids Hasher context allocation + streaming overhead for each file.
        // Especially beneficial for many small files (100+ files of a few KB each).
        if file_size < SMALL_FILE_LIMIT {
            return hash_file_small(algo, file, file_size as usize);
        }
    }

    // Non-regular files or fallback: stream
    #[cfg(target_os = "linux")]
    if file_size >= FADVISE_MIN_SIZE {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
        }
    }
    hash_reader(algo, file)
}

/// Hash a small file by reading it entirely into a thread-local buffer,
/// then using the single-shot hash function. Avoids per-file Hasher allocation.
#[inline]
fn hash_file_small(algo: HashAlgorithm, mut file: File, size: usize) -> io::Result<String> {
    SMALL_FILE_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.clear();
        // Pre-allocate to known file size to avoid reallocation
        let cap = buf.capacity();
        if cap < size {
            buf.reserve(size - cap);
        }
        file.read_to_end(&mut buf)?;
        Ok(hash_bytes(algo, &buf))
    })
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

/// Check if parallel hashing is worthwhile for the given file paths.
/// Always parallelize with 2+ files — rayon's thread pool is lazily initialized
/// once and reused, so per-file work-stealing overhead is negligible (~1µs).
/// Removing the stat()-based size check eliminates N extra syscalls for N files.
pub fn should_use_parallel(paths: &[&Path]) -> bool {
    paths.len() >= 2
}

/// Issue readahead hints for a list of file paths to warm the page cache.
/// Uses POSIX_FADV_WILLNEED which is non-blocking and batches efficiently.
/// Only issues hints for files >= 1MB; small files are read fast enough
/// that the fadvise syscall overhead isn't worth it.
#[cfg(target_os = "linux")]
pub fn readahead_files(paths: &[&Path]) {
    use std::os::unix::io::AsRawFd;
    for path in paths {
        if let Ok(file) = open_noatime(path) {
            if let Ok(meta) = file.metadata() {
                let len = meta.len();
                if meta.file_type().is_file() && len >= FADVISE_MIN_SIZE {
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
/// Uses thread-local buffer for cache-friendly streaming.
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

/// Hash a file with BLAKE2b variable output length.
/// Uses mmap for large files (zero-copy), single-read for small files,
/// and streaming read as fallback.
pub fn blake2b_hash_file(path: &Path, output_bytes: usize) -> io::Result<String> {
    let file = open_noatime(path)?;
    let metadata = file.metadata()?;
    let file_size = metadata.len();

    if metadata.file_type().is_file() && file_size == 0 {
        return Ok(blake2b_hash_data(&[], output_bytes));
    }

    if file_size > 0 && metadata.file_type().is_file() {
        // mmap for large files — zero-copy, eliminates multiple read() syscalls
        if file_size >= SMALL_FILE_LIMIT {
            #[cfg(target_os = "linux")]
            if file_size >= FADVISE_MIN_SIZE {
                use std::os::unix::io::AsRawFd;
                unsafe {
                    libc::posix_fadvise(
                        file.as_raw_fd(),
                        0,
                        file_size as i64,
                        libc::POSIX_FADV_SEQUENTIAL,
                    );
                }
            }
            if let Ok(mmap) = unsafe { memmap2::MmapOptions::new().populate().map(&file) } {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
                    if file_size >= 2 * 1024 * 1024 {
                        let _ = mmap.advise(memmap2::Advice::HugePage);
                    }
                }
                return Ok(blake2b_hash_data(&mmap, output_bytes));
            }
        }
        // Small files: single read into thread-local buffer, then single-shot hash
        if file_size < SMALL_FILE_LIMIT {
            return blake2b_hash_file_small(file, file_size as usize, output_bytes);
        }
    }

    // Non-regular files or fallback: stream
    #[cfg(target_os = "linux")]
    if file_size >= FADVISE_MIN_SIZE {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
        }
    }
    blake2b_hash_reader(file, output_bytes)
}

/// Hash a small file with BLAKE2b by reading it entirely into a thread-local buffer.
#[inline]
fn blake2b_hash_file_small(mut file: File, size: usize, output_bytes: usize) -> io::Result<String> {
    SMALL_FILE_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.clear();
        let cap = buf.capacity();
        if cap < size {
            buf.reserve(size - cap);
        }
        file.read_to_end(&mut buf)?;
        Ok(blake2b_hash_data(&buf, output_bytes))
    })
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
/// Uses raw byte writes to avoid std::fmt overhead.
pub fn print_hash(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    binary: bool,
) -> io::Result<()> {
    let mode = if binary { b'*' } else { b' ' };
    out.write_all(hash.as_bytes())?;
    out.write_all(&[b' ', mode])?;
    out.write_all(filename.as_bytes())?;
    out.write_all(b"\n")
}

/// Print hash in GNU format with NUL terminator instead of newline.
pub fn print_hash_zero(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    binary: bool,
) -> io::Result<()> {
    let mode = if binary { b'*' } else { b' ' };
    out.write_all(hash.as_bytes())?;
    out.write_all(&[b' ', mode])?;
    out.write_all(filename.as_bytes())?;
    out.write_all(b"\0")
}

/// Print hash result in BSD tag format: "ALGO (filename) = hash\n"
pub fn print_hash_tag(
    out: &mut impl Write,
    algo: HashAlgorithm,
    hash: &str,
    filename: &str,
) -> io::Result<()> {
    out.write_all(algo.name().as_bytes())?;
    out.write_all(b" (")?;
    out.write_all(filename.as_bytes())?;
    out.write_all(b") = ")?;
    out.write_all(hash.as_bytes())?;
    out.write_all(b"\n")
}

/// Print hash in BSD tag format with NUL terminator.
pub fn print_hash_tag_zero(
    out: &mut impl Write,
    algo: HashAlgorithm,
    hash: &str,
    filename: &str,
) -> io::Result<()> {
    out.write_all(algo.name().as_bytes())?;
    out.write_all(b" (")?;
    out.write_all(filename.as_bytes())?;
    out.write_all(b") = ")?;
    out.write_all(hash.as_bytes())?;
    out.write_all(b"\0")
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
        out.write_all(b"BLAKE2b (")?;
    } else {
        // Use write! for the rare non-512 path (negligible overhead per file)
        write!(out, "BLAKE2b-{} (", bits)?;
    }
    out.write_all(filename.as_bytes())?;
    out.write_all(b") = ")?;
    out.write_all(hash.as_bytes())?;
    out.write_all(b"\n")
}

/// Print hash in BSD tag format with BLAKE2b length info and NUL terminator.
pub fn print_hash_tag_b2sum_zero(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    bits: usize,
) -> io::Result<()> {
    if bits == 512 {
        out.write_all(b"BLAKE2b (")?;
    } else {
        write!(out, "BLAKE2b-{} (", bits)?;
    }
    out.write_all(filename.as_bytes())?;
    out.write_all(b") = ")?;
    out.write_all(hash.as_bytes())?;
    out.write_all(b"\0")
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
/// Fast path: regular file reads usually return the full buffer on the first call.
#[inline]
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    // Fast path: first read() usually fills the entire buffer for regular files
    let n = reader.read(buf)?;
    if n == buf.len() || n == 0 {
        return Ok(n);
    }
    // Slow path: partial read — retry to fill buffer (pipes, slow devices)
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
