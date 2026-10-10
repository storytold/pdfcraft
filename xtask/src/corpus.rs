//! The pdf.js test corpus, pinned to one commit and verified by content (AGENTS.md §2: shared
//! fixtures are "fetched pinned by commit and sha256-verified").
//!
//! Why this exists: the corpus is the evidence behind "963 of 983 files open with 0 crashes" and
//! behind the robustness baseline in `xtask/baselines/pdfjs.json`. Fetching upstream `HEAD`
//! instead of a fixed commit means those numbers describe a moving target — a file added or
//! changed upstream silently shifts the result, and a compromised upstream is never noticed. The
//! corpus is never committed here (it is git-ignored, and its files keep their own terms); only
//! the pin is.
//!
//! Verification is two-layer. The **commit** fixes which tree we asked for. The **manifest
//! digest** fixes what actually landed on disk: one `<sha256>  <path>` line per file, sorted by
//! path, then one sha256 over that text. Git's own object ids are SHA-1, so the manifest is what
//! makes this a sha256 check rather than a SHA-1 one.
//!
//! `cargo xtask corpus` fetches and verifies; `--update-pin` re-records the constants below after
//! a deliberate bump. A successful verification writes `corpus/pdfjs/.pdfcraft-corpus.json`, the
//! stamp that corpus tests look for so they never run against an unverified tree.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::assets::sha256_hex;
use crate::gates::root;

/// The pdf.js commit the corpus is pinned to. Bump deliberately, with `--update-pin`, and say in
/// the commit message why.
pub const PDFJS_COMMIT: &str = "5d923c9600b1da6772cca2e167356b13b8a9aa9e";

/// sha256 over the manifest of `test/pdfs` at [`PDFJS_COMMIT`]. See the module docs.
pub const PDFJS_MANIFEST_SHA256: &str = "692bfde052a16b191314da14e770178436b7c412f29d3d6cc0d9824b1e6be494";

/// How many files the manifest covered when the pin was recorded. A sanity check that a partial
/// or empty checkout can't pass as a match.
pub const PDFJS_FILES: usize = 1455;

/// The stamp a verified fetch leaves behind, named so tests can look for it.
pub const STAMP: &str = ".pdfcraft-corpus.json";

/// `corpus/pdfjs`, the checkout.
pub fn checkout() -> PathBuf {
    root().join("corpus").join("pdfjs")
}

/// `corpus/pdfjs/test/pdfs`, the PDFs themselves.
pub fn pdfs() -> PathBuf {
    checkout().join("test").join("pdfs")
}

/// Every regular file under `dir`, as paths relative to it with `/` separators, sorted. Symbolic
/// links and directories are not followed or included, so the manifest describes real bytes.
fn files_under(dir: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let path = entry?.path();
            // `symlink_metadata` does not follow links, so a link is skipped rather than hashed
            // through to wherever it points.
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let rel = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The manifest digest of `dir` and the number of files it covers.
pub fn manifest(dir: &Path) -> anyhow::Result<(String, usize)> {
    let files = files_under(dir)?;
    let mut text = String::with_capacity(files.len() * 80);
    for (rel, path) in &files {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        text.push_str(&sha256_hex(&bytes));
        text.push_str("  ");
        text.push_str(rel);
        text.push('\n');
    }
    Ok((sha256_hex(text.as_bytes()), files.len()))
}

/// What a check of the corpus on disk found.
#[derive(Debug, PartialEq)]
pub enum State {
    /// Nothing fetched. Corpus tests skip; the sweep refuses to run.
    Absent,
    /// Present and matching the pin.
    Verified { files: usize },
    /// Present but not what the pin describes. Never silently ignored.
    Mismatch { detail: String },
}

/// Check the corpus on disk against the pin, without fetching.
pub fn state() -> anyhow::Result<State> {
    let dir = pdfs();
    if !dir.is_dir() {
        return Ok(State::Absent);
    }
    let (digest, files) = manifest(&dir)?;
    if files != PDFJS_FILES {
        return Ok(State::Mismatch { detail: format!("{files} files on disk, the pin records {PDFJS_FILES}") });
    }
    if digest != PDFJS_MANIFEST_SHA256 {
        return Ok(State::Mismatch { detail: format!("manifest sha256 is {digest}, the pin records {PDFJS_MANIFEST_SHA256}") });
    }
    Ok(State::Verified { files })
}

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir);
    c
}

fn run(cmd: &mut Command, what: &str) -> anyhow::Result<()> {
    let status = cmd.status().with_context(|| format!("running {what}"))?;
    if !status.success() {
        bail!("{what} failed with {status}");
    }
    Ok(())
}

fn capture(cmd: &mut Command, what: &str) -> anyhow::Result<String> {
    let out = cmd.output().with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        bail!("{what} failed with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Set up the checkout. `core.autocrlf=false` and `core.eol=lf` matter: with the global
/// `autocrlf=true` this repository uses, git would rewrite line endings on checkout and the same
/// commit would produce different bytes — and so a different manifest digest — on Windows than on
/// Linux. The pin has to mean the same thing everywhere.
fn init(dir: &Path) -> anyhow::Result<()> {
    run(git(dir).args(["init", "-q"]), "git init")?;
    run(git(dir).args(["config", "core.autocrlf", "false"]), "git config core.autocrlf")?;
    run(git(dir).args(["config", "core.eol", "lf"]), "git config core.eol")?;
    run(git(dir).args(["remote", "add", "origin", "https://github.com/mozilla/pdf.js.git"]), "git remote add")?;
    run(git(dir).args(["sparse-checkout", "set", "test/pdfs"]), "git sparse-checkout set")?;
    Ok(())
}

/// Fetch exactly [`PDFJS_COMMIT`], without the history or the blobs we don't need.
fn fetch(commit: &str) -> anyhow::Result<()> {
    let dir = checkout();
    std::fs::create_dir_all(&dir)?;
    if !dir.join(".git").is_dir() {
        init(&dir)?;
    }
    // Asking for one commit by id: GitHub serves these, and it keeps the fetch to the tree we
    // pinned rather than whatever HEAD happens to be.
    run(git(&dir).args(["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", commit]), "git fetch")?;
    run(git(&dir).args(["checkout", "-q", "--force", commit]), "git checkout")?;
    Ok(())
}

/// Write the stamp that corpus tests look for.
fn write_stamp(commit: &str, digest: &str, files: usize) -> anyhow::Result<()> {
    let stamp = serde_json::json!({
        "source": "https://github.com/mozilla/pdf.js.git",
        "commit": commit,
        "manifest_sha256": digest,
        "files": files,
    });
    std::fs::write(checkout().join(STAMP), serde_json::to_string_pretty(&stamp)? + "\n")?;
    Ok(())
}

/// `cargo xtask corpus [--update-pin]`.
pub fn run_cmd(args: &[String]) -> anyhow::Result<()> {
    let update = args.iter().any(|a| a == "--update-pin");

    if update {
        // Bumping on purpose: take whatever the default branch points at now, record it, and
        // print the constants to paste back. Nothing is verified against the old pin.
        let dir = checkout();
        std::fs::create_dir_all(&dir)?;
        if !dir.join(".git").is_dir() {
            init(&dir)?;
        }
        run(git(&dir).args(["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", "HEAD"]), "git fetch HEAD")?;
        run(git(&dir).args(["checkout", "-q", "--force", "FETCH_HEAD"]), "git checkout FETCH_HEAD")?;
        let commit = capture(git(&dir).args(["rev-parse", "HEAD"]), "git rev-parse")?;
        let (digest, files) = manifest(&pdfs())?;
        write_stamp(&commit, &digest, files)?;
        println!(
            "Pin these in xtask/src/corpus.rs, and say in the commit message why the bump:\n\n\
             pub const PDFJS_COMMIT: &str = \"{commit}\";\n\
             pub const PDFJS_MANIFEST_SHA256: &str = \"{digest}\";\n\
             pub const PDFJS_FILES: usize = {files};\n"
        );
        return Ok(());
    }

    if PDFJS_COMMIT.chars().all(|c| c == '0') {
        bail!(
            "the corpus pin has not been recorded yet.\n\
             Run `cargo xtask corpus --update-pin`, then paste the printed constants into xtask/src/corpus.rs."
        );
    }

    match state()? {
        State::Verified { files } => {
            println!("corpus/pdfjs already present and verified: {files} files at {PDFJS_COMMIT}");
            write_stamp(PDFJS_COMMIT, PDFJS_MANIFEST_SHA256, files)?;
            return Ok(());
        }
        State::Mismatch { detail } => {
            // Re-fetching can repair a half-finished checkout, so say what was wrong and carry on
            // to the fetch; the check after it is the one that decides.
            println!("corpus/pdfjs does not match the pin ({detail}); re-fetching");
        }
        State::Absent => {}
    }

    fetch(PDFJS_COMMIT)?;

    let head = capture(git(&checkout()).args(["rev-parse", "HEAD"]), "git rev-parse")?;
    if head != PDFJS_COMMIT {
        bail!("checked out {head}, expected {PDFJS_COMMIT}");
    }
    match state()? {
        State::Verified { files } => {
            write_stamp(PDFJS_COMMIT, PDFJS_MANIFEST_SHA256, files)?;
            println!(
                "corpus/pdfjs verified: {files} files at {PDFJS_COMMIT}\n  \
                 manifest sha256 {PDFJS_MANIFEST_SHA256}\n  \
                 git-ignored and never committed; individual files keep their own terms"
            );
            Ok(())
        }
        State::Absent => bail!("the fetch left no {} directory", pdfs().display()),
        State::Mismatch { detail } => bail!(
            "corpus does not match the pin after fetching: {detail}\n\
             Upstream may have changed, or the checkout is damaged. Delete corpus/pdfjs and retry; \
             if it still differs, the pin needs a deliberate bump with `--update-pin`."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pdfcraft-corpus-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    #[test]
    fn manifest_is_stable_and_order_independent() {
        let a = tmp("order-a");
        std::fs::create_dir_all(a.join("sub")).expect("sub");
        std::fs::write(a.join("b.pdf"), b"two").expect("write");
        std::fs::write(a.join("a.pdf"), b"one").expect("write");
        std::fs::write(a.join("sub/c.pdf"), b"three").expect("write");
        let first = manifest(&a).expect("manifest");

        // The same bytes written in a different order hash the same: the manifest sorts by path.
        let b = tmp("order-b");
        std::fs::create_dir_all(b.join("sub")).expect("sub");
        std::fs::write(b.join("sub/c.pdf"), b"three").expect("write");
        std::fs::write(b.join("a.pdf"), b"one").expect("write");
        std::fs::write(b.join("b.pdf"), b"two").expect("write");
        assert_eq!(first, manifest(&b).expect("manifest"));
        assert_eq!(first.1, 3);

        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn one_changed_byte_changes_the_digest() {
        let d = tmp("changed");
        std::fs::write(d.join("a.pdf"), b"one").expect("write");
        let before = manifest(&d).expect("manifest");
        std::fs::write(d.join("a.pdf"), b"onX").expect("write");
        let after = manifest(&d).expect("manifest");
        assert_ne!(before.0, after.0, "a changed byte must change the manifest digest");
        assert_eq!(before.1, after.1, "the file count is unchanged");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_renamed_file_changes_the_digest() {
        let d = tmp("renamed");
        std::fs::write(d.join("a.pdf"), b"one").expect("write");
        let before = manifest(&d).expect("manifest");
        std::fs::rename(d.join("a.pdf"), d.join("b.pdf")).expect("rename");
        let after = manifest(&d).expect("manifest");
        assert_ne!(before.0, after.0, "the path is part of the manifest, not just the bytes");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_added_file_changes_the_count() {
        let d = tmp("added");
        std::fs::write(d.join("a.pdf"), b"one").expect("write");
        assert_eq!(manifest(&d).expect("manifest").1, 1);
        std::fs::write(d.join("b.pdf"), b"two").expect("write");
        assert_eq!(manifest(&d).expect("manifest").1, 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_empty_directory_hashes_without_failing() {
        let d = tmp("empty");
        let (digest, files) = manifest(&d).expect("manifest");
        assert_eq!(files, 0);
        assert_eq!(digest, sha256_hex(b""), "an empty manifest is the digest of no text");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_directory_is_absent_not_a_failure() {
        let d = tmp("missing");
        let _ = std::fs::remove_dir_all(&d);
        assert!(manifest(&d).is_err(), "manifest of a missing directory is an error the caller reports");
    }
}
