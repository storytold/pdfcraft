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

use crate::{Limits, Message, arg, as_number, error, function, platform, printd, printf, printx, refuse, s, scand, text};

mod globals;
mod host;
mod instances;
mod model;
mod nodes;
mod run;

pub(crate) use host::{HNode, XHost};
pub use model::{MAX_ALERTS, MAX_CONSOLE, MAX_EFFECTS, XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, XfaOutcome};
pub(crate) use model::{MAX_LINE, MAX_PAYLOAD, clip, clip_result, reset_names};
pub(crate) use nodes::{child_named, descendants_named, instances, resolve_som};
#[cfg(test)]
pub(crate) use run::run_within_limits;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use run::{ENGINE_START_LIMIT, warm_up};
pub(crate) use run::{run_here, run_within};
pub use run::{run_xfa, run_xfa_within};
