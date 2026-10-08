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

/// Synthetic seeds, so fuzzing works without the corpus: plain, object-stream and encrypted
/// shapes, nested page trees, annotations, forms and inline images.
fn synthetic_seeds() -> Vec<Vec<u8>> {
    let build = |objs: &[&str], trailer: &str| {
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
    };
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
