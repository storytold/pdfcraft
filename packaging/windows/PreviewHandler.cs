// Linkco PDF Editor — Windows File Explorer PDF Preview Handler (IPreviewHandler)
// Publisher: Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco — www.linkco.com.qa)
//
// Compiled into LinkcoPdfPreviewHandler.dll during the Windows build/packaging with the OS-included
// .NET Framework 4.x compiler (C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe, C# 5):
//
//   csc /nologo /target:library /optimize+ /platform:anycpu /out:LinkcoPdfPreviewHandler.dll
//       /r:System.dll /r:System.Drawing.dll /r:System.Windows.Forms.dll PreviewHandler.cs
//
// Hosted out-of-process by Windows' standard Preview Host (prevhost.exe, AppID
// {6d2b5079-2f0b-48dd-ab7f-97cec514d30b}). Page rasterization is delegated to pdfcraft-cli.exe,
// which ships beside this DLL:
//
//   pdfcraft-cli preview <file.pdf> --page N --width PX --max-px 4096 --out <temp.png>
//
// so a damaged or hostile PDF can only ever crash or hang a short-lived child process (which is
// killed after a timeout), never Explorer or the preview host. The main Linkco PDF Editor window is
// never opened.
//
// Production notes:
//  * Rendering runs on a worker thread; the preview pane never blocks while a page renders.
//  * Selecting another file, paging quickly or closing the pane cancels (kills) the render in flight.
//  * The raster is sized to the pane width × zoom (DPI-aware) and re-rendered when the pane grows.
//  * The Explorer-provided stream is copied to a private temp file and released immediately, so the
//    previewed file is never kept locked (rename/delete/move keep working while it is previewed).
//  * Registration is per-user (HKCU, no administrator rights needed) and optionally machine-wide
//    (HKLM); it never touches ProgIDs shared with non-PDF file types (e.g. browsers' HTML ProgIDs).
//  * Diagnostics: %USERPROFILE%\AppData\LocalLow\LinkcoPdfPreview\preview.log (size-capped), and
//    [LinkcoPdfPreview.LinkcoPdfPreviewHandler]::Diagnose() from PowerShell.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;
using System.Threading;
using System.Windows.Forms;
using Microsoft.Win32;

[assembly: AssemblyTitle("Linkco PDF Preview Handler")]
[assembly: AssemblyDescription("Windows File Explorer PDF Preview Handler for Linkco PDF Editor")]
[assembly: AssemblyCompany("Al Rawabet Commercial Services & Contracting Company W.L.L.")]
[assembly: AssemblyProduct("Linkco PDF Editor")]
[assembly: AssemblyCopyright("\u00A9 Al Rawabet Commercial Services & Contracting Company W.L.L.")]
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

    [StructLayout(LayoutKind.Sequential)]
    public struct PREVIEWHANDLERFRAMEINFO
    {
        public IntPtr haccel;
        public uint cAccelEntries;
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
    [Guid("fec87aaf-35f9-447a-adb7-20234491401a")]
    public interface IPreviewHandlerFrame
    {
        void GetWindowContext(out PREVIEWHANDLERFRAMEINFO pinfo);
        [PreserveSig]
        int TranslateAccelerator(ref MSG pmsg);
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
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int x, int y, int cx, int cy, uint uFlags);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

        [DllImport("user32.dll")]
        public static extern IntPtr GetFocus();

        [DllImport("user32.dll", EntryPoint = "GetWindowLong")]
        private static extern int GetWindowLong32(IntPtr hWnd, int nIndex);

        [DllImport("user32.dll", EntryPoint = "GetWindowLongPtr")]
        private static extern IntPtr GetWindowLongPtr64(IntPtr hWnd, int nIndex);

        [DllImport("user32.dll", EntryPoint = "SetWindowLong")]
        private static extern int SetWindowLong32(IntPtr hWnd, int nIndex, int dwNewLong);

        [DllImport("user32.dll", EntryPoint = "SetWindowLongPtr")]
        private static extern IntPtr SetWindowLongPtr64(IntPtr hWnd, int nIndex, IntPtr dwNewLong);

        [DllImport("user32.dll")]
        private static extern uint GetDpiForWindow(IntPtr hwnd);

        [DllImport("shell32.dll")]
        public static extern void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);

        [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
        private static extern int AssocQueryString(uint flags, int str, string pszAssoc, string pszExtra, StringBuilder pszOut, ref uint pcchOut);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool DeleteFileW(string lpFileName);

        public const int GWL_STYLE = -16;
        public const uint WS_CHILD = 0x40000000;
        public const uint WS_POPUP = 0x80000000;
        public const uint WS_CAPTION = 0x00C00000;
        public const uint WS_THICKFRAME = 0x00040000;
        public const uint WS_VISIBLE = 0x10000000;
        public const uint WS_CLIPCHILDREN = 0x02000000;
        public const uint WS_CLIPSIBLINGS = 0x04000000;

        public const uint SWP_NOZORDER = 0x0004;
        public const uint SWP_NOACTIVATE = 0x0010;
        public const uint SWP_SHOWWINDOW = 0x0040;

        public const int SW_SHOW = 5;

        public const int SHCNE_ASSOCCHANGED = 0x08000000;
        public const uint SHCNF_IDLIST = 0x0000;
        public const int E_FAIL = unchecked((int)0x80004005);
        public const int S_OK = 0;
        public const int S_FALSE = 1;
        public const int ASSOCSTR_SHELLEXTENSION = 16;
        public const int WM_DPICHANGED_AFTERPARENT = 0x02E3;

        /// <summary>Makes a window a borderless child window (needed after SetParent across processes).</summary>
        public static void MakeChildWindow(IntPtr hwnd)
        {
            uint style;
            if (IntPtr.Size == 8)
                style = unchecked((uint)GetWindowLongPtr64(hwnd, GWL_STYLE).ToInt64());
            else
                style = unchecked((uint)GetWindowLong32(hwnd, GWL_STYLE));

            style &= ~(WS_POPUP | WS_CAPTION | WS_THICKFRAME);
            style |= WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;

            if (IntPtr.Size == 8)
                SetWindowLongPtr64(hwnd, GWL_STYLE, new IntPtr((long)style));
            else
                SetWindowLong32(hwnd, GWL_STYLE, unchecked((int)style));
        }

        /// <summary>The DPI of a window (Windows 10 1607+), or 0 when the API is unavailable.</summary>
        public static int GetWindowDpi(IntPtr hwnd)
        {
            try
            {
                uint dpi = GetDpiForWindow(hwnd);
                return dpi > 0 ? (int)dpi : 0;
            }
            catch (EntryPointNotFoundException)
            {
                return 0;
            }
            catch (DllNotFoundException)
            {
                return 0;
            }
        }

        /// <summary>The preview handler CLSID Windows actually resolves for an extension, or null.</summary>
        public static string QueryShellExtension(string extension, string category)
        {
            try
            {
                uint size = 260;
                StringBuilder sb = new StringBuilder((int)size);
                int hr = AssocQueryString(0, ASSOCSTR_SHELLEXTENSION, extension, category, sb, ref size);
                return hr == S_OK ? sb.ToString() : null;
            }
            catch
            {
                return null;
            }
        }

        /// <summary>Removes the "downloaded from the Internet" mark (Zone.Identifier stream).</summary>
        public static void RemoveZoneIdentifier(string path)
        {
            try { DeleteFileW(path + ":Zone.Identifier"); } catch { }
        }
    }

    /// <summary>Temp folder, CLI discovery and stale-file housekeeping shared by the handler.</summary>
    internal static class PreviewEnvironment
    {
        private static readonly object Gate = new object();
        private static string _tempDir;
        private static bool _swept;

        public static string TempDir
        {
            get
            {
                lock (Gate)
                {
                    if (_tempDir == null || !Directory.Exists(_tempDir))
                        _tempDir = ComputeWritableTempDir();
                    return _tempDir;
                }
            }
        }

        private static string ComputeWritableTempDir()
        {
            List<string> candidates = new List<string>();
            // LocalLow first: writable from both Low and Medium integrity levels, so it works whether
            // or not prevhost.exe runs in its Low-IL sandbox.
            try
            {
                string userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
                if (!string.IsNullOrEmpty(userProfile))
                    candidates.Add(Path.Combine(userProfile, @"AppData\LocalLow\LinkcoPdfPreview"));
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

        /// <summary>Once per process: deletes temp copies/rasters left behind by a killed preview host.</summary>
        public static void SweepStaleFilesOnce()
        {
            lock (Gate)
            {
                if (_swept)
                    return;
                _swept = true;
            }

            ThreadPool.QueueUserWorkItem(delegate
            {
                try
                {
                    DateTime cutoff = DateTime.UtcNow - TimeSpan.FromHours(6);
                    foreach (string f in Directory.GetFiles(TempDir))
                    {
                        string name = Path.GetFileName(f);
                        if (!(name.StartsWith("stream-", StringComparison.Ordinal) ||
                              name.StartsWith("page-", StringComparison.Ordinal) ||
                              name.StartsWith(".probe-", StringComparison.Ordinal)))
                            continue;
                        try
                        {
                            if (File.GetLastWriteTimeUtc(f) < cutoff)
                                File.Delete(f);
                        }
                        catch { }
                    }
                }
                catch { }
            });
        }

        /// <summary>Deletes a file now, or retries in the background while a killed process lets go of it.</summary>
        public static void DeleteFileSoon(string path)
        {
            if (string.IsNullOrEmpty(path))
                return;
            try
            {
                if (!File.Exists(path))
                    return;
                File.Delete(path);
                return;
            }
            catch { }

            ThreadPool.QueueUserWorkItem(delegate
            {
                for (int i = 0; i < 20; i++)
                {
                    try
                    {
                        if (!File.Exists(path))
                            return;
                        File.Delete(path);
                        return;
                    }
                    catch { }
                    Thread.Sleep(250);
                }
            });
        }

        public static RegistryKey OpenHive(RegistryHive hive)
        {
            RegistryView view = Environment.Is64BitOperatingSystem ? RegistryView.Registry64 : RegistryView.Default;
            return RegistryKey.OpenBaseKey(hive, view);
        }

        public static string LocateCliExecutable()
        {
            List<string> dirs = new List<string>();
            try
            {
                string asmLoc = Assembly.GetExecutingAssembly().Location;
                if (!string.IsNullOrEmpty(asmLoc))
                    dirs.Add(Path.GetDirectoryName(asmLoc));
            }
            catch { }

            try
            {
                string codeBase = Assembly.GetExecutingAssembly().CodeBase;
                if (!string.IsNullOrEmpty(codeBase))
                    dirs.Add(Path.GetDirectoryName(new Uri(codeBase).LocalPath));
            }
            catch { }

            foreach (string dir in dirs)
            {
                try
                {
                    if (string.IsNullOrEmpty(dir))
                        continue;
                    string candidate = Path.Combine(dir, "pdfcraft-cli.exe");
                    if (File.Exists(candidate))
                        return candidate;
                }
                catch { }
            }

            foreach (RegistryHive hive in new RegistryHive[] { RegistryHive.CurrentUser, RegistryHive.LocalMachine })
            {
                try
                {
                    using (RegistryKey root = OpenHive(hive))
                    using (RegistryKey k = root.OpenSubKey(LinkcoPdfPreviewHandler.LinkcoConfigKey, false))
                    {
                        if (k == null)
                            continue;
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
                catch { }
            }

            string[] fallbacks = new string[]
            {
                SafeCombine(Environment.SpecialFolder.LocalApplicationData, @"Programs\Linkco\Linkco PDF Editor\pdfcraft-cli.exe"),
                SafeCombine(Environment.SpecialFolder.LocalApplicationData, @"Programs\Linkco PDF Editor\pdfcraft-cli.exe"),
                SafeCombine(Environment.SpecialFolder.ProgramFiles, @"Linkco\Linkco PDF Editor\pdfcraft-cli.exe"),
                SafeCombine(Environment.SpecialFolder.ProgramFiles, @"Linkco PDF Editor\pdfcraft-cli.exe")
            };
            foreach (string candidate in fallbacks)
            {
                try
                {
                    if (!string.IsNullOrEmpty(candidate) && File.Exists(candidate))
                        return candidate;
                }
                catch { }
            }

            return null;
        }

        private static string SafeCombine(Environment.SpecialFolder folder, string relative)
        {
            try
            {
                string baseDir = Environment.GetFolderPath(folder);
                return string.IsNullOrEmpty(baseDir) ? null : Path.Combine(baseDir, relative);
            }
            catch
            {
                return null;
            }
        }
    }

    /// <summary>Small, size-capped diagnostic log in the preview temp folder (no document contents).</summary>
    internal static class PreviewLog
    {
        private static readonly object Gate = new object();
        private const long MaxBytes = 256 * 1024;

        public static void Write(string message)
        {
            try
            {
                string path = Path.Combine(PreviewEnvironment.TempDir, "preview.log");
                string line = DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss.fff", CultureInfo.InvariantCulture) +
                    " [" + Process.GetCurrentProcess().Id.ToString(CultureInfo.InvariantCulture) + "] " + message + Environment.NewLine;
                lock (Gate)
                {
                    FileInfo fi = new FileInfo(path);
                    if (fi.Exists && fi.Length > MaxBytes)
                    {
                        File.Copy(path, path + ".1", true);
                        File.Delete(path);
                    }
                    File.AppendAllText(path, line, Encoding.UTF8);
                }
            }
            catch { }
        }

        public static void Error(string where, Exception ex)
        {
            Write("ERROR " + where + ": " + (ex == null ? "(null)" : ex.GetType().Name + ": " + ex.Message));
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
        public const string HandlerProgId = "LinkcoPDFEditor.PreviewHandler";
        public const string HandlerName = "Linkco PDF Preview Handler";
        public const string PreviewHandlerCategoryGuid = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
        public const string PrevHostAppId = "{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}";
        public const string DotNetComCategory = "{62C8FE65-4EBB-45e7-B440-6E39B2CDBF29}";
        public const string EdgePreviewHandlerClsid = "{3A84F9C2-6164-485C-A7D9-4B27F8AC009E}";
        public const string LinkcoConfigKey = @"Software\Linkco\Linkco PDF Editor";

        /// <summary>
        /// Bumped whenever the registry layout changes, so LinkcoPDFEditor.exe re-registers (repairs)
        /// existing installs on its next start. Keep in sync with apps/pdfcraft/src/windows_preview.rs.
        /// </summary>
        public const int RegistrationSchema = 3;

        /// <summary>ProgIDs owned by Linkco PDF Editor (or its upstream PdfCraft).</summary>
        private static readonly string[] OwnProgIds = new string[]
        {
            "LinkcoPDFEditor.Document",
            "PdfCraft.Document"
        };

        /// <summary>PDF-only ProgIDs of other PDF viewers that can be the default .pdf handler.</summary>
        private static readonly string[] KnownPdfProgIds = new string[]
        {
            "MSEdgePDF",
            "Acrobat.Document.DC",
            "AcroExch.Document.DC",
            "AcroExch.Document",
            "AcroExch.Document.7",
            "FoxitReader.Document",
            "FoxitPDFEditor.Document",
            "PDFXEdit.PDF"
        };

        /// <summary>
        /// ProgIDs shared with web pages and other non-PDF types. Version 0.5.0 builds (schema 2)
        /// registered on some of these; registration and uninstall remove that again.
        /// </summary>
        private static readonly string[] KnownSharedProgIds = new string[]
        {
            "MSEdgeHTM",
            "ChromeHTML",
            "BraveHTML",
            "FirefoxHTML",
            "OperaStable",
            "htmlfile"
        };

        private static readonly string[] NonPdfExtensions = new string[]
        {
            ".htm", ".html", ".shtml", ".xht", ".xhtml", ".mht", ".mhtml", ".svg", ".xml",
            ".txt", ".webp", ".png", ".jpg", ".jpeg", ".gif", ".epub", ".url"
        };

        private IntPtr _parentHwnd = IntPtr.Zero;
        private RECT _bounds;
        private string _filePath;
        private string _displayName;
        private string _tempStreamPath;
        private string _initError;
        private object _site;
        private IPreviewHandlerFrame _frame;
        private PreviewPaneControl _control;
        private bool _hasBackground;
        private Color _background;
        private bool _hasTextColor;
        private Color _textColor;
        private string _fontFace;

        public LinkcoPdfPreviewHandler()
        {
            PreviewEnvironment.SweepStaleFilesOnce();
        }

        // ------------------------------------------------------------------ initialization

        public void Initialize(string pszFilePath, uint grfMode)
        {
            try
            {
                ResetDocument();
                _filePath = pszFilePath;
                _displayName = string.IsNullOrEmpty(pszFilePath) ? null : Path.GetFileName(pszFilePath);
            }
            catch (Exception ex)
            {
                PreviewLog.Error("IInitializeWithFile.Initialize", ex);
                _initError = "This PDF document could not be opened for preview.";
            }
        }

        public void Initialize(IStream pstream, uint grfMode)
        {
            try
            {
                ResetDocument();
                if (pstream == null)
                {
                    _initError = "Windows did not provide the PDF document to preview.";
                    return;
                }

                try
                {
                    System.Runtime.InteropServices.ComTypes.STATSTG stat;
                    pstream.Stat(out stat, 0);
                    if (!string.IsNullOrEmpty(stat.pwcsName))
                        _displayName = Path.GetFileName(stat.pwcsName);
                }
                catch { }

                string tempFile = Path.Combine(PreviewEnvironment.TempDir, "stream-" + Guid.NewGuid().ToString("N") + ".pdf");
                try { pstream.Seek(0, 0, IntPtr.Zero); } catch { }

                IntPtr bytesReadPtr = Marshal.AllocCoTaskMem(8);
                try
                {
                    using (FileStream fs = new FileStream(tempFile, FileMode.Create, FileAccess.Write, FileShare.Read, 1 << 16))
                    {
                        byte[] buffer = new byte[1 << 20];
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
                }
                finally
                {
                    Marshal.FreeCoTaskMem(bytesReadPtr);
                }

                _tempStreamPath = tempFile;
                _filePath = tempFile;
            }
            catch (Exception ex)
            {
                PreviewLog.Error("IInitializeWithStream.Initialize", ex);
                _initError = "This PDF document could not be read for preview.";
                CleanupTempStreamFile();
            }
            finally
            {
                // Release Explorer's stream right away so the file isn't kept locked while previewed.
                if (pstream != null)
                {
                    try { Marshal.ReleaseComObject(pstream); } catch { }
                }
            }
        }

        private void ResetDocument()
        {
            if (_control != null)
                _control.CancelRendering();
            CleanupTempStreamFile();
            _initError = null;
            _filePath = null;
            _displayName = null;
        }

        // ------------------------------------------------------------------ IPreviewHandler

        public void SetWindow(IntPtr hwnd, ref RECT rect)
        {
            try
            {
                _parentHwnd = hwnd;
                _bounds = rect;
                if (_control != null && _parentHwnd != IntPtr.Zero)
                {
                    AttachControl();
                    UpdateBounds();
                }
            }
            catch (Exception ex)
            {
                PreviewLog.Error("SetWindow", ex);
            }
        }

        public void SetRect(ref RECT rect)
        {
            try
            {
                _bounds = rect;
                UpdateBounds();
            }
            catch (Exception ex)
            {
                PreviewLog.Error("SetRect", ex);
            }
        }

        public void DoPreview()
        {
            try
            {
                if (_parentHwnd == IntPtr.Zero)
                    return;

                if (_control == null || _control.IsDisposed)
                {
                    _control = new PreviewPaneControl();
                    ApplyVisuals();
                }
                AttachControl();
                UpdateBounds();
                _control.LoadDocument(_filePath, _displayName, _initError);
            }
            catch (Exception ex)
            {
                PreviewLog.Error("DoPreview", ex);
                if (_control != null)
                {
                    try { _control.ShowMessage("This PDF document could not be previewed.", true); } catch { }
                }
            }
        }

        public void Unload()
        {
            try
            {
                if (_control != null)
                {
                    PreviewPaneControl c = _control;
                    _control = null;
                    try
                    {
                        c.Shutdown();
                        c.Dispose();
                    }
                    catch (Exception ex)
                    {
                        PreviewLog.Error("Unload/dispose", ex);
                    }
                }
                CleanupTempStreamFile();
                _filePath = null;
                _displayName = null;
                _initError = null;
            }
            catch (Exception ex)
            {
                PreviewLog.Error("Unload", ex);
            }
        }

        public void SetFocus()
        {
            if (_control != null && !_control.IsDisposed)
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
            // Let Explorer handle its own shortcuts (Alt+P, F5, Tab out of the pane, …).
            IPreviewHandlerFrame frame = _frame;
            if (frame != null)
            {
                try { return frame.TranslateAccelerator(ref pmsg); } catch { }
            }
            return NativeMethods.S_FALSE;
        }

        // ------------------------------------------------------------------ IPreviewHandlerVisuals

        public void SetBackgroundColor(uint color)
        {
            _background = ColorFromColorRef(color);
            _hasBackground = true;
            ApplyVisuals();
        }

        public void SetFont(ref LOGFONT plf)
        {
            _fontFace = string.IsNullOrEmpty(plf.lfFaceName) ? null : plf.lfFaceName;
            ApplyVisuals();
        }

        public void SetTextColor(uint color)
        {
            _textColor = ColorFromColorRef(color);
            _hasTextColor = true;
            ApplyVisuals();
        }

        private static Color ColorFromColorRef(uint color)
        {
            return Color.FromArgb((int)(color & 0xFF), (int)((color >> 8) & 0xFF), (int)((color >> 16) & 0xFF));
        }

        private void ApplyVisuals()
        {
            if (_control == null || _control.IsDisposed)
                return;
            try
            {
                _control.ApplyTheme(
                    _hasBackground ? (Color?)_background : null,
                    _hasTextColor ? (Color?)_textColor : null,
                    _fontFace);
            }
            catch (Exception ex)
            {
                PreviewLog.Error("ApplyVisuals", ex);
            }
        }

        // ------------------------------------------------------------------ IOleWindow / IObjectWithSite

        public void GetWindow(out IntPtr phwnd)
        {
            phwnd = (_control != null && !_control.IsDisposed) ? _control.Handle : _parentHwnd;
        }

        public void ContextSensitiveHelp(bool fEnterMode)
        {
        }

        public void SetSite(object pUnkSite)
        {
            _site = pUnkSite;
            _frame = null;
            if (pUnkSite != null)
            {
                try { _frame = pUnkSite as IPreviewHandlerFrame; } catch { _frame = null; }
            }
        }

        public void GetSite(ref Guid riid, out object ppvSite)
        {
            ppvSite = null;
            if (_site == null)
            {
                Marshal.ThrowExceptionForHR(NativeMethods.E_FAIL);
                return;
            }
            IntPtr punk = Marshal.GetIUnknownForObject(_site);
            try
            {
                IntPtr ppv;
                int hr = Marshal.QueryInterface(punk, ref riid, out ppv);
                if (hr != 0)
                {
                    Marshal.ThrowExceptionForHR(hr);
                    return;
                }
                try
                {
                    ppvSite = Marshal.GetObjectForIUnknown(ppv);
                }
                finally
                {
                    Marshal.Release(ppv);
                }
            }
            finally
            {
                Marshal.Release(punk);
            }
        }

        // ------------------------------------------------------------------ hosting

        private void AttachControl()
        {
            if (_control == null || _parentHwnd == IntPtr.Zero)
                return;

            IntPtr handle = _control.Handle;
            NativeMethods.SetParent(handle, _parentHwnd);
            NativeMethods.MakeChildWindow(handle);
            NativeMethods.ShowWindow(handle, NativeMethods.SW_SHOW);
            if (!_control.Visible)
                _control.Visible = true;
            _control.RefreshDpi();
        }

        private void UpdateBounds()
        {
            if (_control == null || _control.IsDisposed || _parentHwnd == IntPtr.Zero)
                return;
            int w = Math.Max(1, _bounds.Width);
            int h = Math.Max(1, _bounds.Height);
            NativeMethods.SetWindowPos(
                _control.Handle,
                IntPtr.Zero,
                _bounds.left,
                _bounds.top,
                w,
                h,
                NativeMethods.SWP_NOZORDER | NativeMethods.SWP_NOACTIVATE | NativeMethods.SWP_SHOWWINDOW);
            _control.Invalidate();
        }

        private void CleanupTempStreamFile()
        {
            string path = _tempStreamPath;
            _tempStreamPath = null;
            if (!string.IsNullOrEmpty(path))
                PreviewEnvironment.DeleteFileSoon(path);
        }

        // ------------------------------------------------------------------ registration

        [ComRegisterFunction]
        public static void Register(Type t)
        {
            if (t == null || t != typeof(LinkcoPdfPreviewHandler))
                return;
            RegisterPreviewHandler(Assembly.GetExecutingAssembly().Location, IsElevated());
        }

        [ComUnregisterFunction]
        public static void Unregister(Type t)
        {
            if (t == null || t != typeof(LinkcoPdfPreviewHandler))
                return;
            UnregisterPreviewHandler();
        }

        /// <summary>Per-user registration (no administrator rights needed).</summary>
        public static void RegisterPreviewHandler(string dllPath)
        {
            RegisterPreviewHandler(dllPath, false);
        }

        /// <summary>
        /// Registers the handler for the current user and, when <paramref name="machineWide"/> is set
        /// (elevated installers), for all users. Throws when the per-user registration fails.
        /// </summary>
        public static void RegisterPreviewHandler(string dllPath, bool machineWide)
        {
            if (string.IsNullOrEmpty(dllPath))
                throw new ArgumentException("The preview handler DLL path is empty.", "dllPath");
            string fullDllPath = Path.GetFullPath(dllPath);
            if (!File.Exists(fullDllPath))
                throw new FileNotFoundException("The preview handler DLL was not found.", fullDllPath);

            NativeMethods.RemoveZoneIdentifier(fullDllPath);
            string cliPath = Path.Combine(Path.GetDirectoryName(fullDllPath) ?? "", "pdfcraft-cli.exe");
            if (File.Exists(cliPath))
                NativeMethods.RemoveZoneIdentifier(cliPath);

            bool wasCurrent = IsRegistrationCurrent(fullDllPath);

            using (RegistryKey hkcu = PreviewEnvironment.OpenHive(RegistryHive.CurrentUser))
            {
                RegisterInRoot(hkcu, fullDllPath, true);
            }

            if (machineWide)
            {
                try
                {
                    using (RegistryKey hklm = PreviewEnvironment.OpenHive(RegistryHive.LocalMachine))
                    {
                        RegisterInRoot(hklm, fullDllPath, false);
                    }
                }
                catch (Exception ex)
                {
                    PreviewLog.Error("RegisterPreviewHandler(HKLM)", ex);
                }
            }

            if (!wasCurrent)
                StopStalePrevHost();
            NotifyShell();

            string effective = QueryEffectiveHandler();
            PreviewLog.Write("Registered " + HandlerName + " (schema " + RegistrationSchema.ToString(CultureInfo.InvariantCulture) +
                ", machineWide=" + machineWide + "); Explorer resolves .pdf previews to " + (effective ?? "(none)"));
        }

        /// <summary>Removes every registration (per-user and, when permitted, machine-wide).</summary>
        public static void UnregisterPreviewHandler()
        {
            foreach (RegistryHive hive in new RegistryHive[] { RegistryHive.CurrentUser, RegistryHive.LocalMachine })
            {
                try
                {
                    using (RegistryKey root = PreviewEnvironment.OpenHive(hive))
                    {
                        UnregisterFromRoot(root, hive == RegistryHive.CurrentUser);
                    }
                }
                catch (Exception ex)
                {
                    if (hive == RegistryHive.CurrentUser)
                        PreviewLog.Error("UnregisterPreviewHandler(HKCU)", ex);
                }
            }
            StopStalePrevHost();
            NotifyShell();
        }

        /// <summary>The preview handler CLSID Explorer currently resolves for .pdf files.</summary>
        public static string QueryEffectiveHandler()
        {
            return NativeMethods.QueryShellExtension(".pdf", PreviewHandlerCategoryGuid);
        }

        /// <summary>True when Explorer resolves .pdf previews to this handler.</summary>
        public static bool IsEffectiveHandler()
        {
            return string.Equals(QueryEffectiveHandler(), ClsidBraced, StringComparison.OrdinalIgnoreCase);
        }

        /// <summary>Human-readable registration and environment report for support and tests.</summary>
        public static string Diagnose()
        {
            StringBuilder sb = new StringBuilder();
            Assembly asm = typeof(LinkcoPdfPreviewHandler).Assembly;
            sb.AppendLine(HandlerName + " " + asm.GetName().Version);
            sb.AppendLine("  CLSID:                 " + ClsidBraced);
            sb.AppendLine("  Registration schema:   " + RegistrationSchema.ToString(CultureInfo.InvariantCulture));
            sb.AppendLine("  .NET runtime:          " + Environment.Version + (Environment.Is64BitProcess ? " (64-bit)" : " (32-bit)"));
            string effective = QueryEffectiveHandler();
            sb.AppendLine("  Explorer .pdf handler: " + (effective ?? "(none)") +
                (string.Equals(effective, ClsidBraced, StringComparison.OrdinalIgnoreCase) ? "  [Linkco - OK]" : "  [NOT Linkco]"));
            sb.AppendLine("  UserChoice ProgId:     " + (ReadUserChoiceProgId(".pdf") ?? "(none)"));
            string cli = PreviewEnvironment.LocateCliExecutable();
            sb.AppendLine("  pdfcraft-cli.exe:      " + (cli ?? "(not found)"));
            sb.AppendLine("  Temp folder:           " + PreviewEnvironment.TempDir);

            foreach (RegistryHive hive in new RegistryHive[] { RegistryHive.CurrentUser, RegistryHive.LocalMachine })
            {
                string label = hive == RegistryHive.CurrentUser ? "HKCU" : "HKLM";
                try
                {
                    using (RegistryKey root = PreviewEnvironment.OpenHive(hive))
                    using (RegistryKey inproc = root.OpenSubKey(@"Software\Classes\CLSID\" + ClsidBraced + @"\InprocServer32", false))
                    {
                        if (inproc == null)
                        {
                            sb.AppendLine("  " + label + " COM server:       (not registered)");
                            continue;
                        }
                        sb.AppendLine("  " + label + " CodeBase:         " + (inproc.GetValue("CodeBase") as string ?? "(missing)"));
                        sb.AppendLine("  " + label + " ThreadingModel:   " + (inproc.GetValue("ThreadingModel") as string ?? "(missing)"));
                    }
                }
                catch (Exception ex)
                {
                    sb.AppendLine("  " + label + ": " + ex.Message);
                }
            }
            return sb.ToString();
        }

        private static bool IsElevated()
        {
            try
            {
                using (System.Security.Principal.WindowsIdentity id = System.Security.Principal.WindowsIdentity.GetCurrent())
                {
                    return new System.Security.Principal.WindowsPrincipal(id).IsInRole(System.Security.Principal.WindowsBuiltInRole.Administrator);
                }
            }
            catch
            {
                return false;
            }
        }

        private static void NotifyShell()
        {
            try
            {
                NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
            }
            catch { }
        }

        private static bool IsRegistrationCurrent(string fullDllPath)
        {
            try
            {
                string expectedCodeBase = new Uri(fullDllPath).AbsoluteUri;
                string expectedAssembly = typeof(LinkcoPdfPreviewHandler).Assembly.FullName;
                using (RegistryKey hkcu = PreviewEnvironment.OpenHive(RegistryHive.CurrentUser))
                using (RegistryKey cfg = hkcu.OpenSubKey(LinkcoConfigKey, false))
                using (RegistryKey inproc = hkcu.OpenSubKey(@"Software\Classes\CLSID\" + ClsidBraced + @"\InprocServer32", false))
                {
                    if (cfg == null || inproc == null)
                        return false;
                    object schema = cfg.GetValue("PreviewHandlerSchema");
                    return schema is int && (int)schema == RegistrationSchema &&
                        string.Equals(cfg.GetValue("PreviewHandlerDll") as string, fullDllPath, StringComparison.OrdinalIgnoreCase) &&
                        string.Equals(inproc.GetValue("CodeBase") as string, expectedCodeBase, StringComparison.OrdinalIgnoreCase) &&
                        string.Equals(inproc.GetValue("Assembly") as string, expectedAssembly, StringComparison.Ordinal) &&
                        string.Equals(inproc.GetValue("ThreadingModel") as string, "Apartment", StringComparison.OrdinalIgnoreCase);
                }
            }
            catch
            {
                return false;
            }
        }

        private static void RegisterInRoot(RegistryKey root, string fullDllPath, bool perUser)
        {
            string dllDir = Path.GetDirectoryName(fullDllPath) ?? "";
            string cliPath = Path.Combine(dllDir, "pdfcraft-cli.exe");
            string codeBase = new Uri(fullDllPath).AbsoluteUri;
            Assembly asm = typeof(LinkcoPdfPreviewHandler).Assembly;
            string asmFullName = asm.FullName;
            string asmVersion = asm.GetName().Version.ToString();
            string className = typeof(LinkcoPdfPreviewHandler).FullName;

            using (RegistryKey clsidKey = root.CreateSubKey(@"Software\Classes\CLSID\" + ClsidBraced))
            {
                clsidKey.SetValue("", HandlerName);
                clsidKey.SetValue("DisplayName", HandlerName);
                clsidKey.SetValue("AppID", PrevHostAppId);
                // The handler starts pdfcraft-cli.exe and reads the PDF by path; the Low-IL sandbox
                // of prevhost.exe would block both.
                clsidKey.SetValue("DisableLowILProcessIsolation", 1, RegistryValueKind.DWord);

                using (RegistryKey inproc = clsidKey.CreateSubKey("InprocServer32"))
                {
                    inproc.SetValue("", "mscoree.dll");
                    // COM only understands Apartment/Both/Free/Neutral; the handler hosts WinForms, so Apartment.
                    inproc.SetValue("ThreadingModel", "Apartment");
                    inproc.SetValue("Class", className);
                    inproc.SetValue("Assembly", asmFullName);
                    inproc.SetValue("RuntimeVersion", "v4.0.30319");
                    inproc.SetValue("CodeBase", codeBase);

                    foreach (string sub in inproc.GetSubKeyNames())
                    {
                        if (!string.Equals(sub, asmVersion, StringComparison.OrdinalIgnoreCase))
                        {
                            try { inproc.DeleteSubKeyTree(sub, false); } catch { }
                        }
                    }
                    using (RegistryKey ver = inproc.CreateSubKey(asmVersion))
                    {
                        ver.SetValue("Class", className);
                        ver.SetValue("Assembly", asmFullName);
                        ver.SetValue("RuntimeVersion", "v4.0.30319");
                        ver.SetValue("CodeBase", codeBase);
                    }
                }

                using (RegistryKey progId = clsidKey.CreateSubKey("ProgId"))
                {
                    progId.SetValue("", HandlerProgId);
                }
                using (RegistryKey cat = clsidKey.CreateSubKey(@"Implemented Categories\" + DotNetComCategory))
                {
                }
            }

            using (RegistryKey handlerProgId = root.CreateSubKey(@"Software\Classes\" + HandlerProgId))
            {
                handlerProgId.SetValue("", HandlerName);
                using (RegistryKey clsid = handlerProgId.CreateSubKey("CLSID"))
                {
                    clsid.SetValue("", ClsidBraced);
                }
            }

            using (RegistryKey handlers = root.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers"))
            {
                handlers.SetValue(ClsidBraced, HandlerName);
            }

            // Windows resolves the preview handler through the default ProgID first (UserChoice, then
            // HKCR\.pdf's default), then .pdf itself, then SystemFileAssociations\.pdf. Register on
            // all of them — but only on ProgIDs that exist and that serve PDF files exclusively.
            BackupAndSetShellEx(root, @"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler");
            BackupAndSetShellEx(root, @"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler");

            foreach (string progId in CandidatePdfProgIds(perUser))
            {
                try
                {
                    BackupAndSetShellEx(root, ProgIdShellExPath(progId), "PrevProgId_" + progId);
                }
                catch (Exception ex)
                {
                    PreviewLog.Error("Register ShellEx on " + progId, ex);
                }
            }

            // Repair: earlier builds registered on browser ProgIDs that also open web pages.
            foreach (string progId in PreviouslyTouchedProgIds(root))
            {
                if (IsSharedProgId(progId))
                {
                    try { RestoreOrRemoveShellEx(root, progId, perUser); } catch { }
                }
            }

            using (RegistryKey cfg = root.CreateSubKey(LinkcoConfigKey))
            {
                if (cfg.GetValue("InstallDir") == null && !string.IsNullOrEmpty(dllDir))
                    cfg.SetValue("InstallDir", dllDir);
                if (File.Exists(cliPath))
                    cfg.SetValue("CliPath", cliPath);
                cfg.SetValue("PreviewHandlerDll", fullDllPath);
                cfg.SetValue("PreviewHandlerSchema", RegistrationSchema, RegistryValueKind.DWord);
                // Let Linkco PDF Editor tell cheaply (without loading .NET) whether this registration
                // still matches the DLL on disk and the user's current default PDF app; it registers
                // again when either changes.
                cfg.SetValue("PreviewHandlerDllStamp", DllStamp(fullDllPath));
                if (perUser)
                    cfg.SetValue("PreviewHandlerUserChoice", ReadUserChoiceProgId(".pdf") ?? "");
            }
        }

        /// <summary>"length:last-write-time-as-UTC-FILETIME" of a file, both decimal.</summary>
        public static string DllStamp(string path)
        {
            try
            {
                FileInfo fi = new FileInfo(path);
                return fi.Length.ToString(CultureInfo.InvariantCulture) + ":" +
                    fi.LastWriteTimeUtc.ToFileTimeUtc().ToString(CultureInfo.InvariantCulture);
            }
            catch
            {
                return "";
            }
        }

        private static string ProgIdShellExPath(string progId)
        {
            return @"Software\Classes\" + progId + @"\ShellEx\" + PreviewHandlerCategoryGuid;
        }

        private static List<string> CandidatePdfProgIds(bool perUser)
        {
            List<string> candidates = new List<string>();
            foreach (string p in OwnProgIds)
                AddUnique(candidates, p);
            foreach (string p in KnownPdfProgIds)
                AddUnique(candidates, p);
            if (perUser)
                AddUnique(candidates, ReadUserChoiceProgId(".pdf"));
            AddUnique(candidates, ReadClassesRootDefault(".pdf"));

            List<string> result = new List<string>();
            foreach (string p in candidates)
            {
                if (ProgIdExists(p) && !IsSharedProgId(p))
                    result.Add(p);
            }
            return result;
        }

        private static List<string> PreviouslyTouchedProgIds(RegistryKey root)
        {
            List<string> progIds = new List<string>();
            foreach (string p in KnownSharedProgIds)
                AddUnique(progIds, p);
            try
            {
                using (RegistryKey cfg = root.OpenSubKey(LinkcoConfigKey, false))
                {
                    if (cfg != null)
                    {
                        foreach (string name in cfg.GetValueNames())
                        {
                            if (name.StartsWith("PrevProgId_", StringComparison.Ordinal))
                                AddUnique(progIds, name.Substring("PrevProgId_".Length));
                        }
                    }
                }
            }
            catch { }
            return progIds;
        }

        private static void AddUnique(List<string> list, string value)
        {
            if (string.IsNullOrEmpty(value) || value.IndexOf('\\') >= 0)
                return;
            foreach (string existing in list)
            {
                if (string.Equals(existing, value, StringComparison.OrdinalIgnoreCase))
                    return;
            }
            list.Add(value);
        }

        private static bool ProgIdExists(string progId)
        {
            try
            {
                using (RegistryKey hkcr = PreviewEnvironment.OpenHive(RegistryHive.ClassesRoot))
                using (RegistryKey k = hkcr.OpenSubKey(progId, false))
                {
                    return k != null;
                }
            }
            catch
            {
                return false;
            }
        }

        private static string ReadClassesRootDefault(string extension)
        {
            try
            {
                using (RegistryKey hkcr = PreviewEnvironment.OpenHive(RegistryHive.ClassesRoot))
                using (RegistryKey k = hkcr.OpenSubKey(extension, false))
                {
                    return k == null ? null : k.GetValue("") as string;
                }
            }
            catch
            {
                return null;
            }
        }

        private static string ReadUserChoiceProgId(string extension)
        {
            string basePath = @"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\" + extension;
            try
            {
                using (RegistryKey hkcu = PreviewEnvironment.OpenHive(RegistryHive.CurrentUser))
                {
                    using (RegistryKey latest = hkcu.OpenSubKey(basePath + @"\UserChoiceLatest", false))
                    {
                        if (latest != null)
                        {
                            string p = latest.GetValue("ProgId") as string;
                            if (string.IsNullOrEmpty(p))
                            {
                                using (RegistryKey sub = latest.OpenSubKey("ProgId", false))
                                {
                                    if (sub != null)
                                        p = sub.GetValue("ProgId") as string;
                                }
                            }
                            if (!string.IsNullOrEmpty(p))
                                return p;
                        }
                    }
                    using (RegistryKey uc = hkcu.OpenSubKey(basePath + @"\UserChoice", false))
                    {
                        if (uc != null)
                            return uc.GetValue("ProgId") as string;
                    }
                }
            }
            catch { }
            return null;
        }

        /// <summary>
        /// True when a ProgID also serves non-PDF types (e.g. a browser's HTML ProgID): registering a
        /// PDF previewer on it would break previews of web pages and images.
        /// </summary>
        private static bool IsSharedProgId(string progId)
        {
            if (string.IsNullOrEmpty(progId))
                return true;
            foreach (string own in OwnProgIds)
            {
                if (string.Equals(own, progId, StringComparison.OrdinalIgnoreCase))
                    return false;
            }

            string lower = progId.ToLowerInvariant();
            if (lower.Contains("htm") || lower.Contains("url") || lower.StartsWith("operastable", StringComparison.Ordinal))
                return true;
            foreach (string s in KnownSharedProgIds)
            {
                if (lower.StartsWith(s.ToLowerInvariant(), StringComparison.Ordinal))
                    return true;
            }

            foreach (string ext in NonPdfExtensions)
            {
                if (string.Equals(ReadClassesRootDefault(ext), progId, StringComparison.OrdinalIgnoreCase))
                    return true;
            }

            try
            {
                using (RegistryKey hkcu = PreviewEnvironment.OpenHive(RegistryHive.CurrentUser))
                using (RegistryKey fileExts = hkcu.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts", false))
                {
                    if (fileExts != null)
                    {
                        int scanned = 0;
                        foreach (string ext in fileExts.GetSubKeyNames())
                        {
                            if (++scanned > 4000)
                                break;
                            if (string.Equals(ext, ".pdf", StringComparison.OrdinalIgnoreCase))
                                continue;
                            using (RegistryKey uc = fileExts.OpenSubKey(ext + @"\UserChoice", false))
                            {
                                if (uc != null && string.Equals(uc.GetValue("ProgId") as string, progId, StringComparison.OrdinalIgnoreCase))
                                    return true;
                            }
                        }
                    }
                }
            }
            catch { }

            return false;
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
                    if (cfg.GetValue(backupValueName) == null)
                        cfg.SetValue(backupValueName, existing);
                }
            }
            using (RegistryKey k = root.CreateSubKey(subKeyPath))
            {
                k.SetValue("", ClsidBraced);
            }
        }

        private static void UnregisterFromRoot(RegistryKey root, bool perUser)
        {
            using (RegistryKey handlers = root.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers", true))
            {
                if (handlers != null)
                    handlers.DeleteValue(ClsidBraced, false);
            }

            List<string> progIds = new List<string>();
            foreach (string p in OwnProgIds)
                AddUnique(progIds, p);
            foreach (string p in KnownPdfProgIds)
                AddUnique(progIds, p);
            foreach (string p in PreviouslyTouchedProgIds(root))
                AddUnique(progIds, p);
            if (perUser)
                AddUnique(progIds, ReadUserChoiceProgId(".pdf"));
            AddUnique(progIds, ReadClassesRootDefault(".pdf"));

            foreach (string progId in progIds)
            {
                try { RestoreOrRemoveShellEx(root, progId, perUser); } catch { }
            }

            RestoreOrRemove(root, @"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler", true);
            RestoreOrRemove(root, @"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler", false);
            DeleteIfEmpty(root, @"Software\Classes\.pdf\ShellEx");
            DeleteIfEmpty(root, @"Software\Classes\SystemFileAssociations\.pdf\ShellEx");

            root.DeleteSubKeyTree(@"Software\Classes\CLSID\" + ClsidBraced, false);
            root.DeleteSubKeyTree(@"Software\Classes\" + HandlerProgId, false);

            using (RegistryKey cfg = root.OpenSubKey(LinkcoConfigKey, true))
            {
                if (cfg != null)
                {
                    cfg.DeleteValue("PreviewHandlerDll", false);
                    cfg.DeleteValue("PreviewHandlerSchema", false);
                    cfg.DeleteValue("PreviewHandlerDllStamp", false);
                    cfg.DeleteValue("PreviewHandlerUserChoice", false);
                }
            }
        }

        private static void RestoreOrRemoveShellEx(RegistryKey root, string progId, bool perUser)
        {
            bool removed = RestoreOrRemove(root, ProgIdShellExPath(progId), "PrevProgId_" + progId, false);
            if (removed)
            {
                DeleteIfEmpty(root, @"Software\Classes\" + progId + @"\ShellEx");
                // Only per-user: an empty HKCU ProgID key would otherwise hide nothing but is clutter.
                if (perUser)
                    DeleteIfEmpty(root, @"Software\Classes\" + progId);
            }
        }

        /// <summary>Restores the previous handler (or removes ours). Returns true when the key was deleted.</summary>
        private static bool RestoreOrRemove(RegistryKey root, string subKeyPath, string backupValueName, bool edgeFallback)
        {
            string current = null;
            using (RegistryKey k = root.OpenSubKey(subKeyPath, false))
            {
                if (k != null)
                    current = k.GetValue("") as string;
            }

            string backup = null;
            using (RegistryKey cfg = root.OpenSubKey(LinkcoConfigKey, true))
            {
                if (cfg != null)
                {
                    backup = cfg.GetValue(backupValueName) as string;
                    if (string.Equals(current, ClsidBraced, StringComparison.OrdinalIgnoreCase) || current == null)
                        cfg.DeleteValue(backupValueName, false);
                }
            }

            if (!string.Equals(current, ClsidBraced, StringComparison.OrdinalIgnoreCase))
                return false;

            string restore = null;
            if (!string.IsNullOrEmpty(backup) && ClsidExists(backup))
                restore = backup;
            else if (edgeFallback && root.Name.StartsWith("HKEY_LOCAL_MACHINE", StringComparison.OrdinalIgnoreCase) && ClsidExists(EdgePreviewHandlerClsid))
                restore = EdgePreviewHandlerClsid;

            if (restore != null)
            {
                using (RegistryKey k = root.CreateSubKey(subKeyPath))
                {
                    k.SetValue("", restore);
                }
                return false;
            }

            root.DeleteSubKeyTree(subKeyPath, false);
            return true;
        }

        private static void DeleteIfEmpty(RegistryKey root, string subKeyPath)
        {
            try
            {
                bool empty;
                using (RegistryKey k = root.OpenSubKey(subKeyPath, false))
                {
                    if (k == null)
                        return;
                    empty = k.SubKeyCount == 0 && k.ValueCount == 0;
                }
                if (empty)
                    root.DeleteSubKey(subKeyPath, false);
            }
            catch { }
        }

        private static bool ClsidExists(string clsid)
        {
            try
            {
                using (RegistryKey hkcr = PreviewEnvironment.OpenHive(RegistryHive.ClassesRoot))
                using (RegistryKey k = hkcr.OpenSubKey(@"CLSID\" + clsid, false))
                {
                    return k != null;
                }
            }
            catch
            {
                return false;
            }
        }

        /// <summary>Restarts the preview host so Explorer picks up a new or updated handler DLL.</summary>
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
                        {
                            p.Kill();
                            p.WaitForExit(2000);
                        }
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

    /// <summary>One page raster request handed to the worker thread.</summary>
    internal sealed class RenderJob
    {
        public int Generation;
        public string PdfPath;
        public string CliPath;
        public int Page;
        public int Width;
    }

    /// <summary>Outcome of a page render, marshalled back to the UI thread.</summary>
    internal sealed class RenderResult
    {
        public int Generation;
        public int Page;
        public int RequestedWidth;
        public int TotalPages;
        public Bitmap Bitmap;
        public string Code;
        public string Message;
        public bool Cancelled;

        public void DisposeBitmap()
        {
            if (Bitmap != null)
            {
                try { Bitmap.Dispose(); } catch { }
                Bitmap = null;
            }
        }
    }

    internal sealed class CachedPage
    {
        public Bitmap Bitmap;
        public int RequestedWidth;

        public long Bytes { get { return Bitmap == null ? 0 : 4L * Bitmap.Width * Bitmap.Height; } }
    }

    /// <summary>
    /// The preview pane: a single custom-painted child window with the Linkco toolbar (page
    /// navigation, zoom) and the page raster. All state is touched on the preview host's UI thread
    /// only; rendering happens on a worker thread and is posted back with BeginInvoke.
    /// </summary>
    internal sealed class PreviewPaneControl : Control
    {
        private enum ToolButton { None, Prev, Next, ZoomOut, Fit, ZoomIn }

        private const int MaxRasterPx = 4096;
        private const int RenderTimeoutMs = 30000;
        private const long CacheBudgetBytes = 96L * 1024 * 1024;
        private const float MinZoom = 0.25f;
        private const float MaxZoom = 4.0f;

        private static readonly Color BrandNavy = Color.FromArgb(1, 19, 28);
        private static readonly Color BrandRed = Color.FromArgb(242, 36, 36);

        private readonly object _sync = new object();
        private int _generation;
        private bool _shutdown;
        private Process _activeProcess;

        private readonly Dictionary<int, CachedPage> _cache = new Dictionary<int, CachedPage>();
        private readonly LinkedList<int> _lru = new LinkedList<int>();
        private readonly System.Windows.Forms.Timer _rerenderTimer;

        private string _pdfPath;
        private string _displayName;
        private int _currentPage = 1;
        private int _totalPages;
        private float _zoom = 1.0f;
        private bool _fitWidth = true;
        private int _scrollX;
        private int _scrollY;
        private bool _loading;
        private string _message = "Select a PDF document to preview.";
        private bool _messageIsError;

        private ToolButton _hover = ToolButton.None;
        private ToolButton _pressed = ToolButton.None;
        private bool _dragging;
        private Point _dragStart;
        private int _dragScrollX;
        private int _dragScrollY;

        private Color _back = Color.FromArgb(242, 243, 245);
        private Color _text = Color.FromArgb(70, 76, 86);
        private bool _textFromHost;
        private string _fontFace = "Segoe UI";
        private float _scale = 1.0f;
        private Font _statusFont;
        private Font _labelFont;
        private Font _buttonFont;
        private Bitmap _display;
        private Bitmap _displaySource;

        public PreviewPaneControl()
        {
            SetStyle(
                ControlStyles.AllPaintingInWmPaint |
                ControlStyles.UserPaint |
                ControlStyles.OptimizedDoubleBuffer |
                ControlStyles.ResizeRedraw |
                ControlStyles.Selectable,
                true);
            DoubleBuffered = true;
            BackColor = _back;
            TabStop = true;
            AccessibleName = "Linkco PDF Preview";
            AccessibleRole = AccessibleRole.Graphic;

            _rerenderTimer = new System.Windows.Forms.Timer();
            _rerenderTimer.Interval = 250;
            _rerenderTimer.Tick += OnRerenderTimer;
        }

        // ------------------------------------------------------------------ public API (UI thread)

        public void ApplyTheme(Color? background, Color? text, string fontFace)
        {
            if (background.HasValue)
            {
                _back = background.Value;
                BackColor = _back;
            }
            if (text.HasValue)
            {
                _text = text.Value;
                _textFromHost = true;
            }
            else if (!_textFromHost)
            {
                _text = IsDark(_back) ? Color.FromArgb(220, 224, 230) : Color.FromArgb(70, 76, 86);
            }
            if (!string.IsNullOrEmpty(fontFace) && !string.Equals(fontFace, _fontFace, StringComparison.OrdinalIgnoreCase))
            {
                _fontFace = fontFace;
                DisposeFonts();
            }
            Invalidate();
        }

        public void RefreshDpi()
        {
            float newScale = 1.0f;
            int dpi = IsHandleCreated ? NativeMethods.GetWindowDpi(Handle) : 0;
            if (dpi > 0)
            {
                newScale = dpi / 96.0f;
            }
            else
            {
                try
                {
                    using (Graphics g = CreateGraphics())
                        newScale = g.DpiX / 96.0f;
                }
                catch { }
            }
            newScale = Math.Max(1.0f, Math.Min(4.0f, newScale));
            if (Math.Abs(newScale - _scale) > 0.01f)
            {
                _scale = newScale;
                DisposeFonts();
                Invalidate();
            }
        }

        public void LoadDocument(string filePath, string displayName, string initError)
        {
            CancelRendering();
            ClearCache();
            _pdfPath = filePath;
            _displayName = displayName;
            _currentPage = 1;
            _totalPages = 0;
            _fitWidth = true;
            _zoom = 1.0f;
            _scrollX = 0;
            _scrollY = 0;

            if (!string.IsNullOrEmpty(initError))
            {
                ShowMessage(initError, true);
                return;
            }
            if (string.IsNullOrEmpty(filePath) || !File.Exists(filePath))
            {
                ShowMessage("The selected PDF document could not be found.", true);
                return;
            }
            RequestRender(1);
        }

        public void ShowMessage(string message, bool isError)
        {
            _loading = false;
            _message = message;
            _messageIsError = isError;
            Invalidate();
        }

        /// <summary>Cancels the render in flight (kills pdfcraft-cli.exe) without tearing down the pane.</summary>
        public void CancelRendering()
        {
            lock (_sync)
            {
                _generation++;
                KillActiveProcessLocked();
            }
            _loading = false;
        }

        public void Shutdown()
        {
            lock (_sync)
            {
                _shutdown = true;
                _generation++;
                KillActiveProcessLocked();
            }
            try { _rerenderTimer.Stop(); } catch { }
            ClearCache();
        }

        // ------------------------------------------------------------------ rendering

        private CachedPage CurrentRaster
        {
            get
            {
                CachedPage c;
                return _cache.TryGetValue(_currentPage, out c) ? c : null;
            }
        }

        private int ComputeRequestWidth()
        {
            int clientW = ClientSize.Width;
            if (clientW < 50)
                clientW = 800;
            int canvasW = Math.Max(1, clientW - 2 * PageMargin);
            float z = _fitWidth ? 1.0f : _zoom;
            int w = (int)Math.Ceiling(canvasW * z);
            w = ((w + 127) / 128) * 128;
            return Math.Max(256, Math.Min(MaxRasterPx, w));
        }

        private int PageMargin
        {
            get { return (int)Math.Round(12 * _scale); }
        }

        private void RequestRender(int page)
        {
            if (string.IsNullOrEmpty(_pdfPath))
                return;

            string cli = PreviewEnvironment.LocateCliExecutable();
            if (string.IsNullOrEmpty(cli))
            {
                PreviewLog.Write("pdfcraft-cli.exe not found");
                ShowMessage("The Linkco PDF preview engine (pdfcraft-cli.exe) was not found. Please reinstall Linkco PDF Editor.", true);
                return;
            }

            int generation;
            lock (_sync)
            {
                if (_shutdown)
                    return;
                _generation++;
                generation = _generation;
                KillActiveProcessLocked();
            }

            RenderJob job = new RenderJob
            {
                Generation = generation,
                PdfPath = _pdfPath,
                CliPath = cli,
                Page = page,
                Width = ComputeRequestWidth()
            };

            _loading = true;
            if (CurrentRaster == null)
            {
                _message = "Loading page " + page.ToString(CultureInfo.CurrentCulture) + "\u2026";
                _messageIsError = false;
            }
            Invalidate();

            ThreadPool.QueueUserWorkItem(RenderWorker, job);
        }

        private void RenderWorker(object state)
        {
            RenderJob job = (RenderJob)state;
            RenderResult result;
            try
            {
                result = RunRender(job);
            }
            catch (Exception ex)
            {
                PreviewLog.Error("RenderWorker", ex);
                result = new RenderResult { Generation = job.Generation, Page = job.Page, Code = "ERROR", Message = ex.Message };
            }

            if (result.Cancelled || !PostResult(result))
                result.DisposeBitmap();
        }

        private bool PostResult(RenderResult result)
        {
            try
            {
                if (IsDisposed || !IsHandleCreated)
                    return false;
                BeginInvoke(new Action<RenderResult>(OnRenderComplete), result);
                return true;
            }
            catch
            {
                return false;
            }
        }

        private bool IsCurrent(int generation)
        {
            lock (_sync)
            {
                return !_shutdown && generation == _generation;
            }
        }

        private void KillActiveProcessLocked()
        {
            Process p = _activeProcess;
            _activeProcess = null;
            if (p == null)
                return;
            try
            {
                if (!p.HasExited)
                    p.Kill();
            }
            catch { }
        }

        private RenderResult RunRender(RenderJob job)
        {
            RenderResult res = new RenderResult { Generation = job.Generation, Page = job.Page, RequestedWidth = job.Width };
            string outPng = Path.Combine(
                PreviewEnvironment.TempDir,
                "page-" + Process.GetCurrentProcess().Id.ToString(CultureInfo.InvariantCulture) + "-" + Guid.NewGuid().ToString("N") + ".png");

            StringBuilder stdout = new StringBuilder();
            StringBuilder stderr = new StringBuilder();
            bool timedOut = false;

            if (!IsCurrent(job.Generation))
            {
                res.Cancelled = true;
                return res;
            }

            try
            {
                ProcessStartInfo psi = new ProcessStartInfo(job.CliPath)
                {
                    Arguments = "preview " + QuoteArg(job.PdfPath) +
                        " --page " + job.Page.ToString(CultureInfo.InvariantCulture) +
                        " --width " + job.Width.ToString(CultureInfo.InvariantCulture) +
                        " --max-px " + MaxRasterPx.ToString(CultureInfo.InvariantCulture) +
                        " --out " + QuoteArg(outPng),
                    CreateNoWindow = true,
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    RedirectStandardInput = false,
                    StandardOutputEncoding = Encoding.UTF8,
                    StandardErrorEncoding = Encoding.UTF8,
                    WorkingDirectory = Path.GetDirectoryName(job.CliPath) ?? ""
                };

                using (Process proc = new Process())
                {
                    proc.StartInfo = psi;
                    proc.OutputDataReceived += delegate(object sender, DataReceivedEventArgs e)
                    {
                        if (e.Data != null)
                            lock (stdout) stdout.AppendLine(e.Data);
                    };
                    proc.ErrorDataReceived += delegate(object sender, DataReceivedEventArgs e)
                    {
                        if (e.Data != null)
                            lock (stderr)
                            {
                                if (stderr.Length < 8192)
                                    stderr.AppendLine(e.Data);
                            }
                    };

                    proc.Start();
                    try
                    {
                        lock (_sync)
                        {
                            if (_shutdown || job.Generation != _generation)
                            {
                                try { proc.Kill(); } catch { }
                                res.Cancelled = true;
                                return res;
                            }
                            _activeProcess = proc;
                        }
                        try { proc.PriorityClass = ProcessPriorityClass.BelowNormal; } catch { }
                        proc.BeginOutputReadLine();
                        proc.BeginErrorReadLine();

                        if (!proc.WaitForExit(RenderTimeoutMs))
                        {
                            timedOut = true;
                            try { proc.Kill(); } catch { }
                            try { proc.WaitForExit(2000); } catch { }
                        }
                        else
                        {
                            proc.WaitForExit(); // drain the asynchronous output readers
                        }
                    }
                    finally
                    {
                        lock (_sync)
                        {
                            if (_activeProcess == proc)
                                _activeProcess = null;
                        }
                    }
                }

                if (!IsCurrent(job.Generation))
                {
                    res.Cancelled = true;
                    return res;
                }

                if (timedOut)
                {
                    PreviewLog.Write("render timed out after " + RenderTimeoutMs.ToString(CultureInfo.InvariantCulture) + " ms (page " + job.Page.ToString(CultureInfo.InvariantCulture) + ")");
                    res.Code = "TIMEOUT";
                    res.Message = "This page is taking too long to preview. Open the document in Linkco PDF Editor to view it.";
                    return res;
                }

                ParseStatus(stdout.ToString(), stderr.ToString(), outPng, res);
            }
            catch (Exception ex)
            {
                PreviewLog.Error("RunRender", ex);
                res.Code = "ERROR";
                res.Message = "The Linkco PDF preview engine could not be started.";
            }
            finally
            {
                PreviewEnvironment.DeleteFileSoon(outPng);
            }
            return res;
        }

        private static void ParseStatus(string stdout, string stderr, string outPng, RenderResult res)
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

            if (statusLine == null)
            {
                string err = LastLine(stderr);
                PreviewLog.Write("preview engine produced no status: " + (err ?? "(no output)"));
                res.Code = "ERROR";
                res.Message = "This PDF document could not be previewed. It may be damaged or use unsupported features.";
                return;
            }

            string[] parts = statusLine.Split('\t');
            res.Code = parts.Length > 1 ? parts[1] : "ERROR";
            res.TotalPages = parts.Length > 2 ? ParseInt(parts[2], 0) : 0;

            switch (res.Code)
            {
                case "OK":
                    res.Page = parts.Length > 3 ? ParseInt(parts[3], res.Page) : res.Page;
                    if (!File.Exists(outPng))
                    {
                        res.Code = "ERROR";
                        res.Message = "The page image could not be created.";
                        return;
                    }
                    try
                    {
                        res.Bitmap = LoadPremultiplied(outPng);
                    }
                    catch (Exception ex)
                    {
                        PreviewLog.Error("LoadPremultiplied", ex);
                        res.Code = "ERROR";
                        res.Message = "The page image could not be loaded.";
                    }
                    return;
                case "PASSWORD_REQUIRED":
                    res.Message = "This PDF document is password-protected. Open it in Linkco PDF Editor to enter the password.";
                    return;
                case "EMPTY_PDF":
                    res.Message = "This PDF document contains no pages.";
                    return;
                case "IO_ERROR":
                    res.Message = "The PDF document could not be read. It may be open in another program or no longer available.";
                    return;
                case "INVALID_PDF":
                    res.Message = "This file is not a valid PDF document, or it is damaged.";
                    return;
                case "RENDER_ERROR":
                    res.Message = "This page could not be previewed. Open the document in Linkco PDF Editor to view it.";
                    return;
                default:
                    res.Message = parts.Length > 3 && !string.IsNullOrEmpty(parts[3])
                        ? parts[3]
                        : "This PDF document could not be previewed.";
                    return;
            }
        }

        private static Bitmap LoadPremultiplied(string path)
        {
            byte[] bytes = File.ReadAllBytes(path);
            using (MemoryStream ms = new MemoryStream(bytes))
            using (Image img = Image.FromStream(ms, false, true))
            {
                Bitmap bmp = new Bitmap(img.Width, img.Height, PixelFormat.Format32bppPArgb);
                try
                {
                    using (Graphics g = Graphics.FromImage(bmp))
                    {
                        g.Clear(Color.White);
                        g.CompositingMode = CompositingMode.SourceOver;
                        g.InterpolationMode = InterpolationMode.NearestNeighbor;
                        g.DrawImage(img, new Rectangle(0, 0, img.Width, img.Height));
                    }
                    return bmp;
                }
                catch
                {
                    bmp.Dispose();
                    throw;
                }
            }
        }

        private void OnRenderComplete(RenderResult r)
        {
            if (IsDisposed || r.Cancelled || !IsCurrent(r.Generation))
            {
                r.DisposeBitmap();
                return;
            }

            _loading = false;
            if (r.TotalPages > 0)
                _totalPages = r.TotalPages;

            if (r.Bitmap != null)
            {
                StoreRaster(r.Page, r.Bitmap, r.RequestedWidth);
                r.Bitmap = null;
                if (_currentPage != r.Page)
                {
                    _currentPage = r.Page;
                    _scrollX = 0;
                    _scrollY = 0;
                }
                _message = null;
                _messageIsError = false;
                ClampScroll();
                Invalidate();
                if (ComputeRequestWidth() > r.RequestedWidth)
                    ScheduleSharperRaster();
                return;
            }

            PreviewLog.Write("preview status " + (r.Code ?? "?") + " for page " + r.Page.ToString(CultureInfo.InvariantCulture));
            if (r.Page == _currentPage && CurrentRaster != null)
            {
                // A sharper re-render failed (e.g. timed out at a high zoom): keep showing what we have.
                Invalidate();
                return;
            }
            ShowMessage(r.Message ?? "This PDF document could not be previewed.", true);
        }

        private void StoreRaster(int page, Bitmap bmp, int requestedWidth)
        {
            CachedPage old;
            if (_cache.TryGetValue(page, out old))
            {
                if (_displaySource == old.Bitmap)
                    DisposeDisplay();
                if (old.Bitmap != null)
                    old.Bitmap.Dispose();
                _lru.Remove(page);
            }
            _cache[page] = new CachedPage { Bitmap = bmp, RequestedWidth = requestedWidth };
            _lru.AddFirst(page);

            long total = 0;
            foreach (CachedPage c in _cache.Values)
                total += c.Bytes;
            while (total > CacheBudgetBytes && _lru.Count > 1)
            {
                int victim = _lru.Last.Value;
                if (victim == _currentPage || victim == page)
                {
                    // Never evict what is (about to be) on screen; move it to the front instead.
                    _lru.RemoveLast();
                    _lru.AddFirst(victim);
                    bool onlyVisibleLeft = true;
                    foreach (int p in _lru)
                    {
                        if (p != _currentPage && p != page)
                        {
                            onlyVisibleLeft = false;
                            break;
                        }
                    }
                    if (onlyVisibleLeft)
                        break;
                    continue;
                }
                total -= _cache[victim].Bytes;
                RemoveRaster(victim);
            }
        }

        private void RemoveRaster(int page)
        {
            CachedPage c;
            if (_cache.TryGetValue(page, out c))
            {
                if (_displaySource == c.Bitmap)
                    DisposeDisplay();
                _cache.Remove(page);
                _lru.Remove(page);
                if (c.Bitmap != null)
                    c.Bitmap.Dispose();
            }
        }

        private void ClearCache()
        {
            DisposeDisplay();
            foreach (CachedPage c in _cache.Values)
            {
                if (c.Bitmap != null)
                {
                    try { c.Bitmap.Dispose(); } catch { }
                }
            }
            _cache.Clear();
            _lru.Clear();
        }

        private void Touch(int page)
        {
            if (_lru.Remove(page))
                _lru.AddFirst(page);
        }

        private void GoToPage(int page)
        {
            if (_totalPages <= 0)
                return;
            int target = Math.Max(1, Math.Min(_totalPages, page));
            if (target == _currentPage && CurrentRaster != null)
                return;

            _currentPage = target;
            _scrollX = 0;
            _scrollY = 0;
            CachedPage cached = CurrentRaster;
            if (cached != null && cached.RequestedWidth >= ComputeRequestWidth())
            {
                CancelRendering();
                Touch(target);
                _message = null;
                Invalidate();
                return;
            }
            RequestRender(target);
        }

        private void ScheduleSharperRaster()
        {
            _rerenderTimer.Stop();
            _rerenderTimer.Start();
        }

        private void OnRerenderTimer(object sender, EventArgs e)
        {
            _rerenderTimer.Stop();
            if (IsDisposed || _loading || string.IsNullOrEmpty(_pdfPath) || _totalPages <= 0)
                return;
            CachedPage c = CurrentRaster;
            if (c != null && ComputeRequestWidth() > c.RequestedWidth)
                RequestRender(_currentPage);
        }

        // ------------------------------------------------------------------ layout

        private int Px(float v)
        {
            return (int)Math.Round(v * _scale);
        }

        private int ToolbarHeight
        {
            get { return Px(36); }
        }

        private void LayoutToolbar(out Rectangle prev, out Rectangle label, out Rectangle next,
                                   out Rectangle zoomOut, out Rectangle fit, out Rectangle zoomIn, out bool showZoom)
        {
            int top = Px(6);
            int h = Math.Max(1, ToolbarHeight - Px(12));
            int bw = Px(28);
            int gap = Px(4);
            int pad = Px(8);
            int clientW = ClientSize.Width;

            int fitW = Px(38);
            int zoomGroupW = bw + gap + fitW + gap + bw;
            int labelW = Px(120);
            int leftW = bw + gap + labelW + gap + bw;

            showZoom = clientW >= pad + leftW + Px(16) + zoomGroupW + pad;
            if (!showZoom)
            {
                // Narrow pane: shrink the page label, keep navigation.
                labelW = Math.Max(Px(60), clientW - 2 * pad - 2 * (bw + gap));
            }

            int x = pad;
            prev = new Rectangle(x, top, bw, h);
            x += bw + gap;
            label = new Rectangle(x, top, labelW, h);
            x += labelW + gap;
            next = new Rectangle(x, top, bw, h);

            int rx = clientW - pad - bw;
            zoomIn = new Rectangle(rx, top, bw, h);
            rx -= gap + fitW;
            fit = new Rectangle(rx, top, fitW, h);
            rx -= gap + bw;
            zoomOut = new Rectangle(rx, top, bw, h);
        }

        private ToolButton HitTest(Point pt)
        {
            Rectangle prev, label, next, zoomOut, fit, zoomIn;
            bool showZoom;
            LayoutToolbar(out prev, out label, out next, out zoomOut, out fit, out zoomIn, out showZoom);
            if (prev.Contains(pt)) return ToolButton.Prev;
            if (next.Contains(pt)) return ToolButton.Next;
            if (showZoom)
            {
                if (zoomOut.Contains(pt)) return ToolButton.ZoomOut;
                if (fit.Contains(pt)) return ToolButton.Fit;
                if (zoomIn.Contains(pt)) return ToolButton.ZoomIn;
            }
            return ToolButton.None;
        }

        private bool IsEnabled(ToolButton b)
        {
            bool hasPage = CurrentRaster != null;
            switch (b)
            {
                case ToolButton.Prev: return _totalPages > 0 && _currentPage > 1;
                case ToolButton.Next: return _totalPages > 0 && _currentPage < _totalPages;
                case ToolButton.ZoomOut: return hasPage && (_fitWidth || _zoom > MinZoom + 0.001f);
                case ToolButton.Fit: return hasPage && !_fitWidth;
                case ToolButton.ZoomIn: return hasPage && (_fitWidth || _zoom < MaxZoom - 0.001f);
                default: return false;
            }
        }

        /// <summary>Page rectangle in client coordinates for the current raster, zoom and scroll.</summary>
        private bool GetPageGeometry(out Rectangle pageRect, out int maxScrollX, out int maxScrollY)
        {
            pageRect = Rectangle.Empty;
            maxScrollX = 0;
            maxScrollY = 0;
            CachedPage c = CurrentRaster;
            if (c == null || c.Bitmap == null || c.Bitmap.Width <= 0 || c.Bitmap.Height <= 0)
                return false;

            int m = PageMargin;
            int clientW = ClientSize.Width;
            int canvasTop = ToolbarHeight;
            int canvasH = Math.Max(1, ClientSize.Height - canvasTop);
            int fitW = Math.Max(Px(40), clientW - 2 * m);
            int pageW = _fitWidth ? fitW : Math.Max(Px(40), (int)Math.Round(fitW * _zoom));
            int pageH = Math.Max(1, (int)Math.Round((double)c.Bitmap.Height * pageW / c.Bitmap.Width));

            maxScrollX = Math.Max(0, pageW + 2 * m - clientW);
            maxScrollY = Math.Max(0, pageH + 2 * m - canvasH);
            int sx = Math.Max(0, Math.Min(maxScrollX, _scrollX));
            int sy = Math.Max(0, Math.Min(maxScrollY, _scrollY));

            int x = maxScrollX > 0 ? m - sx : (clientW - pageW) / 2;
            int y = canvasTop + m - sy;
            pageRect = new Rectangle(x, y, pageW, pageH);
            return true;
        }

        private void ClampScroll()
        {
            Rectangle page;
            int maxX, maxY;
            if (!GetPageGeometry(out page, out maxX, out maxY))
            {
                _scrollX = 0;
                _scrollY = 0;
                return;
            }
            _scrollX = Math.Max(0, Math.Min(maxX, _scrollX));
            _scrollY = Math.Max(0, Math.Min(maxY, _scrollY));
        }

        // ------------------------------------------------------------------ zoom & scrolling

        private void SetZoom(bool fit, float zoom)
        {
            float oldW = _fitWidth ? 1.0f : _zoom;
            _fitWidth = fit;
            _zoom = Math.Max(MinZoom, Math.Min(MaxZoom, zoom));
            float newW = _fitWidth ? 1.0f : _zoom;
            if (oldW > 0)
            {
                // Keep the vertical position roughly in place.
                _scrollY = (int)Math.Round(_scrollY * (newW / oldW));
                _scrollX = (int)Math.Round(_scrollX * (newW / oldW));
            }
            ClampScroll();
            Invalidate();
            ScheduleSharperRaster();
        }

        private void ZoomBy(int direction)
        {
            float current = _fitWidth ? 1.0f : _zoom;
            float next = direction > 0 ? current * 1.25f : current / 1.25f;
            if (Math.Abs(next - 1.0f) < 0.06f)
            {
                SetZoom(true, 1.0f);
                return;
            }
            SetZoom(false, next);
        }

        private void ScrollBy(int dx, int dy, bool flipPagesAtEdges)
        {
            Rectangle page;
            int maxX, maxY;
            if (!GetPageGeometry(out page, out maxX, out maxY))
                return;

            if (flipPagesAtEdges && dy != 0)
            {
                if (dy > 0 && _scrollY >= maxY && _currentPage < _totalPages)
                {
                    GoToPage(_currentPage + 1);
                    return;
                }
                if (dy < 0 && _scrollY <= 0 && _currentPage > 1)
                {
                    int prevPage = _currentPage - 1;
                    GoToPage(prevPage);
                    _scrollY = int.MaxValue / 2; // show the bottom of the previous page
                    ClampScroll();
                    return;
                }
            }

            _scrollX = Math.Max(0, Math.Min(maxX, _scrollX + dx));
            _scrollY = Math.Max(0, Math.Min(maxY, _scrollY + dy));
            Invalidate();
        }

        private void Activate(ToolButton b)
        {
            if (!IsEnabled(b))
                return;
            switch (b)
            {
                case ToolButton.Prev: GoToPage(_currentPage - 1); break;
                case ToolButton.Next: GoToPage(_currentPage + 1); break;
                case ToolButton.ZoomOut: ZoomBy(-1); break;
                case ToolButton.ZoomIn: ZoomBy(+1); break;
                case ToolButton.Fit: SetZoom(true, 1.0f); break;
            }
        }

        // ------------------------------------------------------------------ input

        protected override bool IsInputKey(Keys keyData)
        {
            switch (keyData & Keys.KeyCode)
            {
                case Keys.Left:
                case Keys.Right:
                case Keys.Up:
                case Keys.Down:
                case Keys.PageUp:
                case Keys.PageDown:
                case Keys.Home:
                case Keys.End:
                    return true;
            }
            return base.IsInputKey(keyData);
        }

        protected override void OnKeyDown(KeyEventArgs e)
        {
            base.OnKeyDown(e);
            int line = Px(48);
            int pageStep = Math.Max(line, ClientSize.Height - ToolbarHeight - Px(48));
            bool handled = true;
            switch (e.KeyCode)
            {
                case Keys.Left:
                    if (e.Control) GoToPage(_currentPage - 1); else ScrollBy(-line, 0, false);
                    break;
                case Keys.Right:
                    if (e.Control) GoToPage(_currentPage + 1); else ScrollBy(line, 0, false);
                    break;
                case Keys.Up: ScrollBy(0, -line, true); break;
                case Keys.Down: ScrollBy(0, line, true); break;
                case Keys.PageUp: ScrollBy(0, -pageStep, true); break;
                case Keys.PageDown:
                case Keys.Space:
                    ScrollBy(0, pageStep, true);
                    break;
                case Keys.Home: GoToPage(1); break;
                case Keys.End: GoToPage(_totalPages); break;
                case Keys.Add:
                case Keys.Oemplus:
                    ZoomBy(+1);
                    break;
                case Keys.Subtract:
                case Keys.OemMinus:
                    ZoomBy(-1);
                    break;
                case Keys.D0:
                case Keys.NumPad0:
                    SetZoom(true, 1.0f);
                    break;
                default:
                    handled = false;
                    break;
            }
            if (handled)
            {
                e.Handled = true;
                e.SuppressKeyPress = true;
            }
        }

        protected override void OnMouseDown(MouseEventArgs e)
        {
            base.OnMouseDown(e);
            try { Focus(); } catch { }
            if (e.Button != MouseButtons.Left)
                return;

            ToolButton b = HitTest(e.Location);
            if (b != ToolButton.None)
            {
                _pressed = b;
                Capture = true;
                Invalidate();
                return;
            }

            if (e.Y > ToolbarHeight && CurrentRaster != null)
            {
                _dragging = true;
                _dragStart = e.Location;
                _dragScrollX = _scrollX;
                _dragScrollY = _scrollY;
                Capture = true;
                Cursor = Cursors.SizeAll;
            }
        }

        protected override void OnMouseMove(MouseEventArgs e)
        {
            base.OnMouseMove(e);
            if (_dragging)
            {
                _scrollX = _dragScrollX - (e.X - _dragStart.X);
                _scrollY = _dragScrollY - (e.Y - _dragStart.Y);
                ClampScroll();
                Invalidate();
                return;
            }

            ToolButton hover = HitTest(e.Location);
            if (hover != _hover)
            {
                _hover = hover;
                Invalidate(new Rectangle(0, 0, ClientSize.Width, ToolbarHeight));
            }
            Cursor = (hover != ToolButton.None && IsEnabled(hover)) ? Cursors.Hand : Cursors.Default;
        }

        protected override void OnMouseUp(MouseEventArgs e)
        {
            base.OnMouseUp(e);
            if (_dragging)
            {
                _dragging = false;
                Capture = false;
                Cursor = Cursors.Default;
                return;
            }
            if (_pressed != ToolButton.None)
            {
                ToolButton pressed = _pressed;
                _pressed = ToolButton.None;
                Capture = false;
                if (HitTest(e.Location) == pressed)
                    Activate(pressed);
                Invalidate();
            }
        }

        protected override void OnMouseLeave(EventArgs e)
        {
            base.OnMouseLeave(e);
            if (_hover != ToolButton.None)
            {
                _hover = ToolButton.None;
                Invalidate(new Rectangle(0, 0, ClientSize.Width, ToolbarHeight));
            }
        }

        protected override void OnMouseWheel(MouseEventArgs e)
        {
            base.OnMouseWheel(e);
            int notches = e.Delta / 120;
            if (notches == 0)
                notches = Math.Sign(e.Delta);
            if ((ModifierKeys & Keys.Control) == Keys.Control)
            {
                if (CurrentRaster != null)
                    ZoomBy(notches > 0 ? +1 : -1);
                return;
            }
            int step = Px(64) * Math.Abs(notches);
            if ((ModifierKeys & Keys.Shift) == Keys.Shift)
                ScrollBy(notches > 0 ? -step : step, 0, false);
            else
                ScrollBy(0, notches > 0 ? -step : step, true);
        }

        protected override void OnResize(EventArgs e)
        {
            base.OnResize(e);
            ClampScroll();
            Invalidate();
            if (CurrentRaster != null)
                ScheduleSharperRaster();
        }

        protected override void OnHandleCreated(EventArgs e)
        {
            base.OnHandleCreated(e);
            RefreshDpi();
        }

        protected override void WndProc(ref Message m)
        {
            if (m.Msg == NativeMethods.WM_DPICHANGED_AFTERPARENT)
            {
                RefreshDpi();
                ClampScroll();
                Invalidate();
                ScheduleSharperRaster();
            }
            base.WndProc(ref m);
        }

        // ------------------------------------------------------------------ painting

        private static bool IsDark(Color c)
        {
            return (0.299 * c.R + 0.587 * c.G + 0.114 * c.B) < 128;
        }

        private void EnsureFonts()
        {
            if (_statusFont != null)
                return;
            string face = string.IsNullOrEmpty(_fontFace) ? "Segoe UI" : _fontFace;
            try
            {
                _statusFont = new Font(face, 13f * _scale, FontStyle.Regular, GraphicsUnit.Pixel);
                _labelFont = new Font(face, 12f * _scale, FontStyle.Bold, GraphicsUnit.Pixel);
                _buttonFont = new Font(face, 13f * _scale, FontStyle.Bold, GraphicsUnit.Pixel);
            }
            catch
            {
                DisposeFonts();
                _statusFont = new Font(FontFamily.GenericSansSerif, 13f * _scale, FontStyle.Regular, GraphicsUnit.Pixel);
                _labelFont = new Font(FontFamily.GenericSansSerif, 12f * _scale, FontStyle.Bold, GraphicsUnit.Pixel);
                _buttonFont = new Font(FontFamily.GenericSansSerif, 13f * _scale, FontStyle.Bold, GraphicsUnit.Pixel);
            }
        }

        private void DisposeFonts()
        {
            if (_statusFont != null) { _statusFont.Dispose(); _statusFont = null; }
            if (_labelFont != null) { _labelFont.Dispose(); _labelFont = null; }
            if (_buttonFont != null) { _buttonFont.Dispose(); _buttonFont = null; }
        }

        protected override void OnPaintBackground(PaintEventArgs e)
        {
            // Everything is painted in OnPaint (double-buffered) to avoid flicker.
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            try
            {
                PaintPane(e.Graphics);
            }
            catch (Exception ex)
            {
                PreviewLog.Error("OnPaint", ex);
            }
        }

        private void PaintPane(Graphics g)
        {
            EnsureFonts();
            Rectangle client = ClientRectangle;
            int toolbarH = ToolbarHeight;

            using (SolidBrush bg = new SolidBrush(_back))
                g.FillRectangle(bg, client);

            Rectangle canvas = new Rectangle(0, toolbarH, client.Width, Math.Max(1, client.Height - toolbarH));
            g.SetClip(canvas);

            Rectangle pageRect;
            int maxX, maxY;
            CachedPage raster = CurrentRaster;
            if (raster != null && GetPageGeometry(out pageRect, out maxX, out maxY))
            {
                bool dark = IsDark(_back);
                using (SolidBrush shadow = new SolidBrush(Color.FromArgb(dark ? 140 : 40, 0, 0, 0)))
                    g.FillRectangle(shadow, new Rectangle(pageRect.X + Px(2), pageRect.Y + Px(3), pageRect.Width, pageRect.Height));

                g.FillRectangle(Brushes.White, pageRect);
                g.CompositingQuality = CompositingQuality.HighSpeed;
                Bitmap display = GetDisplayBitmap(raster.Bitmap, pageRect.Size);
                if (display != null)
                {
                    // Pre-scaled once: scrolling and repaints are a plain 1:1 blit.
                    g.InterpolationMode = InterpolationMode.NearestNeighbor;
                    g.PixelOffsetMode = PixelOffsetMode.Half;
                    g.DrawImageUnscaled(display, pageRect.X, pageRect.Y);
                }
                else
                {
                    g.InterpolationMode = _dragging ? InterpolationMode.Bilinear : InterpolationMode.HighQualityBicubic;
                    g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                    g.DrawImage(raster.Bitmap, pageRect);
                }

                using (Pen border = new Pen(dark ? Color.FromArgb(70, 76, 86) : Color.FromArgb(200, 205, 212), 1f))
                    g.DrawRectangle(border, pageRect.X, pageRect.Y, pageRect.Width - 1, pageRect.Height - 1);

                PaintScrollIndicators(g, canvas, pageRect, maxX, maxY);
            }
            else
            {
                PaintMessage(g, canvas);
            }

            g.ResetClip();
            PaintToolbar(g, client.Width, toolbarH);
        }

        /// <summary>
        /// The page raster scaled to its on-screen size (high-quality, cached), or null to draw the
        /// raster directly (when it already has that size or the scaled copy would be too large).
        /// </summary>
        private Bitmap GetDisplayBitmap(Bitmap source, Size size)
        {
            if (source == null || size.Width <= 0 || size.Height <= 0)
                return null;
            if (source.Width == size.Width && source.Height == size.Height)
                return source;
            if ((long)size.Width * size.Height > 12L * 1024 * 1024)
                return null;
            if (_display != null && _displaySource == source && _display.Width == size.Width && _display.Height == size.Height)
                return _display;

            DisposeDisplay();
            try
            {
                Bitmap scaled = new Bitmap(size.Width, size.Height, PixelFormat.Format32bppPArgb);
                using (Graphics g = Graphics.FromImage(scaled))
                {
                    g.Clear(Color.White);
                    g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                    g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                    g.CompositingQuality = CompositingQuality.HighQuality;
                    using (ImageAttributes attrs = new ImageAttributes())
                    {
                        attrs.SetWrapMode(WrapMode.TileFlipXY); // no grey fringe at the page edges
                        g.DrawImage(source, new Rectangle(0, 0, size.Width, size.Height), 0, 0, source.Width, source.Height, GraphicsUnit.Pixel, attrs);
                    }
                }
                _display = scaled;
                _displaySource = source;
                return _display;
            }
            catch (Exception ex)
            {
                PreviewLog.Error("GetDisplayBitmap", ex);
                DisposeDisplay();
                return null;
            }
        }

        private void DisposeDisplay()
        {
            if (_display != null)
            {
                try { _display.Dispose(); } catch { }
                _display = null;
            }
            _displaySource = null;
        }

        private void PaintScrollIndicators(Graphics g, Rectangle canvas, Rectangle pageRect, int maxX, int maxY)
        {
            int thickness = Math.Max(3, Px(4));
            using (SolidBrush thumb = new SolidBrush(Color.FromArgb(110, 128, 128, 128)))
            {
                if (maxY > 0)
                {
                    int trackH = canvas.Height - 2 * Px(4);
                    int total = pageRect.Height + 2 * PageMargin;
                    int thumbH = Math.Max(Px(24), (int)((long)trackH * canvas.Height / Math.Max(1, total)));
                    int thumbY = canvas.Top + Px(4) + (int)((long)(trackH - thumbH) * Math.Min(_scrollY, maxY) / maxY);
                    g.FillRectangle(thumb, canvas.Right - thickness - Px(2), thumbY, thickness, thumbH);
                }
                if (maxX > 0)
                {
                    int trackW = canvas.Width - 2 * Px(4);
                    int total = pageRect.Width + 2 * PageMargin;
                    int thumbW = Math.Max(Px(24), (int)((long)trackW * canvas.Width / Math.Max(1, total)));
                    int thumbX = canvas.Left + Px(4) + (int)((long)(trackW - thumbW) * Math.Min(_scrollX, maxX) / maxX);
                    g.FillRectangle(thumb, thumbX, canvas.Bottom - thickness - Px(2), thumbW, thickness);
                }
            }
        }

        private void PaintMessage(Graphics g, Rectangle canvas)
        {
            string msg = !string.IsNullOrEmpty(_message) ? _message : (_loading ? "Loading\u2026" : "Select a PDF document to preview.");
            int pad = Px(20);
            Rectangle textRect = new Rectangle(canvas.Left + pad, canvas.Top + pad, Math.Max(Px(40), canvas.Width - 2 * pad), Math.Max(Px(40), canvas.Height - 2 * pad));
            g.TextRenderingHint = TextRenderingHint.ClearTypeGridFit;

            using (StringFormat sf = new StringFormat())
            {
                sf.Alignment = StringAlignment.Center;
                sf.LineAlignment = StringAlignment.Center;
                sf.Trimming = StringTrimming.EllipsisWord;

                string full = msg;
                if (!string.IsNullOrEmpty(_displayName) && (_messageIsError || _loading))
                    full = _displayName + "\n\n" + msg;

                using (SolidBrush textBrush = new SolidBrush(_text))
                    g.DrawString(full, _statusFont, textBrush, textRect, sf);

                if (_messageIsError)
                {
                    SizeF size = g.MeasureString(full, _statusFont, textRect.Width, sf);
                    int lineW = Px(36);
                    int lineY = textRect.Top + (int)((textRect.Height + size.Height) / 2) + Px(10);
                    using (Pen accent = new Pen(BrandRed, Math.Max(2f, 2f * _scale)))
                        g.DrawLine(accent, textRect.Left + (textRect.Width - lineW) / 2, lineY, textRect.Left + (textRect.Width + lineW) / 2, lineY);
                }
            }
        }

        private void PaintToolbar(Graphics g, int width, int height)
        {
            Rectangle bar = new Rectangle(0, 0, width, height);
            using (SolidBrush navy = new SolidBrush(BrandNavy))
                g.FillRectangle(navy, bar);
            using (Pen accent = new Pen(BrandRed, Math.Max(2f, 2f * _scale)))
                g.DrawLine(accent, 0, height - 1, width, height - 1);

            Rectangle prev, label, next, zoomOut, fit, zoomIn;
            bool showZoom;
            LayoutToolbar(out prev, out label, out next, out zoomOut, out fit, out zoomIn, out showZoom);

            g.SmoothingMode = SmoothingMode.AntiAlias;
            g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
            DrawButton(g, prev, "\u2039", ToolButton.Prev);
            DrawButton(g, next, "\u203A", ToolButton.Next);
            if (showZoom)
            {
                DrawButton(g, zoomOut, "\u2212", ToolButton.ZoomOut);
                DrawButton(g, fit, "Fit", ToolButton.Fit);
                DrawButton(g, zoomIn, "+", ToolButton.ZoomIn);
            }

            string text;
            if (_totalPages > 0)
                text = "Page " + _currentPage.ToString(CultureInfo.CurrentCulture) + " of " + _totalPages.ToString(CultureInfo.CurrentCulture);
            else
                text = "Linkco PDF Preview";
            if (_loading && CurrentRaster != null)
                text += " \u2026";

            using (StringFormat sf = new StringFormat())
            {
                sf.Alignment = StringAlignment.Center;
                sf.LineAlignment = StringAlignment.Center;
                sf.Trimming = StringTrimming.EllipsisCharacter;
                sf.FormatFlags = StringFormatFlags.NoWrap;
                g.DrawString(text, _labelFont, Brushes.White, label, sf);
            }
            g.SmoothingMode = SmoothingMode.None;
        }

        private void DrawButton(Graphics g, Rectangle r, string glyph, ToolButton id)
        {
            bool enabled = IsEnabled(id);
            bool hover = enabled && _hover == id;
            bool pressed = enabled && _pressed == id;

            Color fill = pressed ? BrandRed : (hover ? Color.FromArgb(32, 58, 78) : (enabled ? Color.FromArgb(18, 38, 54) : Color.FromArgb(10, 26, 36)));
            Color border = enabled ? (hover || pressed ? BrandRed : Color.FromArgb(70, 92, 110)) : Color.FromArgb(40, 54, 64);
            Color fg = enabled ? Color.White : Color.FromArgb(105, 118, 130);

            using (GraphicsPath path = RoundedRect(r, Px(4)))
            {
                using (SolidBrush b = new SolidBrush(fill))
                    g.FillPath(b, path);
                using (Pen p = new Pen(border, 1f))
                    g.DrawPath(p, path);
            }
            using (SolidBrush tb = new SolidBrush(fg))
            using (StringFormat sf = new StringFormat())
            {
                sf.Alignment = StringAlignment.Center;
                sf.LineAlignment = StringAlignment.Center;
                sf.FormatFlags = StringFormatFlags.NoWrap;
                g.DrawString(glyph, _buttonFont, tb, r, sf);
            }
        }

        private static GraphicsPath RoundedRect(Rectangle r, int radius)
        {
            GraphicsPath path = new GraphicsPath();
            int d = Math.Max(1, Math.Min(radius * 2, Math.Min(r.Width, r.Height)));
            path.AddArc(r.X, r.Y, d, d, 180, 90);
            path.AddArc(r.Right - d - 1, r.Y, d, d, 270, 90);
            path.AddArc(r.Right - d - 1, r.Bottom - d - 1, d, d, 0, 90);
            path.AddArc(r.X, r.Bottom - d - 1, d, d, 90, 90);
            path.CloseFigure();
            return path;
        }

        // ------------------------------------------------------------------ helpers & teardown

        private static int ParseInt(string s, int fallback)
        {
            int v;
            return int.TryParse(s, NumberStyles.Integer, CultureInfo.InvariantCulture, out v) ? v : fallback;
        }

        private static string LastLine(string text)
        {
            if (string.IsNullOrEmpty(text))
                return null;
            string[] lines = text.Split(new char[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries);
            if (lines.Length == 0)
                return null;
            string last = lines[lines.Length - 1].Trim();
            return last.Length > 300 ? last.Substring(0, 300) : last;
        }

        /// <summary>Quotes one argument for the Windows (MSVC/Rust std) command-line parser.</summary>
        internal static string QuoteArg(string arg)
        {
            if (string.IsNullOrEmpty(arg))
                return "\"\"";
            StringBuilder sb = new StringBuilder(arg.Length + 2);
            sb.Append('"');
            int backslashes = 0;
            foreach (char c in arg)
            {
                if (c == '\\')
                {
                    backslashes++;
                    continue;
                }
                if (c == '"')
                {
                    sb.Append('\\', backslashes * 2 + 1);
                    sb.Append('"');
                }
                else
                {
                    sb.Append('\\', backslashes);
                    sb.Append(c);
                }
                backslashes = 0;
            }
            sb.Append('\\', backslashes * 2);
            sb.Append('"');
            return sb.ToString();
        }

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                Shutdown();
                try
                {
                    _rerenderTimer.Tick -= OnRerenderTimer;
                    _rerenderTimer.Dispose();
                }
                catch { }
                DisposeFonts();
            }
            base.Dispose(disposing);
        }
    }
}
