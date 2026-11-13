//! A choice list's methods (the scripting reference XFA 3.3 p. 392 points to: `addItem`, `boundItem`, `clearItems`, `deleteItem`, `getDisplayItem`,
//! `getSaveItem`, `setItems`) and its `length` and `selectedIndex`. They read the field's item lists and change the script's
//! copy of them (`HNode::items`), which every later read of the field's items in the run follows (`items.nodes`, `length`).

use boa_engine::{Context, JsObject, JsResult, JsString, JsValue};

use super::host::{XHost, host};
use std::collections::HashMap;

use super::props::{
    ItemList, is_choice_list, items_written, list_count, list_entries, list_len, list_position, list_text, prop_ref, saved_list, take_item_writes,
};
use super::{arg, text};

/// The most entries a script may give one list: more is left out.
pub(crate) const MAX_ITEMS: usize = 10_000;

/// Which list the field shows, and which it saves (`None` when it saves what it shows).
fn which(h: &XHost, node: usize) -> (usize, Option<usize>) {
    let saved = saved_list(h, node);
    let shown = (0..list_count(h, node).max(1)).find(|i| Some(*i) != saved).unwrap_or(0);
    (shown, saved)
}

/// Each entry, its tag kept, with the text a script wrote to it (`writes`, by list, tag and nth of that tag) instead.
fn with_writes(entries: Vec<(String, String)>, list: usize, writes: &Writes) -> Vec<(String, String)> {
    let mut nth: HashMap<String, usize> = HashMap::new();
    entries
        .into_iter()
        .map(|(tag, t)| {
            let n = nth.entry(tag.clone()).or_insert(0);
            let key = (list, tag.clone(), *n);
            *n += 1;
            let t = writes.get(&key).cloned().unwrap_or(t);
            (tag, t)
        })
        .collect()
}

type Writes = HashMap<(usize, String, usize), String>;

/// Make the script's copy of the lists `(shown, saved)`, replacing them: one list when every entry saves what it shows,
/// else the shown list then the saved one. At most [`MAX_ITEMS`] entries, each text charged against the script's payload:
/// what does not fit is left out.
fn set_lists(h: &mut XHost, node: usize, shown: Vec<String>, saved: Vec<String>) {
    let two = shown != saved;
    let mut kept_shown = Vec::new();
    let mut kept_saved = Vec::new();
    for (i, t) in shown.into_iter().take(MAX_ITEMS).enumerate() {
        let s = saved.get(i).cloned().unwrap_or_else(|| t.clone());
        let bytes = t.len() + if two { s.len() } else { 0 };
        if !h.room(bytes) {
            break;
        }
        kept_shown.push(("text".to_string(), t));
        kept_saved.push(("text".to_string(), s));
    }
    let mut out = vec![ItemList { entries: kept_shown, saved: false }];
    if two {
        out.push(ItemList { entries: kept_saved, saved: true });
    }
    if let Some(n) = h.nodes.get_mut(node) {
        n.items = Some(out);
    }
}

/// The run's copy of the lists, made from the template's the first time a method changes them or reads them after a
/// script wrote to an entry: every list, in its place, with every entry and its tag, so `items` paths and lists keep
/// reading what they did (the template is not something the script grows, so nothing is cut or charged). What a script
/// wrote to entries (`items.nodes.item(0).value = …`) is folded in, so entries that move or go take their text with
/// them. Returns the shown list's index and the saved one's, `None` when it saves what it shows.
fn state(h: &mut XHost, node: usize) -> (usize, Option<usize>) {
    let writes = take_item_writes(h, node);
    if h.nodes.get(node).is_some_and(|n| n.items.is_none()) {
        let saved = saved_list(h, node);
        let mut out: Vec<ItemList> = (0..list_count(h, node))
            .map(|i| ItemList { entries: with_writes(list_entries(h, node, i), i, &writes), saved: Some(i) == saved })
            .collect();
        if out.is_empty() {
            out.push(ItemList::default());
        }
        if let Some(n) = h.nodes.get_mut(node) {
            n.items = Some(out);
        }
        return which(h, node);
    }
    if !writes.is_empty()
        && let Some(lists) = h.nodes.get_mut(node).and_then(|n| n.items.as_mut())
    {
        for (i, l) in lists.iter_mut().enumerate() {
            l.entries = with_writes(std::mem::take(&mut l.entries), i, &writes);
        }
    }
    which(h, node)
}

/// Fold what a script wrote to entries into the run's copy before reading them.
fn settle(h: &mut XHost, node: usize) {
    if items_written(h, node) {
        state(h, node);
    }
}

/// `length`: how many entries a choice list shows; `None` for any other node.
pub(crate) fn length(h: &XHost, node: usize) -> Option<usize> {
    is_choice_list(h, node).then(|| list_len(h, node, which(h, node).0))
}

/// `selectedIndex`: the entry the field's value is (by what it saves, else what it shows), or -1.
pub(crate) fn selected_index(h: &mut XHost, node: usize) -> Option<i64> {
    if !is_choice_list(h, node) {
        return None;
    }
    settle(h, node);
    let value = h.nodes.get(node).map(|n| n.value.clone()).unwrap_or_default();
    let (shown, saved) = which(h, node);
    let at = list_position(h, node, saved.unwrap_or(shown), &value).or_else(|| list_position(h, node, shown, &value));
    Some(at.map_or(-1, |i| i as i64))
}

/// The node a list method is called on.
fn target(r: &JsObject, ctx: &mut Context) -> JsResult<usize> {
    Ok(prop_ref(r, ctx)?.0)
}

/// An entry index argument: `None` for one that is not a whole number from 0.
fn index(v: &JsValue, ctx: &mut Context) -> JsResult<Option<usize>> {
    let n = v.to_number(ctx)?;
    Ok((n.is_finite() && n >= 0.0 && n.fract() == 0.0).then_some(n as usize))
}

fn item_at(r: &JsObject, args: &[JsValue], ctx: &mut Context, saved: bool) -> JsResult<JsValue> {
    let node = target(r, ctx)?;
    let Some(i) = index(&arg(args, 0), ctx)? else { return Ok(JsValue::null()) };
    let mut h = host(ctx)?.borrow_mut();
    settle(&mut h, node);
    let (shown, kept) = which(&h, node);
    let list = if saved { kept.unwrap_or(shown) } else { shown };
    Ok(list_text(&h, node, list, i).map_or(JsValue::null(), |t| JsString::from(t.as_str()).into()))
}

/// `getDisplayItem(i)`: what entry `i` shows, or `null`.
pub(crate) fn get_display_item(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    item_at(r, args, ctx, false)
}

/// `getSaveItem(i)`: what entry `i` saves, or `null`.
pub(crate) fn get_save_item(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    item_at(r, args, ctx, true)
}

/// `boundItem(text)`: what the entry showing `text` saves; text no entry shows comes back as it is.
pub(crate) fn bound_item(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let node = target(r, ctx)?;
    let given = arg(args, 0);
    if given.is_null_or_undefined() {
        return Ok(given);
    }
    let t = text(&given, ctx)?;
    let mut h = host(ctx)?.borrow_mut();
    settle(&mut h, node);
    let (shown, saved) = which(&h, node);
    let bound = list_position(&h, node, shown, &t).and_then(|i| list_text(&h, node, saved.unwrap_or(shown), i)).unwrap_or(t);
    Ok(JsString::from(bound.as_str()).into())
}

/// `clearItems()`: no entries left.
pub(crate) fn clear_items(_: &JsValue, _: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let node = target(r, ctx)?;
    let mut h = host(ctx)?.borrow_mut();
    // what was written to the old entries goes with them
    take_item_writes(&mut h, node);
    set_lists(&mut h, node, Vec::new(), Vec::new());
    Ok(JsValue::undefined())
}

/// `addItem(shown[, saved])`: an entry at the end, saving `saved` (what it shows when not given).
pub(crate) fn add_item(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let node = target(r, ctx)?;
    let shown_arg = arg(args, 0);
    if shown_arg.is_undefined() {
        return Ok(JsValue::undefined());
    }
    let shown_text = text(&shown_arg, ctx)?;
    let saved_text = match args.get(1) {
        Some(v) if !v.is_null_or_undefined() => text(v, ctx)?,
        _ => shown_text.clone(),
    };
    let mut h = host(ctx)?.borrow_mut();
    let (shown, saved) = state(&mut h, node);
    let count = h.nodes.get(node).and_then(|n| n.items.as_ref()).and_then(|l| l.get(shown)).map_or(0, |l| l.entries.len());
    // a saved value other than the text needs a saved list: one is made, saving what each entry shows
    let saved = match saved {
        None if saved_text != shown_text => {
            let mut copy = h.nodes.get(node).and_then(|n| n.items.as_ref()).and_then(|l| l.get(shown)).cloned().unwrap_or_default();
            copy.saved = true;
            let bytes = copy.entries.iter().map(|(_, t)| t.len()).sum();
            if !h.room(bytes) {
                return Ok(JsValue::undefined());
            }
            if let Some(lists) = h.nodes.get_mut(node).and_then(|n| n.items.as_mut()) {
                lists.push(copy);
                Some(lists.len() - 1)
            } else {
                None
            }
        }
        other => other,
    };
    let bytes = shown_text.len() + if saved.is_some() { saved_text.len() } else { 0 };
    if count >= MAX_ITEMS || !h.room(bytes) {
        return Ok(JsValue::undefined());
    }
    if let Some(lists) = h.nodes.get_mut(node).and_then(|n| n.items.as_mut()) {
        if let Some(l) = lists.get_mut(shown) {
            l.entries.push(("text".into(), shown_text));
        }
        if let Some(l) = saved.and_then(|i| lists.get_mut(i)) {
            l.entries.push(("text".into(), saved_text));
        }
    }
    Ok(JsValue::undefined())
}

/// `deleteItem(i)`: entry `i` removed; whether there was one.
pub(crate) fn delete_item(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let node = target(r, ctx)?;
    let i = index(&arg(args, 0), ctx)?;
    let mut h = host(ctx)?.borrow_mut();
    // an index past the end changes nothing, not even the run's copy of the lists
    let len = list_len(&h, node, which(&h, node).0);
    if i.is_none_or(|i| i >= len) {
        return Ok(JsValue::from(false));
    }
    let (shown, saved) = state(&mut h, node);
    let Some(lists) = h.nodes.get_mut(node).and_then(|n| n.items.as_mut()) else { return Ok(JsValue::from(false)) };
    let Some(i) = i.filter(|i| lists.get(shown).is_some_and(|l| *i < l.entries.len())) else { return Ok(JsValue::from(false)) };
    for list in [Some(shown), saved].into_iter().flatten() {
        if let Some(l) = lists.get_mut(list)
            && i < l.entries.len()
        {
            l.entries.remove(i);
        }
    }
    Ok(JsValue::from(true))
}

/// `setItems("a,b,c")`, or `setItems("shown1,saved1,shown2,saved2", 2)`: the entries, replacing the list's; `false`, and the
/// list as it was, when the texts don't make whole pairs.
pub(crate) fn set_items(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let node = target(r, ctx)?;
    let all = text(&arg(args, 0), ctx)?;
    let pairs = match args.get(1) {
        Some(v) if !v.is_undefined() => v.to_number(ctx)? == 2.0,
        _ => false,
    };
    if pairs && !all.is_empty() && all.split(',').count() % 2 != 0 {
        return Ok(JsValue::from(false));
    }
    // only the texts the list can keep are copied
    let keep = if pairs { MAX_ITEMS * 2 } else { MAX_ITEMS };
    let parts: Vec<String> = if all.is_empty() { Vec::new() } else { all.split(',').take(keep).map(str::to_string).collect() };
    let (shown, saved): (Vec<String>, Vec<String>) =
        if pairs { parts.chunks_exact(2).filter_map(|c| Some((c.first()?.clone(), c.get(1)?.clone()))).unzip() } else { (parts.clone(), parts) };
    let mut h = host(ctx)?.borrow_mut();
    take_item_writes(&mut h, node);
    set_lists(&mut h, node, shown, saved);
    Ok(JsValue::from(true))
}
