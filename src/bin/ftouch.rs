#[cfg(not(unix))]
fn main() {
    eprintln!("touch: only available on Unix");
    std::process::exit(1);
}

// ftouch -- change file timestamps
//
// Usage: touch [OPTION]... FILE...

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "touch";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Which timestamps to change.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg(unix)]
enum TimeTarget {
    Both,
    AccessOnly,
    ModifyOnly,
}

/// A timespec pair: (access_time, modification_time) in (seconds, nanoseconds).
#[derive(Clone, Copy)]
#[cfg(unix)]
struct TimePair {
    atime_sec: i64,
    atime_nsec: i64,
    mtime_sec: i64,
    mtime_nsec: i64,
}

#[cfg(unix)]
fn current_time() -> (i64, i64) {
    let mut tv = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut tv);
    }
    (tv.tv_sec, tv.tv_nsec)
}

#[cfg(unix)]
fn get_file_times(path: &str) -> Result<TimePair, std::io::Error> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path)?;
    Ok(TimePair {
        atime_sec: meta.atime(),
        atime_nsec: meta.atime_nsec(),
        mtime_sec: meta.mtime(),
        mtime_nsec: meta.mtime_nsec(),
    })
}

/// Parse a -t STAMP format: [[CC]YY]MMDDhhmm[.ss]
#[cfg(unix)]
fn parse_touch_timestamp(s: &str) -> Result<(i64, i64), String> {
    let (main_part, seconds) = if let Some(dot_pos) = s.rfind('.') {
        let secs_str = &s[dot_pos + 1..];
        if secs_str.len() != 2 {
            return Err(format!("invalid date format '{}'", s));
        }
        let secs: u32 = secs_str
            .parse()
            .map_err(|_| format!("invalid date format '{}'", s))?;
        if secs > 60 {
            return Err(format!("invalid date format '{}'", s));
        }
        (&s[..dot_pos], secs)
    } else {
        (s, 0u32)
    };

    // main_part is [[CC]YY]MMDDhhmm
    let (year, month, day, hour, minute) = match main_part.len() {
        8 => {
            // MMDDhhmm — use current year
            let now = current_time();
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            unsafe {
                libc::localtime_r(&now.0, &mut tm);
            }
            let year = tm.tm_year + 1900;
            let month: u32 = main_part[0..2]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let day: u32 = main_part[2..4]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let hour: u32 = main_part[4..6]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let minute: u32 = main_part[6..8]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            (year, month, day, hour, minute)
        }
        10 => {
            // YYMMDDhhmm
            let yy: i32 = main_part[0..2]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let year = if yy >= 69 { 1900 + yy } else { 2000 + yy };
            let month: u32 = main_part[2..4]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let day: u32 = main_part[4..6]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let hour: u32 = main_part[6..8]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let minute: u32 = main_part[8..10]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            (year, month, day, hour, minute)
        }
        12 => {
            // CCYYMMDDhhmm
            let year: i32 = main_part[0..4]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let month: u32 = main_part[4..6]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let day: u32 = main_part[6..8]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let hour: u32 = main_part[8..10]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            let minute: u32 = main_part[10..12]
                .parse()
                .map_err(|_| format!("invalid date format '{}'", s))?;
            (year, month, day, hour, minute)
        }
        _ => return Err(format!("invalid date format '{}'", s)),
    };

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
    {
        return Err(format!("invalid date format '{}'", s));
    }

    let epoch = mktime_local(year, month, day, hour, minute, seconds)?;
    Ok((epoch, 0))
}

#[cfg(unix)]
fn mktime_local(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Result<i64, String> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day as i32;
    tm.tm_hour = hour as i32;
    tm.tm_min = minute as i32;
    tm.tm_sec = second as i32;
    tm.tm_isdst = -1;

    let t = unsafe { libc::mktime(&mut tm) };
    if t == -1 {
        // mktime returns -1 on error, but -1 is also a valid time_t
        // (1969-12-31 23:59:59 UTC). Verify by checking if the normalized
        // tm fields match our input.
        if tm.tm_year != year - 1900
            || tm.tm_mon != month as i32 - 1
            || tm.tm_mday != day as i32
            || tm.tm_hour != hour as i32
            || tm.tm_min != minute as i32
            || tm.tm_sec != second as i32
        {
            return Err("invalid time value".to_string());
        }
    }
    Ok(t)
}

/// Parse a -d DATE string.
/// Supports: YYYY-MM-DD, YYYY-MM-DD HH:MM:SS, YYYY-MM-DDTHH:MM:SS, @epoch, "now"
#[cfg(unix)]
fn parse_date_string(s: &str) -> Result<(i64, i64), String> {
    let trimmed = s.trim();

    if trimmed.eq_ignore_ascii_case("now") {
        let (sec, nsec) = current_time();
        return Ok((sec, nsec));
    }

    // Epoch seconds: @N or @N.N
    if let Some(rest) = trimmed.strip_prefix('@') {
        if let Some(dot_pos) = rest.find('.') {
            let sec_str = &rest[..dot_pos];
            let nsec_str = &rest[dot_pos + 1..];
            let sec: i64 = sec_str
                .parse()
                .map_err(|_| format!("invalid date '{}'", s))?;
            // Pad or truncate nsec to 9 digits
            let nsec_padded = format!("{:0<9}", nsec_str);
            let nsec: i64 = nsec_padded[..9]
                .parse()
                .map_err(|_| format!("invalid date '{}'", s))?;
            return Ok((sec, nsec));
        }
        let sec: i64 = rest
            .parse()
            .map_err(|_| format!("invalid date '{}'", s))?;
        return Ok((sec, 0));
    }

    // ISO 8601: YYYY-MM-DD or YYYY-MM-DD HH:MM:SS or YYYY-MM-DDTHH:MM:SS
    // Also handle optional fractional seconds: YYYY-MM-DDTHH:MM:SS.NNN
    let normalized = trimmed.replace('T', " ");
    let parts: Vec<&str> = normalized.splitn(2, ' ').collect();

    let date_part = parts[0];
    let date_fields: Vec<&str> = date_part.split('-').collect();
    if date_fields.len() != 3 {
        return Err(format!("invalid date '{}'", s));
    }
    let year: i32 = date_fields[0]
        .parse()
        .map_err(|_| format!("invalid date '{}'", s))?;
    let month: u32 = date_fields[1]
        .parse()
        .map_err(|_| format!("invalid date '{}'", s))?;
    let day: u32 = date_fields[2]
        .parse()
        .map_err(|_| format!("invalid date '{}'", s))?;

    let (hour, minute, second, nsec) = if parts.len() > 1 {
        let time_part = parts[1].trim();
        // Check for fractional seconds
        let (time_str, frac_nsec) = if let Some(dot_pos) = time_part.find('.') {
            let frac_str = &time_part[dot_pos + 1..];
            let padded = format!("{:0<9}", frac_str);
            let ns: i64 = padded[..9]
                .parse()
                .map_err(|_| format!("invalid date '{}'", s))?;
            (&time_part[..dot_pos], ns)
        } else {
            (time_part, 0i64)
        };
        let time_fields: Vec<&str> = time_str.split(':').collect();
        if time_fields.len() < 2 || time_fields.len() > 3 {
            return Err(format!("invalid date '{}'", s));
        }
        let h: u32 = time_fields[0]
            .parse()
            .map_err(|_| format!("invalid date '{}'", s))?;
        let m: u32 = time_fields[1]
            .parse()
            .map_err(|_| format!("invalid date '{}'", s))?;
        let sec: u32 = if time_fields.len() == 3 {
            time_fields[2]
                .parse()
                .map_err(|_| format!("invalid date '{}'", s))?
        } else {
            0
        };
        (h, m, sec, frac_nsec)
    } else {
        (0, 0, 0, 0)
    };

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err(format!("invalid date '{}'", s));
    }

    let epoch = mktime_local(year, month, day, hour, minute, second)?;
    Ok((epoch, nsec))
}

/// Apply timestamps to a file using utimensat for nanosecond precision.
#[cfg(unix)]
fn set_file_times(
    path: &str,
    target: TimeTarget,
    sec: i64,
    nsec: i64,
    no_deref: bool,
) -> Result<(), std::io::Error> {
    // Get current times to preserve the one we're not changing
    let current = get_file_times(path).unwrap_or(TimePair {
        atime_sec: sec,
        atime_nsec: nsec,
        mtime_sec: sec,
        mtime_nsec: nsec,
    });

    let atime = match target {
        TimeTarget::Both | TimeTarget::AccessOnly => libc::timespec {
            tv_sec: sec,
            tv_nsec: nsec,
        },
        TimeTarget::ModifyOnly => libc::timespec {
            tv_sec: current.atime_sec,
            tv_nsec: current.atime_nsec,
        },
    };

    let mtime = match target {
        TimeTarget::Both | TimeTarget::ModifyOnly => libc::timespec {
            tv_sec: sec,
            tv_nsec: nsec,
        },
        TimeTarget::AccessOnly => libc::timespec {
            tv_sec: current.mtime_sec,
            tv_nsec: current.mtime_nsec,
        },
    };

    let times = [atime, mtime];
    let c_path =
        CString::new(path).map_err(|_| std::io::Error::other("invalid path"))?;

    let flags = if no_deref {
        libc::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };

    let ret = unsafe {
        libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), flags)
    };

    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let mut target = TimeTarget::Both;
    let mut no_create = false;
    let mut no_deref = false;
    let mut date_str: Option<String> = None;
    let mut reference: Option<String> = None;
    let mut stamp: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut saw_dashdash = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if saw_dashdash {
            files.push(arg.clone());
            i += 1;
            continue;
        }
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            "-a" => target = TimeTarget::AccessOnly,
            "-m" => target = TimeTarget::ModifyOnly,
            "-c" | "--no-create" => no_create = true,
            "-h" | "--no-dereference" => no_deref = true,
            "-d" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'd'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                date_str = Some(args[i].clone());
            }
            "-r" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 'r'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                reference = Some(args[i].clone());
            }
            "-t" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{}: option requires an argument -- 't'", TOOL_NAME);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
                stamp = Some(args[i].clone());
            }
            "--" => saw_dashdash = true,
            _ if arg.starts_with("--date=") => {
                date_str = Some(arg["--date=".len()..].to_string());
            }
            _ if arg.starts_with("--reference=") => {
                reference = Some(arg["--reference=".len()..].to_string());
            }
            _ if arg.starts_with("--time=") => {
                let val = &arg["--time=".len()..];
                match val {
                    "access" | "atime" | "use" => target = TimeTarget::AccessOnly,
                    "modify" | "mtime" => target = TimeTarget::ModifyOnly,
                    _ => {
                        eprintln!("{}: invalid argument '{}' for '--time'", TOOL_NAME, val);
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
            }
            _ if arg.starts_with("-d") && arg.len() > 2 => {
                date_str = Some(arg[2..].to_string());
            }
            _ if arg.starts_with("-r") && arg.len() > 2 => {
                reference = Some(arg[2..].to_string());
            }
            _ if arg.starts_with("-t") && arg.len() > 2 => {
                stamp = Some(arg[2..].to_string());
            }
            _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                // Combined short flags (e.g., -am, -cm)
                for ch in arg[1..].chars() {
                    match ch {
                        'a' => target = TimeTarget::AccessOnly,
                        'm' => target = TimeTarget::ModifyOnly,
                        'c' => no_create = true,
                        'h' => no_deref = true,
                        _ => {
                            eprintln!("{}: invalid option -- '{}'", TOOL_NAME, ch);
                            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                }
            }
            _ => files.push(arg.clone()),
        }
        i += 1;
    }

    if files.is_empty() {
        eprintln!("{}: missing file operand", TOOL_NAME);
        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
        process::exit(1);
    }

    // Determine the timestamp to apply
    let (ts_sec, ts_nsec) = if let Some(ref r) = reference {
        match get_file_times(r) {
            Ok(tp) => {
                match target {
                    TimeTarget::Both | TimeTarget::AccessOnly => (tp.atime_sec, tp.atime_nsec),
                    TimeTarget::ModifyOnly => (tp.mtime_sec, tp.mtime_nsec),
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: failed to get attributes of '{}': {}",
                    TOOL_NAME,
                    r,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    } else if let Some(ref d) = date_str {
        match parse_date_string(d) {
            Ok((sec, nsec)) => (sec, nsec),
            Err(e) => {
                eprintln!("{}: invalid date format '{}': {}", TOOL_NAME, d, e);
                process::exit(1);
            }
        }
    } else if let Some(ref st) = stamp {
        match parse_touch_timestamp(st) {
            Ok((sec, nsec)) => (sec, nsec),
            Err(e) => {
                eprintln!("{}: invalid date format '{}': {}", TOOL_NAME, st, e);
                process::exit(1);
            }
        }
    } else {
        current_time()
    };

    let mut exit_code = 0;
    for file in &files {
        // Create file if it doesn't exist and -c not specified
        if !path_exists(file) {
            if no_create {
                continue;
            }
            if let Err(e) = fs::File::create(file) {
                eprintln!(
                    "{}: cannot touch '{}': {}",
                    TOOL_NAME,
                    file,
                    coreutils_rs::common::io_error_msg(&e)
                );
                exit_code = 1;
                continue;
            }
        }

        if let Err(e) = set_file_times(file, target, ts_sec, ts_nsec, no_deref) {
            eprintln!(
                "{}: setting times of '{}': {}",
                TOOL_NAME,
                file,
                coreutils_rs::common::io_error_msg(&e)
            );
            exit_code = 1;
        }
    }

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

#[cfg(unix)]
fn path_exists(path: &str) -> bool {
    fs::symlink_metadata(path).is_ok()
}

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [OPTION]... FILE...", TOOL_NAME);
    println!("Update the access and modification times of each FILE to the current time.");
    println!();
    println!("A FILE argument that does not exist is created empty, unless -c is supplied.");
    println!();
    println!("  -a                     change only the access time");
    println!("  -c, --no-create        do not create any files");
    println!(
        "  -d, --date=STRING      parse STRING and use it instead of current time"
    );
    println!("  -h, --no-dereference   affect each symbolic link instead of any referenced");
    println!("                         file (useful only on systems that can change the");
    println!("                         timestamps of a symlink)");
    println!("  -m                     change only the modification time");
    println!(
        "  -r, --reference=FILE   use this file's times instead of current time"
    );
    println!(
        "  -t STAMP               use [[CC]YY]MMDDhhmm[.ss] instead of current time"
    );
    println!(
        "      --time=WORD        change the specified time:"
    );
    println!("                           WORD is access, atime, or use: equivalent to -a");
    println!("                           WORD is modify or mtime: equivalent to -m");
    println!("      --help     display this help and exit");
    println!("      --version  output version information and exit");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("ftouch");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("touch"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("touch"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_create_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("newfile.txt");
        assert!(!file.exists());

        let output = cmd().arg(file.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());
        assert!(file.exists());
    }

    #[test]
    fn test_no_create_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nocreate.txt");
        assert!(!file.exists());

        let output = cmd()
            .args(["-c", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!file.exists());
    }

    #[test]
    fn test_update_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("existing.txt");
        fs::write(&file, "content").unwrap();

        // Set a known old timestamp first
        let output = cmd()
            .args(["-t", "200001010000", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        let old_mtime = meta.mtime();

        // Now touch with current time
        let output = cmd().arg(file.to_str().unwrap()).output().unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        assert!(meta.mtime() > old_mtime, "mtime should be updated");
    }

    #[test]
    fn test_access_time_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("atime.txt");
        fs::write(&file, "data").unwrap();

        // Set known times first
        let output = cmd()
            .args(["-t", "200001010000", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        let old_mtime = meta.mtime();

        // Touch access time only with a new time
        let output = cmd()
            .args(["-a", "-t", "202301010000", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        // mtime should not have changed
        assert_eq!(meta.mtime(), old_mtime, "mtime should be unchanged");
        // atime should be updated
        assert!(meta.atime() > old_mtime, "atime should be updated");
    }

    #[test]
    fn test_modification_time_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mtime.txt");
        fs::write(&file, "data").unwrap();

        // Set known times first
        let output = cmd()
            .args(["-t", "200001010000", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        let old_atime = meta.atime();

        // Touch modification time only with a new time
        let output = cmd()
            .args(["-m", "-t", "202301010000", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        // atime should not have changed
        assert_eq!(meta.atime(), old_atime, "atime should be unchanged");
        // mtime should be updated
        assert!(meta.mtime() > old_atime, "mtime should be updated");
    }

    #[test]
    fn test_reference_file() {
        let dir = tempfile::tempdir().unwrap();
        let ref_file = dir.path().join("ref.txt");
        let target = dir.path().join("target.txt");
        fs::write(&ref_file, "ref").unwrap();
        fs::write(&target, "target").unwrap();

        // Set reference file to known time
        let output = cmd()
            .args(["-t", "200506071234", ref_file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let ref_meta = fs::metadata(&ref_file).unwrap();

        // Use reference file for target
        let output = cmd()
            .args(["-r", ref_file.to_str().unwrap(), target.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let target_meta = fs::metadata(&target).unwrap();
        assert_eq!(
            target_meta.atime(),
            ref_meta.atime(),
            "atime should match reference"
        );
        assert_eq!(
            target_meta.mtime(),
            ref_meta.mtime(),
            "mtime should match reference"
        );
    }

    #[test]
    fn test_t_timestamp_ccyymmddhhmmss() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stamp.txt");

        // 2023-06-15 12:30:45
        let output = cmd()
            .args(["-t", "202306151230.45", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        // Just verify it's in a reasonable range (June 2023)
        // 2023-06-15 00:00:00 UTC is approximately 1686787200
        assert!(meta.mtime() > 1686700000, "mtime should be in 2023");
        assert!(meta.mtime() < 1686900000, "mtime should be in June 2023");
    }

    #[test]
    fn test_t_timestamp_mmddhhm() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stamp2.txt");

        // MMDDhhmm (uses current year)
        let output = cmd()
            .args(["-t", "01150800", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(file.exists());
    }

    #[test]
    fn test_d_date_string_iso() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("date.txt");

        let output = cmd()
            .args(["-d", "2023-01-15", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        // 2023-01-15 is approximately 1673740800
        assert!(meta.mtime() > 1673600000);
        assert!(meta.mtime() < 1673900000);
    }

    #[test]
    fn test_d_date_string_iso_with_time() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("datetime.txt");

        let output = cmd()
            .args(["-d", "2023-01-15 10:30:00", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(file.exists());
    }

    #[test]
    fn test_d_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("epoch.txt");

        let output = cmd()
            .args(["-d", "@1000000000", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.mtime(), 1000000000);
    }

    #[test]
    fn test_d_now() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("now.txt");

        let output = cmd()
            .args(["-d", "now", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(file.exists());
    }

    #[test]
    fn test_missing_file_operand() {
        let output = cmd().output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing file operand"));
    }

    #[test]
    fn test_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("multi1.txt");
        let f2 = dir.path().join("multi2.txt");

        let output = cmd()
            .args([f1.to_str().unwrap(), f2.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(f1.exists());
        assert!(f2.exists());
    }

    #[test]
    fn test_time_word_access() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("timeword.txt");
        fs::write(&file, "data").unwrap();

        // Set known times first
        let output = cmd()
            .args(["-t", "200001010000", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        let old_mtime = meta.mtime();

        // Use --time=access to only change access time
        let output = cmd()
            .args([
                "--time=access",
                "-t",
                "202301010000",
                file.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());

        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.mtime(), old_mtime, "mtime should be unchanged");
    }

    #[test]
    fn test_matches_gnu_create() {
        let dir = tempfile::tempdir().unwrap();
        let gnu_file = dir.path().join("gnu_touch.txt");
        let our_file = dir.path().join("our_touch.txt");

        let gnu = Command::new("touch").arg(gnu_file.to_str().unwrap()).output();
        if let Ok(gnu) = gnu {
            let ours = cmd().arg(our_file.to_str().unwrap()).output().unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            assert!(gnu_file.exists() == our_file.exists(), "File creation mismatch");
        }
    }

    #[test]
    fn test_matches_gnu_no_create() {
        let dir = tempfile::tempdir().unwrap();
        let gnu_file = dir.path().join("gnu_nc.txt");
        let our_file = dir.path().join("our_nc.txt");

        let gnu = Command::new("touch")
            .args(["-c", gnu_file.to_str().unwrap()])
            .output();
        if let Ok(gnu) = gnu {
            let ours = cmd()
                .args(["-c", our_file.to_str().unwrap()])
                .output()
                .unwrap();
            assert_eq!(ours.status.code(), gnu.status.code(), "Exit code mismatch");
            assert!(!gnu_file.exists());
            assert!(!our_file.exists());
        }
    }

    #[test]
    fn test_parse_date_string_iso() {
        let (sec, _nsec) = super::parse_date_string("2023-01-15").unwrap();
        assert!(sec > 1673600000);
        assert!(sec < 1673900000);
    }

    #[test]
    fn test_parse_date_string_epoch() {
        let (sec, nsec) = super::parse_date_string("@1000000000").unwrap();
        assert_eq!(sec, 1000000000);
        assert_eq!(nsec, 0);
    }

    #[test]
    fn test_parse_date_string_epoch_frac() {
        let (sec, nsec) = super::parse_date_string("@1000000000.5").unwrap();
        assert_eq!(sec, 1000000000);
        assert_eq!(nsec, 500000000);
    }

    #[test]
    fn test_parse_date_string_now() {
        let (sec, _) = super::parse_date_string("now").unwrap();
        assert!(sec > 1600000000);
    }

    #[test]
    fn test_parse_date_string_invalid() {
        assert!(super::parse_date_string("not-a-date").is_err());
    }

    #[test]
    fn test_parse_touch_timestamp_ccyymmddhhmm() {
        let (sec, _) = super::parse_touch_timestamp("202301151030").unwrap();
        assert!(sec > 1673600000);
    }

    #[test]
    fn test_parse_touch_timestamp_with_seconds() {
        let (sec, _) = super::parse_touch_timestamp("202301151030.45").unwrap();
        assert!(sec > 1673600000);
    }

    #[test]
    fn test_parse_touch_timestamp_invalid() {
        assert!(super::parse_touch_timestamp("abc").is_err());
    }

    #[test]
    fn test_iso_t_format() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("isot.txt");

        let output = cmd()
            .args(["-d", "2023-06-15T10:30:00", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(file.exists());
    }

    #[test]
    fn test_long_date_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("longdate.txt");

        let output = cmd()
            .args(["--date=2023-01-15", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(file.exists());
    }

    #[test]
    fn test_long_reference_flag() {
        let dir = tempfile::tempdir().unwrap();
        let ref_file = dir.path().join("ref_long.txt");
        let target = dir.path().join("target_long.txt");
        fs::write(&ref_file, "ref").unwrap();
        fs::write(&target, "target").unwrap();

        let output = cmd()
            .args([
                &format!("--reference={}", ref_file.to_str().unwrap()),
                target.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
    }
}
