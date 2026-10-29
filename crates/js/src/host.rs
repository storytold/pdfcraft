use super::model::{DocInfo, FieldState, FieldType, Request};
use super::run::MAX_ARRAY_LENGTH;
use super::{
    Arc, BTreeSet, Context, FunctionObjectBuilder, JsArray, JsError, JsFunction, JsNativeError, JsResult, JsString, JsValue, Message, NativeFunction,
    RefCell, xfa,
};

pub(crate) struct Host {
    pub(crate) fields: Arc<Vec<FieldState>>,
    pub(crate) changed: BTreeSet<usize>,
    pub(crate) alerts: Vec<Message>,
    pub(crate) console: Vec<String>,
    pub(crate) requests: Vec<Request>,
    pub(crate) doc: DocInfo,
    /// Bytes of text kept in alerts, console lines and requests, against [`xfa::MAX_PAYLOAD`].
    pub(crate) kept: usize,
    /// Alerts, console lines and requests the limits left out.
    pub(crate) dropped: usize,
}

impl Host {
    /// Room for `bytes` more bytes of kept text: charged when there is, a drop counted when not.
    fn room(&mut self, bytes: usize) -> bool {
        match self.kept.checked_add(bytes).filter(|k| *k <= xfa::MAX_PAYLOAD) {
            Some(k) => {
                self.kept = k;
                true
            }
            None => {
                self.dropped = self.dropped.saturating_add(1);
                false
            }
        }
    }

    /// Keep message `m`, its text and title charged to the run's byte budget, up to
    /// [`xfa::MAX_ALERTS`] messages.
    pub(crate) fn alert(&mut self, m: Message) {
        if self.alerts.len() >= xfa::MAX_ALERTS {
            self.dropped = self.dropped.saturating_add(1);
        } else if self.room(m.bytes()) {
            self.alerts.push(m);
        }
    }

    pub(crate) fn print(&mut self, m: String) {
        if self.console.len() >= xfa::MAX_CONSOLE {
            self.dropped = self.dropped.saturating_add(1);
        } else if self.room(m.len()) {
            self.console.push(m);
        }
    }

    /// Record `r`, its names and URLs cut to a message's length, up to [`xfa::MAX_EFFECTS`]
    /// requests and the run's byte budget.
    pub(crate) fn request(&mut self, r: Request) {
        if self.requests.len() >= xfa::MAX_EFFECTS {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let r = match r {
            Request::Reset(names) => Request::Reset(names.into_iter().map(xfa::clip).collect()),
            Request::LaunchUrl(u) => Request::LaunchUrl(xfa::clip(u)),
            Request::Submit(u) => Request::Submit(xfa::clip(u)),
            Request::Focus(n) => Request::Focus(xfa::clip(n)),
            other => other,
        };
        let bytes = match &r {
            Request::Reset(names) => names.iter().fold(0usize, |n, s| n.saturating_add(s.len()).saturating_add(std::mem::size_of::<String>())),
            Request::LaunchUrl(u) | Request::Submit(u) | Request::Focus(u) => u.len(),
            _ => 0,
        };
        if self.room(bytes) {
            self.requests.push(r);
        }
    }
}

pub(crate) type Shared = RefCell<Host>;

/// The host state, installed before scripts run.
pub(crate) fn host(ctx: &Context) -> JsResult<&Shared> {
    ctx.get_data::<Shared>().ok_or_else(|| error("the script host is not set up"))
}

pub(crate) fn s(v: &str) -> JsValue {
    JsValue::from(JsString::from(v))
}

pub(crate) fn text(v: &JsValue, ctx: &mut Context) -> JsResult<String> {
    if v.is_undefined() || v.is_null() {
        return Ok(String::new());
    }
    Ok(v.to_string(ctx)?.to_std_string_escaped())
}

/// [`text`] cut to a message's length ([`xfa::clip`]), without first copying all of a longer
/// string.
pub(crate) fn text_line(v: &JsValue, ctx: &mut Context) -> JsResult<String> {
    if v.is_undefined() || v.is_null() {
        return Ok(String::new());
    }
    let js = v.to_string(ctx)?;
    // Two UTF-16 units per character at most: the clip still sees more than a line's worth.
    let keep = xfa::MAX_LINE.saturating_add(1).saturating_mul(2);
    let js = if js.len() > keep { js.get(..keep).unwrap_or_default() } else { js };
    Ok(xfa::clip(js.to_std_string_escaped()))
}

pub(crate) fn arg(args: &[JsValue], i: usize) -> JsValue {
    args.get(i).cloned().unwrap_or_default()
}

pub(crate) fn error(msg: impl Into<String>) -> JsError {
    JsNativeError::typ().with_message(msg.into()).into()
}

/// A JavaScript number for a text value that reads as one, as Acrobat's `field.value` gives.
pub(crate) fn as_number(t: &str) -> Option<f64> {
    let t = t.trim();
    if t.is_empty() || t.starts_with('+') || t.len() > 1 && t.starts_with('0') && !t.starts_with("0.") {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

pub(crate) fn function(ctx: &mut Context, f: NativeFunction) -> JsFunction {
    FunctionObjectBuilder::new(ctx.realm(), f).build()
}

pub(crate) fn strings(v: &JsValue, ctx: &mut Context) -> JsResult<Vec<String>> {
    if let Some(o) = v.as_object()
        && o.is_array()
    {
        let a = JsArray::from_object(o.clone())?;
        let n = a.length(ctx)?;
        if n > MAX_ARRAY_LENGTH {
            return Err(error(format!("array length exceeds {MAX_ARRAY_LENGTH}")));
        }
        let mut out = Vec::new();
        for i in 0..n {
            let x = a.get(i, ctx)?;
            out.push(text(&x, ctx)?);
        }
        return Ok(out);
    }
    Ok(if v.is_undefined() || v.is_null() { Vec::new() } else { vec![text(v, ctx)?] })
}

pub(crate) fn colour(c: &Option<Vec<String>>, ctx: &mut Context) -> JsValue {
    match c {
        None => JsArray::from_iter([s("T")], ctx).into(),
        Some(parts) => {
            let vals: Vec<JsValue> =
                parts.iter().enumerate().map(|(i, p)| if i == 0 { s(p) } else { JsValue::from(p.parse::<f64>().unwrap_or(0.0)) }).collect();
            JsArray::from_iter(vals, ctx).into()
        }
    }
}

// ── field objects ───────────────────────────────────────────────────────────────────────────

fn field_index(ctx: &Context, name: &JsString) -> JsResult<usize> {
    let n = name.to_std_string_escaped();
    host(ctx)?.borrow().fields.iter().position(|f| f.name == n).ok_or_else(|| error(format!("no field named {n}")))
}

pub(crate) fn read(ctx: &Context, name: &JsString) -> JsResult<FieldState> {
    let i = field_index(ctx, name)?;
    Ok(host(ctx)?.borrow().fields[i].clone())
}

pub(crate) fn write(ctx: &Context, name: &JsString, change: impl FnOnce(&mut FieldState)) -> JsResult<()> {
    let i = field_index(ctx, name)?;
    let mut h = host(ctx)?.borrow_mut();
    let field = Arc::make_mut(&mut h.fields).get_mut(i).ok_or_else(|| error("the field is no longer available"))?;
    change(field);
    h.changed.insert(i);
    Ok(())
}

pub(crate) fn value_of(f: &FieldState, ctx: &mut Context) -> JsValue {
    match f.kind {
        FieldType::CheckBox | FieldType::RadioButton => s(f.value.first().map_or("Off", String::as_str)),
        FieldType::ListBox if f.value.len() > 1 => {
            let vals: Vec<JsValue> = f.value.iter().map(|v| s(v)).collect();
            JsArray::from_iter(vals, ctx).into()
        }
        _ => {
            let v = f.single();
            as_number(&v).map_or_else(|| s(&v), JsValue::from)
        }
    }
}

pub(crate) fn set_value(f: &mut FieldState, vals: Vec<String>) {
    f.value = match f.kind {
        FieldType::CheckBox | FieldType::RadioButton => vals.into_iter().filter(|v| v != "Off" && !v.is_empty()).take(1).collect(),
        FieldType::ListBox => vals,
        _ => vals.into_iter().take(1).collect(),
    };
}
