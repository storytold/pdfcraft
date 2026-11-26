use super::*;

fn book(rtl: bool, cells: &[(&str, gridcraft_core::Value, gridcraft_model::HAlign)]) -> Workbook {
    let mut wb = Workbook::new();
    let ids: Vec<_> = cells.iter().map(|(_, _, h)| wb.styles.derive(gridcraft_model::StyleId::DEFAULT, |st| st.align.h = *h)).collect();
    let sh = wb.sheet_mut(0).unwrap();
    sh.right_to_left = rtl;
    for ((at, value, _), id) in cells.iter().zip(ids) {
        let c = CellRef::parse(at).unwrap();
        sh.set_value(c, value.clone());
        sh.set_style(c, id);
    }
    wb
}

fn copied(wb: &Workbook, range: &str) -> String {
    let r = RangeRef::parse(range).unwrap();
    range_html(wb, 0, r, &crate::display::range_text(wb, 0, r)).unwrap()
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(unix)]
#[test]
fn save_fails_upfront_when_the_temp_name_is_taken() {
    // Another local user who can write the directory must not be able to point the
    // save's predictable temporary name at their own file: creation is exclusive.
    let dir = std::env::temp_dir().join(format!("gridcraft-fs-safety-temp-name-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let victim = dir.join("victim.txt");
    std::fs::write(&victim, b"original").unwrap();
    let dest = dir.join("book.xlsx");
    std::os::unix::fs::symlink(&victim, dir.join("book.xlsx.sctmp")).unwrap();
    let err = write_file(dest.to_str().unwrap(), b"new").unwrap_err();
    assert_eq!(std::fs::read(&victim).unwrap(), b"original");
    assert!(!dir.join("book.xlsx").exists(), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(unix)]
#[test]
fn save_keeps_the_destination_access_restrictions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("gridcraft-fs-safety-permissions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("private.xlsx");
    std::fs::write(&dest, b"v1").unwrap();
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600)).unwrap();
    write_file(dest.to_str().unwrap(), b"v2").unwrap();
    let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    // The mode is set outright from the old file's, not through the umask: group and other access
    // it had stay as they were.
    for old in [0o640, 0o664] {
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(old)).unwrap();
        write_file(dest.to_str().unwrap(), b"v3").unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777, old);
    }
    assert!(!dir.join("private.xlsx.sctmp").exists());
    // And a brand-new file is created with the restrictive, not permissive, default.
    let fresh = dir.join("new.xlsx");
    write_file(fresh.to_str().unwrap(), b"v").unwrap();
    let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
    assert!(mode & 0o022 == 0, "mode {mode:o}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Without the old group, a saved file keeps the old owner and other bits and nothing for a group.
#[cfg(not(target_arch = "wasm32"))]
#[cfg(unix)]
#[test]
fn restored_mode_drops_the_group_and_special_bits_when_the_group_is_lost() {
    assert_eq!(super::restored_mode(0o100_664, true), 0o664);
    assert_eq!(super::restored_mode(0o100_664, false), 0o604);
    assert_eq!(super::restored_mode(0o102_444, false), 0o404, "an original read-only file stays read-only, setgid is dropped");
    assert_eq!(super::restored_mode(0o104_755, false), 0o705, "setuid is dropped");
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(unix)]
#[test]
fn fresh_save_drops_group_and_other_write_under_a_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var_os("GRIDCRAFT_UMASK_CHILD").is_none() {
        // Re-run this test in a child shell whose umask is 0002.
        let exe = std::env::current_exe().unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("umask 0002 && exec \"$0\" --exact io::tests::fresh_save_drops_group_and_other_write_under_a_permissive_umask")
            .arg(exe)
            .env("GRIDCRAFT_UMASK_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let dir = std::env::temp_dir().join(format!("gridcraft-fs-safety-umask-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fresh = dir.join("new.xlsx");
    write_file(fresh.to_str().unwrap(), b"v").unwrap();
    let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(mode & 0o022 == 0, "mode {mode:o}");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn open_refuses_directories_and_unknowable_lengths() {
    let dir = std::env::temp_dir().join(format!("gridcraft-fs-safety-open-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(read_file(dir.to_str().unwrap()).is_err(), "a directory is not a regular file");
    assert!(read_file(dir.join("missing").to_str().unwrap()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn html_export_refuses_an_outsized_grid_instead_of_growing_forever() {
    // #2387: the used rectangle alone bounds the output (every row writes the whole
    // column span), so a sparse sheet with one cell at A1 and another far right
    // 300 rows down is 4.9M cells → refused before a byte of `<td>` is written.
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::new(0, 0), gridcraft_core::Value::Text("start".into()));
    sh.set_value(CellRef::new(300, 16383), gridcraft_core::Value::Text("corner".into()));
    let err = to_html(&wb, 0).unwrap_err();
    assert!(err.to_string().contains("too tall × wide"), "{err}");
    // A normal sheet is fine.
    let mut ok = Workbook::new();
    ok.sheet_mut(0).unwrap().set_value(CellRef::new(5, 2), gridcraft_core::Value::Text("t".into()));
    assert!(to_html(&ok, 0).is_ok());
}

#[test]
fn html_export_embeds_images_as_data_uris() {
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().images.push(gridcraft_model::Image {
        id: 1,
        anchor: gridcraft_model::Anchor { cell: CellRef::new(0, 0), dx: 0.0, dy: 0.0, width: 72.0, height: 36.0, mode: Default::default() },
        data: vec![1, 2, 3].into(),
        mime: "image/png".into(),
        alt: "a \"logo\"".into(),
    });
    let html = to_html(&wb, 0).unwrap();
    assert!(
        html.contains("<img src=\"data:image/png;base64,AQID\" alt=\"a &quot;logo&quot;\" style=\"position:absolute;left:0px;top:0px;width:72px;height:36px\"></div></body>"),
        "{html}"
    );
    assert!(html.contains("<div style=\"position:relative\"><table"), "{html}");
    assert!(!to_html(&Workbook::new(), 0).unwrap().contains("<img"), "no images, no <img>");
}

#[test]
fn html_export_places_images_at_their_anchor_using_sheet_geometry() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.cols.insert(1, gridcraft_model::LineInfo { size: Some(100.0), ..Default::default() });
    sh.rows.insert(1, gridcraft_model::LineInfo { size: Some(30.0), ..Default::default() });
    sh.images.push(gridcraft_model::Image {
        id: 1,
        anchor: gridcraft_model::Anchor { cell: CellRef::new(3, 2), dx: 5.0, dy: 7.0, width: 10.0, height: 20.0, mode: Default::default() },
        data: vec![1].into(),
        mime: "image/png".into(),
        alt: String::new(),
    });
    sh.set_value(CellRef::new(1, 1), gridcraft_core::Value::Number(1.0));
    let html = to_html(&wb, 0).unwrap();
    // The table starts at B2; the picture is at C4: one 100 px column and two rows (30 + 20) further on.
    assert!(html.contains("<col style=\"width:100px\">"), "{html}");
    assert!(html.contains("<tr style=\"height:30px\">"), "{html}");
    assert!(html.contains("left:105px;top:57px;width:10px;height:20px"), "{html}");
}

fn book_with_picture(anchor: &str, cell_at: (u32, u32)) -> Workbook {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::new(cell_at.0, cell_at.1), gridcraft_core::Value::Number(1.0));
    sh.images.push(gridcraft_model::Image {
        id: 1,
        anchor: gridcraft_model::Anchor {
            cell: CellRef::parse(anchor).unwrap(),
            dx: 0.0,
            dy: 0.0,
            width: 10.0,
            height: 10.0,
            mode: Default::default(),
        },
        data: vec![1].into(),
        mime: "image/png".into(),
        alt: String::new(),
    });
    wb
}

#[test]
fn html_export_places_images_past_hidden_columns_and_rows() {
    let mut wb = book_with_picture("C3", (0, 0));
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::new(2, 2), gridcraft_core::Value::Number(2.0));
    sh.cols.insert(1, gridcraft_model::LineInfo { hidden: true, ..Default::default() });
    sh.rows.insert(1, gridcraft_model::LineInfo { hidden: true, ..Default::default() });
    let html = to_html(&wb, 0).unwrap();
    // Column B and row 2 take no room: C3 is one default column and row from A1.
    assert_eq!(html.matches("<col ").count(), 2, "{html}");
    assert_eq!(html.matches("<tr ").count(), 2, "{html}");
    assert!(html.contains("left:64px;top:20px"), "{html}");
}

#[test]
fn html_export_places_images_above_and_left_of_the_table() {
    let html = to_html(&book_with_picture("A1", (2, 2)), 0).unwrap();
    // The table starts at C3, so a picture at A1 sits before it instead of failing.
    assert!(html.contains("left:-128px;top:-40px"), "{html}");
}

#[test]
fn html_export_marks_right_to_left_sheets() {
    use gridcraft_core::Value;
    use gridcraft_model::HAlign;
    let wb = book(true, &[("A1", Value::text("Hello!"), HAlign::General), ("B1", Value::Number(-5.0), HAlign::General)]);
    let html = to_html(&wb, 0).unwrap();
    assert!(html.contains("<body><table dir=\"rtl\">"), "{html}");
    assert!(html.contains("<td style=\"\" dir=\"auto\">Hello!</td>"), "Latin text keeps its own direction: {html}");
    assert!(html.contains("<td style=\"text-align:left;\" dir=\"ltr\">-5</td>"), "numbers mirror to the left and keep their sign in front: {html}");
    let clip = copied(&wb, "A1:B1");
    assert!(clip.starts_with("<table dir=\"rtl\" style="), "{clip}");
    assert_eq!(clip.matches("dir=\"auto\"").count(), 1, "{clip}");
    assert!(clip.contains("text-align:left;\" dir=\"ltr\">-5</td>"), "{clip}");

    let ltr = book(false, &[("A1", Value::text("Hello!"), HAlign::General)]);
    assert!(to_html(&ltr, 0).unwrap().contains("<body><table><tr>"));
    assert!(copied(&ltr, "A1").starts_with("<table style="));
}

#[test]
fn html_export_lets_arabic_general_text_align_by_direction() {
    use gridcraft_core::Value;
    use gridcraft_model::HAlign;
    let wb = book(
        false,
        &[
            ("A1", Value::text("مرحبا!"), HAlign::General),
            ("B1", Value::text("שלום"), HAlign::Left),
            ("C1", Value::text("مرحبا"), HAlign::Center),
            ("D1", Value::text("Hello"), HAlign::General),
        ],
    );
    let html = to_html(&wb, 0).unwrap();
    assert!(html.contains("<td style=\"\" dir=\"auto\">مرحبا!</td>"), "General Arabic text aligns by its direction: {html}");
    assert!(html.contains("<td style=\"text-align:left;\" dir=\"auto\">שלום</td>"), "explicit alignment stays: {html}");
    assert!(html.contains("<td style=\"text-align:center;\" dir=\"auto\">مرحبا</td>"), "{html}");
    assert!(html.contains("<td style=\"text-align:left;\">Hello</td>"), "{html}");
    assert!(html.contains("<body><table><tr>"), "a left-to-right sheet has no table direction: {html}");

    let clip = copied(&wb, "A1:D1");
    let cells: Vec<&str> = clip.split("<td ").skip(1).collect();
    assert_eq!(cells.len(), 4, "{clip}");
    assert!(cells[0].contains("dir=\"auto\"") && !cells[0].contains("text-align"), "{clip}");
    assert!(cells[1].contains("text-align:left;") && cells[1].contains("dir=\"auto\""), "{clip}");
    assert!(cells[3].contains("text-align:left;") && !cells[3].contains("dir="), "{clip}");
}

#[test]
fn html_export_of_latin_ltr_sheet_is_unchanged() {
    use gridcraft_core::Value;
    use gridcraft_model::HAlign;
    let mut wb = book(
        false,
        &[
            ("A1", Value::text("Name"), HAlign::General),
            ("B1", Value::text("Qty"), HAlign::Center),
            ("A2", Value::text("Apples & <pears>"), HAlign::General),
            ("B2", Value::Number(3.0), HAlign::General),
            ("A3", Value::text("Total"), HAlign::Right),
        ],
    );
    wb.sheet_mut(0).unwrap().merges.push(RangeRef::parse("A3:B3").unwrap());
    assert_eq!(
        to_html(&wb, 0).unwrap(),
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Sheet1</title><style>table{border-collapse:collapse;font-family:Calibri,Carlito,Arial,sans-serif;font-size:11pt}td{border:1px solid #d4d4d4;padding:2px 4px;white-space:nowrap}</style></head><body><table>\
             <tr><td style=\"text-align:left;\">Name</td><td style=\"text-align:center;\">Qty</td></tr>\
             <tr><td style=\"text-align:left;\">Apples &amp; &lt;pears&gt;</td><td style=\"text-align:right;\">3</td></tr>\
             <tr><td style=\"text-align:right;\" rowspan=\"1\" colspan=\"2\">Total</td></tr>\
             </table></body></html>"
    );
    let cell = "border:1px solid #d4d4d4;padding:2px 4px;white-space:pre-wrap;";
    assert_eq!(
        copied(&wb, "A1:B2"),
        format!(
            "<table style=\"border-collapse:collapse;font-family:Calibri,Carlito,Arial,sans-serif;font-size:11pt\">\
                 <tr><td style=\"{cell}text-align:left;\">Name</td><td style=\"{cell}text-align:center;\">Qty</td></tr>\
                 <tr><td style=\"{cell}text-align:left;\">Apples &amp; &lt;pears&gt;</td><td style=\"{cell}text-align:right;\">3</td></tr></table>"
        )
    );
}

#[test]
fn html_export_fails_instead_of_dropping_cells_past_the_row_limit() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::new(0, 0), gridcraft_core::Value::text("near"));
    sh.set_value(CellRef::new(149_999, 0), gridcraft_core::Value::text("far"));
    assert!(save_bytes(&wb, "book.html").is_err());
    let sh = wb.sheet_mut(0).unwrap();
    sh.remove_cell(CellRef::new(149_999, 0));
    sh.set_value(CellRef::new(100_000, 0), gridcraft_core::Value::text("far"));
    let html = String::from_utf8(save_bytes(&wb, "book.html").unwrap()).unwrap();
    assert!(html.contains(">near</td>") && html.contains(">far</td>"));
}

#[test]
fn macros_are_saved_only_into_xlsm() {
    let mut wb = Workbook::new();
    wb.vba_project = Some(b"vba".to_vec().into());
    let has_macros = |path: &str| {
        let bytes = save_bytes(&wb, path).unwrap();
        gridcraft_xlsx::read_xlsx(&bytes).unwrap().0.vba_project.is_some()
    };
    assert!(has_macros("book.xlsm"));
    assert!(has_macros("BOOK.XLSM"));
    assert!(!has_macros("book.xlsx"), "an .xlsx never carries a VBA project");
    assert!(has_macros("book.xltm"), "a macro-enabled template keeps the VBA project");
    assert!(!has_macros("book.xltx"), "a macro-free template never carries a VBA project");
    assert!(wb.vba_project.is_some(), "the open workbook keeps its macros");
}

#[test]
fn csv_named_history_does_not_get_the_reserved_sheet_name() {
    for file in ["History.csv", "hIsToRy.csv", "history.tsv"] {
        let (wb, _) = open_bytes(file, b"1\n").unwrap();
        let name = wb.sheets[0].name.clone();
        assert!(wb.check_sheet_name(&name, Some(0)).is_ok(), "{file} opened as {name:?}");
    }
    let (wb, _) = open_bytes("Sales.csv", b"1\n").unwrap();
    assert_eq!(wb.sheets[0].name, "Sales");
}

#[test]
fn csv_and_text_open_read_day_first_dates_and_report_a_guessed_order() {
    let may = gridcraft_core::date::serial_from_ymd(gridcraft_core::DateSystem::D1900, 2026, 5, 31).unwrap();
    let april = gridcraft_core::date::serial_from_ymd(gridcraft_core::DateSystem::D1900, 2026, 4, 3).unwrap();
    for file in ["dates.csv", "dates.txt"] {
        let (wb, warnings) = open_bytes(file, b"When\n31/05/2026\n03/04/2026\n").unwrap();
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(1, 0)), gridcraft_core::Value::Number(may), "{file}");
        assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(2, 0)), gridcraft_core::Value::Number(april), "{file}");
        assert!(warnings.is_empty(), "{file}: {warnings:?}");
    }
    let (_, warnings) = open_bytes("ambiguous.csv", b"When\n03/04/2026\n12/01/2026\n").unwrap();
    assert!(warnings.iter().any(|w| w.contains("column A") && w.contains("ambiguous")), "{warnings:?}");
}

/// A save that can't be put in place (here a folder already has the workbook's name, as
/// with a file another app holds locked) fails without leaving `<name>.sctmp` behind.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn failed_save_leaves_no_temporary_file() {
    let dir = std::env::temp_dir().join(format!("gridcraft-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let blocked = dir.join("Book1.xlsx");
    std::fs::create_dir(&blocked).unwrap();
    assert!(write_file(blocked.to_str().unwrap(), b"workbook").is_err());
    assert!(!dir.join("Book1.xlsx.sctmp").exists(), "the temporary file is removed");
    // An ordinary save still works and leaves only the workbook.
    let ok = dir.join("Book2.xlsx");
    write_file(ok.to_str().unwrap(), b"workbook").unwrap();
    assert_eq!(std::fs::read(&ok).unwrap(), b"workbook");
    assert!(!dir.join("Book2.xlsx.sctmp").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A save replaces an existing workbook with exactly the new bytes, and a save into a folder
/// that doesn't exist fails without creating anything. The flush to disk before the rename is
/// not covered: a power loss can't be simulated in a unit test.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn save_writes_exact_bytes_and_fails_cleanly_without_a_folder() {
    let dir = std::env::temp_dir().join(format!("gridcraft-write-exact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("Book.xlsx");
    std::fs::write(&dest, b"old workbook that is longer than the new one").unwrap();
    let bytes: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
    write_file(dest.to_str().unwrap(), &bytes).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);
    assert!(!dir.join("Book.xlsx.sctmp").exists());
    let missing = dir.join("no-such-folder").join("Book.xlsx");
    assert!(write_file(missing.to_str().unwrap(), b"x").is_err());
    assert!(!dir.join("no-such-folder").exists());
    assert_eq!(std::fs::read(&dest).unwrap(), bytes, "the existing workbook is untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Only "this file system cannot flush" errors are tolerated when syncing a saved file.
/// The fallback on a real network mount is not tested.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn full_sync_tolerates_only_unsupported_errors() {
    use std::io::{Error, ErrorKind};
    assert_eq!(full_sync_unsupported(&Error::from_raw_os_error(25)), cfg!(unix));
    assert!(full_sync_unsupported(&Error::from(ErrorKind::Unsupported)));
    assert!(!full_sync_unsupported(&Error::from(ErrorKind::PermissionDenied)));
    assert!(!full_sync_unsupported(&Error::from(ErrorKind::StorageFull)));
    assert!(!full_sync_unsupported(&Error::from_raw_os_error(28)), "ENOSPC on Linux");
    assert!(!full_sync_unsupported(&Error::from_raw_os_error(5)), "EIO");
}

/// Apple's ENOTSUP is 45 (EOPNOTSUPP is 102), and std maps only the latter to
/// `ErrorKind::Unsupported`: F_FULLFSYNC refused with ENOTSUP must still save. On Linux, 45 is
/// EL2NSYNC and stays a failed save. On FreeBSD and other BSDs, 45 is ENOTSUP (the same value as
/// EOPNOTSUPP), so this host's std reports `ErrorKind::Unsupported` and the flush is tolerated
/// even when the `apple` flag is false — which is the real helper's behaviour on those systems.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn full_sync_tolerates_apple_enotsup() {
    use std::io::{Error, ErrorKind};
    let enotsup = Error::from_raw_os_error(45);
    assert!(flush_unsupported_on(&enotsup, true, true), "ENOTSUP on Apple");
    let host_maps_45_unsupported = enotsup.kind() == ErrorKind::Unsupported;
    assert_eq!(flush_unsupported_on(&enotsup, true, false), host_maps_45_unsupported, "45 on other Unix");
    assert_eq!(flush_unsupported_on(&enotsup, false, false), host_maps_45_unsupported, "45 on Windows");
    assert!(flush_unsupported_on(&Error::from_raw_os_error(25), true, true), "ENOTTY on Apple");
    assert!(!flush_unsupported_on(&Error::from_raw_os_error(5), true, true), "EIO on Apple");
    assert!(!flush_unsupported_on(&Error::from_raw_os_error(28), true, true), "ENOSPC on Apple");
}

/// The real helper on an Apple host: a raw ENOTSUP from F_FULLFSYNC is tolerated.
#[cfg(target_vendor = "apple")]
#[test]
fn full_sync_tolerates_raw_enotsup_on_apple() {
    assert!(full_sync_unsupported(&std::io::Error::from_raw_os_error(45)));
}

/// While a script runs (#2178), file reads and writes are refused whatever command or
/// parameter asked for them; nesting and dropping the sandbox restore access.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn script_sandbox_refuses_file_access_until_dropped() {
    let dir = std::env::temp_dir().join(format!("gridcraft-sandbox-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let existing = dir.join("in.csv");
    std::fs::write(&existing, b"a,b\n").unwrap();
    let (existing, out) = (existing.to_string_lossy().to_string(), dir.join("out.xlsx").to_string_lossy().to_string());
    {
        let _outer = ScriptSandbox::enter();
        {
            let _inner = ScriptSandbox::enter();
            assert!(read_file(&existing).is_err());
        }
        assert!(read_file(&existing).is_err(), "the outer sandbox still holds");
        assert!(write_file(&out, b"leak").is_err());
        assert!(!std::path::Path::new(&out).exists());
    }
    assert_eq!(read_file(&existing).unwrap(), b"a,b\n");
    write_file(&out, b"ok").unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The guard's drop lowers the counter of the thread it runs on, so it must not move between threads:
/// naming the trait's method is ambiguous (a compile error) if `ScriptSandbox` is `Send`.
#[test]
fn script_sandbox_is_not_send() {
    trait AmbiguousIfSend<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    struct IsSend;
    impl<T: ?Sized + Send> AmbiguousIfSend<IsSend> for T {}
    <ScriptSandbox as AmbiguousIfSend<_>>::check();
}

/// A minimal compound (CFB/OLE2) file — FAT in sector 0, directory in sector 1 — holding a
/// root entry and the given stream names.
fn cfb(streams: &[&str]) -> Vec<u8> {
    let mut b = vec![0u8; 512 * 3];
    let mut put = |at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
    put(0, &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    put(30, &9u16.to_le_bytes());
    put(44, &1u32.to_le_bytes());
    put(48, &1u32.to_le_bytes());
    put(68, &0xFFFF_FFFEu32.to_le_bytes());
    for i in 1..109 {
        put(76 + i * 4, &0xFFFF_FFFFu32.to_le_bytes());
    }
    put(512, &0xFFFF_FFFDu32.to_le_bytes());
    put(516, &0xFFFF_FFFEu32.to_le_bytes());
    for (k, name) in std::iter::once("Root Entry").chain(streams.iter().copied()).enumerate() {
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        put(1024 + k * 128, &units);
        put(1024 + k * 128 + 64, &(units.len() as u16 + 2).to_le_bytes());
    }
    b
}

/// Cut-list rows as a CAD/CAM program exports them: `;`-separated, CRLF, part codes in the last
/// two columns, a part name containing a comma.
const CUT_LIST: &str = "1;511;184;P3_L1_led;2162PE;16;1MM_2162PE;1MM_2162PE;;;1244121406FB;\r\n\
                            1;562.5;510;Komoda, lijevo;2162PE;19;1MM_2162PE;;;;12441214071A;124412140710\r\n";

#[test]
fn csv_save_keeps_the_separator_it_was_opened_with() {
    let (wb, _) = open_bytes("cut list.csv", CUT_LIST.as_bytes()).unwrap();
    assert_eq!(wb.csv_delimiter, Some(b';'));
    assert_eq!(String::from_utf8(save_bytes(&wb, "cut list.csv").unwrap()).unwrap(), CUT_LIST);
    // Save As another .csv name keeps it too; a comma file stays a comma file (quoting only
    // fields that contain the separator in use).
    assert_eq!(String::from_utf8(save_bytes(&wb, "copy.CSV").unwrap()).unwrap(), CUT_LIST);
    let commas = "\"Komoda, lijevo\",b\r\n1,x;y\r\n";
    let (wb, _) = open_bytes("plain.csv", commas.as_bytes()).unwrap();
    assert_eq!(String::from_utf8(save_bytes(&wb, "plain.csv").unwrap()).unwrap(), commas);
    // A tab-separated file named .csv round-trips with tabs.
    let tabs = "a\tb\r\n1\t2\r\n";
    let (wb, _) = open_bytes("tabs.csv", tabs.as_bytes()).unwrap();
    assert_eq!(String::from_utf8(save_bytes(&wb, "tabs.csv").unwrap()).unwrap(), tabs);
}

#[test]
fn csv_save_of_a_workbook_not_read_from_csv_uses_commas() {
    let mut wb = Workbook::new();
    if let Some(sh) = wb.sheet_mut(0) {
        sh.set_value(CellRef::new(0, 0), gridcraft_core::Value::text("a"));
        sh.set_value(CellRef::new(0, 1), gridcraft_core::Value::Number(1.0));
    }
    assert_eq!(save_bytes(&wb, "new.csv").unwrap(), b"a,1\r\n");
    // Through .xlsx the CSV origin is gone, as in Excel.
    let (from_csv, _) = open_bytes("cut list.csv", CUT_LIST.as_bytes()).unwrap();
    let (reopened, _) = open_bytes("book.xlsx", &save_bytes(&from_csv, "book.xlsx").unwrap()).unwrap();
    assert_eq!(reopened.csv_delimiter, None);
    assert!(String::from_utf8(save_bytes(&reopened, "again.csv").unwrap()).unwrap().starts_with("1,511,184,"));
    // A .tsv/.txt saved as .csv gets commas; as .tsv it keeps tabs.
    let (tsv, _) = open_bytes("list.txt", b"a\tb\r\n").unwrap();
    assert_eq!(save_bytes(&tsv, "list.csv").unwrap(), b"a,b\r\n");
    assert_eq!(save_bytes(&tsv, "list.tsv").unwrap(), b"a\tb\r\n");
}

#[test]
fn encrypted_workbook_is_reported_not_misread() {
    let file = gridcraft_xlsx::testing::encrypt_workbook(b"package", "pw");
    let msg = open_bytes("secret.xlsx", &file).unwrap_err().to_string();
    assert!(msg.contains(PASSWORD_REQUIRED), "got: {msg}");
    assert!(!msg.contains("zip"), "must not surface the zip error: {msg}");
    // A compound file naming the streams but unreadable is damage: no password is asked for,
    // and none helps.
    let file = cfb(&["EncryptionInfo", "EncryptedPackage"]);
    for password in [None, Some("pw")] {
        let msg = open_bytes_with_password("secret.xlsx", &file, password).unwrap_err().to_string();
        assert!(msg.contains("damaged") && !msg.contains(PASSWORD_REQUIRED), "got: {msg}");
    }
}

#[test]
fn encryption_no_password_opens_is_reported_without_asking_for_one() {
    let info: &[u8] = &[3, 0, 2, 0, 4, 0, 0, 0]; // Standard header flags: RC4, not AES
    let file = gridcraft_xlsx::testing::compound_file(&[("EncryptionInfo", info), ("EncryptedPackage", &[0; 8])]);
    let msg = open_bytes("rc4.xlsx", &file).unwrap_err().to_string();
    assert!(msg.contains("RC4") && !msg.contains(PASSWORD_REQUIRED), "got: {msg}");
    let irm = gridcraft_xlsx::testing::compound_file(&[("EncryptedPackage", &[0; 8])]);
    let msg = open_bytes("irm.xlsx", &irm).unwrap_err().to_string();
    assert!(msg.contains("rights-management") && !msg.contains("damaged"), "got: {msg}");
}

#[test]
fn other_compound_files_are_not_called_encrypted() {
    let err = open_bytes("old.doc", &cfb(&["WordDocument", "\u{5}SummaryInformation"])).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not an Excel workbook"), "got: {msg}");
    assert!(!msg.contains("password"), "got: {msg}");
}

/// Save As CSV/TSV writes cells as displayed: number formats applied, formula results,
/// whole values in narrow columns (never `#####`), quoting unchanged.
#[test]
fn csv_writes_displayed_text() {
    use gridcraft_core::{CellError, Value};
    use gridcraft_model::{Cell, Formula, LineInfo, NumFmt, StyleId};
    let mut wb = Workbook::new();
    let mut styled = |code: &str| wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new(code));
    let (date, money, pct, frac, text, fixed, thousands) =
        (styled("yyyy-mm-dd"), styled("$#,##0.00"), styled("0.0%"), styled("# ?/?"), styled("@"), styled("0.00"), styled("#,##0"));
    let sh = wb.sheet_mut(0).unwrap();
    let cells = [
        ("A1", Cell { value: Value::Number(46023.0), formula: None, style: date, py: None }),
        ("B1", Cell { value: Value::Number(1234.5), formula: None, style: money, py: None }),
        ("C1", Cell { value: Value::Number(0.125), formula: None, style: pct, py: None }),
        ("D1", Cell { value: Value::Number(1.5), formula: None, style: frac, py: None }),
        ("E1", Cell { value: Value::text("007"), formula: None, style: text, py: None }),
        ("F1", Cell { value: Value::Error(CellError::NA), formula: None, style: money, py: None }),
        ("G1", Cell { value: Value::Bool(true), formula: None, style: StyleId::DEFAULT, py: None }),
        ("A2", Cell { value: Value::Number(46024.0), style: date, ..Cell::formula(Formula::new("A1+1")) }),
        ("B2", Cell { value: Value::Number(12345678.9), formula: None, style: fixed, py: None }),
        ("C2", Cell { value: Value::Number(-0.25), formula: None, style: StyleId::DEFAULT, py: None }),
        ("D2", Cell { value: Value::Number(1234567.0), formula: None, style: thousands, py: None }),
    ];
    for (a, c) in cells {
        sh.set_cell(CellRef::parse(a).unwrap(), c);
    }
    // Column B is far too narrow for its numbers: the grid shows #####, the file must not.
    sh.cols.insert(1, LineInfo { size: Some(1.0), ..Default::default() });
    sh.show_formulas = true;
    let csv = String::from_utf8(save_bytes(&wb, "out.csv").unwrap()).unwrap();
    assert_eq!(csv, "2026-01-01,\"$1,234.50\",12.5%,1 1/2,007,#N/A,TRUE\r\n2026-01-02,12345678.90,-0.25,\"1,234,567\",,,\r\n");
    let tsv = String::from_utf8(save_bytes(&wb, "out.tsv").unwrap()).unwrap();
    assert!(tsv.starts_with("2026-01-01\t$1,234.50\t12.5%\t"), "{tsv}");
}

#[test]
fn delimited_exports_preserve_general_precision_and_explicit_number_formats() {
    use gridcraft_core::Value;
    use gridcraft_model::{Cell, NumFmt, StyleId};
    let mut workbook = Workbook::new();
    let scientific = workbook.styles.derive(StyleId::DEFAULT, |style| style.num_fmt = NumFmt::new("0.00E+00"));
    let sheet = workbook.sheet_mut(0).unwrap();
    sheet.set_cell(CellRef::new(0, 0), Cell { value: Value::Number(124412140710.0), ..Default::default() });
    sheet.set_cell(CellRef::new(0, 1), Cell { value: Value::Number(1.23456789012345), ..Default::default() });
    sheet.set_cell(CellRef::new(0, 2), Cell { value: Value::Number(124412140710.0), style: scientific, ..Default::default() });
    // Every integer below 1e16 that f64 holds (also 2^53 and the even integers above it) keeps
    // every digit; larger numbers keep the shortest digits that read back as the same value,
    // and a number `&` would write with an exponent is written out in full, so reopening the
    // file can't give it a scientific format that then rounds it on the next save.
    let cells = [
        (1234567890123456.0, "1234567890123456"),
        (-9007199254740991.0, "-9007199254740991"),
        (9007199254740992.0, "9007199254740992"),
        (9007199254740994.0, "9007199254740994"),
        (9123456789012346.0, "9123456789012346"),
        (-9999999999999998.0, "-9999999999999998"),
        (12345678901234567.0, "12345678901234568"),
        (1e20, "100000000000000000000"),
        (1e15 - 0.5, "999999999999999.5"),
        (1.0 / 3.0 * 1e-6, "0.000000333333333333333"),
        (1.23456789012346e-10, "0.000000000123456789012346"),
    ];
    for (col, (n, _)) in cells.iter().enumerate() {
        sheet.set_cell(CellRef::new(0, col as u32 + 3), Cell { value: Value::Number(*n), ..Default::default() });
    }
    for (path, separator) in [("data.csv", ","), ("data.tsv", "\t")] {
        let output = String::from_utf8(save_bytes(&workbook, path).unwrap()).unwrap();
        let tail: Vec<&str> = cells.iter().map(|(_, text)| *text).collect();
        assert_eq!(output, format!("124412140710{s}1.23456789012345{s}1.24E+11{s}{}\r\n", tail.join(separator), s = separator));
        // Reopening the file and saving it again changes nothing.
        let (reopened, _) = open_bytes(path, output.as_bytes()).unwrap();
        assert_eq!(String::from_utf8(save_bytes(&reopened, path).unwrap()).unwrap(), output);
    }
}

/// A number shown as General is written as `&` converts it (15 significant digits), whatever
/// colour, sections or literals the format adds, and in a Text cell. Excel writes the cell
/// display (`1.24412E+11`, `0.333333333`); GridCraft keeps the digits so part codes survive
/// (integers below 1e16 keep all their digits, and nothing gets an exponent; see the test above).
#[test]
fn delimited_exports_write_general_numbers_as_text_converts_them() {
    use gridcraft_core::Value;
    use gridcraft_model::{Cell, NumFmt, StyleId};
    let mut wb = Workbook::new();
    let mut styled = |code: &str| wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new(code));
    let (blue, signed, red, text, unit) =
        (styled("[Blue]General"), styled("General;-General"), styled("General;[Red]-General"), styled("@"), styled("General\" mm\""));
    let code = 124412140710.0;
    let sh = wb.sheet_mut(0).unwrap();
    let cells = [
        (Value::Number(1.0 / 3.0), StyleId::DEFAULT),
        (Value::Number(0.1 + 0.2), StyleId::DEFAULT),
        (Value::Number(1e20), StyleId::DEFAULT),
        (Value::Number(code), blue),
        (Value::Number(9007199254740994.0), blue),
        (Value::Number(-1.0 / 3.0), blue),
        (Value::Number(-code), signed),
        (Value::Number(-code), red),
        (Value::Number(code), text),
        (Value::Number(code), unit),
    ];
    for (col, (value, style)) in cells.into_iter().enumerate() {
        sh.set_cell(CellRef::new(0, col as u32), Cell { value, formula: None, py: None, style });
    }
    for (path, sep) in [("out.csv", ","), ("out.txt", "\t")] {
        let output = String::from_utf8(save_bytes(&wb, path).unwrap()).unwrap();
        let expected = [
            "0.333333333333333",
            "0.3",
            "100000000000000000000",
            "124412140710",
            "9007199254740994",
            "-0.333333333333333",
            "-124412140710",
            "-124412140710",
            "124412140710",
            "124412140710 mm",
        ];
        assert_eq!(output, format!("{}\r\n", expected.join(sep)));
    }
}

#[test]
fn template_extension_saves_template_content_type() {
    let bytes = save_bytes(&Workbook::new(), "Budget.xltx").unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut types = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("[Content_Types].xml").unwrap(), &mut types).unwrap();
    assert!(
        types.contains(r#"PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml""#),
        "{types}"
    );
}

/// Where `part`'s entry starts in the central directory of `package`.
fn central_entry(package: &[u8], part: &str) -> usize {
    let mut at = 0;
    while let Some(i) = package.get(at..).and_then(|rest| rest.windows(4).position(|w| w == b"PK\x01\x02")) {
        let entry = at + i;
        let len = u16::from_le_bytes([package[entry + 28], package[entry + 29]]) as usize;
        if &package[entry + 46..entry + 46 + len] == part.as_bytes() {
            return entry;
        }
        at = entry + 46;
    }
    panic!("{part} is not in the package");
}

/// `package` with `part` declaring `size` unpacked bytes: the reader refuses the part on the
/// declared size, so no real 512 MB part has to be built.
fn declaring(mut package: Vec<u8>, part: &str, size: u32) -> Vec<u8> {
    let at = central_entry(&package, part);
    package[at + 24..at + 28].copy_from_slice(&size.to_le_bytes());
    package
}

#[test]
fn a_part_over_the_size_limit_says_how_to_get_the_data_in() {
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(CellRef::new(0, 0), gridcraft_core::Value::text("big"));
    let package = save_bytes(&wb, "big.xlsx").unwrap();
    // The size reported in storytold/gridcraft#1242.
    let msg = open_bytes("big.xlsx", &declaring(package.clone(), "xl/worksheets/sheet1.xml", 540_278_731)).unwrap_err().to_string();
    assert!(msg.starts_with("We can't open 'big.xlsx': "), "{msg}");
    assert!(msg.contains("xl/worksheets/sheet1.xml") && msg.contains("515 MB") && msg.contains("512 MB"), "{msg}");
    assert!(msg.contains("several sheets") && msg.contains("CSV"), "{msg}");
    assert!(!msg.contains("bytes"), "{msg}");
    // Splitting sheets doesn't shrink the shared strings, which every sheet uses: only CSV helps.
    let msg = open_bytes("big.xlsx", &declaring(package.clone(), "xl/sharedStrings.xml", 540_278_731)).unwrap_err().to_string();
    assert!(msg.contains("xl/sharedStrings.xml") && msg.contains("515 MB") && msg.contains("CSV"), "{msg}");
    assert!(!msg.contains("several sheets"), "{msg}");
    // Just over the limit never reads as a size equal to it.
    let msg = open_bytes("big.xlsx", &declaring(package, "xl/worksheets/sheet1.xml", 512 * 1024 * 1024 + 1)).unwrap_err().to_string();
    assert!(msg.contains("more than 512 MB"), "{msg}");
    // A package that fails for another reason keeps its own error, oversized part or not.
    let mut broken = declaring(save_bytes(&wb, "big.xlsx").unwrap(), "xl/worksheets/sheet1.xml", 540_278_731);
    let at = central_entry(&broken, "xl/workbook.xml");
    broken[at + 46 + "xl/workbook.xml".len() - 1] = b'x';
    let msg = open_bytes("big.xlsx", &broken).unwrap_err().to_string();
    assert!(msg.starts_with("We can't open 'big.xlsx': ") && !msg.contains("unpacks"), "{msg}");
}

#[test]
fn content_wins_over_a_mismatched_extension() {
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(CellRef::new(0, 0), gridcraft_core::Value::Number(7.0));
    let package = save_bytes(&wb, "book.xlsx").unwrap();
    for name in ["book.xlsx", "book.ods", "book.xlsb", "book.csv"] {
        let (opened, _) = open_bytes(name, &package).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(opened.sheet(0).unwrap().value(CellRef::new(0, 0)), gridcraft_core::Value::Number(7.0), "{name}");
    }
    for name in ["data.xlsx", "data.csv"] {
        let (opened, _) = open_bytes(name, b"a,b\n1,2\n").unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(opened.sheet(0).unwrap().value(CellRef::new(0, 1)), gridcraft_core::Value::text("b"), "{name}");
    }
}

/// #2871: opening an XLSX reads Google Sheets' `IFERROR(__xludf.DUMMYFUNCTION("…"),cached)`
/// wrapper as the formula inside when GridCraft calculates every function it calls (including
/// the evaluator's own, such as ROW), and keeps the wrapper, and so the cached value, for a
/// Google-only function.
#[test]
fn opening_google_sheets_export_unwraps_only_formulas_gridcraft_calculates() {
    use gridcraft_core::{CellRef, Value};
    let mut wb = Workbook::new();
    let sh = std::sync::Arc::make_mut(&mut wb.sheets[0]);
    let wrapped = [
        ("A1", "IFERROR(__xludf.DUMMYFUNCTION(\"SUM(ROW(B1:B3))\"),6)", Value::Number(6.0)),
        ("A2", "IFERROR(__xludf.DUMMYFUNCTION(\"GOOGLETRANSLATE(\"\"hi\"\",\"\"en\"\",\"\"fr\"\")\"),\"salut\")", Value::text("salut")),
    ];
    for (at, text, value) in &wrapped {
        let cell = gridcraft_model::Cell {
            value: value.clone(),
            formula: Some(std::sync::Arc::new(gridcraft_model::Formula::new(text))),
            ..Default::default()
        };
        sh.cells.set(CellRef::parse(at).unwrap(), cell);
    }
    let bytes = gridcraft_xlsx::write_xlsx(&wb).unwrap();
    let (opened, _) = open_bytes("google.xlsx", &bytes).unwrap();
    let text = |at: &str| opened.sheets[0].cells.get(CellRef::parse(at).unwrap()).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(text("A1").as_deref(), Some("SUM(ROW(B1:B3))"));
    assert_eq!(text("A2").as_deref(), Some("IFERROR(__XLUDF.DUMMYFUNCTION(\"GOOGLETRANSLATE(\"\"hi\"\",\"\"en\"\",\"\"fr\"\")\"),\"salut\")"));
    assert_eq!(opened.sheets[0].cells.get(CellRef::parse("A2").unwrap()).map(|c| c.value.clone()), Some(Value::text("salut")));
}

#[test]
fn a_web_page_saved_as_xls_opens_its_table() {
    // #3166: a web app's "Export to Excel" is an HTML table saved as .xls; it opened as lines of
    // markup. Excel opens the table: values read as typed, <th> bold and centred, <x:Name> names
    // the sheet.
    let page = "\u{feff}<html xmlns:x=\"urn:schemas-microsoft-com:office:excel\"><head><meta charset=\"UTF-8\">\
        <!--[if gte mso 9]><xml><x:ExcelWorksheets><x:ExcelWorksheet><x:Name>Orders</x:Name></x:ExcelWorksheet></x:ExcelWorksheets></xml><![endif]-->\
        </head><body><table><thead><tr><th>Item</th><th>Qty</th><th>Code</th></tr></thead><tbody>\
        <tr><td>Kiwi &amp;\n Lime</td><td>12</td><td>007</td></tr><tr><td>Plum</td><td x:num=\"4.5\">4,50</td><td>042</td></tr>\
        </tbody></table></body></html>";
    let (wb, warnings) = open_bytes("orders.xls", page.as_bytes()).unwrap();
    assert!(warnings.first().is_some_and(|w| w.contains("is a web page (HTML)")), "{warnings:?}");
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.name, "Orders");
    let v = |a: &str| sh.value(CellRef::parse(a).unwrap());
    assert_eq!(v("A1"), gridcraft_core::Value::text("Item"));
    assert_eq!(v("A2"), gridcraft_core::Value::text("Kiwi & Lime"));
    assert_eq!(v("B2"), gridcraft_core::Value::number(12.0));
    assert_eq!(v("C2"), gridcraft_core::Value::number(7.0), "read like typed input, as Excel does");
    assert_eq!(v("B3"), gridcraft_core::Value::number(4.5), "the number the page recorded");
    let style = |a: &str| wb.styles.get(sh.style_id(CellRef::parse(a).unwrap())).clone();
    assert!(style("A1").font.bold && style("A1").align.h == gridcraft_model::HAlign::Center);
    assert!(!style("A2").font.bold);
    // Opened as text, codes keep their zeros; a page named .html gets no warning.
    let (wb, warnings) = open_bytes_with("orders.html", page.as_bytes(), None, true).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(wb.sheet(0).unwrap().value(CellRef::parse("C2").unwrap()), gridcraft_core::Value::text("007"));
    // Plain text that merely starts with a tag-like word is still read as CSV.
    let (wb, _) = open_bytes("notes.csv", b"<b>,1\nx,2\n").unwrap();
    assert_eq!(wb.sheet(0).unwrap().value(CellRef::parse("B2").unwrap()), gridcraft_core::Value::number(2.0));
}

#[test]
fn a_web_page_table_keeps_its_characters_text_and_header_formats() {
    // A page saved in Windows-1252 keeps its accented letters (it read as replacement characters).
    let mut page = b"<html><body><table><tr><th>Caf".to_vec();
    page.push(0xE9);
    page.extend_from_slice(b"</th><th>2024-01-15</th></tr><tr><td>Plum</td><td x:num=\"4.5\">4,50</td></tr></table></body></html>");
    let (wb, _) = open_bytes("menu.xls", &page).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.value(CellRef::parse("A1").unwrap()), gridcraft_core::Value::text("Café"));
    // A date in a header cell stays a date, bold and centred (its format was replaced).
    let b1 = wb.styles.get(sh.style_id(CellRef::parse("B1").unwrap()));
    assert!(matches!(sh.value(CellRef::parse("B1").unwrap()), gridcraft_core::Value::Number(_)));
    assert!(b1.font.bold && b1.align.h == gridcraft_model::HAlign::Center);
    assert_ne!(b1.num_fmt.as_str(), "General", "the date keeps its date format");
    assert_eq!(sh.value(CellRef::parse("B2").unwrap()), gridcraft_core::Value::number(4.5));
    // Opened as text, a cell keeps the text it shows, not the number the page recorded for it.
    let (wb, _) = open_bytes_with("menu.xls", &page, None, true).unwrap();
    assert_eq!(wb.sheet(0).unwrap().value(CellRef::parse("B2").unwrap()), gridcraft_core::Value::text("4,50"));
}

#[test]
fn a_web_page_table_opens_after_a_leading_comment_and_keeps_nonbreaking_spaces() {
    // An exporter's comment before <html> still makes a web page; `&nbsp;` stays a non-breaking
    // space, as Excel keeps it, while ordinary whitespace collapses.
    let page = "<!-- exported by a web app -->\n<html><body><table><tr><td>New&nbsp;&nbsp;York</td><td>a \n b</td></tr></table></body></html>";
    let (wb, warnings) = open_bytes("cities.xls", page.as_bytes()).unwrap();
    assert!(warnings.first().is_some_and(|w| w.contains("is a web page (HTML)")), "{warnings:?}");
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.value(CellRef::parse("A1").unwrap()), gridcraft_core::Value::text("New\u{a0}\u{a0}York"));
    assert_eq!(sh.value(CellRef::parse("B1").unwrap()), gridcraft_core::Value::text("a b"));
}

/// A small SYLK file written the way spreadsheets export one: a header, number formats, column
/// widths, cells that carry on from the last row or column, formulas in R1C1 and a shared formula.
const SYLK: &str = "ID;PTEST;N;E\r\nP;PGeneral\r\nP;P0.00\r\nP;Pyyyy\\-mm\\-dd\r\nF;W1 1 18\r\nF;W2 3 9\r\nB;Y5;X7\r\n\
C;Y1;X1;K\"Item\"\r\nC;X2;K\"Qty\"\r\nC;X4;K\"Total\"\r\n\
C;Y2;X1;K\"Bolts;; small\"\r\nC;X2;K12\r\nF;P1;F$2G;X3\r\nC;X3;K0.35\r\nC;X4;K4.2;ERC[-2]*RC[-1]\r\n\
C;X5;K52;ER2C2+R[1]C[-3]\r\nC;X6;KTRUE\r\nC;X7;K8.4;ERC[-3]*2\r\n\
C;Y3;X1;K\"Nuts \"\"hex\"\"\"\r\nC;X2;K40\r\nC;X3;K0.12\r\nC;X4;K4.8;ERC[-2]*RC[-1]\r\nC;X5;K\"big\";EIF(R4C4>5,\"big\",\"small\")\r\nC;X7;K9.6;S;R2;C7\r\n\
C;Y4;X1;K\"Sum\"\r\nC;X4;K9;ESUM(R[-2]C:R[-1]C)\r\nC;X5;K#DIV/0!;E1/0\r\nC;X7;K18;ESUM(C4)\r\n\
F;P2;FG0G;Y5;X2\r\nC;Y5;X2;K45000\r\nC;X3;K\"R2C3\";E\"R2C3\"\r\nE\r\n";

#[test]
fn sylk_files_open_with_values_formulas_formats_and_widths() {
    let (wb, warnings) = open_bytes("Parts list.slk", SYLK.as_bytes()).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.name, "Parts list", "the sheet is named after the file");
    let at = |a: &str| CellRef::parse(a).unwrap();
    let formula = |a: &str| sh.cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    let text = |s: &str| gridcraft_core::Value::Text(s.into());
    assert_eq!(sh.value(at("A1")), text("Item"));
    assert_eq!(sh.value(at("A2")), text("Bolts; small"), ";; is a semicolon");
    assert_eq!(sh.value(at("A3")), text("Nuts \"\"hex\"\""), "text is kept between the outer quotes as written");
    assert_eq!(sh.value(at("B2")), gridcraft_core::Value::Number(12.0));
    assert_eq!(sh.value(at("F2")), gridcraft_core::Value::Bool(true));
    assert_eq!(sh.value(at("E4")), gridcraft_core::Value::Error(gridcraft_core::CellError::Div0));
    assert_eq!(sh.value(at("D4")), gridcraft_core::Value::Number(9.0), "a formula keeps the value the file cached");
    assert_eq!(formula("D2").as_deref(), Some("B2*C2"));
    assert_eq!(formula("E2").as_deref(), Some("$B$2+B3"));
    assert_eq!(formula("E3").as_deref(), Some("IF($D$4>5,\"big\",\"small\")"));
    assert_eq!(formula("D4").as_deref(), Some("SUM(D2:D3)"));
    assert_eq!(formula("G3").as_deref(), Some("D3*2"), "a shared formula is the other cell's, moved");
    assert_eq!(formula("G4").as_deref(), Some("SUM($D:$D)"), "C4 alone is the whole column");
    assert_eq!(formula("C5").as_deref(), Some("\"R2C3\""), "text in quotes isn't a reference");
    // `F;P1;X3` formats the cell on the current row only.
    let fmt = |a: &str| wb.styles.get(sh.style_id(at(a))).num_fmt.0.to_string();
    assert_eq!(fmt("C2"), "0.00");
    assert_eq!(fmt("C3"), "General");
    assert_eq!(fmt("B5"), "yyyy\\-mm\\-dd");
    // Widths are in characters, like the Column Width dialog's.
    let chars = |c: u32| gridcraft_model::points_to_chars(f64::from(sh.col_width(c)));
    assert_eq!((chars(0), chars(1), chars(2)), (18.0, 9.0, 9.0));
    assert_eq!(sh.cols.get(&3).and_then(|i| i.size), None, "column D keeps the default width");
}

#[test]
fn sylk_r1c1_formulas_become_a1() {
    let b2 = CellRef::parse("B2").unwrap();
    for (r1c1, a1) in [
        ("RC", "B2"),
        ("R[1]C[-1]", "A3"),
        ("R1C1:R2C2", "$A$1:$B$2"),
        ("R3", "$3:$3"),
        ("R1:R[2]", "$1:4"),
        ("C[1]", "C:C"),
        ("SUM(C3:C4)", "SUM($C:$D)"),
        ("ROUND(RC[1],2)+RATE(1,2,3)", "ROUND(C2,2)+RATE(1,2,3)"),
        ("'R1C1 sheet'!R1C1", "'R1C1 sheet'!$A$1"),
        ("Sheet2!RC", "Sheet2!B2"),
        ("R2D2+rc", "R2D2+B2"),
        ("R[-5]C", "#REF!"),
        ("\"RC\"&R1C1", "\"RC\"&$A$1"),
    ] {
        assert_eq!(craft_csv::sylk::r1c1_formula_to_a1(r1c1, (b2.row, b2.col)), a1, "{r1c1}");
    }
}

#[test]
fn sylk_rejects_other_files_and_survives_damage() {
    assert!(open_bytes("not.slk", b"A,B\n1,2\n").is_err(), "a SYLK file starts with ID");
    // Positions off the sheet, broken numbers, a missing end and a format past the list.
    let (wb, warnings) =
        open_bytes("damaged.slk", b"ID;P\nC;Y0;X1;K1\nC;Y2;X99999;K2\nC;Y3;X1;K1e999\nF;P7;Y3;X1\nF;W5 2 9\nC;Y4;X1;K5;E(((\n").unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.cells.iter().count(), 2, "{:?}", sh.cells.iter().collect::<Vec<_>>());
    assert_eq!(sh.value(CellRef::parse("A3").unwrap()), gridcraft_core::Value::Text("1e999".into()));
    assert_eq!(warnings.len(), 1, "skipped records are reported: {warnings:?}");
}

#[test]
fn sylk_survives_huge_relative_offsets_and_width_floods() {
    // Relative offsets at the edge of i64 fall off the sheet instead of overflowing.
    let text = b"ID;P\nC;Y1;X1;K0;ER[9223372036854775807]C\nC;Y2;X1;K0;ER[-9223372036854775808]C[-9223372036854775808]\nE\n";
    assert!(open_bytes("offsets.slk", text).is_ok());
    // Width records over every column, many times over: the import keeps a bounded number.
    let mut flood = b"ID;P\n".to_vec();
    for _ in 0..5000 {
        flood.extend_from_slice(b"F;W1 16384 12\n");
    }
    flood.extend_from_slice(b"C;Y1;X1;K1\nE\n");
    let (wb, _) = open_bytes("widths.slk", &flood).unwrap();
    assert_eq!(wb.sheet(0).unwrap().value(CellRef::parse("A1").unwrap()), gridcraft_core::Value::Number(1.0));
}

/// A FIFO with no writer would block an open forever: it is refused before it is opened.
#[cfg(unix)]
#[test]
fn reading_a_fifo_is_refused_without_blocking() {
    let dir = std::env::temp_dir().join(format!("gridcraft-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("pipe.xlsx");
    if !std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success()) {
        let _ = std::fs::remove_dir_all(&dir);
        return; // no mkfifo here
    }
    let err = read_file(fifo.to_str().unwrap()).unwrap_err().to_string();
    assert!(err.contains("only regular files"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Merges far from what is copied or exported, in a hostile number, are indexed once instead of
/// being scanned for every cell; a merge running past the table is clipped to it.
#[test]
fn html_merge_lookups_and_spans_stay_bounded() {
    let mut wb = book_with_picture("A1", (0, 0));
    let sh = wb.sheet_mut(0).unwrap();
    for i in 0..200_000u32 {
        sh.merges.push(RangeRef::new(CellRef::new(500_000 + i * 2, 5), CellRef::new(500_000 + i * 2, 6)));
    }
    sh.merges.push(RangeRef::new(CellRef::new(0, 0), CellRef::new(gridcraft_core::MAX_ROWS - 1, 1)));
    sh.set_value(CellRef::new(2, 1), gridcraft_core::Value::Number(2.0));
    let html = copied(&wb, "A1:C50");
    assert!(html.contains("rowspan=\"50\" colspan=\"2\""), "{html}");
    let page = to_html(&wb, 0).unwrap();
    // The table is A1:B3 (the used range): the whole-height merge spans its three rows only.
    assert!(page.contains("rowspan=\"3\" colspan=\"2\""), "{page}");
    // Merges stacked over one another past a few per cell are refused, not walked.
    let sh = wb.sheet_mut(0).unwrap();
    sh.merges = vec![RangeRef::new(CellRef::new(0, 0), CellRef::new(2, 1)); 100];
    assert!(to_html(&wb, 0).is_err());
    assert!(range_html(&wb, 0, RangeRef::parse("A1:B3").unwrap(), "").is_none());
}

/// Thousands of pictures over thousands of custom rows: each is placed through a row index.
#[test]
fn html_export_places_many_pictures_over_many_custom_rows() {
    let mut wb = book_with_picture("A1", (0, 0));
    let sh = wb.sheet_mut(0).unwrap();
    for row in 0..50_000 {
        sh.rows.insert(row, gridcraft_model::LineInfo { size: Some(30.0), ..Default::default() });
    }
    let image = sh.images[0].clone();
    for i in 0..5_000u32 {
        let mut im = image.clone();
        im.anchor.cell = CellRef::new(40_000 + i, 0);
        sh.images.push(im);
    }
    let page = to_html(&wb, 0).unwrap();
    assert!(page.contains(&format!("top:{}px", 30 * 44_999)), "last picture placed by the index");
}

/// One long text shown by many cells: the page fails at its byte limit instead of growing on.
#[test]
fn html_export_refuses_a_page_past_its_byte_limit() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    let text = gridcraft_core::Value::text("x".repeat(32_000));
    for row in 0..10_000 {
        sh.set_value(CellRef::new(row, 0), text.clone());
    }
    let err = to_html(&wb, 0).unwrap_err();
    assert!(err.to_string().contains("MiB"), "{err}");
}

#[test]
fn escaping_stops_at_the_room_left() {
    assert_eq!(esc_within("a&b<c>\"d", 100).as_deref(), Some(esc("a&b<c>\"d").as_str()));
    assert_eq!(esc_within("&&&", 14), None, "three `&amp;` are 15 bytes");
    assert_eq!(esc_within("&&&", 15).as_deref(), Some("&amp;&amp;&amp;"));
}

#[test]
fn the_merge_index_holds_a_bounded_number_of_cells() {
    let mut sh = Sheet::new("S");
    let rect = RangeRef::new(CellRef::new(0, 0), CellRef::new(1_100_000, 0));
    sh.merges.push(RangeRef::new(CellRef::new(0, 0), CellRef::new(999_999, 0)));
    assert_eq!(merge_index(&sh, rect).map(|m| m.len()), Some(1_000_000));
    sh.merges = vec![RangeRef::new(CellRef::new(0, 0), CellRef::new(1_000_000, 0))];
    assert!(merge_index(&sh, rect).is_none());
}
