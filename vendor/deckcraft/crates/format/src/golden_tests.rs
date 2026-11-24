//! Byte-for-byte checks on the `.deckcraft` package. The files in `tests/golden` were written by
//! the format code as it was before the package layer moved to `craft-deckcraft-format`
//! (storytold/craft-libs). A save of the same deck must give the same bytes, and a load must give
//! the same presentation and the same damage report.
//! `DECKCRAFT_BLESS_GOLDEN=1` rewrites them. Only do that for a deliberate format change.

use std::path::PathBuf;
use std::sync::Arc;

use deckcraft_model::chart::{Chart, ChartPart, ChartType};
use deckcraft_model::text::TextBody;
use deckcraft_model::{MediaId, MediaItem, Presentation, Shape, ShapeId, ShapeKind, Xfrm, defaults};

use super::{load_checked, save_as};

/// A fixed generator, so the golden bytes don't change with the app's version.
const GENERATOR: &str = "DeckCraft golden";

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name)
}

fn media(id: u32, name: &str, content_type: &str, data: &[u8], link: Option<&str>) -> MediaItem {
    MediaItem { id: MediaId(id), name: name.into(), content_type: content_type.into(), data: Arc::new(data.to_vec()), link: link.map(Into::into) }
}

/// A deck that reaches every kind of package member: stored and deflated media, linked media
/// without bytes (not written), stored and deflated chart parts, and a non-finite number.
fn deck() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let slide = Arc::make_mut(&mut p.slides[0]);
    if let Some(title) = slide.shapes.first_mut() {
        title.text = Some(TextBody::from_text("Golden «deck» ✓"));
    }
    let mut chart = Chart::sample(ChartType::Bar);
    for (i, (content_type, bytes)) in
        [("application/zip", vec![80, 75, 3, 4, 1, 2, 3]), ("application/vnd.ms-office.chartstyle+xml", b"<cs:chartStyle id=\"201\"/>".repeat(8))]
            .into_iter()
            .enumerate()
    {
        chart.kept.parts.push(ChartPart { name: format!("ppt/charts/part{i}"), content_type: content_type.into(), bytes, native_entry: None });
    }
    let chart_id = ShapeId(p.alloc_id().unwrap_or(9000));
    let odd_id = ShapeId(p.alloc_id().unwrap_or(9001));
    let slide = Arc::make_mut(&mut p.slides[0]);
    slide.shapes.push(Shape {
        id: chart_id,
        xfrm: Some(Xfrm::new(10.0, 20.0, 300.0, 200.0)),
        kind: ShapeKind::Chart(Box::new(chart)),
        ..Default::default()
    });
    // Written as 0 by the NaN-safe writer, then clamped on load.
    slide.shapes.push(Shape { id: odd_id, xfrm: Some(Xfrm::new(f64::NAN, 5.0, f64::INFINITY, 7.5)), ..Default::default() });
    p.media = vec![
        media(1, "image1.png", "image/png", &[0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 0], None),
        media(
            2,
            "drawing.svg",
            "image/svg+xml",
            &b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"4\" height=\"4\"/></svg>".repeat(4),
            None,
        ),
        media(3, "clip.mp4", "video/mp4", &[0, 0, 0, 24, b'f', b't', b'y', b'p'], Some("https://example.invalid/clip.mp4")),
        media(4, "linked.png", "image/png", &[], Some("file:///nowhere/linked.png")),
    ];
    p
}

/// The loaded deck and its damage report, as JSON. Media bytes are not in the model's JSON, so
/// each item's bytes follow it in hex. `with_model: false` leaves the presentation out.
fn loaded_json(bytes: &[u8], with_model: bool) -> String {
    let (p, damaged) = load_checked(bytes).unwrap_or_else(|e| panic!("load: {e}"));
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let media: Vec<(u32, String)> = p.media.iter().map(|m| (m.id.0, hex(&m.data))).collect();
    let mut value = serde_json::json!({ "damaged": damaged, "media": media });
    if with_model {
        value["presentation"] = serde_json::to_value(&p).unwrap_or_default();
    }
    serde_json::to_string(&value).unwrap_or_default() + "\n"
}

fn check(name: &str, actual: &[u8]) {
    let path = golden(name);
    if std::env::var_os("DECKCRAFT_BLESS_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let expected = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(expected == actual, "{name} changed ({} bytes, golden has {})", actual.len(), expected.len());
}

#[test]
fn a_save_writes_the_golden_package_bytes() {
    let bytes = save_as(&deck(), GENERATOR).unwrap_or_else(|e| panic!("save: {e}"));
    check("deck.deckcraft", &bytes);
}

#[test]
fn a_load_of_the_golden_package_gives_the_golden_presentation() {
    let bytes = std::fs::read(golden("deck.deckcraft")).unwrap_or_else(|e| panic!("{e}"));
    check("deck.loaded.json", loaded_json(&bytes, true).as_bytes());
}

#[test]
fn a_load_of_a_damaged_package_gives_the_golden_report() {
    // The PNG member and the second chart part removed: both are reported, nothing is refused.
    let bytes = std::fs::read(golden("deck.deckcraft")).unwrap_or_else(|e| panic!("{e}"));
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).unwrap_or_else(|e| panic!("{e}"));
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let Ok(f) = z.by_index_raw(i) else { continue };
        if f.name() != "media/1" && f.name() != "chart-parts/1" {
            out.raw_copy_file(f).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    let damaged = out.finish().unwrap_or_else(|e| panic!("{e}")).into_inner();
    check("damaged.loaded.json", loaded_json(&damaged, false).as_bytes());
}
