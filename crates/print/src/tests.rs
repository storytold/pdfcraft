use std::sync::Arc;

use super::*;
use crate::spool::{Duplex, Job, Printer, lp_args, parse_lpstat, parse_windows_printers};

/// `n` pages of 200×300 (page 3 is landscape 300×200 when n ≥ 3); page i shows "(Page i+1)".
/// Page 1 carries a printable square comment, a non-printing note, and a stamp.
fn fixture(n: usize) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 6 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    // 4: an appearance stream; 5: unused.
    let ap = b"0 0 1 rg 0 0 10 10 re f";
    objs.push(
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} >>\nstream\n{}\nendstream", ap.len(), String::from_utf8_lossy(ap))
            .into_bytes(),
    );
    objs.push(b"null".to_vec());
    for i in 0..n {
        let annots = if i == 0 {
            " /Annots [<< /Type /Annot /Subtype /Square /F 4 /Rect [10 10 50 50] /AP << /N 4 0 R >> >> << /Type /Annot /Subtype /Text /F 0 /Rect [60 10 80 30] /AP << /N 4 0 R >> >> << /Type /Annot /Subtype /Stamp /F 4 /Rect [100 10 140 50] /AP << /N 4 0 R >> >>]"
        } else {
            ""
        };
        let mb = if i == 2 { " /MediaBox [0 0 300 200]" } else { "" };
        objs.push(
            format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >>{mb}{annots} >>", 7 + 2 * i).into_bytes(),
        );
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn settings(pages: Vec<usize>, layout: Layout) -> Settings {
    Settings { pages, paper: (612.0, 792.0), layout, ..Settings::default() }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn page_selection() {
    let labels: Vec<String> = ["i", "ii", "1", "2", "A-1"].iter().map(|s| s.to_string()).collect();
    assert_eq!(select_pages(5, None, &labels, Subset::All, false).unwrap(), [0, 1, 2, 3, 4]);
    assert_eq!(select_pages(5, Some("1-2, 5"), &[], Subset::All, false).unwrap(), [0, 1, 4]);
    assert_eq!(select_pages(5, Some("4-"), &[], Subset::All, false).unwrap(), [3, 4]);
    assert_eq!(select_pages(5, Some("-2"), &[], Subset::All, false).unwrap(), [0, 1]);
    assert_eq!(select_pages(5, Some("ii-2, A-1"), &labels, Subset::All, false).unwrap(), [1, 2, 3, 4], "labels, even with a dash");
    assert_eq!(select_pages(5, None, &[], Subset::Even, true).unwrap(), [3, 1]);
    assert_eq!(select_pages(5, None, &[], Subset::Odd, false).unwrap(), [0, 2, 4]);
    assert_eq!(select_pages(5, Some("7"), &[], Subset::All, false), Err(PrintError::OutOfRange(7, 5)));
    assert_eq!(select_pages(5, Some("x"), &[], Subset::All, false), Err(PrintError::NotAPage("x".into())));
    assert_eq!(select_pages(5, Some("7"), &[], Subset::All, false).unwrap_err().to_string(), "page 7 is out of range (1–5)");
    assert_eq!(select_pages(1, None, &[], Subset::Even, false), Err(PrintError::NoPages));
    // An explicit list (selected thumbnails): positions, never labels, in the order given.
    assert_eq!(select_listed(5, &[1, 3], Subset::All, false).unwrap(), [1, 3]);
    // Picked pages keep their page numbers: pages 1, 3 and 5 are all odd.
    assert_eq!(select_listed(5, &[0, 2, 4], Subset::Even, false), Err(PrintError::NoPages));
    assert_eq!(select_listed(5, &[0, 2, 4], Subset::Odd, true).unwrap(), [4, 2, 0]);
    assert_eq!(select_listed(5, &[1, 2, 3], Subset::Even, false).unwrap(), [1, 3]);
    assert_eq!(select_listed(5, &[1, 5], Subset::All, false), Err(PrintError::OutOfRange(6, 5)));
    assert!(matches!(select_listed(0, &[usize::MAX], Subset::All, false), Err(PrintError::OutOfRange(_, 0))));
    assert_eq!(select_listed(5, &[], Subset::All, false), Err(PrintError::NoPages));
}

/// Every mistake a person can make about what to print says itself out of a wording on the list
/// and the numbers apart from it, and says itself in English exactly as the error prints itself.
#[test]
fn wordings_are_said() {
    let mistakes = [PrintError::NoPages, PrintError::OutOfRange(7, 5), PrintError::NotAPage("x".into())];
    for e in &mistakes {
        assert_eq!(e.said(&in_english), e.to_string(), "{e:?} says itself in English the way it prints itself");
    }
    // Each wording is one of a mistake's, and each mistake's is on the list: the hole names and
    // all, so a catalog that translates the list translates every sentence a person can be shown.
    let said: Vec<String> = WORDINGS.iter().map(|w| w.to_string()).collect();
    assert_eq!(said.len(), mistakes.len(), "one wording per mistake");
    for w in WORDINGS {
        assert!(!w.is_empty() && w.ends_with(|c: char| c.is_alphanumeric() || c == ')'), "a whole sentence, not a fragment: {w:?}");
    }
    // A fault is not a mistake: it carries its own text and is not on the list.
    let fault = PrintError::Spool("lp: no such printer".into());
    assert_eq!(fault.said(&in_english), "lp: no such printer");
    assert_eq!(fault.said(&|_| "traducido".to_string()), "lp: no such printer", "nothing to translate");
    // Said in another language, the numbers still go in, and in that language's own order.
    let other = |w: &str| match w {
        "page {n} is out of range (1–{count})" => "el documento tiene {count} páginas, no {n}".to_string(),
        w => w.to_string(),
    };
    assert_eq!(PrintError::OutOfRange(7, 5).said(&other), "el documento tiene 5 páginas, no 7");
    // What was typed goes in as it stands, braces and all, and is never read as a hole.
    assert_eq!(PrintError::NotAPage("{count}".into()).said(&other), "\"{count}\" is not a page number or label");
}

#[test]
fn page_selection_handles_ranges_between_hyphenated_labels() {
    let labels: Vec<String> = ["A-1", "A-2", "A-3", "B-1", "B-2"].iter().map(|s| s.to_string()).collect();
    let pages = |range| select_pages(5, Some(range), &labels, Subset::All, false).unwrap();
    assert_eq!(pages("A-1"), [0], "the complete label takes precedence over a range");
    assert_eq!(pages("A-1-A-3"), [0, 1, 2]);
    assert_eq!(pages("A-3-A-1"), [2, 1, 0], "reverse logical ranges");
    assert_eq!(pages("A-2-"), [1, 2, 3, 4], "an open-ended range starting at a hyphenated label");
    assert_eq!(pages("-A-2"), [0, 1], "an open-ended range ending at a hyphenated label");
    assert_eq!(pages("A-2-5"), [1, 2, 3, 4], "a label followed by a numeric endpoint");
    assert_eq!(pages("B-1-B-2"), [3, 4]);
    // If an earlier dash yields one known label but an unknown endpoint, continue
    // checking later dashes instead of rejecting the whole range.
    let ambiguous: Vec<String> = ["A-1", "A-1-B", "C"].iter().map(|s| s.to_string()).collect();
    assert_eq!(select_pages(3, Some("A-1-B-C"), &ambiguous, Subset::All, false).unwrap(), [1, 2]);
}

#[test]
fn size_modes() {
    let sizes = [(200.0, 300.0), (1000.0, 1500.0)];
    let fit = layout(&sizes, &settings(vec![0, 1], Layout::Size(SizeMode::Fit))).unwrap();
    let s0 = fit[0].placed[0].matrix.0[0];
    assert!(close(s0, 2.52), "min(576 / 200, 756 / 300): {s0}");
    let actual = layout(&sizes, &settings(vec![0], Layout::Size(SizeMode::Actual))).unwrap();
    assert_eq!(actual[0].placed[0].matrix.0, [1.0, 0.0, 0.0, 1.0, 206.0, 246.0], "centred at 100%");
    let shrink = layout(&sizes, &settings(vec![0, 1], Layout::Size(SizeMode::Shrink))).unwrap();
    assert_eq!(shrink[0].placed[0].matrix.0[0], 1.0, "small pages stay at 100%");
    assert!(shrink[1].placed[0].matrix.0[0] < 1.0, "big pages shrink");
    let custom = layout(&sizes, &settings(vec![0], Layout::Size(SizeMode::Custom(50.0)))).unwrap();
    assert_eq!(custom[0].placed[0].matrix.0[0], 0.5);
    // Auto orientation turns the sheet for landscape pages.
    let land = layout(&[(300.0, 200.0)], &settings(vec![0], Layout::Size(SizeMode::Fit))).unwrap();
    assert_eq!(land[0].size, (792.0, 612.0));
}

#[test]
fn multiple_pages_per_sheet() {
    let sizes = vec![(200.0, 300.0); 5];
    let sheets = layout(&sizes, &settings((0..5).collect(), Layout::multiple(4))).unwrap();
    assert_eq!(sheet_pages(&sheets), [vec![0, 1, 2, 3], vec![4]]);
    // Horizontal order: 0 top-left, 1 top-right, 2 bottom-left.
    let o = |k: usize| (sheets[0].placed[k].matrix.0[4], sheets[0].placed[k].matrix.0[5]);
    assert!(o(1).0 > o(0).0 && close(o(1).1, o(0).1) && o(2).1 < o(0).1 && close(o(2).0, o(0).0));
    // Two per sheet prints side by side on landscape paper.
    let two = layout(&sizes, &settings(vec![0, 1], Layout::multiple(2))).unwrap();
    assert_eq!(two[0].size, (792.0, 612.0));
    assert!(two[0].placed[1].matrix.0[4] > two[0].placed[0].matrix.0[4]);
    // Vertical order and borders.
    let v = layout(
        &sizes,
        &settings(vec![0, 1, 2], Layout::Multiple { cols: 2, rows: 2, per_sheet: 4, order: PageOrder::Vertical, border: true, auto_rotate: false }),
    )
    .unwrap();
    assert!(close(v[0].placed[1].matrix.0[4], v[0].placed[0].matrix.0[4]) && v[0].placed[1].matrix.0[5] < v[0].placed[0].matrix.0[5]);
    assert_eq!(v[0].borders.len(), 3);
    // Auto-rotate turns a landscape page in a portrait cell.
    let r = layout(&[(300.0, 200.0), (200.0, 300.0)], &settings(vec![0, 1], Layout::multiple(4))).unwrap();
    // The first (landscape) page sets a landscape sheet; the portrait page is turned to fit.
    assert_eq!(r[0].size, (792.0, 612.0));
    assert!(r[0].placed[0].matrix.0[0] > 0.0);
    assert_eq!(r[0].placed[1].matrix.0[0], 0.0, "rotated");
}

#[test]
fn custom_pages_per_sheet_never_overfill_the_grid() {
    // Three per sheet draws on a 2 × 2 grid, five on 3 × 2: the spare cells stay blank (#1336).
    let sizes = vec![(200.0, 300.0); 7];
    let pages = |per: usize| sheet_pages(&layout(&sizes, &settings((0..7).collect(), Layout::multiple(per))).unwrap());
    assert_eq!(pages(3), [vec![0, 1, 2], vec![3, 4, 5], vec![6]]);
    assert_eq!(pages(5), [vec![0, 1, 2, 3, 4], vec![5, 6]]);
    assert_eq!(pages(7), [vec![0, 1, 2, 3, 4, 5, 6]]);
    assert_eq!(pages(4), [vec![0, 1, 2, 3], vec![4, 5, 6]]);
    assert_eq!(pages(2), [vec![0, 1], vec![2, 3], vec![4, 5], vec![6]]);
    // Cut-stack piles take the requested count too.
    let stacked = Layout::Multiple { cols: 2, rows: 2, per_sheet: 3, order: PageOrder::CutStack, border: false, auto_rotate: false };
    assert_eq!(sheet_pages(&layout(&sizes, &settings((0..7).collect(), stacked)).unwrap()), [vec![0, 3, 6], vec![1, 4], vec![2, 5]]);
    // A count larger than the grid is capped by the grid; no pages per sheet is refused.
    let grid = |per_sheet| Layout::Multiple { cols: 2, rows: 1, per_sheet, order: PageOrder::Horizontal, border: false, auto_rotate: false };
    assert_eq!(sheet_pages(&layout(&sizes[..3], &settings(vec![0, 1, 2], grid(9))).unwrap()), [vec![0, 1], vec![2]]);
    assert!(matches!(layout(&sizes, &settings(vec![0], grid(0))), Err(PrintError::Invalid(_))));
    // The imposed PDF has one page per sheet.
    let out = impose(&fixture(7), &settings((0..7).collect(), Layout::multiple(3))).unwrap();
    assert_eq!(pdfcraft_model::pages(&Document::open(Arc::new(out)).unwrap()).len(), 3);
}

#[test]
fn imposing_lists_the_pages_once() {
    let n = 150;
    let doc = fixture(n);
    PAGES_LISTED.with(|c| c.set(0));
    let out = impose(&doc, &settings((0..n).collect(), Layout::multiple(2))).unwrap();
    let listed = PAGES_LISTED.with(|c| c.get());
    assert!(listed <= 2 * n, "{listed} pages listed to print {n}");
    assert_eq!(pdfcraft_model::pages(&Document::open(Arc::new(out)).unwrap()).len(), n / 2);
}

#[test]
fn booklets_pair_pages_for_folding() {
    let sizes = vec![(200.0, 300.0); 6];
    let sheets = layout(&sizes, &settings((0..6).collect(), Layout::Booklet { subset: BookletSubset::BothSides, binding: Binding::Left })).unwrap();
    // 6 pages pad to 8: sheet 1 front [8,1] → [blank, 0], back [1, 6]; sheet 2 front [5, 2], back [3, 4].
    assert_eq!(sheet_pages(&sheets), [vec![0], vec![1], vec![5, 2], vec![3, 4]]);
    assert!(sheets[0].placed[0].matrix.0[4] >= 395.9, "page 1 on the right half");
    assert_eq!(sheets[0].size, (792.0, 612.0));
    let front = layout(&sizes, &settings((0..6).collect(), Layout::Booklet { subset: BookletSubset::FrontOnly, binding: Binding::Right })).unwrap();
    assert_eq!(sheet_pages(&front), [vec![0], vec![2, 5]], "right binding mirrors the spread");
}

#[test]
fn posters_tile_with_overlap() {
    // A 200×300 page at 400% = 800×1200 on Letter with 18 pt margins (576×756 tiles), overlap 36.
    let sheets = layout(&[(200.0, 300.0)], &settings(vec![0], Layout::Poster { scale: 400.0, overlap: 36.0, cut_marks: true })).unwrap();
    assert_eq!(sheets.len(), 4, "2 × 2 tiles");
    assert_eq!(sheets[0].lines.len(), 8, "cut marks");
    let c0 = sheets[0].placed[0].clip;
    let c1 = sheets[1].placed[0].clip;
    assert!(close(c0[2] - c1[0], 36.0 / 4.0), "neighbouring tiles share the overlap");
    assert!(layout(&[(200.0, 300.0)], &settings(vec![0], Layout::Poster { scale: 400.0, overlap: 400.0, cut_marks: false })).is_err());
}

/// Issue #1335: a scale so large that the tile counts saturate is refused with the sheet-limit
/// error, not an overflow in counting the sheets.
#[test]
fn huge_posters_are_refused_at_the_sheet_limit() {
    let refused = Err(PrintError::Invalid("the poster would need more than 1024 sheets".into()));
    for scale in [100_000.0, 1e22, f64::MAX] {
        let poster = settings(vec![0], Layout::Poster { scale, overlap: 18.0, cut_marks: true });
        assert_eq!(layout(&[(200.0, 300.0)], &poster), refused, "scale {scale}");
    }
}

#[test]
fn imposed_pdf_has_the_sheets_and_honours_comments_and_forms() {
    let doc = fixture(3);
    let out = impose(&doc, &settings(vec![0, 1, 2], Layout::multiple(2))).unwrap();
    let printed = Document::open(Arc::new(out.clone())).unwrap();
    let pages = pdfcraft_model::pages(&printed);
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].crop(&printed), [0.0, 0.0, 792.0, 612.0]);
    // The source pages are form XObjects holding their content.
    let streams = |d: &Document| -> Vec<String> {
        d.object_numbers()
            .into_iter()
            .filter_map(|n| match &*d.get(ObjRef::new(n, d.generation(n))) {
                Object::Stream(s) => s.decoded().ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
                _ => None,
            })
            .collect()
    };
    let all = streams(&printed).join("\n");
    assert!(all.contains("(Page 1)") && all.contains("(Page 3)"));
    // Markups: the printable square and the stamp, not the note without the Print flag.
    let page1 = streams(&printed).into_iter().find(|s| s.contains("(Page 1)")).unwrap();
    assert_eq!(page1.matches(" Do Q").count(), 2, "{page1}");
    let doc_only = impose(&doc, &Settings { content: Content::Document, ..settings(vec![0], Layout::Size(SizeMode::Fit)) }).unwrap();
    let p = streams(&Document::open(Arc::new(doc_only)).unwrap()).into_iter().find(|s| s.contains("(Page 1)")).unwrap();
    assert_eq!(p.matches(" Do Q").count(), 0);
    let stamps = impose(&doc, &Settings { content: Content::DocumentAndStamps, ..settings(vec![0], Layout::Size(SizeMode::Fit)) }).unwrap();
    let p = streams(&Document::open(Arc::new(stamps)).unwrap()).into_iter().find(|s| s.contains("(Page 1)")).unwrap();
    assert_eq!(p.matches(" Do Q").count(), 1);
    let fields = impose(&doc, &Settings { content: Content::FormFieldsOnly, ..settings(vec![1], Layout::Size(SizeMode::Fit)) }).unwrap();
    assert!(!streams(&Document::open(Arc::new(fields)).unwrap()).join("").contains("(Page 2)"));
    // The output is a fresh file: the unused source pages are gone.
    let one = impose(&doc, &settings(vec![1], Layout::Size(SizeMode::Fit))).unwrap();
    let s = streams(&Document::open(Arc::new(one)).unwrap()).join("\n");
    assert!(s.contains("(Page 2)") && !s.contains("(Page 1)"));
}

#[test]
fn spooler_arguments_and_printer_list() {
    let out = "printer Office_Laser is idle.  enabled since Thu Oct  1 09:00:00 2026\nprinter Label_Writer disabled since …\nsystem default destination: Office_Laser\n";
    assert_eq!(
        parse_lpstat(out),
        [
            spool::Printer { name: "Office_Laser".into(), default: true, two_sided: None },
            spool::Printer { name: "Label_Writer".into(), default: false, two_sided: None }
        ]
    );
    assert!(parse_lpstat("lpstat: No destinations added.\nno system default destination\n").is_empty());
    let job = Job {
        printer: Some("Office_Laser".into()),
        copies: 3,
        collate: false,
        duplex: Some(Duplex::LongEdge),
        grayscale: true,
        title: "memo.pdf".into(),
        options: Vec::new(),
        ..Job::default()
    };
    assert_eq!(
        lp_args(&job).join(" "),
        "-d Office_Laser -n 3 -t memo.pdf -o collate=false -o sides=two-sided-long-edge -o print-color-mode=monochrome -o fit-to-page=false"
    );
    assert_eq!(lp_args(&Job::default())[0], "-n", "no -d: the default printer");
}

#[test]
fn lpstat_output_is_untranslated() {
    // A localized lpstat (here Polish) is unreadable to parse_lpstat...
    assert!(parse_lpstat("drukarka Office_Laser jest bezczynna.\ndomyślny cel systemowy: Office_Laser\n").is_empty());
    // ...so the command must force the C locale, including the SOFTWARE switch macOS CUPS needs.
    let cmd = spool::lpstat_command();
    let envs: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
    for key in ["LC_ALL", "LANG"] {
        assert!(envs.contains(&(key.into(), Some("C".into()))), "{key}=C missing: {envs:?}");
    }
    assert!(envs.iter().any(|(k, v)| k == "SOFTWARE" && v.as_deref().is_some_and(|v| !v.is_empty())), "SOFTWARE missing: {envs:?}");
}

#[test]
fn driverless_printers_join_the_list_without_a_queue() {
    // A driverless network printer (IPP, discovered on the LAN) has no queue, so `lpstat -p`
    // never lists it — yet GTK's print dialog shows it and `lp` prints to it through a temporary
    // queue. Only `lpstat -e` names it, and the Print dialog used to show Save as PDF alone.
    let queues = "printer Office_Laser is idle.  enabled since Thu Oct  1 09:00:00 2026\nsystem default destination: Office_Laser\n";
    assert_eq!(
        spool::printers_parsed(queues, Some("Basement_Color\nOffice_Laser\n")),
        [
            spool::Printer { name: "Office_Laser".into(), default: true, two_sided: None },
            spool::Printer { name: "Basement_Color".into(), default: false, two_sided: None },
        ],
        "queues first, then the driverless destination; the queue listed twice is not doubled"
    );
    // An old spooler without `-e` (CUPS < 1.7) keeps its queues; only the driverless names are lost.
    assert_eq!(spool::printers_parsed(queues, None), [spool::Printer { name: "Office_Laser".into(), default: true, two_sided: None }]);
    // The default can itself be driverless (`lpoptions -d` names a destination no queue exists
    // for): it keeps its marker, so the dialog preselects the user's default, not the first queue.
    assert_eq!(
        spool::printers_parsed("system default destination: Basement_Color\n", Some("Basement_Color\n")),
        [spool::Printer { name: "Basement_Color".into(), default: true, two_sided: None }],
        "a default destination without a queue is still the default"
    );
    assert!(spool::parse_lpstat_e("\n \n").is_empty(), "no destinations is empty, not [\"\"]");
    assert_eq!(spool::parse_lpstat_e("A\r\nB \n"), ["A".to_string(), "B".to_string()], "CRLF and stray spaces are trimmed");
}

fn cut_stack(cols: usize, rows: usize) -> Layout {
    Layout::Multiple { cols, rows, per_sheet: cols.saturating_mul(rows), order: PageOrder::CutStack, border: false, auto_rotate: false }
}

#[test]
fn cut_stack_keeps_piles_in_order_and_blanks_in_place() {
    let sizes = vec![(200.0, 300.0); 10];
    let sheets = layout(&sizes, &settings((0..10).collect(), cut_stack(2, 2))).unwrap();
    assert_eq!(sheet_pages(&sheets), [vec![0, 3, 6, 9], vec![1, 4, 7], vec![2, 5, 8]]);
    for s in &sheets {
        assert_eq!(s.lines, sheets[0].lines, "identical cuts even with blank cells");
        assert_eq!(s.lines.len(), 4);
        for (pl, first) in s.placed.iter().zip(&sheets[0].placed) {
            assert_eq!(pl.matrix, first.matrix, "every pile stays in its cell");
        }
    }
    assert!(sheets[0].placed[1].matrix.0[4] > sheets[0].placed[0].matrix.0[4]);
    assert!(sheets[0].placed[2].matrix.0[5] < sheets[0].placed[0].matrix.0[5]);
    // Selection and reverse order are preserved, including repeated source pages.
    let selected = vec![9, 5, 5, 2, 0];
    let sheets = layout(&sizes, &settings(selected, cut_stack(2, 2))).unwrap();
    assert_eq!(sheet_pages(&sheets), [vec![9, 5, 0], vec![5, 2]]);
    assert_eq!(sheets[1].placed[1].matrix, sheets[0].placed[1].matrix);
}

#[test]
fn cutting_and_stacking_recovers_every_selected_page() {
    // Simulate the actual operation using cell positions, not the imposition formula.
    for (cols, rows) in [(1, 1), (1, 2), (2, 2), (2, 3), (3, 3), (4, 4)] {
        for orientation in [Orientation::Auto, Orientation::Portrait, Orientation::Landscape] {
            for n in 1..=37 {
                let sizes = vec![(200.0, 300.0); n];
                let selected: Vec<usize> = (0..n).rev().collect();
                let s = Settings { orientation, ..settings(selected.clone(), cut_stack(cols, rows)) };
                let sheets = layout(&sizes, &s).unwrap();
                let mut piles: std::collections::BTreeMap<(i64, i64), Vec<usize>> = Default::default();
                for sheet in &sheets {
                    for pl in &sheet.placed {
                        // Equal source sizes, so the page origins identify row/column.
                        let [_, _, _, _, x, y] = pl.matrix.0;
                        piles.entry((-(y * 100.0).round() as i64, (x * 100.0).round() as i64)).or_default().push(pl.page);
                    }
                }
                let restacked: Vec<usize> = piles.into_values().flatten().collect();
                assert_eq!(restacked, selected, "{cols}x{rows}, {n} pages, {orientation:?}");
            }
        }
    }
}

#[test]
fn multiple_rejects_hostile_grids_without_panicking() {
    let sizes = [(200.0, 300.0)];
    for (cols, rows) in [(usize::MAX, 2), (2, usize::MAX), (0, 2), (1, 0), (257, 1)] {
        let mode = Layout::Multiple { cols, rows, per_sheet: 1, order: PageOrder::Horizontal, border: false, auto_rotate: false };
        assert!(matches!(layout(&sizes, &settings(vec![0], mode)), Err(PrintError::Invalid(_))));
    }
    let tiny = Settings { paper: (72.0, 72.0), ..settings(vec![0], cut_stack(16, 16)) };
    assert!(matches!(layout(&sizes, &tiny), Err(PrintError::Invalid(_))));
    for size in [(0.0, 300.0), (200.0, f64::NAN), (f64::INFINITY, 300.0), (f64::from_bits(1), f64::from_bits(1))] {
        assert!(matches!(layout(&[size], &settings(vec![0], cut_stack(2, 2))), Err(PrintError::Invalid(_))));
    }
}

#[test]
fn windows_printers_carry_one_default_and_keep_their_names() {
    // Windows marks the system default with a `*` no queue name may contain; names hold spaces,
    // parentheses and non-ASCII letters, and the lines end with CRLF.
    let printers = parse_windows_printers("*Microsoft Print to PDF\r\nHP LaserJet 400 (Copy 2)\r\nКопир Canon iR\r\n\r\n*XPS via USB\r\n");
    assert_eq!(printers[0], Printer { name: "Microsoft Print to PDF".into(), default: true, two_sided: None });
    assert_eq!(printers[1], Printer { name: "HP LaserJet 400 (Copy 2)".into(), default: false, two_sided: None });
    assert_eq!(printers[2], Printer { name: "Копир Canon iR".into(), default: false, two_sided: None });
    // A spooler marking two defaults: the first still wins, so the dialog has one to preselect.
    assert_eq!(printers[3], Printer { name: "XPS via USB".into(), default: false, two_sided: None });
    assert_eq!(parse_windows_printers("\r\n"), Vec::new());
    // After a tab, whether the driver reports two-sided printing; a tab inside a name stays.
    let sided = parse_windows_printers("*Office PCL\t2\r\nOffice PS\t1\r\nTab\tName\t2\r\n");
    assert_eq!(sided[0], Printer { name: "Office PCL".into(), default: true, two_sided: Some(true) });
    assert_eq!(sided[1], Printer { name: "Office PS".into(), default: false, two_sided: Some(false) });
    assert_eq!(sided[2], Printer { name: "Tab\tName".into(), default: false, two_sided: Some(true) });
}

/// A PPD shaped like a Fiery's: installable options, multi-line PostScript in the choices, Latin-1
/// labels, options the Print dialog sets itself.
const FIERY_PPD: &[u8] = b"*PPD-Adobe: \"4.3\"\n\
*LanguageEncoding: ISOLatin1\n\
*OpenGroup: InstallableOptions/Installable Options\n\
*OpenUI *EFFinisher/Finisher option: PickOne\n\
*DefaultEFFinisher: False\n\
*EFFinisher False/Not installed: \"\"\n\
*EFFinisher SingleStapler/Single stapler: \"\"\n\
*CloseUI: *EFFinisher\n\
*CloseGroup: InstallableOptions\n\
*OpenGroup: FPPaperSource/Media\n\
*OpenUI *InputSlot/Paper tray: PickOne\n\
*OrderDependency: 20.0 AnySetup *InputSlot\n\
*DefaultInputSlot: AutoSelect\n\
*InputSlot AutoSelect/Auto tray select: \"\n\
userdict /XJXEFIsetpageproperties known\n\
{ << /XJXsettrayselV2 [ 7 ] >> XJXEFIsetpageproperties } if\"\n\
*End\n\
*InputSlot ManualFeed/Bypass tray: \"\n\
{ pop 2 XJXsettrayselV2 } if\"\n\
*End\n\
*InputSlot Tray2/Tray 2: \"\"\n\
*fr.InputSlot Tray2/Bac 2: \"\"\n\
*CloseUI: *InputSlot\n\
*OpenUI *EFMediaType/Paper type: PickOne\n\
*DefaultEFMediaType: Plain\n\
*EFMediaType Plain/Plain: \"\"\n\
*EFMediaType Heavy1/Thick 1 (106\xad163 g/m\xb2): \"\"\n\
*CloseUI: *EFMediaType\n\
*CloseGroup: FPPaperSource\n\
*OpenUI *PageSize/Page size: PickOne\n\
*DefaultPageSize: A4\n\
*PageSize A4/A4: \"\"\n\
*CloseUI: *PageSize\n\
*OpenUI *EFRaster/Print queue action: PickOne\n\
*DefaultEFRaster: Bogus\n\
*EFRaster False/Print: \"\"\n\
*EFRaster Hold: \"\"\n\
*CloseUI: *EFRaster\n";

#[test]
fn printer_options_come_from_the_ppd() {
    let options = spool::parse_ppd(&spool::ppd_text(FIERY_PPD));
    let keys: Vec<&str> = options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["InputSlot", "EFMediaType", "EFRaster"], "no installable options, no PageSize");
    let tray = &options[0];
    assert_eq!((tray.label.as_str(), tray.group.as_str(), tray.default.as_str()), ("Paper tray", "Media", "AutoSelect"));
    assert_eq!(
        tray.choices,
        [("AutoSelect".into(), "Auto tray select".into()), ("ManualFeed".into(), "Bypass tray".into()), ("Tray2".into(), "Tray 2".into())],
        "PostScript lines and translations (*fr.InputSlot) are not choices"
    );
    assert_eq!(options[1].choices[1].1, "Thick 1 (106\u{ad}163 g/m²)", "Latin-1 labels");
    // A default that isn't one of the choices falls back to the first; a choice without a label shows its keyword.
    assert_eq!((options[2].default.as_str(), options[2].choices[1].1.as_str()), ("False", "Hold"));
    assert!(spool::parse_ppd("*OpenUI *Broken\n*Broken A/B\n").is_empty(), "unclosed or malformed blocks are skipped");
}

#[test]
fn printer_option_defaults_and_job_arguments() {
    let current =
        spool::parse_lpoptions("InputSlot/Paper tray: AutoSelect *Tray2 ManualFeed\nEFRaster/Print queue action: *False Hold\nbroken line\n");
    assert_eq!(current, [("InputSlot".to_string(), "Tray2".to_string()), ("EFRaster".to_string(), "False".to_string())]);
    let job = Job {
        options: vec![
            ("InputSlot".into(), "Tray2".into()),
            ("EFMediaType".into(), "Heavy1".into()),
            ("Duplex".into(), "DuplexTumble".into()),
            ("Bad Key".into(), "x".into()),
            ("EFRaster".into(), "a=b".into()),
        ],
        ..Job::default()
    };
    let args = lp_args(&job).join(" ");
    assert!(args.ends_with("-o fit-to-page=false -o InputSlot=Tray2 -o EFMediaType=Heavy1"), "{args}");
    assert!(spool::printer_options("../../etc/passwd").is_empty(), "not a queue name");
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod cups_options {
    use crate::spool;

    const RECORD: &str = "PDFCRAFT_LPOPTIONS_TEST_RECORD";
    const ANSWER: &str = "PDFCRAFT_LPOPTIONS_TEST_ANSWER";
    /// Generous: these tests are about what is read, not how long it takes.
    const LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

    /// The child process stands in for lpoptions, recording its real arguments.
    #[test]
    fn stand_in_for_lpoptions() {
        let Some(record) = std::env::var_os(RECORD) else { return };
        let args: Vec<String> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
        std::fs::write(std::path::Path::new(&record).join("args"), args.join("\n")).unwrap();
        let answer = std::env::var(ANSWER).unwrap();
        // A queue that never answers (CUPS stopped responding).
        if answer == "HANG" {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        // Serial libtest prints its test label without a newline before this helper's stdout.
        println!("\n{answer}");
    }

    /// `lpoptions` that never answers is stopped at the deadline: the PPD's defaults stand and
    /// whoever asked is not held (#1818).
    #[test]
    fn a_queue_that_never_answers_is_given_up_at_the_deadline() {
        let dir = folder();
        std::fs::write(
            dir.join("Office.ppd"),
            "*OpenUI *InputSlot/Paper tray: PickOne\n*DefaultInputSlot: Auto\n*InputSlot Auto/Automatic: \"\"\n*InputSlot Tray2/Tray 2: \"\"\n*CloseUI: *InputSlot\n",
        )
        .unwrap();
        let started = std::time::Instant::now();
        let options = spool::printer_options_via_within("Office", &dir, command(&dir, "HANG"), std::time::Duration::from_millis(500), "en");
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "given up at the deadline, not when the command does");
        assert_eq!(options.len(), 1, "the PPD is still read");
        assert_eq!(options[0].default, "Auto", "the PPD default stands when the queue does not answer");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn command(record: &std::path::Path, answer: &str) -> std::process::Command {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "tests::cups_options::stand_in_for_lpoptions", "--nocapture", "--"]).env(RECORD, record).env(ANSWER, answer);
        c
    }

    fn folder() -> std::path::PathBuf {
        use std::io::Read as _;
        use std::os::unix::fs::DirBuilderExt as _;
        let mut random = std::fs::File::open("/dev/urandom").unwrap();
        for _ in 0..32 {
            let mut nonce = [0; 16];
            random.read_exact(&mut nonce).unwrap();
            let path = std::env::temp_dir().join(format!("pdfcraft-cups-options-{:032x}", u128::from_be_bytes(nonce)));
            // Atomic creation, private permissions from the outset; never reuse an existing
            // path (including a symlink) in the shared temporary directory.
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("create private CUPS test directory: {e}"),
            }
        }
        panic!("could not create a unique CUPS test directory");
    }

    #[test]
    fn instances_read_the_queue_ppd_and_their_own_defaults() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = folder();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        // One synthetic queue PPD; there is deliberately no Office/ subdirectory.
        std::fs::write(
            dir.join("Office.ppd"),
            "*OpenUI *InputSlot/Paper tray: PickOne\n*DefaultInputSlot: Auto\n*InputSlot Auto/Automatic: \"\"\n*InputSlot Tray2/Tray 2: \"\"\n*CloseUI: *InputSlot\n",
        )
        .unwrap();
        let boundary_queue = "Q".repeat(64);
        let boundary_instance = format!("Office/{}", "I".repeat(64));
        std::fs::copy(dir.join("Office.ppd"), dir.join(format!("{boundary_queue}.ppd"))).unwrap();
        for (destination, current, expected) in [
            ("Office", "Auto", "Auto"),
            ("Office/duplex", "Tray2", "Tray2"),
            ("Office/draft", "Auto", "Auto"),
            ("Office/unknown-default", "Bogus", "Auto"),
            (boundary_queue.as_str(), "Tray2", "Tray2"),
            (boundary_instance.as_str(), "Tray2", "Tray2"),
        ] {
            let options =
                spool::printer_options_via_within(destination, &dir, command(&dir, &format!("InputSlot/Paper tray: *{current}")), LIMIT, "en");
            assert_eq!(options.len(), 1, "{destination}: the queue's PPD must be read");
            assert_eq!(options[0].default, expected, "{destination}: only valid instance choices override the PPD default");
            assert_eq!(options[0].choices, [("Auto".into(), "Automatic".into()), ("Tray2".into(), "Tray 2".into())]);
            assert_eq!(std::fs::read_to_string(dir.join("args")).unwrap(), format!("-p\n{destination}\n-l"));
        }
        let job = spool::Job { printer: Some("Office/duplex".into()), ..Default::default() };
        assert_eq!(&spool::lp_args(&job)[..2], ["-d", "Office/duplex"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_destinations_are_rejected_before_running_lpoptions() {
        let dir = folder();
        let long_queue = "Q".repeat(65);
        let long_queue_instance = format!("{long_queue}/duplex");
        let long_instance = format!("Office/{}", "I".repeat(65));
        for destination in [
            "",
            ".",
            "..",
            "/Office",
            "Office/",
            "Office/a/b",
            "../../etc/passwd",
            "./duplex",
            "../duplex",
            "Office/.",
            "Office/..",
            "Office\\duplex",
            "Office/\\duplex",
            "Office/with space",
            "Office/line\nbreak",
            "Office/quote\"",
            "Office/equals=",
            "Office/a:b",
            "Office/\0",
            long_queue.as_str(),
            long_queue_instance.as_str(),
            long_instance.as_str(),
        ] {
            assert!(
                spool::printer_options_via_within(destination, &dir, command(&dir, "InputSlot: *Auto"), LIMIT, "en").is_empty(),
                "{destination:?}"
            );
            assert!(!dir.join("args").exists(), "{destination:?}: rejected before spawning");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_scaled_appearance_is_not_scaled_twice() {
    let doc = fixture(1);
    let nums = |v: [f64; 6]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
    let mut form = Dict::new();
    form.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, 10.0, 10.0].iter().map(|x| Object::Real(*x)).collect()));
    form.set(b"Matrix".to_vec(), nums([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]));
    // The form's own /Matrix doubles it to 20x20 when drawn; the placement only has to map 20x20 onto the 40x40 rect.
    let m = appearance_matrix(&doc, &form, [10.0, 10.0, 50.0, 50.0]).unwrap();
    assert!(close(m.0[0], 2.0) && close(m.0[3], 2.0), "{:?}", m.0);
    assert!(close(m.0[4], 10.0) && close(m.0[5], 10.0), "{:?}", m.0);
}

#[test]
fn architectural_sheets_preserve_full_scale() {
    let index = matching_paper((2592.0, 1728.0)).unwrap();
    assert_eq!(PAPERS[index].0, "ARCH D (24 x 36 in)");
    assert_eq!(matching_paper((1728.0, 2592.0)), Some(index));
    assert_eq!(matching_paper((f64::NAN, 2592.0)), None);
    let s = Settings { pages: vec![0], paper: PAPERS[index].1, layout: Layout::Size(SizeMode::Actual), ..Settings::default() };
    let sheets = layout(&[(2592.0, 1728.0)], &s).unwrap();
    assert_eq!(sheets[0].size, (2592.0, 1728.0));
    assert_eq!(sheets[0].placed[0].matrix.0, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let fit = layout(&[(2592.0, 1728.0)], &Settings { layout: Layout::Size(SizeMode::Fit), ..s }).unwrap();
    assert!(fit[0].placed[0].matrix.0[0] < 1.0, "Fit adds margins and must not masquerade as full scale");
}

/// A sheet matches a preset either way round and rounded to whole points, but not half a point off;
/// empty, negative, infinite, NaN and custom sizes match none.
#[test]
fn paper_matching_accepts_rounded_metric_sizes_but_not_custom_or_invalid_sizes() {
    for (i, (_, (w, h))) in PAPERS.iter().enumerate() {
        assert_eq!(matching_paper((*w, *h)), Some(i));
        assert_eq!(matching_paper((*h, *w)), Some(i));
        assert_eq!(matching_paper((w.round(), h.round())), Some(i));
        assert_eq!(matching_paper((w + 0.51, *h)), None);
    }
    for size in [(0.0, 0.0), (-1728.0, 2592.0), (f64::INFINITY, 2592.0), (1728.0, f64::NAN), (1000.0, 1000.0)] {
        assert_eq!(matching_paper(size), None);
    }
}

/// #3752: 13 of the 21 sizes had no media name, so their jobs went out on the queue's default
/// paper (A4 on the report's queue) whatever the sheets were laid out on.
#[test]
fn every_paper_reaches_the_spooler_by_a_name_of_its_size() {
    let ppd_keywords = ["Letter", "Legal", "Tabloid", "A3", "A4", "A5", "w288h432", "Env10"];
    for (i, (name, (w, h))) in PAPERS.iter().enumerate() {
        let media = cups_media(i).unwrap_or_else(|| panic!("{name} has no media name"));
        assert_eq!(media_option(i), Some(("media".to_string(), media.to_string())));
        let args = lp_args(&Job { options: media_option(i).into_iter().collect(), ..Job::default() });
        assert!(args.contains(&format!("media={media}")), "{name}: lp is given it: {args:?}");
        if ppd_keywords.contains(&media) {
            continue;
        }
        // The rest are PWG 5101.1 names, which end in their dimensions: CUPS feeds the driver's
        // size of those dimensions, so they must be the paper's.
        let dims = media.rsplit('_').next().unwrap_or_default();
        let (dims, points_per_unit) = match (dims.strip_suffix("mm"), dims.strip_suffix("in")) {
            (Some(mm), _) => (mm, 72.0 / 25.4),
            (_, Some(inches)) => (inches, 72.0),
            _ => panic!("{name}: {media} is not a PWG name"),
        };
        let (pw, ph) = dims.split_once('x').unwrap_or_else(|| panic!("{name}: {media} has no dimensions"));
        let (pw, ph) = (pw.parse::<f64>().unwrap() * points_per_unit, ph.parse::<f64>().unwrap() * points_per_unit);
        assert!((pw - w).abs() < 0.5 && (ph - h).abs() < 0.5, "{name}: {media} is {pw:.2} x {ph:.2} pt, not {w} x {h}");
    }
    assert_eq!((cups_media(PAPERS.len()), media_option(PAPERS.len())), (None, None));
    // The names themselves, spelled as PWG 5101.1 has them: a driverless printer may take only the
    // names it advertises, so one CUPS can merely parse (ARCH A was `arch_a_9x12in`) is not enough.
    // ARCH E1 is 5101.1's `wide-format` size: `na_arch-e1_30x42in` was asked for once and never
    // registered.
    let names: Vec<(&str, &str)> = PAPERS.iter().enumerate().map(|(i, (name, _))| (*name, cups_media(i).unwrap_or_default())).collect();
    assert_eq!(
        names,
        [
            ("US Letter", "Letter"),
            ("US Legal", "Legal"),
            ("Tabloid", "Tabloid"),
            ("A3", "A3"),
            ("A4", "A4"),
            ("A5", "A5"),
            ("A6", "iso_a6_105x148mm"),
            ("ARCH A (9 x 12 in)", "na_arch-a_9x12in"),
            ("ARCH B (12 x 18 in)", "na_arch-b_12x18in"),
            ("ARCH C (18 x 24 in)", "na_arch-c_18x24in"),
            ("ARCH D (24 x 36 in)", "na_arch-d_24x36in"),
            ("ARCH E1 (30 x 42 in)", "na_wide-format_30x42in"),
            ("ARCH E (36 x 48 in)", "na_arch-e_36x48in"),
            ("ANSI C (17 x 22 in)", "na_c_17x22in"),
            ("ANSI D (22 x 34 in)", "na_d_22x34in"),
            ("ANSI E (34 x 44 in)", "na_e_34x44in"),
            ("A2", "iso_a2_420x594mm"),
            ("A1", "iso_a1_594x841mm"),
            ("A0", "iso_a0_841x1189mm"),
            ("4 x 6 in label (101.6 x 152.4 mm)", "w288h432"),
            ("US #10 Envelope (4.125 x 9.5 in)", "Env10"),
        ]
    );
}

/// #2734: the US #10 envelope (4.125 × 9.5 in) is offered, fed by its PPD name and laid out at
/// its own size.
#[test]
fn us_number_10_envelope_is_offered_and_fed_by_name() {
    let index = matching_paper((684.0, 297.0)).unwrap();
    assert_eq!(PAPERS[index].0, "US #10 Envelope (4.125 x 9.5 in)");
    assert_eq!(PAPERS[index].1, (297.0, 684.0));
    assert_eq!(cups_media(index), Some("Env10"));
    let s = Settings { pages: vec![0], paper: PAPERS[index].1, layout: Layout::Size(SizeMode::Actual), ..Settings::default() };
    assert_eq!(layout(&[(297.0, 684.0)], &s).unwrap()[0].size, (297.0, 684.0));
}

#[test]
fn label_is_cut_turned_upright_and_fitted_without_distortion() {
    let label = LABEL_PAPER;
    assert_eq!(PAPERS[label].0, "4 x 6 in label (101.6 x 152.4 mm)");
    assert_eq!(PAPERS[label].1, (288.0, 432.0));
    assert_eq!(cups_media(label), Some("w288h432"));
    // A sideways label in the lower half of an A4 sheet, text running bottom-to-top.
    let rect = [40.0, 30.0, 560.0, 420.0];
    for (text_up, flip, want_text_dir) in [(true, false, (1.0, 0.0)), (false, false, (1.0, 0.0)), (true, true, (-1.0, 0.0))] {
        // Text direction on the page: up when text_up, down otherwise.
        let page_dir = if text_up { (0.0, 1.0) } else { (0.0, -1.0) };
        let s = Settings {
            pages: vec![0],
            paper: PAPERS[label].1,
            layout: Layout::Label { flip },
            crops: vec![Some(Crop { rect, text_up })],
            ..Settings::default()
        };
        let sheets = layout(&[(595.0, 842.0)], &s).unwrap();
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].size, (288.0, 432.0));
        let pl = sheets[0].placed[0];
        assert_eq!(pl.clip, rect);
        let [a, b, c, d, _, _] = pl.matrix.0;
        // A pure quarter turn with one scale: no stretching.
        assert!((a.hypot(b) - c.hypot(d)).abs() < 1e-9 && (a * c + b * d).abs() < 1e-9);
        let dir = (a * page_dir.0 + c * page_dir.1, b * page_dir.0 + d * page_dir.1);
        let n = dir.0.hypot(dir.1);
        assert!((dir.0 / n - want_text_dir.0).abs() < 1e-9 && (dir.1 / n - want_text_dir.1).abs() < 1e-9, "text_up={text_up} flip={flip}");
        let corners = [(rect[0], rect[1]), (rect[2], rect[1]), (rect[2], rect[3]), (rect[0], rect[3])].map(|(x, y)| pl.matrix.apply(x, y));
        let (x0, x1) = corners.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.0), m.1.max(p.0)));
        let (y0, y1) = corners.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.1), m.1.max(p.1)));
        assert!(x0 >= LABEL_MARGIN - 1e-6 && x1 <= 288.0 - LABEL_MARGIN + 1e-6 && y0 >= LABEL_MARGIN - 1e-6 && y1 <= 432.0 - LABEL_MARGIN + 1e-6);
        // Fills the label's width (the crop's height, 390 pt, becomes it).
        assert!((x1 - x0 - (288.0 - 2.0 * LABEL_MARGIN)).abs() < 1e-6);
    }
    // No crop: the whole page, upright.
    let whole =
        layout(&[(200.0, 300.0)], &Settings { pages: vec![0], paper: (288.0, 432.0), layout: Layout::Label { flip: false }, ..Settings::default() })
            .unwrap();
    assert_eq!(whole[0].placed[0].clip, [0.0, 0.0, 200.0, 300.0]);
    assert_eq!(whole[0].placed[0].matrix.0[1], 0.0);
}

#[test]
fn text_direction_is_read_from_text_and_form_matrices() {
    let doc = fixture(1);
    assert!(!text_reads_up(&doc, 0), "upright text");
    assert!(!text_reads_up(&doc, 9), "no such page");
}

#[test]
fn imposed_pages_preserve_large_user_unit_matrices() {
    let (unit, mode) = (75000.0, SizeMode::Fit);
    let mut doc = fixture(1);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"UserUnit".to_vec(), Object::Real(unit))).unwrap();
    let out = impose(&doc, &Settings { content: Content::Document, ..settings(vec![0], Layout::Size(mode)) }).unwrap();
    let printed = Document::open(Arc::new(out)).unwrap();
    let page = &pdfcraft_model::pages(&printed)[0];
    let contents = printed.resolve(page.dict.get(b"Contents").unwrap());
    let Object::Stream(stream) = &*contents else { panic!("sheet content") };
    let bytes = stream.decoded().unwrap();
    let parsed = pdfcraft_content::parse(&bytes);
    assert_eq!(parsed.skipped, 0);
    let cms: Vec<_> = parsed.ops.iter().filter(|op| op.is("cm")).collect();
    assert_eq!(cms.len(), 2, "placement and source-to-display matrices");
    let normalized = f64::from(unit as f32);
    let scale = if mode == SizeMode::Actual { 1.0 } else { 756.0 / (300.0 * normalized) };
    let expected = [
        [scale, 0.0, 0.0, scale, 306.0 - 100.0 * normalized * scale, 396.0 - 150.0 * normalized * scale],
        [normalized, 0.0, 0.0, normalized, 0.0, 0.0],
    ];
    for (cm, expected) in cms.iter().zip(expected) {
        assert_eq!(cm.operands.len(), 6);
        assert!(!bytes[cm.span.clone()].contains(&b'e') && !bytes[cm.span.clone()].contains(&b'E'));
        for (actual, expected) in cm.nums::<6>().unwrap().into_iter().zip(expected) {
            assert!((actual - expected).abs() <= 1e-12 * expected.abs(), "unit={unit}: {actual} != {expected}");
        }
    }
    let clip = parsed.ops.iter().find(|op| op.is("re")).unwrap();
    assert_eq!(clip.operands.len(), 4);
    let rect = clip.nums::<4>().unwrap();
    assert_eq!(rect, [0.0, 0.0, 200.0 * normalized, 300.0 * normalized]);
    assert!(rect[2] > 0.0 && rect[3] > 0.0, "the clip must retain the visible page");
    let clip_tokens = std::str::from_utf8(&bytes[clip.span.clone()]).unwrap();
    assert!(clip_tokens.split_whitespace().take(4).all(|token| !token.contains('e') && !token.contains('E')));
    let apply = |m: [f64; 6], p: [f64; 2]| [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]];
    let to_sheet = |p| apply(cms[0].nums::<6>().unwrap(), apply(cms[1].nums::<6>().unwrap(), p));
    let mut anchors = vec![([100.0, 150.0], [306.0, 396.0])];
    if mode == SizeMode::Fit {
        anchors.extend([([0.0, 0.0], [54.0, 18.0]), ([200.0, 300.0], [558.0, 774.0])]);
    }
    for (raw, expected) in anchors {
        for (actual, expected) in to_sheet(raw).into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-9, "unit={unit}: {actual} != {expected}");
        }
    }
}

/// `output_within` (#1818): the bounded wait every platform's `lpoptions` call goes through.
#[cfg(not(target_arch = "wasm32"))]
mod bounded_commands {
    const MODE: &str = "PDFCRAFT_COMMAND_TEST_MODE";

    /// The child process stands in for a command: it answers, or it never does.
    #[test]
    fn stand_in_for_a_command() {
        match std::env::var(MODE).as_deref() {
            Ok("answer") => println!("hello from the command"),
            Ok("hang") => std::thread::sleep(std::time::Duration::from_secs(60)),
            // Holds the output pipe it inherited for a while, as a daemon a command starts can.
            Ok("linger") => std::thread::sleep(LINGER),
            // Starts a lingering process on its own output, then exits or hangs itself.
            Ok(mode @ ("orphan-exit" | "orphan-hang")) => {
                let _ = command("linger").spawn();
                println!("started");
                if mode == "orphan-hang" {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                }
            }
            _ => {}
        }
    }

    const LINGER: std::time::Duration = std::time::Duration::from_secs(8);

    fn command(mode: &str) -> std::process::Command {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "tests::bounded_commands::stand_in_for_a_command", "--nocapture", "--"]).env(MODE, mode);
        c
    }

    #[test]
    fn a_command_that_answers_gives_its_output() {
        let out = crate::spool::output_within(&mut command("answer"), std::time::Duration::from_secs(60)).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("hello from the command"));
    }

    #[test]
    fn a_command_that_never_answers_is_stopped_at_the_deadline() {
        let started = std::time::Instant::now();
        let err = crate::spool::output_within(&mut command("hang"), std::time::Duration::from_millis(500)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "stopped soon after the deadline, not when the command gives up");
    }

    /// A process the command started keeps the output pipe open after the command has exited,
    /// or after it was killed at the deadline: the wait still ends at the deadline (#2108
    /// review), not when that process lets go of the pipe.
    #[test]
    fn a_process_left_holding_the_output_does_not_extend_the_wait() {
        for mode in ["orphan-exit", "orphan-hang"] {
            let started = std::time::Instant::now();
            let err = crate::spool::output_within(&mut command(mode), std::time::Duration::from_millis(1500)).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{mode}");
            assert!(started.elapsed() < LINGER / 2, "{mode}: returned after {:?}, when the lingering process let go", started.elapsed());
        }
    }
}

#[test]
fn driver_options_take_the_ppds_own_names_in_the_ui_language() {
    use crate::spool::{PrinterOption, localize, parse_ppd, ppd_text};
    // Latin-1 elsewhere (the é), UTF-8 in the localization lines, as drivers ship them.
    let mut ppd = b"*cupsLanguages: \"bg pt_BR\"\n*OpenGroup: Finishing/Finishing Options\n*OpenUI *KMDuplex/Print Type: PickOne\n*DefaultKMDuplex: Single\n*KMDuplex Single/1-Sided: \"\"\n*KMDuplex Double/2-Sided: \"\"\n*CloseUI: *KMDuplex\n*OpenUI *Staple/Staple: PickOne\n*DefaultStaple: None\n*Staple None/None: \"\"\n*CloseUI: *Staple\n*CloseGroup: Finishing\n*fr.Translation KMDuplex/Type d'impression r".to_vec();
    ppd.push(0xE9);
    ppd.extend_from_slice("el: \"\"\n*bg.Translation Finishing/Довършителни операции: \"\"\n*bg.Translation KMDuplex/Тип печат: \"\"\n*bg.KMDuplex Single/Едностранен: \"\"\n*bg.KMDuplex Double/: \"\"\n*pt_BR.Translation KMDuplex/Tipo de impressão: \"\"\n".as_bytes());
    let parsed = parse_ppd(&ppd_text(&ppd));
    let label = |o: &[PrinterOption], key: &str| o.iter().find(|o| o.key == key).map(|o| o.label.clone()).unwrap();
    let mut bg = parsed.clone();
    localize(&mut bg, &ppd, "bg");
    let duplex = bg.iter().find(|o| o.key == "KMDuplex").unwrap();
    assert_eq!((duplex.label.as_str(), duplex.group.as_str()), ("Тип печат", "Довършителни операции"));
    assert_eq!(
        duplex.choices,
        [("Single".to_string(), "Едностранен".to_string()), ("Double".to_string(), "2-Sided".to_string())],
        "an empty translation keeps the driver's name"
    );
    assert_eq!(label(&bg, "Staple"), "Staple", "untranslated options keep theirs");
    // Region-specific codes, then the language alone; languages the PPD lacks change nothing.
    let mut pt = parsed.clone();
    localize(&mut pt, &ppd, "pt-br");
    assert_eq!(label(&pt, "KMDuplex"), "Tipo de impressão");
    for lang in ["en", "de", "", "bg-"] {
        let mut other = parsed.clone();
        localize(&mut other, &ppd, lang);
        assert_eq!(other == parsed, lang != "bg-", "{lang:?}");
    }
}

mod appearances;

#[test]
fn repeated_content_references_share_one_decode_budget() {
    let mut doc = Document::new_empty();
    let stream = doc.add(Stream::flate(Dict::new(), &vec![b' '; 1 << 20]));
    let page: Dict = [(b"Contents".to_vec(), Object::Array(vec![Object::Ref(stream); 300]))].into_iter().collect();
    let data = decoded_contents(&doc, &page, &mut DecodeBudget::page());
    assert!(data.is_none_or(|d| d.len() <= pdfcraft_cos::MAX_PAGE_DECODED + 300));
}

/// 20 draws of a form that draws the next form 20 times, four levels down: 168,420 form
/// entries from a few hundred bytes.
fn form_fanout(doc: &mut Document) -> Dict {
    let draws = "/X Do ".repeat(20);
    let mut form = |content: &str, inner: Option<ObjRef>| {
        let mut d: Dict = [(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()))].into_iter().collect();
        if let Some(inner) = inner {
            let xobjects: Dict = [(b"X".to_vec(), Object::Ref(inner))].into_iter().collect();
            let res: Dict = [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect();
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
        doc.add(Stream::from_raw(d, content.as_bytes().to_vec()))
    };
    let mut inner = form("BT 0 1 -1 0 0 0 Tm (a) Tj ET", None);
    for _ in 0..3 {
        inner = form(&draws, Some(inner));
    }
    let xobjects: Dict = [(b"X".to_vec(), Object::Ref(inner))].into_iter().collect();
    [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect()
}

#[test]
fn text_direction_stops_at_the_form_budget() {
    let mut doc = Document::new_empty();
    let res = form_fanout(&mut doc);
    let mut score = 0;
    text_direction(&doc, "/X Do ".repeat(20).as_bytes(), &res, Matrix::IDENTITY, 0, &mut score, &mut DecodeBudget::page());
    assert!(score > 0 && score <= pdfcraft_cos::MAX_PAGE_FORMS as i64, "score {score}");
}

/// A form refused by the spent budget is left out; the text drawn after it still counts.
#[test]
fn text_direction_reads_on_past_a_refused_form() {
    let mut doc = Document::new_empty();
    let form = doc.add(Stream::from_raw([(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()))].into_iter().collect(), Vec::new()));
    let xobjects: Dict = [(b"X".to_vec(), Object::Ref(form))].into_iter().collect();
    let res: Dict = [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect();
    let mut score = 0;
    let content = b"/X Do BT 0 1 -1 0 0 0 Tm (a) Tj ET";
    text_direction(&doc, content, &res, Matrix::IDENTITY, 0, &mut score, &mut DecodeBudget::new(1 << 20, 0));
    assert_eq!(score, 1);
}

/// #3984: odd and even are page numbers, as Acrobat counts them, not places in the range.
#[test]
fn odd_and_even_pages_are_page_numbers_not_places_in_the_range() {
    let even = |range: &str| select_pages(12, Some(range), &[], Subset::Even, false).unwrap();
    let odd = |range: &str| select_pages(12, Some(range), &[], Subset::Odd, false).unwrap();
    // Adobe Acrobat DC Help (11/28/2019), Options in the Print dialog box, p. 515: "if you set the
    // range to 2, 7–10 and choose Even Pages Only, only pages 2, 8, and 10 print".
    assert_eq!(even("2, 7-10"), [1, 7, 9]);
    // The report: 4–11 and 3–11 both print pages 4, 6, 8 and 10 as Even pages only.
    assert_eq!(even("4-11"), [3, 5, 7, 9]);
    assert_eq!(even("3-11"), [3, 5, 7, 9]);
    assert_eq!(odd("4-11"), [4, 6, 8, 10]);
    // Typed order is kept, and Reverse turns it round.
    assert_eq!(even("10, 3, 4"), [9, 3]);
    assert_eq!(select_pages(12, Some("10, 3, 4"), &[], Subset::Even, true).unwrap(), [3, 9]);
    // A range with no page of the kind prints nothing.
    assert_eq!(select_pages(12, Some("3, 5"), &[], Subset::Even, false), Err(PrintError::NoPages));
}

/// ISO 32000-2 Table 167: an Invisible annotation of a type that isn't standard does not print,
/// even with its Print flag; Invisible means nothing for a standard type.
#[test]
fn invisible_annotations_of_unknown_types_do_not_print() {
    let doc = fixture(1);
    let annot = |subtype: Option<&str>, flags: i64| {
        let mut d = Dict::new();
        if let Some(s) = subtype {
            d.set(b"Subtype".to_vec(), Object::name(s));
        }
        d.set(b"F".to_vec(), Object::Int(flags));
        d
    };
    let all = Content::DocumentAndMarkups;
    assert!(prints(&doc, &annot(Some("MyCustomType"), 4), all), "an unknown type prints without Invisible");
    assert!(!prints(&doc, &annot(Some("MyCustomType"), 5), all), "Invisible + Print on an unknown type");
    assert!(!prints(&doc, &annot(None, 5), all), "no Subtype is no standard type");
    assert!(prints(&doc, &annot(Some("Square"), 5), all), "Invisible doesn't apply to a standard type");
    assert!(prints(&doc, &annot(Some("Widget"), 5), Content::FormFieldsOnly), "nor to a field");

    // An indirect `/Subtype` (§7.3.10) is resolved: a standard type stays standard.
    let mut doc = doc;
    let square = doc.add(Object::name("Square"));
    let mut indirect = annot(None, 5);
    indirect.set(b"Subtype".to_vec(), Object::Ref(square));
    assert!(prints(&doc, &indirect, all), "an indirect standard Subtype");
}

/// #2707: a booklet's two-sided setting follows its subset.
#[test]
fn a_booklet_prints_both_sides_on_the_short_edge() {
    assert_eq!(booklet_duplex(BookletSubset::BothSides), spool::Duplex::ShortEdge);
    assert_eq!(booklet_duplex(BookletSubset::FrontOnly), spool::Duplex::Off);
    assert_eq!(booklet_duplex(BookletSubset::BackOnly), spool::Duplex::Off);
}
