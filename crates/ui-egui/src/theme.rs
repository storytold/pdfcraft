//! Design tokens and egui style (plan/acrobat/02-ui-ux.md §1: neutral chrome, white panels,
//! one blue accent; Dark Gray keeps pages white). Every custom widget reads `Tokens::get`.

use std::sync::Arc;

use egui::epaint::text::{FallbackAlign, FontTweak, HintingTarget, SmoothHinting};
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Response, Stroke, Vec2, Visuals};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeKind {
    #[default]
    Light,
    Dark,
}

/// The saved user choice, independent of the light/dark colours currently displayed. A fresh
/// install follows the system (#1248); a desktop that says nothing gets Light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    /// The colours to draw. `system` is what the operating system says, `None` when it says
    /// nothing; `fallback` is supplied by the caller. PdfCraft uses Light for an unknown desktop.
    pub fn resolve(self, system: Option<bool>, fallback: ThemeKind) -> ThemeKind {
        match self {
            Self::Light => ThemeKind::Light,
            Self::Dark => ThemeKind::Dark,
            Self::System => match system {
                Some(true) => ThemeKind::Dark,
                Some(false) => ThemeKind::Light,
                None => fallback,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub kind: ThemeKind,
    /// Title/tab strip.
    pub titlebar: Color32,
    /// Mode bar, panels.
    pub chrome: Color32,
    pub panel: Color32,
    /// Document area behind pages.
    pub pasteboard: Color32,
    pub card: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub selected: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub accent_soft: Color32,
    pub field: Color32,
    pub badge_new: Color32,
    pub page_shadow: Color32,
    pub radius: u8,
}

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Light => Self {
                kind,
                titlebar: Color32::from_rgb(0xE9, 0xE9, 0xEB),
                chrome: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                pasteboard: Color32::from_rgb(0xF1, 0xF1, 0xF3),
                card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                border: Color32::from_rgb(0xDA, 0xDA, 0xDE),
                divider: Color32::from_rgb(0xE8, 0xE8, 0xEB),
                text: Color32::from_rgb(0x22, 0x22, 0x26),
                text_muted: Color32::from_rgb(0x5E, 0x5E, 0x66),
                text_faint: Color32::from_rgb(0x6B, 0x6B, 0x73),
                icon: Color32::from_rgb(0x44, 0x44, 0x4B),
                hover: Color32::from_rgb(0xF0, 0xF0, 0xF3),
                pressed: Color32::from_rgb(0xE4, 0xE4, 0xE9),
                selected: Color32::from_rgb(0xE6, 0xEE, 0xFD),
                accent: Color32::from_rgb(0x1B, 0x63, 0xE0),
                accent_text: Color32::from_rgb(0x17, 0x55, 0xC4),
                accent_soft: Color32::from_rgb(0xE3, 0xEC, 0xFD),
                field: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                badge_new: Color32::from_rgb(0x1B, 0x63, 0xE0),
                page_shadow: Color32::from_black_alpha(34),
                radius: 6,
            },
            ThemeKind::Dark => Self {
                kind,
                titlebar: Color32::from_rgb(0x1B, 0x1B, 0x1E),
                chrome: Color32::from_rgb(0x26, 0x26, 0x2A),
                panel: Color32::from_rgb(0x26, 0x26, 0x2A),
                pasteboard: Color32::from_rgb(0x19, 0x19, 0x1C),
                card: Color32::from_rgb(0x2E, 0x2E, 0x33),
                border: Color32::from_rgb(0x3C, 0x3C, 0x43),
                divider: Color32::from_rgb(0x33, 0x33, 0x39),
                text: Color32::from_rgb(0xEC, 0xEC, 0xEF),
                text_muted: Color32::from_rgb(0xAE, 0xAE, 0xB6),
                text_faint: Color32::from_rgb(0x97, 0x97, 0x9E),
                icon: Color32::from_rgb(0xD4, 0xD4, 0xDA),
                hover: Color32::from_rgb(0x34, 0x34, 0x3A),
                pressed: Color32::from_rgb(0x3E, 0x3E, 0x45),
                selected: Color32::from_rgb(0x23, 0x3A, 0x63),
                accent: Color32::from_rgb(0x4B, 0x8B, 0xF5),
                accent_text: Color32::from_rgb(0x8C, 0xB6, 0xFA),
                accent_soft: Color32::from_rgb(0x24, 0x36, 0x57),
                field: Color32::from_rgb(0x1E, 0x1E, 0x22),
                badge_new: Color32::from_rgb(0x3D, 0x7D, 0xEE),
                page_shadow: Color32::from_black_alpha(120),
                radius: 6,
            },
        }
    }

    /// Return the cached theme tokens, falling back to the light palette before the first apply.
    pub fn get(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::unique("pdfcraft-theme"))).unwrap_or_else(|| Self::for_kind(ThemeKind::Light))
    }

    pub fn dark(&self) -> bool {
        self.kind == ThemeKind::Dark
    }
}

/// Hover and press amounts for chrome controls. The wash eases in; holding the pointer
/// darkens it and drops the content one pixel, so a click reads as a key settling.
#[derive(Clone, Copy, Debug)]
pub struct Press {
    pub hover: f32,
    pub down: f32,
}

impl Press {
    pub fn track(ui: &egui::Ui, resp: &Response) -> Self {
        let hot = resp.hovered() || resp.is_pointer_button_down_on();
        let ease = egui::emath::easing::quadratic_out;
        // With animation effects off the wash and the press are simply there.
        let (hover, down) = if crate::motion::animated(ui.ctx()) { (0.12, 0.06) } else { (0.0, 0.0) };
        Self {
            hover: ui.ctx().animate_bool_with_time_and_easing(resp.id.with("chrome-hover"), hot, hover, ease),
            down: ui.ctx().animate_bool_with_time_and_easing(resp.id.with("chrome-down"), resp.is_pointer_button_down_on(), down, ease),
        }
    }

    pub fn fill(self, rest: Color32, hover: Color32, pressed: Color32) -> Color32 {
        rest.lerp_to_gamma(hover, self.hover).lerp_to_gamma(pressed, self.down)
    }

    /// One pixel down at a full press.
    pub fn offset(self) -> Vec2 {
        Vec2::new(0.0, self.down)
    }

    /// The colour [`Self::wash`] paints.
    pub fn wash_color(self, t: &Tokens, selected: bool) -> Color32 {
        if selected {
            self.fill(t.accent_soft, t.accent_soft.lerp_to_gamma(t.accent, 0.16), t.selected)
        } else {
            self.fill(Color32::TRANSPARENT, t.hover, t.pressed)
        }
    }

    /// Soft wash behind a control. `selected` keeps the accent fill.
    pub fn wash(self, ui: &egui::Ui, rect: Rect, radius: impl Into<CornerRadius>, selected: bool) {
        let fill = self.wash_color(&Tokens::get(ui.ctx()), selected);
        if fill.a() > 0 {
            ui.painter().rect_filled(rect, radius, fill);
        }
    }
}

/// Pointing hand, the cursor for a control that clicks.
pub fn hand(resp: Response) -> Response {
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn install_fonts(ctx: &egui::Context) {
    install_fonts_for(ctx, CjkPreference::Default);
}

/// The interface language's claim on the CJK fallback faces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CjkPreference {
    /// Japanese faces first (every non-Chinese language).
    #[default]
    Default,
    SimplifiedChinese,
    TraditionalChinese,
}

impl CjkPreference {
    /// The preference for a UI language code (`zh-hans`, `zh-hant`, anything else).
    pub fn for_language(code: &str) -> Self {
        match code {
            "zh-hans" => Self::SimplifiedChinese,
            "zh-hant" => Self::TraditionalChinese,
            _ => Self::Default,
        }
    }
}

/// Install the interface fonts with the CJK fallback order for the UI language (Chinese
/// first for Chinese, Japanese first otherwise). Call it when the language changes; the new
/// faces take effect next frame.
pub fn install_fonts_for(ctx: &egui::Context, cjk: CjkPreference) {
    ctx.set_fonts(installed_font_definitions(cjk));
}

/// The name of the installed face [`installed_font_definitions`] may add after the embedded ones.
pub const SYSTEM_FALLBACK: &str = "system-fallback";
/// The name of the installed Devanagari (Hindi) face [`installed_font_definitions`] may add.
pub const SYSTEM_DEVANAGARI_FALLBACK: &str = "system-devanagari-fallback";
/// The name of the installed Telugu face [`installed_font_definitions`] may add.
pub const SYSTEM_TELUGU_FALLBACK: &str = "system-telugu-fallback";
/// The name of the installed Sinhala face [`installed_font_definitions`] may add.
pub const SYSTEM_SINHALA_FALLBACK: &str = "system-sinhala-fallback";
/// The name of the installed Tamil face [`installed_font_definitions`] may add.
pub const SYSTEM_TAMIL_FALLBACK: &str = "system-tamil-fallback";
/// The name of the installed Hebrew face [`installed_font_definitions`] may add.
pub const SYSTEM_HEBREW_FALLBACK: &str = "system-hebrew-fallback";
/// The name of the installed Hangul (Korean) face [`installed_font_definitions`] may add.
pub const SYSTEM_HANGUL_FALLBACK: &str = "system-hangul-fallback";

/// Which installed face an interface language needs, given whether the build embeds a `Hans`
/// face.
///
/// A bundled `Hans` face serves Simplified Chinese, so the installed face stays last there. It
/// cannot serve Traditional Chinese: a Traditional interface keeps an installed `Hant` face
/// before the Japanese faces even when a Simplified-only face is embedded, or Traditional-only
/// characters would fall through to a Japanese face first.
#[cfg(not(target_arch = "wasm32"))]
fn installed_fallback_need(cjk: CjkPreference, has_embedded_hans: bool) -> crate::system_fonts::Need {
    use crate::system_fonts::Need;
    match cjk {
        CjkPreference::SimplifiedChinese if has_embedded_hans => Need::Other,
        CjkPreference::SimplifiedChinese => Need::Hans,
        CjkPreference::TraditionalChinese => Need::Hant,
        CjkPreference::Default => Need::Other,
    }
}

/// What [`install_fonts_for`] installs: [`font_definitions_for`], then, on desktop, one face
/// already installed on this machine. It only draws characters no embedded face has.
///
/// - In a Simplified Chinese interface built without an embedded `Hans` face (#826), it is a
///   Simplified Chinese system face (Microsoft YaHei, PingFang SC, …), placed before the Japanese
///   craft-fonts faces: they lack hundreds of the catalog's characters (欢, 导, 签, …), and one
///   line drawn from two faces with different vertical metrics would not sit on one baseline.
/// - A Traditional Chinese interface always uses an installed `Hant` face before the Japanese
///   faces, even when a Simplified-only `Hans` face is embedded: that face cannot supply
///   Traditional-only characters (說, 體, …).
/// - Otherwise it is the last fallback of every family (an Arabic file name in a build without
///   craft-fonts).
///
/// Installed Devanagari, Telugu, Sinhala, Tamil, Hebrew and Hangul faces, when there are, end
/// every family just before that one, so Hindi, Telugu (#1531), Sinhala and Tamil (#1688) and
/// Korean text, such as a file name or a tab title, and Hebrew file names (#586) draw in a build
/// without such faces.
///
/// `PDFCRAFT_SYSTEM_FONTS=0` leaves them out.
pub fn installed_font_definitions(cjk: CjkPreference) -> FontDefinitions {
    #[cfg_attr(target_arch = "wasm32", expect(unused_mut))]
    let mut fonts = font_definitions_for(cjk == CjkPreference::SimplifiedChinese);
    #[cfg(not(target_arch = "wasm32"))]
    {
        use crate::system_fonts::{Need, fallback};
        // Before the general installed face, which stays the last fallback outside Chinese.
        for (name, data) in [
            (SYSTEM_DEVANAGARI_FALLBACK, crate::system_fonts::devanagari_fallback()),
            (SYSTEM_TELUGU_FALLBACK, crate::system_fonts::telugu_fallback()),
            (SYSTEM_SINHALA_FALLBACK, crate::system_fonts::sinhala_fallback()),
            (SYSTEM_TAMIL_FALLBACK, crate::system_fonts::tamil_fallback()),
            (SYSTEM_HEBREW_FALLBACK, crate::system_fonts::hebrew_fallback()),
            (SYSTEM_HANGUL_FALLBACK, crate::system_fonts::hangul_fallback()),
        ] {
            let Some(data) = data else { continue };
            fonts.font_data.insert(name.to_owned(), data);
            for stack in fonts.families.values_mut() {
                stack.push(name.to_owned());
            }
        }
        let need = installed_fallback_need(cjk, !pdfcraft_fonts::ui_chinese_fonts().is_empty());
        if let Some(data) = fallback(need) {
            fonts.font_data.insert(SYSTEM_FALLBACK.to_owned(), data);
            let japanese: Vec<String> = pdfcraft_fonts::ui_japanese_fonts().iter().map(|f| f.name()).collect();
            for stack in fonts.families.values_mut() {
                let at = match need {
                    Need::Other => None,
                    Need::Hans | Need::Hant => stack.iter().position(|n| japanese.contains(n)),
                };
                stack.insert(at.unwrap_or(stack.len()), SYSTEM_FALLBACK.to_owned());
            }
        }
    }
    fonts
}

/// The interface fonts: Inter (and JetBrains Mono for code) first, then egui's defaults, then
/// the CJK, Arabic, Telugu, Hebrew and Thai faces of the optional craft-fonts build input as the
/// last fallback in every family. Without craft-fonts there is no Japanese, Chinese, Arabic,
/// Telugu, Hebrew or Thai face here.
pub fn font_definitions() -> FontDefinitions {
    font_definitions_for(false)
}

/// [`font_definitions`] with the CJK fallback order for the UI language. Simplified Chinese
/// must come first in Chinese mode: otherwise shared characters render in the Japanese face
/// while Simplified-only characters (e.g. U+6B22 欢) fall through to the Chinese face, and
/// the mixed vertical metrics sink them below the line.
pub fn font_definitions_for(prefer_hans: bool) -> FontDefinitions {
    font_definitions_for_platform(prefer_hans, current_font_policy_platform())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontPolicyPlatform {
    Windows,
    Other,
}

fn current_font_policy_platform() -> FontPolicyPlatform {
    if cfg!(target_os = "windows") { FontPolicyPlatform::Windows } else { FontPolicyPlatform::Other }
}

fn primary_font_tweak(platform: FontPolicyPlatform, name: &str) -> FontTweak {
    let mut tweak = FontTweak::default();
    if platform == FontPolicyPlatform::Windows && name == "JetBrainsMono" {
        tweak.hinting_target = HintingTarget::Smooth(SmoothHinting { symmetric_rendering: false, ..SmoothHinting::default() });
    }
    tweak
}

/// `data` with its glyphs placed on the row's baseline (#1676). epaint 0.37 otherwise centres a
/// fallback face's line box on the primary's, so Chinese next to Latin in one label sits higher or
/// lower than the Latin by a few percent of the font size, depending on the installed or embedded
/// face (PingFang, Microsoft JhengHei, LXGW WenKai Lite…). `FallbackAlign::Baseline` makes epaint
/// position every face's glyphs from the primary face's ascent, which is exactly the existing
/// baseline-alignment intent without metric-derived offsets that go stale across egui versions.
pub(crate) fn on_interface_baseline(mut data: FontData) -> FontData {
    data.tweak.fallback_align = FallbackAlign::Baseline;
    data
}

fn font_definitions_for_platform(prefer_hans: bool, platform: FontPolicyPlatform) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        let data = FontData::from_static(bytes);
        let data = if ["Inter", "Inter-Medium", "Inter-SemiBold", "JetBrainsMono"].contains(&name) {
            data.tweak(primary_font_tweak(platform, name))
        } else {
            on_interface_baseline(data)
        };
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    };
    // Shared with typed signatures' fallback glyphs, so the font is embedded once.
    add(&mut fonts, "Inter", pdfcraft_fonts::INTER_REGULAR);
    add(&mut fonts, "Inter-Medium", include_bytes!("../../../assets/fonts/Inter-Medium.ttf"));
    add(&mut fonts, "Inter-SemiBold", include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"));
    add(&mut fonts, "JetBrainsMono", include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Inter".to_owned());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "JetBrainsMono".to_owned());
    // Keep missing Latin/Cyrillic letters in the app's own faces (for example Bulgarian ѝ,
    // absent from JetBrains Mono), before egui's defaults or optional system/CJK fallbacks.
    fonts.families.entry(FontFamily::Monospace).or_default().insert(1, "Inter".to_owned());
    // The same static bytes pdfcraft-fonts uses for Japanese/Chinese text in PDFs: one copy, not two.
    for face in pdfcraft_fonts::ui_cjk_fonts(prefer_hans) {
        let name = face.name();
        // Japanese and Chinese faces have distinct family names, so no collision here.
        if !fonts.font_data.contains_key(&name) {
            add(&mut fonts, &name, face.bytes);
        }
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
    }
    // Arabic-script faces (file names, document titles), Telugu faces (the Telugu interface,
    // file names), Hebrew faces (file names, text being edited), Thai faces and Devanagari faces
    // (the Hindi interface) after the CJK ones; the ranges don't overlap.
    for face in pdfcraft_fonts::ui_arabic_fonts()
        .into_iter()
        .chain(pdfcraft_fonts::ui_telugu_fonts())
        .chain(pdfcraft_fonts::ui_hebrew_fonts())
        .chain(pdfcraft_fonts::ui_thai_fonts())
        .chain(pdfcraft_fonts::ui_devanagari_fonts())
    {
        let name = face.name();
        if !fonts.font_data.contains_key(&name) {
            add(&mut fonts, &name, face.bytes);
        }
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let stack = fonts.families.entry(family).or_default();
            if !stack.contains(&name) {
                stack.push(name.clone());
            }
        }
    }
    let fallback: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    for (fam, primary) in [("medium", "Inter-Medium"), ("semibold", "Inter-SemiBold")] {
        let mut stack = vec![primary.to_owned()];
        stack.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(fam.into()), stack);
    }
    fonts
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// Install the palette, widget styling, text sizes and right-to-left flag for the chosen theme.
pub fn apply(ctx: &egui::Context, kind: ThemeKind) {
    // Keep the native window inherited. A concrete egui theme pins macOS's
    // NSWindow appearance and prevents winit from delivering ThemeChanged.
    // Our saved choice owns the palette; both egui branches receive it below.
    ctx.set_theme(egui::ThemePreference::System);
    let rtl = crate::i18n::current().rtl();
    let t = Tokens::for_kind(kind);
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("pdfcraft-theme"), t));
    let mut v = if t.dark() { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.hover;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.override_text_color = Some(t.text);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if t.dark() { 110 } else { 38 }) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }) };
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(t.radius);
        w.fg_stroke.color = t.text;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = t.field;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.bg_fill = t.pressed;
    // Stock buttons (menus, dialogs) use the same pointing hand as the custom chrome.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.widgets.open.weak_bg_fill = t.hover;
    // Crisper text at 100–150 % scaling (#76): glyphs sit on whole pixels instead of being
    // rendered at quarter-pixel offsets, which egui notes blurs them. In the light theme, a mild
    // gamma darkens the antialiased edges of dark text (egui's default is linear, which reads thin
    // and grey next to the system's text); the dark theme keeps egui's own curve. At 200 % both
    // make little difference.
    v.text_options.subpixel_binning = false;
    if !t.dark() {
        v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Gamma(0.75);
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        // RTL languages mirror stock chrome (menus, combo boxes, scroll bars, headers).
        // Labels still get their paragraph direction explicitly from `bidi::ui_direction`.
        s.right_to_left = rtl;
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
        s.spacing.scroll.floating_width = 3.0;
        s.spacing.scroll.foreground_color = true;
        s.text_styles.insert(egui::TextStyle::Body, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Button, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Small, regular(11.0));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(17.0));
        s.interaction.tooltip_delay = 0.35;
    });
    let style = ctx.global_style();
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.data_mut(|d| d.insert_temp(direction_key(), rtl));
}

/// Return the stable egui id holding the current right-to-left style flag.
fn direction_key() -> egui::Id {
    egui::Id::unique("pdfcraft-style-right-to-left")
}

/// Keep stock egui chrome's direction in step with the drawing language, after a language
/// change as well as at startup. Setting a flag on both concrete themes is cheap; this avoids
/// rebuilding the whole visuals and font state every frame.
pub fn sync_direction(ctx: &egui::Context) {
    let rtl = crate::i18n::current().rtl();
    let changed = ctx.data_mut(|d| {
        if d.get_temp::<bool>(direction_key()) == Some(rtl) {
            false
        } else {
            d.insert_temp(direction_key(), rtl);
            true
        }
    });
    if changed {
        ctx.all_styles_mut(|s| s.right_to_left = rtl);
    }
}

/// WCAG 2 contrast ratio between two opaque colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let lin = |v: u8| {
            let s = v as f32 / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    };
    let (x, y) = (lum(a) + 0.05, lum(b) + 0.05);
    x.max(y) / x.min(y)
}

/// `color` as text on `background`: itself when it reads (a WCAG 2 contrast of at least 3:1, the
/// level for large or bold text), otherwise moved towards `text` until it does. A document's own
/// colours (a bookmark's `/C`) were chosen for a white panel, and dark blue on a dark theme would
/// be lost.
pub fn readable(color: Color32, background: Color32, text: Color32) -> Color32 {
    (0..=4).map(|step| color.lerp_to_gamma(text, step as f32 / 4.0)).find(|c| contrast(*c, background) >= 3.0).unwrap_or(text)
}

#[cfg(test)]
mod tests;
