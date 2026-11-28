//! A form's fields as content controls (`craft_wordmodel::control`), which Word fills in without
//! protecting the document: a text field is a plain-text control (a comb field spreads its
//! characters over its cells, and the PDF field's maximum length is named in the control's
//! title, since Word doesn't limit a control's length); a check box shows an X when checked (the
//! PDF draws the box itself); a list is a drop-down. Each stands in a paragraph framed over the
//! field's box.

use craft_wordmodel::control::{ContentControl, ControlKind, ControlLock, ListItem};
use craft_wordmodel::para::ShapeKind;
use craft_wordmodel::props::{LineSpacing, TextFrame};
use craft_wordmodel::{Align, CharProps, InlineObject, ParaProps, Paragraph, TextColor};

use super::art::{rgb, shape};
use super::{layout_number, push_text, word_font};
use crate::as_html::allowed;
use crate::layout::{Field, FieldKind, Stroke};

/// How much of a field's name its content control's title and tag keep: enough for any real
/// qualified name, and a hostile name can't make each field's paragraph grow without end.
pub(crate) const MAX_FIELD_NAME_CHARS: usize = 128;

/// Most items a list field's drop-down holds in Word; a page with a longer list keeps its first
/// items and is listed by [`crate::docx_checked`].
pub(crate) const MAX_DOCX_LIST_ITEMS: usize = 500;

/// The Word family field text in base font `font` is set in: the font's family, or Arial for a
/// field that names none or a symbol font (whose glyphs aren't letters in Word).
fn field_family(font: &str) -> String {
    word_font(font).filter(|f| !matches!(f.as_str(), "Zapf Dingbats" | "Symbol")).unwrap_or_else(|| "Arial".to_string())
}

/// The formatting of field text in Word family `family` at `size` points, its characters spread
/// `spacing` points apart.
fn chr(f: &Field, family: &str, size: f64, spacing: f64) -> CharProps {
    let half_points = (size * 2.0).round().clamp(2.0, 3276.0);
    let (bold, italic) =
        if matches!(f.kind, FieldKind::Text { .. } | FieldKind::Choice { .. }) { crate::font_weight_and_slant(&f.font) } else { (false, false) };
    let spacing = if spacing.is_finite() { (spacing * 20.0).round().clamp(-31680.0, 31680.0) / 20.0 } else { 0.0 };
    CharProps {
        font: Some(family.to_string()),
        font_ea: Some(family.to_string()),
        font_cs: Some(family.to_string()),
        size: Some(half_points as f32 / 2.0),
        bold: bold.then_some(true),
        italic: italic.then_some(true),
        color: Some(TextColor::Rgb(rgb(f.color))),
        spacing: (spacing != 0.0).then_some(spacing as f32),
        ..CharProps::default()
    }
}

/// A frame over `r` (y up) on a page `page_height` tall: Word places the paragraph exactly
/// there, out of the text flow. `down` points move the frame's top down (and its height with
/// it) so that a single line sits in the middle of the box, rather than a space before the
/// paragraph, which readers treat differently at the top of a frame or page.
fn frame(r: [f64; 4], page_height: f64, down: f64) -> TextFrame {
    let size = |v: f64| layout_number(v).clamp(1.0, 1584.0) as f32;
    let down = if down.is_finite() { down.clamp(0.0, (r[3] - r[1]).max(0.0)) } else { 0.0 };
    TextFrame {
        w: Some(size(r[2] - r[0])),
        h: Some(size(r[3] - r[1] - down)),
        h_exact: true,
        x: layout_number(r[0]) as f32,
        y: layout_number(page_height - r[3] + down) as f32,
        h_anchor: "page".into(),
        v_anchor: "page".into(),
        wrap: "around".into(),
        ..TextFrame::default()
    }
}

/// The border and fill a PDF viewer draws for a field, as a rectangle or ellipse behind the
/// text, with the border inside the field's box as the PDF draws it. `None` for a field with
/// neither, or a hostile box.
pub(super) fn look(f: &Field, page_height: f64, outlined: bool) -> Option<InlineObject> {
    // Preparation supplies borders from /MK or the displayed appearance. Absence means no
    // visible boundary, including deliberately borderless radio buttons.
    let border = f.border.filter(|_| !f.round || !outlined || f.background.is_some());
    if (border.is_none() && f.background.is_none()) || !f.valid_rect() {
        return None;
    }
    let r = f.rect;
    let width = border.map_or(0.0, |b| if b.width.is_finite() { b.width.clamp(0.25, 100.0) } else { 1.0 });
    // The line is centred on the outline: an outline half the width inside keeps it in the box.
    let inset = (width / 2.0).min(((r[2] - r[0]) / 2.0).max(0.0)).min(((r[3] - r[1]) / 2.0).max(0.0));
    let bounds = [r[0] + inset, r[1] + inset, r[2] - inset, r[3] - inset];
    let kind = if f.round { ShapeKind::Ellipse } else { ShapeKind::Rectangle };
    Some(shape(kind, bounds, page_height, f.background, border.map(|b| Stroke { width, ..b }), Vec::new()))
}

/// The characters Word accepts, of a name, title or list entry.
fn clean(text: &str) -> String {
    text.chars().filter(|c| allowed(*c)).collect()
}

/// A form field as a paragraph holding its content control, named and tagged after the field:
/// framed over the field's box, or, when `clipped` (the box reaches off the page), for a text
/// box the caller places there. `None` for a hostile box.
pub(super) fn paragraph(f: &Field, page_height: f64, clipped: bool) -> Option<Paragraph> {
    let r = f.rect;
    if !f.valid_rect() {
        return None;
    }
    let (w, h) = (layout_number(r[2] - r[0]).max(1.0), layout_number(r[3] - r[1]).max(1.0));
    let size = if f.size.is_finite() && f.size > 0.0 { f.size.clamp(2.0, 1638.0) } else { f.auto_size() };
    // One line of `size` points sits in the middle of the box.
    let middle = |size: f64| ((h - 1.15 * size) / 2.0).max(0.0);
    let label = clean(&f.name.chars().take(MAX_FIELD_NAME_CHARS).collect::<String>());
    // A read-only field's control keeps its content: Word lets it be read and copied, not changed.
    let lock = if f.read_only || matches!(f.kind, FieldKind::Radio { .. }) { ControlLock::ContentLocked } else { ControlLock::Unlocked };
    // En spaces (half an em each) keep an empty field as wide as its box.
    let blank = |n: usize| "\u{2002}".repeat(n.clamp(1, 200));
    let fits = |advance: f64| if advance > 0.0 { (w / advance).floor() as usize } else { 1 };
    let alignment = match f.alignment {
        1 => Some(Align::Center),
        2 => Some(Align::Right),
        _ => None,
    };
    let mut control = ContentControl { tag: label.clone(), title: label.clone(), lock, ..ContentControl::default() };
    let (text, rpr, frame_rect, down, indent, align) = match &f.kind {
        FieldKind::Text { value, multiline, comb, max_len } => {
            // A comb cell holds one character, centred: the run is spaced one cell per character.
            // Word's character spacing is constant. A monospaced face gives existing and
            // replacement Latin text the same 0.6 em advance, so wide/narrow letters don't drift.
            let glyph = 0.6 * size;
            let cells = comb.filter(|n| *n > 0);
            // Word adds the spacing after the last character too, which ends the run a cell past
            // the box: the frame gets that cell more, or the last character wraps out of sight.
            let (spacing, left, frame_rect) = match cells {
                Some(n) => {
                    let cell = w / n as f64;
                    (cell - glyph, ((cell - glyph) / 2.0).max(0.0), [r[0], r[1], r[2] + cell, r[3]])
                }
                None => (0.0, 1.0, r),
            };
            // An empty field shows a placeholder of en spaces as wide as its box (one per comb
            // cell), so a click anywhere in the box lands in it; typing replaces it.
            let text = if value.is_empty() {
                control.showing_placeholder = true;
                cells.map_or_else(|| blank(fits(0.5 * size)), |n| " ".repeat(n.clamp(1, 200)))
            } else if *multiline {
                value.clone()
            } else {
                // One line: a line ending in the value is a space, not a break.
                value.replace("\r\n", " ").replace(['\r', '\n'], " ")
            };
            // A content control can be filled in without protecting the document, but Word has no
            // length limit for one: the PDF's maximum length is named in the control's title,
            // which Word shows on the control.
            if let Some(n) = max_len.filter(|n| *n > 0) {
                control.title = format!("{label} (max. {n})");
            }
            control.kind = ControlKind::Text { multi_line: *multiline };
            let family = if cells.is_some() { "Courier New".to_string() } else { field_family(&f.font) };
            let before = if *multiline { 0.0 } else { middle(size) };
            (text, chr(f, &family, size, spacing), frame_rect, before, left, if cells.is_some() { None } else { alignment })
        }
        FieldKind::Check { checked, .. } | FieldKind::Radio { checked, .. } => {
            let size = (h.min(w) * 0.8).clamp(4.0, 24.0);
            // Word cannot enforce mutual exclusion between positioned checkboxes. Preserve the
            // group's displayed state, lock these controls, and report lost editability.
            if let FieldKind::Radio { buttons, .. } = &f.kind
                && *buttons > 1
            {
                control.title = format!("{label} (read-only radio group, {buttons} options)");
            }
            // The X is Arial's, whatever font the PDF's field names (often ZapfDingbats).
            control.kind = ControlKind::CheckBox {
                checked: *checked,
                checked_char: 'X',
                checked_font: "Arial".into(),
                unchecked_char: '\u{2002}',
                unchecked_font: "Arial".into(),
            };
            let text = if *checked { "X" } else { "\u{2002}" };
            (text.to_string(), chr(f, "Arial", size, 0.0), r, middle(size), 0.0, Some(Align::Center))
        }
        FieldKind::Choice { value, options, editable } => {
            let items = options.iter().take(MAX_DOCX_LIST_ITEMS).map(|o| ListItem { display: clean(o), value: clean(o) }).collect();
            let text = if value.is_empty() {
                control.showing_placeholder = true;
                blank(fits(0.5 * size))
            } else {
                value.replace("\r\n", " ").replace(['\r', '\n'], " ")
            };
            // A combo box that takes typed text is a Word combo box, which does too.
            control.kind = if *editable {
                ControlKind::ComboBox { items, last_value: String::new() }
            } else {
                ControlKind::DropDown { items, last_value: String::new() }
            };
            (text, chr(f, &field_family(&f.font), size, 0.0), r, middle(size), 1.0, alignment)
        }
    };
    // The control's own run properties are what Word gives the text typed over its placeholder.
    control.run_props = Some(rpr.clone());
    let mut p = Paragraph::new();
    p.props = ParaProps {
        space_before: Some(0.0),
        space_after: Some(0.0),
        line_spacing: Some(LineSpacing::Multiple(1.0)),
        indent_left: (indent > 0.0).then_some(indent.clamp(0.0, 1584.0) as f32),
        align,
        ..ParaProps::default()
    };
    if clipped {
        // A text box has no frame to move down: the space goes before its one paragraph.
        p.props.space_before = Some(if down.is_finite() { down.clamp(0.0, 1584.0) as f32 } else { 0.0 });
    } else {
        p.props.frame = Some(frame(frame_rect, page_height, down));
    }
    p.push_object(InlineObject::ControlStart { control: Box::new(control) }, &rpr);
    push_text(&mut p, &text, rpr.clone());
    p.push_object(InlineObject::ControlEnd, &rpr);
    Some(p)
}
