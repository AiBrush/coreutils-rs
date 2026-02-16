/// stdbuf -- run a command with modified buffering for its standard streams
///
/// Sets environment variables _STDBUF_I, _STDBUF_O, _STDBUF_E to communicate
/// the desired buffering modes to the child process. A full implementation would
/// use LD_PRELOAD with a shared library that reads these variables and calls
/// setvbuf(). This implementation sets the environment and execs the command.

use std::io;
use std::process;

/// Buffering mode specification.
#[derive(Clone, Debug)]
pub enum BufferMode {
    /// Line buffered (mode "L")
    Line,
    /// Unbuffered (mode "0")
    Unbuffered,
    /// Fully buffered with a specific size in bytes
    Size(usize),
}

impl BufferMode {
    /// Convert to the environment variable value string.
    pub fn to_env_value(&self) -> String {
        match self {
            BufferMode::Line => "L".to_string(),
            BufferMode::Unbuffered => "0".to_string(),
            BufferMode::Size(n) => n.to_string(),
        }
    }
}

/// Configuration for the stdbuf command.
#[derive(Clone, Debug)]
pub struct StdbufConfig {
    pub input: Option<BufferMode>,
    pub output: Option<BufferMode>,
    pub error: Option<BufferMode>,
    pub command: String,
    pub args: Vec<String>,
}

/// Parse a buffer mode string into a BufferMode.
///
/// Accepted formats:
/// - "L" or "l" -> Line buffered
/// - "0" -> Unbuffered
/// - A positive integer (optionally with K, M, G, T suffix) -> Size buffered
pub fn parse_buffer_mode(s: &str) -> Result<BufferMode, String> {
    if s.eq_ignore_ascii_case("L") {
        return Ok(BufferMode::Line);
    }
    if s == "0" {
        return Ok(BufferMode::Unbuffered);
    }

    // Parse size with optional suffix
    let (num_str, multiplier) = if let Some(prefix) = s.strip_suffix('K').or_else(|| s.strip_suffix('k')) {
        (prefix, 1024_usize)
    } else if let Some(prefix) = s.strip_suffix('M').or_else(|| s.strip_suffix('m')) {
        (prefix, 1024 * 1024)
    } else if let Some(prefix) = s.strip_suffix('G').or_else(|| s.strip_suffix('g')) {
        (prefix, 1024 * 1024 * 1024)
    } else if let Some(prefix) = s.strip_suffix('T').or_else(|| s.strip_suffix('t')) {
        (prefix, 1024_usize.wrapping_mul(1024 * 1024 * 1024))
    } else if let Some(prefix) = s.strip_suffix("KB").or_else(|| s.strip_suffix("kB")) {
        (prefix, 1000)
    } else if let Some(prefix) = s.strip_suffix("MB") {
        (prefix, 1_000_000)
    } else if let Some(prefix) = s.strip_suffix("GB") {
        (prefix, 1_000_000_000)
    } else {
        (s, 1)
    };

    let n: usize = num_str
        .parse()
        .map_err(|_| format!("invalid mode '{}'", s))?;

    if n == 0 && multiplier == 1 {
        return Ok(BufferMode::Unbuffered);
    }

    let size = n.checked_mul(multiplier)
        .ok_or_else(|| format!("mode size too large: '{}'", s))?;

    if size == 0 {
        Ok(BufferMode::Unbuffered)
    } else {
        Ok(BufferMode::Size(size))
    }
}

/// Run the stdbuf command: set environment variables and exec the child process.
pub fn run_stdbuf(config: &StdbufConfig) -> io::Result<()> {
    let mut cmd = process::Command::new(&config.command);
    cmd.args(&config.args);

    if let Some(ref mode) = config.input {
        cmd.env("_STDBUF_I", mode.to_env_value());
    }
    if let Some(ref mode) = config.output {
        cmd.env("_STDBUF_O", mode.to_env_value());
    }
    if let Some(ref mode) = config.error {
        cmd.env("_STDBUF_E", mode.to_env_value());
    }

    let status = cmd.status()?;
    process::exit(status.code().unwrap_or(125));
}
