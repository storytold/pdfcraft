use super::{Color32, CornerRadius, DocInfo, DocView, PdfCraftApp, Pos2, Rect, Sense, SnapshotKind, Stroke, Tokens, icons, vec2};

/// Whether `p` lies in the bottom-left quadrant of `area`, where the page size shows (#1855).
pub(super) fn in_size_hint_zone(area: Rect, p: Pos2) -> bool {
    p.x >= area.min.x && p.x < area.center().x && p.y > area.center().y && p.y <= area.max.y
}

/// The current page's size, floating at the bottom-left of the document area while the pointer is
/// in that quadrant, as Acrobat does. Same formatter and unit as Document Properties.
pub(super) fn page_size_hint(
    view: &DocView,
    info: &DocInfo,
    units: crate::units::Unit,
    area: Rect,
    unobstructed: bool,
    ui: &mut egui::Ui,
    t: &Tokens,
) {
    let Some(page) = info.pages.get(view.current) else { return };
    let inside = unobstructed && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| in_size_hint_zone(area, p));
    if !inside {
        return;
    }
    let text = units.size(f64::from(page.width), f64::from(page.height));
    egui::Area::new(egui::Id::unique("page-size-hint"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(area.left_bottom() + vec2(14.0, -14.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).color(t.text)).selectable(false));
                });
        });
}

/// What the notice bar's buttons ask for.
pub(super) enum Notice {
    /// Reload the file that changed on disk, dropping unsaved changes.
    Reload,
    Security,
    Signatures,
    Repairs,
    /// Highlight fields was turned on or off.
    FieldHighlights(bool),
    /// Open the JavaScript console (the form's scripts were turned off; the error is there).
    Console,
    /// Unlock… (enter the permissions password).
    Unlock,
    /// Close this historical snapshot tab (View signed version / View revision).
    Close,
    /// Stop showing the secured-document notice, in every document (#4035).
    HideSecured,
}

/// The notice bar above the pages: a file changed on disk first, then what a historical
/// snapshot is, then the signature status bar, a form's scripts having been turned
/// off, security, forms and warnings.
pub(super) fn notices(
    view: &mut DocView,
    doc: &pdfcraft_engine::Document,
    secured: bool,
    repaired: bool,
    signed: Option<(&str, Color32, &str, String)>,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> Option<Notice> {
    let info = &doc.info;
    let xfa = doc.xfa.as_ref();
    // The file changed on disk while this document has unsaved changes (#431): ask before
    // replacing it, ahead of every other notice.
    if view.disk_changed {
        let mut reload = false;
        egui::Frame::NONE.fill(t.accent_soft).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(icons::image("rotate-cw", 16.0, t.accent_text));
                ui.label(
                    egui::RichText::new(tl!(
                        "This file changed on disk. Reload shows the new version and drops your unsaved changes; Save replaces it with this version."
                    ))
                    .color(t.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, "x", 22.0, false, tl!("Dismiss")).clicked() {
                        view.disk_changed = false;
                    }
                    reload = crate::widgets::pill_button(ui, tl!("Reload"), false).clicked();
                });
            });
        });
        return reload.then_some(Notice::Reload);
    }
    // A historical snapshot (View signed version / View revision): what it is, above the
    // signature and security bars, so the tab is never mistaken for the live document.
    if let Some(snap) = view.snapshot.as_ref() {
        let mut close = false;
        let text = if snap.kind == SnapshotKind::SignedVersion {
            crate::i18n::fmt(
                tl!("You are viewing the signed version of “{from}” (revision {rev}). It is a snapshot: changes here won't affect the original."),
                &[("from", &snap.from), ("rev", &snap.revision.to_string())],
            )
        } else {
            crate::i18n::fmt(
                tl!("You are viewing revision {rev} of “{from}”. It is a snapshot: changes here won't affect the original."),
                &[("rev", &snap.revision.to_string()), ("from", &snap.from)],
            )
        };
        notice_bar(ui, t, "clock", t.accent_text, text, |ui| {
            if crate::widgets::pill_button(ui, tl!("Close"), true).clicked() {
                close = true;
            }
        });
        if close {
            return Some(Notice::Close);
        }
    }
    if let Some((icon, color, template, arg)) = signed {
        let mut open = false;
        notice_bar(ui, t, icon, color, crate::i18n::fmt(tl!(template), &[("by", &arg)]), |ui| {
            if crate::widgets::pill_button(ui, tl!("Signature panel"), false).clicked() {
                open = true;
            }
        });
        return open.then_some(Notice::Signatures);
    }
    if view.notice_dismissed {
        return None;
    }
    let mut open_security = false;
    let mut unlock = false;
    let mut hide_secured = false;
    let mut open_repairs = false;
    let mut open_console = false;
    let mut toggled = None;
    // A form whose scripts were turned off no longer calculates or answers its buttons: that
    // comes before what else there is to say about the document.
    let scripts_off = doc.xfa_scripts_off();
    // What the document is as a form, if it is one: what the bar says first, secured or not.
    let form = if let Some(x) = xfa {
        let lang = crate::i18n::current();
        let pages = crate::i18n::trn(lang, x.pages as u64, "{n} page", "{n} pages");
        let fields = crate::i18n::trn(lang, x.fields as u64, "{n} field", "{n} fields");
        let mut text =
            crate::i18n::fmt(tl!("Dynamic XFA form laid out from its template: {pages}, {fields}."), &[("pages", &pages), ("fields", &fields)]);
        if !x.warnings.is_empty() {
            // The notes are clauses ("a picture is left out"); the first starts a sentence here.
            let notes = x.warnings.join("; ");
            let mut chars = notes.chars();
            text.push(' ');
            text.extend(chars.next().into_iter().flat_map(char::to_uppercase));
            text.push_str(chars.as_str());
            text.push('.');
        }
        Some(("text-cursor-input", text, true))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Dynamic) {
        Some((
            "triangle-alert",
            tl!("This is a dynamic XFA form, which PdfCraft can't display yet. What you see is the file's placeholder page.").to_string(),
            false,
        ))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Static) {
        Some((
            "text-cursor-input",
            tl!("XFA form: its fields and its XFA data are kept in step, so other viewers show what you fill in.").to_string(),
            true,
        ))
    } else if info.fields.iter().any(|f| f.kind != pdfcraft_render::FieldKind::PushButton) {
        // Push buttons alone (LaTeX `animate` frames, navigation buttons) leave nothing to fill in.
        Some((
            "text-cursor-input",
            crate::i18n::fmt(tl!("This document contains {n} interactive form fields."), &[("n", &info.fields.len().to_string())]),
            true,
        ))
    } else {
        None
    };
    let msg = if scripts_off {
        Some(("triangle-alert", crate::js_ui::scripts_off_notice(), true))
    } else if secured {
        // A secured form still says what it is, and keeps Highlight fields: the security
        // sentence follows the form's.
        let security = tl!("This document is secured. Some changes are restricted by its security settings.");
        Some(match form {
            Some((icon, text, fields)) => (icon, format!("{text} {security}"), fields),
            None => ("lock", security.to_string(), false),
        })
    } else if form.is_some() {
        form
    } else if repaired {
        Some(("bandage", tl!("This file was damaged and has been repaired. Saving keeps the repaired version.").to_string(), false))
    } else if !info.warnings.is_empty() {
        Some(("triangle-alert", info.warnings[0].clone(), false))
    } else {
        None
    };
    let (icon, text, fields) = msg?;
    notice_bar(ui, t, icon, t.accent_text, text, |ui| {
        if icons::button(ui, "x", 22.0, false, tl!("Dismiss")).clicked() {
            view.notice_dismissed = true;
        }
        if fields {
            let label = if view.highlight_fields { tl!("Hide field highlights") } else { tl!("Highlight fields") };
            if crate::widgets::pill_button(ui, label, view.highlight_fields).clicked() {
                view.highlight_fields = !view.highlight_fields;
                toggled = Some(Notice::FieldHighlights(view.highlight_fields));
            }
        }
        if scripts_off && crate::widgets::pill_button(ui, tl!("JavaScript Console"), false).clicked() {
            open_console = true;
        }
        if secured && crate::widgets::pill_button(ui, tl!("Security settings"), false).clicked() {
            open_security = true;
        }
        if secured && doc.can_unlock() && crate::widgets::pill_button(ui, tl!("Unlock…"), true).clicked() {
            unlock = true;
        }
        // Only when the bar shows the security message: a form's stopped scripts come first.
        if secured && !scripts_off && crate::widgets::pill_button(ui, tl!("Don't show again"), false).clicked() {
            hide_secured = true;
        }
        if repaired && !secured && info.fields.is_empty() && crate::widgets::pill_button(ui, tl!("Details"), false).clicked() {
            open_repairs = true;
        }
    });
    if open_repairs {
        return Some(Notice::Repairs);
    }
    if open_console {
        return Some(Notice::Console);
    }
    if unlock {
        return Some(Notice::Unlock);
    }
    if hide_secured {
        return Some(Notice::HideSecured);
    }
    open_security.then_some(Notice::Security).or(toggled)
}

/// One notice bar: an icon and a message on the left, `buttons` (added right to left) on the
/// right. The buttons are laid out first, so the message only gets the width they leave: it
/// wraps there (or, when very little is left, is cut short with the full text on hover) instead
/// of running under the buttons and past the document area into the side panel (#785). The
/// buttons are centred on a row of button height, and the message's first line is centred on
/// the same row; a wrapped message grows the bar downwards.
fn notice_bar(ui: &mut egui::Ui, t: &Tokens, icon: &str, icon_color: Color32, text: String, buttons: impl FnOnce(&mut egui::Ui)) {
    const ROW: f32 = 28.0;
    const ICON: f32 = 16.0;
    egui::Frame::NONE.fill(t.accent_soft).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
        let width = ui.available_width();
        ui.allocate_ui_with_layout(vec2(width, ROW), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_min_height(ROW);
            buttons(ui);
            let rest = ui.available_width().max(0.0);
            ui.allocate_ui_with_layout(vec2(rest, ROW), egui::Layout::top_down(egui::Align::Min), |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let line = ui.fonts_mut(|f| f.row_height(&font)).max(ICON);
                ui.add_space(((ROW - line) / 2.0).max(0.0));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    let (slot, _) = ui.allocate_exact_size(vec2(ICON, line), Sense::hover());
                    icons::paint(ui, Rect::from_center_size(slot.center(), vec2(ICON, ICON)), icon, ICON, icon_color);
                    let label = egui::Label::new(egui::RichText::new(text).color(t.text));
                    // Wrapping into a sliver would stack the message a few letters per line.
                    ui.add(if ui.available_width() >= 120.0 { label.wrap() } else { label.truncate() });
                });
            });
        });
    });
}

/// The bar's narrowest form: one 32 pt button, 6 pt margins and a 1 pt border each side.
pub(crate) const QUICK_BAR_WIDTH: f32 = 46.0;
pub(crate) const QUICK_BAR_EDGE: f32 = 14.0;

/// How far from the side panel the quick-action bar stands, given the width of the margin between
/// the panel and the page and the bar's own width: centred in the margin when it holds the bar,
/// else 14 pt in (a page filling the width). With no page on screen the caller passes a margin of 0,
/// which is 14 pt too.
pub(crate) fn quick_bar_offset(margin: f32, width: f32) -> f32 {
    // shortcut: centred in at most 100 pt of margin, so a wide margin keeps the bar by the panel.
    // `f32::min` returns the other operand for NaN, which would pass the fit check below.
    let margin = if margin.is_nan() { 0.0 } else { margin.min(100.0) };
    if margin >= width + 2.0 * 8.0 { (margin - width) / 2.0 } else { QUICK_BAR_EDGE }
}

/// The unified quick-action bar and hotbar in the margin or docked to an edge.
pub(super) fn quick_bar(app: &mut PdfCraftApp, area: Rect, page_left: Option<f32>, ui: &mut egui::Ui) {
    // Touch mode's own toolbar above the pages has the drawing tools instead.
    if app.touch_active() {
        return;
    }
    crate::hotbar::show(app, area, page_left, ui);
}

#[cfg(test)]
mod tests {
    use super::quick_bar_offset;

    #[test]
    fn the_quick_bar_centres_in_the_margin_that_holds_it() {
        assert_eq!(quick_bar_offset(72.0, 46.0), 13.0);
        assert_eq!(quick_bar_offset(62.0, 46.0), 8.0);
        // A wide margin centres in its first 100 pt, not all of it.
        assert_eq!(quick_bar_offset(400.0, 46.0), 27.0);
    }

    #[test]
    fn the_margin_that_just_holds_the_bar_centres_and_a_point_less_does_not() {
        // 46 pt of bar and 8 pt to spare each side: 62 pt.
        assert_eq!(quick_bar_offset(62.0, 46.0), 8.0);
        assert_eq!(quick_bar_offset(61.0, 46.0), 14.0);
        assert_eq!(quick_bar_offset(1000.0, 46.0), 27.0);
    }

    #[test]
    fn a_wider_quick_bar_centres_by_its_own_width_and_needs_a_wider_margin() {
        // A 70 pt bar centres by that, and 72 pt no longer holds it.
        assert_eq!(quick_bar_offset(120.0, 70.0), 15.0);
        assert_eq!(quick_bar_offset(72.0, 70.0), 14.0);
    }

    #[test]
    fn the_quick_bar_keeps_14_pt_in_when_the_margin_cannot_hold_it() {
        for margin in [61.9, 30.0, 0.0, -50.0, f32::NEG_INFINITY, f32::NAN] {
            assert_eq!(quick_bar_offset(margin, 46.0), 14.0, "{margin}");
        }
    }
}
