# fcoreutils

[![Test](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml)
[![Release](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/fcoreutils?color=orange)](https://crates.io/crates/fcoreutils)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/AiBrush/coreutils-rs)](https://github.com/AiBrush/coreutils-rs/releases)

High-performance GNU coreutils replacement in Rust. Faster with SIMD acceleration. Drop-in compatible, cross-platform.

## Performance (independent benchmarks v0.1.4, Linux x86_64, hyperfine)

| Platform | Tests | Passed | Failed | Pass Rate |
|----------|-------|--------|--------|-----------|
| Linux_aarch64 | 413 | 413 | 0 | 100.0% |
| Linux_x86_64 | 413 | 413 | 0 | 100.0% |
| Tool | Test | GNU (mean) | fcoreutils (mean) | uutils (mean) | f* vs GNU | f* vs uutils |
|------|------|-----------|-------------------|---------------|----------:|-------------:|
| wc | default 100KB text | 0.0011s | 0.0012s | 0.0013s | **1.0x** | **1.2x** |
| wc | default 1MB text | 0.0038s | 0.0025s | 0.0034s | **1.5x** | **1.3x** |
| wc | default 10MB text | 0.0345s | 0.0063s | 0.0252s | **5.5x** | **4.0x** |
| wc | default 100MB text | 0.2988s | 0.0451s | 0.2209s | **6.6x** | **4.9x** |
| wc | -l 10MB text | 0.0044s | 0.0022s | 0.0028s | **2.0x** | **1.3x** |
| wc | -w 10MB text | 0.0343s | 0.0063s | 0.0216s | **5.4x** | **3.4x** |
| wc | -c 10MB text | 0.0007s | 0.0009s | 0.0010s | **0.8x** | **1.1x** |
| wc | -m 10MB text | 0.0343s | 0.0025s | 0.0031s | **13.6x** | **1.2x** |
| wc | -L 10MB text | 0.0343s | 0.0063s | 0.0177s | **5.4x** | **2.8x** |
| wc | default 10MB binary | 0.2340s | 0.0169s | 0.1142s | **13.8x** | **6.8x** |
| wc | default 10MB repetitive | 0.0542s | 0.0084s | 0.0380s | **6.4x** | **4.5x** |
| wc | 10 files | 0.0008s | 0.0011s | 0.0011s | **0.8x** | **1.0x** |
| wc | 100 files | 0.0013s | 0.0014s | 0.0017s | **0.9x** | **1.2x** |
| cut | -b1-100 10MB CSV | 0.0187s | 0.0035s | 0.0065s | **5.4x** | **1.9x** |
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
| tr | `ftr` | Optimized | Character translation (SIMD range translate/delete, AVX2/SSE2, parallel) |
| uniq | `funiq` | Optimized | Filter duplicate lines (mmap, zero-copy, single-pass) |
| tac | `ftac` | Optimized | Reverse file lines (chunk-based SIMD scan, zero-copy writev) |

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
