//! System print spooler supplied by craft-print. PDF composition and permission checks
//! stay in PdfCraft; these adapters retain the application's print-error surface.

pub use craft_print::{
    Duplex, HANDLED_OPTIONS, HAS_PRINTER_PREFERENCES, Job, Printer, PrinterOption, Scaling, lp_args, lpstat_command, parse_lpoptions, parse_lpstat,
    parse_lpstat_e, parse_ppd, parse_windows_printers, ppd_text, printers, printers_parsed,
};

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
    // a path: reject dot components, backslashes and any additional separator before I/O.
    let valid = |name: &str| {
        !name.is_empty()
            && name.len() <= 64
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
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn parse_lpoptions_list(out: &str) -> Vec<PrinterOption> {
    let mut options = Vec::new();
    for line in out.lines() {
        let line = line.trim_end_matches('\r');
        let Some((head, choices)) = line.split_once(':') else {
            continue;
        };
        let (key, label) = lpoptions_name_label(head);
        if !is_lpoptions_keyword(key) || HANDLED_OPTIONS.contains(&key) {
            continue;
        }
        let mut choice_list = Vec::new();
        let mut default = String::new();
        for token in choices.split_whitespace() {
            let starred = token.strip_prefix('*').unwrap_or(token);
            let (choice_key, choice_label) = lpoptions_name_label(starred);
            if !is_lpoptions_keyword(choice_key) || is_lpoptions_custom_template(choice_key) {
                continue;
            }
            if token.starts_with('*') {
                default = choice_key.to_string();
            }
            if choice_list.len() < MAX_LPOPTIONS_CHOICES && choice_list.iter().all(|(c, _)| c != choice_key) {
                choice_list.push((choice_key.to_string(), lpoptions_label(choice_label)));
            }
        }
        if choice_list.is_empty() || options.len() >= MAX_LPOPTIONS_OPTIONS {
            continue;
        }
        if !choice_list.iter().any(|(c, _)| *c == default) {
            default = choice_list.first().map(|(c, _)| c.clone()).unwrap_or_default();
        }
        options.push(PrinterOption { key: key.to_string(), label: lpoptions_label(label), group: String::new(), choices: choice_list, default });
    }
    options
}

pub fn submit(pdf: &[u8], job: &Job) -> Result<String, crate::PrintError> {
    craft_print::submit(pdf, job).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

/// [`submit`], told `(sent, sheets)` as each sheet reaches the spooler (Windows; CUPS takes the
/// whole PDF at once and reports nothing).
pub fn submit_with_progress(pdf: &[u8], job: &Job, progress: &mut dyn FnMut(usize, usize)) -> Result<String, crate::PrintError> {
    craft_print::submit_with_progress(pdf, job, progress).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

pub fn open_printer_preferences(printer: &str) -> Result<(), crate::PrintError> {
    craft_print::open_printer_preferences(printer).map_err(|e| crate::PrintError::Spool(e.to_string()))
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
