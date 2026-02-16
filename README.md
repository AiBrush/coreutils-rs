<sub>🏆 Visiting from the **"Built with Opus 4.6: a Claude Code hackathon"**? See our [hackathon submission](Fcoreutils-hackathon-submission.md) for the full story.</sub>

---
# fcoreutils

[![Test](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml)
[![Release](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/fcoreutils?color=orange)](https://crates.io/crates/fcoreutils)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/AiBrush/coreutils-rs)](https://github.com/AiBrush/coreutils-rs/releases)

High-performance GNU coreutils replacement in Rust — 21 tools and counting. SIMD-accelerated, drop-in compatible, cross-platform.

## Performance ([independent benchmarks](https://github.com/AiBrush/coreutils-rs-independent-test) v0.5.8, Linux, hyperfine)

| Tool | Speedup vs GNU | Speedup vs uutils |
|------|---------------:|-------------------:|
| wc | **34.2x** | 20.7x |
| sort | **17.7x** | 16.6x |
| uniq | **14.9x** | 6.0x |
| tr | **7.2x** | 7.1x |
| base64 | **6.8x** | 7.1x |
| cut | **6.2x** | 3.7x |
| tac | **3.8x** | 2.0x |
| md5sum | **1.4x** | 1.3x |
| b2sum | **1.3x** | 1.2x |
| sha256sum | **1.0x** | 4.8x |

## Tools

### Performance-Optimized (10 tools, independently benchmarked)

| Tool | Binary | Description |
|------|--------|-------------|
| wc | `fwc` | Word, line, char, byte count (SIMD SSE2, single-pass, parallel) |
| cut | `fcut` | Field/byte/char extraction (mmap, SIMD) |
| sha256sum | `fsha256sum` | SHA-256 checksums (mmap, madvise, readahead, parallel) |
| md5sum | `fmd5sum` | MD5 checksums (mmap, batch I/O, parallel hash, batched output) |
| b2sum | `fb2sum` | BLAKE2b checksums (mmap, madvise, readahead) |
| base64 | `fbase64` | Base64 encode/decode (SIMD, parallel, fused strip+decode) |
| sort | `fsort` | Line sorting (parallel merge sort) |
| tr | `ftr` | Character translation (SIMD pshufb compact, AVX2/SSE2, parallel) |
| uniq | `funiq` | Filter duplicate lines (mmap, zero-copy, single-pass) |
| tac | `ftac` | Reverse file lines (parallel memchr, zero-copy writev, vmsplice) |

### Additional Tools (11 tools, GNU-compatible)

| Tool | Binary | Description |
|------|--------|-------------|
| head | `fhead` | Output first lines of files (zero-copy mmap, SIMD newline scan) |
| tail | `ftail` | Output last lines of files (reverse SIMD scan, follow mode) |
| cat | `fcat` | Concatenate files (zero-copy splice/sendfile, mmap) |
| rev | `frev` | Reverse lines character-by-character (mmap, SIMD) |
| expand | `fexpand` | Convert tabs to spaces (mmap, configurable tab stops) |
| unexpand | `funexpand` | Convert spaces to tabs (mmap, configurable tab stops) |
| fold | `ffold` | Wrap lines to specified width (mmap, byte/char modes) |
| paste | `fpaste` | Merge lines of files (mmap, serial/parallel modes) |
| nl | `fnl` | Number lines of files (mmap, section delimiters, regex) |
| comm | `fcomm` | Compare sorted files line by line (mmap, SIMD) |
| join | `fjoin` | Join lines of two sorted files on a common field (mmap) |

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

# File viewing and transformation
fhead -n 20 file.txt      # First 20 lines
ftail -n 20 file.txt      # Last 20 lines
ftail -f logfile.txt      # Follow file for new lines
fcat file1.txt file2.txt  # Concatenate files
fcat -n file.txt          # With line numbers
frev file.txt             # Reverse each line

# Text formatting
fexpand file.txt          # Convert tabs to spaces
funexpand file.txt        # Convert spaces to tabs
ffold -w 80 file.txt      # Wrap lines at 80 columns
fnl file.txt              # Number lines
fpaste file1 file2        # Merge files line by line
fpaste -s file.txt        # Serial mode (join all lines)

# Set operations on sorted files
fcomm file1 file2         # Compare two sorted files
fcomm -12 file1 file2     # Only lines common to both
fjoin file1 file2         # Join on common field
fjoin -t, -1 2 -2 1 a b  # Join CSV files on specific fields
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
