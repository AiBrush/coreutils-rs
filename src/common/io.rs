use std::fs::{self, File};
use std::io::{self, Read};
use std::ops::Deref;
use std::path::Path;

#[cfg(target_os = "linux")]
use std::os::unix::io::FromRawFd;
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

/// Read a file entirely into a mutable Vec.
/// Uses exact-size allocation from fstat + single read() for efficiency.
/// Preferred over mmap when the caller needs mutable access (e.g., in-place decode).
pub fn read_file_vec(path: &Path) -> io::Result<Vec<u8>> {
    let file = open_noatime(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len() as usize;
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; len];
    let n = read_full(&mut &file, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// Read a file always using mmap, with MADV_WILLNEED (no MADV_SEQUENTIAL).
/// Used by tac which scans forward then outputs in reverse, and benefits
/// from zero-copy vmsplice output from mmap pages.
/// Skips the MMAP_THRESHOLD — even small files benefit from mmap since:
///   - No memcpy from page cache to userspace (zero-copy)
///   - vmsplice can reference mmap pages directly in the pipe
///   - mmap setup cost for small files (~25 pages) is comparable to read()
pub fn read_file_mmap(path: &Path) -> io::Result<FileData> {
    let file = open_noatime(path)?;
    let metadata = file.metadata()?;
    let len = metadata.len();

    if len > 0 && metadata.file_type().is_file() {
        match unsafe { MmapOptions::new().populate().map(&file) } {
            Ok(mmap) => {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::WillNeed);
                    if len >= 2 * 1024 * 1024 {
                        let _ = mmap.advise(memmap2::Advice::HugePage);
                    }
                }
                return Ok(FileData::Mmap(mmap));
            }
            Err(_) => {
                // mmap failed — fall back to read
                let mut buf = vec![0u8; len as usize];
                let n = read_full(&mut &file, &mut buf)?;
                buf.truncate(n);
                return Ok(FileData::Owned(buf));
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

/// Splice piped stdin into an mmap via memfd for zero-copy access on Linux.
///
/// When stdin is a pipe, this avoids the kernel→userspace copy that read() requires:
///   1. memfd_create() → anonymous in-memory file descriptor
///   2. splice() loops data from pipe fd 0 → memfd (kernel-to-kernel, no userspace copy)
///   3. mmap() the memfd → zero-copy userspace access to the pipe data
///
/// Returns None if stdin is not a pipe or if the pipe is empty.
/// Callers should fall back to read_stdin() on None.
#[cfg(target_os = "linux")]
pub fn splice_stdin_to_mmap() -> Option<Mmap> {
    // Check if stdin is a pipe
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(0, &mut stat) } != 0 {
        return None;
    }
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFIFO {
        return None;
    }

    // Create memfd — anonymous in-memory file, no filesystem overhead
    let name = b"fio\0";
    let memfd = unsafe { libc::memfd_create(name.as_ptr() as *const libc::c_char, 0) };
    if memfd < 0 {
        return None;
    }

    // splice() all data from pipe (fd 0) → memfd (kernel-to-kernel copy)
    const SPLICE_CHUNK: usize = 16 * 1024 * 1024;
    let mut total: usize = 0;
    loop {
        let n = unsafe {
            libc::splice(
                0,
                std::ptr::null_mut(),
                memfd,
                std::ptr::null_mut(),
                SPLICE_CHUNK,
                0,
            )
        };
        if n > 0 {
            total += n as usize;
        } else if n == 0 {
            break; // EOF
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            unsafe { libc::close(memfd) };
            return None;
        }
    }

    if total == 0 {
        unsafe { libc::close(memfd) };
        return None;
    }

    // mmap the memfd for zero-copy access
    let file = unsafe { File::from_raw_fd(memfd) };
    let mmap = unsafe { MmapOptions::new().populate().map(&file) }.ok()?;

    #[cfg(target_os = "linux")]
    {
        let _ = mmap.advise(memmap2::Advice::Sequential);
        let _ = mmap.advise(memmap2::Advice::WillNeed);
        if total >= 2 * 1024 * 1024 {
            let _ = mmap.advise(memmap2::Advice::HugePage);
        }
    }

    Some(mmap)
}

/// Like splice_stdin_to_mmap() but returns a writable MmapMut.
/// Useful for TR translate which can modify data in-place instead of
/// allocating a separate output buffer — saving 10MB allocation + memcpy.
/// Since we own the memfd, MAP_SHARED writes are safe (no other users).
#[cfg(target_os = "linux")]
pub fn splice_stdin_to_mmap_mut() -> Option<memmap2::MmapMut> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(0, &mut stat) } != 0 {
        return None;
    }
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFIFO {
        return None;
    }

    let name = b"fio\0";
    let memfd = unsafe { libc::memfd_create(name.as_ptr() as *const libc::c_char, 0) };
    if memfd < 0 {
        return None;
    }

    const SPLICE_CHUNK: usize = 16 * 1024 * 1024;
    let mut total: usize = 0;
    loop {
        let n = unsafe {
            libc::splice(
                0,
                std::ptr::null_mut(),
                memfd,
                std::ptr::null_mut(),
                SPLICE_CHUNK,
                0,
            )
        };
        if n > 0 {
            total += n as usize;
        } else if n == 0 {
            break;
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            unsafe { libc::close(memfd) };
            return None;
        }
    }

    if total == 0 {
        unsafe { libc::close(memfd) };
        return None;
    }

    let file = unsafe { File::from_raw_fd(memfd) };
    let mmap = unsafe { MmapOptions::new().populate().map_mut(&file) }.ok()?;

    Some(mmap)
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
