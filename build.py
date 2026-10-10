#!/usr/bin/env python3
"""
build.py — Build script for Linkco PDF Editor.

Builds the Linkco PDF Editor desktop application (`pdfcraft` / `LinkcoPDFEditor.exe`)
and command-line tool (`pdfcraft-cli`) and copies the compiled binaries into `dist/release/`.

Usage:
    python build.py                  # Release build for the current machine
    python build.py --debug          # Debug build
    python build.py --arch x64       # Explicit target architecture (x64, x86, arm64)
    python build.py --run            # Launch Linkco PDF Editor after building
"""

from __future__ import annotations

import argparse
import datetime as dt
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent

WINDOWS_TARGETS = {
    "x64": "x86_64-pc-windows-msvc",
    "x86": "i686-pc-windows-msvc",
    "arm64": "aarch64-pc-windows-msvc",
}

PE_MACHINES = {
    "x64": 0x8664,
    "x86": 0x014C,
    "arm64": 0xAA64,
}


def read_workspace_version(root: Path = ROOT) -> str:
    """Read `[workspace.package] version` from Cargo.toml."""
    cargo_toml = (root / "Cargo.toml").read_text(encoding="utf-8")
    in_workspace_pkg = False
    for line in cargo_toml.splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_workspace_pkg = stripped == "[workspace.package]"
            continue
        if in_workspace_pkg:
            m = re.match(r'^version\s*=\s*"([^"]+)"', stripped)
            if m:
                return m.group(1)
    raise RuntimeError("Could not read [workspace.package] version from Cargo.toml")


def detect_host_arch() -> str:
    machine = platform.machine().lower()
    if machine in ("amd64", "x86_64", "x64"):
        return "x64"
    if machine in ("aarch64", "arm64"):
        return "arm64"
    if machine in ("i386", "i686", "x86"):
        return "x86"
    return "x64"


def resolve_cargo() -> str:
    """Find the `cargo` executable in PATH or standard ~/.cargo/bin."""
    found = shutil.which("cargo")
    if found:
        return found
    exe_name = "cargo.exe" if os.name == "nt" else "cargo"
    candidate = Path.home() / ".cargo" / "bin" / exe_name
    if candidate.is_file():
        return str(candidate)
    raise FileNotFoundError(
        "Could not find `cargo`. Install the Rust toolchain from https://www.rust-lang.org/tools/install"
    )


def verify_pe_header(exe_path: Path, expected_arch: str, expected_subsystem: int) -> None:
    """Verify the Windows PE COFF machine type and subsystem (2 = GUI, 3 = Console)."""
    data = exe_path.read_bytes()
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise RuntimeError(f"{exe_path.name} is not a valid Windows PE executable")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    machine = struct.unpack_from("<H", data, pe_offset + 4)[0]
    subsystem = struct.unpack_from("<H", data, pe_offset + 0x5C)[0]
    expected_machine = PE_MACHINES[expected_arch]
    if machine != expected_machine:
        raise RuntimeError(
            f"{exe_path.name} has PE machine 0x{machine:X}, expected 0x{expected_machine:X} ({expected_arch})"
        )
    if subsystem != expected_subsystem:
        raise RuntimeError(
            f"{exe_path.name} has PE subsystem {subsystem}, expected {expected_subsystem}"
        )


def git_head_sha(root: Path = ROOT) -> str:
    try:
        out = subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            stderr=subprocess.DEVNULL,
            text=True,
        )
        return out.strip()
    except Exception:
        return "unknown"


def build_app(
    arch: str | None = None,
    release: bool = True,
    target: str | None = None,
    windows: bool = False,
    static_crt: bool = True,
    locked: bool = False,
    dist_dir: Path | None = None,
) -> dict[str, Path]:
    """
    Compile Linkco PDF Editor (`pdfcraft` and `pdfcraft-cli`) and copy binaries to `dist/release`.
    Returns a dictionary mapping artifact keys (`gui`, `linkco_gui`, `cli`, `bin_dir`, `dist_dir`) to Paths.
    """
    version = read_workspace_version(ROOT)
    resolved_arch = arch or detect_host_arch()
    is_windows_build = windows or os.name == "nt" or (target is not None and "windows" in target)

    if is_windows_build and target is None:
        target = WINDOWS_TARGETS.get(resolved_arch, WINDOWS_TARGETS["x64"])

    cargo = resolve_cargo()
    env = os.environ.copy()
    env.setdefault("PDFCRAFT_BUILD_SHA", git_head_sha(ROOT))
    env.setdefault("PDFCRAFT_BUILD_DATE", dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d"))

    if is_windows_build and target:
        if static_crt:
            flag_var = f"CARGO_TARGET_{target.upper().replace('-', '_')}_RUSTFLAGS"
            env.setdefault(flag_var, "-C target-feature=+crt-static")
        if os.name == "nt":
            env.setdefault("PDFCRAFT_REQUIRE_WINRES", "1")

    profile_name = "release" if release else "debug"
    cmd = [cargo, "build"]
    if release:
        cmd.append("--release")
    if locked:
        cmd.append("--locked")
    cmd.extend(["-p", "pdfcraft", "-p", "pdfcraft-cli"])
    if target:
        cmd.extend(["--target", target])

    print(f"==> Building Linkco PDF Editor v{version} ({profile_name}, arch={resolved_arch})")
    print(f"    Command: {' '.join(cmd)}")
    subprocess.run(cmd, cwd=str(ROOT), env=env, check=True)

    target_root = Path(env.get("CARGO_TARGET_DIR", str(ROOT / "target")))
    bin_dir = (target_root / target / profile_name) if target else (target_root / profile_name)

    ext = ".exe" if is_windows_build else ""
    gui_bin = bin_dir / f"pdfcraft{ext}"
    cli_bin = bin_dir / f"pdfcraft-cli{ext}"

    if not gui_bin.is_file():
        raise FileNotFoundError(f"Expected GUI binary not found at {gui_bin}")
    if not cli_bin.is_file():
        raise FileNotFoundError(f"Expected CLI binary not found at {cli_bin}")

    if is_windows_build and resolved_arch in PE_MACHINES:
        verify_pe_header(gui_bin, resolved_arch, expected_subsystem=2)
        verify_pe_header(cli_bin, resolved_arch, expected_subsystem=3)
        print(f"==> Verified Windows PE headers ({resolved_arch}: GUI subsystem=2, CLI subsystem=3)")

    out_dir = dist_dir or Path(env.get("DIST", str(ROOT / "dist" / "release")))
    out_dir.mkdir(parents=True, exist_ok=True)

    linkco_gui_name = f"LinkcoPDFEditor{ext}"
    dist_linkco_gui = out_dir / linkco_gui_name
    dist_gui = out_dir / gui_bin.name
    dist_cli = out_dir / cli_bin.name

    shutil.copy2(gui_bin, dist_linkco_gui)
    shutil.copy2(gui_bin, dist_gui)
    shutil.copy2(cli_bin, dist_cli)

    # Also keep a copy named LinkcoPDFEditor(.exe) in bin_dir for convenience
    bin_linkco_gui = bin_dir / linkco_gui_name
    shutil.copy2(gui_bin, bin_linkco_gui)

    print("\n==> Build complete:")
    for label, p in [
        ("Linkco PDF Editor (GUI)", dist_linkco_gui),
        ("pdfcraft binary (GUI)", dist_gui),
        ("pdfcraft-cli (CLI)", dist_cli),
    ]:
        size_mb = p.stat().st_size / (1024 * 1024)
        print(f"    - {label:<26}: {p} ({size_mb:.2f} MB)")

    return {
        "gui": dist_gui,
        "linkco_gui": dist_linkco_gui,
        "cli": dist_cli,
        "bin_dir": bin_dir,
        "dist_dir": out_dir,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Build Linkco PDF Editor desktop app and CLI."
    )
    parser.add_argument(
        "--arch",
        choices=["x64", "x86", "arm64"],
        default=None,
        help="Target CPU architecture (default: host architecture).",
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Build in debug mode instead of release mode.",
    )
    parser.add_argument(
        "--target",
        default=None,
        help="Explicit Rust target triple (e.g. x86_64-pc-windows-msvc).",
    )
    parser.add_argument(
        "--windows",
        action="store_true",
        help="Target Windows MSVC even when invoked with default flags.",
    )
    parser.add_argument(
        "--no-static-crt",
        action="store_true",
        help="Do not set +crt-static on Windows builds.",
    )
    parser.add_argument(
        "--locked",
        action="store_true",
        help="Pass --locked to cargo build.",
    )
    parser.add_argument(
        "--dist",
        type=Path,
        default=None,
        help="Output directory for built binaries (default: dist/release).",
    )
    parser.add_argument(
        "--run",
        action="store_true",
        help="Launch Linkco PDF Editor after the build succeeds.",
    )
    args = parser.parse_args(argv)

    try:
        artifacts = build_app(
            arch=args.arch,
            release=not args.debug,
            target=args.target,
            windows=args.windows,
            static_crt=not args.no_static_crt,
            locked=args.locked,
            dist_dir=args.dist,
        )
        if args.run:
            exe = artifacts["linkco_gui"]
            print(f"\n==> Launching {exe} ...")
            subprocess.run([str(exe)], check=False)
        return 0
    except Exception as exc:
        print(f"\nERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
