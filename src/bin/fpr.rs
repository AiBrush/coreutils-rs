use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process;
use std::time::SystemTime;

use coreutils_rs::common::{io_error_msg, reset_sigpipe};
use coreutils_rs::pr::{self, PrConfig};

struct Cli {
    config: PrConfig,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        config: PrConfig::default(),
        files: Vec::new(),
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if arg == "--" {
            cli.files.extend(args[i + 1..].iter().cloned());
            break;
        }

        // Handle +FIRST_PAGE[:LAST_PAGE]
        if let Some(page_spec) = arg.strip_prefix('+') {
            if let Some(colon) = page_spec.find(':') {
                if let Ok(first) = page_spec[..colon].parse::<usize>() {
                    cli.config.first_page = first;
                }
                if let Ok(last) = page_spec[colon + 1..].parse::<usize>() {
                    cli.config.last_page = last;
                }
            } else if let Ok(first) = page_spec.parse::<usize>() {
                cli.config.first_page = first;
            }
            i += 1;
            continue;
        }

        // Handle -COLUMN (numeric)
        if arg.starts_with('-') && arg.len() > 1 {
            let rest = &arg[1..];
            if let Ok(cols) = rest.parse::<usize>() {
                cli.config.columns = cols;
                i += 1;
                continue;
            }
        }

        if arg.starts_with("--") {
            let s = arg.as_str();
            if let Some(val) = s.strip_prefix("--columns=") {
                cli.config.columns = val.parse().unwrap_or(1);
            } else if let Some(val) = s.strip_prefix("--date-format=") {
                cli.config.date_format = val.to_string();
            } else if let Some(val) = s.strip_prefix("--header=") {
                cli.config.header = Some(val.to_string());
            } else if let Some(val) = s.strip_prefix("--length=") {
                cli.config.page_length = val.parse().unwrap_or(pr::DEFAULT_PAGE_LENGTH);
            } else if let Some(val) = s.strip_prefix("--first-line-number=") {
                cli.config.first_line_number = val.parse().unwrap_or(1);
            } else if let Some(val) = s.strip_prefix("--indent=") {
                cli.config.indent = val.parse().unwrap_or(0);
            } else if let Some(val) = s.strip_prefix("--page-width=") {
                cli.config.page_width = val.parse().unwrap_or(pr::DEFAULT_PAGE_WIDTH);
                cli.config.truncate_lines = true;
            } else if let Some(val) = s.strip_prefix("--separator=") {
                cli.config.separator = val.chars().next();
            } else if let Some(val) = s.strip_prefix("--sep-string=") {
                cli.config.sep_string = Some(val.to_string());
            } else {
                match s {
                    "--across" => cli.config.across = true,
                    "--show-control-chars" => cli.config.show_control_chars = true,
                    "--double-space" => cli.config.double_space = true,
                    "--expand-tabs" => cli.config.expand_tabs = Some(('\t', 8)),
                    "--form-feed" => cli.config.form_feed = true,
                    "--join-lines" => cli.config.join_lines = true,
                    "--merge" => cli.config.merge = true,
                    "--number-lines" => cli.config.number_lines = Some(('\t', 5)),
                    "--no-file-warnings" => cli.config.no_file_warnings = true,
                    "--omit-header" => cli.config.omit_header = true,
                    "--omit-pagination" => {
                        cli.config.omit_pagination = true;
                        cli.config.omit_header = true;
                    }
                    "--show-nonprinting" => cli.config.show_nonprinting = true,
                    "--help" => {
                        print_help();
                        process::exit(0);
                    }
                    "--version" => {
                        println!("pr (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                        process::exit(0);
                    }
                    _ => {
                        eprintln!("pr: unrecognized option '{}'", s);
                        eprintln!("Try 'pr --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else if arg.starts_with('-') && arg != "-" {
            let bytes = arg.as_bytes();
            let mut j = 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'a' => cli.config.across = true,
                    b'c' => cli.config.show_control_chars = true,
                    b'd' => cli.config.double_space = true,
                    b'D' => {
                        i += 1;
                        if i < args.len() {
                            cli.config.date_format = args[i].clone();
                        }
                        break;
                    }
                    b'e' => {
                        cli.config.expand_tabs = Some(('\t', 8));
                        // Check if followed by optional [CHAR[WIDTH]]
                        if j + 1 < bytes.len() {
                            let rest = &arg[j + 1..];
                            let mut chars = rest.chars();
                            if let Some(ch) = chars.next() {
                                if !ch.is_ascii_digit() {
                                    let width_str: String = chars.collect();
                                    let width = width_str.parse().unwrap_or(8);
                                    cli.config.expand_tabs = Some((ch, width));
                                } else {
                                    let width: usize = rest.parse().unwrap_or(8);
                                    cli.config.expand_tabs = Some(('\t', width));
                                }
                            }
                            break;
                        }
                    }
                    b'F' | b'f' => cli.config.form_feed = true,
                    b'h' => {
                        i += 1;
                        if i < args.len() {
                            cli.config.header = Some(args[i].clone());
                        }
                        break;
                    }
                    b'i' => {
                        cli.config.output_tabs = Some(('\t', 8));
                        if j + 1 < bytes.len() {
                            let rest = &arg[j + 1..];
                            let mut chars = rest.chars();
                            if let Some(ch) = chars.next() {
                                if !ch.is_ascii_digit() {
                                    let width_str: String = chars.collect();
                                    let width = width_str.parse().unwrap_or(8);
                                    cli.config.output_tabs = Some((ch, width));
                                } else {
                                    let width: usize = rest.parse().unwrap_or(8);
                                    cli.config.output_tabs = Some(('\t', width));
                                }
                            }
                            break;
                        }
                    }
                    b'J' => cli.config.join_lines = true,
                    b'l' => {
                        i += 1;
                        if i < args.len() {
                            cli.config.page_length =
                                args[i].parse().unwrap_or(pr::DEFAULT_PAGE_LENGTH);
                        }
                        break;
                    }
                    b'm' => cli.config.merge = true,
                    b'n' => {
                        cli.config.number_lines = Some(('\t', 5));
                        if j + 1 < bytes.len() {
                            let rest = &arg[j + 1..];
                            let mut chars = rest.chars();
                            if let Some(ch) = chars.next() {
                                if !ch.is_ascii_digit() {
                                    let digits_str: String = chars.collect();
                                    let digits = digits_str.parse().unwrap_or(5);
                                    cli.config.number_lines = Some((ch, digits));
                                } else {
                                    let digits: usize = rest.parse().unwrap_or(5);
                                    cli.config.number_lines = Some(('\t', digits));
                                }
                            }
                            break;
                        }
                    }
                    b'N' => {
                        i += 1;
                        if i < args.len() {
                            cli.config.first_line_number = args[i].parse().unwrap_or(1);
                        }
                        break;
                    }
                    b'o' => {
                        i += 1;
                        if i < args.len() {
                            cli.config.indent = args[i].parse().unwrap_or(0);
                        }
                        break;
                    }
                    b'r' => cli.config.no_file_warnings = true,
                    b's' => {
                        if j + 1 < bytes.len() {
                            cli.config.separator = Some(arg.as_bytes()[j + 1] as char);
                            break;
                        } else {
                            cli.config.separator = Some('\t');
                        }
                    }
                    b'S' => {
                        if j + 1 < bytes.len() {
                            cli.config.sep_string = Some(arg[j + 1..].to_string());
                            break;
                        } else {
                            cli.config.sep_string = Some(String::new());
                        }
                    }
                    b't' => cli.config.omit_header = true,
                    b'T' => {
                        cli.config.omit_pagination = true;
                        cli.config.omit_header = true;
                    }
                    b'v' => cli.config.show_nonprinting = true,
                    b'w' => {
                        i += 1;
                        if i < args.len() {
                            cli.config.page_width =
                                args[i].parse().unwrap_or(pr::DEFAULT_PAGE_WIDTH);
                        }
                        break;
                    }
                    b'W' => {
                        i += 1;
                        if i < args.len() {
                            cli.config.page_width =
                                args[i].parse().unwrap_or(pr::DEFAULT_PAGE_WIDTH);
                            cli.config.truncate_lines = true;
                        }
                        break;
                    }
                    _ => {
                        eprintln!("pr: invalid option -- '{}'", bytes[j] as char);
                        eprintln!("Try 'pr --help' for more information.");
                        process::exit(1);
                    }
                }
                j += 1;
            }
        } else {
            cli.files.push(arg.clone());
        }
        i += 1;
    }

    // Provide a mutable borrow scope to avoid conflict
    let _ = &args;

    cli
}

fn print_help() {
    print!(
        "Usage: pr [OPTION]... [FILE]...\n\
         Paginate or columnate FILE(s) for printing.\n\n\
         With no FILE, or when FILE is -, read standard input.\n\n\
         Mandatory arguments to long options are mandatory for short options too.\n\
         \x20 +FIRST_PAGE[:LAST_PAGE], --pages=FIRST_PAGE[:LAST_PAGE]\n\
         \x20                          begin [stop] printing with page FIRST_[:LAST_]PAGE\n\
         \x20 -COLUMN, --columns=COLUMN\n\
         \x20                          output COLUMN columns and print columns down\n\
         \x20 -a, --across             print columns across rather than down\n\
         \x20 -c, --show-control-chars use hat notation (^G) and octal backslash notation\n\
         \x20 -d, --double-space       double space the output\n\
         \x20 -D, --date-format=FORMAT use FORMAT for the header date\n\
         \x20 -e[CHAR[WIDTH]], --expand-tabs[=CHAR[WIDTH]]\n\
         \x20                          expand input CHARs (TABs) to tab WIDTH (8)\n\
         \x20 -F, -f, --form-feed      use form feeds instead of newlines to separate pages\n\
         \x20 -h, --header=HEADER      use HEADER instead of filename in page header\n\
         \x20 -i[CHAR[WIDTH]], --output-tabs[=CHAR[WIDTH]]\n\
         \x20                          replace spaces with CHARs (TABs) to tab WIDTH (8)\n\
         \x20 -J, --join-lines         merge full lines, turns off -W line truncation\n\
         \x20 -l, --length=PAGE_LENGTH set the page length to PAGE_LENGTH (66) lines\n\
         \x20 -m, --merge              print all files in parallel, one in each column\n\
         \x20 -n[SEP[DIGITS]], --number-lines[=SEP[DIGITS]]\n\
         \x20                          number lines, use DIGITS (5) digits, then SEP (TAB)\n\
         \x20 -N, --first-line-number=NUMBER\n\
         \x20                          start counting with NUMBER at 1st line of first page\n\
         \x20 -o, --indent=MARGIN      offset each line with MARGIN (zero) spaces\n\
         \x20 -r, --no-file-warnings   omit warning when a file cannot be opened\n\
         \x20 -s[CHAR], --separator[=CHAR]\n\
         \x20                          separate columns by a single character (TAB)\n\
         \x20 -S[STRING], --sep-string[=STRING]\n\
         \x20                          separate columns by STRING\n\
         \x20 -t, --omit-header        omit page headers and trailers\n\
         \x20 -T, --omit-pagination    omit page headers and trailers, eliminate form feeds\n\
         \x20 -v, --show-nonprinting   use octal backslash notation\n\
         \x20 -w, --page-width=PAGE_WIDTH\n\
         \x20                          set page width to PAGE_WIDTH (72) columns\n\
         \x20 -W, --page-width=PAGE_WIDTH\n\
         \x20                          set page width to PAGE_WIDTH (72) columns, truncate lines\n\
         \x20     --help               display this help and exit\n\
         \x20     --version            output version information and exit\n"
    );
}

fn file_mod_time(path: &str) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn main() {
    reset_sigpipe();

    let cli = parse_args();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files
    };

    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(64 * 1024, stdout.lock());
    let mut had_error = false;

    if cli.config.merge {
        // Merge mode: read all files and print side by side
        let mut all_inputs: Vec<Vec<String>> = Vec::new();
        let mut filenames: Vec<String> = Vec::new();
        let mut dates: Vec<SystemTime> = Vec::new();

        for filename in &files {
            let lines: Vec<String> = if filename == "-" {
                let stdin = io::stdin();
                stdin.lock().lines().map_while(|l| l.ok()).collect()
            } else {
                match fs::File::open(filename) {
                    Ok(f) => {
                        let reader = BufReader::new(f);
                        reader.lines().map_while(|l| l.ok()).collect()
                    }
                    Err(e) => {
                        if !cli.config.no_file_warnings {
                            eprintln!("pr: {}: {}", filename, io_error_msg(&e));
                        }
                        had_error = true;
                        continue;
                    }
                }
            };
            let date = if filename == "-" {
                SystemTime::now()
            } else {
                file_mod_time(filename).unwrap_or_else(SystemTime::now)
            };
            all_inputs.push(lines);
            filenames.push(filename.clone());
            dates.push(date);
        }

        let name_refs: Vec<&str> = filenames.iter().map(|s| s.as_str()).collect();
        if let Err(e) = pr::pr_merge(&all_inputs, &mut out, &cli.config, &name_refs, &dates) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("pr: write error: {}", io_error_msg(&e));
            had_error = true;
        }
    } else {
        for filename in &files {
            if filename == "-" {
                let stdin = io::stdin();
                let reader = BufReader::new(stdin.lock());
                let date = SystemTime::now();
                if let Err(e) =
                    pr::pr_file(reader, &mut out, &cli.config, "", Some(date))
                {
                    if e.kind() == io::ErrorKind::BrokenPipe {
                        let _ = out.flush();
                        process::exit(0);
                    }
                    eprintln!("pr: write error: {}", io_error_msg(&e));
                    had_error = true;
                }
            } else {
                match fs::File::open(filename) {
                    Ok(f) => {
                        let reader = BufReader::new(f);
                        let date = file_mod_time(filename);
                        if let Err(e) =
                            pr::pr_file(reader, &mut out, &cli.config, filename, date)
                        {
                            if e.kind() == io::ErrorKind::BrokenPipe {
                                let _ = out.flush();
                                process::exit(0);
                            }
                            eprintln!("pr: write error: {}", io_error_msg(&e));
                            had_error = true;
                        }
                    }
                    Err(e) => {
                        if !cli.config.no_file_warnings {
                            eprintln!("pr: {}: {}", filename, io_error_msg(&e));
                        }
                        had_error = true;
                    }
                }
            }
        }
    }

    let _ = out.flush();

    if had_error {
        process::exit(1);
    }
}
