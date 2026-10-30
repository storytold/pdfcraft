//! `cargo xtask interop`: open every kind of output PdfCraft writes in other readers.
//!
//! `docs/gaps.md` gap 12 asks for an interop suite: output that only our own reader opens is not
//! known to be interoperable, and the encrypted kinds are where it has gone wrong before (gap 1).
//! This writes one small deterministic file per kind of output, by running `pdfcraft-cli` on a
//! script, then opens each in MuPDF (`mutool`) and Poppler (`pdfinfo`, `pdftoppm`), every reader a
//! separate process, and checks that it opens, reports the page count we wrote and renders every
//! page without error. An encrypted file must also open with the owner password and refuse both a
//! wrong password and none.
//!
//! A reader that is not installed is a declared skip, never a pass, and a failure when
//! `PDFCRAFT_ORACLES_REQUIRED` is set (`pdfcraft_oracle`). The result is a table on stdout and
//! `interop.json` in the output directory (`target/interop`, or `--out DIR`).
//!
//! This checks that other readers *accept* the files. It does not compare what they draw (that is
//! `render-oracle`) and it says nothing about Acrobat.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use pdfcraft_oracle::Oracle;
use serde_json::{Value, json};

use crate::gates;

const TIMEOUT: Duration = Duration::from_secs(120);
const USER_PW: &str = "interop-user";
const OWNER_PW: &str = "interop-owner";
const SIGN_PW: &str = "interop-signing-pw";
const FIXTURE_TEXT: &str = "PdfCraft interop fixture\ninterop line two";
/// Low enough to keep the renders small; this checks that pages render, not how they look.
const RENDER_DPI: &str = "36";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

impl Outcome {
    fn word(&self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail(_) => "fail",
            Outcome::Skip(_) => "skip",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Outcome::Pass => "",
            Outcome::Fail(d) | Outcome::Skip(d) => d,
        }
    }
}

/// One kind of output: the script that writes it and what a reader must find in it.
struct Case {
    name: &'static str,
    what: String,
    steps: Vec<Value>,
    /// Pages the written file has.
    pages: u32,
    encrypted: bool,
    /// The save must have appended to the file rather than rewriting it.
    incremental: bool,
}

impl Case {
    fn file(&self, out: &Path) -> PathBuf {
        out.join(format!("{}.pdf", self.name))
    }
}

struct Check {
    reader: &'static str,
    name: &'static str,
    outcome: Outcome,
}

struct CaseResult {
    name: &'static str,
    what: String,
    pages: u32,
    generated: Outcome,
    checks: Vec<Check>,
}

/// A reader: a pair of programs, one that reports the page count and one that renders.
struct Reader {
    name: &'static str,
    poppler: bool,
    info: Oracle,
    render: Oracle,
}

/// A reader that is not installed, and why.
struct Skipped {
    name: &'static str,
    why: String,
}

type Probe = Result<Reader, Skipped>;

enum Auth<'a> {
    None,
    User(&'a str),
    Owner(&'a str),
}

fn step(tool: &str, args: Value) -> Value {
    json!({ "tool": tool, "args": args })
}

/// Every kind of output. Each script runs in its own process, so its document is always id 1
/// (id 2 where a case reopens its own file).
fn cases(out: &Path) -> Vec<Case> {
    let file = |name: &str| out.join(format!("{name}.pdf")).to_string_lossy().into_owned();
    let create = || step("doc_create", json!({ "from": "text", "text": FIXTURE_TEXT }));
    let save = |name: &str| step("doc_save", json!({ "doc": 1, "path": file(name) }));
    let case = |name: &'static str, what: &str, steps: Vec<Value>| Case {
        name,
        what: what.to_owned(),
        steps,
        pages: 1,
        encrypted: false,
        incremental: false,
    };
    let encrypted = |name: &'static str, compatibility: &str| Case {
        encrypted: true,
        ..case(
            name,
            &format!("{compatibility} with an open and a permissions password"),
            vec![
                create(),
                step("doc_protect", json!({ "doc": 1, "open_password": USER_PW, "permissions_password": OWNER_PW, "compatibility": compatibility })),
                save(name),
            ],
        )
    };
    let id = out.join("interop-id.p12").to_string_lossy().into_owned();
    vec![
        case("plain-save", "a new document saved as a full rewrite", vec![create(), save("plain-save")]),
        Case {
            incremental: true,
            ..case(
                "incremental-save",
                "a saved document changed and saved in place (an update appended to the file)",
                vec![
                    create(),
                    save("incremental-save"),
                    step("doc_open", json!({ "path": file("incremental-save") })),
                    step("doc_set_info", json!({ "doc": 2, "key": "Title", "value": "Incremental" })),
                    step("doc_save", json!({ "doc": 2 })),
                ],
            )
        },
        encrypted("encrypted-aes-256", "aes-256"),
        encrypted("encrypted-aes-128", "aes-128"),
        encrypted("encrypted-rc4-128", "rc4-128"),
        encrypted("encrypted-rc4-40", "rc4-40"),
        case(
            "signed",
            "a visible digital signature (self-signed P-256 ID)",
            vec![
                step("sign_id_create", json!({ "name": "Interop Test", "password": SIGN_PW, "path": id, "key": "p256" })),
                create(),
                step(
                    "sign_document",
                    json!({ "doc": 1, "id": id, "password": SIGN_PW, "page": 1, "rect": [72, 500, 272, 560], "reason": "interop", "out": file("signed") }),
                ),
            ],
        ),
        case(
            "form-filled",
            "a text field, a check box and a combo box, filled in",
            vec![
                create(),
                step("form_add_field", json!({ "doc": 1, "page": 1, "type": "text", "name": "full_name", "rect": [72, 200, 300, 224] })),
                step("form_add_field", json!({ "doc": 1, "page": 1, "type": "checkbox", "name": "agree", "rect": [72, 240, 90, 258] })),
                step(
                    "form_add_field",
                    json!({ "doc": 1, "page": 1, "type": "combo", "name": "colour", "rect": [72, 280, 200, 304], "options": ["Red", "Green", "Blue"] }),
                ),
                step("form_fill", json!({ "doc": 1, "values": { "full_name": "Ada Lovelace", "agree": true, "colour": "Green" } })),
                save("form-filled"),
            ],
        ),
        case(
            "annotated",
            "a highlight, a note, a rectangle and a text box",
            vec![
                create(),
                step("comment_add", json!({ "doc": 1, "page": 1, "type": "highlight", "find": "interop" })),
                step("comment_add", json!({ "doc": 1, "page": 1, "type": "note", "at": [300, 100], "contents": "Interop note" })),
                step("comment_add", json!({ "doc": 1, "page": 1, "type": "rectangle", "rect": [72, 300, 200, 340] })),
                step("comment_add", json!({ "doc": 1, "page": 1, "type": "textbox", "rect": [72, 360, 250, 400], "contents": "Interop text box" })),
                save("annotated"),
            ],
        ),
        Case {
            pages: 3,
            ..case(
                "page-edited",
                "three pages with one deleted, a blank one inserted first and all rotated",
                vec![
                    step("doc_create", json!({ "from": "blank", "pages": 3, "width": 200, "height": 300 })),
                    step("page_delete", json!({ "doc": 1, "pages": [2] })),
                    step("page_insert_blank", json!({ "doc": 1, "at": 1 })),
                    step("page_rotate", json!({ "doc": 1, "degrees": 90 })),
                    save("page-edited"),
                ],
            )
        },
    ]
}

// ---- running a process ------------------------------------------------------------------------

struct Captured {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn drain<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = r.read_to_end(&mut bytes);
        bytes
    })
}

/// Run `cmd` to completion, killing it after [`TIMEOUT`]. Both streams are read while it runs, so a
/// chatty reader cannot fill a pipe and hang.
fn capture(mut cmd: Command) -> Result<Captured, String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let (stdout, stderr) = (child.stdout.take().map(drain), child.stderr.take().map(drain));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let text = |h: Option<std::thread::JoinHandle<Vec<u8>>>| String::from_utf8_lossy(&h.and_then(|h| h.join().ok()).unwrap_or_default()).into_owned();
    Ok(Captured { ok: status.success(), stdout: text(stdout), stderr: text(stderr) })
}

// ---- the readers ------------------------------------------------------------------------------

/// A reader, or a skip notice. A reader that is missing where [`pdfcraft_oracle::REQUIRED_VAR`] is
/// set is an error instead.
fn probe(
    name: &'static str,
    poppler: bool,
    info: Result<Oracle, pdfcraft_oracle::Absent>,
    render: Result<Oracle, pdfcraft_oracle::Absent>,
) -> anyhow::Result<Probe> {
    match (info, render) {
        (Ok(info), Ok(render)) => Ok(Ok(Reader { name, poppler, info, render })),
        (Err(absent), _) | (_, Err(absent)) if absent.required() => bail!("{absent}"),
        (Err(absent), _) | (_, Err(absent)) => Ok(Err(Skipped { name, why: absent.to_string() })),
    }
}

impl Reader {
    fn info_command(&self, file: &Path, auth: &Auth) -> Command {
        let mut cmd = self.info.command();
        if !self.poppler {
            cmd.arg("info");
        }
        self.auth_args(&mut cmd, auth);
        cmd.arg(file);
        cmd
    }

    fn render_command(&self, file: &Path, auth: &Auth, dir: &Path, pages: u32) -> Command {
        let mut cmd = self.render.command();
        if self.poppler {
            cmd.args(["-png", "-r", RENDER_DPI]);
            self.auth_args(&mut cmd, auth);
            cmd.arg(file).arg(dir.join("page"));
        } else {
            cmd.args(["draw", "-r", RENDER_DPI]);
            self.auth_args(&mut cmd, auth);
            cmd.arg("-o").arg(dir.join("page-%d.png")).arg(file).arg(format!("1-{pages}"));
        }
        cmd
    }

    /// MuPDF takes either password as `-p`; Poppler tells them apart.
    fn auth_args(&self, cmd: &mut Command, auth: &Auth) {
        match (auth, self.poppler) {
            (Auth::None, _) => {}
            (Auth::User(pw) | Auth::Owner(pw), false) => {
                cmd.args(["-p", *pw]);
            }
            (Auth::User(pw), true) => {
                cmd.args(["-upw", *pw]);
            }
            (Auth::Owner(pw), true) => {
                cmd.args(["-opw", *pw]);
            }
        }
    }

    /// The page count the reader reports, or why it could not.
    fn page_count(&self, file: &Path, auth: &Auth) -> Result<u32, String> {
        let got = capture(self.info_command(file, auth))?;
        if !got.ok {
            return Err(format!("exited with failure: {}", first_line(&got.stderr)));
        }
        if let Some(problem) = stderr_problem(&got.stderr) {
            return Err(problem);
        }
        parse_pages(&got.stdout).ok_or_else(|| "printed no page count".to_owned())
    }

    fn opens(&self, file: &Path, auth: &Auth, pages: u32) -> Outcome {
        match self.page_count(file, auth) {
            Ok(n) if n == pages => Outcome::Pass,
            Ok(n) => Outcome::Fail(format!("reports {n} pages, we wrote {pages}")),
            Err(e) => Outcome::Fail(e),
        }
    }

    fn renders(&self, file: &Path, auth: &Auth, dir: &Path, pages: u32) -> Outcome {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Outcome::Fail(format!("{}: {e}", dir.display()));
        }
        let got = match capture(self.render_command(file, auth, dir, pages)) {
            Ok(got) => got,
            Err(e) => return Outcome::Fail(e),
        };
        if !got.ok {
            return Outcome::Fail(format!("exited with failure: {}", first_line(&got.stderr)));
        }
        if let Some(problem) = stderr_problem(&got.stderr) {
            return Outcome::Fail(problem);
        }
        let drawn = count_pngs(dir);
        if drawn == pages as usize { Outcome::Pass } else { Outcome::Fail(format!("rendered {drawn} of {pages} pages")) }
    }

    /// Whether the reader turns the file away, and does so because of the password.
    fn refuses(&self, file: &Path, auth: &Auth) -> Outcome {
        match capture(self.info_command(file, auth)) {
            Err(e) => Outcome::Fail(e),
            Ok(got) if got.ok => Outcome::Fail("opened the file".to_owned()),
            Ok(got) if mentions_password(&got.stderr) => Outcome::Pass,
            Ok(got) => Outcome::Fail(format!("failed, but not over the password: {}", first_line(&got.stderr))),
        }
    }
}

// ---- judging output ---------------------------------------------------------------------------

fn first_line(text: &str) -> &str {
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or("no message").trim()
}

/// The page count in `mutool info` ("Pages: 3") or `pdfinfo` ("Pages:          3") output.
fn parse_pages(text: &str) -> Option<u32> {
    text.lines().find_map(|l| l.trim().strip_prefix("Pages:")).and_then(|n| n.trim().parse().ok())
}

/// The first line a reader printed on stderr that reports an error. Warnings are not failures:
/// both readers print them for files that are legal but untidy.
fn stderr_problem(stderr: &str) -> Option<String> {
    stderr.lines().find(|l| l.to_ascii_lowercase().contains("error")).map(|l| l.trim().to_owned())
}

/// Whether a refusal names the password, so a reader that fails for another reason does not pass
/// the check that it refuses a wrong password.
fn mentions_password(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("password") || lower.contains("authenticat")
}

/// The `%%EOF` markers in a file: one per revision, so more than one means a save appended.
fn count_eof(bytes: &[u8]) -> usize {
    let marker = b"%%EOF";
    bytes.windows(marker.len()).filter(|w| *w == marker).count()
}

fn count_pngs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "png") && e.metadata().is_ok_and(|m| m.len() > 0)).count()
}

// ---- the suite --------------------------------------------------------------------------------

/// Write the case's file with `pdfcraft-cli`.
fn generate(cli: &Path, out: &Path, case: &Case) -> Outcome {
    let script = out.join(format!("{}.steps.json", case.name));
    if let Err(e) = std::fs::write(&script, Value::Array(case.steps.clone()).to_string()) {
        return Outcome::Fail(format!("{}: {e}", script.display()));
    }
    let mut cmd = Command::new(cli);
    cmd.arg("run").arg("--script").arg(&script);
    match capture(cmd) {
        Err(e) => return Outcome::Fail(e),
        Ok(got) if !got.ok => return Outcome::Fail(first_line(&got.stderr).to_owned()),
        Ok(_) => {}
    }
    let bytes = match std::fs::read(case.file(out)) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => return Outcome::Fail("wrote an empty file".to_owned()),
        Err(e) => return Outcome::Fail(format!("wrote no file: {e}")),
    };
    if case.incremental && count_eof(&bytes) < 2 {
        return Outcome::Fail("the second save rewrote the file instead of appending to it".to_owned());
    }
    Outcome::Pass
}

fn check_reader(reader: &Probe, case: &Case, out: &Path, checks: &mut Vec<Check>) {
    let file = case.file(out);
    let scratch = out.join(case.name);
    let mut add = |reader: &'static str, name: &'static str, outcome: Outcome| checks.push(Check { reader, name, outcome });
    let reader = match reader {
        Ok(reader) => reader,
        Err(skipped) => {
            // The checks the reader would have made, so the skip shows in the table.
            let password_checks = ["owner password opens", "wrong password refused", "no password refused"];
            for name in ["open", "render"].into_iter().chain(password_checks.into_iter().filter(|_| case.encrypted)) {
                add(skipped.name, name, Outcome::Skip(skipped.why.clone()));
            }
            return;
        }
    };
    let user = if case.encrypted { Auth::User(USER_PW) } else { Auth::None };
    add(reader.name, "open", reader.opens(&file, &user, case.pages));
    add(reader.name, "render", reader.renders(&file, &user, &scratch.join(reader.name), case.pages));
    if case.encrypted {
        add(reader.name, "owner password opens", reader.opens(&file, &Auth::Owner(OWNER_PW), case.pages));
        add(reader.name, "wrong password refused", reader.refuses(&file, &Auth::User("not-the-password")));
        add(reader.name, "no password refused", reader.refuses(&file, &Auth::None));
    }
}

fn run_suite(cli: &Path, out: &Path, readers: &[Probe]) -> Vec<CaseResult> {
    cases(out)
        .into_iter()
        .map(|case| {
            let generated = generate(cli, out, &case);
            let mut checks = Vec::new();
            if generated == Outcome::Pass {
                for reader in readers {
                    check_reader(reader, &case, out, &mut checks);
                }
            }
            CaseResult { name: case.name, what: case.what, pages: case.pages, generated, checks }
        })
        .collect()
}

// ---- reporting --------------------------------------------------------------------------------

/// One table cell: what a reader made of a case.
fn cell(result: &CaseResult, reader: &str) -> &'static str {
    let mine: Vec<&Check> = result.checks.iter().filter(|c| c.reader == reader).collect();
    if mine.is_empty() {
        "-"
    } else if mine.iter().any(|c| matches!(c.outcome, Outcome::Fail(_))) {
        "FAIL"
    } else if mine.iter().all(|c| matches!(c.outcome, Outcome::Skip(_))) {
        "skip"
    } else {
        "ok"
    }
}

fn failures(results: &[CaseResult]) -> Vec<String> {
    let mut lines = Vec::new();
    for r in results {
        if let Outcome::Fail(why) = &r.generated {
            lines.push(format!("{}: pdfcraft could not write it: {why}", r.name));
        }
        for c in &r.checks {
            if let Outcome::Fail(why) = &c.outcome {
                lines.push(format!("{}: {} {}: {why}", r.name, c.reader, c.name));
            }
        }
    }
    lines
}

fn table(results: &[CaseResult]) -> String {
    let width = results.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let mut text = format!("{:<width$}  {:<8}  {:<6}  {:<7}\n", "case", "pdfcraft", "mupdf", "poppler");
    for r in results {
        let written = match r.generated {
            Outcome::Pass => "ok",
            _ => "FAIL",
        };
        text.push_str(&format!("{:<width$}  {:<8}  {:<6}  {:<7}\n", r.name, written, cell(r, "mupdf"), cell(r, "poppler")));
    }
    for line in failures(results) {
        text.push_str(&format!("  {line}\n"));
    }
    text
}

fn report(results: &[CaseResult], readers: &[Probe]) -> Value {
    let reader = |p: &Probe| match p {
        Ok(r) => json!({ "reader": r.name, "available": true, "version": r.info.version() }),
        Err(skipped) => json!({ "reader": skipped.name, "available": false, "skipped": skipped.why }),
    };
    let count = |word: &str| results.iter().flat_map(|r| &r.checks).filter(|c| c.outcome.word() == word).count();
    let generation_failures = results.iter().filter(|r| matches!(r.generated, Outcome::Fail(_))).count();
    json!({
        "readers": readers.iter().map(reader).collect::<Vec<_>>(),
        "summary": { "pass": count("pass"), "fail": count("fail") + generation_failures, "skip": count("skip") },
        "cases": results.iter().map(|r| json!({
            "name": r.name,
            "what": r.what,
            "pages": r.pages,
            "written": { "result": r.generated.word(), "detail": r.generated.detail() },
            "checks": r.checks.iter().map(|c| json!({
                "reader": c.reader, "check": c.name, "result": c.outcome.word(), "detail": c.outcome.detail(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Remove what an earlier run left in `out`, and nothing else: `--out` may be a directory that holds
/// other files (`--out .`), so the suite never deletes the directory itself.
fn clean(out: &Path) {
    for case in cases(out) {
        let _ = std::fs::remove_file(case.file(out));
        let _ = std::fs::remove_file(out.join(format!("{}.steps.json", case.name)));
        let _ = std::fs::remove_dir_all(out.join(case.name));
    }
    let _ = std::fs::remove_file(out.join("interop-id.p12"));
    let _ = std::fs::remove_file(out.join("interop.json"));
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out = args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)).map_or_else(|| gates::target_dir().join("interop"), PathBuf::from);
    let out = std::path::absolute(&out).with_context(|| format!("{}: not a usable directory", out.display()))?;
    let readers = [
        probe("mupdf", false, pdfcraft_oracle::mutool(), pdfcraft_oracle::mutool())?,
        probe("poppler", true, pdfcraft_oracle::pdfinfo(), pdfcraft_oracle::pdftoppm())?,
    ];
    for skipped in readers.iter().filter_map(|r| r.as_ref().err()) {
        eprintln!("{}", skipped.why);
    }

    gates::run_args(&["build", "--release", "-p", "pdfcraft-cli"])?;
    std::fs::create_dir_all(&out).with_context(|| format!("{}: cannot create", out.display()))?;
    clean(&out);
    let results = run_suite(&gates::release_cli(), &out, &readers);

    let json = serde_json::to_string_pretty(&report(&results, &readers))?;
    let json_path = out.join("interop.json");
    std::fs::write(&json_path, json).with_context(|| format!("{}: cannot write", json_path.display()))?;
    print!("{}", table(&results));
    println!("result: {}", json_path.display());
    if !failures(&results).is_empty() {
        bail!("some outputs do not interoperate (see above)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(generated: Outcome, checks: Vec<(&'static str, Outcome)>) -> CaseResult {
        CaseResult {
            name: "case",
            what: String::new(),
            pages: 1,
            generated,
            checks: checks.into_iter().map(|(reader, outcome)| Check { reader, name: "open", outcome }).collect(),
        }
    }

    #[test]
    fn a_missing_reader_skips_every_check_it_would_have_made() {
        let missing: Probe = Err(Skipped { name: "mupdf", why: "not installed".to_owned() });
        let out = Path::new("out");
        for case in cases(out) {
            let mut checks = Vec::new();
            check_reader(&missing, &case, out, &mut checks);
            let want = if case.encrypted { 5 } else { 2 };
            assert_eq!(checks.len(), want, "{}", case.name);
            assert!(checks.iter().all(|c| matches!(c.outcome, Outcome::Skip(_))));
        }
    }

    #[test]
    fn clean_removes_the_suites_files_and_keeps_the_rest() {
        let out = std::env::temp_dir().join(format!("pdfcraft-interop-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let first = cases(&out).into_iter().next().expect("the suite has cases");
        std::fs::create_dir_all(out.join(first.name)).unwrap();
        std::fs::write(out.join(first.name).join("page-1.png"), b"x").unwrap();
        for name in [format!("{}.pdf", first.name), format!("{}.steps.json", first.name), "interop.json".to_owned(), "interop-id.p12".to_owned()] {
            std::fs::write(out.join(name), b"x").unwrap();
        }
        std::fs::write(out.join("notes.txt"), b"keep").unwrap();
        std::fs::create_dir_all(out.join("other")).unwrap();
        std::fs::write(out.join("other").join("kept.txt"), b"keep").unwrap();

        clean(&out);

        let mut left: Vec<_> = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["notes.txt", "other"]);
        assert!(out.join("other").join("kept.txt").exists());
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn page_counts_read_from_both_readers_output() {
        assert_eq!(parse_pages("PDF-1.7\nPages: 3\n\nRetrieving info"), Some(3));
        assert_eq!(parse_pages("Title:  x\nPages:          12\nEncrypted: yes"), Some(12));
        assert_eq!(parse_pages("Pages: many"), None);
        assert_eq!(parse_pages(""), None);
    }

    #[test]
    fn only_errors_on_stderr_are_problems() {
        assert_eq!(stderr_problem("warning: ignoring object\nSyntax Warning: odd"), None);
        assert_eq!(stderr_problem("warning: x\nerror: cannot parse\n").as_deref(), Some("error: cannot parse"));
        assert_eq!(stderr_problem("Syntax Error (12): bad xref").as_deref(), Some("Syntax Error (12): bad xref"));
    }

    #[test]
    fn a_refusal_must_be_about_the_password() {
        assert!(mentions_password("error: cannot authenticate password: f.pdf"));
        assert!(mentions_password("Command Line Error: Incorrect password"));
        assert!(!mentions_password("error: cannot find startxref"));
        assert!(!mentions_password(""));
    }

    #[test]
    fn revisions_are_counted_by_their_end_markers() {
        assert_eq!(count_eof(b"%PDF-1.7\n%%EOF\n"), 1);
        assert_eq!(count_eof(b"%PDF-1.7\n%%EOF\nupdate\n%%EOF\n"), 2);
        assert_eq!(count_eof(b""), 0);
    }

    #[test]
    fn a_cell_fails_on_any_failed_check_and_skips_only_when_every_check_did() {
        let skip = || Outcome::Skip("not installed".into());
        let fail = || Outcome::Fail("bad".into());
        let r = result(Outcome::Pass, vec![("mupdf", Outcome::Pass), ("mupdf", fail()), ("poppler", skip()), ("poppler", skip())]);
        assert_eq!(cell(&r, "mupdf"), "FAIL");
        assert_eq!(cell(&r, "poppler"), "skip");
        let r = result(Outcome::Pass, vec![("mupdf", Outcome::Pass), ("mupdf", skip())]);
        assert_eq!(cell(&r, "mupdf"), "ok");
        assert_eq!(cell(&r, "poppler"), "-");
    }

    #[test]
    fn a_file_we_could_not_write_is_a_failure_even_with_no_reader_checks() {
        let results = vec![result(Outcome::Fail("step 2 (doc_protect): no".into()), vec![])];
        assert_eq!(failures(&results), vec!["case: pdfcraft could not write it: step 2 (doc_protect): no".to_owned()]);
        assert!(table(&results).contains("FAIL"));
        assert_eq!(report(&results, &[])["summary"]["fail"], 1);
    }

    #[test]
    fn every_case_writes_its_own_file_and_every_script_is_valid_json_steps() {
        let out = Path::new("/out");
        let all = cases(out);
        let mut names: Vec<_> = all.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len(), "case names must be unique");
        for case in &all {
            assert!(case.pages >= 1);
            assert!(case.steps.iter().all(|s| s["tool"].is_string() && s["args"].is_object()), "{}", case.name);
            let saved = case.file(out).to_string_lossy().into_owned();
            assert!(case.steps.iter().any(|s| s["args"]["path"] == saved || s["args"]["out"] == saved), "{} never writes {saved}", case.name);
        }
        // Encrypted output is the point of the suite: every supported algorithm is covered.
        let algorithms = all.iter().filter(|c| c.encrypted).count();
        assert_eq!(algorithms, 4);
    }
}
