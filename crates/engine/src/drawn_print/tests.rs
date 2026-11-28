use super::*;

/// A Letter PDF with a page per content stream, each with `resources`.
fn pdf(pages: &[&str], resources: &str) -> Vec<u8> {
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objects =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len())];
    for (i, content) in pages.iter().enumerate() {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {resources} /Contents {} 0 R >>", 4 + 2 * i));
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len() + 1));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n\r\n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

/// Whether each page of `bytes` draws an image XObject.
fn pages_with_images(bytes: Vec<u8>) -> Vec<bool> {
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    pdfcraft_model::pages(&doc)
        .iter()
        .map(|page| {
            let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
            let xobjects = resources.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
            xobjects.iter().any(|(_, x)| matches!(&*doc.resolve(x), pdfcraft_cos::Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
        })
        .collect()
}

/// The vector route on the in-box PDF printer, when `PDFCRAFT_DRAWN_PRINT_OUT` names a folder to
/// write its output in: a sheet of opaque paths and text reaches the printer without a picture,
/// a sheet with translucent paint is drawn as one and said so, and the image route draws both as
/// pictures.
#[test]
fn the_vector_route_draws_opaque_sheets_as_vectors_and_translucent_ones_as_pixels() {
    let Some(folder) = std::env::var_os("PDFCRAFT_DRAWN_PRINT_OUT") else { return };
    let out = std::path::Path::new(&folder).join("vector-route.pdf");
    let _ = std::fs::remove_file(&out);
    let resources = "<< /ExtGState << /Half << /ca 0.5 >> >> /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>";
    let opaque = "1 0 0 rg 72 600 200 100 re f 0 0 1 RG 3 w 72 500 m 540 500 l S BT /F1 36 Tf 0 g 72 400 Td (Vector) Tj ET";
    let translucent = "/Half gs 0 0.6 0 rg 72 600 200 100 re f";
    let job = Job { printer: Some("Microsoft Print to PDF".into()), title: "drawn route".into(), ..Default::default() };
    let sheets = pdf(&[opaque, translucent], resources);
    let receipt = submit(&sheets, &job, Route::Vector, out.to_str(), &mut |_, _| {}).unwrap();
    assert!(receipt.contains("pixels"), "the receipt says a sheet went as pixels: {receipt}");
    assert_eq!(pages_with_images(written(&out)), [false, true]);
    // The image route draws both sheets as pictures.
    submit(&sheets, &job, Route::Image, out.to_str(), &mut |_, _| {}).unwrap();
    assert_eq!(pages_with_images(written(&out)), [true, true]);
}

/// The file the in-box PDF printer writes at `out` once the spooled job has printed, taken away so
/// the next job writes a new one.
fn written(out: &std::path::Path) -> Vec<u8> {
    let bytes = (0..120).find_map(|_| {
        let bytes = std::fs::read(out).ok().filter(|b| b.len() > 32 && b[b.len() - 32..].windows(5).any(|w| w == b"%%EOF"));
        if bytes.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        bytes
    });
    let _ = std::fs::remove_file(out);
    bytes.expect("the printer wrote its file")
}
