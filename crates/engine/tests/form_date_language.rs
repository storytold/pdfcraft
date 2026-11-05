//! Form date fields name months in the app's date language (#5303). Its own test binary: the
//! names are process-wide, so other tests must not see them change, and its tests take turns.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pdfcraft_cos::{Document, Object};
use pdfcraft_engine::dates::use_form_date_language;
use pdfcraft_engine::js::JsRunner;
use pdfcraft_forms::FieldValue;
use pdfcraft_forms::af::{DateTime, Format, format_date, format_value, keystroke, parse_date};
use pdfcraft_js::{DocInfo, Event, Limits, run};

/// Held by each test while it changes the process-wide date names, which start as English (a
/// failed test may have left others).
fn names_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    use_form_date_language("en");
    lock
}

/// `util.printd(fmt)` of 18 October 2026, then of what `util.scand(fmt, typed)` reads.
fn printd(fmt: &str, typed: &str) -> String {
    let doc = DocInfo { file_name: "form.pdf".into(), num_pages: 1, page: 0, info: Vec::new(), xfa: false };
    let script = format!("event.value = util.printd('{fmt}', new Date(2026, 9, 18)) + '|' + util.printd('{fmt}', util.scand('{fmt}', '{typed}'));");
    let o = run(&script, &Event::field("Format", "date", ""), &doc, &[], &[], Limits::default());
    assert_eq!(o.error, None);
    o.value
}

#[test]
fn form_dates_follow_the_date_language() {
    let _names = names_lock();
    let oct18 = DateTime { y: 2026, m: 10, d: 18, hh: 0, mm: 0, ss: 0 };
    let dd_mmm_yy = Format::Date("dd-mmm-yy".into());
    // English until the app says otherwise.
    assert_eq!(format_date(oct18, "dd-mmm-yy dddd ddd mmmm"), "18-Oct-26 Sunday Sun October");
    use_form_date_language("it");
    // AFDate_FormatEx("dd-mmm-yy"), what the issue's form used.
    assert_eq!(format_value(&dd_mmm_yy, "18-10-26"), "18-ott-26");
    assert_eq!(format_date(oct18, "dddd d mmmm yyyy, ddd"), "domenica 18 ottobre 2026, dom");
    // What it writes reads back, and so does English typing.
    assert_eq!(parse_date("18-ott-26", "dd-mmm-yy"), Some(oct18));
    assert_eq!(parse_date("18 ottobre 2026", "dd mmmm yyyy"), Some(oct18));
    assert_eq!(parse_date("18-Oct-26", "dd-mmm-yy"), Some(oct18));
    assert!(keystroke(&dd_mmm_yy, "date", "18-ott-26").is_ok());
    // util.printd writes Italian names; util.scand reads them.
    assert_eq!(printd("dd-mmm-yy", "18-ott-26"), "18-ott-26|18-ott-26");
    assert_eq!(printd("dd-mmm-yy", "18-Oct-26"), "18-ott-26|18-ott-26", "English typing reads too");
    assert_eq!(printd("dddd d mmmm yyyy", "domenica 18 ottobre 2026"), "domenica 18 ottobre 2026|domenica 18 ottobre 2026");
    // German short names end in a dot; they are written and read whole.
    use_form_date_language("de");
    assert_eq!(format_value(&dd_mmm_yy, "18-10-26"), "18-Okt.-26");
    assert_eq!(parse_date("18-Okt.-26", "dd-mmm-yy"), Some(oct18));
    // An unknown language is English again.
    use_form_date_language("xx");
    assert_eq!(format_value(&dd_mmm_yy, "18-10-26"), "18-Oct-26");
    assert_eq!(printd("dd-mmm-yy", "18-Oct-26"), "18-Oct-26|18-Oct-26");
    every_date_language_reads_back_what_it_writes();
    // Hostile typing never panics and stays quick: long, odd and mixed-script text.
    use_form_date_language("th");
    let junk = ["", " ", "ม.", "ม.ค.ม.ค.ม.ค.", "\u{0}", "1月1月1月", "tháng tháng", "Ottobre", "🙂ott🙂", "İstanbul ott 26"];
    for t in junk.iter().map(|s| s.to_string()).chain(["ม.ค.1".repeat(5000), "9".repeat(100_000)]) {
        let started = std::time::Instant::now();
        let _ = parse_date(&t, "dd-mmm-yy");
        let _ = keystroke(&dd_mmm_yy, "date", &t);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{} chars took {:?}", t.len(), started.elapsed());
    }
    use_form_date_language("en");
}

/// An English weekday typed before the month fills the format's weekday, so a month name that
/// the date language shares with a weekday (Italian, Spanish, Romanian and Albanian "mar") is
/// still read as the month: "Tue 17 Mar 2026" read as "Tue 17 Tuesday 2026" (#5303).
#[test]
fn an_english_weekday_before_a_shared_month_name_reads_as_english() {
    let _names = names_lock();
    let doc = DocInfo { file_name: "form.pdf".into(), num_pages: 1, page: 0, info: Vec::new(), xfa: false };
    let mar17 = DateTime { y: 2026, m: 3, d: 17, hh: 0, mm: 0, ss: 0 };
    let mut bad = Vec::new();
    for language in ["it", "es", "ro", "sq"] {
        use_form_date_language(language);
        let ddd_d_mmm = "ddd d mmm yyyy";
        let typed = [
            // What the language writes, then English, then English and localized mixed.
            (ddd_d_mmm, format_date(mar17, ddd_d_mmm)),
            (ddd_d_mmm, "Tue 17 Mar 2026".into()),
            (ddd_d_mmm, "Tuesday 17 Mar 2026".into()),
            (ddd_d_mmm, "tue 17 mar 2026".into()),
            (ddd_d_mmm, format!("Tue 17 {}", format_date(mar17, "mmm yyyy"))),
            (ddd_d_mmm, format!("{} 17 Mar 2026", format_date(mar17, "ddd"))),
            ("mmm d yyyy ddd", "Mar 17 2026 Tue".into()),
            ("dddd d mmmm yyyy", "Tuesday 17 March 2026".into()),
        ];
        for (fmt, text) in typed {
            if parse_date(&text, fmt) != Some(mar17) {
                bad.push(format!("{language} forms {fmt}: {text:?} -> {:?}", parse_date(&text, fmt)));
            }
            let script = format!("var d = util.scand('{fmt}', {text:?}); event.value = d ? util.printd('yyyy-mm-dd', d) : 'null';");
            let o = run(&script, &Event::field("Format", "date", ""), &doc, &[], &[], Limits::default());
            if o.value != "2026-03-17" {
                bad.push(format!("{language} scand {fmt}: {text:?} -> {:?} {:?}", o.value, o.error));
            }
        }
    }
    use_form_date_language("en");
    assert!(bad.is_empty(), "{}", bad.join("; "));
}

/// Every date language: what a date field writes reads back as the same date, for each date
/// letter that writes a name, through the form path and through `util.scand`. Called from the
/// one test, which owns the process-wide names.
fn every_date_language_reads_back_what_it_writes() {
    let doc = DocInfo { file_name: "form.pdf".into(), num_pages: 1, page: 0, info: Vec::new(), xfa: false };
    let mut bad = Vec::new();
    for l in pdfcraft_engine::dates::DATE_LANGUAGES.iter() {
        use_form_date_language(l.code);
        for m in 1..=12u32 {
            let dt = DateTime { y: 2026, m, d: 18, hh: 0, mm: 0, ss: 0 };
            for fmt in ["dd-mmm-yy", "d mmmm yyyy", "dddd, mmmm d, yyyy", "ddd d mmm yyyy", "mmm d, yyyy ddd", "yyyymmmdd"] {
                let text = format_date(dt, fmt);
                // A short month ending in a number ("Thg 1") glued to the day is ambiguous
                // ("Thg 118": January 18 or November 8): nothing can read it back.
                if fmt == "yyyymmmdd" && l.months_short.iter().any(|n| n.ends_with(|c: char| c.is_ascii_digit())) {
                    continue;
                }
                if parse_date(&text, fmt).map(|p| (p.y, p.m, p.d)) != Some((2026, m, 18)) {
                    bad.push(format!("{} forms {fmt}: {text:?}", l.code));
                }
                let script = format!("var d = util.scand('{fmt}', {text:?}); event.value = d ? util.printd('yyyy-mm-dd', d) : 'null';");
                let o = run(&script, &Event::field("Format", "date", ""), &doc, &[], &[], Limits::default());
                if o.value != format!("2026-{m:02}-18") {
                    bad.push(format!("{} scand {fmt}: {text:?} -> {:?} {:?}", l.code, o.value, o.error));
                }
            }
        }
    }
    use_form_date_language("en");
    assert!(bad.is_empty(), "{} failures:\n{}", bad.len(), bad.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

/// A form whose text fields are each `(name, font, format script)`: `/Helv` is Helvetica with
/// WinAnsi, `/Song` a Chinese font with a Unicode CMap, which also holds Cyrillic.
fn form(fields: &[(&str, &str, &str)]) -> Document {
    let n = fields.len();
    let first = 6;
    let refs = |from: usize| (from..from + n).map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" ");
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", refs(first)),
        format!("<< /Fields [{}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R /Song {} 0 R >> >> >>", refs(first), first + n),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    for (i, (name, font, js)) in fields.iter().enumerate() {
        let js = js.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
        let y = 700 - 30 * i;
        objs.push(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /DA (/{font} 10 Tf 0 g) /Rect [50 {y} 300 {}] /P 3 0 R /AA << /F << /S /JavaScript /JS ({js}) >> >> >>",
            y + 20
        ));
    }
    objs.push("<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> >>] >>".into());
    let mut out = b"%PDF-1.7
"
    .to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(
            format!(
                "{} 0 obj
{o}
endobj
",
                i + 1
            )
            .as_bytes(),
        );
    }
    let xref = out.len();
    out.extend_from_slice(
        format!(
            "xref
0 {}
0000000000 65535 f 
",
            objs.len() + 1
        )
        .as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(
            format!(
                "{o:010} 00000 n 
"
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(
        format!(
            "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
",
            objs.len() + 1
        )
        .as_bytes(),
    );
    Document::open(Arc::new(out)).expect("opens")
}

/// The content of field `name`'s normal appearance.
fn appearance(doc: &Document, name: &str) -> Vec<u8> {
    let all = pdfcraft_forms::fields(doc);
    let f = all.iter().find(|f| f.name == name).expect("field");
    let widget = doc.get(f.widgets[0].obj);
    let normal = widget.as_dict().and_then(|d| d.get(b"AP")).and_then(|a| doc.resolve(a).as_dict().and_then(|a| a.reference(b"N"))).expect("/AP /N");
    match &*doc.get(normal) {
        Object::Stream(s) => s.decoded().expect("decodes"),
        other => panic!("not a stream: {other:?}"),
    }
}

/// What a Unicode-CMap (UCS-2) font is given for `text`.
fn ucs2(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Fills `form`'s field `name` with 18 October 2026 through the JavaScript runner.
fn fill(doc: &mut Document, name: &str) {
    fill_with(doc, name, "18-10-26");
}

/// Fills `form`'s field `name` with `value` through the JavaScript runner.
fn fill_with(doc: &mut Document, name: &str, value: &str) {
    let mut js = JsRunner::new(doc, "form.pdf");
    pdfcraft_forms::set_value_with(doc, name, &FieldValue::Text(value.into()), &mut js).expect("fills");
}

/// Month names in the date language that no font of the field can draw are drawn in English,
/// never as `?` ("18-????.-26" in Russian); a font that holds them keeps them (#5303).
#[test]
fn date_names_no_field_font_can_draw_are_drawn_in_english() {
    let _names = names_lock();
    let date = "AFDate_FormatEx(\"dd-mmm-yy\");";
    let fields = [("helv", "Helv", date), ("song", "Song", date)];
    for (language, local) in [("ru", "окт."), ("cs", "říj"), ("ja", "10月"), ("el", "Οκτ")] {
        use_form_date_language(language);
        let mut doc = form(&fields);
        fill(&mut doc, "helv");
        fill(&mut doc, "song");
        let helv = appearance(&doc, "helv");
        assert!(contains(&helv, b"(18-Oct-26) Tj"), "{language}: {}", String::from_utf8_lossy(&helv));
        assert!(!helv.contains(&b'?'), "{language}: {}", String::from_utf8_lossy(&helv));
        assert!(pdfcraft_forms::undrawn_fields(&doc).is_empty(), "{language}");
        let song = appearance(&doc, "song");
        assert!(contains(&song, &ucs2(&format!("18-{local}-26"))), "{language} keeps its name: {song:?}");
    }
    // Names Helvetica holds are kept: Italian "ott".
    use_form_date_language("it");
    let mut doc = form(&fields);
    fill(&mut doc, "helv");
    assert!(contains(&appearance(&doc, "helv"), b"(18-ott-26) Tj"));
    use_form_date_language("en");
}

/// A Format script's `util.printd` is run again with English names for a field whose fonts
/// can't draw the date language's, and kept where they can (#5303).
#[test]
fn format_scripts_print_dates_in_english_where_the_field_cannot_draw_them() {
    let _names = names_lock();
    let script = "event.value = event.value ? util.printd(\"dd-mmm-yy\", util.scand(\"dd-mm-yy\", event.value)) + \" !\" : \"\";";
    let fields = [("helv", "Helv", script), ("song", "Song", script)];
    use_form_date_language("ru");
    let mut doc = form(&fields);
    fill(&mut doc, "helv");
    fill(&mut doc, "song");
    let helv = appearance(&doc, "helv");
    assert!(contains(&helv, b"(18-Oct-26 !) Tj"), "{}", String::from_utf8_lossy(&helv));
    assert!(!helv.contains(&b'?'), "{}", String::from_utf8_lossy(&helv));
    assert!(pdfcraft_forms::undrawn_fields(&doc).is_empty());
    assert!(contains(&appearance(&doc, "song"), &ucs2("18-окт.-26 !")));
    // A script whose text has no English either keeps it, reported as undrawn.
    let greeting = [("hi", "Helv", "event.value = \"Привет \" + util.printd(\"mmm\", new Date(2026, 9, 18));")];
    let mut doc = form(&greeting);
    fill(&mut doc, "hi");
    assert_eq!(pdfcraft_forms::undrawn_fields(&doc), [("hi".to_string(), 'П')]);
    // The English run is the appearance's alone: scripts go on printing the date language.
    assert_eq!(printd("dd-mmm-yy", "18-окт.-26"), "18-окт.-26|18-окт.-26");
    use_form_date_language("en");
}

/// A Format script's English run is kept when it ran, even where it prints the stored value
/// again: a Helvetica field storing "18-Oct-26", printed again as `dd-mmm-yy`, showed
/// "18-????.-26" in Russian. A script that fails keeps what it showed, reported undrawn (#5303).
#[test]
fn an_english_run_that_prints_the_stored_value_is_drawn() {
    let _names = names_lock();
    let script = "event.value = util.printd(\"dd-mmm-yy\", util.scand(\"dd-mmm-yy\", event.value));";
    use_form_date_language("ru");
    let mut doc = form(&[("helv", "Helv", script), ("fails", "Helv", "throw new Error(\"no\");")]);
    fill_with(&mut doc, "helv", "18-Oct-26");
    let helv = appearance(&doc, "helv");
    assert!(contains(&helv, b"(18-Oct-26) Tj"), "{}", String::from_utf8_lossy(&helv));
    assert!(!helv.contains(&b'?'), "{}", String::from_utf8_lossy(&helv));
    assert!(pdfcraft_forms::undrawn_fields(&doc).is_empty());
    fill_with(&mut doc, "fails", "18-окт.-26");
    assert_eq!(pdfcraft_forms::undrawn_fields(&doc), [("fails".to_string(), 'о')]);
    use_form_date_language("en");
}

/// What `util.scand(fmt, text)` reads, as `yyyy-mm-dd`, or `null`.
fn scanned(fmt: &str, text: &str) -> String {
    let doc = DocInfo { file_name: "form.pdf".into(), num_pages: 1, page: 0, info: Vec::new(), xfa: false };
    let script = format!("var d = util.scand('{fmt}', {text:?}); event.value = d ? util.printd('yyyy-mm-dd', d) : 'null';");
    let o = run(&script, &Event::field("Format", "date", ""), &doc, &[], &[], Limits::default());
    assert_eq!(o.error, None);
    o.value
}

/// A short weekday typed without its dot ("mar" for French "mar.", Tuesday) is the weekday where
/// the format has one, not the English "Mar" (March) that is the only other name of its length:
/// "mar 6 janv 2026" read as no date in French and Romanian (#5303).
#[test]
fn a_short_weekday_typed_without_its_dot_is_the_weekday() {
    let _names = names_lock();
    let mut bad = Vec::new();
    let cases = [
        ("fr", "ddd d mmm yyyy", "Mar 6 janv 2026", "2026-01-06"),
        ("fr", "ddd d mmm yyyy", "mar 17 mars 2026", "2026-03-17"),
        ("fr", "mmm d yyyy ddd", "janv 6 2026 mar", "2026-01-06"),
        ("fr", "ddd dd/mm/yyyy", "mar 06/01/2026", "2026-01-06"),
        ("ro", "ddd d mmm yyyy", "mar 6 ian 2026", "2026-01-06"),
        ("ro", "ddd dd/mm/yyyy", "mar 06/01/2026", "2026-01-06"),
    ];
    for (language, fmt, text, want) in cases {
        use_form_date_language(language);
        let got = scanned(fmt, text);
        if got != want {
            bad.push(format!("{language} scand {fmt}: {text:?} -> {got}"));
        }
        let read = parse_date(text, fmt).map(|d| format!("{:04}-{:02}-{:02}", d.y, d.m, d.d));
        if read.as_deref() != Some(want) {
            bad.push(format!("{language} forms {fmt}: {text:?} -> {read:?}"));
        }
    }
    use_form_date_language("en");
    assert!(bad.is_empty(), "{}", bad.join("; "));
}

/// A weekday left out of a format with one ("17 mar 2026" for "ddd d mmm yyyy") lets a name the
/// date language shares between a weekday and a month be the month: Italian, Spanish and
/// Albanian "mar" (Tuesday, March) and Albanian "sht" (Saturday, September) were read as the
/// weekday, leaving no month (#5303). Typed weekdays still go first.
#[test]
fn a_shared_name_is_the_month_when_the_weekday_is_left_out() {
    let _names = names_lock();
    let mut bad = Vec::new();
    let cases = [
        ("it", "ddd d mmm yyyy", "17 Mar 2026", (2026, 3, 17)),
        ("es", "ddd d mmm yyyy", "17 Mar 2026", (2026, 3, 17)),
        ("sq", "ddd d mmm yyyy", "17 Mar 2026", (2026, 3, 17)),
        ("it", "ddd d mmm yyyy", "17 mar 2026", (2026, 3, 17)),
        ("it", "dddd, mmmm d, yyyy", "mar 17, 2026", (2026, 3, 17)),
        ("it", "dddd, mmm d, yyyy", "mar 17 2026", (2026, 3, 17)),
        ("sq", "ddd d mmm yyyy", "5 sht 2026", (2026, 9, 5)),
        ("it", "ddd d mmm yyyy", "mar 17 Mar 2026", (2026, 3, 17)),
        ("it", "ddd d mmm yyyy", "mar 17 mar 2026", (2026, 3, 17)),
    ];
    for (language, fmt, text, want) in cases {
        use_form_date_language(language);
        let read = parse_date(text, fmt).map(|d| (d.y, d.m, d.d));
        if read != Some(want) {
            bad.push(format!("{language} forms {fmt}: {text:?} -> {read:?}"));
        }
        if keystroke(&Format::Date(fmt.into()), "date", text).is_err() {
            bad.push(format!("{language} keystroke {fmt}: {text:?} refused"));
        }
    }
    use_form_date_language("en");
    assert!(bad.is_empty(), "{}", bad.join("; "));
}

/// Turkish names typed in capitals are read: lower-casing letter by letter made "MAYIS" "mayis"
/// (not "mayıs"), "SALI" "sali" and "EKİM" "eki̇m" (an i with a dot above), so no name matched.
/// Dotted, dotless and capital i are one letter when names are read (#5303).
#[test]
fn turkish_names_typed_in_capitals_are_read() {
    let _names = names_lock();
    use_form_date_language("tr");
    let mut bad = Vec::new();
    for (fmt, text, want) in [
        ("d mmmm yyyy", "1 MAYIS 2026", (2026, 5, 1)),
        ("d mmmm yyyy", "1 EKİM 2026", (2026, 10, 1)),
        ("d mmmm yyyy", "1 Ekim 2026", (2026, 10, 1)),
        ("d mmm yyyy", "1 EKİ 2026", (2026, 10, 1)),
        ("dddd d mmmm yyyy", "SALI 6 OCAK 2026", (2026, 1, 6)),
        ("dddd d mmmm yyyy", "Salı 6 Ocak 2026", (2026, 1, 6)),
    ] {
        let read = parse_date(text, fmt).map(|d| (d.y, d.m, d.d));
        if read != Some(want) {
            bad.push(format!("forms {fmt}: {text:?} -> {read:?}"));
        }
        let (y, m, d) = want;
        let got = scanned(fmt, text);
        if got != format!("{y:04}-{m:02}-{d:02}") {
            bad.push(format!("scand {fmt}: {text:?} -> {got}"));
        }
    }
    // English is read as before, capitals and all.
    use_form_date_language("en");
    for (fmt, text) in [("d mmmm yyyy", "1 APRIL 2026"), ("dddd d mmmm yyyy", "FRIDAY 1 MAY 2026"), ("d mmm yyyy", "1 MAY 2026")] {
        if parse_date(text, fmt).is_none() || scanned(fmt, text) == "null" {
            bad.push(format!("en {fmt}: {text:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("; "));
}

/// #5464 review: Greek names typed in capitals end in a capital Σ, which is the final ς of the
/// name ("ΙΑΝΟΥΆΡΙΟΣ" for "Ιανουάριος"); they read as they did before names were folded.
#[test]
fn greek_names_typed_in_capitals_are_read() {
    let _names = names_lock();
    use_form_date_language("el");
    let mut bad = Vec::new();
    for (fmt, text, want) in [
        ("d mmmm yyyy", "1 ΙΑΝΟΥΆΡΙΟΣ 2026", (2026, 1, 1)),
        ("d mmmm yyyy", "1 Ιανουάριος 2026", (2026, 1, 1)),
        ("d mmmm yyyy", "1 ιανουάριοσ 2026", (2026, 1, 1)),
        ("mmmm d, yyyy", "ΜΆΙΟΣ 5, 2026", (2026, 5, 5)),
        ("dddd, mmmm d, yyyy", "FRIDAY, ΙΑΝΟΥΆΡΙΟΣ 2, 2026", (2026, 1, 2)),
    ] {
        let read = parse_date(text, fmt).map(|d| (d.y, d.m, d.d));
        if read != Some(want) {
            bad.push(format!("forms {fmt}: {text:?} -> {read:?}"));
        }
        let (y, m, d) = want;
        let got = scanned(fmt, text);
        if got != format!("{y:04}-{m:02}-{d:02}") {
            bad.push(format!("scand {fmt}: {text:?} -> {got}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("; "));
}
