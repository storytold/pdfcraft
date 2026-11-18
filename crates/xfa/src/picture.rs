//! Picture clauses that list alternatives (`date{YYYYMMDD}|date{YYMMDD}`): a date may be typed
//! by any of them, and is shown, and kept in the field, by the first.

use crate::data::{date_letters, iso_to_pattern, pattern_to_iso, template_field};
use crate::layout::date_pattern;
use crate::model::{Template, Ui};

/// The alternatives of a picture clause: its parts between `|`s that aren't inside braces or
/// quotes.
pub(crate) fn alternatives(picture: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quoted, mut start) = (0usize, false, 0);
    for (i, c) in picture.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '{' if !quoted => depth = depth.saturating_add(1),
            '}' if !quoted => depth = depth.saturating_sub(1),
            '|' if !quoted && depth == 0 => {
                out.extend(picture.get(start..i));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.extend(picture.get(start..));
    out
}

/// `text` typed into the date field at `som` by one of its edit picture's alternatives after the
/// first, written in the first, which the field shows and checks typed dates against (`261018`
/// becomes `20261018` for `date{YYYYMMDD}|date{YYMMDD}`). A pattern reads the text only when the
/// date it gives, written in that pattern, is the text again, and only date clauses count. `None`
/// when the field isn't a date field with alternatives, the first pattern reads the text, no
/// other does, or the first pattern can't hold the date (a two-digit year for 1901).
pub fn typed_date(tpl: &Template, som: &str, text: &str) -> Option<String> {
    let f = template_field(tpl, som)?;
    if f.ui != Ui::DateTimeEdit {
        return None;
    }
    let picture = f.picture.as_deref()?;
    let reads = |pattern: &str| pattern_to_iso(text, pattern).filter(|iso| iso_to_pattern(iso, pattern).eq_ignore_ascii_case(text.trim()));
    let mut alts = alternatives(picture).into_iter();
    let first = alts.next().filter(|a| is_date_clause(a) && date_letters(a).is_some()).map(|a| date_pattern(Some(a)))?;
    if reads(&first).is_some() {
        return None;
    }
    // The first later date pattern that reads the text and gives a date the first pattern keeps:
    // written in the first, it must read back as the same date (a two-digit year can't hold 1901
    // or 1203), else the next pattern is tried.
    alts.filter(|a| is_date_clause(a)).filter_map(date_letters).find_map(|p| {
        let iso = reads(&p)?;
        let shown = iso_to_pattern(&iso, &first);
        (pattern_to_iso(&shown, &first).as_deref() == Some(iso.as_str())).then_some(shown)
    })
}

/// Whether picture clause `clause` is a date pattern: a `date{…}` clause (with or without a
/// locale, `date(en_US){…}`), or a bare pattern, which a date field reads as a date. A clause of
/// another category (`text{…}`, `num{…}`, `time{…}`) is not, whatever its letters.
fn is_date_clause(clause: &str) -> bool {
    let clause = clause.trim();
    match clause.find(['{', '(']) {
        Some(i) => clause.get(..i).is_some_and(|category| category.trim().eq_ignore_ascii_case("date")),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl_with_pictures(fields: &[(&str, &str)]) -> Template {
        let mut body = String::new();
        for (name, pic) in fields {
            body.push_str(&format!(
                "<field name=\"{name}\" w=\"2in\" h=\"0.3in\"><ui><dateTimeEdit/><picture>{pic}</picture></ui><value><date/></value></field>"
            ));
        }
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\">\
             <template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\">\
             <subform name=\"form\" layout=\"tb\">\
             <pageSet><pageArea name=\"front\"><contentArea x=\"0.5in\" y=\"0.5in\" w=\"7.5in\" h=\"10in\"/><medium short=\"8.5in\" long=\"11in\"/></pageArea></pageSet>\
             {body}\
             </subform></template></xdp:xdp>"
        );
        crate::parse(&xml).expect("valid template").0
    }

    #[test]
    fn typed_date_reads_bare_first_alternative_instead_of_whole_picture_or_later_braces() {
        let tpl = tpl_with_pictures(&[("bare", "YYYYMMDD|YYMMDD"), ("mixed", "YYYYMMDD|date{YYMMDD}")]);
        assert_eq!(typed_date(&tpl, "form[0].bare[0]", "261018").as_deref(), Some("20261018"));
        assert_eq!(typed_date(&tpl, "form[0].bare[0]", "20261018"), None);
        assert_eq!(typed_date(&tpl, "form[0].mixed[0]", "261018").as_deref(), Some("20261018"));
        assert_eq!(typed_date(&tpl, "form[0].mixed[0]", "20261018"), None);
    }

    #[test]
    fn typed_date_rejects_non_date_or_unreadable_first_alternative() {
        let tpl = tpl_with_pictures(&[
            ("text_first", "text{YYYYMMDD}|date{YYMMDD}"),
            ("num_first", "num{99999999}|date{YYMMDD}"),
            ("datetime_first", "date{YYYY-MM-DD HH:MM}|date{YYMMDD}"),
        ]);
        assert_eq!(typed_date(&tpl, "form[0].text_first[0]", "261018"), None);
        assert_eq!(typed_date(&tpl, "form[0].num_first[0]", "261018"), None);
        assert_eq!(typed_date(&tpl, "form[0].datetime_first[0]", "261018"), None);
    }
}
