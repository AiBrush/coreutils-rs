use std::fs::{self, File};
use std::io::{self, Read};
use std::ops::Deref;
use std::path::Path;

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
/// For small files, read() is faster since mmap has setup/teardown overhead
/// (page table creation, TLB flush on munmap) that exceeds the zero-copy benefit.
const MMAP_THRESHOLD: u64 = 256 * 1024;

/// Open a file with O_NOATIME on Linux to avoid atime inode writes.
/// Falls back to normal open if O_NOATIME fails (e.g., not file owner).
#[cfg(target_os = "linux")]
fn open_noatime(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NOATIME = 0o1000000 on Linux
    match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOATIME)
        .open(path)
    {
        Ok(f) => Ok(f),
        Err(_) => File::open(path),
    }
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

        // SAFETY: Read-only mapping. File must not be truncated during use.
        // Don't use populate() — it blocks until all pages are loaded.
        // Instead, MADV_SEQUENTIAL triggers async readahead which overlaps with processing.
        match unsafe { MmapOptions::new().map(&file) } {
            Ok(mmap) => {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
                    // WILLNEED triggers immediate async readahead
                    unsafe {
                        libc::madvise(
                            mmap.as_ptr() as *mut libc::c_void,
                            mmap.len(),
                            libc::MADV_WILLNEED,
                        );
                    }
                    if len >= 2 * 1024 * 1024 {
                        unsafe {
                            libc::madvise(
                                mmap.as_ptr() as *mut libc::c_void,
                                mmap.len(),
                                libc::MADV_HUGEPAGE,
                            );
                        }
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
pub fn read_stdin() -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    io::stdin().lock().read_to_end(&mut buf)?;
    Ok(buf)
}
