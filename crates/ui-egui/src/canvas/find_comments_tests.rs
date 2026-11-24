//! The Search panel's comment results: what is found, what is shown, and what the reader's
//! clicks select (Delete acts on the selection).

use super::find::Find;
use super::find_comments::{comment_hits, excerpt_match};
use super::*;

fn page_info() -> pdfcraft_render::PageInfo {
    pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }
}

fn note(page: usize, index: usize, contents: &str, parent: Option<(usize, usize)>) -> pdfcraft_render::Annotation {
    pdfcraft_render::Annotation {
        page,
        subtype: "Text".into(),
        author: Some("Ada".into()),
        contents: Some(contents.into()),
        modified: None,
        name: None,
        in_reply_to: None,
        parent,
        grouped: false,
        rect: [10.0, 100.0, 30.0, 120.0],
        color: None,
        index,
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: None,
        fill_sign: false,
        no_rotate: false,
        points: Vec::new(),
        text_box: None,
        popup: None,
    }
}

fn commented(annotations: Vec<pdfcraft_render::Annotation>) -> DocInfo {
    DocInfo { pages: vec![page_info(); 2], annotations, ..Default::default() }
}

/// A view searching `query` with Include comments on, its comment results found.
fn searching(info: &DocInfo, query: &str) -> DocView {
    let mut v = DocView::new(DocId(1), info, ViewDefaults { display: PageDisplay::Single, continuous: true, ..Default::default() }, true);
    let mut find = Find::default();
    find.query = query.into();
    find.include_comments = true;
    v.find = Some(find);
    v.rerun_find();
    v.sync_comment_hits(info);
    v
}

fn found(v: &DocView) -> Vec<((usize, usize), String)> {
    v.find.as_ref().map_or_else(Vec::new, |f| f.comment_hits.iter().map(|hit| (hit.key(), hit.excerpt.clone())).collect())
}

#[test]
fn a_reply_finds_its_thread_and_group_members_find_nothing() {
    let mut member = note(1, 1, "zebra in the member", Some((1, 0)));
    member.grouped = true;
    let info = commented(vec![note(0, 0, "plain", None), note(0, 1, "zebra reply", Some((0, 0))), note(1, 0, "other", None), member]);
    let v = searching(&info, "zebra");
    assert_eq!(found(&v), vec![((0, 0), "zebra reply".to_owned())]);
}

#[test]
fn comment_search_honours_case_whole_words_author_and_type() {
    let info = commented(vec![note(0, 0, "Zebras run", None), note(1, 0, "nothing here", None)]);
    let hits = |query: &str, case: bool, whole: bool| comment_hits(&info, query, case, whole).iter().map(|hit| hit.key()).collect::<Vec<_>>();
    assert_eq!(hits("zebra", false, false), vec![(0, 0)]);
    assert!(hits("zebra", true, false).is_empty(), "case-sensitive");
    assert!(hits("zebra", false, true).is_empty(), "whole words");
    assert_eq!(hits("ada", false, true), vec![(0, 0), (1, 0)], "the author");
    assert_eq!(hits("note", false, true), vec![(0, 0), (1, 0)], "the type, as the panel names it");
}

#[test]
fn a_late_match_in_a_long_comment_is_in_its_excerpt() {
    let long = format!("{} needle at the end", "word ".repeat(60));
    let info = commented(vec![note(0, 0, &long, None)]);
    let [(_, excerpt)] = found(&searching(&info, "needle")).try_into().unwrap_or_else(|_| panic!("one hit"));
    assert!(excerpt.starts_with('…') && excerpt.contains("needle"), "{excerpt}");
    assert!(excerpt.len() <= super::find_comments::EXCERPT_BYTES);
}

#[test]
fn a_long_match_after_wide_characters_stays_whole_in_its_excerpt() {
    let needle = "x".repeat(130);
    let text = format!("{} {needle}", "語".repeat(40));
    let info = commented(vec![note(0, 0, &text, None)]);
    let [(_, excerpt)] = found(&searching(&info, &needle)).try_into().unwrap_or_else(|_| panic!("one hit"));
    assert!(excerpt.contains(&needle), "{excerpt}");
    assert!(excerpt.len() <= super::find_comments::EXCERPT_BYTES);
}

#[test]
fn the_excerpt_highlights_the_match_however_it_is_cased() {
    let excerpt = "…the Éclair ends";
    let range = excerpt_match(excerpt, "ÉCLAIR", false, false).unwrap_or_default();
    assert_eq!(excerpt.get(range), Some("Éclair"));
    assert_eq!(excerpt_match(excerpt, "nothing", false, false), None);
}

#[test]
fn comments_are_searched_only_with_include_comments_on() {
    let info = commented(vec![note(0, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    assert_eq!(found(&v).len(), 1);
    if let Some(f) = v.find.as_mut() {
        f.include_comments = false;
    }
    v.sync_comment_hits(&info);
    assert!(found(&v).is_empty());
}

#[test]
fn include_comments_set_off_without_the_panel_drops_the_comment_find_selected() {
    let info = commented(vec![note(0, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    // The control option, with no panel open to sync the results.
    v.set_include_comments(false);
    assert!(v.session.comments.selection().is_empty());
    assert_eq!(v.session.flash, None);
    assert_eq!(v.current_comment(), None);
}

#[test]
fn turning_include_comments_off_drops_the_comment_find_selected_and_its_outline() {
    let info = commented(vec![note(0, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    if let Some(f) = v.find.as_mut() {
        f.include_comments = false;
    }
    v.sync_comment_hits(&info);
    assert!(v.session.comments.selection().is_empty());
    assert_eq!(v.session.flash, None);
}

#[test]
fn going_to_a_comment_selects_and_outlines_its_thread() {
    let info = commented(vec![note(0, 0, "plain", None), note(1, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    v.sync_comment_hits(&info);
    assert_eq!(v.session.comments.selection(), vec![(1, 0)]);
    assert_eq!(v.session.flash.map(|(page, ..)| page), Some(1));
    assert_eq!(v.current_comment(), Some(0));
}

#[test]
fn a_hidden_comment_is_gone_to_but_neither_outlined_nor_selected() {
    let info = commented(vec![note(0, 0, "plain", None), note(1, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    v.session.comments.select_keys(&[(0, 0)]);
    v.go_to_comment(0, &info, true);
    v.sync_comment_hits(&info);
    assert!(v.session.comments.selection().is_empty(), "nothing left for Delete");
    assert_eq!(v.session.flash, None);
    assert_eq!(v.current_comment(), Some(0), "still the current result");
}

#[test]
fn the_current_comment_follows_its_thread_when_an_edit_moves_it() {
    let before = commented(vec![note(0, 0, "gone", None), note(0, 1, "zebra", None)]);
    let mut v = searching(&before, "zebra");
    v.go_to_comment(0, &before, false);
    // Deleting the first comment: the app moves the selection with the thread, to index 0.
    let after = commented(vec![note(0, 0, "zebra", None)]);
    v.session.comments.select_only((0, 0));
    v.sync_comment_hits(&after);
    assert_eq!(found(&v), vec![((0, 0), "zebra".to_owned())]);
    assert_eq!(v.current_comment(), Some(0));
}

#[test]
fn a_hidden_comment_is_not_current_after_an_edit_renumbers_the_threads() {
    let before = commented(vec![note(0, 0, "gone", None), note(0, 1, "zebra", None), note(0, 2, "zebra too", None)]);
    let mut v = searching(&before, "zebra");
    v.go_to_comment(0, &before, true);
    // Deleting the first comment: the found thread is now index 0, and index 1 is the other one.
    let after = commented(vec![note(0, 0, "zebra", None), note(0, 1, "zebra too", None)]);
    v.sync_comment_hits(&after);
    assert_eq!(v.current_comment(), None);
}

#[test]
fn a_comment_the_reader_no_longer_selects_is_not_current() {
    let info = commented(vec![note(0, 0, "zebra", None), note(1, 0, "zebra too", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    v.session.comments.select_keys(&[(0, 0), (1, 0)]);
    v.sync_comment_hits(&info);
    assert_eq!(v.current_comment(), None);
}

#[test]
fn a_text_result_leaves_no_comment_selected() {
    let info = commented(vec![note(0, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    if let Some(f) = v.find.as_mut() {
        f.matches.set_page(1, std::iter::once(0..5).collect::<Vec<_>>());
    }
    v.go_to_match(0);
    v.sync_comment_hits(&info);
    assert!(v.session.comments.selection().is_empty());
    assert_eq!(v.current_comment(), None);
    assert_eq!(v.find.as_ref().and_then(|f| f.matches.current()), Some(0));
}

#[test]
fn find_next_onto_page_text_leaves_no_comment_selected() {
    let info = commented(vec![note(0, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    if let Some(f) = v.find.as_mut() {
        f.matches.set_page(1, std::iter::once(0..5).collect::<Vec<_>>());
    }
    v.find_step(true);
    assert!(v.session.comments.selection().is_empty());
    assert_eq!(v.find.as_ref().and_then(|f| f.matches.current()), Some(0));
}

#[test]
fn clearing_the_query_drops_the_comment_find_selected_and_its_outline() {
    let info = commented(vec![note(0, 0, "zebra", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    assert!(v.session.flash.is_some());
    if let Some(f) = v.find.as_mut() {
        f.query.clear();
    }
    v.rerun_find();
    assert!(v.session.comments.selection().is_empty());
    assert_eq!(v.session.flash, None);
}

#[test]
fn a_new_search_releases_only_the_comment_find_selected() {
    let info = commented(vec![note(0, 0, "zebra", None), note(1, 0, "zebra too", None)]);
    let mut v = searching(&info, "zebra");
    v.go_to_comment(0, &info, false);
    v.rerun_find();
    assert!(v.session.comments.selection().is_empty(), "Find's own selection goes");
    assert_eq!(v.session.flash, None, "and so does its outline");

    v.go_to_comment(0, &info, false);
    v.session.comments.select_only((1, 0));
    // A frame of the panel passes before the next query: Find must not take the reader's thread as its own.
    v.sync_comment_hits(&info);
    v.rerun_find();
    assert_eq!(v.session.comments.selection(), vec![(1, 0)], "the reader's own selection stays");
}
