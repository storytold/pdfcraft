use super::{
    Color32, CornerRadius, DocInfo, DocView, Edit, PdfCraftApp, Pos2, QuickTool, Rect, Sense, SnapshotKind, Stroke, Tokens, comments, icons, theme,
    vec2,
};

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

/// The floating quick-action bar at the left edge of the document area.
pub(super) fn quick_bar(app: &mut PdfCraftApp, area: Rect, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let pos = area.left_top() + vec2(14.0, 14.0);
    egui::Area::new(egui::Id::unique("quick-bar")).order(egui::Order::Middle).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(12))
            .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 100 } else { 28 }) })
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.vertical(|ui| {
                    if icons::button(ui, "mouse-pointer-2", 32.0, app.quick_tool == QuickTool::Select, tl!("Select (V)")).clicked() {
                        app.quick_tool = QuickTool::Select;
                    }
                    if icons::button(ui, "hand", 32.0, app.quick_tool == QuickTool::Hand, tl!("Hand (H)")).clicked() {
                        app.quick_tool = QuickTool::Hand;
                    }
                    // Comment ▸, Highlight ▸, Draw ▸ (Acrobat's comment toolbar groups). Clicking a
                    // group selects its last-used tool; clicking it again opens the flyout.
                    for g in 0..comments::GROUPS.len() {
                        let current = app.comment_prefs.group_tool[g];
                        let active = matches!(app.quick_tool, QuickTool::Comment(t) if t.group() == g);
                        let resp = icons::button(ui, current.icon(), 32.0, active, tl!(current.label()));
                        // A small corner triangle marks the flyout.
                        let r = resp.rect;
                        let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
                        ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), if active { t.accent } else { t.text_faint }, Stroke::NONE));
                        let open = (resp.clicked() && active) || resp.secondary_clicked();
                        if resp.clicked() && !active {
                            app.execute(current.command());
                        }
                        egui::Popup::menu(&resp)
                            .open_memory(open.then_some(egui::SetOpenCommand::Toggle))
                            .align(egui::RectAlign::RIGHT_START)
                            .gap(6.0)
                            .show(|ui| {
                                for tool in comments::GROUPS[g] {
                                    let on = app.quick_tool == QuickTool::Comment(*tool);
                                    let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                                    let press = theme::Press::track(ui, &click);
                                    press.wash(ui, row, 4, on);
                                    let body = row.translate(press.offset());
                                    icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                    let reserve = if on { 28.0 } else { 8.0 };
                                    crate::widgets::paint_left(
                                        ui,
                                        body.left_center() + vec2(34.0, 0.0),
                                        tl!(tool.label()),
                                        theme::regular(13.0),
                                        t.text,
                                        (row.width() - 34.0 - reserve).max(0.0),
                                    );
                                    if on {
                                        icons::paint(
                                            ui,
                                            Rect::from_min_size(body.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                            "check",
                                            14.0,
                                            t.accent,
                                        );
                                    }
                                    let click = theme::hand(click);
                                    let info = tl!(tool.label()).to_string();
                                    click.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, info.clone()));
                                    if click.clicked() {
                                        app.execute(tool.command());
                                        ui.close();
                                    }
                                }
                            });
                    }
                    // Edit text & images, while the Edit workspace is open (Acrobat's Edit toolbar)
                    // or the tool is in use.
                    if app.mode == crate::Mode::Edit || app.quick_tool == QuickTool::EditText {
                        let on = app.quick_tool == QuickTool::EditText;
                        if icons::button(ui, "text-cursor-input", 32.0, on, tl!("Edit text & images")).clicked() {
                            if on {
                                app.put_down_edit_text();
                            } else {
                                app.quick_tool = QuickTool::EditText;
                            }
                        }
                    }
                    // Fill & Sign ▸ (text, marks, date, signature).
                    let current_fill = match app.quick_tool {
                        QuickTool::Fill(f) => Some(f),
                        _ => None,
                    };
                    let shown = current_fill.unwrap_or(crate::fill_sign::FillTool::Text);
                    let resp = icons::button(
                        ui,
                        if current_fill.is_some() { shown.icon() } else { "pen-line" },
                        32.0,
                        current_fill.is_some(),
                        tl!("Fill & Sign"),
                    );
                    let r = resp.rect;
                    let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
                    ui.painter().add(egui::Shape::convex_polygon(
                        tri.to_vec(),
                        if current_fill.is_some() { t.accent } else { t.text_faint },
                        Stroke::NONE,
                    ));
                    if resp.clicked() && current_fill.is_none() {
                        app.execute(shown.command());
                    }
                    let open = (resp.clicked() && current_fill.is_some()) || resp.secondary_clicked();
                    egui::Popup::menu(&resp)
                        .open_memory(open.then_some(egui::SetOpenCommand::Toggle))
                        .align(egui::RectAlign::RIGHT_START)
                        .gap(6.0)
                        .show(|ui| {
                            for tool in crate::fill_sign::FILL_TOOLS {
                                if matches!(tool, crate::fill_sign::FillTool::Signature | crate::fill_sign::FillTool::Initials) {
                                    continue;
                                }
                                let on = current_fill == Some(tool);
                                let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                                let press = theme::Press::track(ui, &click);
                                press.wash(ui, row, 4, on);
                                let body = row.translate(press.offset());
                                icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                let reserve = if on { 28.0 } else { 8.0 };
                                crate::widgets::paint_left(
                                    ui,
                                    body.left_center() + vec2(34.0, 0.0),
                                    tl!(tool.label()),
                                    theme::regular(13.0),
                                    t.text,
                                    (row.width() - 34.0 - reserve).max(0.0),
                                );
                                if on {
                                    icons::paint(
                                        ui,
                                        Rect::from_min_size(body.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                        "check",
                                        14.0,
                                        t.accent,
                                    );
                                }
                                let click = theme::hand(click);
                                let info = tl!(tool.label()).to_string();
                                click.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, info.clone()));
                                if click.clicked() {
                                    app.execute(tool.command());
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if let Some(command) = crate::fill_sign::signature_entries(ui, app, &t) {
                                app.execute(command);
                                ui.close();
                            }
                        });
                    if matches!(current_fill, Some(crate::fill_sign::FillTool::Text | crate::fill_sign::FillTool::Date)) {
                        ui.label(tl!("Font size:"));
                        let size = ui.add(
                            egui::DragValue::new(&mut app.fill_text_size)
                                .range(crate::fill_sign::MIN_TEXT_SIZE..=crate::fill_sign::MAX_TEXT_SIZE)
                                .speed(0.25)
                                .suffix(" pt"),
                        );
                        // #1274: the size also applies to the Fill & Sign text that is selected,
                        // once per change (a typed value, or the end of a drag), so a drag adds
                        // one undo step and not one per frame.
                        if ((size.changed() && !size.dragged()) || size.drag_stopped())
                            && let Some((index, id)) = app.active_ids()
                            && let Some((page, annotation)) = app.views.get(index).and_then(|view| view.comments.selected)
                            && app.session.get(id).is_some_and(|doc| {
                                doc.allows_annotation()
                                    && doc
                                        .info
                                        .annotations
                                        .iter()
                                        .any(|a| a.page == page && a.index == annotation && a.fill_sign && a.subtype == "FreeText" && !a.locked)
                            })
                        {
                            app.apply_edit(Edit::FontSizeAnnotation { page, index: annotation, size: app.fill_text_size });
                        }
                    }
                    if matches!(current_fill, Some(crate::fill_sign::FillTool::Text | crate::fill_sign::FillTool::Date)) {
                        ui.menu_button(tl!("Colour"), |ui| {
                            if let Some(color) = comments::swatch_grid(ui, Some(app.fill_text_color)) {
                                app.fill_text_color = color;
                                ui.close();
                            }
                        });
                    }
                    if let QuickTool::Comment(tool) = app.quick_tool {
                        let (r, _) = ui.allocate_exact_size(vec2(32.0, 9.0), Sense::hover());
                        ui.painter().hline(r.x_range().shrink(6.0), r.center().y, Stroke::new(1.0, t.divider));
                        if let Some(color) = comments::quick_bar_controls(ui, tool, &mut app.comment_prefs)
                            && let Some((index, id)) = app.active_ids()
                            && let Some((page, annotation)) = app.views.get(index).and_then(|view| view.comments.selected)
                            && app.session.get(id).is_some_and(|doc| {
                                doc.allows_annotation()
                                    && doc
                                        .info
                                        .annotations
                                        .iter()
                                        .any(|a| a.page == page && a.index == annotation && !a.is_reply() && comments::made_by(a, tool))
                            })
                        {
                            app.apply_edit(Edit::StyleAnnotation {
                                page,
                                index: annotation,
                                color: Some(color),
                                opacity: None,
                                width: None,
                                endings: None,
                            });
                        }
                    }
                });
            });
    });
}
