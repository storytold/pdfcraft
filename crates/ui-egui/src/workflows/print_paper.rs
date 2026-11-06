//! The Print dialog's paper list and the paper each printer is remembered with (print.page-setup):
//! paper sizes of a person's own, added with Custom… beside Paper and kept with the settings, and
//! for each printer the paper chosen when "Remember this paper for …" was ticked at Print time.
//! Two-sided, grayscale, collate, sizing and comments & forms are left to the dialog itself.

use std::collections::BTreeMap;

use pdfcraft_engine::print::PAPERS;
use serde::{Deserialize, Serialize};

use crate::print_ui::PrintDraft;

/// The largest side a custom paper can have, in points: PDF's own page limit (200 inches).
pub const MAX_SIDE: f64 = 14_400.0;
/// The smallest side, in points (one inch).
pub const MIN_SIDE: f64 = 72.0;
/// How many custom papers the settings keep.
pub const MAX_CUSTOM: usize = 100;
/// The longest name a custom paper keeps, in characters.
const MAX_NAME: usize = 64;

/// A paper size added in the Print dialog (points, portrait).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomPaper {
    pub name: String,
    pub width: f64,
    pub height: f64,
}

impl CustomPaper {
    /// A custom paper from a name and two sides in points, portrait; None when the name is empty
    /// or a side is outside [`MIN_SIDE`]..=[`MAX_SIDE`], or a standard paper already has the name
    /// (any case), which would otherwise be found first when the paper is looked up by name.
    pub fn new(name: &str, a: f64, b: f64) -> Option<Self> {
        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
        let fits = |v: f64| v.is_finite() && (MIN_SIDE..=MAX_SIDE).contains(&v);
        let standard = PAPERS.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name));
        (!name.is_empty() && !standard && fits(a) && fits(b)).then(|| Self { name, width: a.min(b), height: a.max(b) })
    }
}

/// The paper remembered for each printer, by the printer's name ("" is Save as PDF).
pub type RememberedPaper = BTreeMap<String, String>;

/// Custom papers read back from the settings, which are untrusted: each one is checked as if it
/// were typed, a name already taken keeps its first size, and at most [`MAX_CUSTOM`] are kept.
pub fn read_custom(value: &serde_json::Value) -> Vec<CustomPaper> {
    let Some(list) = value.as_array() else { return Vec::new() };
    let mut out: Vec<CustomPaper> = Vec::new();
    for item in list {
        let (Some(name), Some(w), Some(h)) = (item["name"].as_str(), item["width"].as_f64(), item["height"].as_f64()) else { continue };
        if let Some(paper) = CustomPaper::new(name, w, h)
            && !out.iter().any(|p| p.name == paper.name)
            && out.len() < MAX_CUSTOM
        {
            out.push(paper);
        }
    }
    out
}

/// The remembered papers read back from the settings: printer name to paper name, strings only.
pub fn read_remembered(value: &serde_json::Value) -> RememberedPaper {
    let Some(map) = value.as_object() else { return RememberedPaper::new() };
    map.iter().filter_map(|(printer, paper)| paper.as_str().map(|p| (printer.clone(), p.to_string()))).collect()
}

impl PrintDraft {
    /// The paper list: the standard sizes, then the custom ones.
    pub fn paper_count(&self) -> usize {
        PAPERS.len() + self.custom_papers.len()
    }

    /// The chosen paper's size (points, portrait).
    pub fn paper_size(&self) -> (f64, f64) {
        match self.paper.checked_sub(PAPERS.len()) {
            None => PAPERS[self.paper.min(PAPERS.len() - 1)].1,
            Some(c) => self.custom_papers.get(c).map_or(PAPERS[0].1, |p| (p.width, p.height)),
        }
    }

    /// The chosen paper's name (a standard name, or the custom paper's).
    pub fn paper_name(&self) -> String {
        match self.paper.checked_sub(PAPERS.len()) {
            None => PAPERS[self.paper.min(PAPERS.len() - 1)].0.to_string(),
            Some(c) => self.custom_papers.get(c).map_or_else(|| PAPERS[0].0.to_string(), |p| p.name.clone()),
        }
    }

    /// The index of the paper called `name`, standard or custom.
    pub fn paper_index(&self, name: &str) -> Option<usize> {
        PAPERS.iter().position(|(n, _)| *n == name).or_else(|| self.custom_papers.iter().position(|p| p.name == name).map(|i| PAPERS.len() + i))
    }

    /// True when the chosen paper is a custom one (which can be removed).
    pub fn paper_is_custom(&self) -> bool {
        self.paper >= PAPERS.len()
    }

    /// Add (or replace, by name) a custom paper and choose it.
    pub fn add_custom_paper(&mut self, paper: CustomPaper) {
        self.custom_papers.retain(|p| p.name != paper.name);
        if self.custom_papers.len() >= MAX_CUSTOM {
            self.custom_papers.remove(0);
        }
        self.custom_papers.push(paper);
        self.paper = PAPERS.len() + self.custom_papers.len() - 1;
    }

    /// Take the chosen custom paper off the list; the dialog goes back to the first size.
    pub fn remove_custom_paper(&mut self) {
        if let Some(i) = self.paper.checked_sub(PAPERS.len())
            && i < self.custom_papers.len()
        {
            self.custom_papers.remove(i);
            self.paper = 0;
        }
    }

    /// The key the printer's paper is remembered under ("" is Save as PDF).
    fn printer_key(&self) -> String {
        self.printer.clone().unwrap_or_default()
    }

    /// Start from the paper remembered for the printer chosen, if one was and it is still on the
    /// list; the Remember box says whether one was.
    pub fn apply_remembered(&mut self) {
        let remembered = self.remembered_paper.get(&self.printer_key()).cloned();
        self.remember_paper = remembered.is_some();
        if let Some(i) = remembered.and_then(|name| self.paper_index(&name)) {
            self.paper = i;
        }
    }

    /// At Print time: keep the paper for the printer when the box is ticked, forget it when not.
    pub fn remember_now(&mut self) {
        let key = self.printer_key();
        if self.remember_paper {
            let name = self.paper_name();
            self.remembered_paper.insert(key, name);
        } else {
            self.remembered_paper.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_paper_is_portrait_named_and_within_pdfs_limits() {
        let p = CustomPaper::new("  Legal, tray 2 ", 1008.0, 612.0).unwrap();
        assert_eq!((p.name.as_str(), p.width, p.height), ("Legal, tray 2", 612.0, 1008.0));
        assert!(CustomPaper::new("", 612.0, 792.0).is_none());
        assert!(CustomPaper::new("\u{1}", 612.0, 792.0).is_none());
        assert!(CustomPaper::new("Tiny", 10.0, 792.0).is_none());
        assert!(CustomPaper::new("Huge", 612.0, 20_000.0).is_none());
        assert!(CustomPaper::new("NaN", f64::NAN, 792.0).is_none());
        assert_eq!(CustomPaper::new(&"x".repeat(500), 612.0, 792.0).unwrap().name.chars().count(), MAX_NAME);
    }

    #[test]
    fn settings_are_read_as_untrusted() {
        let v = serde_json::json!([
            { "name": "Label", "width": 288.0, "height": 432.0 },
            { "name": "Label", "width": 300.0, "height": 400.0 },
            { "name": "Bad", "width": -1.0, "height": 432.0 },
            { "name": 5, "width": 288.0, "height": 432.0 },
            "nonsense",
        ]);
        assert_eq!(read_custom(&v), vec![CustomPaper { name: "Label".into(), width: 288.0, height: 432.0 }]);
        assert!(read_custom(&serde_json::json!(5)).is_empty());
        let many: Vec<_> = (0..300).map(|i| serde_json::json!({ "name": format!("P{i}"), "width": 300.0, "height": 400.0 })).collect();
        assert_eq!(read_custom(&serde_json::Value::Array(many)).len(), MAX_CUSTOM);
        let r = read_remembered(&serde_json::json!({ "Office": "A4", "": "Label", "Odd": 3 }));
        assert_eq!(r.len(), 2);
        assert_eq!(r.get("").map(String::as_str), Some("Label"));
        assert!(read_remembered(&serde_json::json!([1])).is_empty());
    }

    #[test]
    fn the_paper_list_is_the_standard_sizes_then_the_custom_ones() {
        let mut d = PrintDraft::default();
        assert_eq!(d.paper_count(), PAPERS.len());
        d.add_custom_paper(CustomPaper::new("Label", 288.0, 432.0).unwrap());
        assert_eq!((d.paper, d.paper_name().as_str(), d.paper_size()), (PAPERS.len(), "Label", (288.0, 432.0)));
        assert_eq!(d.paper_index("A4"), PAPERS.iter().position(|(n, _)| *n == "A4"));
        assert_eq!(d.paper_index("Label"), Some(PAPERS.len()));
        // The same name replaces the size; removing goes back to the first standard size.
        d.add_custom_paper(CustomPaper::new("Label", 300.0, 400.0).unwrap());
        assert_eq!((d.paper_count(), d.paper_size()), (PAPERS.len() + 1, (300.0, 400.0)));
        d.remove_custom_paper();
        assert_eq!((d.paper_count(), d.paper), (PAPERS.len(), 0));
        // A stale index reads as the first size, never out of bounds.
        d.paper = PAPERS.len() + 7;
        assert_eq!(d.paper_size(), PAPERS[0].1);
    }

    #[test]
    fn the_paper_is_remembered_per_printer_only_when_asked() {
        let mut d = PrintDraft::default();
        let a4 = d.paper_index("A4").unwrap();
        d.printer = Some("Office".into());
        d.paper = a4;
        d.remember_paper = true;
        d.remember_now();
        d.printer = Some("Plotter".into());
        d.paper = 0;
        d.apply_remembered();
        assert_eq!((d.paper, d.remember_paper), (0, false), "another printer keeps the dialog's paper");
        d.printer = Some("Office".into());
        d.apply_remembered();
        assert_eq!((d.paper, d.remember_paper), (a4, true));
        d.remember_paper = false;
        d.remember_now();
        assert!(d.remembered_paper.is_empty());
        // A remembered custom paper that was removed since leaves the paper alone.
        d.remembered_paper.insert("Office".into(), "Gone".into());
        d.paper = 0;
        d.apply_remembered();
        assert_eq!((d.paper, d.remember_paper), (0, true));
    }
}
