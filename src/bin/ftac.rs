use std::io;

use std::path::Path;
use std::process;

use clap::Parser;

use coreutils_rs::common::io::{FileData, read_file, read_stdin};
use coreutils_rs::tac;

#[derive(Parser)]
#[command(
    name = "ftac",
    about = "Concatenate and print files in reverse",
    version
)]
struct Cli {
    /// Attach the separator before instead of after
    #[arg(short = 'b', long = "before")]
    before: bool,

    /// Interpret the separator as a regular expression
    #[arg(short = 'r', long = "regex")]
    regex: bool,

    /// Use STRING as the separator instead of newline
    #[arg(
        short = 's',
        long = "separator",
        value_name = "STRING",
        allow_hyphen_values = true
    )]
    separator: Option<String>,

    /// Files to process (reads stdin if none given)
    files: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    let files: Vec<String> = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut had_error = false;

    for filename in &files {
        // Read file data — zero-copy mmap for files, buffered read for stdin
        let data: FileData = if filename == "-" {
            match read_stdin() {
                Ok(d) => FileData::Owned(d),
                Err(e) => {
                    eprintln!("ftac: standard input: {}", e);
                    had_error = true;
                    continue;
                }
            }
        } else {
            match read_file(Path::new(filename)) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("ftac: {}: {}", filename, e);
                    had_error = true;
                    continue;
                }
            }
        };

        let bytes: &[u8] = &data;

        let result = if cli.regex {
            let sep = cli.separator.as_deref().unwrap_or("\n");
            tac::tac_regex_separator(bytes, sep, cli.before, &mut out)
        } else if let Some(ref sep) = cli.separator {
            tac::tac_string_separator(bytes, sep.as_bytes(), cli.before, &mut out)
        } else {
            tac::tac_bytes(bytes, b'\n', cli.before, &mut out)
        };

        if let Err(e) = result {
            // Handle broken pipe gracefully
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("ftac: write error: {}", e);
            had_error = true;
        }
    }

    if had_error {
        process::exit(1);
    }
}
