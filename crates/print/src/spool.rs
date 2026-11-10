//! System print spooler supplied by craft-print. PDF composition and permission checks
//! stay in PdfCraft; these adapters retain the application's print-error surface.

pub use craft_print::{
    Duplex, HANDLED_OPTIONS, HAS_PRINTER_PREFERENCES, Job, Printer, PrinterOption, Scaling, lp_args, lpstat_command, parse_lpoptions, parse_lpstat,
    parse_lpstat_e, parse_ppd, parse_windows_printers, ppd_text, printers_parsed,
};

#[cfg(any(windows, test))]
mod windows_duplex;

/// Windows: the in-box DLLs printing draws and describes jobs with. The executables delay-load
/// them (`apps/pdfcraft{,-cli}/build.rs`, and `build.rs` for this crate's tests: the same list)
/// so a Windows installation without them still starts (#2630); a call into a missing
/// delay-loaded DLL would end the process, so printing checks for them first.
#[cfg(windows)]
pub(crate) const PRINT_DLLS: [&str; 2] = ["windows.data.pdf.dll", "prntvpt.dll"];

/// The first print DLL missing from `system32`, if any.
#[cfg(windows)]
pub(crate) fn missing_print_dll_in(system32: &std::path::Path) -> Option<&'static str> {
    PRINT_DLLS.into_iter().find(|dll| !system32.join(dll).is_file())
}

#[cfg(all(windows, test))]
thread_local! {
    /// Tests: the folder the print DLLs are looked for in, instead of the system's, so the
    /// public entry points can be checked against a Windows that lacks them.
    pub(crate) static SYSTEM32_FOR_TEST: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
    /// Tests: how many times [`printers`], [`printer_queues`] or [`two_sided`] asked the system's
    /// printer backend, so a test can tell the guard answered without relying on the machine
    /// having no printers.
    pub(crate) static PRINTER_BACKEND_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The first print DLL this Windows installation lacks. `None` when all are there, and when the
/// system folder is unknown (the loader then decides, as before).
fn missing_print_dll() -> Option<&'static str> {
    #[cfg(windows)]
    {
        #[cfg(test)]
        if let Some(dir) = SYSTEM32_FOR_TEST.with(|d| d.borrow().clone()) {
            return missing_print_dll_in(&dir);
        }
        let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir"))?;
        missing_print_dll_in(&std::path::Path::new(&root).join("System32"))
    }
    #[cfg(not(windows))]
    None
}

fn unavailable(dll: &str) -> crate::PrintError {
    crate::PrintError::Spool(format!("printing needs {dll}, which this Windows installation lacks; use Save as PDF instead"))
}

/// Keeps two threads of this process out of the Windows PrintTicket provider (`PTOpenProvider`
/// and the calls on its handle) at the same time. When two threads make the process's first calls
/// together, winspool can keep a pointer to a device capabilities cache it has freed, and a later
/// call crashes the process. All entry points in this process that query driver capabilities
/// or submit jobs share this process-wide lock.
#[cfg(windows)]
static PRINT_TICKET_PROVIDER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(windows)]
pub(crate) fn print_ticket_provider() -> std::sync::MutexGuard<'static, ()> {
    PRINT_TICKET_PROVIDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The system's printers; none on a Windows installation without the print DLLs.
pub fn printers() -> Vec<Printer> {
    if missing_print_dll().is_some() {
        return Vec::new();
    }
    #[cfg(all(windows, test))]
    PRINTER_BACKEND_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    #[cfg(windows)]
    let _provider = print_ticket_provider();
    craft_print::printers()
}

/// The system's print queues without asking any driver, so a Print dialog's list never waits on
/// a printer (#2192); none on a Windows installation without the print DLLs, as [`printers`].
pub fn printer_queues() -> Vec<Printer> {
    if missing_print_dll().is_some() {
        return Vec::new();
    }
    #[cfg(all(windows, test))]
    PRINTER_BACKEND_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    craft_print::printer_queues()
}

/// Whether `printer`'s driver takes two-sided settings, read for that one printer; it can wait
/// on the device, so call it off the UI thread. Unknown on a Windows installation without the
/// print DLLs: reading it goes through them.
pub fn two_sided(printer: &str) -> Option<bool> {
    if missing_print_dll().is_some() {
        return None;
    }
    #[cfg(all(windows, test))]
    PRINTER_BACKEND_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    #[cfg(windows)]
    let _provider = print_ticket_provider();
    craft_print::two_sided(printer)
}

/// The driver's job options for a printer (CUPS: its PPD, with the queue's current defaults);
/// empty when it has none or on other platforms. Reads files and runs `lpoptions`, so call it
/// when the user asks, not every frame.
pub fn printer_options(printer: &str) -> Vec<PrinterOption> {
    printer_options_in(printer, "en")
}

/// [`printer_options`] with the driver's own names in `lang` (a UI language code such as `bg`,
/// `pt-br` or `zh-hans`) where its PPD has them (`*bg.Translation`, `*bg.InputSlot Tray2/…`);
/// the driver's default names otherwise.
pub fn printer_options_in(printer: &str, lang: &str) -> Vec<PrinterOption> {
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    {
        printer_options_via_within(printer, std::path::Path::new("/etc/cups/ppd"), std::process::Command::new("lpoptions"), LPOPTIONS_LIMIT, lang)
    }
    #[cfg(not(all(unix, not(target_arch = "wasm32"))))]
    {
        let _ = lang;
        craft_print::printer_options(printer)
    }
}

/// How long `lpoptions` may take: a CUPS that has stopped answering must not hold whoever asked
/// (#1818). After this the PPD's own defaults stand.
#[cfg(all(unix, not(target_arch = "wasm32")))]
const LPOPTIONS_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The most of a command's output that is kept (`lpoptions -l` is a few kilobytes).
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
const OUTPUT_CAP: u64 = 4 << 20;

/// Run `command` and collect its standard output, giving it `limit`: a command still running
/// then is killed and reaped, and the error is `TimedOut`. Standard input and error are not
/// connected. The exit status is not checked, as `Command::output` leaves it to the caller.
/// Only CUPS runs a command today; the tests cover every desktop platform.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
pub(crate) fn output_within(command: &mut std::process::Command, limit: std::time::Duration) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    use std::process::Stdio;
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let stop = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let Some(mut stdout) = child.stdout.take() else {
        stop(&mut child);
        return Err(std::io::Error::other("the command's output could not be read"));
    };
    // Read on a thread of its own, so a full pipe never holds the command while the deadline is
    // kept here. Past the cap the rest is drained and dropped, so the command still finishes.
    // The output comes back over a channel rather than through `join`: a process the command
    // started can keep the pipe open after the command itself has exited or been killed, and
    // the wait for the output must end at the deadline too. Such a reader is left to finish on
    // its own when that process closes the pipe.
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new().name("command output".into()).spawn(move || {
        let mut out = Vec::new();
        let read = (&mut stdout).take(OUTPUT_CAP).read_to_end(&mut out);
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        let _ = tx.send(read.map(|_| out));
    });
    if let Err(e) = reader {
        stop(&mut child);
        return Err(e);
    }
    let timed_out = || std::io::Error::new(std::io::ErrorKind::TimedOut, format!("the command did not answer within {limit:?}"));
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                // A moment's grace, so output written just before an exit at the deadline counts.
                let left = deadline.saturating_duration_since(std::time::Instant::now()).max(std::time::Duration::from_millis(100));
                return match rx.recv_timeout(left) {
                    Ok(read) => read,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(timed_out()),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::other("the command's output was lost")),
                };
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                stop(&mut child);
                return Err(timed_out());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => {
                stop(&mut child);
                return Err(e);
            }
        }
    }
}

/// Read a destination's options with the PPD directory and command supplied, so tests need
/// neither a system queue nor a physical printer. CUPS instances share their queue's PPD but
/// have their own defaults, queried with the complete `queue/instance` destination. `limit` is
/// how long the command may take before the PPD's defaults stand on their own.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub(crate) fn printer_options_via_within(
    printer: &str,
    ppd_dir: &std::path::Path,
    mut lpoptions: std::process::Command,
    limit: std::time::Duration,
    lang: &str,
) -> Vec<PrinterOption> {
    let (queue, instance) = printer.split_once('/').map_or((printer, None), |(q, i)| (q, Some(i)));
    // Retain the conservative name checks, applied to each component. Neither component is
    // a path: reject dot components, backslashes and any additional separator before I/O. A
    // leading `-` is refused too: `lpoptions` parses its argv with getopt, so `-p -E -l` would
    // read `-E` as an option rather than as the destination, and `--` cannot help because it
    // ends option parsing before the `-p` that names the queue.
    let valid = |name: &str| {
        !name.is_empty()
            && name.len() <= 64
            && !name.starts_with('-')
            && !matches!(name, "." | "..")
            && name.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b'\\' | b':' | b'=' | b'"'))
    };
    if !valid(queue) || instance.is_some_and(|name| !valid(name)) {
        return Vec::new();
    }
    const PPD_CAP: u64 = 16 << 20;
    let ppd = std::fs::File::open(ppd_dir.join(format!("{queue}.ppd"))).ok().and_then(|f| {
        use std::io::Read as _;
        let mut bytes = Vec::new();
        f.take(PPD_CAP).read_to_end(&mut bytes).ok().map(|_| bytes)
    });
    let mut options = ppd.as_deref().map(|b| parse_ppd(&ppd_text(b))).unwrap_or_default();
    let lpoptions_out = output_within(lpoptions.args(["-p", printer, "-l"]), limit).ok();
    let current = lpoptions_out.as_deref().map(|out| parse_lpoptions(&String::from_utf8_lossy(out))).unwrap_or_default();
    if options.is_empty() {
        // The PPD is missing or unreadable (#4699). The same `lpoptions -l` listing carries every
        // option with its choices and default; build a fallback list from it.
        if let Some(out) = &lpoptions_out {
            options = parse_lpoptions_list(&String::from_utf8_lossy(out));
        }
    }
    // After the fallback: a readable PPD whose `*OpenUI` blocks yielded nothing still supplies the
    // translations, and `localize` leaves options it cannot translate untouched.
    if let Some(b) = &ppd {
        localize(&mut options, b, lang);
    }
    for (key, value) in current {
        if let Some(o) = options.iter_mut().find(|o| o.key == key)
            && o.choices.iter().any(|(c, _)| *c == value)
        {
            o.default = value;
        }
    }
    options
}

/// Caps that match [`craft_print::parse_ppd`]: a malformed spooler answer is not allowed to
/// dominate memory or the dialog.
#[cfg(all(unix, not(target_arch = "wasm32")))]
const MAX_LPOPTIONS_OPTIONS: usize = 400;
#[cfg(all(unix, not(target_arch = "wasm32")))]
const MAX_LPOPTIONS_CHOICES: usize = 400;
#[cfg(all(unix, not(target_arch = "wasm32")))]
const MAX_LPOPTIONS_LABEL: usize = 256;

/// A PPD keyword: printable ASCII without spaces, `/`, `:`, `=` or `"`.
#[cfg(all(unix, not(target_arch = "wasm32")))]
// The three name helpers below mirror `is_ppd_keyword`, `name_label` and `ppd_label` in craft-print,
// which are private there: keep them in step with that crate, or they will accept in one reader what
// the other refuses.
fn is_lpoptions_keyword(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b':' | b'=' | b'"'))
}

/// `Name/Label` → (`Name`, `Label`), the label falling back to the name.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn lpoptions_name_label(s: &str) -> (&str, &str) {
    match s.split_once('/') {
        Some((n, l)) if !l.trim().is_empty() => (n.trim(), l.trim()),
        Some((n, _)) => (n.trim(), n.trim()),
        None => (s.trim(), s.trim()),
    }
}

/// `label`, cut to [`MAX_LPOPTIONS_LABEL`] characters.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn lpoptions_label(label: &str) -> String {
    label.chars().take(MAX_LPOPTIONS_LABEL).collect()
}

/// Whether `choice` is a custom-value template. For a choice the user has to type, `lpoptions -l`
/// prints `Custom`, `Custom.<TYPE>` (`Custom.PASSCODE`, `Custom.STRING`, …) or `Custom.WIDTHxHEIGHT`
/// in place of the choices, and the value is not something Craft can collect: it would be sent to
/// the driver as the literal text. The prefix is reserved by CUPS, case-insensitively.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn is_lpoptions_custom_template(choice: &str) -> bool {
    match choice.split_once('.') {
        Some((name, _)) => name.eq_ignore_ascii_case("Custom"),
        None => choice.eq_ignore_ascii_case("Custom"),
    }
}

/// Build [`PrinterOption`]s from the `lpoptions -p NAME -l` listing (`Key/Label: a *b c`). This is
/// the fallback when the PPD cannot be read, so there is no group information: every option lands in
/// one unnamed group, and the listing cannot say which options sit under `InstallableOptions`, so an
/// optional-hardware entry that [`parse_ppd`] drops may appear here. The starred choice is the
/// driver's default. Options Craft handles itself ([`HANDLED_OPTIONS`]) and custom-value templates
/// ([`is_lpoptions_custom_template`]) are skipped.
///
/// The line is split at its **last** colon, not its first: `list_group()` prints the option as
/// `"%s/%s:"` from the PPD's keyword and translated text, then every choice as a bare `" %s"`, so
/// text such as `Supporto: Tipo` carries colons while a choice keyword cannot. Splitting at the
/// first colon would turn the rest of the label into invented choices.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn parse_lpoptions_list(out: &str) -> Vec<PrinterOption> {
    let mut options = Vec::new();
    for line in out.lines() {
        let line = line.trim_end_matches('\r');
        let Some((head, choices)) = line.rsplit_once(':') else {
            continue;
        };
        let (key, label) = lpoptions_name_label(head);
        if !is_lpoptions_keyword(key) || HANDLED_OPTIONS.contains(&key) {
            continue;
        }
        let mut choice_list = Vec::new();
        let mut default = String::new();
        // Set when the choice CUPS marks is a template the dialog cannot collect. The printer keeps
        // that value, so standing in the first retained choice would show a default the job does not
        // use: only options changed away from the default are submitted.
        let mut marked_template = false;
        for token in choices.split_whitespace() {
            let starred = token.starts_with('*');
            let (choice_key, choice_label) = lpoptions_name_label(token.strip_prefix('*').unwrap_or(token));
            if !is_lpoptions_keyword(choice_key) || is_lpoptions_custom_template(choice_key) {
                marked_template |= starred && is_lpoptions_custom_template(choice_key);
                continue;
            }
            if starred {
                default = choice_key.to_string();
            }
            if choice_list.len() < MAX_LPOPTIONS_CHOICES && choice_list.iter().all(|(c, _)| c != choice_key) {
                choice_list.push((choice_key.to_string(), lpoptions_label(choice_label)));
            }
        }
        if choice_list.is_empty() || options.len() >= MAX_LPOPTIONS_OPTIONS {
            continue;
        }
        if default.is_empty() && !marked_template {
            // No choice marked at all: the dialog needs something selected, and the listing has
            // nothing better than the first one to offer.
            default = choice_list.first().map(|(c, _)| c.clone()).unwrap_or_default();
        }
        options.push(PrinterOption { key: key.to_string(), label: lpoptions_label(label), group: String::new(), choices: choice_list, default });
    }
    options
}

/// The keys the macOS print dialog names a job's title and owner under.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const JOB_NAME_KEY: &str = "com.apple.print.JobInfo.PMJobName";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const JOB_OWNER_KEY: &str = "com.apple.print.JobInfo.PMJobOwner";

/// `text` as a job option's value: `lp_args` passes on only printable ASCII, up to 64 bytes,
/// without `/ : = "`, and CUPS would take `'` and `\` as quoting and `{` as the start of a
/// collection, so all of these become `_`. Text with nothing left of it, such as a title in
/// another script, takes `fallback`: the job must still name something. So does a bare `true` or
/// `false`, which CUPS would send as a boolean, not the name a filter reads.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn job_option_value(text: &str, fallback: &str) -> String {
    let kept = |c: char| c.is_ascii_graphic() && !matches!(c, '/' | ':' | '=' | '"' | '\'' | '\\' | '{');
    let value: String = text.chars().take(64).map(|c| if kept(c) { c } else { '_' }).collect();
    if !text.chars().any(kept) || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
        return fallback.to_string();
    }
    value
}

/// `job` naming its title and `owner` as a job from the macOS print dialog does. `lp` sends
/// neither, and a driver's filter can depend on them: Kyocera's crashes without them, so nothing
/// prints, while the same file sent with both does (#5743). A value the caller set stands: the
/// last of a repeated option wins in CUPS, which compares their names without case.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn with_job_info(job: &Job, owner: Option<&str>) -> Job {
    let mut named = job.clone();
    for (key, value) in [(JOB_NAME_KEY, job.title.as_str()), (JOB_OWNER_KEY, owner.unwrap_or_default())] {
        if !job.options.iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
            named.options.push((key.to_string(), job_option_value(value, "PdfCraft")));
        }
    }
    named
}

/// The job as the system spooler takes it.
fn spooled(job: &Job) -> Job {
    #[cfg(target_os = "macos")]
    {
        with_job_info(job, std::env::var("USER").ok().as_deref())
    }
    #[cfg(not(target_os = "macos"))]
    {
        job.clone()
    }
}

/// Submit a PDF to the system print spooler for the configured `job`.
pub fn submit(pdf: &[u8], job: &Job) -> Result<String, crate::PrintError> {
    if let Some(dll) = missing_print_dll() {
        return Err(unavailable(dll));
    }
    #[cfg(windows)]
    let _provider = print_ticket_provider();
    #[cfg(windows)]
    let resolved = windows_duplex::job(job);
    #[cfg(windows)]
    let job = resolved.as_ref();
    let pdf = spooled_pdf(pdf, crate::font_names::RENAME_FOR_SPOOLER);
    craft_print::submit(&pdf, &spooled(job)).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

/// [`submit`], told `(sent, sheets)` as each sheet reaches the spooler (Windows; CUPS takes the
/// whole PDF at once and reports nothing).
pub fn submit_with_progress(pdf: &[u8], job: &Job, progress: &mut dyn FnMut(usize, usize)) -> Result<String, crate::PrintError> {
    if let Some(dll) = missing_print_dll() {
        return Err(unavailable(dll));
    }
    #[cfg(windows)]
    let _provider = print_ticket_provider();
    #[cfg(windows)]
    let resolved = windows_duplex::job(job);
    #[cfg(windows)]
    let job = resolved.as_ref();
    let pdf = spooled_pdf(pdf, crate::font_names::RENAME_FOR_SPOOLER);
    craft_print::submit_with_progress(&pdf, &spooled(job), progress).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

/// The PDF as the system spooler takes it: with `rename` (Windows), its embedded TrueType and
/// OpenType programs renamed, so Windows can't draw a subset's glyph numbers with an installed
/// font of the same name (#3132, [`crate::font_names`]). A PDF that can't be read back goes as it
/// is: the renaming only protects the text from Windows, and the job shouldn't fail over it.
pub(crate) fn spooled_pdf(pdf: &[u8], rename: bool) -> std::borrow::Cow<'_, [u8]> {
    if !rename {
        return std::borrow::Cow::Borrowed(pdf);
    }
    match crate::font_names::distinct(pdf) {
        Ok(Some(renamed)) => std::borrow::Cow::Owned(renamed),
        Ok(None) | Err(_) => std::borrow::Cow::Borrowed(pdf),
    }
}

/// Opens the printer driver's native Printing Preferences window (Windows `printui.dll`).
pub fn open_printer_preferences(printer: &str) -> Result<(), crate::PrintError> {
    craft_print::open_printer_preferences(printer).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

/// A named paper with dimensions in points `(width, height)`.
pub type Paper = (String, (f64, f64));

/// The paper sizes supported by `printer` as reported by the system / printer driver.
/// Empty when the printer reports none or on platforms where driver capabilities are not queried.
#[cfg(windows)]
pub fn printer_papers(printer: &str) -> Vec<Paper> {
    // A call into a missing delay-loaded PrintTicket DLL would terminate the process (#2630).
    if missing_print_dll().is_some() {
        return Vec::new();
    }
    let _provider = print_ticket_provider();
    printer_papers_internal(printer)
}

#[cfg(windows)]
fn printer_papers_internal(printer: &str) -> Vec<Paper> {
    #[cfg(test)]
    PRINTER_BACKEND_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    use winprint::ticket::{FeatureOptionPack as _, FeatureOptionPackWithPredefined as _};
    let Ok(devices) = winprint::printer::PrinterDevice::all() else { return Vec::new() };
    let Some(device) = devices.into_iter().find(|d| d.name() == printer) else { return Vec::new() };
    let Ok(capabilities) = winprint::ticket::PrintCapabilities::fetch(&device) else { return Vec::new() };
    let mut papers = Vec::new();
    for media in capabilities.page_media_sizes() {
        let size = media.size();
        let w_pts = (size.width_in_micron() as f64) * 72.0 / 25_400.0;
        let h_pts = (size.height_in_micron() as f64) * 72.0 / 25_400.0;
        if w_pts.is_finite() && h_pts.is_finite() && w_pts >= 72.0 && h_pts >= 72.0 && w_pts <= 14400.0 && h_pts <= 14400.0 {
            let name = media.display_name().map(str::to_string).unwrap_or_else(|| {
                if let Some(predefined) = media.as_predefined_name() { format!("{predefined:?}") } else { format!("{w_pts:.1} x {h_pts:.1} pt") }
            });
            let w_pts = (w_pts * 100.0).round() / 100.0;
            let h_pts = (h_pts * 100.0).round() / 100.0;
            if !papers.iter().any(|(n, (w, h)): &(String, (f64, f64))| n == &name && (*w - w_pts).abs() < 0.1 && (*h - h_pts).abs() < 0.1) {
                papers.push((name, (w_pts, h_pts)));
            }
        }
    }
    papers
}

/// The paper sizes supported by `printer` (fallback for non-Windows platforms).
#[cfg(not(windows))]
pub fn printer_papers(_printer: &str) -> Vec<Paper> {
    Vec::new()
}

/// Match `paper_pts` against detected driver `papers` within a 1.0 pt tolerance.
#[cfg(windows)]
fn resolve_paper_name(papers: &[Paper], paper_pts: (f64, f64)) -> Paper {
    let w_pts = (paper_pts.0 * 100.0).round() / 100.0;
    let h_pts = (paper_pts.1 * 100.0).round() / 100.0;
    let name = papers
        .iter()
        .find(|(_, (w, h))| ((*w - w_pts).abs() <= 1.0 && (*h - h_pts).abs() <= 1.0) || ((*w - h_pts).abs() <= 1.0 && (*h - w_pts).abs() <= 1.0))
        .map(|(n, _)| n.clone())
        .or_else(|| crate::matching_paper(paper_pts).and_then(|idx| crate::PAPERS.get(idx).map(|(n, _)| n.to_string())))
        .unwrap_or_default();
    (name, (w_pts, h_pts))
}

/// The default paper size configured for `printer` in the printer properties / DEVMODE.
/// Returns `Some((name, (width, height)))` if readable.
pub fn printer_default_paper(printer: &str) -> Option<Paper> {
    printer_paper_info(printer).1
}

/// Reads the supported paper sizes and the default paper size configured for `printer`.
pub fn printer_paper_info(printer: &str) -> (Vec<Paper>, Option<Paper>) {
    if missing_print_dll().is_some() {
        return (Vec::new(), None);
    }
    #[cfg(windows)]
    let _provider = print_ticket_provider();
    #[cfg(windows)]
    let papers = printer_papers_internal(printer);
    #[cfg(not(windows))]
    let papers = printer_papers(printer);
    #[cfg(windows)]
    let default_paper = {
        let settings = craft_print::rgb::default_settings(printer).ok();
        let page = settings.as_ref().and_then(|s| craft_print::rgb::page(printer, Some(s)).ok());
        page.map(|p| resolve_paper_name(&papers, p.paper))
    };
    #[cfg(not(windows))]
    let default_paper = {
        let _ = &papers;
        None
    };
    (papers, default_paper)
}

/// Shows the printer driver's Page Setup / DocumentProperties dialog (Windows).
///
/// Returns the paper size and orientation selected by the user. Other per-job driver
/// settings (such as tray or quality) are not captured; general driver settings should
/// be configured through [`open_printer_preferences`] (Properties…).
pub fn show_printer_properties(printer: &str, parent: Option<isize>) -> Result<Option<Paper>, crate::PrintError> {
    if let Some(dll) = missing_print_dll() {
        return Err(unavailable(dll));
    }
    #[cfg(windows)]
    {
        let devmode = match craft_print::rgb::settings_dialog(printer, None, parent) {
            Ok(Some(dm)) => dm,
            Ok(None) => return Ok(None),
            Err(_) => {
                open_printer_preferences(printer)?;
                return Ok(None);
            }
        };
        let page = craft_print::rgb::page(printer, Some(&devmode)).map_err(|e| crate::PrintError::Spool(e.to_string()))?;
        let papers = printer_papers(printer);
        Ok(Some(resolve_paper_name(&papers, page.paper)))
    }
    #[cfg(not(windows))]
    {
        let _ = (printer, parent);
        Ok(None)
    }
}

/// The PPD language codes (`*cupsLanguages`, the `ll` of `*ll.Translation`) to try for a UI
/// language code, most specific first: `pt-br` → `pt_BR`, `pt`; `zh-hans` → `zh_CN`, `zh`.
// Read only by the CUPS path; the tests exercise it everywhere.
#[cfg_attr(not(all(unix, not(target_arch = "wasm32"))), allow(dead_code))]
fn ppd_languages(lang: &str) -> Vec<String> {
    let lang = lang.to_ascii_lowercase();
    let mut parts = lang.split(['-', '_']).filter(|p| !p.is_empty());
    let Some(primary) = parts.next() else {
        return Vec::new();
    };
    let region = match (primary, parts.next()) {
        ("zh", Some("hans")) => Some("CN".to_string()),
        ("zh", Some("hant")) => Some("TW".to_string()),
        (_, Some(r)) if r.len() == 2 => Some(r.to_ascii_uppercase()),
        _ => None,
    };
    region.map(|r| format!("{primary}_{r}")).into_iter().chain(std::iter::once(primary.to_string())).collect()
}

/// Give `options` the driver's own names in `lang` from the PPD's localization lines
/// (`*ll.Translation Key/Label: ""` for options and groups, `*ll.Key choice/Label: ""` for
/// choices). Each line is read as UTF-8 on its own: drivers write them so in files whose other
/// text is Latin-1. Names with no translation, or an empty one, stay as they are.
// Read only by the CUPS path; the tests exercise it everywhere.
#[cfg_attr(not(all(unix, not(target_arch = "wasm32"))), allow(dead_code))]
pub(crate) fn localize(options: &mut [PrinterOption], ppd: &[u8], lang: &str) {
    use std::collections::HashMap;
    let Some(code) = ppd_languages(lang).into_iter().find(|code| {
        let prefix = format!("*{code}.");
        ppd.split(|b| *b == b'\n').any(|l| l.starts_with(prefix.as_bytes()))
    }) else {
        return;
    };
    if code == "en" {
        return;
    }
    let prefix = format!("*{code}.");
    let mut names: HashMap<String, String> = HashMap::new();
    let mut choices: HashMap<(String, String), String> = HashMap::new();
    let mut groups: HashMap<String, String> = HashMap::new();
    for line in ppd.split(|b| *b == b'\n') {
        let Some(text) = std::str::from_utf8(line).ok() else {
            continue;
        };
        let text = text.trim_end_matches('\r');
        if let Some(rest) = text.strip_prefix("*OpenGroup:") {
            // Group label → keyword, since options carry their group's label.
            let rest = rest.trim();
            if let Some((key, label)) = rest.split_once('/') {
                groups.insert(label.trim().to_string(), key.trim().to_string());
            }
            continue;
        }
        let Some(rest) = text.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let Some((head, _)) = rest.split_once(':') else {
            continue;
        };
        let Some((main, spec)) = head.split_once(' ') else {
            continue;
        };
        let Some((key, label)) = spec.split_once('/') else {
            continue;
        };
        let label = label.trim();
        if label.is_empty() {
            continue;
        }
        if main == "Translation" {
            names.insert(key.trim().to_string(), label.to_string());
        } else {
            choices.insert((main.to_string(), key.trim().to_string()), label.to_string());
        }
    }
    for o in options.iter_mut() {
        if let Some(label) = names.get(&o.key) {
            o.label = label.clone();
        }
        if let Some(label) = groups.get(&o.group).and_then(|key| names.get(key)) {
            o.group = label.clone();
        }
        for (choice, label) in o.choices.iter_mut() {
            if let Some(t) = choices.get(&(o.key.clone(), choice.clone())) {
                *label = t.clone();
            }
        }
    }
}
