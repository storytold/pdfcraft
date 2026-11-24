use super::{
    CfOperator, CfRule, CfValueKind, CondFormat, Ctx, El, ErrorStyle, MAX_COND_CELLS, RangeRef, Sheet, Style, Validation, ValidationKind, from_file,
    parse_sqref, read_color,
};

fn cf_op(s: Option<&str>) -> CfOperator {
    match s.unwrap_or("between") {
        "notBetween" => CfOperator::NotBetween,
        "equal" => CfOperator::Equal,
        "notEqual" => CfOperator::NotEqual,
        "greaterThan" => CfOperator::Greater,
        "lessThan" => CfOperator::Less,
        "greaterThanOrEqual" => CfOperator::GreaterOrEqual,
        "lessThanOrEqual" => CfOperator::LessOrEqual,
        _ => CfOperator::Between,
    }
}

fn cfvo(e: &El) -> CfValueKind {
    let val = e.attr("val").unwrap_or("");
    let n = val.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    match e.attr("type").unwrap_or("num") {
        "min" | "autoMin" => CfValueKind::Min,
        "max" | "autoMax" => CfValueKind::Max,
        "percent" => n.map(CfValueKind::Percent).unwrap_or_else(|| CfValueKind::Formula(from_file(val))),
        "percentile" => n.map(CfValueKind::Percentile).unwrap_or_else(|| CfValueKind::Formula(from_file(val))),
        "formula" => CfValueKind::Formula(from_file(val)),
        _ => n.map(CfValueKind::Number).unwrap_or_else(|| CfValueKind::Formula(from_file(val))),
    }
}

/// `cfvo` elements: plain children, or x14 ones with the value in an `xm:f` child.
fn cfvos(e: &El) -> Vec<CfValueKind> {
    e.kids("cfvo")
        .map(|c| {
            if c.attr("val").is_none()
                && let Some(f) = c.child("f")
            {
                let mut c2 = c.clone();
                c2.attrs.push(("val".into(), f.text.clone()));
                return cfvo(&c2);
            }
            cfvo(c)
        })
        .collect()
}

/// One `conditionalFormatting` block. Every rule keeps its own copy of the block's ranges, so
/// `kept_cells` counts the range cells the sheet's retained rules hold so far, across blocks; a
/// rule that would take it past [`MAX_COND_CELLS`] is dropped. Unsupported rules cost nothing.
pub(super) fn read_cf(cx: &mut Ctx<'_>, e: &El, sheet: &mut Sheet, kept_cells: &mut u64) {
    let ranges = parse_sqref(e.attr("sqref"));
    if ranges.is_empty() {
        return;
    }
    let block_cells = ranges.iter().map(RangeRef::count).fold(0u64, u64::saturating_add);
    for r in e.kids("cfRule") {
        let style = Box::new(r.attr_u32("dxfId").and_then(|i| cx.dxfs.get(i as usize)).cloned().unwrap_or_default());
        let formulas: Vec<String> = r.kids("formula").map(|f| from_file(f.text.trim())).collect();
        // The x14 id that links a data bar to its Excel 2010 extension (solid fill lives there).
        let ext_id =
            r.child("extLst").and_then(|l| l.kids("ext").find_map(|x| x.child("id"))).map(|i| i.text.trim().to_string()).filter(|i| !i.is_empty());
        let ty = r.attr("type").unwrap_or("");
        match cf_rule(cx, r, ty, formulas, style) {
            Some(rule) => {
                if !push_cf(cx, sheet, &ranges, block_cells, r, rule, kept_cells) {
                    break;
                }
                if let Some(id) = ext_id {
                    cx.cf_ext_ids.push((id, sheet.cond_formats.len() - 1));
                }
            }
            None if matches!(ty, "colorScale" | "dataBar" | "iconSet") => {}
            None => cx.warn(format!("conditional format type \"{ty}\" is not supported")),
        }
    }
}

fn push_cf(cx: &mut Ctx<'_>, sheet: &mut Sheet, ranges: &[RangeRef], block_cells: u64, r: &El, rule: CfRule, kept_cells: &mut u64) -> bool {
    let total = kept_cells.saturating_add(block_cells);
    if total > MAX_COND_CELLS {
        cx.warn(format!("conditional formats past {MAX_COND_CELLS} kept range cells were dropped"));
        return false;
    }
    *kept_cells = total;
    sheet.cond_formats.push(CondFormat {
        ranges: ranges.to_vec(),
        rule,
        priority: r.attr_u32("priority").unwrap_or(0),
        stop_if_true: r.flag("stopIfTrue", false),
    });
    true
}

/// The rule of a `cfRule` (main namespace or x14) of type `ty`, given its formulas and
/// differential style; `None` for an unknown type or a scale, bar or icon set without its body.
fn cf_rule(cx: &Ctx<'_>, r: &El, ty: &str, formulas: Vec<String>, style: Box<Style>) -> Option<CfRule> {
    let mut formulas = formulas.into_iter();
    let f0 = formulas.next().unwrap_or_default();
    let text = r.attr("text").unwrap_or("").to_string();
    Some(match ty {
        "cellIs" => CfRule::CellIs { op: cf_op(r.attr("operator")), a: f0, b: formulas.next(), style },
        "expression" => CfRule::Expression { formula: f0, style },
        "containsText" => CfRule::ContainsText { text, style },
        "notContainsText" => CfRule::NotContainsText { text, style },
        "beginsWith" => CfRule::BeginsWith { text, style },
        "endsWith" => CfRule::EndsWith { text, style },
        "containsBlanks" => CfRule::Blanks { style },
        "notContainsBlanks" => CfRule::NoBlanks { style },
        "containsErrors" => CfRule::Errors { style },
        "notContainsErrors" => CfRule::NoErrors { style },
        "duplicateValues" => CfRule::Duplicate { style },
        "uniqueValues" => CfRule::Unique { style },
        "top10" => {
            CfRule::Top10 { bottom: r.flag("bottom", false), percent: r.flag("percent", false), rank: r.attr_u32("rank").unwrap_or(10), style }
        }
        "aboveAverage" => CfRule::AboveAverage {
            below: !r.flag("aboveAverage", true),
            equal: r.flag("equalAverage", false),
            std_dev: r.attr_u32("stdDev").unwrap_or(0).min(3) as u8,
            style,
        },
        "timePeriod" => CfRule::TimePeriod { period: r.attr("timePeriod").unwrap_or("today").to_string(), style },
        "colorScale" => {
            let cs = r.child("colorScale")?;
            let colors = cs.kids("color").map(|c| read_color(c, &cx.palette));
            CfRule::ColorScale { stops: cfvos(cs).into_iter().zip(colors).collect() }
        }
        "dataBar" => {
            let db = r.child("dataBar")?;
            let v = cfvos(db);
            CfRule::DataBar {
                min: v.first().cloned().unwrap_or(CfValueKind::Min),
                max: v.get(1).cloned().unwrap_or(CfValueKind::Max),
                color: db.child("color").map(|c| read_color(c, &cx.palette)).unwrap_or_default(),
                gradient: true,
                show_value: db.flag("showValue", true),
            }
        }
        "iconSet" => {
            let is = r.child("iconSet")?;
            CfRule::IconSet {
                set: is.attr("iconSet").unwrap_or("3TrafficLights1").to_string(),
                thresholds: cfvos(is),
                reverse: is.flag("reverse", false),
                show_value: is.flag("showValue", true),
            }
        }
        _ => return None,
    })
}

/// `x14:conditionalFormattings` (MS-XLSX 2.4.6): rules kept out of the main section, such as
/// those whose formulas refer to other sheets in the one LibreOffice-saved file seen. They carry
/// `xm:f` formulas, an inline `x14:dxf` and priorities shared with the main-section rules.
/// Read: cellIs, expression, (not)containsBlanks, (not)containsErrors, duplicate/unique values,
/// top10, aboveAverage and timePeriod. The x14 extension a main-section data bar links to by
/// `x14:id` adds no rule, only that bar's solid fill (its other settings are skipped with a
/// warning). Skipped with a warning (and so lost on save): other data bars, icon sets, colour
/// scales and the text rules containsText, notContainsText, beginsWith and endsWith.
pub(super) fn read_x14_cf(cx: &mut Ctx<'_>, e: &El, sheet: &mut Sheet, kept_cells: &mut u64) {
    for cf in e.kids("conditionalFormatting") {
        let ranges = parse_sqref(cf.child("sqref").map(|s| s.text.as_str()));
        let block_cells = ranges.iter().map(RangeRef::count).fold(0u64, u64::saturating_add);
        for r in cf.kids("cfRule") {
            let ty = r.attr("type").unwrap_or("");
            let linked = r.attr("id").and_then(|id| cx.cf_ext_ids.iter().find(|(i, _)| i == id.trim())).map(|(_, n)| *n);
            if let Some(n) = linked {
                match (sheet.cond_formats.get_mut(n), r.child("dataBar")) {
                    (Some(CondFormat { rule: CfRule::DataBar { gradient, .. }, .. }), Some(db)) => {
                        *gradient = db.flag("gradient", true);
                        // Only the fill style is modelled, with negatives in the bar's colour and no axis;
                        // borders, lengths, other axis positions or negative colours are lost.
                        let modelled = |k: &str, v: &str| {
                            k == "gradient"
                                || (k == "axisPosition" && v == "none")
                                || (k == "negativeBarColorSameAsPositive" && matches!(v, "1" | "true"))
                        };
                        if db.attrs.iter().any(|(k, v)| !modelled(k, v)) || db.children.iter().any(|k| k.name != "cfvo") {
                            cx.warn(format!("Excel 2010 conditional format extension \"{ty}\" was partly skipped (only its solid fill is kept)"));
                        }
                    }
                    _ => cx.warn(format!("Excel 2010 conditional format extension \"{ty}\" was skipped")),
                }
                continue;
            }
            if ranges.is_empty() {
                continue;
            }
            // Only rule kinds whose formulas are kept and written back here; the others' x14
            // forms add settings (bar fills and axes, custom icons) the model doesn't hold.
            let supported = matches!(
                ty,
                "cellIs"
                    | "expression"
                    | "containsBlanks"
                    | "notContainsBlanks"
                    | "containsErrors"
                    | "notContainsErrors"
                    | "duplicateValues"
                    | "uniqueValues"
                    | "top10"
                    | "aboveAverage"
                    | "timePeriod"
            );
            if !supported {
                cx.warn(format!("Excel 2010 conditional format extension \"{ty}\" was skipped"));
                continue;
            }
            let style = Box::new(r.child("dxf").map(|x| crate::styles::read_x14_dxf(x, &cx.palette, &cx.num_fmts)).unwrap_or_default());
            let formulas: Vec<String> = r.kids("f").map(|f| from_file(f.text.trim())).collect();
            if let Some(rule) = cf_rule(cx, r, ty, formulas, style)
                && !push_cf(cx, sheet, &ranges, block_cells, r, rule, kept_cells)
            {
                break;
            }
        }
    }
}

/// A `dataValidation` (main namespace, or x14 with `xm:f` formulas and an `xm:sqref` child).
pub(super) fn read_validation(v: &El) -> Option<Validation> {
    let sq = v.attr("sqref").map(str::to_string).or_else(|| v.child("sqref").map(|s| s.text.clone()));
    let ranges = parse_sqref(sq.as_deref());
    if ranges.is_empty() {
        return None;
    }
    let formula = |n: &str| -> Option<String> {
        let f = v.child(n)?;
        let t = f.child("f").map(|x| x.text.as_str()).unwrap_or(f.text.as_str()).trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    };
    let kind = match v.attr("type").unwrap_or("none") {
        "whole" => ValidationKind::Whole,
        "decimal" => ValidationKind::Decimal,
        "list" => ValidationKind::List,
        "date" => ValidationKind::Date,
        "time" => ValidationKind::Time,
        "textLength" => ValidationKind::TextLength,
        "custom" => ValidationKind::Custom,
        _ => ValidationKind::Any,
    };
    let mut f1 = formula("formula1").unwrap_or_default();
    if kind == ValidationKind::List && !f1.is_empty() && !f1.starts_with('"') {
        f1 = format!("={}", from_file(&f1));
    } else if !f1.starts_with('"') && !f1.is_empty() {
        f1 = from_file(&f1);
    }
    Some(Validation {
        ranges,
        kind,
        op: cf_op(v.attr("operator")),
        f1,
        f2: formula("formula2").map(|f| from_file(&f)),
        allow_blank: v.flag("allowBlank", false),
        // `showDropDown="1"` *hides* the in-cell list arrow.
        in_cell_dropdown: !v.flag("showDropDown", false),
        input_title: v.attr("promptTitle").unwrap_or("").to_string(),
        input_message: v.attr("prompt").unwrap_or("").to_string(),
        show_input: v.flag("showInputMessage", false),
        error_title: v.attr("errorTitle").unwrap_or("").to_string(),
        error_message: v.attr("error").unwrap_or("").to_string(),
        error_style: match v.attr("errorStyle") {
            Some("warning") => ErrorStyle::Warning,
            Some("information") => ErrorStyle::Information,
            _ => ErrorStyle::Stop,
        },
        show_error: v.flag("showErrorMessage", false),
    })
}
