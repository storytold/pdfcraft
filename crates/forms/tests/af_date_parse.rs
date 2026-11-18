use pdfcraft_forms::af::parse_date;

#[test]
fn bare_a_suffix_is_am() {
    assert_eq!(parse_date("12:05 a", "h:MM t").map(|d| d.hh), Some(0));
    assert_eq!(parse_date("12:30a", "h:MM t").map(|d| d.hh), Some(0));
    assert_eq!(parse_date("5:30 a", "h:MM t").map(|d| d.hh), Some(5));
    assert_eq!(parse_date("5:30p", "h:MM t").map(|d| d.hh), Some(17));
    assert_eq!(parse_date("12:05 p", "h:MM t").map(|d| d.hh), Some(12));
}
