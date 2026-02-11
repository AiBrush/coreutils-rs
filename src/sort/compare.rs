/// Comparison functions for different sort modes.
/// All comparison functions are allocation-free for maximum sort performance.
use std::cmp::Ordering;

use super::key::KeyOpts;

/// Strip leading blanks (space and tab).
#[inline]
pub fn skip_leading_blanks(s: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < s.len() && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    &s[i..]
}

/// Compare two byte slices lexicographically (default sort).
#[inline]
pub fn compare_lexical(a: &[u8], b: &[u8]) -> Ordering {
    a.cmp(b)
}

/// Numeric sort (-n): compare leading numeric strings.
/// Handles optional leading whitespace, sign, and decimal point.
pub fn compare_numeric(a: &[u8], b: &[u8]) -> Ordering {
    let va = parse_numeric_value(a);
    let vb = parse_numeric_value(b);
    va.partial_cmp(&vb).unwrap_or(Ordering::Equal)
}

pub fn parse_numeric_value(s: &[u8]) -> f64 {
    let s = skip_leading_blanks(s);
    if s.is_empty() {
        return 0.0;
    }

    let mut i = 0;
    let negative = if s[i] == b'-' {
        i += 1;
        true
    } else if s[i] == b'+' {
        i += 1;
        false
    } else {
        false
    };

    // Fast hand-rolled parser (avoids str::parse::<f64>() overhead)
    let mut int_part: u64 = 0;
    let mut has_digits = false;
    while i < s.len() && s[i].is_ascii_digit() {
        int_part = int_part
            .saturating_mul(10)
            .saturating_add((s[i] - b'0') as u64);
        i += 1;
        has_digits = true;
    }

    let mut frac: f64 = 0.0;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        let mut divisor: f64 = 10.0;
        while i < s.len() && s[i].is_ascii_digit() {
            frac += (s[i] - b'0') as f64 / divisor;
            divisor *= 10.0;
            i += 1;
            has_digits = true;
        }
    }

    if !has_digits {
        return 0.0;
    }

    let result = int_part as f64 + frac;
    if negative { -result } else { result }
}

fn find_numeric_end(s: &[u8]) -> usize {
    let mut i = 0;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let mut has_digits = false;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
        has_digits = true;
    }
    if i < s.len() && s[i] == b'.' {
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            has_digits = true;
        }
    }
    if has_digits { i } else { 0 }
}

/// General numeric sort (-g): handles scientific notation, infinity, NaN.
/// O(n) parser.
pub fn compare_general_numeric(a: &[u8], b: &[u8]) -> Ordering {
    let va = parse_general_numeric(a);
    let vb = parse_general_numeric(b);
    match (va.is_nan(), vb.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => va.partial_cmp(&vb).unwrap_or(Ordering::Equal),
    }
}

pub fn parse_general_numeric(s: &[u8]) -> f64 {
    let s = skip_leading_blanks(s);
    let s_str = match std::str::from_utf8(s) {
        Ok(s) => s.trim(),
        Err(_) => return f64::NAN,
    };
    if s_str.is_empty() {
        return f64::NAN;
    }
    // Try parsing the whole string first (handles "inf", "-inf", "nan", etc.)
    if let Ok(v) = s_str.parse::<f64>() {
        return v;
    }

    // Find the longest valid float prefix in O(n)
    let bytes = s_str.as_bytes();
    let mut i = 0;

    // Sign
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    // Digits before decimal
    let mut has_digits = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        has_digits = true;
    }
    // Decimal point
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            has_digits = true;
        }
    }
    if !has_digits {
        return f64::NAN;
    }
    // Exponent
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let save = i;
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        if i < bytes.len() && bytes[i].is_ascii_digit() {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            i = save;
        }
    }

    s_str[..i].parse::<f64>().unwrap_or(f64::NAN)
}

/// Human numeric sort (-h): handles suffixes K, M, G, T, P, E, Z, Y.
pub fn compare_human_numeric(a: &[u8], b: &[u8]) -> Ordering {
    let va = parse_human_numeric(a);
    let vb = parse_human_numeric(b);
    va.partial_cmp(&vb).unwrap_or(Ordering::Equal)
}

pub fn parse_human_numeric(s: &[u8]) -> f64 {
    let s = skip_leading_blanks(s);
    if s.is_empty() {
        return 0.0;
    }
    let end = find_numeric_end(s);
    let base = if end == 0 {
        0.0
    } else {
        match std::str::from_utf8(&s[..end]) {
            Ok(num_str) => num_str.parse::<f64>().unwrap_or(0.0),
            Err(_) => 0.0,
        }
    };
    if end < s.len() {
        let multiplier = match s[end] {
            b'K' | b'k' => 1e3,
            b'M' => 1e6,
            b'G' => 1e9,
            b'T' => 1e12,
            b'P' => 1e15,
            b'E' => 1e18,
            b'Z' => 1e21,
            b'Y' => 1e24,
            _ => 1.0,
        };
        base * multiplier
    } else {
        base
    }
}

/// Month sort (-M).
pub fn compare_month(a: &[u8], b: &[u8]) -> Ordering {
    let ma = parse_month(a);
    let mb = parse_month(b);
    ma.cmp(&mb)
}

fn parse_month(s: &[u8]) -> u8 {
    let s = skip_leading_blanks(s);
    if s.len() < 3 {
        return 0;
    }
    let m = [
        s[0].to_ascii_uppercase(),
        s[1].to_ascii_uppercase(),
        s[2].to_ascii_uppercase(),
    ];
    match &m {
        b"JAN" => 1,
        b"FEB" => 2,
        b"MAR" => 3,
        b"APR" => 4,
        b"MAY" => 5,
        b"JUN" => 6,
        b"JUL" => 7,
        b"AUG" => 8,
        b"SEP" => 9,
        b"OCT" => 10,
        b"NOV" => 11,
        b"DEC" => 12,
        _ => 0,
    }
}

/// Version sort (-V): natural sort of version numbers.
pub fn compare_version(a: &[u8], b: &[u8]) -> Ordering {
    let a_str = std::str::from_utf8(a).unwrap_or("");
    let b_str = std::str::from_utf8(b).unwrap_or("");
    compare_version_str(a_str, b_str)
}

fn compare_version_str(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();

    loop {
        match (ai.peek(), bi.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ac), Some(&bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let anum = consume_number(&mut ai);
                    let bnum = consume_number(&mut bi);
                    match anum.cmp(&bnum) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    match ac.cmp(&bc) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

fn consume_number(iter: &mut std::iter::Peekable<std::str::Chars>) -> u64 {
    let mut n: u64 = 0;
    while let Some(&c) = iter.peek() {
        if c.is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add(c as u64 - '0' as u64);
            iter.next();
        } else {
            break;
        }
    }
    n
}

/// Random sort (-R): hash-based shuffle that groups identical keys.
pub fn compare_random(a: &[u8], b: &[u8], seed: u64) -> Ordering {
    let ha = fnv1a_hash(a, seed);
    let hb = fnv1a_hash(b, seed);
    ha.cmp(&hb)
}

/// FNV-1a hash with seed mixing.
#[inline]
fn fnv1a_hash(data: &[u8], seed: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325u64 ^ seed;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Compare with text filtering (-d, -i, -f flags in any combination).
/// Allocation-free: uses iterator filtering.
#[inline]
fn is_dict_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b' ' || b == b'\t'
}

#[inline]
fn is_printable(b: u8) -> bool {
    b >= 0x20 && b < 0x7f
}

fn compare_text_filtered(
    a: &[u8],
    b: &[u8],
    dict: bool,
    no_print: bool,
    fold_case: bool,
) -> Ordering {
    if !dict && !no_print && !fold_case {
        return a.cmp(b);
    }

    let mut ai = a.iter().copied();
    let mut bi = b.iter().copied();

    loop {
        let na = next_valid(&mut ai, dict, no_print);
        let nb = next_valid(&mut bi, dict, no_print);
        match (na, nb) {
            (Some(ab), Some(bb)) => {
                let ca = if fold_case {
                    ab.to_ascii_uppercase()
                } else {
                    ab
                };
                let cb = if fold_case {
                    bb.to_ascii_uppercase()
                } else {
                    bb
                };
                match ca.cmp(&cb) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

#[inline]
fn next_valid(iter: &mut impl Iterator<Item = u8>, dict: bool, no_print: bool) -> Option<u8> {
    loop {
        match iter.next() {
            None => return None,
            Some(b) => {
                if dict && !is_dict_char(b) {
                    continue;
                }
                if no_print && !is_printable(b) {
                    continue;
                }
                return Some(b);
            }
        }
    }
}

// Public wrappers for backward compatibility with tests
pub fn compare_ignore_case(a: &[u8], b: &[u8]) -> Ordering {
    compare_text_filtered(a, b, false, false, true)
}

pub fn compare_dictionary(a: &[u8], b: &[u8], ignore_case: bool) -> Ordering {
    compare_text_filtered(a, b, true, false, ignore_case)
}

pub fn compare_ignore_nonprinting(a: &[u8], b: &[u8], ignore_case: bool) -> Ordering {
    compare_text_filtered(a, b, false, true, ignore_case)
}

/// Master comparison function that dispatches based on KeyOpts.
pub fn compare_with_opts(a: &[u8], b: &[u8], opts: &KeyOpts, random_seed: u64) -> Ordering {
    let a = if opts.ignore_leading_blanks {
        skip_leading_blanks(a)
    } else {
        a
    };
    let b = if opts.ignore_leading_blanks {
        skip_leading_blanks(b)
    } else {
        b
    };

    let result = if opts.numeric {
        compare_numeric(a, b)
    } else if opts.general_numeric {
        compare_general_numeric(a, b)
    } else if opts.human_numeric {
        compare_human_numeric(a, b)
    } else if opts.month {
        compare_month(a, b)
    } else if opts.version {
        compare_version(a, b)
    } else if opts.random {
        compare_random(a, b, random_seed)
    } else {
        compare_text_filtered(
            a,
            b,
            opts.dictionary_order,
            opts.ignore_nonprinting,
            opts.ignore_case,
        )
    };

    if opts.reverse {
        result.reverse()
    } else {
        result
    }
}
