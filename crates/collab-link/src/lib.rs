//! Typed wire form of a page's link annotations.
//!
//! The room shares links (ISO 32000-2 §12.5.6.5) as two structural operations,
//! `page.link.set` and `page.link.clear`, whose unit is one page's whole list of
//! links: a rewrite names every link the page should have, in order. This crate
//! owns the JSON one link travels as, beside the limits that bound it:
//!
//! - its **area** — the rectangle, and for a text link the quads it draws over
//!   (`/QuadPoints`); a quad list is the area, and the rectangle is its box;
//! - its **target** — a page of this document with a view, a named destination of
//!   this document, a page of another file (a remote go-to, `/S /GoToR`), or a web
//!   address;
//! - its **look** — visible or not, colour, width, dash/underline, highlight.
//!
//! Parsing and validation are pure and bounded: every number is checked, every
//! string is measured, and nothing here reads a file, a socket or a clock, so the
//! crate compiles for the browser. A link the model cannot carry is an error, never
//! a silent omission: a rewrite that dropped it would lose a destination, and no
//! undo would bring it back.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_annot::links::{Highlight, LinkAction, LinkBody, LinkStyle, LinkZoom};
use pdfcraft_collab_proto::{
    MAX_LINK_COORD, MAX_LINK_QUADS, MAX_LINK_REMOTE_FILE_BYTES, MAX_LINK_URI_BYTES, MAX_LINKS, MAX_LINKS_PER_PAGE, MAX_PAGE_INDEX, MIN_LINK_SIDE,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkWireError {
    #[error("{0} is missing")]
    Missing(&'static str),
    #[error("{0} is not what it must be")]
    Malformed(&'static str),
    #[error("{field} is {len} bytes; the limit is {max}")]
    TooLong { field: &'static str, len: usize, max: usize },
    #[error("{field}: {count} is above the limit of {max}")]
    TooMany { field: &'static str, count: usize, max: usize },
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, LinkWireError>;

fn invalid(message: impl Into<String>) -> LinkWireError {
    LinkWireError::Invalid(message.into())
}

/// A link's rectangle and the text areas it draws over, when the file gives them.
type Area = ([f64; 4], Option<Vec<[f64; 8]>>);

/// One link's area: the rectangle, and the text areas it draws over when it has
/// them. The rectangle is always the box the quads fit in, so a rewrite of the
/// same link produces the same area every time.
fn area_of(value: &Value) -> Result<Area> {
    let number = |v: &Value| v.as_f64().filter(|n| n.is_finite() && n.abs() <= MAX_LINK_COORD);
    let quads = match value.get("quads") {
        None | Some(Value::Null) => None,
        Some(Value::Array(list)) => {
            if list.is_empty() {
                return Err(invalid("a link's text areas cannot be empty"));
            }
            if list.len() > MAX_LINK_QUADS {
                return Err(LinkWireError::TooMany { field: "link areas", count: list.len(), max: MAX_LINK_QUADS });
            }
            let mut quads = Vec::with_capacity(list.len());
            for quad in list {
                let values = quad.as_array().filter(|values| values.len() == 8).ok_or(LinkWireError::Malformed("a link area is not four points"))?;
                let mut points = [0.0; 8];
                for (slot, value) in points.iter_mut().zip(values) {
                    *slot = number(value).ok_or(LinkWireError::Malformed("a link area has no finite coordinate"))?;
                }
                quads.push(points);
            }
            Some(quads)
        }
        Some(_) => return Err(LinkWireError::Malformed("link areas")),
    };
    let rect = match (&quads, value.get("rect")) {
        // The area the quads cover is their box: the wire's rectangle is not taken
        // as a second, contradicting statement of the same geometry.
        (Some(quads), _) => box_of(quads),
        (None, Some(Value::Array(values))) if values.len() == 4 => {
            let mut rect = [0.0; 4];
            for (slot, value) in rect.iter_mut().zip(values) {
                *slot = number(value).ok_or(LinkWireError::Malformed("a link area has no finite coordinate"))?;
            }
            normalise(rect)
        }
        (None, Some(_)) => return Err(LinkWireError::Malformed("a link rectangle")),
        (None, None) => return Err(LinkWireError::Missing("area")),
    };
    check_side(rect)?;
    Ok((rect, quads))
}

/// The smallest box that holds every quad: the rectangle a reader hit-tests.
fn box_of(quads: &[[f64; 8]]) -> [f64; 4] {
    let mut rect = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for quad in quads {
        for pair in quad.as_chunks::<2>().0 {
            rect[0] = rect[0].min(pair[0]);
            rect[1] = rect[1].min(pair[1]);
            rect[2] = rect[2].max(pair[0]);
            rect[3] = rect[3].max(pair[1]);
        }
    }
    normalise(rect)
}

fn normalise(rect: [f64; 4]) -> [f64; 4] {
    [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])]
}

/// A link too small to click, or one whose sides do not fit a number, is refused
/// before the document changes; the engine refuses the same shapes.
fn check_side(rect: [f64; 4]) -> Result<()> {
    let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
    if !width.is_finite() || !height.is_finite() || width < MIN_LINK_SIDE || height < MIN_LINK_SIDE {
        return Err(invalid(format!("a link's area is at least {MIN_LINK_SIDE} points on each side: this one is {width} by {height}")));
    }
    Ok(())
}

/// The zoom a target names, as the wire writes it.
fn zoom_of(value: Option<&Value>) -> Result<LinkZoom> {
    let Some(value) = value else { return Ok(LinkZoom::FitPage) };
    let kind = value.get("kind").and_then(Value::as_str).ok_or(LinkWireError::Missing("zoom kind"))?;
    Ok(match kind {
        "fit" => LinkZoom::FitPage,
        "fit_width" => LinkZoom::FitWidth,
        "fit_height" => LinkZoom::FitHeight,
        "fit_visible" => LinkZoom::FitVisible,
        "inherit" => LinkZoom::Inherit,
        "percent" => {
            let percent = value.get("percent").and_then(Value::as_f64).filter(|p| p.is_finite() && (1.0..=6400.0).contains(p));
            LinkZoom::Percent(percent.ok_or_else(|| invalid("a zoom is a percentage from 1 to 6400"))?)
        }
        other => return Err(invalid(format!("unknown zoom {other:?}"))),
    })
}

fn zoom_to_value(zoom: LinkZoom) -> Value {
    match zoom {
        LinkZoom::FitPage => json!({"kind": "fit"}),
        LinkZoom::FitWidth => json!({"kind": "fit_width"}),
        LinkZoom::FitHeight => json!({"kind": "fit_height"}),
        LinkZoom::FitVisible => json!({"kind": "fit_visible"}),
        LinkZoom::Inherit => json!({"kind": "inherit"}),
        LinkZoom::Percent(percent) => json!({"kind": "percent", "percent": percent}),
    }
}

/// Where a link goes.
fn target_of(value: &Value) -> Result<LinkAction> {
    let kind = value.get("kind").and_then(Value::as_str).ok_or(LinkWireError::Missing("target kind"))?;
    let page = |field: &'static str| -> Result<usize> {
        let index = value.get(field).and_then(Value::as_u64).ok_or(LinkWireError::Missing(field))?;
        if index > u64::from(MAX_PAGE_INDEX) {
            return Err(invalid(format!("{field} {index} is past the last page the protocol accepts")));
        }
        usize::try_from(index).map_err(|_| LinkWireError::Malformed(field))
    };
    match kind {
        "page" => Ok(LinkAction::Page { page: page("page")?, zoom: zoom_of(value.get("zoom"))? }),
        "named" => {
            let name = value.get("name").and_then(Value::as_str).map(str::trim).unwrap_or_default();
            if name.is_empty() {
                return Err(LinkWireError::Missing("named destination"));
            }
            if name.len() > pdfcraft_collab_proto::MAX_DEST_NAME {
                return Err(LinkWireError::TooLong { field: "named destination", len: name.len(), max: pdfcraft_collab_proto::MAX_DEST_NAME });
            }
            Ok(LinkAction::Named(name.to_owned()))
        }
        "uri" => {
            let uri = value.get("uri").and_then(Value::as_str).unwrap_or_default().trim();
            if uri.is_empty() {
                return Err(LinkWireError::Missing("web address"));
            }
            if uri.len() > MAX_LINK_URI_BYTES {
                return Err(LinkWireError::TooLong { field: "web address", len: uri.len(), max: MAX_LINK_URI_BYTES });
            }
            Ok(LinkAction::Uri(uri.to_owned()))
        }
        "remote" => {
            let file = value.get("file").and_then(Value::as_str).map(str::trim).unwrap_or_default();
            if file.is_empty() {
                return Err(LinkWireError::Missing("remote file"));
            }
            if file.len() > MAX_LINK_REMOTE_FILE_BYTES {
                return Err(LinkWireError::TooLong { field: "remote file", len: file.len(), max: MAX_LINK_REMOTE_FILE_BYTES });
            }
            Ok(LinkAction::Remote { file: file.to_owned(), page: page("page")?, zoom: zoom_of(value.get("zoom"))? })
        }
        other => Err(invalid(format!("unknown link target {other:?}"))),
    }
}

fn target_to_value(action: &LinkAction) -> Value {
    match action {
        LinkAction::Page { page, zoom } => json!({"kind": "page", "page": page, "zoom": zoom_to_value(*zoom)}),
        LinkAction::Named(name) => json!({"kind": "named", "name": name}),
        LinkAction::Uri(uri) => json!({"kind": "uri", "uri": uri}),
        LinkAction::Remote { file, page, zoom } => json!({"kind": "remote", "file": file, "page": page, "zoom": zoom_to_value(*zoom)}),
        // A link the model cannot carry never reaches the wire: `body_from_value`
        // refuses it, and the server refuses a page that holds one.
        LinkAction::Other(reason) => json!({"kind": "other", "reason": reason}),
    }
}

fn highlight_of(name: &str) -> Option<Highlight> {
    Some(match name {
        "none" => Highlight::None,
        "invert" => Highlight::Invert,
        "outline" => Highlight::Outline,
        "inset" => Highlight::Inset,
        _ => return None,
    })
}

fn highlight_name(highlight: Highlight) -> &'static str {
    match highlight {
        Highlight::None => "none",
        Highlight::Invert => "invert",
        Highlight::Outline => "outline",
        Highlight::Inset => "inset",
    }
}

/// A link's look. Every field is optional and defaults to what a new link has, so
/// a client that does not care about appearance sends none of them.
fn style_of(value: Option<&Value>) -> Result<LinkStyle> {
    let mut style = LinkStyle::default();
    let Some(value) = value else { return Ok(style) };
    if let Some(visible) = value.get("visible") {
        style.visible = visible.as_bool().ok_or(LinkWireError::Malformed("visible"))?;
    }
    match value.get("color") {
        None | Some(Value::Null) => {}
        Some(Value::Array(colour)) if colour.len() == 3 => {
            let mut out = [0.0; 3];
            for (slot, channel) in out.iter_mut().zip(colour) {
                *slot = channel
                    .as_f64()
                    .filter(|c| c.is_finite() && (0.0..=1.0).contains(c))
                    .ok_or_else(|| invalid("a colour is three numbers from 0 to 1"))?;
            }
            style.color = out;
        }
        Some(_) => return Err(LinkWireError::Malformed("color")),
    }
    if let Some(width) = value.get("width") {
        let width = width.as_f64().filter(|w| w.is_finite() && (0.0..=12.0).contains(w)).ok_or_else(|| invalid("a link width is 0 to 12 points"))?;
        style.width = width;
    }
    if let Some(dashed) = value.get("dashed") {
        style.dashed = dashed.as_bool().ok_or(LinkWireError::Malformed("dashed"))?;
    }
    if let Some(underline) = value.get("underline") {
        style.underline = underline.as_bool().ok_or(LinkWireError::Malformed("underline"))?;
    }
    if style.dashed && style.underline {
        return Err(invalid("a link's border is dashed or underlined, not both"));
    }
    if let Some(highlight) = value.get("highlight") {
        let name = highlight.as_str().ok_or(LinkWireError::Malformed("highlight"))?;
        style.highlight = highlight_of(name).ok_or_else(|| invalid(format!("unknown highlight {name:?}")))?;
    }
    Ok(style)
}

fn style_to_value(style: &LinkStyle) -> Value {
    json!({
        "visible": style.visible,
        "color": style.color,
        "width": style.width,
        "dashed": style.dashed,
        "underline": style.underline,
        "highlight": highlight_name(style.highlight),
    })
}

/// One link from its wire form. Every value is checked here, before an adapter
/// touches its document.
pub fn body_from_value(value: &Value) -> Result<LinkBody> {
    let (rect, quads) = area_of(value)?;
    let target = value.get("target").ok_or(LinkWireError::Missing("target"))?;
    let action = target_of(target)?;
    let style = style_of(value.get("style"))?;
    Ok(LinkBody { rect, quads, action, style })
}

/// One link as the wire writes it.
pub fn body_to_value(body: &LinkBody) -> Value {
    let mut value = json!({
        "rect": body.rect,
        "target": target_to_value(&body.action),
        "style": style_to_value(&body.style),
    });
    if let Some(quads) = &body.quads {
        value["quads"] = json!(quads);
    }
    value
}

/// A page's whole link list, in order: at most [`MAX_LINKS_PER_PAGE`] links.
pub fn bodies_from_value(value: &Value) -> Result<Vec<LinkBody>> {
    let list = value.as_array().ok_or(LinkWireError::Malformed("link list"))?;
    if list.len() > MAX_LINKS_PER_PAGE {
        return Err(LinkWireError::TooMany { field: "links on a page", count: list.len(), max: MAX_LINKS_PER_PAGE });
    }
    list.iter().map(body_from_value).collect()
}

/// A page's whole link list as the wire writes it.
pub fn bodies_to_value(bodies: &[LinkBody]) -> Value {
    Value::Array(bodies.iter().map(body_to_value).collect())
}

/// One page's entry in a whole-document link operation: the page and the links
/// the operation names for it.
#[derive(Debug, Clone, PartialEq)]
pub struct PagedLinks {
    pub page: u32,
    pub links: Vec<LinkBody>,
}

/// The body of `links.add_many` and `links.remove_many`: pages with their links,
/// `[{"page": N, "links": [...]}, …]`. An entry without `links` is a page whose
/// list is empty (what the plain removal leaves). A page appears once, its list is
/// at most [`MAX_LINKS_PER_PAGE`], and the whole operation at most [`MAX_LINKS`]
/// links and [`MAX_LINKS`] pages: the room is bounded before it reads anything.
pub fn pages_from_value(value: &Value) -> Result<Vec<PagedLinks>> {
    let list = value.as_array().ok_or(LinkWireError::Malformed("page list"))?;
    if list.len() > MAX_LINKS {
        return Err(LinkWireError::TooMany { field: "pages in one link operation", count: list.len(), max: MAX_LINKS });
    }
    let mut pages = Vec::with_capacity(list.len());
    let mut total = 0usize;
    for entry in list {
        let page = entry.get("page").and_then(Value::as_u64).ok_or(LinkWireError::Missing("page"))?;
        if page > u64::from(MAX_PAGE_INDEX) {
            return Err(invalid(format!("page {page} is past the last page the protocol accepts")));
        }
        let page = u32::try_from(page).map_err(|_| LinkWireError::Malformed("page"))?;
        if pages.iter().any(|paged: &PagedLinks| paged.page == page) {
            return Err(invalid(format!("page {} appears twice in one operation", page + 1)));
        }
        let links = match entry.get("links") {
            None | Some(Value::Null) => Vec::new(),
            Some(links) => bodies_from_value(links)?,
        };
        total = total.saturating_add(links.len());
        if total > MAX_LINKS {
            return Err(LinkWireError::TooMany { field: "links in one operation", count: total, max: MAX_LINKS });
        }
        pages.push(PagedLinks { page, links });
    }
    Ok(pages)
}

/// Pages with their links as the wire writes them: the same shape
/// [`pages_from_value`] reads, so an accepted operation and a client's intent are
/// one form.
pub fn pages_to_value(pages: &[PagedLinks]) -> Value {
    Value::Array(pages.iter().map(|paged| json!({"page": paged.page, "links": bodies_to_value(&paged.links)})).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(target: LinkAction) -> LinkBody {
        LinkBody { rect: [10.0, 20.0, 110.0, 40.0], quads: None, action: target, style: LinkStyle::default() }
    }

    #[test]
    fn every_target_round_trips() {
        let bodies = vec![
            body(LinkAction::Page { page: 3, zoom: LinkZoom::FitWidth }),
            body(LinkAction::Page { page: 0, zoom: LinkZoom::Percent(150.0) }),
            body(LinkAction::Named("chapter-2".into())),
            body(LinkAction::Uri("https://example.org/a?b=c".into())),
            body(LinkAction::Remote { file: "other.pdf".into(), page: 7, zoom: LinkZoom::FitVisible }),
        ];
        let value = bodies_to_value(&bodies);
        assert_eq!(bodies_from_value(&value).unwrap(), bodies, "the wire form is the same list");
    }

    #[test]
    fn a_text_area_is_its_box_and_travels_whole() {
        let quad = [10.0, 20.0, 110.0, 20.0, 10.0, 40.0, 110.0, 40.0];
        let value = json!({"quads": [quad, quad], "target": {"kind": "page", "page": 0}});
        let body = body_from_value(&value).unwrap();
        assert_eq!(body.rect, [10.0, 20.0, 110.0, 40.0]);
        assert_eq!(body.quads.as_deref(), Some(&[quad, quad][..]));
        assert_eq!(body.action, LinkAction::Page { page: 0, zoom: LinkZoom::FitPage });
        assert_eq!(body_from_value(&body_to_value(&body)).unwrap(), body);
    }

    #[test]
    fn a_bad_area_is_refused() {
        let target = json!({"kind": "uri", "uri": "https://example.org"});
        for (reason, area) in [
            ("no area", json!({})),
            ("too small", json!({"rect": [0.0, 0.0, 1.0, 50.0]})),
            ("not numbers", json!({"rect": [0.0, 0.0, "x", 50.0]})),
            ("not four", json!({"rect": [0.0, 0.0, 50.0]})),
            ("past the bound", json!({"rect": [0.0, 0.0, 1.0e9, 50.0]})),
            ("empty quads", json!({"quads": []})),
            ("a quad of seven", json!({"quads": [[0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 10.0]]})),
        ] {
            let mut value = area;
            value["target"] = target.clone();
            assert!(body_from_value(&value).is_err(), "{reason} must be refused");
        }
        let too_many = json!({"rect": [0.0, 0.0, 10.0, 10.0], "quads": vec![[0.0; 8]; MAX_LINK_QUADS + 1], "target": target});
        assert!(body_from_value(&too_many).is_err(), "a quad list over the bound is refused");
    }

    #[test]
    fn a_bad_target_is_refused() {
        for (reason, target) in [
            ("no kind", json!({})),
            ("unknown kind", json!({"kind": "javascript"})),
            ("past the last page", json!({"kind": "page", "page": 4_000_000_000u64})),
            ("an empty address", json!({"kind": "uri", "uri": "  "})),
            ("a long address", json!({"kind": "uri", "uri": "x".repeat(MAX_LINK_URI_BYTES + 1)})),
            ("an empty remote file", json!({"kind": "remote", "file": "", "page": 0})),
            ("a remote name", json!({"kind": "remote", "file": "other.pdf", "page": 0, "zoom": "named"})),
            ("a zoom of zero", json!({"kind": "page", "page": 0, "zoom": {"kind": "percent", "percent": 0}})),
            ("a blank name", json!({"kind": "named", "name": " "})),
        ] {
            let value = json!({"rect": [0.0, 0.0, 10.0, 10.0], "target": target});
            assert!(body_from_value(&value).is_err(), "{reason} must be refused");
        }
    }

    #[test]
    fn a_look_is_optional_and_bounded() {
        let value = json!({"rect": [0.0, 0.0, 10.0, 10.0], "target": {"kind": "uri", "uri": "https://example.org"}});
        assert_eq!(body_from_value(&value).unwrap().style, LinkStyle::default(), "a link without a look gets the usual one");
        for (reason, style) in [
            ("a colour over one", json!({"color": [0.0, 0.0, 1.5]})),
            ("a colour of two", json!({"color": [0.0, 1.0]})),
            ("a negative width", json!({"width": -1.0})),
            ("a wide border", json!({"width": 20.0})),
            ("dashed and underlined", json!({"dashed": true, "underline": true})),
            ("an unknown highlight", json!({"highlight": "blink"})),
        ] {
            let mut value = value.clone();
            value["style"] = style;
            assert!(body_from_value(&value).is_err(), "{reason} must be refused");
        }
    }

    #[test]
    fn a_list_is_bounded() {
        let body = body_to_value(&body(LinkAction::Page { page: 0, zoom: LinkZoom::FitPage }));
        let list = Value::Array(vec![body; MAX_LINKS_PER_PAGE + 1]);
        assert!(matches!(bodies_from_value(&list), Err(LinkWireError::TooMany { .. })));
        assert!(bodies_from_value(&json!({"not": "a list"})).is_err());
    }

    #[test]
    fn pages_round_trip_and_are_bounded() {
        let pages =
            vec![PagedLinks { page: 0, links: vec![body(LinkAction::Uri("https://example.org".into()))] }, PagedLinks { page: 2, links: Vec::new() }];
        let value = pages_to_value(&pages);
        assert_eq!(pages_from_value(&value).unwrap(), pages, "the same pages come back");
        assert_eq!(value[1], json!({"page": 2, "links": []}), "an empty list is a page with no links");

        // A page twice in one operation, one past the bound, and a list over the
        // room's total are all refused before a document is read.
        assert!(pages_from_value(&json!([{"page": 0, "links": []}, {"page": 0, "links": []}])).is_err());
        assert!(pages_from_value(&json!([{"page": MAX_PAGE_INDEX + 1, "links": []}])).is_err());
        let many = Value::Array((0..=MAX_LINKS).map(|page| json!({"page": page, "links": []})).collect());
        assert!(matches!(pages_from_value(&many), Err(LinkWireError::TooMany { .. })));
        let full = json!([{"page": 0, "links": Value::Array(vec![body_to_value(&body(LinkAction::Uri("https://example.org".into()))); MAX_LINKS_PER_PAGE + 1])}]);
        assert!(matches!(pages_from_value(&full), Err(LinkWireError::TooMany { .. })));
    }
}
