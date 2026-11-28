//! pdfcraft-js — Acrobat JavaScript for forms (L3).
//!
//! Field scripts (format, keystroke, validate, calculate), button actions and document-level
//! scripts run in [boa](https://boajs.dev), a JavaScript engine written in Rust, with the subset
//! of Acrobat's object model that forms use, as the public *JavaScript for Acrobat API Reference*
//! describes it:
//!
//! - `event`: `value`, `change`, `willCommit`, `rc`, `name`, `type`, `target`, `targetName`.
//! - the document (`this`, also the global object): `getField`, `numFields`,
//!   `getNthFieldName`, `numPages`, `pageNum`, `documentFileName`, `info`, `resetForm`,
//!   `calculateNow`, `getPageBox`, `print`, `submitForm`, `mailDoc`, `exportDataObject`, `templates`,
//!   `getTemplate`, `spawnPageFromTemplate`.
//! - Template objects: `name`, `hidden` (setting it shows or hides the template's page), `spawn`.
//! - Field objects: `name`, `type`, `value`, `valueAsString`, `defaultValue`, `readonly`,
//!   `required`, `display`, `hidden`, `numItems`, `getItemAt`, `currentValueIndices`,
//!   `isBoxChecked`, `checkThisBox`, `exportValues`, `charLimit`, `textColor`, `fillColor`,
//!   `setFocus`, `rect`, appearance properties, and normal button caption/icon methods.
//! - `app` (`alert`, `beep`, `response`, `launchURL`, `execMenuItem`, `viewerType`, `viewerVersion`,
//!   `platform`, `setTimeOut`), `util` (`printf`, `printd`, `printx`, `scand`), `console` (`println`,
//!   `show`, `clear`), `display` and `color`.
//!
//! The native engine uses [`session::ScriptSession`] to preserve document globals across events.
//! Stateless runners remain available for callers that need isolation per invocation.
//!
//! The engine is a sandbox: no file, network or timer access exists, and loops and recursion
//! are bounded. Side effects (alerts, field changes, resets, printing, navigation) are returned
//! in an [`Outcome`] for the caller to apply.
//! A set exceeding 1,024 document libraries or 8 MiB of combined source is rejected before
//! the event handler runs. Within an admitted set, an individual library over 1 MiB is skipped.
//! individual event handlers remain limited to 256 KiB.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;

use boa_engine::context::HostHooks;
use boa_engine::object::builtins::{JsArray, JsDate, JsFunction};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::realm::Realm;
use boa_engine::{Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, Source, js_string};

mod document;
pub mod form_data;
mod format;
mod globals;
mod host;
mod legacy;
mod model;
mod nesting;
mod run;
pub mod selected_pdf;
mod source;
mod streams;

#[cfg(test)]
pub(crate) use format::PRINTD_SCANNED;
use format::{printd, printf, printx, scand};
pub use format::{set_date_names, with_english_printd};
pub(crate) use globals::menu_item;
use globals::platform;
use host::{arg, as_number, error, function, s, text};
pub use model::WidgetRect;
pub use model::{
    DISPLAY_HIDDEN, DISPLAY_NO_PRINT, DISPLAY_NO_VIEW, DISPLAY_VISIBLE, DocInfo, Event, FieldState, FieldType, Limits, Message, MessageButtons,
    MessageIcon, Outcome, Panel, PrintDuplex, PrintSettings, Request,
};
#[cfg(not(target_arch = "wasm32"))]
use run::SCRIPT_STACK;
use run::refuse;
pub(crate) use run::{CheckedCodeFromStrings, MAX_SCRIPT_BYTES};
pub use run::{run, run_shared, run_shared_calculations};

pub mod formcalc;
pub mod xfa;
pub mod xfa_session;

#[cfg(test)]
mod tests;

mod geometry;
pub use geometry::{rotate_rect, unrotate_rect};

pub mod bookmarks;
mod icons;
mod ocg;
pub mod templates;
pub mod viewer;
pub use ocg::Layer;
mod timers;
pub use timers::{MAX_TIMERS, MIN_TIMER_MS, Timer};

pub mod dialogs;
pub mod interaction;
pub mod session;

/// Maximum reset calls admitted by one document invocation, before argument conversion or field scans.
pub const MAX_RESET_CALLS: usize = 64;
