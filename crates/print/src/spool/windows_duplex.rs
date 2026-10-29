//! Resolve Printer default from the Windows driver's current DEVMODE. Omitting duplex
//! from a PrintTicket leaves its interpretation to the XPS driver (#5130).

use std::borrow::Cow;

use super::{Duplex, Job};
use crate::PrintError;

/// Preserve an explicit duplex choice by borrowing `job` without calling `read`.
/// Otherwise call `read` once with the selected printer (or `None` for the system default),
/// returning an owned copy with the resolved printer and duplex value. Log lookup failures
/// and leave duplex unspecified so the spooler can use its previous default behavior;
/// the original job remains unchanged even when the driver has no duplex setting.
fn resolve<'a>(job: &'a Job, read: impl FnOnce(Option<&str>) -> Result<(String, Option<Duplex>), PrintError>) -> Cow<'a, Job> {
    if job.duplex.is_some() {
        return Cow::Borrowed(job);
    }
    let (printer, duplex) = match read(job.printer.as_deref()) {
        Ok(settings) => settings,
        Err(error) => {
            driver_default(Err(error));
            return Cow::Borrowed(job);
        }
    };
    let mut resolved = job.clone();
    resolved.printer = Some(printer);
    resolved.duplex = duplex;
    Cow::Owned(resolved)
}

/// A driver default is best-effort: record the fallback and let the driver choose.
fn driver_default(result: Result<Option<Duplex>, PrintError>) -> Option<Duplex> {
    match result {
        Ok(value) => value,
        Err(error) => {
            log::warn!("could not resolve the printer's two-sided default; leaving duplex unspecified: {error}");
            None
        }
    }
}

/// DEVMODEW's public dmFields (offset 72) marks dmDuplex (offset 94) as valid with
/// DM_DUPLEX. Values are DMDUP_SIMPLEX, DMDUP_VERTICAL and DMDUP_HORIZONTAL, respectively.
/// Return `None` when DM_DUPLEX is absent, or a spool error for a malformed field or an
/// unrecognized declared value. Layout: <https://learn.microsoft.com/windows/win32/api/wingdi/ns-wingdi-devmodew>.
fn duplex(settings: &craft_print::rgb::DevMode) -> Result<Option<Duplex>, PrintError> {
    let bytes = settings.as_bytes();
    let invalid = || PrintError::Spool("the printer driver returned an invalid two-sided default; check its printing preferences".into());
    let fields = bytes.get(72..76).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes).ok_or_else(invalid)?;
    if fields & 0x1000 == 0 {
        return Ok(None);
    }
    match bytes.get(94..96).and_then(|b| <[u8; 2]>::try_from(b).ok()).map(i16::from_le_bytes) {
        Some(1) => Ok(Some(Duplex::Off)),
        Some(2) => Ok(Some(Duplex::LongEdge)),
        Some(3) => Ok(Some(Duplex::ShortEdge)),
        _ => Err(invalid()),
    }
}

#[cfg(windows)]
/// Resolve an unspecified duplex setting from the selected Windows printer immediately
/// before submission. An unnamed printer is resolved to the system default and pinned to
/// the returned job. Explicit choices bypass the driver lookup. Lookup failures are logged
/// and leave duplex unspecified; a settings failure retains the resolved queue.
pub(super) fn job(job: &Job) -> Cow<'_, Job> {
    resolve(job, |selected| {
        let failed = |e| PrintError::Spool(format!("could not read the printer's two-sided default: {e}"));
        let printer = match selected {
            Some(name) => name.to_owned(),
            None => craft_print::rgb::default_printer()
                .map_err(failed)?
                .ok_or_else(|| PrintError::Spool("there is no default printer; choose a printer".into()))?,
        };
        let value = craft_print::rgb::default_settings(&printer).map_err(failed).and_then(|settings| duplex(&settings));
        Ok((printer, driver_default(value)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(fields: u32, value: i16) -> craft_print::rgb::DevMode {
        let mut bytes = vec![0; 220];
        bytes[68..70].copy_from_slice(&220u16.to_le_bytes());
        bytes[72..76].copy_from_slice(&fields.to_le_bytes());
        bytes[94..96].copy_from_slice(&value.to_le_bytes());
        craft_print::rgb::DevMode::from_bytes(bytes).unwrap()
    }

    #[test]
    fn printer_default_reaches_the_windows_job_for_each_binding() {
        for (value, expected) in [(1, Duplex::Off), (2, Duplex::LongEdge), (3, Duplex::ShortEdge)] {
            let original = Job {
                printer: Some("Office".into()),
                copies: 3,
                collate: false,
                grayscale: true,
                title: "report".into(),
                options: vec![("media".into(), "A4".into())],
                ..Job::default()
            };
            let resolved = resolve(&original, |selected| {
                assert_eq!(selected, Some("Office"));
                Ok(("Office".into(), duplex(&settings(0x1000, value))?))
            });
            assert_eq!(resolved.duplex, Some(expected));
            assert_eq!(resolved.printer.as_deref(), Some("Office"));
            assert_eq!(resolved.copies, 3);
            assert_eq!(resolved.collate, original.collate);
            assert_eq!(resolved.grayscale, original.grayscale);
            assert_eq!(resolved.scaling, original.scaling);
            assert_eq!(resolved.options, original.options);
            assert_eq!(resolved.title, "report");
            assert_eq!(original.duplex, None, "Printer default remains selected for the next job");
        }
    }

    #[test]
    fn explicit_choices_do_not_read_or_change_the_printer_default() {
        for choice in [Duplex::Off, Duplex::LongEdge, Duplex::ShortEdge] {
            let original = Job { duplex: Some(choice), ..Job::default() };
            let resolved = resolve(&original, |_| panic!("explicit choice must skip driver settings"));
            assert!(matches!(resolved, Cow::Borrowed(_)));
            assert_eq!(resolved.duplex, Some(choice));
        }
    }

    #[test]
    fn system_default_queue_is_resolved_once_and_pinned_to_the_job() {
        let original = Job::default();
        let resolved = resolve(&original, |selected| {
            assert_eq!(selected, None);
            Ok(("System default".into(), Some(Duplex::ShortEdge)))
        });
        assert_eq!(resolved.printer.as_deref(), Some("System default"));
        assert_eq!(resolved.duplex, Some(Duplex::ShortEdge));
    }

    #[test]
    fn missing_duplex_field_is_left_to_the_driver_and_invalid_values_fail() {
        assert_eq!(duplex(&settings(0, 99)).unwrap(), None);
        for value in [0, -1, 4, i16::MAX] {
            assert!(duplex(&settings(0x1000, value)).is_err());
        }
        let original = Job::default();
        let resolved = resolve(&original, |_| Ok(("Simplex".into(), None)));
        assert_eq!(resolved.duplex, None);
    }

    #[test]
    fn a_queue_lookup_failure_preserves_the_original_job() {
        let original = Job::default();
        let resolved = resolve(&original, |_| Err(PrintError::Spool("driver unavailable".into())));
        assert!(matches!(resolved, Cow::Borrowed(_)));
        assert_eq!(resolved.duplex, None);
        assert_eq!(resolved.printer, original.printer);
    }

    #[test]
    fn failed_or_invalid_settings_fall_back_and_keep_the_resolved_queue() {
        for value in [Err(PrintError::Spool("driver unavailable".into())), duplex(&settings(0x1000, 99))] {
            let original = Job { copies: 3, ..Job::default() };
            let resolved = resolve(&original, |_| Ok(("Resolved queue".into(), driver_default(value))));
            assert_eq!(resolved.duplex, None);
            assert_eq!(resolved.printer.as_deref(), Some("Resolved queue"));
            assert_eq!(resolved.copies, 3);
            assert_eq!(original.printer, None);
        }
    }

    /// Reads settings only: never submits a job or changes the printer's preferences.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs PDFCRAFT_PRINT_TEST_PRINTER naming an installed Windows printer; sends no job"]
    fn installed_windows_printer_default_is_carried_explicitly() {
        let printer = std::env::var("PDFCRAFT_PRINT_TEST_PRINTER").unwrap();
        let original = Job { printer: Some(printer.clone()), ..Job::default() };
        let expected = duplex(&craft_print::rgb::default_settings(&printer).unwrap()).unwrap();
        let resolved = job(&original);
        assert_eq!(resolved.printer.as_deref(), Some(printer.as_str()));
        assert_eq!(resolved.duplex, expected);
        eprintln!("native Windows default: {expected:?}; resolved job: {:?}", resolved.duplex);
    }
}
