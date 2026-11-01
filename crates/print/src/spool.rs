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
/// the driver's default names otherwise. On non-CUPS platforms, `lang` is ignored.
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
// Gated with `test` as well as unix: `output_within` is compiled in test builds on every
// platform, and it needs this constant. Without it, cross-platform test builds fail to compile.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
const OUTPUT_CAP: u64 = 4 << 20;

/// How long to wait for the output reader after the command itself has exited. A descendant may
/// have inherited stdout and kept the pipe open; do not spend the command's remaining time waiting
/// for that descendant to close it.
// Gated with `test` as well as unix, for the same reason as `OUTPUT_CAP`: `output_within`
// needs it, and that function is compiled in test builds on every platform.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
const OUTPUT_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

/// Maximum PPD size read from the system spooler's PPD directory.
#[cfg(all(unix, not(target_arch = "wasm32")))]
const PPD_CAP: u64 = 16 << 20;

/// Longest PPD line [`localize`] will look at. Directives and translations are short; anything
/// longer is junk or hostile and is skipped rather than parsed.
const PPD_LINE_CAP: usize = 64 << 10;

/// Run `command` and collect its standard output, giving it `limit`: a command still running
/// then is killed and reaped, and the error is `TimedOut`. Standard input and error are not
/// connected. The exit status is not checked, as `Command::output` leaves it to the caller.
// Gated with `test` as well as unix so it compiles under test on every platform. This is
// necessary for cross-platform compiling: only CUPS runs a command today, but test builds on
// Windows and other targets must still build. The `allow` covers non-unix test builds, where
// nothing calls it because the tests that do are unix-only.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
#[cfg_attr(not(all(unix, not(target_arch = "wasm32"))), allow(dead_code))]
pub(crate) fn output_within(command: &mut std::process::Command, limit: std::time::Duration) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    use std::process::Stdio;

    // `Instant + Duration` panics on overflow; a limit that cannot be a deadline is refused.
    let Some(deadline) = std::time::Instant::now().checked_add(limit) else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the time limit is too large"));
    };
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
    // started can keep the pipe open after the command itself has exited or been killed. The
    // caller waits only for a bounded time; a reader blocked on an inherited pipe is left to
    // finish on its own when that process eventually closes the pipe.
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new().name("command output".into()).spawn(move || {
        let mut out = Vec::new();
        let read = (&mut stdout).take(OUTPUT_CAP).read_to_end(&mut out);
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        let _ = tx.send(read.map(|_| out));
    });
    if let Err(error) = reader {
        stop(&mut child);
        return Err(error);
    }

    let timed_out = || std::io::Error::new(std::io::ErrorKind::TimedOut, format!("{:?} did not answer within {limit:?}", command.get_program()));
    let output_uncollected = || {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("{:?} finished but its output was not closed within {OUTPUT_GRACE:?}", command.get_program()),
        )
    };

    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                // Once the child exits, allow only a short grace for its output to be collected.
                // A descendant that inherited stdout must not extend this to the full deadline.
                return match rx.recv_timeout(OUTPUT_GRACE) {
                    Ok(read) => read,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(output_uncollected()),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::other("the command's output was lost")),
                };
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                stop(&mut child);
                return Err(timed_out());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(error) => {
                stop(&mut child);
                return Err(error);
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
    let (queue, instance) = printer.split_once('/').map_or((printer, None), |(queue, instance)| (queue, Some(instance)));
    if !valid_destination(queue) || instance.is_some_and(|name| !valid_destination(name)) {
        return Vec::new();
    }

    let ppd = read_ppd(&ppd_dir.join(format!("{queue}.ppd")));
    let mut options = ppd.as_deref().map(|bytes| parse_ppd(&ppd_text(bytes))).unwrap_or_default();
    if let Some(bytes) = &ppd {
        localize(&mut options, bytes, lang);
    }

    let current = output_within(lpoptions.args(["-p", printer, "-l"]), limit)
        .map(|output| parse_lpoptions(&String::from_utf8_lossy(&output)))
        .unwrap_or_default();
    // `lpoptions -l` returns a few dozen lines at most; linear lookup avoids building an index.
    for (key, value) in current {
        if let Some(option) = options.iter_mut().find(|option| option.key == key)
            && option.choices.iter().any(|(choice, _)| *choice == value)
        {
            option.default = value;
        }
    }

    options
}

/// A CUPS queue or instance name, not a path or an `lpoptions` assignment.
/// Keep the accepted alphabet conservative before using the queue name in a PPD path.
#[cfg(any(test, all(unix, not(target_arch = "wasm32"))))]
fn valid_destination(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !matches!(name, "." | "..")
        && name.bytes().all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'/' | b'\\' | b':' | b'=' | b'"'))
}

/// Read a PPD without allowing a broken or unexpectedly large file to consume unbounded memory.
/// A file at or beyond `PPD_CAP` is truncated mid-line; `parse_ppd` drops any trailing partial directive.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn read_ppd(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::Read as _;

    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(PPD_CAP).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

pub fn submit(pdf: &[u8], job: &Job) -> Result<String, crate::PrintError> {
    craft_print::submit(pdf, job).map_err(|error| crate::PrintError::Spool(error.to_string()))
}

/// [`submit`], told `(sent, sheets)` as each sheet reaches the spooler (Windows; CUPS takes the
/// whole PDF at once and reports nothing).
pub fn submit_with_progress(pdf: &[u8], job: &Job, progress: &mut dyn FnMut(usize, usize)) -> Result<String, crate::PrintError> {
    craft_print::submit_with_progress(pdf, job, progress).map_err(|error| crate::PrintError::Spool(error.to_string()))
}

pub fn open_printer_preferences(printer: &str) -> Result<(), crate::PrintError> {
    craft_print::open_printer_preferences(printer).map_err(|error| crate::PrintError::Spool(error.to_string()))
}

/// The PPD language codes (`*cupsLanguages`, the `ll` of `*ll.Translation`) to try for a UI
/// language code, most specific first: `pt-br` → `pt_BR`, `pt`; `zh-hans` → `zh_CN`, `zh`.
///
/// The code is only ever matched against `*{code}.` line prefixes, but it comes from outside, so
/// anything other than ASCII letters and digits between separators (`/`, `\`, `.`, whitespace,
/// newlines) yields no codes at all.
// Read only by the CUPS path; the tests exercise it everywhere.
#[cfg_attr(not(all(unix, not(target_arch = "wasm32"))), allow(dead_code))]
fn ppd_languages(lang: &str) -> Vec<String> {
    let lang = lang.to_ascii_lowercase();
    let parts: Vec<&str> = lang.split(['-', '_']).filter(|part| !part.is_empty()).collect();
    if parts.iter().any(|part| !part.bytes().all(|byte| byte.is_ascii_alphanumeric())) {
        return Vec::new();
    }
    let Some(&primary) = parts.first() else {
        return Vec::new();
    };

    let region = match (primary, parts.get(1).copied()) {
        ("zh", Some("hans")) => Some("CN".to_string()),
        ("zh", Some("hant")) => Some("TW".to_string()),
        (_, Some(region)) if region.len() == 2 => Some(region.to_ascii_uppercase()),
        _ => None,
    };

    let mut languages = Vec::with_capacity(2);
    if let Some(region) = region {
        languages.push(format!("{primary}_{region}"));
    }
    languages.push(primary.to_string());
    languages
}

/// Parse the `Key/Label` value of an `OpenGroup` line into `(label, key)`.
///
/// The colon after `*OpenGroup` already separates the keyword from its value, and unlike
/// `*xx.Translation` there is no further `: ""` in the PPD grouping syntax, so colons inside the
/// label are kept (`Finishing/Finishing: options` has the label `Finishing: options`, exactly as
/// `parse_ppd` reports it). Only a trailing `: ""`, which some localized entries and the test
/// fixtures carry, is dropped.
#[cfg_attr(not(all(unix, not(target_arch = "wasm32"))), allow(dead_code))]
fn parse_open_group(spec: &str) -> Option<(String, String)> {
    let value = spec.trim();
    let value = value.strip_suffix(": \"\"").unwrap_or(value).trim_end();
    let (key, label) = value.split_once('/')?;
    Some((label.trim().to_string(), key.trim().to_string()))
}

/// Give `options` the driver's own names in `lang` from the PPD's localization lines
/// (`*ll.Translation Key/Label: ""` for options and groups, `*ll.Key choice/Label: ""` for
/// choices). Both default `*OpenGroup` and localized `*ll.OpenGroup` entries are supported.
/// Each line is read as UTF-8 on its own: drivers write translations this way even when other
/// PPD text is Latin-1. Lines longer than `PPD_LINE_CAP` are skipped. Names with no translation,
/// or an empty one, stay as they are.
// Read only by the CUPS path; the tests exercise it everywhere.
#[cfg_attr(not(all(unix, not(target_arch = "wasm32"))), allow(dead_code))]
pub(crate) fn localize(options: &mut [PrinterOption], ppd: &[u8], lang: &str) {
    use std::collections::HashMap;

    let languages = ppd_languages(lang);
    let code = languages.into_iter().find(|code| {
        let prefix = format!("*{code}.");
        ppd.split(|byte| *byte == b'\n').any(|line| line.starts_with(prefix.as_bytes()))
    });
    let Some(code) = code else {
        return;
    };
    if code == "en" {
        return;
    }

    let prefix = format!("*{code}.");
    let mut names = HashMap::<String, String>::new();
    let mut group_names = HashMap::<String, String>::new();
    let mut choices = HashMap::<(String, String), String>::new();
    let mut groups = HashMap::<String, String>::new();

    for line in ppd.split(|byte| *byte == b'\n') {
        if line.len() > PPD_LINE_CAP {
            continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        let text = text.trim_end_matches('\r');

        if let Some(rest) = text.strip_prefix("*OpenGroup:") {
            // Default group label → keyword; options carry the default group label.
            if let Some((label, key)) = parse_open_group(rest) {
                groups.insert(label, key);
            }
            continue;
        }

        let Some(rest) = text.strip_prefix(prefix.as_str()) else {
            continue;
        };

        if let Some(rest) = rest.strip_prefix("OpenGroup:") {
            // A localized OpenGroup directly maps its keyword to the translated label.
            // An empty label is no translation, so the existing name stays.
            if let Some((label, key)) = parse_open_group(rest)
                && !label.is_empty()
            {
                group_names.insert(key, label);
            }
            continue;
        }

        let Some((head, _value)) = rest.rsplit_once(':') else {
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

    for option in options {
        if let Some(label) = names.get(&option.key) {
            option.label.clone_from(label);
        }

        let group_key = groups.get(&option.group).unwrap_or(&option.group);
        if let Some(label) = group_names.get(group_key).or_else(|| names.get(group_key)) {
            option.group.clone_from(label);
        }

        for (choice, label) in &mut option.choices {
            if let Some(translated) = choices.get(&(option.key.clone(), choice.clone())) {
                label.clone_from(translated);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{localize, ppd_languages, valid_destination};
    use craft_print::PrinterOption;

    fn option(key: &str, label: &str) -> PrinterOption {
        PrinterOption {
            key: key.into(),
            label: label.into(),
            group: "General".into(),
            choices: vec![("A4".into(), "A4".into())],
            default: "A4".into(),
        }
    }

    #[test]
    fn destination_names_are_conservative() {
        for valid in ["Printer", "queue_1", "queue-1", "queue.name", "queue+name"] {
            assert!(valid_destination(valid), "expected {valid:?} to be valid");
        }

        for invalid in ["", ".", "..", "a/b", "a\\b", "a:b", "a=b", "a\"b", "has space"] {
            assert!(!valid_destination(invalid), "expected {invalid:?} to be invalid");
        }
        assert!(!valid_destination(&"a".repeat(65)));
    }

    #[test]
    fn language_codes_include_script_and_region_fallbacks() {
        assert_eq!(ppd_languages("pt-br"), vec!["pt_BR", "pt"]);
        assert_eq!(ppd_languages("zh-hans"), vec!["zh_CN", "zh"]);
        assert_eq!(ppd_languages("zh_Hant"), vec!["zh_TW", "zh"]);
        assert_eq!(ppd_languages("bg"), vec!["bg"]);
        assert!(ppd_languages("").is_empty());
    }

    #[test]
    fn language_codes_reject_path_characters() {
        for bad in ["../x", "a/b", "a\\b", "bg\n*en", "x y", "bg.", "..", "pt-br/../x"] {
            assert!(ppd_languages(bad).is_empty(), "expected {bad:?} to yield no codes");
        }
        // Valid input is unchanged.
        assert_eq!(ppd_languages("pt-br"), vec!["pt_BR", "pt"]);
    }

    #[test]
    fn localize_ignores_a_lang_with_path_characters() {
        let mut options = vec![option("PageSize", "Paper size")];
        let ppd = "*bg.Translation PageSize/Размер на хартията: \"\"\n".as_bytes();

        localize(&mut options, ppd, "../x");

        assert_eq!(options[0].label, "Paper size");
    }

    #[test]
    fn localize_translates_default_open_group_options_and_choices() {
        let mut options = vec![option("PageSize", "Paper size")];
        let ppd = r#"
*OpenGroup: General/General: ""
*bg.Translation General/Общи: ""
*bg.Translation PageSize/Размер на хартията: ""
*bg.PageSize A4/А4: ""
"#
        .as_bytes();

        localize(&mut options, ppd, "bg");

        assert_eq!(options[0].group, "Общи");
        assert_eq!(options[0].label, "Размер на хартията");
        assert_eq!(options[0].choices[0].1, "А4");
    }

    #[test]
    fn localize_accepts_language_specific_open_group_entries() {
        let mut options = vec![option("PageSize", "Paper size")];
        let ppd = r#"
*bg.OpenGroup: General/Общи: ""
*bg.Translation PageSize/Размер на хартията: ""
*bg.PageSize A4/А4: ""
"#
        .as_bytes();

        localize(&mut options, ppd, "bg");

        assert_eq!(options[0].group, "Общи");
        assert_eq!(options[0].label, "Размер на хартията");
        assert_eq!(options[0].choices[0].1, "А4");
    }

    #[test]
    fn localize_keeps_group_name_for_empty_localized_open_group_label() {
        let mut options = vec![option("PageSize", "Paper size")];
        let ppd = r#"
*bg.OpenGroup: General/: ""
*bg.Translation PageSize/Размер на хартията: ""
*bg.PageSize A4/А4: ""
"#
        .as_bytes();

        localize(&mut options, ppd, "bg");

        assert_eq!(options[0].group, "General");
        assert_eq!(options[0].label, "Размер на хартията");
        assert_eq!(options[0].choices[0].1, "А4");
    }

    /// A PPD with one option in a group whose label is `group`, translated through
    /// `*bg.Translation Finishing/…`.
    fn finishing_ppd(group: &str) -> Vec<u8> {
        format!(
            "*OpenGroup: Finishing/{group}\n\
             *OpenUI *Staple/Staple: PickOne\n\
             *DefaultStaple: None\n\
             *Staple None/Off: \"\"\n\
             *CloseUI: *Staple\n\
             *CloseGroup: Finishing\n\
             *bg.Translation Finishing/Завършване: \"\"\n"
        )
        .into_bytes()
    }

    #[test]
    fn open_group_value_keeps_colons() {
        assert_eq!(super::parse_open_group(" Finishing/Finishing: options"), Some(("Finishing: options".into(), "Finishing".into())));
        assert_eq!(super::parse_open_group(" General/General: \"\""), Some(("General".into(), "General".into())));
        assert_eq!(super::parse_open_group(" General/: \"\""), Some((String::new(), "General".into())));
        assert_eq!(super::parse_open_group(" no-slash"), None);
    }

    // parse_ppd → localize regression: the group label carries a colon, as parse_ppd reports it.
    #[test]
    fn localize_group_label_with_colon() {
        let ppd = finishing_ppd("Finishing: options");
        let mut options = super::parse_ppd(&super::ppd_text(&ppd));
        assert!(!options.is_empty(), "fixture should parse to at least one option");
        assert_eq!(options[0].group, "Finishing: options");

        localize(&mut options, &ppd, "bg");

        assert_eq!(options[0].group, "Завършване");
    }

    #[test]
    fn localize_group_label_without_colon_control() {
        let ppd = finishing_ppd("Finishing");
        let mut options = super::parse_ppd(&super::ppd_text(&ppd));
        assert!(!options.is_empty(), "fixture should parse to at least one option");
        assert_eq!(options[0].group, "Finishing");

        localize(&mut options, &ppd, "bg");

        assert_eq!(options[0].group, "Завършване");
    }

    #[test]
    fn localize_skips_an_overlong_line_and_keeps_going() {
        let mut options = vec![option("PageSize", "Paper size"), option("Copies", "Copies"), option("Duplex", "Duplex")];

        let mut ppd = "*bg.Translation PageSize/Размер: \"\"\n".as_bytes().to_vec();
        ppd.extend_from_slice(b"*bg.Translation Copies/");
        ppd.extend(std::iter::repeat_n(b'x', 2 << 20)); // 2 MiB label, far past the line cap
        ppd.extend_from_slice(b": \"\"\n");
        ppd.extend_from_slice("*bg.Translation Duplex/Двустранно: \"\"\n".as_bytes());

        localize(&mut options, &ppd, "bg");

        assert_eq!(options[0].label, "Размер");
        assert_eq!(options[1].label, "Copies", "the overlong line must be skipped");
        assert_eq!(options[2].label, "Двустранно");
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn output_within_returns_stdout() {
        use super::output_within;
        use std::process::Command;
        use std::time::Duration;

        let mut cmd = Command::new("printf");
        cmd.arg("hello");
        assert_eq!(output_within(&mut cmd, Duration::from_secs(1)).unwrap(), b"hello");
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn output_within_caps_large_output_without_deadlock() {
        use super::{OUTPUT_CAP, output_within};
        use std::process::Command;
        use std::time::Duration;

        // Twice the cap: the pipe fills, the reader keeps draining, the command still exits.
        let mut cmd = Command::new("head");
        cmd.args(["-c", "8388608", "/dev/zero"]);
        let out = output_within(&mut cmd, Duration::from_secs(10)).unwrap();
        assert_eq!(out.len() as u64, OUTPUT_CAP);
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn output_within_refuses_a_limit_that_overflows() {
        use super::output_within;
        use std::process::Command;
        use std::time::Duration;

        let mut cmd = Command::new("true");
        let err = output_within(&mut cmd, Duration::MAX).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn output_within_kills_on_deadline() {
        use super::output_within;
        use std::process::Command;
        use std::time::{Duration, Instant};

        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let start = Instant::now();
        let err = output_within(&mut cmd, Duration::from_millis(50)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(err.to_string().contains("did not answer within"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn output_within_does_not_wait_for_grandchild_pipe() {
        use super::output_within;
        use std::process::Command;
        use std::time::{Duration, Instant};

        // `sh` exits immediately while backgrounded `sleep 1` inherits stdout and keeps the pipe
        // open past `OUTPUT_GRACE` (100 ms) without leaving a long-lived orphan process.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hi; sleep 1 &"]);
        let start = Instant::now();
        let err = output_within(&mut cmd, Duration::from_secs(5)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(err.to_string().contains("finished but its output was not closed"));
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}
