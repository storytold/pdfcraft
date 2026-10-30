//! `cargo xtask ocr-texts <lang>`: ground-truth paragraphs for the OCR benchmark and regression
//! tests (`crates/engine/tests/data/ocr/`), taken from random articles of that language's
//! Wikipedia.
//!
//! The paragraphs whose letters are most often outside ASCII are kept, so the fixture exercises
//! the letters the language adds to the Latin alphabet. Each row records the article revision it
//! came from (CC BY-SA 4.0), and the command prints the `ATTRIBUTION.toml` entry the new file needs.
//! The text is fetched once and committed: the tests never touch the network.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::assets::{root, sha256_hex};

const USER_AGENT: &str = "pdfcraft-xtask-ocr-texts/1.0 (OCR ground truth; https://github.com/storytold/pdfcraft)";
const MIN_CHARS: usize = 300;
const MAX_CHARS: usize = 900;
/// Random batches asked for before giving up on finding enough paragraphs.
const ROUNDS: usize = 10;

pub fn run(args: &[String]) -> Result<()> {
    let mut lang = None;
    let mut docs = 3usize;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--docs" => docs = it.next().context("--docs needs a number")?.parse().context("--docs needs a number")?,
            "--out" => out = Some(PathBuf::from(it.next().context("--out needs a path")?)),
            s if s.starts_with('-') => bail!("unknown option {s}"),
            s => lang = Some(s.to_string()),
        }
    }
    let Some(lang) = lang else { bail!("usage: cargo xtask ocr-texts <lang> [--docs N] [--out FILE]") };
    if !lang.chars().all(|c| c.is_ascii_lowercase() || c == '-') || lang.is_empty() || lang.len() > 12 {
        bail!("{lang:?} is not a Wikipedia language code");
    }
    if docs == 0 || docs > 20 {
        bail!("--docs must be between 1 and 20");
    }
    let rel = format!("crates/engine/tests/data/ocr/{lang}-wikipedia.tsv");
    let path = out.unwrap_or_else(|| root().join(&rel));

    let picked = fetch(&lang, docs)?;
    let mut tsv = format!(
        "# OCR ground truth ({lang}): {docs} paragraphs from {lang}.wikipedia.org, CC BY-SA 4.0.\n\
         # Written by `cargo xtask ocr-texts {lang}`. Format: doc<TAB>source<TAB>text.\n"
    );
    for (i, p) in picked.iter().enumerate() {
        writeln!(tsv, "wiki{:02}\t{} ({})\t{}", i + 1, p.url, p.title, p.text)?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, &tsv)?;
    println!("ocr-texts: wrote {} ({} paragraphs)", path.display(), picked.len());
    let titles: Vec<String> = picked.iter().map(|p| format!("{} ({})", p.title, p.url)).collect();
    println!(
        "\nAdd to ATTRIBUTION.toml:\n\n[[asset]]\npath = \"{rel}\"\ntitle = \"OCR ground truth, {lang}: Wikipedia paragraphs\"\n\
         author = \"Wikipedia contributors\"\nsource = \"{}\"\nlicence = \"CC-BY-SA-4.0\"\nlicence_file = \"LICENSES/CC-BY-SA-4.0.txt\"\n\
         kind = \"data\"\nusage = \"OCR regression tests and benchmark (crates/engine/tests/it/ocr_ground_truth.rs, examples/ocr_bench.rs)\"\n\
         sha256 = \"{}\"",
        titles.join("; ").replace('"', "'"),
        sha256_hex(tsv.as_bytes())
    );
    Ok(())
}

struct Paragraph {
    title: String,
    url: String,
    text: String,
    /// Share of letters outside ASCII.
    special: f64,
}

fn fetch(lang: &str, docs: usize) -> Result<Vec<Paragraph>> {
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    for _ in 0..ROUNDS {
        let url = format!(
            "https://{lang}.wikipedia.org/w/api.php?format=json&formatversion=2&action=query&generator=random\
             &grnnamespace=0&grnlimit=20&prop=extracts%7Crevisions&rvprop=ids&explaintext=1&exintro=1&exlimit=20"
        );
        let out = Command::new("curl").args(["-sSfL", "--retry", "3", "-A", USER_AGENT]).arg(&url).output().context("running curl")?;
        if !out.status.success() {
            bail!("{url}: {}", String::from_utf8_lossy(&out.stderr));
        }
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).context("Wikipedia answered something that is not JSON")?;
        for page in json.pointer("/query/pages").and_then(|p| p.as_array()).into_iter().flatten() {
            let Some(id) = page.get("pageid").and_then(|v| v.as_u64()) else { continue };
            if !seen.insert(id) {
                continue;
            }
            let title = page.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let Some(rev) = page.pointer("/revisions/0/revid").and_then(|v| v.as_u64()) else { continue };
            let extract = page.get("extract").and_then(|v| v.as_str()).unwrap_or_default();
            let best = extract.split('\n').map(normalize_space).filter(|p| usable(p)).max_by_key(|p| p.chars().count());
            if let Some(text) = best {
                let special = special_share(&text);
                found.push(Paragraph { title, url: format!("https://{lang}.wikipedia.org/w/index.php?oldid={rev}"), text, special });
            }
        }
        if found.len() >= docs * 4 {
            break;
        }
    }
    if found.len() < docs {
        bail!("found only {} usable paragraphs in {lang}.wikipedia.org", found.len());
    }
    found.sort_by(|a, b| b.special.total_cmp(&a.special));
    found.truncate(docs);
    Ok(found)
}

fn normalize_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Long enough to be a paragraph, not a list heading, and only in scripts a Latin text face draws
/// (Basic Latin to Latin Extended-B, general punctuation and currency): a stray IPA transcription
/// or a name in another script would test the font, not the recogniser.
fn usable(p: &str) -> bool {
    let n = p.chars().count();
    (MIN_CHARS..=MAX_CHARS).contains(&n)
        && !p.ends_with(':')
        && p.chars().all(|c| matches!(c as u32, 0x20..=0x7E | 0xA0..=0x24F | 0x2010..=0x2027 | 0x2030..=0x205E | 0x20A0..=0x20C0))
}

fn special_share(p: &str) -> f64 {
    let letters = p.chars().filter(|c| c.is_alphabetic()).count();
    let special = p.chars().filter(|c| c.is_alphabetic() && !c.is_ascii()).count();
    if letters == 0 { 0.0 } else { special as f64 / letters as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_outside_latin_or_too_short_are_left_out() {
        let long = "Größe ".repeat(60);
        assert!(usable(long.trim()));
        assert!(!usable("kurz"));
        assert!(!usable(&format!("{long} [ɡʁøːsə]")), "IPA is outside Latin Extended-B");
        assert!(!usable(&format!("{long} Москва")));
        assert!(!usable(&format!("{}:", long.trim())));
    }

    #[test]
    fn special_share_counts_letters_outside_ascii() {
        assert_eq!(special_share("ab"), 0.0);
        assert_eq!(special_share("äb"), 0.5);
        assert_eq!(special_share("1 2"), 0.0);
    }
}
