use super::items::{Action, Item, Rounded};
use super::{HashMap, Layout, MAX_DEPTH, Node, Rect, Script, date_letters, iso_to_pattern};

pub(super) fn collect_values(nodes: &[Node], out: &mut HashMap<String, String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for n in nodes {
        match n {
            Node::Field(f) => {
                if let Some(id) = &f.common.id {
                    out.insert(id.clone(), f.value.plain().unwrap_or_default());
                }
            }
            Node::Subform(s) => {
                collect_values(&s.children, out, depth + 1);
                if let Some(ps) = &s.page_set {
                    for a in &ps.areas {
                        collect_values(&a.items, out, depth + 1);
                    }
                }
            }
            Node::Area(a) => collect_values(&a.children, out, depth + 1),
            Node::ExclGroup(_) | Node::Draw(_) => {}
        }
    }
}

pub(super) fn clamp_opt(v: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let mut v = v;
    if let Some(m) = min {
        v = v.max(m);
    }
    if let Some(m) = max
        && m > 0.0
    {
        v = v.min(m);
    }
    v.max(0.0)
}

/// Does `node` flow (may be split across pages) rather than being placed as one block?
pub(super) fn splittable(node: &Node) -> bool {
    matches!(node, Node::Subform(s) if matches!(s.layout, Layout::Tb | Layout::Table) && s.common.h.is_none())
}

/// Top and bottom of an item a page break must not cut (text, a picture, a field); `None`
/// for fills and lines, which are cut at the break.
pub(super) fn solid_extent(item: &Item) -> Option<(f64, f64)> {
    match item {
        Item::Text(s) => Some((s.baseline - s.face.size * 0.8, s.baseline + s.face.size * 0.2)),
        Item::Image { rect, .. } => Some((rect.y, rect.bottom())),
        Item::Widget(w) => Some((w.rect.y, w.rect.bottom())),
        Item::Fill { .. } | Item::Line { .. } | Item::Rounded(_) => None,
    }
}

/// `item` as the part above `cut` and the part below: solid items go whole to the side their
/// top is on, fills and lines crossing the cut are cut in two.
pub(super) fn divide(item: Item, cut: f64) -> (Option<Item>, Option<Item>) {
    if let Some((top, _)) = solid_extent(&item) {
        return if top < cut - 0.01 { (Some(item), None) } else { (None, Some(item)) };
    }
    match item {
        Item::Fill { rect, color } => {
            if rect.bottom() <= cut + 0.01 {
                (Some(Item::Fill { rect, color }), None)
            } else if rect.y >= cut - 0.01 {
                (None, Some(Item::Fill { rect, color }))
            } else {
                let above = Rect::new(rect.x, rect.y, rect.w, cut - rect.y);
                let below = Rect::new(rect.x, cut, rect.w, rect.bottom() - cut);
                (Some(Item::Fill { rect: above, color }), Some(Item::Fill { rect: below, color }))
            }
        }
        Item::Line { from, to, width, color, dashed } => {
            let (lo, hi) = if from.1 <= to.1 { (from, to) } else { (to, from) };
            if lo.1 >= cut {
                (None, Some(Item::Line { from, to, width, color, dashed }))
            } else if hi.1 <= cut {
                (Some(Item::Line { from, to, width, color, dashed }), None)
            } else {
                // lo.1 < cut < hi.1, so the divisor is positive.
                let t = (cut - lo.1) / (hi.1 - lo.1);
                let mid = (lo.0 + (hi.0 - lo.0) * t, cut);
                (Some(Item::Line { from: lo, to: mid, width, color, dashed }), Some(Item::Line { from: mid, to: hi, width, color, dashed }))
            }
        }
        Item::Rounded(r) => {
            if r.rect.bottom() <= cut + 0.01 {
                (Some(Item::Rounded(r)), None)
            } else if r.rect.y >= cut - 0.01 {
                (None, Some(Item::Rounded(r)))
            } else {
                // Each part keeps the corners on its side, rounded no more than its own height
                // allows, and is open where it was cut.
                let [tl, tr, br, bl] = r.radii;
                let above = Rect::new(r.rect.x, r.rect.y, r.rect.w, cut - r.rect.y);
                let below = Rect::new(r.rect.x, cut, r.rect.w, r.rect.bottom() - cut);
                let fit = |v: f64, h: f64| v.min(h.max(0.0));
                let upper = Rounded { rect: above, radii: [fit(tl, above.h), fit(tr, above.h), 0.0, 0.0], open_bottom: true, ..(*r).clone() };
                let lower = Rounded { rect: below, radii: [0.0, 0.0, fit(br, below.h), fit(bl, below.h)], open_top: true, ..*r };
                (Some(Item::Rounded(Box::new(upper))), Some(Item::Rounded(Box::new(lower))))
            }
        }
        other => (Some(other), None),
    }
}

/// `item` moved by (`dx`, `dy`).
pub(super) fn moved(mut item: Item, dx: f64, dy: f64) -> Item {
    match &mut item {
        Item::Fill { rect, .. } | Item::Image { rect, .. } => {
            rect.x += dx;
            rect.y += dy;
        }
        Item::Rounded(r) => {
            r.rect.x += dx;
            r.rect.y += dy;
        }
        Item::Line { from, to, .. } => {
            *from = (from.0 + dx, from.1 + dy);
            *to = (to.0 + dx, to.1 + dy);
        }
        Item::Text(s) => {
            s.x += dx;
            s.baseline += dy;
        }
        Item::Widget(w) => {
            w.rect.x += dx;
            w.rect.y += dy;
        }
    }
    item
}

pub(super) fn is_row(node: &Node) -> bool {
    matches!(node, Node::Subform(s) if s.layout == Layout::Row)
}

/// The Acrobat date pattern for an XFA picture clause such as `date{YYYY-MM-DD}`.
pub(crate) fn date_pattern(picture: Option<&str>) -> String {
    picture.and_then(date_letters).unwrap_or_else(|| "yyyy-mm-dd".into())
}

/// What scripts read as a field's `formattedValue` when its display picture is `picture`: the
/// ISO date `raw` shown in a date picture (`date{YYYYMMDD}` turns `2026-10-20` into
/// `20261020`; of several alternatives, the first). `None` for other kinds of picture, for a
/// clause this can't read or show (month and weekday names) and for a value that isn't an ISO
/// date: the raw value stands then.
pub fn formatted_value(picture: &str, raw: &str) -> Option<String> {
    let first = picture.split('|').next()?.trim();
    let rest = first.strip_prefix("date")?;
    if !rest.starts_with(['{', '(']) {
        return None;
    }
    let pattern = date_pattern(Some(first));
    // A picture with parts this can't show yet (month or weekday names, `MMM` or `EEE`) would
    // leave them as letters: the raw value stands then.
    let shows = |t: &String| !t.chars().all(|c| c.is_ascii_alphabetic()) || matches!(t.as_str(), "yyyy" | "yy" | "mm" | "m" | "dd" | "d");
    if !crate::data::date_tokens(&pattern).iter().all(shows) {
        return None;
    }
    // Only the ISO date's own shape (`2026-10-20`): `2026-1-2` is not one, and stands.
    let b = raw.as_bytes();
    let iso = b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if !iso {
        return None;
    }
    let shown = iso_to_pattern(raw, &pattern);
    (shown != raw).then_some(shown)
}

/// The native PDF action a button's click script amounts to (a reset, print, save or link),
/// else the script itself to run; `None` without a click script.
pub(super) fn button_action(scripts: &[Script]) -> Option<Action> {
    let s = scripts.iter().find(|s| s.activity == "click")?;
    let t = &s.text;
    // Only a single-statement idiom becomes a native PDF action; anything else is run by the
    // XFA scripting engine.
    let one = t.trim().trim_end_matches(';');
    let single = !one.contains(';') && !one.contains('\n');
    if !single {
        return (!t.trim().is_empty()).then(|| Action::Script(t.clone()));
    }
    if t.contains("resetData") {
        Some(Action::Reset)
    } else if t.contains("host.print") || t.contains("execMenuItem(\"Print\")") {
        Some(Action::Print)
    } else if t.contains("SaveAs") || t.contains("execMenuItem(\"Save\")") {
        Some(Action::SaveAs)
    } else if let Some(i) = t.find("launchURL(") {
        let rest = &t[i + "launchURL(".len()..];
        let q = rest.find(['"', '\''])?;
        let quote = rest[q..].chars().next()?;
        let body = &rest[q + 1..];
        let end = body.find(quote)?;
        Some(Action::Url(body[..end].to_string()))
    } else if t.trim().is_empty() {
        None
    } else {
        Some(Action::Script(t.clone()))
    }
}

/// A PDF field name: no periods (they separate hierarchy levels), no control characters.
pub(super) fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control()).map(|c| if c == '.' { '_' } else { c }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "field".into() } else { s }
}
