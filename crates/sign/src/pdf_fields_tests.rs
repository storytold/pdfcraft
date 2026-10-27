use super::*;
use pdfcraft_testkit::Pdf;

/// A document whose form lists `fields`, with `objects` numbered from 3.
fn form(fields: &str, objects: &[String]) -> Document {
    let pdf = Pdf::new()
        .obj(format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{fields}] >> >>"))
        .obj("<< /Type /Pages /Kids [] /Count 0 >>")
        .objs(objects);
    Document::open(Arc::new(pdf.build())).unwrap()
}

/// Each name's first three characters and its length: enough to tell them apart without
/// printing thousands of bytes when an assertion fails.
fn short(fields: &[Field]) -> Vec<(String, usize)> {
    fields.iter().map(|f| (f.name.chars().take(3).collect(), f.name.len())).collect()
}

/// A form as writers make them: names join down the tree, a field takes its type from the nearest
/// field above that has one, and named signature fields are listed parents first. A field listed
/// twice, or reached again from below, counts once, and a kid `/Fields` lists as well keeps its
/// full name; a kid may be written in place.
#[test]
fn nested_fields_are_named_down_the_tree() {
    let doc = form(
        "3 0 R 6 0 R 9 0 R 11 0 R 3 0 R 4 0 R",
        &[
            "<< /T (parent) /FT /Sig /Kids [4 0 R 5 0 R] >>".into(),
            "<< /T (a) /Kids [12 0 R 3 0 R] >>".into(),
            "<< /T (b) /FT /Tx >>".into(),
            "<< /T (form) /Kids [7 0 R 8 0 R] >>".into(),
            "<< /T (sig) /FT /Sig /Type /Annot /Subtype /Widget /Rect [0 0 10 10] >>".into(),
            "<< /T (text) /FT /Tx >>".into(),
            "<< /T () /Kids [10 0 R] >>".into(),
            "<< /T (inner) /FT /Sig >>".into(),
            "<< /FT /Sig /Kids [<< /T (direct) >>] >>".into(),
            "<< /Type /Annot /Subtype /Widget /Rect [1 2 3 4] >>".into(),
        ],
    );
    let fields = sig_fields(&doc);
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["parent", "parent.a", "form.sig", "inner", "direct"]);
    let widgets: Vec<Vec<Option<ObjRef>>> = fields.iter().map(|f| f.widgets.iter().map(|(r, _)| *r).collect()).collect();
    assert_eq!(widgets, [vec![], vec![Some(ObjRef::new(12, 0))], vec![Some(ObjRef::new(7, 0))], vec![], vec![]]);
    assert!(walk_sig_fields(&doc).1, "a form within the caps is read whole");
}

/// A widget two fields list stays listed under both, whether each lists it in its own `/Kids` or
/// both list one shared array: the status layers count these claims to leave such a widget alone.
#[test]
fn a_widget_two_fields_list_is_listed_under_both() {
    let widget = || "<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] >>".to_string();
    let own = form("3 0 R 4 0 R", &["<< /T (a) /FT /Sig /Kids [5 0 R] >>".into(), "<< /T (b) /FT /Sig /Kids [5 0 R] >>".into(), widget()]);
    let shared =
        form("3 0 R 4 0 R", &["<< /T (a) /FT /Sig /Kids 6 0 R >>".into(), "<< /T (b) /FT /Sig /Kids 6 0 R >>".into(), widget(), "[5 0 R]".into()]);
    for (how, doc) in [("own arrays", own), ("one shared array", shared)] {
        let widgets: Vec<Vec<Option<ObjRef>>> = sig_fields(&doc).iter().map(|f| f.widgets.iter().map(|(r, _)| *r).collect()).collect();
        assert_eq!(widgets, [vec![Some(ObjRef::new(5, 0))], vec![Some(ObjRef::new(5, 0))]], "{how}");
    }
}

/// Every kid's name starts with its parent's. Copied into each kid, a 1 MB name over 100,000 kids
/// is 100 GB. A name over the cap is not read, and neither is anything under it.
///
/// The small form goes first: without the cap it fails on the names it lists, long before the
/// large one could use up the machine's memory.
#[test]
fn a_field_named_over_the_cap_is_skipped_with_its_kids() {
    for (len, kids) in [(64 << 10, 200), (1 << 20, 100_000)] {
        let doc = form(
            "3 0 R 5 0 R",
            &[
                format!("<< /T 4 0 R /FT /Sig /Kids [{}] >>", "<< /T (x) >> ".repeat(kids)),
                format!("({})", "n".repeat(len)),
                "<< /T (ok) /FT /Sig >>".into(),
            ],
        );
        let (fields, whole) = walk_sig_fields(&doc);
        let names: Vec<String> = fields.into_iter().map(|f| f.name).collect();
        let bytes: usize = names.iter().map(String::len).sum();
        assert!(names == ["ok"], "a {len}-byte name over {kids} kids: {} fields listed, {bytes} bytes of names", names.len());
        assert!(!whole, "the long name was passed over");
    }
}

/// The cap is on the whole name as read, dots included. A partial name stored in more bytes than
/// any name within the cap could take, even as UTF-16, is not read, whatever it trims to.
#[test]
fn names_up_to_the_cap_are_read_and_longer_ones_are_not() {
    let max = MAX_FIELD_NAME_BYTES;
    let utf16 = |n: usize| format!("<FEFF{}>", "0061".repeat(n));
    let doc = form(
        "3 0 R 4 0 R 5 0 R 8 0 R 9 0 R 10 0 R 11 0 R",
        &[
            format!("<< /T ({}) /FT /Sig >>", "a".repeat(max)),
            format!("<< /T ({}) /FT /Sig >>", "b".repeat(max + 1)),
            "<< /T (p) /FT /Sig /Kids [6 0 R 7 0 R] >>".into(),
            format!("<< /T ({}) >>", "c".repeat(max - 2)),
            format!("<< /T ({}) >>", "d".repeat(max - 1)),
            format!("<< /T {} /FT /Sig >>", utf16(max)),
            format!("<< /T {} /FT /Sig >>", utf16(max + 1)),
            format!("<< /T (x{}) /FT /Sig >>", " ".repeat(2 * max + 1)),
            format!("<< /T (y{}) /FT /Sig >>", " ".repeat(2 * max + 2)),
        ],
    );
    let (fields, whole) = walk_sig_fields(&doc);
    let expected = [("aaa", max), ("p", 1), ("p.c", max), ("aaa", max), ("x", 1)].map(|(s, n)| (s.to_string(), n));
    assert_eq!(short(&fields), expected);
    assert!(!whole, "fields passed over for their names");
}

/// Passed over for its name, a leaf of another type leaves the walk whole: it is no signature
/// field and has no kids to hold one, so an unsigned form with a text field named past the cap
/// stays unsigned. A field's own type comes before the one it takes from above.
#[test]
fn a_leaf_of_another_type_named_past_the_cap_leaves_the_walk_whole() {
    let long = "a".repeat(MAX_FIELD_NAME_BYTES + 1);
    let stored = format!("x{}", " ".repeat(2 * MAX_FIELD_NAME_BYTES + 2));
    for (what, field, whole) in [
        ("a text field", format!("<< /T ({long}) /FT /Tx >>"), true),
        ("a text field stored too long to read", format!("<< /T ({stored}) /FT /Tx >>"), true),
        ("a field of no type", format!("<< /T ({long}) >>"), true),
        ("a text field under a signature field", format!("<< /T (p) /FT /Sig /Kids [<< /T ({long}) /FT /Tx >>] >>"), true),
        ("a signature field", format!("<< /T ({long}) /FT /Sig >>"), false),
        ("a kid taking the signature type", format!("<< /T (p) /FT /Sig /Kids [<< /T ({long}) >>] >>"), false),
        ("a text field with a signature kid", format!("<< /T ({long}) /FT /Tx /Kids [<< /T (k) /FT /Sig >>] >>"), false),
    ] {
        assert_eq!(walk_sig_fields(&form("3 0 R", &[field])).1, whole, "{what}");
    }
    let doc = form("3 0 R", &[format!("<< /T ({long}) /FT /Tx >>")]);
    assert!(list(&doc, doc.bytes(), &TrustStore::default()).is_empty(), "no row for a field passed over");
}

/// No more than `MAX_FIELDS` fields are looked at: the first in `/Fields`, then kids as they are
/// reached, out of whatever room is left.
#[test]
fn the_walk_stops_after_max_fields() {
    let fields = |n: usize| (0..n).map(|i| format!("<< /T (f{i}) /FT /Sig >>")).collect::<String>();
    let (listed, whole) = walk_sig_fields(&form(&fields(MAX_FIELDS), &[]));
    assert_eq!((listed.len(), whole), (MAX_FIELDS, true));
    let (listed, whole) = walk_sig_fields(&form(&fields(MAX_FIELDS + 1), &[]));
    assert_eq!((listed.len(), whole), (MAX_FIELDS, false));
    assert_eq!(listed.last().map(|f| f.name.clone()), Some(format!("f{}", MAX_FIELDS - 1)));
    // The first field's kids take the room that is left, so the second field's kid is not reached.
    let kids = (0..MAX_FIELDS).map(|i| format!("<< /T (k{i}) >>")).collect::<String>();
    let (listed, whole) = walk_sig_fields(&form(
        "3 0 R 4 0 R",
        &[format!("<< /T (p) /FT /Sig /Kids [{kids}] >>"), "<< /T (q) /FT /Sig /Kids [<< /T (k) >>] >>".into()],
    ));
    assert_eq!((listed.len(), whole), (MAX_FIELDS, false));
    assert_eq!(listed.last().map(|f| f.name.clone()), Some("q".to_string()));
}

/// No more than `MAX_ENTRIES` entries of `/Fields` and the `/Kids` arrays are read, all together.
#[test]
fn the_walk_reads_no_more_than_max_entries() {
    // The field is one entry of /Fields; its widgets take the rest.
    let field = |widgets: usize| form(&format!("<< /T (p) /FT /Sig /Kids [{}] >>", "<< /Subtype /Widget >> ".repeat(widgets)), &[]);
    let (listed, whole) = walk_sig_fields(&field(MAX_ENTRIES - 1));
    assert_eq!((listed[0].widgets.len(), whole), (MAX_ENTRIES - 1, true));
    let (listed, whole) = walk_sig_fields(&field(MAX_ENTRIES));
    assert_eq!((listed[0].widgets.len(), whole), (MAX_ENTRIES - 1, false));
}

/// A `/Kids` array shared by two fields on every level doubles the paths through the tree each
/// level down: 2^41 fields for 40 levels. A field written in place in a shared array is visited
/// once, under the first field that reaches it.
#[test]
fn a_kids_array_shared_by_many_fields_is_walked_once() {
    for levels in [16, 40] {
        let level = |i: usize| {
            if i + 1 < levels {
                format!("[<< /T (a) /Kids {n} 0 R >> << /T (b) /Kids {n} 0 R >>]", n = i + 4)
            } else {
                "[<< /T (a) >> << /T (b) >>]".to_string()
            }
        };
        let doc = form("<< /T (r) /FT /Sig /Kids 3 0 R >>", &(0..levels).map(level).collect::<Vec<_>>());
        assert_eq!(sig_fields(&doc).len(), 2 * levels + 1, "{levels} levels");
    }
}

/// A `/Fields` array that a field also lists as its `/Kids` leads back to itself, one name longer
/// each time round. Each field in it is listed once.
#[test]
fn a_fields_array_listed_again_as_kids_is_walked_once() {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields 3 0 R >> >>")
        .obj("<< /Type /Pages /Kids [] /Count 0 >>")
        .obj("[<< /T (a) /FT /Sig /Kids 3 0 R >>]");
    let doc = Document::open(Arc::new(pdf.build())).unwrap();
    let names: Vec<String> = sig_fields(&doc).into_iter().map(|f| f.name).collect();
    assert_eq!(names, ["a"]);
}

/// A `/Kids` array every field lists is read under each of them, which the entry cap bounds:
/// 20,000 fields over one array of 1,000 widgets would be 20 million entries.
///
/// The small form goes first, as above.
#[test]
fn a_kids_array_many_fields_list_is_read_within_the_cap() {
    for (fields, widgets) in [(200, 1_000), (MAX_FIELDS, 1_000)] {
        let listing = (0..fields).map(|i| format!("<< /T (f{i}) /FT /Sig /Kids 3 0 R >>")).collect::<String>();
        let array = format!("[{}]", (0..widgets).map(|i| format!("{} 0 R ", i + 4)).collect::<String>());
        let objects: Vec<String> = std::iter::once(array).chain((0..widgets).map(|_| "<< /Subtype /Widget >>".to_string())).collect();
        let (listed, whole) = walk_sig_fields(&form(&listing, &objects));
        let read = listed.iter().map(|f| f.widgets.len()).sum::<usize>() + fields;
        assert!(read <= MAX_ENTRIES && !whole, "{fields} fields over {widgets} widgets: {read} entries read");
    }
}

/// A widget every field lists is listed under each as one copy: copied into each, one 1 MB widget
/// under 20,000 fields is 20 GB.
///
/// The small form goes first, as above.
#[test]
fn a_widget_many_fields_list_is_copied_once() {
    for (len, fields) in [(64 << 10, 1_000), (1 << 20, MAX_FIELDS)] {
        let listing = (0..fields).map(|i| format!("<< /T (f{i}) /FT /Sig /Kids [3 0 R] >>")).collect::<String>();
        let doc = form(&listing, &[format!("<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] /Junk ({}) >>", "j".repeat(len))]);
        let listed = sig_fields(&doc);
        assert_eq!(listed.len(), fields);
        let first = &listed[0].widgets[0].1;
        let shared = listed.iter().all(|f| f.widgets.len() == 1 && Rc::ptr_eq(&f.widgets[0].1, first));
        assert!(shared, "a {len}-byte widget under {fields} fields");
    }
}

/// So is a widget written in place in a `/Kids` array many fields list: its place in the array
/// stands for it, as a reference would.
#[test]
fn a_widget_written_into_a_shared_kids_array_is_copied_once() {
    let listing = (0..1_000).map(|i| format!("<< /T (f{i}) /FT /Sig /Kids 3 0 R >>")).collect::<String>();
    let listed = sig_fields(&form(&listing, &["[<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] >>]".into()]));
    assert_eq!(listed.len(), 1_000);
    let first = &listed[0].widgets[0].1;
    assert!(listed.iter().all(|f| f.widgets.len() == 1 && Rc::ptr_eq(&f.widgets[0].1, first)), "an in-place widget is copied once");
}

/// A one-page form listing the signature field `s`, then `text` text fields.
fn form_with_text_fields(text: usize) -> Document {
    let fields: String = (0..text).map(|i| format!("<< /T (t{i}) /FT /Tx >>")).collect();
    let pdf = Pdf::new()
        .obj(format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R {fields}] >> >>"))
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [4 0 R] >>")
        .obj("<< /Type /Annot /Subtype /Widget /FT /Sig /T (s) /Rect [10 10 110 60] /P 3 0 R >>");
    Document::open(Arc::new(pdf.build())).unwrap()
}

/// The test digital ID, and options that sign a new field.
fn signer() -> (DigitalId, SignOptions) {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ec-p256.p12")).unwrap();
    (crate::pkcs12::open(&p12, "test").unwrap(), SignOptions { date: "D:20261027120000Z".into(), ..SignOptions::default() })
}

/// Signing adds a field only where the walk reads it back, the whole form with it: past a cap the
/// new signature would be listed as unread fields, and its name could repeat an unread field's. A
/// field the walk already reads is still signed in a form it can't read whole.
#[test]
fn a_new_signature_field_is_added_only_where_the_walk_reads_it_back() {
    let (id, opts) = signer();
    // With `s` before them, the new field is the last one the walk takes.
    let signed = sign(&form_with_text_fields(MAX_FIELDS - 2), &id, &opts).unwrap();
    let (fields, whole) = walk_sig_fields(&Document::open(Arc::new(signed)).unwrap());
    assert!(whole && fields.iter().any(|f| f.name == "Signature1"), "the new field is read back");
    let refused = sign(&form_with_text_fields(MAX_FIELDS - 1), &id, &opts);
    assert!(matches!(&refused, Err(SignError::Pdf(m)) if m.contains("too many fields")), "{:?}", refused.as_ref().err());
    let existing = SignOptions { field: Some("s".into()), ..opts };
    assert!(sign(&form_with_text_fields(MAX_FIELDS), &id, &existing).is_ok(), "a field the walk reads is signed as before");
}

/// The walk lists no field without a name, so a new signature field named only with spaces or NULs
/// would be a signature nothing shows: signing refuses it.
#[test]
fn a_new_signature_field_with_a_blank_name_is_refused() {
    let (id, opts) = signer();
    for name in ["", " ", "\0"] {
        let blank = SignOptions { new_field_name: Some(name.into()), ..opts.clone() };
        let refused = sign(&form_with_text_fields(0), &id, &blank);
        assert!(matches!(&refused, Err(SignError::Pdf(m)) if m.contains("blank")), "{name:?}: {:?}", refused.as_ref().err());
    }
    let named = SignOptions { new_field_name: Some(" Approval ".into()), ..opts };
    assert!(sign(&form_with_text_fields(0), &id, &named).is_ok(), "a name with spaces around it is read");
}

/// Every kid's name repeats its parent's, so the names the walk keeps are capped all together: a
/// long name over a few thousand kids stops the walk before it adds up.
#[test]
fn the_names_kept_stop_at_max_name_bytes() {
    let parent = "p".repeat(MAX_FIELD_NAME_BYTES - 8);
    let form_of = |kids: usize| {
        let kids = (0..kids).map(|i| format!("<< /T (k{i}) >>")).collect::<String>();
        form("3 0 R", &[format!("<< /T ({parent}) /FT /Sig /Kids [{kids}] >>")])
    };
    // Each kid's name is the parent's and a few bytes more, just under the per-name cap.
    let fits = MAX_NAME_BYTES / MAX_FIELD_NAME_BYTES - 1;
    let (listed, whole) = walk_sig_fields(&form_of(fits));
    assert_eq!((listed.len(), whole), (fits + 1, true));
    let (listed, whole) = walk_sig_fields(&form_of(2 * fits));
    let kept: usize = listed.iter().map(|f| f.name.len()).sum();
    assert!(!whole && kept <= MAX_NAME_BYTES, "{} fields, {kept} bytes of names", listed.len());
}

/// A form whose only signature field, signed, is named one byte past the cap, and the file's
/// bytes. `list` needs both.
fn signed_field_past_the_cap() -> (Document, Vec<u8>) {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R] >> >>")
        .obj("<< /Type /Pages /Kids [] /Count 0 >>")
        .obj(format!("<< /T ({}) /FT /Sig /V 4 0 R >>", "a".repeat(MAX_FIELD_NAME_BYTES + 1)))
        .obj("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 1 2 1] /Contents <00> >>")
        .build();
    (Document::open(Arc::new(pdf.clone())).unwrap(), pdf)
}

/// A field the walk did not read may be signed, so a walk cut short is listed as one more row,
/// signed and of unknown validity: the document still counts as signed.
#[test]
fn a_walk_cut_short_is_listed_as_a_signed_row_of_unknown_validity() {
    let (doc, bytes) = signed_field_past_the_cap();
    let rows: Vec<_> = list(&doc, &bytes, &TrustStore::default()).into_iter().map(|r| (r.field, r.signed, r.status)).collect();
    assert_eq!(rows, [("UnreadFields".to_string(), true, Status::Unknown)]);
}

/// The row says why it is there, in a sentence an interface can translate.
#[test]
fn the_row_for_a_walk_cut_short_says_why() {
    let (doc, bytes) = signed_field_past_the_cap();
    let rows = list(&doc, &bytes, &TrustStore::default());
    let said: Vec<&str> = rows.iter().flat_map(|r| &r.details).map(|d| d.message.as_str()).collect();
    assert!(matches!(said.as_slice(), [s] if s.starts_with("This form has too many fields")), "{said:?}");
}

/// A walk that reads the whole form adds no such row: a name at the cap is read.
#[test]
fn a_whole_walk_adds_no_row_for_unread_fields() {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R] >> >>")
        .obj("<< /Type /Pages /Kids [] /Count 0 >>")
        .obj(format!("<< /T ({}) /FT /Sig >>", "a".repeat(MAX_FIELD_NAME_BYTES)))
        .build();
    let doc = Document::open(Arc::new(pdf.clone())).unwrap();
    let rows: Vec<_> = list(&doc, &pdf, &TrustStore::default()).into_iter().map(|r| (r.field.len(), r.signed)).collect();
    assert_eq!(rows, [(MAX_FIELD_NAME_BYTES, false)]);
}

/// One rule decides whether a signature widget is on its page, for the badge and for the redrawn
/// layers alike: a box wider and taller than half a point, in either corner order, of finite
/// numbers, on a widget that isn't Hidden or NoView.
#[test]
fn a_widget_shows_with_a_box_over_half_a_point_unless_hidden_or_noview() {
    let doc = form("", &[]);
    let widget = |s: &str| pdfcraft_cos::Lexer::new(s.as_bytes(), 0).object().unwrap().as_dict().unwrap().clone();
    let shown = |s: &str| widget_shown(&doc, &widget(s));
    assert!(shown("<< /Rect [100 100 300 220] >>"));
    assert!(shown("<< /Rect [300 220 100 100] >>"), "corners in either order");
    assert!(shown("<< /Rect [0 0 0.6 0.6] /F 4 >>"), "Print alone hides nothing");
    assert!(!shown("<< /Rect [100 100 300 220] /F 2 >>"), "Hidden");
    assert!(!shown("<< /Rect [100 100 300 220] /F 36 >>"), "NoView");
    assert!(!shown("<< /Rect [0 0 0.5 10] >>"), "half a point wide is no box");
    assert!(!shown("<< /Rect [0 0 10] >>"));
    assert!(!shown("<< /F 4 >>"));
    let mut endless = widget("<< >>");
    endless.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(f64::INFINITY), Object::Int(10)]));
    assert!(!widget_shown(&doc, &endless), "a box needs finite corners");
}
