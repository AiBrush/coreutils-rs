use std::fs::{self, File};
use std::io::{self, Read};
use std::ops::Deref;
use std::path::Path;

use memmap2::Mmap;

/// Threshold above which we use mmap instead of buffered read.
/// mmap has overhead from page table setup; for small files buffered read wins.
const MMAP_THRESHOLD: u64 = 64 * 1024; // 64KB

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

/// Read a file with zero-copy mmap for large files, buffered read for small ones.
pub fn read_file(path: &Path) -> io::Result<FileData> {
    let metadata = fs::metadata(path)?;

    if metadata.len() >= MMAP_THRESHOLD {
        let file = File::open(path)?;
        // SAFETY: Read-only mapping. File must not be truncated during use.
        let mmap = unsafe { Mmap::map(&file)? };
        // Hint to kernel: we'll read sequentially, prefetch aggressively.
        let _ = mmap.advise(memmap2::Advice::Sequential);
        Ok(FileData::Mmap(mmap))
    } else {
        Ok(FileData::Owned(fs::read(path)?))
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
