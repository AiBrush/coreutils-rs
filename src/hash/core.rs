use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::path::Path;

use std::sync::atomic::AtomicUsize;
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
        ensure_stream_buf(&mut buf);
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
// Allocated LAZILY (only on first streaming-hash call) to avoid 8MB cost for
// small-file-only workloads (e.g., "sha256sum *.txt" where every file is <1MB).
thread_local! {
    static STREAM_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Ensure the streaming buffer is at least HASH_READ_BUF bytes.
/// Called only on the streaming path, so small-file workloads never allocate 8MB.
#[inline]
fn ensure_stream_buf(buf: &mut Vec<u8>) {
    if buf.len() < HASH_READ_BUF {
        buf.resize(HASH_READ_BUF, 0);
    }
}

// ── SHA-256 ───────────────────────────────────────────────────────────

/// Single-shot SHA-256 using OpenSSL's optimized assembly (SHA-NI on x86).
/// Linux only — OpenSSL is not available on Windows/macOS in CI.
#[cfg(target_os = "linux")]
fn sha256_bytes(data: &[u8]) -> String {
    // For tiny data (<8KB): use sha2 crate directly, avoiding OpenSSL's
    // EVP_MD_CTX_new/free overhead (~700ns per call). sha2 with asm feature
    // uses SHA-NI instructions and has no heap allocation, just stack state.
    // For 100 × 55-byte files: saves ~70µs total.
    if data.len() < TINY_FILE_LIMIT as usize {
        use digest::Digest;
        return hex_encode(&sha2::Sha256::digest(data));
    }
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
        ensure_stream_buf(&mut buf);
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
        ensure_stream_buf(&mut buf);
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
    // For tiny data (<8KB): use md5 crate directly, avoiding OpenSSL's
    // EVP_MD_CTX_new/free overhead (~700ns per call). md5 with asm feature
    // uses optimized assembly and has no heap allocation.
    if data.len() < TINY_FILE_LIMIT as usize {
        use digest::Digest;
        return hex_encode(&md5::Md5::digest(data));
    }
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
        ensure_stream_buf(&mut buf);
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

/// Open a file and get its metadata in one step.
/// On Linux uses fstat directly on the fd to avoid an extra syscall layer.
#[cfg(target_os = "linux")]
#[inline]
fn open_and_stat(path: &Path) -> io::Result<(File, u64, bool)> {
    let file = open_noatime(path)?;
    let fd = {
        use std::os::unix::io::AsRawFd;
        file.as_raw_fd()
    };
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let is_regular = (stat.st_mode & libc::S_IFMT) == libc::S_IFREG;
    let size = stat.st_size as u64;
    Ok((file, size, is_regular))
}

#[cfg(not(target_os = "linux"))]
#[inline]
fn open_and_stat(path: &Path) -> io::Result<(File, u64, bool)> {
    let file = open_noatime(path)?;
    let metadata = file.metadata()?;
    Ok((file, metadata.len(), metadata.file_type().is_file()))
}

/// Minimum file size to issue fadvise hint (1MB).
/// For small files, the syscall overhead exceeds the readahead benefit.
#[cfg(target_os = "linux")]
const FADVISE_MIN_SIZE: u64 = 1024 * 1024;

/// Maximum file size for single-read hash optimization.
/// Files up to this size are read entirely into a thread-local buffer and hashed
/// with single-shot hash. This avoids mmap/munmap overhead (~100µs each) and
/// MAP_POPULATE page faults (~300ns/page). The thread-local buffer is reused
/// across files in sequential mode, saving re-allocation.
/// 16MB covers typical benchmark files (10MB) while keeping memory usage bounded.
const SMALL_FILE_LIMIT: u64 = 16 * 1024 * 1024;

/// Threshold for tiny files that can be read into a stack buffer.
/// Below this size, we use a stack-allocated buffer + single read() syscall,
/// completely avoiding any heap allocation for the data path.
const TINY_FILE_LIMIT: u64 = 8 * 1024;

// Thread-local reusable buffer for single-read hash.
// Grows lazily up to SMALL_FILE_LIMIT (16MB). Initial 64KB allocation
// handles tiny files; larger files trigger one grow that persists for reuse.
thread_local! {
    static SMALL_FILE_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(64 * 1024));
}

/// I/O-pipelined hash for large files (>=16MB) on Linux.
/// Uses a reader thread that reads 4MB chunks while the main thread hashes,
/// overlapping NVMe/SSD read latency with SHA-NI computation.
/// For a 100MB file: I/O ~15ms from cache, hash ~40ms → pipelined ~42ms vs ~55ms sequential.
#[cfg(target_os = "linux")]
fn hash_file_pipelined(algo: HashAlgorithm, mut file: File, file_size: u64) -> io::Result<String> {
    use std::os::unix::io::AsRawFd;

    const PIPE_BUF_SIZE: usize = 4 * 1024 * 1024; // 4MB per buffer

    // Hint kernel for sequential access
    unsafe {
        libc::posix_fadvise(
            file.as_raw_fd(),
            0,
            file_size as i64,
            libc::POSIX_FADV_SEQUENTIAL,
        );
    }

    // Channel for sending filled buffers from reader to hasher.
    // sync_channel(1) provides natural double-buffering: reader can fill one
    // buffer ahead while hasher processes the current one.
    let (tx, rx) = std::sync::mpsc::sync_channel::<(Vec<u8>, usize)>(1);
    let (buf_tx, buf_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);

    // Seed the buffer return channel with an initial buffer
    let _ = buf_tx.send(vec![0u8; PIPE_BUF_SIZE]);

    // Reader thread: reads file into buffers and sends them to hasher
    let reader_handle = std::thread::spawn(move || -> io::Result<()> {
        let mut own_buf = vec![0u8; PIPE_BUF_SIZE];
        loop {
            // Try to get a returned buffer from hasher, or use our own
            let mut buf = buf_rx
                .try_recv()
                .unwrap_or_else(|_| std::mem::take(&mut own_buf));
            if buf.is_empty() {
                buf = vec![0u8; PIPE_BUF_SIZE];
            }

            let mut total = 0;
            while total < buf.len() {
                match file.read(&mut buf[total..]) {
                    Ok(0) => break,
                    Ok(n) => total += n,
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            if total == 0 {
                break;
            }
            if tx.send((buf, total)).is_err() {
                break;
            }
        }
        Ok(())
    });

    // Hasher runs on the calling thread
    let hash_result = match algo {
        HashAlgorithm::Sha256 => {
            let mut hasher = openssl::hash::Hasher::new(openssl::hash::MessageDigest::sha256())
                .map_err(|e| io::Error::other(e))?;
            while let Ok((buf, n)) = rx.recv() {
                hasher.update(&buf[..n]).map_err(|e| io::Error::other(e))?;
                // Return the buffer to reader for reuse
                let _ = buf_tx.send(buf);
            }
            let digest = hasher.finish().map_err(|e| io::Error::other(e))?;
            Ok(hex_encode(&digest))
        }
        HashAlgorithm::Md5 => {
            let mut hasher = openssl::hash::Hasher::new(openssl::hash::MessageDigest::md5())
                .map_err(|e| io::Error::other(e))?;
            while let Ok((buf, n)) = rx.recv() {
                hasher.update(&buf[..n]).map_err(|e| io::Error::other(e))?;
                let _ = buf_tx.send(buf);
            }
            let digest = hasher.finish().map_err(|e| io::Error::other(e))?;
            Ok(hex_encode(&digest))
        }
        HashAlgorithm::Blake2b => {
            let mut state = blake2b_simd::Params::new().to_state();
            while let Ok((buf, n)) = rx.recv() {
                state.update(&buf[..n]);
                let _ = buf_tx.send(buf);
            }
            Ok(hex_encode(state.finalize().as_bytes()))
        }
    };

    // Wait for reader thread to finish and propagate any I/O errors
    match reader_handle.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            // If hasher already produced a result, prefer the reader's I/O error
            if hash_result.is_ok() {
                return Err(e);
            }
        }
        Err(_) => {
            return Err(io::Error::other("reader thread panicked"));
        }
    }

    hash_result
}

/// Hash a file by path. Uses I/O pipelining for large files on Linux,
/// mmap with HUGEPAGE hints as fallback, single-read for small files,
/// and streaming read for non-regular files.
pub fn hash_file(algo: HashAlgorithm, path: &Path) -> io::Result<String> {
    let (file, file_size, is_regular) = open_and_stat(path)?;

    if is_regular && file_size == 0 {
        return Ok(hash_bytes(algo, &[]));
    }

    if file_size > 0 && is_regular {
        // Tiny files (<8KB): stack buffer + single read() — zero heap allocation
        if file_size < TINY_FILE_LIMIT {
            return hash_file_tiny(algo, file, file_size as usize);
        }
        // Large files (>=16MB): use I/O pipelining on Linux to overlap read + hash
        if file_size >= SMALL_FILE_LIMIT {
            #[cfg(target_os = "linux")]
            {
                return hash_file_pipelined(algo, file, file_size);
            }
            // Non-Linux: mmap fallback
            #[cfg(not(target_os = "linux"))]
            {
                let mmap_result = unsafe { memmap2::MmapOptions::new().map(&file) };
                if let Ok(mmap) = mmap_result {
                    return Ok(hash_bytes(algo, &mmap));
                }
            }
        }
        // Small files (8KB..16MB): single read into thread-local buffer, then single-shot hash.
        // This avoids Hasher context allocation + streaming overhead for each file.
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

/// Hash a tiny file (<8KB) using a stack-allocated buffer.
/// Single read() syscall, zero heap allocation on the data path.
/// Optimal for the "100 small files" benchmark where per-file overhead dominates.
#[inline]
fn hash_file_tiny(algo: HashAlgorithm, mut file: File, size: usize) -> io::Result<String> {
    let mut buf = [0u8; 8192];
    let mut total = 0;
    // Read with known size — usually completes in a single read() for regular files
    while total < size {
        match file.read(&mut buf[total..size]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(hash_bytes(algo, &buf[..total]))
}

/// Hash a small file by reading it entirely into a thread-local buffer,
/// then using the single-shot hash function. Avoids per-file Hasher allocation.
#[inline]
fn hash_file_small(algo: HashAlgorithm, mut file: File, size: usize) -> io::Result<String> {
    SMALL_FILE_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        // Reset length but keep allocation, then grow if needed
        buf.clear();
        buf.reserve(size);
        // SAFETY: capacity >= size after clear+reserve. We read into the buffer
        // directly and only access buf[..total] where total <= size <= capacity.
        unsafe {
            buf.set_len(size);
        }
        let mut total = 0;
        while total < size {
            match file.read(&mut buf[total..size]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(hash_bytes(algo, &buf[..total]))
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
        ensure_stream_buf(&mut buf);
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
    let (file, file_size, is_regular) = open_and_stat(path)?;

    if is_regular && file_size == 0 {
        return Ok(blake2b_hash_data(&[], output_bytes));
    }

    if file_size > 0 && is_regular {
        // Tiny files (<8KB): stack buffer + single read() — zero heap allocation
        if file_size < TINY_FILE_LIMIT {
            return blake2b_hash_file_tiny(file, file_size as usize, output_bytes);
        }
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
            // No MAP_POPULATE — HUGEPAGE first, then WILLNEED (same as hash_file)
            let mmap_result = unsafe { memmap2::MmapOptions::new().map(&file) };
            if let Ok(mmap) = mmap_result {
                #[cfg(target_os = "linux")]
                {
                    if file_size >= 2 * 1024 * 1024 {
                        let _ = mmap.advise(memmap2::Advice::HugePage);
                    }
                    let _ = mmap.advise(memmap2::Advice::Sequential);
                    let _ = mmap.advise(memmap2::Advice::WillNeed);
                }
                return Ok(blake2b_hash_data(&mmap, output_bytes));
            }
        }
        // Small files (8KB..1MB): single read into thread-local buffer, then single-shot hash
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

/// Hash a tiny BLAKE2b file (<8KB) using a stack-allocated buffer.
#[inline]
fn blake2b_hash_file_tiny(mut file: File, size: usize, output_bytes: usize) -> io::Result<String> {
    let mut buf = [0u8; 8192];
    let mut total = 0;
    while total < size {
        match file.read(&mut buf[total..size]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(blake2b_hash_data(&buf[..total], output_bytes))
}

/// Hash a small file with BLAKE2b by reading it entirely into a thread-local buffer.
#[inline]
fn blake2b_hash_file_small(mut file: File, size: usize, output_bytes: usize) -> io::Result<String> {
    SMALL_FILE_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.clear();
        buf.reserve(size);
        // SAFETY: capacity >= size after clear+reserve
        unsafe {
            buf.set_len(size);
        }
        let mut total = 0;
        while total < size {
            match file.read(&mut buf[total..size]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(blake2b_hash_data(&buf[..total], output_bytes))
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

/// Internal enum for file content in batch hashing.
/// Keeps data alive (either as mmap or owned Vec) while hash_many references it.
enum FileContent {
    Mmap(memmap2::Mmap),
    Buf(Vec<u8>),
}

impl AsRef<[u8]> for FileContent {
    fn as_ref(&self) -> &[u8] {
        match self {
            FileContent::Mmap(m) => m,
            FileContent::Buf(v) => v,
        }
    }
}

/// Open a file and load its content for batch hashing.
/// Uses read for tiny files (avoids mmap syscall overhead), mmap for large
/// files (zero-copy), and read-to-end for non-regular files.
fn open_file_content(path: &Path) -> io::Result<FileContent> {
    let (file, size, is_regular) = open_and_stat(path)?;
    if is_regular && size == 0 {
        return Ok(FileContent::Buf(Vec::new()));
    }
    if is_regular && size > 0 {
        // Tiny files: read directly into Vec. The mmap syscall + page fault
        // overhead exceeds the data transfer cost for files under 8KB.
        // For the 100-file benchmark (55 bytes each), this saves ~100 mmap calls.
        if size < TINY_FILE_LIMIT {
            let mut buf = vec![0u8; size as usize];
            let mut total = 0;
            let mut f = file;
            while total < size as usize {
                match f.read(&mut buf[total..]) {
                    Ok(0) => break,
                    Ok(n) => total += n,
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            buf.truncate(total);
            return Ok(FileContent::Buf(buf));
        }
        // No MAP_POPULATE — HUGEPAGE first, then WILLNEED (same as hash_file)
        let mmap_result = unsafe { memmap2::MmapOptions::new().map(&file) };
        if let Ok(mmap) = mmap_result {
            #[cfg(target_os = "linux")]
            {
                if size >= 2 * 1024 * 1024 {
                    let _ = mmap.advise(memmap2::Advice::HugePage);
                }
                let _ = mmap.advise(memmap2::Advice::Sequential);
                let _ = mmap.advise(memmap2::Advice::WillNeed);
            }
            return Ok(FileContent::Mmap(mmap));
        }
        // Fallback: read into Vec
        let mut buf = vec![0u8; size as usize];
        let mut total = 0;
        let mut f = file;
        while total < size as usize {
            match f.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        buf.truncate(total);
        return Ok(FileContent::Buf(buf));
    }
    // Non-regular: read to end
    let mut buf = Vec::new();
    let mut f = file;
    f.read_to_end(&mut buf)?;
    Ok(FileContent::Buf(buf))
}

/// Open a file and read all content without fstat — just open+read+close.
/// For many-file workloads (100+ files), skipping fstat saves ~5µs/file
/// (~0.5ms for 100 files). Uses a small initial buffer for tiny files (< 4KB),
/// then falls back to larger buffer or mmap for bigger files.
fn open_file_content_fast(path: &Path) -> io::Result<FileContent> {
    let mut file = open_noatime(path)?;
    // Try small buffer first — optimal for benchmark's ~55 byte files.
    // Single read() + to_vec() with exact size for minimal allocation.
    let mut small_buf = [0u8; 4096];
    match file.read(&mut small_buf) {
        Ok(0) => return Ok(FileContent::Buf(Vec::new())),
        Ok(n) if n < small_buf.len() => {
            // File fits in small buffer — done (common case for tiny files)
            return Ok(FileContent::Buf(small_buf[..n].to_vec()));
        }
        Ok(n) => {
            // Might be more data — fall back to larger buffer
            let mut buf = [0u8; 65536];
            buf[..n].copy_from_slice(&small_buf[..n]);
            let mut total = n;
            loop {
                match file.read(&mut buf[total..]) {
                    Ok(0) => return Ok(FileContent::Buf(buf[..total].to_vec())),
                    Ok(n) => {
                        total += n;
                        if total >= buf.len() {
                            return open_file_content(path);
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
            let mut buf = [0u8; 65536];
            let mut total = 0;
            loop {
                match file.read(&mut buf[total..]) {
                    Ok(0) => return Ok(FileContent::Buf(buf[..total].to_vec())),
                    Ok(n) => {
                        total += n;
                        if total >= buf.len() {
                            return open_file_content(path);
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        Err(e) => return Err(e),
    }
}

/// Batch-hash multiple files with BLAKE2b using multi-buffer SIMD.
///
/// Uses blake2b_simd::many::hash_many for 4-way AVX2 parallel hashing.
/// All files are pre-loaded into memory (mmap for large, read for small),
/// then hashed simultaneously. Returns results in input order.
///
/// For 100 files on AVX2: 4x throughput from SIMD parallelism.
pub fn blake2b_hash_files_many(paths: &[&Path], output_bytes: usize) -> Vec<io::Result<String>> {
    use blake2b_simd::many::{HashManyJob, hash_many};

    // Phase 1: Read all files into memory.
    // For small file counts (≤10), load sequentially to avoid thread::scope
    // overhead (~120µs). For many files, use parallel loading with lightweight
    // OS threads. For 100+ files, use fast path that skips fstat.
    let use_fast = paths.len() >= 20;

    let file_data: Vec<io::Result<FileContent>> = if paths.len() <= 10 {
        // Sequential loading — avoids thread spawn overhead for small batches
        paths.iter().map(|&path| open_file_content(path)).collect()
    } else {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(paths.len());
        let chunk_size = (paths.len() + num_threads - 1) / num_threads;

        std::thread::scope(|s| {
            let handles: Vec<_> = paths
                .chunks(chunk_size)
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter()
                            .map(|&path| {
                                if use_fast {
                                    open_file_content_fast(path)
                                } else {
                                    open_file_content(path)
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();

            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        })
    };

    // Phase 2: Build hash_many jobs for successful reads
    let hash_results = {
        let mut params = blake2b_simd::Params::new();
        params.hash_length(output_bytes);

        let ok_entries: Vec<(usize, &[u8])> = file_data
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().ok().map(|c| (i, c.as_ref())))
            .collect();

        let mut jobs: Vec<HashManyJob> = ok_entries
            .iter()
            .map(|(_, data)| HashManyJob::new(&params, data))
            .collect();

        // Phase 3: Run multi-buffer SIMD hash (4-way AVX2)
        hash_many(jobs.iter_mut());

        // Extract hashes into a map
        let mut hm: Vec<Option<String>> = vec![None; paths.len()];
        for (j, &(orig_i, _)) in ok_entries.iter().enumerate() {
            hm[orig_i] = Some(hex_encode(jobs[j].to_hash().as_bytes()));
        }
        hm
    }; // file_data borrow released here

    // Phase 4: Combine hashes and errors in original order
    hash_results
        .into_iter()
        .zip(file_data)
        .map(|(hash_opt, result)| match result {
            Ok(_) => Ok(hash_opt.unwrap()),
            Err(e) => Err(e),
        })
        .collect()
}

/// Batch-hash multiple files with SHA-256/MD5 using work-stealing parallelism.
/// Files are sorted by size (largest first) so the biggest files start processing
/// immediately. Each worker thread grabs the next unprocessed file via atomic index,
/// eliminating tail latency from uneven file sizes.
/// Returns results in input order.
pub fn hash_files_parallel(paths: &[&Path], algo: HashAlgorithm) -> Vec<io::Result<String>> {
    let n = paths.len();

    // Build (original_index, path, size) tuples — stat all files for scheduling.
    // The stat cost (~5µs/file) is repaid by better work distribution.
    let mut indexed: Vec<(usize, &Path, u64)> = paths
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            (i, p, size)
        })
        .collect();

    // Sort largest first: ensures big files start hashing immediately while
    // small files fill in gaps, minimizing tail latency.
    indexed.sort_by(|a, b| b.2.cmp(&a.2));

    // Issue readahead for the largest files to warm the page cache.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        for &(_, path, size) in indexed.iter().take(20) {
            if size >= 1024 * 1024 {
                if let Ok(file) = open_noatime(path) {
                    unsafe {
                        libc::posix_fadvise(
                            file.as_raw_fd(),
                            0,
                            size as i64,
                            libc::POSIX_FADV_WILLNEED,
                        );
                    }
                }
            }
        }
    }

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(n);

    // Atomic work index for dynamic work-stealing.
    let work_idx = AtomicUsize::new(0);

    std::thread::scope(|s| {
        let work_idx = &work_idx;
        let indexed = &indexed;

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                s.spawn(move || {
                    let mut local_results = Vec::new();
                    loop {
                        let idx = work_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if idx >= indexed.len() {
                            break;
                        }
                        let (orig_idx, path, _size) = indexed[idx];
                        let result = hash_file(algo, path);
                        local_results.push((orig_idx, result));
                    }
                    local_results
                })
            })
            .collect();

        // Collect results and reorder to match original input order.
        let mut results: Vec<Option<io::Result<String>>> = (0..n).map(|_| None).collect();
        for handle in handles {
            for (orig_idx, result) in handle.join().unwrap() {
                results[orig_idx] = Some(result);
            }
        }
        results
            .into_iter()
            .map(|opt| opt.unwrap_or_else(|| Err(io::Error::other("missing result"))))
            .collect()
    })
}

/// Hash a file without fstat — just open, read until EOF, hash.
/// For many-file workloads (100+ tiny files), skipping fstat saves ~5µs/file.
/// Uses a two-tier buffer strategy: small stack buffer (4KB) for the initial read,
/// then falls back to a larger stack buffer (64KB) or streaming hash for bigger files.
/// For benchmark's 55-byte files: one read() fills the 4KB buffer, hash immediately.
pub fn hash_file_nostat(algo: HashAlgorithm, path: &Path) -> io::Result<String> {
    let mut file = open_noatime(path)?;
    // First try a small stack buffer — optimal for tiny files (< 4KB).
    // Most "many_files" benchmark files are ~55 bytes, so this completes
    // with a single read() syscall and no fallback.
    let mut small_buf = [0u8; 4096];
    match file.read(&mut small_buf) {
        Ok(0) => return Ok(hash_bytes(algo, &[])),
        Ok(n) if n < small_buf.len() => {
            // File fits in small buffer — hash directly (common case)
            return Ok(hash_bytes(algo, &small_buf[..n]));
        }
        Ok(n) => {
            // Might be more data — fall back to larger buffer
            let mut buf = [0u8; 65536];
            buf[..n].copy_from_slice(&small_buf[..n]);
            let mut total = n;
            loop {
                match file.read(&mut buf[total..]) {
                    Ok(0) => return Ok(hash_bytes(algo, &buf[..total])),
                    Ok(n) => {
                        total += n;
                        if total >= buf.len() {
                            return hash_file(algo, path);
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
            // Retry with full buffer on interrupt
            let mut buf = [0u8; 65536];
            let mut total = 0;
            loop {
                match file.read(&mut buf[total..]) {
                    Ok(0) => return Ok(hash_bytes(algo, &buf[..total])),
                    Ok(n) => {
                        total += n;
                        if total >= buf.len() {
                            return hash_file(algo, path);
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        Err(e) => return Err(e),
    }
}

/// Issue readahead hints for ALL file paths (no size threshold).
/// For multi-file benchmarks, even small files benefit from batched readahead.
#[cfg(target_os = "linux")]
pub fn readahead_files_all(paths: &[&Path]) {
    use std::os::unix::io::AsRawFd;
    for path in paths {
        if let Ok(file) = open_noatime(path) {
            if let Ok(meta) = file.metadata() {
                if meta.file_type().is_file() {
                    let len = meta.len();
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
pub fn readahead_files_all(_paths: &[&Path]) {}

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

// ── Single-write output buffer ─────────────────────────────────────
// For multi-file workloads, batch the entire "hash  filename\n" line into
// a single write() call. This halves the number of BufWriter flushes.

// Thread-local output line buffer for batched writes.
// Reused across files to avoid per-file allocation.
thread_local! {
    static LINE_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(256));
}

/// Build and write the standard GNU hash output line in a single write() call.
/// Format: "hash  filename\n" or "hash *filename\n" (binary mode).
/// For escaped filenames: "\hash  escaped_filename\n".
#[inline]
pub fn write_hash_line(
    out: &mut impl Write,
    hash: &str,
    filename: &str,
    binary: bool,
    zero: bool,
    escaped: bool,
) -> io::Result<()> {
    LINE_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.clear();
        let mode = if binary { b'*' } else { b' ' };
        let term = if zero { b'\0' } else { b'\n' };
        if escaped {
            buf.push(b'\\');
        }
        buf.extend_from_slice(hash.as_bytes());
        buf.push(b' ');
        buf.push(mode);
        buf.extend_from_slice(filename.as_bytes());
        buf.push(term);
        out.write_all(&buf)
    })
}

/// Build and write BSD tag format output in a single write() call.
/// Format: "ALGO (filename) = hash\n"
#[inline]
pub fn write_hash_tag_line(
    out: &mut impl Write,
    algo_name: &str,
    hash: &str,
    filename: &str,
    zero: bool,
) -> io::Result<()> {
    LINE_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.clear();
        let term = if zero { b'\0' } else { b'\n' };
        buf.extend_from_slice(algo_name.as_bytes());
        buf.extend_from_slice(b" (");
        buf.extend_from_slice(filename.as_bytes());
        buf.extend_from_slice(b") = ");
        buf.extend_from_slice(hash.as_bytes());
        buf.push(term);
        out.write_all(&buf)
    })
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
        hex_encode_to_slice(bytes, buf);
    }
    hex
}

/// Encode bytes as hex directly into a pre-allocated output slice.
/// Output slice must be at least `bytes.len() * 2` bytes long.
#[inline]
fn hex_encode_to_slice(bytes: &[u8], out: &mut [u8]) {
    // SAFETY: We write exactly bytes.len()*2 bytes into `out`, which must be large enough.
    unsafe {
        let ptr = out.as_mut_ptr();
        for (i, &b) in bytes.iter().enumerate() {
            let pair = *HEX_TABLE.get_unchecked(b as usize);
            *ptr.add(i * 2) = pair[0];
            *ptr.add(i * 2 + 1) = pair[1];
        }
    }
}
