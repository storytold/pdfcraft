use std::sync::Arc;

use super::*;

/// A minimal `n`-page document; page `i` shows "Page i+1".
pub(crate) fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn session_with(n: usize) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(n)), None).expect("opens");
    (s, id)
}

#[test]
fn prepared_open_does_not_mutate_the_session_until_commit() {
    fn assert_send<T: Send>() {}
    assert_send::<crate::PreparedDocument>();

    let mut session = Session::new();
    let prepared = session.prepare_open("prepared.pdf", None, Arc::new(fixture(3)), None).expect("prepares");
    assert!(session.get(DocId(1)).is_none());

    let id = session.commit_open(prepared);
    assert_eq!(id, DocId(1));
    assert_eq!(session.get(id).expect("committed document").info.pages.len(), 3);
}

#[test]
fn custom_image_stamp_keeps_displayed_orientation_on_rotated_pages() {
    let colours = [[240, 20, 20, 255], [20, 180, 20, 255], [20, 20, 240, 255], [230, 180, 20, 255]];
    let image = image::RgbaImage::from_fn(80, 40, |x, y| image::Rgba(colours[usize::from(y >= 20) * 2 + usize::from(x >= 40)]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let bytes = Arc::new(png.into_inner());
    for degrees in [0, 90, 180, 270] {
        let (mut session, id) = session_with(1);
        session.apply(id, Edit::RotatePages { pages: vec![0], degrees }).unwrap();
        let at = session.get(id).unwrap().info.pages[0].view_to_user(100.0, 100.0).map(f64::from);
        session
            .apply(
                id,
                Edit::AddCustomStamp {
                    page: 0,
                    rect: [at[0], at[1], at[0], at[1]],
                    name: "Original quadrants".into(),
                    file: MarkFile { name: "quadrants.png".into(), bytes: bytes.clone(), page: 0 },
                    author: "Test".into(),
                },
            )
            .unwrap();
        let check = |session: &Session, points: [(u32, u32); 4]| {
            let doc = session.get(id).unwrap();
            let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
            let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
            assert!(rendered.error.is_none(), "{:?}", rendered.error);
            for ((x, y), colour) in points.into_iter().zip(colours) {
                let offset = ((y * rendered.width + x) * 4) as usize;
                assert_eq!(&rendered.rgba[offset..offset + 4], &colour, "page rotation {degrees}; displayed point ({x}, {y})");
            }
        };
        let points = [(80, 90), (120, 90), (80, 110), (120, 110)];
        check(&session, points);
        let info = session.get(id).unwrap().info.pages[0].clone();
        let rect = session.get(id).unwrap().info.annotations[0].rect;
        let a = info.user_to_view(rect[0], rect[1]);
        let b = info.user_to_view(rect[2], rect[3]);
        let bounds = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
        for (actual, expected) in bounds.into_iter().zip([60.0, 80.0, 140.0, 120.0]) {
            assert!((actual - expected).abs() < 0.0001, "displayed bounds {bounds:?}");
        }
        session.apply(id, Edit::StyleAnnotation { page: 0, index: 0, color: None, opacity: Some(1.0), width: None, endings: None }).unwrap();
        check(&session, points);
        let resized = info.view_rect_to_user([40.0, 70.0, 160.0, 130.0]).map(f64::from);
        session.apply(id, Edit::ResizeAnnotation { page: 0, index: 0, rect: resized }).unwrap();
        let resized_points = [(70, 85), (130, 85), (70, 115), (130, 115)];
        check(&session, resized_points);
        session.undo(id).unwrap();
        check(&session, points);
        session.redo(id).unwrap();
        check(&session, resized_points);
        let from = info.view_to_user(100.0, 100.0);
        let to = info.view_to_user(110.0, 115.0);
        session.apply(id, Edit::MoveAnnotation { page: 0, index: 0, dx: f64::from(to[0] - from[0]), dy: f64::from(to[1] - from[1]) }).unwrap();
        check(&session, [(80, 100), (140, 100), (80, 130), (140, 130)]);
        session.undo(id).unwrap();
        check(&session, resized_points);
        session.redo(id).unwrap();
        check(&session, [(80, 100), (140, 100), (80, 130), (140, 130)]);
    }
}

/// A blank page turned `degrees`, with one annotation placed from displayed coordinates.
fn rotated_blank(degrees: i64) -> (Session, DocId) {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let blank = session.create_blank(200.0, 300.0, 1).unwrap();
    let id = session.open_new("drawing.pdf", blank).unwrap();
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees }).unwrap();
    (session, id)
}

/// The user-space box of a `w` by `h` point rectangle centred at a displayed point.
fn displayed_rect(info: &pdfcraft_render::PageInfo, vx: f64, vy: f64, w: f64, h: f64) -> [f64; 4] {
    let corners = [(vx - w / 2.0, vy - h / 2.0), (vx + w / 2.0, vy + h / 2.0)];
    let mapped: Vec<[f32; 2]> = corners.iter().map(|(x, y)| info.view_to_user(*x as f32, *y as f32)).collect();
    [
        f64::from(mapped[0][0].min(mapped[1][0])),
        f64::from(mapped[0][1].min(mapped[1][1])),
        f64::from(mapped[0][0].max(mapped[1][0])),
        f64::from(mapped[0][1].max(mapped[1][1])),
    ]
}

fn render_page(session: &Session, id: DocId) -> pdfcraft_render::RenderedPage {
    let doc = session.get(id).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
    let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 2.0, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    rendered
}

fn stamp_pixel(rendered: &pdfcraft_render::RenderedPage, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * rendered.width + x) * 4) as usize;
    rendered.rgba[offset..offset + 4].try_into().unwrap()
}

/// #1398: a Sign Here tag points left as displayed, on every page rotation, including after save.
#[test]
fn rubber_stamps_point_left_as_displayed_on_rotated_pages() {
    let (w, h) = StampKind::SignHere.size();
    for degrees in [0, 90, 180, 270] {
        let (mut session, id) = rotated_blank(degrees);
        let info = session.get(id).unwrap().info.pages[0].clone();
        let rect = displayed_rect(&info, 100.0, 40.0, w, h);
        let shape = Shape::Stamp { rect, stamp: StampKind::SignHere, by: None };
        session
            .apply(
                id,
                Edit::AddAnnotation(NewAnnotation {
                    page: 0,
                    style: Style::default_for(&shape),
                    shape,
                    contents: String::new(),
                    author: "Test".into(),
                }),
            )
            .unwrap();
        let check = |session: &Session, id: DocId| {
            let rendered = render_page(session, id);
            let doc = session.get(id).unwrap();
            let info = doc.info.pages[0].clone();
            let stamp = doc.info.annotations[0].rect;
            let a = info.user_to_view(stamp[0], stamp[1]);
            let b = info.user_to_view(stamp[2], stamp[3]);
            let (left, top) = (a[0].min(b[0]), a[1].min(b[1]));
            let (right, bottom) = (a[0].max(b[0]), a[1].max(b[1]));
            let scale = 2.0;
            let at = |x: f32, y: f32| stamp_pixel(&rendered, (x * scale) as u32, (y * scale) as u32);
            // The tag's point is the left edge. The flat end fills the right edge.
            let tip = at(left + 4.0, (top + bottom) / 2.0);
            let outside = at(left + 4.0, top + 4.0);
            let end = at(right - 4.0, top + 4.0);
            assert!(tip[0] > 180 && tip[2] < 80, "rotation {degrees}: tip {tip:?} should be the orange point");
            assert!(outside.iter().all(|c| *c > 240), "rotation {degrees}: above the point {outside:?} should be paper");
            assert!(end[0] > 180 && end[2] < 80, "rotation {degrees}: flat end {end:?} should be orange");
        };
        check(&session, id);
        let saved = session.save_bytes(id).unwrap();
        let mut reopened = Session::new().with_clock(|| 1_700_000_000);
        let again = reopened.open("drawing.pdf", None, saved, None).unwrap();
        check(&reopened, again);
    }
}

/// #2583, #1869: a sticky note placed on a turned page is drawn upright, with its top-left as
/// displayed where it was placed (the corner its `/Rect` keeps, as the NoRotate flag it is saved
/// with asks), exactly as on an unturned page, including after save. The comment list's
/// `shown_rect`, which the UI hit-tests, is that square.
#[test]
fn sticky_notes_stay_upright_where_they_are_placed_on_rotated_pages() {
    // The displayed 20 × 20 pt square whose top-left is (100, 40), in device pixels at scale 2.
    let crop = |rendered: &pdfcraft_render::RenderedPage| -> Vec<[u8; 4]> {
        (80..120).flat_map(|y| (200..240).map(move |x| (x, y))).map(|(x, y)| stamp_pixel(rendered, x, y)).collect()
    };
    let mut upright = None;
    for degrees in [0, 90, 180, 270] {
        let (mut session, id) = rotated_blank(degrees);
        let info = session.get(id).unwrap().info.pages[0].clone();
        let [x, y] = info.view_to_user(100.0, 40.0);
        let shape = Shape::Note { at: [f64::from(x), f64::from(y)], icon: NoteIcon::Comment };
        let style = Style::default_for(&shape);
        session.apply(id, Edit::AddAnnotation(NewAnnotation { page: 0, style, shape, contents: "Turned".into(), author: "Test".into() })).unwrap();
        let mut check = |session: &Session, id: DocId| {
            let doc = session.get(id).unwrap();
            let info = &doc.info.pages[0];
            let note = &doc.info.annotations[0];
            assert!(note.no_rotate, "rotation {degrees}: notes are saved NoRotate");
            let shown = note.shown_rect(info.rotation);
            let (a, b) = (info.user_to_view(shown[0], shown[1]), info.user_to_view(shown[2], shown[3]));
            let shown = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
            assert!(shown.iter().zip([100.0, 40.0, 120.0, 60.0]).all(|(v, w)| (v - w).abs() < 0.01), "rotation {degrees}: shown at {shown:?}");
            let icon = crop(&render_page(session, id));
            let reference = upright.get_or_insert_with(|| icon.clone());
            assert!(reference.iter().filter(|c| c.iter().any(|v| *v < 200)).count() > 100, "the note's icon is drawn");
            let off = icon.iter().zip(reference.iter()).filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 48)).count();
            assert!(off <= 8, "rotation {degrees}: {off} of the icon's {} pixels differ from the unturned page's", icon.len());
        };
        check(&session, id);
        let saved = session.save_bytes(id).unwrap();
        let mut reopened = Session::new().with_clock(|| 1_700_000_000);
        let again = reopened.open("drawing.pdf", None, saved, None).unwrap();
        check(&reopened, again);
    }
}

/// #2392: Fill & Sign marks are drawn as displayed on every page rotation, including after save:
/// a checkmark's long stroke rises to the right, and a line runs across.
#[test]
fn fill_sign_marks_are_upright_as_displayed_on_rotated_pages() {
    for degrees in [0, 90, 180, 270] {
        let (mut session, id) = rotated_blank(degrees);
        let info = session.get(id).unwrap().info.pages[0].clone();
        // Displayed boxes: a 40 pt checkmark centred at (50, 50), an 80 × 8 pt line at (130, 50).
        for (mark, vx, w, h) in [(FillMark::Check, 50.0, 40.0, 40.0), (FillMark::Line, 130.0, 80.0, 8.0)] {
            let shape = Shape::Mark { rect: displayed_rect(&info, vx, 50.0, w, h), mark };
            session
                .apply(
                    id,
                    Edit::AddAnnotation(NewAnnotation {
                        page: 0,
                        style: Style::default_for(&shape),
                        shape,
                        contents: String::new(),
                        author: "Test".into(),
                    }),
                )
                .unwrap();
        }
        let check = |session: &Session, id: DocId| {
            let rendered = render_page(session, id);
            // Dark pixels in a displayed box, rendered at scale 2.
            let ink = |x0: u32, y0: u32, x1: u32, y1: u32| -> Vec<(u32, u32)> {
                (y0 * 2..y1 * 2)
                    .flat_map(|y| (x0 * 2..x1 * 2).map(move |x| (x, y)))
                    .filter(|&(x, y)| stamp_pixel(&rendered, x, y)[..3].iter().all(|c| *c < 128))
                    .collect()
            };
            let tick = ink(30, 30, 70, 70);
            assert!(!tick.is_empty(), "rotation {degrees}: no checkmark drawn");
            let top = tick.iter().min_by_key(|p| p.1).unwrap();
            let bottom = tick.iter().max_by_key(|p| p.1).unwrap();
            // The long stroke ends at the top right; the short one meets it low, left of centre.
            assert!(top.0 > bottom.0 + 20, "rotation {degrees}: highest ink {top:?} should be right of the lowest {bottom:?}");
            assert!(bottom.0 < 100 && top.0 > 110, "rotation {degrees}: checkmark {bottom:?} .. {top:?} is not upright");
            let line = ink(85, 40, 175, 60);
            assert!(!line.is_empty(), "rotation {degrees}: no line drawn");
            let (x0, x1) = (line.iter().map(|p| p.0).min().unwrap(), line.iter().map(|p| p.0).max().unwrap());
            let (y0, y1) = (line.iter().map(|p| p.1).min().unwrap(), line.iter().map(|p| p.1).max().unwrap());
            assert!((x1 - x0) > 10 * (y1 - y0 + 1), "rotation {degrees}: line {x0}..{x1} x {y0}..{y1} should run across");
        };
        check(&session, id);
        let saved = session.save_bytes(id).unwrap();
        let mut reopened = Session::new().with_clock(|| 1_700_000_000);
        let again = reopened.open("drawing.pdf", None, saved, None).unwrap();
        check(&reopened, again);
    }
}

/// Fill & Sign text and text boxes read across as displayed on turned pages (scanned forms are
/// often stored sideways with `/Rotate`), including after save.
#[test]
fn typed_text_reads_across_as_displayed_on_rotated_pages() {
    for degrees in [0, 90, 180, 270] {
        let (mut session, id) = rotated_blank(degrees);
        let info = session.get(id).unwrap().info.pages[0].clone();
        // Displayed top-left of the text at (40, 60): a wide first line, then one letter below
        // its start.
        let at = info.view_to_user(40.0, 60.0);
        let at = [f64::from(at[0]), f64::from(at[1])];
        let rect = text_box_at(degrees, at, 120.0, 2.0 * 12.0 * 1.2 + 6.0).unwrap();
        let shape = Shape::Typewriter { rect, font_size: 12.0 };
        let style = Style::default_for(&shape);
        session
            .apply(
                id,
                Edit::AddAnnotation(NewAnnotation {
                    page: 0,
                    style,
                    shape,
                    contents: "MMMMMMMM
I"
                    .into(),
                    author: "Test".into(),
                }),
            )
            .unwrap();
        let check = |session: &Session, id: DocId| {
            let rendered = render_page(session, id);
            let dark = |x: u32, y: u32| stamp_pixel(&rendered, x, y)[..3].iter().all(|c| *c < 128);
            let ink: Vec<(u32, u32)> =
                (0..rendered.height).flat_map(|y| (0..rendered.width).map(move |x| (x, y))).filter(|&(x, y)| dark(x, y)).collect();
            assert!(!ink.is_empty(), "rotation {degrees}: no text drawn");
            let (x0, x1) = (ink.iter().map(|p| p.0).min().unwrap(), ink.iter().map(|p| p.0).max().unwrap());
            let (y0, y1) = (ink.iter().map(|p| p.1).min().unwrap(), ink.iter().map(|p| p.1).max().unwrap());
            // Rendered at scale 2: the text starts at the clicked point and runs to the right.
            assert!((x1 - x0) > 2 * (y1 - y0), "rotation {degrees}: ink {x0}..{x1} x {y0}..{y1} should be wide, not tall");
            assert!(x0.abs_diff(80) < 16 && y0.abs_diff(120) < 16, "rotation {degrees}: text starts at ({x0}, {y0}), not near (80, 120)");
            // The lone "I" is on the second line under the start of the first, not upside down
            // (above the end).
            let mid = (y0 + y1) / 2;
            let lower: Vec<u32> = ink.iter().filter(|p| p.1 > mid + 2).map(|p| p.0).collect();
            assert!(!lower.is_empty() && lower.iter().all(|x| *x < x0 + (x1 - x0) / 4), "rotation {degrees}: second line is not under the start");
        };
        check(&session, id);
        let saved = session.save_bytes(id).unwrap();
        let mut reopened = Session::new().with_clock(|| 1_700_000_000);
        let again = reopened.open("drawing.pdf", None, saved, None).unwrap();
        check(&reopened, again);
    }
}

/// A one-page PDF whose left half is red and whose right half is white, as displayed.
fn half_red_page() -> Vec<u8> {
    let content = "1 0 0 rg 0 0 40 40 re f";
    let objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 80 40] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /MediaBox [0 0 80 40] >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn quadrant_png() -> Arc<Vec<u8>> {
    let colours = [[240, 20, 20, 255], [20, 180, 20, 255], [20, 20, 240, 255], [230, 180, 20, 255]];
    let image = image::RgbaImage::from_fn(80, 40, |x, y| image::Rgba(colours[usize::from(y >= 20) * 2 + usize::from(x >= 40)]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    Arc::new(png.into_inner())
}

fn assert_upright_quadrants(session: &Session, id: DocId, degrees: i64) {
    let doc = session.get(id).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
    let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    let colours = [[240, 20, 20, 255], [20, 180, 20, 255], [20, 20, 240, 255], [230, 180, 20, 255]];
    for ((x, y), colour) in [(80, 90), (120, 90), (80, 110), (120, 110)].into_iter().zip(colours) {
        assert_eq!(stamp_pixel(&rendered, x, y), colour, "rotation {degrees}; displayed point ({x}, {y})");
    }
}

/// #1398: an explicit rectangle keeps the caller's box and turns the picture upright.
#[test]
fn explicit_image_stamps_keep_displayed_orientation_on_rotated_pages() {
    let bytes = quadrant_png();
    for degrees in [0, 90, 180, 270] {
        let (mut session, id) = rotated_blank(degrees);
        let info = session.get(id).unwrap().info.pages[0].clone();
        let rect = displayed_rect(&info, 100.0, 100.0, 80.0, 40.0);
        session
            .apply(
                id,
                Edit::AddCustomStamp {
                    page: 0,
                    rect,
                    name: "Original quadrants".into(),
                    file: MarkFile { name: "quadrants.png".into(), bytes: bytes.clone(), page: 0 },
                    author: "Test".into(),
                },
            )
            .unwrap();
        assert_upright_quadrants(&session, id, degrees);
        session.apply(id, Edit::StyleAnnotation { page: 0, index: 0, color: None, opacity: Some(1.0), width: None, endings: None }).unwrap();
        assert_upright_quadrants(&session, id, degrees);
    }
}

/// A visible digital signature reads upright as the page is displayed on every page rotation:
/// its picture, alone in the box, shows its quadrants where they are in the image.
#[test]
fn visible_signatures_keep_displayed_orientation_on_rotated_pages() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    let picture = image::load_from_memory(&quadrant_png()).unwrap().to_rgba8();
    let graphic = sign::Graphic::Image { width: picture.width(), height: picture.height(), rgba: Arc::new(picture.into_raw()) };
    let bare = sign::Appearance { name: false, date: false, reason: false, location: false, distinguished_name: false, labels: false };
    for degrees in [0, 90, 180, 270] {
        // After the test certificate's start date (2026-10-02).
        let mut session = Session::new().with_clock(|| 1_800_000_000);
        let blank = session.create_blank(200.0, 300.0, 1).unwrap();
        let id = session.open_new("contract.pdf", blank).unwrap();
        session.apply(id, Edit::RotatePages { pages: vec![0], degrees }).unwrap();
        let info = session.get(id).unwrap().info.pages[0].clone();
        // The picture's proportions (2:1) as displayed, centred at (100, 100).
        let rect = displayed_rect(&info, 100.0, 100.0, 80.0, 40.0);
        let opts = SignOptions { page: 0, rect: Some(rect), graphic: Some(graphic.clone()), appearance: bare.clone(), ..SignOptions::default() };
        let signed = session.sign(id, &digital_id, opts).unwrap();
        let reopened = session.open("signed.pdf", None, signed, None).unwrap();
        assert_upright_quadrants(&session, reopened, degrees);
    }
}

/// #1398: a PDF page used as a stamp stays upright, at its natural size and in an explicit box.
#[test]
fn pdf_page_stamps_keep_displayed_orientation_on_rotated_pages() {
    let bytes = Arc::new(half_red_page());
    for (natural, degrees) in [true, false].into_iter().flat_map(|natural| [0, 90, 180, 270].map(|degrees| (natural, degrees))) {
        let (mut session, id) = rotated_blank(degrees);
        let info = session.get(id).unwrap().info.pages[0].clone();
        let at = info.view_to_user(100.0, 100.0).map(f64::from);
        let rect = if natural { [at[0], at[1], at[0], at[1]] } else { displayed_rect(&info, 100.0, 100.0, 80.0, 40.0) };
        session
            .apply(
                id,
                Edit::AddCustomStamp {
                    page: 0,
                    rect,
                    name: "Half red".into(),
                    file: MarkFile { name: "half.pdf".into(), bytes: bytes.clone(), page: 0 },
                    author: "Test".into(),
                },
            )
            .unwrap();
        let doc = session.get(id).unwrap();
        let mut renderer = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
        let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        let left = stamp_pixel(&rendered, 80, 100);
        let right = stamp_pixel(&rendered, 120, 100);
        assert!(left[0] > 200 && left[1] < 40, "rotation {degrees}, natural {natural}: left {left:?} should be red");
        assert!(right.iter().all(|c| *c > 240), "rotation {degrees}, natural {natural}: right {right:?} should be paper");
    }
}

/// #4077: a PDF page stamped into a box of another shape is fitted inside it, keeping its shape
/// and centred, and stays vector: a form XObject, not a picture of it.
#[test]
fn pdf_page_stamps_fit_inside_an_explicit_box() {
    let bytes = Arc::new(half_red_page());
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let blank = session.create_blank(595.0, 842.0, 1).unwrap();
    let id = session.open_new("text.pdf", blank).unwrap();
    // 483 pt wide (a text column) and 300 pt tall: the 80 × 40 page is 483 × 241.5, centred.
    let stamp = |rect| Edit::AddCustomStamp {
        page: 0,
        rect,
        name: "Diagram".into(),
        file: MarkFile { name: "diagram.pdf".into(), bytes: bytes.clone(), page: 0 },
        author: "Test".into(),
    };
    session.apply(id, stamp([56.0, 400.0, 539.0, 700.0])).unwrap();
    assert_eq!(session.get(id).unwrap().info.annotations[0].rect, [56.0, 429.25, 539.0, 670.75]);
    // A box too tall and narrow: the width limits it, and it is centred vertically.
    session.apply(id, stamp([100.0, 100.0, 140.0, 300.0])).unwrap();
    assert_eq!(session.get(id).unwrap().info.annotations[1].rect, [100.0, 190.0, 140.0, 210.0]);

    let doc = pdfcraft_cos::Document::open(session.save_bytes(id).unwrap()).unwrap();
    let page = &pdfcraft_model::pages(&doc)[0];
    let annots = doc.resolve(page.dict.get(b"Annots").unwrap());
    let first = annots.as_array().unwrap()[0].as_ref().unwrap();
    let annot = doc.get(first);
    let annot = annot.as_dict().unwrap();
    let pdfcraft_cos::Object::Stream(picture) = doc.get(annot.reference(b"PCPicture").unwrap()).as_ref().clone() else {
        panic!("the picture is a stream")
    };
    assert_eq!(picture.dict.name(b"Subtype"), Some(&b"Form"[..]), "the page is kept as a form XObject");
    let ap = doc.resolve(annot.get(b"AP").unwrap());
    let pdfcraft_cos::Object::Stream(normal) = doc.get(ap.as_dict().unwrap().reference(b"N").unwrap()).as_ref().clone() else {
        panic!("an appearance stream")
    };
    let bbox: Vec<f64> = normal.dict.get(b"BBox").unwrap().as_array().unwrap().iter().filter_map(pdfcraft_cos::Object::as_f64).collect();
    assert_eq!(bbox, [56.0, 429.25, 539.0, 670.75]);
    let content = String::from_utf8(normal.decoded().unwrap().to_vec()).unwrap();
    assert!(content.contains("/Pic Do") && !content.contains("BI"), "{content}");
}

fn page_texts(s: &Session, id: DocId) -> Vec<String> {
    let doc = s.get(id).unwrap();
    let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
    (0..doc.info.pages.len())
        .map(|p| {
            let r = r.render(RenderRequestFor::text(p));
            r.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

/// Shorthand for building text-extraction requests.
struct RenderRequestFor;
impl RenderRequestFor {
    fn text(page: usize) -> pdfcraft_render::RenderRequest {
        pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() }
    }
}

/// A file with its title and author only in XMP shows them in the document information and in
/// Document Properties ▸ Description (#2373).
#[test]
fn document_information_stated_only_in_xmp_is_shown() {
    let mut cos = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
               <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
               <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Quarterly report</rdf:li></rdf:Alt></dc:title>\
               <dc:creator><rdf:Seq><rdf:li>Ada</rdf:li></rdf:Seq></dc:creator></rdf:Description></rdf:RDF></x:xmpmeta>";
    let mut dict = pdfcraft_cos::Dict::new();
    dict.set(b"Type".to_vec(), pdfcraft_cos::Object::name("Metadata"));
    dict.set(b"Subtype".to_vec(), pdfcraft_cos::Object::name("XML"));
    let m = cos.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(dict, xmp.as_bytes().to_vec())));
    let root = cos.root().unwrap();
    cos.update_dict(root, |c| c.set(b"Metadata".to_vec(), pdfcraft_cos::Object::Ref(m))).unwrap();
    let bytes = pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap();
    let mut s = Session::new();
    let id = s.open("xmp.pdf", None, Arc::new(bytes), None).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!((doc.info.title.as_deref(), doc.info.author.as_deref(), doc.info.subject.as_deref()), (Some("Quarterly report"), Some("Ada"), None));
    assert_eq!(doc.info_value("Title").as_deref(), Some("Quarterly report"));
    assert_eq!(doc.info_value("Author").as_deref(), Some("Ada"));

    // A full refresh (any structural edit, undo, redo) reads /Info again and must not lose them.
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!((doc.info.title.as_deref(), doc.info.author.as_deref()), (Some("Quarterly report"), Some("Ada")), "after an edit");
    s.undo(id).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!((doc.info.title.as_deref(), doc.info.author.as_deref()), (Some("Quarterly report"), Some("Ada")), "after undo");
}

#[test]
fn edits_update_view_data_and_mark_dirty() {
    let (mut s, id) = session_with(3);
    assert!(s.get(id).unwrap().editable());
    assert!(!s.get(id).unwrap().dirty);
    s.apply(id, Edit::DeletePages { pages: vec![1] }).unwrap();
    let doc = s.get(id).unwrap();
    assert!(doc.dirty);
    assert_eq!(doc.info.pages.len(), 2, "inspection refreshed");
    assert_eq!(doc.can_undo(), Some("Delete page"));
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 3"], "renderer refreshed");
}

#[test]
fn metadata_edit_keeps_the_renderer_without_changing_page_content() {
    let (mut s, id) = session_with(1);
    let request = pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Pixels, scale: 1.0, ..Default::default() };
    let render = |session: &Session| {
        let doc = session.get(id).unwrap();
        doc.renderer.set_queue(vec![request]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(page) = doc.renderer.try_recv() {
                assert!(page.error.is_none(), "{:?}", page.error);
                return (doc.renderer.stats(), page.rgba);
            }
            assert!(std::time::Instant::now() < deadline, "page render did not finish");
            std::thread::yield_now();
        }
    };

    let (before, before_pixels) = render(&s);
    assert_eq!(before.page_interpretations, 1);
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Report".into() }).unwrap();
    assert_eq!(s.get(id).unwrap().info_value("Title").as_deref(), Some("Report"));
    assert_eq!(s.get(id).unwrap().renderer.stats().page_interpretations, 1, "a metadata edit keeps the renderer and its page interpretation");
    let (after, after_pixels) = render(&s);
    assert_eq!(after.page_interpretations, 2, "the same renderer serves the second request, so its count continues");
    assert_eq!(before_pixels, after_pixels, "document properties do not change page pixels");
}

#[test]
fn metadata_edits_and_history_reuse_the_render_pool() {
    let (mut s, id) = session_with(1);
    let pixels = shown(&s, id, 0).rgba;
    let doc = s.get(id).unwrap();
    let stats = doc.renderer.stats();
    let display = doc.display.clone();
    assert_eq!(stats.page_interpretations, 1);

    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Report".into() }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.title.as_deref(), Some("Report"));
    assert_eq!(doc.renderer.stats(), stats);
    assert!(Arc::ptr_eq(&doc.display, &display));
    assert_eq!(pixels.as_ref().len(), 200 * 300 * 4);

    s.undo(id).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.title, None);
    assert_eq!(doc.renderer.stats(), stats);
    assert!(Arc::ptr_eq(&doc.display, &display));

    s.redo(id).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.title.as_deref(), Some("Report"));
    assert_eq!(doc.renderer.stats(), stats);
    assert!(Arc::ptr_eq(&doc.display, &display));
}

#[test]
fn field_and_comment_edits_still_rebuild_the_render_pool() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, Arc::new(scripted_form()), None).unwrap();
    let _ = shown(&s, id, 0);
    assert_eq!(s.get(id).unwrap().renderer.stats().page_interpretations, 1);

    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("4".into()) }).unwrap();
    assert_eq!(s.get(id).unwrap().renderer.stats().page_interpretations, 0);
    let _ = shown(&s, id, 0);
    assert_eq!(s.get(id).unwrap().renderer.stats().page_interpretations, 1);

    let shape = Shape::Rectangle { rect: [20.0, 20.0, 60.0, 60.0] };
    s.apply(
        id,
        Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Test".into() }),
    )
    .unwrap();
    assert_eq!(s.get(id).unwrap().renderer.stats().page_interpretations, 0);
}

#[test]
fn undo_and_redo_restore_exact_states() {
    let (mut s, id) = session_with(3);
    let original = s.get(id).unwrap().bytes.clone();
    s.apply(id, Edit::MovePages { pages: vec![2], to: 0 }).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert_eq!(s.get(id).unwrap().info.pages[0].rotation, 90);
    assert_eq!(s.undo(id).unwrap().as_str(), "Rotate page");
    assert_eq!(s.get(id).unwrap().info.pages[0].rotation, 0);
    assert_eq!(s.undo(id).unwrap().as_str(), "Move page");
    assert_eq!(s.get(id).unwrap().bytes, original, "undoing everything returns the original bytes");
    assert_eq!(s.undo(id), Err(EditError::NothingToUndo));
    assert_eq!(s.redo(id).unwrap().as_str(), "Move page");
    assert_eq!(page_texts(&s, id), ["Page 3", "Page 1", "Page 2"]);
    // A new edit clears the redo stack.
    s.apply(id, Edit::InsertBlankPage { at: 0, width: 612.0, height: 792.0 }).unwrap();
    assert_eq!(s.redo(id), Err(EditError::NothingToRedo));
    assert_eq!(s.get(id).unwrap().info.pages.len(), 4);
}

#[test]
fn failed_edit_changes_nothing() {
    let (mut s, id) = session_with(2);
    let before = s.get(id).unwrap().bytes.clone();
    let err = s.apply(id, Edit::DeletePages { pages: vec![0, 1] }).unwrap_err();
    assert_eq!(err, EditError::Organize(pdfcraft_organize::OrganizeError::WouldRemoveAllPages));
    let doc = s.get(id).unwrap();
    assert!(!doc.dirty);
    assert_eq!(doc.can_undo(), None);
    assert_eq!(doc.bytes, before);
}

#[test]
fn save_is_incremental_stamps_mod_date_and_rebases() {
    let (mut s, id) = session_with(2);
    let original = s.get(id).unwrap().bytes.clone();
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Quarterly report".into() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    assert_eq!(&saved[..original.len()], &original[..], "incremental save keeps the original bytes");
    let text = String::from_utf8_lossy(&saved[original.len()..]);
    assert!(text.contains("/ModDate (D:20231114221320Z)"), "mod date stamped: {text}");
    s.mark_saved(id, saved.clone(), Some("/tmp/out/report.pdf".into())).unwrap();
    let doc = s.get(id).unwrap();
    assert!(!doc.dirty);
    assert_eq!(doc.name, "report.pdf");
    assert_eq!(doc.info_value("Title").as_deref(), Some("Quarterly report"));
    assert_eq!(doc.info.title.as_deref(), Some("Quarterly report"));
    // Saving again without edits writes the same bytes; a later edit appends just one revision.
    assert_eq!(s.save_bytes(id).unwrap(), saved);
    s.apply(id, Edit::RotatePages { pages: vec![1], degrees: -90 }).unwrap();
    let second = s.save_bytes(id).unwrap();
    assert_eq!(&second[..saved.len()], &saved[..]);
    let reopened = pdfcraft_cos::Document::open(second).unwrap();
    assert_eq!(reopened.revisions().len(), 3);
}

/// #1937: a file whose cross-reference data had to be rebuilt saved as its original bytes when
/// nothing was edited, so the damage Document Properties reported as repaired was still in the
/// file. Save writes the rebuilt file in full; reopened, it needs no repair.
#[test]
fn a_repaired_file_is_rewritten_by_save_even_without_edits() {
    let good = fixture(2);
    // Every offset in the table is off by the inserted line: the chain reads but leads nowhere.
    let mut shifted = b"%PDF-1.7\n%shifted\n".to_vec();
    shifted.extend_from_slice(&good[b"%PDF-1.7\n".len()..]);
    // No cross-reference table at all.
    let mut lost = good.clone();
    lost.truncate(good.windows(9).rposition(|w| w == b"startxref").unwrap());
    lost.extend_from_slice(b"startxref\n999999\n%%EOF\n");
    for (name, damaged) in [("shifted", shifted), ("lost", lost)] {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("broken.pdf", Some("/tmp/broken.pdf".into()), Arc::new(damaged.clone()), None).unwrap();
        let doc = s.get(id).unwrap();
        assert!(doc.repair_log().iter().any(|l| l.contains("rebuilt cross-reference index")), "{name}: {:?}", doc.repair_log());
        assert_eq!(doc.info.pages.len(), 2, "{name}");
        assert!(!doc.dirty, "{name}: a repair is not an edit");
        let saved = s.save_bytes(id).unwrap();
        assert_ne!(&saved[..], &damaged[..], "{name}: the damaged bytes are not what Save writes");
        let reopened = pdfcraft_cos::Document::open(saved.clone()).unwrap();
        assert!(reopened.repair_log().is_empty(), "{name}: {:?}", reopened.repair_log());
        assert_eq!(reopened.revisions().len(), 1, "{name}");
        s.mark_saved(id, saved.clone(), None).unwrap();
        let doc = s.get(id).unwrap();
        assert!(doc.repair_log().is_empty(), "{name}: {:?}", doc.repair_log());
        assert_eq!(doc.info.pages.len(), 2, "{name}");
        // Sound now: saving again without edits writes the same bytes.
        assert_eq!(s.save_bytes(id).unwrap(), saved, "{name}");
    }
}

/// A damaged file that is signed keeps its bytes on a Save without edits: rewriting it in full
/// to put the repair on disk would invalidate its signatures without a word.
#[test]
fn a_signed_repaired_file_keeps_its_bytes_when_saved_without_edits() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(1)), None).unwrap();
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    // A trailing `startxref` that points nowhere: the signed revision's bytes are untouched, but
    // the file's cross-reference data has to be rebuilt.
    let mut damaged = signed.to_vec();
    damaged.extend_from_slice(b"\nstartxref\n999999\n%%EOF\n");
    let mut t = Session::new().with_clock(|| 1_800_000_000);
    let tid = t.open("damaged.pdf", Some("/tmp/damaged.pdf".into()), Arc::new(damaged.clone()), None).unwrap();
    let doc = t.get(tid).unwrap();
    assert!(doc.repair_log().iter().any(|l| l.contains("rebuilt cross-reference index")), "{:?}", doc.repair_log());
    assert!(doc.is_signed());
    assert_eq!(&t.save_bytes(tid).unwrap()[..], &damaged[..], "a signed file is not rewritten without edits");
}

#[test]
fn undo_still_works_after_save() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::DeletePages { pages: vec![0] }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    s.undo(id).unwrap();
    let doc = s.get(id).unwrap();
    assert!(!doc.dirty, "undo back to the saved file is clean");
    assert_eq!(doc.info.pages.len(), 1);
    assert_eq!(doc.info.pages[0].rotation, 0);
    s.undo(id).unwrap();
    let doc = s.get(id).unwrap();
    assert!(doc.dirty, "undoing past a save makes the document dirty again");
    assert_eq!(doc.info.pages.len(), 2);
}

#[test]
fn undo_to_the_opened_file_is_not_dirty() {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let opened = Arc::new(fixture(2));
    let id = session.open("existing.pdf", None, opened.clone(), None).unwrap();
    assert!(!session.get(id).unwrap().dirty);
    assert!(session.autosave_snapshots().is_empty());

    session.apply(id, Edit::DeletePages { pages: vec![0] }).unwrap();
    assert_eq!(session.get(id).unwrap().info.pages.len(), 1);
    session.undo(id).unwrap();
    let document = session.get(id).unwrap();
    assert!(!document.dirty, "the opened bytes are the saved version");
    assert_eq!(document.bytes, opened);
    assert_eq!(document.info.pages.len(), 2);
    assert!(session.autosave_snapshots().is_empty(), "nothing unsaved to recover");

    session.redo(id).unwrap();
    assert!(session.get(id).unwrap().dirty);
    session.undo(id).unwrap();
    assert!(!session.get(id).unwrap().dirty);
}

#[test]
fn save_after_undo_retains_the_latest_saved_revision() {
    let (mut session, document) = session_with(3);
    session.apply(document, Edit::DeletePages { pages: vec![1] }).unwrap();
    let deleted = session.save_bytes(document).unwrap();
    session.mark_saved(document, deleted, None).unwrap();
    session.apply(document, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let rotated = session.save_bytes(document).unwrap();
    session.mark_saved(document, rotated.clone(), None).unwrap();
    let rotated_revisions = pdfcraft_cos::Document::open(rotated.clone()).unwrap().revisions().len();

    session.undo(document).unwrap();
    session.apply(document, Edit::SetInfo { key: "Title".into(), value: "Report".into() }).unwrap();
    let saved = session.save_bytes(document).unwrap();
    assert!(saved.starts_with(rotated.as_slice()), "the saved rotation stays in the file");
    let reopened = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    assert!(reopened.revisions().len() > rotated_revisions, "the title is a newer revision, not a rewrite of the earlier one");
    assert_eq!(pdfcraft_organize::info(&reopened, "Title").as_deref(), Some("Report"));

    session.mark_saved(document, saved, None).unwrap();
    let doc = session.get(document).unwrap();
    assert_eq!(page_texts(&session, document), ["Page 1", "Page 3"]);
    assert_eq!(doc.info.pages[0].rotation, 0, "the undone rotation is the current page");
    assert_eq!(doc.info_value("Title").as_deref(), Some("Report"));
}

#[test]
fn revert_preserves_the_saved_pages_after_undo() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::DeletePages { pages: vec![0] }).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let saved_bytes = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved_bytes, None).unwrap();
    s.undo(id).unwrap();
    assert!(s.get(id).unwrap().dirty);
    assert_eq!(s.get(id).unwrap().info.pages[0].rotation, 0);

    s.revert(id).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.dirty && d.can_undo().is_none() && d.can_redo().is_none());
    assert_eq!(page_texts(&s, id), ["Page 2"], "the last saved pages, not the file as opened");
    assert_eq!(d.info.pages[0].rotation, 90);
}

#[test]
fn full_save_is_readable_and_smaller_after_deletes() {
    let (mut s, id) = session_with(5);
    s.apply(id, Edit::DeletePages { pages: vec![0, 1, 2] }).unwrap();
    let full = s.save_full_bytes(id).unwrap();
    assert!(full.len() < s.save_bytes(id).unwrap().len());
    let mut s2 = Session::new();
    let id2 = s2.open("full.pdf", None, full, None).unwrap();
    assert_eq!(page_texts(&s2, id2), ["Page 4", "Page 5"]);
}

#[test]
fn layer_choices_survive_edits() {
    // No layers in the fixture: just make sure refresh keeps working with an empty list and
    // that unknown layers are rejected.
    let (mut s, id) = session_with(1);
    assert!(!s.set_layer_visible(id, 0, false));
    s.apply(id, Edit::InsertBlankPage { at: 1, width: 100.0, height: 100.0 }).unwrap();
    assert_eq!(s.get(id).unwrap().info.pages.len(), 2);
}

#[test]
fn set_layer_actions_toggle_in_order_and_keep_radio_groups() {
    use pdfcraft_render::LayerOp::{Off, On, Toggle};
    let layer = |n: u32, visible: bool| Layer { id: (n, 0), name: format!("L{n}"), visible, initial: visible };
    let run = |changes: &[(LayerOp, u32)], preserve_rb: bool| {
        // L1 and L2 are a radio-button group; L3 is on its own.
        let mut layers = vec![layer(1, true), layer(2, false), layer(3, true)];
        let changes: Vec<_> = changes.iter().map(|&(op, n)| (op, (n, 0))).collect();
        let changed = apply_layer_state(&mut layers, &[vec![(1, 0), (2, 0)]], &changes, preserve_rb);
        (changed, layers.iter().map(|l| l.visible).collect::<Vec<_>>())
    };
    // Turning a layer on turns off the rest of its group, unless the action says not to.
    assert_eq!(run(&[(On, 2)], true), (true, vec![false, true, true]));
    assert_eq!(run(&[(On, 2)], false), (true, vec![true, true, true]));
    // A toggle that turns a layer on does the same.
    assert_eq!(run(&[(Toggle, 2)], true), (true, vec![false, true, true]));
    // Turning a layer off leaves its group alone.
    assert_eq!(run(&[(Off, 1)], true), (true, vec![false, false, true]));
    // Each toggle sees the state the changes before it left: twice is no change.
    assert_eq!(run(&[(Toggle, 3), (Toggle, 3)], true), (false, vec![true, false, true]));
    // Later changes win, and groups that aren't layers are skipped.
    assert_eq!(run(&[(On, 1), (On, 2), (Off, 99)], true), (true, vec![false, true, true]));
    // What counts is the end state: on and back off again is no change.
    assert_eq!(run(&[(On, 2), (On, 1), (Off, 99)], true), (false, vec![true, false, true]));
}

#[test]
fn set_layer_actions_change_what_renders() {
    // A red left half on layer Red (on) and a green right half on layer Green (off); the two
    // are a radio-button group.
    let content = "/OC /R BDC 1 0 0 rg 0 0 50 100 re f EMC /OC /G BDC 0 1 0 rg 50 0 50 100 re f EMC";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [6 0 R] /RBGroups [[5 0 R 6 0 R]] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Properties << /R 5 0 R /G 6 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /OCG /Name (Red) >>".into(),
        "<< /Type /OCG /Name (Green) >>".into(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new();
    let id = s.open("layers.pdf", None, Arc::new(pdf), None).unwrap();
    // The colours at the middle of the left and right halves, as the document draws now.
    let halves = |s: &Session| {
        let doc = s.get(id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
        let p = r.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Pixels, scale: 1.0, ..Default::default() });
        let at = |x: usize| {
            let i = (50 * p.width as usize + x) * 4;
            p.rgba[i..i + 3].to_vec()
        };
        (at(25), at(75))
    };
    let (red, green, white) = (vec![255, 0, 0], vec![0, 255, 0], vec![255, 255, 255]);
    assert_eq!(halves(&s), (red.clone(), white.clone()));
    // Showing Green hides Red, its radio-button partner.
    assert!(s.set_layer_state(id, &[(LayerOp::On, (6, 0))], true));
    let visible: Vec<_> = s.get(id).unwrap().info.layers.iter().map(|l| (l.name.as_str(), l.visible)).collect();
    assert_eq!(visible, [("Red", false), ("Green", true)]);
    assert_eq!(halves(&s), (white.clone(), green));
    // Running it again changes nothing.
    assert!(!s.set_layer_state(id, &[(LayerOp::On, (6, 0))], true));
    // Turning Green off brings nothing back on.
    assert!(s.set_layer_state(id, &[(LayerOp::Off, (6, 0))], true));
    assert_eq!(halves(&s), (white.clone(), white));
    assert!(!s.set_layer_state(DocId(999), &[(LayerOp::On, (5, 0))], true));
}

#[test]
fn reset_layers_returns_to_the_files_visibility() {
    // Red (left half, on) and Green (right half, off) are a radio-button group.
    let content = "/OC /R BDC 1 0 0 rg 0 0 50 100 re f EMC /OC /G BDC 0 1 0 rg 50 0 50 100 re f EMC";
    let pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [6 0 R] /RBGroups [[5 0 R 6 0 R]] >> >> >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Properties << /R 5 0 R /G 6 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /OCG /Name (Red) >>")
        .obj("<< /Type /OCG /Name (Green) >>")
        .build();
    // A page drawing on layer Blue, which that file has off.
    let other = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] >> >> >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Properties << /B 5 0 R >> >> >>")
        .stream("", "/OC /B BDC 0 0 1 rg 0 0 10 10 re f EMC")
        .obj("<< /Type /OCG /Name (Blue) >>")
        .build();
    let mut s = Session::new();
    let id = s.open("layers.pdf", None, Arc::new(pdf), None).unwrap();
    let shown = |s: &Session| s.get(id).unwrap().info.layers.iter().map(|l| (l.name.clone(), l.visible)).collect::<Vec<_>>();
    let left = |s: &Session| {
        let doc = s.get(id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
        let p = r.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Pixels, scale: 1.0, ..Default::default() });
        let i = (50 * p.width as usize + 25) * 4;
        p.rgba[i..i + 3].to_vec()
    };
    assert!(s.set_layer_state(id, &[(LayerOp::On, (6, 0))], true));
    let edit = Edit::InsertPagesFrom {
        name: "b.pdf".into(),
        bytes: Arc::new(other),
        pages: None,
        at: 1,
        field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
    };
    s.apply(id, edit).unwrap();
    let blue = s.get(id).unwrap().info.layers.iter().position(|l| l.name == "Blue").unwrap();
    assert!(s.set_layer_visible(id, blue, true));
    assert_eq!(shown(&s), [("Red".into(), false), ("Green".into(), true), ("Blue".into(), true)]);
    assert_eq!(left(&s), [255, 255, 255]);
    // Every layer, the inserted one too, goes back to the file's default configuration.
    assert!(s.reset_layers(id));
    assert_eq!(shown(&s), [("Red".into(), true), ("Green".into(), false), ("Blue".into(), false)]);
    assert_eq!(left(&s), [255, 0, 0]);
    assert!(!s.reset_layers(id), "already as the file opens");
    // What the eye sees is not an edit: Undo still undoes the insert.
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Insert pages from b.pdf"));
    assert!(!s.reset_layers(DocId(999)));
}

#[test]
fn edits_to_unknown_documents_are_rejected() {
    let (mut s, _) = session_with(1);
    assert_eq!(s.apply(DocId(999), Edit::DeletePages { pages: vec![0] }), Err(EditError::NoDocument));
    assert_eq!(s.undo(DocId(999)), Err(EditError::NoDocument));
    assert!(s.save_bytes(DocId(999)).is_err());
}

#[test]
fn batch_is_one_undo_step_and_all_or_nothing() {
    let (mut s, id) = session_with(3);
    let batch = Edit::Batch {
        label: "Change properties".into(),
        edits: vec![Edit::SetInfo { key: "Title".into(), value: "T".into() }, Edit::SetInfo { key: "Author".into(), value: "A".into() }],
    };
    s.apply(id, batch).unwrap();
    assert_eq!(s.get(id).unwrap().info_value("Author").as_deref(), Some("A"));
    assert_eq!(s.undo(id).unwrap().as_str(), "Change properties");
    assert_eq!(s.get(id).unwrap().info_value("Title"), None);
    assert_eq!(s.get(id).unwrap().can_undo(), None, "one step");
    // A failing member rolls back the whole batch.
    let bad = Edit::Batch { label: "x".into(), edits: vec![Edit::RotatePages { pages: vec![0], degrees: 90 }, Edit::DeletePages { pages: vec![9] }] };
    assert!(s.apply(id, bad).is_err());
    assert_eq!(s.get(id).unwrap().info.pages[0].rotation, 0);
}

#[test]
fn insert_blank_between_pages_keeps_both_neighbours() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::InsertBlankPage { at: 1, width: 200.0, height: 300.0 }).unwrap();
    assert_eq!(page_texts(&s, id), ["Page 1", "", "Page 2"]);
}

#[test]
fn combine_extract_split_and_insert_from_file() {
    let (mut s, id) = session_with(3);
    let other = Arc::new(fixture(2));
    // Combine: the first file then the second.
    let combined = s.combine(&[("a.pdf".into(), Arc::new(fixture(3))), ("b.pdf".into(), other.clone())]).unwrap();
    let cid = s.open_new("Combined.pdf", combined).unwrap();
    assert_eq!(page_texts(&s, cid), ["Page 1", "Page 2", "Page 3", "Page 1", "Page 2"]);
    assert!(s.get(cid).unwrap().dirty, "a new document starts unsaved");
    assert_eq!(s.get(cid).unwrap().info.outline.len(), 2, "one bookmark per file");
    // Extract pages 3 and 1 into a new document.
    let ex = s.extract(id, &[2, 0]).unwrap();
    let eid = s.open_new("Extract.pdf", ex).unwrap();
    assert_eq!(page_texts(&s, eid), ["Page 3", "Page 1"]);
    // Split every 2 pages.
    let parts = s.split(id, &pdfcraft_organize::SplitBy::PageCount(2)).unwrap();
    assert_eq!(parts.iter().map(|(a, b, _)| (*a, *b)).collect::<Vec<_>>(), [(1, 2), (3, 3)]);
    // Insert pages from a file, undoably, into the open document.
    s.apply(
        id,
        Edit::InsertPagesFrom {
            name: "b.pdf".into(),
            bytes: other,
            pages: Some(vec![1]),
            at: 1,
            field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
        },
    )
    .unwrap();
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2", "Page 2", "Page 3"]);
    assert_eq!(s.undo(id).unwrap().as_str(), "Insert pages from b.pdf");
    assert_eq!(page_texts(&s, id).len(), 3);
    // Garbage sources fail cleanly.
    let bad = s.apply(
        id,
        Edit::InsertPagesFrom {
            name: "junk.pdf".into(),
            bytes: Arc::new(b"nope".to_vec()),
            pages: None,
            at: 0,
            field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
        },
    );
    assert!(matches!(bad, Err(EditError::Source(_))));
}

/// A one-page PDF whose form holds one text field named `name` (a widget that is its own field).
fn form_with_field(name: &str) -> Vec<u8> {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".into(),                   // 3
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /V (x) /Rect [60 60 200 80] /P 3 0 R >>"), // 4
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    bytes
}

/// Inserting pages of a file whose form names a field as the open document's: the incoming
/// Duplicating a page copies the document's own field: the copy keeps its name (Duplicate is
/// not Combine), and nothing about renaming lands in the repair log.
#[test]
fn duplicating_a_page_with_a_field_keeps_the_field_name() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("a.pdf", None, Arc::new(form_with_field("Text1")), None).unwrap();
    s.apply(id, Edit::DuplicatePages { pages: vec![0], at: None }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), 2);
    assert!(doc.form.iter().all(|f| f.name == "Text1"), "{:?}", doc.form.iter().map(|f| &f.name).collect::<Vec<_>>());
    assert!(!doc.repair_log().iter().any(|l| l.contains("Text1-1")), "{:?}", doc.repair_log());
}

/// field is renamed to keep the two separate, the repair log says so (warning that scripts
/// were not rewritten), the undo step's label counts the renames, and undo puts the names back.
#[test]
fn insert_from_file_renames_colliding_fields_and_notes_it() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("a.pdf", None, Arc::new(form_with_field("Text1")), None).unwrap();
    s.apply(
        id,
        Edit::InsertPagesFrom {
            name: "b.pdf".into(),
            bytes: Arc::new(form_with_field("Text1")),
            pages: None,
            at: 1,
            field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
        },
    )
    .unwrap();
    let doc = s.get(id).unwrap();
    let names: Vec<&str> = doc.form.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Text1", "Text1-1"]);
    assert!(
        doc.repair_log().iter().any(|l| l.contains("Text1 → Text1-1") && l.contains("were not rewritten")),
        "the note names the rename and the scripts caveat: {:?}",
        doc.repair_log()
    );
    let label = s.undo(id).unwrap();
    assert_eq!(label.as_str(), "Insert pages from b.pdf — 1 field renamed");
    let doc = s.get(id).unwrap();
    let names: Vec<&str> = doc.form.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Text1"], "undo restores the single original field");
}

/// A combine reports the fields it renamed — the tool result tells callers what their scripts
/// must call the field now — and honours a caller's choice to merge instead.
#[test]
fn combine_reports_renamed_fields_and_can_merge() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let sources: Vec<CombineSource> =
        vec![("a.pdf".into(), Arc::new(form_with_field("Text1")), None), ("b.pdf".into(), Arc::new(form_with_field("Text1")), None)];
    let out = s.combine_unlocked_reporting(&sources, &[], pdfcraft_organize::FieldConflicts::Rename).unwrap();
    assert_eq!(out.renamed_fields, [("Text1".to_string(), "Text1-1".to_string())]);
    let cid = s.open_new("Combined.pdf", out.bytes).unwrap();
    let doc = s.get(cid).unwrap();
    let names: Vec<&str> = doc.form.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Text1", "Text1-1"]);
    let merged = s.combine_unlocked_reporting(&sources, &[], pdfcraft_organize::FieldConflicts::Merge).unwrap();
    assert!(merged.renamed_fields.is_empty(), "merge renames nothing");
}

/// The Combine tab's pre-flight over byte sources: what combining them, in order, would rename
/// in each. A file that cannot be opened previews as nothing (its row already shows the problem).
#[test]
fn combine_field_conflicts_previews_over_bytes() {
    let s = Session::new().with_clock(|| 1_700_000_000);
    let sources: Vec<CombineSource> = vec![
        ("a.pdf".into(), Arc::new(form_with_field("Text1")), None),
        ("b.pdf".into(), Arc::new(form_with_field("Text1")), None),
        ("junk.pdf".into(), Arc::new(b"nope".to_vec()), None),
    ];
    let preview = s.combine_field_conflicts(&sources, &[]);
    assert_eq!(preview, vec![vec![], vec![("Text1".to_string(), "Text1-1".to_string())], vec![]]);
}

/// A minimal baseline JPEG (headers only; the data is embedded as is).
fn jpeg_bytes() -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend_from_slice(&[0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 1, 0x01, 0x2C, 0x01, 0x2C, 0, 0]);
    v.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8, 0, 2, 0, 3, 3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

#[test]
fn mixed_files_convert_and_combine_in_order() {
    let mut s = Session::new();
    let pdf = Arc::new(fixture(2));
    let (kind, same) = s.convert_to_pdf("a.pdf", &pdf).unwrap();
    assert_eq!(kind, SourceKind::Pdf);
    assert!(Arc::ptr_eq(&same, &pdf), "a PDF is passed through untouched");
    let (kind, image) = s.convert_to_pdf("scan.jpg", &Arc::new(jpeg_bytes())).unwrap();
    assert_eq!(kind, SourceKind::Image);
    let (kind, text) = s.convert_to_pdf("notes.txt", &Arc::new(b"hello".to_vec())).unwrap();
    assert_eq!(kind, SourceKind::Text);
    let combined = s.combine_ranges(&[("notes".into(), text, None), ("a".into(), pdf, Some("2".into())), ("scan".into(), image, None)]).unwrap();
    let id = s.open_new("Combined.pdf", combined).unwrap();
    assert_eq!(page_texts(&s, id), ["hello", "Page 2", ""]);
    assert_eq!(outline_titles(&s.get(id).unwrap().info.outline), ["notes→1", "a→2", "scan→3"]);
}

#[test]
fn utf16_and_code_page_text_files_convert_to_their_text() {
    let mut s = Session::new();
    // Notepad's "Unicode": UTF-16 little endian with a byte order mark.
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend("Grüße aus Köln".encode_utf16().flat_map(u16::to_le_bytes));
    let (_, pdf) = s.convert_to_pdf("notes.txt", &Arc::new(utf16)).unwrap();
    let id = s.open_new("notes.pdf", pdf).unwrap();
    assert_eq!(page_texts(&s, id), ["Grüße aus Köln"]);
    // A UTF-8 byte order mark is not text; Windows-1252 bytes are not replacement characters.
    let (_, pdf) = s.convert_to_pdf("menu.txt", &Arc::new(b"\xEF\xBB\xBFCaf\xC3\xA9".to_vec())).unwrap();
    let id = s.open_new("menu.pdf", pdf).unwrap();
    assert_eq!(page_texts(&s, id), ["Café"]);
    let (_, pdf) = s.convert_to_pdf("old.txt", &Arc::new(b"Cr\xE8me br\xFBl\xE9e".to_vec())).unwrap();
    let id = s.open_new("old.pdf", pdf).unwrap();
    assert_eq!(page_texts(&s, id), ["Crème brûlée"]);
}

#[test]
fn a_file_split_around_another_combines_in_order_with_one_bookmark() {
    let mut s = Session::new();
    let (a, b) = (Arc::new(fixture(3)), Arc::new(fixture(2)));
    // a's pages 1–2, then b, then a's page 3: one file (group 7) split around another.
    let sources: Vec<CombineSource> = vec![("a".into(), a.clone(), Some("1-2".into())), ("b".into(), b, None), ("a".into(), a, Some("3".into()))];
    let combined = s.combine_grouped(&sources, &[7, 9, 7], &[]).unwrap();
    let id = s.open_new("Combined.pdf", combined).unwrap();
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2", "Page 1", "Page 2", "Page 3"]);
    assert_eq!(outline_titles(&s.get(id).unwrap().info.outline), ["a→1", "b→3"], "one bookmark for a, at its first page");
    // Without groups each source is a file of its own, as combine_ranges.
    let (a, b) = (Arc::new(fixture(3)), Arc::new(fixture(2)));
    let sources: Vec<CombineSource> = vec![("a".into(), a.clone(), Some("1".into())), ("b".into(), b, None), ("a".into(), a, Some("3".into()))];
    let combined = s.combine_grouped(&sources, &[], &[]).unwrap();
    let id = s.open_new("Combined 2.pdf", combined).unwrap();
    assert_eq!(outline_titles(&s.get(id).unwrap().info.outline), ["a→1", "b→2", "a→4"]);
    // A bad range is refused with the file's name.
    let bad: Vec<CombineSource> = vec![("a".into(), Arc::new(fixture(1)), Some("9".into()))];
    assert!(matches!(s.combine_grouped(&bad, &[1], &[]), Err(EditError::Print(e)) if e.starts_with("a:")));
}

#[test]
fn create_multiple_reads_each_source_only_after_the_previous_one_is_released() {
    let mut s = Session::new();
    let mut earlier: Vec<std::sync::Weak<Vec<u8>>> = Vec::new();
    let texts = ["Alpha", "Beta", "Gamma"];
    let sources = texts.iter().map(|text| {
        // The combine has finished with every earlier source, so their bytes are gone by now.
        assert!(earlier.iter().all(|w| w.upgrade().is_none()), "a source is released before the next is read");
        let (_, pdf) = s.convert_to_pdf(&format!("{text}.txt"), &Arc::new(text.as_bytes().to_vec())).unwrap();
        earlier.push(Arc::downgrade(&pdf));
        Ok::<CombineSource, EditError>((text.to_string(), pdf, None))
    });
    let combined = s.combine_each(sources).unwrap().unwrap();
    assert!(earlier.iter().all(|w| w.upgrade().is_none()));
    let id = s.open_new("Combined.pdf", combined).unwrap();
    assert_eq!(page_texts(&s, id), ["Alpha", "Beta", "Gamma"]);
    assert_eq!(outline_titles(&s.get(id).unwrap().info.outline), ["Alpha→1", "Beta→2", "Gamma→3"]);
}

#[test]
fn create_multiple_gives_the_bytes_of_combine_ranges_and_refuses_a_bad_source() {
    // The two combines are compared byte for byte, and each stamps the clock: pin it.
    let s = Session::new().with_clock(|| 1_700_000_000);
    let (a, b) = (Arc::new(fixture(3)), Arc::new(fixture(2)));
    let sources: Vec<CombineSource> =
        vec![("a".into(), a.clone(), Some("1-2".into())), ("b".into(), b, None), ("a".into(), a.clone(), Some("3".into()))];
    let by_ranges = s.combine_ranges(&sources).unwrap();
    let streamed = s.combine_each(sources.into_iter().map(Ok::<_, EditError>)).unwrap().unwrap();
    assert_eq!(*streamed, *by_ranges);
    // A bad range, or a file that isn't a PDF, stops the combine with an error naming that source.
    let bad_range: Vec<Result<CombineSource, EditError>> = vec![Ok(("a".into(), a.clone(), None)), Ok(("c".into(), a.clone(), Some("9".into())))];
    assert!(matches!(s.combine_each(bad_range), Err(EditError::Print(e)) if e.starts_with("c:")));
    let junk: Vec<Result<CombineSource, EditError>> = vec![Ok(("junk".into(), Arc::new(b"not a pdf".to_vec()), None))];
    assert!(matches!(s.combine_each(junk), Err(EditError::Source(m)) if m.starts_with("junk:")));
    // Nothing to combine is `None`, not an empty document.
    assert!(matches!(s.combine_each(Vec::<Result<CombineSource, EditError>>::new()), Ok(None)));
}

#[test]
fn a_page_with_a_form_field_shown_twice_keeps_one_field() {
    // Regression (found by review): the repeated page copied the whole field again, so the
    // result had two fields named "shared", filled in separately.
    use pdfcraft_cos::{Dict, Document, Object, PdfString};
    let mut doc = Document::open(Arc::new(fixture(2))).unwrap();
    let pages: Vec<_> = pdfcraft_organize::pages(&doc).unwrap().into_iter().map(|p| p.obj).collect();
    let field = doc.add(Object::Null);
    let widgets: Vec<_> = pages
        .iter()
        .map(|p| {
            let mut w = Dict::new();
            w.set(b"Type".to_vec(), Object::name("Annot"));
            w.set(b"Subtype".to_vec(), Object::name("Widget"));
            w.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(100), Object::Int(20)]));
            w.set(b"Parent".to_vec(), Object::Ref(field));
            w.set(b"P".to_vec(), Object::Ref(*p));
            doc.add(w)
        })
        .collect();
    let mut f = Dict::new();
    f.set(b"FT".to_vec(), Object::name("Tx"));
    f.set(b"T".to_vec(), Object::String(PdfString::text("shared")));
    f.set(b"V".to_vec(), Object::String(PdfString::text("Ada")));
    f.set(b"Kids".to_vec(), Object::Array(widgets.iter().copied().map(Object::Ref).collect()));
    doc.set(field, Object::Dict(f));
    for (p, w) in pages.iter().zip(&widgets) {
        doc.update_dict(*p, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(*w)]))).unwrap();
    }
    let mut form = Dict::new();
    form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(field)]));
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();
    let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
    // Pages 1, 2, then page 1 again: one file split around nothing but itself.
    let s = Session::new();
    let sources: Vec<CombineSource> = vec![("form".into(), bytes.clone(), Some("1-2".into())), ("form".into(), bytes, Some("1".into()))];
    let out = s.combine_grouped(&sources, &[1, 1], &[]).unwrap();
    let mut out = Document::open(out).unwrap();
    pdfcraft_forms::set_value(&mut out, "shared", &pdfcraft_forms::FieldValue::Text("Grace".into())).unwrap();
    let fields = pdfcraft_forms::fields(&out);
    assert!(
        !fields.is_empty() && fields.iter().all(|f| f.name == "shared" && f.value == ["Grace"]),
        "one field, one value: {:?}",
        fields.iter().map(|f| (&f.name, &f.value)).collect::<Vec<_>>()
    );
    let catalog = out.get(out.root().unwrap()).as_dict().cloned().unwrap();
    let form = out.resolve(catalog.get(b"AcroForm").unwrap()).as_dict().cloned().unwrap();
    let listed = form.get(b"Fields").and_then(|f| f.as_array()).cloned().unwrap();
    assert_eq!(listed.len(), 1, "the field isn't copied again for the repeated page");
    // Its widgets: one on each of the three pages, each belonging to its own page.
    let out_pages: Vec<_> = pdfcraft_organize::pages(&out).unwrap().into_iter().map(|p| p.obj).collect();
    let kids = out.resolve(&listed[0]).as_dict().and_then(|f| f.get(b"Kids").and_then(|k| k.as_array()).cloned()).unwrap();
    assert_eq!(kids.len(), 3);
    let mut on: Vec<_> = kids.iter().map(|k| out.resolve(k).as_dict().and_then(|w| w.reference(b"P")).unwrap()).collect();
    on.sort();
    let mut expected = out_pages.clone();
    expected.sort();
    assert_eq!(on, expected, "a widget per page, on its own page");
}

#[test]
fn files_that_cannot_be_converted_are_refused_clearly() {
    let s = Session::new();
    let err = s.convert_to_pdf("report.docx", &Arc::new(b"PK\x03\x04".to_vec())).unwrap_err();
    assert_eq!(err, EditError::Source("report.docx: this file type can't be converted; use a PDF, an image or a .txt file".into()));
    assert!(matches!(s.convert_to_pdf("empty", &Arc::new(Vec::new())), Err(EditError::Source(_))));
    // Damaged inputs of a known type fail with an error, not a panic.
    assert!(s.convert_to_pdf("bad.png", &Arc::new(b"\x89PNG\r\n\x1a\nnope".to_vec())).is_err());
    assert!(s.convert_to_pdf("bad.jpg", &Arc::new(vec![0xFF, 0xD8, 0xFF])).is_err());
    assert!(s.convert_to_pdf("bad.pdf", &Arc::new(b"%PDF-1.7 nope".to_vec())).is_err());
    let err = s.convert_to_pdf("locked.pdf", &protected("pw", "o", -1)).unwrap_err();
    assert_eq!(err, EditError::Source("locked.pdf: it is password-protected".into()));
}

/// Combine files (#569): images and text become PDFs; PDFs pass through unread, even protected
/// ones (Combine asks for their password) and ones named .pdf without a header.
#[test]
fn files_for_combine_are_made_into_pdfs_and_pdfs_pass_through() {
    let s = Session::new();
    let locked = protected("pw", "o", -1);
    assert!(Arc::ptr_eq(&s.as_pdf("locked.pdf", &locked).unwrap(), &locked));
    let headless = Arc::new(b"1 0 obj << >> endobj".to_vec());
    assert!(Arc::ptr_eq(&s.as_pdf("old.PDF", &headless).unwrap(), &headless));
    assert!(s.as_pdf("scan.jpg", &Arc::new(jpeg_bytes())).unwrap().starts_with(b"%PDF-"));
    assert!(s.as_pdf("notes.txt", &Arc::new(b"hello".to_vec())).unwrap().starts_with(b"%PDF-"));
    let err = s.as_pdf("report.docx", &Arc::new(b"PK\x03\x04".to_vec())).unwrap_err();
    assert!(matches!(err, EditError::Source(m) if m.starts_with("report.docx: this file type can't be converted")));
}

fn protected(user: &str, owner: &str, permissions: i32) -> Arc<Vec<u8>> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(2))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: user,
        owner_password: owner,
        permissions,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap())
}

#[test]
fn encrypted_documents_open_edit_and_save_encrypted() {
    let bytes = protected("pw", "owner", -1);
    let mut s = Session::new();
    assert!(s.open("x.pdf", None, bytes.clone(), None).is_err(), "needs a password");
    let id = s.open("x.pdf", None, bytes.clone(), Some("pw")).unwrap();
    assert!(s.get(id).unwrap().editable(), "{:?}", s.get(id).unwrap().read_only_reason);
    assert_eq!(s.get(id).unwrap().security_summary().unwrap().method, "AES, 256-bit");
    assert!(s.get(id).unwrap().info.encrypted, "reported as encrypted");
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    assert_eq!(&saved[..bytes.len()], &bytes[..], "incremental");
    assert!(pdfcraft_cos::Document::open(saved.clone()).is_err(), "still protected after saving");
    let mut s2 = Session::new();
    let id2 = s2.open("x.pdf", None, saved, Some("pw")).unwrap();
    assert_eq!(s2.get(id2).unwrap().info.pages[0].rotation, 90);
}

#[test]
fn restricted_documents_refuse_changes_unless_opened_by_the_owner() {
    let bytes = protected("", "owner", 0b0100); // print only, no password to open
    let mut s = Session::new();
    let id = s.open("r.pdf", None, bytes.clone(), None).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.allows_assembly() && !d.allows_modification());
    let summary = d.security_summary().unwrap();
    assert!(!summary.owner && summary.permissions.print() && !summary.permissions.copy());
    assert_eq!(s.apply(id, Edit::DeletePages { pages: vec![0] }), Err(EditError::NotPermitted("page changes")));
    assert_eq!(s.apply(id, Edit::SetInfo { key: "Title".into(), value: "x".into() }), Err(EditError::NotPermitted("changes to the document")));
    assert!(matches!(s.extract(id, &[0]), Err(EditError::NotPermitted(_))));
    assert!(!s.get(id).unwrap().dirty);
    // The owner password lifts the restrictions.
    let oid = s.open("r.pdf", None, bytes, Some("owner")).unwrap();
    assert!(s.get(oid).unwrap().allows_assembly());
    s.apply(oid, Edit::DeletePages { pages: vec![0] }).unwrap();
}

/// Table 22, bit 5: "Copy or otherwise extract text and graphics" (#568). Snapshots, exports and
/// Save Image As are refused, other permissions are untouched, and the owner password lifts it.
#[test]
fn copy_restricted_documents_refuse_extraction_unless_opened_by_the_owner() {
    let bytes = protected("", "owner", -1 ^ 0b1_0000); // everything but copying, no password to open
    let mut s = Session::new();
    let id = s.open("r.pdf", None, bytes.clone(), None).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.allows_copying());
    assert!(d.allows_printing() && d.allows_modification() && d.allows_assembly(), "only copying is withheld");
    assert_eq!(d.check_copying(), Err(EditError::NotPermitted("copying text and images")));
    assert!(d.page_image_file(0, 0).unwrap_err().contains("don't allow copying"));
    let extracting = ["edit.snapshot", "export.image", "export.text", "export.docx", "export.pptx", "export.html", "export.rtf", "export.all_images"];
    let enabled = |s: &Session, id, c| commands::is_enabled(commands::command(c).unwrap(), s, Some(id));
    for c in extracting {
        assert!(!enabled(&s, id, c), "{c} is disabled");
    }
    assert!(enabled(&s, id, "edit.find") && enabled(&s, id, "a11y.check"), "searching and accessibility don't copy");
    assert!(s.get(id).unwrap().autotag_preview(0).is_ok(), "like a11y.check, the auto-tag preview is allowed");
    let oid = s.open("r.pdf", None, bytes, Some("owner")).unwrap();
    assert!(s.get(oid).unwrap().allows_copying());
    assert!(extracting.iter().all(|c| enabled(&s, oid, c)));
    // Comparing reads both documents' text out, into the result, the report and the comments.
    let refused = || Err(EditError::NotPermitted("copying text and images"));
    assert_eq!((s.compare(id, oid).map(|_| ()), s.compare(oid, id).map(|_| ())), (refused(), refused()));
    assert_eq!(s.compare_report(oid, id).map(|_| ()), refused());
    assert!(s.compare(oid, oid).is_ok());
    assert_eq!(s.mark_differences(id, oid).map(|_| ()), refused());
    // Unencrypted documents allow everything.
    let (s, plain) = session_with(1);
    assert!(s.get(plain).unwrap().check_copying().is_ok() && extracting.iter().all(|c| enabled(&s, plain, c)));
}

/// Table 22, bit 3 (#2669): a document that forbids printing disables Print up front instead
/// of refusing once the dialog is confirmed. Low-resolution printing (bit 3 on, bit 12 off) is
/// still printing, and the owner password lifts the restriction.
#[test]
fn print_restricted_documents_disable_print_unless_opened_by_the_owner() {
    let enabled = |s: &Session, id, c| commands::is_enabled(commands::command(c).unwrap(), s, Some(id));
    let bytes = protected("", "owner", -1 ^ 0b100);
    let mut s = Session::new();
    let id = s.open("r.pdf", None, bytes.clone(), None).unwrap();
    assert!(!s.get(id).unwrap().allows_printing());
    assert!(!enabled(&s, id, "print.dialog"));
    assert!(enabled(&s, id, "export.text"), "only printing is withheld");
    let oid = s.open("r.pdf", None, bytes, Some("owner")).unwrap();
    assert!(enabled(&s, oid, "print.dialog"), "the owner may print");
    let low = s.open("low.pdf", None, protected("", "owner", -1 ^ (1 << 11)), None).unwrap();
    assert!(enabled(&s, low, "print.dialog"), "low-resolution printing is printing");
}

/// Table 22, bit 12 clear: printing is limited to a low-level representation. Whatever the
/// settings ask for, the print-ready PDF carries each page as a picture of at most 150 dpi, with
/// no text or vector artwork left; the owner password lifts the limit.
#[test]
fn low_resolution_printing_prints_pictures_of_at_most_150_dpi() {
    let bytes = protected("", "owner", -1 ^ (1 << 11));
    let mut s = Session::new();
    let low = s.open("low.pdf", None, bytes.clone(), None).unwrap();
    let width = f64::from(s.get(low).unwrap().info.pages[0].width);
    let actual = print::Settings { pages: vec![0], layout: print::Layout::Size(print::SizeMode::Actual), ..Default::default() };
    for raster in [None, Some(print::Raster { dpi: 600.0, grayscale: false })] {
        let printed = s.print_pdf(low, &print::Settings { raster, ..actual.clone() }).unwrap();
        let mut s2 = Session::new();
        let id2 = s2.open("print.pdf", None, Arc::new(printed.clone()), None).unwrap();
        assert!(page_texts(&s2, id2).iter().all(|t| t.trim().is_empty()), "{raster:?}: no text is left");
        let doc = pdfcraft_cos::Document::open(Arc::new(printed)).unwrap();
        let widths: Vec<i64> = doc
            .scan_objects()
            .filter_map(|(_, o)| match &*o {
                pdfcraft_cos::Object::Stream(st) if st.dict.name(b"Subtype") == Some(b"Image") => st.dict.int(b"Width"),
                _ => None,
            })
            .collect();
        assert!(!widths.is_empty() && widths.iter().all(|&w| w as f64 <= width * 150.0 / 72.0 + 1.0), "{raster:?}: {widths:?} px for {width} pt");
    }
    let owner = s.open("low.pdf", None, bytes, Some("owner")).unwrap();
    let printed = s.print_pdf(owner, &actual).unwrap();
    let mut s2 = Session::new();
    let id2 = s2.open("print.pdf", None, Arc::new(printed), None).unwrap();
    assert!(page_texts(&s2, id2).iter().any(|t| !t.trim().is_empty()), "the owner prints the pages themselves");
}

/// A 100 × 400 pt page with one band per kind of thing that prints, top to bottom: page content
/// (blue), a filled text field (green), a stamp (red) and a square comment (yellow).
fn print_content_pdf() -> Vec<u8> {
    let content = "0 0 1 rg 0 300 100 100 re f";
    let ap = |colour: &str| {
        let ops = format!("{colour} rg 0 0 100 100 re f");
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >> stream\n{ops}\nendstream", ops.len())
    };
    format!(
        "%PDF-1.7\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 400] /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R] >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Filled) /F 4 /P 3 0 R /Rect [0 200 100 300] /AP << /N 8 0 R >> >> endobj\n\
         6 0 obj << /Type /Annot /Subtype /Stamp /F 4 /P 3 0 R /Rect [0 100 100 200] /AP << /N 9 0 R >> >> endobj\n\
         7 0 obj << /Type /Annot /Subtype /Square /F 4 /P 3 0 R /Rect [0 0 100 100] /AP << /N 10 0 R >> >> endobj\n\
         8 0 obj {} endobj\n\
         9 0 obj {} endobj\n\
         10 0 obj {} endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        content.len(),
        ap("0 1 0"),
        ap("1 0 0"),
        ap("1 1 0"),
    )
    .into_bytes()
}

/// Low-resolution printing prints what the chosen content prints when the pages themselves are
/// printed: form fields only leaves out the page content and every comment, the document alone
/// leaves out the comments, and the document with stamps keeps only the stamps.
#[test]
fn low_resolution_printing_prints_only_the_chosen_content() {
    let mut cos = pdfcraft_cos::Document::open(Arc::new(print_content_pdf())).unwrap();
    cos.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ (1 << 11),
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let bytes = Arc::new(pdfcraft_cos::write_full(&cos, &Default::default()).unwrap());
    let mut s = Session::new();
    let low = s.open("low.pdf", None, bytes.clone(), None).unwrap();
    let owner = s.open("low.pdf", None, bytes, Some("owner")).unwrap();
    // Each band's centre, in pixels from the top of the sheet at 72 dpi, and its colour.
    let bands = [("content", 50, [0, 0, 255]), ("field", 150, [0, 255, 0]), ("stamp", 250, [255, 0, 0]), ("comment", 350, [255, 255, 0])];
    let all: &[&str] = &["content", "field", "stamp", "comment"];
    for (content, shown) in [
        (print::Content::DocumentAndMarkups, all),
        (print::Content::Document, &["content", "field"][..]),
        (print::Content::DocumentAndStamps, &["content", "field", "stamp"][..]),
        (print::Content::FormFieldsOnly, &["field"][..]),
    ] {
        let settings = print::Settings {
            pages: vec![0],
            paper: (100.0, 400.0),
            layout: print::Layout::Size(print::SizeMode::Actual),
            content,
            ..Default::default()
        };
        for (id, how) in [(owner, "pages"), (low, "pictures")] {
            let (w, h, rgba) = printed_pixels(s.print_pdf(id, &settings).unwrap());
            assert_eq!((w, h), (100, 400), "{content:?} as {how}");
            for (band, y, colour) in bands {
                let px = rgba.get((y * w as usize + 50) * 4..).and_then(|p| p.get(..3)).unwrap();
                let near = |c: [u8; 3]| px.iter().zip(c).all(|(&a, b)| a.abs_diff(b) < 40);
                let expected = shown.contains(&band);
                assert!(if expected { near(colour) } else { near([255; 3]) }, "{content:?} as {how}: {band} drawn {expected} expected, pixel {px:?}");
            }
        }
    }
}

#[test]
fn the_permissions_password_unlocks_an_open_document() {
    // Only a permissions password (#995, #1077): opens without asking. Changes are allowed,
    // security changes aren't.
    let bytes = protected("", "owner", 0b1100);
    let mut s = Session::new();
    let id = s.open("r.pdf", None, bytes, None).unwrap();
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Before".into() }).unwrap();
    assert!(!s.get(id).unwrap().allows_security_change());
    assert!(matches!(s.apply(id, Edit::RemoveProtection), Err(EditError::NotPermitted(_))));
    for wrong in ["", "nope"] {
        assert_eq!(s.unlock(id, wrong), Err(EditError::WrongPassword), "{wrong:?}");
    }
    assert!(!s.get(id).unwrap().allows_security_change());
    s.unlock(id, "owner").unwrap();
    let d = s.get(id).unwrap();
    assert!(d.allows_security_change() && d.security_summary().unwrap().owner);
    assert!(d.dirty, "unlocking leaves the unsaved edit alone");
    assert_eq!(d.can_undo(), Some("Change Title"), "and adds no undo step");
    // Undo and redo don't bring the restrictions back.
    s.undo(id).unwrap();
    assert!(s.get(id).unwrap().allows_security_change(), "after undo");
    s.redo(id).unwrap();
    assert!(s.get(id).unwrap().allows_security_change(), "after redo");
    // Neither does saving.
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    assert!(s.get(id).unwrap().allows_security_change(), "after saving");
    s.apply(id, Edit::RemoveProtection).unwrap();
    let plain = s.save_bytes(id).unwrap();
    assert!(pdfcraft_cos::Document::open(plain).unwrap().security().is_none(), "security removed");
}

#[test]
fn remove_security_from_a_file_with_its_encrypt_dictionary_in_the_trailer() {
    // MuPDF writes /Encrypt inline in the trailer rather than as an object (#2661).
    let mut doc = pdfcraft_cos::Document::open_with_password(protected("user", "owner", -1), Some("owner")).unwrap();
    let r = doc.trailer().reference(b"Encrypt").unwrap();
    let dict = doc.get(r).as_dict().cloned().unwrap();
    doc.trailer_mut().set(b"Encrypt".to_vec(), pdfcraft_cos::Object::Dict(dict));
    let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
    let mut s = Session::new();
    let id = s.open("m.pdf", None, bytes, Some("owner")).unwrap();
    s.apply(id, Edit::RemoveProtection).unwrap();
    assert!(!s.get(id).unwrap().info.encrypted);
    let plain = s.save_bytes(id).unwrap();
    assert!(pdfcraft_cos::Document::open(plain).unwrap().security().is_none(), "security removed");
}

#[test]
fn unlocking_needs_the_permissions_password_not_the_open_password() {
    let mut s = Session::new();
    let id = s.open("r.pdf", None, protected("open", "perms", 0b0100), Some("open")).unwrap();
    assert_eq!(s.unlock(id, "open"), Err(EditError::WrongPassword));
    s.unlock(id, "perms").unwrap();
    assert!(s.get(id).unwrap().allows_security_change());
    // Nothing to unlock: opened by the owner already, or not encrypted.
    s.unlock(id, "anything").unwrap();
    let plain = s.open("p.pdf", None, Arc::new(fixture(1)), None).unwrap();
    s.unlock(plain, "anything").unwrap();
}

#[test]
fn combining_protected_files_is_refused_clearly() {
    let s = Session::new();
    let err = s.combine(&[("a.pdf".into(), protected("pw", "o", -1)), ("b.pdf".into(), Arc::new(fixture(1)))]).unwrap_err();
    assert_eq!(err, EditError::Source("a.pdf: it is password-protected".into()));
    let err = s.combine(&[("c.pdf".into(), protected("", "o", 0b0100))]).unwrap_err();
    assert!(matches!(err, EditError::Source(m) if m.contains("don't allow copying pages")));
}

#[test]
fn owner_password_of_older_revisions_opens_the_viewer_too() {
    for alg in [pdfcraft_cos::Algorithm::Rc4_128, pdfcraft_cos::Algorithm::Aes128] {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: alg,
            user_password: "u",
            owner_password: "o",
            permissions: 0,
            encrypt_metadata: true,
            seed: [1; 32],
        })
        .unwrap();
        let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
        let mut s = Session::new();
        let id = s.open("x.pdf", None, bytes, Some("o")).unwrap_or_else(|e| panic!("{alg:?}: {e}"));
        let d = s.get(id).unwrap();
        assert!(d.security_summary().unwrap().owner && d.allows_modification(), "{alg:?}");
        assert_eq!(page_texts(&s, id), ["Page 1"], "{alg:?}: the renderer reads it");
    }
}

/// Render paths outside a session (the CLI's `info`, `text`, `render`) get the password the
/// renderer takes: the recovered user password for an R2–R4 owner password, else as given.
#[test]
fn render_password_for_recovers_the_user_password_of_older_revisions() {
    for alg in [pdfcraft_cos::Algorithm::Rc4_40, pdfcraft_cos::Algorithm::Rc4_128, pdfcraft_cos::Algorithm::Aes128, pdfcraft_cos::Algorithm::Aes256] {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: alg,
            user_password: "u",
            owner_password: "o",
            permissions: 0,
            encrypt_metadata: true,
            seed: [1; 32],
        })
        .unwrap();
        let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
        let owner = crate::render_password_for(bytes.clone(), Some("o"));
        let expected = if alg == pdfcraft_cos::Algorithm::Aes256 { "o" } else { "u" };
        assert_eq!(owner.as_deref(), Some(expected), "{alg:?}");
        assert!(pdfcraft_render::inspect(bytes.clone(), owner.as_deref()).is_ok(), "{alg:?}: the renderer opens it");
        assert_eq!(crate::render_password_for(bytes.clone(), Some("u")).as_deref(), Some("u"), "{alg:?}");
        assert_eq!(crate::render_password_for(bytes.clone(), Some("wrong")).as_deref(), Some("wrong"), "{alg:?}");
        assert_eq!(crate::render_password_for(bytes, None), None, "{alg:?}");
    }
}

#[test]
fn autosave_snapshots_only_changed_documents() {
    let (mut s, id) = session_with(2);
    assert!(!s.has_pending_autosave());
    assert!(s.autosave_snapshots().is_empty(), "clean documents are not snapshotted");
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(s.has_pending_autosave());
    assert!(s.has_pending_autosave(), "checking a deadline must not consume a snapshot");
    let snaps = s.autosave_snapshots();
    assert!(!s.has_pending_autosave());
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].bytes, s.get(id).unwrap().bytes, "the working file");
    assert!(s.autosave_snapshots().is_empty(), "nothing new since the last snapshot");
    s.apply(id, Edit::DeletePages { pages: vec![1] }).unwrap();
    assert_eq!(s.autosave_snapshots().len(), 1);
    s.undo(id).unwrap();
    assert!(s.get(id).unwrap().dirty, "the rotation is still unsaved");
    assert_eq!(s.autosave_snapshots().len(), 1, "undo that leaves unsaved work is a change");
    s.undo(id).unwrap();
    assert!(!s.get(id).unwrap().dirty, "undo back to the opened file is saved");
    assert!(s.autosave_snapshots().is_empty(), "a clean document needs no recovery");
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    assert!(s.autosave_snapshots().is_empty(), "saved documents need no recovery");
}

#[test]
fn autosave_snapshots_a_new_unsaved_document_once() {
    let (mut s, clean) = session_with(1);
    let id = s.open_new("Untitled.pdf", Arc::new(fixture(1))).unwrap();
    let snaps = s.autosave_snapshots();
    assert_eq!(snaps.len(), 1, "the new document is unsaved work");
    assert_eq!(snaps[0].doc, id);
    assert!(snaps.iter().all(|x| x.doc != clean), "the clean document stays out");
    assert!(s.autosave_snapshots().is_empty(), "nothing new since the last snapshot");
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert_eq!(s.autosave_snapshots().len(), 1, "a later edit is snapshotted again");
}

#[test]
fn recovered_documents_reopen_unsaved_at_their_original_path() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::DeletePages { pages: vec![1] }).unwrap();
    let snap = s.autosave_snapshots().remove(0);
    // A new session after a crash.
    let mut s2 = Session::new();
    let rid = s2.open(snap.name.clone(), None, snap.bytes.clone(), None).unwrap();
    s2.mark_recovered(rid, Some("/docs/report.pdf".into()));
    let d = s2.get(rid).unwrap();
    assert!(d.dirty);
    assert_eq!(d.path.as_deref(), Some("/docs/report.pdf"));
    assert_eq!(d.info.pages.len(), 1, "the edit survived");
    assert!(s2.autosave_snapshots().is_empty(), "already in the recovery store");
}

fn outline_titles(items: &[pdfcraft_render::OutlineItem]) -> Vec<String> {
    items
        .iter()
        .map(|o| {
            if o.children.is_empty() {
                format!("{}→{}", o.title, o.page.map_or(0, |p| p + 1))
            } else {
                format!("{}→{}[{}]", o.title, o.page.map_or(0, |p| p + 1), outline_titles(&o.children).join(","))
            }
        })
        .collect()
}

#[test]
fn bookmark_edits_show_in_the_viewer_undo_and_save() {
    let (mut s, id) = session_with(3);
    let titles = |s: &Session| outline_titles(&s.get(id).unwrap().info.outline);
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 0, title: "Start".into(), page: 0, left: None, top: None }).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 1, title: "End".into(), page: 2, left: None, top: None }).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![1], index: 0, title: "Détail".into(), page: 1, left: None, top: None }).unwrap();
    // The inspector (an independent parser) sees the same tree and destinations.
    assert_eq!(titles(&s), ["Start→1", "End→3[Détail→2]"]);
    s.apply(id, Edit::MoveBookmark { from: vec![1, 0], to_parent: vec![], index: 0 }).unwrap();
    s.apply(id, Edit::RenameBookmark { path: vec![2], title: "Finish".into() }).unwrap();
    s.apply(id, Edit::SetBookmarkPage { path: vec![1], page: 1 }).unwrap();
    assert_eq!(titles(&s), ["Détail→2", "Start→2", "Finish→3"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Set bookmark destination"));
    s.undo(id).unwrap();
    assert_eq!(titles(&s), ["Détail→2", "Start→1", "Finish→3"]);
    s.apply(id, Edit::DeleteBookmark { path: vec![0] }).unwrap();
    assert!(matches!(s.apply(id, Edit::DeleteBookmark { path: vec![7] }), Err(EditError::Bookmark(_))));

    let saved = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    let id2 = again.open("again.pdf", None, saved, None).unwrap();
    assert_eq!(outline_titles(&again.get(id2).unwrap().info.outline), ["Start→1", "Finish→3"]);
}

/// Two pages tagged with an H1 on the first and an H2 on the second.
fn tagged_headings_fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] /Resources << /Font << /F1 6 0 R >> >> >>",
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Length 52 >>\nstream\n/H1 << /MCID 0 >> BDC BT /F1 12 Tf (Intro) Tj ET EMC\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Type /StructTreeRoot /K [8 0 R 9 0 R] >>",
        "<< /S /H1 /Pg 3 0 R /K 0 >>",
        "<< /S /H2 /Pg 4 0 R /ActualText (Details) >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn bookmarks_from_structure_nest_the_headings_and_undo() {
    let mut s = Session::new();
    let id = s.open("tagged.pdf", None, Arc::new(tagged_headings_fixture()), None).unwrap();
    s.apply(id, Edit::BookmarksFromStructure).unwrap();
    assert_eq!(outline_titles(&s.get(id).unwrap().info.outline), ["Untitled→0[Intro→1[Details→2]]"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("New bookmarks from structure"));
    s.undo(id).unwrap();
    assert!(s.get(id).unwrap().info.outline.is_empty());

    // Untagged: a clear error and nothing changes.
    let (mut s, id) = session_with(2);
    let err = s.apply(id, Edit::BookmarksFromStructure).unwrap_err();
    assert_eq!(err.to_string(), "this document has no tagged headings to make bookmarks from");
    assert!(s.get(id).unwrap().info.outline.is_empty());
}

#[test]
fn number_pages_shows_in_the_viewer_and_undoes() {
    let (mut s, id) = session_with(5);
    let labels = |s: &Session| s.get(id).unwrap().info.pages.iter().map(|p| p.label.clone()).collect::<Vec<_>>();
    use pdfcraft_organize::LabelStyle;
    s.apply(id, Edit::NumberPages { from: 0, to: 1, style: LabelStyle::LowerRoman, prefix: String::new(), first: 1 }).unwrap();
    s.apply(id, Edit::NumberPages { from: 2, to: 4, style: LabelStyle::Decimal, prefix: "§".into(), first: 10 }).unwrap();
    // The inspector (lopdf-based, independent) formats them the same way.
    assert_eq!(labels(&s), ["i", "ii", "§10", "§11", "§12"]);
    s.undo(id).unwrap();
    assert_eq!(labels(&s), ["i", "ii", "3", "4", "5"]);
    assert!(s.apply(id, Edit::NumberPages { from: 3, to: 9, style: LabelStyle::Decimal, prefix: String::new(), first: 1 }).is_err());
}

/// RGBA of the pixel at PDF point (x, y) on `page`, rendered at 1 px/pt (page height 300).
fn pixel(s: &Session, id: DocId, page: usize, x: u32, y: u32) -> [u8; 4] {
    let doc = s.get(id).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
    let out = r.render(pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let i = (((300 - y) * out.width + x) * 4) as usize;
    out.rgba[i..i + 4].try_into().unwrap()
}

fn rect_comment(page: usize, rect: [f64; 4]) -> Edit {
    Edit::AddAnnotation(NewAnnotation {
        page,
        shape: Shape::Rectangle { rect },
        style: Style { color: [1.0, 0.0, 0.0], opacity: 1.0, width: 2.0, fill: Some([1.0, 0.0, 0.0]) },
        contents: "Look here".into(),
        author: "Reviewer".into(),
    })
}

#[test]
fn comments_are_added_drawn_threaded_and_undone() {
    let (mut s, id) = session_with(2);
    assert_eq!(pixel(&s, id, 1, 100, 100), [255, 255, 255, 255]);
    s.apply(id, rect_comment(1, [80.0, 80.0, 120.0, 120.0])).unwrap();
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add rectangle"));
    let px = pixel(&s, id, 1, 100, 100);
    assert!(px[0] > 200 && px[1] < 40 && px[2] < 40, "the rectangle is drawn: {px:?}");
    let a = &s.get(id).unwrap().info.annotations;
    assert_eq!(a.len(), 1);
    assert_eq!((a[0].page, a[0].index, a[0].author.as_deref(), a[0].contents.as_deref()), (1, 0, Some("Reviewer"), Some("Look here")));
    assert_eq!(a[0].modified.as_deref().map(|m| m.contains("2023")), Some(true), "dated by the session clock");
    let nm = a[0].name.clone().expect("has an /NM");
    assert_eq!(nm.len(), 36);

    s.apply(id, Edit::ReplyToAnnotation { page: 1, index: 0, text: "Done".into(), author: "Ada".into() }).unwrap();
    s.apply(id, Edit::SetAnnotationStatus { page: 1, index: 0, state: ReviewState::Completed, author: "Ada".into() }).unwrap();
    let a = &s.get(id).unwrap().info.annotations;
    assert_eq!(a.len(), 3);
    assert!(a.iter().filter(|r| r.in_reply_to.as_deref() == Some(nm.as_str())).count() == 2);
    assert!(a.iter().any(|r| r.state.as_deref() == Some("Completed")));
    assert_ne!(a[1].name, a[2].name, "every comment gets its own id");

    s.apply(id, Edit::MoveAnnotation { page: 1, index: 0, dx: 50.0, dy: 0.0 }).unwrap();
    assert_eq!(pixel(&s, id, 1, 100, 100), [255, 255, 255, 255]);
    let px = pixel(&s, id, 1, 150, 100);
    assert!(px[0] > 200 && px[1] < 40, "moved: {px:?}");

    s.apply(id, Edit::DeleteAnnotation { page: 1, index: 0 }).unwrap();
    assert!(s.get(id).unwrap().info.annotations.is_empty(), "replies go with their parent");
    for _ in 0..4 {
        s.undo(id).unwrap();
    }
    assert_eq!(s.get(id).unwrap().info.annotations.len(), 1);
    let px = pixel(&s, id, 1, 100, 100);
    assert!(px[0] > 200 && px[1] < 40, "back where it was: {px:?}");
}

#[test]
fn highlight_multiplies_over_text() {
    let (mut s, id) = session_with(1);
    // "Page 1" is drawn at 20,150 in 24 pt Helvetica.
    let quad = [18.0, 172.0, 100.0, 172.0, 18.0, 145.0, 100.0, 145.0];
    s.apply(
        id,
        Edit::AddAnnotation(NewAnnotation {
            page: 0,
            shape: Shape::TextMarkup { kind: Markup::Highlight, quads: vec![quad] },
            style: Style { color: [1.0, 1.0, 0.0], ..Style::default() },
            contents: String::new(),
            author: String::new(),
        }),
    )
    .unwrap();
    let a = &s.get(id).unwrap().info.annotations[0];
    assert_eq!(a.subtype, "Highlight");
    assert_eq!(a.quads, vec![quad.map(|v| v as f32)]);
    // Background inside the quad turns yellow; text stays dark (multiply).
    let bg = pixel(&s, id, 0, 19, 170);
    assert!(bg[0] > 240 && bg[1] > 240 && bg[2] < 30, "{bg:?}");
}

#[test]
fn comment_permission_is_enforced() {
    let mut cos = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    // Owner "own", empty user password, everything allowed except commenting (bit 6).
    let params = pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "own",
        permissions: !(1 << 5),
        encrypt_metadata: true,
        seed: [7; 32],
    };
    cos.set_encryption(&params).unwrap();
    let bytes = pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap();
    let mut s = Session::new();
    let id = s.open("locked.pdf", None, Arc::new(bytes), None).unwrap();
    let err = s.apply(id, rect_comment(0, [10.0, 10.0, 50.0, 50.0])).unwrap_err();
    assert_eq!(err, EditError::NotPermitted("comments"));
}

#[test]
fn saving_a_password_protected_document_rebases_on_it() {
    let bytes = protected("pw", "owner", -1);
    let mut s = Session::new();
    let id = s.open("x.pdf", None, bytes, Some("pw")).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved.clone(), None).expect("rebases on the encrypted file");
    assert!(!s.get(id).unwrap().dirty);
    s.apply(id, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
    let again = s.save_bytes(id).unwrap();
    assert_eq!(&again[..saved.len()], &saved[..], "the next save appends");
}

fn protection(open: Option<&str>, perms: Option<&str>) -> Protection {
    Protection { open_password: open.map(Into::into), permissions_password: perms.map(Into::into), ..Protection::default() }
}

#[test]
fn protect_with_an_open_password_then_save_reopen_and_undo() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::Protect(protection(Some("secret"), None))).unwrap();
    let d = s.get(id).unwrap();
    assert!(d.info.encrypted, "the working file is encrypted: {:?}", d.info.warnings);
    assert_eq!(d.security_summary().unwrap().method, "AES, 256-bit");
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2"], "still viewable in this session");
    let saved = s.save_bytes(id).unwrap();
    assert!(pdfcraft_cos::Document::open(saved.clone()).is_err(), "needs the password");
    s.mark_saved(id, saved.clone(), None).unwrap();
    // Further edits keep working and saving stays encrypted.
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let again = s.save_bytes(id).unwrap();
    let mut s2 = Session::new();
    assert!(s2.open("p.pdf", None, again.clone(), None).is_err());
    let id2 = s2.open("p.pdf", None, again, Some("secret")).unwrap();
    assert_eq!(s2.get(id2).unwrap().info.pages[0].rotation, 90);
    // Nothing is restricted without a permissions password: the opener may remove security.
    assert!(s2.get(id2).unwrap().allows_security_change());
    s2.apply(id2, Edit::RemoveProtection).unwrap();
    let plain = s2.save_bytes(id2).unwrap();
    assert!(pdfcraft_cos::Document::open(plain).is_ok());
}

#[test]
fn permissions_password_restricts_others_but_not_this_session() {
    let (mut s, id) = session_with(1);
    let p = Protection { printing: Printing::Low, changes: Changes::CommentFillSign, copy: false, ..protection(None, Some("boss")) };
    s.apply(id, Edit::Protect(p.clone())).unwrap();
    assert!(s.get(id).unwrap().allows_modification(), "the author keeps full rights");
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved.clone(), None).unwrap();
    assert!(s.get(id).unwrap().allows_modification(), "…also after saving (re-opened as owner)");
    // Someone else opens it without a password: restricted as chosen.
    let mut s2 = Session::new();
    let id2 = s2.open("r.pdf", None, saved.clone(), None).unwrap();
    let perm = s2.get(id2).unwrap().permissions().unwrap();
    assert!(perm.print() && !perm.print_high_quality() && perm.annotate() && perm.fill_forms() && !perm.copy() && !perm.modify());
    assert!(perm.extract_for_accessibility());
    assert!(!s2.get(id2).unwrap().allows_security_change());
    assert_eq!(s2.apply(id2, Edit::RemoveProtection), Err(EditError::NotPermitted("changing security")));
    assert!(s2.get(id2).unwrap().allows_annotation(), "commenting was allowed");
    // The owner password lifts everything.
    let id3 = s2.open("r.pdf", None, saved, Some("boss")).unwrap();
    assert!(s2.get(id3).unwrap().allows_security_change());
}

/// Converting to PDF/A takes the encryption off (PDF/A forbids it), so on an encrypted document
/// it needs the same rights as removing security, not just the right to make changes (#3921).
#[test]
fn convert_to_pdfa_needs_the_owners_rights_on_an_encrypted_document() {
    let pdfa = || Edit::ConvertPdfA { level: pdfcraft_preflight::Level::A2b };
    let refused = Err(EditError::NotPermitted("removing the encryption, which PDF/A forbids"));
    // Changes allowed; printing (bit 3) and copying (bit 5) withheld.
    let bytes = protected("pw", "owner", !((1 << 2) | (1 << 4)));
    let mut s = Session::new();
    let id = s.open("x.pdf", None, bytes.clone(), Some("pw")).unwrap();
    assert!(s.get(id).unwrap().allows_modification());
    assert_eq!(s.apply(id, pdfa()), refused);
    // Nor inside a batch.
    let batch = Edit::Batch { label: "x".into(), edits: vec![Edit::RotatePages { pages: vec![0], degrees: 90 }, pdfa()] };
    assert_eq!(s.apply(id, batch), refused);
    // Nothing was applied: no undo step, and saving gives back the encrypted original.
    assert!(s.get(id).unwrap().can_undo().is_none());
    assert_eq!(s.save_bytes(id).unwrap().as_slice(), bytes.as_slice());
    // The owner's password allows it, and the result is no longer encrypted.
    let id2 = s.open("x.pdf", None, bytes, Some("owner")).unwrap();
    s.apply(id2, pdfa()).unwrap();
    let saved = s.save_bytes(id2).unwrap();
    assert!(pdfcraft_cos::Document::open(saved).unwrap().trailer().get(b"Encrypt").is_none());
    // An unencrypted document needs nothing more.
    let (mut s, id) = session_with(1);
    s.apply(id, pdfa()).unwrap();
}

#[test]
fn combine_source_check_says_why_a_file_cannot_be_combined() {
    let (mut s, id) = session_with(1);
    let plain = s.get(id).unwrap().bytes.clone();
    assert_eq!(crate::combine_source_check(&plain, None), Ok(()));
    // Page assembly withheld (Changes::None) without a password to open it.
    let p = Protection { changes: Changes::None, ..protection(None, Some("boss")) };
    s.apply(id, Edit::Protect(p)).unwrap();
    let locked = s.save_bytes(id).unwrap();
    assert_eq!(crate::combine_source_check(&locked, None), Err(crate::SourceProblem::NotPermitted));
    let (mut s, id) = session_with(1);
    s.apply(id, Edit::Protect(protection(Some("pw"), Some("owner")))).unwrap();
    let secret = s.save_bytes(id).unwrap();
    assert_eq!(crate::combine_source_check(&secret, None), Err(crate::SourceProblem::Password));
    assert_eq!(crate::combine_source_check(&secret, Some("nope")), Err(crate::SourceProblem::WrongPassword));
    // Its open password reads it, but its permissions withhold assembling pages: the owner's
    // password allows it.
    assert_eq!(crate::combine_source_check(&secret, Some("pw")), Err(crate::SourceProblem::NotPermitted));
    assert_eq!(crate::combine_source_check(&secret, Some("owner")), Ok(()));
    // The permissions password lifts the restriction on copying pages.
    assert_eq!(crate::combine_source_check(&locked, Some("boss")), Ok(()));
    // Combining with the passwords: the result opens without any.
    let s = Session::new();
    let sources = vec![("secret".to_string(), secret, None), ("locked".to_string(), locked, None)];
    assert!(s.combine_ranges(&sources).is_err(), "no passwords, no combining");
    let out = s.combine_unlocked(&sources, &[Some("owner"), Some("boss")]).unwrap();
    let combined = pdfcraft_cos::Document::open(out).unwrap();
    assert!(combined.permissions().is_none(), "the combined file is not encrypted");
    assert_eq!(pdfcraft_organize::page_count(&combined).unwrap(), 2);
    assert!(matches!(crate::combine_source_check(&Arc::new(b"not a pdf".to_vec()), None), Err(crate::SourceProblem::Unreadable(_))));
}

#[test]
fn protection_is_validated_undoable_and_never_logged() {
    let (mut s, id) = session_with(1);
    assert!(matches!(s.apply(id, Edit::Protect(protection(None, None))), Err(EditError::Protection(_))));
    assert!(matches!(s.apply(id, Edit::Protect(protection(Some("same"), Some("same")))), Err(EditError::Protection(_))));
    let rc4 = Protection { algorithm: pdfcraft_cos::Algorithm::Rc4_128, ..protection(Some("pässword"), None) };
    assert!(matches!(s.apply(id, Edit::Protect(rc4)), Err(EditError::Protection(_))));
    assert!(!format!("{:?}", Edit::Protect(protection(Some("hunter2"), Some("x")))).contains("hunter2"));
    s.apply(id, Edit::Protect(protection(Some("pw"), Some("owner")))).unwrap();
    assert!(s.get(id).unwrap().info.encrypted);
    s.undo(id).unwrap();
    assert!(!s.get(id).unwrap().info.encrypted, "undo removes the pending protection");
    assert!(s.get(id).unwrap().security_summary().is_none());
    s.redo(id).unwrap();
    assert!(s.get(id).unwrap().info.encrypted);
    assert_eq!(page_texts(&s, id), ["Page 1"]);
}

/// Protect a fresh document with `p` as the app does: the edit's result and, when it is
/// accepted, how the saved file authenticates with no password, the open password and the
/// permissions password (`None`: it doesn't open).
fn protect_and_reopen(p: &Protection) -> (Result<(), EditError>, Option<[Option<pdfcraft_cos::Auth>; 3]>) {
    let (mut s, id) = session_with(1);
    let result = s.apply(id, Edit::Protect(p.clone()));
    let reopened = result.is_ok().then(|| {
        let saved = s.save_bytes(id).unwrap();
        let auth = |pw: Option<&str>| pdfcraft_cos::Document::open_with_password(saved.clone(), pw).ok()?.security().map(|h| h.auth());
        [auth(None), auth(p.open_password.as_deref()), auth(p.permissions_password.as_deref())]
    });
    (result, reopened)
}

#[test]
fn passwords_that_encryption_would_make_empty_or_equal_are_refused() {
    use pdfcraft_cos::Algorithm::{Aes128, Rc4_128};
    use pdfcraft_cos::Auth::{Owner, User};
    let empty = "passwords can't be empty or made only of characters that passwords ignore, such as soft hyphens";
    let same = "the open password and the permissions password must be different (PDF readers treat these two as the same password)";
    // AES-256 keys on the SASLprep form of a password (soft hyphens dropped, then NFKC), RC4 and
    // AES-128 on its first 32 bytes. Written as typed, these would make a file that opens
    // without a password, or whose open password lifts the restrictions.
    let (a31, a32) = ("a".repeat(31), "a".repeat(32));
    let (long1, long2) = (format!("{a32}1"), format!("{a32}2"));
    // The 32-byte padding starts with "(": 31 characters and the same followed by "(" are one key.
    let padded = format!("{a31}(");
    let cases = [
        ("soft hyphen open password", protection(Some("\u{AD}"), None), empty),
        ("soft hyphen permissions password", protection(None, Some("\u{AD}")), empty),
        ("soft hyphen permissions password beside an open one", protection(Some("0"), Some("\u{AD}")), empty),
        ("fullwidth A, then A", protection(Some("\u{FF21}"), Some("A")), same),
        ("A, then fullwidth A", protection(Some("A"), Some("\u{FF21}")), same),
        ("fi ligature, then fi", protection(Some("\u{FB01}"), Some("fi")), same),
        ("fi, then fi ligature", protection(Some("fi"), Some("\u{FB01}")), same),
        ("RC4-128, same first 32 bytes", Protection { algorithm: Rc4_128, ..protection(Some(&long1), Some(&long2)) }, same),
        ("AES-128, same first 32 bytes", Protection { algorithm: Aes128, ..protection(Some(&long1), Some(&long2)) }, same),
        ("RC4-128, same once padded", Protection { algorithm: Rc4_128, ..protection(Some(&a31), Some(&padded)) }, same),
        ("AES-128, same once padded", Protection { algorithm: Aes128, ..protection(Some(&a31), Some(&padded)) }, same),
    ];
    let got: Vec<_> = cases.iter().map(|(case, p, _)| (*case, protect_and_reopen(p))).collect();
    let want: Vec<_> = cases.iter().map(|(case, _, message)| (*case, (Err(EditError::Protection(message.to_string())), None))).collect();
    assert_eq!(got, want, "each is refused, so no file is written");
    // Passwords that are empty or equal as typed keep their messages.
    let (mut s, id) = session_with(1);
    assert_eq!(s.apply(id, Edit::Protect(protection(Some(""), None))), Err(EditError::Protection("passwords can't be empty".into())));
    assert_eq!(
        s.apply(id, Edit::Protect(protection(Some("same"), Some("same")))),
        Err(EditError::Protection("the open password and the permissions password must be different".into()))
    );
    // Passwords that stay different are fine: only the permissions password gives owner rights.
    let distinct = [
        protection(Some("A"), Some("\u{FF21}\u{FF22}")),
        Protection { algorithm: Rc4_128, ..protection(Some(&format!("1{a32}")), Some(&format!("2{a32}"))) },
        Protection { algorithm: Aes128, ..protection(Some(&format!("1{a32}")), Some(&format!("2{a32}"))) },
    ];
    for p in &distinct {
        assert_eq!(protect_and_reopen(p), (Ok(()), Some([None, Some(User), Some(Owner)])), "{:?}", p.algorithm);
    }
}

/// A one-page form: a text field and a check box (with appearance states).
fn form_fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 250 180 270] /P 3 0 R /MK << /BC [0 0 0] >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (ok) /V /Off /AS /Off /Rect [20 200 35 215] /P 3 0 R /AP << /N << /Yes 7 0 R /Off 7 0 R >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Whether `needle` appears in `bytes`, or in any stream of the PDF they hold once decoded (so
/// compressed object streams are searched too), as literal or hex string bytes.
fn appears_in_pdf(bytes: &Arc<Vec<u8>>, needle: &str) -> bool {
    let hex: String = needle.bytes().map(|b| format!("{b:02x}")).collect();
    let forms = [needle.as_bytes().to_vec(), hex.clone().into_bytes(), hex.to_uppercase().into_bytes()];
    let found = |hay: &[u8]| forms.iter().any(|f| hay.windows(f.len()).any(|w| w == f.as_slice()));
    if found(bytes) {
        return true;
    }
    let doc = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    doc.object_numbers().into_iter().any(|num| match &*doc.get(pdfcraft_cos::ObjRef { num, generation: 0 }) {
        pdfcraft_cos::Object::Stream(st) => st.decoded().is_ok_and(|d| found(&d)),
        _ => false,
    })
}

/// #3926: a value typed into a text field with the Password flag stays in the open document but
/// is written to no file: Save, a full rewrite, Reduce File Size, a recovery snapshot, or a new
/// file made from its pages (Extract, Split, Combine). ISO
/// 32000-2 §12.7.5.3, Table 231, note to the Password flag: PDF processors should never store
/// the value in the PDF file.
#[test]
fn password_field_values_are_kept_in_memory_but_written_to_no_file() {
    let fixture = String::from_utf8(form_fixture()).unwrap().replace("/T (name)", "/T (name) /Ff 8192");
    // Offsets shift by the inserted text; the parser rebuilds the cross-reference table.
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, Arc::new(fixture.into_bytes()), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("hunter2".into()) }).unwrap();
    let field = s.get(id).unwrap().form.iter().find(|f| f.name == "name").cloned().unwrap();
    assert!(field.has(pdfcraft_forms::flags::PASSWORD));
    assert_eq!(field.value, vec!["hunter2".to_string()], "the open document keeps the value");
    assert!(appears_in_pdf(&s.get(id).unwrap().bytes, "hunter2"), "the working bytes do hold it (the test can see it)");

    let saved = s.save_bytes(id).unwrap();
    assert!(!appears_in_pdf(&saved, "hunter2"), "Save");
    // As Acrobat saves it (#3943 review): no default value, so an empty field whose appearance
    // no longer shows how long the typed password was.
    assert!(!appears_in_pdf(&saved, "*******"), "the typed length is not kept in the appearance");
    assert!(!appears_in_pdf(&s.save_full_bytes(id).unwrap(), "hunter2"), "a full rewrite");
    assert!(!appears_in_pdf(&s.reduced_bytes(id).unwrap().0, "hunter2"), "Reduce File Size");
    let snaps = s.autosave_snapshots();
    assert_eq!(snaps.len(), 1);
    assert!(!appears_in_pdf(&snaps[0].bytes, "hunter2"), "the recovery snapshot");
    // New files built from its pages too (#3943 review): Extract copies the fields of the pages it
    // takes, and Split writes each part the same way.
    let extracted = s.extract(id, &[0]).unwrap();
    assert!(!appears_in_pdf(&extracted, "hunter2"), "Extract Pages");
    assert!(!appears_in_pdf(&extracted, "*******"), "nor on the extracted page");
    for (_, _, part) in s.split_by_size(id, 1).unwrap() {
        assert!(!appears_in_pdf(&part, "hunter2"), "Split");
    }
    assert!(!appears_in_pdf(&s.combine(&[("form.pdf".into(), s.get(id).unwrap().bytes.clone())]).unwrap(), "hunter2"), "Combine");
    // Reopened, the field is empty, as in any reader.
    let mut t = Session::new();
    let reopened = t.open("saved.pdf", None, saved, None).unwrap();
    assert!(t.get(reopened).unwrap().form.iter().find(|f| f.name == "name").unwrap().value.iter().all(|v| v.is_empty()));
}

/// #3943 review: recording a save rebases editing on the saved file, which carries no Password
/// value; the typed value stays in the open document all the same, and a later save still omits it.
#[test]
fn a_typed_password_value_survives_save_and_mark_saved() {
    let fixture = String::from_utf8(form_fixture()).unwrap().replace("/T (name)", "/T (name) /Ff 8192");
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, Arc::new(fixture.into_bytes()), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("hunter2".into()) }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, Some("saved.pdf".into())).unwrap();
    let value = |s: &Session| s.get(id).unwrap().form.iter().find(|f| f.name == "name").unwrap().value.clone();
    assert_eq!(value(&s), ["hunter2"], "the open document keeps the value after Save");
    assert!(!s.get(id).unwrap().dirty, "restoring it is not an unsaved change");
    assert!(!appears_in_pdf(&s.save_bytes(id).unwrap(), "hunter2"), "the next save still omits it");
}

/// #3926 / #3943 review: signing writes the signed file from the same copy a save does, so a
/// typed Password-field value is not signed into the file either.
#[test]
fn a_signed_file_carries_no_password_field_value() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    let fixture = String::from_utf8(form_fixture()).unwrap().replace("/T (name)", "/T (name) /Ff 8192");
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("form.pdf", None, Arc::new(fixture.into_bytes()), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("hunter2".into()) }).unwrap();
    assert!(appears_in_pdf(&s.get(id).unwrap().bytes, "hunter2"), "the working bytes hold it");
    let signed = s.sign(id, &digital_id, SignOptions { rect: None, ..SignOptions::default() }).unwrap();
    assert!(!appears_in_pdf(&signed, "hunter2"), "the signed file");
    assert!(!appears_in_pdf(&signed, "*******"), "nor the typed length");
}

/// A value its field's font can't draw is kept, and the edit that drew it reports it in
/// `edit_notices` (the interface shows it, tools return it): not a later unrelated edit, not a
/// failed one, and not a value that can be drawn. The form (an inline AcroForm here) asks other
/// viewers to redraw it.
#[test]
fn a_value_its_font_cannot_draw_is_reported_by_the_edit_that_drew_it() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, Arc::new(form_fixture()), None).unwrap();
    let fill = |s: &mut Session, name: &str, v: FieldValue| s.apply(id, Edit::SetFieldValue { name: name.into(), value: v });
    fill(&mut s, "name", FieldValue::Text("张三".into())).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.form[0].value, ["张三"]);
    let n = &d.edit_notices;
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!((n[0].field.as_deref(), n[0].page, n[0].ch, n[0].redrawn_elsewhere), (Some("name"), 0, '张', true));
    assert!(n[0].to_string().contains("\"张\" (U+5F20)") && n[0].to_string().contains("other PDF viewers redraw"), "{}", n[0]);
    fill(&mut s, "ok", FieldValue::Check(true)).unwrap();
    assert!(s.get(id).unwrap().edit_notices.is_empty(), "an unrelated edit reports nothing");
    fill(&mut s, "name", FieldValue::Text("李四".into())).unwrap();
    assert_eq!(s.get(id).unwrap().edit_notices.len(), 1, "a new value is reported again");
    assert!(fill(&mut s, "missing", FieldValue::Text("x".into())).is_err());
    assert!(s.get(id).unwrap().edit_notices.is_empty(), "no stale notice after a failed edit");
    fill(&mut s, "name", FieldValue::Text("Ada".into())).unwrap();
    assert!(s.get(id).unwrap().edit_notices.is_empty());
    let saved = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    assert!(pdfcraft_forms::need_appearances(&saved));
}

#[test]
fn filling_a_form_shows_saves_and_undoes() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, Arc::new(form_fixture()), None).unwrap();
    assert_eq!(s.get(id).unwrap().form.len(), 2);
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Ada Lovelace".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "ok".into(), value: FieldValue::Check(true) }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.can_undo(), Some("Fill in ok"));
    assert_eq!(d.form[0].value, ["Ada Lovelace"]);
    assert_eq!(d.form[1].value, ["Yes"]);
    assert_eq!(page_texts(&s, id), ["Ada Lovelace"], "the new appearance shows the text");
    let err = s.apply(id, Edit::SetFieldValue { name: "missing".into(), value: FieldValue::Text("x".into()) }).unwrap_err();
    assert_eq!(err.to_string(), "there is no field named \"missing\"");
    s.apply(id, Edit::ResetForm { names: None }).unwrap();
    assert!(s.get(id).unwrap().form.iter().all(|f| f.value.is_empty()));
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().form[0].value, ["Ada Lovelace"]);
    let saved = s.save_bytes(id).unwrap();
    let mut s2 = Session::new();
    let id2 = s2.open("f.pdf", None, saved, None).unwrap();
    assert_eq!(s2.get(id2).unwrap().form[1].value, ["Yes"]);
}

#[test]
fn comment_edits_refresh_the_list_exactly_as_a_full_inspection_would() {
    let (mut s, id) = session_with(3);
    let add = |page: usize, shape: Shape, contents: &str| {
        Edit::AddAnnotation(NewAnnotation {
            page,
            shape,
            style: Style::default_for(&Shape::Rectangle { rect: [0.0; 4] }),
            contents: contents.into(),
            author: "Ada".into(),
        })
    };
    s.apply(id, add(0, Shape::Rectangle { rect: [10.0, 10.0, 60.0, 60.0] }, "box")).unwrap();
    s.apply(id, add(1, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[20.0, 170.0, 90.0, 170.0, 20.0, 150.0, 90.0, 150.0]] }, ""))
        .unwrap();
    s.apply(id, add(1, Shape::TextBox { rect: [10.0, 200.0, 150.0, 240.0], font_size: 11.0 }, "Text box")).unwrap();
    s.apply(id, add(2, Shape::Note { at: [100.0, 280.0], icon: NoteIcon::Comment }, "Sticky")).unwrap();
    s.apply(id, Edit::ReplyToAnnotation { page: 2, index: 0, text: "Reply".into(), author: "Bob".into() }).unwrap();
    s.apply(id, Edit::SetAnnotationStatus { page: 2, index: 0, state: ReviewState::Accepted, author: "Bob".into() }).unwrap();
    s.apply(id, Edit::MarkAnnotation { page: 2, index: 0, marked: true, author: "Bob".into() }).unwrap();
    s.apply(id, Edit::LockAnnotation { page: 1, index: 1, locked: true }).unwrap();
    s.apply(id, Edit::MoveAnnotation { page: 0, index: 0, dx: 5.0, dy: 5.0 }).unwrap();
    s.apply(id, Edit::DeleteAnnotation { page: 1, index: 0 }).unwrap();
    s.undo(id).unwrap();
    let d = s.get(id).unwrap();
    let full = pdfcraft_render::inspect(d.bytes.clone(), None).unwrap();
    assert_eq!(format!("{:?}", d.info.annotations), format!("{:?}", full.annotations));
    assert_eq!(d.info.file_size, full.file_size);
    assert_eq!(d.info.annotations.len(), 7);
    assert!(d.info.annotations.iter().any(|a| a.locked));
}

#[test]
fn form_edits_refresh_field_values_without_a_full_inspection() {
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(form_fixture()), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Ada".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "ok".into(), value: FieldValue::Check(true) }).unwrap();
    let d = s.get(id).unwrap();
    let full = pdfcraft_render::inspect(d.bytes.clone(), None).unwrap();
    let values = |fs: &[pdfcraft_render::Field]| fs.iter().map(|f| (f.name.clone(), f.value.clone())).collect::<Vec<_>>();
    assert_eq!(values(&d.info.fields), values(&full.fields));
    assert_eq!(values(&d.info.fields), [("name".to_string(), Some("Ada".to_string())), ("ok".to_string(), Some("Yes".to_string()))]);
}

#[test]
fn form_refresh_keeps_first_value_when_field_names_repeat() {
    // Two widgets share one name; the refresh reads the first.
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 7 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V (first) /Rect [20 250 180 270] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V (second) /Rect [20 220 180 240] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (other) /Rect [20 190 180 210] /P 3 0 R >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new();
    let id = s.open("dup.pdf", None, Arc::new(bytes), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "other".into(), value: FieldValue::Text("x".into()) }).unwrap();
    let dup = s.get(id).unwrap().info.fields.iter().find(|f| f.name == "dup").unwrap().value.clone();
    assert_eq!(dup.as_deref(), Some("first"));
}

#[test]
fn multiselect_choice_indices_stay_sorted_through_engine_save_and_reopen() {
    let (mut s, id) = session_with(1);
    s.apply(
        id,
        Edit::AddField {
            page: 0,
            rect: [20.0, 180.0, 180.0, 240.0],
            kind: NewField::List { options: vec!["Cheese".into(), "Ham".into(), "Olives".into()], multi: true },
            name: Some("toppings".into()),
        },
    )
    .unwrap();
    let chosen = vec!["Olives".to_string(), "Ham".to_string()];
    s.apply(id, Edit::SetFieldValue { name: "toppings".into(), value: FieldValue::Choice(chosen.clone()) }).unwrap();
    assert_eq!(s.get(id).unwrap().form[0].value, chosen, "the live engine keeps selection order");
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    let all = pdfcraft_forms::fields(&cos);
    let list = all.iter().find(|field| field.name == "toppings").unwrap();
    let object = cos.get(list.obj);
    let indices = object.as_dict().unwrap().get(b"I").unwrap().as_array().unwrap();
    assert_eq!(indices.iter().map(|index| index.as_int().unwrap()).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(list.value, chosen, "sorted /I does not reorder /V");
    let reopened = s.open("reopened.pdf", None, saved, None).unwrap();
    assert_eq!(s.get(reopened).unwrap().form[0].value, chosen, "reopening keeps selection order");
}

#[test]
fn preparing_a_form_adds_renames_and_deletes_fields() {
    let (mut s, id) = session_with(1);
    assert!(s.get(id).unwrap().form.is_empty());
    let add = |kind: NewField, y: f64| Edit::AddField { page: 0, rect: [20.0, y, 180.0, y + 22.0], kind, name: None };
    s.apply(id, add(NewField::Text { multiline: false }, 250.0)).unwrap();
    s.apply(id, add(NewField::CheckBox, 200.0)).unwrap();
    s.apply(id, add(NewField::Text { multiline: false }, 150.0)).unwrap();
    let names = |s: &Session| s.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s), ["Text1", "Check Box1", "Text2"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add field"));
    let props = FieldProps { name: Some("email".into()), tooltip: Some("Your e-mail".into()), required: Some(true), ..Default::default() };
    s.apply(id, Edit::SetFieldProps { name: "Text2".into(), props: Box::new(props) }).unwrap();
    s.apply(id, Edit::DeleteField { name: "Check Box1".into() }).unwrap();
    assert_eq!(names(&s), ["Text1", "email"]);
    s.apply(id, Edit::SetFieldValue { name: "email".into(), value: FieldValue::Text("ada@example.org".into()) }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let mut s2 = Session::new();
    let id2 = s2.open("f.pdf", None, saved, None).unwrap();
    let d = s2.get(id2).unwrap();
    assert_eq!(d.form.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Text1", "email"]);
    assert_eq!(d.form[1].value, ["ada@example.org"]);
    assert!(page_texts(&s2, id2).iter().any(|t| t.contains("ada@example.org")));
    s.undo(id).unwrap();
    s.undo(id).unwrap();
    assert_eq!(names(&s), ["Text1", "Check Box1", "email"]);
}

/// Setting the calculation order is one undoable edit that recalculates: values that ran too
/// early come out right at once, a script calculation included, and undo brings back the old
/// order and values together.
#[test]
fn setting_the_calculation_order_recalculates_and_undoes() {
    use form_scripts::{CalcOp, Calculate};
    let (mut s, id) = session_with(1);
    for (i, name) in ["qty", "sub", "total", "tax"].into_iter().enumerate() {
        let y = 250.0 - 40.0 * i as f64;
        let kind = NewField::Text { multiline: false };
        s.apply(id, Edit::AddField { page: 0, rect: [20.0, y, 180.0, y + 22.0], kind, name: Some(name.into()) }).unwrap();
    }
    let calculate = |name: &str, c: Calculate| Edit::SetFieldProps {
        name: name.into(),
        props: Box::new(FieldProps { calculate: Some(c), ..Default::default() }),
    };
    // Defined backwards: total, which reads sub, calculates first.
    s.apply(id, calculate("total", Calculate::Simple { op: CalcOp::Sum, fields: vec!["sub".into()] })).unwrap();
    s.apply(id, calculate("sub", Calculate::Notation("qty * 2".into()))).unwrap();
    let script = Some("event.value = this.getField(\"total\").value / 10;".to_string());
    s.apply(id, Edit::SetFieldScript { name: "tax".into(), event: "calculate".into(), script }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("5".into()) }).unwrap();
    let values = |s: &Session| {
        let form = &s.get(id).unwrap().form;
        ["sub", "total", "tax"].map(|n| form.iter().find(|f| f.name == n).and_then(|f| f.value.first().cloned()).unwrap_or_default())
    };
    assert_eq!(values(&s), ["10", "0", "0"], "total and tax ran before sub");
    let order = s.get(id).unwrap().calculation_order();
    assert_eq!(order.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["total", "sub", "tax"]);
    assert_eq!(order.fields[0].issues, [CalculationIssue::RunsTooEarly { field: "sub".into() }]);
    assert_eq!(order.fixed(), ["sub", "total", "tax"]);
    s.apply(id, Edit::SetCalculationOrder { names: order.fixed() }).unwrap();
    assert_eq!(values(&s), ["10", "10", "1"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Set calculation order"));
    assert!(s.get(id).unwrap().calculation_order().fields.iter().all(|f| f.issues.is_empty()));
    s.undo(id).unwrap();
    assert_eq!(values(&s), ["10", "0", "0"]);
    assert_eq!(s.get(id).unwrap().calculation_order().fixed(), ["sub", "total", "tax"], "the old order is back");
    assert!(matches!(s.apply(id, Edit::SetCalculationOrder { names: vec!["qty".into()] }), Err(EditError::Form(_))));
}

/// One 612×300 page with the content stream `body`. Object 4 is Helvetica; `resources` is what
/// the page's /Resources holds.
fn one_line_page(resources: &str, body: &str) -> Vec<u8> {
    one_line_page_with(resources, "", body)
}

/// [`one_line_page`] with `stream_entries` added to the content stream's dictionary.
fn one_line_page_with(resources: &str, stream_entries: &str, body: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 300] >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /Resources << {resources} >> /Contents 5 0 R >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        format!("<< /Length {} {stream_entries} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Put a redaction mark over `needle` where page 1 shows it, as Find text and redact does.
fn mark_shown_text(s: &mut Session, id: DocId, needle: &str) {
    let doc = s.get(id).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), pdfcraft_render::RenderConfig::default());
    let text = r.render(RenderRequestFor::text(0)).text.expect("the page has text");
    let range = text.find(needle).pop().expect("the page shows the text");
    let info = &doc.info.pages[0];
    let quads = text.line_rects(range).into_iter().map(|v| pdfcraft_annot::rect_quad(info.view_rect_to_user(v).map(f64::from))).collect();
    let shape = Shape::Redact { quads, overlay: String::new(), look: Default::default() };
    let mark =
        Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Ada".into() });
    s.apply(id, mark).unwrap();
}

/// Redaction removes text by the positions its own interpreter works out, and the renderer has
/// the last word: where the two disagree, a page that still shows a glyph mostly under a mark
/// fails the edit. The check is run here on a page nothing was removed from, as if the
/// interpreter had placed the word elsewhere. Only the overlay streams an apply added are left
/// out: a content stream of the input labelled like one (`/PCMark /Redaction`) is checked too.
#[test]
fn the_renderer_check_refuses_text_still_shown_under_a_mark() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let page = one_line_page_with("/Font << /F1 4 0 R >>", "/PCMark /Redaction", "BT /F1 16 Tf 20 150 Td (Public words then SECRET42) Tj ET");
    let id = s.open("shown.pdf", None, Arc::new(page), None).unwrap();
    mark_shown_text(&mut s, id, "SECRET42");
    let doc = s.get(id).unwrap();
    let mut cos = doc.editor.as_ref().unwrap().cos.clone();
    let marks = pdfcraft_redact::marks(&cos);
    let refused = |cos: &pdfcraft_cos::Document| {
        let shown = shown_under_redactions(cos, &marks, &[], &doc.info.pages, None);
        matches!(shown, Err(EditError::Redact(pdfcraft_redact::RedactError::Residue(n))) if n > 0)
    };
    assert!(refused(&cos));
    // The page's contents as an array, as they are once an apply has added its overlay.
    let page = pdfcraft_model::pages(&cos).swap_remove(0);
    let contents = page.dict.get(b"Contents").cloned().unwrap();
    cos.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), pdfcraft_cos::Object::Array(vec![contents]))).unwrap();
    assert!(refused(&cos), "the labelled input stream was not checked");
}

/// Inline image data `7FEI 1 0 0 1 200 0 cm` holds an `EI` the renderer ends the image at, so it
/// runs the `cm` and shows SECRET 200 pt to the right. Redaction reads the image the same way, so
/// a mark drawn over the word as shown takes it out of the file.
#[test]
fn text_after_an_inline_image_is_redacted_where_the_page_shows_it() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let page = one_line_page(
        "/Font << /F1 4 0 R >>",
        "BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7FEI 1 0 0 1 200 0 cm EI BT /F1 10 Tf 10 100 Td (SECRET) Tj ET",
    );
    let id = s.open("inline.pdf", None, Arc::new(page), None).unwrap();
    mark_shown_text(&mut s, id, "SECRET");
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    assert!(!page_texts(&s, id).concat().contains("SECRET"));
    let saved = s.save_bytes(id).unwrap();
    let held = |d: &[u8]| d.windows(6).any(|w| w == b"SECRET");
    assert!(!held(&saved), "the saved file holds the redacted word");
    let reopened = pdfcraft_cos::Document::open(saved).unwrap();
    assert!(
        !reopened.scan_objects().any(|(_, o)| matches!(&*o, pdfcraft_cos::Object::Stream(st) if st.decoded().is_ok_and(|d| held(&d)))),
        "a saved stream holds the redacted word"
    );
}

/// A word whose font and size an ExtGState /Font entry (`gs`) sets is placed where the renderer
/// draws it, so a mark drawn over it as displayed takes it out of the file.
#[test]
fn a_word_in_a_font_set_by_an_extgstate_is_redacted() {
    let line = "Public words then SECRET42 and more";
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let by_gs = one_line_page("/ExtGState << /GS1 << /Type /ExtGState /Font [4 0 R 16] >> >>", &format!("BT /GS1 gs 20 150 Td ({line}) Tj ET"));
    let id = s.open("gs.pdf", None, Arc::new(by_gs), None).unwrap();
    mark_shown_text(&mut s, id, "SECRET42");
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let shown = page_texts(&s, id).swap_remove(0);
    assert!(shown.starts_with("Public words then") && shown.ends_with("and more") && !shown.contains("SECRET42"), "{shown}");
    let saved = s.save_bytes(id).unwrap();
    let held = |d: &[u8]| d.windows(8).any(|w| w == b"SECRET42");
    assert!(!held(&saved), "the saved file holds the redacted word");
    let reopened = pdfcraft_cos::Document::open(saved).unwrap();
    assert!(
        !reopened.scan_objects().any(|(_, o)| matches!(&*o, pdfcraft_cos::Object::Stream(st) if st.decoded().is_ok_and(|d| held(&d)))),
        "a saved stream holds the redacted word"
    );
}

#[test]
fn redaction_marks_apply_for_good_and_undo() {
    let (mut s, id) = session_with(2);
    // "Page 1" at 24 pt from x 20: the "1" starts near x 82.7 (Helvetica widths).
    let shape =
        Shape::Redact { quads: vec![pdfcraft_annot::rect_quad([80.0, 140.0, 100.0, 180.0])], overlay: String::new(), look: Default::default() };
    let mark =
        Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Ada".into() });
    s.apply(id, mark).unwrap();
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add redaction mark"));
    assert_eq!(s.get(id).unwrap().redaction_marks(), 1);
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2"], "marking alone changes nothing");
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!((d.can_undo(), d.redaction_marks()), (Some("Apply redactions"), 0));
    // An independent extractor (the renderer's) no longer finds the "1".
    assert_eq!(page_texts(&s, id), ["Page", "Page 2"]);
    let saved = s.save_bytes(id).unwrap();
    assert!(!saved.windows(8).any(|w| w == b"(Page 1)"), "a full rewrite: the old revision is gone");
    assert!(saved.windows(8).any(|w| w == b"(Page 2)"));
    let mut s2 = Session::new();
    let id2 = s2.open("r.pdf", None, saved, None).unwrap();
    assert_eq!(page_texts(&s2, id2), ["Page", "Page 2"]);
    s.undo(id).unwrap();
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2"]);
    assert_eq!(s.apply(id, Edit::ClearRedactions).map(|_| s.get(id).unwrap().redaction_marks()), Ok(0));
    assert!(matches!(s.apply(id, Edit::ApplyRedactions { pages: None }), Err(EditError::Redact(_))));
}

#[test]
fn oversized_redaction_overlay_preserves_the_document_and_undo() {
    let (mut s, id) = session_with(1);
    let shape = Shape::Redact {
        quads: vec![pdfcraft_annot::rect_quad([0.0, 0.0, 16000.0, 1600.0])],
        overlay: "X".into(),
        look: pdfcraft_annot::OverlayLook { size: 2.0, repeat: true, ..Default::default() },
    };
    s.apply(
        id,
        Edit::AddAnnotation(NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Tester".into() }),
    )
    .unwrap();
    let before = s.get(id).unwrap().bytes.clone();
    assert!(matches!(s.apply(id, Edit::ApplyRedactions { pages: None }), Err(EditError::Redact(pdfcraft_redact::RedactError::OverlayLimit))));
    let doc = s.get(id).unwrap();
    assert_eq!(doc.bytes, before);
    assert_eq!(doc.redaction_marks(), 1);
    assert_eq!(doc.can_undo(), Some("Add redaction mark"));
    assert_eq!(page_texts(&s, id), ["Page 1"]);
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().redaction_marks(), 0);
    assert_eq!(page_texts(&s, id), ["Page 1"]);
}

/// #3046: a layer hidden in the Layers panel stays off on paper, as it is on screen; with the
/// layers untouched, the document's own configuration prints.
#[test]
fn printing_keeps_hidden_layers_hidden() {
    let content = b"/OC /A BDC 1 0 0 rg 10 10 80 80 re f EMC /OC /B BDC 0 1 0 rg 110 10 80 80 re f EMC";
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << /Order [5 0 R 6 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Properties << /A 5 0 R /B 6 0 R >> >> >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content, b"\nendstream"].concat(),
        b"<< /Type /OCG /Name (Layer A) >>".to_vec(),
        b"<< /Type /OCG /Name (Layer B) >>".to_vec(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(o);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());

    // Red and green pixels on the first printed sheet.
    let printed = |s: &Session, id: DocId| {
        let bytes = s.print_pdf(id, &print::Settings { pages: vec![0], ..Default::default() }).unwrap();
        let mut out = Session::new();
        let printed = out.open("print.pdf", None, Arc::new(bytes), None).unwrap();
        let page = render_page(&out, printed);
        let px = page.rgba.as_chunks::<4>().0;
        let red = px.iter().filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60).count();
        let green = px.iter().filter(|p| p[1] > 200 && p[0] < 60 && p[2] < 60).count();
        (red, green)
    };
    let mut s = Session::new();
    let id = s.open("layers.pdf", None, Arc::new(pdf), None).unwrap();
    let (red, green) = printed(&s, id);
    assert!(red > 100 && green > 100, "untouched layers both print: {red} red, {green} green");
    let a = s.get(id).unwrap().info.layers.iter().position(|l| l.name == "Layer A").unwrap();
    assert!(s.set_layer_visible(id, a, false));
    let (red, green) = printed(&s, id);
    assert_eq!(red, 0, "Layer A was hidden, so it doesn't print");
    assert!(green > 100, "Layer B still prints: {green} green");
    assert!(s.set_layer_visible(id, a, true));
    let (red, _) = printed(&s, id);
    assert!(red > 100, "shown again, Layer A prints again: {red} red");
}

#[test]
fn printing_lays_out_sheets_that_render() {
    let (s, id) = session_with(5);
    let settings = print::Settings {
        pages: print::select_pages(5, Some("2-5"), &[], print::Subset::All, false).unwrap(),
        layout: print::Layout::multiple(2),
        ..Default::default()
    };
    let bytes = s.print_pdf(id, &settings).unwrap();
    let mut s2 = Session::new();
    let id2 = s2.open("print.pdf", None, Arc::new(bytes), None).unwrap();
    assert_eq!(s2.get(id2).unwrap().info.pages.len(), 2, "four pages, two per sheet");
    let texts = page_texts(&s2, id2);
    assert!(texts[0].contains("Page 2") && texts[0].contains("Page 3") && texts[1].contains("Page 5"), "{texts:?}");
    // Printing honours the permissions.
    let p =
        Protection { open_password: Some("pw".into()), permissions_password: Some("owner".into()), printing: Printing::None, ..Default::default() };
    let (mut s3, id3) = session_with(1);
    s3.apply(id3, Edit::Protect(p)).unwrap();
    let saved = s3.save_bytes(id3).unwrap();
    let mut s4 = Session::new();
    let id4 = s4.open("locked.pdf", None, saved, Some("pw")).unwrap();
    assert_eq!(s4.print_pdf(id4, &print::Settings { pages: vec![0], ..Default::default() }), Err(EditError::NotPermitted("printing")));
}

/// A one-page document whose red square is in a layer; `off` switches the layer off by default.
fn red_layer_pdf(off: bool) -> Vec<u8> {
    let content = "/OC /L1 BDC 1 0 0 rg 0 0 100 100 re f EMC";
    let off = if off { "/OFF [5 0 R] " } else { "" };
    format!(
        "%PDF-1.7\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << {off}/Order [5 0 R] >> >> >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Properties << /L1 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /OCG /Name (Red) >> endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        content.len()
    )
    .into_bytes()
}

/// Does the first page of `pdf` show any red when drawn?
fn first_page_has_red(pdf: Vec<u8>) -> bool {
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(pdf), pdfcraft_render::RenderConfig::default());
    let page = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 0.5, ..Default::default() });
    page.rgba.as_chunks::<4>().0.iter().any(|p| p[0] > 200 && p[1] < 60 && p[2] < 60)
}

#[test]
fn a_layer_that_is_off_does_not_print_and_one_that_is_on_does() {
    for (off, prints) in [(true, false), (false, true)] {
        let source = red_layer_pdf(off);
        assert_eq!(first_page_has_red(source.clone()), prints, "on screen (off: {off})");
        let mut s = Session::new();
        let id = s.open("layers.pdf", None, Arc::new(source), None).unwrap();
        let sheet = s.print_pdf(id, &print::Settings { pages: vec![0], ..Default::default() }).unwrap();
        assert_eq!(first_page_has_red(sheet), prints, "printed (layer off by default: {off})");
    }
}

#[test]
fn crop_and_duplicate_pages_show_in_the_viewer() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::SetPageBox { pages: vec![0], which: PageBox::Crop, spec: BoxSpec::Margins([10.0, 20.0, 30.0, 40.0]) }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.can_undo(), Some("Crop page"));
    assert_eq!(d.info.pages[0].crop, [10.0, 20.0, 170.0, 260.0]);
    assert_eq!((d.info.pages[0].width, d.info.pages[0].height), (160.0, 240.0), "the viewer shows the cropped size");
    assert_eq!(d.page_boxes()[1][1], [0.0, 0.0, 200.0, 300.0]);
    s.apply(id, Edit::DuplicatePages { pages: vec![1], at: None }).unwrap();
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2", "Page 2"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Duplicate page"));
}

#[test]
fn headers_footers_watermarks_and_backgrounds_show_update_and_remove() {
    let (mut s, id) = session_with(2);
    let mut hf = HeaderFooter::default();
    hf.text[4] = "Page <<1>> of <<n>> · <<yyyy-mm-dd>>".into();
    s.apply(id, Edit::AddHeaderFooter { pages: vec![0, 1], settings: hf.clone(), replace: false }).unwrap();
    // The session clock is 2023-11-14.
    assert_eq!(page_texts(&s, id)[1], "Page 2\nPage 2 of 2 · 2023-11-14");
    assert_eq!(s.get(id).unwrap().marks(), [MarkKind::HeaderFooter]);
    hf.text[4] = "Draft".into();
    s.apply(id, Edit::AddHeaderFooter { pages: vec![0, 1], settings: hf, replace: true }).unwrap();
    assert_eq!(page_texts(&s, id)[0], "Page 1\nDraft");
    s.apply(
        id,
        Edit::AddWatermark { pages: vec![0], settings: Watermark { text: "SECRET".into(), ..Watermark::default() }, file: None, replace: false },
    )
    .unwrap();
    s.apply(
        id,
        Edit::AddBackground {
            pages: vec![1],
            settings: Background { color: [1.0, 0.9, 0.9], opacity: 1.0, ..Background::default() },
            replace: false,
            file: None,
        },
    )
    .unwrap();
    assert_eq!(s.get(id).unwrap().marks().len(), 3);
    assert!(page_texts(&s, id)[0].contains("SECRET"));
    s.apply(id, Edit::RemoveMarks { kind: MarkKind::HeaderFooter }).unwrap();
    s.apply(id, Edit::RemoveMarks { kind: MarkKind::Watermark }).unwrap();
    s.apply(id, Edit::RemoveMarks { kind: MarkKind::Background }).unwrap();
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2"]);
    assert!(s.get(id).unwrap().marks().is_empty());
    assert!(matches!(s.apply(id, Edit::RemoveMarks { kind: MarkKind::Watermark }), Err(EditError::Edit(_))));
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Remove background"));
}

#[test]
fn marker_indexes_match_after_open_and_save_when_requested() {
    let (mut source, source_id) = session_with(1);
    let mut header = HeaderFooter::default();
    header.text[0] = "Header".into();
    source.apply(source_id, Edit::AddHeaderFooter { pages: vec![0], settings: header, replace: false }).unwrap();
    source
        .apply(
            source_id,
            Edit::AddWatermark { pages: vec![0], settings: Watermark { text: "Draft".into(), ..Watermark::default() }, file: None, replace: false },
        )
        .unwrap();
    source
        .apply(
            source_id,
            Edit::AddBackground {
                pages: vec![0],
                settings: Background { color: [0.9, 0.9, 1.0], opacity: 1.0, ..Background::default() },
                replace: false,
                file: None,
            },
        )
        .unwrap();
    source
        .apply(
            source_id,
            Edit::AddText { page: 0, text: AddedText { rect: [20.0, 250.0, 180.0, 280.0], text: "Added note".into(), ..Default::default() } },
        )
        .unwrap();

    let bytes = source.save_bytes(source_id).unwrap();
    let reference = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    let expected_marks = pdfcraft_edit::marks_present(&reference);
    let expected_added = pdfcraft_edit::list_added(&reference);
    source.mark_saved(source_id, bytes.clone(), None).unwrap();
    let refreshed = source.get(source_id).unwrap();
    assert_eq!(refreshed.marks(), expected_marks);
    assert_eq!(refreshed.added(), expected_added);
    let mut reopened = Session::new();
    let reopened_id = reopened.open("marked.pdf", None, bytes, None).unwrap();
    let opened = reopened.get(reopened_id).unwrap();
    assert_eq!(opened.marks(), expected_marks);
    assert_eq!(opened.added(), expected_added);
    assert_eq!(opened.marks().len(), 3);
    assert!(opened.marks().contains(&MarkKind::HeaderFooter));
    assert!(opened.marks().contains(&MarkKind::Watermark));
    assert!(opened.marks().contains(&MarkKind::Background));
    assert_eq!(opened.added().len(), 1);
}

#[test]
fn secondary_indexes_load_once_and_follow_edit_scopes() {
    reset_secondary_scan_counts();
    let (mut session, id) = session_with(1);
    assert_eq!(secondary_scan_counts(), (0, 0), "open does not scan markers or links");

    {
        let doc = session.get(id).unwrap();
        assert!(doc.marks().is_empty());
        assert!(doc.added().is_empty());
        assert!(doc.added().is_empty());
        assert_eq!(secondary_scan_counts(), (1, 0), "marks and added content share one scan");
        assert!(doc.links().is_empty());
        assert!(doc.links().is_empty());
        assert_eq!(secondary_scan_counts(), (1, 1), "repeated reads reuse the link list");
    }

    session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Changed".into() }).unwrap();
    assert!(session.get(id).unwrap().links().is_empty());
    assert!(session.get(id).unwrap().marks().is_empty());
    assert_eq!(secondary_scan_counts(), (1, 1), "metadata does not invalidate either index");

    session.apply(id, rect_comment(0, [20.0, 20.0, 60.0, 60.0])).unwrap();
    assert!(session.get(id).unwrap().links().is_empty());
    assert_eq!(secondary_scan_counts(), (1, 2), "comment edits invalidate the annotation-position link index");

    session
        .apply(
            id,
            Edit::AddLink {
                page: 0,
                rect: [10.0, 10.0, 80.0, 30.0],
                action: LinkAction::Uri("https://example.org".into()),
                style: LinkStyle::default(),
            },
        )
        .unwrap();
    assert_eq!(secondary_scan_counts(), (1, 2), "a link edit invalidates only the link list");
    assert_eq!(session.get(id).unwrap().links()[0].index, 1, "link indexes include earlier annotations");
    assert_eq!(secondary_scan_counts(), (1, 3));
    session.undo(id).unwrap();
    assert_eq!(session.get(id).unwrap().links().len(), 0);
    assert_eq!(secondary_scan_counts(), (1, 4), "undo rebuilds the link list once");
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().links()[0].index, 1);
    assert_eq!(secondary_scan_counts(), (1, 5), "redo rebuilds the link list once");
    session.apply(id, Edit::DeleteAnnotation { page: 0, index: 0 }).unwrap();
    assert_eq!(session.get(id).unwrap().links()[0].index, 0, "deleting an earlier annotation shifts link indexes");
    assert_eq!(secondary_scan_counts(), (1, 6), "comment deletion rebuilds annotation-position indexes");

    session
        .apply(id, Edit::AddText { page: 0, text: AddedText { rect: [20.0, 250.0, 100.0, 280.0], text: "Added".into(), ..Default::default() } })
        .unwrap();
    assert_eq!(secondary_scan_counts(), (1, 6), "content edits defer rebuilding until a marker getter is used");
    assert_eq!(session.get(id).unwrap().added().len(), 1);
    assert_eq!(session.get(id).unwrap().links().len(), 1);
    assert_eq!(secondary_scan_counts(), (2, 6), "content edits preserve the link cache");

    let mut header = HeaderFooter::default();
    header.text[0] = "Header".into();
    session.apply(id, Edit::AddHeaderFooter { pages: vec![0], settings: header, replace: false }).unwrap();
    assert_eq!(session.get(id).unwrap().marks(), [MarkKind::HeaderFooter]);
    assert_eq!(secondary_scan_counts(), (3, 6));
    session.undo(id).unwrap();
    assert!(session.get(id).unwrap().marks().is_empty());
    assert_eq!(secondary_scan_counts(), (4, 6), "undo invalidates marker state");
    session.undo(id).unwrap();
    assert!(session.get(id).unwrap().added().is_empty());
    assert_eq!(secondary_scan_counts(), (5, 6), "undo of added content invalidates marker state");
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().added().len(), 1);
    assert_eq!(secondary_scan_counts(), (6, 6), "redo of added content rebuilds once");
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().marks(), [MarkKind::HeaderFooter]);
    assert_eq!(secondary_scan_counts(), (7, 6), "redo of marks rebuilds once");
}

#[test]
fn export_images_and_text_follow_the_working_file() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
    let doc = s.get(id).unwrap();
    let mut ex = export::Exporter::new(doc);
    let png = ex.png(1, 144.0).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    // 200×300 pt rotated → 300×200 pt at 2 px/pt.
    let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
    assert_eq!((w, h), (600, 400));
    assert_eq!(ex.text_of(&[0, 1]).unwrap(), "Page 1\n\u{c}Page 2\n");
    assert!(ex.png(5, 72.0).is_err());
}

#[test]
fn create_and_reduce() {
    let mut s = Session::new();
    let blank = s.create_blank(612.0, 792.0, 2).unwrap();
    let id = s.open_new("Untitled.pdf", blank).unwrap();
    assert_eq!(s.get(id).unwrap().info.pages.len(), 2);
    let text = s.create_from_text("notes", "hello\nworld").unwrap();
    let t = s.open_new("notes.pdf", text).unwrap();
    assert_eq!(page_texts(&s, t), ["hello\nworld"]);
    // A file with the same page imported twice shares nothing until reduced.
    let (mut s2, id2) = session_with(1);
    let src = Arc::new(fixture(1));
    s2.apply(
        id2,
        Edit::InsertPagesFrom {
            name: "x".into(),
            bytes: src.clone(),
            pages: None,
            at: 1,
            field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
        },
    )
    .unwrap();
    let full = s2.save_full_bytes(id2).unwrap();
    let (reduced, _) = s2.reduced_bytes(id2).unwrap();
    assert!(reduced.len() <= full.len(), "{} > {}", reduced.len(), full.len());
    let mut s3 = Session::new();
    let r = s3.open("r.pdf", None, reduced, None).unwrap();
    assert_eq!(page_texts(&s3, r), ["Page 1", "Page 1"]);
}

#[test]
fn an_optimize_job_reports_its_stages_and_can_be_cancelled() {
    use crate::optimizer::OptimizeStage;
    let (s, id) = session_with(2);
    let settings = optimize::Settings::default();
    let mut stages = Vec::new();
    let (bytes, _) = s
        .optimize_job(id, &settings, &[])
        .unwrap()
        .run(|st| {
            stages.push(st);
            true
        })
        .unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    assert_eq!(stages.first(), Some(&OptimizeStage::Discarding));
    assert_eq!(&stages[stages.len() - 3..], [OptimizeStage::CleaningUp, OptimizeStage::Merging, OptimizeStage::Writing]);
    let fractions: Vec<f32> = stages.iter().map(|st| st.fraction()).collect();
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "the bar never goes back: {fractions:?}");
    assert!(fractions.iter().all(|f| (0.0..=1.0).contains(f)));
    assert_eq!(
        OptimizeStage::Images { done: 0, total: 0 }.fraction(),
        OptimizeStage::Images { done: 5, total: 5 }.fraction(),
        "no images: no division by zero"
    );

    // Cancelled at the merge: nothing is written, the open document is untouched.
    let before = s.get(id).unwrap().bytes.clone();
    let r = s.optimize_job(id, &settings, &[]).unwrap().run(|st| st != OptimizeStage::Merging);
    assert!(matches!(r, Err(EditError::Cancelled)), "{r:?}");
    assert_eq!(s.get(id).unwrap().bytes, before);
}

#[test]
fn flattening_keeps_the_look_and_drops_the_objects() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, Arc::new(form_fixture()), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Ada".into()) }).unwrap();
    s.apply(id, rect_comment(0, [80.0, 80.0, 120.0, 120.0])).unwrap();
    let red_before = pixel(&s, id, 0, 100, 100);
    s.apply(id, Edit::Flatten { comments: true, fields: true }).unwrap();
    let d = s.get(id).unwrap();
    assert!(d.info.annotations.is_empty() && d.form.is_empty(), "no comments or fields remain");
    assert_eq!(pixel(&s, id, 0, 100, 100), red_before, "the rectangle is now page content");
    assert_eq!(page_texts(&s, id), ["Ada"], "the field's text is now page content");
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().form.len(), 2);
}

fn fill_item(shape: Shape, contents: &str) -> Edit {
    let style = Style::default_for(&shape);
    Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: contents.into(), author: "Ada".into() })
}

fn comment_text(s: &Session, id: DocId) -> Vec<String> {
    let mut t: Vec<String> = s.get(id).unwrap().info.annotations.iter().map(|a| a.contents.clone().unwrap_or_default()).collect();
    t.sort();
    t
}

/// A dragged stamp's layers: the page without it (its text and other comments kept) and the
/// stamp alone on a transparent page. Preparing them changes nothing in the document.
#[test]
fn stamp_preview_layers_split_the_stamp_from_its_page() {
    let (mut s, id) = session_with(1);
    s.apply(id, fill_item(Shape::Mark { rect: [30.0, 60.0, 130.0, 66.0], mark: FillMark::Line }, "")).unwrap();
    s.apply(id, rect_comment(0, [140.0, 40.0, 190.0, 90.0])).unwrap();
    let doc = s.get(id).unwrap();
    let (generation, dirty) = (doc.edit_generation(), doc.dirty);
    let preview = doc.stamp_preview(0, 0).unwrap().expect("the line is a stamp");
    let (stamp, background) = (preview.render_stamp(1.0).unwrap(), preview.render_background(1.0).unwrap());
    let px = |p: &pdfcraft_render::RenderedPage, x: u32, y: u32| -> [u8; 4] {
        p.rgba[(((300 - y) * p.width + x) * 4) as usize..][..4].try_into().unwrap()
    };
    let line = |x: u32, y: u32| (30..=130).contains(&x) && (59..=67).contains(&y);
    let (mut drawn, mut stray) = (0, Vec::new());
    for y in 1..300 {
        for x in 0..200 {
            match (line(x, y), px(&stamp, x, y)) {
                (true, [.., 255]) => drawn += 1,
                (false, v) if v != [0, 0, 0, 0] => stray.push((x, y)),
                _ => {}
            }
        }
    }
    assert!(drawn > 100, "the line is drawn opaque: {drawn} pixels");
    assert!(stray.is_empty(), "only the line is drawn on its layer: {:?}", &stray[..stray.len().min(8)]);
    assert!((30..=130).all(|x| px(&background, x, 63) == [255, 255, 255, 255]), "the page beneath has no line");
    assert!((138..=142).any(|x| px(&background, x, 65) != [255, 255, 255, 255]), "other comments stay");
    assert!((20..120).any(|x| (150..170).any(|y| px(&background, x, y)[0] < 128)), "the page text stays");
    assert!(doc.stamp_preview(0, 1).unwrap().is_none(), "a rectangle isn't a stamp");
    assert!(doc.stamp_preview(0, 9).unwrap().is_none());
    let doc = s.get(id).unwrap();
    assert_eq!((doc.edit_generation(), doc.dirty, doc.can_undo()), (generation, dirty, Some("Add rectangle")), "the document is unchanged");

    // Another app's comments without appearances: beneath a dragged stamp they are drawn as on
    // screen, and such a stamp has nothing to lay over the page.
    let bare = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Square /Rect [140 40 190 90] /C [1 0 0] >> endobj
5 0 obj << /Type /Annot /Subtype /Stamp /Rect [20 20 80 40] /Name /Draft >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut s = Session::new();
    let id = s.open("bare.pdf", None, Arc::new(bare.to_vec()), None).unwrap();
    s.apply(id, fill_item(Shape::Mark { rect: [30.0, 60.0, 130.0, 66.0], mark: FillMark::Line }, "")).unwrap();
    let doc = s.get(id).unwrap();
    let background = doc.stamp_preview(0, 2).unwrap().expect("the line is a stamp").render_background(1.0).unwrap();
    assert!((138..=142).any(|x| px(&background, x, 65)[..3] == [255, 0, 0]), "the square without an appearance is drawn beneath");
    assert!(doc.stamp_preview(0, 1).unwrap().is_none(), "a stamp without an appearance");
}

#[test]
fn flattening_fill_and_sign_keeps_other_comments_fields_and_undoes() {
    let (mut s, id) = session_with(1);
    s.apply(id, fill_item(Shape::Typewriter { rect: [20.0, 200.0, 120.0, 220.0], font_size: 10.0 }, "Hello")).unwrap();
    s.apply(id, fill_item(Shape::Mark { rect: [140.0, 200.0, 160.0, 220.0], mark: FillMark::Check }, "")).unwrap();
    s.apply(id, fill_item(Shape::Signature { strokes: vec![vec![[10.0, 40.0], [40.0, 70.0], [70.0, 40.0]]] }, "")).unwrap();
    s.apply(id, fill_item(Shape::TypedSignature { rect: [90.0, 40.0, 150.0, 70.0], contours: vec![vec![[0.1, 0.2], [0.5, 0.9], [0.9, 0.2]]] }, ""))
        .unwrap();
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 8, 8);
        enc.set_color(png::ColorType::Rgba);
        let pixel = [40u8, 60, 80, 200];
        enc.write_header().unwrap().write_image_data(&pixel.repeat(64)).unwrap();
    }
    let image = SignatureImage::from_bytes(&png).unwrap();
    let info = s.get(id).unwrap().info.pages[0].clone();
    s.apply(id, image.edit(0, &info, [20.0, 120.0], false, "Ada").unwrap()).unwrap();
    s.apply(id, fill_item(Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[20.0, 280.0, 80.0, 280.0, 20.0, 270.0, 80.0, 270.0]] }, "keep"))
        .unwrap();
    s.apply(id, fill_item(Shape::TextBox { rect: [20.0, 150.0, 120.0, 180.0], font_size: 12.0 }, "Note")).unwrap();
    s.apply(id, Edit::AddField { page: 0, rect: [20.0, 230.0, 120.0, 250.0], kind: NewField::Text { multiline: false }, name: Some("email".into()) })
        .unwrap();

    s.apply(id, Edit::FlattenFillSign).unwrap();
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Flatten Fill & Sign"));
    assert_eq!(comment_text(&s, id), ["Note", "keep"]);
    assert_eq!(s.get(id).unwrap().form.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["email"]);
    s.undo(id).unwrap();
    assert!(comment_text(&s, id).contains(&"Hello".to_string()), "undo restores the typewriter");

    s.apply(id, Edit::FlattenFillSign).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved.clone(), None).unwrap();
    let mut again = Session::new();
    let id2 = again.open("saved.pdf", None, saved, None).unwrap();
    assert_eq!(comment_text(&again, id2), ["Note", "keep"]);
    assert_eq!(again.get(id2).unwrap().form.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["email"]);
}

#[test]
fn flattening_fill_and_sign_does_nothing_when_there_are_none() {
    let (mut s, id) = session_with(1);
    s.apply(id, fill_item(Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[20.0, 200.0, 80.0, 200.0, 20.0, 190.0, 80.0, 190.0]] }, "keep"))
        .unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    assert!(!s.get(id).unwrap().dirty);
    s.apply(id, Edit::FlattenFillSign).unwrap();
    assert!(!s.get(id).unwrap().dirty, "nothing was baked in, so the document stays saved");
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add highlight"), "no undo step for a flatten that changed nothing");
    assert_eq!(comment_text(&s, id), ["keep"]);
}

#[test]
fn flattening_fill_and_sign_is_refused_without_permission_to_change() {
    let (mut s, id) = session_with(1);
    s.apply(id, fill_item(Shape::Typewriter { rect: [20.0, 200.0, 120.0, 220.0], font_size: 10.0 }, "Hello")).unwrap();
    let p = Protection { printing: Printing::Low, changes: Changes::CommentFillSign, copy: false, ..protection(None, Some("boss")) };
    s.apply(id, Edit::Protect(p)).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let mut other = Session::new();
    let id2 = other.open("locked.pdf", None, saved, None).unwrap();
    let before = other.get(id2).unwrap().bytes.clone();
    assert_eq!(other.apply(id2, Edit::FlattenFillSign), Err(EditError::NotPermitted("changes to the document")));
    assert_eq!(other.get(id2).unwrap().bytes, before);
    assert!(!other.get(id2).unwrap().dirty);
    assert!(comment_text(&other, id2).contains(&"Hello".to_string()));
}

#[test]
fn flattening_fill_and_sign_on_a_signed_file_stays_incremental() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(1)), None).unwrap();
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    s.mark_signed(id, signed.clone(), None).unwrap();
    s.apply(id, fill_item(Shape::Typewriter { rect: [20.0, 200.0, 120.0, 220.0], font_size: 10.0 }, "Hello")).unwrap();
    s.apply(id, Edit::FlattenFillSign).unwrap();
    let saved = s.save_bytes(id).unwrap();
    assert!(saved.starts_with(signed.as_ref()), "the signature's bytes stay an incremental update");
    assert!(!comment_text(&s, id).contains(&"Hello".to_string()));
}

#[test]
fn flattening_fill_and_sign_reopens_an_encrypted_file() {
    let (mut s, id) = session_with(1);
    s.apply(id, fill_item(Shape::Typewriter { rect: [20.0, 200.0, 120.0, 220.0], font_size: 10.0 }, "Hello")).unwrap();
    s.apply(id, Edit::Protect(protection(Some("secret"), None))).unwrap();
    s.apply(id, Edit::FlattenFillSign).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    assert!(again.open("p.pdf", None, saved.clone(), None).is_err());
    let id2 = again.open("p.pdf", None, saved, Some("secret")).unwrap();
    assert!(!comment_text(&again, id2).contains(&"Hello".to_string()));
}

#[test]
fn pages_export_as_png_jpeg_and_tiff() {
    use crate::export::{Exporter, ImageFormat};
    let (s, id) = session_with(1);
    let mut ex = Exporter::new(s.get(id).unwrap());
    let png = ex.image(0, 72.0, ImageFormat::Png).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    let jpg = ex.image(0, 72.0, ImageFormat::Jpeg { quality: 85 }).unwrap();
    let decoded = image::load_from_memory_with_format(&jpg, image::ImageFormat::Jpeg).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (200, 300), "the page at 72 dpi");
    let corner = decoded.to_rgb8().get_pixel(2, 2).0;
    assert!(corner.iter().all(|c| *c > 240), "white paper, not black: {corner:?}");
    let tif = ex.image(0, 144.0, ImageFormat::Tiff).unwrap();
    let decoded = image::load_from_memory_with_format(&tif, image::ImageFormat::Tiff).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (400, 600));
    let mut tags = tiff::decoder::Decoder::new(std::io::Cursor::new(&tif)).unwrap();
    let compression = tags.get_tag_u32(tiff::tags::Tag::Compression).unwrap();
    let predictor = tags.get_tag_u32(tiff::tags::Tag::Predictor).unwrap();
    assert_eq!((compression, predictor), (5, 1), "LZW, without a predictor");
}

/// A TIFF's resolution unit (2 = inch) and its horizontal and vertical resolution.
fn tiff_resolution(bytes: &[u8]) -> (u32, f64, f64) {
    use tiff::{decoder::ifd::Value, tags::Tag};
    let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes)).unwrap();
    let mut res = |tag| match d.get_tag(tag).unwrap() {
        Value::Rational(n, d) => f64::from(n) / f64::from(d),
        v => panic!("{tag:?} is {v:?}"),
    };
    let (x, y) = (res(Tag::XResolution), res(Tag::YResolution));
    (d.get_tag_u32(Tag::ResolutionUnit).unwrap(), x, y)
}

/// An exported TIFF or JPEG says the resolution it was drawn at, so other programs show and print it
/// at the page's size. A TIFF said 1 × 1 with no unit, and a JPEG said nothing.
#[test]
fn exported_pages_record_the_resolution_they_were_drawn_at() {
    use crate::export::{Exporter, ImageFormat};
    let (s, id) = session_with(1);
    let mut ex = Exporter::new(s.get(id).unwrap());
    assert_eq!(tiff_resolution(&ex.image(0, 144.0, ImageFormat::Tiff).unwrap()), (2, 144.0, 144.0));
    let jpg = ex.image(0, 144.0, ImageFormat::Jpeg { quality: 85 }).unwrap();
    assert_eq!(&jpg[6..11], b"JFIF\0");
    assert_eq!(jpg[13..18], [1, 0, 144, 0, 144], "JFIF density in dots per inch");
    // pHYs counts pixels per metre: 72 dpi would read back as 72.009, and a page made from the PNG
    // (doc_create from images) would come out 0.01% small.
    let png = ex.image(0, 72.0, ImageFormat::Png).unwrap();
    assert!(png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap().info().pixel_dims.is_none(), "a PNG states no resolution");

    // A page too large for the renderer at the asked resolution says the one it was drawn at.
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let bytes = s.create_blank(14_400.0, 792.0, 1).unwrap();
    let id = s.open("wide.pdf", None, bytes, None).unwrap();
    let mut ex = Exporter::new(s.get(id).unwrap());
    let used = ex.dpi_used(0, 600.0);
    let (unit, x, y) = tiff_resolution(&ex.image(0, 600.0, ImageFormat::Tiff).unwrap());
    assert!(unit == 2 && (x - used).abs() < 0.01 && (y - used).abs() < 0.01, "{x} × {y}, drawn at {used}");
}

#[test]
fn added_text_and_images_render_and_stay_editable() {
    let (mut s, id) = session_with(1);
    let text = AddedText { rect: [20.0, 250.0, 180.0, 280.0], text: "Added note".into(), size: 12.0, ..AddedText::default() };
    s.apply(id, Edit::AddText { page: 0, text: text.clone() }).unwrap();
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add text"));
    assert!(page_texts(&s, id)[0].contains("Added note"), "the renderer's extractor sees page content");
    // A PNG, centred at its natural size.
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 20, 10);
        enc.set_color(png::ColorType::Rgb);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[0u8; 600]).unwrap();
    }
    s.apply(id, Edit::AddImage { page: 0, rect: None, name: "dot.png".into(), bytes: Arc::new(png) }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.added().len(), 2);
    assert_eq!(d.added()[1].content.rect(), [90.0, 145.0, 110.0, 155.0], "20×10 px at 72 dpi, centred on 200×300");
    let moved = d.added()[0].content.with_rect([30.0, 100.0, 190.0, 130.0]);
    s.apply(id, Edit::UpdateContent { page: 0, index: 0, content: moved }).unwrap();
    assert_eq!(s.get(id).unwrap().added()[0].content.rect()[3], 130.0);
    s.apply(id, Edit::DeleteContent { page: 0, index: 1 }).unwrap();
    assert_eq!(s.get(id).unwrap().added().len(), 1);
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().added().len(), 2);
}

/// Edit ▸ Copy and Paste of added content (#2636): a text box comes back with all its style, an
/// image with its picture and transforms, in the same document and in another.
#[test]
fn pasted_added_content_keeps_its_style_and_picture() {
    let (mut s, id) = session_with(2);
    let text = AddedText {
        rect: [20.0, 250.0, 180.0, 264.4],
        text: "Styled note".into(),
        family: FontFamily::Times,
        bold: true,
        italic: true,
        size: 12.0,
        color: [0.8, 0.1, 0.2],
        align: TextAlign::Center,
        char_spacing: 0.0,
    };
    s.apply(id, Edit::AddText { page: 0, text: text.clone() }).unwrap();
    let copied = s.get(id).unwrap().added()[0].content.clone();
    let moved = copied.with_rect([40.0, 100.0, 200.0, 114.4]);
    s.apply(id, Edit::PasteContent { page: 1, content: moved.clone(), file: None }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Paste"));
    assert_eq!(doc.added()[1].page, 1);
    assert_eq!(doc.added()[1].content, moved, "every part of the style, in the new box");

    // An image, turned, flipped and cropped.
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 20, 10);
        enc.set_color(png::ColorType::Rgb);
        let px: Vec<u8> = (0..200).flat_map(|i| [(i % 256) as u8, 40, 200]).collect();
        enc.write_header().unwrap().write_image_data(&px).unwrap();
    }
    s.apply(id, Edit::AddImage { page: 0, rect: Some([50.0, 100.0, 150.0, 150.0]), name: "p.png".into(), bytes: Arc::new(png) }).unwrap();
    let AddedContent::Image(mut img) = s.get(id).unwrap().added()[1].content.clone() else { panic!("the image is the page's second item") };
    (img.rotation, img.flip_h, img.crop) = (1, true, [0.1, 0.0, 0.2, 0.05]);
    s.apply(id, Edit::UpdateContent { page: 0, index: 1, content: AddedContent::Image(img.clone()) }).unwrap();
    let (ext, file) = s.get(id).unwrap().added_image_file(0, 1).unwrap();
    assert_eq!(ext, "png");
    let file = Some(("Pasted image.png".to_string(), Arc::new(file)));
    assert!(s.get(id).unwrap().added_image_file(0, 0).is_err(), "the first item is text");
    let pasted = AddedImage { rect: [60.0, 90.0, 160.0, 140.0], ..img.clone() };
    s.apply(id, Edit::PasteContent { page: 0, content: AddedContent::Image(pasted.clone()), file: file.clone() }).unwrap();
    let AddedContent::Image(got) = s.get(id).unwrap().added()[2].content.clone() else { panic!("an image was pasted") };
    assert_eq!(got, pasted, "the same picture object, box, turn, flip and crop");

    // Into another document: the picture comes along as a file of its own.
    let other = s.open("other.pdf", None, Arc::new(fixture(1)), None).unwrap();
    s.apply(other, Edit::PasteContent { page: 0, content: AddedContent::Image(pasted.clone()), file: file.clone() }).unwrap();
    s.apply(other, Edit::PasteContent { page: 0, content: copied.clone(), file: None }).unwrap();
    let d = s.get(other).unwrap();
    let AddedContent::Image(theirs) = &d.added()[0].content else { panic!("an image was pasted") };
    assert_eq!(AddedImage { image: img.image, ..theirs.clone() }, pasted, "the same box and transforms");
    assert_eq!(d.added_image_file(0, 0).unwrap().1, file.as_ref().unwrap().1.as_slice(), "the same picture");
    assert_eq!(d.added()[1].content, copied);
    // Pasted there again, it draws the first paste's picture object rather than storing a third.
    let first = theirs.image;
    s.apply(other, Edit::PasteContent { page: 0, content: AddedContent::Image(pasted.clone()), file: file.clone() }).unwrap();
    let AddedContent::Image(again) = s.get(other).unwrap().added()[2].content.clone() else { panic!("an image was pasted") };
    assert_eq!(again.image, first, "one picture object for both pastes");
    // The object holds another picture of the same size (another document's numbering, an undone
    // add): the file is embedded instead of drawing the wrong picture.
    let mut other_png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut other_png, 20, 10);
        enc.set_color(png::ColorType::Rgb);
        enc.write_header().unwrap().write_image_data(&[7u8; 600]).unwrap();
    }
    let other_file = Some(("other.png".to_string(), Arc::new(other_png)));
    s.apply(id, Edit::PasteContent { page: 0, content: AddedContent::Image(pasted.clone()), file: other_file }).unwrap();
    let AddedContent::Image(fresh) = s.get(id).unwrap().added()[3].content.clone() else { panic!("an image was pasted") };
    assert_ne!(fresh.image, img.image, "a picture object of its own");
    assert_ne!(s.get(id).unwrap().added_image_file(0, 3).unwrap().1, file.as_ref().unwrap().1.as_slice());
    assert!(matches!(
        s.apply(id, Edit::PasteContent { page: 0, content: AddedContent::Image(pasted), file: None }),
        Err(EditError::Edit(pdfcraft_edit::EditError::Invalid(_)))
    ));
    // Four items on page 1 and the pasted text on page 2.
    assert_eq!(s.get(id).unwrap().added().len(), 5, "refused pastes add nothing");
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().added().len(), 4, "a paste is one undo step");
}

/// An added JPEG 2000 picture is copied as the file it was added from: pasted into its own
/// document it draws the same object, and into another one it is embedded once.
#[test]
fn an_added_jpeg_2000_image_is_copied_as_its_file() {
    // A JP2 file (30 × 20, three components) with a stub codestream, as Add Image takes it.
    let bx = |ty: &[u8], payload: &[u8]| [((payload.len() + 8) as u32).to_be_bytes().as_slice(), ty, payload].concat();
    let ihdr = [20u32.to_be_bytes().as_slice(), &30u32.to_be_bytes(), &[0, 3, 7, 7, 0, 0]].concat();
    let signature = [0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
    let file = [signature.as_slice(), &bx(b"ftyp", b"jp2 \0\0\0\0jp2 "), &bx(b"jp2h", &bx(b"ihdr", &ihdr)), &bx(b"jp2c", &[0xFF, 0x4F, 0xFF, 0x51])]
        .concat();
    let (mut s, id) = session_with(1);
    s.apply(id, Edit::AddImage { page: 0, rect: Some([50.0, 100.0, 110.0, 140.0]), name: "photo.jp2".into(), bytes: Arc::new(file.clone()) })
        .unwrap();
    let (ext, copied) = s.get(id).unwrap().added_image_file(0, 0).unwrap();
    assert_eq!((ext, &copied), ("jp2", &file), "the file as it was added");
    let AddedContent::Image(img) = s.get(id).unwrap().added()[0].content.clone() else { panic!("an image was added") };
    let copied = Arc::new(copied);
    let file = Some(("Pasted image.jp2".to_string(), copied.clone()));
    let pasted = AddedContent::Image(AddedImage { rect: [70.0, 80.0, 130.0, 120.0], ..img.clone() });
    s.apply(id, Edit::PasteContent { page: 0, content: pasted.clone(), file: file.clone() }).unwrap();
    assert!(matches!(&s.get(id).unwrap().added()[1].content, AddedContent::Image(i) if i.image == img.image), "the same picture object");
    let other = s.open("other.pdf", None, Arc::new(fixture(1)), None).unwrap();
    for _ in 0..2 {
        s.apply(other, Edit::PasteContent { page: 0, content: pasted.clone(), file: file.clone() }).unwrap();
    }
    let d = s.get(other).unwrap();
    let images: Vec<_> = d
        .added()
        .iter()
        .filter_map(|a| match &a.content {
            AddedContent::Image(i) => Some(i.image),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 2);
    assert_eq!(images[0], images[1], "embedded once");
    assert_eq!(d.added_image_file(0, 0).unwrap().1, copied.as_slice());
}

#[test]
fn a_batch_of_content_edits_refreshes_the_added_content_and_marks() {
    let (mut s, id) = session_with(1);
    let text = |t: &str, y: f64| AddedText { rect: [20.0, y, 180.0, y + 14.4], text: t.into(), ..AddedText::default() };
    let edits = vec![Edit::AddText { page: 0, text: text("One", 200.0) }, Edit::AddText { page: 0, text: text("Two", 150.0) }];
    s.apply(id, Edit::Batch { label: "Script".into(), edits }).unwrap();
    assert_eq!(s.get(id).unwrap().added().len(), 2);
    s.apply(id, Edit::Batch { label: "Script".into(), edits: vec![Edit::DeleteContent { page: 0, index: 0 }] }).unwrap();
    assert_eq!(s.get(id).unwrap().added().len(), 1);
    let watermark =
        Edit::AddWatermark { pages: vec![0], settings: Watermark { text: "Draft".into(), ..Watermark::default() }, file: None, replace: false };
    s.apply(id, Edit::Batch { label: "Script".into(), edits: vec![watermark] }).unwrap();
    assert_eq!(s.get(id).unwrap().marks().len(), 1);
}

#[test]
fn added_images_rotate_flip_and_crop_as_drawn() {
    // A 20×10 image: left half red, right half blue.
    let mut px = Vec::new();
    for _y in 0..10 {
        for x in 0..20 {
            px.extend_from_slice(if x < 10 { &[255, 0, 0] } else { &[0, 0, 255] });
        }
    }
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, 20, 10);
        enc.set_color(png::ColorType::Rgb);
        enc.write_header().unwrap().write_image_data(&px).unwrap();
    }
    let (mut s, id) = session_with(1);
    // A 100×100 box at (50, 100) on the 200×300 page.
    s.apply(id, Edit::AddImage { page: 0, rect: Some([50.0, 100.0, 150.0, 200.0]), name: "rb.png".into(), bytes: Arc::new(png) }).unwrap();
    let colour_at = |s: &Session, x: u32, y_from_top: u32| -> [u8; 3] {
        let doc = s.get(id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
        let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        let i = ((y_from_top * out.width + x) * 4) as usize;
        [out.rgba[i], out.rgba[i + 1], out.rgba[i + 2]]
    };
    let red = |c: [u8; 3]| c[0] > 200 && c[2] < 60;
    let blue = |c: [u8; 3]| c[2] > 200 && c[0] < 60;
    // Display y 100–200 is rows 100–200 from the top on a 300 pt page.
    assert!(red(colour_at(&s, 70, 150)) && blue(colour_at(&s, 130, 150)), "as placed: red left, blue right");
    let item = |s: &Session| s.get(id).unwrap().added()[0].content.clone();
    let AddedContent::Image(mut img) = item(&s) else { panic!() };
    img.rotation = 1;
    s.apply(id, Edit::UpdateContent { page: 0, index: 0, content: AddedContent::Image(img.clone()) }).unwrap();
    assert!(red(colour_at(&s, 100, 180)) && blue(colour_at(&s, 100, 120)), "a quarter turn left: red at the bottom");
    img.rotation = 0;
    img.flip_h = true;
    s.apply(id, Edit::UpdateContent { page: 0, index: 0, content: AddedContent::Image(img.clone()) }).unwrap();
    assert!(blue(colour_at(&s, 70, 150)) && red(colour_at(&s, 130, 150)), "flipped");
    img.flip_h = false;
    img.crop = [0.0, 0.0, 0.5, 0.0];
    s.apply(id, Edit::UpdateContent { page: 0, index: 0, content: AddedContent::Image(img) }).unwrap();
    assert!(red(colour_at(&s, 130, 150)), "the right half cropped away: red fills the box");
}

#[test]
fn reload_reads_the_new_file_into_the_same_document() {
    let (mut s, id) = session_with(2);
    let other = s.open("other.pdf", None, Arc::new(fixture(1)), None).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let before = s.get(id).unwrap().edit_generation();
    s.reload(id, Arc::new(fixture(3))).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!((d.id, d.name.as_str()), (id, "fixture.pdf"), "same document, same name");
    assert_eq!(page_texts(&s, id), ["Page 1", "Page 2", "Page 3"], "the new version");
    assert!(!d.dirty && d.can_undo().is_none(), "nothing unsaved, nothing to undo");
    assert!(d.edit_generation() > before, "caches keyed by the generation see a change");
    assert_eq!(s.docs().iter().map(|d| d.id).collect::<Vec<_>>(), [id, other], "no new tab, same order");
    // A file that isn't a PDF (yet) leaves the document as it was.
    assert!(s.reload(id, Arc::new(b"%PDF-1.7 half written".to_vec())).is_err());
    assert_eq!(page_texts(&s, id).len(), 3);
    assert_eq!(s.docs().len(), 2);
}

/// #3443: reloading the file (it changed on disk) keeps how the reader shows it. Reading colours
/// reached the new renderer, not just the setting; hidden comments stay hidden. Each setting is
/// checked alone, so neither one rides on the other being changed.
#[test]
fn reload_keeps_reading_colours_and_hidden_comments() {
    let dark_share = |s: &Session, id: DocId| {
        let doc = s.get(id).unwrap();
        doc.renderer.set_queue(vec![pdfcraft_render::RenderRequest {
            page: 0,
            kind: pdfcraft_render::RequestKind::Pixels,
            scale: 1.0,
            ..Default::default()
        }]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let page = loop {
            if let Some(page) = doc.renderer.try_recv() {
                break page;
            }
            assert!(std::time::Instant::now() < deadline, "page render did not finish");
            std::thread::yield_now();
        };
        let (px, _) = page.rgba.as_chunks::<4>();
        px.iter().filter(|p| p[0] < 64 && p[1] < 64 && p[2] < 64).count() as f64 / px.len() as f64
    };
    let (mut s, id) = session_with(1);
    assert!(dark_share(&s, id) < 0.1, "a white page as the file draws it");
    assert!(s.set_reading_colors(id, ReadingColors::WHITE_ON_BLACK));
    assert!(dark_share(&s, id) > 0.9, "white on black");
    s.reload(id, Arc::new(fixture(2))).unwrap();
    assert_eq!(s.reading_colors(id), Some(ReadingColors::WHITE_ON_BLACK));
    assert!(dark_share(&s, id) > 0.9, "the reloaded page is drawn white on black too");
    assert!(!s.get(id).unwrap().comments_hidden());

    let (mut s, id) = session_with(1);
    assert!(s.set_hide_comments(id, true));
    s.reload(id, Arc::new(fixture(2))).unwrap();
    assert!(s.get(id).unwrap().comments_hidden(), "hidden comments stay hidden");
    assert_eq!(s.reading_colors(id), Some(ReadingColors::default()));
    assert!(dark_share(&s, id) < 0.1);
}

#[test]
fn revert_goes_back_to_the_saved_version() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::DeletePages { pages: vec![0] }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    s.apply(id, Edit::InsertBlankPage { at: 0, width: 200.0, height: 300.0 }).unwrap();
    assert!(s.get(id).unwrap().dirty);
    s.revert(id).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.dirty && d.can_undo().is_none() && d.can_redo().is_none());
    assert_eq!(page_texts(&s, id), ["Page 2"], "the saved state, not the opened one");
    assert_eq!(d.info.pages[0].rotation, 0);
}

#[test]
fn split_by_size_and_bookmarks_and_page_filters() {
    let (mut s, id) = session_with(6);
    // Every page alone is a few hundred bytes: a limit of about two pages gives three parts.
    let one = s.split(id, &pdfcraft_organize::SplitBy::PageCount(1)).unwrap()[0].2.len();
    let parts = s.split_by_size(id, one * 2 + one / 2).unwrap();
    assert!(parts.len() >= 2 && parts.len() <= 6, "{}", parts.len());
    assert_eq!(parts.last().unwrap().1, 6, "every page is in a part");
    assert_eq!(s.split_by_size(id, 1).unwrap().len(), 6, "pages larger than the limit stand alone");
    // Top-level bookmarks name the parts.
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 0, title: "Intro".into(), page: 0, left: None, top: None }).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 1, title: "Results".into(), page: 3, left: None, top: None }).unwrap();
    assert_eq!(s.bookmark_splits(id), [(0, "Intro".to_string()), (3, "Results".to_string())]);
    // Filters: page numbers 2, 4, 6 are even.
    let info = &s.get(id).unwrap().info;
    let all: Vec<usize> = (0..6).collect();
    assert_eq!(filter_pages(info, &all, PageParity::Even, PageOrientation::Both), [1, 3, 5]);
    assert_eq!(filter_pages(info, &all, PageParity::Odd, PageOrientation::Landscape), Vec::<usize>::new());
    assert_eq!(filter_pages(info, &all, PageParity::Odd, PageOrientation::Portrait), [0, 2, 4]);
}

#[test]
fn links_from_urls_and_link_edits() {
    // One page whose text contains a web address.
    let mut objs: Vec<Vec<u8>> =
        vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 200] >>".to_vec()];
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    objs.push(b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec());
    let body = b"BT /F1 12 Tf 20 100 Td (Visit www.example.org today) Tj ET";
    objs.push([format!("<< /Length {} >>\nstream\n", body.len()).into_bytes(), body.to_vec(), b"\nendstream".to_vec()].concat());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new();
    let id = s.open("u.pdf", None, Arc::new(out), None).unwrap();
    let found = s.find_urls(id);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2, "http://www.example.org");
    s.apply(id, Edit::AddLinks { links: found, style: LinkStyle::default() }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.links().len(), 1);
    assert_eq!(d.info.links.len(), 1, "the viewer follows it");
    assert!(s.find_urls(id).is_empty(), "already linked");
    // Remove all links and undo.
    s.apply(id, Edit::RemoveLinks { pages: None }).unwrap();
    assert!(s.get(id).unwrap().links().is_empty());
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Remove all links"));
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().links().len(), 1);
    // Link Properties and delete.
    let l = s.get(id).unwrap().links()[0].clone();
    s.apply(id, Edit::SetLink { page: 0, index: l.index, rect: None, action: Some(LinkAction::Page(0)), style: None }).unwrap();
    assert_eq!(s.get(id).unwrap().links()[0].action, LinkAction::Page(0));
    s.apply(id, Edit::DeleteLink { page: 0, index: l.index }).unwrap();
    assert!(s.get(id).unwrap().links().is_empty());
    assert!(s.apply(id, Edit::RemoveLinks { pages: None }).is_err(), "nothing left to remove");
}

#[test]
fn appearance_only_edit_preserves_gotor_action_semantics() {
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [10 10 80 30] /A << /S /GoToR /F (target.pdf) /D [2 /FitH 180] /NewWindow true >> >>".to_vec(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());

    let mut session = Session::new();
    let id = session.open("source.pdf", Some("/docs/source.pdf".into()), Arc::new(bytes), None).unwrap();
    let before = session.get(id).unwrap().info.links[0].target.clone();
    let mut style = session.get(id).unwrap().links()[0].style;
    style.visible = true;
    style.width = 2.0;
    session.apply(id, Edit::SetLink { page: 0, index: 0, rect: None, action: None, style: Some(style) }).unwrap();

    let after = &session.get(id).unwrap().info.links[0].target;
    assert_eq!(*after, before, "the GoToR file, destination and /NewWindow hint must remain unchanged");
    assert_eq!(session.get(id).unwrap().links()[0].style, style, "the appearance edit is applied");
}

#[test]
fn comments_lock_take_checkmarks_hide_and_summarize() {
    let (mut s, id) = session_with(2);
    s.apply(id, rect_comment(1, [80.0, 80.0, 120.0, 120.0])).unwrap();
    s.apply(id, Edit::ReplyToAnnotation { page: 1, index: 0, text: "Agreed".into(), author: "Ada".into() }).unwrap();
    s.apply(id, Edit::LockAnnotation { page: 1, index: 0, locked: true }).unwrap();
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Lock comment"));
    assert!(s.get(id).unwrap().info.annotations[0].locked);
    assert!(s.apply(id, Edit::MoveAnnotation { page: 1, index: 0, dx: 5.0, dy: 0.0 }).is_err(), "locked comments stay put");
    assert!(s.apply(id, Edit::DeleteAnnotation { page: 1, index: 0 }).is_err(), "and can't be deleted");
    s.apply(id, Edit::SetAnnotationContents { page: 1, index: 0, text: "Still editable".into() }).unwrap();
    s.apply(id, Edit::LockAnnotation { page: 1, index: 0, locked: false }).unwrap();
    s.apply(id, Edit::MoveAnnotation { page: 1, index: 0, dx: 5.0, dy: 0.0 }).unwrap();

    s.apply(id, Edit::MarkAnnotation { page: 1, index: 0, marked: true, author: "Ada".into() }).unwrap();
    let a = &s.get(id).unwrap().info.annotations;
    let mark = a.iter().find(|r| r.is_mark()).expect("a checkmark reply");
    assert_eq!(mark.state.as_deref(), Some("Marked"));

    // Hiding comments drops the rectangle but keeps the page.
    let px = pixel(&s, id, 1, 105, 100);
    assert!(px[0] > 200 && px[1] < 40, "drawn: {px:?}");
    assert!(s.set_hide_comments(id, true));
    assert!(!s.set_hide_comments(id, true));
    assert_eq!(pixel(&s, id, 1, 105, 100), [255, 255, 255, 255]);
    assert!(s.set_hide_comments(id, false));
    assert!(pixel(&s, id, 1, 105, 100)[1] < 40);

    let text = comment_summary("doc.pdf", &s.get(id).unwrap().info.annotations, SummarySort::Page);
    assert!(text.starts_with("Summary of Comments on doc.pdf"));
    assert!(text.contains("Page: 2\nNumber: 1  Author: Reviewer  Subject: Rectangle"), "{text}");
    assert!(text.contains("Still editable") && text.contains("    Agreed"), "{text}");
    assert!(!text.contains("Marked"), "checkmarks aren't listed: {text}");
    let pdf = s.summarize_comments(id, SummarySort::Author).unwrap();
    let n = s.open_new("summary.pdf", pdf).unwrap();
    assert!(!s.get(n).unwrap().info.pages.is_empty());
    assert_eq!(comment_summary("x", &[], SummarySort::Page), "Summary of Comments on x\n\nThis document has no comments.\n");
}

/// #820: the generated summary includes replies to replies, indented one level deeper.
#[test]
fn comment_summary_includes_replies_to_replies() {
    let (mut s, id) = session_with(1);
    s.apply(id, rect_comment(0, [40.0, 40.0, 90.0, 90.0])).unwrap();
    s.apply(id, Edit::ReplyToAnnotation { page: 0, index: 0, text: "FIRST_REPLY".into(), author: "A".into() }).unwrap();
    s.apply(id, Edit::ReplyToAnnotation { page: 0, index: 1, text: "NESTED_REPLY".into(), author: "B".into() }).unwrap();
    let text = comment_summary("doc.pdf", &s.get(id).unwrap().info.annotations, SummarySort::Page);
    assert!(text.contains("FIRST_REPLY"), "{text}");
    assert!(text.contains("NESTED_REPLY"), "replies to replies are summarized: {text}");
    assert!(text.contains("        NESTED_REPLY"), "the nested reply is indented under its parent: {text}");
}

#[test]
fn reading_colors_set_and_invalidated() {
    let (mut s, id) = session_with(1);
    // Every open document holds a value; the default is Off.
    assert_eq!(s.reading_colors(id), Some(ReadingColors::default()));
    assert!(s.set_reading_colors(id, ReadingColors::WHITE_ON_BLACK));
    assert_eq!(s.reading_colors(id), Some(ReadingColors::WHITE_ON_BLACK));
    assert!(!s.set_reading_colors(id, ReadingColors::WHITE_ON_BLACK), "unchanged");
    assert!(s.set_reading_colors(id, ReadingColors::default()), "back to Off");
    assert_eq!(s.reading_colors(id), Some(ReadingColors::default()));
    // No such document.
    assert_eq!(s.reading_colors(DocId(u64::MAX)), None);
    assert!(!s.set_reading_colors(DocId(u64::MAX), ReadingColors::WHITE_ON_BLACK));
}

#[test]
fn signing_saving_trusting_and_commenting_afterwards() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(2)), None).unwrap();
    assert!(s.get(id).unwrap().signatures.is_empty());
    let opts = SignOptions { page: 1, rect: Some([20.0, 20.0, 180.0, 60.0]), reason: Some("Approved".into()), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    s.mark_signed(id, signed, Some("/tmp/signed.pdf".into())).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.name, "signed.pdf");
    assert!(doc.is_signed() && !doc.dirty && doc.can_undo().is_none(), "signing can't be undone");
    let sig = &doc.signatures[0];
    assert_eq!((sig.status, sig.page, sig.date.as_deref()), (SignatureStatus::Unknown, Some(1), Some("D:20270115080000Z")));
    assert_eq!(s.save_full_bytes(id).unwrap_err(), EditError::Signed);
    // Trusting the signer makes it valid; a later comment is permitted.
    s.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    assert_eq!(s.get(id).unwrap().signatures[0].status, SignatureStatus::Valid);
    s.apply(id, rect_comment(0, [80.0, 80.0, 120.0, 120.0])).unwrap();
    let sig = &s.get(id).unwrap().signatures[0];
    let comments = sign::Modification::Allowed(vec![sign::kind::COMMENTS]);
    assert_eq!((sig.status, sig.modification.clone()), (SignatureStatus::Valid, comments));
    // Saving keeps the signature (incremental).
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    assert_eq!(s.get(id).unwrap().signatures[0].status, SignatureStatus::Valid);
    // Edits that would need a full rewrite are refused, and change nothing.
    let before = s.get(id).unwrap().can_undo().map(str::to_owned);
    assert!(matches!(s.apply(id, Edit::Sanitize), Err(EditError::SignedRewrite(_))));
    assert!(!s.get(id).unwrap().dirty && s.get(id).unwrap().can_undo().map(str::to_owned) == before);
}

#[test]
fn validated_signatures_without_layers_keep_their_appearance() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(1)), None).unwrap();
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    s.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    let doc = s.get(id).unwrap();
    let sig = &doc.signatures[0];
    assert_eq!(sig.status, SignatureStatus::Valid);
    // Our own flat appearance has no validation layers: nothing is substituted, and
    // the display copy carries no generated caption.
    let display = doc.display.clone();
    assert!(!display.windows(b"Signature valid".len()).any(|w| w == b"Signature valid"));
    // With nothing to draw, the display copy aliases the file exactly as before.
    assert!(Arc::ptr_eq(&doc.display, &doc.bytes));
}

#[test]
fn signed_pages_render_trust_save_and_reopen() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(1)), None).unwrap();
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    // The signed bytes open elsewhere too: validity is in the file, not the session.
    let mut t = Session::new().with_clock(|| 1_800_000_000);
    let tid = t.open("signed.pdf", None, signed.clone(), None).unwrap();
    assert!(t.get(tid).unwrap().signatures.iter().any(|x| x.signed && x.status == SignatureStatus::Unknown));
    s.mark_signed(id, signed, None).unwrap();
    // The signed page renders, from the file and from the display copy alike.
    for bytes in [s.get(id).unwrap().bytes.clone(), s.get(id).unwrap().display.clone()] {
        let mut renderer = pdfcraft_render::PageRenderer::new(bytes, s.get(id).unwrap().config.clone());
        let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        assert!(rendered.width > 0 && rendered.height > 0);
    }
    // Trusting the signer makes it valid; saving keeps it valid across sessions.
    s.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    assert_eq!(s.get(id).unwrap().signatures[0].status, SignatureStatus::Valid);
    let saved = s.save_bytes(id).unwrap();
    let mut u = Session::new().with_clock(|| 1_800_000_000);
    u.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    let uid = u.open("saved.pdf", None, saved.clone(), None).unwrap();
    assert_eq!(u.get(uid).unwrap().signatures[0].status, SignatureStatus::Valid);
}

#[test]
fn signed_documents_round_trip_behind_a_user_password() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    // After the test certificate's start date (2026-10-02).
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes128,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let encrypted = Arc::new(pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap());
    // Wrong passwords refuse; the right one signs, validates, saves and reopens.
    let mut e = Session::new().with_clock(|| 1_800_000_000);
    assert!(e.open("enc.pdf", None, encrypted.clone(), Some("wrong")).is_err(), "wrong passwords refuse");
    let eid = e.open("enc.pdf", None, encrypted, Some("user")).unwrap();
    let signed = e.sign(eid, &digital_id, SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() }).unwrap();
    e.mark_signed(eid, signed, None).unwrap();
    e.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    assert_eq!(e.get(eid).unwrap().signatures[0].status, SignatureStatus::Valid);
    let saved = e.save_bytes(eid).unwrap();
    let mut f = Session::new().with_clock(|| 1_800_000_000);
    f.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    assert!(f.open("enc-saved.pdf", None, saved.clone(), Some("wrong")).is_err());
    let fid = f.open("enc-saved.pdf", None, saved.clone(), Some("user")).unwrap();
    assert_eq!(f.get(fid).unwrap().signatures[0].status, SignatureStatus::Valid);
}

/// ByteRange placeholder values: fixed width, patched after writing.
const LAYERED_BR_MARK: [i64; 3] = [1_111_111_111, 2_222_222_222, 3_333_333_333];

/// A one-page file whose signature field carries a layered appearance on the
/// page (background, words, vector glyph, border, unverified caption): object
/// numbers 1 catalog, 2 pages, 3 page, 4 field, 5 AcroForm, 6 /N, 7 frame,
/// 8 background, 9 words, 10 glyph, 11 caption, 12 border, 13 Helvetica,
/// 14 page content. The field is unsigned: `sign_prepared` adds the value.
fn layered_signable_fixture() -> Vec<u8> {
    fn stream(bbox: &str, resources: &str, ops: &str) -> String {
        format!("<< /Type /XObject /Subtype /Form /BBox {bbox} /Length {} /Resources {resources} >>\nstream\n{ops}\nendstream", ops.len())
    }
    let helv = "<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>";
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 14 0 R /Resources << /Font << /F1 13 0 R >> >> /Annots [4 0 R] >>"
            .to_string(),
        "<< /FT /Sig /T (Signature1) /Subtype /Widget /Rect [10 10 110 60] /P 3 0 R /AP << /N 6 0 R >> >>".to_string(),
        "<< /Fields [4 0 R] /SigFlags 3 >>".to_string(),
        stream("[0 0 50 30]", "<< /XObject << /frame 7 0 R >> >>", "q /frame Do Q"),
        stream(
            "[0 0 50 30]",
            "<< /XObject << /bg 8 0 R /words 9 0 R /glyph 10 0 R /bd 12 0 R /cap 11 0 R >> >>",
            "q /bg Do /words Do /glyph Do /bd Do /cap Do Q",
        ),
        stream("[0 0 100 100]", "<< >>", "0 g 0 0 100 100 re f"),
        stream("[0 0 100 100]", helv, "BT /F1 10 Tf 5 50 Td (Signed by Test) Tj ET"),
        stream("[0 0 50 30]", "<< >>", "0.8 0.1 0.1 RG 3 w 5 5 m 20 5 l 12 22 l h S"),
        stream("[0 21 50 30]", helv, "BT /F1 9 Tf 2 22 Td (Signature Not Verified) Tj ET"),
        stream("[0 0 100 100]", "<< >>", "0 g 0 0 100 100 re f"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 150 Td (Contract text) Tj ET\nendstream".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (k, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", k + 1).as_bytes());
    }
    let start = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for o in &offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF", objs.len() + 1).as_bytes());
    out
}

/// Whether `haystack` holds `needle`.
fn bytes_has(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Sign `doc`, whose field already carries its layered appearance, without
/// touching that appearance: reserve `/ByteRange` and `/Contents`, hash the
/// ranges and embed a detached CMS over them. The signed bytes keep the
/// prepared appearance, so they validate exactly like a product signer's file.
fn sign_prepared(doc: &mut pdfcraft_cos::Document, id: &sign::pkcs12::DigitalId, date: &str) -> Vec<u8> {
    use pdfcraft_cos::{Dict, Object, PdfString};
    let reserve = 8192 + id.certificate.raw.len() + id.chain.iter().map(|c| c.raw.len()).sum::<usize>();
    let mut v = Dict::new();
    v.set(b"Type".to_vec(), Object::name("Sig"));
    v.set(b"Filter".to_vec(), Object::name("Adobe.PPKLite"));
    v.set(b"SubFilter".to_vec(), Object::name("ETSI.CAdES.detached"));
    v.set(b"ByteRange".to_vec(), Object::Array([0].iter().chain(LAYERED_BR_MARK.iter()).map(|n| Object::Int(*n)).collect()));
    v.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0; reserve], hex: true }));
    v.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    let sig = doc.add(Object::Dict(v));
    // The field by name, never by number: a full save (encryption, repairs) is
    // free to renumber objects, and a hardcoded reference would stamp the value
    // onto whatever moves into that slot.
    let root = doc.root().expect("catalog");
    let af = doc.get(root).as_dict().expect("catalog dict").get(b"AcroForm").cloned().expect("AcroForm");
    let fields = doc.resolve(&af).as_dict().expect("AcroForm dict").get(b"Fields").cloned().expect("Fields");
    let field = doc.resolve(&fields).as_array().expect("Fields array").iter().filter_map(|o| o.as_ref()).next().expect("signature field");
    doc.update_dict(field, |d| d.set(b"V".to_vec(), Object::Ref(sig))).unwrap();
    let save = pdfcraft_cos::SaveOptions { object_streams: false, ..pdfcraft_cos::SaveOptions::default() };
    let mut out = pdfcraft_cos::write_incremental(doc, &save).unwrap();
    let mark = format!("0 {} {} {}", LAYERED_BR_MARK[0], LAYERED_BR_MARK[1], LAYERED_BR_MARK[2]);
    let br = out.windows(mark.len()).position(|w| w == mark.as_bytes()).expect("ByteRange placeholder");
    let start = out[..br].windows(3).rposition(|w| w == b"obj").expect("signature object");
    let end = br + out[br..].windows(6).position(|w| w == b"endobj").expect("signature end");
    let mut zeros = vec![b'<'];
    zeros.extend(std::iter::repeat_n(b'0', reserve * 2));
    zeros.push(b'>');
    let region = &out[start..end];
    let c = region.windows(zeros.len()).position(|w| w == zeros.as_slice()).expect("Contents placeholder");
    let (cstart, cend) = (start + c, start + c + zeros.len());
    let mut br_text = format!("0 {cstart} {cend} {}", out.len() - cend).into_bytes();
    assert!(br_text.len() <= mark.len(), "ByteRange patch fits its placeholder");
    br_text.resize(mark.len(), b' ');
    out[br..br + mark.len()].copy_from_slice(&br_text);
    let alg = id.key.preferred_digest();
    let digest = alg.digest(&[&out[..cstart], &out[cend..]]);
    let cms = sign::cms::sign_detached(&id.key, &id.certificate, &id.chain, alg, &digest).unwrap();
    assert!(cms.len() <= reserve, "signature fits its placeholder");
    let hex: Vec<u8> = cms.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    out[cstart + 1..cstart + 1 + hex.len()].copy_from_slice(&hex);
    out
}

/// Replace one synthetic appearance stream while retaining its resource dictionary.
fn signature_layer_ops(doc: &mut pdfcraft_cos::Document, number: u32, ops: &[u8]) {
    let r = pdfcraft_cos::ObjRef::new(number, 0);
    let dict = doc.resolve(&pdfcraft_cos::Object::Ref(r)).as_dict().unwrap().clone();
    doc.set(r, pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(dict, ops.to_vec())));
}

fn printed_pixels(bytes: Vec<u8>) -> (u32, u32, Vec<u8>) {
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), Default::default());
    let r = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(r.error.is_none(), "{:?}", r.error);
    (r.width, r.height, r.rgba.to_vec())
}

/// Validation belongs to viewer chrome, not to document artwork or export sources.
fn assert_signature_artwork_survives_trust(mut cos: pdfcraft_cos::Document, expected_text: &[&str]) {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let identity = sign::pkcs12::open(&p12, "test").unwrap();
    let anchor = identity.chain.first().cloned().unwrap_or_else(|| identity.certificate.clone());
    let signed = Arc::new(sign_prepared(&mut cos, &identity, "D:20261002120000+01'00'"));
    let mut session = Session::new().with_clock(|| 1_800_000_000);
    let id = session.open("artwork.pdf", None, signed.clone(), None).unwrap();
    let original_display = session.get(id).unwrap().display.clone();
    let mut exporter = export::Exporter::new(session.get(id).unwrap());
    let original_text = exporter.text_of(&[0]).unwrap();
    let original_png = exporter.png(0, 72.0).unwrap();
    for text in expected_text {
        assert!(original_text.contains(text), "fixture text {text:?} must be exported: {original_text:?}");
    }
    let mut settings = print::Settings { pages: vec![0], paper: (200.0, 200.0), ..Default::default() };
    let original_vector_print = printed_pixels(session.print_pdf(id, &settings).unwrap());
    settings.raster = Some(print::Raster { dpi: 72.0, grayscale: false });
    let original_raster_print = printed_pixels(session.print_pdf(id, &settings).unwrap());
    session.set_trusted_certificates(vec![anchor.clone()]);
    let doc = session.get(id).unwrap();
    assert_eq!(doc.signatures[0].status, SignatureStatus::Valid);
    assert!(doc.display == original_display, "trust must preserve all original appearance bytes");
    let mut exporter = export::Exporter::new(doc);
    assert_eq!(exporter.text_of(&[0]).unwrap(), original_text, "text exports retain signer details and baked caption");
    assert_eq!(exporter.png(0, 72.0).unwrap(), original_png, "image exports exclude the viewer overlay");
    assert_eq!(printed_pixels(session.print_pdf(id, &settings).unwrap()), original_raster_print, "Print as Image retains document artwork");
    settings.raster = None;
    assert_eq!(printed_pixels(session.print_pdf(id, &settings).unwrap()), original_vector_print);
    assert_eq!(session.save_bytes(id).unwrap(), signed, "unchanged saves retain the signed file exactly");
    session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Preserved artwork".into() }).unwrap();
    let saved = session.save_bytes(id).unwrap();
    assert!(saved.starts_with(&signed), "a real save appends without replacing signed appearance streams");
    let mut reopened = Session::new().with_clock(|| 1_800_000_000);
    reopened.set_trusted_certificates(vec![anchor]);
    let id = reopened.open("saved.pdf", None, saved, None).unwrap();
    let doc = reopened.get(id).unwrap();
    assert_eq!(doc.signatures[0].status, SignatureStatus::Valid);
    let mut exporter = export::Exporter::new(doc);
    assert_eq!(exporter.text_of(&[0]).unwrap(), original_text);
    assert_eq!(exporter.png(0, 72.0).unwrap(), original_png);
}

#[test]
fn chamfered_signature_borders_survive_trust_exports_printing_and_save() {
    for raster_glyph in [false, true] {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(layered_signable_fixture())).unwrap();
        signature_layer_ops(&mut doc, 12, b"0 G 1 w 10 1 m 90 1 l 99 10 l 99 90 l 90 99 l 10 99 l 1 90 l 1 10 l h S");
        signature_layer_ops(&mut doc, 10, b"0 G 2 w 0 0 50 30 re S");
        if raster_glyph {
            let mut image = pdfcraft_cos::Dict::new();
            image.set(b"Subtype", pdfcraft_cos::Object::name("Image"));
            image.set(b"Width", pdfcraft_cos::Object::Int(8));
            image.set(b"Height", pdfcraft_cos::Object::Int(8));
            image.set(b"ColorSpace", pdfcraft_cos::Object::name("DeviceRGB"));
            image.set(b"BitsPerComponent", pdfcraft_cos::Object::Int(8));
            let image = doc.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::from_raw(image, vec![0; 8 * 8 * 3])));
            let mut xobjects = pdfcraft_cos::Dict::new();
            xobjects.set(b"im", pdfcraft_cos::Object::Ref(image));
            xobjects.set(b"unused", pdfcraft_cos::Object::Ref(pdfcraft_cos::ObjRef::new(8, 0)));
            let mut resources = pdfcraft_cos::Dict::new();
            resources.set(b"XObject", pdfcraft_cos::Object::Dict(xobjects));
            doc.update_dict(pdfcraft_cos::ObjRef::new(10, 0), |d| d.set(b"Resources", pdfcraft_cos::Object::Dict(resources))).unwrap();
            signature_layer_ops(&mut doc, 10, b"q 20 0 0 20 5 5 cm /im Do Q");
        }
        assert_signature_artwork_survives_trust(doc, &["Not Verified"]);
    }
}

#[test]
fn mixed_signature_caption_details_survive_trust_exports_printing_and_save() {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(layered_signable_fixture())).unwrap();
    signature_layer_ops(&mut doc, 11, b"BT /F1 5 Tf 2 27 Td (Alice Signer) Tj 0 -3 Td (alice@example.org) Tj 0 -3 Td (Signature Not Verified) Tj ET");
    assert_signature_artwork_survives_trust(doc, &["Alice Signer", "alice@example.org", "Not Verified"]);
}

/// A real layered signature through `Session`: validity, original artwork,
/// rendering, saving and reopening. `password` opens the signed bytes.
fn check_layered_round_trip(signed: Arc<Vec<u8>>, password: Option<&str>, anchor: &sign::x509::Certificate) {
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let vid = s.open("layered.pdf", None, signed.clone(), password).unwrap();
    let doc = s.get(vid).unwrap();
    let sig = &doc.signatures[0];
    assert!(sig.signed && sig.field == "Signature1" && sig.page == Some(0) && sig.visible);
    assert_eq!(sig.status, SignatureStatus::Unknown, "untrusted at first");
    // Trust changes the verdict while preserving every appearance byte. The page
    // overlay is UI chrome and is not part of the document's display/export source.
    s.set_trusted_certificates(vec![anchor.clone()]);
    let doc = s.get(vid).unwrap();
    assert_eq!(doc.signatures[0].status, SignatureStatus::Valid);
    assert!(Arc::ptr_eq(&doc.display, &doc.bytes), "trust does not build a signature display copy");
    assert!(bytes_has(&doc.display, b"Not Verified"));
    assert!(!bytes_has(&doc.display, b"Signature valid"));
    let mut renderer = pdfcraft_render::PageRenderer::new(doc.display.clone(), doc.config.clone());
    let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    // Saving keeps the original appearance and the signature: the validated
    // state never reaches the file.
    let saved = s.save_bytes(vid).unwrap();
    assert!(bytes_has(&saved, b"Not Verified"), "saved bytes keep the baked appearance");
    assert!(!bytes_has(&saved, b"Signature valid"));
    let mut t = Session::new().with_clock(|| 1_800_000_000);
    t.set_trusted_certificates(vec![anchor.clone()]);
    let tid = t.open("layered-saved.pdf", None, saved.clone(), password).unwrap();
    let rsig = &t.get(tid).unwrap().signatures[0];
    assert_eq!(rsig.status, SignatureStatus::Valid);
}

/// A trusted layered signature whose original appearance remains document artwork.
fn trusted_layered_session() -> (Session, DocId) {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let id = sign::pkcs12::open(&p12, "test").unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let mut doc = pdfcraft_cos::Document::open(Arc::new(layered_signable_fixture())).unwrap();
    let signed = Arc::new(sign_prepared(&mut doc, &id, "D:20261002120000+01'00'"));
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let vid = s.open("layered.pdf", None, signed, None).unwrap();
    s.set_trusted_certificates(vec![anchor]);
    assert_eq!(s.get(vid).unwrap().signatures[0].status, SignatureStatus::Valid);
    (s, vid)
}

/// Metadata edits retain the original caption; the UI can still show its verdict badge.
#[test]
fn a_metadata_edit_keeps_the_original_signature_appearance() {
    let (mut s, vid) = trusted_layered_session();
    s.apply(vid, Edit::SetInfo { key: "Title".into(), value: "Contract".into() }).unwrap();
    let doc = s.get(vid).unwrap();
    assert_eq!(doc.signatures[0].status, SignatureStatus::Valid);
    assert!(bytes_has(&doc.display, b"Not Verified"));
    assert!(!bytes_has(&doc.display, b"Signature valid"));
}

/// An unchanged document never reaches the writer, so the edit is what makes this the
/// place the display copy would leak if saving ever read it.
#[test]
fn saving_after_an_edit_keeps_the_baked_appearance() {
    let (mut s, vid) = trusted_layered_session();
    s.apply(vid, Edit::SetInfo { key: "Title".into(), value: "Contract".into() }).unwrap();
    let saved = s.save_bytes(vid).unwrap();
    assert!(bytes_has(&saved, b"Not Verified"), "the writer keeps the baked appearance: {saved:?}");
    assert!(!bytes_has(&saved, b"Signature valid"), "the validated state never reaches the file");
}

#[test]
fn real_layered_signatures_keep_their_original_appearance() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let id = sign::pkcs12::open(&p12, "test").unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let date = "D:20261002120000+01'00'";
    let mut doc = pdfcraft_cos::Document::open(Arc::new(layered_signable_fixture())).unwrap();
    let signed = Arc::new(sign_prepared(&mut doc, &id, date));
    check_layered_round_trip(signed, None, &anchor);
}

#[test]
fn real_layered_signatures_round_trip_behind_a_user_password() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let id = sign::pkcs12::open(&p12, "test").unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let date = "D:20261002120000+01'00'";
    let mut doc = pdfcraft_cos::Document::open(Arc::new(layered_signable_fixture())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes128,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let encrypted = Arc::new(pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap());
    let mut doc = pdfcraft_cos::Document::open_with_password(encrypted, Some("user")).unwrap();
    let signed = Arc::new(sign_prepared(&mut doc, &id, date));
    let mut probe = Session::new().with_clock(|| 1_800_000_000);
    assert!(probe.open("enc.pdf", None, signed.clone(), Some("wrong")).is_err(), "wrong passwords refuse");
    // Encrypted appearance bytes remain unchanged through trust and save/reopen.
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let vid = s.open("enc.pdf", None, signed.clone(), Some("user")).unwrap();
    assert_eq!(s.get(vid).unwrap().signatures[0].status, SignatureStatus::Unknown);
    s.set_trusted_certificates(vec![anchor.clone()]);
    let doc = s.get(vid).unwrap();
    assert_eq!(doc.signatures[0].status, SignatureStatus::Valid);
    assert!(Arc::ptr_eq(&doc.display, &doc.bytes), "encrypted signature artwork remains original");
    let saved = s.save_bytes(vid).unwrap();
    let mut t = Session::new().with_clock(|| 1_800_000_000);
    t.set_trusted_certificates(vec![anchor.clone()]);
    assert!(t.open("enc-saved.pdf", None, saved.clone(), Some("wrong")).is_err());
    let tid = t.open("enc-saved.pdf", None, saved.clone(), Some("user")).unwrap();
    assert_eq!(t.get(tid).unwrap().signatures[0].status, SignatureStatus::Valid);
}

#[test]
#[ignore = "timing probe"]
fn probe_edit_latency_on_a_large_signed_document() {
    // ~120 MB: one page whose content stream is large.
    let big: Vec<u8> = (0..120_000_000u32).map(|i| b"0123456789 "[(i % 11) as usize]).collect();
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offs = vec![];
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", big.len() + 3).into_bytes(), b"%  ".to_vec(), big, b"\nendstream".to_vec()].concat(),
    ];
    for (i, o) in objs.iter().enumerate() {
        offs.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n", i + 1).bytes());
        pdf.extend(o);
        pdf.extend(b"\nendobj\n");
    }
    let x = pdf.len();
    pdf.extend(b"xref\n0 5\n0000000000 65535 f \n");
    for o in offs {
        pdf.extend(format!("{o:010} 00000 n \n").bytes());
    }
    pdf.extend(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n").bytes());
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let t = std::time::Instant::now();
    let id = s.open("big.pdf", None, Arc::new(pdf), None).unwrap();
    eprintln!("open: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    s.apply(id, rect_comment(0, [5.0, 10.0, 50.0, 50.0])).unwrap();
    eprintln!("comment edit before signing: {:?}", t.elapsed());
    s.undo(id).unwrap();
    let t = std::time::Instant::now();
    let signed = s.sign(id, &digital_id, SignOptions { rect: None, ..SignOptions::default() }).unwrap();
    eprintln!("sign: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    s.mark_signed(id, signed, None).unwrap();
    eprintln!("mark_signed (reopen + validate): {:?}", t.elapsed());
    for k in 0..3 {
        let t = std::time::Instant::now();
        s.apply(id, rect_comment(0, [10.0 + k as f64, 10.0, 50.0, 50.0])).unwrap();
        eprintln!("comment edit {k}: {:?}", t.elapsed());
    }
}

/// A tiny uncompressed BMP of one colour.
fn bmp(w: u32, h: u32, bgr: [u8; 3]) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = b"BM".to_vec();
    b.extend(size.to_le_bytes());
    b.extend([0u8; 4]);
    b.extend(54u32.to_le_bytes());
    b.extend(40u32.to_le_bytes());
    b.extend((w as i32).to_le_bytes());
    b.extend((h as i32).to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(24u16.to_le_bytes());
    b.extend([0u8; 24]);
    for _ in 0..h {
        for _ in 0..w {
            b.extend(bgr);
        }
        b.extend(vec![0u8; (row - w * 3) as usize]);
    }
    b
}

#[test]
fn backgrounds_and_watermarks_from_files() {
    let (mut s, id) = session_with(1);
    // A source PDF whose second page is solid blue.
    let blue = {
        let mut src = Session::new();
        let sid = src.open("src.pdf", None, Arc::new(fixture(2)), None).unwrap();
        src.apply(
            sid,
            Edit::AddBackground {
                pages: vec![1],
                settings: Background { color: [0.0, 0.0, 1.0], ..Background::default() },
                replace: false,
                file: None,
            },
        )
        .unwrap();
        src.save_bytes(sid).unwrap()
    };
    let file = MarkFile { name: "src.pdf".into(), bytes: blue, page: 1 };
    s.apply(id, Edit::AddBackground { pages: vec![0], settings: Background::default(), replace: false, file: Some(file) }).unwrap();
    let corner = pixel(&s, id, 0, 3, 3);
    assert!(corner[2] > 200 && corner[0] < 40, "the blue page fills the background: {corner:?}");
    // The page's own text still draws on top (at "Page 1", 20,150).
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add background"));
    // A red image watermark at half the page, on top, unrotated.
    let red = MarkFile { name: "stamp.bmp".into(), bytes: Arc::new(bmp(4, 4, [0, 0, 255])), page: 0 };
    let wm = Watermark { rotation: 0.0, opacity: 1.0, scale: 0.5, ..Watermark::default() };
    s.apply(id, Edit::AddWatermark { pages: vec![0], settings: wm, replace: false, file: Some(red) }).unwrap();
    let centre = pixel(&s, id, 0, 100, 150);
    assert!(centre[0] > 200 && centre[2] < 40, "the red picture sits in the middle: {centre:?}");
    let edge = pixel(&s, id, 0, 10, 150);
    assert!(edge[2] > 200, "outside it the background shows: {edge:?}");
    // A bad file is refused clearly.
    let junk = MarkFile { name: "x.png".into(), bytes: Arc::new(b"not an image".to_vec()), page: 0 };
    assert!(s.apply(id, Edit::AddBackground { pages: vec![0], settings: Background::default(), replace: true, file: Some(junk) }).is_err());
}

/// Scan & OCR ▸ Recognize text on a page that is only a picture of text (needs the models:
/// `cargo xtask models`; skipped without them).
#[test]
fn recognize_text_reads_the_euro_sign() {
    if !ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let text = s.create_from_text("t", "Total amount due: €250 by Friday").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
    let scan = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
    let id = s.open("scan.pdf", None, scan, None).unwrap();
    s.recognize_text(id, &[], ocr::OcrSettings::default()).unwrap();
    let text = page_texts(&s, id)[0].clone();
    // The model was trained with € where the ocrs crate's alphabet has a second E.
    assert!(text.contains("€250"), "{text}");
}

const SYNTHETIC_SCAN_DPI: f64 = 300.0;

/// A scan of the OCR sentence at the source page's physical size, without a text layer.
fn synthetic_ocr_scan(s: &mut Session) -> (DocId, [f32; 2]) {
    let text = s.create_from_text("t", "The quick brown fox jumps over the lazy dog.").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    let original = &s.get(id).unwrap().info.pages[0];
    let size = [original.width, original.height];
    // A typical 300 DPI scan keeps the small leading glyph in "quick" legible to OCR.
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, SYNTHETIC_SCAN_DPI).unwrap();
    // Exported PNGs have no resolution metadata. The 72 DPI import default would enlarge
    // the page and change the PDF-point coordinates the placement assertions exercise.
    let scan = s.create_from_images_with_resolution(&[("scan.png".into(), png)], ImageResolution::Dpi(SYNTHETIC_SCAN_DPI)).unwrap();
    (s.open("scan.pdf", None, scan, None).unwrap(), size)
}

#[test]
fn synthetic_ocr_scan_keeps_the_original_page_size_without_models() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let (id, expected) = synthetic_ocr_scan(&mut s);
    let page = &s.get(id).unwrap().info.pages[0];
    // Rounding to whole raster pixels may change a page edge by at most one pixel.
    let tolerance = 72.0 / SYNTHETIC_SCAN_DPI;
    for (actual, expected) in [page.width, page.height].into_iter().zip(expected) {
        assert!(f64::from((actual - expected).abs()) <= tolerance, "page edge {actual} pt, expected {expected} pt");
    }
    assert_eq!(page_texts(&s, id), [""], "the scan has no text until OCR runs");
}

#[test]
fn recognize_text_makes_a_scanned_page_searchable() {
    if !ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let (id, _) = synthetic_ocr_scan(&mut s);
    assert_eq!(page_texts(&s, id), [""], "a picture has no text");

    let found = s.recognize_text(id, &[], ocr::OcrSettings::default()).unwrap();
    assert_eq!(found.len(), 1);
    let text = page_texts(&s, id)[0].to_lowercase();
    for w in ["quick", "brown", "fox", "lazy"] {
        assert!(text.contains(w), "{text}");
    }
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Recognize text"));
    // The text sits over the words: "quick" is left of "lazy" and on the same line.
    let words = &found[0].words;
    let q = words.iter().find(|w| w.text.to_lowercase().contains("quick")).unwrap();
    let l = words.iter().find(|w| w.text.to_lowercase().contains("lazy")).unwrap();
    let top = |w: &ocr::PlacedWord| w.origin[1] + w.up[1];
    // OCR boxes can differ by a few pixels at 300 DPI and still sit on the same line.
    let line_tolerance = q.up[1].abs().max(l.up[1].abs()) / 4.0;
    assert!(q.origin[0] < l.origin[0] && (top(q) - top(l)).abs() < line_tolerance, "{q:?} {l:?}");

    // A second pass skips the page: it has text now.
    let again = s.recognize_text(id, &[], ocr::OcrSettings::default()).unwrap();
    assert!(again[0].skipped.is_some());
}

/// #1028: a page with its own text and a picture of text. By default it is skipped, as before;
/// with `skip_text_pages: false` the picture is read and the page's own text is not added again.
#[test]
fn recognize_text_reads_a_picture_on_a_page_that_has_text() {
    if !ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let text = s.create_from_text("t", "The quick brown fox jumps over the lazy dog.").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
    let scan = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
    let id = s.open("scan.pdf", None, scan, None).unwrap();
    // Real text in the empty lower part of the picture.
    let (w, h) = {
        let p = &s.get(id).unwrap().info.pages[0];
        (p.width as f64, p.height as f64)
    };
    let note = AddedText { rect: [w * 0.1, h * 0.75, w * 0.9, h * 0.85], text: "Invoice reference 4711".into(), size: 18.0, ..Default::default() };
    s.apply(id, Edit::AddText { page: 0, text: note }).unwrap();
    let count = |s: &Session, word: &str| page_texts(s, id)[0].to_lowercase().matches(word).count();
    assert_eq!((count(&s, "invoice"), count(&s, "brown")), (1, 0));

    let skipped = s.recognize_text(id, &[], ocr::OcrSettings::default()).unwrap();
    assert!(skipped[0].skipped.is_some(), "pages with text are still skipped by default");
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add text"), "nothing was added");

    let hybrid = ocr::OcrSettings { skip_text_pages: false, ..Default::default() };
    let found = s.recognize_text(id, &[], hybrid.clone()).unwrap();
    assert_eq!(found[0].skipped, None);
    assert!(found[0].already_on_page >= 2, "the page's own words were recognised and left out: {:?}", found[0]);
    let added = found[0].text().to_lowercase();
    assert!(added.contains("brown") && added.contains("lazy") && !added.contains("invoice"), "{added}");
    assert_eq!((count(&s, "invoice"), count(&s, "brown")), (1, 1), "{:?}", page_texts(&s, id));
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Recognize text"));

    // Reading the page again adds nothing: every word is there now.
    let again = s.recognize_text(id, &[], hybrid).unwrap();
    assert!(again[0].words.is_empty(), "{:?}", again[0]);
    assert_eq!((count(&s, "invoice"), count(&s, "brown")), (1, 1));
}

/// Edit reads a scan that only has comments on it, not a page with real text and a picture
/// (`native_text_only`): a text box over the picture is not the page's text, page content is.
#[test]
fn native_text_only_ignores_comments_but_not_page_text() {
    if !ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let text = s.create_from_text("t", "The quick brown fox jumps over the lazy dog.").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
    let scan = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
    let comment_only = |s: &mut Session| {
        let id = s.open("scan.pdf", None, scan.clone(), None).unwrap();
        let (w, h) = {
            let p = &s.get(id).unwrap().info.pages[0];
            (p.width as f64, p.height as f64)
        };
        let shape = pdfcraft_annot::Shape::TextBox { rect: [w * 0.1, h * 0.75, w * 0.9, h * 0.85], font_size: 18.0 };
        s.apply(
            id,
            Edit::AddAnnotation(NewAnnotation {
                page: 0,
                style: pdfcraft_annot::Style::default_for(&shape),
                shape,
                contents: "Invoice reference 4711".into(),
                author: "Test".into(),
            }),
        )
        .unwrap();
        id
    };
    let native = ocr::OcrSettings { native_text_only: true, ..Default::default() };

    // A comment over the picture: the default run counts it as text and skips the page, the
    // native-text run reads the picture.
    let id = comment_only(&mut s);
    let skipped = s.recognize_text(id, &[], ocr::OcrSettings::default()).unwrap();
    assert!(skipped[0].skipped.is_some(), "a comment is text for the default run");
    let found = s.recognize_text(id, &[], native.clone()).unwrap();
    assert_eq!(found[0].skipped, None, "{:?}", found[0]);
    assert!(found[0].text().to_lowercase().contains("brown"), "{:?}", found[0]);

    // Page content over the picture is the page's own text: skipped, whatever the setting.
    let id = comment_only(&mut s);
    let (w, h) = {
        let p = &s.get(id).unwrap().info.pages[0];
        (p.width as f64, p.height as f64)
    };
    let note = AddedText { rect: [w * 0.1, h * 0.6, w * 0.9, h * 0.7], text: "Page text".into(), size: 18.0, ..Default::default() };
    s.apply(id, Edit::AddText { page: 0, text: note }).unwrap();
    let skipped = s.recognize_text(id, &[], native).unwrap();
    assert!(skipped[0].skipped.is_some(), "{:?}", skipped[0]);
}

/// A form whose scripts are custom JavaScript: total = price × qty (calculate, through a
/// document-level function), shown with a custom format; qty is validated; a button script.
fn scripted_form() -> Vec<u8> {
    scripted_form_with("function times\\(a, b\\) { console.println('times'); return a * b; }")
}

/// [`scripted_form`] with `helpers` (PDF string syntax) as its document-level script.
fn scripted_form_with(helpers: &str) -> Vec<u8> {
    let text = |name: &str, y: u32, aa: &str| {
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [20 {y} 180 {}] /P 3 0 R /F 4 {aa} >>", y + 20)
    };
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript << /Names [(helpers) 10 0 R] >> >> /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 7 0 R] /CO [6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 8 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >>".to_string(),
        text("price", 250, "/V (2.5)"),
        text("qty", 220, "/AA << /V << /S /JavaScript /JS (if (event.value > 100) { app.alert('At most 100'); event.rc = false; }) >> >>"),
        text("total", 190, "/AA << /C << /S /JavaScript /JS (event.value = times\\(getField('price').value, getField('qty').value\\);) >> /F << /S /JavaScript /JS (event.value = 'USD ' + util.printf('%.2f', event.value);) >> >>"),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (go) /Rect [20 150 80 170] /P 3 0 R /F 4 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< >>".to_string(),
        format!("<< /S /JavaScript /JS ({helpers}) >>"),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn form_javascript_validates_calculates_and_formats() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("order.pdf", None, Arc::new(scripted_form()), None).unwrap();
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).unwrap().value.clone();
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("4".into()) }).unwrap();
    assert_eq!(value(&s, "total"), ["10"], "calculated by the script");
    assert!(page_texts(&s, id)[0].contains("USD 10.00"), "{:?}", page_texts(&s, id));
    let out = s.take_js_output(id);
    assert!(out.console.contains(&"times".to_string()), "{out:?}");

    let err = s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("500".into()) }).unwrap_err();
    assert_eq!(err.to_string(), "At most 100");
    assert_eq!(value(&s, "qty"), ["4"]);

    // A button script changes fields in one undoable step, and asks for a page.
    let o =
        s.run_javascript(id, "getField('price').value = 3; getField('total').readonly = true; app.alert('Done'); this.print();", Some("go")).unwrap();
    assert_eq!(o.alerts, ["Done"]);
    assert_eq!(o.requests, [js::Request::Print]);
    assert_eq!(value(&s, "total"), ["12"], "recalculated after the change");
    assert!(s.get(id).unwrap().form.iter().find(|f| f.name == "total").unwrap().read_only());
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Run JavaScript"));

    // JavaScript off: scripts don't run.
    s.set_javascript(false);
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("500".into()) }).unwrap();
    assert_eq!(value(&s, "total"), ["12"]);
    assert!(s.run_javascript(id, "1", None).is_err());
}

/// XFA forms check `xfa_installed` and `xfa_version` in a document script that runs before
/// each of their scripts, and alert when either is missing.
#[test]
fn scripts_in_an_xfa_form_see_the_xfa_plugin_installed() {
    let check = "typeof xfa_installed + ' ' + typeof xfa_version + ' ' + xfa_version";
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let xfa = s.open("form.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template(3))), None).unwrap();
    let o = s.run_javascript(xfa, check, None).unwrap();
    assert_eq!((o.error, o.result.as_deref()), (None, Some("boolean number 3.5")));
    let plain = s.open("order.pdf", None, Arc::new(scripted_form()), None).unwrap();
    let o = s.run_javascript(plain, check, None).unwrap();
    assert_eq!((o.error, o.result.as_deref()), (None, Some("undefined number 3.5")));
}

/// #3940: a document-level script that defines the helper and then calls a method PdfCraft lacks
/// (an Acrobat form copying a button icon at open) left every calculated field blank. The
/// helper stays defined, the form calculates, and the error is reported once.
#[test]
fn a_document_script_that_throws_still_lets_the_form_calculate() {
    let helpers =
        "function times\\(a, b\\) { return a * b; } this.getField\\('go'\\).buttonSetIcon\\(this.getField\\('go'\\).buttonGetIcon\\(\\)\\);";
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("order.pdf", None, Arc::new(scripted_form_with(helpers)), None).unwrap();
    let _ = s.take_js_output(id);
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("4".into()) }).unwrap();
    let total = s.get(id).unwrap().form.iter().find(|f| f.name == "total").unwrap().value.clone();
    assert_eq!(total, ["10"], "calculated through the helper the failing script defined");
    let out = s.take_js_output(id);
    assert_eq!(out.errors.len(), 1, "reported once, not once per script: {out:?}");
    assert!(out.errors[0].starts_with("document script 1: TypeError"), "{out:?}");
    // The console runs too, and says which document script failed.
    let o = s.run_javascript(id, "times(2, 3)", None).unwrap();
    assert_eq!((o.error, o.result.as_deref(), o.doc_errors.len()), (None, Some("6"), 1));
}

#[test]
fn active_xfa_does_not_run_pdf_container_scripts_but_keeps_requested_alerts() {
    let template = pdfcraft_xfa::fixtures::scripted_template().replace(
        r#"xfa.host.messageBox("Hello " + qty.rawValue);"#,
        r#"xfa.host.messageBox("Hello " + qty.rawValue); xfa.host.print(1, "0", "0", 0, 0, 0, 0, 0);"#,
    );
    let mut cos = pdfcraft_cos::Document::open(Arc::new(pdfcraft_xfa::fixtures::shell(&template))).unwrap();
    let fallback = "app.alert('This PDF form requires a newer version of Adobe Acrobat');";
    pdfcraft_forms::set_document_script(&mut cos, "viewerFallback", Some(fallback)).unwrap();
    let bytes = Arc::new(pdfcraft_cos::write_full(&cos, &Default::default()).unwrap());
    let mut s = Session::new();
    let id = s.open("xfa.pdf", None, bytes, None).unwrap();
    assert!(s.get(id).unwrap().xfa.is_some());
    assert!(s.take_js_output(id).alerts.is_empty());
    let out = s.run_javascript(id, "app.alert('Console message');", None).unwrap();
    assert!(out.error.is_none(), "{out:?}");
    assert_eq!(out.alerts, ["Console message"]);
    assert_eq!(s.get(id).unwrap().document_scripts(), [("viewerFallback".into(), fallback.into())]);
    let out = s.run_javascript(id, "", Some("hello")).unwrap();
    assert!(out.error.is_none(), "{out:?}");
    assert_eq!(out.alerts, ["Hello 2"]);
    assert_eq!(out.requests, [js::Request::Print]);
    // The same context rule applies to format/validate/calculate through JsRunner.
    s.apply(id, Edit::SetFieldScript { name: "qty".into(), event: "validate".into(), script: Some("app.alert('Field message');".into()) }).unwrap();
    s.take_js_output(id);
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("3".into()) }).unwrap();
    assert_eq!(s.take_js_output(id).alerts, ["Field message"]);
    // Saved XFA keeps its container scripts for inspection, but does not execute them.
    let saved = s.get(id).unwrap().bytes.clone();
    let reopened = s.open("saved.pdf", None, saved, None).unwrap();
    assert!(s.run_javascript(reopened, "", None).unwrap().alerts.is_empty());
    // Making the form static restores the ordinary PDF context; undo restores XFA's.
    s.apply(id, Edit::XfaToStatic).unwrap();
    assert_eq!(s.run_javascript(id, "", None).unwrap().alerts.len(), 1);
    s.undo(id).unwrap();
    assert!(s.run_javascript(id, "", None).unwrap().alerts.is_empty());
}

#[test]
fn active_xfa_without_template_scripts_does_not_initialize_pdf_fallback_scripts() {
    let template = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb"><pageSet><pageArea name="page">
<contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/>
</pageArea></pageSet><field name="text" w="200pt" h="30pt"><ui><textEdit/></ui></field></subform></template>"#;
    let mut s = Session::new();
    let id = s.open("no-scripts.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(template)), None).unwrap();
    assert!(s.get(id).unwrap().xfa.is_some());
    assert!(s.get(id).unwrap().xfa_template.is_none());
    s.apply(id, Edit::SetDocumentScript { name: "fallback".into(), script: Some("throw new Error('PDF fallback ran');".into()) }).unwrap();
    let out = s.run_javascript(id, "app.alert('Console message');", None).unwrap();
    assert!(out.error.is_none(), "{out:?}");
    assert_eq!(out.alerts, ["Console message"]);
}

#[test]
fn inactive_xfa_and_ordinary_pdfs_keep_document_helpers_and_alerts() {
    for bytes in [
        fixture(1),
        pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Test</name></page1></form1>"),
        pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template()),
    ] {
        let mut cos = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
        pdfcraft_forms::set_document_script(&mut cos, "helper", Some("function twice(n) { return n * 2; } app.alert('PDF message');")).unwrap();
        // The raw dynamic shell has not been laid out: XFA's presence alone is not enough.
        let runner = js::JsRunner::new(&cos, "control.pdf");
        let out = pdfcraft_js::run("twice(3)", &pdfcraft_js::Event::doc("Console"), &runner.doc, &[], &runner.doc_scripts, Default::default());
        assert!(out.error.is_none(), "{out:?}");
        assert_eq!(out.result.as_deref(), Some("6"));
        assert_eq!(out.alerts, ["PDF message"]);
    }
}

#[test]
fn edit_history_does_not_replay_field_script_side_effects() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("order.pdf", None, Arc::new(scripted_form()), None).unwrap();
    let value = |s: &Session, name: &str| s.get(id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone();
    let initial_qty = value(&s, "qty");
    let initial_total = value(&s, "total");
    assert!(s.take_js_output(id).is_empty());

    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("4".into()) }).unwrap();
    assert_eq!(value(&s, "qty"), ["4"]);
    assert_eq!(value(&s, "total"), ["10"]);
    let output = s.take_js_output(id);
    assert!(output.console.contains(&"times".to_string()), "{output:?}");
    assert_eq!(output.console.iter().filter(|line| *line == "times").count(), 1, "{output:?}");

    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Report".into() }).unwrap();
    assert!(s.take_js_output(id).is_empty(), "a metadata edit does not rerun field scripts");

    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().info.title, None);
    assert_eq!(value(&s, "qty"), ["4"]);
    assert!(s.take_js_output(id).is_empty(), "undoing metadata does not rerun field scripts");

    s.redo(id).unwrap();
    assert_eq!(s.get(id).unwrap().info.title.as_deref(), Some("Report"));
    assert_eq!(value(&s, "qty"), ["4"]);
    assert!(s.take_js_output(id).is_empty(), "redoing metadata does not rerun field scripts");

    s.undo(id).unwrap();
    s.undo(id).unwrap();
    assert_eq!(value(&s, "qty"), initial_qty);
    assert_eq!(value(&s, "total"), initial_total);
    assert!(s.take_js_output(id).is_empty(), "undoing the scripted edit does not replay its side effect");

    s.redo(id).unwrap();
    assert_eq!(value(&s, "qty"), ["4"]);
    assert_eq!(value(&s, "total"), ["10"]);
    assert!(s.take_js_output(id).is_empty(), "redoing the scripted edit restores its snapshot without replay");
}

#[test]
fn detecting_fields_on_a_printed_form() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let text = s.create_from_text("t", "Name: ______________________\n\nEmail address: ____________________\n\nPlain text without blanks.").unwrap();
    let id = s.open("paper.pdf", None, text, None).unwrap();
    let found = s.detect_fields(id, &[]);
    let names: Vec<&str> = found.iter().map(|(_, c)| c.name.as_str()).collect();
    assert_eq!(names, ["Name", "Email address"], "{found:?}");
    let added = s.auto_detect_fields(id, &[]).unwrap();
    assert_eq!(added, ["Name", "Email address"]);
    let d = s.get(id).unwrap();
    assert_eq!(d.form.len(), 2);
    assert_eq!(d.can_undo(), Some("Detect form fields"));
    // The field sits on the blank: right of the label, at its height.
    let f = &d.form[0];
    assert!(f.widgets[0].rect[0] > 72.0 + 20.0, "{:?}", f.widgets[0].rect);
    assert!(s.detect_fields(id, &[]).is_empty(), "nothing left to detect");
}

/// #294, #2831: a detected field sits on a line or box the page already draws, so it gets no border
/// and no fill that would cover the printed form. A field drawn by hand keeps the defaults.
#[test]
fn detected_fields_have_no_border_or_fill() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("paper.pdf", None, Arc::new(paper_form()), None).unwrap();
    let added = s.auto_detect_fields(id, &[]).unwrap();
    let d = s.get(id).unwrap();
    let kinds: Vec<pdfcraft_forms::FieldKind> = d.form.iter().map(|f| f.kind).collect();
    assert!(kinds.contains(&pdfcraft_forms::FieldKind::Text) && kinds.contains(&pdfcraft_forms::FieldKind::CheckBox), "{kinds:?}");
    for name in &added {
        let look = d.field_look(name).unwrap();
        assert_eq!((look.border, look.fill), (None, None), "{name}");
    }
    assert_eq!(d.can_undo(), Some("Detect form fields"), "still one step");

    s.apply(id, Edit::AddField { page: 0, rect: [200.0, 10.0, 300.0, 30.0], kind: NewField::Text { multiline: false }, name: Some("drawn".into()) })
        .unwrap();
    let look = s.get(id).unwrap().field_look("drawn").unwrap();
    assert!(look.border.is_some() && look.fill.is_some(), "{look:?}");
}

/// A printed form, 420 × 300 pt: a title, a label with a rule to write on, a box with a small
/// label in its corner, an empty box with its label above, and two check boxes.
fn paper_form() -> Vec<u8> {
    let content = "BT /F1 14 Tf 30 270 Td (Gewerbe-Ummeldung) Tj ET \
        BT /F1 10 Tf 30 230 Td (Name:) Tj ET 0.75 w 80 228 m 380 228 l S \
        1 w 30 150 350 50 re S BT /F1 7 Tf 34 191 Td (Anschrift) Tj ET \
        BT /F1 8 Tf 30 124 Td (Telefon) Tj ET 30 90 350 30 re S \
        30 50 10 10 re S BT /F1 9 Tf 46 51 Td (Haupt) Tj ET 150 50 10 10 re S BT /F1 9 Tf 166 51 Td (Zweig) Tj ET";
    let objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 420 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// How the paper form was scanned.
#[derive(Clone, Copy, Debug)]
enum Scan {
    Straight,
    /// Stored sideways and turned upright by `/Rotate 90`.
    Sideways,
    /// Speckled paper, uneven light and lines that lose some pixels.
    Noisy,
    /// Laid on the scanner a degree askew.
    Skewed,
}

/// The paper form as an image-only PDF at 150 dpi (the page keeps its 420 × 300 pt).
fn scanned_form(scan: Scan) -> Arc<Vec<u8>> {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("paper.pdf", None, Arc::new(paper_form()), None).unwrap();
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
    let mut img = image::load_from_memory(&png).unwrap().to_rgba8();
    match scan {
        Scan::Straight => {}
        Scan::Sideways => img = image::imageops::rotate270(&img),
        Scan::Skewed => {
            // Turned a degree about the centre (nearest pixel; the corners stay paper).
            let (w, h) = (img.width() as f64, img.height() as f64);
            let (sin, cos) = 1f64.to_radians().sin_cos();
            let src = img.clone();
            for (x, y, p) in img.enumerate_pixels_mut() {
                let (dx, dy) = (x as f64 - w / 2.0, y as f64 - h / 2.0);
                let (sx, sy) = (cos * dx + sin * dy + w / 2.0, -sin * dx + cos * dy + h / 2.0);
                *p = if sx >= 0.0 && sy >= 0.0 && sx < w && sy < h { *src.get_pixel(sx as u32, sy as u32) } else { image::Rgba([255; 4]) };
            }
        }
        Scan::Noisy => {
            let (w, mut seed) = (img.width(), 7u32);
            for (x, _, p) in img.enumerate_pixels_mut() {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = ((seed >> 16) % 48) as u8;
                let shade = (x * 50 / w) as u8;
                for c in &mut p.0[..3] {
                    *c = if *c < 128 && noise < 4 { 230 } else { c.saturating_sub(shade).saturating_sub(noise) };
                }
            }
        }
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let pdf = s.create_from_images_with_resolution(&[("scan.png".into(), bytes.into_inner())], ImageResolution::Dpi(150.0)).unwrap();
    if !matches!(scan, Scan::Sideways) {
        return pdf;
    }
    let id = s.open("scan.pdf", None, pdf, None).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    s.save_bytes(id).unwrap()
}

/// Fields on a scanned paper form are found from its lines and boxes, wherever the page turns
/// them, and named from the labels when text recognition is installed.
#[test]
fn detecting_fields_on_a_scanned_form() {
    use pdfcraft_forms::detect::Kind::{CheckBox, Text};
    for scan in [Scan::Straight, Scan::Sideways, Scan::Noisy, Scan::Skewed] {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("scan.pdf", None, scanned_form(scan), None).unwrap();
        assert_eq!(page_texts(&s, id), [""], "{scan:?}: a picture has no text");
        let info = s.get(id).unwrap().info.pages[0].clone();
        assert_eq!((info.width.round(), info.height.round()), (420.0, 300.0), "{scan:?}");
        // Where a field shows on the page, in points from the bottom-left as displayed.
        let shown = |r: [f64; 4]| {
            let a = info.user_to_view(r[0] as f32, r[1] as f32);
            let b = info.user_to_view(r[2] as f32, r[3] as f32);
            [a[0].min(b[0]), 300.0 - a[1].max(b[1]), a[0].max(b[0]), 300.0 - a[1].min(b[1])].map(f64::from)
        };
        let found = s.detect_fields(id, &[]);
        let got: Vec<(pdfcraft_forms::detect::Kind, [f64; 4])> = found.iter().map(|(_, c)| (c.kind.clone(), shown(c.rect))).collect();
        // In reading order: the rule after "Name:", the box below "Anschrift", the "Telefon"
        // box, two check boxes.
        let want = [
            (Text, [80.0, 229.0, 380.0, 245.0]),
            (Text, [31.0, 151.0, 379.0, 189.0]),
            (Text, [31.0, 91.0, 379.0, 119.0]),
            (CheckBox, [30.0, 50.0, 40.0, 60.0]),
            (CheckBox, [150.0, 50.0, 160.0, 60.0]),
        ];
        assert_eq!(got.len(), want.len(), "{scan:?}: {got:?}");
        for ((kind, r), (want_kind, w)) in got.iter().zip(&want) {
            assert_eq!(kind, want_kind, "{scan:?}: {got:?}");
            // The labelled box's top is wherever the label ends; everything else is close. A
            // degree askew, a 350 pt line climbs 6 pt, and the field stays inside the box.
            let tol = if matches!(scan, Scan::Skewed) { [6.5; 4] } else { [2.5, 2.5, 2.5, if w[3] == 189.0 { 6.0 } else { 2.5 }] };
            assert!(r.iter().zip(w).zip(tol).all(|((a, b), t)| (a - b).abs() <= t), "{scan:?}: {r:?}, want {w:?}");
        }
        let names: Vec<&str> = found.iter().map(|(_, c)| c.name.as_str()).collect();
        if ocr::available() {
            // (Askew, recognition may read "Haupt" and "Zweig" as one phrase.)
            assert!(names.contains(&"Name") && names.contains(&"Telefon") && names.iter().any(|n| n.starts_with("Haupt")), "{scan:?}: {names:?}");
        } else {
            // Without text recognition the fields get the default names.
            assert!(names.iter().all(|n| n.starts_with("Text") || n.starts_with("Check Box")), "{scan:?}: {names:?}");
        }

        // Added as one step, where they were found; on the turned page each field turns with
        // it, so typing reads upright.
        let added = s.auto_detect_fields(id, &[]).unwrap();
        assert_eq!(added.len(), want.len());
        let d = s.get(id).unwrap();
        assert_eq!(d.can_undo(), Some("Detect form fields"));
        let turn = if matches!(scan, Scan::Sideways) { 90 } else { 0 };
        for (_, c) in &found {
            let f = d.form.iter().find(|f| f.name == c.name).unwrap();
            assert_eq!(f.widgets[0].rotation, turn, "{scan:?}: {}", f.name);
            let r = f.widgets[0].rect;
            assert!(r.iter().zip(c.rect).all(|(a, b)| (a - b).abs() < 0.01), "{scan:?}: {r:?} moved from {:?}", c.rect);
        }
        assert!(s.detect_fields(id, &[]).is_empty(), "{scan:?}: nothing left to detect");
    }
}

#[test]
fn detecting_comb_boxes_adds_one_comb_field_per_row() {
    // The rows from #1887: five 18 x 22 cells and ten 16 x 20 cells, each a stroked `re`.
    let mut body = String::from("BT /F1 10 Tf 40 205 Td (Postcode A) Tj ET BT /F1 10 Tf 40 105 Td (Postcode B) Tj ET\n");
    for i in 0..5 {
        body.push_str(&format!("{} 200 18 22 re S\n", 120 + i * 18));
    }
    for i in 0..10 {
        body.push_str(&format!("{} 100 16 20 re S\n", 120 + i * 16));
    }
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("comb.pdf", None, Arc::new(one_line_page("/Font << /F1 4 0 R >>", &body)), None).unwrap();
    let found: Vec<(String, pdfcraft_forms::detect::Kind)> = s.detect_fields(id, &[]).into_iter().map(|(_, c)| (c.name, c.kind)).collect();
    assert_eq!(
        found,
        [
            ("Postcode A".to_string(), pdfcraft_forms::detect::Kind::Comb { cells: 5 }),
            ("Postcode B".to_string(), pdfcraft_forms::detect::Kind::Comb { cells: 10 })
        ]
    );
    s.auto_detect_fields(id, &[]).unwrap();
    let d = s.get(id).unwrap();
    let combs: Vec<(&str, FormFieldKind, bool, Option<usize>)> =
        d.form.iter().map(|f| (f.name.as_str(), f.kind, f.has(pdfcraft_forms::flags::COMB), f.max_len)).collect();
    assert_eq!(combs, [("Postcode A", FormFieldKind::Text, true, Some(5)), ("Postcode B", FormFieldKind::Text, true, Some(10))]);
    assert_eq!(d.can_undo(), Some("Detect form fields"));
}

#[test]
fn comparing_two_versions_of_a_document() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let v1 = s.create_from_text("t", "The contract starts on Monday.\n\nPayment is due within thirty days.\n\nSigned by both parties.").unwrap();
    let v2 = s
        .create_from_text("t", "The contract starts on Tuesday.\n\nPayment is due within thirty days of invoice.\n\nSigned by both parties.")
        .unwrap();
    let old = s.open("v1.pdf", None, v1, None).unwrap();
    let new = s.open("v2.pdf", None, v2, None).unwrap();
    let c = s.compare(old, new).unwrap();
    let got: Vec<(compare::Kind, &str, &str)> = c.changes.iter().map(|x| (x.kind, x.old.text.as_str(), x.new.text.as_str())).collect();
    assert_eq!(got, [(compare::Kind::Replaced, "Monday.", "Tuesday."), (compare::Kind::Replaced, "days.", "days of invoice.")], "{got:?}");
    assert!(s.compare(old, old).unwrap().identical());

    let report = s.compare_report(old, new).unwrap();
    let rid = s.open("report.pdf", None, report, None).unwrap();
    let text = page_texts(&s, rid).join(" ");
    assert!(text.contains("2 changes: 2 replaced") && text.contains("Tuesday."), "{text}");

    // Visually, only the changed words' places differ (and the page is otherwise the same).
    let vis = s.compare_visual(old, new, 72.0).unwrap();
    assert!(!vis.is_empty() && vis.iter().all(|(p, r)| *p == 0 && r[3] > 600.0), "{vis:?}");
    assert!(s.compare_visual(old, old, 72.0).unwrap().is_empty());

    assert_eq!(s.mark_differences(old, new).unwrap(), 2);
    let d = s.get(new).unwrap();
    assert_eq!(d.can_undo(), Some("Mark differences"));
    let marks: Vec<_> = d.info.annotations.iter().filter(|a| a.author.as_deref() == Some("Compare")).collect();
    assert_eq!(marks.len(), 2);
    assert!(marks[0].contents.as_deref().unwrap().starts_with("Replaced: \"Monday.\""));
}

#[test]
fn actions_run_their_steps_on_files() {
    let s = Session::new();
    let src = s.create_from_text("t", "Quarterly numbers").unwrap();
    let built = actions::builtin();
    let numbers = built.iter().find(|a| a.name == "Add Page Numbers").unwrap();
    let r = actions::run_on(numbers, "q.pdf", src.clone(), |_, _| {}).unwrap();
    let mut s2 = Session::new();
    let id = s2.open("out.pdf", None, r.bytes, None).unwrap();
    assert!(page_texts(&s2, id)[0].contains("Page 1 of 1"), "{:?}", page_texts(&s2, id));
    assert_eq!(r.log, ["Add footer"]);

    let custom = actions::Action {
        name: "Mine".into(),
        description: String::new(),
        steps: vec![
            actions::Step::from_id("set_title", "Q3").unwrap(),
            actions::Step::AddWatermark("DRAFT".into()),
            actions::Step::ReduceFileSize,
            actions::Step::FlattenComments,
        ],
        builtin: false,
    };
    let mut steps = Vec::new();
    let r = actions::run_on(&custom, "q.pdf", src, |i, n| steps.push((i, n))).unwrap();
    assert_eq!(steps, [(0, 4), (1, 4), (2, 4), (3, 4)]);
    assert_eq!(r.log.len(), 4, "{:?}", r.log);
    let id = s2.open("out2.pdf", None, r.bytes, None).unwrap();
    let d = s2.get(id).unwrap();
    assert_eq!(d.info.title.as_deref(), Some("Q3"));
    assert!(page_texts(&s2, id)[0].contains("DRAFT"));
    assert!(actions::run_on(&custom, "bad.pdf", Arc::new(b"nope".to_vec()), |_, _| {}).is_err());
}

/// A removal step that fails (here: a signed file can't be rewritten without its hidden data)
/// fails the action, so the file is not passed on as cleaned.
#[test]
fn actions_fail_when_hidden_information_could_not_be_removed() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(2)), None).unwrap();
    s.apply(id, Edit::SetInfo { key: "Author".into(), value: "Private Name".into() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    let signed = s.sign(id, &digital_id, SignOptions { page: 1, ..SignOptions::default() }).unwrap();
    let built = actions::builtin();
    let sanitize = built.iter().find(|a| a.name == "Sanitize Documents").unwrap();
    let e = actions::run_on(sanitize, "signed.pdf", signed, |_, _| {}).map(|r| r.log).unwrap_err();
    assert!(e.starts_with("Sanitize document:"), "{e}");
}

#[test]
fn exporting_office_files_keeps_images() {
    let mut s = Session::new();
    let text = s.create_from_text("t", "Picture page").unwrap();
    let id = s.open("t.pdf", None, text, None).unwrap();
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 20.0).unwrap();
    let pic = s.create_from_images(&[("p.png".into(), png)]).unwrap();
    let pid = s.open("p.pdf", None, pic, None).unwrap();
    let d = s.get(pid).unwrap();
    assert_eq!(d.export_pages()[0].images.len(), 1);
    let docx = d.export_office(compare::OfficeFormat::Docx);
    assert!(docx.windows(16).any(|w| w == b"word/media/image"));
    let pptx = d.export_office(compare::OfficeFormat::Pptx);
    assert!(pptx.windows(15).any(|w| w == b"ppt/media/image"));
    if let Ok(dir) = std::env::var("PDFCRAFT_EXPORT_DIR") {
        std::fs::write(format!("{dir}/pic.docx"), &docx).unwrap();
    }
    assert!(String::from_utf8(d.export_office(compare::OfficeFormat::Html)).unwrap().contains("data:image/png;base64,"));
}

/// #314: a page whose content is drawn through a form XObject exports its text (the file from
/// the report: no xref table, wrong stream lengths).
#[test]
fn exporting_office_files_reads_form_xobjects() {
    let pdf = b"%PDF-1.4
1 0 obj <</Type/Catalog/Pages 2 0 R>> endobj
2 0 obj <</Type/Pages/Kids[3 0 R]/Count 1>> endobj
3 0 obj <</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Resources<</XObject<</Fm1 7 0 R>>>>/Contents 5 0 R>> endobj
4 0 obj <</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>> endobj
5 0 obj <</Length 22>> stream
q 1 0 0 1 0 0 cm /Fm1 Do Q
endstream endobj
7 0 obj <</Type/XObject/Subtype/Form/BBox[0 0 595 842]/Resources<</Font<</F1 4 0 R>>>>/Length 60>> stream
BT /F1 18 Tf 72 760 Td (Hello from a test invoice) Tj ET
endstream endobj
trailer <</Root 1 0 R>>
%%EOF
";
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(pdf.to_vec()), None).unwrap();
    let d = s.get(id).unwrap();
    let blocks: Vec<String> = d.export_pages()[0].blocks.iter().map(|b| b.text.clone()).collect();
    assert_eq!(blocks, ["Hello from a test invoice"]);
    assert!(String::from_utf8(d.export_office(compare::OfficeFormat::Html)).unwrap().contains("Hello from a test invoice"));
    // Edit text still offers only what the page's own streams show.
    assert!(d.text_blocks(0).is_empty());
}

#[test]
fn exporting_office_files_keeps_text_colour() {
    // #526: a dark green heading came out black in Word.
    let content = "BT /F1 24 Tf 0.05 0.23 0.18 rg 72 700 Td (Annual Report) Tj ET \
BT /F1 11 Tf 0 g 72 650 Td (Black body text that is long enough to be the body size.) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".into(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new();
    let id = s.open("c.pdf", None, Arc::new(pdf), None).unwrap();
    let d = s.get(id).unwrap();
    let html = String::from_utf8(d.export_office(compare::OfficeFormat::Html)).unwrap();
    assert!(html.contains("style=\"color:#0D3B2E\">Annual Report"), "{html}");
    let pptx = d.export_office(compare::OfficeFormat::Pptx);
    assert!(pptx.starts_with(b"PK"));
    assert!(pptx.windows(b"ppt/slides/slide1.xml".len()).any(|w| w == b"ppt/slides/slide1.xml"));
    let blocks = &d.export_pages()[0].blocks;
    assert!(blocks.iter().any(|b| b.text == "Annual Report" && b.color == [0.05, 0.23, 0.18]), "{blocks:?}");
}

#[test]
fn office_export_positions_follow_the_displayed_crop_rotation_and_units() {
    use pdfcraft_cos::{Document as CosDocument, ObjRef, Object, SaveOptions, write_full};
    let mut pdf = CosDocument::open(Arc::new(fixture(1))).unwrap();
    let mut page = pdf.get(ObjRef::new(4, 0)).as_dict().unwrap().clone();
    page.set("CropBox", Object::Array([10, 20, 190, 280].map(Object::Int).to_vec()));
    page.set("Rotate", Object::Int(90));
    page.set("UserUnit", Object::Int(2));
    pdf.set(ObjRef::new(4, 0), page);
    let mut session = Session::new();
    let id = session.open("turned.pdf", None, Arc::new(write_full(&pdf, &SaveOptions::default()).unwrap()), None).unwrap();
    let doc = session.get(id).unwrap();
    let pages = doc.export_pages();
    let b = &pages[0].blocks[0];
    assert!((b.rotation + 90.0).abs() < 0.01, "clockwise page rotation: {b:?}");
    assert!((b.rect[0] - 248.0).abs() < 0.01, "crop and UserUnit applied: {b:?}");
    assert_eq!(b.size, 48.0);
    assert_eq!((pages[0].width, pages[0].height), (520.0, 360.0));
}

#[test]
fn guard_turns_a_panic_into_an_error() {
    assert_eq!(guard(|| 7), Ok(7));
    assert_eq!(guard(|| -> u8 { panic!("boom") }), Err("boom".to_string()));
    let n = 3;
    assert_eq!(guard(|| -> u8 { panic!("page {n} is bad") }), Err("page 3 is bad".to_string()));
}

#[test]
fn measurement_edits_are_transactional_and_honour_annotation_permissions() {
    let mut session = Session::default();
    let id = session.open("plan.pdf", None, Arc::new(fixture(1)), None).unwrap();
    let m = measure::NewMeasurement {
        page: 0,
        kind: measure::Kind::Distance,
        points: vec![[10.0, 20.0], [40.0, 60.0]],
        scale: measure::Scale::new(0.1, "m", 2).unwrap(),
        style: Style::default(),
        label: "Wall".into(),
        author: "Tester".into(),
    };
    session.apply(id, Edit::AddMeasurement(m.clone())).unwrap();
    let before = session.get(id).unwrap().bytes.clone();
    assert_eq!(session.get(id).unwrap().measurements().unwrap().measurements[0].reading.label, "5.00 m");
    let mut invalid = m.clone();
    invalid.points[1][0] = f64::INFINITY;
    assert!(session.apply(id, Edit::AddMeasurement(invalid)).is_err());
    assert_eq!(session.get(id).unwrap().bytes, before);
    session.undo(id).unwrap();
    assert!(session.get(id).unwrap().measurements().unwrap().measurements.is_empty());
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().bytes, before);
    let p = pdfcraft_cos::Permissions { bits: 0, owner: false };
    assert_eq!(check_permission(&Edit::AddMeasurement(m), &p), Err(EditError::NotPermitted("comments")));
    assert_eq!(
        check_permission(
            &Edit::SetMeasurementScale { page: 0, bbox: [0.0, 0.0, 100.0, 100.0], name: "Detail".into(), scale: measure::Scale::default() },
            &p
        ),
        Err(EditError::NotPermitted("comments"))
    );
}

/// A laid-out XFA field that doesn't scroll (`hScrollPolicy="off"`) opens as a text field with
/// DoNotScroll, and its text box says what fits it: what the editor takes no more than.
#[test]
fn an_xfa_field_that_doesnt_scroll_opens_with_do_not_scroll_and_a_text_box() {
    let xdp = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<field name="code" w="1in" h="0.3in"><ui><textEdit hScrollPolicy="off"/></ui><font typeface="Arial" size="10pt"/></field>
<field name="note" w="1in" h="0.3in"><ui><textEdit/></ui><font typeface="Arial" size="10pt"/></field>
</subform></template></xdp:xdp>"##;
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(xdp)), None).expect("opens");
    let doc = s.get(id).unwrap();
    let field = |end: &str| doc.form.iter().find(|f| f.name.ends_with(end)).unwrap_or_else(|| panic!("no field {end}"));
    assert!(field("code").has(field_flags::DO_NOT_SCROLL) && !field("note").has(field_flags::DO_NOT_SCROLL));
    let tb = doc.field_text_box(&field("code").name, 0).expect("a text box");
    assert!(tb.fits("A1B2") && !tb.fits(&"W".repeat(40)), "a 1-inch field at 10 pt");
    assert!(doc.field_text_box("no such field", 0).is_none() && doc.field_text_box(&field("code").name, 5).is_none());
}

/// A round XFA check button opens drawn round: the appearance its laid-out widget gets draws a
/// circle, the on state with its mark in it, while a square one keeps its box.
#[test]
fn round_xfa_check_buttons_open_drawn_round() {
    let xdp = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<field name="round" w="1in" h="0.3in"><ui><checkButton shape="round"/></ui><items><integer>1</integer><integer>0</integer></items></field>
<field name="square" w="1in" h="0.3in"><ui><checkButton/></ui><items><integer>1</integer><integer>0</integer></items></field>
</subform></template></xdp:xdp>"##;
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(xdp)), None).expect("opens");
    let cos = s.get(id).unwrap().editor.as_ref().unwrap().cos.clone();
    let all = pdfcraft_forms::fields(&cos);
    // The decoded appearance of `name`'s widget in `state`.
    let drawn = |name: &str, state: &[u8]| {
        let w = all.iter().find(|f| f.name.ends_with(name)).unwrap_or_else(|| panic!("no field {name}")).widgets[0].obj;
        let wd = cos.get(w).as_dict().cloned().unwrap();
        let n = cos.resolve(wd.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap());
        let r = n.as_dict().unwrap().reference(state).unwrap_or_else(|| panic!("{name} has no {} state", String::from_utf8_lossy(state)));
        let pdfcraft_cos::Object::Stream(st) = &*cos.get(r) else { panic!("a stream") };
        String::from_utf8_lossy(&st.decoded().unwrap()).into_owned()
    };
    for state in [&b"Off"[..], b"1"] {
        let round = drawn("round", state);
        assert!(round.contains(" c S") && !round.contains(" re S"), "the round one's border is a circle: {round}");
        assert!(drawn("square", state).contains(" re S"), "the square one keeps its box");
    }
}

#[test]
fn a_dynamic_xfa_form_is_saved_without_the_usage_rights_its_layout_voided() {
    let xdp = pdfcraft_xfa::fixtures::template(3);
    let original = Arc::new(pdfcraft_xfa::fixtures::usage_rights_shell(&xdp, "", false));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, original.clone(), None).expect("opens");
    let doc = s.get(id).unwrap();
    let warnings = &doc.xfa.as_ref().expect("laid out from the template").warnings;
    assert!(warnings.iter().any(|w| w.contains("usage rights")), "the user is told: {warnings:?}");
    assert!(!doc.dirty, "laying out is still not an edit");
    s.apply(id, Edit::SetFieldValue { name: "familyName".into(), value: FieldValue::Text("Singh".into()) }).unwrap();
    let incremental = s.save_bytes(id).unwrap();
    assert!(incremental.starts_with(&original[..]), "still an incremental save");
    // Saved either way, the file no longer carries them, and keeps what was filled in.
    for saved in [incremental, s.save_full_bytes(id).unwrap()] {
        let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
        let catalog = cos.get(cos.root().unwrap()).as_dict().cloned().unwrap();
        assert!(!catalog.contains(b"Perms"), "{catalog:?}");
        let again = s.open("saved.pdf", None, saved, None).unwrap();
        let d2 = s.get(again).unwrap();
        assert_eq!(d2.form.iter().find(|f| f.name == "familyName").unwrap().value, vec!["Singh".to_string()]);
        let warnings = &d2.xfa.as_ref().expect("the layout is remembered").warnings;
        assert!(!warnings.iter().any(|w| w.contains("usage rights")), "nothing left to say: {warnings:?}");
    }
}

#[test]
fn dynamic_xfa_forms_are_laid_out_on_open_filled_and_saved_incrementally() {
    let original = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template(3)));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, original.clone(), None).expect("opens");
    let doc = s.get(id).unwrap();
    let x = doc.xfa.as_ref().expect("laid out from the template");
    assert_eq!((x.pages, x.fields), (2, 14), "{x:?}");
    assert!(x.warnings.is_empty(), "{:?}", x.warnings);
    assert_eq!(doc.info.pages.len(), 2, "the placeholder page is gone");
    assert_eq!(doc.info.xfa, Some(pdfcraft_render::Xfa::Dynamic), "still an XFA form for Adobe's viewers");
    assert!(!doc.dirty, "laying out is not an edit");
    assert!(doc.bytes.starts_with(&original[..]), "the layout is an appended revision; the original bytes stay");
    let names: Vec<&str> = doc.form.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"familyName") && names.contains(&"answer") && names.contains(&"what2"), "{names:?}");
    let family = doc.form.iter().find(|f| f.name == "familyName").unwrap();
    assert_eq!((family.tooltip.as_deref(), family.max_len), (Some("Your family name"), Some(30)));
    let answer = doc.form.iter().find(|f| f.name == "answer").unwrap();
    assert_eq!(answer.widgets.iter().filter_map(|w| w.on_state.clone()).collect::<Vec<_>>(), ["Y", "N"]);
    assert!(page_texts(&s, id)[0].contains("Sample Form"), "{:?}", page_texts(&s, id));
    // Fill it in and save: one more incremental revision on top.
    s.apply(id, Edit::SetFieldValue { name: "familyName".into(), value: FieldValue::Text("Singh".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "answer".into(), value: FieldValue::Radio(Some("N".into())) }).unwrap();
    assert!(s.get(id).unwrap().dirty);
    let saved = s.save_bytes(id).unwrap();
    assert!(saved.starts_with(&original[..]));
    // The datasets packet carries the values too, for Adobe's viewers.
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    let data = pdfcraft_xfa::parse_datasets(&pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp).expect("a datasets packet");
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form[0].head[0].familyName[0]")), Some("Singh"));
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form[0].head[0].answer[0]")), Some("N"));
    let id2 = s.open("saved.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    assert_eq!(d2.info.pages.len(), 2, "a saved form is not laid out a second time");
    assert_eq!(d2.xfa.as_ref().map(|x| (x.pages, x.fields)), Some((2, 14)), "the layout is remembered");
    assert_eq!(d2.form.iter().find(|f| f.name == "familyName").unwrap().value, vec!["Singh".to_string()]);
    assert_eq!(d2.form.iter().find(|f| f.name == "answer").unwrap().value, vec!["N".to_string()]);
}

#[test]
fn a_signed_xfa_signature_box_shows_the_picture_the_file_holds() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("signed.pdf", None, Arc::new(pdfcraft_xfa::fixtures::signed_shell()), None).expect("opens");
    let doc = s.get(id).unwrap();
    assert_eq!(doc.xfa.as_ref().map(|x| (x.pages, x.fields)), Some((1, 2)));
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * out.width + x) * 4) as usize;
        out.rgba[i..i + 4].try_into().unwrap()
    };
    // The signed box (3in by 1in from half an inch in) is filled with the signature's blue
    // picture; the unsigned one below it is not.
    assert_eq!(at(36 + 108, 36 + 36), [0, 0, 255, 255]);
    assert_ne!(at(36 + 108, 36 + 72 + 36), [0, 0, 255, 255]);
}

#[test]
fn an_xfa_form_without_a_template_opens_with_its_placeholder_and_a_warning() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell("<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"></xdp:xdp>"));
    let mut s = Session::new();
    let id = s.open("form.pdf", None, bytes, None).expect("opens");
    let doc = s.get(id).unwrap();
    assert!(doc.xfa.is_none());
    assert_eq!(doc.info.pages.len(), 1);
    assert!(doc.info.warnings.iter().any(|w| w.contains("could not be laid out") && w.contains("no template")), "{:?}", doc.info.warnings);
}

#[test]
fn dynamic_xfa_forms_open_with_the_values_and_rows_of_their_data() {
    let data = "<form><head><familyName>Kaur</familyName><born>2001-02-03</born><answer>Y</answer></head><table><row><what0>a</what0></row><row><what0>b</what0></row><row><what0>c</what0></row></table></form>";
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template_with_data(1, data)));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, bytes, None).expect("opens");
    let doc = s.get(id).unwrap();
    let value = |n: &str| doc.form.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("no field {n}")).value.clone();
    assert_eq!(value("familyName"), vec!["Kaur".to_string()]);
    assert_eq!(value("born"), vec!["2001-02-03".to_string()]);
    assert_eq!(value("answer"), vec!["Y".to_string()]);
    assert_eq!(value("what0_3"), vec!["c".to_string()], "three rows from the data: {:?}", doc.form.iter().map(|f| &f.name).collect::<Vec<_>>());
    assert!(!doc.dirty);
}

/// #3180: an XFA form with no fields whose configuration says `dynamicRender` is `forbidden` is a
/// static form: its pages are the form. Opening it used to lay the template out into new pages
/// and write them as a revision, so even a save with no change replaced the pages.
#[test]
fn an_xfa_form_without_fields_that_forbids_dynamic_rendering_keeps_its_pages() {
    // The fixture's own config packet gains `acrobat/acrobat7/dynamicRender`.
    let template = pdfcraft_xfa::fixtures::template(1);
    let forbidden = "<acrobat><acrobat7><dynamicRender>forbidden</dynamicRender></acrobat7></acrobat><present>";
    let with_config = template.replacen("<present>", forbidden, 1);
    assert_ne!(with_config, template, "the fixture has a config packet");
    // The fixture shell without /NeedsRendering (blanked in place, so the offsets stay right).
    let shell = |xdp: &str| {
        let bytes = pdfcraft_xfa::fixtures::shell(xdp);
        let at = bytes.windows(20).position(|w| w == b"/NeedsRendering true").expect("the fixture sets it");
        let mut bytes = bytes;
        bytes.splice(at..at + 20, *b"                    ");
        Arc::new(bytes)
    };
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let original = shell(&with_config);
    let id = s.open("static.pdf", None, original.clone(), None).expect("opens");
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.xfa, Some(pdfcraft_render::Xfa::Static));
    assert!(doc.xfa.is_none(), "nothing was laid out");
    assert_eq!(doc.bytes[..], original[..], "no revision was appended on open");
    assert_eq!(doc.info.pages.len(), 1);
    // The same form without the config is still taken to be dynamic: laid out from the template
    // into an appended revision.
    let plain = shell(&template);
    let id = s.open("dynamic.pdf", None, plain.clone(), None).expect("opens");
    let doc = s.get(id).unwrap();
    assert!(doc.xfa.is_some(), "laid out from the template");
    assert!(doc.bytes.len() > plain.len() && doc.bytes.starts_with(&plain[..]), "the layout is a revision");
}

/// A static XFA form's fields take their values from the datasets on opening, and edits are
/// written back there.
#[test]
fn static_xfa_forms_take_their_values_from_the_datasets_and_write_them_back() {
    let original = Arc::new(pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name><agree>1</agree></page1></form1>"));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("static.pdf", None, original.clone(), None).expect("opens");
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.xfa, Some(pdfcraft_render::Xfa::Static));
    assert!(doc.xfa.is_none(), "nothing was laid out");
    assert_eq!(doc.form.iter().find(|f| f.name == "form1[0].page1[0].name[0]").unwrap().value, vec!["Ada".to_string()]);
    assert_eq!(doc.form.iter().find(|f| f.name == "form1[0].page1[0].agree[0]").unwrap().value, vec!["1".to_string()]);
    assert!(!doc.dirty && doc.bytes.starts_with(&original[..]), "the values are an appended revision");
    // Rewriting the fields from the XFA data is recorded, not silent.
    let note = doc.info.warnings.iter().find(|w| w.contains("taken from this form's XFA data")).expect("a warning");
    assert!(note.starts_with("2 form field values") && note.contains("form1[0].page1[0].name[0]"), "{note}");
    s.apply(id, Edit::SetFieldValue { name: "form1[0].page1[0].name[0]".into(), value: FieldValue::Text("Grace".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "form1[0].page1[0].agree[0]".into(), value: FieldValue::Check(false) }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    let data = pdfcraft_xfa::parse_datasets(&pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp).unwrap();
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form1[0].page1[0].name[0]")), Some("Grace"));
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form1[0].page1[0].agree[0]")), Some(""), "unchecked, with no off value");
    // Reopening agrees with itself: nothing to apply, nothing dirty.
    let id2 = s.open("again.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    assert_eq!(d2.form.iter().find(|f| f.name == "form1[0].page1[0].name[0]").unwrap().value, vec!["Grace".to_string()]);
    assert!(!d2.dirty);
    assert!(!d2.info.warnings.iter().any(|w| w.contains("taken from this form's XFA data")), "nothing was rewritten");
}

/// A date goes into the XFA data in the field's data picture (`<bind><picture>`), as other
/// viewers write and read it, and comes back from there when the form is opened again. It used
/// to be written as an ISO date whatever the picture said.
#[test]
fn xfa_dates_are_written_and_read_back_in_their_data_picture() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::bound_date_template("")));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, bytes, None).expect("opens");
    s.apply(id, Edit::SetFieldValue { name: "due".into(), value: FieldValue::Text("10/18/2026".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "when".into(), value: FieldValue::Text("10/18/2026".into()) }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    let data = pdfcraft_xfa::parse_datasets(&pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp).unwrap();
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form[0].page[0].due[0]")), Some("20261018"), "in the field's data picture");
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form[0].page[0].when[0]")), Some("2026-10-18"), "ISO without one");
    let id2 = s.open("again.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    let value = |n: &str| d2.form.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("no field {n}")).value.clone();
    assert_eq!(value("due"), vec!["10/18/2026".to_string()]);
    assert_eq!(value("when"), vec!["10/18/2026".to_string()]);
}

/// Same-named date fields are each written in their own data picture, and a data picture
/// without a day keeps the ISO date.
#[test]
fn each_xfa_date_field_is_written_in_its_own_data_picture_and_a_partial_one_keeps_iso() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::sibling_date_template("")));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, bytes, None).expect("opens");
    // Two siblings named `due` (laid out as `due` and `due_2`) and a field bound `YYYYMM`.
    for name in ["due", "due_2", "month"] {
        s.apply(id, Edit::SetFieldValue { name: name.into(), value: FieldValue::Text("10/18/2026".into()) }).unwrap();
    }
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    let data = pdfcraft_xfa::parse_datasets(&pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp).unwrap();
    let at = |som: &str| data.text_at(&pdfcraft_xfa::som_to_path(som)).map(str::to_string);
    assert_eq!(at("form[0].page[0].due[0]").as_deref(), Some("20261018"), "the first sibling's data picture");
    assert_eq!(at("form[0].page[0].due[1]").as_deref(), Some("18102026"), "the second sibling's own");
    // A data picture without a day would drop the one the field shows: the data keeps ISO.
    assert_eq!(at("form[0].page[0].month[0]").as_deref(), Some("2026-10-18"));
    let id2 = s.open("again.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    for name in ["due", "due_2", "month"] {
        let f = d2.form.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no field {name}"));
        assert_eq!(f.value, vec!["10/18/2026".to_string()], "{name} reads back");
    }
}

/// Data no field binds to (other namespaces, history) survives an edit and a save.
#[test]
fn xfa_data_no_field_binds_to_survives_an_edit_and_save() {
    // Regression: editing a field replaced the whole data element with the fields' values.
    let data = concat!(
        "<form1><page1><name>Ada</name><agree>1</agree><extra xmlns:my=\"urn:my\"><my:note id=\"7\">keep</my:note></extra></page1>",
        "<history><entry><when>2020</when></entry></history></form1><my:other xmlns:my=\"urn:my\"><my:x>unbound</my:x></my:other>"
    );
    let original = Arc::new(pdfcraft_xfa::fixtures::static_shell(data));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("static.pdf", None, original, None).expect("opens");
    s.apply(id, Edit::SetFieldValue { name: "form1[0].page1[0].name[0]".into(), value: FieldValue::Text("Grace".into()) }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    let xdp = pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp;
    for kept in [
        "<extra xmlns:my=\"urn:my\"><my:note id=\"7\">keep</my:note></extra>",
        "<history><entry><when>2020</when></entry></history>",
        "<my:other xmlns:my=\"urn:my\"><my:x>unbound</my:x></my:other>",
    ] {
        assert!(xdp.contains(kept), "lost {kept:?}: {xdp}");
    }
    let data = pdfcraft_xfa::parse_datasets(&xdp).unwrap();
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form1[0].page1[0].name[0]")), Some("Grace"));
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form1[0].page1[0].agree[0]")), Some("1"));
    // A value that can't be written is reported on the document.
    let original = Arc::new(pdfcraft_xfa::fixtures::static_shell("<form1><page1><name><b>rich</b></name></page1></form1>"));
    let id = s.open("rich.pdf", None, original, None).expect("opens");
    s.apply(id, Edit::SetFieldValue { name: "form1[0].page1[0].name[0]".into(), value: FieldValue::Text("plain".into()) }).unwrap();
    let doc = s.get(id).unwrap();
    assert!(doc.info.warnings.iter().any(|w| w.contains("structured content")), "{:?}", doc.info.warnings);
    assert!(doc.xfa_warnings.iter().any(|w| w.contains("structured content")));
}

#[test]
fn xfa_choice_lists_signature_and_picture_fields_open_fill_script_and_save() {
    let data = "<form><page1><country>Japan</country><langs>English\nSpanish</langs></page1></form>";
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::fields_template(data)));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("fields.pdf", None, bytes, None).expect("opens");
    let field = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).cloned().unwrap_or_else(|| panic!("no field {n}"));
    use pdfcraft_forms::FieldKind as K;
    // The forms layer sees a combo box with its options and the data's saved value, a
    // multi-select list with both picks, an editable combo, a password field and a signature.
    let country = field(&s, "country");
    assert_eq!((country.kind, country.value.clone()), (K::Combo, vec!["JP".to_string()]));
    assert_eq!(country.options, vec![("CA".into(), "Canada".into()), ("FR".into(), "France".into()), ("JP".into(), "Japan".into())]);
    let langs = field(&s, "langs");
    assert_eq!(langs.kind, K::List);
    assert!(langs.has(pdfcraft_forms::flags::MULTI_SELECT));
    assert_eq!(langs.value, vec!["English".to_string(), "Spanish".to_string()]);
    assert!(field(&s, "other").has(pdfcraft_forms::flags::EDIT));
    assert!(field(&s, "pin").has(pdfcraft_forms::flags::PASSWORD));
    assert_eq!(field(&s, "sign").kind, K::Signature);
    // Scripts read a choice list's saved value.
    assert_eq!(field(&s, "summary").value, vec!["Country: JP".to_string()], "{:?}", s.take_js_output(id));
    // Choosing another item recalculates; the list takes several values; a typed value is kept
    // in the editable list and refused in the fixed one.
    s.apply(id, Edit::SetFieldValue { name: "country".into(), value: FieldValue::Choice(vec!["CA".into()]) }).unwrap();
    assert_eq!(field(&s, "summary").value, vec!["Country: CA".to_string()]);
    s.apply(id, Edit::SetFieldValue { name: "langs".into(), value: FieldValue::Choice(vec!["French".into(), "Spanish".into()]) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "other".into(), value: FieldValue::Text("typed".into()) }).unwrap();
    assert!(s.apply(id, Edit::SetFieldValue { name: "country".into(), value: FieldValue::Choice(vec!["Mars".into()]) }).is_err());
    // Saved: the datasets hold the saved values, several on separate lines; reopened, the
    // fields and the calculation are what they were.
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    let d = pdfcraft_xfa::data_of(&cos).unwrap();
    assert_eq!(d.text_at(&pdfcraft_xfa::som_to_path("form[0].page1[0].country[0]")), Some("CA"));
    assert_eq!(d.text_at(&pdfcraft_xfa::som_to_path("form[0].page1[0].langs[0]")), Some("French\nSpanish"));
    assert_eq!(d.text_at(&pdfcraft_xfa::som_to_path("form[0].page1[0].other[0]")), Some("typed"));
    let id2 = s.open("again.pdf", None, saved.clone(), None).unwrap();
    let d2 = s.get(id2).unwrap();
    let f2 = |n: &str| d2.form.iter().find(|f| f.name == n).unwrap().value.clone();
    assert_eq!(f2("country"), vec!["CA".to_string()]);
    assert_eq!(f2("langs"), vec!["French".to_string(), "Spanish".to_string()]);
    assert_eq!(f2("summary"), vec!["Country: CA".to_string()]);
    // The PNG of the image field and the GIF of the draw are pictures on the page.
    assert_eq!(s.get(id).unwrap().page_images(0).len(), 2);
    // Filled in by another viewer since: saved values, several on separate lines, and shown
    // text (an older viewer's), are taken on reopen; shown text that means the current value
    // is not a change.
    let mut cos = pdfcraft_cos::Document::open(saved).unwrap();
    pdfcraft_xfa::write_data_value(&mut cos, &pdfcraft_xfa::som_to_path("form[0].page1[0].langs[0]"), "English\nSpanish").unwrap();
    pdfcraft_xfa::write_data_value(&mut cos, &pdfcraft_xfa::som_to_path("form[0].page1[0].country[0]"), "Canada").unwrap();
    let edited = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap());
    let id3 = s.open("elsewhere.pdf", None, edited, None).unwrap();
    let d3 = s.get(id3).unwrap();
    let f3 = |n: &str| d3.form.iter().find(|f| f.name == n).unwrap().value.clone();
    assert_eq!(f3("langs"), vec!["English".to_string(), "Spanish".to_string()]);
    assert_eq!(f3("country"), vec!["CA".to_string()]);
    let taken: Vec<&String> = d3.info.warnings.iter().filter(|w| w.contains("another viewer")).collect();
    assert_eq!(taken.len(), 1, "{:?}", d3.info.warnings);
    assert!(taken[0].contains("langs") && !taken[0].contains("country"), "{}", taken[0]);
}

/// A form's data exports, imports and takes pictures through the session, and what it holds
/// comes back the same.
#[test]
fn xfa_data_exports_imports_and_pictures_round_trip_through_the_session() {
    use pdfcraft_xfa::som_to_path;
    let data = "<form><page1><country>JP</country><langs>English\nSpanish</langs></page1></form>";
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::fields_template(data)));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("fields.pdf", None, bytes, None).expect("opens");
    let field = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).cloned().unwrap_or_else(|| panic!("no field {n}"));
    // Export: XML is the datasets document, XDP wraps it; the fields' current values are in it.
    s.apply(id, Edit::SetFieldValue { name: "country".into(), value: FieldValue::Choice(vec!["CA".into()]) }).unwrap();
    let xml = String::from_utf8(s.export_data(id, DataFormat::Xml, false, true).unwrap()).unwrap();
    assert!(xml.starts_with("<?xml") && xml.contains("<form") && xml.contains("<country>CA</country>") && !xml.contains("<xfa:datasets"), "{xml}");
    let xdp = String::from_utf8(s.export_data(id, DataFormat::Xdp, false, true).unwrap()).unwrap();
    assert!(xdp.contains("<xdp:xdp") && xdp.contains("<country>CA</country>") && xdp.contains("href=\"fields.pdf\""), "{xdp}");
    // Import: the data file's values replace the form's; the calculation runs; undo brings
    // the old values back.
    let file = "<form><page1><country>FR</country><langs>French</langs><pin>1234</pin></page1></form>";
    s.apply(id, Edit::ImportData { name: "data.xml".into(), bytes: Arc::new(file.as_bytes().to_vec()) }).unwrap();
    assert_eq!(field(&s, "country").value, vec!["FR".to_string()]);
    assert_eq!(field(&s, "langs").value, vec!["French".to_string()]);
    assert_eq!(field(&s, "pin").value, vec!["1234".to_string()]);
    assert_eq!(field(&s, "summary").value, vec!["Country: FR".to_string()], "{:?}", s.take_js_output(id));
    s.undo(id).unwrap();
    assert_eq!(field(&s, "country").value, vec!["CA".to_string()]);
    s.redo(id).unwrap();
    assert_eq!(field(&s, "country").value, vec!["FR".to_string()]);
    // A file in another encoding, and one with a long comment before its root, import too.
    let latin1: Vec<u8> =
        "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><form><page1><other>Société</other></page1></form>".chars().map(|c| c as u32 as u8).collect();
    s.apply(id, Edit::ImportData { name: "l1.xml".into(), bytes: Arc::new(latin1) }).unwrap();
    assert_eq!(field(&s, "other").value, vec!["Société".to_string()]);
    let commented = format!("<!-- {} --><form><page1><other>after</other></page1></form>", "x".repeat(5000));
    s.apply(id, Edit::ImportData { name: "c.xml".into(), bytes: Arc::new(commented.into_bytes()) }).unwrap();
    assert_eq!(field(&s, "other").value, vec!["after".to_string()]);
    let utf16: Vec<u8> = "<form><page1><other>wide</other></page1></form>".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    s.apply(id, Edit::ImportData { name: "w.xml".into(), bytes: Arc::new([&[0xFF, 0xFE][..], &utf16].concat()) }).unwrap();
    assert_eq!(field(&s, "other").value, vec!["wide".to_string()]);
    let shift_jis = b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><form/>".to_vec();
    assert!(s.apply(id, Edit::ImportData { name: "sj.xml".into(), bytes: Arc::new(shift_jis) }).is_err());
    // A data file for another form is refused.
    assert!(s.apply(id, Edit::ImportData { name: "o.xml".into(), bytes: Arc::new(b"<other><x>1</x></other>".to_vec()) }).is_err());
    assert_eq!(field(&s, "other").value, vec!["wide".to_string()]);
    // XFDF still goes the usual way on an XFA form.
    let xfdf =
        "<?xml version=\"1.0\"?><xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><fields><field name=\"pin\"><value>9999</value></field></fields></xfdf>";
    s.apply(id, Edit::ImportData { name: "d.xfdf".into(), bytes: Arc::new(xfdf.as_bytes().to_vec()) }).unwrap();
    assert_eq!(field(&s, "pin").value, vec!["9999".to_string()]);
    assert_eq!(field(&s, "country").value, vec!["FR".to_string()]);
    // A picture chosen for the image field goes into the data and onto the page.
    let png = pdfcraft_xfa::fixtures::tiny_png();
    assert_eq!(field(&s, "photo").button, Some(pdfcraft_forms::af::ButtonAction::ImportIcon), "clicking it picks a picture");
    s.apply(id, Edit::SetFieldImage { name: "photo".into(), image: Arc::new(pdfcraft_xfa::fixtures::TINY_GIF.to_vec()) }).unwrap();
    let cos = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    let d = pdfcraft_xfa::data_of(&cos).unwrap();
    assert!(d.text_at(&som_to_path("form[0].page1[0].photo[0]")).unwrap().starts_with("R0lGODlh"));
    assert!(pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp.contains("xfa:contentType=\"image/gif\""));
    assert_eq!(s.get(id).unwrap().page_images(0).len(), 2, "the field's picture (now the GIF) and the logo");
    // Only pictures the form can show; only image fields.
    for (data, declared) in [(pdfcraft_xfa::fixtures::tiny_bmp(false), "image/bmp"), (pdfcraft_xfa::fixtures::tiny_tiff(false), "image/tiff")] {
        s.apply(id, Edit::SetFieldImage { name: "photo".into(), image: Arc::new(data) }).unwrap();
        let cos = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
        assert!(pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp.contains(&format!("xfa:contentType=\"{declared}\"")));
    }
    assert!(s.apply(id, Edit::SetFieldImage { name: "photo".into(), image: Arc::new(b"BM\0\0\0\0".to_vec()) }).is_err());
    assert!(s.apply(id, Edit::SetFieldImage { name: "country".into(), image: Arc::new(pdfcraft_xfa::fixtures::TINY_GIF.to_vec()) }).is_err());
    // One datasets stream per edit, however many steps it took.
    let before = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap().object_numbers().len();
    s.apply(id, Edit::SetFieldImage { name: "photo".into(), image: Arc::new(pdfcraft_xfa::fixtures::TINY_GIF.to_vec()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "pin".into(), value: FieldValue::Text("1".into()) }).unwrap();
    let after = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    let datasets = after
        .object_numbers()
        .into_iter()
        .filter(|n| matches!(&*after.get(pdfcraft_cos::ObjRef { num: *n, generation: 0 }), pdfcraft_cos::Object::Stream(st) if st.decoded().is_ok_and(|b| b.starts_with(b"<xfa:datasets"))))
        .count();
    assert!(datasets <= 3, "{datasets} datasets streams (one per edit at most), {before} objects before");
    s.apply(id, Edit::SetFieldImage { name: "photo".into(), image: Arc::new(png) }).unwrap();
    let cos = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    assert!(pdfcraft_xfa::data_of(&cos).unwrap().text_at(&som_to_path("form[0].page1[0].photo[0]")).unwrap().starts_with("iVBOR"));
    // Reopened: the picture and the imported values are what they were.
    let id2 = s.open("again.pdf", None, s.save_bytes(id).unwrap(), None).unwrap();
    let d2 = s.get(id2).unwrap();
    assert_eq!(d2.form.iter().find(|f| f.name == "country").unwrap().value, vec!["FR".to_string()]);
    assert_eq!(d2.page_images(0).len(), 2);
}

/// Importing data into a static form replaces every value, emptying the fields the file
/// doesn't name, and pictures chosen for it reach its data.
#[test]
fn xfa_data_import_on_a_static_form_replaces_every_value_and_pictures_reach_its_data() {
    let original = Arc::new(pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name><agree>1</agree></page1></form1>"));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("static.pdf", None, original, None).expect("opens");
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name.ends_with(n)).map(|f| f.value.clone()).unwrap_or_default();
    assert_eq!(value(&s, "name[0]"), vec!["Ada".to_string()]);
    assert_eq!(value(&s, "agree[0]"), vec!["1".to_string()]);
    // The file names only `name`: `agree` is emptied too, since the data was replaced.
    s.apply(id, Edit::ImportData { name: "d.xml".into(), bytes: Arc::new(b"<form1><page1><name>Bob</name></page1></form1>".to_vec()) }).unwrap();
    assert_eq!(value(&s, "name[0]"), vec!["Bob".to_string()]);
    assert!(value(&s, "agree[0]").is_empty(), "{:?}", value(&s, "agree[0]"));
    let cos = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    let d = pdfcraft_xfa::data_of(&cos).unwrap();
    assert_eq!(d.text_at(&pdfcraft_xfa::som_to_path("form1[0].page1[0].name[0]")), Some("Bob"));
    assert!(d.text_at(&pdfcraft_xfa::som_to_path("form1[0].page1[0].agree[0]")).is_none_or(str::is_empty));
    let xml = String::from_utf8(s.export_data(id, DataFormat::Xml, false, true).unwrap()).unwrap();
    assert!(xml.contains("<name>Bob</name>") && !xml.contains("<agree>1</agree>"), "{xml}");
}

/// Importing data into a dynamic form adds and removes the rows of its repeating subform to
/// match, and the calculations over them run again.
#[test]
fn xfa_data_import_adds_and_removes_rows_of_a_dynamic_form() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template()));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("rows.pdf", None, bytes, None).expect("opens");
    let names = |s: &Session| s.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).map(|f| f.value.clone()).unwrap_or_default();
    assert_eq!(names(&s).iter().filter(|n| n.starts_with("amount")).count(), 1);
    let file = "<form><page1><qty>3</qty><table><row><item>a</item><amount>10</amount></row><row><item>b</item><amount>32</amount></row><row><item>c</item><amount>0.5</amount></row></table></page1></form>";
    s.apply(id, Edit::ImportData { name: "rows.xml".into(), bytes: Arc::new(file.as_bytes().to_vec()) }).unwrap();
    assert_eq!(names(&s).iter().filter(|n| n.starts_with("amount")).count(), 3, "{:?}", names(&s));
    assert_eq!(value(&s, "total"), vec!["15".to_string()], "qty 3 × price 5, calculated after the import: {:?}", s.take_js_output(id));
    assert_eq!(value(&s, "grand"), vec!["42.5".to_string()]);
    // More rows than the layout shows: the document says so.
    let rows: String = (0..60).map(|i| format!("<row><item>r{i}</item><amount>1</amount></row>")).collect();
    let file = format!("<form><page1><qty>1</qty><table>{rows}</table></page1></form>");
    s.apply(id, Edit::ImportData { name: "many.xml".into(), bytes: Arc::new(file.into_bytes()) }).unwrap();
    let out = s.take_js_output(id);
    assert!(out.errors.iter().chain(out.console.iter()).any(|l| l.contains("60 rows")), "{out:?}");
    // A file with one row takes the others away.
    let file = "<form><page1><qty>1</qty><table><row><item>only</item><amount>7</amount></row></table></page1></form>";
    s.apply(id, Edit::ImportData { name: "one.xml".into(), bytes: Arc::new(file.as_bytes().to_vec()) }).unwrap();
    assert_eq!(names(&s).iter().filter(|n| n.starts_with("amount")).count(), 1, "{:?}", names(&s));
    assert_eq!(value(&s, "grand"), vec!["7".to_string()]);
    // Not an XFA data file on an XFA form: refused with the usual message, nothing changed.
    assert!(s.apply(id, Edit::ImportData { name: "junk.xml".into(), bytes: Arc::new(b"<a><b></a>".to_vec()) }).is_err());
    assert_eq!(value(&s, "grand"), vec!["7".to_string()]);
    // The automation tool's view: export then import through a file round-trips the rows.
    let out = s.export_data(id, DataFormat::Xdp, false, true).unwrap();
    assert!(String::from_utf8_lossy(&out).contains("<item>only</item>"));
}

/// Hundreds of calculations, in JavaScript and FormCalc, run on opening in one engine within
/// the opening's budget, and a change recalculates them within the event's.
#[test]
fn hundreds_of_xfa_calculations_run_in_one_engine_within_the_open_budget() {
    let (sections, rows) = (10, 20);
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::many_calculations_template(sections, rows)));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let started = std::time::Instant::now();
    let id = s.open("many.pdf", None, bytes, None).expect("opens");
    let elapsed = started.elapsed();
    let out = s.take_js_output(id);
    assert!(!out.errors.iter().any(|e| e.contains("were skipped") || e.contains("stopped")), "{:?}", out.errors);
    let form = &s.get(id).unwrap().form;
    let totals: Vec<&pdfcraft_forms::Field> = form.iter().filter(|f| f.name.starts_with("total")).collect();
    assert_eq!(totals.len(), sections * rows);
    assert!(totals.iter().all(|f| !f.value.is_empty()), "every row's total was calculated");
    let sums: Vec<&pdfcraft_forms::Field> = form.iter().filter(|f| f.name.starts_with("sum")).collect();
    assert_eq!(sums.len(), sections);
    // 2 × (1 + 2 + … + rows)
    let expect = (rows * (rows + 1)).to_string();
    assert!(sums.iter().all(|f| f.value == vec![expect.clone()]), "{:?}", sums.iter().map(|f| f.value.clone()).collect::<Vec<_>>());
    eprintln!("{} calculate scripts on open in {elapsed:?}", sections * rows + sections);
    // A change recalculates them all again, well within the event budget.
    let started = std::time::Instant::now();
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("100".into()) }).unwrap();
    let out = s.take_js_output(id);
    assert!(!out.errors.iter().any(|e| e.contains("were skipped")), "{:?}", out.errors);
    assert_eq!(s.get(id).unwrap().form.iter().find(|f| f.name == "total").unwrap().value, vec!["200".to_string()]);
    assert_eq!(s.get(id).unwrap().form.iter().find(|f| f.name == "sum").unwrap().value, vec![(rows * (rows + 1) - 2 + 200).to_string()]);
    eprintln!("recalculated after a change in {:?}", started.elapsed());
}

/// The scripts of one event share the form as the engine holds it: a failed script's
/// half-done writes reach nobody, and a script reads the saved value a choice list was given
/// by its shown text.
#[test]
fn xfa_scripts_of_one_event_see_the_form_as_the_engine_holds_it() {
    // In document order: `a` holds 1; `b`'s calculate writes `a` then fails; `c`'s calculate
    // reads `a`. A failed script keeps what it changed before the error, and the next script
    // sees it. And `d` sets a choice list by its shown text: the next script reads the saved
    // value the engine kept.
    let tpl = pdfcraft_xfa::fixtures::fields_template("<form><page1><country>JP</country></page1></form>").replace(
        r#"<field name="summary" w="3in" h="0.3in" access="readOnly"><ui><textEdit/></ui>"#,
        r#"<field name="a" w="1in" h="0.3in"><ui><numericEdit/></ui><value><integer>1</integer></value></field>
  <field name="b" w="1in" h="0.3in"><ui><numericEdit/></ui><calculate><script contentType="application/x-javascript">a.rawValue = 99; throw new Error("boom");</script></calculate></field>
  <field name="c" w="1in" h="0.3in"><ui><numericEdit/></ui><calculate><script contentType="application/x-javascript">a.rawValue + 1</script></calculate></field>
  <field name="d" w="1in" h="0.3in"><ui><textEdit/></ui><calculate><script contentType="application/x-javascript">country.rawValue = "Canada"; "set"</script></calculate></field>
  <field name="e" w="1in" h="0.3in"><ui><textEdit/></ui><calculate><script>country</script></calculate></field>
  <field name="summary" w="3in" h="0.3in" access="readOnly"><ui><textEdit/></ui>"#,
    );
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("iso.pdf", None, bytes, None).expect("opens");
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).map(|f| f.value.clone()).unwrap_or_default();
    let out = s.take_js_output(id);
    assert!(out.errors.iter().any(|e| e.contains("boom")), "{out:?}");
    assert_eq!(value(&s, "a"), vec!["99".to_string()], "the failed script's write before the error stands");
    assert_eq!(value(&s, "c"), vec!["100".to_string()]);
    assert_eq!(value(&s, "country"), vec!["CA".to_string()], "shown text mapped to the saved value");
    assert_eq!(value(&s, "e"), vec!["CA".to_string()], "the next script reads the saved value");
    assert_eq!(value(&s, "summary"), vec!["Country: CA".to_string()]);
}

/// A row of dates worked out the way leave forms do it: the calculation checks that the raw value
/// is a whole ISO date, then adds days to the date as `formattedValue` shows it (`YYYYMMDD`).
#[test]
fn xfa_date_arithmetic_reads_the_date_as_its_display_picture_shows_it() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::date_row_template()));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("row.pdf", None, bytes, None).expect("opens");
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).map(|f| f.value.clone()).unwrap_or_default();
    s.apply(id, Edit::SetFieldValue { name: "from".into(), value: FieldValue::Text("20261020".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "days".into(), value: FieldValue::Text("3".into()) }).unwrap();
    let out = s.take_js_output(id);
    assert_eq!(value(&s, "to"), ["20261022"], "{:?}", out.errors);
}

#[test]
fn filling_growable_xfa_text_reflows_and_undo_restores_the_layout() {
    let text: String = (0..150).map(|i| format!("Row{i:03} {}sample\n", "sample ".repeat(8))).collect();
    assert_eq!(text.len(), 10_500);
    for scripted in [false, true] {
        let template = pdfcraft_xfa::fixtures::growable_text_template(scripted);
        let mut s = Session::new();
        let id = s.open("growing.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        assert_eq!(s.get(id).unwrap().info.pages.len(), 2);
        s.apply(id, Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text(text.clone()) }).unwrap();
        assert_eq!(s.get(id).unwrap().info.pages.len(), 3, "scripted={scripted}");
        let data = pdfcraft_xfa::data_of(&s.get(id).unwrap().editor.as_ref().unwrap().cos).unwrap();
        assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form[0].Page3[0].ACTIVIDADES[0]")), Some(text.as_str()));
        s.undo(id).unwrap();
        assert_eq!(s.get(id).unwrap().info.pages.len(), 2);
        assert_eq!(s.get(id).unwrap().form.iter().find(|f| f.name == "ACTIVIDADES").unwrap().value, vec!["Short"]);
        s.redo(id).unwrap();
        assert_eq!(s.get(id).unwrap().info.pages.len(), 3);
    }
}

#[test]
fn xfa_text_growth_refreshes_widget_geometry_but_fixed_height_fields_keep_their_widgets() {
    for fixed in [false, true] {
        let size = if fixed { r#"h="30pt""# } else { r#"minH="20pt""# };
        let template = pdfcraft_xfa::fixtures::growable_text_template(false).replace(r#"minH="207.017mm""#, size);
        let mut s = Session::new();
        let id = s.open("geometry.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        let field = |s: &Session| s.get(id).unwrap().form.iter().find(|f| f.name == "ACTIVIDADES").unwrap().clone();
        let before = field(&s);
        s.apply(id, Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text("Line\n".repeat(10)) }).unwrap();
        let after = field(&s);
        assert_eq!(s.get(id).unwrap().info.pages.len(), 2);
        if fixed {
            assert_eq!(before.obj, after.obj, "fixed-height fields must not trigger layout");
            assert_eq!(before.widgets[0].rect, after.widgets[0].rect);
        } else {
            assert_ne!(before.obj, after.obj);
            assert!(after.widgets[0].rect[3] - after.widgets[0].rect[1] > before.widgets[0].rect[3] - before.widgets[0].rect[1]);
            assert_eq!(
                s.get(id).unwrap().info.fields.iter().find(|f| f.name == "ACTIVIDADES").unwrap().value.as_deref(),
                Some("Line\n".repeat(10).trim_end())
            );
        }
        s.undo(id).unwrap();
        assert_eq!(field(&s).widgets[0].rect, before.widgets[0].rect);
        s.redo(id).unwrap();
        assert_eq!(field(&s).widgets[0].rect, after.widgets[0].rect);
    }
}

#[test]
fn xfa_text_that_cannot_sync_to_structured_data_is_not_replaced_by_a_relayout() {
    let template = pdfcraft_xfa::fixtures::growable_text_template(false)
        .replace("<ACTIVIDADES>Short</ACTIVIDADES>", "<ACTIVIDADES><nested>Short</nested></ACTIVIDADES>");
    let mut s = Session::new();
    let id = s.open("structured.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
    let text = "Keep this accepted value\n".repeat(150);
    s.apply(id, Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text(text.clone()) }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), 2);
    assert_eq!(doc.form.iter().find(|f| f.name == "ACTIVIDADES").unwrap().value, vec![text]);
    assert!(doc.xfa_warnings.iter().any(|w| w.contains("structured content")));
}

#[test]
fn xfa_structured_data_whitespace_edits_survive_without_stale_relayout() {
    let template = pdfcraft_xfa::fixtures::growable_text_template(false)
        .replace("<ACTIVIDADES>Short</ACTIVIDADES>", "<ACTIVIDADES><nested>Short</nested></ACTIVIDADES>");
    for text in ["Short\n", "\nShort", " Short ", "\tShort\t"] {
        let mut s = Session::new();
        let id = s.open("structured.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        let field = |s: &Session| s.get(id).unwrap().form.iter().find(|f| f.name == "ACTIVIDADES").unwrap().clone();
        let before = field(&s);
        s.apply(id, Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text(text.into()) }).unwrap();
        assert_eq!(field(&s).value, vec![text], "the accepted whitespace must not be replaced from stale datasets");
        assert_eq!(field(&s).obj, before.obj, "a skipped data write must not rebuild the widgets");
        let doc = s.get(id).unwrap();
        assert!(doc.dirty);
        assert!(doc.xfa_warnings.iter().any(|w| w.contains("structured content")), "{:?}", doc.xfa_warnings);
        let cos = pdfcraft_cos::Document::open(doc.bytes.clone()).unwrap();
        let xdp = pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp;
        assert!(xdp.contains("<ACTIVIDADES><nested>Short</nested></ACTIVIDADES>"));
        s.undo(id).unwrap();
        assert_eq!(field(&s).value, before.value);
        s.redo(id).unwrap();
        assert_eq!(field(&s).value, vec![text]);
    }
}

/// A script that fails partway keeps what it did before the error, as JavaScript does: a form
/// whose check box opens the next boxes and then trips over an object the form no longer has
/// still opens them (DA Form 31's leave types).
#[test]
fn an_xfa_script_that_fails_partway_keeps_what_it_changed_before_the_error() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::partly_failing_script_template()));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("form.pdf", None, bytes, None).expect("opens");
    let field = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).cloned();
    assert!(field(&s, "leave").is_some_and(|f| f.flags & 1 != 0), "protected until `kind` is checked");
    assert!(field(&s, "details").is_none(), "invisible: not laid out");
    s.take_js_output(id);
    s.apply(id, Edit::SetFieldValue { name: "kind".into(), value: FieldValue::Check(true) }).unwrap();
    let out = s.take_js_output(id);
    assert!(out.errors.iter().any(|e| e.contains("kind") && e.contains("change")), "the error is still reported: {out:?}");
    assert!(field(&s, "leave").is_some_and(|f| f.flags & 1 == 0), "opened before the error");
    assert!(field(&s, "details").is_some(), "shown before the error");
    s.apply(id, Edit::SetFieldValue { name: "leave".into(), value: FieldValue::Check(true) }).expect("can be checked now");
}

/// A scripted form end to end: initialize and calculate on open, recalculation after a change,
/// a failing validation's message, buttons that add and remove rows and show messages, a check
/// box that shows a subform (undoable), FormCalc, and a save and reopen that keep it all.
#[test]
fn repeated_xfa_print_buttons_run_their_click_script() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::repeated_print_button_template()));
    let mut s = Session::new();
    let id = s.open("print.pdf", None, bytes, None).unwrap();
    assert_eq!(s.get(id).unwrap().info.pages.len(), 3);
    for name in ["PrintButton1", "PrintButton1_2", "PrintButton1_3"] {
        let out = s.run_javascript(id, "throw new Error('AcroForm fallback ran')", Some(name)).unwrap();
        assert_eq!(out.error, None, "{name}: {out:?}");
        assert_eq!(out.requests, vec![pdfcraft_js::Request::Print], "{name}: {out:?}");
    }
}

#[test]
fn xfa_scripts_initialize_calculate_validate_toggle_and_add_rows() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template()));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("scripted.pdf", None, bytes, None).expect("opens");
    let add_row = s.get(id).unwrap().form.iter().find(|f| f.name == "addRow").expect("the XFA button is in the form snapshot");
    assert!(matches!(add_row.button.as_ref(), Some(pdfcraft_forms::af::ButtonAction::Script(_))));
    let value = |s: &Session, n: &str| {
        s.get(id).unwrap().form.iter().find(|f| f.name == n).map(|f| f.value.clone()).unwrap_or_else(|| panic!("no field {n}"))
    };
    let names = |s: &Session| s.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    // On open: the initialize script set qty, the calculations ran, nothing is dirty.
    assert_eq!(value(&s, "qty"), vec!["2".to_string()]);
    assert_eq!(value(&s, "total"), vec!["10".to_string()], "{:?}", s.take_js_output(id));
    assert_eq!(value(&s, "grand"), vec!["0".to_string()]);
    assert!(!s.get(id).unwrap().xfa_scripts_off());
    assert!(!s.get(id).unwrap().dirty);
    assert!(!names(&s).iter().any(|n| n == "note"), "details is hidden");
    // A change recalculates.
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("3".into()) }).unwrap();
    assert_eq!(value(&s, "total"), vec!["15".to_string()], "{:?}", s.take_js_output(id));
    // A failing validate script shows its message; the value stays, as in Acrobat.
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("500".into()) }).unwrap();
    let out = s.take_js_output(id);
    assert!(out.alerts.iter().any(|a| a == "Quantity must be 100 or less"), "{out:?}");
    assert_eq!(value(&s, "total"), vec!["2500".to_string()]);
    // A button adds a row: new fields, same page, one undo step.
    let before = names(&s).len();
    let o = s.run_javascript(id, "", Some("addRow")).unwrap();
    assert_eq!(o.error, None);
    let after = names(&s);
    assert_eq!(after.len(), before + 2, "{after:?}");
    assert!(after.iter().any(|n| n == "amount_2"));
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Run form script"));
    s.apply(id, Edit::SetFieldValue { name: "amount".into(), value: FieldValue::Text("10".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "amount_2".into(), value: FieldValue::Text("32".into()) }).unwrap();
    assert_eq!(value(&s, "grand"), vec!["42".to_string()], "{:?}", s.take_js_output(id));
    // Message boxes reach the viewer.
    let o = s.run_javascript(id, "", Some("hello")).unwrap();
    assert_eq!(o.alerts, vec!["Hello 500".to_string()]);
    // The check box shows the hidden subform; undo hides it again.
    s.apply(id, Edit::SetFieldValue { name: "more".into(), value: FieldValue::Check(true) }).unwrap();
    assert!(names(&s).iter().any(|n| n == "note"), "{:?} {:?}", names(&s), s.take_js_output(id));
    s.undo(id).unwrap();
    assert!(!names(&s).iter().any(|n| n == "note"));
    s.redo(id).unwrap();
    assert!(names(&s).iter().any(|n| n == "note"));
    // Removing the row takes its amount out of the sum.
    s.run_javascript(id, "", Some("removeRow")).unwrap();
    assert!(!names(&s).iter().any(|n| n == "amount_2"));
    assert_eq!(value(&s, "grand"), vec!["10".to_string()]);
    // FormCalc scripts run too.
    let o = s.run_javascript(id, "", Some("legacy")).unwrap();
    assert_eq!(o.error, None);
    assert_eq!(o.alerts, vec!["FormCalc".to_string()]);
    // Saved and reopened: the visible subform, the values and the rows are what they were.
    let saved = s.save_bytes(id).unwrap();
    let id2 = s.open("again.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    let n2: Vec<&str> = d2.form.iter().map(|f| f.name.as_str()).collect();
    assert!(n2.contains(&"note") && !n2.contains(&"amount_2"), "{n2:?}");
    assert_eq!(d2.form.iter().find(|f| f.name == "grand").unwrap().value, vec!["10".to_string()]);
    assert_eq!(d2.form.iter().find(|f| f.name == "qty").unwrap().value, vec!["500".to_string()]);
    assert!(!d2.dirty);
}

/// A repeating table inside an area of a positioned page (as in real Designer forms): every
/// row is laid out, the values Adobe wrote (under the area's parent: areas are
/// not data scopes) are read, a FormCalc total over all rows computes, and what is filled here
/// is written back where Adobe's viewers read it, with no element for the area.
#[test]
fn xfa_rows_in_areas_are_laid_out_read_calculated_and_written_where_adobe_binds_them() {
    let data = "<form><page><table><row><reason>Regular</reason><days>4</days></row><row><reason/><days/></row><row><reason/><days/></row></table><carried>5</carried></page></form>";
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::leave_template(data)));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("leave.pdf", None, bytes, None).expect("opens");
    let value = |s: &Session, n: &str| {
        s.get(id).unwrap().form.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("no field {n}")).value.first().cloned().unwrap_or_default()
    };
    let names = |s: &Session| s.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    // Three rows (occur min="3"), not one.
    assert_eq!(names(&s).iter().filter(|n| n.starts_with("reason")).count(), 3, "{:?}", names(&s));
    // Adobe's data, bound past the area.
    assert_eq!((value(&s, "reason"), value(&s, "days"), value(&s, "carried")), ("Regular".into(), "4".into(), "5".into()));
    // The total walks rows 0 to 2: 5 + 30 - 4.
    assert_eq!(value(&s, "rest"), "31", "{:?}", s.take_js_output(id));
    // A second regular row recalculates it.
    s.apply(id, Edit::SetFieldValue { name: "reason_2".into(), value: FieldValue::Text("Regular".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "days_2".into(), value: FieldValue::Text("2".into()) }).unwrap();
    assert_eq!(value(&s, "rest"), "29", "{:?}", s.take_js_output(id));
    assert!(s.take_js_output(id).errors.is_empty());
    // Written where Adobe reads it: no element for the area, the rows under the table.
    let cos = pdfcraft_cos::Document::open(s.get(id).unwrap().bytes.clone()).unwrap();
    let d = pdfcraft_xfa::data_of(&cos).unwrap();
    let path = |som: &str| pdfcraft_xfa::som_to_path(som);
    assert_eq!(d.count(&path("form[0].page[0]"), "box"), 0, "an element for the area was written");
    assert_eq!(d.count(&path("form[0].page[0].table[0]"), "row"), 3);
    assert_eq!(d.text_at(&path("form[0].page[0].table[0].row[1].days[0]")), Some("2"));
    assert_eq!(d.text_at(&path("form[0].page[0].rest[0]")), Some("29"));
    // Reopened, the values come back from that data.
    let mut s2 = Session::new().with_clock(|| 1_700_000_000);
    let id2 = s2.open("again.pdf", None, s.get(id).unwrap().bytes.clone(), None).expect("reopens");
    let v2 = |n: &str| s2.get(id2).unwrap().form.iter().find(|f| f.name == n).unwrap().value.first().cloned().unwrap_or_default();
    assert_eq!((v2("days_2"), v2("rest")), ("2".into(), "29".into()));
}

#[test]
fn a_value_a_script_sets_reaches_every_field_bound_to_the_same_data_node() {
    // `carried` inside the area and a second `carried` outside it bind to one data node (areas
    // are not data scopes). In document order: `setter` writes the inner one, `echo` reads the
    // outer one in the same event, and the saved data holds the one value.
    let tpl = pdfcraft_xfa::fixtures::leave_template("<form><page><carried>5</carried></page></form>")
        .replace(
            r#"    <field name="rest" y="2.5in" w="2in" h="0.3in" access="readOnly">"#,
            r#"    <field name="setter" y="3in" w="2in" h="0.3in"><ui><numericEdit/></ui><calculate><script>carried = 9
9</script></calculate></field>
    <field name="rest" y="2.5in" w="2in" h="0.3in" access="readOnly">"#,
        )
        .replace(
            "  </subform>\n </subform>\n</subform>",
            r#"  </subform>
  <field name="carried" y="8.2in" w="2in" h="0.3in"><ui><numericEdit/></ui></field>
  <field name="echo" y="8.5in" w="2in" h="0.3in"><ui><numericEdit/></ui><calculate><script>carried + 0</script></calculate></field>
 </subform>
</subform>"#,
        );
    assert!(tpl.contains("name=\"echo\"") && tpl.contains("name=\"setter\""), "the fixture was reshaped");
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("shared.pdf", None, bytes, None).expect("opens");
    let out = s.take_js_output(id);
    assert!(out.errors.is_empty(), "{out:?}");
    let values = |s: &Session, id: DocId, n: &str| {
        s.get(id).unwrap().form.iter().filter(|f| f.name.starts_with(n)).map(|f| f.value.join("")).collect::<Vec<_>>()
    };
    assert_eq!(values(&s, id, "carried"), vec!["9".to_string(), "9".to_string()], "both fields show the node's value");
    assert_eq!(values(&s, id, "echo"), vec!["9".to_string()], "read in the same event the setter ran in");
    let cos = pdfcraft_cos::Document::open(s.get(id).unwrap().bytes.clone()).unwrap();
    let d = pdfcraft_xfa::data_of(&cos).unwrap();
    assert_eq!(d.text_at(&pdfcraft_xfa::som_to_path("form[0].page[0].carried[0]")), Some("9"));
    assert_eq!(d.count(&pdfcraft_xfa::som_to_path("form[0].page[0]"), "carried"), 1, "one data node");
    // The inner field hidden (no widget): the outer widget still takes the node's value, and
    // an unrelated edit does not write the old value back over it.
    let hidden = Arc::new(pdfcraft_xfa::fixtures::shell(
        &tpl.replace(r#"<field name="carried" y="1.5in""#, r#"<field name="carried" presence="hidden" y="1.5in""#),
    ));
    let id = s.open("hidden.pdf", None, hidden, None).expect("opens");
    assert_eq!(values(&s, id, "carried"), vec!["9".to_string()]);
    assert_eq!(values(&s, id, "echo"), vec!["9".to_string()]);
    s.apply(id, Edit::SetFieldValue { name: "allowance".into(), value: FieldValue::Text("31".into()) }).unwrap();
    assert_eq!(values(&s, id, "carried"), vec!["9".to_string()]);
    let cos = pdfcraft_cos::Document::open(s.get(id).unwrap().bytes.clone()).unwrap();
    assert_eq!(pdfcraft_xfa::data_of(&cos).unwrap().text_at(&pdfcraft_xfa::som_to_path("form[0].page[0].carried[0]")), Some("9"));
    // A check box bound to the node a text field sets: checked by its own on state.
    let checkbox = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl.replace(
        r#"<field name="carried" y="1.5in" w="2in" h="0.3in"><ui><numericEdit/></ui></field>"#,
        r#"<field name="carried" y="1.5in" w="2in" h="0.3in"><ui><checkButton/></ui><items><integer>9</integer><integer>0</integer></items></field>"#,
    )));
    let id = s.open("check.pdf", None, checkbox, None).expect("opens");
    let carried: Vec<(pdfcraft_forms::FieldKind, Vec<String>)> =
        s.get(id).unwrap().form.iter().filter(|f| f.name.starts_with("carried")).map(|f| (f.kind, f.value.clone())).collect();
    assert_eq!(
        carried,
        vec![(pdfcraft_forms::FieldKind::CheckBox, vec!["9".to_string()]), (pdfcraft_forms::FieldKind::Text, vec!["9".to_string()])],
        "{carried:?}"
    );
}

#[test]
fn flattening_an_xfa_form_bakes_its_pages_and_takes_the_xfa_out() {
    let data = "<form><page1><country>JP</country><langs>English\nSpanish</langs></page1></form>";
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::fields_template(data)));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("fields.pdf", None, bytes, None).expect("opens");
    assert_eq!(s.get(id).unwrap().info.xfa, Some(pdfcraft_render::Xfa::Dynamic));
    s.apply(id, Edit::SetFieldValue { name: "other".into(), value: FieldValue::Text("typed here".into()) }).unwrap();
    s.apply(id, Edit::Flatten { comments: false, fields: true }).unwrap();
    let d = s.get(id).unwrap();
    assert!(d.form.is_empty(), "no fields remain");
    assert_eq!(d.info.xfa, None, "no longer an XFA form: Adobe's viewers show the baked pages");
    assert!(d.xfa.is_none());
    let texts = page_texts(&s, id).join("\n");
    for want in ["Japan", "typed here", "Country: JP", "12345"] {
        assert!(texts.contains(want), "{want} is page content now: {texts}");
    }
    // Saved and reopened: a plain PDF, the same page content, nothing of the XFA left.
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    assert!(pdfcraft_xfa::read_packets(&cos).unwrap().is_none());
    let id2 = s.open("flat.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    assert_eq!((d2.info.xfa, d2.form.len()), (None, 0));
    assert!(page_texts(&s, id2).join("\n").contains("typed here"));
    // Undone, the form is a laid-out XFA form again.
    s.undo(id).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.info.xfa, Some(pdfcraft_render::Xfa::Dynamic));
    assert_eq!(d.form.iter().find(|f| f.name == "other").unwrap().value, vec!["typed here".to_string()]);
    // Flattening comments alone leaves the XFA form as it is.
    s.apply(id, Edit::Flatten { comments: true, fields: false }).unwrap();
    assert_eq!(s.get(id).unwrap().info.xfa, Some(pdfcraft_render::Xfa::Dynamic));
}

#[test]
fn a_dynamic_xfa_form_that_was_never_laid_out_keeps_its_xfa_and_the_user_is_told_what_went() {
    // A template that can't be read: the form keeps its placeholder page and its XFA.
    let broken = "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\"><subform name=\"form\"</template></xdp:xdp>";
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("broken.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(broken)), None).expect("opens");
    assert_eq!(s.get(id).unwrap().info.xfa, Some(pdfcraft_render::Xfa::Dynamic));
    assert!(s.get(id).unwrap().xfa.is_none(), "not laid out");
    assert!(s.apply(id, Edit::XfaToStatic).is_err(), "nothing to keep");
    s.apply(id, Edit::Flatten { comments: false, fields: true }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.info.xfa, Some(pdfcraft_render::Xfa::Dynamic), "the XFA stays: Adobe can still render the form");
    assert!(d.info.warnings.iter().any(|w| w.contains("The XFA was kept")), "{:?}", d.info.warnings);
    // A laid-out form with data no field holds: converting says what went with the XFA, and
    // the note stays on the document, while the XFA's own warnings go; undo brings those back.
    let data = "<form><page1><country>JP</country><nobody>holds this</nobody></page1></form>";
    let id = s.open("fields.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::fields_template(data))), None).unwrap();
    let before: Vec<String> = s.get(id).unwrap().info.warnings.clone();
    assert!(before.iter().any(|w| w.contains("scripts changed it on opening")), "an XFA-only warning: {before:?}");
    s.apply(id, Edit::XfaToStatic).unwrap();
    let w = s.get(id).unwrap().info.warnings.clone();
    assert!(w.iter().any(|n| n.starts_with("XFA removed: The XFA was taken out") && n.contains("template, datasets")), "{w:?}");
    assert!(w.iter().any(|n| n.contains("1 value(s) the form's XFA data held that no field showed")), "{w:?}");
    assert!(w.iter().any(|n| n.contains("1 image field(s) do nothing now")), "{w:?}");
    assert!(!w.iter().any(|n| n.contains("scripts changed it on opening")), "the XFA's own warning is gone: {w:?}");
    let js = s.take_js_output(id);
    assert!(js.errors.is_empty(), "{js:?}");
    s.undo(id).unwrap();
    let w = s.get(id).unwrap().info.warnings.clone();
    assert!(!w.iter().any(|n| n.starts_with("XFA removed")), "{w:?}");
    assert_eq!(s.get(id).unwrap().info.xfa, Some(pdfcraft_render::Xfa::Dynamic));
}

#[test]
fn an_xfa_form_converts_to_a_static_form_that_keeps_its_fields() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template()));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("scripted.pdf", None, bytes, None).expect("opens");
    let value = |s: &Session, id: DocId, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).map(|f| f.value.clone()).unwrap_or_default();
    assert_eq!(value(&s, id, "total"), vec!["10".to_string()], "the scripts ran on open");
    let fields_before = s.get(id).unwrap().form.len();
    s.apply(id, Edit::XfaToStatic).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.info.xfa, None);
    assert_eq!(d.form.len(), fields_before, "the fields stay");
    assert_eq!(value(&s, id, "qty"), vec!["2".to_string()]);
    assert_eq!(value(&s, id, "total"), vec!["10".to_string()]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Convert to a static form"));
    // The form is an ordinary one now: a change no longer runs the XFA calculation.
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("7".into()) }).unwrap();
    assert_eq!(value(&s, id, "total"), vec!["10".to_string()]);
    assert!(s.take_js_output(id).is_empty());
    // Saved: no XFA, no private keys on the fields, the pages as laid out.
    let saved = s.save_bytes(id).unwrap();
    let cos = pdfcraft_cos::Document::open(saved.clone()).unwrap();
    assert!(pdfcraft_xfa::read_packets(&cos).unwrap().is_none());
    assert!(pdfcraft_xfa::fields_by_som(&cos).is_empty(), "no /PCSom left");
    let id2 = s.open("static.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    assert_eq!(d2.info.xfa, None);
    assert_eq!(value(&s, id2, "qty"), vec!["7".to_string()]);
    assert_eq!(d2.info.pages.len(), 1);
    // Not an XFA form: refused.
    assert!(s.apply(id2, Edit::XfaToStatic).is_err());
    // Undone, the XFA form is back and its scripts run again.
    s.undo(id).unwrap();
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().info.xfa, Some(pdfcraft_render::Xfa::Dynamic));
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("3".into()) }).unwrap();
    assert_eq!(value(&s, id, "total"), vec!["15".to_string()]);
}

#[test]
fn dynamic_xfa_forms_run_formcalc_scripts() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::formcalc_template()));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("formcalc.pdf", None, bytes, None).expect("opens");
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).unwrap().value.clone();
    let names = |s: &Session| s.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
    // Initialize set qty; the calculates ran from it, including the English words.
    assert_eq!(value(&s, "qty"), vec!["2".to_string()], "{:?}", s.take_js_output(id));
    assert_eq!(value(&s, "total"), vec!["10".to_string()]);
    assert_eq!(value(&s, "words"), vec!["Ten".to_string()]);
    assert!(s.take_js_output(id).is_empty());
    // Editing recalculates and validates.
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("7".into()) }).unwrap();
    assert_eq!(value(&s, "total"), vec!["35".to_string()]);
    assert_eq!(value(&s, "words"), vec!["Thirty-Five".to_string()]);
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("500".into()) }).unwrap();
    let out = s.take_js_output(id);
    assert!(out.alerts.iter().any(|a| a == "Quantity must be 100 or less"), "{out:?}");
    assert_eq!(value(&s, "qty"), vec!["500".to_string()]);
    // Buttons: a row is added, summed and removed; the message box reaches the viewer.
    let before = names(&s).len();
    let o = s.run_javascript(id, "", Some("addRow")).unwrap();
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(names(&s).len(), before + 2, "{:?}", names(&s));
    s.apply(id, Edit::SetFieldValue { name: "amount".into(), value: FieldValue::Text("10".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "amount_2".into(), value: FieldValue::Text("32".into()) }).unwrap();
    assert_eq!(value(&s, "grand"), vec!["42".to_string()], "{:?}", s.take_js_output(id));
    let o = s.run_javascript(id, "", Some("hello")).unwrap();
    assert_eq!(o.alerts, vec!["Hello 500".to_string()]);
    s.apply(id, Edit::SetFieldValue { name: "more".into(), value: FieldValue::Check(true) }).unwrap();
    assert!(names(&s).iter().any(|n| n == "note"), "{:?} {:?}", names(&s), s.take_js_output(id));
    s.run_javascript(id, "", Some("removeRow")).unwrap();
    assert!(!names(&s).iter().any(|n| n == "amount_2"));
    assert_eq!(value(&s, "grand"), vec!["10".to_string()]);
    // Saved and reopened, the form is what it was.
    let saved = s.save_bytes(id).unwrap();
    let id2 = s.open("again.pdf", None, saved, None).unwrap();
    let d2 = s.get(id2).unwrap();
    assert_eq!(d2.form.iter().find(|f| f.name == "words").unwrap().value, vec!["Two Thousand Five Hundred".to_string()]);
    assert!(d2.form.iter().any(|f| f.name == "note"));
}

#[test]
fn hostile_formcalc_scripts_cannot_hang_or_bloat_the_document() {
    let tpl = pdfcraft_xfa::fixtures::formcalc_template()
        .replace(
            "<calculate><script>qty * price</script></calculate>",
            "<calculate><script>table._row.addInstance(1)
table._row.count</script></calculate>",
        )
        .replace("<script>table._row.addInstance(1)</script>", "<script>while (1) do table._row.addInstance(1) endwhile</script>")
        .replace(
            "<script>WordNum(total)</script>",
            "<script>func f(n) do f(n + 1) endfunc
f(1)</script>",
        );
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let started = std::time::Instant::now();
    let id = s.open("hostile.pdf", None, bytes, None).expect("opens");
    let rows = |s: &Session| s.get(id).unwrap().form.iter().filter(|f| f.name.starts_with("amount")).count();
    assert!(rows(&s) <= 50, "{} rows", rows(&s));
    let out = s.take_js_output(id);
    assert!(out.errors.iter().any(|l| l.contains("nested")), "{out:?}");
    let o = s.run_javascript(id, "", Some("addRow")).unwrap();
    assert!(o.error.is_some() || rows(&s) <= 50, "{o:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
}

#[test]
fn a_formcalc_calculate_result_is_cut_before_it_is_saved() {
    let tpl = pdfcraft_xfa::fixtures::formcalc_template()
        .replace("<script>WordNum(total)</script>", "<script>Replace(Space(100000), \" \", \"x\")</script>");
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("long.pdf", None, bytes, None).expect("opens");
    let words = |s: &Session, id| s.get(id).unwrap().form.iter().find(|f| f.name == "words").unwrap().value.concat();
    assert_eq!(words(&s, id).chars().count(), 65_536);
    let out = s.take_js_output(id);
    assert!(out.errors.iter().any(|e| e.contains("cut")), "{out:?}");
    let saved = s.save_bytes(id).unwrap();
    let id2 = s.open("again.pdf", None, saved, None).unwrap();
    assert!(words(&s, id2).chars().count() <= 65_536);
}

#[test]
fn xfa_scripts_stay_off_with_javascript_off() {
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template()));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    s.set_javascript(false);
    let id = s.open("scripted.pdf", None, bytes, None).expect("opens");
    let qty = s.get(id).unwrap().form.iter().find(|f| f.name == "qty").unwrap().value.clone();
    assert!(qty.is_empty(), "no initialize script ran");
    assert!(s.run_javascript(id, "", Some("addRow")).is_err());
    assert!(!s.get(id).unwrap().xfa_scripts_off(), "the preference, not a script that ran away");
}

#[test]
fn hostile_xfa_scripts_cannot_hang_or_bloat_the_document() {
    // A calculate script that adds a row every time it runs, a click script that loops on
    // addInstance, and one that nests setInstances: each ends quickly with a bounded form.
    let tpl = pdfcraft_xfa::fixtures::scripted_template()
        .replace(
            r#"<calculate><script contentType="application/x-javascript">qty.rawValue * price.rawValue</script></calculate>"#,
            r#"<calculate><script contentType="application/x-javascript">table._row.addInstance(1); table._row.count</script></calculate>"#,
        )
        .replace(
            r#"table._row.addInstance(1);</script>"#,
            r#"for (var i = 0; i < 100000; i++) { table._row.addInstance(1); table._row.setInstances(0); }</script>"#,
        );
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let started = std::time::Instant::now();
    let id = s.open("hostile.pdf", None, bytes, None).expect("opens");
    let rows = |s: &Session| s.get(id).unwrap().form.iter().filter(|f| f.name.starts_with("amount")).count();
    assert!(rows(&s) <= 50, "{} rows", rows(&s));
    let o = s.run_javascript(id, "", Some("addRow")).unwrap();
    assert!(o.error.is_some() || rows(&s) <= 50, "{o:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
}

#[test]
fn xfa_scripts_can_remove_rows_twice_and_reset_fields_by_name() {
    let tpl = pdfcraft_xfa::fixtures::scripted_template()
        .replace(r#"if (table._row.count > 1) { table._row.removeInstance(table._row.count - 1); }"#, r#"table._row.setInstances(1);"#)
        .replace(r#"xfa.host.messageBox("Hello " + qty.rawValue);"#, r#"xfa.host.resetData("qty, table");"#);
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("rows.pdf", None, bytes, None).expect("opens");
    let rows = |s: &Session| s.get(id).unwrap().form.iter().filter(|f| f.name.starts_with("amount")).count();
    s.run_javascript(id, "", Some("addRow")).unwrap();
    s.run_javascript(id, "", Some("addRow")).unwrap();
    assert_eq!(rows(&s), 3);
    s.apply(id, Edit::SetFieldValue { name: "amount".into(), value: FieldValue::Text("7".into()) }).unwrap();
    // setInstances(1) removes two rows in one script: both go, nothing comes back.
    s.run_javascript(id, "", Some("removeRow")).unwrap();
    assert_eq!(rows(&s), 1, "{:?}", s.get(id).unwrap().form.iter().map(|f| &f.name).collect::<Vec<_>>());
    let cos = pdfcraft_cos::Document::open(s.get(id).unwrap().bytes.clone()).unwrap();
    let data = pdfcraft_xfa::data_of(&cos).unwrap();
    assert_eq!(data.count(&pdfcraft_xfa::som_to_path("form[0].page1[0].table[0]"), "row"), 1);
    // resetData with names clears those fields (the row's amount too) and nothing else.
    s.apply(id, Edit::SetFieldValue { name: "price".into(), value: FieldValue::Text("9".into()) }).unwrap();
    let o = s.run_javascript(id, "", Some("hello")).unwrap();
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).unwrap().value.clone();
    assert!(value(&s, "qty").is_empty(), "{:?} {o:?}", value(&s, "qty"));
    assert!(value(&s, "amount").is_empty());
    assert_eq!(value(&s, "price"), vec!["9".to_string()]);
}

/// Streams in `bytes` (every revision) that hold an XFA datasets packet.
fn datasets_streams(bytes: &Arc<Vec<u8>>) -> usize {
    let cos = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
    cos.object_numbers()
        .into_iter()
        .filter(|&n| {
            cos.try_get(n).ok().is_some_and(|o| match &*o {
                pdfcraft_cos::Object::Stream(s) => s.decoded_within(1 << 24).is_ok_and(|b| b.windows(9).any(|w| w == b"datasets>")),
                _ => false,
            })
        })
        .count()
}

#[test]
fn hostile_xfa_open_scripts_are_merged_capped_kept_from_printing_and_noted() {
    // On open: a calculate setting its own value 50 000 times, an initialize script showing a
    // thousand messages and asking to print, open a link and save.
    let tpl = pdfcraft_xfa::fixtures::scripted_template()
        .replace(
            r#"<calculate><script contentType="application/x-javascript">qty.rawValue * price.rawValue</script></calculate>"#,
            r#"<calculate><script contentType="application/x-javascript">for (var i = 0; i &lt; 50000; i++) this.rawValue = i;</script></calculate>"#,
        )
        .replace(
            r#"if (qty.rawValue === null) qty.rawValue = 2;"#,
            r#"for (var i = 0; i &lt; 1000; i++) xfa.host.messageBox("m" + i); xfa.host.print(); xfa.host.gotoURL("https://example.com/"); app.execMenuItem("SaveAs"); if (qty.rawValue === null) qty.rawValue = 2;"#,
        );
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let started = std::time::Instant::now();
    let id = s.open("hostile.pdf", None, bytes, None).expect("opens");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    let doc = s.get(id).unwrap();
    assert_eq!(doc.form.iter().find(|f| f.name == "total").unwrap().value, vec!["49999".to_string()]);
    // One datasets stream for the scripts' revision, not one per value set.
    assert!(datasets_streams(&doc.bytes) <= 3, "{} datasets streams", datasets_streams(&doc.bytes));
    // What opening changed is noted with the form's other XFA notes.
    assert!(doc.xfa_warnings.iter().any(|w| w.contains("scripts changed it on opening")), "{:?}", doc.xfa_warnings);
    let out = s.take_js_output(id);
    assert_eq!(out.alerts.len(), 100, "message boxes are capped");
    assert!(out.errors.iter().any(|e| e.contains("more than 100 messages")), "{:?}", out.errors);
    assert!(out.requests.is_empty(), "only a click may print, save or open links: {:?}", out.requests);
    assert!(out.console.iter().any(|c| c.contains("only print when a button is clicked")), "{:?}", out.console);
}

#[test]
fn runaway_xfa_calculations_at_open_end_quickly_with_a_report() {
    // Nested loops in one frame stop at the loop limit; loops in nested calls (that limit is per
    // call) are abandoned at the time limit and turn the form's scripts off.
    let tpl = pdfcraft_xfa::fixtures::scripted_template()
        .replace(
            r#"<calculate><script contentType="application/x-javascript">qty.rawValue * price.rawValue</script></calculate>"#,
            r#"<calculate><script contentType="application/x-javascript">for (var i = 0; i &lt; 1e9; i++) { for (var j = 0; j &lt; 1e9; j++) {} }</script></calculate>"#,
        )
        .replace(
            r#"var t = 0; var rows"#,
            r#"function f() { for (var j = 0; j &lt; 99999; j++) {} } for (var k = 0; k &lt; 5000; k++) f(); var t = 0; var rows"#,
        );
    let bytes = Arc::new(pdfcraft_xfa::fixtures::shell(&tpl));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let started = std::time::Instant::now();
    let id = s.open("runaway.pdf", None, bytes, None).expect("opens");
    assert!(started.elapsed() < std::time::Duration::from_secs(4), "{:?}", started.elapsed());
    let out = s.take_js_output(id);
    assert!(out.errors.iter().any(|e| e.contains("total") && e.contains("maximum number of iteration")), "{:?}", out.errors);
    assert!(out.errors.iter().any(|e| e.contains("abandoned")), "{:?}", out.errors);
    assert!(out.errors.iter().any(|e| e.contains("scripts are off")), "{:?}", out.errors);
    assert!(s.get(id).unwrap().xfa_scripts_off());
    // The scripts stay off: a change runs nothing more.
    let started = std::time::Instant::now();
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("3".into()) }).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(s.take_js_output(id).errors.is_empty());
    assert!(s.get(id).unwrap().xfa_scripts_off());
}

/// The page view's raster of `page` at 1 px/pt (the document's own render pool).
fn shown(s: &Session, id: DocId, page: usize) -> pdfcraft_render::RenderedPage {
    let doc = s.get(id).unwrap();
    doc.renderer.set_queue(vec![pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() }]);
    let start = std::time::Instant::now();
    loop {
        if let Some(p) = doc.renderer.try_recv() {
            assert!(p.error.is_none(), "{:?}", p.error);
            return p;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "no render");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// #260 (page 7): comments without an appearance stream, here a FreeText box (`/C` background,
/// blue `/DA` text) and an Ink stroke, weren't drawn. The page view now draws them from
/// appearances made for display only; the working file and what Save writes stay as they were.
#[test]
fn comments_without_appearances_are_drawn_but_not_saved() {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        b"<< /Type /Annot /Subtype /FreeText /Rect [20 200 180 260] /Contents (Box) /DA (/Helv 10 Tf 0 0 1 rg) /Border [0 0 1] /C [.85 .47 .02] >>",
        b"<< /Type /Annot /Subtype /Ink /Rect [20 20 180 120] /InkList [[30 30 100 110 170 30]] /Border [0 0 4] /C [0 .6 0] >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(o);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let original = Arc::new(bytes);
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("comments.pdf", None, original.clone(), None).expect("opens");
    let check = |s: &Session| {
        let p = shown(s, id, 0);
        let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
        // y down: the FreeText box spans rows 40..100, the Ink apex is at (100, 190).
        assert_eq!(px(100, 90), vec![217, 120, 5, 255], "the FreeText background");
        // On the apex row: Ink is drawn as a curve through its points (#374), level at the apex.
        assert_eq!(px(100, 190), vec![0, 153, 0, 255], "the Ink stroke");
    };
    check(&s);
    let doc = s.get(id).unwrap();
    assert!(Arc::ptr_eq(&doc.bytes, &original), "the working file is the file as opened");
    assert!(!Arc::ptr_eq(&doc.display, &doc.bytes) && Arc::ptr_eq(&doc.export_source().bytes, &doc.display), "page images show it too");
    // After an edit (a scoped refresh) they are still drawn, and Save writes no appearances.
    s.apply(id, rect_comment(0, [150.0, 270.0, 190.0, 290.0])).unwrap();
    check(&s);
    let saved = s.save_bytes(id).unwrap();
    let reopened = pdfcraft_cos::Document::open(saved).unwrap();
    for r in [pdfcraft_cos::ObjRef::new(4, 0), pdfcraft_cos::ObjRef::new(5, 0)] {
        assert!(reopened.get(r).as_dict().is_some_and(|d| !d.contains(b"AP")), "{r:?} is saved without /AP");
    }
}

/// ISO 32000-2 Table 178: with `/LL`, `/L` gives the ends of the leader lines, and the line itself
/// is drawn `/LL` from it (above, for a positive `/LL` on a line drawn left to right). The page
/// view shows the line there, with its leaders down to `/L`, and nothing along `/L` itself.
#[test]
fn a_line_without_an_appearance_is_drawn_at_its_leader_length() {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Line /Rect [0 0 200 300] /L [40 100 160 100] /LL 30 /C [1 0 0] /BS << /W 4 >> >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(o);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("line.pdf", None, Arc::new(bytes), None).expect("opens");
    let p = shown(&s, id, 0);
    let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
    // y down: /L is row 200, the line itself row 170.
    assert_eq!(px(100, 170), vec![255, 0, 0, 255], "the line, 30 above /L");
    assert_eq!(px(40, 185), vec![255, 0, 0, 255], "the leader line at the start");
    assert_eq!(px(160, 185), vec![255, 0, 0, 255], "the leader line at the end");
    assert_eq!(px(100, 200), vec![255, 255, 255, 255], "nothing along /L");
}

/// A stamp's `/Rect` and the colour family expected inside it.
type StampRect = ([f64; 4], char);

/// Rubber stamps without an appearance (ISO 32000-2 Table 184): the fourteen standard names, one
/// without `/Name`, one custom name, and last a Draft stamp with artwork of its own (magenta).
/// Each sits in a 120 × 60 `/Rect` on a 612 × 792 page; the name and the expected colour family
/// (`'r'`, `'g'`, `'b'`, or `'n'` for the neutral placeholder) of each, in page order.
fn stamps_without_appearances() -> (Arc<Vec<u8>>, Vec<StampRect>) {
    let stamps: [(Option<&str>, char); 16] = [
        (Some("Approved"), 'g'),
        (Some("Experimental"), 'b'),
        (Some("NotApproved"), 'r'),
        (Some("AsIs"), 'b'),
        (Some("Expired"), 'r'),
        (Some("NotForPublicRelease"), 'r'),
        (Some("Confidential"), 'r'),
        (Some("Final"), 'g'),
        (Some("Sold"), 'g'),
        (Some("Departmental"), 'b'),
        (Some("ForComment"), 'b'),
        (Some("TopSecret"), 'r'),
        (Some("Draft"), 'b'),
        (Some("ForPublicRelease"), 'b'),
        (None, 'b'),
        (Some("MyCustomStamp"), 'n'),
    ];
    let mut objs: Vec<String> =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".into(), String::new()];
    let mut expected = Vec::new();
    for (i, (name, family)) in stamps.iter().enumerate() {
        let (x, y) = (30 + (i % 4) * 145, 660 - (i / 4) * 150);
        let rect = [x as f64, y as f64, x as f64 + 120.0, y as f64 + 60.0];
        let name = name.map(|n| format!("/Name /{n}")).unwrap_or_default();
        objs.push(format!("<< /Type /Annot /Subtype /Stamp /Rect [{} {} {} {}] {name} /F 4 >>", rect[0], rect[1], rect[2], rect[3]));
        expected.push((rect, *family));
    }
    let own = objs.len() + 2;
    objs.push(format!("<< /Type /Annot /Subtype /Stamp /Rect [30 60 150 120] /Name /Draft /F 4 /AP << /N {own} 0 R >> >>"));
    expected.push(([30.0, 60.0, 150.0, 120.0], 'm'));
    objs.push("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 23 >>\nstream\n1 0 1 rg 0 0 10 10 re f\nendstream".into());
    let annots: Vec<String> = (4..own).map(|n| format!("{n} 0 R")).collect();
    objs[2] = format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", annots.join(" "));
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    (Arc::new(bytes), expected)
}

/// The colour family of an RGBA pixel that isn't white: `'r'`, `'g'` or `'b'` for the dominant
/// channel, `'n'` for a grey, `'m'` for magenta.
fn colour_family(p: &[u8]) -> Option<char> {
    let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
    if r.min(g).min(b) > 200 {
        return None;
    }
    Some(if (r - g).abs() < 24 && (g - b).abs() < 24 {
        'n'
    } else if r > 200 && b > 200 && g < 80 {
        'm'
    } else if r > g && r > b {
        'r'
    } else if g > r && g > b {
        'g'
    } else {
        'b'
    })
}

/// Rubber stamps without an appearance are drawn from their `/Name` (ISO 32000-2 Table 184), in
/// the colour of the stamp they name; an unknown name gets a grey placeholder; a stamp's own
/// artwork is drawn as it is. Display only: Save writes the file without new appearances.
#[test]
fn stamps_without_appearances_draw_the_stamp_they_name() {
    let (original, expected) = stamps_without_appearances();
    let mut s = Session::new();
    let id = s.open("stamps.pdf", None, original.clone(), None).expect("opens");
    let p = shown(&s, id, 0);
    for (i, (rect, family)) in expected.iter().enumerate() {
        let mut counts = std::collections::HashMap::new();
        for y in (792.0 - rect[3]) as u32..(792.0 - rect[1]) as u32 {
            for x in rect[0] as u32..rect[2] as u32 {
                let px = &p.rgba[((y * p.width + x) * 4) as usize..][..4];
                if let Some(f) = colour_family(px) {
                    *counts.entry(f).or_insert(0) += 1;
                }
            }
        }
        let total: u32 = counts.values().sum();
        let main = counts.iter().max_by_key(|(_, n)| **n).map(|(f, _)| *f);
        assert!(total > 300, "stamp {i} draws ink inside its /Rect: {counts:?}");
        assert_eq!(main, Some(*family), "stamp {i} is drawn in its colour: {counts:?}");
    }
    let saved = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    for n in 4..20 {
        assert!(saved.get(pdfcraft_cos::ObjRef::new(n, 0)).as_dict().is_some_and(|d| !d.contains(b"AP")), "{n} is saved without /AP");
    }
}

/// A page with a FreeText comment that has no `/AP` stream: the smallest case behind #2371.
fn free_text_without_appearance() -> Arc<Vec<u8>> {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /FreeText /Rect [20 200 180 260] /Contents (Box) /DA (/Helv 10 Tf 0 0 1 rg) /Border [0 0 1] /C [.85 .47 .02] >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(o);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Arc::new(bytes)
}

/// #2371: a comment without an appearance stream is displayed by the app, so the CLI's page image
/// must show it too. The helper's copy is for rendering only; documents that need no copy, and
/// files it can't display, keep exactly the original source bytes.
#[test]
fn cli_display_adds_missing_appearances_without_changing_the_source() {
    let original = free_text_without_appearance();
    let display = display_bytes_for(original.clone(), None);
    assert_ne!(display.as_slice(), original.as_slice(), "the display copy draws the missing comment appearance");

    let doc = pdfcraft_cos::Document::open(display).unwrap();
    let object = doc.get(pdfcraft_cos::ObjRef::new(4, 0));
    let annot = object.as_dict().unwrap();
    assert!(annot.contains(b"AP"), "the display copy has an appearance stream");

    let plain = Arc::new(fixture(1));
    assert!(Arc::ptr_eq(&display_bytes_for(plain.clone(), None), &plain), "a complete document needs no copy");
    let damaged = Arc::new(b"not a PDF".to_vec());
    assert!(Arc::ptr_eq(&display_bytes_for(damaged.clone(), None), &damaged), "an unreadable file falls back to its source");
}

/// Titles a browser writes for a blank page or a pop-up name no document; real titles stay.
#[test]
fn placeholder_titles_are_recognised() {
    for t in ["about:blank", "About:Blank", " about:srcdoc ", "blob:https://bank.example/3f2a", "data:text/html,x", "Untitled", "untitled"] {
        assert!(is_placeholder_title(t), "{t:?}");
    }
    for t in ["Quarterly report", "Untitled report", "About: our company", "Statement", "blank"] {
        assert!(!is_placeholder_title(t), "{t:?}");
    }
}

/// A form whose fields are only page widgets (the `/Fields` list is empty): they are adopted, and
/// the leniency is recorded in the repair log rather than applied silently.
#[test]
fn adopting_page_only_fields_is_noted_as_a_repair() {
    let objs: Vec<&str> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>",                                     // 1
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",                     // 2
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R] >>",                                 // 3
        "<< /Fields [] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 7 0 R >> >> >>",               // 4
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (alpha) /Rect [10 200 90 220] /P 3 0 R >>", // 5
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (beta) /Rect [10 150 90 170] /P 3 0 R >>",  // 6
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",                                // 7
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("orphan.pdf", None, Arc::new(bytes), None).expect("opens");
    let doc = s.get(id).unwrap();
    let names: Vec<&str> = doc.form.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"]);
    assert_eq!(doc.repair_log(), vec!["the form's /Fields list omits 2 fields; they were read from the page annotations".to_string()]);
}

/// A page too large for the renderer at the asked resolution exports at the most it allows (and
/// says at what), as File ▸ Export does; a source that asks for strictness refuses it instead.
#[test]
fn oversized_exports_clamp_and_report_their_dpi_unless_strict() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    // 14,400 pt (200 in) wide: 600 dpi would be 120,000 px.
    let bytes = s.create_blank(14_400.0, 792.0, 1).unwrap();
    let id = s.open("wide.pdf", None, bytes, None).unwrap();
    let src = s.get(id).unwrap().export_source();
    let mut ex = export::Exporter::from_source(src.clone());
    let used = ex.dpi_used(0, 600.0);
    assert!(used < 600.0 && used > 0.0, "{used}");
    let png = ex.png(0, 600.0).expect("clamped, not refused");
    assert!(png.starts_with(b"\x89PNG"));
    assert!((ex.dpi_used(0, 18.0) - 18.0).abs() < 0.01, "a small enough request (3,600 px) is drawn as asked");
    let mut strict = src;
    strict.config.reject_oversize = true;
    let err = export::Exporter::from_source(strict).png(0, 600.0).unwrap_err();
    assert!(err.contains("exceeds renderer limits"), "{err}");
}

#[test]
fn mixed_object_moves_are_one_undo_step_and_survive_save_reopen() {
    let (mut s, id) = session_with(1);
    let text = AddedText { text: "Added label".into(), rect: [20.0, 240.0, 140.0, 265.0], ..Default::default() };
    s.apply(id, Edit::AddText { page: 0, text }).unwrap();
    let image = image::RgbImage::from_pixel(12, 8, image::Rgb([240, 20, 20]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    s.apply(id, Edit::AddImage { page: 0, rect: Some([40.0, 40.0, 80.0, 70.0]), name: "red.png".into(), bytes: Arc::new(png.into_inner()) }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved.clone(), None).unwrap();
    let before = s.get(id).unwrap().editable_objects(0).unwrap();
    assert_eq!(before.len(), 3, "one native paragraph, one added label and one added image");
    let generation = s.get(id).unwrap().edit_generation();
    let targets = before.iter().map(|o| o.target).collect();
    s.apply(id, Edit::MoveObjects { page: 0, objects: targets, offset: [24.0, -24.0] }).unwrap();
    let doc = s.get(id).unwrap();
    assert!(doc.dirty && doc.edit_generation() > generation);
    assert_eq!(doc.can_undo(), Some("Move objects"));
    let after = doc.editable_objects(0).unwrap();
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.text, b.text);
        for (i, (x, y)) in a.rect.iter().zip(b.rect).enumerate() {
            assert!((y - x - if i % 2 == 0 { 24.0 } else { -24.0 }).abs() < 0.001);
        }
    }
    let moved = s.save_bytes(id).unwrap();
    assert!(moved.starts_with(saved.as_ref()), "an incremental update preserves the original revision");
    let mut again = Session::new();
    let other = again.open("moved.pdf", None, moved.clone(), None).unwrap();
    assert_eq!(again.get(other).unwrap().editable_objects(0).unwrap(), after);
    assert_eq!(s.undo(id).unwrap().as_str(), "Move objects");
    assert_eq!(s.get(id).unwrap().editable_objects(0).unwrap(), before);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add image"), "one undo restores all three objects");
    s.redo(id).unwrap();
    assert_eq!(s.get(id).unwrap().editable_objects(0).unwrap(), after);
    let mut renderer = pdfcraft_render::PageRenderer::new(moved, Default::default());
    let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    let pixel = |x: usize, y: usize| &rendered.rgba[(y * rendered.width as usize + x) * 4..][..3];
    assert!(pixel(84, 269)[0] > 200 && pixel(84, 269)[1] < 50, "the added image renders at its new position");
    assert!(pixel(45, 240).iter().all(|v| *v > 240), "the old image position is clear");
}

#[test]
fn failed_group_moves_preserve_generation_dirty_state_and_redo_history() {
    let (mut s, id) = session_with(1);
    let objects: Vec<_> = s.get(id).unwrap().editable_objects(0).unwrap().iter().map(|o| o.target).collect();
    s.apply(id, Edit::MoveObjects { page: 0, objects: objects.clone(), offset: [10.0, 0.0] }).unwrap();
    s.undo(id).unwrap();
    let before = s.save_bytes(id).unwrap();
    let generation = s.get(id).unwrap().edit_generation();
    let dirty = s.get(id).unwrap().dirty;
    let mut bad = objects;
    bad.push(ObjectTarget { kind: ObjectKind::Image, index: 999 });
    assert!(s.apply(id, Edit::MoveObjects { page: 0, objects: bad, offset: [10.0, 0.0] }).is_err());
    let doc = s.get(id).unwrap();
    assert_eq!(doc.edit_generation(), generation);
    assert_eq!(doc.dirty, dirty);
    assert!(doc.can_undo().is_none());
    assert_eq!(doc.can_redo(), Some("Move object"));
    assert_eq!(s.save_bytes(id).unwrap(), before);
    s.redo(id).unwrap();
    assert!(s.get(id).unwrap().dirty);
}

#[test]
fn group_move_respects_modify_permission_and_display_coordinate_rotation() {
    for rotation in [0, 90, 180, 270] {
        let (mut s, id) = session_with(1);
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: rotation }).unwrap();
        let doc = s.get(id).unwrap();
        let delta = doc.object_move_offset(0, [17.0, 23.0]).unwrap();
        let info = &doc.info.pages[0];
        let a = info.user_to_view(30.0, 70.0);
        let b = info.user_to_view(30.0 + delta[0] as f32, 70.0 + delta[1] as f32);
        assert!((b[0] - a[0] - 17.0).abs() < 0.001 && (b[1] - a[1] - 23.0).abs() < 0.001);
    }
    let (mut s, id) = session_with(1);
    let mut p = protection(None, Some("owner"));
    p.changes = Changes::None;
    s.apply(id, Edit::Protect(p)).unwrap();
    let bytes = s.save_bytes(id).unwrap();
    let mut reader = Session::new();
    let id = reader.open("restricted.pdf", None, bytes, None).unwrap();
    let objects = reader.get(id).unwrap().editable_objects(0).unwrap().iter().map(|o| o.target).collect();
    let generation = reader.get(id).unwrap().edit_generation();
    assert!(reader.apply(id, Edit::MoveObjects { page: 0, objects, offset: [10.0, 0.0] }).is_err());
    let doc = reader.get(id).unwrap();
    assert!(!doc.dirty && doc.can_undo().is_none() && doc.edit_generation() == generation);
}

#[test]
fn group_moves_preserve_rotated_added_content_raster_geometry() {
    let (mut s, id) = session_with(1);
    s.apply(id, Edit::AddText { page: 0, text: AddedText { text: "Added label".into(), rect: [20.0, 240.0, 140.0, 265.0], ..Default::default() } })
        .unwrap();
    let image = image::RgbImage::from_fn(20, 10, |x, _| image::Rgb(if x < 10 { [240, 20, 20] } else { [20, 20, 240] }));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    s.apply(id, Edit::AddImage { page: 0, rect: Some([40.0, 40.0, 80.0, 70.0]), name: "quadrants.png".into(), bytes: Arc::new(png.into_inner()) })
        .unwrap();
    let AddedContent::Image(mut image) = s.get(id).unwrap().added()[1].content.clone() else { panic!() };
    image.flip_h = true;
    image.crop = [0.1, 0.0, 0.2, 0.0];
    s.apply(id, Edit::UpdateContent { page: 0, index: 1, content: AddedContent::Image(image) }).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let render = |bytes| {
        let mut r = pdfcraft_render::PageRenderer::new(bytes, Default::default());
        r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() })
    };
    let before = render(s.get(id).unwrap().bytes.clone());
    let doc = s.get(id).unwrap();
    let offset = doc.object_move_offset(0, [24.0, 36.0]).unwrap();
    let objects = doc.editable_objects(0).unwrap().iter().map(|o| o.target).collect();
    s.apply(id, Edit::MoveObjects { page: 0, objects, offset }).unwrap();
    let after = render(s.get(id).unwrap().bytes.clone());
    assert_eq!((before.width, before.height), (after.width, after.height));
    let mut painted = 0;
    for y in 0..before.height as usize {
        for x in 0..before.width as usize {
            let pos = (y * before.width as usize + x) * 4;
            let expected = if x >= 24 && y >= 36 {
                let old = ((y - 36) * before.width as usize + x - 24) * 4;
                &before.rgba[old..old + 4]
            } else {
                &[255, 255, 255, 255]
            };
            // Raster coverage can round by one intensity level when equivalent placements
            // reach the renderer through different text operators. Geometry and image colours
            // must still agree across every pixel of the translated page.
            assert!(after.rgba[pos..pos + 4].iter().zip(expected).all(|(a, b)| a.abs_diff(*b) <= 2), "pixel {x},{y}");
            painted += usize::from(expected[..3].iter().any(|c| *c < 240));
        }
    }
    assert!(painted > 500, "the comparison contains real rendered text and image pixels");
}

#[test]
fn legacy_passwords_with_turkish_characters_open() {
    // RC4-128 (R3): user "ılık€" in PDFDocEncoding (ı = 0x9A, € = 0xA0), owner "Çağlar"
    // (PDFDocEncoding has no ğ: written without it, as pdfcraft-crypt writes R2–R4 passwords).
    let mut cos = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    let params = pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Rc4_128,
        user_password: "ılık€",
        owner_password: "Çağlar",
        permissions: -1,
        encrypt_metadata: true,
        seed: [7; 32],
    };
    cos.set_encryption(&params).unwrap();
    let bytes = Arc::new(pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap());
    let mut s = Session::new();
    // The renderer used to try only the UTF-8 bytes, and the owner fallback handed it the
    // recovered user password as Latin-1: both came back as a wrong password.
    for pw in ["ılık€", "Çağlar"] {
        let id = s.open("x.pdf", None, bytes.clone(), Some(pw)).unwrap_or_else(|e| panic!("{pw}: {e:?}"));
        assert_eq!(page_texts(&s, id), ["Page 1"], "{pw}");
        assert!(s.get(id).unwrap().info.encrypted, "{pw}");
    }
    assert!(matches!(s.open("x.pdf", None, bytes, Some("ilik€")), Err(OpenError::WrongPassword)));
}

#[test]
fn recovered_user_passwords_reach_the_renderer_byte_for_byte() {
    assert_eq!(crate::renderer_password(b"pw"), "pw");
    // UTF-8 bytes (qpdf writes "şifre" so; its \xC5\x9F has no PDFDocEncoding reading).
    assert_eq!(crate::renderer_password("şifre".as_bytes()), "şifre");
    // PDFDocEncoding bytes: ı = 0x9A, € = 0xA0, Ç = 0xC7.
    assert_eq!(crate::renderer_password(&[0x9A, b'l', 0x9A, b'k', 0xA0]), "ılık€");
    assert_eq!(crate::renderer_password(&[0xC7, b'o', b'k']), "Çok");
}

/// AES-256 passwords as typed, each with its SASLprep form, which ISO 32000-2 keys the file on:
/// a decomposed é (NFKC composes it), a no-break space (mapped to a space) and fullwidth letters
/// (what a Japanese IME types in full-width mode; NFKC narrows them). Precomposed "café" is the
/// control: SASLprep leaves it as it is.
const SASLPREP_CHANGES: [(&str, &str); 4] =
    [("cafe\u{301}", "caf\u{E9}"), ("a\u{A0}b", "a b"), ("\u{FF41}\u{FF42}", "ab"), ("caf\u{E9}", "caf\u{E9}")];

#[test]
fn aes256_open_passwords_that_saslprep_changes_protect_and_reopen_as_typed() {
    for (typed, prepared) in SASLPREP_CHANGES {
        let (mut s, id) = session_with(1);
        // The renderer used to take the password as typed, so the protected file didn't reopen
        // and the edit was rolled back: "the password is incorrect".
        s.apply(id, Edit::Protect(protection(Some(typed), None))).unwrap_or_else(|e| panic!("{typed:?}: {e:?}"));
        assert!(s.get(id).unwrap().info.encrypted, "{typed:?}");
        assert_eq!(page_texts(&s, id), ["Page 1"], "{typed:?}");
        let saved = s.save_bytes(id).unwrap();
        for pw in [typed, prepared] {
            let mut again = Session::new();
            let reopened = again.open("x.pdf", None, saved.clone(), Some(pw)).unwrap_or_else(|e| panic!("{typed:?} as {pw:?}: {e:?}"));
            assert_eq!(page_texts(&again, reopened), ["Page 1"], "{typed:?} as {pw:?}");
        }
        assert!(matches!(Session::new().open("x.pdf", None, saved, Some("cafe")), Err(OpenError::WrongPassword)), "{typed:?}");
    }
}

#[test]
fn aes256_files_whose_passwords_saslprep_changes_open_as_typed() {
    // A fullwidth Ｏ: SASLprep makes the owner password "Owner".
    let (owner, owner_prepared) = ("\u{FF2F}wner", "Owner");
    for (typed, prepared) in SASLPREP_CHANGES {
        // pdfcraft-cos keys the file on the SASLprep forms, as ISO 32000-2 asks.
        let mut cos = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
        let params = pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: typed,
            owner_password: owner,
            permissions: -1,
            encrypt_metadata: true,
            seed: [7; 32],
        };
        cos.set_encryption(&params).unwrap();
        let bytes = Arc::new(pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap());
        let mut s = Session::new();
        // pdfcraft-cos accepts the password as typed (it applies SASLprep); the renderer used to
        // refuse it, so the session called it a wrong password.
        for pw in [typed, prepared, owner, owner_prepared] {
            let id = s.open("x.pdf", None, bytes.clone(), Some(pw)).unwrap_or_else(|e| panic!("{typed:?} as {pw:?}: {e:?}"));
            assert_eq!(page_texts(&s, id), ["Page 1"], "{typed:?} as {pw:?}");
            assert!(s.get(id).unwrap().info.encrypted, "{typed:?} as {pw:?}");
        }
        assert!(matches!(s.open("x.pdf", None, bytes, Some("cafe")), Err(OpenError::WrongPassword)), "{typed:?}");
    }
}

/// A synthetic phone photo, 400 × 300 px: a white sheet, tilted, on a grey table, with a black
/// mark in the sheet's top-left corner. Returns the PNG and the sheet's corners in pixels.
fn camera_photo() -> (Vec<u8>, [[f64; 2]; 4]) {
    let sheet = [[60.0, 40.0], [330.0, 20.0], [350.0, 270.0], [40.0, 250.0]];
    let inside = |x: f64, y: f64| {
        (0..4).all(|i| {
            let (a, b) = (sheet[i], sheet[(i + 1) % 4]);
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]) >= 0.0
        })
    };
    let img = image::RgbImage::from_fn(400, 300, |x, y| {
        let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
        if (75.0..95.0).contains(&fx) && (55.0..75.0).contains(&fy) {
            image::Rgb([0, 0, 0])
        } else if inside(fx, fy) {
            image::Rgb([255, 255, 255])
        } else {
            image::Rgb([110, 110, 110])
        }
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    (png, sheet)
}

#[test]
fn enhance_camera_page_straightens_the_sheet() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let (png, sheet) = camera_photo();
    // 72 dpi: one point per pixel, so the page is 400 × 300 pt and y flips to user space.
    let bytes = s.create_from_images_with_resolution(&[("photo.png".into(), png)], ImageResolution::Dpi(72.0)).unwrap();
    let id = s.open("photo.pdf", None, bytes, None).unwrap();
    let corners = sheet.map(|[x, y]| [x, 300.0 - y]);
    // Corners in any order.
    s.enhance_camera_page(id, 0, [corners[2], corners[0], corners[3], corners[1]]).unwrap();

    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), 1);
    assert_eq!(doc.can_undo(), Some("Enhance camera image"));
    // The longer of each pair of opposite sides: 310 × 250 pt at the photo's own resolution.
    let p = &doc.info.pages[0];
    assert!((p.width - 310.0).abs() <= 1.0 && (p.height - 250.0).abs() <= 1.0, "{} × {}", p.width, p.height);

    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
    let shot = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(shot.error.is_none(), "{:?}", shot.error);
    let at = |x: u32, y: u32| {
        let i = ((y * shot.width + x) * 4) as usize;
        shot.rgba[i..i + 3].to_vec()
    };
    // No table left in the corners: the sheet fills the page, and its mark is top left.
    for (x, y) in [(4, 4), (shot.width - 5, 4), (shot.width - 5, shot.height - 5), (4, shot.height - 5), (shot.width / 2, shot.height / 2)] {
        assert!(at(x, y).iter().all(|&v| v > 235), "({x}, {y}) = {:?}", at(x, y));
    }
    assert!(at(30, 30).iter().all(|&v| v < 40), "the mark: {:?}", at(30, 30));

    s.undo(id).unwrap();
    let p = &s.get(id).unwrap().info.pages[0];
    assert_eq!((p.width, p.height), (400.0, 300.0));
}

#[test]
fn detect_camera_corners_finds_the_sheet_in_user_space() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let (png, sheet) = camera_photo();
    let bytes = s.create_from_images_with_resolution(&[("photo.png".into(), png)], ImageResolution::Dpi(72.0)).unwrap();
    let id = s.open("photo.pdf", None, bytes, None).unwrap();
    let found = s.detect_camera_corners(id, 0).expect("the sheet's edges");
    // One point per pixel, y flipped: the sheet's corners in reading order.
    for (f, p) in found.iter().zip(sheet.iter()) {
        let want = [p[0], 300.0 - p[1]];
        assert!((f[0] - want[0]).hypot(f[1] - want[1]) < 4.0, "{found:?} vs {sheet:?}");
    }
    // A page with no photo on it: nothing to find.
    let id = s.open("doc.pdf", None, Arc::new(fixture(1)), None).unwrap();
    assert_eq!(s.detect_camera_corners(id, 0), None);
    assert_eq!(s.detect_camera_corners(id, 7), None);
}

#[test]
fn enhance_camera_page_at_a_level_whitens_a_shadowed_sheet() {
    // A sheet lit unevenly: full light at the left, a third of it at the right.
    let img = image::RgbImage::from_fn(400, 300, |x, y| {
        let light = 1.0 - 0.66 * x as f32 / 400.0;
        let ink = if (y / 10) % 3 == 1 && (40..360).contains(&x) { 0.12 } else { 0.96 };
        let v = (ink * light * 255.0) as u8;
        image::Rgb([v, v, (v as f32 * 0.85) as u8])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let whole = [[0.0, 0.0], [400.0, 0.0], [400.0, 300.0], [0.0, 300.0]];
    let paper_at_right = |level: u8| {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let bytes = s.create_from_images_with_resolution(&[("photo.png".into(), png.clone())], ImageResolution::Dpi(72.0)).unwrap();
        let id = s.open("photo.pdf", None, bytes, None).unwrap();
        s.enhance_camera_page_at(id, 0, whole, camera_clean::Look::Colour, level).unwrap();
        assert_eq!(s.get(id).unwrap().can_undo(), Some("Enhance camera image"), "one step at level {level}");
        let doc = s.get(id).unwrap();
        let shot = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone()).render(pdfcraft_render::RenderRequest {
            page: 0,
            scale: 1.0,
            ..Default::default()
        });
        // A paper pixel near the right edge, between text rows.
        let i = ((5 * shot.width + shot.width - 12) * 4) as usize;
        shot.rgba[i..i + 3].to_vec()
    };
    let (before, after) = (paper_at_right(0), paper_at_right(80));
    assert!(before.iter().all(|&v| v < 140), "the shadow: {before:?}");
    assert!(after.iter().all(|&v| v > 235), "white paper: {after:?}");
}

#[test]
fn enhance_camera_page_refuses_bad_corners_and_pages() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("doc.pdf", None, Arc::new(fixture(1)), None).unwrap();
    let line = [[0.0, 0.0], [50.0, 50.0], [100.0, 100.0], [150.0, 150.0]];
    let e = s.enhance_camera_page(id, 0, line).unwrap_err().to_string();
    assert!(e.contains("corners"), "{e}");
    let ok = [[0.0, 0.0], [200.0, 0.0], [200.0, 300.0], [0.0, 300.0]];
    assert!(s.enhance_camera_page(id, 5, ok).unwrap_err().to_string().contains("no page 6"));
    assert!(matches!(s.enhance_camera_page(DocId(999), 0, ok), Err(EditError::NoDocument)));
    assert_eq!(s.get(id).unwrap().can_undo(), None, "nothing changed");
}

/// A content-only edit must not disturb the inspection results: pages, annotations, fields,
/// links and marks are unchanged, only the working bytes, the added-content list and the
/// display copy move.
#[test]
fn content_edit_keeps_pages_annotations_fields_and_links() {
    let (mut session, id) = session_with(2);
    let before = session.get(id).unwrap();
    let pages = format!("{:?}", before.info.pages);
    let annotations = format!("{:?}", before.info.annotations);
    let links = format!("{:?}", before.links);
    let form = before.form.clone();
    let marks = before.marks().to_vec();
    let bytes = before.bytes.clone();
    let text = AddedText {
        rect: [20.0, 20.0, 200.0, 60.0],
        text: "hello".into(),
        family: FontFamily::Helvetica,
        bold: false,
        italic: false,
        size: 12.0,
        color: [0.0, 0.0, 0.0],
        align: TextAlign::Left,
        char_spacing: 0.0,
    };
    session.apply(id, Edit::AddText { page: 1, text }).unwrap();
    let after = session.get(id).unwrap();
    assert_eq!(format!("{:?}", after.info.pages), pages, "page geometry is untouched");
    assert_eq!(format!("{:?}", after.info.annotations), annotations);
    assert_eq!(format!("{:?}", after.links), links);
    assert_eq!(after.form, form, "no field was touched");
    assert_eq!(after.marks(), marks, "no mark was touched");
    assert!(after.added().iter().any(|a| a.page == 1), "the added text is listed");
    assert_ne!(after.bytes, bytes, "the working file carries the edit");
    assert_ne!(after.display, bytes, "the display copy carries the edit too");
    // The display bytes still open and render the edited page.
    let mut renderer = pdfcraft_render::PageRenderer::new(after.display.clone(), after.config.clone());
    let rendered = renderer.render(pdfcraft_render::RenderRequest { page: 1, scale: 1.0, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    // Undo goes through the same narrow refresh without the edit at hand.
    session.undo(id).unwrap();
    let undone = session.get(id).unwrap();
    assert!(undone.added().is_empty(), "undo drops the added text");
    assert_eq!(format!("{:?}", undone.info.pages), pages);
}

/// `refresh_content` re-reads only the derived lists the edit can change (undo/redo, with no
/// edit at hand, re-read both).
#[test]
fn content_refresh_lists_match_the_storages_an_edit_touches() {
    let target = |kind| ObjectTarget { kind, index: 0 };
    let added = [
        Edit::AddText {
            page: 0,
            text: AddedText {
                rect: [0.0, 0.0, 10.0, 10.0],
                text: String::new(),
                family: FontFamily::Helvetica,
                bold: false,
                italic: false,
                size: 12.0,
                color: [0.0, 0.0, 0.0],
                align: TextAlign::Left,
                char_spacing: 0.0,
            },
        },
        Edit::MoveObjects { page: 0, objects: vec![target(crate::ObjectKind::Added)], offset: [1.0, 0.0] },
    ];
    for edit in &added {
        assert_eq!(Session::content_refresh_lists(Some(edit)), (true, false), "{}", edit.label());
    }
    let plain = [
        Edit::MoveObjects { page: 0, objects: vec![target(crate::ObjectKind::Text)], offset: [1.0, 0.0] },
        Edit::EditTextBlock { page: 0, block: 0, text: String::new(), style: pdfcraft_edit::BlockStyle::default() },
    ];
    for edit in &plain {
        assert_eq!(Session::content_refresh_lists(Some(edit)), (false, false), "{}", edit.label());
    }
    let marks = [Edit::AddWatermark { pages: vec![0], settings: Watermark::default(), replace: false, file: None }];
    for edit in &marks {
        assert_eq!(Session::content_refresh_lists(Some(edit)), (false, true), "{}", edit.label());
    }
    assert_eq!(Session::content_refresh_lists(None), (true, true), "undo/redo re-read both");
}

/// A content edit can add a font (Add text in Times); the narrow refresh leaves the font list
/// as it was, and `refresh_fonts` reads it again before Properties ▸ Fonts or `doc_info` show it.
#[test]
fn fonts_added_by_a_content_edit_show_after_refresh_fonts() {
    let (mut session, id) = session_with(1);
    let has_times = |s: &Session| s.get(id).unwrap().info.fonts.iter().any(|f| f.name.contains("Times"));
    assert!(!has_times(&session));
    let text = AddedText { text: "serif".into(), rect: [20.0, 20.0, 200.0, 60.0], family: FontFamily::Times, ..Default::default() };
    session.apply(id, Edit::AddText { page: 0, text }).unwrap();
    assert!(!has_times(&session), "the narrow refresh doesn't re-inspect");
    session.refresh_fonts(id);
    assert!(has_times(&session), "{:?}", session.get(id).unwrap().info.fonts);
    // Nothing changed since: a second call is a no-op.
    let fonts = format!("{:?}", session.get(id).unwrap().info.fonts);
    session.refresh_fonts(id);
    assert_eq!(format!("{:?}", session.get(id).unwrap().info.fonts), fonts);
}

/// Gap 13: every edit writes a new working file (the original plus one revision). Before the
/// next one is written, the current one is let go of by the working bytes, the display bytes and
/// the renderer: on a 1 GiB file, keeping it is a whole extra gigabyte at the peak of each edit.
#[test]
fn the_working_file_is_let_go_of_before_the_next_is_written() {
    let (mut s, id) = session_with(2);
    s.apply(id, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
    let doc = s.doc_mut(id).unwrap();
    let (working, original) = (Arc::downgrade(&doc.bytes), doc.editor.as_ref().unwrap().cos.bytes().clone());
    assert!(!Arc::ptr_eq(&doc.bytes, &original), "an edited working file is a buffer of its own");
    Session::release_working(doc);
    assert!(Arc::ptr_eq(&doc.bytes, &original) && Arc::ptr_eq(&doc.display, &original));
    // The old renderer's workers let go of the file when they exit, just after.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while working.upgrade().is_some() {
        assert!(std::time::Instant::now() < deadline, "the working file is still held");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The refresh that follows brings everything back.
    Session::refresh(doc).unwrap();
    assert_eq!(s.get(id).unwrap().info.pages[1].rotation, 90);
    let _ = shown(&s, id, 1);
}

/// Every refresh path (full, comments, content, undo, redo) lets go of the previous working file
/// and still shows the current state.
#[test]
fn edits_let_go_of_the_previous_working_file() {
    let (mut s, id) = session_with(2);
    let shape = Shape::Rectangle { rect: [20.0, 20.0, 60.0, 60.0] };
    let comment = || {
        Edit::AddAnnotation(NewAnnotation {
            page: 0,
            style: Style::default_for(&shape),
            shape: shape.clone(),
            contents: String::new(),
            author: "Test".into(),
        })
    };
    let watermark =
        || Edit::AddWatermark { pages: vec![0], settings: Watermark { text: "Draft".into(), ..Watermark::default() }, file: None, replace: false };
    s.apply(id, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
    // A full refresh, a comment refresh, a content refresh, then undo and redo.
    type Step<'a> = Box<dyn Fn(&mut Session) + 'a>;
    let steps: [(&str, Step<'_>); 5] = [
        ("rotate", Box::new(|s: &mut Session| s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap())),
        ("comment", Box::new(move |s: &mut Session| s.apply(id, comment()).unwrap())),
        ("watermark", Box::new(move |s: &mut Session| s.apply(id, watermark()).unwrap())),
        ("undo", Box::new(|s: &mut Session| drop(s.undo(id).unwrap()))),
        ("redo", Box::new(|s: &mut Session| drop(s.redo(id).unwrap()))),
    ];
    for (name, step) in steps {
        let (bytes, display) = {
            let doc = s.get(id).unwrap();
            (Arc::downgrade(&doc.bytes), Arc::downgrade(&doc.display))
        };
        step(&mut s);
        // The old renderer's workers let go of the file when they exit, just after.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while bytes.upgrade().is_some() || display.upgrade().is_some() {
            assert!(std::time::Instant::now() < deadline, "{name}: the previous working file is still held");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    // What is shown is still the current state.
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.annotations.len(), 1);
    let _ = shown(&s, id, 0);
}

#[test]
fn copy_image_finds_the_image_under_a_point_and_decodes_it_at_full_size() {
    let mut s = Session::new();
    let mut px = image::RgbaImage::new(3, 2);
    px.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    px.put_pixel(2, 1, image::Rgba([0, 0, 255, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(px.clone()).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let pic = s.create_from_images(&[("p.png".into(), png.into_inner())]).unwrap();
    let id = s.open("p.pdf", None, pic, None).unwrap();
    let d = s.get(id).unwrap();
    let r = d.page_images(0)[0].rect;
    assert_eq!(d.page_image_at(0, [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]), Some(0));
    assert_eq!(d.page_image_at(0, [r[2] + 10.0, r[3] + 10.0]), None);
    assert_eq!(d.page_image_at(7, [0.0, 0.0]), None);
    let (w, h, rgba) = d.page_image_rgba(0, 0).unwrap();
    assert_eq!((w, h), (3, 2));
    assert_eq!(rgba, px.into_raw());
    assert!(d.page_image_rgba(0, 1).is_err());
}

/// Copy Image: a JPEG drawn through grouped Form artwork (as Word, InDesign and scanners write
/// them), with a caption over it and an empty full-page Form on top.
#[test]
fn copy_image_finds_a_picture_inside_form_artwork_under_text() {
    let jpeg = jpeg_bytes();
    let content = b"q 100 0 0 100 50 50 cm /Fm1 Do Q BT /F1 12 Tf 60 100 Td (Caption over the picture) Tj ET /Fm2 Do".to_vec();
    let form = b"/Im1 Do".to_vec();
    let stream = |dict: &str, data: &[u8]| {
        let mut o = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        o.extend_from_slice(data);
        o.extend_from_slice(b"\nendstream");
        o
    };
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> /XObject << /Fm1 6 0 R /Fm2 9 0 R >> >> >>".to_vec(),
        stream("", &content),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Im1 7 0 R /Tiny 8 0 R >> >>", &form),
        stream("/Type /XObject /Subtype /Image /Width 3 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", &jpeg),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", &[255, 0, 0]),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 200 300]", b""),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(out), None).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.page_images(0).len(), 2, "{:?}", d.page_images(0));
    // Under the caption, inside the picture: the Form holding it, not the empty Form on top.
    assert_eq!(d.page_image_at(0, [100.0, 100.0]), Some(0));
    assert_eq!(d.page_image_at(0, [10.0, 10.0]), None, "the empty full-page Form holds no picture");
    // The largest picture in the group, as the JPEG stored in the PDF.
    assert_eq!(d.page_image_copy_file(0, 0).unwrap(), ("jpg", jpeg));
    assert!(d.page_image_copy_file(0, 1).is_err());
}

/// `fixture(2)` with the catalog pointing at a page tree object the file does not have (#1656).
fn fixture_without_page_tree() -> Vec<u8> {
    let bytes = fixture(2);
    let catalog = b"<< /Type /Catalog /Pages 2 0 R >>";
    let at = bytes.windows(catalog.len()).position(|w| w == catalog).unwrap();
    let mut out = bytes.clone();
    out[at..at + catalog.len()].copy_from_slice(b"<< /Type /Catalog /Pages 9 0 R >>");
    out
}

#[test]
fn a_recovered_page_tree_opens_unmodified_and_edits_the_pages_shown() {
    let mut s = Session::new();
    let id = s.open("damaged.pdf", None, Arc::new(fixture_without_page_tree()), None).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), 2);
    assert!(!doc.editor.as_ref().unwrap().cos.is_modified(), "the repair is a revision of the opened file, not an unsaved edit");
    assert!(doc.repair_log().iter().any(|l| l.contains("page tree is missing")), "{:?}", doc.repair_log());
    s.apply(id, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
    assert_eq!(s.get(id).unwrap().info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>(), [0, 90]);
}

#[test]
fn a_signed_document_rebuilt_from_object_headers_refuses_edits() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(2)), None).unwrap();
    let opts = SignOptions { page: 1, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    // A broken startxref: the file opens only by rebuilding from object headers, and any save
    // of it is a full rewrite.
    let mut bytes = s.get(id).unwrap().bytes.as_ref().clone();
    let sx = bytes.windows(10).rposition(|w| w == b"startxref\n").unwrap() + 10;
    let end = sx + bytes[sx..].iter().position(|b| !b.is_ascii_digit()).unwrap();
    bytes.splice(sx..end, b"1".iter().copied());
    let id = s.open("damaged.pdf", None, Arc::new(bytes), None).unwrap();
    assert!(s.get(id).unwrap().is_signed());
    assert!(s.get(id).unwrap().editor.as_ref().unwrap().cos.revisions().is_empty(), "the fixture must be rebuilt");
    assert!(matches!(s.apply(id, rect_comment(0, [80.0, 80.0, 120.0, 120.0])), Err(EditError::SignedRewrite(_))));
    assert!(!s.get(id).unwrap().dirty);
}

/// #1324: marking a figure decorative rewrites that figure only. The header, added text and an
/// unknown entry on the original stream stay attached to the page. This lives in the engine tests
/// because edit and a11y are the same layer and cannot depend on each other.
#[test]
fn marking_a_figure_decorative_keeps_other_page_streams() {
    let figure = "/Figure << /MCID 0 >> BDC 10 10 50 50 re f EMC";
    let header = "BT (HeaderSample) Tj ET";
    let added = "BT (AddedSample) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 7 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents [4 0 R 5 0 R 6 0 R] /StructParents 0 >>".into(),
        format!("<< /VendorKey 7 /Length {} >>\nstream\n{figure}\nendstream", figure.len()),
        format!("<< /PCMark /HeaderFooter /Length {} >>\nstream\n{header}\nendstream", header.len()),
        format!(
            "<< /PCMark /Added /PCAdded << /Kind /Text /Rect [70 70 150 100] /Text (AddedSample) /Size 12 >> /Length {} >>\nstream\n{added}\nendstream",
            added.len()
        ),
        "<< /Type /StructTreeRoot /K 8 0 R /ParentTree 10 0 R >>".into(),
        "<< /Type /StructElem /S /Document /P 7 0 R /K [9 0 R] >>".into(),
        "<< /Type /StructElem /S /Figure /P 8 0 R /Pg 3 0 R /K 0 >>".into(),
        "<< /Nums [0 [9 0 R]] >>".into(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(pdfcraft_edit::scan_markers(&doc).marks, vec![MarkKind::HeaderFooter]);
    let before = pdfcraft_edit::scan_markers(&doc);
    assert_eq!(before.added.len(), 1);
    a11y::mark_decorative(&mut doc, pdfcraft_cos::ObjRef::new(9, 0)).unwrap();
    assert!(a11y::figures(&doc).is_empty());
    let page = &pdfcraft_model::pages(&doc)[0];
    let contents = page.dict.get(b"Contents").and_then(|c| c.as_array()).unwrap();
    assert_eq!(contents.len(), 3, "the other streams stay in the page's content list");
    let figure_ref = contents[0].as_ref().unwrap();
    assert_eq!(contents[1].as_ref(), Some(pdfcraft_cos::ObjRef::new(5, 0)));
    assert_eq!(contents[2].as_ref(), Some(pdfcraft_cos::ObjRef::new(6, 0)));
    let stream = |doc: &pdfcraft_cos::Document, n: u32| match &*doc.get(pdfcraft_cos::ObjRef::new(n, 0)) {
        pdfcraft_cos::Object::Stream(s) => s.clone(),
        other => panic!("object {n} is {other:?}"),
    };
    let figure = stream(&doc, figure_ref.num);
    assert_eq!(figure.dict.get(b"VendorKey").and_then(pdfcraft_cos::Object::as_int), Some(7));
    let drawn = String::from_utf8(figure.decoded().unwrap()).unwrap();
    assert!(drawn.contains("/Artifact BMC") && !drawn.contains("/MCID 0"), "{drawn}");
    let header = stream(&doc, 5);
    assert_eq!(header.dict.name(b"PCMark"), Some(&b"HeaderFooter"[..]));
    assert!(String::from_utf8(header.decoded().unwrap()).unwrap().contains("HeaderSample"));
    assert!(stream(&doc, 6).dict.get(b"PCAdded").is_some());
    let after = pdfcraft_edit::scan_markers(&doc);
    assert_eq!(after.marks, vec![MarkKind::HeaderFooter]);
    assert_eq!(after.added.len(), 1);
    assert_eq!(after.added[0].content, before.added[0].content);
    assert_eq!(pdfcraft_edit::remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
    let left = pdfcraft_model::pages(&doc)[0].dict.get(b"Contents").and_then(|c| c.as_array()).cloned().unwrap();
    let text: String = left
        .iter()
        .filter_map(|o| match &*doc.resolve(o) {
            pdfcraft_cos::Object::Stream(s) => String::from_utf8(s.decoded().unwrap()).ok(),
            _ => None,
        })
        .collect();
    assert!(!text.contains("HeaderSample"));
    assert_eq!(pdfcraft_edit::scan_markers(&doc).added[0].content, before.added[0].content);
    assert_eq!(stream(&doc, figure_ref.num).dict.get(b"VendorKey").and_then(pdfcraft_cos::Object::as_int), Some(7));
}

/// Print as image (Acrobat's Advanced Print Setup): the print-ready PDF carries each page as a
/// picture, the size of the page, with no text left to search; in grayscale the pictures are
/// grey. A page used on several sheets is rendered once.
#[test]
fn printing_as_image_sends_pictures_instead_of_pages() {
    let (s, id) = session_with(3);
    let page_size = (s.get(id).unwrap().info.pages[1].width as f64, s.get(id).unwrap().info.pages[1].height as f64);
    let settings = print::Settings {
        pages: vec![1, 2, 1],
        layout: print::Layout::Size(print::SizeMode::Actual),
        raster: Some(print::Raster { dpi: 100.0, grayscale: false }),
        ..Default::default()
    };
    let bytes = s.print_pdf(id, &settings).unwrap();
    let mut s2 = Session::new();
    let id2 = s2.open("print.pdf", None, Arc::new(bytes.clone()), None).unwrap();
    assert_eq!(s2.get(id2).unwrap().info.pages.len(), 3, "three sheets, one page each");
    assert!(page_texts(&s2, id2).iter().all(|t| t.trim().is_empty()), "pictures carry no text");
    let doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    let images: Vec<pdfcraft_cos::Dict> = doc
        .scan_objects()
        .filter_map(|(_, o)| match &*o {
            pdfcraft_cos::Object::Stream(st) if st.dict.name(b"Subtype") == Some(b"Image") => Some(st.dict.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 2, "two distinct pages, each rendered once: {images:?}");
    // 100 dpi: the picture's pixels match the page size.
    let w = images.iter().map(|d| d.int(b"Width").unwrap_or(0)).max().unwrap();
    assert!((w as f64 - page_size.0 * 100.0 / 72.0).abs() <= 1.0, "{w} px for {} pt", page_size.0);
    assert!(images.iter().all(|d| d.name(b"ColorSpace") == Some(b"DeviceRGB")), "{images:?}");
    // Grayscale renders the pictures grey (the printer is not merely asked for monochrome).
    let grey = s.print_pdf(id, &print::Settings { raster: Some(print::Raster { dpi: 72.0, grayscale: true }), ..settings.clone() }).unwrap();
    let mut s3 = Session::new();
    let id3 = s3.open("grey.pdf", None, Arc::new(grey), None).unwrap();
    let mut ex = crate::export::Exporter::new(s3.get(id3).unwrap());
    let png = ex.png(0, 36.0).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert!(img.pixels().all(|p| p[0] == p[1] && p[1] == p[2]), "every pixel grey");
}

/// A one-page PDF: a JPEG logo (DCTDecode) at the top and a big text title below it.
fn jpeg_cover_pdf(logo_y: i32) -> Vec<u8> {
    let mut jpeg = Vec::new();
    let img = image::RgbImage::from_fn(40, 20, |x, y| image::Rgb([(x * 6) as u8, (y * 12) as u8, 90]));
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&img).unwrap();
    let content = format!(
        "q 200 0 0 100 72 {logo_y} cm /Im1 Do Q BT /F1 30 Tf 72 580 Td (Annual Report) Tj ET \
BT /F1 11 Tf 72 500 Td (Body text that is long enough to count as the body size of this page.) Tj ET"
    );
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut add = |pdf: &mut Vec<u8>, n: usize, body: &[u8]| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    };
    add(&mut pdf, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    add(&mut pdf, 2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    add(
        &mut pdf,
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >> >>",
    );
    add(&mut pdf, 4, format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).as_bytes());
    add(&mut pdf, 5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    let mut obj = format!("<< /Type /XObject /Subtype /Image /Width 40 /Height 20 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", jpeg.len()).into_bytes();
    obj.extend_from_slice(&jpeg);
    obj.extend_from_slice(b"\nendstream");
    add(&mut pdf, 6, &obj);
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    pdf
}

/// #1936: a JPEG logo on a cover page reaches the Word file as the unchanged JPEG, both where it
/// sits clear of the title (inline) and where it overlaps it (a positioned page).
#[test]
fn exporting_a_jpeg_logo_to_word_keeps_the_image() {
    for (logo_y, overlaps) in [(650, false), (560, true)] {
        let pdf = jpeg_cover_pdf(logo_y);
        let mut s = Session::new();
        let id = s.open("c.pdf", None, Arc::new(pdf), None).unwrap();
        let d = s.get(id).unwrap();
        let pages = d.export_pages();
        assert_eq!((pages[0].images.len(), pages[0].images[0].ext), (1, "jpg"), "overlap {overlaps}");
        let jpeg = pages[0].images[0].bytes.clone();
        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
        let docx = d.export_office(compare::OfficeFormat::Docx);
        assert!(docx.windows(21).any(|w| w == b"word/media/image1.jpg"), "overlap {overlaps}");
        assert!(docx.windows(jpeg.len()).any(|w| w == &jpeg[..]), "the JPEG is stored unchanged (overlap {overlaps})");
    }
}

/// Cropping an existing page image (Acrobat's crop handles on a picture) shows only the chosen
/// box, the pixels staying where they are: `visible_rect` says what shows, the full placement is
/// unchanged, and a later move carries the crop along.
#[test]
fn cropping_an_existing_image_clips_it_and_moves_with_it() {
    let mut s = Session::new();
    let mut px = image::RgbaImage::new(40, 40);
    for (x, y, p) in px.enumerate_pixels_mut() {
        *p = if x < 20 {
            image::Rgba([255, 0, 0, 255])
        } else if y < 20 {
            image::Rgba([0, 255, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        };
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(px).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let pic = s.create_from_images(&[("p.png".into(), png.into_inner())]).unwrap();
    let id = s.open("p.pdf", None, pic, None).unwrap();
    let full = s.get(id).unwrap().page_images(0)[0].rect;
    let (w, h) = (full[2] - full[0], full[3] - full[1]);
    // Keep the right half, upper part.
    let keep = [full[0] + w / 2.0, full[1] + h / 2.0, full[2], full[3]];
    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Crop(keep) }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!(d.can_undo(), Some("Crop image"));
    let im = &d.page_images(0)[0];
    assert_eq!(im.rect, full, "the placement is not scaled");
    let v = im.visible_rect.expect("clipped");
    assert!(v.iter().zip(keep.iter()).all(|(a, b)| (a - b).abs() < 0.01), "{v:?} vs {keep:?}");
    // What shows: the kept quarter is green, the cropped-away quarters are paper.
    let (pw, ph, rgba) = {
        let mut ex = crate::export::Exporter::new(d);
        let png = ex.png(0, 72.0).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        (img.width(), img.height(), img)
    };
    let at = |fx: f64, fy: f64| rgba.get_pixel((pw as f64 * fx) as u32, (ph as f64 * fy) as u32).0;
    let page_h = d.info.pages[0].height as f64;
    let page_w = d.info.pages[0].width as f64;
    // Fractions of the page from the top-left: the kept box is the top-right quarter of the image.
    let fx = |x: f64| x / page_w;
    let fy = |y: f64| 1.0 - y / page_h;
    assert_eq!(at(fx(full[0] + 0.75 * w), fy(full[1] + 0.75 * h))[1], 255, "kept green");
    assert_eq!(at(fx(full[0] + 0.25 * w), fy(full[1] + 0.25 * h)), [255, 255, 255, 255], "cropped-away red is gone");
    // A move takes the crop along (down and left, so the kept part stays on the page).
    let to = [full[0] - 10.0, full[1] - 10.0, full[2] - 10.0, full[3] - 10.0];
    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Move(to) }).unwrap();
    let im = &s.get(id).unwrap().page_images(0)[0];
    let v = im.visible_rect.expect("still clipped");
    let moved_keep = [keep[0] - 10.0, keep[1] - 10.0, keep[2] - 10.0, keep[3] - 10.0];
    assert!(v.iter().zip(moved_keep.iter()).all(|(a, b)| (a - b).abs() < 0.01), "{v:?} vs {moved_keep:?}");
    // Cropping again replaces the earlier crop, so the picture can grow back (Remove crop).
    let full_now = s.get(id).unwrap().page_images(0)[0].rect;
    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Crop(full_now) }).unwrap();
    let im = &s.get(id).unwrap().page_images(0)[0];
    let v = im.visible_rect.expect("visible");
    let page = [0.0, 0.0, page_w, page_h];
    let shown = [full_now[0].max(page[0]), full_now[1].max(page[1]), full_now[2].min(page[2]), full_now[3].min(page[3])];
    assert!(v.iter().zip(shown.iter()).all(|(a, b)| (a - b).abs() < 0.01), "{v:?} vs {shown:?}");
    // No area, or a box off the image, is refused.
    assert!(s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Crop([10.0, 10.0, 10.0, 50.0]) }).is_err());
}

/// A 60 x 20 JPEG at 144 x 72 dpi, tagged with EXIF Orientation 6 (shown turned 90 degrees clockwise).
fn phone_jpeg() -> Vec<u8> {
    use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};
    let mut jpeg = Vec::new();
    let mut encoder = JpegEncoder::new(&mut jpeg);
    encoder.set_pixel_density(PixelDensity { density: (144, 72), unit: PixelDensityUnit::Inches });
    encoder.encode_image(&image::RgbImage::from_pixel(60, 20, image::Rgb([20, 20, 20]))).unwrap();
    let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend_from_slice(&[6, 0, 0, 0, 0, 0, 0, 0]);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xff, 0xe1]);
    out.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&exif);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[test]
fn a_phone_photo_used_as_a_custom_stamp_is_embedded_upright() {
    let mut session = Session::new();
    let blank = session.create_blank(300.0, 400.0, 1).unwrap();
    let id = session.open_new("form.pdf", blank).unwrap();
    // A click places the stamp at its natural size: 30 x 20 pt as stored, 20 x 30 pt upright.
    // Named "Signature" so `signature_image` finds its picture.
    let file = MarkFile { name: "photo.jpg".into(), bytes: Arc::new(phone_jpeg()), page: 0 };
    let stamp = Edit::AddCustomStamp { page: 0, rect: [100.0, 100.0, 100.0, 100.0], name: "Signature".into(), file, author: "Ada".into() };
    session.apply(id, stamp).unwrap();
    let cos = &session.get(id).unwrap().editor.as_ref().unwrap().cos;
    let picture = pdfcraft_annot::signature_image(cos, 0, 0).unwrap().unwrap();
    let obj = cos.get(picture);
    let pdfcraft_cos::Object::Stream(s) = &*obj else { panic!() };
    assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height")), (Some(20), Some(60)));
    let rect = pdfcraft_annot::summaries(cos)[0].rect;
    assert!(((rect[2] - rect[0]) - 20.0).abs() < 0.01 && ((rect[3] - rect[1]) - 30.0).abs() < 0.01, "{rect:?}");
}

/// Opening a paragraph and applying its edit read the page's text blocks; after an edit they are
/// still read once per document state, and undo and redo never get another state's blocks.
#[test]
fn text_blocks_stay_cached_after_an_edit_and_follow_undo_and_redo() {
    let (mut session, id) = session_with(1);
    let blocks = |s: &Session| s.get(id).unwrap().editor.as_ref().unwrap().text_blocks(0).unwrap();
    let before = blocks(&session);
    assert_eq!(before[0].editor_text, "Page 1");
    let edit = Edit::EditTextBlock { page: 0, block: 0, text: "Edited".into(), style: pdfcraft_edit::BlockStyle::default() };
    session.apply(id, edit).unwrap();
    let edited = blocks(&session);
    assert_eq!(edited[0].editor_text, "Edited");
    assert!(Arc::ptr_eq(&edited, &blocks(&session)), "the edited page is grouped once");
    session.undo(id).unwrap();
    assert_eq!(blocks(&session)[0].editor_text, "Page 1");
    session.redo(id).unwrap();
    let redone = blocks(&session);
    assert_eq!(redone[0].editor_text, "Edited");
    assert!(Arc::ptr_eq(&redone, &blocks(&session)));
}

/// #4199, #3797: the spaces typed into a paragraph are all there after saving and reopening.
#[test]
fn edited_text_keeps_every_space_typed_through_save_and_reopen() {
    let (mut s, id) = session_with(1);
    let typed = "  A   b  c";
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: typed.into(), style: pdfcraft_edit::BlockStyle::default() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("saved.pdf", None, saved, None).unwrap();
    let blocks = s.get(reopened).unwrap().editor.as_ref().unwrap().text_blocks(0).unwrap();
    assert_eq!(blocks[0].editor_text, typed);
}

/// A new document, or one restored from recovery, has never been written where it belongs: undo
/// back to the bytes it was opened from must not make it clean, or closing it would discard the
/// whole document without asking. Saving makes it clean; Revert of a saved one too.
#[test]
fn undo_never_makes_an_unsaved_document_clean() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let new = s.open_new("Combined.pdf", Arc::new(fixture(2))).unwrap();
    s.apply(new, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    s.undo(new).unwrap();
    assert!(s.get(new).unwrap().dirty, "a new document is unsaved until it is saved");
    assert_eq!(s.autosave_snapshots().len(), 1, "and still needs recovery");
    s.revert(new).unwrap();
    assert!(s.get(new).unwrap().dirty, "Revert goes back to the created bytes, which aren't saved anywhere");

    let recovered = s.open("Report.pdf", None, Arc::new(fixture(2)), None).unwrap();
    s.mark_recovered(recovered, Some("Report.pdf".into()));
    s.apply(recovered, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    s.undo(recovered).unwrap();
    assert!(s.get(recovered).unwrap().dirty, "the recovered work was never saved");

    let saved = s.save_bytes(new).unwrap();
    s.mark_saved(new, saved, Some("Combined.pdf".into())).unwrap();
    assert!(!s.get(new).unwrap().dirty);
    s.apply(new, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    s.undo(new).unwrap();
    assert!(!s.get(new).unwrap().dirty, "once saved, undo back to the saved state is clean");
}

#[test]
fn undo_and_redo_roll_back_on_a_refresh_panic_or_error() {
    let (mut s, id) = session_with(1);
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let (bytes_90, gen_90) = {
        let d = s.get(id).unwrap();
        (d.bytes.clone(), d.generation)
    };

    // 1. A panic during undo's refresh catches the unwind and rolls back cos, undo/redo, info, and working bytes.
    panic_on_refresh(1);
    let err = s.undo(id).unwrap_err();
    assert_eq!(err, EditError::Reopen("injected refresh panic".into()));
    {
        let d = s.get(id).unwrap();
        let ed = d.editor.as_ref().unwrap();
        assert_eq!(d.info.pages[0].rotation, 90);
        assert_eq!(d.bytes, bytes_90);
        assert_eq!(d.generation, gen_90);
        assert!(d.dirty);
        assert!(ed.cos.is_modified());
        assert_eq!(ed.undo.len(), 1);
        assert!(ed.redo.is_empty());
    }
    let _ = shown(&s, id, 0);

    // 2. If the rollback refresh also panics, `show_released` restores the renderer so the page still draws.
    panic_on_refresh(2);
    assert_eq!(s.undo(id), Err(EditError::Reopen("injected refresh panic".into())));
    assert_eq!(s.get(id).unwrap().editor.as_ref().unwrap().undo.len(), 1);
    let _ = shown(&s, id, 0);

    // Undo now succeeds cleanly.
    assert_eq!(s.undo(id).unwrap().as_str(), "Rotate page");
    let (bytes_0, gen_0) = {
        let d = s.get(id).unwrap();
        assert_eq!(d.info.pages[0].rotation, 0);
        assert!(!d.dirty);
        (d.bytes.clone(), d.generation)
    };

    // 3. A panic during redo's refresh catches the unwind and rolls back to the undone state.
    panic_on_refresh(1);
    let err = s.redo(id).unwrap_err();
    assert_eq!(err, EditError::Reopen("injected refresh panic".into()));
    {
        let d = s.get(id).unwrap();
        let ed = d.editor.as_ref().unwrap();
        assert_eq!(d.info.pages[0].rotation, 0);
        assert_eq!(d.bytes, bytes_0);
        assert_eq!(d.generation, gen_0);
        assert!(!d.dirty);
        assert!(!ed.cos.is_modified());
        assert!(ed.undo.is_empty());
        assert_eq!(ed.redo.len(), 1);
    }
    let _ = shown(&s, id, 0);

    // 4. If redo's rollback refresh also panics, `show_released` restores the renderer.
    panic_on_refresh(2);
    assert_eq!(s.redo(id), Err(EditError::Reopen("injected refresh panic".into())));
    assert_eq!(s.get(id).unwrap().editor.as_ref().unwrap().redo.len(), 1);
    let _ = shown(&s, id, 0);

    // Redo now succeeds cleanly.
    assert_eq!(s.redo(id).unwrap().as_str(), "Rotate page");
    assert_eq!(s.get(id).unwrap().info.pages[0].rotation, 90);

    // 5. Metadata scope: a panic after `doc.info.title` was written rolls back `info.title` and keeps `edit_notices`.
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Draft".into() }).unwrap();
    let notice = TextNotice { field: None, page: 0, ch: '☺', redrawn_elsewhere: false };
    s.doc_mut(id).unwrap().edit_notices.push(notice.clone());
    panic_on_refresh(1);
    assert_eq!(s.undo(id), Err(EditError::Reopen("injected refresh panic".into())));
    assert_eq!(s.get(id).unwrap().info.title.as_deref(), Some("Draft"));
    assert_eq!(s.get(id).unwrap().edit_notices, [notice]);
    assert_eq!(s.undo(id).unwrap().as_str(), "Change Title");
    assert_eq!(s.get(id).unwrap().info.title, None);
    assert!(s.get(id).unwrap().edit_notices.is_empty());

    panic_on_refresh(1);
    assert_eq!(s.redo(id), Err(EditError::Reopen("injected refresh panic".into())));
    assert_eq!(s.get(id).unwrap().info.title, None);
    assert_eq!(s.redo(id).unwrap().as_str(), "Change Title");
    assert_eq!(s.get(id).unwrap().info.title.as_deref(), Some("Draft"));

    // 6. Protection keys: undo/redo rollback restores `editor.keys` and `doc.password` before re-inspecting.
    let protect = Protection { open_password: Some("u".into()), permissions_password: Some("o".into()), ..Protection::default() };
    s.apply(id, Edit::Protect(protect)).unwrap();
    assert!(s.get(id).unwrap().info.encrypted);
    panic_on_refresh(1);
    assert_eq!(s.undo(id), Err(EditError::Reopen("injected refresh panic".into())));
    assert!(s.get(id).unwrap().info.encrypted);
    assert_eq!(s.get(id).unwrap().password.as_deref(), Some("u"));
    assert_eq!(s.undo(id).unwrap().as_str(), "Protect with password");
    assert!(!s.get(id).unwrap().info.encrypted);
    assert_eq!(s.get(id).unwrap().password, None);

    panic_on_refresh(1);
    assert_eq!(s.redo(id), Err(EditError::Reopen("injected refresh panic".into())));
    assert!(!s.get(id).unwrap().info.encrypted);
    assert_eq!(s.get(id).unwrap().password, None);
    assert_eq!(s.redo(id).unwrap().as_str(), "Protect with password");
    assert!(s.get(id).unwrap().info.encrypted);
    assert_eq!(s.get(id).unwrap().password.as_deref(), Some("u"));
}

#[test]
fn a_bookmark_added_at_a_point_goes_there_through_undo_and_save() {
    use pdfcraft_render::DestView;
    let (mut s, id) = session_with(3);
    let view = |s: &Session, i: usize| s.get(id).unwrap().info.outline.get(i).map(|o| (o.page, o.view));
    let add = |s: &mut Session, title: &str, page: usize, left: Option<f64>, top: Option<f64>| {
        s.apply(id, Edit::AddBookmark { parent: vec![], index: usize::MAX, title: title.into(), page, left, top })
    };
    add(&mut s, "Plain", 0, None, None).unwrap();
    add(&mut s, "Section", 1, None, Some(500.5)).unwrap();
    add(&mut s, "Table", 2, Some(72.0), Some(300.0)).unwrap();
    // The page top (no position) is what it always was; the others carry their place.
    assert_eq!(view(&s, 0), Some((Some(0), DestView::Xyz { left: None, top: None, zoom: None })));
    assert_eq!(view(&s, 1), Some((Some(1), DestView::Xyz { left: None, top: Some(500.5), zoom: None })));
    assert_eq!(view(&s, 2), Some((Some(2), DestView::Xyz { left: Some(72.0), top: Some(300.0), zoom: None })));

    // A position that is not a number fails the edit, which then changes nothing.
    for (left, top) in [(Some(f64::NAN), None), (None, Some(f64::INFINITY))] {
        assert!(matches!(add(&mut s, "Bad", 0, left, top), Err(EditError::Bookmark(_))), "{left:?} {top:?}");
    }
    assert_eq!(s.get(id).unwrap().info.outline.len(), 3);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Add bookmark"));

    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().info.outline.len(), 2);
    s.redo(id).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    let id2 = again.open("again.pdf", None, saved, None).unwrap();
    let outline = &again.get(id2).unwrap().info.outline;
    assert_eq!(outline[2].view, DestView::Xyz { left: Some(72.0), top: Some(300.0), zoom: None });
    assert_eq!(outline[1].view, DestView::Xyz { left: None, top: Some(500.5), zoom: None });
}

/// #3470: a filled form whose writer left the appearances to the viewer (`/NeedAppearances`):
/// field A has an empty appearance, B none. The page as displayed (and printed) shows both
/// values, and a comment without an appearance beside them still gets its own; the file itself
/// is not changed, so saving writes it as it was.
#[test]
fn fields_left_to_the_viewer_are_displayed_from_their_values() {
    let pdf = |need: bool| {
        let flag = if need { "/NeedAppearances true " } else { "" };
        format!(
            "%PDF-1.7\n\
             1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Annots [6 0 R 7 0 R 10 0 R] >> endobj\n\
             10 0 obj << /Type /Annot /Subtype /Square /Rect [360 20 390 50] /C [1 0 0] /BS << /W 4 >> >> endobj\n\
             5 0 obj << /Fields [6 0 R 7 0 R] {flag}/DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 8 0 R >> >> >> endobj\n\
             6 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (a) /V (MMMMMMMM) /DA (/Helv 12 Tf 0 g) /Rect [50 120 350 140] /F 4 /P 3 0 R /AP << /N 9 0 R >> >> endobj\n\
             7 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (b) /V (MMMMMMMM) /DA (/Helv 12 Tf 0 g) /Rect [50 60 350 80] /F 4 /P 3 0 R >> endobj\n\
             8 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> endobj\n\
             9 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 300 20] /Length 4 >> stream\nq\nQ\nendstream endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF"
        )
        .into_bytes()
    };
    let red = |s: &Session, id: DocId| {
        let doc = s.get(id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.display.clone(), doc.config.clone());
        let page = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        page.rgba.as_chunks::<4>().0.iter().filter(|p| p[0] > 200 && p[1] < 80 && p[2] < 80).count()
    };
    // Dark pixels inside a field's box (user-space rectangle, page drawn at scale 1).
    let ink = |s: &Session, id: DocId, y0: u32, y1: u32| {
        let doc = s.get(id).unwrap();
        let mut r = pdfcraft_render::PageRenderer::new(doc.display.clone(), doc.config.clone());
        let page = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        let (px, _) = page.rgba.as_chunks::<4>();
        (200 - y1..200 - y0)
            .flat_map(|y| (55..345).map(move |x| (x, y)))
            .filter(|&(x, y)| px.get((y * page.width + x) as usize).is_some_and(|p| p[0] < 128))
            .count()
    };
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(pdf(true)), None).unwrap();
    let (a, b) = (ink(&s, id, 122, 138), ink(&s, id, 62, 78));
    assert!(a > 100 && b > 100, "both values are shown: a {a}, b {b}");
    assert!(red(&s, id) > 100, "the comment is drawn too");
    // The file is saved as it was: B still has no appearance, A its empty one.
    let saved = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    let fields = pdfcraft_forms::fields(&saved);
    let widget = |name: &str| saved.get(fields.iter().find(|f| f.name == name).unwrap().widgets[0].obj);
    assert!(widget("b").as_dict().is_some_and(|d| !d.contains(b"AP")), "B has no appearance");
    let n = widget("a").as_dict().and_then(|d| d.get(b"AP").and_then(|ap| saved.resolve(ap).as_dict().and_then(|ap| ap.reference(b"N")))).unwrap();
    let pdfcraft_cos::Object::Stream(n) = &*saved.get(n) else { panic!("A's appearance is a stream") };
    assert_eq!(n.decoded().unwrap().trim_ascii_end(), b"q\nQ", "A keeps its empty appearance");
    // Typing in one field redraws that field only, not the whole form, on each keystroke.
    let builds = |s: &Session| s.get(id).unwrap().editor.as_ref().unwrap().field_appearances.builds();
    let before = builds(&s);
    s.apply(id, Edit::SetFieldValue { name: "a".into(), value: pdfcraft_forms::FieldValue::Text("Typed".into()) }).unwrap();
    assert_eq!(builds(&s), before + 1);

    // Without the flag, the empty appearance is the file's own, and only B is drawn.
    let id = s.open("form.pdf", None, Arc::new(pdf(false)), None).unwrap();
    let (a, b) = (ink(&s, id, 122, 138), ink(&s, id, 62, 78));
    assert!(a == 0 && b > 100, "a {a}, b {b}");
}
/// Reproducible workload with 3,129 fields and 190 ordered custom calculations.
/// No external PDF assets are needed; only in-memory contributor-original objects.
fn synthetic_acroform(field_count: usize, calculation_count: usize) -> Vec<u8> {
    use pdfcraft_cos::{Dict, Object, PdfString};
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(6))).unwrap();
    let mut refs = Vec::new();
    let mut order = Vec::new();
    for i in 0..field_count {
        let name = match i {
            0 => "ranks".to_string(),
            1 => "trained".to_string(),
            2 => "bonus".to_string(),
            _ => format!("field{i}"),
        };
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Widget"));
        d.set(b"FT".to_vec(), Object::name(if i == 1 { "Btn" } else { "Tx" }));
        d.set(b"T".to_vec(), PdfString::text(&name));
        d.set(b"V".to_vec(), if i == 1 { Object::name("Off") } else { Object::String(PdfString::text("0")) });
        d.set(b"Rect".to_vec(), Object::Array([20, 20, 100, 40].map(Object::Int).to_vec()));
        d.set(b"P".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(4, 0)));
        if (2..calculation_count + 2).contains(&i) {
            let js = if i == 2 {
                "var r = Number(getField('ranks').value); event.value = r + (r > 0 && getField('trained').isBoxChecked(0) ? 3 : 0);".to_string()
            } else {
                format!("event.value = Number(getField('{}').value) + 1;", if i == 3 { "bonus".to_string() } else { format!("field{}", i - 1) })
            };
            let mut a = Dict::new();
            a.set(b"S".to_vec(), Object::name("JavaScript"));
            a.set(b"JS".to_vec(), PdfString::text(&js));
            let mut aa = Dict::new();
            aa.set(b"C".to_vec(), Object::Dict(a));
            d.set(b"AA".to_vec(), Object::Dict(aa));
        }
        let r = doc.add(Object::Dict(d));
        refs.push(Object::Ref(r));
        if (2..calculation_count + 2).contains(&i) {
            order.push(Object::Ref(r));
        }
    }
    doc.update_dict(pdfcraft_cos::ObjRef::new(4, 0), |d| d.set(b"Annots".to_vec(), Object::Array(refs.clone()))).unwrap();
    let mut af = Dict::new();
    af.set(b"Fields".to_vec(), Object::Array(refs));
    af.set(b"CO".to_vec(), Object::Array(order));
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"AcroForm".to_vec(), Object::Dict(af))).unwrap();
    pdfcraft_cos::write_incremental(&doc, &Default::default()).unwrap()
}

#[test]
#[ignore = "manual performance measurement: 3,129 fields, 190 scripts"]
fn acroform_interaction_benchmark() {
    let bytes = Arc::new(synthetic_acroform(3129, 190));
    // Optional local export for comparing CLI build profiles. Never overwrite an input.
    if let Some(path) = std::env::var_os("PDFCRAFT_BENCH_EXPORT") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).unwrap();
        file.write_all(&bytes).unwrap();
    }
    let mut s = Session::new();
    let id = s.open("synthetic-large-form.pdf", None, bytes, None).unwrap();
    let value = |s: &Session, name: &str| s.get(id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone();
    for (name, v) in [
        ("ranks", FieldValue::Text("1".into())),
        ("trained", FieldValue::Check(true)),
        ("ranks", FieldValue::Text("2".into())),
        ("trained", FieldValue::Check(false)),
        ("ranks", FieldValue::Text("0".into())),
    ] {
        let started = std::time::Instant::now();
        s.apply(id, Edit::SetFieldValue { name: name.into(), value: v }).unwrap();
        eprintln!("BENCH {name}: {:.3} ms", started.elapsed().as_secs_f64() * 1000.0);
        assert!(s.take_js_output(id).errors.is_empty());
    }
    assert_eq!(value(&s, "bonus"), ["0"]);
    assert_eq!(value(&s, "field191"), ["189"]);
    assert_eq!(value(&s, "field3128"), ["0"]);
}

#[test]
fn checkbox_and_rank_changes_recalculate_ordered_dependents() {
    let mut s = Session::new();
    let id = s.open("skills.pdf", None, Arc::new(synthetic_acroform(8, 4)), None).unwrap();
    for (name, input, bonus) in [
        ("trained", FieldValue::Check(true), "0"),
        ("ranks", FieldValue::Text("1".into()), "4"),
        ("trained", FieldValue::Check(false), "1"),
        ("ranks", FieldValue::Text("2".into()), "2"),
        ("trained", FieldValue::Check(true), "5"),
        ("ranks", FieldValue::Text("0".into()), "0"),
    ] {
        s.apply(id, Edit::SetFieldValue { name: name.into(), value: input }).unwrap();
        let fields = &s.get(id).unwrap().form;
        let value = |name: &str| fields.iter().find(|f| f.name == name).unwrap().value.clone();
        assert_eq!(value("bonus"), [bonus]);
        assert_eq!(value("field5"), [(bonus.parse::<u32>().unwrap() + 3).to_string()]);
        assert_eq!(value("field7"), ["0"]);
        assert!(s.take_js_output(id).errors.is_empty());
    }
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().form.iter().find(|f| f.name == "bonus").unwrap().value, ["5"]);
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("saved.pdf", None, saved, None).unwrap();
    assert_eq!(s.get(reopened).unwrap().form.iter().find(|f| f.name == "bonus").unwrap().value, ["5"]);
    let appearance = |s: &Session| {
        let doc = s.get(id).unwrap();
        let widget = doc.form.iter().find(|f| f.name == "bonus").unwrap().widgets.first().unwrap().obj;
        doc.editor.as_ref().unwrap().cos.get(widget).as_dict().unwrap().get(b"AP").cloned()
    };
    let before = appearance(&s);
    assert!(before.is_some());
    s.apply(id, Edit::SetFieldValue { name: "field7".into(), value: FieldValue::Text("9".into()) }).unwrap();
    assert_eq!(appearance(&s), before, "unchanged calculated values keep their appearance streams");
}

#[test]
fn dynamic_calculation_reads_and_side_effects_reach_later_fields() {
    use pdfcraft_cos::{Dict, Object, PdfString};
    let mut cos = pdfcraft_cos::Document::open(Arc::new(synthetic_acroform(8, 4))).unwrap();
    let fields = pdfcraft_forms::fields(&cos);
    let bonus = fields.iter().find(|f| f.name == "bonus").unwrap();
    let selector = fields.iter().find(|f| f.name == "field7").unwrap();
    cos.update_dict(selector.obj, |d| d.set(b"V".to_vec(), PdfString::text("ranks"))).unwrap();
    let mut a = Dict::new();
    a.set(b"S".to_vec(), Object::name("JavaScript"));
    a.set(b"JS".to_vec(), PdfString::text(
        "var key = getField('field7').value; var v = Number(getField(key).value); getField('field6').value = v * 2; getField('field6').readonly = true; event.value = v + 1;"
    ));
    let mut aa = Dict::new();
    aa.set(b"C".to_vec(), Object::Dict(a));
    cos.update_dict(bonus.obj, |d| d.set(b"AA".to_vec(), Object::Dict(aa))).unwrap();
    let bytes = pdfcraft_cos::write_incremental(&cos, &Default::default()).unwrap();
    let mut s = Session::new();
    let id = s.open("dynamic.pdf", None, Arc::new(bytes), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "ranks".into(), value: FieldValue::Text("4".into()) }).unwrap();
    let fields = &s.get(id).unwrap().form;
    let field = |name: &str| fields.iter().find(|f| f.name == name).unwrap();
    assert_eq!(field("bonus").value, ["5"]);
    assert_eq!(field("field5").value, ["8"], "later calculations see the ordered result");
    assert_eq!(field("field6").value, ["8"], "side-effect value applied");
    assert!(field("field6").read_only(), "side-effect property applied");
    assert_eq!(field("field7").value, ["ranks"]);
    assert!(s.take_js_output(id).errors.is_empty());
}

#[test]
fn scripted_display_changes_survive_warm_cache_and_new_edit() {
    use pdfcraft_forms::{FieldEvent, Scripts};
    let mut cos = pdfcraft_cos::Document::open(Arc::new(synthetic_acroform(2, 0))).unwrap();
    for (display, flags) in [(1, 6), (3, 36), (2, 0), (0, 4)] {
        let fields = pdfcraft_forms::fields(&cos);
        let target = fields.iter().find(|f| f.name == "ranks").unwrap();
        let other = fields.iter().find(|f| f.name == "trained").unwrap();
        let old_display = crate::js::field_state(&cos, other).display;
        let mut runner = crate::js::JsRunner::new(&cos, "synthetic.pdf");
        // Populate the display cache before a script writes the same field.
        let warm = runner.run(FieldEvent::Calculate, "event.value = getField('trained').display;", target, "", &fields);
        assert_eq!(warm.value, old_display.to_string());
        let result = runner.run(
            FieldEvent::Calculate,
            &format!("getField('trained').display = {display}; event.value = getField('trained').display;"),
            target,
            "",
            &fields,
        );
        assert_eq!(result.value, display.to_string());
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes.first().unwrap().display, Some(display));
        pdfcraft_forms::apply_script_changes(&mut cos, &result.changes, &mut pdfcraft_forms::NoScripts).unwrap();
        assert_eq!(cos.get(other.widgets.first().unwrap().obj).as_dict().unwrap().int(b"F").unwrap() & 38, flags);
        // Within an edit the original implementation reads an immutable pre-edit COS snapshot.
        let same_edit = runner.run(FieldEvent::Calculate, "event.value = getField('trained').display;", target, "", &fields);
        assert_eq!(same_edit.value, old_display.to_string());
        // The next edit must rebuild its cache from the newly written annotation flags.
        let mut next = crate::js::JsRunner::new(&cos, "synthetic.pdf");
        let read = next.run(FieldEvent::Calculate, "event.value = getField('trained').display;", target, "", &pdfcraft_forms::fields(&cos));
        assert_eq!(read.value, display.to_string());
        assert!(read.changes.is_empty());
        assert!(runner.output.errors.is_empty());
        assert!(next.output.errors.is_empty());
    }
}

/// #2992: a page coloured with Edit a PDF ▸ Background exports to Word with that page colour,
/// behind its text.
#[test]
fn exporting_to_word_keeps_the_page_background() {
    let mut s = Session::new();
    let text = s.create_from_text("t", "Blue text on teal").unwrap();
    let id = s.open("t.pdf", None, text, None).unwrap();
    assert_eq!(s.get(id).unwrap().export_pages()[0].background, None, "no background yet");
    s.apply(
        id,
        Edit::AddBackground {
            pages: vec![0],
            settings: Background { color: [0.0, 0.5, 0.5], opacity: 1.0, ..Background::default() },
            replace: false,
            file: None,
        },
    )
    .unwrap();
    let d = s.get(id).unwrap();
    let page = &d.export_pages()[0];
    assert!(page.background.is_some_and(|c| c.iter().zip([0.0, 0.5, 0.5]).all(|(a, b)| (a - b).abs() < 1e-6)), "{:?}", page.background);
    assert!(page.blocks.iter().any(|b| b.text.contains("Blue text")), "the text is still there");
    let docx = d.export_office(compare::OfficeFormat::Docx);
    assert!(docx.windows(b"word/settings.xml".len()).any(|w| w == b"word/settings.xml"), "Word is told to show the page colour");
}

#[test]
fn exporting_office_keeps_dotted_leader_entries_as_paragraphs() {
    let mut content = String::new();
    for (title, page, y) in [("Art.1. Scope", "3", 200), ("Art.2. Eligibility", "7", 180), ("Art.3. Application", "12", 160)] {
        content.push_str(&format!(
            "BT /F1 12 Tf 72 {y} Td ({title}) Tj ET \
             BT /F1 12 Tf 250 {y} Td (....................) Tj ET \
             BT /F1 12 Tf 500 {y} Td ({page}) Tj ET\n"
        ));
    }
    let pdf = one_line_page("/Font << /F1 4 0 R >>", &content);
    let mut session = Session::new();
    let id = session.open("contents.pdf", None, Arc::new(pdf), None).unwrap();
    let doc = session.get(id).unwrap();
    let html = String::from_utf8(doc.export_office(compare::OfficeFormat::Html)).unwrap();
    assert!(!html.contains("<table>"), "contents entries are paragraphs: {html}");
    let entries = ["Art.1. Scope .................... 3", "Art.2. Eligibility .................... 7", "Art.3. Application .................... 12"];
    let positions: Vec<_> = entries.iter().map(|entry| html.find(&format!("<p>{entry}</p>")).expect("one paragraph per entry")).collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "reading order: {html}");
}

/// A date typed in another of an XFA date field's edit patterns is kept in its first: yymmdd and
/// yyyymd are rewritten as yyyymmdd, a date none of them reads is refused, and the data holds the date.
#[test]
fn a_date_typed_by_another_of_an_xfa_fields_edit_patterns_is_kept_in_its_first() {
    let mut s = Session::new();
    let id = s
        .open("dates.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::date_alternatives_template())), None)
        .expect("opens");
    let value = |s: &Session| s.get(id).unwrap().form.iter().find(|f| f.name == "leave").unwrap().value.clone();
    s.apply(id, Edit::SetFieldValue { name: "leave".into(), value: FieldValue::Text("261018".into()) }).expect("yymmdd is one of its patterns");
    assert_eq!(value(&s), ["20261018"]);
    s.apply(id, Edit::SetFieldValue { name: "leave".into(), value: FieldValue::Text("2026118".into()) }).expect("so is yyyymd");
    assert_eq!(value(&s), ["20261108"]);
    let refused = s.apply(id, Edit::SetFieldValue { name: "leave".into(), value: FieldValue::Text("2618".into()) });
    assert!(refused.is_err_and(|e| e.to_string().contains("yyyymmdd")), "none of its patterns reads it");
    assert_eq!(value(&s), ["20261108"]);
    // The data holds the date.
    let cos = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    let data = pdfcraft_xfa::parse_datasets(&pdfcraft_xfa::read_packets(&cos).unwrap().unwrap().xdp).expect("a datasets packet");
    assert_eq!(data.text_at(&pdfcraft_xfa::som_to_path("form[0].leave[0]")), Some("2026-11-08"));
}

/// A date the field's own scripts refuse is not tried again in another of an XFA date field's
/// patterns: only a date its format refuses is, which happens before any script runs, so the
/// scripts run once. This validate script counts its runs in `name` and accepts on its second.
#[test]
fn a_date_its_validate_script_refuses_is_not_tried_again() {
    let mut s = Session::new();
    let id = s
        .open("dates.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::date_alternatives_template())), None)
        .expect("opens");
    let script = Some("var n = Number(getField('name').value || 0) + 1; getField('name').value = String(n); event.rc = n >= 2;".to_string());
    s.apply(id, Edit::SetFieldScript { name: "leave".into(), event: "validate".into(), script }).unwrap();
    let value = |s: &Session, n: &str| s.get(id).unwrap().form.iter().find(|f| f.name == n).unwrap().value.clone();
    // Its format reads 18OCT26, and so does its DDMMMYY pattern.
    let refused = s.apply(id, Edit::SetFieldValue { name: "leave".into(), value: FieldValue::Text("18OCT26".into()) });
    assert!(refused.is_err(), "the validate script refused it, once");
    assert!(value(&s, "name").is_empty(), "nothing the refused run changed is kept: {:?}", value(&s, "name"));
}

/// An XFA button's click script may show the attachments panel; a text field's exit script
/// asking the same is noted on the console and ignored.
#[test]
fn an_xfa_button_may_show_a_side_panel_and_other_events_may_not() {
    let xdp = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <field name="files" w="2in" h="0.3in"><ui><button/></ui><caption><value><text>Attachments</text></value></caption>
  <event activity="click"><script contentType="application/x-javascript">app.execMenuItem("ShowHideFileAttachment");</script></event></field>
 <field name="note" w="2in" h="0.3in"><ui><textEdit/></ui>
  <event activity="exit"><script contentType="application/x-javascript">app.execMenuItem("ShowHideFileAttachment");</script></event></field>
</subform>
</template>
</xdp:xdp>
"##;
    let mut s = Session::new();
    let id = s.open("files.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(xdp)), None).expect("opens");
    s.take_js_output(id);
    s.apply(id, Edit::XfaEvent { som: "form[0].files[0]".into(), activity: "click".into() }).unwrap();
    let out = s.take_js_output(id);
    assert_eq!(out.requests, [pdfcraft_js::Request::ShowPanel(pdfcraft_js::Panel::Attachments)], "{out:?}");
    s.apply(id, Edit::SetFieldValue { name: "note".into(), value: FieldValue::Text("x".into()) }).unwrap();
    let out = s.take_js_output(id);
    assert!(out.requests.is_empty(), "only a click shows a panel: {out:?}");
    assert!(out.console.iter().any(|c| c.contains("show a side panel")), "{:?}", out.console);
}

/// A validate script that asks for Save As and a side panel gets neither, with a console note
/// for each; a button's script may still open Save As.
#[test]
fn a_fields_own_script_may_not_save_as_or_show_a_panel() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("order.pdf", None, Arc::new(scripted_form()), None).unwrap();
    s.apply(
        id,
        Edit::SetFieldScript {
            name: "qty".into(),
            event: "validate".into(),
            script: Some("app.execMenuItem('SaveAs'); app.execMenuItem('ShowHideFileAttachment');".into()),
        },
    )
    .unwrap();
    s.take_js_output(id);
    s.apply(id, Edit::SetFieldValue { name: "qty".into(), value: FieldValue::Text("4".into()) }).unwrap();
    let out = s.take_js_output(id);
    assert!(out.requests.is_empty(), "a validate script runs unasked: {out:?}");
    assert_eq!(out.console.iter().filter(|c| c.contains("only a button may")).count(), 2, "{:?}", out.console);
    // A button's script may.
    let o = s.run_javascript(id, "app.execMenuItem('SaveAs');", Some("go")).unwrap();
    assert_eq!(o.requests, [js::Request::SaveAs]);
}

/// A page whose forms go past the reading limit and whose picture goes past the image budget is
/// listed once among the truncated pages.
#[test]
fn a_page_truncated_for_forms_and_pictures_is_listed_once() {
    let depth = compare::MAX_FORM_DEPTH + 1;
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".into(),
        String::new(),
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 4 >>\nstream\n\0\x20\x40\x7f\nendstream".into(),
        "<< /Length 34 >>\nstream\nq 10 0 0 10 0 0 cm /Im Do Q /D1 Do\nendstream".into(),
    ];
    // Forms D1..D{depth}, objects 6 onwards, each drawing the next.
    let xobjects: String = (1..=depth).map(|i| format!("/D{i} {} 0 R ", 5 + i)).collect();
    let resources = format!("<< /XObject << /Im 4 0 R {xobjects}>> >>");
    for i in 1..=depth {
        let body = if i == depth { "q Q".to_string() } else { format!("/D{} Do", i + 1) };
        objs.push(format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources {resources} /Length {} >>\nstream\n{body}\nendstream",
            body.len()
        ));
    }
    objs[2] = format!("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources {resources} >>");
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut s = Session::new();
    let id = s.open("both.pdf", None, Arc::new(bytes), None).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.export_pages_checked(false, 0).1, [0], "the forms alone truncate the page");
    let (exported, _) = doc.export_pages_checked(true, 1 << 20);
    assert_eq!(exported[0].images.len(), 1, "the picture exports within a sufficient budget");
    let (pages, truncated) = doc.export_pages_checked(true, 0);
    assert!(pages[0].images.is_empty(), "the picture is past the budget");
    assert_eq!(truncated, [0]);
}

/// ISO 32000-2 Table 192, 12.7.5.2.3 and Table 228 (Comb): a check box, a push button, a comb
/// field the file leaves to the viewer. The page as displayed and printed as an image shows the
/// check box's caption symbol in its text colour, the button's grey face and the comb's cells in
/// the border colour, and saving writes the file as it was.
#[test]
fn buttons_and_combs_left_to_the_viewer_are_displayed() {
    let pdf = b"%PDF-1.7\n\
        1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Annots [6 0 R 7 0 R 8 0 R] >> endobj\n\
        4 0 obj << /Fields [6 0 R 7 0 R 8 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> endobj\n\
        5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> endobj\n\
        6 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /T (k) /V /Yes /AS /Yes /DA (/ZaDb 0 Tf 1 0 0 rg) /MK << /CA (8) /BC [0 0 0] >> /Rect [20 150 60 190] /F 4 /P 3 0 R >> endobj\n\
        7 0 obj << /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (o) /DA (/Helv 12 Tf 0 g) /MK << /CA (Press) /BG [0.8 0.8 0.8] /BC [0 0 0] >> /BS << /W 2 /S /B >> /Rect [100 150 220 190] /F 4 /P 3 0 R >> endobj\n\
        8 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /Ff 16777216 /MaxLen 4 /T (d) /V (AB) /DA (/Helv 12 Tf 0 g) /MK << /BC [0 0 1] >> /Rect [20 80 220 110] /F 4 /P 3 0 R >> endobj\n\
        trailer << /Root 1 0 R >>\n%%EOF"
        .to_vec();
    let render = |bytes: Arc<Vec<u8>>| {
        let mut r = pdfcraft_render::PageRenderer::new(bytes, pdfcraft_render::RenderConfig::default());
        r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() })
    };
    // Pixels of the page (drawn at scale 1, 400 x 200) in user-space box x0..x1, y0..y1 that `keep`.
    let count = |page: &pdfcraft_render::RenderedPage, (x0, y0, x1, y1): (u32, u32, u32, u32), keep: &dyn Fn(&[u8; 4]) -> bool| {
        let (px, _) = page.rgba.as_chunks::<4>();
        (200 - y1..200 - y0)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| px.get((y * page.width + x) as usize).is_some_and(keep))
            .count()
    };
    let red = |p: &[u8; 4]| p[0] > 200 && p[1] < 80 && p[2] < 80;
    let grey = |p: &[u8; 4]| (180..=230).contains(&p[0]) && p[0] == p[1] && p[1] == p[2];
    // Bluish: a 1 pt line on a pixel edge is half blue on each side.
    let blue = |p: &[u8; 4]| p[2] > 200 && p[0] < 180 && p[1] < 180 && p[0] + 40 < p[2];
    let mut s = Session::new();
    let id = s.open("form.pdf", None, Arc::new(pdf), None).unwrap();
    let shown = render(s.get(id).unwrap().display.clone());
    assert!(count(&shown, (24, 154, 56, 186), &red) > 60, "the check box's red cross");
    assert!(count(&shown, (104, 154, 216, 186), &grey) > 1000, "the push button's grey face");
    // The comb's three inner lines at x = 70, 120 and 170, between its cells.
    for x in [70, 120, 170] {
        assert!(count(&shown, (x - 1, 84, x + 1, 106), &blue) >= 20, "a cell line at {x}");
    }
    assert_eq!(count(&shown, (95, 84, 97, 106), &blue), 0, "no line inside a cell");
    // Print as Image: the page as a picture, at 72 dpi so it is the size of the page here.
    let raster = pdfcraft_print::Raster { dpi: 72.0, grayscale: false };
    let pictures = crate::print_raster::rasterize(s.get(id).unwrap(), &[0], &raster, pdfcraft_print::Content::Document).unwrap();
    let printed = render(Arc::new(pictures));
    assert!(count(&printed, (24, 154, 56, 186), &red) > 60, "the check box prints");
    assert!(count(&printed, (104, 154, 216, 186), &grey) > 1000, "and the push button");
    // The file is saved as it was: no widget has an appearance.
    let saved = pdfcraft_cos::Document::open(s.save_bytes(id).unwrap()).unwrap();
    for f in pdfcraft_forms::fields(&saved) {
        assert!(saved.get(f.widgets[0].obj).as_dict().is_some_and(|d| !d.contains(b"AP")), "{} has no appearance", f.name);
    }
}

/// ISO 32000-2 Tables 166 and 183, drawn on the page: a square without an appearance whose
/// `/Border [0 0 3 [6 3]]` names its width before a dash array is stroked 3 pt wide and dashed
/// (it was 1 pt and solid), and a caret with `/Sy /P` shows a ¶ to its right.
#[test]
fn legacy_border_dashes_and_the_caret_paragraph_symbol_are_drawn() {
    let original = Arc::new(pdf_with_annots(&[
        b"<< /Type /Annot /Subtype /Square /Rect [20 180 120 280] /Border [0 0 3 [6 3]] /C [1 0 0] >>",
        b"<< /Type /Annot /Subtype /Caret /Rect [20 20 80 80] /Sy /P /C [0 0 1] >>",
    ]));
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("noap.pdf", None, original, None).expect("opens");
    let p = shown(&s, id, 0);
    let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..4].to_vec();
    // y down: the bottom edge's stroke is centred 1.5 pt inside /Rect, on row 118.
    let row: Vec<Vec<u8>> = (24..116).map(|x| px(x, 118)).collect();
    let red = row.iter().filter(|c| c[0] > 200 && c[1] < 80).count();
    let white = row.iter().filter(|c| c[1] > 200).count();
    assert!(red > 40 && white > 15, "3 pt and dashed: {red} red, {white} white");
    // The ¶ sits in the right part of the caret's /Rect, where the caret no longer reaches.
    let blue = (250..270).flat_map(|y| (68..80).map(move |x| (x, y))).filter(|&(x, y)| {
        let c = px(x, y);
        c[2] > 150 && c[0] < 100
    });
    assert!(blue.count() > 20, "the pilcrow is drawn beside the caret");
}

/// A one-page 200 × 300 document whose page lists `annots` (objects 4, 5, …).
fn pdf_with_annots(annots: &[&[u8]]) -> Vec<u8> {
    let list: String = (0..annots.len()).map(|i| format!("{} 0 R ", i + 4)).collect();
    let page = format!("<< /Type /Page /Parent 2 0 R /Annots [{list}] >>");
    let mut objs: Vec<&[u8]> =
        vec![b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>", page.as_bytes()];
    objs.extend_from_slice(annots);
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(o);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    bytes
}
