//! Legacy SpiderMonkey replacement callbacks use empty strings for unmatched captures.
//! See Mozilla's Firefox 34 release notes (bug 369778). Older PDF rule libraries rely on
//! that behavior. The original intrinsic still performs matching and string replacements.
use crate::host::{arg, error, function, s};
use crate::{Context, JsObject, JsResult, JsValue, NativeFunction, js_string};

pub(crate) fn install(ctx: &mut Context) -> JsResult<()> {
    let prototype = ctx.intrinsics().constructors().string().prototype();
    let original = prototype.get(js_string!("replace"), ctx)?.as_object().ok_or_else(|| error("String.replace is unavailable"))?;
    let replacement = function(ctx, NativeFunction::from_copy_closure_with_captures(replace, original));
    prototype.set(js_string!("replace"), replacement, false, ctx)?;
    Ok(())
}
fn replace(this: &JsValue, args: &[JsValue], original: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let mut args = args.to_vec();
    if let Some(callback) = arg(&args, 1).as_callable() {
        let wrapper = function(ctx, NativeFunction::from_copy_closure_with_captures(captures, callback));
        if let Some(replacement) = args.get_mut(1) {
            *replacement = wrapper.into();
        }
    }
    original.call(this, &args, ctx)
}
fn captures(this: &JsValue, args: &[JsValue], callback: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let mut values = args.to_vec();
    // Tail arguments are offset, input, and optionally the named-groups object.
    let tail = if values.last().is_some_and(JsValue::is_object) { 3 } else { 2 };
    let end = values.len().saturating_sub(tail);
    for (index, value) in values.iter_mut().enumerate() {
        if index > 0 && index < end && value.is_undefined() {
            *value = s("");
        }
    }
    callback.call(this, &values, ctx)
}
