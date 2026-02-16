use std::io::{self, BufWriter, Write};
use std::process;

use coreutils_rs::du::{
    DuConfig, DuEntry, du_path, parse_block_size, parse_threshold, print_entry,
    read_exclude_file,
};

const TOOL_NAME: &str = "du";

fn usage() {
    eprintln!(
        "Usage: {} [OPTION]... [FILE]...
  or:  {} [OPTION]... --files0-from=F
Summarize device usage of the set of FILEs, recursively for directories.

  -0, --null            end each output line with NUL, not newline
  -a, --all             write counts for all files, not just directories
      --apparent-size   print apparent sizes rather than device usage
  -B, --block-size=SIZE scale sizes by SIZE before printing them
  -b, --bytes           equivalent to --apparent-size --block-size=1
  -c, --total           produce a grand total
  -d, --max-depth=N     print the total for a directory only if it is N or
                        fewer levels below the command line argument
      --exclude=PATTERN exclude files that match PATTERN
  -h, --human-readable  print sizes in human readable format (e.g., 1K 234M 2G)
      --inodes          list inode usage information instead of block usage
  -k                    like --block-size=1K
  -L, --dereference     dereference all symbolic links
  -l, --count-links     count sizes many times if hard linked
  -m                    like --block-size=1M
  -P, --no-dereference  don't follow any symbolic links (this is the default)
  -S, --separate-dirs   for directories do not include size of subdirectories
      --si              like -h, but use powers of 1000 not 1024
  -s, --summarize       display only a total for each argument
  -t, --threshold=SIZE  exclude entries smaller than SIZE if positive,
                        or entries greater than SIZE if negative
      --time            show time of the last modification of any file in the
                        directory, or any of its subdirectories
      --time-style=STYLE show times using STYLE: full-iso, long-iso, iso
  -X, --exclude-from=FILE  exclude files that match any pattern in FILE
  -x, --one-file-system    skip directories on different file systems
      --help            display this help and exit
      --version         output version information and exit",
        TOOL_NAME, TOOL_NAME
    );
}

fn version() {
    eprintln!("{} (fcoreutils) {}", TOOL_NAME, env!("CARGO_PKG_VERSION"));
}

/// Parse command-line arguments manually (matching the project's style for sort, touch, etc.).
fn parse_args() -> (DuConfig, Vec<String>) {
    let mut config = DuConfig::default();
    let mut files = Vec::new();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if arg == "--" {
            // Everything after -- is a file argument.
            files.extend_from_slice(&args[i + 1..]);
            break;
        }

        if arg.starts_with("--") {
            // Long options.
            if arg == "--help" {
                usage();
                process::exit(0);
            } else if arg == "--version" {
                version();
                process::exit(0);
            } else if arg == "--null" {
                config.null_terminator = true;
            } else if arg == "--all" {
                config.all = true;
            } else if arg == "--apparent-size" {
                config.apparent_size = true;
            } else if arg == "--bytes" {
                config.apparent_size = true;
                config.block_size = 1;
            } else if arg == "--total" {
                config.total = true;
            } else if arg == "--summarize" {
                config.summarize = true;
            } else if arg == "--human-readable" {
                config.human_readable = true;
            } else if arg == "--si" {
                config.si = true;
            } else if arg == "--inodes" {
                config.inodes = true;
            } else if arg == "--dereference" {
                config.dereference = true;
            } else if arg == "--no-dereference" {
                config.dereference = false;
            } else if arg == "--count-links" {
                config.count_links = true;
            } else if arg == "--separate-dirs" {
                config.separate_dirs = true;
            } else if arg == "--one-file-system" {
                config.one_file_system = true;
            } else if arg == "--time" {
                config.show_time = true;
            } else if let Some(val) = arg.strip_prefix("--block-size=") {
                match parse_block_size(val) {
                    Ok(bs) => config.block_size = bs,
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
            } else if let Some(val) = arg.strip_prefix("--max-depth=") {
                match val.parse::<usize>() {
                    Ok(d) => config.max_depth = Some(d),
                    Err(_) => {
                        eprintln!("{}: invalid maximum depth '{}'", TOOL_NAME, val);
                        process::exit(1);
                    }
                }
            } else if let Some(val) = arg.strip_prefix("--threshold=") {
                match parse_threshold(val) {
                    Ok(t) => config.threshold = Some(t),
                    Err(e) => {
                        eprintln!("{}: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
            } else if let Some(val) = arg.strip_prefix("--exclude=") {
                config.exclude_patterns.push(val.to_string());
            } else if let Some(val) = arg.strip_prefix("--exclude-from=") {
                match read_exclude_file(val) {
                    Ok(pats) => config.exclude_patterns.extend(pats),
                    Err(e) => {
                        eprintln!("{}: cannot read '{}': {}", TOOL_NAME, val, e);
                        process::exit(1);
                    }
                }
            } else if let Some(val) = arg.strip_prefix("--time-style=") {
                config.time_style = val.to_string();
            } else if let Some(val) = arg.strip_prefix("--time=") {
                // --time=WORD: we accept it and enable time display.
                let _ = val; // currently we only show mtime
                config.show_time = true;
            } else {
                eprintln!("{}: unrecognized option '{}'", TOOL_NAME, arg);
                eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                process::exit(1);
            }
        } else if arg.starts_with('-') && arg.len() > 1 {
            // Short options (can be combined, e.g., -shc).
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut j = 0;
            while j < chars.len() {
                match chars[j] {
                    '0' => config.null_terminator = true,
                    'a' => config.all = true,
                    'b' => {
                        config.apparent_size = true;
                        config.block_size = 1;
                    }
                    'c' => config.total = true,
                    'h' => config.human_readable = true,
                    'k' => config.block_size = 1024,
                    'l' => config.count_links = true,
                    'm' => config.block_size = 1024 * 1024,
                    'L' => config.dereference = true,
                    'P' => config.dereference = false,
                    'S' => config.separate_dirs = true,
                    's' => config.summarize = true,
                    'x' => config.one_file_system = true,
                    'd' => {
                        // -d N (next chars or next arg)
                        let rest: String = chars[j + 1..].iter().collect();
                        let val_str = if rest.is_empty() {
                            i += 1;
                            if i >= args.len() {
                                eprintln!("{}: option requires an argument -- 'd'", TOOL_NAME);
                                process::exit(1);
                            }
                            args[i].clone()
                        } else {
                            j = chars.len(); // consume rest
                            rest
                        };
                        match val_str.parse::<usize>() {
                            Ok(d) => config.max_depth = Some(d),
                            Err(_) => {
                                eprintln!(
                                    "{}: invalid maximum depth '{}'",
                                    TOOL_NAME, val_str
                                );
                                process::exit(1);
                            }
                        }
                    }
                    'B' => {
                        let rest: String = chars[j + 1..].iter().collect();
                        let val_str = if rest.is_empty() {
                            i += 1;
                            if i >= args.len() {
                                eprintln!("{}: option requires an argument -- 'B'", TOOL_NAME);
                                process::exit(1);
                            }
                            args[i].clone()
                        } else {
                            j = chars.len();
                            rest
                        };
                        match parse_block_size(&val_str) {
                            Ok(bs) => config.block_size = bs,
                            Err(e) => {
                                eprintln!("{}: {}", TOOL_NAME, e);
                                process::exit(1);
                            }
                        }
                    }
                    't' => {
                        let rest: String = chars[j + 1..].iter().collect();
                        let val_str = if rest.is_empty() {
                            i += 1;
                            if i >= args.len() {
                                eprintln!("{}: option requires an argument -- 't'", TOOL_NAME);
                                process::exit(1);
                            }
                            args[i].clone()
                        } else {
                            j = chars.len();
                            rest
                        };
                        match parse_threshold(&val_str) {
                            Ok(t) => config.threshold = Some(t),
                            Err(e) => {
                                eprintln!("{}: {}", TOOL_NAME, e);
                                process::exit(1);
                            }
                        }
                    }
                    'X' => {
                        let rest: String = chars[j + 1..].iter().collect();
                        let val_str = if rest.is_empty() {
                            i += 1;
                            if i >= args.len() {
                                eprintln!("{}: option requires an argument -- 'X'", TOOL_NAME);
                                process::exit(1);
                            }
                            args[i].clone()
                        } else {
                            j = chars.len();
                            rest
                        };
                        match read_exclude_file(&val_str) {
                            Ok(pats) => config.exclude_patterns.extend(pats),
                            Err(e) => {
                                eprintln!("{}: cannot read '{}': {}", TOOL_NAME, val_str, e);
                                process::exit(1);
                            }
                        }
                    }
                    _ => {
                        eprintln!("{}: invalid option -- '{}'", TOOL_NAME, chars[j]);
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
                j += 1;
            }
        } else {
            // Positional argument (file path).
            files.push(arg.clone());
        }

        i += 1;
    }

    // Default to current directory if no files specified.
    if files.is_empty() {
        files.push(".".to_string());
    }

    (config, files)
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let (config, files) = parse_args();

    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut had_error = false;
    let mut grand_total: u64 = 0;

    for file in &files {
        let path = std::path::Path::new(file);
        match du_path(path, &config) {
            Ok(entries) => {
                for entry in &entries {
                    if let Err(e) = print_entry(&mut out, entry, &config) {
                        eprintln!("{}: write error: {}", TOOL_NAME, e);
                        process::exit(1);
                    }
                }
                // The last entry for a path is the root's total.
                if let Some(last) = entries.last() {
                    grand_total += last.size;
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: cannot access '{}': {}",
                    TOOL_NAME,
                    file,
                    format_io_error(&e)
                );
                had_error = true;
            }
        }
    }

    // Print grand total if requested.
    if config.total {
        let total_entry = DuEntry {
            size: grand_total,
            path: std::path::PathBuf::from("total"),
            mtime: None,
        };
        if let Err(e) = print_entry(&mut out, &total_entry, &config) {
            eprintln!("{}: write error: {}", TOOL_NAME, e);
            process::exit(1);
        }
    }

    let _ = out.flush();

    if had_error {
        process::exit(1);
    }
}

/// Format an IO error without the "(os error N)" suffix.
fn format_io_error(e: &io::Error) -> String {
    if let Some(raw) = e.raw_os_error() {
        let os_err = io::Error::from_raw_os_error(raw);
        let msg = format!("{}", os_err);
        msg.replace(&format!(" (os error {})", raw), "")
    } else {
        format!("{}", e)
    }
}
