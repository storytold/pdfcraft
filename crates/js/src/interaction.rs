//! Bounded synchronous script prompts. The worker waits; the UI never evaluates script text.
use crate::host::{arg, error, s, text_with_budget};
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
/// Tell alert/menu helpers whether this worker has an interactive or deterministic reply channel.
pub(crate) fn has_host() -> bool {
    HOST.with(|h| h.borrow().is_some())
}
#[cfg(not(target_arch = "wasm32"))]
/// Install or clear the worker-local prompt channel and reset the per-invocation prompt budget.
pub(crate) fn set_host(host: Option<Sender<InteractiveEvent>>) {
    HOST.with(|h| *h.borrow_mut() = host.map(|h| (h, 0)));
}
/// Charge one prompt to the invocation's budget: the channel to send it on, if the worker has one.
fn reserve() -> JsResult<Option<Sender<InteractiveEvent>>> {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let Some((tx, n)) = h.as_mut() else { return Ok(None) };
        if *n >= 32 {
            return Err(error("too many interactive prompts"));
        }
        *n += 1;
        Ok(Some(tx.clone()))
    })
}
/// Send a bounded prompt and await its reply, treating disconnect/timeout as cancellation.
pub(crate) fn ask(prompt: Prompt) -> JsResult<Option<String>> {
    let Some(sender) = reserve()? else { return Ok(None) };
    let (tx, rx) = mpsc::channel();
    sender.send(InteractiveEvent::Prompt(prompt, tx)).map_err(|_| error("interactive host closed"))?;
    let answer = rx.recv_timeout(std::time::Duration::from_secs(300)).ok().flatten();
    if answer.as_ref().is_some_and(|s| s.len() > 4096) {
        return Err(error("interactive response exceeds its limit"));
    }
    Ok(answer)
}
/// Reject prompt text before copying beyond the public per-property byte limit.
fn menu_text(value: &JsValue, ctx: &mut Context) -> JsResult<String> {
    text_with_budget(value, ctx, 4096, "popup menu text exceeds its limit")
}
fn response_text(value: &JsValue, ctx: &mut Context) -> JsResult<String> {
    text_with_budget(value, ctx, 4096, "response dialog text exceeds its limit")
}
/// Parse one menu subtree with shared entry/depth/string budgets before exposing it to a host.
fn read_menu(value: &JsValue, ctx: &mut Context, depth: usize, remaining: &mut usize) -> JsResult<MenuItem> {
    if depth > 16 || *remaining == 0 {
        return Err(error("popup menu exceeds its size or depth limit"));
    }
    *remaining -= 1;
    let obj = value.as_object().ok_or_else(|| error("popup menu item must be an object"))?;
    let name = menu_text(&obj.get(js_string!("cName"), ctx)?, ctx)?;
    let ret = obj.get(js_string!("cReturn"), ctx)?;
    let value = if ret.is_undefined() { name.clone() } else { menu_text(&ret, ctx)? };
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

/// Accept only a matching enabled leaf whose ancestors are also enabled, excluding separators.
fn allowed(items: &[MenuItem], value: &str) -> bool {
    items.iter().any(|i| i.enabled && i.name != "-" && if i.children.is_empty() { i.value == value } else { allowed(&i.children, value) })
}
/// Parse and display a bounded popup, returning null on cancel and refusing invalid host choices.
pub(crate) fn popup_menu(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let mut remaining = 1024;
    let mut items = Vec::new();
    for a in args {
        items.push(read_menu(a, ctx, 0, &mut remaining)?);
    }
    // A menu with no items has nothing to pick: it answers null at once, without showing an empty
    // menu or using up a later menu's answer, but still counts toward the prompt limit (#2081).
    if items.is_empty() {
        reserve()?;
        return Ok(JsValue::null());
    }
    match ask(Prompt::Menu(items.clone()))? {
        Some(answer) if allowed(&items, &answer) => Ok(s(&answer)),
        Some(_) => Err(error("invalid popup menu selection")),
        None => Ok(JsValue::null()),
    }
}
/// Read named or positional input parameters and return bounded text or null on cancellation.
pub(crate) fn response(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let a = arg(args, 0);
    let (question, title, default, password, label) = if let Some(o) = a.as_object() {
        (
            response_text(&o.get(js_string!("cQuestion"), ctx)?, ctx)?,
            response_text(&o.get(js_string!("cTitle"), ctx)?, ctx)?,
            response_text(&o.get(js_string!("cDefault"), ctx)?, ctx)?,
            o.get(js_string!("bPassword"), ctx)?.to_boolean(),
            response_text(&o.get(js_string!("cLabel"), ctx)?, ctx)?,
        )
    } else {
        (
            response_text(&a, ctx)?,
            response_text(&arg(args, 1), ctx)?,
            response_text(&arg(args, 2), ctx)?,
            arg(args, 3).to_boolean(),
            response_text(&arg(args, 4), ctx)?,
        )
    };
    Ok(ask(Prompt::Text { question, title, default, password, label })?.map_or_else(JsValue::null, |v| s(&v)))
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    /// Reject an oversized question before evaluating or copying any later property.
    #[test]
    fn response_refuses_text_before_reading_later_properties() {
        let mut ctx = Context::default();
        let input = ctx.eval(crate::Source::from_bytes("({cQuestion:'x'.repeat(4097),get cTitle(){throw Error('later property read');}})")).unwrap();
        let error = response(&JsValue::undefined(), &[input], &mut ctx).unwrap_err().to_string();
        assert!(error.contains("response dialog text exceeds its limit"), "{error}");
        assert!(!error.contains("later property"));
    }
    /// Menu labels must fail before an oversized return value's getter is invoked.
    #[test]
    fn menu_refuses_name_before_reading_return_value() {
        let mut ctx = Context::default();
        let input = ctx.eval(crate::Source::from_bytes("({cName:'x'.repeat(4097),get cReturn(){throw Error('later property read');}})")).unwrap();
        let mut remaining = 1024;
        let error = read_menu(&input, &mut ctx, 0, &mut remaining).unwrap_err().to_string();
        assert!(error.contains("popup menu text exceeds its limit"), "{error}");
        assert!(!error.contains("later property"));
    }
}

#[cfg(test)]
mod text_boundary_tests {
    use super::*;
    #[test]
    fn prompt_conversions_preserve_exact_unicode_and_user_exceptions() {
        let mut ctx = Context::default();
        for (unit, count) in [("a", 4096), ("é", 2048), ("😀", 1024)] {
            let text = unit.repeat(count);
            let value = s(&text);
            assert_eq!(menu_text(&value, &mut ctx).unwrap(), text);
            assert_eq!(response_text(&value, &mut ctx).unwrap(), text);
            assert!(menu_text(&s(&(text.clone() + "x")), &mut ctx).is_err());
            assert!(response_text(&s(&(text + "x")), &mut ctx).is_err());
        }
        let surrogate = ctx.eval(crate::Source::from_bytes("'\\uD800'.repeat(682)+'xxxx'")).unwrap();
        assert_eq!(response_text(&surrogate, &mut ctx).unwrap().len(), 4096);
        let too_many = ctx.eval(crate::Source::from_bytes("'\\uD800'.repeat(683)")).unwrap();
        assert!(response_text(&too_many, &mut ctx).is_err());
        let throwing = ctx.eval(crate::Source::from_bytes("({toString(){throw Error('original conversion failure');}})")).unwrap();
        assert!(response_text(&throwing, &mut ctx).unwrap_err().to_string().contains("original conversion failure"));
        assert_eq!(response_text(&JsValue::null(), &mut ctx).unwrap(), "");
    }
    #[test]
    fn all_prompt_text_properties_and_positional_arguments_are_bounded() {
        let mut ctx = Context::default();
        for key in ["cQuestion", "cTitle", "cDefault", "cLabel"] {
            let script = format!("({{{key}:'x'.repeat(4097)}})");
            let input = ctx.eval(crate::Source::from_bytes(script.as_bytes())).unwrap();
            assert!(response(&JsValue::undefined(), &[input], &mut ctx).unwrap_err().to_string().contains("response dialog text exceeds its limit"));
        }
        for index in [0, 1, 2, 4] {
            let mut args = vec![JsValue::undefined(); 5];
            args[index] = s(&"x".repeat(4097));
            assert!(response(&JsValue::undefined(), &args, &mut ctx).is_err());
        }
        let input = ctx.eval(crate::Source::from_bytes("({cName:'ok',cReturn:'x'.repeat(4097)})")).unwrap();
        assert!(read_menu(&input, &mut ctx, 0, &mut 1024).is_err());
        // Four independently valid text properties remain supported; no new shared 4096-byte cap.
        let input = ctx
            .eval(crate::Source::from_bytes(
                "({cQuestion:'x'.repeat(4096),cTitle:'x'.repeat(4096),cDefault:'x'.repeat(4096),cLabel:'x'.repeat(4096)})",
            ))
            .unwrap();
        assert!(response(&JsValue::undefined(), &[input], &mut ctx).unwrap().is_null());
    }
}
