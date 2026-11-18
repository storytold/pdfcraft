//! `toSource()` on objects and arrays, which older forms use to keep state in a field or a
//! global as text (#269, ported from #344). It writes a JavaScript literal of the value,
//! including source functions, regular expressions and dates used by imported form rules.
//! Native functions and cycles are refused, and
//! so is a value past the depth, size or length bounds, rather than writing something that
//! reads as another value.

use super::host::{error, function, s, text};
use super::{Context, JsArray, JsObject, JsResult, JsValue, NativeFunction, js_string};
use boa_engine::property::{PropertyDescriptor, PropertyKey};

/// Deepest nesting written.
const MAX_DEPTH: usize = 64;
/// Most values (each array item, property and the value itself) written.
const MAX_VALUES: usize = 100_000;
/// Longest source written, in bytes.
const MAX_BYTES: usize = 16 << 20;
const MAX_RESULT_BYTES: usize = 8 << 20;

/// Install `toSource` on `Object.prototype` (arrays inherit it), not enumerable, as in the
/// engines that had it.
pub(crate) fn install(ctx: &mut Context) -> JsResult<()> {
    let to_source = function(ctx, NativeFunction::from_fn_ptr(to_source));
    let prototype = ctx.intrinsics().constructors().object().prototype();
    prototype
        .insert_property(js_string!("toSource"), PropertyDescriptor::builder().value(to_source).writable(true).configurable(true).enumerable(false));
    Ok(())
}

fn to_source(this: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let mut writer = Writer { values: MAX_VALUES, bytes: MAX_BYTES, open: Vec::new() };
    let out = writer.value(this, ctx)?;
    if out.len() > MAX_RESULT_BYTES {
        return Err(error("toSource: the result would be too long"));
    }
    Ok(s(&out))
}

/// What is left of the bounds, and the objects being written (to refuse a cycle).
struct Writer {
    values: usize,
    bytes: usize,
    open: Vec<JsObject>,
}

impl Writer {
    fn value(&mut self, v: &JsValue, ctx: &mut Context) -> JsResult<String> {
        self.values = self.values.checked_sub(1).ok_or_else(|| error("toSource: the value has too many parts"))?;
        let out = if v.is_callable() {
            let prototype = ctx.intrinsics().constructors().function().prototype();
            let stringify =
                prototype.get(js_string!("toString"), ctx)?.as_callable().ok_or_else(|| error("toSource: function source is unavailable"))?;
            let source = crate::host::text_with_budget(&stringify.call(v, &[], ctx)?, ctx, MAX_RESULT_BYTES, "toSource: function is too long")?;
            if source.contains("[native code]") {
                return Err(error("toSource: native functions can't be written"));
            }
            // Object methods stringify as `name(args) { ... }`, not an expression.
            // Parse only: a method is not a standalone expression, whereas an arrow or
            // function is. Nested arrows and literal text must not classify its outer form.
            let expression = format!("({source})");
            let source = if boa_engine::Script::parse(boa_engine::Source::from_bytes(&expression), None, ctx).is_ok() {
                source
            } else if let Some(method) = source.strip_prefix("async ") {
                format!("async function {method}")
            } else {
                format!("function {source}")
            };
            format!("({source})")
        } else if let Some(o) = v.as_object() {
            if let Ok(regex) = boa_engine::object::builtins::JsRegExp::from_object(o.clone()) {
                let mut pattern = String::new();
                let mut escaped = false;
                let source = regex.source(ctx)?;
                if source.len() > MAX_RESULT_BYTES {
                    return Err(error("toSource: regular expression is too long"));
                }
                for c in source.chars() {
                    match c {
                        '/' if !escaped => pattern.push_str("\\/"),
                        '\n' => pattern.push_str("\\n"),
                        '\r' => pattern.push_str("\\r"),
                        '\u{2028}' => pattern.push_str("\\u2028"),
                        '\u{2029}' => pattern.push_str("\\u2029"),
                        _ => pattern.push(c),
                    }
                    escaped = c == '\\' && !escaped;
                }
                format!("/{pattern}/{}", regex.flags(ctx)?)
            } else if let Ok(date) = crate::JsDate::from_object(o.clone()) {
                format!("(new Date({}))", text(&date.get_time(ctx)?, ctx)?)
            } else if o.is::<boa_engine::builtins::error::Error>() {
                let name = crate::host::text_with_budget(&o.get(js_string!("name"), ctx)?, ctx, 256, "toSource: error name is too long")?;
                let name = match name.as_str() {
                    "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError" | "EvalError" | "URIError" => name.as_str(),
                    _ => "Error",
                };
                let message =
                    crate::host::text_with_budget(&o.get(js_string!("message"), ctx)?, ctx, MAX_RESULT_BYTES, "toSource: error message is too long")?;
                format!("(new {name}({}))", quote(&message))
            } else {
                self.object(&o, ctx)?
            }
        } else if v.is_string() {
            quote(&crate::host::text_with_budget(v, ctx, self.bytes.min(MAX_RESULT_BYTES), "toSource: the result would be too long")?)
        } else if v.is_null() {
            "null".into()
        } else if v.is_undefined() {
            "undefined".into()
        } else {
            text(v, ctx)?
        };
        self.charge(out.len())?;
        Ok(out)
    }

    fn object(&mut self, o: &JsObject, ctx: &mut Context) -> JsResult<String> {
        if self.open.iter().any(|open| open == o) {
            return Err(error("toSource: the value refers to itself"));
        }
        if self.open.len() >= MAX_DEPTH {
            return Err(error("toSource: the value is nested too deeply"));
        }
        if !o.is_array() && !plain(o, ctx) {
            return Err(error("toSource: only plain objects, arrays, strings, numbers, booleans, null and undefined can be written"));
        }
        self.open.push(o.clone());
        let out = if o.is_array() { self.array(o, ctx) } else { self.fields(o, ctx) };
        self.open.pop();
        out
    }

    fn array(&mut self, o: &JsObject, ctx: &mut Context) -> JsResult<String> {
        let array = JsArray::from_object(o.clone())?;
        let len = array.length(ctx)?;
        if len > self.values as u64 {
            return Err(error("toSource: the array is too long"));
        }
        let mut items = Vec::new();
        for i in 0..len {
            items.push(self.value(&array.get(i, ctx)?, ctx)?);
        }
        Ok(format!("[{}]", items.join(", ")))
    }

    fn fields(&mut self, o: &JsObject, ctx: &mut Context) -> JsResult<String> {
        let mut items = Vec::new();
        for key in o.own_property_keys(ctx)? {
            let label = match &key {
                PropertyKey::Symbol(_) => continue,
                PropertyKey::String(k) => k.to_std_string_escaped(),
                PropertyKey::Index(i) => i.get().to_string(),
            };
            // `"__proto__": v` in a literal sets the prototype rather than an own property.
            if label == "__proto__" {
                return Err(error("toSource: a property named __proto__ can't be written"));
            }
            self.charge(label.len())?;
            let value = self.value(&o.get(key, ctx)?, ctx)?;
            items.push(format!("{}:{value}", quote(&label)));
        }
        Ok(format!("({{{}}})", items.join(", ")))
    }

    /// Count `n` bytes against the length bound. Every part counts, including the parts of what
    /// is joined later, so a value repeated many times can't build a long result first.
    fn charge(&mut self, n: usize) -> JsResult<()> {
        self.bytes = self.bytes.checked_sub(n).ok_or_else(|| error("toSource: the result would be too long"))?;
        Ok(())
    }
}

/// Whether `o` is a plain object: an ordinary one whose prototype is `Object.prototype` (or
/// none), so its own properties are all there is to it. A Date, a RegExp, a boxed value, a
/// proxy or a class instance holds more than a literal of its properties would give back.
fn plain(o: &JsObject, ctx: &Context) -> bool {
    let object_prototype = ctx.intrinsics().constructors().object().prototype();
    o.is_ordinary() && o.prototype().is_none_or(|p| p == object_prototype)
}

/// `text` as a double-quoted JavaScript string literal.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
