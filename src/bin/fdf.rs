use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::df::{DfConfig, parse_block_size, parse_output_fields, run_df};

const TOOL_NAME: &str = "df";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Take the next value for an option from the rest of the current arg or the next arg.
fn take_value(
    bytes: &[u8],
    pos: usize,
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> String {
    if pos < bytes.len() {
        let full = String::from_utf8_lossy(bytes).into_owned();
        full[pos..].to_string()
    } else {
        args.next()
            .unwrap_or_else(|| {
                eprintln!("{}: option requires an argument -- '{}'", TOOL_NAME, flag);
                process::exit(1);
            })
            .to_string_lossy()
            .into_owned()
    }
}

fn parse_args() -> DfConfig {
    let mut config = DfConfig::default();
    let mut args = std::env::args_os().skip(1);

    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();

        if bytes == b"--" {
            // Everything after -- is a file argument.
            for a in args {
                config.files.push(a.to_string_lossy().into_owned());
            }
            break;
        }

        if bytes.starts_with(b"--") {
            let arg_str = arg.to_string_lossy();
            let long = &arg_str[2..];

            // Handle --option=value form.
            let (opt, val) = if let Some(eq_pos) = long.find('=') {
                (&long[..eq_pos], Some(&long[eq_pos + 1..]))
            } else {
                (long, None)
            };

            match opt {
                "all" => config.all = true,
                "block-size" => {
                    let v = val.map(|s| s.to_string()).unwrap_or_else(|| {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("{}: option '--block-size' requires an argument", TOOL_NAME);
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    });
                    match parse_block_size(&v) {
                        Ok(bs) => config.block_size = bs,
                        Err(e) => {
                            eprintln!("{}: {}", TOOL_NAME, e);
                            process::exit(1);
                        }
                    }
                }
                "human-readable" => config.human_readable = true,
                "si" => config.si = true,
                "inodes" => config.inodes = true,
                "local" => config.local_only = true,
                "no-sync" => config.sync_before = false,
                "sync" => config.sync_before = true,
                "output" => {
                    if let Some(v) = val {
                        match parse_output_fields(v) {
                            Ok(fields) => config.output_fields = Some(fields),
                            Err(e) => {
                                eprintln!("{}: {}", TOOL_NAME, e);
                                process::exit(1);
                            }
                        }
                    } else {
                        // --output without =LIST means all fields.
                        config.output_fields = Some(
                            coreutils_rs::df::VALID_OUTPUT_FIELDS
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                        );
                    }
                }
                "portability" => config.portability = true,
                "print-type" => config.print_type = true,
                "total" => config.total = true,
                "type" => {
                    let v = val.map(|s| s.to_string()).unwrap_or_else(|| {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!("{}: option '--type' requires an argument", TOOL_NAME);
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    });
                    config.type_filter.insert(v);
                }
                "exclude-type" => {
                    let v = val.map(|s| s.to_string()).unwrap_or_else(|| {
                        args.next()
                            .unwrap_or_else(|| {
                                eprintln!(
                                    "{}: option '--exclude-type' requires an argument",
                                    TOOL_NAME
                                );
                                process::exit(1);
                            })
                            .to_string_lossy()
                            .into_owned()
                    });
                    config.exclude_type.insert(v);
                }
                "help" => {
                    print_help();
                    process::exit(0);
                }
                "version" => {
                    println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                    process::exit(0);
                }
                _ => {
                    eprintln!("{}: unrecognized option '--{}'", TOOL_NAME, opt);
                    eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                    process::exit(1);
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            // Short options.
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'a' => config.all = true,
                    b'B' => {
                        let v = take_value(bytes, i + 1, &mut args, "B");
                        match parse_block_size(&v) {
                            Ok(bs) => config.block_size = bs,
                            Err(e) => {
                                eprintln!("{}: {}", TOOL_NAME, e);
                                process::exit(1);
                            }
                        }
                        break;
                    }
                    b'h' => config.human_readable = true,
                    b'H' => config.si = true,
                    b'i' => config.inodes = true,
                    b'k' => config.block_size = 1024,
                    b'l' => config.local_only = true,
                    b'P' => config.portability = true,
                    b'T' => config.print_type = true,
                    b't' => {
                        let v = take_value(bytes, i + 1, &mut args, "t");
                        config.type_filter.insert(v);
                        break;
                    }
                    b'x' => {
                        let v = take_value(bytes, i + 1, &mut args, "x");
                        config.exclude_type.insert(v);
                        break;
                    }
                    _ => {
                        eprintln!(
                            "{}: invalid option -- '{}'",
                            TOOL_NAME, bytes[i] as char
                        );
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
                i += 1;
            }
        } else {
            config.files.push(arg.to_string_lossy().into_owned());
        }
    }

    config
}

fn print_help() {
    print!(
        "Usage: {0} [OPTION]... [FILE]...\n\
         Show information about the file system on which each FILE resides,\n\
         or all file systems by default.\n\n\
         Mandatory arguments to long options are mandatory for short options too.\n\
         \x20 -a, --all             include pseudo, duplicate, inaccessible file systems\n\
         \x20 -B, --block-size=SIZE  scale sizes by SIZE before printing them; e.g.,\n\
         \x20                         '-BM' prints sizes in units of 1,048,576 bytes\n\
         \x20 -h, --human-readable  print sizes in powers of 1024 (e.g., 1023M)\n\
         \x20 -H, --si              print sizes in powers of 1000 (e.g., 1.1G)\n\
         \x20 -i, --inodes          list inode information instead of block usage\n\
         \x20 -k                    like --block-size=1K\n\
         \x20 -l, --local           limit listing to local file systems\n\
         \x20     --no-sync         do not invoke sync before getting usage info (default)\n\
         \x20     --output[=FIELD_LIST]  use the output format defined by FIELD_LIST,\n\
         \x20                         or print all fields if FIELD_LIST is omitted.\n\
         \x20 -P, --portability     use the POSIX output format\n\
         \x20     --sync            invoke sync before getting usage info\n\
         \x20     --total           elicit a grand total\n\
         \x20 -t, --type=TYPE       limit listing to file systems of type TYPE\n\
         \x20 -T, --print-type      print file system type\n\
         \x20 -x, --exclude-type=TYPE   limit listing to file systems not of type TYPE\n\
         \x20     --help            display this help and exit\n\
         \x20     --version         output version information and exit\n\n\
         FIELD_LIST is a comma-separated list of columns to be included.  Valid\n\
         field names are: 'source', 'fstype', 'itotal', 'iused', 'iavail',\n\
         'ipcent', 'size', 'used', 'avail', 'pcent', 'file' and 'target'\n",
        TOOL_NAME
    );
}

fn main() {
    reset_sigpipe();

    let config = parse_args();
    let exit_code = run_df(&config);
    process::exit(exit_code);
}
