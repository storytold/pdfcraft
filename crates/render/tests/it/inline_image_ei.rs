//! Regression tests for inline images whose unfiltered samples contain `"EI "` (#3107).

use std::fmt::Write as _;
use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

fn build_pdf(pages: &[Vec<u8>], extra_objects: &[(usize, Vec<u8>)], page_xobjects: &str) -> Vec<u8> {
    let n = pages.len();
    let font = 3 + 2 * n;
    let mut objs: Vec<(usize, Vec<u8>)> = Vec::new();
    objs.push((1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()));
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    objs.push((2, format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes()));
    for (i, content) in pages.iter().enumerate() {
        let page_num = 3 + 2 * i;
        let stream_num = 4 + 2 * i;
        objs.push((
            page_num,
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {stream_num} 0 R /Resources << /Font << /F1 {font} 0 R >> {page_xobjects} >> >>")
                .into_bytes(),
        ));
        let mut stm = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stm.extend_from_slice(content);
        stm.extend_from_slice(b"\nendstream");
        objs.push((stream_num, stm));
    }
    objs.push((font, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()));
    for (id, body) in extra_objects {
        objs.push((*id, body.clone()));
    }
    objs.sort_by_key(|(id, _)| *id);

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::with_capacity(objs.len());
    for (id, body) in &objs {
        offs.push(out.len());
        out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = out.len();
    let size = objs.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for off in offs {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
    out
}

fn page_stream(image: &[u8], n: usize) -> Vec<u8> {
    let mut s = b"q 200 0 0 200 100 300 cm ".to_vec();
    s.extend_from_slice(image);
    s.extend_from_slice(format!(" Q BT /F1 18 Tf 72 700 Td (After image P{n}) Tj ET").as_bytes());
    s
}

fn mean_grey_in_box(rgba: &[u8], width: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
    let mut sum = 0u64;
    let mut count = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = ((y * width + x) * 4) as usize;
            sum += u64::from(rgba[idx]);
            count += 1;
        }
    }
    sum as f64 / count.max(1) as f64
}

fn dark_pixels_in_box(rgba: &[u8], width: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
    let mut dark = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = ((y * width + x) * 4) as usize;
            if rgba[idx] < 128 && rgba[idx + 1] < 128 && rgba[idx + 2] < 128 {
                dark += 1;
            }
        }
    }
    dark
}

#[test]
fn unfiltered_inline_image_samples_containing_ei_are_not_executed_as_content() {
    let samples = b"0EI Q BT /F1 18 Tf 72 600 Td (Injected) Tj ET q";
    let w = samples.len();
    let mut hexed = String::with_capacity(w * 2 + 1);
    for b in samples {
        let _ = write!(hexed, "{b:02X}");
    }
    hexed.push('>');

    let mut img1 = format!("BI /W {w} /H 1 /BPC 8 /CS /G /L {w} ID ").into_bytes();
    img1.extend_from_slice(samples);
    img1.extend_from_slice(b" EI");

    let mut img2 = format!("BI /W {w} /H 1 /BPC 8 /CS /G /L {w} ID 0AB").into_bytes();
    img2.extend_from_slice(&samples[3..]);
    img2.extend_from_slice(b" EI");

    let img3 = format!("BI /W {w} /H 1 /BPC 8 /CS /G /F /AHx ID {hexed} EI").into_bytes();
    let img4 = b"BI /W 4 /H 1 /BPC 8 /CS /G ID \x20\x60\xa0\xe0 EI".to_vec();

    let pdf = Arc::new(build_pdf(&[page_stream(&img1, 1), page_stream(&img2, 2), page_stream(&img3, 3), page_stream(&img4, 4)], &[], ""));

    let mut renderer = PageRenderer::new(Arc::clone(&pdf), RenderConfig::default());
    let mut greys = Vec::new();
    for page_idx in 0..4 {
        let text_page = renderer.render(RenderRequest { page: page_idx, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
        assert!(text_page.error.is_none(), "page {} text error: {:?}", page_idx + 1, text_page.error);
        let text = text_page.text.expect("page text");
        let plain = text.plain_text();
        assert_eq!(plain.trim(), format!("After image P{}", page_idx + 1), "page {}", page_idx + 1);
        assert!(text.find("Injected").is_empty(), "page {} must not match Injected in Find", page_idx + 1);

        let rendered = renderer.render(RenderRequest { page: page_idx, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
        assert!(rendered.error.is_none(), "page {} render error: {:?}", page_idx + 1, rendered.error);

        let dark = dark_pixels_in_box(&rendered.rgba, rendered.width, 72, 172, 200, 196);
        assert_eq!(dark, 0, "page {} must not draw Injected at (72, 600)", page_idx + 1);

        let grey = mean_grey_in_box(&rendered.rgba, rendered.width, 110, 302, 290, 482);
        greys.push(grey);
    }

    // Page 1 and Page 3 have the exact same 47 samples (unfiltered vs ASCIIHexDecode).
    assert!((greys[0] - greys[2]).abs() < 1.0, "page 1 mean grey ({}) must match page 3 ({})", greys[0], greys[2]);
    assert!((greys[0] - greys[1]).abs() < 2.0, "page 1 mean grey ({}) must match control page 2 ({})", greys[0], greys[1]);
    assert!((greys[3] - 128.0).abs() < 2.0, "page 4 control mean grey ({})", greys[3]);
}

#[test]
fn form_xobject_inline_image_with_ei_in_samples_keeps_all_samples() {
    let form_content = b"q 200 0 0 200 100 300 cm BI /W 12 /H 1 /BPC 8 /CS /G ID 0EI 1 0 0 rg EI Q BT /F1 18 Tf 72 700 Td (Form after) Tj ET";
    let mut form_obj = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Length {} >>\nstream\n",
        form_content.len()
    )
    .into_bytes();
    form_obj.extend_from_slice(form_content);
    form_obj.extend_from_slice(b"\nendstream");

    let page_content = b"/Fm1 Do".to_vec();
    // 1 page => font is object 5, form XObject is object 6.
    let pdf = Arc::new(build_pdf(&[page_content], &[(6, form_obj)], "/XObject << /Fm1 6 0 R >>"));

    let mut renderer = PageRenderer::new(Arc::clone(&pdf), RenderConfig::default());
    let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);

    let grey = mean_grey_in_box(&rendered.rgba, rendered.width, 110, 302, 290, 482);
    assert!((grey - 54.37).abs() < 1.0, "form inline image mean grey {grey} must be ~54.37 (not ~2 from 1-byte cut)");
}

#[test]
fn unfiltered_inline_image_layouts_and_mismatched_l_do_not_inject_content() {
    let inject_prefix = b"0EI Q BT /F1 18 Tf 72 600 Td (Injected) Tj ET q";
    let make_image = |params: &str, len: usize| {
        let mut samples = inject_prefix.to_vec();
        samples.resize(len, b'0');
        let mut img = format!("BI {params} ID ").into_bytes();
        img.extend_from_slice(&samples);
        img.extend_from_slice(b" EI");
        img
    };

    let cases = [
        ("/W 16 /H 2 /CS /RGB /BPC 8", 96),
        ("/W 12 /H 2 /CS /CMYK /BPC 8", 96),
        ("/W 70 /H 4 /CS /G /BPC 1", 36),
        ("/IM true /W 70 /H 4", 36),
        ("/W 47 /H 1 /CS /G /BPC 8 /L 5", 47),
    ];

    let pages: Vec<Vec<u8>> = cases.iter().enumerate().map(|(i, (params, len))| page_stream(&make_image(params, *len), i + 1)).collect();

    let pdf = Arc::new(build_pdf(&pages, &[], ""));
    let mut renderer = PageRenderer::new(Arc::clone(&pdf), RenderConfig::default());

    for (i, (params, _)) in cases.iter().enumerate() {
        let page_idx = i;
        let text_page = renderer.render(RenderRequest { page: page_idx, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
        assert!(text_page.error.is_none(), "case {params} text error: {:?}", text_page.error);
        let text = text_page.text.expect("page text");
        let plain = text.plain_text();
        assert_eq!(plain.trim(), format!("After image P{}", i + 1), "case {params}");
        assert!(text.find("Injected").is_empty(), "case {params} must not match Injected in Find");

        let rendered = renderer.render(RenderRequest { page: page_idx, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
        assert!(rendered.error.is_none(), "case {params} render error: {:?}", rendered.error);

        let dark = dark_pixels_in_box(&rendered.rgba, rendered.width, 72, 172, 200, 196);
        assert_eq!(dark, 0, "case {params} must not draw Injected at (72, 600)");
    }
}
