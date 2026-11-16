//! The printer list as it was last read, kept across restarts so the Print dialog of a fresh
//! window offers it at once rather than waiting on the spooler. Listing can take many seconds
//! (CUPS' `lpstat -e` browses the network for driverless printers; a Windows spooler may wait on
//! a networked printer), and each document opened from the file manager is often a new process.
//! The dialog still reads the list again in the background, and the fresh one replaces this.
//!
//! Two-sided support isn't kept: it is read for the printer picked (`TwoSidedSource`).

use super::spool::Printer;

/// At most this many printers are kept (settings are untrusted).
pub const MAX_KNOWN: usize = 256;
/// A longer name or description in the settings file is refused (CUPS and Windows queue names
/// are far shorter; descriptions are capped at 256 characters by `parse_lpstat`).
const MAX_BYTES: usize = 1024;

/// The list for the settings file: each printer's name, description and whether it is the
/// system default.
pub fn to_json(printers: &[Printer]) -> serde_json::Value {
    printers.iter().take(MAX_KNOWN).map(|p| serde_json::json!({ "name": p.name, "description": p.description, "default": p.default })).collect()
}

/// The list read back from the settings file; `None` when it holds no list, so the caller keeps
/// what it has. A malformed entry or a repeated name is skipped, and at most one printer is the
/// default.
pub fn from_json(v: &serde_json::Value) -> Option<Vec<Printer>> {
    let mut out: Vec<Printer> = Vec::new();
    for entry in v.as_array()? {
        if out.len() >= MAX_KNOWN {
            break;
        }
        let Some(mut p) = read_entry(entry) else { continue };
        if out.iter().any(|known| known.name == p.name) {
            continue;
        }
        p.default &= !out.iter().any(|known| known.default);
        out.push(p);
    }
    Some(out)
}

fn read_entry(entry: &serde_json::Value) -> Option<Printer> {
    let name = entry.get("name")?.as_str().filter(|n| !n.is_empty() && n.len() <= MAX_BYTES)?;
    let description = entry.get("description").and_then(serde_json::Value::as_str).unwrap_or_default();
    let description = if description.len() <= MAX_BYTES { description } else { "" };
    let default = entry.get("default").and_then(serde_json::Value::as_bool).unwrap_or(false);
    Some(Printer { name: name.to_string(), description: description.to_string(), default, two_sided: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(name: &str, description: &str, default: bool) -> Printer {
        Printer { name: name.into(), description: description.into(), default, two_sided: None }
    }

    #[test]
    fn the_list_round_trips_through_the_settings_file() {
        let list = vec![printer("Office", "Office laser", true), printer("_192_0_2_15", "192.0.2.15", false), printer("Label", "", false)];
        assert_eq!(from_json(&to_json(&list)), Some(list));
    }

    #[test]
    fn two_sided_support_is_not_kept() {
        let list = vec![Printer { two_sided: Some(true), ..printer("Office", "", true) }];
        assert_eq!(from_json(&to_json(&list)).unwrap()[0].two_sided, None);
    }

    #[test]
    fn malformed_entries_are_skipped() {
        let list = serde_json::json!([
            {"name": ""},
            {"name": 5},
            {"name": "x".repeat(MAX_BYTES + 1)},
            {"description": "no name"},
            "Office",
            null,
            {"name": "Office", "default": true},
            {"name": "Office", "description": "again"},
            {"name": "Second default", "default": true},
            {"name": "Long", "description": "x".repeat(MAX_BYTES + 1), "default": "yes"},
        ]);
        assert_eq!(from_json(&list), Some(vec![printer("Office", "", true), printer("Second default", "", false), printer("Long", "", false)]));
        for v in [serde_json::Value::Null, serde_json::json!({}), serde_json::json!("Office"), serde_json::json!(3)] {
            assert_eq!(from_json(&v), None, "{v}");
        }
        let many: Vec<_> = (0..MAX_KNOWN * 2).map(|i| serde_json::json!({"name": format!("Q{i}")})).collect();
        assert_eq!(from_json(&serde_json::Value::Array(many)).unwrap().len(), MAX_KNOWN);
    }
}
