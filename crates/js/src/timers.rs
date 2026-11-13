//! `app.setInterval`, `app.setTimeOut`, `app.clearInterval` and `app.clearTimeOut` in a
//! document's retained realm (#169, ported from #4751). The realm keeps the timers its scripts
//! set and reports them in every [`crate::Outcome`]; the caller owns the clock and fires one
//! by running a [`crate::session::Job`] with its `timer` when it falls due, in the same realm. A
//! one-off run (a fresh context) keeps none: there the functions return an object and nothing
//! ever fires, as before.

use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::host::{arg, error, text};
use crate::{Context, JsObject, JsResult, JsValue, MAX_SCRIPT_BYTES, ObjectInitializer};

/// Most timers one document's scripts keep at once; setting another is an error.
pub const MAX_TIMERS: usize = 64;
/// The shortest period, in milliseconds: a timer asked to run sooner or more often runs at this.
pub const MIN_TIMER_MS: u32 = 10;

/// A timer a script set, as the realm reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    /// The realm's name for it, never reused while the realm lives.
    pub id: u32,
    /// Milliseconds between runs (`setInterval`) or before the one run (`setTimeOut`), at least
    /// [`MIN_TIMER_MS`].
    pub period_ms: u32,
    /// `setInterval`: it runs until it is cleared. `setTimeOut`: once.
    pub repeat: bool,
}

/// What a timer runs: its expression, or a function the script gave instead.
#[derive(Clone)]
pub(crate) enum Code {
    Source(String),
    Function(JsObject),
}

struct Entry {
    /// The object `set*` returned, which `clear*` is given back.
    handle: JsObject,
    code: Code,
    period_ms: u32,
    repeat: bool,
}

/// A realm's timers by id, in the order they were set (ids only grow).
#[derive(Default)]
pub(crate) struct Timers {
    last: u32,
    table: BTreeMap<u32, Entry>,
}

type Shared = RefCell<Timers>;

/// Let the timers of a retained realm run: the functions keep what they are given from now on.
pub(crate) fn enable(ctx: &mut Context) {
    if ctx.get_data::<Shared>().is_none() {
        ctx.insert_data(RefCell::new(Timers::default()));
    }
}

/// The realm's timers as they stand, in the order they were set; none in a one-off run.
pub(crate) fn report(ctx: &Context) -> Vec<Timer> {
    ctx.get_data::<Shared>()
        .map(|t| t.borrow().table.iter().map(|(id, e)| Timer { id: *id, period_ms: e.period_ms, repeat: e.repeat }).collect())
        .unwrap_or_default()
}

/// What timer `id` runs now that it fell due; a `setTimeOut` timer is gone after this. `None`
/// when it was cleared (or the realm has no timers).
pub(crate) fn due(ctx: &Context, id: u32) -> Option<Code> {
    let mut timers = ctx.get_data::<Shared>()?.borrow_mut();
    let entry = timers.table.get(&id)?;
    let code = entry.code.clone();
    if !entry.repeat {
        timers.table.remove(&id);
    }
    Some(code)
}

/// `app.setInterval(cExpr, nMilliseconds)`: run the expression (or a function) every period
/// until it is cleared.
pub(crate) fn set_interval(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    set(args, true, ctx)
}

/// `app.setTimeOut(cExpr, nMilliseconds)`: run the expression (or a function) once, after the
/// period.
pub(crate) fn set_time_out(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    set(args, false, ctx)
}

fn set(args: &[JsValue], repeat: bool, ctx: &mut Context) -> JsResult<JsValue> {
    let handle = ObjectInitializer::new(ctx).build();
    if ctx.get_data::<Shared>().is_none() {
        return Ok(handle.into());
    }
    let what = arg(args, 0);
    let code = match what.as_callable() {
        Some(f) => Code::Function(f.clone()),
        None => {
            let source = text(&what, ctx)?;
            if source.len() > MAX_SCRIPT_BYTES {
                return Err(error(format!("the timer's script is too long (the limit is {} KiB)", MAX_SCRIPT_BYTES / 1024)));
            }
            Code::Source(source)
        }
    };
    let ms = arg(args, 1).to_number(ctx)?;
    // A missing, negative or NaN period runs as soon as allowed; the float cast saturates.
    let period_ms = if ms.is_nan() { MIN_TIMER_MS } else { (ms.min(f64::from(u32::MAX)) as u32).max(MIN_TIMER_MS) };
    let Some(timers) = ctx.get_data::<Shared>() else { return Ok(handle.into()) };
    let mut timers = timers.borrow_mut();
    if timers.table.len() >= MAX_TIMERS {
        return Err(error(format!("a document's scripts can keep at most {MAX_TIMERS} timers")));
    }
    let id = timers.last.checked_add(1).ok_or_else(|| error("no more timers can be set"))?;
    timers.last = id;
    timers.table.insert(id, Entry { handle: handle.clone(), code, period_ms, repeat });
    Ok(handle.into())
}

/// `app.clearInterval(oInterval)` and `app.clearTimeOut(oTime)`: forget the timer the object
/// stands for. Anything else, or a timer already gone, is ignored.
pub(crate) fn clear(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    if let Some(o) = arg(args, 0).as_object()
        && let Some(timers) = ctx.get_data::<Shared>()
    {
        timers.borrow_mut().table.retain(|_, e| !JsObject::equals(&e.handle, &o));
    }
    Ok(JsValue::undefined())
}
