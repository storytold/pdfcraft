//! Preferences ▸ Page display: how a PDF opens (one page or two side by side, scrolling, zoom),
//! how the wheel and keys scroll it, and the units page sizes are shown in.

use super::tiles::{self, Picture, Tile};
use super::{card, hint, radios, row};
use crate::canvas::{Fit, PageLayout};
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    card(ui, t, Some(tl!("Default page display")), |ui| {
        let defaults = &mut app.view_defaults;
        let continuous = defaults.continuous;
        // The pictures follow Continuous scrolling, so the tile shows the display a PDF opens in.
        let layouts: Vec<_> = PageLayout::ORDER
            .into_iter()
            .map(|l| Tile { value: l, label: tl!(l.label()), picture: Picture::Pages { two_up: l == PageLayout::TwoUp, continuous } })
            .collect();
        tiles::choose(ui, &mut defaults.layout, &layouts);
        ui.add(widgets::checkbox(&mut defaults.continuous, tl!("Continuous scrolling")));
        ui.add(widgets::checkbox(&mut defaults.show_page_gaps, tl!("Show gaps between pages")));
        hint(ui, t, tl!("Hide gaps to join page edges. Margins inside the PDF remain unchanged."));
    });
    card(ui, t, Some(tl!("Default zoom")), |ui| {
        let defaults = &mut app.view_defaults;
        tiles::choose(
            ui,
            &mut defaults.fit,
            &[
                Tile { value: Fit::Width, label: tl!("Fit to width"), picture: Picture::Zoom(Fit::Width) },
                Tile { value: Fit::Page, label: tl!("Zoom to page level"), picture: Picture::Zoom(Fit::Page) },
                Tile { value: Fit::None, label: tl!("Custom"), picture: Picture::Zoom(Fit::None) },
            ],
        );
        row(ui, |ui| {
            let label = ui.label(egui::RichText::new(tl!("Zoom")).color(t.text_muted));
            // Editing the percentage selects Custom.
            let mut percent = defaults.zoom * 100.0;
            if ui.add(egui::DragValue::new(&mut percent).range(8.0..=6400.0).max_decimals(0).suffix("%")).labelled_by(label.id).changed() {
                (defaults.fit, defaults.zoom) = (Fit::None, percent / 100.0);
            }
        });
        hint(ui, t, tl!("Used when a PDF doesn't ask for a layout or zoom. Continuous scrolling never snaps between pages."));
        ui.add(widgets::checkbox(&mut app.always_default_view, tl!("Always use my default zoom and page layout")));
        // Acrobat's "Restore last view settings when reopening documents" (#1767).
        #[cfg(not(target_arch = "wasm32"))]
        ui.add(widgets::checkbox(
            &mut app.restore_last_view,
            tl!("Restore the last view (page, zoom and page display) when a document is opened again"),
        ));
    });
    card(ui, t, Some(tl!("Scroll speed (mouse wheel and arrow keys)")), |ui| {
        radios(ui, &mut app.scroll_speed, crate::scroll_speed::ScrollSpeed::ALL.map(|s| (s, tl_ctx!("scroll speed", s.label()))));
    });
    card(ui, t, Some(tl!("Units")), |ui| {
        radios(ui, &mut app.units, crate::units::Unit::ALL.map(|u| (u, u.label())));
        hint(ui, t, tl!("For page sizes and margins."));
    });
}
