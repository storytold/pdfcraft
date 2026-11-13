//! `this.getOCGs()`: the document's layers (optional content groups) as OCG objects with
//! `name`, `state` and `initState` (#169, ported from #4751). Setting `state` turns a layer on
//! or off as the Layers panel does: how the document is shown, not the document; the caller
//! reads the layers a run turned in [`crate::Outcome::layers`].

use crate::host::{arg, error, function, host, s};
use crate::{Attribute, Context, JsArray, JsResult, JsValue, NativeFunction, ObjectInitializer, js_string};

/// A layer as `getOCGs` shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    /// Shown now (`ocg.state`).
    pub on: bool,
    /// Shown as the file opens (`ocg.initState`).
    pub initial: bool,
}

/// `this.getOCGs(nPage)`: every layer of the document, whatever the page (which pages a layer
/// is on isn't known here), or `null` when it has none.
pub(crate) fn get_ocgs(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let n = host(ctx)?.borrow().doc.layers.len();
    if n == 0 {
        return Ok(JsValue::null());
    }
    let items: Vec<JsValue> = (0..n).map(|i| ocg(ctx, i)).collect();
    Ok(JsArray::from_iter(items, ctx).into())
}

fn layer(c: &Context, i: u32) -> JsResult<Layer> {
    host(c)?.borrow().doc.layers.get(i as usize).cloned().ok_or_else(|| error("the layer is no longer available"))
}

fn ocg(ctx: &mut Context, index: usize) -> JsValue {
    fn name(_: &JsValue, _: &[JsValue], i: &u32, c: &mut Context) -> JsResult<JsValue> {
        Ok(s(&layer(c, *i)?.name))
    }
    fn state(_: &JsValue, _: &[JsValue], i: &u32, c: &mut Context) -> JsResult<JsValue> {
        Ok(JsValue::from(layer(c, *i)?.on))
    }
    fn initial(_: &JsValue, _: &[JsValue], i: &u32, c: &mut Context) -> JsResult<JsValue> {
        Ok(JsValue::from(layer(c, *i)?.initial))
    }
    fn set_state(_: &JsValue, args: &[JsValue], i: &u32, c: &mut Context) -> JsResult<JsValue> {
        let on = arg(args, 0).to_boolean();
        let mut h = host(c)?.borrow_mut();
        let l = h.doc.layers.get_mut(*i as usize).ok_or_else(|| error("the layer is no longer available"))?;
        l.on = on;
        h.layers_changed.insert(*i as usize);
        Ok(JsValue::undefined())
    }
    let i = u32::try_from(index).unwrap_or(u32::MAX);
    let name = function(ctx, NativeFunction::from_copy_closure_with_captures(name, i));
    let state = function(ctx, NativeFunction::from_copy_closure_with_captures(state, i));
    let set_state = function(ctx, NativeFunction::from_copy_closure_with_captures(set_state, i));
    let initial = function(ctx, NativeFunction::from_copy_closure_with_captures(initial, i));
    let rw = Attribute::CONFIGURABLE | Attribute::ENUMERABLE;
    ObjectInitializer::new(ctx)
        .accessor(js_string!("name"), Some(name), None, rw)
        .accessor(js_string!("state"), Some(state), Some(set_state), rw)
        .accessor(js_string!("initState"), Some(initial), None, rw)
        .build()
        .into()
}
