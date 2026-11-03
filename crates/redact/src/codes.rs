//! Redaction code sets (Acrobat's Redaction Properties ▸ Redaction Code): the exemption codes
//! that justify a redaction, used as its overlay text. The built-in sets are the exemptions of
//! the U.S. Freedom of Information Act (5 U.S.C. 552(b)) and the U.S. Privacy Act
//! (5 U.S.C. 552a(d), (j) and (k)).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeSet {
    pub id: &'static str,
    pub name: &'static str,
    pub codes: &'static [&'static str],
}

pub const CODE_SETS: [CodeSet; 2] = [
    CodeSet {
        id: "foia",
        name: "U.S. FOIA",
        codes: &[
            "(b)(1)(A)",
            "(b)(1)(B)",
            "(b)(2)",
            "(b)(3)(A)",
            "(b)(3)(B)",
            "(b)(4)",
            "(b)(5)",
            "(b)(6)",
            "(b)(7)(A)",
            "(b)(7)(B)",
            "(b)(7)(C)",
            "(b)(7)(D)",
            "(b)(7)(E)",
            "(b)(7)(F)",
            "(b)(8)",
            "(b)(9)",
        ],
    },
    CodeSet {
        id: "privacy-act",
        name: "U.S. Privacy Act",
        codes: &["(d)(5)", "(j)(1)", "(j)(2)", "(k)(1)", "(k)(2)", "(k)(3)", "(k)(4)", "(k)(5)", "(k)(6)", "(k)(7)"],
    },
];

impl CodeSet {
    pub fn from_id(id: &str) -> Option<Self> {
        CODE_SETS.into_iter().find(|s| s.id == id)
    }

    /// The overlay text for the picked codes: in the set's order, once each, joined by ", ".
    /// Fails with the first picked code that isn't in the set.
    pub fn overlay<'a>(&self, picked: &[&'a str]) -> Result<String, &'a str> {
        if let Some(bad) = picked.iter().find(|p| !self.codes.contains(p)) {
            return Err(bad);
        }
        Ok(self.codes.iter().filter(|c| picked.contains(c)).copied().collect::<Vec<_>>().join(", "))
    }
}
