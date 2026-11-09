//! `cargo xtask fuzz`: a mutation fuzzer that runs on stable Rust (M1.10).
//!
//! Seeds are small PDFs (synthetic ones, plus pdf.js corpus files under 64 KB when `corpus/` is
//! present). Each iteration mutates a seed, writes it to `fuzz-out/work/`, and runs
//! `pdfcraft-cli check-one <file> --edit` in a child process with a timeout. That opens,
//! inspects, renders and extracts text from every page (the bootstrap renderer), then runs our
//! object layer end to end: decode streams, edit, save incrementally and in full, reopen.
//!
//! A child that dies (panic outside a safety net, abort, stack overflow, signal) is a **crash**,
//! and one that runs past the timeout is a **hang**. Each finding is minimized (chunks are
//! removed while it still fails the same way) and kept in `fuzz-out/findings/` with a JSON note.
//! `fuzz-out/` is git-ignored; findings derived from corpus files must not be committed. Turn
//! each real bug into a synthetic regression test instead.
//!
//! ```text
//! cargo xtask fuzz [--time 300] [--iterations N] [--seed 1] [--jobs 8] [--timeout 10]
//! ```
//! It exits non-zero when it finds a crash (hangs are reported but, like the corpus sweep's
//! known renderer hangs, do not fail the run).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::gates::{release_cli, root};

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// xorshift64*: small, deterministic, good enough for mutation choices.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// Bytes that matter to PDF syntax, and tokens worth splicing in.
const SYNTAX: &[u8] = b"0123456789 /<>[]()R\n\r%-.+#\\";
const TOKENS: &[&[u8]] = &[
    b" 0 R",
    b" 99999 0 R",
    b" -1 ",
    b" 2147483648 ",
    b" 1e308 ",
    b"<<",
    b">>",
    b"[",
    b"]",
    b"(",
    b")",
    b"stream\n",
    b"\nendstream",
    b"endobj",
    b" obj ",
    b"/Length 0",
    b"/Length 99999999",
    b"/Filter /FlateDecode",
    b"/Filter [/ASCIIHexDecode /LZWDecode]",
    b"/DecodeParms << /Predictor 15 /Columns 99999 >>",
    b"/Type /ObjStm /N 99999 /First 0",
    b"/Type /XRef /W [1 9 1]",
    b"/Kids [1 0 R]",
    b"/Parent 1 0 R",
    b"/Prev 0",
    b"xref\n0 1\n",
    b"trailer << /Root 1 0 R >>",
    b"startxref\n0\n%%EOF",
    b"/Encrypt << /Filter /Standard /V 5 /R 6 >>",
    b"/Count -5",
    b"/MediaBox [0 0 0 0]",
    b"/Rotate 45",
];

fn mutate(rng: &mut Rng, seed: &[u8], other: &[u8]) -> Vec<u8> {
    let mut d = seed.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        if d.is_empty() {
            d.extend_from_slice(b"%PDF-1.7\n");
        }
        let at = rng.below(d.len());
        match rng.below(9) {
            0 => d[at] ^= 1 << rng.below(8),
            1 => d[at] = SYNTAX[rng.below(SYNTAX.len())],
            2 => {
                let n = 1 + rng.below(64.min(d.len() - at));
                d.drain(at..at + n);
            }
            3 => {
                let n = 1 + rng.below(256.min(d.len() - at));
                let chunk = d[at..at + n].to_vec();
                let to = rng.below(d.len());
                d.splice(to..to, chunk);
            }
            4 => {
                let t = TOKENS[rng.below(TOKENS.len())];
                d.splice(at..at, t.iter().copied());
            }
            5 => d.truncate(at.max(1)),
            6 if !other.is_empty() => {
                let from = rng.below(other.len());
                let n = 1 + rng.below(512.min(other.len() - from));
                d.splice(at..at, other[from..from + n].iter().copied());
            }
            7 => {
                // Replace a run of digits with an extreme number.
                if let Some(start) = d[at..].iter().position(u8::is_ascii_digit).map(|p| p + at) {
                    let end = d[start..].iter().position(|c| !c.is_ascii_digit()).map_or(d.len(), |p| p + start);
                    let v: &[u8] = [&b"0"[..], b"4294967295", b"9223372036854775807", b"65535", b"1"][rng.below(5)];
                    d.splice(start..end, v.iter().copied());
                }
            }
            _ => {
                // Corrupt compressed data: flip bytes inside a stream body.
                if let Some(s) = find(&d[at..], b"stream").map(|p| p + at + 7) {
                    for _ in 0..1 + rng.below(8) {
                        if s < d.len() {
                            let i = s + rng.below((d.len() - s).min(512));
                            d[i] = rng.next() as u8;
                        }
                    }
                }
            }
        }
    }
    d
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A classic-xref PDF whose objects are numbered 1.. in order. ASCII object text only, so the
/// byte offsets are the string offsets.
fn build(objs: &[&str], trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} {trailer} >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// An uncompressed stream object body (`Filter`-free, so the seed stays plain text).
fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// Every feature the engine reads on a page: fonts (Type 1 and Type 3), form fields with
/// actions, the annotation kinds, page labels, names, optional content, a structure tree,
/// shading, a form XObject, an image with a soft mask, a colour space and a graphics state.
fn feature_rich_seed() -> Vec<u8> {
    let content = stream(
        "",
        "q /G1 gs BT /F1 18 Tf 10 250 Td (Hello) Tj /T3 12 Tf 0 -20 Td (aaa) Tj ET Q /Sh1 sh \
         q 50 0 0 50 100 20 cm /Fm1 Do Q q 40 0 0 40 20 20 cm /Im1 Do Q \
         /OC /OC1 BDC 1 0 0 rg 0 0 20 20 re f EMC /CS1 cs 1 sc 150 10 30 30 re f",
    );
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 50 50]", "0 1 0 rg 0 0 50 50 re f");
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 24 0 R",
        "\u{0}\u{40}\u{7f}\u{20}",
    );
    let charproc = stream("", "10 0 0 0 10 10 d1 0 0 10 10 re f");
    let appearance = stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", "0 0 1 1 re f");
    let mask = stream("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8", "\u{7f}\u{7f}\u{20}\u{0}");
    build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Lang (en) /MarkInfo << /Marked true >> /StructTreeRoot 13 0 R \
             /AcroForm << /Fields [8 0 R 9 0 R 10 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >> \
             /PageLabels << /Nums [0 << /S /D /St 1 >>] >> /Names << /JavaScript << /Names [(init) 11 0 R] >> >> \
             /OCProperties << /OCGs [12 0 R] /D << /Order [12 0 R] /ON [12 0 R] >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /StructParents 0 /Annots [8 0 R 9 0 R 10 0 R 17 0 R 18 0 R 19 0 R 20 0 R] \
             /Resources << /Font << /F1 5 0 R /T3 7 0 R >> /Shading << /Sh1 14 0 R >> /XObject << /Fm1 15 0 R /Im1 16 0 R >> \
             /ExtGState << /G1 << /ca 0.5 /BM /Multiply >> >> /Properties << /OC1 12 0 R >> \
             /ColorSpace << /CS1 [/Indexed /DeviceRGB 1 <ff000000ff00>] >> >> >>",
            &content,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /sq 21 0 R >> \
             /Encoding << /Type /Encoding /Differences [97 /sq] >> /FirstChar 97 /LastChar 97 /Widths [10] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (t) /V (v) /Rect [10 10 90 30] /P 3 0 R \
             /AA << /K << /S /JavaScript /JS (AFNumber_Keystroke\\(2,0,0,0,\"\",true\\);) >> >> >>",
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (c) /V /Yes /AS /Yes /Rect [10 40 30 60] /P 3 0 R \
             /AP << /N << /Yes 22 0 R /Off 22 0 R >> >> >>",
            "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (d) /Opt [(a) (b)] /V (a) /Rect [10 70 90 90] /P 3 0 R >>",
            "<< /S /JavaScript /JS (var x = 1;) >>",
            "<< /Type /OCG /Name (Layer) >>",
            "<< /Type /StructTreeRoot /K 23 0 R /ParentTree << /Nums [0 [23 0 R]] >> >>",
            "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 100 0] \
             /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> /Extend [true true] >>",
            &form,
            &image,
            "<< /Type /Annot /Subtype /Link /Rect [10 120 90 140] /Border [0 0 0] /A << /S /URI /URI (http://example.com) >> >>",
            "<< /Type /Annot /Subtype /Highlight /Rect [10 80 90 100] /QuadPoints [10 100 90 100 10 80 90 80] /C [1 1 0] \
             /Contents (note) /Popup 25 0 R >>",
            "<< /Type /Annot /Subtype /Ink /Rect [10 10 40 30] /InkList [[10 10 20 30 40 20]] /C [1 0 0] >>",
            "<< /Type /Annot /Subtype /FreeText /Rect [100 200 190 240] /DA (/Helv 12 Tf 0 g) /Contents (text) >>",
            &charproc,
            &appearance,
            "<< /Type /StructElem /S /P /P 13 0 R /Pg 3 0 R /K 0 >>",
            &mask,
            "<< /Type /Annot /Subtype /Popup /Rect [100 100 150 150] /Parent 18 0 R >>",
        ],
        "/Root 1 0 R /Info << /Title (Rich) >>",
    )
}

/// The same small document stored the modern way: an uncompressed object stream and a
/// cross-reference stream, so the tolerant and the strict readers both walk the compressed-object
/// and `/Index` paths.
fn object_stream_seed() -> Vec<u8> {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
    ];
    let nums = [1u32, 2, 3, 5];
    let mut header = String::new();
    let mut body = String::new();
    for (n, b) in nums.iter().zip(bodies) {
        header.push_str(&format!("{n} {} ", body.len()));
        body.push_str(b);
        body.push('\n');
    }
    let objstm_data = format!("{header}{body}");
    let mut out = b"%PDF-1.7\n".to_vec();
    let content_off = out.len();
    out.extend_from_slice(format!("4 0 obj\n{}\nendobj\n", stream("", "BT /F1 14 Tf 10 100 Td (Object stream) Tj ET")).as_bytes());
    let objstm_off = out.len();
    let dict = format!("/Type /ObjStm /N {} /First {}", nums.len(), header.len());
    out.extend_from_slice(format!("6 0 obj\n{}\nendobj\n", stream(&dict, &objstm_data)).as_bytes());
    let xref_off = out.len();
    // /W [1 3 1]: type, offset (or object-stream number), generation (or index in the stream).
    let entry = |t: u8, f: usize, g: u8| [t, (f >> 16) as u8, (f >> 8) as u8, f as u8, g];
    let rows = [
        entry(0, 0, 255),
        entry(2, 6, 0),
        entry(2, 6, 1),
        entry(2, 6, 2),
        entry(1, content_off, 0),
        entry(2, 6, 3),
        entry(1, objstm_off, 0),
        entry(1, xref_off, 0),
    ];
    out.extend_from_slice(
        format!("7 0 obj\n<< /Type /XRef /Size 8 /W [1 3 1] /Index [0 8] /Root 1 0 R /Length {} >>\nstream\n", rows.len() * 5).as_bytes(),
    );
    for r in rows {
        out.extend_from_slice(&r);
    }
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_off}\n%%EOF\n").as_bytes());
    out
}

/// A tiny document plus an incremental update (a new annotation and a rewritten page) chained
/// by `/Prev`, so mutations reach the revision walker and the incremental save path.
fn incremental_update_seed() -> Vec<u8> {
    let mut out = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ],
        "/Root 1 0 R",
    );
    let prev = out.windows(10).rposition(|w| w == b"startxref\n").and_then(|i| {
        let tail = String::from_utf8_lossy(&out[i + 10..]).into_owned();
        tail.lines().next().and_then(|l| l.trim().parse::<usize>().ok())
    });
    let prev = prev.unwrap_or(0);
    let page_off = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>\nendobj\n");
    let annot_off = out.len();
    out.extend_from_slice(b"4 0 obj\n<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [0 0 1] /Contents (update) >>\nendobj\n");
    let x = out.len();
    out.extend_from_slice(
        format!(
            "xref\n3 2\n{page_off:010} 00000 n \n{annot_off:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Prev {prev} >>\nstartxref\n{x}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// A file that opens, whose graphs loop: a page whose annotation array holds the page, a form
/// XObject that draws itself and an outline item that is its own sibling. Every walker needs a
/// depth limit or a seen-set; these are what they meet. The page tree stays sound on purpose: a
/// kid that lists its own parent makes the whole document refuse to open ("no pages"), and a seed
/// refused at open only reaches the error path.
fn cyclic_seed() -> Vec<u8> {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /Fm1 4 0 R >> >>", "/Fm1 Do 0 0 10 10 re f");
    build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /XObject << /Fm1 4 0 R >> >> /Annots [3 0 R] >>",
            &form,
            "<< /Type /Outlines /First 6 0 R /Last 6 0 R /Count 1 >>",
            "<< /Title (Loop) /Parent 5 0 R /Next 6 0 R /Prev 6 0 R /Dest [3 0 R /Fit] >>",
        ],
        "/Root 1 0 R",
    )
}

/// Synthetic seeds, so fuzzing works without the corpus: a small mixed document, a minimal one,
/// a feature-rich page, an object-stream file with a cross-reference stream, an incremental
/// update and looping object graphs. There is no encrypted seed yet: `xtask` does not depend on
/// the crypt crate, and a hand-rolled encryptor here would be a second implementation to keep
/// right.
fn synthetic_seeds() -> Vec<Vec<u8>> {
    let content = "BT /F1 24 Tf 20 150 Td (Hello) Tj ET 0 0 1 rg 10 10 50 50 re f q 20 0 0 20 100 100 cm BI /W 2 /H 2 /BPC 8 /CS /G ID \u{0}\u{ff}\u{ff}\u{0} EI Q";
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    vec![
        build(
            &[
                "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> /Outlines 7 0 R >>",
                "<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 /MediaBox [0 0 200 300] >>",
                "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>",
                &stream,
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /V (v) /Rect [10 10 90 30] /P 3 0 R >>",
                "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>",
                "<< /Type /Pages /Parent 2 0 R /Kids [10 0 R] /Count 1 /Rotate 90 >>",
                "<< /Title (One) /Parent 7 0 R /Dest [3 0 R /Fit] >>",
                "<< /Type /Page /Parent 8 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
            ],
            "/Root 1 0 R /Info << /Title (Fuzz) >>",
        ),
        build(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>",
            ],
            "/Root 1 0 R",
        ),
        feature_rich_seed(),
        object_stream_seed(),
        incremental_update_seed(),
        cyclic_seed(),
    ]
}

fn seeds() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = synthetic_seeds().into_iter().enumerate().map(|(i, b)| (format!("synthetic-{i}"), b)).collect();
    // Seeds the corpus sweep already knows to hang only rediscover that hang.
    let known_hangs: std::collections::HashSet<String> = std::fs::read_to_string(root().join("xtask/baselines/pdfjs.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<std::collections::BTreeMap<String, String>>(&t).ok())
        .map(|m| m.into_iter().filter(|(_, v)| v == "timeout").map(|(k, _)| k).collect())
        .unwrap_or_default();
    let dir = root().join("corpus/pdfjs/test/pdfs");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pdf")).collect();
        files.sort();
        for f in files {
            let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if known_hangs.contains(&name) {
                continue;
            }
            if let Ok(b) = std::fs::read(&f)
                && b.len() <= 64 * 1024
            {
                out.push((f.file_name().unwrap_or_default().to_string_lossy().into_owned(), b));
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Crash,
    Hang,
}

/// Address-space cap for each child on Linux and FreeBSD, in KiB (4 GiB). An input that makes us allocate
/// without bound then fails its allocation in the child (an abort, kept as a crash finding)
/// instead of exhausting the machine: on CI that killed the runner and lost the findings.
const CHILD_ADDRESS_SPACE_KIB: u64 = 4 * 1024 * 1024;

/// The child process, under the address-space cap where the shell can set one (Linux and
/// FreeBSD; macOS does not support `ulimit -v` and Windows has no `sh`).
fn child_command(exe: &Path) -> Command {
    if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        let mut c = Command::new("sh");
        c.args(["-c", &format!("ulimit -v {CHILD_ADDRESS_SPACE_KIB} && exec \"$0\" \"$@\"")]).arg(exe);
        c
    } else {
        Command::new(exe)
    }
}

fn run_one(exe: &Path, file: &Path, timeout: Duration) -> (Outcome, String) {
    let child =
        child_command(exe).args(["check-one", &file.to_string_lossy(), "--dpi", "18", "--edit"]).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let Ok(mut child) = child else { return (Outcome::Ok, String::new()) };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return (Outcome::Ok, String::new());
                }
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
                // Keep the first panic location or abort message: it identifies the bug.
                let what = err
                    .lines()
                    .find(|l| l.contains("panicked at") || l.contains("overflow") || l.contains("fatal") || l.contains("memory allocation"))
                    .unwrap_or("")
                    .trim();
                // Drop the per-process thread id ("thread 'main' (12345)") so runs compare equal.
                let what: String = what
                    .split(" (")
                    .map(|part| match part.split_once(')') {
                        Some((id, rest)) if id.chars().all(|c| c.is_ascii_digit()) => rest.to_string(),
                        _ => format!(" ({part}"),
                    })
                    .collect::<String>()
                    .trim_start_matches(" (")
                    .to_string();
                return (Outcome::Crash, format!("{status}; {what}"));
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return (Outcome::Hang, format!("over {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return (Outcome::Ok, e.to_string()),
        }
    }
}

/// Remove chunks while the input still fails with the same outcome and message.
fn minimize(exe: &Path, work: &Path, data: Vec<u8>, outcome: Outcome, what: &str, timeout: Duration) -> Vec<u8> {
    let mut best = data;
    let mut chunk = best.len() / 2;
    let mut tries = 0;
    let probe = work.join("minimize.pdf");
    while chunk >= 1 && tries < 300 {
        let mut i = 0;
        let mut progressed = false;
        while i < best.len() && tries < 300 {
            let mut cand = best.clone();
            cand.drain(i..(i + chunk).min(cand.len()));
            tries += 1;
            if std::fs::write(&probe, &cand).is_err() {
                break;
            }
            let (o, w) = run_one(exe, &probe, timeout);
            if o == outcome && (outcome == Outcome::Hang || w == what) {
                best = cand;
                progressed = true;
            } else {
                i += chunk;
            }
        }
        if !progressed {
            chunk /= 2;
        }
    }
    let _ = std::fs::remove_file(probe);
    best
}

pub fn run(args: &[String]) -> Result<()> {
    let time = Duration::from_secs(flag(args, "--time").unwrap_or("300").parse()?);
    let iterations: usize = flag(args, "--iterations").map(str::parse).transpose()?.unwrap_or(usize::MAX);
    let seed: u64 = flag(args, "--seed").unwrap_or("1").parse()?;
    // Half the cores by default: a saturated machine turns slow inputs into false hangs.
    let jobs: usize =
        flag(args, "--jobs").map(str::parse).transpose()?.unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)));
    let timeout = Duration::from_secs(flag(args, "--timeout").unwrap_or("10").parse()?);

    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root())
        .args(["build", "--release", "-q", "-p", "pdfcraft-cli"])
        .status()?;
    if !status.success() {
        bail!("building pdfcraft-cli failed");
    }
    let exe = release_cli();
    if !exe.is_file() {
        // Every run would fail to start and count as a pass.
        bail!("{} not found after building it", exe.display());
    }
    let out = root().join("fuzz-out");
    let work = out.join("work");
    let findings = out.join("findings");
    std::fs::create_dir_all(&work)?;
    std::fs::create_dir_all(&findings)?;
    let seeds = Arc::new(seeds());
    println!("fuzz: {} seeds, {jobs} jobs, {} s, timeout {} s, seed {seed}", seeds.len(), time.as_secs(), timeout.as_secs());

    let done = Arc::new(AtomicUsize::new(0));
    let found: Arc<Mutex<Vec<(Outcome, String, String)>>> = Arc::default();
    // Held while confirming a hang; the other jobs keep running, so this only serializes
    // confirmations with each other.
    let solo = Arc::new(Mutex::new(()));
    let start = Instant::now();
    std::thread::scope(|s| {
        for job in 0..jobs {
            let (seeds, done, found, exe, work, findings, solo) =
                (seeds.clone(), done.clone(), found.clone(), exe.clone(), work.clone(), findings.clone(), solo.clone());
            s.spawn(move || {
                let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(job as u64 + 1) | 1);
                let file = work.join(format!("job{job}.pdf"));
                while start.elapsed() < time {
                    let n = done.fetch_add(1, Ordering::Relaxed);
                    if n >= iterations {
                        break;
                    }
                    let (name, base) = &seeds[rng.below(seeds.len())];
                    let other = &seeds[rng.below(seeds.len())].1;
                    let data = mutate(&mut rng, base, other);
                    if std::fs::write(&file, &data).is_err() {
                        continue;
                    }
                    let (outcome, what) = run_one(&exe, &file, timeout);
                    if outcome == Outcome::Ok {
                        continue;
                    }
                    let key = format!("{outcome:?}:{what}");
                    {
                        let f = found.lock().expect("lock");
                        // One finding per distinct failure (hangs: one per seed).
                        let k = if outcome == Outcome::Hang { format!("Hang:{name}") } else { key.clone() };
                        if f.iter().any(|(o, w, s)| (if *o == Outcome::Hang { format!("Hang:{s}") } else { format!("{o:?}:{w}") }) == k) {
                            continue;
                        }
                    }
                    // A hang under full load may just be a slow machine: confirm it alone, with
                    // three times the time, before recording it. Hangs are not minimized (each
                    // probe costs a full timeout).
                    let small = if outcome == Outcome::Hang {
                        let _guard = solo.lock().expect("lock");
                        if run_one(&exe, &file, timeout * 3).0 != Outcome::Hang {
                            continue;
                        }
                        data.clone()
                    } else {
                        minimize(&exe, &work, data.clone(), outcome, &what, timeout)
                    };
                    let id = format!("{:?}-{:016x}", outcome, fxhash(&small)).to_lowercase();
                    let _ = std::fs::write(findings.join(format!("{id}.pdf")), &small);
                    let note = serde_json::json!({ "outcome": format!("{outcome:?}"), "detail": what, "seed_file": name, "bytes": small.len(), "original_bytes": data.len() });
                    let _ = std::fs::write(findings.join(format!("{id}.json")), serde_json::to_string_pretty(&note).unwrap_or_default());
                    println!("  {outcome:?} from {name}: {what} → fuzz-out/findings/{id}.pdf ({} bytes)", small.len());
                    found.lock().expect("lock").push((outcome, what, name.clone()));
                }
            });
        }
    });
    let n = done.load(Ordering::Relaxed).min(iterations);
    let found = found.lock().expect("lock");
    let crashes = found.iter().filter(|f| f.0 == Outcome::Crash).count();
    let hangs = found.len() - crashes;
    println!(
        "fuzz: {n} runs in {:.0} s ({:.1}/s): {crashes} distinct crash(es), {hangs} hang(s)",
        start.elapsed().as_secs_f64(),
        n as f64 / start.elapsed().as_secs_f64().max(0.001)
    );
    if crashes > 0 {
        bail!("{crashes} crash(es): see fuzz-out/findings/");
    }
    Ok(())
}

fn fxhash(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ u64::from(*x)).wrapping_mul(0x100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offset after the last `startxref`, if the file has a well-formed one.
    fn startxref(pdf: &[u8]) -> Option<usize> {
        let i = pdf.windows(10).rposition(|w| w == b"startxref\n")?;
        String::from_utf8_lossy(pdf.get(i + 10..)?).lines().next()?.trim().parse().ok()
    }

    /// A seed whose offsets are wrong would only exercise the reconstruction path, which hides the
    /// readers the seed is there to reach.
    #[test]
    fn every_synthetic_seed_points_startxref_at_its_cross_reference() {
        let seeds = synthetic_seeds();
        assert_eq!(seeds.len(), 6, "update this count and the doc comment together with the seeds");
        for (n, pdf) in seeds.iter().enumerate() {
            assert!(pdf.starts_with(b"%PDF-"), "seed {n}: header");
            assert!(pdf.ends_with(b"%%EOF\n"), "seed {n}: trailer");
            let at = startxref(pdf).unwrap_or_else(|| panic!("seed {n}: no startxref"));
            let target = pdf.get(at..).unwrap_or_default();
            let table = target.starts_with(b"xref\n");
            let xref_stream = find(target.get(..40).unwrap_or(target), b"0 obj").is_some() && find(target, b"/Type /XRef").is_some();
            assert!(table || xref_stream, "seed {n}: startxref {at} does not reach a cross-reference");
        }
    }

    /// The incremental-update seed must chain to the first section, or it is not an update.
    #[test]
    fn the_incremental_seed_chains_to_the_first_cross_reference() {
        let pdf = incremental_update_seed();
        let at = startxref(&pdf).unwrap_or(0);
        let tail = String::from_utf8_lossy(pdf.get(at..).unwrap_or_default()).into_owned();
        let prev: usize = tail.split("/Prev ").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok()).unwrap_or(0);
        assert!(prev > 0 && prev < at, "/Prev {prev} must point at the earlier section (startxref {at})");
        assert!(pdf.get(prev..).unwrap_or_default().starts_with(b"xref\n"));
    }

    /// Every cross-reference entry of the classic seeds points at its own object.
    #[test]
    fn classic_seed_entries_point_at_their_objects() {
        for (n, pdf) in [synthetic_seeds().remove(0), feature_rich_seed(), cyclic_seed()].iter().enumerate() {
            let at = startxref(pdf).unwrap_or(0);
            let text = String::from_utf8_lossy(pdf.get(at..).unwrap_or_default()).into_owned();
            for (num, line) in text.lines().skip(2).take_while(|l| !l.starts_with("trailer")).enumerate() {
                if line.ends_with(" n ") {
                    let off: usize = line.get(..10).and_then(|o| o.parse().ok()).unwrap_or(usize::MAX);
                    let want = format!("{num} 0 obj");
                    assert!(pdf.get(off..).unwrap_or_default().starts_with(want.as_bytes()), "seed {n}: entry {num} does not reach `{want}`");
                }
            }
        }
    }
}
