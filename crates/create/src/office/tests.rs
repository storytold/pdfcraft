//! Office import tests. Every package is generated here from synthetic text and pixels; no
//! Office file, font or image is committed.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, write_full};

use super::zip::tests::build;
use super::*;
use crate::{SourceKind, source_kind};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const T: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn rels(items: &[(&str, &str, &str)]) -> String {
    let mut s = format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{REL}">"#);
    for (id, ty, target) in items {
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="{T}/{ty}" Target="{target}"/>"#));
    }
    s.push_str("</Relationships>");
    s
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        wr.write_image_data(&vec![90u8; (w * h * 3) as usize]).unwrap();
    }
    out
}

fn reopen(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    Document::open(Arc::new(bytes)).unwrap()
}

fn pages(doc: &Document) -> Vec<Dict> {
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let kids = doc.get(pages).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().clone();
    kids.iter().map(|k| doc.resolve(k).as_dict().cloned().unwrap()).collect()
}

fn content(doc: &Document, page: &Dict) -> String {
    let Object::Stream(s) = &*doc.resolve(page.get(b"Contents").unwrap()) else { panic!("no content") };
    // WinAnsi bytes as characters (Latin-1 agrees with WinAnsi from 0xA0 up).
    s.decoded().unwrap().iter().map(|b| char::from(*b)).collect()
}

fn all_text(doc: &Document) -> String {
    pages(doc).iter().map(|p| content(doc, p)).collect::<Vec<_>>().join("\n")
}

fn size(p: &Dict) -> (f64, f64) {
    let m: Vec<f64> = p.get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    (m[2], m[3])
}

// ── Word ────────────────────────────────────────────────────────────────────────────────────

fn docx(body: &str, extra: &[(&str, &[u8])], doc_rels: &[(&str, &str, &str)]) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}" xmlns:a="{A}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}</w:body></w:document>"#
    );
    let styles = format!(
        r#"<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
        <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
        <w:style w:type="paragraph" w:styleId="Heading1"><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="240"/></w:pPr><w:rPr><w:rFonts w:ascii="Cambria"/><w:b/><w:sz w:val="32"/><w:color w:val="2F5496"/></w:rPr></w:style>
        <w:style w:type="table" w:styleId="TableGrid"><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4"/><w:left w:val="single" w:sz="4"/><w:bottom w:val="single" w:sz="4"/><w:right w:val="single" w:sz="4"/><w:insideH w:val="single" w:sz="4"/><w:insideV w:val="single" w:sz="4"/></w:tblBorders></w:tblPr></w:style>
        <w:style w:type="paragraph" w:styleId="Loop"><w:basedOn w:val="Loop2"/></w:style><w:style w:type="paragraph" w:styleId="Loop2"><w:basedOn w:val="Loop"/></w:style></w:styles>"#
    );
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2)"/></w:lvl></w:abstractNum>
        <w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="&#xF0B7;"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#
    );
    let mut r = vec![("rIdS", "styles", "styles.xml"), ("rIdN", "numbering", "numbering.xml")];
    r.extend_from_slice(doc_rels);
    let doc_rels = rels(&r);
    let root_rels = rels(&[("rId1", "officeDocument", "word/document.xml")]);
    let mut parts: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", b"<Types/>"),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/_rels/document.xml.rels", doc_rels.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
        ("word/numbering.xml", numbering.as_bytes()),
    ];
    parts.extend_from_slice(extra);
    build(&parts)
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
}

#[test]
fn word_paragraphs_styles_lists_tables_images_and_page_numbers() {
    let image = png(40, 20);
    let header = format!(
        r#"<w:hdr xmlns:w="{W}"><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> of </w:t></w:r><w:fldSimple w:instr="NUMPAGES"><w:r><w:t>9</w:t></w:r></w:fldSimple></w:p></w:hdr>"#
    );
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Relatório Anual</w:t></w:r></w:p>
        <w:p><w:r><w:t xml:space="preserve">Texto com </w:t></w:r><w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>negrito</w:t></w:r><w:r><w:t xml:space="preserve"> e ação, coração, São Paulo.</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Primeiro</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Sub</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Segundo</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>Marcador</w:t></w:r></w:p>
        <w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>
          <w:tr><w:tc><w:tcPr><w:shd w:fill="D9E2F3"/></w:tcPr>{}</w:tc><w:tc>{}</w:tc></w:tr>
          <w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr>{}</w:tc></w:tr></w:tbl>
        <w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
        <w:p><w:r><w:t>中文</w:t></w:r><w:r><w:br w:type="page"/></w:r><w:r><w:t>Depois da quebra</w:t></w:r></w:p>
        <w:p><w:pPr><w:pStyle w:val="Loop"/></w:pPr><w:r><w:t>Estilo em ciclo</w:t></w:r></w:p>
        <w:sectPr><w:headerReference w:type="default" r:id="rIdH"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708"/></w:sectPr>"#,
        p("Nome"),
        p("Valor"),
        p("Linha inteira")
    );
    let bytes = docx(
        &body,
        &[("word/media/image1.png", &image), ("word/header1.xml", header.as_bytes())],
        &[("rIdImg", "image", "media/image1.png"), ("rIdH", "header", "header1.xml")],
    );
    let (doc, report) = from_office("Relatorio.docx", &bytes).unwrap();
    assert_eq!(report.kind, OfficeKind::Word);
    assert_eq!(report.pages, 2, "{report:?}");
    assert_eq!(report.escaped, 2, "the two Chinese characters are written as code points: {report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(pg.len(), 2);
    let (w, h) = size(&pg[0]);
    assert!((w - 595.3).abs() < 0.2 && (h - 841.9).abs() < 0.2, "A4 from the section: {w}x{h}");
    let first = content(&doc, &pg[0]);
    let second = content(&doc, &pg[1]);
    for needle in [
        "(Relatório)",
        "(negrito)",
        "(ação,)",
        "(coração,)",
        "(1.)",
        "(a\\))",
        "(2.)",
        "(Primeiro)",
        "(Nome)",
        "(Valor)",
        "(inteira)",
        "([U+4E2D][U+6587])",
        "/Im0 Do",
    ] {
        assert!(first.contains(needle), "missing {needle} on page 1");
    }
    assert!(first.contains("(\u{95})"), "the Symbol bullet is drawn as a bullet");
    assert!(second.contains("(quebra)"), "text after the page break starts page 2");
    assert!(first.contains("(1)") && first.contains("(2)"), "header page number and count on page 1: {first}");
    assert!(second.contains("(2)"), "header page number on page 2");
    // Heading in Times (Cambria is a serif face), body in Helvetica (Calibri).
    let fonts = pg[0].get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let names: Vec<String> =
        fonts.iter().map(|(_, r)| String::from_utf8_lossy(doc.resolve(r).as_dict().unwrap().name(b"BaseFont").unwrap()).into_owned()).collect();
    assert!(
        names.iter().any(|n| n == "Times-Bold") && names.iter().any(|n| n == "Helvetica") && names.iter().any(|n| n == "Helvetica-BoldOblique"),
        "{names:?}"
    );
}

#[test]
fn headings_keep_with_their_next_paragraph_and_lists_drop_contextual_spacing() {
    // Enough body text to leave a little room at the bottom of page 1, then a heading.
    let filler: String = (0..41).map(|i| p(&format!("Linha de enchimento {i}"))).collect();
    let keep = r#"<w:p><w:pPr><w:keepNext/><w:spacing w:before="0" w:after="0"/></w:pPr><w:r><w:t>Titulo preso</w:t></w:r></w:p>"#;
    let tall = r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0"/></w:pPr><w:r><w:t>Corpo</w:t></w:r><w:r><w:br/><w:t>a</w:t><w:br/><w:t>b</w:t><w:br/><w:t>c</w:t><w:br/><w:t>d</w:t></w:r></w:p>"#;
    let (doc, _) = from_office("k.docx", &docx(&format!("{filler}{keep}{tall}"), &[], &[])).unwrap();
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(pg.len(), 2);
    assert!(content(&doc, &pg[1]).contains("(preso)"), "the heading moves to the next page with its paragraph");
    assert!(!content(&doc, &pg[0]).contains("(preso)"));

    // Two list items with contextual spacing sit closer than two plain paragraphs.
    let item = |t: &str, ctx: bool| {
        let c = if ctx { "<w:contextualSpacing/>" } else { "" };
        format!(r#"<w:p><w:pPr><w:spacing w:after="400"/>{c}</w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#)
    };
    let gap = |ctx: bool| {
        let (doc, _) = from_office("c.docx", &docx(&format!("{}{}", item("Um", ctx), item("Dois", ctx)), &[], &[])).unwrap();
        let text = content(&reopen(&doc), &pages(&reopen(&doc))[0]);
        let y = |w: &str| -> f64 {
            let at = text.find(&format!("({w})")).unwrap();
            let tm = text[..at].rfind(" Tm").unwrap();
            text[..tm].split_whitespace().last().unwrap().parse().unwrap()
        };
        y("Um") - y("Dois")
    };
    assert!(gap(false) - gap(true) > 19.0, "contextual spacing removes the 20 pt after: {} vs {}", gap(false), gap(true));
}

#[test]
fn long_documents_paginate_and_tables_split_across_pages() {
    let many: String = (0..120).map(|i| p(&format!("Paragrafo numero {i} com algum texto para ocupar a linha inteira da pagina"))).collect();
    let rows: String = (0..80).map(|i| format!("<w:tr><w:tc>{}</w:tc></w:tr>", p(&format!("Celula {i}")))).collect();
    let tall = format!(
        "<w:tbl><w:tblGrid><w:gridCol w:w=\"5000\"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>",
        (0..90).map(|i| p(&format!("Linha alta {i}"))).collect::<String>()
    );
    let body = format!("{many}<w:tbl><w:tblGrid><w:gridCol w:w=\"5000\"/></w:tblGrid>{rows}</w:tbl>{tall}");
    let (doc, report) = from_office("long.docx", &docx(&body, &[], &[])).unwrap();
    assert!(report.pages >= 6, "{report:?}");
    let text = all_text(&reopen(&doc));
    for needle in ["(119)", "(Celula) Tj", "(79)", "(89)"] {
        assert!(text.contains(needle), "missing {needle}");
    }
}

// ── PowerPoint ──────────────────────────────────────────────────────────────────────────────

fn pptx(slides: &[&str], hidden: &[bool]) -> Vec<u8> {
    pptx_with_master(slides, hidden, "")
}

/// [`pptx`] with `master_extra` (shapes) added to the slide master's shape tree.
fn pptx_with_master(slides: &[&str], hidden: &[bool], master_extra: &str) -> Vec<u8> {
    let theme = format!(
        r#"<a:theme xmlns:a="{A}"><a:themeElements><a:clrScheme name="t"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1></a:clrScheme><a:fontScheme name="f"><a:majorFont><a:latin typeface="Georgia"/></a:majorFont><a:minorFont><a:latin typeface="Arial"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#
    );
    let master = format!(
        r#"<p:sldMaster xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="FAFAFA"/></a:solidFill></p:bgPr></p:bg><p:spTree>
        <p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="274638"/><a:ext cx="8229600" cy="1143000"/></a:xfrm></p:spPr></p:sp>
        <p:sp><p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="1600200"/><a:ext cx="8229600" cy="4525963"/></a:xfrm></p:spPr></p:sp>
        <p:sp><p:nvSpPr><p:cNvPr id="4" name="Band"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="6629400"/><a:ext cx="9144000" cy="228600"/></a:xfrm><a:prstGeom prst="rect"/><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></p:spPr></p:sp>
        {master_extra}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1"/>
        <p:txStyles><p:titleStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="4400"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="342900" indent="-342900"><a:buChar char="•"/><a:defRPr sz="2800"/></a:lvl1pPr><a:lvl2pPr marL="742950" indent="-285750"><a:buChar char="–"/><a:defRPr sz="2400"/></a:lvl2pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#
    );
    let layout = format!(
        r#"<p:sldLayout xmlns:p="{P}" xmlns:a="{A}"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp></p:spTree></p:cSld></p:sldLayout>"#
    );
    let mut ids = String::new();
    let mut pres_rels = Vec::new();
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    for (i, body) in slides.iter().enumerate() {
        let show = if hidden.get(i).copied().unwrap_or(false) { r#" show="0""# } else { "" };
        ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rIdS{i}"/>"#, 256 + i));
        pres_rels.push((format!("rIdS{i}"), format!("slides/slide{}.xml", i + 1)));
        parts.push((
            format!("ppt/slides/slide{}.xml", i + 1),
            format!(r#"<p:sld xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"{show}><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#).into_bytes(),
        ));
        parts.push((
            format!("ppt/slides/_rels/slide{}.xml.rels", i + 1),
            rels(&[("rIdL", "slideLayout", "../slideLayouts/slideLayout1.xml"), ("rIdI", "image", "../media/image1.png")]).into_bytes(),
        ));
    }
    let pres = format!(
        r#"<p:presentation xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/></p:presentation>"#
    );
    let pr: Vec<(&str, &str, &str)> = pres_rels.iter().map(|(id, t)| (id.as_str(), "slide", t.as_str())).collect();
    parts.push(("ppt/presentation.xml".into(), pres.into_bytes()));
    parts.push(("ppt/_rels/presentation.xml.rels".into(), rels(&pr).into_bytes()));
    parts.push(("_rels/.rels".into(), rels(&[("rId1", "officeDocument", "ppt/presentation.xml")]).into_bytes()));
    parts.push(("ppt/slideLayouts/slideLayout1.xml".into(), layout.into_bytes()));
    parts.push((
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels".into(),
        rels(&[("rIdM", "slideMaster", "../slideMasters/slideMaster1.xml")]).into_bytes(),
    ));
    parts.push(("ppt/slideMasters/slideMaster1.xml".into(), master.into_bytes()));
    parts.push(("ppt/slideMasters/_rels/slideMaster1.xml.rels".into(), rels(&[("rIdT", "theme", "../theme/theme1.xml")]).into_bytes()));
    parts.push(("ppt/theme/theme1.xml".into(), theme.into_bytes()));
    parts.push(("ppt/media/image1.png".into(), png(30, 30)));
    let refs: Vec<(&str, &[u8])> = parts.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    build(&refs)
}

#[test]
fn powerpoint_slides_with_inherited_placeholders_bullets_shapes_and_pictures() {
    let title = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:rPr lang="pt-BR"/><a:t>Resultados</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let bullets = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr><a:normAutofit fontScale="90000"/></a:bodyPr><a:p><a:r><a:t>Vendas subiram</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:t>no Brasil</a:t></a:r></a:p><a:p><a:r><a:t>Slide </a:t></a:r><a:fld id="{1}" type="slidenum"><a:t>9</a:t></a:fld></a:p></p:txBody></p:sp>"#;
    let shape = r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="5400000"><a:off x="6000000" y="5000000"/><a:ext cx="2000000" cy="500000"/></a:xfrm><a:prstGeom prst="roundRect"/><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></p:spPr><p:txBody><a:bodyPr anchor="ctr"/><a:p><a:pPr algn="ctr"/><a:r><a:rPr b="1"/><a:t>Destaque</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let pic = r#"<p:pic><p:nvPicPr><p:cNvPr id="6" name="Pic"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdI"/><a:srcRect l="10000" r="10000"/></p:blipFill><p:spPr><a:xfrm><a:off x="100000" y="100000"/><a:ext cx="500000" cy="500000"/></a:xfrm></p:spPr></p:pic>"#;
    let group = r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="7" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="2000000" cy="2000000"/><a:chOff x="0" y="0"/><a:chExt cx="1000000" cy="1000000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="8" name="In"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1000000" cy="300000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr wrap="none"/><a:p><a:r><a:t>Agrupado</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp>"#;
    let s1 = format!("{title}{bullets}{shape}{pic}{group}");
    let s2 = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Hidden"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Oculto</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let s3 = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="Chart"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="2000000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"/></a:graphic></p:graphicFrame>
      <p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="3500000" y="0"/><a:ext cx="4000000" cy="800000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1" bandRow="1"><a:tableStyleId>{5C22544A}</a:tableStyleId></a:tblPr><a:tblGrid><a:gridCol w="2000000"/><a:gridCol w="2000000"/></a:tblGrid><a:tr h="370840"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Mes</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Total</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr h="370840"><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>Jan</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:p><a:r><a:t>42</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#;
    let bytes = pptx(&[&s1, s2, s3], &[false, true, false]);
    let (doc, report) = from_office("deck.pptx", &bytes).unwrap();
    assert_eq!(report.kind, OfficeKind::PowerPoint);
    assert_eq!(report.pages, 2, "the hidden slide is left out: {report:?}");
    assert!(report.notes.iter().any(|n| n.contains("hidden slides")) && report.notes.iter().any(|n| n.contains("charts")), "{report:?}");
    let doc = reopen(&doc);
    let pg = pages(&doc);
    assert_eq!(size(&pg[0]), (720.0, 540.0));
    let s1 = content(&doc, &pg[0]);
    for needle in
        ["(Resultados)", "(Vendas)", "(subiram)", "(\u{95})", "(\u{96})", "(Brasil)", "(1)", "(Destaque)", "/Im0 Do", "(Agrupado)", " re W n"]
    {
        assert!(s1.contains(needle), "missing {needle} on slide 1");
    }
    assert!(!all_text(&doc).contains("Oculto"));
    // The title inherits the master's position and 44 pt centred style in the theme's major font.
    assert!(s1.contains(" 44 Tf"), "title size from the master text style");
    let fonts = pg[0].get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let names: Vec<String> =
        fonts.iter().map(|(_, r)| String::from_utf8_lossy(doc.resolve(r).as_dict().unwrap().name(b"BaseFont").unwrap()).into_owned()).collect();
    assert!(names.contains(&"Times-Roman".to_string()) && names.contains(&"Helvetica".to_string()), "{names:?}");
    // The body was scaled by its 90 % autofit: 28 pt becomes 25.2 pt.
    assert!(s1.contains(" 25.2 Tf"), "autofit scale");
    // The master's accent band is drawn under the slide (scheme colour accent1 = 4472C4).
    assert!(s1.contains("0.267 0.447 0.769 rg"), "master decoration in the theme colour");
    let s3 = content(&doc, &pg[1]);
    assert!(s3.contains("(Mes)") && s3.contains("(42)"), "table cells");
    assert!(s3.contains("1 1 1 rg") && s3.contains("0.267 0.447 0.769 rg"), "header row: white text on the accent colour: {s3}");
}

// ── Excel ───────────────────────────────────────────────────────────────────────────────────

fn xlsx(sheet: &str, extra_sheets: &[(&str, &str, bool)]) -> Vec<u8> {
    xlsx_with(sheet, extra_sheets, "")
}

/// [`xlsx`] with `workbook_pr` (a `<workbookPr>` element, or nothing) in the workbook.
fn xlsx_with(sheet: &str, extra_sheets: &[(&str, &str, bool)], workbook_pr: &str) -> Vec<u8> {
    xlsx_styled(sheet, extra_sheets, workbook_pr, r"[$R$-416]\ #,##0.00")
}

/// [`xlsx_with`] with `format` as the number format of style 1 (`s="1"`).
fn xlsx_styled(sheet: &str, extra_sheets: &[(&str, &str, bool)], workbook_pr: &str, format: &str) -> Vec<u8> {
    let styles = format!(
        r#"<styleSheet xmlns="{S}"><numFmts count="2"><numFmt numFmtId="164" formatCode="{format}"/><numFmt numFmtId="165" formatCode="dd/mm/yyyy"/></numFmts>
        <fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><color rgb="FFFFFFFF"/><name val="Calibri"/></font></fonts>
        <fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor theme="4"/></patternFill></fill></fills>
        <borders count="2"><border/><border><left style="thin"/><right style="thin"/><top style="thin"/><bottom style="medium"><color rgb="FF0000FF"/></bottom></border></borders>
        <cellXfs count="5"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="1" applyNumberFormat="1"/><xf numFmtId="165" fontId="0" fillId="0" borderId="0"/><xf numFmtId="0" fontId="1" fillId="2" borderId="1"><alignment horizontal="center"/></xf><xf numFmtId="10" fontId="0" fillId="0" borderId="0"/></cellXfs></styleSheet>"#
    );
    let shared = format!(
        r#"<sst xmlns="{S}" count="3"><si><t>Produto</t></si><si><t>Preço</t></si><si><r><t>Caf</t></r><r><rPr><b/></rPr><t>é</t></r><rPh><t>X</t></rPh></si></sst>"#
    );
    let mut sheets = r#"<sheet name="Vendas" sheetId="1" r:id="rId1"/>"#.to_string();
    let mut wb_rels = vec![("rId1".to_string(), "worksheet", "worksheets/sheet1.xml".to_string())];
    let mut parts: Vec<(String, Vec<u8>)> =
        vec![("xl/worksheets/sheet1.xml".into(), format!(r#"<worksheet xmlns="{S}" xmlns:r="{R}">{sheet}</worksheet>"#).into_bytes())];
    for (i, (name, body, hidden)) in extra_sheets.iter().enumerate() {
        let id = format!("rIdX{i}");
        let state = if *hidden { r#" state="hidden""# } else { "" };
        sheets.push_str(&format!(r#"<sheet name="{name}" sheetId="{}" r:id="{id}"{state}/>"#, i + 2));
        // A sheet named "Macro…" is related as an Excel 4 macro sheet, not a worksheet.
        let ty = if name.starts_with("Macro") { "xlMacrosheet" } else { "worksheet" };
        wb_rels.push((id, ty, format!("worksheets/sheet{}.xml", i + 2)));
        parts.push((format!("xl/worksheets/sheet{}.xml", i + 2), format!(r#"<worksheet xmlns="{S}" xmlns:r="{R}">{body}</worksheet>"#).into_bytes()));
    }
    let wb = format!(r#"<workbook xmlns="{S}" xmlns:r="{R}">{workbook_pr}<sheets>{sheets}</sheets></workbook>"#);
    let mut r: Vec<(&str, &str, &str)> = wb_rels.iter().map(|(a, b, c)| (a.as_str(), *b, c.as_str())).collect();
    r.push(("rIdSt", "styles", "styles.xml"));
    r.push(("rIdSs", "sharedStrings", "sharedStrings.xml"));
    parts.push(("xl/workbook.xml".into(), wb.into_bytes()));
    parts.push(("xl/_rels/workbook.xml.rels".into(), rels(&r).into_bytes()));
    parts.push(("xl/styles.xml".into(), styles.into_bytes()));
    parts.push(("xl/sharedStrings.xml".into(), shared.into_bytes()));
    parts.push(("_rels/.rels".into(), rels(&[("rId1", "officeDocument", "xl/workbook.xml")]).into_bytes()));
    let refs: Vec<(&str, &[u8])> = parts.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    build(&refs)
}

#[test]
fn excel_sheets_with_formats_fills_borders_and_merges() {
    let sheet = r#"<cols><col min="1" max="1" width="20" customWidth="1"/><col min="2" max="4" width="14" customWidth="1"/></cols><sheetData>
      <row r="1"><c r="A1" t="s" s="3"><v>0</v></c><c r="B1" t="s" s="3"><v>1</v></c></row>
      <row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" s="1"><v>1234.5</v></c><c r="C2" s="2"><v>45000</v></c><c r="D2" s="4"><v>0.1234</v></c></row>
      <row r="3"><c r="A3" t="inlineStr"><is><t>Total</t></is></c><c r="B3" s="1"><f>SUM(B2)</f><v>1234.5</v></c><c r="C3" t="b"><v>1</v></c><c r="D3"><f>A1*2</f></c></row>
      <row r="5"><c r="A5" t="str"><v>Mesclado</v></c></row>
      <row r="40"><c r="A40"><v>3.14159</v></c></row>
    </sheetData><mergeCells count="1"><mergeCell ref="A5:C5"/></mergeCells>"#;
    let wide = r#"<sheetData><row r="1"><c r="A1"><v>1</v></c><c r="Z1"><v>26</v></c></row></sheetData><printOptions gridLines="1"/>"#;
    let bytes = xlsx(
        sheet,
        &[
            ("Wide", wide, false),
            ("Secret", r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Escondido</t></is></c></row></sheetData>"#, true),
        ],
    );
    let (doc, report) = from_office("planilha.xlsx", &bytes).unwrap();
    assert_eq!(report.kind, OfficeKind::Excel);
    assert!(report.pages >= 3, "Vendas fits one page; Wide spreads over two: {report:?}");
    assert!(report.notes.iter().any(|n| n.contains("hidden sheets")) && report.notes.iter().any(|n| n.contains("formulas without")), "{report:?}");
    let doc = reopen(&doc);
    let text = all_text(&doc);
    for needle in
        ["(Produto)", "(Preço)", "(Café)", "(R$ 1.234,50)", "(15/03/2023)", "(12.34%)", "(TRUE)", "(Total)", "(Mesclado)", "(3.14159)", "(26)"]
    {
        assert!(text.contains(needle), "missing {needle}");
    }
    assert!(!text.contains("Escondido") && !text.contains("(X)"), "hidden sheets and phonetic runs are not printed");
    // Header cells: theme accent fill, white bold text, and the blue medium bottom border.
    assert!(text.contains("0 0 1 RG 1 w"), "border colour and weight");
}

#[test]
fn shape_style_font_colour_beats_inherited_text_colour() {
    let shape = r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="800000"/></a:xfrm><a:prstGeom prst="rect"/><a:solidFill><a:srgbClr val="2E7D32"/></a:solidFill></p:spPr><p:style><a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="3"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="2"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style><p:txBody><a:bodyPr anchor="ctr"/><a:p><a:r><a:t>Branco</a:t></a:r><a:r><a:rPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>Vermelho</a:t></a:r></a:p></p:txBody></p:sp>"#;
    let (doc, _) = from_office("s.pptx", &pptx(&[shape], &[])).unwrap();
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("1 1 1 rg 1 0 0 1") && text.contains("(Branco)"), "the style's lt1 font colour");
    assert!(text.contains("1 0 0 rg 1 0 0 1") && text.contains("(Vermelho)"), "a run's own colour still wins");
}

#[test]
fn excel_fit_to_page_shrinks_wide_sheets_onto_one_page() {
    let cols = r#"<cols><col min="1" max="8" width="30" customWidth="1"/></cols>"#;
    let row: String = (0..8).map(|i| format!(r#"<c r="{}1" t="inlineStr"><is><t>Coluna{i}</t></is></c>"#, (b'A' + i) as char)).collect();
    let body = |fit: &str| format!(r#"{fit}{cols}<sheetData><row r="1">{row}</row></sheetData>"#);
    let (_, plain) = from_office("w.xlsx", &xlsx(&body(""), &[])).unwrap();
    assert!(plain.pages >= 3, "{plain:?}");
    let fit = r#"<sheetPr><pageSetUpPr fitToPage="1"/></sheetPr>"#;
    let (doc, fitted) = from_office("w.xlsx", &xlsx(&body(fit), &[])).unwrap();
    assert_eq!(fitted.pages, 1, "{fitted:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Coluna0)") && text.contains("(Coluna7)") && text.contains(" cm\n"), "every column on the page, scaled");
    let scaled = r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><pageSetup scale="50"/>"#;
    let (doc, _) = from_office("s.xlsx", &xlsx(scaled, &[])).unwrap();
    assert!(content(&reopen(&doc), &pages(&reopen(&doc))[0]).starts_with("q\n0.5 0 0 0.5 "), "pageSetup scale");
}

// ── robustness and detection ───────────────────────────────────────────────────────────────

#[test]
fn hostile_and_unsupported_packages_are_refused_with_a_reason() {
    let err = |bytes: &[u8]| from_office("x.docx", bytes).unwrap_err().to_string();
    assert!(err(b"plain text").contains("not an Office Open XML package"));
    assert!(err(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).contains("password-protected"));
    let no_main = build(&[("_rels/.rels", rels(&[]).as_bytes())]);
    assert!(err(&no_main).contains("no main document part"));
    let odd = build(&[("_rels/.rels", rels(&[("r", "officeDocument", "x.xml")]).as_bytes()), ("x.xml", b"<other/>")]);
    assert!(err(&odd).contains("not a Word, PowerPoint or Excel"));
    let dtd =
        build(&[("_rels/.rels", rels(&[("r", "officeDocument", "x.xml")]).as_bytes()), ("x.xml", b"<!DOCTYPE x [<!ENTITY a \"aaaa\">]><x>&a;</x>")]);
    assert!(err(&dtd).contains("malformed XML"), "DTDs are refused");
    // Deep nesting, cyclic styles, huge numbers and missing parts don't panic.
    let deep = format!("{}<w:p><w:r><w:t>fundo</w:t></w:r></w:p>{}", "<w:sdt><w:sdtContent>".repeat(200), "</w:sdtContent></w:sdt>".repeat(200));
    let (_, r) = from_office("deep.docx", &docx(&deep, &[], &[])).unwrap();
    assert_eq!(r.pages, 1);
    let huge = r#"<w:p><w:pPr><w:ind w:left="999999999999" w:hanging="-1e308"/><w:spacing w:line="NaN" w:before="1e300"/></w:pPr><w:r><w:rPr><w:sz w:val="1e30"/></w:rPr><w:t>x</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="0" w:h="-5"/><w:pgMar w:top="99999999" w:left="99999999" w:right="1" w:bottom="1"/></w:sectPr>"#;
    assert!(from_office("huge.docx", &docx(huge, &[], &[])).is_ok());
    let tbl = r#"<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="99999"/><w:vMerge/></w:tcPr></w:tc></w:tr></w:tbl>"#;
    assert!(from_office("t.docx", &docx(tbl, &[], &[])).is_ok());
    let sheet = r#"<sheetData><row r="99999999999"><c r="ZZZZZZ1"><v>1e400</v></c><c r="A1" s="999"><v>NaN</v></c></row></sheetData><cols><col min="-5" max="99999999" width="1e300"/></cols>"#;
    assert!(from_office("h.xlsx", &xlsx(sheet, &[])).is_ok());
    // A table cell taller than a page is cut at the page bottom and the row moves on, rather
    // than starting page after page until the page limit.
    let tall = r#"<w:tbl><w:tblGrid><w:gridCol w:w="5000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="127000000"/><a:graphic><a:graphicData uri="urn:other"/></a:graphic></wp:inline></w:drawing></w:r></w:p><w:p><w:r><w:t>depois</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let (_, r) = from_office("tall.docx", &docx(tall, &[], &[])).unwrap();
    assert!(r.pages <= 3, "{r:?}");
    // Many merged ranges are capped, and each page only searches the ones that touch it.
    let merges: String = (1..=25_000).map(|i| format!(r#"<mergeCell ref="A{i}:B{i}"/>"#)).collect();
    let sheet = format!(r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><mergeCells>{merges}</mergeCells>"#);
    let started = std::time::Instant::now();
    let (_, r) = from_office("m.xlsx", &xlsx(&sheet, &[])).unwrap();
    assert!(r.notes.iter().any(|n| n.contains("merged ranges beyond")), "{r:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let slide = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="x"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="NaN"><a:off x="1e308" y="-1e308"/><a:ext cx="1e308" cy="0"/></a:xfrm><a:custGeom><a:pathLst><a:path w="0" h="0"><a:moveTo><a:pt x="1" y="1"/></a:moveTo><a:arcTo wR="1" hR="1" stAng="0" swAng="1"/></a:path></a:pathLst></a:custGeom><a:solidFill><a:schemeClr val="nope"/></a:solidFill></p:spPr></p:sp>"#;
    assert!(from_office("p.pptx", &pptx(&[slide], &[])).is_ok());
}

#[test]
fn office_files_are_recognised_by_name_and_signature() {
    let d = docx("", &[], &[]);
    assert_eq!(source_kind("a.DOCX", &d), Some(SourceKind::Office));
    assert_eq!(source_kind("a.xlsx", &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]), Some(SourceKind::Office));
    assert_eq!(source_kind("a.zip", &d), None);
    assert_eq!(source_kind("notes.docx", b"just text"), None);
    // An empty document still converts to one blank page.
    let (doc, r) = from_office("vazio.docx", &d).unwrap();
    assert_eq!((r.pages, pages(&reopen(&doc)).len()), (1, 1));
}

#[test]
fn deeply_nested_word_content_is_reported_when_left_out() {
    let deep = format!("{}{}{}", "<w:sdt><w:sdtContent>".repeat(200), p("deep text"), "</w:sdtContent></w:sdt>".repeat(200));
    let (_, report) = from_office("deep.docx", &docx(&deep, &[], &[])).unwrap();
    assert!(report.notes.iter().any(|note| note.contains("nesting limit")), "{report:?}");
}

/// The text a page shows, decoded through each font's own encoding and `/ToUnicode` (whitespace
/// left out: words may be shown one by one).
fn shown_text(doc: &Document, page: &Dict) -> String {
    let fonts = page.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let Object::Stream(s) = &*doc.resolve(page.get(b"Contents").unwrap()) else { panic!("no content") };
    let ops = s.decoded().unwrap();
    let (mut out, mut font, mut i) = (String::new(), None, 0);
    while let Some(&b) = ops.get(i) {
        match b {
            b'/' => {
                let end = ops[i + 1..].iter().position(|c| !c.is_ascii_alphanumeric()).map_or(ops.len(), |e| i + 1 + e);
                let name = ops[i + 1..end].to_vec();
                if let Some(f) = fonts.get(&name) {
                    font = Some(pdfcraft_fonts::pdf::Metrics::from_dict(doc, doc.resolve(f).as_dict().unwrap()));
                }
                i = end;
            }
            b'(' => {
                let mut bytes = Vec::new();
                i += 1;
                while let Some(&c) = ops.get(i) {
                    i += 1;
                    match c {
                        b')' => break,
                        b'\\' => {
                            bytes.push(ops[i]);
                            i += 1;
                        }
                        c => bytes.push(c),
                    }
                }
                out.push_str(&font.as_ref().expect("a font is set before text").decode(&bytes));
            }
            _ => i += 1,
        }
    }
    out.split_whitespace().collect()
}

/// Turkish letters (and the other Latin letters the standard faces carry beyond WinAnsi) are drawn,
/// searchable, in the run's own face; only scripts those faces lack are written as [U+XXXX].
#[test]
fn turkish_and_central_european_letters_are_drawn_not_escaped() {
    let turkish = "Ağaç İşçi ılık Şükrü Ğöl";
    let others = "Łódź Čeština Ősz Șțară";
    let body = format!(
        r#"<w:p><w:r><w:t>{turkish}</w:t></w:r></w:p><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>{others}</w:t></w:r><w:r><w:t xml:space="preserve"> Ωμέγα</w:t></w:r></w:p>"#
    );
    let (doc, report) = from_office("Türkçe.docx", &docx(&body, &[], &[])).unwrap();
    assert_eq!(report.escaped, 5, "only the Greek word is escaped: {report:?}");
    let doc = reopen(&doc);
    let page = &pages(&doc)[0];
    let text = shown_text(&doc, page);
    let expected: String = format!("{turkish} {others} [U+03A9][U+03BC][U+03AD][U+03B3][U+03B1]").split_whitespace().collect();
    assert_eq!(text, expected);
    // The extra letters come from Helvetica and Helvetica-Bold re-encoded, not from another font.
    let fonts = page.get(b"Resources").unwrap().as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap().clone();
    let latin: Vec<Dict> = fonts.iter().filter(|(k, _)| k.starts_with(b"L")).map(|(_, r)| doc.resolve(r).as_dict().unwrap().clone()).collect();
    let mut bases: Vec<&[u8]> = latin.iter().map(|f| f.name(b"BaseFont").unwrap()).collect();
    bases.sort();
    assert_eq!(bases, [&b"Helvetica"[..], b"Helvetica-Bold"]);
    for f in &latin {
        let enc = doc.resolve(f.get(b"Encoding").unwrap()).as_dict().unwrap().clone();
        let diffs = enc.get(b"Differences").unwrap().as_array().unwrap().clone();
        assert!(diffs.iter().any(|d| d.as_name() == Some(b"gbreve")) && diffs.iter().any(|d| d.as_name() == Some(b"dotlessi")));
        // ğ is as wide as g in the same face.
        let face = Std14::from_base_font(std::str::from_utf8(f.name(b"BaseFont").unwrap()).unwrap()).unwrap();
        let m = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, f);
        assert_eq!(m.width(u32::from(latin::code('ğ').unwrap())) * 1000.0, face.width(b'g'));
    }
}

/// PAGEREF (a table of contents' page number) keeps Word's cached result; only PAGE, NUMPAGES and
/// SECTIONPAGES are page fields, with or without a cached result.
#[test]
fn page_reference_fields_keep_their_cached_result() {
    let field = |instr: &str, cached: Option<&str>| {
        let result = cached.map_or(String::new(), |c| format!(r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{c}</w:t></w:r>"#));
        format!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r>{result}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        )
    };
    let body = [
        p("Contents"),
        field(r"PAGEREF _Toc123 \h", Some("77")),
        r#"<w:p><w:fldSimple w:instr=" PAGEREF _Toc456 \h "><w:r><w:t>88</w:t></w:r></w:fldSimple></w:p>"#.to_string(),
        field("PAGE", Some("9")),
        field("SECTIONPAGES", None),
        field("NUMPAGESX", Some("66")),
    ]
    .concat();
    let (doc, report) = from_office("toc.docx", &docx(&body, &[], &[])).unwrap();
    assert_eq!(report.pages, 1);
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(77)") && text.contains("(88)"), "the cross-reference page numbers Word cached: {text}");
    assert!(!text.contains("(9)"), "PAGE is the live page number, not its cached 9");
    assert_eq!(text.matches("(1)").count(), 2, "PAGE and SECTIONPAGES (no cached result) both show 1: {text}");
    assert!(text.contains("(66)"), "a field merely starting with NUMPAGES keeps its cached result");
}

/// The shape limit is per slide: a deck whose slides together pass it draws every slide, and a
/// slide past it is reported.
#[test]
fn the_shape_limit_applies_per_slide_and_is_reported() {
    let shape = |i: usize| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{i}" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr></p:sp>"#
        )
    };
    let label = |t: &str| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="L"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="500000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:txBody></p:sp>"#
        )
    };
    let busy: String = (0..12_000).map(shape).collect();
    let first = format!("{busy}{}", label("Primeiro"));
    let second = format!("{busy}{}", label("Segundo"));
    let (doc, report) = from_office("busy.pptx", &pptx(&[&first, &second], &[])).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("cut short")), "{report:?}");
    let doc = reopen(&doc);
    assert!(all_text(&doc).contains("(Segundo)"), "the second slide is drawn, not cut by the first slide's shapes");
    let crowded: String = (0..20_010).map(shape).collect();
    let (_, report) = from_office("crowded.pptx", &pptx(&[&crowded], &[])).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("cut short")), "{report:?}");
}

/// Workbooks saved with the 1904 date system (older Excel for Mac) show their own dates, not dates
/// 66 years later.
#[test]
fn dates_in_1904_workbooks_are_not_shifted() {
    let sheet = |day: u32| {
        format!(
            r#"<cols><col min="1" max="3" width="14" customWidth="1"/></cols><sheetData><row r="1"><c r="A1" s="2"><v>0</v></c><c r="B1" s="2"><v>{day}</v></c><c r="C1" s="2"><v>61</v></c></row></sheetData>"#
        )
    };
    // 14 March 2019 is day 43538 in the 1900 system and day 42076 (1,462 fewer) in the 1904 one.
    let (doc, _) = from_office("1900.xlsx", &xlsx(&sheet(43_538), &[])).unwrap();
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(14/03/2019)") && text.contains("(01/03/1900)"), "the 1900 system, around its 29 February 1900: {text}");
    let (doc, _) = from_office("1904.xlsx", &xlsx_with(&sheet(42_076), &[], r#"<workbookPr date1904="1"/>"#)).unwrap();
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(01/01/1904)") && text.contains("(14/03/2019)") && text.contains("(02/03/1904)"), "{text}");
}

/// A sheet with one value in its last cell, columns as wide and rows as tall as Excel allows, has
/// billions of empty print pages: only the page with the value is looked at.
#[test]
fn a_sparse_sheet_prints_only_its_occupied_page_quickly() {
    let sheet = r#"<sheetFormatPr defaultColWidth="400" defaultRowHeight="409"/><sheetData><row r="1048576"><c r="XFD1048576" t="inlineStr"><is><t>Fim</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:B2"/></mergeCells>"#;
    let started = std::time::Instant::now();
    let (doc, report) = from_office("sparse.xlsx", &xlsx(sheet, &[])).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 1, "{report:?}");
    assert!(all_text(&reopen(&doc)).contains("(Fim)"));
}

/// Every cell re-reads its number format, so a format of a million zeros (Excel stops at 255
/// characters) is shown as General, and reported, instead of padding each cell for minutes.
#[test]
fn an_oversized_number_format_is_shown_as_general_quickly() {
    let rows: String = (1..=200).map(|r| format!(r#"<row r="{r}"><c r="A{r}" s="1"><v>1234.5</v></c></row>"#)).collect();
    let sheet = format!(r#"<cols><col min="1" max="1" width="30" customWidth="1"/></cols><sheetData>{rows}</sheetData>"#);
    let started = std::time::Instant::now();
    let (doc, report) = from_office("zeros.xlsx", &xlsx_styled(&sheet, &[], "", &"0".repeat(1_000_000))).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert!(report.notes.iter().any(|n| n.contains("longer than Excel allows")), "{report:?}");
    assert!(all_text(&reopen(&doc)).contains("(1234.5)"));
    // Excel's longest format is still read.
    let (_, report) = from_office("long.xlsx", &xlsx_styled(&sheet, &[], "", &"0".repeat(255))).unwrap();
    assert!(!report.notes.iter().any(|n| n.contains("longer than Excel allows")), "{report:?}");
}

/// Gridlines and cell positions are found from running totals: a page whose band holds 200 000
/// hidden (zero-height) rows used to sum their heights again for every gridline, and drew a
/// gridline for each of them at the same place.
#[test]
fn gridlines_over_many_hidden_rows_are_drawn_once_and_quickly() {
    let hidden: String = (2..=200_000).map(|r| format!(r#"<row r="{r}" hidden="1"/>"#)).collect();
    let sheet = format!(
        r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Topo</t></is></c></row>{hidden}<row r="200001"><c r="A200001" t="inlineStr"><is><t>Base</t></is></c></row></sheetData><printOptions gridLines="1"/>"#
    );
    let started = std::time::Instant::now();
    let (doc, report) = from_office("hidden.xlsx", &xlsx(&sheet, &[])).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 1, "{report:?}");
    let doc = reopen(&doc);
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Topo)") && text.contains("(Base)"));
    // Three horizontal gridlines (above, between and below the two visible rows) and the
    // column's two vertical ones.
    assert_eq!(text.matches(" l S").count(), 5, "{text}");
}

/// A macro sheet is a relationship of its own type: its cells are not printed, and the report
/// says it was left out.
#[test]
fn macro_sheets_are_left_out_and_reported() {
    let main = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Vendas</t></is></c></row></sheetData>"#;
    let macro_sheet = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>=HALT()</t></is></c></row></sheetData>"#;
    let (doc, report) = from_office("macros.xlsm", &xlsx(main, &[("Macro1", macro_sheet, false)])).unwrap();
    assert_eq!(report.pages, 1, "{report:?}");
    assert!(report.notes.iter().any(|n| n.contains("macro sheets, left out")), "{report:?}");
    let text = all_text(&reopen(&doc));
    assert!(text.contains("(Vendas)") && !text.contains("HALT"), "{text}");
}

/// Placeholder lookups use an index built once per slide: a slide of 10 000 placeholders over a
/// master holding 50 000 used to walk the whole master for every one of them.
#[test]
fn placeholder_lookups_over_a_large_master_are_quick() {
    let sp = |id: usize, idx: usize| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="P"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="{idx}"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>"#
        )
    };
    // Wrapped in an element the importer doesn't draw: the lookups still walked all of it.
    let master = format!("<p:custDataLst>{}</p:custDataLst>", (0..50_000).map(|i| sp(100 + i, 100_000 + i)).collect::<String>());
    let mut slide: String = (0..10_000).map(|i| sp(10 + i, 900_000 + i)).collect();
    slide.push_str(r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>Titulo</a:t></a:r></a:p></p:txBody></p:sp>"#);
    let started = std::time::Instant::now();
    let (doc, report) = from_office("big.pptx", &pptx_with_master(&[&slide], &[], &master)).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 1, "{report:?}");
    let doc = reopen(&doc);
    // The title still takes the master title placeholder's 44 pt style.
    let text = content(&doc, &pages(&doc)[0]);
    assert!(text.contains("(Titulo)") && text.contains(" 44 Tf"), "{text}");
}

/// A layout, master or theme shared by every slide is parsed and indexed once: 200 slides over a
/// 100 000-shape master used to parse it 200 times.
#[test]
fn a_large_master_shared_by_many_slides_is_parsed_once() {
    let sp = |id: usize| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="P"/><p:cNvSpPr/><p:nvPr><p:ph type="body" idx="{id}"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>"#
        )
    };
    let master = format!("<p:custDataLst>{}</p:custDataLst>", (100..100_100).map(sp).collect::<String>());
    let slides: Vec<String> = (1..=200)
        .map(|i| {
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="T"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="500000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>Slide{i}</a:t></a:r></a:p></p:txBody></p:sp>"#
            )
        })
        .collect();
    let refs: Vec<&str> = slides.iter().map(String::as_str).collect();
    let started = std::time::Instant::now();
    let (doc, report) = from_office("deck.pptx", &pptx_with_master(&refs, &[], &master)).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    assert_eq!(report.pages, 200, "{report:?}");
    let doc = reopen(&doc);
    assert!(content(&doc, &pages(&doc)[199]).contains("(Slide200)"));
}
