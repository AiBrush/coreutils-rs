# coreutils-rs

[![Test](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml)
[![Release](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/coreutils-rs?color=orange)](https://crates.io/crates/coreutils-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/AiBrush/coreutils-rs)](https://github.com/AiBrush/coreutils-rs/releases)

High-performance GNU coreutils replacement in Rust. Faster with SIMD acceleration. Drop-in compatible, cross-platform.

## Performance (100MB text file)

| Command | GNU | coreutils-rs | Speedup |
|---------|-----|-------------|---------|
| `wc -l` | 42ms | 28ms | **1.5x** |
| `wc -w` | 297ms | 117ms | **2.5x** |
| `wc -c` | ~0ms | ~0ms | instant |
| `wc` (default) | 302ms | 135ms | **2.2x** |
| `cut -d: -f5` | 325ms | 161ms | **2.0x** |
| `cut -b1-20` | 310ms | 49ms | **6.3x** |

## Tools

| Tool | Binary | Status | Description |
|------|--------|--------|-------------|
| wc | `fwc` | Complete | Word, line, byte, char count |
| cut | `fcut` | Complete | Field/byte/char extraction |
| sha256sum | `fsha256sum` | Complete | SHA-256 checksums (SHA-NI) |
| md5sum | `fmd5sum` | Complete | MD5 checksums |
| b2sum | `fb2sum` | Complete | BLAKE2b checksums |
| base64 | `fbase64` | Planned | Base64 encode/decode |
| sort | `fsort` | Planned | Line sorting |
| tr | `ftr` | Planned | Character translation |
| uniq | `funiq` | Planned | Filter duplicate lines |
| tac | `ftac` | Planned | Reverse file lines |

## Installation

```bash
cargo install coreutils-rs
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
```

## Key Optimizations

- **Zero-copy mmap**: Large files are memory-mapped directly, avoiding copies
- **SIMD scanning**: `memchr` crate auto-detects AVX2/NEON for byte searches
- **stat-only byte counting**: `wc -c` uses `stat()` without reading file content
- **Hardware-accelerated hashing**: sha2 crate detects SHA-NI, blake2 uses optimized implementations
- **Optimized release profile**: Fat LTO, single codegen unit, abort on panic, stripped binaries

## GNU Compatibility

Output is byte-identical to GNU coreutils. All flags are supported including `--files0-from`, `--total`, `--complement`, `--check`, and correct column alignment.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for design decisions and [PROGRESS.md](PROGRESS.md) for development status.

## License

MIT
