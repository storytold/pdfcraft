//! Where a link annotation is active, and where its URI action goes.
//!
//! - ISO 32000-2 §12.5.6.5, Table 176: a link's `/QuadPoints` are "the region in which the link
//!   should be activated"; when they are absent, not understood, or any coordinate lies outside
//!   `/Rect`, the activation region is `/Rect`.
//! - ISO 32000-2 §12.6.4.8, Table 211: the catalog's `/URI /Base` is "the base URI that shall be
//!   used in resolving relative URI references" in URI actions.

/// The most quadrilaterals a link's `/QuadPoints` may give before they are not used (the link is
/// then active over its `/Rect`, as for an array that isn't understood).
pub const MAX_LINK_QUADS: usize = 1024;

/// The longest `/URI /Base` that is used. A longer base would only produce an address the link
/// check refuses as too long, so it is ignored.
pub const MAX_URI_BASE_BYTES: usize = 8192;

/// How far (in points) a quadrilateral's corner may lie outside `/Rect` and still count as inside:
/// rounding in the writer, not a different region.
const RECT_SLACK: f32 = 0.01;

/// A link's quadrilaterals from its `/QuadPoints` values (`None` for an entry that isn't a
/// number), or an empty list when the link is active over its whole `/Rect`: no values, a count
/// that isn't a multiple of 8, more than [`MAX_LINK_QUADS`], a value that isn't a finite number,
/// or a corner outside `rect` (normalized, `[x0, y0, x1, y1]`).
pub fn link_quads(values: &[Option<f32>], rect: [f32; 4]) -> Vec<[f32; 8]> {
    if values.is_empty() || !values.len().is_multiple_of(8) || values.len() / 8 > MAX_LINK_QUADS {
        return Vec::new();
    }
    let mut numbers = Vec::with_capacity(values.len());
    for v in values {
        match v {
            Some(n) if n.is_finite() => numbers.push(*n),
            _ => return Vec::new(),
        }
    }
    let [x0, y0, x1, y1] = rect;
    let inside = numbers
        .as_chunks::<2>()
        .0
        .iter()
        .all(|[x, y]| *x >= x0 - RECT_SLACK && *x <= x1 + RECT_SLACK && *y >= y0 - RECT_SLACK && *y <= y1 + RECT_SLACK);
    if !inside {
        return Vec::new();
    }
    numbers.as_chunks::<8>().0.to_vec()
}

/// Whether the point `(x, y)` lies in the quadrilateral `q` (`x1 y1 … x4 y4`), edges included.
/// Writers disagree on the corners' order (the specification says counterclockwise, text-markup
/// writers often go top-left, top-right, bottom-left, bottom-right), so the quadrilateral is taken
/// as the area its four corners enclose: the union of the four triangles they make.
pub fn quad_contains(q: &[f32; 8], x: f32, y: f32) -> bool {
    if !(x.is_finite() && y.is_finite()) {
        return false;
    }
    let p = [[q[0], q[1]], [q[2], q[3]], [q[4], q[5]], [q[6], q[7]]];
    [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]].iter().any(|&[a, b, c]| match (p.get(a), p.get(b), p.get(c)) {
        (Some(a), Some(b), Some(c)) => triangle_contains(*a, *b, *c, [x, y]),
        _ => false,
    })
}

fn triangle_contains(a: [f32; 2], b: [f32; 2], c: [f32; 2], p: [f32; 2]) -> bool {
    let cross = |o: [f32; 2], u: [f32; 2], v: [f32; 2]| (u[0] - o[0]) * (v[1] - o[1]) - (u[1] - o[1]) * (v[0] - o[0]);
    // A triangle with no area holds no point (it would otherwise hold its whole line).
    if cross(a, b, c) == 0.0 {
        return false;
    }
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// Whether `s` begins with an RFC 3986 scheme (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`),
/// so it is an absolute URI rather than a relative reference.
fn has_scheme(s: &str) -> bool {
    s.split_once(':').is_some_and(|(scheme, _)| {
        scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// The address a URI action goes to when the document's catalog gives `base` (`/URI /Base`):
/// a relative reference (`page.html`, `../a/`, `?q=1`, `//host/x`) resolved against it by
/// RFC 3986 §5 (the WHATWG URL rules, which the browser applies too). `None` when the address is
/// used as written: it already has a scheme (`https:`, `mailto:`, `javascript:`…), there is no
/// usable base, or the result wouldn't be a web address.
///
/// Only a web (`http`, `https`) base is used, and the result must keep the base's scheme, so a
/// base never turns a relative link into a `file:`, `javascript:` or other address. An address
/// with control characters or whitespace other than a plain space is left as written, since the
/// URL rules would drop them and read what is left (`java\nscript:`) differently. Either way the
/// address still goes through the same check and confirmation as any other link.
pub fn resolve_uri(base: Option<&str>, raw: &str) -> Option<String> {
    let reference = raw.trim();
    let base = base?.trim();
    if reference.is_empty() || base.len() > MAX_URI_BASE_BYTES || has_scheme(reference) {
        return None;
    }
    let hidden = |s: &str| s.chars().any(|c| c.is_control() || (c.is_whitespace() && c != ' '));
    // The base must name its host as written (`https://host/…`): the URL rules would also accept
    // `https:///host/` or `https:host/`, which no link check reads as a web address.
    let names_host = base.split_once("://").is_some_and(|(_, rest)| !rest.starts_with(['/', '\\']));
    if hidden(reference) || hidden(base) || !names_host {
        return None;
    }
    let base = url::Url::parse(base).ok()?;
    if !matches!(base.scheme(), "http" | "https") || base.host().is_none() {
        return None;
    }
    let resolved = base.join(reference).ok()?;
    (resolved.scheme() == base.scheme() && resolved.host().is_some()).then(|| resolved.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.com/base/";

    #[test]
    fn relative_references_resolve_against_the_base() {
        let r = |raw: &str| resolve_uri(Some(BASE), raw);
        assert_eq!(r("page.html").as_deref(), Some("https://example.com/base/page.html"));
        assert_eq!(r("  page.html ").as_deref(), Some("https://example.com/base/page.html"), "trimmed like any link");
        assert_eq!(r("../up.html").as_deref(), Some("https://example.com/up.html"));
        assert_eq!(r("/root.html").as_deref(), Some("https://example.com/root.html"));
        assert_eq!(r("?q=1").as_deref(), Some("https://example.com/base/?q=1"));
        assert_eq!(r("#part").as_deref(), Some("https://example.com/base/#part"));
        assert_eq!(r("a/b:c").as_deref(), Some("https://example.com/base/a/b:c"), "a colon after a slash is no scheme");
        // A network-path reference names its own host; the confirmation shows that host.
        assert_eq!(r("//other.example/x").as_deref(), Some("https://other.example/x"));
        assert_eq!(resolve_uri(Some("http://example.com/dir/file.pdf"), "next.pdf").as_deref(), Some("http://example.com/dir/next.pdf"));
    }

    #[test]
    fn absolute_addresses_and_missing_bases_are_used_as_written() {
        for raw in ["https://example.org/x", "HTTP://example.org", "mailto:a@example.org", "javascript:alert(1)", "file:///etc/passwd", "C:\\x.pdf"] {
            assert_eq!(resolve_uri(Some(BASE), raw), None, "{raw}");
        }
        assert_eq!(resolve_uri(None, "page.html"), None);
        assert_eq!(resolve_uri(Some(BASE), ""), None);
        assert_eq!(resolve_uri(Some(BASE), "   "), None);
    }

    #[test]
    fn a_base_that_is_not_a_web_address_is_not_used() {
        for base in [
            "file:///C:/docs/",
            "javascript:alert(1)//",
            "data:text/html,x/",
            "mailto:a@example.org",
            "smb://server/share/",
            "relative/base/",
            "",
            "https://",
            "https:///nohost/",
            "https:nohost/",
            "https:\\\\nohost/",
            "https://exa\nmple.com/",
        ] {
            assert_eq!(resolve_uri(Some(base), "page.html"), None, "{base:?}");
        }
        let long = format!("https://example.com/{}/", "a".repeat(MAX_URI_BASE_BYTES));
        assert_eq!(resolve_uri(Some(&long), "page.html"), None);
    }

    #[test]
    fn hidden_characters_never_make_a_scheme() {
        // The URL rules drop tabs and newlines, which would read these as `javascript:`.
        for raw in ["java\nscript:alert(1)", "java\tscript:alert(1)", "\u{0}javascript:alert(1)", "page\u{2028}.html"] {
            assert_eq!(resolve_uri(Some(BASE), raw), None, "{raw:?}");
        }
    }

    #[test]
    fn quads_inside_the_rect_are_used() {
        let rect = [10.0, 10.0, 200.0, 30.0];
        let q = [10.0, 10.0, 60.0, 10.0, 60.0, 30.0, 10.0, 30.0].map(Some);
        assert_eq!(link_quads(&q, rect), vec![[10.0, 10.0, 60.0, 10.0, 60.0, 30.0, 10.0, 30.0]]);
        let two: Vec<Option<f32>> = q.iter().chain(q.iter()).copied().collect();
        assert_eq!(link_quads(&two, rect).len(), 2);
    }

    #[test]
    fn quads_that_are_not_understood_fall_back_to_the_rect() {
        let rect = [10.0, 10.0, 200.0, 30.0];
        let good = [10.0, 10.0, 60.0, 10.0, 60.0, 30.0, 10.0, 30.0];
        assert!(link_quads(&[], rect).is_empty());
        assert!(link_quads(&good[..7].iter().copied().map(Some).collect::<Vec<_>>(), rect).is_empty(), "not a multiple of 8");
        let mut odd: Vec<Option<f32>> = good.map(Some).to_vec();
        odd.push(Some(1.0));
        assert!(link_quads(&odd, rect).is_empty());
        let mut not_number = good.map(Some);
        not_number[3] = None;
        assert!(link_quads(&not_number, rect).is_empty());
        let mut nan = good.map(Some);
        nan[0] = Some(f32::NAN);
        assert!(link_quads(&nan, rect).is_empty());
        // A /Rect too large for f32 would hold an infinite corner.
        let mut infinite = good.map(Some);
        infinite[2] = Some(f32::INFINITY);
        assert!(link_quads(&infinite, [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY]).is_empty());
        let mut outside = good.map(Some);
        outside[2] = Some(250.0);
        assert!(link_quads(&outside, rect).is_empty(), "a corner outside /Rect");
        let mut below = good.map(Some);
        below[1] = Some(9.0);
        assert!(link_quads(&below, rect).is_empty(), "a corner below /Rect");
        let too_many: Vec<Option<f32>> = std::iter::repeat_n(good.map(Some), MAX_LINK_QUADS + 1).flatten().collect();
        assert!(link_quads(&too_many, rect).is_empty());
        let most: Vec<Option<f32>> = std::iter::repeat_n(good.map(Some), MAX_LINK_QUADS).flatten().collect();
        assert_eq!(link_quads(&most, rect).len(), MAX_LINK_QUADS);
    }

    #[test]
    fn a_quad_holds_its_area_in_any_corner_order() {
        // Counterclockwise from the bottom left, as the specification writes them.
        let ccw = [10.0, 10.0, 60.0, 10.0, 60.0, 30.0, 10.0, 30.0];
        // Top-left, top-right, bottom-left, bottom-right, as text-markup writers often do.
        let z = [10.0, 30.0, 60.0, 30.0, 10.0, 10.0, 60.0, 10.0];
        for q in [ccw, z] {
            assert!(quad_contains(&q, 30.0, 20.0));
            assert!(quad_contains(&q, 10.0, 10.0), "corners count");
            assert!(quad_contains(&q, 60.0, 20.0), "edges count");
            assert!(!quad_contains(&q, 100.0, 20.0), "right of the quad");
            assert!(!quad_contains(&q, 30.0, 31.0), "above the quad");
            assert!(!quad_contains(&q, 9.0, 20.0), "left of the quad");
            assert!(!quad_contains(&q, 30.0, 9.0), "below the quad");
        }
        // A tilted quad: the corner of its bounding box is outside it.
        let diamond = [50.0, 0.0, 100.0, 50.0, 50.0, 100.0, 0.0, 50.0];
        assert!(quad_contains(&diamond, 50.0, 50.0));
        assert!(!quad_contains(&diamond, 5.0, 5.0));
        // A quad with no area holds nothing.
        let flat = [0.0, 0.0, 10.0, 0.0, 20.0, 0.0, 30.0, 0.0];
        assert!(!quad_contains(&flat, 5.0, 0.0));
        assert!(!quad_contains(&ccw, f32::NAN, 20.0), "a point that isn't a number is nowhere");
    }
}
