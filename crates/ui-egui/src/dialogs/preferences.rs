//! Preferences: the draft the window edits, the model that maps its rows to the app, and the live
//! effects an OK applies.

use std::sync::{Arc, Mutex};

use craft::theme::color::{Rgba, Srgba};
use craft::theme::selection::MotionChoice;
use craft::theme::variant::Tone;
// PLAN: assumed `craft::theme_store::{selection::SelectionChange, ui::AppearancePage}` (theme-store L-10)
use craft::theme_store::selection::SelectionChange;
use craft::theme_store::ui::AppearancePage;
use craft::ui::contract::{Bind, Entry, Props as _, SaveKey, Show as _};
use craft::ui::dialog::DialogHost;
use craft::ui::notices::{Notice, NoticeLayout};
use craft::ui::patterns::preferences::{GroupSpec, PageSpec, PrefValue, PreferencesModel, PreferencesState, PreferencesWindow, RowKind, RowSpec};
use craft::ui::runtime::env::Label;
use craft::ui::text::TextField;
use egui::Ui;

use crate::PdfCraftApp;
use crate::canvas::{Fit, PageLayout, ViewDefaults};
use crate::i18n::{self, Lang};
use crate::scroll_speed::ScrollSpeed;
use crate::state::Mode;
use crate::ui_scale;
use crate::units::Unit;

/// What the open Preferences window keeps between passes; `PdfCraftApp::preferences` holds it while
/// the window is open.
pub struct PreferencesSession {
    state: PreferencesState,
    draft: PrefsDraft,
    selection: SelectionChange,
    /// The dialogs opened from inside the window (the colour picker), run in its own viewport.
    host: DialogHost,
    /// The colours the picker answered, waiting for `Prefs::nested` to write them into the draft.
    answers: Answers,
}

/// Colour answers by row key, filled by the picker's `then` and drained in the same window's pass.
type Answers = Arc<Mutex<Vec<(String, Srgba)>>>;

impl PreferencesSession {
    fn new(app: &PdfCraftApp) -> Self {
        Self {
            state: PreferencesState::default(),
            draft: PrefsDraft::from_app(app),
            selection: SelectionChange::default(),
            host: DialogHost::new(),
            answers: Answers::default(),
        }
    }
}

/// Every setting the window edits, copied from the app when the window opens. Nothing reaches the
/// app before OK or Apply.
#[derive(Clone, PartialEq)]
struct PrefsDraft {
    language: String,
    ui_scale: Option<f32>,
    tools_right: bool,
    rail_right: bool,
    animation: MotionChoice,
    reopen_last_session: bool,
    restore_last_view: bool,
    reload_changed_files: bool,
    update_check_at_start: bool,
    show_secured_notice: bool,
    recent_limit: usize,
    system_title_bar: bool,
    show_page_edge_shadows: bool,
    reading_colors: pdfcraft_render::ReadingColors,
    default_mode: Mode,
    view_defaults: ViewDefaults,
    always_default_view: bool,
    text_links: bool,
    units: Unit,
    scroll_speed: ScrollSpeed,
    invert_middle_drag: bool,
    author: String,
    hide_empty_highlight_hover: bool,
    flatten_fill_sign_on_save: bool,
    ask_to_lock_signatures: bool,
    date_format: String,
    date_language: Option<String>,
    javascript: bool,
}

impl PrefsDraft {
    fn from_app(app: &PdfCraftApp) -> Self {
        Self {
            language: app.language.clone(),
            ui_scale: app.ui_scale,
            tools_right: app.side_panels.tools_on_right(),
            rail_right: app.side_panels.rail_on_right(),
            animation: app.animation_effects,
            reopen_last_session: app.reopen_last_session,
            restore_last_view: app.restore_last_view,
            reload_changed_files: app.reload_changed_files,
            update_check_at_start: app.update_check_at_start,
            show_secured_notice: app.show_secured_notice,
            recent_limit: app.recent_limit,
            system_title_bar: app.system_title_bar,
            show_page_edge_shadows: app.show_page_edge_shadows,
            reading_colors: app.reading_colors,
            default_mode: app.default_mode,
            view_defaults: app.view_defaults,
            always_default_view: app.always_default_view,
            text_links: app.text_links,
            units: app.units,
            scroll_speed: app.scroll_speed,
            invert_middle_drag: app.invert_middle_drag,
            author: app.comment_prefs.author.clone(),
            hide_empty_highlight_hover: app.comment_prefs.hide_empty_highlight_hover,
            flatten_fill_sign_on_save: app.flatten_fill_sign_on_save,
            ask_to_lock_signatures: app.ask_to_lock_signatures,
            date_format: app.session.date_format().to_string(),
            date_language: app.session.date_language().map(str::to_string),
            javascript: app.session.javascript(),
        }
    }

    /// The draft a fresh install starts with, read from the settings defaults through the mapping
    /// `apply_settings` uses, so no second list of defaults exists.
    // PLAN: assumed `PdfCraftSettings::default()` with the app's field names (pdf-shell-w9)
    fn defaults() -> Self {
        let s = crate::settings::PdfCraftSettings::default();
        Self {
            language: s.language,
            ui_scale: s.ui_scale,
            tools_right: s.tools_right,
            rail_right: s.rail_right,
            animation: s.animation_effects,
            reopen_last_session: s.reopen_last_session,
            restore_last_view: s.restore_last_view,
            reload_changed_files: s.reload_changed_files,
            update_check_at_start: s.update_check_at_start,
            show_secured_notice: s.show_secured_notice,
            recent_limit: s.recent_limit,
            system_title_bar: s.system_title_bar,
            show_page_edge_shadows: s.show_page_edge_shadows,
            reading_colors: s.reading_colors,
            default_mode: s.default_mode,
            view_defaults: s.view_defaults,
            always_default_view: s.always_default_view,
            text_links: s.text_links,
            units: s.units,
            scroll_speed: s.scroll_speed,
            invert_middle_drag: s.invert_middle_drag,
            author: s.author,
            hide_empty_highlight_hover: s.hide_empty_highlight_hover,
            flatten_fill_sign_on_save: s.flatten_fill_sign_on_save,
            ask_to_lock_signatures: s.ask_to_lock_signatures,
            date_format: s.date_format,
            date_language: s.date_language,
            javascript: s.javascript,
        }
    }
}

impl PdfCraftApp {
    /// Opens Preferences, or leaves the open window as it is.
    pub fn open_preferences(&mut self) {
        if self.preferences.is_none() {
            self.preferences = Some(PreferencesSession::new(self));
        }
    }

    /// Shows the Preferences window for this pass while it is open, and closes it when answered.
    pub(crate) fn show_preferences(&mut self, ui: &mut Ui) {
        let Some(mut session) = self.preferences.take() else {
            return;
        };
        let PreferencesSession { state, draft, selection, host, answers } = &mut session;
        let output = {
            let mut prefs = Prefs::new(self, draft, selection, host, answers, ui.ctx().native_pixels_per_point().unwrap_or(1.0));
            PreferencesWindow::new(Bind::bound(state)).remember(SaveKey::new("pdfcraft.preferences")).show(ui, &mut prefs)
        };
        if let Some(request) = output.pick_color {
            let answers = Arc::clone(answers);
            let key = request.key.clone();
            // PLAN: assumed `crate::comments::open_colour_picker(&mut DialogHost, ColorRequest, impl FnOnce(Srgba) + 'static)` (pdf-comments-w10, L-35, q180)
            crate::comments::open_colour_picker(host, request, move |colour| {
                if let Ok(mut queue) = answers.lock() {
                    queue.push((key, colour));
                }
            });
        }
        if output.closed.is_none() {
            self.preferences = Some(session);
        }
    }
}

/// Strings the pages borrow, built once per pass from the app and the draft.
struct Texts {
    auto_scale: String,
    scale_ids: Vec<(String, String)>,
    date_samples: Vec<String>,
    date_follow: String,
    today: Result<String, String>,
    unwritable: String,
    date_help: String,
    date_custom_id: &'static str,
}

impl Texts {
    fn new(app: &PdfCraftApp, draft: &PrefsDraft, native_pixels: f32) -> Self {
        use pdfcraft_engine::dates::DATE_FORMATS;
        let auto_percent = ui_scale::percent(ui_scale::drawn(None, app.system_text_scale, native_pixels));
        let sample = |f: &str| format!("{}   {f}", app.date_text(Some(f)).unwrap_or_default());
        let interface = i18n::current().picker_name(|_| true);
        let bad = app.date_text(None).map(|today| pdfcraft_engine::dates::unwritable(&today)).unwrap_or_default();
        let unwritable = if bad.is_empty() {
            String::new()
        } else {
            i18n::fmt(
                tl!("{chars} can't be written into the PDF yet: Fill & Sign text is Western European only. Pick a numeric format such as dd.mm.yyyy."),
                &[("chars", &bad)],
            )
        };
        Self {
            auto_scale: i18n::fmt(tl!("Auto ({percent}%)"), &[("percent", &auto_percent.to_string())]),
            scale_ids: ui_scale::PRESETS.iter().map(|p| (p.to_string(), format!("{p}%"))).collect(),
            date_samples: DATE_FORMATS.iter().map(|f| sample(f)).collect(),
            date_follow: i18n::fmt(tl!("Same as interface ({language})"), &[("language", interface)]),
            today: app.date_text(Some(&draft.date_format)),
            unwritable,
            date_help: i18n::fmt(
                tl!(
                    "yyyy or yy year · m or mm month · mmm or mmmm month name · d or dd day · ddd or dddd weekday. H, h, M, s and t are time letters and can't be used on their own: put {escape} before a letter to show it as is."
                ),
                &[("escape", "\\")],
            ),
            date_custom_id: "custom",
        }
    }
}

/// The id a choice row reports for the "no value" option of the date language and the interface size.
const AUTO_ID: &str = "auto";
/// The date language option that follows the interface language.
const FOLLOW_ID: &str = "";

/// The app's side of the window: maps rows to the draft and the draft to the app.
struct Prefs<'a> {
    app: &'a mut PdfCraftApp,
    draft: &'a mut PrefsDraft,
    selection: &'a mut SelectionChange,
    host: &'a mut DialogHost,
    answers: &'a Answers,
    texts: Texts,
}

impl<'a> Prefs<'a> {
    fn new(
        app: &'a mut PdfCraftApp,
        draft: &'a mut PrefsDraft,
        selection: &'a mut SelectionChange,
        host: &'a mut DialogHost,
        answers: &'a Answers,
        native_pixels: f32,
    ) -> Self {
        let texts = Texts::new(app, draft, native_pixels);
        Self { app, draft, selection, host, answers, texts }
    }
}

type Table<'a, T> = Vec<(T, &'static str, &'a str)>;

/// A choice row over `table` (value, id, label) with `current` chosen.
fn choice_row<'a, T: Copy + PartialEq>(key: &'a str, label: &'a str, current: T, table: &Table<'a, T>) -> RowSpec<'a> {
    let value = table.iter().find(|(v, ..)| *v == current).map_or("", |(_, id, _)| *id);
    RowSpec::new(key, label, RowKind::choice(value, table.iter().map(|(_, id, l)| Entry::new(*id, *l)).collect()))
}

/// The value a choice row's `id` stands for.
fn lookup<T: Copy>(table: &Table<'_, T>, id: &str) -> Option<T> {
    table.iter().find(|(_, i, _)| *i == id).map(|(v, ..)| *v)
}

fn side_table<'a>() -> Table<'a, bool> {
    vec![(false, "left", tl!("Left")), (true, "right", tl!("Right"))]
}

fn animation_table<'a>() -> Table<'a, MotionChoice> {
    vec![
        (MotionChoice::System, "system", tl_ctx!("animation effects", "System")),
        (MotionChoice::Full, "on", tl_ctx!("animation effects", "On")),
        (MotionChoice::Off, "off", tl_ctx!("animation effects", "Off")),
    ]
}

fn mode_table<'a>() -> Table<'a, Mode> {
    vec![
        (Mode::AllTools, "all", tl!("All tools")),
        (Mode::Read, "read", tl!("Read")),
        (Mode::Edit, "edit", tl!("Edit")),
        (Mode::Convert, "convert", tl!("Convert")),
        (Mode::Sign, "sign", tl!("E-Sign")),
    ]
}

fn layout_table<'a>() -> Table<'a, PageLayout> {
    vec![(PageLayout::Single, "single", tl!("Single page")), (PageLayout::TwoUp, "two-up", tl!("Two-page view"))]
}

fn fit_table<'a>() -> Table<'a, Fit> {
    vec![(Fit::Width, "width", tl!("Fit to width")), (Fit::Page, "page", tl!("Zoom to page level")), (Fit::None, "custom", tl!("Custom"))]
}

fn unit_table<'a>() -> Table<'a, Unit> {
    vec![
        (Unit::Inches, "in", Unit::Inches.label()),
        (Unit::Millimetres, "mm", Unit::Millimetres.label()),
        (Unit::Centimetres, "cm", Unit::Centimetres.label()),
        (Unit::Points, "pt", Unit::Points.label()),
    ]
}

fn speed_table<'a>() -> Table<'a, ScrollSpeed> {
    vec![
        (ScrollSpeed::Slow, "slow", tl_ctx!("scroll speed", ScrollSpeed::Slow.label())),
        (ScrollSpeed::Normal, "normal", tl_ctx!("scroll speed", ScrollSpeed::Normal.label())),
        (ScrollSpeed::Fast, "fast", tl_ctx!("scroll speed", ScrollSpeed::Fast.label())),
    ]
}

/// The reading-colour presets by the id `ReadingColors::mode_name` gives them.
fn reading_presets() -> [(&'static str, &'static str, pdfcraft_render::ReadingColors); 5] {
    [
        ("off", tl!("Off"), pdfcraft_render::ReadingColors::default()),
        ("black-on-white", tl!("Black on white"), pdfcraft_render::ReadingColors::BLACK_ON_WHITE),
        ("white-on-black", tl!("White on black"), pdfcraft_render::ReadingColors::WHITE_ON_BLACK),
        ("yellow-on-black", tl!("Yellow on black"), pdfcraft_render::ReadingColors::YELLOW_ON_BLACK),
        ("green-on-black", tl!("Green on black"), pdfcraft_render::ReadingColors::GREEN_ON_BLACK),
    ]
}

fn rgb(c: [u8; 3]) -> Rgba {
    Rgba([c[0], c[1], c[2], 255])
}

/// The name a date-language option shows: the language's own name, as the interface picker does.
fn date_language_picker_name(l: &pdfcraft_engine::dates::DateLanguage) -> &'static str {
    Lang::from_code(l.code).map_or(l.name, |lang| lang.picker_name(|_| true))
}

impl PreferencesModel for Prefs<'_> {
    fn pages(&self) -> Vec<PageSpec<'_>> {
        vec![self.general_page(), self.appearance_page(), self.documents_page(), self.comment_page(), self.fill_sign_page(), self.javascript_page()]
    }

    fn set(&mut self, key: &str, value: PrefValue) {
        let d = &mut *self.draft;
        match (key, value) {
            ("general.language", PrefValue::Choice(id)) => {
                d.language = id;
                // Previews the language; `cancel` installs the saved one again.
                i18n::set_current(Lang::from_pref(&d.language));
            }
            ("general.scale", PrefValue::Choice(id)) => d.ui_scale = id.parse::<u16>().ok().map(|p| f32::from(p) / 100.0),
            ("general.tools_side", PrefValue::Choice(id)) => d.tools_right = lookup(&side_table(), &id).unwrap_or(d.tools_right),
            ("general.rail_side", PrefValue::Choice(id)) => d.rail_right = lookup(&side_table(), &id).unwrap_or(d.rail_right),
            ("general.animation", PrefValue::Choice(id)) => d.animation = lookup(&animation_table(), &id).unwrap_or(d.animation),
            ("general.reopen", PrefValue::Bool(v)) => d.reopen_last_session = v,
            ("general.restore_view", PrefValue::Bool(v)) => d.restore_last_view = v,
            ("general.reload", PrefValue::Bool(v)) => d.reload_changed_files = v,
            ("general.updates", PrefValue::Bool(v)) => d.update_check_at_start = v,
            ("general.secured_notice", PrefValue::Bool(v)) => d.show_secured_notice = v,
            ("general.recent", PrefValue::Int(v)) => d.recent_limit = usize::try_from(v).unwrap_or(0).min(crate::RECENT_FILES_MAX),
            ("general.title_bar", PrefValue::Bool(v)) => d.system_title_bar = v,
            ("appearance.edge_shadows", PrefValue::Bool(v)) => d.show_page_edge_shadows = v,
            ("appearance.highlight_fields", PrefValue::Bool(v)) => d.view_defaults.highlight_fields = v,
            ("appearance.reading.mode", PrefValue::Choice(id)) => {
                if id == "custom" {
                    if d.reading_colors.params.is_none() {
                        d.reading_colors = pdfcraft_render::ReadingColors::BLACK_ON_WHITE;
                    }
                } else if let Some((.., colors)) = reading_presets().into_iter().find(|(name, ..)| *name == id) {
                    d.reading_colors = colors;
                }
            }
            ("appearance.reading.text", PrefValue::Color(c)) => {
                if let Some(p) = d.reading_colors.params.as_mut() {
                    p.text = [c.0[0], c.0[1], c.0[2]];
                }
            }
            ("appearance.reading.background", PrefValue::Color(c)) => {
                if let Some(p) = d.reading_colors.params.as_mut() {
                    p.background = [c.0[0], c.0[1], c.0[2]];
                }
            }
            ("appearance.reading.only_black", PrefValue::Bool(v)) => {
                if let Some(p) = d.reading_colors.params.as_mut() {
                    p.only_black = v;
                }
            }
            ("appearance.reading.line_art", PrefValue::Bool(v)) => {
                if let Some(p) = d.reading_colors.params.as_mut() {
                    p.line_art = v;
                }
            }
            ("view.mode", PrefValue::Choice(id)) => d.default_mode = lookup(&mode_table(), &id).unwrap_or(d.default_mode),
            ("view.layout", PrefValue::Choice(id)) => d.view_defaults.layout = lookup(&layout_table(), &id).unwrap_or(d.view_defaults.layout),
            ("view.continuous", PrefValue::Bool(v)) => d.view_defaults.continuous = v,
            ("view.gaps", PrefValue::Bool(v)) => d.view_defaults.show_page_gaps = v,
            ("view.fit", PrefValue::Choice(id)) => d.view_defaults.fit = lookup(&fit_table(), &id).unwrap_or(d.view_defaults.fit),
            ("view.zoom", PrefValue::Int(v)) => {
                d.view_defaults.fit = Fit::None;
                d.view_defaults.zoom = v.clamp(8, 6400) as f32 / 100.0;
            }
            ("view.always", PrefValue::Bool(v)) => d.always_default_view = v,
            ("view.text_links", PrefValue::Bool(v)) => d.text_links = v,
            ("view.units", PrefValue::Choice(id)) => d.units = lookup(&unit_table(), &id).unwrap_or(d.units),
            ("view.scroll_speed", PrefValue::Choice(id)) => d.scroll_speed = lookup(&speed_table(), &id).unwrap_or(d.scroll_speed),
            ("view.invert_middle", PrefValue::Bool(v)) => d.invert_middle_drag = v,
            ("comment.author", PrefValue::Text(v)) => d.author = v.chars().take(crate::MAX_AUTHOR_CHARS).collect(),
            ("comment.hide_hover", PrefValue::Bool(v)) => d.hide_empty_highlight_hover = v,
            ("fill.flatten", PrefValue::Bool(v)) => d.flatten_fill_sign_on_save = v,
            ("fill.ask_lock", PrefValue::Bool(v)) => d.ask_to_lock_signatures = v,
            ("fill.date_preset", PrefValue::Choice(id)) => {
                if id != self.texts.date_custom_id {
                    d.date_format = id;
                }
            }
            ("fill.date_language", PrefValue::Choice(id)) => d.date_language = (id != FOLLOW_ID).then_some(id),
            ("javascript.enable", PrefValue::Bool(v)) => d.javascript = v,
            _ => {}
        }
    }

    fn custom_row(&mut self, ui: &mut Ui, key: &str) {
        if key != "fill.date_pattern" {
            return;
        }
        let mut text = self.draft.date_format.clone();
        TextField::new(tl!("Pattern"), Bind::bound(&mut text)).limit(pdfcraft_engine::dates::MAX_DATE_FORMAT_CHARS).show(ui);
        self.draft.date_format = text;
        match &self.texts.today {
            Ok(today) => {
                let line = i18n::fmt(tl!("Today: {date}"), &[("date", today)]);
                Notice::new(Tone::Info, Label::from(line)).layout(NoticeLayout::Inline).show(ui);
            }
            Err(e) => {
                Notice::new(Tone::Bad, Label::from(e.clone())).layout(NoticeLayout::Inline).show(ui);
            }
        }
        if !self.texts.unwritable.is_empty() {
            Notice::new(Tone::Warn, Label::from(self.texts.unwritable.clone())).layout(NoticeLayout::Inline).show(ui);
        }
    }

    fn custom_page(&mut self, ui: &mut Ui, page: &str) {
        if page != "appearance" {
            return;
        }
        // PLAN: assumed `suite_view()`, `favorite_themes()`, `system_dark()` and `toggle_favorite_theme(ThemeId)` on the app (theme-store L-10, pdf-shell-w10)
        let suite = self.app.suite_view();
        let favorites = self.app.favorite_themes();
        let system_dark = self.app.system_dark();
        let shown = AppearancePage::new(&suite, Bind::bound(&mut *self.selection)).favorites(&favorites).text_size(false).system_dark(system_dark).show(ui);
        if let Some(theme) = shown.favorite_toggled {
            self.app.toggle_favorite_theme(theme);
        }
    }

    fn differs(&self) -> bool {
        *self.draft != PrefsDraft::from_app(self.app) || *self.selection != SelectionChange::default()
    }

    fn can_ok(&self) -> bool {
        self.texts.today.is_ok()
    }

    fn apply(&mut self) -> Result<(), String> {
        let d = self.draft.clone();
        self.app.date_text(Some(&d.date_format))?;
        let old_defaults = self.app.view_defaults;
        let old_always = self.app.always_default_view;

        // The date settings can still be refused by the session; they go first so a failure leaves the app unchanged.
        self.app.session.set_date_format(&d.date_format).map_err(|_| tl!("The date pattern can't be used.").to_string())?;
        self.app.session.set_date_language(d.date_language.as_deref()).map_err(|_| tl!("The date language can't be used.").to_string())?;

        self.app.language = d.language;
        i18n::set_current(Lang::from_pref(&self.app.language));
        self.app.ui_scale = d.ui_scale;
        // PLAN: assumed `SidePanels::set_tools_on_right(bool)` / `set_rail_on_right(bool)` writing the workspace edge and `rail_side` (pdf-chrome-w13)
        self.app.side_panels.set_tools_on_right(d.tools_right);
        self.app.side_panels.set_rail_on_right(d.rail_right);
        self.app.animation_effects = d.animation;
        self.app.reopen_last_session = d.reopen_last_session;
        self.app.restore_last_view = d.restore_last_view;
        self.app.reload_changed_files = d.reload_changed_files;
        self.app.update_check_at_start = d.update_check_at_start;
        self.app.show_secured_notice = d.show_secured_notice;
        self.app.recent_limit = d.recent_limit;
        self.app.recent.truncate(d.recent_limit);
        self.app.system_title_bar = d.system_title_bar;
        self.app.show_page_edge_shadows = d.show_page_edge_shadows;
        self.app.set_reading_colors(d.reading_colors);
        self.app.default_mode = d.default_mode;
        self.app.view_defaults = d.view_defaults;
        self.app.always_default_view = d.always_default_view;
        self.app.text_links = d.text_links;
        self.app.units = d.units;
        self.app.scroll_speed = d.scroll_speed;
        self.app.invert_middle_drag = d.invert_middle_drag;
        self.app.comment_prefs.author = d.author;
        self.app.comment_prefs.hide_empty_highlight_hover = d.hide_empty_highlight_hover;
        self.app.flatten_fill_sign_on_save = d.flatten_fill_sign_on_save;
        self.app.ask_to_lock_signatures = d.ask_to_lock_signatures;

        self.app.session.set_javascript(d.javascript);
        // PLAN: assumed `apply_selection(SelectionChange)` on the app, calling `SuiteClient::change` (theme-store L-10, pdf-shell-w10)
        self.app.apply_selection(std::mem::take(&mut *self.selection));

        let defaults = self.app.view_defaults;
        let changes = ViewPrefChanges {
            always_reset: old_always != self.app.always_default_view,
            layout: old_defaults.layout != defaults.layout,
            continuous: old_defaults.continuous != defaults.continuous,
            gaps: old_defaults.show_page_gaps != defaults.show_page_gaps,
            fit: old_defaults.fit != defaults.fit,
            zoom: old_defaults.zoom != defaults.zoom,
        };
        if self.app.always_default_view && changes.any() {
            for view in &mut self.app.views {
                apply_view_pref_changes(view, &defaults, changes);
            }
        } else if let Some(view) = self.app.active.and_then(|i| self.app.views.get_mut(i)) {
            apply_view_pref_changes(view, &defaults, ViewPrefChanges { always_reset: false, ..changes });
        }
        Ok(())
    }

    fn cancel(&mut self) {
        i18n::set_current(Lang::from_pref(&self.app.language));
        *self.selection = SelectionChange::default();
    }

    fn reset_page(&mut self, page: &str) {
        let fresh = PrefsDraft::defaults();
        let d = &mut *self.draft;
        match page {
            "general" => {
                d.language = fresh.language;
                i18n::set_current(Lang::from_pref(&d.language));
                d.ui_scale = fresh.ui_scale;
                d.tools_right = fresh.tools_right;
                d.rail_right = fresh.rail_right;
                d.animation = fresh.animation;
                d.reopen_last_session = fresh.reopen_last_session;
                d.restore_last_view = fresh.restore_last_view;
                d.reload_changed_files = fresh.reload_changed_files;
                d.update_check_at_start = fresh.update_check_at_start;
                d.show_secured_notice = fresh.show_secured_notice;
                d.recent_limit = fresh.recent_limit;
                d.system_title_bar = fresh.system_title_bar;
            }
            "appearance" => {
                *self.selection = SelectionChange::default();
                d.show_page_edge_shadows = fresh.show_page_edge_shadows;
                d.reading_colors = fresh.reading_colors;
                d.view_defaults.highlight_fields = fresh.view_defaults.highlight_fields;
            }
            "documents" => {
                d.default_mode = fresh.default_mode;
                d.view_defaults = ViewDefaults { highlight_fields: d.view_defaults.highlight_fields, ..fresh.view_defaults };
                d.always_default_view = fresh.always_default_view;
                d.text_links = fresh.text_links;
                d.units = fresh.units;
                d.scroll_speed = fresh.scroll_speed;
                d.invert_middle_drag = fresh.invert_middle_drag;
            }
            "comment" => {
                d.author = fresh.author;
                d.hide_empty_highlight_hover = fresh.hide_empty_highlight_hover;
            }
            "fill" => {
                d.flatten_fill_sign_on_save = fresh.flatten_fill_sign_on_save;
                d.ask_to_lock_signatures = fresh.ask_to_lock_signatures;
                d.date_format = fresh.date_format;
                d.date_language = fresh.date_language;
            }
            "javascript" => d.javascript = fresh.javascript,
            _ => {}
        }
    }

    fn nested(&mut self, ui: &mut Ui) {
        self.host.run(ui);
        let answered = self.answers.lock().map(|mut queue| std::mem::take(&mut *queue)).unwrap_or_default();
        for (key, colour) in answered {
            self.set_color(&key, colour.to_rgba());
        }
    }

    fn nested_open(&self) -> bool {
        !self.host.is_empty()
    }
}

impl Prefs<'_> {
    fn general_page(&self) -> PageSpec<'_> {
        let d = &*self.draft;
        let t = &self.texts;
        let mut languages = vec![Entry::new(i18n::AUTO, tl!("Auto"))];
        languages.extend(Lang::for_picker_by(|l| l.picker_name(|_| true)).into_iter().map(|l| Entry::new(l.code(), l.picker_name(|_| true))));
        let language = if Lang::from_code(&d.language).is_some() { d.language.as_str() } else { i18n::AUTO };
        let mut scales = vec![Entry::new(AUTO_ID, t.auto_scale.as_str())];
        scales.extend(t.scale_ids.iter().map(|(id, label)| Entry::new(id.as_str(), label.as_str())));
        let scale = d.ui_scale.map_or(AUTO_ID, |f| t.scale_ids.iter().find(|(id, _)| id.parse::<u32>().is_ok_and(|p| p == ui_scale::percent(f))).map_or(AUTO_ID, |(id, _)| id.as_str()));
        let mut rows = vec![
            RowSpec::new("general.language", tl!("Interface language"), RowKind::choice(language, languages)),
            RowSpec::new("general.scale", tl!("Interface size"), RowKind::choice(scale, scales))
                .help(tl!("Auto follows the system's text size. Pages always render at the display's full resolution.")),
            choice_row("general.tools_side", tl!("Tools panel side"), d.tools_right, &side_table()),
            choice_row("general.rail_side", tl!("Navigation rail side"), d.rail_right, &side_table()),
            choice_row("general.animation", tl!("Animation effects"), d.animation, &animation_table()),
        ];
        #[cfg(not(target_arch = "wasm32"))]
        {
            rows.push(RowSpec::new("general.reopen", tl!("Reopen the files that were open when PdfCraft last closed"), RowKind::Bool(d.reopen_last_session)));
            rows.push(RowSpec::new(
                "general.restore_view",
                tl!("Restore the last view (page, zoom and page display) when a document is opened again"),
                RowKind::Bool(d.restore_last_view),
            ));
            rows.push(RowSpec::new("general.reload", tl!("Reload documents when their files change on disk"), RowKind::Bool(d.reload_changed_files)));
            rows.push(RowSpec::new("general.updates", tl!("Check for updates when PdfCraft starts"), RowKind::Bool(d.update_check_at_start)));
        }
        rows.push(RowSpec::new("general.secured_notice", tl!("Show a notice when a document is secured"), RowKind::Bool(d.show_secured_notice)));
        #[cfg(not(target_arch = "wasm32"))]
        rows.push(
            RowSpec::new("general.recent", tl!("Recent files to keep"), RowKind::int(d.recent_limit as i64, 0..=crate::RECENT_FILES_MAX as i64))
                .help(tl!("Lowering this forgets the oldest files. At 0 there is no list.")),
        );
        // PLAN: assumed `system_title_bar_available()` on the app: chrome-w10's policy, replacing `window_controls.is_some()`
        rows.push(
            RowSpec::new("general.title_bar", tl!("Use system title bar"), RowKind::Bool(d.system_title_bar))
                .help(tl!(
                    "Show a basic title bar for the window buttons instead of the tab strip. GNOME doesn't draw title bars for apps, so this one doesn't follow its theme."
                ))
                .visible(self.app.system_title_bar_available()),
        );
        PageSpec::new("general", tl!("General & Interface"), rows)
    }

    fn appearance_page(&self) -> PageSpec<'_> {
        let d = &*self.draft;
        let mode = d.reading_colors.mode_name();
        let presets = reading_presets();
        let mut modes: Vec<_> = presets.iter().map(|(id, label, _)| Entry::new(*id, *label)).collect();
        modes.push(Entry::new("custom", tl!("Custom")));
        let custom = mode == "custom";
        let (text, background, only_black, line_art) = d.reading_colors.params.map_or(([0, 0, 0], [255, 255, 255], false, true), |p| (p.text, p.background, p.only_black, p.line_art));
        let reading = vec![
            RowSpec::new("appearance.reading.mode", tl!("Reading colours"), RowKind::choice(mode, modes)),
            RowSpec::new("appearance.reading.text", tl!("Text"), RowKind::Color(rgb(text))).enabled(custom),
            RowSpec::new("appearance.reading.background", tl!("Page background"), RowKind::Color(rgb(background))).enabled(custom),
            RowSpec::new("appearance.reading.only_black", tl!("Only change the colour of black text or line art"), RowKind::Bool(only_black)).enabled(custom),
            RowSpec::new("appearance.reading.line_art", tl!("Change the colour of line art as well as text"), RowKind::Bool(line_art)).enabled(custom),
        ];
        let display = vec![
            RowSpec::new("appearance.edge_shadows", tl!("Show Page Edge Shadows"), RowKind::Bool(d.show_page_edge_shadows)),
            RowSpec::new("appearance.highlight_fields", tl!("Highlight fields"), RowKind::Bool(d.view_defaults.highlight_fields)),
        ];
        PageSpec::custom("appearance", tl_ctx!("theme", "Appearance"))
            .group(GroupSpec::new(None, display))
            .group(GroupSpec::new(Some(tl!("Reading colours")), reading))
            .footnote(tl!("Colours are remapped while rendering; the document itself never changes."))
    }

    fn documents_page(&self) -> PageSpec<'_> {
        let d = &*self.draft;
        let v = &d.view_defaults;
        let rows = vec![
            choice_row("view.mode", tl!("Default workspace mode"), d.default_mode, &mode_table())
                .help(tl!("Used when opening PDFs. An explicit launch or control mode takes precedence for the session.")),
            choice_row("view.layout", tl!("Default page display"), v.layout, &layout_table()),
            RowSpec::new("view.continuous", tl!("Continuous scrolling"), RowKind::Bool(v.continuous)),
            RowSpec::new("view.gaps", tl!("Show gaps between pages"), RowKind::Bool(v.show_page_gaps))
                .help(tl!("Hide gaps to join page edges. Margins inside the PDF remain unchanged.")),
            choice_row("view.fit", tl!("Default zoom"), v.fit, &fit_table())
                .help(tl!("Used when a PDF doesn't ask for a layout or zoom. Continuous scrolling never snaps between pages.")),
            RowSpec::new("view.zoom", tl!("Custom zoom (%)"), RowKind::int((v.zoom * 100.0).round() as i64, 8..=6400)),
            RowSpec::new("view.always", tl!("Always use my default zoom and page layout"), RowKind::Bool(d.always_default_view)),
            RowSpec::new("view.text_links", tl!("Open web and email addresses written in the page text"), RowKind::Bool(d.text_links))
                .help(tl!("Only web (http, https) and email addresses open, and PdfCraft asks first, as for other links.")),
            choice_row("view.units", tl!("Units"), d.units, &unit_table()).help(tl!("For page sizes and margins.")),
            choice_row("view.scroll_speed", tl!("Scroll speed (mouse wheel and arrow keys)"), d.scroll_speed, &speed_table()),
            RowSpec::new("view.invert_middle", tl!("Invert middle-button drag"), RowKind::Bool(d.invert_middle_drag))
                .help(tl!("Panning or auto-scrolling with the middle (wheel) button moves the page the other way.")),
        ];
        PageSpec::new("documents", tl!("Documents and view"), rows)
    }

    fn comment_page(&self) -> PageSpec<'_> {
        let d = &*self.draft;
        let rows = vec![
            RowSpec::new("comment.author", tl!("Name on new comments"), RowKind::Text(d.author.as_str())),
            RowSpec::new("comment.hide_hover", tl!("Hide hover previews for highlights without comment text"), RowKind::Bool(d.hide_empty_highlight_hover)),
        ];
        PageSpec::new("comment", tl!("Comment"), rows)
    }

    fn fill_sign_page(&self) -> PageSpec<'_> {
        use pdfcraft_engine::dates::{DATE_FORMATS, DATE_LANGUAGES};
        let d = &*self.draft;
        let t = &self.texts;
        let preset = DATE_FORMATS.iter().position(|f| *f == d.date_format).and_then(|i| DATE_FORMATS.get(i).copied()).unwrap_or(t.date_custom_id);
        let mut presets: Vec<_> = DATE_FORMATS.iter().zip(&t.date_samples).map(|(f, sample)| Entry::new(*f, sample.as_str())).collect();
        presets.push(Entry::new(t.date_custom_id, tl!("Custom")));
        let mut languages = vec![Entry::new(FOLLOW_ID, t.date_follow.as_str())];
        languages.extend(DATE_LANGUAGES.iter().map(|l| Entry::new(l.code, date_language_picker_name(l))));
        let language = d.date_language.as_deref().filter(|c| DATE_LANGUAGES.iter().any(|l| l.code == *c)).unwrap_or(FOLLOW_ID);
        let rows = vec![
            RowSpec::new("fill.flatten", tl!("Flatten Fill & Sign when saving"), RowKind::Bool(d.flatten_fill_sign_on_save))
                .help(tl!("Text, marks and signatures become part of the page. Other comments stay editable.")),
            RowSpec::new("fill.ask_lock", tl!("Otherwise, ask on the first save after a signature is placed"), RowKind::Bool(d.ask_to_lock_signatures))
                .enabled(!d.flatten_fill_sign_on_save),
            RowSpec::new("fill.date_preset", tl!("Fill & Sign dates"), RowKind::choice(preset, presets)).keywords(tl!("Date format")),
            RowSpec::new("fill.date_pattern", tl!("Pattern"), RowKind::Custom).keywords(tl!("Date format")).help(t.date_help.as_str()),
            RowSpec::new("fill.date_language", tl!("Language"), RowKind::choice(language, languages)).keywords(tl!("Date format")),
        ];
        PageSpec::new("fill", tl!("Fill & Sign"), rows)
    }

    fn javascript_page(&self) -> PageSpec<'_> {
        let rows = vec![
            RowSpec::new("javascript.enable", tl!("Enable Acrobat JavaScript"), RowKind::Bool(self.draft.javascript)).help(tl!(
                "Scripts run in a sandbox without file or network access. A script that asks to open a web page needs your permission first. With JavaScript off, Acrobat's standard format, validate and calculate functions still work."
            )),
        ];
        PageSpec::new("javascript", "JavaScript", rows)
    }
}

#[derive(Clone, Copy, Default)]
struct ViewPrefChanges {
    always_reset: bool,
    layout: bool,
    continuous: bool,
    gaps: bool,
    fit: bool,
    zoom: bool,
}

impl ViewPrefChanges {
    /// Whether any default or the "always" switch changed.
    fn any(self) -> bool {
        self.always_reset || self.layout || self.continuous || self.gaps || self.fit || self.zoom
    }
}

fn apply_view_pref_changes(view: &mut crate::DocView, defaults: &ViewDefaults, changes: ViewPrefChanges) {
    if changes.always_reset || changes.gaps {
        view.set_page_gaps(defaults.show_page_gaps);
    }
    if let Some(saved) = view.saved_display_mut() {
        if changes.always_reset || changes.layout {
            saved.layout = defaults.layout;
        }
        if changes.always_reset || changes.continuous {
            saved.continuous = defaults.continuous;
        }
        if changes.always_reset || changes.fit {
            saved.fit = defaults.fit;
            if defaults.fit == Fit::None {
                saved.zoom = defaults.zoom;
            }
        }
        if changes.zoom {
            saved.fit = Fit::None;
            saved.zoom = defaults.zoom;
        }
    } else {
        if changes.always_reset || (changes.layout && changes.continuous) {
            view.set_display(defaults.layout, defaults.continuous);
        } else {
            if changes.layout {
                view.set_layout(defaults.layout);
            }
            if changes.continuous {
                view.set_continuous(defaults.continuous);
            }
        }
        if changes.always_reset || changes.fit {
            match defaults.fit {
                Fit::None => view.set_zoom(defaults.zoom),
                fit if view.fit != fit => view.set_fit(fit),
                _ => {}
            }
        }
        if changes.zoom {
            view.set_zoom(defaults.zoom);
        }
    }
}
