//! A form with resources of its own looks a name they lack up in its caller's, as the renderer
//! does (hayro-syntax `Resources::for_form`): the walk and the proof read it so too.

use super::surviving_references::full_save_holds;
use super::*;

/// A tiling pattern whose cell is red, so a box filled with it is told apart from black.
fn red_cell() -> Vec<u8> {
    stream(
        "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << >>",
        b"1 0 0 rg 0 0 300 300 re f",
    )
}

/// The image `/Im1` (object 7, bytes `SECRET_IMAGE_BYTES`), drawn under the mark by the form
/// `/Fm` (object 8), whose `/Resources` are `form_res`: the page's draw it either way.
fn image_through_a_form(form_res: &str, extra: Vec<Vec<u8>>) -> Document {
    let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
    let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {form_res}"), b"q 100 0 0 30 20 240 cm /Im1 Do Q");
    let mut objs = vec![image, form];
    objs.extend(extra);
    let mut doc = one_page(b"/Fm Do", "/XObject << /Im1 7 0 R /Fm 8 0 R >>", objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    doc
}

/// An image a form draws under the mark through its caller's `/XObject`, its own resources lacking
/// the name, is cleared, and its bytes leave the file, as when the form has no resources at all.
/// Before, the walk looked the name up in the form's own resources only, found nothing, and left
/// the image in the file, drawn under the mark.
#[test]
fn an_image_a_form_draws_through_its_callers_names_is_redacted() {
    for form_res in ["", "/Resources << /ProcSet [/PDF] >>", "/Resources << >>", "/Resources << /XObject << >> >>", "/Resources 9 0 R"] {
        let mut doc = image_through_a_form(form_res, vec![b"<< /ProcSet [/PDF] >>".to_vec()]);
        let report = apply(&mut doc, None).unwrap();
        assert_eq!(report.images_removed, 1, "form {form_res:?}");
        assert!(!full_save_holds(&doc, b"SECRET_IMAGE_BYTES"), "form {form_res:?}: the image stayed in the file");
    }
}

/// A name a form's own resources give is looked up there, not in its caller's: the form's own
/// `/Im1`, outside the mark's reach as it is drawn, isn't the page's under it. And one they give
/// as something no XObject is (a number) is looked up in the caller's, as the renderer does.
#[test]
fn a_forms_own_name_wins_unless_it_isnt_an_xobject() {
    let own_image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"PUBLIC_IMAGE_BYTES");
    let mut doc = image_through_a_form("/Resources << /XObject << /Im1 9 0 R >> >>", vec![own_image]);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.images_removed, 1, "the form's own image, under the mark, goes");
    assert!(!full_save_holds(&doc, b"PUBLIC_IMAGE_BYTES"));
    assert!(full_save_holds(&doc, b"SECRET_IMAGE_BYTES"), "the page's /Im1, which nothing draws, stays");
    let mut doc = image_through_a_form("/Resources << /XObject << /Im1 5 >> >>", vec![]);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.images_removed, 1, "a number is no XObject: the page's image goes");
    assert!(!full_save_holds(&doc, b"SECRET_IMAGE_BYTES"));
}

/// The page fills a box under the mark with `/P0` and draws `/Fm`, which fills a box beside it
/// with `/P0`, a name only the page's resources give: the renderer paints it red there. Whatever
/// the form's own resources hold besides, the pattern stays, as it does for a form without
/// resources; before, a form with resources of its own passed nothing it painted to the page, and
/// the pattern went, leaving the box beside the mark unpainted.
#[test]
fn a_page_pattern_a_form_paints_through_its_callers_names_stays() {
    let page = b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g /Fm Do";
    let res = "/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R >>";
    // A form's own `/P0` that refers to an object the file doesn't define is no pattern: the renderer
    // looks the name up in the page's.
    for form_res in [
        "",
        "/Resources << >>",
        "/Resources << /ProcSet [/PDF] >>",
        "/Resources << /Font << /F1 5 0 R >> >>",
        "/Resources << /Pattern << /P0 99 0 R >> >>",
    ] {
        let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {form_res}"), b"/Pattern cs /P0 scn 20 40 100 30 re f");
        let mut doc = one_page(page, res, vec![red_cell(), form]);
        mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().patterns_removed, 0, "form {form_res:?}");
    }
    // A form whose own resources give `/P0` paints with its own: the page's goes.
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Pattern << /P0 9 0 R >> >>",
        b"/Pattern cs /P0 scn 20 40 100 30 re f",
    );
    let mut doc = one_page(page, res, vec![red_cell(), form, red_cell()]);
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().patterns_removed, 1, "the form's own /P0");
}

/// The page fills a box under the mark with `/P0`, and draws `/Fm` clear of the mark (its bounding
/// box below it), filling a box, or stroking a line, with `/P0` through the page's names: the walk
/// leaves the form as it is, and the pattern it still paints with stays, with resources of its own
/// or without. Where the form's own resources give `/P0`, the page's goes.
#[test]
fn a_page_pattern_a_form_clear_of_the_marks_paints_stays() {
    let run = |form_res: &str, extra: Vec<Vec<u8>>, paint: &[u8]| {
        let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {form_res}"), paint);
        let mut objs = vec![red_cell(), form];
        objs.extend(extra);
        let mut doc = one_page(b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g /Fm Do", "/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R >>", objs);
        mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
        apply(&mut doc, None).map(|r| r.patterns_removed)
    };
    let fill = b"/Pattern cs /P0 scn 20 40 100 30 re f".as_slice();
    let stroke = b"5 w /Pattern CS /P0 SCN 20 40 m 120 40 l S".as_slice();
    for paint in [fill, stroke] {
        for form_res in ["", "/Resources << >>", "/Resources << /ProcSet [/PDF] >>"] {
            assert_eq!(run(form_res, vec![], paint), Ok(0), "form {form_res:?}");
        }
        assert_eq!(run("/Resources << /Pattern << /P0 9 0 R >> >>", vec![red_cell()], paint), Ok(1), "the form's own /P0");
    }
}

/// `/G`, clear of the mark, draws `/H` through the names of what draws it. The page draws `/Fm`
/// over the mark, which draws `/G` with its own `/H`, a form painting nothing; then the page draws
/// `/G` itself, whose `/H` is then the page's, painting with `/P0`. `/G` is read for each, and the
/// page's `/P0` stays.
#[test]
fn a_form_clear_of_the_marks_is_read_for_each_chain_of_forms_it_is_drawn_in() {
    let clear = "/Type /XObject /Subtype /Form /BBox [0 0 300 100]";
    let mut doc = one_page(
        b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g /Fm Do /G Do",
        "/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R /G 9 0 R /H 10 0 R >>",
        vec![
            red_cell(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /G 9 0 R /H 11 0 R >> >>", b"/G Do"),
            stream(clear, b"/H Do"),
            stream(clear, b"/Pattern cs /P0 scn 20 40 100 30 re f"),
            stream(clear, b"0 0 10 10 re f"),
        ],
    );
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    crate::interp::UNWALKED_READS.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.patterns_removed), Ok(0));
    assert_eq!(crate::interp::UNWALKED_READS.with(std::cell::Cell::get), 2);
}

/// The page draws `/Fm` over the mark, which draws `/G` clear of it through `/Fm`'s names: `/G` is
/// left as it is, and the page's `/P0` it paints with stays.
#[test]
fn a_page_pattern_a_form_clear_of_the_marks_paints_inside_a_walked_form_stays() {
    for fm_res in ["", "/Resources << /XObject << /G 9 0 R >> >>"] {
        let fm = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {fm_res}"), b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g /G Do");
        let g = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100]", b"/Pattern cs /P0 scn 20 40 100 30 re f");
        let mut doc = one_page(b"/Fm Do", "/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R /G 9 0 R >>", vec![red_cell(), fm, g]);
        mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.patterns_removed), Ok(0), "/Fm {fm_res:?}");
    }
}

/// A form clear of the mark that can't all be read (a string left open) may paint with the page's
/// `/P0`, which nothing else still paints with: the redaction is refused rather than drop it, for
/// the form drawn by the page or inside a form the walk enters. Where the page paints with `/P0`
/// beside the mark too, nothing is dropped, and the page drawing such a form with resources of its
/// own is redacted.
#[test]
fn a_page_pattern_a_form_clear_of_the_marks_may_paint_past_what_is_read_refuses_the_redaction() {
    let run = |form_res: &str, page: &[u8]| {
        let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {form_res}"), b"( /Pattern cs /P0 scn 20 40 100 30 re f");
        // Over the mark, so it is walked, but painting nothing there: it is left unchanged.
        let over = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/Fm Do");
        let mut doc = one_page(page, "/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R /Over 9 0 R >>", vec![red_cell(), form, over]);
        mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
        apply(&mut doc, None)
    };
    let refused = |out: &Result<Report, RedactError>| matches!(out, Err(RedactError::Cos(pdfcraft_cos::CosError::Syntax { detail, .. })) if detail.contains("may paint with a pattern it would remove"));
    for form_res in ["", "/Resources << >>"] {
        let out = run(form_res, b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g /Fm Do");
        assert!(refused(&out), "{form_res:?}: {out:?}");
        let out = run(form_res, b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g /Over Do");
        assert!(refused(&out), "inside /Over, {form_res:?}: {out:?}");
    }
    // With resources of its own: a form without, which the page draws, is read in full by the
    // page's font cleanup, and refused there, as before.
    let out = run("/Resources << >>", b"/Pattern cs /P0 scn 20 140 100 30 re f 20 240 100 30 re f 0 g /Fm Do");
    assert_eq!(out.map(|r| r.patterns_removed), Ok(0), "painted beside the mark too");
}

/// A form clear of the mark drawn 1,000 times is read for its patterns once; on a page whose
/// resources give no pattern, not at all. 4,096 forms clear of the mark are read, and the page's
/// `/P0`, painted only under the mark, goes; with one more, which isn't read, which of the patterns
/// that one paints with isn't known, and the redaction is refused rather than drop `/P0`.
#[test]
fn forms_clear_of_the_marks_are_read_for_their_patterns_once_and_up_to_4096() {
    let reads = |page_res: &str, objs: Vec<Vec<u8>>, content: &[u8]| {
        let mut doc = one_page(content, page_res, objs);
        mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
        crate::interp::UNWALKED_READS.with(|c| c.set(0));
        let out = apply(&mut doc, None).map(|r| r.patterns_removed);
        (out, crate::interp::UNWALKED_READS.with(std::cell::Cell::get))
    };
    let form = || stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100]", b"0 0 10 10 re f");
    let again = [b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g ".as_slice(), &b"/Fm Do ".repeat(1000)].concat();
    assert_eq!(reads("/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R >>", vec![red_cell(), form()], &again), (Ok(1), 1));
    let unpatterned = [b"0 g 20 140 100 30 re f ".as_slice(), &b"/Fm Do ".repeat(1000)].concat();
    assert_eq!(reads("/XObject << /Fm 7 0 R >>", vec![form()], &unpatterned), (Ok(0), 0));
    for (forms, refused) in [(4096, false), (4097, true)] {
        let names: String = (0..forms).map(|i| format!("/F{i} {} 0 R ", 8 + i)).collect();
        let draws: String = (0..forms).map(|i| format!("/F{i} Do ")).collect();
        let mut objs = vec![red_cell()];
        objs.extend((0..forms).map(|_| form()));
        let content = format!("/Pattern cs /P0 scn 20 140 100 30 re f 0 g {draws}");
        let (out, read) = reads(&format!("/Pattern << /P0 7 0 R >> /XObject << {names}>>"), objs, content.as_bytes());
        assert_eq!((out.is_err(), read), (refused, 4096), "{forms} forms: {out:?}");
        if !refused {
            assert_eq!(out, Ok(1));
        }
    }
}

/// A form clear of the mark selecting `/P0` 10,000 times and `/Q` once is drawn 1,000 times by the
/// page and 300 times inside a form over the mark: it is read once for each chain, and counts its
/// two names once in each scope it is drawn in. The page's `/P1`, painted only under the mark, goes.
#[test]
fn a_form_clear_of_the_marks_counts_each_pattern_once_in_each_scope() {
    let paints = [b"/Pattern cs ".as_slice(), &b"/P0 scn ".repeat(10_000), b"/Q scn 0 0 10 10 re f"].concat();
    let draws = b"/Fm Do ".repeat(1000);
    let over = [b"/Pattern cs /P1 scn 20 140 100 30 re f 0 g ".as_slice(), &draws[..300 * 7]].concat();
    let page = [b"/Pattern cs /P1 scn 20 140 100 30 re f 0 g /Over Do ".as_slice(), &draws].concat();
    let mut doc = one_page(
        &page,
        "/Pattern << /P0 7 0 R /P1 7 0 R >> /XObject << /Fm 8 0 R /Over 9 0 R >>",
        vec![
            red_cell(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100]", &paints),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", &over),
        ],
    );
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    crate::interp::UNWALKED_READS.with(|c| c.set(0));
    crate::interp::UNWALKED_NAMES.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.patterns_removed), Ok(1));
    assert_eq!(crate::interp::UNWALKED_READS.with(std::cell::Cell::get), 2);
    assert_eq!(crate::interp::UNWALKED_NAMES.with(std::cell::Cell::get), 4);
}

/// A form clear of the mark selects one pattern whose name is 4,064 bytes long, counted as 4,096,
/// drawn inside a form over the mark, a scope for each draw: 4,096 draws count 16 MiB, and the
/// page's `/Q`, painted only under the mark, goes; with one more, past what a page counts, which
/// patterns it paints with isn't known, and the redaction is refused rather than drop `/Q`.
#[test]
fn forms_clear_of_the_marks_count_up_to_16_mib_of_pattern_names_on_a_page() {
    let long = "P".repeat(4064);
    for (draws, refused) in [(4096, false), (4097, true)] {
        let mut doc = one_page(
            &[b"/Pattern cs /Q scn 20 140 100 30 re f 0 g ".as_slice(), &b"/Over Do ".repeat(draws)].concat(),
            &format!("/Pattern << /{long} 7 0 R /Q 7 0 R >> /XObject << /Fm 8 0 R /Over 9 0 R >>"),
            vec![
                red_cell(),
                stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100]", format!("/Pattern cs /{long} scn 0 0 10 10 re f").as_bytes()),
                stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/Fm Do"),
            ],
        );
        mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
        let out = apply(&mut doc, None).map(|r| r.patterns_removed);
        assert_eq!(out.is_err(), refused, "{draws} draws: {out:?}");
        if !refused {
            assert_eq!(out, Ok(1));
        }
    }
}

/// A page whose `/Pattern` gives 1,000 names draws 100 forms clear of the mark, each 1,000 times:
/// whether the page's names give a pattern is looked up once, not for each form or draw, and not
/// at all by the check that walks the page again after.
#[test]
fn forms_clear_of_the_marks_drawn_again_and_again_look_the_patterns_up_once() {
    let names: String = (0..1000).map(|i| format!("/P{i} 7 0 R ")).collect();
    let forms: String = (0..100).map(|i| format!("/F{i} {} 0 R ", 8 + i)).collect();
    let draws: Vec<u8> = (0..100_000).flat_map(|i| format!("/F{} Do ", i % 100).into_bytes()).collect();
    let mut objs = vec![red_cell()];
    objs.extend((0..100).map(|_| stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100]", b"0 0 10 10 re f")));
    let mut doc = one_page(
        &[b"/Pattern cs /P0 scn 20 140 100 30 re f 0 g ".as_slice(), &draws].concat(),
        &format!("/Pattern << {names}>> /XObject << {forms}>>"),
        objs,
    );
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    crate::interp::PATTERN_LOOKUPS.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.patterns_removed), Ok(1));
    assert_eq!(crate::interp::PATTERN_LOOKUPS.with(std::cell::Cell::get), 1);
}

/// 100 forms clear of the mark each draw `/F`, whose own resources give 1,000 XObject names: the
/// reads of those forms for their patterns copy `/F`'s own names once between them, not once each.
#[test]
fn forms_clear_of_the_marks_share_what_their_reads_take_of_a_form() {
    let own: String = (0..1000).map(|i| format!("/X{i} 9 0 R ")).collect();
    let forms: String = (0..100).map(|i| format!("/G{i} {} 0 R ", 10 + i)).collect();
    let draws: String = (0..100).map(|i| format!("/G{i} Do ")).collect();
    let clear = "/Type /XObject /Subtype /Form /BBox [0 0 300 100]";
    let mut objs = vec![red_cell(), stream(&format!("{clear} /Resources << /XObject << {own}>> >>"), b"0 0 10 10 re f"), stream(clear, b"")];
    objs.extend((0..100).map(|_| stream(clear, b"/F Do")));
    let mut doc = one_page(
        format!("/Pattern cs /P0 scn 20 140 100 30 re f 0 g {draws}").as_bytes(),
        &format!("/Pattern << /P0 7 0 R >> /XObject << /F 8 0 R {forms}>>"),
        objs,
    );
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    crate::interp::OWN_ENTRIES.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.patterns_removed), Ok(1));
    let copied = crate::interp::OWN_ENTRIES.with(std::cell::Cell::get);
    assert!(copied < 10_000, "{copied}");
}

/// 1,000 forms with resources of their own, drawn by a form clear of the mark, each give the page's
/// `/XObject` (object 7, 1,002 names) by reference, or all give one `/Resources` by reference whose
/// `/XObject` holds the same names: the page's cleanups copy those names once each, not once for
/// each form.
#[test]
fn forms_giving_one_dictionary_share_one_copy_of_it() {
    let forms: String = (0..1000).map(|i| format!("/F{i} {} 0 R ", 11 + i)).collect();
    let draws: String = (0..1000).map(|i| format!("/F{i} Do ")).collect();
    for form_res in ["/Resources << /XObject 7 0 R >>", "/Resources 10 0 R"] {
        let mut objs = vec![
            format!("<< /Im 8 0 R /U 9 0 R {forms}>>").into_bytes(),
            stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100]", draws.as_bytes()),
            format!("<< /XObject << /Im 8 0 R /U 9 0 R {forms}>> >>").into_bytes(),
        ];
        objs.extend((0..1000).map(|_| stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {form_res}"), b"")));
        let mut doc = one_page(b"q 100 0 0 30 20 240 cm /Im Do Q /U Do", "/XObject 7 0 R", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::interp::OWN_ENTRIES.with(|c| c.set(0));
        // What the redaction then decides is for other tests; this one weighs what it copies.
        let _ = apply(&mut doc, None);
        let copied = crate::interp::OWN_ENTRIES.with(std::cell::Cell::get);
        assert!(copied < 10_020, "{form_res}: {copied}");
    }
}

/// 4,096 forms clear of the mark each draw `/H`, which selects 1,000 patterns: their reads count
/// those names until 16 MiB is passed, keeping no more than that, then no more are read, and the
/// redaction is refused rather than drop the page's `/P0`.
#[test]
fn forms_clear_of_the_marks_stop_being_read_once_16_mib_of_names_is_passed() {
    let selects: String = (0..1000).map(|i| format!("/N{i} scn ")).collect();
    let forms: String = (0..4096).map(|i| format!("/F{i} {} 0 R ", 9 + i)).collect();
    let draws: String = (0..4096).map(|i| format!("/F{i} Do ")).collect();
    let clear = "/Type /XObject /Subtype /Form /BBox [0 0 300 100]";
    let mut objs = vec![red_cell(), stream(clear, format!("/Pattern cs {selects}0 0 10 10 re f").as_bytes())];
    objs.extend((0..4096).map(|_| stream(clear, b"/H Do")));
    let mut doc = one_page(
        format!("/Pattern cs /P0 scn 20 140 100 30 re f 0 g {draws}").as_bytes(),
        &format!("/Pattern << /P0 7 0 R >> /XObject << /H 8 0 R {forms}>>"),
        objs,
    );
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    crate::interp::UNWALKED_READS.with(|c| c.set(0));
    crate::interp::UNWALKED_HELD.with(|c| c.set(0));
    let out = apply(&mut doc, None);
    assert!(
        matches!(&out, Err(RedactError::Cos(pdfcraft_cos::CosError::Syntax { detail, .. })) if detail.contains("may paint with a pattern it would remove")),
        "{out:?}"
    );
    let reads = crate::interp::UNWALKED_READS.with(std::cell::Cell::get);
    assert!(reads < 1000, "{reads}");
    let held = crate::interp::UNWALKED_HELD.with(std::cell::Cell::get);
    assert!(held <= 16 << 20, "{held}");
}

/// A page whose form shows `SECRET` with a font only the page's resources give, 20 points a glyph
/// at 10 points: `T`, the sixth, lies at x 120 to 140.
fn wide_text_through_a_form() -> Document {
    let widths = format!("[{}]", vec!["2000"; 95].join(" "));
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F2 6 0 R >> /XObject << /Fm 5 0 R >> >> >>"
            .to_vec(),
        stream("", b"/Fm Do"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] >>", b"BT /F2 10 Tf 20 250 Td (SECRET) Tj ET"),
        FONT.replace("95 0 R", "7 0 R").into_bytes(),
        widths.into_bytes(),
    ])
}

/// The proof reads a form's names as the walk does: before anything is applied, the `T` the
/// page's wide font places under the mark, and the image the form draws under it through the
/// page's names, are found, and the proof fails.
#[test]
fn the_proof_reads_a_forms_names_through_its_callers() {
    let mut doc = wide_text_through_a_form();
    mark(&mut doc, 0, &[[121.0, 245.0, 139.0, 262.0]], "");
    let proof = crate::tests_verify::prove_unapplied(&mut doc);
    // Placed with the page's font, the `T` is inside the region, where the proof places it
    // independently of the redaction.
    let text = proof.to_text();
    assert!(text.contains("1 glyph(s) still show inside the region"), "{text}");
    let mut doc = image_through_a_form("/Resources << /ProcSet [/PDF] >>", vec![]);
    let proof = crate::tests_verify::prove_unapplied(&mut doc);
    assert!(!proof.passed(), "{}", proof.to_text());
    // A pattern cell drawing, through a form, an image only the page's resources give: the proof
    // can't place images inside a pattern, and says so.
    let cell = stream(
        "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << /XObject << /Fm 9 0 R >> >>",
        b"/Fm Do",
    );
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] >>", b"/Im1 Do");
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x");
    let content = b"/Pattern cs /P0 scn 0 0 300 300 re f BT /F1 10 Tf 20 250 Td (SECRET) Tj ET";
    let mut doc = one_page(content, "/Pattern << /P0 7 0 R >> /XObject << /Im1 8 0 R >>", vec![cell, image, form]);
    mark(&mut doc, 0, &[[15.0, 245.0, 60.0, 262.0]], "");
    let proof = crate::tests_verify::prove_unapplied(&mut doc);
    let text = proof.to_text();
    assert!(text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"), "{text}");
}

/// Text a form shows with a font only its caller's resources give is placed with that font: its
/// glyphs are 20 points wide at 10 points, so `T`, the sixth, lies under a mark at x 121 to 139,
/// and only it goes. Before, the walk placed them with fallback metrics, about a quarter as wide,
/// and found nothing under the mark.
#[test]
fn text_a_form_shows_with_its_callers_font_is_redacted() {
    let mut doc = wide_text_through_a_form();
    let rect = [121.0, 245.0, 139.0, 262.0];
    assert_eq!(under(&mut doc, 0, &[rect]), 1, "the walk's count finds the T under the mark");
    mark(&mut doc, 0, &[rect], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
}

/// `/Fm1` and `/Fm2`, forms whose own resources lack `/Im1` (object 7, `SECRET_IMAGE_BYTES`),
/// both draw the page's `/Im1`; `/Fm1` under the mark, `/Fm2` (with `fm2_res`) beside it. Object
/// 10 is `extra`.
fn two_forms_drawing_the_pages_image(fm2_res: &str, extra: Vec<Vec<u8>>) -> Document {
    let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
    let form = |res: &str, y: u32| {
        stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {res}"), format!("q 100 0 0 30 20 {y} cm /Im1 Do Q").as_bytes())
    };
    let mut objs = vec![image, form("/Resources << /ProcSet [/PDF] >>", 240), form(fm2_res, 40)];
    objs.extend(extra);
    let mut doc = one_page(b"/Fm1 Do /Fm2 Do", "/XObject << /Im1 7 0 R /Fm1 8 0 R /Fm2 9 0 R >>", objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    doc
}

/// The image a form draws under the mark is cleared there, but another form on the page, with
/// resources of its own that lack the name, still draws the page's image beside the mark: the
/// page's entry stays, and so do the image's bytes, as for a form without resources.
#[test]
fn an_image_another_form_still_draws_through_its_callers_names_stays() {
    // `/Fm2`'s own `/Im1`, where it has one, is a number: no XObject, so the page's is drawn.
    for fm2_res in ["/Resources << /ProcSet [/PDF] >>", "", "/Resources << /XObject << /Im1 5 >> >>"] {
        let mut doc = two_forms_drawing_the_pages_image(fm2_res, vec![]);
        assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1, "/Fm2 {fm2_res:?}");
        assert!(full_save_holds(&doc, b"SECRET_IMAGE_BYTES"), "/Fm2 {fm2_res:?} still draws the image beside the mark");
    }
}

/// A form that gives `/Im1` itself draws its own: the page's image, which only the form under the
/// mark drew, leaves the file, and the other form's own image stays.
#[test]
fn a_forms_own_image_keeps_nothing_of_the_pages() {
    let own = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"PUBLIC_IMAGE_BYTES");
    let mut doc = two_forms_drawing_the_pages_image("/Resources << /XObject << /Im1 10 0 R >> >>", vec![own]);
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(!full_save_holds(&doc, b"SECRET_IMAGE_BYTES"), "nothing else draws the page's image");
    assert!(full_save_holds(&doc, b"PUBLIC_IMAGE_BYTES"), "the other form's own image stays");
}

/// `/FmA`, with marked-content properties of its own, draws `/FmB`, which has no resources and
/// shows `SECRET` in the layer named `/MC0`. The renderer reads `/FmB`'s properties from the page's
/// (ISO 32000-2 §7.8.3), not `/FmA`'s: with the page's `/MC0` on and `/FmA`'s off, the text shows.
/// `page_on` swaps which is which; `fma_names_fmb` has `/FmA` find `/FmB` in its own resources
/// rather than the page's.
fn layered_through_two_forms(page_on: bool, fma_names_fmb: bool, fmb_res: &str) -> Document {
    let (page_mc, fma_mc) = if page_on { (7, 8) } else { (8, 7) };
    let fma_xobject = if fma_names_fmb { "/XObject << /FmB 10 0 R >>" } else { "" };
    let fma = stream(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Properties << /MC0 {fma_mc} 0 R >> {fma_xobject} >>"),
        b"/FmB Do",
    );
    let fmb =
        stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {fmb_res}"), b"/OC /MC0 BDC BT /F1 10 Tf 20 260 Td (SECRET) Tj ET EMC");
    let res = format!("/Properties << /MC0 {page_mc} 0 R >> /XObject << /FmA 9 0 R /FmB 10 0 R >>");
    let mut doc = one_page(b"/FmA Do", &res, vec![b"<< /Type /OCG /Name (On) >>".to_vec(), b"<< /Type /OCG /Name (Off) >>".to_vec(), fma, fmb]);
    doc.update_dict(ObjRef::new(1, 0), |d| {
        d.set(
            b"OCProperties".to_vec(),
            Object::Dict(Dict::from_iter([
                (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                (b"D".to_vec(), Object::Dict(Dict::from_iter([(b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))]))]))),
            ])),
        );
    })
    .unwrap();
    doc
}

/// A form without resources of its own reads its marked-content properties from the page's, as
/// the renderer does, however it is reached: removing hidden layers keeps the text the page's on
/// layer shows, and removes it where the page's layer is off. One whose /Resources isn't a
/// dictionary (a number, `null`, a reference to nothing) reads none, as the renderer does.
#[test]
fn a_form_without_resources_reads_the_pages_properties() {
    use crate::sanitize::{Hidden, remove_hidden};
    for fma_names_fmb in [false, true] {
        let mut doc = layered_through_two_forms(true, fma_names_fmb, "");
        let removed = remove_hidden(&mut doc, &[Hidden::HiddenLayers]);
        assert!(full_save_holds(&doc, b"SECRET"), "/FmA names /FmB itself: {fma_names_fmb}: {removed:?}");
        let mut doc = layered_through_two_forms(false, fma_names_fmb, "");
        let removed = remove_hidden(&mut doc, &[Hidden::HiddenLayers]);
        assert!(!full_save_holds(&doc, b"SECRET"), "/FmA names /FmB itself: {fma_names_fmb}: {removed:?}");
    }
    // A /Resources that isn't a dictionary gives no properties: `/MC0` names no layer, and the
    // text shows whatever the page's says.
    for fmb_res in ["/Resources 5", "/Resources null", "/Resources 99 0 R"] {
        let mut doc = layered_through_two_forms(false, false, fmb_res);
        let removed = remove_hidden(&mut doc, &[Hidden::HiddenLayers]);
        assert!(full_save_holds(&doc, b"SECRET"), "{fmb_res}: {removed:?}");
    }
}

/// The check of what the page still draws decodes each form once, however many scopes of forms
/// with resources of their own it is drawn in: here 22 such forms, two at each of 11 levels, each
/// giving an XObject of its own and drawing both of the next level's and `/R`, which has no
/// resources, put `/R` in 4,094 scopes. `/G` draws the page's image under the mark, so the check
/// runs.
#[test]
fn the_shared_resources_check_decodes_each_form_once() {
    let mut names = vec!["/Im1 7 0 R".to_string(), "/R 8 0 R".to_string(), "/G 9 0 R".to_string()];
    let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
    let g = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] >>", b"q 100 0 0 30 20 240 cm /Im1 Do Q");
    let mut objs = vec![image, stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"0 0 m"), g];
    let levels = 11;
    for level in 0..levels {
        for side in ["A", "B"] {
            let next = if level + 1 < levels { format!("/A{} Do /B{} Do ", level + 1, level + 1) } else { String::new() };
            names.push(format!("/{side}{level} {} 0 R", 7 + objs.len()));
            objs.push(stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] /XObject << /Own 8 0 R >> >>",
                format!("{next}/R Do").as_bytes(),
            ));
        }
    }
    let res = format!("/XObject << {} >>", names.join(" "));
    let mut doc = one_page(b"/G Do /A0 Do /B0 Do", &res, objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    crate::interp::SHARED_FORM_READS.with(|c| c.set(0));
    crate::interp::SHARED_SCOPES.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    // The 22, `/R` and the rewritten `/G`.
    assert_eq!(crate::interp::SHARED_FORM_READS.with(std::cell::Cell::get), 2 * levels + 2, "each form once");
    assert_eq!(crate::interp::SHARED_SCOPES.with(std::cell::Cell::get), (1 << (levels + 1)) - 2, "each form in each of its callers' scopes");
}

/// Page 1 draws `/G`, which draws the page's image under the mark, so the check of what pages
/// sharing a resource dictionary still draw runs. `copies` more pages each hold a copy of the
/// page tree's resources (another dictionary to check), or with `inherit` share them, and draw
/// `direct` names of their own and
/// `/A0 /B0`: 22 forms, two at each of 11 levels, each giving an XObject of its own and drawing
/// both of the next level's and `/R`, which has no resources and draws `drawn` names. Each copy's
/// forms look up 8,186 names in their scopes, and `/R`'s `drawn` in each of 4,094.
fn names_in_many_scopes(drawn: usize, direct: usize, copies: usize, inherit: bool) -> Document {
    let tree = 8 + 2 * copies;
    let mut names = vec!["/Im1 5 0 R".to_string(), "/R 6 0 R".to_string(), "/G 7 0 R".to_string()];
    let mut forms = Vec::new();
    for level in 0..11 {
        for side in ["A", "B"] {
            let next = if level < 10 { format!("/A{} Do /B{} Do ", level + 1, level + 1) } else { String::new() };
            names.push(format!("/{side}{level} {} 0 R", tree + forms.len()));
            forms.push(stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /Own 6 0 R >> >>",
                format!("{next}/R Do").as_bytes(),
            ));
        }
    }
    let res = format!("/XObject << {} >>", names.join(" "));
    let kids: String = (0..=copies).map(|p| format!("{} 0 R ", if p == 0 { 3 } else { 6 + 2 * p })).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} /MediaBox [0 0 300 300] /Resources << {res} >> >>", copies + 1).into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        stream("", b"/G Do"),
        stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", (0..drawn).map(|i| format!("/N{i} Do ")).collect::<String>().as_bytes()),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] >>", b"q 100 0 0 30 20 240 cm /Im1 Do Q"),
    ];
    let content: String = (0..direct).map(|i| format!("/D{i} Do ")).collect::<String>() + "/A0 Do /B0 Do";
    for _ in 0..copies {
        let own = if inherit { String::new() } else { format!("/Resources << {res} >>") };
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R {own} >>", objs.len() + 2).into_bytes());
        objs.push(stream("", content.as_bytes()));
    }
    objs.extend(forms);
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    doc
}

/// The check of what pages sharing a resource dictionary still draw has one allowance of
/// 1,048,576 look-ups in the scopes of forms whose own `/XObject` gives a stream, for the whole
/// operation: past it, what is drawn is unknown, and the redaction is refused rather than guess.
/// Names drawn outside such scopes are bounded by the content that draws them, and not charged.
#[test]
fn the_shared_resources_check_has_one_allowance_of_look_ups() {
    // 8,186 + 4,094 × 200 = 826,986 look-ups.
    assert_eq!(apply(&mut names_in_many_scopes(200, 0, 1, false), None).unwrap().images_removed, 1);
    // Twice that, in two dictionaries; two pages sharing one are read in one pass.
    assert!(matches!(apply(&mut names_in_many_scopes(200, 0, 2, false), None), Err(RedactError::Residue(1))));
    assert_eq!(apply(&mut names_in_many_scopes(200, 0, 2, true), None).unwrap().images_removed, 1);
    // 8,186 + 4,094 × 300 = 1,236,386.
    assert!(matches!(apply(&mut names_in_many_scopes(300, 0, 1, false), None), Err(RedactError::Residue(1))));
    // 8,186 + 4,094 × 250 = 1,031,686, and 20,000 names the page draws itself.
    assert_eq!(apply(&mut names_in_many_scopes(250, 20_000, 1, false), None).unwrap().images_removed, 1);
}

/// Forms nested `levels` deep within `bbox`, `/Fm0` the outermost, each giving `entries` names of
/// its own, of one small image, in its `/XObject` besides the next form, and the innermost showing `SECRET`
/// at 20 250. Each is read with its own names and its callers', so the forms down the chain hold
/// about `entries` × levels² / 2 between them. Objects from 7: the image, then the forms.
fn names_held_down_a_chain(levels: usize, entries: usize, bbox: &str) -> Vec<Vec<u8>> {
    let mut objs = vec![stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x")];
    for level in 0..levels {
        let junk: String = (0..entries).map(|i| format!("/j{level}_{i} 7 0 R ")).collect();
        let (next, content) = if level + 1 < levels {
            (format!("/Fm{} {} 0 R ", level + 1, 9 + level), format!("/Fm{} Do", level + 1))
        } else {
            (String::new(), "BT /F1 10 Tf 20 250 Td (SECRET) Tj ET".to_string())
        };
        objs.push(stream(&format!("/Type /XObject /Subtype /Form /BBox {bbox} /Resources << /XObject << {next}{junk}>> >>"), content.as_bytes()));
    }
    objs
}

/// Each form is read with a copy of its caller's names, its own added, so the forms nested along
/// one path hold at most 16 MiB of resources between them, each entry counted with its name's
/// length and 56 bytes: past that, Apply refuses the page, and the proof's readers of forms say
/// they can't check, rather than hold depth times their size. Six levels of 10,000 names of
/// about 7 bytes hold some 13 MB; eight reach some 17.6 MB at the seventh.
#[test]
fn forms_nested_with_many_names_are_refused_past_the_bound() {
    let page = |levels, bbox, content: &[u8], res: &str, extra: Vec<Vec<u8>>, region: [f64; 4]| {
        let mut objs = names_held_down_a_chain(levels, 10_000, bbox);
        objs.extend(extra);
        let mut doc = one_page(content, res, objs);
        mark(&mut doc, 0, &[region], "");
        doc
    };
    let under = |levels| page(levels, "[0 0 300 300]", b"/Fm0 Do", "/XObject << /Fm0 8 0 R >>", vec![], [0.0, 230.0, 300.0, 280.0]);
    assert_eq!(apply(&mut under(6), None).unwrap().glyphs, 6);
    let refused = apply(&mut under(8), None).unwrap_err().to_string();
    assert!(refused.contains("hold more than 16 MiB of resources"), "{refused}");
    let text = crate::tests_verify::prove_unapplied(&mut under(8)).to_text();
    assert!(text.contains("the forms under the region hold too many resources to check"), "{text}");
    // Beside the region, only the page's text is read through them.
    let beside = |levels| page(levels, "[0 0 100 100]", b"/Fm0 Do", "/XObject << /Fm0 8 0 R >>", vec![], [200.0, 200.0, 300.0, 300.0]);
    let text = crate::tests_verify::prove_unapplied(&mut beside(6)).to_text();
    assert!(!text.contains("page 1 can't be read"), "{text}");
    let text = crate::tests_verify::prove_unapplied(&mut beside(8)).to_text();
    assert!(text.contains("page 1 can't be read"), "{text}");
    // Drawn by a pattern cell, they are searched for images through their names as given, none of
    // them copied, so the bound doesn't apply.
    let cell = stream(
        "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << /XObject << /Fm0 8 0 R >> >>",
        b"/Fm0 Do",
    );
    let mut patterned =
        page(8, "[0 0 300 300]", b"/Pattern cs /P0 scn 0 0 300 300 re f", "/Pattern << /P0 16 0 R >>", vec![cell], [0.0, 230.0, 300.0, 280.0]);
    let text = crate::tests_verify::prove_unapplied(&mut patterned).to_text();
    assert!(!text.contains("a pattern, soft mask or Type3 glyph"), "{text}");
    // A form without resources of its own is read with its caller's, here a page `/XObject`, given
    // by reference, of `names` names `len` bytes long: ten nested with 15,000 short ones hold some
    // 9 MB, twenty some 19 MB; with 2,000 of 1,000 bytes, four hold some 8 MB, ten some 21 MB.
    let without_resources = |levels: usize, names: usize, len: usize, extra_res: &str| {
        let mut objs = vec![stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x")];
        let forms: String = (0..levels).map(|i| format!("/Fm{i} {} 0 R ", 9 + i)).collect();
        let pad = "k".repeat(len.saturating_sub(5));
        let junk: String = (levels..names).map(|i| format!("/{pad}{i:05} 7 0 R ")).collect();
        objs.push(format!("<< {forms}{junk}>>").into_bytes());
        for level in 0..levels {
            let content = if level + 1 < levels { format!("/Fm{} Do", level + 1) } else { "BT /F1 10 Tf 20 50 Td (PUBLIC) Tj ET".to_string() };
            objs.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", content.as_bytes()));
        }
        let mut doc = one_page(b"/Fm0 Do", &format!("/XObject 8 0 R {extra_res}"), objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        doc
    };
    apply(&mut without_resources(10, 15_000, 6, ""), None).unwrap();
    let refused = apply(&mut without_resources(20, 15_000, 6, ""), None).unwrap_err().to_string();
    assert!(refused.contains("hold more than 16 MiB of resources"), "{refused}");
    apply(&mut without_resources(4, 2_000, 1_000, ""), None).unwrap();
    let refused = apply(&mut without_resources(10, 2_000, 1_000, ""), None).unwrap_err().to_string();
    assert!(refused.contains("hold more than 16 MiB of resources"), "{refused}");
    // Keys of their own count too: 2,000 of 1,000 bytes beside the categories, each given a number.
    let keys: String = (0..2_000).map(|i| format!("/{}{i:05} 0 ", "x".repeat(995))).collect();
    apply(&mut without_resources(4, 20, 6, &keys), None).unwrap();
    let refused = apply(&mut without_resources(10, 20, 6, &keys), None).unwrap_err().to_string();
    assert!(refused.contains("hold more than 16 MiB of resources"), "{refused}");
    // Only the categories a lookup copies are read to be weighed: what else the resources give by
    // reference stays unread, however often a form is drawn.
    let held_reads = |extra_res: &str| {
        crate::interp::HELD_READS.with(|c| c.set(0));
        apply(&mut without_resources(10, 15_000, 6, extra_res), None).unwrap();
        crate::interp::HELD_READS.with(std::cell::Cell::get)
    };
    // Nor are the colour spaces, shadings and properties: no copy holds them, as nothing looks
    // their names up in a form's caller's.
    assert_eq!(held_reads("/ProcSet 7 0 R /Other 7 0 R /ColorSpace 7 0 R /Shading 7 0 R /Properties 7 0 R"), held_reads(""));
    // Forms side by side hold their names one at a time: twenty under the mark, each read with the
    // page's 15,000 names, are walked, and with a soft mask's group of the page's drawing them
    // too, the proof's readers check them all. Holding them all at once would be some 18 MB.
    let side_by_side = |extra_res: &str, extra: Vec<Vec<u8>>| {
        let form =
            || stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] >>", b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET");
        let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x");
        let mut objs = vec![image];
        objs.extend((0..20).map(|_| form()));
        objs.extend(extra);
        let forms: String = (0..20).map(|i| format!("/S{i} {} 0 R ", 8 + i)).collect();
        let junk: String = (0..14_980).map(|i| format!("/k{i} 7 0 R ")).collect();
        let content: String = (0..20).map(|i| format!("/S{i} Do ")).collect();
        let mut doc = one_page(content.as_bytes(), &format!("/XObject << {forms}{junk}>> {extra_res}"), objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        doc
    };
    assert_eq!(apply(&mut side_by_side("", vec![]), None).unwrap().glyphs, 120);
    let forms: String = (0..20).map(|i| format!("/S{i} {} 0 R ", 8 + i)).collect();
    let content: String = (0..20).map(|i| format!("/S{i} Do ")).collect();
    let group = stream(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency >> /Resources << /XObject << {forms}>> >>"),
        content.as_bytes(),
    );
    let text =
        crate::tests_verify::prove_unapplied(&mut side_by_side("/ExtGState << /GS0 << /SMask << /S /Luminosity /G 28 0 R >> >> >>", vec![group]))
            .to_text();
    for refusal in ["hold too many resources", "page 1 can't be read", "can't be read in full"] {
        assert!(!text.contains(refusal), "{refusal}: {text}");
    }
}

/// The check of what pages sharing a resource dictionary still draw reads the own `/XObject` of a
/// form they draw again for each dictionary it checks, and charges its entries to the operation's
/// allowance of 1,048,576. Page 1 draws `/Im1` under the mark; `holders` more pages each hold
/// object 6 as their resources (each a dictionary to check, naming `/Im1`) and draw `/F`, whose own
/// `/XObject` gives 100,000 streams.
fn own_names_read_for_each_holder(holders: usize) -> Document {
    let names: String = (0..100_000).map(|i| format!("/j{i} 10 0 R ")).collect();
    let kids: String = (0..=holders).map(|p| format!("{} 0 R ", if p == 0 { 3 } else { 9 + 2 * p })).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} /MediaBox [0 0 300 300] >>", holders + 1).into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 7 0 R >> >> >>".to_vec(),
        stream("", b"q 100 0 0 30 20 240 cm /Im1 Do Q"),
        b"null".to_vec(),
        b"<< /XObject << /Im1 7 0 R /F 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject 9 0 R >>", b"0 0 m"),
        format!("<< {names}>>").into_bytes(),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
    ];
    for _ in 0..holders {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources 6 0 R >>", objs.len() + 2).into_bytes());
        objs.push(stream("", b"/F Do"));
    }
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    doc
}

/// Ten pages read 1,000,000 entries, within the allowance; eleven don't fit, and the redaction is
/// refused rather than guess.
#[test]
fn the_shared_resources_check_charges_the_own_names_it_reads() {
    assert_eq!(apply(&mut own_names_read_for_each_holder(10), None).unwrap().images_removed, 1);
    assert!(matches!(apply(&mut own_names_read_for_each_holder(11), None), Err(RedactError::Residue(1))));
}

/// A form whose own `/XObject` names 2,000 objects, each an object of its own: `one_page`'s
/// objects from 7, the form, its `/XObject`, then the objects.
fn a_form_naming_many_objects() -> Vec<Vec<u8>> {
    let names: String = (0..2_000).map(|i| format!("/e{i} {} 0 R ", 9 + i)).collect();
    let mut objs =
        vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject 8 0 R >>", b""), format!("<< {names}>>").into_bytes()];
    objs.extend((0..2_000).map(|_| b"0".to_vec()));
    objs
}

/// How many objects reading a form's names takes from the file, while `doc` is redacted.
fn entry_reads(mut doc: Document) -> usize {
    crate::interp::ENTRY_READS.with(|c| c.set(0));
    apply(&mut doc, None).unwrap();
    crate::interp::ENTRY_READS.with(std::cell::Cell::get)
}

/// What each object a form's resources name is, is read from the file once for a page by the walk
/// and each of the proof's readers, however many times the form is drawn, and once for the check
/// of what pages sharing a resource dictionary still draw, however many dictionaries it checks:
/// kept in object streams the cache can't hold, each read can cost decoding one.
#[test]
fn the_objects_a_forms_names_give_are_read_once() {
    let drawn = |draws: usize| {
        let content: String = (0..draws).map(|_| "/F Do ").collect();
        let mut doc = one_page(content.as_bytes(), "/XObject << /F 7 0 R >>", a_form_naming_many_objects());
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        doc
    };
    let once = entry_reads(drawn(1));
    assert!(once >= 2_000, "{once}");
    assert_eq!(entry_reads(drawn(20)), once, "twenty draws");
    // The proof read alone, its text extraction among its readers.
    let proof_reads = |mut doc: Document| {
        crate::interp::ENTRY_READS.with(|c| c.set(0));
        crate::tests_verify::prove_unapplied(&mut doc);
        crate::interp::ENTRY_READS.with(std::cell::Cell::get)
    };
    let once = proof_reads(drawn(1));
    assert!(once >= 2_000, "{once}");
    assert_eq!(proof_reads(drawn(20)), once, "twenty draws, the proof");
    // A form painting beside the mark with a pattern of its own, which isn't passed to the page.
    let painting = |draws: usize| {
        let content: String = (0..draws).map(|_| "/F Do ").collect();
        let pattern = b"<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >> >> >>";
        let objs = vec![
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Pattern << /P0 8 0 R >> >>",
                b"/Pattern cs /P0 scn 0 0 10 10 re f",
            ),
            pattern.to_vec(),
        ];
        let mut doc = one_page(content.as_bytes(), "/XObject << /F 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        doc
    };
    let once = entry_reads(painting(1));
    assert!(once >= 1, "{once}");
    assert_eq!(entry_reads(painting(20)), once, "twenty draws painting");
    // Page 1 draws `/Im1` under the mark; `holders` more pages each hold object 6 as their
    // resources, naming `/Im1`, and draw the form, and `/Im1` beside the mark, which is no form to
    // decode.
    let holders = |holders: usize| {
        let kids: String = (0..=holders).map(|p| format!("{} 0 R ", if p == 0 { 3 } else { 2_007 + 2 * p })).collect();
        let mut objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [{kids}] /Count {} /MediaBox [0 0 300 300] >>", holders + 1).into_bytes(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >>".to_vec(),
            stream("", b"q 100 0 0 30 20 240 cm /Im1 Do Q"),
            stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES"),
            b"<< /XObject << /Im1 5 0 R /F 7 0 R >> >>".to_vec(),
        ];
        objs.extend(a_form_naming_many_objects());
        for _ in 0..holders {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources 6 0 R >>", objs.len() + 2).into_bytes());
            objs.push(stream("", b"/F Do q 100 0 0 30 20 40 cm /Im1 Do Q"));
        }
        let mut doc = pdf(objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        doc
    };
    let once = entry_reads(holders(1));
    assert!(once >= 2_000, "{once}");
    crate::interp::SHARED_FORM_READS.with(|c| c.set(0));
    assert_eq!(entry_reads(holders(10)), once, "ten dictionaries");
    assert_eq!(crate::interp::SHARED_FORM_READS.with(std::cell::Cell::get), 10, "the form, once for each dictionary");
}

/// The proof reads each tiling pattern a page gives, for its text and for images, where the page
/// gives it: a form or pattern read with a copy of the page's names doesn't read them again. Eight
/// patterns with resources of their own, and a form with resources drawn once or 50 times, are
/// read as often either way (before, every pattern read the others again: 8! times). One given
/// under 50 names is read for its text, and looked at for images, as often as under one.
#[test]
fn the_proof_reads_the_patterns_a_page_gives_once() {
    let read = |draws: usize| {
        let cell = || {
            stream(
                "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /ProcSet [/PDF] >>",
                b"q Q",
            )
        };
        let mut objs = vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] >>", b"q Q")];
        objs.extend((0..8).map(|_| cell()));
        let patterns: String = (0..8).map(|i| format!("/P{i} {} 0 R ", 8 + i)).collect();
        let content: String = (0..draws).map(|_| "/F Do ").collect();
        let mut doc = one_page(content.as_bytes(), &format!("/XObject << /F 7 0 R >> /Pattern << {patterns}>>"), objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::verify::PATTERN_READS.with(|c| c.set(0));
        crate::verify::HIDDEN_LOOKS.with(|c| c.set(0));
        crate::tests_verify::prove_unapplied(&mut doc);
        (crate::verify::PATTERN_READS.with(std::cell::Cell::get), crate::verify::HIDDEN_LOOKS.with(std::cell::Cell::get))
    };
    let once = read(1);
    assert!(once.0 >= 8 && once.1 >= 8, "{once:?}");
    assert_eq!(read(50), once);
    let named = |count: usize| {
        let cell = stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /ProcSet [/PDF] >>",
            b"q Q",
        );
        let names: String = (0..count).map(|i| format!("/P{i} 7 0 R ")).collect();
        let mut doc = one_page(b"", &format!("/Pattern << {names}>>"), vec![cell]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::verify::PATTERN_READS.with(|c| c.set(0));
        crate::verify::HIDDEN_LOOKS.with(|c| c.set(0));
        crate::tests_verify::prove_unapplied(&mut doc);
        (crate::verify::PATTERN_READS.with(std::cell::Cell::get), crate::verify::HIDDEN_LOOKS.with(std::cell::Cell::get))
    };
    let once = named(1);
    assert!(once.0 >= 1 && once.1 >= 1, "{once:?}");
    assert_eq!(named(50), once, "one pattern under fifty names");
}

/// An annotation's appearance is searched for images where its own resources give them, as before:
/// one whose own pattern cell draws an image under the region is no image the proof can place.
#[test]
fn the_proof_searches_an_appearances_own_patterns() {
    let objs = vec![
        b"<< /Type /Annot /Subtype /Square /Rect [0 0 300 300] /AP << /N 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Pattern << /P0 9 0 R >> >>", b"0 0 m"),
        stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << /XObject << /Im 10 0 R >> >>",
            b"/Im Do",
        ),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
    ];
    let mut doc = one_page(b"", "", objs);
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0))]))).unwrap();
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
    assert!(text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"), "{text}");
}

/// Forms whose own resources give no XObject stream look every name up as their caller does, and
/// forms that give the same `/XObject` dictionary (by reference, or through the same `/Resources`)
/// look names up alike: neither opens a scope of its own in the check of what the page still
/// draws, so a page of many such forms is read in full rather than refused. `/P` gives only a
/// `/ProcSet` and `/Q` an empty `/XObject`; `/S1`, `/S2` and `/S3` give object 12, and `/S4` and
/// `/S5` object 17's: two scopes.
#[test]
fn forms_looking_names_up_alike_share_a_scope() {
    let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
    let g = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ProcSet [/PDF] >>", b"q 100 0 0 30 20 240 cm /Im1 Do Q");
    let form = |res: &str| stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources {res}"), b"/R Do");
    let objs = vec![
        image,
        g,
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"0 0 m"),
        form("<< /ProcSet [/PDF] >>"),
        form("<< /XObject << >> >>"),
        b"<< /Own 9 0 R >>".to_vec(),
        form("<< /XObject 12 0 R >>"),
        form("<< /XObject 12 0 R >>"),
        b"<< /XObject 12 0 R >>".to_vec(),
        form("15 0 R"),
        b"<< /XObject << /Own 9 0 R >> >>".to_vec(),
        form("17 0 R"),
        form("17 0 R"),
    ];
    let res = "/XObject << /Im1 7 0 R /G 8 0 R /R 9 0 R /P 10 0 R /Q 11 0 R /S1 13 0 R /S2 14 0 R /S3 16 0 R /S4 18 0 R /S5 19 0 R >>";
    let mut doc = one_page(b"/G Do /P Do /Q Do /S1 Do /S2 Do /S3 Do /S4 Do /S5 Do", res, objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    crate::interp::SHARED_SCOPES.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(!full_save_holds(&doc, b"SECRET_IMAGE_BYTES"));
    assert_eq!(crate::interp::SHARED_SCOPES.with(std::cell::Cell::get), 2);
}

/// A form's own font entry that is a stream is no font dictionary: the renderer looks the name up
/// in the page's, whose glyphs are 20 points wide, and the `T` lies under the mark. (Read as the
/// stream's dictionary, a font without widths, it would lie well before it.)
#[test]
fn a_forms_own_font_given_as_a_stream_is_looked_up_in_its_callers() {
    let mut doc = wide_text_through_a_form();
    let font = Dict::from_iter([
        (b"Type".to_vec(), Object::Name(b"Font".to_vec())),
        (b"Subtype".to_vec(), Object::Name(b"TrueType".to_vec())),
        (b"BaseFont".to_vec(), Object::Name(b"Arial".to_vec())),
    ]);
    let font = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(font, Vec::new())));
    let own = Dict::from_iter([(b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"F2".to_vec(), Object::Ref(font))])))]);
    doc.update_dict(ObjRef::new(5, 0), |d| d.set(b"Resources".to_vec(), Object::Dict(own))).unwrap();
    assert_eq!(under(&mut doc, 0, &[[121.0, 245.0, 139.0, 262.0]]), 1, "the walk places the T with the page's font");
}

/// A form without resources of its own, drawn by a form with resources of its own, is read with
/// its callers' names but written back with its caller's resources as written, as on `main`, not
/// with a copy of every name the page gives at each rewrite. The page gives 1,001 fonts by
/// reference; a form giving one of its own draws, ten times, a form without resources showing
/// text under the mark with the page's `/F0`. Written back with the lookup, the saved file held
/// ten copies of the page's fonts (115 KB).
#[test]
fn a_form_without_resources_is_written_back_with_its_callers_resources_as_written() {
    let fonts: String = std::iter::once("/F0 5 0 R ".to_string()).chain((0..1000).map(|i| format!("/K{i} 5 0 R "))).collect();
    let objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font 8 0 R /XObject << /F1 9 0 R /F2 10 0 R >> >> >>"
            .to_vec(),
        stream("", b"/F1 Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< >>".to_vec(),
        format!("<< {fonts}>>").into_bytes(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /G 5 0 R >> /XObject << /F2 10 0 R >> >>",
            "/F2 Do\n".repeat(10).as_bytes(),
        ),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT /F0 10 Tf 100 100 Td (SECRET) Tj ET"),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]], "");
    assert!(apply(&mut doc, None).unwrap().glyphs > 0, "the text under the mark is placed with the page's font and removed");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap().len();
    assert!(saved < 30_000, "{saved} bytes");
}

/// A form without resources of its own is written back with the resources of what draws it, as
/// written: inside a form with resources of its own, that form's, whose `/F0` (object 7) shadows the
/// page's (object 5); drawn by the page after that form, the page's.
#[test]
fn a_form_without_resources_takes_the_resources_of_what_draws_it() {
    let objs = vec![
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F0 7 0 R >> /XObject << /F2 9 0 R >> >>", b"/F2 Do"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT /F0 10 Tf 100 100 Td (SECRET) Tj ET"),
    ];
    let mut doc = one_page(b"/F1 Do /F2 Do", "/Font << /F0 5 0 R >> /XObject << /F1 8 0 R /F2 9 0 R >>", objs);
    mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]], "");
    assert!(apply(&mut doc, None).unwrap().glyphs > 0);
    // The `/F0` each form's resources give, by how deep it is drawn.
    let mut found: Vec<(usize, Option<ObjRef>)> = Vec::new();
    let page = pdfcraft_model::pages(&doc)[0].dict.clone();
    let mut pending: Vec<(usize, Dict)> =
        page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).map(|r| (0, r)).into_iter().collect();
    while let Some((depth, res)) = pending.pop() {
        let xobjects = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
        for (_, value) in xobjects.iter() {
            let form = doc.resolve(value);
            let Object::Stream(s) = &*form else { continue };
            let Some(own) = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()) else { continue };
            let font = own.get(b"Font").and_then(|f| doc.resolve(f).as_dict().cloned()).and_then(|f| f.get(b"F0").and_then(Object::as_ref));
            found.push((depth + 1, font));
            if depth < 2 {
                pending.push((depth + 1, own));
            }
        }
    }
    let at = |depth: usize| -> Vec<u32> {
        let mut nums: Vec<u32> = found.iter().filter(|(d, _)| *d == depth).filter_map(|(_, f)| f.map(|f| f.num)).collect();
        nums.sort_unstable();
        nums.dedup();
        nums
    };
    assert_eq!(at(1), vec![5, 7], "drawn by the page: the form with its own, and the rewritten one with the page's: {found:?}");
    assert_eq!(at(2), vec![7], "drawn inside the form with its own: that form's: {found:?}");
}

/// A soft mask's group without resources, given by a form's own `/ExtGState` and drawing the
/// page's image, draws it on screen: the renderer looks the group's names up in those of the
/// content using the soft mask, the form's own then the page's. The proof searches the group so
/// too, and says it can't place the image. Where the form's own `/XObject` gives `/Im` as a form
/// drawing nothing, there is no image to find.
#[test]
fn the_proof_searches_a_forms_own_soft_mask_through_its_callers_names() {
    let prove = |form_res: &str| {
        let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x");
        let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << {form_res} >>"), b"/GS1 gs 0 0 300 300 re f");
        let group = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >>", b"/Im Do");
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let mut doc = one_page(b"/Fm Do", "/XObject << /Im 7 0 R /Fm 8 0 R >>", vec![image, form, group, empty]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::tests_verify::prove_unapplied(&mut doc).to_text()
    };
    let soft_mask = "/ExtGState << /GS1 << /SMask << /S /Luminosity /G 9 0 R >> >> >>";
    let text = prove(soft_mask);
    assert!(text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"), "{text}");
    let text = prove(&format!("{soft_mask} /XObject << /Im 10 0 R >>"));
    assert!(!text.contains("an image inside a pattern, soft mask or Type3 glyph"), "{text}");
}

/// Two forms, each giving the same soft mask in its own `/ExtGState`, whose group without
/// resources draws `/Im`: in `/A`, whose own `/XObject` gives `/Im` as a form drawing nothing, the
/// group draws no image; in `/B`, which gives no `/XObject`, it draws the page's. Searched in `/A`
/// first, the group is searched again in `/B`, whichever is drawn first; drawn by `/A` alone, it
/// draws no image. So too where the group draws, through its own `/XObject`, a form without
/// resources that draws `/Im`.
#[test]
fn a_soft_mask_drawn_through_two_forms_names_is_searched_in_each() {
    let prove = |content: &[u8], group_res: &str, group_content: &[u8]| {
        let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x");
        let soft_mask = "/ExtGState << /GS1 << /SMask << /S /Luminosity /G 10 0 R >> >> >>";
        let form =
            |res: &str| stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << {res} >>"), b"/GS1 gs 0 0 300 300 re f");
        let a = form(&format!("{soft_mask} /XObject << /Im 11 0 R >>"));
        let b = form(soft_mask);
        let group = stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> {group_res}"),
            group_content,
        );
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let inner = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/Im Do");
        let mut doc = one_page(content, "/XObject << /Im 7 0 R /A 8 0 R /B 9 0 R >>", vec![image, a, b, group, empty, inner]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::tests_verify::prove_unapplied(&mut doc).to_text()
    };
    let flagged = "an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared";
    for (group_res, group_content) in [("", &b"/Im Do"[..]), ("/Resources << /XObject << /X 12 0 R >> >>", b"/X Do")] {
        for content in [&b"/A Do /B Do"[..], b"/B Do /A Do"] {
            let text = prove(content, group_res, group_content);
            assert!(text.contains(flagged), "{text}");
        }
        let text = prove(b"/A Do", group_res, group_content);
        assert!(!text.contains(flagged), "{text}");
    }
}

/// A soft mask's group is searched once however many forms with `/XObject` names of their own
/// give it, where every name it draws is its own; where it looks a name up through theirs, once in
/// each, and past 4,096 such searches the proof says it can't check.
#[test]
fn hidden_content_is_searched_again_only_where_it_reads_its_callers_names() {
    let searches = |forms: usize, group_res: &str| {
        let soft_mask = "/ExtGState << /GS1 << /SMask << /S /Luminosity /G 8 0 R >> >> >>";
        let mut objs = vec![
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
            stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> {group_res}"),
                b"/Im Do",
            ),
        ];
        objs.extend((0..forms).map(|_| {
            stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << {soft_mask} /XObject << /Im 7 0 R >> >>"),
                b"/GS1 gs 0 0 300 300 re f",
            )
        }));
        let names: String = (0..forms).map(|i| format!("/F{i} {} 0 R ", 9 + i)).collect();
        let content: String = (0..forms).map(|i| format!("/F{i} Do ")).collect();
        let mut doc = one_page(content.as_bytes(), &format!("/XObject << {names}>>"), objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::verify::HIDDEN_SEARCHES.with(|c| c.set(0));
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        (crate::verify::HIDDEN_SEARCHES.with(std::cell::Cell::get), text)
    };
    let own = "/Resources << /XObject << /Im 7 0 R >> >>";
    // The group and the form it draws, once each.
    assert_eq!(searches(1, own).0, 2);
    assert_eq!(searches(50, own).0, 2);
    // Read through each form's names: the group once in each, the form it draws once.
    assert_eq!(searches(1, "").0, 2);
    assert_eq!(searches(50, "").0, 51);
    let (_, text) = searches(4_097, "");
    assert!(!text.contains("can't be read in full"), "{text}");
    let (_, text) = searches(4_098, "");
    assert!(text.contains("a pattern, soft mask or Type3 glyph can't be read in full"), "{text}");
}

/// The page's soft mask's group looks the XObjects it draws up in its own resources first, an
/// entry there that is no stream passed over, as the renderer does: given its own `/Im` as a
/// number, it draws the page's image; given it as a form drawing nothing, no image. A group that
/// draws itself through its own names is searched once, as before, not refused.
#[test]
fn the_proof_looks_a_hidden_groups_names_up_as_the_renderer_does() {
    let prove = |group_res: &str, content: &[u8]| {
        let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x");
        let group =
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> {group_res}"), content);
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let res = "/XObject << /Im 7 0 R >> /ExtGState << /GS1 << /SMask << /S /Luminosity /G 8 0 R >> >> >>";
        let mut doc = one_page(b"/GS1 gs 0 0 300 300 re f", res, vec![image, group, empty]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::tests_verify::prove_unapplied(&mut doc).to_text()
    };
    let flagged = "an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared";
    let text = prove("/Resources << /XObject << /Im 5 >> >>", b"/Im Do");
    assert!(text.contains(flagged), "{text}");
    let text = prove("/Resources << /XObject << /Im 9 0 R >> >>", b"/Im Do");
    assert!(!text.contains(flagged), "{text}");
    let text = prove("/Resources << /XObject << /Self 8 0 R >> >>", b"/Self Do");
    assert!(!text.contains("a pattern, soft mask or Type3 glyph"), "{text}");
}

/// An appearance with resources of its own gives the page's pattern, whose cell draws `/Im`
/// through the names of what paints with it: the page gives no `/Im`, the appearance gives an
/// image. Searched with the page's names first, the cell is searched again with the appearance's,
/// and the image found.
#[test]
fn the_proof_searches_a_pattern_again_with_an_appearances_names() {
    let objs = vec![
        b"<< /Type /Annot /Subtype /Square /Rect [0 0 300 300] /AP << /N 9 0 R >> >>".to_vec(),
        stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << >>", b"/Im Do"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Pattern << /P0 8 0 R >> /XObject << /Im 10 0 R >> >>", b"0 0 m"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
    ];
    let mut doc = one_page(b"", "/Pattern << /P0 8 0 R >>", objs);
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0))]))).unwrap();
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
    assert!(text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"), "{text}");
}

/// A Type 3 glyph looks the XObjects it draws up in its own resources, then in its font's
/// `/Resources`, then in those of the content whose resources give the font, as the renderer
/// does where that content shows it: the font's
/// `/Im`, given directly or through `/Resources` by reference, is an image the proof can't place;
/// the font's `/Im` given as a form drawing nothing hides the page's image, and the glyph's own
/// hides the font's. A glyph two fonts share is searched with each font's names.
#[test]
fn the_proof_searches_a_type3_glyph_through_its_fonts_names() {
    let prove_with = |fonts: &str, font_res: &str, glyph_res: &str, page_im: &str| {
        let form = stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << {fonts} >> >>"),
            b"BT /T3 100 Tf 10 250 Td (a) Tj ET",
        );
        let font = format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 9 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] {font_res} >>"
        );
        let glyph = stream(glyph_res, b"1000 0 d0 /Im Do");
        let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x");
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let other = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 9 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources 12 0 R >>";
        let objs = vec![form, font.into_bytes(), glyph, image, empty, b"<< /XObject << /Im 10 0 R >> >>".to_vec(), other.as_bytes().to_vec()];
        let mut doc = one_page(b"/Fm Do", &format!("/XObject << /Fm 7 0 R {page_im} >>"), objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::tests_verify::prove_unapplied(&mut doc).to_text()
    };
    let prove = |font_res: &str, glyph_res: &str, page_im: &str| prove_with("/T3 8 0 R", font_res, glyph_res, page_im);
    let flagged = "an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared";
    let text = prove("/Resources << /XObject << /Im 10 0 R >> >>", "", "");
    assert!(text.contains(flagged), "{text}");
    let text = prove("/Resources 12 0 R", "", "");
    assert!(text.contains(flagged), "{text}");
    let text = prove("/Resources << /XObject << /Im 11 0 R >> >>", "", "/Im 10 0 R");
    assert!(!text.contains(flagged), "{text}");
    let text = prove("/Resources << /XObject << /Im 10 0 R >> >>", "/Resources << /XObject << /Im 11 0 R >> >>", "");
    assert!(!text.contains(flagged), "{text}");
    // `/T3`'s `/Im` draws nothing, `/T4`'s (object 13) is the image.
    let text = prove_with("/T3 8 0 R /T4 13 0 R", "/Resources << /XObject << /Im 11 0 R >> >>", "", "");
    assert!(text.contains(flagged), "{text}");
}

/// A Type 3 font written directly in a form's `/Font`, its glyph drawing a form its font's
/// `/XObject` gives: drawn 10 times, the glyph is searched as often as when the font is an object
/// of its own, and drawn 4,100 times, the proof checks it rather than run out of searches, also
/// with a `null` entry beside `/X`. Two
/// such fonts giving `/X` as different objects are told apart: where one is an image, it is found.
#[test]
fn a_type3_font_written_in_place_is_searched_once() {
    let searches = |font: &str, draws: usize| {
        let form = stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /T3 {font} >> >>"),
            b"BT /T3 100 Tf 10 250 Td (a) Tj ET",
        );
        let glyph = stream("", b"1000 0 d0 /X Do");
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let objs = vec![form, glyph, empty, type3_font_drawing_x().into_bytes()];
        let mut doc = one_page("/Fm Do ".repeat(draws).as_bytes(), "/XObject << /Fm 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::verify::HIDDEN_SEARCHES.with(|c| c.set(0));
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        (crate::verify::HIDDEN_SEARCHES.with(std::cell::Cell::get), text)
    };
    let direct = type3_font_drawing_x();
    assert_eq!(searches(&direct, 10).0, searches("10 0 R", 10).0);
    // An entry given as null names nothing (ISO 32000-2 §7.3.7), and changes nothing.
    for font in [direct.clone(), direct.replace("/X 9 0 R", "/X 9 0 R /Y null")] {
        assert_eq!(searches(&font, 1).0, searches(&font, 10).0);
        let (_, text) = searches(&font, 4_100);
        assert!(!text.contains("can't be read in full"), "{text}");
    }
    let form = |font: &str| {
        stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /T3 {font} >> >>"),
            b"BT /T3 100 Tf 10 250 Td (a) Tj ET",
        )
    };
    let objs = vec![
        form(&direct),
        stream("", b"1000 0 d0 /X Do"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
        form(&direct.replace("/X 9 0 R", "/X 10 0 R")),
    ];
    let mut doc = one_page(b"/Fm Do /Gm Do", "/XObject << /Fm 7 0 R /Gm 11 0 R >>", objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
    assert!(text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"), "{text}");
}

/// A form gives 2,000 names of Type 3 fonts whose `/XObject` gives 2,001 names: all of one font
/// object, which writes them in its own `/Resources`, directly or through a reference to it each;
/// or each its own font, all giving the same `/Resources`, or the same `/XObject`, directly or
/// through a reference to it each. The image search holds those names once for the object they
/// are written in, and searches the glyph the fonts share once, as for one name; one font object
/// is read once, however many names give it.
#[test]
fn the_proof_holds_a_type3_fonts_names_once_however_many_names_give_them() {
    const NAMES: usize = 2_000;
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Given {
        OneFont,
        OneFontThroughReferences,
        Resources,
        ResourcesThroughReferences,
        XObject,
        XObjectThroughReferences,
    }
    let looks = |given: Given, count: usize| {
        // Object 13 + i: font i, or a reference to font 11; 13 + count + i: a reference to 10 or 12.
        let font_of = |i: usize| if given == Given::OneFont { 11 } else { 13 + i };
        let fonts: String = (0..count).map(|i| format!("/T{i} {} 0 R ", font_of(i))).collect();
        let form = stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << {fonts}>> >>"),
            b"BT /T0 100 Tf 10 250 Td (a) Tj ET",
        );
        let names: String = (0..NAMES).map(|i| format!("/X{i} 9 0 R ")).collect();
        let font = |resources: &str| {
            format!("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] {resources} >>")
                .into_bytes()
        };
        let mut objs = vec![
            form,
            stream("", b"1000 0 d0 /X Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
            format!("<< /XObject << /X 9 0 R {names}>> >>").into_bytes(),
            font(&format!("/Resources << /XObject << /X 9 0 R {names}>> >>")),
            format!("<< /X 9 0 R {names}>>").into_bytes(),
        ];
        let reference = |i: usize| 13 + count + i;
        objs.extend((0..count).filter(|_| given != Given::OneFont).map(|i| match given {
            Given::OneFont | Given::OneFontThroughReferences => b"11 0 R".to_vec(),
            Given::Resources => font("/Resources 10 0 R"),
            Given::ResourcesThroughReferences => font(&format!("/Resources {} 0 R", reference(i))),
            Given::XObject => font("/Resources << /XObject 12 0 R >>"),
            Given::XObjectThroughReferences => font(&format!("/Resources << /XObject {} 0 R >>", reference(i))),
        }));
        objs.extend((0..count).filter_map(|_| match given {
            Given::ResourcesThroughReferences => Some(b"10 0 R".to_vec()),
            Given::XObjectThroughReferences => Some(b"12 0 R".to_vec()),
            _ => None,
        }));
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::verify::HIDDEN_SEARCHES.with(|c| c.set(0));
        crate::verify::HIDDEN_LOOKS.with(|c| c.set(0));
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        assert!(!text.contains("can't be placed or cleared"), "{given:?}: {text}");
        (crate::verify::HIDDEN_SEARCHES.with(std::cell::Cell::get), crate::verify::HIDDEN_LOOKS.with(std::cell::Cell::get))
    };
    for given in [
        Given::OneFont,
        Given::OneFontThroughReferences,
        Given::Resources,
        Given::ResourcesThroughReferences,
        Given::XObject,
        Given::XObjectThroughReferences,
    ] {
        let (searches, looked) = looks(given, 1);
        assert!(searches >= 1, "{given:?}");
        let many = looks(given, NAMES);
        assert_eq!(many.0, searches, "{given:?}");
        // One font object is read once, however many names give it.
        if matches!(given, Given::OneFont | Given::OneFontThroughReferences) {
            assert_eq!(many.1, looked, "{given:?}");
        }
    }
}

/// A form gives 2,000 Type 3 fonts that share one `/CharProcs` of ten glyphs, each drawing `/X`,
/// and give the same `/XObject` names, written in the font: fonts written in the form's `/Font`, or
/// font objects of their own, or fonts each giving a `/Resources` object of its own. The glyphs
/// are read and searched as often as for one font, and the proof checks them.
#[test]
fn the_proof_reads_glyphs_fonts_with_the_same_names_share_once() {
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Written {
        InPlace,
        Objects,
        Resources,
    }
    let read = |written: Written, count: usize| {
        let font = |resources: &str| {
            format!(
                "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs 18 0 R /Encoding << /Type /Encoding /Differences [97 /g0] >> /FirstChar 97 /LastChar 97 /Widths [1000] {resources} >>"
            )
        };
        let names = "/Resources << /XObject << /X 19 0 R >> >>";
        let fonts: String = (0..count)
            .map(|i| match written {
                Written::InPlace => format!("/T{i} {} ", font(names)),
                Written::Objects | Written::Resources => format!("/T{i} {} 0 R ", 20 + i),
            })
            .collect();
        let mut objs = vec![stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << {fonts}>> >>"),
            b"BT /T0 100 Tf 10 250 Td (a) Tj ET",
        )];
        objs.extend((0..10).map(|_| stream("", b"1000 0 d0 /X Do")));
        let glyphs: String = (0..10).map(|i| format!("/g{i} {} 0 R ", 8 + i)).collect();
        objs.push(format!("<< {glyphs}>>").into_bytes());
        objs.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""));
        match written {
            Written::InPlace => {}
            Written::Objects => objs.extend((0..count).map(|_| font(names).into_bytes())),
            Written::Resources => {
                objs.extend((0..count).map(|i| font(&format!("/Resources {} 0 R", 20 + count + i)).into_bytes()));
                objs.extend((0..count).map(|_| b"<< /XObject << /X 19 0 R >> >>".to_vec()));
            }
        }
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::verify::HIDDEN_LOOKS.with(|c| c.set(0));
        crate::verify::HIDDEN_SEARCHES.with(|c| c.set(0));
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        assert!(!text.contains("can't be read in full") && !text.contains("can't be placed"), "{written:?}: {text}");
        (crate::verify::HIDDEN_LOOKS.with(std::cell::Cell::get), crate::verify::HIDDEN_SEARCHES.with(std::cell::Cell::get))
    };
    for written in [Written::InPlace, Written::Objects, Written::Resources] {
        let once = read(written, 1);
        assert!(once.0 >= 10 && once.1 >= 10, "{written:?}: {once:?}");
        assert_eq!(read(written, 2_000), once, "{written:?}");
    }
}

/// Two Type 3 fonts share one `/CharProcs` object, its glyph drawing `/X`, and give other names:
/// one's `/X` is an empty form, the other's an image. Whichever the form's `/Font` gives first,
/// the glyphs are read with each font's names, and the proof finds the image.
#[test]
fn the_proof_reads_glyphs_two_fonts_share_with_each_fonts_names() {
    let flagged = |fonts: &str| {
        let font = |x: usize| {
            format!("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs 11 0 R /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << /XObject << /X {x} 0 R >> >> >>")
                .into_bytes()
        };
        let objs = vec![
            stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << {fonts} >> >>"),
                b"BT /A 100 Tf 10 250 Td (a) Tj ET",
            ),
            stream("", b"1000 0 d0 /X Do"),
            font(13),
            font(12),
            b"<< /a 8 0 R >>".to_vec(),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
        ];
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared")
    };
    assert!(flagged("/A 9 0 R /B 10 0 R"));
    assert!(flagged("/A 10 0 R /B 9 0 R"));
}

/// Malformed: a Type 3 font's `/XObject` given through a chain of `k` references to names whose
/// `/X` is an empty form. The renderer reads the names through 32 references and none past them,
/// and the glyph then draws the content's `/X`, an image; the proof reads them as far, and finds it.
#[test]
fn the_proof_reads_a_type3_fonts_names_through_as_many_references_as_the_renderer() {
    let flagged = |k: usize| {
        let mut objs = vec![
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /T 11 0 R >> >>", b""),
            stream("", b"1000 0 d0 /X Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
            b"<< /Type /Font /Subtype /Type3 /CharProcs << /a 8 0 R >> /Resources << /XObject 13 0 R >> >>".to_vec(),
            b"<< /X 9 0 R >>".to_vec(),
        ];
        objs.extend((0..k).map(|i| format!("{} 0 R", if i + 1 == k { 12 } else { 14 + i }).into_bytes()));
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R /X 10 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared")
    };
    assert!(!flagged(32), "32 references");
    assert!(flagged(33), "33 references");
    assert!(flagged(64), "64 references");
}

/// A Type 3 font given through a reference to it, or whose `/Resources` are given through a
/// reference to them, whose names give `/X` as an image its glyph draws: the names are read at the
/// end of the chain, and the proof finds the image.
#[test]
fn the_proof_reads_a_type3_fonts_names_at_the_end_of_a_chain() {
    for font in ["13 0 R", "12 0 R"] {
        let objs = vec![
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /T {font} >> >>"), b""),
            stream("", b"1000 0 d0 /X Do"),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
            b"<< /Type /Font /Subtype /Type3 /CharProcs << /a 8 0 R >> /Resources << /XObject << /X 9 0 R >> >> >>".to_vec(),
            b"<< /XObject << /X 9 0 R >> >>".to_vec(),
            b"<< /Type /Font /Subtype /Type3 /CharProcs << /a 8 0 R >> /Resources 14 0 R >>".to_vec(),
            b"10 0 R".to_vec(),
            b"11 0 R".to_vec(),
        ];
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        assert!(text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"), "{font}: {text}");
    }
}

/// Malformed: a Type 3 font gives as its `/XObject`, or as its `/Resources`, the form `/F` its glyph
/// draws through the page's names, whose own resources give `/Im`, an image. The proof reads the
/// form with its own names wherever a glyph draws it, the font's names never taken for the form's
/// own, and finds the image. The renderer was seen to draw none here, so this fails closed; taking
/// them for the form's own would let a font giving the same names as a dictionary share the
/// clean answer.
#[test]
fn the_proof_reads_a_form_a_type3_font_gives_as_its_names_with_the_forms_own() {
    for resources in ["/Resources << /XObject 12 0 R >>", "/Resources 12 0 R"] {
        let objs = vec![
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /T 11 0 R >> >>", b""),
            stream("", b"1000 0 d0 /F Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
            format!("<< /Type /Font /Subtype /Type3 /CharProcs << /a 8 0 R >> {resources} >>").into_bytes(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /F 12 0 R /XObject << /F 12 0 R >> /Resources << /XObject << /Im 10 0 R >> >>",
                b"/Im Do",
            ),
        ];
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R /F 12 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        assert!(text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"), "{resources}: {text}");
    }
}

/// One object is given to two Type 3 fonts in two ways, and gives other names in each: with one
/// font the `/X` the glyph they share draws is an image, and with the other it is nothing (the
/// object read as an `/XObject` gives no `/X`) or an empty form.
/// `<< /XObject << /X 12 0 R >> >>` is one's `/Resources` and the other's `/XObject`; or a font
/// whose own `/Resources` give the empty form, but which has an `/XObject` key giving the image,
/// is the other's `/Resources`. Whichever the form's `/Font` gives first, the proof finds the image.
#[test]
fn the_proof_tells_apart_how_type3_fonts_give_one_object() {
    let font = |rest: &str| {
        format!("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] {rest} >>")
            .into_bytes()
    };
    let flagged = |fonts: &str, a: Vec<u8>, b: Vec<u8>, shared: &[u8]| {
        let objs = vec![
            stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << {fonts} >> >>"),
                b"BT /A 100 Tf 10 250 Td (a) Tj ET",
            ),
            stream("", b"1000 0 d0 /X Do"),
            a,
            b,
            shared.to_vec(),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
        ];
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let text = crate::tests_verify::prove_unapplied(&mut doc).to_text();
        text.contains("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared")
    };
    for fonts in ["/A 9 0 R /B 10 0 R", "/A 10 0 R /B 9 0 R"] {
        let resources = font("/Resources 11 0 R");
        let xobject = font("/Resources << /XObject 11 0 R >>");
        assert!(flagged(fonts, resources, xobject, b"<< /XObject << /X 12 0 R >> >>"), "{fonts}");
        let own = font("/Resources << /XObject << /X 13 0 R >> >> /XObject << /X 12 0 R >>");
        assert!(flagged(fonts, own, font("/Resources 9 0 R"), b"<< >>"), "{fonts}");
    }
}

/// A Type 3 font whose glyph `/a` (object 8) draws `/X`, an empty form (object 9) its own
/// `/XObject` gives.
fn type3_font_drawing_x() -> String {
    "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << /XObject << /X 9 0 R >> >> >>".to_string()
}

/// The page draws its image under the mark, and a form beside it draws the same `/Im` through the
/// page's names, its own resources lacking it: the page's cleanup follows that form and keeps the
/// image, which still shows beside the mark. Where the form's own `/Im` is a form drawing nothing,
/// nothing else draws the page's, and its bytes leave the file. A form without resources draws it
/// too, as before.
#[test]
fn an_image_a_form_draws_beside_the_mark_through_its_callers_names_stays() {
    let run = |form_res: &str| {
        let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
        let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {form_res}"), b"q 100 0 0 30 20 20 cm /Im Do Q");
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let mut doc = one_page(b"q 100 0 0 30 20 240 cm /Im Do Q /Fm Do", "/XObject << /Im 7 0 R /Fm 8 0 R >>", vec![image, form, empty]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let report = apply(&mut doc, None).unwrap();
        (report.images_removed, full_save_holds(&doc, b"SECRET_IMAGE_BYTES"))
    };
    assert_eq!(run("/Resources << /ProcSet [/PDF] >>"), (1, true));
    assert_eq!(run("/Resources << /XObject << /Im 9 0 R >> >>"), (1, false));
    // An own `/Im` that is a number is no XObject: the page's is drawn.
    assert_eq!(run("/Resources << /XObject << /Im 5 >> >>"), (1, true));
    assert_eq!(run(""), (1, true));
}

/// Whether `out` is the refusal of a cleanup that would remove a name a form it couldn't all read
/// may draw with.
fn refused_past_what_is_read(out: &Result<Report, RedactError>) -> bool {
    matches!(out, Err(RedactError::Cos(pdfcraft_cos::CosError::Syntax { detail, .. })) if detail.contains("may draw with what it would remove"))
}

/// A form with resources of its own that give nothing, drawing `/Im` through its caller's names
/// after `before`, and the entries `entries` in its dictionary.
fn drawing_im_after(before: &[u8], entries: &str) -> Vec<u8> {
    stream(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << >> {entries}"),
        [before, b"q 100 0 0 30 20 20 cm /Im Do Q"].concat(),
    )
}

/// The page draws its image under the mark, and beside it a form whose own resources give nothing,
/// which draws the same `/Im` through the page's names after what can't be read: 2,000,000
/// operators, bytes the parser can't read (a string left open), or a filter that doesn't decode.
/// Removing `/Im` would lose the image beside the mark, and keeping it would keep the image
/// redacted, so the redaction is refused and the document saves as before. Read in full, the
/// form keeps the image.
#[test]
fn an_image_a_form_beside_the_mark_may_draw_past_what_is_read_refuses_the_redaction() {
    let run = |form: Vec<u8>| {
        let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
        let mut doc = one_page(b"q 100 0 0 30 20 240 cm /Im Do Q /Fm Do", "/XObject << /Im 7 0 R /Fm 8 0 R >>", vec![image, form]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        let out = apply(&mut doc, None);
        let unchanged = write_full(&doc, &SaveOptions::default()).unwrap() == before;
        (out, unchanged)
    };
    for (label, form) in [
        ("2,000,000 operators", drawing_im_after(&b"n ".repeat(2_000_000), "")),
        ("a string left open", drawing_im_after(b"(", "")),
        ("a filter that doesn't decode", drawing_im_after(b"", "/Filter /NoSuchFilter")),
    ] {
        let (out, unchanged) = run(form);
        assert!(refused_past_what_is_read(&out), "{label}: {out:?}");
        assert!(unchanged, "{label}");
    }
    let (out, _) = run(drawing_im_after(b"", ""));
    assert_eq!(out.map(|r| r.images_removed), Ok(1), "read in full");
}

/// A form beside the mark that can't all be read changes nothing where no name would be removed:
/// the page draws its image again beside the mark, so `/Im` stays and the redaction goes through.
#[test]
fn a_form_that_cant_all_be_read_refuses_only_a_removal() {
    let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
    let mut doc = one_page(
        b"q 100 0 0 30 20 240 cm /Im Do Q q 100 0 0 30 20 120 cm /Im Do Q /Fm Do",
        "/XObject << /Im 7 0 R /Fm 8 0 R >>",
        vec![image, drawing_im_after(b"(", "")],
    );
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(full_save_holds(&doc, b"SECRET_IMAGE_BYTES"), "drawn again beside the mark");
}

/// The page lists a property nothing it reads names, beside a form that can't all be read, which
/// may draw a form without resources naming it: the redaction is refused rather than drop it.
/// Read in full, the property goes.
#[test]
fn a_property_a_form_may_name_past_what_is_read_refuses_the_redaction() {
    let run = |before: &[u8]| {
        let layer = b"<< /Type /OCG /Name (PAGE_LAYER_NAME) >>".to_vec();
        let mut doc = one_page(
            b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /Fm Do",
            "/XObject << /Fm 7 0 R >> /Properties << /MC0 8 0 R >>",
            vec![drawing_im_after(before, ""), layer],
        );
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        apply(&mut doc, None)
    };
    let out = run(b"(");
    assert!(refused_past_what_is_read(&out), "{out:?}");
    assert_eq!(run(b"").map(|r| r.glyphs), Ok(6), "read in full");
}

/// The page lists a Type 3 font nothing it reads shows, beside a form that can't all be read,
/// which may show it: the font's cleanup refuses the redaction rather than drop it.
#[test]
fn a_type3_font_a_form_may_show_past_what_is_read_refuses_the_redaction() {
    let run = |before: &[u8]| {
        let mut doc = one_page(b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /Fm Do", "/XObject << /Fm 7 0 R >>", vec![drawing_im_after(before, "")]);
        list_type3(&mut doc, ObjRef::new(3, 0));
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        apply(&mut doc, None)
    };
    let out = run(b"(");
    assert!(refused_past_what_is_read(&out), "{out:?}");
    assert_eq!(run(b"").map(|r| r.glyphs), Ok(6), "read in full");
}

/// Two pages share the `/Pages` node's resources, and the second draws a form (with resources of
/// its own that give nothing, or none) that may draw `/Im` after a string left open: once the first
/// page's redaction removes its own `/Im`, the check of what the second still draws can't tell, and
/// the redaction is refused rather than unlink `/Im` there. Read in full, the second keeps it.
#[test]
fn an_image_a_page_sharing_the_resources_may_draw_past_what_is_read_refuses_the_redaction() {
    let run = |form_res: &str, before: &[u8]| {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [6 0 R 7 0 R] /Count 2 /Resources << /XObject << /Im 5 0 R /Fm 3 0 R >> >> >>".to_vec(),
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {form_res}"), [before, b"q 100 0 0 30 20 20 cm /Im Do Q"].concat()),
            stream("", b"q 100 0 0 30 20 240 cm /Im Do Q"),
            stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES"),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R >>".to_vec(),
            stream("", b"/Fm Do"),
        ]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let out = apply(&mut doc, Some(&[0]));
        (out, full_save_holds(&doc, b"SECRET_IMAGE_BYTES"))
    };
    for form_res in ["/Resources << >>", ""] {
        let (out, holds) = run(form_res, b"(");
        assert!(matches!(out, Err(RedactError::Residue(1))), "{form_res:?}: {out:?}");
        assert!(holds, "{form_res:?}");
        let (out, holds) = run(form_res, b"");
        assert_eq!((out.map(|r| r.images_removed), holds), (Ok(1), true), "{form_res:?}, read in full");
    }
}

/// Two pages share the `/Pages` node's resources, and the second draws `/Fa`, whose own resources
/// give `/Fa` again, which it draws inside itself, or give `/Fb`, which draws `/Fa` back: the check
/// of what the second page still draws reads such a form to the depth limit, can't tell, and
/// refuses unlinking the first page's `/Im` (malformed: the renderer draws no form inside itself,
/// but whether a form is inside itself depends on the way to it, and the check reads a form once
/// for each scope, whatever the way).
#[test]
fn a_form_drawing_itself_on_a_page_sharing_the_resources_refuses_the_unlink() {
    for fa_gives in ["/Fa 3 0 R", "/Fa 9 0 R"] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [6 0 R 7 0 R] /Count 2 /Resources << /XObject << /Im 5 0 R /Fa 3 0 R >> >> >>".to_vec(),
            stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /XObject << {fa_gives} >> >>"),
                b"/Fa Do 0 0 10 10 re f",
            ),
            stream("", b"q 100 0 0 30 20 240 cm /Im Do Q"),
            stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES"),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R >>".to_vec(),
            stream("", b"/Fa Do"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /XObject << /Fa 3 0 R >> >>", b"/Fa Do"),
        ]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        assert!(matches!(apply(&mut doc, Some(&[0])), Err(RedactError::Residue(1))), "{fa_gives}");
        assert!(full_save_holds(&doc, b"SECRET_IMAGE_BYTES"), "{fa_gives}");
    }
}

/// A form the redaction rewrites draws a form beside the mark that can't all be read, which may draw
/// `/X` through the rewritten form's names: the rewritten form's cleanup refuses the redaction
/// rather than drop `/X`. The page draws the form again away from the mark, so its own cleanup keeps
/// the form and removes nothing.
#[test]
fn an_image_a_nested_form_may_draw_past_what_is_read_refuses_the_redaction() {
    let run = |inner: &[u8]| {
        let outer = stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /XObject << /X 8 0 R /G 9 0 R >> >>",
            b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /G Do",
        );
        let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"OUTSIDE_IMAGE_BYTES");
        let inner = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << >>", [inner, b"q 100 0 0 30 20 20 cm /X Do Q"].concat());
        let mut doc = one_page(b"/Fm Do q 1 0 0 1 0 -200 cm /Fm Do Q", "/XObject << /Fm 7 0 R >>", vec![outer, image, inner]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let out = apply(&mut doc, None);
        (out, full_save_holds(&doc, b"OUTSIDE_IMAGE_BYTES"))
    };
    let (out, holds) = run(b"(");
    assert!(refused_past_what_is_read(&out), "{out:?}");
    assert!(holds);
    let (out, holds) = run(b"");
    assert_eq!((out.map(|r| r.glyphs), holds), (Ok(6), true), "read in full");
}

/// The page draws its image under the mark, and beside it `/A`, whose own `/Im` is a form drawing
/// nothing; `/A` draws `/B`, whose own resources give nothing, so `/B`'s `/Im` is `/A`'s: nothing
/// else draws the page's, and its bytes leave the file. Drawn by the page, `/B`'s `/Im` is the
/// page's, and it stays, also where the page draws `/A` first: `/B`, read once, is taken through
/// the names of each form on the way to it.
#[test]
fn an_image_a_form_draws_through_a_form_giving_its_name_isnt_the_pages() {
    let run = |content: &[u8]| {
        let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
        let a = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /XObject << /Im 9 0 R /B 10 0 R >> >>", b"/B Do");
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let b = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << >>", b"q 100 0 0 30 20 20 cm /Im Do Q");
        let mut doc = one_page(content, "/XObject << /Im 7 0 R /A 8 0 R /B 10 0 R >>", vec![image, a, empty, b]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let report = apply(&mut doc, None).unwrap();
        (report.images_removed, full_save_holds(&doc, b"SECRET_IMAGE_BYTES"))
    };
    assert_eq!(run(b"q 100 0 0 30 20 240 cm /Im Do Q /A Do"), (1, false));
    assert_eq!(run(b"q 100 0 0 30 20 240 cm /Im Do Q /B Do"), (1, true));
    assert_eq!(run(b"q 100 0 0 30 20 240 cm /Im Do Q /A Do /B Do"), (1, true));
}

/// A form the redaction rewrites draws, through its own `/XObject`, a form beside the mark whose
/// own resources lack `/X`, which it draws through the rewritten form's names: the rewritten form
/// keeps `/X`. Where that form's own `/X` is a form drawing nothing, the image goes.
#[test]
fn an_image_a_nested_form_draws_through_a_rewritten_forms_names_stays() {
    let run = |inner_res: &str| {
        let outer = stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /XObject << /X 9 0 R /G 10 0 R >> >>",
            b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /G Do",
        );
        let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"OUTSIDE_IMAGE_BYTES");
        let inner = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {inner_res}"), b"q 100 0 0 30 20 20 cm /X Do Q");
        let empty = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"");
        let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![outer, empty.clone(), image, inner, empty]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        let report = apply(&mut doc, None).unwrap();
        (report.glyphs, full_save_holds(&doc, b"OUTSIDE_IMAGE_BYTES"))
    };
    assert_eq!(run("/Resources << /ProcSet [/PDF] >>"), (6, true));
    assert_eq!(run("/Resources << /XObject << /X 11 0 R >> >>"), (6, false));
}

/// A form beside the mark with resources of its own marks content with `/MC0`, which its own
/// resources don't give: it reads properties in its own, so the page's `/MC0`, which nothing on
/// the page names, still goes.
#[test]
fn a_forms_own_marked_content_keeps_nothing_of_the_pages_properties() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /ProcSet [/PDF] >>", b"/OC /MC0 BDC 0 0 10 10 re f EMC");
    let layer = b"<< /Type /OCG /Name (PAGE_LAYER_NAME) >>".to_vec();
    let mut doc =
        one_page(b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /Fm Do", "/XObject << /Fm 7 0 R >> /Properties << /MC0 8 0 R >>", vec![form, layer]);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    let page = pdfcraft_model::pages(&doc)[0].dict.clone();
    let res = page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap();
    let properties = res.get(b"Properties").and_then(|p| doc.resolve(p).as_dict().cloned()).unwrap_or_default();
    assert!(properties.get(b"MC0").is_none(), "{properties:?}");
}

/// Inside a form with resources of its own, beside the mark, a form without resources marks content
/// with `/MC1`: it reads the page's properties (ISO 32000-2 §7.8.3), so the page keeps `/MC1`.
#[test]
fn a_nested_form_without_resources_keeps_the_pages_property() {
    let outer = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /XObject << /G 8 0 R >> >>", b"/G Do");
    let inner = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100]", b"/OC /MC1 BDC 0 0 10 10 re f EMC");
    let layer = b"<< /Type /OCG /Name (PAGE_LAYER_NAME) >>".to_vec();
    let mut doc =
        one_page(b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /Fm Do", "/XObject << /Fm 7 0 R >> /Properties << /MC1 9 0 R >>", vec![outer, inner, layer]);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    let page = pdfcraft_model::pages(&doc)[0].dict.clone();
    let res = page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap();
    let properties = res.get(b"Properties").and_then(|p| doc.resolve(p).as_dict().cloned()).unwrap_or_default();
    assert!(properties.get(b"MC1").is_some(), "{properties:?}");
}

/// A form beside the mark with resources of its own draws itself, or one that draws it back, and
/// the page's image through the page's names: the cleanups follow each once, open a scope for each
/// form on the way and no more, and keep the image.
#[test]
fn forms_drawing_themselves_open_a_scope_each() {
    for (fa, fb) in [(&b"/Fa Do q 100 0 0 30 20 20 cm /Im Do Q"[..], &b""[..]), (b"/Fb Do", b"/Fa Do q 100 0 0 30 20 20 cm /Im Do Q")] {
        let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
        let own = "/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /ProcSet [/PDF] >>";
        let mut doc = one_page(
            b"q 100 0 0 30 20 240 cm /Im Do Q /Fa Do",
            "/XObject << /Im 7 0 R /Fa 8 0 R /Fb 9 0 R >>",
            vec![image, stream(own, fa), stream(own, fb)],
        );
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::interp::CLEANUP_SCOPES.with(|c| c.set(0));
        assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
        assert!(full_save_holds(&doc, b"SECRET_IMAGE_BYTES"));
        let forms = if fb.is_empty() { 1 } else { 2 };
        // The page's XObject cleanup opens one scope for each form; the font cleanup, with no Type 3
        // font listed, doesn't read them.
        assert_eq!(crate::interp::CLEANUP_SCOPES.with(std::cell::Cell::get), forms);
    }
}

/// A form beside the mark with resources of its own draws itself through the page's `/Fa`, and
/// nothing else: not following it inside itself, where the renderer draws nothing, leaves the read
/// whole, so the page's image under the mark still goes.
#[test]
fn a_form_drawing_itself_leaves_the_cleanups_whole() {
    let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /ProcSet [/PDF] >>", b"/Fa Do 0 0 10 10 re f");
    let mut doc = one_page(b"q 100 0 0 30 20 240 cm /Im Do Q /Fa Do", "/XObject << /Im 7 0 R /Fa 8 0 R >>", vec![image, form]);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(!full_save_holds(&doc, b"SECRET_IMAGE_BYTES"));
}

/// The page draws `/W` over the mark 64 times, and each draw, rewritten, draws `/S`, a form with
/// resources of its own of 100,000 operators: the cleanups of the rewritten forms read `/S` within
/// one allowance and 2,000,000 operators for the page between them, not afresh for each.
#[test]
fn a_rewritten_forms_cleanups_read_forms_with_resources_within_one_allowance_a_page() {
    let w = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /XObject << /S 8 0 R >> >>",
        b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /S Do",
    );
    let s = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /ProcSet [/PDF] >>", b"n ".repeat(100_000));
    let mut doc = one_page(&b"/W Do ".repeat(64), "/XObject << /W 7 0 R >>", vec![w, s]);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    crate::interp::SCOPED_OPS_READ.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(64 * 6));
    let read = crate::interp::SCOPED_OPS_READ.with(std::cell::Cell::get);
    assert!(read <= 2_000_001, "{read}");
}

/// The same, `/S` setting graphics states its own resources don't give 100,000 times: read once, it
/// is taken again for each rewritten `/W` at what it keeps. Setting one state each time, it keeps
/// that one once, and the page is redacted (its cleanups then read the rewritten `/W`s too);
/// setting 100,000 states, the page's 2,000,000 operators run out, and the redaction is refused
/// rather than drop `/W`, which `/S` may then draw.
#[test]
fn a_form_taken_again_and_again_costs_what_it_keeps() {
    let one = b"/a gs ".repeat(100_000);
    let each: Vec<u8> = (0..100_000).flat_map(|i| format!("/a{i} gs ").into_bytes()).collect();
    for (content, redacted) in [(one, true), (each, false)] {
        let w = stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /XObject << /S 8 0 R >> >>",
            b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /S Do",
        );
        let s = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /ProcSet [/PDF] >>", &content);
        let mut doc = one_page(&b"/W Do ".repeat(64), "/XObject << /W 7 0 R >>", vec![w, s]);
        mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
        crate::interp::SCOPED_OPS_READ.with(|c| c.set(0));
        let out = apply(&mut doc, None);
        if redacted {
            assert_eq!(out.map(|r| r.glyphs), Ok(64 * 6));
        } else {
            assert!(refused_past_what_is_read(&out), "{out:?}");
        }
        let read = crate::interp::SCOPED_OPS_READ.with(std::cell::Cell::get);
        assert!((100_000..101_000).contains(&read), "redacted {redacted}: {read}");
    }
}

/// Sixty forms beside the mark, each with resources of its own giving the next: the cleanups follow
/// the chain no deeper than the walk follows forms, opening 50 scopes in the page's XObject cleanup
/// (the font cleanup, with no Type 3 font listed, doesn't read them).
#[test]
fn a_chain_of_forms_is_followed_no_deeper_than_the_walk() {
    let image = stream("/Type /XObject /Subtype /Image /Width 6 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"SECRET_IMAGE_BYTES");
    let mut objs = vec![image];
    for i in 0..60 {
        let (next, content): (String, &[u8]) = if i + 1 < 60 { (format!("/XObject << /F {} 0 R >>", 9 + i), b"/F Do") } else { (String::new(), b"") };
        objs.push(stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] /Resources << /ProcSet [/PDF] {next} >>"), content));
    }
    let mut doc = one_page(b"q 100 0 0 30 20 240 cm /Im Do Q /F Do", "/XObject << /Im 7 0 R /F 8 0 R >>", objs);
    mark(&mut doc, 0, &[[0.0, 230.0, 300.0, 280.0]], "");
    crate::interp::CLEANUP_SCOPES.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert_eq!(crate::interp::CLEANUP_SCOPES.with(std::cell::Cell::get), crate::interp::MAX_DEPTH);
}
