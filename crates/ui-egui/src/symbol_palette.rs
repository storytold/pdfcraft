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
use egui::{Color32, CornerRadius, FontId, Pos2, Stroke, Vec2};

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
    pub fn show(ui: &mut egui::Ui, anchor: Pos2) -> Option<&'static str> {
        let env = Env::of(ui);
        let to_c32 = |c: [u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        let card = to_c32(env.tokens.color.surface.raised.0);
        let border = to_c32(env.tokens.color.line.border.0);
        let dark = card.r() < 128;
        let mut inserted = None;

        egui::Area::new(egui::Id::unique("floating-symbol-palette")).order(egui::Order::Foreground).fixed_pos(anchor).show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(card)
                .stroke(Stroke::new(1.0, border))
                .corner_radius(CornerRadius::same(8))
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if dark { 100 } else { 36 }) })
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(280.0);
                    ui.horizontal(|ui| {
                        Text::new("Insert Symbol").role(TypeRole::Label).show(ui);
                    });
                    ui.add_space(6.0);

                    for cat in SYMBOL_CATEGORIES {
                        Text::new(tl!(cat.name).to_string()).role(TypeRole::Caption).show(ui);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::splat(3.0);
                            for &sym in cat.symbols {
                                let btn = egui::Button::new(egui::RichText::new(sym).font(FontId::proportional(13.0)))
                                    .min_size(Vec2::splat(26.0))
                                    .corner_radius(CornerRadius::same(4));
                                if ui.add(btn).clicked() {
                                    inserted = Some(sym);
                                }
                            }
                        });
                        ui.add_space(4.0);
                    }
                });
        });

        inserted
    }
}
