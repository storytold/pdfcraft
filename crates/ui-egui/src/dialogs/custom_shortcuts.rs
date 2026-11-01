//! Customizable keyboard shortcuts (#4635): Help ▸ Keyboard shortcuts lets each registered
//! command take another key, lose its key, or get one it lacks, and puts them back. The changes
//! are kept in the settings and published to the context each frame, so the menus, the palette,
//! tooltips and the key handling all read the same binding. Keys another command runs on are
//! only taken once the user says Reassign. The keys the document view handles itself (the
//! arrows, zoom, copy…) stay as they are and can't be taken.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Key, Modifiers};
use pdfcraft_engine::commands::{COMMANDS, CommandSpec, Shortcut};

/// Where the published bindings live in the context's data.
const DATA_ID: &str = "custom-shortcuts";

/// Commands whose key is the system's and can't be changed here.
const FIXED: [&str; 1] = ["edit.copy"];

/// The commands whose binding was changed: a new key, or `None` for none. Commands left out keep
/// their default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides(BTreeMap<&'static str, Option<Shortcut>>);

/// Why a key can't be a command's shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Alt combinations aren't shortcuts here.
    Alt,
    /// Escape, Tab, Enter, Space and Backspace on their own work the interface.
    Navigation,
    /// The document view uses the key itself (named by what it does there).
    Reserved(&'static str),
}

impl Refusal {
    /// What the dialog says, translated.
    pub fn message(&self) -> String {
        match self {
            Refusal::Alt => tl!("Alt combinations can't be shortcuts. Try Ctrl or Shift.").to_string(),
            Refusal::Navigation => tl!("This key moves around the interface on its own. Add Ctrl to use it.").to_string(),
            Refusal::Reserved(what) => crate::i18n::fmt(tl!("This key is used for “{what}” and can't be changed."), &[("what", tl!(what))]),
        }
    }
}

/// Keys the document view reads itself, after the registered commands (which would take them
/// first): binding one to a command would break what it does there.
fn reserved() -> [(Shortcut, &'static str); 13] {
    use crate::commands::{
        ACTUAL_SIZE, COPY, FIND_NEXT, FIND_PREV, FIT_WIDTH, PAGE_LEVEL, PAGE_NEXT, PAGE_PREV, ROTATE_CCW, ROTATE_CW, SELECT_ALL, ZOOM_IN, ZOOM_OUT,
    };
    [
        (FIND_NEXT, "Next / previous match"),
        (FIND_PREV, "Next / previous match"),
        (COPY, "Copy selected text"),
        (ACTUAL_SIZE, "Actual size"),
        (PAGE_LEVEL, "Zoom to page level"),
        (FIT_WIDTH, "Fit to width"),
        (ZOOM_IN, "Zoom in"),
        (ZOOM_OUT, "Zoom out"),
        (ROTATE_CW, "Rotate view"),
        (ROTATE_CCW, "Rotate view"),
        (SELECT_ALL, "Select all"),
        (PAGE_PREV, "Previous / next page"),
        (PAGE_NEXT, "Previous / next page"),
    ]
}

/// Whether `a` and `b` are the same keys, however the key is spelled ("," or "Comma").
pub fn same(a: Shortcut, b: Shortcut) -> bool {
    (a.command, a.shift, a.mac_ctrl) == (b.command, b.shift, b.mac_ctrl) && key_of(a.key).is_some_and(|k| key_of(b.key) == Some(k))
}

/// The key a shortcut names; the arrows the view writes as `←`/`→` too.
fn key_of(name: &str) -> Option<Key> {
    match name {
        "←" => Some(Key::ArrowLeft),
        "→" => Some(Key::ArrowRight),
        _ => Key::from_name(name),
    }
}

/// How a key is written in a shortcut: punctuation as itself (`,`), arrows and named keys by
/// name (`Left`, `Delete`, `F5`), letters and digits as they are.
fn key_name(key: Key) -> &'static str {
    match key {
        Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp => key.name(),
        _ => key.symbol_or_name(),
    }
}

/// The shortcut a key press makes, or why it can't be one. `mac` reads Control as ⌃, apart from
/// ⌘; elsewhere Ctrl is the command key.
pub fn from_press(key: Key, m: Modifiers, mac: bool) -> Result<Shortcut, Refusal> {
    if m.alt {
        return Err(Refusal::Alt);
    }
    let s = Shortcut { command: m.command, shift: m.shift, mac_ctrl: mac && m.ctrl, key: key_name(key) };
    if !s.command && !s.mac_ctrl && matches!(key, Key::Escape | Key::Tab | Key::Enter | Key::Space | Key::Backspace) {
        return Err(Refusal::Navigation);
    }
    if let Some((_, what)) = reserved().into_iter().find(|(r, _)| same(*r, s)) {
        return Err(Refusal::Reserved(what));
    }
    if let Some(what) = view_key(key, s) {
        return Err(Refusal::Reserved(what));
    }
    Ok(s)
}

/// What the document view does with `key` pressed without Ctrl/⌘ (or ⌃), where it reads the key
/// itself: the tool keys, moving through the pages, and deleting the selection. ⌘Page Up /
/// ⌘Page Down turn pages too.
fn view_key(key: Key, s: Shortcut) -> Option<&'static str> {
    let plain = !s.command && !s.mac_ctrl;
    match key {
        Key::PageUp | Key::PageDown => Some("Previous / next page"),
        _ if !plain => None,
        Key::V if !s.shift => Some("Select tool"),
        Key::H if !s.shift => Some("Hand tool"),
        Key::Home | Key::End => Some("First / last page"),
        Key::ArrowLeft | Key::ArrowRight => Some("Previous / next page"),
        Key::ArrowUp | Key::ArrowDown => Some("Scroll the page"),
        Key::Delete => Some("Delete the selection"),
        _ => None,
    }
}

/// A shortcut as the settings store it: `cmd+shift+ctrl+K` (only the modifiers it has).
pub fn token(s: Shortcut) -> String {
    let mut parts = Vec::new();
    for (on, name) in [(s.command, "cmd"), (s.shift, "shift"), (s.mac_ctrl, "ctrl")] {
        if on {
            parts.push(name);
        }
    }
    parts.push(s.key);
    parts.join("+")
}

/// [`token`] read back; `None` for anything else. The key is last, so `cmd++` is ⌘ and `+`.
pub fn parse(token: &str) -> Option<Shortcut> {
    let (mods, key) = match token.strip_suffix('+') {
        Some(rest) if rest.is_empty() || rest.ends_with('+') => (rest.strip_suffix('+').unwrap_or(rest), "+"),
        _ => token.rsplit_once('+').unwrap_or(("", token)),
    };
    let key = key_name(key_of(key)?);
    let mut s = Shortcut { command: false, shift: false, mac_ctrl: false, key };
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        let flag = match m {
            "cmd" => &mut s.command,
            "shift" => &mut s.shift,
            "ctrl" => &mut s.mac_ctrl,
            _ => return None,
        };
        if *flag {
            return None;
        }
        *flag = true;
    }
    Some(s)
}

impl Overrides {
    /// The shortcut `spec` runs on now.
    pub fn get(&self, spec: &CommandSpec) -> Option<Shortcut> {
        self.0.get(spec.id).copied().unwrap_or(spec.shortcut)
    }

    /// Whether `id`'s binding was changed.
    pub fn is_custom(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    /// The command that runs on `s` now, if any.
    pub fn holder(&self, s: Shortcut) -> Option<&'static CommandSpec> {
        COMMANDS.iter().find(|c| self.get(c).is_some_and(|k| same(k, s)))
    }

    /// Give command `id` the shortcut `s`. Another command that had it loses it; that command is
    /// returned so the change can be said. Unknown commands change nothing.
    pub fn assign(&mut self, id: &str, s: Shortcut) -> Option<&'static CommandSpec> {
        let spec = pdfcraft_engine::commands::command(id).filter(|c| !FIXED.contains(&c.id))?;
        let taken = self.holder(s).filter(|c| c.id != spec.id);
        if let Some(other) = taken {
            self.set(other, None);
        }
        self.set(spec, Some(s));
        taken
    }

    /// Leave command `id` without a shortcut.
    pub fn remove(&mut self, id: &str) {
        if let Some(spec) = pdfcraft_engine::commands::command(id) {
            self.set(spec, None);
        }
    }

    /// Give command `id` its default shortcut back. Another command that took that key meanwhile
    /// loses it, so one key never runs two commands.
    pub fn reset(&mut self, id: &str) -> Option<&'static CommandSpec> {
        let spec = pdfcraft_engine::commands::command(id)?;
        match spec.shortcut {
            Some(s) => self.assign(id, s),
            None => {
                self.0.remove(spec.id);
                None
            }
        }
    }

    /// Every command back to its default.
    pub fn reset_all(&mut self) {
        self.0.clear();
    }

    /// Record `to` for `spec`, or forget the change when it is the default anyway.
    fn set(&mut self, spec: &'static CommandSpec, to: Option<Shortcut>) {
        // Copy is the system's own Ctrl/⌘C (egui delivers it as a clipboard event), which no
        // binding can move or take away; its menu entry always names that key.
        if FIXED.contains(&spec.id) {
            return;
        }
        let default = match (spec.shortcut, to) {
            (None, None) => true,
            (Some(a), Some(b)) => same(a, b),
            _ => false,
        };
        if default {
            self.0.remove(spec.id);
        } else {
            self.0.insert(spec.id, to);
        }
    }

    /// For the settings file: `{ "command id": "cmd+K" or null }`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Object(self.0.iter().map(|(id, s)| ((*id).to_string(), s.map_or(serde_json::Value::Null, |s| token(s).into()))).collect())
    }

    /// From the settings file, leniently: unknown commands and unreadable keys are dropped, as is
    /// anything [`from_press`] would refuse, and a key given twice runs only the first command
    /// that has it.
    pub fn from_json(v: &serde_json::Value) -> Self {
        let mut out = Self::default();
        let Some(map) = v.as_object() else { return out };
        // Unknown commands are skipped before the cap, so they can't crowd out a real one.
        let known = map.iter().filter_map(|(id, s)| pdfcraft_engine::commands::command(id).map(|spec| (spec, s)));
        for (spec, s) in known.take(COMMANDS.len()) {
            match s {
                serde_json::Value::Null => out.set(spec, None),
                serde_json::Value::String(t) => {
                    let Some(s) = parse(t).filter(|s| allowed(*s)) else { continue };
                    // Another entry kept it already (a default it duplicates goes below).
                    if out.0.iter().any(|(id, k)| *id != spec.id && k.is_some_and(|k| same(k, s))) {
                        continue;
                    }
                    out.set(spec, Some(s));
                }
                _ => {}
            }
        }
        // A default that a kept key now duplicates loses it.
        for spec in COMMANDS {
            if let Some(s) = spec.shortcut.filter(|_| !out.is_custom(spec.id))
                && out.0.iter().any(|(id, k)| *id != spec.id && k.is_some_and(|k| same(k, s)))
            {
                out.0.insert(spec.id, None);
            }
        }
        out
    }
}

/// `ui.set shortcut <command>=<keys>` (CodeRabbit on #4646): the control channel's way to change a
/// binding, by the same rules as the Keyboard shortcuts dialog. `<keys>` is written as the
/// settings write it (`cmd+shift+K`, see [`token`]); `none` takes the command's key away,
/// `default` gives it back, and `all=default` puts every command back. A key another command had
/// is taken from it, as the dialog's Reassign does (there is no one to ask here). Refused, with
/// the reason, are unknown commands, Copy (the system's own key), keys that aren't a shortcut,
/// and the keys the dialog refuses.
pub(crate) fn set_from_control(app: &mut crate::PdfCraftApp, value: &str) -> Result<(), String> {
    let (id, to) = value.split_once('=').ok_or("shortcut must be <command id>=<keys>, <command id>=none, <command id>=default or all=default")?;
    let mut overrides = (*app.shortcuts).clone();
    if id == "all" {
        if to != "default" {
            return Err("shortcut: all only takes default (all=default)".into());
        }
        overrides.reset_all();
    } else {
        let spec = pdfcraft_engine::commands::command(id).ok_or_else(|| format!("shortcut: unknown command {id} (ui.commands lists them)"))?;
        if FIXED.contains(&spec.id) {
            return Err(format!("shortcut: {id} keeps the system's own key and can't be changed"));
        }
        match to {
            "none" => overrides.remove(id),
            "default" => {
                overrides.reset(id);
            }
            keys => {
                let s =
                    parse(keys).ok_or_else(|| format!("shortcut: {keys} is not a shortcut; write it as cmd+shift+ctrl+K, each modifier optional"))?;
                let key = key_of(s.key).ok_or_else(|| format!("shortcut: {keys} names no key"))?;
                let mut m = Modifiers::NONE;
                m.command = s.command;
                m.shift = s.shift;
                m.ctrl = s.mac_ctrl;
                from_press(key, m, s.mac_ctrl).map_err(|r| format!("shortcut: {}", r.message()))?;
                overrides.assign(id, s);
            }
        }
    }
    app.shortcuts = Arc::new(overrides);
    Ok(())
}

/// Whether `s` could have come from a key press ([`from_press`] says yes).
fn allowed(s: Shortcut) -> bool {
    let Some(key) = key_of(s.key) else { return false };
    let mut m = Modifiers::NONE;
    m.command = s.command;
    m.shift = s.shift;
    m.ctrl = s.mac_ctrl;
    from_press(key, m, s.mac_ctrl).is_ok()
}

/// Make `overrides` the bindings this frame's menus, tooltips and keys use.
pub(crate) fn publish(ctx: &egui::Context, overrides: &Arc<Overrides>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(DATA_ID), Arc::clone(overrides)));
}

/// The shortcut `spec` runs on in this context: the published one, or its default.
pub(crate) fn effective(ctx: &egui::Context, spec: &CommandSpec) -> Option<Shortcut> {
    match ctx.data(|d| d.get_temp::<Arc<Overrides>>(egui::Id::unique(DATA_ID))) {
        Some(o) => o.get(spec),
        None => spec.shortcut,
    }
}

/// While the dialog waits for `app.shortcut_capture`'s new keys: the next key press (taken from
/// everything else this frame) becomes its shortcut, or is refused with a reason; Escape alone
/// stops waiting. Runs before the dialog is drawn, so the modal never sees that Escape.
pub(crate) fn capture(ctx: &egui::Context, app: &mut crate::PdfCraftApp) {
    let Some(id) = app.shortcut_capture else { return };
    let mac = crate::commands::mac_shortcuts(ctx);
    let press = ctx.input_mut(|i| {
        let at = i.events.iter().position(|e| matches!(e, egui::Event::Key { pressed: true, repeat: false, .. }))?;
        let egui::Event::Key { key, modifiers, .. } = i.events.remove(at) else { return None };
        // Its release and any text it typed go too.
        i.events.retain(|e| !matches!(e, egui::Event::Key { key: k, .. } if *k == key) && !matches!(e, egui::Event::Text(_)));
        i.keys_down.remove(&key);
        Some((key, modifiers))
    });
    let Some((key, modifiers)) = press else { return };
    if key == Key::Escape && modifiers.is_none() {
        app.shortcut_capture = None;
        return;
    }
    match from_press(key, modifiers, mac) {
        Ok(s) => {
            app.shortcut_capture = None;
            // Keys another command runs on are only taken once the dialog's Reassign says so.
            if app.shortcuts.holder(s).is_some_and(|c| c.id != id) {
                app.shortcut_conflict = Some((id, s));
            } else {
                Arc::make_mut(&mut app.shortcuts).assign(id, s);
            }
        }
        Err(why) => app.notify(why.message()),
    }
}

/// Give the pending conflict's keys to its command, taking them from the one that had them, and
/// say so.
fn reassign(ctx: &egui::Context, app: &mut crate::PdfCraftApp) {
    let Some((id, s)) = app.shortcut_conflict.take() else { return };
    if let Some(other) = Arc::make_mut(&mut app.shortcuts).assign(id, s) {
        let label = crate::commands::shortcut_label(ctx, s);
        let other = tl!(other.label).trim_end_matches('…').to_string();
        app.notify(crate::i18n::fmt(tl!("{key} was taken from “{other}”, which now has no shortcut."), &[("key", &label), ("other", &other)]));
    }
}

/// The question a pressed key that another command runs on asks (#4635): Reassign takes it from
/// that command, Cancel keeps both as they were. Nothing is shown once the conflict is gone (the
/// other command lost the key meanwhile, say).
fn conflict_ui(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp, t: &crate::theme::Tokens) {
    let Some((id, s)) = app.shortcut_conflict else { return };
    let Some(other) = app.shortcuts.holder(s).filter(|c| c.id != id) else {
        app.shortcut_conflict = None;
        return;
    };
    let key = crate::commands::shortcut_label(ui.ctx(), s);
    let other = tl!(other.label).trim_end_matches('…').to_string();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(crate::i18n::fmt(tl!("{key} is already used by “{other}”."), &[("key", &key), ("other", &other)])).color(t.accent),
        );
        if ui.add(egui::Button::new(tl!("Reassign"))).clicked() {
            reassign(ui.ctx(), app);
        }
        if ui.add(egui::Button::new(tl!("Cancel"))).clicked() {
            app.shortcut_conflict = None;
        }
    });
    ui.add_space(4.0);
}

/// The registered commands' rows of Help ▸ Keyboard shortcuts: each with its key, Change, and
/// Remove or Reset where they apply. Commands without a key are listed when the search finds
/// them, so one can be given a key.
pub(crate) fn rows_ui(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp, t: &crate::theme::Tokens) {
    let search_id = egui::Id::unique("shortcut-search");
    let mut search = ui.data_mut(|d| d.get_temp::<String>(search_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        let l = ui.label(tl!("Find a command"));
        ui.add(crate::widgets::line(&mut search).desired_width(220.0).id(search_id.with("field"))).labelled_by(l.id);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any = COMMANDS.iter().any(|c| app.shortcuts.is_custom(c.id));
            if ui.add_enabled(any, egui::Button::new(tl!("Reset all shortcuts"))).clicked() {
                Arc::make_mut(&mut app.shortcuts).reset_all();
                app.shortcut_capture = None;
                app.shortcut_conflict = None;
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(search_id, search.clone()));
    let q = search.trim().to_lowercase();
    ui.add_space(4.0);
    conflict_ui(ui, app, t);
    egui::Grid::new("keys-registered").num_columns(3).spacing([16.0, 4.0]).show(ui, |ui| {
        for spec in COMMANDS {
            let key = app.shortcuts.get(spec);
            let label = tl!(spec.label).trim_end_matches('…').to_string();
            let found = !q.is_empty() && (label.to_lowercase().contains(&q) || spec.label.to_lowercase().contains(&q));
            let listed = if q.is_empty() { key.is_some() || app.shortcuts.is_custom(spec.id) } else { found };
            if !listed {
                continue;
            }
            let waiting = app.shortcut_capture == Some(spec.id);
            let shown = if waiting {
                egui::RichText::new(tl!("Press the new keys (Esc cancels)")).color(t.accent)
            } else {
                egui::RichText::new(key.map_or_else(|| "—".to_string(), |k| crate::commands::shortcut_label(ui.ctx(), k)))
                    .font(egui::FontId::monospace(12.5))
            };
            ui.label(shown);
            ui.label(&label);
            ui.horizontal(|ui| {
                let change = ui
                    .add(egui::Button::new(tl!("Change")))
                    .on_hover_text(crate::i18n::fmt(tl!("Choose new keys for {command}"), &[("command", &label)]));
                if change.clicked() {
                    app.shortcut_capture = if waiting { None } else { Some(spec.id) };
                    app.shortcut_conflict = None;
                }
                if key.is_some() && ui.add(egui::Button::new(tl!("Remove"))).clicked() {
                    Arc::make_mut(&mut app.shortcuts).remove(spec.id);
                }
                if app.shortcuts.is_custom(spec.id)
                    && ui.add(egui::Button::new(tl!("Reset"))).on_hover_text(tl!("Back to the default keys")).clicked()
                {
                    let back = spec.shortcut;
                    if let Some(other) = Arc::make_mut(&mut app.shortcuts).reset(spec.id)
                        && let Some(s) = back
                    {
                        let k = crate::commands::shortcut_label(ui.ctx(), s);
                        let other = tl!(other.label).trim_end_matches('…').to_string();
                        app.notify(crate::i18n::fmt(
                            tl!("{key} was taken from “{other}”, which now has no shortcut."),
                            &[("key", &k), ("other", &other)],
                        ));
                    }
                }
            });
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(key: &'static str) -> Shortcut {
        Shortcut::cmd(key)
    }

    #[test]
    fn tokens_round_trip_and_odd_ones_are_refused() {
        for s in [cmd("K"), Shortcut::cmd_shift("S"), cmd("+"), cmd(","), Shortcut { command: false, shift: false, mac_ctrl: false, key: "F5" }] {
            let back = parse(&token(s)).unwrap_or_else(|| panic!("{}", token(s)));
            assert!(same(back, s), "{} → {back:?}", token(s));
        }
        let ctrl_left = Shortcut { command: true, shift: false, mac_ctrl: true, key: "Left" };
        assert!(same(parse("cmd+ctrl+Left").unwrap(), ctrl_left));
        for bad in ["", "cmd+", "cmd+cmd+K", "alt+K", "cmd+Nope", "cmd+shift", "shift+ctrl"] {
            assert_eq!(parse(bad).map(token), None, "{bad:?}");
        }
        let plus = parse("+").unwrap();
        assert!(!plus.command && plus.key == "+", "a lone + is the plus key");
    }

    #[test]
    fn a_key_press_becomes_a_shortcut_unless_the_interface_or_view_needs_it() {
        let ctrl = Modifiers::COMMAND;
        assert!(same(from_press(Key::K, ctrl, false).unwrap(), cmd("K")));
        assert!(same(from_press(Key::Comma, ctrl, false).unwrap(), cmd(",")));
        assert_eq!(from_press(Key::K, Modifiers::ALT | ctrl, false), Err(Refusal::Alt));
        assert_eq!(from_press(Key::Escape, Modifiers::NONE, false), Err(Refusal::Navigation));
        assert!(from_press(Key::Tab, ctrl, false).is_ok(), "with Ctrl it is free");
        assert!(matches!(from_press(Key::C, ctrl, false), Err(Refusal::Reserved("Copy selected text"))));
        assert!(from_press(Key::F5, Modifiers::NONE, false).is_ok());
        // Keys the document view reads on its own, before any command could have them.
        for (key, m) in [
            (Key::V, Modifiers::NONE),
            (Key::H, Modifiers::NONE),
            (Key::Home, Modifiers::NONE),
            (Key::End, Modifiers::NONE),
            (Key::ArrowLeft, Modifiers::NONE),
            (Key::ArrowDown, Modifiers::SHIFT),
            (Key::PageDown, Modifiers::NONE),
            (Key::PageUp, ctrl),
            (Key::Delete, Modifiers::NONE),
        ] {
            assert!(matches!(from_press(key, m, false), Err(Refusal::Reserved(_))), "{key:?} {m:?}");
        }
        assert!(from_press(Key::V, Modifiers::SHIFT, false).is_ok() && from_press(Key::Home, ctrl, false).is_ok());
        assert!(!allowed(Shortcut { command: false, shift: false, mac_ctrl: false, key: "Home" }), "nor from the settings");
        // On macOS Control is ⌃, a modifier of its own.
        let mac = from_press(Key::H, Modifiers::MAC_CMD | Modifiers::COMMAND | Modifiers::CTRL, true).unwrap();
        assert!(mac.command && mac.mac_ctrl);
    }

    #[test]
    fn assigning_a_key_takes_it_from_the_command_that_had_it() {
        let mut o = Overrides::default();
        let save = pdfcraft_engine::commands::command("file.save").unwrap();
        let open = pdfcraft_engine::commands::command("file.open").unwrap();
        let (save_key, open_key) = (save.shortcut.unwrap(), open.shortcut.unwrap());
        let taken = o.assign("file.open", save_key).map(|c| c.id);
        assert_eq!(taken, Some("file.save"));
        assert!(o.get(open).is_some_and(|k| same(k, save_key)) && o.get(save).is_none());
        assert_eq!(o.holder(open_key).map(|c| c.id), None, "Open's old key is free now");
        // Reset gives Save its key back, and Open loses it again.
        assert_eq!(o.reset("file.save").map(|c| c.id), Some("file.open"));
        assert!(o.get(save).is_some_and(|k| same(k, save_key)) && o.get(open).is_none());
        o.reset("file.open");
        assert_eq!(o, Overrides::default(), "both back to their defaults leave nothing recorded");
        o.remove("file.save");
        assert!(o.get(save).is_none() && o.is_custom("file.save"));
        o.reset_all();
        assert!(o.get(save).is_some());
        assert_eq!(o.assign("no.such.command", cmd("J")).map(|c| c.id), None);
        assert_eq!(o, Overrides::default());
        // Copy keeps the system's Ctrl/⌘C: it can't be given another key, lose it, or be
        // changed from the settings (CodeRabbit on #4646).
        o.assign("edit.copy", cmd("J"));
        o.remove("edit.copy");
        assert_eq!(o, Overrides::default());
        assert_eq!(Overrides::from_json(&serde_json::json!({ "edit.copy": "cmd+J" })), Overrides::default());
    }

    #[test]
    fn settings_round_trip_and_damaged_ones_are_read_leniently() {
        let mut o = Overrides::default();
        o.assign("file.open", Shortcut::cmd_shift("J"));
        o.remove("file.save");
        assert_eq!(Overrides::from_json(&o.to_json()), o);
        let v = serde_json::json!({
            "no.such.command": "cmd+J",
            "file.open": "alt+J",
            "print.dialog": "cmd+C",
            "file.close": 7,
            "file.save": "cmd+shift+J",
            "file.save_as": "cmd+shift+J",
        });
        let read = Overrides::from_json(&v);
        let get = |id: &str| read.get(pdfcraft_engine::commands::command(id).unwrap());
        assert!(same(get("file.save").unwrap(), Shortcut::cmd_shift("J")));
        assert!(get("file.save_as").is_none_or(|k| !same(k, Shortcut::cmd_shift("J"))), "a key runs one command");
        for id in ["file.open", "print.dialog", "file.close"] {
            assert_eq!(get(id), pdfcraft_engine::commands::command(id).unwrap().shortcut, "{id} keeps its default");
        }
        assert_eq!(Overrides::from_json(&serde_json::json!([1, 2])), Overrides::default());
        // Many unknown commands sorting first don't push a real one out.
        let mut crowded: serde_json::Map<String, serde_json::Value> =
            (0..COMMANDS.len() + 5).map(|i| (format!("a.unknown{i:04}"), serde_json::json!("cmd+J"))).collect();
        crowded.insert("file.open".into(), serde_json::json!("cmd+shift+J"));
        let read = Overrides::from_json(&serde_json::Value::Object(crowded));
        assert!(read.get(pdfcraft_engine::commands::command("file.open").unwrap()).is_some_and(|k| same(k, Shortcut::cmd_shift("J"))));
        // A kept key that is another command's default takes it from that command.
        let save_key = pdfcraft_engine::commands::command("file.save").unwrap().shortcut.unwrap();
        let read = Overrides::from_json(&serde_json::json!({ "file.open": token(save_key) }));
        assert!(read.get(pdfcraft_engine::commands::command("file.save").unwrap()).is_none());
    }

    #[test]
    fn menus_and_tooltips_name_the_published_keys() {
        let ctx = egui::Context::default();
        crate::i18n::set_current(crate::i18n::Lang::from_pref("en"));
        assert_eq!(crate::commands::command_shortcut_label(&ctx, "view.palette"), "Ctrl+K", "nothing published: the default");
        let mut o = Overrides::default();
        o.assign("view.palette", Shortcut::cmd_shift("J"));
        o.remove("file.save");
        publish(&ctx, &Arc::new(o));
        assert_eq!(crate::commands::command_shortcut_label(&ctx, "view.palette"), "Ctrl+Shift+J");
        assert_eq!(crate::commands::command_shortcut_label(&ctx, "file.save"), "", "a removed key isn't shown");
        let palette = pdfcraft_engine::commands::command("view.palette").unwrap();
        assert!(effective(&ctx, palette).is_some_and(|s| same(s, Shortcut::cmd_shift("J"))));
    }

    #[test]
    fn the_registry_has_no_key_twice() {
        let o = Overrides::default();
        for spec in COMMANDS {
            let Some(s) = spec.shortcut else { continue };
            assert_eq!(o.holder(s).map(|c| c.id), Some(spec.id), "{} shares {}", spec.id, token(s));
        }
    }
}
