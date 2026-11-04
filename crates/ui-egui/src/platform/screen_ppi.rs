//! Preferences ▸ Documents and view ▸ Screen resolution: how many of the screen's own pixels make
//! an inch, so 100% (Actual size) draws a page at its real size on the glass (#739).
//!
//! `Standard` keeps PdfCraft's fixed basis, 96 logical pixels per inch ([`crate::canvas::PT`]),
//! whatever the screen. A chosen resolution is in device pixels per inch, typed or measured with a
//! ruler (Calibrate…), and is divided by the current pixels-per-point when drawing, so it stays
//! true when the system's display scaling or PdfCraft's interface size changes. Every fixed
//! percentage scales from that basis, as in Acrobat's Page Display ▸ Custom Resolution; Fit modes
//! still fit the window.

use crate::canvas::PT;
use crate::theme::{self, Tokens};
use crate::units::Unit;
use crate::{Dialog, PdfCraftApp, widgets};

/// The lowest and highest device resolutions accepted, in pixels per inch: below any desktop
/// monitor or projector a person would read a page on, and above any phone panel.
pub const MIN: f32 = 40.0;
pub const MAX: f32 = 1200.0;

/// A device resolution from settings, the control channel or a calibration: finite and within
/// [`MIN`]..=[`MAX`], or `None`.
pub fn valid(ppi: f64) -> Option<f32> {
    let ppi = ppi as f32;
    (ppi.is_finite() && (MIN..=MAX).contains(&ppi)).then_some(ppi)
}

/// Logical pixels per PDF point at 100%: [`PT`] for `Standard` (`None`), or the device
/// resolution brought back to logical pixels at `pixels_per_point`.
pub fn basis(ppi: Option<f32>, pixels_per_point: f32) -> f32 {
    match ppi {
        None => PT,
        Some(ppi) => ppi / usable(pixels_per_point) / 72.0,
    }
}

/// The device resolution a calibration gives: a line `points` logical points long, drawn at
/// `pixels_per_point`, measured on the screen as `inches`. `None` when the reading can't be right.
pub fn from_measure(points: f32, pixels_per_point: f32, inches: f64) -> Option<f32> {
    if !(inches.is_finite() && inches > 0.0) {
        return None;
    }
    valid(f64::from(points) * f64::from(usable(pixels_per_point)) / inches)
}

/// The length a line `points` logical points long should measure on the screen at `ppi`, in
/// inches (what the calibration window shows once a resolution is set, to check it).
pub fn expected_inches(points: f32, pixels_per_point: f32, ppi: f32) -> f64 {
    f64::from(points) * f64::from(usable(pixels_per_point)) / f64::from(ppi.max(MIN))
}

/// The display's scale, or 1.0 when egui doesn't know it (or reports nonsense).
fn usable(pixels_per_point: f32) -> f32 {
    if pixels_per_point.is_finite() && pixels_per_point > 0.0 { pixels_per_point } else { 1.0 }
}

/// Parse a control-channel value: `standard`, or a device resolution such as `163` or `163ppi`.
pub fn parse(value: &str) -> Result<Option<f32>, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("standard") {
        return Ok(None);
    }
    let number = value.strip_suffix("ppi").unwrap_or(value).trim();
    let ppi: f64 = number.parse().map_err(|_| format!("screen-ppi must be standard or pixels per inch, not {value}"))?;
    valid(ppi).map(Some).ok_or_else(|| format!("screen-ppi must be between {MIN} and {MAX} pixels per inch"))
}

/// Preferences ▸ Documents and view: the Screen resolution row, with Calibrate….
pub(crate) fn preferences_row(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.label(tl!("Screen resolution"));
        widgets::radio_value(ui, &mut app.screen_ppi, None, tl!("Standard (96 ppi)"));
        // Custom starts from what Standard draws at on this display, so choosing it changes nothing yet.
        let standard = (96.0 * ui.ctx().pixels_per_point()).clamp(MIN, MAX);
        if ui.add(widgets::radio(app.screen_ppi.is_some(), tl!("Custom"))).clicked() && app.screen_ppi.is_none() {
            app.screen_ppi = Some(standard.round());
        }
        if let Some(ppi) = app.screen_ppi.as_mut() {
            ui.add(egui::DragValue::new(ppi).range(MIN..=MAX).max_decimals(1).speed(0.1).suffix(" ppi"));
        }
        if ui.button(tl!("Calibrate…")).clicked() {
            app.calibration_readings = [0.0; LINES.len()];
            app.dialog = Some(Dialog::CalibrateScreen);
        }
    });
    ui.label(
        egui::RichText::new(tl!(
            "The screen's own pixels per inch. Calibrate with a ruler so that Actual size (100%) shows pages at their real size."
        ))
        .small()
        .color(t.text_muted),
    );
}

/// The Calibrate screen window: lines of the whole width, a half and a quarter of the window,
/// so a short ruler can measure one. Each reading gives a resolution; several are averaged.
/// Returns the dialog to show next: Preferences once applied or cancelled, else this one.
pub(crate) fn calibrate_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> Dialog {
    ui.label(egui::RichText::new(tl!("Calibrate screen")).font(theme::semibold(18.0)));
    ui.label(tl!(
        "Measure one of the lines below with a ruler, from one end mark to the other, and type its length beside it. The longest line you can measure is the most accurate; measuring several averages them."
    ));
    ui.add_space(8.0);
    let ppp = ui.ctx().pixels_per_point();
    // Points aren't a ruler's unit: they read in millimetres.
    let unit = if app.units == Unit::Points { Unit::Millimetres } else { app.units };
    let width = ui.available_width();
    let mut estimates = Vec::with_capacity(LINES.len());
    let mut wrong = false;
    for (i, fraction) in LINES.into_iter().enumerate() {
        // Whole points, so the line ends on pixel edges at common scales.
        let points = (width * fraction).floor().max(60.0);
        ui.add_space(12.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(points, 28.0), egui::Sense::hover());
        let stroke = egui::Stroke::new(1.0, t.text);
        ui.painter().hline(rect.x_range(), rect.center().y, stroke);
        for x in [rect.left(), rect.right()] {
            ui.painter().vline(x, rect.y_range(), stroke);
        }
        let Some(reading) = app.calibration_readings.get_mut(i) else { continue };
        ui.horizontal(|ui| {
            ui.label(tl!("Measured length"));
            ui.push_id(i, |ui| {
                ui.add(
                    egui::DragValue::new(reading)
                        .range(0.0..=10_000.0)
                        .max_decimals(unit.decimals())
                        .speed(unit.speed())
                        .suffix(format!(" {}", unit.label())),
                );
            });
            if let Some(ppi) = app.screen_ppi {
                let expected = expected_inches(points, ppp, ppi) * 72.0 * unit.per_point();
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("At the current {ppi} ppi this line should measure {length} {unit}."),
                        &[("ppi", &format!("{ppi:.1}")), ("length", &format!("{expected:.prec$}", prec = unit.decimals())), ("unit", unit.label())],
                    ))
                    .color(t.text_muted),
                );
            }
        });
        if *reading > 0.0 {
            // The reading in the unit, to points (`per_point` is units per point), to inches.
            match from_measure(points, ppp, *reading / unit.per_point() / 72.0) {
                Some(ppi) => estimates.push(ppi),
                None => wrong = true,
            }
        }
    }
    ui.add_space(12.0);
    let average = average(&estimates);
    if wrong {
        ui.label(egui::RichText::new(tl!("That length can't be right for this line: check the unit.")).color(t.text));
    } else if average.is_some_and(|(_, agree)| !agree) {
        ui.label(egui::RichText::new(tl!("These measurements differ by more than 2%: check them.")).color(t.text));
    }
    ui.add_space(8.0);
    let mut next = Dialog::CalibrateScreen;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Apply"), true).clicked()
            && !wrong
            && let Some((ppi, _)) = average
        {
            app.screen_ppi = Some(ppi);
            next = Dialog::Preferences;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            next = Dialog::Preferences;
        }
    });
    next
}

/// The calibration lines, as fractions of the window's width.
pub const LINES: [f32; 3] = [1.0, 0.5, 0.25];

/// The mean of the resolutions measured, and whether they agree within 2%. `None` for none.
pub fn average(estimates: &[f32]) -> Option<(f32, bool)> {
    let (min, max) = estimates.iter().fold((f32::INFINITY, 0.0f32), |(lo, hi), &e| (lo.min(e), hi.max(e)));
    let mean = estimates.iter().sum::<f32>() / estimates.len().max(1) as f32;
    (!estimates.is_empty()).then_some((mean, max <= min * 1.02))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_keeps_96_dpi_whatever_the_display() {
        for ppp in [1.0, 1.25, 2.0, f32::NAN, 0.0] {
            assert_eq!(basis(None, ppp), PT, "{ppp}");
        }
    }

    #[test]
    fn a_resolution_is_physical_so_scaling_does_not_change_the_size_on_the_glass() {
        // A 27" 4K monitor (163 ppi) at 100%, 150% and 200% scaling: one PDF inch (72 pt) is
        // always 163 device pixels.
        for ppp in [1.0, 1.5, 2.0] {
            let device_px_per_inch = basis(Some(163.0), ppp) * 72.0 * ppp;
            assert!((device_px_per_inch - 163.0).abs() < 1e-3, "{ppp}: {device_px_per_inch}");
        }
        // 96 device ppi at 1× is the standard basis.
        assert!((basis(Some(96.0), 1.0) - PT).abs() < 1e-6);
        assert_eq!(basis(Some(144.0), f32::NAN), 2.0, "an unusable scale counts as 1×");
    }

    #[test]
    fn a_ruler_reading_gives_the_device_resolution() {
        // 1000 points at 1.5× is 1500 device pixels; measured 233.7 mm (9.2 in) is about 163 ppi.
        let ppi = from_measure(1000.0, 1.5, 233.7 / 25.4).expect("plausible");
        assert!((ppi - 163.0).abs() < 0.1, "{ppi}");
        assert!((expected_inches(1000.0, 1.5, ppi) - 233.7 / 25.4).abs() < 1e-3);
        for bad in [0.0, -3.0, f64::NAN, f64::INFINITY, 1000.0, 0.01] {
            assert_eq!(from_measure(1000.0, 1.5, bad), None, "{bad} in");
        }
    }

    #[test]
    fn several_readings_are_averaged_and_a_disagreement_is_flagged() {
        assert_eq!(average(&[]), None);
        assert_eq!(average(&[163.0]), Some((163.0, true)));
        let (mean, agree) = average(&[161.0, 162.0, 163.0]).expect("three, 1.2% apart");
        assert!((mean - 162.0).abs() < 1e-3 && agree, "{mean} {agree}");
        assert_eq!(average(&[150.0, 163.0]).map(|(_, agree)| agree), Some(false), "8% apart");
    }

    #[test]
    fn control_values_parse_or_say_what_is_allowed() {
        assert_eq!(parse("standard"), Ok(None));
        assert_eq!(parse(" Standard "), Ok(None));
        assert_eq!(parse("163"), Ok(Some(163.0)));
        assert_eq!(parse("108.5 ppi"), Ok(Some(108.5)));
        assert_eq!(parse("96ppi"), Ok(Some(96.0)));
        for bad in ["", "auto", "10", "5000", "NaN", "inf", "-163", "163 dpi"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
