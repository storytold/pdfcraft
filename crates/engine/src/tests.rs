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
    let layer = |n: u32, visible: bool| Layer { id: (n, 0), name: format!("L{n}"), visible };
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
    s.apply(id, Edit::DuplicatePages { pages: vec![0] }).unwrap();
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
    // Unencrypted documents allow everything.
    let (s, plain) = session_with(1);
    assert!(s.get(plain).unwrap().check_copying().is_ok() && extracting.iter().all(|c| enabled(&s, plain, c)));
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

#[test]
fn autosave_snapshots_only_changed_documents() {
    let (mut s, id) = session_with(2);
    assert!(s.autosave_snapshots().is_empty(), "clean documents are not snapshotted");
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let snaps = s.autosave_snapshots();
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
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 0, title: "Start".into(), page: 0 }).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 1, title: "End".into(), page: 2 }).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![1], index: 0, title: "Détail".into(), page: 1 }).unwrap();
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

/// One 612×300 page with the content stream `body`. Object 4 is Helvetica; `resources` is what
/// the page's /Resources holds.
fn one_line_page(resources: &str, body: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 300] >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /Resources << {resources} >> /Contents 5 0 R >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
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

/// Redaction removes text by the positions its own interpreter works out. Here the font and size
/// come from an ExtGState /Font entry (`gs`), which the renderer honours and the interpreter does
/// not model, so it places no glyph under the mark the user drew over the word as displayed: it
/// used to report success and leave the word in the file. The renderer is asked now, and the edit
/// is refused.
#[test]
fn redaction_is_refused_when_the_page_still_shows_text_under_a_mark() {
    let line = "Public words then SECRET42 and more";
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let by_gs = one_line_page("/ExtGState << /GS1 << /Type /ExtGState /Font [4 0 R 16] >> >>", &format!("BT /GS1 gs 20 150 Td ({line}) Tj ET"));
    let id = s.open("gs.pdf", None, Arc::new(by_gs), None).unwrap();
    mark_shown_text(&mut s, id, "SECRET42");
    let refused = s.apply(id, Edit::ApplyRedactions { pages: None });
    assert!(matches!(refused, Err(EditError::Redact(pdfcraft_redact::RedactError::Residue(n))) if n > 0), "{refused:?}");
    // Nothing was changed: the mark is still pending and the page still shows the line.
    assert_eq!(s.get(id).unwrap().redaction_marks(), 1);
    assert_eq!(page_texts(&s, id), [line]);

    // The same line with its font set by `Tf`: the mark is applied and the word is gone.
    let by_tf = one_line_page("/Font << /F1 4 0 R >>", &format!("BT /F1 16 Tf 20 150 Td ({line}) Tj ET"));
    let id = s.open("tf.pdf", None, Arc::new(by_tf), None).unwrap();
    mark_shown_text(&mut s, id, "SECRET42");
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let shown = page_texts(&s, id).swap_remove(0);
    assert!(shown.starts_with("Public words then") && shown.ends_with("and more") && !shown.contains("SECRET42"), "{shown}");
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
    s.apply(id, Edit::DuplicatePages { pages: vec![1] }).unwrap();
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
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 0, title: "Intro".into(), page: 0 }).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 1, title: "Results".into(), page: 3 }).unwrap();
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

#[test]
fn recognize_text_makes_a_scanned_page_searchable() {
    if !ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let text = s.create_from_text("t", "The quick brown fox jumps over the lazy dog.").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    // A typical 300 DPI scan keeps the small leading glyph in "quick" legible to OCR.
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 300.0).unwrap();
    let scan = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
    let id = s.open("scan.pdf", None, scan, None).unwrap();
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

/// A form whose scripts are custom JavaScript: total = price × qty (calculate, through a
/// document-level function), shown with a custom format; qty is validated; a button script.
fn scripted_form() -> Vec<u8> {
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
        "<< /S /JavaScript /JS (function times\\(a, b\\) { console.println('times'); return a * b; }) >>".to_string(),
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

#[test]
fn xfa_scripts_of_one_event_see_the_form_as_the_engine_holds_it() {
    // In document order: `a` holds 1; `b`'s calculate writes `a` then fails; `c`'s calculate
    // reads `a`. A failed script's half-done changes reach nobody. And `d` sets a choice list
    // by its shown text: the next script reads the saved value the engine kept.
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
    assert_eq!(value(&s, "a"), vec!["1".to_string()], "the failed script's write reached nothing");
    assert_eq!(value(&s, "c"), vec!["2".to_string()]);
    assert_eq!(value(&s, "country"), vec!["CA".to_string()], "shown text mapped to the saved value");
    assert_eq!(value(&s, "e"), vec!["CA".to_string()], "the next script reads the saved value");
    assert_eq!(value(&s, "summary"), vec!["Country: CA".to_string()]);
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
    let stream = |doc: &pdfcraft_cos::Document, n: u32| match &*doc.get(pdfcraft_cos::ObjRef::new(n, 0)) {
        pdfcraft_cos::Object::Stream(s) => s.clone(),
        other => panic!("object {n} is {other:?}"),
    };
    let figure = stream(&doc, 4);
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
    assert_eq!(stream(&doc, 4).dict.get(b"VendorKey").and_then(pdfcraft_cos::Object::as_int), Some(7));
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
        assert!(docx.windows(jpeg.len()).any(|w| w == jpeg.as_slice()), "the JPEG is stored unchanged (overlap {overlaps})");
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

/// Convert colours re-encodes pictures too (RGB → gray, the JPEG staying JPEG) and is one undo
/// step; a document with nothing to convert refuses rather than leaving an empty step.
#[test]
fn converting_colours_recolours_pictures() {
    let mut s = Session::new();
    let mut px = image::RgbImage::new(8, 8);
    for p in px.pixels_mut() {
        *p = image::Rgb([200, 40, 40]);
    }
    let mut jpg = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(px).write_to(&mut jpg, image::ImageFormat::Jpeg).unwrap();
    let pic = s.create_from_images(&[("p.jpg".into(), jpg.into_inner())]).unwrap();
    let id = s.open("p.pdf", None, pic, None).unwrap();
    // The picture's colour space and filter, read from the document's current bytes.
    let picture = |s: &Session| -> (String, String) {
        let doc = pdfcraft_cos::Document::open(s.get(id).unwrap().bytes.clone()).unwrap();
        doc.scan_objects()
            .find_map(|(_, o)| match &*o {
                pdfcraft_cos::Object::Stream(st) if st.dict.name(b"Subtype") == Some(b"Image") => Some((
                    String::from_utf8_lossy(st.dict.name(b"ColorSpace").unwrap_or_default()).into_owned(),
                    String::from_utf8_lossy(st.dict.name(b"Filter").unwrap_or_default()).into_owned(),
                )),
                _ => None,
            })
            .expect("the picture")
    };
    assert_eq!(picture(&s).0, "DeviceRGB");
    s.apply(id, Edit::ConvertColours { pages: vec![0], settings: ConvertColours::default() }).unwrap();
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Convert colours"));
    assert_eq!(picture(&s), ("DeviceGray".to_string(), "DCTDecode".to_string()));
    assert!(matches!(s.apply(id, Edit::ConvertColours { pages: vec![0], settings: ConvertColours::default() }), Err(EditError::Edit(_))));
    s.undo(id).unwrap();
    assert_eq!(picture(&s).0, "DeviceRGB");
}
