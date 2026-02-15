# Optimization Experiments Log

## CRITICAL: Read Before Every Optimization Round
Before starting ANY optimization work, review this file and the regression analysis below.
Check what has been tried, what worked, and what REGRESSED performance.

## Regression Analysis: v0.4.3 (peak) vs v0.5.3 (current)

### Per-Tool Regressions (10MB benchmark, x86_64 CI)

| Tool | Benchmark | v0.4.3 time | v0.5.3 time | Change | Status |
|------|-----------|-------------|-------------|--------|--------|
| tr | a-z→A-Z 10MB | 0.0038s | **0.0078s** | **+105% SLOWER** | CRITICAL |
| tr | -d digits 10MB | 0.0052s | **0.0072s** | **+38% SLOWER** | CRITICAL |
| cut | -d, -f1 10MB | 0.0031s | **0.0044s** | **+42% SLOWER** | MAJOR |
| cut | -d, -f1 1MB | 0.0015s | **0.0025s** | **+67% SLOWER** | MAJOR |
| base64 | decode 10MB | 0.0037s | 0.0043s | +16% slower | moderate |
| base64 | encode 10MB | 0.0033s | 0.0040s | +21% slower | moderate |
| tac | reverse 10MB | 0.0060s | 0.0047s | -22% FASTER | improved |
| uniq | dedup 10MB | 0.0035s | 0.0033s | -6% faster | stable |
| wc | all modes | ~same | ~same | stable | stable |
| sort | all modes | ~same | ~same | stable | stable |

### Root Cause: PRs That Caused Regressions

**PR #197 (perf: optimize tr/tac I/O paths for 10x speedup)**
- Changed tr from simple lookup-table translate to parallel chunks
- RESULT: tr went from 0.0038s to ~0.0078s — 2x SLOWER
- Likely cause: Parallel overhead + thread management for 10MB files (too small for parallelism)

**PR #208 (perf: optimize base64 + cut for 10x speedup)**
- Replaced Rayon with std::thread::scope in cut
- Added MADV_POPULATE_WRITE for base64
- RESULT: cut -d,-f1 went from 0.0031s to 0.0044s — 42% slower
- Likely cause: std::thread::scope creates/destroys threads each call vs Rayon's pool

### Key Lessons Learned
1. **DO NOT parallelize for <50MB files** — thread overhead dominates for 10MB
2. **Rayon > std::thread::scope** — Rayon's pool amortizes thread creation
3. **MADV_POPULATE_WRITE hurts for small files** — prefaulting pages isn't free
4. **The independent benchmarks use 10MB files** — optimize for that size, not 100MB
5. **Always compare against the PEAK version** (v0.4.3) not just the previous version
6. **Simple scalar code can beat SIMD for small data** if SIMD adds setup overhead

---

## Experiment Log

### EXP-001: Rayon → std::thread::scope for cut (PR #208) — FAILED
- **Idea**: Replace Rayon thread pool with std::thread::scope to eliminate 0.5ms pool init
- **Implementation**: Converted all 10+ parallel paths in cut from par_iter to thread::scope
- **Result**: cut -d,-f1 regressed 0.0031s → 0.0044s (+42%)
- **Conclusion**: Thread creation/destruction per call > Rayon's one-time pool init. REVERT.

### EXP-002: MADV_POPULATE_WRITE for base64 output (PR #208) — FAILED
- **Idea**: Pre-fault output buffer pages before parallel threads write to them
- **Implementation**: Added madvise(MADV_POPULATE_WRITE) after mmap for output buffer
- **Result**: base64 encode regressed 0.0033s → 0.0040s (+21%)
- **Conclusion**: For 10MB files, prefaulting is slower than demand-faulting. Remove for small files.

### EXP-003: Parallel tr with chunk processing (PR #197) — FAILED
- **Idea**: Split stdin input into chunks, translate in parallel
- **Implementation**: Added parallel chunk processing for tr translate mode
- **Result**: tr a-z→A-Z went from 0.0038s to 0.0078s (+105%)
- **Conclusion**: tr on piped stdin (10MB) is too small for parallelism. Simple sequential is faster.

### EXP-004: Contiguous buffer tac + vmsplice (PR #209) — SUCCESS
- **Idea**: Build contiguous output buffer for tac, output via vmsplice
- **Implementation**: Collect reversed lines into contiguous buffer, vmsplice to pipe
- **Result**: tac reverse 10MB improved 0.0060s → 0.0047s (-22%)
- **Conclusion**: Contiguous buffer + batched output works well for tac.

### EXP-005: Doubling-memcmp skip for uniq (PR #204) — SUCCESS
- **Idea**: Skip duplicate runs by doubling block size comparison
- **Implementation**: Exponential block comparison to skip large duplicate groups
- **Result**: uniq 15.2x (up from 13.7x)
- **Conclusion**: Works well for sorted data with many duplicates.

---

## What To Try Next

### Priority 1: Revert regressions
- Revert std::thread::scope → back to Rayon for cut
- Remove MADV_POPULATE_WRITE for small files in base64
- Revert parallel tr → back to simple sequential translate

### Priority 2: Targeted optimizations
- **base64**: Focus on decode path (7.2x headline). Try larger decode chunks, fused whitespace strip+decode.
- **cut**: After revert, optimize the sequential path: larger output buffer, writev batching
- **tac**: Push vmsplice harder, try direct iovec from mmap (no contiguous copy)
- **tr**: After revert, try SIMD vectorization of the lookup table (SSE2 pshufb)

### Priority 3: What NOT to try
- Do NOT parallelize anything for <50MB without benchmarking first
- Do NOT use std::thread::scope instead of Rayon
- Do NOT add MADV_POPULATE_WRITE for small-file paths
