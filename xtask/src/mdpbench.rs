//! `cargo xtask mdpbench [--langs de,fr,…] [--dir DIR]`: fetch MDPBench's public split (Apache-2.0,
//! <https://huggingface.co/datasets/Delores-Lin/MDPBench>) for the OCR benchmark on real pages
//! (`cargo run --release -p pdfcraft-engine --example ocr_mdpbench`).
//!
//! The annotations and the pages of the chosen languages go to `target/mdpbench/` (git-ignored):
//! the pages are other people's documents, so they are never committed and no test reads them.
//! Everything comes from one pinned revision, the annotations checked by SHA-256; files already
//! there are kept, so a second run only fills gaps.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::assets::{root, sha256_hex};

/// The dataset revision the benchmark is pinned to.
const REVISION: &str = "252df132fe1b418dbd5830515ef4b5a06fa7164f";
const BASE: &str = "https://huggingface.co/datasets/Delores-Lin/MDPBench/resolve";
pub const ANNOTATIONS: &str = "MDPBench_public.json";
const ANNOTATIONS_SHA256: &str = "9b5f0f233917f5ab6c83c55dd526f66c930414254079097477d2ec5572e450f3";
/// The languages fetched without `--langs`: the ones written in the Latin alphabet.
const LATIN: &[&str] = &["de", "en", "es", "fr", "it", "nl", "pt"];

pub fn run(args: &[String]) -> Result<()> {
    let mut langs: Vec<String> = LATIN.iter().map(|l| l.to_string()).collect();
    let mut dir = root().join("target/mdpbench");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--langs" => langs = it.next().context("--langs needs a list")?.split(',').map(str::to_string).collect(),
            "--dir" => dir = PathBuf::from(it.next().context("--dir needs a path")?),
            s => bail!("unknown argument {s}; usage: cargo xtask mdpbench [--langs de,fr,…] [--dir DIR]"),
        }
    }
    if langs.iter().any(|l| l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
        bail!("--langs takes MDPBench's file prefixes, e.g. de,fr,zh-CHT");
    }
    let images = dir.join("images");
    std::fs::create_dir_all(&images).with_context(|| format!("creating {}", images.display()))?;

    let annotations = dir.join(ANNOTATIONS);
    if !annotations.is_file() {
        fetch(ANNOTATIONS, &annotations)?;
    }
    let bytes = std::fs::read(&annotations).with_context(|| format!("reading {}", annotations.display()))?;
    let sum = sha256_hex(&bytes);
    if sum != ANNOTATIONS_SHA256 {
        let _ = std::fs::remove_file(&annotations);
        bail!("{ANNOTATIONS} has SHA-256 {sum}, expected {ANNOTATIONS_SHA256}; removed it, run again");
    }
    let pages: serde_json::Value = serde_json::from_slice(&bytes).context("parsing the annotations")?;
    let wanted: Vec<String> = pages
        .as_array()
        .context("the annotations are not a list")?
        .iter()
        .filter_map(|p| p.pointer("/page_info/image_path")?.as_str())
        .filter(|name| langs.iter().any(|l| name.starts_with(&format!("{l}_"))))
        // Only plain file names: they become paths under images/.
        .filter(|name| !name.is_empty() && !name.contains(['/', '\\']) && !name.starts_with('.'))
        .map(str::to_string)
        .collect();
    if wanted.is_empty() {
        bail!("no pages for {}", langs.join(","));
    }
    let (mut fetched, mut kept, mut failed) = (0usize, 0usize, Vec::new());
    for (i, name) in wanted.iter().enumerate() {
        let path = images.join(name);
        if path.is_file() {
            kept += 1;
            continue;
        }
        // A few names carry spaces; one missing page is reported, not fatal.
        match fetch(&format!("MDPBench_img_public/{}", name.replace(' ', "%20")), &path) {
            Ok(()) => fetched += 1,
            Err(e) => failed.push(format!("{name}: {e}")),
        }
        if (i + 1) % 50 == 0 {
            println!("mdpbench: {} of {}", i + 1, wanted.len());
        }
    }
    println!(
        "mdpbench: {} pages for {} in {} ({fetched} fetched, {kept} already there)\n\
         run: cargo run --release -p pdfcraft-engine --example ocr_mdpbench -- {} --langs {}",
        wanted.len(),
        langs.join(","),
        dir.display(),
        dir.display(),
        langs.join(",")
    );
    if !failed.is_empty() {
        bail!("{} pages could not be fetched:\n{}", failed.len(), failed.join("\n"));
    }
    Ok(())
}

/// Download `file` of the pinned revision to `to`, through a temporary name so an interrupted
/// download is never taken for a complete one.
fn fetch(file: &str, to: &std::path::Path) -> Result<()> {
    let url = format!("{BASE}/{REVISION}/{file}");
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    let part = std::path::PathBuf::from(part);
    let status = Command::new("curl").args(["-sSfL", "--retry", "3", "-o"]).arg(&part).arg(&url).status().context("running curl")?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        bail!("could not fetch {url}");
    }
    std::fs::rename(&part, to).with_context(|| format!("moving {} into place", to.display()))?;
    Ok(())
}
