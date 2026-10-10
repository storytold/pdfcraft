#!/usr/bin/env python3
"""
installer.py — Build the Linkco PDF Editor Windows application and Windows installers.

This script:
  1. Builds `LinkcoPDFEditor.exe` (`pdfcraft.exe`) and `pdfcraft-cli.exe` for Windows
     in release mode with static CRT linking (`+crt-static`) and embedded Linkco metadata.
  2. Builds the Windows Setup installer `dist/release/LinkcoPDFEditorSetup.exe`
     (using NSIS `makensis` if installed, or Windows' built-in .NET compiler `csc.exe`
     so a standalone GUI `LinkcoPDFEditorSetup.exe` is always produced on Windows even
     without third-party installer tools).
  3. Builds the Windows MSI installer `dist/release/LinkcoPDFEditorSetup-<version>-windows-<arch>.msi`
     when WiX Toolset v5 (`wix`) is available.
  4. Packages the portable archive `dist/release/LinkcoPDFEditor-<version>-windows-<arch>-portable.zip`.

Usage:
    python installer.py                    # Build Windows app + installers (x64 default)
    python installer.py --arch arm64       # Build for Windows ARM64
    python installer.py --skip-build       # Package already-built binaries
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

import build

ROOT = Path(__file__).resolve().parent

APP_NAME = "Linkco PDF Editor"
COMPANY_NAME = "Al Rawabet Commercial Services & Contracting Company W.L.L."
COPYRIGHT = "© Al Rawabet Commercial Services & Contracting Company W.L.L."


def find_makensis() -> str | None:
    """Locate NSIS `makensis` in PATH or standard Windows installation directories."""
    found = shutil.which("makensis")
    if found:
        return found
    if os.name == "nt":
        for prog_env in ("ProgramFiles(x86)", "ProgramFiles"):
            base = os.environ.get(prog_env)
            if base:
                candidate = Path(base) / "NSIS" / "makensis.exe"
                if candidate.is_file():
                    return str(candidate)
    return None


def find_wix() -> str | None:
    """Locate WiX v5 (`wix`) in PATH or ~/.dotnet/tools."""
    found = shutil.which("wix")
    if found:
        return found
    exe_name = "wix.exe" if os.name == "nt" else "wix"
    candidate = Path.home() / ".dotnet" / "tools" / exe_name
    if candidate.is_file():
        return str(candidate)
    return None


def find_csc() -> str | None:
    """Locate the built-in Windows .NET Framework C# compiler (`csc.exe`)."""
    found = shutil.which("csc")
    if found:
        return found
    if os.name == "nt":
        windir = Path(os.environ.get("WINDIR", r"C:\Windows"))
        for sub in (
            r"Microsoft.NET\Framework64\v4.0.30319\csc.exe",
            r"Microsoft.NET\Framework\v4.0.30319\csc.exe",
        ):
            candidate = windir / sub
            if candidate.is_file():
                return str(candidate)
    return None


def maybe_sign(files: list[Path]) -> None:
    """Invoke packaging/windows/sign.ps1 when running on Windows with PowerShell available."""
    if os.name != "nt":
        return
    sign_ps1 = ROOT / "packaging" / "windows" / "sign.ps1"
    pwsh = shutil.which("pwsh") or shutil.which("powershell")
    if not sign_ps1.is_file() or not pwsh:
        return
    cmd = [pwsh, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(sign_ps1)] + [
        str(f) for f in files if f.is_file()
    ]
    subprocess.run(cmd, cwd=str(ROOT), check=False)


CSHARP_INSTALLER_SOURCE = r"""
using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Windows.Forms;
using Microsoft.Win32;

[assembly: AssemblyTitle("Linkco PDF Editor Setup")]
[assembly: AssemblyDescription("Linkco PDF Editor Installer")]
[assembly: AssemblyCompany("Al Rawabet Commercial Services & Contracting Company W.L.L.")]
[assembly: AssemblyProduct("Linkco PDF Editor")]
[assembly: AssemblyCopyright("© Al Rawabet Commercial Services & Contracting Company W.L.L.")]
[assembly: AssemblyVersion("__FILE_VERSION__")]
[assembly: AssemblyFileVersion("__FILE_VERSION__")]

namespace LinkcoSetup
{
    static class Program
    {
        public const string AppName = "Linkco PDF Editor";
        public const string CompanyName = "Al Rawabet Commercial Services & Contracting Company W.L.L.";
        public const string AppVersion = "__DISPLAY_VERSION__";
        public const string UninstallKey = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\Linkco PDF Editor";

        [STAThread]
        static int Main(string[] args)
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);

            bool silent = false;
            bool uninstall = false;
            foreach (string a in args)
            {
                string low = a.ToLowerInvariant();
                if (low == "/s" || low == "-s" || low == "/quiet" || low == "--silent")
                    silent = true;
                if (low == "/uninstall" || low == "--uninstall" || low == "/u")
                    uninstall = true;
            }

            if (uninstall)
            {
                return RunUninstall(silent);
            }

            string defaultDir = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
                "Linkco",
                "Linkco PDF Editor"
            );

            if (silent)
            {
                try
                {
                    PerformInstall(defaultDir, true, true, false);
                    return 0;
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine("Installation failed: " + ex.Message);
                    return 1;
                }
            }

            using (SetupForm form = new SetupForm(defaultDir))
            {
                Application.Run(form);
                return form.ExitCode;
            }
        }

        public static void PerformInstall(string installDir, bool desktopShortcut, bool startMenuShortcut, bool launchAfter)
        {
            Directory.CreateDirectory(installDir);

            Assembly asm = Assembly.GetExecutingAssembly();
            using (Stream payload = asm.GetManifestResourceStream("payload.zip"))
            {
                if (payload == null)
                    throw new InvalidOperationException("Embedded payload.zip resource is missing.");

                using (ZipArchive archive = new ZipArchive(payload, ZipArchiveMode.Read))
                {
                    foreach (ZipArchiveEntry entry in archive.Entries)
                    {
                        if (string.IsNullOrEmpty(entry.Name))
                            continue;
                        string destPath = Path.Combine(installDir, entry.Name);
                        using (Stream src = entry.Open())
                        using (FileStream dst = new FileStream(destPath, FileMode.Create, FileAccess.Write))
                        {
                            src.CopyTo(dst);
                        }
                    }
                }
            }

            string selfExe = Assembly.GetExecutingAssembly().Location;
            string uninstallerPath = Path.Combine(installDir, "Uninstall.exe");
            File.Copy(selfExe, uninstallerPath, true);

            string mainExe = Path.Combine(installDir, "LinkcoPDFEditor.exe");
            if (!File.Exists(mainExe))
            {
                string pdfcraftExe = Path.Combine(installDir, "pdfcraft.exe");
                if (File.Exists(pdfcraftExe))
                    File.Copy(pdfcraftExe, mainExe, true);
            }

            if (startMenuShortcut)
            {
                string programsDir = Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms);
                string groupDir = Path.Combine(programsDir, AppName);
                Directory.CreateDirectory(groupDir);
                CreateShortcut(Path.Combine(groupDir, AppName + ".lnk"), mainExe, installDir, AppName);
            }

            if (desktopShortcut)
            {
                string desktopDir = Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
                if (string.IsNullOrEmpty(desktopDir) || !Directory.Exists(desktopDir))
                    desktopDir = Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory);
                CreateShortcut(Path.Combine(desktopDir, AppName + ".lnk"), mainExe, installDir, AppName);
            }

            RegisterApplication(installDir, mainExe, uninstallerPath);

            if (launchAfter && File.Exists(mainExe))
            {
                Process.Start(new ProcessStartInfo(mainExe) { UseShellExecute = true });
            }
        }

        private static void RegisterApplication(string installDir, string mainExe, string uninstallerPath)
        {
            using (RegistryKey k = Registry.LocalMachine.CreateSubKey(UninstallKey))
            {
                if (k != null)
                {
                    k.SetValue("DisplayName", AppName);
                    k.SetValue("Publisher", CompanyName);
                    k.SetValue("DisplayVersion", AppVersion);
                    k.SetValue("DisplayIcon", mainExe + ",0");
                    k.SetValue("InstallLocation", installDir);
                    k.SetValue("UninstallString", "\"" + uninstallerPath + "\" /uninstall");
                    k.SetValue("QuietUninstallString", "\"" + uninstallerPath + "\" /uninstall /S");
                    k.SetValue("NoModify", 1, RegistryValueKind.DWord);
                    k.SetValue("NoRepair", 1, RegistryValueKind.DWord);
                }
            }

            using (RegistryKey progId = Registry.LocalMachine.CreateSubKey(@"Software\Classes\LinkcoPDFEditor.Document"))
            {
                if (progId != null)
                {
                    progId.SetValue("", "PDF Document");
                    using (RegistryKey iconKey = progId.CreateSubKey("DefaultIcon"))
                        if (iconKey != null) iconKey.SetValue("", mainExe + ",0");
                    using (RegistryKey cmdKey = progId.CreateSubKey(@"shell\open\command"))
                        if (cmdKey != null) cmdKey.SetValue("", "\"" + mainExe + "\" \"%1\"");
                }
            }

            using (RegistryKey openWith = Registry.LocalMachine.CreateSubKey(@"Software\Classes\.pdf\OpenWithProgids"))
            {
                if (openWith != null)
                    openWith.SetValue("LinkcoPDFEditor.Document", "");
            }
        }

        private static int RunUninstall(bool silent)
        {
            if (!silent)
            {
                DialogResult confirm = MessageBox.Show(
                    "Are you sure you want to uninstall " + AppName + "?",
                    AppName + " Uninstall",
                    MessageBoxButtons.YesNo,
                    MessageBoxIcon.Question
                );
                if (confirm != DialogResult.Yes)
                    return 0;
            }

            try
            {
                string installDir = Path.GetDirectoryName(Assembly.GetExecutingAssembly().Location);
                string desktopLnk = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory),
                    AppName + ".lnk"
                );
                if (File.Exists(desktopLnk)) File.Delete(desktopLnk);

                string userDesktopLnk = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
                    AppName + ".lnk"
                );
                if (File.Exists(userDesktopLnk)) File.Delete(userDesktopLnk);

                string startMenuGroup = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms),
                    AppName
                );
                if (Directory.Exists(startMenuGroup)) Directory.Delete(startMenuGroup, true);

                Registry.LocalMachine.DeleteSubKeyTree(UninstallKey, false);
                Registry.LocalMachine.DeleteSubKeyTree(@"Software\Classes\LinkcoPDFEditor.Document", false);
                using (RegistryKey openWith = Registry.LocalMachine.OpenSubKey(@"Software\Classes\.pdf\OpenWithProgids", true))
                {
                    if (openWith != null)
                        openWith.DeleteValue("LinkcoPDFEditor.Document", false);
                }

                foreach (string f in new string[] { "LinkcoPDFEditor.exe", "pdfcraft.exe", "pdfcraft-cli.exe" })
                {
                    string p = Path.Combine(installDir, f);
                    if (File.Exists(p)) File.Delete(p);
                }

                if (!silent)
                {
                    MessageBox.Show(
                        AppName + " has been uninstalled.",
                        AppName + " Uninstall",
                        MessageBoxButtons.OK,
                        MessageBoxIcon.Information
                    );
                }

                string selfExe = Assembly.GetExecutingAssembly().Location;
                Process.Start(new ProcessStartInfo("cmd.exe", "/C ping 127.0.0.1 -n 2 > nul & del /F /Q \"" + selfExe + "\" & rmdir \"" + installDir + "\"")
                {
                    WindowStyle = ProcessWindowStyle.Hidden,
                    CreateNoWindow = true,
                    UseShellExecute = false
                });
                return 0;
            }
            catch (Exception ex)
            {
                if (!silent)
                    MessageBox.Show("Uninstall error: " + ex.Message, AppName, MessageBoxButtons.OK, MessageBoxIcon.Error);
                return 1;
            }
        }

        private static void CreateShortcut(string shortcutPath, string targetPath, string workingDir, string description)
        {
            Type shellType = Type.GetTypeFromProgID("WScript.Shell");
            if (shellType == null) return;
            object shell = Activator.CreateInstance(shellType);
            object shortcut = shellType.InvokeMember("CreateShortcut", BindingFlags.InvokeMethod, null, shell, new object[] { shortcutPath });
            Type scType = shortcut.GetType();
            scType.InvokeMember("TargetPath", BindingFlags.SetProperty, null, shortcut, new object[] { targetPath });
            scType.InvokeMember("WorkingDirectory", BindingFlags.SetProperty, null, shortcut, new object[] { workingDir });
            scType.InvokeMember("Description", BindingFlags.SetProperty, null, shortcut, new object[] { description });
            scType.InvokeMember("IconLocation", BindingFlags.SetProperty, null, shortcut, new object[] { targetPath + ",0" });
            scType.InvokeMember("Save", BindingFlags.InvokeMethod, null, shortcut, null);
        }
    }

    sealed class SetupForm : Form
    {
        public int ExitCode = 0;
        private TextBox dirBox;
        private CheckBox desktopCheck;
        private CheckBox startMenuCheck;
        private CheckBox launchCheck;
        private Button installBtn;

        public SetupForm(string defaultDir)
        {
            Text = Program.AppName + " v" + Program.AppVersion + " Setup";
            Size = new Size(520, 340);
            FormBorderStyle = FormBorderStyle.FixedDialog;
            MaximizeBox = false;
            StartPosition = FormStartPosition.CenterScreen;
            Icon = Icon.ExtractAssociatedIcon(Assembly.GetExecutingAssembly().Location);

            Label title = new Label
            {
                Text = Program.AppName,
                Font = new Font("Segoe UI", 15f, FontStyle.Bold),
                AutoSize = true,
                Location = new Point(22, 18)
            };
            Controls.Add(title);

            Label sub = new Label
            {
                Text = "Publisher: " + Program.CompanyName + "\nVersion: " + Program.AppVersion,
                Font = new Font("Segoe UI", 9f, FontStyle.Regular),
                AutoSize = true,
                Location = new Point(24, 50)
            };
            Controls.Add(sub);

            Label dirLabel = new Label
            {
                Text = "Installation Folder:",
                Font = new Font("Segoe UI", 9f, FontStyle.Bold),
                AutoSize = true,
                Location = new Point(24, 102)
            };
            Controls.Add(dirLabel);

            dirBox = new TextBox
            {
                Text = defaultDir,
                Location = new Point(24, 124),
                Size = new Size(370, 24)
            };
            Controls.Add(dirBox);

            Button browseBtn = new Button
            {
                Text = "Browse...",
                Location = new Point(404, 122),
                Size = new Size(80, 26)
            };
            browseBtn.Click += (s, e) =>
            {
                using (FolderBrowserDialog fbd = new FolderBrowserDialog { SelectedPath = dirBox.Text })
                {
                    if (fbd.ShowDialog(this) == DialogResult.OK)
                        dirBox.Text = fbd.SelectedPath;
                }
            };
            Controls.Add(browseBtn);

            startMenuCheck = new CheckBox
            {
                Text = "Create Start Menu shortcut",
                Checked = true,
                AutoSize = true,
                Location = new Point(24, 166)
            };
            Controls.Add(startMenuCheck);

            desktopCheck = new CheckBox
            {
                Text = "Create Desktop shortcut",
                Checked = true,
                AutoSize = true,
                Location = new Point(24, 192)
            };
            Controls.Add(desktopCheck);

            launchCheck = new CheckBox
            {
                Text = "Launch " + Program.AppName + " when installation finishes",
                Checked = true,
                AutoSize = true,
                Location = new Point(24, 218)
            };
            Controls.Add(launchCheck);

            installBtn = new Button
            {
                Text = "Install",
                Font = new Font("Segoe UI", 9.5f, FontStyle.Bold),
                Location = new Point(304, 256),
                Size = new Size(90, 30)
            };
            installBtn.Click += OnInstallClick;
            Controls.Add(installBtn);

            Button cancelBtn = new Button
            {
                Text = "Cancel",
                Location = new Point(404, 256),
                Size = new Size(80, 30)
            };
            cancelBtn.Click += (s, e) => { ExitCode = 1; Close(); };
            Controls.Add(cancelBtn);
        }

        private void OnInstallClick(object sender, EventArgs e)
        {
            installBtn.Enabled = false;
            Cursor = Cursors.WaitCursor;
            try
            {
                Program.PerformInstall(dirBox.Text, desktopCheck.Checked, startMenuCheck.Checked, launchCheck.Checked);
                Cursor = Cursors.Default;
                MessageBox.Show(
                    this,
                    Program.AppName + " has been installed successfully.",
                    Program.AppName + " Setup",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Information
                );
                ExitCode = 0;
                Close();
            }
            catch (Exception ex)
            {
                Cursor = Cursors.Default;
                installBtn.Enabled = true;
                MessageBox.Show(
                    this,
                    "Installation failed:\n" + ex.Message,
                    Program.AppName + " Setup",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Error
                );
            }
        }
    }
}
"""


def build_dotnet_setup_exe(
    csc_path: str,
    stage_dir: Path,
    icon_path: Path,
    version: str,
    msi_version: str,
    out_exe: Path,
) -> Path:
    """
    Build a standalone Windows GUI installer `LinkcoPDFEditorSetup.exe` using Windows' built-in
    `csc.exe` compiler and an embedded `payload.zip` containing the built release binaries.
    """
    parts = [p for p in msi_version.split(".") if p.isdigit()]
    while len(parts) < 4:
        parts.append("0")
    file_version = ".".join(parts[:4])

    with tempfile.TemporaryDirectory(prefix="linkco-installer-") as tmp_str:
        tmp = Path(tmp_str)
        payload_zip = tmp / "payload.zip"
        with zipfile.ZipFile(payload_zip, "w", compression=zipfile.ZIP_DEFLATED) as zf:
            for exe_name in ("LinkcoPDFEditor.exe", "pdfcraft.exe", "pdfcraft-cli.exe"):
                f = stage_dir / exe_name
                if f.is_file():
                    zf.write(f, arcname=exe_name)

        cs_file = tmp / "Setup.cs"
        source = (
            CSHARP_INSTALLER_SOURCE.replace("__FILE_VERSION__", file_version).replace(
                "__DISPLAY_VERSION__", version
            )
        )
        cs_file.write_text(source, encoding="utf-8-sig")

        manifest_file = tmp / "app.manifest"
        manifest_file.write_text(
            """<?xml version="1.0" encoding="utf-8"?>
<assembly manifestVersion="1.0" xmlns="urn:schemas-microsoft-com:asm.v1">
  <assemblyIdentity version="1.0.0.0" name="LinkcoPDFEditor.Setup"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v2">
    <security>
      <requestedPrivileges xmlns="urn:schemas-microsoft-com:asm.v3">
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
""",
            encoding="utf-8",
        )

        cmd = [
            csc_path,
            "/nologo",
            "/target:winexe",
            "/optimize+",
            f"/out:{out_exe}",
            f"/win32icon:{icon_path}",
            f"/win32manifest:{manifest_file}",
            f"/resource:{payload_zip},payload.zip",
            "/r:System.dll",
            "/r:System.Drawing.dll",
            "/r:System.Windows.Forms.dll",
            "/r:System.IO.Compression.dll",
            "/r:System.IO.Compression.FileSystem.dll",
            str(cs_file),
        ]
        print(f"==> Building standalone Windows Setup EXE via csc.exe: {out_exe}")
        subprocess.run(cmd, cwd=str(ROOT), check=True)

    return out_exe


def build_windows_installer(
    arch: str = "x64",
    skip_build: bool = False,
    dist_dir: Path | None = None,
) -> list[Path]:
    """
    Build the Windows application binaries and package the Windows installer(s) and portable ZIP.
    Returns a list of generated artifact paths in `dist/release`.
    """
    version = build.read_workspace_version(ROOT)
    msi_version = version.split("-")[0]
    target = build.WINDOWS_TARGETS.get(arch, build.WINDOWS_TARGETS["x64"])

    out_dir = dist_dir or Path(os.environ.get("DIST", str(ROOT / "dist" / "release")))
    out_dir.mkdir(parents=True, exist_ok=True)
    target_root = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))

    if not skip_build:
        build.build_app(
            arch=arch,
            release=True,
            target=target,
            windows=True,
            static_crt=True,
            dist_dir=out_dir,
        )

    bin_dir = target_root / target / "release"
    gui_exe = bin_dir / "pdfcraft.exe"
    cli_exe = bin_dir / "pdfcraft-cli.exe"

    if not gui_exe.is_file() or not cli_exe.is_file():
        raise FileNotFoundError(
            f"Windows binaries not found in {bin_dir}. Run without --skip-build first."
        )

    build.verify_pe_header(gui_exe, arch, expected_subsystem=2)
    build.verify_pe_header(cli_exe, arch, expected_subsystem=3)

    stage_dir = target_root / "windows-package" / arch
    if stage_dir.exists():
        shutil.rmtree(stage_dir)
    stage_dir.mkdir(parents=True, exist_ok=True)

    shutil.copy2(gui_exe, stage_dir / "pdfcraft.exe")
    shutil.copy2(gui_exe, stage_dir / "LinkcoPDFEditor.exe")
    shutil.copy2(cli_exe, stage_dir / "pdfcraft-cli.exe")

    maybe_sign([stage_dir / "pdfcraft.exe", stage_dir / "LinkcoPDFEditor.exe", stage_dir / "pdfcraft-cli.exe"])

    generated: list[Path] = [
        out_dir / "LinkcoPDFEditor.exe",
        out_dir / "pdfcraft.exe",
        out_dir / "pdfcraft-cli.exe",
    ]
    icon_path = ROOT / "assets" / "app-icon" / "pdfcraft.ico"

    # 1. Build LinkcoPDFEditorSetup.exe (via NSIS makensis or built-in Windows .NET csc.exe)
    setup_exe = out_dir / "LinkcoPDFEditorSetup.exe"
    versioned_setup_exe = out_dir / f"LinkcoPDFEditorSetup-{version}-windows-{arch}.exe"
    makensis = find_makensis()
    csc = find_csc()

    if makensis:
        nsi_script = ROOT / "packaging" / "windows" / "installer.nsi"
        print(f"==> Building Windows Setup EXE with NSIS ({makensis})")
        subprocess.run(
            [
                makensis,
                f"/DVERSION={msi_version}",
                f"/DBIN_DIR={stage_dir}",
                f"/DICON_PATH={icon_path}",
                f"/DOUT_FILE={setup_exe}",
                str(nsi_script),
            ],
            cwd=str(ROOT),
            check=True,
        )
        maybe_sign([setup_exe])
        shutil.copy2(setup_exe, versioned_setup_exe)
        generated.extend([setup_exe, versioned_setup_exe])
    elif csc:
        build_dotnet_setup_exe(
            csc_path=csc,
            stage_dir=stage_dir,
            icon_path=icon_path,
            version=version,
            msi_version=msi_version,
            out_exe=setup_exe,
        )
        maybe_sign([setup_exe])
        shutil.copy2(setup_exe, versioned_setup_exe)
        generated.extend([setup_exe, versioned_setup_exe])
    else:
        print(
            "==> Note: Neither NSIS (`makensis`) nor Windows `csc.exe` was found on this host; "
            "skipping LinkcoPDFEditorSetup.exe generation."
        )

    # 2. Build LinkcoPDFEditorSetup-<version>-windows-<arch>.msi (when WiX v5 is installed)
    wix = find_wix()
    if wix:
        msi_path = out_dir / f"LinkcoPDFEditorSetup-{version}-windows-{arch}.msi"
        wxs_main = ROOT / "packaging" / "windows" / "pdfcraft.wxs"
        wxs_ui = ROOT / "packaging" / "windows" / "installer-ui.wxs"
        print(f"==> Building Windows MSI installer with WiX v5 ({wix})")
        subprocess.run(
            [
                wix,
                "build",
                str(wxs_main),
                "-arch",
                arch,
                str(wxs_ui),
                "-d",
                f"Version={msi_version}",
                "-d",
                f"BinDir={stage_dir}",
                "-d",
                f"IconPath={icon_path}",
                "-o",
                str(msi_path),
            ],
            cwd=str(ROOT),
            check=True,
        )
        wixpdb = msi_path.with_suffix(".wixpdb")
        if wixpdb.is_file():
            wixpdb.unlink()

        if os.name == "nt":
            pwsh = shutil.which("pwsh") or shutil.which("powershell")
            test_msi = ROOT / "packaging" / "windows" / "test-msi.ps1"
            if pwsh and test_msi.is_file():
                subprocess.run(
                    [pwsh, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(test_msi), str(msi_path)],
                    cwd=str(ROOT),
                    check=True,
                )

        maybe_sign([msi_path])
        generated.append(msi_path)
    else:
        print(
            "==> Note: WiX v5 (`wix`) not found (optional: `dotnet tool install --global wix --version 5.0.2`); "
            "skipping .msi generation."
        )

    # 3. Build Portable ZIP archive
    portable_zip = out_dir / f"LinkcoPDFEditor-{version}-windows-{arch}-portable.zip"
    print(f"==> Creating portable archive: {portable_zip}")
    with zipfile.ZipFile(portable_zip, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for exe_name in ("LinkcoPDFEditor.exe", "pdfcraft.exe", "pdfcraft-cli.exe"):
            f = stage_dir / exe_name
            if f.is_file():
                zf.write(f, arcname=exe_name)
        for doc_name in ("README.md", "LICENSE-MIT", "LICENSE-APACHE"):
            doc_path = ROOT / doc_name
            if doc_path.is_file():
                zf.write(doc_path, arcname=doc_name)
    generated.append(portable_zip)

    print("\n==> Windows installer & package output in dist/release:")
    for artifact in generated:
        if artifact.is_file():
            size_mb = artifact.stat().st_size / (1024 * 1024)
            print(f"    - {artifact.name:<48} ({size_mb:.2f} MB) -> {artifact}")

    return generated


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Build Linkco PDF Editor for Windows and package the Windows installer(s)."
    )
    parser.add_argument(
        "--arch",
        choices=["x64", "x86", "arm64"],
        default="x64",
        help="Target Windows architecture (default: x64).",
    )
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="Skip compiling the Rust workspace and package existing binaries.",
    )
    parser.add_argument(
        "--dist",
        type=Path,
        default=None,
        help="Output directory for generated installers (default: dist/release).",
    )
    args = parser.parse_args(argv)

    try:
        build_windows_installer(
            arch=args.arch,
            skip_build=args.skip_build,
            dist_dir=args.dist,
        )
        return 0
    except Exception as exc:
        print(f"\nERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
