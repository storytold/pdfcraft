//! The Measurements side panel: every saved measurement in the document, by page, with totals per
//! kind. A click shows the measurement on its page and makes it the Measure tool's reading.

use pdfcraft_engine::measure::{Kind, Listing, Measurement};

use super::right::empty;
use super::{Color32, DocInfo, Layout, Nav, Rect, Sense, Tokens, icons, theme, vec2};
use crate::canvas::DocView;

/// What the panel asks of the app once it has been drawn.
pub(super) enum Action {
    /// Export every saved measurement as CSV.
    Export,
}

/// Which kinds the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Only(Kind),
}

impl Filter {
    fn keeps(self, kind: Kind) -> bool {
        match self {
            Self::All => true,
            Self::Only(k) => k == kind,
        }
    }
}

/// The number of saved measurements, for the panel's title (the readable ones).
pub(super) fn count(view: &mut DocView, doc: &pdfcraft_engine::Document) -> Option<usize> {
    crate::measure_ui::listing(&mut view.measure, doc).as_ref().ok().map(|l| l.measurements.len())
}

/// The header's buttons, right to left after Close: export as CSV.
pub(super) fn header_controls(ui: &mut egui::Ui, any: bool) -> Option<Action> {
    let export = ui.add_enabled_ui(any, |ui| icons::button(ui, "file-spreadsheet", 26.0, false, tl!("Export measurements as CSV"))).inner;
    export.clicked().then_some(Action::Export)
}

/// Draw the list. Navigation goes through `nav`, as in the other side panels.
pub(super) fn show(ui: &mut egui::Ui, t: &Tokens, doc: &pdfcraft_engine::Document, view: &mut DocView, nav: &mut Option<Nav>) {
    let info = &doc.info;
    let selection = view.comments.selected;
    // The list is borrowed from the cache, so a click is noted by position and acted on after.
    let clicked = {
        let listing = match crate::measure_ui::listing(&mut view.measure, doc) {
            Ok(listing) => listing,
            Err(e) => {
                ui.colored_label(ERROR, e.as_str());
                return;
            }
        };
        list(ui, t, info, listing, selection)
    };
    let Some(i) = clicked else { return };
    let picked = crate::measure_ui::listing(&mut view.measure, doc).as_ref().ok().and_then(|l| l.measurements.get(i)).cloned();
    if let Some(m) = picked {
        view.comments.selected = Some((m.page, m.index));
        crate::measure_ui::show_saved(&mut view.measure, &m);
        *nav = Some(target(info, &m));
    }
}

const ERROR: Color32 = Color32::from_rgb(190, 50, 50);
const WARNING: Color32 = Color32::from_rgb(190, 120, 30);

/// Filters, totals and the rows by page; the position in `listing.measurements` of a row clicked.
fn list(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, listing: &Listing, selection: Option<(usize, usize)>) -> Option<usize> {
    if listing.measurements.is_empty() && listing.unsupported.is_empty() {
        empty(ui, t, "ruler", "This document has no measurements.");
        return None;
    }
    let filter_id = egui::Id::unique("measurement-filter");
    let mut filter: Filter = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    filter_row(ui, listing, &mut filter);
    ui.data_mut(|d| d.insert_temp(filter_id, filter));
    ui.add_space(4.0);
    totals(ui, t, listing, filter);
    if !listing.unsupported.is_empty() {
        let reasons: Vec<String> =
            listing.unsupported.iter().map(|u| format!("{} {}: {}", tl!("Page"), u.page.saturating_add(1), u.reason)).collect();
        let n = listing.unsupported.len();
        let text = crate::i18n::trn(crate::i18n::current(), n as u64, "{n} measurement can't be read", "{n} measurements can't be read");
        ui.label(egui::RichText::new(text).font(theme::regular(11.5)).color(WARNING)).on_hover_text(reasons.join("\n"));
    }
    if listing.truncated {
        ui.label(
            egui::RichText::new(tl!("Some measurements are not listed: the document has too many.")).font(theme::regular(11.5)).color(t.text_muted),
        );
    }
    ui.add_space(6.0);
    let mut shown = listing.measurements.iter().enumerate().filter(|(_, m)| filter.keeps(m.reading.kind)).peekable();
    if shown.peek().is_none() {
        ui.label(egui::RichText::new(tl!("No measurements of this kind.")).color(t.text_muted));
        return None;
    }
    let mut last_page = None;
    let mut clicked = None;
    for (i, m) in shown {
        if last_page != Some(m.page) {
            last_page = Some(m.page);
            ui.add_space(4.0);
            let label = info.pages.get(m.page).map(|p| p.label.as_str()).filter(|l| !l.is_empty());
            let n = m.page.saturating_add(1).to_string();
            let title = match label {
                Some(l) if *l != n => crate::i18n::fmt(tl!("Page {n} ({label})"), &[("n", &n), ("label", l)]),
                _ => crate::i18n::fmt(tl!("Page {n}"), &[("n", &n)]),
            };
            ui.label(egui::RichText::new(title).font(theme::semibold(12.0)).color(t.text_muted));
        }
        if row(ui, t, m, selection == Some((m.page, m.index))).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Where a click on `m` goes: its annotation's rectangle, flashed, or else its page.
fn target(info: &DocInfo, m: &Measurement) -> Nav {
    let rotation = info.pages.get(m.page).map_or(0, |p| p.rotation);
    match info.annotations.iter().find(|a| a.page == m.page && a.index == m.index) {
        Some(a) => Nav::Flash(m.page, a.shown_rect(rotation)),
        None => Nav::Page(m.page),
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Distance => "Distance",
        Kind::Perimeter => "Perimeter",
        Kind::Area => "Area",
    }
}

fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Distance => "ruler",
        Kind::Perimeter => "spline",
        Kind::Area => "pentagon",
    }
}

/// All, Distance, Perimeter, Area: each with how many there are.
fn filter_row(ui: &mut egui::Ui, listing: &Listing, filter: &mut Filter) {
    let count = |kind: Option<Kind>| listing.measurements.iter().filter(|m| kind.is_none_or(|k| m.reading.kind == k)).count();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (value, label, n) in [
            (Filter::All, tl!("All"), count(None)),
            (Filter::Only(Kind::Distance), tl!("Distance"), count(Some(Kind::Distance))),
            (Filter::Only(Kind::Perimeter), tl!("Perimeter"), count(Some(Kind::Perimeter))),
            (Filter::Only(Kind::Area), tl!("Area"), count(Some(Kind::Area))),
        ] {
            if (value == Filter::All || n > 0) && ui.selectable_label(*filter == value, format!("{label} {n}")).clicked() {
                *filter = value;
            }
        }
    });
    // A kind whose last measurement was deleted leaves nothing to filter by.
    if let Filter::Only(k) = *filter
        && count(Some(k)) == 0
    {
        *filter = Filter::All;
    }
}

/// One total per kind and unit, as the measurements' own scales give them.
fn sums(listing: &Listing, filter: Filter) -> Vec<(Kind, String, f64, u8, usize)> {
    let mut out: Vec<(Kind, String, f64, u8, usize)> = Vec::new();
    for m in listing.measurements.iter().filter(|m| filter.keeps(m.reading.kind) && m.reading.value.is_finite()) {
        let unit = &m.reading.unit;
        match out.iter_mut().find(|(k, u, ..)| *k == m.reading.kind && u == unit) {
            Some((_, _, sum, precision, n)) => {
                *sum += m.reading.value;
                *precision = (*precision).max(m.scale.precision);
                *n = n.saturating_add(1);
            }
            None => out.push((m.reading.kind, unit.clone(), m.reading.value, m.scale.precision, 1)),
        }
    }
    out.retain(|(.., sum, _, _)| sum.is_finite());
    out
}

fn totals(ui: &mut egui::Ui, t: &Tokens, listing: &Listing, filter: Filter) {
    let sums = sums(listing, filter);
    if sums.is_empty() {
        return;
    }
    egui::Frame::NONE.fill(t.hover).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(egui::RichText::new(tl!("Totals")).font(theme::semibold(12.0)).color(t.text_muted));
        for (kind, unit, sum, precision, n) in sums {
            let precision = usize::from(precision.min(10));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} ({n})", tl!(kind_label(kind)))).font(theme::regular(12.5)).color(t.text));
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{sum:.precision$} {unit}")).font(theme::semibold(12.5)).color(t.text));
                });
            });
        }
    });
}

/// A measurement's row: kind icon, value, then its label, author and scale in small text.
fn row(ui: &mut egui::Ui, t: &Tokens, m: &Measurement, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
    let kind = tl!(kind_label(m.reading.kind));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, format!("{kind}: {}", m.reading.label)));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, selected);
    let body = rect.translate(press.offset());
    icons::paint(ui, Rect::from_min_size(body.min + vec2(4.0, 11.0), vec2(18.0, 18.0)), kind_icon(m.reading.kind), 16.0, t.icon);
    let width = (rect.width() - 36.0).max(0.0);
    let value = if m.label.is_empty() { m.reading.label.clone() } else { format!("{} — {}", m.reading.label, m.label) };
    crate::widgets::paint_left(ui, body.left_top() + vec2(30.0, 13.0), &value, theme::medium(13.0), t.text, width);
    let mut details = vec![kind.to_string()];
    if !m.scale.ratio.is_empty() {
        details.push(m.scale.ratio.clone());
    }
    if !m.author.is_empty() {
        details.push(m.author.clone());
    }
    crate::widgets::paint_left(ui, body.left_top() + vec2(30.0, 30.0), &details.join(" · "), theme::regular(11.0), t.text_faint, width);
    theme::hand(resp).on_hover_text(tl!("Show on the page"))
}

#[cfg(test)]
mod tests {
    use pdfcraft_engine::measure::{Reading, Scale};

    use super::*;

    fn measurement(kind: Kind, value: f64, unit: &str) -> Measurement {
        Measurement {
            page: 0,
            index: 0,
            id: String::new(),
            author: String::new(),
            label: String::new(),
            points: Vec::new(),
            scale: Scale::default(),
            reading: Reading {
                kind,
                value,
                unit: unit.into(),
                label: format!("{value} {unit}"),
                delta_x: 0.0,
                delta_y: 0.0,
                angle: 0.0,
                length: 0.0,
                area: 0.0,
            },
        }
    }

    #[test]
    fn totals_add_up_per_kind_and_unit() {
        let listing = Listing {
            measurements: vec![
                measurement(Kind::Distance, 1.5, "m"),
                measurement(Kind::Distance, 2.0, "m"),
                measurement(Kind::Distance, 3.0, "ft"),
                measurement(Kind::Area, 4.0, "m²"),
            ],
            ..Default::default()
        };
        let all = sums(&listing, Filter::All);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].0, Kind::Distance);
        assert_eq!(all[0].1, "m");
        assert!((all[0].2 - 3.5).abs() < 1e-9);
        assert_eq!(all[0].4, 2);
        let areas = sums(&listing, Filter::Only(Kind::Area));
        assert_eq!(areas.len(), 1);
        assert!((areas[0].2 - 4.0).abs() < 1e-9);
    }

    #[test]
    fn totals_skip_values_that_are_not_finite() {
        let listing = Listing {
            measurements: vec![
                measurement(Kind::Distance, f64::NAN, "m"),
                measurement(Kind::Distance, f64::MAX, "in"),
                measurement(Kind::Distance, f64::MAX, "in"),
            ],
            ..Default::default()
        };
        assert!(sums(&listing, Filter::All).is_empty());
        assert!(sums(&Listing::default(), Filter::All).is_empty());
    }
}
