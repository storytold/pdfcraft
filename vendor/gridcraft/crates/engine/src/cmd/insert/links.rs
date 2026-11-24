use super::{
    Cell, Color, Comment, EngineError, Hyperlink, Json, RangeRef, Result, Session, Underline, Workbook, bool_param, cell_param_on, edit, json, ok,
    split_sheet, str_param, target_ranges, target_sheet,
};

#[cfg(test)]
pub(super) fn compiled() -> bool {
    true
}

// ---------------------------------------------------------------- links & comments

pub(super) fn insert_link(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let at = cell_param_on(s, p, "cell", sheet)?.unwrap_or(s.doc()?.selection.active);
    if bool_param(p, "remove").unwrap_or(false) {
        return edit(s, |cx| {
            cx.sheet_mut(sheet)?.hyperlinks.remove(&at);
            Ok(Json::Null)
        });
    }
    let Some(target) = str_param(p, "target").map(str::to_string) else {
        s.ui_requests.push(crate::UiRequest::Dialog("insertLink".into(), json!({})));
        return ok();
    };
    let text = str_param(p, "text").map(str::to_string);
    let tooltip = str_param(p, "tooltip").map(str::to_string);
    let external = links_outside(&s.doc()?.wb, sheet, &target);
    edit(s, |cx| {
        let link_style = cx.wb.styles.derive(cx.wb.sheet(sheet).map(|sh| sh.style_id(at)).unwrap_or_default(), |st| {
            st.font.color = Color::Theme(10, 0);
            st.font.underline = Underline::Single;
        });
        let sh = cx.sheet_mut(sheet)?;
        if let Some(t) = text.clone().or_else(|| {
            (sh.spill_anchor(at).is_none() && sh.value(at).is_empty() && sh.cell(at).is_none_or(|cell| !cell.has_content())).then(|| target.clone())
        }) {
            sh.set_cell(at, Cell { value: gridcraft_core::Value::text(t.as_str()), formula: None, style: link_style, py: None });
        } else {
            sh.set_style(at, link_style);
        }
        sh.hyperlinks.insert(at, Hyperlink { target: target.clone(), tooltip: tooltip.clone(), external });
        cx.touch(sheet, at);
        Ok(Json::Null)
    })
}

/// Follows a link: a reference selects it; anything else is refused ([`crate::links::classify`]).
/// An `http`/`https`/`mailto` address is refused too unless the host set
/// [`crate::links::LinkPolicy::Allow`]: every caller of a command may be a script or an agent, so
/// the user opens such links by clicking and confirming, and the UI then calls
/// [`Session::open_confirmed_link`] (#2181). Opens no dialog.
pub(super) fn open_link(s: &mut Session, p: &Json) -> Result<Json> {
    let target = match str_param(p, "url").or_else(|| str_param(p, "target")) {
        Some(t) => t.to_string(),
        None => {
            let sheet = target_sheet(s, p)?;
            let at = cell_param_on(s, p, "cell", sheet)?.unwrap_or(s.doc()?.selection.active);
            let Some(h) = s.doc()?.wb.sheet(sheet).and_then(|sh| sh.hyperlinks.get(&at)) else {
                return Err(EngineError::Other("There's no hyperlink in this cell.".into()));
            };
            h.target.clone()
        }
    };
    match crate::links::classify(&target) {
        crate::links::LinkKind::Internal(r) => crate::cmd::edit::go_to(s, &json!({"reference": r.trim_start_matches('#')})),
        crate::links::LinkKind::External(u) => {
            if s.link_policy != crate::links::LinkPolicy::Allow {
                return Err(EngineError::Other(crate::links::NEEDS_CONFIRMATION.into()));
            }
            s.ui_requests.push(crate::UiRequest::OpenUrl(u.clone()));
            Ok(json!({"opened": u}))
        }
        crate::links::LinkKind::Refused(why) => Err(EngineError::Other(why)),
    }
}

/// Sorts a link target as the two pages of Excel's Insert Hyperlink dialog do: "Place in This
/// Document" is a sheet reference, a defined name or `#…`; anything else ("Existing File or Web
/// Page") is outside the workbook and saved as a relationship.
fn links_outside(wb: &Workbook, sheet: usize, target: &str) -> bool {
    let t = target.trim();
    if t.starts_with('#') || wb.name(t, sheet).is_some() || wb.table(t).is_some() {
        return false;
    }
    let (on, body) = split_sheet(t);
    // Sheet names can't hold `: \ / ? * [ ]`, so `https://…!A1` or `C:\…!A1` is no sheet reference,
    // and `Book.xlsx#Sheet1!A1` is a place in another file unless a sheet has that name.
    let on_sheet = on.is_none_or(|name| {
        wb.sheets.iter().any(|s| s.name.eq_ignore_ascii_case(&name))
            || (!name.is_empty() && !name.contains([':', '\\', '/', '?', '*', '[', ']', '#']))
    });
    !(on_sheet && RangeRef::parse(body).is_some())
}

pub(super) fn comment(s: &mut Session, p: &Json, threaded: bool) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let at = cell_param_on(s, p, "cell", sheet)?.unwrap_or(s.doc()?.selection.active);
    let text = str_param(p, "text").unwrap_or("").to_string();
    let named = str_param(p, "author").map(str::to_string);
    let author = named.clone().unwrap_or_else(|| s.prefs.user_name.clone());
    edit(s, |cx| {
        let comments = &mut cx.sheet_mut(sheet)?.comments;
        // Writing a note on a cell that has one edits it, as Excel's Edit Note does: the box, the
        // replies, the dates and who wrote it stay. A note replaces a comment and vice versa.
        if let Some(c) = comments.get_mut(&at).filter(|c| c.threaded == threaded) {
            c.text = text.clone();
            if let Some(author) = named.clone() {
                c.author = author;
            }
            return Ok(json!({"cell": at.a1()}));
        }
        comments.insert(
            at,
            Comment {
                author: author.clone(),
                text: text.clone(),
                replies: vec![],
                threaded,
                resolved: false,
                visible: false,
                dates: vec![],
                note_box: None,
            },
        );
        Ok(json!({"cell": at.a1()}))
    })
}

pub(super) fn reply_comment(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let at = cell_param_on(s, p, "cell", sheet)?.unwrap_or(s.doc()?.selection.active);
    let text = str_param(p, "text").unwrap_or("").to_string();
    let author = str_param(p, "author").map(str::to_string).unwrap_or_else(|| s.prefs.user_name.clone());
    edit(s, |cx| {
        let c = cx.sheet_mut(sheet)?.comments.get_mut(&at).ok_or_else(|| EngineError::Other("There's no comment in this cell.".into()))?;
        c.replies.push((author.clone(), text.clone()));
        Ok(Json::Null)
    })
}

pub(super) fn resolve_comment(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let at = cell_param_on(s, p, "cell", sheet)?.unwrap_or(s.doc()?.selection.active);
    let v = bool_param(p, "resolved").unwrap_or(true);
    edit(s, |cx| {
        if let Some(c) = cx.sheet_mut(sheet)?.comments.get_mut(&at) {
            c.resolved = v;
        }
        Ok(Json::Null)
    })
}

pub(super) fn delete_comment(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let ranges = target_ranges(s, p)?;
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.comments.retain(|c, _| !ranges.iter().any(|r| r.contains(*c)));
        Ok(Json::Null)
    })
}

pub(super) fn show_note(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let at = cell_param_on(s, p, "cell", sheet)?.unwrap_or(s.doc()?.selection.active);
    edit(s, |cx| {
        if let Some(c) = cx.sheet_mut(sheet)?.comments.get_mut(&at) {
            c.visible = !c.visible;
        }
        Ok(Json::Null)
    })
}
