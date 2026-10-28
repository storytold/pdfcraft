//! #4966: committing typed letters must not reuse missing Type 1 subset glyphs.
//! The PDF and its rectangular test glyphs are contributor-original, generated in memory.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions, Stream, write_full};
use pdfcraft_engine::DocId;
use serde_json::{Value, json};

/// A tiny Type 1 program: every defined letter draws a rectangle; .notdef draws nothing.
/// P and b are deliberately absent, just as in the reported TeX subset.
fn subset_program(remap: bool) -> Stream {
    let letters = "QuestionrlmChc";
    let mut private = b"\0\0\0\0dup /Private 8 dict dup begin\n/lenIV -1 def\n/password 5839 def\n/BlueValues [] def\n/RD {string currentfile exch readstring pop} executeonly def\n/ND {noaccess def} executeonly def\nend readonly put\ndup /CharStrings 15 dict dup begin\n".to_vec();
    // 0 600 hsbw endchar. Each visible glyph adds an original 500 x 700 rectangle.
    let empty = [139, 248, 236, 13, 14];
    let rectangle = [139, 248, 236, 13, 189, 139, 21, 139, 249, 80, 5, 248, 136, 139, 5, 139, 253, 80, 5, 252, 136, 139, 5, 9, 14];
    for (name, outline) in
        std::iter::once((".notdef".to_string(), empty.as_slice())).chain(letters.chars().map(|ch| (ch.to_string(), rectangle.as_slice())))
    {
        private.extend_from_slice(format!("/{name} {} RD ", outline.len()).as_bytes());
        private.extend_from_slice(outline);
        private.extend_from_slice(b" ND\n");
    }
    private.extend_from_slice(b"end readonly put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n");
    let mut program = b"%!FontType1-1.0: ABCDEF+Example-Bold 1.0\n11 dict begin\n/FontName /ABCDEF+Example-Bold def\n/FontType 1 def\n/FontMatrix [0.001 0 0 0.001 0 0] def\n/FontBBox [0 0 600 700] def\n/Encoding StandardEncoding def\ncurrentdict end\ncurrentfile eexec\n".to_vec();
    if remap {
        let encoding: String = (0..=255_u8)
            .map(|code| {
                if code == b'P' {
                    "/Q ".to_string()
                } else if letters.contains(char::from(code)) {
                    format!("/{} ", char::from(code))
                } else {
                    "/.notdef ".to_string()
                }
            })
            .collect();
        program = String::from_utf8(program)
            .unwrap()
            .replace("/Encoding StandardEncoding def", &format!("/Encoding [{encoding}] readonly def"))
            .into_bytes();
    }
    let clear_length = program.len();
    // Type 1 eexec cipher (not document encryption); four discarded bytes prefix the private dict.
    let mut state = 55_665_u16;
    for byte in private {
        let encrypted = byte ^ (state >> 8) as u8;
        state = u16::from(encrypted).wrapping_add(state).wrapping_mul(52_845).wrapping_add(22_719);
        program.extend_from_slice(format!("{encrypted:02X}").as_bytes());
    }
    let encrypted_length = program.len() - clear_length;
    program.extend_from_slice(b"\n");
    program.extend_from_slice("0".repeat(512).as_bytes());
    program.extend_from_slice(b"\ncleartomark\n");
    let dict = Dict::from_iter([
        (b"Length1".to_vec(), Object::Int(clear_length as i64)),
        (b"Length2".to_vec(), Object::Int(encrypted_length as i64)),
        (b"Length3".to_vec(), Object::Int((program.len() - clear_length - encrypted_length) as i64)),
    ]);
    Stream::flate(dict, &program)
}

fn fixture(remap: bool) -> Vec<u8> {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let program = doc.add(Object::Stream(subset_program(remap)));
    let descriptor = Dict::from_iter([
        (b"Type".to_vec(), Object::name("FontDescriptor")),
        (b"FontName".to_vec(), Object::name("ABCDEF+Example-Bold")),
        (b"Flags".to_vec(), Object::Int(32)),
        (b"FontBBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 700.into()])),
        (b"Ascent".to_vec(), Object::Int(700)),
        (b"Descent".to_vec(), Object::Int(0)),
        (b"CapHeight".to_vec(), Object::Int(700)),
        (b"ItalicAngle".to_vec(), Object::Int(0)),
        (b"StemV".to_vec(), Object::Int(80)),
        (b"CharSet".to_vec(), PdfString::literal(b"/Q/u/e/s/t/i/o/n/r/l/m/C/h/c").into()),
        (b"FontFile".to_vec(), Object::Ref(program)),
    ]);
    let descriptor = doc.add(Object::Dict(descriptor));
    let unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), b"1 beginbfrange <20> <7E> <0020> endbfrange")));
    let font = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type1")),
        (b"BaseFont".to_vec(), Object::name("ABCDEF+Example-Bold")),
        (b"FirstChar".to_vec(), Object::Int(32)),
        (b"LastChar".to_vec(), Object::Int(126)),
        (b"Widths".to_vec(), Object::Array(vec![Object::Int(600); 95])),
        (b"FontDescriptor".to_vec(), Object::Ref(descriptor)),
        (b"ToUnicode".to_vec(), Object::Ref(unicode)),
    ])));
    let regular = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type1")),
        (b"BaseFont".to_vec(), Object::name("Helvetica")),
    ])));
    // Mixed styles take the same per-character source/fallback path as the reported region.
    let contents =
        doc.add(Object::Stream(Stream::flate(Dict::new(), b"BT /F2 20 Tf 20 125 Td (Guide) Tj ET BT /F1 20 Tf 20 100 Td (Choice Questions) Tj ET")));
    let page = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(pages)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 300.into(), 150.into()])),
        (
            b"Resources".to_vec(),
            Object::Dict(Dict::from_iter([(
                b"Font".to_vec(),
                Object::Dict(Dict::from_iter([(b"F1".to_vec(), Object::Ref(font)), (b"F2".to_vec(), Object::Ref(regular))])),
            )])),
        ),
        (b"Contents".to_vec(), Object::Ref(contents)),
    ])));
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn call(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON from {name}: {other:?}"),
    }
}

fn render(a: &mut Automation, id: u64) -> Vec<u8> {
    match a.call("page_render", &json!({"doc": id, "page": 1, "dpi": 144})).unwrap().remove(0) {
        Content::Png { data, .. } => data,
        other => panic!("expected PNG: {other:?}"),
    }
}

fn check_replacement(a: &mut Automation, id: u64) -> Vec<u8> {
    let lines = a.session().get(DocId(id)).unwrap().text_lines(0);
    let png = render(a, id);
    let mut reader = png::Decoder::new(std::io::Cursor::new(&png)).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let image = reader.next_frame(&mut pixels).unwrap();
    let channels = image.color_type.samples();
    assert!(channels >= 3);
    // The subset has no space glyph; the writer preserves its gap with positioning.
    assert_eq!(lines.iter().flat_map(|l| l.text.chars()).filter(|ch| !ch.is_whitespace()).collect::<String>(), "GuideChoiceProblems");
    for ch in ['P', 'b'] {
        let line = lines.iter().find(|l| l.text.contains(ch)).unwrap();
        assert_ne!(line.font, "F1", "{ch} must not use a missing subset outline");
        assert!(line.bold, "the replacement retains the source weight");
        let [left, bottom, right, top] = line.rect;
        let dark = ((150.0 - top).max(0.0) as usize * 2..((150.0 - bottom).ceil() as usize * 2).min(image.height as usize))
            .flat_map(|y| (left.max(0.0) as usize * 2..(right.ceil() as usize * 2).min(image.width as usize)).map(move |x| (x, y)))
            .filter(|(x, y)| pixels[(y * image.width as usize + x) * channels..][..3].iter().any(|v| *v < 128))
            .count();
        assert!(dark > 10, "{ch} has visible ink, not just a Unicode mapping");
    }
    assert!(lines.iter().any(|l| l.font == "F1" && l.text.contains('r')), "surviving source glyphs keep their own font");
    png
}

#[test]
fn typed_letters_absent_from_a_type1_subset_survive_commit_history_and_save() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-type1-subset-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("source.pdf"), fixture(false)).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = call(&mut a, "doc_open", json!({"path": "source.pdf"}))["doc"].as_u64().unwrap();
    let before = render(&mut a, id);
    call(&mut a, "text_paragraphs", json!({"doc": id, "page": 1}));
    call(&mut a, "text_edit", json!({"doc": id, "page": 1, "paragraph": 1, "text": "Guide\nChoice Problems"}));
    let after = check_replacement(&mut a, id);
    assert_ne!(after, before);
    call(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(render(&mut a, id), before);
    call(&mut a, "edit_redo", json!({"doc": id}));
    assert_eq!(check_replacement(&mut a, id), after);
    call(&mut a, "doc_save", json!({"doc": id, "path": "saved.pdf"}));
    let reopened = call(&mut a, "doc_open", json!({"path": "saved.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(check_replacement(&mut a, reopened), after);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn type1_charset_respects_custom_builtin_glyph_names() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-type1-builtin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("source.pdf"), fixture(true)).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = call(&mut a, "doc_open", json!({"path": "source.pdf"}))["doc"].as_u64().unwrap();
    call(&mut a, "text_paragraphs", json!({"doc": id, "page": 1}));
    call(&mut a, "text_edit", json!({"doc": id, "page": 1, "paragraph": 1, "text": "Guide\nChoice P"}));
    let lines = a.session().get(DocId(id)).unwrap().text_lines(0);
    assert_eq!(lines.iter().flat_map(|l| l.text.chars()).filter(|ch| !ch.is_whitespace()).collect::<String>(), "GuideChoiceP");
    let line = lines.iter().find(|l| l.text.contains('P')).unwrap();
    assert_eq!(line.font, "F1", "code 80 uses the present Q glyph, not StandardEncoding's absent P");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cached_type1_encoding_keeps_each_descriptors_charset_and_replaced_program() {
    let mut doc = Document::new_empty();
    let program = doc.add(Object::Stream(subset_program(true)));
    let descriptor = doc
        .add(Object::Dict(Dict::from_iter([(b"FontFile".to_vec(), Object::Ref(program)), (b"CharSet".to_vec(), PdfString::literal(b"/Q").into())])));
    let unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), b"1 beginbfchar <50> <0050> endbfchar")));
    let mut font = Dict::from_iter([
        (b"Subtype".to_vec(), Object::name("Type1")),
        (b"BaseFont".to_vec(), Object::name("ABCDEF+Example-Bold")),
        (b"FirstChar".to_vec(), Object::Int(32)),
        (b"Widths".to_vec(), Object::Array(vec![Object::Int(600); 95])),
        (b"FontDescriptor".to_vec(), Object::Ref(descriptor)),
        (b"ToUnicode".to_vec(), Object::Ref(unicode)),
    ]);
    let present = || Some(b"P".as_slice());
    assert_eq!(pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &font).encode("P").as_deref(), present());
    // Reuse the program through a distinct descriptor: CharSet is not part of the program cache.
    let other = doc
        .add(Object::Dict(Dict::from_iter([(b"FontFile".to_vec(), Object::Ref(program)), (b"CharSet".to_vec(), PdfString::literal(b"/P").into())])));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(other));
    assert_eq!(pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &font).encode("P"), None, "the cached encoding still names Q");
    font.set(b"FontDescriptor".to_vec(), Object::Ref(descriptor));
    assert_eq!(pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &font).encode("P").as_deref(), present());
    doc.set(program, Object::Stream(subset_program(false)));
    assert_eq!(pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &font).encode("P"), None, "the replacement program has no P glyph");
}
