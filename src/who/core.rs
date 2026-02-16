/// who — show who is logged on
///
/// Reads utmpx records and displays information about currently logged-in users,
/// boot time, dead processes, run level, etc.

use std::ffi::CStr;
use std::fmt::Write as FmtWrite;

// utmpx entry type constants (from utmpx.h)
const RUN_LVL: i16 = 1;
const BOOT_TIME: i16 = 2;
const NEW_TIME: i16 = 3;
const OLD_TIME: i16 = 4;
const INIT_PROCESS: i16 = 5;
const LOGIN_PROCESS: i16 = 6;
const USER_PROCESS: i16 = 7;
const DEAD_PROCESS: i16 = 8;

/// A decoded utmpx entry.
#[derive(Clone, Debug)]
pub struct UtmpxEntry {
    pub ut_type: i16,
    pub ut_pid: i32,
    pub ut_line: String,
    pub ut_id: String,
    pub ut_user: String,
    pub ut_host: String,
    pub ut_tv_sec: i64,
}

/// Read all utmpx entries from the system database.
///
/// # Safety
/// Uses libc's setutxent/getutxent/endutxent which are not thread-safe.
/// This function must not be called concurrently.
pub fn read_utmpx() -> Vec<UtmpxEntry> {
    let mut entries = Vec::new();

    unsafe {
        libc::setutxent();
        loop {
            let entry = libc::getutxent();
            if entry.is_null() {
                break;
            }
            let e = &*entry;

            let user = cstr_from_buf(&e.ut_user);
            let line = cstr_from_buf(&e.ut_line);
            let host = cstr_from_buf(&e.ut_host);
            let id = cstr_from_buf(&e.ut_id);

            let tv_sec = e.ut_tv.tv_sec as i64;

            entries.push(UtmpxEntry {
                ut_type: e.ut_type as i16,
                ut_pid: e.ut_pid,
                ut_line: line,
                ut_id: id,
                ut_user: user,
                ut_host: host,
                ut_tv_sec: tv_sec,
            });
        }
        libc::endutxent();
    }

    entries
}

/// Extract a Rust String from a fixed-size C char buffer.
unsafe fn cstr_from_buf(buf: &[libc::c_char]) -> String {
    // Find the first NUL byte or use the entire buffer length
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let bytes: Vec<u8> = buf[..len].iter().map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Configuration for the who command, derived from CLI flags.
#[derive(Clone, Debug, Default)]
pub struct WhoConfig {
    pub show_boot: bool,
    pub show_dead: bool,
    pub show_heading: bool,
    pub show_login: bool,
    pub only_current: bool, // -m
    pub show_init_spawn: bool, // -p
    pub show_count: bool, // -q
    pub show_runlevel: bool, // -r
    pub short_format: bool, // -s (default)
    pub show_clock_change: bool, // -t
    pub show_mesg: bool, // -T, -w
    pub show_users: bool, // -u
    pub show_all: bool, // -a
    pub show_ips: bool, // --ips
    pub show_lookup: bool, // --lookup
    pub am_i: bool, // "who am i"
}

impl WhoConfig {
    /// Apply the --all flag: equivalent to -b -d --login -p -r -t -T -u.
    pub fn apply_all(&mut self) {
        self.show_boot = true;
        self.show_dead = true;
        self.show_login = true;
        self.show_init_spawn = true;
        self.show_runlevel = true;
        self.show_clock_change = true;
        self.show_mesg = true;
        self.show_users = true;
    }

    /// Returns true if no specific filter flags are set,
    /// meaning only USER_PROCESS entries should be shown (default behavior).
    pub fn is_default_filter(&self) -> bool {
        !self.show_boot
            && !self.show_dead
            && !self.show_login
            && !self.show_init_spawn
            && !self.show_runlevel
            && !self.show_clock_change
            && !self.show_users
    }
}

/// Format a Unix timestamp as "YYYY-MM-DD HH:MM".
pub fn format_time(tv_sec: i64) -> String {
    if tv_sec == 0 {
        return String::new();
    }
    let t = tv_sec as libc::time_t;
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
    )
}

/// Determine the message status character for a terminal line.
/// '+' means writable (mesg y), '-' means not writable (mesg n), '?' means unknown.
fn mesg_status(line: &str) -> char {
    if line.is_empty() {
        return '?';
    }
    let dev_path = if line.starts_with('/') {
        line.to_string()
    } else {
        format!("/dev/{}", line)
    };

    let mut stat_buf: libc::stat = unsafe { std::mem::zeroed() };
    let c_path = std::ffi::CString::new(dev_path).unwrap_or_default();
    let rc = unsafe { libc::stat(c_path.as_ptr(), &mut stat_buf) };
    if rc != 0 {
        return '?';
    }
    if stat_buf.st_mode & libc::S_IWGRP != 0 {
        '+'
    } else {
        '-'
    }
}

/// Compute idle time string for a terminal.
/// Returns "." if active within the last minute, "old" if more than 24h,
/// or "HH:MM" otherwise.
fn idle_str(line: &str) -> String {
    if line.is_empty() {
        return "?".to_string();
    }
    let dev_path = if line.starts_with('/') {
        line.to_string()
    } else {
        format!("/dev/{}", line)
    };

    let mut stat_buf: libc::stat = unsafe { std::mem::zeroed() };
    let c_path = std::ffi::CString::new(dev_path).unwrap_or_default();
    let rc = unsafe { libc::stat(c_path.as_ptr(), &mut stat_buf) };
    if rc != 0 {
        return "?".to_string();
    }

    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let atime = stat_buf.st_atime;
    let idle_secs = now - atime;

    if idle_secs < 60 {
        ".".to_string()
    } else if idle_secs >= 86400 {
        "old".to_string()
    } else {
        let hours = idle_secs / 3600;
        let mins = (idle_secs % 3600) / 60;
        format!("{:02}:{:02}", hours, mins)
    }
}

/// Get the terminal device for the current process (for "who am i" / -m).
pub fn current_tty() -> Option<String> {
    unsafe {
        let name = libc::ttyname(0); // stdin
        if name.is_null() {
            None
        } else {
            let s = CStr::from_ptr(name).to_string_lossy().into_owned();
            // Strip /dev/ prefix to match utmpx ut_line
            Some(s.strip_prefix("/dev/").unwrap_or(&s).to_string())
        }
    }
}

/// Check if an entry should be displayed given the config.
pub fn should_show(entry: &UtmpxEntry, config: &WhoConfig) -> bool {
    if config.am_i || config.only_current {
        // Only show entries matching the current terminal
        if let Some(tty) = current_tty() {
            return entry.ut_type == USER_PROCESS && entry.ut_line == tty;
        }
        return false;
    }

    if config.show_count {
        return entry.ut_type == USER_PROCESS;
    }

    if config.is_default_filter() {
        return entry.ut_type == USER_PROCESS;
    }

    match entry.ut_type {
        BOOT_TIME => config.show_boot,
        DEAD_PROCESS => config.show_dead,
        LOGIN_PROCESS => config.show_login,
        INIT_PROCESS => config.show_init_spawn,
        RUN_LVL => config.show_runlevel,
        NEW_TIME | OLD_TIME => config.show_clock_change,
        USER_PROCESS => config.show_users || config.is_default_filter(),
        _ => false,
    }
}

/// Format a single utmpx entry as an output line.
pub fn format_entry(entry: &UtmpxEntry, config: &WhoConfig) -> String {
    let mut out = String::new();

    let name = match entry.ut_type {
        BOOT_TIME => "system boot".to_string(),
        RUN_LVL => "run-level".to_string(),
        NEW_TIME | OLD_TIME => {
            if entry.ut_type == NEW_TIME {
                "new-time".to_string()
            } else {
                "old-time".to_string()
            }
        }
        _ => entry.ut_user.clone(),
    };

    // NAME column (left-aligned, 8 chars min)
    let _ = write!(out, "{:<8}", name);

    // Mesg status column
    if config.show_mesg {
        let status = if entry.ut_type == USER_PROCESS {
            mesg_status(&entry.ut_line)
        } else {
            '?'
        };
        let _ = write!(out, " {}", status);
    }

    // LINE column
    let _ = write!(out, " {:<12}", entry.ut_line);

    // TIME column
    let time_str = format_time(entry.ut_tv_sec);
    let _ = write!(out, " {}", time_str);

    // IDLE + PID for -u
    if config.show_users || config.show_all {
        if entry.ut_type == USER_PROCESS {
            let idle = idle_str(&entry.ut_line);
            let _ = write!(out, " {:>5}", idle);
            let _ = write!(out, " {:>5}", entry.ut_pid);
        } else if entry.ut_type == DEAD_PROCESS {
            let _ = write!(out, "       {:>5}", entry.ut_pid);
        }
    }

    // COMMENT (host) column
    if !entry.ut_host.is_empty() {
        if config.show_ips {
            let _ = write!(out, " ({})", entry.ut_host);
        } else if config.show_lookup {
            // Attempt DNS lookup
            let resolved = lookup_host(&entry.ut_host);
            let _ = write!(out, " ({})", resolved);
        } else {
            let _ = write!(out, " ({})", entry.ut_host);
        }
    }

    out
}

/// Attempt to resolve a hostname via DNS. Falls back to original on failure.
fn lookup_host(host: &str) -> String {
    let c_host = match std::ffi::CString::new(host) {
        Ok(s) => s,
        Err(_) => return host.to_string(),
    };

    unsafe {
        let mut hints: libc::addrinfo = std::mem::zeroed();
        hints.ai_flags = libc::AI_CANONNAME;
        hints.ai_family = libc::AF_UNSPEC;

        let mut result: *mut libc::addrinfo = std::ptr::null_mut();
        let rc = libc::getaddrinfo(c_host.as_ptr(), std::ptr::null(), &hints, &mut result);
        if rc != 0 || result.is_null() {
            return host.to_string();
        }

        let canonical = if !(*result).ai_canonname.is_null() {
            CStr::from_ptr((*result).ai_canonname)
                .to_string_lossy()
                .into_owned()
        } else {
            host.to_string()
        };

        libc::freeaddrinfo(result);
        canonical
    }
}

/// Format output for the -q / --count mode.
pub fn format_count(entries: &[UtmpxEntry]) -> String {
    let users: Vec<&str> = entries
        .iter()
        .filter(|e| e.ut_type == USER_PROCESS)
        .map(|e| e.ut_user.as_str())
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "{}", users.join(" "));
    let _ = write!(out, "# users={}", users.len());
    out
}

/// Format heading line.
pub fn format_heading(config: &WhoConfig) -> String {
    let mut out = String::new();
    let _ = write!(out, "{:<8}", "NAME");
    if config.show_mesg {
        let _ = write!(out, " S");
    }
    let _ = write!(out, " {:<12}", "LINE");
    let _ = write!(out, " {}", "TIME");
    if config.show_users || config.show_all {
        let _ = write!(out, " {:>14}", "IDLE    PID");
    }
    let _ = write!(out, " {}", "COMMENT");
    out
}

/// Run the who command and return the formatted output.
pub fn run_who(config: &WhoConfig) -> String {
    let entries = read_utmpx();
    let mut output = String::new();

    if config.show_count {
        return format_count(&entries);
    }

    if config.show_heading {
        let _ = writeln!(output, "{}", format_heading(config));
    }

    for entry in &entries {
        if should_show(entry, config) {
            let _ = writeln!(output, "{}", format_entry(entry, config));
        }
    }

    // Remove trailing newline for consistency
    if output.ends_with('\n') {
        output.pop();
    }

    output
}
