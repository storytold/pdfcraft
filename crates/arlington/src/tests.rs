//! Synthetic documents built in code, each departing from the model in one way.

use super::*;
use proptest::prelude::*;

fn name(s: &str) -> Object {
    Object::Name(s.as_bytes().to_vec())
}

fn r(num: u32) -> Object {
    Object::Ref(Ref { num, generation: 0 })
}

fn text(s: &str) -> Object {
    Object::String { bytes: s.as_bytes().to_vec(), hex: false }
}

fn ints(v: &[i64]) -> Object {
    Object::Array(v.iter().map(|i| Object::Int(*i)).collect())
}

fn entries(e: &[(&str, Object)]) -> Vec<(Vec<u8>, Object)> {
    e.iter().map(|(k, v)| (k.as_bytes().to_vec(), v.clone())).collect()
}

fn dict(e: &[(&str, Object)]) -> Object {
    Object::Dict(entries(e))
}

/// Catalog 1, page tree 2, one page 3: a document the model has nothing to say about.
fn minimal() -> MemSource {
    let mut doc = MemSource { version: Version::parse("1.7"), pages: vec![Ref { num: 3, generation: 0 }], ..Default::default() };
    doc.trailer = entries(&[("Size", Object::Int(4)), ("Root", r(1))]);
    doc.objects.insert(1, dict(&[("Type", name("Catalog")), ("Pages", r(2))]));
    doc.objects.insert(2, dict(&[("Type", name("Pages")), ("Kids", Object::Array(vec![r(3)])), ("Count", Object::Int(1))]));
    doc.objects.insert(3, page(&[]));
    doc
}

fn page(extra: &[(&str, Object)]) -> Object {
    let mut e = entries(&[("Type", name("Page")), ("Parent", r(2)), ("MediaBox", ints(&[0, 0, 612, 792])), ("Resources", dict(&[]))]);
    for (k, v) in extra {
        e.retain(|(n, _)| n.as_slice() != k.as_bytes());
        if !matches!(v, Object::Null) {
            e.push((k.as_bytes().to_vec(), v.clone()));
        }
    }
    Object::Dict(e)
}

/// Replace (or with `Null`, remove) entries of object `num`.
fn set(doc: &mut MemSource, num: u32, changes: &[(&str, Object)]) {
    let Some(Object::Dict(e)) = doc.objects.get_mut(&num) else { panic!("object {num} is a dictionary") };
    for (k, v) in changes {
        e.retain(|(n, _)| n.as_slice() != k.as_bytes());
        if !matches!(v, Object::Null) {
            e.push((k.as_bytes().to_vec(), v.clone()));
        }
    }
}

fn check(doc: &MemSource) -> Report {
    validate(doc, &Options::default())
}

fn find<'a>(report: &'a Report, rule: Rule, key: &str) -> Option<&'a Finding> {
    report.findings.iter().find(|f| f.rule == rule && f.key == key)
}

fn problems(report: &Report) -> Vec<String> {
    report.findings.iter().filter(|f| f.severity != Severity::Info).map(|f| format!("{}: {}", f.path, f.message())).collect()
}

#[test]
fn a_minimal_document_has_no_problems() {
    let report = check(&minimal());
    assert_eq!(problems(&report), Vec::<String>::new());
    assert_eq!(report.version, Version(17));
    assert!(report.is_valid());
    // Trailer, catalog, page tree, page, its resources and its media box at least.
    assert!(report.objects >= 5, "{}", report.objects);
}

#[test]
fn a_missing_required_key_is_an_error() {
    let mut doc = minimal();
    set(&mut doc, 1, &[("Pages", Object::Null)]);
    let report = check(&doc);
    let f = find(&report, Rule::Required, "Pages").expect("Pages is required");
    assert_eq!(f.severity, Severity::Error);
    assert_eq!(f.type_name, "Catalog");
    assert_eq!(f.object, Some(Ref { num: 1, generation: 0 }));
    assert_eq!(f.message(), "Catalog lacks the required entry Pages");
    assert!(!report.is_valid());
}

#[test]
fn a_wrong_type_is_an_error_on_its_page() {
    let mut doc = minimal();
    doc.objects.insert(3, page(&[("Rotate", name("Ninety"))]));
    let report = check(&doc);
    let f = find(&report, Rule::Type, "Rotate").expect("Rotate must be an integer");
    assert_eq!(f.page, Some(0));
    assert_eq!(f.message(), "Rotate in PageObject is name, but must be integer");
}

#[test]
fn values_outside_the_list_and_rules_are_errors() {
    let mut doc = minimal();
    set(&mut doc, 1, &[("PageLayout", name("Diagonal"))]);
    doc.objects.insert(3, page(&[("Rotate", Object::Int(45))]));
    let report = check(&doc);
    let layout = find(&report, Rule::Value, "PageLayout").expect("PageLayout has a fixed set of values");
    assert!(layout.message().contains("/Diagonal") && layout.message().contains("/SinglePage"), "{}", layout.message());
    let rotate = find(&report, Rule::Value, "Rotate").expect("Rotate is a multiple of 90");
    assert!(rotate.message().contains("(@Rotate mod 90)==0"), "{}", rotate.message());
}

#[test]
fn inherited_keys_satisfy_the_requirement() {
    let mut doc = minimal();
    // The media box on the page tree node instead of the page (ISO 32000-2 §7.7.3.4).
    set(&mut doc, 2, &[("MediaBox", ints(&[0, 0, 100, 100]))]);
    doc.objects.insert(3, page(&[("MediaBox", Object::Null)]));
    assert_eq!(problems(&check(&doc)), Vec::<String>::new());
    // Nowhere at all is an error.
    set(&mut doc, 2, &[("MediaBox", Object::Null)]);
    assert!(find(&check(&doc), Rule::Required, "MediaBox").is_some());
}

#[test]
fn indirect_and_direct_requirements() {
    let mut doc = minimal();
    // The page tree must be an indirect object; the trailer's /Size must be direct.
    let tree = doc.objects.get(&2).cloned().unwrap();
    set(&mut doc, 1, &[("Pages", tree)]);
    doc.objects.insert(9, Object::Int(4));
    doc.trailer = entries(&[("Size", r(9)), ("Root", r(1))]);
    let report = check(&doc);
    assert_eq!(find(&report, Rule::Indirect, "Pages").map(|f| f.severity), Some(Severity::Error));
    assert_eq!(find(&report, Rule::Direct, "Size").map(|f| f.message()), Some("Size in FileTrailer must be a direct object, not a reference".into()));
}

#[test]
fn newer_and_deprecated_keys_are_warnings() {
    let mut doc = minimal();
    doc.version = Version::parse("1.3");
    set(&mut doc, 1, &[("Lang", text("en"))]);
    doc.objects.insert(3, page(&[("Hid", Object::Bool(false))]));
    let report = check(&doc);
    let lang = find(&report, Rule::Version, "Lang").expect("Lang came in PDF 1.4");
    assert_eq!(lang.severity, Severity::Warning);
    assert_eq!(lang.message(), "Lang in Catalog is from PDF 1.4, later than the file's PDF 1.3");
    let hid = find(&report, Rule::Deprecated, "Hid").expect("Hid was deprecated in PDF 1.3");
    assert_eq!(hid.message(), "Hid in PageObject is deprecated since PDF 1.3");
    assert!(report.is_valid(), "warnings only");
}

/// What a newer key leads to is as new as the key, so its own keys aren't reported again: in a
/// PDF 1.3 file the catalog's /Metadata is from PDF 1.4, and every key of the metadata stream is
/// too ("Length in Metadata is from PDF 1.4" would say nothing more). A key newer still is
/// reported. pdf.js devicen.pdf, issue12706.pdf, bug1708040.pdf.
#[test]
fn keys_inside_a_newer_object_are_not_reported_again() {
    let mut doc = minimal();
    doc.version = Version::parse("1.3");
    let metadata = |extra: &[(&str, Object)]| {
        let mut e = vec![("Type", name("Metadata")), ("Subtype", name("XML")), ("Length", Object::Int(0))];
        e.extend(extra.iter().cloned());
        Object::Stream(entries(&e))
    };
    doc.objects.insert(4, metadata(&[]));
    set(&mut doc, 1, &[("Metadata", r(4))]);
    let report = check(&doc);
    assert_eq!(problems(&report), vec!["trailer/Root: Metadata in Catalog is from PDF 1.4, later than the file's PDF 1.3".to_string()]);
    doc.objects.insert(4, metadata(&[("DL", Object::Int(0))]));
    let report = check(&doc);
    assert_eq!(report.warnings, 2, "{:?}", problems(&report));
    assert_eq!(find(&report, Rule::Version, "DL").map(|f| f.message()), Some("DL in Metadata is from PDF 1.5, later than the file's PDF 1.3".into()));
}

#[test]
fn the_catalog_version_overrides_an_older_header() {
    let mut doc = minimal();
    doc.version = Version::parse("1.3");
    set(&mut doc, 1, &[("Version", name("1.7")), ("Lang", text("en"))]);
    let report = check(&doc);
    assert_eq!(report.version, Version(17));
    assert!(find(&report, Rule::Version, "Lang").is_none());
    doc.version = None;
    set(&mut doc, 1, &[("Version", Object::Null)]);
    let report = check(&doc);
    assert!(report.version_assumed);
    assert_eq!(report.version, Version::V2_0);
}

#[test]
fn references_to_nothing_are_reported() {
    let mut doc = minimal();
    set(&mut doc, 1, &[("Pages", r(99))]);
    let report = check(&doc);
    let missing = find(&report, Rule::MissingObject, "Pages").expect("dangling reference");
    assert_eq!(missing.message(), "Pages in Catalog refers to object 99 0 R, which doesn't exist");
    assert!(find(&report, Rule::Required, "Pages").is_some(), "a reference to nothing is no page tree");
}

#[test]
fn unknown_keys_are_information_and_can_be_turned_off() {
    let mut doc = minimal();
    set(&mut doc, 1, &[("PdfCraftPrivate", Object::Int(1))]);
    let report = check(&doc);
    let f = find(&report, Rule::UnknownKey, "PdfCraftPrivate").expect("not in the model");
    assert_eq!(f.severity, Severity::Info);
    assert!(report.is_valid());
    let quiet = validate(&doc, &Options { unknown_keys: false, ..Options::default() });
    assert_eq!(quiet.infos, 0);
}

/// The annotation's type comes from its /Subtype, and its action's from /S.
#[test]
fn linked_types_are_chosen_by_their_fixed_values() {
    let mut doc = minimal();
    doc.objects.insert(3, page(&[("Annots", Object::Array(vec![r(4)]))]));
    let link = |s: &str| {
        dict(&[
            ("Type", name("Annot")),
            ("Subtype", name("Link")),
            ("Rect", ints(&[10, 20, 110, 40])),
            ("A", dict(&[("S", name(s)), ("URI", text("https://example.org"))])),
        ])
    };
    doc.objects.insert(4, link("URI"));
    assert_eq!(problems(&check(&doc)), Vec::<String>::new());
    // An action no action type has: reported once, not checked against a wrong type.
    doc.objects.insert(4, link("Teleport"));
    let report = check(&doc);
    let f = find(&report, Rule::UnknownType, "A").expect("no action type fits /S /Teleport");
    assert_eq!(f.type_name, "AnnotLink");
    assert_eq!(f.rect, Some([10.0, 20.0, 110.0, 40.0]));
    assert_eq!(f.page, Some(0));
    assert_eq!(problems(&report).len(), 1, "{:?}", problems(&report));
    // A link without its rectangle.
    doc.objects.insert(4, dict(&[("Subtype", name("Link"))]));
    assert!(find(&check(&doc), Rule::Required, "Rect").is_some());
    // An optional key out of range doesn't make the link something else: its value is wrong.
    doc.objects.insert(4, dict(&[("Subtype", name("Link")), ("Rect", ints(&[0, 0, 1, 1])), ("H", name("Glow"))]));
    let report = check(&doc);
    assert!(find(&report, Rule::Value, "H").is_some(), "{:?}", problems(&report));
    assert!(find(&report, Rule::UnknownType, "0").is_none(), "{:?}", problems(&report));
}

#[test]
fn trees_are_walked_to_their_leaves() {
    let mut doc = minimal();
    let labels = dict(&[("Nums", Object::Array(vec![Object::Int(0), dict(&[("S", name("D"))]), Object::Int(2), r(5)]))]);
    set(&mut doc, 1, &[("PageLabels", labels)]);
    doc.objects.insert(5, dict(&[("S", name("Roman")), ("St", Object::Int(0))]));
    let report = check(&doc);
    let style = find(&report, Rule::Value, "S").expect("PageLabel /S is one of D, R, r, A, a");
    assert_eq!(style.object, Some(Ref { num: 5, generation: 0 }));
    assert!(style.path.ends_with("PageLabels/Nums/2"), "{}", style.path);
    assert!(find(&report, Rule::Value, "St").is_some(), "St is at least 1");
    // Kids lead to more nodes; an odd Nums array is reported.
    let kid = dict(&[("Nums", Object::Array(vec![Object::Int(0)]))]);
    doc.objects.insert(6, kid);
    set(&mut doc, 1, &[("PageLabels", dict(&[("Kids", Object::Array(vec![r(6)]))]))]);
    assert!(find(&check(&doc), Rule::ArrayLength, "PageLabels").is_some());
}

#[test]
fn widgets_merged_with_their_field_are_not_unknown_keys() {
    let mut doc = minimal();
    doc.objects.insert(3, page(&[("Annots", Object::Array(vec![r(4)]))]));
    doc.objects.insert(
        4,
        dict(&[
            ("Type", name("Annot")),
            ("Subtype", name("Widget")),
            ("Rect", ints(&[0, 0, 10, 10])),
            ("FT", name("Tx")),
            ("T", text("name")),
            ("DA", text("/Helv 0 Tf 0 g")),
            ("P", r(3)),
        ]),
    );
    set(&mut doc, 1, &[("AcroForm", dict(&[("Fields", Object::Array(vec![r(4)]))]))]);
    let report = check(&doc);
    assert!(find(&report, Rule::UnknownKey, "FT").is_none(), "{:?}", report.findings);
    assert!(find(&report, Rule::UnknownKey, "Rect").is_none(), "{:?}", report.findings);
}

/// A text field merged with its widget (pdf.js issue21068.pdf): its field keys (/FT, /DA, /Q,
/// /MaxLen) are unknown to AnnotWidget, so a candidate its /Subtype rules out may score higher.
/// The widget is the only candidate the object doesn't contradict, so it is the type.
#[test]
fn a_contradicted_candidate_never_wins_on_score() {
    let mut doc = minimal();
    doc.version = Version::parse("1.6");
    doc.objects.insert(3, page(&[("Annots", Object::Array(vec![r(4)]))]));
    doc.objects.insert(
        4,
        dict(&[
            ("AP", dict(&[("N", r(5))])),
            ("DA", text("/Helv 12 Tf 0 g")),
            ("F", Object::Int(4)),
            ("FT", name("Tx")),
            ("Ff", Object::Int(29_360_128)),
            ("MK", dict(&[("BC", ints(&[0, 0, 0]))])),
            ("MaxLen", Object::Int(10)),
            ("P", r(3)),
            ("Q", Object::Int(2)),
            ("Rect", ints(&[205, 623, 355, 645])),
            ("Subtype", name("Widget")),
            ("T", text("Text3")),
            ("Type", name("Annot")),
        ]),
    );
    doc.objects.insert(
        5,
        Object::Stream(entries(&[
            ("BBox", ints(&[0, 0, 150, 22])),
            ("Length", Object::Int(0)),
            ("Resources", dict(&[])),
            ("Subtype", name("Form")),
            ("Type", name("XObject")),
        ])),
    );
    set(&mut doc, 1, &[("AcroForm", dict(&[("Fields", Object::Array(vec![r(4)])), ("DA", text("/Helv 0 Tf 0 g"))]))]);
    let report = check(&doc);
    assert_eq!(problems(&report), Vec::<String>::new());
}

/// A text field's default appearance comes from the form's document-wide /DA when no field up
/// its /Parent chain has one (ISO 32000-2, the interactive form dictionary's DA; pdf.js
/// resetform_parent.pdf and bug1922766.pdf).
#[test]
fn a_fields_appearance_string_may_come_from_the_form() {
    let mut doc = minimal();
    doc.objects.insert(3, page(&[("Annots", Object::Array(vec![r(5)]))]));
    doc.objects.insert(4, dict(&[("FT", name("Tx")), ("T", text("a")), ("Kids", Object::Array(vec![r(5)]))]));
    doc.objects.insert(5, dict(&[("Subtype", name("Widget")), ("Rect", ints(&[0, 0, 10, 10])), ("Parent", r(4)), ("P", r(3))]));
    let form = |da: bool| {
        let mut e = vec![("Fields", Object::Array(vec![r(4)]))];
        if da {
            e.push(("DA", text("/Helv 0 Tf 0 g")));
        }
        dict(&e)
    };
    set(&mut doc, 1, &[("AcroForm", form(true))]);
    assert_eq!(problems(&check(&doc)), Vec::<String>::new());
    // Nowhere at all is an error.
    set(&mut doc, 1, &[("AcroForm", form(false))]);
    assert!(find(&check(&doc), Rule::Required, "DA").is_some());
}

/// Each attribute object of a structure element may be followed by a revision number, and each
/// class name too; neither has to be (`0*` required, `1*` not; ISO 32000-2 §14.7.2, the
/// structure element's /A and /C). pdf.js 160F-2019.pdf, issue21346.pdf, bug1997343.pdf.
#[test]
fn optional_members_of_a_repeating_set_may_be_left_out() {
    let mut doc = minimal();
    doc.objects.insert(4, dict(&[("Type", name("StructTreeRoot")), ("K", r(5))]));
    let attributes = Object::Array(vec![dict(&[("O", name("Layout"))]), dict(&[("O", name("List"))]), Object::Int(0)]);
    let classes = Object::Array(vec![name("Cls1"), name("Cls2"), Object::Int(1)]);
    doc.objects.insert(5, dict(&[("S", name("P")), ("P", r(4)), ("A", attributes), ("C", classes)]));
    set(&mut doc, 1, &[("StructTreeRoot", r(4))]);
    assert_eq!(problems(&check(&doc)), Vec::<String>::new());
    // A revision number where an attribute object must be is still wrong.
    set(&mut doc, 5, &[("A", Object::Array(vec![Object::Int(0), dict(&[("O", name("Layout"))])]))]);
    let report = check(&doc);
    assert_eq!(problems(&report).len(), 1, "{:?}", problems(&report));
    assert!(find(&report, Rule::Type, "0").is_some(), "{:?}", problems(&report));
}

#[test]
fn cycles_end() {
    let mut doc = minimal();
    // The page tree lists itself among its kids, and the page is its own parent's kid twice.
    set(&mut doc, 2, &[("Kids", Object::Array(vec![r(3), r(2), r(3)]))]);
    set(&mut doc, 3, &[("Parent", r(3))]);
    let report = check(&doc);
    assert!(!report.stopped_early);
    assert!(report.objects < 50, "{}", report.objects);
}

#[test]
fn rules_the_validator_cannot_evaluate_are_counted_not_guessed() {
    let report = check(&minimal());
    // A page's /TemplateInstantiated is required when it is in a name tree (fn:InNameTree).
    assert!(report.not_checked.iter().any(|(n, c)| n == "InNameTree" && *c >= 1), "{:?}", report.not_checked);
}

#[test]
fn limits_stop_the_walk_and_keep_the_totals() {
    let mut doc = minimal();
    let kids: Vec<Object> = (10..60).map(r).collect();
    for n in 10..60 {
        // Fifty pages without a media box, anywhere.
        doc.objects.insert(n, dict(&[("Type", name("Page")), ("Parent", r(2)), ("Resources", dict(&[]))]));
    }
    set(&mut doc, 2, &[("Kids", Object::Array(kids)), ("Count", Object::Int(50))]);
    let report = validate(&doc, &Options { max_findings: 3, ..Options::default() });
    assert_eq!(report.errors, 50);
    assert_eq!(report.findings.iter().filter(|f| f.severity == Severity::Error).count(), 3);
    assert!(report.truncated);
    let report = validate(&doc, &Options { max_objects: 10, ..Options::default() });
    assert!(report.stopped_early);
    assert!(report.objects <= 10);
}

/// A source that counts the indirect objects visited (each is asked for its page once), to see
/// where a walk stopped. Loads don't tell: a page tree node loads all its kids to type them.
struct Counted<'a>(&'a MemSource, std::cell::Cell<usize>);

impl Source for Counted<'_> {
    fn trailer(&self) -> Object {
        self.0.trailer()
    }
    fn get(&self, r: Ref) -> Option<Object> {
        self.0.get(r)
    }
    fn header_version(&self) -> Option<Version> {
        self.0.header_version()
    }
    fn page_index(&self, r: Ref) -> Option<usize> {
        self.1.set(self.1.get() + 1);
        self.0.page_index(r)
    }
}

/// A worker reports how far it is, and the caller can stop it: the walk ends at the first
/// `false`, without a report.
#[test]
fn progress_is_reported_and_stops_the_walk() {
    let mut doc = minimal();
    let kids: Vec<Object> = (10..5010).map(r).collect();
    for n in 10..5010 {
        doc.objects.insert(n, page(&[]));
    }
    set(&mut doc, 2, &[("Kids", Object::Array(kids)), ("Count", Object::Int(5000))]);
    let whole = check(&doc);

    let mut said = Vec::new();
    let same = validate_with_progress(&doc, &Options::default(), &mut |n| {
        said.push(n);
        true
    });
    assert_eq!(same.as_ref(), Some(&whole), "progress changes nothing");
    assert!(said.len() > 5, "progress is said along the way: {said:?}");
    assert!(said.windows(2).all(|w| w[0] < w[1]) && said.last() < Some(&whole.objects), "{said:?} of {}", whole.objects);

    let counted = Counted(&doc, std::cell::Cell::new(0));
    let mut asked = 0;
    let stopped = validate_with_progress(&counted, &Options::default(), &mut |_| {
        asked += 1;
        false
    });
    assert_eq!((stopped, asked), (None, 1), "the first `false` stops it");
    assert!(counted.1.get() < 1000, "the rest of the 5,000 pages isn't visited: {}", counted.1.get());
    let whole_walk = Counted(&doc, std::cell::Cell::new(0));
    validate(&whole_walk, &Options::default());
    assert!(whole_walk.1.get() > 5000, "a whole walk visits every page: {}", whole_walk.1.get());
}

#[test]
fn deep_direct_nesting_is_bounded() {
    let mut doc = minimal();
    // An outline item whose /Next chain is 300 direct dictionaries deep.
    let mut item = dict(&[("Title", text("x"))]);
    for _ in 0..300 {
        item = dict(&[("Title", text("x")), ("Next", item)]);
    }
    set(&mut doc, 1, &[("Outlines", dict(&[("First", item)]))]);
    let report = check(&doc);
    assert!(report.not_checked.iter().any(|(n, _)| n == "nesting past the limit"), "{:?}", report.not_checked);
}

#[test]
fn findings_say_their_words_through_a_translator() {
    let mut doc = minimal();
    set(&mut doc, 1, &[("Pages", Object::Null)]);
    let report = check(&doc);
    let f = find(&report, Rule::Required, "Pages").unwrap();
    assert!(WORDINGS.contains(&f.wording()));
    let spanish = |w: &str| {
        if w == "{type} lacks the required entry {key}" { "A {type} le falta la entrada obligatoria {key}".to_string() } else { w.to_string() }
    };
    assert_eq!(f.said(&spanish), "A Catalog le falta la entrada obligatoria Pages");
    // Every wording names the type and the key it is about, or the type alone.
    for w in WORDINGS {
        assert!(w.contains("{type}"), "{w}");
    }
}

/// An annotation's /M is a date or any text string (`date;string-text`: ISO 32000-2 Table 166
/// says it should be a date, and that a string in any format is to be accepted), so a string that
/// is no date is text, not a malformed date. pdf.js inks.pdf, issue14438.pdf.
#[test]
fn a_value_takes_the_alternative_whose_format_it_has() {
    let mut doc = minimal();
    let annot = |m: &str| dict(&[("Subtype", name("Text")), ("Rect", ints(&[0, 0, 10, 10])), ("M", text(m))]);
    doc.objects.insert(3, page(&[("Annots", Object::Array(vec![r(4)]))]));
    doc.objects.insert(4, annot("D:-001-1-1-1-1-1-00'30'"));
    assert_eq!(problems(&check(&doc)), Vec::<String>::new());
    doc.objects.insert(4, annot("D:20240101120000Z"));
    assert_eq!(problems(&check(&doc)), Vec::<String>::new());
    // Where a date is the only choice, the malformed one is still reported.
    set(&mut doc, 3, &[("LastModified", text("D:-001-1-1-1-1-1-00'30'")), ("PieceInfo", dict(&[]))]);
    let report = check(&doc);
    assert_eq!(find(&report, Rule::Type, "LastModified").map(|f| f.severity), Some(Severity::Warning), "{:?}", problems(&report));
}

#[test]
fn dates_follow_the_specification() {
    for ok in ["D:2024", "D:20240229", "D:20240101120000Z", "D:20240101120000+01'00'", "D:20240101120000-05'00", "20240101"] {
        assert!(check::date_ok(ok), "{ok}");
    }
    for bad in ["", "D:", "D:24", "D:20241301", "D:2024010112000", "D:20240101120000X", "yesterday"] {
        assert!(!check::date_ok(bad), "{bad}");
    }
}

// ── never panic ────────────────────────────────────────────────────────────────────────────

fn any_object() -> impl Strategy<Value = Object> {
    let leaf = prop_oneof![
        Just(Object::Null),
        any::<bool>().prop_map(Object::Bool),
        any::<i64>().prop_map(Object::Int),
        any::<f64>().prop_map(Object::Real),
        prop::collection::vec(any::<u8>(), 0..8).prop_map(|bytes| Object::String { bytes, hex: false }),
        prop::sample::select(vec![
            "Type",
            "Page",
            "Pages",
            "Catalog",
            "Annot",
            "Link",
            "S",
            "URI",
            "Widget",
            "Tx",
            "XYZ",
            "Font",
            "Type1",
            "FlateDecode"
        ])
        .prop_map(name),
        (0u32..12).prop_map(r),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        let keys = prop::sample::select(vec![
            "Type",
            "Subtype",
            "S",
            "Pages",
            "Kids",
            "Parent",
            "Count",
            "MediaBox",
            "Annots",
            "A",
            "Rect",
            "Root",
            "Names",
            "Nums",
            "Dests",
            "Outlines",
            "First",
            "Next",
            "AcroForm",
            "Fields",
            "Resources",
            "Font",
            "Filter",
            "DecodeParms",
            "Length",
            "PageLabels",
            "Version",
            "Encrypt",
            "ID",
            "Info",
            "P",
            "FT",
            "Ff",
            "AP",
            "N",
            "BaseFont",
            "Widths",
        ]);
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Object::Array),
            prop::collection::vec((keys.clone(), inner.clone()), 0..6)
                .prop_map(|e| Object::Dict(e.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())),
            prop::collection::vec((keys, inner), 0..4).prop_map(|e| Object::Stream(e.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Any object graph, however cyclic or ill-typed, gives a report and never a panic.
    #[test]
    fn arbitrary_graphs_never_panic(
        trailer in prop::collection::vec((prop::sample::select(vec!["Root", "Info", "Size", "ID", "Encrypt"]), any_object()), 0..5),
        objects in prop::collection::vec(any_object(), 0..12),
        version in prop::option::of(10u8..=20),
    ) {
        let mut doc = MemSource { version: version.map(Version), ..Default::default() };
        doc.trailer = trailer.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect();
        for (i, o) in objects.into_iter().enumerate() {
            doc.objects.insert(i as u32, o);
        }
        doc.pages = vec![Ref { num: 3, generation: 0 }];
        let report = validate(&doc, &Options::default());
        prop_assert!(report.findings.len() <= 3 * Options::default().max_findings);
        for f in &report.findings {
            let _ = f.message();
        }
    }

    /// The minimal document with any one object replaced still gives a report.
    #[test]
    fn mutated_documents_never_panic(at in 1u32..4, replacement in any_object()) {
        let mut doc = minimal();
        doc.objects.insert(at, replacement);
        let _ = validate(&doc, &Options::default());
    }
}
