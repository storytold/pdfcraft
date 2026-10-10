//! Read-only diagnostics for the macOS document-integration boundary.
//!
//! The command intentionally reports evidence and limitations instead of trying to register an
//! application, refresh LaunchServices, index a file, or install a Quick Look provider. That
//! keeps it safe to run from CI, a release checkout, or a developer's normal machine.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow, bail};

const INFO_KEYS: [&str; 8] = ["Title", "Author", "Subject", "Keywords", "Creator", "Producer", "CreationDate", "ModDate"];
const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PDF_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PDF_PAGES: usize = 32;
const MAX_METADATA_CHARS: usize = 1024;
const MAX_PROBE_OUTPUT_BYTES: usize = 4096;

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    help: bool,
    bundle: Option<PathBuf>,
    pdf: Option<PathBuf>,
}

#[derive(Debug)]
struct Probe {
    status: Option<i32>,
    stdout: String,
    stderr: String,
    error: Option<String>,
}

/// `cargo xtask macos-integration [--bundle APP] [--pdf FILE]`.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let options = parse_args(args)?;
    if options.help {
        println!("Usage: cargo xtask macos-integration [--bundle PdfCraft.app] [--pdf file.pdf]");
        return Ok(());
    }
    let mut report = String::new();
    writeln!(report, "PdfCraft macOS integration diagnostics")?;
    writeln!(report, "target: {} / {}", std::env::consts::OS, std::env::consts::ARCH)?;
    writeln!(report, "os version: {}", os_version())?;

    match options.bundle.as_deref() {
        Some(bundle) => write_bundle_report(&mut report, bundle)?,
        None => writeln!(report, "bundle: not supplied (use --bundle /path/to/PdfCraft.app)")?,
    }

    write_tool_report(&mut report, "quick look", "qlmanage", &["-m", "plugins"])?;
    match options.pdf.as_deref() {
        Some(pdf) => {
            write_pdf_report(&mut report, pdf)?;
            write_spotlight_report(&mut report, pdf)?;
        }
        None => {
            writeln!(report, "pdf: not supplied (use --pdf /path/to/synthetic.pdf)")?;
            writeln!(report, "spotlight: skipped (a PDF path is required for test-import)")?;
        }
    }

    print!("{report}");
    Ok(())
}

fn parse_args(args: &[String]) -> anyhow::Result<Options> {
    let mut options = Options::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bundle" => options.bundle = Some(next_path(&mut it, "--bundle")?),
            "--pdf" => options.pdf = Some(next_path(&mut it, "--pdf")?),
            "-h" | "--help" => options.help = true,
            other => bail!("unknown argument `{other}` (expected --bundle, --pdf or --help)"),
        }
    }
    Ok(options)
}

fn next_path<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> anyhow::Result<PathBuf> {
    it.next().map(PathBuf::from).ok_or_else(|| anyhow!("{flag} needs a path"))
}

fn os_version() -> String {
    let probe = probe("sw_vers", &["-productVersion"]);
    if probe.status == Some(0) {
        let value = combined_output(&probe).trim().to_owned();
        if !value.is_empty() {
            return value;
        }
    }
    "unavailable".into()
}

fn write_bundle_report(report: &mut String, bundle: &Path) -> anyhow::Result<()> {
    writeln!(report, "bundle: {}", bundle.display())?;
    if !bundle.is_dir() {
        writeln!(report, "  status: missing or not a directory")?;
        return Ok(());
    }
    let contents = bundle.join("Contents");
    let plist = contents.join("Info.plist");
    let plist_text = fs::read_to_string(&plist).ok();
    if let Some(text) = plist_text.as_deref() {
        let bundle_id = plist_value(text, "CFBundleIdentifier").unwrap_or_else(|| "unavailable".into());
        let minimum = plist_value(text, "LSMinimumSystemVersion").unwrap_or_else(|| "unavailable".into());
        writeln!(report, "  bundle identifier: {bundle_id}")?;
        writeln!(report, "  minimum macOS: {minimum}")?;
        writeln!(report, "  PDF registration: {}", pdf_registration(text))?;
        let executable = plist_value(text, "CFBundleExecutable").unwrap_or_else(|| "PdfCraft".into());
        write_architecture_report(report, &contents.join("MacOS").join(executable))?;
    } else if plist.exists() {
        writeln!(report, "  Info.plist: present but not UTF-8 XML (binary plist inspection unavailable)")?;
    } else {
        writeln!(report, "  Info.plist: missing")?;
    }

    let plugins = contents.join("PlugIns");
    if plugins.is_dir() {
        let entries = bounded_entries(&plugins, 64)?;
        if entries.is_empty() {
            writeln!(report, "  nested code: PlugIns directory is empty")?;
        } else {
            writeln!(report, "  nested code: {}", entries.join(", "))?;
        }
    } else {
        writeln!(report, "  nested code: none under Contents/PlugIns")?;
    }

    let signature = probe("codesign", &["--verify", "--deep", "--strict", bundle.to_string_lossy().as_ref()]);
    writeln!(report, "  signature: {}", probe_status(&signature, "valid"))?;
    Ok(())
}

fn write_architecture_report(report: &mut String, executable: &Path) -> anyhow::Result<()> {
    if !executable.is_file() {
        writeln!(report, "  architectures: executable missing ({})", executable.display())?;
        return Ok(());
    }
    let probe = probe("lipo", &["-info", executable.to_string_lossy().as_ref()]);
    writeln!(report, "  architectures: {}", probe_status(&probe, "reported"))?;
    Ok(())
}

fn write_tool_report(report: &mut String, label: &str, program: &str, args: &[&str]) -> anyhow::Result<()> {
    let probe = probe(program, args);
    let status = if probe.status == Some(0) && useful_output(&probe).is_none() {
        "available (no output returned)".into()
    } else {
        probe_status(&probe, "available")
    };
    writeln!(report, "{label}: {status}")?;
    if let Some(output) = useful_output(&probe) {
        writeln!(report, "  output: {}", output.replace('\n', "\\n"))?;
    }
    Ok(())
}

fn write_spotlight_report(report: &mut String, pdf: &Path) -> anyhow::Result<()> {
    let probe = probe("mdimport", &["-t", "-d2", pdf.to_string_lossy().as_ref()]);
    let status = if probe.status == Some(0) && useful_output(&probe).is_none_or(|o| o.contains("Nothing returned by server")) {
        "no metadata returned".to_owned()
    } else {
        probe_status(&probe, "test-import succeeded")
    };
    writeln!(report, "spotlight: {status}")?;
    if let Some(output) = useful_output(&probe) {
        writeln!(report, "  output: {}", output.replace('\n', "\\n"))?;
    }
    Ok(())
}

fn write_pdf_report(report: &mut String, pdf: &Path) -> anyhow::Result<()> {
    let metadata = fs::metadata(pdf).with_context(|| format!("stat PDF {}", pdf.display()))?;
    if metadata.len() > MAX_PDF_BYTES {
        bail!("PDF {} exceeds the {MAX_PDF_BYTES}-byte diagnostic limit", pdf.display());
    }
    let bytes = fs::read(pdf).with_context(|| format!("read PDF {}", pdf.display()))?;
    let doc = lopdf::Document::load_mem(&bytes).with_context(|| format!("parse PDF {}", pdf.display()))?;
    let pages = doc.get_pages();
    writeln!(report, "pdf: {} ({} bytes)", pdf.display(), bytes.len())?;
    writeln!(report, "  pages: {}", pages.len())?;
    writeln!(report, "  metadata:")?;
    for key in INFO_KEYS {
        let value = info_value(&doc, key).unwrap_or_else(|| "<absent>".into());
        writeln!(report, "    {key}: {value}")?;
    }
    let page_numbers: Vec<u32> = pages.keys().copied().take(MAX_PDF_PAGES).collect();
    if page_numbers.is_empty() {
        writeln!(report, "  text: no pages")?;
    } else {
        match doc.extract_text_with_limit(&page_numbers, MAX_PDF_TEXT_BYTES) {
            Ok(text) => {
                let sample: String = text.chars().take(240).collect();
                writeln!(report, "  text: {} bytes from first {} page(s); sample: {:?}", text.len(), page_numbers.len(), sample)?;
            }
            Err(err) => writeln!(report, "  text: unavailable ({err})")?,
        }
        if pages.len() > MAX_PDF_PAGES {
            writeln!(report, "  text limit: first {MAX_PDF_PAGES} pages inspected")?;
        }
    }
    Ok(())
}

fn info_value(doc: &lopdf::Document, key: &str) -> Option<String> {
    let info = doc.trailer.get(b"Info").ok()?;
    let (_, info) = doc.dereference(info).ok()?;
    let dict = info.as_dict().ok()?;
    let value = dict.get(key.as_bytes()).ok()?;
    let (_, value) = doc.dereference(value).ok()?;
    let bytes = value.as_str().ok()?;
    Some(bounded_text(&pdf_string(bytes), MAX_METADATA_CHARS))
}

fn pdf_string(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = utf16.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn plist_value(plist: &str, key: &str) -> Option<String> {
    let key = format!("<key>{key}</key>");
    let after = plist.split_once(&key)?.1;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].to_owned())
}

fn pdf_registration(plist: &str) -> String {
    let docs = plist.split_once("<key>CFBundleDocumentTypes</key>").map(|(_, rest)| rest).unwrap_or_default();
    let pdf = docs
        .split("<dict>")
        .map(|chunk| chunk.split("</dict>").next().unwrap_or_default())
        .find(|chunk| chunk.contains("<string>com.adobe.pdf</string>"));
    match pdf {
        Some(entry) => {
            let role = if entry.contains("<string>Editor</string>") { "Editor" } else { "unknown role" };
            let rank = if entry.contains("<string>Alternate</string>") { "Alternate" } else { "unknown rank" };
            format!("com.adobe.pdf (role={role}, rank={rank})")
        }
        None => "com.adobe.pdf not declared".into(),
    }
}

fn bounded_entries(dir: &Path, limit: usize) -> anyhow::Result<Vec<String>> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("read nested-code directory {}", dir.display()))?
        .take(limit)
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries)
}

fn probe(program: &str, args: &[&str]) -> Probe {
    match Command::new(program).args(args).output() {
        Ok(output) => Probe {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            error: None,
        },
        Err(error) => Probe { status: None, stdout: String::new(), stderr: String::new(), error: Some(error.to_string()) },
    }
}

fn probe_status(probe: &Probe, success: &str) -> String {
    match (probe.status, probe.error.as_deref()) {
        (Some(0), _) => success.into(),
        (Some(code), _) => format!("failed (exit {code})"),
        (None, Some(error)) => format!("unavailable ({error})"),
        (None, None) => "unavailable".into(),
    }
}

fn combined_output(probe: &Probe) -> String {
    let mut output = probe.stdout.clone();
    if !probe.stderr.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&probe.stderr);
    }
    output
}

fn useful_output(probe: &Probe) -> Option<String> {
    let output = combined_output(probe).trim().to_owned();
    (!output.is_empty()).then(|| output.chars().take(MAX_PROBE_OUTPUT_BYTES).collect())
}

fn bounded_text(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut bounded: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        bounded.push('…');
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::{Options, parse_args, pdf_registration, plist_value};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_optional_paths() {
        assert_eq!(
            parse_args(&args(&["--bundle", "PdfCraft.app", "--pdf", "fixture.pdf"])).expect("arguments"),
            Options { help: false, bundle: Some("PdfCraft.app".into()), pdf: Some("fixture.pdf".into()) }
        );
    }

    #[test]
    fn rejects_missing_path() {
        assert!(parse_args(&args(&["--pdf"])).is_err());
    }

    #[test]
    fn reads_pdf_bundle_values_without_invoking_macos_tools() {
        let plist = r#"
            <key>CFBundleIdentifier</key><string>ai.storyteller.pdfcraft</string>
            <key>LSMinimumSystemVersion</key><string>11.0</string>
            <key>CFBundleDocumentTypes</key><array><dict>
              <key>CFBundleTypeRole</key><string>Editor</string>
              <key>LSHandlerRank</key><string>Alternate</string>
              <key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
            </dict></array>
        "#;
        assert_eq!(plist_value(plist, "CFBundleIdentifier").as_deref(), Some("ai.storyteller.pdfcraft"));
        assert_eq!(plist_value(plist, "LSMinimumSystemVersion").as_deref(), Some("11.0"));
        assert_eq!(pdf_registration(plist), "com.adobe.pdf (role=Editor, rank=Alternate)");
    }
}
