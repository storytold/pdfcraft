//! `cargo xtask web-worker-link [DIR]`: give the web build's OCR worker hashed file names and
//! tell the page where it is.
//!
//! Trunk names the page's files after a hash of their contents, so hosts can cache them forever,
//! but leaves a worker's files under their plain names (`pdfcraft-ocr-worker.js`, `_bg.wasm`,
//! `_loader.js`) and writes nothing into `index.html` for it. A browser holding an old worker
//! script with a new `.wasm` would break, so Trunk runs this after each build
//! (apps/pdfcraft-web/Trunk.toml, `TRUNK_STAGING_DIR`): it renames the three files to
//! `pdfcraft-ocr-worker-<hash>…`, updates the names inside them, and adds
//! `<meta name="pdfcraft-ocr-worker" content="./pdfcraft-ocr-worker-<hash>_loader.js">`, which
//! crates/ui-egui/src/workflows/ocr_web.rs reads.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The `<meta>` name the page looks for.
const META: &str = "pdfcraft-ocr-worker";
const STEM: &str = "pdfcraft-ocr-worker";

pub fn worker_link(args: &[String]) -> Result<()> {
    let dir = match args.first() {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("TRUNK_STAGING_DIR").context("give the site's folder, or run from a Trunk hook (TRUNK_STAGING_DIR)")?),
    };
    let read = |name: &str| std::fs::read(dir.join(name)).with_context(|| format!("reading {}", dir.join(name).display()));
    let worker = Worker { js: read(&format!("{STEM}.js"))?, wasm: read(&format!("{STEM}_bg.wasm"))?, loader: read(&format!("{STEM}_loader.js"))? };
    let hashed = worker.hashed()?;
    for (name, bytes) in &hashed.files {
        write(&dir.join(name), bytes)?;
    }
    for old in [format!("{STEM}.js"), format!("{STEM}_bg.wasm"), format!("{STEM}_loader.js")] {
        std::fs::remove_file(dir.join(&old)).with_context(|| format!("removing {old}"))?;
    }
    let html_path = dir.join("index.html");
    let html = std::fs::read_to_string(&html_path).with_context(|| format!("reading {}", html_path.display()))?;
    write(&html_path, add_worker_meta(&html, &hashed.loader)?.as_bytes())?;
    println!("web-worker-link: the page loads its OCR worker from {}", hashed.loader);
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// The worker's three files as Trunk wrote them.
struct Worker {
    js: Vec<u8>,
    wasm: Vec<u8>,
    loader: Vec<u8>,
}

struct Hashed {
    /// The files to write, by name.
    files: Vec<(String, Vec<u8>)>,
    /// The loader's new name.
    loader: String,
}

/// Replace the one occurrence of `from` in `text` with `to`.
fn replace_once(text: &str, from: &str, to: &str, what: &str) -> Result<String> {
    match text.matches(from).count() {
        1 => Ok(text.replacen(from, to, 1)),
        n => bail!("expected {what} to name {from} once, found {n}"),
    }
}

impl Worker {
    fn hashed(&self) -> Result<Hashed> {
        let mut both = self.js.clone();
        both.extend_from_slice(&self.wasm);
        let hash: String = crate::assets::sha256_hex(&both).chars().take(16).collect();
        let (js, wasm, loader) = (format!("{STEM}-{hash}.js"), format!("{STEM}-{hash}_bg.wasm"), format!("{STEM}-{hash}_loader.js"));
        let glue = std::str::from_utf8(&self.js).context("the worker's script is not UTF-8")?;
        let glue = replace_once(glue, &format!("{STEM}_bg.wasm"), &wasm, "the worker's script")?;
        let shim = std::str::from_utf8(&self.loader).context("the worker's loader is not UTF-8")?;
        let shim = replace_once(shim, &format!("./{STEM}.js"), &format!("./{js}"), "the worker's loader")?;
        Ok(Hashed { files: vec![(js, glue.into_bytes()), (wasm, self.wasm.clone()), (loader.clone(), shim.into_bytes())], loader })
    }
}

/// `html` with the meta tag naming `loader` (replacing an earlier one).
fn add_worker_meta(html: &str, loader: &str) -> Result<String> {
    if loader.contains(['"', '<', '>', '&']) {
        bail!("unexpected characters in the worker loader's name: {loader}");
    }
    let tag = format!("<meta name=\"{META}\" content=\"./{loader}\" />");
    let without: String = html.lines().filter(|l| !l.contains(&format!("name=\"{META}\""))).map(|l| format!("{l}\n")).collect();
    let Some(head_end) = without.find("</head>") else {
        bail!("index.html has no </head>");
    };
    Ok(format!("{}  {tag}\n  {}", without[..head_end].trim_end_matches([' ', '\t']), &without[head_end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trunk_output(wasm: &[u8]) -> Worker {
        Worker {
            js: b"function f(){ module_or_path = new URL('pdfcraft-ocr-worker_bg.wasm', import.meta.url); }".to_vec(),
            wasm: wasm.to_vec(),
            loader: b"import init from './pdfcraft-ocr-worker.js';await init();".to_vec(),
        }
    }

    #[test]
    fn worker_files_get_content_hashed_names_that_point_at_each_other() {
        let h = trunk_output(b"\0asm one").hashed().unwrap();
        let names: Vec<&str> = h.files.iter().map(|f| f.0.as_str()).collect();
        let hash = &h.loader["pdfcraft-ocr-worker-".len()..h.loader.len() - "_loader.js".len()];
        assert_eq!(hash.len(), 16);
        assert_eq!(names, [format!("{STEM}-{hash}.js"), format!("{STEM}-{hash}_bg.wasm"), h.loader.clone()]);
        let text = |i: usize| String::from_utf8(h.files[i].1.clone()).unwrap();
        assert!(text(0).contains(&format!("new URL('{STEM}-{hash}_bg.wasm', import.meta.url)")), "{}", text(0));
        assert_eq!(text(2), format!("import init from './{STEM}-{hash}.js';await init();"));
        assert_eq!(h.files[1].1, b"\0asm one");
        // A different build gets a different name, so a cached old worker is never paired with it.
        assert_ne!(trunk_output(b"\0asm two").hashed().unwrap().loader, h.loader);
    }

    #[test]
    fn unexpected_worker_files_are_refused() {
        let mut w = trunk_output(b"\0asm");
        w.js = b"no wasm named here".to_vec();
        assert!(w.hashed().is_err());
        let mut w = trunk_output(b"\0asm");
        w.loader = b"import a from './pdfcraft-ocr-worker.js'; import b from './pdfcraft-ocr-worker.js';".to_vec();
        assert!(w.hashed().is_err());
    }

    #[test]
    fn the_page_names_the_loader_once() {
        let html = "<html>\n  <head>\n    <title>PdfCraft</title>\n  </head>\n  <body></body>\n</html>\n";
        let once = add_worker_meta(html, "pdfcraft-ocr-worker-9f8e_loader.js").unwrap();
        assert!(once.contains("<meta name=\"pdfcraft-ocr-worker\" content=\"./pdfcraft-ocr-worker-9f8e_loader.js\" />\n  </head>"), "{once}");
        // Running again replaces the tag rather than adding another.
        let twice = add_worker_meta(&once, "pdfcraft-ocr-worker-7777_loader.js").unwrap();
        assert_eq!(twice.matches("pdfcraft-ocr-worker").count(), 2, "{twice}");
        assert!(twice.contains("./pdfcraft-ocr-worker-7777_loader.js"));
        assert!(twice.contains("<title>PdfCraft</title>") && twice.ends_with("</html>\n"));
        assert!(add_worker_meta(html, "pdfcraft-ocr-worker-\"x_loader.js").is_err());
        assert!(add_worker_meta("<body></body>", "pdfcraft-ocr-worker_loader.js").is_err());
    }
}
