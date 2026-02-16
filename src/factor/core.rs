/// Prime factorization using trial division for small factors and
/// Pollard's rho algorithm with Miller-Rabin primality testing for larger factors.
/// Supports numbers up to u128.

/// Modular multiplication for u128 that avoids overflow.
/// Computes (a * b) % m using the Russian peasant (binary) method.
fn mod_mul(mut a: u128, mut b: u128, m: u128) -> u128 {
    // If the product fits in u128, use direct multiplication
    if a.leading_zeros() + b.leading_zeros() >= 128 {
        return (a * b) % m;
    }
    a %= m;
    b %= m;
    let mut result: u128 = 0;
    while b > 0 {
        if b & 1 == 1 {
            result = result.wrapping_add(a);
            if result >= m {
                result -= m;
            }
        }
        a <<= 1;
        if a >= m {
            a -= m;
        }
        b >>= 1;
    }
    result
}

/// Modular exponentiation: computes (base^exp) % m using repeated squaring.
fn mod_pow(mut base: u128, mut exp: u128, m: u128) -> u128 {
    if m == 1 {
        return 0;
    }
    let mut result: u128 = 1;
    base %= m;
    while exp > 0 {
        if exp & 1 == 1 {
            result = mod_mul(result, base, m);
        }
        exp >>= 1;
        base = mod_mul(base, base, m);
    }
    result
}

/// Deterministic Miller-Rabin primality test.
/// For n < 3,317,044,064,679,887,385,961,981 the witnesses below are sufficient
/// for a deterministic result.
fn is_prime_miller_rabin(n: u128) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 || n == 3 {
        return true;
    }
    if n.is_multiple_of(2) || n.is_multiple_of(3) {
        return false;
    }

    // Small primes check
    const SMALL_PRIMES: [u128; 15] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47];
    for &p in &SMALL_PRIMES {
        if n == p {
            return true;
        }
        if n.is_multiple_of(p) {
            return false;
        }
    }

    // Write n-1 as 2^r * d where d is odd
    let mut d = n - 1;
    let mut r = 0u32;
    while d.is_multiple_of(2) {
        d /= 2;
        r += 1;
    }

    // Witnesses sufficient for deterministic results up to very large numbers.
    // These cover all composites below 3,317,044,064,679,887,385,961,981.
    let witnesses: &[u128] = &[2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];

    'witness: for &a in witnesses {
        if a >= n {
            continue;
        }
        let mut x = mod_pow(a, d, n);
        if x == 1 || x == n - 1 {
            continue;
        }
        for _ in 0..r - 1 {
            x = mod_mul(x, x, n);
            if x == n - 1 {
                continue 'witness;
            }
        }
        return false;
    }
    true
}

/// Compute gcd using the binary GCD algorithm.
fn gcd(mut a: u128, mut b: u128) -> u128 {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            break;
        }
    }
    a << shift
}

/// Pollard's rho algorithm to find a non-trivial factor of n.
/// Uses Brent's cycle detection variant for efficiency.
fn pollard_rho(n: u128) -> u128 {
    if n.is_multiple_of(2) {
        return 2;
    }

    // We try multiple starting values and constants until we find a factor.
    // Use a simple pseudo-random sequence based on the iteration count.
    for c_offset in 1u128..n {
        let c = c_offset;
        let mut x: u128 = c_offset.wrapping_mul(6364136223846793005).wrapping_add(1) % n;
        let mut y = x;
        let mut d: u128 = 1;

        while d == 1 {
            // Brent's improvement: advance x, accumulate gcd in batches
            x = mod_mul(x, x, n).wrapping_add(c) % n;
            y = mod_mul(y, y, n).wrapping_add(c) % n;
            y = mod_mul(y, y, n).wrapping_add(c) % n;

            d = gcd(x.abs_diff(y), n);
        }

        if d != n {
            return d;
        }
    }
    // Fallback: should not reach here for composite numbers
    n
}

/// Recursively factor n, appending prime factors to the vector.
fn factor_recursive(n: u128, factors: &mut Vec<u128>) {
    if n <= 1 {
        return;
    }
    if is_prime_miller_rabin(n) {
        factors.push(n);
        return;
    }

    // Find a non-trivial factor
    let mut d = pollard_rho(n);
    // In rare cases, pollard_rho may return n itself; try again with trial division fallback
    if d == n {
        // Brute force trial division for this case
        d = 2;
        while d * d <= n {
            if n.is_multiple_of(d) {
                break;
            }
            d += 1;
        }
        if d * d > n {
            // n is prime after all
            factors.push(n);
            return;
        }
    }
    factor_recursive(d, factors);
    factor_recursive(n / d, factors);
}

/// Return the sorted list of prime factors of n (with repetition).
/// For n <= 1, returns an empty vector.
///
/// Uses trial division for small primes, then Pollard's rho for large factors.
pub fn factorize(n: u128) -> Vec<u128> {
    if n <= 1 {
        return vec![];
    }

    let mut factors = Vec::new();
    let mut n = n;

    // Trial division by small primes
    const SMALL_PRIMES: [u128; 15] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47];
    for &p in &SMALL_PRIMES {
        while n.is_multiple_of(p) {
            factors.push(p);
            n /= p;
        }
    }

    // Continue trial division with 6k +/- 1 up to a threshold
    let mut i: u128 = 53;
    while i * i <= n && i < 10000 {
        while n.is_multiple_of(i) {
            factors.push(i);
            n /= i;
        }
        i += 2;
        while n.is_multiple_of(i) {
            factors.push(i);
            n /= i;
        }
        i += 4;
    }

    // Use Pollard's rho for whatever remains
    if n > 1 {
        factor_recursive(n, &mut factors);
        factors.sort();
    }

    factors
}

/// Format a factorization result as "NUMBER: FACTOR FACTOR ..." matching GNU factor output.
pub fn format_factors(n: u128) -> String {
    let factors = factorize(n);
    if factors.is_empty() {
        format!("{}:", n)
    } else {
        let factor_strs: Vec<String> = factors.iter().map(|f| f.to_string()).collect();
        format!("{}: {}", n, factor_strs.join(" "))
    }
}
