//! Exhaustive checks of the WIA rule tables, without COM: every row of the HRESULT tables, every
//! combination of the document-handling status bits, and the whole grid of transfer outcomes. The
//! example tests in `rules.rs` document individual cases; these make sure a new code, a reordered
//! rule or a changed sentence cannot leave the tables disagreeing with each other.

use std::collections::HashSet;

use craft_scan::{HandlingStatus, InterruptKind, PageSize, ScanError, Source};
use pdfcraft_wia::rules::{self, PageSizeOmit, PageSizePlan};

// ---- the HRESULT tables ------------------------------------------------------------------------

#[test]
fn no_code_appears_twice_in_a_table() {
    let mut names = HashSet::new();
    for (code, name) in rules::CODE_NAMES {
        assert!(names.insert(code), "{code:#010x} is named twice");
        assert!(name.starts_with("WIA_") || name.starts_with("E_"), "{name} is not a Microsoft name");
    }
    let mut kinds = HashSet::new();
    for (code, _) in rules::KIND_BY_CODE {
        assert!(kinds.insert(code), "{code:#010x} has two kinds");
    }
}

#[test]
fn every_classified_code_has_a_sentence_and_an_error_of_its_kind() {
    for (code, kind) in rules::KIND_BY_CODE {
        let sentence = rules::wia_error_message(code).unwrap_or_else(|| panic!("{code:#010x} has a kind but no sentence"));
        assert!(!sentence.is_empty());
        let error = rules::from_hresult(code, "");
        // PAPER_PROBLEM is the one code whose sentence may sharpen the kind; with no driver text it stays.
        assert_eq!(error.interrupt_kind(), kind, "{code:#010x}: from_hresult gives another kind");
        let info = error.native().unwrap_or_else(|| panic!("{code:#010x} lost its native code"));
        assert_eq!((info.domain, info.code), ("hresult", i64::from(code)), "{code:#010x}");
        assert_eq!(info.name, rules::code_name(code), "{code:#010x}: the code name travels with the error");
        let text = error.to_string();
        // A classified code reads as a sentence; the number stays in the error's native code, not in the text.
        assert!(!text.contains("0x") && text == sentence, "{code:#010x}: {text}");
    }
}

#[test]
fn every_sentence_reads_back_as_the_kind_of_a_code_that_produces_it() {
    // A sentence shared by two codes (a cover, the lamp, a double feed) must stand for one kind.
    for (code, kind) in rules::KIND_BY_CODE {
        let Some(sentence) = rules::wia_error_message(code) else { continue };
        assert_eq!(rules::wia_interrupt_kind(sentence), kind, "{code:#010x}: {sentence:?} reads back as another kind");
    }
}

#[test]
fn a_named_code_without_a_kind_is_still_an_error_with_its_name() {
    let classified: HashSet<u32> = rules::KIND_BY_CODE.iter().map(|(code, _)| *code).collect();
    for (code, name) in rules::CODE_NAMES {
        if classified.contains(&code) {
            continue;
        }
        let error = rules::from_hresult(code, "");
        assert_eq!(error.native().and_then(|n| n.name), Some(name), "{code:#010x}");
        assert!(!error.to_string().is_empty(), "{code:#010x}");
    }
}

#[test]
fn an_unknown_code_keeps_its_number_and_reads_as_other() {
    for code in [0x8021_00FF_u32, 0x8021_FFFF, 0x8007_0005, 0x0000_0000, 0xFFFF_FFFF, 0x8000_FFFF] {
        assert_eq!(rules::code_name(code), None, "{code:#010x}");
        assert_eq!(rules::kind_for_hresult(code), None, "{code:#010x}");
        let error = rules::from_hresult(code, "");
        assert_eq!(error.interrupt_kind(), InterruptKind::Other, "{code:#010x}");
        assert_eq!(error.native().map(|n| n.code), Some(i64::from(code)), "{code:#010x}");
        assert!(error.to_string().contains(&format!("{code:#010x}")), "{code:#010x}: an unknown code is shown so it can be looked up");
    }
}

#[test]
fn driver_text_never_overrides_a_classified_code_except_paper_problem() {
    let jam_text = "clientErrorMediaJam";
    for (code, kind) in rules::KIND_BY_CODE {
        let error = rules::from_hresult(code, jam_text);
        if code == 0x8021_0004 {
            assert_eq!(error.interrupt_kind(), InterruptKind::Jam, "PAPER_PROBLEM is sharpened by the driver's text");
        } else {
            assert_eq!(error.interrupt_kind(), kind, "{code:#010x}: the code decides, not the text");
        }
    }
}

// ---- document-handling status bits -------------------------------------------------------------

const FEED_READY: i32 = 0x1;
const FLAT_READY: i32 = 0x2;
const DUP_READY: i32 = 0x4;
const FLAT_COVER_UP: i32 = 0x8;
const PATH_COVER_UP: i32 = 0x10;
const PAPER_JAM: i32 = 0x20;
const MULTIPLE_FEED: i32 = 0x200;
const DEVICE_ATTENTION: i32 = 0x400;
const LAMP_ERR: i32 = 0x800;
const NAMED: [i32; 9] = [FEED_READY, FLAT_READY, DUP_READY, FLAT_COVER_UP, PATH_COVER_UP, PAPER_JAM, MULTIPLE_FEED, DEVICE_ATTENTION, LAMP_ERR];

/// Every combination of the named bits, plus each one with the unnamed bits around it set.
fn every_status_word() -> impl Iterator<Item = i32> {
    (0..1u32 << NAMED.len()).flat_map(|mask| {
        let word = NAMED.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).fold(0, |acc, (_, bit)| acc | bit);
        // The top bit makes the LONG negative; it must not hide the named bits.
        [word, word | 0x40 | 0x80 | 0x100 | 0x1000 | 0x0100_0000, word | i32::MIN]
    })
}

#[test]
fn each_status_field_is_exactly_its_bit_and_unnamed_bits_change_nothing() {
    for word in every_status_word() {
        let status = rules::status_from_bits(word);
        let has = |bit: i32| word & bit != 0;
        assert_eq!(status.feeder_ready, Some(has(FEED_READY)), "{word:#x}");
        assert_eq!(
            (status.flat_ready, status.duplex_ready, status.cover_open, status.path_cover_open, status.jam),
            (has(FLAT_READY), has(DUP_READY), has(FLAT_COVER_UP), has(PATH_COVER_UP), has(PAPER_JAM)),
            "{word:#x}"
        );
        assert_eq!(
            (status.multiple_feed, status.attention, status.lamp_error),
            (has(MULTIPLE_FEED), has(DEVICE_ATTENTION), has(LAMP_ERR)),
            "{word:#x}"
        );
    }
}

#[test]
fn the_blocking_reason_follows_the_documented_order() {
    let jam = rules::wia_error_message(0x8021_0002);
    let cover = rules::wia_error_message(0x8021_0010);
    let double = rules::wia_error_message(0x8021_0014);
    let lamp = rules::wia_error_message(0x8021_0011);
    for word in every_status_word() {
        let status = rules::status_from_bits(word);
        let expected = if status.jam {
            jam
        } else if status.cover_open || status.path_cover_open {
            cover
        } else if status.multiple_feed {
            double
        } else if status.lamp_error {
            lamp
        } else {
            None
        };
        assert_eq!(rules::acquire_block_reason(status), expected, "{word:#x}");
        // The paper path names the same faults minus the lamp.
        let path = if expected == lamp { None } else { expected };
        assert_eq!(rules::paper_path_reason(status), path, "{word:#x}");
        // Attention, ready bits and readiness never block on their own.
        if word & (PAPER_JAM | FLAT_COVER_UP | PATH_COVER_UP | MULTIPLE_FEED | LAMP_ERR) == 0 {
            assert_eq!(rules::acquire_block_reason(status), None, "{word:#x}");
        }
    }
}

#[test]
fn capabilities_read_the_three_low_bits_only() {
    for word in every_status_word() {
        assert_eq!(rules::handling_caps(word), (word & 0x1 != 0, word & 0x2 != 0, word & 0x4 != 0), "{word:#x}");
    }
}

#[test]
fn an_empty_feeder_blocks_only_a_feeder_scan_on_a_port_that_reports_it() {
    for source in [Source::Auto, Source::Feeder, Source::Glass] {
        for ready in [false, true] {
            for wsd in [false, true] {
                let expected = source == Source::Feeder && !ready && !wsd;
                assert_eq!(rules::feeder_empty_blocks_acquire(source, ready, wsd), expected, "{source:?} ready={ready} wsd={wsd}");
            }
        }
    }
}

// ---- transfer outcomes -------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    FeederEmpty,
    BusyOrEmpty,
    Jam,
    Generic,
    Offline,
}

impl Fault {
    const ALL: [Fault; 6] = [Fault::None, Fault::FeederEmpty, Fault::BusyOrEmpty, Fault::Jam, Fault::Generic, Fault::Offline];

    /// The error this fault produces, or `None` for no fault.
    fn error(self) -> Option<ScanError> {
        match self {
            Fault::None => None,
            Fault::FeederEmpty => Some(rules::from_hresult(0x8021_0003, "")),
            Fault::BusyOrEmpty => Some(ScanError::native_error(
                InterruptKind::BusyOrFeederEmpty,
                craft_scan::NativeCode::hresult(0x8021_0006, Some("WIA_ERROR_BUSY")),
                rules::BUSY_OR_FEEDER_EMPTY,
            )),
            Fault::Jam => Some(rules::from_hresult(0x8021_0002, "")),
            Fault::Generic => Some(rules::from_hresult(0x8000_4005, "")),
            Fault::Offline => Some(rules::from_hresult(0x8021_0005, "")),
        }
    }

    /// Whether a transfer with this fault ends the stack of paper rather than failing it.
    fn ends_the_stack(self) -> bool {
        matches!(self, Fault::None | Fault::FeederEmpty | Fault::BusyOrEmpty)
    }
}

#[test]
fn every_transfer_outcome_is_decided_by_the_documented_rules() {
    for requested in [0usize, 1, 3] {
        for feeder in [false, true] {
            for transferred in 0..=4usize {
                for cancelled in [false, true] {
                    for fault in Fault::ALL {
                        let case = format!("requested={requested} feeder={feeder} transferred={transferred} cancelled={cancelled} {fault:?}");
                        let result = rules::after_transfer_checked(requested, feeder, transferred, cancelled, fault.error());
                        if transferred == 0 {
                            // Nothing in hand is never a success, whatever else happened.
                            assert!(result.is_err(), "{case}");
                            continue;
                        }
                        if !fault.ends_the_stack() {
                            // A hard fault after pages stays an error (the caller keeps the pages).
                            let error = result.expect_err(&case);
                            if matches!(fault, Fault::Jam) {
                                assert_eq!(error.interrupt_kind(), InterruptKind::Jam, "{case}");
                            }
                            continue;
                        }
                        let short = feeder && !cancelled && requested > 0 && transferred < requested;
                        if short {
                            let error = result.expect_err(&case);
                            assert_eq!(error.interrupt_kind(), InterruptKind::FeederEmpty, "{case}");
                            assert_eq!(error.to_string(), craft_scan::feeder_shortfall_message(requested, transferred), "{case}");
                        } else {
                            assert!(result.is_ok(), "{case}: {result:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_generic_failure_is_restated_by_whether_pages_arrived() {
    let generic = || Some(rules::from_hresult(0x8000_4005, ""));
    let none = rules::after_transfer(0, false, generic()).expect_err("no pages");
    assert!(rules::no_usable_page(&none), "{none}");
    let some = rules::after_transfer(2, false, generic()).expect_err("a stack that stopped part-way");
    assert!(some.to_string().starts_with("The scan stopped before all pages arrived."), "{some}");
    // Restating keeps the code it came from.
    assert_eq!(some.native().map(|n| n.code), Some(0x8000_4005));
}

#[test]
fn only_a_generic_failure_is_explained_by_the_status_bits() {
    // A fault the driver named keeps its own sentence whatever the bits say.
    for word in every_status_word() {
        let status = rules::status_from_bits(word);
        for (code, kind) in [(0x8021_0002_u32, InterruptKind::Jam), (0x8021_0005, InterruptKind::Offline), (0x8021_0020, InterruptKind::MultiFeed)] {
            let explained = rules::explain_transfer_failure(rules::from_hresult(code, ""), status, true, 1, 3);
            assert_eq!(explained.interrupt_kind(), kind, "{code:#010x} with {word:#x}");
        }
        // A generic failure takes the blocking bit's sentence when there is one.
        if let Some(reason) = rules::acquire_block_reason(status) {
            let explained = rules::explain_transfer_failure(rules::from_hresult(0x8000_4005, ""), status, true, 0, 1);
            assert_eq!(explained.to_string(), reason, "{word:#x}");
            assert_eq!(explained.native().map(|n| n.code), Some(0x8000_4005), "the code stays");
        }
    }
}

#[test]
fn a_busy_feeder_with_no_naming_bit_is_busy_or_empty_and_a_named_bit_wins() {
    for word in every_status_word() {
        let status = rules::status_from_bits(word);
        let explained = rules::explain_transfer_failure(rules::from_hresult(0x8021_0006, ""), status, true, 0, 1);
        let named = rules::acquire_block_reason(status).or_else(|| status.attention.then(|| rules::wia_error_message(0x8021_0008)).flatten());
        match named {
            Some(reason) => assert_eq!(explained.to_string(), reason, "{word:#x}"),
            None => assert_eq!(explained.interrupt_kind(), InterruptKind::BusyOrFeederEmpty, "{word:#x}"),
        }
        // On the glass a busy scanner is just busy.
        let glass = rules::explain_transfer_failure(rules::from_hresult(0x8021_0006, ""), status, false, 0, 1);
        assert_eq!(glass.interrupt_kind(), InterruptKind::Busy, "{word:#x}");
    }
}

#[test]
fn a_whole_feeder_ticket_asks_for_the_maximum_and_a_count_is_kept() {
    assert_eq!(rules::wsd_feeder_ticket_pages(0), i32::try_from(craft_scan::MAX_PAGES).unwrap_or(i32::MAX));
    for n in [1, 2, 25, 500] {
        assert_eq!(rules::wsd_feeder_ticket_pages(n), n);
    }
}

// ---- page size plan ----------------------------------------------------------------------------

/// Every subset of the named sizes, as a published list.
fn published_lists() -> Vec<Vec<PageSize>> {
    let all = PageSize::ALL;
    (0..1u32 << all.len()).map(|mask| all.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, size)| *size).collect()).collect()
}

#[test]
fn the_page_size_plan_tries_the_asked_size_then_only_covering_sizes_smallest_first() {
    for requested in PageSize::ALL.iter().copied() {
        assert_eq!(rules::plan_page_size(requested, None), PageSizePlan::Omit(PageSizeOmit::NotPublished));
        assert_eq!(rules::plan_page_size(requested, Some(&[])), PageSizePlan::Omit(PageSizeOmit::NotPublished));
        for listed in published_lists().iter().filter(|list| !list.is_empty()) {
            let case = format!("{requested:?} against {listed:?}");
            match rules::plan_page_size(requested, Some(listed)) {
                PageSizePlan::Omit(why) => {
                    assert_eq!(why, PageSizeOmit::NotOffered, "{case}");
                    assert!(!listed.contains(&requested), "{case}: a listed size is always tried");
                }
                PageSizePlan::Try(tries) => {
                    let unique: HashSet<_> = tries.iter().map(|size| size.name()).collect();
                    assert_eq!(unique.len(), tries.len(), "{case}: a size is tried once");
                    assert!(tries.iter().all(|size| listed.contains(size)), "{case}: only listed sizes are tried");
                    assert_eq!(tries.first() == Some(&requested), listed.contains(&requested), "{case}: the asked size comes first when listed");
                    let others: Vec<_> = tries.iter().filter(|size| **size != requested).collect();
                    let want = rules::page_size_dims(requested);
                    let mut last_area = 0u64;
                    for size in others {
                        let (w, h) = rules::page_size_dims(*size).unwrap_or_else(|| panic!("{case}: {size:?} has no sheet but was tried"));
                        let (want_w, want_h) = want.unwrap_or_else(|| panic!("{case}: a sheetless request only tries itself"));
                        assert!(w >= want_w && h >= want_h, "{case}: {size:?} does not cover the asked sheet");
                        let area = u64::from(w) * u64::from(h);
                        assert!(area >= last_area, "{case}: not smallest first");
                        last_area = area;
                    }
                }
            }
        }
    }
}

#[test]
fn a_status_with_nothing_set_reads_as_an_unready_feeder_not_an_unknown_one() {
    // The WIA property always has a value; zero means "nothing ready", which is a reading, not a gap.
    let status = rules::status_from_bits(0);
    assert_eq!(status, HandlingStatus { feeder_ready: Some(false), ..HandlingStatus::default() });
}

// ---- duplex on the feeder item -----------------------------------------------------------------

#[test]
fn duplex_is_selected_on_the_feeder_item_from_what_it_publishes() {
    use rules::{HANDLING_DUPLEX as DUP, HANDLING_FRONT_ONLY as FRONT, HandlingChoices as C, item_handling_select as pick};
    // The inbox WSD feeder item publishes the flag bits FRONT_ONLY | DUPLEX as allowed, currently FRONT_ONLY. It refuses the
    // value FRONT_ONLY | DUPLEX (E_INVALIDARG, measured on the fake WSD scanner) and takes DUPLEX.
    let wsd_flags = C::Flags(FRONT | DUP);
    assert_eq!(pick(&wsd_flags, Some(FRONT), true).expect("offered"), Some(DUP));
    assert_eq!(pick(&wsd_flags, Some(DUP), true).expect("already"), None, "nothing to write when it already holds duplex");
    // Back to single-sided after a duplex scan: the item holds DUPLEX alone, and 0 is not a value this item takes, so
    // the simplex value is FRONT_ONLY. Already single-sided (or unread) writes nothing.
    assert_eq!(pick(&wsd_flags, Some(DUP), false).expect("simplex"), Some(FRONT));
    assert_eq!(pick(&wsd_flags, Some(FRONT), false).expect("already simplex"), None);
    assert_eq!(pick(&wsd_flags, None, false).expect("unread"), None);
    // A mask without FRONT_ONLY has nothing to say but the cleared value; one with BACK_ONLY kept it.
    assert_eq!(pick(&C::Flags(DUP), Some(DUP), false).expect("only duplex published"), Some(0));
    assert_eq!(pick(&C::Flags(DUP | 0x40), Some(DUP | 0x40), false).expect("back only stays"), Some(0x40));
    // The same as a list: never the contradictory FRONT_ONLY | DUPLEX.
    let wsd = C::List(vec![FRONT, FRONT | DUP]);
    assert_eq!(pick(&wsd, Some(FRONT), true).expect("offered"), Some(DUP));
    assert_eq!(pick(&wsd, Some(FRONT | DUP), false).expect("single"), Some(FRONT), "a single-sided scan clears it");
    assert_eq!(pick(&C::List(vec![FRONT, DUP]), Some(FRONT), true).expect("offered"), Some(DUP));
    assert_eq!(pick(&wsd, Some(FRONT), false).expect("single"), None);
    // A feeder item without a duplex value: asking for duplex is an error, never a silent single-sided scan.
    let simplex = C::List(vec![FRONT]);
    assert!(pick(&simplex, Some(FRONT), true).is_err());
    assert_eq!(pick(&simplex, Some(FRONT), false).expect("single"), None);
    assert!(pick(&C::Flags(FRONT), Some(FRONT), true).is_err());
    // An item without the property leaves the choice to the device-level select.
    for duplex in [false, true] {
        assert_eq!(pick(&C::NotPublished, None, duplex).expect("not published"), None);
    }
}

#[test]
fn the_feeder_item_choice_is_two_sided_exactly_when_asked_and_never_contradictory() {
    use rules::{HandlingChoices as C, item_handling_select as pick};
    let choices = [
        C::List(vec![0x20]),
        C::List(vec![0x20, 0x24]),
        C::List(vec![0x24]),
        C::List(vec![0x1, 0x5, 0x20, 0x24]),
        C::List(vec![0x8, 0xC]),
        C::Flags(0x24),
        C::Flags(0x2C),
        C::Flags(0x20),
    ];
    for choice in &choices {
        for current in [None, Some(0x20), Some(0x24), Some(0x4), Some(0x5), Some(0x8)] {
            for duplex in [false, true] {
                if let Ok(Some(value)) = pick(choice, current, duplex) {
                    assert_eq!(value & 0x4 != 0, duplex, "{choice:?} {current:?}: {value:#x} for duplex={duplex}");
                    if duplex {
                        assert_eq!(value & 0x60, 0, "{choice:?}: {value:#x} pairs DUPLEX with a single-side bit");
                    } else if let C::List(list) = choice {
                        assert!(list.contains(&value), "{value:#x} not in {list:x?}");
                    }
                }
            }
        }
    }
}
