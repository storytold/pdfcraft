//! Document-bound icon handles. Only object identities minted by this host are accepted.
use crate::host::{arg, error, host, read, write};
use crate::{Attribute, Context, JsObject, JsResult, JsString, JsValue, ObjectInitializer};

fn normal_face(args: &[JsValue], index: usize, c: &mut Context) -> JsResult<()> {
    if let Some(face) = args.get(index)
        && !face.is_undefined()
        && face.to_number(c)? != 0.0
    {
        return Err(error("only normal icon face 0 is supported"));
    }
    Ok(())
}
pub(crate) fn get(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
    normal_face(args, 0, c)?;
    let f = read(c, name)?;
    if f.kind != crate::FieldType::Button {
        return Err(error("icons require a push button"));
    }
    let Some(id) = f.icon else { return Ok(JsValue::null()) };
    if let Some((obj, _)) = host(c)?.borrow().icons.iter().find(|(_, i)| *i == id) {
        return Ok(obj.clone().into());
    }
    if host(c)?.borrow().icons.len() >= 4096 {
        return Err(error("too many icon handles"));
    }
    let obj = ObjectInitializer::new(c).property(crate::js_string!("name"), JsValue::null(), Attribute::READONLY).build();
    host(c)?.borrow_mut().icons.push((obj.clone(), id));
    Ok(obj.into())
}
pub(crate) fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
    normal_face(args, 1, c)?;
    if read(c, name)?.kind != crate::FieldType::Button {
        return Err(error("icons require a push button"));
    }
    let value = arg(args, 0);
    let id = if value.is_null() {
        None
    } else {
        let object = value.as_object().ok_or_else(|| error("buttonSetIcon requires an icon from this document"))?;
        Some(
            host(c)?
                .borrow()
                .icons
                .iter()
                .find(|(o, _)| JsObject::equals(o, &object))
                .map(|(_, i)| *i)
                .ok_or_else(|| error("buttonSetIcon requires an icon from this document"))?,
        )
    };
    write(c, name, |f| f.icon = id)?;
    Ok(JsValue::undefined())
}
