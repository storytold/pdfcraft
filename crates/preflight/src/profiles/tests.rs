use std::cell::RefCell;
use std::collections::BTreeSet;

use pdfcraft_cos::{Document, ObjRef, Object};

use super::*;

/// A one-page file: catalog 1, pages 2, page 3, content 4, then `more` from object 5.
struct Pdf {
    header: &'static str,
    catalog: String,
    page: String,
    resources: String,
    content: String,
    content_dict: String,
    more: Vec<String>,
    trailer: String,
}

impl Pdf {
    fn new(content: &str) -> Pdf {
        Pdf {
            header: "%PDF-1.7",
            catalog: String::new(),
            page: String::new(),
            resources: String::new(),
            content: content.into(),
            content_dict: String::new(),
            more: Vec::new(),
            trailer: String::new(),
        }
    }

    fn catalog(mut self, s: &str) -> Pdf {
        self.catalog = s.into();
        self
    }

    fn page(mut self, s: &str) -> Pdf {
        self.page = s.into();
        self
    }

    fn resources(mut self, s: &str) -> Pdf {
        self.resources = s.into();
        self
    }

    fn content_dict(mut self, s: &str) -> Pdf {
        self.content_dict = s.into();
        self
    }

    fn header(mut self, h: &'static str) -> Pdf {
        self.header = h;
        self
    }

    fn trailer(mut self, s: &str) -> Pdf {
        self.trailer = s.into();
        self
    }

    /// The next object, numbered from 5.
    fn obj(mut self, s: &str) -> Pdf {
        self.more.push(s.into());
        self
    }

    fn open(&self) -> Document {
        let mut objs = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {} >>", self.catalog),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << {} >> {} >>", self.resources, self.page),
            stream(&self.content_dict, &self.content),
        ];
        objs.extend(self.more.iter().cloned());
        let mut out = format!("{}\n", self.header).into_bytes();
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
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1, self.trailer).as_bytes());
        Document::open(std::sync::Arc::new(out)).unwrap()
    }
}

fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// An image XObject whose pixels aren't needed.
fn image(dict: &str) -> String {
    stream(&format!("/Type /XObject /Subtype /Image {dict}"), "")
}

fn run_one(doc: &Document, check: Check) -> CheckResult {
    let profile =
        Profile { id: "t".into(), name: "t".into(), description: String::new(), checks: vec![CheckSpec { check, severity: Severity::Error }] };
    run(doc, &profile, &NoHost).checks.remove(0)
}

fn messages(r: &CheckResult) -> Vec<&str> {
    r.hits.iter().map(|h| h.message.as_str()).collect()
}

fn defaults(id: &str) -> Check {
    single_checks().into_iter().find(|p| p.id == format!("check:{id}")).unwrap().checks.remove(0).check
}

#[test]
fn profiles_are_data_that_round_trip_through_json() {
    for p in builtin().into_iter().chain(single_checks()) {
        p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.id));
        assert_eq!(Profile::from_json(&p.to_json()).unwrap(), p, "{}", p.id);
        // The JSON tag is the check's id.
        let json: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
        for (spec, written) in p.checks.iter().zip(json["checks"].as_array().unwrap()) {
            assert_eq!(written["check"], spec.check.id(), "{}", p.id);
        }
    }
    // Written by hand: whole numbers for the resolutions, a level by its label or short id.
    let custom = Profile::from_json(
        r#"{ "id": "shop", "name": "Our press", "checks": [
            { "check": "image-resolution", "severity": "error", "colour_ppi": 250, "gray_ppi": 250, "bitmap_ppi": 800 },
            { "check": "pdfx", "severity": "warning", "level": "x4" },
            { "check": "colour-spaces", "severity": "error", "forbid": ["rgb", "spot"] },
            { "check": "version", "severity": "info", "max": "1.6" }
        ] }"#,
    )
    .unwrap();
    assert_eq!(custom.checks[0].check, Check::ImageResolution { colour_ppi: 250.0, gray_ppi: 250.0, bitmap_ppi: 800.0 });
    assert_eq!(custom.checks[1].check, Check::PdfX { level: crate::pdfx::Level::X4 });
    assert_eq!(custom.checks[1].severity, Severity::Warning);
    assert_eq!(custom.description, "");
    // What a profile from outside may get wrong is refused, not run.
    for bad in [
        r#"{ "id": "x", "name": "x", "checks": [] }"#,
        r#"{ "id": "", "name": "x", "checks": [{ "check": "encryption", "severity": "error" }] }"#,
        r#"{ "id": "x", "name": "x", "checks": [{ "check": "teleport", "severity": "error" }] }"#,
        r#"{ "id": "x", "name": "x", "checks": [{ "check": "encryption", "severity": "fatal" }] }"#,
        r#"{ "id": "x", "name": "x", "checks": [{ "check": "image-resolution", "severity": "error", "colour_ppi": -1, "gray_ppi": 1, "bitmap_ppi": 1 }] }"#,
        r#"{ "id": "x", "name": "x", "checks": [{ "check": "hairlines", "severity": "error", "min_width_pt": 50 }] }"#,
        r#"{ "id": "x", "name": "x", "checks": [{ "check": "version", "severity": "error", "max": "one" }] }"#,
        r#"{ "id": "x", "name": "x", "checks": [{ "check": "pdfx", "severity": "error", "level": "x9" }] }"#,
        "not json",
    ] {
        assert!(Profile::from_json(bad).is_err(), "{bad}");
    }
    assert!(find("commercial-print").is_some() && find("pdfx-1a-2003").is_some() && find("pdfa-2b").is_some() && find("check:hairlines").is_some());
    assert_eq!(find("pdfx-4").map(|p| p.name), Some("PDF/X-4".to_string()));
}

#[test]
fn fonts_not_embedded_are_listed_with_the_page_that_uses_them() {
    let doc = Pdf::new("BT /F1 12 Tf (a) Tj /F2 12 Tf (b) Tj ET")
        .resources("/Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R /F4 10 0 R >>")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj("<< /Type /Font /Subtype /TrueType /BaseFont /Arial /FontDescriptor 8 0 R >>")
        .obj("<< /Type /Font /Subtype /Type0 /BaseFont /Ryumin /Encoding /Identity-H /DescendantFonts [11 0 R] >>")
        .obj("<< /Type /FontDescriptor /FontName /Arial /FontFile2 9 0 R >>")
        .obj(&stream("", ""))
        .obj("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << >> /Encoding << >> >>")
        .obj("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Ryumin /FontDescriptor 12 0 R >>")
        .obj("<< /Type /FontDescriptor /FontName /Ryumin >>")
        .open();
    let r = run_one(&doc, Check::FontsEmbedded);
    assert_eq!(messages(&r), ["The font Helvetica isn't embedded", "The font Ryumin isn't embedded"], "{r:#?}");
    // Helvetica is shown on page 1; the Type 0 font is only listed in the resources.
    assert_eq!((r.hits[0].page, r.hits[0].object), (Some(0), Some(5)));
    assert_eq!((r.hits[1].page, r.hits[1].object), (None, Some(7)));
    assert_eq!(r.hits[0].details, [("Font type", "Type1".to_string())]);
    assert_eq!(r.status, Status::Found);
}

/// Content in RGB, Lab, ICC-based RGB and CMYK, a spot colour, process CMYK, a shading and an
/// RGB image.
fn colourful() -> Document {
    Pdf::new(
        "1 0 0 rg 0 0 10 10 re f 0 0 0 1 k 0 0 10 10 re f /CS0 cs 50 0 0 sc /CS1 cs 0 0 0 1 sc /CS2 cs 1 scn /CS3 CS 1 0 0 SCN \
         /CS4 cs 1 1 scn /CS5 CS 1 SCN /CS6 cs 1 scn /Sh0 sh q 100 0 0 100 0 0 cm /Im Do Q",
    )
    .resources(
        "/ColorSpace << /CS0 [/Lab << /WhitePoint [0.95 1 1.09] >>] /CS1 [/ICCBased 5 0 R] /CS2 [/Separation /PANTONE#20485#20C /DeviceCMYK 6 0 R] \
         /CS3 [/ICCBased 7 0 R] /CS4 [/DeviceN [/PANTONE#20485#20C /Cyan] /DeviceCMYK 6 0 R] /CS5 [/Separation /All /DeviceCMYK 6 0 R] \
         /CS6 [/Separation /Reflex#20Blue /DeviceCMYK 6 0 R] /Unused [/Separation /Gold /DeviceCMYK 6 0 R] >> \
         /Shading << /Sh0 << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function 6 0 R >> >> /XObject << /Im 8 0 R >>",
    )
    .obj(&stream("/N 4", ""))
    .obj("<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0.5 1 0] /N 1 >>")
    .obj(&stream("/N 3", ""))
    .obj(&image("/Width 10 /Height 10 /ColorSpace /DeviceRGB /BitsPerComponent 8"))
    .open()
}

#[test]
fn rgb_and_lab_are_refused_in_a_cmyk_job() {
    let doc = colourful();
    let r = run_one(&doc, Check::ColourSpaces { forbid: vec![Space::Rgb, Space::Lab] });
    let got: BTreeSet<&str> = messages(&r).into_iter().collect();
    let want: BTreeSet<&str> = [
        "Content uses DeviceRGB colour",
        "Content uses Lab colour",
        "Content uses ICCBased RGB colour",
        "A shading uses DeviceRGB colour",
        "An image uses DeviceRGB colour",
    ]
    .into_iter()
    .collect();
    assert_eq!(got, want, "{r:#?}");
    let image = r.hits.iter().find(|h| h.message.starts_with("An image")).unwrap();
    assert_eq!((image.page, image.object, image.bbox), (Some(0), Some(8), Some([0.0, 0.0, 100.0, 100.0])));
    // ICC-based colour of any kind, and spot colour, when a profile refuses those.
    let r = run_one(&doc, Check::ColourSpaces { forbid: vec![Space::Icc] });
    assert_eq!(
        messages(&r).into_iter().collect::<BTreeSet<_>>(),
        ["Content uses ICCBased CMYK colour", "Content uses ICCBased RGB colour"].into_iter().collect()
    );
    let r = run_one(&doc, Check::ColourSpaces { forbid: vec![Space::Spot] });
    assert_eq!(
        messages(&r).into_iter().collect::<BTreeSet<_>>(),
        [
            "Content uses Separation PANTONE 485 C colour",
            "Content uses DeviceN PANTONE 485 C, Cyan colour",
            "Content uses Separation Reflex Blue colour"
        ]
        .into_iter()
        .collect(),
        "{r:#?}"
    );
    // CMYK passes a CMYK job.
    assert_eq!(run_one(&doc, Check::ColourSpaces { forbid: vec![Space::Gray] }).status, Status::Passed);
}

#[test]
fn spot_colours_are_listed_once_each() {
    let doc = colourful();
    let r = run_one(&doc, Check::SpotColours { max: None });
    // Process inks, `All` and a space no content selects are not spot colours on the pages.
    assert_eq!(messages(&r), ["Spot colour PANTONE 485 C", "Spot colour Reflex Blue"], "{r:#?}");
    assert_eq!(r.hits[0].page, Some(0));
    let r = run_one(&doc, Check::SpotColours { max: Some(1) });
    assert_eq!(r.hits[0].message, "The pages use 2 spot colours");
    assert_eq!(r.hits[0].details, [("Most spot colours", "1".to_string())]);
    assert_eq!(run_one(&doc, Check::SpotColours { max: Some(2) }).hits.len(), 2);
}

#[test]
fn image_resolution_is_measured_where_each_image_is_drawn() {
    let doc = Pdf::new(
        "q 144 0 0 144 300 10 cm /Im1 Do Q q 216 0 0 216 10 10 cm /Im1 Do Q q 216 0 0 216 10 300 cm /Im2 Do Q q 216 0 0 216 300 300 cm /Im3 Do Q \
         q 216 0 0 216 10 500 cm /Im4 Do Q /Fm Do q 72 0 0 72 400 600 cm BI /W 2 /H 2 /CS /RGB /BPC 8 ID abcdefghijkl EI Q \
         q 0 0 0 0 5 5 cm /Im6 Do Q",
    )
    .resources("/XObject << /Im1 5 0 R /Im2 6 0 R /Im3 7 0 R /Im4 8 0 R /Fm 9 0 R /Im6 11 0 R >>")
    .obj(&image("/Width 300 /Height 300 /ColorSpace /DeviceRGB /BitsPerComponent 8"))
    .obj(&image("/Width 900 /Height 900 /ColorSpace /DeviceGray /BitsPerComponent 8"))
    .obj(&image("/Width 1200 /Height 1200 /ColorSpace /DeviceGray /BitsPerComponent 1"))
    .obj(&image("/Width 3600 /Height 3600 /ImageMask true"))
    .obj(&stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Matrix [0.5 0 0 0.5 0 0] /Resources << /XObject << /Im5 10 0 R >> >>",
        "q 216 0 0 216 0 0 cm /Im5 Do Q",
    ))
    .obj(&image("/Width 300 /Height 300 /ColorSpace /DeviceRGB /BitsPerComponent 8"))
    .obj(&image("/Width 10 /Height 10 /ColorSpace /DeviceRGB /BitsPerComponent 8"))
    .open();
    let r = run_one(&doc, defaults("image-resolution"));
    assert_eq!(
        messages(&r),
        [
            // Drawn twice: listed once, where it is drawn largest (lowest resolution).
            "A colour image is drawn at 100 ppi",
            // 900 px across 3 in is 300 ppi, which is enough; a one-bit image needs 1200.
            "A bitmap image is drawn at 400 ppi",
            // The form draws its image at half size: twice the resolution.
            "A colour image is drawn at 200 ppi",
            "A colour image is drawn at 2 ppi",
            "The resolution of an image can't be measured",
        ],
        "{r:#?}"
    );
    let first = &r.hits[0];
    assert_eq!((first.object, first.bbox), (Some(5), Some([10.0, 10.0, 226.0, 226.0])));
    assert_eq!(
        first.details,
        [("Pixels", "300 × 300".to_string()), ("Placed size", "76.2 × 76.2 mm".to_string()), ("Least resolution", "300 ppi".to_string())]
    );
    assert_eq!(r.hits[2].object, Some(10));
    assert_eq!(r.hits[3].object, None, "an inline image has no object");
    // Lower thresholds pass them all but the unmeasurable one.
    let r = run_one(&doc, Check::ImageResolution { colour_ppi: 1.0, gray_ppi: 1.0, bitmap_ppi: 1.0 });
    assert_eq!(messages(&r), ["The resolution of an image can't be measured"]);
}

#[test]
fn hairlines_are_measured_where_they_are_drawn() {
    let doc = Pdf::new(
        "0.1 w 10 10 m 100 10 l S 0 w 10 20 m 100 20 l S 1 w q 0.1 0 0 0.1 0 0 cm 100 300 m 1000 300 l S Q 2 w 10 40 m 100 40 l S \
         q 0.1 0 0 0.1 0 0 cm /Fm Do Q /GS1 gs 10 60 m 100 60 l S 1 w 0.3 w 10 70 100 10 re B",
    )
    .resources("/XObject << /Fm 5 0 R >> /ExtGState << /GS1 << /LW 0.05 >> >>")
    .obj(&stream("/Type /XObject /Subtype /Form /BBox [0 0 1000 1000]", "1 w 0 0 m 100 0 l S"))
    .open();
    let r = run_one(&doc, defaults("hairlines"));
    assert_eq!(
        messages(&r),
        [
            "A line is 0.1 pt wide",
            "A line has width 0, the thinnest the printer can draw",
            "A line is 0.1 pt wide",
            "A line is 0.1 pt wide",
            "A line is 0.05 pt wide"
        ],
        "{r:#?}"
    );
    assert_eq!(r.hits[0].bbox, Some([10.0, 10.0, 100.0, 10.0]));
    assert_eq!(r.hits[0].details, [("Line width", "0.1 pt".to_string())]);
    assert_eq!(r.hits[3].object, Some(5), "drawn in the form");
    assert!(r.hits.iter().all(|h| h.fixup.is_none()), "hairlines are reported, not fixed");
    // A thinner limit lets the 0.1 pt lines through.
    let r = run_one(&doc, Check::Hairlines { min_width_pt: 0.08 });
    assert_eq!(messages(&r), ["A line has width 0, the thinnest the printer can draw", "A line is 0.05 pt wide"]);
    // Many lines on a page: the first fifty one by one, the rest counted.
    let doc = Pdf::new(&"0 w 0 0 m 1 1 l S ".repeat(60)).open();
    let r = run_one(&doc, defaults("hairlines"));
    assert_eq!(r.hits.len(), 51);
    assert_eq!(r.hits[50].message, "… and 10 more on this page");
}

#[test]
fn transparency_is_what_the_pages_use() {
    let doc = Pdf::new("/GSa gs /GSb gs /GSc gs q 10 0 0 10 0 0 cm /Im Do Q /Fm Do")
        .page("/Group << /S /Transparency /CS /DeviceRGB >>")
        .resources("/ExtGState << /GSa << /ca 0.5 >> /GSb << /BM /Multiply >> /GSc 5 0 R /Unused << /CA 0.2 >> >> /XObject << /Im 6 0 R /Fm 7 0 R >>")
        .obj("<< /Type /ExtGState /SMask << /S /Luminosity /G 7 0 R >> >>")
        .obj(&image("/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 8 0 R"))
        .obj(&stream("/Type /XObject /Subtype /Form /BBox [0 0 50 50] /Group << /S /Transparency >>", ""))
        .obj(&image("/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8"))
        .open();
    let r = run_one(&doc, Check::Transparency);
    assert_eq!(
        messages(&r),
        [
            "The page is a transparency group",
            "A graphics state sets an opacity below 1",
            "A graphics state uses the Multiply blend mode",
            "A graphics state uses a soft mask",
            "An image has a soft mask",
            "A form XObject is a transparency group",
        ],
        "{r:#?}"
    );
    assert_eq!(r.hits[3].object, Some(5));
    assert_eq!((r.hits[5].object, r.hits[5].bbox), (Some(7), Some([0.0, 0.0, 50.0, 50.0])));
    // An opaque page passes.
    assert_eq!(run_one(&Pdf::new("0 0 10 10 re f").open(), Check::Transparency).status, Status::Passed);
}

#[test]
fn a_missing_trim_box_is_set_from_the_crop_box() {
    let mut doc = Pdf::new("").page("/CropBox [10 10 590 790]").open();
    let r = run_one(&doc, Check::TrimBox);
    assert_eq!(messages(&r), ["The page has neither a TrimBox nor an ArtBox"]);
    assert_eq!((r.hits[0].page, r.hits[0].object, r.hits[0].fixup), (Some(0), Some(3), Some(Fixup::SetTrimBox)));
    assert_eq!(fix(&mut doc, Fixup::SetTrimBox).unwrap(), 1);
    let page = doc.get(ObjRef::new(3, 0));
    let trim: Vec<f64> = page.as_dict().unwrap().get(b"TrimBox").unwrap().as_array().unwrap().iter().filter_map(Object::as_f64).collect();
    assert_eq!(trim, [10.0, 10.0, 590.0, 790.0]);
    assert_eq!(run_one(&doc, Check::TrimBox).status, Status::Passed);
    assert_eq!(fix(&mut doc, Fixup::SetTrimBox).unwrap(), 0, "nothing left to fix");
    // A BleedBox narrower than the page narrows the trim; an ArtBox is left alone.
    let mut doc = Pdf::new("").page("/BleedBox [20 20 580 780]").open();
    fix(&mut doc, Fixup::SetTrimBox).unwrap();
    let page = doc.get(ObjRef::new(3, 0));
    let trim: Vec<f64> = page.as_dict().unwrap().get(b"TrimBox").unwrap().as_array().unwrap().iter().filter_map(Object::as_f64).collect();
    assert_eq!(trim, [20.0, 20.0, 580.0, 780.0]);
    let mut doc = Pdf::new("").page("/ArtBox [20 20 580 780]").open();
    assert_eq!(fix(&mut doc, Fixup::SetTrimBox).unwrap(), 0);
    assert_eq!(run_one(&doc, Check::TrimBox).status, Status::Passed);
}

#[test]
fn bleed_needs_a_bleed_box_wide_enough() {
    let r = run_one(&Pdf::new("").page("/TrimBox [20 20 580 780]").open(), defaults("bleed"));
    assert_eq!(messages(&r), ["The page has no BleedBox"]);
    // 1 mm (2.835 pt) of bleed on the left; 9 pt elsewhere.
    let r = run_one(&Pdf::new("").page("/TrimBox [20 20 580 780] /BleedBox [17.165 11 589 789]").open(), defaults("bleed"));
    assert_eq!(messages(&r), ["The bleed is 1.0 mm on its narrowest side"], "{r:#?}");
    assert_eq!(r.hits[0].details, [("Least bleed", "3 mm".to_string())]);
    // 9 pt is 3.175 mm; a BleedBox past the MediaBox counts only as far as the media.
    assert_eq!(run_one(&Pdf::new("").page("/TrimBox [20 20 580 780] /BleedBox [11 11 589 789]").open(), defaults("bleed")).status, Status::Passed);
    let r = run_one(&Pdf::new("").page("/TrimBox [2 20 580 780] /BleedBox [-20 11 589 789]").open(), defaults("bleed"));
    assert_eq!(messages(&r), ["The bleed is 0.7 mm on its narrowest side"]);
    // No trim to measure from: left to the TrimBox check.
    assert_eq!(run_one(&Pdf::new("").open(), defaults("bleed")).status, Status::Passed);
}

#[test]
fn encryption_is_reported_with_its_security_handler() {
    let mut doc = Pdf::new("").obj("<< /Filter /Standard /V 2 /R 3 /Length 128 >>").open();
    assert_eq!(run_one(&doc, Check::Encryption).status, Status::Passed);
    doc.trailer_mut().set(b"Encrypt".to_vec(), Object::Ref(ObjRef::new(5, 0)));
    let r = run_one(&doc, Check::Encryption);
    assert_eq!(messages(&r), ["The document is encrypted"]);
    assert_eq!(r.hits[0].details, [("Security handler", "Standard V 2 R 3".to_string())]);
    assert_eq!(r.hits[0].object, Some(5));
}

/// Scripts and actions in every place a file holds them, beside a link that only goes somewhere.
fn scripted() -> Document {
    Pdf::new("")
        .catalog("/OpenAction 5 0 R /Names << /JavaScript << /Names [(init) 6 0 R] >> >> /AA << /WC 6 0 R >> /Outlines 12 0 R")
        .page("/AA << /O 6 0 R >> /Annots [7 0 R 8 0 R 9 0 R 10 0 R]")
        .obj("<< /Type /Action /S /JavaScript /JS (app.alert(1)) >>")
        .obj("<< /Type /Action /S /JavaScript /JS (init()) >>")
        .obj("<< /Type /Annot /Subtype /Link /Rect [10 10 50 30] /A << /S /JavaScript /JS (go()) >> >>")
        .obj("<< /Type /Annot /Subtype /Link /Rect [60 10 100 30] /A << /S /URI /URI (https://example.com) >> >>")
        .obj("<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [10 40 100 60] /AA << /K 6 0 R >> >>")
        .obj("<< /Type /Annot /Subtype /Link /Rect [10 70 50 90] /A << /S /GoTo /D [3 0 R /Fit] /Next 11 0 R >> >>")
        .obj("<< /Type /Action /S /Launch /F (calc.exe) >>")
        .obj("<< /Type /Outlines /First 13 0 R /Last 13 0 R /Count 1 >>")
        .obj("<< /Title (Start) /Parent 12 0 R /A << /S /JavaScript /JS (y()) >> >>")
        .obj("<< /FT /Tx /T (parent) /Kids [] /AA << /F 6 0 R >> >>")
        .open()
}

#[test]
fn scripts_and_actions_are_reported_where_they_are_held() {
    let r = run_one(&scripted(), Check::Scripts);
    let got: Vec<(&str, Option<usize>, Option<u32>)> = r.hits.iter().map(|h| (h.message.as_str(), h.page, h.object)).collect();
    assert_eq!(
        got,
        [
            ("The document has additional actions (/AA)", None, Some(1)),
            ("The document has document-level JavaScript", None, Some(1)),
            ("A JavaScript action isn't allowed", None, Some(1)),
            ("Page 1 has additional actions", Some(0), Some(3)),
            ("A JavaScript action isn't allowed", Some(0), Some(7)),
            ("A Widget annotation has additional actions", Some(0), Some(9)),
            ("A Launch action isn't allowed", Some(0), Some(10)),
            ("A JavaScript action isn't allowed", None, Some(13)),
            ("A form field has additional actions", None, Some(14)),
        ],
        "{r:#?}"
    );
    assert!(r.hits.iter().all(|h| h.fixup == Some(Fixup::RemoveScripts)));
    assert_eq!(r.hits[4].bbox, Some([10.0, 10.0, 50.0, 30.0]));
}

#[test]
fn removing_scripts_keeps_links_that_only_go_somewhere() {
    let mut doc = scripted();
    assert!(fix(&mut doc, Fixup::RemoveScripts).unwrap() > 0);
    assert_eq!(run_one(&doc, Check::Scripts).status, Status::Passed);
    let get = |n: u32| doc.get(ObjRef::new(n, 0));
    // The web link keeps its action; the script and program actions are emptied.
    assert_eq!(get(8).as_dict().and_then(|d| d.get(b"A")).and_then(Object::as_dict).and_then(|a| a.name(b"S")), Some(b"URI".as_slice()));
    for n in [5, 6, 11] {
        assert!(matches!(&*get(n), Object::Null), "object {n}: {:?}", get(n));
    }
    // A chain with a program in it goes whole.
    assert!(get(10).as_dict().is_some_and(|d| !d.contains(b"A")));
    let cat = get(1);
    let cat = cat.as_dict().unwrap();
    assert!(!cat.contains(b"OpenAction") && !cat.contains(b"AA"));
    assert!(cat.get(b"Names").and_then(Object::as_dict).is_some_and(|n| !n.contains(b"JavaScript")));
    // PDF/X has nothing left to say about scripts or actions either.
    let left: Vec<&str> = crate::pdfx::verify(&doc, crate::pdfx::Level::X4).iter().map(|f| f.rule).collect();
    assert!(!left.contains(&"pdfx.javascript") && !left.contains(&"pdfx.actions"), "{left:?}");
    assert_eq!(fix(&mut doc, Fixup::RemoveScripts).unwrap(), 0, "nothing left to remove");
}

#[test]
fn annotations_inside_the_trim_are_listed_with_their_flags() {
    let doc = Pdf::new("")
        .page("/TrimBox [20 20 580 780] /Annots [5 0 R 6 0 R 7 0 R << /Type /Annot /Subtype /FreeText /Rect [300 300 400 350] /F 2 >>]")
        .obj("<< /Type /Annot /Subtype /Square /Rect [100 100 200 200] /F 4 >>")
        .obj("<< /Type /Annot /Subtype /Text /Rect [5 5 15 15] >>")
        .obj("<< /Type /Annot /Subtype /Popup /Rect [100 100 200 200] >>")
        .open();
    let r = run_one(&doc, Check::AnnotationsInTrim);
    assert_eq!(messages(&r), ["A Square annotation lies inside the trim", "A FreeText annotation lies inside the trim"], "{r:#?}");
    assert_eq!((r.hits[0].page, r.hits[0].object, r.hits[0].bbox), (Some(0), Some(5), Some([100.0, 100.0, 200.0, 200.0])));
    assert_eq!(r.hits[0].details, [("Flags", "Print".to_string())]);
    assert_eq!(r.hits[1].details, [("Flags", "Hidden".to_string())]);
}

#[test]
fn the_version_is_listed_or_held_to_a_maximum() {
    let doc = Pdf::new("").open();
    assert_eq!(messages(&run_one(&doc, Check::Version { max: None })), ["The file is PDF 1.7"]);
    assert_eq!(run_one(&doc, Check::Version { max: Some("1.7".into()) }).status, Status::Passed);
    let r = run_one(&doc, Check::Version { max: Some("1.6".into()) });
    assert_eq!(messages(&r), ["The file is PDF 1.7, newer than the profile allows"]);
    assert_eq!(r.hits[0].details, [("Highest version", "1.6".to_string())]);
    // The catalog's /Version wins when it is newer than the header.
    let doc = Pdf::new("").header("%PDF-1.4").catalog("/Version /2.0").open();
    assert_eq!(messages(&run_one(&doc, Check::Version { max: None })), ["The file is PDF 2.0"]);
    let doc = Pdf::new("").header("%PDF-1.4").catalog("/Version /1.3").open();
    assert_eq!(messages(&run_one(&doc, Check::Version { max: None })), ["The file is PDF 1.4"]);
}

fn metadata(trapped: &str) -> String {
    stream(
        "/Type /Metadata /Subtype /XML",
        &format!("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description pdf:Trapped=\"{trapped}\"/></rdf:RDF></x:xmpmeta>"),
    )
}

#[test]
fn trapped_is_missing_wrong_or_disagrees() {
    let r = run_one(&Pdf::new("").open(), Check::Trapped);
    assert_eq!(messages(&r), ["The document doesn't say whether it is trapped (Trapped)"]);
    assert_eq!(r.hits[0].fixup, Some(Fixup::SetTrapped { trapped: false }));
    let r = run_one(&Pdf::new("").trailer("/Info << /Trapped /Unknown >>").open(), Check::Trapped);
    assert_eq!(messages(&r), ["Trapped is /Unknown, but it must be /True or /False"]);
    // The document information and the metadata disagree: the document information wins.
    let doc = Pdf::new("").catalog("/Metadata 5 0 R").obj(&metadata("False")).obj("<< /Trapped /True >>").trailer("/Info 6 0 R").open();
    let r = run_one(&doc, Check::Trapped);
    assert_eq!(messages(&r), ["The Trapped key and the metadata's pdf:Trapped disagree"]);
    assert_eq!((r.hits[0].object, r.hits[0].fixup), (Some(6), Some(Fixup::SetTrapped { trapped: true })));
    // The key is right and the metadata's value isn't True or False: the metadata is wrong.
    let doc = Pdf::new("").catalog("/Metadata 5 0 R").obj(&metadata("Unknown")).obj("<< /Trapped /False >>").trailer("/Info 6 0 R").open();
    let r = run_one(&doc, Check::Trapped);
    assert_eq!(messages(&r), ["Trapped is Unknown, but it must be /True or /False"]);
    assert_eq!(r.hits[0].fixup, Some(Fixup::SetTrapped { trapped: false }));
    // Either one saying it is enough.
    assert_eq!(run_one(&Pdf::new("").trailer("/Info << /Trapped /False >>").open(), Check::Trapped).status, Status::Passed);
    assert_eq!(run_one(&Pdf::new("").catalog("/Metadata 5 0 R").obj(&metadata("True")).open(), Check::Trapped).status, Status::Passed);
}

#[test]
fn setting_trapped_writes_the_information_and_the_metadata() {
    // Information written into the trailer becomes an object of its own.
    let mut doc = Pdf::new("").trailer("/Info << /Title (Job) >>").open();
    assert_eq!(fix(&mut doc, Fixup::SetTrapped { trapped: false }).unwrap(), 1);
    let info = doc.resolve(doc.trailer().get(b"Info").unwrap());
    assert_eq!(info.as_dict().and_then(|d| d.name(b"Trapped")), Some(b"False".as_slice()));
    assert!(info.as_dict().unwrap().contains(b"Title"));
    assert!(doc.trailer().get(b"Info").unwrap().as_ref().is_some());
    assert_eq!(run_one(&doc, Check::Trapped).status, Status::Passed);
    assert_eq!(fix(&mut doc, Fixup::SetTrapped { trapped: false }).unwrap(), 0, "already says so");
    // The metadata, written as an attribute or as an element, says the same afterwards.
    for packet in
        [metadata("False"), stream("/Type /Metadata /Subtype /XML", "<rdf:Description><pdf:Trapped>Unknown</pdf:Trapped></rdf:Description>")]
    {
        let mut doc = Pdf::new("").catalog("/Metadata 5 0 R").obj(&packet).obj("<< /Trapped /False >>").trailer("/Info 6 0 R").open();
        assert_eq!(fix(&mut doc, Fixup::SetTrapped { trapped: true }).unwrap(), 2);
        let x = crate::metadata(&doc).unwrap();
        assert_eq!(crate::xmp::value(&x, "pdf:Trapped").as_deref(), Some("True"), "{x}");
        assert_eq!(doc.get(ObjRef::new(6, 0)).as_dict().and_then(|d| d.name(b"Trapped")), Some(b"True".as_slice()));
        assert_eq!(run_one(&doc, Check::Trapped).status, Status::Passed);
    }
}

#[test]
fn the_pdfx_profiles_wrap_the_verifier_and_offer_its_fixups() {
    let mut doc = Pdf::new("0 0 0 1 k 0 0 10 10 re f").open();
    let profile = find("pdfx-4").unwrap();
    let r = run(&doc, &profile, &NoHost);
    let c = &r.checks[0];
    assert_eq!((c.check.id(), c.status, c.severity), ("pdfx", Status::Found, Severity::Error));
    let boxes = c.hits.iter().find(|h| h.rule == Some("pdfx.page-boxes")).unwrap();
    assert_eq!((boxes.page, boxes.object, boxes.fixup), (Some(0), Some(3), Some(Fixup::SetTrimBox)));
    assert!(boxes.details.contains(&("Clause", "CGATS notes 2.10".to_string())));
    let trapped = c.hits.iter().find(|h| h.rule == Some("pdfx.trapped")).unwrap();
    assert_eq!(trapped.fixup, Some(Fixup::SetTrapped { trapped: false }));
    assert!(c.hits.iter().any(|h| h.rule == Some("pdfx.output-intent") && h.fixup.is_none()), "no output intents are written here");
    assert_eq!(c.fixups(), [Fixup::SetTrapped { trapped: false }, Fixup::SetTrimBox]);
    for f in r.fixups() {
        fix(&mut doc, f).unwrap();
    }
    let r = run(&doc, &profile, &NoHost);
    let rules: Vec<_> = r.checks[0].hits.iter().filter_map(|h| h.rule).collect();
    assert!(!rules.contains(&"pdfx.trapped") && !rules.contains(&"pdfx.page-boxes"), "{rules:?}");
    assert_eq!(r.checks[0].check.said(&crate::in_english), "PDF/X-4 compliance");
}

#[test]
fn the_pdfa_profiles_wrap_the_verifier() {
    let doc = Pdf::new("1 0 0 rg 0 0 10 10 re f").open();
    let r = run(&doc, &find("pdfa-3b").unwrap(), &NoHost);
    let c = &r.checks[0];
    let hit = c.hits.iter().find(|h| h.message == "The document has no XMP metadata").unwrap();
    assert_eq!((hit.rule, hit.details.as_slice()), (Some("6.6.2.1"), [("Clause", "ISO 19005-3 6.6.2.1".to_string())].as_slice()));
    assert!(c.fixups().is_empty(), "Save as PDF/A fixes these");
    assert!(!r.passed());
}

struct Checker;

impl Host for Checker {
    fn accessibility(&self, _doc: &Document) -> Option<Vec<Hit>> {
        Some(vec![Hit::worded("{what} on page {p}", vec![("{p}", "1".into()), ("{what}", "A figure has no alternate text".into())]).on(Some(0))])
    }
}

#[test]
fn accessibility_runs_through_the_host() {
    let doc = Pdf::new("").open();
    let profile = find("accessibility").unwrap();
    let r = run(&doc, &profile, &NoHost);
    assert_eq!(
        (r.checks[0].status, r.checks[0].note),
        (Status::Skipped, Some("This check needs the Accessibility Checker, which isn't available here"))
    );
    assert!(r.passed(), "a check that didn't run found nothing");
    let r = run(&doc, &profile, &Checker);
    assert_eq!(r.checks[0].status, Status::Found);
    // The nested wording is translated before the document's words go in.
    let upper = |w: &str| w.to_uppercase().replace("{P}", "{p}").replace("{WHAT}", "{what}");
    assert_eq!(r.checks[0].hits[0].said(&upper), "A FIGURE HAS NO ALTERNATE TEXT ON PAGE 1");
    assert_eq!(r.checks[0].hits[0].message, "A figure has no alternate text on page 1");
}

/// A print job with something for every commercial-print check to find.
fn print_job() -> Document {
    Pdf::new(
        "1 0 0 rg 0 0 10 10 re f BT /F1 12 Tf (a) Tj ET q 216 0 0 216 10 10 cm /Im Do Q 0.1 w 10 300 m 100 300 l S /GS gs /CS0 cs 1 scn 0 0 5 5 re f",
    )
    .catalog("/OpenAction 5 0 R")
    .page("/Annots [6 0 R]")
    .resources("/Font << /F1 7 0 R >> /XObject << /Im 8 0 R >> /ExtGState << /GS << /ca 0.5 >> >> /ColorSpace << /CS0 [/Separation /Gold /DeviceCMYK 9 0 R] >>")
    .obj("<< /Type /Action /S /JavaScript /JS (print()) >>")
    .obj("<< /Type /Annot /Subtype /Square /Rect [100 100 200 200] /F 4 >>")
    .obj("<< /Type /Font /Subtype /Type1 /BaseFont /F#7Bp#7D >>")
    .obj(&image("/Width 300 /Height 300 /ColorSpace /DeviceRGB /BitsPerComponent 8"))
    .obj("<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0.2 0.8 0.1] /N 1 >>")
    .open()
}

#[test]
fn commercial_print_runs_every_check_in_order() {
    let doc = print_job();
    let profile = find("commercial-print").unwrap();
    let r = run(&doc, &profile, &NoHost);
    let got: Vec<(&str, Status)> = r.checks.iter().map(|c| (c.check.id(), c.status)).collect();
    assert_eq!(
        got,
        [
            ("fonts-embedded", Status::Found),
            ("colour-spaces", Status::Found),
            ("encryption", Status::Passed),
            ("image-resolution", Status::Found),
            ("hairlines", Status::Found),
            ("transparency", Status::Found),
            ("trim-box", Status::Found),
            ("bleed", Status::Passed),
            ("scripts", Status::Found),
            ("annotations-in-trim", Status::Found),
            ("spot-colours", Status::Found),
            ("trapped", Status::Found),
            ("version", Status::Found),
        ]
    );
    // Errors: the font, the RGB rectangle and the RGB image.
    assert_eq!((r.count(Severity::Error), r.count(Severity::Warning), r.count(Severity::Info)), (3, 6, 3));
    assert!(!r.passed());
    assert_eq!(r.fixups(), [Fixup::SetTrimBox, Fixup::RemoveScripts, Fixup::SetTrapped { trapped: false }]);
    assert_eq!(r.pages, 1);
    assert!(r.unread_pages.is_empty());
    // Every fixup applied, the checks they address pass.
    let mut doc = doc;
    for f in r.fixups() {
        assert!(fix(&mut doc, f).unwrap() > 0, "{f:?}");
    }
    let r = run(&doc, &profile, &NoHost);
    for id in ["trim-box", "scripts", "trapped"] {
        assert_eq!(r.checks.iter().find(|c| c.check.id() == id).map(|c| c.status), Some(Status::Passed), "{id}");
    }
    assert!(r.fixups().is_empty());
}

#[test]
fn reports_say_the_results_in_the_interface_language() {
    let r = run(&print_job(), &find("commercial-print").unwrap(), &NoHost);
    let text = text(&r, "job.pdf", "2026-10-16", &crate::in_english);
    for want in [
        "Preflight report\nDocument: job.pdf\nProfile: Commercial print (CMYK)\nDate: 2026-10-16\nSummary: 3 errors, 6 warnings, 3 information\n",
        "\nErrors\n  Fonts not embedded (1)\n    - The font F{p} isn't embedded (page 1) · 7 0 obj\n      Font type: Type1\n",
        "  Image resolution (1) [Colour images: 300 ppi, Gray images: 300 ppi, Bitmap images: 1200 ppi]\n    - A colour image is drawn at 100 ppi (page 1) · 8 0 obj\n",
        "  Pages without a TrimBox (1)\n    Fixup: Set missing TrimBoxes from the CropBox\n",
        "\nChecks passed\n  Encryption\n  Bleed\n",
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    // Wordings are translated and the document's own words go in as they are: the font named
    // `F{p}` isn't read as a hole.
    let shout = |w: &str| format!("«{w}»");
    let said = super::text(&r, "job.pdf", "2026-10-16", &shout);
    assert!(said.starts_with("«Preflight report»\n«Document: job.pdf»\n"), "{said}");
    assert!(said.contains("- «The font F{p} isn't embedded»« (page 1)»"), "{said}");
    assert!(said.contains("«Fonts not embedded» (1)"), "{said}");
    // JSON: in English, pages from 1, the parameters as the profile holds them.
    let j = json(&r, "job.pdf");
    assert_eq!((j["passed"].as_bool(), j["summary"]["errors"].as_u64(), j["summary"]["warnings"].as_u64()), (Some(false), Some(3), Some(6)));
    let image = &j["checks"][3];
    assert_eq!(image["check"], "image-resolution");
    assert_eq!(image["params"]["colour_ppi"], 300.0);
    assert_eq!(image["hits"][0]["page"], 1);
    assert_eq!(image["hits"][0]["object"], 8);
    assert_eq!(image["hits"][0]["details"]["Pixels"], "300 × 300");
    assert_eq!(j["checks"][6]["fixups"][0]["fixup"], "set-trim-box");
    assert_eq!(j["checks"][11]["hits"][0]["fixup"], "set-trapped");
    assert_eq!(j["profile"]["id"], "commercial-print");
}

#[test]
fn wordings_are_complete() {
    let asked = RefCell::new(BTreeSet::new());
    let words = |w: &str| {
        asked.borrow_mut().insert(w.to_owned());
        w.to_owned()
    };
    let docs = [print_job(), scripted(), colourful(), Pdf::new("").trailer("/Info << /Trapped /Unknown >>").open()];
    let mut profiles = builtin();
    profiles.extend(single_checks());
    profiles.push(Profile {
        id: "all".into(),
        name: "Commercial print (CMYK)".into(),
        description: String::new(),
        checks: vec![
            CheckSpec { check: Check::SpotColours { max: Some(0) }, severity: Severity::Info },
            CheckSpec { check: Check::Version { max: Some("1.0".into()) }, severity: Severity::Info },
        ],
    });
    for doc in &docs {
        for p in &profiles {
            let r = run(doc, p, &Checker);
            let _ = text(&r, "x.pdf", "today", &words);
            for c in &r.checks {
                for h in &c.hits {
                    let _ = h.said(&words);
                }
            }
        }
    }
    for p in builtin() {
        words(&p.name);
        words(&p.description);
    }
    for s in Severity::ALL {
        words(s.label());
    }
    for f in FIXUPS {
        words(f.label());
    }
    let known: BTreeSet<&str> = WORDINGS.iter().chain(REPORT_WORDS).chain(crate::WORDINGS).chain(crate::pdfx::WORDINGS).copied().collect();
    let standards: BTreeSet<&str> = crate::pdfx::Level::ALL.iter().map(|l| l.label()).chain(["PDF/A-2b", "PDF/A-3b"]).collect();
    // The test host's own wordings are its own to list.
    let host = ["{what} on page {p}", "A figure has no alternate text"];
    let unlisted: Vec<String> = asked
        .into_inner()
        .into_iter()
        .filter(|w| !known.contains(w.as_str()) && !standards.contains(w.as_str()) && !host.contains(&w.as_str()))
        .collect();
    assert!(unlisted.is_empty(), "said but not in WORDINGS or REPORT_WORDS: {unlisted:#?}");
}

#[test]
fn the_walk_is_bounded() {
    // A form that draws itself is drawn once.
    let doc = Pdf::new("/Fm Do")
        .resources("/XObject << /Fm 5 0 R >>")
        .obj(&stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /XObject << /Fm 5 0 R >> >>", "0 w 0 0 m 1 1 l S /Fm Do"))
        .open();
    let r = run(&doc, &find("check:hairlines").unwrap(), &NoHost);
    assert_eq!(r.checks[0].hits.len(), 1);
    assert!(r.unread_pages.is_empty());
    // Forms nested deeper than the walk follows: the page is listed as not read to the end, and
    // what comes after the form on the page is still read.
    let mut pdf = Pdf::new("/F0 Do 1 0 0 rg 0 0 1 1 re f");
    for i in 0..20 {
        pdf =
            pdf.obj(&stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /XObject << /F0 {} 0 R >> >>", i + 6), "/F0 Do"));
    }
    let doc = pdf.resources("/XObject << /F0 5 0 R >>").obj(&stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1]", "")).open();
    let r = run(&doc, &find("commercial-print").unwrap(), &NoHost);
    assert_eq!(r.unread_pages, [0]);
    assert!(text(&r, "x", "y", &crate::in_english).contains("Content not read to the end on pages: 1"));
    assert_eq!(r.checks[1].hits.first().map(|h| h.message.as_str()), Some("Content uses DeviceRGB colour"));
    // Content that can't be decoded, or only in part, wasn't checked either.
    for filter in ["/Filter /FlateDecode", "/Filter /NoSuchFilter"] {
        let doc = Pdf::new("not flate data").content_dict(filter).open();
        assert_eq!(run(&doc, &find("check:hairlines").unwrap(), &NoHost).unread_pages, [0], "{filter}");
    }
    let doc = Pdf::new("/Fm Do")
        .resources("/XObject << /Fm 5 0 R >>")
        .obj(&stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Filter /FlateDecode", "not flate data"))
        .open();
    assert_eq!(run(&doc, &find("check:hairlines").unwrap(), &NoHost).unread_pages, [0], "a damaged form");
    assert!(run(&Pdf::new("0 0 m 1 1 l S").open(), &find("check:hairlines").unwrap(), &NoHost).unread_pages.is_empty());
    // A form drawn thousands of times: the page stops at the operator budget.
    let doc = Pdf::new(&"/Fm Do ".repeat(2_000))
        .resources("/XObject << /Fm 5 0 R >>")
        .obj(&stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", &"0 0 m 1 1 l n ".repeat(400)))
        .open();
    let r = run(&doc, &find("check:hairlines").unwrap(), &NoHost);
    assert_eq!(r.unread_pages, [0]);
    // Malformed content and parameters don't stop it.
    let doc = Pdf::new("q q Q Q Q Q cm w -1 w (x) w /Nope gs /Nope Do /Nope sh /Nope cs BI EI 1 0 0 rg S")
        .resources("/XObject << /Nope 3 0 R >> /ExtGState << /Nope 4 0 R >>")
        .open();
    let r = run(&doc, &find("commercial-print").unwrap(), &NoHost);
    assert_eq!(r.checks.len(), 13);
}

#[test]
fn holes_are_filled_once() {
    assert_eq!(fill("{a} and {b}", &[("{a}", "{b}".into()), ("{b}", "x".into())]), "{b} and x");
    assert_eq!(fill("no holes", &[]), "no holes");
    assert_eq!(fill("open { only", &[("{}", "x".into())]), "open { only");
    assert_eq!(fill("{} {}", &[("{}", "é".into())]), "é é");
    assert_eq!(Hit::new("The file is PDF {}", Some("{x}".into())).message, "The file is PDF {x}");
}
