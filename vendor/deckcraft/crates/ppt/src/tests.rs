//! End-to-end tests over `.ppt` files built here from records (never files made by PowerPoint).
#![allow(clippy::unwrap_used)]

use deckcraft_model::text::{Align, Bullet, RunKind};
use deckcraft_model::{Background, Fill, PhType, Presentation, Rgba, ShapeKind, Slide};

use crate::doc::rt;
use crate::draw::prop;
use crate::record::build::{atom, container};
use crate::text::tests::{cf_bytes, pf_bytes};

fn le32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn fopt(entries: &[(u16, u32)]) -> Vec<u8> {
    let mut b = vec![];
    for (id, op) in entries {
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&op.to_le_bytes());
    }
    atom(rt::FOPT, entries.len() as u16, &b)
}

fn fsp(shape_type: u16, spid: u32, flags: u32) -> Vec<u8> {
    atom(rt::FSP, shape_type, &le32(&[spid, flags]))
}

/// A client anchor in master units: top, left, right, bottom.
fn anchor(t: i16, l: i16, r: i16, b: i16) -> Vec<u8> {
    atom(rt::CLIENT_ANCHOR, 0, &[t, l, r, b].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>())
}

fn text_box(text_type: u32, text: &str, extra: &[Vec<u8>]) -> Vec<u8> {
    let mut kids = vec![atom(rt::TEXT_HEADER, 0, &text_type.to_le_bytes()), atom(rt::TEXT_CHARS, 0, &utf16(text))];
    kids.extend_from_slice(extra);
    container(rt::CLIENT_TEXTBOX, 0, &kids)
}

fn placeholder(kind: u8) -> Vec<u8> {
    container(rt::CLIENT_DATA, 0, &[atom(rt::PLACEHOLDER_ATOM, 0, &[0, 0, 0, 0, kind, 0, 0, 0])])
}

fn drawing(background: Option<Vec<u8>>, shapes: &[Vec<u8>]) -> Vec<u8> {
    let patriarch = container(rt::SP_CONTAINER, 0, &[atom(rt::FSPGR, 1, &[0; 16]), fsp(0, 1024, 0x5)]);
    let mut tree = vec![patriarch];
    tree.extend_from_slice(shapes);
    let mut dg = vec![container(rt::SPGR_CONTAINER, 0, &tree)];
    dg.extend(background);
    container(rt::DRAWING, 0, &[container(rt::DG_CONTAINER, 0, &dg)])
}

fn solid_background(rgb: u32) -> Vec<u8> {
    container(rt::SP_CONTAINER, 0, &[fsp(1, 1025, 0x400 | 0x800), fopt(&[(prop::FILL_COLOR, rgb), (prop::FILL_BOOLS, 0x0010_0010)])])
}

fn slide_atom(geom: u32, master_id: u32, flags: u16) -> Vec<u8> {
    let mut b = geom.to_le_bytes().to_vec();
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&le32(&[master_id, 0]));
    b.extend_from_slice(&flags.to_le_bytes());
    b.extend_from_slice(&[0; 2]);
    atom(rt::SLIDE_ATOM, 0, &b)
}

/// A master text style with one level: a paragraph and a character exception.
fn master_style(instance: u16, pf: &[u8], cf: &[u8]) -> Vec<u8> {
    atom(rt::TEXT_MASTER_STYLE, instance, &[&1u16.to_le_bytes()[..], pf, cf].concat())
}

const MASTER_ID: u32 = 0x8000_0001;
const SLIDE_ID: u32 = 256;
const TITLE_MASTER_ID: u32 = 0x8000_0002;

fn master() -> Vec<u8> {
    master_with(&[])
}

/// The test master with `shapes` after its decoration.
fn master_with(shapes: &[Vec<u8>]) -> Vec<u8> {
    // Background, text, shadow, title, fill, accent, hyperlink, followed hyperlink.
    let scheme: Vec<u8> = [0xFFFFFFu32, 0x000000, 0x808080, 0x000099, 0x00CC00, 0x0000FF, 0x00FF00, 0xFF0000]
        .iter()
        .flat_map(|c| [(c >> 16) as u8, (c >> 8) as u8, *c as u8, 0])
        .collect();
    let decoration = container(
        rt::SP_CONTAINER,
        0,
        &[
            fsp(1, 1026, 0xA00),
            fopt(&[(prop::FILL_COLOR, 0x0800_0004), (prop::FILL_BOOLS, 0x0010_0010), (prop::LINE_BOOLS, 0x0008_0000)]),
            anchor(0, 0, 5760, 400),
        ],
    );
    container(
        rt::MAIN_MASTER,
        0,
        &[
            slide_atom(1, 0, 0),
            atom(rt::COLOR_SCHEME, 1, &scheme),
            master_style(0, &pf_bytes(1, false), &cf_bytes(true, 0, 44, [0x11, 0x22, 0x33])),
            master_style(1, &pf_bytes(0, true), &cf_bytes(false, 0, 28, [0, 0, 0])),
            drawing(Some(solid_background(0x0000_00FF)), &[&[decoration][..], shapes].concat()),
        ],
    )
}

fn slide() -> Vec<u8> {
    let title = container(
        rt::SP_CONTAINER,
        0,
        &[fsp(1, 2049, 0xA00), fopt(&[(prop::ANCHOR_TEXT, 1)]), anchor(400, 500, 5260, 1200), placeholder(0x0D), text_box(0, "Hello\rWorld", &[])],
    );
    let picture = container(rt::SP_CONTAINER, 0, &[fsp(75, 2050, 0xA00), fopt(&[(prop::PIB, 1)]), anchor(1600, 800, 2400, 2400)]);
    let group = container(
        rt::SPGR_CONTAINER,
        0,
        &[
            container(rt::SP_CONTAINER, 0, &[atom(rt::FSPGR, 1, &le32(&[0, 0, 1000, 1000])), fsp(0, 2051, 0x201), anchor(1600, 3000, 4000, 3200)]),
            container(
                rt::SP_CONTAINER,
                0,
                &[
                    fsp(3, 2052, 0x2),
                    fopt(&[(prop::FILL_COLOR, 0x0000_FF00), (prop::LINE_WIDTH, 25400)]),
                    atom(rt::CHILD_ANCHOR, 0, &le32(&[500, 0, 1000, 500])),
                ],
            ),
        ],
    );
    // "Page *": the star is a slide number field. The first four characters are bold.
    let mut style = vec![];
    style.extend_from_slice(&7u32.to_le_bytes());
    style.extend_from_slice(&0u16.to_le_bytes());
    style.extend_from_slice(&pf_bytes(2, false));
    for (n, bold) in [(4u32, true), (3, false)] {
        style.extend_from_slice(&n.to_le_bytes());
        style.extend_from_slice(&cf_bytes(bold, 1, 12, [0xAB, 0xCD, 0xEF]));
    }
    let footer = container(
        rt::SP_CONTAINER,
        0,
        &[
            fsp(202, 2053, 0xA00),
            anchor(4000, 500, 2000, 4200),
            text_box(4, "Page *", &[atom(rt::STYLE_TEXT_PROP, 0, &style), atom(rt::SLIDE_NUMBER_MC, 0, &5u32.to_le_bytes())]),
        ],
    );
    // A hidden slide (SlideShowSlideInfoAtom fHidden).
    let mut info = vec![0u8; 16];
    info[10] = 0x4;
    container(
        rt::SLIDE,
        0,
        &[slide_atom(1, MASTER_ID, 0x7), atom(rt::SLIDE_SHOW_SLIDE_INFO, 0, &info), drawing(None, &[title, picture, group, footer])],
    )
}

fn notes() -> Vec<u8> {
    let number = container(rt::SP_CONTAINER, 0, &[fsp(1, 3073, 0xA00), anchor(0, 0, 100, 100), placeholder(0x08), text_box(2, "*", &[])]);
    let body = container(
        rt::SP_CONTAINER,
        0,
        &[fsp(1, 3074, 0xA00), anchor(100, 0, 4000, 4000), placeholder(0x0C), text_box(2, "Say hello\rThen wave", &[])],
    );
    container(rt::NOTES, 0, &[atom(rt::NOTES_ATOM, 0, &le32(&[SLIDE_ID, 0])), drawing(None, &[number, body])])
}

fn png() -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])).write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// The streams of a one-slide presentation, with `edit` choosing the current user's header token.
fn streams(token: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    streams_listing(token, 1)
}

/// The test document with its slide listed `times` times in the slide list.
fn streams_listing(token: u32, times: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    streams_of(token, times, slide(), None)
}

/// The test document with `slide` as its slide and, when given, a title master (persist id 5,
/// slide id `TITLE_MASTER_ID`) listed after the main master.
fn streams_of(token: u32, times: usize, slide: Vec<u8>, title_master: Option<Vec<u8>>) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    streams_with_outline(token, times, slide, title_master, &[])
}

/// As [`streams_of`], with `outline` (text blocks: header, characters, styles) following the
/// slide's persist atom in the slide list.
fn streams_with_outline(token: u32, times: usize, slide: Vec<u8>, title_master: Option<Vec<u8>>, outline: &[Vec<u8>]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    streams_with_document(token, times, master(), slide, title_master, outline, &[])
}

/// As [`streams_with_outline`], with `master` as the main master and `extra` records at the end of
/// the document container.
fn streams_with_document(
    token: u32,
    times: usize,
    master: Vec<u8>,
    slide: Vec<u8>,
    title_master: Option<Vec<u8>>,
    outline: &[Vec<u8>],
    extra: &[Vec<u8>],
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pictures = atom(0xF01E, 0x6E0, &[&[0u8; 17][..], &png()].concat());
    let mut fbse = vec![0u8; 36];
    fbse[0] = 6;
    fbse[1] = 6;
    fbse[20..24].copy_from_slice(&(pictures.len() as u32).to_le_bytes());
    let bstore = container(rt::BSTORE, 1, &[atom(rt::FBSE, 6, &fbse)]);
    let mut font = utf16("Arial");
    font.resize(68, 0);
    let mut wing = utf16("Wingdings");
    wing.resize(68, 0);
    let mut doc_atom = le32(&[5760, 4320, 4320, 5760, 1, 2, 0, 0]);
    doc_atom.extend_from_slice(&1u16.to_le_bytes());
    doc_atom.resize(40, 0);
    let persist = |id: u32| {
        atom(
            rt::SLIDE_PERSIST_ATOM,
            0,
            &le32(&[
                id,
                0,
                0,
                if id == 2 {
                    MASTER_ID
                } else if id == 5 {
                    TITLE_MASTER_ID
                } else if id == 3 {
                    SLIDE_ID
                } else {
                    SLIDE_ID + 1
                },
                0,
            ]),
        )
    };
    let parts = [
        atom(rt::DOCUMENT_ATOM, 0, &doc_atom),
        container(rt::ENVIRONMENT, 0, &[container(rt::FONT_COLLECTION, 0, &[atom(rt::FONT_ENTITY, 0, &font), atom(rt::FONT_ENTITY, 1, &wing)])]),
        container(rt::DRAWING_GROUP, 0, &[container(rt::DGG_CONTAINER, 0, &[bstore])]),
        container(rt::SLIDE_LIST_WITH_TEXT, 1, &if title_master.is_some() { vec![persist(2), persist(5)] } else { vec![persist(2)] }),
        container(rt::SLIDE_LIST_WITH_TEXT, 0, &[vec![persist(3); times], outline.to_vec()].concat()),
        container(rt::SLIDE_LIST_WITH_TEXT, 2, &[persist(4)]),
    ];
    let document = container(rt::DOCUMENT, 0, &[&parts[..], extra].concat());
    let mut stream = vec![];
    let mut offsets = vec![];
    let parts: Vec<Vec<u8>> = [document, master, slide, notes()].into_iter().chain(title_master).collect();
    for part in &parts {
        offsets.push(stream.len() as u32);
        stream.extend_from_slice(part);
    }
    let dir_at = stream.len() as u32;
    let mut dir = vec![1u32 | ((parts.len() as u32) << 20)];
    dir.extend_from_slice(&offsets);
    stream.extend_from_slice(&atom(rt::PERSIST_DIRECTORY, 0, &le32(&dir)));
    let edit_at = stream.len() as u32;
    let mut edit = le32(&[0, 0x0300_03F4, 0, dir_at, 1, 5]);
    edit.extend_from_slice(&[0; 4]);
    stream.extend_from_slice(&atom(rt::USER_EDIT, 0, &edit));
    let current_user = atom(rt::CURRENT_USER, 0, &le32(&[0x14, token, edit_at, 0, 0]));
    (current_user, stream, pictures)
}

fn file_with(token: u32) -> Vec<u8> {
    let (cu, doc, pics) = streams(token);
    crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])
}

fn file() -> Vec<u8> {
    file_with(0xE391_C05F)
}

#[test]
fn imports_a_presentation() {
    let bytes = file();
    assert!(crate::sniff(&bytes));
    let p = crate::import(&bytes).unwrap();
    assert_eq!((p.slide_size.width, p.slide_size.height), (720.0, 540.0));
    assert_eq!(p.slides.len(), 1);
    let s = &p.slides[0];
    assert!(s.hidden);
    assert_eq!(s.notes.text(), "Say hello\nThen wave");
    check_title_placeholder(s);
    check_picture(&p, s);
    check_group(s);
    check_text_box(s);
    check_master(&p, s);
}

fn check_title_placeholder(s: &Slide) {
    // The title placeholder: its box, its text and the master's title style.
    let title = &s.shapes[0];
    assert_eq!(title.ph_type(), Some(PhType::Title));
    let x = title.xfrm.unwrap();
    assert_eq!((x.x, x.y, x.w, x.h), (62.5, 50.0, 595.0, 100.0));
    assert_eq!(title.fill, Some(Fill::None));
    let t = title.text.as_ref().unwrap();
    assert_eq!(t.text(), "Hello\nWorld");
    assert_eq!(t.body.anchor, Some(deckcraft_model::text::Anchor::Middle));
    let p0 = &t.paragraphs[0];
    assert_eq!(p0.props.align, Some(Align::Center));
    assert_eq!(p0.props.bullet, Some(Bullet::None));
    let r = &p0.runs[0].props;
    assert_eq!((r.size, r.bold, r.font.as_deref()), (Some(44.0), Some(true), Some("Arial")));
    assert_eq!(r.fill, Some(Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0x11, 0x22, 0x33)))));
    assert_eq!(s.title(), "Hello\nWorld");
}

fn check_picture(p: &Presentation, s: &Slide) {
    // The picture.
    let ShapeKind::Picture { fill } = &s.shapes[1].kind else { panic!("not a picture: {:?}", s.shapes[1].kind) };
    let media = p.media.iter().find(|m| m.id == fill.media).unwrap();
    assert_eq!(media.content_type, "image/png");
    assert!(media.data.starts_with(b"\x89PNG"));
}

fn check_group(s: &Slide) {
    // The group: children in slide points, the child space the group's own box.
    let g = &s.shapes[2];
    let ShapeKind::Group { children, child } = &g.kind else { panic!("not a group") };
    let gx = g.xfrm.unwrap();
    assert_eq!((gx.x, gx.y, gx.w, gx.h), (375.0, 200.0, 125.0, 200.0));
    assert_eq!((child.x, child.y, child.w, child.h), (gx.x, gx.y, gx.w, gx.h));
    let e = children[0].xfrm.unwrap();
    assert_eq!((e.x, e.y, e.w, e.h, e.flip_h), (437.5, 200.0, 62.5, 100.0, false));
    assert_eq!(children[0].geom.preset_name(), Some("ellipse"));
    assert_eq!(children[0].fill, Some(Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0, 0xFF, 0)))));
    assert_eq!(children[0].line.as_ref().and_then(|l| l.width), Some(2.0));
}

fn check_text_box(s: &Slide) {
    // The text box: runs split at the format change, the star a slide number field.
    let tb = s.shapes[3].text.as_ref().unwrap();
    let runs = &tb.paragraphs[0].runs;
    assert_eq!(tb.paragraphs[0].props.align, Some(Align::Right));
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert_eq!((runs[0].text.as_str(), runs[0].props.bold), ("Page", Some(true)));
    assert_eq!((runs[1].text.as_str(), runs[1].props.bold), (" ", Some(false)));
    assert_eq!(runs[2].kind, RunKind::Field { field: "slidenum".into() });
    assert_eq!(runs[0].props.font.as_deref(), Some("Wingdings"));
    assert_eq!(s.shapes[3].fill, Some(Fill::None));
}

fn check_master(p: &Presentation, s: &Slide) {
    // The master: its background, colours and decoration.
    let m = &p.masters[0];
    assert_eq!(m.background, Some(Background::Fill { fill: Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0xFF, 0, 0))) }));
    assert_eq!(m.theme.colors.get(deckcraft_model::SchemeSlot::Accent1), Rgba::rgb(0, 0xCC, 0));
    assert!(m.shapes.iter().any(|sh| sh.ph.is_none() && sh.fill == Some(Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0, 0xCC, 0))))));
    assert_eq!(m.title_style.level(0).and_then(|l| l.run.size), Some(44.0));
    assert!(p.layout(s.layout).is_some());
}

/// A slide listed many times is one slide: a hostile file can't make the importer convert one
/// big drawing again for every listing (180k listings of a 100k-shape slide fit in ~10 MB).
#[test]
fn a_slide_listed_again_is_imported_once() {
    let (cu, doc, pics) = streams_listing(0xE391_C05F, 3);
    let bytes = crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)]);
    let p = crate::import(&bytes).unwrap();
    assert_eq!(p.slides.len(), 1);
    assert_eq!(
        p.slides[0].notes.text(),
        "Say hello
Then wave"
    );
}

/// Text with very many formatting runs is laid into runs in about linear time. Each lookup used to
/// scan the run starts from the beginning, so the cost grew with runs²: in a debug build these 640k
/// one-character runs (about 6 MB of file) took 75 s that way, and take about 2 s with binary search.
#[test]
fn many_character_runs_are_not_quadratic() {
    const N: usize = 640_000;
    let text: String = (0..N).map(|i| if i % 2 == 0 { 'a' } else { '\u{0B}' }).collect();
    let mut style = vec![];
    style.extend_from_slice(&(N as u32 + 1).to_le_bytes());
    style.extend_from_slice(&0u16.to_le_bytes());
    style.extend_from_slice(&0u32.to_le_bytes()); // a paragraph exception with no fields
    for _ in 0..=N {
        style.extend_from_slice(&1u32.to_le_bytes());
        style.extend_from_slice(&0u32.to_le_bytes()); // a character exception with no fields
    }
    let shape = container(
        rt::SP_CONTAINER,
        0,
        &[fsp(202, 2060, 0xA00), anchor(0, 0, 1000, 1000), text_box(1, &text, &[atom(rt::STYLE_TEXT_PROP, 0, &style)])],
    );
    let slide = container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[shape])]);
    let (cu, doc, pics) = streams_of(0xE391_C05F, 1, slide, None);
    let bytes = crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)]);
    let t = std::time::Instant::now();
    let p = crate::import(&bytes).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(20), "{:?}", t.elapsed());
    let runs = &p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs[0].runs;
    assert_eq!(runs.iter().filter(|r| r.kind == RunKind::Break).count(), N / 2);
}

/// A slide naming a master that doesn't exist falls back to the first main master, every time. The
/// fallback used to be the lowest index in a HashMap where a title master ties with its main
/// master, so the slide took the "Title Master" layout on some opens and not on others.
#[test]
fn a_slide_with_an_unknown_master_falls_back_the_same_way_every_time() {
    let title_master = container(rt::SLIDE, 0, &[slide_atom(0, MASTER_ID, 0x7), drawing(None, &[])]);
    let slide = container(rt::SLIDE, 0, &[slide_atom(1, 0x8000_0099, 0x7), drawing(None, &[])]);
    let (cu, doc, pics) = streams_of(0xE391_C05F, 1, slide, Some(title_master));
    let bytes = crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)]);
    for _ in 0..30 {
        let p = crate::import(&bytes).unwrap();
        assert_eq!(p.masters[0].layouts.iter().filter(|l| l.name == "Title Master").count(), 1, "the title master became a layout");
        let (_, layout) = p.layout(p.slides[0].layout).unwrap();
        assert_ne!(layout.name, "Title Master");
    }
}

/// A damaged Current User record doesn't make a PowerPoint 97 file "too old": the newest edit is
/// found by scanning the document stream instead. A file with no edit record at all, behind a
/// token that isn't PowerPoint 97's, is still reported as too old.
#[test]
fn a_damaged_current_user_falls_back_to_the_scan() {
    let (cu, doc, pics) = streams(0xE391_C05F);
    let open = |cu: &[u8], doc: &[u8]| crate::import(&crate::cfb::build(&[("Current User", cu), ("PowerPoint Document", doc), ("Pictures", &pics)]));
    // Truncated after the header and two bytes, so the token can't be read.
    let truncated = atom(rt::CURRENT_USER, 0, &[0x14, 0]);
    assert_eq!(open(&truncated, &doc).unwrap().slides.len(), 1);
    // A corrupted token.
    let corrupted = atom(rt::CURRENT_USER, 0, &le32(&[0x14, 0xDEAD_BEEF, 0, 0, 0]));
    assert_eq!(open(&corrupted, &doc).unwrap().slides.len(), 1);
    assert!(matches!(open(&corrupted, &atom(rt::DOCUMENT_ATOM, 0, &[0; 40])), Err(crate::PptError::TooOld)));
    // An intact token that points at no edit still gets the scan, and a stream without one is damaged.
    assert!(matches!(open(&cu, &atom(rt::DOCUMENT_ATOM, 0, &[0; 40])), Err(crate::PptError::Damaged(_))));
}

/// Persist directories nested inside one another are not each read to the end of the stream. A chain
/// of edits, each naming a directory header 8 bytes after the last one, made every edit reparse the
/// rest of the stream (4095 edits × the filler): about 48 s for this 64 MiB stream in a debug build.
/// Within the shared budget it takes milliseconds, and the file is reported as damaged.
#[test]
fn nested_persist_directories_are_read_within_the_stream() {
    const FILLER: usize = 1 << 26;
    const EDITS: u32 = 4095;
    let (_, mut stream, _) = streams(0xE391_C05F);
    let mut prev = stream.len() as u32 - 36; // the test document's own UserEditAtom (8 + 28 bytes)
    let base = stream.len() as u32;
    for _ in 0..EDITS {
        // A PersistDirectoryAtom header whose length runs past the end (clipped to the stream).
        stream.extend_from_slice(&0u16.to_le_bytes());
        stream.extend_from_slice(&rt::PERSIST_DIRECTORY.to_le_bytes());
        stream.extend_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    }
    stream.resize(stream.len() + FILLER, 0);
    for k in 0..EDITS {
        let at = stream.len() as u32;
        let mut edit = le32(&[0, 0x0300_03F4, prev, base + 8 * k, 1, 5]);
        edit.extend_from_slice(&[0; 4]);
        stream.extend_from_slice(&atom(rt::USER_EDIT, 0, &edit));
        prev = at;
    }
    let cu = atom(rt::CURRENT_USER, 0, &le32(&[0x14, 0xE391_C05F, prev, 0, 0]));
    // The streams are read directly: the test file builder can't hold a stream this size.
    let t = std::time::Instant::now();
    let _ = crate::doc::read(&cu, &stream);
    assert!(t.elapsed() < std::time::Duration::from_secs(20), "{:?}", t.elapsed());
}

#[test]
fn an_encrypted_presentation_says_so() {
    assert!(matches!(crate::import(&file_with(0xF3D1_C4DF)), Err(crate::PptError::Encrypted)));
}

#[test]
fn other_files_are_not_ppt() {
    assert!(!crate::sniff(b"PK\x03\x04"));
    assert!(!crate::sniff(&crate::cfb::build(&[("WordDocument", b"x")])));
    assert!(matches!(crate::import(b"not a presentation"), Err(crate::PptError::NotPpt)));
    assert!(matches!(crate::import(&crate::cfb::build(&[("Workbook", b"x")])), Err(crate::PptError::NotPpt)));
}

/// Damaged input never panics, loops or allocates without bound: every 4-byte word of the
/// document stream set to hostile values, and truncations of it.
#[test]
fn hostile_documents_never_panic() {
    let (cu, doc, pics) = streams(0xE391_C05F);
    for at in (0..doc.len().saturating_sub(4)).step_by(3) {
        for v in [0u32, 1, 0x7FFF_FFFF, 0xFFFF_FFFF, 0x0000_FFFF, 0xF00F_000F] {
            let mut d = doc.clone();
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
            let f = crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &d), ("Pictures", &pics)]);
            let _ = crate::import(&f);
        }
    }
    for cut in (0..doc.len()).step_by(7) {
        let f = crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc[..cut]), ("Pictures", &pics[..cut.min(pics.len())])]);
        let _ = crate::import(&f);
    }
}

/// File > Open on a `.ppt` gives an untitled-to-disk copy: Save asks where to save instead of
/// writing DeckCraft's own format under the `.ppt` name.
#[test]
fn the_app_opens_a_ppt_and_save_asks_for_a_new_name() {
    let dir = std::env::temp_dir().join(format!("deckcraft-ppt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Lecture.ppt");
    std::fs::write(&path, file()).unwrap();
    let mut s = deckcraft_engine::Session::new();
    let r = s.execute("file.open", &serde_json::json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["slides"], 1);
    let st = s.doc().unwrap();
    assert_eq!(st.path, None);
    assert_eq!(st.title(), "Lecture");
    let saved = s.execute("file.save", &serde_json::json!({})).unwrap();
    assert_eq!(saved["needsPath"], true);
    // Saved as .pptx, it opens again with its slide.
    let out = dir.join("Lecture.pptx");
    s.execute("file.saveAs", &serde_json::json!({"path": out.to_string_lossy()})).unwrap();
    let back = deckcraft_engine::cmd::file::open_presentation("Lecture.pptx", &std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(back.slides.len(), 1);
    assert_eq!(back.slides[0].shapes[0].text.as_ref().unwrap().text(), "Hello\nWorld");
    std::fs::remove_dir_all(&dir).ok();

    // Password-protected: the same message as for a .pptx.
    let err = deckcraft_engine::cmd::file::open_presentation("Locked.ppt", &file_with(0xF3D1_C4DF)).unwrap_err().to_string();
    assert!(err.contains("Locked.ppt is password-protected"), "{err}");
}

/// A slide shape that inherits from a master shape (`hspMaster`) takes it from its own master.
/// Master shapes used to be remembered by shape id alone, so a title master reusing the main
/// master's shape id (here 1026, red instead of the scheme's green) replaced it for every slide.
#[test]
fn a_master_shape_id_is_looked_up_in_the_slides_own_master() {
    let red = container(
        rt::SP_CONTAINER,
        0,
        &[fsp(1, 1026, 0xA00), fopt(&[(prop::FILL_COLOR, 0x0000_00FF), (prop::FILL_BOOLS, 0x0010_0010)]), anchor(0, 0, 100, 100)],
    );
    let title_master = container(rt::SLIDE, 0, &[slide_atom(0, MASTER_ID, 0x7), drawing(None, &[red])]);
    let inheriting = container(rt::SP_CONTAINER, 0, &[fsp(1, 2049, 0xA00), fopt(&[(prop::HSP_MASTER, 1026)]), anchor(0, 0, 100, 100)]);
    let slide = container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[inheriting])]);
    let (cu, doc, pics) = streams_of(0xE391_C05F, 1, slide, Some(title_master));
    let p = crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])).unwrap();
    assert_eq!(p.slides[0].shapes[0].fill, Some(Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0, 0xCC, 0)))));
}

/// A run's shadow bit (TextCFException fontStyle bit 4) gives the run a shadow in the colour
/// scheme's shadow colour (grey in the test scheme). It used to be read and then dropped.
#[test]
fn a_shadowed_run_keeps_its_shadow() {
    let mut style = vec![];
    style.extend_from_slice(&7u32.to_le_bytes());
    style.extend_from_slice(&0u16.to_le_bytes());
    style.extend_from_slice(&pf_bytes(0, false));
    for (n, shadow) in [(3u32, true), (4, false)] {
        style.extend_from_slice(&n.to_le_bytes());
        style.extend_from_slice(&0x10u32.to_le_bytes()); // mask: shadow
        style.extend_from_slice(&(if shadow { 0x10u16 } else { 0 }).to_le_bytes());
    }
    let tb = container(
        rt::SP_CONTAINER,
        0,
        &[fsp(202, 2049, 0xA00), anchor(0, 0, 1000, 1000), text_box(4, "Hi you", &[atom(rt::STYLE_TEXT_PROP, 0, &style)])],
    );
    let slide = container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[tb])]);
    let (cu, doc, pics) = streams_of(0xE391_C05F, 1, slide, None);
    let p = crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])).unwrap();
    let runs = &p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs[0].runs;
    assert_eq!(runs[0].text, "Hi ");
    let shadow = runs[0].props.shadow.as_ref().expect("the first run is shadowed");
    assert_eq!(shadow.color, deckcraft_model::ColorRef::rgb(Rgba::rgb(0x80, 0x80, 0x80)));
    assert!(runs[1].props.shadow.is_none());
}

/// `tests/fixtures/old-talk.pps`: a one-slide `.pps` built here from records, for tests in other
/// crates that need real 97–2003 bytes (`ui-egui`'s show-on-open test). Run this test with
/// `DECKCRAFT_WRITE_PPT_FIXTURE=1` to rebuild the file after changing the builders.
#[test]
fn the_pps_fixture_is_a_built_presentation() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/old-talk.pps");
    if std::env::var_os("DECKCRAFT_WRITE_PPT_FIXTURE").is_some() {
        let title =
            container(rt::SP_CONTAINER, 0, &[fsp(1, 2049, 0xA00), anchor(400, 500, 5260, 1200), placeholder(0x0D), text_box(0, "Old talk", &[])]);
        let slide = container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[title])]);
        let (cu, doc, pics) = streams_of(0xE391_C05F, 1, slide, None);
        std::fs::write(path, crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])).unwrap();
    }
    let p = crate::import(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(p.slides.len(), 1);
    assert!(!p.slides[0].hidden);
    let title = p.slides[0].shapes[0].text.as_ref().unwrap();
    assert_eq!(title.paragraphs[0].runs[0].text, "Old talk");
}

/// A placeholder whose client text box holds only an `OutlineTextRefAtom` takes its text and
/// formatting from that block of the slide list ([MS-PPT] OfficeArtClientTextbox allows the
/// reference alone). A text box that also has a header but no characters falls back the same way.
#[test]
fn placeholder_text_can_come_from_the_slide_list() {
    let mut style = vec![];
    style.extend_from_slice(&7u32.to_le_bytes()); // the text and its closing paragraph mark
    style.extend_from_slice(&0u16.to_le_bytes());
    style.extend_from_slice(&pf_bytes(1, false));
    for (n, bold) in [(2u32, true), (5, false)] {
        style.extend_from_slice(&n.to_le_bytes());
        style.extend_from_slice(&cf_bytes(bold, 0, 40, [0x12, 0x34, 0x56]));
    }
    let outline = [
        atom(rt::TEXT_HEADER, 0, &0u32.to_le_bytes()),
        atom(rt::TEXT_CHARS, 0, &utf16("Hi all")),
        atom(rt::STYLE_TEXT_PROP, 0, &style),
        atom(rt::TEXT_HEADER, 0, &1u32.to_le_bytes()),
        atom(rt::TEXT_CHARS, 0, &utf16("Second block")),
    ];
    let reference = |n: u32| atom(rt::OUTLINE_TEXT_REF, 0, &n.to_le_bytes());
    let only_ref = container(
        rt::SP_CONTAINER,
        0,
        &[fsp(1, 2049, 0xA00), anchor(400, 500, 5260, 1200), placeholder(0x0D), container(rt::CLIENT_TEXTBOX, 0, &[reference(0)])],
    );
    let header_and_ref = container(
        rt::SP_CONTAINER,
        0,
        &[
            fsp(1, 2050, 0xA00),
            anchor(1300, 500, 5260, 3000),
            placeholder(0x0E),
            container(rt::CLIENT_TEXTBOX, 0, &[atom(rt::TEXT_HEADER, 0, &1u32.to_le_bytes()), reference(1)]),
        ],
    );
    let slide = container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[only_ref, header_and_ref])]);
    let (cu, doc, pics) = streams_with_outline(0xE391_C05F, 1, slide, None, &outline);
    let p = crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])).unwrap();
    let shapes = &p.slides[0].shapes;
    let title = shapes[0].text.as_ref().expect("the title has the outline text");
    let runs = &title.paragraphs[0].runs;
    assert_eq!((runs[0].text.as_str(), runs[0].props.bold), ("Hi", Some(true)));
    assert_eq!((runs[1].text.as_str(), runs[1].props.bold), (" all", Some(false)));
    assert_eq!(runs[0].props.size, Some(40.0));
    assert_eq!(title.paragraphs[0].props.align, Some(Align::Center));
    let body = shapes[1].text.as_ref().expect("the body has the second block");
    assert_eq!(body.paragraphs[0].runs[0].text, "Second block");
}

/// The newest edit wins: a later edit's persist directory that gives the slide's persist id a new
/// slide replaces the old one.
#[test]
fn a_later_edit_replaces_a_slide() {
    let (_, mut stream, pics) = streams(0xE391_C05F);
    let first_edit = stream.len() as u32 - 36; // the test document's UserEditAtom (8 + 28 bytes)
    let title = container(rt::SP_CONTAINER, 0, &[fsp(1, 2049, 0xA00), anchor(400, 500, 5260, 1200), placeholder(0x0D), text_box(0, "Edited", &[])]);
    let slide_at = stream.len() as u32;
    stream.extend_from_slice(&container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[title])]));
    // Persist id 3 (the slide) only.
    let dir_at = stream.len() as u32;
    stream.extend_from_slice(&atom(rt::PERSIST_DIRECTORY, 0, &le32(&[3 | (1 << 20), slide_at])));
    let edit_at = stream.len() as u32;
    let mut edit = le32(&[0, 0x0300_03F4, first_edit, dir_at, 1, 5]);
    edit.extend_from_slice(&[0; 4]);
    stream.extend_from_slice(&atom(rt::USER_EDIT, 0, &edit));
    let cu = atom(rt::CURRENT_USER, 0, &le32(&[0x14, 0xE391_C05F, edit_at, 0, 0]));
    let p = crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &stream), ("Pictures", &pics)])).unwrap();
    assert_eq!(p.slides.len(), 1);
    assert_eq!(p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs[0].runs[0].text, "Edited");
    // The master, from the first edit's directory, is still there.
    assert_eq!(p.masters.len(), 1);
}

/// An edit whose "previous edit" points back at itself cuts the older edits off the chain, and
/// with them the document, master and notes (#1137 review: such a file opened as an empty deck).
/// Every edit in the stream is read instead, the newest still winning.
#[test]
fn a_broken_edit_chain_is_recovered_from_every_edit() {
    let (_, mut stream, pics) = streams(0xE391_C05F);
    let title = container(rt::SP_CONTAINER, 0, &[fsp(1, 2049, 0xA00), anchor(400, 500, 5260, 1200), placeholder(0x0D), text_box(0, "Edited", &[])]);
    let slide_at = stream.len() as u32;
    stream.extend_from_slice(&container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[title])]));
    let dir_at = stream.len() as u32;
    stream.extend_from_slice(&atom(rt::PERSIST_DIRECTORY, 0, &le32(&[3 | (1 << 20), slide_at])));
    let edit_at = stream.len() as u32;
    let mut edit = le32(&[0, 0x0300_03F4, edit_at, dir_at, 1, 5]);
    edit.extend_from_slice(&[0; 4]);
    stream.extend_from_slice(&atom(rt::USER_EDIT, 0, &edit));
    let cu = atom(rt::CURRENT_USER, 0, &le32(&[0x14, 0xE391_C05F, edit_at, 0, 0]));
    let (p, lost) =
        crate::import_checked(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &stream), ("Pictures", &pics)])).unwrap();
    assert_eq!(p.slides.len(), 1);
    assert_eq!(p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs[0].runs[0].text, "Edited");
    assert_eq!(p.masters.len(), 1);
    assert!(lost.is_empty(), "{lost:?}");
}

/// A slide the list names but whose object is gone is reported as lost, so the app and the CLI
/// say so instead of opening a deck that silently misses it.
#[test]
fn a_slide_without_its_object_is_reported_lost() {
    let (cu, mut stream, pics) = streams(0xE391_C05F);
    // The persist directory (ids 1–4: document, master, slide, notes) ends just before the
    // 36-byte edit; point the slide's entry past the end of the stream.
    let slide_entry = stream.len() - 36 - 8;
    stream[slide_entry..slide_entry + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    let bytes = crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &stream), ("Pictures", &pics)]);
    let (p, lost) = crate::import_checked(&bytes).unwrap();
    assert!(p.slides.is_empty());
    assert_eq!(lost, vec![format!("slide {SLIDE_ID}")]);

    let mut s = deckcraft_engine::Session::new();
    let data = deckcraft_engine::cmd::base64_encode(&bytes);
    let r = s.execute("file.openBytes", &serde_json::json!({"name": "Damaged.ppt", "data": data})).unwrap();
    assert_eq!(r["lostSlides"].as_array().map(Vec::len), Some(1));
    assert!(s.doc().unwrap().title().ends_with("(Repaired)"));
}

/// Two more signs of an encrypted file besides the Current User token: an edit record long enough
/// to hold `encryptSessionPersistIdRef`, and a `CryptSession` object in the persist directory.
#[test]
fn encryption_is_found_in_the_edit_and_the_persist_directory() {
    let open = |doc: &[u8], pics: &[u8]| {
        let cu = atom(rt::CURRENT_USER, 0, &le32(&[0x14, 0xE391_C05F, doc.len() as u32 - 36, 0, 0]));
        crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", doc), ("Pictures", pics)]))
    };
    let (_, stream, pics) = streams(0xE391_C05F);
    assert!(open(&stream, &pics).is_ok());
    // The edit record with a fifth reference: 28 bytes become 32. The Current User points at the
    // new last record, 40 bytes from the end.
    let mut long_edit = stream.clone();
    let edit_at = long_edit.len() - 36;
    long_edit.truncate(edit_at);
    let mut edit = le32(&[0, 0x0300_03F4, 0, 0, 1, 5]);
    edit[12..16].copy_from_slice(&stream[edit_at + 8 + 12..edit_at + 8 + 16]); // offsetPersistDirectory
    edit.extend_from_slice(&[0; 4]);
    edit.extend_from_slice(&6u32.to_le_bytes()); // encryptSessionPersistIdRef
    long_edit.extend_from_slice(&atom(rt::USER_EDIT, 0, &edit));
    let cu = atom(rt::CURRENT_USER, 0, &le32(&[0x14, 0xE391_C05F, edit_at as u32, 0, 0]));
    let r = crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &long_edit), ("Pictures", &pics)]));
    assert!(matches!(r, Err(crate::PptError::Encrypted)), "{:?}", r.err());
    // A CryptSession container given a persist id by a later edit.
    let mut crypt = stream.clone();
    let first_edit = crypt.len() as u32 - 36;
    let session_at = crypt.len() as u32;
    crypt.extend_from_slice(&container(rt::CRYPT_SESSION, 0, &[atom(0x0FFF, 0, &[0; 8])]));
    let dir_at = crypt.len() as u32;
    crypt.extend_from_slice(&atom(rt::PERSIST_DIRECTORY, 0, &le32(&[9 | (1 << 20), session_at])));
    let mut edit = le32(&[0, 0x0300_03F4, first_edit, dir_at, 1, 9]);
    edit.extend_from_slice(&[0; 4]);
    crypt.extend_from_slice(&atom(rt::USER_EDIT, 0, &edit));
    assert!(matches!(open(&crypt, &pics), Err(crate::PptError::Encrypted)));
}

/// A colour scheme record whose fill colour (index 4) is `fill`, the rest as the test master's.
fn scheme_with_fill(fill: [u8; 3]) -> Vec<u8> {
    let mut colors: Vec<u8> = [0xFFFFFFu32, 0x000000, 0x808080, 0x000099, 0, 0x0000FF, 0x00FF00, 0xFF0000]
        .iter()
        .flat_map(|c| [(c >> 16) as u8, (c >> 8) as u8, *c as u8, 0])
        .collect();
    colors[16..19].copy_from_slice(&fill);
    atom(rt::COLOR_SCHEME, 1, &colors)
}

/// A shape filled with the scheme's fill colour.
fn scheme_filled(spid: u32) -> Vec<u8> {
    container(
        rt::SP_CONTAINER,
        0,
        &[fsp(1, spid, 0xA00), fopt(&[(prop::FILL_COLOR, 0x0800_0004), (prop::FILL_BOOLS, 0x0010_0010)]), anchor(0, 0, 100, 100)],
    )
}

/// A title master becomes a layout of its main master, with its own background and shapes, and
/// without the master's shapes when it doesn't follow them. A slide of the title master uses that
/// layout. A slide that follows nothing keeps its own background and scheme and hides the
/// master's shapes.
#[test]
fn title_masters_and_slides_that_dont_follow_the_master() {
    let title_master =
        container(rt::SLIDE, 0, &[slide_atom(0, MASTER_ID, 0x0), drawing(Some(solid_background(0x0000_FF00)), &[scheme_filled(3000)])]);
    let on_title = container(rt::SLIDE, 0, &[slide_atom(0, TITLE_MASTER_ID, 0x7), drawing(None, &[])]);
    let (cu, doc, pics) = streams_of(0xE391_C05F, 1, on_title, Some(title_master));
    let p = crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])).unwrap();
    let (_, layout) = p.layout(p.slides[0].layout).unwrap();
    assert_eq!(layout.name, "Title Master");
    assert_eq!(layout.background, Some(Background::Fill { fill: Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0, 0xFF, 0))) }));
    assert!(!layout.show_master_shapes);
    assert!(layout.shapes.iter().any(|sh| sh.ph.is_none()), "the title master's own shape");
    assert_eq!(p.slides[0].background, None, "the slide follows its master's background");

    let alone = container(
        rt::SLIDE,
        0,
        &[slide_atom(1, MASTER_ID, 0x0), scheme_with_fill([0x11, 0x22, 0xCC]), drawing(Some(solid_background(0x0000_00FF)), &[scheme_filled(2049)])],
    );
    let (cu, doc, pics) = streams_of(0xE391_C05F, 1, alone, None);
    let p = crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])).unwrap();
    let s = &p.slides[0];
    assert_eq!(s.background, Some(Background::Fill { fill: Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0xFF, 0, 0))) }));
    assert!(!s.show_master_shapes);
    assert_eq!(s.shapes[0].fill, Some(Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(0x11, 0x22, 0xCC)))), "the slide's own scheme");
}

fn open_streams((cu, doc, pics): (Vec<u8>, Vec<u8>, Vec<u8>)) -> Presentation {
    crate::import(&crate::cfb::build(&[("Current User", &cu), ("PowerPoint Document", &doc), ("Pictures", &pics)])).unwrap()
}

/// A picture frame that says nothing about its fill or line draws neither (#1137 review): the
/// drawing defaults' fill would cover a picture's transparent parts, or an OLE chart's preview.
#[test]
fn a_picture_frame_has_no_fill_or_line_of_its_own() {
    let p = crate::import(&file()).unwrap();
    let picture = &p.slides[0].shapes[1];
    assert!(matches!(picture.kind, ShapeKind::Picture { .. }));
    assert_eq!(picture.fill, Some(Fill::None));
    assert_eq!(picture.line.as_ref().map(|l| l.fill.clone()), Some(Some(Fill::None)));
}

/// A picture fill whose blip is held in the shape's own `fillBlip` property (complex, with fBid),
/// as table cells with a picture have it (#1137 review), shows that picture.
#[test]
fn a_picture_held_in_the_fill_property_is_read() {
    let blip = atom(0xF01E, 0x6E0, &[&[0u8; 17][..], &png()].concat());
    let mut entries = vec![];
    for (id, op) in [(prop::FILL_TYPE, 3u32), (0xC000 | prop::FILL_BLIP, blip.len() as u32), (prop::FILL_BOOLS, 0x0010_0010)] {
        entries.extend_from_slice(&id.to_le_bytes());
        entries.extend_from_slice(&op.to_le_bytes());
    }
    entries.extend_from_slice(&blip);
    let cell = container(rt::SP_CONTAINER, 0, &[fsp(1, 2049, 0xA00), atom(rt::FOPT, 3, &entries), anchor(400, 500, 2000, 1200)]);
    let slide = container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[cell])]);
    let p = open_streams(streams_of(0xE391_C05F, 1, slide, None));
    let Some(Fill::Picture(fill)) = &p.slides[0].shapes[0].fill else { panic!("{:?}", p.slides[0].shapes[0].fill) };
    let media = p.media.iter().find(|m| m.id == fill.media).unwrap();
    assert!(media.data.starts_with(b"\x89PNG"));
}

/// The headers and footers the document sets for its slides (#1137 review: the date, footer and
/// slide number were missing) become the slide's footer placeholders, placed and styled as the
/// master's are on every layout.
#[test]
fn headers_and_footers_become_footer_placeholders() {
    let cstring = |instance: u16, s: &str| atom(rt::CSTRING, instance, &utf16(s));
    // fHasDate | fHasUserDate | fHasSlideNumber | fHasFooter. The footer string ends in a NUL, as
    // some writers add one: the text stops there.
    let flags: u16 = 0x01 | 0x04 | 0x08 | 0x20;
    let hf = container(
        rt::HEADERS_FOOTERS,
        3,
        &[
            atom(rt::HEADERS_FOOTERS_ATOM, 0, &[&0u16.to_le_bytes()[..], &flags.to_le_bytes()].concat()),
            cstring(0, "26 May 2004"),
            cstring(2, "Workshop\0"),
        ],
    );
    // The master's slide number placeholder: bottom right, right-aligned 10 pt text.
    let mut style = vec![];
    style.extend_from_slice(&2u32.to_le_bytes());
    style.extend_from_slice(&0u16.to_le_bytes());
    style.extend_from_slice(&pf_bytes(2, false));
    style.extend_from_slice(&2u32.to_le_bytes());
    style.extend_from_slice(&cf_bytes(false, 0, 10, [1, 2, 3]));
    let number = container(
        rt::SP_CONTAINER,
        0,
        &[fsp(1, 1030, 0xA00), anchor(4000, 5000, 5700, 4300), placeholder(0x08), text_box(4, "*", &[atom(rt::STYLE_TEXT_PROP, 0, &style)])],
    );
    let p = open_streams(streams_with_document(0xE391_C05F, 1, master_with(&[number]), slide(), None, &[], &[hf]));
    let shapes = &p.slides[0].shapes;
    let of = |k: PhType| shapes.iter().find(|s| s.ph_type() == Some(k)).unwrap_or_else(|| panic!("no {k:?}"));
    assert_eq!(of(PhType::Date).text.as_ref().unwrap().text(), "26 May 2004");
    assert_eq!(of(PhType::Footer).text.as_ref().unwrap().text(), "Workshop");
    let runs = &of(PhType::SlideNum).text.as_ref().unwrap().paragraphs[0].runs;
    assert!(matches!(&runs[0].kind, RunKind::Field { field } if field == "slidenum"));
    assert!(p.header_footer.slide_number && p.header_footer.footer && p.header_footer.date);
    // Every layout's slide number sits where the master's does, with its look.
    let master = &p.masters[0];
    for l in &master.layouts {
        if let Some(n) = l.shapes.iter().find(|s| s.ph_type() == Some(PhType::SlideNum)) {
            let x = n.xfrm.unwrap();
            assert_eq!((x.x, x.y), (5000.0 / 8.0, 4000.0 / 8.0), "{}", l.name);
            let level = n.text.as_ref().unwrap().list_style.level(0).unwrap();
            assert_eq!((level.para.align, level.run.size), (Some(Align::Right), Some(10.0)));
        }
    }
}

/// Numbered paragraphs (#1137 review: drawn with the bullet character) take their numbering from
/// the document's `___PPT9` tag: the slide's outline block of that text type, and the entry the
/// run's `pp9rt` names. The tag's name ends in a NUL here, as some writers add one.
#[test]
fn outline_numbering_from_the_ppt9_tag() {
    // Entry 0: nothing. Entry 1: numbered, "1." (scheme 3), from 4.
    let mut prop9 = vec![0u8; 12];
    prop9.extend_from_slice(&0x0300_0000u32.to_le_bytes());
    prop9.extend_from_slice(&1u16.to_le_bytes());
    prop9.extend_from_slice(&3u16.to_le_bytes());
    prop9.extend_from_slice(&4i16.to_le_bytes());
    prop9.extend_from_slice(&[0; 8]);
    let tag_data = [atom(rt::OUTLINE_TEXT_PROPS_HEADER, 1, &le32(&[SLIDE_ID, 1])), atom(rt::STYLE_TEXT_PROP9, 0, &prop9)];
    let tag = container(
        rt::PROG_BINARY_TAG,
        0,
        &[atom(rt::CSTRING, 0, &utf16("___PPT9\0")), atom(rt::BINARY_TAG_DATA, 0, &container(rt::OUTLINE_TEXT_PROPS9, 0, &tag_data))],
    );
    let list = container(rt::LIST, 0, &[container(rt::PROG_TAGS, 0, &[tag])]);
    // Two bulleted paragraphs: the second's run names entry 1 (pp9rt in fontStyle bits 10–13).
    let pp9rt = |i: u16| [&0x0000_0400u32.to_le_bytes()[..], &(i << 10).to_le_bytes()].concat();
    let mut style = vec![];
    style.extend_from_slice(&8u32.to_le_bytes());
    style.extend_from_slice(&0u16.to_le_bytes());
    style.extend_from_slice(&pf_bytes(0, true));
    for (n, i) in [(4u32, 0u16), (4, 1)] {
        style.extend_from_slice(&n.to_le_bytes());
        style.extend_from_slice(&pp9rt(i));
    }
    let outline = [atom(rt::TEXT_HEADER, 0, &1u32.to_le_bytes()), atom(rt::TEXT_CHARS, 0, &utf16("One\rTwo")), atom(rt::STYLE_TEXT_PROP, 0, &style)];
    let body = container(
        rt::SP_CONTAINER,
        0,
        &[
            fsp(1, 2049, 0xA00),
            anchor(1300, 500, 5260, 3000),
            placeholder(0x0E),
            container(rt::CLIENT_TEXTBOX, 0, &[atom(rt::OUTLINE_TEXT_REF, 0, &0u32.to_le_bytes())]),
        ],
    );
    let slide = container(rt::SLIDE, 0, &[slide_atom(1, MASTER_ID, 0x7), drawing(None, &[body])]);
    let p = open_streams(streams_with_document(0xE391_C05F, 1, master(), slide, None, &outline, &[list]));
    let paras = &p.slides[0].shapes[0].text.as_ref().unwrap().paragraphs;
    assert!(matches!(paras[0].props.bullet, Some(Bullet::Char { .. })), "{:?}", paras[0].props.bullet);
    assert_eq!(paras[1].props.bullet, Some(Bullet::AutoNum { scheme: "arabicPeriod".into(), start_at: 4 }));
}
