//! Native Tesseract adapter. No shell, network, or document-controlled executable/config paths.
//! TSV word boxes are documented at https://tesseract-ocr.github.io/tessdoc/Command-Line-Usage.html.
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{Line, OcrError, Word};

const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
const SETUP: &str = "Hungarian OCR needs Tesseract 5 and the hun language model. Install tesseract-ocr and tesseract-ocr-hun (Ubuntu/Debian), then restart PdfCraft. For a custom installation set PDFCRAFT_TESSERACT to the executable and TESSDATA_PREFIX to its tessdata folder.";

pub(super) struct Tesseract {
    executable: OsString,
}

fn error(e: impl std::fmt::Display) -> OcrError {
    OcrError::Recognize(e.to_string())
}

impl Tesseract {
    pub(super) fn find() -> Result<Self, OcrError> {
        let t = Self { executable: std::env::var_os("PDFCRAFT_TESSERACT").unwrap_or_else(|| "tesseract".into()) };
        let mut cmd = Command::new(&t.executable);
        cmd.arg("--list-langs").stdin(Stdio::null());
        let languages = run(cmd, Duration::from_secs(10)).map_err(|e| error(format!("{SETUP}\n{e}")))?;
        if !languages.lines().any(|l| l.trim() == "hun") {
            return Err(error(SETUP));
        }
        Ok(t)
    }

    pub(super) fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        let count = crate::image_pixels(pixels, width, height)?;
        // A private, automatically removed file avoids pipe deadlocks and works on Windows too.
        let mut input = tempfile::tempfile().map_err(error)?;
        let channels = pixels.len() / count;
        write!(input, "P6\n{width} {height}\n255\n").map_err(error)?;
        {
            let mut out = std::io::BufWriter::new(&mut input);
            for p in pixels.chunks_exact(channels) {
                let rgb = match p {
                    [g] => [*g; 3],
                    [r, g, b] | [r, g, b, _] => [*r, *g, *b],
                    _ => return Err(error("invalid OCR image channels")),
                };
                out.write_all(&rgb).map_err(error)?;
            }
            out.flush().map_err(error)?;
        }
        input.seek(SeekFrom::Start(0)).map_err(error)?;
        let mut cmd = Command::new(&self.executable);
        cmd.args(["stdin", "stdout", "-l", "hun", "--oem", "1", "--psm", "3", "-c", "tessedit_create_tsv=1"])
            .env("OMP_THREAD_LIMIT", "2")
            .stdin(Stdio::from(input));
        let output = run(cmd, Duration::from_secs(120))?;
        parse_tsv(&output, width, height)
    }
}

/// File-backed output is capped while the process runs. Always reap the child, including on
/// timeout/I/O failure; neither an unread pipe nor a hung executable can strand the OCR worker.
fn run(mut cmd: Command, timeout: Duration) -> Result<String, OcrError> {
    let mut stdout = tempfile::tempfile().map_err(error)?;
    let mut stderr = tempfile::tempfile().map_err(error)?;
    cmd.stdout(Stdio::from(stdout.try_clone().map_err(error)?)).stderr(Stdio::from(stderr.try_clone().map_err(error)?));
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW: no console flashes from a GUI run.
    }
    let mut child = cmd.spawn().map_err(error)?;
    let start = Instant::now();
    let result = (|| {
        loop {
            if stdout.metadata().map_err(error)?.len() > MAX_OUTPUT || stderr.metadata().map_err(error)?.len() > MAX_OUTPUT {
                return Err(error("Tesseract output exceeded 16 MiB"));
            }
            if let Some(status) = child.try_wait().map_err(error)? {
                return Ok(status);
            }
            if start.elapsed() >= timeout {
                return Err(error("Tesseract timed out; try fewer pages or a lower OCR resolution"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })();
    let status = match result {
        Ok(status) => status,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    if !status.success() {
        return Err(error(format!("Tesseract failed ({status}): {}", read_output(&mut stderr)?.trim())));
    }
    read_output(&mut stdout)
}

fn read_output(file: &mut File) -> Result<String, OcrError> {
    file.seek(SeekFrom::Start(0)).map_err(error)?;
    let mut bytes = Vec::new();
    file.take(MAX_OUTPUT + 1).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err(error("Tesseract output exceeded 16 MiB"));
    }
    String::from_utf8(bytes).map_err(error)
}

fn parse_tsv(tsv: &str, width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
    let mut rows = tsv.lines();
    if rows.next() != Some("level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext") {
        return Err(error("Tesseract returned an invalid TSV header"));
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut previous = None;
    for row in rows {
        let cols: Vec<_> = row.splitn(12, '\t').collect();
        if cols.first().copied() != Some("5") {
            continue;
        }
        let field = |i: usize| cols.get(i).copied().ok_or_else(|| error("incomplete Tesseract word row"));
        let n = |i| field(i)?.parse::<u32>().map_err(error);
        let key = [n(1)?, n(2)?, n(3)?, n(4)?];
        let (left, top, w, h) = (n(6)?, n(7)?, n(8)?, n(9)?);
        let right = left.checked_add(w).ok_or_else(|| error("invalid Tesseract word bounds"))?;
        let bottom = top.checked_add(h).ok_or_else(|| error("invalid Tesseract word bounds"))?;
        if right > width || bottom > height || w == 0 || h == 0 {
            return Err(error("Tesseract word bounds fall outside the image"));
        }
        let text = field(11)?.trim();
        if text.is_empty() {
            continue;
        }
        if previous != Some(key) {
            lines.push(Line::default());
            previous = Some(key);
        }
        if let Some(line) = lines.last_mut() {
            line.words.push(Word { text: text.into(), rect: [left as f32, top as f32, right as f32, bottom as f32] });
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n";

    #[test]
    fn parses_hungarian_words_and_line_boxes() {
        let tsv = format!(
            "{HEADER}5\t1\t1\t1\t1\t1\t10\t20\t80\t15\t95\tÁrvíztűrő\n5\t1\t1\t1\t1\t2\t100\t20\t80\t15\t94\ttükörfúrógép\n5\t1\t1\t1\t2\t1\t10\t50\t40\t15\t90\tŐŰőű\n"
        );
        let lines = parse_tsv(&tsv, 200, 100).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text(), "Árvíztűrő tükörfúrógép");
        assert_eq!(lines[1].words[0].rect, [10.0, 50.0, 50.0, 65.0]);
    }

    #[test]
    fn refuses_malformed_or_overflowing_output() {
        for row in ["5", "5\t1\t1\t1\t1\t1\t4294967295\t0\t10\t10\t99\tx", "5\t1\t1\t1\t1\t1\t0\t0\t201\t10\t99\tx"] {
            assert!(parse_tsv(&format!("{HEADER}{row}"), 200, 100).is_err());
        }
        assert!(parse_tsv("not TSV", 200, 100).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn failed_and_stalled_processes_return_errors() {
        let mut failed = Command::new("sh");
        failed.args(["-c", "echo missing-model >&2; exit 1"]);
        assert!(run(failed, Duration::from_secs(1)).unwrap_err().to_string().contains("missing-model"));
        let mut stalled = Command::new("sh");
        stalled.args(["-c", "exec sleep 5"]);
        assert!(run(stalled, Duration::from_millis(50)).unwrap_err().to_string().contains("timed out"));
    }

    #[test]
    fn refuses_invalid_image_buffers_before_starting_process() {
        let t = Tesseract { executable: "does-not-exist".into() };
        for (w, h, px) in [(u32::MAX, u32::MAX, vec![0]), (10, 10, vec![0; 99])] {
            let e = t.recognize(&px, w, h).unwrap_err().to_string();
            assert!(e.contains("OCR image"), "{e}");
        }
    }
}
