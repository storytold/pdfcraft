use pdfcraft_ffi::{Engine, FfiFieldKind, FfiPoint, FfiRect, FfiRenderRequest};

fn synthetic_form_pdf() -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let objs: Vec<String> = vec![
        // 1: Catalog
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        // 2: Pages
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        // 3: Page 1
        "<< /Type /Page /Parent 2 0 R /Annots [10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 16 0 R 17 0 R 21 0 R 22 0 R 23 0 R 24 0 R 25 0 R 26 0 R 27 0 R] >>".into(),
        // 4: AcroForm
        "<< /Fields [10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R 17 0 R 20 0 R 23 0 R 24 0 R 25 0 R 26 0 R 27 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R /ZaDb 6 0 R >> >> >>".into(),
        // 5: Font Helv
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        // 6: Font ZaDb
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>".into(),
        // 7: AP stream on
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        // 8: AP stream off
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        // 9: null
        "null".into(),
        // 10: name text field
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [50 700 250 720] /P 3 0 R /V (Ada) /DV (Ada) /MK << /BG [1 1 0.9] /BC [0 0 0] >> >>".into(),
        // 11: notes multiline text field
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (notes) /Ff 4096 /DA (/Helv 10 Tf 0 0 1 rg) /Rect [50 600 250 680] /P 3 0 R >>".into(),
        // 12: zip comb text field
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (zip) /Ff 16777216 /MaxLen 5 /Rect [50 560 150 580] /P 3 0 R >>".into(),
        // 13: pin password text field
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (pin) /Ff 8192 /Rect [50 530 150 550] /P 3 0 R >>".into(),
        // 14: id read-only text field
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (id) /Ff 1 /V (A-1) /Rect [50 500 150 520] /P 3 0 R >>".into(),
        // 15: parent address
        "<< /T (address) /Kids [16 0 R] >>".into(),
        // 16: city nested text field
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (city) /Parent 15 0 R /Q 1 /Rect [50 470 250 490] /P 3 0 R >>".into(),
        // 17: agree check box
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [300 700 315 715] /P 3 0 R /AP << /N << /Yes 7 0 R /Off 8 0 R >> >> >>".into(),
        // 18: null
        "null".into(),
        // 19: null
        "null".into(),
        // 20: size radio group
        "<< /FT /Btn /T (size) /Ff 49152 /V /Off /DV /S /Kids [21 0 R 22 0 R] >>".into(),
        // 21: radio S widget
        "<< /Type /Annot /Subtype /Widget /Parent 20 0 R /AS /Off /Rect [300 650 315 665] /P 3 0 R /AP << /N << /S 7 0 R /Off 8 0 R >> >> >>".into(),
        // 22: radio L widget
        "<< /Type /Annot /Subtype /Widget /Parent 20 0 R /AS /Off /Rect [330 650 345 665] /P 3 0 R /AP << /N << /L 7 0 R /Off 8 0 R >> >> >>".into(),
        // 23: country combo
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (country) /Ff 131072 /Opt [[(ca) (Canada)] [(fr) (France)]] /Rect [300 600 450 620] /P 3 0 R >>".into(),
        // 24: toppings list
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (toppings) /Ff 2097152 /Opt [(Cheese) (Ham) (Olives)] /Rect [300 500 450 580] /P 3 0 R >>".into(),
        // 25: go button
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (go) /Ff 65536 /Rect [300 450 380 470] /P 3 0 R >>".into(),
        // 26: bare check box without AP
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (bare) /Rect [400 700 415 715] /P 3 0 R /Foo (kept) >>".into(),
        // 27: sig signature field
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig) /Rect [50 400 250 440] /P 3 0 R >>".into(),
    ];

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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn test_acroform_field_listing_and_properties() {
    let engine = Engine::new();
    let pdf = synthetic_form_pdf();
    let doc_id = engine.open_bytes("form.pdf".into(), pdf, None).expect("open form");

    let fields = engine.form_fields(doc_id, None).expect("get form fields");
    assert!(!fields.is_empty());

    // Verify text field
    let name_field = fields.iter().find(|f| f.name == "name").expect("find name field");
    assert_eq!(name_field.kind, FfiFieldKind::Text);
    assert_eq!(name_field.value, vec!["Ada".to_string()]);
    assert_eq!(name_field.default_value, vec!["Ada".to_string()]);
    assert!(!name_field.read_only);

    // Verify multiline
    let notes_field = fields.iter().find(|f| f.name == "notes").expect("find notes field");
    assert!(notes_field.multiline);

    // Verify comb with max_len
    let zip_field = fields.iter().find(|f| f.name == "zip").expect("find zip field");
    assert_eq!(zip_field.max_len, Some(5));

    // Verify password
    let pin_field = fields.iter().find(|f| f.name == "pin").expect("find pin field");
    assert!(pin_field.password);

    // Verify read-only
    let id_field = fields.iter().find(|f| f.name == "id").expect("find id field");
    assert!(id_field.read_only);

    // Verify check box
    let agree_field = fields.iter().find(|f| f.name == "agree").expect("find agree field");
    assert_eq!(agree_field.kind, FfiFieldKind::CheckBox);
    assert_eq!(agree_field.on_state.as_deref(), Some("Yes"));

    // Verify radio widgets
    let radio_widgets: Vec<_> = fields.iter().filter(|f| f.name == "size").collect();
    assert_eq!(radio_widgets.len(), 2);
    assert_eq!(radio_widgets[0].kind, FfiFieldKind::Radio);
    assert!(radio_widgets.iter().any(|w| w.on_state.as_deref() == Some("S")));
    assert!(radio_widgets.iter().any(|w| w.on_state.as_deref() == Some("L")));

    // Verify choice combo
    let country_field = fields.iter().find(|f| f.name == "country").expect("find country field");
    assert_eq!(country_field.kind, FfiFieldKind::Choice);
    assert_eq!(country_field.options.len(), 2);
    assert_eq!(country_field.options[0].export_value, "ca");
    assert_eq!(country_field.options[0].display_text, "Canada");
    assert_eq!(country_field.options[1].export_value, "fr");
    assert_eq!(country_field.options[1].display_text, "France");

    // Verify signature field
    let sig_field = fields.iter().find(|f| f.name == "sig").expect("find sig field");
    assert_eq!(sig_field.kind, FfiFieldKind::Signature);
}

#[test]
fn test_acroform_filling_and_reset() {
    let engine = Engine::new();
    let pdf = synthetic_form_pdf();
    let doc_id = engine.open_bytes("form.pdf".into(), pdf, None).expect("open form");

    // 1. Fill text field
    engine.set_form_field_text(doc_id, "name".into(), "Charles Babbage".into()).expect("set text");
    let fields = engine.form_fields(doc_id, Some(0)).expect("get page fields");
    let name_field = fields.iter().find(|f| f.name == "name").expect("find name");
    assert_eq!(name_field.value, vec!["Charles Babbage".to_string()]);

    // 2. CheckBox toggle and set
    let toggled_on = engine.toggle_form_field_check(doc_id, "agree".into()).expect("toggle check");
    assert!(toggled_on);
    let agree_field = engine.form_fields(doc_id, Some(0)).expect("fields").into_iter().find(|f| f.name == "agree").expect("agree");
    assert_eq!(agree_field.value, vec!["Yes".to_string()]);

    engine.set_form_field_check(doc_id, "agree".into(), false).expect("uncheck");
    let agree_field2 = engine.form_fields(doc_id, Some(0)).expect("fields").into_iter().find(|f| f.name == "agree").expect("agree");
    assert!(agree_field2.value.is_empty());

    // 3. Radio selection
    engine.set_form_field_radio(doc_id, "size".into(), Some("L".into())).expect("set radio");
    let size_field = engine.form_fields(doc_id, Some(0)).expect("fields").into_iter().find(|f| f.name == "size").expect("size");
    assert_eq!(size_field.value, vec!["L".to_string()]);

    // 4. Choice dropdown selection
    engine.set_form_field_choice(doc_id, "country".into(), vec!["fr".into()]).expect("set choice");
    let country_field = engine.form_fields(doc_id, Some(0)).expect("fields").into_iter().find(|f| f.name == "country").expect("country");
    assert_eq!(country_field.value, vec!["fr".to_string()]);

    // 5. General setter
    engine.set_form_field_value(doc_id, "name".into(), "Ada Lovelace".into()).expect("set value");
    let name_after = engine.form_fields(doc_id, Some(0)).expect("fields").into_iter().find(|f| f.name == "name").expect("name");
    assert_eq!(name_after.value, vec!["Ada Lovelace".to_string()]);

    // 6. Reset form restores defaults
    engine.reset_form(doc_id, None).expect("reset form");
    let fields_reset = engine.form_fields(doc_id, Some(0)).expect("fields");
    let name_reset = fields_reset.iter().find(|f| f.name == "name").expect("name");
    assert_eq!(name_reset.value, vec!["Ada".to_string()]);
}

#[test]
fn test_signature_annotations_and_signing() {
    let engine = Engine::new();
    let pdf = synthetic_form_pdf();
    let doc_id = engine.open_bytes("form.pdf".into(), pdf, None).expect("open form");

    // 1. Add freehand drawn visual signature
    let strokes = vec![vec![FfiPoint { x: 100.0, y: 150.0 }, FfiPoint { x: 120.0, y: 180.0 }, FfiPoint { x: 150.0, y: 140.0 }]];
    engine.add_drawn_signature(doc_id, 0, strokes, Some("Ada".into())).expect("add drawn signature");

    // 2. Add typed visual signature
    let rect = FfiRect { x: 200.0, y: 150.0, width: 120.0, height: 40.0 };
    engine.add_typed_signature(doc_id, 0, "Ada Lovelace".into(), rect, Some("Ada".into())).expect("add typed signature");

    // 3. Sign signature field with drawn strokes (normalized 0..1)
    let norm_strokes = vec![vec![FfiPoint { x: 0.1, y: 0.5 }, FfiPoint { x: 0.5, y: 0.9 }, FfiPoint { x: 0.9, y: 0.2 }]];
    engine.sign_form_field_drawn(doc_id, "sig".into(), norm_strokes, Some("Ada".into())).expect("sign field drawn");

    // 4. Sign signature field with typed script name
    engine.sign_form_field_typed(doc_id, "sig".into(), "Ada Lovelace".into(), Some("Ada".into())).expect("sign field typed");

    // Verify tile render runs cleanly with the new signatures
    let req = FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 1, preview: false, reading_colors: None, hide_comments: false };
    let result = engine.render_tile(doc_id, req).expect("render tile with signatures");
    assert!(result.width > 0);
    assert!(result.height > 0);
    assert_eq!(result.pixels.len(), (result.width * result.height * 4) as usize);
}
