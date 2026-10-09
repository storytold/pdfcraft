//! The system print spooler. On macOS and Linux this is CUPS: printers come from `lpstat`, jobs
//! are piped to `lp` with the job options (copies, collation, duplex, colour). Other platforms
//! report that printing isn't available yet; the print-ready PDF can still be saved.

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
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
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
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
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
