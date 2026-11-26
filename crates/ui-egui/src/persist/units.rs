//! The unit lengths are shown in: page sizes (Document Properties, Print) and the margins typed
//! in Header & Footer and Set Page Boxes. Preferences ▸ Documents and view ▸ Units.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Inches,
    Millimetres,
    Centimetres,
    Points,
}

impl Unit {
    pub const ALL: [Unit; 4] = [Unit::Millimetres, Unit::Centimetres, Unit::Inches, Unit::Points];

    /// Units per PDF point.
    pub fn per_point(self) -> f64 {
        match self {
            Unit::Points => 1.0,
            Unit::Inches => 1.0 / 72.0,
            Unit::Millimetres => 25.4 / 72.0,
            Unit::Centimetres => 2.54 / 72.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Unit::Points => "pt",
            Unit::Inches => "in",
            Unit::Millimetres => "mm",
            Unit::Centimetres => "cm",
        }
    }

    /// Decimals worth showing: a tenth of a millimetre or point, a hundredth of an inch or centimetre.
    pub fn decimals(self) -> usize {
        match self {
            Unit::Millimetres | Unit::Points => 1,
            Unit::Inches | Unit::Centimetres => 2,
        }
    }

    /// Drag speed for a value field in this unit.
    pub fn speed(self) -> f64 {
        match self {
            Unit::Inches | Unit::Centimetres => 0.01,
            Unit::Millimetres | Unit::Points => 0.5,
        }
    }

    /// The unit the system's region measures in: inches in the United States, Liberia and
    /// Myanmar, millimetres everywhere else; inches when the region can't be told. Used on first
    /// start, before the user picks one in Preferences.
    pub fn system() -> Unit {
        match craft::i18n::Region::system().measurement {
            craft::i18n::Measurement::Us => Unit::Inches,
            craft::i18n::Measurement::Metric => Unit::Millimetres,
        }
    }

    /// A page size given in points, e.g. "210.0 × 297.0 mm".
    pub fn size(self, width_pt: f64, height_pt: f64) -> String {
        let (k, d) = (self.per_point(), self.decimals());
        format!("{:.d$} × {:.d$} {}", width_pt * k, height_pt * k, self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::Unit;

    /// One page size printed in every unit is the same size in points: a wrong factor mis-sizes
    /// every length.
    #[test]
    fn a_page_size_is_the_same_length_in_every_unit() {
        let (width, height) = (595.276, 841.89);
        for unit in Unit::ALL {
            let text = unit.size(width, height);
            let parts: Vec<&str> = text.split_whitespace().collect();
            assert_eq!(parts.len(), 4, "{text}");
            assert_eq!(parts[3], unit.label());
            // Printing rounds to `decimals`, so the points read back are as close as that allows.
            let tolerance = 0.5 * 10f64.powi(-i32::try_from(unit.decimals()).unwrap()) / unit.per_point() + 1e-9;
            for (shown, points) in [(parts[0], width), (parts[2], height)] {
                let back = shown.parse::<f64>().unwrap() / unit.per_point();
                assert!((back - points).abs() <= tolerance, "{text}: {back} pt is not {points} pt");
            }
        }
    }

    #[test]
    fn saved_names_round_trip() {
        for u in Unit::ALL {
            let v = serde_json::to_value(u).unwrap();
            assert_eq!(serde_json::from_value::<Unit>(v).unwrap(), u);
        }
        assert_eq!(serde_json::to_value(Unit::Millimetres).unwrap(), "millimetres");
    }
}
