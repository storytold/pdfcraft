use super::thumbs::{Key, Look, MAX_SIDE, PREVIEW, PREVIEW_SIDE, SIZE_STEP, Thumbs, Want};
use super::*;

/// One card's place and what it shows.
pub(super) struct Card {
    /// The file, by its place in the list.
    pub(super) i: usize,
    /// A page card (the file expanded): its place among the pages the file adds, the page and how
    /// many it adds. `None` for the file's own card.
    pub(super) page: Option<(usize, usize, usize)>,
    pub(super) rect: Rect,
    pub(super) check: Result<usize, String>,
    pub(super) selected: bool,
    pub(super) twice: bool,
    pub(super) revision: u64,
    pub(super) ppp: f32,
    /// The box the page is fitted in (the zoom).
    pub(super) fit: Vec2,
}

impl Card {
    pub(super) fn id(&self, ui: &egui::Ui, f: &CombineFile) -> egui::Id {
        match self.page {
            None => ui.scope_id().with(("combine-card", f.id)),
            Some((nth, ..)) => ui.scope_id().with(("combine-page", f.id, nth)),
        }
    }

    pub(super) fn show(&self, ui: &mut egui::Ui, t: &Tokens, f: &mut CombineFile, thumbs: &mut Thumbs) -> Option<RowAction> {
        let mut action = None;
        let c = self.rect;
        let id = self.id(ui, f);
        let resp = ui.interact(c.shrink(4.0), id, Sense::click_and_drag());
        let name = f.name.clone();
        // A page card: "Page 3", and the file it belongs to.
        let page_label = self.page.map(|(_, p, _)| crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.saturating_add(1).to_string())]));
        let label = match &page_label {
            Some(page) => format!("{page} · {name}"),
            None => name.clone(),
        };
        // As the organize grid's pages: a check box that is on when selected; and, for screen
        // readers, selected as items in a list are.
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, self.selected, &label));
        let pages = match self.page {
            // For screen readers: which page of how many.
            Some((_, p, _)) => crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &p.saturating_add(1).to_string()), ("n", &f.pages.to_string())]),
            None if f.pages == 1 => tl!("1 page").to_string(),
            None => crate::i18n::fmt(tl!("{n} pages"), &[("n", &f.pages.to_string())]),
        };
        let painter = ui.painter_at(c);

        // Selected and hovered cards.
        if self.selected || resp.hovered() {
            painter.rect_filled(c.shrink(6.0), CornerRadius::same(8), if self.selected { t.accent_soft } else { t.hover });
        }
        if self.selected {
            painter.rect_stroke(c.shrink(6.0), CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }

        // The page, fitted in the paper box (top-aligned, room above for the stack).
        let page = match self.page {
            Some((_, p, _)) => Ok(p),
            None => f.first_page(),
        };
        // The page's own size, when it is known and is one.
        let known = page.as_ref().ok().and_then(|p| f.sizes.get(*p).copied()).filter(|(w, h)| paper_size(*w, *h, self.fit).is_some());
        let size_pt = known.unwrap_or(FALLBACK_PAGE);
        let size = paper_size(size_pt.0, size_pt.1, self.fit).unwrap_or(self.fit);
        let top = c.top() + ABOVE;
        let paper = Rect::from_min_size(pos2(c.center().x - size.x / 2.0 - 3.0, top + (self.fit.y - size.y)), size);
        // A file's card that adds more than one page: sheets behind it.
        let takes = self.check.as_ref().map_or(f.pages, |k| *k);
        if self.page.is_none() && takes > 1 {
            for d in [6.0, 3.0] {
                let sheet = paper.translate(vec2(d, -d));
                painter.rect_filled(sheet, CornerRadius::ZERO, Color32::WHITE);
                painter.rect_stroke(sheet, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
            }
        }
        painter.rect_filled(paper.translate(vec2(0.0, 1.5)), CornerRadius::same(1), t.page_shadow);
        painter.rect_filled(paper, CornerRadius::ZERO, Color32::WHITE);

        // What goes on the paper: the thumbnail, or why there isn't one.
        let mut why: Option<(&str, Color32, String)> = None;
        if let Some(lock) = f.lock {
            let text = match lock {
                Lock::Open => tl!("Password-protected"),
                Lock::Permissions => tl!("Its security settings don't allow copying pages"),
            };
            why = Some(("lock", ON_PAPER, text.to_owned()));
        } else if let Some(p) = &f.problem {
            why = Some(("circle-x", ERROR, p.clone()));
        } else {
            match page {
                Err(e) => why = Some(("circle-x", ERROR, e)),
                // The pages couldn't be counted, or the page's size is unknown (some passwords
                // open a file for combining but not for reading it here): nothing is drawn (the
                // engine decides at Combine), and the card says so rather than look blank.
                Ok(p) if p >= f.pages || known.is_none() => why = Some(("eye-off", ON_PAPER, tl!("No preview").to_owned())),
                Ok(p) => {
                    // Pixels per point: the size the paper is drawn at, on both sides, and never a
                    // side longer than a thumbnail's (a long, thin page fits by its long side).
                    let (w, h) = size_pt;
                    let scale = (size.x * self.ppp / w).min(size.y * self.ppp / h).min(MAX_SIDE / w.max(h));
                    let px = [w * scale, h * scale].map(|v| (v.max(1.0).ceil() as u32).div_ceil(SIZE_STEP).saturating_mul(SIZE_STEP));
                    let key = Key { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, page: p, px };
                    let look = thumbs.look((f.group, p), key);
                    let mut want = false;
                    match look {
                        Look::Ready(tex) => {
                            painter.image(tex, paper, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        Look::Failed(e) => why = Some(("triangle-alert", WARNING, crate::i18n::fmt(tl!("Preview unavailable: {e}"), &[("e", &e)]))),
                        Look::Pending(stand_in) => {
                            if let Some(tex) = stand_in {
                                painter.image(tex, paper, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            want = true;
                        }
                    }
                    if want && scale.is_finite() && scale > 0.0 {
                        thumbs.wants.push(Want {
                            slot: (f.group, p),
                            key,
                            bytes: f.bytes.clone(),
                            password: f.password().map(Arc::from),
                            scale,
                            visible: ui.is_rect_visible(c),
                        });
                    }
                }
            }
        }
        if let Some((icon, colour, _)) = &why {
            icons::paint(ui, Rect::from_center_size(paper.center(), Vec2::splat(28.0)), icon, 28.0, *colour);
        }
        // For screen readers: the pages, and why no page is shown (otherwise only in the tooltip).
        let description = match &why {
            Some((_, _, reason)) => format!("{pages}. {reason}"),
            None => pages.clone(),
        };
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_selected(self.selected);
            node.set_description(description);
        });
        painter.rect_stroke(paper, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);

        // A warning that doesn't stop it being combined: a badge on the file's card.
        let warned = self.page.is_none() && (!f.notes.is_empty() || self.twice);
        if warned && why.as_ref().is_none_or(|(icon, ..)| *icon == "eye-off") {
            let r = Rect::from_center_size(paper.right_top() + vec2(-2.0, 2.0), Vec2::splat(18.0));
            painter.circle_filled(r.center(), 10.0, t.card);
            icons::paint(ui, r, "triangle-alert", 15.0, WARNING);
        }

        // The name, shortened in the middle to fit, and the pages taken when a range is set; on a
        // page card, the page first and the file's name below it, smaller.
        let name_y = top + self.fit.y + 16.0;
        let max_w = c.width() - 20.0;
        let (font, colour, y) = match &page_label {
            Some(page) => {
                painter.text(pos2(c.center().x, name_y), Align2::CENTER_CENTER, page, theme::semibold(12.5), t.text);
                (theme::regular(11.5), t.text_muted, name_y + 17.0)
            }
            None => (theme::medium(12.5), t.text, name_y),
        };
        let fits = |s: &str| crate::bidi::layout_ltr(ui, s.to_owned(), font.clone(), colour).size().x <= max_w;
        let shown = fit_name(&f.name, fits);
        // A file name is file/path chrome: always left to right, so an Arabic name ends in `.pdf`.
        let name_galley = crate::bidi::layout_ltr(ui, shown, font, colour);
        painter.galley(pos2(c.center().x - name_galley.size().x / 2.0, y - name_galley.size().y / 2.0), name_galley, colour);
        let note = if self.page.is_none() && !f.range.trim().is_empty() {
            match &self.check {
                Ok(k) => Some((crate::i18n::fmt(tl!("Pages: {k} of {n}"), &[("k", &k.to_string()), ("n", &f.pages.to_string())]), t.text_muted)),
                Err(_) => Some((tl!("Check the pages").to_owned(), ERROR)),
            }
        } else {
            None
        };
        if let Some((text, colour)) = note {
            painter.text(pos2(c.center().x, name_y + 17.0), Align2::CENTER_CENTER, text, theme::regular(11.5), colour);
        }

        // On hover, the card's own actions along the top of the page (not while a card is
        // dragged): remove this file only, and show it large when there is a page to show. Drawn
        // over the card, so a click on one doesn't select or move it.
        let previewable = why.is_none();
        // A file that adds more pages than one can be spread out into them.
        let expandable = self.page.is_none() && takes > 1 && takes <= MAX_EXPAND && f.lock.is_none() && f.problem.is_none() && self.check.is_ok();
        // Shown large: the file from its first page, or a page card's page.
        let preview = match self.page {
            Some((nth, ..)) => RowAction::PreviewAt(f.id, nth),
            None => RowAction::Preview(f.id),
        };
        // The card's own actions, in this order: expand (or, on a page, collapse), remove (the file,
        // or the page), show large.
        let mut buttons: Vec<(&str, String, RowAction)> = Vec::new();
        if expandable {
            buttons.push(("maximize-2", tl!("Expand").to_owned(), RowAction::Expand(f.id, true)));
        }
        if let Some((nth, ..)) = self.page {
            buttons.push(("minimize-2", tl!("Collapse").to_owned(), RowAction::Expand(f.id, false)));
            buttons.push(("trash-2", tl!("Remove page").to_owned(), RowAction::RemovePages(Some((f.id, nth)))));
        } else {
            buttons.push(("trash-2", crate::i18n::fmt(tl!("Remove {name}"), &[("name", &f.name)]), RowAction::RemoveFile(f.id)));
        }
        if previewable {
            buttons.push(("search", tl!("Preview").to_owned(), preview));
        }
        // Nothing of the card shows over a dialog (the preview, the password box).
        let covered = ui.ctx().memory(|m| m.top_modal_layer().is_some());
        let mut over_bar = false;
        if !covered && ui.rect_contains_pointer(c.shrink(4.0)) && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
            let count = buttons.len() as f32;
            // Mostly above the page (a quarter over its top edge), so it hides little of it, and
            // never the name below.
            let y = paper.top() + BAR_H / 4.0 - BAR_H / 2.0;
            let bar = Rect::from_center_size(pos2(paper.center().x, y), vec2(count * 26.0 + (count - 1.0).max(0.0) * 2.0 + 8.0, BAR_H));
            over_bar = ui.rect_contains_pointer(bar);
            painter.rect(bar, CornerRadius::same(8), t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink(4.0)).layout(egui::Layout::left_to_right(egui::Align::Center)));
            bar_ui.spacing_mut().item_spacing.x = 2.0;
            for (icon, label, act) in &buttons {
                if icons::button(&mut bar_ui, icon, 26.0, false, label).clicked() {
                    action = Some(act.clone());
                }
            }
        }

        // The full name and everything known about the file, on hover (on a page card, which page).
        let mut tip = match &page_label {
            Some(page) => format!("{}\n{page}", f.name),
            None => format!("{}\n{pages} · {}", f.name, crate::panels::human_size(f.bytes.len())),
        };
        for line in why.iter().map(|(_, _, s)| s).chain(&f.notes) {
            tip.push('\n');
            tip.push_str(line);
        }
        if self.twice {
            tip.push('\n');
            tip.push_str(tl!("Added more than once"));
        }
        // Page ranges are typed in the list.
        if self.page.is_none() && self.check.is_err() {
            tip.push('\n');
            tip.push_str(tl!("Switch to List view to change the pages it takes."));
        }
        // (Over the bar, its buttons say what they do instead.)
        let resp = if covered || over_bar { resp } else { resp.on_hover_text(tip) };

        // The same actions on a right click, show large first (and Unlock… for a locked file).
        let at = resp.rect.left_bottom();
        let locked = f.lock.is_some();
        resp.context_menu(|ui| {
            let item = |ui: &mut egui::Ui, icon: &str, label: &str| {
                ui.add(egui::Button::image_and_text(icons::image(icon, 15.0, ui.visuals().text_color()), label)).clicked()
            };
            for (icon, label, act) in buttons.iter().rev() {
                // (Remove names its file, as the trash button does.)
                if item(ui, icon, label) {
                    action = Some(act.clone());
                    ui.close();
                }
            }
            if locked && item(ui, "lock-open", tl!("Unlock…")) {
                action = Some(RowAction::Unlock(Some(self.i), at));
                ui.close();
            }
        });
        if resp.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), GridDrag { file: f.id, revision: self.revision, page: self.page.map(|(nth, ..)| (f.id, nth)) });
        }
        if resp.clicked() {
            let mods = resp.ctx.input(|i| i.modifiers);
            action = Some(match self.page {
                Some((nth, ..)) => RowAction::ClickPage(f.id, nth, mods),
                None => RowAction::Click(self.i, mods),
            });
        }
        action
    }
}

/// The file a card's magnifier opened, shown large over the page: one of the pages it adds at a
/// time, flipped with the buttons or the Left and Right keys; Esc, the close button or a click
/// outside closes it. It closes by itself when the file leaves the list or has nothing to show.
pub(crate) fn preview(app: &mut PdfCraftApp, ctx: &egui::Context, t: &Tokens) {
    // The command palette goes over everything: the preview waits under it (drawn again after).
    if app.palette_open {
        return;
    }
    let Some(p) = app.combine_tab.preview.as_mut() else { return };
    let Some(f) = app.combine_draft.iter().find(|f| f.id == p.file).filter(|f| f.lock.is_none() && f.problem.is_none()) else {
        app.combine_tab.preview = None;
        return;
    };
    // The pages it adds, worked out again only when its range changes.
    let pages = match &p.pages {
        Some(((range, count), pages)) if *range == f.range && *count == f.pages => pages.clone(),
        _ => match f.pages_taken() {
            Ok(pages) => {
                let pages: Arc<[usize]> = Arc::from(pages);
                p.pages = Some(((f.range.clone(), f.pages), pages.clone()));
                pages
            }
            Err(_) => {
                app.combine_tab.preview = None;
                return;
            }
        },
    };
    let n = pages.len();
    p.at = p.at.min(n.saturating_sub(1));
    let Some(&page) = pages.get(p.at) else {
        app.combine_tab.preview = None;
        return;
    };
    p.page = Some(page);
    let at = p.at;
    // Flipped with the keys (the grid's arrows wait while this shows), unless a text field has
    // them.
    let typing = ctx.text_edit_focused();
    let (back, on) = ctx.input_mut(|i| {
        for k in [egui::Key::Plus, egui::Key::Equals, egui::Key::Minus, egui::Key::Num0] {
            i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, k));
        }
        if typing {
            return (false, false);
        }
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) | i.consume_key(egui::Modifiers::NONE, egui::Key::PageUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) | i.consume_key(egui::Modifiers::NONE, egui::Key::PageDown),
        )
    });
    let (mut step, mut close) = (i64::from(on) - i64::from(back), false);
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.86).max(320.0);
    let body_h = (screen.height() * 0.86 - 110.0).max(200.0);
    let ppp = ctx.pixels_per_point();
    // No larger than the GPU takes, or the render would be refused.
    let longest = ctx.input(|i| i.max_texture_side as f32).min(PREVIEW_SIDE);
    let thumbs = &mut app.combine_thumbs;
    let modal = egui::Modal::new(egui::Id::unique("combine-preview")).show(ctx, |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(f.name.clone()).font(theme::semibold(15.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, "x", 30.0, false, tl!("Close")).clicked() {
                    close = true;
                }
                let of = crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(at + 1).to_string()), ("n", &n.to_string())]);
                ui.label(egui::RichText::new(of).color(t.text_muted));
            });
        });
        ui.add_space(8.0);
        // The page, as large as fits.
        let (area, _) = ui.allocate_exact_size(vec2(width, body_h), Sense::hover());
        let known = f.sizes.get(page).copied().filter(|(w, h)| paper_size(*w, *h, area.size()).is_some());
        let (w, h) = known.unwrap_or(FALLBACK_PAGE);
        let size = paper_size(w, h, area.size() - vec2(8.0, 8.0)).unwrap_or(area.size());
        let paper = Rect::from_center_size(area.center(), size);
        let painter = ui.painter_at(area);
        painter.rect_filled(paper.translate(vec2(0.0, 2.0)), CornerRadius::same(1), t.page_shadow);
        painter.rect_filled(paper, CornerRadius::ZERO, Color32::WHITE);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        if known.is_some() {
            let scale = (size.x * ppp / w).min(size.y * ppp / h).min(longest / w.max(h));
            let px = [w * scale, h * scale].map(|v| (v.max(1.0).ceil() as u32).div_ceil(SIZE_STEP).saturating_mul(SIZE_STEP));
            let key = Key { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, page, px };
            match thumbs.look(PREVIEW, key) {
                Look::Ready(tex) => {
                    painter.image(tex, paper, uv, Color32::WHITE);
                }
                Look::Failed(e) => {
                    let text = crate::i18n::fmt(tl!("Preview unavailable: {e}"), &[("e", &e)]);
                    painter.text(paper.center(), Align2::CENTER_CENTER, text, theme::regular(13.0), ON_PAPER);
                }
                Look::Pending(stand_in) => {
                    // A smaller render of this page stands in: the last size shown, or its card's.
                    let card = thumbs.entries.get(&(f.group, page)).filter(|e| e.key.auth == f.auth).and_then(|e| e.tex.as_ref());
                    if let Some(tex) = stand_in.or(card.map(TextureHandle::id)) {
                        painter.image(tex, paper, uv, Color32::WHITE);
                    }
                    if scale.is_finite() && scale > 0.0 {
                        thumbs.wants.push(Want {
                            slot: PREVIEW,
                            key,
                            bytes: f.bytes.clone(),
                            password: f.password().map(Arc::from),
                            scale,
                            visible: true,
                        });
                    }
                }
            }
        } else {
            icons::paint(ui, Rect::from_center_size(paper.center(), Vec2::splat(36.0)), "eye-off", 36.0, ON_PAPER);
        }
        painter.rect_stroke(paper, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
        ui.add_space(8.0);
        // Previous and next, centred below.
        ui.horizontal(|ui| {
            ui.add_space((width - 2.0 * 30.0 - 6.0) / 2.0);
            if ui.add_enabled_ui(at > 0, |ui| icons::button(ui, "chevron-left", 30.0, false, tl!("Previous page"))).inner.clicked() {
                step = -1;
            }
            if ui.add_enabled_ui(at + 1 < n, |ui| icons::button(ui, "chevron-right", 30.0, false, tl!("Next page"))).inner.clicked() {
                step = 1;
            }
        });
    });
    if close || modal.should_close() {
        app.combine_tab.preview = None;
    } else if let Some(p) = app.combine_tab.preview.as_mut() {
        p.at = match step {
            s if s < 0 => p.at.saturating_sub(1),
            s if s > 0 => (p.at + 1).min(n.saturating_sub(1)),
            _ => p.at,
        };
    }
}
