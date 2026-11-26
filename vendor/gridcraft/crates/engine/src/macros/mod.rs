//! GridCraft macros: typed, declarative command lists, not VBA. The model is plain data in
//! `gridcraft_model::Macro`; this module reads and writes it and keeps it within its limits.
//!
//! Storage: the JSON format keeps `Workbook::macros` as a field. In xlsx each macro is one item of
//! GridCraft's hidden workbook data ([`crate::cmd::stash`]), `_gc_macro_<n>_<k>`, written when the
//! file is saved and taken back out of the names when it is opened, so the names never linger in
//! the open document. Scripts made with Automate › New Script or Record Actions are a separate,
//! older feature with their own storage; macros neither read nor change it. A recording keeps the
//! steps a script run ran while it recorded, as the run reported them.

use std::collections::{BTreeMap, HashMap, HashSet};

use gridcraft_model::{DefinedName, Macro, Step, StepKind, Workbook};
use serde_json::{Value as Json, json};

use crate::cmd::stash;

pub mod debug;
pub mod policy;
pub mod recorder;
pub mod resolve;
pub mod run;

/// The hidden-data key prefix of a stored macro (`macro_<n>`).
const MACRO_KEY: &str = "macro_";

/// Reads a macro from JSON, as `macro.save` and files give it. A command step may also use the
/// short form `{"command": "cell.set", "params": {…}}`.
pub fn parse_macro(v: &Json) -> Result<Macro, String> {
    // A newer format is refused before its steps are read, so the message says why.
    if let Some(found) = v.get("v").and_then(Json::as_u64) {
        gridcraft_model::check_version(found)?;
    }
    let mut v = v.clone();
    if let Some(steps) = v.get_mut("steps") {
        normalize_steps(steps, 1)?;
    }
    if v.get("v").is_none()
        && let Some(o) = v.as_object_mut()
    {
        o.insert("v".into(), json!(gridcraft_model::MACRO_VERSION));
    }
    let mut m: Macro = serde_json::from_value(v).map_err(|e| format!("This isn't a valid macro: {e}"))?;
    m.name = m.name.trim().to_string();
    check(&m)?;
    Ok(m)
}

/// Every check a macro must pass before it is kept in a workbook: the model's limits
/// ([`Macro::check_limits`]) and the size an xlsx file can hold. Every path that adds a macro,
/// from JSON, from a file or built in code, goes through this one check.
pub fn check(m: &Macro) -> Result<(), String> {
    m.check_limits()?;
    check_size(m)
}

/// Refuses a macro too large to keep in an xlsx file ([`stash::MAX_ITEM_CHARS`] characters of
/// JSON). Every command that adds or changes a macro calls [`check`], so the size of a macro kept
/// in the workbook doesn't make a save fail. (A clashing visible name doesn't either:
/// [`with_names`] stores the macro under another index.)
fn check_size(m: &Macro) -> Result<(), String> {
    if macro_json(m).to_string().chars().count() > stash::MAX_ITEM_CHARS {
        return Err(format!("This macro is too large to keep in the workbook ({} characters at most).", stash::MAX_ITEM_CHARS));
    }
    Ok(())
}

/// Leaves out the last top-level steps of `m` until it fits the size [`check_size`] allows, and
/// returns how many steps it left out (a recording cut to fit, as at the step limit). Each step's
/// JSON is measured once.
pub fn fit_size(m: &mut Macro) -> usize {
    let chars = |v: &Json| v.to_string().chars().count();
    let steps = std::mem::take(&mut m.steps);
    // The macro without steps holds `"steps":[]`; each step adds its JSON, and a comma after the
    // first.
    let mut size = chars(&macro_json(m));
    let mut kept = 0usize;
    for (i, st) in steps.iter().enumerate() {
        let add = chars(&serde_json::to_value(st).unwrap_or(Json::Null)).saturating_add(usize::from(i > 0));
        if size.saturating_add(add) > stash::MAX_ITEM_CHARS {
            break;
        }
        size = size.saturating_add(add);
        kept = i.saturating_add(1);
    }
    let cut = steps.len().saturating_sub(kept);
    m.steps = steps;
    m.steps.truncate(kept);
    cut
}

/// How many macros the workbook holds: the readable ones and the ones kept in a newer format
/// (`Workbook::macros_unread`), which a save writes back and the next open counts too.
pub fn count(wb: &Workbook) -> usize {
    wb.macros.len().saturating_add(wb.macros_unread.len())
}

/// Refuses one more macro in a workbook that holds [`gridcraft_model::MAX_MACROS`] already,
/// counting the kept ones, so a save never holds more than the next open keeps.
pub fn check_room(wb: &Workbook) -> Result<(), String> {
    let max = gridcraft_model::MAX_MACROS;
    if count(wb) < max {
        return Ok(());
    }
    Err(match wb.macros_unread.len() {
        0 => format!("A workbook can hold at most {max} macros."),
        kept => format!(
            "A workbook can hold at most {max} macros, and {kept} of the {} in this one {} made by a newer version of GridCraft and kept unchanged.",
            count(wb),
            if kept == 1 { "was" } else { "were" }
        ),
    })
}

/// The name of the macro kept in a newer format called `name` (compared as [`find`] compares),
/// as the file held it. Its name is taken: a new or renamed macro with it would clash when the
/// newer version reads the file.
pub fn kept_name(wb: &Workbook, name: &str) -> Option<String> {
    let name = name.trim();
    wb.macros_unread
        .iter()
        .filter_map(|v| v.get("name").and_then(Json::as_str))
        .map(str::trim)
        .find(|k| k.eq_ignore_ascii_case(name))
        .map(str::to_string)
}

/// Rewrites the short step form into the tagged one, in place, refusing nesting past the limit
/// before the steps are deserialized.
fn normalize_steps(steps: &mut Json, level: usize) -> Result<(), String> {
    let Some(list) = steps.as_array_mut() else { return Err("`steps` must be a list.".into()) };
    for st in list {
        let Some(o) = st.as_object_mut() else { return Err("Every step must be an object.".into()) };
        if !o.contains_key("type")
            && let Some(id) = o.remove("command")
        {
            o.insert("type".into(), json!("command"));
            o.insert("id".into(), id);
        }
        // A step with a body is a Repeat or an If: one at the top is nested 1 deep.
        let nested = ["body", "then", "else"].iter().any(|key| o.contains_key(*key));
        if nested && level > gridcraft_model::MAX_MACRO_DEPTH {
            return Err(format!("Repeat and If steps can nest at most {} deep.", gridcraft_model::MAX_MACRO_DEPTH));
        }
        for key in ["body", "then", "else"] {
            if let Some(body) = o.get_mut(key) {
                normalize_steps(body, level + 1)?;
            }
        }
    }
    Ok(())
}

/// A macro as JSON (the shape `macro.get` returns and `macro.save` takes).
pub fn macro_json(m: &Macro) -> Json {
    serde_json::to_value(m).unwrap_or(Json::Null)
}

/// The index of the macro called `name` (case-insensitive).
pub fn find(wb: &Workbook, name: &str) -> Option<usize> {
    let name = name.trim();
    wb.macros.iter().position(|m| m.name.eq_ignore_ascii_case(name))
}

/// A macro's content as canonical JSON text (object keys sorted): approval to run a macro holds
/// until the macro changes. It is the text itself rather than a hash, so no two different macros
/// share it.
pub fn fingerprint(m: &Macro) -> String {
    macro_json(m).to_string()
}

/// A copy of `wb` for xlsx with each macro in hidden names (`_gc_macro_<n>_<k>`) instead of the
/// field, or `None` when there is nothing to change. The macros keep their order; the index `n`
/// is this build's choice, so it steps around any a person's own visible name already uses.
pub fn with_names(wb: &Workbook) -> crate::Result<Option<Workbook>> {
    // Only the numbered items are this build's: a hidden `_gc_macro_<x>_<k>` name with some
    // other key stays, as `load` leaves it.
    let stored = |n: &DefinedName| macro_item(n).is_some_and(|(_, i)| i.is_some());
    if wb.macros.is_empty() && wb.macros_unread.is_empty() && !wb.names.iter().any(stored) {
        return Ok(None);
    }
    // A macro that skipped `check` (built in code) fails the save, as the JSON save does, rather
    // than being written and dropped the next time the file opens.
    for m in wb.macros.iter() {
        check(m).map_err(crate::EngineError::Other)?;
    }
    let mut out = wb.clone();
    // One pass drops the items of the last save, and one writes the macros (a file can hold a
    // thousand macros and many thousands of names).
    out.names.retain(|n| !stored(n));
    // A visible workbook-scope name of the storage shape is someone's own, and `set_many` won't
    // write a hidden one of the same name beside it. Such a name (`_GC_MACRO_1_0`, or one a
    // person defined after the file opened) blocks its item's key, in any chunk, so the macros
    // take the free indices from 0 up. The keys compare as the stash reads them, so this skips
    // exactly what `set_many` would refuse, and no arithmetic depends on a number from the file.
    let blocked: HashSet<String> = out.names.iter().filter(|n| !n.hidden && n.scope.is_none()).filter_map(|n| stash::item_key(&n.name)).collect();
    let free = (0..=u64::MAX).map(|i| format!("{MACRO_KEY}{i}")).filter(|key| !blocked.contains(key));
    // Macros in a newer format follow the readable ones, written back as the file held them.
    let texts = wb.macros.iter().map(|m| macro_json(m).to_string()).chain(wb.macros_unread.iter().map(Json::to_string));
    let items: Vec<(String, String)> = texts.zip(free).map(|(text, key)| (key, text)).collect();
    stash::set_many(&mut out, &items)?;
    // An empty list of its own, not a copy of the shared one cleared.
    out.macros = gridcraft_model::Macros::default();
    out.macros_unread = std::sync::Arc::default();
    Ok(Some(out))
}

/// The JSON format's bytes for `wb`: the workbook, with the macros in a newer format put back into
/// the `macros` list as the file held them.
pub fn json_bytes(wb: &Workbook) -> crate::Result<Vec<u8>> {
    // Every path that adds a macro checks it, so this refuses only one that skipped the check, as
    // the xlsx save does, instead of writing a macro the next open would drop.
    for m in &wb.macros {
        check(m).map_err(crate::EngineError::Other)?;
    }
    craft_gridcraft_format::write_with_kept(wb, "macros", &wb.macros_unread).map_err(|e| crate::EngineError::Other(e.to_string()))
}

/// Whether `v` is a macro in a newer format than this build reads.
fn is_newer(v: &Json) -> bool {
    v.get("v").and_then(Json::as_u64).is_some_and(|found| found > u64::from(gridcraft_model::MACRO_VERSION))
}

/// Keeps a macro in a newer format unread, if it is small enough to save back
/// ([`stash::MAX_ITEM_CHARS`] characters of JSON, the size every xlsx save can write).
fn keep_unread(unread: &mut Vec<Json>, v: Json, bad: &mut usize) {
    if v.to_string().chars().count() <= stash::MAX_ITEM_CHARS {
        unread.push(v);
    } else {
        *bad = bad.saturating_add(1);
    }
}

/// After a file is opened: moves macros out of hidden names into `Workbook::macros`, keeps
/// macros in a newer format unread (`Workbook::macros_unread`), and drops macros that are damaged
/// or over the limits. Returns a warning for what was kept unread and one for what was dropped.
pub fn load(wb: &mut Workbook) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut bad = 0usize;
    let mut unread = Vec::new();
    // Entries of the JSON format's field, each read on its own so a damaged one drops alone.
    let read = std::mem::take(&mut wb.macros_read);
    bad = bad.saturating_add(read.len().saturating_sub(gridcraft_model::MAX_MACROS));
    for v in read.into_iter().take(gridcraft_model::MAX_MACROS) {
        if is_newer(&v) {
            keep_unread(&mut unread, v, &mut bad);
            continue;
        }
        match parse_macro(&v) {
            Ok(m) => wb.macros.push(m),
            Err(_) => bad += 1,
        }
    }
    // One pass takes the hidden macro names out of the workbook, grouped by macro in saved order.
    let mut stored: BTreeMap<(u64, String), Vec<DefinedName>> = BTreeMap::new();
    let mut kept = Vec::new();
    for n in std::mem::take(&mut wb.names) {
        match macro_item(&n) {
            None => kept.push(n),
            Some((key, Some(i))) => stored.entry((i, key)).or_default().push(n),
            // A `macro_` item that isn't numbered isn't one this build wrote: it stays as it is,
            // unread, and a save keeps it.
            Some((_, None)) => kept.push(n),
        }
    }
    wb.names = kept;
    // Past the macro limit, the rest are counted without being read.
    bad = bad.saturating_add(stored.len().saturating_sub(gridcraft_model::MAX_MACROS));
    let mut found = Vec::new();
    let mut item = Workbook::new();
    for ((_, key), names) in stored.into_iter().take(gridcraft_model::MAX_MACROS) {
        item.names = names;
        let v = stash::get_json(&item, &key);
        if let Some(v) = v.as_ref().filter(|v| is_newer(v)) {
            keep_unread(&mut unread, v.clone(), &mut bad);
            continue;
        }
        match v.ok_or_else(|| "unreadable".to_string()).and_then(|v| parse_macro(&v)) {
            Ok(m) => found.push(m),
            Err(_) => bad += 1,
        }
    }
    // All of them go through the same limits, and a clashing name gets a suffix.
    let mut all = std::mem::take(&mut wb.macros);
    all.extend(found);
    let mut taken = Taken::default();
    for m in all {
        if wb.macros.len() >= gridcraft_model::MAX_MACROS {
            bad += 1;
            continue;
        }
        let mut m = m;
        let checked = m.check_limits().and_then(|()| {
            m.name = taken.unique(m.name.trim());
            // The macro as it is kept, renamed, must still pass, or the next save would lose it.
            check(&m)
        });
        match checked {
            Ok(()) => {
                taken.insert(&m.name);
                wb.macros.push(m);
            }
            Err(_) => bad += 1,
        }
    }
    // Readable and unread macros together stay within the limit, the readable ones first.
    let room = gridcraft_model::MAX_MACROS.saturating_sub(wb.macros.len());
    bad = bad.saturating_add(unread.len().saturating_sub(room));
    unread.truncate(room);
    if !unread.is_empty() {
        let one = unread.len() == 1;
        warnings.push(format!(
            "{} macro{} in the workbook {} made by a newer version of GridCraft: this version can't show or run {}, and saving keeps {} unchanged.",
            unread.len(),
            if one { "" } else { "s" },
            if one { "was" } else { "were" },
            if one { "it" } else { "them" },
            if one { "it" } else { "them" },
        ));
    }
    wb.macros_unread = std::sync::Arc::new(unread);
    if bad > 0 {
        warnings.push(format!(
            "{bad} macro{} in the workbook couldn't be opened: {} damaged or over the limits ({} macros, {} steps each, Repeat and If nested {} deep).",
            if bad == 1 { "" } else { "s" },
            if bad == 1 { "it is" } else { "they are" },
            gridcraft_model::MAX_MACROS,
            gridcraft_model::MAX_MACRO_STEPS,
            gridcraft_model::MAX_MACRO_DEPTH
        ));
    }
    warnings
}

/// The item key (`macro_<n>`, lower-cased) of a hidden macro name, and `n` when it is a number.
fn macro_item(n: &DefinedName) -> Option<(String, Option<u64>)> {
    // Only a hidden workbook-scope name is macro storage: a person's own visible name that
    // happens to look like one stays a name.
    let key = stash::item_key(&n.name).filter(|k| n.hidden && n.scope.is_none() && k.starts_with(MACRO_KEY))?;
    let i = key.strip_prefix(MACRO_KEY).and_then(|i| i.parse().ok());
    Some((key, i))
}

/// `name`, or `name (2)`, `name (3)`… if a macro already has it, a kept one too. A free name comes back as it is;
/// a renamed one is shortened to make room for the suffix, so it stays within
/// [`gridcraft_model::MAX_MACRO_NAME`] characters. The result still goes through [`check`].
pub fn unique_name(wb: &Workbook, name: &str) -> String {
    let mut taken = Taken::default();
    for m in &wb.macros {
        taken.insert(&m.name);
    }
    for kept in wb.macros_unread.iter().filter_map(|v| v.get("name").and_then(Json::as_str)) {
        taken.insert(kept);
    }
    taken.unique(name)
}

/// Macro names in use, compared as [`find`] compares them, and the next suffix to try for each
/// shortened name and suffix width, so naming many clashing macros takes near-linear work.
#[derive(Default)]
struct Taken {
    names: HashSet<String>,
    next: HashMap<(String, usize), u64>,
    /// Name lookups made, for tests.
    probes: usize,
}

impl Taken {
    fn key(name: &str) -> String {
        name.trim().to_ascii_lowercase()
    }

    fn insert(&mut self, name: &str) {
        self.names.insert(Taken::key(name));
    }

    fn contains(&mut self, name: &str) -> bool {
        self.probes = self.probes.saturating_add(1);
        self.names.contains(&Taken::key(name))
    }

    /// The first of `name`, `name (2)`, `name (3)`… not in use.
    fn unique(&mut self, name: &str) -> String {
        let name = if name.is_empty() { "Macro" } else { name };
        if !self.contains(name) {
            return name.to_string();
        }
        let mut n: u64 = 2;
        loop {
            let suffix = format!(" ({n})");
            let keep = gridcraft_model::MAX_MACRO_NAME.saturating_sub(suffix.chars().count());
            let base: String = name.chars().take(keep).collect();
            let base = base.trim_end();
            // Every candidate for this base and suffix width below `next` is in use already
            // (names are only added), so the search goes on from there.
            let key = (Taken::key(base), suffix.len());
            let next = self.next.get(&key).copied().unwrap_or(n);
            if next > n {
                n = next;
                continue;
            }
            let Some(after) = n.checked_add(1) else { return name.to_string() };
            self.next.insert(key, after);
            let candidate = format!("{base}{suffix}");
            if !self.contains(&candidate) {
                return candidate;
            }
            n = after;
        }
    }
}

/// Ctrl+Shift (Cmd+Shift on a Mac) letters that GridCraft already uses, on any platform, which a
/// macro cannot take.
pub const RESERVED_SHORTCUTS: &[(char, &str)] = &[
    ('F', "Format Cells (Font) or Filter"),
    ('G', "Workbook Statistics"),
    ('H', "Replace"),
    ('J', "Ungroup"),
    ('K', "Group"),
    ('L', "Filter"),
    ('R', "Sort"),
    ('S', "Save As"),
    ('T', "AutoSum"),
    ('Z', "Redo"),
];

/// The letter of a macro shortcut: `"Ctrl+Shift+M"`, `"Cmd+Shift+M"`, `"⌘⇧M"` and `"m"` all give
/// `'M'`. `None` when it is not one letter from A to Z, alone or after Ctrl+Shift (Cmd+Shift):
/// other modifiers (`"Ctrl+Alt+M"`) are refused rather than read as Ctrl+Shift.
pub fn parse_shortcut(s: &str) -> Option<char> {
    let letter = |t: &str| {
        let mut chars = t.trim().chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
            _ => None,
        }
    };
    let t = s.trim().to_ascii_lowercase();
    ["ctrl+shift+", "cmd+shift+", "⌘⇧"].iter().find_map(|p| t.strip_prefix(p)).map_or_else(|| letter(&t), letter)
}

/// How a shortcut is stored and shown: `Ctrl+Shift+M`.
pub fn shortcut_text(letter: char) -> String {
    format!("Ctrl+Shift+{}", letter.to_ascii_uppercase())
}

/// Why macro `name` cannot use Ctrl+Shift+`letter`: GridCraft uses it, or another macro has it.
pub fn shortcut_conflict(wb: &Workbook, name: &str, letter: char) -> Option<String> {
    let letter = letter.to_ascii_uppercase();
    if let Some((_, what)) = RESERVED_SHORTCUTS.iter().find(|(c, _)| *c == letter) {
        return Some(format!("{} is {what}. Choose another letter.", shortcut_text(letter)));
    }
    wb.macros
        .iter()
        .find(|m| !m.name.eq_ignore_ascii_case(name.trim()) && m.shortcut.as_deref().and_then(parse_shortcut) == Some(letter))
        .map(|m| format!("{} already runs macro “{}”. Choose another letter.", shortcut_text(letter), m.name))
}

/// `name` trimmed, if it can name a new macro in `wb`; otherwise why not: it is empty, too long,
/// holds line breaks or another macro has it.
pub fn check_new_name(wb: &Workbook, name: &str) -> Result<String, String> {
    let name = name.trim();
    let probe = Macro { name: name.to_string(), ..Macro::default() };
    probe.check_limits()?;
    if name.chars().any(char::is_control) {
        return Err("A macro name cannot hold line breaks or other control characters.".into());
    }
    if find(wb, name).is_some() {
        return Err(format!("A macro called “{name}” already exists. Choose another name."));
    }
    if let Some(kept) = kept_name(wb, name) {
        return Err(format!("A macro called “{kept}” is kept from a newer version of GridCraft. Choose another name."));
    }
    Ok(name.to_string())
}

/// The first free `Macro1`, `Macro2`… name, also free among the kept macros ([`kept_name`]).
pub fn next_name(wb: &Workbook) -> String {
    (1..).map(|n| format!("Macro{n}")).find(|n| find(wb, n).is_none() && kept_name(wb, n).is_none()).unwrap_or_else(|| "Macro".into())
}

/// Every command step of `steps` (inside Repeat and If too), with its path.
pub fn command_steps(steps: &[Step]) -> Vec<(String, &str, &Json)> {
    fn walk<'a>(steps: &'a [Step], prefix: &str, out: &mut Vec<(String, &'a str, &'a Json)>) {
        for (i, st) in steps.iter().enumerate() {
            let path = if prefix.is_empty() { (i + 1).to_string() } else { format!("{prefix}.{}", i + 1) };
            match &st.kind {
                StepKind::Command { id, params } => out.push((path, id.as_str(), params)),
                StepKind::Repeat { body, .. } => walk(body, &path, out),
                StepKind::If { then, otherwise, .. } => {
                    walk(then, &format!("{path}.then"), out);
                    walk(otherwise, &format!("{path}.else"), out);
                }
                StepKind::Stop { .. } => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(steps, "", &mut out);
    out
}

#[cfg(test)]
mod tests;
