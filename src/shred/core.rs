use std::fs;
use std::io::{self, Seek, Write};
use std::path::Path;

/// How to remove files after shredding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveMode {
    /// Just unlink the file.
    Unlink,
    /// Wipe the filename by renaming before unlinking.
    Wipe,
    /// Wipe and sync before unlinking.
    WipeSync,
}

/// Configuration for the shred operation.
#[derive(Debug, Clone)]
pub struct ShredConfig {
    pub iterations: usize,
    pub zero_pass: bool,
    pub remove: Option<RemoveMode>,
    pub force: bool,
    pub verbose: bool,
    pub exact: bool,
    pub size: Option<u64>,
}

impl Default for ShredConfig {
    fn default() -> Self {
        Self {
            iterations: 3,
            zero_pass: false,
            remove: None,
            force: false,
            verbose: false,
            exact: false,
            size: None,
        }
    }
}

/// Fill a buffer with random bytes from /dev/urandom.
pub fn fill_random(buf: &mut [u8]) {
    use std::fs::File;
    use std::io::Read;
    if let Ok(mut f) = File::open("/dev/urandom") {
        let _ = f.read_exact(buf);
    } else {
        // Fallback: simple PRNG seeded from the clock
        let mut seed: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x12345678);
        for byte in buf.iter_mut() {
            // xorshift64
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *byte = seed as u8;
        }
    }
}

/// Shred a single file according to the given configuration.
pub fn shred_file(path: &Path, config: &ShredConfig) -> io::Result<()> {
    // If force, make writable if needed
    if config.force {
        if let Ok(meta) = fs::metadata(path) {
            let mut perms = meta.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = perms.mode();
                if mode & 0o200 == 0 {
                    perms.set_mode(mode | 0o200);
                    let _ = fs::set_permissions(path, perms);
                }
            }
            #[cfg(not(unix))]
            {
                #[allow(clippy::permissions_set_readonly_false)]
                if perms.readonly() {
                    perms.set_readonly(false);
                    let _ = fs::set_permissions(path, perms);
                }
            }
        }
    }

    let file_size = if let Some(s) = config.size {
        s
    } else {
        fs::metadata(path)?.len()
    };

    let write_size = if config.exact {
        file_size
    } else {
        // Round up to 512-byte block boundary
        let block = 512u64;
        (file_size + block - 1) / block * block
    };

    let mut file = fs::OpenOptions::new().write(true).open(path)?;
    let buf_size = 65536usize; // 64KB buffer
    let mut rng_buf = vec![0u8; buf_size];

    let total_passes = config.iterations + if config.zero_pass { 1 } else { 0 };

    // Random passes
    for pass in 0..config.iterations {
        if config.verbose {
            eprintln!(
                "shred: {}: pass {}/{} (random)...",
                path.display(),
                pass + 1,
                total_passes
            );
        }
        file.seek(io::SeekFrom::Start(0))?;
        let mut remaining = write_size;
        while remaining > 0 {
            let chunk = remaining.min(rng_buf.len() as u64) as usize;
            fill_random(&mut rng_buf[..chunk]);
            file.write_all(&rng_buf[..chunk])?;
            remaining -= chunk as u64;
        }
        file.sync_all()?;
    }

    // Zero pass
    if config.zero_pass {
        if config.verbose {
            eprintln!(
                "shred: {}: pass {}/{} (000000)...",
                path.display(),
                total_passes,
                total_passes
            );
        }
        file.seek(io::SeekFrom::Start(0))?;
        let zeros = vec![0u8; buf_size];
        let mut remaining = write_size;
        while remaining > 0 {
            let chunk = remaining.min(zeros.len() as u64) as usize;
            file.write_all(&zeros[..chunk])?;
            remaining -= chunk as u64;
        }
        file.sync_all()?;
    }

    drop(file);

    // Remove file if requested
    if let Some(ref mode) = config.remove {
        match mode {
            RemoveMode::Wipe | RemoveMode::WipeSync => {
                // Try to rename the file to obscure the name before removing
                if let Some(parent) = path.parent() {
                    let name_len = path.file_name().map(|n| n.len()).unwrap_or(1);
                    // Rename to progressively shorter names
                    let mut current = path.to_path_buf();
                    let mut len = name_len;
                    while len > 0 {
                        let new_name: String = std::iter::repeat_n('0', len).collect();
                        let new_path = parent.join(&new_name);
                        if fs::rename(&current, &new_path).is_ok() {
                            if *mode == RemoveMode::WipeSync {
                                // Sync the directory
                                if let Ok(dir) = fs::File::open(parent) {
                                    let _ = dir.sync_all();
                                }
                            }
                            current = new_path;
                        }
                        len /= 2;
                    }
                    if config.verbose {
                        eprintln!("shred: {}: removed", path.display());
                    }
                    fs::remove_file(&current)?;
                } else {
                    if config.verbose {
                        eprintln!("shred: {}: removed", path.display());
                    }
                    fs::remove_file(path)?;
                }
            }
            RemoveMode::Unlink => {
                if config.verbose {
                    eprintln!("shred: {}: removed", path.display());
                }
                fs::remove_file(path)?;
            }
        }
    }

    Ok(())
}

/// Parse a size string with optional suffix (K, M, G, etc.).
pub fn parse_size(s: &str) -> Result<u64, String> {
    if s.is_empty() {
        return Err("invalid size: ''".to_string());
    }

    let s = s.trim();

    // Check for suffix
    let (num_str, multiplier) = if s.ends_with("GB") || s.ends_with("gB") {
        (&s[..s.len() - 2], 1_000_000_000u64)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1_000_000u64)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1_000u64)
    } else if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1_073_741_824u64)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1_048_576u64)
    } else if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1_024u64)
    } else {
        (s, 1u64)
    };

    let value: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid size: '{}'", s))?;

    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("size too large: '{}'", s))
}

/// Parse a --remove[=HOW] argument.
pub fn parse_remove_mode(arg: &str) -> Result<RemoveMode, String> {
    if arg == "--remove" || arg == "-u" {
        Ok(RemoveMode::WipeSync)
    } else if let Some(how) = arg.strip_prefix("--remove=") {
        match how {
            "unlink" => Ok(RemoveMode::Unlink),
            "wipe" => Ok(RemoveMode::Wipe),
            "wipesync" => Ok(RemoveMode::WipeSync),
            _ => Err(format!(
                "invalid argument '{}' for '--remove'\nValid arguments are:\n  - 'unlink'\n  - 'wipe'\n  - 'wipesync'",
                how
            )),
        }
    } else {
        Err(format!("unrecognized option '{}'", arg))
    }
}
