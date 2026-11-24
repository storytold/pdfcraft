//! Owned corpus: decks this repository authors, then open → save → reopen.
//!
//! Each package is built here (nothing from another product). The check compares the text in the
//! package and the content parts. A second save must come out the same as the first.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_model::chart::{Chart, ChartType};
use deckcraft_model::style::PictureFill;
use deckcraft_model::table::Table;
use deckcraft_model::text::{Action, Hyperlink, Paragraph, Run, RunProps, TextBody};
use deckcraft_model::{Comment, Geom, PhType, Presentation, ShapeKind, Slide, defaults};

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

fn shape(p: &mut Presentation, name: &str, x: f64, y: f64, w: f64, h: f64) -> deckcraft_model::Shape {
    deckcraft_model::Shape {
        id: deckcraft_model::ShapeId(p.alloc_id().expect("id")),
        name: name.into(),
        xfrm: Some(deckcraft_geom::Xfrm::new(x, y, w, h)),
        ..Default::default()
    }
}

fn new_slide(p: &mut Presentation, title: &str) -> Slide {
    let layout = defaults::layout_of_kind(p, deckcraft_model::LayoutType::TitleOnly).unwrap_or_default();
    let mut s = defaults::new_slide(p, layout).expect("slide");
    if let Some(t) = s.shapes.first_mut() {
        t.text = Some(TextBody::from_text(title));
    }
    s
}

/// A short talk: title, notes, a link, a comment, a picture, a table and a chart.
fn talk() -> Presentation {
    let mut p = defaults::new_presentation(None);
    p.props.title = "Quarterly review".into();
    p.props.subject = "Owned corpus".into();
    p.props.author = "DeckCraft tests".into();
    let img = p.add_media("mark.png", "image/png", png(4, 4, [20, 90, 160])).expect("media");

    {
        let id = p.slides[0].id;
        let s = p.slide_mut(id).expect("slide");
        s.notes = TextBody::from_text("Say the number slowly");
        if let Some(t) = s.shapes.first_mut() {
            t.text = Some(TextBody {
                paragraphs: vec![Paragraph {
                    runs: vec![
                        Run::new("Quarterly review"),
                        Run::with(
                            "North report",
                            RunProps {
                                link: Some(Hyperlink {
                                    action: Action::Url { url: "https://example.org/north?q=1&x=2".into() },
                                    tooltip: "Open the report".into(),
                                    highlight_click: false,
                                    sound: None,
                                    stop_sound: false,
                                    zoom_return: None,
                                }),
                                ..Default::default()
                            },
                        ),
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        s.comments = vec![Comment {
            author: "Ada".into(),
            initials: "AL".into(),
            text: "Ada checks the axis".into(),
            date: "2026-03-04T05:06:07.000".into(),
            x: 40.0,
            y: 40.0,
            ..Default::default()
        }];
    }

    let mut s = new_slide(&mut p, "Figures");
    let mut pic = shape(&mut p, "Mark", 60.0, 80.0, 80.0, 80.0);
    pic.kind = ShapeKind::Picture { fill: PictureFill { media: img, ..Default::default() } };
    pic.geom = Geom::preset("rect");
    let mut table = Table::new(2, 2, 200.0, 24.0);
    for (r, c, text) in [(0, 0, "Region"), (0, 1, "Amount"), (1, 0, "North"), (1, 1, "12")] {
        if let Some(cell) = table.cell_mut(r, c) {
            cell.text = TextBody::from_text(text);
        }
    }
    let mut ts = shape(&mut p, "Regions", 160.0, 80.0, 200.0, 48.0);
    ts.kind = ShapeKind::Table(table);
    let mut chart = Chart::sample(ChartType::Column);
    chart.title = Some("Revenue".into());
    chart.categories = vec!["Jan".into(), "Feb".into()];
    chart.series.truncate(1);
    if let Some(series) = chart.series.first_mut() {
        series.name = "Actual".into();
        series.values = vec![Some(10.0), Some(20.0)];
    }
    chart.axis_titles = (Some("Month".into()), Some("Amount due".into()));
    let mut ch = shape(&mut p, "Revenue chart", 400.0, 80.0, 280.0, 180.0);
    ch.kind = ShapeKind::Chart(Box::new(chart));
    s.shapes.extend([pic, ts, ch]);
    p.slides.push(Arc::new(s));
    p
}

struct Pkg {
    parts: BTreeMap<String, Vec<u8>>,
}

impl Pkg {
    fn open(bytes: &[u8]) -> Self {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
        let mut parts = BTreeMap::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).expect("entry");
            if f.is_dir() {
                continue;
            }
            let name = f.name().trim_start_matches('/').to_string();
            let mut data = vec![];
            f.read_to_end(&mut data).expect("read");
            parts.insert(name, data);
        }
        Pkg { parts }
    }

    fn replace(&self, name: &str, data: Vec<u8>) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        let mut wrote = false;
        for (n, bytes) in &self.parts {
            let body = if n == name {
                wrote = true;
                data.as_slice()
            } else {
                bytes.as_slice()
            };
            out.start_file(n, opts).expect("start");
            out.write_all(body).expect("write");
        }
        assert!(wrote, "missing part {name}");
        out.finish().expect("finish").into_inner()
    }
}

fn xml_text(parts: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (name, bytes) in parts {
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".xml") || lower.ends_with(".rels")) {
            continue;
        }
        let Ok(xml) = std::str::from_utf8(bytes) else { continue };
        for tag in ["a:t", "c:v", "p:text", "dc:title", "dc:subject", "dc:creator", "dc:description"] {
            for text in element_texts(xml, tag) {
                if !text.is_empty() {
                    *counts.entry(text).or_insert(0) += 1;
                }
            }
        }
        for url in attr_values(xml, "Target") {
            if url.starts_with("http://") || url.starts_with("https://") {
                *counts.entry(url).or_insert(0) += 1;
            }
        }
        for tip in attr_values(xml, "tooltip") {
            if !tip.is_empty() {
                *counts.entry(tip).or_insert(0) += 1;
            }
        }
    }
    counts
}

fn element_texts(xml: &str, tag: &str) -> Vec<String> {
    let mut out = vec![];
    let open = format!("<{tag}");
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let boundary = after.chars().next();
        if !matches!(boundary, Some('>' | '/' | ' ' | '\n' | '\t' | '\r')) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        if after.as_bytes().get(gt.saturating_sub(1)) == Some(&b'/') {
            rest = &after[gt + 1..];
            continue;
        }
        let body = &after[gt + 1..];
        let close = format!("</{tag}>");
        let Some(end) = body.find(&close) else { break };
        out.push(decode_xml(&body[..end]));
        rest = &body[end + close.len()..];
    }
    out
}

fn attr_values(xml: &str, name: &str) -> Vec<String> {
    let mut out = vec![];
    let key = format!("{name}=\"");
    let mut rest = xml;
    while let Some(i) = rest.find(&key) {
        let after = &rest[i + key.len()..];
        let Some(end) = after.find('"') else { break };
        out.push(decode_xml(&after[..end]));
        rest = &after[end + 1..];
    }
    out
}

fn decode_xml(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let Some(end) = after.find(';') else {
            out.push_str(after);
            return out;
        };
        let ent = &after[1..end];
        let ch = match ent {
            "amp" => "&".into(),
            "lt" => "<".into(),
            "gt" => ">".into(),
            "quot" => "\"".into(),
            "apos" => "'".into(),
            hex if hex.starts_with("#x") || hex.starts_with("#X") => {
                u32::from_str_radix(&hex[2..], 16).ok().and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default()
            }
            dec if dec.starts_with('#') => dec[1..].parse::<u32>().ok().and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default(),
            _ => String::new(),
        };
        out.push_str(&ch);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn part_count(parts: &BTreeMap<String, Vec<u8>>, pred: impl Fn(&str) -> bool) -> usize {
    parts.keys().filter(|n| pred(n)).count()
}

fn content_counts(parts: &BTreeMap<String, Vec<u8>>) -> [usize; 6] {
    let is = |prefix: &str, name: &str| {
        let n = name.to_ascii_lowercase();
        n.starts_with(prefix) && n.ends_with(".xml") && !n.contains("_rels")
    };
    [
        part_count(parts, |n| is("ppt/slides/slide", n)),
        part_count(parts, |n| is("ppt/notesslides/notesslide", n)),
        part_count(parts, |n| is("ppt/notesmasters/notesmaster", n)),
        part_count(parts, |n| is("ppt/charts/chart", n)),
        part_count(parts, |n| n.to_ascii_lowercase().starts_with("ppt/media/") && !n.to_ascii_lowercase().contains("_rels")),
        part_count(parts, |n| is("ppt/comments/comment", n)),
    ]
}

fn lost<'a>(origin: &'a BTreeMap<String, usize>, saved: &BTreeMap<String, usize>) -> Vec<(&'a str, usize, usize)> {
    origin
        .iter()
        .filter_map(|(s, n)| {
            let got = saved.get(s).copied().unwrap_or(0);
            (got < *n).then_some((s.as_str(), *n, got))
        })
        .collect()
}

/// Open, save, reopen. Text from the original is still there, content parts are still there, and
/// saving again does not change the package's part names or text.
fn round_trip(label: &str, bytes: &[u8]) -> (Pkg, Pkg) {
    let origin = Pkg::open(bytes);
    let once = deckcraft_pptx::export(&deckcraft_pptx::import(bytes).unwrap_or_else(|e| panic!("{label}: import: {e}")))
        .unwrap_or_else(|e| panic!("{label}: save: {e}"));
    let saved = Pkg::open(&once);
    let origin_text = xml_text(&origin.parts);
    let missing = lost(&origin_text, &xml_text(&saved.parts));
    assert!(missing.is_empty(), "{label}: text lost on save: {missing:?}");
    assert_eq!(content_counts(&origin.parts), content_counts(&saved.parts), "{label}: content parts changed");
    let again = deckcraft_pptx::export(&deckcraft_pptx::import(&once).unwrap_or_else(|e| panic!("{label}: reopen: {e}")))
        .unwrap_or_else(|e| panic!("{label}: second save: {e}"));
    let saved2 = Pkg::open(&again);
    assert_eq!(saved.parts.keys().collect::<Vec<_>>(), saved2.parts.keys().collect::<Vec<_>>(), "{label}: part names changed on the second save");
    assert_eq!(xml_text(&saved.parts), xml_text(&saved2.parts), "{label}: text changed on the second save");
    (origin, saved)
}

fn set_placeholder_text(xml: &str, ph: &str, text: &str) -> String {
    let needle = format!("type=\"{ph}\"");
    let at = xml.find(&needle).unwrap_or_else(|| panic!("notes master has no {ph} placeholder"));
    let after = &xml[at..];
    let start_rel = after.find("<p:txBody>").unwrap_or_else(|| panic!("no txBody after {ph}"));
    let end_rel = after.find("</p:txBody>").unwrap_or_else(|| panic!("no txBody end after {ph}"));
    assert!(start_rel < end_rel, "{ph} txBody is out of order");
    let start = at + start_rel;
    let end = at + end_rel + "</p:txBody>".len();
    let body = format!("<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody>");
    let mut out = String::with_capacity(xml.len() + body.len());
    out.push_str(&xml[..start]);
    out.push_str(&body);
    out.push_str(&xml[end..]);
    out
}

/// A received deck whose notes master carries the header and footer (where PowerPoint stores them).
fn notes_header_deck() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    let id = p.slides[0].id;
    let s = p.slide_mut(id).expect("slide");
    if let Some(t) = s.shapes.first_mut() {
        t.text = Some(TextBody::from_text("Quarterly review"));
    }
    s.notes = TextBody::from_text("Pause after the title");
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let pkg = Pkg::open(&bytes);
    let master = pkg.parts.get("ppt/notesMasters/notesMaster1.xml").expect("notes master").clone();
    let xml = String::from_utf8(master).expect("utf8");
    let xml = set_placeholder_text(&xml, "hdr", "Acme review");
    let xml = set_placeholder_text(&xml, "ftr", "Do not circulate");
    pkg.replace("ppt/notesMasters/notesMaster1.xml", xml.into_bytes())
}

fn placeholder_text(p: &Presentation, kind: PhType) -> String {
    p.notes_master
        .as_ref()
        .and_then(|m| m.shapes.iter().find(|s| s.ph_type() == Some(kind)))
        .and_then(|s| s.text.as_ref())
        .map(|t| t.text())
        .unwrap_or_default()
}

#[test]
fn owned_corpus_round_trip_keeps_parts_and_text() {
    let talk_bytes = deckcraft_pptx::export(&talk()).expect("talk");
    let (origin, saved) = round_trip("talk", &talk_bytes);
    let text = xml_text(&origin.parts);
    for s in [
        "Quarterly review",
        "North report",
        "Say the number slowly",
        "Region",
        "North",
        "Revenue",
        "Jan",
        "Feb",
        "Actual",
        "Month",
        "Amount due",
        "Ada checks the axis",
        "Owned corpus",
        "DeckCraft tests",
        "https://example.org/north?q=1&x=2",
        "Open the report",
    ] {
        assert!(text.contains_key(s), "talk corpus is missing {s:?}");
    }
    let mark = png(4, 4, [20, 90, 160]);
    assert!(saved.parts.values().any(|b| b == &mark), "the picture bytes did not survive the save");
    assert_eq!(content_counts(&origin.parts), [2, 1, 1, 1, 1, 1], "talk corpus shape");

    let notes = notes_header_deck();
    let _ = round_trip("notes header", &notes);
    let opened = deckcraft_pptx::import(&notes).expect("import notes header");
    assert_eq!(placeholder_text(&opened, PhType::Header), "Acme review");
    assert_eq!(placeholder_text(&opened, PhType::Footer), "Do not circulate");
    let saved = deckcraft_pptx::export(&opened).expect("save notes header");
    let reopened = deckcraft_pptx::import(&saved).expect("reopen notes header");
    assert_eq!(placeholder_text(&reopened, PhType::Header), "Acme review");
    assert_eq!(placeholder_text(&reopened, PhType::Footer), "Do not circulate");
    assert_eq!(reopened.slides[0].notes_text(), "Pause after the title");
}
