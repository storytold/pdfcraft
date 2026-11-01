//! Writes a synthetic A1 engineering sheet, for timing large vector pages without the proprietary
//! sample from storytold/pdfcraft#623 (item 8). It follows the sample's content layout: 1684 × 2384
//! pt with `/Rotate 90`, one Flate stream of unit-length polyline strokes, each in its own `q … Q`
//! block with `0.12 0 0 0.12 0 0 cm` and integer coordinates. Almost every stroke starts where the
//! previous one ended, and most run vertically, as hatching does. Strokes are 2 units wide. A colour
//! change comes before about 29% of strokes, a twelve-rectangle clip with one fill every 50,000
//! strokes, and 1,143 labels come last, so a cut in the content loses the labels first. The colour
//! space is named DeviceRGB, because the sample's ICC space renders at about the same cost, while
//! CalRGB adds about 0.6 s to every request. The labels use non-embedded Helvetica, so the file
//! carries no profile or font data.
//!
//! Usage: `synth_sheet OUT.pdf [STROKES]`. The default is 4,400,000 strokes (about 277 MB decoded).

use std::error::Error;
use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;

const DEFAULT_STROKES: u64 = 4_400_000;
/// The sheet spans this many content units (0.12 pt each).
const SPAN: u64 = 14_000;
const CLIP_EVERY: u64 = 50_000;
const LABELS: u64 = 1_143;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// xorshift64, so every run writes the same sheet.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x % n
    }
}

/// The content stream, compressed as it is written.
struct Content {
    encoder: ZlibEncoder<Vec<u8>>,
    decoded: u64,
}

impl Content {
    fn push(&mut self, text: &str) -> std::io::Result<()> {
        self.encoder.write_all(text.as_bytes())?;
        self.decoded += text.len() as u64;
        Ok(())
    }
}

/// A colour component in millionths, written with six decimals.
fn micro(v: i64) -> String {
    format!("{}.{:06}", v / 1_000_000, v % 1_000_000)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).ok_or("usage: synth_sheet OUT.pdf [STROKES]")?;
    let strokes: u64 = match args.get(2) {
        Some(s) => s.parse()?,
        None => DEFAULT_STROKES,
    };

    let mut content = Content { encoder: ZlibEncoder::new(Vec::new(), Compression::default()), decoded: 0 };
    content.push("q\n0 1 -1 0 1684 0 cm\n1 J\n2 w\n")?;
    let mut rng = Rng(SEED);
    let mut colour = [49_509_i64, 49_175, 47_727];
    let (mut x, mut y) = (rng.below(SPAN), rng.below(SPAN));
    let (mut clips, mut colour_changes) = (0_u64, 0_u64);
    for i in 0..strokes {
        if i % CLIP_EVERY == 0 {
            // A clip of twelve small rectangles with one fill, as the sample's clips are. The clip
            // wraps no strokes.
            content.push("q\n")?;
            for _ in 0..12 {
                let (rx, ry) = (rng.below(SPAN), rng.below(SPAN));
                content.push(&format!("{} {} m\n{} {} l\n{} {} l\n{} {} l\nh\n", rx, ry, rx + 14, ry, rx + 14, ry + 30, rx, ry + 30))?;
            }
            content.push("W* n\n/Cs1 cs 0.930000 0.940000 0.960000 sc\n")?;
            let (rx, ry) = (rng.below(SPAN), rng.below(SPAN));
            content.push(&format!("{rx} {ry} 14 30 re f\nQ\n"))?;
            clips += 1;
        }
        if rng.below(1000) < 293 {
            for c in &mut colour {
                *c = (*c + rng.below(4_001) as i64 - 2_000).clamp(0, 1_000_000);
            }
            content.push(&format!("/Cs1 CS {} {} {} SC\n", micro(colour[0]), micro(colour[1]), micro(colour[2])))?;
            colour_changes += 1;
        }
        if rng.below(1000) < 11 {
            x = rng.below(SPAN);
            y = rng.below(SPAN);
        }
        let (dx, dy) = match rng.below(1000) {
            0..=693 => (0, 1),
            694..=845 => (1, 1),
            846..=992 => (1, 0),
            _ => (0, 0),
        };
        let (nx, ny) = (x + dx, y + dy);
        content.push(&format!("q\n0.12 0 0 0.12 0 0 cm\n{x} {y} m\n{nx} {ny} l\nS\nQ\n"))?;
        (x, y) = (nx, ny);
    }
    for k in 0..LABELS {
        let text = if k + 1 == LABELS { "END OF SHEET".to_string() } else { format!("CBL -{}", 100 + k) };
        let (tx, ty) = (1210 + (k % 30) * 40, 497 + (k / 30) * 12);
        content.push(&format!("q\n1 0 0 1 0 0 cm\n0 g\nBT\n/F3 1 Tf\n4 0 0 4 {tx} {ty} Tm\n({text}) Tj\nET\nQ\n"))?;
    }
    content.push("Q\n")?;

    let Content { encoder, decoded } = content;
    let stream = encoder.finish()?;
    let mut stream_object = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", stream.len()).into_bytes();
    stream_object.extend_from_slice(&stream);
    stream_object.extend_from_slice(b"\nendstream");

    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1684 2384] /Rotate 90 /Contents 4 0 R /Resources << /ColorSpace << /Cs1 /DeviceRGB >> /Font << /F3 5 0 R >> >> >>"
            .to_vec(),
        stream_object,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (n, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", n + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = pdf.len();
    let size = objects.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    std::fs::write(out, &pdf)?;

    println!(
        "wrote {out}: {strokes} strokes, {colour_changes} colour changes, {clips} clip groups, {LABELS} labels, {decoded} decoded bytes, {} stream bytes ({:.2}:1)",
        stream.len(),
        decoded as f64 / stream.len() as f64
    );
    Ok(())
}
