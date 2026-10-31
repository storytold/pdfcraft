//! `cargo xtask arlington --from <checkout>`: compact the pinned Arlington PDF Model into
//! `crates/arlington/data/arlington.tsv`, the one file `pdfcraft-arlington` embeds.
//!
//! The model (<https://github.com/pdf-association/arlington-pdf-model>, Apache-2.0) is 613 TSV
//! files under `tsv/latest`. This reads them from a local checkout of the pinned commit, refuses
//! any other content (a SHA-256 digest over every file, so a different commit or a local edit is
//! caught), and joins them: each type starts with a `=Name` line, and the header rows and the free
//! text Note column are left out. Nothing is downloaded, the way `CRAFT_FONTS_DIR` works:
//!
//! ```text
//! git clone https://github.com/pdf-association/arlington-pdf-model ../arlington-pdf-model
//! git -C ../arlington-pdf-model checkout d37f4c8b11f54ae0d81a278bdeb91d00d6ccde26
//! cargo xtask arlington --from ../arlington-pdf-model          # writes the file
//! cargo xtask arlington --from ../arlington-pdf-model --check  # fails if it would change
//! ```
//!
//! Moving to a newer model: change [`COMMIT`], run with `--update-pin` to print the new digest,
//! put it in [`TREE_SHA256`], regenerate, and update the `ATTRIBUTION.toml` entry.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::assets::sha256_hex;

/// The model commit the committed table comes from.
pub const COMMIT: &str = "d37f4c8b11f54ae0d81a278bdeb91d00d6ccde26";

/// [`tree_digest`] of `tsv/latest` at [`COMMIT`].
pub const TREE_SHA256: &str = "1fdfc5a24f780edb88512c9eb6e049ec404c480e6e5a7e6e11348575e48dff9a";

/// Where the compacted model goes.
pub const OUTPUT: &str = "crates/arlington/data/arlington.tsv";

/// The header every model file starts with (INTERNAL_GRAMMAR.md, "TSV file rules").
const HEADER: &str =
    "Key\tType\tSinceVersion\tDeprecatedIn\tRequired\tIndirectReference\tInheritable\tDefaultValue\tPossibleValues\tSpecialCase\tLink\tNote";

/// Columns kept: all but the last (Note).
const KEPT: usize = 11;

pub fn run(args: &[String]) -> Result<()> {
    let mut from = None;
    let (mut check, mut update_pin) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--from" => from = it.next().map(PathBuf::from),
            "--check" => check = true,
            "--update-pin" => update_pin = true,
            other => bail!("unknown argument {other:?} (--from DIR, --check, --update-pin)"),
        }
    }
    let Some(from) = from else { bail!("--from DIR is required: a checkout of {COMMIT} (see `xtask/src/arlington.rs`)") };
    let dir = if from.join("tsv/latest").is_dir() { from.join("tsv/latest") } else { from };
    let files = read_model(&dir)?;
    let digest = tree_digest(&files);
    if update_pin {
        println!("arlington: {} files, tree digest {digest}", files.len());
        return Ok(());
    }
    if digest != TREE_SHA256 {
        bail!("{}: tree digest {digest} is not the pinned {TREE_SHA256} (commit {COMMIT}); refusing to use it", dir.display());
    }
    let table = compact(&files)?;
    let out = crate::assets::root().join(OUTPUT);
    if check {
        let committed = std::fs::read_to_string(&out).with_context(|| format!("reading {OUTPUT}"))?;
        if committed.replace("\r\n", "\n") != table {
            bail!("{OUTPUT} is out of date: run `cargo xtask arlington --from {}`", dir.display());
        }
        println!("arlington: {OUTPUT} matches the pinned model ({} types)", files.len());
        return Ok(());
    }
    std::fs::write(&out, &table).with_context(|| format!("writing {OUTPUT}"))?;
    println!("arlington: wrote {OUTPUT} ({} types, {} bytes, sha256 {})", files.len(), table.len(), sha256_hex(table.as_bytes()));
    Ok(())
}

/// Every `*.tsv` in `dir`, as (type name, text with LF line ends), sorted by name.
fn read_model(dir: &Path) -> Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".tsv")) else { continue };
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        files.push((name.to_string(), normalise(&text)));
    }
    files.sort();
    if files.is_empty() {
        bail!("{}: no .tsv files", dir.display());
    }
    Ok(files)
}

/// LF line ends, so a checkout made with `core.autocrlf` gives the same digest and table.
fn normalise(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// SHA-256 over `name\nsha256(text)\n` for each file in name order (texts already [`normalise`]d).
pub fn tree_digest(files: &[(String, String)]) -> String {
    let mut list = String::new();
    for (name, text) in files {
        let _ = writeln!(list, "{name}\n{}", sha256_hex(text.as_bytes()));
    }
    sha256_hex(list.as_bytes())
}

/// The joined table: a provenance header, then for each type a `=Name` line and its rows
/// without the header row and the Note column.
pub fn compact(files: &[(String, String)]) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!(
        "# Arlington PDF Model: tsv/latest of https://github.com/pdf-association/arlington-pdf-model at commit {COMMIT}.\n\
         # Copyright 2020 PDF Association, Inc. Licensed under the Apache License, Version 2.0 (LICENSE-APACHE; notice in NOTICE).\n\
         # Changed for PdfCraft by `cargo xtask arlington`: the {} files are joined into this one, each starting with a\n\
         # \"=Name\" line, and their header rows and Note column are left out. Do not edit by hand.\n\
         # Columns: Key, Type, SinceVersion, DeprecatedIn, Required, IndirectReference, Inheritable, DefaultValue,\n\
         # PossibleValues, SpecialCase, Link.\n",
        files.len()
    ));
    for (name, text) in files {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            bail!("{name}.tsv: unexpected file name");
        }
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            bail!("{name}.tsv: the header row is not the model's");
        }
        let _ = writeln!(out, "={name}");
        let mut rows = 0;
        for (n, line) in lines.enumerate() {
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != KEPT + 1 {
                bail!("{name}.tsv line {}: {} fields, not {}", n + 2, fields.len(), KEPT + 1);
            }
            out.push_str(&fields[..KEPT].join("\t"));
            out.push('\n');
            rows += 1;
        }
        if rows == 0 {
            bail!("{name}.tsv: no rows");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, rows: &[&str]) -> (String, String) {
        let mut text = format!("{HEADER}\n");
        for r in rows {
            text.push_str(r);
            text.push('\n');
        }
        (name.to_string(), text)
    }

    #[test]
    fn compact_drops_headers_and_notes() {
        let files = [file("Catalog", &["Type\tname\t1.0\t\tTRUE\tFALSE\tFALSE\t\t[Catalog]\t\t\tTable 29"])];
        let out = compact(&files).unwrap();
        assert!(out.lines().any(|l| l == "=Catalog"));
        assert!(out.lines().any(|l| l == "Type\tname\t1.0\t\tTRUE\tFALSE\tFALSE\t\t[Catalog]\t\t"), "{out}");
        assert!(!out.contains("Table 29"));
        assert!(!out.contains("\nKey\t"));
    }

    #[test]
    fn compact_rejects_malformed_files() {
        assert!(compact(&[("Bad".into(), "Key\tType\n".into())]).is_err(), "wrong header");
        assert!(compact(&[file("Short", &["Type\tname"])]).is_err(), "too few fields");
        assert!(compact(&[file("Empty", &[])]).is_err(), "no rows");
        assert!(compact(&[file("../x", &["Type\tname\t1.0\t\tTRUE\tFALSE\tFALSE\t\t\t\t\t"])]).is_err(), "odd name");
    }

    #[test]
    fn digest_ignores_line_ends_and_sees_edits() {
        let lf = vec![file("A", &["Type\tname\t1.0\t\tTRUE\tFALSE\tFALSE\t\t\t\t\t"])];
        let crlf: Vec<_> = lf.iter().map(|(n, t)| (n.clone(), normalise(&t.replace('\n', "\r\n")))).collect();
        assert_eq!(tree_digest(&lf), tree_digest(&crlf));
        let edited = vec![file("A", &["Type\tname\t1.1\t\tTRUE\tFALSE\tFALSE\t\t\t\t\t"])];
        assert_ne!(tree_digest(&lf), tree_digest(&edited));
    }

    /// The committed table is the one the pinned model compacts to, as far as it can be checked
    /// without the model: the header names the pinned commit and the file is attributed.
    #[test]
    fn committed_table_names_the_pinned_commit() {
        let root = crate::assets::root();
        let table = std::fs::read_to_string(root.join(OUTPUT)).unwrap();
        assert!(table.lines().next().is_some_and(|l| l.contains(COMMIT)));
        assert_eq!(table.lines().filter(|l| l.starts_with('=')).count(), 613);
        let manifest = std::fs::read_to_string(root.join("ATTRIBUTION.toml")).unwrap();
        assert!(manifest.contains(&format!("path = \"{OUTPUT}\"")));
        assert!(manifest.contains(COMMIT));
    }
}
