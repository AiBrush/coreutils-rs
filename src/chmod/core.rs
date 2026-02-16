use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Configuration for chmod operations.
#[derive(Debug, Clone, Default)]
pub struct ChmodConfig {
    /// Report only when a change is made.
    pub changes: bool,
    /// Suppress most error messages.
    pub quiet: bool,
    /// Output a diagnostic for every file processed.
    pub verbose: bool,
    /// Fail to operate recursively on '/'.
    pub preserve_root: bool,
    /// Operate recursively.
    pub recursive: bool,
}

// Permission bit constants
const S_ISUID: u32 = 0o4000;
const S_ISGID: u32 = 0o2000;
const S_ISVTX: u32 = 0o1000;

const S_IRUSR: u32 = 0o0400;
const S_IWUSR: u32 = 0o0200;
const S_IXUSR: u32 = 0o0100;

const S_IRGRP: u32 = 0o0040;
const S_IWGRP: u32 = 0o0020;
const S_IXGRP: u32 = 0o0010;

const S_IROTH: u32 = 0o0004;
const S_IWOTH: u32 = 0o0002;
const S_IXOTH: u32 = 0o0001;

const USER_BITS: u32 = S_IRUSR | S_IWUSR | S_IXUSR;
const GROUP_BITS: u32 = S_IRGRP | S_IWGRP | S_IXGRP;
const OTHER_BITS: u32 = S_IROTH | S_IWOTH | S_IXOTH;
const ALL_BITS: u32 = USER_BITS | GROUP_BITS | OTHER_BITS | S_ISUID | S_ISGID | S_ISVTX;

/// Parse a mode string (octal or symbolic) and return the new mode.
///
/// `mode_str` can be:
/// - Octal: "755", "0644"
/// - Symbolic: "u+x", "g-w", "o=r", "a+rw", "u=rw,g=r,o=", "+t", "u+s"
/// - Combined: "u+rwx,g+rx,o+r"
///
/// `current_mode` is the existing file mode, needed for symbolic modes.
pub fn parse_mode(mode_str: &str, current_mode: u32) -> Result<u32, String> {
    // Try octal first: only if all characters are octal digits
    if !mode_str.is_empty() && mode_str.chars().all(|c| c.is_ascii_digit() && c < '8') {
        if let Ok(octal) = u32::from_str_radix(mode_str, 8) {
            return Ok(octal & 0o7777);
        }
    }
    parse_symbolic_mode(mode_str, current_mode)
}

/// Parse a symbolic mode string and compute the resulting mode.
///
/// Format: `[ugoa]*[+-=][rwxXstugo]+` (comma-separated clauses)
fn parse_symbolic_mode(mode_str: &str, current_mode: u32) -> Result<u32, String> {
    let mut mode = current_mode & 0o7777;

    // Preserve the file type bits from the original mode so that
    // apply_symbolic_clause can detect directories for capital-X handling.
    let file_type_bits = current_mode & 0o170000;

    // Get the current umask
    let umask = get_umask();

    for clause in mode_str.split(',') {
        if clause.is_empty() {
            return Err(format!("invalid mode: '{}'", mode_str));
        }
        mode = apply_symbolic_clause(clause, mode | file_type_bits, umask)? & 0o7777;
    }

    Ok(mode)
}

/// Get the current umask value.
fn get_umask() -> u32 {
    // Set umask to 0, read the old value, then restore it.
    // SAFETY: umask is always safe to call.
    let old = unsafe { libc::umask(0) };
    unsafe {
        libc::umask(old);
    }
    old as u32
}

/// Apply a single symbolic mode clause (e.g. "u+x", "go-w", "a=r").
fn apply_symbolic_clause(clause: &str, current_mode: u32, umask: u32) -> Result<u32, String> {
    let bytes = clause.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    // Parse the "who" part: [ugoa]*
    let mut who_mask: u32 = 0;
    let mut who_specified = false;
    while pos < len {
        match bytes[pos] {
            b'u' => {
                who_mask |= USER_BITS | S_ISUID;
                who_specified = true;
            }
            b'g' => {
                who_mask |= GROUP_BITS | S_ISGID;
                who_specified = true;
            }
            b'o' => {
                who_mask |= OTHER_BITS | S_ISVTX;
                who_specified = true;
            }
            b'a' => {
                who_mask |= ALL_BITS;
                who_specified = true;
            }
            _ => break,
        }
        pos += 1;
    }

    // If no who specified, default to 'a' but filtered by umask
    if !who_specified {
        who_mask = ALL_BITS;
    }

    if pos >= len {
        return Err(format!("invalid mode: '{}'", clause));
    }

    let mut mode = current_mode;

    // Process one or more operator+perm sequences: [+-=][rwxXstugo]*
    while pos < len {
        // Parse operator
        let op = match bytes[pos] {
            b'+' => '+',
            b'-' => '-',
            b'=' => '=',
            _ => return Err(format!("invalid mode: '{}'", clause)),
        };
        pos += 1;

        // Parse permission bits
        let mut perm_bits: u32 = 0;
        let mut has_x_cap = false;

        while pos < len && bytes[pos] != b'+' && bytes[pos] != b'-' && bytes[pos] != b'=' {
            match bytes[pos] {
                b'r' => {
                    perm_bits |= S_IRUSR | S_IRGRP | S_IROTH;
                }
                b'w' => {
                    perm_bits |= S_IWUSR | S_IWGRP | S_IWOTH;
                }
                b'x' => {
                    perm_bits |= S_IXUSR | S_IXGRP | S_IXOTH;
                }
                b'X' => {
                    has_x_cap = true;
                }
                b's' => {
                    perm_bits |= S_ISUID | S_ISGID;
                }
                b't' => {
                    perm_bits |= S_ISVTX;
                }
                b'u' => {
                    // Copy user bits
                    let u = current_mode & USER_BITS;
                    perm_bits |= u | (u >> 3) | (u >> 6);
                }
                b'g' => {
                    // Copy group bits
                    let g = current_mode & GROUP_BITS;
                    perm_bits |= (g << 3) | g | (g >> 3);
                }
                b'o' => {
                    // Copy other bits
                    let o = current_mode & OTHER_BITS;
                    perm_bits |= (o << 6) | (o << 3) | o;
                }
                b',' => break,
                _ => return Err(format!("invalid mode: '{}'", clause)),
            }
            pos += 1;
        }

        // Handle capital X: add execute only if directory or already has execute
        if has_x_cap {
            // Check if current mode has any execute bit set, or if we're going
            // to set any execute bit. The caller's is_dir check happens at
            // chmod_file level. For parse_mode, we check the current_mode.
            let is_executable = (current_mode & (S_IXUSR | S_IXGRP | S_IXOTH)) != 0;
            // Note: directory check is indicated by the S_IFDIR bit (0o40000)
            let is_dir = (current_mode & 0o170000) == 0o040000;
            if is_executable || is_dir {
                perm_bits |= S_IXUSR | S_IXGRP | S_IXOTH;
            }
        }

        // Apply the operation, masked by who_mask
        let effective = perm_bits & who_mask;

        // When who is not specified, apply umask filtering for + and -
        let effective = if !who_specified {
            // For +/-, umask filters which bits can be changed
            // For =, umask also applies
            let umask_filter = !(umask) & (USER_BITS | GROUP_BITS | OTHER_BITS);
            // Keep setuid/setgid/sticky that are in effective
            let special = effective & (S_ISUID | S_ISGID | S_ISVTX);
            (effective & umask_filter) | special
        } else {
            effective
        };

        match op {
            '+' => {
                mode |= effective;
            }
            '-' => {
                mode &= !effective;
            }
            '=' => {
                // Clear the who bits, then set the specified ones
                let clear_mask = who_mask & (USER_BITS | GROUP_BITS | OTHER_BITS);
                // For '=', also clear setuid/setgid/sticky if who includes them
                let clear_special = who_mask & (S_ISUID | S_ISGID | S_ISVTX);
                mode &= !(clear_mask | clear_special);

                let effective_eq = if !who_specified {
                    let umask_filter = !(umask) & (USER_BITS | GROUP_BITS | OTHER_BITS);
                    let special = (perm_bits & who_mask) & (S_ISUID | S_ISGID | S_ISVTX);
                    ((perm_bits & who_mask) & umask_filter) | special
                } else {
                    perm_bits & who_mask
                };
                mode |= effective_eq;
            }
            _ => unreachable!(),
        }
    }

    Ok(mode)
}

/// Format a mode as an octal string (4 digits).
fn format_mode(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

/// Apply a mode to a file and return whether a change was made.
///
/// If `config.verbose` is true, prints a message for every file.
/// If `config.changes` is true, prints only when the mode changes.
pub fn chmod_file(path: &Path, mode: u32, config: &ChmodConfig) -> Result<bool, io::Error> {
    let metadata = fs::symlink_metadata(path)?;

    // Skip symlinks
    if metadata.file_type().is_symlink() {
        return Ok(false);
    }

    let old_mode = metadata.mode() & 0o7777;
    let changed = old_mode != mode;

    if changed {
        let perms = fs::Permissions::from_mode(mode);
        fs::set_permissions(path, perms)?;
    }

    let path_display = path.display();
    if config.verbose {
        if changed {
            eprintln!(
                "mode of '{}' changed from {} to {}",
                path_display,
                format_mode(old_mode),
                format_mode(mode)
            );
        } else {
            eprintln!(
                "mode of '{}' retained as {}",
                path_display,
                format_mode(old_mode)
            );
        }
    } else if config.changes && changed {
        eprintln!(
            "mode of '{}' changed from {} to {}",
            path_display,
            format_mode(old_mode),
            format_mode(mode)
        );
    }

    Ok(changed)
}

/// Recursively apply a mode string to a directory tree.
///
/// The mode is re-parsed for each file using its current mode, which matters
/// for symbolic modes (e.g. `a+X` behaves differently for files vs directories).
pub fn chmod_recursive(
    path: &Path,
    mode_str: &str,
    config: &ChmodConfig,
) -> Result<bool, io::Error> {
    if config.preserve_root && path == Path::new("/") {
        return Err(io::Error::other(
            "it is dangerous to operate recursively on '/'",
        ));
    }

    let mut had_error = false;

    // Process the path itself first
    match process_entry(path, mode_str, config) {
        Ok(()) => {}
        Err(e) => {
            if !config.quiet {
                eprintln!("chmod: cannot access '{}': {}", path.display(), e);
            }
            had_error = true;
        }
    }

    // Walk the directory tree
    if path.is_dir() {
        walk_dir(path, mode_str, config, &mut had_error);
    }

    if had_error {
        Err(io::Error::other("some operations failed"))
    } else {
        Ok(true)
    }
}

/// Process a single entry: read its mode, parse the mode string, and apply.
fn process_entry(path: &Path, mode_str: &str, config: &ChmodConfig) -> Result<(), io::Error> {
    let metadata = fs::symlink_metadata(path)?;

    // Skip symlinks
    if metadata.file_type().is_symlink() {
        return Ok(());
    }

    let current_mode = metadata.mode();
    let new_mode = parse_mode(mode_str, current_mode)
        .map_err(|e| io::Error::other(e))?;
    chmod_file(path, new_mode, config)?;
    Ok(())
}

/// Walk a directory recursively, applying the mode to each entry.
fn walk_dir(dir: &Path, mode_str: &str, config: &ChmodConfig, had_error: &mut bool) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if !config.quiet {
                eprintln!(
                    "chmod: cannot open directory '{}': {}",
                    dir.display(),
                    e
                );
            }
            *had_error = true;
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                if !config.quiet {
                    eprintln!("chmod: error reading directory entry: {}", e);
                }
                *had_error = true;
                continue;
            }
        };

        let entry_path = entry.path();

        // Don't follow symlinks
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                if !config.quiet {
                    eprintln!(
                        "chmod: cannot read file type of '{}': {}",
                        entry_path.display(),
                        e
                    );
                }
                *had_error = true;
                continue;
            }
        };

        if file_type.is_symlink() {
            continue;
        }

        match process_entry(&entry_path, mode_str, config) {
            Ok(()) => {}
            Err(e) => {
                if !config.quiet {
                    eprintln!(
                        "chmod: changing permissions of '{}': {}",
                        entry_path.display(),
                        e
                    );
                }
                *had_error = true;
            }
        }

        if file_type.is_dir() {
            walk_dir(&entry_path, mode_str, config, had_error);
        }
    }
}
