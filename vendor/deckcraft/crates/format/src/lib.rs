//! The native `.deckcraft` format: a zip holding `presentation.json` (the model, minus media
//! bytes), `media/<id>` and `chart-parts/<index>` binary entries, and `mimetype`. Readers ignore unknown
//! fields, so newer files open in older builds with what those builds understand.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod chart_parts;
pub mod rtf;

#[cfg(test)]
mod chart_parts_tests;
#[cfg(test)]
mod golden_tests;

use std::sync::Arc;

use deckcraft_model::text::{Paragraph, TextBody};
use deckcraft_model::{LayoutType, Presentation, ShapeKind, defaults};

// The package itself (identity, manifest, member names, bounded reads) is shared.
pub use craft_deckcraft_format::{Budget, EXTENSION, FormatError, LEGACY_MIMETYPE, MIMETYPE, Result, VERSION, sniff};
use craft_deckcraft_format::{Member, Package, media_entry};

/// Serialize a presentation.
pub fn save(p: &Presentation) -> Result<Vec<u8>> {
    save_as(p, &format!("DeckCraft {}", env!("CARGO_PKG_VERSION")))
}

/// [`save`] naming `generator` in the manifest.
fn save_as(p: &Presentation, generator: &str) -> Result<Vec<u8>> {
    let mut presentation = p.clone();
    let parts = chart_parts::separate(&mut presentation)?;
    // Linked media saved without its bytes has no member.
    let media: Vec<(String, &deckcraft_model::MediaItem)> =
        p.media.iter().filter(|item| !(item.link.is_some() && item.data.is_empty())).map(|item| (media_entry(item.id.0), item)).collect();
    let members: Vec<Member<'_>> = media
        .iter()
        .map(|(name, item)| Member { name, content_type: &item.content_type, bytes: &item.data })
        .chain(parts.iter().map(|(name, content_type, bytes)| Member { name, content_type, bytes }))
        .collect();
    craft_deckcraft_format::write(&presentation, generator, &members)
}

/// A member of a package a test built or rewrote with `zip`, within `max` bytes.
#[cfg(test)]
fn read_entry_max(z: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str, max: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = z.by_name(name).map_err(|e| FormatError::Damaged(format!("{name}: {e}")))?;
    if f.size() > max {
        return Err(FormatError::Damaged(format!("{name} is too large")));
    }
    let mut out = vec![];
    f.take(max.saturating_add(1)).read_to_end(&mut out).map_err(|e| FormatError::Damaged(format!("{name}: {e}")))?;
    if out.len() as u64 > max {
        return Err(FormatError::Damaged(format!("{name} is too large")));
    }
    Ok(out)
}

#[cfg(test)]
fn read_entry(z: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    read_entry_max(z, name, craft_deckcraft_format::MAX_ENTRY)
}

/// Read a presentation. Missing media/chart parts are left empty; use [`load_checked`] to
/// learn which. A damaged chart member refuses the file before further chart inflation.
pub fn load(bytes: &[u8]) -> Result<Presentation> {
    load_checked(bytes).map(|(p, _)| p)
}

/// Read a presentation and list media that couldn't be read, plus missing chart members.
/// A chart member that is present but fails decoding, CRC or its entry limit refuses the file.
/// Linked media saved without its bytes isn't damage.
pub fn load_checked(bytes: &[u8]) -> Result<(Presentation, Vec<String>)> {
    load_within(bytes, Budget::default())
}

fn load_within(bytes: &[u8], budget: Budget) -> Result<(Presentation, Vec<String>)> {
    let (m, mut z) = Package::open_manifest::<Presentation>(bytes, budget.manifest)?;
    let mut p = m.presentation;
    let mut damaged = vec![];
    let items = p.media.len();
    if items > budget.items {
        return Err(FormatError::Damaged(format!("{} media items (at most {})", p.media.len(), budget.items)));
    }
    // A repeated id names the same member: keep the first item, so the member is inflated once
    // and the deck can be saved again (a zip can't hold two `media/<id>`).
    let mut seen = std::collections::HashSet::new();
    p.media.retain(|m| seen.insert(m.id));
    let mut left = budget.total;
    for item in &mut p.media {
        let name = media_entry(item.id.0);
        let linked_only = item.link.is_some() && !z.contains(&name);
        if z.declared_size(&name).is_some_and(|size| size > left) {
            return Err(FormatError::Damaged("embedded media is larger than DeckCraft can open".into()));
        }
        // A failed read is charged for what it inflated, so later members can't reuse that budget.
        match z.read_charged(&name, &mut left) {
            Ok(data) => item.data = Arc::new(data),
            Err(_) if linked_only => {}
            Err(e) => damaged.push(e.to_string()),
        }
    }
    chart_parts::restore(&mut p, &mut z, &mut left, items, budget.items, &mut damaged)?;
    repair(&mut p);
    Ok((p, damaged))
}

/// Make a loaded presentation safe to edit: a master exists, every slide's layout exists, tables
/// are rectangular, and duplicate ids are re-id'd — unless the deck has used every id, in which
/// case a duplicate stays (growth is refused; see the comment in the loop).
pub fn repair(p: &mut Presentation) {
    if p.masters.is_empty() {
        let fresh = defaults::blank_presentation(p.slide_size, Default::default(), false);
        p.masters = fresh.masters;
    }
    p.fix_next_id();
    let first_layout = p.masters.first().and_then(|m| m.layouts.first()).map(|l| l.id).unwrap_or_default();
    // Indexed once: a per-slide scan of every layout is slides × layouts work.
    let layouts: std::collections::HashSet<_> = p.masters.iter().flat_map(|m| m.layouts.iter().map(|l| l.id)).collect();
    let mut seen = std::collections::HashSet::new();
    for i in 0..p.slides.len() {
        let needs_layout = p.slides.get(i).is_some_and(|s| !layouts.contains(&s.layout));
        let dup = p.slides.get(i).is_some_and(|s| !seen.insert(s.id));
        if needs_layout || dup {
            // An exhausted id space leaves a duplicate slide as it is: the engine refuses to
            // grow such a deck (edits by id stay ambiguous for the duplicate), and the duplicate
            // stays visible rather than the file failing to open (#407).
            let new_id = if dup { p.alloc_id().map(deckcraft_model::SlideId) } else { None };
            if let Some(s) = p.slides.get_mut(i) {
                let s = Arc::make_mut(s);
                if needs_layout {
                    s.layout = first_layout;
                }
                if let Some(id) = new_id {
                    s.id = id;
                }
            }
        }
    }
    reid_duplicate_shapes(p);
    p.slide_size = sane_slide_size(p.slide_size);
    // The notes page only lays out notes, but its size divides too.
    if !(p.notes_size.width.is_finite() && p.notes_size.width > 0.0 && p.notes_size.height.is_finite() && p.notes_size.height > 0.0) {
        p.notes_size = Presentation::default().notes_size;
    }
    // A notes page is bounded like a slide: canvas and handout drawing scale with it.
    p.notes_size = deckcraft_model::Size::new(
        p.notes_size.width.clamp(*SLIDE_SIDE.start(), *SLIDE_SIDE.end()),
        p.notes_size.height.clamp(*SLIDE_SIDE.start(), *SLIDE_SIDE.end()),
    );
    sane_numbers(p);
}

/// Give a shape that repeats an id already used on its slide, layout or master (groups included)
/// a fresh one, so edits, selection and the Selection Pane find one shape per id. The first keeps
/// its id, and with it what animations and connectors point at. An exhausted id space leaves the
/// rest as they are (#407).
fn reid_duplicate_shapes(p: &mut Presentation) {
    fn has_duplicates(shapes: &[deckcraft_model::Shape]) -> bool {
        let mut seen = std::collections::HashSet::new();
        let mut dup = false;
        deckcraft_model::walk(shapes, &mut |sh, _| dup |= !seen.insert(sh.id));
        dup
    }
    fn reid(shapes: &mut [deckcraft_model::Shape], seen: &mut std::collections::HashSet<deckcraft_model::ShapeId>, next: &mut u32, depth: usize) {
        if depth > 64 {
            return;
        }
        for sh in shapes {
            if !seen.insert(sh.id)
                && let Some(id) = (*next).max(1).checked_add(1)
            {
                *next = id;
                sh.id = deckcraft_model::ShapeId(id);
                seen.insert(sh.id);
            }
            if let deckcraft_model::ShapeKind::Group { children, .. } = &mut sh.kind {
                reid(children, seen, next, depth + 1);
            }
        }
    }
    let mut next = p.next_id;
    let mut fix = |shapes: &mut Vec<deckcraft_model::Shape>| {
        if has_duplicates(shapes) {
            reid(shapes, &mut std::collections::HashSet::new(), &mut next, 0);
        }
    };
    for s in &mut p.slides {
        if has_duplicates(&s.shapes) {
            fix(&mut Arc::make_mut(s).shapes);
        }
    }
    for m in p.masters.iter_mut().chain(p.notes_master.iter_mut()).chain(p.handout_master.iter_mut()) {
        if has_duplicates(&m.shapes) || m.layouts.iter().any(|l| has_duplicates(&l.shapes)) {
            let m = Arc::make_mut(m);
            fix(&mut m.shapes);
            for l in &mut m.layouts {
                fix(&mut l.shapes);
            }
        }
    }
    p.next_id = next;
}

/// Smallest and largest slide side, in points: PowerPoint's 1–56 in, as Design › Slide Size allows.
pub const SLIDE_SIDE: std::ops::RangeInclusive<f64> = 72.0..=4032.0;

/// A slide size the views and renderer can always lay out: a missing, zero, negative or
/// non-finite side gives the 16:9 default, and each side is brought into [`SLIDE_SIDE`].
pub fn sane_slide_size(s: deckcraft_model::Size) -> deckcraft_model::Size {
    let ok = |v: f64| v.is_finite() && v > 0.0;
    if !(ok(s.width) && ok(s.height)) {
        return defaults::WIDE;
    }
    deckcraft_model::Size::new(s.width.clamp(*SLIDE_SIDE.start(), *SLIDE_SIDE.end()), s.height.clamp(*SLIDE_SIDE.start(), *SLIDE_SIDE.end()))
}

/// Slides read from an outline: each title with its body paragraphs (level 0–8, text).
type OutlineSlides = Vec<(String, Vec<(u8, String)>)>;

/// Append a Title and Content slide per outline entry; the number made.
fn add_outline_slides(p: &mut Presentation, slides: OutlineSlides) -> usize {
    let Some(lay) = defaults::layout_of_kind(p, LayoutType::TitleAndContent) else { return 0 };
    let mut made = 0;
    for (title, body) in slides {
        // An exhausted id space makes no new slides: what fit is kept (#407).
        let Some(mut s) = defaults::new_slide(p, lay) else { break };
        if let Some(t) = s.shapes.first_mut() {
            t.text = Some(TextBody::from_text(&title));
        }
        if let Some(b) = s.shapes.get_mut(1) {
            for (level, line) in body {
                let tb = b.text.get_or_insert_with(TextBody::default);
                let mut para = Paragraph::new(&line);
                para.level = level.min(8);
                tb.paragraphs.push(para);
            }
        }
        p.slides.push(Arc::new(s));
        made += 1;
    }
    made
}

/// Largest coordinate or size kept, in points (about 350 m; PowerPoint's own limit is far smaller).
const MAX_COORD: f64 = 1.0e6;

/// Numbers no view can draw or no file should keep: non-finite positions, sizes and angles become
/// 0, huge ones are clamped, angles are brought into 0–360, crops into a sane range, font sizes
/// into 1–4000 pt and line widths into 0–1584 pt (PowerPoint's limits).
fn sane_xfrm(x: &mut deckcraft_model::Xfrm) {
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    x.x = fin(x.x).clamp(-MAX_COORD, MAX_COORD);
    x.y = fin(x.y).clamp(-MAX_COORD, MAX_COORD);
    x.w = fin(x.w).clamp(0.0, MAX_COORD);
    x.h = fin(x.h).clamp(0.0, MAX_COORD);
    x.rot = fin(x.rot).rem_euclid(360.0);
    if x.rot >= 360.0 {
        x.rot = 0.0;
    }
}

fn sane_run(p: &mut deckcraft_model::text::RunProps) {
    p.size = p.size.filter(|v| v.is_finite() && *v > 0.0).map(|v| v.clamp(1.0, 4000.0));
    for s in p.shadow.iter_mut().chain(p.inner_shadow.iter_mut()) {
        sane_shadow(s);
    }
    if let Some(g) = p.glow.as_mut() {
        g.radius = effect_size(g.radius, MAX_GLOW);
    }
    if let Some(r) = p.reflection.as_mut() {
        sane_reflection(r);
    }
    p.soft_edge = p.soft_edge.map(|v| effect_size(v, MAX_SOFT_EDGE));
}

/// The largest effect sizes, in points: what Format Shape and `shape.effects` allow. A file can
/// say anything, and the renderer's memory grows with these (a glow of 10^18 pt asked for 34 GB).
const MAX_GLOW: f64 = 150.0;
const MAX_SOFT_EDGE: f64 = 100.0;
const MAX_SHADOW: f64 = 200.0;
const MAX_REFLECTION: f64 = 100.0;

/// `v` within 0..=`max`; not a number is 0.
fn effect_size(v: f64, max: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, max) } else { 0.0 }
}

fn sane_shadow(s: &mut deckcraft_model::style::Shadow) {
    s.blur = effect_size(s.blur, MAX_SHADOW);
    s.dist = effect_size(s.dist, MAX_SHADOW);
    s.dir = if s.dir.is_finite() { s.dir.rem_euclid(360.0) } else { 0.0 };
    // Perspective scale (1 = 100%) and skew (degrees).
    for v in [&mut s.sx, &mut s.sy] {
        *v = if v.is_finite() { v.clamp(-10.0, 10.0) } else { 1.0 };
    }
    for v in [&mut s.kx, &mut s.ky] {
        *v = if v.is_finite() { v.clamp(-89.0, 89.0) } else { 0.0 };
    }
}

fn sane_reflection(r: &mut deckcraft_model::style::Reflection) {
    r.blur = effect_size(r.blur, MAX_REFLECTION);
    r.dist = effect_size(r.dist, MAX_REFLECTION);
    if r.align.as_deref().is_some_and(|v| !matches!(v, "tl" | "t" | "tr" | "l" | "ctr" | "r" | "bl" | "b" | "br")) {
        r.align = None;
    }
}

/// Run properties a list style hands down to the text that inherits it.
fn sane_list_style(ls: &mut deckcraft_model::text::ListStyle) {
    for level in ls.levels.iter_mut().flatten() {
        sane_run(&mut level.run);
    }
}

fn sane_effects(e: &mut deckcraft_model::Effects) {
    for s in e.outer_shadow.iter_mut().chain(e.inner_shadow.iter_mut()) {
        sane_shadow(s);
    }
    if let Some(g) = e.glow.as_mut() {
        g.radius = effect_size(g.radius, MAX_GLOW);
    }
    e.soft_edge = e.soft_edge.map(|v| effect_size(v, MAX_SOFT_EDGE));
    if let Some(r) = e.reflection.as_mut() {
        sane_reflection(r);
    }
}

fn sane_shapes(shapes: &mut [deckcraft_model::Shape], depth: usize) {
    if depth > 64 {
        return;
    }
    for sh in shapes {
        if let Some(x) = sh.xfrm.as_mut() {
            sane_xfrm(x);
        }
        if let Some(l) = sh.line.as_mut() {
            l.width = l.width.filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.min(1584.0));
        }
        if let Some(e) = sh.effects.as_mut() {
            sane_effects(e);
        }
        if let Some(t) = sh.text.as_mut() {
            sane_list_style(&mut t.list_style);
            for para in &mut t.paragraphs {
                sane_run(&mut para.end_props);
                for r in &mut para.runs {
                    sane_run(&mut r.props);
                }
            }
        }
        match &mut sh.kind {
            deckcraft_model::ShapeKind::Picture { fill: deckcraft_model::style::PictureFill { crop, .. } }
            | deckcraft_model::ShapeKind::Media(deckcraft_model::MediaClip { crop, .. }) => {
                for c in crop {
                    *c = if c.is_finite() { c.clamp(-10.0, 1.0) } else { 0.0 };
                }
            }
            deckcraft_model::ShapeKind::Group { children, child } => {
                sane_xfrm(child);
                sane_shapes(children, depth + 1);
            }
            deckcraft_model::ShapeKind::Table(t) => {
                for cell in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()) {
                    sane_list_style(&mut cell.text.list_style);
                }
                for para in t.rows.iter_mut().flat_map(|r| r.cells.iter_mut()).flat_map(|c| c.text.paragraphs.iter_mut()) {
                    sane_run(&mut para.end_props);
                    for r in &mut para.runs {
                        sane_run(&mut r.props);
                    }
                }
            }
            _ => {}
        }
    }
}

/// [`sane_shapes`] over every slide, layout and master; slides are only copied when they change.
pub fn sane_numbers(p: &mut Presentation) {
    for slide in &mut p.slides {
        let mut fixed = (**slide).clone();
        sane_shapes(&mut fixed.shapes, 0);
        if fixed != **slide {
            *slide = Arc::new(fixed);
        }
    }
    // What text and shapes inherit: the deck's default text style, each master's title, body and
    // other styles, and the theme's effect styles that shape styles point at.
    sane_list_style(&mut p.default_text_style);
    for m in p.masters.iter_mut().chain(p.notes_master.iter_mut()).chain(p.handout_master.iter_mut()) {
        let mut fixed = (**m).clone();
        sane_shapes(&mut fixed.shapes, 0);
        for l in &mut fixed.layouts {
            sane_shapes(&mut l.shapes, 0);
        }
        for ls in [&mut fixed.title_style, &mut fixed.body_style, &mut fixed.other_style] {
            sane_list_style(ls);
        }
        for e in &mut fixed.theme.format.effects {
            sane_effects(e);
        }
        if fixed != **m {
            *m = Arc::new(fixed);
        }
    }
}

/// Outline text: one slide per unindented line; tab-indented lines become body bullets at their
/// level (Insert ▸ Slides from Outline, View ▸ Outline copy).
pub fn outline_to_slides(p: &mut Presentation, text: &str) -> usize {
    let mut slides: OutlineSlides = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let tabs = raw.chars().take_while(|c| *c == '\t').count();
        let spaces = raw.chars().take_while(|c| *c == ' ').count() / 2;
        let level = tabs.max(spaces);
        let t = raw.trim();
        let line = match t.strip_prefix(['-', '*', '•']) {
            Some(rest) if rest.starts_with([' ', '\t']) => rest.trim(),
            _ => t,
        }
        .to_string();
        if level == 0 {
            slides.push((line, Vec::new()));
        } else if let Some((_, body)) = slides.last_mut() {
            body.push(((level - 1).min(8) as u8, line));
        }
    }
    add_outline_slides(p, slides)
}

/// An ATX heading's text (CommonMark: up to three spaces, one to six `#`, then a space or the end;
/// a closing run of `#` is dropped).
fn md_heading(line: &str) -> Option<&str> {
    let t = line.strip_prefix("   ").or_else(|| line.strip_prefix("  ")).or_else(|| line.strip_prefix(' ')).unwrap_or(line);
    let hashes = t.chars().take_while(|c| *c == '#').count();
    let rest = t.get(hashes..)?;
    if !(1..=6).contains(&hashes) || !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let rest = rest.trim();
    let closed = rest.trim_end_matches('#');
    Some(if closed.is_empty() || closed.ends_with([' ', '\t']) { closed.trim_end() } else { rest })
}

/// A list item's nesting level (two spaces or a tab each) and text: `-`, `*` or `+`, or digits
/// then `.` or `)`, followed by a space.
fn md_list_item(line: &str) -> Option<(u8, &str)> {
    let indent: usize = line.chars().take_while(|c| matches!(c, ' ' | '\t')).map(|c| if c == '\t' { 2 } else { 1 }).sum();
    let t = line.trim_start();
    let rest = match t.strip_prefix(['-', '*', '+']) {
        Some(r) => r,
        None => {
            let digits = t.chars().take_while(char::is_ascii_digit).count();
            if digits == 0 || digits > 9 {
                return None;
            }
            t.get(digits..)?.strip_prefix(['.', ')'])?
        }
    };
    rest.starts_with([' ', '\t']).then(|| ((indent / 2).min(8) as u8, rest.trim()))
}

/// Markdown outline text (`.md`): each heading starts a slide with its text as the title; list
/// items under it become bullets nested by their indentation, other lines body paragraphs. Text
/// before the first heading is skipped, as indented lines before the first title are in the
/// plain format. A file without headings is read as a plain outline ([`outline_to_slides`]).
pub fn markdown_to_slides(p: &mut Presentation, text: &str) -> usize {
    if !text.lines().any(|l| md_heading(l).is_some()) {
        return outline_to_slides(p, text);
    }
    let mut slides: OutlineSlides = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(title) = md_heading(line) {
            slides.push((title.to_string(), Vec::new()));
        } else if let Some((_, body)) = slides.last_mut() {
            let (level, text) = md_list_item(line).unwrap_or((0, line.trim()));
            body.push((level, text.to_string()));
        }
    }
    add_outline_slides(p, slides)
}

/// The deck as outline text (titles unindented, body paragraphs tab-indented by level).
pub fn slides_to_outline(p: &Presentation) -> String {
    let mut out = String::new();
    for s in &p.slides {
        // One line per title: a newline would start another slide on re-import.
        out.push_str(&s.title().replace('\n', " "));
        out.push('\n');
        for sh in &s.shapes {
            // The title is the slide's line; footer, date and slide-number placeholders are not
            // body text (PowerPoint's outline view shows title and body placeholders only).
            if sh.ph_type().is_some_and(|k| k.is_title() || k.is_footer_kind()) {
                continue;
            }
            if let Some(t) = &sh.text {
                for para in &t.paragraphs {
                    if para.is_empty() {
                        continue;
                    }
                    for _ in 0..=para.level {
                        out.push('\t');
                    }
                    out.push_str(&para.text().replace('\u{b}', " "));
                    out.push('\n');
                }
            }
            if let ShapeKind::Table(t) = &sh.kind {
                for row in &t.rows {
                    let cells: Vec<String> = row.cells.iter().map(|c| c.text.text().replace(['\u{b}', '\n'], " ")).collect();
                    out.push('\t');
                    out.push_str(&cells.join("\t"));
                    out.push('\n');
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;

#[test]
fn outline_table_rows_are_indented_and_do_not_become_slides() {
    use deckcraft_model::table::{Cell, Row, Table};
    use deckcraft_model::{Shape, ShapeKind, Xfrm};
    let mut p = defaults::new_presentation(None);
    let layout = defaults::layout_of_kind(&p, LayoutType::TitleAndContent).unwrap();
    let mut s = defaults::new_slide(&mut p, layout).expect("slide");
    for sh in s.shapes.iter_mut().filter(|sh| sh.ph_type().is_some_and(|k| k.is_title())) {
        sh.text = Some(TextBody::from_text("Slide title"));
    }
    let mut table = Table { cols: vec![200.0, 200.0], ..Default::default() };
    for texts in [["A1", "B1"], ["A2", "B2"]] {
        let cells: Vec<Cell> = texts.iter().map(|t| Cell { text: TextBody::from_text(t), ..Default::default() }).collect();
        table.rows.push(Row { height: 30.0, cells });
    }
    let mut tbl = Shape { xfrm: Some(Xfrm::new(0.0, 100.0, 400.0, 60.0)), ..Default::default() };
    tbl.kind = ShapeKind::Table(table);
    s.shapes.push(tbl);
    p.slides = vec![std::sync::Arc::new(s)];

    let out = slides_to_outline(&p);
    let mut q = defaults::new_presentation(None);
    q.slides.clear();
    assert_eq!(outline_to_slides(&mut q, &out), 1, "table rows became slides: {out:?}");
    assert_eq!(q.slides.len(), 1, "{out:?}");
    let body = q.slides[0].shapes.iter().filter_map(|sh| sh.text.as_ref()).map(|t| t.text()).collect::<Vec<_>>().join("\n");
    assert!(body.contains("A1\tB1"), "{body:?}");
    assert!(body.contains("A2\tB2"), "{body:?}");
}
