//! The Print dialog's paper list (print.page-setup): the standard sizes, then paper sizes of a
//! person's own, added with Custom… beside Paper and kept with the settings. Which paper each
//! printer prints on is remembered with the rest of its setup (`print_ui::setups`, #2001), by name,
//! so a custom paper is found again however the list changes.

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

/// True when `name` can name a paper: a standard size's own name, or one a custom paper could
/// have been given ([`CustomPaper::new`] keeps it as it is).
pub fn is_paper_name(name: &str) -> bool {
    PAPERS.iter().any(|(n, _)| *n == name) || CustomPaper::new(name, MIN_SIDE, MIN_SIDE).is_some_and(|p| p.name == name)
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
        self.paper_name_at(self.paper)
    }

    /// The name of the paper at `index` in the list; the first size's for a stale index.
    pub fn paper_name_at(&self, index: usize) -> String {
        match index.checked_sub(PAPERS.len()) {
            None => PAPERS[index.min(PAPERS.len() - 1)].0.to_string(),
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
        // A paper name read back is a standard name, or one a custom paper could have.
        assert!(is_paper_name("A4") && is_paper_name("Label 4×6"));
        assert!(!is_paper_name("a4") && !is_paper_name("") && !is_paper_name(" Label") && !is_paper_name(&"x".repeat(MAX_NAME + 1)));
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
}
