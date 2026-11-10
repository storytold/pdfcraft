//! Named destinations (ISO 32000-2 §12.3.2.3): `destinations` lists them, `destination_set` adds
//! or moves one, `destination_delete` removes one.
//!
//! A name is the destination's address: `/Names /Dests` keeps one entry per name, and a bookmark
//! or a link points at the name rather than at a page. A destination opens a page with a view type
//! from Table 149 and exactly that type's values, in PDF user space (a `null` value keeps the
//! reader's position or zoom there); a `/FitR` needs all four of its numbers.
use crate::{Args, Automation, Result, ToolError};
use pdfcraft_engine::{BookmarkDest, Edit};
use serde_json::{Value, json};

/// The longest name a destination may have: the room's own bound, so a name an automation client
/// writes is one a shared session can carry back.
const MAX_NAME: usize = 128;

/// The destination types of Table 149 and how many values each takes.
const VIEW_TYPES: [(&str, usize); 8] = [("XYZ", 3), ("Fit", 0), ("FitH", 1), ("FitV", 1), ("FitB", 0), ("FitBH", 1), ("FitBV", 1), ("FitR", 4)];

fn bad(what: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(what.into())
}

impl Automation {
    /// Every named destination, its name, page (1-based) and view.
    pub(crate) fn destinations_list(&self, a: &Args) -> Result<Value> {
        let rows: Vec<Value> = self.doc(a)?.named_destinations().iter().map(dest_json).collect();
        Ok(json!({ "destinations": rows, "count": rows.len() }))
    }

    /// Set the name to a page and a view: an existing name is moved, a new one is added.
    pub(crate) fn destination_set(&mut self, a: &Args) -> Result<Value> {
        let name = a.opt_str("name")?.unwrap_or_default().trim().to_owned();
        check_name(&name)?;
        let pages = self.doc(a)?.info.pages.len();
        let page = a.int("page")?;
        if page < 1 || page > pages as i64 {
            return Err(bad(format!("page must be 1–{pages}")));
        }
        let kind = a.opt_str("view")?.unwrap_or("XYZ").trim().to_owned();
        let values = view_values(a, &kind)?;
        let dest = BookmarkDest::View { page: page as usize - 1, kind: kind.clone().into_bytes(), values: values.clone() };
        let mut out = self.apply(a, Edit::SetNamedDestination { name: name.clone(), dest })?;
        out["name"] = json!(name);
        out["page"] = json!(page);
        out["view"] = json!({ "type": kind, "values": values });
        Ok(out)
    }

    /// Remove a named destination. A name this document does not define is refused.
    pub(crate) fn destination_delete(&mut self, a: &Args) -> Result<Value> {
        let name = a.opt_str("name")?.unwrap_or_default().trim().to_owned();
        check_name(&name)?;
        let mut out = self.apply(a, Edit::DeleteNamedDestination { name: name.clone() })?;
        out["name"] = json!(name);
        Ok(out)
    }
}

/// A name must be text a reader can address: not blank, and no longer than the room carries.
fn check_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(bad("name must not be blank"));
    }
    if name.len() > MAX_NAME {
        return Err(bad(format!("name is {} bytes; at most {MAX_NAME} are written", name.len())));
    }
    Ok(())
}

/// The values of the chosen view type, checked before anything is written: exactly the type's own
/// count, every number finite, and a `/FitR` with all four of its rectangle's numbers.
fn view_values(a: &Args, kind: &str) -> Result<Vec<Option<f64>>> {
    let arity = VIEW_TYPES.iter().find(|(name, _)| *name == kind).map(|(_, arity)| *arity).ok_or_else(|| {
        bad(format!(
            "view must be one of {} (ISO 32000-2 §12.3.2.2 Table 149)",
            VIEW_TYPES.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
        ))
    })?;
    let values = match a.get("values").filter(|value| !value.is_null()) {
        None => vec![None; arity],
        Some(list) => {
            let list = list.as_array().ok_or_else(|| bad("values must be a list of numbers or nulls"))?;
            if list.len() != arity {
                return Err(bad(format!("a {kind} destination takes {arity} values, not {}", list.len())));
            }
            list.iter()
                .map(|value| match value {
                    Value::Null => Ok(None),
                    value => match value.as_f64().filter(|v| v.is_finite()) {
                        Some(number) => Ok(Some(number)),
                        None => Err(bad("values must be numbers or null")),
                    },
                })
                .collect::<Result<Vec<_>>>()?
        }
    };
    if kind == "FitR" && values.iter().any(Option::is_none) {
        return Err(bad("a FitR destination needs all four values: left, bottom, right, top"));
    }
    Ok(values)
}

/// A destination as the tools list it: its name, the 1-based page it opens and its view.
fn dest_json(destination: &pdfcraft_organize::NamedDestination) -> Value {
    let view = destination.view.as_ref().map(|(kind, values)| {
        json!({
            "type": String::from_utf8_lossy(kind).into_owned(),
            "values": values.iter().map(|value| value.map_or(Value::Null, |v| json!(v))).collect::<Vec<_>>(),
        })
    });
    json!({
        "name": destination.name,
        "page": destination.page.map(|page| page.saturating_add(1)),
        "view": view,
    })
}
