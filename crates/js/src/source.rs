//! `toSource()` on objects and arrays, which older forms use to keep state in a field or a
//! global as text (#269, ported from #344). It writes a JavaScript literal of the value; scripts
//! here can't `eval` it back, since strings never compile. Plain data only: strings, numbers,
//! booleans, null, undefined, arrays and objects of them. Functions and cycles are refused, and
//! so is a value past the depth, size or length bounds, rather than writing something that
//! reads as another value.

use super::host::{error, function, s, text};
use super::{Context, JsArray, JsObject, JsResult, JsValue, NativeFunction, js_string};
use boa_engine::property::{PropertyDescriptor, PropertyKey};

/// Deepest nesting written.
const MAX_DEPTH: usize = 64;
/// Most values (each array item, property and the value itself) written.
const MAX_VALUES: usize = 10_000;
/// Longest source written, in bytes.
const MAX_BYTES: usize = 1 << 20;

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
            return Err(error("toSource: functions can't be written"));
        } else if let Some(o) = v.as_object() {
            self.object(&o, ctx)?
        } else if let Some(text) = v.as_string() {
            quote(&text.to_std_string_escaped())
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
