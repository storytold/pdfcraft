//! Organize pages, laid out as Acrobat's panel: the pages selected (typed, or picked by kind from
//! its menu); page options as a row of icon buttons (rotate both ways, delete, extract) and Insert,
//! Replace, Split and Bates numbering; page setup (boxes, crop, transitions, templates, printing);
//! and page labels and properties. It shows whenever the page grid does, so every way into
//! Organize (the tool, the Edit panel, the command, `--organize`) gets the same controls, and the
//! grid's own toolbar keeps only moving pages, their size, Save pages and Close.
//!
//! The page actions queue on the view (`pending_edit`, `pending_action`) exactly as the grid's
//! toolbar did, so they run, refuse and undo the same way.

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::Edit;

use crate::canvas::ViewAction;
use crate::theme::{self, Tokens};
use crate::{LeftPanel, PdfCraftApp, icons, widgets};

/// Whether the left panel is Organize pages: the tool is chosen, or the active view shows the grid.
pub(crate) fn showing(app: &PdfCraftApp) -> bool {
    app.left == LeftPanel::Tool("organize") || app.active.and_then(|i| app.views.get(i)).is_some_and(|v| v.organize)
}

/// The panel of the workspace Organize was opened from, shown again once Organize closes (by
/// its Close, the grid's Close, or opening a page from the grid): Edit's panel under the Edit
/// tab, as Acrobat goes back to the tool you came from, and All tools otherwise.
pub(crate) fn workspace_panel(mode: crate::Mode) -> LeftPanel {
    match mode {
        crate::Mode::Edit => LeftPanel::Tool("edit"),
        crate::Mode::Convert => LeftPanel::Tool("export"),
        crate::Mode::Sign => LeftPanel::Tool("fill_sign"),
        _ => LeftPanel::AllTools,
    }
}

/// What a row does once clicked.
enum Act {
    Edit(Box<Edit>),
    View(ViewAction),
    Command(&'static str),
}

pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some((index, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let info = &doc.info;
    let editable = doc.allows_assembly();
    let n = info.pages.len();
    let mut act: Option<Act> = None;
    let mut back = false;
    let mut close = false;
    let Some(view) = app.views.get_mut(index) else { return };

    ui.horizontal(|ui| {
        if icons::button(ui, "chevron-left", 26.0, false, tl!("Back to all tools")).clicked() {
            back = true;
        }
        let title = tl!("Organize pages");
        let title_width = (ui.available_width() - 26.0 - ui.spacing().item_spacing.x).max(1.0);
        ui.add_sized([title_width, 26.0], egui::Label::new(egui::RichText::new(title).font(theme::semibold(15.5)).color(t.text)).truncate())
            .on_hover_text(title);
        if icons::button(ui, "x", 26.0, false, tl!("Close panel")).clicked() {
            close = true;
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(2.0);
        widgets::section_title(ui, tl!("Pages selected"));
        let current = view.current;
        let picked = view.organize_range.show(ui, &view.selected, n, current, |ui| {
            use pdfcraft_engine::{PageOrientation as O, PageParity as P, filter_pages};
            let all: Vec<usize> = (0..n).collect();
            let mut chosen = None;
            for (label, parity, orient) in [
                ("All pages", P::Both, O::Both),
                ("Even pages", P::Even, O::Both),
                ("Odd pages", P::Odd, O::Both),
                ("Landscape pages", P::Both, O::Landscape),
                ("Portrait pages", P::Both, O::Portrait),
            ] {
                if ui.button(tl!(label)).clicked() {
                    chosen = Some(filter_pages(info, &all, parity, orient));
                }
            }
            if ui.button(tl!("None")).clicked() {
                chosen = Some(Vec::new());
            }
            chosen
        });
        if let Some(pages) = picked {
            view.select_pages(&pages);
        }

        let targets = view.target_pages();
        let last = targets.last().copied().unwrap_or(0);
        widgets::section_title(ui, tl!("Page options"));
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if icons::button(ui, "rotate-ccw", 34.0, false, tl!("Rotate counterclockwise")).clicked() {
                    act = Some(Act::Edit(Box::new(Edit::RotatePages { pages: targets.clone(), degrees: -90 })));
                }
                if icons::button(ui, "rotate-cw", 34.0, false, tl!("Rotate clockwise")).clicked() {
                    act = Some(Act::Edit(Box::new(Edit::RotatePages { pages: targets.clone(), degrees: 90 })));
                }
                // A document keeps at least one page.
                let can_delete = targets.len() < n;
                if ui.add_enabled_ui(can_delete, |ui| icons::button(ui, "trash-2", 34.0, false, tl!("Delete pages (Delete)"))).inner.clicked() {
                    act = Some(Act::Edit(Box::new(Edit::DeletePages { pages: targets.clone() })));
                }
                if icons::button(ui, "file-output", 34.0, false, tl!("Extract pages to a new document")).clicked() {
                    act = Some(Act::View(ViewAction::Extract));
                }
            });
            ui.add_space(2.0);
            // Insert ▸: a blank page, pages from a file, or copies of the selected pages.
            let insert = row(ui, t, "file-plus-2", tl!("Insert"), Availability::Ready, true);
            egui::Popup::menu(&insert).align(egui::RectAlign::RIGHT_START).gap(6.0).show(|ui| {
                if ui.button(tl!("Insert a blank page after the selection")).clicked() {
                    let crop = info.pages.get(last).map_or([0.0, 0.0, 612.0, 792.0], |p| p.crop);
                    let (w, h) = ((crop[2] - crop[0]).abs().max(1.0) as f64, (crop[3] - crop[1]).abs().max(1.0) as f64);
                    act = Some(Act::Edit(Box::new(Edit::InsertBlankPage { at: last.saturating_add(1), width: w, height: h })));
                }
                if ui.button(tl!("Insert pages from a file…")).clicked() {
                    act = Some(Act::View(ViewAction::InsertFromFile));
                }
                if ui.button(tl!("Duplicate pages")).clicked() {
                    act = Some(Act::Command("page.duplicate"));
                }
            });
            if row(ui, t, "replace", tl!("Replace"), Availability::Ready, false).clicked() {
                act = Some(Act::View(ViewAction::ReplacePages));
            }
            if row(ui, t, "scissors", tl!("Split"), Availability::Ready, false).clicked() {
                act = Some(Act::View(ViewAction::Split));
            }
        });
        if row(ui, t, "hash", tl!("Bates numbering"), Availability::Ready, false).clicked() {
            act = Some(Act::Command("edit.bates"));
        }
        divider(ui, t);
        for (icon, label, availability, command) in [
            ("square-dashed-mouse-pointer", "Set page boxes", Availability::Ready, Some("page.boxes")),
            ("crop", "Crop pages", Availability::Ready, Some("page.crop")),
            ("presentation", "Page transitions", Availability::Later, Some("page.transitions")),
            ("file-text", "Page templates", Availability::Later, None),
            ("printer", "Print pages", Availability::Ready, Some("print.dialog")),
        ] {
            if row(ui, t, icon, tl!(label), availability, false).clicked()
                && let Some(command) = command
            {
                act = Some(Act::Command(command));
            }
        }
        divider(ui, t);
        if row(ui, t, "tag", tl!("Page labels"), Availability::Ready, false).clicked() {
            act = Some(Act::Command("page.number"));
        }
        row(ui, t, "info", tl!("Page properties"), Availability::Later, false);
    });

    if close {
        view.organize = false;
        app.left = workspace_panel(app.mode);
    } else if back {
        view.organize = false;
        app.left = LeftPanel::AllTools;
    }
    match act {
        Some(Act::Edit(edit)) => view.pending_edit = Some(*edit),
        Some(Act::View(action)) => view.pending_action = Some(action),
        Some(Act::Command(command)) => app.run_command(command),
        None => {}
    }
    // Leaving the workspace needs the whole app, so it waits until the view is done with: leaving
    // Edit puts Edit text down and applies a paragraph still open on the page.
    if back && !close {
        app.enter_mode(crate::Mode::AllTools);
    }
}

/// Whether a row works in this build.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Availability {
    Ready,
    /// Not in PdfCraft yet: shown muted, as Acrobat lists it, and says it is planned when hovered.
    Later,
}

/// One row of the panel: a monochrome icon and the label, as Acrobat draws them, with a corner
/// mark when it opens a menu.
fn row(ui: &mut egui::Ui, t: &Tokens, icon: &str, label: &str, availability: Availability, menu: bool) -> egui::Response {
    let ready = availability == Availability::Ready && ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), if ready { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ready, label));
    let press = theme::Press::track(ui, &resp);
    if ready {
        press.wash(ui, rect, 6, false);
    }
    let body = rect.translate(press.offset());
    let fg = if ready { t.text } else { t.text_faint };
    let icon_rect = Rect::from_min_size(body.min + vec2(6.0, 8.0), vec2(18.0, 18.0));
    icons::paint(ui, icon_rect, icon, 17.0, fg);
    let font = theme::regular(13.5);
    let width = crate::widgets::text_width(ui, label, &font);
    ui.painter().text(body.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, fg);
    if menu {
        // The small corner triangle Acrobat puts after a label that opens a menu.
        let tip = body.left_center() + vec2(34.0 + width + 6.0, 6.0);
        let tri = vec![tip, tip + vec2(-5.0, 0.0), tip + vec2(0.0, -5.0)];
        ui.painter().add(egui::Shape::convex_polygon(tri, fg, Stroke::NONE));
    }
    match availability {
        Availability::Ready => theme::hand(resp),
        Availability::Later => resp.on_hover_text(tl!("Planned")),
    }
}

fn divider(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::ZERO, t.divider);
    ui.add_space(6.0);
}
