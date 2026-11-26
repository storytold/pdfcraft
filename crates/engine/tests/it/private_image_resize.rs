//! Opt-in acceptance checks. Private files, pixels and font programs never become fixtures.

use std::sync::Arc;

use pdfcraft_engine::{Edit, ImageEdit, Session};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};

fn pixels(bytes: Arc<Vec<u8>>, page: usize) -> pdfcraft_render::RenderedPage {
    let mut renderer = PageRenderer::new(bytes, Default::default());
    let rendered = renderer.render(RenderRequest { page, kind: RequestKind::Pixels, scale: 1.5, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    assert!(rendered.warnings.is_empty());
    rendered
}

fn near(actual: [f64; 4], expected: [f64; 4]) {
    assert!(actual.iter().zip(expected).all(|(a, b)| (a - b).abs() < 0.01), "{actual:?} vs {expected:?}");
}

#[test]
#[ignore = "requires an explicitly supplied local PDF with cropped artwork"]
fn private_cropped_images_resize_save_reopen_and_undo() {
    let path = std::env::var("PDFCRAFT_CLIPPED_ARTWORK_PDF").expect("supply a local PDF path");
    let original = Arc::new(std::fs::read(&path).unwrap());
    let mut session = Session::new();
    let id = session.open("private.pdf", None, original.clone(), None).unwrap();
    let pages = session.get(id).unwrap().info.pages.len();
    let mut checked = 0;
    for page in 0..pages {
        let images = session.get(id).unwrap().page_images(page);
        let original_pixels = pixels(session.get(id).unwrap().display.clone(), page);
        let info = session.get(id).unwrap().info.pages[page].clone();
        let text = || {
            session
                .get(id)
                .unwrap()
                .text_lines(page)
                .into_iter()
                .map(|l| (l.text.clone(), l.rect, l.font.clone(), l.size, l.color, l.baseline()))
                .collect::<Vec<_>>()
        };
        let original_text = text();
        for (index, image) in images.iter().enumerate() {
            let Some(visible) = image.visible_rect else { continue };
            for factor in [0.7, 1.04] {
                // Keep the visible upper-left corner fixed; transform the same cropped pixels.
                let transform = |r: [f64; 4]| {
                    [
                        visible[0] + (r[0] - visible[0]) * factor,
                        visible[3] + (r[1] - visible[3]) * factor,
                        visible[0] + (r[2] - visible[0]) * factor,
                        visible[3] + (r[3] - visible[3]) * factor,
                    ]
                };
                session.apply(id, Edit::EditPageImage { page, index, change: ImageEdit::Move(transform(image.rect)) }).unwrap();
                let doc = session.get(id).unwrap();
                let after = doc.page_images(page);
                near(after[index].visible_rect.unwrap(), transform(visible));
                near(after[index].rect, transform(image.rect));
                assert_eq!(after[index].object, image.object, "source pixels remain embedded unchanged");
                let current_text = doc
                    .text_lines(page)
                    .into_iter()
                    .map(|l| (l.text.clone(), l.rect, l.font.clone(), l.size, l.color, l.baseline()))
                    .collect::<Vec<_>>();
                assert_eq!(current_text, original_text, "all Chinese and Latin labels stay put");
                for (other, (a, b)) in after.iter().zip(&images).enumerate().filter(|(other, _)| *other != index) {
                    assert_eq!(
                        (a.rect, a.visible_rect, a.matrix, a.object),
                        (b.rect, b.visible_rect, b.matrix, b.object),
                        "neighbor image {other} unchanged"
                    );
                }
                let live = pixels(doc.display.clone(), page);
                assert_eq!((live.width, live.height), (original_pixels.width, original_pixels.height));
                assert_ne!(live.rgba, original_pixels.rgba, "the actual rendered image changes size");
                let dest = after[index].visible_rect.unwrap();
                let union = [visible[0].min(dest[0]), visible[1].min(dest[1]), visible[2].max(dest[2]), visible[3].max(dest[3])];
                let a = info.user_to_view(union[0] as f32, union[1] as f32);
                let b = info.user_to_view(union[2] as f32, union[3] as f32);
                for (offset, (before, after)) in original_pixels.rgba.as_chunks::<4>().0.iter().zip(live.rgba.as_chunks::<4>().0.iter()).enumerate() {
                    let x = (offset % live.width as usize) as f32 / 1.5;
                    let y = (offset / live.width as usize) as f32 / 1.5;
                    if x < a[0].min(b[0]) - 2.0 || x > a[0].max(b[0]) + 2.0 || y < a[1].min(b[1]) - 2.0 || y > a[1].max(b[1]) + 2.0 {
                        assert_eq!(before, after, "pixels outside the moved crop stay identical");
                    }
                }
                let saved = session.save_bytes(id).unwrap();
                let scratch = std::env::temp_dir().join(format!("pdfcraft-private-resize-{}.pdf", std::process::id()));
                std::fs::write(&scratch, saved.as_slice()).unwrap();
                let disk = Arc::new(std::fs::read(&scratch).unwrap());
                let mut reopened = Session::new();
                let re = reopened.open("saved.pdf", None, disk, None).unwrap();
                near(reopened.get(re).unwrap().page_images(page)[index].visible_rect.unwrap(), dest);
                assert_eq!(
                    pixels(reopened.get(re).unwrap().display.clone(), page).rgba,
                    live.rgba,
                    "saved/reopened rendering exactly matches the applied edit"
                );
                std::fs::remove_file(scratch).unwrap();
                session.undo(id).unwrap();
                assert_eq!(pixels(session.get(id).unwrap().display.clone(), page).rgba, original_pixels.rgba, "undo restores original rendering");
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
    assert_eq!(&std::fs::read(path).unwrap(), original.as_ref(), "the supplied file is untouched");
    println!("verified {checked} cropped-image grow/shrink save/reopen/undo cycles on {pages} pages");
}
