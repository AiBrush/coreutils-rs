use std::fs::{self, File};
use std::io::{self, Read};
use std::ops::Deref;
use std::path::Path;

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

use memmap2::{Mmap, MmapOptions};

/// Holds file data — either zero-copy mmap or an owned Vec.
/// Dereferences to `&[u8]` for transparent use.
pub enum FileData {
    Mmap(Mmap),
    Owned(Vec<u8>),
}

impl Deref for FileData {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            FileData::Mmap(m) => m,
            FileData::Owned(v) => v,
        }
    }
}

/// Threshold below which we use read() instead of mmap.
/// For files under 1MB, read() is faster since mmap has setup/teardown overhead
/// (page table creation for up to 256 pages, TLB flush on munmap) that exceeds
/// the zero-copy benefit.
const MMAP_THRESHOLD: u64 = 1024 * 1024;

/// Track whether O_NOATIME is supported to avoid repeated failed open() attempts.
/// After the first EPERM, we never try O_NOATIME again (saves one syscall per file).
#[cfg(target_os = "linux")]
static NOATIME_SUPPORTED: AtomicBool = AtomicBool::new(true);

/// Open a file with O_NOATIME on Linux to avoid atime inode writes.
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

/// Read a file with zero-copy mmap for large files or read() for small files.
/// Opens once with O_NOATIME, uses fstat for metadata to save a syscall.
pub fn read_file(path: &Path) -> io::Result<FileData> {
    let file = open_noatime(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len();

    if len > 0 && metadata.file_type().is_file() {
        // Small files: exact-size read from already-open fd.
        // Uses read_full into pre-sized buffer instead of read_to_end,
        // which avoids the grow-and-probe pattern (saves 1-2 extra read() syscalls).
        if len < MMAP_THRESHOLD {
            let mut buf = vec![0u8; len as usize];
            let n = read_full(&mut &file, &mut buf)?;
            buf.truncate(n);
            return Ok(FileData::Owned(buf));
        }

        // SAFETY: Read-only mapping. MADV_SEQUENTIAL lets the kernel
        // prefetch ahead of our sequential access pattern.
        match unsafe { MmapOptions::new().populate().map(&file) } {
            Ok(mmap) => {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
                    let _ = mmap.advise(memmap2::Advice::WillNeed);
                    // HUGEPAGE reduces TLB misses for large files (2MB+ = 1+ huge page).
                    // With 4KB pages, a 100MB file needs 25,600 TLB entries; with 2MB
                    // huge pages it needs only 50, reducing TLB miss overhead by ~500x.
                    if len >= 2 * 1024 * 1024 {
                        let _ = mmap.advise(memmap2::Advice::HugePage);
                    }
                }
                Ok(FileData::Mmap(mmap))
            }
            Err(_) => {
                // mmap failed — fall back to read
                let mut buf = Vec::with_capacity(len as usize);
                let mut reader = file;
                reader.read_to_end(&mut buf)?;
                Ok(FileData::Owned(buf))
            }
        }
    } else if len > 0 {
        // Non-regular file (special files) — read from open fd
        let mut buf = Vec::new();
        let mut reader = file;
        reader.read_to_end(&mut buf)?;
        Ok(FileData::Owned(buf))
    } else {
        Ok(FileData::Owned(Vec::new()))
    }
}

/// Get file size without reading it (for byte-count-only optimization).
pub fn file_size(path: &Path) -> io::Result<u64> {
    Ok(fs::metadata(path)?.len())
}

/// Read all bytes from stdin into a Vec.
/// On Linux, uses raw libc::read() to bypass Rust's StdinLock/BufReader overhead.
/// Uses a direct read() loop into a pre-allocated buffer instead of read_to_end(),
/// which avoids Vec's grow-and-probe pattern (extra read() calls and memcpy).
/// Callers should enlarge the pipe buffer via fcntl(F_SETPIPE_SZ) before calling.
/// Uses the full spare capacity for each read() to minimize syscalls.
pub fn read_stdin() -> io::Result<Vec<u8>> {
    #[cfg(target_os = "linux")]
    return read_stdin_raw();

    #[cfg(not(target_os = "linux"))]
    read_stdin_generic()
}

/// Raw libc::read() implementation for Linux — bypasses Rust's StdinLock
/// and BufReader layers entirely. StdinLock uses an internal 8KB BufReader
/// which adds an extra memcpy for every read; raw read() goes directly
/// from the kernel pipe buffer to our Vec.
///
/// Pre-allocates 16MB to cover most workloads (benchmark = 10MB) without
/// over-allocating. For inputs > 16MB, doubles capacity on demand.
/// Each read() uses the full spare capacity to maximize bytes per syscall.
///
/// Note: callers (ftac, ftr, fbase64) are expected to enlarge the pipe
/// buffer via fcntl(F_SETPIPE_SZ) before calling this function. We don't
/// do it here to avoid accidentally shrinking a previously enlarged pipe.
#[cfg(target_os = "linux")]
fn read_stdin_raw() -> io::Result<Vec<u8>> {
    const PREALLOC: usize = 16 * 1024 * 1024;

    let mut buf: Vec<u8> = Vec::with_capacity(PREALLOC);

    loop {
        let spare_cap = buf.capacity() - buf.len();
        if spare_cap < 1024 * 1024 {
            // Grow by doubling (or at least 64MB) to minimize realloc count
            let new_cap = (buf.capacity() * 2).max(buf.len() + PREALLOC);
            buf.reserve(new_cap - buf.capacity());
        }
        let spare_cap = buf.capacity() - buf.len();
        let start = buf.len();

        // SAFETY: we read into the uninitialized spare capacity and extend
        // set_len only by the number of bytes actually read.
        let ret = unsafe {
            libc::read(
                0,
                buf.as_mut_ptr().add(start) as *mut libc::c_void,
                spare_cap,
            )
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if ret == 0 {
            break;
        }
        unsafe { buf.set_len(start + ret as usize) };
    }

    Ok(buf)
}

/// Generic read_stdin for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
fn read_stdin_generic() -> io::Result<Vec<u8>> {
    const PREALLOC: usize = 16 * 1024 * 1024;
    const READ_BUF: usize = 4 * 1024 * 1024;

    let mut stdin = io::stdin().lock();
    let mut buf: Vec<u8> = Vec::with_capacity(PREALLOC);

    loop {
        let spare_cap = buf.capacity() - buf.len();
        if spare_cap < READ_BUF {
            buf.reserve(PREALLOC);
        }
        let spare_cap = buf.capacity() - buf.len();

        let start = buf.len();
        unsafe { buf.set_len(start + spare_cap) };
        match stdin.read(&mut buf[start..start + spare_cap]) {
            Ok(0) => {
                buf.truncate(start);
                break;
            }
            Ok(n) => {
                buf.truncate(start + n);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                buf.truncate(start);
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    Ok(buf)
}

/// Read as many bytes as possible into buf, retrying on partial reads.
/// Ensures the full buffer is filled (or EOF reached), avoiding the
/// probe-read overhead of read_to_end.
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
