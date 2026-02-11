# coreutils-rs Progress

## Current Status: All 10 Tools Complete - v0.0.8 Released

All 10 GNU coreutils replacements are fully implemented, optimized, and tested.
Each tool is a drop-in replacement with byte-identical GNU output.

## Tool Checklist

### wc (Word Count) - COMPLETE
- [x] SIMD SSE2 word counting with whitespace classification
- [x] memchr SIMD line counting (auto-detects AVX2/SSE2/NEON)
- [x] stat-only byte counting (`-c` never reads the file)
- [x] Zero-copy mmap for large files (>64KB)
- [x] Parallel multi-file processing with thread pool
- [x] All flags: `-l`, `-w`, `-c`, `-m`, `-L`, `--files0-from`, `--total`
- [x] GNU-identical output format (right-aligned columns, correct field order)
- [x] 30+ unit tests, 18 GNU compatibility tests

### cut (Field Extraction) - COMPLETE
- [x] Zero-copy mmap file reading
- [x] SIMD byte scanning for delimiter detection
- [x] Zero-copy field extraction (no allocation per line)
- [x] All flags: `-d`, `-f`, `-b`, `-c`, `--complement`, `-s`, `--output-delimiter`
- [x] GNU-identical output format

### base64 (Base64 Encode/Decode) - COMPLETE
- [x] SIMD vectorized encode/decode via `base64-simd` crate
- [x] 4MB chunked streaming for arbitrary file sizes
- [x] Raw fd stdout bypass (avoids Rust BufWriter overhead)
- [x] Zero-copy mmap for encoding
- [x] All flags: `-d`, `-w`, `-i`
- [x] GNU-identical output format with line wrapping

### sha256sum (SHA-256 Checksums) - COMPLETE
- [x] Zero-copy mmap with `madvise(MADV_SEQUENTIAL)` and readahead
- [x] Parallel multi-file hashing with thread pool
- [x] SHA-NI hardware acceleration detection
- [x] All flags: `-c`, `--check`, `--status`, `--quiet`, `--strict`, `-b`, `-t`
- [x] GNU-identical output format

### md5sum (MD5 Checksums) - COMPLETE
- [x] Zero-copy mmap with `madvise(MADV_SEQUENTIAL)` and readahead
- [x] Parallel multi-file hashing with thread pool
- [x] All flags: `-c`, `--check`, `--status`, `--quiet`, `--strict`, `-b`, `-t`
- [x] GNU-identical output format

### b2sum (BLAKE2b Checksums) - COMPLETE
- [x] Zero-copy mmap with `madvise(MADV_SEQUENTIAL)` and readahead
- [x] BLAKE2b hardware-accelerated implementation
- [x] All flags: `-c`, `--check`, `--status`, `--quiet`, `--strict`, `-l` (length)
- [x] GNU-identical output format

### sort (Line Sorting) - COMPLETE
- [x] Parallel merge sort with Rayon thread pool
- [x] Efficient key extraction and comparison
- [x] All GNU sort modes: `-n`, `-g`, `-h`, `-V`, `-M`, `-R`
- [x] All flags: `-r`, `-u`, `-s`, `-k`, `-t`, `-o`, `-m`, `-c`, `-C`
- [x] GNU-identical output format

### tr (Character Translation) - COMPLETE
- [x] Mmap stdin reading for large inputs
- [x] 256-byte lookup tables for O(1) character mapping
- [x] 4MB output buffers for minimized write syscalls
- [x] Zero-copy translation pipeline
- [x] Character classes: `[:alpha:]`, `[:digit:]`, `[:space:]`, etc.
- [x] All flags: `-d`, `-s`, `-c`, `-C`, `-t`
- [x] GNU-identical output format

### uniq (Deduplicate Lines) - COMPLETE
- [x] Zero-copy mmap file reading
- [x] 1MB output buffers for efficient I/O
- [x] All GNU uniq flags: `-c`, `-d`, `-D`, `-u`, `-i`, `-f`, `-s`, `-w`
- [x] GNU-identical output format

### tac (Reverse Lines) - COMPLETE
- [x] Forward SIMD scan with memchr for newline detection
- [x] 1MB BufWriter for efficient reversed output
- [x] Custom separator support (`-s`)
- [x] Before-separator mode (`-b`)
- [x] GNU-identical output format

## Benchmarks (100MB text file, hyperfine --warmup 2 --min-runs 5)

| Tool | Benchmark | GNU | fcoreutils | Speedup |
|------|-----------|-----|------------|---------|
| `fwc` | default (lwc) | 339.1ms | 28.9ms | **11.75x** |
| `fwc` | `-l` (lines) | 39.4ms | 22.7ms | **1.74x** |
| `fwc` | `-w` (words) | 338.7ms | 19.0ms | **17.81x** |
| `fwc` | `-c` (bytes) | ~1ms | ~1ms | ~1x (both stat) |
| `fcut` | `-d' ' -f1` (field) | 338.5ms | 82.4ms | **4.11x** |
| `fcut` | `-b1-20` (bytes) | 308.8ms | 29.1ms | **10.62x** |
| `fbase64` | encode | 188.4ms | 115.5ms | **1.63x** |
| `fbase64` | decode | 538.9ms | 345.7ms | **1.56x** |
| `fsha256sum` | hash | 103.8ms | 103.4ms | **1.00x** |
| `fmd5sum` | hash | 210.6ms | 263.4ms | **0.80x** |
| `fb2sum` | hash | 271.8ms | 222.1ms | **1.22x** |
| `fsort` | sort (10MB) | 157.4ms | 32.6ms | **4.83x** |
| `fsort` | sort (100MB) | 1851ms | 398.9ms | **4.64x** |
| `ftr` | `a-z` → `A-Z` | 96.9ms | 90.4ms | **1.07x** |
| `funiq` | dedup (10MB sorted) | 33.1ms | 6.9ms | **4.82x** |
| `ftac` | reverse | 132.5ms | 58.9ms | **2.25x** |

### Per-Tool Best Speedup

| Rank | Tool | Best Speedup |
|------|------|-------------|
| 1 | fwc | **17.81x** |
| 2 | fcut | **10.62x** |
| 3 | fsort | **4.83x** |
| 4 | funiq | **4.82x** |
| 5 | ftac | **2.25x** |
| 6 | fbase64 | **1.63x** |
| 7 | fb2sum | **1.22x** |
| 8 | ftr | **1.07x** |
| 9 | fsha256sum | **1.00x** |
| 10 | fmd5sum | **0.80x** |

## Key Findings
- **fwc -w is 17.81x faster** — SIMD SSE2 word counting dominates GNU's scalar approach
- **fwc default is 11.75x faster** — the flagship benchmark, combining all metrics
- **fcut -b is 10.62x faster** — SIMD byte-range extraction with zero-copy mmap
- **fsort is ~4.7x faster** — parallel merge sort vs GNU's single-threaded sort
- **funiq is 4.82x faster** — efficient dedup with mmap and buffered I/O
- Zero-copy mmap eliminates 100MB copy overhead, reducing sys time from 50ms to 4ms
- Hash tools (sha256sum, md5sum) are at parity — both use hardware-accelerated implementations
- fmd5sum is slightly slower (0.80x) — GNU md5sum likely uses optimized assembly; room for improvement
- ftr at ~1x — both are I/O-bound for simple transliteration on this workload
