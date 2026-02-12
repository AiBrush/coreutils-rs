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

/// Fast custom numeric parser: parses sign + digits + optional decimal directly from bytes.
/// Avoids UTF-8 validation and str::parse::<f64>() overhead entirely.
pub fn parse_numeric_value(s: &[u8]) -> f64 {
    let s = skip_leading_blanks(s);
    if s.is_empty() {
        return 0.0;
    }

    let mut i = 0;
    let negative = if s[i] == b'-' {
        i += 1;
        true
    } else {
        if s[i] == b'+' {
            i += 1;
        }
        false
    };

    // Parse integer part
    let mut integer: u64 = 0;
    let mut has_digits = false;
    while i < s.len() && s[i].is_ascii_digit() {
        integer = integer.wrapping_mul(10).wrapping_add((s[i] - b'0') as u64);
        has_digits = true;
        i += 1;
    }

    // Parse fractional part
    if i < s.len() && s[i] == b'.' {
        i += 1;
        let frac_start = i;
        let mut frac_val: u64 = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            frac_val = frac_val.wrapping_mul(10).wrapping_add((s[i] - b'0') as u64);
            has_digits = true;
            i += 1;
        }
        if !has_digits {
            return 0.0;
        }
        let frac_digits = i - frac_start;
        let result = if frac_digits > 0 {
            // Use pre-computed powers of 10 for common cases
            let divisor = POW10[frac_digits.min(POW10.len() - 1)];
            integer as f64 + frac_val as f64 / divisor
        } else {
            integer as f64
        };
        return if negative { -result } else { result };
    }

    if !has_digits {
        return 0.0;
    }

    let result = integer as f64;
    if negative { -result } else { result }
}

/// Pre-computed powers of 10 for fast decimal conversion.
const POW10: [f64; 20] = [
    1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19,
];

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
    if s.is_empty() {
        return f64::NAN;
    }

    // Find the longest valid float prefix
    let mut i = 0;

    // Handle "inf", "-inf", "+inf", "nan" etc.
    let start = if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
        i - 1
    } else {
        i
    };

    // Check for "inf"/"infinity"/"nan" prefix (case-insensitive)
    if i + 2 < s.len() {
        let c0 = s[i].to_ascii_lowercase();
        let c1 = s[i + 1].to_ascii_lowercase();
        let c2 = s[i + 2].to_ascii_lowercase();
        if (c0 == b'i' && c1 == b'n' && c2 == b'f') || (c0 == b'n' && c1 == b'a' && c2 == b'n') {
            // Try parsing the prefix as a special float
            let end = s.len().min(i + 8); // "infinity" is 8 chars
            for e in (i + 3..=end).rev() {
                if let Ok(text) = std::str::from_utf8(&s[start..e]) {
                    if let Ok(v) = text.parse::<f64>() {
                        return v;
                    }
                }
            }
            return f64::NAN;
        }
    }

    // Reset i for numeric parsing
    i = start;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }

    // Digits before decimal
    let mut has_digits = false;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
        has_digits = true;
    }
    // Decimal point
    if i < s.len() && s[i] == b'.' {
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            has_digits = true;
        }
    }
    if !has_digits {
        return f64::NAN;
    }
    // Exponent
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let save = i;
        i += 1;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        if i < s.len() && s[i].is_ascii_digit() {
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            i = save;
        }
    }

    // Use fast-float for the numeric prefix — avoids UTF-8 validation overhead
    match fast_float::parse_partial::<f64, _>(&s[start..i]) {
        Ok((v, _)) => v,
        Err(_) => f64::NAN,
    }
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

    // Use the fast custom parser for the numeric part
    let base = parse_numeric_value(s);
    let end = find_numeric_end(s);

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
/// Uses byte slices directly instead of char iterators for maximum performance.
pub fn compare_version(a: &[u8], b: &[u8]) -> Ordering {
    let mut ai = 0usize;
    let mut bi = 0usize;

    loop {
        if ai >= a.len() && bi >= b.len() {
            return Ordering::Equal;
        }
        if ai >= a.len() {
            return Ordering::Less;
        }
        if bi >= b.len() {
            return Ordering::Greater;
        }

        let ac = a[ai];
        let bc = b[bi];

        if ac.is_ascii_digit() && bc.is_ascii_digit() {
            let anum = consume_number_bytes(a, &mut ai);
            let bnum = consume_number_bytes(b, &mut bi);
            match anum.cmp(&bnum) {
                Ordering::Equal => continue,
                other => return other,
            }
        } else {
            match ac.cmp(&bc) {
                Ordering::Equal => {
                    ai += 1;
                    bi += 1;
                }
                other => return other,
            }
        }
    }
}

#[inline]
fn consume_number_bytes(data: &[u8], pos: &mut usize) -> u64 {
    let mut n: u64 = 0;
    while *pos < data.len() && data[*pos].is_ascii_digit() {
        n = n
            .saturating_mul(10)
            .saturating_add((data[*pos] - b'0') as u64);
        *pos += 1;
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

/// Concrete comparison function type. Selected once at setup time to avoid
/// per-comparison flag checking in hot sort loops.
pub type CompareFn = fn(&[u8], &[u8]) -> Ordering;

/// Select a concrete comparison function based on KeyOpts.
/// Returns (compare_fn, needs_leading_blank_strip, needs_reverse).
/// The caller applies blank-stripping and reversal outside the function pointer,
/// eliminating all per-comparison branching.
pub fn select_comparator(opts: &KeyOpts, random_seed: u64) -> (CompareFn, bool, bool) {
    let needs_blank = opts.ignore_leading_blanks;
    let needs_reverse = opts.reverse;

    let cmp: CompareFn = if opts.numeric {
        compare_numeric
    } else if opts.general_numeric {
        compare_general_numeric
    } else if opts.human_numeric {
        compare_human_numeric
    } else if opts.month {
        compare_month
    } else if opts.version {
        compare_version
    } else if opts.random {
        // Random needs seed — wrap in a closure-like pattern
        // Since we need random_seed, we use a special case
        return (
            make_random_comparator(random_seed),
            needs_blank,
            needs_reverse,
        );
    } else if opts.dictionary_order || opts.ignore_nonprinting || opts.ignore_case {
        // Text filtering: select specialized variant
        match (
            opts.dictionary_order,
            opts.ignore_nonprinting,
            opts.ignore_case,
        ) {
            (false, false, true) => compare_ignore_case,
            (true, false, false) => |a: &[u8], b: &[u8]| compare_dictionary(a, b, false),
            (true, false, true) => |a: &[u8], b: &[u8]| compare_dictionary(a, b, true),
            (false, true, false) => |a: &[u8], b: &[u8]| compare_ignore_nonprinting(a, b, false),
            (false, true, true) => |a: &[u8], b: &[u8]| compare_ignore_nonprinting(a, b, true),
            (true, true, false) => {
                |a: &[u8], b: &[u8]| compare_text_filtered(a, b, true, true, false)
            }
            (true, true, true) => {
                |a: &[u8], b: &[u8]| compare_text_filtered(a, b, true, true, true)
            }
            _ => |a: &[u8], b: &[u8]| a.cmp(b),
        }
    } else {
        |a: &[u8], b: &[u8]| a.cmp(b)
    };

    (cmp, needs_blank, needs_reverse)
}

fn make_random_comparator(seed: u64) -> CompareFn {
    // We can't capture the seed in a function pointer, so we use a static.
    // This is safe because sort is single-process and seed doesn't change during a sort.
    RANDOM_SEED.store(seed, std::sync::atomic::Ordering::Relaxed);
    random_compare_with_static_seed
}

static RANDOM_SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn random_compare_with_static_seed(a: &[u8], b: &[u8]) -> Ordering {
    let seed = RANDOM_SEED.load(std::sync::atomic::Ordering::Relaxed);
    compare_random(a, b, seed)
}
