//! Layer (optional content) visibility: what the view shows and what printing writes.

use std::sync::Arc;

use pdfcraft_render::{Layer, LayerOp, RenderPool};

use crate::{Document, display_config, render_threads};

/// Most usage application dictionaries (`/AS` entries) that [`print_layer_states`] reads, and
/// most category names and groups it reads in all of them together (entries can share one
/// array, so the count is over every entry, not each).
const MAX_AUTO_STATES: usize = 1024;
const MAX_AUTO_ITEMS: usize = 65_536;

/// The layers as they print: `layers` as shown, then the default configuration's auto state for
/// the Print event (ISO 32000-2 §8.11.4.4, Table 101). Each usage application dictionary
/// (`/AS` entry) whose `/Event` is `/Print` and whose `/Category` includes `/Print` recommends,
/// for each group in its `/OCGs`, the group's `/Usage /Print /PrintState`. A group with
/// recommendations is on only if all of them are ON; other groups keep their state. Categories
/// other than Print give no recommendation here. `None` when no `/AS` entry is for printing.
pub(crate) fn print_layer_states(cos: &pdfcraft_cos::Document, layers: &[Layer]) -> Option<Vec<Layer>> {
    use pdfcraft_cos::Object;
    let root = cos.trailer().get(b"Root").map(|r| cos.resolve(r))?;
    let properties = root.as_dict()?.get(b"OCProperties").map(|p| cos.resolve(p))?;
    let config = properties.as_dict()?.get(b"D").map(|d| cos.resolve(d))?;
    let auto = config.as_dict()?.get(b"AS").map(|a| cos.resolve(a))?;
    let mut printing = false;
    let mut recommended: std::collections::HashMap<(u32, u16), bool> = std::collections::HashMap::new();
    let mut items_left = MAX_AUTO_ITEMS;
    for entry in auto.as_array()?.iter().take(MAX_AUTO_STATES) {
        let entry = cos.resolve(entry);
        let Some(entry) = entry.as_dict() else { continue };
        if entry.get(b"Event").map(|e| cos.resolve(e)).as_deref().and_then(Object::as_name) != Some(b"Print") {
            continue;
        }
        printing = true;
        let categories = entry.get(b"Category").map(|c| cos.resolve(c));
        let for_print = match categories.as_deref() {
            Some(Object::Array(names)) => {
                let read = names.len().min(items_left);
                items_left -= read;
                names.iter().take(read).any(|n| cos.resolve(n).as_name() == Some(b"Print"))
            }
            Some(Object::Name(n)) => n.as_slice() == b"Print",
            _ => false,
        };
        if !for_print {
            continue;
        }
        let Some(groups) = entry.get(b"OCGs").map(|g| cos.resolve(g)) else { continue };
        for group in groups.as_array().into_iter().flatten() {
            let Some(left) = items_left.checked_sub(1) else { break };
            items_left = left;
            let Some(id) = group.as_ref() else { continue };
            let ocg = cos.resolve(group);
            let state = ocg
                .as_dict()
                .and_then(|g| g.get(b"Usage").map(|u| cos.resolve(u)))
                .and_then(|u| u.as_dict().and_then(|u| u.get(b"Print").map(|p| cos.resolve(p))))
                .and_then(|p| p.as_dict().and_then(|p| p.get(b"PrintState").map(|s| cos.resolve(s))))
                .and_then(|s| match s.as_name() {
                    Some(b"ON") => Some(true),
                    Some(b"OFF") => Some(false),
                    _ => None,
                });
            if let Some(on) = state {
                let all_on = recommended.entry((id.num, id.generation)).or_insert(true);
                *all_on = *all_on && on;
            }
        }
    }
    printing.then(|| layers.iter().map(|l| Layer { visible: recommended.get(&l.id).copied().unwrap_or(l.visible), ..l.clone() }).collect())
}

/// Write `layers`' visibility into the default optional-content configuration of `cos` (a copy
/// made for printing): `/BaseState /ON`, `/ON` the visible layers, `/OFF` the hidden ones. The
/// states written are final, so what would set them again goes: the auto state (`/AS`), and the
/// layers' own `/Usage`, which some readers apply when they draw (a `/ViewState /ON` turns a layer
/// that prints off back on). The configuration's other entries (`/Order`, `/RBGroups`…) are kept.
/// A document without `/OCProperties` is left alone.
pub(crate) fn set_default_layer_state(cos: &mut pdfcraft_cos::Document, layers: &[Layer]) {
    use pdfcraft_cos::{Dict, ObjRef, Object};
    let Some(root_ref) = cos.trailer().get(b"Root").and_then(Object::as_ref) else { return };
    let Some(mut root) = cos.get(root_ref).as_dict().cloned() else { return };
    let Some(properties) = root.get(b"OCProperties").cloned() else { return };
    let Some(mut properties) = cos.resolve(&properties).as_dict().cloned() else { return };
    let mut config = properties.get(b"D").and_then(|d| cos.resolve(d).as_dict().cloned()).unwrap_or_else(Dict::new);
    let refs =
        |visible: bool| Object::Array(layers.iter().filter(|l| l.visible == visible).map(|l| Object::Ref(ObjRef::new(l.id.0, l.id.1))).collect());
    config.set(b"BaseState".to_vec(), Object::name("ON"));
    config.set(b"ON".to_vec(), refs(true));
    config.set(b"OFF".to_vec(), refs(false));
    config.remove(b"AS");
    properties.set(b"D".to_vec(), Object::Dict(config));
    root.set(b"OCProperties".to_vec(), Object::Dict(properties));
    cos.set(root_ref, Object::Dict(root));
    for layer in layers {
        let id = ObjRef::new(layer.id.0, layer.id.1);
        let Some(mut group) = cos.get(id).as_dict().cloned() else { continue };
        if group.remove(b"Usage").is_some() {
            cos.set(id, Object::Dict(group));
        }
    }
}

/// Render `doc` with the layer visibility in its `info.layers`.
pub(crate) fn use_layer_choices(doc: &mut Document) {
    let overrides: Vec<(i32, i32, bool)> = doc.info.layers.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect();
    doc.config.layers = Arc::new(overrides);
    doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), display_config(&doc.config));
}

/// Apply a set-layer-visibility action to `layers`, one change at a time, so a toggle flips the
/// state the changes before it left. Returns whether any layer ended up changed.
pub(crate) fn apply_layer_state(layers: &mut [Layer], groups: &[Vec<(u32, u16)>], changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) -> bool {
    let before: Vec<bool> = layers.iter().map(|l| l.visible).collect();
    let index: std::collections::HashMap<(u32, u16), usize> = layers.iter().enumerate().map(|(i, l)| (l.id, i)).collect();
    for &(op, ocg) in changes {
        let Some(layer) = index.get(&ocg).and_then(|&i| layers.get_mut(i)) else { continue };
        let on = match op {
            LayerOp::On => true,
            LayerOp::Off => false,
            LayerOp::Toggle => !layer.visible,
        };
        layer.visible = on;
        // Turning a layer off leaves the rest of its groups alone.
        if on && preserve_rb {
            for other in groups.iter().filter(|g| g.contains(&ocg)).flatten().filter(|&&o| o != ocg) {
                if let Some(l) = index.get(other).and_then(|&i| layers.get_mut(i)) {
                    l.visible = false;
                }
            }
        }
    }
    layers.iter().zip(before).any(|(l, was)| l.visible != was)
}
