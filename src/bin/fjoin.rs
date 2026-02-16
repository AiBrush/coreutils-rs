use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process;

use coreutils_rs::common::io::{read_file, read_stdin};
use coreutils_rs::common::io_error_msg;
use coreutils_rs::join::{self, JoinConfig, OrderCheck, OutputSpec};

struct Cli {
    config: JoinConfig,
    files: Vec<String>,
}

fn parse_field_num(s: &str, flag: &str) -> usize {
    match s.parse::<usize>() {
        Ok(0) => {
            eprintln!("join: invalid field number: '{}'", s);
            process::exit(1);
        }
        Ok(n) => n - 1, // Convert to 0-indexed
        Err(_) => {
            eprintln!("join: invalid field number for '{}': '{}'", flag, s);
            process::exit(1);
        }
    }
}

fn parse_output_format(s: &str) -> Vec<OutputSpec> {
    let mut specs = Vec::new();
    for token in s.split([',', ' ']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if token == "0" {
            specs.push(OutputSpec::JoinField);
        } else if let Some((file_str, field_str)) = token.split_once('.') {
            let file_num: usize = match file_str.parse() {
                Ok(n) if n == 1 || n == 2 => n,
                _ => {
                    eprintln!("join: invalid file number in field spec '{}'", token);
                    process::exit(1);
                }
            };
            let field_num: usize = match field_str.parse() {
                Ok(0) => {
                    // Field 0 means join field
                    specs.push(OutputSpec::JoinField);
                    continue;
                }
                Ok(n) => n,
                Err(_) => {
                    eprintln!("join: invalid field number in field spec '{}'", token);
                    process::exit(1);
                }
            };
            specs.push(OutputSpec::FileField(file_num - 1, field_num - 1));
        } else {
            eprintln!("join: invalid field specification '{}'", token);
            process::exit(1);
        }
    }
    specs
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        config: JoinConfig::default(),
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
            match bytes {
                b"--check-order" => cli.config.order_check = OrderCheck::Strict,
                b"--nocheck-order" => cli.config.order_check = OrderCheck::None,
                b"--header" => cli.config.header = true,
                b"--ignore-case" => cli.config.case_insensitive = true,
                b"--zero-terminated" => cli.config.zero_terminated = true,
                b"--help" => {
                    print_help();
                    process::exit(0);
                }
                b"--version" => {
                    println!("join (fcoreutils) {}", env!("CARGO_PKG_VERSION"));
                    process::exit(0);
                }
                _ => {
                    eprintln!("join: unrecognized option '{}'", s);
                    eprintln!("Try 'join --help' for more information.");
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' && bytes != b"-" {
            // Short options
            let s = arg.to_string_lossy();
            let chars_bytes = &bytes[1..];

            match chars_bytes[0] {
                b'a' => {
                    let val = if chars_bytes.len() > 1 {
                        String::from_utf8_lossy(&chars_bytes[1..]).into_owned()
                    } else {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("join: option requires an argument -- 'a'");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    };
                    match val.as_str() {
                        "1" => cli.config.print_unpaired1 = true,
                        "2" => cli.config.print_unpaired2 = true,
                        _ => {
                            eprintln!("join: invalid file number: '{}'", val);
                            process::exit(1);
                        }
                    }
                }
                b'v' => {
                    let val = if chars_bytes.len() > 1 {
                        String::from_utf8_lossy(&chars_bytes[1..]).into_owned()
                    } else {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("join: option requires an argument -- 'v'");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    };
                    match val.as_str() {
                        "1" => cli.config.only_unpaired1 = true,
                        "2" => cli.config.only_unpaired2 = true,
                        _ => {
                            eprintln!("join: invalid file number: '{}'", val);
                            process::exit(1);
                        }
                    }
                }
                b'e' => {
                    let val = if chars_bytes.len() > 1 {
                        String::from_utf8_lossy(&chars_bytes[1..]).into_owned()
                    } else {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("join: option requires an argument -- 'e'");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    };
                    cli.config.empty_filler = Some(val.into_bytes());
                }
                b'i' => {
                    cli.config.case_insensitive = true;
                }
                b'j' => {
                    let val = if chars_bytes.len() > 1 {
                        String::from_utf8_lossy(&chars_bytes[1..]).into_owned()
                    } else {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("join: option requires an argument -- 'j'");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    };
                    let field = parse_field_num(&val, "-j");
                    cli.config.field1 = field;
                    cli.config.field2 = field;
                }
                b'o' => {
                    let val = if chars_bytes.len() > 1 {
                        String::from_utf8_lossy(&chars_bytes[1..]).into_owned()
                    } else {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("join: option requires an argument -- 'o'");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    };
                    if val == "auto" {
                        cli.config.auto_format = true;
                    } else {
                        let specs = parse_output_format(&val);
                        if let Some(ref mut existing) = cli.config.output_format {
                            existing.extend(specs);
                        } else {
                            cli.config.output_format = Some(specs);
                        }
                    }
                }
                b't' => {
                    let val = if chars_bytes.len() > 1 {
                        chars_bytes[1]
                    } else {
                        let next = args.next().unwrap_or_else(|| {
                            eprintln!("join: option requires an argument -- 't'");
                            process::exit(1);
                        });
                        let b = next.as_encoded_bytes();
                        if b.is_empty() {
                            eprintln!("join: empty separator");
                            process::exit(1);
                        }
                        b[0]
                    };
                    cli.config.separator = Some(val);
                }
                b'z' => {
                    cli.config.zero_terminated = true;
                }
                b'1' => {
                    let val = if chars_bytes.len() > 1 {
                        String::from_utf8_lossy(&chars_bytes[1..]).into_owned()
                    } else {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("join: option requires an argument -- '1'");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    };
                    cli.config.field1 = parse_field_num(&val, "-1");
                }
                b'2' => {
                    let val = if chars_bytes.len() > 1 {
                        String::from_utf8_lossy(&chars_bytes[1..]).into_owned()
                    } else {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("join: option requires an argument -- '2'");
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    };
                    cli.config.field2 = parse_field_num(&val, "-2");
                }
                _ => {
                    eprintln!(
                        "join: invalid option -- '{}'",
                        s.chars().nth(1).unwrap_or('?')
                    );
                    eprintln!("Try 'join --help' for more information.");
                    process::exit(1);
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
        "Usage: join [OPTION]... FILE1 FILE2\n\
         For each pair of input lines with identical join fields, write a line to\n\
         standard output.  The default join field is the first, delimited by blanks.\n\n\
         When FILE1 or FILE2 (not both) is -, read standard input.\n\n\
         \x20 -a FILENUM        also print unpairable lines from file FILENUM, where\n\
         \x20                     FILENUM is 1 or 2, corresponding to FILE1 or FILE2\n\
         \x20 -e EMPTY          replace missing input fields with EMPTY\n\
         \x20 -i, --ignore-case ignore differences in case when comparing fields\n\
         \x20 -j FIELD          equivalent to '-1 FIELD -2 FIELD'\n\
         \x20 -o FORMAT         obey FORMAT while constructing output line\n\
         \x20 -t CHAR           use CHAR as input and output field separator\n\
         \x20 -v FILENUM        like -a FILENUM, but suppress joined output lines\n\
         \x20 -1 FIELD          join on this FIELD of file 1\n\
         \x20 -2 FIELD          join on this FIELD of file 2\n\
         \x20 --check-order     check that the input is correctly sorted, even\n\
         \x20                     if all input lines are pairable\n\
         \x20 --nocheck-order   do not check that the input is correctly sorted\n\
         \x20 --header          treat the first line in each file as field headers,\n\
         \x20                     print them without trying to pair them\n\
         \x20 -z, --zero-terminated    line delimiter is NUL, not newline\n\
         \x20     --help        display this help and exit\n\
         \x20     --version     output version information and exit\n\n\
         Unless -t CHAR is given, leading blanks separate fields and are ignored\n\
         in comparison.  Otherwise, fields are separated by CHAR.\n"
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
    let tool_name = "join";

    if cli.files.is_empty() {
        eprintln!("{}: missing operand", tool_name);
        eprintln!("Try 'join --help' for more information.");
        process::exit(1);
    }
    if cli.files.len() == 1 {
        eprintln!("{}: missing operand after '{}'", tool_name, cli.files[0]);
        eprintln!("Try 'join --help' for more information.");
        process::exit(1);
    }
    if cli.files.len() > 2 {
        eprintln!("{}: extra operand '{}'", tool_name, cli.files[2]);
        eprintln!("Try 'join --help' for more information.");
        process::exit(1);
    }

    let data1 = read_input(&cli.files[0], tool_name);
    let data2 = read_input(&cli.files[1], tool_name);

    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(256 * 1024, stdout.lock());

    match join::join(&data1, &data2, &cli.config, tool_name, &mut out) {
        Ok(had_order_error) => {
            if let Err(e) = out.flush() {
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("{}: write error: {}", tool_name, io_error_msg(&e));
                }
                process::exit(1);
            }
            if had_order_error {
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
