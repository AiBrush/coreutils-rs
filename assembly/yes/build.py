#!/usr/bin/env python3
"""
build.py — Build fyes for the current platform (or a specified target).

Detects your system's `yes` output (help text, version text, error messages),
patches the assembly DATA section, assembles, links, and verifies.

Supports four targets:
  linux-x86_64   — NASM flat binary (no linker needed)
  linux-arm64    — GNU as + ld, for aarch64-linux-gnu
  macos-x86_64   — NASM macho64 + Apple ld, for macOS Intel
  macos-arm64    — Apple as + Apple ld, for macOS Apple Silicon

Usage:
    python3 build.py                              # auto-detect target
    python3 build.py --target linux-x86_64        # explicit target
    python3 build.py --target linux-arm64
    python3 build.py --target macos-x86_64
    python3 build.py --target macos-arm64
    python3 build.py -o myyes                     # custom output name
    python3 build.py --detect                     # show detected data, no build
    python3 build.py --no-verify                  # skip verification step
"""

import subprocess
import sys
import os
import shutil
import argparse
import platform
import tempfile

# File markers — everything between these is replaced with detected data.
MARKER_NASM  = ("; @@DATA_START@@", "; @@DATA_END@@")   # NASM .asm files
MARKER_GAS   = ("// @@DATA_START@@", "// @@DATA_END@@") # GNU as .s files

# Assembly source files relative to this script's directory
ASM_FILES = {
    "linux-x86_64":  "fyes.asm",
    "linux-arm64":   "fyes_arm64.s",
    "macos-x86_64":  "fyes_macos_x86_64.asm",
    "macos-arm64":   "fyes_macos_arm64.s",
}


# =============================================================================
# Subprocess helpers
# =============================================================================

def capture(args):
    """Run a command; return (stdout, stderr, returncode)."""
    try:
        p = subprocess.run(args, capture_output=True, timeout=10)
        return p.stdout, p.stderr, p.returncode
    except FileNotFoundError:
        return b"", ("{}: not found".format(args[0])).encode(), 127
    except subprocess.TimeoutExpired:
        return b"", ("{}: timed out".format(args[0])).encode(), 124


def run(args, check=True):
    """Run a command (capture output)."""
    result = subprocess.run(args, capture_output=True)
    if check and result.returncode != 0:
        print("Error: {} failed:".format(args[0]), file=sys.stderr)
        if result.stderr:
            print(result.stderr.decode(errors="replace"), file=sys.stderr)
        sys.exit(1)
    return result


# =============================================================================
# Platform / target detection
# =============================================================================

def detect_target():
    """Auto-detect the build target from the current OS and CPU."""
    os_name = platform.system()   # 'Linux', 'Darwin', 'Windows'
    machine = platform.machine()  # 'x86_64', 'aarch64', 'arm64', 'AMD64'

    if os_name == "Linux":
        if machine in ("x86_64", "AMD64"):
            return "linux-x86_64"
        elif machine in ("aarch64", "arm64"):
            return "linux-arm64"
        else:
            print("Warning: unknown Linux arch '{}', assuming x86_64".format(machine),
                  file=sys.stderr)
            return "linux-x86_64"
    elif os_name == "Darwin":
        if machine in ("arm64", "aarch64"):
            return "macos-arm64"
        elif machine == "x86_64":
            return "macos-x86_64"
        else:
            print("Warning: unknown macOS arch '{}', assuming arm64".format(machine),
                  file=sys.stderr)
            return "macos-arm64"
    else:
        print("Warning: unsupported OS '{}', assuming linux-x86_64".format(os_name),
              file=sys.stderr)
        return "linux-x86_64"


# =============================================================================
# Data detection — capture system yes output
# =============================================================================

def find_yes_binary(target):
    """Find the best `yes` binary for detecting text to embed."""
    # On macOS, system yes is BSD (no --help/--version).
    # Try Homebrew GNU coreutils first.
    if target.startswith("macos"):
        # Prefer gnubin paths (basename='yes') over gyes (basename='gyes')
        # so that captured error messages use 'yes:' not 'gyes:'.
        for candidate in ["/opt/homebrew/opt/coreutils/libexec/gnubin/yes",
                          "/usr/local/opt/coreutils/libexec/gnubin/yes",
                          "/opt/homebrew/bin/gyes", "/usr/local/bin/gyes"]:
            out, _, rc = capture([candidate, "--help"])
            if rc == 0 and b"STRING" in out:
                return candidate
        # Also check if plain 'yes' on PATH is GNU yes
        yo, _, yr = capture(["yes", "--help"])
        if yr == 0 and b"STRING" in yo:
            return "yes"
        return None  # No GNU yes on macOS — use defaults

    # On Linux, system yes is GNU yes.
    # Return just "yes" (bare name) so argv[0]=="yes" in error/help text.
    if shutil.which("yes"):
        return "yes"
    return None


def detect_system_yes(target):
    """
    Capture all output from the system's `yes` binary.
    Returns a dict with keys: help, version, err_unrec, err_inval, err_suffix.
    Returns None if detection fails (e.g. GNU yes not available on macOS).

    On Linux, yes_bin is bare "yes" so program_name == "yes".
    On macOS, gnubin paths (basename="yes") are preferred over gyes paths.
    """
    yes_bin = find_yes_binary(target)
    if yes_bin is None:
        print("  [info] GNU yes not found; using built-in defaults", file=sys.stderr)
        return None

    # Use yes_bin directly — subprocess.run with a list passes the full path
    # as argv[0], which is fine since GNU yes derives its program name from it.
    yes_cmd = yes_bin

    help_out, _, _ = capture([yes_cmd, "--help"])
    ver_out, _, _  = capture([yes_cmd, "--version"])

    LONG_PROBE  = "--bogus_test_option_xyz"
    SHORT_PROBE = "Z"
    _, err_long, _  = capture([yes_cmd, LONG_PROBE])
    _, err_short, _ = capture([yes_cmd, "-{}".format(SHORT_PROBE)])

    long_lines  = err_long.split(b"\n")
    short_lines = err_short.split(b"\n")

    line1_long  = long_lines[0]
    line1_short = short_lines[0]
    try_line    = long_lines[1] if len(long_lines) > 1 else b""

    opt_pos = line1_long.find(LONG_PROBE.encode())
    if opt_pos < 0:
        print("  [warn] probe not found in long error output; using defaults",
              file=sys.stderr)
        return None

    err_unrec   = line1_long[:opt_pos]
    close_quote = line1_long[opt_pos + len(LONG_PROBE):]

    short_pos = line1_short.find(SHORT_PROBE.encode())
    if short_pos < 0:
        print("  [warn] probe not found in short error output; using defaults",
              file=sys.stderr)
        return None

    err_inval  = line1_short[:short_pos]
    err_suffix = close_quote + b"\n" + try_line + b"\n"

    return {
        "help":       help_out,
        "version":    ver_out,
        "err_unrec":  err_unrec,
        "err_inval":  err_inval,
        "err_suffix": err_suffix,
    }


def print_detection(data):
    """Print detected data summary."""
    if data is None:
        print("  (using built-in defaults — no GNU yes detected)")
        return

    def qs(b):
        if b"\xe2\x80\x98" in b: return "UTF-8 curly quotes"
        if b"\x27" in b:          return "ASCII apostrophe"
        return "unknown"

    print("  --help:      {:>5} bytes  quotes: {}".format(len(data["help"]), qs(data["help"])))
    print("  --version:   {:>5} bytes".format(len(data["version"])))
    print("  err_unrec:   {:>5} bytes  {!r}".format(len(data["err_unrec"]), data["err_unrec"]))
    print("  err_inval:   {:>5} bytes  {!r}".format(len(data["err_inval"]), data["err_inval"]))
    print("  err_suffix:  {:>5} bytes  quotes: {}".format(
        len(data["err_suffix"]), qs(data["err_suffix"])))


# =============================================================================
# Data section generation
# =============================================================================

def bytes_to_nasm(data, label):
    """Convert bytes to NASM `db` directives."""
    lines = []
    for i in range(0, len(data), 16):
        chunk = data[i:i + 16]
        hexb  = ", ".join("0x{:02x}".format(b) for b in chunk)
        if i == 0:
            lines.append("{:<16}db {}".format(label, hexb))
        else:
            lines.append("                db {}".format(hexb))
    return "\n".join(lines)


def bytes_to_gas(data, label):
    """Convert bytes to GNU as `.byte` directives."""
    lines = ["{}:".format(label)]
    for i in range(0, len(data), 16):
        chunk = data[i:i + 16]
        hexb  = ", ".join("0x{:02x}".format(b) for b in chunk)
        lines.append("    .byte {}".format(hexb))
    return "\n".join(lines)


def generate_data_nasm(data):
    """Generate NASM data section content (linux-x86_64 flat binary)."""
    parts = []
    # linux-x86_64 flat binary doesn't need a section directive here
    # (the surrounding code has already set up the data section)
    parts.append(bytes_to_nasm(data["help"],       "help_text:"))
    parts.append("help_text_len equ $ - help_text\n")
    parts.append(bytes_to_nasm(data["version"],    "version_text:"))
    parts.append("version_text_len equ $ - version_text\n")
    parts.append(bytes_to_nasm(data["err_unrec"],  "err_unrec:"))
    parts.append("err_unrec_len equ $ - err_unrec\n")
    parts.append(bytes_to_nasm(data["err_inval"],  "err_inval:"))
    parts.append("err_inval_len equ $ - err_inval\n")
    parts.append(bytes_to_nasm(data["err_suffix"], "err_suffix:"))
    parts.append("err_suffix_len equ $ - err_suffix")
    return "\n".join(parts)


def generate_data_gas_linux(data):
    """Generate GNU as data section for Linux ARM64 (.rodata section)."""
    parts = ["    .section .rodata\n"]
    parts.append(bytes_to_gas(data["help"],       "help_text"))
    parts.append("    .set help_text_len, . - help_text\n")
    parts.append(bytes_to_gas(data["version"],    "version_text"))
    parts.append("    .set version_text_len, . - version_text\n")
    parts.append(bytes_to_gas(data["err_unrec"],  "err_unrec_pre"))
    parts.append("    .set err_unrec_pre_len, . - err_unrec_pre\n")
    parts.append(bytes_to_gas(data["err_inval"],  "err_inval_pre"))
    parts.append("    .set err_inval_pre_len, . - err_inval_pre\n")
    parts.append(bytes_to_gas(data["err_suffix"], "err_suffix"))
    parts.append("    .set err_suffix_len, . - err_suffix")
    return "\n".join(parts)


def generate_data_nasm_macos(data):
    """Generate NASM data section for macOS x86_64 (macho64, rodata→__const)."""
    parts = ["section .rodata\n"]
    parts.append(bytes_to_nasm(data["help"],       "help_text:"))
    parts.append("help_text_len equ $ - help_text\n")
    parts.append(bytes_to_nasm(data["version"],    "version_text:"))
    parts.append("version_text_len equ $ - version_text\n")
    parts.append(bytes_to_nasm(data["err_unrec"],  "err_unrec:"))
    parts.append("err_unrec_len equ $ - err_unrec\n")
    parts.append(bytes_to_nasm(data["err_inval"],  "err_inval:"))
    parts.append("err_inval_len equ $ - err_inval\n")
    parts.append(bytes_to_nasm(data["err_suffix"], "err_suffix:"))
    parts.append("err_suffix_len equ $ - err_suffix")
    return "\n".join(parts)


def generate_data_gas_macos(data):
    """Generate GNU as data section for macOS ARM64 (__TEXT,__const section)."""
    parts = ["    .section __TEXT,__const\n"]
    parts.append(bytes_to_gas(data["help"],       "help_text"))
    parts.append("    .set help_text_len, . - help_text\n")
    parts.append(bytes_to_gas(data["version"],    "version_text"))
    parts.append("    .set version_text_len, . - version_text\n")
    parts.append(bytes_to_gas(data["err_unrec"],  "err_unrec_pre"))
    parts.append("    .set err_unrec_pre_len, . - err_unrec_pre\n")
    parts.append(bytes_to_gas(data["err_inval"],  "err_inval_pre"))
    parts.append("    .set err_inval_pre_len, . - err_inval_pre\n")
    parts.append(bytes_to_gas(data["err_suffix"], "err_suffix"))
    parts.append("    .set err_suffix_len, . - err_suffix")
    return "\n".join(parts)


# =============================================================================
# Patching — replace data between markers in a COPY of the source file
# =============================================================================

def patch_asm(src_path, new_data, markers):
    """
    Replace content between markers in a COPY of src_path.
    The original file is NEVER modified.
    Returns path to a temporary file (caller must delete it).
    """
    with open(src_path, "r") as f:
        content = f.read()

    start_marker, end_marker = markers
    start_idx = content.find(start_marker)
    end_idx   = content.find(end_marker)

    if start_idx < 0 or end_idx < 0:
        print("  [warn] markers not found in {}; using file as-is".format(src_path),
              file=sys.stderr)
        ext = os.path.splitext(src_path)[1]
        fd, tmp = tempfile.mkstemp(suffix=ext)
        os.close(fd)
        shutil.copy2(src_path, tmp)
        return tmp

    before  = content[:start_idx + len(start_marker)]
    after   = content[end_idx:]
    patched = "{}\n{}\n{}".format(before, new_data, after)

    ext = os.path.splitext(src_path)[1]
    fd, tmp = tempfile.mkstemp(suffix=ext)
    os.close(fd)
    with open(tmp, "w") as f:
        f.write(patched)
    return tmp


# =============================================================================
# Target-specific build functions
# =============================================================================

def get_sdk_path():
    """Get the macOS SDK path via xcrun."""
    out, _, rc = capture(["xcrun", "--show-sdk-path"])
    if rc != 0 or not out.strip():
        return "/"
    return out.strip().decode()


def build_linux_x86_64(asm_path, output):
    """Assemble Linux x86_64 flat binary with NASM (no linker needed)."""
    if not shutil.which("nasm"):
        print("Error: nasm not found. Install: apt-get install nasm", file=sys.stderr)
        sys.exit(1)
    run(["nasm", "-f", "bin", asm_path, "-o", output])
    os.chmod(output, 0o755)
    size = os.path.getsize(output)
    print("  Built {} ({} bytes, flat ELF)".format(output, size))


def build_linux_arm64(asm_path, output):
    """Assemble Linux ARM64 ELF with cross-assembler + cross-linker."""
    # Prefer cross-assembler; fall back to native `as` (on real ARM64 host)
    asbin = None
    for candidate in ("aarch64-linux-gnu-as", "as"):
        if shutil.which(candidate):
            asbin = candidate
            break
    if asbin is None:
        print("Error: no AArch64 assembler found.", file=sys.stderr)
        print("  Install: apt-get install binutils-aarch64-linux-gnu", file=sys.stderr)
        sys.exit(1)

    ldbin = None
    for candidate in ("aarch64-linux-gnu-ld", "ld"):
        if shutil.which(candidate):
            ldbin = candidate
            break
    if ldbin is None:
        print("Error: no AArch64 linker found.", file=sys.stderr)
        sys.exit(1)

    fd, obj = tempfile.mkstemp(suffix=".o")
    os.close(fd)
    try:
        run([asbin, "-o", obj, asm_path])
        run([ldbin, "-static", "-s", "-e", "_start", "-o", output, obj])
        os.chmod(output, 0o755)
        size = os.path.getsize(output)
        print("  Built {} ({} bytes, static ELF ARM64)".format(output, size))
    finally:
        if os.path.exists(obj):
            os.unlink(obj)


def build_macos_x86_64(asm_path, output):
    """Assemble macOS x86_64 Mach-O with NASM + Apple ld."""
    if not shutil.which("nasm"):
        print("Error: nasm not found. Install: brew install nasm", file=sys.stderr)
        sys.exit(1)

    fd, obj = tempfile.mkstemp(suffix=".o")
    os.close(fd)
    try:
        run(["nasm", "-f", "macho64", asm_path, "-o", obj])
        sdk = get_sdk_path()
        run([
            "ld", "-arch", "x86_64",
            "-o", output, obj,
            "-lSystem",
            "-syslibroot", sdk,
            "-e", "_start",
            "-macosx_version_min", "10.14",
        ])
        os.chmod(output, 0o755)
        size = os.path.getsize(output)
        print("  Built {} ({} bytes, Mach-O x86_64)".format(output, size))
    finally:
        if os.path.exists(obj):
            os.unlink(obj)


def build_macos_arm64(asm_path, output):
    """Assemble macOS ARM64 Mach-O with Apple as + Apple ld."""
    if not shutil.which("as"):
        print("Error: 'as' not found. Install Xcode command line tools.", file=sys.stderr)
        sys.exit(1)

    fd, obj = tempfile.mkstemp(suffix=".o")
    os.close(fd)
    try:
        run(["as", "-arch", "arm64", "-o", obj, asm_path])
        sdk = get_sdk_path()
        run([
            "ld", "-arch", "arm64",
            "-o", output, obj,
            "-lSystem",
            "-syslibroot", sdk,
            "-e", "_start",
            "-macosx_version_min", "11.0",
        ])
        os.chmod(output, 0o755)
        size = os.path.getsize(output)
        print("  Built {} ({} bytes, Mach-O ARM64)".format(output, size))
    finally:
        if os.path.exists(obj):
            os.unlink(obj)


# =============================================================================
# Verification
# =============================================================================

def verify(binary, target):
    """Quick verification of the built fyes binary."""
    binary_path = binary if os.path.isabs(binary) else "./{}".format(binary)
    is_macos = target.startswith("macos")
    passed = failed = 0

    def test(label, ok):
        nonlocal passed, failed
        tag = "PASS" if ok else "FAIL"
        print("  [{}] {}".format(tag, label))
        if ok: passed += 1
        else:  failed += 1

    # Basic: default output
    fo, _, _ = capture(["sh", "-c", "{} | head -n 5".format(binary_path)])
    test("default output (y\\n x5)", fo == b"y\ny\ny\ny\ny\n")

    # Custom string
    fo, _, _ = capture(["sh", "-c", "{} hello | head -n 3".format(binary_path)])
    test("custom string 'hello'", fo == b"hello\nhello\nhello\n")

    # Multiple args
    fo, _, _ = capture(["sh", "-c", "{} a b | head -n 3".format(binary_path)])
    test("multiple args 'a b'", fo == b"a b\na b\na b\n")

    # EPIPE handling
    fo, _, fr = capture(["sh", "-c", "{} | head -n 1".format(binary_path)])
    test("EPIPE (yes | head -n 1) → exit 0", fo == b"y\n" and fr == 0)

    # "--" separator
    fo, _, _ = capture(["sh", "-c", "{} -- foo | head -n 2".format(binary_path)])
    test("'-- foo' outputs 'foo' forever", fo == b"foo\nfoo\n")

    # "--" passed through
    fo, _, _ = capture(["sh", "-c", "{} -- -- | head -n 2".format(binary_path)])
    test("'-- --' outputs '--' forever", fo == b"--\n--\n")

    # Error handling
    _, _, fr = capture([binary_path, "--bad-option"])
    test("--bad-option exits 1", fr == 1)

    _, _, fr = capture([binary_path, "-x"])
    test("-x exits 1", fr == 1)

    _, _, fr = capture([binary_path, "--help"])
    test("--help exits 0", fr == 0)

    _, _, fr = capture([binary_path, "--version"])
    test("--version exits 0", fr == 0)

    if not is_macos:
        # On Linux, do full byte-identical comparison with GNU yes
        fo, _, _ = capture(["sh", "-c", "yes | head -n 5"])
        fy, _, _ = capture(["sh", "-c", "{} | head -n 5".format(binary_path)])
        test("byte-identical default output vs GNU yes", fo == fy)

        fo, _, _ = capture(["yes", "--help"])
        fy, _, _ = capture([binary_path, "--help"])
        test("--help byte-identical to GNU yes", fo == fy)

        fo, _, _ = capture(["yes", "--version"])
        fy, _, _ = capture([binary_path, "--version"])
        test("--version byte-identical to GNU yes", fo == fy)

        # Error message comparison (stderr)
        _, fe, fr = capture(["yes", "--bad-test-option"])
        _, fy_e, fy_r = capture([binary_path, "--bad-test-option"])
        test("bad long opt stderr byte-identical", fe == fy_e and fr == fy_r)

        _, fe, fr = capture(["yes", "-z"])
        _, fy_e, fy_r = capture([binary_path, "-z"])
        test("bad short opt stderr byte-identical", fe == fy_e and fr == fy_r)

    print("  {}/{} passed".format(passed, passed + failed))


# =============================================================================
# Main
# =============================================================================

GENERATORS = {
    "linux-x86_64": generate_data_nasm,
    "linux-arm64":  generate_data_gas_linux,
    "macos-x86_64": generate_data_nasm_macos,
    "macos-arm64":  generate_data_gas_macos,
}

BUILDERS = {
    "linux-x86_64": build_linux_x86_64,
    "linux-arm64":  build_linux_arm64,
    "macos-x86_64": build_macos_x86_64,
    "macos-arm64":  build_macos_arm64,
}

MARKERS = {
    "linux-x86_64": MARKER_NASM,
    "linux-arm64":  MARKER_GAS,
    "macos-x86_64": MARKER_NASM,
    "macos-arm64":  MARKER_GAS,
}


def main():
    parser = argparse.ArgumentParser(
        description="Build fyes matched to system GNU yes"
    )
    parser.add_argument(
        "--target", "-t",
        choices=["linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64"],
        help="Build target (default: auto-detect from current platform)",
    )
    parser.add_argument(
        "-o", "--output",
        default="fyes",
        help="Output binary name (default: fyes)",
    )
    parser.add_argument(
        "--detect",
        action="store_true",
        help="Just detect system yes data, don't build",
    )
    parser.add_argument(
        "--no-verify",
        action="store_true",
        help="Skip verification after build",
    )
    args = parser.parse_args()

    # Change to script's directory so relative paths work
    script_dir = os.path.dirname(os.path.abspath(__file__))
    os.chdir(script_dir)

    target = args.target or detect_target()
    print("[*] Target: {}".format(target))

    print("[*] Detecting system yes...")
    data = detect_system_yes(target)
    print_detection(data)

    if args.detect:
        return

    asm_file = ASM_FILES[target]
    if not os.path.exists(asm_file):
        print("Error: {} not found in {}".format(asm_file, script_dir),
              file=sys.stderr)
        sys.exit(1)

    tmp_asm = None
    try:
        if data is not None:
            print("[*] Patching assembly data section...")
            new_data = GENERATORS[target](data)
            tmp_asm  = patch_asm(asm_file, new_data, MARKERS[target])
            print("  Patched {} → temp file".format(asm_file))
        else:
            print("[*] Using built-in defaults (no patching)")
            tmp_asm = None  # Use original directly

        asm_to_build = tmp_asm if tmp_asm else asm_file

        print("[*] Assembling...")
        BUILDERS[target](asm_to_build, args.output)

        if not args.no_verify:
            print("[*] Verifying...")
            verify(args.output, target)

    finally:
        if tmp_asm and os.path.exists(tmp_asm):
            os.unlink(tmp_asm)


if __name__ == "__main__":
    main()
