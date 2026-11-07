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
//!   `calculateNow`, `print`, `submitForm`, `mailDoc`.
//! - Field objects: `name`, `type`, `value`, `valueAsString`, `defaultValue`, `readonly`,
//!   `required`, `display`, `hidden`, `numItems`, `getItemAt`, `currentValueIndices`,
//!   `isBoxChecked`, `checkThisBox`, `exportValues`, `charLimit`, `textColor`, `fillColor`,
//!   `setFocus`.
//! - `app` (`alert`, `beep`, `response`, `launchURL`, `execMenuItem`, `viewerType`, `viewerVersion`,
//!   `platform`, `setTimeOut`), `util` (`printf`, `printd`, `printx`, `scand`), `console` (`println`,
//!   `show`, `clear`), `display` and `color`.
//!
//! The engine is a sandbox: no file, network or timer access exists, and loops and recursion
//! are bounded. Side effects (alerts, field changes, resets, printing, navigation) are returned
//! in an [`Outcome`] for the caller to apply.
//!
//! Script editors use [`highlight`] to colour a script and [`in_literal`] before adding white space,
//! without running or parsing it.

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
mod format;
mod globals;
mod highlight;
mod host;
mod model;
mod run;

#[cfg(test)]
pub(crate) use format::PRINTD_SCANNED;
use format::{printd, printf, printx, scand};
pub use format::{set_date_names, with_english_printd};
pub(crate) use globals::menu_item;
use globals::platform;
pub use highlight::{Token, TokenKind, highlight, in_literal};
use host::{arg, as_number, error, function, s, text};
pub use model::{
    DISPLAY_HIDDEN, DISPLAY_NO_PRINT, DISPLAY_NO_VIEW, DISPLAY_VISIBLE, DocInfo, Event, FieldState, FieldType, Limits, Message, MessageButtons,
    MessageIcon, Outcome, Panel, Request,
};
pub(crate) use run::CheckedCodeFromStrings;
#[cfg(not(target_arch = "wasm32"))]
use run::SCRIPT_STACK;
use run::refuse;
pub use run::{MAX_SCRIPT_BYTES, run, run_shared};

pub mod formcalc;
pub mod xfa;
pub mod xfa_session;

#[cfg(test)]
mod tests;
