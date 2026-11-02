//! Bounded synchronous script prompts. The worker waits; the UI never evaluates script text.
use crate::host::{arg, error, s, text};
use crate::{Context, JsArray, JsResult, JsValue, Outcome, js_string};
use std::cell::RefCell;
use std::sync::mpsc::{self, Sender};
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub name: String,
    pub value: String,
    pub enabled: bool,
    pub marked: bool,
    pub children: Vec<MenuItem>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    Menu(Vec<MenuItem>),
    Text { title: String, question: String, default: String, password: bool, label: String },
    Alert(crate::Message),
}
pub enum InteractiveEvent {
    Prompt(Prompt, Sender<Option<String>>),
    Complete(Outcome),
}
thread_local! { static HOST:RefCell<Option<(Sender<InteractiveEvent>,usize)>>=const {RefCell::new(None)}; }
pub(crate) fn has_host() -> bool {
    HOST.with(|h| h.borrow().is_some())
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_host(host: Option<Sender<InteractiveEvent>>) {
    HOST.with(|h| *h.borrow_mut() = host.map(|h| (h, 0)));
}
pub(crate) fn ask(prompt: Prompt) -> JsResult<Option<String>> {
    let sender = HOST.with(|h| {
        let mut h = h.borrow_mut();
        let Some((tx, n)) = h.as_mut() else { return Ok(None) };
        if *n >= 32 {
            return Err(error("too many interactive prompts"));
        }
        *n += 1;
        Ok(Some(tx.clone()))
    })?;
    let Some(sender) = sender else { return Ok(None) };
    let (tx, rx) = mpsc::channel();
    sender.send(InteractiveEvent::Prompt(prompt, tx)).map_err(|_| error("interactive host closed"))?;
    let answer = rx.recv_timeout(std::time::Duration::from_secs(300)).ok().flatten();
    if answer.as_ref().is_some_and(|s| s.len() > 4096) {
        return Err(error("interactive response exceeds its limit"));
    }
    Ok(answer)
}
fn read_menu(value: &JsValue, ctx: &mut Context, depth: usize, remaining: &mut usize) -> JsResult<MenuItem> {
    if depth > 16 || *remaining == 0 {
        return Err(error("popup menu exceeds its size or depth limit"));
    }
    *remaining -= 1;
    let obj = value.as_object().ok_or_else(|| error("popup menu item must be an object"))?;
    let name = text(&obj.get(js_string!("cName"), ctx)?, ctx)?;
    let ret = obj.get(js_string!("cReturn"), ctx)?;
    let value = if ret.is_undefined() { name.clone() } else { text(&ret, ctx)? };
    if name.len() > 4096 || value.len() > 4096 {
        return Err(error("popup menu text exceeds its limit"));
    }
    let enabled = obj.get(js_string!("bEnabled"), ctx)?;
    let marked = obj.get(js_string!("bMarked"), ctx)?.to_boolean();
    let sub = obj.get(js_string!("oSubMenu"), ctx)?;
    let mut children = Vec::new();
    if let Some(o) = sub.as_object() {
        if o.is_array() {
            let array = JsArray::from_object(o.clone())?;
            let len = array.length(ctx)?;
            if len > *remaining as u64 {
                return Err(error("popup menu has too many entries"));
            }
            for i in 0..len {
                children.push(read_menu(&array.get(i, ctx)?, ctx, depth + 1, remaining)?);
            }
        } else {
            children.push(read_menu(&sub, ctx, depth + 1, remaining)?);
        }
    } else if !sub.is_undefined() && !sub.is_null() {
        return Err(error("invalid popup submenu"));
    }
    Ok(MenuItem { name, value, enabled: enabled.is_undefined() || enabled.to_boolean(), marked, children })
}

fn allowed(items: &[MenuItem], value: &str) -> bool {
    items.iter().any(|i| i.enabled && i.name != "-" && if i.children.is_empty() { i.value == value } else { allowed(&i.children, value) })
}
pub(crate) fn popup_menu(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let mut remaining = 1024;
    let mut items = Vec::new();
    for a in args {
        items.push(read_menu(a, ctx, 0, &mut remaining)?);
    }
    match ask(Prompt::Menu(items.clone()))? {
        Some(answer) if allowed(&items, &answer) => Ok(s(&answer)),
        Some(_) => Err(error("invalid popup menu selection")),
        None => Ok(JsValue::null()),
    }
}
pub(crate) fn response(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let a = arg(args, 0);
    let (question, title, default, password, label) = if let Some(o) = a.as_object() {
        (
            text(&o.get(js_string!("cQuestion"), ctx)?, ctx)?,
            text(&o.get(js_string!("cTitle"), ctx)?, ctx)?,
            text(&o.get(js_string!("cDefault"), ctx)?, ctx)?,
            o.get(js_string!("bPassword"), ctx)?.to_boolean(),
            text(&o.get(js_string!("cLabel"), ctx)?, ctx)?,
        )
    } else {
        (text(&a, ctx)?, text(&arg(args, 1), ctx)?, text(&arg(args, 2), ctx)?, arg(args, 3).to_boolean(), text(&arg(args, 4), ctx)?)
    };
    if [&question, &title, &default, &label].iter().any(|s| s.len() > 4096) {
        return Err(error("response dialog text exceeds its limit"));
    }
    Ok(ask(Prompt::Text { question, title, default, password, label })?.map_or_else(JsValue::null, |v| s(&v)))
}
