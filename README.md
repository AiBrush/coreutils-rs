# fcoreutils

[![Test](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml)
[![Release](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/fcoreutils?color=orange)](https://crates.io/crates/fcoreutils)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/AiBrush/coreutils-rs)](https://github.com/AiBrush/coreutils-rs/releases)

High-performance GNU coreutils replacement in Rust. Faster with SIMD acceleration. Drop-in compatible, cross-platform.

## Performance (100MB text file, hyperfine)

| Command | GNU | fcoreutils | Speedup |
|---------|-----|------------|---------|
| `wc` (default) | 339ms | 29ms | **11.7x** |
| `wc -w` | 339ms | 19ms | **17.8x** |
| `wc -l` | 39ms | 23ms | **1.7x** |
| `cut -b1-20` | 309ms | 29ms | **10.6x** |
| `cut -d' ' -f1` | 339ms | 82ms | **4.1x** |
| `sort` (100MB) | 1851ms | 399ms | **4.6x** |
| `sort` (10MB) | 157ms | 33ms | **4.8x** |
| `uniq` (10MB) | 33ms | 7ms | **4.8x** |
| `tac` | 133ms | 59ms | **2.3x** |
| `base64` encode | 188ms | 116ms | **1.6x** |
| `base64` decode | 539ms | 346ms | **1.6x** |
| `b2sum` | 272ms | 222ms | **1.2x** |
| `sha256sum` | 104ms | 103ms | **1.0x** |
| `tr a-z A-Z` | 97ms | 90ms | **1.1x** |
| `md5sum` | 211ms | 263ms | 0.8x |

## Tools

| Tool | Binary | Status | Description |
|------|--------|--------|-------------|
| wc | `fwc` | Optimized | Word, line, char, byte count (SIMD SSE2, single-pass, parallel) |
| cut | `fcut` | Optimized | Field/byte/char extraction (mmap, SIMD) |
| sha256sum | `fsha256sum` | Optimized | SHA-256 checksums (mmap, madvise, readahead, parallel) |
| md5sum | `fmd5sum` | Optimized | MD5 checksums (mmap, madvise, readahead, parallel) |
| b2sum | `fb2sum` | Optimized | BLAKE2b checksums (mmap, madvise, readahead) |
| base64 | `fbase64` | Optimized | Base64 encode/decode (SIMD, 4MB chunks, raw fd stdout) |
| sort | `fsort` | Optimized | Line sorting (parallel merge sort) |
| tr | `ftr` | Optimized | Character translation (mmap, 4MB buffers, lookup tables) |
| uniq | `funiq` | Optimized | Filter duplicate lines (mmap, 1MB buffers) |
| tac | `ftac` | Optimized | Reverse file lines (forward SIMD scan, 1MB buffers) |

## Installation

```bash
cargo install fcoreutils
```

Or build from source:

```bash
git clone https://github.com/AiBrush/coreutils-rs.git
cd coreutils-rs
cargo build --release
```

Binaries are in `target/release/`.

## Usage

Each tool is prefixed with `f` to avoid conflicts with system utilities:

```bash
# Word count (drop-in replacement for wc)
fwc file.txt
fwc -l file.txt          # Line count only
fwc -w file.txt          # Word count only
fwc -c file.txt          # Byte count only (uses stat, instant)
fwc -m file.txt          # Character count (UTF-8 aware)
fwc -L file.txt          # Max line display width
cat file.txt | fwc       # Stdin support
fwc file1.txt file2.txt  # Multiple files with total

# Cut (drop-in replacement for cut)
fcut -d: -f2 file.csv    # Extract field 2 with : delimiter
fcut -d, -f1,3-5 data.csv  # Multiple fields
fcut -b1-20 file.txt     # Byte range selection

# Hash tools (drop-in replacements)
fsha256sum file.txt       # SHA-256 checksum
fmd5sum file.txt          # MD5 checksum
fb2sum file.txt           # BLAKE2b checksum
fsha256sum -c sums.txt    # Verify checksums

# Base64 encode/decode
fbase64 file.txt          # Encode to base64
fbase64 -d encoded.txt    # Decode from base64
fbase64 -w 0 file.txt     # No line wrapping

# Sort, translate, deduplicate, reverse
fsort file.txt            # Sort lines alphabetically
fsort -n file.txt         # Numeric sort
ftr 'a-z' 'A-Z' < file   # Translate lowercase to uppercase
ftr -d '[:space:]' < file # Delete whitespace
funiq file.txt            # Remove adjacent duplicates
funiq -c file.txt         # Count occurrences
ftac file.txt             # Print lines in reverse order
```

## Key Optimizations

- **Zero-copy mmap**: Large files are memory-mapped directly, avoiding copies
- **SIMD scanning**: `memchr` crate auto-detects AVX2/SSE2/NEON for byte searches
- **stat-only byte counting**: `wc -c` uses `stat()` without reading file content
- **Hardware-accelerated hashing**: sha2 detects SHA-NI, blake2 uses optimized implementations
- **SIMD base64**: Vectorized encode/decode with 4MB chunked streaming
- **Parallel processing**: Multi-file hashing and wc use thread pools
- **Lookup tables**: `tr` uses 256-byte translation tables for O(1) character mapping
- **Forward SIMD scan**: `tac` scans forward with memchr for newlines, then reverses output
- **Optimized release profile**: Fat LTO, single codegen unit, abort on panic, stripped binaries

## GNU Compatibility

Output is byte-identical to GNU coreutils. All flags are supported including `--files0-from`, `--total`, `--complement`, `--check`, and correct column alignment.

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for design decisions and [PROGRESS.md](PROGRESS.md) for development status.

## Security

To report a vulnerability, please see our [Security Policy](SECURITY.md).

## License

MIT
