# fcoreutils: Fix Bugs & Improve Performance

## Overview

You are tasked with fixing compatibility bugs and dramatically improving performance of **fcoreutils** — a Rust reimplementation of GNU coreutils.

- **Source code**: `/home/aibrush/coreutils-rs` (Rust crate, tools are in `src/<toolname>/`)
- **Independent test suite**: `/home/aibrush/coreutils-rs-independent-test`
- **Latest CI report**: `/home/aibrush/coreutils-rs-independent-test/REPORT.md`
- **Published crate**: `fcoreutils` on crates.io

## How to Read the Report

1. `git pull` in `/home/aibrush/coreutils-rs-independent-test` to get the latest REPORT.md
2. Read the **Executive Summary** section for pass/fail counts and overall rating
3. Read the **Compatibility Overview** table for per-tool pass rates
4. Read the **Failed Test Details** section for exact diffs and reproduction commands
5. Read the **Performance Results** section for benchmark numbers per platform (Linux x86_64, Linux aarch64, macOS arm64)
6. The report also includes Windows results but Windows is secondary priority

## Current State (Latest CI Run)

### Compatibility: 1222/1239 passed (98.6%)

| Tool | Pass Rate | Status |
|------|-----------|--------|
| wc | 65.4% | CRITICAL — fix immediately |
| b2sum | 69.6% | CRITICAL — fix immediately |
| sha256sum | 71.2% | CRITICAL — fix immediately |
| md5sum | 72.7% | CRITICAL — fix immediately |
| sort | 94.7% | Fix remaining failures |
| cut | 97.1% | Fix remaining failures |
| tac | 98.7% | Minor fixes |
| base64 | 100% | OK |
| tr | 100% | OK |
| uniq | 100% | OK |

NOTE: The low pass rates for wc/b2sum/sha256sum/md5sum are inflated by Windows platform failures (path handling, missing GNU tools). On Linux/macOS, the actual pass rate is much higher. But ALL failures must be fixed.

### Performance: NOT ACCEPTABLE

These tools are too slow and need significant optimization:

| Tool | Best Speedup | Target | Action Required |
|------|-------------|--------|-----------------|
| md5sum | 1.4x | 5x+ | CRITICAL — needs major optimization (SIMD, parallel I/O) |
| b2sum | 1.4x | 3x+ | CRITICAL — needs optimization (better BLAKE2 impl, SIMD) |
| tr | 2.6x | 5x+ | Needs optimization (lookup table, SIMD for ASCII) |
| tac | 5.3x | 8x+ | Could be better (memory-mapped I/O, better buffering) |
| uniq | 6.7x | 10x+ | Good but can improve (SIMD string comparison) |
| wc | 16.7x | Keep/improve | Good on some platforms, but 0.1x on others — fix regressions |
| base64 | 10.3x | Keep/improve | Good but some cases are 0.1x — fix regressions |
| cut | 12.2x | Keep/improve | Good |
| sha256sum | 11.1x | Keep/improve | Good on macOS, but only 1.0x on Linux — fix |
| sort | 83.7x | Keep | Excellent |

### Specific cases where fcoreutils is SLOWER than GNU (0.x speedup — UNACCEPTABLE):

- `wc (default 1MB text)` on macOS: **0.1x** (10x slower than GNU!)
- `wc (100 files)` on macOS: **0.1x**
- `base64 (decode 1MB)` on macOS: **0.1x**
- `base64 (encode -w 76 10MB)` on macOS: **0.7x**
- `b2sum (100 files)` everywhere: **0.3-0.7x**
- `md5sum` on Linux x86_64: **0.5-0.8x** (SLOWER than GNU on most benchmarks!)
- `sha256sum (100 files)` on Linux: **0.6x**
- `tr (-s spaces 10MB)` on Linux: **0.8-0.9x**
- `tac (custom separator)` on macOS: **0.3x**

## Known Bug Categories to Fix

### Category 1: Binary Name in Error Messages
**Tools affected**: md5sum, sha256sum, b2sum, sort, cut
**Issue**: Error messages say `fmd5sum:` instead of `md5sum:`, `fsort:` instead of `sort:`, etc.
**Root cause**: The tools use `argv[0]` or hardcoded binary name for error messages
**Fix**: Strip the `f` prefix from the program name when generating error messages, OR make the test suite normalize this. The GNU-compatible approach is to show the actual binary name, so this may be a test issue. Investigate both options and pick the right one.

### Category 2: sha256sum -c --status Exit Code
**Issue**: `fsha256sum -c --status` returns exit 1 when all checksums are valid (GNU returns 0)
**This is a real bug** — fix the exit code logic in check mode with --status flag

### Category 3: wc Binary File Word Counting
**Issue**: `fwc` counts 0 words in a 7-byte file with null bytes, GNU counts 1 word
**This is a real bug** — fwc's word boundary detection differs from GNU when null bytes are present

### Category 4: Windows Path Handling
**Tools affected**: wc, md5sum, sha256sum, b2sum (all tools that output filenames)
**Issue**: On Windows/MSYS2, fcoreutils outputs `C:/Users/RUNNER~1/AppData/Local/Temp/...` while GNU tools output `/tmp/...`
**Root cause**: fcoreutils isn't translating MSYS2 paths or is using native Windows paths
**Fix**: On Windows, use the MSYS2/Unix-style path when available, or normalize the path

### Category 5: sort Broken Pipe Error Message
**Issue**: When piped to `head`, the error message shows `fsort:` instead of `sort:`
**Related to Category 1**

### Category 6: cut Tab Delimiter
**Issue**: `cut -d'\t' -f2` — both GNU and fcoreutils error on the literal 2-char string `\t`, but error messages differ (tool name)
**Related to Category 1**

## Performance Optimization Priorities

### Priority 1: md5sum (currently 1.4x, target 5x+)
Source: `src/md5sum/` and `src/hash/`
- Use hardware-accelerated MD5 (x86: SSE4.2/AVX2, ARM: crypto extensions)
- Consider the `md-5` crate with `asm` feature or a custom SIMD implementation
- Implement parallel file hashing for multi-file input (rayon)
- Use memory-mapped I/O for large files
- Optimize buffer sizes (64KB+ read buffers)

### Priority 2: b2sum (currently 1.4x, target 3x+)
Source: `src/b2sum/` and `src/hash/`
- Use the `blake2` crate with SIMD support
- On x86, use AVX2 BLAKE2 implementations
- On ARM, use NEON-optimized BLAKE2
- Parallel file hashing with rayon
- Memory-mapped I/O for large files
- Multi-file batch processing is **0.3-0.7x** — this needs urgent attention

### Priority 3: tr (currently 2.6x, target 5x+)
Source: `src/tr/`
- Build 256-byte lookup table for single-byte translations
- Use SIMD (SSE2/AVX2 on x86, NEON on ARM) for bulk character translation
- Optimize squeeze (-s) with SIMD
- Process in 64KB+ chunks

### Priority 4: Fix Performance Regressions
- `wc` on macOS 1MB files: startup overhead? Fix cold-start path
- `base64 decode 1MB`: likely buffer management issue
- `sha256sum` on Linux x86_64 is barely 1.0x despite being 11.1x on macOS — likely not using SHA-NI instructions on Linux
- Multi-file operations (100 files) are slow everywhere — batch I/O overhead

### Priority 5: tac, uniq, wc further improvements
- tac: Use mmap for large files, optimize custom separator case
- uniq: SIMD string comparison for line dedup
- wc: Ensure SIMD paths are used on ALL platforms (the 16.7x on Linux x86_64 suggests SIMD works there but not elsewhere)

## Workflow

### Git Setup
The source code repo is at `/home/aibrush/coreutils-rs`. Work on a feature branch.

### Never Run Tests Locally
This machine is resource-constrained. Never run `cargo test` or any test locally.

### CI-First Development
1. Make changes to the Rust source code in `/home/aibrush/coreutils-rs`
2. Commit and push to trigger CI
3. The independent test suite at `/home/aibrush/coreutils-rs-independent-test` will need a `cargo install fcoreutils` update — but since it installs from crates.io, you need to either:
   - Publish a new version to crates.io, OR
   - Modify the test CI to install from the git repo instead
4. Check CI results and iterate

### Using Teammates with Git Worktree

Use **exactly 2 teammates maximum** at a time (this is a low-end machine).

Setup worktrees:
```bash
# Create worktree directory if needed
mkdir -p /home/aibrush/coreutils-worktrees

# Create worktrees for parallel work
cd /home/aibrush/coreutils-rs
git worktree add /home/aibrush/coreutils-worktrees/perf-md5sum perf-md5sum
git worktree add /home/aibrush/coreutils-worktrees/perf-b2sum perf-b2sum
```

Each teammate works in their own worktree on a separate branch. Example team structure:

**Teammate 1**: Fix compatibility bugs (branch: `fix/compat-bugs`)
- Fix sha256sum --status exit code
- Fix wc null byte word counting
- Fix Windows path handling
- Fix error message tool names

**Teammate 2**: Optimize performance (branch: `perf/md5sum-b2sum`)
- Optimize md5sum with hardware acceleration
- Optimize b2sum with SIMD
- Fix multi-file performance regression

Then swap:

**Teammate 1**: Optimize tr and fix regressions (branch: `perf/tr-regressions`)
- Optimize tr with lookup tables and SIMD
- Fix wc macOS regression
- Fix base64 decode regression

**Teammate 2**: Optimize remaining tools (branch: `perf/tac-uniq-sha256`)
- Optimize tac with mmap
- Optimize uniq with SIMD
- Fix sha256sum Linux performance

### After each push, wait for CI, read the updated REPORT.md, and iterate.

## Key Files in Source Code

```
/home/aibrush/coreutils-rs/
  Cargo.toml              — workspace/crate config
  src/
    lib.rs                — library root
    common/               — shared utilities
    bin/                  — binary entry points
    wc/                   — wc implementation
    cut/                  — cut implementation
    sha256sum/            — sha256sum implementation
    md5sum/               — md5sum implementation
    b2sum/                — b2sum implementation
    base64/               — base64 implementation
    sort/                 — sort implementation
    tr/                   — tr implementation
    uniq/                 — uniq implementation
    tac/                  — tac implementation
    hash/                 — shared hash infrastructure
```

## Success Criteria

1. **Compatibility**: 100% pass rate on Linux and macOS (all 17 current failures fixed)
2. **Performance**: No tool should EVER be slower than GNU (no 0.x speedups)
3. **Performance**: md5sum must reach at least 5x speedup
4. **Performance**: b2sum must reach at least 3x speedup
5. **Performance**: tr must reach at least 5x speedup
6. **Performance**: All existing fast tools (sort 83x, wc 16x, cut 12x, sha256sum 11x, base64 10x) must maintain their speedups
7. **Overall rating**: REPORT.md must say **READY**

## Important Reminders

- Always read the REPORT.md FIRST before making changes — understand what's failing
- Use `git pull` in the test repo to get the latest report after each CI run
- Do NOT run tests locally — the machine can't handle it
- Maximum 2 teammates at a time using git worktree
- Focus on the worst performers first (md5sum, b2sum, tr)
- Fix ALL compatibility bugs — 100% is the only acceptable pass rate
- When in doubt, read the GNU coreutils source to understand expected behavior
- Track progress in a PROGRESS.md file
