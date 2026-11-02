//! Quality gates: layering, wasm, the CI bundle, corpora and the robustness sweep.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow, bail};

use crate::layers;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent dir").to_path_buf()
}

/// Cargo's target directory: `CARGO_TARGET_DIR` when it is set (parallel agents use their own),
/// else `target/`.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), |d| root().join(d))
}

/// The release `pdfcraft-cli` that `cargo build --release -p pdfcraft-cli` produced.
pub fn release_cli() -> PathBuf {
    target_dir().join("release").join(format!("pdfcraft-cli{}", std::env::consts::EXE_SUFFIX))
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> anyhow::Result<()> {
    eprintln!("$ {what}");
    let status = cmd.status().with_context(|| format!("{what}: failed to spawn"))?;
    if !status.success() {
        bail!("{what}: exited with {status}");
    }
    Ok(())
}

fn metadata() -> anyhow::Result<serde_json::Value> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().context("cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub fn layers(_: &[String]) -> anyhow::Result<()> {
    let crates = layers::from_metadata(&metadata()?).map_err(|e| anyhow!(e))?;
    println!("Dependency layering (plan/architecture.md §3)\n");
    println!("{:<24} {:<14} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| format!("{}{}", layers::short_name(&d.name), if d.kind == layers::DepKind::Dev { " (dev)" } else { "" }))
            .collect();
        println!("{:<24} {:<14} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        return Ok(());
    }
    for v in &violations {
        println!("  - {v}");
    }
    bail!("{} layering violation(s)", violations.len())
}

pub fn wasm(_: &[String]) -> anyhow::Result<()> {
    let crates = layers::from_metadata(&metadata()?).map_err(|e| anyhow!(e))?;
    let set: Vec<String> = crates
        .into_iter()
        .filter(|c| matches!(layers::classify(&c.name), Some(layers::Class::Layer(_) | layers::Class::Standalone(_))))
        .map(|c| c.name)
        .collect();
    // One call over every crate: they share one dependency build, and `--keep-going` reports
    // each failing crate instead of stopping at the first.
    let mut c = cargo();
    c.args(["check", "--target", "wasm32-unknown-unknown", "--keep-going"]);
    for pkg in &set {
        c.args(["-p", pkg]);
    }
    run(c, &format!("cargo check --target wasm32-unknown-unknown --keep-going ({} crates)", set.len()))?;
    println!("wasm32: {} crates ok", set.len());
    Ok(())
}

/// The workspace tests. Unit and integration tests run under cargo-nextest when it is installed
/// (one process per test, so the run is not held up by its slowest binary), else under
/// `cargo test`; doctests run separately because nextest does not run them. Examples are left
/// out of the test build: clippy `--all-targets` already checks that they compile.
pub fn test(_: &[String]) -> anyhow::Result<()> {
    let targets = ["--workspace", "--lib", "--bins", "--tests"];
    let nextest = cargo().args(["nextest", "--version"]).output().is_ok_and(|o| o.status.success());
    if nextest {
        run_args(&[&["nextest", "run", "--no-fail-fast"][..], &targets].concat())?;
    } else {
        eprintln!("test: cargo-nextest is not installed, using cargo test (cargo install cargo-nextest --locked)");
        run_args(&[&["test", "--no-fail-fast"][..], &targets].concat())?;
    }
    run_args(&["test", "--workspace", "--doc"])
}

pub fn ci(_: &[String]) -> anyhow::Result<()> {
    type Step = (&'static str, Box<dyn Fn() -> anyhow::Result<()>>);
    let steps: Vec<Step> = vec![
        ("fmt", Box::new(|| run_args(&["fmt", "--all", "--", "--check"]))),
        ("clippy", Box::new(|| run_args(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]))),
        ("cli-no-default-features", Box::new(|| run_args(&["check", "-p", "pdfcraft-cli", "--no-default-features"]))),
        ("test", Box::new(|| test(&[]))),
        ("i18n-report", Box::new(|| i18n_report(&["--workspace".to_string()]))),
        ("layers", Box::new(|| layers(&[]))),
        ("wasm", Box::new(|| wasm(&[]))),
        ("license-check", Box::new(|| crate::license_check::run(&[]))),
        ("deny", Box::new(|| deny(&[]))),
        ("parity", Box::new(|| crate::parity::run(&[]))),
    ];
    for (i, (name, f)) in steps.iter().enumerate() {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = f() {
            println!("\nCI: {} passed, FAILED at `{name}`: {e:#}", i);
            bail!("ci failed at `{name}`");
        }
    }
    println!("\nCI: all {} steps passed", steps.len());
    Ok(())
}

/// `cargo xtask i18n-report`: the strings each interface language has no translation for, as
/// counts and the first few keys, from the UI crate's `translation_gaps_report` test. It always
/// passes: a missing string shows in English. `--workspace` runs the test from the workspace
/// test build, which `xtask ci` has just made; without it only the UI crate is built.
pub fn i18n_report(args: &[String]) -> anyhow::Result<()> {
    const TEST: &str = "i18n::tests::translation_gaps_report";
    const START: &str = "=== translation gaps ===";
    const END: &str = "=== end of translation gaps ===";
    let scope: &[&str] = if args.iter().any(|a| a == "--workspace") { &["--workspace"] } else { &["-p", "pdfcraft-ui-egui"] };
    let mut c = cargo();
    c.arg("test").args(scope).args(["--lib", "--quiet", "--", TEST, "--exact", "--nocapture"]).stderr(std::process::Stdio::inherit());
    eprintln!("$ cargo test {} --lib --quiet -- {TEST} --exact --nocapture", scope.join(" "));
    let out = match c.output() {
        Ok(out) => out,
        Err(e) => {
            println!("i18n-report: no report, cargo test did not start: {e}");
            return Ok(());
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Vec<&str> = stdout.lines().skip_while(|l| *l != START).skip(1).take_while(|l| *l != END).collect();
    if report.is_empty() {
        println!("i18n-report: no report (cargo test exited with {})", out.status);
    }
    for line in report {
        println!("{line}");
    }
    Ok(())
}

/// Dependency licences, bans, sources and advisories (`deny.toml`). Skipped with a hint when
/// cargo-deny is not installed, except under CI (`CI` set), where it is required.
pub fn deny(_: &[String]) -> anyhow::Result<()> {
    let installed = Command::new("cargo-deny").arg("--version").output().is_ok_and(|o| o.status.success());
    if !installed {
        if std::env::var_os("CI").is_some() {
            bail!("cargo-deny is required in CI: cargo install cargo-deny --locked");
        }
        eprintln!("deny: SKIPPED, cargo-deny is not installed (cargo install cargo-deny --locked)");
        return Ok(());
    }
    run_args(&["deny", "--log-level", "error", "check"])
}

fn run_args(args: &[&str]) -> anyhow::Result<()> {
    let mut c = cargo();
    c.args(args);
    run(c, &format!("cargo {}", args.join(" ")))
}

pub fn corpus(args: &[String]) -> anyhow::Result<()> {
    crate::corpus::run_cmd(args)
}

pub fn check(args: &[String]) -> anyhow::Result<()> {
    let update = args.iter().any(|a| a == "--update-baseline");
    let corpus = crate::corpus::pdfs();
    // The baseline in xtask/baselines/pdfjs.json describes one exact tree. Sweeping a different
    // one silently moves the numbers it is compared against, so verify the pin first.
    match crate::corpus::state()? {
        crate::corpus::State::Absent => bail!("corpus missing: run `cargo xtask corpus` first"),
        crate::corpus::State::Mismatch { detail } => {
            bail!("corpus does not match the pin ({detail}); run `cargo xtask corpus` to repair it")
        }
        crate::corpus::State::Verified { files } => println!("corpus verified: {files} files at {}", crate::corpus::PDFJS_COMMIT),
    }
    run_args(&["build", "--release", "-p", "pdfcraft-cli"])?;
    let report = target_dir().join("check-pdfjs.json");
    let mut c = Command::new(release_cli());
    c.args(["check"]).arg(&corpus).args(["--timeout", "20", "--dpi", "36", "--json"]).arg(&report);
    run(c, "pdfcraft-cli check corpus/pdfjs")?;
    let results: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(&report)?)?;
    let name =
        |v: &serde_json::Value| Path::new(v["file"].as_str().unwrap_or("")).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let status = |v: &serde_json::Value| v["status"].as_str().unwrap_or("?").split(':').next().unwrap_or("?").to_string();
    let current: BTreeMap<String, String> = results.iter().filter(|v| status(v) != "ok").map(|v| (name(v), status(v))).collect();
    let crashes: Vec<&String> = current.iter().filter(|(_, s)| s.as_str() == "crash").map(|(f, _)| f).collect();
    let baseline_path = root().join("xtask/baselines/pdfjs.json");
    if update {
        std::fs::create_dir_all(baseline_path.parent().expect("has parent"))?;
        std::fs::write(&baseline_path, serde_json::to_string_pretty(&current)? + "\n")?;
        println!("baseline updated: {} known non-ok files", current.len());
    }
    let baseline: BTreeMap<String, String> =
        std::fs::read_to_string(&baseline_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let regressions: Vec<String> = current.iter().filter(|(f, _)| !baseline.contains_key(*f)).map(|(f, s)| format!("{f} ({s})")).collect();
    let fixed: Vec<&String> = baseline.keys().filter(|f| !current.contains_key(*f)).collect();
    println!(
        "\n{} files: {} ok, {} known issues, {} regressions, {} newly fixed, {} crashes",
        results.len(),
        results.len() - current.len(),
        current.len() - regressions.len(),
        regressions.len(),
        fixed.len(),
        crashes.len()
    );
    if !fixed.is_empty() {
        println!("newly fixed (run with --update-baseline): {fixed:?}");
    }
    if !crashes.is_empty() {
        bail!("crashes: {crashes:?}");
    }
    if !regressions.is_empty() {
        bail!("regressions: {regressions:?}");
    }
    Ok(())
}

/// `cargo xtask text-oracle [--limit N]`: compare `pdfcraft-cli text` with poppler's `pdftotext`
/// (external oracle process, never linked) over the corpus.
///
/// Two scores per file, because one hides the other. **Word F1** ([`f1`]) is a bag of words: it
/// catches a character read wrong, a glyph not read at all, a font we cannot map, and is blind to
/// order. **Reading order** ([`order_score`]) catches what F1 cannot see — a column read into the
/// middle of its neighbour, a figure's labels threaded through a paragraph. Reports both; gates
/// neither. Plan target (M2): median F1 ≥ 0.97.
pub fn text_oracle(args: &[String]) -> anyhow::Result<()> {
    let limit: usize = args.iter().position(|a| a == "--limit").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let pdftotext = match pdfcraft_oracle::pdftotext() {
        Ok(o) => o,
        Err(absent) => bail!("{absent}"),
    };
    run_args(&["build", "--release", "-p", "pdfcraft-cli"])?;
    let corpus = root().join("corpus/pdfjs/test/pdfs");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&corpus)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pdf")).collect();
    files.sort();
    let cli = release_cli();
    let mut scores = Vec::new();
    let mut orders = Vec::new();
    let mut worst: Vec<(f64, String)> = Vec::new();
    let mut worst_order: Vec<(f64, String)> = Vec::new();
    let mut not_permitted = 0usize;
    let mut truncated = 0usize;
    for f in files.iter().take(limit) {
        // A document may forbid copying its text, and we honour that: `pdfcraft-cli text` refuses
        // and says how to override it with the owner password. pdftotext ignores the flag and
        // extracts anyway, so scoring us against it there measures obedience, not quality --
        // bug1815476.pdf scored 0.000 for behaving correctly on a page we render perfectly.
        if !copying_allowed(f) {
            not_permitted = not_permitted.saturating_add(1);
            continue;
        }
        let ours = run_capture(Command::new(&cli).arg("text").arg(f), 20);
        let theirs = run_capture(pdftotext.command().arg("-q").arg(f).arg("-"), 20);
        let (Some(ours), Some(theirs)) = (ours, theirs) else { continue };
        let (sa, sb) = (word_sequence(&ours), word_sequence(&theirs));
        let (a, b) = (words(&sa), words(&sb));
        if b.len() < 20 {
            continue; // image-only or near-empty per the oracle
        }
        let score = f1(&a, &b);
        let (order, cut) = order_score(&sa, &sb);
        truncated = truncated.saturating_add(usize::from(cut));
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        scores.push(score);
        orders.push(order);
        worst.push((score, name.clone()));
        worst_order.push((order, name));
    }
    if scores.is_empty() {
        bail!("no comparable files");
    }
    let summary = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        (v[v.len() / 2], mean, v[v.len() / 10])
    };
    let compared = scores.len();
    let (median, mean, p10) = summary(&mut scores);
    let (omedian, omean, op10) = summary(&mut orders);
    worst.sort_by(|a, b| a.0.total_cmp(&b.0));
    worst_order.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!("text oracle: {compared} files with text; median F1 {median:.3}, mean {mean:.3}, p10 {p10:.3}");
    println!(
        "  reading order: median {omedian:.3}, mean {omean:.3}, p10 {op10:.3}{}",
        if truncated > 0 { format!(" ({truncated} cut to {MAX_ORDER_WORDS} words)") } else { String::new() }
    );
    if not_permitted > 0 {
        println!("  {not_permitted} skipped: the document forbids copying its text, which we honour and pdftotext does not");
    }
    println!("lowest F1:");
    for (s, n) in worst.iter().take(12) {
        println!("  {s:.3}  {n}");
    }
    println!("lowest reading order:");
    for (s, n) in worst_order.iter().take(12) {
        println!("  {s:.3}  {n}");
    }
    Ok(())
}

/// Compare our rendering against Poppler's, page 1 of every PDF in a directory.
///
/// This is the measured half of the fidelity gap (`docs/gaps.md` gap 12): until it exists, every
/// claim about rendering quality is a judgement. It reports rather than gates, because the corpus
/// is not fetched on CI and because a first reading of a borrowed renderer is a baseline to
/// improve on, not a bar to clear.
///
/// The score is the share of pixels with no near match in the other renderer's 3x3 neighbourhood
/// (`pdfcraft_oracle::pixels`), which forgives antialiasing and subpixel placement but not
/// missing or wrong content. **Lower is better; it is a disagreement rate, not a quality score.**
pub fn render_oracle(args: &[String]) -> anyhow::Result<()> {
    let limit: usize = flag_num(args, "--limit").unwrap_or(usize::MAX);
    let dpi: u32 = flag_num(args, "--dpi").unwrap_or(72);
    let tolerance: u8 = flag_num(args, "--tolerance").unwrap_or(8);
    // Annotations are shown by default, which is not the obvious choice and was settled by
    // measurement rather than argument. Poppler synthesises an appearance for an annotation that
    // has none, where we draw only what the document carries (`view.render-annotation-appearances`
    // records link borders without `/AP` as not covered), so showing them scores us for a known
    // gap: bug1552113.pdf is 84% purely because Poppler paints a 112-unit link border over the
    // text that fixture exists to show -- and our rendering, not Poppler's, is the readable one.
    //
    // Hiding them is worse, though, because it scores Poppler for every appearance we *do* draw.
    // Over the same 250 files: shown gives mean 2.16%, p90 5.35%, 25 files within 0.1%; hidden
    // gives 2.18%, 5.55% and 22, with annotation-tx3.pdf jumping to 45.64%. The two biases very
    // nearly cancel, so the default stays with the fuller picture and this flag exists to tell
    // the two apart when a single file's score needs explaining.
    let hide_annotations = args.iter().any(|a| a == "--hide-annotations");
    let dir =
        args.iter().position(|a| a == "--dir").and_then(|i| args.get(i + 1)).map_or_else(|| root().join("corpus/pdfjs/test/pdfs"), PathBuf::from);

    let pdftoppm = match pdfcraft_oracle::pdftoppm() {
        Ok(tool) => tool,
        Err(absent) => bail!("{absent}"),
    };
    run_args(&["build", "--release", "-p", "pdfcraft-cli"])?;
    let cli = release_cli();

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("{}: no such directory (cargo xtask corpus, or pass --dir)", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    files.sort();

    let out_pam = std::env::temp_dir().join(format!("pdfcraft-render-oracle-{}.pam", std::process::id()));
    let (mut scores, mut sizes_differed, mut ours_failed, mut theirs_failed) = (Vec::new(), 0usize, 0usize, 0usize);
    let mut worst: Vec<(f64, String)> = Vec::new();
    for file in files.iter().take(limit) {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ours = Command::new(&cli)
            .args(["render", "--page", "1", "--dpi"])
            .arg(dpi.to_string())
            .arg("--out")
            .arg(&out_pam)
            .arg(file)
            .status()
            .ok()
            .filter(std::process::ExitStatus::success)
            .and_then(|_| std::fs::read(&out_pam).ok())
            .and_then(|b| pdfcraft_oracle::pixels::decode_pam_gray(&b));
        let mut theirs_cmd = pdftoppm.command();
        theirs_cmd.args(["-singlefile", "-gray", "-f", "1", "-l", "1", "-r"]).arg(dpi.to_string());
        if hide_annotations {
            theirs_cmd.arg("-hide-annotations");
        }
        theirs_cmd.arg(file);
        let theirs = run_capture_bytes(&mut theirs_cmd, 30).and_then(|b| pdfcraft_oracle::pixels::decode_pgm(&b));
        // An oracle that cannot read a file says nothing about our fidelity, and a file we cannot
        // read is `xtask check`'s business, not this one's. Count each side and move on.
        if ours.is_none() {
            ours_failed = ours_failed.saturating_add(1);
        }
        if theirs.is_none() {
            theirs_failed = theirs_failed.saturating_add(1);
        }
        let (Some((aw, ah, a)), Some((bw, bh, b))) = (ours, theirs) else { continue };
        let (Some(a), Some(b)) = (pixels_of(aw, ah, &a), pixels_of(bw, bh, &b)) else { continue };
        match pdfcraft_oracle::pixels::compare(a, b, tolerance) {
            Ok(diff) => {
                scores.push(diff.percent());
                worst.push((diff.percent(), name));
            }
            // Different rounding of the page box at this scale: not a fidelity signal.
            Err(_) => sizes_differed = sizes_differed.saturating_add(1),
        }
    }
    let _ = std::fs::remove_file(&out_pam);

    if scores.is_empty() {
        bail!(
            "no comparable files in {} ({sizes_differed} size mismatches, {ours_failed} we could not render, {theirs_failed} Poppler could not)",
            dir.display()
        );
    }
    scores.sort_by(f64::total_cmp);
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    let median = scores.get(scores.len() / 2).copied().unwrap_or(0.0);
    let mean = scores.iter().sum::<f64>() / scores.len() as f64;
    let p90 = scores.get(scores.len().saturating_mul(9) / 10).copied().unwrap_or(0.0);
    let clean = scores.iter().filter(|s| **s < 0.1).count();
    println!("render oracle vs Poppler at {dpi} dpi, tolerance {tolerance} (disagreement rate, lower is better)");
    println!("  {} compared: median {median:.2}%, mean {mean:.2}%, p90 {p90:.2}%", scores.len());
    println!("  {clean} within 0.1%; {sizes_differed} size mismatches, {ours_failed} we could not render, {theirs_failed} Poppler could not");
    println!("worst:");
    for (score, name) in worst.iter().take(12) {
        println!("  {score:6.2}%  {name}");
    }
    Ok(())
}

/// Whether the document lets its text be copied.
///
/// `true` when it is unencrypted, when the flag is set, and when the file cannot be opened at all
/// — a file we cannot read is a different problem, and silently dropping it from the comparison
/// would hide it.
fn copying_allowed(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return true };
    let Ok(doc) = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)) else { return true };
    doc.permissions().is_none_or(|p| p.copy())
}

/// The value after `name` on the command line, parsed, or `None` when it is absent or malformed.
fn flag_num<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i.checked_add(1)?)).and_then(|v| v.parse().ok())
}

fn pixels_of(w: usize, h: usize, buf: &[u8]) -> Option<pdfcraft_oracle::pixels::Gray<'_>> {
    pdfcraft_oracle::pixels::Gray::new(w, h, buf)
}

fn run_capture(cmd: &mut Command, timeout_s: u64) -> Option<String> {
    // `read_to_string` left the buffer empty when the output was not UTF-8; `from_utf8` keeps
    // exactly that behaviour now that the capture itself is byte-oriented.
    Some(String::from_utf8(run_capture_bytes(cmd, timeout_s)?).unwrap_or_default())
}

/// [`run_capture`] for output that is not text — an oracle's image on stdout, for instance.
fn run_capture_bytes(cmd: &mut Command, timeout_s: u64) -> Option<Vec<u8>> {
    let mut child = cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    let mut out = Vec::new();
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = Vec::new();
        let _ = stdout.read_to_end(&mut s);
        s
    });
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed().as_secs() > timeout_s => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(_) => return None,
        }
    }
    out.extend_from_slice(&reader.join().ok()?);
    Some(out)
}

/// The words of `s`, in the order they were extracted.
fn word_sequence(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

fn words(seq: &[String]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for w in seq {
        *m.entry(w.clone()).or_insert(0) += 1;
    }
    m
}

/// Both word sequences are cut to this length before [`order_score`] compares them: the
/// comparison is quadratic, and three pages of prose is plenty of signal about reading order.
const MAX_ORDER_WORDS: usize = 3000;

/// How much of the longer word sequence reads in the same order in both, and whether either side
/// was cut to [`MAX_ORDER_WORDS`] first.
///
/// [`f1`] is a bag of words and cannot see order at all: a page emitted back to front scores a
/// perfect 1.000 against the page read properly. Reading order is most of what extraction has
/// left to get
/// wrong — a column read into the middle of its neighbour, a figure's labels threaded through a
/// paragraph, a form's value parted from its label — so it needs a number of its own. Reviewing
/// #2431, a change that moves text and alters nothing else, F1 called 26 of the 29 files whose
/// output changed identical, while this separated 18 improvements from 6 regressions.
///
/// The score is the length of the longest common subsequence over the longer side. Like every
/// other oracle number here it is a *disagreement* with Poppler, not a verdict: the same split
/// scores as a win on one file and a loss on another purely because `pdftotext` happens to make
/// it in one place and not the other.
fn order_score(a: &[String], b: &[String]) -> (f64, bool) {
    let cut = a.len() > MAX_ORDER_WORDS || b.len() > MAX_ORDER_WORDS;
    let a = a.get(..a.len().min(MAX_ORDER_WORDS)).unwrap_or(a);
    let b = b.get(..b.len().min(MAX_ORDER_WORDS)).unwrap_or(b);
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return (0.0, cut);
    }
    // Two rows of the usual table: after row `i`, `prev[j]` answers `a[..i]` against `b[..j]`.
    let (mut prev, mut cur) = (vec![0u32; m + 1], vec![0u32; m + 1]);
    for x in a {
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = if x == y { prev[j].saturating_add(1) } else { cur[j].max(prev[j + 1]) };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    (f64::from(prev[m]) / n.max(m) as f64, cut)
}

fn f1(a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>) -> f64 {
    let common: usize = a.iter().map(|(w, n)| (*n).min(*b.get(w).unwrap_or(&0))).sum();
    let (na, nb): (usize, usize) = (a.values().sum(), b.values().sum());
    if na == 0 || nb == 0 {
        return 0.0;
    }
    let (p, r) = (common as f64 / na as f64, common as f64 / nb as f64);
    if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
}

#[cfg(test)]
mod tests {
    use super::{f1, order_score, root, word_sequence, words};

    /// The two scores answer different questions, and the point of adding the second one is the
    /// first row: F1 cannot tell a page from the same page read backwards.
    #[test]
    fn reading_order_sees_what_word_f1_cannot() {
        let score = |ours: &str, theirs: &str| {
            let (a, b) = (word_sequence(ours), word_sequence(theirs));
            (f1(&words(&a), &words(&b)), order_score(&a, &b).0)
        };

        // Two columns, read as columns and read interleaved: every word is there either way.
        let columns = "left one left two left three right one right two right three";
        let woven = "left one right one left two right two left three right three";
        let (same_words, out_of_order) = score(woven, columns);
        assert_eq!(same_words, 1.0, "F1 is a bag of words and cannot see the difference");
        assert!(out_of_order < 0.7, "reading order can: {out_of_order}");

        // It is not blind to the thing F1 measures, either: a missing word costs both.
        let (f, o) = score("left one left two left", columns);
        assert!(f < 0.9 && o < 0.9, "{f} {o}");

        // Identical text, and the degenerate inputs.
        assert_eq!(score(columns, columns), (1.0, 1.0));
        assert_eq!(score("", columns).1, 0.0);
        assert_eq!(score(columns, "").1, 0.0);
        assert_eq!(order_score(&[], &[]), (0.0, false));

        // Case and punctuation are not order: both sides are folded the same way.
        assert_eq!(score("Left, one. LEFT two!", "left one left two"), (1.0, 1.0));

        // Over the cap, the comparison still answers, and says that it was cut.
        let long: Vec<String> = (0..super::MAX_ORDER_WORDS + 10).map(|i| i.to_string()).collect();
        let (s, cut) = order_score(&long, &long);
        assert!((s - 1.0).abs() < 1e-9 && cut, "{s} {cut}");
    }

    // Issue #211: Finder lists the app under Open With > Recommended only when the
    // bundle claims PDFs by UTI (#176).
    #[test]
    fn macos_bundle_claims_pdf_by_uti() {
        let pdf = pdf_document_type();
        assert!(pdf.contains("<string>com.adobe.pdf</string>"), "claim PDFs by UTI, not just extension/MIME");
        assert!(pdf.contains("<string>Editor</string>"), "PDF role stays Editor");
        assert!(pdf.contains("<string>Alternate</string>"), "rank stays Alternate: offered without taking over Preview");
        assert!(pdf.contains("<key>CFBundleTypeIconFile</key>"), "declares document icon file for Finder");
    }

    // The bundle id must match the app id the binary uses; renames change both (#174).
    #[test]
    fn macos_bundle_id_matches_app_id() {
        assert!(read("packaging/macos/Info.plist.in").contains("<string>ai.storyteller.pdfcraft</string>"), "bundle id");
        let main = read("apps/pdfcraft/src/main.rs");
        assert!(main.contains(r#"const APP_ID: &str = "ai.storyteller.pdfcraft""#), "APP_ID drifted from the bundle id");
    }

    // Committed file with `<!-- -->` comments stripped, so a commented-out claim cannot pass.
    fn read(rel: &str) -> String {
        let text = std::fs::read_to_string(root().join(rel)).expect("committed file");
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some(i) = rest.find("<!--") {
            out.push_str(&rest[..i]);
            rest = rest[i..].find("-->").map_or("", |j| &rest[i + j + 3..]);
        }
        out.push_str(rest);
        out
    }

    // The `<dict>` inside `CFBundleDocumentTypes` that mentions the PDF UTI: scoped so a
    // UTI string anywhere else cannot satisfy the asserts above.
    fn pdf_document_type() -> String {
        let plist = read("packaging/macos/Info.plist.in");
        let docs = plist.split("<key>CFBundleDocumentTypes</key>").nth(1).expect("CFBundleDocumentTypes");
        docs.split("<dict>")
            .map(|chunk| chunk.split("</dict>").next().unwrap_or(""))
            .find(|chunk| chunk.contains("com.adobe.pdf"))
            .expect("a document type for com.adobe.pdf")
            .to_string()
    }
}
