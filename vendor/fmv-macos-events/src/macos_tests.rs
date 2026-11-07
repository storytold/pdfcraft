//! The open-documents handler's walk and decode, [`odoc_paths`], on real
//! `NSAppleEventDescriptor`s built in-process: no Apple event is sent, and no main
//! thread is needed (the handler class is main-thread-only, and libtest never runs
//! a test on the main thread, so the handler itself is not called here).

use crate::macos::odoc_paths;
use objc2::rc::Retained;
use objc2_foundation::{NSAppleEventDescriptor, NSString, NSURL};
use std::path::{Path, PathBuf};

/// A `'furl'` descriptor, as AppleScript's `POSIX file "…"` sends one.
fn file_url(path: &Path) -> Retained<NSAppleEventDescriptor> {
    let path = NSString::from_str(path.to_str().expect("test paths are UTF-8"));
    NSAppleEventDescriptor::descriptorWithFileURL(&NSURL::fileURLWithPath(&path))
}

/// A list descriptor holding `items`, in order.
fn list(items: &[&NSAppleEventDescriptor]) -> Retained<NSAppleEventDescriptor> {
    let list = NSAppleEventDescriptor::listDescriptor();
    for item in items {
        list.insertDescriptor_atIndex(item, 0); // index 0 appends
    }
    list
}

/// AppleScript's `open POSIX file "…"` without braces makes the `'furl'` itself the
/// direct object. A descriptor that is not a list counts no items, so the
/// published crate's walk of `1..=numberOfItems()` queued nothing for it; it is now
/// decoded as the one item. In a list, the same `'furl'` yields the same path.
#[test]
fn a_file_url_direct_object_is_decoded_as_the_one_item() {
    let directory = std::env::temp_dir().join(format!("fmv-direct-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create the directory");
    let file = directory.join("doc.pdf");
    std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
    let direct = file_url(&file);
    // The premise the handler relies on.
    assert_eq!(direct.numberOfItems(), 0);
    assert!(direct.descriptorAtIndex(1).is_none());
    assert_eq!(odoc_paths(&direct), std::slice::from_ref(&file));
    assert_eq!(odoc_paths(&list(&[&direct])), std::slice::from_ref(&file));
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// A partial multi-file open through the handler's own walk and decode: the file
/// that exists and the one that does not are both delivered, in order (never drop;
/// the app's open error reports the missing one).
#[test]
fn a_list_delivers_each_item_in_order_existing_or_not() {
    let directory = std::env::temp_dir().join(format!("fmv-list-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create the directory");
    let file = directory.join("doc.pdf");
    std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
    let missing = Path::new("/definitely/missing/nowhere.pdf");
    let direct = list(&[&file_url(&file), &file_url(missing)]);
    assert_eq!(direct.numberOfItems(), 2);
    assert_eq!(odoc_paths(&direct), [file.clone(), missing.to_path_buf()]);
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// Lists are not flattened: a list inside the list is one item of no type the raw
/// decode knows, so it takes the published crate's NSURL chain alone, whose
/// `fileURLValue` reads a one-item list as its item.
#[test]
fn a_list_inside_the_list_takes_the_nsurl_chain() {
    let directory = std::env::temp_dir().join(format!("fmv-nested-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create the directory");
    let file = directory.join("doc.pdf");
    std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
    let inner = list(&[&file_url(&file)]);
    assert_eq!(odoc_paths(&list(&[&inner])), std::slice::from_ref(&file));
    std::fs::remove_dir_all(&directory).expect("clean up");
}

/// Nothing to open: an empty list and a null descriptor decode to no path, so the
/// handler still queues no `Event::Open`, as in the published crate.
#[test]
fn an_empty_list_or_a_null_direct_object_yields_no_path() {
    assert_eq!(odoc_paths(&NSAppleEventDescriptor::listDescriptor()), Vec::<PathBuf>::new());
    assert_eq!(odoc_paths(&NSAppleEventDescriptor::nullDescriptor()), Vec::<PathBuf>::new());
}
