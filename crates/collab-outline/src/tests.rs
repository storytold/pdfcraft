//! The wire shapes of an outline operation: a destination and the copy a delete
//! keeps. Every bound is proved here, because the adapter and the client both
//! refuse an operation before it touches a document.

use super::{
    OutlineWireError, check_name, check_title, copy_bytes, dest_from_value, dest_to_value, look_from_value, look_to_value, node_count, subtree_depth,
    tree_from_value, tree_to_value,
};
use pdfcraft_collab_proto::{MAX_DEST_NAME, MAX_OUTLINE_COPY_NODES, MAX_OUTLINE_DEPTH, MAX_OUTLINE_TITLE};
use pdfcraft_organize::{BookmarkDest, OutlineNode};
use serde_json::json;

fn page_dest(page: usize, left: Option<f64>, top: Option<f64>) -> BookmarkDest {
    BookmarkDest::Page { page, left, top }
}

#[test]
fn every_destination_round_trips() {
    let destinations = [
        page_dest(2, Some(40.0), None),
        page_dest(0, None, None),
        BookmarkDest::View { page: 1, kind: b"FitR".to_vec(), values: vec![Some(0.0), Some(0.0), Some(200.0), Some(300.0)] },
        BookmarkDest::Named { name: "chapter-1".into() },
        BookmarkDest::Nowhere,
    ];
    for dest in destinations {
        let value = dest_to_value(&dest);
        assert_eq!(dest_from_value(&value).expect("the wire form parses"), dest);
    }
    // The plain form a client writes by hand is the same shape.
    assert_eq!(
        dest_from_value(&json!({"kind": "page", "page": 3, "left": 10.0, "top": 720.0})).expect("parses"),
        page_dest(3, Some(10.0), Some(720.0))
    );
    assert_eq!(dest_from_value(&json!({"kind": "page", "page": 3})).expect("parses"), page_dest(3, None, None));
}

#[test]
fn a_destination_that_is_not_a_place_is_refused() {
    assert!(dest_from_value(&json!({"kind": "page"})).is_err(), "no page");
    assert!(dest_from_value(&json!({"kind": "page", "page": -1})).is_err(), "a negative page");
    assert!(dest_from_value(&json!({"kind": "page", "page": 1, "left": "here"})).is_err(), "a coordinate that is not a number");
    let infinite = serde_json::Number::from_f64(f64::INFINITY);
    assert!(infinite.is_none(), "serde_json refuses a non-finite number at the door");
    assert!(dest_from_value(&json!({"kind": "named", "name": "   "})).is_err(), "a blank name");
    assert!(dest_from_value(&json!({"kind": "named"})).is_err(), "no name");
    assert!(dest_from_value(&json!({"kind": "somewhere"})).is_err(), "an unknown kind");
    // A view the specification does not list, or one of the wrong width.
    assert!(dest_from_value(&json!({"kind": "view", "page": 1, "view": {"kind": "Far", "values": []}})).is_err());
    assert!(dest_from_value(&json!({"kind": "view", "page": 1, "view": {"kind": "FitR", "values": [0, 0, 1]}})).is_err());
    assert!(dest_from_value(&json!({"kind": "view", "page": 1})).is_err(), "no view");
    // A rectangle with a hole is not a rectangle.
    assert!(dest_from_value(&json!({"kind": "view", "page": 1, "view": {"kind": "FitR", "values": [0, 0, null, 1]}})).is_err());
    // A hole is fine where the type allows one.
    assert!(dest_from_value(&json!({"kind": "view", "page": 1, "view": {"kind": "XYZ", "values": [null, null, null]}})).is_ok());
}

#[test]
fn titles_and_names_are_bounded() {
    assert!(check_title("Start").is_ok());
    assert!(check_title("  ").is_err(), "a blank title is not a title");
    assert!(check_title(&"a".repeat(MAX_OUTLINE_TITLE)).is_ok());
    assert!(matches!(check_title(&"a".repeat(MAX_OUTLINE_TITLE + 1)), Err(OutlineWireError::TooLong { .. })));
    assert!(check_name("chapter-1").is_ok());
    assert!(check_name("").is_err());
    assert!(matches!(check_name(&"n".repeat(MAX_DEST_NAME + 1)), Err(OutlineWireError::TooLong { .. })));
}

fn node(title: &str, children: Vec<OutlineNode>) -> OutlineNode {
    OutlineNode { title: title.into(), page: Some(0), view: None, color: None, italic: false, bold: true, open: true, children }
}

#[test]
fn a_subtree_round_trips_whole() {
    let tree = vec![OutlineNode {
        title: "Part".into(),
        page: Some(1),
        view: Some((b"XYZ".to_vec(), vec![Some(10.0), Some(720.0), None])),
        color: Some([1.0, 0.5, 0.0]),
        italic: true,
        bold: true,
        open: false,
        children: vec![
            node("Chapter", Vec::new()),
            OutlineNode {
                title: "Appendix".into(),
                page: None,
                view: None,
                color: None,
                italic: false,
                bold: false,
                open: true,
                children: vec![node("Tables", Vec::new())],
            },
        ],
    }];
    let value = tree_to_value(&tree);
    let parsed = tree_from_value(&value).expect("the copy parses");
    assert_eq!(parsed, tree, "nesting, views, colours, styles and open state all come back");
    assert_eq!(node_count(&parsed), 4);
    assert_eq!(subtree_depth(&parsed), 3);
    assert!(copy_bytes(&parsed) > 0);
}

#[test]
fn a_copy_that_breaks_a_bound_is_refused() {
    // A title over the limit.
    let long = vec![node(&"t".repeat(MAX_OUTLINE_TITLE + 1), Vec::new())];
    assert!(tree_from_value(&tree_to_value(&long)).is_err());
    // Nesting deeper than the room carries.
    let mut deep = node("bottom", Vec::new());
    for level in 0..MAX_OUTLINE_DEPTH {
        deep = node(&format!("level {level}"), vec![deep]);
    }
    assert!(tree_from_value(&tree_to_value(&[deep])).is_err());
    // More nodes than one copy carries.
    let wide: Vec<OutlineNode> = (0..=MAX_OUTLINE_COPY_NODES).map(|i| node(&format!("item {i}"), Vec::new())).collect();
    assert!(matches!(tree_from_value(&tree_to_value(&wide)), Err(OutlineWireError::TooMany { .. })));
    // Not a tree, and a node that is not one.
    assert!(tree_from_value(&json!({"title": "x"})).is_err());
    assert!(tree_from_value(&json!([{"page": 0}])).is_err(), "no title");
    assert!(tree_from_value(&json!([{"title": "x", "color": [2.0, 0.0, 0.0]}])).is_err(), "a colour past 1");
    assert!(tree_from_value(&json!([{"title": "x", "children": {}}])).is_err(), "children that are not a list");
    assert!(tree_from_value(&json!([{"title": "x", "view": {"kind": "Fit", "values": [1]}}])).is_err(), "a Fit takes no values");
}

#[test]
fn an_empty_copy_is_an_empty_list() {
    assert_eq!(tree_to_value(&[]), json!([]));
    assert_eq!(tree_from_value(&json!([])).expect("parses"), Vec::new());
    assert_eq!(node_count(&[]), 0);
    assert_eq!(subtree_depth(&[]), 0);
}

#[test]
fn a_look_round_trips_and_leaves_out_what_it_did_not_name() {
    let look = look_from_value(&json!({"color": [0.1, 0.2, 0.3], "bold": true, "open": false})).expect("parses");
    assert_eq!(look.color, Some(Some([0.1, 0.2, 0.3])));
    assert_eq!(look.bold, Some(true));
    assert_eq!(look.italic, None, "a style the operation does not name is left alone");
    assert_eq!(look.open, Some(false));
    let value = look_to_value(&look);
    assert_eq!(value, json!({"color": [0.1, 0.2, 0.3], "bold": true, "open": false}));
    assert_eq!(look_from_value(&value).expect("parses"), look);
    // A colour given as null is the default black, not "leave it alone": the
    // undo of a colour has to be able to take the colour away.
    let cleared = look_from_value(&json!({"color": null})).expect("parses");
    assert_eq!(cleared.color, Some(None));
    assert_eq!(look_to_value(&cleared), json!({"color": null}));
    assert!(look_from_value(&json!({})).expect("parses").is_empty());
    assert_eq!(look_from_value(&json!({"italic": true})).expect("parses").italic, Some(true));
}

#[test]
fn a_look_that_is_not_what_it_must_be_is_refused() {
    for bad in [
        json!({"color": [1, 0]}),
        json!({"color": [0, 0, 1, 0]}),
        json!({"color": [1.1, 0, 0]}),
        json!({"color": [-0.1, 0, 0]}),
        json!({"color": ["red", 0, 0]}),
        json!({"color": "red"}),
        json!({"color": 0}),
        json!({"bold": 1}),
        json!({"italic": "yes"}),
        json!({"open": null}),
        json!({"open": 1}),
    ] {
        assert!(look_from_value(&bad).is_err(), "{bad} must be refused");
    }
    // The default colour is the one every reader draws: three components in range.
    assert!(look_from_value(&json!({"color": [0, 0, 0]})).is_ok());
    assert!(look_from_value(&json!({"color": [1, 1, 1]})).is_ok());
}
