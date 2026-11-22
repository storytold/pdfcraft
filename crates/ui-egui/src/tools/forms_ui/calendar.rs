//! A date field's calendar under the field, as the framework's `CalendarPopover`.

use craft::ui::contract::overlay::{Anchor, Dismiss};
use craft::ui::contract::{Bind, ShowContainer as _};
// PLAN: assumed `craft::ui::pickers` re-exports `CalendarPopover`, `DatePrecision` and `PickedDate`.
use craft::ui::pickers::{CalendarPopover, DatePrecision, PickedDate};
use craft::ui::runtime::focus::{FocusPolicy, Initial};
use egui::Rect;
use pdfcraft_engine::form_scripts::{DateTime, format_date, parse_date};

/// `text`'s date, as the calendar shows it.
fn picked_date(text: &str, fmt: &str) -> Option<PickedDate> {
    let d = parse_date(text, fmt)?;
    Some(PickedDate::day(d.y, u8::try_from(d.m).ok()?, u8::try_from(d.d).ok()?))
}

/// The calendar of a date field with format `fmt`, hung from `rect` and open while `open` is true. A day picked comes
/// back written in the field's own format, so the caller commits it. Today comes from the session clock; the
/// words follow the interface language.
pub(super) fn calendar(ui: &mut egui::Ui, rect: Rect, text: &str, fmt: &str, today: (i64, u32, u32), open: &mut bool) -> Option<String> {
    let before = picked_date(text, fmt);
    let mut picked = before.clone();
    let today = PickedDate::day(i32::try_from(today.0).unwrap_or(1970), u8::try_from(today.1).unwrap_or(1), u8::try_from(today.2).unwrap_or(1));
    CalendarPopover::new("Calendar", Bind::bound(&mut *open), Bind::bound(&mut picked))
        .precisions(&[DatePrecision::Day])
        .first_weekday(craft::i18n::Weekday::Sunday)
        .today(today)
        .overlay(|overlay| {
            overlay
                .anchor(Anchor::Rect(rect))
                .focus(FocusPolicy::POPUP.trap(false).restore(false).initial(Initial::KeepAnchor))
                .dismiss(Dismiss::new().focus_left(false))
        })
        .show(ui, |_, _| {});
    if picked == before {
        return None;
    }
    let Some(PickedDate { year, month: Some(m), day: Some(d), .. }) = picked else { return None };
    Some(format_date(DateTime { y: year, m: u32::from(m), d: u32::from(d), hh: 0, mm: 0, ss: 0 }, fmt))
}
