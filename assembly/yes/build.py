#!/usr/bin/env python3
"""
build.py — Build fyes matched to the local system's GNU yes.

Detects your system's yes --help, --version, and error message format,
patches fyes_secure.asm data section in-place, and assembles the binary.

Usage:
    python3 build.py              # build ./fyes
    python3 build.py -o myyes     # build with custom output name
    python3 build.py --detect     # just show what was detected, don't build
"""

import subprocess
import sys
import os
import shutil
import argparse

ASM_FILE = "fyes.asm"
MARKER_START = "; @@DATA_START@@"
MARKER_END = "; @@DATA_END@@"


def capture(args: list[str]) -> tuple[bytes, bytes, int]:
    """Run a command and return (stdout, stderr, returncode)."""
    p = subprocess.run(args, capture_output=True, timeout=10)
    return p.stdout, p.stderr, p.returncode


def bytes_to_nasm(data: bytes, label: str) -> str:
    """Convert raw bytes to NASM db directives with a label."""
    lines = []
    for i in range(0, len(data), 16):
        chunk = data[i:i + 16]
        hexb = ", ".join(f"0x{b:02x}" for b in chunk)
        if i == 0:
            lines.append(f'{label:<16}db {hexb}')
        else:
            lines.append(f'                db {hexb}')
    return "\n".join(lines)


def detect_system_yes() -> dict:
    """Capture all output from the system's GNU yes."""

    yes_bin = shutil.which("yes")
    if not yes_bin:
        print("Error: GNU yes not found in PATH", file=sys.stderr)
        sys.exit(1)

    # Capture --help and --version (stdout)
    help_out, _, _ = capture(["yes", "--help"])
    ver_out, _, _ = capture(["yes", "--version"])

    # Capture error messages (stderr) using known test options
    LONG_PROBE = "--bogus_test_option_xyz"
    SHORT_PROBE = "Z"
    _, err_long, _ = capture(["yes", LONG_PROBE])
    _, err_short, _ = capture(["yes", f"-{SHORT_PROBE}"])

    # Parse error structure:
    #   line 1: "yes: unrecognized option <LQ>--bogus...<RQ>"
    #   line 2: "Try <LQ>yes --help<RQ> for more information."
    long_lines = err_long.split(b"\n")
    short_lines = err_short.split(b"\n")

    line1_long = long_lines[0]
    line1_short = short_lines[0]
    try_line = long_lines[1] if len(long_lines) > 1 else b""

    # Find probe string to split prefix / suffix
    opt_pos = line1_long.find(LONG_PROBE.encode())
    if opt_pos < 0:
        print("Error: couldn't find probe option in error output", file=sys.stderr)
        print(f"  stderr was: {err_long!r}", file=sys.stderr)
        sys.exit(1)

    err_unrec = line1_long[:opt_pos]
    close_quote = line1_long[opt_pos + len(LONG_PROBE):]

    short_pos = line1_short.find(SHORT_PROBE.encode())
    err_inval = line1_short[:short_pos]

    err_suffix = close_quote + b"\n" + try_line + b"\n"

    return {
        "help": help_out,
        "version": ver_out,
        "err_unrec": err_unrec,
        "err_inval": err_inval,
        "err_suffix": err_suffix,
    }


def generate_data_section(data: dict) -> str:
    """Generate NASM data section from detected data."""
    lines = []

    lines.append(bytes_to_nasm(data["help"], "help_text:"))
    lines.append("help_text_len equ $ - help_text")
    lines.append("")

    lines.append(bytes_to_nasm(data["version"], "version_text:"))
    lines.append("version_text_len equ $ - version_text")
    lines.append("")

    lines.append(bytes_to_nasm(data["err_unrec"], "err_unrec:"))
    lines.append("err_unrec_len equ $ - err_unrec")
    lines.append("")

    lines.append(bytes_to_nasm(data["err_inval"], "err_inval:"))
    lines.append("err_inval_len equ $ - err_inval")
    lines.append("")

    lines.append(bytes_to_nasm(data["err_suffix"], "err_suffix:"))
    lines.append("err_suffix_len equ $ - err_suffix")

    return "\n".join(lines)


def patch_asm(asm_path: str, new_data: str) -> None:
    """Replace everything between @@DATA_START@@ and @@DATA_END@@ markers."""

    with open(asm_path, "r") as f:
        content = f.read()

    start_idx = content.find(MARKER_START)
    end_idx = content.find(MARKER_END)

    if start_idx < 0 or end_idx < 0:
        print(f"Error: markers not found in {asm_path}", file=sys.stderr)
        print(f"  Expected {MARKER_START!r} and {MARKER_END!r}", file=sys.stderr)
        sys.exit(1)

    # Keep the marker lines, replace content between them
    before = content[:start_idx + len(MARKER_START)]
    after = content[end_idx:]

    patched = f"{before}\n{new_data}\n{after}"

    with open(asm_path, "w") as f:
        f.write(patched)


def print_detection(data: dict) -> None:
    """Print what was detected."""

    def quote_style(b: bytes) -> str:
        if b"\xe2\x80\x98" in b:
            return "UTF-8 curly quotes"
        if b"\x27" in b:
            return "ASCII apostrophe (0x27)"
        return "unknown"

    print(f"  --help:        {len(data['help']):>4} bytes  quotes: {quote_style(data['help'])}")
    print(f"  --version:     {len(data['version']):>4} bytes")
    print(f"  err_unrec:     {len(data['err_unrec']):>4} bytes  {data['err_unrec']!r}")
    print(f"  err_inval:     {len(data['err_inval']):>4} bytes  {data['err_inval']!r}")
    print(f"  err_suffix:    {len(data['err_suffix']):>4} bytes  quotes: {quote_style(data['err_suffix'])}")


def build(output: str) -> None:
    """Assemble the binary."""

    if not shutil.which("nasm"):
        print("Error: nasm not found in PATH", file=sys.stderr)
        sys.exit(1)

    result = subprocess.run(
        ["nasm", "-f", "bin", ASM_FILE, "-o", output],
        capture_output=True,
    )
    if result.returncode != 0:
        print(f"Error: nasm failed:\n{result.stderr.decode()}", file=sys.stderr)
        sys.exit(1)

    os.chmod(output, 0o755)
    size = os.path.getsize(output)
    print(f"  Built {output} ({size} bytes)")


def verify(binary: str) -> None:
    """Quick verification against system yes."""
    passed = 0
    failed = 0

    tests = [
        ("--help", ["--help"], True),
        ("--version", ["--version"], True),
        ("--helpx error", ["--helpx"], True),
        ("-n error", ["-n"], True),
        ("--help extra", ["--help", "extra"], True),
    ]

    for label, args, exact in tests:
        fo, fe, fr = capture([f"./{binary}"] + args)
        yo, ye, yr = capture(["yes"] + args)

        ok = (fo == yo and fe == ye and fr == yr)
        tag = "PASS" if ok else "FAIL"
        print(f"  [{tag}] {label}")
        if ok:
            passed += 1
        else:
            failed += 1
            if fo != yo:
                print(f"         stdout: fyes={len(fo)}b yes={len(yo)}b")
            if fe != ye:
                print(f"         stderr: fyes={fe[:60]!r}")
                print(f"                  yes={ye[:60]!r}")
            if fr != yr:
                print(f"         exit:   fyes={fr} yes={yr}")

    print(f"  {passed}/{passed + failed} passed")


def main():
    parser = argparse.ArgumentParser(description="Build fyes matched to system GNU yes")
    parser.add_argument("-o", "--output", default="fyes", help="Output binary name")
    parser.add_argument("--detect", action="store_true", help="Just detect, don't build")
    parser.add_argument("--no-verify", action="store_true", help="Skip verification")
    args = parser.parse_args()

    script_dir = os.path.dirname(os.path.abspath(__file__))
    os.chdir(script_dir)

    print("[*] Detecting system yes...")
    data = detect_system_yes()
    print_detection(data)

    if args.detect:
        return

    print("[*] Patching assembly...")
    nasm_data = generate_data_section(data)
    patch_asm(ASM_FILE, nasm_data)
    print(f"  Updated {ASM_FILE}")

    print("[*] Assembling...")
    build(args.output)

    if not args.no_verify:
        print("[*] Verifying...")
        verify(args.output)


if __name__ == "__main__":
    main()