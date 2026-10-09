//! The system print spooler. On macOS and Linux this is CUPS: printers come from `lpstat`, jobs
//! are piped to `lp` with the job options (copies, collation, duplex, colour). On Windows the
//! printers come from WMI and the sheets are rasterized and drawn through .NET's `PrintDocument`
//! (see `WIN_PRINT_SCRIPT`), which works with every driver, not only PDF-capable ones. Other
//! platforms report that printing isn't available yet; the print-ready PDF can still be saved.

use crate::PrintError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printer {
    pub name: String,
    pub default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Duplex {
    #[default]
    Off,
    LongEdge,
    ShortEdge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// `None` = the system default printer.
    pub printer: Option<String>,
    pub copies: u32,
    pub collate: bool,
    pub duplex: Duplex,
    pub grayscale: bool,
    pub title: String,
}

impl Default for Job {
    fn default() -> Self {
        Job { printer: None, copies: 1, collate: true, duplex: Duplex::Off, grayscale: false, title: "PdfCraft".into() }
    }
}

/// Parse `lpstat -p -d` output.
pub fn parse_lpstat(out: &str) -> Vec<Printer> {
    let default = out.lines().find_map(|l| l.strip_prefix("system default destination:")).map(|s| s.trim().to_string());
    out.lines()
        .filter_map(|l| l.strip_prefix("printer "))
        .filter_map(|l| l.split_whitespace().next())
        .map(|n| Printer { name: n.to_string(), default: default.as_deref() == Some(n) })
        .collect()
}

/// The `lp` arguments for a job. There is no file argument: `lp` reads the job from stdin.
pub fn lp_args(job: &Job) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(p) = &job.printer {
        a.extend(["-d".to_string(), p.clone()]);
    }
    a.extend(["-n".to_string(), job.copies.clamp(1, 999).to_string()]);
    a.extend(["-t".to_string(), job.title.clone()]);
    let mut opt = |o: &str| a.extend(["-o".to_string(), o.to_string()]);
    opt(if job.collate { "collate=true" } else { "collate=false" });
    opt(match job.duplex {
        Duplex::Off => "sides=one-sided",
        Duplex::LongEdge => "sides=two-sided-long-edge",
        Duplex::ShortEdge => "sides=two-sided-short-edge",
    });
    if job.grayscale {
        opt("print-color-mode=monochrome");
    }
    // The sheets are already laid out at their final size.
    opt("fit-to-page=false");
    a
}

/// `lpstat -p -d`, forced to print untranslated messages so [`parse_lpstat`] can read them.
///
/// `LC_ALL`/`LANG=C` is enough on Linux. macOS CUPS ignores them and follows the user's
/// interface language (`AppleLanguages`) unless `SOFTWARE` is set, in which case it uses `LANG`.
pub fn lpstat_command() -> std::process::Command {
    let mut c = std::process::Command::new("lpstat");
    c.args(["-p", "-d"]).env("LC_ALL", "C").env("LANG", "C").env("SOFTWARE", "PdfCraft");
    c
}

/// The printers the system knows (empty when there are none or no spooler).
pub fn printers() -> Vec<Printer> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        match lpstat_command().output() {
            Ok(o) => parse_lpstat(&String::from_utf8_lossy(&o.stdout)),
            Err(_) => Vec::new(),
        }
    }
    #[cfg(windows)]
    {
        match win_printers_command().output() {
            Ok(o) => parse_win_printers(&String::from_utf8_lossy(&o.stdout)),
            Err(_) => Vec::new(),
        }
    }
    #[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
    {
        Vec::new()
    }
}

/// Send a print-ready PDF to the spooler. Returns the spooler's message (the job id).
pub fn submit(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        submit_via(std::process::Command::new("lp"), pdf, job)
    }
    #[cfg(windows)]
    {
        submit_windows(pdf, job)
    }
    #[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
    {
        let _ = (pdf, job);
        Err(PrintError::Spool("printing to a printer isn't available on this platform yet; save the print-ready PDF instead".into()))
    }
}

/// [`submit`] with the spooler command given, so tests can stand in for `lp`.
///
/// The job goes to `lp` on stdin, never through a file: a predictable job folder in a shared
/// temp directory lets another local user read printed documents or swap the file before `lp`
/// reads it.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
pub(crate) fn submit_via(mut lp: std::process::Command, pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = lp
        .args(lp_args(job))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    let stdin = child.stdin.take();
    // Feed the job on its own thread while collecting the output, so neither side can fill a
    // pipe and wait on the other. Dropping `stdin` at the end closes it: the end of the job.
    let fed_and_out = std::thread::scope(|s| {
        let feeder = std::thread::Builder::new().name("print job".into()).spawn_scoped(s, move || match stdin {
            Some(mut stdin) => stdin.write_all(pdf),
            None => Err(std::io::Error::other("no pipe to the spooler")),
        });
        match feeder {
            Ok(feeder) => {
                let out = child.wait_with_output();
                Ok((feeder.join(), out))
            }
            Err(e) => {
                // No thread to feed it: stop `lp` rather than leave it with an empty job.
                let _ = child.kill();
                let _ = child.wait();
                Err(PrintError::Spool(format!("the print job could not be sent to the spooler: {e}")))
            }
        }
    });
    let (fed, out) = fed_and_out?;
    let out = out.map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    if !out.status.success() {
        // `lp` may refuse before reading the job (an unknown printer); its message says why,
        // and the broken pipe that leaves behind does not.
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(PrintError::Spool(if err.is_empty() { "the print job was refused".into() } else { err }));
    }
    match fed {
        Ok(Ok(())) => Ok(job_message(&out.stdout)),
        Ok(Err(e)) => Err(PrintError::Spool(format!("the print job could not be sent to the spooler: {e}"))),
        Err(_) => Err(PrintError::Spool("the print job could not be sent to the spooler".into())),
    }
}

/// `lp`'s reply ("request id is Office-12"), without the "(0 file(s))" CUPS adds when the job came
/// on stdin: it reads as if nothing was sent.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
pub(crate) fn job_message(stdout: &[u8]) -> String {
    let s = String::from_utf8_lossy(stdout);
    let s = s.trim();
    s.strip_suffix("(0 file(s))").map_or(s, str::trim_end).to_string()
}

// ---- Windows -------------------------------------------------------------------------------

/// Resolution the sheets are rasterized at for a Windows print job.
#[cfg(any(test, windows))]
const WIN_DPI: f32 = 200.0;

/// Printers and which one is the default, one per line as `True<TAB>Name` (`False` for the rest).
#[cfg(windows)]
const WIN_PRINTERS_SCRIPT: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
Get-CimInstance Win32_Printer | ForEach-Object { \"$($_.Default)`t$($_.Name)\" }";

/// Parse `WIN_PRINTERS_SCRIPT`'s output.
pub fn parse_win_printers(out: &str) -> Vec<Printer> {
    out.lines()
        .filter_map(|l| {
            let (default, name) = l.trim_end_matches('\r').split_once('\t')?;
            let name = name.trim();
            (!name.is_empty()).then(|| Printer { name: name.to_string(), default: default.trim().eq_ignore_ascii_case("true") })
        })
        .collect()
}

/// The PowerShell script that prints a job. It reads the job from stdin (see [`write_frames`])
/// and the options from `PDFCRAFT_*` environment variables, so no document or printer name is
/// ever spliced into the script text or written to a file.
#[cfg(windows)]
const WIN_PRINT_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
try {
  Add-Type -AssemblyName System.Drawing
  $in = [Console]::OpenStandardInput()
  function ReadExact([int]$n) {
    $b = New-Object byte[] $n; $o = 0
    while ($o -lt $n) { $r = $in.Read($b, $o, $n - $o); if ($r -le 0) { throw 'the print data ended early' }; $o += $r }
    ,$b
  }
  $script:pages = [BitConverter]::ToInt32((ReadExact 4), 0)
  $script:idx = 0; $script:bmp = $null; $script:err = $null
  $pd = New-Object System.Drawing.Printing.PrintDocument
  $pd.DocumentName = $env:PDFCRAFT_TITLE
  if ($env:PDFCRAFT_PRINTER) { $pd.PrinterSettings.PrinterName = $env:PDFCRAFT_PRINTER }
  if (-not $pd.PrinterSettings.IsValid) { throw 'the printer is not available' }
  $pd.PrintController = New-Object System.Drawing.Printing.StandardPrintController
  $pd.PrinterSettings.Copies = [int16]$env:PDFCRAFT_COPIES
  $pd.PrinterSettings.Collate = ($env:PDFCRAFT_COLLATE -eq '1')
  if ($env:PDFCRAFT_DUPLEX -ne 'off' -and $pd.PrinterSettings.CanDuplex) {
    $pd.PrinterSettings.Duplex = if ($env:PDFCRAFT_DUPLEX -eq 'long') { [System.Drawing.Printing.Duplex]::Vertical } else { [System.Drawing.Printing.Duplex]::Horizontal }
  }
  $gray = ($env:PDFCRAFT_GRAY -eq '1')
  if ($gray) { $pd.DefaultPageSettings.Color = $false }
  $pd.add_QueryPageSettings({
    param($s, $e)
    try {
      $h = ReadExact 16
      $wpx = [BitConverter]::ToInt32($h, 0); $hpx = [BitConverter]::ToInt32($h, 4)
      $script:wh = [BitConverter]::ToInt32($h, 8); $script:hh = [BitConverter]::ToInt32($h, 12)
      $data = ReadExact ($wpx * $hpx * 4)
      if ($script:bmp) { $script:bmp.Dispose() }
      $bm = New-Object System.Drawing.Bitmap($wpx, $hpx, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
      $bd = $bm.LockBits((New-Object System.Drawing.Rectangle(0, 0, $wpx, $hpx)), [System.Drawing.Imaging.ImageLockMode]::WriteOnly, $bm.PixelFormat)
      [Runtime.InteropServices.Marshal]::Copy($data, 0, $bd.Scan0, $data.Length)
      $bm.UnlockBits($bd)
      $script:bmp = $bm
      $land = $script:wh -gt $script:hh
      $pw = if ($land) { $script:hh } else { $script:wh }
      $ph = if ($land) { $script:wh } else { $script:hh }
      $paper = $null
      foreach ($p in $pd.PrinterSettings.PaperSizes) {
        if ([math]::Abs($p.Width - $pw) -le 6 -and [math]::Abs($p.Height - $ph) -le 6) { $paper = $p; break }
      }
      if (-not $paper) { $paper = New-Object System.Drawing.Printing.PaperSize('PdfCraft', $pw, $ph) }
      $e.PageSettings.PaperSize = $paper
      $e.PageSettings.Landscape = $land
      if ($gray) { $e.PageSettings.Color = $false }
    } catch { $script:err = $_.Exception.Message; $e.Cancel = $true }
  })
  $pd.add_PrintPage({
    param($s, $e)
    try {
      $g = $e.Graphics
      $g.TranslateTransform(-$e.PageSettings.HardMarginX, -$e.PageSettings.HardMarginY)
      $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
      $g.DrawImage($script:bmp, 0, 0, $script:wh, $script:hh)
      $script:idx++
      $e.HasMorePages = ($script:idx -lt $script:pages)
    } catch { $script:err = $_.Exception.Message; $e.Cancel = $true; $e.HasMorePages = $false }
  })
  $pd.Print()
  if ($script:err) { throw $script:err }
  [Console]::Out.WriteLine("$($script:idx) page(s) sent")
} catch {
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 1
}
"#;

/// Why [`write_frames`] stopped.
#[cfg(any(test, windows))]
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum Frames {
    /// The document can't be printed (nothing to print, a page that won't render).
    Document(String),
    /// The reader went away, usually because the print script failed first and said why.
    Pipe(String),
}

/// Rasterize `pdf` and write it as the stream `WIN_PRINT_SCRIPT` reads: the page count (`i32`),
/// then per page `[width px, height px, width and height in 1/100 inch]` (four `i32`) followed by
/// `width * height` BGRA pixels. All little endian. Returns the page count.
#[cfg(any(test, windows))]
pub(crate) fn write_frames(pdf: &[u8], out: &mut impl std::io::Write) -> Result<usize, Frames> {
    use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
    let mut renderer = PageRenderer::new(std::sync::Arc::new(pdf.to_vec()), RenderConfig::default());
    let pages = renderer.page_count();
    if pages == 0 {
        return Err(Frames::Document("there are no pages to print".into()));
    }
    let io = |e: std::io::Error| Frames::Pipe(format!("the print job could not be sent to the spooler: {e}"));
    out.write_all(&i32::try_from(pages).unwrap_or(i32::MAX).to_le_bytes()).map_err(io)?;
    let scale = WIN_DPI / 72.0;
    for page in 0..pages {
        let r = renderer.render(RenderRequest { page, scale, ..Default::default() });
        if let Some(e) = r.error {
            return Err(Frames::Document(format!("page {} could not be rendered: {e}", page + 1)));
        }
        // A page past the renderer's size caps comes back smaller than asked for and prints
        // proportionally smaller; such pages are far beyond any paper.
        let hundredths = |px: u32| i32::try_from((f64::from(px) * 100.0 / f64::from(WIN_DPI)).round() as u64).unwrap_or(i32::MAX);
        let (w, h) = (
            i32::try_from(r.width).map_err(|_| Frames::Document("page too large".into()))?,
            i32::try_from(r.height).map_err(|_| Frames::Document("page too large".into()))?,
        );
        for v in [w, h, hundredths(r.width), hundredths(r.height)] {
            out.write_all(&v.to_le_bytes()).map_err(io)?;
        }
        // The page is opaque (white background), so premultiplied RGBA is plain RGBA: swap to BGRA.
        let bgra: Vec<u8> = r.rgba.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [b, g, r, 255]).collect();
        out.write_all(&bgra).map_err(io)?;
    }
    out.flush().map_err(io)?;
    Ok(pages)
}

/// The `PDFCRAFT_*` environment `WIN_PRINT_SCRIPT` reads.
#[cfg(any(test, windows))]
pub(crate) fn win_job_env(job: &Job) -> Vec<(&'static str, String)> {
    vec![
        ("PDFCRAFT_PRINTER", job.printer.clone().unwrap_or_default()),
        ("PDFCRAFT_COPIES", job.copies.clamp(1, 999).to_string()),
        ("PDFCRAFT_COLLATE", if job.collate { "1" } else { "0" }.into()),
        (
            "PDFCRAFT_DUPLEX",
            match job.duplex {
                Duplex::Off => "off",
                Duplex::LongEdge => "long",
                Duplex::ShortEdge => "short",
            }
            .into(),
        ),
        ("PDFCRAFT_GRAY", if job.grayscale { "1" } else { "0" }.into()),
        ("PDFCRAFT_TITLE", job.title.clone()),
    ]
}

/// `-EncodedCommand` takes the script as base64 of its UTF-16LE text.
#[cfg(any(test, windows))]
pub(crate) fn encode_command(script: &str) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(c.get(1).copied().unwrap_or(0)) << 8) | u32::from(c.get(2).copied().unwrap_or(0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(windows)]
fn powershell(script: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encode_command(script)]);
    // CREATE_NO_WINDOW: no console flashes up from the GUI app.
    c.creation_flags(0x0800_0000);
    c
}

#[cfg(windows)]
fn win_printers_command() -> std::process::Command {
    powershell(WIN_PRINTERS_SCRIPT)
}

#[cfg(windows)]
fn submit_windows(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    use std::process::Stdio;
    let mut cmd = powershell(WIN_PRINT_SCRIPT);
    cmd.envs(win_job_env(job)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    let stdin = child.stdin.take();
    // Render and feed on a thread while collecting the output, so neither side can fill a pipe
    // and wait on the other. Dropping `stdin` at the end of the thread closes it.
    let (fed, out) = std::thread::scope(|s| {
        let feeder = std::thread::Builder::new().name("print job".into()).spawn_scoped(s, move || match stdin {
            Some(mut stdin) => write_frames(pdf, &mut std::io::BufWriter::new(&mut stdin)),
            None => Err(Frames::Pipe("no pipe to the spooler".into())),
        });
        match feeder {
            Ok(feeder) => {
                let out = child.wait_with_output();
                (feeder.join().unwrap_or_else(|_| Err(Frames::Document("the print job could not be prepared".into()))), out)
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                (Err(Frames::Pipe(format!("the print job could not be sent to the spooler: {e}"))), Err(e))
            }
        }
    });
    // A page that can't be rendered is the root cause; PowerShell's "data ended early" is not.
    if let Err(Frames::Document(e)) = &fed {
        return Err(PrintError::Spool(e.clone()));
    }
    let out = out.map_err(|e| PrintError::Spool(format!("the print spooler is not available: {e}")))?;
    if !out.status.success() {
        // The script may refuse before reading the job (an unknown printer); its message says
        // why, and the broken pipe that leaves behind does not.
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(PrintError::Spool(if err.is_empty() { "the print job was refused".into() } else { err }));
    }
    if let Err(Frames::Pipe(e)) = fed {
        return Err(PrintError::Spool(e));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
