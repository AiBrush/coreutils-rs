# fcoreutils — Fix Bugs & Maximize Performance

## Your Mission

An independent test suite has been run against fcoreutils. The results are bad. You need to fix every compatibility bug and dramatically improve performance. The current numbers are not acceptable.

**Independent Test Report**: https://github.com/AiBrush/coreutils-rs-independent-test/blob/main/REPORT.md

Read this report carefully before starting. It contains:
- Exact test failures with diffs and reproduction scripts
- Measured performance numbers (GNU vs fcoreutils) across Linux x86_64, Linux ARM64, and macOS ARM64
- Per-tool compatibility pass rates

## Current State (From Independent Tests)

### Compatibility — NEEDS WORK

| Tool | Pass Rate | Status |
|------|-----------|--------|
| wc | 64.5% | FAILING — many test failures |
| cut | 97.1% | CLOSE — a few edge cases |
| sha256sum | 66.5% | FAILING — check mode issues |
| md5sum | 66.0% | FAILING — check mode issues |
| b2sum | 64.8% | FAILING — check mode issues |
| base64 | 100.0% | PASSING |
| sort | 92.7% | CLOSE — some flag issues |
| tr | 100.0% | PASSING |
| uniq | 100.0% | PASSING |
| tac | 82.0% | NEEDS WORK |

**Target: 100% compatibility on all tools.** Output must be byte-identical to GNU coreutils.

### Performance — NOT ACCEPTABLE

Many tools are barely faster than GNU or even SLOWER. The following need major improvement:

| Tool | Current Best | Target | Priority |
|------|-------------|--------|----------|
| md5sum | **0.80x** (SLOWER than GNU!) | 5x+ | CRITICAL |
| sha256sum | **1.0x** (same as GNU) | 4x+ | CRITICAL |
| tr | **1.07x** | 5x+ | HIGH |
| base64 | **1.6x** | 5x+ | HIGH |
| b2sum | **1.22x** | 5x+ | HIGH |
| tac | **2.25x** | 5x+ | MEDIUM |
| cut | **4.1x** | 8x+ | MEDIUM |
| sort | **4.8x** | 8x+ | MEDIUM |
| wc | **11.75x** (best case, but -L is only 1.4x) | maintain/improve | LOW |
| uniq | **4.82x** | 8x+ | MEDIUM |

## How to Work

### Use Teammates with Git Worktree

You MUST use teammates to parallelize the work. Use `git worktree` so teammates can work on different tools simultaneously without conflicts.

**Rules:**
- Maximum **2 teammates** at a time (low-end machine, limited resources)
- Each teammate works on a **separate tool** in its own worktree
- Coordinate through the task list — no two teammates touch the same file
- After a teammate finishes one tool, move to the next

**Worktree setup pattern:**
```bash
# Create worktrees for parallel work
git worktree add ../coreutils-rs-fix-md5sum fix-md5sum
git worktree add ../coreutils-rs-fix-sha256sum fix-sha256sum
```

### Workflow

1. **Read the full independent test report** from the link above
2. **Prioritize** — fix the worst compatibility issues first, then performance
3. **Create branches** per tool or per issue
4. **Fix and verify locally** with `cargo test`
5. **Push branches** and create PRs
6. After merging fixes, the independent test suite will re-run automatically

### Priority Order

**Phase 1 — Critical Compatibility Fixes:**
1. Fix `wc` — 64.5% pass rate is unacceptable. Read the failed test diffs carefully.
2. Fix `sha256sum` / `md5sum` / `b2sum` — check mode (`-c`) is broken. `--ignore-missing`, `--strict`, `--warn` flags have issues.
3. Fix `tac` — 82% pass rate, likely separator/edge case issues.
4. Fix `sort` — 92.7%, close but not there.
5. Fix `cut` — 97.1%, minor edge cases.

**Phase 2 — Performance (the big wins):**
1. `md5sum` — currently SLOWER than GNU. Use hardware-accelerated MD5 (md5-asm crate or hand-rolled assembly). Consider parallel file processing.
2. `sha256sum` — at parity with GNU. Use SHA-NI intrinsics directly, not through a generic crypto crate. Parallel multi-file hashing.
3. `tr` — 1.07x is embarrassing. Use SIMD lookup tables for character translation. Process 16/32 bytes at a time with SSE2/AVX2.
4. `base64` — 1.6x is weak. Use AVX2 base64 encoding/decoding. Consider `base64-simd` with proper SIMD backend selection.
5. `b2sum` — use the `blake2b_simd` crate with SIMD acceleration. Parallel file processing.
6. `tac` — use reverse mmap reading, avoid copying data. Read file backwards in large chunks.
7. `sort` — parallel merge sort with better partitioning. Use SIMD string comparisons.
8. `uniq` — SIMD line comparison, avoid per-byte comparison.
9. `cut` — SIMD delimiter scanning with AVX2 `_mm256_cmpeq_epi8`.

### Performance Optimization Techniques to Apply

- **SIMD everywhere**: SSE2/AVX2 on x86_64, NEON on ARM64. Don't use generic byte-at-a-time loops.
- **Zero-copy mmap**: Use `mmap` for file reading. Use `madvise(MADV_SEQUENTIAL)` for sequential reads.
- **Parallel processing**: Use `rayon` for multi-file operations. Process files in parallel with thread pools.
- **Minimize syscalls**: Use large buffer sizes (4MB+). Batch writes. Use `writev` for scatter-gather I/O.
- **Avoid allocations**: Reuse buffers. Use `&[u8]` slices instead of `String`. Pre-allocate output buffers.
- **Hardware crypto**: For hash tools, use CPU-specific hardware acceleration (SHA-NI, AES-NI). Detect at runtime and dispatch.
- **Raw fd I/O**: Bypass Rust's `BufWriter` for hot paths. Write directly to fd with large buffers.

## Important Notes

- **Never run tests locally** — the machine is too busy. Push code and let CI run.
- **Always maintain PROGRESS.md** — update it as you fix things.
- **Read the reproduction scripts** in the test report — they show exact commands and expected vs actual output.
- **Byte-identical output is non-negotiable** — if GNU outputs a space before a number, you output a space before a number. If GNU prints an error to stderr, you print the same error to stderr.
- **Don't break what already works** — base64, tr, and uniq are at 100%. Don't regress them.
- **Benchmark with hyperfine** before and after your changes to verify speedups: `hyperfine --warmup 3 'gnu_tool file' 'ftool file'`

## Files Reference

- Source code: `src/` (one file per tool, e.g., `src/wc.rs`, `src/md5sum.rs`)
- Tests: `tests/`
- Benchmarks: `benches/`
- Progress: `PROGRESS.md`
- Architecture: `ARCHITECTURE.md`
