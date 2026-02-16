<sub>🏆 Visiting from the **"Built with Opus 4.6: a Claude Code hackathon"**? See our [hackathon submission](Fcoreutils-hackathon-submission.md) for the full story.</sub>

---
# fcoreutils

[![Test](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/test.yml)
[![Release](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml/badge.svg)](https://github.com/AiBrush/coreutils-rs/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/fcoreutils?color=orange)](https://crates.io/crates/fcoreutils)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/AiBrush/coreutils-rs)](https://github.com/AiBrush/coreutils-rs/releases)

High-performance GNU coreutils replacement in Rust — 71 tools and counting. SIMD-accelerated, drop-in compatible, cross-platform.

## Performance ([independent benchmarks](https://github.com/AiBrush/coreutils-rs-independent-test) v0.6.8, Linux, hyperfine)

| Tool | Speedup vs GNU | Speedup vs uutils |
|------|---------------:|-------------------:|
| wc | **34.9x** | 19.1x |
| sort | **19.1x** | 16.5x |
| uniq | **16.5x** | 6.6x |
| base64 | **7.1x** | 7.0x |
| tr | **6.9x** | 7.2x |
| cut | **6.5x** | 3.3x |
| tac | **3.8x** | 2.0x |
| md5sum | **1.4x** | 1.3x |
| b2sum | **1.3x** | 1.2x |
| sha256sum | **1.0x** | 4.0x |

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


## *NOT INCLUDED IN HACKATHON SUBMISSION*
### Additional Tools (61 tools)

| Tool | Binary | Description | Compatibility | Passed | Failed | Skipped | Total |
|------|--------|-------------|---------------|--------|--------|---------|-------|
| head | `fhead` | Output first lines of files (zero-copy mmap, SIMD newline scan) | :white_check_mark: | 47 | 0 | 0 | 47 |
| tail | `ftail` | Output last lines of files (reverse SIMD scan, follow mode) | :white_check_mark: | 44 | 0 | 0 | 44 |
| cat | `fcat` | Concatenate files (zero-copy splice/sendfile, mmap) | :white_check_mark: | 44 | 0 | 0 | 44 |
| rev | `frev` | Reverse lines character-by-character (mmap, SIMD) | :white_check_mark: | 32 | 0 | 0 | 32 |
| expand | `fexpand` | Convert tabs to spaces (mmap, configurable tab stops) | :white_check_mark: | 33 | 0 | 0 | 33 |
| unexpand | `funexpand` | Convert spaces to tabs (mmap, configurable tab stops) | :white_check_mark: | 26 | 0 | 0 | 26 |
| fold | `ffold` | Wrap lines to specified width (mmap, byte/char modes) | :white_check_mark: | 35 | 0 | 0 | 35 |
| paste | `fpaste` | Merge lines of files (mmap, serial/parallel modes) | :white_check_mark: | 30 | 0 | 0 | 30 |
| nl | `fnl` | Number lines of files (mmap, section delimiters, regex) | :white_check_mark: | 47 | 0 | 0 | 47 |
| comm | `fcomm` | Compare sorted files line by line (mmap, SIMD) | :white_check_mark: | 30 | 0 | 0 | 30 |
| join | `fjoin` | Join lines of two sorted files on a common field (mmap) | :white_check_mark: | 35 | 0 | 0 | 35 |
| base32 | `fbase32` | RFC 4648 base32 encoding/decoding | :white_check_mark: | 29 | 0 | 0 | 29 |
| basenc | `fbasenc` | Multi-format encoder/decoder (base64, base32, base16, base2, z85) | :white_check_mark: | 40 | 0 | 0 | 40 |
| sha1sum | `fsha1sum` | SHA-1 checksums | :white_check_mark: | 15 | 0 | 0 | 15 |
| sha224sum | `fsha224sum` | SHA-224 checksums | :white_check_mark: | 10 | 0 | 0 | 10 |
| sha384sum | `fsha384sum` | SHA-384 checksums | :white_check_mark: | 10 | 0 | 0 | 10 |
| sha512sum | `fsha512sum` | SHA-512 checksums | :white_check_mark: | 10 | 0 | 0 | 10 |
| sum | `fsum` | BSD/SysV checksums | :white_check_mark: | 23 | 0 | 0 | 23 |
| cksum | `fcksum` | CRC-32 checksums | :white_check_mark: | 21 | 0 | 0 | 21 |
| ln | `fln` | Create hard and symbolic links | :white_check_mark: | 16 | 0 | 0 | 16 |
| touch | `ftouch` | Change file timestamps | :white_check_mark: | 21 | 0 | 0 | 21 |
| truncate | `ftruncate` | Shrink or extend file sizes | :white_check_mark: | 25 | 0 | 0 | 25 |
| mkdir | `fmkdir` | Create directories (symbolic mode support) | :white_check_mark: | 17 | 0 | 0 | 17 |
| rmdir | `frmdir` | Remove empty directories | :white_check_mark: | 12 | 0 | 0 | 12 |
| mkfifo | `fmkfifo` | Create named pipes (FIFOs) | :white_check_mark: | 11 | 0 | 0 | 11 |
| mknod | `fmknod` | Create special files | :white_check_mark: | 10 | 0 | 0 | 10 |
| mktemp | `fmktemp` | Create temporary files/directories | :white_check_mark: | 15 | 0 | 0 | 15 |
| link | `flink` | Create hard link (low-level) | :white_check_mark: | 8 | 0 | 0 | 8 |
| unlink | `funlink` | Remove file (low-level) | :white_check_mark: | 7 | 0 | 0 | 7 |
| basename | `fbasename` | Strip directory and suffix from paths | :white_check_mark: | 26 | 0 | 0 | 26 |
| dirname | `fdirname` | Strip last path component | :white_check_mark: | 23 | 0 | 0 | 23 |
| readlink | `freadlink` | Print symlink targets | :white_check_mark: | 19 | 0 | 0 | 19 |
| realpath | `frealpath` | Resolve absolute paths | :white_check_mark: | 24 | 0 | 0 | 24 |
| pathchk | `fpathchk` | Validate path names | :white_check_mark: | 17 | 0 | 0 | 17 |
| seq | `fseq` | Generate number sequences | :white_check_mark: | 53 | 0 | 0 | 53 |
| shuf | `fshuf` | Random permutations of input | :white_check_mark: | 27 | 0 | 0 | 27 |
| tsort | `ftsort` | Topological sorting | :white_check_mark: | 19 | 0 | 0 | 19 |
| tee | `ftee` | Read stdin, write to stdout and files | :white_check_mark: | 15 | 0 | 0 | 15 |
| yes | `fyes` | Output a string repeatedly | :white_check_mark: | 5 | 0 | 0 | 5 |
| id | `fid` | Print user and group IDs | :white_check_mark: | 16 | 0 | 0 | 16 |
| groups | `fgroups` | Print group memberships | :white_check_mark: | 4 | 0 | 0 | 4 |
| whoami | `fwhoami` | Print effective user name | :white_check_mark: | 4 | 0 | 0 | 4 |
| logname | `flogname` | Print login name | :white_check_mark: | 3 | 0 | 0 | 3 |
| uname | `funame` | Print system information | :white_check_mark: | 14 | 0 | 0 | 14 |
| uptime | `fuptime` | System uptime and load averages | :white_check_mark: | 5 | 0 | 0 | 5 |
| arch | `farch` | Print machine architecture | :white_check_mark: | 5 | 0 | 0 | 5 |
| hostid | `fhostid` | Print host identifier | :white_check_mark: | 6 | 0 | 0 | 6 |
| tty | `ftty` | Print terminal name | :white_check_mark: | 6 | 0 | 0 | 6 |
| nproc | `fnproc` | Print number of processors | :white_check_mark: | 8 | 0 | 0 | 8 |
| pwd | `fpwd` | Print working directory | :white_check_mark: | 8 | 0 | 0 | 8 |
| printenv | `fprintenv` | Print environment variables | :white_check_mark: | 5 | 0 | 0 | 5 |
| env | `fenv` | Run program with modified environment | :white_check_mark: | 17 | 0 | 0 | 17 |
| timeout | `ftimeout` | Run command with time limit | :white_check_mark: | 21 | 0 | 0 | 21 |
| nice | `fnice` | Run with modified scheduling priority | :white_check_mark: | 12 | 0 | 0 | 12 |
| nohup | `fnohup` | Run immune to hangups | :white_check_mark: | 6 | 0 | 0 | 6 |
| sleep | `fsleep` | Delay for specified time | :white_check_mark: | 10 | 0 | 0 | 10 |
| sync | `fsync` | Flush filesystem caches | :white_check_mark: | 5 | 0 | 1 | 6 |
| chroot | `fchroot` | Change root directory (requires root) | :white_check_mark: | 11 | 0 | 0 | 11 |
| true | `ftrue` | Exit with status 0 | :white_check_mark: | 8 | 0 | 0 | 8 |
| false | `ffalse` | Exit with status 1 | :white_check_mark: | 7 | 0 | 0 | 7 |
| dircolors | `fdircolors` | Setup LS_COLORS environment variable | :white_check_mark: | 14 | 0 | 0 | 14 |


## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for design decisions and [PROGRESS.md](PROGRESS.md) for development status.

## Security

To report a vulnerability, please see our [Security Policy](SECURITY.md).

## License

MIT
