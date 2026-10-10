// Linkco PDF Editor — Windows File Explorer PDF Preview Handler (IPreviewHandler)
// Publisher: Al Rawabet Commercial Services & Contracting Company W.L.L. (Linkco — www.linkco.com.qa)
//
// Compiled into LinkcoPdfPreviewHandler.dll during Windows build/packaging using the OS-included
// .NET Framework 4.x compiler (C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe).
// Hosted out-of-process by Windows' standard 64-bit Preview Host (prevhost.exe,
// AppID {6d2b5079-2f0b-48dd-ab7f-97cec514d30b}) and delegates single-page PDF rasterization to
// pdfcraft-cli.exe (`pdfcraft-cli preview <file.pdf> --page N --dpi 150 --out <temp.png>`),
// never opening the main Linkco PDF Editor window.

using System;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Threading;
using System.Windows.Forms;
using Microsoft.Win32;

[assembly: AssemblyTitle("Linkco PDF Preview Handler")]
[assembly: AssemblyDescription("Windows File Explorer PDF Preview Handler for Linkco PDF Editor")]
[assembly: AssemblyCompany("Al Rawabet Commercial Services & Contracting Company W.L.L.")]
[assembly: AssemblyProduct("Linkco PDF Editor")]
[assembly: AssemblyCopyright("© Al Rawabet Commercial Services & Contracting Company W.L.L.")]
[assembly: AssemblyVersion("0.3.0.0")]
[assembly: AssemblyFileVersion("0.3.0.0")]
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
        public static extern IntPtr GetFocus();

        [DllImport("shell32.dll")]
        public static extern void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);

        public const int SHCNE_ASSOCCHANGED = 0x08000000;
        public const uint SHCNF_IDLIST = 0x0000;
        public const int E_FAIL = unchecked((int)0x80004005);
        public const int S_FALSE = 1;
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
        public const string EdgePreviewHandlerClsid = "{3A84F9C2-6164-485C-A7D9-4B27F8AC009E}";
        public const string LinkcoConfigKey = @"Software\Linkco\Linkco PDF Editor";

        private IntPtr _parentHwnd = IntPtr.Zero;
        private RECT _windowBounds;
        private string _filePath;
        private string _tempStreamPath;
        private object _unkSite;
        private PreviewPaneControl _control;

        public void Initialize(string pszFilePath, uint grfMode)
        {
            CleanupTempStreamFile();
            _filePath = pszFilePath;
        }

        public void Initialize(IStream pstream, uint grfMode)
        {
            CleanupTempStreamFile();
            if (pstream == null)
                return;

            string tempDir = Path.Combine(Path.GetTempPath(), "LinkcoPdfPreview");
            Directory.CreateDirectory(tempDir);
            string tempFile = Path.Combine(tempDir, "stream-" + Guid.NewGuid().ToString("N") + ".pdf");

            try
            {
                using (FileStream fs = new FileStream(tempFile, FileMode.Create, FileAccess.Write, FileShare.Read))
                {
                    byte[] buffer = new byte[65536];
                    IntPtr bytesReadPtr = Marshal.AllocCoTaskMem(sizeof(int));
                    try
                    {
                        while (true)
                        {
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
            catch
            {
                CleanupTempStreamFile();
            }
        }

        public void SetWindow(IntPtr hwnd, ref RECT rect)
        {
            _parentHwnd = hwnd;
            _windowBounds = rect;
            if (_control != null)
            {
                NativeMethods.SetParent(_control.Handle, _parentHwnd);
                UpdateBounds();
            }
        }

        public void SetRect(ref RECT rect)
        {
            _windowBounds = rect;
            UpdateBounds();
        }

        public void DoPreview()
        {
            if (_parentHwnd == IntPtr.Zero)
                return;

            if (_control == null)
            {
                _control = new PreviewPaneControl();
                IntPtr handle = _control.Handle;
                NativeMethods.SetParent(handle, _parentHwnd);
            }

            UpdateBounds();
            _control.Visible = true;
            _control.LoadDocument(_filePath);
        }

        public void Unload()
        {
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
                _control.Focus();
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

        private void UpdateBounds()
        {
            if (_control != null)
            {
                _control.Bounds = new Rectangle(
                    _windowBounds.left,
                    _windowBounds.top,
                    Math.Max(10, _windowBounds.Width),
                    Math.Max(10, _windowBounds.Height)
                );
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
            string codeBase = new Uri(dllPath).AbsoluteUri;
            string asmFullName = typeof(LinkcoPdfPreviewHandler).Assembly.FullName;

            using (RegistryKey clsidKey = Registry.LocalMachine.CreateSubKey(@"Software\Classes\CLSID\" + ClsidBraced))
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

            using (RegistryKey handlers = Registry.LocalMachine.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers"))
            {
                if (handlers != null)
                    handlers.SetValue(ClsidBraced, "Linkco PDF Preview Handler");
            }

            // Register under Linkco PDF Editor ProgIDs.
            foreach (string progId in new string[] { "LinkcoPDFEditor.Document", "PdfCraft.Document" })
            {
                using (RegistryKey k = Registry.LocalMachine.CreateSubKey(@"Software\Classes\" + progId + @"\ShellEx\" + PreviewHandlerCategoryGuid))
                {
                    if (k != null)
                        k.SetValue("", ClsidBraced);
                }
            }

            // Backup any existing non-Linkco .pdf preview handler before setting Linkco's handler.
            BackupAndSetShellEx(@"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler");
            BackupAndSetShellEx(@"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler");

            NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
        }

        private static void BackupAndSetShellEx(string subKeyPath, string backupValueName)
        {
            string existing = null;
            using (RegistryKey k = Registry.LocalMachine.OpenSubKey(subKeyPath, false))
            {
                if (k != null)
                    existing = k.GetValue("") as string;
            }
            if (!string.IsNullOrEmpty(existing) && !string.Equals(existing, ClsidBraced, StringComparison.OrdinalIgnoreCase))
            {
                using (RegistryKey cfg = Registry.LocalMachine.CreateSubKey(LinkcoConfigKey))
                {
                    if (cfg != null)
                        cfg.SetValue(backupValueName, existing);
                }
            }
            using (RegistryKey k = Registry.LocalMachine.CreateSubKey(subKeyPath))
            {
                if (k != null)
                    k.SetValue("", ClsidBraced);
            }
        }

        public static void UnregisterPreviewHandler()
        {
            using (RegistryKey handlers = Registry.LocalMachine.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\PreviewHandlers", true))
            {
                if (handlers != null)
                    handlers.DeleteValue(ClsidBraced, false);
            }

            Registry.LocalMachine.DeleteSubKeyTree(@"Software\Classes\CLSID\" + ClsidBraced, false);
            Registry.LocalMachine.DeleteSubKeyTree(@"Software\Classes\LinkcoPDFEditor.Document\ShellEx\" + PreviewHandlerCategoryGuid, false);
            Registry.LocalMachine.DeleteSubKeyTree(@"Software\Classes\PdfCraft.Document\ShellEx\" + PreviewHandlerCategoryGuid, false);

            RestoreOrRemoveShellEx(@"Software\Classes\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousPdfPreviewHandler");
            RestoreOrRemoveShellEx(@"Software\Classes\SystemFileAssociations\.pdf\ShellEx\" + PreviewHandlerCategoryGuid, "PreviousSysPdfPreviewHandler");

            NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
        }

        private static void RestoreOrRemoveShellEx(string subKeyPath, string backupValueName)
        {
            string current = null;
            using (RegistryKey k = Registry.LocalMachine.OpenSubKey(subKeyPath, false))
            {
                if (k != null)
                    current = k.GetValue("") as string;
            }
            if (!string.Equals(current, ClsidBraced, StringComparison.OrdinalIgnoreCase))
                return;

            string backup = null;
            using (RegistryKey cfg = Registry.LocalMachine.OpenSubKey(LinkcoConfigKey, true))
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
                using (RegistryKey k = Registry.LocalMachine.CreateSubKey(subKeyPath))
                {
                    if (k != null)
                        k.SetValue("", candidate);
                }
            }
            else
            {
                Registry.LocalMachine.DeleteSubKeyTree(subKeyPath, false);
            }
        }

        private static bool ClsidExists(string clsid)
        {
            using (RegistryKey k = Registry.ClassesRoot.OpenSubKey(@"CLSID\" + clsid, false))
            {
                return k != null;
            }
        }
    }

    internal sealed class PreviewPaneControl : UserControl
    {
        private readonly Panel _topBar;
        private readonly Button _prevBtn;
        private readonly Button _nextBtn;
        private readonly Label _pageLabel;
        private readonly Button _zoomOutBtn;
        private readonly Button _zoomFitBtn;
        private readonly Button _zoomInBtn;
        private readonly Panel _canvasPanel;
        private readonly PictureBox _pictureBox;
        private readonly Label _statusLabel;

        private string _pdfPath;
        private int _currentPage = 1;
        private int _totalPages = 0;
        private float _zoomFactor = 1.0f;
        private bool _fitToWidth = true;
        private int _requestId = 0;
        private Process _activeProcess;
        private readonly object _procLock = new object();

        public PreviewPaneControl()
        {
            DoubleBuffered = true;
            BackColor = Color.FromArgb(242, 243, 245);

            _topBar = new Panel
            {
                Dock = DockStyle.Top,
                Height = 36,
                BackColor = Color.FromArgb(1, 19, 28)
            };

            _prevBtn = CreateNavButton("‹", 8, 5, 28);
            _prevBtn.Click += (s, e) => ChangePage(_currentPage - 1);

            _pageLabel = new Label
            {
                Text = "Linkco PDF Preview",
                ForeColor = Color.White,
                Font = new Font("Segoe UI", 9f, FontStyle.Bold),
                AutoSize = false,
                TextAlign = ContentAlignment.MiddleCenter,
                Location = new Point(40, 6),
                Size = new Size(135, 24)
            };

            _nextBtn = CreateNavButton("›", 178, 5, 28);
            _nextBtn.Click += (s, e) => ChangePage(_currentPage + 1);

            _zoomOutBtn = CreateNavButton("−", 216, 5, 28);
            _zoomOutBtn.Click += (s, e) => { _fitToWidth = false; _zoomFactor = Math.Max(0.4f, _zoomFactor - 0.2f); LayoutPageImage(); };

            _zoomFitBtn = CreateNavButton("Fit", 248, 5, 38);
            _zoomFitBtn.Click += (s, e) => { _fitToWidth = true; _zoomFactor = 1.0f; LayoutPageImage(); };

            _zoomInBtn = CreateNavButton("+", 290, 5, 28);
            _zoomInBtn.Click += (s, e) => { _fitToWidth = false; _zoomFactor = Math.Min(3.0f, _zoomFactor + 0.2f); LayoutPageImage(); };

            _topBar.Controls.AddRange(new Control[] { _prevBtn, _pageLabel, _nextBtn, _zoomOutBtn, _zoomFitBtn, _zoomInBtn });

            _canvasPanel = new Panel
            {
                Dock = DockStyle.Fill,
                AutoScroll = true,
                BackColor = Color.FromArgb(242, 243, 245)
            };
            _canvasPanel.Resize += (s, e) => LayoutPageImage();

            _pictureBox = new PictureBox
            {
                SizeMode = PictureBoxSizeMode.Zoom,
                BackColor = Color.White,
                Visible = false
            };
            _canvasPanel.Controls.Add(_pictureBox);

            _statusLabel = new Label
            {
                Dock = DockStyle.Fill,
                TextAlign = ContentAlignment.MiddleCenter,
                Font = new Font("Segoe UI", 9.5f, FontStyle.Regular),
                ForeColor = Color.FromArgb(90, 95, 105),
                Text = "Select a PDF document to preview.",
                Padding = new Padding(20),
                Visible = true
            };
            _canvasPanel.Controls.Add(_statusLabel);

            Controls.Add(_canvasPanel);
            Controls.Add(_topBar);
        }

        private static Button CreateNavButton(string text, int x, int y, int w)
        {
            Button b = new Button
            {
                Text = text,
                Location = new Point(x, y),
                Size = new Size(w, 25),
                FlatStyle = FlatStyle.Flat,
                ForeColor = Color.White,
                BackColor = Color.FromArgb(18, 38, 54),
                Font = new Font("Segoe UI", 8.5f, FontStyle.Bold),
                TabStop = false
            };
            b.FlatAppearance.BorderColor = Color.FromArgb(242, 36, 36);
            b.FlatAppearance.BorderSize = 1;
            return b;
        }

        public void SetThemeBackground(Color c)
        {
            _canvasPanel.BackColor = c;
        }

        public void LoadDocument(string filePath)
        {
            CancelActiveProcess();
            _pdfPath = filePath;
            _currentPage = 1;
            _totalPages = 0;
            _fitToWidth = true;
            _zoomFactor = 1.0f;

            if (string.IsNullOrEmpty(filePath) || !File.Exists(filePath))
            {
                ShowStatus("Unable to preview: the selected PDF file could not be found.");
                return;
            }

            RenderCurrentPageAsync();
        }

        public void CancelAndClear()
        {
            Interlocked.Increment(ref _requestId);
            CancelActiveProcess();
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
            RenderCurrentPageAsync();
        }

        private void RenderCurrentPageAsync()
        {
            int reqId = Interlocked.Increment(ref _requestId);
            string path = _pdfPath;
            int page = _currentPage;
            ShowStatus("Loading page " + page + "...");

            ThreadPool.QueueUserWorkItem(_ =>
            {
                string cliExe = LocateCliExecutable();
                if (string.IsNullOrEmpty(cliExe) || !File.Exists(cliExe))
                {
                    PostResult(reqId, null, 0, page, "Linkco PDF Preview engine (pdfcraft-cli.exe) was not found in the installation folder.");
                    return;
                }

                string tempDir = Path.Combine(Path.GetTempPath(), "LinkcoPdfPreview");
                Directory.CreateDirectory(tempDir);
                string outPng = Path.Combine(tempDir, "page-" + Process.GetCurrentProcess().Id + "-" + reqId + ".png");

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

                    Process proc = Process.Start(psi);
                    lock (_procLock)
                    {
                        _activeProcess = proc;
                    }

                    string stdout = proc.StandardOutput.ReadToEnd();
                    string stderr = proc.StandardError.ReadToEnd();
                    proc.WaitForExit(15000);

                    lock (_procLock)
                    {
                        if (_activeProcess == proc)
                            _activeProcess = null;
                    }

                    if (reqId != _requestId)
                        return;

                    ParseAndDeliver(reqId, stdout, stderr, outPng, page);
                }
                catch (Exception ex)
                {
                    PostResult(reqId, null, 0, page, "Preview failed: " + ex.Message);
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
            });
        }

        private void ParseAndDeliver(int reqId, string stdout, string stderr, string outPng, int requestedPage)
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
                        PostResult(reqId, bmp, total, actualPage, null);
                        return;
                    }
                }
                else if (code == "PASSWORD_REQUIRED")
                {
                    string msg = parts.Length > 3 ? parts[3] : "This PDF document is password-protected.";
                    PostResult(reqId, null, 0, 1, msg);
                    return;
                }
                else if (code == "EMPTY_PDF")
                {
                    PostResult(reqId, null, 0, 1, "This PDF document contains no pages.");
                    return;
                }
                else
                {
                    string msg = parts.Length > 3 ? parts[3] : "Unable to preview this PDF document.";
                    PostResult(reqId, null, 0, 1, "Unable to preview PDF: " + msg);
                    return;
                }
            }

            string errText = !string.IsNullOrEmpty(stderr) ? stderr.Trim() : "The file may be damaged or unsupported.";
            PostResult(reqId, null, 0, 1, "Unable to preview PDF: " + errText);
        }

        private static int ParseInt(string s, int fallback)
        {
            int v;
            return int.TryParse(s, out v) ? v : fallback;
        }

        private void PostResult(int reqId, Bitmap bmp, int totalPages, int actualPage, string errorMsg)
        {
            if (IsDisposed || !IsHandleCreated)
            {
                if (bmp != null) bmp.Dispose();
                return;
            }
            try
            {
                BeginInvoke((MethodInvoker)delegate
                {
                    if (reqId != _requestId || IsDisposed)
                    {
                        if (bmp != null) bmp.Dispose();
                        return;
                    }
                    if (bmp != null)
                    {
                        _totalPages = totalPages;
                        _currentPage = actualPage;
                        _pageLabel.Text = "Page " + _currentPage + " of " + _totalPages;
                        _prevBtn.Enabled = (_currentPage > 1);
                        _nextBtn.Enabled = (_currentPage < _totalPages);
                        ClearImage();
                        _pictureBox.Image = bmp;
                        _statusLabel.Visible = false;
                        _pictureBox.Visible = true;
                        LayoutPageImage();
                    }
                    else
                    {
                        _totalPages = 0;
                        _pageLabel.Text = "Linkco PDF Preview";
                        _prevBtn.Enabled = false;
                        _nextBtn.Enabled = false;
                        ShowStatus(errorMsg ?? "Unable to preview PDF.");
                    }
                });
            }
            catch
            {
                if (bmp != null) bmp.Dispose();
            }
        }

        private void ShowStatus(string message)
        {
            ClearImage();
            _pictureBox.Visible = false;
            _statusLabel.Text = message;
            _statusLabel.Visible = true;
        }

        private void ClearImage()
        {
            if (_pictureBox.Image != null)
            {
                Image old = _pictureBox.Image;
                _pictureBox.Image = null;
                old.Dispose();
            }
        }

        private void LayoutPageImage()
        {
            if (_pictureBox.Image == null || !_pictureBox.Visible)
                return;

            int availW = Math.Max(60, _canvasPanel.ClientSize.Width - 24);
            int imgW = _pictureBox.Image.Width;
            int imgH = _pictureBox.Image.Height;
            if (imgW <= 0 || imgH <= 0)
                return;

            int targetW = _fitToWidth ? availW : (int)(availW * _zoomFactor);
            int targetH = (int)Math.Round((double)imgH * targetW / imgW);
            int x = Math.Max(12, (_canvasPanel.ClientSize.Width - targetW) / 2);
            _pictureBox.Bounds = new Rectangle(x, 12, targetW, targetH);
        }

        private void CancelActiveProcess()
        {
            lock (_procLock)
            {
                if (_activeProcess != null)
                {
                    try
                    {
                        if (!_activeProcess.HasExited)
                            _activeProcess.Kill();
                    }
                    catch { }
                    _activeProcess = null;
                }
            }
        }

        private static string LocateCliExecutable()
        {
            string asmDir = Path.GetDirectoryName(Assembly.GetExecutingAssembly().Location);
            if (!string.IsNullOrEmpty(asmDir))
            {
                string candidate = Path.Combine(asmDir, "pdfcraft-cli.exe");
                if (File.Exists(candidate))
                    return candidate;
            }
            using (RegistryKey k = Registry.LocalMachine.OpenSubKey(LinkcoPdfPreviewHandler.LinkcoConfigKey, false))
            {
                if (k != null)
                {
                    string installDir = k.GetValue("InstallDir") as string;
                    if (!string.IsNullOrEmpty(installDir))
                    {
                        string candidate = Path.Combine(installDir, "pdfcraft-cli.exe");
                        if (File.Exists(candidate))
                            return candidate;
                    }
                }
            }
            return null;
        }
    }
}
