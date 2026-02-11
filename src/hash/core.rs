use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;

use md5::Md5;
use memmap2::MmapOptions;
use sha2::{Digest, Sha256};

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

/// Threshold above which we use mmap instead of buffered read.
const MMAP_THRESHOLD: u64 = 64 * 1024;

// ── Generic hash helpers ────────────────────────────────────────────

fn hash_digest<D: Digest>(data: &[u8]) -> String {
    hex_encode(&D::digest(data))
}

fn hash_reader_impl<D: Digest>(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = D::new();
    let mut buf = vec![0u8; 8 * 1024 * 1024]; // 8MB buffer for better throughput
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

// ── Public hashing API ──────────────────────────────────────────────

/// Compute hash of a byte slice directly (zero-copy fast path).
pub fn hash_bytes(algo: HashAlgorithm, data: &[u8]) -> String {
    match algo {
        HashAlgorithm::Sha256 => hash_digest::<Sha256>(data),
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
        HashAlgorithm::Sha256 => hash_reader_impl::<Sha256>(reader),
        HashAlgorithm::Md5 => hash_reader_impl::<Md5>(reader),
        HashAlgorithm::Blake2b => blake2b_hash_reader(reader, 64),
    }
}

/// Hash a file by path using mmap for large files. Returns the hex digest.
pub fn hash_file(algo: HashAlgorithm, path: &Path) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    let len = metadata.len();
    let is_regular = metadata.file_type().is_file();

    // mmap fast path for regular files >= 64KB
    if is_regular && len >= MMAP_THRESHOLD {
        let file = File::open(path)?;
        match unsafe { MmapOptions::new().map(&file) } {
            Ok(mmap) => {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
                }
                return Ok(hash_bytes(algo, &mmap));
            }
            Err(_) => {
                let reader = BufReader::with_capacity(8 * 1024 * 1024, file);
                return hash_reader(algo, reader);
            }
        }
    }

    // Small regular files: read into memory directly
    if is_regular && len > 0 {
        let data = fs::read(path)?;
        return Ok(hash_bytes(algo, &data));
    }

    // Fallback: buffered read (special files, empty files, etc.)
    let file = File::open(path)?;
    let reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    hash_reader(algo, reader)
}

/// Hash stdin. Returns the hex digest.
pub fn hash_stdin(algo: HashAlgorithm) -> io::Result<String> {
    hash_reader(algo, io::stdin().lock())
}

/// Issue readahead hints for a list of file paths to warm the page cache.
/// This should be called before parallel hashing to reduce I/O stalls.
#[cfg(target_os = "linux")]
pub fn readahead_files(paths: &[&Path]) {
    use std::os::unix::io::AsRawFd;
    for path in paths {
        if let Ok(file) = File::open(path) {
            if let Ok(meta) = file.metadata() {
                let len = meta.len();
                if meta.file_type().is_file() && len > 0 {
                    unsafe {
                        libc::readahead(file.as_raw_fd(), 0, len as usize);
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
pub fn blake2b_hash_reader<R: Read>(mut reader: R, output_bytes: usize) -> io::Result<String> {
    let mut state = blake2b_simd::Params::new()
        .hash_length(output_bytes)
        .to_state();
    let mut buf = vec![0u8; 8 * 1024 * 1024]; // 8MB buffer
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        state.update(&buf[..n]);
    }
    Ok(hex_encode(state.finalize().as_bytes()))
}

/// Hash a file with BLAKE2b variable output length using mmap.
pub fn blake2b_hash_file(path: &Path, output_bytes: usize) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    let len = metadata.len();
    let is_regular = metadata.file_type().is_file();

    if is_regular && len >= MMAP_THRESHOLD {
        let file = File::open(path)?;
        match unsafe { MmapOptions::new().map(&file) } {
            Ok(mmap) => {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
                }
                return Ok(blake2b_hash_data(&mmap, output_bytes));
            }
            Err(_) => {
                let reader = BufReader::with_capacity(8 * 1024 * 1024, file);
                return blake2b_hash_reader(reader, output_bytes);
            }
        }
    }

    if is_regular && len > 0 {
        let data = fs::read(path)?;
        return Ok(blake2b_hash_data(&data, output_bytes));
    }

    let file = File::open(path)?;
    let reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    blake2b_hash_reader(reader, output_bytes)
}

/// Hash stdin with BLAKE2b variable output length.
pub fn blake2b_hash_stdin(output_bytes: usize) -> io::Result<String> {
    blake2b_hash_reader(io::stdin().lock(), output_bytes)
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

/// Fast hex encoding using lookup table.
const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut hex = vec![0u8; bytes.len() * 2];
    for (i, &b) in bytes.iter().enumerate() {
        hex[i * 2] = HEX_CHARS[(b >> 4) as usize];
        hex[i * 2 + 1] = HEX_CHARS[(b & 0x0f) as usize];
    }
    // SAFETY: All bytes are ASCII hex digits [0-9a-f]
    unsafe { String::from_utf8_unchecked(hex) }
}
