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
/// Uses a direct read() loop into a pre-allocated buffer instead of read_to_end(),
/// which avoids Vec's grow-and-probe pattern (extra read() calls and memcpy).
/// On Linux, enlarges the pipe buffer to 4MB first for fewer read() syscalls.
pub fn read_stdin() -> io::Result<Vec<u8>> {
    const PREALLOC: usize = 16 * 1024 * 1024;
    const READ_BUF: usize = 4 * 1024 * 1024;

    // Enlarge pipe buffer on Linux for fewer read() syscalls
    #[cfg(target_os = "linux")]
    unsafe {
        libc::fcntl(0, libc::F_SETPIPE_SZ, READ_BUF as i32);
    }

    let mut stdin = io::stdin().lock();
    let mut buf: Vec<u8> = Vec::with_capacity(PREALLOC);

    // Direct read loop: read in large chunks, grow Vec only when needed.
    // This avoids the overhead of read_to_end()'s small initial reads and
    // zero-filling of the buffer on each grow.
    loop {
        if buf.len() + READ_BUF > buf.capacity() {
            buf.reserve(READ_BUF);
        }
        let spare_cap = buf.capacity() - buf.len();
        let read_size = spare_cap.min(READ_BUF);

        // SAFETY: we read into the uninitialized spare capacity and extend
        // set_len only by the number of bytes actually read.
        let start = buf.len();
        unsafe { buf.set_len(start + read_size) };
        match stdin.read(&mut buf[start..start + read_size]) {
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
