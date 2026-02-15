# fcoreutils: Claude Code Autonomously Rewrites GNU coreutils in Rust - Up to 54x Faster

## Built with Opus 4.6 - Claude Code Hackathon Submission by AiBrush

---

## Why GNU coreutils?

At AiBrush, we build AI-powered video creation technology. We have a long list of Python and system-level libraries we rely on daily that could benefit from Rust rewrites - things like Pillow/PIL for image processing, difflib for text comparison, and many others.

But when we were accepted into the "Built with Opus 4.6" hackathon, we asked ourselves a different question: what if we used Claude Code to build something that benefits not just us, but every developer on the planet?

GNU coreutils is the answer. It's the most fundamental set of command-line tools in computing. Every `wc`, `sort`, `cut`, `tr`, `uniq`, `base64`, `md5sum`, `sha256sum`, `b2sum`, and `tac` invocation on Linux runs through GNU coreutils. These tools are executed billions of times daily across millions of servers, developer machines, CI/CD pipelines, and containers worldwide. Even a modest performance improvement here has a massive multiplier effect.

So we gave Claude Code a simple goal: rewrite GNU coreutils in Rust, achieve 100% compatibility, and target 10x-30x performance improvement. Then we stepped back and let it work.

---

## The Experiment: Fully Autonomous Development

This was not a human-guided coding project with AI assistance. This was Claude Code working fully autonomously, with zero human intervention on the code itself.

Here's exactly how it worked:

1. We created an empty GitHub repository: [AiBrush/coreutils-rs](https://github.com/AiBrush/coreutils-rs)
2. We set up a small, low-end dedicated Linux machine
3. We gave Claude Code full access to the machine, including sudo
4. We defined the target: 100% GNU compatibility and 10x-30x performance improvement
5. We walked away

Claude Code consumed the full **$500 USD in API credits** from the hackathon during the first three days alone, running around the clock. After the credits ran out, it continued working for an additional full week using a Claude Max 20 subscription.

During this time, Claude Code:

- Designed the entire project architecture from scratch
- Implemented 10 high-performance command-line tools
- Built a SIMD-accelerated processing pipeline (AVX2, SSE2, NEON)
- Created zero-copy memory-mapped file I/O
- Implemented parallel processing with thread pools
- Wrote comprehensive test suites
- Set up CI/CD with GitHub Actions
- Published 62+ releases to crates.io
- Maintained byte-identical output compatibility with GNU coreutils
- Iterated through performance optimizations across dozens of versions

All of this happened without a single line of human-written code. We simply set the target and let Claude Code run.

---

## The Results

### Performance (independent benchmarks v0.4.4, Linux x86_64, hyperfine)

| Tool | Speedup vs GNU | Speedup vs uutils/coreutils |
|------|---------------:|----------------------------:|
| wc   |     **54.3x**  |                    **28.6x** |
| sort |     **18.1x**  |                    **15.1x** |
| uniq |     **13.4x**  |                     **4.0x** |
| base64 |    **8.8x**  |                     **8.4x** |
| cut  |      **6.9x**  |                     **3.7x** |
| tr   |      **6.8x**  |                     **6.7x** |
| tac  |      **5.5x**  |                     **2.8x** |
| md5sum |    **1.4x**  |                     **1.3x** |
| b2sum |     **1.3x**  |                     **1.3x** |
| sha256sum | **1.2x**  |                     **4.8x** |

### Compatibility: 826/826 tests passed (100%)

All benchmarks are independently verified in a separate repository: [AiBrush/coreutils-rs-independent-test](https://github.com/AiBrush/coreutils-rs-independent-test), which runs automated GitHub Actions on every release, comparing fcoreutils against both GNU coreutils and uutils/coreutils.

Note: Claude Code is still running on this project. By the time you check the repositories, the numbers may have improved further.

---

## The uutils Surprise

Four days into the project, we had a thought: there's already a well-known Rust rewrite of coreutils called [uutils/coreutils](https://github.com/uutils/coreutils). It has 18,000+ GitHub stars and hundreds of contributors. We were worried. What if uutils already matched or exceeded what Claude Code had built? Would all this autonomous work be meaningless?

We added uutils to the independent benchmark suite and ran the numbers.

The results were stunning. Claude Code's autonomous output didn't just match uutils - it surpassed it across the board, from 28.6x faster on `wc` down to 1.3x faster on the hash tools. This is a single AI agent outperforming a major open-source project maintained by hundreds of human contributors.

That's not a knock on uutils - it's an excellent project with broader goals. But it powerfully demonstrates what Claude Code can achieve when given a focused objective and full autonomy.

---

## How Claude Code Did It: Key Optimizations

Claude Code didn't just translate C to Rust. It redesigned each tool from the ground up with modern hardware in mind:

- **SIMD Acceleration**: Automatic detection and use of AVX2/SSE2/NEON instruction sets for byte-level scanning. The `wc` tool processes text using single-pass SIMD parallel counting, achieving the 54.3x speedup.
- **Zero-Copy Memory Mapping**: Large files are memory-mapped directly via mmap, eliminating unnecessary data copies between kernel and user space.
- **Parallel Processing**: Multi-file operations use thread pools to saturate all available CPU cores.
- **stat()-Only Byte Counting**: `wc -c` returns file size via stat() without reading any file content - instant results regardless of file size.
- **Hardware-Accelerated Hashing**: SHA-NI detection for sha256sum, optimized BLAKE2 implementations for b2sum.
- **SIMD Range Translation**: `tr` detects contiguous byte ranges and processes them with vectorized SIMD operations.
- **Chunk-Based Reverse Scanning**: `tac` processes files backward in 512KB chunks with forward SIMD scanning within each chunk, using zero-copy writev.
- **Aggressive Release Optimization**: Fat LTO, single codegen unit, abort-on-panic, stripped binaries.

---

## The Performance Journey

The chart from the independent test repository tells the story visually. Over 62+ releases, you can see Claude Code systematically improving performance while maintaining compatibility:

- Early versions (v0.0.x): Getting compatibility right, establishing the baseline
- Mid versions (v0.1.x-v0.3.x): Major performance optimizations, SIMD implementation
- Recent versions (v0.4.x): Fine-tuning, reaching 54.3x on wc while maintaining 100% compatibility

The green compatibility line stays at or near 100% throughout, while the blue performance line steadily drops (faster execution time). This shows Claude Code's disciplined approach: never sacrifice correctness for speed.

---

## Beyond coreutils: The AiBrush Rust Rewrite Portfolio

fcoreutils is not our only autonomous Claude Code project. At AiBrush, we use Claude Code to improve the performance and reliability of our underlying technology. While not all the tools we build are open-sourced, we have released four projects that demonstrate this approach:

| Project | Replaces | Best Speedup | PyPI/crates.io |
|---------|----------|-------------:|:--------------:|
| [coreutils-rs](https://github.com/AiBrush/coreutils-rs) | GNU coreutils | **54.3x** | crates.io |
| [mutagen-rs](https://github.com/AiBrush/mutagen-rs) | Python mutagen | **340x** | Both |
| [pyparsing-rs](https://github.com/AiBrush/pyparsing-rs) | Python pyparsing | **15,966x** | PyPI |
| [pyval](https://github.com/AiBrush/pyval) | python-email-validator | **450x** | PyPI |

All four were built using the same methodology: define the target, give Claude Code full access, and let it work autonomously.

---

## What This Means for Claude Code

This project is a proof point for something we believe is the future of software development: AI agents that can autonomously produce production-quality, high-performance software.

Claude Code didn't just write code that "works." It wrote code that:

- Outperforms GNU coreutils (maintained for 30+ years) by up to 54x
- Outperforms uutils/coreutils (18K+ stars, hundreds of contributors) by up to 28x
- Passes 826 out of 826 compatibility tests
- Is published on crates.io and installable via `cargo install fcoreutils`
- Has proper CI/CD, documentation, architecture docs, security policy, and contribution guidelines
- Went through 62+ iterative releases, each improving on the last

The total development effort from the human side was minimal: create a repo, set a goal, provide a machine. Claude Code did everything else.

---

## Try It

```bash
cargo install fcoreutils
```

Each tool is prefixed with `f` to avoid conflicts with system utilities:

```bash
fwc file.txt          # 54x faster than wc
fsort large.txt       # 18x faster than sort
funiq data.txt        # 13x faster than uniq
fbase64 file.bin      # 9x faster than base64
fcut -d, -f1 data.csv # 7x faster than cut
ftr 'a-z' 'A-Z' < f  # 7x faster than tr
ftac file.txt         # 5x faster than tac
```

---

## Links

- Main repository: [github.com/AiBrush/coreutils-rs](https://github.com/AiBrush/coreutils-rs)
- Independent benchmarks: [github.com/AiBrush/coreutils-rs-independent-test](https://github.com/AiBrush/coreutils-rs-independent-test)
- crates.io: [crates.io/crates/fcoreutils](https://crates.io/crates/fcoreutils)
- Team: AiBrush (Tarek Abdellatef, @tarekbadrsh)

---

*Built autonomously by Claude Code during the "Built with Opus 4.6" hackathon, February 10-16, 2026. Claude Code consumed $500 in API credits over three days, then continued on Claude Max 20 subscription for one additional week. Zero human-written code.*
