use super::*;

fn cf_op_name(op: CfOperator) -> &'static str {
    match op {
        CfOperator::Between => "between",
        CfOperator::NotBetween => "notBetween",
        CfOperator::Equal => "equal",
        CfOperator::NotEqual => "notEqual",
        CfOperator::Greater => "greaterThan",
        CfOperator::Less => "lessThan",
        CfOperator::GreaterOrEqual => "greaterThanOrEqual",
        CfOperator::LessOrEqual => "lessThanOrEqual",
    }
}

fn cfvo_xml(v: &CfValueKind) -> String {
    match v {
        CfValueKind::Min => "<cfvo type=\"min\"/>".into(),
        CfValueKind::Max => "<cfvo type=\"max\"/>".into(),
        CfValueKind::Number(n) => format!("<cfvo type=\"num\" val=\"{}\"/>", num(*n)),
        CfValueKind::Percent(n) => format!("<cfvo type=\"percent\" val=\"{}\"/>", num(*n)),
        CfValueKind::Percentile(n) => format!("<cfvo type=\"percentile\" val=\"{}\"/>", num(*n)),
        CfValueKind::Formula(f) => format!("<cfvo type=\"formula\" val=\"{}\"/>", esc_attr(&text_to_file(f))),
    }
}

fn quote(t: &str) -> String {
    format!("\"{}\"", t.replace('"', "\"\""))
}

/// The id linking a solid data bar's `cfRule` to its x14 extension; unique per sheet because priorities are.
pub(super) fn solid_bar_id(priority: u32) -> String {
    format!("{{00000000-0000-0000-0000-{priority:012}}}")
}

/// The `cfRule` extension that points a solid data bar at its x14 rule.
fn solid_bar_ext(priority: u32) -> String {
    let id = solid_bar_id(priority);
    format!("<extLst><ext uri=\"{{B025F937-C7B1-47D3-B67F-A62EFF666E3E}}\" xmlns:x14=\"{X14_NS}\"><x14:id>{id}</x14:id></ext></extLst>")
}

/// An x14 `cfvo`: unlike the 2006 one it has no `val` attribute, the value is an `xm:f` child.
pub(super) fn x14_cfvo(v: &CfValueKind) -> String {
    let (ty, val) = match v {
        CfValueKind::Min => return "<x14:cfvo type=\"min\"/>".into(),
        CfValueKind::Max => return "<x14:cfvo type=\"max\"/>".into(),
        CfValueKind::Number(n) => ("num", num(*n)),
        CfValueKind::Percent(n) => ("percent", num(*n)),
        CfValueKind::Percentile(n) => ("percentile", num(*n)),
        CfValueKind::Formula(f) => ("formula", text_to_file(f)),
    };
    format!("<x14:cfvo type=\"{ty}\"><xm:f>{}</xm:f></x14:cfvo>", esc(&val))
}

/// One `cfRule` element. The formulas generated for text, blank, error and date rules refer to the
/// top-left cell of all the rule's areas, the cell Excel reads relative references from.
pub(super) fn cf_rule_xml(cf: &CondFormat, priority: u32, out: &mut Out) -> String {
    let tl = RangeRef::top_left(&cf.ranges).map(|c| c.a1()).unwrap_or_else(|| "A1".into());
    let stop = if cf.stop_if_true { " stopIfTrue=\"1\"" } else { "" };
    let mut dxf = |st: &Style| dxf_id(&mut out.dxfs, st);
    let f = |x: &str| format!("<formula>{}</formula>", esc(&text_to_file(x)));
    let simple = |ty: &str, st: &Style, extra: &str, body: String, dxf: &mut dyn FnMut(&Style) -> u32| {
        format!("<cfRule type=\"{ty}\" dxfId=\"{}\" priority=\"{priority}\"{stop}{extra}>{body}</cfRule>", dxf(st))
    };
    match &cf.rule {
        CfRule::CellIs { op, a, b, style } => {
            let mut body = f(a);
            if let Some(b) = b {
                body.push_str(&f(b));
            }
            simple("cellIs", style, &format!(" operator=\"{}\"", cf_op_name(*op)), body, &mut dxf)
        }
        CfRule::Expression { formula, style } => simple("expression", style, "", f(formula), &mut dxf),
        CfRule::ContainsText { text, style } => simple(
            "containsText",
            style,
            &format!(" operator=\"containsText\" text=\"{}\"", esc_attr(text)),
            f(&format!("NOT(ISERROR(SEARCH({},{tl})))", quote(text))),
            &mut dxf,
        ),
        CfRule::NotContainsText { text, style } => simple(
            "notContainsText",
            style,
            &format!(" operator=\"notContains\" text=\"{}\"", esc_attr(text)),
            f(&format!("ISERROR(SEARCH({},{tl}))", quote(text))),
            &mut dxf,
        ),
        CfRule::BeginsWith { text, style } => simple(
            "beginsWith",
            style,
            &format!(" operator=\"beginsWith\" text=\"{}\"", esc_attr(text)),
            f(&format!("LEFT({tl},LEN({0}))={0}", quote(text))),
            &mut dxf,
        ),
        CfRule::EndsWith { text, style } => simple(
            "endsWith",
            style,
            &format!(" operator=\"endsWith\" text=\"{}\"", esc_attr(text)),
            f(&format!("RIGHT({tl},LEN({0}))={0}", quote(text))),
            &mut dxf,
        ),
        CfRule::Blanks { style } => simple("containsBlanks", style, "", f(&format!("LEN(TRIM({tl}))=0")), &mut dxf),
        CfRule::NoBlanks { style } => simple("notContainsBlanks", style, "", f(&format!("LEN(TRIM({tl}))>0")), &mut dxf),
        CfRule::Errors { style } => simple("containsErrors", style, "", f(&format!("ISERROR({tl})")), &mut dxf),
        CfRule::NoErrors { style } => simple("notContainsErrors", style, "", f(&format!("NOT(ISERROR({tl}))")), &mut dxf),
        CfRule::Duplicate { style } => simple("duplicateValues", style, "", String::new(), &mut dxf),
        CfRule::Unique { style } => simple("uniqueValues", style, "", String::new(), &mut dxf),
        CfRule::Top10 { bottom, percent, rank, style } => {
            let extra = format!("{}{} rank=\"{}\"", if *percent { " percent=\"1\"" } else { "" }, if *bottom { " bottom=\"1\"" } else { "" }, rank);
            simple("top10", style, &extra, String::new(), &mut dxf)
        }
        CfRule::AboveAverage { below, equal, std_dev, style } => {
            let mut extra = String::new();
            if *below {
                extra.push_str(" aboveAverage=\"0\"");
            }
            if *equal {
                extra.push_str(" equalAverage=\"1\"");
            }
            if *std_dev > 0 {
                let _ = write!(extra, " stdDev=\"{std_dev}\"");
            }
            simple("aboveAverage", style, &extra, String::new(), &mut dxf)
        }
        CfRule::TimePeriod { period, style } => {
            let d = format!("FLOOR({tl},1)");
            let body = match period.as_str() {
                "yesterday" => format!("{d}=TODAY()-1"),
                "tomorrow" => format!("{d}=TODAY()+1"),
                "last7Days" => format!("AND(TODAY()-{d}<=6,{d}<=TODAY())"),
                "thisMonth" => format!("AND(MONTH({tl})=MONTH(TODAY()),YEAR({tl})=YEAR(TODAY()))"),
                "lastMonth" => format!("AND(MONTH({tl})=MONTH(EDATE(TODAY(),0-1)),YEAR({tl})=YEAR(EDATE(TODAY(),0-1)))"),
                "nextMonth" => format!("AND(MONTH({tl})=MONTH(EDATE(TODAY(),0+1)),YEAR({tl})=YEAR(EDATE(TODAY(),0+1)))"),
                "thisWeek" => format!("AND(TODAY()-ROUNDDOWN({tl},0)<=WEEKDAY(TODAY())-1,ROUNDDOWN({tl},0)-TODAY()<=7-WEEKDAY(TODAY()))"),
                "lastWeek" => format!("AND(TODAY()-ROUNDDOWN({tl},0)>=(WEEKDAY(TODAY())),TODAY()-ROUNDDOWN({tl},0)<(WEEKDAY(TODAY())+7))"),
                "nextWeek" => format!("AND(ROUNDDOWN({tl},0)-TODAY()>(7-WEEKDAY(TODAY())),ROUNDDOWN({tl},0)-TODAY()<(15-WEEKDAY(TODAY())))"),
                _ => format!("{d}=TODAY()"),
            };
            simple("timePeriod", style, &format!(" timePeriod=\"{}\"", esc_attr(period)), f(&body), &mut dxf)
        }
        CfRule::ColorScale { stops } => {
            let mut b = String::from("<colorScale>");
            for (v, _) in stops {
                b.push_str(&cfvo_xml(v));
            }
            for (_, c) in stops {
                let _ = write!(b, "<color {}/>", color_attrs(c));
            }
            b.push_str("</colorScale>");
            format!("<cfRule type=\"colorScale\" priority=\"{priority}\"{stop}>{b}</cfRule>")
        }
        CfRule::DataBar { min, max, color, show_value, gradient } => format!(
            "<cfRule type=\"dataBar\" priority=\"{priority}\"{stop}><dataBar{}>{}{}<color {}/></dataBar>{}</cfRule>",
            if *show_value { "" } else { " showValue=\"0\"" },
            cfvo_xml(min),
            cfvo_xml(max),
            if *color == Color::Auto { "rgb=\"FF638EC6\"".to_string() } else { color_attrs(color) },
            if *gradient { String::new() } else { solid_bar_ext(priority) }
        ),
        CfRule::IconSet { set, thresholds, reverse, show_value } => {
            let mut b = format!("<iconSet iconSet=\"{}\"", esc_attr(set));
            if *reverse {
                b.push_str(" reverse=\"1\"");
            }
            if !*show_value {
                b.push_str(" showValue=\"0\"");
            }
            b.push('>');
            // CT_IconSet needs one cfvo per icon (minOccurs 2); the first is the lower bound.
            let k = icon_count(set);
            let defaults = default_icon_thresholds(set);
            let cfvos: Vec<&CfValueKind> = match thresholds.len() {
                n if n == k => thresholds.iter().collect(),
                n if n + 1 == k => defaults.iter().take(1).chain(thresholds).collect(),
                _ => defaults.iter().collect(),
            };
            for t in cfvos {
                b.push_str(&cfvo_xml(t));
            }
            b.push_str("</iconSet>");
            format!("<cfRule type=\"iconSet\" priority=\"{priority}\"{stop}>{b}</cfRule>")
        }
    }
}

/// The x14 rule a solid data bar's `cfRule` links to (it holds the fill); `None` for other rules.
/// GridCraft draws negative values in the bar's colour from the low end, with no axis; the x14
/// defaults are an automatic axis and a separate negative colour, so both are written out.
pub(super) fn solid_bar_x14(cf: &CondFormat, priority: u32) -> Option<String> {
    let CfRule::DataBar { min, max, gradient: false, .. } = &cf.rule else { return None };
    let (id, lo, hi) = (solid_bar_id(priority), x14_cfvo(min), x14_cfvo(max));
    Some(format!(
        "<x14:cfRule type=\"dataBar\" id=\"{id}\"><x14:dataBar gradient=\"0\" negativeBarColorSameAsPositive=\"1\" axisPosition=\"none\">{lo}{hi}</x14:dataBar></x14:cfRule>"
    ))
}

/// Whether formula `f` refers to a sheet other than `own`, and whether it refers to a 3-D span of
/// sheets.
fn sheet_refs(f: &str, own: &str) -> (bool, bool) {
    let Ok(e) = gridcraft_formula::parse(f.trim_start_matches('=')) else { return (false, false) };
    let refs = e.references();
    let other = refs.iter().any(|r| match &r.sheet {
        gridcraft_formula::SheetSel::Named(n) => !n.chars().flat_map(char::to_lowercase).eq(own.chars().flat_map(char::to_lowercase)),
        _ => false,
    });
    let span = refs.iter().any(|r| matches!(r.sheet, gridcraft_formula::SheetSel::Span(..)));
    (other, span)
}

/// The Excel 2010 form (`x14:cfRule`, `xm:f` formulas, inline `x14:dxf`) of a cell-value or
/// formula rule that refers to another sheet; `None` for rules that belong in the main section.
/// MS-XLSX 2.6.27 (CT_CfRule) forbids 3-D spans (`sheet-range-reference`) in `xm:f`, so a rule
/// with one stays in the main section.
pub(super) fn x14_cf_rule_xml(cf: &CondFormat, own: &str, priority: u32, out: &mut Out) -> Option<String> {
    let (ty, extra, formulas, style): (&str, String, Vec<&str>, &Style) = match &cf.rule {
        CfRule::CellIs { op, a, b, style } => {
            ("cellIs", format!(" operator=\"{}\"", cf_op_name(*op)), std::iter::once(a.as_str()).chain(b.as_deref()).collect(), style)
        }
        CfRule::Expression { formula, style } => ("expression", String::new(), vec![formula.as_str()], style),
        _ => return None,
    };
    let refs: Vec<(bool, bool)> = formulas.iter().map(|f| sheet_refs(f, own)).collect();
    if !refs.iter().any(|&(other, _)| other) || refs.iter().any(|&(_, span)| span) {
        return None;
    }
    out.x14_cf_rules += 1;
    let mut s = format!(
        "<x14:cfRule type=\"{ty}\" priority=\"{priority}\"{}{extra} id=\"{}\">",
        if cf.stop_if_true { " stopIfTrue=\"1\"" } else { "" },
        x14_cf_rule_guid(out.x14_cf_rules)
    );
    for f in formulas {
        let _ = write!(s, "<xm:f>{}</xm:f>", esc(&text_to_file(f)));
    }
    s.push_str(&dxf_xml("x14:dxf", style, &mut out.num_fmts));
    s.push_str("</x14:cfRule>");
    Some(s)
}

fn validation_formula(f: &str) -> String {
    let f = f.trim();
    if f.starts_with('"') {
        return esc(f);
    }
    esc(&text_to_file(f.strip_prefix('=').unwrap_or(f)))
}

pub(super) fn validation_xml(v: &Validation) -> String {
    let ty = match v.kind {
        ValidationKind::Any => "none",
        ValidationKind::Whole => "whole",
        ValidationKind::Decimal => "decimal",
        ValidationKind::List => "list",
        ValidationKind::Date => "date",
        ValidationKind::Time => "time",
        ValidationKind::TextLength => "textLength",
        ValidationKind::Custom => "custom",
    };
    let mut s = format!("<dataValidation type=\"{ty}\"");
    match v.error_style {
        ErrorStyle::Stop => {}
        ErrorStyle::Warning => s.push_str(" errorStyle=\"warning\""),
        ErrorStyle::Information => s.push_str(" errorStyle=\"information\""),
    }
    if v.op != CfOperator::Between && !matches!(v.kind, ValidationKind::List | ValidationKind::Custom | ValidationKind::Any) {
        let _ = write!(s, " operator=\"{}\"", cf_op_name(v.op));
    }
    if v.allow_blank {
        s.push_str(" allowBlank=\"1\"");
    }
    if !v.in_cell_dropdown {
        s.push_str(" showDropDown=\"1\"");
    }
    if v.show_input {
        s.push_str(" showInputMessage=\"1\"");
    }
    if v.show_error {
        s.push_str(" showErrorMessage=\"1\"");
    }
    for (a, t) in [("errorTitle", &v.error_title), ("error", &v.error_message), ("promptTitle", &v.input_title), ("prompt", &v.input_message)] {
        if !t.is_empty() {
            let _ = write!(s, " {a}=\"{}\"", esc_attr(t));
        }
    }
    let _ = write!(s, " sqref=\"{}\">", sqref(&v.ranges));
    if !v.f1.is_empty() {
        let _ = write!(s, "<formula1>{}</formula1>", validation_formula(&v.f1));
    }
    if let Some(f2) = v.f2.as_ref().filter(|f| !f.is_empty()) {
        let _ = write!(s, "<formula2>{}</formula2>", validation_formula(f2));
    }
    s.push_str("</dataValidation>");
    s
}
