//! Linearized saves (Fast Web View, Annex F) checked by qpdf as an independent oracle, read back
//! by our parser and by hayro, encrypted, and updated incrementally afterwards.

use std::io::Write;
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document, NewEncryption, ObjRef, Object, PdfString, SaveOptions, is_linearized, write_full, write_incremental};
use pdfcraft_oracle::Oracle;
use pdfcraft_testkit::Pdf;

fn linearized() -> SaveOptions {
    SaveOptions { linearize: true, ..SaveOptions::default() }
}

/// Nine pages, so the hint tables' per-page arrays don't end on a byte boundary. Page 1 has its
/// own font, a link to page 5, a note with its popup and one widget of a check box whose other
/// widget is on page 2. Pages 2–9 share a font (with its descriptor and font file) and an image;
/// pages 4–9 inherit their resources, and every page its media box and rotation. Page 6 has a
/// thumbnail, and there are two outline items, document info and an unknown trailer entry.
fn fixture(outlines_open: bool) -> Vec<u8> {
    let mode = if outlines_open { "UseOutlines" } else { "UseNone" };
    let shared = "<< /Font << /F2 22 0 R >> /XObject << /Im 25 0 R >> >>";
    let mut pdf = Pdf::new()
        .version("1.7")
        .obj(format!("<< /Type /Catalog /Pages 2 0 R /Outlines 30 0 R /PageMode /{mode} /AcroForm 40 0 R >>"))
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 9 /MediaBox [0 0 200 300] /Rotate 0 >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 13 0 R /Resources << /Font << /F1 27 0 R >> >> /Annots [28 0 R 29 0 R 31 0 R 34 0 R] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 14 0 R /Resources << /Font << /F2 22 0 R >> /XObject << /Im 25 0 R /Own 42 0 R >> >> /Annots [35 0 R] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 15 0 R /Resources 41 0 R >>")
        .obj(format!("<< /Type /Pages /Parent 2 0 R /Kids [7 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R] /Count 6 /Resources {shared} >>"));
    for page in 4..=9 {
        let thumb = if page == 6 { " /Thumb 26 0 R" } else { "" };
        pdf = pdf.obj(format!("<< /Type /Page /Parent 6 0 R /Contents {} 0 R{thumb} >>", 12 + page));
    }
    for page in 1..=9 {
        let font = if page == 1 { "F1" } else { "F2" };
        pdf = pdf.stream("", format!("BT /{font} 24 Tf 20 150 Td (Page {page}) Tj ET"));
    }
    let image = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let appearance = "/Type /XObject /Subtype /Form /BBox [0 0 10 10]";
    pdf = pdf
        .obj("<< /Type /Font /Subtype /TrueType /BaseFont /Foo /FirstChar 32 /LastChar 32 /Widths [500] /FontDescriptor 23 0 R >>")
        .obj("<< /Type /FontDescriptor /FontName /Foo /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 24 0 R >>")
        .stream("", "not really a font")
        .stream(image, b"\x80")
        .stream(image, b"\x40")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [8 0 R /Fit] >>")
        .obj("<< /Type /Annot /Subtype /Text /Rect [20 20 30 30] /P 3 0 R /Popup 31 0 R /Contents (note) >>")
        .obj("<< /Type /Outlines /First 32 0 R /Last 33 0 R /Count 2 >>")
        .obj("<< /Type /Annot /Subtype /Popup /Rect [30 30 90 90] /P 3 0 R /Parent 29 0 R >>")
        .obj("<< /Title (One) /Parent 30 0 R /Next 33 0 R /Dest [3 0 R /Fit] >>")
        .obj("<< /Title (Six) /Parent 30 0 R /Prev 32 0 R /Dest [9 0 R /Fit] >>")
        .obj("<< /Type /Annot /Subtype /Widget /Rect [50 50 60 60] /P 3 0 R /Parent 36 0 R /AP << /N 37 0 R >> >>")
        .obj("<< /Type /Annot /Subtype /Widget /Rect [50 50 60 60] /P 4 0 R /Parent 36 0 R /AP << /N 38 0 R >> >>")
        .obj("<< /FT /Btn /T (box) /Kids [34 0 R 35 0 R] >>")
        .stream(appearance, "0 0 m 10 10 l S")
        .stream(appearance, "0 10 m 10 0 l S")
        .obj("<< /Title (Fast) /Producer (test) >>")
        .obj("<< /Fields [36 0 R] >>")
        .obj(shared)
        .stream(image, b"\x20");
    pdf.trailer_entry("/Info 39 0 R").trailer_entry("/PdfCraftTest (kept)").build()
}

fn one_page() -> Vec<u8> {
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> /Contents 4 0 R >>")
        .stream("", "0 0 m 10 10 l S")
        .build()
}

/// Three pages with extra catalog, page 2 and page 3 entries, and extra objects from number 9.
fn three_pages(catalog: &str, page2: &str, page3: &str, extra: &[Vec<u8>]) -> Vec<u8> {
    let mut pdf = Pdf::new()
        .obj(format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"))
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Resources << >> /Contents 6 0 R >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /Resources << >> /Contents 7 0 R {page2} >>"))
        .obj(format!("<< /Type /Page /Parent 2 0 R /Contents 8 0 R {page3} >>"))
        .stream("", "0 0 m 10 10 l S")
        .stream("", "0 0 m 20 20 l S")
        .stream("", "0 0 m 30 30 l S");
    for body in extra {
        pdf = pdf.obj(body);
    }
    pdf.build()
}

/// Objects reached in ways the nine-page fixture doesn't: each case is one way qpdf's idea of
/// who owns an object once differed from ours.
fn awkward() -> Vec<(&'static str, Vec<u8>)> {
    let obj = |s: &str| s.as_bytes().to_vec();
    vec![
        (
            "an open outline and page 3 share a destination",
            three_pages(
                "/Outlines 9 0 R /PageMode /UseOutlines",
                "",
                "/Resources << >> /Annots [12 0 R]",
                &[
                    obj("<< /Type /Outlines /First 10 0 R /Last 10 0 R /Count 1 >>"),
                    obj("<< /Title (Three) /Parent 9 0 R /Dest 11 0 R >>"),
                    obj("[5 0 R /Fit]"),
                    obj("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest 11 0 R >>"),
                ],
            ),
        ),
        (
            "page 2 reaches the catalog",
            three_pages(
                "/Names 9 0 R",
                "/PieceInfo << /Back 1 0 R >>",
                "/Resources << >>",
                &[obj("<< /Dests 10 0 R >>"), obj("<< /Names [(three) 11 0 R] >>"), obj("[5 0 R /Fit]")],
            ),
        ),
        (
            "an indirect /PageMode opens the outlines",
            three_pages(
                "/Outlines 9 0 R /PageMode 12 0 R",
                "",
                "/Resources << >>",
                &[
                    obj("<< /Type /Outlines /First 10 0 R /Last 11 0 R /Count 2 >>"),
                    obj("<< /Title (One) /Parent 9 0 R /Next 11 0 R /Dest [3 0 R /Fit] >>"),
                    obj("<< /Title (Three) /Parent 9 0 R /Prev 10 0 R /Dest [5 0 R /Fit] >>"),
                    obj("/UseOutlines"),
                ],
            ),
        ),
    ]
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).unwrap()
}

/// What qpdf says about `bytes`: whether `--check-linearization` finds a linearized file with no
/// errors and `--check` agrees, and its reports. `--check-linearization` exits 0 on a file that
/// isn't linearized at all, so the verdict reads its output, not just the exit status.
fn qpdf_verdict(qpdf: &Oracle, bytes: &[u8], password: Option<&str>) -> (bool, String) {
    let dir = crate::encryption::private_temp_dir("pdfcraft-linearize");
    let path = dir.join("linearized.pdf");
    std::fs::File::create_new(&path).and_then(|mut f| f.write_all(bytes)).unwrap();
    let run = |arg: &str| {
        let mut command = qpdf.command();
        if let Some(pw) = password {
            command.arg(format!("--password={pw}"));
        }
        let out = command.arg(arg).arg(&path).output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (code, stdout, stderr) = run("--check-linearization");
    let (check_code, check_out, check_err) = run("--check");
    let _ = std::fs::remove_dir_all(&dir);
    let clean = code == Some(0) && stdout.contains("no linearization errors") && !stderr.contains("WARNING");
    let agreed = check_code == Some(0) && check_out.contains("File is linearized");
    let report = format!("qpdf {}: {stdout}{stderr}\n--check: {check_out}{check_err}", qpdf.version());
    (clean && agreed, report)
}

fn title(doc: &Document) -> String {
    let info = doc.resolve(doc.trailer().get(b"Info").unwrap());
    info.as_dict().unwrap().get(b"Title").unwrap().as_string().unwrap().to_text()
}

fn hayro_pages(bytes: &[u8], password: &str) -> usize {
    hayro_syntax::Pdf::new_with_password(Arc::new(bytes.to_vec()), password).expect("hayro opens").pages().len()
}

#[test]
fn qpdf_finds_no_linearization_errors() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    for (name, bytes) in [("outlines closed", fixture(false)), ("outlines open", fixture(true)), ("one page", one_page())] {
        let out = write_full(&open(bytes), &linearized()).unwrap();
        let (ok, report) = qpdf_verdict(&qpdf, &out, None);
        assert!(ok, "{name}: {report}");
    }
    // The verdict can fail: a plain rewrite isn't linearized.
    let plain = write_full(&open(fixture(false)), &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let (ok, report) = qpdf_verdict(&qpdf, &plain, None);
    assert!(!ok && report.contains("is not linearized"), "{report}");
}

#[test]
fn qpdf_agrees_on_objects_reached_in_awkward_ways() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    // qpdf 11 places objects shared by open outlines and pages differently from qpdf 12, whose
    // checks these rules follow (Ubuntu 24.04 ships 11.9).
    let major = qpdf.version().split_whitespace().last().and_then(|v| v.split('.').next()?.parse::<u32>().ok());
    if major.is_none_or(|m| m < 12) {
        eprintln!("skipping: needs qpdf 12 or later, found {:?}", qpdf.version());
        return;
    }
    let failures: Vec<String> = awkward()
        .into_iter()
        .filter_map(|(name, bytes)| {
            let (ok, report) = qpdf_verdict(&qpdf, &write_full(&open(bytes), &linearized()).unwrap(), None);
            (!ok).then(|| format!("{name}: {report}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn pages_and_document_data_survive() {
    let out = write_full(&open(fixture(true)), &linearized()).unwrap();
    assert!(is_linearized(&out));
    assert_eq!(hayro_pages(&out, ""), 9);
    let back = open(out);
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(title(&back), "Fast");
    assert_eq!(back.trailer().get(b"PdfCraftTest"), Some(&Object::String(PdfString::literal("kept"))));
    let catalog = back.get(back.root().unwrap());
    let outlines = back.get(catalog.as_dict().unwrap().reference(b"Outlines").unwrap());
    assert_eq!(outlines.as_dict().unwrap().int(b"Count"), Some(2));
}

#[test]
fn every_encryption_algorithm_linearizes() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    for algorithm in [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let mut doc = open(fixture(false));
        let encryption =
            NewEncryption { algorithm, user_password: "user", owner_password: "owner", permissions: -1, encrypt_metadata: true, seed: [7; 32] };
        doc.set_encryption(&encryption).unwrap();
        let out = write_full(&doc, &linearized()).unwrap();
        assert!(is_linearized(&out), "{algorithm:?}");
        let (ok, report) = qpdf_verdict(&qpdf, &out, Some("user"));
        assert!(ok, "{algorithm:?}: {report}");
        assert_eq!(hayro_pages(&out, "user"), 9, "{algorithm:?}");
        let back = Document::open_with_password(Arc::new(out), Some("user")).unwrap();
        assert!(back.repair_log().is_empty(), "{algorithm:?}: {:?}", back.repair_log());
        assert_eq!(title(&back), "Fast", "{algorithm:?}");
    }
}

#[test]
fn an_update_on_top_opens_but_is_no_longer_linearized() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    let out = write_full(&open(fixture(false)), &linearized()).unwrap();
    let mut doc = open(out);
    let info = doc.trailer().reference(b"Info").unwrap();
    doc.update_dict(info, |d| d.set(b"Title".to_vec(), Object::String(PdfString::literal("Updated")))).unwrap();
    let updated = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(!is_linearized(&updated));
    let (ok, report) = qpdf_verdict(&qpdf, &updated, None);
    assert!(!ok && report.contains("is not linearized") && report.contains("No syntax or stream encoding errors"), "{report}");
    let back = open(updated);
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(title(&back), "Updated");
    assert_eq!(
        back.get(ObjRef::new(info.num, 0)).as_dict().unwrap().get(b"Producer").and_then(Object::as_string).map(PdfString::to_text).as_deref(),
        Some("test")
    );
}
