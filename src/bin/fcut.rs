use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::process;

use clap::Parser;

use coreutils_rs::common::io::read_file;
use coreutils_rs::cut::{self, CutMode};

#[derive(Parser)]
#[command(name = "fcut", about = "Remove sections from each line of files")]
struct Cli {
    /// Select only these bytes
    #[arg(short = 'b', long = "bytes", value_name = "LIST")]
    bytes: Option<String>,

    /// Select only these characters
    #[arg(short = 'c', long = "characters", value_name = "LIST")]
    characters: Option<String>,

    /// Select only these fields
    #[arg(short = 'f', long = "fields", value_name = "LIST")]
    fields: Option<String>,

    /// Use DELIM instead of TAB for field delimiter
    #[arg(short = 'd', long = "delimiter", value_name = "DELIM")]
    delimiter: Option<String>,

    /// Complement the set of selected bytes, characters, or fields
    #[arg(long = "complement")]
    complement: bool,

    /// Do not print lines not containing delimiters
    #[arg(short = 's', long = "only-delimited")]
    only_delimited: bool,

    /// Use STRING as the output delimiter
    #[arg(long = "output-delimiter", value_name = "STRING")]
    output_delimiter: Option<String>,

    /// Line delimiter is NUL, not newline
    #[arg(short = 'z', long = "zero-terminated")]
    zero_terminated: bool,

    /// (ignored, for historical compatibility)
    #[arg(short = 'n', hide = true)]
    _legacy_n: bool,

    /// Files to process
    files: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    // Determine mode
    let mode_count =
        cli.bytes.is_some() as u8 + cli.characters.is_some() as u8 + cli.fields.is_some() as u8;
    if mode_count == 0 {
        eprintln!("fcut: you must specify a list of bytes, characters, or fields");
        eprintln!("Try 'fcut --help' for more information.");
        process::exit(1);
    }
    if mode_count > 1 {
        eprintln!("fcut: only one type of list may be specified");
        eprintln!("Try 'fcut --help' for more information.");
        process::exit(1);
    }

    let (mode, spec) = if let Some(ref s) = cli.bytes {
        (CutMode::Bytes, s.as_str())
    } else if let Some(ref s) = cli.characters {
        (CutMode::Characters, s.as_str())
    } else {
        (CutMode::Fields, cli.fields.as_ref().unwrap().as_str())
    };

    let ranges = match cut::parse_ranges(spec) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("fcut: {}", e);
            process::exit(1);
        }
    };

    let delim = if let Some(ref d) = cli.delimiter {
        if d.len() != 1 {
            eprintln!("fcut: the delimiter must be a single character");
            eprintln!("Try 'fcut --help' for more information.");
            process::exit(1);
        }
        d.as_bytes()[0]
    } else {
        b'\t'
    };

    // Default output delimiter: field delimiter for -f, empty for -b/-c
    // GNU cut only uses a delimiter between fields, not between byte/char ranges
    let output_delim = if let Some(ref od) = cli.output_delimiter {
        od.as_bytes().to_vec()
    } else if mode == CutMode::Fields {
        vec![delim]
    } else {
        vec![]
    };

    let line_delim = if cli.zero_terminated { b'\0' } else { b'\n' };

    let files = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut had_error = false;

    let cfg = cut::CutConfig {
        mode,
        ranges: &ranges,
        complement: cli.complement,
        delim,
        output_delim: &output_delim,
        suppress_no_delim: cli.only_delimited,
        line_delim,
    };

    for filename in &files {
        let result: io::Result<()> = if filename == "-" {
            let reader = BufReader::new(io::stdin().lock());
            cut::process_cut_reader(reader, &cfg, &mut out)
        } else {
            match read_file(Path::new(filename)) {
                Ok(data) => cut::process_cut_data(&data, &cfg, &mut out),
                Err(e) => {
                    eprintln!("fcut: {}: {}", filename, e);
                    had_error = true;
                    continue;
                }
            }
        };

        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("fcut: write error: {}", e);
            had_error = true;
        }
    }

    if let Err(e) = out.flush() {
        if e.kind() == io::ErrorKind::BrokenPipe {
            process::exit(0);
        }
        eprintln!("fcut: write error: {}", e);
        had_error = true;
    }

    if had_error {
        process::exit(1);
    }
}
