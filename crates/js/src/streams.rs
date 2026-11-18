//! Text streams selected explicitly by the host; scripts never open filesystem paths.
use crate::host::{arg, error, s, text_with_budget};
use crate::{Context, JsResult, JsValue, ObjectInitializer, js_string};
pub(crate) fn read(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let path = arg(args, 0);
    let path = if let Some(o) = path.as_object() { o.get(js_string!("cDIPath"), ctx)? } else { path };
    if !path.is_null_or_undefined() && !text_with_budget(&path, ctx, 4096, "file path is too long")?.is_empty() {
        return Err(error("scripts cannot open paths; omit cDIPath and select a file explicitly"));
    }
    let Some(source) = crate::interaction::ask(crate::interaction::Prompt::File)? else { return Ok(JsValue::null()) };
    Ok(ObjectInitializer::new(ctx).property(js_string!("text"), s(&source), crate::Attribute::READONLY).build().into())
}
pub(crate) fn string(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let stream = arg(args, 0).as_object().ok_or_else(|| error("stringFromStream expects a selected text stream"))?;
    let value = stream.get(js_string!("text"), ctx)?;
    Ok(s(&text_with_budget(&value, ctx, 8 << 20, "text stream exceeds 8 MiB")?))
}
