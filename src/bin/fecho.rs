use std::io::{self, Write};
use std::process;

use coreutils_rs::common::reset_sigpipe;
use coreutils_rs::echo::{echo_output, parse_echo_args};

fn main() {
    reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let (config, text_args) = parse_echo_args(&args);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Fast path: no escape interpretation — write args directly to stdout
    // avoiding intermediate Vec allocation entirely.
    if !config.interpret_escapes {
        let result = (|| -> io::Result<()> {
            for (i, arg) in text_args.iter().enumerate() {
                if i > 0 {
                    out.write_all(b" ")?;
                }
                out.write_all(arg.as_bytes())?;
            }
            if config.trailing_newline {
                out.write_all(b"\n")?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            if e.kind() == io::ErrorKind::BrokenPipe {
                process::exit(0);
            }
            eprintln!("echo: write error: {}", e);
            process::exit(1);
        }
        return;
    }

    // Slow path: escape interpretation needed
    let output = echo_output(text_args, &config);
    if let Err(e) = out.write_all(&output) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            process::exit(0);
        }
        eprintln!("echo: write error: {}", e);
        process::exit(1);
    }
}
