use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_full};
use pdfcraft_testkit::Pdf;

use super::*;
use crate::output::IccProfile;

/// A one-page document: object 3 is the page, 4 its content, and `extra` objects follow from 5.
fn page(content: &[u8], resources: &str, extra: &[Vec<u8>]) -> Document {
    page_with("<< /Type /Catalog /Pages 2 0 R >>", "", content, resources, extra)
}

fn page_with(catalog: &str, page_entries: &str, content: &[u8], resources: &str, extra: &[Vec<u8>]) -> Document {
    let bytes = Pdf::new()
        .obj(catalog)
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R /Resources << {resources} >> {page_entries} >>"))
        .stream("", content)
        .objs(extra)
        .build();
    Document::open(Arc::new(bytes)).unwrap()
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    pdfcraft_testkit::stream_obj(dict, data)
}

fn press() -> OutputCondition {
    OutputCondition::profile(IccProfile::read(pdfcraft_testkit::icc::cmyk_printer_profile("Test press (synthetic)")).unwrap(), None)
}

fn opts(condition: Option<OutputCondition>) -> Options {
    Options { condition, now: Some("D:20261016120000Z".into()), title: Some("test.pdf".into()), ..Options::default() }
}

/// Write the document as a save after conversion does, and open what was written.
fn reopen(doc: &Document) -> (Document, Vec<u8>) {
    let object_streams = max_version(doc).is_none_or(|v| v >= "1.5");
    let bytes = write_full(doc, &SaveOptions { object_streams, mod_date: None, ..SaveOptions::default() }).unwrap();
    (Document::open(Arc::new(bytes.clone())).unwrap(), bytes)
}

fn content_of(doc: &Document, page: usize) -> Vec<pdfcraft_content::Op> {
    let p = &pdfcraft_model::pages(doc)[page];
    let walked: Vec<Vec<u8>> = walk::contents(doc)
        .into_iter()
        .filter(|c| c.page == Some(page) && c.streams.iter().any(|r| p.dict.get(b"Contents").and_then(Object::as_ref) == Some(*r)))
        .flat_map(|c| c.streams)
        .map(|r| match &*doc.get(r) {
            Object::Stream(s) => s.decoded().unwrap(),
            _ => Vec::new(),
        })
        .collect();
    pdfcraft_content::Pieces::join(&walked).parse().ops
}

fn near(a: &[f64], b: &[f64], tolerance: f64) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
}

fn messages(r: &Refusal) -> Vec<String> {
    r.blockers.iter().map(|b| b.message.clone()).collect()
}

/// RGB in every form PDF/X-1a converts: operators, an ICC-based space set with `cs`, an 8-bit
/// picture, an indexed picture and an inline picture.
fn rgb_page() -> Document {
    let mut content = b"q 1 0 0 rg 0 0 50 50 re f 0 0 1 RG 0 0 m 10 10 l S /CS0 cs 0 1 0 sc 50 50 50 50 re f Q q 20 0 0 20 0 0 cm /Im0 Do Q \
q 20 0 0 20 30 0 cm /Im1 Do Q q 10 0 0 10 60 0 cm BI /W 2 /H 1 /CS /RGB /BPC 8 ID "
        .to_vec();
    content.extend([255, 0, 0, 0, 0, 255]);
    content.extend(b"\nEI Q");
    page(
        &content,
        "/ColorSpace << /CS0 [/ICCBased 5 0 R] >> /XObject << /Im0 6 0 R /Im1 7 0 R >>",
        &[
            stream("/N 3", &crate::icc::srgb()),
            stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", &[255, 0, 0, 0, 0, 255]),
            stream(
                "/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace [/Indexed /DeviceRGB 1 <FF000000FF00>] /BitsPerComponent 8",
                &[0, 1],
            ),
        ],
    )
}

#[test]
fn pdfx1a_converts_rgb_through_the_output_profile() {
    let mut doc = rgb_page();
    let fixed = pdfx(&mut doc, pdfx::Level::X1a2003, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.iter().any(|f| f.starts_with("Converted 4 colours, 2 pictures and 1 colour tables to CMYK")), "{fixed:#?}");
    let ops = content_of(&doc, 0);
    let colour =
        |op: &str| ops.iter().filter(|o| o.is(op)).map(|o| o.operands.iter().filter_map(Object::as_f64).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert!(colour("rg").is_empty() && colour("RG").is_empty(), "no RGB operators remain");
    // The naive press prints red as magenta and yellow, blue as cyan and magenta (its Lab grid
    // of 17 points is a few percent off at the gamut's edge).
    assert!(near(&colour("k")[0], &[0.0, 1.0, 1.0, 0.0], 0.1), "{:?}", colour("k"));
    assert!(near(&colour("K")[0], &[1.0, 1.0, 0.0, 0.0], 0.1), "{:?}", colour("K"));
    let fills = colour("sc");
    assert!(near(fills.last().unwrap(), &[1.0, 0.0, 1.0, 0.0], 0.1), "{fills:?}");
    // The picture and the indexed picture's table.
    let Object::Stream(im0) = &*doc.get(pdfcraft_cos::ObjRef::new(6, 0)) else { panic!() };
    assert_eq!(im0.dict.name(b"ColorSpace"), Some(b"DeviceCMYK".as_slice()));
    let px = im0.decoded().unwrap();
    assert_eq!(px.len(), 8);
    assert!(near(&px.iter().map(|v| f64::from(*v)).collect::<Vec<_>>(), &[0.0, 255.0, 255.0, 0.0, 255.0, 255.0, 0.0, 0.0], 26.0), "{px:?}");
    let Object::Stream(im1) = &*doc.get(pdfcraft_cos::ObjRef::new(7, 0)) else { panic!() };
    let cs = im1.dict.get(b"ColorSpace").and_then(Object::as_array).unwrap();
    assert_eq!(cs[1].as_name(), Some(b"DeviceCMYK".as_slice()));
    assert_eq!(cs[3].as_string().unwrap().bytes.len(), 8);
    // The inline picture: CMYK, hex over Flate.
    let inline = ops.iter().find_map(|o| o.inline.clone()).unwrap();
    assert_eq!(inline.0.name(b"CS"), Some(b"DeviceCMYK".as_slice()));
    // The resource that named the ICC space is gone, and the result verifies after a save.
    let page = &pdfcraft_model::pages(&doc)[0];
    assert!(walk::dict_of(&doc, walk::dict_of(&doc, page.dict.get(b"Resources")).get(b"ColorSpace")).get(b"CS0").is_none());
    assert_eq!(pdfx::verify(&doc, pdfx::Level::X1a2003), Vec::new());
    let (saved, bytes) = reopen(&doc);
    assert!(bytes.starts_with(b"%PDF-1.4\n"), "{:?}", String::from_utf8_lossy(&bytes[..9]));
    assert!(
        !bytes.windows(7).any(|w| w == b"/ObjStm") && !bytes.windows(6).any(|w| w == b"/XRef "),
        "PDF 1.4 has no object or cross-reference streams"
    );
    assert_eq!(pdfx::verify(&saved, pdfx::Level::X1a2003), Vec::new());
    assert_eq!(pdfx::declared(&saved), Some(pdfx::Level::X1a2003));
}

#[test]
fn pdfx1a_converts_jpeg_pictures_and_writes_them_lossless() {
    let mut jpeg = Vec::new();
    let red = image::RgbImage::from_pixel(8, 8, image::Rgb([255, 0, 0]));
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95).encode_image(&red).unwrap();
    let mut doc = page(
        b"q 20 0 0 20 0 0 cm /Im0 Do Q",
        "/XObject << /Im0 5 0 R >>",
        &[stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", &jpeg)],
    );
    pdfx(&mut doc, pdfx::Level::X1a2003, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    let Object::Stream(im) = &*doc.get(pdfcraft_cos::ObjRef::new(5, 0)) else { panic!() };
    assert_eq!(im.dict.name(b"Filter"), Some(b"FlateDecode".as_slice()));
    let px = im.decoded().unwrap();
    assert_eq!(px.len(), 8 * 8 * 4);
    assert!(near(&px[..4].iter().map(|v| f64::from(*v)).collect::<Vec<_>>(), &[0.0, 255.0, 255.0, 0.0], 24.0), "{:?}", &px[..4]);
}

#[test]
fn pdfx1a_refuses_what_it_cannot_convert_and_changes_nothing() {
    let mut doc = page(
        b"/Sh0 sh /Sp cs 1 sc 0 0 10 10 re f",
        "/Shading << /Sh0 << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >> >> \
         /ColorSpace << /Sp [/Separation /Gold /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0.8 0] /N 1 >>] /L [/Lab << /WhitePoint [0.9642 1 0.8249] >>] >>",
        &[],
    );
    let refusal = pdfx(&mut doc, pdfx::Level::X1a2003, &opts(Some(press()))).unwrap_err();
    let m = messages(&refusal);
    for expected in [
        "A shading in RGB or device-independent colour can't be converted to CMYK yet",
        "The spot colour Gold has an alternate that can't be converted to CMYK yet",
        "Lab colour can't be converted to CMYK yet",
    ] {
        assert!(m.iter().any(|x| x == expected), "{expected}: {m:#?}");
    }
    assert_eq!(refusal.level, "PDF/X-1a:2003");
    assert!(refusal.to_string().starts_with("can't save as PDF/X-1a:2003: "));
    assert!(!doc.is_modified(), "a refused conversion changes nothing");
    assert!(pdfx::declared(&doc).is_none());
}

#[test]
fn pdfx1a_names_fogra39_without_a_profile_and_needs_one_for_rgb() {
    let mut cmyk = page(b"0 1 1 0 k 0 0 10 10 re f 0.5 g 0 0 5 5 re f", "", &[]);
    pdfx(&mut cmyk, pdfx::Level::X1a2003, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
    let intents = crate::declared(&cmyk).output_intents;
    assert_eq!(intents, vec!["FOGRA39".to_string()]);
    let cat = crate::catalog(&cmyk);
    let oi = cat.get(b"OutputIntents").and_then(Object::as_array).unwrap()[0].as_dict().unwrap().clone();
    assert!(oi.get(b"DestOutputProfile").is_none());
    assert_eq!(oi.get(b"RegistryName").and_then(Object::as_string).map(|s| s.to_text()).as_deref(), Some("http://www.color.org"));

    let mut rgb = page(b"1 0 0 rg 0 0 10 10 re f", "", &[]);
    let m = messages(&pdfx(&mut rgb, pdfx::Level::X1a2003, &opts(None)).unwrap_err());
    assert_eq!(
        m.first().map(String::as_str),
        Some("Converting colour to CMYK needs the printing condition's ICC profile: choose a profile file"),
        "{m:#?}"
    );
}

#[test]
fn pdfx4_embeds_a_chosen_profile_and_maps_rgb_to_srgb() {
    let mut doc = page(
        b"1 0 0 rg 0 0 10 10 re f 0 0 0 1 k 0 0 5 5 re f q 20 0 0 20 0 0 cm /Im0 Do Q",
        "/XObject << /Im0 5 0 R >>",
        &[stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", &[0, 255, 0])],
    );
    let refusal = pdfx(&mut doc.clone(), pdfx::Level::X4, &opts(None)).unwrap_err();
    assert!(messages(&refusal).contains(&"PDF/X-4 embeds the printing condition's ICC profile: choose a profile file".to_string()));
    let refusal = pdfx(&mut doc.clone(), pdfx::Level::X4, &opts(OutputCondition::registered("FOGRA39"))).unwrap_err();
    assert!(messages(&refusal).contains(&"PDF/X-4 embeds the printing condition's ICC profile: choose a profile file".to_string()));
    let fixed = pdfx(&mut doc, pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.iter().any(|f| f.starts_with("RGB colour is now sRGB (DefaultRGB")), "{fixed:#?}");
    // RGB stays RGB, described by sRGB: as DefaultRGB for operators, as the picture's own space.
    assert!(content_of(&doc, 0).iter().any(|o| o.is("rg")));
    let page = &pdfcraft_model::pages(&doc)[0];
    let spaces = walk::dict_of(&doc, walk::dict_of(&doc, page.dict.get(b"Resources")).get(b"ColorSpace"));
    assert!(spaces.get(b"DefaultRGB").is_some());
    let Object::Stream(im) = &*doc.get(pdfcraft_cos::ObjRef::new(5, 0)) else { panic!() };
    assert_eq!(im.dict.get(b"ColorSpace").and_then(Object::as_array).and_then(|a| a[0].as_name()), Some(b"ICCBased".as_slice()));
    let metadata = crate::metadata(&doc).unwrap();
    assert_eq!(xmp::value(&metadata, "pdfxid:GTS_PDFXVersion").as_deref(), Some("PDF/X-4"));
    assert_eq!(xmp::value(&metadata, "dc:title").as_deref(), Some("test.pdf"));
    assert_eq!(xmp::value(&metadata, "pdf:Trapped").as_deref(), Some("False"));
    assert!(xmp::value(&metadata, "xmpMM:DocumentID").is_some_and(|id| id.starts_with("uuid:")));
    let (saved, bytes) = reopen(&doc);
    assert!(bytes.starts_with(b"%PDF-1.6\n"));
    assert_eq!(pdfx::verify(&saved, pdfx::Level::X4), Vec::new());
    assert_eq!(crate::declared(&saved).output_intents, vec!["Test press (synthetic)".to_string()]);
}

#[test]
fn without_a_choice_the_documents_own_printing_condition_is_kept() {
    let mut doc = page(b"1 0 0 rg 0 0 10 10 re f", "", &[]);
    pdfx(&mut doc, pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    let (mut saved, _) = reopen(&doc);
    assert_eq!(printing_condition(&saved).map(|c| c.identifier()).as_deref(), Some("Test press (synthetic)"));
    // Converted again with no condition chosen, to PDF/X-1a: the embedded press converts the RGB.
    pdfx(&mut saved, pdfx::Level::X1a2003, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!(crate::declared(&saved).output_intents, vec!["Test press (synthetic)".to_string()]);
    // A registered characterization is kept by name.
    let mut cmyk = page(b"0 0 0 1 k", "", &[]);
    pdfx(&mut cmyk, pdfx::Level::X3_2003, &opts(OutputCondition::registered("JC200103"))).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!(printing_condition(&cmyk), OutputCondition::registered("JC200103"));
    pdfx(&mut cmyk, pdfx::Level::X1a2003, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!(crate::declared(&cmyk).output_intents, vec!["JC200103".to_string()]);
}

#[test]
fn pdfx3_needs_an_embedded_profile_for_rgb() {
    let mut doc = page(b"1 0 0 rg 0 0 10 10 re f", "", &[]);
    let m = messages(&pdfx(&mut doc, pdfx::Level::X3_2003, &opts(None)).unwrap_err());
    assert!(
        m.contains(
            &"PDF/X-3:2003 needs the printing condition's ICC profile embedded for RGB or device-independent colour: choose a profile file"
                .to_string()
        ),
        "{m:#?}"
    );
    pdfx(&mut doc, pdfx::Level::X3_2003, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!(pdfx::verify(&reopen(&doc).0, pdfx::Level::X3_2003), Vec::new());
}

#[test]
fn a_profile_that_isnt_a_cmyk_printer_profile_is_refused() {
    let mut doc = page(b"0 0 0 1 k", "", &[]);
    let display = OutputCondition::profile(IccProfile::read(crate::icc::srgb()).unwrap(), None);
    let m = messages(&pdfx(&mut doc, pdfx::Level::X4, &opts(Some(display.clone()))).unwrap_err());
    assert!(m.iter().any(|x| x.ends_with("isn't an output (printer) profile")), "{m:#?}");
    let mut rgb_press = pdfcraft_testkit::icc::cmyk_printer_profile("RGB press");
    rgb_press[16..20].copy_from_slice(b"RGB ");
    let rgb_press = OutputCondition::profile(IccProfile::read(rgb_press).unwrap(), None);
    let m = messages(&pdfx(&mut doc, pdfx::Level::X1a2003, &opts(Some(rgb_press))).unwrap_err());
    assert!(m.contains(&"The ICC profile RGB press isn't a CMYK profile, which PDF/X-1a needs".to_string()), "{m:#?}");
}

#[test]
fn page_boxes_are_set_and_fitted() {
    // No TrimBox: the CropBox becomes it, and the BleedBox.
    let mut doc = page_with("<< /Type /Catalog /Pages 2 0 R >>", "/CropBox [10 10 190 290]", b"0 g", "", &[]);
    let fixed = pdfx(&mut doc, pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.contains(&"1 pages: set the TrimBox to the CropBox".to_string()), "{fixed:#?}");
    let p = &pdfcraft_model::pages(&doc)[0];
    assert_eq!(rect(&doc, p.dict.get(b"TrimBox")), Some([10.0, 10.0, 190.0, 290.0]));
    assert_eq!(rect(&doc, p.dict.get(b"BleedBox")), Some([10.0, 10.0, 190.0, 290.0]));
    // Both a TrimBox and an ArtBox: the ArtBox goes. A TrimBox past the MediaBox is fitted.
    let mut doc = page_with("<< /Type /Catalog /Pages 2 0 R >>", "/TrimBox [-5 0 200 300] /ArtBox [0 0 100 100]", b"0 g", "", &[]);
    pdfx(&mut doc, pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    let p = &pdfcraft_model::pages(&doc)[0];
    assert!(p.dict.get(b"ArtBox").is_none());
    assert_eq!(rect(&doc, p.dict.get(b"TrimBox")), Some([0.0, 0.0, 200.0, 300.0]));
    assert_eq!(pdfx::verify(&doc, pdfx::Level::X4), Vec::new());
}

#[test]
fn scripts_and_automatic_actions_are_removed() {
    let mut doc = page_with(
        "<< /Type /Catalog /Pages 2 0 R /OpenAction << /S /JavaScript /JS (app.alert(1)) >> /AA << /WC 5 0 R >> /Names << /JavaScript << /Names [(a) 5 0 R] >> >> >>",
        "/AA << /O << /S /GoTo /D [3 0 R /Fit] >> >>",
        b"0 g",
        "",
        &[b"<< /S /JavaScript /JS (this.print()) >>".to_vec()],
    );
    let fixed = pdfx(&mut doc, pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.iter().any(|f| f.ends_with("removed JavaScript or additional actions")), "{fixed:#?}");
    let cat = crate::catalog(&doc);
    assert!(cat.get(b"OpenAction").is_none() && cat.get(b"AA").is_none());
    assert!(walk::dict_of(&doc, cat.get(b"Names")).get(b"JavaScript").is_none());
    assert!(pdfcraft_model::pages(&doc)[0].dict.get(b"AA").is_none());
}

#[test]
fn identification_trapped_and_the_document_id() {
    let mut doc = Document::open(Arc::new(
        Pdf::new()
            .version("1.7")
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
            .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>")
            .stream("", b"0 0 0 1 k 0 0 10 10 re f")
            .obj("<< /Trapped /Unknown /GTS_PDFXVersion (PDF/X-4) >>")
            .trailer_entry("/Info 5 0 R")
            .build(),
    ))
    .unwrap();
    let fixed = pdfx(&mut doc, pdfx::Level::X1a2001, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.contains(&"Trapped set to /False: the document didn't say it was trapped".to_string()));
    assert!(fixed.contains(&"Gave the file a document ID".to_string()));
    assert!(fixed.contains(&"Wrote the file as PDF 1.3".to_string()));
    assert_eq!(crate::info(&doc, "GTS_PDFXVersion").as_deref(), Some("PDF/X-1:2001"));
    assert_eq!(crate::info(&doc, "GTS_PDFXConformance").as_deref(), Some("PDF/X-1a:2001"));
    let (saved, bytes) = reopen(&doc);
    assert!(bytes.starts_with(b"%PDF-1.3\n"));
    assert_eq!(pdfx::declared(&saved), Some(pdfx::Level::X1a2001));
    assert_eq!(pdfx::verify(&saved, pdfx::Level::X1a2001), Vec::new());
    assert_eq!(max_version(&saved), Some("1.3"));
}

#[test]
fn fonts_that_arent_embedded_and_transparency_refuse() {
    let font = page(b"BT /F1 12 Tf (Hi) Tj ET", "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>", &[]);
    let m = messages(&pdfx(&mut font.clone(), pdfx::Level::X4, &opts(Some(press()))).unwrap_err());
    assert!(m.contains(&"The font Helvetica isn't embedded".to_string()), "{m:#?}");
    let transparent = page(b"/G0 gs 0 0 0 1 k 0 0 10 10 re f", "/ExtGState << /G0 << /ca 0.5 >> >>", &[]);
    let m = messages(&pdfx(&mut transparent.clone(), pdfx::Level::X1a2003, &opts(None)).unwrap_err());
    assert!(m.contains(&"A graphics state sets an opacity below 1".to_string()), "{m:#?}");
    // PDF/X-4 keeps transparency.
    pdfx(&mut transparent.clone(), pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
}

#[test]
fn annotations_that_print_nothing_leave_the_bleed_when_asked() {
    let doc = page_with(
        "<< /Type /Catalog /Pages 2 0 R >>",
        "/Annots [5 0 R]",
        b"0 0 0 1 k",
        "",
        &[b"<< /Type /Annot /Subtype /Link /Rect [10 10 50 20] /A << /S /URI /URI (https://example.org) >> >>".to_vec()],
    );
    let m = messages(&pdfx(&mut doc.clone(), pdfx::Level::X4, &opts(Some(press()))).unwrap_err());
    assert!(m.contains(&"A Link annotation lies within the BleedBox".to_string()), "{m:#?}");
    let mut doc = doc;
    let fixed = pdfx(&mut doc, pdfx::Level::X4, &Options { drop_unprinted: true, ..opts(Some(press())) }).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.contains(&"1: removed links and annotations that don't print from the bleed".to_string()), "{fixed:#?}");
}

#[test]
fn later_features_than_the_level_allows_refuse() {
    let doc = page_with(
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /AS [<< /Event /Print /Category [/Print] /OCGs [5 0 R] >>] >> >> >>",
        "",
        b"0 0 0 1 k",
        "",
        &[b"<< /Type /OCG /Name (Layer) >>".to_vec()],
    );
    let m = messages(&pdfx(&mut doc.clone(), pdfx::Level::X1a2003, &opts(None)).unwrap_err());
    assert!(m.contains(&"Layers (optional content) need PDF 1.5, newer than PDF/X-1a:2003 allows".to_string()), "{m:#?}");
    // PDF/X-4 has layers: each configuration gets a name and loses its automatic states.
    let mut doc = doc;
    pdfx(&mut doc, pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    let props = walk::dict_of(&doc, crate::catalog(&doc).get(b"OCProperties"));
    let d = walk::dict_of(&doc, props.get(b"D"));
    assert!(d.get(b"Name").is_some() && d.get(b"AS").is_none());
}

#[test]
fn an_encrypted_document_needs_its_owner_password() {
    let plain = page(b"0 0 0 1 k", "", &[]);
    let mut locked = plain.clone();
    locked
        .set_encryption(&pdfcraft_crypt::NewEncryption {
            algorithm: pdfcraft_crypt::Algorithm::Aes256,
            user_password: "user",
            owner_password: "owner",
            permissions: -4,
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
    let bytes = Arc::new(write_full(&locked, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap());
    let mut as_user = Document::open_with_password(bytes.clone(), Some("user")).unwrap();
    let m = messages(&pdfx(&mut as_user, pdfx::Level::X4, &opts(Some(press()))).unwrap_err());
    assert!(m.contains(&"The document is encrypted: open it with its permissions password to remove the security".to_string()), "{m:#?}");
    let mut as_owner = Document::open_with_password(bytes, Some("owner")).unwrap();
    let fixed = pdfx(&mut as_owner, pdfx::Level::X4, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.contains(&"Removed the security (encryption)".to_string()));
}

#[test]
fn pdfa_gives_a_cmyk_document_the_chosen_profile_as_its_output_intent() {
    let doc = page(b"0 0 0 1 k 0 0 10 10 re f 1 0 0 rg 0 0 5 5 re f", "", &[]);
    let m = messages(&pdfa(&mut doc.clone(), Level::A2b, &opts(None)).unwrap_err());
    assert!(
        m.contains(&"The document uses CMYK colour: choose the ICC profile of a CMYK printing condition for its output intent".to_string()),
        "{m:#?}"
    );
    let mut doc = doc;
    let fixed = pdfa(&mut doc, Level::A2b, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.iter().any(|f| f.starts_with("Set the output intent (GTS_PDFA1)")), "{fixed:#?}");
    assert!(fixed.iter().any(|f| f.starts_with("RGB colour is now sRGB")), "{fixed:#?}");
    let (saved, _) = reopen(&doc);
    assert_eq!(crate::verify(&saved, Level::A2b), Vec::new());
    assert_eq!(crate::declared(&saved).pdfa, Some((2, "B".to_string())));
    // A registered condition has no profile to embed.
    let registered = OutputCondition::registered("FOGRA39").unwrap();
    let m = messages(&pdfa(&mut page(b"0 0 0 1 k", "", &[]), Level::A2b, &opts(Some(registered))).unwrap_err());
    assert!(m.contains(&"PDF/A embeds the output intent's ICC profile: choose a profile file".to_string()), "{m:#?}");
}

/// An embedded TrueType font (its program is a stand-in: the rules only ask that there is one)
/// with `encoding`.
fn font(encoding: &str) -> Vec<Vec<u8>> {
    vec![
        format!("<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Test {encoding} /FirstChar 32 /LastChar 126 /Widths [{}] /FontDescriptor 6 0 R >>", "500 ".repeat(95)).into_bytes(),
        b"<< /Type /FontDescriptor /FontName /ABCDEF+Test /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 7 0 R >>".to_vec(),
        stream("", b"stand-in font program"),
    ]
}

#[test]
fn pdfa_u_needs_unicode_for_every_character_shown() {
    let mut mapped = page(b"BT /F1 12 Tf (Hello) Tj ET", "/Font << /F1 5 0 R >>", &font("/Encoding /WinAnsiEncoding"));
    pdfa(&mut mapped, Level::A2u, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!(crate::declared(&mapped).pdfa, Some((2, "U".to_string())));
    // Glyph names that aren't Unicode names give the codes no Unicode value.
    let unmapped = page(b"BT /F1 12 Tf (AB) Tj ET", "/Font << /F1 5 0 R >>", &font("/Encoding << /Differences [65 /g001 /g002] >>"));
    let refusal = pdfa(&mut unmapped.clone(), Level::A2u, &opts(None)).unwrap_err();
    assert!(messages(&refusal).contains(&"The font ABCDEF+Test has characters with no Unicode value".to_string()), "{refusal:#?}");
    assert_eq!(refusal.blockers[0].rule.as_deref(), Some("ISO 19005-2 6.2.11.7.2"));
    // Level b doesn't ask.
    pdfa(&mut unmapped.clone(), Level::A2b, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
}

#[test]
fn pdfa_a_needs_tags() {
    let untagged = page(b"0 g", "", &[]);
    let m = messages(&pdfa(&mut untagged.clone(), Level::A3a, &opts(None)).unwrap_err());
    assert!(m.contains(&"The document has no structure tree: it isn't tagged".to_string()), "{m:#?}");
    // A structure tree without MarkInfo: marked as tagged. A type the role map maps is fine.
    let mut tagged = page_with(
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /Lang (es-ES) >>",
        "",
        b"/P <</MCID 0>> BDC 0 g EMC",
        "",
        &[
            b"<< /Type /StructTreeRoot /K 6 0 R /RoleMap << /Heading /H1 >> >>".to_vec(),
            b"<< /Type /StructElem /S /Document /P 5 0 R /K [7 0 R] >>".to_vec(),
            b"<< /Type /StructElem /S /Heading /P 6 0 R /Pg 3 0 R /K 0 >>".to_vec(),
        ],
    );
    let fixed = pdfa(&mut tagged, Level::A3a, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
    assert!(fixed.contains(&"Marked the document as tagged (it has a structure tree)".to_string()), "{fixed:#?}");
    assert_eq!(crate::verify(&reopen(&tagged).0, Level::A3a), Vec::new());
    // A type nothing maps, and a malformed language tag, refuse.
    let mut odd = page_with(
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> /Lang (en_US) >>",
        "",
        b"0 g",
        "",
        &[b"<< /Type /StructTreeRoot /K 6 0 R >>".to_vec(), b"<< /Type /StructElem /S /Fancy /P 5 0 R >>".to_vec()],
    );
    let m = messages(&pdfa(&mut odd, Level::A2a, &opts(None)).unwrap_err());
    assert!(m.contains(&"The structure type Fancy isn't standard and isn't mapped to a standard type".to_string()), "{m:#?}");
    assert!(m.contains(&"The language en_US isn't a valid language tag".to_string()), "{m:#?}");
}

#[test]
fn pdfa_keeps_a_pdfua_identification_and_describes_its_schema() {
    let xmp_packet = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"><pdfuaid:part>1</pdfuaid:part></rdf:Description></rdf:RDF></x:xmpmeta>";
    let mut doc = page_with(
        "<< /Type /Catalog /Pages 2 0 R /Metadata 5 0 R >>",
        "",
        b"0 g",
        "",
        &[stream("/Type /Metadata /Subtype /XML", xmp_packet.as_bytes())],
    );
    pdfa(&mut doc, Level::A2b, &opts(None)).unwrap_or_else(|r| panic!("{r}"));
    let metadata = crate::metadata(&doc).unwrap();
    assert_eq!(xmp::value(&metadata, "pdfuaid:part").as_deref(), Some("1"));
    assert!(metadata.contains("<pdfaSchema:prefix>pdfuaid</pdfaSchema:prefix>"));
}

/// The converted files as written, for an independent reader: `qpdf --check` finds them sound.
/// They stay in the temporary folder (`PDFCRAFT_CONVERTED` names another) so they can be checked
/// by hand as well.
#[test]
fn qpdf_finds_the_converted_files_sound() {
    let dir = std::env::var_os("PDFCRAFT_CONVERTED")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("pdfcraft-converted-{}", std::process::id())));
    std::fs::create_dir_all(&dir).unwrap();
    let mut written = Vec::new();
    for (name, level) in
        [("x1a-2001", pdfx::Level::X1a2001), ("x1a-2003", pdfx::Level::X1a2003), ("x3-2003", pdfx::Level::X3_2003), ("x4", pdfx::Level::X4)]
    {
        let mut doc = rgb_page();
        pdfx(&mut doc, level, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
        let (saved, bytes) = reopen(&doc);
        assert_eq!(pdfx::verify(&saved, level), Vec::new(), "{name}");
        let path = dir.join(format!("{name}.pdf"));
        std::fs::write(&path, bytes).unwrap();
        written.push(path);
    }
    let mut doc = page(b"0 0 0 1 k 0 0 10 10 re f 1 0 0 rg 0 0 5 5 re f", "", &[]);
    pdfa(&mut doc, Level::A2u, &opts(Some(press()))).unwrap_or_else(|r| panic!("{r}"));
    let path = dir.join("a2u.pdf");
    std::fs::write(&path, reopen(&doc).1).unwrap();
    written.push(path);
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    for path in written {
        let out = qpdf.command().arg("--check").arg(&path).output().unwrap();
        assert!(out.status.success(), "{}: {}{}", path.display(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn levels_read_their_ids() {
    for l in Level::ALL {
        assert_eq!(Level::from_id(l.label()), Some(l));
        assert_eq!(Level::from_id(&format!("{}{}", l.part(), l.conformance().to_lowercase())), Some(l));
    }
    assert_eq!(Level::from_id("a2b"), Some(Level::A2b));
    assert_eq!(Level::from_id("4f"), None);
}

/// Every blocker the conversion can report is in `WORDINGS`, so an interface can translate it.
#[test]
fn wordings_are_complete() {
    let mut said = Vec::new();
    for source in [include_str!("../conform.rs"), include_str!("../recolour.rs")] {
        for opener in ["block(\"", "Blocked(\""] {
            let mut rest = source;
            while let Some(at) = rest.find(opener) {
                rest = &rest[at + opener.len()..];
                let Some(end) = rest.find('"') else { break };
                said.push(&rest[..end]);
            }
        }
        // Wordings split over lines in a call: `self.block(\n    "…",`.
        let mut rest = source;
        while let Some(at) = rest.find("block(\n") {
            rest = &rest[at + 7..];
            let Some(open) = rest.find('"') else { break };
            let text = &rest[open + 1..];
            let Some(end) = text.find('"') else { break };
            said.push(&text[..end]);
        }
    }
    assert!(said.len() >= 25, "the wordings are not being read off the files: {said:#?}");
    let missing: Vec<&&str> = said.iter().filter(|w| !WORDINGS.contains(w)).collect();
    assert!(missing.is_empty(), "add these to WORDINGS: {missing:#?}");
    let unsaid: Vec<&&str> = WORDINGS.iter().filter(|w| !said.contains(w)).collect();
    assert!(unsaid.is_empty(), "WORDINGS lists blockers nothing reports: {unsaid:#?}");
}
