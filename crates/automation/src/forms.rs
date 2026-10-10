//! Form tools: list fields, fill them (several at once, one undo step), clear the form, and
//! prepare a form (add, change and delete fields).

use pdfcraft_engine::form_scripts::{CalcOp, Calculate, Format, Validate};
use pdfcraft_engine::{Edit, FieldProps, FieldValue, FormField, FormFieldKind, NewField, field_flags};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// `{"type": "number", "decimals": 2, "currency": "$"}`, `{"type": "date", "pattern": "mm/dd/yyyy"}`,
/// `{"type": "phone"}`, `{"type": "mask", "mask": "AA-9999"}`, `"none"`…
fn format_arg(v: &Value) -> Result<Format> {
    if v.as_str() == Some("none") {
        return Ok(Format::None);
    }
    let o = v.as_object().ok_or_else(|| bad("format must be an object such as {\"type\": \"number\", \"decimals\": 2}, or \"none\""))?;
    let num = |k: &str, d: u64| o.get(k).and_then(Value::as_u64).unwrap_or(d).min(255) as u8;
    let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
    Ok(match o.get("type").and_then(Value::as_str).unwrap_or("") {
        "none" => Format::None,
        "number" => Format::Number {
            decimals: num("decimals", 2),
            sep: num("separator", 0).min(4),
            neg: num("negative", 0).min(3),
            currency: s("currency").unwrap_or_default(),
            prepend: !o.get("currency_after").and_then(Value::as_bool).unwrap_or(false),
        },
        "percent" => Format::Percent { decimals: num("decimals", 2), sep: num("separator", 0).min(4) },
        "date" => Format::Date(s("pattern").unwrap_or_else(|| "mm/dd/yyyy".into())),
        "time" => Format::Time(s("pattern").unwrap_or_else(|| "HH:MM".into())),
        "zip" => Format::Special(0),
        "zip4" => Format::Special(1),
        "phone" => Format::Special(2),
        "ssn" => Format::Special(3),
        "mask" => Format::Mask(s("mask").ok_or_else(|| bad("mask needs \"mask\""))?),
        t => return Err(bad(format!("unknown format type {t:?} (none, number, percent, date, time, zip, zip4, phone, ssn, mask)"))),
    })
}

/// `{"border": "#FF0000" | "none", "fill": …, "width": 1-3, "style": "solid"|…, "text_color": …,
/// "font": "helvetica"|"times"|"courier"}`, changing only what is given.
fn look_arg(v: &Value) -> Result<pdfcraft_engine::FieldLookPatch> {
    use pdfcraft_engine::{BorderStyle, FieldFont, FieldLookPatch};
    let o = v.as_object().ok_or_else(|| bad("appearance must be an object"))?;
    for key in o.keys() {
        if !["border", "fill", "text_color", "width", "style", "font"].contains(&key.as_str()) {
            return Err(bad(format!("unknown appearance property {key:?}")));
        }
    }
    let colour = |k: &str| -> Result<Option<Option<[f64; 3]>>> {
        match o.get(k) {
            None => Ok(None),
            Some(Value::String(s)) if s == "none" => Ok(Some(None)),
            Some(Value::String(s)) => Ok(Some(Some(crate::comments::parse_color(s)?))),
            Some(_) => Err(bad(format!("{k} must be a colour or \"none\""))),
        }
    };
    let string =
        |k: &str| -> Result<Option<&str>> { o.get(k).map(|v| v.as_str().ok_or_else(|| bad(format!("appearance.{k} must be a string")))).transpose() };
    Ok(FieldLookPatch {
        border: colour("border")?,
        fill: colour("fill")?,
        text: colour("text_color")?.map(|c| c.unwrap_or([0.0; 3])),
        width: o
            .get("width")
            .map(|v| {
                v.as_f64()
                    .filter(|v| v.is_finite() && (0.0..=12.0).contains(v))
                    .ok_or_else(|| bad("appearance.width must be between 0 and 12 points"))
            })
            .transpose()?,
        style: string("style")?
            .map(|s| match s {
                "solid" => Ok(BorderStyle::Solid),
                "dashed" => Ok(BorderStyle::Dashed),
                "beveled" => Ok(BorderStyle::Beveled),
                "inset" => Ok(BorderStyle::Inset),
                "underline" => Ok(BorderStyle::Underline),
                x => Err(bad(format!("unknown style {x:?}"))),
            })
            .transpose()?,
        font: string("font")?
            .map(|s| match s {
                "helvetica" => Ok(FieldFont::Helvetica),
                "times" => Ok(FieldFont::Times),
                "courier" => Ok(FieldFont::Courier),
                x => Err(bad(format!("unknown font {x:?} (helvetica, times, courier)"))),
            })
            .transpose()?,
    })
}

/// `{"min": 0, "max": 100}` or `"none"`.
fn validate_arg(v: &Value) -> Result<Validate> {
    if v.as_str() == Some("none") {
        return Ok(Validate::None);
    }
    let o = v.as_object().ok_or_else(|| bad("validate must be {\"min\": …, \"max\": …} or \"none\""))?;
    let (min, max) = (o.get("min").and_then(Value::as_f64), o.get("max").and_then(Value::as_f64));
    if min.is_none() && max.is_none() {
        return Err(bad("validate needs min, max or both"));
    }
    Ok(Validate::Range { min, max })
}

/// `{"op": "sum", "fields": ["a", "b"]}`, `{"notation": "Price * Qty"}` or `"none"`.
fn calculate_arg(v: &Value) -> Result<Calculate> {
    if v.as_str() == Some("none") {
        return Ok(Calculate::None);
    }
    let o = v.as_object().ok_or_else(|| bad("calculate must be {\"op\": …, \"fields\": […]}, {\"notation\": …} or \"none\""))?;
    if let Some(n) = o.get("notation").and_then(Value::as_str) {
        return Ok(Calculate::Notation(n.to_string()));
    }
    let op = match o.get("op").and_then(Value::as_str).unwrap_or("") {
        "sum" => CalcOp::Sum,
        "product" => CalcOp::Product,
        "average" => CalcOp::Average,
        "min" | "minimum" => CalcOp::Minimum,
        "max" | "maximum" => CalcOp::Maximum,
        x => return Err(bad(format!("unknown op {x:?} (sum, product, average, min, max)"))),
    };
    let fields: Vec<String> =
        o.get("fields").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    if fields.is_empty() {
        return Err(bad("calculate needs fields"));
    }
    Ok(Calculate::Simple { op, fields })
}

fn format_json(f: &Format) -> Value {
    match f {
        Format::None => Value::Null,
        Format::Number { decimals, currency, .. } => json!({ "type": "number", "decimals": decimals, "currency": currency }),
        Format::Percent { decimals, .. } => json!({ "type": "percent", "decimals": decimals }),
        Format::Date(p) => json!({ "type": "date", "pattern": p }),
        Format::Time(p) => json!({ "type": "time", "pattern": p }),
        Format::Special(n) => {
            let t = ["zip", "zip4", "phone", "ssn"][(*n).min(3) as usize];
            json!({ "type": t })
        }
        Format::Mask(m) => json!({ "type": "mask", "mask": m }),
    }
}

fn kind_name(k: FormFieldKind) -> &'static str {
    match k {
        FormFieldKind::Text => "text",
        FormFieldKind::CheckBox => "checkbox",
        FormFieldKind::Radio => "radio",
        FormFieldKind::PushButton => "button",
        FormFieldKind::Combo => "combo",
        FormFieldKind::List => "list",
        FormFieldKind::Signature => "signature",
    }
}

/// A JSON value → the value for this field (booleans for check boxes, arrays for lists…).
fn value_for(f: &FormField, v: &Value) -> Result<FieldValue> {
    let wrong = |what: &str| ToolError::InvalidArgs(format!("{}: {what}", f.name));
    Ok(match (f.kind, v) {
        (FormFieldKind::CheckBox, Value::Bool(b)) => FieldValue::Check(*b),
        (FormFieldKind::Radio, Value::Null) => FieldValue::Radio(None),
        (FormFieldKind::Radio, Value::String(s)) => FieldValue::Radio(Some(s.clone())),
        (FormFieldKind::Combo | FormFieldKind::List, Value::Array(a)) => FieldValue::Choice(
            a.iter().map(|x| x.as_str().map(str::to_owned).ok_or_else(|| wrong("list values must be strings"))).collect::<Result<_>>()?,
        ),
        (FormFieldKind::Combo | FormFieldKind::List, Value::String(s)) => FieldValue::Choice(if s.is_empty() { Vec::new() } else { vec![s.clone()] }),
        (_, Value::String(s)) => FieldValue::Text(s.clone()),
        (_, Value::Number(n)) => FieldValue::Text(n.to_string()),
        (_, Value::Bool(b)) => FieldValue::Text(b.to_string()),
        _ => return Err(wrong("use a string (text, radio option, choice), true/false (check box) or an array (multi-select list)")),
    })
}

impl Automation {
    pub(crate) fn form_fields(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let out: Vec<Value> = doc
            .form
            .iter()
            .map(|f| {
                let w = f.widgets.first();
                let rect = w.and_then(|w| {
                    let p = doc.info.pages.get(w.page?)?;
                    let (a, b) = (p.user_to_view(w.rect[0] as f32, w.rect[1] as f32), p.user_to_view(w.rect[2] as f32, w.rect[3] as f32));
                    Some([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(|v| (v * 100.0).round() / 100.0))
                });
                let mut v = json!({
                    "name": f.name,
                    "type": kind_name(f.kind),
                    "value": match f.kind {
                        FormFieldKind::CheckBox => json!(!f.value.is_empty()),
                        FormFieldKind::List if f.has(field_flags::MULTI_SELECT) => json!(f.value),
                        FormFieldKind::Radio => json!(f.value.first().map(|v| f.export_for_state(v))),
                        _ => json!(f.value.first()),
                    },
                    "page": w.and_then(|w| w.page).map(|p| p + 1),
                    "rect": rect,
                    "rotation": w.map(|w| w.rotation).unwrap_or(0),
                    "read_only": f.read_only(),
                    "required": f.has(field_flags::REQUIRED),
                });
                let mut more = serde_json::Map::new();
                let o = &mut more;
                if let Some(t) = &f.tooltip {
                    o.insert("tooltip".into(), json!(t));
                }
                if let Some(s) = doc.field_check_style(&f.name) {
                    o.insert("check_style".into(), json!(s.label().to_lowercase()));
                }
                if f.actions.format != Format::None {
                    o.insert("format".into(), format_json(&f.actions.format));
                    o.insert("display".into(), json!(f.value.first().map(|v| pdfcraft_engine::form_scripts::format_value(&f.actions.format, v))));
                }
                if let Validate::Range { min, max } = &f.actions.validate {
                    o.insert("validate".into(), json!({ "min": min, "max": max }));
                }
                match &f.actions.calculate {
                    Calculate::Simple { op, fields } => {
                        o.insert("calculate".into(), json!({ "op": op.code().to_lowercase(), "fields": fields }));
                    }
                    Calculate::Notation(n) => {
                        o.insert("calculate".into(), json!({ "notation": n }));
                    }
                    Calculate::None => {}
                }
                if !f.actions.unsupported.is_empty() {
                    o.insert("unsupported_scripts".into(), json!(f.actions.unsupported));
                }
                match f.kind {
                    FormFieldKind::Radio => {
                        o.insert("options".into(), json!((0..f.widgets.len()).filter_map(|i| f.export_of(i)).collect::<Vec<_>>()));
                    }
                    FormFieldKind::Combo | FormFieldKind::List => {
                        o.insert("options".into(), f.options.iter().map(|(e, d)| json!({ "value": e, "label": d })).collect());
                        o.insert("multi_select".into(), json!(f.has(field_flags::MULTI_SELECT)));
                        o.insert("editable".into(), json!(f.has(field_flags::EDIT)));
                    }
                    FormFieldKind::Text => {
                        o.insert("multiline".into(), json!(f.has(field_flags::MULTILINE)));
                        if let Some(m) = f.max_len {
                            o.insert("max_length".into(), json!(m));
                        }
                    }
                    _ => {}
                }
                if let Value::Object(m) = &mut v {
                    m.extend(more);
                }
                v
            })
            .collect();
        Ok(json!({ "count": out.len(), "fields": out }))
    }

    pub(crate) fn form_fill(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let values = a
            .get("values")
            .and_then(Value::as_object)
            .ok_or_else(|| ToolError::InvalidArgs("values must be an object of field name → value".into()))?;
        if values.is_empty() {
            return Err(ToolError::InvalidArgs("values is empty".into()));
        }
        let mut edits = Vec::new();
        for (name, v) in values {
            let f = doc.form.iter().find(|f| &f.name == name).ok_or_else(|| failed(format!("there is no field named {name:?} (see form_fields)")))?;
            edits.push(Edit::SetFieldValue { name: name.clone(), value: value_for(f, v)? });
        }
        let edit = if edits.len() == 1 { edits.remove(0) } else { Edit::Batch { label: "Fill in form".into(), edits } };
        let mut out = self.apply(a, edit)?;
        out["filled"] = json!(values.len());
        Ok(out)
    }

    pub(crate) fn form_reset(&mut self, a: &Args) -> Result<Value> {
        let names = a.get("fields").map(|_| a.strs("fields").map(|v| v.into_iter().map(str::to_owned).collect::<Vec<_>>())).transpose()?;
        self.apply(a, Edit::ResetForm { names })
    }

    pub(crate) fn form_add_field(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let doc = self.doc(a)?;
        let options = || -> Result<Vec<String>> {
            Ok(a.get("options").map(|_| a.strs("options")).transpose()?.unwrap_or_default().into_iter().map(str::to_owned).collect())
        };
        let kind = match a.str("type")? {
            "text" => NewField::Text { multiline: a.opt_bool("multiline")?.unwrap_or(false) },
            "date" => NewField::Date,
            "checkbox" => NewField::CheckBox,
            "radio" => {
                NewField::Radio { group: a.opt_str("group")?.map(str::to_owned), export: a.opt_str("export")?.unwrap_or("Choice1").to_owned() }
            }
            "combo" => NewField::Combo { options: options()?, editable: a.opt_bool("editable")?.unwrap_or(false) },
            "list" => NewField::List { options: options()?, multi: a.opt_bool("multi_select")?.unwrap_or(false) },
            "button" => NewField::Button { caption: a.opt_str("caption")?.unwrap_or("").to_owned() },
            "image" => NewField::Image,
            "signature" => NewField::Signature,
            t => return Err(ToolError::InvalidArgs(format!("unknown field type {t:?}"))),
        };
        let r: Vec<f64> = a.get("rect").and_then(Value::as_array).map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let r = <[f64; 4]>::try_from(r).map_err(|_| ToolError::InvalidArgs("rect must be 4 numbers".into()))?;
        // Top-left-origin points on the displayed page → user space.
        let p = &doc.info.pages[page];
        let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
        let rect = [u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64];
        if rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
            return Err(ToolError::InvalidArgs("rect is empty".into()));
        }
        let before: Vec<String> = doc.form.iter().map(|f| f.name.clone()).collect();
        let name = a.opt_str("name")?.map(str::to_owned);
        let mut out = self.apply(a, Edit::AddField { page, rect, kind, name })?;
        let doc = self.doc(a)?;
        let added = doc
            .form
            .iter()
            .map(|f| &f.name)
            .find(|n| !before.contains(n))
            .or_else(|| a.opt_str("group").ok().flatten().and_then(|g| doc.form.iter().map(|f| &f.name).find(|n| *n == g)));
        out["field"] = json!(added);
        Ok(out)
    }

    pub(crate) fn form_set_props(&mut self, a: &Args) -> Result<Value> {
        let single = a.opt_str("field")?;
        let bulk = a.get("fields").is_some();
        if single.is_some() == bulk {
            return Err(bad("give either field or fields, not both"));
        }
        let names: Vec<String> = if bulk {
            let count = a.get("fields").and_then(Value::as_array).ok_or_else(|| bad("fields must be an array of field names"))?.len();
            if count == 0 || count > 1_000 {
                return Err(bad("fields must contain between 1 and 1000 names"));
            }
            let names = a.strs("fields")?;
            let mut seen = std::collections::HashSet::new();
            for name in &names {
                if !seen.insert(*name) {
                    return Err(bad(format!("field {name:?} is listed more than once")));
                }
            }
            for key in ["name", "rect", "rotation"] {
                if a.get(key).is_some() {
                    return Err(bad(format!("{key} applies to one field; use field instead of fields")));
                }
            }
            names.into_iter().map(str::to_owned).collect()
        } else {
            single.into_iter().map(str::to_owned).collect()
        };
        let mut edits = Vec::with_capacity(names.len());
        let mut new_name = None;
        for name in &names {
            // Merge appearance changes separately for each field: changing just the border
            // must not copy the first field's fill, font or text colour onto the others.
            let props = self.field_props_arg(a, name)?;
            if props == FieldProps::default() {
                return Err(bad("nothing to change"));
            }
            new_name = Some(props.name.clone().unwrap_or_else(|| name.clone()));
            edits.push(Edit::SetFieldProps { name: name.clone(), props: Box::new(props) });
        }
        let edit = if bulk {
            Edit::Batch { label: "Change field properties".into(), edits }
        } else {
            edits.pop().ok_or_else(|| bad("give a field name"))?
        };
        let mut out = self.apply(a, edit)?;
        if bulk {
            out["fields"] = json!(names);
        } else {
            out["field"] = json!(new_name);
        }
        Ok(out)
    }

    fn field_props_arg(&self, a: &Args, name: &str) -> Result<FieldProps> {
        let doc = self.doc(a)?;
        let f = doc.form.iter().find(|f| f.name == name).ok_or_else(|| failed(format!("there is no field named {name:?} (see form_fields)")))?;
        // Position: top-left-origin points on the displayed page → user space.
        let rect = match a.get("rect") {
            None => None,
            Some(v) => {
                let r: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                let r = <[f64; 4]>::try_from(r).map_err(|_| ToolError::InvalidArgs("rect must be 4 numbers".into()))?;
                let page = f.widgets.first().and_then(|w| w.page).ok_or_else(|| failed(format!("{name} is not on a page")))?;
                let p = &doc.info.pages[page];
                let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
                Some((0, [u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64]))
            }
        };
        Ok(FieldProps {
            rect,
            name: a.opt_str("name")?.map(str::to_owned),
            tooltip: a.opt_str("tooltip")?.map(str::to_owned),
            read_only: a.opt_bool("read_only")?,
            required: a.opt_bool("required")?,
            multiline: a.opt_bool("multiline")?,
            max_len: match a.opt_int("max_length")? {
                None => None,
                Some(0) => Some(None),
                Some(n) if n > 0 => Some(Some(n as usize)),
                Some(_) => return Err(ToolError::InvalidArgs("max_length must be 0 (no limit) or more".into())),
            },
            options: a.get("options").map(|_| a.strs("options")).transpose()?.map(|v| v.into_iter().map(str::to_owned).collect()),
            font_size: a.opt_num("font_size")?,
            look: None,
            appearance: a.get("appearance").map(look_arg).transpose()?.filter(|p| *p != pdfcraft_engine::FieldLookPatch::default()),
            format: a.get("format").map(format_arg).transpose()?,
            validate: a.get("validate").map(validate_arg).transpose()?,
            calculate: a.get("calculate").map(calculate_arg).transpose()?,
            quadding: match a.opt_str("align")? {
                None => None,
                Some("left") => Some(0),
                Some("center" | "centre") => Some(1),
                Some("right") => Some(2),
                Some(o) => return Err(bad(format!("unknown alignment {o:?} (left, center, right)"))),
            },
            default_value: a.opt_str("default")?.map(|d| (!d.is_empty()).then(|| d.to_owned())),
            locked: a.opt_bool("locked")?,
            flags: match a.get("flags") {
                None => Vec::new(),
                Some(v) => {
                    use pdfcraft_engine::field_flags as ff;
                    let obj = v.as_object().ok_or_else(|| bad("flags must be an object of booleans"))?;
                    let mut out = Vec::new();
                    for (k, on) in obj {
                        let on = on.as_bool().ok_or_else(|| bad(format!("flags.{k} must be true or false")))?;
                        // Acrobat's wording: "Scroll long text" and "Check spelling" are the
                        // inverse of their bits.
                        let (bit, inverted) = match k.as_str() {
                            "scroll" => (ff::DO_NOT_SCROLL, true),
                            "rich_text" => (ff::RICH_TEXT, false),
                            "password" => (ff::PASSWORD, false),
                            "file_select" => (ff::FILE_SELECT, false),
                            "spell_check" => (ff::DO_NOT_SPELL_CHECK, true),
                            "comb" => (ff::COMB, false),
                            "sort" => (ff::SORT, false),
                            "editable" => (ff::EDIT, false),
                            "multi_select" => (ff::MULTI_SELECT, false),
                            "commit_immediately" => (ff::COMMIT_ON_SEL_CHANGE, false),
                            "radios_in_unison" => (ff::RADIOS_IN_UNISON, false),
                            other => return Err(bad(format!("unknown flag {other:?}"))),
                        };
                        out.push((bit, on != inverted));
                    }
                    out
                }
            },
            rotation: match a.opt_int("rotation")? {
                None => None,
                Some(r @ (0 | 90 | 180 | 270)) => Some((0, r)),
                Some(r) => return Err(ToolError::InvalidArgs(format!("rotation must be 0, 90, 180 or 270, not {r}"))),
            },
            actions: None,
            check_style: match a.opt_str("check_style")? {
                None => None,
                Some(s) => Some(
                    pdfcraft_engine::CheckStyle::ALL
                        .into_iter()
                        .find(|c| c.label().eq_ignore_ascii_case(s))
                        .ok_or_else(|| bad(format!("unknown check style {s:?} (check, circle, cross, diamond, square, star)")))?,
                ),
            },
        })
    }

    pub(crate) fn form_tab_order(&mut self, a: &Args) -> Result<Value> {
        let n = self.doc(a)?.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        // Order tabs manually: move one field earlier or later on its page.
        if let Some(field) = a.opt_str("field")? {
            let earlier = match a.opt_str("move")?.unwrap_or("earlier") {
                "earlier" => true,
                "later" => false,
                m => return Err(bad(format!("move must be earlier or later, not {m:?}"))),
            };
            let mut out = self.apply(a, Edit::MoveInTabOrder { name: field.to_owned(), earlier })?;
            out["tab_order"] = self.tab_sequence(a)?;
            return Ok(out);
        }
        let order = match a.str("order")? {
            "row" | "rows" => pdfcraft_engine::TabOrder::Row,
            "column" | "columns" => pdfcraft_engine::TabOrder::Column,
            "structure" => pdfcraft_engine::TabOrder::Structure,
            "annotations" | "unspecified" => pdfcraft_engine::TabOrder::Annotations,
            o => return Err(bad(format!("unknown order {o:?} (row, column, structure, annotations)"))),
        };
        let mut out = self.apply(a, Edit::SetTabOrder { pages, order })?;
        out["tab_order"] = self.tab_sequence(a)?;
        Ok(out)
    }

    fn tab_sequence(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let mut seq: Vec<(usize, &str)> =
            doc.form.iter().flat_map(|f| f.widgets.iter().filter(|w| w.page.is_some()).map(move |w| (w.tab, f.name.as_str()))).collect();
        seq.sort();
        seq.dedup_by(|x, y| x.1 == y.1);
        Ok(json!(seq.iter().map(|x| x.1).collect::<Vec<_>>()))
    }

    pub(crate) fn form_delete_field(&mut self, a: &Args) -> Result<Value> {
        let name = a.str("field")?.to_owned();
        if !self.doc(a)?.form.iter().any(|f| f.name == name) {
            return Err(failed(format!("there is no field named {name:?} (see form_fields)")));
        }
        self.apply(a, Edit::DeleteField { name })
    }

    pub(crate) fn doc_export_data(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let path = self.resolve(a.str("path")?, true)?;
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let format = pdfcraft_engine::DataFormat::from_extension(&ext).ok_or_else(|| bad("the path must end in .xfdf, .fdf, .xml, .csv or .txt"))?;
        let (comments, fields) = match a.opt_str("what")?.unwrap_or("all") {
            "all" => (true, true),
            "comments" => (true, false),
            "fields" => (false, true),
            w => return Err(bad(format!("unknown what {w:?} (all, comments, fields)"))),
        };
        if comments && !matches!(format, pdfcraft_engine::DataFormat::Xfdf | pdfcraft_engine::DataFormat::Fdf) {
            return Err(bad("comments travel as .xfdf or .fdf; .xml, .csv and .txt hold form data (use what: fields)"));
        }
        let bytes = self.session.export_data(id, format, comments, fields).map_err(failed)?;
        crate::write_atomic(&path, &bytes)?;
        Ok(json!({ "path": path.to_string_lossy(), "bytes": bytes.len() }))
    }

    pub(crate) fn doc_import_data(&mut self, a: &Args) -> Result<Value> {
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let before = (self.doc(a)?.info.annotations.len(), self.doc(a)?.form.iter().filter(|f| !f.value.is_empty()).count());
        let mut out = self.apply(a, Edit::ImportData { name, bytes: std::sync::Arc::new(bytes) })?;
        let doc = self.doc(a)?;
        out["comments"] = json!(doc.info.annotations.len() as i64 - before.0 as i64);
        out["filled_fields"] = json!(doc.form.iter().filter(|f| !f.value.is_empty()).count());
        Ok(out)
    }
}
