use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// How interactive prompting should behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveMode {
    /// Never prompt.
    Never,
    /// Prompt once before removing more than 3 files or when recursive.
    Once,
    /// Prompt before every removal.
    Always,
}

/// Whether to protect the root directory from recursive removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreserveRoot {
    /// Refuse to remove '/' (default).
    Yes,
    /// Refuse to remove '/' and also reject arguments on different mount points.
    All,
    /// Allow removing '/'.
    No,
}

/// Configuration for the rm operation.
#[derive(Debug)]
pub struct RmConfig {
    /// Ignore nonexistent files, never prompt.
    pub force: bool,
    /// Interactive prompting mode.
    pub interactive: InteractiveMode,
    /// Remove directories and their contents recursively.
    pub recursive: bool,
    /// Remove empty directories.
    pub dir: bool,
    /// Print a message for each removed file.
    pub verbose: bool,
    /// Root protection mode.
    pub preserve_root: PreserveRoot,
    /// When used with -r, skip directories on different file systems.
    pub one_file_system: bool,
}

impl Default for RmConfig {
    fn default() -> Self {
        Self {
            force: false,
            interactive: InteractiveMode::Never,
            recursive: false,
            dir: false,
            verbose: false,
            preserve_root: PreserveRoot::Yes,
            one_file_system: false,
        }
    }
}

/// Prompt the user on stderr and return true if they answer 'y' or 'Y'.
fn prompt_yes(msg: &str) -> bool {
    eprint!("{}", msg);
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    let trimmed = answer.trim();
    trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
}

/// Remove a single path according to the given configuration.
///
/// Returns `Ok(true)` on success, `Ok(false)` on non-fatal failure (e.g. the
/// user declined a prompt, or the path was skipped), and `Err` on I/O errors
/// that should propagate.
pub fn rm_path(path: &Path, config: &RmConfig) -> Result<bool, io::Error> {
    // Check preserve-root: canonicalize to detect '/' even through symlinks.
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if canonical == Path::new("/") {
        if matches!(config.preserve_root, PreserveRoot::Yes | PreserveRoot::All) {
            eprintln!("rm: it is dangerous to operate recursively on '/'");
            eprintln!("rm: use --no-preserve-root to override this failsafe");
            return Ok(false);
        }
    }

    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            if config.force && e.kind() == io::ErrorKind::NotFound {
                return Ok(true);
            }
            eprintln!("rm: cannot remove '{}': {}", path.display(), e);
            return Ok(false);
        }
    };

    if meta.is_dir() {
        if config.recursive {
            if config.interactive == InteractiveMode::Always
                && !prompt_yes(&format!(
                    "rm: descend into directory '{}'? ",
                    path.display()
                ))
            {
                return Ok(false);
            }
            #[cfg(unix)]
            let root_dev = meta.dev();
            #[cfg(not(unix))]
            let root_dev = 0u64;
            let ok = rm_recursive(path, config, root_dev)?;
            Ok(ok)
        } else if config.dir {
            if config.interactive == InteractiveMode::Always
                && !prompt_yes(&format!("rm: remove directory '{}'? ", path.display()))
            {
                return Ok(false);
            }
            match std::fs::remove_dir(path) {
                Ok(()) => {
                    if config.verbose {
                        eprintln!("removed directory '{}'", path.display());
                    }
                    Ok(true)
                }
                Err(e) => {
                    eprintln!("rm: cannot remove '{}': {}", path.display(), e);
                    Ok(false)
                }
            }
        } else {
            eprintln!("rm: cannot remove '{}': Is a directory", path.display());
            Ok(false)
        }
    } else {
        if config.interactive == InteractiveMode::Always
            && !prompt_yes(&format!("rm: remove file '{}'? ", path.display()))
        {
            return Ok(false);
        }
        match std::fs::remove_file(path) {
            Ok(()) => {
                if config.verbose {
                    eprintln!("removed '{}'", path.display());
                }
                Ok(true)
            }
            Err(e) => {
                eprintln!("rm: cannot remove '{}': {}", path.display(), e);
                Ok(false)
            }
        }
    }
}

/// Recursively remove a directory tree.
/// Uses parallel removal via rayon when not in interactive mode.
fn rm_recursive(path: &Path, config: &RmConfig, root_dev: u64) -> Result<bool, io::Error> {
    // For non-interactive mode, use parallel recursive removal
    if config.interactive == InteractiveMode::Never && !config.verbose {
        let success = std::sync::atomic::AtomicBool::new(true);
        rm_recursive_parallel(path, config, root_dev, &success);
        // Remove the directory itself after children are removed
        if let Err(e) = std::fs::remove_dir(path) {
            eprintln!("rm: cannot remove '{}': {}", path.display(), e);
            return Ok(false);
        }
        return Ok(success.load(std::sync::atomic::Ordering::Relaxed));
    }

    let mut success = true;

    let entries = match std::fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => {
            eprintln!("rm: cannot remove '{}': {}", path.display(), e);
            return Ok(false);
        }
    };

    for entry in entries {
        let entry = entry?;
        let child_path = entry.path();
        let child_meta = match std::fs::symlink_metadata(&child_path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("rm: cannot remove '{}': {}", child_path.display(), e);
                success = false;
                continue;
            }
        };

        #[cfg(unix)]
        let skip_fs = config.one_file_system && child_meta.dev() != root_dev;
        #[cfg(not(unix))]
        let skip_fs = false;

        if skip_fs {
            continue;
        }

        if child_meta.is_dir() {
            if config.interactive == InteractiveMode::Always
                && !prompt_yes(&format!(
                    "rm: descend into directory '{}'? ",
                    child_path.display()
                ))
            {
                success = false;
                continue;
            }
            if !rm_recursive(&child_path, config, root_dev)? {
                success = false;
            }
        } else {
            if config.interactive == InteractiveMode::Always
                && !prompt_yes(&format!("rm: remove file '{}'? ", child_path.display()))
            {
                success = false;
                continue;
            }
            match std::fs::remove_file(&child_path) {
                Ok(()) => {
                    if config.verbose {
                        eprintln!("removed '{}'", child_path.display());
                    }
                }
                Err(e) => {
                    eprintln!("rm: cannot remove '{}': {}", child_path.display(), e);
                    success = false;
                }
            }
        }
    }

    // Now remove the (hopefully empty) directory itself.
    if config.interactive == InteractiveMode::Always
        && !prompt_yes(&format!("rm: remove directory '{}'? ", path.display()))
    {
        return Ok(false);
    }

    match std::fs::remove_dir(path) {
        Ok(()) => {
            if config.verbose {
                eprintln!("removed directory '{}'", path.display());
            }
        }
        Err(e) => {
            eprintln!("rm: cannot remove '{}': {}", path.display(), e);
            success = false;
        }
    }

    Ok(success)
}

/// Parallel recursive removal for non-interactive, non-verbose mode.
fn rm_recursive_parallel(
    path: &Path,
    config: &RmConfig,
    root_dev: u64,
    success: &std::sync::atomic::AtomicBool,
) {
    let entries = match std::fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => {
            if !config.force {
                eprintln!("rm: cannot remove '{}': {}", path.display(), e);
            }
            success.store(false, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    };

    let entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();

    use rayon::prelude::*;
    entries.par_iter().for_each(|entry| {
        let child_path = entry.path();
        let child_meta = match std::fs::symlink_metadata(&child_path) {
            Ok(m) => m,
            Err(e) => {
                if !config.force {
                    eprintln!("rm: cannot remove '{}': {}", child_path.display(), e);
                }
                success.store(false, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        };

        #[cfg(unix)]
        let skip_fs = config.one_file_system && child_meta.dev() != root_dev;
        #[cfg(not(unix))]
        let skip_fs = false;

        if skip_fs {
            return;
        }

        if child_meta.is_dir() {
            rm_recursive_parallel(&child_path, config, root_dev, success);
            if let Err(e) = std::fs::remove_dir(&child_path) {
                if !config.force {
                    eprintln!("rm: cannot remove '{}': {}", child_path.display(), e);
                }
                success.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        } else if let Err(e) = std::fs::remove_file(&child_path) {
            if !config.force {
                eprintln!("rm: cannot remove '{}': {}", child_path.display(), e);
            }
            success.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    });
}
