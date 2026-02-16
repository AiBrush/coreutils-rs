// fbasenc — encode/decode data in various encodings (GNU basenc replacement)

use std::io::{self, Read, Write};
use std::process;

const TOOL_NAME: &str = "basenc";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq)]
enum Encoding {
    Base64,
    Base64Url,
    Base32,
    Base32Hex,
    Base16,
    Base2Msbf,
    Base2Lsbf,
    Z85,
}

struct Cli {
    encoding: Option<Encoding>,
    decode: bool,
    ignore_garbage: bool,
    wrap: usize,
    file: Option<String>,
}

fn parse_args() -> Cli {
    let mut cli = Cli {
        encoding: None,
        decode: false,
        ignore_garbage: false,
        wrap: 76,
        file: None,
    };

    let mut args = std::env::args_os().skip(1);
    #[allow(clippy::while_let_on_iterator)]
    while let Some(arg) = args.next() {
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            if let Some(f) = args.next() {
                cli.file = Some(f.to_string_lossy().into_owned());
            }
            break;
        }
        if bytes.starts_with(b"--") {
            if bytes.starts_with(b"--wrap=") {
                let val = std::str::from_utf8(&bytes[7..]).unwrap_or("76");
                cli.wrap = val.parse().unwrap_or_else(|_| {
                    eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, val);
                    process::exit(1);
                });
            } else {
                match bytes {
                    b"--base64" => cli.encoding = Some(Encoding::Base64),
                    b"--base64url" => cli.encoding = Some(Encoding::Base64Url),
                    b"--base32" => cli.encoding = Some(Encoding::Base32),
                    b"--base32hex" => cli.encoding = Some(Encoding::Base32Hex),
                    b"--base16" => cli.encoding = Some(Encoding::Base16),
                    b"--base2msbf" => cli.encoding = Some(Encoding::Base2Msbf),
                    b"--base2lsbf" => cli.encoding = Some(Encoding::Base2Lsbf),
                    b"--z85" => cli.encoding = Some(Encoding::Z85),
                    b"--decode" => cli.decode = true,
                    b"--ignore-garbage" => cli.ignore_garbage = true,
                    b"--wrap" => {
                        if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.wrap = s.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("{}: option '--wrap' requires an argument", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    b"--help" => {
                        print!(
                            "Usage: {tool} [OPTION]... [FILE]\n\
                             basenc encode or decode FILE, or standard input, to standard output.\n\n\
                             With no FILE, or when FILE is -, read standard input.\n\n\
                             Mandatory arguments to long options are mandatory for short options too.\n\
                             \x20     --base64          same as 'base64' program (RFC4648 section 4)\n\
                             \x20     --base64url       file- and url-safe base64 (RFC4648 section 5)\n\
                             \x20     --base32          same as 'base32' program (RFC4648 section 6)\n\
                             \x20     --base32hex       extended hex alphabet base32 (RFC4648 section 7)\n\
                             \x20     --base16          hex encoding (RFC4648 section 8)\n\
                             \x20     --base2msbf       bit string with most significant bit (msb) first\n\
                             \x20     --base2lsbf       bit string with least significant bit (lsb) first\n\
                             \x20     --z85             ascii85-like encoding (ZeroMQ spec:32/Z85)\n\
                             \x20 -d, --decode          decode data\n\
                             \x20 -i, --ignore-garbage  when decoding, ignore non-alphabet characters\n\
                             \x20 -w, --wrap=COLS       wrap encoded lines after COLS character (default 76).\n\
                             \x20                         Use 0 to disable line wrapping\n\
                             \x20     --help             display this help and exit\n\
                             \x20     --version          output version information and exit\n\n\
                             When decoding, the input may contain newlines in addition to the bytes of\n\
                             the formal alphabet.  Use --ignore-garbage to attempt to recover\n\
                             from any other non-alphabet bytes in the encoded stream.\n",
                            tool = TOOL_NAME
                        );
                        process::exit(0);
                    }
                    b"--version" => {
                        println!("{} (fcoreutils) {}", TOOL_NAME, VERSION);
                        process::exit(0);
                    }
                    _ => {
                        eprintln!(
                            "{}: unrecognized option '{}'",
                            TOOL_NAME,
                            arg.to_string_lossy()
                        );
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'd' => cli.decode = true,
                    b'i' => cli.ignore_garbage = true,
                    b'w' => {
                        if i + 1 < bytes.len() {
                            let val = std::str::from_utf8(&bytes[i + 1..]).unwrap_or("76");
                            cli.wrap = val.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, val);
                                process::exit(1);
                            });
                            i = bytes.len();
                            continue;
                        } else if let Some(v) = args.next() {
                            let s = v.to_string_lossy();
                            cli.wrap = s.parse().unwrap_or_else(|_| {
                                eprintln!("{}: invalid wrap size: '{}'", TOOL_NAME, s);
                                process::exit(1);
                            });
                        } else {
                            eprintln!("{}: option requires an argument -- 'w'", TOOL_NAME);
                            process::exit(1);
                        }
                    }
                    _ => {
                        eprintln!("{}: invalid option -- '{}'", TOOL_NAME, bytes[i] as char);
                        eprintln!("Try '{} --help' for more information.", TOOL_NAME);
                        process::exit(1);
                    }
                }
                i += 1;
            }
        } else {
            cli.file = Some(arg.to_string_lossy().into_owned());
        }
    }

    cli
}

// ======================== Base64 ========================

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

const fn build_base64_decode_table(alphabet: &[u8; 64]) -> [u8; 256] {
    let mut table = [0xFFu8; 256];
    let mut i = 0;
    while i < 64 {
        table[alphabet[i] as usize] = i as u8;
        i += 1;
    }
    table
}

const BASE64_DECODE: [u8; 256] = build_base64_decode_table(BASE64_ALPHABET);
const BASE64URL_DECODE: [u8; 256] = build_base64_decode_table(BASE64URL_ALPHABET);

fn base64_encode(data: &[u8], alphabet: &[u8; 64]) -> String {
    if data.is_empty() {
        return String::new();
    }
    let mut result = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };

        result.push(alphabet[(b0 >> 2) as usize]);
        result.push(alphabet[((b0 & 0x03) << 4 | b1 >> 4) as usize]);
        if chunk.len() > 1 {
            result.push(alphabet[((b1 & 0x0F) << 2 | b2 >> 6) as usize]);
        } else {
            result.push(b'=');
        }
        if chunk.len() > 2 {
            result.push(alphabet[(b2 & 0x3F) as usize]);
        } else {
            result.push(b'=');
        }
    }
    // SAFETY: result contains only bytes from the base64 alphabet (ASCII subset), guaranteed valid UTF-8
    unsafe { String::from_utf8_unchecked(result) }
}

fn base64_decode(
    input: &[u8],
    decode_table: &[u8; 256],
    ignore_garbage: bool,
) -> Result<Vec<u8>, String> {
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\n' || b == b'\r' {
            continue;
        }
        if b == b'=' {
            filtered.push(b);
            continue;
        }
        if decode_table[b as usize] != 0xFF {
            filtered.push(b);
        } else if !ignore_garbage {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
    }

    if filtered.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::with_capacity(filtered.len() * 3 / 4);
    for chunk in filtered.chunks(4) {
        let mut buf = [b'='; 4];
        buf[..chunk.len()].copy_from_slice(chunk);

        let mut vals = [0u8; 4];
        let mut valid_count = 0;
        for (i, &b) in buf.iter().enumerate() {
            if b == b'=' {
                break;
            }
            let v = decode_table[b as usize];
            if v == 0xFF {
                return Err(format!("{}: invalid input", TOOL_NAME));
            }
            vals[i] = v;
            valid_count += 1;
        }

        if valid_count >= 2 {
            result.push((vals[0] << 2) | (vals[1] >> 4));
        }
        if valid_count >= 3 {
            result.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if valid_count >= 4 {
            result.push((vals[2] << 6) | vals[3]);
        }
    }

    Ok(result)
}

// ======================== Base32 ========================

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const BASE32HEX_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";

const fn build_base32_decode_table(alphabet: &[u8; 32]) -> [u8; 256] {
    let mut table = [0xFFu8; 256];
    let mut i = 0;
    while i < 32 {
        table[alphabet[i] as usize] = i as u8;
        // Also accept lowercase for letters
        if alphabet[i] >= b'A' && alphabet[i] <= b'Z' {
            table[(alphabet[i] - b'A' + b'a') as usize] = i as u8;
        }
        i += 1;
    }
    table
}

const BASE32_DECODE: [u8; 256] = build_base32_decode_table(BASE32_ALPHABET);
const BASE32HEX_DECODE: [u8; 256] = build_base32_decode_table(BASE32HEX_ALPHABET);

fn base32_encode(data: &[u8], alphabet: &[u8; 32]) -> String {
    if data.is_empty() {
        return String::new();
    }

    let mut result = Vec::with_capacity(data.len().div_ceil(5) * 8);

    for chunk in data.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);

        let b0 = buf[0];
        let b1 = buf[1];
        let b2 = buf[2];
        let b3 = buf[3];
        let b4 = buf[4];

        result.push(alphabet[(b0 >> 3) as usize]);
        result.push(alphabet[((b0 & 0x07) << 2 | b1 >> 6) as usize]);

        if chunk.len() > 1 {
            result.push(alphabet[((b1 >> 1) & 0x1F) as usize]);
            result.push(alphabet[((b1 & 0x01) << 4 | b2 >> 4) as usize]);
        } else {
            result.extend_from_slice(b"======");
            continue;
        }

        if chunk.len() > 2 {
            result.push(alphabet[((b2 & 0x0F) << 1 | b3 >> 7) as usize]);
        } else {
            result.extend_from_slice(b"====");
            continue;
        }

        if chunk.len() > 3 {
            result.push(alphabet[((b3 >> 2) & 0x1F) as usize]);
            result.push(alphabet[((b3 & 0x03) << 3 | b4 >> 5) as usize]);
        } else {
            result.extend_from_slice(b"===");
            continue;
        }

        if chunk.len() > 4 {
            result.push(alphabet[(b4 & 0x1F) as usize]);
        } else {
            result.push(b'=');
        }
    }

    // SAFETY: result contains only bytes from the base32 alphabet (ASCII subset), guaranteed valid UTF-8
    unsafe { String::from_utf8_unchecked(result) }
}

fn base32_decode(
    input: &[u8],
    decode_table: &[u8; 256],
    ignore_garbage: bool,
) -> Result<Vec<u8>, String> {
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\n' || b == b'\r' {
            continue;
        }
        if b == b'=' {
            filtered.push(b);
            continue;
        }
        if decode_table[b as usize] != 0xFF {
            filtered.push(b);
        } else if !ignore_garbage {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
    }

    if filtered.is_empty() {
        return Ok(Vec::new());
    }

    let mut result = Vec::with_capacity(filtered.len() * 5 / 8);
    for chunk in filtered.chunks(8) {
        let mut buf = [b'='; 8];
        buf[..chunk.len()].copy_from_slice(chunk);

        let mut valid_count = 0;
        let mut vals = [0u8; 8];
        for (i, &b) in buf.iter().enumerate() {
            if b == b'=' {
                break;
            }
            let v = decode_table[b as usize];
            if v == 0xFF {
                return Err(format!("{}: invalid input", TOOL_NAME));
            }
            vals[i] = v;
            valid_count += 1;
        }

        if valid_count >= 2 {
            result.push((vals[0] << 3) | (vals[1] >> 2));
        }
        if valid_count >= 4 {
            result.push((vals[1] << 6) | (vals[2] << 1) | (vals[3] >> 4));
        }
        if valid_count >= 5 {
            result.push((vals[3] << 4) | (vals[4] >> 1));
        }
        if valid_count >= 7 {
            result.push((vals[4] << 7) | (vals[5] << 2) | (vals[6] >> 3));
        }
        if valid_count >= 8 {
            result.push((vals[6] << 5) | vals[7]);
        }
    }

    Ok(result)
}

// ======================== Base16 ========================

const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";

fn base16_encode(data: &[u8]) -> String {
    let mut result = Vec::with_capacity(data.len() * 2);
    for &b in data {
        result.push(HEX_CHARS[(b >> 4) as usize]);
        result.push(HEX_CHARS[(b & 0x0F) as usize]);
    }
    // SAFETY: result contains only hex digit bytes ('0'-'9', 'A'-'F'), guaranteed valid UTF-8
    unsafe { String::from_utf8_unchecked(result) }
}

fn base16_decode(input: &[u8], ignore_garbage: bool) -> Result<Vec<u8>, String> {
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\n' || b == b'\r' {
            continue;
        }
        let val = hex_val(b);
        if val != 0xFF {
            filtered.push(b);
        } else if !ignore_garbage {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
    }

    if filtered.len() % 2 != 0 {
        return Err(format!("{}: invalid input", TOOL_NAME));
    }

    let mut result = Vec::with_capacity(filtered.len() / 2);
    for pair in filtered.chunks(2) {
        let high = hex_val(pair[0]);
        let low = hex_val(pair[1]);
        if high == 0xFF || low == 0xFF {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
        result.push((high << 4) | low);
    }

    Ok(result)
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'A'..=b'F' => b - b'A' + 10,
        b'a'..=b'f' => b - b'a' + 10,
        _ => 0xFF,
    }
}

// ======================== Base2 ========================

fn base2msbf_encode(data: &[u8]) -> String {
    let mut result = Vec::with_capacity(data.len() * 8);
    for &b in data {
        for i in (0..8).rev() {
            result.push(if (b >> i) & 1 == 1 { b'1' } else { b'0' });
        }
    }
    // SAFETY: result contains only b'0' and b'1' bytes, guaranteed valid UTF-8
    unsafe { String::from_utf8_unchecked(result) }
}

fn base2msbf_decode(input: &[u8], ignore_garbage: bool) -> Result<Vec<u8>, String> {
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\n' || b == b'\r' {
            continue;
        }
        if b == b'0' || b == b'1' {
            filtered.push(b);
        } else if !ignore_garbage {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
    }

    if filtered.len() % 8 != 0 {
        return Err(format!("{}: invalid input", TOOL_NAME));
    }

    let mut result = Vec::with_capacity(filtered.len() / 8);
    for chunk in filtered.chunks(8) {
        let mut byte = 0u8;
        for &bit in chunk {
            byte = (byte << 1) | (bit - b'0');
        }
        result.push(byte);
    }

    Ok(result)
}

fn base2lsbf_encode(data: &[u8]) -> String {
    let mut result = Vec::with_capacity(data.len() * 8);
    for &b in data {
        for i in 0..8 {
            result.push(if (b >> i) & 1 == 1 { b'1' } else { b'0' });
        }
    }
    // SAFETY: result contains only b'0' and b'1' bytes, guaranteed valid UTF-8
    unsafe { String::from_utf8_unchecked(result) }
}

fn base2lsbf_decode(input: &[u8], ignore_garbage: bool) -> Result<Vec<u8>, String> {
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\n' || b == b'\r' {
            continue;
        }
        if b == b'0' || b == b'1' {
            filtered.push(b);
        } else if !ignore_garbage {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
    }

    if filtered.len() % 8 != 0 {
        return Err(format!("{}: invalid input", TOOL_NAME));
    }

    let mut result = Vec::with_capacity(filtered.len() / 8);
    for chunk in filtered.chunks(8) {
        let mut byte = 0u8;
        for (i, &bit) in chunk.iter().enumerate() {
            byte |= (bit - b'0') << i;
        }
        result.push(byte);
    }

    Ok(result)
}

// ======================== Z85 ========================

const Z85_ENCODE_TABLE: &[u8; 85] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";

const fn build_z85_decode_table() -> [u8; 256] {
    let alpha =
        b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?&<>()[]{}@%$#";
    let mut table = [0xFFu8; 256];
    let mut i = 0;
    while i < 85 {
        table[alpha[i] as usize] = i as u8;
        i += 1;
    }
    table
}

const Z85_DECODE_TABLE: [u8; 256] = build_z85_decode_table();

fn z85_encode(data: &[u8]) -> Result<String, String> {
    if !data.len().is_multiple_of(4) {
        return Err(format!(
            "{}: invalid input (length must be a multiple of 4 for Z85 encoding)",
            TOOL_NAME
        ));
    }

    let mut result = Vec::with_capacity(data.len() * 5 / 4);

    for chunk in data.chunks(4) {
        let mut value = u32::from(chunk[0]) << 24
            | u32::from(chunk[1]) << 16
            | u32::from(chunk[2]) << 8
            | u32::from(chunk[3]);

        let mut chars = [0u8; 5];
        for c in chars.iter_mut().rev() {
            *c = Z85_ENCODE_TABLE[(value % 85) as usize];
            value /= 85;
        }
        result.extend_from_slice(&chars);
    }

    // SAFETY: result contains only bytes from the Z85 alphabet (ASCII subset), guaranteed valid UTF-8
    Ok(unsafe { String::from_utf8_unchecked(result) })
}

fn z85_decode(input: &[u8], ignore_garbage: bool) -> Result<Vec<u8>, String> {
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'\n' || b == b'\r' {
            continue;
        }
        if Z85_DECODE_TABLE[b as usize] != 0xFF {
            filtered.push(b);
        } else if !ignore_garbage {
            return Err(format!("{}: invalid input", TOOL_NAME));
        }
    }

    if filtered.len() % 5 != 0 {
        return Err(format!(
            "{}: invalid input (length must be a multiple of 5 for Z85 decoding)",
            TOOL_NAME
        ));
    }

    let mut result = Vec::with_capacity(filtered.len() * 4 / 5);
    for chunk in filtered.chunks(5) {
        let mut value: u32 = 0;
        for &b in chunk {
            let v = Z85_DECODE_TABLE[b as usize];
            if v == 0xFF {
                return Err(format!("{}: invalid input", TOOL_NAME));
            }
            value = value * 85 + u32::from(v);
        }
        result.push((value >> 24) as u8);
        result.push((value >> 16) as u8);
        result.push((value >> 8) as u8);
        result.push(value as u8);
    }

    Ok(result)
}

// ======================== Common ========================

/// Write encoded output with line wrapping directly to writer, avoiding
/// intermediate String allocation.
fn write_wrapped(out: &mut impl Write, encoded: &str, wrap: usize) -> io::Result<()> {
    if encoded.is_empty() {
        return Ok(());
    }
    if wrap == 0 {
        // GNU basenc with -w 0 does NOT add a trailing newline
        return out.write_all(encoded.as_bytes());
    }

    let bytes = encoded.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        let end = (pos + wrap).min(bytes.len());
        out.write_all(&bytes[pos..end])?;
        out.write_all(b"\n")?;
        pos = end;
    }
    Ok(())
}

fn encode_data(data: &[u8], encoding: Encoding) -> Result<String, String> {
    match encoding {
        Encoding::Base64 => Ok(base64_encode(data, BASE64_ALPHABET)),
        Encoding::Base64Url => Ok(base64_encode(data, BASE64URL_ALPHABET)),
        Encoding::Base32 => Ok(base32_encode(data, BASE32_ALPHABET)),
        Encoding::Base32Hex => Ok(base32_encode(data, BASE32HEX_ALPHABET)),
        Encoding::Base16 => Ok(base16_encode(data)),
        Encoding::Base2Msbf => Ok(base2msbf_encode(data)),
        Encoding::Base2Lsbf => Ok(base2lsbf_encode(data)),
        Encoding::Z85 => z85_encode(data),
    }
}

fn decode_data(data: &[u8], encoding: Encoding, ignore_garbage: bool) -> Result<Vec<u8>, String> {
    match encoding {
        Encoding::Base64 => base64_decode(data, &BASE64_DECODE, ignore_garbage),
        Encoding::Base64Url => base64_decode(data, &BASE64URL_DECODE, ignore_garbage),
        Encoding::Base32 => base32_decode(data, &BASE32_DECODE, ignore_garbage),
        Encoding::Base32Hex => base32_decode(data, &BASE32HEX_DECODE, ignore_garbage),
        Encoding::Base16 => base16_decode(data, ignore_garbage),
        Encoding::Base2Msbf => base2msbf_decode(data, ignore_garbage),
        Encoding::Base2Lsbf => base2lsbf_decode(data, ignore_garbage),
        Encoding::Z85 => z85_decode(data, ignore_garbage),
    }
}

fn main() {
    coreutils_rs::common::reset_sigpipe();

    let cli = parse_args();

    let encoding = match cli.encoding {
        Some(e) => e,
        None => {
            eprintln!("{}: missing encoding type", TOOL_NAME);
            eprintln!("Try '{} --help' for more information.", TOOL_NAME);
            process::exit(1);
        }
    };

    let filename = cli.file.as_deref().unwrap_or("-");

    let data = if filename == "-" {
        let mut buf = Vec::new();
        if let Err(e) = io::stdin().lock().read_to_end(&mut buf) {
            eprintln!("{}: {}", TOOL_NAME, coreutils_rs::common::io_error_msg(&e));
            process::exit(1);
        }
        buf
    } else {
        match std::fs::read(filename) {
            Ok(data) => data,
            Err(e) => {
                eprintln!(
                    "{}: {}: {}",
                    TOOL_NAME,
                    filename,
                    coreutils_rs::common::io_error_msg(&e)
                );
                process::exit(1);
            }
        }
    };

    let stdout = io::stdout();
    let mut out = io::BufWriter::with_capacity(1024 * 1024, stdout.lock());

    if cli.decode {
        match decode_data(&data, encoding, cli.ignore_garbage) {
            Ok(decoded) => {
                if let Err(e) = out.write_all(&decoded) {
                    if e.kind() == io::ErrorKind::BrokenPipe {
                        process::exit(0);
                    }
                    eprintln!("{}: write error: {}", TOOL_NAME, e);
                    process::exit(1);
                }
            }
            Err(msg) => {
                eprintln!("{}", msg);
                process::exit(1);
            }
        }
    } else {
        match encode_data(&data, encoding) {
            Ok(encoded) => {
                if let Err(e) = write_wrapped(&mut out, &encoded, cli.wrap) {
                    if e.kind() == io::ErrorKind::BrokenPipe {
                        process::exit(0);
                    }
                    eprintln!("{}: write error: {}", TOOL_NAME, e);
                    process::exit(1);
                }
            }
            Err(msg) => {
                eprintln!("{}", msg);
                process::exit(1);
            }
        }
    }

    if let Err(e) = out.flush()
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("{}: write error: {}", TOOL_NAME, e);
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Command;

    fn cmd() -> Command {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("fbasenc");
        Command::new(path)
    }

    #[test]
    fn test_help() {
        let output = cmd().arg("--help").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("--base64"));
        assert!(stdout.contains("--z85"));
    }

    #[test]
    fn test_version() {
        let output = cmd().arg("--version").output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("basenc"));
        assert!(stdout.contains("fcoreutils"));
    }

    #[test]
    fn test_no_encoding_error() {
        let output = cmd().output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("missing encoding type"));
    }

    // ---- Base64 tests ----

    #[test]
    fn test_base64_encode() {
        let mut child = cmd()
            .arg("--base64")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hello").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "SGVsbG8=");
    }

    #[test]
    fn test_base64_decode() {
        let mut child = cmd()
            .arg("--base64")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"SGVsbG8=\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout, b"Hello");
    }

    #[test]
    fn test_base64_roundtrip() {
        let input = b"The quick brown fox";
        let mut child = cmd()
            .arg("--base64")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let enc = child.wait_with_output().unwrap();
        assert!(enc.status.success());

        let mut child = cmd()
            .arg("--base64")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&enc.stdout).unwrap();
        let dec = child.wait_with_output().unwrap();
        assert!(dec.status.success());
        assert_eq!(&dec.stdout, input);
    }

    // ---- Base64url tests ----

    #[test]
    fn test_base64url_encode() {
        let mut child = cmd()
            .arg("--base64url")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hello").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "SGVsbG8=");
    }

    #[test]
    fn test_base64url_special_chars() {
        // Test data that produces +/ in standard base64 but -_ in url-safe
        let input = b"\xfb\xff\xfe";
        let encoded = base64_encode(input, BASE64URL_ALPHABET);
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        // Should contain - or _ if the data maps to those
        let standard = base64_encode(input, BASE64_ALPHABET);
        // Verify the url-safe version substitutes correctly
        assert_eq!(standard.replace('+', "-").replace('/', "_"), encoded);
    }

    #[test]
    fn test_base64url_roundtrip() {
        let input = b"\xff\xfe\xfd\xfc\xfb\xfa";
        let mut child = cmd()
            .arg("--base64url")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let enc = child.wait_with_output().unwrap();
        assert!(enc.status.success());

        let mut child = cmd()
            .arg("--base64url")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&enc.stdout).unwrap();
        let dec = child.wait_with_output().unwrap();
        assert!(dec.status.success());
        assert_eq!(&dec.stdout, input);
    }

    // ---- Base32 tests ----

    #[test]
    fn test_base32_encode() {
        let mut child = cmd()
            .arg("--base32")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hello").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "JBSWY3DP");
    }

    #[test]
    fn test_base32_decode() {
        let mut child = cmd()
            .arg("--base32")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"JBSWY3DP\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout, b"Hello");
    }

    // ---- Base32hex tests ----

    #[test]
    fn test_base32hex_encode() {
        let mut child = cmd()
            .arg("--base32hex")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hello").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // "Hello" in base32hex is "91IMOR3F"
        assert_eq!(stdout.trim(), "91IMOR3F");
    }

    #[test]
    fn test_base32hex_roundtrip() {
        let input = b"test data 12345";
        let mut child = cmd()
            .arg("--base32hex")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let enc = child.wait_with_output().unwrap();
        assert!(enc.status.success());

        let mut child = cmd()
            .arg("--base32hex")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&enc.stdout).unwrap();
        let dec = child.wait_with_output().unwrap();
        assert!(dec.status.success());
        assert_eq!(&dec.stdout, input);
    }

    // ---- Base16 tests ----

    #[test]
    fn test_base16_encode() {
        let mut child = cmd()
            .arg("--base16")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hello").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "48656C6C6F");
    }

    #[test]
    fn test_base16_decode() {
        let mut child = cmd()
            .arg("--base16")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"48656C6C6F\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout, b"Hello");
    }

    #[test]
    fn test_base16_roundtrip() {
        let input = b"\x00\x01\x02\xff\xfe\xfd";
        let mut child = cmd()
            .arg("--base16")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let enc = child.wait_with_output().unwrap();
        assert!(enc.status.success());

        let mut child = cmd()
            .arg("--base16")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&enc.stdout).unwrap();
        let dec = child.wait_with_output().unwrap();
        assert!(dec.status.success());
        assert_eq!(&dec.stdout, input);
    }

    // ---- Base2 MSB first tests ----

    #[test]
    fn test_base2msbf_encode() {
        let mut child = cmd()
            .arg("--base2msbf")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"A").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // 'A' = 0x41 = 01000001
        assert_eq!(stdout.trim(), "01000001");
    }

    #[test]
    fn test_base2msbf_roundtrip() {
        let input = b"Hi";
        let mut child = cmd()
            .arg("--base2msbf")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let enc = child.wait_with_output().unwrap();
        assert!(enc.status.success());

        let mut child = cmd()
            .arg("--base2msbf")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&enc.stdout).unwrap();
        let dec = child.wait_with_output().unwrap();
        assert!(dec.status.success());
        assert_eq!(&dec.stdout, input);
    }

    // ---- Base2 LSB first tests ----

    #[test]
    fn test_base2lsbf_encode() {
        let mut child = cmd()
            .arg("--base2lsbf")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"A").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // 'A' = 0x41 = 01000001, LSB first = 10000010
        assert_eq!(stdout.trim(), "10000010");
    }

    #[test]
    fn test_base2lsbf_roundtrip() {
        let input = b"Hi";
        let mut child = cmd()
            .arg("--base2lsbf")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let enc = child.wait_with_output().unwrap();
        assert!(enc.status.success());

        let mut child = cmd()
            .arg("--base2lsbf")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&enc.stdout).unwrap();
        let dec = child.wait_with_output().unwrap();
        assert!(dec.status.success());
        assert_eq!(&dec.stdout, input);
    }

    // ---- Z85 tests ----

    #[test]
    fn test_z85_encode() {
        // Z85 requires input length to be multiple of 4
        // "Test" (4 bytes) -> Z85 encode
        let mut child = cmd()
            .arg("--z85")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Test").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Verify it produces valid Z85 output
        assert!(!stdout.trim().is_empty());
    }

    #[test]
    fn test_z85_roundtrip() {
        let input = b"TestData"; // 8 bytes = multiple of 4
        let mut child = cmd()
            .arg("--z85")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let enc = child.wait_with_output().unwrap();
        assert!(enc.status.success());

        let mut child = cmd()
            .arg("--z85")
            .arg("-d")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&enc.stdout).unwrap();
        let dec = child.wait_with_output().unwrap();
        assert!(dec.status.success());
        assert_eq!(&dec.stdout, input);
    }

    #[test]
    fn test_z85_invalid_length() {
        // Z85 encoding requires input length to be a multiple of 4
        let mut child = cmd()
            .arg("--z85")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"Hi").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
    }

    // ---- Wrap tests ----

    #[test]
    fn test_wrap_flag() {
        let mut child = cmd()
            .arg("--base64")
            .arg("-w")
            .arg("10")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"Hello, World! This is a test.")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            assert!(line.len() <= 10, "line longer than 10: {}", line);
        }
    }

    #[test]
    fn test_wrap_zero() {
        let mut child = cmd()
            .arg("--base64")
            .arg("-w")
            .arg("0")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"Hello, World! This is a longer test input.")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 1, "should be a single line with -w 0");
    }

    // ---- Ignore garbage tests ----

    #[test]
    fn test_ignore_garbage() {
        let mut child = cmd()
            .arg("--base64")
            .arg("-d")
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"SG!!Vs##bG8=\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(&output.stdout, b"Hello");
    }

    // ---- Empty input tests ----

    #[test]
    fn test_empty_input_base64() {
        let mut child = cmd()
            .arg("--base64")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        // GNU basenc produces no output for empty input
        assert!(output.stdout.is_empty());
    }

    // ---- Unit tests for encoding functions ----

    #[test]
    fn test_base64_encode_fn() {
        assert_eq!(base64_encode(b"", BASE64_ALPHABET), "");
        assert_eq!(base64_encode(b"f", BASE64_ALPHABET), "Zg==");
        assert_eq!(base64_encode(b"fo", BASE64_ALPHABET), "Zm8=");
        assert_eq!(base64_encode(b"foo", BASE64_ALPHABET), "Zm9v");
        assert_eq!(base64_encode(b"foob", BASE64_ALPHABET), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba", BASE64_ALPHABET), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar", BASE64_ALPHABET), "Zm9vYmFy");
    }

    #[test]
    fn test_base64_decode_fn() {
        assert_eq!(base64_decode(b"", &BASE64_DECODE, false).unwrap(), b"");
        assert_eq!(base64_decode(b"Zg==", &BASE64_DECODE, false).unwrap(), b"f");
        assert_eq!(
            base64_decode(b"Zm8=", &BASE64_DECODE, false).unwrap(),
            b"fo"
        );
        assert_eq!(
            base64_decode(b"Zm9v", &BASE64_DECODE, false).unwrap(),
            b"foo"
        );
        assert_eq!(
            base64_decode(b"Zm9vYmFy", &BASE64_DECODE, false).unwrap(),
            b"foobar"
        );
    }

    #[test]
    fn test_base16_encode_fn() {
        assert_eq!(base16_encode(b""), "");
        assert_eq!(base16_encode(b"\x00"), "00");
        assert_eq!(base16_encode(b"\xff"), "FF");
        assert_eq!(base16_encode(b"Hello"), "48656C6C6F");
    }

    #[test]
    fn test_base16_decode_fn() {
        assert_eq!(base16_decode(b"", false).unwrap(), b"");
        assert_eq!(base16_decode(b"00", false).unwrap(), b"\x00");
        assert_eq!(base16_decode(b"FF", false).unwrap(), b"\xff");
        assert_eq!(base16_decode(b"48656C6C6F", false).unwrap(), b"Hello");
    }

    #[test]
    fn test_base2msbf_encode_fn() {
        assert_eq!(base2msbf_encode(b"A"), "01000001");
        assert_eq!(base2msbf_encode(b"\x00"), "00000000");
        assert_eq!(base2msbf_encode(b"\xff"), "11111111");
    }

    #[test]
    fn test_base2lsbf_encode_fn() {
        assert_eq!(base2lsbf_encode(b"A"), "10000010");
        assert_eq!(base2lsbf_encode(b"\x00"), "00000000");
        assert_eq!(base2lsbf_encode(b"\xff"), "11111111");
    }

    #[test]
    fn test_z85_encode_fn() {
        // ZeroMQ Z85 reference test vector: 4 bytes [0x86, 0x4F, 0xD2, 0x6F] -> "HelloWorld" (partial)
        // Actually the reference: [0x86, 0x4F, 0xD2, 0x6F, 0xB5, 0x59, 0xF7, 0x5B] -> "HelloWorld"
        let input = b"\x86\x4F\xD2\x6F\xB5\x59\xF7\x5B";
        let encoded = z85_encode(input).unwrap();
        assert_eq!(encoded, "HelloWorld");
    }

    #[test]
    fn test_z85_decode_fn() {
        let decoded = z85_decode(b"HelloWorld", false).unwrap();
        assert_eq!(decoded, b"\x86\x4F\xD2\x6F\xB5\x59\xF7\x5B");
    }

    #[test]
    fn test_compare_gnu_basenc_base64() {
        if Command::new("basenc").arg("--version").output().is_err() {
            return;
        }
        let input = b"Hello, World!\n";

        let mut gnu_child = match Command::new("basenc")
            .arg("--base64")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        gnu_child.stdin.take().unwrap().write_all(input).unwrap();
        let gnu_out = gnu_child.wait_with_output().unwrap();

        let mut our_child = cmd()
            .arg("--base64")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        our_child.stdin.take().unwrap().write_all(input).unwrap();
        let our_out = our_child.wait_with_output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&our_out.stdout),
            String::from_utf8_lossy(&gnu_out.stdout),
            "Base64 encoding mismatch with GNU basenc"
        );
    }

    #[test]
    fn test_compare_gnu_basenc_base16() {
        if Command::new("basenc").arg("--version").output().is_err() {
            return;
        }
        let input = b"Hello\n";

        let mut gnu_child = match Command::new("basenc")
            .arg("--base16")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        gnu_child.stdin.take().unwrap().write_all(input).unwrap();
        let gnu_out = gnu_child.wait_with_output().unwrap();

        let mut our_child = cmd()
            .arg("--base16")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        our_child.stdin.take().unwrap().write_all(input).unwrap();
        let our_out = our_child.wait_with_output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&our_out.stdout),
            String::from_utf8_lossy(&gnu_out.stdout),
            "Base16 encoding mismatch with GNU basenc"
        );
    }

    #[test]
    fn test_compare_gnu_basenc_base32() {
        if Command::new("basenc").arg("--version").output().is_err() {
            return;
        }
        let input = b"Hello\n";

        let mut gnu_child = match Command::new("basenc")
            .arg("--base32")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        gnu_child.stdin.take().unwrap().write_all(input).unwrap();
        let gnu_out = gnu_child.wait_with_output().unwrap();

        let mut our_child = cmd()
            .arg("--base32")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        our_child.stdin.take().unwrap().write_all(input).unwrap();
        let our_out = our_child.wait_with_output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&our_out.stdout),
            String::from_utf8_lossy(&gnu_out.stdout),
            "Base32 encoding mismatch with GNU basenc"
        );
    }
}
