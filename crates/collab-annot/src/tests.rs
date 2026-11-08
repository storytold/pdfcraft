use super::{
    AnnotationInput, BorderShapeWire, EndingWire, IconWire, MAX_STROKE_POINTS, MAX_STROKES, MarkWire, MarkupWire, ShapeWire, StampWire, StyleFields,
    StyleWire, WireError,
};
use pdfcraft_annot::{BorderShape, FillMark, LineEnding, Markup, NoteIcon, Shape, StampKind, Style};
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
