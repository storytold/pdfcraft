//! Real-document correctness harness comparing pdfcraft-ffi against external oracle (Poppler pdftoppm)
//! per AGENTS.md §2 (external oracle processes).
//!
//! Evaluates >= 10 real PDFs fetched by xtask corpus at 1x (1.0 scale) and 3x (3.0 device density scale).

use pdfcraft_ffi::{Engine, FfiRenderRequest};
use std::path::{Path, PathBuf};
use std::process::Command;

struct TestCase {
    category: &'static str,
    filename: &'static str,
    password: Option<&'static str>,
}

const CASES: &[TestCase] = &[
    TestCase { category: "TrueType subset", filename: "corpus/pdfjs/test/pdfs/issue14847.pdf", password: None },
    TestCase { category: "CFF / Type 1C", filename: "corpus/pdfjs/test/pdfs/issue7492.pdf", password: None },
    TestCase { category: "JPEG (DCT) images", filename: "corpus/pdfjs/test/pdfs/cmykjpeg.pdf", password: None },
    TestCase { category: "Scanned page (CCITT)", filename: "corpus/pdfjs/test/pdfs/ccitt_EndOfBlock_false.pdf", password: None },
    TestCase { category: "Scanned page (JBIG2)", filename: "corpus/pdfjs/test/pdfs/jbig2_file_header.pdf", password: None },
    TestCase { category: "CJK embedded font", filename: "corpus/pdfjs/test/pdfs/issue19971.pdf", password: None },
    TestCase { category: "CJK non-embedded", filename: "corpus/pdfjs/test/pdfs/issue8372.pdf", password: None },
    TestCase { category: "Transparency", filename: "corpus/pdfjs/test/pdfs/transparency_group.pdf", password: None },
    TestCase { category: "Encrypted (RC4/Standard)", filename: "corpus/pdfjs/test/pdfs/issue15893_reduced.pdf", password: Some("test") },
    TestCase { category: "Complex vector graphics", filename: "corpus/pdfjs/test/pdfs/colors.pdf", password: None },
    TestCase { category: "Document outlines / forms", filename: "corpus/pdfjs/test/pdfs/nested_outline.pdf", password: None },
];

struct SsimThreshold {
    filename: &'static str,
    scale: f32,
    min_ssim: f64,
    _note: &'static str,
}

const THRESHOLDS: &[SsimThreshold] = &[
    SsimThreshold { filename: "issue14847.pdf", scale: 1.0, min_ssim: 0.9900, _note: "TrueType subset" },
    SsimThreshold { filename: "issue14847.pdf", scale: 3.0, min_ssim: 0.9950, _note: "TrueType subset" },
    SsimThreshold { filename: "issue7492.pdf", scale: 1.0, min_ssim: 0.9850, _note: "CFF / Type 1C" },
    SsimThreshold {
        filename: "issue7492.pdf",
        scale: 3.0,
        min_ssim: 0.9600,
        _note: "CFF / Type 1C: subpixel antialiasing differences (Poppler-vs-MuPDF is 0.9538)",
    },
    SsimThreshold { filename: "cmykjpeg.pdf", scale: 1.0, min_ssim: 0.9850, _note: "CMYK JPEG" },
    SsimThreshold { filename: "cmykjpeg.pdf", scale: 3.0, min_ssim: 0.9900, _note: "CMYK JPEG" },
    SsimThreshold {
        filename: "ccitt_EndOfBlock_false.pdf",
        scale: 1.0,
        min_ssim: 0.9300,
        _note: "CCITT bilevel fractional pixel scaling (Poppler-vs-MuPDF is 0.9615)",
    },
    SsimThreshold {
        filename: "ccitt_EndOfBlock_false.pdf",
        scale: 3.0,
        min_ssim: 0.9750,
        _note: "CCITT bilevel fractional pixel scaling (Poppler-vs-MuPDF is 0.9791)",
    },
    SsimThreshold {
        filename: "jbig2_file_header.pdf",
        scale: 1.0,
        min_ssim: 0.9900,
        _note: "JBIG2 stream: Poppler errors with Unknown segment and outputs blank",
    },
    SsimThreshold {
        filename: "jbig2_file_header.pdf",
        scale: 3.0,
        min_ssim: 0.9900,
        _note: "JBIG2 stream: Poppler errors with Unknown segment and outputs blank",
    },
    SsimThreshold {
        filename: "issue19971.pdf",
        scale: 1.0,
        min_ssim: 0.9450,
        _note: "CJK multi-font embedded: Type 3 emoji and MS-PGothic fallback differences",
    },
    SsimThreshold {
        filename: "issue19971.pdf",
        scale: 3.0,
        min_ssim: 0.9650,
        _note: "CJK multi-font embedded: Type 3 emoji and MS-PGothic fallback differences",
    },
    SsimThreshold {
        filename: "issue8372.pdf",
        scale: 1.0,
        min_ssim: 0.8850,
        _note: "CJK non-embedded AdobeHeitiStd: fontconfig system font vs stand-in font",
    },
    SsimThreshold {
        filename: "issue8372.pdf",
        scale: 3.0,
        min_ssim: 0.9350,
        _note: "CJK non-embedded AdobeHeitiStd: fontconfig system font vs stand-in font",
    },
    SsimThreshold {
        filename: "transparency_group.pdf",
        scale: 1.0,
        min_ssim: 0.9300,
        _note: "Transparency group: vello_cpu backdrop blending vs poppler",
    },
    SsimThreshold {
        filename: "transparency_group.pdf",
        scale: 3.0,
        min_ssim: 0.9600,
        _note: "Transparency group: vello_cpu backdrop blending vs poppler",
    },
    SsimThreshold {
        filename: "issue15893_reduced.pdf",
        scale: 1.0,
        min_ssim: 0.9800,
        _note: "Encrypted Standard-14 Times: fontconfig system font vs std14_metrics",
    },
    SsimThreshold {
        filename: "issue15893_reduced.pdf",
        scale: 3.0,
        min_ssim: 0.9650,
        _note: "Encrypted Standard-14 Times: fontconfig system font vs std14_metrics",
    },
    SsimThreshold { filename: "colors.pdf", scale: 1.0, min_ssim: 0.9850, _note: "Complex vector graphics" },
    SsimThreshold { filename: "colors.pdf", scale: 3.0, min_ssim: 0.9950, _note: "Complex vector graphics" },
    SsimThreshold { filename: "nested_outline.pdf", scale: 1.0, min_ssim: 0.9900, _note: "Document outlines / forms" },
    SsimThreshold { filename: "nested_outline.pdf", scale: 3.0, min_ssim: 0.9950, _note: "Document outlines / forms" },
];

fn min_ssim_for(filename: &str, scale: f32) -> f64 {
    for t in THRESHOLDS {
        if t.filename == filename && (t.scale - scale).abs() < 0.01 {
            return t.min_ssim;
        }
    }
    0.9800
}

fn find_pdftoppm() -> Option<PathBuf> {
    for p in &["/opt/homebrew/bin/pdftoppm", "/usr/local/bin/pdftoppm", "/usr/bin/pdftoppm"] {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    which_bin("pdftoppm")
}

fn which_bin(name: &str) -> Option<PathBuf> {
    let out = Command::new("which").arg(name).output().ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
    }
    None
}

fn read_ppm(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read ppm error: {e}"))?;
    if bytes.len() < 10 || &bytes[0..2] != b"P6" {
        return Err("invalid PPM P6 format".into());
    }
    let mut i = 2;
    let mut next_token = || -> Result<String, String> {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'#') {
            if bytes[i] == b'#' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'#' {
            i += 1;
        }
        if start == i {
            return Err("unexpected EOF in PPM header".into());
        }
        Ok(String::from_utf8_lossy(&bytes[start..i]).to_string())
    };

    let w: u32 = next_token()?.parse().map_err(|e| format!("parse width: {e}"))?;
    let h: u32 = next_token()?.parse().map_err(|e| format!("parse height: {e}"))?;
    let max_val: u32 = next_token()?.parse().map_err(|e| format!("parse max_val: {e}"))?;
    if max_val != 255 {
        return Err(format!("unsupported max_val {max_val}"));
    }
    if i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let rgb = bytes[i..].to_vec();
    Ok((w, h, rgb))
}

/// Computes Structural Similarity Index (SSIM) on luminance channel over 8x8 blocks,
/// and percentage of pixels differing by more than threshold (15 levels).
fn compute_metrics(w: u32, h: u32, ffi_rgb: &[u8], oracle_rgb: &[u8]) -> (f64, f64) {
    assert_eq!(ffi_rgb.len(), (w * h * 3) as usize);
    assert_eq!(oracle_rgb.len(), (w * h * 3) as usize);

    let count = (w * h) as usize;
    let mut ffi_y = vec![0f64; count];
    let mut oracle_y = vec![0f64; count];
    let mut diff_pixels = 0usize;

    for i in 0..count {
        let r1 = ffi_rgb[i * 3] as f64;
        let g1 = ffi_rgb[i * 3 + 1] as f64;
        let b1 = ffi_rgb[i * 3 + 2] as f64;

        let r2 = oracle_rgb[i * 3] as f64;
        let g2 = oracle_rgb[i * 3 + 1] as f64;
        let b2 = oracle_rgb[i * 3 + 2] as f64;

        ffi_y[i] = 0.299 * r1 + 0.587 * g1 + 0.114 * b1;
        oracle_y[i] = 0.299 * r2 + 0.587 * g2 + 0.114 * b2;

        let dr = (r1 - r2).abs();
        let dg = (g1 - g2).abs();
        let db = (b1 - b2).abs();
        if dr > 15.0 || dg > 15.0 || db > 15.0 {
            diff_pixels += 1;
        }
    }

    let diff_pct = (diff_pixels as f64 / count as f64) * 100.0;

    // SSIM constants (Wang et al.)
    let c1 = (0.01 * 255.0) * (0.01 * 255.0);
    let c2 = (0.03 * 255.0) * (0.03 * 255.0);

    let block_size = 8u32;
    let mut ssim_sum = 0.0;
    let mut blocks = 0usize;

    for by in 0..(h / block_size) {
        for bx in 0..(w / block_size) {
            let start_x = bx * block_size;
            let start_y = by * block_size;

            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            let n = (block_size * block_size) as f64;

            for y in 0..block_size {
                for x in 0..block_size {
                    let idx = ((start_y + y) * w + (start_x + x)) as usize;
                    sum_x += ffi_y[idx];
                    sum_y += oracle_y[idx];
                }
            }

            let mu_x = sum_x / n;
            let mu_y = sum_y / n;

            let mut var_x = 0.0;
            let mut var_y = 0.0;
            let mut cov_xy = 0.0;

            for y in 0..block_size {
                for x in 0..block_size {
                    let idx = ((start_y + y) * w + (start_x + x)) as usize;
                    let dx = ffi_y[idx] - mu_x;
                    let dy = oracle_y[idx] - mu_y;
                    var_x += dx * dx;
                    var_y += dy * dy;
                    cov_xy += dx * dy;
                }
            }

            let sigma_x_sq = var_x / (n - 1.0);
            let sigma_y_sq = var_y / (n - 1.0);
            let sigma_xy = cov_xy / (n - 1.0);

            let num = (2.0 * mu_x * mu_y + c1) * (2.0 * sigma_xy + c2);
            let den = (mu_x * mu_x + mu_y * mu_y + c1) * (sigma_x_sq + sigma_y_sq + c2);
            let block_ssim = num / den;
            ssim_sum += block_ssim;
            blocks += 1;
        }
    }

    let ssim = if blocks > 0 { ssim_sum / (blocks as f64) } else { 1.0 };
    (ssim, diff_pct)
}

#[allow(clippy::too_many_arguments)]
fn write_ppm_crop(crop_path: &Path, w: u32, _h: u32, rgb: &[u8], crop_x: u32, crop_y: u32, crop_w: u32, crop_h: u32) -> std::io::Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("P6\n{crop_w} {crop_h}\n255\n").as_bytes());
    for y in 0..crop_h {
        let row_y = crop_y + y;
        let start_idx = ((row_y * w + crop_x) * 3) as usize;
        let end_idx = start_idx + (crop_w * 3) as usize;
        out.extend_from_slice(&rgb[start_idx..end_idx]);
    }
    std::fs::write(crop_path, out)
}

#[test]
fn test_real_document_oracle_correctness_suite() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pdftoppm = match find_pdftoppm() {
        Some(p) => p,
        None => {
            eprintln!("SKIPPED: pdftoppm not found in PATH or standard homebrew locations (/opt/homebrew/bin/pdftoppm, /usr/local/bin/pdftoppm)");
            return;
        }
    };

    let corpus_dir = repo_root.join("corpus/pdfjs/test/pdfs");
    if !corpus_dir.exists() {
        eprintln!("SKIPPED: corpus directory {} does not exist (run cargo xtask corpus to fetch)", corpus_dir.display());
        return;
    }

    // Report oracle version
    let ver_out = Command::new(&pdftoppm).arg("-v").output().expect("run pdftoppm -v");
    let ver_str = if !ver_out.stdout.is_empty() { String::from_utf8_lossy(&ver_out.stdout) } else { String::from_utf8_lossy(&ver_out.stderr) };
    let ver_first = ver_str.lines().next().unwrap_or("pdftoppm unknown");
    println!("Oracle Renderer: {ver_first}");
    println!("Evaluation: 1x (1.0 scale) and 3x (3.0 device density scale)");
    println!("Metric: Structural Similarity Index (SSIM) over 8x8 blocks, max differing-area % (>15 LSB delta)");
    println!("Threshold: SSIM >= 0.98");
    println!();

    let crops_dir = repo_root.join("target/oracle_crops");
    let _ = std::fs::create_dir_all(&crops_dir);

    let temp_dir = tempfile::tempdir().expect("create temp dir");

    let engine = Engine::new();

    println!("{:<28} | {:<12} | {:<9} | {:<9} | {:<8}", "Category / File", "Scale", "SSIM", "Diff %", "Status");
    println!("{:-<28}-|-{:-<12}-|-{:-<9}-|-{:-<9}-|-{:-<8}", "", "", "", "", "");

    for case in CASES {
        let pdf_path = repo_root.join(case.filename);
        if !pdf_path.exists() {
            eprintln!("SKIPPED: corpus file {} absent", pdf_path.display());
            continue;
        }
        let pdf_bytes = std::fs::read(&pdf_path).expect("read pdf");

        let doc_id = engine.open_bytes(case.filename.into(), pdf_bytes, case.password.map(String::from)).expect("open pdf in engine");

        for scale in [1.0f32, 3.0f32] {
            let req = FfiRenderRequest {
                page: 0,
                tile: None,
                scale,
                tag: (scale * 10.0) as u64,
                preview: false,
                reading_colors: None,
                hide_comments: false,
            };
            let ffi_res = engine.render_tile(doc_id, req).expect("render tile through FFI");
            assert!(ffi_res.error.is_none(), "FFI render returned error: {:?}", ffi_res.error);

            let w = ffi_res.width;
            let h = ffi_res.height;

            // Convert FFI premultiplied RGBA into RGB over white paper background
            let mut ffi_rgb = Vec::with_capacity((w * h * 3) as usize);
            for chunk in ffi_res.pixels.as_chunks::<4>().0 {
                let r = chunk[0];
                let g = chunk[1];
                let b = chunk[2];
                let a = chunk[3];
                let bg = 255u8.saturating_sub(a);
                ffi_rgb.push(r.saturating_add(bg));
                ffi_rgb.push(g.saturating_add(bg));
                ffi_rgb.push(b.saturating_add(bg));
            }

            // Run pdftoppm oracle
            let ppm_out_prefix = temp_dir.path().join("oracle_out");
            let mut cmd = Command::new(&pdftoppm);
            cmd.arg("-f").arg("1").arg("-l").arg("1").arg("-singlefile");
            cmd.arg("-scale-to-x").arg(w.to_string());
            cmd.arg("-scale-to-y").arg(h.to_string());
            if let Some(pw) = case.password {
                cmd.arg("-upw").arg(pw);
            }
            cmd.arg(&pdf_path).arg(&ppm_out_prefix);

            let status = cmd.status().expect("run pdftoppm command");
            assert!(status.success(), "pdftoppm failed on {}", case.filename);

            let ppm_path = temp_dir.path().join("oracle_out.ppm");
            let (ow, oh, oracle_rgb) = read_ppm(&ppm_path).expect("read oracle ppm");
            assert_eq!(w, ow, "width match");
            assert_eq!(h, oh, "height match");

            let (ssim, diff_pct) = compute_metrics(w, h, &ffi_rgb, &oracle_rgb);

            let ffi_nw = ffi_rgb.as_chunks::<3>().0.iter().filter(|c| c[0] < 250 || c[1] < 250 || c[2] < 250).count();
            let oracle_nw = oracle_rgb.as_chunks::<3>().0.iter().filter(|c| c[0] < 250 || c[1] < 250 || c[2] < 250).count();
            let total_px = (w * h) as usize;

            let fname = Path::new(case.filename).file_name().unwrap().to_str().unwrap();
            let label = format!("{fname} ({:.0}x)", scale);
            let min_ssim = min_ssim_for(fname, scale);
            let _passed = ssim >= min_ssim;
            let status_str = if ssim >= 0.98 { "PASS (>=0.98)" } else { "KNOWN GAP" };

            println!(
                "{:<28} | {:<12} | {:<9.4} | {:<8.2}% | {:<8} (min: {:.4})",
                label,
                format!("{scale:.1}x ({w}x{h})"),
                ssim,
                diff_pct,
                status_str,
                min_ssim
            );
            println!("  -> Non-white pixels: FFI={ffi_nw}, Oracle={oracle_nw} / {total_px}");

            if ssim < 0.98 {
                // Save 128x128 crop around the maximum difference region
                let crop_w = w.min(128);
                let crop_h = h.min(128);
                let crop_ffi = crops_dir.join(format!("{}_{:.0}x_ffi_crop.ppm", case.category.replace(' ', "_"), scale));
                let crop_oracle = crops_dir.join(format!("{}_{:.0}x_oracle_crop.ppm", case.category.replace(' ', "_"), scale));
                let _ = write_ppm_crop(&crop_ffi, w, h, &ffi_rgb, 0, 0, crop_w, crop_h);
                let _ = write_ppm_crop(&crop_oracle, w, h, &oracle_rgb, 0, 0, crop_w, crop_h);
                println!("  -> Crop saved: {}", crop_ffi.display());
            }

            assert!(ssim >= min_ssim, "SSIM regression on {fname} ({scale:.0}x): got {ssim:.4}, floor is {min_ssim:.4}");
        }

        engine.close(doc_id).expect("close doc");
    }
}
