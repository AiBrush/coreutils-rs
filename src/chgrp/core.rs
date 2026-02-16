use std::io;
use std::path::Path;

use crate::chown::{ChownConfig, SymlinkFollow};

/// Configuration for chgrp operations.
/// This is a convenience wrapper that builds a `ChownConfig` internally.
#[derive(Debug, Clone)]
pub struct ChgrpConfig {
    pub verbose: bool,
    pub changes: bool,
    pub silent: bool,
    pub recursive: bool,
    pub no_dereference: bool,
    pub preserve_root: bool,
    pub from_group: Option<u32>,
    pub symlink_follow: SymlinkFollow,
}

impl Default for ChgrpConfig {
    fn default() -> Self {
        Self {
            verbose: false,
            changes: false,
            silent: false,
            recursive: false,
            no_dereference: false,
            preserve_root: false,
            from_group: None,
            symlink_follow: SymlinkFollow::Never,
        }
    }
}

impl ChgrpConfig {
    /// Convert to a `ChownConfig` for reuse of the chown infrastructure.
    pub fn to_chown_config(&self) -> ChownConfig {
        ChownConfig {
            verbose: self.verbose,
            changes: self.changes,
            silent: self.silent,
            recursive: self.recursive,
            no_dereference: self.no_dereference,
            preserve_root: self.preserve_root,
            from_owner: None,
            from_group: self.from_group,
            symlink_follow: self.symlink_follow,
        }
    }
}

/// Change the group of a single file.
///
/// This is a thin wrapper around `chown_file` with `uid = None`.
/// Returns `Ok(true)` if the group was actually changed.
pub fn chgrp_file(path: &Path, gid: u32, config: &ChgrpConfig) -> io::Result<bool> {
    let chown_config = config.to_chown_config();
    crate::chown::chown_file(path, None, Some(gid), &chown_config)
}

/// Recursively change the group of a directory tree.
///
/// Returns the number of errors encountered.
pub fn chgrp_recursive(
    path: &Path,
    gid: u32,
    config: &ChgrpConfig,
    is_command_line_arg: bool,
    tool_name: &str,
) -> i32 {
    let chown_config = config.to_chown_config();
    crate::chown::chown_recursive(path, None, Some(gid), &chown_config, is_command_line_arg, tool_name)
}
