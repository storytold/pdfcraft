use super::{
    Anchor, BorderLine, BorderStyle, Borders, CfOperator, CfRule, CfValueKind, Color, CondFormat, EngineError, Fill, Json, RangeRef, Result, Session,
    Shape, ShapeKind, Style, anchor_mode_param, bad, bool_param, cell_param, edit, f64_param, json, ok, str_param, target_ranges, target_sheet,
    u32_param,
};

#[cfg(test)]
pub(super) fn compiled() -> bool {
    true
}

// ---------------------------------------------------------------- conditional formatting

fn preset_style(name: &str) -> Style {
    let mut s = Style::default();
    match name {
        "redText" => s.font.color = Color::rgb(0x9C, 0x00, 0x06),
        "yellowFill" | "yellowFillDarkYellowText" => {
            s.fill = Fill::solid(Color::rgb(0xFF, 0xEB, 0x9C));
            s.font.color = Color::rgb(0x9C, 0x57, 0x00);
        }
        "greenFill" | "greenFillDarkGreenText" => {
            s.fill = Fill::solid(Color::rgb(0xC6, 0xEF, 0xCE));
            s.font.color = Color::rgb(0x00, 0x61, 0x00);
        }
        "lightRedFill" => s.fill = Fill::solid(Color::rgb(0xFF, 0xC7, 0xCE)),
        "redBorder" => {
            let l = BorderLine { style: BorderStyle::Thin, color: Color::rgb(0xC0, 0x00, 0x00) };
            s.borders = Borders { left: l, right: l, top: l, bottom: l, ..Default::default() };
        }
        _ => {
            s.fill = Fill::solid(Color::rgb(0xFF, 0xC7, 0xCE));
            s.font.color = Color::rgb(0x9C, 0x00, 0x06);
        }
    }
    s
}

/// Whether `v` is one formula string literal: `"…"` with every inner quote doubled.
fn is_string_literal(v: &str) -> bool {
    v.len() >= 2 && v.starts_with('"') && v.ends_with('"') && v.get(1..v.len() - 1).is_some_and(|inner| !inner.replace("\"\"", "").contains('"'))
}

/// A conditional format's `style` as the model's `Style` JSON (`font`, `fill`, `borders`, `align`,
/// `num_fmt`, `protection`, merged over the default by the caller), where a colour may also be
/// written as `format_range` takes it (`"#RRGGBB"`, `{theme, tint}`, `"auto"`) and `fill` may be a
/// colour alone (a solid fill) or `"none"` (no fill). An unknown key is an error rather than
/// dropped: dropped, it left a rule with an empty format that matches and shows nothing.
fn cf_style_json(st: &Json) -> Result<Json> {
    const KEYS: [&str; 6] = ["font", "fill", "borders", "align", "num_fmt", "protection"];
    let err = |m: String| bad("home.conditionalFormat", m);
    let Some(obj) = st.as_object() else { return Err(err("`style` must be an object, e.g. {\"fill\": \"#FFC7CE\"}".into())) };
    let mut st = obj.clone();
    if let Some(k) = st.keys().find(|k| !KEYS.contains(&k.as_str())) {
        let hint = if k == "preset" { "; a preset goes in `rule.preset`" } else { "" };
        return Err(err(format!("unknown key `{k}` in `style` (known: {}){hint}", KEYS.join(", "))));
    }
    // A colour written the way the other formatting commands take it, as the model stores it.
    let color = |v: &mut Json, what: &str| -> Result<()> {
        if v.is_string() || v.get("theme").is_some() {
            let c = crate::cmd::format::color_param(Some(v)).ok_or_else(|| err(format!("invalid colour {v} for `{what}`")))?;
            *v = serde_json::to_value(c).map_err(|e| EngineError::Other(e.to_string()))?;
        }
        Ok(())
    };
    if let Some(f) = st.get_mut("font").and_then(Json::as_object_mut)
        && let Some(c) = f.get_mut("color")
    {
        color(c, "font.color")?;
    }
    if let Some(fill) = st.get_mut("fill") {
        // `"none"` (or `"auto"`) is no fill, as `home.fillColor` reads it, not a solid automatic one.
        if fill.is_string() && crate::cmd::format::color_param(Some(fill)) == Some(Color::Auto) {
            *fill = serde_json::to_value(Fill::default()).map_err(|e| EngineError::Other(e.to_string()))?;
        } else if fill.is_string() || fill.get("theme").is_some() {
            *fill = json!({"pattern": "Solid", "fg": fill.clone()});
        }
        if let Some(f) = fill.as_object_mut() {
            for k in ["fg", "bg"] {
                if let Some(c) = f.get_mut(k) {
                    color(c, &format!("fill.{k}"))?;
                }
            }
            // A colour with no pattern is a solid fill, not an invisible one.
            if f.contains_key("fg") && !f.contains_key("pattern") {
                f.insert("pattern".into(), json!("Solid"));
            }
        }
    }
    if let Some(b) = st.get_mut("borders").and_then(Json::as_object_mut) {
        for (side, line) in b.iter_mut() {
            if let Some(c) = line.get_mut("color") {
                color(c, &format!("borders.{side}.color"))?;
            }
        }
    }
    Ok(Json::Object(st))
}

/// #1999: a formula that doesn't parse must never reach the workbook — Excel refuses to open
/// files carrying it, even in repair mode. Validated here so the API and the New Rule dialog
/// (which dispatches this command) share one gate, the way Excel's object model refuses.
fn check_cf_formula(cmd: &str, what: &str, formula: &str) -> Result<()> {
    // Strip exactly one leading '=', the way the XLSX writer does: a value like '===A1'
    // only parses after several strips and would still be saved unparseable. (#1999 review)
    if gridcraft_formula::parse(formula.strip_prefix('=').unwrap_or(formula)).is_err() {
        return Err(bad(cmd, format!("invalid {what} formula `{formula}` — it does not parse; Excel would reject the saved file")));
    }
    Ok(())
}

fn check_cf_value(what: &str, v: &gridcraft_model::features::CfValueKind) -> Result<()> {
    if let gridcraft_model::features::CfValueKind::Formula(f) = v {
        check_cf_formula("home.conditionalFormat", what, f)?;
    }
    Ok(())
}

fn check_cf_rule(rule: &CfRule) -> Result<()> {
    const CMD: &str = "home.conditionalFormat";
    match rule {
        CfRule::CellIs { a, b, .. } => {
            check_cf_formula(CMD, "comparison value", a)?;
            if let Some(b) = b {
                check_cf_formula(CMD, "second comparison value", b)?;
            }
        }
        CfRule::Expression { formula, .. } => check_cf_formula(CMD, "rule", formula)?,
        CfRule::DataBar { min, max, .. } => {
            check_cf_value("data bar minimum", min)?;
            check_cf_value("data bar maximum", max)?;
        }
        CfRule::ColorScale { stops } => {
            for (kind, _) in stops {
                check_cf_value("color scale threshold", kind)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn add_cf(s: &mut Session, p: &Json) -> Result<Json> {
    let ranges = target_ranges(s, p)?;
    let sheet = target_sheet(s, p)?;
    let Some(rule) = p.get("rule") else {
        s.ui_requests.push(crate::UiRequest::Dialog("newFormattingRule".into(), json!({})));
        return ok();
    };
    let g = |k: &str| rule.get(k).and_then(Json::as_str).map(str::to_string);
    let style = Box::new(match rule.get("style") {
        Some(st) if !st.is_null() => {
            let st = cf_style_json(st)?;
            let base = serde_json::to_value(Style::default()).map_err(|e| EngineError::Other(e.to_string()))?;
            serde_json::from_value(crate::cmd::format::merge_json(base, &st)).map_err(|e| bad("home.conditionalFormat", e.to_string()))?
        }
        _ => preset_style(g("preset").as_deref().unwrap_or("lightRedFillDarkRedText")),
    });
    let kind = g("type").unwrap_or_else(|| "cellIs".into());
    let value_kind = |v: Option<&Json>, default: CfValueKind| -> CfValueKind {
        match v {
            Some(Json::Object(o)) => {
                let t = o.get("type").and_then(Json::as_str).unwrap_or("num");
                let val = o.get("value").and_then(Json::as_f64).unwrap_or(0.0);
                match t {
                    "min" => CfValueKind::Min,
                    "max" => CfValueKind::Max,
                    "percent" => CfValueKind::Percent(val),
                    "percentile" => CfValueKind::Percentile(val),
                    "formula" => CfValueKind::Formula(o.get("value").and_then(Json::as_str).unwrap_or("").into()),
                    _ => CfValueKind::Number(val),
                }
            }
            _ => default,
        }
    };
    let rule = match kind.as_str() {
        "cellIs" => {
            // Excel's file and VBA names (`lessThan`, …) are accepted too.
            let op = match g("operator").as_deref().unwrap_or("greater") {
                "between" => CfOperator::Between,
                "notBetween" => CfOperator::NotBetween,
                "equal" => CfOperator::Equal,
                "notEqual" => CfOperator::NotEqual,
                "less" | "lessThan" => CfOperator::Less,
                "greaterOrEqual" | "greaterThanOrEqual" => CfOperator::GreaterOrEqual,
                "lessOrEqual" | "lessThanOrEqual" => CfOperator::LessOrEqual,
                "greater" | "greaterThan" => CfOperator::Greater,
                other => return Err(bad("home.conditionalFormat", format!("unknown operator `{other}`"))),
            };
            let a = rule.get("value").map(crate::cmd::edit::json_to_input).unwrap_or_default();
            let b = rule.get("value2").map(crate::cmd::edit::json_to_input);
            let quote = |v: String| {
                // A number, a formula, or text already written as a string literal (`"Closed"`, as
                // in the file and in openpyxl's `CellIsRule`) is kept; other text is quoted.
                if gridcraft_core::parse::parse_number_text(&v).is_some() || v.starts_with('=') {
                    v.trim_start_matches('=').to_string()
                } else if is_string_literal(&v) {
                    v
                } else {
                    format!("\"{}\"", v.replace('"', "\"\""))
                }
            };
            CfRule::CellIs { op, a: quote(a), b: b.map(quote), style }
        }
        "expression" => CfRule::Expression { formula: g("formula").unwrap_or_default().trim_start_matches('=').to_string(), style },
        "containsText" => CfRule::ContainsText { text: g("text").unwrap_or_default(), style },
        "notContainsText" => CfRule::NotContainsText { text: g("text").unwrap_or_default(), style },
        "beginsWith" => CfRule::BeginsWith { text: g("text").unwrap_or_default(), style },
        "endsWith" => CfRule::EndsWith { text: g("text").unwrap_or_default(), style },
        "blanks" => CfRule::Blanks { style },
        "noBlanks" => CfRule::NoBlanks { style },
        "errors" => CfRule::Errors { style },
        "noErrors" => CfRule::NoErrors { style },
        "duplicate" => CfRule::Duplicate { style },
        "unique" => CfRule::Unique { style },
        "top10" => CfRule::Top10 {
            bottom: rule.get("bottom").and_then(Json::as_bool).unwrap_or(false),
            percent: rule.get("percent").and_then(Json::as_bool).unwrap_or(false),
            rank: rule.get("rank").and_then(Json::as_u64).unwrap_or(10) as u32,
            style,
        },
        "aboveAverage" => CfRule::AboveAverage { below: rule.get("below").and_then(Json::as_bool).unwrap_or(false), equal: false, std_dev: 0, style },
        "timePeriod" => CfRule::TimePeriod { period: g("period").unwrap_or_else(|| "today".into()), style },
        "colorScale" => {
            let cols: Vec<Color> = rule
                .get("colors")
                .and_then(Json::as_array)
                .map(|a| a.iter().filter_map(|c| crate::cmd::format::color_param(Some(c))).collect())
                .unwrap_or_else(|| vec![Color::rgb(0xF8, 0x69, 0x6B), Color::rgb(0xFF, 0xEB, 0x84), Color::rgb(0x63, 0xBE, 0x7B)]);
            let stops = match cols.len() {
                2 => vec![(CfValueKind::Min, cols[0]), (CfValueKind::Max, cols[1])],
                _ => vec![
                    (CfValueKind::Min, cols.first().copied().unwrap_or_default()),
                    (CfValueKind::Percentile(50.0), cols.get(1).copied().unwrap_or_default()),
                    (CfValueKind::Max, cols.get(2).copied().unwrap_or_default()),
                ],
            };
            CfRule::ColorScale { stops }
        }
        "dataBar" => CfRule::DataBar {
            min: value_kind(rule.get("min"), CfValueKind::Min),
            max: value_kind(rule.get("max"), CfValueKind::Max),
            color: crate::cmd::format::color_param(rule.get("color")).unwrap_or(Color::rgb(0x63, 0x8E, 0xC6)),
            gradient: rule.get("gradient").and_then(Json::as_bool).unwrap_or(true),
            show_value: true,
        },
        "iconSet" => CfRule::IconSet {
            set: g("set").unwrap_or_else(|| "3TrafficLights1".into()),
            thresholds: vec![],
            reverse: rule.get("reverse").and_then(Json::as_bool).unwrap_or(false),
            show_value: true,
        },
        other => return Err(bad("home.conditionalFormat", format!("unknown rule type `{other}`"))),
    };
    check_cf_rule(&rule)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        let priority = sh.cond_formats.iter().map(|c| c.priority).max().unwrap_or(0) + 1;
        // New rules take precedence over older ones.
        for cf in sh.cond_formats.iter_mut() {
            cf.priority += 1;
        }
        sh.cond_formats.insert(0, CondFormat { ranges: ranges.clone(), rule, priority: 1, stop_if_true: false });
        let _ = priority;
        Ok(json!({"rules": sh.cond_formats.len()}))
    })
}

pub(super) fn clear_cf(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let whole = bool_param(p, "sheet").unwrap_or(false);
    let ranges = target_ranges(s, p)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        if whole {
            sh.cond_formats.clear();
        } else {
            for cf in sh.cond_formats.iter_mut() {
                let before = RangeRef::top_left(&cf.ranges);
                cf.ranges.retain(|r| !ranges.iter().any(|x| x.intersects(r)));
                // The relative formulas are written for the top-left cell: when it goes, they're
                // rewritten for the new one, so the areas left keep reading the same cells.
                if let (Some(from), Some(to)) = (before, RangeRef::top_left(&cf.ranges)) {
                    for f in cf.rule.relative_formulas_mut() {
                        *f = crate::cmd::data::move_rule_formula(f, from, to);
                    }
                }
            }
            sh.cond_formats.retain(|c| !c.ranges.is_empty());
        }
        Ok(Json::Null)
    })
}

pub(super) fn manage_cf(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    if p.get("delete").is_none() && p.get("moveUp").is_none() && p.get("moveDown").is_none() {
        let list: Vec<Json> = s
            .doc()?
            .wb
            .sheet(sheet)
            .map(|sh| {
                sh.cond_formats.iter().map(|c| json!({"ranges": c.ranges.iter().map(|r| r.a1()).collect::<Vec<_>>(), "rule": c.rule})).collect()
            })
            .unwrap_or_default();
        return Ok(json!({"rules": list}));
    }
    edit(s, |cx| {
        let sh = cx.sheet_mut(sheet)?;
        // The count is taken again after each step: a delete shortens the list.
        if let Some(i) = u32_param(p, "delete").map(|v| v as usize)
            && i < sh.cond_formats.len()
        {
            sh.cond_formats.remove(i);
        }
        if let Some(i) = u32_param(p, "moveUp").map(|v| v as usize)
            && i > 0
            && i < sh.cond_formats.len()
        {
            sh.cond_formats.swap(i, i - 1);
        }
        if let Some(i) = u32_param(p, "moveDown").map(|v| v as usize)
            && i + 1 < sh.cond_formats.len()
        {
            sh.cond_formats.swap(i, i + 1);
        }
        for (k, cf) in sh.cond_formats.iter_mut().enumerate() {
            cf.priority = k as u32 + 1;
        }
        Ok(Json::Null)
    })
}

pub(super) fn insert_icon(s: &mut Session, p: &Json) -> Result<Json> {
    let name = str_param(p, "name").unwrap_or("check").to_string();
    let size = f64_param(p, "size").unwrap_or(48.0).clamp(8.0, 1000.0) as f32;
    let d = s.doc()?;
    let id = d.wb.next_object_id();
    let at = cell_param(s, p, "at")?.unwrap_or(d.selection.active);
    let color = crate::cmd::format::color_param(p.get("color")).unwrap_or(Color::Theme(1, 0));
    let shape = Shape {
        id,
        kind: ShapeKind::Icon,
        anchor: Anchor { cell: at, dx: 4.0, dy: 4.0, width: size, height: size, mode: anchor_mode_param(p).unwrap_or_default() },
        fill: color,
        line: Color::Auto,
        text: name,
        format: Default::default(),
        rotation: 0.0,
        math: Default::default(),
    };
    let sheet = d.wb.active_sheet();
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.shapes.push(shape);
        Ok(json!({"shape": id}))
    })
}
