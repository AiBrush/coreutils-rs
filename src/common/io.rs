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
        // Small files: read from already-open fd (avoids double open + page table overhead)
        if len < MMAP_THRESHOLD {
            let mut buf = Vec::with_capacity(len as usize);
            let mut reader = file;
            reader.read_to_end(&mut buf)?;
            return Ok(FileData::Owned(buf));
        }

        // SAFETY: Read-only mapping. MADV_SEQUENTIAL lets the kernel
        // prefetch ahead of our sequential access pattern.
        match unsafe { MmapOptions::new().map(&file) } {
            Ok(mmap) => {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
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
pub fn read_stdin() -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    io::stdin().lock().read_to_end(&mut buf)?;
    Ok(buf)
}
