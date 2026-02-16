use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{self, DirEntry, Metadata};
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// How to sort directory entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Size,
    Time,
    Extension,
    Version,
    None,
    Width,
}

/// Output layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Long,
    SingleColumn,
    Columns,
    Comma,
    Across,
}

/// When to emit ANSI colour escapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Always,
    Auto,
    Never,
}

/// Which timestamp to show / sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeField {
    Mtime,
    Atime,
    Ctime,
    Birth,
}

/// How to format timestamps in long listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeStyle {
    FullIso,
    LongIso,
    Iso,
    Locale,
    Custom(String),
}

/// What indicators to append to names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorStyle {
    None,
    Slash,
    FileType,
    Classify,
}

/// File-type classify mode (for -F / --classify).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifyMode {
    Always,
    Auto,
    Never,
}

/// Quoting style for file names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotingStyle {
    Literal,
    Locale,
    Shell,
    ShellAlways,
    ShellEscape,
    ShellEscapeAlways,
    C,
    Escape,
}

/// When to emit hyperlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyperlinkMode {
    Always,
    Auto,
    Never,
}

/// Full configuration for ls.
#[derive(Debug, Clone)]
pub struct LsConfig {
    pub all: bool,
    pub almost_all: bool,
    pub long_format: bool,
    pub human_readable: bool,
    pub si: bool,
    pub reverse: bool,
    pub recursive: bool,
    pub sort_by: SortBy,
    pub format: OutputFormat,
    pub classify: ClassifyMode,
    pub color: ColorMode,
    pub group_directories_first: bool,
    pub show_inode: bool,
    pub show_size: bool,
    pub show_owner: bool,
    pub show_group: bool,
    pub numeric_ids: bool,
    pub dereference: bool,
    pub directory: bool,
    pub time_field: TimeField,
    pub time_style: TimeStyle,
    pub ignore_patterns: Vec<String>,
    pub ignore_backups: bool,
    pub width: usize,
    pub quoting_style: QuotingStyle,
    pub hide_control_chars: bool,
    pub kibibytes: bool,
    pub indicator_style: IndicatorStyle,
    pub tab_size: usize,
    pub hyperlink: HyperlinkMode,
    pub context: bool,
    pub literal: bool,
}

impl Default for LsConfig {
    fn default() -> Self {
        LsConfig {
            all: false,
            almost_all: false,
            long_format: false,
            human_readable: false,
            si: false,
            reverse: false,
            recursive: false,
            sort_by: SortBy::Name,
            format: OutputFormat::Columns,
            classify: ClassifyMode::Never,
            color: ColorMode::Auto,
            group_directories_first: false,
            show_inode: false,
            show_size: false,
            show_owner: true,
            show_group: true,
            numeric_ids: false,
            dereference: false,
            directory: false,
            time_field: TimeField::Mtime,
            time_style: TimeStyle::Locale,
            ignore_patterns: Vec::new(),
            ignore_backups: false,
            width: 80,
            quoting_style: QuotingStyle::Literal,
            hide_control_chars: false,
            kibibytes: false,
            indicator_style: IndicatorStyle::None,
            tab_size: 8,
            hyperlink: HyperlinkMode::Never,
            context: false,
            literal: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Default LS_COLORS
// ---------------------------------------------------------------------------

/// Parsed colour database.
#[derive(Debug, Clone)]
pub struct ColorDb {
    pub map: HashMap<String, String>,
    pub dir: String,
    pub link: String,
    pub exec: String,
    pub pipe: String,
    pub socket: String,
    pub block_dev: String,
    pub char_dev: String,
    pub orphan: String,
    pub setuid: String,
    pub setgid: String,
    pub sticky: String,
    pub other_writable: String,
    pub sticky_other_writable: String,
    pub reset: String,
}

impl Default for ColorDb {
    fn default() -> Self {
        ColorDb {
            map: HashMap::new(),
            dir: "\x1b[01;34m".to_string(),            // bold blue
            link: "\x1b[01;36m".to_string(),           // bold cyan
            exec: "\x1b[01;32m".to_string(),           // bold green
            pipe: "\x1b[33m".to_string(),              // yellow
            socket: "\x1b[01;35m".to_string(),         // bold magenta
            block_dev: "\x1b[01;33m".to_string(),      // bold yellow
            char_dev: "\x1b[01;33m".to_string(),       // bold yellow
            orphan: "\x1b[01;31m".to_string(),         // bold red
            setuid: "\x1b[37;41m".to_string(),         // white on red
            setgid: "\x1b[30;43m".to_string(),         // black on yellow
            sticky: "\x1b[37;44m".to_string(),         // white on blue
            other_writable: "\x1b[34;42m".to_string(), // blue on green
            sticky_other_writable: "\x1b[30;42m".to_string(), // black on green
            reset: "\x1b[0m".to_string(),
        }
    }
}

impl ColorDb {
    /// Parse from LS_COLORS environment variable.
    pub fn from_env() -> Self {
        let mut db = ColorDb::default();
        if let Ok(val) = std::env::var("LS_COLORS") {
            for item in val.split(':') {
                if let Some((key, code)) = item.split_once('=') {
                    let esc = format!("\x1b[{}m", code);
                    match key {
                        "di" => db.dir = esc,
                        "ln" => db.link = esc,
                        "ex" => db.exec = esc,
                        "pi" | "fi" if key == "pi" => db.pipe = esc,
                        "so" => db.socket = esc,
                        "bd" => db.block_dev = esc,
                        "cd" => db.char_dev = esc,
                        "or" => db.orphan = esc,
                        "su" => db.setuid = esc,
                        "sg" => db.setgid = esc,
                        "st" => db.sticky = esc,
                        "ow" => db.other_writable = esc,
                        "tw" => db.sticky_other_writable = esc,
                        "rs" => db.reset = esc,
                        _ => {
                            if key.starts_with('*') {
                                db.map.insert(key[1..].to_string(), esc);
                            }
                        }
                    }
                }
            }
        }
        db
    }

    /// Look up the colour escape for a file entry.
    fn color_for(&self, entry: &FileEntry) -> &str {
        let mode = entry.mode;
        let ft = mode & (libc::S_IFMT as u32);

        // Symlink
        if ft == libc::S_IFLNK as u32 {
            if entry.link_target_ok {
                return &self.link;
            } else {
                return &self.orphan;
            }
        }

        // Directory with special bits
        if ft == libc::S_IFDIR as u32 {
            let sticky = mode & (libc::S_ISVTX as u32) != 0;
            let ow = mode & (libc::S_IWOTH as u32) != 0;
            if sticky && ow {
                return &self.sticky_other_writable;
            }
            if ow {
                return &self.other_writable;
            }
            if sticky {
                return &self.sticky;
            }
            return &self.dir;
        }

        // Special files
        if ft == libc::S_IFIFO as u32 {
            return &self.pipe;
        }
        if ft == libc::S_IFSOCK as u32 {
            return &self.socket;
        }
        if ft == libc::S_IFBLK as u32 {
            return &self.block_dev;
        }
        if ft == libc::S_IFCHR as u32 {
            return &self.char_dev;
        }

        // Setuid / setgid
        if mode & (libc::S_ISUID as u32) != 0 {
            return &self.setuid;
        }
        if mode & (libc::S_ISGID as u32) != 0 {
            return &self.setgid;
        }

        // Extension match
        if let Some(ext_pos) = entry.name.rfind('.') {
            let ext = &entry.name[ext_pos..];
            if let Some(c) = self.map.get(ext) {
                return c;
            }
        }

        // Executable
        if ft == libc::S_IFREG as u32
            && mode & (libc::S_IXUSR as u32 | libc::S_IXGRP as u32 | libc::S_IXOTH as u32) != 0
        {
            return &self.exec;
        }

        ""
    }
}

// ---------------------------------------------------------------------------
// File entry
// ---------------------------------------------------------------------------

/// One entry to display.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub ino: u64,
    pub nlink: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blocks: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
    pub atime: i64,
    pub atime_nsec: i64,
    pub ctime: i64,
    pub ctime_nsec: i64,
    pub rdev_major: u32,
    pub rdev_minor: u32,
    pub is_dir: bool,
    pub link_target: Option<String>,
    pub link_target_ok: bool,
}

impl FileEntry {
    /// Create from a DirEntry.
    fn from_dir_entry(de: &DirEntry, config: &LsConfig) -> io::Result<Self> {
        let name = de.file_name().to_string_lossy().into_owned();
        let path = de.path();

        let meta = if config.dereference {
            fs::metadata(&path).or_else(|_| fs::symlink_metadata(&path))?
        } else {
            fs::symlink_metadata(&path)?
        };

        Self::from_metadata(name, path, &meta, config)
    }

    /// Create from a path using the full path as the display name (for -d with
    /// arguments, or for the `.` and `..` virtual entries).
    pub fn from_path_with_name(name: String, path: &Path, config: &LsConfig) -> io::Result<Self> {
        let meta = if config.dereference {
            fs::metadata(path).or_else(|_| fs::symlink_metadata(path))?
        } else {
            fs::symlink_metadata(path)?
        };
        Self::from_metadata(name, path.to_path_buf(), &meta, config)
    }

    fn from_metadata(
        name: String,
        path: PathBuf,
        meta: &Metadata,
        _config: &LsConfig,
    ) -> io::Result<Self> {
        let file_type = meta.file_type();
        let is_symlink = file_type.is_symlink();

        let (link_target, link_target_ok) = if is_symlink {
            match fs::read_link(&path) {
                Ok(target) => {
                    let ok = fs::metadata(&path).is_ok();
                    (Some(target.to_string_lossy().into_owned()), ok)
                }
                Err(_) => (None, false),
            }
        } else {
            (None, true)
        };

        let rdev = meta.rdev();

        Ok(FileEntry {
            name,
            path,
            ino: meta.ino(),
            nlink: meta.nlink(),
            mode: meta.mode(),
            uid: meta.uid(),
            gid: meta.gid(),
            size: meta.size(),
            blocks: meta.blocks(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            atime: meta.atime(),
            atime_nsec: meta.atime_nsec(),
            ctime: meta.ctime(),
            ctime_nsec: meta.ctime_nsec(),
            rdev_major: ((rdev >> 8) & 0xfff) as u32,
            rdev_minor: (rdev & 0xff) as u32,
            is_dir: meta.is_dir(),
            link_target,
            link_target_ok,
        })
    }

    /// Get the timestamp for the chosen time field.
    fn time_secs(&self, field: TimeField) -> i64 {
        match field {
            TimeField::Mtime => self.mtime,
            TimeField::Atime => self.atime,
            TimeField::Ctime | TimeField::Birth => self.ctime,
        }
    }

    fn time_nsec(&self, field: TimeField) -> i64 {
        match field {
            TimeField::Mtime => self.mtime_nsec,
            TimeField::Atime => self.atime_nsec,
            TimeField::Ctime | TimeField::Birth => self.ctime_nsec,
        }
    }

    /// Return the extension (lowercase) for sorting.
    fn extension(&self) -> &str {
        match self.name.rfind('.') {
            Some(pos) if pos > 0 => &self.name[pos + 1..],
            _ => "",
        }
    }

    /// Is this a directory (or symlink-to-directory when dereferencing)?
    fn is_directory(&self) -> bool {
        self.is_dir
    }

    /// Indicator character for classify.
    fn indicator(&self, style: IndicatorStyle) -> &'static str {
        let ft = self.mode & (libc::S_IFMT as u32);
        match style {
            IndicatorStyle::None => "",
            IndicatorStyle::Slash => {
                if ft == libc::S_IFDIR as u32 {
                    "/"
                } else {
                    ""
                }
            }
            IndicatorStyle::FileType => match ft {
                x if x == libc::S_IFDIR as u32 => "/",
                x if x == libc::S_IFLNK as u32 => "@",
                x if x == libc::S_IFIFO as u32 => "|",
                x if x == libc::S_IFSOCK as u32 => "=",
                _ => "",
            },
            IndicatorStyle::Classify => match ft {
                x if x == libc::S_IFDIR as u32 => "/",
                x if x == libc::S_IFLNK as u32 => "@",
                x if x == libc::S_IFIFO as u32 => "|",
                x if x == libc::S_IFSOCK as u32 => "=",
                _ => {
                    if ft == libc::S_IFREG as u32
                        && self.mode
                            & (libc::S_IXUSR as u32 | libc::S_IXGRP as u32 | libc::S_IXOTH as u32)
                            != 0
                    {
                        "*"
                    } else {
                        ""
                    }
                }
            },
        }
    }

    /// Display width of the name (accounting for quoting, indicator).
    fn display_width(&self, config: &LsConfig) -> usize {
        let quoted = quote_name(&self.name, config);
        let ind = self.indicator(config.indicator_style);
        quoted.len() + ind.len()
    }
}

// ---------------------------------------------------------------------------
// Name quoting
// ---------------------------------------------------------------------------

/// Quote a filename according to the configured quoting style.
pub fn quote_name(name: &str, config: &LsConfig) -> String {
    match config.quoting_style {
        QuotingStyle::Literal => {
            if config.hide_control_chars {
                hide_control(name)
            } else {
                name.to_string()
            }
        }
        QuotingStyle::Escape => escape_name(name),
        QuotingStyle::C => c_quote(name),
        QuotingStyle::Shell => shell_quote(name, false, false),
        QuotingStyle::ShellAlways => shell_quote(name, true, false),
        QuotingStyle::ShellEscape => shell_quote(name, false, true),
        QuotingStyle::ShellEscapeAlways => shell_quote(name, true, true),
        QuotingStyle::Locale => locale_quote(name),
    }
}

fn hide_control(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn escape_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn c_quote(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for c in name.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0C' => out.push_str("\\f"),
            '\x0B' => out.push_str("\\v"),
            c if c.is_control() => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn shell_quote(name: &str, always: bool, escape: bool) -> String {
    let needs_quoting = name.is_empty()
        || name
            .chars()
            .any(|c| " \t\n'\"\\|&;()<>!$`#~{}[]?*".contains(c) || c.is_control());

    if !needs_quoting && !always {
        return name.to_string();
    }

    if escape {
        // Use $'...' form with escape sequences for control chars
        let has_control = name.chars().any(|c| c.is_control());
        if has_control {
            let mut out = String::with_capacity(name.len() + 4);
            out.push_str("$'");
            for c in name.chars() {
                match c {
                    '\'' => out.push_str("\\'"),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c.is_control() => {
                        out.push_str(&format!("\\{:03o}", c as u32));
                    }
                    c => out.push(c),
                }
            }
            out.push('\'');
            return out;
        }
    }

    // Use single quotes
    let mut out = String::with_capacity(name.len() + 2);
    out.push('\'');
    for c in name.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn locale_quote(name: &str) -> String {
    // Use \u{2018} and \u{2019} (left/right single quotation marks)
    let mut out = String::with_capacity(name.len() + 2);
    out.push('\u{2018}');
    for c in name.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('\u{2019}');
    out
}

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

/// Natural version sort comparison (like GNU `ls -v` / `sort -V`).
pub(crate) fn version_cmp(a: &str, b: &str) -> Ordering {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let mut ai = 0;
    let mut bi = 0;
    while ai < ab.len() && bi < bb.len() {
        let ac = ab[ai];
        let bc = bb[bi];
        if ac.is_ascii_digit() && bc.is_ascii_digit() {
            // Skip leading zeros
            let a_start = ai;
            let b_start = bi;
            while ai < ab.len() && ab[ai] == b'0' {
                ai += 1;
            }
            while bi < bb.len() && bb[bi] == b'0' {
                bi += 1;
            }
            let a_num_start = ai;
            let b_num_start = bi;
            while ai < ab.len() && ab[ai].is_ascii_digit() {
                ai += 1;
            }
            while bi < bb.len() && bb[bi].is_ascii_digit() {
                bi += 1;
            }
            let a_len = ai - a_num_start;
            let b_len = bi - b_num_start;
            if a_len != b_len {
                return a_len.cmp(&b_len);
            }
            let ord = ab[a_num_start..ai].cmp(&bb[b_num_start..bi]);
            if ord != Ordering::Equal {
                return ord;
            }
            // If numeric parts are equal, fewer leading zeros comes first
            let a_zeros = a_num_start - a_start;
            let b_zeros = b_num_start - b_start;
            if a_zeros != b_zeros {
                return a_zeros.cmp(&b_zeros);
            }
        } else {
            let ord = ac.cmp(&bc);
            if ord != Ordering::Equal {
                return ord;
            }
            ai += 1;
            bi += 1;
        }
    }
    ab.len().cmp(&bb.len())
}

fn sort_entries(entries: &mut [FileEntry], config: &LsConfig) {
    if config.group_directories_first {
        // Stable sort: directories first, then sort within each group
        entries.sort_by(|a, b| {
            let a_dir = a.is_directory();
            let b_dir = b.is_directory();
            match (a_dir, b_dir) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => compare_entries(a, b, config),
            }
        });
    } else {
        entries.sort_by(|a, b| compare_entries(a, b, config));
    }
}

fn compare_entries(a: &FileEntry, b: &FileEntry, config: &LsConfig) -> Ordering {
    let ord = match config.sort_by {
        SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortBy::Size => {
            let size_ord = b.size.cmp(&a.size);
            if size_ord == Ordering::Equal {
                // GNU uses name as tie-breaker for equal sizes.
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            } else {
                size_ord
            }
        }
        SortBy::Time => {
            let ta = a.time_secs(config.time_field);
            let tb = b.time_secs(config.time_field);
            let ord = tb.cmp(&ta);
            if ord == Ordering::Equal {
                let na = a.time_nsec(config.time_field);
                let nb = b.time_nsec(config.time_field);
                let nsec_ord = nb.cmp(&na);
                if nsec_ord == Ordering::Equal {
                    // GNU uses name as tie-breaker for equal times.
                    a.name.to_lowercase().cmp(&b.name.to_lowercase())
                } else {
                    nsec_ord
                }
            } else {
                ord
            }
        }
        SortBy::Extension => {
            let ea = a.extension().to_lowercase();
            let eb = b.extension().to_lowercase();
            let ord = ea.cmp(&eb);
            if ord == Ordering::Equal {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            } else {
                ord
            }
        }
        SortBy::Version => version_cmp(&a.name, &b.name),
        SortBy::None => Ordering::Equal,
        SortBy::Width => {
            let wa = a.display_width(config);
            let wb = b.display_width(config);
            wa.cmp(&wb)
        }
    };

    if config.reverse { ord.reverse() } else { ord }
}

// ---------------------------------------------------------------------------
// Permission formatting
// ---------------------------------------------------------------------------

/// Format permission bits as `drwxr-xr-x` (10 chars).
pub fn format_permissions(mode: u32) -> String {
    let mut s = String::with_capacity(10);

    // File type character
    s.push(match mode & (libc::S_IFMT as u32) {
        x if x == libc::S_IFDIR as u32 => 'd',
        x if x == libc::S_IFLNK as u32 => 'l',
        x if x == libc::S_IFBLK as u32 => 'b',
        x if x == libc::S_IFCHR as u32 => 'c',
        x if x == libc::S_IFIFO as u32 => 'p',
        x if x == libc::S_IFSOCK as u32 => 's',
        _ => '-',
    });

    // User
    s.push(if mode & (libc::S_IRUSR as u32) != 0 {
        'r'
    } else {
        '-'
    });
    s.push(if mode & (libc::S_IWUSR as u32) != 0 {
        'w'
    } else {
        '-'
    });
    s.push(if mode & (libc::S_ISUID as u32) != 0 {
        if mode & (libc::S_IXUSR as u32) != 0 {
            's'
        } else {
            'S'
        }
    } else if mode & (libc::S_IXUSR as u32) != 0 {
        'x'
    } else {
        '-'
    });

    // Group
    s.push(if mode & (libc::S_IRGRP as u32) != 0 {
        'r'
    } else {
        '-'
    });
    s.push(if mode & (libc::S_IWGRP as u32) != 0 {
        'w'
    } else {
        '-'
    });
    s.push(if mode & (libc::S_ISGID as u32) != 0 {
        if mode & (libc::S_IXGRP as u32) != 0 {
            's'
        } else {
            'S'
        }
    } else if mode & (libc::S_IXGRP as u32) != 0 {
        'x'
    } else {
        '-'
    });

    // Other
    s.push(if mode & (libc::S_IROTH as u32) != 0 {
        'r'
    } else {
        '-'
    });
    s.push(if mode & (libc::S_IWOTH as u32) != 0 {
        'w'
    } else {
        '-'
    });
    s.push(if mode & (libc::S_ISVTX as u32) != 0 {
        if mode & (libc::S_IXOTH as u32) != 0 {
            't'
        } else {
            'T'
        }
    } else if mode & (libc::S_IXOTH as u32) != 0 {
        'x'
    } else {
        '-'
    });

    s
}

// ---------------------------------------------------------------------------
// Size formatting
// ---------------------------------------------------------------------------

/// Format a file size for display.
pub fn format_size(size: u64, human: bool, si: bool, kibibytes: bool) -> String {
    if human || si {
        let base: f64 = if si { 1000.0 } else { 1024.0 };
        let suffixes = ["", "K", "M", "G", "T", "P", "E"];

        if size == 0 {
            return "0".to_string();
        }

        let mut val = size as f64;
        let mut idx = 0;
        while val >= base && idx < suffixes.len() - 1 {
            val /= base;
            idx += 1;
        }

        if idx == 0 {
            format!("{}", size)
        } else if val >= 10.0 {
            format!("{:.0}{}", val, suffixes[idx])
        } else {
            format!("{:.1}{}", val, suffixes[idx])
        }
    } else if kibibytes {
        // Show blocks in 1K units
        let blocks_k = (size + 1023) / 1024;
        format!("{}", blocks_k)
    } else {
        format!("{}", size)
    }
}

/// Format blocks for the -s option (in 1K units by default, or --si / -h).
pub fn format_blocks(blocks_512: u64, human: bool, si: bool, kibibytes: bool) -> String {
    let bytes = blocks_512 * 512;
    if human || si {
        format_size(bytes, human, si, false)
    } else if kibibytes {
        let k = (bytes + 1023) / 1024;
        format!("{}", k)
    } else {
        // Default: 1K blocks
        let k = (bytes + 1023) / 1024;
        format!("{}", k)
    }
}

// ---------------------------------------------------------------------------
// Timestamp formatting
// ---------------------------------------------------------------------------

/// Format a unix timestamp for long listing.
pub fn format_time(secs: i64, nsec: i64, style: &TimeStyle) -> String {
    // Convert to SystemTime for the six-months-ago check
    let now_sys = SystemTime::now();
    let now_secs = now_sys
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let six_months_ago = now_secs - 6 * 30 * 24 * 3600;

    // Break down the timestamp
    let tm = time_from_epoch(secs);

    match style {
        TimeStyle::FullIso => {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09} {}",
                tm.year,
                tm.month,
                tm.day,
                tm.hour,
                tm.min,
                tm.sec,
                nsec,
                format_tz_offset(tm.utc_offset_secs)
            )
        }
        TimeStyle::LongIso => {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                tm.year, tm.month, tm.day, tm.hour, tm.min
            )
        }
        TimeStyle::Iso => {
            if secs > six_months_ago && secs <= now_secs {
                format!("{:02}-{:02} {:02}:{:02}", tm.month, tm.day, tm.hour, tm.min)
            } else {
                format!("{:02}-{:02}  {:04}", tm.month, tm.day, tm.year)
            }
        }
        TimeStyle::Locale | TimeStyle::Custom(_) => {
            let month_names = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let mon = if tm.month >= 1 && tm.month <= 12 {
                month_names[(tm.month - 1) as usize]
            } else {
                "???"
            };

            if secs > six_months_ago && secs <= now_secs {
                format!("{} {:>2} {:02}:{:02}", mon, tm.day, tm.hour, tm.min)
            } else {
                format!("{} {:>2}  {:04}", mon, tm.day, tm.year)
            }
        }
    }
}

fn format_tz_offset(offset_secs: i32) -> String {
    let sign = if offset_secs >= 0 { '+' } else { '-' };
    let abs = offset_secs.unsigned_abs();
    let hours = abs / 3600;
    let mins = (abs % 3600) / 60;
    format!("{}{:02}{:02}", sign, hours, mins)
}

struct BrokenDownTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
    utc_offset_secs: i32,
}

/// Convert epoch seconds to broken-down local time using libc::localtime_r.
fn time_from_epoch(secs: i64) -> BrokenDownTime {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time_t = secs as libc::time_t;
    unsafe {
        libc::localtime_r(&time_t, &mut tm);
    }
    BrokenDownTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        min: tm.tm_min as u32,
        sec: tm.tm_sec as u32,
        utc_offset_secs: tm.tm_gmtoff as i32,
    }
}

// ---------------------------------------------------------------------------
// User/group name lookup
// ---------------------------------------------------------------------------

/// Look up a username by UID. Returns numeric string on failure.
fn lookup_user(uid: u32) -> String {
    // Use libc::getpwuid_r for thread safety
    let mut buf = vec![0u8; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if ret == 0 && !result.is_null() {
        let cstr = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
        cstr.to_string_lossy().into_owned()
    } else {
        uid.to_string()
    }
}

/// Look up a group name by GID. Returns numeric string on failure.
fn lookup_group(gid: u32) -> String {
    let mut buf = vec![0u8; 1024];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    let ret = unsafe {
        libc::getgrgid_r(
            gid,
            &mut grp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if ret == 0 && !result.is_null() {
        let cstr = unsafe { std::ffi::CStr::from_ptr(grp.gr_name) };
        cstr.to_string_lossy().into_owned()
    } else {
        gid.to_string()
    }
}

// ---------------------------------------------------------------------------
// Pattern matching (for --ignore)
// ---------------------------------------------------------------------------

/// Simple glob matching (supports * and ?).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let pat = pattern.as_bytes();
    let txt = name.as_bytes();
    let mut pi = 0;
    let mut ti = 0;
    let mut star_p = usize::MAX;
    let mut star_t = 0;

    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_p = pi;
            star_t = ti;
            pi += 1;
        } else if star_p != usize::MAX {
            pi = star_p + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

fn should_ignore(name: &str, config: &LsConfig) -> bool {
    if config.ignore_backups && name.ends_with('~') {
        return true;
    }
    for pat in &config.ignore_patterns {
        if glob_match(pat, name) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Reading directory entries
// ---------------------------------------------------------------------------

/// Read entries from a directory path.
pub fn read_entries(path: &Path, config: &LsConfig) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    if config.all {
        // Add . and ..
        if let Ok(e) = FileEntry::from_path_with_name(".".to_string(), path, config) {
            entries.push(e);
        }
        let parent = path.parent().unwrap_or(path);
        if let Ok(e) = FileEntry::from_path_with_name("..".to_string(), parent, config) {
            entries.push(e);
        }
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();

        // Filter hidden files
        if !config.all && !config.almost_all && name.starts_with('.') {
            continue;
        }
        if config.almost_all && (name == "." || name == "..") {
            continue;
        }

        // Filter ignored patterns
        if should_ignore(&name, config) {
            continue;
        }

        match FileEntry::from_dir_entry(&entry, config) {
            Ok(fe) => entries.push(fe),
            Err(e) => {
                eprintln!("ls: cannot access '{}': {}", entry.path().display(), e);
            }
        }
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Long format output
// ---------------------------------------------------------------------------

/// Print entries in long format to the writer.
fn print_long(
    out: &mut impl Write,
    entries: &[FileEntry],
    config: &LsConfig,
    color_db: Option<&ColorDb>,
) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    // Calculate column widths for alignment
    let max_nlink = entries
        .iter()
        .map(|e| count_digits(e.nlink))
        .max()
        .unwrap_or(1);
    let max_owner = if config.show_owner {
        entries
            .iter()
            .map(|e| {
                if config.numeric_ids {
                    e.uid.to_string().len()
                } else {
                    lookup_user(e.uid).len()
                }
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let max_group = if config.show_group {
        entries
            .iter()
            .map(|e| {
                if config.numeric_ids {
                    e.gid.to_string().len()
                } else {
                    lookup_group(e.gid).len()
                }
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };

    // Size width: use the formatted size for human-readable, else raw digits
    let has_device = entries.iter().any(|e| {
        let ft = e.mode & (libc::S_IFMT as u32);
        ft == libc::S_IFBLK as u32 || ft == libc::S_IFCHR as u32
    });
    let max_size = if has_device {
        // For device files, need room for "major, minor"
        entries
            .iter()
            .map(|e| {
                let ft = e.mode & (libc::S_IFMT as u32);
                if ft == libc::S_IFBLK as u32 || ft == libc::S_IFCHR as u32 {
                    format!("{}, {}", e.rdev_major, e.rdev_minor).len()
                } else {
                    format_size(e.size, config.human_readable, config.si, config.kibibytes).len()
                }
            })
            .max()
            .unwrap_or(1)
    } else {
        entries
            .iter()
            .map(|e| format_size(e.size, config.human_readable, config.si, config.kibibytes).len())
            .max()
            .unwrap_or(1)
    };

    let max_inode = if config.show_inode {
        entries
            .iter()
            .map(|e| count_digits(e.ino))
            .max()
            .unwrap_or(1)
    } else {
        0
    };

    let max_blocks = if config.show_size {
        entries
            .iter()
            .map(|e| {
                format_blocks(e.blocks, config.human_readable, config.si, config.kibibytes).len()
            })
            .max()
            .unwrap_or(1)
    } else {
        0
    };

    for entry in entries {
        // Inode
        if config.show_inode {
            write!(out, "{:>width$} ", entry.ino, width = max_inode)?;
        }

        // Block size
        if config.show_size {
            let bs = format_blocks(
                entry.blocks,
                config.human_readable,
                config.si,
                config.kibibytes,
            );
            write!(out, "{:>width$} ", bs, width = max_blocks)?;
        }

        // Permissions
        write!(out, "{} ", format_permissions(entry.mode))?;

        // Hard link count
        write!(out, "{:>width$} ", entry.nlink, width = max_nlink)?;

        // Owner
        if config.show_owner {
            let owner = if config.numeric_ids {
                entry.uid.to_string()
            } else {
                lookup_user(entry.uid)
            };
            write!(out, "{:<width$} ", owner, width = max_owner)?;
        }

        // Group
        if config.show_group {
            let group = if config.numeric_ids {
                entry.gid.to_string()
            } else {
                lookup_group(entry.gid)
            };
            write!(out, "{:<width$} ", group, width = max_group)?;
        }

        // Size or device numbers
        let ft = entry.mode & (libc::S_IFMT as u32);
        if ft == libc::S_IFBLK as u32 || ft == libc::S_IFCHR as u32 {
            let dev = format!("{}, {}", entry.rdev_major, entry.rdev_minor);
            write!(out, "{:>width$} ", dev, width = max_size)?;
        } else {
            let sz = format_size(
                entry.size,
                config.human_readable,
                config.si,
                config.kibibytes,
            );
            write!(out, "{:>width$} ", sz, width = max_size)?;
        }

        // Timestamp
        let ts = format_time(
            entry.time_secs(config.time_field),
            entry.time_nsec(config.time_field),
            &config.time_style,
        );
        write!(out, "{} ", ts)?;

        // Name (with colour)
        let quoted = quote_name(&entry.name, config);
        if let Some(db) = color_db {
            let c = db.color_for(entry);
            if c.is_empty() {
                write!(out, "{}", quoted)?;
            } else {
                write!(out, "{}{}{}", c, quoted, db.reset)?;
            }
        } else {
            write!(out, "{}", quoted)?;
        }

        // Indicator
        let ind = entry.indicator(config.indicator_style);
        if !ind.is_empty() {
            write!(out, "{}", ind)?;
        }

        // Symlink target
        if let Some(ref target) = entry.link_target {
            write!(out, " -> {}", target)?;
        }

        writeln!(out)?;
    }

    Ok(())
}

fn count_digits(n: u64) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut v = n;
    while v > 0 {
        count += 1;
        v /= 10;
    }
    count
}

// ---------------------------------------------------------------------------
// Column format output
// ---------------------------------------------------------------------------

/// Print entries in multi-column format.
fn print_columns(
    out: &mut impl Write,
    entries: &[FileEntry],
    config: &LsConfig,
    color_db: Option<&ColorDb>,
) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let term_width = config.width;
    let tab = config.tab_size;

    // Collect display names and their display widths
    let items: Vec<(String, usize, &FileEntry)> = entries
        .iter()
        .map(|e| {
            let quoted = quote_name(&e.name, config);
            let ind = e.indicator(config.indicator_style);
            let display = format!("{}{}", quoted, ind);
            let w = display.len();
            (display, w, e)
        })
        .collect();

    let max_inode_w = if config.show_inode {
        entries
            .iter()
            .map(|e| count_digits(e.ino))
            .max()
            .unwrap_or(1)
    } else {
        0
    };
    let max_blocks_w = if config.show_size {
        entries
            .iter()
            .map(|e| {
                format_blocks(e.blocks, config.human_readable, config.si, config.kibibytes).len()
            })
            .max()
            .unwrap_or(1)
    } else {
        0
    };

    let prefix_width = if config.show_inode && config.show_size {
        max_inode_w + 1 + max_blocks_w + 1
    } else if config.show_inode {
        max_inode_w + 1
    } else if config.show_size {
        max_blocks_w + 1
    } else {
        0
    };

    // Try to fit into columns
    let n = items.len();
    let max_name_width = items.iter().map(|(_, w, _)| *w).max().unwrap_or(0);
    let col_width_raw = max_name_width + prefix_width;

    // Round up to next tab stop
    let col_width = if tab > 0 {
        ((col_width_raw + tab) / tab) * tab
    } else {
        col_width_raw + 2
    };

    if col_width == 0 || col_width >= term_width {
        // Fall back to single column
        return print_single_column(out, entries, config, color_db);
    }

    let num_cols = std::cmp::max(1, term_width / col_width);
    let num_rows = (n + num_cols - 1) / num_cols;

    for row in 0..num_rows {
        let mut col = 0;
        loop {
            let idx = col * num_rows + row;
            if idx >= n {
                break;
            }

            let (ref display, w, entry) = items[idx];
            let is_last_col = col + 1 >= num_cols || (col + 1) * num_rows + row >= n;

            // inode prefix
            if config.show_inode {
                write!(out, "{:>width$} ", entry.ino, width = max_inode_w)?;
            }
            // blocks prefix
            if config.show_size {
                let bs = format_blocks(
                    entry.blocks,
                    config.human_readable,
                    config.si,
                    config.kibibytes,
                );
                write!(out, "{:>width$} ", bs, width = max_blocks_w)?;
            }

            // Name with colour
            if let Some(db) = color_db {
                let c = db.color_for(entry);
                let quoted = quote_name(&entry.name, config);
                let ind = entry.indicator(config.indicator_style);
                if c.is_empty() {
                    write!(out, "{}{}", quoted, ind)?;
                } else {
                    write!(out, "{}{}{}{}", c, quoted, db.reset, ind)?;
                }
            } else {
                write!(out, "{}", display)?;
            }

            if !is_last_col {
                // Pad to column width
                let name_w = w + prefix_width;
                let padding = if col_width > name_w {
                    col_width - name_w
                } else {
                    2
                };
                for _ in 0..padding {
                    write!(out, " ")?;
                }
            }

            col += 1;
        }
        writeln!(out)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Single column output
// ---------------------------------------------------------------------------

fn print_single_column(
    out: &mut impl Write,
    entries: &[FileEntry],
    config: &LsConfig,
    color_db: Option<&ColorDb>,
) -> io::Result<()> {
    let max_inode_w = if config.show_inode {
        entries
            .iter()
            .map(|e| count_digits(e.ino))
            .max()
            .unwrap_or(1)
    } else {
        0
    };
    let max_blocks_w = if config.show_size {
        entries
            .iter()
            .map(|e| {
                format_blocks(e.blocks, config.human_readable, config.si, config.kibibytes).len()
            })
            .max()
            .unwrap_or(1)
    } else {
        0
    };

    for entry in entries {
        if config.show_inode {
            write!(out, "{:>width$} ", entry.ino, width = max_inode_w)?;
        }
        if config.show_size {
            let bs = format_blocks(
                entry.blocks,
                config.human_readable,
                config.si,
                config.kibibytes,
            );
            write!(out, "{:>width$} ", bs, width = max_blocks_w)?;
        }

        let quoted = quote_name(&entry.name, config);
        if let Some(db) = color_db {
            let c = db.color_for(entry);
            if c.is_empty() {
                write!(out, "{}", quoted)?;
            } else {
                write!(out, "{}{}{}", c, quoted, db.reset)?;
            }
        } else {
            write!(out, "{}", quoted)?;
        }

        let ind = entry.indicator(config.indicator_style);
        if !ind.is_empty() {
            write!(out, "{}", ind)?;
        }

        writeln!(out)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Comma-separated output
// ---------------------------------------------------------------------------

pub fn print_comma(
    out: &mut impl Write,
    entries: &[FileEntry],
    config: &LsConfig,
    color_db: Option<&ColorDb>,
) -> io::Result<()> {
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            write!(out, ", ")?;
        }
        let quoted = quote_name(&entry.name, config);
        if let Some(db) = color_db {
            let c = db.color_for(entry);
            if c.is_empty() {
                write!(out, "{}", quoted)?;
            } else {
                write!(out, "{}{}{}", c, quoted, db.reset)?;
            }
        } else {
            write!(out, "{}", quoted)?;
        }
        let ind = entry.indicator(config.indicator_style);
        if !ind.is_empty() {
            write!(out, "{}", ind)?;
        }
    }
    if !entries.is_empty() {
        writeln!(out)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Total blocks line
// ---------------------------------------------------------------------------

fn print_total(out: &mut impl Write, entries: &[FileEntry], config: &LsConfig) -> io::Result<()> {
    let total_blocks: u64 = entries.iter().map(|e| e.blocks).sum();
    let formatted = format_blocks(
        total_blocks,
        config.human_readable,
        config.si,
        config.kibibytes,
    );
    writeln!(out, "total {}", formatted)
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// List a single directory to the provided writer.
pub fn ls_dir(
    out: &mut impl Write,
    path: &Path,
    config: &LsConfig,
    color_db: Option<&ColorDb>,
    show_header: bool,
) -> io::Result<bool> {
    if show_header {
        writeln!(out, "{}:", path.display())?;
    }

    let mut entries = read_entries(path, config)?;
    sort_entries(&mut entries, config);

    // Print total in long / show_size modes
    if config.long_format || config.show_size {
        print_total(out, &entries, config)?;
    }

    match config.format {
        OutputFormat::Long => print_long(out, &entries, config, color_db)?,
        OutputFormat::SingleColumn => print_single_column(out, &entries, config, color_db)?,
        OutputFormat::Columns | OutputFormat::Across => {
            print_columns(out, &entries, config, color_db)?
        }
        OutputFormat::Comma => print_comma(out, &entries, config, color_db)?,
    }

    // Recursive
    if config.recursive {
        let dirs: Vec<PathBuf> = entries
            .iter()
            .filter(|e| {
                e.is_directory()
                    && e.name != "."
                    && e.name != ".."
                    && (e.mode & (libc::S_IFMT as u32)) != libc::S_IFLNK as u32
            })
            .map(|e| e.path.clone())
            .collect();

        for dir in dirs {
            writeln!(out)?;
            ls_dir(out, &dir, config, color_db, true)?;
        }
    }

    Ok(true)
}

/// Top-level entry: list the given paths.
///
/// Returns `true` if all operations succeeded.
pub fn ls_main(paths: &[String], config: &LsConfig) -> io::Result<bool> {
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(64 * 1024, stdout.lock());

    let color_db = match config.color {
        ColorMode::Always => Some(ColorDb::from_env()),
        ColorMode::Auto => {
            if atty_stdout() {
                Some(ColorDb::from_env())
            } else {
                None
            }
        }
        ColorMode::Never => None,
    };

    let mut had_error = false;

    // Separate files and directories
    let mut file_args: Vec<FileEntry> = Vec::new();
    let mut dir_args: Vec<PathBuf> = Vec::new();

    for p in paths {
        let path = PathBuf::from(p);
        let meta_result = if config.dereference {
            fs::metadata(&path).or_else(|_| fs::symlink_metadata(&path))
        } else {
            fs::symlink_metadata(&path)
        };

        match meta_result {
            Ok(meta) => {
                if config.directory || !meta.is_dir() {
                    match FileEntry::from_path_with_name(p.to_string(), &path, config) {
                        Ok(fe) => file_args.push(fe),
                        Err(e) => {
                            eprintln!("ls: cannot access '{}': {}", p, e);
                            had_error = true;
                        }
                    }
                } else {
                    dir_args.push(path);
                }
            }
            Err(e) => {
                eprintln!(
                    "ls: cannot access '{}': {}",
                    p,
                    crate::common::io_error_msg(&e)
                );
                had_error = true;
            }
        }
    }

    // Sort file args
    sort_entries(&mut file_args, config);

    // Print file arguments
    if !file_args.is_empty() {
        match config.format {
            OutputFormat::Long => print_long(&mut out, &file_args, config, color_db.as_ref())?,
            OutputFormat::SingleColumn => {
                print_single_column(&mut out, &file_args, config, color_db.as_ref())?
            }
            OutputFormat::Columns | OutputFormat::Across => {
                print_columns(&mut out, &file_args, config, color_db.as_ref())?
            }
            OutputFormat::Comma => print_comma(&mut out, &file_args, config, color_db.as_ref())?,
        }
    }

    // Sort directory args by name
    dir_args.sort_by(|a, b| {
        let an = a.to_string_lossy().to_lowercase();
        let bn = b.to_string_lossy().to_lowercase();
        let ord = an.cmp(&bn);
        if config.reverse { ord.reverse() } else { ord }
    });

    let show_header =
        dir_args.len() > 1 || (!file_args.is_empty() && !dir_args.is_empty()) || config.recursive;

    for (i, dir) in dir_args.iter().enumerate() {
        if i > 0 || !file_args.is_empty() {
            writeln!(out)?;
        }
        match ls_dir(&mut out, dir, config, color_db.as_ref(), show_header) {
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "ls: cannot open directory '{}': {}",
                    dir.display(),
                    crate::common::io_error_msg(&e)
                );
                had_error = true;
            }
        }
    }

    out.flush()?;

    Ok(!had_error)
}

/// Check if stdout is a TTY.
pub fn atty_stdout() -> bool {
    unsafe { libc::isatty(1) != 0 }
}

// ---------------------------------------------------------------------------
// Testable helpers (exported for tests module)
// ---------------------------------------------------------------------------

/// Collect entries for a directory into a Vec (for testing).
pub fn collect_entries(path: &Path, config: &LsConfig) -> io::Result<Vec<FileEntry>> {
    let mut entries = read_entries(path, config)?;
    sort_entries(&mut entries, config);
    Ok(entries)
}

/// Render long format lines to a String (for testing).
pub fn render_long(entries: &[FileEntry], config: &LsConfig) -> io::Result<String> {
    let mut buf = Vec::new();
    print_long(&mut buf, entries, config, None)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Render single-column output to a String (for testing).
pub fn render_single_column(entries: &[FileEntry], config: &LsConfig) -> io::Result<String> {
    let mut buf = Vec::new();
    print_single_column(&mut buf, entries, config, None)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Render full ls_dir output to a String (for testing).
pub fn render_dir(path: &Path, config: &LsConfig) -> io::Result<String> {
    let mut buf = Vec::new();
    ls_dir(&mut buf, path, config, None, false)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
