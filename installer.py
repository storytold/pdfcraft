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
import hashlib
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


def find_makensis(auto_install: bool = True) -> str | None:
    """Locate NSIS `makensis` in PATH or standard Windows directories, auto-installing via winget if possible."""
    found = shutil.which("makensis")
    if found:
        return found
    if os.name == "nt":
        for prog_env in ("ProgramFiles(x86)", "ProgramFiles"):
            base = os.environ.get(prog_env)
            if base:
                candidate = Path(base) / "NSIS" / "makensis.exe"
                if candidate.is_file():
                    build.prepend_to_path(candidate.parent)
                    return str(candidate)
        if auto_install and shutil.which("winget"):
            print("==> Installing NSIS (`makensis`) via winget...")
            subprocess.run(
                [
                    "winget",
                    "install",
                    "--id",
                    "NSIS.NSIS",
                    "-e",
                    "--accept-source-agreements",
                    "--accept-package-agreements",
                    "--silent",
                ],
                check=False,
            )
            for prog_env in ("ProgramFiles(x86)", "ProgramFiles"):
                base = os.environ.get(prog_env)
                if base:
                    candidate = Path(base) / "NSIS" / "makensis.exe"
                    if candidate.is_file():
                        build.prepend_to_path(candidate.parent)
                        return str(candidate)
    return shutil.which("makensis")


def find_wix(auto_install: bool = True) -> str | None:
    """Locate WiX v5 (`wix`) in PATH or ~/.dotnet/tools, auto-installing via `dotnet tool` if `dotnet` is available."""
    dotnet_tools = Path.home() / ".dotnet" / "tools"
    build.prepend_to_path(dotnet_tools)

    found = shutil.which("wix")
    if found:
        return found
    exe_name = "wix.exe" if os.name == "nt" else "wix"
    candidate = dotnet_tools / exe_name
    if candidate.is_file():
        return str(candidate)

    if auto_install and shutil.which("dotnet"):
        print("==> Installing WiX Toolset v5 (`wix`) via `dotnet tool install`...")
        subprocess.run(
            ["dotnet", "tool", "install", "--global", "wix", "--version", "5.0.2"],
            check=False,
        )
        build.prepend_to_path(dotnet_tools)
        if candidate.is_file():
            return str(candidate)
        return shutil.which("wix")
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
using System.Collections.Generic;
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
        public const string LinkcoConfigKey = @"Software\Linkco\Linkco PDF Editor";
        public const string PreviewHandlerClsid = "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}";
        public const string PreviewHandlerCategoryGuid = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
        public const string PrevHostAppId64 = "{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}";
        public const string EdgePreviewHandlerClsid = "{3A84F9C2-6164-485C-A7D9-4B27F8AC009E}";

        [System.Runtime.InteropServices.DllImport("shell32.dll")]
        private static extern void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);

        public static bool ForcePerUser = false;

        public static bool IsMachineInstall()
        {
            if (ForcePerUser)
                return false;
            try
            {
                using (WindowsIdentity id = WindowsIdentity.GetCurrent())
                {
                    WindowsPrincipal principal = new WindowsPrincipal(id);
                    return principal.IsInRole(WindowsBuiltInRole.Administrator);
                }
            }
            catch
            {
                return false;
            }
        }

        private static RegistryKey GetRegistryRoot()
        {
            return IsMachineInstall() ? Registry.LocalMachine : Registry.CurrentUser;
        }

        [STAThread]
        static int Main(string[] args)
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);

            bool silent = false;
            bool uninstall = false;
            bool removeData = false;
            foreach (string a in args)
            {
                string low = a.ToLowerInvariant();
                if (low == "/s" || low == "-s" || low == "/quiet" || low == "--silent")
                    silent = true;
                if (low == "/uninstall" || low == "--uninstall" || low == "/u")
                    uninstall = true;
                if (low == "/currentuser" || low == "--current-user" || low == "/peruser")
                    ForcePerUser = true;
                if (low == "/remove-data" || low == "--remove-data" || low == "/purge-data")
                    removeData = true;
            }

            if (uninstall)
            {
                return RunUninstall(silent, removeData);
            }

            string defaultDir = IsMachineInstall()
                ? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Linkco", "Linkco PDF Editor")
                : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "Linkco", "Linkco PDF Editor");

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

            bool machine = IsMachineInstall();
            if (startMenuShortcut)
            {
                string programsDir = Environment.GetFolderPath(
                    machine ? Environment.SpecialFolder.CommonPrograms : Environment.SpecialFolder.Programs
                );
                if (string.IsNullOrEmpty(programsDir))
                    programsDir = Environment.GetFolderPath(Environment.SpecialFolder.Programs);
                string groupDir = Path.Combine(programsDir, AppName);
                Directory.CreateDirectory(groupDir);
                CreateShortcut(Path.Combine(groupDir, AppName + ".lnk"), mainExe, installDir, AppName);
            }

            if (desktopShortcut)
            {
                string commonDesktop = Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
                string userDesktop = Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory);
                string targetDesktop = (machine && !string.IsNullOrEmpty(commonDesktop) && Directory.Exists(commonDesktop))
                    ? commonDesktop
                    : userDesktop;
                if (targetDesktop == commonDesktop && !string.IsNullOrEmpty(userDesktop) && userDesktop != commonDesktop)
                {
                    string dup = Path.Combine(userDesktop, AppName + ".lnk");
                    if (File.Exists(dup))
                    {
                        try { File.Delete(dup); } catch { }
                    }
                }
                CreateShortcut(Path.Combine(targetDesktop, AppName + ".lnk"), mainExe, installDir, AppName);
            }

            RegisterApplication(installDir, mainExe, uninstallerPath);

            if (launchAfter && File.Exists(mainExe))
            {
                Process.Start(new ProcessStartInfo(mainExe) { UseShellExecute = true });
            }
        }

        private static void RegisterApplication(string installDir, string mainExe, string uninstallerPath)
        {
            RegistryKey root = GetRegistryRoot();
            using (RegistryKey k = root.CreateSubKey(UninstallKey))
            {
                if (k != null)
                {
                    k.SetValue("DisplayName", AppName);
                    k.SetValue("Publisher", CompanyName);
                    k.SetValue("DisplayVersion", AppVersion);
                    k.SetValue("DisplayIcon", mainExe + ",0");
                    k.SetValue("InstallLocation", installDir);
                    k.SetValue("URLInfoAbout", "https://www.linkco.com.qa");
                    k.SetValue("HelpLink", "https://www.linkco.com.qa/contact-us/");
                    k.SetValue("Contact", "info@linkco.com.qa (+974 4437 2511)");
                    k.SetValue("UninstallString", "\"" + uninstallerPath + "\" /uninstall");
                    k.SetValue("QuietUninstallString", "\"" + uninstallerPath + "\" /uninstall /S");
                    k.SetValue("NoModify", 1, RegistryValueKind.DWord);
                    k.SetValue("NoRepair", 1, RegistryValueKind.DWord);
                }
            }

            using (RegistryKey progId = root.CreateSubKey(@"Software\Classes\LinkcoPDFEditor.Document"))
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

            using (RegistryKey openWith = root.CreateSubKey(@"Software\Classes\.pdf\OpenWithProgids"))
            {
                if (openWith != null)
                    openWith.SetValue("LinkcoPDFEditor.Document", "");
            }

            using (RegistryKey cfg = root.CreateSubKey(LinkcoConfigKey))
            {
                if (cfg != null)
                {
                    cfg.SetValue("InstallDir", installDir);
                    string cliExe = Path.Combine(installDir, "pdfcraft-cli.exe");
                    if (File.Exists(cliExe))
                        cfg.SetValue("CliPath", cliExe);
                }
            }

            string previewDll = Path.Combine(installDir, "LinkcoPdfPreviewHandler.dll");
            if (File.Exists(previewDll))
            {
                try { RegisterPreviewHandler(Registry.CurrentUser, previewDll); } catch { }
                if (root != Registry.CurrentUser)
                {
                    try { RegisterPreviewHandler(root, previewDll); } catch { }
                }
                StopStalePrevHost();
            }

            try { SHChangeNotify(0x08000000, 0, IntPtr.Zero, IntPtr.Zero); } catch { }
        }

        private static void RegisterPreviewHandler(RegistryKey root, string dllPath)
        {
            string fullDllPath = Path.GetFullPath(dllPath);
            string dllDir = Path.GetDirectoryName(fullDllPath) ?? "";
            string cliPath = Path.Combine(dllDir, "pdfcraft-cli.exe");
            string codeBase = new Uri(fullDllPath).AbsoluteUri;

            using (RegistryKey cfg = root.CreateSubKey(LinkcoConfigKey))
            {
                if (cfg != null)
                {
                    if (!string.IsNullOrEmpty(dllDir))
                        cfg.SetValue("InstallDir", dllDir);
                    if (File.Exists(cliPath))
                        cfg.SetValue("CliPath", cliPath);
                    cfg.SetValue("PreviewHandlerDll", fullDllPath);
                }
            }

            using (RegistryKey clsidKey = root.CreateSubKey(@"Software\Classes\CLSID\" + PreviewHandlerClsid))
            {
                if (clsidKey != null)
                {
                    clsidKey.SetValue("", "Linkco PDF Preview Handler");
                    clsidKey.SetValue("DisplayName", "Linkco PDF Preview Handler");
                    clsidKey.SetValue("AppID", PrevHostAppId64);
                    clsidKey.SetValue("DisableLowILProcessIsolation", 1, RegistryValueKind.DWord);
                    using (RegistryKey inproc = clsidKey.CreateSubKey("InprocServer32"))
                    {
                        if (inproc != null)
                        {
                            inproc.SetValue("", "mscoree.dll");
                            inproc.SetValue("ThreadingModel", "STA");
                            inproc.SetValue("Class", "LinkcoPdfPreview.LinkcoPdfPreviewHandler");
                            inproc.SetValue("Assembly", "LinkcoPdfPreviewHandler, Version=0.5.0.0, Culture=neutral, PublicKeyToken=null");
                            inproc.SetValue("RuntimeVersion", "v4.0.30319");
                            inproc.SetValue("CodeBase", codeBase);
                        }
                    }
                    using (RegistryKey progId = clsidKey.CreateSubKey("ProgId"))
                    {
                        if (progId != null)
                            progId.SetValue("", "LinkcoPDFEditor.PreviewHandler");
                    }
                }
            }

            using (RegistryKey handlers = root.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers"))
            {
                if (handlers != null)
                    handlers.SetValue(PreviewHandlerClsid, "Linkco PDF Preview Handler");
            }

            List<string> progIds = new List<string>
            {
                "LinkcoPDFEditor.Document",
                "PdfCraft.Document",
                "MSEdgePDF",
                "MSEdgeHTM",
                "Acrobat.Document.DC",
                "AcroExch.Document.DC",
                "AcroExch.Document",
                "ChromeHTML"
            };

            try
            {
                using (RegistryKey uc = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice", false))
                {
                    if (uc != null)
                    {
                        string ucProgId = uc.GetValue("ProgId") as string;
                        if (!string.IsNullOrEmpty(ucProgId) && !progIds.Contains(ucProgId))
                            progIds.Add(ucProgId);
                    }
                }
            }
            catch { }

            foreach (string progId in progIds)
            {
                try
                {
                    BackupAndSetShellEx(
                        root,
                        @"Software\Classes\" + progId + @"\ShellEx\" + PreviewHandlerCategoryGuid,
                        "PrevProgId_" + progId
                    );
                }
                catch { }
            }

            BackupAndSetShellEx(root, @"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler");
            BackupAndSetShellEx(root, @"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler");
        }

        private static void StopStalePrevHost()
        {
            try
            {
                foreach (Process p in Process.GetProcessesByName("prevhost"))
                {
                    try { p.Kill(); } catch { }
                    finally { try { p.Dispose(); } catch { } }
                }
            }
            catch { }
        }

        private static void BackupAndSetShellEx(RegistryKey root, string subKeyPath, string backupValueName)
        {
            string existing = null;
            using (RegistryKey k = root.OpenSubKey(subKeyPath, false))
            {
                if (k != null)
                    existing = k.GetValue("") as string;
            }
            if (!string.IsNullOrEmpty(existing) && !string.Equals(existing, PreviewHandlerClsid, StringComparison.OrdinalIgnoreCase))
            {
                using (RegistryKey cfg = root.CreateSubKey(LinkcoConfigKey))
                {
                    if (cfg != null)
                        cfg.SetValue(backupValueName, existing);
                }
            }
            using (RegistryKey k = root.CreateSubKey(subKeyPath))
            {
                if (k != null)
                    k.SetValue("", PreviewHandlerClsid);
            }
        }

        private static void UnregisterPreviewHandler(RegistryKey root)
        {
            foreach (RegistryKey r in new RegistryKey[] { Registry.CurrentUser, root })
            {
                try
                {
                    using (RegistryKey handlers = r.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers", true))
                    {
                        if (handlers != null)
                            handlers.DeleteValue(PreviewHandlerClsid, false);
                    }
                    r.DeleteSubKeyTree(@"Software\Classes\CLSID\" + PreviewHandlerClsid, false);
                    r.DeleteSubKeyTree(@"Software\Classes\LinkcoPDFEditor.Document\ShellEx\" + PreviewHandlerCategoryGuid, false);
                    r.DeleteSubKeyTree(@"Software\Classes\PdfCraft.Document\ShellEx\" + PreviewHandlerCategoryGuid, false);

                    List<string> progIds = new List<string>
                    {
                        "MSEdgePDF",
                        "MSEdgeHTM",
                        "Acrobat.Document.DC",
                        "AcroExch.Document.DC",
                        "AcroExch.Document",
                        "ChromeHTML"
                    };
                    try
                    {
                        using (RegistryKey cfg = r.OpenSubKey(LinkcoConfigKey, false))
                        {
                            if (cfg != null)
                            {
                                foreach (string name in cfg.GetValueNames())
                                {
                                    if (name.StartsWith("PrevProgId_", StringComparison.Ordinal))
                                    {
                                        string p = name.Substring("PrevProgId_".Length);
                                        if (!string.IsNullOrEmpty(p) && !progIds.Contains(p))
                                            progIds.Add(p);
                                    }
                                }
                            }
                        }
                    }
                    catch { }

                    foreach (string progId in progIds)
                    {
                        try
                        {
                            RestoreOrRemoveShellEx(
                                r,
                                @"Software\Classes\" + progId + @"\ShellEx\" + PreviewHandlerCategoryGuid,
                                "PrevProgId_" + progId
                            );
                        }
                        catch { }
                    }

                    RestoreOrRemoveShellEx(r, @"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler");
                    RestoreOrRemoveShellEx(r, @"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler");
                }
                catch { }
            }
            StopStalePrevHost();
        }

        private static void RestoreOrRemoveShellEx(RegistryKey root, string subKeyPath, string backupValueName)
        {
            string current = null;
            using (RegistryKey k = root.OpenSubKey(subKeyPath, false))
            {
                if (k != null)
                    current = k.GetValue("") as string;
            }
            if (!string.Equals(current, PreviewHandlerClsid, StringComparison.OrdinalIgnoreCase))
                return;

            string backup = null;
            using (RegistryKey cfg = root.OpenSubKey(LinkcoConfigKey, true))
            {
                if (cfg != null)
                {
                    backup = cfg.GetValue(backupValueName) as string;
                    cfg.DeleteValue(backupValueName, false);
                }
            }

            string candidate = null;
            if (!string.IsNullOrEmpty(backup) && ClsidExists(backup))
                candidate = backup;
            else if (ClsidExists(EdgePreviewHandlerClsid))
                candidate = EdgePreviewHandlerClsid;

            if (!string.IsNullOrEmpty(candidate))
            {
                using (RegistryKey k = root.CreateSubKey(subKeyPath))
                {
                    if (k != null)
                        k.SetValue("", candidate);
                }
            }
            else
            {
                root.DeleteSubKeyTree(subKeyPath, false);
            }
        }

        private static bool ClsidExists(string clsid)
        {
            using (RegistryKey k = Registry.ClassesRoot.OpenSubKey(@"CLSID\" + clsid, false))
            {
                return k != null;
            }
        }

        private static int RunUninstall(bool silent, bool removeData)
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

                DialogResult keepData = MessageBox.Show(
                    "Keep your saved " + AppName + " preferences and recovery data?\n\nSelect 'Yes' (recommended) to preserve your settings and documents, or 'No' to remove application data.",
                    AppName + " Uninstall",
                    MessageBoxButtons.YesNo,
                    MessageBoxIcon.Question,
                    MessageBoxDefaultButton.Button1
                );
                if (keepData == DialogResult.No)
                    removeData = true;
            }

            try
            {
                string installDir = Path.GetDirectoryName(Assembly.GetExecutingAssembly().Location);
                foreach (Environment.SpecialFolder df in new Environment.SpecialFolder[] {
                    Environment.SpecialFolder.CommonDesktopDirectory,
                    Environment.SpecialFolder.DesktopDirectory
                })
                {
                    string folder = Environment.GetFolderPath(df);
                    if (!string.IsNullOrEmpty(folder))
                    {
                        string lnk = Path.Combine(folder, AppName + ".lnk");
                        if (File.Exists(lnk)) { try { File.Delete(lnk); } catch { } }
                    }
                }

                foreach (Environment.SpecialFolder pf in new Environment.SpecialFolder[] {
                    Environment.SpecialFolder.CommonPrograms,
                    Environment.SpecialFolder.Programs
                })
                {
                    string folder = Environment.GetFolderPath(pf);
                    if (!string.IsNullOrEmpty(folder))
                    {
                        string grp = Path.Combine(folder, AppName);
                        if (Directory.Exists(grp)) { try { Directory.Delete(grp, true); } catch { } }
                    }
                }

                foreach (RegistryKey root in new RegistryKey[] { Registry.LocalMachine, Registry.CurrentUser })
                {
                    try
                    {
                        UnregisterPreviewHandler(root);
                        root.DeleteSubKeyTree(UninstallKey, false);
                        root.DeleteSubKeyTree(@"Software\Classes\LinkcoPDFEditor.Document", false);
                        root.DeleteSubKeyTree(LinkcoConfigKey, false);
                        using (RegistryKey openWith = root.OpenSubKey(@"Software\Classes\.pdf\OpenWithProgids", true))
                        {
                            if (openWith != null)
                                openWith.DeleteValue("LinkcoPDFEditor.Document", false);
                        }
                    }
                    catch { }
                }

                if (removeData)
                {
                    foreach (Environment.SpecialFolder sf in new Environment.SpecialFolder[] {
                        Environment.SpecialFolder.ApplicationData,
                        Environment.SpecialFolder.LocalApplicationData
                    })
                    {
                        string baseDir = Environment.GetFolderPath(sf);
                        if (!string.IsNullOrEmpty(baseDir))
                        {
                            string appDataDir = Path.Combine(baseDir, AppName);
                            if (Directory.Exists(appDataDir))
                            {
                                try { Directory.Delete(appDataDir, true); } catch { }
                            }
                        }
                    }
                }

                try { SHChangeNotify(0x08000000, 0, IntPtr.Zero, IntPtr.Zero); } catch { }

                foreach (string f in new string[] { "LinkcoPDFEditor.exe", "pdfcraft.exe", "pdfcraft-cli.exe", "LinkcoPdfPreviewHandler.dll" })
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

            Panel headerBanner = new Panel
            {
                BackColor = Color.FromArgb(1, 19, 28),
                Location = new Point(0, 0),
                Size = new Size(520, 82)
            };
            Panel accentStrip = new Panel
            {
                BackColor = Color.FromArgb(242, 36, 36),
                Location = new Point(0, 79),
                Size = new Size(520, 3)
            };
            headerBanner.Controls.Add(accentStrip);

            Label title = new Label
            {
                Text = Program.AppName + "  (LINKCO)",
                ForeColor = Color.White,
                BackColor = Color.Transparent,
                Font = new Font("Segoe UI", 14.5f, FontStyle.Bold),
                AutoSize = true,
                Location = new Point(22, 14)
            };
            headerBanner.Controls.Add(title);

            Label sub = new Label
            {
                Text = Program.CompanyName + "\nDoha, State of Qatar  ·  C.R. No. 32942  ·  www.linkco.com.qa  ·  v" + Program.AppVersion,
                ForeColor = Color.FromArgb(203, 213, 225),
                BackColor = Color.Transparent,
                Font = new Font("Segoe UI", 8.5f, FontStyle.Regular),
                AutoSize = true,
                Location = new Point(24, 42)
            };
            headerBanner.Controls.Add(sub);
            Controls.Add(headerBanner);

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
            for exe_name in ("LinkcoPDFEditor.exe", "pdfcraft.exe", "pdfcraft-cli.exe", "LinkcoPdfPreviewHandler.dll"):
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
        <requestedExecutionLevel level="highestAvailable" uiAccess="false" />
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

    preview_dll_src = ROOT / "packaging" / "windows" / "PreviewHandler.cs"
    preview_dll_bin = bin_dir / "LinkcoPdfPreviewHandler.dll"
    csc_preview = find_csc()
    if not preview_dll_bin.is_file() and csc_preview and preview_dll_src.is_file():
        subprocess.run(
            [
                csc_preview,
                "/nologo",
                "/target:library",
                "/optimize+",
                "/platform:anycpu",
                f"/out:{preview_dll_bin}",
                "/r:System.dll",
                "/r:System.Drawing.dll",
                "/r:System.Windows.Forms.dll",
                str(preview_dll_src),
            ],
            cwd=str(ROOT),
            check=True,
        )
    if preview_dll_bin.is_file():
        shutil.copy2(preview_dll_bin, stage_dir / "LinkcoPdfPreviewHandler.dll")
        shutil.copy2(preview_dll_bin, out_dir / "LinkcoPdfPreviewHandler.dll")

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

    # Fetch OCR models into stage_dir/models (when cargo/network is available) so MSI and portable ZIP bundle them
    models_dir = stage_dir / "models"
    models_dir.mkdir(parents=True, exist_ok=True)
    existing_models = ROOT / "assets" / "models"
    if existing_models.is_dir() and any(existing_models.glob("*.rten")):
        for item in existing_models.iterdir():
            if item.is_file():
                shutil.copy2(item, models_dir / item.name)
    else:
        subprocess.run(
            ["cargo", "xtask", "models", str(models_dir)],
            cwd=str(ROOT),
            check=False,
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
                f"ModelsDir={models_dir}",
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
        for exe_name in ("LinkcoPDFEditor.exe", "pdfcraft.exe", "pdfcraft-cli.exe", "LinkcoPdfPreviewHandler.dll"):
            f = stage_dir / exe_name
            if f.is_file():
                zf.write(f, arcname=exe_name)
        portable_marker = ROOT / "packaging" / "windows" / "portable.txt"
        if portable_marker.is_file():
            zf.write(portable_marker, arcname="portable.txt")
        if models_dir.is_dir():
            for mf in sorted(models_dir.iterdir()):
                if mf.is_file() and not mf.name.endswith(".part"):
                    zf.write(mf, arcname=f"models/{mf.name}")
        for doc_name in ("README.md", "RELEASE_NOTES.md", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "ATTRIBUTION.md"):
            doc_path = ROOT / doc_name
            if doc_path.is_file():
                zf.write(doc_path, arcname=doc_name)
    generated.append(portable_zip)

    # 4. Stage release notes, licensing notices, and SHA256SUMS.txt in dist/release/
    for doc_name in ("RELEASE_NOTES.md", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE", "ATTRIBUTION.md"):
        src_doc = ROOT / doc_name
        if src_doc.is_file():
            dst_doc = out_dir / doc_name
            shutil.copy2(src_doc, dst_doc)
            if dst_doc not in generated:
                generated.append(dst_doc)

    checksums_path = out_dir / "SHA256SUMS.txt"
    checksum_lines: list[str] = []
    for entry in sorted(out_dir.iterdir(), key=lambda p: p.name):
        if entry.is_file() and entry.name != "SHA256SUMS.txt":
            digest = hashlib.sha256(entry.read_bytes()).hexdigest()
            checksum_lines.append(f"{digest}  {entry.name}")
    checksums_path.write_text("\n".join(checksum_lines) + "\n", encoding="utf-8")
    generated.append(checksums_path)

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
