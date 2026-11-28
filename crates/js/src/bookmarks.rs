//! Live outline objects; edits are replayed by the engine in the script's undo transaction.
use crate::host::{arg, error, function, host, s, text_with_budget};
use crate::{Context, JsArray, JsObject, JsResult, JsValue, NativeFunction, ObjectInitializer, js_string};

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub object_id: u64,
    pub path: Vec<usize>,
    pub name: String,
    pub open: bool,
    pub style: i32,
    pub color: [f64; 3],
    pub page: Option<usize>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Property {
    Name(String),
    Open(bool),
    Style(i32),
    Color([f64; 3]),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub object_id: u64,
    pub property: Property,
}

fn get(_: &JsValue, _: &[JsValue], key: &(u64, crate::JsString), ctx: &mut Context) -> JsResult<JsValue> {
    let bookmarks = host(ctx)?.borrow().doc.bookmarks.clone();
    let prop = key.1.to_std_string_escaped();
    let bookmark = bookmarks.iter().find(|b| b.object_id == key.0).cloned();
    if key.0 != 0 && bookmark.is_none() {
        return Err(error("this bookmark no longer exists"));
    }
    match prop.as_str() {
        "children" => {
            let parent = bookmark.map_or_else(Vec::new, |b| b.path);
            let children: Vec<_> =
                bookmarks.iter().filter(|b| b.path.len() == parent.len() + 1 && b.path.starts_with(&parent)).map(|b| b.object_id).collect();
            if children.is_empty() {
                return Ok(JsValue::null());
            }
            let mut values = Vec::new();
            for id in children {
                values.push(object(ctx, id)?.into());
            }
            Ok(JsArray::from_iter(values, ctx).into())
        }
        "parent" => {
            let mut path = bookmark.map_or_else(Vec::new, |b| b.path);
            if path.is_empty() {
                return Ok(JsValue::null());
            }
            path.pop();
            let id = bookmarks.iter().find(|b| b.path == path).map_or(0, |b| b.object_id);
            Ok(object(ctx, id)?.into())
        }
        "name" => Ok(s(bookmark.as_ref().map_or("", |b| b.name.as_str()))),
        "open" => Ok(bookmark.is_none_or(|b| b.open).into()),
        "style" => Ok(bookmark.map_or(0, |b| b.style).into()),
        "color" => {
            let color = bookmark.map_or([0.0; 3], |b| b.color);
            Ok(JsArray::from_iter(std::iter::once(s("RGB")).chain(color.map(JsValue::from)), ctx).into())
        }
        _ => Ok(JsValue::undefined()),
    }
}
fn set(_: &JsValue, args: &[JsValue], key: &(u64, crate::JsString), ctx: &mut Context) -> JsResult<JsValue> {
    {
        let h = host(ctx)?.borrow();
        if h.calculate_depth > 0 || matches!(h.event_name.as_str(), "Calculate" | "Format" | "Keystroke" | "Validate") {
            return Err(error("field calculations cannot edit bookmarks"));
        }
    }
    let value = arg(args, 0);
    let property = match key.1.to_std_string_escaped().as_str() {
        "name" => Property::Name(text_with_budget(&value, ctx, 4096, "bookmark name is too long")?),
        "open" => Property::Open(value.to_boolean()),
        "style" => {
            let n = value.to_number(ctx)?;
            if !n.is_finite() || !(0.0..=3.0).contains(&n) {
                return Err(error("bookmark style must be 0 through 3"));
            }
            Property::Style(n as i32)
        }
        "color" => {
            let array = JsArray::from_object(value.as_object().ok_or_else(|| error("bookmark color must be a color array"))?)?;
            let tag = text_with_budget(&array.get(0, ctx)?, ctx, 16, "bookmark color space is too long")?;
            let count = match tag.as_str() {
                "G" => 1,
                "RGB" => 3,
                "CMYK" => 4,
                _ => return Err(error("bookmark color space must be G, RGB or CMYK")),
            };
            if array.length(ctx)? != count + 1 {
                return Err(error("bookmark color has the wrong number of components"));
            }
            let mut components = [0.0; 4];
            for (i, c) in components.iter_mut().take(count as usize).enumerate() {
                let n = array.get((i + 1) as u32, ctx)?.to_number(ctx)?;
                if !n.is_finite() {
                    return Err(error("bookmark color must be finite"));
                }
                *c = n.clamp(0.0, 1.0);
            }
            let [a, b, c, k] = components;
            let color = match tag.as_str() {
                "G" => [a; 3],
                "CMYK" => [1.0 - (a + k).min(1.0), 1.0 - (b + k).min(1.0), 1.0 - (c + k).min(1.0)],
                _ => [a, b, c],
            };
            Property::Color(color)
        }
        _ => return Err(error("bookmark property is read-only")),
    };
    let mut h = host(ctx)?.borrow_mut();
    if h.document_changes.len() >= 1024 {
        return Err(error("too many bookmark changes in one script"));
    }
    let b = h.doc.bookmarks.iter_mut().find(|b| b.object_id == key.0).ok_or_else(|| error("this bookmark no longer exists"))?;
    match &property {
        Property::Name(v) => b.name.clone_from(v),
        Property::Open(v) => b.open = *v,
        Property::Style(v) => b.style = *v,
        Property::Color(v) => b.color = *v,
    }
    h.document_changes.push(Change { object_id: key.0, property });
    Ok(JsValue::undefined())
}
fn execute(_: &JsValue, _: &[JsValue], id: &u64, ctx: &mut Context) -> JsResult<JsValue> {
    let h = host(ctx)?;
    let page = h.borrow().doc.bookmarks.iter().find(|b| b.object_id == *id).and_then(|b| b.page);
    if let Some(page) = page {
        h.borrow_mut().doc.page = page;
        h.borrow_mut().request(crate::Request::GoToPage(page));
    }
    Ok(JsValue::undefined())
}
fn object(ctx: &mut Context, id: u64) -> JsResult<JsObject> {
    let object = ObjectInitializer::new(ctx).build();
    for property in ["children", "parent", "name", "open", "style", "color"] {
        let capture = (id, crate::JsString::from(property));
        let getter = function(ctx, NativeFunction::from_copy_closure_with_captures(get, capture.clone()));
        let setter =
            (!matches!(property, "children" | "parent")).then(|| function(ctx, NativeFunction::from_copy_closure_with_captures(set, capture)));
        object.define_property_or_throw(
            crate::JsString::from(property),
            boa_engine::property::PropertyDescriptor::builder().get(getter).maybe_set(setter).enumerable(true).configurable(true),
            ctx,
        )?;
    }
    let execute = function(ctx, NativeFunction::from_copy_closure_with_captures(execute, id));
    object.create_data_property_or_throw(js_string!("execute"), execute, ctx)?;
    Ok(object)
}
pub(crate) fn root(ctx: &mut Context) -> JsResult<JsObject> {
    object(ctx, 0)
}
