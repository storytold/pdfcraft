//! Speaker notes keep their run and paragraph formatting: a bold heading, indented points with
//! custom bullets, plain lines.

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_model::defaults;
use deckcraft_model::text::{Bullet, ParaProps, TextBody};

const NOTES: &str = "ppt/notesSlides/notesSlide1.xml";

/// A notes slide written the way PowerPoint writes one (Slovak text): a bold heading, a plain
/// line, two level-2 points with a hanging indent and a Wingdings square bullet (one mixing a bold
/// label and plain text), a level-2 paragraph with buNone, and a closing plain line.
const NOTES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sk-SK" b="1" dirty="0"/><a:t>Metodický prehľad</a:t></a:r></a:p><a:p><a:r><a:rPr lang="sk-SK" dirty="0"/><a:t>Žiaci pracujú vo dvojiciach.</a:t></a:r></a:p><a:p><a:pPr marL="285750" lvl="1" indent="-285750"><a:buFont typeface="Wingdings" panose="05000000000000000000" pitchFamily="2" charset="2"/><a:buChar char="&#167;"/></a:pPr><a:r><a:rPr lang="sk-SK" b="1" dirty="0"/><a:t>Cieľ: </a:t></a:r><a:r><a:rPr lang="sk-SK" dirty="0"/><a:t>vysvetliť cyklus</a:t></a:r></a:p><a:p><a:pPr marL="285750" lvl="1" indent="-285750"><a:buFont typeface="Wingdings" panose="05000000000000000000" pitchFamily="2" charset="2"/><a:buChar char="&#167;"/></a:pPr><a:r><a:rPr lang="sk-SK" i="1" dirty="0"/><a:t>Reflexia</a:t></a:r></a:p><a:p><a:pPr marL="285750" lvl="1" indent="-285750"><a:buNone/></a:pPr><a:r><a:rPr lang="sk-SK" dirty="0"/><a:t>Bez odrážky</a:t></a:r></a:p><a:p><a:r><a:rPr lang="sk-SK" dirty="0"/><a:t>Pomôcky: počítač</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#;
const EMPTY_NOTES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#;

fn empty_notes_deck() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    Arc::make_mut(&mut p.slides[0]).notes = TextBody::from_text("placeholder");
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut found = false;
    for i in 0..src.len() {
        let mut f = src.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        if name == NOTES {
            data = EMPTY_NOTES_XML.as_bytes().to_vec();
            found = true;
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        out.write_all(&data).expect("write");
    }
    assert!(found, "the export has no {NOTES}");
    out.finish().expect("finish").into_inner()
}

/// A deck with one notes slide, its notes replaced by [`NOTES_XML`].
fn deck() -> Vec<u8> {
    let mut p = defaults::new_presentation(None);
    Arc::make_mut(&mut p.slides[0]).notes = TextBody::from_text("placeholder");
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut found = false;
    for i in 0..src.len() {
        let mut f = src.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut data = vec![];
        f.read_to_end(&mut data).expect("read");
        if name == NOTES {
            data = NOTES_XML.as_bytes().to_vec();
            found = true;
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        out.write_all(&data).expect("write");
    }
    assert!(found, "the export has no {NOTES}");
    out.finish().expect("finish").into_inner()
}

fn assert_formatted(notes: &TextBody) {
    assert_eq!(notes.text(), "Metodický prehľad\nŽiaci pracujú vo dvojiciach.\nCieľ: vysvetliť cyklus\nReflexia\nBez odrážky\nPomôcky: počítač");
    let [head, plain, goal, reflect, unbulleted, closing] = notes.paragraphs.as_slice() else { panic!("six paragraphs: {notes:?}") };
    assert_eq!(head.runs[0].props.bold, Some(true), "bold heading");
    for para in [head, plain, closing] {
        assert_eq!((para.level, &para.props), (0, &ParaProps::default()), "{:?}", para.text());
    }
    assert_eq!(plain.runs[0].props.bold, None);
    for para in [goal, reflect] {
        assert_eq!(para.level, 1, "level 2");
        assert_eq!(para.props.margin_left, Some(22.5), "marL 285750 EMU");
        assert_eq!(para.props.indent, Some(-22.5), "hanging indent");
        assert_eq!(para.props.bullet, Some(Bullet::Char { char: "§".into() }));
        assert_eq!(para.props.bullet_font.as_deref(), Some("Wingdings"));
    }
    assert_eq!((goal.runs[0].text.as_str(), goal.runs[0].props.bold), ("Cieľ: ", Some(true)), "bold label");
    assert_eq!((goal.runs[1].text.as_str(), goal.runs[1].props.bold), ("vysvetliť cyklus", None), "then plain");
    assert_eq!(reflect.runs[0].props.italic, Some(true));
    assert_eq!((unbulleted.level, unbulleted.props.bullet.clone(), unbulleted.props.margin_left), (1, Some(Bullet::None), Some(22.5)));
}

#[test]
fn notes_import_keeps_bold_levels_and_indents() {
    let p = deckcraft_pptx::import(&deck()).expect("import");
    assert_formatted(&p.slides[0].notes);
}

#[test]
fn notes_formatting_survives_a_round_trip() {
    let p = deckcraft_pptx::import(&deck()).expect("import");
    let again = deckcraft_pptx::import(&deckcraft_pptx::export(&p).expect("export")).expect("reimport");
    assert_formatted(&again.slides[0].notes);
    // And the written XML says so, for PowerPoint.
    let bytes = deckcraft_pptx::export(&again).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut xml = String::new();
    z.by_name(NOTES).expect("notes part").read_to_string(&mut xml).expect("utf-8");
    assert!(xml.contains(r#"b="1""#), "{xml}");
    assert!(xml.contains(r#"lvl="1""#) && xml.contains(r#"marL="285750""#) && xml.contains(r#"indent="-285750""#), "{xml}");
    assert!(xml.contains(r#"<a:buFont typeface="Wingdings""#) && xml.contains("<a:buChar") && xml.contains("<a:buNone/>"), "{xml}");
}

#[test]
fn empty_notes_slide_is_preserved_on_save_and_reopen() {
    let raw = empty_notes_deck();
    let p = deckcraft_pptx::import(&raw).expect("import");
    assert!(p.slides[0].notes.is_empty(), "imported notes text is empty");
    assert!(p.slides[0].has_notes_slide, "slide notes presence is captured");

    let saved = deckcraft_pptx::export(&p).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(&saved)).expect("zip");
    assert!(z.by_name(NOTES).is_ok(), "empty notes slide part is kept in saved package");
    let mut rels_xml = String::new();
    z.by_name("ppt/slides/_rels/slide1.xml.rels").expect("slide rels").read_to_string(&mut rels_xml).expect("read slide rels");
    assert!(rels_xml.contains("notesSlide"), "slide points to notes slide: {rels_xml}");

    let again = deckcraft_pptx::import(&saved).expect("reimport");
    assert!(again.slides[0].notes.is_empty());
    assert!(again.slides[0].has_notes_slide, "notes slide presence survives roundtrip");

    let saved_again = deckcraft_pptx::export(&again).expect("export again");
    let mut z2 = zip::ZipArchive::new(Cursor::new(saved_again)).expect("zip");
    assert!(z2.by_name(NOTES).is_ok(), "empty notes slide part is kept on subsequent export");
}
