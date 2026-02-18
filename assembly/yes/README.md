# fyes — Assembly Implementation of `yes`

A drop-in replacement for GNU coreutils `yes` written in pure x86_64 Linux assembly.
Produces a static ELF binary under 1,300 bytes with zero runtime dependencies.

## Performance

Benchmarked on Linux x86_64 (Debian), writing to a pipe (pipe-limited throughput):

| Binary         | Size          | Throughput  | Memory (RSS) | Startup  | vs GNU   |
|----------------|---------------|-------------|--------------|----------|----------|
| fyes (asm)     | 1,701 bytes   | 2,060 MB/s  | 28 KB        | 0.24 ms  | **1.00×**|
| GNU yes (C)    | 43,432 bytes  | 2,189 MB/s  | 1,956 KB     | 0.75 ms  | baseline |
| fyes (Rust)    | ~435 KB       | ~2,190 MB/s | ~2,000 KB    | ~0.75 ms | ~1.00×   |

At pipe-limited throughput all three binaries write at essentially the same rate (~2.1 GB/s).
The assembly wins on **binary size** (25× smaller), **memory** (70× less RSS), and **startup** (3× faster).

## Build

Requires `nasm` and `python3`. The build script auto-detects your system's GNU yes
output (help text, version text, error message format) and patches it into the binary
so `--help` and `--version` are byte-identical to the system's `yes`.

```bash
# Auto-detect and build (recommended)
python3 build.py

# Custom output name
python3 build.py -o /usr/local/bin/fyes

# Detect only (show what would be embedded, don't build)
python3 build.py --detect
```

Manual build (uses default help/version text already in the asm):

```bash
nasm -f bin fyes.asm -o fyes
chmod +x fyes
```

## Security

All security audit findings have been addressed:

- **ARGBUF bounds checking**: argument buffer overflow prevented — stops copying 2 bytes before end, leaving room for the trailing `\n`
- **fill_loop clamped to BUFSZ exactly**: no overshoot into adjacent buffer space
- **PT_GNU_STACK header**: marks stack non-executable (NX bit)
- **EINTR retry in write loop**: handles interrupted syscalls without data loss
- **No dynamic linking**: immune to `LD_PRELOAD` attacks — no interpreter, no `libc`
- **Minimal syscall surface**: only `write(2)` and `exit(2)` — no `open`, `mmap`, `brk`, or `socket`
- **No RWX segments**: W⊕X policy enforced in ELF program headers

## GNU Compatibility

Behavior is byte-identical to GNU coreutils `yes`:

- Default output: `y\n` repeated forever
- Multiple arguments: joined with spaces, `\n`-terminated, repeated
- `--help` / `--version`: detected from system `yes` and embedded verbatim
- `--` end-of-options: first `--` stripped, subsequent `--` included in output
- Unrecognized long options (`--foo`): error to stderr, exit 1
- Invalid short options (`-x`): error to stderr, exit 1
- Bare `-` is a literal string, not an option
- SIGPIPE / EPIPE: clean exit 0

## Platform Support

### x86_64 (fyes.asm)
The x86_64 implementation uses NASM flat binary format (`nasm -f bin`)
with fixed virtual addresses (`org 0x400000`). It produces a ~1,700 byte static ELF
with no dependencies.

### ARM64 / AArch64 (fyes_arm64.s)
The ARM64 implementation uses GNU assembler (GAS) format. Build with:

```bash
as -o fyes_arm64.o fyes_arm64.s
ld -static -s -e _start -o fyes_arm64 fyes_arm64.o
```

It produces a small static ELF binary using only two syscalls (`write` and `exit_group`).

### Other platforms
macOS, Windows, and other architectures use the Rust implementation (`src/bin/fyes.rs`),
which achieves ~1.51x the throughput of GNU yes with full cross-platform support.

## Testing

```bash
# Run the full test and benchmark suite (requires ./fyes to be built first)
python3 build.py               # build first
python3 ../../tests/assembly/fyes_vs_yes_tests.py

# Tests only (skip benchmarks)
python3 ../../tests/assembly/fyes_vs_yes_tests.py --test-only

# Benchmarks only
python3 ../../tests/assembly/fyes_vs_yes_tests.py --bench-only
```

## Architecture

The binary uses a fixed two-segment layout:
- `0x400000`: code + read-only data (ELF + program headers + text + help/version/error strings)
- `0x500000`: runtime buffers (16 KB write buffer + 2 MB argument assembly buffer)

See the extensive comments in `fyes.asm` for a full walkthrough of every design decision.
