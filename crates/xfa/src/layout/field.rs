use super::common::{button_action, date_pattern};
use super::items::{BorderShape, Item, Widget, WidgetBorder, WidgetKind};
use super::*;

impl Layouter<'_> {
    /// Draw field `f` at `rect`: its border (a button's is its widget's box), its caption, then its
    /// widget, or its value as plain text when it is non-interactive, a barcode, or a text field
    /// taller than its content area. `radio` is set for a group's radio button; `sib` is its SOM index.
    pub(super) fn draw_field(
        &mut self,
        f: &Field,
        rect: Rect,
        ctx: &Ctx,
        radio: Option<(&str, &str, Option<String>, bool)>,
        sib: usize,
    ) -> Result<(), XfaError> {
        if let Some(b) = &f.common.border
            && f.ui != Ui::Button
        {
            self.draw_border(b, rect)?;
        }
        let fctx = ctx.child(&f.common);
        let inner = rect.inset(&f.common.margin);
        let (mut ui, mut cap_rect) = (inner, None);
        if let Some((placement, reserve)) = self.caption_reserve(f, inner.w, ctx) {
            let reserve = auto_check_reserve(f, placement, inner.w, Some(inner.h)).unwrap_or(reserve);
            let r = reserve.max(0.0);
            match placement {
                Placement::Top => {
                    cap_rect = Some(Rect::new(inner.x, inner.y, inner.w, r.min(inner.h)));
                    ui = Rect::new(inner.x, inner.y + r, inner.w, inner.h - r);
                }
                Placement::Bottom => {
                    cap_rect = Some(Rect::new(inner.x, inner.bottom() - r.min(inner.h), inner.w, r.min(inner.h)));
                    ui = Rect::new(inner.x, inner.y, inner.w, inner.h - r);
                }
                Placement::Right => {
                    cap_rect = Some(Rect::new(inner.right() - r.min(inner.w), inner.y, r.min(inner.w), inner.h));
                    ui = Rect::new(inner.x, inner.y, inner.w - r, inner.h);
                }
                _ => {
                    cap_rect = Some(Rect::new(inner.x, inner.y, r.min(inner.w), inner.h));
                    ui = Rect::new(inner.x + r, inner.y, inner.w - r, inner.h);
                }
            }
        }
        if let (Some(cr), Some(cap)) = (cap_rect, &f.caption)
            && cap.presence == Presence::Visible
        {
            let font = ctx.font.apply(&cap.font);
            let para = ctx.para.apply(&cap.para);
            self.draw_value_text(&cap.value, cr, &font, &para)?;
        }
        let som_path = || {
            if fctx.som.is_empty() {
                format!("{}[{sib}]", f.common.name.as_deref().unwrap_or("field"))
            } else {
                format!("{}.{}[{sib}]", fctx.som, f.common.name.as_deref().unwrap_or("field"))
            }
        };
        match f.ui {
            Ui::Unknown => {
                self.warn_once("a field of an unknown kind is left blank");
                return Ok(());
            }
            Ui::ImageEdit => {
                // The picture the data holds (base64, as Designer saves it) or the template's.
                let field_name = f.common.name.clone().unwrap_or_else(|| "an image field".into());
                let text: Vec<u8> = self
                    .data
                    .and_then(|d| d.text_at(&data_path(self.tpl, &som_path())))
                    .map(|t| t.bytes().filter(|b| !b.is_ascii_whitespace()).collect())
                    .unwrap_or_default();
                let from_data = if text.is_empty() {
                    None
                } else if text.len() > MAX_DATA_IMAGE / 3 * 4 + 4 {
                    self.warn_once(format!("{field_name}: the picture in the data is larger than 32 MB and is left out"));
                    None
                } else {
                    match base64::engine::general_purpose::STANDARD.decode(&text) {
                        Ok(d) if crate::image::kind(&d).is_some() => Some(d),
                        Ok(_) => {
                            self.warn_once(format!("{field_name}: the picture in the data is not a JPEG, PNG, GIF, BMP or TIFF and is left out"));
                            None
                        }
                        Err(_) => {
                            self.warn_once(format!("{field_name}: the data holds no readable picture (not base64) and is left out"));
                            None
                        }
                    }
                };
                let box_ = ui.inset(&f.ui_margin);
                match (&from_data, &f.value) {
                    (Some(d), _) => self.draw_image(d, "", box_)?,
                    (None, Value::Image { content_type, data }) => self.draw_image(data, content_type, box_)?,
                    _ => {}
                }
                // Then the field itself, which clicking gives a new picture.
            }
            Ui::Barcode => {
                // The value as text, where the bars would be.
                self.warn_once("barcode fields show their value as text, not as bars");
                let value = self
                    .data
                    .and_then(|d| d.text_at(&data_path(self.tpl, &som_path())))
                    .map(str::to_string)
                    .or_else(|| f.value.plain())
                    .unwrap_or_default();
                if !value.trim().is_empty() {
                    let mut para = fctx.para.clone();
                    para.h_align = HAlign::Center;
                    self.draw_value_text(&Value::Text(value), ui.inset(&f.ui_margin), &fctx.font, &para)?;
                }
                return Ok(());
            }
            _ => {}
        }
        if f.access == Access::NonInteractive && f.ui != Ui::Button {
            // Shown as text, not as a field.
            return self.draw_value_text(&f.value, ui.inset(&f.ui_margin), &fctx.font, &fctx.para);
        }
        // A text field that grew past what a content area holds can't be one widget: its text
        // is drawn as lines the page breaks fall between, and stays what the data holds.
        if f.grows_with_text() && rect.h > self.cursor().0.h + 0.01 {
            let value =
                self.data.and_then(|d| d.value_at(&data_path(self.tpl, &som_path()))).filter(|t| !t.trim().is_empty()).or_else(|| f.value.plain());
            if let Some(value) = value.filter(|t| !t.trim().is_empty()) {
                let name = f.common.name.clone().unwrap_or_else(|| "a text field".into());
                self.warn_once(format!("{name}: its text is longer than a page; it is shown across pages and can't be edited here"));
                let m = Margin {
                    top: f.ui_margin.top + 2.0,
                    bottom: f.ui_margin.bottom + 2.0,
                    left: f.ui_margin.left + 2.0,
                    right: f.ui_margin.right + 2.0,
                };
                let para = Para { v_align: VAlign::Top, ..fctx.para.clone() };
                // In the face it was measured in, so it takes the lines it was given.
                let face = Self::field_text_face(f, &fctx.font);
                let block = text::layout(&text::plain(value.trim_end()), ui.inset(&m), &para, &face, &|_, _| String::new());
                return self.push_block(block);
            }
        }
        let face = match (&f.ui, &f.caption) {
            // Buttons show their caption in the caption's font.
            (Ui::Button, Some(cap)) => Face::of(&ctx.font.apply(&cap.font)).unscaled(),
            _ => Face::of(&fctx.font).unscaled(),
        };
        let h_align = match (&f.ui, &f.caption) {
            (Ui::Button, Some(cap)) => ctx.para.apply(&cap.para).h_align,
            _ => fctx.para.h_align,
        };
        let name = self.unique_name(f.common.name.as_deref().unwrap_or("field"));
        let som = som_path();
        let read_only = matches!(f.access, Access::ReadOnly | Access::Protected);
        // The data's value wins over the template's default.
        let data_value: Option<String> =
            self.data.and_then(|d| d.value_at(&data_path(self.tpl, &som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        // A choice list's default is a saved value too.
        let template_default = f.value.plain().map(|v| f.saved_value(v.trim())).filter(|v| !v.is_empty());
        let plain_value = data_value.clone().or_else(|| template_default.clone());
        let (kind, rect, border, value, tooltip) = match f.ui {
            Ui::CheckButton => {
                let s = f.check_size.unwrap_or(10.0).min(ui.w.max(1.0)).min(ui.h.max(1.0));
                // Without a caption the box sits at the right of the content area, as if a
                // caption on its left took the rest, so the paragraph's alignment has no room to
                // act on; beside a caption that alignment places it. The box's own margin doesn't
                // move it.
                let x = match (cap_rect, fctx.para.h_align) {
                    (None, _) | (Some(_), HAlign::Right) => ui.right() - s,
                    (Some(_), HAlign::Center) => ui.x + (ui.w - s) / 2.0,
                    (Some(_), HAlign::Left | HAlign::Justify) => ui.x,
                };
                let y = match fctx.para.v_align {
                    VAlign::Top => ui.y,
                    VAlign::Middle => ui.y + (ui.h - s) / 2.0,
                    VAlign::Bottom => ui.bottom() - s,
                };
                let r = Rect::new(x, y, s, s);
                let on = f.items.first().cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "1".into());
                let (kind, tip, checked) = match radio {
                    Some((group, on_value, tip, chosen)) => (
                        WidgetKind::Radio { group: group.to_string(), on: on_value.to_string(), round: f.check_round },
                        tip.or_else(|| f.tooltip.clone()),
                        chosen,
                    ),
                    None => (
                        WidgetKind::CheckBox { on: on.clone(), round: f.check_round },
                        f.tooltip.clone(),
                        plain_value.as_deref().is_some_and(|v| is_on(v, &on)),
                    ),
                };
                let border = widget_border(f.ui_border.as_ref(), true);
                (kind, r, border, checked.then(|| on.clone()), tip)
            }
            Ui::Button => {
                let caption = f.caption.as_ref().and_then(|c| c.value.plain()).unwrap_or_default().trim().to_string();
                // A button's box is its field border, around the whole field (its margins pad
                // the caption, not the box); without one it is invisible (a link over text, say).
                let border = match &f.common.border {
                    Some(b) => widget_border(Some(b), false),
                    None => WidgetBorder { color: None, fill: None, shape: BorderShape::None, width: 0.0 },
                };
                (WidgetKind::Button { caption }, rect, border, None, f.tooltip.clone())
            }
            Ui::DateTimeEdit => {
                let pattern = date_pattern(f.picture.as_deref());
                // The data holds the date in the field's data picture, or else as an ISO date; the
                // field shows it in its own pattern.
                let data_pattern = data_date_letters(f);
                let shown = plain_value
                    .as_ref()
                    .map(|v| if data_value.is_some() { iso_to_pattern(&data_to_iso(v, data_pattern.as_deref()), &pattern) } else { v.clone() });
                (WidgetKind::Date(pattern), ui, widget_border(f.ui_border.as_ref(), false), shown, f.tooltip.clone())
            }
            Ui::ChoiceList => {
                let options: Vec<(String, String)> = f
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, shown)| (f.item_values.get(i).cloned().unwrap_or_else(|| shown.clone()), shown.clone()))
                    .collect();
                // The data holds saved values (several on separate lines); shown text is taken
                // too, as a user typing into an editable list leaves it.
                let value = plain_value.as_ref().map(|v| f.saved_value(v));
                let kind = WidgetKind::Choice { options, list_box: f.choice.list_box, multi: f.choice.multi, editable: f.choice.editable };
                (kind, ui, widget_border(f.ui_border.as_ref(), false), value, f.tooltip.clone())
            }
            Ui::Signature => (WidgetKind::Signature, ui, widget_border(f.ui_border.as_ref(), false), None, f.tooltip.clone()),
            Ui::ImageEdit => {
                let none = WidgetBorder { color: None, fill: None, shape: BorderShape::None, width: 0.0 };
                (WidgetKind::Image, ui, none, None, f.tooltip.clone())
            }
            Ui::PasswordEdit => (WidgetKind::Password, ui, widget_border(f.ui_border.as_ref(), false), plain_value, f.tooltip.clone()),
            _ => (WidgetKind::Text, ui, widget_border(f.ui_border.as_ref(), false), plain_value, f.tooltip.clone()),
        };
        if rect.w < 1.0 || rect.h < 1.0 {
            return Ok(());
        }
        let action = if f.ui == Ui::Button { button_action(&f.scripts) } else { None };
        let items: Vec<String> = match &kind {
            WidgetKind::CheckBox { on, .. } => vec![on.clone(), f.items.get(1).cloned().unwrap_or_default()],
            WidgetKind::Radio { on, .. } => vec![on.clone()],
            _ => Vec::new(),
        };
        self.fields += 1;
        self.push(Item::Widget(Box::new(Widget {
            rect,
            kind,
            name,
            som,
            tooltip,
            face,
            h_align,
            multiline: f.multiline,
            max_chars: f.max_chars,
            read_only,
            border,
            value,
            default: template_default,
            action,
            items,
            no_scroll: f.no_scroll,
        })))
    }
}

/// The room a caption without a reserve takes beside a check box: all the box leaves of a
/// content area `w` wide (and `h` tall, when known), the box at the size it can be drawn there.
/// `None` for any other field, or a caption with a reserve of its own.
pub(super) fn auto_check_reserve(f: &Field, placement: Placement, w: f64, h: Option<f64>) -> Option<f64> {
    if f.ui != Ui::CheckButton || f.caption.as_ref().is_none_or(|c| c.reserve.is_some()) {
        return None;
    }
    let s = f.check_size.unwrap_or(10.0).min(w.max(1.0)).min(h.map_or(f64::INFINITY, |h| h.max(1.0)));
    let rest = match (placement, h) {
        (Placement::Top | Placement::Bottom, Some(h)) => h - s,
        (Placement::Top | Placement::Bottom, None) => return None,
        _ => w - s,
    };
    Some(rest.max(0.0))
}

/// The widget's own border from the `<ui>` border element. Without one, a check box gets a
/// solid box, as Designer draws it, and any other field has no box of its own.
fn widget_border(b: Option<&Border>, check: bool) -> WidgetBorder {
    let Some(b) = b else {
        if !check {
            return WidgetBorder { color: None, fill: None, shape: BorderShape::None, width: 0.0 };
        }
        return WidgetBorder { color: Some([0.0, 0.0, 0.0]), fill: Some([1.0, 1.0, 1.0]), shape: BorderShape::Full, width: 0.5 };
    };
    let vis = b.visible_edges();
    let shape = match vis {
        [false, false, false, false] => BorderShape::None,
        [false, false, true, false] => BorderShape::Underline,
        _ => BorderShape::Full,
    };
    let edge = (0..4).find(|&i| vis[i]).map(|i| b.edges[i]).unwrap_or_default();
    WidgetBorder {
        color: (shape != BorderShape::None).then_some(edge.color.0),
        fill: b.fill().map(|c| c.0),
        shape,
        width: edge.thickness.clamp(0.1, 12.0),
    }
}
