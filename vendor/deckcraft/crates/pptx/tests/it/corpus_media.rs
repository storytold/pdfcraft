//! Owned corpus: SmartArt and OLE kept as XML, audio and video bookmarks and trims, a section with a
//! hidden slide, slide-number and date fields, ink, 3-D models, and animation repeat, bounce and triggers.
//!
//! Relationship-bearing SmartArt and OLE parts stay with issue #203. Media playback options stay
//! with #74. Animation rewind (`cTn fill="remove"`) stays with #77. This file does not assert those.

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use deckcraft_color::Rgba;
use deckcraft_geom::Xfrm;
use deckcraft_model::anim::{AnimClass, Animation};
use deckcraft_model::text::{Run, RunKind, RunProps, TextBody};
use deckcraft_model::{Geom, InkStroke, MediaClip, Presentation, Section, Shape, ShapeId, ShapeKind, Slide, SlideId, defaults};

fn png(rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb(rgb))).write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

fn glb(marker: u8) -> Vec<u8> {
    let mut v = b"glTF\x02\x00\x00\x00\x0d\x00\x00\x00".to_vec();
    v.push(marker);
    v
}

fn shape(p: &mut Presentation, name: &str, x: f64, y: f64, w: f64, h: f64) -> Shape {
    Shape { id: ShapeId(p.alloc_id().expect("id")), name: name.into(), xfrm: Some(Xfrm::new(x, y, w, h)), ..Default::default() }
}

fn new_slide(p: &mut Presentation, title: &str) -> Slide {
    let layout = defaults::layout_of_kind(p, deckcraft_model::LayoutType::TitleOnly).unwrap_or_default();
    let mut s = defaults::new_slide(p, layout).expect("slide");
    if let Some(t) = s.shapes.first_mut() {
        t.text = Some(TextBody::from_text(title));
    }
    s
}

fn field(text: &str, kind: &str) -> TextBody {
    let mut body = TextBody::from_text("");
    body.paragraphs[0].runs = vec![Run { text: text.into(), props: RunProps::default(), kind: RunKind::Field { field: kind.into() } }];
    body
}

const SMARTART: &str = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr name="Diagram"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="3657600" cy="1828800"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/><dgm:pt xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:t>Quarter</dgm:t></dgm:pt></a:graphicData></a:graphic></p:graphicFrame>"#;

const OLE: &str = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr name="Embedded"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="3810000" y="0"/><a:ext cx="1828800" cy="914400"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"><p:oleObj name="Note" progId="DeckCraft.Note.1"><p:embed/></p:oleObj></a:graphicData></a:graphic></p:graphicFrame>"#;

fn corpus() -> (Presentation, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut p = defaults::new_presentation(None);
    let audio_bytes = b"audio-bytes-deckcraft".to_vec();
    let video_bytes = b"video-bytes-deckcraft".to_vec();
    let block = glb(1);
    let globe = glb(2);
    let poster = png([20, 80, 160]);
    let audio = p.add_media("clip.m4a", "audio/mp4", audio_bytes).expect("media");
    let video = p.add_media("clip.mp4", "video/mp4", video_bytes).expect("media");
    let block_id = p.add_media("block.glb", "model/gltf-binary", block.clone()).expect("media");
    let globe_id = p.add_media("globe.glb", "model/gltf-binary", globe.clone()).expect("media");
    let poster_id = p.add_media("globe-poster.png", "image/png", poster.clone()).expect("media");

    let mut objects = new_slide(&mut p, "Objects");
    let mut diagram = shape(&mut p, "Diagram", 40.0, 80.0, 288.0, 144.0);
    diagram.kind = ShapeKind::Opaque { xml: SMARTART.into(), preview: None, label: "SmartArt".into(), parts: vec![] };
    let mut embedded = shape(&mut p, "Embedded", 340.0, 80.0, 144.0, 72.0);
    embedded.kind = ShapeKind::Opaque { xml: OLE.into(), preview: None, label: "DeckCraft.Note.1".into(), parts: vec![] };
    let mut block_shape = shape(&mut p, "Block", 40.0, 240.0, 120.0, 90.0);
    block_shape.kind = ShapeKind::Model3d { media: block_id, poster: None };
    let mut globe_shape = shape(&mut p, "Globe", 180.0, 240.0, 160.0, 120.0);
    globe_shape.kind = ShapeKind::Model3d { media: globe_id, poster: Some(poster_id) };
    let mut number = shape(&mut p, "Slide number", 360.0, 240.0, 80.0, 28.0);
    number.text = Some(field("4", "slidenum"));
    let mut date = shape(&mut p, "Date", 360.0, 276.0, 160.0, 28.0);
    date.text = Some(field("14 October 2026", "datetime4"));
    objects.shapes.extend([diagram, embedded, block_shape, globe_shape, number, date]);
    p.slides.push(Arc::new(objects));

    let mut media = new_slide(&mut p, "Media");
    let mut clip = shape(&mut p, "Narration", 40.0, 80.0, 80.0, 80.0);
    clip.kind = ShapeKind::Media(MediaClip {
        media: audio,
        video: false,
        trim_start_ms: 400,
        trim_end_ms: 2600,
        fade_in_ms: 80,
        fade_out_ms: 120,
        bookmarks: vec![("Intro".into(), 400), ("Outro".into(), 2200)],
        volume: 1.0,
        ..Default::default()
    });
    let mut film = shape(&mut p, "Film", 160.0, 80.0, 240.0, 135.0);
    film.kind = ShapeKind::Media(MediaClip {
        media: video,
        video: true,
        poster: Some(poster_id),
        trim_start_ms: 250,
        trim_end_ms: 9000,
        bookmarks: vec![("Mark".into(), 1500)],
        volume: 1.0,
        ..Default::default()
    });
    let mut trigger = shape(&mut p, "Trigger", 40.0, 240.0, 100.0, 36.0);
    trigger.text = Some(TextBody::from_text("Play"));
    let mut fader = shape(&mut p, "Fader", 160.0, 240.0, 140.0, 60.0);
    fader.text = Some(TextBody::from_text("Hello"));
    let trigger_id = trigger.id;
    let fader_id = fader.id;
    media.shapes.extend([clip, film, trigger, fader]);
    media.animations = vec![
        Animation {
            shape: fader_id,
            effect: "fade".into(),
            duration_ms: 600,
            repeat: 4,
            bounce_end: 0.25,
            trigger: Some(trigger_id),
            ..Default::default()
        },
        Animation { shape: fader_id, class: AnimClass::Emphasis, effect: "pulse".into(), duration_ms: 400, repeat: u32::MAX, ..Default::default() },
    ];
    p.slides.push(Arc::new(media));

    let mut aside = new_slide(&mut p, "Aside");
    aside.hidden = true;
    let mut ink = shape(&mut p, "Sketch", 48.0, 96.0, 200.0, 80.0);
    ink.kind = ShapeKind::Ink {
        strokes: vec![
            InkStroke {
                points: vec![(48.0, 100.0, 0.4), (90.0, 140.0, 0.8), (150.0, 110.0, 0.3)],
                color: Rgba::rgb(20, 20, 20),
                width: 2.0,
                highlighter: false,
            },
            InkStroke { points: vec![(50.0, 150.0, 1.0), (160.0, 156.0, 1.0)], color: Rgba::rgb(255, 230, 40), width: 12.0, highlighter: true },
        ],
    };
    aside.shapes.push(ink);
    p.slides.push(Arc::new(aside));

    let ids: Vec<SlideId> = p.slides.iter().map(|s| s.id).collect();
    let hidden = *ids.last().expect("aside");
    p.sections = vec![
        Section { name: "Open".into(), slides: ids.iter().copied().filter(|id| *id != hidden).collect() },
        Section { name: "Aside".into(), slides: vec![hidden] },
    ];
    p.first_slide_number = 4;
    (p, block, globe, poster)
}

fn round(p: &Presentation) -> Presentation {
    let bytes = deckcraft_pptx::export(p).expect("export");
    deckcraft_pptx::import(&bytes).expect("import")
}

fn by_name<'a>(p: &'a Presentation, name: &str) -> &'a Shape {
    let mut found = None;
    for s in &p.slides {
        deckcraft_model::walk(&s.shapes, &mut |x, _| {
            if found.is_none() && x.name == name {
                found = Some(x);
            }
        });
    }
    found.unwrap_or_else(|| panic!("no shape {name}"))
}

fn media_bytes(p: &Presentation, shape: &str) -> Vec<u8> {
    let id = match &by_name(p, shape).kind {
        ShapeKind::Model3d { media, .. } | ShapeKind::Media(MediaClip { media, .. }) => *media,
        other => panic!("{shape} has no media: {other:?}"),
    };
    p.media(id).expect("media").data.as_ref().clone()
}

fn assert_field(p: &Presentation, name: &str, kind: &str, text: &str) {
    let run = &by_name(p, name).text.as_ref().expect("text").paragraphs[0].runs[0];
    assert!(matches!(&run.kind, RunKind::Field { field } if field == kind), "{name}: {:?}", run.kind);
    assert_eq!(run.text, text);
}

fn assert_ink(s: &Shape) {
    let ShapeKind::Group { children, .. } = &s.kind else { panic!("ink comes back as a group, got {:?}", s.kind) };
    assert_eq!(children.len(), 2, "pen and highlighter");
    assert!(children.iter().all(|c| matches!(c.geom, Geom::Custom { .. })));
}

#[test]
fn object_corpus_round_trips() {
    let (p, block, globe, poster) = corpus();
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let types = zip_text(&bytes, "[Content_Types].xml");
    assert!(types.contains("model/gltf-binary"), "{types}");
    assert!(zip_has_payload(&bytes, &block), "block model was not written");
    assert!(zip_has_payload(&bytes, &globe), "globe model was not written");
    let once = deckcraft_pptx::import(&bytes).expect("import");
    let twice = round(&once);

    for q in [&once, &twice] {
        opaque_parts_survive(q);
        models_survive(q, &block, &globe, &poster);
        media_clips_survive(q);
        fields_and_sections_survive(q);
        ink_and_timing_survive(q);
    }
}

fn opaque_parts_survive(q: &Presentation) {
    let diagram = by_name(q, "Diagram");
    let ShapeKind::Opaque { xml, label, .. } = &diagram.kind else { panic!("{:?}", diagram.kind) };
    assert_eq!(label, "SmartArt");
    assert!(xml.contains("Quarter"), "{xml}");
    assert!(!xml.contains("r:dm"), "diagram parts are not part of this corpus");

    let embedded = by_name(q, "Embedded");
    let ShapeKind::Opaque { xml, label, .. } = &embedded.kind else { panic!("{:?}", embedded.kind) };
    assert_eq!(label, "DeckCraft.Note.1");
    assert!(xml.contains("DeckCraft.Note.1"), "{xml}");
    assert!(!xml.contains("r:id"), "an embedding part is issue #203");
}

fn models_survive(q: &Presentation, block: &[u8], globe: &[u8], poster: &[u8]) {
    let block_shape = by_name(q, "Block");
    assert!(matches!(block_shape.kind, ShapeKind::Model3d { poster: None, .. }), "{:?}", block_shape.kind);
    assert_eq!(block_shape.xfrm, Some(Xfrm::new(40.0, 240.0, 120.0, 90.0)));
    assert_eq!(media_bytes(q, "Block"), block);
    let ShapeKind::Model3d { media, .. } = &block_shape.kind else { panic!("{:?}", block_shape.kind) };
    let block_media = q.media(*media).expect("block media");
    assert_eq!(block_media.content_type, "model/gltf-binary");
    assert!(block_media.name.to_ascii_lowercase().ends_with(".glb"), "{}", block_media.name);

    let globe_shape = by_name(q, "Globe");
    let ShapeKind::Model3d { media, poster: Some(poster_media) } = &globe_shape.kind else { panic!("{:?}", globe_shape.kind) };
    assert_eq!(globe_shape.xfrm, Some(Xfrm::new(180.0, 240.0, 160.0, 120.0)));
    assert_eq!(q.media(*media).expect("globe").data.as_ref(), globe);
    assert_eq!(q.media(*poster_media).expect("poster").data.as_ref(), poster);
}

fn media_clips_survive(q: &Presentation) {
    let narration = by_name(q, "Narration");
    let ShapeKind::Media(m) = &narration.kind else { panic!("{:?}", narration.kind) };
    assert!(!m.video);
    assert_eq!(m.trim_start_ms, 400);
    assert_eq!(m.trim_end_ms, 2600);
    assert_eq!(m.fade_in_ms, 80);
    assert_eq!(m.fade_out_ms, 120);
    assert_eq!(m.bookmarks, vec![("Intro".into(), 400), ("Outro".into(), 2200)]);
    assert_eq!(q.media(m.media).expect("audio").data.as_ref(), b"audio-bytes-deckcraft".as_slice());

    let film = by_name(q, "Film");
    let ShapeKind::Media(m) = &film.kind else { panic!("{:?}", film.kind) };
    assert!(m.video);
    assert_eq!(m.trim_start_ms, 250);
    assert_eq!(m.trim_end_ms, 9000);
    assert_eq!(m.bookmarks, vec![("Mark".into(), 1500)]);
    assert_eq!(q.media(m.media).expect("video").data.as_ref(), b"video-bytes-deckcraft".as_slice());
}

fn fields_and_sections_survive(q: &Presentation) {
    assert_field(q, "Slide number", "slidenum", "4");
    assert_field(q, "Date", "datetime4", "14 October 2026");
    assert_eq!(q.first_slide_number, 4);

    assert_eq!(q.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Open", "Aside"]);
    let hidden = q.slides.iter().position(|s| s.hidden).expect("a hidden slide");
    assert_eq!(q.slides.iter().filter(|s| s.hidden).count(), 1);
    assert!(q.sections[1].slides.contains(&q.slides[hidden].id));
    assert!(!q.sections[0].slides.contains(&q.slides[hidden].id));
}

fn ink_and_timing_survive(q: &Presentation) {
    assert_ink(by_name(q, "Sketch"));

    let timing = q.slides.iter().find(|s| s.animations.iter().any(|a| a.effect == "fade")).expect("timing slide");
    let fade = timing.animations.iter().find(|a| a.effect == "fade").expect("fade");
    let pulse = timing.animations.iter().find(|a| a.effect == "pulse").expect("pulse");
    assert_eq!(fade.repeat, 4);
    assert!((fade.bounce_end - 0.25).abs() < 1e-9, "{}", fade.bounce_end);
    assert_eq!(fade.trigger, Some(by_name(q, "Trigger").id));
    assert_eq!(fade.shape, by_name(q, "Fader").id);
    assert_eq!(pulse.repeat, u32::MAX);
    assert_eq!(pulse.class, AnimClass::Emphasis);
    assert!(pulse.trigger.is_none());
}

/// An exported deck with `parts` added (or replaced) and `edit` applied to every XML part.
fn with_parts(parts: &[(&str, Vec<u8>)], edit: impl Fn(&str, String) -> String) -> Vec<u8> {
    let base = deckcraft_pptx::export(&defaults::new_presentation(None)).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(base)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let name = f.name().to_string();
        if parts.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        if name.ends_with(".xml") || name.ends_with(".rels") {
            d = edit(&name, String::from_utf8(d).expect("utf8")).into_bytes();
        }
        out.start_file(name, o).expect("start");
        out.write_all(&d).expect("write");
    }
    for (name, data) in parts {
        out.start_file(*name, o).expect("start");
        out.write_all(data).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

fn zip_text(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut f = z.by_name(name).unwrap_or_else(|_| panic!("no {name}"));
    let mut d = String::new();
    f.read_to_string(&mut d).expect("read");
    d
}

fn zip_has_payload(bytes: &[u8], payload: &[u8]) -> bool {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        if d == payload {
            return true;
        }
    }
    false
}

#[test]
fn an_alternate_content_model_keeps_the_glb_and_its_poster() {
    let model = glb(7);
    let poster = png([200, 30, 40]);
    let frame = r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:am3d="http://schemas.microsoft.com/office/drawing/2017/model3d" Requires="am3d"><p:contentPart r:id="rIdModel"><p:nvContentPartPr><p:cNvPr id="60" name="Model from file"/><p:cNvContentPartPr/><p:nvPr/></p:nvContentPartPr><p:xfrm><a:off x="508000" y="635000"/><a:ext cx="1524000" cy="1143000"/></p:xfrm></p:contentPart></mc:Choice><mc:Fallback><p:pic><p:nvPicPr><p:cNvPr id="61" name="Model poster"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdPoster"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="508000" y="635000"/><a:ext cx="1524000" cy="1143000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></mc:Fallback></mc:AlternateContent>"#;
    let bytes = with_parts(&[("ppt/media/model.glb", model.clone()), ("ppt/media/poster.png", poster.clone())], |name, s| {
        if name == "ppt/slides/slide1.xml" {
            s.replacen("</p:spTree>", &format!("{frame}</p:spTree>"), 1)
        } else if name == "ppt/slides/_rels/slide1.xml.rels" {
            s.replacen(
                    "</Relationships>",
                    r#"<Relationship Id="rIdModel" Type="http://schemas.microsoft.com/office/2017/06/relationships/model3d" Target="../media/model.glb"/><Relationship Id="rIdPoster" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/poster.png"/></Relationships>"#,
                    1,
                )
        } else {
            s
        }
    });
    let once = deckcraft_pptx::import(&bytes).expect("import");
    let twice = round(&once);
    for q in [&once, &twice] {
        assert!(q.slides[0].shapes.iter().all(|s| s.name != "Model poster"), "the fallback picture replaced the model");
        let s = by_name(q, "Model from file");
        let ShapeKind::Model3d { media, poster: Some(poster_id) } = &s.kind else { panic!("{:?}", s.kind) };
        assert_eq!(s.xfrm, Some(Xfrm::new(40.0, 50.0, 120.0, 90.0)));
        let model_item = q.media(*media).expect("glb");
        assert_eq!(model_item.data.as_ref(), model.as_slice());
        assert_eq!(model_item.content_type, "model/gltf-binary");
        assert_eq!(q.media(*poster_id).expect("poster").data.as_ref(), poster.as_slice());
    }
}

#[test]
fn an_ink_content_part_still_uses_its_fallback_picture() {
    let poster = png([10, 10, 10]);
    let frame = r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="p14"><p:contentPart r:id="rIdInk"><p:nvContentPartPr><p:cNvPr id="50" name="Ink marks"/><p:cNvContentPartPr/><p:nvPr/></p:nvContentPartPr></p:contentPart></mc:Choice><mc:Fallback><p:pic><p:nvPicPr><p:cNvPr id="51" name="Ink poster"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdPoster"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic></mc:Fallback></mc:AlternateContent>"#;
    let bytes = with_parts(&[("ppt/ink/ink1.xml", b"<ink/>".to_vec()), ("ppt/media/ink-poster.png", poster.clone())], |name, s| {
        if name == "ppt/slides/slide1.xml" {
            s.replacen("</p:spTree>", &format!("{frame}</p:spTree>"), 1)
        } else if name == "ppt/slides/_rels/slide1.xml.rels" {
            s.replacen(
                    "</Relationships>",
                    r#"<Relationship Id="rIdInk" Type="http://schemas.microsoft.com/office/2007/relationships/ink" Target="../ink/ink1.xml"/><Relationship Id="rIdPoster" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/ink-poster.png"/></Relationships>"#,
                    1,
                )
        } else {
            s
        }
    });
    let p = deckcraft_pptx::import(&bytes).expect("import");
    assert!(p.slides[0].shapes.iter().all(|s| s.name != "Ink marks"));
    let s = by_name(&p, "Ink poster");
    let ShapeKind::Picture { fill } = &s.kind else { panic!("{:?}", s.kind) };
    assert_eq!(p.media(fill.media).expect("poster").data.as_ref(), poster.as_slice());
}
