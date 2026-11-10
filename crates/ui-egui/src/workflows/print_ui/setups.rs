//! What each printer was last printed with (#2001): its paper, page sizing and two-sided choice,
//! kept across restarts, so a label printer keeps 4 × 6 in at Actual size while an office
//! printer keeps A4 at Fit. The printer printed to last is preselected the next time Print opens.
//!
//! Copies, the pages to print, orientation and the Multiple / Booklet / Poster layouts stay one
//! job's choices, as before.

use super::{CUSTOM_SCALE, SizeMode, print::names, spool::Duplex};
use crate::print_paper::is_paper_name;

/// At most this many printers are remembered, the most recently printed to first (settings are
/// untrusted, and a long-gone queue shouldn't stay forever).
pub const MAX_PRINTERS: usize = 32;
/// A longer printer name in the settings file is refused (CUPS and Windows queue names are far
/// shorter).
const MAX_NAME_BYTES: usize = 1024;

/// The paper, sizing and two-sided choice a printer was last printed with.
#[derive(Clone, Debug, PartialEq)]
pub struct PrinterSetup {
    /// The paper's name: a standard size's canonical name, or a custom paper's (print.page-setup).
    pub paper: String,
    pub size: SizeMode,
    /// The percent `SizeMode::Custom` prints at.
    pub custom_scale: f64,
    /// `None` = the printer's own default.
    pub duplex: Option<Duplex>,
}

/// Put `setup` first for `printer`, replacing what it had, and drop the oldest past
/// [`MAX_PRINTERS`].
pub fn record(setups: &mut Vec<(String, PrinterSetup)>, printer: &str, setup: PrinterSetup) {
    setups.retain(|(name, _)| name != printer);
    setups.insert(0, (printer.to_string(), setup));
    setups.truncate(MAX_PRINTERS);
}

/// The setups for the settings file: the paper by its name (as `print_paper`), sizing and
/// two-sided by the names `doc_print` and the `print` setting use ([`names`]).
pub fn to_json(setups: &[(String, PrinterSetup)]) -> serde_json::Value {
    setups
        .iter()
        .map(|(printer, s)| {
            serde_json::json!({
                "printer": printer,
                "paper": s.paper,
                "size": names::size_mode(s.size),
                "custom_scale": s.custom_scale,
                "duplex": names::duplex(s.duplex),
            })
        })
        .collect()
}

/// The setups read back from the settings file; `None` when it holds no list, so the caller
/// keeps what it has. An entry with an unknown or malformed value is skipped whole (a half-read
/// setup would print the label at the wrong size), as is a repeated printer.
pub fn from_json(v: &serde_json::Value) -> Option<Vec<(String, PrinterSetup)>> {
    let mut out: Vec<(String, PrinterSetup)> = Vec::new();
    for entry in v.as_array()?.iter() {
        if out.len() >= MAX_PRINTERS {
            break;
        }
        let Some(read) = read_entry(entry) else { continue };
        if out.iter().all(|(name, _)| *name != read.0) {
            out.push(read);
        }
    }
    Some(out)
}

fn read_entry(entry: &serde_json::Value) -> Option<(String, PrinterSetup)> {
    let printer = entry.get("printer")?.as_str().filter(|p| !p.is_empty() && p.len() <= MAX_NAME_BYTES)?;
    // A custom paper's name is kept even when the paper isn't on this list: it may be added again.
    let paper = entry.get("paper")?.as_str().filter(|name| is_paper_name(name))?.to_string();
    let custom_scale = entry.get("custom_scale")?.as_f64().filter(|s| CUSTOM_SCALE.contains(s))?;
    let size = names::parse_size_mode(entry.get("size")?.as_str()?, custom_scale)?;
    let duplex = names::parse_duplex(entry.get("duplex")?.as_str()?)?;
    Some((printer.to_string(), PrinterSetup { paper, size, custom_scale, duplex }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(paper: usize, size: SizeMode, duplex: Option<Duplex>) -> PrinterSetup {
        PrinterSetup { paper: super::super::PAPERS[paper].0.to_string(), size, custom_scale: 62.5, duplex }
    }

    #[test]
    fn setups_round_trip_through_the_settings_file() {
        let mut setups = Vec::new();
        record(&mut setups, "Office", setup(4, SizeMode::Fit, Some(Duplex::LongEdge)));
        record(&mut setups, "Label", setup(super::super::print::LABEL_PAPER, SizeMode::Actual, Some(Duplex::Off)));
        record(&mut setups, "Plotter", setup(0, SizeMode::Custom(62.5), None));
        record(&mut setups, "Shrinker", setup(1, SizeMode::Shrink, Some(Duplex::ShortEdge)));
        // A custom paper goes by the name it was given (print.page-setup).
        record(&mut setups, "Labeller", PrinterSetup { paper: "Label 4×6".into(), ..setup(0, SizeMode::Actual, None) });
        assert_eq!(from_json(&to_json(&setups)), Some(setups));
    }

    #[test]
    fn printing_again_moves_a_printer_first_and_the_oldest_drops_off() {
        let mut setups = Vec::new();
        for i in 0..MAX_PRINTERS + 3 {
            record(&mut setups, &format!("Queue {i}"), setup(0, SizeMode::Fit, None));
        }
        assert_eq!(setups.len(), MAX_PRINTERS);
        assert_eq!(setups[0].0, format!("Queue {}", MAX_PRINTERS + 2));
        assert!(setups.iter().all(|(name, _)| name != "Queue 0"));
        record(&mut setups, "Queue 10", setup(4, SizeMode::Actual, None));
        assert_eq!(setups.len(), MAX_PRINTERS);
        assert_eq!(setups[0], ("Queue 10".to_string(), setup(4, SizeMode::Actual, None)));
        assert_eq!(setups.iter().filter(|(name, _)| name == "Queue 10").count(), 1);
    }

    #[test]
    fn malformed_entries_are_skipped_whole() {
        let good = serde_json::json!({"printer": "Label", "paper": "A4", "size": "actual", "custom_scale": 100.0, "duplex": "off"});
        let mut bad = Vec::new();
        for (key, value) in [
            ("printer", serde_json::json!("")),
            ("printer", serde_json::json!(5)),
            ("printer", serde_json::json!("x".repeat(MAX_NAME_BYTES + 1))),
            ("paper", serde_json::json!("a4")),
            ("paper", serde_json::json!("")),
            ("paper", serde_json::json!("x".repeat(65))),
            ("paper", serde_json::json!(4)),
            ("size", serde_json::json!("fit-to-page")),
            ("size", serde_json::Value::Null),
            ("custom_scale", serde_json::json!(0.5)),
            ("custom_scale", serde_json::json!(1e9)),
            ("custom_scale", serde_json::json!("100")),
            ("duplex", serde_json::json!("Long-Edge")),
            ("duplex", serde_json::json!(true)),
        ] {
            let mut entry = good.clone();
            entry[key] = value;
            bad.push(entry);
        }
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove("duplex");
        bad.push(missing);
        bad.push(serde_json::json!("Label"));
        bad.push(serde_json::Value::Null);
        for entry in &bad {
            assert_eq!(from_json(&serde_json::json!([entry])), Some(Vec::new()), "{entry}");
        }
        // The good entry still reads among bad ones, once.
        let mut list = bad.clone();
        list.push(good.clone());
        list.push(serde_json::json!({"printer": "Label", "paper": "A3", "size": "fit", "custom_scale": 100.0, "duplex": "printer"}));
        let read = from_json(&serde_json::Value::Array(list)).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].0, "Label");
        assert_eq!(read[0].1.size, SizeMode::Actual);
        // Not a list at all: the caller keeps what it has.
        for v in [serde_json::Value::Null, serde_json::json!({}), serde_json::json!("Label"), serde_json::json!(3)] {
            assert_eq!(from_json(&v), None, "{v}");
        }
        // A file listing more printers than are kept reads the first ones.
        let many: Vec<_> = (0..MAX_PRINTERS * 2)
            .map(|i| serde_json::json!({"printer": format!("Q{i}"), "paper": "A4", "size": "fit", "custom_scale": 100.0, "duplex": "printer"}))
            .collect();
        assert_eq!(from_json(&serde_json::Value::Array(many)).unwrap().len(), MAX_PRINTERS);
    }
}
