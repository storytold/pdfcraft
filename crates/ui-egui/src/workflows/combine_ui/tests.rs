use super::*;

#[test]
fn utc_stamp_is_utc_minutes_and_clamps_pre_epoch() {
    use std::time::Duration;
    let at = |s: u64| utc_stamp(SystemTime::UNIX_EPOCH + Duration::from_secs(s));
    assert_eq!(at(0), "1970-01-01 00:00 UTC");
    assert_eq!(at(1_791_446_357), "2026-10-08 07:59 UTC");
    // The 400-year leap rule.
    assert_eq!(at(951_782_400), "2000-02-29 00:00 UTC");
    // A clock before 1970 reads as the epoch, as the old `unwrap_or(0)` did.
    assert_eq!(utc_stamp(SystemTime::UNIX_EPOCH - Duration::from_secs(5)), "1970-01-01 00:00 UTC");
    // Past chrono's range keeps the proleptic Gregorian date the old code gave, not the epoch.
    // Windows' SystemTime holds only up to about year 30828, so this is checked where it fits.
    if let Some(far) = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(10_000_000_000_000)) {
        assert_eq!(utc_stamp(far), "318857-05-20 17:46 UTC");
    }
    // Windows' SystemTime can't hold u64::MAX / 2 seconds, so it is only checked where it exists.
    if let Some(far) = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(u64::MAX / 2)) {
        assert_eq!(utc_stamp(far), "292277026596-12-04 15:30 UTC");
    }
    // Years past 9999 keep the plain zero-padded year (no `+`).
    assert_eq!(at(253_402_300_800), "10000-01-01 00:00 UTC");
}

/// A one-page PDF that opens with "pw".
fn locked() -> Vec<u8> {
    let objs =
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>", "<< /Type /Page /Parent 2 0 R >>"];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut doc = pdfcraft_cos::Document::open(Arc::new(out)).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "pw",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

#[test]
fn duplicate_warnings_reuse_results_and_track_removal_and_parts() {
    let bytes = locked();
    let mut app = PdfCraftApp::new();
    app.stage_combine(vec![("same.pdf".into(), bytes.clone())]);
    let mut part = app.combine_draft[0].clone();
    part.id = 99;
    app.combine_draft.push(part);
    let first = app.combine_tab.duplicate_warnings(&app.combine_draft);
    assert_eq!(&*first, &[false, false], "parts of one source are not duplicate additions");
    let again = app.combine_tab.duplicate_warnings(&app.combine_draft);
    assert!(Arc::ptr_eq(&first, &again), "unchanged frames reuse the result without scanning files");
    app.stage_combine(vec![("same.pdf".into(), bytes)]);
    assert_eq!(&*app.combine_tab.duplicate_warnings(&app.combine_draft), &[true, true, true]);
    let duplicate = app.combine_draft.last().unwrap().id;
    app.combine_apply(RowAction::RemoveFile(duplicate));
    assert_eq!(&*app.combine_tab.duplicate_warnings(&app.combine_draft), &[false, false]);
    app.combine_history_step(true);
    assert_eq!(&*app.combine_tab.duplicate_warnings(&app.combine_draft), &[true, true, true]);
    app.close_combine_tab();
    assert!(app.combine_tab.duplicate_warnings(&app.combine_draft).is_empty());
}

/// #1303: an image or text file is converted anew, with its own date, each time it is added,
/// so it is the file as added that is compared, not the PDF made from it.
#[test]
fn the_same_image_or_text_file_added_twice_is_a_duplicate() {
    static NOW: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1_700_000_000);
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(12, 4, image::Rgba([20, 30, 40, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png = png.into_inner();
    let warnings = |first: (&str, &[u8]), second: (&str, &[u8])| {
        let mut app = PdfCraftApp::new();
        app.session = std::mem::take(&mut app.session).with_clock(|| NOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        for (name, bytes) in [first, second] {
            app.stage_combine(vec![(name.into(), bytes.to_vec())]);
        }
        assert_ne!(app.combine_draft[0].bytes, app.combine_draft[1].bytes, "each addition is converted with its own date");
        app.combine_tab.duplicate_warnings(&app.combine_draft).to_vec()
    };
    assert_eq!(warnings(("notes.txt", b"Hello"), ("notes.txt", b"Hello")), [true, true], "the same text file");
    assert_eq!(warnings(("photo.png", &png), ("photo.png", &png)), [true, true], "the same image");
    assert_eq!(warnings(("notes.txt", b"Hello"), ("notes.txt", b"World")), [false, false], "different text, same name and size");
}

#[test]
fn every_read_of_a_file_is_new_even_after_undo() {
    // Regression: the read counter lived in the undo history, so unlock → undo → unlock (with
    // another password) could repeat a value and keep a thumbnail of the other read.
    let mut app = PdfCraftApp::new();
    app.stage_combine(vec![("secret.pdf".into(), locked())]);
    let added = app.combine_draft[0].auth;
    assert!(app.combine_unlock_rows(&[0], "pw"));
    let first = app.combine_draft[0].auth;
    app.combine_history_step(true);
    assert_eq!(app.combine_draft[0].auth, added, "undo brings back the file as it was read");
    assert!(app.combine_unlock_rows(&[0], "owner"));
    let second = app.combine_draft[0].auth;
    assert!(second != first && second != added, "added {added}, unlocked {first}, unlocked again {second}");
}

#[test]
fn moves_by_rank_never_lose_or_repeat_a_file() {
    let mut app = PdfCraftApp::new();
    app.stage_combine((0..6).map(|i| (format!("f{i}.pdf"), locked())).collect());
    let mut ids: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
    for (moving, gap) in [(vec![0usize, 2, 4], 6), (vec![5], 0), (vec![1, 3], 3)] {
        let set: BTreeSet<u64> = moving.iter().filter_map(|i| ids.get(*i).copied()).collect();
        app.combine_move_to_gap(set, gap);
        let mut now: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
        ids = now.clone();
        now.sort_unstable();
        assert_eq!(now, (0..6).collect::<Vec<u64>>(), "every file once");
    }
}

#[test]
fn bookmarks_are_named_without_the_extension_of_a_converted_file() {
    assert_eq!(title_of("report.pdf"), "report");
    assert_eq!(title_of("REPORT.PDF"), "REPORT");
    assert_eq!(title_of("scan.2024.JPG"), "scan.2024");
    assert_eq!(title_of("notes.txt"), "notes");
    assert_eq!(title_of("v1.2"), "v1.2", "not a known extension");
    assert_eq!(title_of(".png"), ".png", "nothing would be left");
}

#[test]
fn page_lists_become_the_shortest_range() {
    assert_eq!(range_of(&[0, 1, 2], 3), "", "every page in order: no range");
    assert_eq!(range_of(&[0, 1, 2], 5), "1-3");
    assert_eq!(range_of(&[2, 0, 3], 5), "3, 1, 4");
    assert_eq!(range_of(&[8, 7, 6, 0], 9), "9-7, 1");
    assert_eq!(range_of(&[0, 0, 1], 2), "1, 1-2");
    assert_eq!(range_of(&[4], 9), "5");
    assert_eq!(range_of(&[], 3), "");
    // Whatever comes out reads back as the same pages.
    for pages in [vec![0, 1, 2, 5, 4, 3, 3, 7], vec![9, 0, 9, 1], vec![2, 1, 0, 1, 2]] {
        let range = range_of(&pages, 10);
        let back = pdfcraft_engine::print::select_pages(10, Some(&range), &[], pdfcraft_engine::print::Subset::All, false).unwrap();
        assert_eq!(back, pages, "{range}");
    }
}

fn part(id: u64, group: u64, pages: &[usize]) -> Part {
    Part { id, group, pages: Some(pages.to_vec()) }
}

#[test]
fn a_page_moved_between_another_files_pages_splits_its_file_and_parts_rejoin() {
    let mut next = 100;
    let mut new_id = || {
        next += 1;
        next
    };
    // A: pages 1-3, B: pages 1-2.
    let list = [part(1, 1, &[0, 1, 2]), part(2, 2, &[0, 1])];
    // A's page 3 between B's pages: A splits around B's first page.
    let moved = moved_pages(&list, &BTreeSet::from([(1, 2)]), 4, &mut new_id).unwrap();
    assert_eq!(moved, [part(1, 1, &[0, 1]), part(2, 2, &[0]), part(101, 1, &[2]), part(102, 2, &[1])]);
    // Back where it was: the parts join again, under their first ids.
    let back = moved_pages(&moved, &BTreeSet::from([(101, 0)]), 2, &mut new_id).unwrap();
    assert_eq!(back, [part(1, 1, &[0, 1, 2]), part(2, 2, &[0, 1])]);
    // Within a file: only its order changes.
    let within = moved_pages(&list, &BTreeSet::from([(1, 0)]), 3, &mut new_id).unwrap();
    assert_eq!(within, [part(1, 1, &[1, 2, 0]), part(2, 2, &[0, 1])]);
    // Several pages carried together keep their order.
    let both = moved_pages(&list, &BTreeSet::from([(1, 0), (1, 2)]), 5, &mut new_id).unwrap();
    assert_eq!(both, [part(1, 1, &[1]), part(2, 2, &[0, 1]), part(103, 1, &[0, 2])]);
    // Dropped where it is, or nothing picked: no change.
    assert_eq!(moved_pages(&list, &BTreeSet::from([(1, 1)]), 1, &mut new_id), None);
    assert_eq!(moved_pages(&list, &BTreeSet::from([(9, 9)]), 0, &mut new_id), None);
    // A file that can't be split moves whole, and pages move around it.
    let locked = [part(1, 1, &[0, 1]), Part { id: 3, group: 3, pages: None }];
    let around = moved_pages(&locked, &BTreeSet::from([(1, 0)]), 3, &mut new_id).unwrap();
    assert_eq!(around, [part(1, 1, &[1]), Part { id: 3, group: 3, pages: None }, part(104, 1, &[0])]);
}

#[test]
fn removing_pages_drops_emptied_parts_and_joins_what_meets() {
    let mut next = 200;
    let mut new_id = || {
        next += 1;
        next
    };
    // A split around B: A[1], B[1], A[2].
    let list = [part(1, 1, &[0]), part(2, 2, &[0]), part(3, 1, &[1])];
    // B goes: A's two parts meet and join.
    assert_eq!(removed_pages(&list, &BTreeSet::from([(2, 0)]), &mut new_id), Some(vec![part(1, 1, &[0, 1])]));
    // One page of a part.
    let two = [part(1, 1, &[0, 1, 2])];
    assert_eq!(removed_pages(&two, &BTreeSet::from([(1, 1)]), &mut new_id), Some(vec![part(1, 1, &[0, 2])]));
    // Every page: nothing left. Repeated pages are told apart by their place.
    assert_eq!(removed_pages(&two, &BTreeSet::from([(1, 0), (1, 1), (1, 2)]), &mut new_id), Some(Vec::new()));
    let repeats = [part(1, 1, &[0, 0])];
    assert_eq!(removed_pages(&repeats, &BTreeSet::from([(1, 1)]), &mut new_id), Some(vec![part(1, 1, &[0])]));
    // A page that isn't there: nothing changes, and it says so.
    assert_eq!(removed_pages(&two, &BTreeSet::from([(1, 9)]), &mut new_id), None);
}
