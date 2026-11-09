//! Contributor-original synthetic PDFs for issue #2908. No fixture/font binaries are added.
use lopdf::{Document, Object, Stream, dictionary};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};
use std::sync::Arc;

const FONTS: [&str; 4] = ["ArialMT", "Arial-BoldMT", "Calibri", "TimesNewRomanPSMT"];
const CYRILLIC: &str = "Обща цена, лв. Здравей";

/// Build WinAnsi Differences mapping bytes 192..=255 to Cyrillic A through ya,
/// excluding Yo, using the corresponding afii glyph names.
fn cyrillic_encoding() -> Object {
    let mut differences = vec![Object::Integer(192)];
    for i in 0..64 {
        let number = (if i < 32 { 10017 } else { 10065 - 32 }) + i + i32::from(i % 32 >= 6);
        differences.push(Object::Name(format!("afii{number}").into_bytes()));
    }
    dictionary! {"Type"=>"Encoding", "BaseEncoding"=>"WinAnsiEncoding", "Differences"=>differences}.into()
}

/// Build a non-embedded TrueType dictionary with 600-unit widths for bytes 32..=255.
/// Callers replace widths, subtype or descriptor programs to construct controls.
fn simple_font(name: &str, encoding: Object) -> Object {
    dictionary! {
        "Type"=>"Font", "Subtype"=>"TrueType", "BaseFont"=>Object::Name(name.as_bytes().to_vec()),
        "FirstChar"=>32, "LastChar"=>255, "Widths"=>vec![Object::Integer(600);224],
        "Encoding"=>encoding,
        "FontDescriptor"=>dictionary! {
            "Type"=>"FontDescriptor", "FontName"=>Object::Name(name.as_bytes().to_vec()),
            "Flags"=>32, "FontBBox"=>vec![(-600).into(),(-300).into(),2000.into(),1000.into()],
            "ItalicAngle"=>0,"Ascent"=>900,"Descent"=>-200,"CapHeight"=>700,"StemV"=>80,
        }
    }
    .into()
}

/// Serialize a 595 by 842 single-page fixture with dictionary fonts named F0, F1, etc.
/// Lift descriptor programs and ToUnicode streams into indirect objects; fixture
/// construction and serialization must succeed.
fn pdf(fonts: Vec<Object>, content: Vec<u8>) -> Vec<u8> {
    let mut doc = Document::with_version("1.4");
    let pages = doc.new_object_id();
    let mut resources = lopdf::Dictionary::new();
    for (i, mut font) in fonts.into_iter().enumerate() {
        let dict = font.as_dict_mut().unwrap();
        if let Ok(Object::Dictionary(mut descriptor)) = dict.get(b"FontDescriptor").cloned() {
            for key in ["FontFile", "FontFile2", "FontFile3"] {
                if let Ok(Object::Stream(stream)) = descriptor.get(key.as_bytes()).cloned() {
                    descriptor.set(key, doc.add_object(stream));
                }
            }
            dict.set("FontDescriptor", doc.add_object(descriptor));
        }
        if let Ok(Object::Stream(stream)) = dict.get(b"ToUnicode").cloned() {
            dict.set("ToUnicode", doc.add_object(stream));
        }
        let id = doc.add_object(font);
        resources.set(format!("F{i}"), id);
    }
    let stream = doc.add_object(Stream::new(dictionary! {}, content));
    let page = doc.add_object(dictionary! {
        "Type"=>"Page","Parent"=>pages,"MediaBox"=>vec![0.into(),0.into(),595.into(),842.into()],
        "Resources"=>dictionary!{"Font"=>resources},"Contents"=>stream,
    });
    doc.objects.insert(pages, dictionary! {"Type"=>"Pages","Kids"=>vec![page.into()],"Count"=>1}.into());
    let catalog = doc.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

/// Reproduce the four reported families with Latin prefixes and Bulgarian text on
/// separate baselines, using explicit 600-unit advances for predictable ink cells.
fn reporter_pdf() -> Vec<u8> {
    let mut content = Vec::new();
    for (i, name) in FONTS.iter().enumerate() {
        content.extend_from_slice(format!("BT /F{i} 16 Tf 40 {} Td (", 780 - 40 * i).as_bytes());
        content.extend_from_slice(format!("{name}: Price Total / ").as_bytes());
        for c in CYRILLIC.chars() {
            let code = if ('А'..='я').contains(&c) { 192 + c as u32 - 'А' as u32 } else { c as u32 };
            content.push(u8::try_from(code).unwrap());
        }
        content.extend_from_slice(b") Tj ET\n");
    }
    pdf(FONTS.iter().map(|name| simple_font(name, cyrillic_encoding())).collect(), content)
}

/// Render page zero at 96 dpi with production font selection, asserting no render error.
fn render(bytes: Vec<u8>) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(RenderRequest { scale: 96.0 / 72.0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    page
}

/// Require ink in every alphabetic Cyrillic cell for each family; rendering only
/// the Latin prefix cannot satisfy this regression check.
#[test]
fn all_four_reported_nonembedded_fonts_draw_every_cyrillic_letter() {
    let page = render(reporter_pdf());
    for (line, font) in FONTS.iter().enumerate() {
        let prefix = format!("{font}: Price Total / ");
        let baseline = (842.0 - (780.0 - 40.0 * line as f32)) * 96.0 / 72.0;
        for (index, c) in CYRILLIC.chars().enumerate().filter(|(_, c)| c.is_alphabetic()) {
            let x = (40.0 + (prefix.len() + index) as f32 * 9.6) * 96.0 / 72.0;
            let mut ink = 0;
            for y in (baseline - 22.0) as u32..baseline as u32 + 2 {
                for x in x as u32..(x + 12.8).ceil() as u32 {
                    if page.rgba[((y * page.width + x) * 4) as usize] < 128 {
                        ink += 1;
                    }
                }
            }
            assert!(ink > 5, "{font}: missing Cyrillic {c} at character {index} (ink={ink})");
        }
    }
}

/// Verify all four Bulgarian strings extract independently of missing primary outlines.
#[test]
fn reporter_text_decodes_independently_of_missing_outlines() {
    let mut renderer = PageRenderer::new(Arc::new(reporter_pdf()), RenderConfig::default());
    let page = renderer.render(RenderRequest { kind: RequestKind::Text, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    let text = page.text.unwrap().plain_text();
    assert_eq!(text.matches(CYRILLIC).count(), 4, "{text}");
}

/// Check that an overflowing Differences cursor neither panics nor loses a later
/// valid reset: Cyrillic decodes, draws and resolves its alternate only once.
#[test]
fn overflowing_differences_indices_do_not_crash_or_discard_later_entries() {
    let encoding = dictionary! {"BaseEncoding"=>"WinAnsiEncoding", "Differences"=>vec![
        Object::Integer(i64::MAX), Object::Name(b"A".to_vec()),
        Object::Integer(192),Object::Name(b"afii10017".to_vec()),
    ]};
    let bytes = pdf(vec![simple_font("ArialMT", encoding.into())], b"BT /F0 16 Tf 40 780 Td <C0> Tj ET".to_vec());
    let _ = render(bytes.clone());
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let glyphs = probe(bytes, s);
    assert_eq!(glyphs[0].text, "А");
    assert!(!glyphs[0].path.as_ref().unwrap().elements().is_empty());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    font::{FontData, FontQuery, Glyph},
    interpret_page,
};
use kurbo::{Affine, BezPath, Rect, Shape};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct DrawnGlyph {
    text: String,
    path: Option<BezPath>,
    glyph: u32,
    advance: Option<f32>,
    matrix: [f64; 6],
    embedded: bool,
}

#[derive(Default)]
struct Probe(Vec<DrawnGlyph>);
/// Observe glyph geometry only; painting, masks, clips, images and groups are ignored
/// because this probe compares interpreter placement rather than composited pixels.
impl<'a> Device<'a> for Probe {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    /// Ignores transparency groups in this geometry-only probe.
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    /// Record semantic text, outline/id, advance, embedding status and the combined page
    /// and glyph transform at draw time, preserving interpreter order for comparisons.
    fn draw_glyph(&mut self, g: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        let text = match g.as_unicode() {
            Some(hayro::hayro_interpret::hayro_cmap::BfString::Char(c)) => c.to_string(),
            Some(hayro::hayro_interpret::hayro_cmap::BfString::String(s)) => s,
            None => String::new(),
        };
        let (path, glyph, advance, embedded) = match g {
            Glyph::Outline(o) => (Some(o.outline()), o.glyph_id().to_u32(), o.advance_width(), o.font_data().is_some()),
            Glyph::Type3(t) => (None, 0, t.advance_width(), true),
        };
        self.0.push(DrawnGlyph { text, path, glyph, advance, matrix: (transform * glyph_transform).as_coeffs(), embedded });
    }
    /// Ignores images in this geometry-only probe.
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    /// Ignores clip closure in this geometry-only probe.
    fn pop_clip_path(&mut self) {}
    /// Ignores group closure in this geometry-only probe.
    fn pop_transparency_group(&mut self) {}
}

/// Interpret page zero of a valid fixture into glyph observations with the supplied
/// resolver settings. Fixture parsing must succeed.
fn probe(bytes: Vec<u8>, settings: InterpreterSettings) -> Vec<DrawnGlyph> {
    let parsed = hayro::hayro_syntax::Pdf::new(bytes).unwrap();
    let page = &parsed.pages()[0];
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, 595.0, 842.0), &cache, page.xref(), settings);
    let mut device = Probe::default();
    interpret_page(page, &mut ctx, &mut device);
    device.0
}

/// Return a test resolver and MissingGlyph request counter. Alternate bytes share one
/// stable Arc to exercise face caching; absent, corrupt or non-covering bytes are
/// intentional inputs. Other queries use the default resolver, without system fonts.
fn settings(fallback: Option<&'static [u8]>) -> (InterpreterSettings, Arc<AtomicUsize>) {
    let standard = InterpreterSettings::default().font_resolver;
    let data = fallback.map(|bytes| Arc::new(bytes) as FontData);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    (
        InterpreterSettings {
            font_resolver: Arc::new(move |query| match query {
                FontQuery::MissingGlyph { .. } => {
                    count.fetch_add(1, Ordering::Relaxed);
                    data.clone().map(|d| (d, 0))
                }
                _ => standard(query),
            }),
            ..Default::default()
        },
        calls,
    )
}

/// Build explicit 600-unit widths for all bytes and supplied Differences pairs.
/// Keep arbitrary signed indices to exercise malformed encodings.
fn encoded_font(name: &str, names: &[(i64, &str)]) -> Object {
    let differences = names.iter().flat_map(|(code, name)| [Object::Integer(*code), Object::Name(name.as_bytes().to_vec())]).collect::<Vec<_>>();
    let mut f = simple_font(name, dictionary! {"BaseEncoding"=>"WinAnsiEncoding","Differences"=>differences}.into());
    f.as_dict_mut().unwrap().set("FirstChar", 0);
    f.as_dict_mut().unwrap().set("LastChar", 255);
    f.as_dict_mut().unwrap().set("Widths", vec![Object::Integer(600); 256]);
    f
}

/// Show the supplied bytes as one hexadecimal text string at 20 pt and (40, 700),
/// avoiding literal-string escaping in byte-code fixtures.
fn glyph_pdf(font: Object, codes: &[u8]) -> Vec<u8> {
    let hex = codes.iter().map(|b| format!("{b:02X}")).collect::<String>();
    pdf(vec![font], format!("BT /F0 20 Tf 40 700 Td <{hex}> Tj ET").into_bytes())
}

/// Compare absent Widths against explicit natural alternate advances, including
/// spacing, TJ adjustments, transforms, raster output and repeated-code cache reuse.
#[test]
fn standard_fonts_without_widths_use_alternate_advances_and_keep_text_placement() {
    use skrifa::{
        FontRef, MetadataProvider,
        instance::{LocationRef, Size},
    };
    let face = FontRef::new(pdfcraft_fonts::INTER_REGULAR).unwrap();
    let metrics = face.glyph_metrics(Size::new(1000.0), LocationRef::default());
    for names in [
        [(192, "afii10033"), (193, "afii10066"), (194, "afii10074"), (195, "afii10067"), (196, "afii10070"), (197, "afii10084")],
        [(192, "Alpha"), (193, "beta"), (194, "gamma"), (195, "delta"), (196, "epsilon"), (197, "theta")],
    ] {
        let mut font = encoded_font("Helvetica", &names);
        let dict = font.as_dict_mut().unwrap();
        dict.set("Subtype", "Type1");
        dict.remove(b"Widths");
        dict.remove(b"FirstChar");
        dict.remove(b"LastChar");
        let content =
            b"q 0.8 0.2 -0.1 1.1 13 17 cm BT /F0 20 Tf 75 Tz 2 Tc 4 Tw 3 Ts 1 0.1 0.2 1 40 700 Tm [<41C0C1C2> 120 <C3C4C5C02042>] TJ ET Q".to_vec();
        let bytes = pdf(vec![font.clone()], content.clone());
        let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
        let actual = probe(bytes.clone(), s);
        assert_eq!(actual.iter().map(|g| g.text.as_str()).collect::<String>(), if names[0].1 == "Alpha" { "AΑβγδεθΑ B" } else { "AПбиветП B" });
        assert_eq!(calls.load(Ordering::Relaxed), 6, "repeated codes reuse the fallback cache");
        let mut widths = vec![Object::Integer(600); 256];
        for (index, g) in actual.iter().skip(1).take(6).enumerate() {
            let c = g.text.chars().next().unwrap();
            let expected = metrics.advance_width(face.charmap().map(c).unwrap()).unwrap();
            assert_eq!(g.advance, Some(expected), "missing width for {c} must use the alternate face");
            assert!(expected > 0.0);
            widths[192 + index] = Object::Real(expected);
        }
        // The primary face still supplies A, space and B's standard Helvetica metrics.
        for (index, width) in [(65, 667), (32, 278), (66, 667)] {
            widths[index] = Object::Integer(width);
        }
        let dict = font.as_dict_mut().unwrap();
        dict.set("FirstChar", 0);
        dict.set("LastChar", 255);
        dict.set("Widths", widths);
        let explicit = pdf(vec![font], content);
        let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
        let expected = probe(explicit.clone(), s);
        for (a, e) in actual.iter().zip(&expected) {
            assert_eq!(a.advance, e.advance);
            assert_eq!(a.matrix, e.matrix, "spacing, TJ, transforms and baseline must match explicit advances");
            assert_eq!(a.path, e.path);
        }
        assert_eq!(render(bytes).rgba, render(explicit).rgba);
    }
}

/// Oversized PDF advances must not widen narrow Greek fallback outlines into blocks.
/// Narrow and zero advances still fit the outline; PDF advances, following-glyph
/// positions, text and the baseline remain independent of that visual width limit.
#[test]
fn greek_fallback_widths_never_widen_outlines_or_change_pdf_placement() {
    let names = [(192, "Iota"), (193, "iota"), (194, "iotatonos")];
    for (name, face) in [("Arial", pdfcraft_fonts::INTER_REGULAR), ("Arial,Bold", pdfcraft_fonts::INTER_SEMIBOLD)] {
        for subtype in ["TrueType", "Type1"] {
            for (code, glyph_name) in names {
                let mut font = encoded_font(name, &[(code, glyph_name)]);
                let dict = font.as_dict_mut().unwrap();
                dict.set("Subtype", subtype);
                dict.remove(b"Widths");
                let (s, _) = settings(Some(face));
                let natural = probe(glyph_pdf(font.clone(), &[code as u8, code as u8]), s);
                assert_eq!(natural.len(), 2);
                let original = &natural[0];
                let original_path = original.path.as_ref().unwrap();
                let original_bounds = original_path.bounding_box();
                let advance = original.advance.unwrap();
                assert!(advance > 100.0 && advance < 600.0, "{name} {glyph_name}: {advance}");
                assert!(original_bounds.width() > 0.0);
                for width in [0, 100, 600, 1200] {
                    font.as_dict_mut().unwrap().set("Widths", vec![Object::Integer(width); 256]);
                    let (s, calls) = settings(Some(face));
                    let glyphs = probe(glyph_pdf(font.clone(), &[code as u8, code as u8]), s);
                    assert_eq!(glyphs.len(), 2);
                    assert_eq!(calls.load(Ordering::Relaxed), 1);
                    for glyph in &glyphs {
                        assert_eq!(glyph.text, original.text);
                        assert!(!glyph.embedded);
                        assert_eq!(glyph.advance, Some(width as f32));
                        let path = glyph.path.as_ref().unwrap();
                        if width >= 600 {
                            assert_eq!(path, original_path, "{name} {subtype} {glyph_name}: oversized width {width}");
                        } else {
                            let bounds = path.bounding_box();
                            let ratio = f64::from(width as f32 / advance);
                            assert!((bounds.width() - original_bounds.width() * ratio).abs() < 0.001);
                            assert!((bounds.y0 - original_bounds.y0).abs() < 0.001);
                            assert!((bounds.y1 - original_bounds.y1).abs() < 0.001);
                        }
                    }
                    assert_eq!(glyphs[0].matrix, original.matrix);
                    let mut expected = original.matrix;
                    expected[4] += width as f64 * 0.02;
                    assert_eq!(glyphs[1].matrix, expected, "PDF advance must still place the next glyph");
                }
            }
        }
    }
}

/// Keep explicit widths, including zero, ahead of alternate metrics; array holes
/// must use MissingWidth, including an explicit zero.
#[test]
fn explicit_zero_and_missing_width_keep_priority_over_alternate_metrics() {
    for (entry, missing_width, expected) in
        [(Object::Integer(0), 400, 0.0), (Object::Integer(600), 400, 600.0), (Object::Null, 400, 400.0), (Object::Null, 0, 0.0)]
    {
        let mut font = encoded_font("Helvetica", &[(192, "afii10033")]);
        let dict = font.as_dict_mut().unwrap();
        dict.set("Subtype", "Type1");
        dict.set("FirstChar", 192);
        dict.set("LastChar", 192);
        dict.set("Widths", vec![entry]);
        dict.get_mut(b"FontDescriptor").unwrap().as_dict_mut().unwrap().set("MissingWidth", missing_width);
        let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
        let glyphs = probe(glyph_pdf(font, &[192, 192]), s);
        assert_eq!(glyphs.len(), 2);
        for g in &glyphs {
            assert_eq!(g.advance, Some(expected));
        }
        assert!((glyphs[1].matrix[4] - glyphs[0].matrix[4] - f64::from(expected) * 0.02).abs() < 0.0001);
    }
}

/// Check MissingWidth precedence over alternate advances without a Widths array,
/// including zero and the resulting following-glyph positions.
#[test]
fn explicit_missing_width_without_widths_keeps_priority_over_alternate_metrics() {
    for width in [0, 400] {
        let mut font = encoded_font("Helvetica", &[(192, "afii10033")]);
        let dict = font.as_dict_mut().unwrap();
        dict.remove(b"Widths");
        dict.get_mut(b"FontDescriptor").unwrap().as_dict_mut().unwrap().set("MissingWidth", width);
        let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
        let glyphs = probe(glyph_pdf(font, &[192, 192]), s);
        assert_eq!(glyphs.len(), 2);
        assert!(glyphs.iter().all(|g| g.advance == Some(width as f32)));
        assert!((glyphs[1].matrix[4] - glyphs[0].matrix[4] - f64::from(width) * 0.02).abs() < 0.0001);
    }
}

/// Require visible 600-unit missing boxes when neither PDF widths nor a covering
/// alternate exists, preventing collapsed text placement.
#[test]
fn unavailable_alternate_without_widths_spaces_missing_boxes() {
    let mut font = encoded_font("Helvetica", &[(192, "afii10033")]);
    font.as_dict_mut().unwrap().remove(b"Widths");
    let (s, _) = settings(None);
    let glyphs = probe(glyph_pdf(font, &[192, 192]), s);
    assert_eq!(glyphs.len(), 2);
    assert!(glyphs.iter().all(|g| g.advance == Some(600.0) && !g.path.as_ref().unwrap().elements().is_empty()));
    assert!((glyphs[1].matrix[4] - glyphs[0].matrix[4] - 12.0).abs() < 0.0001);
}

/// Preserve a combining alternate's natural zero advance instead of substituting 600;
/// repeated codes reuse resolution and retain the same position.
#[test]
fn zero_advance_combining_fallback_without_widths_stays_zero() {
    let mut font = encoded_font("Helvetica", &[(192, "uni0301")]);
    font.as_dict_mut().unwrap().remove(b"Widths");
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let glyphs = probe(glyph_pdf(font, &[192, 192]), s);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(glyphs.len(), 2);
    assert!(glyphs.iter().all(|g| g.advance == Some(0.0) && !g.path.as_ref().unwrap().elements().is_empty()));
    assert_eq!(glyphs[0].matrix, glyphs[1].matrix);
}

/// Keep Latin and primary accent-composite paths while sourcing unsupported characters
/// from another face, without exposing substitutes as embedded font programs.
#[test]
fn mixed_runs_keep_primary_composites_and_use_a_second_outline_source() {
    let bytes =
        glyph_pdf(encoded_font("ArialMT", &[(193, "afii10017"), (194, "uni04D9"), (195, "ccaron"), (196, "lcaron")]), &[65, 193, 194, 195, 196, 66]);
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let glyphs = probe(bytes.clone(), s);
    assert_eq!(glyphs.iter().map(|g| g.text.as_str()).collect::<String>(), "AАәčľB");
    assert!(glyphs.iter().all(|g| !g.path.as_ref().unwrap().elements().is_empty()));
    assert!(glyphs.iter().all(|g| !g.embedded), "display fallback must never become embedded font data");
    assert_eq!(calls.load(Ordering::Relaxed), 3, "A/B and the existing č composite do not request another face");
    let (s, _) = settings(None);
    let primary = probe(bytes, s);
    for i in [0, 3, 5] {
        assert_eq!(glyphs[i].path, primary[i].path, "primary/composite outline {i} changed");
    }
    assert_ne!(glyphs[1].path, primary[1].path, "a real Cyrillic outline replaces the missing box");
}

/// Replacing missing boxes with alternate outlines must preserve PDF advances,
/// TJ/character/word spacing, transforms and baselines.
#[test]
fn pdf_advances_spacing_transforms_and_baselines_are_independent_of_fallback() {
    let font = encoded_font("ArialMT", &[(192, "afii10017")]);
    let bytes = pdf(
        vec![font],
        b"q 0.8 0.2 -0.1 1.1 13 17 cm BT /F0 20 Tf 75 Tz 2 Tc 4 Tw 3 Ts 1 0.1 0.2 1 40 700 Tm [<41> 120 <C020> -80 <42>] TJ ET Q".to_vec(),
    );
    let (s, _) = settings(None);
    let before = probe(bytes.clone(), s);
    let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let after = probe(bytes, s);
    assert_eq!(before.len(), 4);
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.text, b.text);
        assert_eq!(a.advance, b.advance);
        assert_eq!(a.matrix, b.matrix);
    }
    assert_ne!(before[1].path, after[1].path);
    assert_eq!(before[0].path, after[0].path);
    assert_eq!(before[3].path, after[3].path);
}

/// Keep distinct narrow PDF widths and fitted paths for two codes naming one character,
/// with one alternate resolution per repeated code.
#[test]
fn repeated_aliases_keep_distinct_widths_and_cache_one_resolution_per_code() {
    let mut font = encoded_font("TimesNewRomanPSMT", &[(65, "afii10017"), (66, "uni0410"), (67, "A")]);
    let dict = font.as_dict_mut().unwrap();
    dict.set("FirstChar", 65);
    dict.set("LastChar", 67);
    dict.set("Widths", vec![150.into(), 300.into(), 600.into()]);
    let bytes = glyph_pdf(font, &[65, 66, 67, 65, 66]);
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let g = probe(bytes, s);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(g.iter().map(|g| g.advance).collect::<Vec<_>>(), vec![Some(150.0), Some(300.0), Some(600.0), Some(150.0), Some(300.0)]);
    assert_ne!(g[0].glyph, g[1].glyph, "different code/width must not share an outline cache entry");
    assert_eq!(g[0].path, g[3].path);
    assert_eq!(g[1].path, g[4].path);
    let w = |i: usize| g[i].path.as_ref().unwrap().bounding_box().width();
    assert!((w(1) - 2.0 * w(0)).abs() < 0.01, "{} {}", w(0), w(1));
}

/// Absent, corrupt and non-covering alternates yield the same box; covering Inter
/// replaces Cyrillic while an unsupported emoji still receives a box.
#[test]
fn unavailable_corrupt_and_noncovering_faces_draw_the_same_explicit_box() {
    let font = encoded_font("Calibri", &[(65, "afii10017"), (66, "u1FAE0")]);
    let bytes = glyph_pdf(font, &[65, 66]);
    let (s, _) = settings(None);
    let missing = probe(bytes.clone(), s);
    for data in [b"corrupt font".as_slice(), include_bytes!("../../../assets/fonts/DancingScript.ttf").as_slice()] {
        let (s, _) = settings(Some(data));
        let g = probe(bytes.clone(), s);
        for (a, b) in missing.iter().zip(g) {
            assert_eq!(a.path, b.path);
            assert!(!b.path.unwrap().elements().is_empty());
        }
    }
    let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let g = probe(bytes, s);
    assert_ne!(missing[0].path, g[0].path);
    assert_eq!(missing[1].path, g[1].path, "unsupported emoji stays explicit");
}

/// Keep spaces, invisible/default-ignorable characters and invalid names free of
/// invented ink or guessed Unicode, without unnecessary alternate-face queries.
#[test]
fn invisible_and_invalid_names_do_not_gain_visible_boxes_or_guess_unicode() {
    let names = ["uni0000", "uni200D", "uni2060", "uniFE0F", "uni034F", "space", "uni00AD", "not_a_glyph", "u110000", "uniD800"];
    let codes = (1..=names.len() as u8).collect::<Vec<_>>();
    let mapping = names.iter().enumerate().map(|(i, n)| ((i + 1) as i64, *n)).collect::<Vec<_>>();
    let bytes = glyph_pdf(encoded_font("ArialMT", &mapping), &codes);
    let (s, calls) = settings(None);
    let glyphs = probe(bytes, s);
    assert!(glyphs.iter().all(|g| g.path.as_ref().unwrap().elements().is_empty()), "{glyphs:?}");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

/// Wrap CMap operators in an eight-bit codespace stream for later insertion as an
/// indirect ToUnicode object.
fn to_unicode(mappings: &str) -> Object {
    Object::Stream(Stream::new(dictionary!{},format!("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Synthetic def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange {mappings} endcmap end end").into_bytes()))
}

/// Allow single-scalar ToUnicode recovery for unknown names. Preserve sequence
/// semantics without selecting the first scalar, and keep supported primary outlines.
#[test]
fn single_scalar_to_unicode_can_rescue_unknown_names_but_never_overrides_primary_glyphs() {
    let mut font = encoded_font("Calibri", &[(1, "unknownOne"), (2, "unknownTwo")]);
    font.as_dict_mut().unwrap().set("ToUnicode", to_unicode("3 beginbfchar <01> <041E> <02> <00660069> <41> <0411> endbfchar"));
    let bytes = glyph_pdf(font, &[1, 2, 65]);
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let g = probe(bytes, s);
    assert_eq!(g[0].text, "О");
    assert!(!g[0].path.as_ref().unwrap().elements().is_empty());
    assert_eq!(g[1].text, "fi");
    assert!(!g[1].path.as_ref().unwrap().elements().is_empty(), "a semantic sequence gets an explicit box, never a guessed first character");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let (s, _) = settings(None);
    let latin = probe(glyph_pdf(encoded_font("Calibri", &[]), b"A"), s);
    assert_eq!(g[2].path, latin[0].path, "ToUnicode text semantics do not change a supported visual A");
}

/// Primary Latin, Symbol and Dingbat outlines must remain unchanged across alternate
/// resolvers and issue no MissingGlyph requests.
#[test]
fn latin_only_symbol_and_dingbat_outlines_ignore_the_fallback_resolver() {
    for (name, codes) in [
        ("ArialMT", b"Price Total / 123, abc.".as_slice()),
        ("Arial-BoldMT", b"ABcd"),
        ("TimesNewRomanPSMT", b"AVfi"),
        ("Symbol", b"ABCD"),
        ("ZapfDingbats", b"!\"#$"),
    ] {
        let bytes = glyph_pdf(simple_font(name, Object::Name(b"WinAnsiEncoding".to_vec())), codes);
        let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
        let a = probe(bytes.clone(), s);
        let (s, _) = settings(Some(pdfcraft_fonts::INTER_SEMIBOLD));
        let b = probe(bytes, s);
        assert_eq!(calls.load(Ordering::Relaxed), 0, "{name} unexpectedly asks for a fallback");
        assert!(a.iter().any(|g| !g.path.as_ref().unwrap().elements().is_empty()));
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.matrix, b.matrix);
        }
    }
}

/// Keep embedded programs authoritative, including missing glyphs: alternate faces
/// must not affect ids, outlines or advances or trigger MissingGlyph queries.
#[test]
fn embedded_fonts_keep_their_programs_and_missing_glyphs_without_fallback_override() {
    for bytes in [pdfcraft_fonts::INTER_REGULAR, include_bytes!("../../../assets/fonts/DancingScript.ttf").as_slice()] {
        let mut font = encoded_font("ArialMT", &[(192, "afii10017")]);
        font.as_dict_mut()
            .unwrap()
            .get_mut(b"FontDescriptor")
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("FontFile2", Object::Stream(Stream::new(dictionary! {}, bytes.to_vec())));
        let bytes = glyph_pdf(font, &[65, 192]);
        let (s, calls) = settings(Some(pdfcraft_fonts::INTER_SEMIBOLD));
        let a = probe(bytes.clone(), s);
        let (s, _) = settings(None);
        let b = probe(bytes, s);
        assert_eq!(calls.load(Ordering::Relaxed), 0, "a valid embedded program must have absolute precedence");
        assert!(a.iter().all(|g| g.embedded));
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.glyph, b.glyph);
            assert_eq!(a.advance, b.advance);
        }
    }
}

/// Check the alternate's 0.2126 horizontal shear while preserving vertical bounds,
/// baseline and advance relative to the upright control.
#[test]
fn italic_fallback_slants_the_outline_without_moving_the_baseline_or_advance() {
    let bytes = |name| glyph_pdf(encoded_font(name, &[(65, "afii10017")]), b"A");
    let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let normal = probe(bytes("ArialMT"), s);
    let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let italic = probe(bytes("Arial-ItalicMT"), s);
    let a = normal[0].path.as_ref().unwrap();
    let b = italic[0].path.as_ref().unwrap();
    assert_eq!(&(Affine::new([1.0, 0.0, 0.2126, 1.0, 0.0, 0.0]) * a.clone()), b);
    assert_eq!(normal[0].advance, italic[0].advance);
    assert_eq!(normal[0].matrix, italic[0].matrix);
    assert_eq!(a.bounding_box().y0, b.bounding_box().y0);
    assert_eq!(a.bounding_box().y1, b.bounding_box().y1);
}

/// Exercise non-embedded Type1: Cyrillic decodes and receives alternate outlines with
/// PDF advances while supported Latin retains its primary path.
#[test]
fn type1_nonembedded_fonts_use_the_same_per_glyph_fallback() {
    let mut font = encoded_font("Helvetica", &[(192, "afii10017")]);
    font.as_dict_mut().unwrap().set("Subtype", "Type1");
    let bytes = glyph_pdf(font, &[65, 192]);
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let g = probe(bytes.clone(), s);
    let (s, _) = settings(None);
    let boxes = probe(bytes, s);
    assert_eq!(g[0].path, boxes[0].path);
    assert_ne!(g[1].path, boxes[1].path);
    assert_eq!(g[1].text, "А");
    assert_eq!(g[1].advance, boxes[1].advance);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

/// Select distinct alternates for Cyrillic and lcaron in one run, retaining primary
/// Latin outlines and reusing resolved faces.
#[test]
fn one_run_can_use_distinct_fallback_faces_without_replacing_primary_glyphs() {
    let bytes = glyph_pdf(encoded_font("ArialMT", &[(192, "afii10017"), (193, "lcaron")]), &[65, 192, 193, 192]);
    let (mut s, _) = settings(None);
    let original = s.font_resolver.clone();
    let inter: FontData = Arc::new(pdfcraft_fonts::INTER_REGULAR);
    let script: FontData = Arc::new(include_bytes!("../../../assets/fonts/DancingScript.ttf").as_slice());
    s.font_resolver = Arc::new(move |q| match q {
        FontQuery::MissingGlyph { character: 'А', .. } => Some((inter.clone(), 0)),
        FontQuery::MissingGlyph { character: 'ľ', .. } => Some((script.clone(), 0)),
        _ => original(q),
    });
    let selected = probe(bytes.clone(), s);
    let (s, _) = settings(Some(include_bytes!("../../../assets/fonts/DancingScript.ttf")));
    let script = probe(bytes.clone(), s);
    let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let inter = probe(bytes, s);
    assert_eq!(selected[0].path, inter[0].path);
    assert_eq!(selected[1].path, inter[1].path);
    assert_eq!(selected[2].path, script[2].path);
    assert_ne!(selected[2].path, inter[2].path);
    assert_eq!(selected[1].path, selected[3].path);
}

/// Exercise malformed encoding types, unknown bases and out-of-range Differences
/// without panics or invented Cyrillic; supported Latin A remains drawable.
#[test]
fn malformed_encoding_entries_do_not_panic_or_invent_cyrillic() {
    for encoding in [
        Object::Integer(42),
        Object::Name(b"NotAnEncoding".to_vec()),
        dictionary! {"BaseEncoding"=>42,"Differences"=>"wrong type"}.into(),
        dictionary! {"Differences"=>vec![Object::Integer(-99), Object::Name(b"afii10017".to_vec()), Object::Null, Object::Integer(256), Object::Name(b"afii10017".to_vec())]}.into(),
    ] {
        let bytes = glyph_pdf(simple_font("ArialMT", encoding), b"A");
        let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
        let glyphs = probe(bytes, s);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(glyphs[0].text, "A");
    }
}

/// Compare a full stop following alternate Cyrillic against its primary-only control.
#[test]
fn terminal_full_stop_keeps_its_primary_outline_next_to_fallback() {
    let bytes = glyph_pdf(encoded_font("ArialMT", &[(192, "afii10017")]), &[192, b'.']);
    let (s, _) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let glyphs = probe(bytes, s);
    let (s, _) = settings(None);
    let dot = probe(glyph_pdf(encoded_font("ArialMT", &[]), b"."), s);
    assert!(!glyphs[0].path.as_ref().unwrap().elements().is_empty());
    assert_eq!(glyphs[1].path, dot[0].path);
    assert_eq!(glyphs[1].text, ".");
}

/// Keep a multi-scalar name as a box without resolving its first scalar, even with
/// conflicting ToUnicode; extraction retains ToUnicode semantics.
#[test]
fn multi_scalar_glyph_names_get_a_box_without_guessing_a_first_scalar() {
    let mut font = encoded_font("ArialMT", &[(192, "dalethatafpatah")]);
    font.as_dict_mut().unwrap().set("ToUnicode", to_unicode("1 beginbfchar <C0> <0410> endbfchar"));
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let glyphs = probe(glyph_pdf(font, &[192]), s);
    let (s, _) = settings(None);
    let boxed = probe(glyph_pdf(encoded_font("ArialMT", &[(192, "afii10017")]), &[192]), s);
    assert_eq!(calls.load(Ordering::Relaxed), 0, "neither first scalar nor conflicting ToUnicode chooses the outline");
    assert_eq!(glyphs[0].path, boxed[0].path);
    assert_eq!(glyphs[0].text, "А", "text extraction retains ToUnicode semantics");
}

/// Exercise truncated alternate programs at several lengths: printable text remains
/// visible and explicit 600-unit advances survive parsing failures.
#[test]
fn truncated_fallback_programs_fail_safely_with_visible_printable_text() {
    let program: &'static [u8] = pdfcraft_fonts::INTER_REGULAR;
    let bytes = glyph_pdf(encoded_font("ArialMT", &[(192, "afii10017")]), &[192]);
    for length in [0, 12, 64, program.len() / 2, program.len() - 1] {
        let (s, _) = settings(Some(&program[..length]));
        let glyphs = probe(bytes.clone(), s);
        assert!(!glyphs[0].path.as_ref().unwrap().elements().is_empty(), "truncation at {length} silently discarded printable text");
        assert_eq!(glyphs[0].advance, Some(600.0));
    }
}

/// Keep primary A outlines for Unicode, suffix and ToUnicode aliases without
/// requesting an alternate face.
#[test]
fn unicode_aliases_and_to_unicode_keep_a_supported_primary_outline() {
    let mut font = encoded_font("ArialMT", &[(192, "uni0041"), (193, "unknownName"), (194, "A.alt")]);
    font.as_dict_mut().unwrap().set("ToUnicode", to_unicode("1 beginbfchar <C1> <0041> endbfchar"));
    let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
    let glyphs = probe(glyph_pdf(font, &[65, 192, 193, 194]), s);
    assert_eq!(calls.load(Ordering::Relaxed), 0, "all four codes can draw with the primary face");
    for glyph in &glyphs {
        assert_eq!(glyph.path, glyphs[0].path);
    }
}

/// Keep the primary A advance (667 units) and following positions for aliases without
/// PDF widths in both Helvetica and ArialMT.
#[test]
fn unicode_aliases_without_pdf_widths_keep_primary_advances() {
    for base in ["Helvetica", "ArialMT"] {
        let mut font = encoded_font(base, &[(192, "uni0041"), (193, "unknownName"), (194, "A.alt")]);
        font.as_dict_mut().unwrap().remove(b"Widths");
        font.as_dict_mut().unwrap().set("ToUnicode", to_unicode("1 beginbfchar <C1> <0041> endbfchar"));
        let (s, calls) = settings(Some(pdfcraft_fonts::INTER_REGULAR));
        let glyphs = probe(glyph_pdf(font, &[65, 192, 193, 194, 66]), s);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        for (i, glyph) in glyphs.iter().take(4).enumerate() {
            assert_eq!(glyph.advance, Some(667.0), "{base}, code index {i}");
            assert!((glyphs[i + 1].matrix[4] - glyph.matrix[4] - 13.34).abs() < 0.0001, "{base}, code index {i}");
        }
    }
}

/// Create an in-memory Inter variant retaining A's cmap/post names but emptying its
/// glyf record. Require expected tables and glyph record; never write a font asset.
fn primary_face_without_a_outline() -> Vec<u8> {
    use skrifa::{FontRef, MetadataProvider};
    let mut bytes = pdfcraft_fonts::INTER_REGULAR.to_vec();
    let glyph = FontRef::new(&bytes).unwrap().charmap().map('A').unwrap().to_u32() as usize;
    let u16_at = |b: &[u8], at: usize| u16::from_be_bytes(b[at..at + 2].try_into().unwrap());
    let u32_at = |b: &[u8], at: usize| u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize;
    let table = |tag: &[u8]| {
        (0..u16_at(&bytes, 4) as usize)
            .find_map(|i| {
                let record = 12 + 16 * i;
                (&bytes[record..record + 4] == tag).then(|| u32_at(&bytes, record + 8))
            })
            .unwrap()
    };
    let (glyf, loca, head) = (table(b"glyf"), table(b"loca"), table(b"head"));
    let offset =
        |index| if u16_at(&bytes, head + 50) == 0 { u16_at(&bytes, loca + index * 2) as usize * 2 } else { u32_at(&bytes, loca + index * 4) };
    let (start, end) = (glyf + offset(glyph), glyf + offset(glyph + 1));
    assert!(end > start);
    bytes[start..end].fill(0);
    bytes
}

/// Empty named/cmap primaries must reach an alternate or box, retain explicit widths,
/// reuse per-code resolution and leave spaces and primary B intact.
#[test]
fn empty_primary_glyphs_use_alternate_outlines_or_missing_boxes() {
    for alternate in [true, false] {
        let primary: FontData = Arc::new(primary_face_without_a_outline());
        let mut font = encoded_font("Helvetica", &[(192, "A"), (193, "uni0041"), (194, "unknownName")]);
        font.as_dict_mut().unwrap().set("ToUnicode", to_unicode("1 beginbfchar <C2> <0041> endbfchar"));
        let (mut s, calls) = settings(alternate.then_some(pdfcraft_fonts::INTER_REGULAR));
        let resolver = s.font_resolver.clone();
        s.font_resolver = Arc::new(move |query| match query {
            FontQuery::Standard(_) => Some((primary.clone(), 0)),
            _ => resolver(query),
        });
        let glyphs = probe(glyph_pdf(font, &[192, 193, 194, 192, 32, 66]), s);
        for glyph in glyphs.iter().take(4) {
            assert!(!glyph.path.as_ref().unwrap().elements().is_empty(), "alternate {alternate}: {glyph:?}");
            assert_eq!(glyph.advance, Some(600.0));
            assert!(!glyph.embedded);
        }
        assert!(glyphs[4].path.as_ref().unwrap().elements().is_empty(), "space must remain invisible");
        assert!(!glyphs[5].path.as_ref().unwrap().elements().is_empty(), "supported B must retain its outline");
        assert_eq!(calls.load(Ordering::Relaxed), 3, "each empty primary code is resolved once");
    }
}

/// Without PDF widths, rejected empty aliases must keep alternate natural advances
/// or 600-unit box advances consistently across repeats and following positions.
#[test]
fn empty_primary_aliases_without_pdf_widths_keep_fallback_metrics_consistent() {
    use skrifa::{
        FontRef, MetadataProvider,
        instance::{LocationRef, Size},
    };
    for alternate in [true, false] {
        let primary: FontData = Arc::new(primary_face_without_a_outline());
        let mut font = encoded_font("Helvetica", &[(192, "uni0041")]);
        font.as_dict_mut().unwrap().remove(b"Widths");
        let (mut s, _) = settings(alternate.then_some(pdfcraft_fonts::INTER_REGULAR));
        let resolver = s.font_resolver.clone();
        s.font_resolver = Arc::new(move |query| match query {
            FontQuery::Standard(_) => Some((primary.clone(), 0)),
            _ => resolver(query),
        });
        let glyphs = probe(glyph_pdf(font, &[192, 192]), s);
        let face = FontRef::new(pdfcraft_fonts::INTER_REGULAR).unwrap();
        let expected = if alternate {
            face.glyph_metrics(Size::new(1000.0), LocationRef::default()).advance_width(face.charmap().map('A').unwrap()).unwrap()
        } else {
            600.0
        };
        assert_eq!(glyphs[0].advance, Some(expected));
        assert!(!glyphs[0].path.as_ref().unwrap().elements().is_empty());
        assert!((glyphs[1].matrix[4] - glyphs[0].matrix[4] - f64::from(expected) * 0.02).abs() < 0.0001);
    }
}
