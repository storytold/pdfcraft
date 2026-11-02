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
fn text_counted_from_a_file_that_goes_away_is_an_error_and_not_a_short_count() {
    let pdf = Pdf::new()
        .obj("<</Type/Catalog/Pages 2 0 R>>")
        .obj("<</Type/Pages/Kids[3 0 R]/Count 1>>")
        .obj("<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 100]/Contents 4 0 R/Resources<</Font<</F 5 0 R>>>>>>")
        .stream("", format!("BT /F 24 Tf 20 50 Td (Hello) Tj ET % {}", "x".repeat(30_000)))
        .obj("<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>")
        .build();
    let (held, file) = on_disk(pdf);
    let mut renderer = PageRenderer::new(held, RenderConfig::default());
    let text = |renderer: &mut PageRenderer| renderer.filtered_text(0, crate::text::TextFilter::default()).map(|t| t.plain_text());
    assert_eq!(text(&mut renderer).unwrap().trim(), "Hello");
    file.gone.store(true, Ordering::Relaxed);
    assert_eq!(text(&mut renderer).err().as_deref(), Some(PART_UNREAD));
}

/// Two pages whose small contents stay in memory and use `shared`, object 7 on, whose body stays
/// on disk: a read of that body is all that can fail.
fn sharing(resources: &str, contents: &str, shared: impl FnOnce(Pdf) -> Pdf) -> Vec<u8> {
    let page = |contents: usize| format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 100 100]/Contents {contents} 0 R/Resources<<{resources}>>>>");
    let pdf = Pdf::new()
        .obj("<</Type/Catalog/Pages 2 0 R>>")
        .obj("<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>")
        .obj(page(5))
        .obj(page(6))
        .stream("", contents)
        .stream("", contents);
    shared(pdf).build()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_page_after_a_failed_read_reads_again_and_never_reuses_what_failed() {
    let type0 = |pdf: Pdf| {
        pdf.obj("<</Type/Font/Subtype/Type0/BaseFont/X/Encoding/Identity-H/DescendantFonts[8 0 R]>>")
            .obj("<</Type/Font/Subtype/CIDFontType2/BaseFont/X/CIDSystemInfo<</Registry(Adobe)/Ordering(Identity)/Supplement 0>>/FontDescriptor 9 0 R>>")
            .obj("<</Type/FontDescriptor/FontName/X/Flags 4/FontFile2 10 0 R>>")
            .stream("", vec![0_u8; 8_000])
    };
    let simple = |pdf: Pdf| {
        pdf.obj("<</Type/Font/Subtype/TrueType/BaseFont/X/FirstChar 65/LastChar 65/Widths[600]/FontDescriptor 8 0 R>>")
            .obj("<</Type/FontDescriptor/FontName/X/Flags 4/FontFile2 9 0 R>>")
            .stream("", vec![0_u8; 8_000])
    };
    let image = "/Type/XObject/Subtype/Image/Width 200/Height 200/BitsPerComponent 8/ColorSpace/DeviceGray";
    let pixels: Vec<u8> = (0..40_000).map(|i| (i / 7 % 251) as u8).collect();
    let text = "BT /F 40 Tf 10 30 Td <0041> Tj ET";
    let cases = [
        ("Type0 font", sharing("/Font<</F 7 0 R>>", text, type0)),
        ("TrueType font", sharing("/Font<</F 7 0 R>>", "BT /F 40 Tf 10 30 Td (A) Tj ET", simple)),
        (
            "colour space",
            sharing("/ColorSpace<</C[/ICCBased 7 0 R]>>", "/C cs 1 0 0 sc 0 0 100 100 re f", |pdf| pdf.stream("/N 3", vec![0_u8; 8_000])),
        ),
        ("image", sharing("/XObject<</I 7 0 R>>", "q 100 0 0 100 0 0 cm /I Do Q", |pdf| pdf.stream(image, pixels))),
    ];
    for (name, pdf) in cases {
        let (held, file) = on_disk(pdf.clone());
        assert_eq!(held.body_count(), 1, "{name}");
        let pool = RenderPool::new(held, 1, RenderConfig::default());
        let draw = |page: usize| {
            pool.set_queue(vec![RenderRequest { page, scale: 1.0, ..Default::default() }]);
            receive_before_deadline(&pool)
        };
        file.gone.store(true, Ordering::Relaxed);
        assert_eq!(draw(0).error.as_deref(), Some(PART_UNREAD), "{name}");
        assert_eq!(draw(1).error.as_deref(), Some(PART_UNREAD), "{name}: the page drew without the body");
        file.gone.store(false, Ordering::Relaxed);
        let page = draw(1);
        assert_eq!(page.error, None, "{name}");
        let whole = PageRenderer::new(Arc::new(pdf), RenderConfig::default()).render(RenderRequest { page: 1, scale: 1.0, ..Default::default() });
        assert!(page.rgba == whole.rgba, "{name}: the page differs once the file is back");
    }
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
