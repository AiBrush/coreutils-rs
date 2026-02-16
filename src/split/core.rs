use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Suffix type for output filenames.
#[derive(Clone, Debug, PartialEq)]
pub enum SuffixType {
    /// Alphabetic suffixes: aa, ab, ..., zz, aaa, ...
    Alphabetic,
    /// Numeric suffixes: 00, 01, ..., 99, 000, ...
    Numeric(u64),
    /// Hexadecimal suffixes: 00, 01, ..., ff, 000, ...
    Hex(u64),
}

/// Split mode: how to divide the input.
#[derive(Clone, Debug)]
pub enum SplitMode {
    /// Split every N lines (default 1000).
    Lines(u64),
    /// Split every N bytes.
    Bytes(u64),
    /// Split at line boundaries, at most N bytes per file.
    LineBytes(u64),
    /// Split into exactly N output files (by byte count).
    Number(u64),
}

/// Configuration for the split command.
#[derive(Clone, Debug)]
pub struct SplitConfig {
    pub mode: SplitMode,
    pub suffix_type: SuffixType,
    pub suffix_length: usize,
    pub additional_suffix: String,
    pub prefix: String,
    pub elide_empty: bool,
    pub verbose: bool,
    pub filter: Option<String>,
    pub separator: u8,
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            mode: SplitMode::Lines(1000),
            suffix_type: SuffixType::Alphabetic,
            suffix_length: 2,
            additional_suffix: String::new(),
            prefix: "x".to_string(),
            elide_empty: false,
            verbose: false,
            filter: None,
            separator: b'\n',
        }
    }
}

/// Parse a SIZE string with optional suffix.
/// Supports: K=1024, M=1024^2, G=1024^3, T=1024^4, P=1024^5, E=1024^6
/// Also: KB=1000, MB=1000^2, GB=1000^3, etc.
/// Also: b=512, KiB=1024, MiB=1024^2, etc.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size".to_string());
    }

    // Find where the numeric part ends
    let mut num_end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '+' || c == '-')) {
            num_end = i + c.len_utf8();
        } else {
            break;
        }
    }

    if num_end == 0 {
        return Err(format!("invalid number: '{}'", s));
    }

    let num_str = &s[..num_end];
    let suffix = &s[num_end..];

    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid number: '{}'", num_str))?;

    let multiplier: u64 = match suffix {
        "" => 1,
        "b" => 512,
        "kB" => 1000,
        "K" | "KiB" => 1024,
        "MB" => 1_000_000,
        "M" | "MiB" => 1_048_576,
        "GB" => 1_000_000_000,
        "G" | "GiB" => 1_073_741_824,
        "TB" => 1_000_000_000_000,
        "T" | "TiB" => 1_099_511_627_776,
        "PB" => 1_000_000_000_000_000,
        "P" | "PiB" => 1_125_899_906_842_624,
        "EB" => 1_000_000_000_000_000_000,
        "E" | "EiB" => 1_152_921_504_606_846_976,
        "ZB" | "Z" | "ZiB" | "YB" | "Y" | "YiB" => {
            if num > 0 {
                return Ok(u64::MAX);
            }
            return Ok(0);
        }
        _ => return Err(format!("invalid suffix in '{}'", s)),
    };

    num.checked_mul(multiplier)
        .ok_or_else(|| format!("number too large: '{}'", s))
}

/// Generate the suffix string for a given chunk index.
pub fn generate_suffix(index: u64, suffix_type: &SuffixType, suffix_length: usize) -> String {
    match suffix_type {
        SuffixType::Alphabetic => {
            let mut result = Vec::with_capacity(suffix_length);
            let mut remaining = index;
            for _ in 0..suffix_length {
                result.push(b'a' + (remaining % 26) as u8);
                remaining /= 26;
            }
            result.reverse();
            String::from_utf8(result).unwrap()
        }
        SuffixType::Numeric(start) => {
            let val = start + index;
            format!("{:0>width$}", val, width = suffix_length)
        }
        SuffixType::Hex(start) => {
            let val = start + index;
            format!("{:0>width$x}", val, width = suffix_length)
        }
    }
}

/// Compute the maximum number of chunks supported for a given suffix configuration.
pub fn max_chunks(suffix_type: &SuffixType, suffix_length: usize) -> u64 {
    match suffix_type {
        SuffixType::Alphabetic => 26u64.saturating_pow(suffix_length as u32),
        SuffixType::Numeric(_) | SuffixType::Hex(_) => 10u64.saturating_pow(suffix_length as u32),
    }
}

/// Build the output file path for a given chunk index.
fn output_path(config: &SplitConfig, index: u64) -> String {
    let suffix = generate_suffix(index, &config.suffix_type, config.suffix_length);
    format!("{}{}{}", config.prefix, suffix, config.additional_suffix)
}

/// Trait for output sinks: either a file or a filter command pipe.
trait ChunkWriter: Write {
    fn finish(&mut self) -> io::Result<()>;
}

/// Writes chunks to files on disk.
struct FileChunkWriter {
    writer: BufWriter<File>,
}

impl FileChunkWriter {
    fn create(path: &str) -> io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: BufWriter::with_capacity(128 * 1024, file),
        })
    }
}

impl Write for FileChunkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl ChunkWriter for FileChunkWriter {
    fn finish(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// Writes chunks to a filter command via pipe.
struct FilterChunkWriter {
    child: std::process::Child,
    _stdin_taken: bool,
}

impl FilterChunkWriter {
    fn create(filter_cmd: &str, output_path: &str) -> io::Result<Self> {
        let child = Command::new("sh")
            .arg("-c")
            .arg(filter_cmd)
            .env("FILE", output_path)
            .stdin(Stdio::piped())
            .spawn()?;
        Ok(Self {
            child,
            _stdin_taken: false,
        })
    }
}

impl Write for FilterChunkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(ref mut stdin) = self.child.stdin {
            stdin.write(buf)
        } else {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "stdin closed"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(ref mut stdin) = self.child.stdin {
            stdin.flush()
        } else {
            Ok(())
        }
    }
}

impl ChunkWriter for FilterChunkWriter {
    fn finish(&mut self) -> io::Result<()> {
        // Close stdin so the child can finish
        self.child.stdin.take();
        let status = self.child.wait()?;
        if !status.success() {
            return Err(io::Error::other(
                format!("filter command exited with status {}", status),
            ));
        }
        Ok(())
    }
}

/// Create a chunk writer for the given chunk index.
fn create_writer(config: &SplitConfig, index: u64) -> io::Result<Box<dyn ChunkWriter>> {
    let path = output_path(config, index);
    if config.verbose {
        eprintln!("creating file '{}'", path);
    }
    if let Some(ref filter_cmd) = config.filter {
        Ok(Box::new(FilterChunkWriter::create(filter_cmd, &path)?))
    } else {
        Ok(Box::new(FileChunkWriter::create(&path)?))
    }
}

/// Split input by line count.
fn split_by_lines(
    reader: &mut dyn BufRead,
    config: &SplitConfig,
    lines_per_chunk: u64,
) -> io::Result<()> {
    let limit = max_chunks(&config.suffix_type, config.suffix_length);
    let mut chunk_index: u64 = 0;
    let mut line_count: u64 = 0;
    let mut writer: Option<Box<dyn ChunkWriter>> = None;
    let sep = config.separator;

    let mut buf = Vec::with_capacity(8192);
    loop {
        buf.clear();
        let bytes_read = read_until_sep(reader, sep, &mut buf)?;
        if bytes_read == 0 {
            break;
        }

        if writer.is_none() {
            if chunk_index >= limit {
                return Err(io::Error::other(
                    "output file suffixes exhausted",
                ));
            }
            writer = Some(create_writer(config, chunk_index)?);
        }

        writer.as_mut().unwrap().write_all(&buf)?;
        line_count += 1;

        if line_count >= lines_per_chunk {
            writer.as_mut().unwrap().finish()?;
            writer = None;
            line_count = 0;
            chunk_index += 1;
        }
    }

    if let Some(ref mut w) = writer {
        w.finish()?;
    }

    Ok(())
}

/// Read bytes from reader until the separator byte (inclusive), appending to buf.
/// Returns number of bytes read (0 at EOF).
fn read_until_sep(reader: &mut dyn BufRead, sep: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
    if sep == b'\n' {
        // Use the built-in BufRead::read_until for newline, it's optimized
        let n = reader.read_until(b'\n', buf)?;
        return Ok(n);
    }
    // Custom separator
    let start_len = buf.len();
    loop {
        let available = match reader.fill_buf() {
            Ok(b) => b,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(buf.len() - start_len);
        }
        if let Some(pos) = memchr::memchr(sep, available) {
            buf.extend_from_slice(&available[..=pos]);
            let consume = pos + 1;
            reader.consume(consume);
            return Ok(buf.len() - start_len);
        }
        buf.extend_from_slice(available);
        let len = available.len();
        reader.consume(len);
    }
}

/// Split input by byte count.
fn split_by_bytes(
    reader: &mut dyn Read,
    config: &SplitConfig,
    bytes_per_chunk: u64,
) -> io::Result<()> {
    let limit = max_chunks(&config.suffix_type, config.suffix_length);
    let mut chunk_index: u64 = 0;
    let mut bytes_in_chunk: u64 = 0;
    let mut writer: Option<Box<dyn ChunkWriter>> = None;

    let mut read_buf = vec![0u8; 128 * 1024];
    loop {
        let bytes_read = match reader.read(&mut read_buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        let mut offset = 0usize;
        while offset < bytes_read {
            if writer.is_none() {
                if chunk_index >= limit {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "output file suffixes exhausted",
                    ));
                }
                writer = Some(create_writer(config, chunk_index)?);
                bytes_in_chunk = 0;
            }

            let remaining_in_chunk = (bytes_per_chunk - bytes_in_chunk) as usize;
            let remaining_in_buf = bytes_read - offset;
            let to_write = remaining_in_chunk.min(remaining_in_buf);

            writer.as_mut().unwrap().write_all(&read_buf[offset..offset + to_write])?;
            bytes_in_chunk += to_write as u64;
            offset += to_write;

            if bytes_in_chunk >= bytes_per_chunk {
                writer.as_mut().unwrap().finish()?;
                writer = None;
                chunk_index += 1;
            }
        }
    }

    if let Some(ref mut w) = writer {
        if config.elide_empty && bytes_in_chunk == 0 {
            w.finish()?;
            // Remove the empty file
            let path = output_path(config, chunk_index);
            let _ = fs::remove_file(&path);
        } else {
            w.finish()?;
        }
    }

    Ok(())
}

/// Split input by line-bytes: at most N bytes per file, breaking at line boundaries.
fn split_by_line_bytes(
    reader: &mut dyn BufRead,
    config: &SplitConfig,
    max_bytes: u64,
) -> io::Result<()> {
    let limit = max_chunks(&config.suffix_type, config.suffix_length);
    let mut chunk_index: u64 = 0;
    let mut bytes_in_chunk: u64 = 0;
    let mut writer: Option<Box<dyn ChunkWriter>> = None;
    let sep = config.separator;

    let mut buf = Vec::with_capacity(8192);
    loop {
        buf.clear();
        let bytes_read = read_until_sep(reader, sep, &mut buf)?;
        if bytes_read == 0 {
            break;
        }

        let line_len = buf.len() as u64;

        // If this line alone exceeds the max, we must write it (possibly to its own chunk).
        // If adding this line would exceed the max and we've already written something,
        // start a new chunk.
        if bytes_in_chunk > 0 && bytes_in_chunk + line_len > max_bytes {
            if let Some(ref mut w) = writer {
                w.finish()?;
            }
            writer = None;
            chunk_index += 1;
            bytes_in_chunk = 0;
        }

        if writer.is_none() {
            if chunk_index >= limit {
                return Err(io::Error::other(
                    "output file suffixes exhausted",
                ));
            }
            writer = Some(create_writer(config, chunk_index)?);
            bytes_in_chunk = 0;
        }

        // If the line itself is longer than max_bytes, we still write the whole line
        // to this chunk (GNU split behavior: -C never splits a line).
        writer.as_mut().unwrap().write_all(&buf)?;
        bytes_in_chunk += line_len;

        if bytes_in_chunk >= max_bytes {
            if let Some(ref mut w) = writer {
                w.finish()?;
            }
            writer = None;
            chunk_index += 1;
            bytes_in_chunk = 0;
        }
    }

    if let Some(ref mut w) = writer {
        w.finish()?;
    }

    Ok(())
}

/// Split input into exactly N chunks by byte count.
/// Reads the whole file to determine size, then distributes bytes evenly.
fn split_by_number(
    input_path: &str,
    config: &SplitConfig,
    n_chunks: u64,
) -> io::Result<()> {
    let limit = max_chunks(&config.suffix_type, config.suffix_length);
    if n_chunks > limit {
        return Err(io::Error::other(
            "output file suffixes exhausted",
        ));
    }
    if n_chunks == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid number of chunks: 0",
        ));
    }

    // Read input data
    let data = if input_path == "-" {
        let mut buf = Vec::new();
        io::stdin().lock().read_to_end(&mut buf)?;
        buf
    } else {
        fs::read(input_path)?
    };

    let total = data.len() as u64;
    let base_chunk_size = total / n_chunks;
    let remainder = total % n_chunks;

    let mut offset: u64 = 0;
    for i in 0..n_chunks {
        // First `remainder` chunks get one extra byte
        let chunk_size = base_chunk_size + if i < remainder { 1 } else { 0 };

        if config.elide_empty && chunk_size == 0 {
            continue;
        }

        let mut writer = create_writer(config, i)?;
        if chunk_size > 0 {
            let start = offset as usize;
            let end = start + chunk_size as usize;
            writer.write_all(&data[start..end])?;
        }
        writer.finish()?;
        offset += chunk_size;
    }

    Ok(())
}

/// Main entry point: split a file according to the given configuration.
/// `input_path` is the path to the input file, or "-" for stdin.
pub fn split_file(input_path: &str, config: &SplitConfig) -> io::Result<()> {
    // For number-based splitting, we need to read the whole file to know size.
    if let SplitMode::Number(n) = config.mode {
        return split_by_number(input_path, config, n);
    }

    // Open input
    let reader: Box<dyn Read> = if input_path == "-" {
        Box::new(io::stdin().lock())
    } else {
        let path = Path::new(input_path);
        if !path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("cannot open '{}' for reading: No such file or directory", input_path),
            ));
        }
        Box::new(File::open(path)?)
    };

    match config.mode {
        SplitMode::Lines(n) => {
            let mut buf_reader = BufReader::with_capacity(256 * 1024, reader);
            split_by_lines(&mut buf_reader, config, n)
        }
        SplitMode::Bytes(n) => {
            let mut reader = reader;
            split_by_bytes(&mut reader, config, n)
        }
        SplitMode::LineBytes(n) => {
            let mut buf_reader = BufReader::with_capacity(256 * 1024, reader);
            split_by_line_bytes(&mut buf_reader, config, n)
        }
        SplitMode::Number(_) => unreachable!(),
    }
}

/// Get the list of output file paths that would be generated for given config and chunk count.
pub fn output_paths(config: &SplitConfig, count: u64) -> Vec<PathBuf> {
    (0..count)
        .map(|i| PathBuf::from(output_path(config, i)))
        .collect()
}
