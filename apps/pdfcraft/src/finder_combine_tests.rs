//! The Quick Action's list: file names with a newline or trailing spaces stay whole.

use super::*;

#[test]
fn a_name_with_a_newline_stays_one_path() {
    let text = "/tmp/report\n2026.pdf\0/tmp/trailing  \0/tmp/plain.pdf\0";
    assert_eq!(parse_list(text), vec!["/tmp/report\n2026.pdf".to_string(), "/tmp/trailing  ".to_string(), "/tmp/plain.pdf".to_string()]);
}

#[test]
fn a_list_from_an_earlier_quick_action_is_still_read_by_line() {
    assert_eq!(parse_list("/a/b.pdf\n\n/c/d e.pdf  \n"), vec!["/a/b.pdf".to_string(), "/c/d e.pdf".to_string()]);
    assert!(parse_list("").is_empty());
}
