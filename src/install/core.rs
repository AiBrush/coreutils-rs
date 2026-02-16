use std::fs;
use std::io;
use std::path::Path;

/// Backup mode for destination files (shared with mv).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupMode {
    Simple,
    Numbered,
    Existing,
    None,
}

/// Configuration for install operations.
#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub mode: u32,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub directory_mode: bool,
    pub create_leading: bool,
    pub compare: bool,
    pub preserve_timestamps: bool,
    pub strip: bool,
    pub strip_program: String,
    pub verbose: bool,
    pub backup: Option<BackupMode>,
    pub suffix: String,
    pub target_directory: Option<String>,
    pub no_target_directory: bool,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            mode: 0o755,
            owner: None,
            group: None,
            directory_mode: false,
            create_leading: false,
            compare: false,
            preserve_timestamps: false,
            strip: false,
            strip_program: "strip".to_string(),
            verbose: false,
            backup: None,
            suffix: "~".to_string(),
            target_directory: None,
            no_target_directory: false,
        }
    }
}

/// Parse a backup control string.
pub fn parse_backup_mode(s: &str) -> Option<BackupMode> {
    match s {
        "none" | "off" => Some(BackupMode::None),
        "simple" | "never" => Some(BackupMode::Simple),
        "numbered" | "t" => Some(BackupMode::Numbered),
        "existing" | "nil" => Some(BackupMode::Existing),
        _ => Option::None,
    }
}

/// Generate a backup file name for a given destination path.
pub fn make_backup_name(dst: &Path, mode: &BackupMode, suffix: &str) -> std::path::PathBuf {
    match mode {
        BackupMode::Simple | BackupMode::None => {
            let mut name = dst.as_os_str().to_os_string();
            name.push(suffix);
            std::path::PathBuf::from(name)
        }
        BackupMode::Numbered => make_numbered_backup(dst),
        BackupMode::Existing => {
            if has_numbered_backup(dst) {
                make_numbered_backup(dst)
            } else {
                let mut name = dst.as_os_str().to_os_string();
                name.push(suffix);
                std::path::PathBuf::from(name)
            }
        }
    }
}

fn has_numbered_backup(path: &Path) -> bool {
    let file_name = match path.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => return false,
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}.~", file_name)) && name.ends_with('~') {
                let middle = &name[file_name.len() + 2..name.len() - 1];
                if middle.parse::<u64>().is_ok() {
                    return true;
                }
            }
        }
    }
    false
}

fn make_numbered_backup(path: &Path) -> std::path::PathBuf {
    let mut n = 1u64;
    loop {
        let candidate = format!("{}.~{}~", path.display(), n);
        let p = std::path::PathBuf::from(&candidate);
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Parse a mode string (octal) into a u32.
pub fn parse_mode(mode_str: &str) -> Result<u32, String> {
    u32::from_str_radix(mode_str, 8).map_err(|_| format!("invalid mode: '{}'", mode_str))
}

/// Install a single file from `src` to `dst`.
pub fn install_file(src: &Path, dst: &Path, config: &InstallConfig) -> io::Result<()> {
    // Create leading directories if -D
    if config.create_leading {
        if let Some(parent) = dst.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
    }

    // Handle backup
    if dst.exists() {
        if let Some(ref mode) = config.backup {
            let backup_name = make_backup_name(dst, mode, &config.suffix);
            fs::rename(dst, &backup_name)?;
        }
    }

    // Compare if -C: skip copy if files are identical
    if config.compare && dst.exists() {
        if files_are_identical(src, dst)? {
            return Ok(());
        }
    }

    // Copy file — use optimized path on Linux
    #[cfg(target_os = "linux")]
    {
        optimized_copy(src, dst)?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        fs::copy(src, dst)?;
    }

    // Set mode
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dst, fs::Permissions::from_mode(config.mode))?;
    }

    // Set ownership if specified
    #[cfg(unix)]
    if config.owner.is_some() || config.group.is_some() {
        set_ownership(dst, &config.owner, &config.group)?;
    }

    // Preserve timestamps
    if config.preserve_timestamps {
        preserve_times(src, dst)?;
    }

    // Strip if requested
    if config.strip {
        strip_binary(dst, &config.strip_program)?;
    }

    if config.verbose {
        eprintln!("'{}' -> '{}'", src.display(), dst.display());
    }

    Ok(())
}

/// Create directories (install -d).
pub fn install_directories(dirs: &[&Path], config: &InstallConfig) -> io::Result<()> {
    for dir in dirs {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(config.mode))?;
        }
        if config.verbose {
            eprintln!("creating directory '{}'", dir.display());
        }
    }
    Ok(())
}

/// Check if two files have identical contents.
fn files_are_identical(a: &Path, b: &Path) -> io::Result<bool> {
    let meta_a = fs::metadata(a)?;
    let meta_b = fs::metadata(b)?;

    // Quick check: different sizes means different
    if meta_a.len() != meta_b.len() {
        return Ok(false);
    }

    // For large files, use mmap to avoid double allocation
    #[cfg(target_os = "linux")]
    if meta_a.len() > 1024 * 1024 {
        let file_a = fs::File::open(a)?;
        let file_b = fs::File::open(b)?;
        let mmap_a = unsafe { memmap2::MmapOptions::new().map(&file_a)? };
        let mmap_b = unsafe { memmap2::MmapOptions::new().map(&file_b)? };
        return Ok(mmap_a[..] == mmap_b[..]);
    }

    let data_a = fs::read(a)?;
    let data_b = fs::read(b)?;
    Ok(data_a == data_b)
}

/// Set ownership on a file using chown(2).
#[cfg(unix)]
fn set_ownership(path: &Path, owner: &Option<String>, group: &Option<String>) -> io::Result<()> {
    use std::ffi::CString;

    let uid = if let Some(name) = owner {
        resolve_uid(name)?
    } else {
        u32::MAX // -1 means "don't change"
    };

    let gid = if let Some(name) = group {
        resolve_gid(name)?
    } else {
        u32::MAX
    };

    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains null byte"))?;

    let ret = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    if ret != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Resolve a username or numeric UID to a uid_t.
#[cfg(unix)]
fn resolve_uid(name: &str) -> io::Result<u32> {
    if let Ok(uid) = name.parse::<u32>() {
        return Ok(uid);
    }
    let c_name = std::ffi::CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid username"))?;
    let pw = unsafe { libc::getpwnam(c_name.as_ptr()) };
    if pw.is_null() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("invalid user: '{}'", name),
        ))
    } else {
        Ok(unsafe { (*pw).pw_uid })
    }
}

/// Resolve a group name or numeric GID to a gid_t.
#[cfg(unix)]
fn resolve_gid(name: &str) -> io::Result<u32> {
    if let Ok(gid) = name.parse::<u32>() {
        return Ok(gid);
    }
    let c_name = std::ffi::CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid group name"))?;
    let gr = unsafe { libc::getgrnam(c_name.as_ptr()) };
    if gr.is_null() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("invalid group: '{}'", name),
        ))
    } else {
        Ok(unsafe { (*gr).gr_gid })
    }
}

/// Preserve access and modification times from src to dst.
fn preserve_times(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(src)?;
        let atime = libc::timespec {
            tv_sec: meta.atime(),
            tv_nsec: meta.atime_nsec(),
        };
        let mtime = libc::timespec {
            tv_sec: meta.mtime(),
            tv_nsec: meta.mtime_nsec(),
        };
        let times = [atime, mtime];
        let c_path = std::ffi::CString::new(dst.as_os_str().as_encoded_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains null byte"))?;
        let ret = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (src, dst);
    }

    Ok(())
}

/// Optimized file copy on Linux: try FICLONE (CoW reflink), then copy_file_range,
/// then fall back to fs::copy.
#[cfg(target_os = "linux")]
fn optimized_copy(src: &Path, dst: &Path) -> io::Result<u64> {
    use std::os::unix::io::AsRawFd;

    let src_file = fs::File::open(src)?;
    let src_meta = src_file.metadata()?;
    let file_size = src_meta.len();

    // Create destination with same permissions initially
    let dst_file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;

    // Try FICLONE first (instant CoW copy on btrfs/XFS/OCFS2)
    const FICLONE: libc::c_ulong = 0x40049409;
    let ret = unsafe { libc::ioctl(dst_file.as_raw_fd(), FICLONE, src_file.as_raw_fd()) };
    if ret == 0 {
        return Ok(file_size);
    }

    // Try copy_file_range for zero-copy in-kernel copy
    let mut off_in: i64 = 0;
    let mut off_out: i64 = 0;
    let mut remaining = file_size;
    let mut used_cfr = false;

    while remaining > 0 {
        let chunk = remaining.min(1 << 30) as usize; // 1GB max per call
        let n = unsafe {
            libc::syscall(
                libc::SYS_copy_file_range,
                src_file.as_raw_fd(),
                &mut off_in as *mut i64,
                dst_file.as_raw_fd(),
                &mut off_out as *mut i64,
                chunk,
                0u32,
            )
        };
        if n <= 0 {
            if !used_cfr {
                // copy_file_range not supported, fall back
                drop(dst_file);
                drop(src_file);
                return fs::copy(src, dst);
            }
            // Partial failure after some success — this is an error
            return Err(io::Error::last_os_error());
        }
        used_cfr = true;
        remaining -= n as u64;
    }

    Ok(file_size)
}

/// Strip symbol tables from a binary using an external strip program.
fn strip_binary(path: &Path, strip_program: &str) -> io::Result<()> {
    let status = std::process::Command::new(strip_program)
        .arg(path)
        .status()?;
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "{} failed with exit code {}",
                strip_program,
                status.code().unwrap_or(-1)
            ),
        ));
    }
    Ok(())
}
