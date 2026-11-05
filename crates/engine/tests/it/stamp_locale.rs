//! Dynamic stamp by-lines share the session's local clock and language in UI and automation.

use pdfcraft_engine::Session;

#[test]
fn brazilian_portuguese_stamps_use_localized_words_and_24_hour_time() {
    // 15:45 on 15 October 2026, taken as local time by the fixed test clock.
    let mut session = Session::new().with_clock(|| 1_792_079_100);
    session.set_date_language(Some("PT-BR")).unwrap();
    assert_eq!(session.stamp_by_line("  Ana  "), "Por Ana às 15:45, 15/10/2026");
    assert_eq!(session.stamp_by_line("  "), "às 15:45, 15/10/2026");
    session.set_date_format("yyyy.mm.dd.").unwrap();
    assert_eq!(session.stamp_by_line("Ana"), "Por Ana às 15:45, 15/10/2026", "Fill & Sign's custom pattern does not change stamps");
}

#[test]
fn stamp_language_keeps_the_local_day_and_leaves_the_file_clock_in_utc() {
    // 00:30Z on the 15th is 21:30 on the 14th in São Paulo.
    let mut session = Session::new().with_wall_clock(|| 1_792_024_200, || -3 * 3600);
    session.set_date_language(Some("pt-br")).unwrap();
    assert_eq!(session.stamp_by_line("Ana"), "Por Ana às 21:30, 14/10/2026");
    assert_eq!(session.now_secs(), 1_792_024_200);
    assert_eq!(session.signing_date(), "D:20261014213000-03'00'");
}

#[test]
fn english_stamps_keep_their_existing_format() {
    let session = Session::new().with_clock(|| 1_792_079_100);
    assert_eq!(session.stamp_by_line("  Ada  "), "By Ada at 3:45 pm, Oct 15, 2026");
    assert_eq!(session.stamp_by_line("  "), "at 3:45 pm, Oct 15, 2026");
}

#[test]
fn callers_can_follow_the_interface_without_changing_the_date_preference() {
    let mut session = Session::new().with_clock(|| 1_792_079_100);
    assert_eq!(session.stamp_by_line_in_language("Ana", Some(" PT-BR ")), "Por Ana às 15:45, 15/10/2026");
    assert_eq!(session.date_language(), None);
    session.set_date_language(Some("pt-br")).unwrap();
    assert_eq!(session.stamp_by_line_in_language("Ada", Some("en")), "By Ada at 3:45 pm, Oct 15, 2026");
    assert_eq!(session.date_language(), Some("pt-br"));
    for language in ["fr", "xx", ""] {
        assert_eq!(session.stamp_by_line_in_language("Ada", Some(language)), "By Ada at 3:45 pm, Oct 15, 2026");
    }
}

#[test]
fn midnight_and_noon_do_not_mix_12_and_24_hour_clocks() {
    for (session, portuguese, english) in [
        (Session::new().with_clock(|| 1_792_022_700), "às 00:05, 15/10/2026", "at 12:05 am, Oct 15, 2026"),
        (Session::new().with_clock(|| 1_792_065_600), "às 12:00, 15/10/2026", "at 12:00 pm, Oct 15, 2026"),
    ] {
        assert_eq!(session.stamp_by_line_in_language("", Some("pt-br")), portuguese);
        assert_eq!(session.stamp_by_line(""), english);
    }
}

#[test]
fn hostile_injected_clock_offsets_do_not_panic() {
    for session in [Session::new().with_wall_clock(|| i64::MAX, || i64::MAX), Session::new().with_wall_clock(|| i64::MIN, || i64::MIN)] {
        assert!(!session.stamp_by_line_in_language("Ana", Some("pt-br")).is_empty());
        assert!(!session.stamp_by_line("Ada").is_empty());
    }
}
