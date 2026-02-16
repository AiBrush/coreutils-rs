use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// How to handle symlinks during recursive traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkFollow {
    /// -H: follow symlinks given on the command line only
    CommandLine,
    /// -L: follow all symlinks
    Always,
    /// -P: never follow symlinks (default)
    Never,
}

/// Configuration for chown/chgrp operations.
#[derive(Debug, Clone)]
pub struct ChownConfig {
    pub verbose: bool,
    pub changes: bool,
    pub silent: bool,
    pub recursive: bool,
    pub no_dereference: bool,
    pub preserve_root: bool,
    pub from_owner: Option<u32>,
    pub from_group: Option<u32>,
    pub symlink_follow: SymlinkFollow,
}

impl Default for ChownConfig {
    fn default() -> Self {
        Self {
            verbose: false,
            changes: false,
            silent: false,
            recursive: false,
            no_dereference: false,
            preserve_root: false,
            from_owner: None,
            from_group: None,
            symlink_follow: SymlinkFollow::Never,
        }
    }
}

/// Resolve a username to a UID, or parse a numeric UID.
pub fn resolve_user(name: &str) -> Option<u32> {
    if let Ok(uid) = name.parse::<u32>() {
        return Some(uid);
    }
    let c_name = CString::new(name).ok()?;
    let pw = unsafe { libc::getpwnam(c_name.as_ptr()) };
    if pw.is_null() {
        None
    } else {
        Some(unsafe { (*pw).pw_uid })
    }
}

/// Resolve a group name to a GID, or parse a numeric GID.
pub fn resolve_group(name: &str) -> Option<u32> {
    if let Ok(gid) = name.parse::<u32>() {
        return Some(gid);
    }
    let c_name = CString::new(name).ok()?;
    let gr = unsafe { libc::getgrnam(c_name.as_ptr()) };
    if gr.is_null() {
        None
    } else {
        Some(unsafe { (*gr).gr_gid })
    }
}

/// Convert a UID to a username string. Falls back to the numeric string.
pub fn uid_to_name(uid: u32) -> String {
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        return uid.to_string();
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*pw).pw_name) };
    name.to_string_lossy().into_owned()
}

/// Convert a GID to a group name string. Falls back to the numeric string.
pub fn gid_to_name(gid: u32) -> String {
    let gr = unsafe { libc::getgrgid(gid) };
    if gr.is_null() {
        return gid.to_string();
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*gr).gr_name) };
    name.to_string_lossy().into_owned()
}

/// Parse an ownership specification string.
///
/// Accepted formats:
/// - `USER` -- set owner only
/// - `USER:GROUP` or `USER.GROUP` -- set both (dot form is deprecated)
/// - `USER:` -- set owner and group to that user's login group
/// - `:GROUP` -- set group only
/// - numeric IDs are accepted anywhere a name is accepted
///
/// Returns `(Option<uid>, Option<gid>)`.
pub fn parse_owner_spec(spec: &str) -> Result<(Option<u32>, Option<u32>), String> {
    if spec.is_empty() {
        return Err("invalid spec: ''".to_string());
    }

    // Determine separator: prefer ':', fall back to '.' (deprecated)
    let sep = if spec.contains(':') {
        ':'
    } else if spec.contains('.') {
        '.'
    } else {
        // No separator -- just a user
        let uid = resolve_user(spec).ok_or_else(|| format!("invalid user: '{}'", spec))?;
        return Ok((Some(uid), None));
    };

    let idx = spec.find(sep).unwrap();
    let user_part = &spec[..idx];
    let group_part = &spec[idx + 1..];

    let uid = if user_part.is_empty() {
        None
    } else {
        Some(resolve_user(user_part).ok_or_else(|| format!("invalid user: '{}'", user_part))?)
    };

    let gid = if group_part.is_empty() {
        if let Some(u) = uid {
            // "USER:" means use the user's login group
            let pw = unsafe { libc::getpwuid(u) };
            if pw.is_null() {
                // For numeric UIDs that don't map to a user, we can't resolve
                // their login group -- GNU chown errors out here
                return Err(format!("failed to get login group for uid '{}'", u));
            }
            Some(unsafe { (*pw).pw_gid })
        } else {
            None
        }
    } else {
        Some(resolve_group(group_part).ok_or_else(|| format!("invalid group: '{}'", group_part))?)
    };

    Ok((uid, gid))
}

/// Get the owner and group of a reference file.
pub fn get_reference_ids(path: &Path) -> io::Result<(u32, u32)> {
    let meta = fs::metadata(path)?;
    Ok((meta.uid(), meta.gid()))
}

/// Change the owner and/or group of a single file.
///
/// Returns `Ok(true)` if the ownership was actually changed,
/// `Ok(false)` if it was already correct (or skipped due to `--from`).
pub fn chown_file(
    path: &Path,
    uid: Option<u32>,
    gid: Option<u32>,
    config: &ChownConfig,
) -> io::Result<bool> {
    // Read current metadata to check --from filter and detect no-op
    let meta = if config.no_dereference {
        fs::symlink_metadata(path)?
    } else {
        fs::metadata(path)?
    };

    // --from filter: skip if current owner/group does not match
    if let Some(from_uid) = config.from_owner {
        if meta.uid() != from_uid {
            return Ok(false);
        }
    }
    if let Some(from_gid) = config.from_group {
        if meta.gid() != from_gid {
            return Ok(false);
        }
    }

    let new_uid = uid.map(|u| u as libc::uid_t).unwrap_or(u32::MAX);
    let new_gid = gid.map(|g| g as libc::gid_t).unwrap_or(u32::MAX);

    // Detect no-op
    let current_uid = meta.uid();
    let current_gid = meta.gid();
    let uid_match = uid.is_none() || uid == Some(current_uid);
    let gid_match = gid.is_none() || gid == Some(current_gid);
    if uid_match && gid_match {
        // No change needed
        if config.verbose {
            print_verbose(path, uid, gid, false);
        }
        return Ok(false);
    }

    // Use -1 (u32::MAX cast) to mean "don't change" for lchown/chown
    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let ret = if config.no_dereference {
        unsafe { libc::lchown(c_path.as_ptr(), new_uid, new_gid) }
    } else {
        unsafe { libc::chown(c_path.as_ptr(), new_uid, new_gid) }
    };

    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    if config.verbose || config.changes {
        print_verbose(path, uid, gid, true);
    }

    Ok(true)
}

/// Print a verbose message about an ownership change.
fn print_verbose(path: &Path, uid: Option<u32>, gid: Option<u32>, changed: bool) {
    let action = if changed { "changed" } else { "retained" };
    let display = path.display();
    match (uid, gid) {
        (Some(u), Some(g)) => {
            eprintln!(
                "ownership of '{}' {} to {}:{}",
                display,
                action,
                uid_to_name(u),
                gid_to_name(g)
            );
        }
        (Some(u), None) => {
            eprintln!(
                "ownership of '{}' {} to {}",
                display,
                action,
                uid_to_name(u)
            );
        }
        (None, Some(g)) => {
            eprintln!("group of '{}' {} to {}", display, action, gid_to_name(g));
        }
        (None, None) => {
            eprintln!("ownership of '{}' {}", display, action);
        }
    }
}

/// Recursively change ownership of a directory tree.
/// Uses rayon for parallel processing when verbose/changes output is not needed.
pub fn chown_recursive(
    path: &Path,
    uid: Option<u32>,
    gid: Option<u32>,
    config: &ChownConfig,
    is_command_line_arg: bool,
    tool_name: &str,
) -> i32 {
    // Preserve-root check
    if config.preserve_root && path == Path::new("/") {
        eprintln!(
            "{}: it is dangerous to operate recursively on '/'",
            tool_name
        );
        eprintln!(
            "{}: use --no-preserve-root to override this failsafe",
            tool_name
        );
        return 1;
    }

    // For non-verbose mode, use parallel traversal
    if !config.verbose && !config.changes {
        let error_count = std::sync::atomic::AtomicI32::new(0);
        chown_recursive_parallel(
            path,
            uid,
            gid,
            config,
            is_command_line_arg,
            tool_name,
            &error_count,
        );
        return error_count.load(std::sync::atomic::Ordering::Relaxed);
    }

    // Sequential path for verbose/changes mode
    let mut errors = 0;

    if let Err(e) = chown_file(path, uid, gid, config) {
        if !config.silent {
            eprintln!(
                "{}: changing ownership of '{}': {}",
                tool_name,
                path.display(),
                crate::common::io_error_msg(&e)
            );
        }
        errors += 1;
    }

    let should_follow = match config.symlink_follow {
        SymlinkFollow::Always => true,
        SymlinkFollow::CommandLine => is_command_line_arg,
        SymlinkFollow::Never => false,
    };

    let is_dir = if should_follow {
        fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
    } else {
        fs::symlink_metadata(path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
    };

    if is_dir {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(e) => {
                if !config.silent {
                    eprintln!(
                        "{}: cannot read directory '{}': {}",
                        tool_name,
                        path.display(),
                        crate::common::io_error_msg(&e)
                    );
                }
                return errors + 1;
            }
        };
        for entry in entries {
            match entry {
                Ok(entry) => {
                    errors += chown_recursive(&entry.path(), uid, gid, config, false, tool_name);
                }
                Err(e) => {
                    if !config.silent {
                        eprintln!(
                            "{}: cannot access entry in '{}': {}",
                            tool_name,
                            path.display(),
                            crate::common::io_error_msg(&e)
                        );
                    }
                    errors += 1;
                }
            }
        }
    }

    errors
}

/// Parallel recursive chown using rayon.
fn chown_recursive_parallel(
    path: &Path,
    uid: Option<u32>,
    gid: Option<u32>,
    config: &ChownConfig,
    is_command_line_arg: bool,
    tool_name: &str,
    error_count: &std::sync::atomic::AtomicI32,
) {
    if let Err(e) = chown_file(path, uid, gid, config) {
        if !config.silent {
            eprintln!(
                "{}: changing ownership of '{}': {}",
                tool_name,
                path.display(),
                crate::common::io_error_msg(&e)
            );
        }
        error_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    let should_follow = match config.symlink_follow {
        SymlinkFollow::Always => true,
        SymlinkFollow::CommandLine => is_command_line_arg,
        SymlinkFollow::Never => false,
    };

    let is_dir = if should_follow {
        fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
    } else {
        fs::symlink_metadata(path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
    };

    if is_dir {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(e) => {
                if !config.silent {
                    eprintln!(
                        "{}: cannot read directory '{}': {}",
                        tool_name,
                        path.display(),
                        crate::common::io_error_msg(&e)
                    );
                }
                error_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        };
        let entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();

        use rayon::prelude::*;
        entries.par_iter().for_each(|entry| {
            chown_recursive_parallel(
                &entry.path(),
                uid,
                gid,
                config,
                false,
                tool_name,
                error_count,
            );
        });
    }
}
