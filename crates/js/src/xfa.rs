//! The XFA scripting object model (XFA 3.3 part 6, "Scripting"): `xfa.form`, `this`, `xfa.host`,
//! `xfa.layout`, `xfa.event`, `xfa.datasets`, instance managers and SOM resolution, as native
//! objects over a snapshot of the form tree the caller hands in. Scripts work on the snapshot
//! and everything they change or ask for comes back as [`XfaEffect`]s for the engine to apply.
//!
//! Form nodes are Proxies, because XFA scripts address children by name (`page1.familyName`)
//! and instance managers by `_name`. Unqualified names resolve the XFA way: the current node's
//! children, then its siblings, then each ancestor's children.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use boa_engine::object::ObjectInitializer;
use boa_engine::object::builtins::{JsArray, JsProxy};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsObject, JsResult, JsString, JsValue, NativeFunction, Source, js_string};

use crate::globals::data_object;
use crate::{Limits, Message, arg, as_number, error, function, platform, printd, printf, printx, refuse, s, scand, text};

mod globals;
mod host;
mod instances;
mod model;
mod nodes;
pub(crate) mod props;
// The rules a property change must meet, the crate the form (`pdfcraft_xfa`, which re-exports it
// as `pdfcraft_xfa::prop_rules`) depends on too: one source for both sides.
pub(crate) use pdfcraft_xfa_rules as prop_rules;
mod run;

#[cfg(target_arch = "wasm32")]
pub(crate) use host::forget_base;
pub(crate) use host::{HNode, XHost};
pub(crate) use model::{FormCopy, MAX_LINE, MAX_PAYLOAD, clip, clip_result, reset_names};
pub use model::{MAX_ALERTS, MAX_CONSOLE, MAX_EFFECTS, SharedSoms, XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, XfaOutcome, XfaPropSet};
pub(crate) use nodes::{child_named, descendants_named, instance_place, instances, managed_named, resolve_som, scope_of, scoped};
pub use props::{MAX_CHANGES_PER_OBJECT, XfaProp, XfaProps, property_value_ok};
#[cfg(test)]
pub(crate) use run::run_within_limits;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use run::{ENGINE_START_LIMIT, warm_up};
pub(crate) use run::{run_here, run_within};
pub use run::{run_xfa, run_xfa_within};
