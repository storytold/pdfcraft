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
pub(crate) use nodes::{descendants_named, instance_source, instances, resolve_manager, resolve_som};
#[cfg(test)]
pub(crate) use run::run_within_limits;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use run::{ENGINE_START_LIMIT, warm_up};
pub(crate) use run::{run_here, run_within};
pub use run::{run_xfa, run_xfa_within};

#[cfg(test)]
mod zero_instance_limits {
    use super::{MAX_EFFECTS, MAX_PAYLOAD, XHost, XfaDoc, XfaEffect, XfaNode, instance_source, instances};

    /// A failed bounded clone must roll back arena allocations and emit no structural effect.
    #[test]
    fn zero_instance_clone_failure_leaves_no_partial_row_or_effect() {
        let row = XfaNode { name: "row".into(), repeatable: true, occur_min: 0, ..Default::default() };
        let root = XfaNode { name: "root".into(), instance_templates: vec![row], ..Default::default() };
        let mut host = XHost::new(root, "", &XfaDoc::default());
        let row = instance_source(&host, 1, "row").unwrap();
        // Exercise the clone depth guard without allocating MAX_NODES test objects.
        host.nodes[row].children.push(row);
        let before = host.nodes.len();
        assert_eq!(host.add_instance(1, "row"), None);
        assert_eq!(host.nodes.len(), before);
        assert!(instances(&host, 1, "row").is_empty());
        assert!(host.effects.is_empty());
    }

    /// A dormant definition must not bypass upstream's reserve-before-mutation effect cap.
    #[test]
    fn zero_instance_add_past_effect_cap_keeps_the_arena_and_row_count() {
        let row = XfaNode { name: "row".into(), repeatable: true, occur_min: 0, ..Default::default() };
        let root = XfaNode { name: "root".into(), instance_templates: vec![row], ..Default::default() };
        let mut host = XHost::new(root, "", &XfaDoc::default());
        for _ in 0..MAX_EFFECTS {
            host.push(XfaEffect::Beep);
        }
        let before = host.nodes.len();
        assert_eq!(host.add_instance(1, "row"), None);
        assert_eq!(host.nodes.len(), before);
        assert!(instances(&host, 1, "row").is_empty());
        assert_eq!(host.effects.len(), MAX_EFFECTS);
        assert!(host.effects.iter().all(|effect| matches!(effect, XfaEffect::Beep)));
    }

    /// Copied defaults include raw and formatted UTF-8 bytes, including dormant descendants.
    /// A rejection after copying the parent must remove that partial subtree and its effect.
    #[test]
    fn cloned_defaults_spend_the_payload_budget_before_allocating_strings() {
        for dormant_child in [false, true] {
            for remaining in [4, 5] {
                let field = XfaNode { name: "value".into(), value: "é".into(), formatted: Some("€".into()), ..Default::default() };
                let row = XfaNode {
                    name: "row".into(),
                    repeatable: true,
                    children: if dormant_child { vec![] } else { vec![field.clone()] },
                    instance_templates: if dormant_child { vec![field] } else { vec![] },
                    ..Default::default()
                };
                let root = XfaNode { name: "root".into(), instance_templates: vec![row], ..Default::default() };
                let mut host = XHost::new(root, "", &XfaDoc::default());
                // Reserve all but the structural effect and 4 or 5 UTF-8 value bytes, without
                // allocating a 32 MiB fixture. A clone needs 2 raw + 3 formatted bytes.
                assert!(host.reserve(&XfaEffect::Beep, MAX_PAYLOAD - "row[0]".len() - remaining));
                let before = host.nodes.len();
                let added = host.add_instance(1, "row");
                if remaining == 4 {
                    assert_eq!(added, None, "dormant child: {dormant_child}");
                    assert_eq!(host.nodes.len(), before);
                    assert!(instances(&host, 1, "row").is_empty());
                    assert!(host.effects.is_empty());
                    assert!(host.notes().iter().any(|note| note.contains("MiB")));
                } else {
                    let added = added.expect("the exact budget must allow the row");
                    assert_eq!(instances(&host, 1, "row"), vec![added]);
                    assert_eq!(host.effects, vec![XfaEffect::AddInstance { som: "row[0]".into() }]);
                    let children = if dormant_child { &host.nodes[added].instance_templates } else { &host.nodes[added].children };
                    let value = &host.nodes[children[0]];
                    assert_eq!(value.value, "é");
                    assert_eq!(value.formatted.as_deref(), Some("€"));
                    // None of the budget remains: another clone cannot modify the arena.
                    let after = host.nodes.len();
                    assert_eq!(host.add_instance(1, "row"), None);
                    assert_eq!(host.nodes.len(), after);
                    assert_eq!(host.effects.len(), 1);
                }
            }
        }
    }
}
