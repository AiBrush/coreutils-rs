use std::collections::HashSet;
use std::io::{self, Write};

// ──────────────────────────────────────────────────
// Configuration
// ──────────────────────────────────────────────────

/// Configuration for the df command.
pub struct DfConfig {
    pub all: bool,
    pub block_size: u64,
    pub human_readable: bool,
    pub si: bool,
    pub inodes: bool,
    pub local_only: bool,
    pub portability: bool,
    pub print_type: bool,
    pub total: bool,
    pub sync_before: bool,
    pub type_filter: HashSet<String>,
    pub exclude_type: HashSet<String>,
    pub output_fields: Option<Vec<String>>,
    pub files: Vec<String>,
}

impl Default for DfConfig {
    fn default() -> Self {
        Self {
            all: false,
            block_size: 1024,
            human_readable: false,
            si: false,
            inodes: false,
            local_only: false,
            portability: false,
            print_type: false,
            total: false,
            sync_before: false,
            type_filter: HashSet::new(),
            exclude_type: HashSet::new(),
            output_fields: None,
            files: Vec::new(),
        }
    }
}

// ──────────────────────────────────────────────────
// Mount entry and filesystem info
// ──────────────────────────────────────────────────

/// A parsed mount entry from /proc/mounts.
pub struct MountEntry {
    pub source: String,
    pub target: String,
    pub fstype: String,
}

/// Filesystem information after calling statvfs.
pub struct FsInfo {
    pub source: String,
    pub fstype: String,
    pub target: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub use_percent: f64,
    pub itotal: u64,
    pub iused: u64,
    pub iavail: u64,
    pub iuse_percent: f64,
}

// Remote filesystem types that should be excluded with --local.
const REMOTE_FS_TYPES: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smbfs",
    "ncpfs",
    "afs",
    "coda",
    "ftpfs",
    "mfs",
    "sshfs",
    "fuse.sshfs",
    "ncp",
    "9p",
];

// Pseudo filesystem types filtered out unless --all is given.
const PSEUDO_FS_TYPES: &[&str] = &[
    "sysfs",
    "proc",
    "devtmpfs",
    "devpts",
    "securityfs",
    "cgroup",
    "cgroup2",
    "pstore",
    "efivarfs",
    "bpf",
    "autofs",
    "mqueue",
    "hugetlbfs",
    "debugfs",
    "tracefs",
    "fusectl",
    "configfs",
    "ramfs",
    "binfmt_misc",
    "rpc_pipefs",
    "nsfs",
    "overlay",
    "squashfs",
];

// ──────────────────────────────────────────────────
// Reading mount entries
// ──────────────────────────────────────────────────

/// Read mount entries from /proc/mounts (falls back to /etc/mtab).
pub fn read_mounts() -> Vec<MountEntry> {
    let content = std::fs::read_to_string("/proc/mounts")
        .or_else(|_| std::fs::read_to_string("/etc/mtab"))
        .unwrap_or_default();
    content
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                Some(MountEntry {
                    source: unescape_octal(parts[0]),
                    target: unescape_octal(parts[1]),
                    fstype: parts[2].to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Unescape octal sequences like \040 (space) in mount paths.
fn unescape_octal(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let d1 = bytes[i + 1];
            let d2 = bytes[i + 2];
            let d3 = bytes[i + 3];
            if (b'0'..=b'3').contains(&d1)
                && (b'0'..=b'7').contains(&d2)
                && (b'0'..=b'7').contains(&d3)
            {
                let val = (d1 - b'0') * 64 + (d2 - b'0') * 8 + (d3 - b'0');
                result.push(val as char);
                i += 4;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

// ──────────────────────────────────────────────────
// Calling statvfs
// ──────────────────────────────────────────────────

/// Call statvfs(2) on a path and return filesystem info.
#[cfg(unix)]
fn statvfs_info(mount: &MountEntry) -> Option<FsInfo> {
    use std::ffi::CString;

    let path = CString::new(mount.target.as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(path.as_ptr(), &mut stat) };
    if ret != 0 {
        return None;
    }

    #[cfg(target_os = "linux")]
    let block_size = stat.f_frsize as u64;
    #[cfg(not(target_os = "linux"))]
    let block_size = stat.f_bsize as u64;
    let total = stat.f_blocks as u64 * block_size;
    let free = stat.f_bfree as u64 * block_size;
    let available = stat.f_bavail as u64 * block_size;
    let used = total.saturating_sub(free);

    let use_percent = if total == 0 {
        0.0
    } else {
        // GNU df calculates use% as: used / (used + available) * 100
        let denom = used + available;
        if denom == 0 {
            0.0
        } else {
            (used as f64 / denom as f64) * 100.0
        }
    };

    let itotal = stat.f_files as u64;
    let ifree = stat.f_ffree as u64;
    let iused = itotal.saturating_sub(ifree);
    let iuse_percent = if itotal == 0 {
        0.0
    } else {
        (iused as f64 / itotal as f64) * 100.0
    };

    Some(FsInfo {
        source: mount.source.clone(),
        fstype: mount.fstype.clone(),
        target: mount.target.clone(),
        total,
        used,
        available,
        use_percent,
        itotal,
        iused,
        iavail: ifree,
        iuse_percent,
    })
}

#[cfg(not(unix))]
fn statvfs_info(_mount: &MountEntry) -> Option<FsInfo> {
    None
}

// ──────────────────────────────────────────────────
// Finding filesystem for a specific file
// ──────────────────────────────────────────────────

/// Find the mount entry for a given file path by finding the longest
/// matching mount target prefix.
fn find_mount_for_file<'a>(path: &str, mounts: &'a [MountEntry]) -> Option<&'a MountEntry> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let canonical_str = canonical.to_string_lossy();
    let mut best: Option<&MountEntry> = None;
    let mut best_len = 0;
    for mount in mounts {
        let target = &mount.target;
        if canonical_str.starts_with(target.as_str())
            && (canonical_str.len() == target.len()
                || target == "/"
                || canonical_str.as_bytes().get(target.len()) == Some(&b'/'))
        {
            if target.len() > best_len {
                best_len = target.len();
                best = Some(mount);
            }
        }
    }
    best
}

// ──────────────────────────────────────────────────
// Getting filesystem info
// ──────────────────────────────────────────────────

/// Determine whether a filesystem type is remote.
fn is_remote(fstype: &str) -> bool {
    REMOTE_FS_TYPES.contains(&fstype)
}

/// Determine whether a filesystem type is pseudo.
fn is_pseudo(fstype: &str) -> bool {
    PSEUDO_FS_TYPES.contains(&fstype)
}

/// Get filesystem info for all relevant mount points.
/// Returns (filesystems, had_error) where had_error is true if any file was not found.
pub fn get_filesystems(config: &DfConfig) -> (Vec<FsInfo>, bool) {
    let mounts = read_mounts();
    let mut had_error = false;

    // If specific files are given, find their mount points.
    if !config.files.is_empty() {
        let mut result = Vec::new();
        // GNU df does NOT deduplicate when specific files are given.
        for file in &config.files {
            match find_mount_for_file(file, &mounts) {
                Some(mount) => {
                    if let Some(info) = statvfs_info(mount) {
                        result.push(info);
                    }
                }
                None => {
                    eprintln!("df: {}: No such file or directory", file);
                    had_error = true;
                }
            }
        }
        return (result, had_error);
    }

    let mut result = Vec::new();
    let mut seen_sources = HashSet::new();

    for mount in &mounts {
        // Filter by type.
        if !config.type_filter.is_empty() && !config.type_filter.contains(&mount.fstype) {
            continue;
        }

        // Exclude by type.
        if config.exclude_type.contains(&mount.fstype) {
            continue;
        }

        // Skip remote filesystems if --local.
        if config.local_only && is_remote(&mount.fstype) {
            continue;
        }

        // Skip pseudo filesystems unless --all.
        if !config.all && is_pseudo(&mount.fstype) {
            continue;
        }

        // Skip duplicate sources unless --all (keep last mount for a given device).
        if !config.all {
            if mount.source == "none" || mount.source == "tmpfs" || mount.source == "devtmpfs" {
                // Allow these through; filter by fstype instead of source.
            } else if !seen_sources.insert(mount.source.clone()) {
                continue;
            }
        }

        if let Some(info) = statvfs_info(mount) {
            // Without --all, skip filesystems with 0 total blocks (pseudo/virtual).
            if !config.all && info.total == 0 && config.type_filter.is_empty() {
                continue;
            }
            result.push(info);
        }
    }

    (result, had_error)
}

// ──────────────────────────────────────────────────
// Size formatting
// ──────────────────────────────────────────────────

/// Format a byte count in human-readable form using powers of 1024.
pub fn human_readable_1024(bytes: u64) -> String {
    const UNITS: &[&str] = &["", "K", "M", "G", "T", "P", "E"];
    if bytes == 0 {
        return "0".to_string();
    }
    let mut value = bytes as f64;
    for unit in UNITS {
        if value < 1024.0 {
            if value < 10.0 && !unit.is_empty() {
                return format!("{:.1}{}", value, unit);
            }
            return format!("{:.0}{}", value, unit);
        }
        value /= 1024.0;
    }
    format!("{:.0}E", value)
}

/// Format a byte count in human-readable form using powers of 1000.
pub fn human_readable_1000(bytes: u64) -> String {
    const UNITS: &[&str] = &["", "k", "M", "G", "T", "P", "E"];
    if bytes == 0 {
        return "0".to_string();
    }
    let mut value = bytes as f64;
    for unit in UNITS {
        if value < 1000.0 {
            if value < 10.0 && !unit.is_empty() {
                return format!("{:.1}{}", value, unit);
            }
            return format!("{:.0}{}", value, unit);
        }
        value /= 1000.0;
    }
    format!("{:.0}E", value)
}

/// Format a size value according to the config.
pub fn format_size(bytes: u64, config: &DfConfig) -> String {
    if config.human_readable {
        human_readable_1024(bytes)
    } else if config.si {
        human_readable_1000(bytes)
    } else {
        format!("{}", bytes / config.block_size)
    }
}

/// Format a percentage for display.
fn format_percent(pct: f64) -> String {
    if pct == 0.0 {
        return "0%".to_string();
    }
    // GNU df rounds up: ceil the percentage.
    let rounded = pct.ceil() as u64;
    format!("{}%", rounded)
}

// ──────────────────────────────────────────────────
// Block size parsing
// ──────────────────────────────────────────────────

/// Parse a block size string like "1K", "1M", "1G", etc.
pub fn parse_block_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("invalid block size".to_string());
    }

    // Check for leading apostrophe (thousands grouping) - just strip it.
    let s = s.strip_prefix('\'').unwrap_or(s);

    let (num_str, suffix) = if s
        .as_bytes()
        .last()
        .map_or(false, |b| b.is_ascii_alphabetic())
    {
        let last = s.len() - 1;
        (&s[..last], &s[last..])
    } else {
        (s, "")
    };

    let num: u64 = if num_str.is_empty() {
        1
    } else {
        num_str
            .parse()
            .map_err(|_| format!("invalid block size: '{}'", s))?
    };

    let multiplier = match suffix.to_uppercase().as_str() {
        "" => 1u64,
        "K" => 1024,
        "M" => 1024 * 1024,
        "G" => 1024 * 1024 * 1024,
        "T" => 1024 * 1024 * 1024 * 1024,
        "P" => 1024u64 * 1024 * 1024 * 1024 * 1024,
        "E" => 1024u64 * 1024 * 1024 * 1024 * 1024 * 1024,
        _ => return Err(format!("invalid suffix in block size: '{}'", s)),
    };

    Ok(num * multiplier)
}

// ──────────────────────────────────────────────────
// Valid output field names
// ──────────────────────────────────────────────────

/// Valid field names for --output.
pub const VALID_OUTPUT_FIELDS: &[&str] = &[
    "source", "fstype", "itotal", "iused", "iavail", "ipcent", "size", "used", "avail", "pcent",
    "file", "target",
];

/// Parse the --output field list.
pub fn parse_output_fields(s: &str) -> Result<Vec<String>, String> {
    let fields: Vec<String> = s.split(',').map(|f| f.trim().to_lowercase()).collect();
    for field in &fields {
        if !VALID_OUTPUT_FIELDS.contains(&field.as_str()) {
            return Err(format!("df: '{}': not a valid field for --output", field));
        }
    }
    Ok(fields)
}

// ──────────────────────────────────────────────────
// Output formatting (GNU-compatible auto-sized columns)
// ──────────────────────────────────────────────────

/// Determine the size column header.
fn size_header(config: &DfConfig) -> String {
    if config.human_readable || config.si {
        "Size".to_string()
    } else if config.portability {
        "1024-blocks".to_string()
    } else if config.block_size == 1024 {
        "1K-blocks".to_string()
    } else if config.block_size == 1024 * 1024 {
        "1M-blocks".to_string()
    } else {
        format!("{}-blocks", config.block_size)
    }
}

/// Build a row of string values for a filesystem entry.
fn build_row(info: &FsInfo, config: &DfConfig) -> Vec<String> {
    if let Some(ref fields) = config.output_fields {
        return fields
            .iter()
            .map(|f| match f.as_str() {
                "source" => info.source.clone(),
                "fstype" => info.fstype.clone(),
                "itotal" => format!("{}", info.itotal),
                "iused" => format!("{}", info.iused),
                "iavail" => format!("{}", info.iavail),
                "ipcent" => format_percent(info.iuse_percent),
                "size" => format_size(info.total, config),
                "used" => format_size(info.used, config),
                "avail" => format_size(info.available, config),
                "pcent" => format_percent(info.use_percent),
                "file" => info.target.clone(),
                "target" => info.target.clone(),
                _ => String::new(),
            })
            .collect();
    }

    if config.inodes {
        vec![
            info.source.clone(),
            format!("{}", info.itotal),
            format!("{}", info.iused),
            format!("{}", info.iavail),
            format_percent(info.iuse_percent),
            info.target.clone(),
        ]
    } else if config.print_type {
        vec![
            info.source.clone(),
            info.fstype.clone(),
            format_size(info.total, config),
            format_size(info.used, config),
            format_size(info.available, config),
            format_percent(info.use_percent),
            info.target.clone(),
        ]
    } else {
        vec![
            info.source.clone(),
            format_size(info.total, config),
            format_size(info.used, config),
            format_size(info.available, config),
            format_percent(info.use_percent),
            info.target.clone(),
        ]
    }
}

/// Build the header row.
fn build_header_row(config: &DfConfig) -> Vec<String> {
    if let Some(ref fields) = config.output_fields {
        return fields
            .iter()
            .map(|f| match f.as_str() {
                "source" => "Filesystem".to_string(),
                "fstype" => "Type".to_string(),
                "itotal" => "Inodes".to_string(),
                "iused" => "IUsed".to_string(),
                "iavail" => "IFree".to_string(),
                "ipcent" => "IUse%".to_string(),
                "size" => size_header(config),
                "used" => "Used".to_string(),
                "avail" => "Avail".to_string(),
                "pcent" => "Use%".to_string(),
                "file" => "File".to_string(),
                "target" => "Mounted on".to_string(),
                _ => f.clone(),
            })
            .collect();
    }

    let pct_header = if config.portability {
        "Capacity"
    } else if config.inodes {
        "IUse%"
    } else {
        "Use%"
    };

    if config.inodes {
        vec![
            "Filesystem".to_string(),
            "Inodes".to_string(),
            "IUsed".to_string(),
            "IFree".to_string(),
            pct_header.to_string(),
            "Mounted on".to_string(),
        ]
    } else if config.print_type {
        vec![
            "Filesystem".to_string(),
            "Type".to_string(),
            size_header(config),
            "Used".to_string(),
            if config.portability {
                "Available"
            } else {
                "Avail"
            }
            .to_string(),
            pct_header.to_string(),
            "Mounted on".to_string(),
        ]
    } else {
        vec![
            "Filesystem".to_string(),
            size_header(config),
            "Used".to_string(),
            if config.portability {
                "Available"
            } else {
                "Avail"
            }
            .to_string(),
            pct_header.to_string(),
            "Mounted on".to_string(),
        ]
    }
}

/// Build a total row.
fn build_total_row(filesystems: &[FsInfo], config: &DfConfig) -> Vec<String> {
    let total_size: u64 = filesystems.iter().map(|f| f.total).sum();
    let total_used: u64 = filesystems.iter().map(|f| f.used).sum();
    let total_avail: u64 = filesystems.iter().map(|f| f.available).sum();
    let total_itotal: u64 = filesystems.iter().map(|f| f.itotal).sum();
    let total_iused: u64 = filesystems.iter().map(|f| f.iused).sum();
    let total_iavail: u64 = filesystems.iter().map(|f| f.iavail).sum();

    let use_pct = {
        let denom = total_used + total_avail;
        if denom == 0 {
            0.0
        } else {
            (total_used as f64 / denom as f64) * 100.0
        }
    };
    let iuse_pct = if total_itotal == 0 {
        0.0
    } else {
        (total_iused as f64 / total_itotal as f64) * 100.0
    };

    if config.inodes {
        vec![
            "total".to_string(),
            format!("{}", total_itotal),
            format!("{}", total_iused),
            format!("{}", total_iavail),
            format_percent(iuse_pct),
            "-".to_string(),
        ]
    } else if config.print_type {
        vec![
            "total".to_string(),
            "-".to_string(),
            format_size(total_size, config),
            format_size(total_used, config),
            format_size(total_avail, config),
            format_percent(use_pct),
            "-".to_string(),
        ]
    } else {
        vec![
            "total".to_string(),
            format_size(total_size, config),
            format_size(total_used, config),
            format_size(total_avail, config),
            format_percent(use_pct),
            "-".to_string(),
        ]
    }
}

/// Column alignment type.
enum ColAlign {
    Left,
    Right,
    None, // last column, no padding
}

/// Get column alignment for the standard df output.
fn get_col_alignments(config: &DfConfig, num_cols: usize) -> Vec<ColAlign> {
    if num_cols == 0 {
        return vec![];
    }
    let mut aligns = Vec::with_capacity(num_cols);

    if config.output_fields.is_some() {
        // For --output, first column is left-aligned, rest right-aligned, last is no-pad.
        aligns.push(ColAlign::Left);
        for _ in 1..num_cols.saturating_sub(1) {
            aligns.push(ColAlign::Right);
        }
        if num_cols > 1 {
            aligns.push(ColAlign::None);
        }
    } else if config.print_type {
        // Filesystem(left) Type(left) Size(right) Used(right) Avail(right) Use%(right) Mounted(none)
        aligns.push(ColAlign::Left);
        aligns.push(ColAlign::Left);
        for _ in 2..num_cols.saturating_sub(1) {
            aligns.push(ColAlign::Right);
        }
        if num_cols > 2 {
            aligns.push(ColAlign::None);
        }
    } else {
        // Filesystem(left) numeric(right)... last(none)
        aligns.push(ColAlign::Left);
        for _ in 1..num_cols.saturating_sub(1) {
            aligns.push(ColAlign::Right);
        }
        if num_cols > 1 {
            aligns.push(ColAlign::None);
        }
    }

    aligns
}

/// Print all rows with auto-sized columns, matching GNU df output format.
fn print_table(
    header: &[String],
    rows: &[Vec<String>],
    config: &DfConfig,
    out: &mut impl Write,
) -> io::Result<()> {
    let num_cols = header.len();
    if num_cols == 0 {
        return Ok(());
    }

    // Compute column widths from header and all data rows.
    let mut widths = vec![0usize; num_cols];
    for (i, h) in header.iter().enumerate() {
        widths[i] = widths[i].max(h.len());
    }
    for row in rows {
        for (i, val) in row.iter().enumerate() {
            if i < num_cols {
                widths[i] = widths[i].max(val.len());
            }
        }
    }

    let aligns = get_col_alignments(config, num_cols);

    // Print header.
    print_row(header, &widths, &aligns, out)?;

    // Print data rows.
    for row in rows {
        print_row(row, &widths, &aligns, out)?;
    }

    Ok(())
}

/// Print a single row with the given column widths and alignments.
fn print_row(
    row: &[String],
    widths: &[usize],
    aligns: &[ColAlign],
    out: &mut impl Write,
) -> io::Result<()> {
    let num_cols = widths.len();
    for (i, val) in row.iter().enumerate() {
        if i < num_cols {
            if i > 0 {
                write!(out, " ")?;
            }
            let w = widths[i];
            match aligns.get(i).unwrap_or(&ColAlign::Right) {
                ColAlign::Left => write!(out, "{:<width$}", val, width = w)?,
                ColAlign::Right => write!(out, "{:>width$}", val, width = w)?,
                ColAlign::None => write!(out, "{}", val)?,
            }
        }
    }
    writeln!(out)?;
    Ok(())
}

/// Print the df output header (for backward compat with tests).
pub fn print_header(config: &DfConfig, out: &mut impl Write) -> io::Result<()> {
    let header = build_header_row(config);
    // Use minimal widths from just the header itself.
    let widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    let aligns = get_col_alignments(config, header.len());
    print_row(&header, &widths, &aligns, out)
}

/// Print a single filesystem info line (for backward compat with tests).
pub fn print_fs_line(info: &FsInfo, config: &DfConfig, out: &mut impl Write) -> io::Result<()> {
    let header = build_header_row(config);
    let row = build_row(info, config);
    // Compute widths from both header and this single row.
    let num_cols = header.len();
    let mut widths = vec![0usize; num_cols];
    for (i, h) in header.iter().enumerate() {
        widths[i] = widths[i].max(h.len());
    }
    for (i, v) in row.iter().enumerate() {
        if i < num_cols {
            widths[i] = widths[i].max(v.len());
        }
    }
    let aligns = get_col_alignments(config, num_cols);
    print_row(&row, &widths, &aligns, out)
}

/// Print the total line (for backward compat with tests).
pub fn print_total_line(
    filesystems: &[FsInfo],
    config: &DfConfig,
    out: &mut impl Write,
) -> io::Result<()> {
    let row = build_total_row(filesystems, config);
    let header = build_header_row(config);
    let num_cols = header.len();
    let mut widths = vec![0usize; num_cols];
    for (i, h) in header.iter().enumerate() {
        widths[i] = widths[i].max(h.len());
    }
    for (i, v) in row.iter().enumerate() {
        if i < num_cols {
            widths[i] = widths[i].max(v.len());
        }
    }
    let aligns = get_col_alignments(config, num_cols);
    print_row(&row, &widths, &aligns, out)
}

/// Run the df command and write output.
pub fn run_df(config: &DfConfig) -> i32 {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    let (filesystems, had_error) = get_filesystems(config);

    let header = build_header_row(config);
    let mut rows: Vec<Vec<String>> = Vec::new();
    for info in &filesystems {
        rows.push(build_row(info, config));
    }

    if config.total {
        rows.push(build_total_row(&filesystems, config));
    }

    if let Err(e) = print_table(&header, &rows, config, &mut out) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            return 0;
        }
        eprintln!("df: write error: {}", e);
        return 1;
    }

    let _ = out.flush();
    if had_error { 1 } else { 0 }
}
