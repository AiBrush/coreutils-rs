use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::Instant;

/// Status output level for dd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusLevel {
    /// Print transfer stats at end (default).
    #[default]
    Default,
    /// No informational messages to stderr.
    None,
    /// Print periodic transfer stats (like GNU dd `status=progress`).
    Progress,
    /// Like default but also suppress error messages.
    NoError,
}

/// Conversion flags for dd (`conv=` option).
#[derive(Debug, Clone, Default)]
pub struct DdConv {
    /// Convert to lowercase.
    pub lcase: bool,
    /// Convert to uppercase.
    pub ucase: bool,
    /// Swap every pair of input bytes.
    pub swab: bool,
    /// Continue after read errors.
    pub noerror: bool,
    /// Do not truncate the output file.
    pub notrunc: bool,
    /// Pad every input block with NULs to ibs-size.
    pub sync: bool,
    /// Call fdatasync on output before finishing.
    pub fdatasync: bool,
    /// Call fsync on output before finishing.
    pub fsync: bool,
    /// Fail if the output file already exists.
    pub excl: bool,
    /// Do not create the output file.
    pub nocreat: bool,
}

/// Input/output flags for dd (`iflag=`/`oflag=` options).
#[derive(Debug, Clone, Default)]
pub struct DdFlags {
    pub append: bool,
    pub direct: bool,
    pub directory: bool,
    pub dsync: bool,
    pub sync: bool,
    pub fullblock: bool,
    pub nonblock: bool,
    pub noatime: bool,
    pub nocache: bool,
    pub noctty: bool,
    pub nofollow: bool,
    pub count_bytes: bool,
    pub skip_bytes: bool,
}

/// Configuration for a dd operation.
#[derive(Debug, Clone)]
pub struct DdConfig {
    /// Input file path (None = stdin).
    pub input: Option<String>,
    /// Output file path (None = stdout).
    pub output: Option<String>,
    /// Input block size in bytes.
    pub ibs: usize,
    /// Output block size in bytes.
    pub obs: usize,
    /// Copy only this many input blocks (None = unlimited).
    pub count: Option<u64>,
    /// Skip this many ibs-sized blocks at start of input.
    pub skip: u64,
    /// Skip this many obs-sized blocks at start of output.
    pub seek: u64,
    /// Conversion options.
    pub conv: DdConv,
    /// Status output level.
    pub status: StatusLevel,
    /// Input flags.
    pub iflag: DdFlags,
    /// Output flags.
    pub oflag: DdFlags,
}

impl Default for DdConfig {
    fn default() -> Self {
        DdConfig {
            input: None,
            output: None,
            ibs: 512,
            obs: 512,
            count: None,
            skip: 0,
            seek: 0,
            conv: DdConv::default(),
            status: StatusLevel::default(),
            iflag: DdFlags::default(),
            oflag: DdFlags::default(),
        }
    }
}

/// Statistics from a dd copy operation.
#[derive(Debug, Clone, Default)]
pub struct DdStats {
    /// Number of full input blocks read.
    pub records_in_full: u64,
    /// Number of partial input blocks read.
    pub records_in_partial: u64,
    /// Number of full output blocks written.
    pub records_out_full: u64,
    /// Number of partial output blocks written.
    pub records_out_partial: u64,
    /// Total bytes copied.
    pub bytes_copied: u64,
}

/// Parse a SIZE string with optional suffix.
///
/// Supported suffixes: c (1), w (2), b (512),
/// K/kB (1000), KiB/k (1024),
/// M/MB (1000^2), MiB (1024^2),
/// G/GB (1000^3), GiB (1024^3),
/// T/TB (1000^4), TiB (1024^4),
/// P/PB (1000^5), PiB (1024^5),
/// E/EB (1000^6), EiB (1024^6).
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size string".to_string());
    }

    // Find where the numeric part ends
    let num_end = s
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(s.len());

    if num_end == 0 {
        return Err(format!("invalid number: '{}'", s));
    }

    let num: u64 = s[..num_end]
        .parse()
        .map_err(|e| format!("invalid number '{}': {}", &s[..num_end], e))?;

    let suffix = &s[num_end..];
    let multiplier: u64 = match suffix {
        "" => 1,
        "c" => 1,
        "w" => 2,
        "b" => 512,
        "K" | "kB" => 1000,
        "KiB" | "k" => 1024,
        "M" | "MB" => 1_000_000,
        "MiB" => 1_048_576,
        "G" | "GB" => 1_000_000_000,
        "GiB" => 1_073_741_824,
        "T" | "TB" => 1_000_000_000_000,
        "TiB" => 1_099_511_627_776,
        "P" | "PB" => 1_000_000_000_000_000,
        "PiB" => 1_125_899_906_842_624,
        "E" | "EB" => 1_000_000_000_000_000_000,
        "EiB" => 1_152_921_504_606_846_976,
        _ => return Err(format!("invalid suffix: '{}'", suffix)),
    };

    num.checked_mul(multiplier)
        .ok_or_else(|| format!("size overflow: {} * {}", num, multiplier))
}

/// Parse dd command-line arguments (key=value pairs).
pub fn parse_dd_args(args: &[String]) -> Result<DdConfig, String> {
    let mut config = DdConfig::default();
    let mut bs_set = false;

    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            match key {
                "if" => config.input = Some(value.to_string()),
                "of" => config.output = Some(value.to_string()),
                "bs" => {
                    let size = parse_size(value)? as usize;
                    config.ibs = size;
                    config.obs = size;
                    bs_set = true;
                }
                "ibs" => {
                    if !bs_set {
                        config.ibs = parse_size(value)? as usize;
                    }
                }
                "obs" => {
                    if !bs_set {
                        config.obs = parse_size(value)? as usize;
                    }
                }
                "count" => config.count = Some(parse_size(value)?),
                "skip" => config.skip = parse_size(value)?,
                "seek" => config.seek = parse_size(value)?,
                "conv" => {
                    for flag in value.split(',') {
                        match flag {
                            "lcase" => config.conv.lcase = true,
                            "ucase" => config.conv.ucase = true,
                            "swab" => config.conv.swab = true,
                            "noerror" => config.conv.noerror = true,
                            "notrunc" => config.conv.notrunc = true,
                            "sync" => config.conv.sync = true,
                            "fdatasync" => config.conv.fdatasync = true,
                            "fsync" => config.conv.fsync = true,
                            "excl" => config.conv.excl = true,
                            "nocreat" => config.conv.nocreat = true,
                            "" => {}
                            _ => return Err(format!("invalid conversion: '{}'", flag)),
                        }
                    }
                }
                "iflag" => {
                    for flag in value.split(',') {
                        parse_flag(flag, &mut config.iflag)?;
                    }
                }
                "oflag" => {
                    for flag in value.split(',') {
                        parse_flag(flag, &mut config.oflag)?;
                    }
                }
                "status" => {
                    config.status = match value {
                        "none" => StatusLevel::None,
                        "noerrror" | "noerror" => StatusLevel::NoError,
                        "progress" => StatusLevel::Progress,
                        _ => return Err(format!("invalid status level: '{}'", value)),
                    };
                }
                _ => return Err(format!("unrecognized operand: '{}'", arg)),
            }
        } else {
            return Err(format!("unrecognized operand: '{}'", arg));
        }
    }

    // Validate conflicting options
    if config.conv.lcase && config.conv.ucase {
        return Err("conv=lcase and conv=ucase are mutually exclusive".to_string());
    }
    if config.conv.excl && config.conv.nocreat {
        return Err("conv=excl and conv=nocreat are mutually exclusive".to_string());
    }

    Ok(config)
}

/// Parse a single iflag/oflag value into the DdFlags struct.
fn parse_flag(flag: &str, flags: &mut DdFlags) -> Result<(), String> {
    match flag {
        "append" => flags.append = true,
        "direct" => flags.direct = true,
        "directory" => flags.directory = true,
        "dsync" => flags.dsync = true,
        "sync" => flags.sync = true,
        "fullblock" => flags.fullblock = true,
        "nonblock" => flags.nonblock = true,
        "noatime" => flags.noatime = true,
        "nocache" => flags.nocache = true,
        "noctty" => flags.noctty = true,
        "nofollow" => flags.nofollow = true,
        "count_bytes" => flags.count_bytes = true,
        "skip_bytes" => flags.skip_bytes = true,
        "" => {}
        _ => return Err(format!("invalid flag: '{}'", flag)),
    }
    Ok(())
}

/// Read a full block from the reader, retrying on partial reads.
/// Returns the number of bytes actually read (0 means EOF).
fn read_full_block(reader: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// Apply conversion options to a data block.
/// Returns a new Vec with the conversions applied.
pub fn apply_conversions(data: &[u8], conv: &DdConv) -> Vec<u8> {
    let mut result = data.to_vec();

    if conv.swab {
        // Swap every pair of bytes
        let pairs = result.len() / 2;
        for i in 0..pairs {
            result.swap(i * 2, i * 2 + 1);
        }
    }

    if conv.lcase {
        for b in &mut result {
            b.make_ascii_lowercase();
        }
    } else if conv.ucase {
        for b in &mut result {
            b.make_ascii_uppercase();
        }
    }

    result
}

/// Skip input blocks by reading and discarding them.
fn skip_input(reader: &mut dyn Read, blocks: u64, block_size: usize) -> io::Result<()> {
    let mut discard_buf = vec![0u8; block_size];
    for _ in 0..blocks {
        let n = read_full_block(reader, &mut discard_buf)?;
        if n == 0 {
            break;
        }
    }
    Ok(())
}

/// Seek output by writing zero blocks (for non-seekable outputs) or using seek.
fn seek_output(writer: &mut Box<dyn Write>, seek_blocks: u64, block_size: usize) -> io::Result<()> {
    // Try to seek if the writer supports it. Since we use Box<dyn Write>,
    // we write zero blocks for the general case.
    let zero_block = vec![0u8; block_size];
    for _ in 0..seek_blocks {
        writer.write_all(&zero_block)?;
    }
    Ok(())
}

/// Seek output on a file using actual file seeking.
fn seek_output_file(file: &mut File, seek_blocks: u64, block_size: usize) -> io::Result<()> {
    let offset = seek_blocks * block_size as u64;
    file.seek(SeekFrom::Start(offset))?;
    Ok(())
}

/// Perform the dd copy operation.
pub fn dd_copy(config: &DdConfig) -> io::Result<DdStats> {
    let start_time = Instant::now();

    let mut input: Box<dyn Read> = if let Some(ref path) = config.input {
        Box::new(File::open(path).map_err(|e| {
            io::Error::new(e.kind(), format!("failed to open '{}': {}", path, e))
        })?)
    } else {
        Box::new(io::stdin())
    };

    // Handle output file creation/opening
    let have_output_file = config.output.is_some();
    let mut output_file: Option<File> = None;
    let mut output: Box<dyn Write> = if let Some(ref path) = config.output {
        let mut opts = OpenOptions::new();
        opts.write(true);

        if config.conv.excl {
            // excl: fail if file exists (create_new implies create)
            opts.create_new(true);
        } else if config.conv.nocreat {
            // nocreat: do not create, file must exist
            // Don't set create at all
        } else {
            opts.create(true);
        }

        if config.conv.notrunc {
            opts.truncate(false);
        } else if !config.conv.excl {
            // Default: truncate (but not with excl since create_new starts fresh)
            opts.truncate(true);
        }

        let file = opts.open(path).map_err(|e| {
            io::Error::new(e.kind(), format!("failed to open '{}': {}", path, e))
        })?;
        output_file = Some(file.try_clone()?);
        Box::new(file)
    } else {
        Box::new(io::stdout())
    };

    // Skip input blocks
    if config.skip > 0 {
        skip_input(&mut input, config.skip, config.ibs)?;
    }

    // Seek output blocks
    if config.seek > 0 {
        if let Some(ref mut f) = output_file {
            seek_output_file(f, config.seek, config.obs)?;
            // Rebuild the output Box with a new clone at the seeked position
            let seeked = f.try_clone()?;
            output = Box::new(seeked);
        } else {
            seek_output(&mut output, config.seek, config.obs)?;
        }
    }

    let mut stats = DdStats::default();
    let mut ibuf = vec![0u8; config.ibs];
    let mut obuf: Vec<u8> = Vec::with_capacity(config.obs);

    loop {
        // Check count limit
        if let Some(count) = config.count {
            if stats.records_in_full + stats.records_in_partial >= count {
                break;
            }
        }

        // Read one input block
        let n = match read_full_block(&mut input, &mut ibuf) {
            Ok(n) => n,
            Err(e) => {
                if config.conv.noerror {
                    if config.status != StatusLevel::None {
                        eprintln!("dd: error reading input: {}", e);
                    }
                    // On noerror with sync, fill the entire block with NULs
                    if config.conv.sync {
                        ibuf.fill(0);
                        config.ibs
                    } else {
                        continue;
                    }
                } else {
                    return Err(e);
                }
            }
        };

        if n == 0 {
            break;
        }

        // Track full vs partial blocks
        if n == config.ibs {
            stats.records_in_full += 1;
        } else {
            stats.records_in_partial += 1;
            // Pad with NULs if conv=sync
            if config.conv.sync {
                ibuf[n..].fill(0);
            }
        }

        // Determine the data slice to use
        let effective_len = if config.conv.sync { config.ibs } else { n };
        let data = apply_conversions(&ibuf[..effective_len], &config.conv);

        // Buffer output and flush when we have enough for a full output block
        obuf.extend_from_slice(&data);
        while obuf.len() >= config.obs {
            output.write_all(&obuf[..config.obs])?;
            stats.records_out_full += 1;
            stats.bytes_copied += config.obs as u64;
            obuf.drain(..config.obs);
        }
    }

    // Flush remaining partial output block
    if !obuf.is_empty() {
        output.write_all(&obuf)?;
        stats.records_out_partial += 1;
        stats.bytes_copied += obuf.len() as u64;
    }

    // Flush output
    output.flush()?;

    // fsync / fdatasync
    if have_output_file {
        if let Some(ref f) = output_file {
            if config.conv.fsync {
                f.sync_all()?;
            } else if config.conv.fdatasync {
                f.sync_data()?;
            }
        }
    }

    let elapsed = start_time.elapsed();

    // Print status
    if config.status != StatusLevel::None {
        print_stats(&stats, elapsed);
    }

    Ok(stats)
}

/// Print dd transfer statistics to stderr.
fn print_stats(stats: &DdStats, elapsed: std::time::Duration) {
    eprintln!(
        "{}+{} records in",
        stats.records_in_full, stats.records_in_partial
    );
    eprintln!(
        "{}+{} records out",
        stats.records_out_full, stats.records_out_partial
    );

    let secs = elapsed.as_secs_f64();
    if secs > 0.0 {
        let rate = stats.bytes_copied as f64 / secs;
        eprintln!(
            "{} bytes copied, {:.6} s, {}/s",
            stats.bytes_copied,
            secs,
            human_size(rate as u64)
        );
    } else {
        eprintln!("{} bytes copied", stats.bytes_copied);
    }
}

/// Format a byte count as a human-readable string (e.g., "1.5 MB").
fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "kB", "MB", "GB", "TB", "PB", "EB"];
    let mut size = bytes as f64;
    for &unit in UNITS {
        if size < 1000.0 {
            if size == size.floor() {
                return format!("{} {}", size as u64, unit);
            }
            return format!("{:.1} {}", size, unit);
        }
        size /= 1000.0;
    }
    format!("{:.1} EB", size * 1000.0)
}

/// Print help message for dd.
pub fn print_help() {
    eprint!(
        "\
Usage: dd [OPERAND]...
  or:  dd OPTION
Copy a file, converting and formatting according to the operands.

  bs=BYTES        read and write up to BYTES bytes at a time (default: 512)
  cbs=BYTES       convert BYTES bytes at a time
  conv=CONVS      convert the file as per the comma separated symbol list
  count=N         copy only N input blocks
  ibs=BYTES       read up to BYTES bytes at a time (default: 512)
  if=FILE         read from FILE instead of stdin
  iflag=FLAGS     read as per the comma separated symbol list
  obs=BYTES       write BYTES bytes at a time (default: 512)
  of=FILE         write to FILE instead of stdout
  oflag=FLAGS     write as per the comma separated symbol list
  seek=N          skip N obs-sized blocks at start of output
  skip=N          skip N ibs-sized blocks at start of input
  status=LEVEL    LEVEL of information to print to stderr;
                  'none' suppresses everything but error messages,
                  'noerrror' suppresses the final transfer statistics,
                  'progress' shows periodic transfer statistics

  BLOCKS and BYTES may be followed by the following multiplicative suffixes:
  c=1, w=2, b=512, kB=1000, K=1024, MB=1000*1000, M=1024*1024,
  GB=1000*1000*1000, GiB=1024*1024*1024, and so on for T, P, E.

Each CONV symbol may be:

  lcase     change upper case to lower case
  ucase     change lower case to upper case
  swab      swap every pair of input bytes
  sync      pad every input block with NULs to ibs-size
  noerror   continue after read errors
  notrunc   do not truncate the output file
  fdatasync physically write output file data before finishing
  fsync     likewise, but also write metadata
  excl      fail if the output file already exists
  nocreat   do not create the output file

Each FLAG symbol may be:

  append    append mode (makes sense only for output; conv=notrunc suggested)
  direct    use direct I/O for data
  directory fail unless a directory
  dsync     use synchronized I/O for data
  sync      likewise, but also for metadata
  fullblock accumulate full blocks of input (iflag only)
  nonblock  use non-blocking I/O
  noatime   do not update access time
  nocache   Request to drop cache
  noctty    do not assign controlling terminal from file
  nofollow  do not follow symlinks
  count_bytes  treat 'count=N' as a byte count (iflag only)
  skip_bytes   treat 'skip=N' as a byte count (iflag only)

  --help     display this help and exit
  --version  output version information and exit
"
    );
}

/// Print version information for dd.
pub fn print_version() {
    eprintln!("dd (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
}
