use std::io;

/// Convert a baud rate constant to its numeric value.
pub fn baud_to_num(speed: libc::speed_t) -> u32 {
    match speed {
        libc::B0 => 0,
        libc::B50 => 50,
        libc::B75 => 75,
        libc::B110 => 110,
        libc::B134 => 134,
        libc::B150 => 150,
        libc::B200 => 200,
        libc::B300 => 300,
        libc::B600 => 600,
        libc::B1200 => 1200,
        libc::B1800 => 1800,
        libc::B2400 => 2400,
        libc::B4800 => 4800,
        libc::B9600 => 9600,
        libc::B19200 => 19200,
        libc::B38400 => 38400,
        libc::B57600 => 57600,
        libc::B115200 => 115200,
        libc::B230400 => 230400,
        _ => 0,
    }
}

/// Convert a numeric baud value to the corresponding constant.
pub fn num_to_baud(num: u32) -> Option<libc::speed_t> {
    match num {
        0 => Some(libc::B0),
        50 => Some(libc::B50),
        75 => Some(libc::B75),
        110 => Some(libc::B110),
        134 => Some(libc::B134),
        150 => Some(libc::B150),
        200 => Some(libc::B200),
        300 => Some(libc::B300),
        600 => Some(libc::B600),
        1200 => Some(libc::B1200),
        1800 => Some(libc::B1800),
        2400 => Some(libc::B2400),
        4800 => Some(libc::B4800),
        9600 => Some(libc::B9600),
        19200 => Some(libc::B19200),
        38400 => Some(libc::B38400),
        57600 => Some(libc::B57600),
        115200 => Some(libc::B115200),
        230400 => Some(libc::B230400),
        _ => None,
    }
}

/// Get the termios structure for a file descriptor.
pub fn get_termios(fd: i32) -> io::Result<libc::termios> {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(termios)
}

/// Set the termios structure for a file descriptor.
pub fn set_termios(fd: i32, termios: &libc::termios) -> io::Result<()> {
    if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, termios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Get the window size for a file descriptor.
pub fn get_winsize(fd: i32) -> io::Result<libc::winsize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ws)
}

/// Print terminal size as "rows cols".
pub fn print_size(fd: i32) -> io::Result<()> {
    let ws = get_winsize(fd)?;
    println!("{} {}", ws.ws_row, ws.ws_col);
    Ok(())
}

/// Print terminal speed.
pub fn print_speed(termios: &libc::termios) {
    let ispeed = unsafe { libc::cfgetispeed(termios) };
    let ospeed = unsafe { libc::cfgetospeed(termios) };
    if ispeed == ospeed {
        println!("{}", baud_to_num(ospeed));
    } else {
        println!("{} {}", baud_to_num(ispeed), baud_to_num(ospeed));
    }
}

/// Format a control character for display.
pub fn format_cc(c: libc::cc_t) -> String {
    if c == 0 {
        "<undef>".to_string()
    } else if c == 0x7f {
        "^?".to_string()
    } else if c < 0x20 {
        format!("^{}", (c + 0x40) as char)
    } else {
        format!("{}", c as char)
    }
}

/// Special character names and their termios indices.
const SPECIAL_CHARS: &[(&str, usize)] = &[
    ("intr", libc::VINTR as usize),
    ("quit", libc::VQUIT as usize),
    ("erase", libc::VERASE as usize),
    ("kill", libc::VKILL as usize),
    ("eof", libc::VEOF as usize),
    ("eol", libc::VEOL as usize),
    ("eol2", libc::VEOL2 as usize),
    ("swtch", libc::VSWTC as usize),
    ("start", libc::VSTART as usize),
    ("stop", libc::VSTOP as usize),
    ("susp", libc::VSUSP as usize),
    ("rprnt", libc::VREPRINT as usize),
    ("werase", libc::VWERASE as usize),
    ("lnext", libc::VLNEXT as usize),
    ("discard", libc::VDISCARD as usize),
    ("min", libc::VMIN as usize),
    ("time", libc::VTIME as usize),
];

/// Input flags and their names.
const INPUT_FLAGS: &[(&str, libc::tcflag_t)] = &[
    ("ignbrk", libc::IGNBRK),
    ("brkint", libc::BRKINT),
    ("ignpar", libc::IGNPAR),
    ("parmrk", libc::PARMRK),
    ("inpck", libc::INPCK),
    ("istrip", libc::ISTRIP),
    ("inlcr", libc::INLCR),
    ("igncr", libc::IGNCR),
    ("icrnl", libc::ICRNL),
    ("iuclc", libc::IUCLC),
    ("ixon", libc::IXON),
    ("ixany", libc::IXANY),
    ("ixoff", libc::IXOFF),
    ("imaxbel", libc::IMAXBEL),
    ("iutf8", libc::IUTF8),
];

/// Output flags and their names.
const OUTPUT_FLAGS: &[(&str, libc::tcflag_t)] = &[
    ("opost", libc::OPOST),
    ("olcuc", libc::OLCUC),
    ("onlcr", libc::ONLCR),
    ("ocrnl", libc::OCRNL),
    ("onocr", libc::ONOCR),
    ("onlret", libc::ONLRET),
    ("ofill", libc::OFILL),
    ("ofdel", libc::OFDEL),
];

/// Control flags and their names.
const CONTROL_FLAGS: &[(&str, libc::tcflag_t)] = &[
    ("cread", libc::CREAD),
    ("clocal", libc::CLOCAL),
    ("hupcl", libc::HUPCL),
    ("cstopb", libc::CSTOPB),
    ("parenb", libc::PARENB),
    ("parodd", libc::PARODD),
];

/// Local flags and their names.
const LOCAL_FLAGS: &[(&str, libc::tcflag_t)] = &[
    ("isig", libc::ISIG),
    ("icanon", libc::ICANON),
    ("iexten", libc::IEXTEN),
    ("echo", libc::ECHO),
    ("echoe", libc::ECHOE),
    ("echok", libc::ECHOK),
    ("echonl", libc::ECHONL),
    ("echoctl", libc::ECHOCTL),
    ("echoprt", libc::ECHOPRT),
    ("echoke", libc::ECHOKE),
    ("noflsh", libc::NOFLSH),
    ("tostop", libc::TOSTOP),
    ("xcase", libc::XCASE),
];

/// Character size names.
fn csize_str(cflag: libc::tcflag_t) -> &'static str {
    match cflag & libc::CSIZE {
        libc::CS5 => "cs5",
        libc::CS6 => "cs6",
        libc::CS7 => "cs7",
        libc::CS8 => "cs8",
        _ => "cs8",
    }
}

/// Print all terminal settings (stty -a).
pub fn print_all(termios: &libc::termios, fd: i32) {
    let ispeed = unsafe { libc::cfgetispeed(termios) };
    let ospeed = unsafe { libc::cfgetospeed(termios) };

    // Line 1: speed and window size
    let speed_str = if ispeed == ospeed {
        format!("speed {} baud", baud_to_num(ospeed))
    } else {
        format!(
            "speed {} baud; ispeed {} baud; ospeed {} baud",
            baud_to_num(ospeed),
            baud_to_num(ispeed),
            baud_to_num(ospeed)
        )
    };
    if let Ok(ws) = get_winsize(fd) {
        println!("{}; rows {}; columns {};", speed_str, ws.ws_row, ws.ws_col);
    } else {
        println!("{};", speed_str);
    }

    // Line 2: special characters
    let mut cc_parts: Vec<String> = Vec::new();
    for &(name, idx) in SPECIAL_CHARS {
        cc_parts.push(format!("{} = {}", name, format_cc(termios.c_cc[idx])));
    }
    println!("{};", cc_parts.join("; "));

    // Input flags
    let mut parts: Vec<String> = Vec::new();
    for &(name, flag) in INPUT_FLAGS {
        if termios.c_iflag & flag != 0 {
            parts.push(name.to_string());
        } else {
            parts.push(format!("-{}", name));
        }
    }
    println!("{}", parts.join(" "));

    // Output flags
    parts.clear();
    for &(name, flag) in OUTPUT_FLAGS {
        if termios.c_oflag & flag != 0 {
            parts.push(name.to_string());
        } else {
            parts.push(format!("-{}", name));
        }
    }
    println!("{}", parts.join(" "));

    // Control flags
    parts.clear();
    parts.push(csize_str(termios.c_cflag).to_string());
    for &(name, flag) in CONTROL_FLAGS {
        if termios.c_cflag & flag != 0 {
            parts.push(name.to_string());
        } else {
            parts.push(format!("-{}", name));
        }
    }
    println!("{}", parts.join(" "));

    // Local flags
    parts.clear();
    for &(name, flag) in LOCAL_FLAGS {
        if termios.c_lflag & flag != 0 {
            parts.push(name.to_string());
        } else {
            parts.push(format!("-{}", name));
        }
    }
    println!("{}", parts.join(" "));
}

/// Parse a control character specification like "^C", "^?", "^-", or a literal.
pub fn parse_control_char(s: &str) -> Option<libc::cc_t> {
    if s == "^-" || s == "undef" {
        Some(0)
    } else if s == "^?" {
        Some(0x7f)
    } else if s.len() == 2 && s.starts_with('^') {
        let ch = s.as_bytes()[1];
        if ch >= b'@' && ch <= b'_' {
            Some(ch - b'@')
        } else if ch >= b'a' && ch <= b'z' {
            Some(ch - b'a' + 1)
        } else {
            None
        }
    } else if s.len() == 1 {
        Some(s.as_bytes()[0])
    } else {
        // Try parsing as decimal
        s.parse::<u8>().ok()
    }
}

/// Apply "sane" settings to a termios structure.
pub fn set_sane(termios: &mut libc::termios) {
    // Input flags
    termios.c_iflag = libc::BRKINT
        | libc::ICRNL
        | libc::IMAXBEL
        | libc::IXON
        | libc::IUTF8;

    // Output flags
    termios.c_oflag = libc::OPOST | libc::ONLCR;

    // Control flags: preserve baud rate, set cs8, cread, hupcl
    termios.c_cflag = (termios.c_cflag & (libc::CBAUD | libc::CBAUDEX))
        | libc::CS8
        | libc::CREAD
        | libc::HUPCL;

    // Local flags
    termios.c_lflag = libc::ISIG
        | libc::ICANON
        | libc::IEXTEN
        | libc::ECHO
        | libc::ECHOE
        | libc::ECHOK
        | libc::ECHOCTL
        | libc::ECHOKE;

    // Special characters
    termios.c_cc[libc::VINTR] = 0x03;    // ^C
    termios.c_cc[libc::VQUIT] = 0x1c;    // ^\
    termios.c_cc[libc::VERASE] = 0x7f;   // ^?
    termios.c_cc[libc::VKILL] = 0x15;    // ^U
    termios.c_cc[libc::VEOF] = 0x04;     // ^D
    termios.c_cc[libc::VSTART] = 0x11;   // ^Q
    termios.c_cc[libc::VSTOP] = 0x13;    // ^S
    termios.c_cc[libc::VSUSP] = 0x1a;    // ^Z
    termios.c_cc[libc::VREPRINT] = 0x12; // ^R
    termios.c_cc[libc::VWERASE] = 0x17;  // ^W
    termios.c_cc[libc::VLNEXT] = 0x16;   // ^V
    termios.c_cc[libc::VDISCARD] = 0x0f; // ^O
    termios.c_cc[libc::VMIN] = 1;
    termios.c_cc[libc::VTIME] = 0;
}

/// Set raw mode on a termios structure.
pub fn set_raw(termios: &mut libc::termios) {
    // Equivalent to cfmakeraw
    termios.c_iflag &= !(libc::IGNBRK
        | libc::BRKINT
        | libc::PARMRK
        | libc::ISTRIP
        | libc::INLCR
        | libc::IGNCR
        | libc::ICRNL
        | libc::IXON);
    termios.c_oflag &= !libc::OPOST;
    termios.c_lflag &= !(libc::ECHO
        | libc::ECHONL
        | libc::ICANON
        | libc::ISIG
        | libc::IEXTEN);
    termios.c_cflag &= !(libc::CSIZE | libc::PARENB);
    termios.c_cflag |= libc::CS8;
    termios.c_cc[libc::VMIN] = 1;
    termios.c_cc[libc::VTIME] = 0;
}

/// Set cooked mode (undo raw) on a termios structure.
pub fn set_cooked(termios: &mut libc::termios) {
    termios.c_iflag |= libc::BRKINT | libc::IGNPAR | libc::ICRNL | libc::IXON;
    termios.c_oflag |= libc::OPOST;
    termios.c_lflag |= libc::ISIG | libc::ICANON | libc::ECHO;
}

/// Open a device and return its file descriptor.
pub fn open_device(path: &str) -> io::Result<i32> {
    use std::ffi::CString;
    let cpath = CString::new(path).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "invalid device path")
    })?;
    let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Look up a special character name and return its index.
pub fn find_special_char(name: &str) -> Option<usize> {
    for &(n, idx) in SPECIAL_CHARS {
        if n == name {
            return Some(idx);
        }
    }
    None
}

/// Apply a single flag setting. Returns true if the argument was recognized.
pub fn apply_flag(termios: &mut libc::termios, name: &str) -> bool {
    let (negate, flag_name) = if let Some(stripped) = name.strip_prefix('-') {
        (true, stripped)
    } else {
        (false, name)
    };

    // Check input flags
    for &(n, flag) in INPUT_FLAGS {
        if n == flag_name {
            if negate {
                termios.c_iflag &= !flag;
            } else {
                termios.c_iflag |= flag;
            }
            return true;
        }
    }

    // Check output flags
    for &(n, flag) in OUTPUT_FLAGS {
        if n == flag_name {
            if negate {
                termios.c_oflag &= !flag;
            } else {
                termios.c_oflag |= flag;
            }
            return true;
        }
    }

    // Check control flags
    for &(n, flag) in CONTROL_FLAGS {
        if n == flag_name {
            if negate {
                termios.c_cflag &= !flag;
            } else {
                termios.c_cflag |= flag;
            }
            return true;
        }
    }

    // Check local flags
    for &(n, flag) in LOCAL_FLAGS {
        if n == flag_name {
            if negate {
                termios.c_lflag &= !flag;
            } else {
                termios.c_lflag |= flag;
            }
            return true;
        }
    }

    // Check character size
    match flag_name {
        "cs5" => {
            termios.c_cflag = (termios.c_cflag & !libc::CSIZE) | libc::CS5;
            return true;
        }
        "cs6" => {
            termios.c_cflag = (termios.c_cflag & !libc::CSIZE) | libc::CS6;
            return true;
        }
        "cs7" => {
            termios.c_cflag = (termios.c_cflag & !libc::CSIZE) | libc::CS7;
            return true;
        }
        "cs8" => {
            termios.c_cflag = (termios.c_cflag & !libc::CSIZE) | libc::CS8;
            return true;
        }
        _ => {}
    }

    false
}

/// The result of parsing stty arguments.
pub enum SttyAction {
    PrintAll,
    PrintSize,
    PrintSpeed,
    ApplySettings,
}

/// Parsed stty configuration.
pub struct SttyConfig {
    pub action: SttyAction,
    pub device: Option<String>,
    pub settings: Vec<String>,
}

/// Parse command-line arguments for stty.
pub fn parse_args(args: &[String]) -> Result<SttyConfig, String> {
    let mut action = SttyAction::ApplySettings;
    let mut device: Option<String> = None;
    let mut settings: Vec<String> = Vec::new();
    let mut i = 0;
    let mut has_explicit_action = false;

    while i < args.len() {
        match args[i].as_str() {
            "-a" | "--all" => {
                action = SttyAction::PrintAll;
                has_explicit_action = true;
            }
            "-F" | "--file" => {
                i += 1;
                if i >= args.len() {
                    return Err("option requires an argument -- 'F'".to_string());
                }
                device = Some(args[i].clone());
            }
            s if s.starts_with("--file=") => {
                device = Some(s["--file=".len()..].to_string());
            }
            s if s.starts_with("-F") && s.len() > 2 => {
                device = Some(s[2..].to_string());
            }
            "size" => {
                action = SttyAction::PrintSize;
                has_explicit_action = true;
            }
            "speed" => {
                action = SttyAction::PrintSpeed;
                has_explicit_action = true;
            }
            _ => {
                settings.push(args[i].clone());
            }
        }
        i += 1;
    }

    if !has_explicit_action && settings.is_empty() {
        action = SttyAction::PrintAll;
    }

    Ok(SttyConfig {
        action,
        device,
        settings,
    })
}

/// Apply settings from the parsed arguments to a termios structure.
/// Returns Ok(true) if any changes were made, Ok(false) otherwise.
pub fn apply_settings(
    termios: &mut libc::termios,
    settings: &[String],
) -> Result<bool, String> {
    let mut changed = false;
    let mut i = 0;

    while i < settings.len() {
        let arg = &settings[i];

        match arg.as_str() {
            "sane" => {
                set_sane(termios);
                changed = true;
            }
            "raw" => {
                set_raw(termios);
                changed = true;
            }
            "-raw" | "cooked" => {
                set_cooked(termios);
                changed = true;
            }
            "ispeed" => {
                i += 1;
                if i >= settings.len() {
                    return Err("missing argument to 'ispeed'".to_string());
                }
                let n: u32 = settings[i]
                    .parse()
                    .map_err(|_| format!("invalid integer argument: '{}'", settings[i]))?;
                let baud = num_to_baud(n)
                    .ok_or_else(|| format!("invalid speed: '{}'", n))?;
                unsafe {
                    libc::cfsetispeed(termios, baud);
                }
                changed = true;
            }
            "ospeed" => {
                i += 1;
                if i >= settings.len() {
                    return Err("missing argument to 'ospeed'".to_string());
                }
                let n: u32 = settings[i]
                    .parse()
                    .map_err(|_| format!("invalid integer argument: '{}'", settings[i]))?;
                let baud = num_to_baud(n)
                    .ok_or_else(|| format!("invalid speed: '{}'", n))?;
                unsafe {
                    libc::cfsetospeed(termios, baud);
                }
                changed = true;
            }
            _ => {
                // Check if it is a bare baud rate (numeric)
                if let Ok(n) = arg.parse::<u32>() {
                    if let Some(baud) = num_to_baud(n) {
                        unsafe {
                            libc::cfsetispeed(termios, baud);
                            libc::cfsetospeed(termios, baud);
                        }
                        changed = true;
                        i += 1;
                        continue;
                    }
                }

                // Check if it is a special character setting (e.g., "intr ^C")
                if let Some(idx) = find_special_char(arg) {
                    i += 1;
                    if i >= settings.len() {
                        return Err(format!("missing argument to '{}'", arg));
                    }
                    let cc = parse_control_char(&settings[i])
                        .ok_or_else(|| {
                            format!("invalid integer argument: '{}'", settings[i])
                        })?;
                    termios.c_cc[idx] = cc;
                    changed = true;
                    i += 1;
                    continue;
                }

                // Try as a flag
                if !apply_flag(termios, arg) {
                    return Err(format!("invalid argument '{}'", arg));
                }
                changed = true;
            }
        }

        i += 1;
    }

    Ok(changed)
}
