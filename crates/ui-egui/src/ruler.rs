//! Pure geometry and tick selection for document rulers.
//!
//! The canvas remains the authority for the final displayed page origin, length and scale. This
//! module only converts that one-dimensional geometry into visible ruler ticks; it does not lay
//! out pages, read scroll state or paint widgets.

use crate::units::Unit;

const MIN_MINOR_SPACING: f64 = 6.0;
const MIN_LABEL_SPACING: f64 = 48.0;
const MAX_TICKS: usize = 16_384;

const INCH_INTERVALS: &[f64] = &[
    1.0 / 64.0,
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
    64.0,
    128.0,
    256.0,
    512.0,
    1_024.0,
    2_048.0,
    4_096.0,
];
const METRIC_INTERVALS: &[f64] =
    &[0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0, 50_000.0, 100_000.0];
const POINT_INTERVALS: &[f64] = &[
    1.0,
    2.0,
    5.0,
    10.0,
    20.0,
    50.0,
    100.0,
    200.0,
    500.0,
    1_000.0,
    2_000.0,
    5_000.0,
    10_000.0,
    20_000.0,
    50_000.0,
    100_000.0,
    200_000.0,
    500_000.0,
    1_000_000.0,
];

/// Screen axis measured by a ruler. Values grow rightward or downward in displayed-page space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulerOrientation {
    Horizontal,
    Vertical,
}

/// Importance of a generated ruler tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickKind {
    Minor,
    Quarter,
    Medium,
    Major,
}

/// Tick cadence selected for a unit at the current displayed scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickSpec {
    pub minor_interval_units: f64,
    pub medium_every: u32,
    pub major_every: u32,
    pub label_every: u32,
}

/// One visible tick, in logical screen points and the selected display unit.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerTick {
    pub screen_position: f64,
    pub value: f64,
    pub kind: TickKind,
    pub label: Option<String>,
}

/// Final one-dimensional page geometry supplied by the canvas.
///
/// `ruler_start` and `ruler_end` are the visible ruler bounds in logical screen points.
/// `page_origin` is the displayed page's left or top edge on that same axis. The page length is in
/// PDF points and `screen_points_per_pdf_point` is the final scale used to draw the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerGeometry {
    pub orientation: RulerOrientation,
    pub ruler_start: f64,
    pub ruler_end: f64,
    pub page_origin: f64,
    pub page_length_pdf_points: f64,
    pub screen_points_per_pdf_point: f64,
    pub unit: Unit,
}

impl RulerGeometry {
    /// Whether all geometry can be converted without division by zero or non-finite coordinates.
    pub fn is_valid(self) -> bool {
        self.ruler_start.is_finite()
            && self.ruler_end.is_finite()
            && self.ruler_end > self.ruler_start
            && self.page_origin.is_finite()
            && self.page_length_pdf_points.is_finite()
            && self.page_length_pdf_points >= 0.0
            && self.screen_points_per_pdf_point.is_finite()
            && self.screen_points_per_pdf_point > 0.0
            && (self.page_origin + self.page_length_pdf_points * self.screen_points_per_pdf_point).is_finite()
    }

    /// Logical screen points occupied by one selected display unit.
    pub fn screen_points_per_unit(self) -> Option<f64> {
        if !self.is_valid() {
            return None;
        }
        let per_point = self.unit.per_point();
        let scale = self.screen_points_per_pdf_point / per_point;
        scale.is_finite().then_some(scale)
    }
}

/// Choose a readable labelled interval and divide it into eight standard-ruler subdivisions.
pub fn choose_tick_spec(unit: Unit, screen_points_per_unit: f64) -> Option<TickSpec> {
    if !screen_points_per_unit.is_finite() || screen_points_per_unit <= 0.0 {
        return None;
    }
    if unit == Unit::Inches {
        return inch_tick_spec(screen_points_per_unit);
    }

    // Choose a natural whole-number label interval with enough room for text and eight visible
    // subdivisions. Subdivision marks remain visible even though metric and point labels suppress
    // non-whole values.
    let minimum_label_interval = (MIN_LABEL_SPACING / screen_points_per_unit).max(8.0 * MIN_MINOR_SPACING / screen_points_per_unit).max(1.0);
    let label_interval = intervals(unit).iter().copied().find(|interval| *interval >= minimum_label_interval)?;
    Some(eight_subdivision_spec(label_interval))
}

/// Inch rulers follow the observed binary label ladder with eight subdivisions per label interval.
fn inch_tick_spec(screen_points_per_inch: f64) -> Option<TickSpec> {
    if !screen_points_per_inch.is_finite() || screen_points_per_inch <= 0.0 {
        return None;
    }

    // The label ladder follows the observed zoom policy. Every labelled interval always contains
    // eight equal subdivisions. Text stops at 1/32 inch; the 1/64, 1/128 and 1/256 marks remain
    // visible at high zoom without labels.
    let label_interval = if screen_points_per_inch < 4.5 {
        32.0
    } else if screen_points_per_inch < 9.0 {
        16.0
    } else if screen_points_per_inch < 18.0 {
        8.0
    } else if screen_points_per_inch < 36.0 {
        4.0
    } else if screen_points_per_inch < 72.0 {
        2.0
    } else if screen_points_per_inch < 216.0 {
        1.0
    } else if screen_points_per_inch < 432.0 {
        1.0 / 4.0
    } else if screen_points_per_inch < 864.0 {
        1.0 / 8.0
    } else if screen_points_per_inch < 1_728.0 {
        1.0 / 16.0
    } else {
        1.0 / 32.0
    };
    Some(eight_subdivision_spec(label_interval))
}

/// Builds the tick hierarchy for units divided into eighths.
fn eight_subdivision_spec(label_interval_units: f64) -> TickSpec {
    TickSpec { minor_interval_units: label_interval_units / 8.0, medium_every: 4, major_every: 8, label_every: 8 }
}

/// Classifies a tick as major, medium, minor, or subminor within its interval.
fn tick_kind(index: i64, spec: TickSpec) -> TickKind {
    let within_label = index.rem_euclid(i64::from(spec.label_every));
    if within_label == 0 {
        TickKind::Major
    } else if within_label == i64::from(spec.medium_every) {
        TickKind::Medium
    } else if within_label == i64::from(spec.medium_every / 2) || within_label == i64::from(spec.medium_every + spec.medium_every / 2) {
        TickKind::Quarter
    } else {
        TickKind::Minor
    }
}

/// Convert PDF points from the displayed page origin to a logical screen position.
pub fn page_points_to_screen(geometry: RulerGeometry, page_points: f64) -> Option<f64> {
    if !geometry.is_valid() || !page_points.is_finite() {
        return None;
    }
    let screen = geometry.page_origin + page_points * geometry.screen_points_per_pdf_point;
    screen.is_finite().then_some(screen)
}

/// Convert a logical screen position to PDF points from the displayed page origin.
pub fn screen_to_page_points(geometry: RulerGeometry, screen_position: f64) -> Option<f64> {
    if !geometry.is_valid() || !screen_position.is_finite() {
        return None;
    }
    let points = (screen_position - geometry.page_origin) / geometry.screen_points_per_pdf_point;
    points.is_finite().then_some(points)
}

/// Generate ticks across the complete ruler. The page supplies a stable origin and scale but does
/// not clip the ticks; measurements continue through the pasteboard on both sides.
pub fn visible_ticks(geometry: RulerGeometry) -> Vec<RulerTick> {
    let Some(screen_points_per_unit) = geometry.screen_points_per_unit() else {
        return Vec::new();
    };
    let Some(spec) = choose_tick_spec(geometry.unit, screen_points_per_unit) else {
        return Vec::new();
    };
    let visible_start = geometry.ruler_start;
    let visible_end = geometry.ruler_end;
    let per_point = geometry.unit.per_point();
    let start_units = (visible_start - geometry.page_origin) / geometry.screen_points_per_pdf_point * per_point;
    let end_units = (visible_end - geometry.page_origin) / geometry.screen_points_per_pdf_point * per_point;
    if !start_units.is_finite() || !end_units.is_finite() || end_units < start_units {
        return Vec::new();
    }

    let first_f = (start_units / spec.minor_interval_units).ceil();
    let last_f = (end_units / spec.minor_interval_units).floor();
    if first_f < i64::MIN as f64 || first_f > i64::MAX as f64 || last_f < i64::MIN as f64 || last_f > i64::MAX as f64 {
        return Vec::new();
    }
    let first = first_f as i64;
    let last = last_f as i64;
    if last < first {
        return Vec::new();
    }
    let available = last.saturating_sub(first).saturating_add(1);
    let count = usize::try_from(available).unwrap_or(MAX_TICKS).min(MAX_TICKS);
    let mut ticks = Vec::with_capacity(count);
    for offset in 0..count {
        let Some(index) = first.checked_add(offset as i64) else {
            break;
        };
        let value = index as f64 * spec.minor_interval_units;
        let screen_position = geometry.page_origin + value * screen_points_per_unit;
        if !screen_position.is_finite() {
            break;
        }
        let kind = tick_kind(index, spec);
        let label = label_tick(geometry.unit, value, index, spec, screen_points_per_unit);
        ticks.push(RulerTick { screen_position, value, kind, label });
    }
    ticks
}

/// Returns the number of minor intervals contained in one major tick span.
fn intervals(unit: Unit) -> &'static [f64] {
    match unit {
        Unit::Inches => INCH_INTERVALS,
        Unit::Millimetres | Unit::Centimetres => METRIC_INTERVALS,
        Unit::Points => POINT_INTERVALS,
    }
}

/// Returns whether a tick index aligns with the requested interval.
fn divisible(index: i64, every: u32) -> bool {
    every != 0 && index.rem_euclid(i64::from(every)) == 0
}

/// Returns whether this tick receives a numeric ruler label.
fn label_tick(unit: Unit, value: f64, index: i64, spec: TickSpec, _screen_points_per_unit: f64) -> Option<String> {
    if !divisible(index, spec.label_every) {
        return None;
    }
    if unit != Unit::Inches && !is_whole_number(value) {
        return None;
    }
    Some(format_label(unit, value))
}

/// Formats a major ruler label in the selected display unit.
fn format_label(unit: Unit, value: f64) -> String {
    if unit == Unit::Inches {
        return format_inch_label(value);
    }

    let rounded = value.abs().round();
    if rounded == 0.0 { "0".to_owned() } else { format!("{rounded:.0}") }
}

/// Returns whether a finite value is effectively an integer at ruler precision.
fn is_whole_number(value: f64) -> bool {
    value.is_finite() && (value - value.round()).abs() <= f64::EPSILON * value.abs().max(1.0) * 8.0
}

/// Acrobat labels each whole inch with its magnitude and labels subdivisions with only their
/// reduced binary fraction. The pattern therefore repeats `1/4, 1/2, 3/4, 1, 1/4 ...` rather than
/// using decimals or mixed numbers. Signed spread coordinates use the same positive magnitudes on
/// both sides of zero.
fn format_inch_label(value: f64) -> String {
    const DENOMINATOR: i64 = 64;
    let sixty_fourths = (value.abs() * DENOMINATOR as f64).round() as i64;
    if sixty_fourths == 0 {
        return "0".to_owned();
    }
    let whole = sixty_fourths / DENOMINATOR;
    let numerator = sixty_fourths % DENOMINATOR;
    if numerator == 0 {
        return whole.to_string();
    }
    let divisor = gcd(numerator, DENOMINATOR);
    format!("{}/{}", numerator / divisor, DENOMINATOR / divisor)
}

/// Computes the greatest common divisor used to reduce fractional inch labels.
fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs().max(1)
}

/// Compact, screen-fixed ruler bands modelled on the reference application's canvas chrome.
/// Zoom changes tick cadence, never the amount of document viewport reserved by the rulers.
pub const HORIZONTAL_RULER_HEIGHT: f32 = 18.0;
pub const VERTICAL_RULER_WIDTH: f32 = 20.0;
const HORIZONTAL_LABEL_OUTER_PADDING: f32 = 1.0;
const HORIZONTAL_LABEL_AFTER_TICK: f32 = 3.0;
const VERTICAL_LABEL_AFTER_TICK: f32 = 3.0;
const RULER_LABEL_SIZE: f32 = 9.0;

/// Read-only paint input for one ruler band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerPaintInput {
    pub orientation: RulerOrientation,
    pub rect: egui::Rect,
    pub page_origin: f64,
    pub page_length_pdf_points: f64,
    pub screen_points_per_pdf_point: f64,
    pub unit: Unit,
    /// Pointer position on this ruler's screen axis. The caller supplies it only while the
    /// pointer is over the document canvas.
    pub pointer_screen_position: Option<f64>,
}

impl RulerPaintInput {
    /// Normalizes ruler input into finite geometry used for ticks and pointer markers.
    fn calculation_geometry(self) -> RulerGeometry {
        let (ruler_start, ruler_end) = match self.orientation {
            RulerOrientation::Horizontal => (f64::from(self.rect.left()), f64::from(self.rect.right())),
            RulerOrientation::Vertical => (f64::from(self.rect.top()), f64::from(self.rect.bottom())),
        };
        RulerGeometry {
            orientation: self.orientation,
            ruler_start,
            ruler_end,
            page_origin: self.page_origin,
            page_length_pdf_points: self.page_length_pdf_points,
            screen_points_per_pdf_point: self.screen_points_per_pdf_point,
            unit: self.unit,
        }
    }
}

/// Paint one clipped read-only ruler band. The caller owns layout and pointer policy.
pub fn paint_ruler(ui: &egui::Ui, input: RulerPaintInput) {
    let style = RulerPaintStyle::from_ui(ui);
    let painter = ui.painter().with_clip_rect(input.rect);
    painter.rect_filled(input.rect, egui::CornerRadius::ZERO, style.outside_background);

    let calculation = input.calculation_geometry();

    let ticks = visible_ticks(calculation);
    for kind in [TickKind::Minor, TickKind::Quarter, TickKind::Medium, TickKind::Major] {
        for tick in ticks.iter().filter(|tick| tick.kind == kind) {
            paint_tick(&painter, input, tick, style);
        }
    }
    if let Some(pointer) = input.pointer_screen_position.and_then(screen_coordinate) {
        paint_pointer_marker(&painter, input.orientation, input.rect, pointer, style.pointer);
    }
    paint_canvas_edge(&painter, input.orientation, input.rect, style.edge);
}

/// Paint the inert square where horizontal and vertical rulers meet.
pub fn paint_corner(ui: &egui::Ui, rect: egui::Rect, unit: Unit) {
    let style = RulerPaintStyle::from_ui(ui);
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, egui::CornerRadius::ZERO, style.outside_background);
    painter.line_segment([rect.right_top(), rect.right_bottom()], style.edge);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], style.edge);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, unit.label(), crate::theme::regular(RULER_LABEL_SIZE), style.label);
}

#[derive(Clone, Copy)]
struct RulerPaintStyle {
    outside_background: egui::Color32,
    tick: egui::Stroke,
    major_tick: egui::Stroke,
    edge: egui::Stroke,
    label: egui::Color32,
    pointer: egui::Color32,
}

impl RulerPaintStyle {
    /// Builds ruler colors and strokes from the active egui visual style.
    fn from_ui(ui: &egui::Ui) -> Self {
        let tokens = crate::theme::Tokens::get(ui.ctx());
        Self {
            outside_background: tokens.chrome,
            tick: egui::Stroke::new(1.0, tokens.text_muted),
            major_tick: egui::Stroke::new(1.0, tokens.text),
            edge: egui::Stroke::new(1.0, tokens.border),
            label: tokens.text,
            pointer: tokens.accent,
        }
    }
}

/// Paints tick.
fn paint_tick(painter: &egui::Painter, input: RulerPaintInput, tick: &RulerTick, style: RulerPaintStyle) {
    let Some(at) = screen_coordinate(tick.screen_position) else { return };
    let length = tick_length(input.rect, input.orientation, tick.kind);
    let stroke = if tick.kind == TickKind::Major { style.major_tick } else { style.tick };
    match input.orientation {
        RulerOrientation::Horizontal => {
            painter.line_segment([egui::pos2(at, input.rect.bottom()), egui::pos2(at, input.rect.bottom() - length)], stroke);
            if let Some(label) = &tick.label
                && horizontal_label_fits(input.rect, at, label)
            {
                painter.text(
                    egui::pos2(at + HORIZONTAL_LABEL_AFTER_TICK, input.rect.top() + HORIZONTAL_LABEL_OUTER_PADDING),
                    egui::Align2::LEFT_TOP,
                    label,
                    crate::theme::regular(RULER_LABEL_SIZE),
                    style.label,
                );
            }
        }
        RulerOrientation::Vertical => {
            painter.line_segment([egui::pos2(input.rect.right(), at), egui::pos2(input.rect.right() - length, at)], stroke);
            if let Some(label) = &tick.label {
                paint_vertical_label(painter, input.rect, at, label, style.label);
            }
        }
    }
}

/// Handles estimated label width.
fn estimated_label_width(label: &str) -> f32 {
    label.chars().count() as f32 * RULER_LABEL_SIZE * 0.62
}

/// Handles horizontal label fits.
fn horizontal_label_fits(rect: egui::Rect, at: f32, label: &str) -> bool {
    let left = at + HORIZONTAL_LABEL_AFTER_TICK;
    left >= rect.left() + 1.0 && left + estimated_label_width(label) <= rect.right() - 1.0
}

/// Paints vertical label.
fn paint_vertical_label(painter: &egui::Painter, rect: egui::Rect, at: f32, label: &str, color: egui::Color32) {
    let galley = painter.layout_no_wrap(label.to_owned(), crate::theme::regular(RULER_LABEL_SIZE), color);
    if !vertical_label_fits(rect, at, galley.size()) {
        return;
    }
    // Rotating clockwise makes the baseline run down the ruler's Y axis. The left-centre anchor
    // holds the label's start on its tick and centres the glyph height inside the ruler band.
    let shape = egui::epaint::TextShape::new(egui::pos2(rect.center().x, at + VERTICAL_LABEL_AFTER_TICK), galley, color)
        .with_angle_and_anchor(std::f32::consts::FRAC_PI_2, egui::Align2::LEFT_CENTER);
    painter.add(shape);
}

/// Handles vertical label fits.
fn vertical_label_fits(rect: egui::Rect, at: f32, unrotated_size: egui::Vec2) -> bool {
    // After a quarter turn, the galley's height lies across the band and its width runs downward.
    unrotated_size.y <= rect.width() - 2.0 && at >= rect.top() + 1.0 && at + unrotated_size.x <= rect.bottom() - 1.0
}

/// Handles tick length.
fn tick_length(rect: egui::Rect, orientation: RulerOrientation, kind: TickKind) -> f32 {
    let available = match orientation {
        RulerOrientation::Horizontal => (rect.height() - 1.0).max(0.0),
        RulerOrientation::Vertical => (rect.width() - 1.0).max(0.0),
    };
    let desired: f32 = match (orientation, kind) {
        (RulerOrientation::Horizontal, TickKind::Minor) => 4.0,
        (RulerOrientation::Horizontal, TickKind::Quarter) => 6.0,
        (RulerOrientation::Horizontal, TickKind::Medium) => 8.0,
        (RulerOrientation::Horizontal, TickKind::Major) => 11.0,
        (RulerOrientation::Vertical, TickKind::Minor) => 4.0,
        (RulerOrientation::Vertical, TickKind::Quarter) => 6.0,
        (RulerOrientation::Vertical, TickKind::Medium) => 8.0,
        (RulerOrientation::Vertical, TickKind::Major) => 11.0,
    };
    desired.min(available)
}

/// Paints canvas edge.
fn paint_canvas_edge(painter: &egui::Painter, orientation: RulerOrientation, rect: egui::Rect, stroke: egui::Stroke) {
    let points = match orientation {
        RulerOrientation::Horizontal => [rect.left_bottom(), rect.right_bottom()],
        RulerOrientation::Vertical => [rect.right_top(), rect.right_bottom()],
    };
    painter.line_segment(points, stroke);
}

/// Paints pointer marker.
fn paint_pointer_marker(painter: &egui::Painter, orientation: RulerOrientation, rect: egui::Rect, axis_position: f32, color: egui::Color32) {
    let points = pointer_marker_line(orientation, rect, axis_position);
    painter.line_segment(points, egui::Stroke::new(1.0, color));
}

/// Handles pointer marker line.
fn pointer_marker_line(orientation: RulerOrientation, rect: egui::Rect, axis_position: f32) -> [egui::Pos2; 2] {
    match orientation {
        RulerOrientation::Horizontal => [egui::pos2(axis_position, rect.top()), egui::pos2(axis_position, rect.bottom())],
        RulerOrientation::Vertical => [egui::pos2(rect.left(), axis_position), egui::pos2(rect.right(), axis_position)],
    }
}

/// Handles screen coordinate.
fn screen_coordinate(value: f64) -> Option<f32> {
    (value.is_finite() && (f64::from(f32::MIN)..=f64::from(f32::MAX)).contains(&value)).then_some(value as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Handles geometry.
    fn geometry(unit: Unit, scale: f64) -> RulerGeometry {
        RulerGeometry {
            orientation: RulerOrientation::Horizontal,
            ruler_start: 0.0,
            ruler_end: 1_000.0,
            page_origin: 200.0,
            page_length_pdf_points: 612.0,
            screen_points_per_pdf_point: scale,
            unit,
        }
    }

    #[test]
    /// Verifies that page and screen coordinates round trip.
    fn page_and_screen_coordinates_round_trip() {
        let g = geometry(Unit::Points, 2.0);
        assert_eq!(page_points_to_screen(g, 0.0), Some(200.0));
        assert_eq!(page_points_to_screen(g, 72.0), Some(344.0));
        assert_eq!(screen_to_page_points(g, 344.0), Some(72.0));
    }

    #[test]
    /// Verifies that vertical geometry uses the same displayed axis policy.
    fn vertical_geometry_uses_the_same_displayed_axis_policy() {
        let mut g = geometry(Unit::Millimetres, 1.25);
        g.orientation = RulerOrientation::Vertical;
        g.page_origin = 40.0;
        assert_eq!(page_points_to_screen(g, 80.0), Some(140.0));
        assert_eq!(screen_to_page_points(g, 140.0), Some(80.0));
    }

    #[test]
    /// Handles invalid geometry and positions are rejected.
    fn invalid_geometry_and_positions_are_rejected() {
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let g = geometry(Unit::Inches, scale);
            assert!(!g.is_valid());
            assert!(visible_ticks(g).is_empty());
            assert_eq!(page_points_to_screen(g, 10.0), None);
        }
        let g = geometry(Unit::Inches, 1.0);
        assert_eq!(page_points_to_screen(g, f64::NAN), None);
        assert_eq!(screen_to_page_points(g, f64::INFINITY), None);
    }

    #[test]
    /// Handles tick specs meet spacing and coarsen when zooming out.
    fn tick_specs_meet_spacing_and_coarsen_when_zooming_out() {
        for unit in Unit::ALL {
            let zoomed_in = choose_tick_spec(unit, 100.0).unwrap();
            let zoomed_out = choose_tick_spec(unit, 10.0).unwrap();
            assert!(zoomed_out.minor_interval_units >= zoomed_in.minor_interval_units);
            if unit != Unit::Inches {
                assert!(zoomed_in.minor_interval_units * 100.0 >= MIN_MINOR_SPACING);
                assert!(zoomed_out.minor_interval_units * 10.0 >= MIN_MINOR_SPACING);
            }
            assert!(zoomed_in.medium_every <= zoomed_in.major_every);
            if unit != Unit::Inches {
                assert!(zoomed_in.major_every <= zoomed_in.label_every);
            }
        }
    }

    #[test]
    /// Handles inch ticks and labels match every reference zoom band.
    fn inch_ticks_and_labels_match_every_reference_zoom_band() {
        let cases = [
            (0.72, 4.0, 32.0),
            (5.76, 2.0, 16.0),
            (9.0, 1.0, 8.0),
            (18.0, 1.0 / 2.0, 4.0),
            (36.0, 1.0 / 4.0, 2.0),
            (72.0, 1.0 / 8.0, 1.0),
            (216.0, 1.0 / 32.0, 1.0 / 4.0),
            (432.0, 1.0 / 64.0, 1.0 / 8.0),
            (864.0, 1.0 / 128.0, 1.0 / 16.0),
            (1_728.0, 1.0 / 256.0, 1.0 / 32.0),
        ];
        for (points_per_inch, minor, label_interval) in cases {
            let spec = inch_tick_spec(points_per_inch).unwrap();
            assert_eq!(spec.minor_interval_units, minor);
            assert_eq!(spec.label_every, 8);
            assert_eq!(minor * f64::from(spec.label_every), label_interval);
        }
    }

    #[test]
    /// Handles inch numbering stops at thirty seconds while ticks continue finer.
    fn inch_numbering_stops_at_thirty_seconds_while_ticks_continue_finer() {
        let far = inch_tick_spec(5.76).unwrap();
        assert_eq!(far.minor_interval_units, 2.0);
        assert_eq!(far.minor_interval_units * f64::from(far.label_every), 16.0);

        let thirty_seconds = inch_tick_spec(1_728.0).unwrap();
        assert_eq!(thirty_seconds.minor_interval_units, 1.0 / 256.0);
        assert_eq!(thirty_seconds.minor_interval_units * f64::from(thirty_seconds.label_every), 1.0 / 32.0);
        assert_eq!(thirty_seconds.label_every, 8);
    }

    #[test]
    /// Handles common scales have practical unit intervals.
    fn common_scales_have_practical_unit_intervals() {
        let inches = choose_tick_spec(Unit::Inches, 96.0).unwrap();
        assert_eq!(inches.minor_interval_units, 1.0 / 8.0);
        assert_eq!(inches.medium_every, 4);
        assert_eq!(inches.major_every, 8);
        assert_eq!(inches.label_every, 8);

        let millimetres = choose_tick_spec(Unit::Millimetres, 96.0 / 25.4).unwrap();
        assert_eq!(millimetres.minor_interval_units, 2.5);
        assert_eq!(millimetres.medium_every, 4);
        assert_eq!(millimetres.major_every, 8);
        assert_eq!(millimetres.label_every, 8);
    }

    #[test]
    /// Verifies that visible ticks are clipped ordered and labelled.
    fn visible_ticks_are_clipped_ordered_and_labelled() {
        let mut g = geometry(Unit::Inches, 96.0 / 72.0);
        g.ruler_start = 240.0;
        g.ruler_end = 500.0;
        let ticks = visible_ticks(g);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| tick.screen_position >= g.ruler_start && tick.screen_position <= g.ruler_end));
        assert!(ticks.windows(2).all(|pair| pair[0].screen_position < pair[1].screen_position && pair[0].value < pair[1].value));
        assert!(ticks.iter().any(|tick| tick.kind == TickKind::Major));
        assert!(ticks.iter().any(|tick| tick.label.is_some()));
    }

    #[test]
    /// Verifies that vertical point labels continue past nine hundred.
    fn vertical_point_labels_continue_past_nine_hundred() {
        let geometry = RulerGeometry {
            orientation: RulerOrientation::Vertical,
            ruler_start: 0.0,
            ruler_end: 1_600.0,
            page_origin: 0.0,
            page_length_pdf_points: 792.0,
            screen_points_per_pdf_point: 1.0,
            unit: Unit::Points,
        };
        let labels: Vec<f64> = visible_ticks(geometry).into_iter().filter(|tick| tick.label.is_some()).map(|tick| tick.value).collect();
        assert!(labels.iter().any(|value| *value > 900.0), "point labels stopped at {labels:?}");
    }

    #[test]
    /// Handles a page partly before the ruler starts at the first visible tick.
    fn a_page_partly_before_the_ruler_starts_at_the_first_visible_tick() {
        let mut g = geometry(Unit::Points, 1.0);
        g.ruler_start = 0.0;
        g.ruler_end = 200.0;
        g.page_origin = -100.0;
        let ticks = visible_ticks(g);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| tick.screen_position >= g.ruler_start && tick.screen_position <= g.ruler_end));
        assert!(ticks.first().is_some_and(|tick| tick.value >= 100.0));
    }

    #[test]
    /// Handles negative page coordinates convert without changing the axis policy.
    fn negative_page_coordinates_convert_without_changing_the_axis_policy() {
        let g = geometry(Unit::Points, 2.0);
        assert_eq!(page_points_to_screen(g, -10.0), Some(180.0));
        assert_eq!(screen_to_page_points(g, 180.0), Some(-10.0));
    }

    #[test]
    /// Handles extreme scales do not generate dense or non finite ticks.
    fn extreme_scales_do_not_generate_dense_or_non_finite_ticks() {
        for scale in [1.0e-12, 1.0e12] {
            for unit in Unit::ALL {
                let ticks = visible_ticks(geometry(unit, scale));
                assert!(ticks.len() <= MAX_TICKS);
                assert!(ticks.iter().all(|tick| tick.screen_position.is_finite() && tick.value.is_finite()));
            }
        }
        assert!(choose_tick_spec(Unit::Points, 0.0).is_none());
        assert!(choose_tick_spec(Unit::Points, f64::NAN).is_none());
    }

    #[test]
    /// Verifies that ruler ticks continue when the page is outside the viewport.
    fn ruler_ticks_continue_when_the_page_is_outside_the_viewport() {
        let mut g = geometry(Unit::Points, 1.0);
        g.ruler_start = 0.0;
        g.ruler_end = 100.0;
        g.page_origin = 200.0;
        let ticks = visible_ticks(g);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|tick| tick.value < 0.0));
    }

    #[test]
    /// Handles zero length page still supplies a ruler origin.
    fn zero_length_page_still_supplies_a_ruler_origin() {
        let mut g = geometry(Unit::Centimetres, 1.0);
        g.page_length_pdf_points = 0.0;
        assert!(g.is_valid());
        assert!(!visible_ticks(g).is_empty());
    }

    #[test]
    /// Handles every unit generates bounded ticks at fractional scales.
    fn every_unit_generates_bounded_ticks_at_fractional_scales() {
        for unit in Unit::ALL {
            for scale in [0.75, 1.0, 1.25, 1.333_333, 1.5, 2.0, 4.0] {
                let ticks = visible_ticks(geometry(unit, scale));
                assert!(!ticks.is_empty(), "{unit:?} at {scale}");
                assert!(ticks.len() <= MAX_TICKS);
                assert!(ticks.iter().all(|tick| tick.screen_position.is_finite() && tick.value.is_finite()));
            }
        }
    }

    #[test]
    /// Handles hostile huge inputs are bounded or rejected.
    fn hostile_huge_inputs_are_bounded_or_rejected() {
        let mut g = geometry(Unit::Points, f64::MIN_POSITIVE);
        g.ruler_end = f64::MAX / 4.0;
        g.page_length_pdf_points = f64::MAX / 4.0;
        assert!(visible_ticks(g).len() <= MAX_TICKS);

        g.page_origin = 0.0;
        g.page_length_pdf_points = f64::MAX;
        g.screen_points_per_pdf_point = 2.0;
        assert!(!g.is_valid());
        assert!(visible_ticks(g).is_empty());
    }

    #[test]
    /// Handles inch labels use reduced binary fractions without decimals or mixed numbers.
    fn inch_labels_use_reduced_binary_fractions_without_decimals_or_mixed_numbers() {
        assert_eq!(format_inch_label(0.0), "0");
        assert_eq!(format_inch_label(1.0 / 64.0), "1/64");
        assert_eq!(format_inch_label(2.0 / 64.0), "1/32");
        assert_eq!(format_inch_label(8.0 / 64.0), "1/8");
        assert_eq!(format_inch_label(16.0 / 64.0), "1/4");
        assert_eq!(format_inch_label(32.0 / 64.0), "1/2");
        assert_eq!(format_inch_label(48.0 / 64.0), "3/4");
        assert_eq!(format_inch_label(1.0), "1");
        assert_eq!(format_inch_label(1.25), "1/4");
        assert_eq!(format_inch_label(-2.75), "3/4");
        assert!(!format_inch_label(5.0 / 16.0).contains('.'));
    }

    #[test]
    /// Handles non inch units label only whole numbers.
    fn non_inch_units_label_only_whole_numbers() {
        for unit in [Unit::Millimetres, Unit::Centimetres, Unit::Points] {
            let spec = TickSpec { minor_interval_units: 0.1, medium_every: 5, major_every: 10, label_every: 1 };
            assert_eq!(label_tick(unit, 1.5, 15, spec, 100.0), None);
            assert_eq!(label_tick(unit, 2.0, 20, spec, 100.0).as_deref(), Some("2"));
            assert_eq!(format_label(unit, 12.0), "12");
            assert!(!format_label(unit, 12.0).contains('.'));
        }
    }
    #[test]
    /// Handles labels do not show negative zero.
    fn labels_do_not_show_negative_zero() {
        assert_eq!(format_label(Unit::Inches, -0.0), "0");
        assert_eq!(format_label(Unit::Points, 72.0), "72");
    }

    /// Paints input.
    fn paint_input(orientation: RulerOrientation, rect: egui::Rect) -> RulerPaintInput {
        RulerPaintInput {
            orientation,
            rect,
            page_origin: match orientation {
                RulerOrientation::Horizontal => f64::from(rect.left()) + 20.0,
                RulerOrientation::Vertical => f64::from(rect.top()) + 20.0,
            },
            page_length_pdf_points: 612.0,
            screen_points_per_pdf_point: 1.0,
            unit: Unit::Points,
            pointer_screen_position: Some(match orientation {
                RulerOrientation::Horizontal => f64::from(rect.center().x),
                RulerOrientation::Vertical => f64::from(rect.center().y),
            }),
        }
    }

    #[test]
    /// Handles generated labels keep the minimum screen spacing.
    fn generated_labels_keep_the_minimum_screen_spacing() {
        for unit in Unit::ALL {
            for scale in [0.25, 0.75, 1.0, 1.333_333, 2.0, 4.0, 16.0] {
                let labels: Vec<f64> =
                    visible_ticks(geometry(unit, scale)).into_iter().filter(|tick| tick.label.is_some()).map(|tick| tick.screen_position).collect();
                let minimum = if unit == Unit::Inches {
                    let screen_points_per_unit = geometry(unit, scale).screen_points_per_unit().unwrap();
                    let spec = choose_tick_spec(unit, screen_points_per_unit).unwrap();
                    spec.minor_interval_units * f64::from(spec.label_every) * screen_points_per_unit
                } else {
                    MIN_LABEL_SPACING
                };
                assert!(labels.windows(2).all(|pair| pair[1] - pair[0] >= minimum - 0.01));
            }
        }
    }

    #[test]
    /// Handles every label interval has eight subdivisions with standard ruler hierarchy.
    fn every_label_interval_has_eight_subdivisions_with_standard_ruler_hierarchy() {
        let spec = eight_subdivision_spec(1.0);
        assert_eq!(spec.minor_interval_units, 1.0 / 8.0);
        assert_eq!(spec.label_every, 8);
        assert_eq!(tick_kind(1, spec), TickKind::Minor);
        assert_eq!(tick_kind(2, spec), TickKind::Quarter);
        assert_eq!(tick_kind(3, spec), TickKind::Minor);
        assert_eq!(tick_kind(4, spec), TickKind::Medium);
        assert_eq!(tick_kind(6, spec), TickKind::Quarter);
        assert_eq!(tick_kind(8, spec), TickKind::Major);
        assert_eq!(tick_kind(-2, spec), TickKind::Quarter);
        assert_eq!(tick_kind(-8, spec), TickKind::Major);
    }

    #[test]
    /// Handles unlabeled ticks keep their hierarchy.
    fn unlabeled_ticks_keep_their_hierarchy() {
        let spec = eight_subdivision_spec(1.0);
        assert_eq!(tick_kind(4, spec), TickKind::Medium);
        assert!(!divisible(4, spec.label_every));
        assert_eq!(tick_kind(8, spec), TickKind::Major);
        assert!(divisible(8, spec.label_every));
    }
    #[test]
    /// Handles tick hierarchy has strictly increasing lengths.
    fn tick_hierarchy_has_strictly_increasing_lengths() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        for orientation in [RulerOrientation::Horizontal, RulerOrientation::Vertical] {
            let minor = tick_length(rect, orientation, TickKind::Minor);
            let quarter = tick_length(rect, orientation, TickKind::Quarter);
            let medium = tick_length(rect, orientation, TickKind::Medium);
            let major = tick_length(rect, orientation, TickKind::Major);
            assert!(minor < quarter && quarter < medium && medium < major);
        }
    }

    #[test]
    /// Verifies that horizontal labels keep outer padding while major ticks span the band.
    fn horizontal_labels_keep_outer_padding_while_major_ticks_span_the_band() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        let label_top = rect.top() + HORIZONTAL_LABEL_OUTER_PADDING;
        assert!(label_top > rect.top());
        assert_eq!(tick_length(rect, RulerOrientation::Horizontal, TickKind::Major), 11.0);
    }

    #[test]
    /// Verifies that vertical labels are centred above major ticks without overlap.
    fn vertical_labels_are_centred_above_major_ticks_without_overlap() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, 300.0));
        let at = 100.0;
        assert!(vertical_label_fits(rect, at, egui::vec2(8.0, RULER_LABEL_SIZE)));
        assert_eq!(rect.center().x, VERTICAL_RULER_WIDTH * 0.5);
    }

    #[test]
    /// Verifies that horizontal labels begin after their major tick.
    fn horizontal_labels_begin_after_their_major_tick() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        assert!(horizontal_label_fits(rect, 72.0, "1"));
        assert!(!horizontal_label_fits(rect, rect.right() - 2.0, "8"));
    }

    #[test]
    /// Handles inch ticks use binary fractions and reference label cadence.
    fn inch_ticks_use_binary_fractions_and_reference_label_cadence() {
        let at_100 = choose_tick_spec(Unit::Inches, 96.0).unwrap();
        assert_eq!(at_100.minor_interval_units, 1.0 / 8.0);
        assert_eq!((at_100.medium_every, at_100.major_every, at_100.label_every), (4, 8, 8));

        let zoomed_out = choose_tick_spec(Unit::Inches, 12.0).unwrap();
        assert_eq!(zoomed_out.minor_interval_units, 1.0);
        assert_eq!(zoomed_out.label_every, 8);
    }

    #[test]
    /// Handles inch labels show magnitudes on both sides of zero.
    fn inch_labels_show_magnitudes_on_both_sides_of_zero() {
        assert_eq!(format_label(Unit::Inches, -8.0), "8");
        assert_eq!(format_label(Unit::Inches, 8.0), "8");
        assert_eq!(format_label(Unit::Inches, -0.0), "0");
    }

    #[test]
    /// Handles edge labels are suppressed instead of clipped.
    fn edge_labels_are_suppressed_instead_of_clipped() {
        let horizontal = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, HORIZONTAL_RULER_HEIGHT));
        assert!(horizontal_label_fits(horizontal, horizontal.left(), "32"));
        assert!(horizontal_label_fits(horizontal, horizontal.center().x, "32"));
        assert!(!horizontal_label_fits(horizontal, horizontal.right(), "32"));

        let vertical = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, 100.0));
        assert!(!vertical_label_fits(vertical, vertical.top(), egui::vec2(8.0, RULER_LABEL_SIZE)));
        assert!(!vertical_label_fits(vertical, vertical.bottom() - 4.0, egui::vec2(8.0, RULER_LABEL_SIZE)));
        assert!(vertical_label_fits(vertical, vertical.center().y, egui::vec2(8.0, RULER_LABEL_SIZE)));
    }

    #[test]
    /// Verifies that pointer markers are full band lines.
    fn pointer_markers_are_full_band_lines() {
        let horizontal = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, HORIZONTAL_RULER_HEIGHT));
        let h = pointer_marker_line(RulerOrientation::Horizontal, horizontal, 100.0);
        assert_eq!(h, [egui::pos2(100.0, horizontal.top()), egui::pos2(100.0, horizontal.bottom())]);

        let vertical = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, 300.0));
        let v = pointer_marker_line(RulerOrientation::Vertical, vertical, 100.0);
        assert_eq!(v, [egui::pos2(vertical.left(), 100.0), egui::pos2(vertical.right(), 100.0)]);
    }

    #[test]
    /// Handles both themes paint two rulers and the corner in an isolated harness.
    fn both_themes_paint_two_rulers_and_the_corner_in_an_isolated_harness() {
        for kind in [crate::theme::ThemeKind::Light, crate::theme::ThemeKind::Dark] {
            let ctx = egui::Context::default();
            ctx.set_fonts(crate::theme::font_definitions());
            crate::theme::apply(&ctx, kind);
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let corner = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VERTICAL_RULER_WIDTH, HORIZONTAL_RULER_HEIGHT));
                let horizontal = egui::Rect::from_min_size(egui::pos2(VERTICAL_RULER_WIDTH, 0.0), egui::vec2(400.0, HORIZONTAL_RULER_HEIGHT));
                let vertical = egui::Rect::from_min_size(egui::pos2(0.0, HORIZONTAL_RULER_HEIGHT), egui::vec2(VERTICAL_RULER_WIDTH, 400.0));
                paint_corner(ui, corner, Unit::Points);
                paint_ruler(ui, paint_input(RulerOrientation::Horizontal, horizontal));
                paint_ruler(ui, paint_input(RulerOrientation::Vertical, vertical));
            });
            assert!(output.shapes.len() > 20, "{kind:?} ruler harness produced too few shapes");
            output.drop_without_applying_deltas();
        }
    }
}
