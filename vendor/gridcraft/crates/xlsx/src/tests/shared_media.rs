//! A picture used on several sheets is written as one media part that every drawing points to.

use std::io::Read;
use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, Image, Sheet, Workbook};

use crate::{read_xlsx, write_xlsx};

fn png(fill: u8) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend(std::iter::repeat_n(fill, 4096));
    v
}

fn sheet_with(name: &str, pictures: &[(u32, &[u8])]) -> Arc<Sheet> {
    let mut s = Sheet::new(name);
    for (id, data) in pictures {
        let anchor = Anchor { cell: CellRef::parse("B2").unwrap(), width: 40.0, height: 30.0, ..Default::default() };
        s.images.push(Image { id: *id, anchor, data: data.to_vec().into(), mime: "image/png".into(), alt: String::new() });
    }
    Arc::new(s)
}

fn names(bytes: &[u8]) -> Vec<String> {
    let z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    z.file_names().map(str::to_string).collect()
}

fn text(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn a_picture_on_several_sheets_is_one_media_part() {
    let (a, b) = (png(1), png(2));
    // `a` on three sheets (and twice on one of them), `b` on one: two media parts, not five.
    let wb = Workbook {
        sheets: vec![sheet_with("One", &[(1, &a)]), sheet_with("Two", &[(1, &a), (2, &b)]), sheet_with("Three", &[(1, &a), (2, &a)])],
        ..Default::default()
    };
    let bytes = write_xlsx(&wb).unwrap();
    let media: Vec<String> = names(&bytes).into_iter().filter(|n| n.starts_with("xl/media/")).collect();
    assert_eq!(media.len(), 2, "{media:?}");
    // Every drawing relationship points to an existing part, and the three sheets that show `a`
    // point to the same one.
    let mut targets = vec![];
    for d in 1..=3 {
        let rels = text(&bytes, &format!("xl/drawings/_rels/drawing{d}.xml.rels"));
        for t in rels.split("Target=\"").skip(1) {
            let t = t.split('"').next().unwrap();
            let part = format!("xl/media/{}", t.trim_start_matches("../media/"));
            assert!(media.contains(&part), "drawing{d}: dangling {t}");
            targets.push((d, part));
        }
    }
    assert_eq!(targets.len(), 5, "{targets:?}");
    let distinct: std::collections::BTreeSet<&String> = targets.iter().map(|(_, p)| p).collect();
    assert_eq!(distinct.len(), 2, "{targets:?}");
    let first_of = |d: usize| targets.iter().find(|(x, _)| *x == d).map(|(_, p)| p.clone());
    assert_eq!(first_of(1), first_of(2), "sheets One and Two show the same picture");
    assert_eq!(first_of(1), first_of(3));
    // They read back with the right bytes on the right sheets.
    let back = read_xlsx(&bytes).unwrap().0;
    let data = |i: usize| back.sheets[i].images.iter().map(|im| im.data.to_vec()).collect::<Vec<_>>();
    assert_eq!(data(0), vec![a.clone()]);
    assert_eq!(data(1), vec![a.clone(), b.clone()]);
    assert_eq!(data(2), vec![a.clone(), a.clone()]);
    // A second save of what was read back is stable.
    let again = write_xlsx(&back).unwrap();
    assert_eq!(names(&again).iter().filter(|n| n.starts_with("xl/media/")).count(), 2);
}

#[test]
fn different_pictures_with_the_same_length_stay_separate() {
    let (a, b) = (png(1), png(2));
    assert_eq!(a.len(), b.len());
    let wb = Workbook { sheets: vec![sheet_with("One", &[(1, &a), (2, &b)])], ..Default::default() };
    let bytes = write_xlsx(&wb).unwrap();
    assert_eq!(names(&bytes).iter().filter(|n| n.starts_with("xl/media/")).count(), 2);
    let back = read_xlsx(&bytes).unwrap().0;
    assert_eq!(back.sheets[0].images.iter().map(|i| i.data.to_vec()).collect::<Vec<_>>(), vec![a, b]);
}

/// Many distinct pictures that share a long prefix: each is matched against the earlier ones
/// through a content hash, not by comparing bytes with every one of them.
#[test]
fn many_distinct_pictures_with_a_common_prefix_are_each_written_once() {
    let pictures: Vec<Vec<u8>> = (0..400u32)
        .map(|i| {
            let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
            v.extend(std::iter::repeat_n(7u8, 65_536));
            v.extend(i.to_le_bytes());
            v
        })
        .collect();
    let refs: Vec<(u32, &[u8])> = pictures.iter().enumerate().map(|(i, p)| (i as u32 + 1, p.as_slice())).collect();
    let wb = Workbook { sheets: vec![sheet_with("One", &refs), sheet_with("Two", &refs)], ..Default::default() };
    let bytes = write_xlsx(&wb).unwrap();
    assert_eq!(names(&bytes).iter().filter(|n| n.starts_with("xl/media/")).count(), 400);
}
