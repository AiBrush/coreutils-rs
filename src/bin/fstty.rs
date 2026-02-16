#[cfg(not(unix))]
fn main() {
    eprintln!("stty: only available on Unix");
    std::process::exit(1);
}

#[cfg(unix)]
use std::process;

#[cfg(unix)]
const TOOL_NAME: &str = "stty";
#[cfg(unix)]
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(unix)]
fn print_help() {
    println!("Usage: {} [-F DEVICE | --file=DEVICE] [SETTING]...", TOOL_NAME);
    println!("  or:  {} [-F DEVICE | --file=DEVICE] [-a|--all]", TOOL_NAME);
    println!("Print or change terminal line settings.");
    println!();
    println!("  -a, --all          print all current settings in human-readable form");
    println!("  -F, --file=DEVICE  open and use the specified DEVICE instead of stdin");
    println!("      --help         display this help and exit");
    println!("      --version      output version information and exit");
    println!();
    println!("Special settings:");
    println!("  size       print the number of rows and columns");
    println!("  speed      print the terminal speed");
    println!("  sane       reset all settings to reasonable values");
    println!("  raw        set raw mode");
    println!("  cooked     set cooked mode (same as -raw)");
    println!();
    println!("Special characters:");
    println!("  intr CHAR   interrupt character (default ^C)");
    println!("  quit CHAR   quit character (default ^\\)");
    println!("  erase CHAR  erase character (default ^?)");
    println!("  kill CHAR   kill character (default ^U)");
    println!("  eof CHAR    end-of-file character (default ^D)");
    println!("  start CHAR  start character (default ^Q)");
    println!("  stop CHAR   stop character (default ^S)");
    println!("  susp CHAR   suspend character (default ^Z)");
    println!();
    println!("Control settings: [-]cread [-]clocal [-]hupcl [-]cstopb [-]parenb [-]parodd");
    println!("  cs5 cs6 cs7 cs8");
    println!();
    println!("Input settings: [-]ignbrk [-]brkint [-]ignpar [-]parmrk [-]inpck [-]istrip");
    println!("  [-]inlcr [-]igncr [-]icrnl [-]ixon [-]ixany [-]ixoff [-]imaxbel [-]iutf8");
    println!();
    println!("Output settings: [-]opost [-]olcuc [-]onlcr [-]ocrnl [-]onocr [-]onlret");
    println!("  [-]ofill [-]ofdel");
    println!();
    println!("Local settings: [-]isig [-]icanon [-]iexten [-]echo [-]echoe [-]echok");
    println!("  [-]echonl [-]noflsh [-]tostop [-]echoctl [-]echoprt [-]echoke [-]xcase");
    println!();
    println!("Speed: ispeed N  ospeed N  N (set both)");
}

#[cfg(unix)]
fn main() {
    coreutils_rs::common::reset_sigpipe();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Check for --help and --version first
    for arg in &args {
        match arg.as_str() {
            "--help" => {
                print_help();
                return;
            }
            "--version" => {
                println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                return;
            }
            _ => {}
        }
    }

    let config = match coreutils_rs::stty::parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: {}", TOOL_NAME, e);
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
    };

    // Determine the file descriptor to use
    let (fd, _owned) = if let Some(ref dev) = config.device {
        match coreutils_rs::stty::open_device(dev) {
            Ok(fd) => (fd, true),
            Err(e) => {
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    dev,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    } else {
        (0i32, false) // stdin
    };

    match config.action {
        coreutils_rs::stty::SttyAction::PrintSize => {
            if let Err(e) = coreutils_rs::stty::print_size(fd) {
                let src = config.device.as_deref().unwrap_or("standard input");
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    src,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
        coreutils_rs::stty::SttyAction::PrintSpeed => {
            let termios = match coreutils_rs::stty::get_termios(fd) {
                Ok(t) => t,
                Err(e) => {
                    let src = config.device.as_deref().unwrap_or("standard input");
                    eprintln!(
                        "{}: {}: {}",
                        TOOL_NAME,
                        src,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    process::exit(1);
                }
            };
            coreutils_rs::stty::print_speed(&termios);
        }
        coreutils_rs::stty::SttyAction::PrintAll => {
            let termios = match coreutils_rs::stty::get_termios(fd) {
                Ok(t) => t,
                Err(e) => {
                    let src = config.device.as_deref().unwrap_or("standard input");
                    eprintln!(
                        "{}: {}: {}",
                        TOOL_NAME,
                        src,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    process::exit(1);
                }
            };
            coreutils_rs::stty::print_all(&termios, fd);
        }
        coreutils_rs::stty::SttyAction::ApplySettings => {
            let mut termios = match coreutils_rs::stty::get_termios(fd) {
                Ok(t) => t,
                Err(e) => {
                    let src = config.device.as_deref().unwrap_or("standard input");
                    eprintln!(
                        "{}: {}: {}",
                        TOOL_NAME,
                        src,
                        coreutils_rs::common::io_error_msg(&e)
                    );
                    process::exit(1);
                }
            };
            match coreutils_rs::stty::apply_settings(&mut termios, &config.settings) {
                Ok(changed) => {
                    if changed && let Err(e) = coreutils_rs::stty::set_termios(fd, &termios) {
                        let src =
                            config.device.as_deref().unwrap_or("standard input");
                        eprintln!(
                            "{}: {}: {}",
                            TOOL_NAME,
                            src,
                            coreutils_rs::common::io_error_msg(&e)
                        );
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("{}: {}", TOOL_NAME, e);
                    process::exit(1);
                }
            }
        }
    }

    // Close owned fd
    if _owned {
        unsafe {
            libc::close(fd);
        }
    }
}
