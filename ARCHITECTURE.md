# coreutils-rs Architecture

## Common Infrastructure

### File I/O: Zero-Copy mmap
All tools use a common `FileData` enum that provides zero-copy file access:
- Files >64KB are memory-mapped via `mmap()`, avoiding copies entirely
- Small files use `fs::read()` into an owned `Vec<u8>`
- `FileData` implements `Deref<Target=[u8]>` so all code sees a unified `&[u8]`
- Hash tools additionally use `madvise(MADV_SEQUENTIAL)` and `readahead()` hints

### Release Profile
Aggressive optimization: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`, `opt-level = 3`.

---

## wc (Word Count)

**Core strategy:** Single-pass scanning with specialized SIMD paths per metric.

- **Line counting:** `memchr::memchr_iter(b'\n', data).count()` — auto-detects AVX2/SSE2/NEON
- **Word counting:** SIMD SSE2 whitespace classification. Tracks whitespace-to-non-whitespace transitions. Whitespace set: space, tab, newline, CR, form feed (0x0C), vertical tab (0x0B)
- **Byte counting:** `stat()` file size for regular files — never reads the file for `-c` only
- **Char counting:** UTF-8 leading byte detection (bytes not matching `10xxxxxx` pattern)
- **Max line length:** Tab expansion to 8-column stops, zero width for `\r` and `\v`, `wcwidth` for multibyte
- **Multi-file:** Parallel processing with thread pool when multiple files given
- **GNU compat:** Right-aligned columns, output order: lines, words, chars, bytes, max_line_length. `--total` modes, `--files0-from` support

## cut (Field Extraction)

**Core strategy:** Zero-copy field extraction with SIMD delimiter scanning.

- **Delimiter scanning:** Uses SIMD byte search to find delimiter positions in each line
- **Field extraction:** Outputs byte slices directly from mmap'd data — no per-line allocation
- **Byte/char ranges:** Direct byte offset slicing from mmap'd buffer
- **GNU compat:** `--complement` inverts selection, `--output-delimiter` for custom output, `-s` suppresses lines without delimiters

## base64 (Base64 Encode/Decode)

**Core strategy:** SIMD vectorized codec with chunked streaming.

- **Encoding:** `base64-simd` crate provides vectorized base64 encoding
- **Decoding:** Same crate for SIMD-accelerated decoding with `-i` for ignoring garbage
- **Streaming:** 4MB chunks allow processing arbitrarily large files without loading entirely
- **Output:** Raw fd stdout write bypasses Rust's BufWriter lock overhead
- **GNU compat:** 76-character line wrapping (configurable with `-w`), decode ignores newlines

## sha256sum (SHA-256 Checksums)

**Core strategy:** mmap + hardware acceleration + parallel multi-file.

- **Hashing:** `sha2` crate auto-detects SHA-NI x86 instructions for hardware-accelerated SHA-256
- **I/O:** mmap with `madvise(MADV_SEQUENTIAL)` tells kernel to prefetch pages linearly
- **Readahead:** Explicit `readahead()` syscall pre-populates page cache
- **Multi-file:** Thread pool hashes multiple files concurrently
- **GNU compat:** `--check` mode parses GNU checksum files, `--status`/`--quiet`/`--strict` modes

## md5sum (MD5 Checksums)

**Core strategy:** Same architecture as sha256sum with MD5 digest.

- **Hashing:** `md-5` crate for MD5 computation
- **I/O:** Identical mmap + madvise + readahead pipeline
- **Multi-file:** Parallel processing via thread pool
- **GNU compat:** Same check/verify interface as sha256sum

## b2sum (BLAKE2b Checksums)

**Core strategy:** Same mmap pipeline with BLAKE2b hardware-optimized implementation.

- **Hashing:** `blake2` crate with optimized BLAKE2b implementation
- **I/O:** mmap + madvise + readahead
- **Variable length:** `-l` flag for custom digest length (default 512 bits)
- **GNU compat:** Same check/verify interface, supports `--length` for digest size

## sort (Line Sorting)

**Core strategy:** Parallel merge sort with efficient key extraction.

- **Sorting:** Rayon-based parallel merge sort distributes work across CPU cores
- **Key extraction:** `-k` fields parsed and cached for efficient comparison
- **Sort modes:** Numeric (`-n`), general numeric (`-g`), human-numeric (`-h`), version (`-V`), month (`-M`), random (`-R`)
- **Stability:** `-s` flag for stable sort (preserves input order for equal keys)
- **GNU compat:** Field separators (`-t`), unique output (`-u`), merge mode (`-m`), check-sorted (`-c`/`-C`), output file (`-o`)

## tr (Character Translation)

**Core strategy:** 256-byte lookup tables for O(1) per-byte translation.

- **Translation table:** Pre-built 256-byte array maps each input byte to its output byte — branch-free
- **Delete mode:** Lookup table marks bytes for deletion, tight output loop
- **Squeeze mode:** Tracks previous output byte to collapse repeats
- **I/O:** mmap for stdin when possible, 4MB output buffers minimize write syscalls
- **Character classes:** `[:alpha:]`, `[:digit:]`, `[:space:]`, etc. expanded at table build time
- **GNU compat:** `-c`/`-C` complement, `-t` truncate SET2, ranges (`a-z`), repeats (`[c*n]`)

## uniq (Deduplicate Lines)

**Core strategy:** Sequential line comparison with mmap I/O.

- **Comparison:** Adjacent line comparison with optional case-insensitive (`-i`), skip-fields (`-f`), skip-chars (`-s`), compare-width (`-w`)
- **I/O:** mmap input, 1MB BufWriter output buffer
- **Modes:** Count (`-c`), duplicates-only (`-d`), all-duplicates (`-D`), unique-only (`-u`)
- **GNU compat:** All output format flags, field/char skipping matches GNU behavior

## tac (Reverse Lines)

**Core strategy:** Forward SIMD scan, reversed output.

- **Scanning:** `memchr` finds all newline positions scanning forward (SIMD-accelerated)
- **Reversal:** Iterates found positions in reverse, outputting line slices
- **I/O:** 1MB BufWriter for efficient output
- **Separator:** Custom separator with `-s`, before-separator mode with `-b`
- **GNU compat:** Matches GNU tac output for all separator modes
