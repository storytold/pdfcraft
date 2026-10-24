//! The macOS menu bar (`menu_bar`'s items) as native menus, with muda.
//!
//! Built on the first frame (on the main thread, after AppKit has finished launching, so it
//! replaces winit's default menu) and rebuilt when the interface language changes. Clicks and
//! key equivalents arrive on a channel; `raw_input_hook` turns keys and Cut / Copy / Paste /
//! Select All into the input events the keyboard would have produced, and `logic` runs
//! everything else.

use std::sync::mpsc::{Receiver, Sender, channel};

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use pdfcraft_engine::commands::Shortcut;

use crate::PdfCraftApp;
use crate::menu_bar::{self, Action, Entry, Item, Role, System};

/// The key equivalent a registry shortcut shows as (`menu_bar::key_code`).
pub(crate) fn accelerator(s: Shortcut) -> Option<Accelerator> {
    let code: Code = menu_bar::key_code(s)?.parse().ok()?;
    let mut mods = Modifiers::empty();
    if s.command {
        mods |= Modifiers::META;
    }
    if s.shift {
        mods |= Modifiers::SHIFT;
    }
    if s.mac_ctrl {
        mods |= Modifiers::CONTROL;
    }
    Some(Accelerator::new(mods, code))
}

/// A native item and the state it shows, so a frame only calls AppKit for what changed.
enum Native {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

struct Tracked {
    native: Native,
    item: Item,
    enabled: bool,
    label: String,
}

impl Tracked {
    fn id(&self) -> &muda::MenuId {
        match &self.native {
            Native::Plain(i) => i.id(),
            Native::Check(i) => i.id(),
        }
    }
}

pub(crate) struct NativeMenu {
    /// Kept alive: AppKit holds the menu only while muda's handle exists.
    _menu: Menu,
    /// Every submenu, kept alive for the same reason.
    _submenus: Vec<Submenu>,
    events: Receiver<(MenuEvent, bool)>,
    tracked: Vec<Tracked>,
    recent: Option<Submenu>,
    /// Open Recent's items: what each does.
    recent_actions: Vec<(muda::MenuId, Action)>,
    recent_paths: Option<Vec<String>>,
    seq: u64,
    /// The interface language (its code) the labels are in.
    lang: &'static str,
    /// Every menu, or only `menu_bar::system_only`'s.
    full: bool,
}

/// Appending only fails for an item already in the menu, which this module never does; a
/// failure just leaves the item out.
fn append(menu: &Submenu, item: &dyn IsMenuItem) {
    if let Err(e) = menu.append(item) {
        log::warn!("native menu: {e}");
    }
}

struct Builder {
    tracked: Vec<Tracked>,
    submenus: Vec<Submenu>,
    recent: Option<Submenu>,
    seq: u64,
}

impl Builder {
    fn next_id(&mut self) -> String {
        self.seq = self.seq.saturating_add(1);
        format!("pdfcraft.{}", self.seq)
    }

    fn fill(&mut self, menu: &Submenu, entries: Vec<Entry>) {
        for entry in entries {
            match entry {
                Entry::Separator => append(menu, &PredefinedMenuItem::separator()),
                Entry::Item(item) => {
                    let id = self.next_id();
                    let accel = item.shortcut.and_then(accelerator);
                    let native = match item.check {
                        Some(_) => {
                            let c = CheckMenuItem::with_id(id, &item.label, true, false, accel);
                            append(menu, &c);
                            Native::Check(c)
                        }
                        None => {
                            let i = MenuItem::with_id(id, &item.label, true, accel);
                            append(menu, &i);
                            Native::Plain(i)
                        }
                    };
                    self.tracked.push(Tracked { native, label: item.label.clone(), item, enabled: true });
                }
                Entry::Submenu(title, sub) => {
                    let s = Submenu::new(title, true);
                    self.fill(&s, sub);
                    append(menu, &s);
                    self.submenus.push(s);
                }
                Entry::OpenRecent(title) => {
                    let s = Submenu::new(title, false);
                    append(menu, &s);
                    self.recent = Some(s);
                }
                Entry::System(kind, label) => {
                    let l = Some(label.as_str());
                    let item = match kind {
                        System::Services => PredefinedMenuItem::services(l),
                        System::Hide => PredefinedMenuItem::hide(l),
                        System::HideOthers => PredefinedMenuItem::hide_others(l),
                        System::ShowAll => PredefinedMenuItem::show_all(l),
                        System::Minimize => PredefinedMenuItem::minimize(l),
                        System::Zoom => PredefinedMenuItem::maximize(l),
                        System::FullScreen => PredefinedMenuItem::fullscreen(l),
                        System::BringAllToFront => PredefinedMenuItem::bring_all_to_front(l),
                    };
                    append(menu, &item);
                }
            }
        }
    }
}

impl NativeMenu {
    /// Build the menu bar and install it: every menu when `full`, else `menu_bar::system_only`'s.
    /// `None` off the main thread (tests, headless hosts). `events` is the previous bar's channel:
    /// muda takes the click handler only once per process, so a rebuilt bar keeps receiving on
    /// the first one.
    pub(crate) fn install(ctx: &egui::Context, events: Option<Receiver<(MenuEvent, bool)>>, full: bool) -> Option<Self> {
        objc2::MainThreadMarker::new()?;
        let events = events.unwrap_or_else(|| {
            let (tx, rx) = channel::<(MenuEvent, bool)>();
            set_handler(tx, ctx.clone());
            rx
        });
        let mut b = Builder { tracked: Vec::new(), submenus: Vec::new(), recent: None, seq: 0 };
        let menu = Menu::new();
        let mut roles = Vec::new();
        let menus = if full { menu_bar::bar() } else { menu_bar::system_only(menu_bar::bar()) };
        for m in menus {
            let s = Submenu::new(&m.title, true);
            b.fill(&s, m.entries);
            if let Err(e) = menu.append(&s) {
                log::warn!("native menu: {e}");
            }
            roles.push((m.role, s.clone()));
            b.submenus.push(s);
        }
        menu.init_for_nsapp();
        for (role, s) in &roles {
            match role {
                Role::Window => s.set_as_windows_menu_for_nsapp(),
                Role::Help => s.set_as_help_menu_for_nsapp(),
                Role::App | Role::Plain => {}
            }
        }
        Some(Self {
            _menu: menu,
            _submenus: b.submenus,
            events,
            tracked: b.tracked,
            recent: b.recent,
            recent_actions: Vec::new(),
            recent_paths: None,
            seq: b.seq,
            lang: crate::i18n::current().code(),
            full,
        })
    }

    /// The actions chosen since the last call. A pressed key equivalent comes back as what that
    /// key does (`Item::key_action`).
    pub(crate) fn take_actions(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        while let Ok((e, from_key)) = self.events.try_recv() {
            if let Some(t) = self.tracked.iter().find(|t| t.id() == e.id()) {
                out.push(t.item.key_action().filter(|_| from_key).unwrap_or_else(|| t.item.action.clone()));
            } else if let Some((_, a)) = self.recent_actions.iter().find(|(id, _)| id == e.id()) {
                out.push(a.clone());
            }
        }
        out
    }

    /// Whether the interface language, or where the menus go (`full`), changed since the bar was built.
    pub(crate) fn stale(&self, full: bool) -> bool {
        self.lang != crate::i18n::current().code() || self.full != full
    }

    /// Bring labels, enablement, check marks and Open Recent in line with the app.
    pub(crate) fn sync(&mut self, app: &PdfCraftApp, text_focused: bool) {
        for t in &mut self.tracked {
            let state = app.menu_item_state(&t.item, text_focused);
            if state.enabled != t.enabled {
                match &t.native {
                    Native::Plain(i) => i.set_enabled(state.enabled),
                    Native::Check(i) => i.set_enabled(state.enabled),
                }
                t.enabled = state.enabled;
            }
            let label = state.label.unwrap_or_else(|| t.item.label.clone());
            if label != t.label {
                match &t.native {
                    Native::Plain(i) => i.set_text(&label),
                    Native::Check(i) => i.set_text(&label),
                }
                t.label = label;
            }
            // Read back, not remembered: AppKit flips a check item on click whether or not the
            // command took effect.
            if let (Native::Check(i), Some(checked)) = (&t.native, state.checked)
                && i.is_checked() != checked
            {
                i.set_checked(checked);
            }
        }
        let Some(recent) = &self.recent else { return };
        let paths: Vec<String> = app.recent.iter().map(|r| r.path.clone()).collect();
        if self.recent_paths.as_ref() == Some(&paths) {
            return;
        }
        // Bounded: each pass removes one item of a finite submenu.
        while recent.remove_at(0).is_some() {}
        self.recent_actions.clear();
        let mut next_id = || {
            self.seq = self.seq.saturating_add(1);
            format!("pdfcraft.recent.{}", self.seq)
        };
        for r in &app.recent {
            let label = if r.name.is_empty() { r.path.clone() } else { r.name.clone() };
            let item = MenuItem::with_id(next_id(), label, true, None);
            self.recent_actions.push((item.id().clone(), Action::Recent(r.path.clone())));
            append(recent, &item);
        }
        if !app.recent.is_empty() {
            append(recent, &PredefinedMenuItem::separator());
            let clear = MenuItem::with_id(next_id(), tl!("Clear Recent Files"), true, None);
            self.recent_actions.push((clear.id().clone(), Action::Command("file.clear_recent")));
            append(recent, &clear);
        }
        recent.set_enabled(!app.recent.is_empty());
        self.recent_paths = Some(paths);
    }
}

/// Clicks and key equivalents go to `tx`, and wake the event loop so the next frame sees them.
fn set_handler(tx: Sender<(MenuEvent, bool)>, ctx: egui::Context) {
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = tx.send((e, from_key()));
        ctx.request_repaint();
    }));
}

/// Whether the event AppKit is handling (the one that chose the item) is a key press.
fn from_key() -> bool {
    use objc2_app_kit::{NSApplication, NSEventType};
    let Some(mtm) = objc2::MainThreadMarker::new() else { return false };
    NSApplication::sharedApplication(mtm).currentEvent().is_some_and(|e| e.r#type() == NSEventType::KeyDown)
}

impl PdfCraftApp {
    /// Before input: chosen items that are keyboard input join this frame's events; the rest
    /// wait for `logic`.
    pub(crate) fn poll_native_menu(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(menu) = self.native_menu.as_mut() else { return };
        let text_focused = ctx.text_edit_focused();
        for action in menu.take_actions() {
            match Self::menu_action_for_text(action, text_focused) {
                Action::Input(kind) => {
                    let clipboard = || arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
                    raw.events.extend(menu_bar::input_events(kind, clipboard));
                }
                Action::Key(s) => raw.events.extend(menu_bar::shortcut_events(s)),
                a => self.native_actions.push(a),
            }
        }
    }

    /// Each frame: build (or, after a language change or the menus moving, rebuild) the bar, run
    /// what was chosen, and bring labels, enablement and check marks up to date.
    pub(crate) fn native_menu_frame(&mut self, ctx: &egui::Context) {
        if !self.native_menu_bar {
            return;
        }
        let full = self.menus_in_system_bar();
        if self.native_menu.as_ref().is_none_or(|m| m.stale(full)) {
            let events = self.native_menu.take().map(|m| m.events);
            self.native_menu = NativeMenu::install(ctx, events, full);
        }
        for action in std::mem::take(&mut self.native_actions) {
            self.run_menu_action(action);
        }
        if let Some(mut menu) = self.native_menu.take() {
            menu.sync(self, ctx.text_edit_focused());
            self.native_menu = Some(menu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_equivalent_is_a_key_muda_knows() {
        for m in menu_bar::bar() {
            for item in menu_bar::items(&m.entries) {
                if let Some(s) = item.shortcut {
                    let code = menu_bar::key_code(s);
                    assert!(code.is_none_or(|c| c.parse::<Code>().is_ok()), "{}: {code:?}", item.label);
                }
            }
        }
    }

    #[test]
    fn shortcuts_become_key_equivalents_with_their_modifiers() {
        let s = |key, command, shift, mac_ctrl| Shortcut { command, shift, mac_ctrl, key };
        assert_eq!(accelerator(s("O", true, false, false)), Some(Accelerator::new(Modifiers::META, Code::KeyO)));
        assert_eq!(accelerator(s("S", true, true, false)), Some(Accelerator::new(Modifiers::META | Modifiers::SHIFT, Code::KeyS)));
        assert_eq!(accelerator(s(",", true, false, false)), Some(Accelerator::new(Modifiers::META, Code::Comma)));
        assert_eq!(accelerator(s("Tab", false, false, true)), Some(Accelerator::new(Modifiers::CONTROL, Code::Tab)));
        assert_eq!(accelerator(s("+", true, false, false)), Some(Accelerator::new(Modifiers::META, Code::Equal)));
        assert_eq!(accelerator(s("−", true, true, false)), Some(Accelerator::new(Modifiers::META | Modifiers::SHIFT, Code::Minus)));
        // Keys egui keeps to itself, and bare keys that would fire while typing.
        assert_eq!(accelerator(s("Delete", true, false, false)), None);
        assert_eq!(accelerator(s("H", false, false, false)), None);
    }
}
