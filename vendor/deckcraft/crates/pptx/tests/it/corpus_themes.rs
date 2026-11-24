//! Owned corpus: themes, several masters and layouts, shape effects (shadow, glow, soft edge,
//! reflection) and hyperlinks and actions on text and shapes.
//!
//! The decks are built as PowerPoint writes them: a DeckCraft package whose theme, slides and
//! relationships are replaced with PowerPoint-shaped XML, and a two-master deck built in the model.
//! The check is open → save → reopen with nothing lost.

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_model::style::Effects;
use deckcraft_model::text::{Action, Hyperlink};
use deckcraft_model::{LayoutType, Presentation, Shape, defaults};

/// Replace (or add) parts of a package.
fn repack(bytes: &[u8], parts: &[(&str, String)]) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(vec![]));
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let name = f.name().to_string();
        if parts.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        out.start_file(name, opts).expect("start");
        out.write_all(&data).expect("write");
    }
    for (n, xml) in parts {
        out.start_file(*n, opts).expect("start");
        out.write_all(xml.as_bytes()).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut s = String::new();
    z.by_name(name).expect(name).read_to_string(&mut s).expect("utf8");
    s
}

fn round_trip(p: &Presentation) -> Presentation {
    deckcraft_pptx::import(&deckcraft_pptx::export(p).expect("export")).expect("reimport")
}

fn shapes_named<'a>(p: &'a Presentation, slide: usize) -> impl Fn(&str) -> &'a Shape {
    move |n| p.slides[slide].shapes.iter().find(|s| s.name == n).unwrap_or_else(|| panic!("no shape {n}"))
}

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

fn sp(id: u32, name: &str, nv_extra: &str, x: u32, effects: &str, body: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}">{nv_extra}</p:cNvPr><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="1270000"/><a:ext cx="2540000" cy="1270000"/></a:xfrm><a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent1"/></a:solidFill><a:effectLst>{effects}</a:effectLst></p:spPr><p:txBody><a:bodyPr rtlCol="0" anchor="ctr"/><a:lstStyle/>{body}</p:txBody></p:sp>"#
    )
}

/// A two-slide PowerPoint-style deck: a custom theme, effects on four shapes, and text and shape
/// hyperlinks of every common kind.
fn effects_and_links() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    let layout = defaults::layout_of_kind(&p, LayoutType::Blank).unwrap_or_default();
    for _ in 0..2 {
        let s = defaults::new_slide(&mut p, layout).expect("slide");
        p.slides.push(Arc::new(s));
    }
    p.slides.truncate(2);
    let base = deckcraft_pptx::export(&p).expect("export");
    let rels = part(&base, "ppt/slides/_rels/slide1.xml.rels");
    let rels = rels.replace(
        "</Relationships>",
        r#"<Relationship Id="rId90" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/report?q=1&amp;r=2" TargetMode="External"/><Relationship Id="rId91" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slide2.xml"/><Relationship Id="rId92" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="mailto:team@example.com?subject=Deck" TargetMode="External"/></Relationships>"#,
    );
    let shadow = sp(
        10,
        "Shadowed",
        r#"<a:hlinkClick r:id="" action="ppaction://hlinkshowjump?jump=endshow" highlightClick="1"/><a:hlinkHover r:id="rId90" tooltip="Hover for the report"/>"#,
        254000,
        r#"<a:outerShdw blurRad="76200" dist="38100" dir="2700000" sx="102000" sy="102000" algn="tl" rotWithShape="0"><a:prstClr val="black"><a:alpha val="40000"/></a:prstClr></a:outerShdw>"#,
        r#"<a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>Shadow</a:t></a:r></a:p>"#,
    );
    let glow = sp(
        11,
        "Glowing",
        "",
        3048000,
        r#"<a:glow rad="139700"><a:schemeClr val="accent2"><a:satMod val="175000"/><a:alpha val="40000"/></a:schemeClr></a:glow>"#,
        r#"<a:p><a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rId90" tooltip="Open the report"/></a:rPr><a:t>Report</a:t></a:r><a:r><a:rPr lang="en-US"/><a:t> and </a:t></a:r><a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rId91" action="ppaction://hlinksldjump"/></a:rPr><a:t>appendix</a:t></a:r></a:p>"#,
    );
    let soft = sp(
        12,
        "Soft edged",
        "",
        5842000,
        r#"<a:softEdge rad="63500"/>"#,
        r#"<a:p><a:r><a:rPr lang="en-US"><a:hlinkClick r:id="" action="ppaction://hlinkshowjump?jump=nextslide"/></a:rPr><a:t>Next</a:t></a:r><a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rId92"/></a:rPr><a:t> mail us</a:t></a:r></a:p>"#,
    );
    let refl = sp(
        13,
        "Reflected",
        "",
        8636000,
        r#"<a:reflection blurRad="6350" stA="50000" endA="300" endPos="55500" dist="50800" stPos="20000" dir="5400000" fadeDir="0" sx="75000" sy="-50000" kx="600000" ky="-300000" algn="bl" rotWithShape="0"/>"#,
        r#"<a:p><a:r><a:rPr lang="en-US"><a:hlinkMouseOver r:id="rId90" tooltip="Preview"/></a:rPr><a:t>Hover me</a:t></a:r></a:p>"#,
    );
    let slide = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{shadow}{glow}{soft}{refl}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    );
    repack(&base, &[("ppt/slides/slide1.xml", slide), ("ppt/slides/_rels/slide1.xml.rels", rels)])
}

fn fx(s: &Shape) -> Effects {
    s.effects.clone().unwrap_or_default()
}

fn run_links(s: &Shape) -> Vec<(String, Option<Hyperlink>)> {
    s.text.iter().flat_map(|t| t.paragraphs.iter()).flat_map(|p| p.runs.iter()).map(|r| (r.text.clone(), r.props.link.clone())).collect()
}

#[test]
fn effects_and_links_survive_a_round_trip() {
    let bytes = effects_and_links();
    let a = deckcraft_pptx::import(&bytes).expect("import");
    let get = shapes_named(&a, 0);
    // Read as PowerPoint wrote them.
    let sh = fx(get("Shadowed")).outer_shadow.expect("shadow");
    assert!((sh.blur - 6.0).abs() < 1e-6 && (sh.dist - 3.0).abs() < 1e-6 && (sh.dir - 45.0).abs() < 1e-6, "{sh:?}");
    assert_eq!((sh.align.as_str(), sh.rotate_with_shape), ("tl", false));
    assert!((sh.sx - 1.02).abs() < 1e-6);
    assert!(fx(get("Glowing")).glow.is_some_and(|g| (g.radius - 11.0).abs() < 1e-6));
    assert_eq!(fx(get("Soft edged")).soft_edge, Some(5.0));
    let r = fx(get("Reflected")).reflection.expect("reflection");
    assert!((r.end_pos - 0.555).abs() < 1e-6 && (r.dist - 4.0).abs() < 1e-6, "{r:?}");
    assert_eq!(
        (r.st_pos, r.dir, r.fade_dir, r.sx, r.sy, r.kx, r.ky),
        (Some(20_000), Some(5_400_000), Some(0), Some(75_000), Some(-50_000), Some(600_000), Some(-300_000))
    );
    assert_eq!((r.align.as_deref(), r.rotate_with_shape), (Some("bl"), Some(false)));
    let shape = get("Shadowed");
    assert!(matches!(shape.click.as_ref().map(|h| (&h.action, h.highlight_click)), Some((Action::EndShow, true))));
    assert!(
        matches!(shape.hover.as_ref().map(|h| (&h.action, h.tooltip.as_str())), Some((Action::Url { url }, "Hover for the report")) if url.contains("q=1&r=2"))
    );
    let links = run_links(get("Glowing"));
    assert!(matches!(&links[0].1, Some(Hyperlink { action: Action::Url { .. }, tooltip, .. }) if tooltip == "Open the report"));
    assert!(links[1].1.is_none());
    assert!(matches!(&links[2].1, Some(Hyperlink { action: Action::Slide { slide }, .. }) if *slide == a.slides[1].id));
    let links = run_links(get("Soft edged"));
    assert!(matches!(&links[0].1, Some(Hyperlink { action: Action::NextSlide, .. })));
    assert!(matches!(&links[1].1, Some(Hyperlink { action: Action::Url { url }, .. }) if url.starts_with("mailto:")));
    let hover = get("Reflected").text.as_ref().and_then(|t| t.paragraphs[0].runs[0].props.hover.clone());
    assert!(matches!(hover, Some(Hyperlink { action: Action::Url { .. }, ref tooltip, .. }) if tooltip == "Preview"), "{hover:?}");
    effects_and_links_survive_resave(&a);
}

fn effects_and_links_survive_resave(a: &Presentation) {
    // Saved and reopened: the same effects and links.
    let b = round_trip(a);
    let (ga, gb) = (shapes_named(a, 0), shapes_named(&b, 0));
    for n in ["Shadowed", "Glowing", "Soft edged", "Reflected"] {
        assert_eq!(fx(ga(n)), fx(gb(n)), "effects of {n}");
        assert_eq!(ga(n).click, gb(n).click, "click of {n}");
        assert_eq!(ga(n).hover, gb(n).hover, "hover of {n}");
        let la: Vec<_> = run_links(ga(n))
            .into_iter()
            .map(|(t, l)| (t, l.map(|h| (format!("{:?}", h.action).replace(&format!("{:?}", a.slides[1].id), "S2"), h.tooltip))))
            .collect();
        let lb: Vec<_> = run_links(gb(n))
            .into_iter()
            .map(|(t, l)| (t, l.map(|h| (format!("{:?}", h.action).replace(&format!("{:?}", b.slides[1].id), "S2"), h.tooltip))))
            .collect();
        assert_eq!(la, lb, "text links of {n}");
        let ha: Vec<_> = ga(n).text.iter().flat_map(|t| &t.paragraphs).flat_map(|p| &p.runs).map(|r| r.props.hover.clone()).collect();
        let hb: Vec<_> = gb(n).text.iter().flat_map(|t| &t.paragraphs).flat_map(|p| &p.runs).map(|r| r.props.hover.clone()).collect();
        assert_eq!(ha, hb, "text hover of {n}");
    }
    let out = deckcraft_pptx::export(&b).expect("export");
    let xml = part(&out, "ppt/slides/slide1.xml");
    assert!(xml.contains("hlinkMouseOver"));
    let c = deckcraft_pptx::import(&out).expect("second reopen");
    assert_eq!(fx(shapes_named(&b, 0)("Reflected")).reflection, fx(shapes_named(&c, 0)("Reflected")).reflection);
    assert!(
        xml.contains("stPos=\"20000\"")
            && xml.contains("fadeDir=\"0\"")
            && xml.contains("sx=\"75000\"")
            && xml.contains("sy=\"-50000\"")
            && xml.contains("kx=\"600000\"")
    );
}

/// Two masters with their own themes (colours, fonts) and layouts, slides on each.
fn two_masters() -> Presentation {
    let mut p = defaults::new_presentation(None);
    let mut second = (*p.masters[0]).clone();
    second.id = deckcraft_model::MasterId(p.alloc_id().expect("id"));
    second.name = "Brand Dark".into();
    second.theme.name = "Brand Dark Theme".into();
    second.theme.colors.name = "Brand Colours".into();
    second.theme.colors.colors[4] = deckcraft_model::Rgba::rgb(0x12, 0x34, 0x56);
    second.theme.colors.colors[2] = deckcraft_model::Rgba::rgb(0x20, 0x20, 0x30);
    second.theme.fonts.name = "Brand Fonts".into();
    second.theme.fonts.major.latin = "Georgia".into();
    second.theme.fonts.minor.latin = "Verdana".into();
    second.background = Some(deckcraft_model::Background::Fill {
        fill: deckcraft_model::Fill::solid(deckcraft_model::ColorRef::rgb(deckcraft_model::Rgba::rgb(0x10, 0x10, 0x18))),
    });
    for l in &mut second.layouts {
        l.id = deckcraft_model::LayoutId(p.alloc_id().expect("id"));
        l.name = format!("Dark {}", l.name);
    }
    second.layouts.truncate(4);
    let dark_layout = second.layouts[1].id;
    p.masters.push(Arc::new(second));
    let light_layout = p.masters[0].layouts[0].id;
    let s1 = defaults::new_slide(&mut p, light_layout).expect("slide");
    let s2 = defaults::new_slide(&mut p, dark_layout).expect("slide");
    p.slides = vec![Arc::new(s1), Arc::new(s2)];
    p
}

fn same_master(ma: &deckcraft_model::Master, mb: &deckcraft_model::Master) {
    assert_eq!(ma.name, mb.name);
    assert_eq!(ma.theme.name, mb.theme.name);
    // The writer adds PowerPoint's empty objectDefaults/extraClrSchemeLst, kept as raw XML.
    let bare = |t: &deckcraft_model::theme::Theme| deckcraft_model::theme::Theme { raw_extra: None, ..t.clone() };
    assert_eq!(bare(&ma.theme), bare(&mb.theme), "whole theme of {}", ma.name);
    assert_eq!(ma.theme.colors, mb.theme.colors, "colour scheme of {}", ma.name);
    assert_eq!(ma.theme.fonts.name, mb.theme.fonts.name);
    assert_eq!(ma.theme.fonts.major.latin, mb.theme.fonts.major.latin);
    assert_eq!(ma.theme.fonts.minor.latin, mb.theme.fonts.minor.latin);
    assert_eq!(ma.background, mb.background, "background of {}", ma.name);
    let names = |m: &deckcraft_model::Master| m.layouts.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(ma), names(mb));
}

#[test]
fn themes_masters_and_layouts_survive_a_round_trip() {
    let a = two_masters();
    let b = round_trip(&a);
    assert_eq!(b.masters.len(), 2);
    for (ma, mb) in a.masters.iter().zip(&b.masters) {
        same_master(ma, mb);
    }
    // Each slide stays on its master's layout.
    let layout_name =
        |p: &Presentation, i: usize| p.masters.iter().flat_map(|m| m.layouts.iter()).find(|l| l.id == p.slides[i].layout).map(|l| l.name.clone());
    for i in 0..2 {
        assert_eq!(layout_name(&a, i), layout_name(&b, i), "slide {i}");
    }
    // A second save keeps the same theme and master parts.
    let names = |bytes: &[u8]| {
        let z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
        let mut v: Vec<String> =
            z.file_names().filter(|n| n.contains("theme") || n.contains("slideMaster") || n.contains("slideLayout")).map(str::to_string).collect();
        v.sort();
        v
    };
    let c = round_trip(&b);
    for (mb, mc) in b.masters.iter().zip(&c.masters) {
        assert_eq!(mb.theme, mc.theme, "stable after a second save");
        // An empty map is the default one, which the writer spells out.
        assert_eq!(mb.color_map, mc.color_map);
    }
    let once = deckcraft_pptx::export(&b).expect("export");
    let twice = deckcraft_pptx::export(&deckcraft_pptx::import(&once).expect("import")).expect("export");
    assert_eq!(names(&once), names(&twice));
}
