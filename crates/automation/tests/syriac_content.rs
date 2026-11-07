//! Syriac content through the same tools exposed by CLI and MCP.
use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::{DocId, Session};
use serde_json::{Value, json};

const ORIGINAL: &str = "\u{072b}\u{0730}\u{0720}\u{0735}\u{0721} PDF 123";
const EDITED: &str = "\u{0712}\u{0730}\u{0712} ABC 456";

/// Allow optional-font skips locally while requiring the Syriac face in qualification runs.
fn available() -> bool {
    let present = pdfcraft_fonts::document_syriac_font().is_some();
    assert!(present || std::env::var_os("PDFCRAFT_REQUIRE_SYRIAC").is_none(), "qualification requires Syriac fonts");
    present
}

/// Invoke an automation tool and require its first response to be structured JSON.
fn call(auto: &mut Automation, name: &str, args: Value) -> Value {
    match auto.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON from {name}: {other:?}"),
    }
}

/// Require the operation to produce no text-fidelity warnings.
fn no_warning(result: &Value) {
    assert!(result.get("warnings").and_then(Value::as_array).is_none_or(Vec::is_empty), "{result}");
}

/// Write and open a synthetic one-page PDF inside a temporary automation root.
fn blank(dir: &std::path::Path) -> (Automation, u64) {
    let session = Session::new();
    let bytes = session.create_blank(600.0, 800.0, 1).unwrap();
    std::fs::write(dir.join("blank.pdf"), bytes.as_slice()).unwrap();
    let mut auto = Automation::new().with_root(dir).unwrap();
    let id = call(&mut auto, "doc_open", json!({"path":"blank.pdf"}))["doc"].as_u64().unwrap();
    (auto, id)
}

/// Read and trim the first page's extracted logical text through automation.
fn extracted(auto: &mut Automation, id: u64) -> String {
    call(auto, "text_extract", json!({"doc":id}))["pages"][0]["text"].as_str().unwrap().trim().to_owned()
}

/// Build a minimal PDF containing one editable Helvetica text run for source-font tests.
fn source_page() -> Vec<u8> {
    let contents = "BT /F1 16 Tf 1 0 0 1 40 700 Tm (Original) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{contents}\nendstream", contents.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).bytes());
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    bytes.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
    bytes
}

/// Check existing-text fallback and imported full faces through edits, undo/redo and save/reopen.
#[test]
fn existing_paragraph_uses_approved_fallback_and_explicit_source_styles() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source.pdf"), source_page()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let id = call(&mut auto, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
    call(&mut auto, "text_edit", json!({"doc":id,"page":1,"paragraph":1,"text":ORIGINAL,"width":450}));
    assert_eq!(extracted(&mut auto, id), ORIGINAL);
    call(&mut auto, "edit_undo", json!({"doc":id}));
    assert_eq!(extracted(&mut auto, id), "Original");
    call(&mut auto, "edit_redo", json!({"doc":id}));
    assert_eq!(extracted(&mut auto, id), ORIGINAL);
    for (index, face) in pdfcraft_fonts::CRAFT_FONTS.iter().filter(|face| face.covers("Syrc")).enumerate() {
        let filename = format!("face-{index}.ttf");
        std::fs::write(dir.path().join(&filename), face.bytes).unwrap();
        let mut auto = Automation::new().with_root(dir.path()).unwrap();
        let id = call(&mut auto, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
        let imported = call(&mut auto, "text_font_import", json!({"doc":id,"page":1,"path":filename}));
        let resource = imported["resource"].as_str().unwrap();
        call(&mut auto, "text_edit", json!({"doc":id,"page":1,"paragraph":1,"text":ORIGINAL,"width":450,"font_resource":resource}));
        assert_eq!(extracted(&mut auto, id), ORIGINAL, "{}", face.family);
        call(&mut auto, "text_edit", json!({"doc":id,"page":1,"paragraph":1,"text":EDITED,"width":450,"font_resource":resource}));
        assert_eq!(extracted(&mut auto, id), EDITED);
        let pixels = auto.render_pixels(id, 1, 1.0).unwrap().2;
        call(&mut auto, "doc_save", json!({"doc":id,"path":"source-edited.pdf"}));
        let re = call(&mut auto, "doc_open", json!({"path":"source-edited.pdf"}))["doc"].as_u64().unwrap();
        assert_eq!(extracted(&mut auto, re), EDITED);
        assert_eq!(auto.render_pixels(re, 1, 1.0).unwrap().2, pixels);
    }
}

/// Added Syriac text preserves logical extraction, search, history and rendered saved output.
#[test]
fn added_text_marks_mixed_bidi_history_search_and_reopen() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut auto, id) = blank(dir.path());
    let (_, _, empty) = auto.render_pixels(id, 1, 1.0).unwrap();
    call(&mut auto, "page_add_text", json!({"doc":id,"page":1,"text":ORIGINAL,"at":[40,60],"width":500,"size":18}));
    assert_eq!(extracted(&mut auto, id), ORIGINAL);
    let found = call(&mut auto, "text_find", json!({"doc":id,"query":ORIGINAL}));
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);
    call(&mut auto, "content_update", json!({"doc":id,"page":1,"index":1,"text":EDITED}));
    assert_eq!(extracted(&mut auto, id), EDITED);
    call(&mut auto, "edit_undo", json!({"doc":id}));
    assert_eq!(extracted(&mut auto, id), ORIGINAL);
    call(&mut auto, "edit_redo", json!({"doc":id}));
    let before = auto.session().save_bytes(DocId(id)).unwrap();
    assert!(auto.call("content_update", &json!({"doc":id,"page":1,"index":1,"text":"\u{0860}"})).is_err());
    assert_eq!(auto.session().save_bytes(DocId(id)).unwrap(), before, "unsupported supplement must not change the document");
    let (_, _, pixels) = auto.render_pixels(id, 1, 1.0).unwrap();
    assert_ne!(pixels, empty, "glyphs must reach the renderer");
    call(&mut auto, "doc_save", json!({"doc":id,"path":"edited.pdf"}));
    let reopened = call(&mut auto, "doc_open", json!({"path":"edited.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(extracted(&mut auto, reopened), EDITED);
    assert_eq!(auto.render_pixels(reopened, 1, 1.0).unwrap().2, pixels);
}

/// Default Syriac appearances on forms, comments, marks and signatures survive save/reopen.
#[test]
fn forms_comments_page_marks_and_signatures_keep_rendered_appearances() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut auto, id) = blank(dir.path());
    let (_, _, empty) = auto.render_pixels(id, 1, 1.0).unwrap();
    for (name, kind, rect, extra) in [
        ("single", "text", [40, 80, 270, 115], json!({})),
        ("multi", "text", [40, 130, 270, 205], json!({"multiline":true})),
        ("choice", "combo", [40, 225, 270, 260], json!({"options":[ORIGINAL,EDITED]})),
        ("list", "list", [40, 280, 270, 360], json!({"options":[ORIGINAL,EDITED],"multi_select":true})),
        ("button", "button", [40, 380, 270, 420], json!({"caption":ORIGINAL})),
    ] {
        let mut args = json!({"doc":id,"page":1,"name":name,"type":kind,"rect":rect});
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        no_warning(&call(&mut auto, "form_add_field", args));
    }
    no_warning(&call(
        &mut auto,
        "form_fill",
        json!({"doc":id,"values":{"single":ORIGINAL,"multi":format!("{ORIGINAL}\n{EDITED}"),"choice":ORIGINAL,"list":[EDITED]}}),
    ));
    no_warning(&call(
        &mut auto,
        "comment_add",
        json!({"doc":id,"page":1,"type":"textbox","rect":[300,80,560,170],"contents":ORIGINAL,"font_size":16}),
    ));
    no_warning(&call(
        &mut auto,
        "comment_add",
        json!({"doc":id,"page":1,"type":"callout","rect":[300,200,560,280],"from":[280,190],"to":[300,220],"contents":EDITED,"font_size":16}),
    ));
    no_warning(&call(&mut auto, "fill_sign_add", json!({"doc":id,"page":1,"type":"text","at":[300,330],"text":ORIGINAL,"font_size":16})));
    no_warning(&call(&mut auto, "fill_sign_add", json!({"doc":id,"page":1,"type":"signature","at":[300,400],"text":EDITED})));
    call(&mut auto, "doc_header_footer", json!({"doc":id,"header_left":ORIGINAL,"footer_right":EDITED,"font_size":12}));
    call(&mut auto, "doc_watermark", json!({"doc":id,"text":ORIGINAL,"font_size":18,"rotation":0,"offset_y":-170}));
    let fields = call(&mut auto, "form_fields", json!({"doc":id}));
    assert!(fields["fields"].as_array().unwrap().iter().any(|field| field["name"] == "button" && field["caption"] == ORIGINAL));
    let (_, _, pixels) = auto.render_pixels(id, 1, 1.0).unwrap();
    assert_ne!(pixels, empty);
    call(&mut auto, "doc_save", json!({"doc":id,"path":"surfaces.pdf"}));
    let re = call(&mut auto, "doc_open", json!({"path":"surfaces.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(call(&mut auto, "form_fields", json!({"doc":re})), fields);
    assert_eq!(auto.render_pixels(re, 1, 1.0).unwrap().2, pixels);
}

/// Compare all three families in isolated rendered regions across the supported content writers.
/// Verify readback persistence, unrelated edits, save/reopen and atomic refusal of invalid selectors.
#[test]
fn selected_families_persist_across_content_surfaces_and_refuse_irrelevant_selectors() {
    use pdfcraft_fonts::{SyriacFamily, document_syriac_font_for_family};
    if !available() {
        return;
    }
    let complete = SyriacFamily::ALL.into_iter().all(|family| document_syriac_font_for_family(family).is_some());
    assert!(complete || std::env::var_os("PDFCRAFT_REQUIRE_SYRIAC").is_none(), "qualification requires all three Syriac families");
    if !complete {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Each region isolates one writer, so a working page-text family cannot hide a
    // default-family regression in forms, annotations, signatures or page marks.
    let regions = [
        ("header", [0, 0, 600, 60]),
        ("added text", [0, 60, 600, 110]),
        ("single field", [35, 115, 285, 165]),
        ("multiline field", [35, 175, 285, 255]),
        ("comb field", [35, 265, 285, 315]),
        ("combo", [35, 325, 285, 375]),
        ("list", [35, 385, 285, 475]),
        ("button caption", [35, 485, 285, 535]),
        ("textbox", [310, 115, 580, 175]),
        ("callout", [310, 195, 580, 255]),
        ("Fill & Sign text", [310, 290, 600, 355]),
        ("typed signature", [310, 365, 600, 435]),
        ("watermark", [0, 610, 600, 715]),
        ("footer", [0, 740, 600, 800]),
    ];
    let mut rendered: Vec<(&str, Vec<Vec<u8>>)> = Vec::new();
    for family in SyriacFamily::ALL {
        let family = family.name();
        let (mut auto, id) = blank(dir.path());
        no_warning(&call(
            &mut auto,
            "page_add_text",
            json!({
                "doc":id,"page":1,"text":ORIGINAL,"at":[40,70],"width":500,"size":18,"syriac_family":family,
            }),
        ));
        for (name, kind, rect, extra) in [
            ("single", "text", [40, 120, 280, 160], json!({})),
            ("multi", "text", [40, 180, 280, 250], json!({"multiline":true})),
            ("comb", "text", [40, 270, 280, 310], json!({})),
            ("combo", "combo", [40, 330, 280, 370], json!({"options":[ORIGINAL,EDITED]})),
            ("list", "list", [40, 390, 280, 470], json!({"options":[ORIGINAL,EDITED]})),
            ("button", "button", [40, 490, 280, 530], json!({"caption":ORIGINAL})),
            ("check", "checkbox", [40, 550, 60, 570], json!({})),
        ] {
            let mut args = json!({"doc":id,"page":1,"name":name,"type":kind,"rect":rect});
            args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            no_warning(&call(&mut auto, "form_add_field", args));
        }
        no_warning(&call(
            &mut auto,
            "form_set_props",
            json!({
                "doc":id,"fields":["single","multi","comb","combo","list","button"],"syriac_family":family,"font_size":18,
            }),
        ));
        no_warning(&call(&mut auto, "form_set_props", json!({"doc":id,"field":"comb","max_length":16,"font_size":12,"flags":{"comb":true}})));
        no_warning(&call(
            &mut auto,
            "form_fill",
            json!({
                "doc":id,"values":{"single":ORIGINAL,"multi":format!("{ORIGINAL}\n{EDITED}"),"comb":ORIGINAL,"combo":ORIGINAL,"list":ORIGINAL},
            }),
        ));
        let mut textbox_index = None;
        for (kind, rect, extra) in
            [("textbox", [320, 120, 570, 170], json!({})), ("callout", [320, 200, 570, 250], json!({"from":[300,190],"to":[320,220]}))]
        {
            let mut args = json!({"doc":id,"page":1,"type":kind,"rect":rect,"contents":ORIGINAL,"font_size":16,"syriac_family":family});
            args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            let added = call(&mut auto, "comment_add", args);
            no_warning(&added);
            if kind == "textbox" {
                textbox_index = Some(added["comment"]["index"].as_u64().unwrap());
            }
        }
        no_warning(&call(
            &mut auto,
            "fill_sign_add",
            json!({
                "doc":id,"page":1,"type":"text","at":[320,310],"text":ORIGINAL,"font_size":16,"syriac_family":family,
            }),
        ));
        no_warning(&call(
            &mut auto,
            "fill_sign_add",
            json!({
                "doc":id,"page":1,"type":"signature","at":[320,390],"text":ORIGINAL,"syriac_family":family,
            }),
        ));
        no_warning(&call(
            &mut auto,
            "doc_header_footer",
            json!({
                "doc":id,"header_left":ORIGINAL,"footer_right":ORIGINAL,"font_size":16,"syriac_family":family,
            }),
        ));
        no_warning(&call(
            &mut auto,
            "doc_watermark",
            json!({
                "doc":id,"text":ORIGINAL,"font_size":18,"rotation":0,"offset_y":-260,"opacity":1,"syriac_family":family,
            }),
        ));

        // Exercise updates that regenerate appearances without selecting a family again.
        no_warning(&call(&mut auto, "content_update", json!({"doc":id,"page":1,"index":1,"size":20})));
        no_warning(&call(&mut auto, "form_set_props", json!({"doc":id,"field":"single","tooltip":"retained family"})));
        no_warning(&call(&mut auto, "comment_edit", json!({"doc":id,"page":1,"index":textbox_index.unwrap(),"color":"#102030"})));
        let content = call(&mut auto, "content_list", json!({"doc":id}));
        assert_eq!(content["items"][0]["syriac_family"], family);
        let fields = call(&mut auto, "form_fields", json!({"doc":id}));
        let text_fields: Vec<_> = fields["fields"].as_array().unwrap().iter().filter(|field| field["name"] != "check").collect();
        assert_eq!(text_fields.len(), 6);
        assert!(text_fields.iter().all(|field| field["syriac_family"] == family), "{fields}");
        let comments = call(&mut auto, "comment_list", json!({"doc":id}));
        let annotations = comments["comments"].as_array().unwrap();
        assert_eq!(annotations.len(), 4);
        assert!(annotations.iter().all(|comment| comment["syriac_family"] == family), "{comments}");
        // Typed signatures are outline-backed Stamps, not Ink annotations. Fill & Sign
        // has no returned index, so resolve the fixture's sole Stamp through readback.
        let signatures: Vec<_> = annotations.iter().filter(|comment| comment["type"] == "Stamp").collect();
        assert_eq!(signatures.len(), 1);
        let signature_index = signatures[0]["index"].as_u64().unwrap();

        let before = auto.session().save_bytes(DocId(id)).unwrap();
        for (tool, args) in [
            ("form_set_props", json!({"doc":id,"field":"check","tooltip":"must not be stored","syriac_family":family})),
            ("fill_sign_add", json!({"doc":id,"page":1,"type":"check","at":[320,550],"syriac_family":family})),
            ("comment_edit", json!({"doc":id,"page":1,"index":signature_index,"contents":"must not be stored","syriac_family":family})),
            ("doc_background", json!({"doc":id,"color":"#eeeeee","syriac_family":family})),
            ("doc_watermark", json!({"doc":id,"file":"missing.png","syriac_family":family})),
            ("page_add_text", json!({"doc":id,"page":1,"text":ORIGINAL,"at":[40,600],"syriac_family":"unknown"})),
        ] {
            assert!(auto.call(tool, &args).is_err(), "{tool} must refuse {args}");
        }
        assert_eq!(auto.session().save_bytes(DocId(id)).unwrap(), before, "refused selectors must leave the document unchanged");

        let (width, height, pixels) = auto.render_pixels(id, 1, 1.0).unwrap();
        assert_eq!((width, height), (600, 800));
        let crops: Vec<Vec<u8>> = regions
            .iter()
            .map(|(_, [x0, y0, x1, y1])| {
                let mut crop = Vec::new();
                for y in *y0..*y1 {
                    let start = (y * width as usize + *x0) * 4;
                    crop.extend_from_slice(&pixels.as_ref()[start..start + (x1 - x0) * 4]);
                }
                crop
            })
            .collect();
        for (other, prior) in &rendered {
            for ((region, _), (current, previous)) in regions.iter().zip(crops.iter().zip(prior)) {
                assert_ne!(current, previous, "{region} must draw distinct {family} and {other} faces at the same text/coordinates");
            }
        }
        rendered.push((family, crops));
        let path = format!("selected-{family}.pdf");
        call(&mut auto, "doc_save", json!({"doc":id,"path":path}));
        let re = call(&mut auto, "doc_open", json!({"path":path}))["doc"].as_u64().unwrap();
        assert_eq!(call(&mut auto, "content_list", json!({"doc":re})), content);
        assert_eq!(call(&mut auto, "form_fields", json!({"doc":re})), fields);
        assert_eq!(call(&mut auto, "comment_list", json!({"doc":re})), comments);
        assert_eq!(auto.render_pixels(re, 1, 1.0).unwrap().2, pixels, "saved {family} appearances must survive reopening");
    }
}

#[test]
fn review2_syriac_and_other_rows_resolve_independently() {
    if !available() {
        return;
    }
    let face = pdfcraft_fonts::document_syriac_font().unwrap();
    let other = "\u{65e5}";
    assert!(!pdfcraft_fonts::embedding::covers(face.bytes, other));
    assert!(pdfcraft_fonts::embedding::covers(pdfcraft_fonts::document_japanese_font().unwrap().bytes, other));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source.pdf"), source_page()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let id = call(&mut auto, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
    let draft = format!("\u{072b}\u{0720}\u{0721}\u{0710}\n{other}");
    call(&mut auto, "text_edit", json!({"doc":id,"page":1,"paragraph":1,"text":draft,"width":450}));
    assert_eq!(extracted(&mut auto, id), draft);
}

#[test]
fn review2_mixed_hebrew_syriac_is_refused_atomically() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source.pdf"), source_page()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let id = call(&mut auto, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
    let before = extracted(&mut auto, id);
    let error = auto
        .call("text_edit", &json!({"doc":id,"page":1,"paragraph":1,"text":"\u{05d0}\u{0710}","width":450}))
        .expect_err("unsupported mixed-script paragraph");
    assert!(error.to_string().contains("mixing Hebrew and Syriac"), "reject before choosing a writer: {error}");
    assert_eq!(extracted(&mut auto, id), before);
}

#[test]
fn review3_source_family_precedes_unrelated_loaded_fonts() {
    use pdfcraft_fonts::{SyriacFamily, document_syriac_font_for_family, embedding};
    let (Some(eastern), Some(western)) =
        (document_syriac_font_for_family(SyriacFamily::Eastern), document_syriac_font_for_family(SyriacFamily::Western))
    else {
        assert!(std::env::var_os("PDFCRAFT_REQUIRE_SYRIAC").is_none());
        return;
    };
    for (load_matching, explicit, unknown_source) in [(false, false, false), (true, false, false), (false, true, false), (false, false, true)] {
        let dir = tempfile::tempdir().unwrap();
        let mut source = pdfcraft_cos::Document::open(std::sync::Arc::new(source_page())).unwrap();
        source
            .update_dict(pdfcraft_cos::ObjRef::new(5, 0), |d| {
                d.set(b"BaseFont".to_vec(), pdfcraft_cos::Object::name(if unknown_source { "UnlistedSyriacSource" } else { western.family }))
            })
            .unwrap();
        std::fs::write(dir.path().join("source.pdf"), pdfcraft_cos::write_full(&source, &Default::default()).unwrap()).unwrap();
        std::fs::write(dir.path().join("eastern.ttf"), eastern.bytes).unwrap();
        let mut auto = Automation::new().with_root(dir.path()).unwrap();
        let id = call(&mut auto, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
        let unrelated = call(&mut auto, "text_font_import", json!({"doc":id,"page":1,"path":"eastern.ttf"}))["resource"].as_str().unwrap().to_owned();
        if load_matching {
            std::fs::write(dir.path().join("western.ttf"), western.bytes).unwrap();
            call(&mut auto, "text_font_import", json!({"doc":id,"page":1,"path":"western.ttf"}));
        }
        let text = "\u{072b}\u{0720}\u{0721}\u{0710}";
        let mut args = json!({"doc":id,"page":1,"paragraph":1,"text":text,"width":450});
        if explicit {
            args["font_resource"] = unrelated.into();
        }
        call(&mut auto, "text_edit", args);
        assert_eq!(extracted(&mut auto, id), text);
        let expected = if explicit || unknown_source { eastern } else { western };
        let lines = auto.session().get(DocId(id)).unwrap().text_lines(0);
        assert!(!lines.is_empty());
        assert!(
            lines.iter().all(|line| embedding::matching_faces(&line.base_font).iter().any(|face| face.family == expected.family)),
            "expected {}, got {:?}",
            expected.name(),
            lines.iter().map(|line| &line.base_font).collect::<Vec<_>>()
        );
    }
}
