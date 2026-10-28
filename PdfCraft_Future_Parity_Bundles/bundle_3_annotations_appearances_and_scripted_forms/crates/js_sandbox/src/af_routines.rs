//! Standard Acrobat Form Calculation & Validation Built-ins (AF* Routines).

use std::collections::HashMap;

pub struct AfRoutines;

impl AfRoutines {
    /// Implements `AFSimple_Calculate(cFunction, cFields)` per Adobe Acrobat JS Reference.
    /// Operations: "SUM", "PRD" (Product), "AVG", "MIN", "MAX".
    pub fn simple_calculate(op: &str, field_names: &[String], field_values: &HashMap<String, String>) -> f64 {
        let values: Vec<f64> = field_names.iter()
            .filter_map(|name| field_values.get(name))
            .filter_map(|val| val.trim().parse::<f64>().ok())
            .collect();

        if values.is_empty() {
            return 0.0;
        }

        match op.to_uppercase().as_str() {
            "SUM" => values.iter().sum(),
            "PRD" => values.iter().product(),
            "AVG" => values.iter().sum::<f64>() / values.len() as f64,
            "MIN" => values.iter().copied().fold(f64::INFINITY, f64::min),
            "MAX" => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            _ => 0.0,
        }
    }

    /// Implements `AFNumber_Format(nDec, sepStyle, negStyle, currStyle, strCurrency, bCurrencyPrepend)`.
    pub fn number_format(value: f64, n_dec: usize, currency: Option<&str>) -> String {
        let formatted = format!("{:.prec$}", value, prec = n_dec);
        if let Some(curr) = currency {
            format!("{}{}", curr, formatted)
        } else {
            formatted
        }
    }

    /// Implements `AFRange_Validate(bGreaterThan, nGreaterThan, bLessThan, nLessThan)`.
    pub fn range_validate(val: f64, min: Option<f64>, max: Option<f64>) -> bool {
        if let Some(lower) = min {
            if val < lower {
                return false;
            }
        }
        if let Some(upper) = max {
            if val > upper {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_af_simple_calculate() {
        let mut fields = HashMap::new();
        fields.insert("Subtotal".into(), "100.50".into());
        fields.insert("Tax".into(), "8.25".into());
        fields.insert("Shipping".into(), "15.00".into());

        let sum = AfRoutines::simple_calculate(
            "SUM",
            &["Subtotal".into(), "Tax".into(), "Shipping".into()],
            &fields,
        );
        assert!((sum - 123.75).abs() < 1e-4);

        let avg = AfRoutines::simple_calculate(
            "AVG",
            &["Subtotal".into(), "Tax".into()],
            &fields,
        );
        assert!((avg - 54.375).abs() < 1e-4);
    }
}
