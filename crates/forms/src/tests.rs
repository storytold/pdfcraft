use std::sync::Arc;

use pdfcraft_cos::{Document, Object, SaveOptions, write_incremental};

use super::*;

/// One page with a form: text fields (plain, multiline, comb, password, read-only, nested),
/// a check box with appearances, one without, a radio group, a combo and a multi-select list,
/// and a push button.
fn fixture() -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),                                             // 1
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),                              // 2
        "<< /Type /Page /Parent 2 0 R /Annots [10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 16 0 R 17 0 R 21 0 R 22 0 R 23 0 R 24 0 R 25 0 R 26 0 R] >>".into(), // 3
        "<< /Fields [10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R 17 0 R 20 0 R 23 0 R 24 0 R 25 0 R 26 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R /ZaDb 6 0 R >> >> >>".into(), // 4
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),               // 5
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>".into(),                                      // 6
        "<< /Length 0 >>\nstream\n\nendstream".into(),                                                            // 7 (empty appearance)
        "<< /Length 0 >>\nstream\n\nendstream".into(),                                                            // 8
        "null".into(),                                                                                            // 9
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [50 700 250 720] /P 3 0 R /V (Ada) /DV (Ada) /MK << /BG [1 1 0.9] /BC [0 0 0] >> >>".into(), // 10
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (notes) /Ff 4096 /DA (/Helv 10 Tf 0 0 1 rg) /Rect [50 600 250 680] /P 3 0 R >>".into(), // 11
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (zip) /Ff 16777216 /MaxLen 5 /Rect [50 560 150 580] /P 3 0 R >>".into(), // 12
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (pin) /Ff 8192 /Rect [50 530 150 550] /P 3 0 R >>".into(), // 13
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (id) /Ff 1 /V (A-1) /Rect [50 500 150 520] /P 3 0 R >>".into(), // 14
        "<< /T (address) /Kids [16 0 R] >>".into(),                                                               // 15
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (city) /Parent 15 0 R /Q 1 /Rect [50 470 250 490] /P 3 0 R >>".into(), // 16
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [300 700 315 715] /P 3 0 R /AP << /N << /Yes 7 0 R /Off 8 0 R >> >> >>".into(), // 17
        "null".into(),                                                                                            // 18
        "null".into(),                                                                                            // 19
        "<< /FT /Btn /T (size) /Ff 49152 /V /Off /DV /S /Kids [21 0 R 22 0 R] >>".into(),                        // 20 radio, NoToggleToOff
        "<< /Type /Annot /Subtype /Widget /Parent 20 0 R /AS /Off /Rect [300 650 315 665] /P 3 0 R /AP << /N << /S 7 0 R /Off 8 0 R >> >> >>".into(), // 21
        "<< /Type /Annot /Subtype /Widget /Parent 20 0 R /AS /Off /Rect [330 650 345 665] /P 3 0 R /AP << /N << /L 7 0 R /Off 8 0 R >> >> >>".into(), // 22
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (country) /Ff 131072 /Opt [[(ca) (Canada)] [(fr) (France)]] /Rect [300 600 450 620] /P 3 0 R >>".into(), // 23
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (toppings) /Ff 2097152 /Opt [(Cheese) (Ham) (Olives)] /Rect [300 500 450 580] /P 3 0 R >>".into(), // 24
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (go) /Ff 65536 /Rect [300 450 380 470] /P 3 0 R >>".into(), // 25
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (bare) /Rect [400 700 415 715] /P 3 0 R /Foo (kept) >>".into(), // 26 check box without AP
    ];
    document(&objs)
}

/// A PDF of the given objects (numbered from 1).
fn document(objs: &[String]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
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
    Document::open(Arc::new(out)).expect("opens")
}

fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).expect("writes");
    hayro_syntax::Pdf::new(bytes.clone()).expect("hayro-syntax parses the output");
    Document::open(Arc::new(bytes)).expect("reopens")
}

fn legacy_two_call_enumeration(doc: &Document) -> (Vec<Field>, usize) {
    (enumerate(doc).0, enumerate(doc).1)
}

fn assert_combined_matches_legacy(doc: &Document, adopted: usize) {
    let (old_fields, old_adopted) = legacy_two_call_enumeration(doc);
    let combined = fields_with_adopted(doc);
    assert_eq!(combined.items, old_fields);
    assert_eq!(combined.adopted_page_fields, old_adopted);
    assert_eq!(combined.adopted_page_fields, adopted);
    assert_eq!(fields(doc), old_fields, "the compatibility wrapper keeps the old fields");
    assert_eq!(adopted_page_fields(doc), old_adopted, "the compatibility wrapper keeps the old count");
}

fn field<'a>(all: &'a [Field], name: &str) -> &'a Field {
    all.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no field {name}"))
}

fn ap(doc: &Document, w: &Widget) -> String {
    let wd = doc.get(w.obj).as_dict().cloned().unwrap();
    let n = wd.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").expect("normal appearance stream");
    let Object::Stream(s) = &*doc.get(n) else { panic!() };
    String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
}

#[test]
fn button_caption_preserves_other_widgets_and_shared_dictionaries() {
    for indirect_caption in [false, true] {
        let mut doc = one_page();
        let a = add_field(&mut doc, 0, [40.0, 600.0, 200.0, 630.0], &NewField::Button { caption: "Original".into() }, Some("first")).unwrap();
        let b = add_field(&mut doc, 0, [40.0, 500.0, 200.0, 530.0], &NewField::Button { caption: "Original".into() }, Some("second")).unwrap();
        let all = fields(&doc);
        let (first, second) = (field(&all, &a).obj, field(&all, &b).obj);
        let mut parent = Dict::new();
        parent.set(b"FT".to_vec(), Object::name("Btn"));
        parent.set(b"Ff".to_vec(), Object::Int(flags::PUSH_BUTTON as i64));
        parent.set(b"T".to_vec(), PdfString::text("clear_button"));
        parent.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(first), Object::Ref(second)]));
        let mut action = Dict::new();
        action.set(b"S".to_vec(), Object::name("ResetForm"));
        parent.set(b"A".to_vec(), Object::Dict(action.clone()));
        let parent = doc.add(parent);
        let mut mk = Dict::new();
        let ca = Object::String(PdfString::text("Original"));
        let ca = if indirect_caption { Object::Ref(doc.add(ca)) } else { ca };
        mk.set(b"CA".to_vec(), ca);
        mk.set(b"TP".to_vec(), Object::Int(0));
        mk.set(b"R".to_vec(), Object::Int(90));
        mk.set(b"VendorNote".to_vec(), PdfString::text("kept"));
        let mk = doc.add(mk);
        let mut shared_ap = doc.get(first).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().clone();
        let normal = shared_ap.get(b"N").unwrap().clone();
        shared_ap.set(b"R".to_vec(), normal.clone());
        shared_ap.set(b"D".to_vec(), normal);
        shared_ap.set(b"VendorAP".to_vec(), PdfString::text("kept"));
        let shared_ap = doc.add(shared_ap);
        for widget in [first, second] {
            doc.update_dict(widget, |d| {
                for key in [b"FT".as_slice(), b"Ff", b"T"] {
                    d.remove(key);
                }
                d.set(b"Parent".to_vec(), Object::Ref(parent));
                d.set(b"MK".to_vec(), Object::Ref(mk));
                d.set(b"AP".to_vec(), Object::Ref(shared_ap));
            })
            .unwrap();
        }
        let af = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"AcroForm").unwrap();
        doc.update_dict(af, |d| d.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(parent)]))).unwrap();
        let original = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let original = Document::open(Arc::new(original)).unwrap();
        // Full saves can renumber objects: reacquire references from the reopened model.
        let f = field(&fields(&original), "clear_button").clone();
        let first = f.widgets[0].obj;
        let mk = original.get(first).as_dict().unwrap().reference(b"MK").unwrap();
        for full in [false, true] {
            let mut doc = original.clone();
            let untouched = doc.get(first).clone();
            let shared_mk = doc.get(mk).clone();
            assert_eq!(field(&fields(&doc), "clear_button").widgets[1].caption.as_deref(), Some("Original"));
            set_props(&mut doc, "clear_button", &FieldProps { caption: Some((1, "Clear Form".into())), ..Default::default() }).unwrap();
            assert_eq!(doc.get(first), untouched, "the other widget and all its appearances are untouched");
            assert_eq!(doc.get(mk), shared_mk, "shared MK is not patched in place");
            let bytes = if full {
                pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap()
            } else {
                let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
                assert!(bytes.starts_with(original.bytes().as_slice()));
                assert!(bytes.len() > original.bytes().len());
                bytes
            };
            let doc = Document::open(Arc::new(bytes)).unwrap();
            let f = field(&fields(&doc), "clear_button").clone();
            assert_eq!(f.widgets.iter().map(|w| w.caption.as_deref()).collect::<Vec<_>>(), [Some("Original"), Some("Clear Form")]);
            assert_eq!(f.button, Some(af::ButtonAction::Reset { fields: Vec::new(), exclude: false }));
            assert_eq!(doc.get(f.obj).as_dict().unwrap().get(b"A"), Some(&Object::Dict(action.clone())));
            let w = doc.get(f.widgets[1].obj);
            let mk = w.as_dict().unwrap().get(b"MK").unwrap().as_dict().unwrap();
            assert_eq!(mk.get(b"VendorNote").unwrap().as_string().unwrap().to_text(), "kept");
            assert_eq!(mk.get(b"R").unwrap().as_int(), Some(90));
            assert_eq!(mk.get(b"TP").unwrap().as_int(), Some(0));
            let apd = w.as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap();
            assert!(apd.contains(b"VendorAP"));
            assert!(!apd.contains(b"R") && !apd.contains(b"D"), "stale hover/pressed text must not survive");
            assert!(ap(&doc, &f.widgets[1]).contains("(Clear Form) Tj"));
        }
    }
}

#[test]
fn button_caption_rejects_wrong_type_and_index_before_mutation() {
    let mut doc = fixture();
    for (name, wi) in [("name", 0), ("agree", 0), ("go", usize::MAX)] {
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let props = FieldProps { caption: Some((wi, "Clear Form".into())), name: Some("renamed".into()), ..Default::default() };
        assert!(set_props(&mut doc, name, &props).is_err());
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before);
    }
    set_props(&mut doc, "go", &FieldProps { caption: Some((0, "提交".into())), ..Default::default() }).unwrap();
    assert_eq!(field(&fields(&doc), "go").widgets[0].caption.as_deref(), Some("提交"));
    assert_eq!(undrawn_fields(&doc), [("go".to_string(), '提')]);
    set_props(&mut doc, "go", &FieldProps { caption: Some((0, String::new())), ..Default::default() }).unwrap();
    assert_eq!(field(&fields(&doc), "go").widgets[0].caption.as_deref(), Some(""));
    assert!(undrawn_fields(&doc).is_empty());
    assert!(!ap(&doc, &field(&fields(&doc), "go").widgets[0]).contains(" Tj"));
}

/// Read the drawn strings together with the appearance's actual font after saving and reopening.
fn appearance_text(doc: &Document, w: &Widget, font_name: &[u8]) -> (Object, Vec<Vec<u8>>) {
    let widget = doc.get(w.obj);
    let normal = widget.as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let appearance = doc.get(normal);
    let Object::Stream(stream) = appearance.as_ref() else { panic!("appearance stream") };
    let resources = doc.resolve(stream.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
    let font = doc.resolve(fonts.as_dict().unwrap().get(font_name).unwrap());
    let shown = pdfcraft_content::parse(&stream.decoded().unwrap())
        .ops
        .into_iter()
        .filter(|op| op.is("Tj"))
        .map(|op| op.operands[0].as_string().unwrap().bytes.clone())
        .collect();
    (font.as_ref().clone(), shown)
}

#[test]
fn the_field_tree_is_read_with_inheritance() {
    let doc = fixture();
    assert_combined_matches_legacy(&doc, 0);
    let all = fields(&doc);
    let names: Vec<&str> = all.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["name", "notes", "zip", "pin", "id", "address.city", "agree", "size", "country", "toppings", "go", "bare"]);
    let name = field(&all, "name");
    assert_eq!((name.kind, name.value.as_slice(), name.da.as_str()), (FieldKind::Text, &["Ada".to_string()][..], "/Helv 0 Tf 0 g"));
    assert_eq!(name.widgets[0].page, Some(0));
    assert_eq!(field(&all, "zip").max_len, Some(5));
    assert!(field(&all, "id").read_only());
    let size = field(&all, "size");
    assert_eq!(size.kind, FieldKind::Radio);
    assert_eq!(size.widgets.iter().map(|w| w.on_state.clone().unwrap()).collect::<Vec<_>>(), ["S", "L"]);
    assert!(size.value.is_empty(), "/Off is no value");
    assert_eq!(size.default, ["S"]);
    assert_eq!(field(&all, "country").options, [("ca".into(), "Canada".into()), ("fr".into(), "France".into())]);
    assert_eq!(field(&all, "toppings").kind, FieldKind::List);
    assert_eq!(field(&all, "go").kind, FieldKind::PushButton);
    assert_eq!(field(&all, "address.city").quadding, 1);
}

#[test]
fn field_flags_reject_values_outside_the_unsigned_32_bit_domain() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [] >>".into(),
        "<< /Fields [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >>".into(),
        "<< /FT /Btn /T (valid) /Ff 65536 >>".into(),
        "<< /FT /Btn /T (negative) /Ff -1 >>".into(),
        "<< /FT /Btn /T (overflow) /Ff 4295032832 >>".into(),
        "<< /FT /Btn /T (wrong_type) /Ff (65536) >>".into(),
        "<< /FT /Btn /T (missing) >>".into(),
    ];
    let all = fields(&document(&objs));
    assert_eq!(field(&all, "valid").kind, FieldKind::PushButton);
    assert_eq!(field(&all, "valid").flags, flags::PUSH_BUTTON);
    for name in ["negative", "overflow", "wrong_type", "missing"] {
        assert_eq!(field(&all, name).kind, FieldKind::CheckBox, "{name}");
        assert_eq!(field(&all, name).flags, 0, "{name}");
    }
}

/// Some writers leave every field out of `/Fields` and put the widgets only in the page
/// annotations: the tree walk alone finds nothing, and Acrobat and the browsers fill them anyway.
/// The pages' widgets are adopted as fields, after the listed tree fields.
#[test]
fn fields_listed_only_on_the_pages_are_adopted() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),                 // 1
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 10 0 R 11 0 R 13 0 R 14 0 R 15 0 R] >>".into(), // 3
        "<< /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >>".into(), // 4
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (listed) /V (keep) /Rect [50 700 250 720] /P 3 0 R >>".into(), // 5
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(), // 6
        "null".into(),                                                              // 7
        "null".into(),                                                              // 8
        "null".into(),                                                              // 9
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (alpha) /V (one) /Rect [50 600 250 620] /P 3 0 R >>".into(), // 10
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (beta) /DA (/Helv 9 Tf 0 g) /Rect [50 560 250 580] /P 3 0 R >>".into(), // 11
        "<< /FT /Tx /T (pair) /Kids [13 0 R 14 0 R] >>".into(),                     // 12 unlisted parent field
        "<< /Type /Annot /Subtype /Widget /Parent 12 0 R /Rect [50 500 150 520] /P 3 0 R >>".into(), // 13 its widget
        "<< /Type /Annot /Subtype /Widget /Parent 12 0 R /Rect [50 460 150 480] /P 3 0 R >>".into(), // 14 its other widget
        "<< /Type /Annot /Subtype /Widget /Rect [400 700 415 715] /P 3 0 R >>".into(), // 15 no /FT, /T or /Parent: not a field
    ];
    let mut doc = document(&objs);
    assert_combined_matches_legacy(&doc, 3);
    let all = fields(&doc);
    let names: Vec<&str> = all.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["listed", "alpha", "beta", "pair"]);
    assert_eq!(adopted_page_fields(&doc), 3);
    let alpha = field(&all, "alpha");
    assert_eq!((alpha.kind, alpha.value.as_slice(), alpha.widgets[0].page), (FieldKind::Text, &["one".to_string()][..], Some(0)));
    assert_eq!(field(&all, "beta").da, "/Helv 9 Tf 0 g");
    assert_eq!(field(&all, "pair").widgets.len(), 2, "one field carries both widgets");
    // They are filled and keep the value across a save.
    set_value(&mut doc, "beta", &FieldValue::Text("typed".into())).unwrap();
    assert!(ap(&doc, &field(&fields(&doc), "beta").widgets[0]).contains("typed"));
    let doc = reopen(&doc);
    assert_eq!(field(&fields(&doc), "beta").value, ["typed"]);
    // Without a /Fields list at all, everything on the pages is adopted.
    let mut objs = objs;
    objs[3] = "<< /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >>".into();
    let doc = document(&objs);
    assert_combined_matches_legacy(&doc, 4);
    let all = fields(&doc);
    let names: Vec<&str> = all.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["listed", "alpha", "beta", "pair"]);
    assert_eq!(adopted_page_fields(&doc), 4);
}

/// pdf-lib and other writers give radio groups and check boxes an `/Opt` array and name the on
/// states by position (`/0`, `/1`): the export values select them, as in Acrobat.
#[test]
fn opt_export_values_select_button_states() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),                                                // 1
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),                                 // 2
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 7 0 R 8 0 R] >>".into(),                                       // 3
        "<< /Fields [5 0 R 8 0 R] /DA (/Helv 0 Tf 0 g) >>".into(),                                                 // 4
        "<< /FT /Btn /T (ship) /Ff 49152 /Opt [(Post) (Pick-up)] /V /Off /Kids [6 0 R 7 0 R] >>".into(),            // 5 radio
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /AS /Off /Rect [100 700 115 715] /P 3 0 R /AP << /N << /0 9 0 R /Off 10 0 R >> >> >>".into(), // 6
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /AS /Off /Rect [200 700 215 715] /P 3 0 R /AP << /N << /1 9 0 R /Off 10 0 R >> >> >>".into(), // 7
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (terms) /Opt [(Accepted)] /V /Off /AS /Off /Rect [100 650 115 665] /P 3 0 R /AP << /N << /0 9 0 R /Off 10 0 R >> >> >>".into(), // 8 check box
        "<< /Length 0 >>\nstream\n\nendstream".into(),                                                              // 9
        "<< /Length 0 >>\nstream\n\nendstream".into(),                                                              // 10
    ];
    let mut doc = document(&objs);
    let all = fields(&doc);
    let ship = field(&all, "ship");
    assert_eq!((0..2).map(|i| ship.export_of(i)).collect::<Vec<_>>(), [Some("Post"), Some("Pick-up")]);
    assert_eq!((ship.state_for("Pick-up"), ship.state_for("1"), ship.state_for("Courier")), (Some("1"), Some("1"), None));
    set_value(&mut doc, "ship", &FieldValue::Radio(Some("Pick-up".into()))).unwrap();
    set_value(&mut doc, "terms", &FieldValue::Text("Accepted".into())).unwrap();
    let mut doc = reopen(&doc);
    let all = fields(&doc);
    let ship = field(&all, "ship");
    assert_eq!(ship.value, ["1"]);
    assert_eq!(ship.export_for_state("1"), "Pick-up");
    assert_eq!(ship.widgets.iter().map(|w| w.state.as_deref()).collect::<Vec<_>>(), [Some("Off"), Some("1")]);
    assert_eq!(field(&all, "terms").value, ["0"]);
    let err = set_value(&mut doc, "ship", &FieldValue::Radio(Some("Courier".into()))).unwrap_err();
    assert!(err.to_string().contains("options: Post, Pick-up"), "{err}");
}

/// Issue #1323: a text field without a format keeps a whitespace-only value as typed; only an
/// empty value clears it.
#[test]
fn whitespace_only_text_is_kept_in_a_field_without_a_format() {
    for value in [" ", "\t", "\u{a0}"] {
        let mut doc = fixture();
        set_value(&mut doc, "name", &FieldValue::Text(value.into())).unwrap();
        let doc = reopen(&doc);
        assert_eq!(field(&fields(&doc), "name").value, [value], "{value:?}");
    }
    let mut doc = fixture();
    set_value(&mut doc, "name", &FieldValue::Text(String::new())).unwrap();
    assert!(field(&fields(&reopen(&doc)), "name").value.is_empty());
}

#[test]
fn text_fields_get_new_appearances() {
    let mut doc = fixture();
    set_value(&mut doc, "name", &FieldValue::Text("Grace (Hopper)".into())).unwrap();
    set_value(&mut doc, "notes", &FieldValue::Text("A long note that has to wrap over several lines of the box".into())).unwrap();
    set_value(&mut doc, "zip", &FieldValue::Text("12345".into())).unwrap();
    set_value(&mut doc, "pin", &FieldValue::Text("1234".into())).unwrap();
    set_value(&mut doc, "address.city", &FieldValue::Text("Zürich".into())).unwrap();
    let doc = reopen(&doc);
    let all = fields(&doc);
    assert_eq!(field(&all, "name").value, ["Grace (Hopper)"]);
    let name_ap = ap(&doc, &field(&all, "name").widgets[0]);
    assert!(name_ap.contains("/Tx BMC") && name_ap.contains("EMC"), "{name_ap}");
    assert!(name_ap.contains("(Grace \\(Hopper\\)) Tj"), "{name_ap}");
    assert!(name_ap.contains("1 1 0.9 rg"), "background from /MK: {name_ap}");
    let notes = ap(&doc, &field(&all, "notes").widgets[0]);
    assert!(notes.matches(" Tj").count() >= 2, "wrapped: {notes}");
    assert!(notes.contains("/Helv 10 Tf") && notes.contains("0 0 1 rg"));
    let zip = ap(&doc, &field(&all, "zip").widgets[0]);
    assert_eq!(zip.matches(" Tj").count(), 5, "one cell per character: {zip}");
    let pin = ap(&doc, &field(&all, "pin").widgets[0]);
    assert!(pin.contains("(****) Tj") && !pin.contains("1234"), "{pin}");
    // WinAnsi bytes for non-ASCII text.
    let w = &field(&all, "address.city").widgets[0];
    let n = doc.get(w.obj).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let Object::Stream(s) = &*doc.get(n) else { panic!() };
    let raw = s.decoded().unwrap();
    assert!(raw.windows(8).any(|x| x == b"(Z\xfcrich)"));
}

#[test]
fn existing_field_font_encoding_keeps_italian_punctuation_visible() {
    // ReportLab-style Helvetica: an indirect encoding dictionary with PDFDoc-like Differences.
    // WinAnsi 0x92/0x97 would show trademark/Scaron in this font, not quoteright/emdash.
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>".into(),
        "<< /Fields [6 0 R] /DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding 7 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (localita) /Rect [50 700 450 730] /P 3 0 R >>".into(),
        "<< /Type /Encoding /Differences [39 /quotesingle 96 /grave 132 /emdash 144 /quoteright 146 /trademark 151 /Scaron 224 /agrave] >>".into(),
    ];
    let mut doc = document(&objs);
    let original_font = doc.get(ObjRef::new(5, 0)).as_ref().clone();
    let value = "Città Sant’Angelo — qualità";
    set_value(&mut doc, "localita", &FieldValue::Text(value.into())).unwrap();
    let doc = reopen(&doc);
    let all = fields(&doc);
    let f = field(&all, "localita");
    assert_eq!(f.value, [value], "the stored value and the drawn text must agree");
    let (font, lines) = appearance_text(&doc, &f.widgets[0], b"Helv");
    assert_eq!(font, original_font, "keep the existing field's font and encoding");
    let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, font.as_dict().unwrap());
    let shown = lines.concat();
    assert_eq!(metrics.decode(&shown), value, "decode the appearance with its actual font");
    assert_eq!(shown, b"Citt\xe0 Sant\x90Angelo \x84 qualit\xe0");
}

#[test]
fn unavailable_glyph_uses_an_appearance_font_without_changing_the_source_font() {
    // A subset font with only A cannot draw the newly entered accented text. Replacing its
    // encoding would corrupt every existing use of that font, so only the appearance falls back.
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>".into(),
        "<< /Fields [6 0 R] /DA (/Original 12 Tf 0 g) /DR << /Font << /Original 5 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Custom /FirstChar 65 /LastChar 65 /Widths [600] /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (localita) /Rect [50 700 450 730] /P 3 0 R >>".into(),
    ];
    let mut doc = document(&objs);
    let original_font = doc.get(ObjRef::new(5, 0)).as_ref().clone();
    let value = "Città Sant’Angelo — qualità";
    set_value(&mut doc, "localita", &FieldValue::Text(value.into())).unwrap();
    let doc = reopen(&doc);
    assert_eq!(doc.get(ObjRef::new(5, 0)).as_ref(), &original_font);
    let all = fields(&doc);
    let f = field(&all, "localita");
    assert_eq!(f.value, [value]);
    let (font, lines) = appearance_text(&doc, &f.widgets[0], b"Helv");
    let font = font.as_dict().unwrap();
    assert_eq!(font.name(b"BaseFont"), Some(&b"Helvetica"[..]));
    assert_eq!(font.name(b"Encoding"), Some(&b"WinAnsiEncoding"[..]));
    let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, font);
    let shown = lines.concat();
    assert_eq!(metrics.decode(&shown), value);
    assert_eq!(shown, b"Citt\xe0 Sant\x92Angelo \x97 qualit\xe0");
}

#[test]
fn multiline_wrapping_keeps_encodable_text_without_requiring_ascii_space() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>".into(),
        "<< /Fields [6 0 R] /DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding 7 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (localita) /Ff 4096 /Rect [50 500 130 550] /P 3 0 R >>".into(),
        "<< /Type /Encoding /Differences [32 /.notdef 144 /quoteright 160 /nbspace 224 /agrave] >>".into(),
    ];
    let mut doc = document(&objs);
    let original_font = doc.get(ObjRef::new(5, 0)).as_ref().clone();
    // NBSP remains part of the word; wrapping splits it by character without adding spaces.
    let value = "Città\u{a0}Sant’Angelo\nqualità";
    set_value(&mut doc, "localita", &FieldValue::Text(value.into())).unwrap();
    let doc = reopen(&doc);
    let all = fields(&doc);
    let f = field(&all, "localita");
    assert_eq!(f.value, [value]);
    let (font, lines) = appearance_text(&doc, &f.widgets[0], b"Helv");
    assert_eq!(font, original_font);
    assert!(lines.len() >= 3 && lines.iter().all(|line| !line.is_empty()));
    let metrics = pdfcraft_fonts::pdf::Metrics::from_dict(&doc, font.as_dict().unwrap());
    assert_eq!(metrics.decode(&lines.concat()), value.replace('\n', ""));
}

#[test]
fn japanese_choices_use_the_unicode_cid_font() {
    // A choice field whose /DA names a non-embedded CID font with a predefined Unicode CMap
    // (UniJIS-UTF16-H). Selecting 令 must draw it in that font, not "?" in Helvetica.
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [8 0 R 9 0 R 10 0 R] >>".into(),
        "<< /Fields [8 0 R 9 0 R 10 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R /HeiseiMin-W3 6 0 R /Emb 7 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Font /Subtype /Type0 /BaseFont /HeiseiMin-W3 /Encoding /UniJIS-UTF16-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType0 /BaseFont /HeiseiMin-W3 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 6 >> >>] >>".into(),
        "<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+Subset /Encoding /Identity-H /DescendantFonts [] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (era) /Ff 131072 /DA (/HeiseiMin-W3 10 Tf 0 g) /Q 1 /Opt [<FEFF3000> <FEFF660E> <FEFF4EE4>] /V <FEFF3000> /Rect [50 700 80 715] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (address) /Ff 4096 /DA (/HeiseiMin-W3 10 Tf 0 g) /Rect [50 600 100 680] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (subset) /DA (/Emb 10 Tf 0 g) /Rect [50 500 100 520] /P 3 0 R >>".into(),
    ];
    let mut doc = document(&objs);
    assert_eq!(field(&fields(&doc), "era").options.get(2), Some(&("令".to_string(), "令".to_string())));
    set_value(&mut doc, "era", &FieldValue::Text("令".into())).unwrap();
    set_value(&mut doc, "address", &FieldValue::Text("日本語のテキスト入力欄です".into())).unwrap();
    // An Identity-H subset can't be addressed by Unicode: Helvetica stands in, and can't draw 令.
    // The value is kept, drawn as `??`, marked, and other viewers are asked to redraw the form.
    set_value(&mut doc, "subset", &FieldValue::Text("令和".into())).unwrap();
    let doc = reopen(&doc);
    let all = fields(&doc);
    assert_eq!(field(&all, "era").value, ["令"]);
    let raw = |name: &str| {
        let w = &field(&all, name).widgets[0];
        let n = doc.get(w.obj).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
        let Object::Stream(s) = &*doc.get(n) else { panic!() };
        let fonts = doc.resolve(s.dict.get(b"Resources").unwrap()).as_dict().unwrap().get(b"Font").map(|f| doc.resolve(f)).unwrap();
        let fonts: Vec<String> = fonts.as_dict().unwrap().iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect();
        (s.decoded().unwrap(), fonts)
    };
    let utf16 = |t: &str| t.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>();
    let (era, era_fonts) = raw("era");
    assert!(era.windows(4).any(|x| x == b"(N\xe4)"), "令 as UTF-16BE: {}", String::from_utf8_lossy(&era));
    assert!(String::from_utf8_lossy(&era).contains("/HeiseiMin-W3 10 Tf"));
    assert!(!era.windows(3).any(|x| x == b"(?)"), "no WinAnsi fallback");
    assert_eq!(era_fonts, ["HeiseiMin-W3"]);
    // Multiline Japanese wraps by character at about one em each (50 pt wide, 10 pt type).
    let (addr, _) = raw("address");
    let addr_text = String::from_utf8_lossy(&addr);
    assert!(addr_text.matches(" Tj").count() >= 4, "{addr_text}");
    assert!(addr.windows(4).any(|x| x == utf16("日本").as_slice()));
    let (sub, sub_fonts) = raw("subset");
    assert!(String::from_utf8_lossy(&sub).contains("/Helv "), "{}", String::from_utf8_lossy(&sub));
    assert!(sub.windows(4).any(|x| x == b"(??)"), "{}", String::from_utf8_lossy(&sub));
    assert_eq!(sub_fonts, ["Helv"]);
    assert_eq!(field(&all, "subset").value, ["令和"]);
    // Only that widget is marked: the CID fonts drew theirs.
    assert_eq!(undrawn_fields(&doc), [("subset".to_string(), '令')]);
    assert!(need_appearances(&doc));
}

/// Text a field's font can't hold is kept, as Acrobat keeps it: the value is stored as typed, the
/// appearance draws `?` for those characters, the widget is marked with the first one, and the
/// form asks other viewers to redraw it (`/NeedAppearances`). Drawable text clears the mark.
#[test]
fn text_a_field_font_cannot_hold_is_kept_marked_and_redrawn_elsewhere() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R 8 0 R 9 0 R] >>".into(),
        "<< /Fields [7 0 R 8 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R /CN 6 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> >>] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /DA (/Helv 10 Tf 0 g) /V (Ada) /Rect [50 700 200 715] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (cn) /DA (/CN 10 Tf 0 g) /Rect [50 600 200 615] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (city) /Ff 131072 /DA (/Helv 10 Tf 0 g) /Opt [(Zurich) <FEFF5317004E4EAC>] /Rect [50 500 200 515] /P 3 0 R >>".into(),
    ];
    let mut doc = document(&objs);
    let shown = |doc: &Document, name: &str| {
        let all = fields(doc);
        let w = &field(&all, name).widgets[0];
        let n = doc.get(w.obj).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
        let Object::Stream(s) = &*doc.get(n) else { panic!() };
        let mark = doc.get(w.obj).as_dict().and_then(pdfcraft_fonts::undrawn_mark);
        (String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(), mark)
    };
    // A WinAnsi field: the Chinese value is kept and marked; Helvetica draws `?` for it.
    assert!(!need_appearances(&doc));
    set_value(&mut doc, "name", &FieldValue::Text("签名 Ada".into())).unwrap();
    assert_eq!(field(&fields(&doc), "name").value, ["签名 Ada"]);
    let (ap, mark) = shown(&doc, "name");
    assert!(ap.contains("(?? Ada) Tj"), "{ap}");
    assert_eq!(mark, Some('签'));
    assert!(need_appearances(&doc), "other viewers are asked to redraw the form");
    // A choice whose label Helvetica can't draw, the same way.
    set_value(&mut doc, "city", &FieldValue::Text("北N京".into())).unwrap();
    assert_eq!(shown(&doc, "city").1, Some('北'));
    // A UCS-2 CMap holds 签, so nothing is marked, but not a character beyond U+FFFF.
    set_value(&mut doc, "cn", &FieldValue::Text("签名".into())).unwrap();
    assert_eq!(shown(&doc, "cn").1, None);
    set_value(&mut doc, "cn", &FieldValue::Text("𠀋".into())).unwrap();
    assert_eq!(field(&fields(&doc), "cn").value, ["𠀋"]);
    assert_eq!(shown(&doc, "cn").1, Some('𠀋'));
    let mut undrawn = undrawn_fields(&doc);
    undrawn.sort();
    assert_eq!(undrawn, [("city".to_string(), '北'), ("cn".to_string(), '𠀋'), ("name".to_string(), '签')]);
    // Drawable text clears the mark; a line break in a single-line value is a space, not `?`.
    set_value(&mut doc, "name", &FieldValue::Text("Ada\nLovelace".into())).unwrap();
    let (ap, mark) = shown(&doc, "name");
    assert!(ap.contains("(Ada Lovelace) Tj") && !ap.contains('?'), "{ap}");
    assert_eq!(mark, None);
    // Kept through a save.
    let doc = reopen(&doc);
    assert_eq!(field(&fields(&doc), "city").value, ["北N京"]);
    assert!(need_appearances(&doc));
}

/// A document that declares PDF/A can't ask other viewers to redraw its fields (PDF/A forbids
/// `/NeedAppearances`): the value is still kept and its widget marked, and the flag stays off.
#[test]
fn a_pdfa_form_keeps_the_value_without_asking_viewers_to_redraw() {
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="2" pdfaid:conformance="B"/></rdf:RDF></x:xmpmeta>"#;
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R /Metadata 7 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>".into(),
        "<< /Fields [6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /DA (/Helv 10 Tf 0 g) /Rect [50 700 200 715] /P 3 0 R >>".into(),
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len() + 1),
    ];
    let mut doc = document(&objs);
    assert!(declares_pdfa(&doc));
    set_value(&mut doc, "name", &FieldValue::Text("张三".into())).unwrap();
    assert_eq!(field(&fields(&doc), "name").value, ["张三"]);
    assert_eq!(undrawn_fields(&doc), [("name".to_string(), '张')]);
    assert!(!need_appearances(&doc), "PDF/A forbids /NeedAppearances");
}

/// A button caption Helvetica can't draw is kept and marked like a value; redrawing a value another
/// program stored (a dynamic form's layout, a property change) draws and marks it rather than
/// failing, and an edit to an unrelated field never fails because of it.
#[test]
fn captions_and_redrawn_values_are_kept_and_marked() {
    let mut doc = fixture();
    let go = add_field(&mut doc, 0, [50.0, 50.0, 150.0, 80.0], &NewField::Button { caption: "提交".into() }, Some("go2")).unwrap();
    assert_eq!(undrawn_fields(&doc), [(go.clone(), '提')]);
    assert!(need_appearances(&doc));
    // A value stored by another program that Helvetica can't draw.
    let name = field(&fields(&doc), "name").clone();
    doc.update_dict(name.obj, |d| d.set(b"V".to_vec(), PdfString::text("张三"))).unwrap();
    redraw_field(&mut doc, "name").unwrap();
    let mut undrawn = undrawn_fields(&doc);
    undrawn.sort();
    assert_eq!(undrawn, [(go, '提'), ("name".to_string(), '张')]);
    set_value(&mut doc, "notes", &FieldValue::Text("Unrelated".into())).unwrap();
    // Drawable again: the mark goes.
    doc.update_dict(name.obj, |d| d.set(b"V".to_vec(), PdfString::text("Ada"))).unwrap();
    redraw_field(&mut doc, "name").unwrap();
    assert!(!undrawn_fields(&doc).iter().any(|(n, _)| n == "name"));
}

#[test]
fn check_boxes_and_radios_switch_states() {
    let mut doc = fixture();
    set_value(&mut doc, "agree", &FieldValue::Check(true)).unwrap();
    set_value(&mut doc, "size", &FieldValue::Radio(Some("L".into()))).unwrap();
    set_value(&mut doc, "bare", &FieldValue::Text("yes".into())).unwrap();
    let doc2 = reopen(&doc);
    let all = fields(&doc2);
    assert_eq!(field(&all, "agree").value, ["Yes"]);
    assert_eq!(field(&all, "agree").widgets[0].state.as_deref(), Some("Yes"));
    let size = field(&all, "size");
    assert_eq!(size.value, ["L"]);
    assert_eq!(size.widgets.iter().map(|w| w.state.clone().unwrap()).collect::<Vec<_>>(), ["Off", "L"]);
    // The bare check box got PdfCraft's own appearances and kept its other keys.
    let bare = field(&all, "bare");
    assert_eq!(bare.value, ["Yes"]);
    assert_eq!(bare.widgets[0].on_state.as_deref(), Some("Yes"));
    assert!(doc2.get(bare.widgets[0].obj).as_dict().unwrap().contains(b"Foo"));
    let mut doc = doc2;
    // Radio validation and NoToggleToOff.
    assert!(matches!(set_value(&mut doc, "size", &FieldValue::Radio(Some("XL".into()))), Err(FormError::Invalid(_))));
    assert!(matches!(set_value(&mut doc, "size", &FieldValue::Radio(None)), Err(FormError::Invalid(_))));
    set_value(&mut doc, "agree", &FieldValue::Check(false)).unwrap();
    assert!(field(&fields(&doc), "agree").value.is_empty());
}

#[test]
fn check_boxes_keep_non_utf8_state_names() {
    // Japanese forms often name a check box's on state 「はい」 in Shift-JIS: ticking it has to
    // write those exact bytes to /AS and /V, or no appearance matches and no mark shows.
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".into(),
        "<< /Fields [5 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /AS /Off /Rect [50 700 56 706] /P 3 0 R /AP << /N << /Off 6 0 R /#82#CD#82#A2 7 0 R >> >> >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
    ];
    let sjis_hai: &[u8] = b"\x82\xcd\x82\xa2";
    let mut doc = document(&objs);
    let all = fields(&doc);
    assert_eq!(field(&all, "agree").widgets[0].on_state.as_deref(), Some("#82#CD#82#A2"));
    set_value(&mut doc, "agree", &FieldValue::Check(true)).unwrap();
    let doc = reopen(&doc);
    let f = field(&fields(&doc), "agree").clone();
    let wd = doc.get(f.widgets[0].obj).as_dict().cloned().unwrap();
    assert_eq!(wd.name(b"AS"), Some(sjis_hai));
    assert_eq!(doc.get(f.obj).as_dict().unwrap().name(b"V"), Some(sjis_hai));
    assert_eq!(f.value, ["#82#CD#82#A2"]);
    let mut doc = doc;
    set_value(&mut doc, "agree", &FieldValue::Check(false)).unwrap();
    assert_eq!(doc.get(f.widgets[0].obj).as_dict().unwrap().name(b"AS"), Some(&b"Off"[..]));
}

#[test]
fn name_text_round_trips() {
    for bytes in [&b"Yes"[..], b"\x82\xcd\x82\xa2", "はい".as_bytes(), b"A#1", b"a b", b"", b"\xff#\x00"] {
        assert_eq!(name_bytes(&name_text(bytes)), bytes, "{bytes:?}");
    }
    assert_eq!(name_text("はい".as_bytes()), "はい");
    assert_eq!(name_text(b"A#1"), "A#231");
    // Malformed escapes stay as they are.
    assert_eq!(name_bytes("#G1#4"), b"#G1#4");
}

#[test]
fn choices_accept_exports_or_display_text() {
    let mut doc = fixture();
    set_value(&mut doc, "country", &FieldValue::Text("France".into())).unwrap();
    set_value(&mut doc, "toppings", &FieldValue::Choice(vec!["Ham".into(), "Olives".into()])).unwrap();
    let doc = reopen(&doc);
    let all = fields(&doc);
    let country = field(&all, "country");
    assert_eq!(country.value, ["fr"]);
    assert_eq!(country.display_value(), "France");
    assert!(ap(&doc, &country.widgets[0]).contains("(France) Tj"));
    let toppings = field(&all, "toppings");
    assert_eq!(toppings.value, ["Ham", "Olives"]);
    let list = ap(&doc, &toppings.widgets[0]);
    assert_eq!(list.matches("0.6 0.75 0.86 rg").count(), 2, "two highlighted rows: {list}");
    assert_eq!(doc.get(toppings.obj).as_dict().unwrap().get(b"I").unwrap().as_array().unwrap().len(), 2);
    let mut doc = doc;
    assert!(matches!(set_value(&mut doc, "country", &FieldValue::Text("Spain".into())), Err(FormError::Invalid(_))));
    assert!(matches!(set_value(&mut doc, "country", &FieldValue::Choice(vec!["ca".into(), "fr".into()])), Err(FormError::Invalid(_))));
}

#[test]
fn list_box_selection_scrolls_to_show_selected_options() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>".into(),
        "<< /Fields [6 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (days) /Ff 2097152 /Opt [(Mon) (Tue) (Wed) (Thu) (Fri)] /Rect [50 700 200 746] /P 3 0 R >>"
            .into(),
    ];
    let mut doc = document(&objs);
    // Setting value to Tue and Thu (indices 1 and 3) in a 46 pt high box (fits 3 rows).
    // It should scroll /TI to 1 so Tue (1), Wed (2), Thu (3) are drawn and highlighted.
    set_value(&mut doc, "days", &FieldValue::Choice(vec!["Tue".into(), "Thu".into()])).unwrap();
    let doc = reopen(&doc);
    let all = fields(&doc);
    let days = field(&all, "days");
    assert_eq!(days.value, ["Tue", "Thu"]);
    let ti = doc.get(days.obj).as_dict().unwrap().get(b"TI").and_then(|o| o.as_int());
    assert_eq!(ti, Some(1), "TI scrolled to 1 so both selected options are visible");
    let appearance = ap(&doc, &days.widgets[0]);
    assert!(appearance.contains("(Tue) Tj"), "Tue is drawn: {appearance}");
    assert!(appearance.contains("(Thu) Tj"), "Thu is drawn: {appearance}");
    assert_eq!(appearance.matches("0.6 0.75 0.86 rg").count(), 2, "both Tue and Thu highlighted");
}

#[test]
fn multiselect_choice_indices_are_sorted_without_reordering_values() {
    let mut forward_appearance = None;
    for values in [["Ham", "Olives"], ["Olives", "Ham"]] {
        let mut doc = fixture();
        let chosen: Vec<String> = values.iter().map(|value| (*value).into()).collect();
        set_value(&mut doc, "toppings", &FieldValue::Choice(chosen.clone())).unwrap();
        let doc = reopen(&doc);
        let all = fields(&doc);
        let list = field(&all, "toppings");
        assert_eq!(list.value, chosen, "saving preserves the caller's /V order");
        let object = doc.get(list.obj);
        let indices = object.as_dict().unwrap().get(b"I").unwrap().as_array().unwrap();
        assert_eq!(indices.iter().map(|index| index.as_int().unwrap()).collect::<Vec<_>>(), [1, 2], "saved option indices are ascending");
        let appearance = ap(&doc, &list.widgets[0]);
        assert_eq!(appearance.matches("0.6 0.75 0.86 rg").count(), 2, "both selected rows stay highlighted");
        if let Some(forward) = &forward_appearance {
            assert_eq!(&appearance, forward, "the same selected rows have the same appearance in either selection order");
        } else {
            forward_appearance = Some(appearance);
        }
    }
}

#[test]
fn errors_are_specific_and_change_nothing() {
    let mut doc = fixture();
    assert_eq!(set_value(&mut doc, "nope", &FieldValue::Text("x".into())), Err(FormError::NoSuchField("nope".into())));
    assert_eq!(set_value(&mut doc, "id", &FieldValue::Text("x".into())), Err(FormError::ReadOnly("id".into())));
    assert!(matches!(set_value(&mut doc, "zip", &FieldValue::Text("123456".into())), Err(FormError::Invalid(_))));
    assert!(matches!(set_value(&mut doc, "go", &FieldValue::Text("x".into())), Err(FormError::Invalid(_))));
    assert!(matches!(set_value(&mut doc, "agree", &FieldValue::Text("maybe".into())), Err(FormError::Invalid(_))));
    assert!(!doc.is_modified());
}

#[test]
fn reset_restores_defaults() {
    let mut doc = fixture();
    set_value(&mut doc, "name", &FieldValue::Text("Grace".into())).unwrap();
    set_value(&mut doc, "agree", &FieldValue::Check(true)).unwrap();
    set_value(&mut doc, "size", &FieldValue::Radio(Some("L".into()))).unwrap();
    assert_eq!(reset(&mut doc, Some(&["agree".to_string()])).unwrap(), 1);
    assert!(field(&fields(&doc), "agree").value.is_empty());
    assert_eq!(field(&fields(&doc), "name").value, ["Grace"], "only the named field");
    let n = reset(&mut doc, None).unwrap();
    assert!(n >= 2);
    let all = fields(&reopen(&doc));
    assert_eq!(field(&all, "name").value, ["Ada"]);
    assert_eq!(field(&all, "size").value, ["S"], "back to /DV");
    assert!(matches!(reset(&mut doc, Some(&["nope".to_string()])), Err(FormError::NoSuchField(_))));
}

#[test]
fn documents_without_forms_say_so() {
    let mut doc = Document::new_empty();
    assert_combined_matches_legacy(&doc, 0);
    assert!(fields(&doc).is_empty());
    assert_eq!(set_value(&mut doc, "x", &FieldValue::Text("y".into())), Err(FormError::NoForm));
}

#[test]
fn combined_enumeration_keeps_page_order_across_pages() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R] >>".into(),
        "<< /Fields [6 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (listed) /Rect [50 700 250 720] /P 3 0 R >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (adopted) /Rect [50 700 250 720] /P 4 0 R >>".into(),
    ];
    let doc = document(&objs);
    assert_combined_matches_legacy(&doc, 1);

    let combined = fields_with_adopted(&doc);
    assert_eq!(combined.items.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["listed", "adopted"]);
    assert_eq!(combined.items.iter().map(|f| f.widgets[0].page).collect::<Vec<_>>(), [Some(0), Some(1)]);
}

#[test]
fn combined_enumeration_keeps_leniency_for_dangling_field_entries() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [] >>".into(),
        "<< /Fields [99 0 R null 5 0 R] >>".into(),
        "<< /FT /Tx /T (valid) >>".into(),
    ];
    let doc = document(&objs);
    assert_combined_matches_legacy(&doc, 0);
    assert_eq!(fields_with_adopted(&doc).items.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["valid"]);
}

#[test]
fn every_field_type_can_be_added_named_and_drawn() {
    let mut doc = Document::new_empty();
    {
        // One page.
        let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        let mut p = pdfcraft_cos::Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
        let r = doc.add(p);
        doc.update_dict(pages, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();
    }
    let r = |y: f64| [50.0, y, 250.0, y + 20.0];
    let names: Vec<String> = [
        (NewField::Text { multiline: false }, r(700.0)),
        (NewField::Text { multiline: true }, [50.0, 600.0, 250.0, 680.0]),
        (NewField::Date, r(560.0)),
        (NewField::CheckBox, [50.0, 520.0, 64.0, 534.0]),
        (NewField::Radio { group: None, export: "Small".into() }, [50.0, 480.0, 64.0, 494.0]),
        (NewField::Combo { options: vec!["Red".into(), "Blue".into()], editable: false }, r(440.0)),
        (NewField::List { options: vec!["A".into(), "B".into(), "C".into()], multi: true }, [50.0, 340.0, 250.0, 420.0]),
        (NewField::Button { caption: "Submit".into() }, r(300.0)),
        (NewField::Signature, [50.0, 220.0, 250.0, 270.0]),
    ]
    .iter()
    .map(|(k, rect)| add_field(&mut doc, 0, *rect, k, None).unwrap())
    .collect();
    assert_eq!(names, ["Text1", "Text2", "Date1", "Check Box1", "Group1", "Dropdown1", "List Box1", "Button1", "Signature1"]);
    // A second radio button joins the group.
    assert_eq!(
        add_field(&mut doc, 0, [80.0, 480.0, 94.0, 494.0], &NewField::Radio { group: Some("Group1".into()), export: "Large".into() }, None).unwrap(),
        "Group1"
    );
    let doc = reopen(&doc);
    let all = fields(&doc);
    assert_eq!(all.len(), 9);
    let group = field(&all, "Group1");
    assert_eq!(group.widgets.iter().filter_map(|w| w.on_state.clone()).collect::<Vec<_>>(), ["Small", "Large"]);
    assert!(field(&all, "Text2").has(flags::MULTILINE));
    assert_eq!(field(&all, "Dropdown1").options.len(), 2);
    assert!(field(&all, "List Box1").has(flags::MULTI_SELECT));
    assert_eq!(field(&all, "Button1").kind, FieldKind::PushButton);
    assert!(ap(&doc, &field(&all, "Button1").widgets[0]).contains("(Submit) Tj"));
    // Every widget is on the page and has an appearance.
    for f in &all {
        for w in &f.widgets {
            assert_eq!(w.page, Some(0), "{}", f.name);
            assert!(doc.get(w.obj).as_dict().unwrap().contains(b"AP"), "{} has an appearance", f.name);
        }
    }
    // The new fields can be filled.
    let mut doc = doc;
    set_value(&mut doc, "Text1", &FieldValue::Text("hello".into())).unwrap();
    set_value(&mut doc, "Group1", &FieldValue::Radio(Some("Large".into()))).unwrap();
    assert!(add_field(&mut doc, 0, r(100.0), &NewField::CheckBox, Some("Text1")).is_err(), "names are unique");
    assert!(add_field(&mut doc, 0, [0.0, 0.0, 2.0, 2.0], &NewField::CheckBox, None).is_err(), "too small");
}

#[test]
fn properties_rename_and_delete() {
    let mut doc = fixture();
    let renamed = set_props(
        &mut doc,
        "name",
        &FieldProps {
            name: Some("full_name".into()),
            tooltip: Some("Your name".into()),
            required: Some(true),
            max_len: Some(Some(20)),
            font_size: Some(9.0),
            ..FieldProps::default()
        },
    )
    .unwrap();
    assert_eq!(renamed, "full_name");
    let all = fields(&doc);
    let f = field(&all, "full_name");
    assert!(f.has(flags::REQUIRED) && f.max_len == Some(20) && f.tooltip.as_deref() == Some("Your name"));
    assert!(ap(&doc, &f.widgets[0]).contains("/Helv 9 Tf"), "redrawn with the new size");
    let nested = set_props(&mut doc, "address.city", &FieldProps { name: Some("town".into()), ..FieldProps::default() }).unwrap();
    assert_eq!(nested, "address.town", "only the last part changes");
    set_props(&mut doc, "country", &FieldProps { options: Some(vec!["Japan".into()]), ..FieldProps::default() }).unwrap();
    assert_eq!(field(&fields(&doc), "country").options, [("Japan".to_string(), "Japan".to_string())]);
    assert!(set_props(&mut doc, "zip", &FieldProps { name: Some("pin".into()), ..FieldProps::default() }).is_err(), "taken");
    set_props(&mut doc, "full_name", &FieldProps { rect: Some((0, [10.0, 10.0, 110.0, 30.0])), ..FieldProps::default() }).unwrap();
    let f = field(&fields(&doc), "full_name").clone();
    assert_eq!(f.widgets[0].rect, [10.0, 10.0, 110.0, 30.0]);
    assert!(ap(&doc, &f.widgets[0]).contains("100"), "the appearance takes the new size");
    assert!(set_props(&mut doc, "full_name", &FieldProps { rect: Some((0, [0.0, 0.0, 2.0, 2.0])), ..FieldProps::default() }).is_err());
    let before = fields(&doc).len();
    delete_field(&mut doc, "size").unwrap();
    delete_field(&mut doc, "address.town").unwrap();
    let doc = reopen(&doc);
    let all = fields(&doc);
    assert_eq!(all.len(), before - 2);
    assert!(!all.iter().any(|f| f.name == "size" || f.name == "address.town"));
    let p = &page_refs(&doc)[0];
    let annots = doc.resolve(doc.get(*p).as_dict().unwrap().get(b"Annots").unwrap());
    assert_eq!(annots.as_array().unwrap().len(), 13 - 3, "the deleted widgets left the page");
}

#[test]
fn deleting_works_with_a_form_dictionary_inside_the_catalog() {
    let mut doc = fixture();
    let root = doc.root().unwrap();
    let af = doc.get(pdfcraft_cos::ObjRef::new(4, 0)).as_dict().cloned().unwrap();
    doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af))).unwrap();
    let before = fields(&doc).len();
    delete_field(&mut doc, "name").unwrap();
    let all = fields(&reopen(&doc));
    assert_eq!(all.len(), before - 1);
    assert!(!all.iter().any(|f| f.name == "name"));
}

fn one_page() -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut p = pdfcraft_cos::Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
    let r = doc.add(p);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    doc
}

#[test]
fn formats_validation_and_calculations_run_like_acrobat() {
    use crate::af::{CalcOp, Calculate, Format, Validate};
    let mut doc = one_page();
    let text = NewField::Text { multiline: false };
    for (name, y) in [("Price", 700.0), ("Qty", 660.0), ("Total", 620.0), ("Count", 580.0), ("Due", 540.0)] {
        add_field(&mut doc, 0, [50.0, y, 250.0, y + 20.0], &text, Some(name)).unwrap();
    }
    let money = Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true };
    set_props(
        &mut doc,
        "Price",
        &FieldProps { format: Some(money.clone()), validate: Some(Validate::Range { min: Some(0.0), max: None }), ..FieldProps::default() },
    )
    .unwrap();
    set_props(
        &mut doc,
        "Qty",
        &FieldProps {
            format: Some(Format::Number { decimals: 0, sep: 0, neg: 0, currency: String::new(), prepend: true }),
            validate: Some(Validate::Range { min: Some(1.0), max: Some(99.0) }),
            ..FieldProps::default()
        },
    )
    .unwrap();
    set_props(
        &mut doc,
        "Total",
        &FieldProps {
            format: Some(money),
            calculate: Some(Calculate::Notation("Price * Qty".into())),
            read_only: Some(true),
            ..FieldProps::default()
        },
    )
    .unwrap();
    set_props(
        &mut doc,
        "Count",
        &FieldProps { calculate: Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["Qty".into()] }), ..FieldProps::default() },
    )
    .unwrap();
    set_props(&mut doc, "Due", &FieldProps { format: Some(Format::Date("mmm d, yyyy".into())), ..FieldProps::default() }).unwrap();
    // The scripts are Acrobat's, so Acrobat (and we, after a save) read them back.
    let doc2 = reopen(&doc);
    let total = field(&fields(&doc2), "Total").clone();
    assert_eq!(total.actions.calculate, Calculate::Notation("Price * Qty".into()));
    assert!(matches!(total.actions.format, Format::Number { decimals: 2, .. }));
    let co =
        doc2.get(doc2.root().unwrap()).as_dict().unwrap().get(b"AcroForm").cloned().map(|a| doc2.resolve(&a).as_dict().cloned().unwrap()).unwrap();
    assert_eq!(doc2.resolve(co.get(b"CO").unwrap()).as_array().unwrap().len(), 2, "both calculated fields are in the calculation order");
    let mut doc = doc2;
    // Typing a formatted number stores the number and shows it formatted.
    set_value(&mut doc, "Price", &FieldValue::Text("$1,234.5".into())).unwrap();
    set_value(&mut doc, "Qty", &FieldValue::Text("2".into())).unwrap();
    let all = fields(&doc);
    assert_eq!(field(&all, "Price").value, ["1234.5"]);
    assert!(ap(&doc, &field(&all, "Price").widgets[0]).contains("($1,234.50) Tj"));
    // Calculations ran in order.
    assert_eq!(field(&all, "Total").value, ["2469"]);
    assert!(ap(&doc, &field(&all, "Total").widgets[0]).contains("($2,469.00) Tj"));
    assert_eq!(field(&all, "Count").value, ["2"]);
    // Keystroke and validation errors use Acrobat's messages and change nothing.
    assert_eq!(
        set_value(&mut doc, "Qty", &FieldValue::Text("lots".into())),
        Err(FormError::Invalid("The value entered does not match the format of the field [ Qty ]".into()))
    );
    assert_eq!(
        set_value(&mut doc, "Qty", &FieldValue::Text("120".into())),
        Err(FormError::Invalid("Invalid value: must be greater than or equal to 1 and less than or equal to 99.".into()))
    );
    assert_eq!(field(&fields(&doc), "Qty").value, ["2"]);
    // Dates: stored as typed, shown in the format; impossible dates are refused.
    set_value(&mut doc, "Due", &FieldValue::Text("10/1/2026".into())).unwrap();
    assert!(ap(&doc, &field(&fields(&doc), "Due").widgets[0]).contains("(Oct 1, 2026) Tj"));
    assert!(set_value(&mut doc, "Due", &FieldValue::Text("2/30/2026".into())).is_err());
    // Clear form recalculates.
    reset(&mut doc, None).unwrap();
    assert_eq!(field(&fields(&doc), "Total").value, ["0"]);
    // Removing a format.
    set_props(&mut doc, "Price", &FieldProps { format: Some(Format::None), ..FieldProps::default() }).unwrap();
    assert_eq!(field(&fields(&doc), "Price").actions.format, Format::None);
}

#[test]
fn new_text_fields_fit_their_box() {
    // #1888: a fixed 12 pt was clipped in a short box. New text and date fields take Acrobat's
    // Auto size: 12 pt where there is room, smaller where there isn't.
    let mut doc = one_page();
    let tf_size = |ap: &str| {
        ap.split("/Helv ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .and_then(|n| n.parse::<f64>().ok())
            .unwrap_or_else(|| panic!("no /Helv size in {ap}"))
    };
    for (kind, name, value) in [(NewField::Text { multiline: false }, "short", "Hello world"), (NewField::Date, "date", "10/13/2026")] {
        add_field(&mut doc, 0, [20.0, 20.0, 200.0, 28.0], &kind, Some(name)).unwrap();
        set_value(&mut doc, name, &FieldValue::Text(value.into())).unwrap();
        let f = field(&fields(&doc), name).clone();
        assert_eq!(f.da, "/Helv 0 Tf 0 g", "{name}");
        let size = tf_size(&ap(&doc, &f.widgets[0]));
        assert!(size > 0.0 && size <= 8.0, "{name}: {size} pt in an 8 pt box");
    }
    add_field(&mut doc, 0, [20.0, 50.0, 200.0, 72.0], &NewField::Text { multiline: false }, Some("tall")).unwrap();
    set_value(&mut doc, "tall", &FieldValue::Text("Hello world".into())).unwrap();
    assert_eq!(tf_size(&ap(&doc, &field(&fields(&doc), "tall").widgets[0])), 12.0, "a box with room keeps the 12 pt look");
    // An explicit size still wins.
    set_props(&mut doc, "short", &FieldProps { font_size: Some(12.0), ..FieldProps::default() }).unwrap();
    assert_eq!(tf_size(&ap(&doc, &field(&fields(&doc), "short").widgets[0])), 12.0);
}

#[test]
fn appearance_properties_restyle_every_widget() {
    let mut doc = one_page();
    add_field(&mut doc, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: false }, Some("name")).unwrap();
    set_value(&mut doc, "name", &FieldValue::Text("Ada".into())).unwrap();
    let f = field(&fields(&doc), "name").clone();
    let before = look(&doc, &f);
    assert_eq!((before.font, before.style, before.text), (FieldFont::Helvetica, BorderStyle::Solid, [0.0; 3]));
    let new = Look {
        border: Some([1.0, 0.0, 0.0]),
        fill: Some([1.0, 1.0, 0.8]),
        width: 2.0,
        style: BorderStyle::Dashed,
        text: [0.0, 0.0, 1.0],
        font: FieldFont::Times,
    };
    set_props(&mut doc, "name", &FieldProps { look: Some(new), font_size: Some(14.0), ..FieldProps::default() }).unwrap();
    let doc = reopen(&doc);
    let f = field(&fields(&doc), "name").clone();
    assert_eq!(look(&doc, &f), new);
    let a = ap(&doc, &f.widgets[0]);
    assert!(a.contains("/TiRo 14 Tf") && a.contains("0 0 1 rg") && a.contains("[3] 0 d") && a.contains("(Ada) Tj"), "{a}");
    let mut doc = doc;
    set_props(&mut doc, "name", &FieldProps { look: Some(Look { style: BorderStyle::Underline, fill: None, ..new }), ..FieldProps::default() })
        .unwrap();
    let a = ap(&doc, &field(&fields(&doc), "name").widgets[0]);
    assert!(!a.contains(" re S") && a.contains(" l S"), "underline only: {a}");
}

#[test]
fn tab_order_follows_the_page_setting() {
    let mut doc = one_page();
    let text = NewField::Text { multiline: false };
    // Added in a scrambled order: annotation order is C, A, D, B.
    // Layout:  A (50,700)  B (300,700)
    //          C (50,600)  D (300,600)
    for (name, x, y) in [("C", 50.0, 600.0), ("A", 50.0, 700.0), ("D", 300.0, 600.0), ("B", 300.0, 700.0)] {
        add_field(&mut doc, 0, [x, y, x + 200.0, y + 20.0], &text, Some(name)).unwrap();
    }
    let order = |doc: &Document| -> String {
        let mut all: Vec<(usize, String)> = fields(doc).into_iter().map(|f| (f.widgets[0].tab, f.name)).collect();
        all.sort();
        all.into_iter().map(|x| x.1).collect()
    };
    assert_eq!(order(&doc), "CADB", "unspecified: annotation order");
    set_tab_order(&mut doc, &[0], TabOrder::Row).unwrap();
    assert_eq!(order(&doc), "ABCD");
    set_tab_order(&mut doc, &[0], TabOrder::Column).unwrap();
    assert_eq!(order(&doc), "ACBD");
    let doc = reopen(&doc);
    assert_eq!(order(&doc), "ACBD", "saved as /Tabs");
    assert!(set_tab_order(&mut doc.clone(), &[3], TabOrder::Row).is_err());
}

#[test]
fn options_tab_flags_alignment_and_defaults() {
    let mut doc = one_page();
    let text = add_field(&mut doc, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: false }, None).unwrap();
    let combo = add_field(
        &mut doc,
        0,
        [50.0, 600.0, 250.0, 620.0],
        &NewField::Combo { options: vec!["Pear".into(), "apple".into(), "Fig".into()], editable: false },
        None,
    )
    .unwrap();
    let check = add_field(&mut doc, 0, [50.0, 500.0, 64.0, 514.0], &NewField::CheckBox, None).unwrap();
    // A comb needs a limit.
    let comb = FieldProps { flags: vec![(flags::COMB, true)], ..FieldProps::default() };
    assert!(matches!(set_props(&mut doc, &text, &comb), Err(FormError::Invalid(_))));
    set_props(
        &mut doc,
        &text,
        &FieldProps {
            flags: vec![(flags::COMB, true), (flags::DO_NOT_SPELL_CHECK, true)],
            max_len: Some(Some(6)),
            quadding: Some(1),
            default_value: Some(Some("ABC123".into())),
            ..FieldProps::default()
        },
    )
    .unwrap();
    let f = fields(&doc).into_iter().find(|f| f.name == text).unwrap();
    assert!(f.has(flags::COMB) && f.has(flags::DO_NOT_SPELL_CHECK) && !f.has(flags::MULTILINE));
    assert_eq!((f.max_len, f.quadding, f.default.clone()), (Some(6), 1, vec!["ABC123".to_string()]));
    // Reset form restores the default.
    set_value(&mut doc, &text, &FieldValue::Text("XYZ".into())).unwrap();
    reset(&mut doc, None).unwrap();
    assert_eq!(fields(&doc).into_iter().find(|f| f.name == text).unwrap().value, vec!["ABC123".to_string()]);
    // Sort items orders the list; custom text allowed.
    set_props(&mut doc, &combo, &FieldProps { flags: vec![(flags::SORT, true), (flags::EDIT, true)], ..FieldProps::default() }).unwrap();
    let c = fields(&doc).into_iter().find(|f| f.name == combo).unwrap();
    assert_eq!(c.options.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>(), vec!["apple", "Fig", "Pear"]);
    assert!(c.has(flags::EDIT));
    // Checked by default: the on state is the default.
    let on = fields(&doc).into_iter().find(|f| f.name == check).unwrap().widgets[0].on_state.clone().unwrap();
    set_props(&mut doc, &check, &FieldProps { default_value: Some(Some(on.clone())), ..FieldProps::default() }).unwrap();
    reset(&mut doc, None).unwrap();
    assert_eq!(fields(&doc).into_iter().find(|f| f.name == check).unwrap().value, vec![on]);
    assert!(set_props(&mut doc, &text, &FieldProps { quadding: Some(5), ..FieldProps::default() }).is_err());
}

#[test]
fn check_box_styles_change_the_mark() {
    // #94: Field Properties ▸ Options ▸ Check Box Style (stored as /MK /CA).
    let mut doc = one_page();
    let check = add_field(&mut doc, 0, [50.0, 700.0, 70.0, 720.0], &NewField::CheckBox, None).unwrap();
    let radio = add_field(&mut doc, 0, [50.0, 600.0, 70.0, 620.0], &NewField::Radio { group: None, export: "A".into() }, None).unwrap();
    let text = add_field(&mut doc, 0, [50.0, 500.0, 250.0, 520.0], &NewField::Text { multiline: false }, None).unwrap();
    let style = |doc: &Document, name: &str| check_style(doc, field(&fields(doc), name));
    assert_eq!((style(&doc, &check), style(&doc, &radio)), (CheckStyle::Check, CheckStyle::Circle), "the defaults");
    // The "on" appearance: the widget's /AP /N entry for its on state.
    let on_ap = |doc: &Document, name: &str| {
        let all = fields(doc);
        let w = &field(&all, name).widgets[0];
        let on = w.on_state.clone().unwrap();
        let n = doc
            .get(w.obj)
            .as_dict()
            .unwrap()
            .get(b"AP")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"N")
            .unwrap()
            .as_dict()
            .unwrap()
            .reference(on.as_bytes())
            .unwrap();
        let Object::Stream(s) = &*doc.get(n) else { panic!() };
        String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
    };
    let before = on_ap(&doc, &check);
    for (s, code) in
        [(CheckStyle::Cross, "8"), (CheckStyle::Square, "n"), (CheckStyle::Star, "H"), (CheckStyle::Diamond, "u"), (CheckStyle::Circle, "l")]
    {
        set_props(&mut doc, &check, &FieldProps { check_style: Some(s), ..FieldProps::default() }).unwrap();
        let doc2 = reopen(&doc);
        assert_eq!(style(&doc2, &check), s, "saved and read back");
        let mk = doc2.get(field(&fields(&doc2), &check).widgets[0].obj).as_dict().unwrap().get(b"MK").unwrap().as_dict().cloned().unwrap();
        assert_eq!(mk.get(b"CA").and_then(|c| c.as_string()).map(|c| c.to_text()).as_deref(), Some(code));
        assert!(mk.get(b"BC").is_some(), "the border colour is kept");
        assert_ne!(on_ap(&doc, &check), before, "{s:?} draws a different mark");
    }
    assert!(on_ap(&doc, &check).contains(" c f"), "a circle is a filled curve");
    set_props(&mut doc, &radio, &FieldProps { check_style: Some(CheckStyle::Square), ..FieldProps::default() }).unwrap();
    assert!(on_ap(&doc, &radio).contains(" re f"), "a square");
    assert!(
        set_props(&mut doc, &text, &FieldProps { check_style: Some(CheckStyle::Star), ..FieldProps::default() }).is_err(),
        "text fields have no style"
    );
}

#[test]
fn ordering_tabs_manually() {
    let mut doc = one_page();
    let text = NewField::Text { multiline: false };
    for (name, x, y) in [("A", 50.0, 700.0), ("B", 300.0, 700.0), ("C", 50.0, 600.0)] {
        add_field(&mut doc, 0, [x, y, x + 200.0, y + 20.0], &text, Some(name)).unwrap();
    }
    // A comment between the widgets keeps its place.
    let page = page_refs(&doc)[0];
    let mut note = pdfcraft_cos::Dict::new();
    note.set(b"Type".to_vec(), Object::name("Annot"));
    note.set(b"Subtype".to_vec(), Object::name("Text"));
    note.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
    let note = doc.add(Object::Dict(note));
    doc.update_dict(page, |d| {
        if let Some(Object::Array(a)) = d.get_mut(b"Annots") {
            a.insert(1, Object::Ref(note));
        }
    })
    .unwrap();
    set_tab_order(&mut doc, &[0], TabOrder::Row).unwrap();
    let order = |doc: &Document| -> String {
        let mut all: Vec<(usize, String)> = fields(doc).into_iter().map(|f| (f.widgets[0].tab, f.name)).collect();
        all.sort();
        all.into_iter().map(|x| x.1).collect()
    };
    assert_eq!(order(&doc), "ABC");
    move_in_tab_order(&mut doc, "C", true).unwrap();
    assert_eq!(order(&doc), "ACB", "row order became manual");
    move_in_tab_order(&mut doc, "A", false).unwrap();
    assert_eq!(order(&doc), "CAB");
    move_in_tab_order(&mut doc, "B", false).unwrap();
    assert_eq!(order(&doc), "CAB", "already last");
    let annots = doc.get(page).as_dict().unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    assert_eq!(annots[1].as_ref(), Some(note), "the comment stays where it was");
    assert!(!doc.get(page).as_dict().unwrap().contains(b"Tabs"));
    let doc = reopen(&doc);
    assert_eq!(order(&doc), "CAB", "saved");
}

#[test]
fn duplicating_a_field_across_pages_shares_its_value() {
    let mut doc = one_page();
    // A second and third page.
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    for _ in 0..2 {
        let mut p = pdfcraft_cos::Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
        let r = doc.add(p);
        doc.update_dict(pages, |d| {
            let mut kids = d.get(b"Kids").unwrap().as_array().unwrap().clone();
            kids.push(Object::Ref(r));
            d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
            d.set(b"Kids".to_vec(), Object::Array(kids));
        })
        .unwrap();
    }
    let name = add_field(&mut doc, 0, [50.0, 750.0, 250.0, 770.0], &NewField::Text { multiline: false }, Some("Initials")).unwrap();
    assert_eq!(duplicate_field(&mut doc, &name, &[0, 1, 2]).unwrap(), 2, "page 1 already has it");
    let f = fields(&doc).into_iter().find(|f| f.name == name).unwrap();
    assert_eq!(f.widgets.iter().filter_map(|w| w.page).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert!(f.widgets.iter().all(|w| w.rect == [50.0, 750.0, 250.0, 770.0]));
    set_value(&mut doc, &name, &FieldValue::Text("AL".into())).unwrap();
    let doc = reopen(&doc);
    let f = fields(&doc).into_iter().find(|f| f.name == name).unwrap();
    assert_eq!((f.value.clone(), f.widgets.len()), (vec!["AL".to_string()], 3), "one field, one value, three widgets");
    assert_eq!(fields(&doc).len(), 1);
    assert!(duplicate_field(&mut doc.clone(), &name, &[7]).is_err());
}

/// Fields with the same fully qualified name are one field (§12.7.3.1): a parent whose widgets are
/// its kids, as Duplicate Across Pages writes it, and separate field dictionaries that repeat the
/// same `/T`, as other writers do. Filling the name fills every one of them.
#[test]
fn fields_sharing_a_name_take_the_same_value() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".into(),                         // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 600 800] >>".into(),   // 2
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R 11 0 R] >>".into(),                    // 3
        "<< /Type /Page /Parent 2 0 R /Annots [8 0 R 9 0 R 12 0 R] >>".into(),              // 4
        "<< /Fields [6 0 R 9 0 R 11 0 R 12 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 10 0 R >> >> >>".into(), // 5
        "<< /FT /Tx /T (ClientName) /Kids [7 0 R 8 0 R] >>".into(),                         // 6 parent
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [50 700 250 720] /P 3 0 R >>".into(), // 7 its widget, page 1
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [50 700 250 720] /P 4 0 R >>".into(), // 8 its widget, page 2
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (ClientName) /Rect [50 600 250 620] /P 4 0 R >>".into(), // 9 same name, own field
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(), // 10
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Agree) /V /Off /AS /Off /Rect [300 700 315 715] /P 3 0 R /AP << /N << /Yes 13 0 R /Off 14 0 R >> >> >>".into(), // 11
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (Agree) /V /Off /AS /Off /Rect [300 700 315 715] /P 4 0 R /AP << /N << /Yes 13 0 R /Off 14 0 R >> >> >>".into(), // 12
        "<< /Length 0 >>\nstream\n\nendstream".into(),                                      // 13
        "<< /Length 0 >>\nstream\n\nendstream".into(),                                      // 14
    ];
    let mut doc = document(&objs);
    set_value(&mut doc, "ClientName", &FieldValue::Text("Ada Lovelace".into())).unwrap();
    set_value(&mut doc, "Agree", &FieldValue::Check(true)).unwrap();
    let doc = reopen(&doc);
    let v = |n: u32| doc.get(ObjRef { num: n, generation: 0 }).as_dict().unwrap().get(b"V").cloned();
    let typed = Some(Object::String(PdfString::text("Ada Lovelace")));
    assert_eq!((v(6), v(9)), (typed.clone(), typed), "the value sits on the parent and on the field that repeats its name");
    assert_eq!((v(7), v(8)), (None, None), "the parent's widgets carry no value of their own");
    let all = fields(&doc);
    let named: Vec<&Field> = all.iter().filter(|f| f.name == "ClientName").collect();
    let widgets: Vec<&Widget> = named.iter().flat_map(|f| &f.widgets).collect();
    assert_eq!(widgets.iter().map(|w| w.obj.num).collect::<Vec<_>>(), [7, 8, 9]);
    for w in widgets {
        assert!(ap(&doc, w).contains("Ada Lovelace"), "widget {} shows the value", w.obj.num);
    }
    let agree: Vec<&Field> = all.iter().filter(|f| f.name == "Agree").collect();
    assert_eq!(agree.len(), 2);
    for f in agree {
        assert_eq!((f.value.as_slice(), f.widgets[0].state.as_deref()), (&["Yes".to_string()][..], Some("Yes")), "check box {}", f.obj.num);
    }
}

#[test]
fn locked_fields_only_take_unlocking() {
    let mut doc = one_page();
    let name = add_field(&mut doc, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: false }, Some("ID")).unwrap();
    set_props(&mut doc, &name, &FieldProps { locked: Some(true), ..FieldProps::default() }).unwrap();
    assert!(fields(&doc)[0].locked());
    assert!(set_props(&mut doc, &name, &FieldProps { required: Some(true), ..FieldProps::default() }).is_err(), "locked");
    // Values can still be filled in.
    set_value(&mut doc, &name, &FieldValue::Text("42".into())).unwrap();
    set_props(&mut doc, &name, &FieldProps { locked: Some(false), required: Some(true), ..FieldProps::default() }).unwrap();
    let f = &fields(&doc)[0];
    assert!(!f.locked() && f.has(flags::REQUIRED));
}

#[test]
fn image_fields_ask_for_a_picture_and_show_it() {
    let mut doc = one_page();
    let name = add_field(&mut doc, 0, [50.0, 600.0, 250.0, 700.0], &NewField::Image, None).unwrap();
    assert_eq!(name, "Image1");
    let f = fields(&doc).into_iter().find(|f| f.name == name).unwrap();
    assert_eq!((f.kind, f.button.clone()), (FieldKind::PushButton, Some(af::ButtonAction::ImportIcon)));
    // A 4×2 image: the icon keeps its shape and is centred in the 200×100 box.
    let mut d = Dict::new();
    for (k, v) in
        [(&b"Type"[..], Object::name("XObject")), (b"Subtype", Object::name("Image")), (b"Width", Object::Int(4)), (b"Height", Object::Int(2))]
    {
        d.set(k.to_vec(), v);
    }
    let img = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, vec![0; 24])));
    set_button_icon(&mut doc, &name, img, (4, 2)).unwrap();
    let w = doc.get(f.widgets[0].obj).as_dict().cloned().unwrap();
    let mk = w.get(b"MK").unwrap().as_dict().unwrap().clone();
    assert_eq!(mk.int(b"TP"), Some(1));
    let ap = w.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let Object::Stream(s) = &*doc.get(ap) else { panic!("appearance") };
    let text = String::from_utf8(s.decoded().unwrap()).unwrap();
    assert!(text.contains("/Icon Do") && text.contains("49.000000 0 0 49.000000 2.000 1.000 cm"), "{text}");
    assert!(set_button_icon(&mut doc, "nope", img, (1, 1)).is_err());
}

#[test]
fn field_actions_round_trip_on_every_trigger() {
    let mut doc = fixture();
    let name = "go".to_string();
    let acts = vec![
        (Trigger::MouseUp, FieldAction::JavaScript("app.alert('hi')".into())),
        (Trigger::MouseEnter, FieldAction::ShowHide { fields: vec!["other".into()], hide: false }),
        (Trigger::OnFocus, FieldAction::Uri("https://example.org".into())),
        (Trigger::OnBlur, FieldAction::Reset(vec![])),
        (Trigger::MouseDown, FieldAction::GoTo(0)),
    ];
    set_field_actions(&mut doc, &name, &acts).unwrap();
    let mut got = field_actions(&doc, &name).unwrap();
    let mut want = acts.clone();
    let key = |x: &(Trigger, FieldAction)| Trigger::ALL.iter().position(|t| *t == x.0);
    got.sort_by_key(key);
    want.sort_by_key(key);
    assert_eq!(got, want);
    set_field_actions(&mut doc, &name, &[(Trigger::MouseExit, FieldAction::Named("NextPage".into()))]).unwrap();
    assert_eq!(field_actions(&doc, &name).unwrap(), [(Trigger::MouseExit, FieldAction::Named("NextPage".into()))]);
    // The Mouse Up script is what the button runs.
    set_field_actions(&mut doc, &name, &[(Trigger::MouseUp, FieldAction::JavaScript("this.print();".into()))]).unwrap();
    assert_combined_matches_legacy(&doc, 0);
    assert_eq!(fields(&doc).iter().find(|f| f.name == "go").unwrap().button, Some(af::ButtonAction::Named("Print".into())));
}

#[test]
fn push_buttons_read_hide_actions() {
    let mut doc = fixture();
    let go = field(&fields(&doc), "go").obj;
    let s = |t: &str| Object::String(PdfString::text(t));
    let city = ObjRef::new(16, 0);
    let null = ObjRef::new(9, 0);
    let button = |doc: &mut Document, t: Object, h: Option<bool>| {
        let mut a = Dict::new();
        a.set(b"S".to_vec(), Object::name("Hide"));
        a.set(b"T".to_vec(), t);
        if let Some(h) = h {
            a.set(b"H".to_vec(), Object::Bool(h));
        }
        doc.update_dict(go, |d| d.set(b"A".to_vec(), Object::Dict(a))).unwrap();
        field(&fields(doc), "go").button.clone()
    };
    let hide = |names: &[&str], hide: bool| Some(af::ButtonAction::ShowHide { fields: names.iter().map(|n| n.to_string()).collect(), hide });
    // A name; /H defaults to true (hide).
    assert_eq!(button(&mut doc, s("name"), None), hide(&["name"], true));
    // A widget reference names its field by its fully qualified name; /H false shows.
    assert_eq!(button(&mut doc, Object::Ref(city), Some(false)), hide(&["address.city"], false));
    // An array of both; a reference to something that isn't a field is skipped.
    assert_eq!(button(&mut doc, Object::Array(vec![s("name"), Object::Ref(city), Object::Ref(null)]), None), hide(&["name", "address.city"], true));
    // A /Parent cycle ends instead of looping.
    let looped = doc.add(Object::Dict(Dict::new()));
    doc.update_dict(looped, |d| {
        d.set(b"T".to_vec(), s("loop"));
        d.set(b"Parent".to_vec(), Object::Ref(looped));
    })
    .unwrap();
    assert_eq!(button(&mut doc, Object::Ref(looped), None), hide(&["loop"], true));
}

#[test]
fn push_buttons_read_set_layer_actions() {
    use af::LayerOp::{Off, On, Toggle};
    let mut doc = fixture();
    let go = field(&fields(&doc), "go").obj;
    let ocg = |n: u32| Object::Ref(ObjRef::new(n, 0));
    let button = |doc: &mut Document, state: Option<Object>, preserve_rb: Option<Object>| {
        let mut a = Dict::new();
        a.set(b"S".to_vec(), Object::name("SetOCGState"));
        if let Some(state) = state {
            a.set(b"State".to_vec(), state);
        }
        if let Some(p) = preserve_rb {
            a.set(b"PreserveRB".to_vec(), p);
        }
        doc.update_dict(go, |d| d.set(b"A".to_vec(), Object::Dict(a))).unwrap();
        field(&fields(doc), "go").button.clone()
    };
    let layers = |changes: &[(af::LayerOp, u32)], preserve_rb: bool| {
        Some(af::ButtonAction::SetLayers { changes: changes.iter().map(|&(op, n)| (op, (n, 0))).collect(), preserve_rb })
    };
    // Each name applies to the groups after it; /PreserveRB false is read.
    let state = Object::Array(vec![Object::name("ON"), ocg(30), ocg(31), Object::name("OFF"), ocg(32), Object::name("Toggle"), ocg(30)]);
    assert_eq!(button(&mut doc, Some(state), Some(Object::Bool(false))), layers(&[(On, 30), (On, 31), (Off, 32), (Toggle, 30)], false));
    // An indirect /State; groups before the first name or after an unknown one, and entries that
    // aren't groups, are skipped. /PreserveRB defaults to true, also when it isn't a boolean.
    let state = doc.add(Object::Array(vec![
        ocg(29),
        Object::name("OFF"),
        ocg(30),
        Object::Int(7),
        Object::name("Hide"),
        ocg(31),
        Object::name("Toggle"),
        ocg(32),
    ]));
    assert_eq!(button(&mut doc, Some(Object::Ref(state)), Some(Object::Int(0))), layers(&[(Off, 30), (Toggle, 32)], true));
    // No /State: an action that changes nothing (the button still has an action).
    assert_eq!(button(&mut doc, None, None), layers(&[], true));
}

#[test]
fn detection_finds_blanks_rules_boxes_and_names_them() {
    use crate::detect::*;
    let w = |t: &str, x0: f64, y0: f64, x1: f64| Word { text: t.into(), rect: [x0, y0, x1, y0 + 10.0] };
    let words = vec![
        w("Name:", 50.0, 700.0, 80.0),
        w("______________", 85.0, 699.0, 250.0),
        w("Date", 300.0, 700.0, 325.0),
        w("of", 328.0, 700.0, 338.0),
        w("birth", 341.0, 700.0, 365.0),
        w("I", 72.0, 602.0, 75.0),
        w("agree", 78.0, 602.0, 105.0),
        w("Comments", 50.0, 560.0, 100.0),
        w("Heading", 50.0, 750.0, 100.0),
    ];
    let shapes = Shapes {
        boxes: vec![[56.0, 600.0, 68.0, 612.0], [50.0, 500.0, 300.0, 540.0], [40.0, 20.0, 560.0, 780.0]],
        rules: vec![[370.0, 699.0, 500.0, 699.0], [50.0, 748.0, 100.0, 748.0]],
    };
    let found = detect(&words, &shapes, &[], &["Comments".into()]);
    let names: Vec<(&str, &Kind)> = found.iter().map(|c| (c.name.as_str(), &c.kind)).collect();
    assert_eq!(names, [("Name", &Kind::Text), ("Date of birth", &Kind::Text), ("I agree", &Kind::CheckBox), ("Comments2", &Kind::Text)], "{found:?}");
    // An existing field there: nothing new.
    let again = detect(&words, &shapes, &found.iter().map(|c| c.rect).collect::<Vec<_>>(), &[]);
    assert!(again.is_empty(), "{again:?}");
}

#[test]
fn detection_makes_a_row_of_touching_cells_one_comb_field() {
    use crate::detect::*;
    let w = |t: &str, x0: f64, y0: f64, x1: f64| Word { text: t.into(), rect: [x0, y0, x1, y0 + 10.0] };
    let words = vec![
        w("Postcode", 40.0, 205.0, 85.0),
        w("A", 88.0, 205.0, 95.0),
        w("Postcode", 40.0, 105.0, 85.0),
        w("B", 88.0, 105.0, 95.0),
        w("Yes", 70.0, 41.0, 90.0),
    ];
    let row = |n: usize, x: f64, y: f64, cw: f64, ch: f64| (0..n).map(move |i| [x + i as f64 * cw, y, x + (i + 1) as f64 * cw, y + ch]);
    // 18 x 22 cells (too tall for a check box), 16 x 20 cells (each one check-box sized) drawn
    // right to left, and a lone square.
    let mut boxes: Vec<[f64; 4]> = row(5, 120.0, 200.0, 18.0, 22.0).collect();
    boxes.extend(row(10, 120.0, 100.0, 16.0, 20.0).rev());
    boxes.push([50.0, 40.0, 62.0, 52.0]);
    let found = detect(&words, &Shapes { boxes, rules: Vec::new() }, &[], &[]);
    let got: Vec<(&str, &Kind, [f64; 4])> = found.iter().map(|c| (c.name.as_str(), &c.kind, c.rect)).collect();
    assert_eq!(
        got,
        [
            ("Postcode A", &Kind::Comb { cells: 5 }, [120.0, 200.0, 210.0, 222.0]),
            ("Postcode B", &Kind::Comb { cells: 10 }, [120.0, 100.0, 280.0, 120.0]),
            ("Yes", &Kind::CheckBox, [50.0, 40.0, 62.0, 52.0]),
        ],
        "{found:?}"
    );
    // Squares with a gap between them stay check boxes, and so do touching squares of
    // different sizes.
    let apart = Shapes {
        boxes: vec![[50.0, 40.0, 62.0, 52.0], [66.0, 40.0, 78.0, 52.0], [80.0, 40.0, 92.0, 52.0], [92.0, 40.0, 102.0, 52.0]],
        rules: Vec::new(),
    };
    let kinds: Vec<Kind> = detect(&[], &apart, &[], &[]).into_iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [Kind::CheckBox, Kind::CheckBox, Kind::CheckBox, Kind::CheckBox]);
    // An empty table of cells wider than tall (an attendance sheet) is not a comb.
    let grid = |cw: f64, ch: f64| {
        (0..3).flat_map(move |r| {
            (0..3).map(move |c| [100.0 + c as f64 * cw, 300.0 + r as f64 * ch, 100.0 + (c + 1) as f64 * cw, 300.0 + (r + 1) as f64 * ch])
        })
    };
    for (cw, ch) in [(24.0, 18.0), (28.0, 20.0)] {
        let table = detect(&[], &Shapes { boxes: grid(cw, ch).collect(), rules: Vec::new() }, &[], &[]);
        assert!(table.is_empty(), "{cw} x {ch}: {table:?}");
    }
}

#[test]
fn page_shapes_reads_boxes_and_rules() {
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let s = doc.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::flate(
        Default::default(),
        b"q 2 0 0 2 0 0 cm 10 10 6 6 re S 20 50 m 120 50 l S 0 0 m 5 5 l S 30 300 100 0.5 re f Q",
    )));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), pdfcraft_cos::Object::Ref(s))).unwrap();
    let sh = crate::detect::page_shapes(&doc, 0);
    assert_eq!(sh.boxes, [[20.0, 20.0, 32.0, 32.0]]);
    assert_eq!(sh.rules, [[40.0, 100.0, 240.0, 100.0], [60.0, 600.5, 260.0, 600.5]]);
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

fn nums(o: &Object) -> Vec<f64> {
    o.as_array().unwrap().iter().filter_map(Object::as_f64).collect()
}

/// Normal appearance `/BBox` and `/Matrix` (absent when the widget is not rotated).
fn appearance_box(doc: &Document, w: &Widget) -> (Vec<f64>, Option<Vec<f64>>) {
    let n = doc.get(w.obj).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let Object::Stream(s) = &*doc.get(n) else { panic!("appearance") };
    (nums(s.dict.get(b"BBox").unwrap()), s.dict.get(b"Matrix").map(nums))
}

#[test]
fn rotated_fields_draw_in_their_quadrant() {
    let mut doc = fixture();
    // No /R: the appearance is the widget rectangle and carries no /Matrix.
    set_value(&mut doc, "name", &FieldValue::Text("Ada".into())).unwrap();
    let (bbox, matrix) = appearance_box(&doc, &field(&fields(&doc), "name").widgets[0]);
    assert!(close(bbox[2], 200.0) && close(bbox[3], 20.0), "{bbox:?}");
    assert!(matrix.is_none(), "{matrix:?}");

    let set_r = |doc: &mut Document, degrees: i64| {
        let obj = field(&fields(doc), "name").widgets[0].obj;
        doc.update_dict(obj, |d| {
            let mut mk = d.get(b"MK").and_then(|m| m.as_dict().cloned()).unwrap_or_default();
            mk.set(b"R".to_vec(), Object::Int(degrees));
            d.set(b"MK".to_vec(), Object::Dict(mk));
        })
        .unwrap();
        set_value(doc, "name", &FieldValue::Text("Ada".into())).unwrap();
    };
    // 90 and 270 lay the text out in the swapped box. 180 keeps the box and turns it over.
    set_r(&mut doc, 90);
    let (bbox, matrix) = appearance_box(&doc, &field(&fields(&doc), "name").widgets[0]);
    assert!(close(bbox[2], 20.0) && close(bbox[3], 200.0), "{bbox:?}");
    let m = matrix.unwrap();
    assert!(close(m[0], 0.0) && close(m[1], 1.0) && close(m[2], -1.0) && close(m[4], 200.0), "{m:?}");
    set_r(&mut doc, 180);
    let (bbox, matrix) = appearance_box(&doc, &field(&fields(&doc), "name").widgets[0]);
    assert!(close(bbox[2], 200.0) && close(bbox[3], 20.0), "{bbox:?}");
    let m = matrix.unwrap();
    assert!(close(m[0], -1.0) && close(m[3], -1.0) && close(m[4], 200.0) && close(m[5], 20.0), "{m:?}");
    set_r(&mut doc, 270);
    let (bbox, matrix) = appearance_box(&doc, &field(&fields(&doc), "name").widgets[0]);
    assert!(close(bbox[2], 20.0) && close(bbox[3], 200.0), "{bbox:?}");
    let m = matrix.unwrap();
    assert!(close(m[1], -1.0) && close(m[2], 1.0) && close(m[5], 20.0), "{m:?}");
    // A non-quadrant /R is drawn upright.
    set_r(&mut doc, 45);
    let (bbox, matrix) = appearance_box(&doc, &field(&fields(&doc), "name").widgets[0]);
    assert!(matrix.is_none() && close(bbox[2], 200.0) && close(bbox[3], 20.0), "{bbox:?} {matrix:?}");
    assert_eq!(field(&fields(&doc), "name").widgets[0].rotation, 0);

    let before = field(&fields(&doc), "name").widgets[0].rect;
    assert!(set_props(&mut doc, "name", &FieldProps { rotation: Some((0, 45)), ..FieldProps::default() }).is_err());
    assert!(field(&fields(&doc), "name").widgets[0].rect.iter().zip(before).all(|(a, b)| close(*a, b)));
    set_props(&mut doc, "name", &FieldProps { rotation: Some((0, 90)), ..FieldProps::default() }).unwrap();
    let turned = field(&fields(&doc), "name").widgets[0].clone();
    assert_eq!(turned.rotation, 90);
    let r = turned.rect;
    // [50 700 250 720] around its center becomes 20 wide and 200 tall.
    assert!(close(r[0], 140.0) && close(r[1], 610.0) && close(r[2], 160.0) && close(r[3], 810.0), "{r:?}");
    let widget = doc.get(turned.obj);
    let mk = widget.as_dict().unwrap().get(b"MK").unwrap().as_dict().cloned().unwrap();
    assert!(mk.contains(b"BG") && mk.contains(b"BC") && mk.int(b"R") == Some(90));
    let (_, matrix) = appearance_box(&doc, &turned);
    assert!(matrix.is_some());
    // 270 stays on the vertical axis, so the rectangle is not swapped again.
    set_props(&mut doc, "name", &FieldProps { rotation: Some((0, 270)), ..FieldProps::default() }).unwrap();
    let r270 = field(&fields(&doc), "name").widgets[0].rect;
    assert!(r270.iter().zip(r).all(|(a, b)| close(*a, b)), "{r270:?}");

    // A check box and a push button use the same matrix.
    let mut doc = one_page();
    add_field(&mut doc, 0, [20.0, 40.0, 40.0, 60.0], &NewField::CheckBox, Some("agree")).unwrap();
    add_field(&mut doc, 0, [20.0, 80.0, 120.0, 110.0], &NewField::Button { caption: "Go".into() }, Some("go")).unwrap();
    set_props(&mut doc, "agree", &FieldProps { rotation: Some((0, 90)), ..FieldProps::default() }).unwrap();
    set_props(&mut doc, "go", &FieldProps { rotation: Some((0, 90)), ..FieldProps::default() }).unwrap();
    let agree = field(&fields(&doc), "agree").widgets[0].clone();
    let widget = doc.get(agree.obj);
    let n = widget.as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().get(b"N").cloned().unwrap();
    let states = doc.resolve(&n);
    let yes = states.as_dict().unwrap().reference(b"Yes").unwrap();
    let Object::Stream(s) = &*doc.get(yes) else { panic!("check") };
    assert!(s.dict.contains(b"Matrix"), "the check mark is rotated");
    let go = field(&fields(&doc), "go").widgets[0].clone();
    let (text, matrix) = (ap(&doc, &go), appearance_box(&doc, &go).1);
    assert!(text.contains("(Go) Tj") && matrix.is_some(), "{text}");
}

#[test]
fn a_rotation_refused_as_too_small_changes_nothing_else() {
    let mut doc = one_page();
    add_field(&mut doc, 0, [50.0, 700.0, 250.0, 720.0], &NewField::Text { multiline: false }, Some("thin")).unwrap();
    // 200 x 3: turned a quarter it would be 3 wide, too small to use.
    let obj = field(&fields(&doc), "thin").widgets[0].obj;
    doc.update_dict(obj, |d| d.set(b"Rect".to_vec(), Object::Array([50.0, 700.0, 250.0, 703.0].iter().map(|v| Object::Real(*v)).collect()))).unwrap();
    let props = FieldProps { tooltip: Some("changed".into()), rotation: Some((0, 90)), ..FieldProps::default() };
    assert!(set_props(&mut doc, "thin", &props).is_err());
    let f = field(&fields(&doc), "thin").clone();
    assert_eq!(f.tooltip, None, "the tooltip was written before the rotation was refused");
    assert_eq!(f.widgets[0].rotation, 0);
}

#[test]
fn filling_and_authoring_keep_shared_appearances_and_drop_stale_alternates() {
    let alternates: [(&[u8], &[u8]); 2] = [(b"R", b"0 0 20 10 re f\n"), (b"D", b"1 1 18 8 re S\n")];
    let cases = [
        (NewField::Text { multiline: false }, true),
        (NewField::Text { multiline: false }, false),
        (NewField::CheckBox, false),
        (NewField::Button { caption: "Button".into() }, false),
        (NewField::Signature, false),
    ];
    for (kind, fill) in cases {
        for indirect in [false, true] {
            let mut doc = one_page();
            let first = add_field(&mut doc, 0, [50.0, 600.0, 250.0, 630.0], &kind, Some("First")).unwrap();
            let other = add_field(&mut doc, 0, [50.0, 550.0, 250.0, 580.0], &kind, Some("Other")).unwrap();
            let first_widget = field(&fields(&doc), &first).widgets[0].obj;
            let other_widget = field(&fields(&doc), &other).widgets[0].obj;
            let mut entries = doc.get(first_widget).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().clone();
            for (key, bytes) in alternates {
                let mut d = Dict::new();
                d.set(b"Type".to_vec(), Object::name("XObject"));
                d.set(b"Subtype".to_vec(), Object::name("Form"));
                d.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 20.into(), 10.into()]));
                let r = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, bytes.to_vec())));
                entries.set(key.to_vec(), Object::Ref(r));
            }
            entries.set(b"VendorState".to_vec(), Object::name("Retained"));
            let shared = if indirect { Object::Ref(doc.add(Object::Dict(entries))) } else { Object::Dict(entries) };
            for widget in [first_widget, other_widget] {
                doc.update_dict(widget, |d| d.set(b"AP".to_vec(), shared.clone())).unwrap();
            }
            let original = reopen(&doc);
            for full in [false, true] {
                let mut doc = original.clone();
                let other_widget = field(&fields(&doc), &other).widgets[0].obj;
                let source = doc.get(other_widget).as_dict().unwrap().get(b"AP").unwrap().clone();
                let before = doc.resolve(&source);
                if fill {
                    set_value(&mut doc, &first, &FieldValue::Text("Updated value".into())).unwrap();
                } else {
                    set_props(&mut doc, &first, &FieldProps { tooltip: Some("Updated help".into()), ..FieldProps::default() }).unwrap();
                }
                assert_eq!(doc.resolve(&source), before, "a shared AP must not change");
                let bytes = if full {
                    pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap()
                } else {
                    assert!(!doc.revisions().is_empty(), "incremental save needs an existing revision");
                    assert!(!doc.encryption_changed() && !doc.full_save_required(), "this edit must not require a full rewrite");
                    let original = doc.bytes();
                    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
                    assert!(bytes.len() > original.len(), "incremental save appends the changed appearance");
                    assert!(bytes.starts_with(original.as_slice()), "incremental save preserves the complete original prefix");
                    bytes
                };
                hayro_syntax::Pdf::new(bytes.clone()).unwrap();
                let doc = Document::open(Arc::new(bytes)).unwrap();
                let all = fields(&doc);
                let first = field(&all, &first);
                let other = field(&all, &other);
                let first_ap = doc.resolve(doc.get(first.widgets[0].obj).as_dict().unwrap().get(b"AP").unwrap());
                let other_ap = doc.resolve(doc.get(other.widgets[0].obj).as_dict().unwrap().get(b"AP").unwrap());
                let first_ap = first_ap.as_dict().unwrap();
                let other_ap = other_ap.as_dict().unwrap();
                assert_ne!(first_ap.get(b"N"), other_ap.get(b"N"), "normal appearance was regenerated");
                assert_eq!(other_ap.len(), 4);
                // The redrawn widget keeps unknown entries, but not down/rollover looks that
                // would show its old value or caption on press or hover.
                assert_eq!(
                    first_ap.len(),
                    2,
                    "{kind:?} fill={fill} indirect={indirect} full={full}: {:?}",
                    first_ap.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect::<Vec<_>>()
                );
                for (key, bytes) in alternates {
                    assert!(first_ap.get(key).is_none(), "stale /{}", String::from_utf8_lossy(key));
                    let object = doc.resolve(other_ap.get(key).unwrap());
                    let Object::Stream(stream) = &*object else { panic!("alternate appearance") };
                    assert_eq!(stream.decoded().unwrap(), bytes);
                }
                assert_eq!(first_ap.name(b"VendorState"), Some(&b"Retained"[..]));
                assert_eq!(other_ap.name(b"VendorState"), Some(&b"Retained"[..]));
                if fill {
                    assert_eq!(first.value, ["Updated value"]);
                    assert!(ap(&doc, &first.widgets[0]).contains("(Updated value) Tj"));
                    assert!(other.value.is_empty());
                } else {
                    assert_eq!(first.tooltip.as_deref(), Some("Updated help"));
                    assert_eq!(other.tooltip, None);
                }
            }
        }
    }
}

#[test]
fn pushbutton_redraw_resolves_caption_and_layout_after_save_and_reopen() {
    fn saved(doc: &Document, full: bool) -> Document {
        let bytes = if full {
            pdfcraft_cos::write_full(doc, &SaveOptions::default()).unwrap()
        } else {
            assert!(!doc.revisions().is_empty(), "incremental save needs an existing revision");
            assert!(!doc.encryption_changed() && !doc.full_save_required(), "this edit must not require a full rewrite");
            let original = doc.bytes();
            let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
            assert!(bytes.len() > original.len(), "incremental save appends the changed appearance");
            assert!(bytes.starts_with(original.as_slice()), "incremental save preserves the complete original prefix");
            bytes
        };
        hayro_syntax::Pdf::new(bytes.clone()).unwrap();
        Document::open(Arc::new(bytes)).unwrap()
    }

    for indirect_caption in [false, true] {
        for indirect_layout in [false, true] {
            for icon_only in [false, true] {
                let mut doc = one_page();
                let name =
                    add_field(&mut doc, 0, [50.0, 600.0, 250.0, 650.0], &NewField::Button { caption: "Original caption".into() }, Some("Picture"))
                        .unwrap();
                let widget = field(&fields(&doc), &name).widgets[0].obj;
                let caption = Object::String(pdfcraft_cos::PdfString::literal(b"Original caption".to_vec()));
                let caption = if indirect_caption { Object::Ref(doc.add(caption)) } else { caption };
                let layout = Object::Int(i64::from(icon_only));
                let layout = if indirect_layout { Object::Ref(doc.add(layout)) } else { layout };
                let mut icon_dict = Dict::new();
                icon_dict.set(b"Type".to_vec(), Object::name("XObject"));
                icon_dict.set(b"Subtype".to_vec(), Object::name("Form"));
                icon_dict.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 4.into(), 2.into()]));
                let icon_bytes = b"0 0 4 2 re f\n";
                let icon = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(icon_dict, icon_bytes.to_vec())));
                let mut mk = Dict::new();
                mk.set(b"CA".to_vec(), caption);
                mk.set(b"TP".to_vec(), layout);
                mk.set(b"I".to_vec(), Object::Ref(icon));
                mk.set(b"VendorNote".to_vec(), Object::name("Retained"));
                let mk = doc.add(Object::Dict(mk));
                doc.update_dict(widget, |d| d.set(b"MK".to_vec(), Object::Ref(mk))).unwrap();

                // Import the operands from a real saved file before an unrelated public edit.
                let original = saved(&doc, true);
                for full in [false, true] {
                    let mut doc = original.clone();
                    let widget = field(&fields(&doc), &name).widgets[0].obj;
                    let mk_before = doc.get(widget).as_dict().unwrap().get(b"MK").unwrap().clone();
                    let original_mk = doc.resolve(&mk_before);
                    set_props(&mut doc, &name, &FieldProps { tooltip: Some("Picture help".into()), ..FieldProps::default() }).unwrap();
                    assert_eq!(doc.get(widget).as_dict().unwrap().get(b"MK"), Some(&mk_before));
                    assert_eq!(doc.resolve(&mk_before), original_mk, "redraw must not rewrite the source MK");
                    let doc = saved(&doc, full);
                    let f = field(&fields(&doc), &name).clone();
                    assert_eq!(f.tooltip.as_deref(), Some("Picture help"));
                    let w = &f.widgets[0];
                    let wd = doc.get(w.obj);
                    let mk_object = doc.resolve(wd.as_dict().unwrap().get(b"MK").unwrap());
                    let mk = mk_object.as_dict().unwrap();
                    assert_eq!(mk.get(b"CA").unwrap().as_ref().is_some(), indirect_caption);
                    assert_eq!(doc.resolve(mk.get(b"CA").unwrap()).as_string().unwrap().bytes, b"Original caption");
                    assert_eq!(mk.get(b"TP").unwrap().as_ref().is_some(), indirect_layout);
                    assert_eq!(doc.resolve(mk.get(b"TP").unwrap()).as_int(), Some(i64::from(icon_only)));
                    assert_eq!(mk.name(b"VendorNote"), Some(&b"Retained"[..]));
                    let appearance = ap(&doc, w);
                    assert_eq!(appearance.contains("(Original caption) Tj"), !icon_only, "{appearance}");
                    assert_eq!(appearance.matches("/Icon Do").count(), 1, "{appearance}");
                    assert!(appearance.contains("24.000000 0 0 24.000000 52.000 1.000 cm"), "{appearance}");
                    let n = wd.as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
                    let appearance_object = doc.get(n);
                    let Object::Stream(stream) = &*appearance_object else { panic!("appearance") };
                    let resource_icon =
                        stream.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().get(b"Icon").unwrap();
                    assert_eq!(resource_icon, mk.get(b"I").unwrap());
                    let icon_object = doc.resolve(resource_icon);
                    let Object::Stream(icon) = &*icon_object else { panic!("icon") };
                    assert_eq!(icon.decoded().unwrap(), icon_bytes);
                }
            }
        }
    }
}

/// A check box's down appearances survive a redraw for the states it still draws (same names),
/// and a down appearance for a state it no longer has is dropped rather than left stale.
#[test]
fn check_box_redraw_keeps_down_states_it_still_draws() {
    let mut doc = one_page();
    let name = add_field(&mut doc, 0, [50.0, 600.0, 70.0, 620.0], &NewField::CheckBox, Some("agree")).unwrap();
    let widget = field(&fields(&doc), &name).widgets[0].obj;
    let on = field(&fields(&doc), &name).widgets[0].on_state.clone().unwrap_or_else(|| "Yes".into());
    let mut ap = doc.get(widget).as_dict().unwrap().get(b"AP").unwrap().as_dict().unwrap().clone();
    let mut down = Dict::new();
    for state in [on.as_bytes(), b"Off".as_slice(), b"Gone".as_slice()] {
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        d.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 20.into(), 20.into()]));
        let r = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, b"0 0 20 20 re f\n".to_vec())));
        down.set(state.to_vec(), Object::Ref(r));
    }
    ap.set(b"D".to_vec(), Object::Dict(down));
    doc.update_dict(widget, |d| d.set(b"AP".to_vec(), Object::Dict(ap))).unwrap();
    set_props(&mut doc, &name, &FieldProps { tooltip: Some("Tick to agree".into()), ..FieldProps::default() }).unwrap();
    let ap = doc.get(widget).as_dict().unwrap().get(b"AP").map(|a| doc.resolve(a)).unwrap();
    let d = ap.as_dict().unwrap().get(b"D").map(|d| doc.resolve(d)).expect("down states kept");
    let d = d.as_dict().unwrap();
    assert!(d.contains(on.as_bytes()) && d.contains(b"Off"));
    assert!(!d.contains(b"Gone"), "a state the box no longer draws");
}

#[test]
fn recalc_chain_where_script_changes_checkbox() {
    struct TestScripts;
    impl Scripts for TestScripts {
        fn run(&mut self, _event: FieldEvent, script: &str, _target: &Field, _value: &str, fields: &[Field]) -> ScriptResult {
            if script == "set_box" {
                ScriptResult {
                    rc: true,
                    value: "calc1".into(),
                    changes: vec![FieldChange {
                        name: "agree".into(),
                        value: Some(vec!["Yes".into()]),
                        read_only: None,
                        required: None,
                        display: None,
                    }],
                    message: None,
                }
            } else if script == "read_box" {
                let box_val = fields.iter().find(|f| f.name == "agree").and_then(|f| f.value.first()).cloned().unwrap_or_default();
                ScriptResult { rc: true, value: format!("box_was_{box_val}"), changes: Vec::new(), message: None }
            } else {
                ScriptResult { rc: true, value: String::new(), changes: Vec::new(), message: None }
            }
        }
    }

    let mut doc = one_page();
    add_field(&mut doc, 0, [10.0, 10.0, 100.0, 30.0], &NewField::Text { multiline: false }, Some("trigger")).unwrap();
    add_field(&mut doc, 0, [10.0, 40.0, 30.0, 60.0], &NewField::CheckBox, Some("agree")).unwrap();
    add_field(&mut doc, 0, [10.0, 70.0, 100.0, 90.0], &NewField::Text { multiline: false }, Some("result")).unwrap();

    set_field_script(&mut doc, "trigger", "calculate", Some("set_box")).unwrap();
    set_field_script(&mut doc, "result", "calculate", Some("read_box")).unwrap();

    let changed = recalculate_with(&mut doc, &mut TestScripts).unwrap();
    assert!(changed >= 2);

    let all = fields(&doc);
    assert_eq!(field(&all, "agree").value, ["Yes"], "checkbox was set by script in recalc chain");
    assert_eq!(field(&all, "result").value, ["box_was_Yes"], "second recalc saw the normalized checkbox value");
}

#[test]
fn partial_appearance_keeps_custom_default_appearance_and_font_resources() {
    let mut doc = fixture();
    let obj = field(&fields(&doc), "name").obj;
    let af = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"AcroForm").unwrap();
    let mut dr = doc.get(af).as_dict().unwrap().get(b"DR").map(|v| doc.resolve(v)).unwrap().as_dict().cloned().unwrap();
    let mut fonts = dr.get(b"Font").map(|v| doc.resolve(v)).unwrap().as_dict().cloned().unwrap();
    let resource = fonts.get(b"Helv").cloned().unwrap();
    fonts.set(b"ProjectFont".to_vec(), resource.clone());
    dr.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(af, |d| d.set(b"DR".to_vec(), Object::Dict(dr))).unwrap();
    let da = "/ProjectFont 13 Tf 0.15 g 2 Tc";
    doc.update_dict(obj, |d| d.set(b"DA".to_vec(), pdfcraft_cos::PdfString::literal(da.as_bytes().to_vec()))).unwrap();

    let patch = LookPatch { border: Some(Some([1.0, 0.0, 0.0])), width: Some(3.0), ..Default::default() };
    set_props(&mut doc, "name", &FieldProps { appearance: Some(patch), ..Default::default() }).unwrap();
    let f = field(&fields(&doc), "name").clone();
    assert_eq!(f.da, da, "a border edit must not replace the custom font or colour space");
    assert_eq!(f.value, ["Ada"]);
    assert_eq!(look(&doc, &f).fill, Some([1.0, 1.0, 0.9]));

    set_props(&mut doc, "name", &FieldProps { font_size: Some(18.0), ..Default::default() }).unwrap();
    let f = field(&fields(&doc), "name").clone();
    // The size is rewritten in place: other operators survive and the string doesn't grow.
    assert_eq!(f.da, "/ProjectFont 18 Tf 0.15 g 2 Tc", "other default-appearance operators survive");
    assert_eq!(appearance::parse_da(&f.da).font, "ProjectFont");
    assert_eq!(appearance::parse_da(&f.da).size, 18.0);

    set_props(
        &mut doc,
        "name",
        &FieldProps { appearance: Some(LookPatch { text: Some([0.0, 0.0, 1.0]), ..Default::default() }), ..Default::default() },
    )
    .unwrap();
    let doc = reopen(&doc);
    let f = field(&fields(&doc), "name").clone();
    assert_eq!(f.da, "/ProjectFont 18 Tf 0 0 1 rg 2 Tc", "the colour is replaced where it was");
    assert_eq!(appearance::parse_da(&f.da).font, "ProjectFont");
    assert_eq!(appearance::parse_da(&f.da).color, "0 0 1 rg");
    let dr = doc.get(af).as_dict().unwrap().get(b"DR").map(|v| doc.resolve(v)).unwrap();
    let fonts = dr.as_dict().unwrap().get(b"Font").map(|v| doc.resolve(v)).unwrap();
    assert_eq!(fonts.as_dict().unwrap().get(b"ProjectFont"), Some(&resource));
}

#[test]
fn repeated_partial_appearance_edits_keep_the_default_appearance_one_operator_each() {
    let mut doc = fixture();
    for size in [9.0, 11.0, 14.0, 250.0, 400.0] {
        let props = FieldProps {
            appearance: Some(LookPatch { text: Some([0.0, 0.5, 0.0]), ..Default::default() }),
            font_size: Some(size),
            ..Default::default()
        };
        set_props(&mut doc, "name", &props).unwrap();
    }
    let da = field(&fields(&doc), "name").da.clone();
    assert_eq!(da.matches("Tf").count(), 1, "{da}");
    assert_eq!(da.matches(" rg").count() + da.matches(" g").count(), 1, "{da}");
    // Sizes up to the 300 points `parse_da` reads are kept, not cut to 100.
    assert_eq!(appearance::parse_da(&da).size, 300.0, "{da}");
}

#[test]
fn a_font_only_edit_leaves_the_widget_and_a_shared_indirect_border_untouched() {
    let mut doc = fixture();
    let widgets = field(&fields(&doc), "size").widgets.clone();
    let mut mk = pdfcraft_cos::Dict::new();
    mk.set(b"BC".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into()]));
    let shared = doc.add(Object::Dict(mk));
    for w in &widgets {
        doc.update_dict(w.obj, |d| d.set(b"MK".to_vec(), Object::Ref(shared))).unwrap();
    }
    let border = |doc: &Document| -> Vec<_> {
        widgets.iter().map(|w| doc.get(w.obj).as_dict().map(|d| (d.get(b"MK").cloned(), d.get(b"BS").cloned()))).collect()
    };
    let before = border(&doc);
    set_props(&mut doc, "size", &FieldProps { font_size: Some(12.0), ..Default::default() }).unwrap();
    assert_eq!(border(&doc), before, "a size-only edit leaves each widget's /MK and /BS alone");
    // A border edit updates the shared /MK where it lives instead of copying it into each widget.
    let patch = LookPatch { border: Some(Some([1.0, 0.0, 0.0])), ..Default::default() };
    set_props(&mut doc, "size", &FieldProps { appearance: Some(patch), ..Default::default() }).unwrap();
    for w in &widgets {
        assert_eq!(doc.get(w.obj).as_dict().unwrap().get(b"MK"), Some(&Object::Ref(shared)), "the widget still points at the shared /MK");
    }
    let shared_mk = doc.get(shared).as_dict().cloned().unwrap();
    assert_eq!(shared_mk.get(b"BC"), Some(&Object::Array(vec![Object::Real(1.0), Object::Real(0.0), Object::Real(0.0)])));
}

#[test]
fn partial_appearance_preserves_each_widgets_indirect_colours_and_border_extensions() {
    let mut doc = fixture();
    let widgets = field(&fields(&doc), "size").widgets.clone();
    let mut refs = Vec::new();
    for (i, widget) in widgets.iter().enumerate() {
        let mut mk = pdfcraft_cos::Dict::new();
        mk.set(b"BG".to_vec(), Object::Array(vec![Object::Real(i as f64), 0.into(), 1.into()]));
        mk.set(b"BC".to_vec(), Object::Array(vec![0.into(), Object::Real(i as f64), 0.into()]));
        mk.set(b"CA".to_vec(), pdfcraft_cos::PdfString::text("l"));
        mk.set(b"R".to_vec(), Object::Int(90));
        mk.set(b"VendorKey".to_vec(), Object::Int(71));
        let mk_ref = doc.add(Object::Dict(mk));
        let mut bs = pdfcraft_cos::Dict::new();
        bs.set(b"S".to_vec(), Object::name("D"));
        bs.set(b"W".to_vec(), Object::Real(1.0));
        bs.set(b"D".to_vec(), Object::Array(vec![4.into(), 2.into()]));
        bs.set(b"VendorKey".to_vec(), Object::Int(83));
        let bs_ref = doc.add(Object::Dict(bs));
        let da = if i == 0 { "/Helv 8 Tf 0.1 g" } else { "/Cour 19 Tf 0.3 g" };
        doc.update_dict(widget.obj, |d| {
            d.set(b"MK".to_vec(), Object::Ref(mk_ref));
            d.set(b"BS".to_vec(), Object::Ref(bs_ref));
            d.set(b"DA".to_vec(), pdfcraft_cos::PdfString::text(da));
        })
        .unwrap();
        refs.push(mk_ref);
    }
    let patch = LookPatch { width: Some(5.0), ..Default::default() };
    set_props(&mut doc, "size", &FieldProps { font_size: Some(16.0), appearance: Some(patch), ..Default::default() }).unwrap();
    for (i, widget) in widgets.iter().enumerate() {
        let wd = doc.get(widget.obj);
        let wd = wd.as_dict().unwrap();
        assert_eq!(wd.reference(b"MK"), Some(refs[i]), "untouched MK stays indirect");
        let bs = wd.get(b"BS").map(|v| doc.resolve(v)).unwrap();
        let bs = bs.as_dict().unwrap();
        assert_eq!(bs.get(b"W").and_then(Object::as_f64), Some(5.0));
        assert_eq!(bs.int(b"VendorKey"), Some(83));
        assert_eq!(bs.get(b"D").unwrap().as_array().unwrap().len(), 2);
        let da = wd.get(b"DA").map(|v| doc.resolve(v)).unwrap().as_string().unwrap().to_text();
        let parsed = appearance::parse_da(&da);
        assert_eq!(parsed.font, if i == 0 { "Helv" } else { "Cour" });
        assert_eq!(parsed.color, if i == 0 { "0.1 g" } else { "0.3 g" });
        assert_eq!(parsed.size, 16.0);
    }
    set_props(&mut doc, "size", &FieldProps { appearance: Some(LookPatch { fill: Some(None), ..Default::default() }), ..Default::default() })
        .unwrap();
    let doc = reopen(&doc);
    for widget in &widgets {
        let mk = doc.get(widget.obj).as_dict().unwrap().get(b"MK").map(|v| doc.resolve(v)).unwrap();
        let mk = mk.as_dict().unwrap();
        assert!(!mk.contains(b"BG"));
        assert!(mk.contains(b"BC") && mk.contains(b"CA"));
        assert_eq!(mk.int(b"R"), Some(90));
        assert_eq!(mk.int(b"VendorKey"), Some(71));
    }
}

/// A whole look sets the colours of an indirect /MK and keeps its mark, rotation and unknown
/// entries (#1322), as it already did for a direct one.
#[test]
fn a_whole_look_keeps_the_rest_of_an_indirect_mk() {
    let mut doc = fixture();
    let widget = field(&fields(&doc), "agree").widgets[0].obj;
    let mut mk = pdfcraft_cos::Dict::new();
    mk.set(b"CA".to_vec(), pdfcraft_cos::PdfString::text("8"));
    mk.set(b"R".to_vec(), Object::Int(90));
    mk.set(b"VendorKey".to_vec(), Object::Int(71));
    let mk_ref = doc.add(Object::Dict(mk));
    doc.update_dict(widget, |d| d.set(b"MK".to_vec(), Object::Ref(mk_ref))).unwrap();
    let new = Look {
        border: Some([1.0, 0.0, 0.0]),
        fill: Some([1.0, 1.0, 0.8]),
        width: 2.0,
        style: BorderStyle::Solid,
        text: [0.0; 3],
        font: FieldFont::Helvetica,
    };
    set_props(&mut doc, "agree", &FieldProps { look: Some(new), ..FieldProps::default() }).unwrap();
    let doc = reopen(&doc);
    let f = field(&fields(&doc), "agree").clone();
    assert_eq!((check_style(&doc, &f), f.widgets[0].rotation), (CheckStyle::Cross, 90));
    let mk = doc.get(widget).as_dict().unwrap().get(b"MK").map(|v| doc.resolve(v)).unwrap();
    let mk = mk.as_dict().unwrap();
    assert_eq!(mk.int(b"VendorKey"), Some(71));
    assert_eq!(look(&doc, &f).border, Some([1.0, 0.0, 0.0]));
    assert_eq!(look(&doc, &f).fill, Some([1.0, 1.0, 0.8]));
}

#[test]
fn invalid_appearance_patches_are_refused_before_anything_is_written() {
    let mut doc = fixture();
    let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
    for size in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(
            set_props(&mut doc, "name", &FieldProps { font_size: Some(size), tooltip: Some("Not applied".into()), ..Default::default() }).is_err()
        );
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    }
    for patch in [
        LookPatch { width: Some(f64::NAN), ..Default::default() },
        LookPatch { width: Some(-1.0), ..Default::default() },
        LookPatch { fill: Some(Some([f64::INFINITY, 0.0, 0.0])), ..Default::default() },
        LookPatch { text: Some([1.1, 0.0, 0.0]), ..Default::default() },
    ] {
        assert!(
            set_props(&mut doc, "name", &FieldProps { appearance: Some(patch), tooltip: Some("Not applied".into()), ..Default::default() }).is_err()
        );
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    }
    let props = FieldProps {
        appearance: Some(LookPatch { width: Some(2.0), ..Default::default() }),
        look: Some(look(&doc, &field(&fields(&doc), "name").clone())),
        ..Default::default()
    };
    assert!(set_props(&mut doc, "name", &props).is_err());
    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
}

#[test]
fn page_shapes_preserves_closed_path_rectangle_height() {
    let shapes = |content: &[u8]| {
        let mut doc = fixture();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let stream = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Default::default(), content)));
        doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
        crate::detect::page_shapes(&doc, 0)
    };
    let shorthand = shapes(b"50 100 120 40 re S");
    let outlined = shapes(b"50 100 m 170 100 l 170 140 l 50 140 l h S");
    assert_eq!(outlined, shorthand, "equivalent rectangle encodings must have equal detection geometry");
    let fields = crate::detect::detect(&[], &outlined, &[], &[]);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].rect, [51.0, 101.0, 169.0, 139.0]);
}

fn detection_test_shapes(content: &[u8]) -> crate::detect::Shapes {
    let mut doc = fixture();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let stream = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Default::default(), content)));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(stream))).unwrap();
    crate::detect::page_shapes(&doc, 0)
}

#[test]
fn page_shapes_accepts_rectangle_closure_and_paint_variants() {
    for end in ["h S", "s", "50 100 l S", "f", "F", "f*", "B", "B*", "b", "b*"] {
        let content = format!("50 100 m 170 100 l 170 140 l 50 140 l {end}");
        let shapes = detection_test_shapes(content.as_bytes());
        assert_eq!(shapes.boxes, [[50.0, 100.0, 170.0, 140.0]], "{end}");
        assert!(shapes.rules.is_empty(), "rectangle edges must not become extra fields: {end}");
    }
}

/// #2942: a cell drawn with a point in the middle of each vertical side (the French 2042 form's
/// letter cells), or on every side, is the rectangle it looks like. A point that turns back
/// along a side is not on the way between its neighbours, so such a path is still not a box.
#[test]
fn page_shapes_reads_rectangles_with_points_along_their_sides() {
    for content in [
        "100 100 m 111.1 100 l 111.1 107.35 l 111.1 114.7 l 100 114.7 l 100 107.35 l h f",
        "100 100 m 105 100 l 111.1 100 l 111.1 107.35 l 111.1 114.7 l 105 114.7 l 100 114.7 l 100 107.35 l h S",
        "100 107.35 m 100 100 l 111.1 100 l 111.1 114.7 l 100 114.7 l h f",
    ] {
        let shapes = detection_test_shapes(content.as_bytes());
        assert_eq!(shapes.boxes, [[100.0, 100.0, 111.1, 114.7]], "{content}");
    }
    for content in [
        // Out past the corner and back: a spike, not a side.
        "100 100 m 130 100 l 111.1 100 l 111.1 114.7 l 100 114.7 l h S",
        // A sixth point off the sides.
        "100 100 m 111.1 100 l 115 107 l 111.1 114.7 l 100 114.7 l 100 107.35 l h S",
    ] {
        assert!(detection_test_shapes(content.as_bytes()).boxes.is_empty(), "{content}");
    }
}

#[test]
fn page_shapes_rejects_open_diagonal_curved_and_complex_rectangles() {
    for content in [
        "50 100 m 170 100 l 170 140 l 50 140 l S",
        "50 100 m 170 110 l 170 150 l 50 140 l h S",
        "50 100 m 170 100 l 170 140 l 100 150 80 150 50 140 c h S",
        "50 100 m 170 100 l 170 140 l h 50 140 l f",
        "50 100 m 170 100 l 50 100 l 50 140 l h S",
    ] {
        assert!(detection_test_shapes(content.as_bytes()).boxes.is_empty(), "{content}");
    }
}

#[test]
fn page_shapes_preserves_multiple_subpaths_and_discards_unpainted_paths() {
    let shapes = detection_test_shapes(b"50 100 120 40 re 200 100 m 320 100 l 320 140 l 200 140 l h 50 400 m 170 400 l S");
    assert_eq!(shapes.boxes, [[50.0, 100.0, 170.0, 140.0], [200.0, 100.0, 320.0, 140.0]]);
    assert_eq!(shapes.rules, [[50.0, 400.0, 170.0, 400.0]]);
    assert_eq!(detection_test_shapes(b"50 100 m 170 100 l 170 140 l 50 140 l h W n"), crate::detect::Shapes::default());
    let continued = detection_test_shapes(b"50 100 120 40 re 170 100 l 170 140 l 50 140 l h S");
    assert_eq!(continued.boxes, [[50.0, 100.0, 170.0, 140.0]], "continuing re must not invent a second box");
}

#[test]
fn page_shapes_transforms_path_rectangles_and_preserves_rules() {
    let scaled = detection_test_shapes(b"q 2 0 0 2 0 0 cm 50 100 m 170 100 l 170 140 l 50 140 l h S Q 10 20 m 100 20 l S");
    assert_eq!(scaled.boxes, [[100.0, 200.0, 340.0, 280.0]]);
    assert_eq!(scaled.rules, [[10.0, 20.0, 100.0, 20.0]]);
    let rotated = detection_test_shapes(b"0 1 -1 0 300 0 cm 50 100 m 170 100 l 170 140 l 50 140 l h S");
    assert_eq!(rotated.boxes, [[160.0, 50.0, 200.0, 170.0]]);
}

#[test]
fn page_shapes_closes_only_last_stroked_subpath_but_all_filled_subpaths() {
    let paths = "50 100 m 170 100 l 170 140 l 50 140 l 200 100 m 320 100 l 320 140 l 200 140 l";
    let stroked = detection_test_shapes(format!("{paths} s").as_bytes());
    assert_eq!(stroked.boxes, [[200.0, 100.0, 320.0, 140.0]]);
    assert_eq!(stroked.rules, [[50.0, 100.0, 170.0, 100.0], [50.0, 140.0, 170.0, 140.0]]);
    let filled = detection_test_shapes(format!("{paths} f").as_bytes());
    assert_eq!(filled.boxes, [[50.0, 100.0, 170.0, 140.0], [200.0, 100.0, 320.0, 140.0]]);
    assert!(filled.rules.is_empty());
}

#[test]
fn auto_text_size_fits_single_and_multiline_text() {
    use crate::appearance::auto_text_size;
    use pdfcraft_fonts::{helvetica_width, wrap_with};
    let long = "Weekly progress and diligence: CoBank refi, IL and Redwood Falls";
    // Single line: the height's size for short text, shrunk to the width for long text.
    assert_eq!(auto_text_size("Hi", 94.0, 8.0, false, helvetica_width), 8.0 / 1.15);
    let s = auto_text_size(long, 194.0, 8.0, false, helvetica_width);
    assert!(s > 4.0 && s < 8.0 / 1.15 && helvetica_width(long, s) <= 194.0 + 1e-9, "{s}");
    // Never past 12 pt or under 4 pt.
    assert_eq!(auto_text_size("Hi", 500.0, 500.0, false, helvetica_width), 12.0);
    assert_eq!(auto_text_size(&long.repeat(20), 20.0, 8.0, false, helvetica_width), 4.0);
    // Multiline: the largest size whose wrapped lines fit the height.
    let m = auto_text_size(long, 114.0, 44.0, true, helvetica_width);
    assert!(wrap_with(long, m, 114.0, helvetica_width).len() as f64 * m * 1.15 <= 44.0, "{m}");
    assert!(wrap_with(long, m + 0.5, 114.0, helvetica_width).len() as f64 * (m + 0.5) * 1.15 > 44.0, "{m} is the largest that fits");
    // Hostile boxes and measurements don't panic or return nonsense.
    for (w, h) in [(f64::NAN, 10.0), (10.0, f64::INFINITY), (-5.0, -5.0), (0.0, 0.0)] {
        for multiline in [false, true] {
            let s = auto_text_size(long, w, h, multiline, helvetica_width);
            assert!((4.0..=12.0).contains(&s), "{w}×{h}: {s}");
        }
    }
    assert_eq!(auto_text_size(long, 50.0, 10.0, false, |_, _| f64::NAN), 10.0 / 1.15);
}

#[test]
fn document_scripts_read_each_name_tree_node_once() {
    // The root lists itself and the leaf twice each.
    let doc = document(&[
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 3 0 R >> >>".into(),
        "<< /Type /Pages /Kids [] /Count 0 >>".into(),
        "<< /Kids [3 0 R 4 0 R 3 0 R 4 0 R] >>".into(),
        "<< /Names [(init) 5 0 R] >>".into(),
        "<< /S /JavaScript /JS (var ready = true;) >>".into(),
    ]);
    assert_eq!(document_scripts_named(&doc), [("init".to_string(), "var ready = true;".to_string())]);
}
