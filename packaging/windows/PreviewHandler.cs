// Linkco PDF Editor — Windows File Explorer PDF Preview Handler (IPreviewHandler)
// Publisher: Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco — www.linkco.com.qa)
//
// Compiled into LinkcoPdfPreviewHandler.dll during Windows build/packaging using the OS-included
// .NET Framework 4.x compiler (C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe).
// Hosted out-of-process by Windows' standard Preview Host (prevhost.exe,
// AppID {6d2b5079-2f0b-48dd-ab7f-97cec514d30b}) and delegates single-page PDF rasterization to
// pdfcraft-cli.exe (`pdfcraft-cli preview <file.pdf> --page N --dpi 150 --out <temp.png>`),
// never opening the main Linkco PDF Editor window.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Windows.Forms;
using Microsoft.Win32;

[assembly: AssemblyTitle("Linkco PDF Preview Handler")]
[assembly: AssemblyDescription("Windows File Explorer PDF Preview Handler for Linkco PDF Editor")]
[assembly: AssemblyCompany("Al Rawabet Commercial Services & Contracting Company W.L.L.")]
[assembly: AssemblyProduct("Linkco PDF Editor")]
[assembly: AssemblyCopyright("© Al Rawabet Commercial Services & Contracting Company W.L.L.")]
[assembly: AssemblyVersion("0.5.0.0")]
[assembly: AssemblyFileVersion("0.5.0.0")]
[assembly: ComVisible(true)]

namespace LinkcoPdfPreview
{
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int left;
        public int top;
        public int right;
        public int bottom;

        public int Width { get { return right - left; } }
        public int Height { get { return bottom - top; } }
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct MSG
    {
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public int pt_x;
        public int pt_y;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct LOGFONT
    {
        public int lfHeight;
        public int lfWidth;
        public int lfEscapement;
        public int lfOrientation;
        public int lfWeight;
        public byte lfItalic;
        public byte lfUnderline;
        public byte lfStrikeOut;
        public byte lfCharSet;
        public byte lfOutPrecision;
        public byte lfClipPrecision;
        public byte lfQuality;
        public byte lfPitchAndFamily;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string lfFaceName;
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("8895b1c6-b41f-4c1c-a562-0d564250836f")]
    public interface IPreviewHandler
    {
        void SetWindow(IntPtr hwnd, ref RECT rect);
        void SetRect(ref RECT rect);
        void DoPreview();
        void Unload();
        void SetFocus();
        void QueryFocus(out IntPtr phwnd);
        [PreserveSig]
        int TranslateAccelerator(ref MSG pmsg);
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("196bf9a5-b346-4ef0-aa1e-5dcdb76768b1")]
    public interface IPreviewHandlerVisuals
    {
        void SetBackgroundColor(uint color);
        void SetFont(ref LOGFONT plf);
        void SetTextColor(uint color);
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("b7d14566-0509-4cce-a71f-0a554233bd9b")]
    public interface IInitializeWithFile
    {
        void Initialize([MarshalAs(UnmanagedType.LPWStr)] string pszFilePath, uint grfMode);
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("b824b49d-22ac-4161-ac8a-9916e8fa3f7f")]
    public interface IInitializeWithStream
    {
        void Initialize(IStream pstream, uint grfMode);
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("00000114-0000-0000-C000-000000000046")]
    public interface IOleWindow
    {
        void GetWindow(out IntPtr phwnd);
        void ContextSensitiveHelp([MarshalAs(UnmanagedType.Bool)] bool fEnterMode);
    }

    [ComImport]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    [Guid("fc4801a3-2ba9-11cf-a229-00aa003d7352")]
    public interface IObjectWithSite
    {
        void SetSite([MarshalAs(UnmanagedType.IUnknown)] object pUnkSite);
        void GetSite(ref Guid riid, [MarshalAs(UnmanagedType.IUnknown)] out object ppvSite);
    }

    internal static class NativeMethods
    {
        [DllImport("user32.dll", SetLastError = true)]
        public static extern IntPtr SetParent(IntPtr hWndChild, IntPtr hWndNewParent);

        [DllImport("user32.dll", SetLastError = true)]
        public static extern bool SetWindowPos(
            IntPtr hWnd,
            IntPtr hWndInsertAfter,
            int X,
            int Y,
            int cx,
            int cy,
            uint uFlags
        );

        [DllImport("user32.dll", SetLastError = true)]
        public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

        [DllImport("user32.dll", SetLastError = true)]
        public static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);

        [DllImport("user32.dll", SetLastError = true)]
        public static extern IntPtr GetFocus();

        [DllImport("shell32.dll")]
        public static extern void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);

        public const int GWL_STYLE = -16;
        public const int WS_CHILD = 0x40000000;
        public const int WS_VISIBLE = 0x10000000;
        public const int WS_CLIPCHILDREN = 0x02000000;
        public const int WS_CLIPSIBLINGS = 0x04000000;

        public const uint SWP_NOZORDER = 0x0004;
        public const uint SWP_NOACTIVATE = 0x0010;
        public const uint SWP_FRAMECHANGED = 0x0020;
        public const uint SWP_SHOWWINDOW = 0x0040;

        public const int SW_SHOW = 5;

        public const int SHCNE_ASSOCCHANGED = 0x08000000;
        public const uint SHCNF_IDLIST = 0x0000;
        public const int E_FAIL = unchecked((int)0x80004005);
        public const int S_FALSE = 1;

        public static string GetWritableTempDir()
        {
            List<string> candidates = new List<string>();
            try
            {
                string userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
                if (!string.IsNullOrEmpty(userProfile))
                {
                    candidates.Add(Path.Combine(userProfile, @"AppData\LocalLow\LinkcoPdfPreview"));
                }
            }
            catch { }

            try
            {
                string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                if (!string.IsNullOrEmpty(localAppData))
                {
                    string parent = Path.GetDirectoryName(localAppData);
                    if (!string.IsNullOrEmpty(parent))
                        candidates.Add(Path.Combine(parent, @"LocalLow\LinkcoPdfPreview"));
                }
            }
            catch { }

            try
            {
                string sysTemp = Path.GetTempPath();
                if (!string.IsNullOrEmpty(sysTemp))
                {
                    candidates.Add(Path.Combine(sysTemp, @"Low\LinkcoPdfPreview"));
                    candidates.Add(Path.Combine(sysTemp, "LinkcoPdfPreview"));
                }
            }
            catch { }

            foreach (string dir in candidates)
            {
                if (string.IsNullOrEmpty(dir))
                    continue;
                try
                {
                    Directory.CreateDirectory(dir);
                    string probe = Path.Combine(dir, ".probe-" + Guid.NewGuid().ToString("N"));
                    File.WriteAllBytes(probe, new byte[] { 1 });
                    File.Delete(probe);
                    return dir;
                }
                catch { }
            }

            return Path.GetTempPath();
        }
    }

    [ComVisible(true)]
    [ClassInterface(ClassInterfaceType.None)]
    [ProgId("LinkcoPDFEditor.PreviewHandler")]
    [Guid(LinkcoPdfPreviewHandler.ClsidString)]
    public sealed class LinkcoPdfPreviewHandler :
        IPreviewHandler,
        IPreviewHandlerVisuals,
        IInitializeWithFile,
        IInitializeWithStream,
        IOleWindow,
        IObjectWithSite
    {
        public const string ClsidString = "D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20";
        public const string ClsidBraced = "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}";
        public const string PreviewHandlerCategoryGuid = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
        public const string PrevHostAppId64 = "{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}";
        public const string PrevHostAppId32 = "{534A1E02-D58F-44f0-B58B-36CBED287C7C}";
        public const string EdgePreviewHandlerClsid = "{3A84F9C2-6164-485C-A7D9-4B27F8AC009E}";
        public const string LinkcoConfigKey = @"Software\Linkco\Linkco PDF Editor";

        private IntPtr _parentHwnd = IntPtr.Zero;
        private RECT _windowBounds;
        private string _filePath;
        private string _tempStreamPath;
        private string _initError;
        private bool _previewRequested;
        private object _unkSite;
        private PreviewPaneControl _control;

        public void Initialize(string pszFilePath, uint grfMode)
        {
            CleanupTempStreamFile();
            _initError = null;
            _filePath = pszFilePath;
            if (_previewRequested && _control != null && _parentHwnd != IntPtr.Zero)
            {
                _control.LoadDocument(_filePath, _initError);
            }
        }

        public void Initialize(IStream pstream, uint grfMode)
        {
            CleanupTempStreamFile();
            _initError = null;
            if (pstream == null)
            {
                _initError = "No PDF stream was provided by Windows Explorer.";
                return;
            }

            try
            {
                string tempDir = NativeMethods.GetWritableTempDir();
                string tempFile = Path.Combine(tempDir, "stream-" + Guid.NewGuid().ToString("N") + ".pdf");

                try
                {
                    pstream.Seek(0, 0, IntPtr.Zero);
                }
                catch { }

                using (FileStream fs = new FileStream(tempFile, FileMode.Create, FileAccess.Write, FileShare.Read))
                {
                    byte[] buffer = new byte[65536];
                    IntPtr bytesReadPtr = Marshal.AllocCoTaskMem(sizeof(long));
                    try
                    {
                        while (true)
                        {
                            Marshal.WriteInt64(bytesReadPtr, 0);
                            pstream.Read(buffer, buffer.Length, bytesReadPtr);
                            int read = Marshal.ReadInt32(bytesReadPtr);
                            if (read <= 0)
                                break;
                            fs.Write(buffer, 0, read);
                        }
                    }
                    finally
                    {
                        Marshal.FreeCoTaskMem(bytesReadPtr);
                    }
                }

                _tempStreamPath = tempFile;
                _filePath = tempFile;
            }
            catch (Exception ex)
            {
                _initError = "Could not read PDF stream for preview: " + ex.Message;
                CleanupTempStreamFile();
            }

            if (_previewRequested && _control != null && _parentHwnd != IntPtr.Zero)
            {
                _control.LoadDocument(_filePath, _initError);
            }
        }

        public void SetWindow(IntPtr hwnd, ref RECT rect)
        {
            _parentHwnd = hwnd;
            _windowBounds = rect;
            if (_parentHwnd != IntPtr.Zero)
            {
                EnsureControlAttached();
                UpdateBounds();
                if (_previewRequested && (!string.IsNullOrEmpty(_filePath) || !string.IsNullOrEmpty(_initError)))
                {
                    _control.LoadDocument(_filePath, _initError);
                }
            }
        }

        public void SetRect(ref RECT rect)
        {
            _windowBounds = rect;
            UpdateBounds();
        }

        public void DoPreview()
        {
            _previewRequested = true;
            if (_parentHwnd == IntPtr.Zero)
                return;

            EnsureControlAttached();
            UpdateBounds();
            _control.LoadDocument(_filePath, _initError);
        }

        public void Unload()
        {
            _previewRequested = false;
            _initError = null;
            if (_control != null)
            {
                try
                {
                    _control.CancelAndClear();
                    _control.Visible = false;
                    _control.Dispose();
                }
                catch { }
                _control = null;
            }
            _filePath = null;
            CleanupTempStreamFile();
        }

        public void SetFocus()
        {
            if (_control != null)
            {
                try { _control.Focus(); } catch { }
            }
        }

        public void QueryFocus(out IntPtr phwnd)
        {
            phwnd = NativeMethods.GetFocus();
        }

        public int TranslateAccelerator(ref MSG pmsg)
        {
            return NativeMethods.S_FALSE;
        }

        public void SetBackgroundColor(uint color)
        {
            if (_control != null)
            {
                int r = (int)(color & 0xFF);
                int g = (int)((color >> 8) & 0xFF);
                int b = (int)((color >> 16) & 0xFF);
                _control.SetThemeBackground(Color.FromArgb(r, g, b));
            }
        }

        public void SetFont(ref LOGFONT plf) { }

        public void SetTextColor(uint color) { }

        public void GetWindow(out IntPtr phwnd)
        {
            phwnd = _control != null ? _control.Handle : _parentHwnd;
        }

        public void ContextSensitiveHelp(bool fEnterMode) { }

        public void SetSite(object pUnkSite)
        {
            _unkSite = pUnkSite;
        }

        public void GetSite(ref Guid riid, out object ppvSite)
        {
            if (_unkSite == null)
            {
                ppvSite = null;
                Marshal.ThrowExceptionForHR(NativeMethods.E_FAIL);
                return;
            }
            IntPtr punk = Marshal.GetIUnknownForObject(_unkSite);
            IntPtr ppv;
            int hr = Marshal.QueryInterface(punk, ref riid, out ppv);
            Marshal.Release(punk);
            if (hr != 0)
            {
                ppvSite = null;
                Marshal.ThrowExceptionForHR(hr);
                return;
            }
            ppvSite = Marshal.GetObjectForIUnknown(ppv);
            Marshal.Release(ppv);
        }

        private void EnsureControlAttached()
        {
            if (_parentHwnd == IntPtr.Zero)
                return;

            if (_control == null)
            {
                _control = new PreviewPaneControl();
                _control.CreateControl();
            }

            IntPtr handle = _control.Handle;
            NativeMethods.SetWindowLong(
                handle,
                NativeMethods.GWL_STYLE,
                NativeMethods.WS_CHILD | NativeMethods.WS_VISIBLE | NativeMethods.WS_CLIPCHILDREN | NativeMethods.WS_CLIPSIBLINGS
            );
            NativeMethods.SetParent(handle, _parentHwnd);
            NativeMethods.ShowWindow(handle, NativeMethods.SW_SHOW);
            _control.Visible = true;
        }

        private void UpdateBounds()
        {
            if (_control != null && _parentHwnd != IntPtr.Zero)
            {
                int w = Math.Max(10, _windowBounds.Width);
                int h = Math.Max(10, _windowBounds.Height);
                IntPtr handle = _control.Handle;
                NativeMethods.SetWindowPos(
                    handle,
                    IntPtr.Zero,
                    _windowBounds.left,
                    _windowBounds.top,
                    w,
                    h,
                    NativeMethods.SWP_NOZORDER | NativeMethods.SWP_NOACTIVATE | NativeMethods.SWP_SHOWWINDOW | NativeMethods.SWP_FRAMECHANGED
                );
                _control.Bounds = new Rectangle(_windowBounds.left, _windowBounds.top, w, h);
                _control.Invalidate();
                _control.Update();
            }
        }

        private void CleanupTempStreamFile()
        {
            if (!string.IsNullOrEmpty(_tempStreamPath))
            {
                try
                {
                    if (File.Exists(_tempStreamPath))
                        File.Delete(_tempStreamPath);
                }
                catch { }
                _tempStreamPath = null;
            }
        }

        [ComRegisterFunction]
        public static void Register(Type t)
        {
            if (t == null || t != typeof(LinkcoPdfPreviewHandler))
                return;
            string dllPath = Assembly.GetExecutingAssembly().Location;
            RegisterPreviewHandler(dllPath);
        }

        [ComUnregisterFunction]
        public static void Unregister(Type t)
        {
            if (t == null || t != typeof(LinkcoPdfPreviewHandler))
                return;
            UnregisterPreviewHandler();
        }

        public static void RegisterPreviewHandler(string dllPath)
        {
            string fullDllPath = Path.GetFullPath(dllPath);
            string dllDir = Path.GetDirectoryName(fullDllPath) ?? "";
            string cliPath = Path.Combine(dllDir, "pdfcraft-cli.exe");
            string expectedCodeBase = new Uri(fullDllPath).AbsoluteUri;
            bool wasAlreadyCurrent = false;

            try
            {
                using (RegistryKey inproc = Registry.CurrentUser.OpenSubKey(@"Software\Classes\CLSID\" + ClsidBraced + @"\InprocServer32", false))
                using (RegistryKey dotPdf = Registry.CurrentUser.OpenSubKey(@"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, false))
                {
                    if (inproc != null && dotPdf != null)
                    {
                        string cb = inproc.GetValue("CodeBase") as string;
                        string sh = dotPdf.GetValue("") as string;
                        wasAlreadyCurrent =
                            string.Equals(cb, expectedCodeBase, StringComparison.OrdinalIgnoreCase) &&
                            string.Equals(sh, ClsidBraced, StringComparison.OrdinalIgnoreCase);
                    }
                }
            }
            catch { }

            // Always register in HKEY_CURRENT_USER (highest precedence in HKEY_CLASSES_ROOT and
            // does not require Administrator privileges).
            try
            {
                RegisterInRoot(Registry.CurrentUser, fullDllPath, dllDir, cliPath);
            }
            catch { }

            // Best-effort machine-wide registration when elevated as Administrator.
            try
            {
                RegisterInRoot(Registry.LocalMachine, fullDllPath, dllDir, cliPath);
            }
            catch { }

            if (!wasAlreadyCurrent)
            {
                StopStalePrevHost();
            }
            try
            {
                NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
            }
            catch { }
        }

        private static void RegisterInRoot(RegistryKey root, string fullDllPath, string dllDir, string cliPath)
        {
            string codeBase = new Uri(fullDllPath).AbsoluteUri;
            string asmFullName = typeof(LinkcoPdfPreviewHandler).Assembly.FullName;
            string appId = Environment.Is64BitOperatingSystem ? PrevHostAppId64 : PrevHostAppId32;

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

            using (RegistryKey clsidKey = root.CreateSubKey(@"Software\Classes\CLSID\" + ClsidBraced))
            {
                if (clsidKey != null)
                {
                    clsidKey.SetValue("", "Linkco PDF Preview Handler");
                    clsidKey.SetValue("DisplayName", "Linkco PDF Preview Handler");
                    clsidKey.SetValue("AppID", appId);
                    clsidKey.SetValue("DisableLowILProcessIsolation", 1, RegistryValueKind.DWord);

                    using (RegistryKey inproc = clsidKey.CreateSubKey("InprocServer32"))
                    {
                        if (inproc != null)
                        {
                            inproc.SetValue("", "mscoree.dll");
                            inproc.SetValue("ThreadingModel", "STA");
                            inproc.SetValue("Class", typeof(LinkcoPdfPreviewHandler).FullName);
                            inproc.SetValue("Assembly", asmFullName);
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
                    handlers.SetValue(ClsidBraced, "Linkco PDF Preview Handler");
            }

            // Collect all ProgIDs that may own .pdf on Windows 10/11 (including UserChoice ProgId
            // such as MSEdgePDF, Acrobat, ChromeHTML, etc.) so HKEY_CLASSES_ROOT\<ProgId>\ShellEx
            // resolves to Linkco's preview handler.
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

            try
            {
                using (RegistryKey dotPdf = Registry.ClassesRoot.OpenSubKey(".pdf", false))
                {
                    if (dotPdf != null)
                    {
                        string defProgId = dotPdf.GetValue("") as string;
                        if (!string.IsNullOrEmpty(defProgId) && !progIds.Contains(defProgId))
                            progIds.Add(defProgId);
                    }
                }
            }
            catch { }

            try
            {
                using (RegistryKey owp = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\OpenWithProgids", false))
                {
                    if (owp != null)
                    {
                        foreach (string name in owp.GetValueNames())
                        {
                            if (!string.IsNullOrEmpty(name) && !progIds.Contains(name))
                                progIds.Add(name);
                        }
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

            try
            {
                BackupAndSetShellEx(root, @"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler");
            }
            catch { }

            try
            {
                BackupAndSetShellEx(root, @"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler");
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
            if (!string.IsNullOrEmpty(existing) && !string.Equals(existing, ClsidBraced, StringComparison.OrdinalIgnoreCase))
            {
                using (RegistryKey cfg = root.CreateSubKey(LinkcoConfigKey))
                {
                    if (cfg != null && cfg.GetValue(backupValueName) == null)
                        cfg.SetValue(backupValueName, existing);
                }
            }
            using (RegistryKey k = root.CreateSubKey(subKeyPath))
            {
                if (k != null)
                    k.SetValue("", ClsidBraced);
            }
        }

        public static void UnregisterPreviewHandler()
        {
            try { UnregisterFromRoot(Registry.CurrentUser); } catch { }
            try { UnregisterFromRoot(Registry.LocalMachine); } catch { }
            StopStalePrevHost();
            try
            {
                NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
            }
            catch { }
        }

        private static void UnregisterFromRoot(RegistryKey root)
        {
            using (RegistryKey handlers = root.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers", true))
            {
                if (handlers != null)
                    handlers.DeleteValue(ClsidBraced, false);
            }

            root.DeleteSubKeyTree(@"Software\Classes\CLSID\" + ClsidBraced, false);
            root.DeleteSubKeyTree(@"Software\Classes\LinkcoPDFEditor.Document\ShellEx\" + PreviewHandlerCategoryGuid, false);
            root.DeleteSubKeyTree(@"Software\Classes\PdfCraft.Document\ShellEx\" + PreviewHandlerCategoryGuid, false);

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
                using (RegistryKey cfg = root.OpenSubKey(LinkcoConfigKey, false))
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
                        root,
                        @"Software\Classes\" + progId + @"\ShellEx\" + PreviewHandlerCategoryGuid,
                        "PrevProgId_" + progId
                    );
                }
                catch { }
            }

            RestoreOrRemoveShellEx(root, @"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler");
            RestoreOrRemoveShellEx(root, @"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler");
        }

        private static void RestoreOrRemoveShellEx(RegistryKey root, string subKeyPath, string backupValueName)
        {
            string current = null;
            using (RegistryKey k = root.OpenSubKey(subKeyPath, false))
            {
                if (k != null)
                    current = k.GetValue("") as string;
            }
            if (!string.Equals(current, ClsidBraced, StringComparison.OrdinalIgnoreCase))
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

            if (!string.IsNullOrEmpty(backup) && ClsidExists(backup))
            {
                using (RegistryKey k = root.CreateSubKey(subKeyPath))
                {
                    if (k != null)
                        k.SetValue("", backup);
                }
            }
            else if (root == Registry.LocalMachine && ClsidExists(EdgePreviewHandlerClsid))
            {
                using (RegistryKey k = root.CreateSubKey(subKeyPath))
                {
                    if (k != null)
                        k.SetValue("", EdgePreviewHandlerClsid);
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

        private static void StopStalePrevHost()
        {
            try
            {
                int selfPid = Process.GetCurrentProcess().Id;
                foreach (Process p in Process.GetProcessesByName("prevhost"))
                {
                    try
                    {
                        if (p.Id != selfPid)
                            p.Kill();
                    }
                    catch { }
                    finally
                    {
                        try { p.Dispose(); } catch { }
                    }
                }
            }
            catch { }
        }
    }

    internal sealed class PreviewPaneControl : Control
    {
        private const int TopBarHeight = 36;

        private readonly Rectangle _prevRect = new Rectangle(8, 5, 28, 25);
        private readonly Rectangle _pageLabelRect = new Rectangle(40, 5, 134, 25);
        private readonly Rectangle _nextRect = new Rectangle(178, 5, 28, 25);
        private readonly Rectangle _zoomOutRect = new Rectangle(216, 5, 28, 25);
        private readonly Rectangle _zoomFitRect = new Rectangle(248, 5, 38, 25);
        private readonly Rectangle _zoomInRect = new Rectangle(290, 5, 28, 25);

        private Color _canvasBackColor = Color.FromArgb(242, 243, 245);
        private string _pdfPath;
        private int _currentPage = 1;
        private int _totalPages = 0;
        private float _zoomFactor = 1.0f;
        private bool _fitToWidth = true;
        private int _scrollY = 0;
        private bool _isDragging = false;
        private Point _dragStart;
        private int _dragStartScrollY = 0;
        private Bitmap _pageBitmap;
        private string _statusMessage = "Select a PDF document to preview.";

        public PreviewPaneControl()
        {
            DoubleBuffered = true;
            SetStyle(
                ControlStyles.AllPaintingInWmPaint |
                ControlStyles.UserPaint |
                ControlStyles.OptimizedDoubleBuffer |
                ControlStyles.ResizeRedraw |
                ControlStyles.Selectable,
                true
            );
            BackColor = _canvasBackColor;
            TabStop = true;
        }

        public void SetThemeBackground(Color c)
        {
            _canvasBackColor = c;
            BackColor = c;
            Invalidate();
        }

        public void LoadDocument(string filePath, string initError)
        {
            _pdfPath = filePath;
            _currentPage = 1;
            _totalPages = 0;
            _fitToWidth = true;
            _zoomFactor = 1.0f;
            _scrollY = 0;

            if (!string.IsNullOrEmpty(initError))
            {
                ShowStatus(initError);
                return;
            }

            if (string.IsNullOrEmpty(filePath) || !File.Exists(filePath))
            {
                ShowStatus("Unable to preview: the selected PDF file could not be found.");
                return;
            }

            RenderCurrentPageSync();
        }

        public void CancelAndClear()
        {
            ClearImage();
        }

        private void ChangePage(int newPage)
        {
            if (_totalPages <= 0)
                return;
            int clamped = Math.Max(1, Math.Min(_totalPages, newPage));
            if (clamped == _currentPage)
                return;
            _currentPage = clamped;
            _scrollY = 0;
            RenderCurrentPageSync();
        }

        private void RenderCurrentPageSync()
        {
            string path = _pdfPath;
            int page = _currentPage;
            ShowStatus("Loading page " + page + "...");

            string cliExe = LocateCliExecutable();
            if (string.IsNullOrEmpty(cliExe) || !File.Exists(cliExe))
            {
                ShowStatus("Linkco PDF Preview engine (pdfcraft-cli.exe) was not found.");
                return;
            }

            string tempDir = NativeMethods.GetWritableTempDir();
            string outPng = Path.Combine(tempDir, "page-" + Process.GetCurrentProcess().Id + "-" + Guid.NewGuid().ToString("N") + ".png");

            try
            {
                ProcessStartInfo psi = new ProcessStartInfo
                {
                    FileName = cliExe,
                    Arguments = "preview \"" + path + "\" --page " + page + " --dpi 150 --out \"" + outPng + "\"",
                    CreateNoWindow = true,
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true
                };

                using (Process proc = Process.Start(psi))
                {
                    string stdout = proc.StandardOutput.ReadToEnd();
                    string stderr = proc.StandardError.ReadToEnd();
                    proc.WaitForExit(15000);
                    ParseAndDeliver(stdout, stderr, outPng, page);
                }
            }
            catch (Exception ex)
            {
                ShowStatus("Preview failed: " + ex.Message);
            }
            finally
            {
                try
                {
                    if (File.Exists(outPng))
                        File.Delete(outPng);
                }
                catch { }
            }
        }

        private void ParseAndDeliver(string stdout, string stderr, string outPng, int requestedPage)
        {
            string statusLine = null;
            using (StringReader sr = new StringReader(stdout ?? ""))
            {
                string line;
                while ((line = sr.ReadLine()) != null)
                {
                    if (line.StartsWith("STATUS\t", StringComparison.Ordinal))
                    {
                        statusLine = line;
                        break;
                    }
                }
            }

            if (statusLine != null)
            {
                string[] parts = statusLine.Split('\t');
                string code = parts.Length > 1 ? parts[1] : "";
                if (code == "OK" && File.Exists(outPng))
                {
                    int total = parts.Length > 2 ? ParseInt(parts[2], 1) : 1;
                    int actualPage = parts.Length > 3 ? ParseInt(parts[3], requestedPage) : requestedPage;
                    byte[] pngBytes = File.ReadAllBytes(outPng);
                    using (MemoryStream ms = new MemoryStream(pngBytes))
                    using (Image tmp = Image.FromStream(ms))
                    {
                        Bitmap bmp = new Bitmap(tmp);
                        ClearImage();
                        _pageBitmap = bmp;
                        _totalPages = total;
                        _currentPage = actualPage;
                        _statusMessage = null;
                        ClampScroll();
                        Invalidate();
                        Update();
                        return;
                    }
                }
                else if (code == "PASSWORD_REQUIRED")
                {
                    string msg = parts.Length > 3 ? parts[3] : "This PDF document is password-protected.";
                    ShowStatus(msg);
                    return;
                }
                else if (code == "EMPTY_PDF")
                {
                    ShowStatus("This PDF document contains no pages.");
                    return;
                }
                else
                {
                    string msg = parts.Length > 3 ? parts[3] : "Unable to preview this PDF document.";
                    ShowStatus("Unable to preview PDF: " + msg);
                    return;
                }
            }

            string errText = !string.IsNullOrEmpty(stderr) ? stderr.Trim() : "The file may be damaged or unsupported.";
            ShowStatus("Unable to preview PDF: " + errText);
        }

        private static int ParseInt(string s, int fallback)
        {
            int v;
            return int.TryParse(s, out v) ? v : fallback;
        }

        private void ShowStatus(string message)
        {
            ClearImage();
            _totalPages = 0;
            _statusMessage = message;
            Invalidate();
            Update();
        }

        private void ClearImage()
        {
            if (_pageBitmap != null)
            {
                Bitmap old = _pageBitmap;
                _pageBitmap = null;
                old.Dispose();
            }
        }

        private int GetTargetPageHeight(out int targetW)
        {
            int canvasW = Math.Max(60, ClientSize.Width - 24);
            if (_pageBitmap == null || _pageBitmap.Width <= 0 || _pageBitmap.Height <= 0)
            {
                targetW = canvasW;
                return 0;
            }
            targetW = _fitToWidth ? canvasW : Math.Max(60, (int)Math.Round(canvasW * _zoomFactor));
            return (int)Math.Round((double)_pageBitmap.Height * targetW / _pageBitmap.Width);
        }

        private void ClampScroll()
        {
            int targetW;
            int targetH = GetTargetPageHeight(out targetW);
            int viewH = Math.Max(10, ClientSize.Height - TopBarHeight - 24);
            int maxScroll = Math.Max(0, targetH - viewH);
            if (_scrollY > maxScroll)
                _scrollY = maxScroll;
            if (_scrollY < 0)
                _scrollY = 0;
        }

        protected override void OnResize(EventArgs e)
        {
            base.OnResize(e);
            ClampScroll();
            Invalidate();
        }

        protected override void OnMouseDown(MouseEventArgs e)
        {
            base.OnMouseDown(e);
            try { Focus(); } catch { }

            if (e.Button != MouseButtons.Left)
                return;

            Point pt = e.Location;
            if (_prevRect.Contains(pt))
            {
                ChangePage(_currentPage - 1);
                return;
            }
            if (_nextRect.Contains(pt))
            {
                ChangePage(_currentPage + 1);
                return;
            }
            if (_zoomOutRect.Contains(pt))
            {
                _fitToWidth = false;
                _zoomFactor = Math.Max(0.4f, _zoomFactor - 0.2f);
                ClampScroll();
                Invalidate();
                Update();
                return;
            }
            if (_zoomFitRect.Contains(pt))
            {
                _fitToWidth = true;
                _zoomFactor = 1.0f;
                _scrollY = 0;
                Invalidate();
                Update();
                return;
            }
            if (_zoomInRect.Contains(pt))
            {
                _fitToWidth = false;
                _zoomFactor = Math.Min(3.0f, _zoomFactor + 0.2f);
                ClampScroll();
                Invalidate();
                Update();
                return;
            }

            if (pt.Y > TopBarHeight && _pageBitmap != null)
            {
                _isDragging = true;
                _dragStart = pt;
                _dragStartScrollY = _scrollY;
            }
        }

        protected override void OnMouseMove(MouseEventArgs e)
        {
            base.OnMouseMove(e);
            if (_isDragging && e.Button == MouseButtons.Left)
            {
                int dy = e.Y - _dragStart.Y;
                _scrollY = _dragStartScrollY - dy;
                ClampScroll();
                Invalidate();
                Update();
            }
        }

        protected override void OnMouseUp(MouseEventArgs e)
        {
            base.OnMouseUp(e);
            _isDragging = false;
        }

        protected override void OnMouseWheel(MouseEventArgs e)
        {
            base.OnMouseWheel(e);
            if (_pageBitmap == null)
                return;

            int step = 64;
            int delta = -(e.Delta / 120) * step;
            int targetW;
            int targetH = GetTargetPageHeight(out targetW);
            int viewH = Math.Max(10, ClientSize.Height - TopBarHeight - 24);
            int maxScroll = Math.Max(0, targetH - viewH);

            if (delta > 0 && _scrollY >= maxScroll && _currentPage < _totalPages)
            {
                ChangePage(_currentPage + 1);
                return;
            }
            if (delta < 0 && _scrollY <= 0 && _currentPage > 1)
            {
                ChangePage(_currentPage - 1);
                return;
            }

            _scrollY += delta;
            ClampScroll();
            Invalidate();
            Update();
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            Graphics g = e.Graphics;
            g.SmoothingMode = SmoothingMode.AntiAlias;
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;

            Rectangle client = ClientRectangle;
            using (SolidBrush bgBrush = new SolidBrush(_canvasBackColor))
            {
                g.FillRectangle(bgBrush, client);
            }

            Rectangle canvasRect = new Rectangle(0, TopBarHeight, client.Width, Math.Max(1, client.Height - TopBarHeight));
            g.SetClip(canvasRect);

            if (_pageBitmap != null && _pageBitmap.Width > 0 && _pageBitmap.Height > 0)
            {
                int targetW;
                int targetH = GetTargetPageHeight(out targetW);
                int x = Math.Max(12, (client.Width - targetW) / 2);
                int y = TopBarHeight + 12 - _scrollY;

                Rectangle shadowRect = new Rectangle(x + 2, y + 3, targetW, targetH);
                using (SolidBrush shadowBrush = new SolidBrush(Color.FromArgb(45, 0, 0, 0)))
                {
                    g.FillRectangle(shadowBrush, shadowRect);
                }

                Rectangle pageRect = new Rectangle(x, y, targetW, targetH);
                g.FillRectangle(Brushes.White, pageRect);
                g.DrawImage(_pageBitmap, pageRect);
                using (Pen borderPen = new Pen(Color.FromArgb(200, 205, 212), 1f))
                {
                    g.DrawRectangle(borderPen, pageRect);
                }
            }
            else
            {
                string msg = !string.IsNullOrEmpty(_statusMessage) ? _statusMessage : "Select a PDF document to preview.";
                Rectangle textRect = new Rectangle(20, TopBarHeight + 20, Math.Max(40, client.Width - 40), Math.Max(40, canvasRect.Height - 40));
                using (Font statusFont = new Font("Segoe UI", 9.5f, FontStyle.Regular))
                using (StringFormat sf = new StringFormat { Alignment = StringAlignment.Center, LineAlignment = StringAlignment.Center })
                using (SolidBrush textBrush = new SolidBrush(Color.FromArgb(80, 86, 96)))
                {
                    g.DrawString(msg, statusFont, textBrush, textRect, sf);
                }
            }

            g.ResetClip();

            // Draw Linkco top toolbar
            Rectangle topRect = new Rectangle(0, 0, client.Width, TopBarHeight);
            using (SolidBrush navBg = new SolidBrush(Color.FromArgb(1, 19, 28)))
            {
                g.FillRectangle(navBg, topRect);
            }
            using (Pen accentPen = new Pen(Color.FromArgb(242, 36, 36), 2f))
            {
                g.DrawLine(accentPen, 0, TopBarHeight - 1, client.Width, TopBarHeight - 1);
            }

            DrawNavButton(g, _prevRect, "‹", _currentPage > 1 && _totalPages > 0);
            DrawNavButton(g, _nextRect, "›", _currentPage < _totalPages && _totalPages > 0);
            DrawNavButton(g, _zoomOutRect, "−", _pageBitmap != null);
            DrawNavButton(g, _zoomFitRect, "Fit", _pageBitmap != null);
            DrawNavButton(g, _zoomInRect, "+", _pageBitmap != null);

            string pageText = (_totalPages > 0)
                ? ("Page " + _currentPage + " of " + _totalPages)
                : "Linkco PDF Preview";
            using (Font labelFont = new Font("Segoe UI", 9f, FontStyle.Bold))
            using (StringFormat sf = new StringFormat { Alignment = StringAlignment.Center, LineAlignment = StringAlignment.Center })
            {
                g.DrawString(pageText, labelFont, Brushes.White, _pageLabelRect, sf);
            }
        }

        private static void DrawNavButton(Graphics g, Rectangle r, string text, bool enabled)
        {
            Color fill = enabled ? Color.FromArgb(18, 38, 54) : Color.FromArgb(10, 24, 34);
            Color border = enabled ? Color.FromArgb(242, 36, 36) : Color.FromArgb(70, 80, 90);
            Color fg = enabled ? Color.White : Color.FromArgb(120, 130, 140);

            using (SolidBrush b = new SolidBrush(fill))
            {
                g.FillRectangle(b, r);
            }
            using (Pen p = new Pen(border, 1f))
            {
                g.DrawRectangle(p, r);
            }
            using (Font f = new Font("Segoe UI", 8.5f, FontStyle.Bold))
            using (SolidBrush tb = new SolidBrush(fg))
            using (StringFormat sf = new StringFormat { Alignment = StringAlignment.Center, LineAlignment = StringAlignment.Center })
            {
                g.DrawString(text, f, tb, r, sf);
            }
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                ClearImage();
            }
            base.Dispose(disposing);
        }

        private static string LocateCliExecutable()
        {
            try
            {
                string asmLoc = Assembly.GetExecutingAssembly().Location;
                if (!string.IsNullOrEmpty(asmLoc))
                {
                    string asmDir = Path.GetDirectoryName(asmLoc);
                    if (!string.IsNullOrEmpty(asmDir))
                    {
                        string candidate = Path.Combine(asmDir, "pdfcraft-cli.exe");
                        if (File.Exists(candidate))
                            return candidate;
                    }
                }
            }
            catch { }

            try
            {
                string codeBase = Assembly.GetExecutingAssembly().CodeBase;
                if (!string.IsNullOrEmpty(codeBase))
                {
                    string localPath = new Uri(codeBase).LocalPath;
                    string cbDir = Path.GetDirectoryName(localPath);
                    if (!string.IsNullOrEmpty(cbDir))
                    {
                        string candidate = Path.Combine(cbDir, "pdfcraft-cli.exe");
                        if (File.Exists(candidate))
                            return candidate;
                    }
                }
            }
            catch { }

            foreach (RegistryKey root in new RegistryKey[] { Registry.CurrentUser, Registry.LocalMachine })
            {
                try
                {
                    using (RegistryKey k = root.OpenSubKey(LinkcoPdfPreviewHandler.LinkcoConfigKey, false))
                    {
                        if (k != null)
                        {
                            string cliPath = k.GetValue("CliPath") as string;
                            if (!string.IsNullOrEmpty(cliPath) && File.Exists(cliPath))
                                return cliPath;

                            string installDir = k.GetValue("InstallDir") as string;
                            if (!string.IsNullOrEmpty(installDir))
                            {
                                string candidate = Path.Combine(installDir, "pdfcraft-cli.exe");
                                if (File.Exists(candidate))
                                    return candidate;
                            }
                        }
                    }
                }
                catch { }
            }

            try
            {
                string localPrograms = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                    @"Programs\Linkco PDF Editor\pdfcraft-cli.exe"
                );
                if (File.Exists(localPrograms))
                    return localPrograms;
            }
            catch { }

            try
            {
                string progFiles = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
                    @"Linkco PDF Editor\pdfcraft-cli.exe"
                );
                if (File.Exists(progFiles))
                    return progFiles;
            }
            catch { }

            return null;
        }
    }
}
