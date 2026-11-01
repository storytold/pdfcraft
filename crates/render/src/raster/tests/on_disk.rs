use super::*;
use pdfcraft_cos::{ByteSource, OnDiskFile};
use pdfcraft_testkit::Pdf;
use std::sync::atomic::{AtomicBool, Ordering};

/// A file in memory, read through [`ByteSource`], that can stop being readable.
struct File {
    bytes: Vec<u8>,
    gone: AtomicBool,
}

impl ByteSource for File {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
        if self.gone.load(Ordering::Relaxed) {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        let at = offset as usize;
        out.copy_from_slice(self.bytes.get(at..at + out.len()).ok_or(std::io::ErrorKind::UnexpectedEof)?);
        Ok(())
    }
}

/// Two pages, each filled by its own 200 by 200 grey image.
fn scanned() -> Vec<u8> {
    let image = |seed: usize| -> Vec<u8> { (0..40_000).map(|i| ((i * (seed + 1)) / 7 % 251) as u8).collect() };
    let page = |contents: usize, image: usize| {
        format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 100 100]/Contents {contents} 0 R/Resources<</XObject<</I {image} 0 R>>>>>>")
    };
    let image_dict = "/Type/XObject/Subtype/Image/Width 200/Height 200/BitsPerComponent 8/ColorSpace/DeviceGray";
    Pdf::new()
        .obj("<</Type/Catalog/Pages 2 0 R>>")
        .obj("<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>")
        .obj(page(5, 6))
        .obj(page(7, 8))
        .stream("", "q 100 0 0 100 0 0 cm /I Do Q")
        .stream(image_dict, image(0))
        .stream("", "q 100 0 0 100 0 0 cm /I Do Q")
        .stream(image_dict, image(1))
        .build()
}

fn on_disk(bytes: Vec<u8>) -> (Arc<OnDiskFile>, Arc<File>) {
    let file = Arc::new(File { bytes, gone: false.into() });
    (Arc::new(OnDiskFile::read(file.clone()).unwrap().expect("stays on disk")), file)
}

#[test]
fn pages_draw_the_same_with_their_bodies_on_disk() {
    let pdf = scanned();
    let (file, _) = on_disk(pdf.clone());
    assert_eq!(file.body_count(), 2);
    let mut whole = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let mut held = PageRenderer::new(file, RenderConfig::default());
    assert_eq!(held.page_count(), 2);
    for page in 0..2 {
        let request = RenderRequest { page, scale: 1.0, ..Default::default() };
        let (a, b) = (whole.render(request), held.render(request));
        assert_eq!(b.error, None);
        assert!(a.rgba.as_chunks::<4>().0.iter().any(|p| p[0] != 255), "the image draws");
        assert!(a.rgba == b.rgba, "page {page} differs");
    }
}

#[test]
fn a_cross_reference_entry_inside_a_body_draws_as_in_the_whole_file() {
    let mut pdf = scanned();
    let image = pdf.windows(4).position(|w| w == b"/Wid").unwrap();
    let inside = pdf[image..].windows(7).position(|w| w == b"stream\n").unwrap() + image + 200;
    // Object 6 is the first page's image: only a repair finds it.
    let table = pdf.windows(6).position(|w| w == b"\nxref\n").unwrap() + 6;
    let entry = table + pdf[table..].iter().position(|b| *b == b'\n').unwrap() + 1 + 6 * 20;
    pdf.splice(entry..entry + 10, format!("{inside:010}").into_bytes());
    let (file, _) = on_disk(pdf.clone());
    let mut whole = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let mut held = PageRenderer::new(file, RenderConfig::default());
    assert_eq!(held.page_count(), 2);
    let request = RenderRequest { page: 0, scale: 1.0, ..Default::default() };
    let (a, b) = (whole.render(request), held.render(request));
    assert_eq!(b.error, None);
    assert!(a.rgba.as_chunks::<4>().0.iter().any(|p| p[0] != 255), "the image draws");
    assert!(a.rgba == b.rgba);
}

#[test]
fn an_image_ending_in_a_line_end_byte_draws_as_in_the_whole_file() {
    // Its /Length comes after it, so the walk has to settle where it ends.
    let mut data: Vec<u8> = (0..39_999).map(|i| (i / 7 % 251) as u8).collect();
    data.push(b'\r');
    let image = b"<</Type/XObject/Subtype/Image/Width 200/Height 200/BitsPerComponent 8/ColorSpace/DeviceGray/Length 6 0 R>>\nstream\n";
    let pdf = Pdf::new()
        .obj("<</Type/Catalog/Pages 2 0 R>>")
        .obj("<</Type/Pages/Kids[3 0 R]/Count 1>>")
        .obj("<</Type/Page/Parent 2 0 R/MediaBox[0 0 100 100]/Contents 4 0 R/Resources<</XObject<</I 5 0 R>>>>>>")
        .stream("", "q 100 0 0 100 0 0 cm /I Do Q")
        .obj([image.as_slice(), &data, b"\nendstream"].concat())
        .obj("40000")
        .build();
    let (file, _) = on_disk(pdf.clone());
    assert_eq!(file.body_count(), 1);
    let mut whole = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let mut held = PageRenderer::new(file, RenderConfig::default());
    let request = RenderRequest { page: 0, scale: 2.0, ..Default::default() };
    let (a, b) = (whole.render(request), held.render(request));
    assert_eq!(b.error, None);
    assert!(a.rgba == b.rgba);
}

#[test]
fn the_pool_draws_pages_on_disk() {
    let (file, _) = on_disk(scanned());
    let pool = RenderPool::new_inline(file, RenderConfig::default());
    pool.set_queue(vec![RenderRequest { page: 1, scale: 1.0, ..Default::default() }]);
    let page = pool.try_recv().expect("rendered");
    assert_eq!(page.error, None);
    assert!(page.rgba.as_chunks::<4>().0.iter().any(|p| p[0] != 255));
}

#[test]
fn a_file_that_goes_away_is_an_error_and_not_a_blank_page() {
    let (held, file) = on_disk(scanned());
    let mut renderer = PageRenderer::new(held, RenderConfig::default());
    file.gone.store(true, Ordering::Relaxed);
    let page = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(page.error.as_deref().is_some_and(|e| e.contains("could not be read")), "{:?}", page.error);
}

#[test]
fn a_file_on_disk_inspects_as_the_whole_file_does() {
    let pdf = scanned();
    let (file, _) = on_disk(pdf.clone());
    let whole = crate::inspect(Arc::new(pdf), None).unwrap();
    let held = crate::inspect_on_disk(&file).unwrap().expect("read");
    assert_eq!(format!("{:?}", whole.pages), format!("{:?}", held.pages));
    assert_eq!(whole.file_size, held.file_size);
    assert_eq!(whole.warnings, held.warnings);
}

#[test]
fn inspecting_does_not_read_the_images() {
    struct Counted(File, std::sync::atomic::AtomicU64);
    impl ByteSource for Counted {
        fn size(&self) -> u64 {
            self.0.size()
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
            self.1.fetch_add(out.len() as u64, Ordering::Relaxed);
            self.0.read_at(offset, out)
        }
    }
    let bytes = scanned();
    let source = Arc::new(Counted(File { bytes, gone: false.into() }, 0.into()));
    let file = Arc::new(OnDiskFile::read(source.clone()).unwrap().unwrap());
    source.1.store(0, Ordering::Relaxed);
    crate::inspect_on_disk(&file).unwrap().expect("read");
    assert_eq!(source.1.load(Ordering::Relaxed), 0, "only what is held was read");
}

/// One page and `body` attached to the document, stored as it is.
fn attached(body: &[u8]) -> Vec<u8> {
    Pdf::new()
        .obj("<</Type/Catalog/Pages 2 0 R/Names<</EmbeddedFiles<</Names[(a.bin)6 0 R]>>>>>>")
        .obj("<</Type/Pages/Kids[3 0 R]/Count 1>>")
        .obj("<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>")
        .obj("<<>>")
        .stream("/Type/EmbeddedFile", body)
        .obj("<</Type/Filespec/F(a.bin)/EF<</F 5 0 R>>>>")
        .build()
}

#[test]
fn a_big_attachment_on_disk_reads_and_keeps_its_size() {
    let body: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    let (file, _) = on_disk(attached(&body));
    let info = crate::inspect_on_disk(&file).unwrap().expect("read");
    let attachment = info.attachments.first().expect("listed");
    assert_eq!(attachment.size, Some(body.len()), "from its /Length, without reading it");
    assert_eq!(crate::attachment_on_disk(&file, attachment).unwrap(), body);
}

#[test]
fn an_attachment_the_file_can_not_give_is_an_error_and_not_an_empty_file() {
    let (file, source) = on_disk(attached(&[7; 20_000]));
    let info = crate::inspect_on_disk(&file).unwrap().expect("read");
    let attachment = info.attachments.first().expect("listed");
    source.gone.store(true, Ordering::Relaxed);
    assert_eq!(crate::attachment_on_disk(&file, attachment), Err(crate::PART_UNREAD.to_string()));
}
