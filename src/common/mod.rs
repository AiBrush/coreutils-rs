pub mod io;

/// Get the GNU-compatible tool name by stripping the 'f' prefix.
/// e.g., "fmd5sum" -> "md5sum", "fcut" -> "cut"
#[inline]
pub fn gnu_name(binary_name: &str) -> &str {
    binary_name.strip_prefix('f').unwrap_or(binary_name)
}

/// Reset SIGPIPE to default behavior (SIG_DFL) for GNU coreutils compatibility.
/// Rust sets SIGPIPE to SIG_IGN by default, but GNU tools are killed by SIGPIPE
/// (exit code 141 = 128 + 13). This must be called at the start of main().
#[inline]
pub fn reset_sigpipe() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Format an IO error message without the "(os error N)" suffix.
/// GNU coreutils prints e.g. "No such file or directory" while Rust's
/// Display impl adds " (os error 2)". This strips the suffix for compat.
pub fn io_error_msg(e: &std::io::Error) -> String {
    if let Some(raw) = e.raw_os_error() {
        let os_err = std::io::Error::from_raw_os_error(raw);
        let msg = format!("{}", os_err);
        msg.replace(&format!(" (os error {})", raw), "")
    } else {
        format!("{}", e)
    }
}
