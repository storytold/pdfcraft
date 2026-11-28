//! Form data methods operate on the current script snapshot. The host owns the codecs and paths.
use std::collections::{HashMap, HashSet};

use crate::host::{arg, error, host, s, text_with_budget, value_for};
use crate::{Context, FieldType, JsArray, JsResult, JsValue, Outcome, js_string};

pub type Values = Vec<(String, Vec<String>)>;
pub type PickResult = Result<Option<Values>, String>;
pub type Selection = std::sync::Arc<dyn Fn(Picker) -> PickResult + Send + Sync>;
pub const MAX_BYTES: usize = 8 << 20;

#[derive(Clone, Copy, Debug)]
pub enum Format {
    Fdf,
    Xfdf,
}

pub enum Picker {
    Import(Format),
    Export(String),
}

#[derive(Clone, Debug)]
pub struct Codec {
    pub may_copy: bool,
    pub write_xfdf: fn(Values, &str) -> String,
}

impl PartialEq for Codec {
    fn eq(&self, other: &Self) -> bool {
        self.may_copy == other.may_copy && std::ptr::fn_addr_eq(self.write_xfdf, other.write_xfdf)
    }
}

#[derive(Default)]
pub(crate) struct Transaction {
    failed: bool,
    cancelled: bool,
}

pub(crate) fn finish(ctx: &mut Context, out: &mut Outcome) {
    if let Some(transaction) = ctx.remove_data::<Transaction>() {
        if transaction.failed && out.error.is_none() {
            out.error = Some("script form data operation failed".into());
        }
        out.execution_refused |= transaction.cancelled || transaction.failed || out.error.is_some();
    }
}

fn parameter(args: &[JsValue], index: usize, key: &'static str, ctx: &mut Context) -> JsResult<JsValue> {
    let first = arg(args, 0);
    match first.as_object() {
        Some(options) => options.get(js_string!(key), ctx),
        None => Ok(arg(args, index)),
    }
}

fn reject_path(path: &JsValue, ctx: &mut Context) -> JsResult<()> {
    if !text_with_budget(path, ctx, 4096, "file path is too long")?.is_empty() {
        return Err(error("scripts cannot open or save paths; omit cPath and select a file explicitly"));
    }
    Ok(())
}

fn pick(ctx: &mut Context, request: Picker) -> JsResult<Option<Values>> {
    if !crate::selected_pdf::primary(ctx) {
        return Err(error("form data selection requires the active document"));
    }
    {
        let h = host(ctx)?.borrow();
        if h.calculate_depth != 0 || !matches!(h.event_name.as_str(), "Console" | "Mouse Up") {
            return Err(error("form data selection requires a button or console script"));
        }
    }
    if ctx.get_data::<Transaction>().is_some_and(|state| state.failed || state.cancelled) {
        return Ok(None);
    }
    let answer = crate::interaction::form_data(request);
    ctx.insert_data(Transaction { failed: answer.is_err(), cancelled: matches!(answer, Ok(None)) });
    answer
}

fn import(args: &[JsValue], ctx: &mut Context, format: Format) -> JsResult<JsValue> {
    let path = parameter(args, 0, "cPath", ctx)?;
    reject_path(&path, ctx)?;
    let Some(values) = pick(ctx, Picker::Import(format))? else { return Ok(false.into()) };
    let result = apply_import(ctx, values);
    if result.is_err() {
        ctx.insert_data(Transaction { failed: true, cancelled: false });
    }
    result.map(|()| true.into())
}

fn apply_import(ctx: &Context, values: Values) -> JsResult<()> {
    let mut h = host(ctx)?.borrow_mut();
    let mut by_name = HashMap::with_capacity(h.fields.len());
    for (i, field) in h.fields.iter().enumerate() {
        by_name.entry(field.name.as_str()).or_insert(i);
    }
    let mut updates = HashMap::new();
    for (name, mut values) in values {
        let Some(&i) = by_name.get(name.as_str()) else { continue };
        let field = &h.fields[i];
        match field.kind {
            FieldType::Button | FieldType::Signature => continue,
            FieldType::ComboBox | FieldType::ListBox => values.retain(|v| !v.is_empty()),
            FieldType::CheckBox | FieldType::RadioButton => {
                if let Some(value) = values.first_mut()
                    && let Some((state, _)) = field.button_exports.iter().find(|(_, export)| export == value)
                {
                    value.clone_from(state);
                }
            }
            FieldType::Text if values.is_empty() => values.push(String::new()),
            FieldType::Text => {}
        }
        updates.insert(i, value_for(field.kind, values));
    }
    let mut freed = 0usize;
    let mut added = 0usize;
    updates.retain(|i, values| {
        let old = &h.fields[*i].value;
        if old == values {
            return false;
        }
        freed = freed.saturating_add(crate::host::text_bytes(old));
        added = added.saturating_add(crate::host::text_bytes(values));
        true
    });
    let retained = h.retained.saturating_sub(freed).saturating_add(added);
    if retained > crate::xfa::MAX_PAYLOAD {
        return Err(error("imported form values exceed the script text limit"));
    }
    if !updates.is_empty() {
        h.retained = retained;
        h.calculations_current = false;
        for (i, values) in updates {
            std::sync::Arc::make_mut(&mut h.fields)[i].value = values;
            h.changed.insert(i);
        }
    }
    Ok(())
}

pub(crate) fn import_fdf(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    import(args, ctx, Format::Fdf)
}

pub(crate) fn import_xfdf(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    import(args, ctx, Format::Xfdf)
}

fn selected_fields(value: &JsValue, ctx: &mut Context) -> JsResult<Option<HashSet<String>>> {
    if value.is_null_or_undefined() {
        return Ok(None);
    }
    let Some(array) = value.as_object().filter(|o| o.is_array()) else {
        return Ok(Some(HashSet::from([text_with_budget(value, ctx, MAX_BYTES, "field name is too long")?])));
    };
    let array = JsArray::from_object(array.clone())?;
    let count = array.length(ctx)?;
    if count > crate::run::MAX_ARRAY_LENGTH {
        return Err(error("too many selected field names"));
    }
    let mut names = HashSet::new();
    let mut remaining = MAX_BYTES;
    for i in 0..count {
        let name = text_with_budget(&array.get(i, ctx)?, ctx, remaining, "selected field names exceed their limit")?;
        remaining = remaining.saturating_sub(name.len() + std::mem::size_of::<String>());
        names.insert(name);
    }
    Ok(Some(names))
}

fn export(args: &[JsValue], ctx: &mut Context, to_file: bool) -> JsResult<String> {
    let all = parameter(args, 0, "bAllFields", ctx)?.to_boolean();
    let no_password = parameter(args, 1, "bNoPassword", ctx)?;
    let no_password = no_password.is_undefined() || no_password.to_boolean();
    let names = selected_fields(&parameter(args, 2, "aFields", ctx)?, ctx)?;
    let href = if to_file {
        reject_path(&parameter(args, 3, "cPath", ctx)?, ctx)?;
        JsValue::undefined()
    } else {
        parameter(args, 4, "cHRef", ctx)?
    };
    if parameter(args, if to_file { 4 } else { 3 }, "bAnnotations", ctx)?.to_boolean() {
        return Err(error("script XFDF export supports form values only"));
    }
    let href = (!href.is_null_or_undefined()).then(|| text_with_budget(&href, ctx, 4096, "XFDF reference is too long")).transpose()?;
    let mut h = host(ctx)?.borrow_mut();
    if h.form_data_work >= crate::xfa::MAX_PAYLOAD {
        return Err(error("script XFDF exports exceed the invocation text limit"));
    }
    let codec = h.doc.form_data.as_ref().ok_or_else(|| error("XFDF export is unavailable"))?;
    if !codec.may_copy {
        return Err(error("document permissions prohibit exporting form data"));
    }
    let mut values = Vec::new();
    let mut remaining = MAX_BYTES;
    for field in h.fields.iter() {
        if matches!(field.kind, FieldType::Button | FieldType::Signature)
            || (no_password && field.password)
            || (!all && field.value.iter().all(String::is_empty))
            || names
                .as_ref()
                .is_some_and(|names| !names.contains(&field.name) && !field.name.match_indices('.').any(|(i, _)| names.contains(&field.name[..i])))
        {
            continue;
        }
        let size = field.name.len().saturating_add(crate::host::text_bytes(&field.value)).saturating_add(64);
        remaining = remaining.checked_sub(size).ok_or_else(|| error("XFDF export exceeds its size limit"))?;
        let mut field_values = field.value.clone();
        if matches!(field.kind, FieldType::CheckBox | FieldType::RadioButton) {
            if let Some(value) = field_values.first_mut() {
                if let Some((_, export)) = field.button_exports.iter().find(|(state, _)| state == value) {
                    remaining =
                        remaining.checked_sub(export.len().saturating_sub(value.len())).ok_or_else(|| error("XFDF export exceeds its size limit"))?;
                    value.clone_from(export);
                }
            } else {
                field_values.push("Off".into());
            }
        } else if field_values.is_empty() {
            field_values.push(String::new());
        }
        values.push((field.name.clone(), field_values));
    }
    let xml = (codec.write_xfdf)(values, href.as_deref().unwrap_or(&h.doc.file_name));
    h.form_data_work = h.form_data_work.saturating_add(xml.len());
    if xml.len() > MAX_BYTES {
        return Err(error("XFDF export exceeds its size limit"));
    }
    if h.form_data_work > crate::xfa::MAX_PAYLOAD {
        return Err(error("script XFDF exports exceed the invocation text limit"));
    }
    Ok(xml)
}

pub(crate) fn export_file(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let xml = export(args, ctx, true)?;
    Ok(pick(ctx, Picker::Export(xml))?.is_some().into())
}

pub(crate) fn export_string(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(s(&export(args, ctx, false)?))
}
