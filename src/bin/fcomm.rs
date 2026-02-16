use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process;

use coreutils_rs::comm::{self, CommConfig, OrderCheck};
use coreutils_rs::common::io::{read_file, read_stdin};
use coreutils_rs::common::io_error_msg;

struct Cli {
    config: CommConfig,
    files: Vec<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        config: CommConfig::default(),
        files: Vec::new(),
    };

    let mut args = std::env::args_os().skip(1);
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            for a in args {
                cli.files.push(a.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            let s = arg.to_string_lossy();
            if let Some(val) = s.strip_prefix("--output-delimiter=") {
                cli.config.output_delimiter = Some(val.as_bytes().to_vec());
            } else {
                match bytes {
                    b"--case-insensitive" => cli.config.case_insensitive = true,
                    b"--check-order" => cli.config.order_check = OrderCheck::Strict,
                    b"--nocheck-order" => cli.config.order_check = OrderCheck::None,
                    b"--output-delimiter" => {
                        let val = args.next().unwrap_or_else(|| {
                            eprintln!("comm: option '--output-delimiter' requires an argument");
                            process::exit(1);
                        });
                        cli.config.output_delimiter = Some(val.as_encoded_bytes().to_vec());
                    }
                    b"--total" => cli.config.total = true,
                    b"--zero-terminated" => cli.config.zero_terminated = true,
                    b"--help" => {
                        print_help();
                        process::exit(0);
                    }
                    b"--version" => {
                        println!("comm (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                        process::exit(0);
                    }
                    _ => {
                        eprintln!("comm: unrecognized option '{}'", s);
                        eprintln!("Try 'comm --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // Short options: -1, -2, -3, -i, -z (can be combined)
            for &b in &bytes[1..] {
                match b {
                    b'1' => cli.config.suppress_col1 = true,
                    b'2' => cli.config.suppress_col2 = true,
                    b'3' => cli.config.suppress_col3 = true,
                    b'i' => cli.config.case_insensitive = true,
                    b'z' => cli.config.zero_terminated = true,
                    _ => {
                        eprintln!("comm: invalid option -- '{}'", b as char);
                        eprintln!("Try 'comm --help' for more information.");
                        process::exit(1);
                    }
                }
            }
        } else {
            cli.files.push(arg.to_string_lossy().into_owned());
        }
    }

    cli
}

fn print_help() {
    print!(
        "Usage: comm [OPTION]... FILE1 FILE2\n\
         Compare sorted files FILE1 and FILE2 line by line.\n\n\
         When FILE1 or FILE2 (not both) is -, read standard input.\n\n\
         With no options, produce three-column output.  Column one contains\n\
         lines unique to FILE1, column two contains lines unique to FILE2,\n\
         and column three contains lines common to both files.\n\n\
         \x20 -1              suppress column 1 (lines unique to FILE1)\n\
         \x20 -2              suppress column 2 (lines unique to FILE2)\n\
         \x20 -3              suppress column 3 (lines that appear in both files)\n\
         \x20 -i, --case-insensitive  ignore differences in case when comparing\n\
         \x20 --check-order   check that the input is correctly sorted, even\n\
         \x20                   if all input lines are pairable\n\
         \x20 --nocheck-order do not check that the input is correctly sorted\n\
         \x20 --output-delimiter=STR  separate columns with STR\n\
         \x20 --total          output a summary\n\
         \x20 -z, --zero-terminated    line delimiter is NUL, not newline\n\
         \x20     --help       display this help and exit\n\
         \x20     --version    output version information and exit\n"
    );
}

fn read_input(filename: &str, tool_name: &str) -> coreutils_rs::common::io::FileData {
    if filename == "-" {
        match read_stdin() {
            Ok(d) => coreutils_rs::common::io::FileData::Owned(d),
            Err(e) => {
                eprintln!("{}: standard input: {}", tool_name, io_error_msg(&e));
                process::exit(1);
            }
        }
    } else {
        match read_file(Path::new(filename)) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{}: {}: {}", tool_name, filename, io_error_msg(&e));
                process::exit(1);
            }
        }
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let cli = parse_args();
    let tool_name = "comm";

    if cli.files.is_empty() {
        eprintln!("{}: missing operand", tool_name);
        eprintln!("Try 'comm --help' for more information.");
        process::exit(1);
    }
    if cli.files.len() == 1 {
        eprintln!("{}: missing operand after '{}'", tool_name, cli.files[0]);
        eprintln!("Try 'comm --help' for more information.");
        process::exit(1);
    }
    if cli.files.len() > 2 {
        eprintln!("{}: extra operand '{}'", tool_name, cli.files[2]);
        eprintln!("Try 'comm --help' for more information.");
        process::exit(1);
    }

    let data1 = read_input(&cli.files[0], tool_name);
    let data2 = read_input(&cli.files[1], tool_name);

    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(256 * 1024, stdout.lock());

    match comm::comm(&data1, &data2, &cli.config, tool_name, &mut out) {
        Ok(result) => {
            if let Err(e) = out.flush() {
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("{}: write error: {}", tool_name, io_error_msg(&e));
                }
                process::exit(1);
            }
            if result.had_order_error {
                process::exit(1);
            }
        }
        Err(e) => {
            if e.kind() == io::ErrorKind::BrokenPipe {
                let _ = out.flush();
                process::exit(0);
            }
            eprintln!("{}: write error: {}", tool_name, io_error_msg(&e));
            process::exit(1);
        }
    }
}
