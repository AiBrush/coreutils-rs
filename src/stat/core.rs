use std::ffi::CString;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};

/// Configuration for the stat command.
pub struct StatConfig {
    pub dereference: bool,
    pub filesystem: bool,
    pub format: Option<String>,
    pub printf_format: Option<String>,
    pub terse: bool,
}

/// Extract the fsid value from a libc::fsid_t as a u64.
fn extract_fsid(fsid: &libc::fsid_t) -> u64 {
    // fsid_t is an opaque type; safely read its raw bytes
    let bytes: [u8; std::mem::size_of::<libc::fsid_t>()] =
        unsafe { std::mem::transmute_copy(fsid) };
    // Interpret as two i32 values in native endian (matching __val[0] and __val[1])
    let val0 = i32::from_ne_bytes(bytes[0..4].try_into().unwrap()) as u64;
    let val1 = i32::from_ne_bytes(bytes[4..8].try_into().unwrap()) as u64;
    (val0 << 32) | val1
}

/// Perform a libc stat/lstat call and return the raw `libc::stat` structure.
fn raw_stat(path: &str, dereference: bool) -> Result<libc::stat, io::Error> {
    let c_path = CString::new(path)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid path"))?;
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        let rc = if dereference {
            libc::stat(c_path.as_ptr(), &mut st)
        } else {
            libc::lstat(c_path.as_ptr(), &mut st)
        };
        if rc != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(st)
        }
    }
}

/// Perform a libc statfs call and return the raw `libc::statfs` structure.
fn raw_statfs(path: &str) -> Result<libc::statfs, io::Error> {
    let c_path = CString::new(path)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid path"))?;
    unsafe {
        let mut sfs: libc::statfs = std::mem::zeroed();
        let rc = libc::statfs(c_path.as_ptr(), &mut sfs);
        if rc != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(sfs)
        }
    }
}

/// Display file or filesystem status.
///
/// Returns the formatted output string, or an error if the file cannot be accessed.
pub fn stat_file(path: &str, config: &StatConfig) -> Result<String, io::Error> {
    if config.filesystem {
        stat_filesystem(path, config)
    } else {
        stat_regular(path, config)
    }
}

// ──────────────────────────────────────────────────
// Regular file stat
// ──────────────────────────────────────────────────

fn stat_regular(path: &str, config: &StatConfig) -> Result<String, io::Error> {
    let meta = if config.dereference {
        std::fs::metadata(path)?
    } else {
        std::fs::symlink_metadata(path)?
    };
    let st = raw_stat(path, config.dereference)?;

    if let Some(ref fmt) = config.printf_format {
        let expanded = expand_backslash_escapes(fmt);
        return Ok(format_file_specifiers(&expanded, path, &meta, &st));
    }

    if let Some(ref fmt) = config.format {
        let result = format_file_specifiers(fmt, path, &meta, &st);
        return Ok(result + "\n");
    }

    if config.terse {
        return Ok(format_file_terse(path, &meta, &st));
    }

    Ok(format_file_default(path, &meta, &st))
}

// ──────────────────────────────────────────────────
// Filesystem stat
// ──────────────────────────────────────────────────

fn stat_filesystem(path: &str, config: &StatConfig) -> Result<String, io::Error> {
    let sfs = raw_statfs(path)?;

    if let Some(ref fmt) = config.printf_format {
        let expanded = expand_backslash_escapes(fmt);
        return Ok(format_fs_specifiers(&expanded, path, &sfs));
    }

    if let Some(ref fmt) = config.format {
        let result = format_fs_specifiers(fmt, path, &sfs);
        return Ok(result + "\n");
    }

    if config.terse {
        return Ok(format_fs_terse(path, &sfs));
    }

    Ok(format_fs_default(path, &sfs))
}

// ──────────────────────────────────────────────────
// Default file format
// ──────────────────────────────────────────────────

fn format_file_default(path: &str, meta: &std::fs::Metadata, st: &libc::stat) -> String {
    let mode = meta.mode();
    let file_type_str = file_type_label(mode);
    let perms_str = mode_to_human(mode);
    let uid = meta.uid();
    let gid = meta.gid();
    let uname = lookup_username(uid);
    let gname = lookup_groupname(gid);
    let dev = meta.dev();
    let dev_major = major(dev);
    let dev_minor = minor(dev);

    let name_display = if meta.file_type().is_symlink() {
        match std::fs::read_link(path) {
            Ok(target) => format!("'{}' -> '{}'", path, target.display()),
            Err(_) => format!("'{}'", path),
        }
    } else {
        format!("'{}'", path)
    };

    let size_line = if meta.file_type().is_block_device() || meta.file_type().is_char_device() {
        let rdev = meta.rdev();
        let rmaj = major(rdev);
        let rmin = minor(rdev);
        format!(
            "  Size: {:<15} Blocks: {:<10} IO Block: {:<6} {}",
            format!("{}, {}", rmaj, rmin),
            meta.blocks(),
            meta.blksize(),
            file_type_str
        )
    } else {
        format!(
            "  Size: {:<15} Blocks: {:<10} IO Block: {:<6} {}",
            meta.size(),
            meta.blocks(),
            meta.blksize(),
            file_type_str
        )
    };

    let device_line = format!(
        "Device: {:x}h/{}d\tInode: {:<11} Links: {}",
        dev,
        dev_major * 256 + dev_minor,
        meta.ino(),
        meta.nlink()
    );

    let access_line = format!(
        "Access: ({:04o}/{})  Uid: ({:5}/{:>8})   Gid: ({:5}/{:>8})",
        mode & 0o7777,
        perms_str,
        uid,
        uname,
        gid,
        gname
    );

    let atime = format_timestamp(st.st_atime, st.st_atime_nsec);
    let mtime = format_timestamp(st.st_mtime, st.st_mtime_nsec);
    let ctime = format_timestamp(st.st_ctime, st.st_ctime_nsec);
    let birth = format_birth_time(st);

    format!(
        "  File: {}\n{}\n{}\n{}\nAccess: {}\nModify: {}\nChange: {}\n Birth: {}\n",
        name_display, size_line, device_line, access_line, atime, mtime, ctime, birth
    )
}

// ──────────────────────────────────────────────────
// Terse file format
// ──────────────────────────────────────────────────

fn format_file_terse(path: &str, meta: &std::fs::Metadata, st: &libc::stat) -> String {
    let dev = meta.dev();
    format!(
        "{} {} {} {:x} {} {} {} {} {} {} {} {} {} {} {} {}\n",
        path,
        meta.size(),
        meta.blocks(),
        meta.mode(),
        meta.uid(),
        meta.gid(),
        dev,
        meta.ino(),
        meta.nlink(),
        major(meta.rdev()),
        minor(meta.rdev()),
        st.st_atime,
        st.st_mtime,
        st.st_ctime,
        st.st_atime,
        meta.blksize()
    )
}

// ──────────────────────────────────────────────────
// Default filesystem format
// ──────────────────────────────────────────────────

fn format_fs_default(path: &str, sfs: &libc::statfs) -> String {
    #[cfg(target_os = "linux")]
    let fs_type = sfs.f_type;
    #[cfg(not(target_os = "linux"))]
    let fs_type = 0u32;
    let fs_type_name = fs_type_name(fs_type as u64);
    let fsid = sfs.f_fsid;
    let fsid_val = extract_fsid(&fsid);

    #[cfg(target_os = "linux")]
    let namelen = sfs.f_namelen;
    #[cfg(not(target_os = "linux"))]
    let namelen = 255i64; // macOS doesn't expose f_namelen

    #[cfg(target_os = "linux")]
    let frsize = sfs.f_frsize;
    #[cfg(not(target_os = "linux"))]
    let frsize = sfs.f_bsize as u64; // fallback to bsize

    format!(
        "  File: \"{}\"\n    ID: {:x} Namelen: {}     Type: {}\nBlock size: {:<10} Fundamental block size: {}\nBlocks: Total: {:<10} Free: {:<10} Available: {}\nInodes: Total: {:<10} Free: {}\n",
        path,
        fsid_val,
        namelen,
        fs_type_name,
        sfs.f_bsize,
        frsize,
        sfs.f_blocks,
        sfs.f_bfree,
        sfs.f_bavail,
        sfs.f_files,
        sfs.f_ffree
    )
}

// ──────────────────────────────────────────────────
// Terse filesystem format
// ──────────────────────────────────────────────────

fn format_fs_terse(path: &str, sfs: &libc::statfs) -> String {
    let fsid = sfs.f_fsid;
    let fsid_val = extract_fsid(&fsid);

    #[cfg(target_os = "linux")]
    let namelen = sfs.f_namelen;
    #[cfg(not(target_os = "linux"))]
    let namelen = 255i64;

    #[cfg(target_os = "linux")]
    let fs_type = sfs.f_type;
    #[cfg(not(target_os = "linux"))]
    let fs_type = 0u32; // macOS doesn't have f_type

    #[cfg(target_os = "linux")]
    let frsize = sfs.f_frsize;
    #[cfg(not(target_os = "linux"))]
    let frsize = sfs.f_bsize as u64;

    format!(
        "{} {} {} {} {} {} {} {} {} {} {} {}\n",
        path,
        fsid_val,
        namelen,
        fs_type,
        sfs.f_bsize,
        frsize,
        sfs.f_blocks,
        sfs.f_bfree,
        sfs.f_bavail,
        sfs.f_files,
        sfs.f_ffree,
        0 // flags placeholder
    )
}

// ──────────────────────────────────────────────────
// Custom format specifiers for files
// ──────────────────────────────────────────────────

fn format_file_specifiers(
    fmt: &str,
    path: &str,
    meta: &std::fs::Metadata,
    st: &libc::stat,
) -> String {
    let mut result = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                'a' => {
                    result.push_str(&format!("{:o}", meta.mode() & 0o7777));
                }
                'A' => {
                    result.push_str(&mode_to_human(meta.mode()));
                }
                'b' => {
                    result.push_str(&meta.blocks().to_string());
                }
                'B' => {
                    result.push_str("512");
                }
                'd' => {
                    result.push_str(&meta.dev().to_string());
                }
                'D' => {
                    result.push_str(&format!("{:x}", meta.dev()));
                }
                'f' => {
                    result.push_str(&format!("{:x}", meta.mode()));
                }
                'F' => {
                    result.push_str(file_type_label(meta.mode()));
                }
                'g' => {
                    result.push_str(&meta.gid().to_string());
                }
                'G' => {
                    result.push_str(&lookup_groupname(meta.gid()));
                }
                'h' => {
                    result.push_str(&meta.nlink().to_string());
                }
                'i' => {
                    result.push_str(&meta.ino().to_string());
                }
                'm' => {
                    result.push_str(&find_mount_point(path));
                }
                'n' => {
                    result.push_str(path);
                }
                'N' => {
                    if meta.file_type().is_symlink() {
                        match std::fs::read_link(path) {
                            Ok(target) => {
                                result.push_str(&format!("'{}' -> '{}'", path, target.display()));
                            }
                            Err(_) => {
                                result.push_str(&format!("'{}'", path));
                            }
                        }
                    } else {
                        result.push_str(&format!("'{}'", path));
                    }
                }
                'o' => {
                    result.push_str(&meta.blksize().to_string());
                }
                's' => {
                    result.push_str(&meta.size().to_string());
                }
                't' => {
                    result.push_str(&format!("{:x}", major(meta.rdev())));
                }
                'T' => {
                    result.push_str(&format!("{:x}", minor(meta.rdev())));
                }
                'u' => {
                    result.push_str(&meta.uid().to_string());
                }
                'U' => {
                    result.push_str(&lookup_username(meta.uid()));
                }
                'w' => {
                    result.push_str(&format_birth_time(st));
                }
                'W' => {
                    result.push_str(&format_birth_seconds(st));
                }
                'x' => {
                    result.push_str(&format_timestamp(st.st_atime, st.st_atime_nsec));
                }
                'X' => {
                    result.push_str(&st.st_atime.to_string());
                }
                'y' => {
                    result.push_str(&format_timestamp(st.st_mtime, st.st_mtime_nsec));
                }
                'Y' => {
                    result.push_str(&st.st_mtime.to_string());
                }
                'z' => {
                    result.push_str(&format_timestamp(st.st_ctime, st.st_ctime_nsec));
                }
                'Z' => {
                    result.push_str(&st.st_ctime.to_string());
                }
                '%' => {
                    result.push('%');
                }
                other => {
                    result.push('%');
                    result.push(other);
                }
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }

    result
}

// ──────────────────────────────────────────────────
// Custom format specifiers for filesystems
// ──────────────────────────────────────────────────

fn format_fs_specifiers(fmt: &str, path: &str, sfs: &libc::statfs) -> String {
    let mut result = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    let fsid = sfs.f_fsid;
    let fsid_val = extract_fsid(&fsid);

    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                'a' => {
                    result.push_str(&sfs.f_bavail.to_string());
                }
                'b' => {
                    result.push_str(&sfs.f_blocks.to_string());
                }
                'c' => {
                    result.push_str(&sfs.f_files.to_string());
                }
                'd' => {
                    result.push_str(&sfs.f_ffree.to_string());
                }
                'f' => {
                    result.push_str(&sfs.f_bfree.to_string());
                }
                'i' => {
                    result.push_str(&format!("{:x}", fsid_val));
                }
                'l' => {
                    #[cfg(target_os = "linux")]
                    result.push_str(&sfs.f_namelen.to_string());
                    #[cfg(not(target_os = "linux"))]
                    result.push_str("255");
                }
                'n' => {
                    result.push_str(path);
                }
                's' => {
                    result.push_str(&sfs.f_bsize.to_string());
                }
                'S' => {
                    #[cfg(target_os = "linux")]
                    result.push_str(&sfs.f_frsize.to_string());
                    #[cfg(not(target_os = "linux"))]
                    result.push_str(&sfs.f_bsize.to_string());
                }
                't' => {
                    #[cfg(target_os = "linux")]
                    result.push_str(&format!("{:x}", sfs.f_type));
                    #[cfg(not(target_os = "linux"))]
                    result.push('0');
                }
                'T' => {
                    #[cfg(target_os = "linux")]
                    result.push_str(fs_type_name(sfs.f_type as u64));
                    #[cfg(not(target_os = "linux"))]
                    result.push_str("unknown");
                }
                '%' => {
                    result.push('%');
                }
                other => {
                    result.push('%');
                    result.push(other);
                }
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }

    result
}

// ──────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────

/// Convert a Unix file mode to a human-readable permission string like `-rwxr-xr-x`.
pub fn mode_to_human(mode: u32) -> String {
    let file_char = match mode & (libc::S_IFMT as u32) {
        m if m == libc::S_IFREG as u32 => '-',
        m if m == libc::S_IFDIR as u32 => 'd',
        m if m == libc::S_IFLNK as u32 => 'l',
        m if m == libc::S_IFBLK as u32 => 'b',
        m if m == libc::S_IFCHR as u32 => 'c',
        m if m == libc::S_IFIFO as u32 => 'p',
        m if m == libc::S_IFSOCK as u32 => 's',
        _ => '?',
    };

    let mut s = String::with_capacity(10);
    s.push(file_char);

    // Owner
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & (libc::S_ISUID as u32) != 0 {
        if mode & 0o100 != 0 { 's' } else { 'S' }
    } else if mode & 0o100 != 0 {
        'x'
    } else {
        '-'
    });

    // Group
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(if mode & (libc::S_ISGID as u32) != 0 {
        if mode & 0o010 != 0 { 's' } else { 'S' }
    } else if mode & 0o010 != 0 {
        'x'
    } else {
        '-'
    });

    // Others
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(if mode & (libc::S_ISVTX as u32) != 0 {
        if mode & 0o001 != 0 { 't' } else { 'T' }
    } else if mode & 0o001 != 0 {
        'x'
    } else {
        '-'
    });

    s
}

/// Return a human-readable label for the file type portion of a mode value.
pub fn file_type_label(mode: u32) -> &'static str {
    match mode & (libc::S_IFMT as u32) {
        m if m == libc::S_IFREG as u32 => "regular file",
        m if m == libc::S_IFDIR as u32 => "directory",
        m if m == libc::S_IFLNK as u32 => "symbolic link",
        m if m == libc::S_IFBLK as u32 => "block special file",
        m if m == libc::S_IFCHR as u32 => "character special file",
        m if m == libc::S_IFIFO as u32 => "fifo",
        m if m == libc::S_IFSOCK as u32 => "socket",
        _ => "unknown",
    }
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM:SS.NNNNNNNNN +ZZZZ`.
fn format_timestamp(secs: i64, nsec: i64) -> String {
    // Use libc localtime_r for timezone-aware formatting
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }

    let offset_secs = tm.tm_gmtoff;
    let offset_sign = if offset_secs >= 0 { '+' } else { '-' };
    let offset_abs = offset_secs.unsigned_abs();
    let offset_hours = offset_abs / 3600;
    let offset_mins = (offset_abs % 3600) / 60;

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09} {}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        nsec,
        offset_sign,
        offset_hours,
        offset_mins
    )
}

/// Format birth time. Returns "-" if unavailable.
fn format_birth_time(st: &libc::stat) -> String {
    #[cfg(target_os = "linux")]
    {
        // Linux statx provides birth time, but libc::stat does not reliably expose it.
        // st_birthtim is not available on all Linux libc versions.
        // Fall back to "-" which matches GNU stat behavior on older kernels.
        let _ = st;
        "-".to_string()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = st;
        "-".to_string()
    }
}

/// Format birth time as seconds since epoch. Returns "0" if unavailable.
fn format_birth_seconds(st: &libc::stat) -> String {
    let _ = st;
    "0".to_string()
}

/// Extract the major device number from a dev_t.
fn major(dev: u64) -> u64 {
    // Linux major/minor encoding
    ((dev >> 8) & 0xff) | ((dev >> 32) & !0xffu64)
}

/// Extract the minor device number from a dev_t.
fn minor(dev: u64) -> u64 {
    (dev & 0xff) | ((dev >> 12) & !0xffu64)
}

/// Look up a username by UID. Returns the numeric UID as string if lookup fails.
fn lookup_username(uid: u32) -> String {
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return uid.to_string();
        }
        let name = std::ffi::CStr::from_ptr((*pw).pw_name);
        name.to_string_lossy().into_owned()
    }
}

/// Look up a group name by GID. Returns the numeric GID as string if lookup fails.
fn lookup_groupname(gid: u32) -> String {
    unsafe {
        let gr = libc::getgrgid(gid);
        if gr.is_null() {
            return gid.to_string();
        }
        let name = std::ffi::CStr::from_ptr((*gr).gr_name);
        name.to_string_lossy().into_owned()
    }
}

/// Find the mount point for a given path by walking up the directory tree.
fn find_mount_point(path: &str) -> String {
    use std::path::PathBuf;

    let abs = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => PathBuf::from(path),
    };

    let mut current = abs.as_path();
    let dev = match std::fs::metadata(current) {
        Ok(m) => m.dev(),
        Err(_) => return "/".to_string(),
    };

    loop {
        match current.parent() {
            Some(parent) => {
                match std::fs::metadata(parent) {
                    Ok(pm) => {
                        if pm.dev() != dev {
                            return current.to_string_lossy().into_owned();
                        }
                    }
                    Err(_) => {
                        return current.to_string_lossy().into_owned();
                    }
                }
                current = parent;
            }
            None => {
                return current.to_string_lossy().into_owned();
            }
        }
    }
}

/// Expand backslash escape sequences in a format string (for --printf).
pub fn expand_backslash_escapes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                'r' => result.push('\r'),
                'a' => result.push('\x07'),
                'b' => result.push('\x08'),
                'f' => result.push('\x0C'),
                'v' => result.push('\x0B'),
                '\\' => result.push('\\'),
                '"' => result.push('"'),
                '0' => {
                    // Octal escape: \0NNN (up to 3 octal digits after the leading 0)
                    let mut val: u32 = 0;
                    let mut count = 0;
                    while i + 1 < chars.len() && count < 3 {
                        let next = chars[i + 1];
                        if next >= '0' && next <= '7' {
                            val = val * 8 + (next as u32 - '0' as u32);
                            i += 1;
                            count += 1;
                        } else {
                            break;
                        }
                    }
                    if let Some(c) = char::from_u32(val) {
                        result.push(c);
                    }
                }
                other => {
                    result.push('\\');
                    result.push(other);
                }
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }

    result
}

/// Map a filesystem type magic number to a human-readable name.
fn fs_type_name(fs_type: u64) -> &'static str {
    // Common Linux filesystem magic numbers
    match fs_type {
        0xEF53 => "ext2/ext3",
        0x6969 => "nfs",
        0x58465342 => "xfs",
        0x2FC12FC1 => "zfs",
        0x9123683E => "btrfs",
        0x01021994 => "tmpfs",
        0x28cd3d45 => "cramfs",
        0x3153464a => "jfs",
        0x52654973 => "reiserfs",
        0x7275 => "romfs",
        0x858458f6 => "ramfs",
        0x73717368 => "squashfs",
        0x62646576 => "devfs",
        0x64626720 => "debugfs",
        0x1cd1 => "devpts",
        0xf15f => "ecryptfs",
        0x794c7630 => "overlayfs",
        0xFF534D42 => "cifs",
        0xfe534d42 => "smb2",
        0x137F => "minix",
        0x4d44 => "msdos",
        0x4006 => "fat",
        0x65735546 => "fuse",
        0x65735543 => "fusectl",
        0x9fa0 => "proc",
        0x62656572 => "sysfs",
        0x27e0eb => "cgroup",
        0x63677270 => "cgroup2",
        0x19800202 => "mqueue",
        0x50495045 => "pipefs",
        0x74726163 => "tracefs",
        0x68756773 => "hugetlbfs",
        0xBAD1DEA => "futexfs",
        0x5346544e => "ntfs",
        0x00011954 => "ufs",
        _ => "UNKNOWN",
    }
}
