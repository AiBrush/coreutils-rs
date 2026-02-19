# fyes — Assembly Implementation of `yes`

A drop-in replacement for GNU coreutils `yes` in pure assembly.
Supported on Linux x86_64, Linux ARM64, macOS x86_64, and macOS ARM64.

## Performance

Benchmarked on Linux x86_64 (Debian), writing to a pipe (pipe-limited throughput):

| Binary         | Size          | Throughput  | Memory (RSS) | Startup  | vs GNU   |
|----------------|---------------|-------------|--------------|----------|----------|
| fyes (asm)     | 1,701 bytes   | 2,060 MB/s  | 28 KB        | 0.24 ms  | **1.00×**|
| GNU yes (C)    | 43,432 bytes  | 2,189 MB/s  | 1,956 KB     | 0.75 ms  | baseline |
| fyes (Rust)    | ~435 KB       | ~2,190 MB/s | ~2,000 KB    | ~0.75 ms | ~1.00×   |

At pipe-limited throughput all three binaries write at essentially the same rate (~2.1 GB/s).
The assembly wins on **binary size** (25× smaller), **memory** (70× less RSS), and **startup** (3× faster).

## Quick Build

The `build.py` script auto-detects your platform, captures help/version text from your
system's GNU `yes`, patches it into the binary, and verifies byte-identical output.

```bash
# Auto-detect platform and build (recommended)
python3 build.py

# Explicit target
python3 build.py --target linux-x86_64
python3 build.py --target linux-arm64
python3 build.py --target macos-x86_64
python3 build.py --target macos-arm64

# Custom output name
python3 build.py -o /usr/local/bin/fyes

# Just detect what would be embedded (no build)
python3 build.py --detect
```

## Platform Support

### Linux x86_64 (`fyes.asm`)
Uses NASM flat binary format (`nasm -f bin`) with fixed virtual addresses (`org 0x400000`).
Produces a ~1,700 byte **static ELF** with zero runtime dependencies.

Requirements: `nasm`

Manual build:
```bash
nasm -f bin fyes.asm -o fyes && chmod +x fyes
```

### Linux ARM64 (`fyes_arm64.s`)
Uses GNU assembler (GAS) + linker. Produces a small **static ELF** binary.
Compatible with native ARM64 hosts and aarch64-linux-gnu cross-toolchain.

Requirements: `binutils-aarch64-linux-gnu` (for cross-build on x86_64)

Manual build:
```bash
aarch64-linux-gnu-as -o fyes_arm64.o fyes_arm64.s
aarch64-linux-gnu-ld -static -s -e _start -o fyes fyes_arm64.o
```

### macOS x86_64 (`fyes_macos_x86_64.asm`)
Uses NASM Mach-O format (`nasm -f macho64`) + Apple linker.
Produces a **Mach-O dynamic executable** linked against libSystem.
No libc functions are called; all output via direct BSD syscalls.

Requirements: `nasm` (`brew install nasm`), Xcode command line tools

Manual build:
```bash
nasm -f macho64 fyes_macos_x86_64.asm -o fyes_macos.o
SDK=$(xcrun --show-sdk-path)
ld -arch x86_64 -o fyes fyes_macos.o -lSystem \
   -syslibroot $SDK -e _start -macosx_version_min 10.14
```

### macOS ARM64 (`fyes_macos_arm64.s`)
Uses Apple's assembler (`as`) + Apple linker.
Produces a **Mach-O ARM64 executable** for Apple Silicon.
Uses `svc #0x80` with `x16` register for macOS BSD syscalls.

Requirements: Xcode command line tools

Manual build:
```bash
SDK=$(xcrun --show-sdk-path)
as -arch arm64 -o fyes_macos_arm64.o fyes_macos_arm64.s
ld -arch arm64 -o fyes fyes_macos_arm64.o -lSystem \
   -syslibroot $SDK -e _start -macosx_version_min 11.0
```

### Windows
Uses the Rust implementation (`src/bin/fyes.rs`).

## GNU Compatibility

All assembly implementations produce byte-identical output to GNU coreutils `yes`:

- Default output: `y\n` repeated forever
- Multiple arguments: joined with spaces, `\n`-terminated, repeated
- `--help` / `--version`: detected from system GNU `yes` and embedded verbatim
- `--` end-of-options: first `--` stripped, subsequent `--` included in output
- GNU permutation: `--help`/`--version` recognized **anywhere** in argv (e.g. `yes foo --help bar`)
- Unrecognized long options (`--foo`): error to stderr, exit 1
- Invalid short options (`-x`): error to stderr, exit 1
- Bare `-` is a literal string, not an option
- SIGPIPE / EPIPE: clean exit 0
- Partial writes: tracked and continued (no line corruption)

## Security Properties

| Property               | Linux x86_64 | Linux ARM64 | macOS x86_64 | macOS ARM64 |
|------------------------|:---:|:---:|:---:|:---:|
| Static binary          | ✅  | ✅  | —   | —   |
| No libc calls          | ✅  | ✅  | ✅  | ✅  |
| NX stack               | ✅  | ✅  | ✅  | ✅  |
| No RWX segments        | ✅  | ✅  | ✅  | ✅  |
| SIGPIPE blocked        | ✅  | ✅  | ✅  | ✅  |
| ARGBUF bounds check    | ✅  | ✅  | ✅  | ✅  |
| EINTR retry            | ✅  | ✅  | ✅  | ✅  |
| Partial write safe     | ✅  | ✅  | ✅  | ✅  |

## macOS Syscall ABI Differences

The macOS assembly implementations handle key ABI differences from Linux:

| Feature              | Linux x86_64/ARM64 | macOS x86_64 | macOS ARM64 |
|----------------------|-------------------|--------------|-------------|
| Syscall instruction  | `syscall`/`svc #0` | `syscall`   | `svc #0x80` |
| Syscall number reg   | rax / x8          | rax          | x16         |
| Error indication     | negative rax/x0   | carry flag   | carry flag  |
| SIG_BLOCK constant   | 0                 | 1            | 1           |
| sigset_t size        | 64-bit            | 32-bit       | 32-bit      |
| `write` syscall #    | 1 / 64            | 0x2000004    | 4           |
| `exit` syscall #     | 60 / 94           | 0x2000001    | 1           |

## Testing

```bash
# Build and run full test suite (Linux x86_64)
python3 build.py
python3 ../../tests/assembly/fyes_vs_yes_tests.py

# Test only (no benchmarks)
python3 ../../tests/assembly/fyes_vs_yes_tests.py --test-only

# Benchmark only
python3 ../../tests/assembly/fyes_vs_yes_tests.py --bench-only
```

## Architecture

**Linux x86_64** uses a fixed two-segment flat ELF layout:
- `0x400000`: code + read-only data (ELF headers + text + help/version/error strings)
- `0x500000`: runtime BSS buffers (16 KB write buffer + 2 MB argument buffer)

**Linux ARM64** / **macOS** use standard ELF/Mach-O layouts with linker-managed sections:
- `.text` / `__TEXT,__text`: code
- `.rodata` / `__TEXT,__const`: help/version/error strings
- `.bss` / `__DATA,__bss`: runtime buffers (zero-initialized)

All implementations use the same algorithm:
1. Block SIGPIPE at startup
2. PASS 1: scan all argv for `--help`/`--version`/bad options (GNU permutation)
3. Build output line from args into argbuf (joined with spaces + `\n`)
4. Fill write buffer with repeated complete copies of the output line
5. Write loop: write the buffer forever, tracking partial writes

See inline comments in each `.asm`/`.s` file for full implementation details.
