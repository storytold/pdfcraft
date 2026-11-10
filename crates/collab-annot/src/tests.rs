use super::{
    AnnotationInput, BorderShapeWire, EndingWire, FontSizeUpdate, GeometryUpdate, IconWire, InfoUpdate, MAX_STROKE_POINTS, MAX_STROKES, MarkWire,
    MarkupWire, MoveUpdate, PartsUpdate, ReviewStateWire, ShapeWire, StampWire, StyleFields, StyleUpdate, StyleWire, WireError, rect_from_value,
    rect_matches,
};
use pdfcraft_annot::{BorderShape, FillMark, LineEnding, Markup, NoteIcon, ReviewState, Shape, StampKind, Style};
use serde_json::json;

type BoxError = Box<dyn std::error::Error>;

fn round_trip(shape: Shape) -> Result<Shape, WireError> {
    let input = AnnotationInput { shape: shape.clone(), style: Style::default_for(&shape), contents: "text".into(), author: "ada".into() };
    let payload = input.to_payload("ada-1")?;
    assert_eq!(payload["action"], "annotation.add");
    assert_eq!(payload["nm"], "ada-1");
    assert!(payload["shape"].is_object(), "the shape is a tagged object");
    let parsed = AnnotationInput::from_payload(&payload)?;
    assert_eq!(parsed.style, input.style);
    assert_eq!(parsed.contents, "text");
    assert_eq!(parsed.author, "ada");
    Ok(parsed.shape)
}

#[test]
fn every_supported_shape_round_trips() -> Result<(), WireError> {
    let quads = vec![[72.0, 700.0, 220.0, 700.0, 72.0, 686.0, 220.0, 686.0]];
    let shapes = vec![
        Shape::Note { at: [40.0, 250.0], icon: NoteIcon::Help },
        Shape::TextMarkup { kind: Markup::Underline, quads: quads.clone() },
        Shape::Rectangle { rect: [10.0, 10.0, 80.0, 40.0] },
        Shape::Oval { rect: [10.0, 10.0, 80.0, 40.0] },
        Shape::Line { from: [1.0, 2.0], to: [30.0, 40.0], start: LineEnding::None, end: LineEnding::OpenArrow },
        Shape::Ink { strokes: vec![vec![[1.0, 1.0], [2.0, 2.0], [3.0, 1.5]]] },
        Shape::FreehandHighlight { strokes: vec![vec![[5.0, 5.0], [6.0, 7.0]]] },
        Shape::TextBox { rect: [20.0, 20.0, 200.0, 60.0], font_size: 11.0 },
        Shape::Typewriter { rect: [20.0, 20.0, 200.0, 60.0], font_size: 14.0 },
        Shape::Stamp { rect: [10.0, 10.0, 120.0, 50.0], stamp: StampKind::DynReviewed, by: Some("Ada".into()) },
        Shape::Mark { rect: [10.0, 10.0, 30.0, 30.0], mark: FillMark::Cross },
    ];
    for shape in shapes {
        assert_eq!(round_trip(shape.clone())?, shape);
    }
    Ok(())
}

#[test]
fn a_text_markup_carries_kind_quads_and_colour() -> Result<(), BoxError> {
    let shape = Shape::TextMarkup {
        kind: Markup::Highlight,
        quads: vec![[0.0, 10.0, 100.0, 10.0, 0.0, 0.0, 100.0, 0.0], [0.0, 30.0, 100.0, 30.0, 0.0, 20.0, 100.0, 20.0]],
    };
    let mut input = AnnotationInput::new(shape);
    input.style.color = [0.2, 0.4, 0.6];
    input.style.opacity = 0.5;
    let payload = input.to_payload("hl-1")?;
    assert_eq!(payload["shape"]["kind"], "text_markup");
    assert_eq!(payload["shape"]["markup"], "highlight");
    assert_eq!(payload["shape"]["quads"].as_array().map(Vec::len), Some(2));
    assert_eq!(payload["style"]["color"], json!([0.2, 0.4, 0.6]));

    let parsed = AnnotationInput::from_payload(&payload)?;
    assert_eq!(parsed.shape.subtype(), "Highlight");
    match parsed.shape {
        Shape::TextMarkup { kind, quads } => {
            assert_eq!(kind, Markup::Highlight);
            assert_eq!(quads.len(), 2);
        }
        _ => return Err("the parsed shape is not a text markup".into()),
    }
    assert!((parsed.style.opacity - 0.5).abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn a_minimal_payload_gets_the_shape_default_style() -> Result<(), BoxError> {
    let payload = json!({
        "action": "annotation.add",
        "nm": "hl-1",
        "shape": {"kind": "text_markup", "markup": "highlight", "quads": [[0.0, 10.0, 100.0, 10.0, 0.0, 0.0, 100.0, 0.0]]},
        "contents": "look"
    });
    let parsed = AnnotationInput::from_payload(&payload)?;
    assert_eq!(parsed.style, Style::default_for(&parsed.shape));
    assert_eq!(parsed.author, "");
    Ok(())
}

#[test]
fn a_partial_style_keeps_the_shape_defaults() -> Result<(), BoxError> {
    let payload = json!({
        "shape": {"kind": "ink", "strokes": [[[1.0, 1.0], [2.0, 2.0]]]},
        "style": {"color": [0.0, 0.0, 0.0]}
    });
    let parsed = AnnotationInput::from_payload(&payload)?;
    let defaults = Style::default_for(&parsed.shape);
    assert_eq!(parsed.style.color, [0.0, 0.0, 0.0]);
    assert_eq!(parsed.style.width, defaults.width);
    assert_eq!(parsed.style.opacity, defaults.opacity);
    Ok(())
}

#[test]
fn a_legacy_note_payload_still_lands() -> Result<(), BoxError> {
    let payload = json!({"action": "annotation.add", "nm": "n-1", "at": [40.0, 250.0], "contents": "hello", "author": "ada"});
    let parsed = AnnotationInput::from_payload(&payload)?;
    assert_eq!(parsed.shape, Shape::Note { at: [40.0, 250.0], icon: NoteIcon::Comment });
    assert_eq!(parsed.contents, "hello");
    Ok(())
}

#[test]
fn unsupported_shapes_are_refused() {
    let shape = Shape::Redact { quads: vec![[0.0; 8]], overlay: String::new(), look: pdfcraft_annot::OverlayLook::default() };
    assert!(matches!(ShapeWire::try_from(&shape), Err(WireError::UnsupportedShape("a redaction mark"))));

    let signature = Shape::Signature { strokes: vec![vec![[0.0, 0.0], [1.0, 1.0]]] };
    assert!(matches!(ShapeWire::try_from(&signature), Err(WireError::UnsupportedShape("a drawn signature"))));

    let payload = json!({"shape": {"kind": "polygon", "vertices": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], "cloud": false}});
    assert!(matches!(AnnotationInput::from_payload(&payload), Err(WireError::UnsupportedShape(_))));
    let signature = json!({"shape": {"kind": "signature", "strokes": [[[0.0, 0.0], [1.0, 1.0]]]}});
    assert!(matches!(AnnotationInput::from_payload(&signature), Err(WireError::UnsupportedShape(_))));
    let empty = json!({"contents": "nothing to place"});
    assert!(matches!(AnnotationInput::from_payload(&empty), Err(WireError::Malformed(_))));
}

#[test]
fn invalid_geometry_and_style_are_refused() {
    let non_finite = AnnotationInput::new(Shape::Rectangle { rect: [0.0, 0.0, f64::NAN, 10.0] });
    assert!(matches!(non_finite.validate(), Err(WireError::Invalid { field: "rect", .. })));

    let bad_opacity = json!({
        "shape": {"kind": "rectangle", "rect": [0.0, 0.0, 10.0, 10.0]},
        "style": {"color": [0.0, 0.0, 0.0], "opacity": 2.0, "width": 1.0}
    });
    assert!(matches!(AnnotationInput::from_payload(&bad_opacity), Err(WireError::Invalid { field: "opacity", .. })));

    let too_many_strokes = Shape::Ink { strokes: vec![vec![[0.0, 0.0], [1.0, 1.0]]; MAX_STROKES + 1] };
    assert!(matches!(ShapeWire::try_from(&too_many_strokes), Err(WireError::TooMany { what: "strokes", .. })));

    let long_stroke = Shape::Ink { strokes: vec![vec![[0.0, 0.0]; MAX_STROKE_POINTS + 1]] };
    assert!(matches!(ShapeWire::try_from(&long_stroke), Err(WireError::TooMany { what: "points in a stroke", .. })));
}

#[test]
fn style_fields_ignore_unknown_keys_and_keep_the_known_ones() -> Result<(), BoxError> {
    let fields: StyleFields = serde_json::from_value(json!({"color": [1.0, 0.0, 0.0], "width": 3.0, "future": true}))?;
    let shape = Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] };
    let style = fields.into_style(&shape)?;
    assert_eq!(style.color, [1.0, 0.0, 0.0]);
    assert_eq!(style.width, 3.0);
    Ok(())
}

#[test]
fn the_wire_names_are_stable() -> Result<(), serde_json::Error> {
    assert_eq!(serde_json::to_value(MarkupWire::StrikeOut)?, json!("strike_out"));
    assert_eq!(serde_json::to_value(EndingWire::ClosedArrow)?, json!("closed_arrow"));
    assert_eq!(serde_json::to_value(IconWire::CrossHairs)?, json!("cross_hairs"));
    assert_eq!(serde_json::to_value(StampWire::DynApproved)?, json!("dyn_approved"));
    assert_eq!(serde_json::to_value(MarkWire::Dot)?, json!("dot"));
    assert_eq!(serde_json::to_value(BorderShapeWire::Oval)?, json!("oval"));
    Ok(())
}

#[test]
fn a_style_wire_round_trips_through_json() -> Result<(), BoxError> {
    let shape = Shape::Stamp { rect: [0.0; 4], stamp: StampKind::Final, by: None };
    let mut style = Style::default_for(&shape);
    style.fill = Some([0.1, 0.2, 0.3]);
    style.border_color = Some([0.4, 0.5, 0.6]);
    style.border_shape = BorderShape::Oval;
    style.fill_opacity = Some(0.25);
    style.text_opacity = Some(0.75);
    let value = serde_json::to_value(StyleWire::from(&style))?;
    let fields: StyleFields = serde_json::from_value(value)?;
    assert_eq!(fields.into_style(&shape)?, style);
    Ok(())
}

// ── `annotation.update`: the final look of an existing comment ───────────────

/// The properties of a rectangle: a width, a fill, no border of its own.
fn square() -> pdfcraft_annot::Props {
    pdfcraft_annot::Props {
        subtype: "Square".into(),
        author: "ada".into(),
        subject: String::new(),
        color: Some([1.0, 0.0, 0.0]),
        opacity: 1.0,
        width: Some(1.0),
        icon: None,
        modified: None,
        restylable: true,
        locked: false,
        endings: None,
        fillable: true,
        fill: None,
        font_size: None,
        border_color: None,
        border_shape: None,
        fill_opacity: Some(1.0),
        text_opacity: None,
        rect: Some([10.0, 10.0, 80.0, 40.0]),
        frame: Some([10.0, 10.0, 80.0, 40.0]),
        points: Vec::new(),
        intent: None,
    }
}

#[test]
fn a_style_update_round_trips_and_says_what_it_clears() -> Result<(), BoxError> {
    let update = StyleUpdate {
        color: Some([0.0, 0.0, 1.0]),
        opacity: Some(0.5),
        width: Some(2.5),
        endings: Some(vec![EndingWire::OpenArrow, EndingWire::Slash]),
        fill: Some(None),
        border_color: Some(Some([0.25, 0.5, 0.75])),
        border_shape: Some(BorderShapeWire::Oval),
    };
    let payload = update.to_payload();
    assert_eq!(payload["action"], "annotation.update");
    assert_eq!(payload["style"]["fill"], serde_json::Value::Null, "a null fill takes the fill away");
    assert_eq!(payload["style"]["endings"], json!(["open_arrow", "slash"]));
    assert_eq!(StyleUpdate::from_payload(&payload)?, update);

    let restyle = update.to_restyle();
    assert_eq!(restyle.color, Some([0.0, 0.0, 1.0]));
    assert_eq!(restyle.endings, Some(vec![LineEnding::OpenArrow, LineEnding::Slash]));
    assert_eq!(restyle.fill, Some(None));
    assert_eq!(restyle.border_shape, Some(BorderShape::Oval));
    Ok(())
}

#[test]
fn a_style_update_needs_a_property_and_names_no_unknown_one() {
    assert!(matches!(StyleUpdate::from_payload(&json!({"action": "annotation.update"})), Err(WireError::Malformed(_))));
    assert!(matches!(StyleUpdate::from_value(&json!({})), Err(WireError::Malformed(_))));
    assert!(matches!(StyleUpdate::from_value(&json!(["color"])), Err(WireError::Malformed(_))));
    assert!(matches!(StyleUpdate::from_payload(&json!({"action": "annotation.update", "style": null})), Err(WireError::Malformed(_))));
    assert!(matches!(StyleUpdate::from_value(&json!({"shadow": 1})), Err(WireError::Invalid { field: "style", .. })));
    assert!(matches!(StyleUpdate::from_value(&json!({"opacity": 2.0})), Err(WireError::Invalid { field: "opacity", .. })));
    assert!(matches!(StyleUpdate::from_value(&json!({"color": [1.0, 0.0]})), Err(WireError::Invalid { field: "color", .. })));
    assert!(matches!(StyleUpdate::from_value(&json!({"width": -1.0})), Err(WireError::Invalid { field: "width", .. })));
    assert!(matches!(StyleUpdate::from_value(&json!({"border_shape": "hexagon"})), Err(WireError::Invalid { field: "border shape", .. })));
    // A comment that has the property reads back what it wrote.
    let update = StyleUpdate::from_value(&json!({"color": [0.5, 0.5, 0.5], "fill": [0.0, 1.0, 0.0]})).expect("a valid update");
    assert_eq!(update.fill, Some(Some([0.0, 1.0, 0.0])));
}

#[test]
fn line_endings_are_named_and_counted() {
    assert!(StyleUpdate::from_value(&json!({"endings": ["open_arrow"]})).is_ok(), "a callout has one ending");
    assert!(StyleUpdate::from_value(&json!({"endings": ["square", "closed_arrow"]})).is_ok(), "a line has two");
    for bad in [json!({"endings": "open_arrow"}), json!({"endings": []}), json!({"endings": ["open_arrow", "slash", "butt"]})] {
        assert!(matches!(StyleUpdate::from_value(&bad), Err(WireError::Invalid { field: "endings", .. })), "{bad}");
    }
    for bad in [json!({"endings": ["arrow"]}), json!({"endings": [1]})] {
        assert!(matches!(StyleUpdate::from_value(&bad), Err(WireError::Invalid { field: "endings", .. })), "{bad}");
    }
}

#[test]
fn a_style_update_refuses_a_property_the_comment_does_not_have() {
    let mut note = square();
    note.subtype = "Text".into();
    note.width = None;
    note.fillable = false;
    assert!(matches!(StyleUpdate { width: Some(2.0), ..StyleUpdate::default() }.check_for(&note), Err(WireError::Invalid { field: "width", .. })));
    assert!(matches!(StyleUpdate { fill: Some(None), ..StyleUpdate::default() }.check_for(&note), Err(WireError::Invalid { field: "fill", .. })));
    assert!(matches!(
        StyleUpdate { border_color: Some(None), ..StyleUpdate::default() }.check_for(&square()),
        Err(WireError::Invalid { field: "border colour", .. })
    ));
    assert!(matches!(
        StyleUpdate { border_shape: Some(BorderShapeWire::Circle), ..StyleUpdate::default() }.check_for(&square()),
        Err(WireError::Invalid { field: "border shape", .. })
    ));

    let mut locked = square();
    locked.locked = true;
    assert!(matches!(StyleUpdate { color: Some([0.0, 0.0, 0.0]), ..StyleUpdate::default() }.check_for(&locked), Err(WireError::NotRestylable(_))));
    let mut fixed = square();
    fixed.restylable = false;
    assert!(matches!(StyleUpdate { opacity: Some(0.2), ..StyleUpdate::default() }.check_for(&fixed), Err(WireError::NotRestylable(_))));

    // What the comment has, it takes: a width, a fill and a colour.
    let update = StyleUpdate { color: Some([0.0, 0.0, 1.0]), width: Some(3.0), fill: Some(Some([0.0, 1.0, 0.0])), ..StyleUpdate::default() };
    assert!(update.check_for(&square()).is_ok());

    // A line has two endings, a callout one; a note has none to change.
    let mut line = square();
    line.subtype = "Line".into();
    line.endings = Some(vec![LineEnding::None, LineEnding::OpenArrow]);
    assert!(StyleUpdate { endings: Some(vec![EndingWire::Slash, EndingWire::Square]), ..StyleUpdate::default() }.check_for(&line).is_ok());
    assert!(matches!(
        StyleUpdate { endings: Some(vec![EndingWire::Slash]), ..StyleUpdate::default() }.check_for(&line),
        Err(WireError::Invalid { field: "endings", .. })
    ));
    assert!(matches!(
        StyleUpdate { endings: Some(vec![EndingWire::Slash]), ..StyleUpdate::default() }.check_for(&note),
        Err(WireError::Invalid { field: "endings", .. })
    ));
    let mut callout = square();
    callout.subtype = "FreeText".into();
    callout.endings = Some(vec![LineEnding::OpenArrow]);
    assert!(StyleUpdate { endings: Some(vec![EndingWire::ClosedArrow]), ..StyleUpdate::default() }.check_for(&callout).is_ok());
    assert!(matches!(
        StyleUpdate { endings: Some(vec![EndingWire::Slash, EndingWire::Butt]), ..StyleUpdate::default() }.check_for(&callout),
        Err(WireError::Invalid { field: "endings", .. })
    ));
}

#[test]
fn before_records_only_what_the_update_names_and_the_undo_checks_it() {
    let props = square();
    let update = StyleUpdate { color: Some([0.0, 0.0, 1.0]), fill: Some(Some([0.0, 1.0, 0.0])), ..StyleUpdate::default() };
    let before = update.before(&props);
    assert_eq!(before, StyleUpdate { color: Some([1.0, 0.0, 0.0]), fill: Some(None), ..StyleUpdate::default() });
    // The undo is a compare-and-set on what the update left, not on the whole style.
    assert!(!update.still_matches(&props), "the look the update is about to set is not there yet");
    let mut restyled = props.clone();
    restyled.color = Some([0.0, 0.0, 1.0]);
    restyled.fill = Some([0.0, 1.0, 0.0]);
    assert!(update.still_matches(&restyled), "the look the update left is still there");
    restyled.opacity = 0.25;
    assert!(update.still_matches(&restyled), "an unnamed property does not make the undo a conflict");
    restyled.color = Some([0.0, 1.0, 0.0]);
    assert!(!update.still_matches(&restyled), "someone restyled the same comment");

    // Line endings are recorded and compared the same way.
    let mut line = square();
    line.subtype = "Line".into();
    line.endings = Some(vec![LineEnding::None, LineEnding::OpenArrow]);
    let update = StyleUpdate { endings: Some(vec![EndingWire::Butt, EndingWire::Slash]), ..StyleUpdate::default() };
    assert_eq!(update.before(&line).endings, Some(vec![EndingWire::None, EndingWire::OpenArrow]));
    assert!(!update.still_matches(&line), "the endings the update sets are not there yet");
    let mut redrawn = line.clone();
    redrawn.endings = Some(vec![LineEnding::Butt, LineEnding::Slash]);
    assert!(update.still_matches(&redrawn));
    let mut theirs = redrawn;
    theirs.endings = Some(vec![LineEnding::None, LineEnding::None]);
    assert!(!update.still_matches(&theirs), "someone changed the endings since");
}

#[test]
fn a_parts_update_round_trips_and_refuses_a_part_the_comment_has_not() -> Result<(), BoxError> {
    let update = PartsUpdate { fill: Some(0.4), text: Some(0.8) };
    let payload = update.to_payload();
    assert_eq!(payload["action"], "annotation.parts");
    assert_eq!(payload["parts"], json!({"fill": 0.4, "text": 0.8}));
    assert_eq!(PartsUpdate::from_payload(&payload)?, update);
    assert_eq!(PartsUpdate::from_value(&json!({"fill": 1.0, "text": 0.0}))?, PartsUpdate { fill: Some(1.0), text: Some(0.0) });
    assert_eq!(PartsUpdate { fill: Some(0.5), text: None }.to_value(), json!({"fill": 0.5}));

    // An opacity is 0–1, an unknown key or an empty object is refused.
    for bad in [json!({"fill": 1.5}), json!({"text": -0.1}), json!({"fill": "half"}), json!({"fill": null})] {
        assert!(PartsUpdate::from_value(&bad).is_err(), "{bad}");
    }
    assert!(matches!(PartsUpdate::from_value(&json!({"opacity": 0.5})), Err(WireError::Invalid { field: "parts", .. })));
    assert!(matches!(PartsUpdate::from_value(&json!({})), Err(WireError::Malformed(_))));
    assert!(matches!(PartsUpdate::from_payload(&json!({"action": "annotation.parts"})), Err(WireError::Malformed(_))));

    // A rectangle has a fill and no text of its own; a note has neither.
    let square = square();
    assert!(update.check_for(&square).is_err(), "a rectangle has no text of its own");
    assert!(PartsUpdate { fill: Some(0.4), text: None }.check_for(&square).is_ok());
    let mut note = square.clone();
    note.subtype = "Text".into();
    note.fillable = false;
    note.fill_opacity = None;
    assert!(matches!(PartsUpdate { fill: Some(0.4), text: None }.check_for(&note), Err(WireError::Invalid { field: "fill opacity", .. })));

    let mut text = square.clone();
    text.subtype = "FreeText".into();
    text.text_opacity = Some(1.0);
    let both = PartsUpdate { fill: Some(0.3), text: Some(1.0) };
    assert!(both.check_for(&text).is_ok());
    let mut current = text.clone();
    current.fill_opacity = Some(0.5);
    current.text_opacity = Some(0.9);
    assert_eq!(both.before(&current), PartsUpdate { fill: Some(0.5), text: Some(0.9) }, "only the parts the update names");
    assert!(!both.still_matches(&current), "the parts the update sets are not there yet");
    let mut changed = text.clone();
    changed.fill_opacity = Some(0.3);
    changed.text_opacity = Some(1.0);
    assert!(both.still_matches(&changed));
    changed.text_opacity = Some(0.2);
    assert!(!both.still_matches(&changed), "someone changed the text's opacity since");
    let mut locked = text;
    locked.locked = true;
    assert!(matches!(both.check_for(&locked), Err(WireError::NotRestylable(_))));
    Ok(())
}

#[test]
fn a_geometry_update_round_trips_a_rect_and_refuses_a_bad_one() -> Result<(), BoxError> {
    let rect = GeometryUpdate::Rect([20.0, 30.0, 90.0, 70.0]);
    let payload = rect.to_payload();
    assert_eq!(payload["action"], "annotation.geometry");
    assert_eq!(payload["geometry"], json!({"rect": [20.0, 30.0, 90.0, 70.0]}));
    assert_eq!(GeometryUpdate::from_payload(&payload)?, rect);

    // Corners come in any order; a rectangle under 1 pt and non-finite numbers are refused.
    assert_eq!(GeometryUpdate::from_value(&json!({"rect": [90.0, 70.0, 20.0, 30.0]}))?, rect);
    for bad in [
        json!({"rect": [1.0, 2.0, 3.0]}),
        json!({"rect": [10.0, 10.0, 10.5, 40.0]}),
        json!({"rect": [0.0, 0.0, true, 1.0]}),
        json!({}),
        json!({"rect": [10.0, 10.0, 50.0, 50.0], "points": [[1.0, 1.0]]}),
        json!({"radius": 3.0}),
    ] {
        assert!(GeometryUpdate::from_value(&bad).is_err(), "{bad}");
    }

    // A resize names a rectangle of a comment the engine can resize; a callout
    // whose text box can't be read only moves.
    let square = square();
    assert!(rect.check_for(&square).is_ok());
    assert_eq!(rect.before(&square), square.frame.map(GeometryUpdate::Rect));
    assert_eq!(rect.observed(&square), rect.before(&square));
    let mut frame = square.clone();
    frame.frame = Some([20.0, 30.0, 90.0, 70.0]);
    assert!(rect.still_matches(&frame));
    frame.frame = Some([20.0, 30.5, 90.0, 70.0]);
    assert!(!rect.still_matches(&frame), "someone resized the comment since");
    let mut note = square.clone();
    note.subtype = "Text".into();
    assert!(matches!(rect.check_for(&note), Err(WireError::Invalid { field: "rect", .. })));
    let mut callout = square;
    callout.subtype = "FreeText".into();
    callout.frame = None;
    assert!(matches!(rect.check_for(&callout), Err(WireError::Invalid { field: "rect", .. })));
    Ok(())
}

#[test]
fn a_geometry_update_round_trips_points_and_a_measurement_keeps_its_own() -> Result<(), BoxError> {
    let points = GeometryUpdate::Points(vec![[1.0, 2.0], [30.0, 40.0]]);
    assert_eq!(points.to_value(), json!({"points": [[1.0, 2.0], [30.0, 40.0]]}));
    assert_eq!(GeometryUpdate::from_value(&points.to_value())?, points);
    for bad in [json!({"points": []}), json!({"points": [[1.0]]}), json!({"points": [[1.0, 2.0], [3.0, "4"]]})] {
        assert!(GeometryUpdate::from_value(&bad).is_err(), "{bad}");
    }

    // A line has two ends, a polygon at least two corners, a callout one or two
    // points; a measurement keeps the points it measured.
    let square = square();
    let mut line = square.clone();
    line.subtype = "Line".into();
    line.points = vec![[1.0, 2.0], [30.0, 40.0]];
    assert!(points.check_for(&line).is_ok());
    assert!(matches!(
        GeometryUpdate::Points(vec![[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]]).check_for(&line),
        Err(WireError::Invalid { field: "points", .. })
    ));
    let mut polygon = square.clone();
    polygon.subtype = "Polygon".into();
    polygon.points = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
    assert!(GeometryUpdate::Points(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]).check_for(&polygon).is_ok());
    assert!(matches!(GeometryUpdate::Points(vec![[0.0, 0.0]]).check_for(&polygon), Err(WireError::Invalid { .. })));
    let mut callout = square;
    callout.subtype = "FreeText".into();
    callout.points = vec![[1.0, 2.0]];
    assert!(GeometryUpdate::Points(vec![[5.0, 6.0]]).check_for(&callout).is_ok());
    assert!(GeometryUpdate::Points(vec![[5.0, 6.0], [7.0, 8.0]]).check_for(&callout).is_ok());
    assert!(matches!(
        GeometryUpdate::Points(vec![[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]]).check_for(&callout),
        Err(WireError::Invalid { field: "points", .. })
    ));
    let mut dimension = line.clone();
    dimension.intent = Some("LineDimension".into());
    assert!(matches!(GeometryUpdate::Points(vec![[0.0, 0.0], [1.0, 1.0]]).check_for(&dimension), Err(WireError::NotReshapable(_))));

    // The before and the compare-and-set are the points themselves.
    let reshape = GeometryUpdate::Points(vec![[5.0, 5.0], [40.0, 50.0]]);
    assert_eq!(reshape.before(&line), Some(GeometryUpdate::Points(vec![[1.0, 2.0], [30.0, 40.0]])));
    let mut moved = line.clone();
    moved.points = vec![[5.0, 5.0], [40.0, 50.0]];
    assert!(reshape.still_matches(&moved));
    moved.points[1] = [41.0, 50.0];
    assert!(!reshape.still_matches(&moved), "someone moved an end since");
    let mut locked = line;
    locked.locked = true;
    assert!(matches!(reshape.check_for(&locked), Err(WireError::NotRestylable(_))));
    Ok(())
}

#[test]
fn a_move_round_trips_and_keeps_text_markup_in_place() -> Result<(), BoxError> {
    let update = MoveUpdate { dx: 5.0, dy: -3.0 };
    let payload = update.to_payload();
    assert_eq!(payload["action"], "annotation.move");
    assert_eq!(payload["dx"], json!(5.0));
    assert_eq!(payload["dy"], json!(-3.0));
    assert_eq!(MoveUpdate::from_payload(&payload)?, update);
    for bad in [json!({"dx": 1.0}), json!({"dy": 1.0}), json!({"dx": "5", "dy": 0.0}), json!({"dx": 1.0, "dy": null})] {
        assert!(MoveUpdate::from_payload(&bad).is_err(), "{bad}");
    }

    let square = square();
    let rect = square.rect.expect("a rectangle");
    assert_eq!(update.before(&square), Some(rect));
    assert_eq!(update.after(rect), [rect[0] + 5.0, rect[1] - 3.0, rect[2] + 5.0, rect[3] - 3.0]);
    assert!(rect_matches(rect, square.rect));
    assert!(!rect_matches(update.after(rect), square.rect));
    assert_eq!(rect_from_value(&json!(rect))?, rect);
    assert!(rect_from_value(&json!([1.0, 2.0])).is_err());
    assert!(rect_from_value(&json!([1.0, 2.0, 1.5, 4.0])).is_err(), "a rectangle under 1 pt is not one");

    // A text markup stays on its text; a locked comment stays put.
    let mut markup = square.clone();
    markup.subtype = "Highlight".into();
    assert!(matches!(update.check_for(&markup), Err(WireError::NotMovable(_))));
    let mut caret = square.clone();
    caret.subtype = "Caret".into();
    caret.intent = Some("Replace".into());
    assert!(matches!(update.check_for(&caret), Err(WireError::NotMovable(_))));
    let mut locked = square;
    locked.locked = true;
    assert!(matches!(update.check_for(&locked), Err(WireError::NotRestylable(_))));
    Ok(())
}

#[test]
fn a_font_size_update_round_trips_and_is_bounded() -> Result<(), BoxError> {
    let update = FontSizeUpdate { size: 14.5 };
    let payload = update.to_payload();
    assert_eq!(payload["action"], "annotation.font_size");
    assert_eq!(payload["size"], json!(14.5));
    assert_eq!(FontSizeUpdate::from_payload(&payload)?, update);
    assert_eq!(FontSizeUpdate::from_expect(&json!(14.5))?, update);

    // A size is a number of points in the range the engine draws.
    assert!(matches!(FontSizeUpdate::from_payload(&json!({"action": "annotation.font_size"})), Err(WireError::Malformed(_))));
    for bad in [json!("12"), json!(0.0), json!(401.0), json!(-3.0)] {
        assert!(matches!(FontSizeUpdate::from_expect(&bad), Err(WireError::Invalid { field: "font size", .. })), "{bad}");
    }

    // Only text boxes, typewriter text and callouts have a font of their own.
    let mut box_ = square();
    box_.subtype = "FreeText".into();
    box_.font_size = Some(10.0);
    assert!(update.check_for(&box_).is_ok());
    assert_eq!(update.before(&box_), Some(10.0));
    assert!(!update.still_matches(&box_));
    box_.font_size = Some(14.5);
    assert!(update.still_matches(&box_));
    assert!(matches!(update.check_for(&square()), Err(WireError::Invalid { field: "font size", .. })));

    let mut locked = box_.clone();
    locked.locked = true;
    assert!(matches!(update.check_for(&locked), Err(WireError::NotRestylable(_))));
    Ok(())
}

#[test]
fn an_info_update_round_trips_and_refuses_an_empty_name() -> Result<(), BoxError> {
    let update = InfoUpdate { author: Some(Some("ada".into())), subject: Some(Some("Review".into())), icon: Some(IconWire::Help) };
    let payload = update.to_payload();
    assert_eq!(payload["action"], "annotation.info");
    assert_eq!(payload["info"], json!({"author": "ada", "subject": "Review", "icon": "help"}));
    assert_eq!(InfoUpdate::from_payload(&payload)?, update);

    // `null` takes a property away — what the undo of a comment that had none says.
    let clear = InfoUpdate::from_value(&json!({"author": null})).expect("a valid clear");
    assert_eq!(clear.author, Some(None));
    assert_eq!(clear.to_value(), json!({"author": null}));

    // An empty string is a mistake, not a clear; an unknown property or icon is refused.
    assert!(matches!(InfoUpdate::from_value(&json!({"subject": ""})), Err(WireError::Invalid { field: "subject", .. })));
    assert!(matches!(InfoUpdate::from_value(&json!({"subject": "   "})), Err(WireError::Invalid { field: "subject", .. })));
    assert!(matches!(InfoUpdate::from_value(&json!({"author": ""})), Err(WireError::Invalid { field: "author", .. })));
    assert!(matches!(InfoUpdate::from_value(&json!({"icon": "smiley"})), Err(WireError::Invalid { field: "icon", .. })));
    assert!(matches!(InfoUpdate::from_value(&json!({"icon": 7})), Err(WireError::Invalid { field: "icon", .. })));
    assert!(matches!(InfoUpdate::from_value(&json!({"colour": "red"})), Err(WireError::Invalid { field: "info", .. })));
    assert!(matches!(InfoUpdate::from_value(&json!({})), Err(WireError::Malformed(_))));
    assert!(matches!(InfoUpdate::from_payload(&json!({"action": "annotation.info"})), Err(WireError::Malformed(_))));

    // An icon belongs to a sticky note; author and subject belong to any comment.
    let mut note = square();
    note.subtype = "Text".into();
    note.icon = Some(NoteIcon::Note);
    note.author = "bo".into();
    note.subject = String::new();
    assert!(update.check_for(&note).is_ok());
    assert_eq!(update.before(&note), InfoUpdate { author: Some(Some("bo".into())), subject: Some(None), icon: Some(IconWire::Note) });
    assert!(matches!(
        InfoUpdate { icon: Some(IconWire::Star), ..InfoUpdate::default() }.check_for(&square()),
        Err(WireError::Invalid { field: "icon", .. })
    ));
    assert!(InfoUpdate { author: Some(Some("ada".into())), ..InfoUpdate::default() }.check_for(&square()).is_ok());

    // The compare-and-set sees a change to any property it names, and only those.
    let mut changed = note.clone();
    changed.author = "ada".into();
    changed.subject = "Review".into();
    changed.icon = Some(NoteIcon::Help);
    assert!(update.still_matches(&changed));
    changed.subject = "Other".into();
    assert!(!update.still_matches(&changed), "someone changed the subject since");
    assert!(
        clear.still_matches(&{
            let mut empty = changed.clone();
            empty.author = String::new();
            empty
        }),
        "a comment without an author matches `null`"
    );
    Ok(())
}

#[test]
fn the_review_state_names_are_stable() {
    for (state, name) in [
        (ReviewState::None, "none"),
        (ReviewState::Accepted, "accepted"),
        (ReviewState::Rejected, "rejected"),
        (ReviewState::Cancelled, "cancelled"),
        (ReviewState::Completed, "completed"),
    ] {
        assert_eq!(ReviewStateWire::from(state).name(), name);
        assert_eq!(ReviewState::from(ReviewStateWire::parse(name).expect("a known state")), state);
        assert_eq!(serde_json::to_value(ReviewStateWire::from(state)).ok(), Some(json!(name)));
    }
    assert!(matches!(ReviewStateWire::parse("approved"), Err(WireError::Invalid { field: "state", .. })));
}
