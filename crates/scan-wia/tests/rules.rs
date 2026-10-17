//! The WIA rules of the backend, tested without COM: the HRESULT tables, the sentences, the
//! status bits and the rules that sharpen a vague transfer failure. They come from the craft-scan
//! tests that documented them (`wia_reference`), which keep only the neutral core.

use craft_scan::{HandlingStatus, InterruptKind, ScanError, Source};
use pdfcraft_wia::rules;

mod windows_rules {
    use super::*;
    use craft_scan::InterruptKind as K;
    use rules::BUSY_OR_FEEDER_EMPTY;

    #[test]
    fn a_busy_scanner_is_explained_without_the_hex_code() {
        let text = rules::wia_error_message(0x8021_0006).expect("busy");
        assert!(text.contains("busy"));
        assert!(!text.contains("8021"));
        assert_eq!(rules::describe_windows_error(0x8021_0006, "0x80210006"), text);
        let e_fail = rules::describe_windows_error(0x8000_4005, "");
        assert!(e_fail.contains("interrupted") || e_fail.contains("online"), "{e_fail}");
        assert!(!e_fail.contains("80004005"), "{e_fail}");
        assert_eq!(rules::wia_error_message(0x8021_0002), Some("The scanner has a paper jam."));
        assert_eq!(rules::wia_error_message(0x8021_0003), Some("The feeder has no paper."));
        assert_eq!(rules::wia_error_message(0x8021_0004), Some("The scanner has a paper problem."));
        let offline = rules::wia_error_message(0x8021_0005).expect("offline");
        assert!(offline.contains("offline") && offline.contains("network"), "{offline}");
        let open_fail = rules::wia_error_message(0x8021_0001).expect("open");
        assert!(open_fail.contains("opened") && open_fail.contains("network"), "{open_fail}");
        assert_eq!(rules::wia_error_message(0x8021_0007), Some("The scanner is warming up. Try again in a moment."));
        assert_eq!(rules::wia_error_message(0x8021_0008), Some("The scanner needs attention. Check it, then try again."));
        assert_eq!(rules::wia_error_message(0x8021_0010), Some("A cover on the scanner is open."));
        assert_eq!(rules::wia_error_message(0x8021_0011), Some("The scanner lamp is off."));
        assert_eq!(rules::wia_error_message(0x8021_0014), Some("The feeder took more than one sheet."));
        assert_eq!(rules::describe_windows_error(0x8021_0002, "ignored hex"), "The scanner has a paper jam.");
        let unknown_wia = rules::describe_windows_error(0x8021_00FE, "ok");
        assert!(unknown_wia.contains("0x802100fe") && unknown_wia.contains("scanner"), "{unknown_wia}");
        assert!(rules::describe_windows_error(0x8021_00FE, "The operation completed successfully.").contains("0x802100fe"));
        let jam_soap = rules::describe_windows_error(0x8000_4005, "ClientErrorMediaJam: jam_after_n=2");
        assert!(jam_soap.contains("paper jam"), "{jam_soap}");
        let offline_soap = rules::describe_windows_error(0x8000_4005, "ServerErrorNotAcceptingJobs: offline_after_n=1");
        assert!(offline_soap.contains("offline"), "{offline_soap}");
    }

    #[test]
    fn handling_bits_name_feeder_glass_and_duplex() {
        assert_eq!(rules::handling_caps(3), (true, true, false));
        assert_eq!(rules::handling_caps(7), (true, true, true));
        let status = rules::status_from_bits(0x3F);
        assert!(
            status.feeder_ready == Some(true)
                && status.flat_ready
                && status.duplex_ready
                && status.cover_open
                && status.path_cover_open
                && status.jam
        );
        assert!(!status.multiple_feed && !status.attention && !status.lamp_error);
        let empty = rules::status_from_bits(0);
        assert!(empty.feeder_ready == Some(false) && !empty.jam);
        assert_eq!(rules::acquire_block_reason(status), Some("The scanner has a paper jam."));
        assert_eq!(
            rules::acquire_block_reason(HandlingStatus { cover_open: true, ..HandlingStatus::default() }),
            Some("A cover on the scanner is open.")
        );
        assert_eq!(rules::acquire_block_reason(rules::status_from_bits(0x10)), Some("A cover on the scanner is open."));
        assert_eq!(rules::acquire_block_reason(rules::status_from_bits(0x200)), Some("The feeder took more than one sheet."));
        assert_eq!(rules::acquire_block_reason(rules::status_from_bits(0x800)), Some("The scanner lamp is off."));
        // Attention alone is informational (paused, lamp warning): Windows still transfers.
        assert_eq!(rules::acquire_block_reason(rules::status_from_bits(0x400)), None);
        assert_eq!(rules::acquire_block_reason(rules::status_from_bits(0x400 | 0x800)), Some("The scanner lamp is off."));
        // Jam outranks every other bit.
        assert_eq!(rules::acquire_block_reason(rules::status_from_bits(0x200 | 0x20)), Some("The scanner has a paper jam."));
        assert_eq!(rules::acquire_block_reason(empty), None);
    }

    #[test]
    fn a_generic_download_error_uses_the_status_bit_when_one_is_set() {
        let generic = || ScanError::Failed("The scanner reported an error.".into());
        let jam = HandlingStatus { jam: true, ..HandlingStatus::default() };
        assert!(rules::explain_transfer_failure(generic(), jam, true, 0, 0).to_string().contains("paper jam"));
        let unspecified = ScanError::Failed("Unspecified error".into());
        let cover = HandlingStatus { cover_open: true, ..HandlingStatus::default() };
        assert!(rules::explain_transfer_failure(unspecified, cover, false, 0, 0).to_string().contains("cover"));
        let multi = HandlingStatus { multiple_feed: true, ..HandlingStatus::default() };
        assert!(rules::explain_transfer_failure(generic(), multi, true, 0, 0).to_string().contains("more than one"));
        let lamp = HandlingStatus { lamp_error: true, ..HandlingStatus::default() };
        assert!(
            rules::explain_transfer_failure(ScanError::Failed("The scan failed (0x80004005).".into()), lamp, false, 0, 0)
                .to_string()
                .contains("lamp")
        );
        let named = || ScanError::Failed("The feeder has no paper.".into());
        assert_eq!(rules::explain_transfer_failure(named(), jam, true, 0, 0).to_string(), named().to_string());
        assert!(rules::retry_with_file_transfer(&ScanError::Failed("The parameter is incorrect.".into())));
        assert!(!rules::retry_with_file_transfer(&named()));
        assert_eq!(rules::explain_transfer_failure(generic(), HandlingStatus::default(), false, 0, 0).to_string(), "The scanner reported an error.");
    }

    #[test]
    fn a_feeder_transfer_that_delivered_nothing_and_named_no_fault_is_recognised() {
        let generic = ScanError::Failed("The scanner reported an error.".into());
        let restated = rules::after_transfer(0, false, Some(generic)).expect_err("no pages");
        assert!(rules::no_usable_page(&restated), "{restated}");
        let named = ScanError::Failed("The scanner has a paper jam.".into());
        assert!(!rules::no_usable_page(&named));
    }

    #[test]
    fn a_feeder_transfer_with_nothing_delivered_reads_as_an_empty_tray_unless_the_ready_bit_says_paper() {
        let generic = || ScanError::Failed("The scanner reported an error.".into());
        let unknown = HandlingStatus { feeder_ready: None, ..HandlingStatus::default() };
        let ready = HandlingStatus { feeder_ready: Some(true), ..HandlingStatus::default() };
        let empty = rules::explain_transfer_failure(generic(), unknown, true, 0, 0);
        assert!(empty.to_string().contains("no paper"), "{empty}");
        // A ready bit that says paper (a driver that reports the tray) keeps the generic sentence.
        let kept = rules::explain_transfer_failure(generic(), ready, true, 0, 0);
        assert!(!kept.to_string().contains("no paper"), "{kept}");
        // The glass has no tray to be empty.
        assert!(!rules::explain_transfer_failure(generic(), unknown, false, 0, 0).to_string().contains("no paper"));
    }

    #[test]
    fn an_attention_bit_beats_the_empty_tray_inference() {
        // A door interlock sets the attention bit and clears FEED_READY; it is not an empty tray.
        let interlock = HandlingStatus { attention: true, feeder_ready: Some(false), ..HandlingStatus::default() };
        for (transferred, requested) in [(0, 0), (0, 4), (2, 4)] {
            let explained =
                rules::explain_transfer_failure(ScanError::Failed("The scan failed (0x80004005).".into()), interlock, true, transferred, requested);
            assert_eq!(rules::wia_interrupt_kind(&explained.to_string()), craft_scan::InterruptKind::Attention, "{explained}");
        }
        // Without the attention bit a clear FEED_READY is still an empty tray.
        let empty = HandlingStatus { feeder_ready: Some(false), ..HandlingStatus::default() };
        let explained = rules::explain_transfer_failure(ScanError::Failed("Unspecified error".into()), empty, true, 0, 0);
        assert!(explained.to_string().contains("no paper"), "{explained}");
    }

    #[test]
    fn retry_with_file_transfer_only_for_unnamed_mode_rejection() {
        for text in [
            "The scanner reported an error.",
            "The scanner reported an error (0x802100fe).",
            "The scan failed (0x80004005).",
            "Unspecified error",
            "The parameter is incorrect.",
            "The scan was interrupted. Check for a paper jam, an open cover, or that the scanner stayed online.",
        ] {
            assert!(rules::retry_with_file_transfer(&ScanError::Failed(text.into())), "{text}");
        }
        for text in [
            "The scanner has a paper jam.",
            "A cover on the scanner is open.",
            "The feeder has no paper.",
            "the feeder is empty",
            "The feeder took more than one sheet.",
            "The scanner lamp is off.",
            "The scanner needs attention. Check it, then try again.",
            "The scanner has a paper problem.",
        ] {
            assert!(!rules::retry_with_file_transfer(&ScanError::Failed(text.into())), "{text}");
        }
        // Generic HRESULT + clear FEED_READY on the feeder becomes paper-empty, so no file retry.
        let empty_tray = HandlingStatus { feeder_ready: Some(false), ..HandlingStatus::default() };
        let explained = rules::explain_transfer_failure(ScanError::Failed("Unspecified error".into()), empty_tray, true, 0, 0);
        assert!(explained.to_string().contains("no paper"), "{explained}");
        assert!(!rules::retry_with_file_transfer(&explained));
        // Glass must not treat a clear FEED_READY as an empty tray (status remap leaves it generic).
        assert!(rules::generic_scan_failure(&rules::explain_transfer_failure(
            ScanError::Failed("Unspecified error".into()),
            empty_tray,
            false,
            0,
            0,
        )));
        // Still-ready feeder keeps the generic error so banded→file can retry.
        let ready = HandlingStatus { feeder_ready: Some(true), ..HandlingStatus::default() };
        let keep = rules::explain_transfer_failure(ScanError::Failed("The parameter is incorrect.".into()), ready, true, 0, 4);
        assert!(rules::retry_with_file_transfer(&keep));
        // Jam still outranks a clear ready bit.
        let jammed = HandlingStatus { feeder_ready: Some(false), jam: true, ..HandlingStatus::default() };
        assert!(
            rules::explain_transfer_failure(ScanError::Failed("The scan failed (0x80004005).".into()), jammed, true, 2, 4)
                .to_string()
                .contains("paper jam")
        );
        // Mid-job with fewer pages than requested: do not call it an empty tray.
        let explained_mid = rules::explain_transfer_failure(ScanError::Failed("Unspecified error".into()), empty_tray, true, 2, 4);
        assert!(rules::generic_scan_failure(&explained_mid), "{explained_mid}");
        assert!(rules::after_transfer(2, false, Some(explained_mid)).unwrap_err().to_string().contains("paper jam"));
    }

    #[test]
    fn a_mid_job_jam_is_still_an_error_after_pages_arrive() {
        let jam = || ScanError::Failed("The scanner has a paper jam.".into());
        let empty = || ScanError::Failed("The feeder has no paper.".into());
        let cover = || ScanError::Failed("A cover on the scanner is open.".into());
        let multi = || ScanError::Failed("The feeder took more than one sheet.".into());
        let lamp = || ScanError::Failed("The scanner lamp is off.".into());
        let busy = || ScanError::Failed(rules::wia_error_message(0x8021_0006).expect("busy").into());
        let offline = || ScanError::Failed(rules::wia_error_message(0x8021_0005).expect("offline").into());
        assert!(rules::after_transfer(0, false, Some(jam())).unwrap_err().to_string().contains("paper jam"));
        // Pages stay in the progress token; the dialog shows the jam.
        assert!(rules::after_transfer(2, false, Some(jam())).unwrap_err().to_string().contains("paper jam"));
        assert!(rules::after_transfer(2, false, Some(cover())).unwrap_err().to_string().contains("cover"));
        assert!(rules::after_transfer(1, false, Some(multi())).unwrap_err().to_string().contains("more than one"));
        assert!(rules::after_transfer(2, false, Some(lamp())).unwrap_err().to_string().contains("lamp"));
        // Named busy / offline keep their sentences (not success, not the generic mid-stop rewrite).
        let busy_mid = rules::after_transfer(2, false, Some(busy())).unwrap_err().to_string();
        assert!(busy_mid.contains("busy"), "{busy_mid}");
        assert!(!busy_mid.contains("stopped before"), "{busy_mid}");
        let offline_mid = rules::after_transfer(1, false, Some(offline())).unwrap_err().to_string();
        assert!(offline_mid.contains("offline"), "{offline_mid}");
        assert!(!offline_mid.contains("stopped before"), "{offline_mid}");
        let attention = || ScanError::Failed(rules::wia_error_message(0x8021_0008).expect("attention").into());
        let attention_mid = rules::after_transfer(1, false, Some(attention())).unwrap_err().to_string();
        assert!(attention_mid.contains("interrupted"), "{attention_mid}");
        assert!(attention_mid.contains("stayed online"), "{attention_mid}");
        assert!(rules::after_transfer(0, false, Some(attention())).unwrap_err().to_string().contains("needs attention"));
        assert!(rules::after_transfer(2, false, Some(empty())).is_ok());
        assert!(rules::after_transfer(3, false, Some(ScanError::Failed("the feeder is empty".into()))).is_ok());
        assert!(rules::after_transfer(0, false, Some(empty())).unwrap_err().to_string().contains("no paper"));
        assert!(rules::after_transfer(0, true, None).unwrap_err().to_string().contains("cancelled"));
        assert!(rules::after_transfer(1, true, None).is_ok());
        assert!(rules::after_transfer(0, false, None).unwrap_err().to_string().contains("no pages"));
        // Explained empty-tray end with pages already streamed is success.
        let empty_tray = HandlingStatus { feeder_ready: Some(false), ..HandlingStatus::default() };
        let explained = rules::explain_transfer_failure(ScanError::Failed("The scanner reported an error.".into()), empty_tray, true, 2, 2);
        assert!(rules::after_transfer(2, false, Some(explained)).is_ok());
        // Whole feeder (`pages_requested == 0`): empty tray after some pages is a normal end.
        let explained_whole = rules::explain_transfer_failure(ScanError::Failed("Unspecified error".into()), empty_tray, true, 3, 0);
        assert!(explained_whole.to_string().contains("no paper"), "{explained_whole}");
        assert!(rules::after_transfer(3, false, Some(explained_whole)).is_ok());
        // Sticky FEED_READY after the stack is gone still ends whole-stack acquire.
        let sticky_ready = HandlingStatus { feeder_ready: Some(true), ..HandlingStatus::default() };
        let explained_sticky = rules::explain_transfer_failure(ScanError::Failed("The scan failed (0x80004005).".into()), sticky_ready, true, 3, 0);
        assert!(explained_sticky.to_string().contains("no paper"), "{explained_sticky}");
        assert!(rules::after_transfer(3, false, Some(explained_sticky)).is_ok());
        // Generic E_FAIL with no pages → usable-page / re-Add hint (not raw Unspecified).
        let clarified = rules::after_transfer(0, false, Some(ScanError::Failed("Unspecified error".into()))).unwrap_err();
        assert!(clarified.to_string().contains("usable page") || clarified.to_string().contains("online"), "{clarified}");
        // Generic E_FAIL after some pages → mid-scan drop sentence.
        let mid = rules::after_transfer(2, false, Some(ScanError::Failed("The scan failed (0x80004005).".into()))).unwrap_err();
        assert!(mid.to_string().contains("stopped") || mid.to_string().contains("online"), "{mid}");
    }

    #[test]
    fn wsd_feeder_skips_empty_tray_preflight() {
        assert!(!rules::feeder_empty_blocks_acquire(Source::Feeder, false, true));
        assert!(rules::feeder_empty_blocks_acquire(Source::Feeder, false, false));
        assert!(!rules::feeder_empty_blocks_acquire(Source::Feeder, true, false));
        assert!(!rules::feeder_empty_blocks_acquire(Source::Glass, false, false));
    }

    #[test]
    fn wsd_whole_feeder_uses_a_large_ticket_page_count() {
        assert_eq!(rules::wsd_feeder_ticket_pages(0), i32::try_from(craft_scan::MAX_PAGES).unwrap_or(i32::MAX));
        assert_eq!(rules::wsd_feeder_ticket_pages(3), 3);
    }

    #[test]
    fn interrupt_kind_follows_the_engine_sentences() {
        for (code, kind) in [
            (0x8021_0002, K::Jam),
            (0x8021_0004, K::PaperProblem),
            (0x8021_0010, K::CoverOpen),
            (0x8021_0014, K::MultiFeed),
            (0x8021_0003, K::FeederEmpty),
            (0x8021_0005, K::Offline),
            (0x8021_000A, K::Offline),
            (0x8021_0008, K::Attention),
            (0x8021_0006, K::Busy),
            (0x8021_0007, K::Busy),
            (0x8021_000D, K::Busy),
            (0x8021_0011, K::DeviceError),
            (0x8021_000E, K::DeviceError),
        ] {
            assert_eq!(rules::wia_interrupt_kind(rules::wia_error_message(code).expect("sentence")), kind, "{code:#x}");
        }
    }

    #[test]
    fn a_double_feed_after_page_one_is_not_called_a_jam() {
        let problem = || ScanError::Failed(rules::describe_windows_error(0x8021_0004, ""));
        assert_eq!(problem().to_string(), "The scanner has a paper problem.");
        // No bit names a cause: stay neutral.
        let none = rules::explain_transfer_failure(problem(), HandlingStatus::default(), true, 1, 3);
        assert_eq!(rules::wia_interrupt_kind(&none.to_string()), craft_scan::InterruptKind::PaperProblem);
        // The double-feed bit read after the transfer is used.
        let multi = rules::explain_transfer_failure(problem(), rules::status_from_bits(0x200 | 0x1), true, 1, 3);
        assert_eq!(rules::wia_interrupt_kind(&multi.to_string()), craft_scan::InterruptKind::MultiFeed);
        // A real jam bit still says jam; a cover says cover.
        let jam = rules::explain_transfer_failure(problem(), rules::status_from_bits(0x20), true, 1, 3);
        assert_eq!(rules::wia_interrupt_kind(&jam.to_string()), craft_scan::InterruptKind::Jam);
        let cover = rules::explain_transfer_failure(problem(), rules::status_from_bits(0x8), true, 1, 3);
        assert_eq!(rules::wia_interrupt_kind(&cover.to_string()), craft_scan::InterruptKind::CoverOpen);
        // The wire message names the fault: it beats the folded HRESULT.
        assert_eq!(
            rules::wia_interrupt_kind(&rules::describe_windows_error(0x8021_0004, "ClientErrorMultiFeed: sheets stuck")),
            craft_scan::InterruptKind::MultiFeed
        );
        assert_eq!(
            rules::wia_interrupt_kind(&rules::describe_windows_error(0x8021_0004, "ClientErrorMediaJam: jam_after_n=2")),
            craft_scan::InterruptKind::Jam
        );
    }

    #[test]
    fn feeder_busy_with_no_naming_bit_is_busy_or_empty() {
        let busy = || ScanError::Failed(rules::wia_error_message(0x8021_0006).expect("busy").into());
        let kind = |e: ScanError| e.interrupt_kind();
        // Feeder, busy, ready bit set, nothing else: the new sentence, with or without earlier pages.
        let none = rules::explain_transfer_failure(busy(), rules::status_from_bits(0x1), true, 0, 0);
        assert_eq!(none.to_string(), BUSY_OR_FEEDER_EMPTY);
        assert_eq!(kind(none), K::BusyOrFeederEmpty);
        let some = rules::explain_transfer_failure(busy(), HandlingStatus::default(), true, 2, 5);
        assert_eq!(kind(some), K::BusyOrFeederEmpty);
        // With pages in hand the stack ends normally for a whole-feeder scan; a fixed count reports the shortfall.
        assert!(rules::after_transfer_checked(0, true, 2, false, Some(ScanError::Failed(BUSY_OR_FEEDER_EMPTY.into()))).is_ok());
        let short = rules::after_transfer_checked(5, true, 2, false, Some(ScanError::Failed(BUSY_OR_FEEDER_EMPTY.into())));
        assert_eq!(short.err().map(|e| e.interrupt_kind()), Some(K::FeederEmpty));
        // With no pages it stays an error.
        assert!(rules::after_transfer_checked(0, true, 0, false, Some(ScanError::Failed(BUSY_OR_FEEDER_EMPTY.into()))).is_err());
        // A naming bit wins.
        let jam = rules::explain_transfer_failure(busy(), rules::status_from_bits(0x20), true, 0, 0);
        assert_eq!(kind(jam), K::Jam);
        let attention = rules::explain_transfer_failure(busy(), rules::status_from_bits(0x400 | 0x1), true, 0, 0);
        assert_eq!(kind(attention), K::Attention);
        // The glass is unchanged.
        let glass = rules::explain_transfer_failure(busy(), HandlingStatus::default(), false, 0, 0);
        assert_eq!(glass.to_string(), busy().to_string());
        assert_eq!(kind(glass), K::Busy);
        assert_eq!(rules::wia_interrupt_kind(BUSY_OR_FEEDER_EMPTY), K::BusyOrFeederEmpty);
        assert_eq!(rules::wia_interrupt_kind("Example: The scanner is busy, or the feeder has no paper."), K::BusyOrFeederEmpty);
    }

    #[test]
    fn attention_alone_never_blocks_but_explains_a_failure() {
        let attention = rules::status_from_bits(0x400 | 0x3);
        assert_eq!(rules::acquire_block_reason(attention), None);
        // A generic failure with the attention bit set (door interlock, full store) says so.
        let generic = ScanError::Failed(rules::describe_windows_error(0x8000_4005, ""));
        let explained = rules::explain_transfer_failure(generic, attention, true, 0, 3);
        assert_eq!(rules::wia_interrupt_kind(&explained.to_string()), craft_scan::InterruptKind::Attention);
        assert!(!rules::retry_with_file_transfer(&explained));
        // Without the bit the failure stays generic.
        let plain = ScanError::Failed(rules::describe_windows_error(0x8000_4005, ""));
        assert!(rules::generic_scan_failure(&rules::explain_transfer_failure(plain, rules::status_from_bits(0x3), true, 0, 3)));
        // A finished whole-stack scan with the bit set (paused) still ends normally.
        let ended = ScanError::Failed("The scan failed (0x80004005).".into());
        assert!(rules::explain_transfer_failure(ended, attention, true, 2, 0).to_string().contains("no paper"));
    }

    #[test]
    fn only_documented_codes_have_a_name_and_the_names_are_microsofts() {
        assert_eq!(rules::code_name(0x8021_0006), Some("WIA_ERROR_BUSY"));
        assert_eq!(rules::code_name(0x8021_0016), Some("WIA_ERROR_COVER_OPEN"));
        assert_eq!(rules::CODE_NAMES.len(), 22);
        for (code, _) in rules::CODE_NAMES.iter().filter(|(code, _)| *code != 0x8000_4005 && *code != 0x8007_0057) {
            assert!(rules::wia_error_message(*code).is_some(), "{code:#x} has no sentence");
        }
        // Driver-observed, undocumented: no name, but the kind and the sentence stay.
        for (code, kind) in
            [(0x8021_0010, InterruptKind::CoverOpen), (0x8021_0011, InterruptKind::DeviceError), (0x8021_0014, InterruptKind::MultiFeed)]
        {
            assert_eq!(rules::code_name(code), None, "{code:#x}");
            let info = rules::from_hresult(code, "").info(craft_scan::ErrorContext::default());
            assert_eq!((info.hresult(), info.code_name(), info.kind), (Some(code), None, kind));
        }
        assert_eq!(rules::kind_for_hresult(0x8021_0015), Some(InterruptKind::Offline));
    }

    #[test]
    fn a_hresult_error_carries_its_code_name_and_kind_without_reading_the_sentence() {
        let error = rules::from_hresult(0x8021_0006, "some driver text that names nothing");
        let info = error.info(craft_scan::ErrorContext::default());
        assert_eq!((info.hresult(), info.code_name(), info.kind), (Some(0x8021_0006), Some("WIA_ERROR_BUSY"), InterruptKind::Busy));
        assert_eq!(info.hresult_hex().as_deref(), Some("0x80210006"));
        // A sentence chosen from a status read keeps the HRESULT that triggered it.
        let busy = rules::from_hresult(0x8021_0006, "");
        let explained = rules::explain_transfer_failure(busy, HandlingStatus { feeder_ready: Some(true), ..HandlingStatus::default() }, true, 0, 1);
        let info = explained.info(craft_scan::ErrorContext::default());
        assert_eq!((info.kind, info.code_name()), (InterruptKind::BusyOrFeederEmpty, Some("WIA_ERROR_BUSY")));
        let jam = rules::explain_transfer_failure(rules::from_hresult(0x8021_0004, ""), rules::status_from_bits(0x20), true, 0, 1);
        assert_eq!((jam.interrupt_kind(), jam.hresult()), (InterruptKind::Jam, Some(0x8021_0004)));
    }

    #[test]
    fn wia_sentences_map_back_to_the_code_they_came_from() {
        for (code, kind) in rules::KIND_BY_CODE {
            if let Some(sentence) = rules::wia_error_message(code) {
                assert_eq!(rules::wia_interrupt_kind(sentence), kind, "{code:#x}");
            }
        }
    }
}

mod registration {
    use super::*;

    #[test]
    fn the_hresult_table_is_added_to_the_contract_dump() {
        let dump = rules::contract_json();
        assert!(dump.starts_with("{\"kinds\":[") && dump.ends_with("}}"), "{dump}");
        assert!(dump.contains("\"hresults\":{\"0x80210001\":\"WIA_ERROR_GENERAL_ERROR\""), "{dump}");
        assert!(dump.contains("\"0x80210006\":\"WIA_ERROR_BUSY\""), "{dump}");
        assert!(craft_scan::contract_json().len() < dump.len());
    }

    #[test]
    fn the_native_code_is_an_hresult_that_the_info_reports_under_the_old_names() {
        let info = rules::from_hresult(0x8021_0020, "").info(craft_scan::ErrorContext::default());
        assert_eq!(info.native.map(|native| (native.domain, native.code)), Some(("hresult", 0x8021_0020)));
        assert_eq!((info.hresult_hex().as_deref(), info.code_name()), (Some("0x80210020"), Some("WIA_ERROR_MULTI_FEED")));
    }

    #[test]
    fn registering_twice_leaves_one_backend() {
        let first = pdfcraft_wia::register();
        let second = pdfcraft_wia::register();
        assert_eq!(first, cfg!(windows));
        assert_eq!(second, cfg!(windows));
        let names = craft_scan::backend_names();
        assert_eq!(names.iter().filter(|name| **name == pdfcraft_wia::NAME).count(), usize::from(cfg!(windows)));
        assert_eq!(craft_scan::supported(), cfg!(windows));
    }
}

mod page_size_per_source {
    use super::*;
    use craft_scan::PageSize as P;
    use rules::{PageSizeOmit, PageSizePlan};

    /// The page-size list the inbox WSD driver published for the feeder item of a letter-only tray
    /// (measured: custom, a4, letter and business card are known names, legal is absent).
    const TRAY5_FEEDER: [P; 4] = [P::Custom, P::A4, P::Letter, P::BusinessCard];

    #[test]
    fn an_item_without_the_property_gets_no_write() {
        // The glass of the inbox WSD driver publishes no page size: unsupported means omitted.
        assert_eq!(rules::plan_page_size(P::A4, None), PageSizePlan::Omit(PageSizeOmit::NotPublished));
        assert_eq!(rules::plan_page_size(P::A4, Some(&[])), PageSizePlan::Omit(PageSizeOmit::NotPublished));
    }

    #[test]
    fn a_listed_size_is_tried_first_and_the_covering_sizes_follow() {
        assert_eq!(rules::plan_page_size(P::Letter, Some(&TRAY5_FEEDER)), PageSizePlan::Try(vec![P::Letter]));
        // A4 is listed on the feeder, then nothing listed covers it (letter is shorter).
        assert_eq!(rules::plan_page_size(P::A4, Some(&TRAY5_FEEDER)), PageSizePlan::Try(vec![P::A4]));
        // A business card is listed and every larger listed sheet covers it, smallest first.
        assert_eq!(rules::plan_page_size(P::BusinessCard, Some(&TRAY5_FEEDER)), PageSizePlan::Try(vec![P::BusinessCard, P::Letter, P::A4]));
    }

    #[test]
    fn an_unlisted_size_falls_to_the_nearest_listed_sheet_that_covers_it() {
        let wide = [P::Custom, P::Letter, P::Legal];
        // Letter asked for on a list without it: legal covers it.
        assert_eq!(rules::plan_page_size(P::Letter, Some(&[P::Custom, P::Legal])), PageSizePlan::Try(vec![P::Legal]));
        // A4 asked for: letter is too short, legal covers it.
        assert_eq!(rules::plan_page_size(P::A4, Some(&wide)), PageSizePlan::Try(vec![P::Legal]));
        // Legal asked for on a letter-only list: nothing covers it, the area stays the item default.
        assert_eq!(rules::plan_page_size(P::Legal, Some(&TRAY5_FEEDER)), PageSizePlan::Omit(PageSizeOmit::NotOffered));
    }

    #[test]
    fn feeder_and_glass_lists_decide_separately() {
        // Same request, two items: the feeder is written, the glass is not.
        assert!(matches!(rules::plan_page_size(P::A4, Some(&TRAY5_FEEDER)), PageSizePlan::Try(_)));
        assert_eq!(rules::plan_page_size(P::A4, None), PageSizePlan::Omit(PageSizeOmit::NotPublished));
    }

    #[test]
    fn custom_and_auto_are_only_written_when_listed() {
        assert_eq!(rules::plan_page_size(P::Custom, Some(&TRAY5_FEEDER)), PageSizePlan::Try(vec![P::Custom]));
        assert_eq!(rules::plan_page_size(P::Auto, Some(&TRAY5_FEEDER)), PageSizePlan::Omit(PageSizeOmit::NotOffered));
        assert_eq!(rules::plan_page_size(P::Auto, Some(&[P::Auto, P::Custom])), PageSizePlan::Try(vec![P::Auto]));
    }

    #[test]
    fn the_notes_say_what_was_scanned() {
        assert!(rules::page_size_note(P::A4, Some(P::Letter), None).contains("scanned as letter"));
        assert!(rules::page_size_note(P::A4, None, Some(PageSizeOmit::Rejected)).contains("full area"));
        assert!(rules::page_size_note(P::A4, None, Some(PageSizeOmit::NotPublished)).contains("no page size to set"));
    }

    #[test]
    fn a_refused_property_is_named_in_words_with_the_driver_text_as_detail() {
        let text = rules::rejected_property_message(3097, "The parameter is incorrect.");
        assert!(text.contains("page size"), "{text}");
        assert!(text.ends_with("(The parameter is incorrect.)"), "{text}");
        assert!(!text.contains("3097"), "{text}");
        assert!(rules::rejected_property_message(99_999, "x").contains("property 99999"));
    }
}
