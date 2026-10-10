#!/usr/bin/env python3
"""
build.py — Automatic dependency installer and build script for Linkco PDF Editor.

Automatically checks for and installs any missing build dependencies first:
  1. Visual Studio C++ Build Tools & Windows SDK (`cl.exe`, `link.exe`, `rc.exe`) on Windows
  2. CMake (`cmake.exe`) — auto-installed via `pip install cmake` (or `winget`) if missing
  3. Rust toolchain (`rustup`, `cargo`, `rustc`) — auto-downloaded and installed from
     `https://win.rustup.rs` (or `https://sh.rustup.rs`) if missing, and added to PATH
     for the current session.

Then compiles the Linkco PDF Editor desktop application (`LinkcoPDFEditor.exe` / `pdfcraft.exe`)
and command-line tool (`pdfcraft-cli.exe`) and copies them into `dist/release/`.

Usage:
    python build.py                  # Auto-install dependencies + release build
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
import sysconfig
import tempfile
import urllib.request
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


def prepend_to_path(directory: Path | str) -> None:
    """Prepend `directory` to `os.environ['PATH']` if it exists and is not already present."""
    p = Path(directory)
    if not p.is_dir():
        return
    dir_str = str(p)
    current = os.environ.get("PATH", "")
    parts = current.split(os.pathsep) if current else []
    norm_target = os.path.normcase(os.path.normpath(dir_str))
    for existing in parts:
        if os.path.normcase(os.path.normpath(existing)) == norm_target:
            return
    os.environ["PATH"] = dir_str + (os.pathsep + current if current else "")


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


def _find_windows_csc() -> Path | None:
    windir = Path(os.environ.get("WINDIR", r"C:\Windows"))
    for sub in [
        r"Microsoft.NET\Framework64\v4.0.30319\csc.exe",
        r"Microsoft.NET\Framework\v4.0.30319\csc.exe",
    ]:
        candidate = windir / sub
        if candidate.is_file():
            return candidate
    found = shutil.which("csc") or shutil.which("csc.exe")
    return Path(found) if found else None


def find_vs_installation() -> Path | None:
    """Return the Visual Studio / BuildTools installation directory if MSVC C++ tools are present."""
    if os.name != "nt":
        return None

    prog_x86 = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"))
    prog = Path(os.environ.get("ProgramFiles", r"C:\Program Files"))
    vswhere = prog_x86 / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"

    if vswhere.is_file():
        try:
            out = subprocess.check_output(
                [
                    str(vswhere),
                    "-latest",
                    "-products",
                    "*",
                    "-requires",
                    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                    "-property",
                    "installationPath",
                ],
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip()
            if out:
                vs_dir = Path(out.splitlines()[0].strip())
                if (vs_dir / "VC" / "Tools" / "MSVC").is_dir():
                    return vs_dir
        except Exception:
            pass

    for base in (prog_x86 / "Microsoft Visual Studio", prog / "Microsoft Visual Studio"):
        if not base.is_dir():
            continue
        for msvc_dir in base.glob("*/*/VC/Tools/MSVC"):
            if msvc_dir.is_dir() and any(msvc_dir.iterdir()):
                return msvc_dir.parent.parent.parent

    return None


def configure_windows_sdk_and_vs_paths(vs_dir: Path | None, arch: str = "x64") -> None:
    """Add Visual Studio bundled CMake, Ninja, MSVC bin, and Windows SDK `rc.exe` to PATH."""
    if os.name != "nt":
        return

    if vs_dir and vs_dir.is_dir():
        cmake_bin = (
            vs_dir
            / "Common7"
            / "IDE"
            / "CommonExtensions"
            / "Microsoft"
            / "CMake"
            / "CMake"
            / "bin"
        )
        ninja_bin = (
            vs_dir
            / "Common7"
            / "IDE"
            / "CommonExtensions"
            / "Microsoft"
            / "CMake"
            / "Ninja"
        )
        prepend_to_path(cmake_bin)
        prepend_to_path(ninja_bin)

        msvc_root = vs_dir / "VC" / "Tools" / "MSVC"
        if msvc_root.is_dir():
            versions = sorted([d for d in msvc_root.iterdir() if d.is_dir()], reverse=True)
            if versions:
                host_folder = "Hostarm64" if detect_host_arch() == "arm64" else "Hostx64"
                cl_dir = versions[0] / "bin" / host_folder / arch
                prepend_to_path(cl_dir)

    # Locate Windows 10/11 SDK bin folder (for rc.exe and signtool.exe)
    prog_x86 = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"))
    kits_bin = prog_x86 / "Windows Kits" / "10" / "bin"
    if kits_bin.is_dir():
        sdk_versions = sorted(
            [d for d in kits_bin.iterdir() if d.is_dir() and d.name.startswith("10.")],
            reverse=True,
        )
        for sdk_ver in sdk_versions:
            rc_dir = sdk_ver / arch
            if (rc_dir / "rc.exe").is_file():
                prepend_to_path(rc_dir)
                break


def ensure_msvc_build_tools(arch: str = "x64") -> None:
    """Ensure Visual Studio C++ Build Tools and the Windows SDK are installed on Windows."""
    if os.name != "nt":
        return

    vs_dir = find_vs_installation()
    if vs_dir is not None:
        configure_windows_sdk_and_vs_paths(vs_dir, arch=arch)
        print(f"==> Found Visual Studio C++ Build Tools at: {vs_dir}")
        return

    print("==> Visual Studio C++ Build Tools not found. Installing Microsoft C++ Build Tools + Windows SDK...")
    print("    (A Visual Studio Installer window may appear — please allow UAC if prompted.)")

    url = "https://aka.ms/vs/17/release/vs_buildtools.exe"
    with tempfile.TemporaryDirectory(prefix="linkco-vsbt-") as tmp_str:
        installer_exe = Path(tmp_str) / "vs_buildtools.exe"
        print(f"    Downloading {url} ...")
        urllib.request.urlretrieve(url, str(installer_exe))

        cmd = [
            str(installer_exe),
            "--passive",
            "--wait",
            "--norestart",
            "--nocache",
            "--add",
            "Microsoft.VisualStudio.Workload.VCTools",
            "--add",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "--add",
            "Microsoft.VisualStudio.Component.VC.CMake.Project",
            "--includeRecommended",
        ]
        if arch == "arm64":
            cmd.extend(["--add", "Microsoft.VisualStudio.Component.VC.Tools.ARM64"])

        res = subprocess.run(cmd, check=False)
        # 0 = success, 3010 = success (reboot recommended but not required to compile)
        if res.returncode not in (0, 3010):
            print(
                f"    Warning: vs_buildtools.exe exited with code {res.returncode}. Checking installation..."
            )

    vs_dir = find_vs_installation()
    if vs_dir is None:
        raise RuntimeError(
            "Visual Studio C++ Build Tools installation was not detected.\n"
            "Please install 'Desktop development with C++' via Visual Studio Build Tools:\n"
            "  https://visualstudio.microsoft.com/visual-cpp-build-tools/"
        )
    configure_windows_sdk_and_vs_paths(vs_dir, arch=arch)
    print(f"==> Visual Studio C++ Build Tools ready at: {vs_dir}")


def ensure_cmake() -> str:
    """Ensure `cmake` is available in PATH (required by aws-lc-sys). Auto-installs via pip if missing."""
    # Check standard Python Scripts directories first
    scripts_dir = sysconfig.get_path("scripts")
    if scripts_dir:
        prepend_to_path(scripts_dir)
    user_scripts = Path(sysconfig.get_path("scripts", f"{os.name}_user") or "")
    if user_scripts:
        prepend_to_path(user_scripts)

    if os.name == "nt":
        for prog_env in ("ProgramFiles", "ProgramFiles(x86)"):
            base = os.environ.get(prog_env)
            if base:
                prepend_to_path(Path(base) / "CMake" / "bin")

    found = shutil.which("cmake")
    if found:
        print(f"==> Found CMake: {found}")
        return found

    print("==> CMake not found. Installing CMake automatically via pip (`pip install cmake`)...")
    subprocess.run(
        [sys.executable, "-m", "pip", "install", "--upgrade", "cmake"],
        check=True,
    )

    if scripts_dir:
        prepend_to_path(scripts_dir)
    if user_scripts:
        prepend_to_path(user_scripts)
    prepend_to_path(Path(sys.executable).resolve().parent / "Scripts")
    prepend_to_path(Path(sys.executable).resolve().parent)

    found = shutil.which("cmake")
    if found:
        print(f"==> Installed CMake: {found}")
        return found

    if os.name == "nt" and shutil.which("winget"):
        print("==> Installing CMake via winget...")
        subprocess.run(
            [
                "winget",
                "install",
                "--id",
                "Kitware.CMake",
                "-e",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ],
            check=False,
        )
        for prog_env in ("ProgramFiles", "ProgramFiles(x86)"):
            base = os.environ.get(prog_env)
            if base:
                prepend_to_path(Path(base) / "CMake" / "bin")
        found = shutil.which("cmake")
        if found:
            return found

    raise RuntimeError("Could not find or install `cmake`. Please install CMake from https://cmake.org/download/")


def ensure_rust_toolchain(target: str | None = None) -> str:
    """
    Ensure `cargo` and `rustc` are installed and in PATH.
    If missing, automatically downloads and runs `rustup-init` non-interactively.
    """
    cargo_bin_dir = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))) / "bin"
    prepend_to_path(cargo_bin_dir)

    cargo = shutil.which("cargo")
    if not cargo:
        print("==> Rust (`cargo`) not found. Installing Rust toolchain automatically via rustup...")
        if os.name == "nt":
            host_arch = detect_host_arch()
            rustup_url = (
                "https://win.rustup.rs/aarch64"
                if host_arch == "arm64"
                else "https://win.rustup.rs/x86_64"
            )
            default_host = (
                "aarch64-pc-windows-msvc"
                if host_arch == "arm64"
                else "x86_64-pc-windows-msvc"
            )
            with tempfile.TemporaryDirectory(prefix="linkco-rustup-") as tmp_str:
                rustup_init = Path(tmp_str) / "rustup-init.exe"
                print(f"    Downloading {rustup_url} ...")
                urllib.request.urlretrieve(rustup_url, str(rustup_init))
                cmd = [
                    str(rustup_init),
                    "-y",
                    "--default-toolchain",
                    "stable",
                    "--default-host",
                    default_host,
                    "--profile",
                    "minimal",
                ]
                print(f"    Running: {' '.join(cmd)}")
                subprocess.run(cmd, check=True)
        else:
            with tempfile.TemporaryDirectory(prefix="linkco-rustup-") as tmp_str:
                rustup_sh = Path(tmp_str) / "rustup-init.sh"
                print("    Downloading https://sh.rustup.rs ...")
                urllib.request.urlretrieve("https://sh.rustup.rs", str(rustup_sh))
                subprocess.run(
                    ["sh", str(rustup_sh), "-y", "--default-toolchain", "stable", "--profile", "minimal"],
                    check=True,
                )

        prepend_to_path(cargo_bin_dir)
        cargo = shutil.which("cargo")

    if not cargo:
        raise RuntimeError(
            "Rust installation finished, but `cargo` was still not found in PATH or ~/.cargo/bin."
        )

    print(f"==> Found Cargo: {cargo}")

    # Ensure the requested Rust target triple is installed
    rustup = shutil.which("rustup")
    if target and rustup:
        try:
            installed_targets = subprocess.check_output(
                [rustup, "target", "list", "--installed"],
                text=True,
                stderr=subprocess.DEVNULL,
            ).splitlines()
            if target not in [t.strip() for t in installed_targets]:
                print(f"==> Installing Rust target `{target}` via rustup...")
                subprocess.run([rustup, "target", "add", target], check=True)
        except Exception as exc:
            print(f"    Warning: could not verify rustup target `{target}`: {exc}")

    return cargo


def ensure_dependencies(arch: str = "x64", target: str | None = None, windows: bool = False) -> str:
    """
    Ensure all required build dependencies (MSVC C++ Build Tools, Windows SDK, CMake, and Rust)
    are installed and configured in `os.environ['PATH']`. Returns the path to `cargo`.
    """
    print("==> Checking and installing build dependencies...")
    if windows or os.name == "nt":
        ensure_msvc_build_tools(arch=arch)
    ensure_cmake()
    cargo = ensure_rust_toolchain(target=target)
    return cargo


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
    Ensure all build dependencies are installed, compile Linkco PDF Editor (`pdfcraft` and
    `pdfcraft-cli`), and copy binaries to `dist/release`.
    Returns a dictionary mapping artifact keys (`gui`, `linkco_gui`, `cli`, `bin_dir`, `dist_dir`) to Paths.
    """
    version = read_workspace_version(ROOT)
    resolved_arch = arch or detect_host_arch()
    is_windows_build = windows or os.name == "nt" or (target is not None and "windows" in target)

    if is_windows_build and target is None:
        target = WINDOWS_TARGETS.get(resolved_arch, WINDOWS_TARGETS["x64"])

    cargo = ensure_dependencies(arch=resolved_arch, target=target, windows=is_windows_build)

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

    print(f"\n==> Building Linkco PDF Editor v{version} ({profile_name}, arch={resolved_arch})")
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

    bin_linkco_gui = bin_dir / linkco_gui_name
    shutil.copy2(gui_bin, bin_linkco_gui)

    preview_dll_src = ROOT / "packaging" / "windows" / "PreviewHandler.cs"
    dist_preview_dll = out_dir / "LinkcoPdfPreviewHandler.dll"
    if os.name == "nt" and preview_dll_src.is_file():
        csc = _find_windows_csc()
        if csc:
            bin_preview_dll = bin_dir / "LinkcoPdfPreviewHandler.dll"
            csc_cmd = [
                str(csc),
                "/nologo",
                "/target:library",
                "/optimize+",
                "/platform:anycpu",
                f"/out:{bin_preview_dll}",
                "/r:System.dll",
                "/r:System.Drawing.dll",
                "/r:System.Windows.Forms.dll",
                str(preview_dll_src),
            ]
            subprocess.run(csc_cmd, cwd=str(ROOT), check=True)
            shutil.copy2(bin_preview_dll, dist_preview_dll)
            print(f"==> Built Windows File Explorer Preview Handler: {dist_preview_dll}")

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
        description="Automatically install dependencies and build Linkco PDF Editor desktop app and CLI."
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
