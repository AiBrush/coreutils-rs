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

/// Read a file with zero-copy mmap. Uses populate() for eager page table setup
/// and MADV_HUGEPAGE for TLB efficiency on large files.
pub fn read_file(path: &Path) -> io::Result<FileData> {
    let metadata = fs::metadata(path)?;
    let len = metadata.len();

    if len > 0 && metadata.file_type().is_file() {
        let file = File::open(path)?;
        // SAFETY: Read-only mapping. File must not be truncated during use.
        match unsafe { MmapOptions::new().populate().map(&file) } {
            Ok(mmap) => {
                #[cfg(target_os = "linux")]
                {
                    let _ = mmap.advise(memmap2::Advice::Sequential);
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
            Err(_) => Ok(FileData::Owned(fs::read(path)?)),
        }
    } else if len > 0 {
        // Non-regular file (special files)
        Ok(FileData::Owned(fs::read(path)?))
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
