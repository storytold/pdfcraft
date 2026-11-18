//! Running headers, footers and page numbers among auto-tagged text objects, found by repetition.
//!
//! A text object is running text when it lies wholly inside the top or bottom 15% of the page as
//! displayed, and the same text appears within 4 units of the same distance from that edge on at
//! least 40% of the pages, and at least two. Text is compared lowercase with whitespace collapsed,
//! each run of digits, and a roman numeral standing alone or after `Page`, matching any number, so
//! page numbers repeat. 40% lets alternating left- and right-page headers qualify. A group whose
//! text changes from page to page and is all heading-sized (`Chapter 1`, `Chapter 2`) is a
//! sequence of headings, not running text. Position alone never makes running text.

use std::collections::HashMap;

use crate::autotag_roles::{Evidence, HEADING_SIZE_RATIO, MAX_KEPT_CHARS};

/// Depth of the top and bottom bands, as a fraction of the page height.
const BAND: f64 = 0.15;
/// How far, in user-space units, the same running text may move from page to page.
const TOLERANCE: f64 = 4.0;

/// The page edge running text repeats at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Edge {
    Top,
    Bottom,
}

impl Edge {
    /// The artifact subtype (ISO 32000-2 Table 363) for running text at this edge.
    pub(crate) fn subtype(self) -> &'static str {
        match self {
            Edge::Top => "Header",
            Edge::Bottom => "Footer",
        }
    }
}

/// How a page is displayed: its `/Rotate` (0, 90, 180 or 270) and its crop box in user space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub rotation: i64,
    pub crop: [f64; 4],
}

/// One page's text objects, in the order of its `(BT, ET)` spans.
pub(crate) struct PageText<'a> {
    pub frame: Frame,
    pub objects: &'a [Evidence],
}

struct Member<'a> {
    page: usize,
    object: usize,
    distance: f64,
    evidence: &'a Evidence,
}

/// The edge each text object repeats at, for running text, per page and object.
pub(crate) fn find(pages: &[PageText<'_>], body: Option<f64>) -> Vec<Vec<Option<Edge>>> {
    let mut out: Vec<Vec<Option<Edge>>> = pages.iter().map(|p| vec![None; p.objects.len()]).collect();
    if pages.len() < 2 {
        return out;
    }
    let needed = pages.len().saturating_mul(2).div_ceil(5).max(2);
    for ((edge, _), mut members) in groups(pages) {
        members.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        let (best, distinct) = best_window(&members);
        if distinct < needed {
            continue;
        }
        let texts_differ = best.windows(2).any(|w| matches!(w, [a, b] if a.evidence.text != b.evidence.text));
        let heading_sized = body.is_some_and(|body| best.iter().all(|m| m.evidence.size >= body * HEADING_SIZE_RATIO));
        if texts_differ && heading_sized {
            continue;
        }
        for m in best {
            if let Some(slot) = out.get_mut(m.page).and_then(|p| p.get_mut(m.object)) {
                *slot = Some(edge);
            }
        }
    }
    out
}

/// Text objects inside a band, grouped by edge and normalised text.
fn groups<'a>(pages: &'a [PageText<'a>]) -> HashMap<(Edge, String), Vec<Member<'a>>> {
    let mut groups: HashMap<(Edge, String), Vec<Member<'a>>> = HashMap::new();
    for (page, text) in pages.iter().enumerate() {
        let (low, high) = vertical(text.frame.rotation, text.frame.crop);
        let band = (high - low) * BAND;
        for (object, evidence) in text.objects.iter().enumerate() {
            let Some(extent) = evidence.extent.filter(|_| evidence.chars > 0 && evidence.text.chars().count() < MAX_KEPT_CHARS) else {
                continue;
            };
            let (bottom, top) = vertical(text.frame.rotation, extent);
            let (edge, distance) = if bottom >= high - band && top <= high {
                (Edge::Top, high - top)
            } else if top <= low + band && bottom >= low {
                (Edge::Bottom, bottom - low)
            } else {
                continue;
            };
            let key = normalise(&evidence.text);
            if key.is_empty() || !distance.is_finite() {
                continue;
            }
            groups.entry((edge, key)).or_default().push(Member { page, object, distance, evidence });
        }
    }
    groups
}

/// The run of `members` (sorted by distance) spanning at most [`TOLERANCE`] that covers the most
/// pages, and that number of pages.
fn best_window<'m, 'a>(members: &'m [Member<'a>]) -> (&'m [Member<'a>], usize) {
    let mut on_page: HashMap<usize, usize> = HashMap::new();
    let (mut low, mut best, mut most) = (0usize, (0usize, 0usize), 0usize);
    for (high, member) in members.iter().enumerate() {
        let n = on_page.entry(member.page).or_default();
        *n = n.saturating_add(1);
        while let Some(first) = members.get(low).filter(|m| member.distance - m.distance > TOLERANCE) {
            if let Some(n) = on_page.get_mut(&first.page) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    on_page.remove(&first.page);
                }
            }
            low = low.saturating_add(1);
        }
        if on_page.len() > most {
            (most, best) = (on_page.len(), (low, high.saturating_add(1)));
        }
    }
    (members.get(best.0..best.1).unwrap_or(&[]), most)
}

/// The bottom and top of a user-space box as the page displays it.
fn vertical(rotation: i64, [x0, y0, x1, y1]: [f64; 4]) -> (f64, f64) {
    let (a, b) = match rotation {
        90 => (-x0, -x1),
        180 => (-y0, -y1),
        270 => (x0, x1),
        _ => (y0, y1),
    };
    (a.min(b), a.max(b))
}

/// Lowercase, whitespace collapsed, and each digit run, and a roman numeral standing alone or
/// after `page`, replaced by `#`.
fn normalise(text: &str) -> String {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let roman_alone = match words.as_slice() {
        [w] | ["page", w] => is_roman(w),
        _ => false,
    };
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if roman_alone && i + 1 == words.len() {
            out.push('#');
            continue;
        }
        let mut in_digits = false;
        for c in w.chars() {
            if c.is_ascii_digit() {
                if !in_digits {
                    out.push('#');
                }
                in_digits = true;
            } else {
                out.push(c);
                in_digits = false;
            }
        }
    }
    out
}

/// A canonical lowercase roman numeral from 1 to 3999 (`mid` and `iiii` are not).
fn is_roman(s: &str) -> bool {
    const UNITS: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    if s.is_empty() || s.len() > 15 {
        return false;
    }
    let (mut rest, mut value) = (s, 0u32);
    for (v, sym) in UNITS {
        let mut repeats = 0;
        while let Some(r) = rest.strip_prefix(sym) {
            (rest, value, repeats) = (r, value.saturating_add(v), repeats + 1);
            if repeats > 3 {
                return false;
            }
        }
    }
    if !rest.is_empty() || value == 0 {
        return false;
    }
    let mut canonical = String::new();
    for (v, sym) in UNITS {
        while value >= v {
            canonical.push_str(sym);
            value = value.saturating_sub(v);
        }
    }
    canonical == s
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: Frame = Frame { rotation: 0, crop: [0.0, 0.0, 612.0, 792.0] };

    /// One line of 10-unit text at baseline `y`.
    fn line(text: &str, y: f64) -> Evidence {
        Evidence {
            size: 10.0,
            lines: 1,
            chars: text.chars().filter(|c| !c.is_whitespace()).count(),
            letters: true,
            text: text.into(),
            extent: Some([72.0, y, 72.0, y + 10.0]),
            ..Evidence::default()
        }
    }

    fn run(pages: &[Vec<Evidence>], body: Option<f64>) -> Vec<Vec<Option<Edge>>> {
        let text: Vec<PageText<'_>> = pages.iter().map(|objects| PageText { frame: LETTER, objects }).collect();
        find(&text, body)
    }

    #[test]
    fn digits_and_roman_numerals_match_any_number() {
        assert_eq!(normalise("Page 12  of 30"), "page # of #");
        assert_eq!(normalise("Annual Report 2026"), "annual report #");
        assert_eq!(normalise("xiv"), "#");
        assert_eq!(normalise("Page IX"), "page #");
        assert_eq!(normalise("mid"), "mid", "not a canonical numeral");
        assert_eq!(normalise("Chapter iv"), "chapter iv", "a numeral after another word is a word");
        assert!(is_roman("mmmcmxcix") && !is_roman("iiii") && !is_roman("ic") && !is_roman(""));
    }

    #[test]
    fn a_header_and_a_numbered_footer_repeat_but_a_body_line_does_not() {
        let page = |n: usize| vec![line("Annual report", 760.0), line("Repeated body line", 400.0), line(&format!("Page {n}"), 30.0)];
        let found = run(&[page(1), page(2), page(3)], Some(10.0));
        for page in found {
            assert_eq!(page, [Some(Edge::Top), None, Some(Edge::Bottom)]);
        }
    }

    #[test]
    fn a_single_page_has_no_running_text() {
        let found = run(&[vec![line("Annual report", 760.0), line("1", 30.0)]], Some(10.0));
        assert_eq!(found, [[None, None]]);
    }

    #[test]
    fn text_on_too_few_pages_or_at_another_height_is_not_running() {
        // Five pages: the footer on one of them, the header 20 units lower on each.
        let pages: Vec<Vec<Evidence>> = (0..5u32)
            .map(|i| {
                let mut page = vec![line("Header", 760.0 - f64::from(i) * 20.0)];
                if i < 1 {
                    page.push(line("Draft", 30.0));
                }
                page
            })
            .collect();
        assert!(run(&pages, None).iter().flatten().all(Option::is_none));
        // Within the tolerance, alternate headers on two of five pages qualify.
        let pages: Vec<Vec<Evidence>> = (0..5u32).map(|i| vec![line(if i % 2 == 0 { "Left" } else { "Right" }, 760.0 + f64::from(i))]).collect();
        let found = run(&pages, None);
        assert!(found.iter().flatten().all(|e| *e == Some(Edge::Top)), "{found:?}");
    }

    #[test]
    fn distinct_pages_are_counted_not_objects() {
        // Six pages need three. Copies at 30 and 32 on the first page and one at 31 on the second
        // are three objects in a row by height, but only two pages.
        let note = |y: f64| line("Note", y);
        let mut pages = vec![vec![note(30.0), note(32.0)], vec![note(31.0)], Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        assert!(run(&pages, None).iter().flatten().all(Option::is_none));
        pages[2].push(note(31.0));
        assert!(run(&pages, None).iter().flatten().any(Option::is_some), "on three pages it runs");
    }

    #[test]
    fn heading_sized_text_that_changes_is_a_heading_not_a_header() {
        let big = |text: &str| Evidence { size: 20.0, ..line(text, 750.0) };
        let found = run(&[vec![big("Chapter 1")], vec![big("Chapter 2")]], Some(10.0));
        assert_eq!(found, [[None], [None]]);
        let found = run(&[vec![big("Annual report")], vec![big("Annual report")]], Some(10.0));
        assert_eq!(found, [[Some(Edge::Top)], [Some(Edge::Top)]], "the same large text on every page is still a header");
    }

    #[test]
    fn the_bands_follow_the_displayed_page() {
        // Turned 90° clockwise, the displayed top is the user-space left edge (x = 0).
        let turned = Frame { rotation: 90, crop: [0.0, 0.0, 612.0, 792.0] };
        let side = Evidence { extent: Some([20.0, 300.0, 30.0, 300.0]), ..line("Side title", 0.0) };
        let objects = vec![side];
        let pages = [PageText { frame: turned, objects: &objects }, PageText { frame: turned, objects: &objects }];
        assert_eq!(find(&pages, None), [[Some(Edge::Top)], [Some(Edge::Top)]]);
        assert_eq!(run(&[objects.clone(), objects], None), [[None], [None]], "upright, it sits mid-page");
    }
}
