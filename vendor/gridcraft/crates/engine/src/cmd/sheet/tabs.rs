use super::{Arc, Ctx, EngineError, Json, Result, Session, Sheet, Visibility, bad, bool_param, edit, json, ok, str_param, target_sheet};

pub(super) fn insert_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let name = match str_param(p, "name") {
        Some(n) => n.to_string(),
        None => s.doc()?.wb.next_sheet_name(),
    };
    s.doc()?.wb.check_sheet_name(&name, None).map_err(EngineError::Other)?;
    let d = s.doc_mut()?;
    let before = p.get("before").and_then(Json::as_u64).map(|v| v as usize);
    let idx = edit_doc(d, |cx| {
        // Excel inserts before the active sheet; the "+" button appends after it.
        let at = before.unwrap_or(cx.wb.active_sheet() + 1).min(cx.wb.sheets.len());
        cx.wb.sheets.insert(at, Arc::new(Sheet::new(name.clone())));
        // Names scoped to later sheets shift.
        for n in cx.wb.names.iter_mut() {
            if let Some(sc) = n.scope.as_mut()
                && *sc >= at
            {
                *sc += 1;
            }
        }
        cx.wb.set_active_sheet(at);
        *cx.sel = crate::Selection::default();
        cx.structural = true;
        Ok(at)
    })?;
    d.sheet_selections.insert(idx.min(d.sheet_selections.len()), crate::Selection::default());
    Ok(json!({"index": idx, "name": name}))
}

fn edit_doc<R>(d: &mut crate::DocState, f: impl FnOnce(&mut Ctx) -> Result<R>) -> Result<R> {
    crate::cmd::commit(d, f)
}

pub(super) fn delete_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    let i = target_sheet(s, p)?;
    let d = s.doc()?;
    if d.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    if d.wb.sheets.iter().filter(|s| s.visibility == Visibility::Visible).count() <= 1 {
        return Err(EngineError::Other("A workbook must contain at least one visible worksheet.".into()));
    }
    let name = d.wb.sheet(i).map(|s| s.name.clone()).unwrap_or_default();
    let order: Vec<String> = d.wb.sheets.iter().map(|s| s.name.clone()).collect();
    let d = s.doc_mut()?;
    edit_doc(d, |cx| {
        cx.wb.sheets.remove(i);
        cx.wb.names.retain(|n| n.scope != Some(i));
        // Every reference into the deleted sheet becomes #REF!.
        cx.wb.rewrite_references(&|_, e| gridcraft_formula::adjust::delete_sheet(e, &name, &order));
        crate::cmd::edit::rewrite_python_refs(&mut cx.wb, &|_, e| gridcraft_formula::adjust::delete_sheet(e, &name, &order))?;
        for n in cx.wb.names.iter_mut() {
            if let Some(sc) = n.scope.as_mut()
                && *sc > i
            {
                *sc -= 1;
            }
        }
        if cx.wb.active_sheet() >= cx.wb.sheets.len() || cx.wb.active_sheet() > i {
            cx.wb.set_active_sheet(cx.wb.active_sheet().saturating_sub(1).min(cx.wb.sheets.len().saturating_sub(1)));
        }
        *cx.sel = crate::Selection::default();
        cx.structural = true;
        Ok(())
    })?;
    if i < d.sheet_selections.len() {
        d.sheet_selections.remove(i);
    }
    ok()
}

pub(super) fn rename_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let i = target_sheet(s, p)?;
    let Some(name) = str_param(p, "name").map(|n| n.trim().to_string()) else { return Err(bad("sheet.rename", "missing `name`")) };
    s.doc()?.wb.check_sheet_name(&name, Some(i)).map_err(EngineError::Other)?;
    edit(s, |cx| {
        let old = cx.wb.sheet(i).map(|s| s.name.clone()).unwrap_or_default();
        cx.sheet_mut(i)?.name = name.clone();
        // Every reference the workbook holds follows the sheet.
        cx.wb.rewrite_references(&|_, e| gridcraft_formula::adjust::rename_sheet(e, &old, &name));
        // A PivotChart follows its PivotTable's sheet. Only a sheet holding such a chart is
        // copied, so the others stay shared with undo snapshots.
        let on_old = |link: Option<&str>| link.and_then(crate::pivot::parse_chart_link).is_some_and(|(sheet, _)| sheet.eq_ignore_ascii_case(&old));
        for si in 0..cx.wb.sheets.len() {
            if !cx.wb.sheets.get(si).is_some_and(|sh| sh.charts.iter().any(|ch| on_old(ch.pivot.as_deref()))) {
                continue;
            }
            let Some(sh) = cx.wb.sheet_mut(si) else { continue };
            for ch in sh.charts.iter_mut() {
                if let Some((sheet, pivot)) = ch.pivot.as_deref().and_then(crate::pivot::parse_chart_link)
                    && sheet.eq_ignore_ascii_case(&old)
                {
                    ch.pivot = Some(crate::pivot::chart_link(&name, &pivot));
                }
            }
        }
        crate::cmd::edit::rewrite_python_refs(&mut cx.wb, &|_, e| gridcraft_formula::adjust::rename_sheet(e, &old, &name))?;
        cx.structural = true;
        Ok(Json::Null)
    })
}

pub(super) fn move_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let i = target_sheet(s, p)?;
    let copy = bool_param(p, "copy").unwrap_or(false);
    let to = p.get("to").and_then(Json::as_u64).map(|v| v as usize).ok_or_else(|| bad("sheet.move", "missing `to`"))?;
    edit(s, |cx| {
        let n = cx.wb.sheets.len();
        let Some(src) = cx.wb.sheets.get(i).cloned() else { return Err(bad("sheet.move", "no such sheet")) };
        if copy {
            let mut sh = (*src).clone();
            // The copy is a sheet of its own, not the one it was copied from.
            sh.uid = gridcraft_model::SheetUid::fresh();
            let base = sh.name.clone();
            // `Name (2)`: the name is shortened so the suffix fits in Excel's 31 characters.
            let name = (2..)
                .map(|k| {
                    let suffix = format!(" ({k})");
                    let keep = 31usize.saturating_sub(suffix.chars().count());
                    format!("{}{suffix}", base.chars().take(keep).collect::<String>().trim_end())
                })
                .find(|nm| cx.wb.sheet_index(nm).is_none())
                .unwrap_or(base.clone());
            sh.name = name;
            // A copied sheet's charts plot the copy's own data, as Excel's do (its formulas keep
            // what they name).
            // Text that names no other sheet is kept as written (a leading `=` included).
            let copy = sh.name.clone();
            let fix = |t: &mut String| {
                let Ok(e) = gridcraft_formula::parse(t.trim_start_matches('=')) else { return };
                let before = gridcraft_formula::print(&e);
                let after = gridcraft_formula::print(&gridcraft_formula::adjust::rename_sheet(e, &base, &copy));
                if after != before {
                    *t = if t.starts_with('=') { format!("={after}") } else { after };
                }
            };
            for ch in &mut sh.charts {
                for se in &mut ch.series {
                    fix(&mut se.values);
                    for t in [&mut se.categories, &mut se.name, &mut se.bubble_sizes].into_iter().flatten() {
                        fix(t);
                    }
                }
                if let Some(t) = ch.source.as_mut() {
                    fix(t);
                }
            }
            sh.tables.clear(); // table names must be unique
            sh.preserved = None; // kept file content (form controls, OLE objects…) stays with the original
            // Object ids are unique across the workbook (selection and the `{kind, id}` commands
            // rely on it), so the copied objects get fresh ones (free ones, even when a file used
            // the largest id).
            let count = sh.charts.len() + sh.images.len() + sh.shapes.len() + sh.slicers.len() + sh.controls.len();
            let ids = cx.wb.free_object_ids(count);
            if ids.len() < count {
                return Err(bad("sheet.move", "no free object ids are left for the copied objects"));
            }
            let mut ids = ids.into_iter();
            let mut fresh = || ids.next().unwrap_or(u32::MAX);
            sh.charts.iter_mut().for_each(|o| o.id = fresh());
            sh.images.iter_mut().for_each(|o| o.id = fresh());
            sh.shapes.iter_mut().for_each(|o| o.id = fresh());
            sh.slicers.iter_mut().for_each(|o| o.id = fresh());
            sh.controls.iter_mut().for_each(|o| o.id = fresh());
            let at = to.min(n);
            // The copy gets its own copies of the sheet's names, referring to the copy.
            let copies: Vec<gridcraft_model::DefinedName> = cx
                .wb
                .names
                .iter()
                .filter(|nm| nm.scope == Some(i))
                .map(|nm| {
                    let formula = gridcraft_formula::parse_name(&nm.formula)
                        .map(|e| gridcraft_formula::print(&gridcraft_formula::adjust::rename_sheet(e, &base, &sh.name)))
                        .unwrap_or_else(|_| nm.formula.clone());
                    gridcraft_model::DefinedName { scope: Some(at), formula, ..nm.clone() }
                })
                .collect();
            for nm in cx.wb.names.iter_mut() {
                if let Some(sc) = nm.scope.as_mut()
                    && *sc >= at
                {
                    *sc += 1;
                }
            }
            cx.wb.names.extend(copies);
            cx.wb.sheets.insert(at, Arc::new(sh));
            cx.wb.set_active_sheet(at);
        } else {
            let sh = cx.wb.sheets.remove(i);
            let at = to.min(n - 1);
            cx.wb.sheets.insert(at, sh);
            cx.wb.set_active_sheet(at);
            // Sheet-level names follow their sheet.
            for nm in cx.wb.names.iter_mut() {
                if let Some(sc) = nm.scope.as_mut() {
                    *sc = match *sc {
                        x if x == i => at,
                        x if i < x && x <= at => x - 1,
                        x if at <= x && x < i => x + 1,
                        x => x,
                    };
                }
            }
        }
        cx.structural = true;
        Ok(Json::Null)
    })
}

pub(super) fn tab_color(s: &mut Session, p: &Json) -> Result<Json> {
    let i = target_sheet(s, p)?;
    let c = crate::cmd::format::color_param(p.get("color"));
    edit(s, |cx| {
        cx.sheet_mut(i)?.tab_color = c.filter(|c| *c != gridcraft_model::Color::Auto);
        Ok(Json::Null)
    })
}

pub(super) fn hide_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    let i = target_sheet(s, p)?;
    if s.doc()?.wb.sheets.iter().filter(|s| s.visibility == Visibility::Visible).count() <= 1 {
        return Err(EngineError::Other("A workbook must contain at least one visible worksheet.".into()));
    }
    edit(s, |cx| {
        cx.sheet_mut(i)?.visibility = Visibility::Hidden;
        if cx.wb.active_sheet() == i {
            cx.wb.set_active_sheet(cx.wb.sheets.iter().position(|s| s.visibility == Visibility::Visible).unwrap_or(0));
        }
        Ok(Json::Null)
    })
}

pub(super) fn unhide_sheet(s: &mut Session, p: &Json) -> Result<Json> {
    if s.doc()?.wb.protected_structure {
        return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
    }
    if p.get("sheet").is_none() {
        let hidden: Vec<String> = s.doc()?.wb.sheets.iter().filter(|s| s.visibility != Visibility::Visible).map(|s| s.name.clone()).collect();
        if hidden.is_empty() {
            return Err(EngineError::Other("No sheets are hidden.".into()));
        }
        s.ui_requests.push(crate::UiRequest::Dialog("unhideSheet".into(), json!({"sheets": hidden})));
        return ok();
    }
    let i = target_sheet(s, p)?;
    edit(s, |cx| {
        cx.sheet_mut(i)?.visibility = Visibility::Visible;
        cx.wb.set_active_sheet(i);
        Ok(Json::Null)
    })
}

pub(super) fn activate(s: &mut Session, p: &Json) -> Result<Json> {
    let i = target_sheet(s, p)?;
    let d = s.doc_mut()?;
    d.wb_switch_sheet(i);
    Ok(json!({"sheet": i, "name": d.wb.active().map(|s| s.name.clone())}))
}

pub(super) fn step_sheet(s: &mut Session, dir: isize) -> Result<Json> {
    let d = s.doc()?;
    let mut i = d.wb.active_sheet();
    while let Some(next) = i.checked_add_signed(dir) {
        let Some(sheet) = d.wb.sheet(next) else { break };
        if sheet.visibility == Visibility::Visible {
            s.doc_mut()?.wb_switch_sheet(next);
            return ok();
        }
        i = next;
    }
    // No visible sheet in this direction: keep the current sheet and its selection.
    ok()
}
