use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// How to dereference (follow) symbolic links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerefMode {
    /// Never follow symlinks (copy the link itself).
    Never,
    /// Follow symlinks given on the command line, but not encountered during recursion.
    CommandLine,
    /// Always follow symlinks.
    Always,
}

/// Backup strategy, following GNU `--backup` semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupMode {
    /// Numbered backups (~1~, ~2~, ...).
    Numbered,
    /// Numbered if numbered backups already exist, otherwise simple.
    Existing,
    /// Simple backup with suffix.
    Simple,
    /// Never make backups.
    None,
}

/// Reflink (copy-on-write clone) strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflinkMode {
    /// Try reflink, fall back to normal copy.
    Auto,
    /// Require reflink; fail if not supported.
    Always,
    /// Never attempt reflink.
    Never,
}

/// Configuration for a cp invocation.
pub struct CpConfig {
    pub recursive: bool,
    pub force: bool,
    pub interactive: bool,
    pub no_clobber: bool,
    pub verbose: bool,
    pub preserve_mode: bool,
    pub preserve_ownership: bool,
    pub preserve_timestamps: bool,
    pub dereference: DerefMode,
    pub link: bool,
    pub symbolic_link: bool,
    pub update: bool,
    pub one_file_system: bool,
    pub backup: Option<BackupMode>,
    pub suffix: String,
    pub reflink: ReflinkMode,
    pub target_directory: Option<String>,
    pub no_target_directory: bool,
}

impl Default for CpConfig {
    fn default() -> Self {
        Self {
            recursive: false,
            force: false,
            interactive: false,
            no_clobber: false,
            verbose: false,
            preserve_mode: false,
            preserve_ownership: false,
            preserve_timestamps: false,
            dereference: DerefMode::CommandLine,
            link: false,
            symbolic_link: false,
            update: false,
            one_file_system: false,
            backup: None,
            suffix: "~".to_string(),
            reflink: ReflinkMode::Auto,
            target_directory: None,
            no_target_directory: false,
        }
    }
}

/// Parse a `--backup=CONTROL` value.
pub fn parse_backup_mode(s: &str) -> Result<BackupMode, String> {
    match s {
        "none" | "off" => Ok(BackupMode::None),
        "numbered" | "t" => Ok(BackupMode::Numbered),
        "existing" | "nil" => Ok(BackupMode::Existing),
        "simple" | "never" => Ok(BackupMode::Simple),
        _ => Err(format!("invalid backup type '{}'", s)),
    }
}

/// Parse a `--reflink[=WHEN]` value.
pub fn parse_reflink_mode(s: &str) -> Result<ReflinkMode, String> {
    match s {
        "auto" => Ok(ReflinkMode::Auto),
        "always" => Ok(ReflinkMode::Always),
        "never" => Ok(ReflinkMode::Never),
        _ => Err(format!("invalid reflink value '{}'", s)),
    }
}

/// Parse a `--preserve[=LIST]` attribute list.
///
/// Supports: mode, ownership, timestamps, links, context, xattr, all.
pub fn apply_preserve(list: &str, config: &mut CpConfig) {
    for attr in list.split(',') {
        match attr.trim() {
            "mode" => config.preserve_mode = true,
            "ownership" => config.preserve_ownership = true,
            "timestamps" => config.preserve_timestamps = true,
            "links" | "context" | "xattr" => { /* acknowledged but not yet implemented */ }
            "all" => {
                config.preserve_mode = true;
                config.preserve_ownership = true;
                config.preserve_timestamps = true;
            }
            _ => {}
        }
    }
}

// ---- backup helpers ----

/// Create a backup of `dst` if it exists, according to the configured backup mode.
/// Returns `Ok(())` when no backup is needed or the backup was made successfully.
fn make_backup(dst: &Path, config: &CpConfig) -> io::Result<()> {
    let mode = match config.backup {
        Some(m) => m,
        None => return Ok(()),
    };
    if mode == BackupMode::None {
        return Ok(());
    }
    if !dst.exists() {
        return Ok(());
    }

    let backup_path = match mode {
        BackupMode::Simple | BackupMode::None => {
            let mut p = dst.as_os_str().to_os_string();
            p.push(&config.suffix);
            std::path::PathBuf::from(p)
        }
        BackupMode::Numbered => numbered_backup_path(dst),
        BackupMode::Existing => {
            // Use numbered if any numbered backup already exists.
            let numbered = numbered_backup_candidate(dst, 1);
            if numbered.exists() {
                numbered_backup_path(dst)
            } else {
                let mut p = dst.as_os_str().to_os_string();
                p.push(&config.suffix);
                std::path::PathBuf::from(p)
            }
        }
    };

    std::fs::rename(dst, &backup_path)?;
    Ok(())
}

fn numbered_backup_path(dst: &Path) -> std::path::PathBuf {
    let mut n: u64 = 1;
    loop {
        let candidate = numbered_backup_candidate(dst, n);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn numbered_backup_candidate(dst: &Path, n: u64) -> std::path::PathBuf {
    let mut p = dst.as_os_str().to_os_string();
    p.push(format!(".~{}~", n));
    std::path::PathBuf::from(p)
}

// ---- attribute preservation ----

/// Preserve file attributes (mode, timestamps, ownership) from `src` on `dst`
/// according to the configuration.
fn preserve_attributes(src: &Path, dst: &Path, config: &CpConfig) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;

    #[cfg(unix)]
    if config.preserve_mode {
        let mode = meta.mode();
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode))?;
    }

    #[cfg(unix)]
    if config.preserve_timestamps {
        let atime_spec = libc::timespec {
            tv_sec: meta.atime(),
            tv_nsec: meta.atime_nsec(),
        };
        let mtime_spec = libc::timespec {
            tv_sec: meta.mtime(),
            tv_nsec: meta.mtime_nsec(),
        };
        let times = [atime_spec, mtime_spec];
        // SAFETY: CString::new checks for interior NULs; the path is valid UTF-8/bytes.
        let c_path = std::ffi::CString::new(dst.as_os_str().as_encoded_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        // SAFETY: c_path is a valid NUL-terminated C string, times is a valid [timespec; 2].
        let ret = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
    }

    #[cfg(unix)]
    if config.preserve_ownership {
        // SAFETY: CString::new checks for interior NULs; the path is valid bytes.
        let c_path = std::ffi::CString::new(dst.as_os_str().as_encoded_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        // SAFETY: c_path is a valid NUL-terminated C string, uid/gid are valid u32 values.
        let ret = unsafe { libc::lchown(c_path.as_ptr(), meta.uid(), meta.gid()) };
        if ret != 0 {
            // Ownership preservation may fail for non-root; ignore EPERM.
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EPERM) {
                return Err(err);
            }
        }
    }

    // Suppress unused-variable warnings on non-unix platforms.
    #[cfg(not(unix))]
    {
        let _ = (&meta, config);
    }

    Ok(())
}

// ---- Linux copy_file_range optimisation ----

#[cfg(target_os = "linux")]
fn copy_file_range_linux(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;

    let src_file = std::fs::File::open(src)?;
    let src_meta = src_file.metadata()?;
    let len = src_meta.len();

    let dst_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;

    let mut remaining = len as i64;
    while remaining > 0 {
        // Cap to isize::MAX to avoid overflow on 32-bit when casting to usize.
        let to_copy = (remaining as u64).min(isize::MAX as u64) as usize;
        // SAFETY: src_file and dst_file are valid open file descriptors;
        // null offsets mean the kernel uses and updates the file offsets.
        // Uses raw syscall instead of libc::copy_file_range to support
        // older glibc versions (e.g. cross-compilation with cross-rs).
        let ret = unsafe {
            libc::syscall(
                libc::SYS_copy_file_range,
                src_file.as_raw_fd(),
                std::ptr::null_mut::<libc::off64_t>(),
                dst_file.as_raw_fd(),
                std::ptr::null_mut::<libc::off64_t>(),
                to_copy,
                0u32,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        if ret == 0 {
            // EOF before all bytes copied — break to avoid infinite loop
            break;
        }
        remaining -= ret as i64;
    }
    Ok(())
}

// ---- single-file copy ----

/// Copy a single file (or symlink) from `src` to `dst`.
pub fn copy_file(src: &Path, dst: &Path, config: &CpConfig) -> io::Result<()> {
    let src_meta = if config.dereference == DerefMode::Always {
        std::fs::metadata(src)?
    } else {
        std::fs::symlink_metadata(src)?
    };

    // Handle symlink when not dereferencing.
    if src_meta.file_type().is_symlink() && config.dereference == DerefMode::Never {
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, dst)?;
        }
        #[cfg(not(unix))]
        {
            // Fallback: try a regular copy (symlinks are not portable).
            let _ = target;
            std::fs::copy(src, dst)?;
        }
        return Ok(());
    }

    // Hard link mode.
    if config.link {
        std::fs::hard_link(src, dst)?;
        return Ok(());
    }

    // Symbolic link mode.
    if config.symbolic_link {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(src, dst)?;
        }
        #[cfg(not(unix))]
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "symbolic links are not supported on this platform",
            ));
        }
        return Ok(());
    }

    // Try Linux copy_file_range for zero-copy.
    #[cfg(target_os = "linux")]
    {
        match copy_file_range_linux(src, dst) {
            Ok(()) => {
                preserve_attributes(src, dst, config)?;
                return Ok(());
            }
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::EINVAL | libc::ENOSYS | libc::EXDEV)
                ) =>
            {
                // Unsupported/cross-device — fall through to std::fs::copy
            }
            Err(e) => return Err(e),
        }
    }

    // Fallback: standard copy.
    std::fs::copy(src, dst)?;
    preserve_attributes(src, dst, config)?;
    Ok(())
}

// ---- recursive copy ----

/// Recursively copy `src` to `dst`.
fn copy_recursive(
    src: &Path,
    dst: &Path,
    config: &CpConfig,
    root_dev: Option<u64>,
) -> io::Result<()> {
    let src_meta = std::fs::symlink_metadata(src)?;

    #[cfg(unix)]
    if config.one_file_system {
        if let Some(dev) = root_dev {
            if src_meta.dev() != dev {
                return Ok(());
            }
        }
    }

    if src_meta.is_dir() {
        if !dst.exists() {
            std::fs::create_dir_all(dst)?;
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let child_dst = dst.join(entry.file_name());
            #[cfg(unix)]
            let next_dev = Some(root_dev.unwrap_or(src_meta.dev()));
            #[cfg(not(unix))]
            let next_dev: Option<u64> = None;
            copy_recursive(&entry.path(), &child_dst, config, next_dev)?;
        }
        // Preserve directory attributes after copying contents.
        preserve_attributes(src, dst, config)?;
    } else {
        // If parent directory does not exist, create it.
        if let Some(parent) = dst.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        copy_file(src, dst, config)?;
    }
    Ok(())
}

// ---- main entry point ----

/// Determine the effective destination and perform the copy.
///
/// `sources` is the list of source paths; `raw_dest` is the positional destination
/// (may be `None` when `--target-directory` is used).
///
/// Returns a list of per-file error messages (empty on full success) and a bool
/// indicating whether any error occurred.
pub fn run_cp(
    sources: &[String],
    raw_dest: Option<&str>,
    config: &CpConfig,
) -> (Vec<String>, bool) {
    let mut errors: Vec<String> = Vec::new();
    let mut had_error = false;

    // Resolve destination directory.
    let dest_dir: Option<std::path::PathBuf> = config
        .target_directory
        .as_deref()
        .or(raw_dest)
        .map(std::path::PathBuf::from);

    let dest_dir = match dest_dir {
        Some(d) => d,
        None => {
            errors.push("cp: missing destination operand".to_string());
            return (errors, true);
        }
    };

    // Multiple sources or target is an existing directory => copy into directory.
    let copy_into_dir = sources.len() > 1 || dest_dir.is_dir() || config.target_directory.is_some();

    // When -T is set, never treat destination as a directory.
    let copy_into_dir = copy_into_dir && !config.no_target_directory;

    for source in sources {
        let src = Path::new(source);
        let dst = if copy_into_dir {
            let name = src.file_name().unwrap_or(src.as_ref());
            dest_dir.join(name)
        } else {
            dest_dir.clone()
        };

        if let Err(e) = do_copy(src, &dst, config) {
            let msg = format!(
                "cp: cannot copy '{}' to '{}': {}",
                src.display(),
                dst.display(),
                strip_os_error(&e)
            );
            errors.push(msg);
            had_error = true;
        } else if config.verbose {
            // Verbose output goes to stderr to match GNU behavior when piped.
            eprintln!("'{}' -> '{}'", src.display(), dst.display());
        }
    }

    (errors, had_error)
}

/// Core copy dispatcher for a single source -> destination pair.
fn do_copy(src: &Path, dst: &Path, config: &CpConfig) -> io::Result<()> {
    let src_meta = if config.dereference == DerefMode::Always {
        std::fs::metadata(src)?
    } else {
        std::fs::symlink_metadata(src)?
    };

    // Reject directory source without -R.
    if src_meta.is_dir() && !config.recursive {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("omitting directory '{}'", src.display()),
        ));
    }

    // No-clobber: skip if destination exists.
    if config.no_clobber && dst.exists() {
        return Ok(());
    }

    // Update: skip if destination is same age or newer.
    if config.update && dst.exists() {
        if let (Ok(src_m), Ok(dst_m)) = (src.metadata(), dst.metadata()) {
            if let (Ok(src_t), Ok(dst_t)) = (src_m.modified(), dst_m.modified()) {
                if dst_t >= src_t {
                    return Ok(());
                }
            }
        }
    }

    // Interactive: prompt on stderr.
    if config.interactive && dst.exists() {
        eprint!("cp: overwrite '{}'? ", dst.display());
        let mut response = String::new();
        io::stdin().read_line(&mut response)?;
        let r = response.trim().to_lowercase();
        if !(r == "y" || r == "yes") {
            return Ok(());
        }
    }

    // Force: remove existing destination if it cannot be opened for writing.
    if config.force && dst.exists() {
        if let Ok(m) = dst.metadata() {
            if m.permissions().readonly() {
                std::fs::remove_file(dst)?;
            }
        }
    }

    // Make backup if requested.
    make_backup(dst, config)?;

    if src_meta.is_dir() {
        #[cfg(unix)]
        let root_dev = Some(src_meta.dev());
        #[cfg(not(unix))]
        let root_dev: Option<u64> = None;
        copy_recursive(src, dst, config, root_dev)
    } else {
        copy_file(src, dst, config)
    }
}

/// Strip the " (os error N)" suffix from an io::Error for GNU-compatible messages.
fn strip_os_error(e: &io::Error) -> String {
    if let Some(raw) = e.raw_os_error() {
        let msg = format!("{}", e);
        msg.replace(&format!(" (os error {})", raw), "")
    } else {
        format!("{}", e)
    }
}
