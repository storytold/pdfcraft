//! Named destinations: listing and the Destinations panel's edits (`dests.rs`).

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::tests::{build, full_roundtrip, open, save_and_reopen};
use crate::{
    OrganizeError, add_named_destination, delete_named_destination, named_destinations, pages, rename_named_destination, set_named_destination,
};

/// Three pages (objects 3, 4, 5). An indirect `/Names` (6) holds an attachment tree and a
/// two-leaf `/Dests` tree (7: 9 and 10); the catalog's old `/Dests` dictionary (8) has `shared`
/// too, which the tree's entry hides. `beta` is an indirect array (11), `gamma` a dictionary
/// with a structure destination (`/SD`, 12), `broken` goes nowhere and `num` names its page by
/// number.
fn fixture() -> Document {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names 6 0 R /Dests 8 0 R >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R >>".into(),                             // 3
        "<< /Type /Page /Parent 2 0 R >>".into(),                             // 4
        "<< /Type /Page /Parent 2 0 R >>".into(),                             // 5
        "<< /Dests 7 0 R /EmbeddedFiles << /Names [] >> >>".into(),           // 6
        "<< /Kids [9 0 R 10 0 R] >>".into(),                                  // 7
        "<< /old [5 0 R /Fit] /shared [3 0 R /Fit] /num [0 /Fit] >>".into(),  // 8
        "<< /Names [(alpha) [3 0 R /Fit] (beta) 11 0 R (broken) 42] /Limits [(alpha) (broken)] >>".into(), // 9
        "<< /Names [(gamma) << /D [5 0 R /XYZ 10 380 null] /SD [12 0 R] >> (shared) [4 0 R /FitH 200]] /Limits [(gamma) (shared)] >>".into(), // 10
        "[4 0 R /XYZ 0 300 2]".into(),                                        // 11
        "<< /Type /StructElem /S /P >>".into(),                               // 12
    ];
    open(build(&b, "/Root 1 0 R"))
}

/// Each destination's name and page, in listing order.
fn list(doc: &Document) -> Vec<(String, Option<usize>)> {
    named_destinations(doc).into_iter().map(|d| (d.name, d.page)).collect()
}

/// The catalog, resolved.
fn catalog(doc: &Document) -> Dict {
    doc.get(doc.root().unwrap()).as_dict().cloned().unwrap()
}

/// The `/Names` dictionary, resolved.
fn names(doc: &Document) -> Dict {
    doc.resolve(catalog(doc).get(b"Names").unwrap()).as_dict().cloned().unwrap()
}

/// The `/Dests` tree's root node's `(key, value)` pairs, its values resolved.
fn tree_pairs(doc: &Document) -> Vec<(Vec<u8>, Object)> {
    let node = doc.resolve(names(doc).get(b"Dests").unwrap());
    let node = node.as_dict().unwrap();
    assert!(node.get(b"Kids").is_none(), "an edit writes one flat node: {node:?}");
    let pairs = doc.resolve(node.get(b"Names").unwrap()).as_array().cloned().unwrap();
    pairs.chunks(2).map(|p| (p[0].as_string().unwrap().bytes.clone(), (*doc.resolve(&p[1])).clone())).collect()
}

/// The old `/Dests` dictionary, resolved.
fn legacy(doc: &Document) -> Dict {
    doc.resolve(catalog(doc).get(b"Dests").unwrap()).as_dict().cloned().unwrap()
}

/// The page object of page `i` (0-based).
fn page(doc: &Document, i: usize) -> ObjRef {
    pages(doc).unwrap()[i].obj
}

/// `[page /XYZ left top null]`, as a new destination is written.
fn xyz(page: ObjRef, left: Option<f64>, top: Option<f64>) -> Object {
    let n = |v: Option<f64>| v.map_or(Object::Null, Object::Real);
    Object::Array(vec![Object::Ref(page), Object::name("XYZ"), n(left), n(top), Object::Null])
}

fn owned(v: &[(&str, Option<usize>)]) -> Vec<(String, Option<usize>)> {
    v.iter().map(|(n, p)| (n.to_string(), *p)).collect()
}

#[test]
fn the_tree_and_the_old_dictionary_are_listed_by_name_with_their_page_and_view() {
    let doc = fixture();
    let all = named_destinations(&doc);
    assert_eq!(
        all.iter().map(|d| (d.name.clone(), d.page)).collect::<Vec<_>>(),
        owned(&[
            ("alpha", Some(0)),
            ("beta", Some(1)),
            ("broken", None),
            ("gamma", Some(2)),
            ("num", Some(0)),
            ("old", Some(2)),
            // The tree's entry hides the old dictionary's (which goes to page 1).
            ("shared", Some(1)),
        ])
    );
    let view = |name: &str| all.iter().find(|d| d.name == name).unwrap().view.clone();
    assert_eq!(view("beta"), Some((b"XYZ".to_vec(), vec![Some(0.0), Some(300.0), Some(2.0)])));
    assert_eq!(view("gamma"), Some((b"XYZ".to_vec(), vec![Some(10.0), Some(380.0), None])));
    assert_eq!(view("shared"), Some((b"FitH".to_vec(), vec![Some(200.0)])));
    assert_eq!(view("broken"), None);
}

#[test]
fn a_new_destination_goes_into_the_tree_beside_the_other_trees_and_survives_saving() {
    let mut doc = fixture();
    add_named_destination(&mut doc, "Résumé", 2, Some(15.0), Some(350.0)).unwrap();
    // The indirect /Names is updated in place: its attachment tree stays.
    assert_eq!(catalog(&doc).get(b"Names"), Some(&Object::Ref(ObjRef::new(6, 0))));
    assert!(names(&doc).contains(b"EmbeddedFiles"));
    let pairs = tree_pairs(&doc);
    let keys: Vec<Vec<u8>> = pairs.iter().map(|(k, _)| k.clone()).collect();
    let resume = PdfString::text("Résumé").bytes;
    assert_eq!(keys, vec![b"alpha".to_vec(), b"beta".to_vec(), b"broken".to_vec(), b"gamma".to_vec(), b"shared".to_vec(), resume.clone()]);
    assert_eq!(pairs[5].1, xyz(page(&doc, 2), Some(15.0), Some(350.0)));
    // Entries that were indirect stay so.
    let node = doc.resolve(names(&doc).get(b"Dests").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(node.get(b"Names").and_then(Object::as_array).unwrap()[3], Object::Ref(ObjRef::new(11, 0)));
    for reopened in [save_and_reopen(&doc), full_roundtrip(&doc)] {
        let found = named_destinations(&reopened).into_iter().find(|d| d.name == "Résumé").unwrap();
        assert_eq!(found.page, Some(2));
        assert_eq!(found.view, Some((b"XYZ".to_vec(), vec![Some(15.0), Some(350.0), None])));
        // A link naming it goes there.
        let dest = crate::outline::resolve_dest(&reopened, &Object::String(PdfString::text("Résumé"))).unwrap();
        assert_eq!(dest.first(), Some(&Object::Ref(page(&reopened, 2))));
        assert_eq!(list(&reopened).len(), 8);
    }
}

#[test]
fn a_new_destination_is_refused_when_its_name_or_place_is_not_valid() {
    let mut doc = fixture();
    let before = (list(&doc), doc.object_numbers().len());
    let invalid = |e: OrganizeError| match e {
        OrganizeError::Invalid(m) => m,
        other => panic!("{other:?}"),
    };
    // Taken in the tree, and in the old dictionary only.
    assert_eq!(invalid(add_named_destination(&mut doc, "alpha", 0, None, None).unwrap_err()), "a destination named alpha already exists");
    assert_eq!(invalid(add_named_destination(&mut doc, "old", 0, None, None).unwrap_err()), "a destination named old already exists");
    assert_eq!(invalid(add_named_destination(&mut doc, "  ", 0, None, None).unwrap_err()), "a destination needs a name");
    let long = "x".repeat(crate::MAX_DESTINATION_NAME + 1);
    assert!(invalid(add_named_destination(&mut doc, &long, 0, None, None).unwrap_err()).contains("at most 1024 characters"));
    assert_eq!(add_named_destination(&mut doc, "new", 3, None, None), Err(OrganizeError::NoSuchPage(3)));
    assert!(invalid(add_named_destination(&mut doc, "new", 0, Some(f64::NAN), None).unwrap_err()).contains("finite"));
    assert!(invalid(add_named_destination(&mut doc, "new", 0, None, Some(f64::INFINITY)).unwrap_err()).contains("finite"));
    assert_eq!((list(&doc), doc.object_numbers().len()), before, "a refusal changes nothing");
    // The longest name allowed is taken.
    let longest = "y".repeat(crate::MAX_DESTINATION_NAME);
    add_named_destination(&mut doc, &longest, 0, None, None).unwrap();
    assert!(list(&doc).contains(&(longest, Some(0))));
}

#[test]
fn set_destination_sends_every_entry_of_the_name_to_the_new_view() {
    let mut doc = fixture();
    let (p0, p1) = (page(&doc, 0), page(&doc, 1));
    // A dictionary keeps its structure destination; only /D changes.
    set_named_destination(&mut doc, "gamma", 0, None, Some(100.0)).unwrap();
    let gamma = tree_pairs(&doc).into_iter().find(|(k, _)| k == b"gamma").unwrap().1;
    let gamma = gamma.as_dict().unwrap();
    assert_eq!(gamma.get(b"D"), Some(&xyz(p0, None, Some(100.0))));
    assert_eq!(gamma.get(b"SD"), Some(&Object::Array(vec![Object::Ref(ObjRef::new(12, 0))])));
    // A name in both places changes in both, so neither is left going elsewhere.
    set_named_destination(&mut doc, "shared", 1, Some(5.0), Some(6.0)).unwrap();
    let shared = tree_pairs(&doc).into_iter().find(|(k, _)| k == b"shared").unwrap().1;
    assert_eq!(shared, xyz(p1, Some(5.0), Some(6.0)));
    assert_eq!(legacy(&doc).get(b"shared"), Some(&xyz(p1, Some(5.0), Some(6.0))));
    // A name only in the old dictionary changes there, and the tree is not rewritten.
    let tree = names(&doc).get(b"Dests").cloned();
    set_named_destination(&mut doc, "old", 0, None, None).unwrap();
    assert_eq!(legacy(&doc).get(b"old"), Some(&xyz(p0, None, None)));
    assert_eq!(names(&doc).get(b"Dests").cloned(), tree);
    assert_eq!(
        set_named_destination(&mut doc, "nothing", 0, None, None),
        Err(OrganizeError::Invalid("there is no destination named nothing".into()))
    );
    let reopened = full_roundtrip(&doc);
    let found: Vec<_> = list(&reopened).into_iter().filter(|(n, _)| ["gamma", "shared", "old"].contains(&n.as_str())).collect();
    assert_eq!(found, owned(&[("gamma", Some(0)), ("old", Some(0)), ("shared", Some(1))]));
}

#[test]
fn rename_keeps_where_a_destination_goes_in_the_tree_and_the_old_dictionary() {
    let mut doc = fixture();
    rename_named_destination(&mut doc, "beta", "bravo").unwrap();
    // Its indirect value is kept as it was.
    let bravo = doc.resolve(names(&doc).get(b"Dests").unwrap()).as_dict().unwrap().get(b"Names").and_then(Object::as_array).cloned().unwrap();
    let at = bravo.iter().position(|k| k.as_string().is_some_and(|s| s.bytes == b"bravo")).unwrap();
    assert_eq!(bravo[at + 1], Object::Ref(ObjRef::new(11, 0)));
    rename_named_destination(&mut doc, "shared", "common").unwrap();
    rename_named_destination(&mut doc, "old", "Ältere").unwrap();
    let old = legacy(&doc);
    assert!(!old.contains(b"shared") && !old.contains(b"old"), "{old:?}");
    assert_eq!(old.get(b"common"), Some(&Object::Array(vec![Object::Ref(page(&doc, 0)), Object::name("Fit")])));
    assert!(old.contains("Ältere".as_bytes()));
    let after = list(&doc);
    assert_eq!(
        after,
        owned(&[
            ("alpha", Some(0)),
            ("bravo", Some(1)),
            ("broken", None),
            ("common", Some(1)),
            ("gamma", Some(2)),
            ("num", Some(0)),
            ("Ältere", Some(2)),
        ])
    );
    let invalid = |r: Result<(), OrganizeError>| r.unwrap_err().to_string();
    assert_eq!(invalid(rename_named_destination(&mut doc, "alpha", "gamma")), "a destination named gamma already exists");
    assert_eq!(invalid(rename_named_destination(&mut doc, "alpha", "")), "a destination needs a name");
    assert_eq!(invalid(rename_named_destination(&mut doc, "beta", "b")), "there is no destination named beta");
    assert_eq!(list(&full_roundtrip(&doc)), after);
}

#[test]
fn delete_removes_the_name_everywhere_and_an_empty_tree_with_it() {
    let mut doc = fixture();
    delete_named_destination(&mut doc, "shared").unwrap();
    assert!(!legacy(&doc).contains(b"shared"));
    assert!(list(&doc).iter().all(|(n, _)| n != "shared"), "the old dictionary's entry is not uncovered");
    assert_eq!(delete_named_destination(&mut doc, "shared"), Err(OrganizeError::Invalid("there is no destination named shared".into())));
    for name in ["alpha", "beta", "broken", "gamma"] {
        delete_named_destination(&mut doc, name).unwrap();
    }
    // The tree is gone; the attachments beside it stay.
    let left = names(&doc);
    assert!(!left.contains(b"Dests") && left.contains(b"EmbeddedFiles"), "{left:?}");
    assert_eq!(list(&full_roundtrip(&doc)), owned(&[("num", Some(0)), ("old", Some(2))]));

    // With nothing else in an inline /Names, it goes too.
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests << /Names [(only) [3 0 R /Fit]] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    let mut doc = open(build(&b, "/Root 1 0 R"));
    delete_named_destination(&mut doc, "only").unwrap();
    assert!(!catalog(&doc).contains(b"Names"));
    assert!(list(&full_roundtrip(&doc)).is_empty());
    // A first destination in a document without one makes the tree.
    add_named_destination(&mut doc, "first", 0, None, None).unwrap();
    assert_eq!(list(&full_roundtrip(&doc)), owned(&[("first", Some(0))]));
}

#[test]
fn a_tree_nested_too_deeply_to_read_whole_is_listed_but_not_rewritten() {
    // A chain of 40 nodes, one name at each level: those past the read depth are not listed.
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 4 0 R >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    for level in 0..40 {
        let kid = if level < 39 { format!("/Kids [{} 0 R]", level + 5) } else { String::new() };
        b.push(format!("<< /Names [(n{level:02}) [3 0 R /Fit]] {kid} >>"));
    }
    let mut doc = open(build(&b, "/Root 1 0 R"));
    assert_eq!(list(&doc).len(), 33, "levels 0 to 32 are read");
    let before = doc.object_numbers().len();
    for refused in [
        add_named_destination(&mut doc, "new", 0, None, None),
        set_named_destination(&mut doc, "n00", 0, None, None),
        rename_named_destination(&mut doc, "n00", "m00"),
        delete_named_destination(&mut doc, "n00"),
    ] {
        assert!(refused.unwrap_err().to_string().contains("cannot be edited without losing some"));
    }
    assert_eq!(doc.object_numbers().len(), before);
}

#[test]
fn a_cyclic_tree_is_read_once_and_rewritten_whole() {
    // The root lists itself and a leaf twice: every entry is read once, nothing is lost.
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 4 0 R >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Kids [4 0 R 5 0 R 5 0 R] >>".into(),
        "<< /Names [(a) [3 0 R /Fit] (b) [3 0 R /Fit]] >>".into(),
    ];
    let mut doc = open(build(&b, "/Root 1 0 R"));
    assert_eq!(list(&doc), owned(&[("a", Some(0)), ("b", Some(0))]));
    add_named_destination(&mut doc, "c", 0, None, None).unwrap();
    assert_eq!(tree_pairs(&doc).len(), 3);
    assert_eq!(list(&full_roundtrip(&doc)), owned(&[("a", Some(0)), ("b", Some(0)), ("c", Some(0))]));
}

#[test]
fn a_name_standing_for_another_name_goes_nowhere_and_is_not_followed() {
    // 20,000 names, each standing for the next one's name: none is a destination (ISO 32000-2
    // §12.3.2.4), and following them would read the whole tree once per entry.
    let n = 20_000;
    let pairs: String = (0..n).map(|i| format!("(d{i:05}) (d{:05}) ", i + 1)).collect();
    let b: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Names << /Dests << /Names [{pairs}(last) [3 0 R /Fit]] >> >> >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    let doc = open(build(&b, "/Root 1 0 R"));
    let all = named_destinations(&doc);
    assert_eq!(all.len(), n + 1);
    assert!(all.iter().filter(|d| d.name != "last").all(|d| d.page.is_none() && d.view.is_none()));
    assert_eq!(all.last().map(|d| (d.name.as_str(), d.page)), Some(("last", Some(0))));
}

/// Two pages (3, 4). `ch` is in the tree, going to page 2; `old` only in the old dictionary,
/// going to page 1. What names `ch`: a link's `/Dest` (5), a bookmark's indirect GoTo action (7,
/// under the outline 6), a widget's mouse-up trigger in an inline `/AA` (9), the catalog's opening
/// action, whose inline `/Next` array chains a GoTo naming `ch` as a name, and an indirect action
/// (10) whose `/Next` is itself. A link naming `old` (11) by name; a GoToR (12) naming `ch` in
/// another file, which is not this document's.
fn linked() -> Document {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R /Dests << /old [3 0 R /Fit] >> /Names << /Dests << /Names [(ch) [4 0 R /FitH 300]] >> >> \
         /OpenAction << /S /GoTo /D (ch) /Next [<< /S /GoTo /D /ch >> 10 0 R] >> >>"
            .into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 9 0 R 11 0 R 12 0 R] >>".into(),     // 3
        "<< /Type /Page /Parent 2 0 R >>".into(),                                         // 4
        "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest (ch) >>".into(),          // 5
        "<< /Type /Outlines /First 8 0 R /Last 8 0 R /Count 1 >>".into(),                 // 6
        "<< /S /GoTo /D (ch) >>".into(),                                                  // 7
        "<< /Title (Chapter) /Parent 6 0 R /A 7 0 R >>".into(),                           // 8
        "<< /Type /Annot /Subtype /Widget /Rect [0 0 10 10] /AA << /U << /S /GoTo /D (ch) >> >> >>".into(), // 9
        "<< /S /GoTo /D <6368> /Next 10 0 R >>".into(),                                   // 10
        "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest /old >>".into(),          // 11
        "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /GoToR /F (other.pdf) /D (ch) >> >>".into(), // 12
    ];
    open(build(&b, "/Root 1 0 R"))
}

/// Every destination [`linked`] has a reference to, as written, in the order its doc lists them:
/// link 5, bookmark action 7, widget 9's trigger, the opening action, its chained name and the
/// indirect chained action 10, link 11 and the GoToR's.
fn references(doc: &Document) -> Vec<Object> {
    let dict = |n: u32| doc.get(ObjRef::new(n, 0)).as_dict().cloned().unwrap();
    let open = catalog(doc).get(b"OpenAction").and_then(Object::as_dict).cloned().unwrap();
    let next = open.get(b"Next").and_then(Object::as_array).cloned().unwrap();
    vec![
        dict(5).get(b"Dest").cloned().unwrap(),
        dict(7).get(b"D").cloned().unwrap(),
        dict(9)
            .get(b"AA")
            .and_then(Object::as_dict)
            .and_then(|aa| aa.get(b"U"))
            .and_then(Object::as_dict)
            .and_then(|u| u.get(b"D"))
            .cloned()
            .unwrap(),
        open.get(b"D").cloned().unwrap(),
        next[0].as_dict().and_then(|n| n.get(b"D")).cloned().unwrap(),
        dict(10).get(b"D").cloned().unwrap(),
        dict(11).get(b"Dest").cloned().unwrap(),
        dict(12).get(b"A").and_then(Object::as_dict).and_then(|a| a.get(b"D")).cloned().unwrap(),
    ]
}

fn string(s: &str) -> Object {
    Object::String(PdfString::literal(s.as_bytes().to_vec()))
}

#[test]
fn renaming_gives_the_documents_links_bookmarks_and_actions_the_new_name() {
    let mut doc = linked();
    rename_named_destination(&mut doc, "ch", "intro").unwrap();
    let hex = Object::String(PdfString { bytes: b"intro".to_vec(), hex: true });
    let expected = [
        string("intro"),
        string("intro"),
        string("intro"),
        string("intro"),
        // A name naming a tree entry that isn't in the old dictionary becomes a string, which
        // is how a tree entry is named.
        string("intro"),
        // A hex string stays one.
        hex,
        Object::name("old"),
        // Another file's destination is left alone.
        string("ch"),
    ];
    assert_eq!(references(&doc), expected);
    // The old dictionary's destination keeps its kind of reference: a name.
    rename_named_destination(&mut doc, "old", "Übersicht").unwrap();
    assert_eq!(references(&doc)[6], Object::Name("Übersicht".as_bytes().to_vec()));
    // Saved incrementally, so the objects keep their numbers.
    let reopened = save_and_reopen(&doc);
    for (i, r) in references(&reopened).iter().enumerate().take(7) {
        let want = if i == 6 { page(&reopened, 0) } else { page(&reopened, 1) };
        let dest = crate::outline::resolve_dest(&reopened, r).unwrap_or_else(|| panic!("reference {i} goes nowhere: {r:?}"));
        assert_eq!(dest.first(), Some(&Object::Ref(want)), "reference {i}");
    }
}

#[test]
fn deleting_sends_the_documents_links_bookmarks_and_actions_where_it_went() {
    let mut doc = linked();
    delete_named_destination(&mut doc, "ch").unwrap();
    let went = Object::Array(vec![Object::Ref(page(&doc, 1)), Object::name("FitH"), Object::Int(300)]);
    let mut expected = vec![went; 6];
    expected.extend([Object::name("old"), string("ch")]);
    assert_eq!(references(&doc), expected);
    assert_eq!(list(&doc), owned(&[("old", Some(0))]));
    delete_named_destination(&mut doc, "old").unwrap();
    assert_eq!(references(&doc)[6], Object::Array(vec![Object::Ref(page(&doc, 0)), Object::name("Fit")]));
    let reopened = save_and_reopen(&doc);
    assert!(list(&reopened).is_empty());
    assert_eq!(
        crate::outline::resolve_dest(&reopened, &references(&reopened)[0]).and_then(|d| d.first().cloned()),
        Some(Object::Ref(page(&reopened, 1)))
    );
}

#[test]
fn deleting_a_destination_that_goes_nowhere_leaves_what_names_it_alone() {
    let mut doc = fixture();
    let link = doc.add({
        let mut d = Dict::new();
        d.set(b"Subtype".to_vec(), Object::name("Link"));
        d.set(b"Dest".to_vec(), string("broken"));
        d
    });
    doc.update_dict(page(&doc, 0), |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(link)]))).unwrap();
    delete_named_destination(&mut doc, "broken").unwrap();
    assert_eq!(doc.get(link).as_dict().and_then(|d| d.get(b"Dest")).cloned(), Some(string("broken")));
}

#[test]
fn hostile_action_chains_end_and_are_rewritten_once() {
    // An action chain 200 deep (its first 65 are followed), and an annotation that is also its
    // own action, its own trigger and the page's /AA: nothing loops or overflows, and its own
    // /D, as an action, is rewritten all the same.
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests << /Names [(ch) [3 0 R /Fit]] >> >> /OpenAction 5 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 4 0 R] /AA 4 0 R >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /A 4 0 R /AA << /U 4 0 R >> /S /GoTo /D (ch) /Next [4 0 R 3 0 R] >>".into(),
    ];
    for i in 0..200 {
        b.push(format!("<< /S /GoTo /D (ch) /Next {} 0 R >>", i + 6));
    }
    b.push("<< /S /GoTo /D (ch) >>".into());
    let mut doc = open(build(&b, "/Root 1 0 R"));
    rename_named_destination(&mut doc, "ch", "new").unwrap();
    let d = |n: u32| doc.get(ObjRef::new(n, 0)).as_dict().and_then(|d| d.get(b"D").cloned());
    assert_eq!(d(4), Some(string("new")));
    // The opening action is chain level 0; levels up to 64 are rewritten, deeper ones are not.
    assert_eq!(d(5 + 64), Some(string("new")));
    assert_eq!(d(5 + 65), Some(string("ch")));
    assert_eq!(list(&doc), owned(&[("new", Some(0))]));
}

/// One page (3) and a `/Dests` tree whose root is `root`, then the objects `rest`, numbered from 4.
fn shared_tree(root: &str, rest: Vec<String>) -> Document {
    let mut b: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Names << /Dests {root} >> >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    b.extend(rest);
    open(build(&b, "/Root 1 0 R"))
}

#[test]
fn name_tree_nodes_sharing_one_names_array_read_it_once() {
    // 2,000 nodes (6 on) share one /Names array (5). Read for every node, its two entries would
    // be read 4,000 times, and an edit would write all of them back.
    let n = 2_000;
    let nodes: Vec<String> = (0..n).map(|i| format!("{} 0 R", i + 6)).collect();
    let mut rest = vec![format!("<< /Kids [{}] >>", nodes.join(" ")), "[(a) [3 0 R /Fit] (b) [3 0 R /Fit]]".into()];
    rest.extend((0..n).map(|_| "<< /Names 5 0 R >>".to_string()));
    let mut doc = shared_tree("4 0 R", rest);
    crate::take_work();
    assert_eq!(list(&doc), owned(&[("a", Some(0)), ("b", Some(0))]));
    let work = crate::take_work();
    assert!(work <= n + 4, "{work} kids and entries read for {n} nodes");
    add_named_destination(&mut doc, "c", 0, None, None).unwrap();
    let keys: Vec<Vec<u8>> = tree_pairs(&doc).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
}

#[test]
fn name_tree_nodes_sharing_one_kids_array_queue_it_once() {
    // The root and 1,000 nodes (6 on) share one /Kids array (4) listing the leaf (5) and every
    // node. Queued for each of them, it would put a million kids on the walk's stack.
    let n = 1_000;
    let kids: Vec<String> = std::iter::once("5 0 R".to_string()).chain((0..n).map(|i| format!("{} 0 R", i + 6))).collect();
    let mut rest = vec![format!("[{}]", kids.join(" ")), "<< /Names [(a) [3 0 R /Fit]] >>".into()];
    rest.extend((0..n).map(|_| "<< /Kids 4 0 R >>".to_string()));
    let mut doc = shared_tree("<< /Kids 4 0 R >>", rest);
    crate::take_work();
    assert_eq!(list(&doc), owned(&[("a", Some(0))]));
    let work = crate::take_work();
    assert!(work <= n + 3, "{work} kids and entries read for {n} nodes");
    add_named_destination(&mut doc, "b", 0, None, None).unwrap();
    let keys: Vec<Vec<u8>> = tree_pairs(&doc).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, [b"a".to_vec(), b"b".to_vec()]);
}

#[test]
fn pages_sharing_one_annots_array_list_its_links_once() {
    // 500 pages share one /Annots array (3) of 500 links (4 on) naming `ch`. Listed for every
    // page, the links would be gathered 250,000 times for a rename.
    let n = 500;
    let first_page = 4 + n;
    let links: Vec<String> = (0..n).map(|i| format!("{} 0 R", i + 4)).collect();
    let pages: Vec<String> = (0..n).map(|i| format!("{} 0 R", i + first_page)).collect();
    let mut b: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Names << /Dests << /Names [(ch) [{first_page} 0 R /Fit]] >> >> >>"),
        format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 300 400] >>", pages.join(" ")),
        format!("[{}]", links.join(" ")),
    ];
    b.extend((0..n).map(|_| "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest (ch) >>".to_string()));
    b.extend((0..n).map(|_| "<< /Type /Page /Parent 2 0 R /Annots 3 0 R >>".to_string()));
    let mut doc = open(build(&b, "/Root 1 0 R"));
    crate::take_work();
    rename_named_destination(&mut doc, "ch", "new").unwrap();
    let work = crate::take_work();
    assert!(work <= n + 2, "{work} links and entries read for {n} pages");
    let dests: Vec<Option<Object>> =
        (0..n).map(|i| doc.get(ObjRef::new((i + 4) as u32, 0)).as_dict().and_then(|d| d.get(b"Dest").cloned())).collect();
    assert!(dests.iter().all(|d| *d == Some(string("new"))), "{:?}", dests.iter().find(|d| **d != Some(string("new"))));
}
