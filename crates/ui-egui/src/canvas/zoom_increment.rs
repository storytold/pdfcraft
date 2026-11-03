//! How far Zoom In and Zoom Out go (#5884): Acrobat's standard levels by default, or a fixed
//! number of percentage points for finer steps. Only the document view; the page grid sizes its
//! pages on its own scale.

/// The user's choice in Preferences ▸ Documents and view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ZoomIncrement {
    /// The standard levels (`DocView::zoom_step`).
    #[default]
    #[serde(rename = "standard")]
    Standard,
    #[serde(rename = "5")]
    Five,
    #[serde(rename = "10")]
    Ten,
    #[serde(rename = "25")]
    TwentyFive,
}

/// The smallest and largest zooms, in percent, as `DocView::zoom_at` clamps them.
const MIN_PERCENT: f32 = 8.0;
const MAX_PERCENT: f32 = 6400.0;

impl ZoomIncrement {
    pub const ALL: [Self; 4] = [Self::Standard, Self::Five, Self::Ten, Self::TwentyFive];

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard levels",
            Self::Five => "5%",
            Self::Ten => "10%",
            Self::TwentyFive => "25%",
        }
    }

    /// The setting as `ui.set` takes it: `standard`, or `5`, `10` or `25` (a `%` may follow).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().trim_end_matches('%').trim_end() {
            v if v.eq_ignore_ascii_case("standard") => Some(Self::Standard),
            "5" => Some(Self::Five),
            "10" => Some(Self::Ten),
            "25" => Some(Self::TwentyFive),
            _ => None,
        }
    }

    /// Percentage points per step; `None` for the standard levels.
    pub fn points(self) -> Option<f32> {
        match self {
            Self::Standard => None,
            Self::Five => Some(5.0),
            Self::Ten => Some(10.0),
            Self::TwentyFive => Some(25.0),
        }
    }

    /// The zoom one step in or out from `zoom` (a factor, 1.0 = 100%): the next multiple of the
    /// increment, so a zoom between two (a fitted 87%) lands on one (90% in, 80% out with 10).
    /// Out below the first multiple goes to the smallest zoom. `None` for the standard levels,
    /// or at the end of the range.
    pub fn step(self, zoom: f32, up: bool) -> Option<f32> {
        let points = self.points()?;
        let percent = zoom * 100.0;
        if !percent.is_finite() {
            return None;
        }
        // A zoom already on a multiple (110% stored as 109.99999) counts as on it.
        let steps = percent / points;
        let next = if up {
            ((steps + 1e-3).floor() + 1.0) * points
        } else {
            let below = ((steps - 1e-3).ceil() - 1.0) * points;
            if below < points { MIN_PERCENT } else { below }
        }
        .clamp(MIN_PERCENT, MAX_PERCENT);
        let moved = if up { next > percent + 1e-3 } else { next < percent - 1e-3 };
        moved.then_some(next / 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::ZoomIncrement;

    /// The zooms, in whole percent, that `n` steps from `start` percent go through.
    fn walk(increment: ZoomIncrement, start: f32, up: bool, n: usize) -> Vec<f32> {
        let mut zoom = start / 100.0;
        let mut out = Vec::new();
        for _ in 0..n {
            match increment.step(zoom, up) {
                Some(z) => zoom = z,
                None => break,
            }
            out.push((zoom * 100.0).round());
        }
        out
    }

    #[test]
    fn fixed_increments_step_by_their_points_and_land_on_multiples() {
        assert_eq!(walk(ZoomIncrement::Ten, 100.0, true, 3), [110.0, 120.0, 130.0]);
        assert_eq!(walk(ZoomIncrement::Ten, 100.0, false, 3), [90.0, 80.0, 70.0]);
        assert_eq!(walk(ZoomIncrement::Five, 100.0, true, 2), [105.0, 110.0]);
        assert_eq!(walk(ZoomIncrement::TwentyFive, 100.0, false, 2), [75.0, 50.0]);
        // A fitted zoom between two multiples lands on the next one each way.
        assert_eq!(walk(ZoomIncrement::Ten, 87.3, true, 1), [90.0]);
        assert_eq!(walk(ZoomIncrement::Ten, 87.3, false, 1), [80.0]);
        // In and out again come back to where they started.
        let there = ZoomIncrement::Ten.step(1.1, true).unwrap();
        assert!((ZoomIncrement::Ten.step(there, false).unwrap() - 1.1).abs() < 1e-6);
    }

    #[test]
    fn fixed_increments_stop_at_the_ends_of_the_zoom_range() {
        // Below the first multiple, out goes to the smallest zoom, then no further.
        assert_eq!(walk(ZoomIncrement::Ten, 20.0, false, 4), [10.0, 8.0]);
        assert_eq!(walk(ZoomIncrement::TwentyFive, 8.0, true, 1), [25.0]);
        assert_eq!(walk(ZoomIncrement::TwentyFive, 6350.0, true, 3), [6375.0, 6400.0]);
        assert_eq!(ZoomIncrement::Ten.step(64.0, true), None, "6400% is the largest");
        assert_eq!(ZoomIncrement::Ten.step(f32::NAN, true), None);
    }

    #[test]
    fn standard_levels_have_no_fixed_step_and_settings_parse() {
        assert_eq!(ZoomIncrement::default(), ZoomIncrement::Standard);
        assert_eq!(ZoomIncrement::Standard.step(1.0, true), None);
        for (typed, expected) in
            [("standard", ZoomIncrement::Standard), (" 5 ", ZoomIncrement::Five), ("10%", ZoomIncrement::Ten), ("25 %", ZoomIncrement::TwentyFive)]
        {
            assert_eq!(ZoomIncrement::parse(typed), Some(expected), "{typed}");
        }
        for typed in ["", "7", "-10", "fast"] {
            assert_eq!(ZoomIncrement::parse(typed), None, "{typed}");
        }
        for increment in ZoomIncrement::ALL {
            let json = serde_json::to_value(increment).unwrap();
            assert_eq!(serde_json::from_value::<ZoomIncrement>(json).unwrap(), increment);
        }
    }
}
