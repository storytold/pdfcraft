use super::{ContentKey, Limits, StructureTextIndex};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

fn fixture() -> (Document, [ObjRef; 2]) {
    let mut doc = Document::new_empty();
    let pages = std::array::from_fn(|_| {
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        doc.add(page)
    });
    (doc, pages)
}

fn install(doc: &mut Document, kids: Object) -> ObjRef {
    let mut tree = Dict::new();
    tree.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
    tree.set(b"K".to_vec(), kids);
    let tree = doc.add(tree);
    let root = doc.root().unwrap();
    let mut catalog = doc.get(root).as_dict().unwrap().clone();
    catalog.set(b"StructTreeRoot".to_vec(), tree);
    doc.set(root, catalog);
    tree
}

fn element(page: Option<ObjRef>, text: Option<&str>, kids: Object) -> Object {
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("StructElem"));
    dict.set(b"S".to_vec(), Object::name("Span"));
    if let Some(page) = page {
        dict.set(b"Pg".to_vec(), page);
    }
    if let Some(text) = text {
        dict.set(b"ActualText".to_vec(), PdfString::text(text));
    }
    dict.set(b"K".to_vec(), kids);
    Object::Dict(dict)
}

fn mcr(page: Option<ObjRef>, stream: Option<ObjRef>, mcid: i64) -> Object {
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("MCR"));
    if let Some(page) = page {
        dict.set(b"Pg".to_vec(), page);
    }
    if let Some(stream) = stream {
        dict.set(b"Stm".to_vec(), stream);
    }
    dict.set(b"MCID".to_vec(), Object::Int(mcid));
    Object::Dict(dict)
}

fn key(page: ObjRef, mcid: i64) -> ContentKey {
    ContentKey { page, stream: None, mcid }
}

#[test]
fn inherited_page_noncontiguous_members_and_equal_text_keep_element_identity() {
    let (mut doc, pages) = fixture();
    let first = element(None, Some("same"), Object::Array(vec![Object::Int(0), Object::Int(2)]));
    let second = element(None, Some("same"), Object::Int(3));
    let neighbor = element(None, None, Object::Int(1));
    install(&mut doc, element(Some(pages[0]), None, Object::Array(vec![first, neighbor, second])));
    let index = StructureTextIndex::new(&doc, &pages);
    let first = index.owner(&key(pages[0], 0)).unwrap();
    let second = index.owner(&key(pages[0], 3)).unwrap();
    assert_eq!(index.owner(&key(pages[0], 2)), Some(first));
    assert_ne!(first, second);
    assert_eq!(index.text(first), Some("same"));
    assert_eq!(index.members(first), &[key(pages[0], 0), key(pages[0], 2)]);
    assert_eq!(index.owner(&key(pages[0], 1)), None);
    assert_eq!(index.owners_on_page(pages[0]).collect::<Vec<_>>(), vec![first, second]);
    assert!(!index.incomplete(pages[0]));
}

#[test]
fn empty_outer_replacement_shadows_nested_text_without_decoding_it() {
    let (mut doc, pages) = fixture();
    let nested = element(None, Some("ignored child replacement"), Object::Int(0));
    install(&mut doc, element(Some(pages[0]), Some(""), nested));
    let index = StructureTextIndex::with_limits(&doc, &pages, Limits { raw_text: 0, decoded_text: 0, ..Limits::DEFAULT });
    let owner = index.owner(&key(pages[0], 0)).unwrap();
    assert_eq!(index.text(owner), Some(""));
    assert!(!index.incomplete(pages[0]));
}

#[test]
fn indirect_arrays_scalars_and_unicode_text_are_read_without_losing_members() {
    let (mut doc, pages) = fixture();
    let text = doc.add(PdfString::text("logical \u{03bb}\u{1f642}"));
    let mcid = doc.add(Object::Int(7));
    let kids = doc.add(Object::Array(vec![Object::Ref(mcid)]));
    let mut span = element(Some(pages[0]), None, Object::Ref(kids)).as_dict().unwrap().clone();
    span.set(b"ActualText".to_vec(), text);
    let span = doc.add(span);
    let tree = install(&mut doc, Object::Ref(span));
    let mut parented = doc.get(span).as_dict().unwrap().clone();
    parented.set(b"P".to_vec(), tree);
    doc.set(span, parented);
    let index = StructureTextIndex::new(&doc, &pages);
    let owner = index.owner(&key(pages[0], 7)).unwrap();
    assert_eq!(index.text(owner), Some("logical \u{03bb}\u{1f642}"));
    assert!(!index.incomplete(pages[0]));
}

#[test]
fn page_and_form_mcids_have_distinct_contexts_and_mcr_page_overrides_inheritance() {
    let (mut doc, pages) = fixture();
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    let form = doc.add(Object::Stream(Stream::from_raw(form, Vec::new())));
    let page = element(Some(pages[0]), Some("page"), Object::Int(0));
    let form_owner = element(Some(pages[0]), Some("form"), mcr(None, Some(form), 0));
    let override_page = element(Some(pages[0]), Some("other page"), mcr(Some(pages[1]), None, 0));
    install(&mut doc, Object::Array(vec![page, form_owner, override_page]));
    let index = StructureTextIndex::new(&doc, &pages);
    let form_key = ContentKey { page: pages[0], stream: Some(form), mcid: 0 };
    let owners = [key(pages[0], 0), form_key, key(pages[1], 0)].map(|key| index.owner(&key).unwrap());
    assert_ne!(owners[0], owners[1]);
    assert_ne!(owners[1], owners[2]);
    assert_eq!(index.text(owners[0]), Some("page"));
    assert_eq!(index.text(owners[1]), Some("form"));
    assert_eq!(index.text(owners[2]), Some("other page"));
    assert!(pages.into_iter().all(|page| !index.incomplete(page)));
}

#[test]
fn multipage_owner_is_withheld_but_unrelated_complete_owner_survives() {
    let (mut doc, pages) = fixture();
    let mixed = element(Some(pages[0]), Some("mixed"), Object::Array(vec![Object::Int(0), mcr(Some(pages[1]), None, 0)]));
    let good = element(Some(pages[1]), Some("good"), Object::Int(1));
    install(&mut doc, Object::Array(vec![mixed, good]));
    let index = StructureTextIndex::new(&doc, &pages);
    assert_eq!(index.owner(&key(pages[0], 0)), None);
    assert_eq!(index.owner(&key(pages[1], 0)), None);
    let good = index.owner(&key(pages[1], 1)).unwrap();
    assert_eq!(index.text(good), Some("good"));
    assert!(pages.into_iter().all(|page| index.incomplete(page)));
}

#[test]
fn duplicate_claims_are_withheld_even_if_the_first_claim_has_no_actual_text() {
    for first in [None, Some("first")] {
        let (mut doc, pages) = fixture();
        let first = element(Some(pages[0]), first, Object::Int(0));
        let second = element(Some(pages[0]), Some("second"), Object::Int(0));
        install(&mut doc, Object::Array(vec![first, second]));
        let index = StructureTextIndex::new(&doc, &pages);
        assert_eq!(index.owner(&key(pages[0], 0)), None);
        assert!(index.declared(&key(pages[0], 0)));
        assert_eq!(index.owners_on_page(pages[0]).count(), 0);
        assert!(index.incomplete(pages[0]));
    }
}

#[test]
fn repeated_key_inside_one_owner_is_ambiguous() {
    let (mut doc, pages) = fixture();
    install(&mut doc, element(Some(pages[0]), Some("once"), Object::Array(vec![Object::Int(0), Object::Int(0)])));
    let index = StructureTextIndex::new(&doc, &pages);
    assert_eq!(index.owner(&key(pages[0], 0)), None);
    assert!(index.incomplete(pages[0]));
}

#[test]
fn every_traversal_limit_discards_preceding_valid_mappings() {
    let (mut doc, pages) = fixture();
    install(
        &mut doc,
        Object::Array(vec![element(Some(pages[0]), Some("first"), Object::Int(0)), element(Some(pages[0]), Some("second"), Object::Int(1))]),
    );
    let limits = [
        Limits { depth: 1, ..Limits::DEFAULT },
        Limits { work: 10, ..Limits::DEFAULT },
        Limits { owners: 1, ..Limits::DEFAULT },
        Limits { keys: 1, ..Limits::DEFAULT },
    ];
    for limit in limits {
        let index = StructureTextIndex::with_limits(&doc, &pages, limit);
        assert_eq!(index.owner(&key(pages[0], 0)), None);
        assert!(!index.declared(&key(pages[0], 0)), "a stopped walk must expose no declaration prefix");
        assert_eq!(index.owners_on_page(pages[0]).count(), 0);
        assert!(pages.into_iter().all(|page| index.incomplete(page)));
    }
}

#[test]
fn cyclic_k_and_reference_aliases_discard_valid_prefix() {
    for alias in [false, true] {
        let (mut doc, pages) = fixture();
        let cycle = doc.add(Object::Null);
        let child = if alias { Object::Ref(cycle) } else { Object::Array(vec![Object::Ref(cycle)]) };
        doc.set(cycle, child);
        install(&mut doc, Object::Array(vec![element(Some(pages[0]), Some("first"), Object::Int(0)), Object::Ref(cycle)]));
        let index = StructureTextIndex::new(&doc, &pages);
        assert_eq!(index.owner(&key(pages[0], 0)), None);
        assert!(index.incomplete(pages[0]));
    }
}

#[test]
fn wide_k_is_scanned_under_one_shared_work_budget() {
    let (mut doc, pages) = fixture();
    let mut kids = vec![element(Some(pages[0]), Some("first"), Object::Int(0))];
    kids.extend((1..100).map(|mcid| element(Some(pages[0]), None, Object::Int(mcid))));
    install(&mut doc, Object::Array(kids));
    let index = StructureTextIndex::with_limits(&doc, &pages, Limits { work: 32, ..Limits::DEFAULT });
    assert_eq!(index.owner(&key(pages[0], 0)), None);
    assert!(index.incomplete(pages[0]));
}

#[test]
fn raw_and_cumulative_decoding_limits_withhold_affected_owners() {
    let (mut doc, pages) = fixture();
    install(
        &mut doc,
        Object::Array(vec![element(Some(pages[0]), Some("one"), Object::Int(0)), element(Some(pages[0]), Some("two"), Object::Int(1))]),
    );
    let raw = StructureTextIndex::with_limits(&doc, &pages, Limits { raw_text: 2, ..Limits::DEFAULT });
    assert_eq!(raw.owner(&key(pages[0], 0)), None);
    assert!(raw.incomplete(pages[0]));
    // The first three-byte string consumes three bytes; a second decode needs a
    // conservative nine-byte expansion allowance before allocating its String.
    let decoded = StructureTextIndex::with_limits(&doc, &pages, Limits { decoded_text: 9, ..Limits::DEFAULT });
    assert!(decoded.owner(&key(pages[0], 0)).is_some());
    assert_eq!(decoded.owner(&key(pages[0], 1)), None);
    assert!(decoded.incomplete(pages[0]));
}

#[test]
fn unsupported_graphic_members_invalidate_the_whole_text_owner() {
    let (mut doc, pages) = fixture();
    let mut objr = Dict::new();
    objr.set(b"Type".to_vec(), Object::name("OBJR"));
    install(&mut doc, element(Some(pages[0]), Some("graphic and text"), Object::Array(vec![Object::Int(0), Object::Dict(objr)])));
    let index = StructureTextIndex::new(&doc, &pages);
    assert_eq!(index.owner(&key(pages[0], 0)), None);
    assert!(index.incomplete(pages[0]));
}

#[test]
fn unknown_containers_missing_page_and_unsupported_streams_disable_uncertain_prefix() {
    for invalid in [Object::Dict(Dict::new()), element(None, Some("no page"), Object::Int(1)), mcr(None, Some(ObjRef::new(9999, 0)), 1)] {
        let (mut doc, pages) = fixture();
        install(&mut doc, Object::Array(vec![element(Some(pages[0]), Some("first"), Object::Int(0)), invalid]));
        let index = StructureTextIndex::new(&doc, &pages);
        assert_eq!(index.owner(&key(pages[0], 0)), None);
        assert!(index.incomplete(pages[0]));
    }
}

#[test]
fn malformed_element_role_or_type_cannot_authorize_replacement() {
    for field in [b"S".as_slice(), b"Type".as_slice()] {
        let (mut doc, pages) = fixture();
        let mut invalid = element(Some(pages[0]), Some("bad"), Object::Int(1)).as_dict().unwrap().clone();
        invalid.set(field.to_vec(), Object::Int(42));
        install(&mut doc, Object::Array(vec![element(Some(pages[0]), Some("first"), Object::Int(0)), Object::Dict(invalid)]));
        let index = StructureTextIndex::new(&doc, &pages);
        assert_eq!(index.owner(&key(pages[0], 0)), None);
        assert_eq!(index.owner(&key(pages[0], 1)), None);
        assert!(index.incomplete(pages[0]));
    }
}

#[test]
fn contradictory_parent_disables_uncertain_prefix_and_untagged_document_is_complete() {
    let (mut doc, pages) = fixture();
    let untagged = StructureTextIndex::new(&doc, &pages);
    assert_eq!(untagged.owners_on_page(pages[0]).count(), 0);
    assert!(!untagged.incomplete(pages[0]));
    let mut wrong_parent = element(Some(pages[0]), Some("bad"), Object::Int(1)).as_dict().unwrap().clone();
    wrong_parent.set(b"P".to_vec(), pages[0]);
    install(&mut doc, Object::Array(vec![element(Some(pages[0]), Some("first"), Object::Int(0)), Object::Dict(wrong_parent)]));
    let index = StructureTextIndex::new(&doc, &pages);
    assert_eq!(index.owner(&key(pages[0], 0)), None);
    assert!(index.incomplete(pages[0]));
}

#[test]
fn untagged_document_skips_page_enrollment_even_without_walk_budget() {
    let doc = Document::new_empty();
    let pages: Vec<_> = (100..100_100).map(|num| ObjRef::new(num, 0)).collect();
    let index = StructureTextIndex::with_limits(&doc, &pages, Limits { work: 0, ..Limits::DEFAULT });
    assert!(index.bindings.is_empty());
    assert!(index.by_page.is_empty());
    assert!(index.incomplete_pages.is_empty());
    assert!(!index.all_incomplete);
    for page in [pages.first().unwrap(), pages.last().unwrap()] {
        assert_eq!(index.owners_on_page(*page).count(), 0);
        assert!(!index.incomplete(*page));
    }
}

#[test]
fn declared_content_survives_without_a_valid_replacement_owner() {
    let (mut doc, pages) = fixture();
    let plain = element(Some(pages[0]), None, Object::Int(0));
    let mut invalid = element(Some(pages[0]), None, Object::Int(1)).as_dict().unwrap().clone();
    invalid.set(b"ActualText".to_vec(), Object::Int(42));
    let good = element(Some(pages[0]), Some("good"), Object::Int(2));
    install(&mut doc, Object::Array(vec![plain, Object::Dict(invalid), good]));
    let index = StructureTextIndex::new(&doc, &pages);
    for mcid in [0, 1] {
        let key = key(pages[0], mcid);
        assert!(index.declared(&key));
        assert_eq!(index.owner(&key), None);
    }
    let good = key(pages[0], 2);
    assert!(index.declared(&good));
    assert_eq!(index.text(index.owner(&good).unwrap()), Some("good"));
    assert!(!index.declared(&key(pages[0], 3)));
    assert!(!index.declared(&key(pages[1], 0)), "declarations retain their page context");
    assert!(!index.declared(&ContentKey { page: pages[0], stream: Some(pages[1]), mcid: 0 }), "declarations retain their stream context");
    assert_eq!(index.owners_on_page(pages[0]).count(), 1);
    assert!(index.incomplete(pages[0]));
}
