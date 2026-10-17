//! A named-layer signature appearance (layers n0–n4) in the real shell: the state drawn into it
//! follows trust and the interface language, the page redraws without a zoom change, and the page
//! badge stays for a signature that is not valid.

use std::sync::Arc;

use egui::{Pos2, Rect, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::SignatureStatus;
use pdfcraft_ui_egui::PdfCraftApp;

/// A 400×300 page whose unsigned signature field (object 4) carries the named layers in the
/// geometry of a real layered appearance scaled by 4; every word and drawing is synthetic. n1, a
/// yellow question mark, is drawn into the square (146, 106)–(254, 214) of the page.
fn fixture() -> Vec<u8> {
    fn form(bbox: &str, resources: &str, ops: &str) -> String {
        format!("<< /Type /XObject /Subtype /Form /BBox {bbox} /Resources {resources} /Length {} >>\nstream\n{ops}\nendstream", ops.len())
    }
    let helv = "<< /Font << /F1 13 0 R >> >>";
    let glyph = "1.08 0 0 1.08 46 6 cm";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents 14 0 R /Resources << /Font << /F1 13 0 R >> >> /Annots [4 0 R] >>"
            .to_string(),
        "<< /FT /Sig /T (Signature1) /Type /Annot /Subtype /Widget /F 132 /Rect [100 100 300 220] /P 3 0 R /AP << /N 6 0 R >> >>".to_string(),
        "<< /Fields [4 0 R] /SigFlags 3 >>".to_string(),
        form("[0 0 200 120]", "<< /XObject << /FRM 7 0 R >> >>", "q 1 0 0 1 0 0 cm /FRM Do Q"),
        form(
            "[0 0 200 120]",
            "<< /XObject << /n0 8 0 R /n1 9 0 R /n2 10 0 R /n3 11 0 R /n4 12 0 R >> >>",
            &format!("q /n0 Do Q q {glyph} /n1 Do Q q /n2 Do Q q {glyph} /n3 Do Q q /n4 Do Q"),
        ),
        form("[0 0 100 100]", "<< >>", "% DSBlank"),
        form(
            "[0 0 100 100]",
            "<< >>",
            "q 1 0.84 0.2 rg 0.55 0.4 0 RG 3 w 30 72 m 30 88 42 98 56 98 c 72 98 84 88 84 74 c 66 50 l 58 28 l 44 28 l 44 48 l h B 44 6 14 14 re B Q",
        ),
        form("[0 0 200 120]", helv, "BT /F1 13.6 Tf 1 0 0 1 8 62.4 Tm (Digitally signed by) Tj 1 0 0 1 8 48.8 Tm (Test Signer) Tj ET"),
        form("[0 0 100 100]", "<< >>", "% DSBlank"),
        form("[0 84 200 120]", helv, "BT /F1 18.8 Tf 1 0 0 1 8 93.2 Tm (Signature Not Verified) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 270 Td (Contract text) Tj ET\nendstream".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (k, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", k + 1).as_bytes());
    }
    let start = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in &offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF", objs.len() + 1).as_bytes());
    out
}

/// Sign the fixture's field without touching its appearance: reserve `/ByteRange` and
/// `/Contents`, hash the ranges and embed a detached CMS.
fn signed(id: &pdfcraft_engine::sign::pkcs12::DigitalId) -> Vec<u8> {
    use pdfcraft_cos::{Dict, ObjRef, Object, PdfString};
    const MARK: &str = "0 1111111111 2222222222 3333333333";
    const RESERVE: usize = 8192;
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture())).unwrap();
    let mut v = Dict::new();
    v.set(b"Type".to_vec(), Object::name("Sig"));
    v.set(b"SubFilter".to_vec(), Object::name("ETSI.CAdES.detached"));
    v.set(b"ByteRange".to_vec(), Object::Array([0, 1_111_111_111, 2_222_222_222, 3_333_333_333].map(Object::Int).to_vec()));
    v.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0; RESERVE], hex: true }));
    v.set(b"M".to_vec(), PdfString::literal(b"D:20261002120000+01'00'".to_vec()));
    let sig = doc.add(Object::Dict(v));
    doc.update_dict(ObjRef::new(4, 0), |d| d.set(b"V".to_vec(), Object::Ref(sig))).unwrap();
    let save = pdfcraft_cos::SaveOptions { object_streams: false, ..Default::default() };
    let mut out = pdfcraft_cos::write_incremental(&doc, &save).unwrap();
    let br = out.windows(MARK.len()).position(|w| w == MARK.as_bytes()).unwrap();
    let cstart = br + out[br..].windows(8).position(|w| w == b"<0000000").unwrap();
    let cend = cstart + RESERVE * 2 + 2;
    let mut range = format!("0 {cstart} {cend} {}", out.len() - cend).into_bytes();
    range.resize(MARK.len(), b' ');
    out[br..br + MARK.len()].copy_from_slice(&range);
    let alg = id.key.preferred_digest();
    let digest = alg.digest(&[&out[..cstart], &out[cend..]]);
    let cms = pdfcraft_engine::sign::cms::sign_detached(&id.key, &id.certificate, &id.chain, alg, &digest).unwrap();
    let hex: Vec<u8> = cms.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    out[cstart + 1..cstart + 1 + hex.len()].copy_from_slice(&hex);
    out
}

/// Let the page render finish.
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..400 {
        h.run_steps(1);
        if !h.state().render_pending() {
            h.run_steps(2);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the page never finished rendering");
}

/// The screen point of page point (x, y) on the 400×300 page.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    pos2(r.left() + x / 400.0 * r.width(), r.top() + (300.0 - y) / 300.0 * r.height())
}

/// How many rendered pixels in n1's square are within 40 of `rgb` on every channel.
fn in_n1_square(h: &mut Harness<'static, PdfCraftApp>, rgb: [u8; 3]) -> usize {
    let ppp = h.ctx.pixels_per_point();
    let square = Rect::from_two_pos(at(h, 146.0, 106.0), at(h, 254.0, 214.0));
    let image = h.render().unwrap();
    let (x0, y0, x1, y1) = ((square.left() * ppp) as u32, (square.top() * ppp) as u32, (square.right() * ppp) as u32, (square.bottom() * ppp) as u32);
    (y0..y1.min(image.height()))
        .flat_map(|y| (x0..x1.min(image.width())).map(move |x| (x, y)))
        .filter(|&(x, y)| image.get_pixel(x, y).0.iter().zip(rgb).all(|(a, b)| a.abs_diff(b) <= 40))
        .count()
}

/// The rendered pixels of n4's caption strip, (100, 184)–(300, 220) of the page.
fn caption_strip(h: &mut Harness<'static, PdfCraftApp>) -> Vec<u8> {
    let ppp = h.ctx.pixels_per_point();
    let strip = Rect::from_two_pos(at(h, 100.0, 184.0), at(h, 300.0, 220.0));
    let image = h.render().unwrap();
    let (x0, y0, x1, y1) = ((strip.left() * ppp) as u32, (strip.top() * ppp) as u32, (strip.right() * ppp) as u32, (strip.bottom() * ppp) as u32);
    (y0..y1.min(image.height())).flat_map(|y| (x0..x1.min(image.width())).map(move |x| (x, y))).flat_map(|(x, y)| image.get_pixel(x, y).0).collect()
}

/// How many nodes read `label` while the pointer rests on the signature: the badge's tooltip adds
/// one, a widget without a badge none.
fn hovered(h: &mut Harness<'static, PdfCraftApp>, label: &str) -> usize {
    h.hover_at(at(h, 120.0, 130.0));
    h.run_steps(3);
    let n = h.query_all_by_label(label).count();
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(2);
    n
}

#[test]
fn named_layers_follow_trust_and_language_and_keep_the_badge_until_valid() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let id = pdfcraft_engine::sign::pkcs12::open(&p12, "test").unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let bytes = signed(&id);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("named.pdf", None, bytes).expect("opens");
        app.set_option("left", "closed").unwrap();
        app
    });
    settle(&mut h);
    let (amber, green) = ([0xE6, 0x86, 0x19], [0x2D, 0x9D, 0x78]);
    let doc_id = h.state().views[0].id;
    // Untrusted: the appearance draws the unknown state, and the badge stays.
    assert_eq!(h.state().session.get(doc_id).unwrap().signatures[0].status, SignatureStatus::Unknown);
    assert!(in_n1_square(&mut h, amber) > 100, "n1 shows the unknown icon");
    assert!(
        hovered(&mut h, "Signature validity is unknown") > h.query_all_by_label("Signature validity is unknown").count(),
        "an unknown signature keeps its badge"
    );
    // Trusted: the same page, at the same zoom, redraws in the valid state, and the badge goes.
    let zoom = h.state().views[0].zoom;
    h.state_mut().apply_trusted_certificates(vec![anchor]);
    settle(&mut h);
    assert_eq!(h.state().session.get(doc_id).unwrap().signatures[0].status, SignatureStatus::Valid);
    assert_eq!(h.state().views[0].zoom, zoom);
    assert!(in_n1_square(&mut h, green) > 100 && in_n1_square(&mut h, amber) < 10, "n1 redraws in the valid state");
    assert_eq!(
        hovered(&mut h, "Signature is valid"),
        h.query_all_by_label("Signature is valid").count(),
        "the appearance shows the verdict: no badge"
    );
    // The interface language changes the caption, on the page as well as in the document.
    let english = caption_strip(&mut h);
    h.state_mut().set_option("language", "de").unwrap();
    settle(&mut h);
    assert_ne!(caption_strip(&mut h), english, "the page redraws the caption");
    let text = pdfcraft_engine::export::Exporter::new(h.state().session.get(doc_id).unwrap()).text_of(&[0]).unwrap();
    assert!(text.contains("Signatur gültig"), "{text:?}");
}
