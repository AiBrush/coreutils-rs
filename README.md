# fcoreutils

[![Test](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml)
[![Release](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/fcoreutils?color=orange)](https://crates.io/crates/fcoreutils)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/AiBrush/coreutils-rs)](https://github.com/AiBrush/coreutils-rs/releases)

High-performance GNU coreutils replacement in Rust. Faster with SIMD acceleration. Drop-in compatible, cross-platform.

## Performance ([independent benchmarks](https://github.com/AiBrush/coreutils-rs-independent-test) v0.3.5, Linux, hyperfine)

| Tool | Speedup vs GNU | Speedup vs uutils |
|------|---------------:|-------------------:|
| wc | **50.7x** | 26.6x |
| sort | **18.7x** | 16.4x |
| uniq | **14.2x** | 4.4x |
| base64 | **9.4x** | 8.5x |
| tr | **6.6x** | 6.2x |
| cut | **6.5x** | 3.5x |
| tac | **5.6x** | 2.9x |
| md5sum | **1.4x** | 1.3x |
| b2sum | **1.3x** | 1.3x |
| sha256sum | **1.2x** | 4.7x |

## Tools

| Tool | Binary | Status | Description |
|------|--------|--------|-------------|
| wc | `fwc` | Optimized | Word, line, char, byte count (SIMD SSE2, single-pass, parallel) |
| cut | `fcut` | Optimized | Field/byte/char extraction (mmap, SIMD) |
| sha256sum | `fsha256sum` | Optimized | SHA-256 checksums (mmap, madvise, readahead, parallel) |
| md5sum | `fmd5sum` | Optimized | MD5 checksums (mmap, madvise, readahead, parallel) |
| b2sum | `fb2sum` | Optimized | BLAKE2b checksums (mmap, madvise, readahead) |
| base64 | `fbase64` | Optimized | Base64 encode/decode (SIMD, parallel, fused strip+decode) |
| sort | `fsort` | Optimized | Line sorting (parallel merge sort) |
| tr | `ftr` | Optimized | Character translation (SIMD pshufb compact, AVX2/SSE2, parallel) |
| uniq | `funiq` | Optimized | Filter duplicate lines (mmap, zero-copy, single-pass) |
| tac | `ftac` | Optimized | Reverse file lines (parallel memchr, zero-copy writev, vmsplice) |

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
- **SIMD range translate/delete**: `tr` detects contiguous byte ranges and uses AVX2/SSE2 SIMD
- **Chunk-based reverse scan**: `tac` processes backward in 512KB chunks with forward SIMD within each chunk
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
