use std::collections::HashMap;

use super::*;

/// Valid table name: letters, digits, `_` and `.`, starting with a letter, `_` or `\`
/// (Microsoft "Names in formulas" rules).
pub(super) fn table_name(name: &str, used: &mut HashSet<String>, n: u32) -> String {
    let mut s: String =
        name.chars().enumerate().map(|(i, c)| if c.is_alphanumeric() || c == '_' || c == '.' || (i == 0 && c == '\\') { c } else { '_' }).collect();
    if s.is_empty() || !s.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '\\') || gridcraft_core::CellRef::parse(&s).is_some() {
        s = format!("Table{n}");
    }
    let mut out = s.clone();
    let mut k = 2;
    while !used.insert(out.to_ascii_lowercase()) {
        out = format!("{s}{k}");
        k += 1;
    }
    out
}

/// Column names for a table: one per column of its range, unique and non-empty.
pub(super) fn table_columns(t: &Table) -> Vec<String> {
    let w = t.range.width() as usize;
    let mut used = HashSet::new();
    // The next suffix to try per lowercased base, so repeated headers don't retry every suffix
    // already handed out.
    let mut next: HashMap<String, u32> = HashMap::new();
    (0..w)
        .map(|i| {
            let base = t.columns.get(i).map(|c| c.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Column{}", i + 1));
            let lower = base.to_lowercase();
            if used.insert(lower.clone()) {
                return base;
            }
            let k = next.entry(lower).or_insert(2);
            loop {
                let name = format!("{base}{k}");
                *k += 1;
                if used.insert(name.to_lowercase()) {
                    return name;
                }
            }
        })
        .collect()
}

fn filter_operator(op: &str) -> &'static str {
    match op.trim() {
        "<" => " operator=\"lessThan\"",
        "<=" => " operator=\"lessThanOrEqual\"",
        "<>" => " operator=\"notEqual\"",
        ">=" => " operator=\"greaterThanOrEqual\"",
        ">" => " operator=\"greaterThan\"",
        _ => "",
    }
}

pub(super) fn autofilter_xml(range: RangeRef, criteria: &[(u32, FilterCriterion)], out: &mut Out) -> String {
    let mut s = format!("<autoFilter ref=\"{}\"", range.a1());
    if criteria.is_empty() {
        s.push_str("/>");
        return s;
    }
    s.push('>');
    for (col, c) in criteria {
        let _ = write!(s, "<filterColumn colId=\"{col}\">");
        match c {
            FilterCriterion::Values { values, blanks } => {
                let _ = write!(s, "<filters{}>", if *blanks { " blank=\"1\"" } else { "" });
                for v in values {
                    let _ = write!(s, "<filter val=\"{}\"/>", esc_attr(v));
                }
                s.push_str("</filters>");
            }
            FilterCriterion::Custom { a, b, and } => {
                let _ = write!(s, "<customFilters{}>", if *and { " and=\"1\"" } else { "" });
                for (op, v) in std::iter::once(a).chain(b.iter()) {
                    let _ = write!(s, "<customFilter{} val=\"{}\"/>", filter_operator(op), esc_attr(v));
                }
                s.push_str("</customFilters>");
            }
            FilterCriterion::Top10 { bottom, percent, count } => {
                let _ =
                    write!(s, "<top10{}{} val=\"{count}\"/>", if *bottom { " top=\"0\"" } else { "" }, if *percent { " percent=\"1\"" } else { "" });
            }
            FilterCriterion::AboveAverage(above) => {
                let _ = write!(s, "<dynamicFilter type=\"{}\"/>", if *above { "aboveAverage" } else { "belowAverage" });
            }
            FilterCriterion::FillColor(c) => {
                let st = Style { fill: gridcraft_model::Fill::solid(*c), ..Default::default() };
                let _ = write!(s, "<colorFilter dxfId=\"{}\"/>", dxf_id(&mut out.dxfs, &st));
            }
            FilterCriterion::FontColor(c) => {
                let mut st = Style::default();
                st.font.color = *c;
                let _ = write!(s, "<colorFilter dxfId=\"{}\" cellColor=\"0\"/>", dxf_id(&mut out.dxfs, &st));
            }
            FilterCriterion::Icon { set, id } => {
                let _ = write!(s, "<iconFilter iconSet=\"{}\"", esc_attr(set));
                if let Some(id) = id {
                    let _ = write!(s, " iconId=\"{id}\"");
                }
                s.push_str("/>");
            }
        }
        s.push_str("</filterColumn>");
    }
    s.push_str("</autoFilter>");
    s
}

pub(super) fn table_xml(t: &Table, cols: &[String], name: &str, id: u32, out: &mut Out) -> String {
    let mut s =
        format!("{XML_DECL}<table xmlns=\"{NS_MAIN}\" id=\"{id}\" name=\"{0}\" displayName=\"{0}\" ref=\"{1}\"", esc_attr(name), t.range.a1());
    if !t.header_row {
        s.push_str(" headerRowCount=\"0\"");
    }
    if t.totals_row {
        s.push_str(" totalsRowCount=\"1\"");
    } else {
        s.push_str(" totalsRowShown=\"0\"");
    }
    if let Some(ty) = &t.table_type {
        let _ = write!(s, " tableType=\"{}\"", esc_attr(ty));
    }
    if let Some(c) = t.connection_id {
        let _ = write!(s, " connectionId=\"{c}\"");
    }
    s.push('>');
    if t.header_row && t.filter_button {
        let end_row = if t.totals_row { t.range.end.row.saturating_sub(1).max(t.range.start.row) } else { t.range.end.row };
        let r = RangeRef::new(t.range.start, CellRef::new(end_row, t.range.end.col));
        s.push_str(&autofilter_xml(r, &t.filter, out));
    }
    let _ = write!(s, "<tableColumns count=\"{}\">", cols.len());
    for (i, name) in cols.iter().enumerate() {
        let col = t.columns.get(i);
        let _ = write!(s, "<tableColumn id=\"{}\"", i + 1);
        if let Some(u) = col.and_then(|c| c.unique_name.as_ref()) {
            let _ = write!(s, " uniqueName=\"{}\"", esc_attr(u));
        }
        let _ = write!(s, " name=\"{}\"", esc_attr(&encode_xstring(name)));
        let totals = col.map(|c| c.totals).unwrap_or_default();
        // A hidden Total Row keeps each column's calculation, as Excel's files do: turning the row
        // back on restores it (#4296). The reader takes them whether the row is shown or not.
        let f = match totals {
            TotalsFn::None => None,
            TotalsFn::Average => Some("average"),
            TotalsFn::Count => Some("count"),
            TotalsFn::CountNums => Some("countNums"),
            TotalsFn::Max => Some("max"),
            TotalsFn::Min => Some("min"),
            TotalsFn::Sum => Some("sum"),
            TotalsFn::StdDev => Some("stdDev"),
            TotalsFn::Var => Some("var"),
            TotalsFn::Custom => Some("custom"),
        };
        if let Some(f) = f {
            let _ = write!(s, " totalsRowFunction=\"{f}\"");
        } else if let Some(l) = col.and_then(|c| c.totals_label.as_ref()) {
            let _ = write!(s, " totalsRowLabel=\"{}\"", esc_attr(l));
        }
        let calc = col.and_then(|c| c.formula.as_ref()).filter(|f| !f.is_empty());
        let custom = if totals == TotalsFn::Custom { col.and_then(|c| c.totals_label.as_ref()) } else { None };
        let xml = col.and_then(|c| c.xml.as_ref());
        if calc.is_none() && custom.is_none() && xml.is_none() {
            s.push_str("/>");
            continue;
        }
        s.push('>');
        if let Some(f) = calc {
            let _ = write!(s, "<calculatedColumnFormula>{}</calculatedColumnFormula>", esc(&text_to_file(f)));
        }
        if let Some(f) = custom {
            let _ = write!(s, "<totalsRowFormula>{}</totalsRowFormula>", esc(&text_to_file(f)));
        }
        if let Some(x) = xml {
            let _ = write!(s, "<xmlColumnPr mapId=\"{}\" xpath=\"{}\"", x.map_id, esc_attr(&x.xpath));
            if x.denormalized {
                s.push_str(" denormalized=\"1\"");
            }
            let _ = write!(s, " xmlDataType=\"{}\"/>", esc_attr(&x.data_type));
        }
        s.push_str("</tableColumn>");
    }
    s.push_str("</tableColumns>");
    let style = if t.style.is_empty() { "TableStyleMedium2" } else { &t.style };
    let _ = write!(
        s,
        "<tableStyleInfo name=\"{}\" showFirstColumn=\"{}\" showLastColumn=\"{}\" showRowStripes=\"{}\" showColumnStripes=\"{}\"/></table>",
        esc_attr(style),
        t.first_col as u8,
        t.last_col as u8,
        t.banded_rows as u8,
        t.banded_cols as u8
    );
    s
}

/// Ids of the threaded comments of sheet `si`: the thread's id, then one per reply. GUID-shaped
/// and deterministic; they only need to be unique within the file.
pub(super) fn thread_ids(sheet: &Sheet, si: usize) -> BTreeMap<CellRef, Vec<String>> {
    let mut seq: u64 = 0;
    let mut ids = BTreeMap::new();
    for (cell, c) in sheet.comments.iter().filter(|(_, c)| c.threaded) {
        let v: Vec<String> = (0..=c.replies.len())
            .map(|_| {
                seq += 1;
                format!("{{{:08X}-0000-4000-8001-{:012X}}}", si as u64 & 0xFFFF_FFFF, seq & 0xFFFF_FFFF_FFFF)
            })
            .collect();
        ids.insert(*cell, v);
    }
    ids
}

pub(super) fn comments_xml(sheet: &Sheet, ids: &BTreeMap<CellRef, Vec<String>>) -> String {
    // A threaded comment's legacy note is authored by `tc={thread id}`.
    let author_of = |cell: &CellRef, c: &gridcraft_model::Comment| match ids.get(cell).and_then(|v| v.first()) {
        Some(id) => format!("tc={id}"),
        None => c.author.clone(),
    };
    let mut authors: Vec<String> = vec![];
    for (cell, c) in &sheet.comments {
        let a = author_of(cell, c);
        if !authors.contains(&a) {
            authors.push(a);
        }
    }
    let mut s = if ids.is_empty() {
        format!("{XML_DECL}<comments xmlns=\"{NS_MAIN}\"><authors>")
    } else {
        format!(
            "{XML_DECL}<comments xmlns=\"{NS_MAIN}\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"xr\" xmlns:xr=\"http://schemas.microsoft.com/office/spreadsheetml/2014/revision\"><authors>"
        )
    };
    for a in &authors {
        let _ = write!(s, "<author>{}</author>", esc(a));
    }
    s.push_str("</authors><commentList>");
    for (cell, c) in &sheet.comments {
        let a = author_of(cell, c);
        let aid = authors.iter().position(|x| *x == a).unwrap_or(0);
        let mut text = c.text.clone();
        // Threaded replies are kept as text lines.
        for (who, reply) in &c.replies {
            let _ = write!(text, "\n{who}: {reply}");
        }
        let link = match ids.get(cell).and_then(|v| v.first()) {
            Some(id) => format!(" shapeId=\"0\" xr:uid=\"{id}\""),
            None => String::new(),
        };
        let _ = write!(s, "<comment ref=\"{}\" authorId=\"{aid}\"{link}><text><r>{}</r></text></comment>", cell.a1(), crate::xml::t_el(&text));
    }
    s.push_str("</commentList></comments>");
    s
}

/// `2024-05-01T09:30:00` with optional fraction and zone: what `dT` (xsd:dateTime) accepts.
/// Anything else is left out rather than written into the file.
fn iso_datetime(d: &str) -> bool {
    let b = d.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    d.len() <= 40
        && digits(0..4)
        && b.get(4) == Some(&b'-')
        && digits(5..7)
        && b.get(7) == Some(&b'-')
        && digits(8..10)
        && b.get(10) == Some(&b'T')
        && digits(11..13)
        && b.get(13) == Some(&b':')
        && digits(14..16)
        && b.get(16) == Some(&b':')
        && digits(17..19)
        && b.get(19..).is_some_and(|rest| rest.iter().all(|c| c.is_ascii_digit() || b".:+-Z".contains(c)))
}

/// `xl/threadedComments/threadedCommentN.xml`: each thread, then its replies (`parentId`).
pub(super) fn threaded_xml(sheet: &Sheet, ids: &BTreeMap<CellRef, Vec<String>>, persons: &mut Vec<String>) -> String {
    let mut person = |name: &str| {
        let i = persons.iter().position(|p| p == name).unwrap_or_else(|| {
            persons.push(name.to_string());
            persons.len() - 1
        });
        crate::write::person_guid(i)
    };
    let mut s = format!("{XML_DECL}<ThreadedComments xmlns=\"{NS_THREADED}\" xmlns:x=\"{NS_MAIN}\">");
    for (cell, id) in ids {
        let Some(c) = sheet.comments.get(cell) else { continue };
        let entries = std::iter::once((&c.author, &c.text)).chain(c.replies.iter().map(|(a, t)| (a, t)));
        for (k, ((author, text), own)) in entries.zip(id).enumerate() {
            let date = match c.dates.get(k).filter(|d| iso_datetime(d)) {
                Some(d) => format!(" dT=\"{}\"", esc_attr(d)),
                None => String::new(),
            };
            let tail = match (k, id.first()) {
                (0, _) if c.resolved => " done=\"1\"".to_string(),
                (0, _) | (_, None) => String::new(),
                (_, Some(parent)) => format!(" parentId=\"{parent}\""),
            };
            let _ = write!(
                s,
                "<threadedComment ref=\"{}\"{date} personId=\"{}\" id=\"{own}\"{tail}><text>{}</text></threadedComment>",
                cell.a1(),
                person(author),
                esc(text)
            );
        }
    }
    s.push_str("</ThreadedComments>");
    s
}

/// A protection password hash as attributes, each name preceded by `prefix` (see
/// [`crate::sheet_read::read_password`]).
pub(crate) fn password_attrs(h: &gridcraft_model::PasswordHash, prefix: &str) -> String {
    let name = |base: &str| crate::sheet_read::password_attr(prefix, base);
    match h {
        gridcraft_model::PasswordHash::Legacy(h) => format!(" {}=\"{h:04X}\"", name("password")),
        gridcraft_model::PasswordHash::Iterated { algorithm, hash, salt, spin_count } => format!(
            " {}=\"{}\" {}=\"{}\" {}=\"{}\" {}=\"{spin_count}\"",
            name("algorithmName"),
            esc_attr(algorithm),
            name("hashValue"),
            esc_attr(hash),
            name("saltValue"),
            esc_attr(salt),
            name("spinCount")
        ),
    }
}

/// Allow Edit Ranges: `<protectedRanges>` (after `sheetProtection`).
pub(super) fn protected_ranges_xml(sheet: &Sheet) -> String {
    let mut s = String::new();
    for r in sheet.edit_ranges.iter().filter(|r| !r.ranges.is_empty()) {
        let sqref: Vec<String> = r.ranges.iter().map(|x| x.a1()).collect();
        let _ = write!(s, "<protectedRange name=\"{}\" sqref=\"{}\"", esc_attr(&r.title), sqref.join(" "));
        if let Some(h) = &r.password {
            s.push_str(&password_attrs(h, ""));
        }
        if let Some(sd) = &r.security_descriptor {
            let _ = write!(s, " securityDescriptor=\"{}\"", esc_attr(sd));
        }
        s.push_str("/>");
    }
    if s.is_empty() { s } else { format!("<protectedRanges>{s}</protectedRanges>") }
}

/// A note box's `<x:Anchor>` text and its width and height in points: the box read from the file
/// or [`DEFAULT_NOTE_BOX`], its corners moved with the note's `cell` and kept on the sheet.
fn note_anchor(cell: CellRef, b: Option<&NoteBox>) -> (String, (f64, f64)) {
    let b = b.unwrap_or(&DEFAULT_NOTE_BOX);
    let at = |base: u32, d: i64, max: u32| (i64::from(base) + d).clamp(0, i64::from(max) - 1);
    let corner = |k: &NoteCorner| format!("{}, {}, {}, {}", at(cell.col, k.col, MAX_COLS), k.col_px, at(cell.row, k.row, MAX_ROWS), k.row_px);
    (format!("{}, {}", corner(&b.from), corner(&b.to)), b.size_pt.unwrap_or(DEFAULT_NOTE_SIZE_PT))
}

/// Legacy VML shapes Excel uses to display notes (numbered from `base + 1`), followed by the
/// form control shapes `controls`. `idmap` lists the shape blocks the drawing uses.
pub(super) fn vml_xml(sheet: &Sheet, idmap: &str, base: u64, controls: &str) -> String {
    let mut s = format!(
        "<xml xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:x=\"urn:schemas-microsoft-com:office:excel\"><o:shapelayout v:ext=\"edit\"><o:idmap v:ext=\"edit\" data=\"{idmap}\"/></o:shapelayout><v:shapetype id=\"_x0000_t202\" coordsize=\"21600,21600\" o:spt=\"202\" path=\"m,l,21600r21600,l21600,xe\"><v:stroke joinstyle=\"miter\"/><v:path gradientshapeok=\"t\" o:connecttype=\"rect\"/></v:shapetype>"
    );
    for (i, (cell, c)) in sheet.comments.iter().enumerate() {
        let id = base + 1 + i as u64;
        let vis = if c.visible { "visible" } else { "hidden" };
        let (anchor, (w, h)) = note_anchor(*cell, c.note_box.as_ref());
        let _ = write!(
            s,
            "<v:shape id=\"_x0000_s{id}\" type=\"#_x0000_t202\" style=\"position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:{}pt;height:{}pt;z-index:{};visibility:{vis}\" fillcolor=\"#ffffe1\" o:insetmode=\"auto\"><v:fill color2=\"#ffffe1\"/><v:shadow on=\"t\" color=\"black\" obscured=\"t\"/><v:path o:connecttype=\"none\"/><v:textbox style=\"mso-direction-alt:auto\"><div style=\"text-align:left\"></div></v:textbox><x:ClientData ObjectType=\"Note\"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>{anchor}</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column>{}</x:ClientData></v:shape>",
            num(w),
            num(h),
            i + 1,
            cell.row,
            cell.col,
            if c.visible { "<x:Visible/>" } else { "" }
        );
    }
    s.push_str(controls);
    s.push_str("</xml>");
    s
}

#[cfg(test)]
mod tests {
    use gridcraft_model::TableColumn;

    use super::*;

    #[test]
    fn repeated_headers_are_renamed_without_retrying_suffixes() {
        let col = |n: &str| TableColumn { name: n.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None };
        let mut columns: Vec<TableColumn> = (0..10_000).map(|_| col("Total")).collect();
        columns[1] = col("total3");
        let t = Table {
            id: 1,
            name: "T".into(),
            range: RangeRef::parse("A1:NTP2").unwrap(),
            header_row: true,
            totals_row: false,
            columns,
            style: String::new(),
            banded_rows: false,
            banded_cols: false,
            first_col: false,
            last_col: false,
            filter_button: false,
            filter: vec![],
            table_type: None,
            connection_id: None,
        };
        let names = table_columns(&t);
        assert_eq!(names.len(), 10_000);
        assert_eq!(&names[..4], ["Total", "total3", "Total2", "Total4"]);
        assert_eq!(names.iter().map(|n| n.to_lowercase()).collect::<HashSet<_>>().len(), 10_000);
    }
}
