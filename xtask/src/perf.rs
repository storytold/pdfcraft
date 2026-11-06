//! `cargo xtask perf`: performance budgets for large documents (gap 13, `misc.performance-budgets`).
//!
//! Generates synthetic PDFs of a few sizes in a temporary directory (deterministic; never
//! committed), then runs the release `pdfcraft-cli` once per size and phase in a child process,
//! recording wall time and, on Linux, peak resident memory. The results are written as JSON and
//! compared with the committed `xtask/perf-budgets.json`; a budget exceeded makes the command
//! fail.
//!
//! ```text
//! cargo xtask perf [--sizes small,medium,large] [--runs 3] [--out FILE] [--budgets FILE]
//! ```
//!
//! Each phase is its own process, so its peak memory is its own, and its time is the whole
//! process: `scroll` includes the open, `save_full` includes the open and the edit. Of
//! `--runs` repeats the fastest time and the highest peak are kept, which absorbs a noisy
//! runner without hiding a real regression.
//!
//! Peak memory is the child's `VmHWM`, sampled every 5 ms, so a run that ends within a few
//! milliseconds (the `small` size) reads low; it is only a meaningful bound on the larger sizes,
//! and is read on Linux only.
//!
//! The budgets were set from Linux CI runs of this command: at least eight times the measured
//! time (the slowest `large` phase took 1.3 s) and between 2.5 and 4 times the measured peak
//! (the `large` saves peaked at 340 MiB). They catch an order-of-magnitude regression, such as a
//! second or third copy of the file, not a few percent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::gates::{release_cli, root, target_dir};

/// Rendering calls in the `scroll` phase: the first pages, at screen resolution.
const SCROLL_PAGES: usize = 10;
const SCROLL_DPI: u32 = 72;
/// A phase that runs longer than this is killed and fails the run (a hang is a failed budget).
const PHASE_TIMEOUT: Duration = Duration::from_secs(600);

const PHASES: [&str; 5] = ["open", "scroll", "edit", "save_incremental", "save_full"];

/// One synthetic input: page count and bytes of incompressible filler that no page draws.
struct Size {
    name: &'static str,
    pages: usize,
    filler: usize,
}

const SIZES: [Size; 3] = [
    Size { name: "small", pages: 10, filler: 0 },
    Size { name: "medium", pages: 1_000, filler: 0 },
    Size { name: "large", pages: 10_000, filler: 64 << 20 },
];

#[derive(Debug, Clone, Serialize)]
struct Measurement {
    size: String,
    phase: String,
    pages: usize,
    file_bytes: usize,
    secs: f64,
    /// Peak resident memory of the child, in KiB; `None` where the platform gives no way to read it.
    peak_kib: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    max_secs: f64,
    max_peak_mib: f64,
}

/// `size -> phase -> budget`.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct Budgets(BTreeMap<String, BTreeMap<String, Budget>>);

/// Build a deterministic PDF: `pages` pages of text sharing one font, plus `filler` bytes of
/// pseudo-random data in a form XObject that page 1 lists but never draws.
fn generate(pages: usize, filler: usize) -> Vec<u8> {
    // Objects: 1 catalog, 2 page tree, 3 font, 4 filler, then a page and its content per page.
    let mut objects: Vec<Vec<u8>> = Vec::with_capacity(4 + pages * 2);
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 5 + i * 2)).collect();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} /MediaBox [0 0 612 792] >>").into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    let mut bytes = Vec::with_capacity(filler);
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    while bytes.len() < filler {
        // xorshift64: deterministic and incompressible enough that the file really is this big.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(filler);
    let mut form = format!("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length {} >>\nstream\n", bytes.len()).into_bytes();
    form.extend_from_slice(&bytes);
    form.extend_from_slice(b"\nendstream");
    objects.push(form);
    for i in 0..pages {
        let xobject = if i == 0 && filler > 0 { " /XObject << /Filler 4 0 R >>" } else { "" };
        objects.push(
            format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >>{xobject} >> >>", 6 + i * 2).into_bytes(),
        );
        let mut text = String::from("BT /F1 12 Tf 14 TL 50 740 Td\n");
        for line in 0..45 {
            text.push_str(&format!("(Page {} line {line}: the quick brown fox jumps over the lazy dog) Tj T*\n", i + 1));
        }
        text.push_str("ET\n");
        objects.push(format!("<< /Length {} >>\nstream\n{text}endstream", text.len()).into_bytes());
    }

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

/// The automation script for a phase; paths are relative to the `--root` the child runs with.
/// `input` is the generated file; incremental saves work on `inc.pdf`, a copy made beforehand.
fn script(phase: &str, pages: usize) -> Result<serde_json::Value> {
    use serde_json::json;
    let open = |path: &str| json!({ "tool": "doc_open", "args": { "path": path } });
    let edit = || {
        [
            json!({ "tool": "doc_set_info", "args": { "doc": 1, "key": "Title", "value": "perf" } }),
            json!({ "tool": "page_rotate", "args": { "doc": 1, "pages": [1], "degrees": 90 } }),
        ]
    };
    let mut steps = vec![open(if phase == "save_incremental" { "inc.pdf" } else { "input.pdf" })];
    match phase {
        "open" => {}
        "scroll" => {
            steps.extend((1..=SCROLL_PAGES.min(pages)).map(|p| json!({ "tool": "page_render", "args": { "doc": 1, "page": p, "dpi": SCROLL_DPI } })))
        }
        "edit" => steps.extend(edit()),
        "save_incremental" => {
            steps.extend(edit());
            steps.push(json!({ "tool": "doc_save", "args": { "doc": 1, "full": false } }));
        }
        "save_full" => {
            steps.extend(edit());
            steps.push(json!({ "tool": "doc_save", "args": { "doc": 1, "path": "saved.pdf", "full": true } }));
        }
        other => bail!("unknown phase {other}"),
    }
    Ok(serde_json::Value::Array(steps))
}

/// Peak resident set of a running process (`VmHWM`), where `/proc` has it.
fn peak_kib(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// The peak of a phase after all its runs. Where `/proc` has it (Linux) a phase that never yielded
/// one exited before the first sample, which would silently skip its memory budget, so that is an
/// error; elsewhere no peak is expected.
fn require_peak(peak: Option<u64>, linux: bool) -> Result<Option<u64>> {
    if linux && peak.is_none() {
        bail!("exited before its peak memory could be read in any run; use a larger input or more --runs");
    }
    Ok(peak)
}

/// Run `pdfcraft-cli run --script` in `dir`; returns wall time and the last peak memory seen.
/// The high-water mark only rises, so the last sample before the exit is within one poll of it.
fn run_phase(exe: &Path, dir: &Path, steps: &serde_json::Value) -> Result<(f64, Option<u64>)> {
    let script_path = dir.join("steps.json");
    std::fs::write(&script_path, serde_json::to_vec(steps)?)?;
    let start = Instant::now();
    let mut child = Command::new(exe)
        .arg("run")
        .arg("--script")
        .arg(&script_path)
        .arg("--root")
        .arg(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let mut peak = None;
    loop {
        peak = peak_kib(child.id()).or(peak);
        if let Some(status) = child.try_wait()? {
            let secs = start.elapsed().as_secs_f64();
            if !status.success() {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
                bail!("pdfcraft-cli exited with {status}: {}", err.trim());
            }
            return Ok((secs, peak));
        }
        if start.elapsed() > PHASE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("pdfcraft-cli ran past {} s", PHASE_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Every way `measured` breaks `budgets`, as one line each. A phase with no budget is a
/// violation (the budgets file must stay complete); a peak that could not be read is skipped.
fn compare(measured: &[Measurement], budgets: &Budgets) -> Vec<String> {
    let mut violations = Vec::new();
    for m in measured {
        let Some(budget) = budgets.0.get(&m.size).and_then(|b| b.get(&m.phase)) else {
            violations.push(format!("{}/{}: no budget in perf-budgets.json", m.size, m.phase));
            continue;
        };
        if m.secs > budget.max_secs {
            violations.push(format!("{}/{}: {:.2} s exceeds the budget of {:.2} s", m.size, m.phase, m.secs, budget.max_secs));
        }
        if let Some(kib) = m.peak_kib {
            let mib = kib as f64 / 1024.0;
            if mib > budget.max_peak_mib {
                violations.push(format!("{}/{}: peak {:.0} MiB exceeds the budget of {:.0} MiB", m.size, m.phase, mib, budget.max_peak_mib));
            }
        }
    }
    violations
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// The temporary directory for one run, removed on drop (also when a phase fails).
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let wanted: Vec<&str> = flag(args, "--sizes").map_or_else(|| SIZES.iter().map(|s| s.name).collect(), |s| s.split(',').collect());
    for w in &wanted {
        if !SIZES.iter().any(|s| s.name == *w) {
            bail!("unknown size `{w}` (choose from small, medium, large)");
        }
    }
    let runs: usize = flag(args, "--runs").map_or(Ok(3), str::parse).context("--runs takes a number")?;
    if runs == 0 {
        bail!("--runs must be at least 1");
    }
    let budgets_path = flag(args, "--budgets").map_or_else(|| root().join("xtask/perf-budgets.json"), PathBuf::from);
    let budgets: Budgets = serde_json::from_str(&std::fs::read_to_string(&budgets_path).with_context(|| budgets_path.display().to_string())?)
        .with_context(|| format!("{}: not a valid budgets file", budgets_path.display()))?;
    let out_path = flag(args, "--out").map_or_else(|| target_dir().join("perf/results.json"), PathBuf::from);

    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["build", "--release", "-q", "-p", "pdfcraft-cli"])
        .status()?;
    if !status.success() {
        bail!("building pdfcraft-cli failed");
    }
    let exe = release_cli();

    let scratch = Scratch(std::env::temp_dir().join(format!("pdfcraft-perf-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0)?;
    let mut measured = Vec::new();
    for size in SIZES.iter().filter(|s| wanted.contains(&s.name)) {
        let input = generate(size.pages, size.filler);
        std::fs::write(scratch.0.join("input.pdf"), &input)?;
        eprintln!("{}: {} pages, {:.1} MiB", size.name, size.pages, input.len() as f64 / (1 << 20) as f64);
        for phase in PHASES {
            let steps = script(phase, size.pages)?;
            let mut best: Option<(f64, Option<u64>)> = None;
            for _ in 0..runs {
                // An incremental save changes its input, so each run starts from a fresh copy.
                std::fs::write(scratch.0.join("inc.pdf"), &input)?;
                let (secs, peak) = run_phase(&exe, &scratch.0, &steps).with_context(|| format!("{}/{phase}", size.name))?;
                best = Some(best.map_or((secs, peak), |(s, p)| (s.min(secs), p.max(peak))));
            }
            let (secs, peak_kib) = best.unwrap_or_default();
            let peak_kib = require_peak(peak_kib, cfg!(target_os = "linux")).with_context(|| format!("{}/{phase}", size.name))?;
            eprintln!("  {phase:<17} {secs:>7.2} s  {}", peak_kib.map_or("peak n/a".into(), |k| format!("{:>6} MiB", k / 1024)));
            measured.push(Measurement { size: size.name.into(), phase: phase.into(), pages: size.pages, file_bytes: input.len(), secs, peak_kib });
        }
    }

    let violations = compare(&measured, &budgets);
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let report = serde_json::json!({ "os": std::env::consts::OS, "runs": runs, "measurements": measured, "violations": violations });
    std::fs::write(&out_path, serde_json::to_vec_pretty(&report)?)?;
    eprintln!("results: {}", out_path.display());
    if !violations.is_empty() {
        bail!("performance budgets exceeded:\n  {}", violations.join("\n  "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(size: &str, phase: &str, secs: f64, peak_kib: Option<u64>) -> Measurement {
        Measurement { size: size.into(), phase: phase.into(), pages: 1, file_bytes: 1, secs, peak_kib }
    }

    fn budgets() -> Budgets {
        serde_json::from_str(r#"{"small": {"open": {"max_secs": 2.0, "max_peak_mib": 100.0}}}"#).unwrap()
    }

    #[test]
    fn a_linux_phase_without_a_peak_is_an_error() {
        assert!(require_peak(None, true).is_err());
        assert_eq!(require_peak(Some(7), true).unwrap(), Some(7));
        assert_eq!(require_peak(None, false).unwrap(), None);
    }

    #[test]
    fn within_budget_passes() {
        assert!(compare(&[m("small", "open", 1.9, Some(100 * 1024))], &budgets()).is_empty());
    }

    #[test]
    fn slow_and_large_runs_are_reported_separately() {
        assert_eq!(compare(&[m("small", "open", 2.1, Some(1))], &budgets()).len(), 1);
        assert_eq!(compare(&[m("small", "open", 1.0, Some(101 * 1024))], &budgets()).len(), 1);
        assert_eq!(compare(&[m("small", "open", 3.0, Some(200 * 1024))], &budgets()).len(), 2);
    }

    #[test]
    fn an_unreadable_peak_skips_only_the_memory_budget() {
        assert!(compare(&[m("small", "open", 1.0, None)], &budgets()).is_empty());
        assert_eq!(compare(&[m("small", "open", 5.0, None)], &budgets()).len(), 1);
    }

    #[test]
    fn a_phase_without_a_budget_fails() {
        assert_eq!(compare(&[m("small", "save_full", 0.1, None), m("huge", "open", 0.1, None)], &budgets()).len(), 2);
    }

    #[test]
    fn budgets_reject_unknown_fields() {
        assert!(serde_json::from_str::<Budgets>(r#"{"small": {"open": {"max_secs": 2.0, "max_peak_mib": 1.0, "max_cpu": 1}}}"#).is_err());
    }

    #[test]
    fn committed_budgets_cover_every_size_and_phase() {
        let text = std::fs::read_to_string(root().join("xtask/perf-budgets.json")).unwrap();
        let committed: Budgets = serde_json::from_str(&text).unwrap();
        for size in &SIZES {
            for phase in PHASES {
                let b = committed.0.get(size.name).and_then(|p| p.get(phase));
                assert!(b.is_some_and(|b| b.max_secs > 0.0 && b.max_peak_mib > 0.0), "{}/{phase}", size.name);
            }
        }
    }

    #[test]
    fn generated_pdf_is_deterministic_and_has_the_pages() {
        let a = generate(7, 4096);
        assert_eq!(a, generate(7, 4096));
        let doc = lopdf::Document::load_mem(&a).unwrap();
        assert_eq!(doc.get_pages().len(), 7);
    }

    #[test]
    fn filler_sets_the_file_size() {
        assert!(generate(1, 1 << 20).len() > 1 << 20);
        assert!(generate(1, 0).len() < 10_000);
    }

    #[test]
    fn every_phase_has_a_script_that_opens_first() {
        for phase in PHASES {
            let steps = script(phase, 3).unwrap();
            assert_eq!(steps[0]["tool"], "doc_open", "{phase}");
        }
        assert_eq!(script("scroll", 3).unwrap().as_array().unwrap().len(), 4);
        assert!(script("nonsense", 3).is_err());
    }
}
