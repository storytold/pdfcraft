//! PdfCraft Platform Readiness: Keyboard trap prevention, performance budgets, schema preferences, and i18n.
//!
//! Subsystem covering Milestone M14:
//! - [P0] Full keyboard operation of the app (WCAG 2.1)
//! - [P0] Meet performance budgets on Tier-1 platforms (<50ms, <150MB)
//! - [P1] Preferences dialog generated from a schema
//! - [P1] Localization (Fluent catalogs)

pub mod keyboard;
pub mod budget;
pub mod preferences;
pub mod i18n;

pub use keyboard::{FocusableControl, KeyboardFocusManager};
pub use budget::{PerformanceBudgetWatcher, PlatformBudgetLimits};
pub use preferences::{PreferenceItem, PreferencesManager, PrefValue};
pub use i18n::FluentLocalizationCatalog;
