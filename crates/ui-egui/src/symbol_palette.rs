//! Symbol insertion palette for PDF Craft rich text editing (L7 UI).
//!
//! Provides an intuitive, categorized palette of mathematical, typographical, currency,
//! and general utility symbols (©, ®, ™, §, ¶, •, —, –, ±, ≠, ≤, ≥, °, €, £, ¥, etc.)
//! with one-click insertion directly into active text lines and paragraphs.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use craft::theme::TypeRole;
use craft::ui::contract::Show as _;
use craft::ui::display::Text;
use craft::ui::runtime::env::Env;
use egui::{Color32, CornerRadius, Pos2, Stroke, Vec2};

/// Common symbol categories for rapid insertion.
pub struct SymbolCategory {
    pub name: &'static str,
    pub symbols: &'static [&'static str],
}

pub const SYMBOL_CATEGORIES: &[SymbolCategory] = &[
    SymbolCategory { name: "Common & Legal", symbols: &["©", "®", "™", "§", "¶", "•", "–", "—", "†", "‡", "№"] },
    SymbolCategory {
        name: "Math & Logic", symbols: &["±", "≠", "≤", "≥", "≈", "×", "÷", "√", "∞", "∑", "∏", "π", "µ", "°", "‰"]
    },
    SymbolCategory { name: "Currency", symbols: &["$", "€", "£", "¥", "₹", "¢", "₿", "₽", "₩", "₪"] },
    SymbolCategory { name: "Arrows & Marks", symbols: &["→", "←", "↑", "↓", "↔", "⇒", "⇔", "✓", "✗", "★", "☆", "▲", "▼"] },
    SymbolCategory {
        name: "Fractions & Subscripts",
        symbols: &["½", "⅓", "¼", "¾", "⅛", "⅜", "⅝", "⅞", "⁰", "¹", "²", "³", "⁴", "₀", "₁", "₂", "₃"],
    },
];

/// Floating symbol palette modal / popup.
pub struct SymbolPalette;

impl SymbolPalette {
    /// Render the symbol picker popup anchored at `anchor`.
    /// Returns the inserted character string if a symbol button was clicked.
    /// Descriptive English name for a symbol glyph, used for tooltips and accessibility labels.
    fn symbol_description(sym: &str) -> &'static str {
        match sym {
            "©" => "Copyright sign",
            "®" => "Registered sign",
            "™" => "Trademark sign",
            "§" => "Section sign",
            "¶" => "Pilcrow sign",
            "•" => "Bullet",
            "–" => "En dash",
            "—" => "Em dash",
            "†" => "Dagger",
            "‡" => "Double dagger",
            "№" => "Numero sign",
            "±" => "Plus-minus sign",
            "≠" => "Not equal to",
            "≤" => "Less than or equal to",
            "≥" => "Greater than or equal to",
            "≈" => "Almost equal to",
            "×" => "Multiplication sign",
            "÷" => "Division sign",
            "√" => "Square root",
            "∞" => "Infinity",
            "∑" => "Summation",
            "∏" => "Product",
            "π" => "Pi",
            "µ" => "Micro sign",
            "°" => "Degree sign",
            "‰" => "Per mille sign",
            "$" => "Dollar sign",
            "€" => "Euro sign",
            "£" => "Pound sign",
            "¥" => "Yen sign",
            "₹" => "Indian rupee sign",
            "¢" => "Cent sign",
            "₿" => "Bitcoin sign",
            "₽" => "Ruble sign",
            "₩" => "Won sign",
            "₪" => "Shekel sign",
            "→" => "Right arrow",
            "←" => "Left arrow",
            "↑" => "Up arrow",
            "↓" => "Down arrow",
            "↔" => "Left-right arrow",
            "⇒" => "Double right arrow",
            "⇔" => "Double left-right arrow",
            "✓" => "Check mark",
            "✗" => "Cross mark",
            "★" => "Filled star",
            "☆" => "Outline star",
            "▲" => "Up triangle",
            "▼" => "Down triangle",
            "½" => "One half",
            "⅓" => "One third",
            "¼" => "One quarter",
            "¾" => "Three quarters",
            "⅛" => "One eighth",
            "⅜" => "Three eighths",
            "⅝" => "Five eighths",
            "⅞" => "Seven eighths",
            "⁰" => "Superscript zero",
            "¹" => "Superscript one",
            "²" => "Superscript two",
            "³" => "Superscript three",
            "⁴" => "Superscript four",
            "₀" => "Subscript zero",
            "₁" => "Subscript one",
            "₂" => "Subscript two",
            "₃" => "Subscript three",
            _ => "Symbol",
        }
    }

    pub fn show(ui: &mut egui::Ui, anchor: Pos2) -> Option<&'static str> {
        let env = Env::of(ui);
        let to_c32 = |c: [u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        let card = to_c32(env.tokens.color.surface.raised.0);
        let border = to_c32(env.tokens.color.line.border.0);
        let ink = to_c32(env.tokens.color.text.normal.0);
        let radius = CornerRadius::same(env.tokens.shape.radius.sm.round().clamp(0.0, 255.0) as u8);
        let gap = env.tokens.space.gap.tight;
        let pad = env.tokens.space.gap.inline;
        let btn_side = env.tokens.size.control.md;
        let glyph_font = ui.style().text_styles.get(&egui::TextStyle::Body).cloned().unwrap_or_default();
        let dark = card.r() < 128;
        let mut inserted = None;

        egui::Area::new(egui::Id::unique("floating-symbol-palette")).order(egui::Order::Foreground).fixed_pos(anchor).show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(card)
                .stroke(Stroke::new(1.0, border))
                .corner_radius(radius)
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if dark { 100 } else { 36 }) })
                .inner_margin(egui::Margin::same(pad.round().clamp(0.0, 127.0) as i8))
                .show(ui, |ui| {
                    ui.set_width((btn_side + gap) * 9.0 + pad * 2.0);
                    ui.horizontal(|ui| {
                        Text::new(tl!("Insert Symbol").to_string()).role(TypeRole::Label).show(ui);
                    });
                    ui.add_space(gap);

                    for cat in SYMBOL_CATEGORIES {
                        Text::new(tl!(cat.name).to_string()).role(TypeRole::Caption).show(ui);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::splat(gap);
                            for &sym in cat.symbols {
                                let desc = tl!(Self::symbol_description(sym)).to_string();
                                let a11y_label = format!("{sym} {desc}");
                                let btn = egui::Button::new(egui::RichText::new(sym).font(glyph_font.clone()).color(ink))
                                    .min_size(Vec2::splat(btn_side))
                                    .corner_radius(radius);
                                let resp = ui.add(btn).on_hover_text(&desc);
                                resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, a11y_label.clone()));
                                if resp.clicked() {
                                    inserted = Some(sym);
                                }
                            }
                        });
                        ui.add_space(gap);
                    }
                });
        });

        inserted
    }
}
