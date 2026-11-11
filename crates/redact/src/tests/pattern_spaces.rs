//! `cs` and `CS` reset the colour, and with it the pattern selected before, unless the renderer
//! takes the space for a Pattern space: then it keeps the pattern (ISO 32000-2 §8.6.8 and Poppler
//! reset that too). It reads the name as the renderer does: `/Pattern` itself, a device space's
//! name never, and otherwise the first `/ColorSpace` that has the name, the scope's own resources
//! first, then those of the scopes that drew it. A pattern the renderer still paints with is kept;
//! one it doesn't leaves the page.

use super::*;
use crate::interp::{ENTRY_BYTES, MAX_SPACE_BYTES, PatternSpaces, SPACE_READS};

/// A tiling pattern whose cell draws a rectangle and nothing else (object 7), which can stay.
fn cell() -> Vec<u8> {
    stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 300 300] /XStep 300 /YStep 300 /Resources << >>", b"0 0 300 300 re f")
}

/// Fills a box under the mark with `/P0`, then a box beside it after `{space} cs`.
fn two_boxes(space: &str) -> String {
    format!("/Pattern cs /P0 scn 20 140 100 30 re f {space} cs 20 40 100 30 re f")
}

/// How many patterns applying a mark over the first box removes from a page drawing `content`,
/// with `res` its resources (besides the font), the cell as object 7 and `extra` from object 8.
fn removed(content: &str, res: &str, extra: Vec<Vec<u8>>) -> usize {
    let mut objs = vec![cell()];
    objs.extend(extra);
    let mut doc = one_page(content.as_bytes(), res, objs);
    mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
    apply(&mut doc, None).unwrap().patterns_removed
}

/// A space the resources give as `/Pattern` or `[/Pattern base]`, directly or by reference at any
/// level, keeps the pattern the box beside the mark is filled with, as does `/Pattern` itself even
/// where the resources name something else so; and a `cs` whose operand isn't a name, which the
/// renderer skips. A grey space, a device space's name (read before the resources), a name the
/// resources don't have (a `/ColorSpace` that isn't a dictionary has none; a stream's is its
/// dictionary's), a name naming a name, and `null` reset it, and the pattern leaves the page. `CS`
/// likewise for the stroke.
#[test]
fn a_named_pattern_space_keeps_the_pattern() {
    let pattern = "/Pattern << /P0 7 0 R >>";
    for (space, spaces, extra, gone) in [
        ("/CSP", "<< /CSP /Pattern >>", None, 0),
        ("/CSP", "<< /CSP [/Pattern /DeviceRGB] >>", None, 0),
        ("/CSP", "<< /CSP [/Pattern] >>", None, 0),
        ("/Pattern", "<< /Pattern /DeviceGray >>", None, 0),
        ("(x)", "<< >>", None, 0),
        ("/CSP", "8 0 R", Some("<< /CSP /Pattern >>"), 0),
        ("/CSP", "<< /CSP 8 0 R >>", Some("/Pattern"), 0),
        ("/CSP", "<< /CSP 8 0 R >>", Some("[/Pattern /DeviceRGB]"), 0),
        ("/CSP", "<< /CSP [8 0 R /DeviceRGB] >>", Some("/Pattern"), 0),
        ("/CSG", "<< /CSG /DeviceGray >>", None, 1),
        ("/G", "<< /G /Pattern >>", None, 1),
        ("/CSX", "<< /CSP /Pattern >>", None, 1),
        ("/CSP", "<< /CSP /CSQ /CSQ /Pattern >>", None, 1),
        ("/CSP", "<< /CSP null >>", None, 1),
        ("/CSP", "<< /CSP 8 0 R >>", Some("/DeviceGray"), 1),
        ("/CSP", "8 0 R", Some("[/CSP /Pattern]"), 1),
    ] {
        let extra: Vec<Vec<u8>> = extra.map(|e| e.as_bytes().to_vec()).into_iter().collect();
        let res = format!("{pattern} /ColorSpace {spaces}");
        assert_eq!(removed(&two_boxes(space), &res, extra), gone, "{space} cs with /ColorSpace {spaces}");
    }
    let stroked = |space: &str| format!("/Pattern CS /P0 SCN 20 140 100 30 re S {space} CS 20 40 100 30 re S");
    let res = format!("{pattern} /ColorSpace << /CSP /Pattern /CSG /DeviceGray >>");
    assert_eq!(removed(&stroked("/CSP"), &res, vec![]), 0, "CS of a named Pattern space");
    assert_eq!(removed(&stroked("/CSG"), &res, vec![]), 1, "CS of a grey space");
    // A `/ColorSpace` that is a stream is read by its dictionary, as the renderer reads it.
    let res = format!("{pattern} /ColorSpace 8 0 R");
    assert_eq!(removed(&two_boxes("/CSP"), &res, vec![stream("/CSP /Pattern", b"")]), 0, "a stream's dictionary");
}

/// A form without resources of its own paints with the page's patterns and reads its colour space
/// names in the page's resources, as the renderer does: the page selects `/P0` and fills the box
/// under the mark, and the form, drawn after `0 g`, selects it again and fills after `/CSP cs`, so
/// the page's `/CSP` decides whether the pattern still paints.
#[test]
fn a_form_without_resources_reads_the_pages_colour_spaces() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"/P0 scn /CSP cs 20 40 100 30 re f");
    let content = "/Pattern cs /P0 scn 20 140 100 30 re f 0 g /Fm Do";
    for (spaces, gone) in [("<< /CSP /Pattern >>", 0), ("<< /CSP /DeviceGray >>", 1)] {
        let res = format!("/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R >> /ColorSpace {spaces}");
        assert_eq!(removed(content, &res, vec![form.clone()]), gone, "/ColorSpace {spaces}");
    }
}

/// What `cs` reads by reference is read once for the page, however often `cs` names it, however
/// many times a form naming it is drawn, and however deep the forms nest: a large object, read
/// again, can be parsed again, all of it. A `/ColorSpace` given by reference, an entry of it given
/// by reference and an entry's first element given by reference are read as often for 20,000 `cs`
/// as for one; and a `/ColorSpace` of 5,000 spaces as often for 1,000 drawings of a form as for
/// one, and for forms 40 deep as for one.
#[test]
fn a_colour_space_dictionary_is_read_once_for_the_page() {
    let spaces: String = (0..5000).map(|i| format!("/C{i} /DeviceGray ")).collect();
    let spaces = format!("<< {spaces}/CSP /Pattern >>").into_bytes();
    let reads = |content: &str, res: &str, extra: Vec<Vec<u8>>| {
        SPACE_READS.with(|c| c.set(0));
        assert_eq!(removed(content, res, extra), 0, "{}", &content[..content.len().min(60)]);
        SPACE_READS.with(std::cell::Cell::get)
    };
    let many = format!("/Pattern cs /P0 scn 20 140 100 30 re f {}20 40 100 30 re f", "/CSP cs ".repeat(20_000));
    // With no pattern selected, `cs` has nothing to reset and reads nothing.
    assert_eq!(reads("/CSP cs 20 40 100 30 re f", "/Pattern << /P0 7 0 R >> /ColorSpace 8 0 R", vec![spaces.clone()]), 0, "no pattern selected");
    // The dictionary by reference, an entry by reference, and an entry's first element by
    // reference.
    for (spaces_entry, extra) in
        [("8 0 R", spaces.clone()), ("<< /CSP 8 0 R >>", b"[/Pattern /DeviceRGB]".to_vec()), ("<< /CSP [8 0 R /DeviceRGB] >>", b"/Pattern".to_vec())]
    {
        let res = format!("/Pattern << /P0 7 0 R >> /ColorSpace {spaces_entry}");
        let once = reads(&two_boxes("/CSP"), &res, vec![extra.clone()]);
        assert!(once >= 1, "{spaces_entry} is read");
        assert_eq!(reads(&many, &res, vec![extra]), once, "20,000 cs with /ColorSpace {spaces_entry}");
    }

    // A form with its own `/ColorSpace` (object 9), drawn once, then 1,000 times, with a pattern
    // selected: without one, `cs` has nothing to reset and looks nothing up.
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace 9 0 R >>", b"/CSP cs 0 0 1 1 re f");
    let drawn =
        |n: usize| reads(&format!("/Pattern cs /P0 scn {}", "/Fm Do ".repeat(n)), "/XObject << /Fm 8 0 R >>", vec![form.clone(), spaces.clone()]);
    let one = drawn(1);
    assert!(one >= 1, "the form's dictionary is read");
    assert_eq!(drawn(1000), one, "a form drawn 1,000 times");

    // Forms 8 to 47, each with the same `/ColorSpace` (object 48), each naming a space and drawing
    // the next.
    let nested = |depth: usize| {
        let mut extra: Vec<Vec<u8>> = (0..depth)
            .map(|k| {
                let next = if k + 1 < depth { format!("/XObject << /Fn {} 0 R >>", 9 + k) } else { String::new() };
                let draw = if k + 1 < depth { "/Fn Do" } else { "" };
                stream(
                    &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace {} 0 R {next} >>", 8 + depth),
                    format!("/CSP cs {draw}").as_bytes(),
                )
            })
            .collect();
        extra.push(spaces.clone());
        reads("/Pattern cs /P0 scn /Fm Do", "/XObject << /Fm 8 0 R >>", extra)
    };
    assert_eq!(nested(40), nested(1), "forms 40 deep");
}

/// What the lookup keeps is what each name of the `/ColorSpace` dictionaries it reads gives, within
/// `MAX_SPACE_BYTES` at once: each name's length and `ENTRY_BYTES`. A dictionary whose names alone
/// would go past it keeps no names, and a name in it keeps the pattern: `/CSG`, grey, beside one
/// long name resets it in a dictionary whose names cost `MAX_SPACE_BYTES`, and keeps it in one
/// byte more. One that fits only once the names kept are let go is read so: `/CSG` resets the
/// pattern in a dictionary of 60% of the allowance read after a form with resources of its own
/// read another 60%.
#[test]
fn the_lookup_keeps_a_bounded_amount_of_names() {
    // `/CSG` and a name of `len` bytes, costing `len + 3 + 2 * ENTRY_BYTES` together.
    let spaces = |len: usize| format!("<< /CSG /DeviceGray /{} /DeviceGray >>", "N".repeat(len)).into_bytes();
    let fits = MAX_SPACE_BYTES - 3 - 2 * ENTRY_BYTES;
    let res = "/Pattern << /P0 7 0 R >> /ColorSpace 8 0 R";
    assert_eq!(removed(&two_boxes("/CSG"), res, vec![spaces(fits)]), 1, "within the allowance");
    assert_eq!(removed(&two_boxes("/CSG"), res, vec![spaces(fits + 1)]), 0, "past it");

    // The form (object 8), drawn before the page selects `/P0` (a `Do` keeps the pattern
    // selected), selects one of its own and reads its own `/ColorSpace` (object 9); the page's is
    // object 10.
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace 9 0 R >>", b"/PF scn /CSF cs");
    let res = "/Pattern << /P0 7 0 R >> /XObject << /Fm 8 0 R >> /ColorSpace 10 0 R";
    let content = "q /Fm Do Q /Pattern cs /P0 scn 20 140 100 30 re f /CSG cs 20 40 100 30 re f";
    let extra = vec![form, spaces(MAX_SPACE_BYTES * 6 / 10), spaces(MAX_SPACE_BYTES * 6 / 10)];
    assert_eq!(removed(content, res, extra), 1, "after the form's names");
}

/// A run that reads more names than the lookup keeps at once lets the kept ones go and goes on: three
/// pages, each with a `/ColorSpace` of its own given by reference, of 40% of the allowance each,
/// each reset the pattern at `/CSG cs`, and the pattern leaves each page.
#[test]
fn a_run_reading_more_names_than_it_keeps_lets_the_kept_ones_go() {
    let names = (MAX_SPACE_BYTES * 4 / 10) / (8 + ENTRY_BYTES);
    let spaces: String = (0..names).map(|i| format!("/C{i:07} /DeviceGray ")).collect();
    let pages = 3;
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 8 + 2 * i)).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes(),
        b"null".to_vec(),
        stream("", two_boxes("/CSG").as_bytes()),
        cell(),
        b"null".to_vec(),
        b"null".to_vec(),
    ];
    for i in 0..pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Pattern << /P0 5 0 R >> /ColorSpace {} 0 R >> >>", 9 + 2 * i).into_bytes());
        objs.push(format!("<< {spaces}/CSG /DeviceGray >>").into_bytes());
    }
    let mut doc = pdf(objs);
    for page in 0..pages {
        mark(&mut doc, page, &[[15.0, 135.0, 130.0, 175.0]], "");
    }
    assert_eq!(apply(&mut doc, None).unwrap().patterns_removed, pages, "each page's pattern goes");
}

/// `pages` pages, each marked over the first box, sharing one content stream (object 4) and one
/// `/Resources` (object 5), whose `/ColorSpace` (object 7) names `/CSP` as `/Pattern`.
fn pages_sharing_spaces(pages: usize) -> Document {
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 9 + i)).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes(),
        b"null".to_vec(),
        stream("", two_boxes("/CSP").as_bytes()),
        b"<< /Pattern << /P0 6 0 R >> /ColorSpace 7 0 R >>".to_vec(),
        cell(),
        b"<< /CSP /Pattern >>".to_vec(),
        b"null".to_vec(),
    ];
    objs.extend((0..pages).map(|_| b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec()));
    let mut doc = pdf(objs);
    for page in 0..pages {
        mark(&mut doc, page, &[[15.0, 135.0, 130.0, 175.0]], "");
    }
    doc
}

/// What `cs` reads by reference is read once for the run, not again for each page that names it:
/// 20 pages sharing a `/ColorSpace` given by reference read it as often as one page does, once.
#[test]
fn a_colour_space_dictionary_is_read_once_for_the_run() {
    let reads = |pages: usize| {
        let mut doc = pages_sharing_spaces(pages);
        SPACE_READS.with(|c| c.set(0));
        let report = apply(&mut doc, None).unwrap();
        assert_eq!(report.patterns_removed, 0, "{pages} pages keep the pattern the second box still paints");
        SPACE_READS.with(std::cell::Cell::get)
    };
    assert_eq!(reads(1), 1, "one page");
    assert_eq!(reads(20), 1, "20 pages");
}

/// Only Apply prunes patterns, so only its walk of a page looks a colour space up: the proof's
/// snapshot and Apply's check of what it wrote read none, and nor do Sanitize's scan and removal.
#[test]
fn a_walk_whose_patterns_nothing_prunes_reads_no_colour_space() {
    let reads = |f: &dyn Fn(&mut Document)| {
        let mut doc = pages_sharing_spaces(1);
        SPACE_READS.with(|c| c.set(0));
        f(&mut doc);
        SPACE_READS.with(std::cell::Cell::get)
    };
    assert_eq!(reads(&|doc| drop(crate::sanitize::scan(doc))), 0, "Sanitize's scan");
    assert_eq!(reads(&|doc| drop(crate::sanitize::remove_hidden(doc, &[crate::sanitize::Hidden::HiddenText]))), 0, "Remove Hidden Information");
    assert_eq!(reads(&|doc| drop(apply(doc, None).unwrap())), 1, "Apply, its walk alone");
}

/// Letting the kept names go keeps what kept none: a `/ColorSpace` that isn't a dictionary, read
/// once, is not read again when it is named after two others of 60% of the allowance each have
/// made the lookup let go of the first's names.
#[test]
fn what_kept_no_names_is_not_read_again_after_the_kept_ones_go() {
    let names = (MAX_SPACE_BYTES * 6 / 10) / (8 + ENTRY_BYTES);
    let spaces = |prefix: char| {
        let names: String = (0..names).map(|i| format!("/{prefix}{i:07} /DeviceGray ")).collect();
        format!("<< {names}>>").into_bytes()
    };
    // Forms 8, 9 and 10 name `/CSX` in /ColorSpace objects 11 (an array), 12 and 13.
    let form =
        |space: u32| stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace {space} 0 R >>"), b"/CSX cs");
    let extra = vec![form(11), form(12), form(13), b"[/DeviceGray]".to_vec(), spaces('A'), spaces('B')];
    let res = "/Pattern << /P0 7 0 R >> /XObject << /F1 8 0 R /F2 9 0 R /F3 10 0 R >>";
    SPACE_READS.with(|c| c.set(0));
    removed("/Pattern cs /P0 scn /F1 Do /F2 Do /F3 Do /F1 Do", res, extra);
    assert_eq!(SPACE_READS.with(std::cell::Cell::get), 3, "the array, then each dictionary, once");
}

/// A `/ColorSpace` of 60% of the allowance: `/CSX` and `/CSG`, grey, and one long name.
fn sixty_percent() -> Vec<u8> {
    format!("<< /CSX /DeviceGray /CSG /DeviceGray /{} /DeviceGray >>", "N".repeat(MAX_SPACE_BYTES * 6 / 10)).into_bytes()
}

/// Two dictionaries that don't fit together, named in turn on one page, are read no more for
/// 1,000 drawings than for one: once each. The second lets go of the first, which the page then
/// can't keep beside it, and a name in it keeps the pattern for the rest of the page.
#[test]
fn dictionaries_named_in_turn_are_read_no_more_for_more_drawings() {
    let reads = |pairs: usize| {
        let form = |spaces: u32| {
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace {spaces} 0 R >>"), b"/PF scn /CSX cs")
        };
        let content = format!("/Pattern cs /P0 scn 20 140 100 30 re f {}/CSG cs 20 40 100 30 re f", "q /F1 Do Q q /F2 Do Q ".repeat(pairs));
        let res = "/Pattern << /P0 7 0 R >> /XObject << /F1 8 0 R /F2 9 0 R >>";
        let mut doc = one_page(content.as_bytes(), res, vec![cell(), form(10), form(11), sixty_percent(), sixty_percent()]);
        mark(&mut doc, 0, &[[15.0, 135.0, 130.0, 175.0]], "");
        SPACE_READS.with(|c| c.set(0));
        apply(&mut doc, None).unwrap();
        SPACE_READS.with(std::cell::Cell::get)
    };
    assert_eq!(reads(1), 2, "one drawing of each");
    assert_eq!(reads(1_000), 2, "1,000 drawings of each");
}

/// Ten pages whose `/ColorSpace`, of 60% of the allowance, is in turn one dictionary and another:
/// each is read twice, once and again after the other let it go. Named a third time, the first
/// isn't read, and its `/CSG` keeps the pattern on that page, as on the pages after that name it;
/// the second, kept since, still resets it.
#[test]
fn a_dictionary_is_read_at_most_twice_in_a_run() {
    let pages = 10;
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 8 + i)).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes(),
        sixty_percent(),
        stream("", two_boxes("/CSG").as_bytes()),
        cell(),
        sixty_percent(),
        b"null".to_vec(),
    ];
    for i in 0..pages {
        let spaces = if i % 2 == 0 { 3 } else { 6 };
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Pattern << /P0 5 0 R >> /ColorSpace {spaces} 0 R >> >>").into_bytes());
    }
    let mut doc = pdf(objs);
    for page in 0..pages {
        mark(&mut doc, page, &[[15.0, 135.0, 130.0, 175.0]], "");
    }
    SPACE_READS.with(|c| c.set(0));
    let removed = apply(&mut doc, None).unwrap().patterns_removed;
    assert_eq!(SPACE_READS.with(std::cell::Cell::get), 4, "each dictionary twice");
    assert_eq!(removed, 7, "all but the three pages naming the first a third time and after");
}

/// The names a page names are the last to go: five pages, each drawing a form whose own
/// `/ColorSpace` (object 7) they share, then naming a `/ColorSpace` of their own of 40% of the
/// allowance. The pages' own let go of each other's, never of the shared one, read once.
#[test]
fn the_names_a_page_names_are_the_last_to_go() {
    let names = (MAX_SPACE_BYTES * 4 / 10) / (8 + ENTRY_BYTES);
    let spaces: String = (0..names).map(|i| format!("/C{i:07} /DeviceGray ")).collect();
    let pages = 5;
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 8 + 2 * i)).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes(),
        b"null".to_vec(),
        stream("", format!("q /Fm Do Q {}", two_boxes("/CSG")).as_bytes()),
        cell(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ColorSpace 7 0 R >>", b"/PF scn /CSS cs"),
        b"<< /CSS /DeviceGray >>".to_vec(),
    ];
    for i in 0..pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Pattern << /P0 5 0 R >> /XObject << /Fm 6 0 R >> /ColorSpace {} 0 R >> >>", 9 + 2 * i).into_bytes());
        objs.push(format!("<< {spaces}/CSG /DeviceGray >>").into_bytes());
    }
    let mut doc = pdf(objs);
    for page in 0..pages {
        mark(&mut doc, page, &[[15.0, 135.0, 130.0, 175.0]], "");
    }
    SPACE_READS.with(|c| c.set(0));
    assert_eq!(apply(&mut doc, None).unwrap().patterns_removed, pages, "each page's pattern goes");
    assert_eq!(SPACE_READS.with(std::cell::Cell::get), 1 + pages, "the shared dictionary once, each page's own once");
}

/// What the lookup counts its kept names as is what they cost, within the allowance, however they
/// come and go: one dictionary kept on a page and named again on the next counts for that page, so
/// another of 60% lets all go, not only what the page hasn't named; and on a third page the first,
/// let go and named again, comes back in place of the other.
#[test]
fn the_lookup_counts_what_it_keeps() {
    let doc = one_page(b"", "", vec![sixty_percent(), sixty_percent()]);
    let (first, second) = (Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0)));
    let mut spaces = PatternSpaces::default();
    let check = |spaces: &PatternSpaces, step: &str| {
        let (counted, kept) = spaces.counted();
        assert_eq!(counted, kept, "{step}");
        assert!(counted <= MAX_SPACE_BYTES, "{step}: {counted}");
    };
    for (page, names) in [(1, &[&first][..]), (2, &[&first, &second]), (3, &[&first])] {
        spaces.next_page();
        for (i, spaces_ref) in names.iter().enumerate() {
            assert!(!spaces.named(&doc, Some(spaces_ref), b"CSG"), "page {page}, name {i}: grey");
            check(&spaces, &format!("page {page}, name {i}"));
        }
    }
}
