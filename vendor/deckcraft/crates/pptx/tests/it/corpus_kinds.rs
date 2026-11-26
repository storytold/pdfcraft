//! Owned corpus of more deck kinds: tables, charts, animations, media, sections, masters and layouts.
//!
//! Each package is built here. The check is open → save → reopen: text in the package and the
//! content parts. A second save keeps the same part names and text.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_model::anim::{AnimStart, Animation, TextBuild};
use deckcraft_model::chart::{Chart, ChartType};
use deckcraft_model::style::PictureFill;
use deckcraft_model::table::Table;
use deckcraft_model::text::{Action, Hyperlink, Run, RunProps, TextBody};
use deckcraft_model::{Geom, LayoutId, LayoutType, MasterId, MediaClip, PhType, Presentation, ShapeKind, Slide, defaults};

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

fn slide_with(p: &mut Presentation, kind: LayoutType, title: &str) -> Slide {
    let layout = defaults::layout_of_kind(p, kind).unwrap_or_default();
    let mut s = defaults::new_slide(p, layout).expect("slide");
    if let Some(t) = s.shapes.first_mut() {
        t.text = Some(TextBody::from_text(title));
    }
    s
}

fn tables() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let mut s = slide_with(&mut p, LayoutType::TitleOnly, "Regions");
    let mut t = Table::new(3, 3, 420.0, 28.0);
    for (r, c, text) in [
        (0, 0, "Quarter"),
        (0, 1, "Region"),
        (0, 2, "Amount"),
        (1, 0, "Q1"),
        (1, 1, "North"),
        (1, 2, "12"),
        (2, 0, "Q2"),
        (2, 1, "South"),
        (2, 2, "Total"),
    ] {
        if let Some(cell) = t.cell_mut(r, c) {
            cell.text = TextBody::from_text(text);
        }
    }
    t.merge(2, 1, 1, 2);
    t.style = "light2-accent3".into();
    let mut ts = shape(&mut p, "Region table", 80.0, 140.0, 420.0, 84.0);
    ts.kind = ShapeKind::Table(t);
    s.shapes.push(ts);
    p.slides = vec![Arc::new(s)];
    p
}

fn chart_of(p: &mut Presentation, name: &str, kind: ChartType, title: &str, x: f64) -> deckcraft_model::Shape {
    let mut chart = Chart::sample(kind);
    chart.title = Some(title.into());
    chart.categories = vec!["Jan".into(), "Feb".into()];
    chart.series.truncate(1);
    if let Some(series) = chart.series.first_mut() {
        series.name = "Actual".into();
        series.values = vec![Some(10.0), Some(20.0)];
    }
    if !matches!(kind, ChartType::Pie) {
        chart.axis_titles = (Some("Month".into()), Some("Amount due".into()));
    }
    let mut ch = shape(p, name, x, 120.0, 280.0, 200.0);
    ch.kind = ShapeKind::Chart(Box::new(chart));
    ch
}

fn charts() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let mut s = slide_with(&mut p, LayoutType::Blank, "Charts");
    s.shapes.extend([
        chart_of(&mut p, "Column chart", ChartType::Column, "Revenue", 40.0),
        chart_of(&mut p, "Line chart", ChartType::Line, "Trend", 340.0),
        chart_of(&mut p, "Pie chart", ChartType::Pie, "Share", 640.0),
    ]);
    p.slides = vec![Arc::new(s)];
    p
}

fn animations() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let mut s = slide_with(&mut p, LayoutType::TitleOnly, "Motion");
    let mut flyer = shape(&mut p, "Flyer", 80.0, 160.0, 200.0, 80.0);
    flyer.geom = Geom::preset("roundRect");
    flyer.text = Some(TextBody::from_text("Fly in"));
    let fly = flyer.id;
    let mut built = shape(&mut p, "Build list", 360.0, 160.0, 240.0, 120.0);
    built.text = Some(TextBody::from_text("One\nTwo\nThree"));
    let body = built.id;
    s.shapes.extend([flyer, built]);
    s.animations = vec![
        Animation { shape: fly, effect: "fly".into(), option: "l".into(), duration_ms: 750, ..Default::default() },
        Animation {
            shape: body,
            effect: "fade".into(),
            text_build: TextBuild::ByParagraph,
            start: AnimStart::AfterPrevious,
            duration_ms: 400,
            ..Default::default()
        },
    ];
    s.transition = Some(deckcraft_model::anim::Transition { kind: "wipe".into(), option: "l".into(), duration_ms: 800, ..Default::default() });
    p.slides = vec![Arc::new(s)];
    p
}

fn media() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let picture = p.add_media("mark.png", "image/png", png(4, 4, [20, 90, 160])).expect("media");
    let tone = p.add_media("tone.wav", "audio/wav", b"owned-wav-tone".to_vec()).expect("media");
    let clip = p.add_media("clip.mp4", "video/mp4", b"owned-mp4-clip".to_vec()).expect("media");
    let mut s = slide_with(&mut p, LayoutType::TitleOnly, "Media");
    let mut pic = shape(&mut p, "Mark", 60.0, 120.0, 80.0, 80.0);
    pic.kind = ShapeKind::Picture { fill: PictureFill { media: picture, ..Default::default() } };
    pic.geom = Geom::preset("rect");
    let mut audio = shape(&mut p, "Tone", 180.0, 120.0, 80.0, 80.0);
    audio.kind = ShapeKind::Media(MediaClip { media: tone, video: false, bookmarks: vec![("Cue the demo".into(), 400)], ..Default::default() });
    let mut video = shape(&mut p, "Clip", 300.0, 120.0, 160.0, 90.0);
    video.kind = ShapeKind::Media(MediaClip { media: clip, video: true, poster: Some(picture), ..Default::default() });
    s.shapes.extend([pic, audio, video]);
    p.slides = vec![Arc::new(s)];
    p
}

fn sections() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let mut first = slide_with(&mut p, LayoutType::Title, "Opening remarks");
    if let Some(t) = first.shapes.get_mut(1) {
        t.text = Some(TextBody {
            paragraphs: vec![deckcraft_model::text::Paragraph {
                runs: vec![Run::with(
                    "Brief",
                    RunProps {
                        link: Some(Hyperlink {
                            action: Action::Url { url: "https://example.org/brief?q=1&x=2".into() },
                            tooltip: "Open the brief".into(),
                            highlight_click: false,
                            sound: None,
                            stop_sound: false,
                            zoom_return: None,
                        }),
                        ..Default::default()
                    },
                )],
                ..Default::default()
            }],
            ..Default::default()
        });
    }
    let second = slide_with(&mut p, LayoutType::SectionHeader, "Figures");
    let ids = [first.id, second.id];
    p.slides = vec![Arc::new(first), Arc::new(second)];
    p.sections = vec![
        deckcraft_model::Section { name: "Opening".into(), slides: vec![ids[0]] },
        deckcraft_model::Section { name: "Figures".into(), slides: vec![ids[1]] },
    ];
    p.custom_shows = vec![deckcraft_model::CustomShow { name: "Short show".into(), slides: vec![ids[0]] }];
    p
}

fn masters() -> Presentation {
    let mut p = defaults::new_presentation(None);
    if let Some(m) = p.masters.first_mut() {
        let m = Arc::make_mut(m);
        if let Some(f) = m.shapes.iter_mut().find(|s| s.ph_type() == Some(PhType::Footer)) {
            f.text = Some(TextBody::from_text("Slide footer line"));
        }
    }
    let mut evening = (**p.masters.first().expect("master")).clone();
    evening.id = MasterId(p.alloc_id().expect("id"));
    evening.name = "Evening".into();
    evening.shapes.retain(|s| s.ph_type().is_some());
    let mut banner = shape(&mut p, "Evening banner", 40.0, 40.0, 400.0, 40.0);
    banner.text = Some(TextBody::from_text("Evening master"));
    evening.shapes.push(banner);
    let mut quote = evening.layouts.first().expect("layout").clone();
    quote.id = LayoutId(p.alloc_id().expect("id"));
    quote.name = "Quote".into();
    quote.kind = LayoutType::Custom;
    quote.shapes.retain(|s| s.ph_type() == Some(PhType::CtrTitle) || s.ph_type() == Some(PhType::Title));
    let mut line = shape(&mut p, "Quote line", 80.0, 200.0, 600.0, 80.0);
    line.text = Some(TextBody::from_text("Pull quote"));
    quote.shapes.push(line);
    evening.layouts = vec![quote.clone()];
    let quote_id = quote.id;
    p.masters.push(Arc::new(evening));

    let title = slide_with(&mut p, LayoutType::Title, "Welcome");
    let agenda = slide_with(&mut p, LayoutType::SectionHeader, "Agenda");
    let compared = slide_with(&mut p, LayoutType::Comparison, "Comparison");
    let mut quoted = defaults::new_slide(&mut p, quote_id).expect("slide");
    if let Some(t) = quoted.shapes.first_mut() {
        t.text = Some(TextBody::from_text("A line"));
    }
    p.slides = vec![Arc::new(title), Arc::new(agenda), Arc::new(compared), Arc::new(quoted)];
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

    fn bytes(&self) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (n, data) in &self.parts {
            out.start_file(n, opts).expect("start");
            out.write_all(data).expect("write");
        }
        out.finish().expect("finish").into_inner()
    }

    fn text_mut(&mut self, name: &str, body: String) {
        self.parts.insert(name.to_string(), body.into_bytes());
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
        for tag in ["p:cSld", "p14:section", "p:custShow"] {
            for text in tag_attr(xml, tag, "name") {
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

fn tag_attr(xml: &str, tag: &str, attr: &str) -> Vec<String> {
    let mut out = vec![];
    let open = format!("<{tag}");
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i..];
        let Some(gt) = after.find('>') else { break };
        let start = after[..gt].find(&format!("{attr}=\"")).map(|k| k + attr.len() + 2);
        if let Some(start) = start
            && let Some(end) = after[start..gt].find('"')
        {
            out.push(decode_xml(&after[start..start + end]));
        }
        rest = &after[gt + 1..];
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

/// Slides, charts, media, comments, handout masters.
fn content_counts(parts: &BTreeMap<String, Vec<u8>>) -> [usize; 5] {
    let is = |prefix: &str, name: &str| {
        let n = name.to_ascii_lowercase();
        n.starts_with(prefix) && n.ends_with(".xml") && !n.contains("_rels")
    };
    [
        part_count(parts, |n| is("ppt/slides/slide", n)),
        part_count(parts, |n| is("ppt/charts/chart", n)),
        part_count(parts, |n| n.to_ascii_lowercase().starts_with("ppt/media/") && !n.to_ascii_lowercase().contains("_rels")),
        part_count(parts, |n| is("ppt/comments/comment", n)),
        part_count(parts, |n| is("ppt/handoutmasters/handoutmaster", n)),
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

fn require(text: &BTreeMap<String, usize>, label: &str, words: &[&str]) {
    for s in words {
        assert!(text.contains_key(*s), "{label} corpus is missing {s:?}");
    }
}

fn ph(id: u32, kind: &str, name: &str, text: &str) -> String {
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{name}\"/><p:cNvSpPr/><p:nvPr><p:ph type=\"{kind}\"/></p:nvPr></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"3657600\" cy=\"365760\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>\
         <p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"
    )
}

fn next_rid(rels: &str) -> String {
    let mut max = 0u32;
    let mut rest = rels;
    while let Some(i) = rest.find("rId") {
        let digits: String = rest[i + 3..].chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u32>() {
            max = max.max(n);
        }
        rest = &rest[i + 3..];
    }
    format!("rId{}", max + 1)
}

/// A deck whose handout master carries the header, footer, date and slide number.
fn handout_deck() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    let s = slide_with(&mut p, LayoutType::TitleOnly, "Leave-behind");
    p.slides = vec![Arc::new(s)];
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut pkg = Pkg::open(&bytes);
    let master = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <p:handoutMaster xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
         <p:cSld name=\"Handouts\"><p:spTree>\
         <p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>\
         {}{}{}{}\
         </p:spTree></p:cSld>\
         <p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>\
         </p:handoutMaster>",
        ph(2, "hdr", "Header Placeholder 1", "Board pack"),
        ph(3, "ftr", "Footer Placeholder 2", "Not for circulation"),
        ph(4, "dt", "Date Placeholder 3", "1 March 2026"),
        ph(5, "sldNum", "Slide Number Placeholder 4", "3"),
    );
    pkg.parts.insert("ppt/handoutMasters/handoutMaster1.xml".into(), master.into_bytes());
    pkg.parts.insert(
        "ppt/handoutMasters/_rels/handoutMaster1.xml.rels".into(),
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
          <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
          <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"../theme/theme1.xml\"/>\
          </Relationships>"
            .to_vec(),
    );
    let rels_name = "ppt/_rels/presentation.xml.rels";
    let rels = String::from_utf8(pkg.parts.get(rels_name).expect("rels").clone()).expect("utf8");
    let rid = next_rid(&rels);
    let rels = rels.replacen(
        "</Relationships>",
        &format!(
            "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/handoutMaster\" Target=\"handoutMasters/handoutMaster1.xml\"/></Relationships>"
        ),
        1,
    );
    pkg.text_mut(rels_name, rels);
    let pres_name = "ppt/presentation.xml";
    let pres = String::from_utf8(pkg.parts.get(pres_name).expect("presentation").clone()).expect("utf8");
    let list = format!("<p:handoutMasterIdLst><p:handoutMasterId r:id=\"{rid}\"/></p:handoutMasterIdLst>");
    let pres = if let Some(at) = pres.find("<p:sldIdLst") {
        let mut out = String::new();
        out.push_str(&pres[..at]);
        out.push_str(&list);
        out.push_str(&pres[at..]);
        out
    } else {
        pres.replacen("<p:sldSz", &format!("{list}<p:sldSz"), 1)
    };
    pkg.text_mut(pres_name, pres);
    let ct = String::from_utf8(pkg.parts.get("[Content_Types].xml").expect("types").clone()).expect("utf8");
    let ct = ct.replacen(
        "</Types>",
        "<Override PartName=\"/ppt/handoutMasters/handoutMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.handoutMaster+xml\"/></Types>",
        1,
    );
    pkg.text_mut("[Content_Types].xml", ct);
    pkg.bytes()
}

fn placeholder_text(p: &Presentation, kind: PhType) -> String {
    p.handout_master
        .as_ref()
        .and_then(|m| m.shapes.iter().find(|s| s.ph_type() == Some(kind)))
        .and_then(|s| s.text.as_ref())
        .map(|t| t.text())
        .unwrap_or_default()
}

#[test]
fn authored_kinds_round_trip_parts_and_text() {
    let tables_bytes = deckcraft_pptx::export(&tables()).expect("tables");
    let (origin, _) = round_trip("tables", &tables_bytes);
    require(&xml_text(&origin.parts), "tables", &["Quarter", "North", "12", "Total", "Regions"]);
    assert_eq!(content_counts(&origin.parts), [1, 0, 0, 0, 0]);

    let charts_bytes = deckcraft_pptx::export(&charts()).expect("charts");
    let (origin, _) = round_trip("charts", &charts_bytes);
    require(&xml_text(&origin.parts), "charts", &["Revenue", "Trend", "Share", "Actual", "Jan", "Feb", "Month", "Amount due"]);
    assert_eq!(content_counts(&origin.parts), [1, 3, 0, 0, 0]);

    let anim_bytes = deckcraft_pptx::export(&animations()).expect("animations");
    let (origin, saved) = round_trip("animations", &anim_bytes);
    require(&xml_text(&origin.parts), "animations", &["Motion", "Fly in", "One", "Two", "Three"]);
    let anim = deckcraft_pptx::import(&saved.bytes()).expect("animations import");
    assert_eq!(anim.slides[0].animations.len(), 2);
    assert_eq!(anim.slides[0].animations[0].effect, "fly");
    assert_eq!(anim.slides[0].animations[1].text_build, TextBuild::ByParagraph);
    assert_eq!(anim.slides[0].transition.as_ref().map(|t| t.kind.as_str()), Some("wipe"));

    let media_bytes = deckcraft_pptx::export(&media()).expect("media");
    let (origin, saved) = round_trip("media", &media_bytes);
    require(&xml_text(&origin.parts), "media", &["Media"]);
    let mark = png(4, 4, [20, 90, 160]);
    assert!(saved.parts.values().any(|b| b == &mark), "the picture bytes did not survive the save");
    assert!(saved.parts.values().any(|b| b == b"owned-wav-tone"), "the audio bytes did not survive the save");
    assert!(saved.parts.values().any(|b| b == b"owned-mp4-clip"), "the video bytes did not survive the save");
    assert!(content_counts(&origin.parts)[2] >= 3, "picture, audio and video");
    let media = deckcraft_pptx::import(&saved.bytes()).expect("media import");
    let tone = media.slides[0].shapes.iter().find(|s| s.name == "Tone").expect("tone");
    let ShapeKind::Media(clip) = &tone.kind else { panic!("not media") };
    assert_eq!(clip.bookmarks, vec![("Cue the demo".into(), 400)]);
    assert!(!clip.video);

    let section_bytes = deckcraft_pptx::export(&sections()).expect("sections");
    let (origin, saved) = round_trip("sections", &section_bytes);
    require(
        &xml_text(&origin.parts),
        "sections",
        &["Opening remarks", "Brief", "Figures", "Opening", "Short show", "https://example.org/brief?q=1&x=2", "Open the brief"],
    );
    let sections = deckcraft_pptx::import(&saved.bytes()).expect("sections import");
    assert_eq!(sections.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Opening", "Figures"]);
    assert_eq!(sections.custom_shows[0].name, "Short show");

    let master_bytes = deckcraft_pptx::export(&masters()).expect("masters");
    let (origin, saved) = round_trip("masters", &master_bytes);
    require(
        &xml_text(&origin.parts),
        "masters",
        &[
            "Welcome",
            "Agenda",
            "Comparison",
            "Pull quote",
            "Evening",
            "Evening master",
            "Quote",
            "Slide footer line",
            "Title Slide",
            "Section Header",
        ],
    );
    let masters = deckcraft_pptx::import(&saved.bytes()).expect("masters import");
    assert_eq!(masters.masters.len(), 2);
    assert!(masters.masters.iter().any(|m| m.name == "Evening"));
    assert!(masters.masters.iter().flat_map(|m| &m.layouts).any(|l| l.name == "Quote" && l.kind == LayoutType::Custom));

    let handout = handout_deck();
    let (origin, _) = round_trip("handout master", &handout);
    require(&xml_text(&origin.parts), "handout", &["Leave-behind", "Board pack", "Not for circulation", "1 March 2026", "Handouts"]);
    assert_eq!(content_counts(&origin.parts)[4], 1, "handout master part");
    let opened = deckcraft_pptx::import(&handout).expect("import handout");
    assert_eq!(opened.handout_master.as_ref().map(|m| m.name.as_str()), Some("Handouts"));
    assert_eq!(placeholder_text(&opened, PhType::Header), "Board pack");
    assert_eq!(placeholder_text(&opened, PhType::Footer), "Not for circulation");
    assert_eq!(placeholder_text(&opened, PhType::Date), "1 March 2026");
    assert_eq!(placeholder_text(&opened, PhType::SlideNum), "3");
    let saved = deckcraft_pptx::export(&opened).expect("save handout");
    let reopened = deckcraft_pptx::import(&saved).expect("reopen handout");
    assert_eq!(placeholder_text(&reopened, PhType::Header), "Board pack");
    assert_eq!(placeholder_text(&reopened, PhType::Footer), "Not for circulation");
    assert_eq!(placeholder_text(&reopened, PhType::Date), "1 March 2026");
    assert_eq!(placeholder_text(&reopened, PhType::SlideNum), "3");
    assert_eq!(reopened.slides[0].shapes.first().and_then(|s| s.text.as_ref()).map(|t| t.text()).unwrap_or_default(), "Leave-behind");
}
