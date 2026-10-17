//! Regression coverage for #2664 using a deterministic local clock.
use pdfcraft_engine::Session;

#[test]
fn brazilian_dates_default_to_day_first_without_overriding_explicit_patterns() {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    assert_eq!(session.today_text(None, Some("pt-br")).unwrap(), "14/11/2023");
    session.set_date_language(Some("PT-BR")).unwrap();
    assert_eq!(session.date_format(), "dd/mm/yyyy");
    assert_eq!(session.today_text(None, None).unwrap(), "14/11/2023");
    session.set_date_format("m/d/yyyy").unwrap();
    assert_eq!(session.today_text(None, None).unwrap(), "11/14/2023");
    session.set_date_language(Some("en")).unwrap();
    session.set_date_language(Some("pt-br")).unwrap();
    assert_eq!(session.today_text(None, None).unwrap(), "11/14/2023");
    assert_eq!(session.today_text(Some("yyyy-mm-dd"), None).unwrap(), "2023-11-14");
    assert!(session.set_date_format("HH:MM").is_err());
    assert_eq!(session.today_text(None, None).unwrap(), "11/14/2023");
}

#[test]
fn automatic_defaults_can_be_restored_and_unknown_locales_keep_the_fallback() {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    assert_eq!(session.today_text(None, Some("unknown")).unwrap(), "11/14/2023");
    session.set_date_language(Some("pt-br")).unwrap();
    session.set_date_format("yyyy-mm-dd").unwrap();
    session.set_date_format(" AUTO ").unwrap();
    assert_eq!(session.date_format_override(), None);
    assert_eq!(session.today_text(None, None).unwrap(), "14/11/2023");
    assert_eq!(session.today_text(None, Some("en")).unwrap(), "11/14/2023");
    session.set_date_language(None).unwrap();
    assert_eq!(session.date_format(), "m/d/yyyy");
}
