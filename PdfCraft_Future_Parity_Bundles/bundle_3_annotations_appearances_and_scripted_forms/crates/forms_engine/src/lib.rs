//! PdfCraft Forms Engine: AcroForm appearance generator and button action dispatcher.
//!
//! Subsystem covering Milestone M6:
//! - [P0] Push buttons run their actions
//! - [P0] Regenerate field appearances (auto-size, comb, multiline, password)
//! - [P1] Submit form and reset form actions

pub mod field_types;
pub mod appearance_generator;
pub mod button_actions;

pub use field_types::{FormField, FormFieldType, FLAG_COMB, FLAG_MULTILINE, FLAG_PASSWORD, FLAG_READ_ONLY, FLAG_REQUIRED};
pub use appearance_generator::FieldAppearanceGenerator;
pub use button_actions::{ButtonActionDispatcher, ButtonMouseEvent, FormAction};
