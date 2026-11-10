//! The open-documents handler's walk and decode, [`odoc_paths`], on real
//! `NSAppleEventDescriptor`s built in-process: no Apple event is sent, and no main
//! thread is needed (the handler class is main-thread-only, and libtest never runs
//! a test on the main thread, so the handler itself is not called here).

use crate::macos::odoc_paths;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, msg_send};
use objc2_foundation::{NSAppleEventDescriptor, NSData, NSString, NSURL};
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

/// #6324 on Apple's own bookmark bytes, measured on macOS 13.3 only: an NSURL
/// bookmark of a file under an NFC-named directory, as a `'bmrk'` item (the
/// Finder/`open` shape), has no `fileURLValue` in-process, and the handler's raw
/// decode delivers the on-disk NFC path, in a list and as the direct object itself.
#[test]
fn an_nfc_bookmark_that_file_url_value_cannot_resolve_is_delivered() {
    // A bookmark records the resolved path, and the temp dir is under the /var symlink.
    let temp = std::env::temp_dir().canonicalize().expect("the temp dir");
    let directory = temp.join(format!("fmv-bookmark-{}", std::process::id()));
    let file = directory.join("\u{00c1}rea fiscal").join("doc.pdf");
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("create the directory");
    std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
    let url = NSURL::fileURLWithPath(&NSString::from_str(file.to_str().expect("UTF-8")));
    // SAFETY: -[NSURL bookmarkDataWithOptions:includingResourceValuesForKeys:relativeToURL:error:]
    // takes an NSUInteger, two nullable objects and a nullable NSError**, and returns a
    // nullable NSData; nil keys, nil base URL and a null error pointer are all allowed.
    // objc2-foundation binds this selector as a safe method behind its NSArray and NSError
    // features, which this crate does not enable.
    let data: Option<Retained<NSData>> = unsafe {
        msg_send![&url, bookmarkDataWithOptions: 0usize, includingResourceValuesForKeys: None::<&AnyObject>,
            relativeToURL: None::<&NSURL>, error: std::ptr::null_mut::<*mut AnyObject>()]
    };
    let data = data.expect("bookmark data");
    // SAFETY: +[NSAppleEventDescriptor descriptorWithDescriptorType:data:] takes a DescType
    // (a FourCharCode, whose ABI is u32) and a nullable NSData, and returns a nullable descriptor.
    // objc2-foundation binds it as a safe method behind its objc2-core-services feature, which
    // this crate does not enable.
    let bookmark: Option<Retained<NSAppleEventDescriptor>> = unsafe {
        msg_send![NSAppleEventDescriptor::class(), descriptorWithDescriptorType: u32::from_be_bytes(*b"bmrk"), data: &*data]
    };
    let bookmark = bookmark.expect("a 'bmrk' descriptor");
    // The decoder first: on a macOS that does resolve this bookmark, it is still checked.
    assert_eq!(odoc_paths(&list(&[&bookmark])), std::slice::from_ref(&file));
    assert_eq!(odoc_paths(&bookmark), std::slice::from_ref(&file));
    assert!(
        bookmark.fileURLValue().is_none(),
        "fileURLValue resolved an NFC bookmark: #6324's premise (macOS 13.3) no longer holds here"
    );
    std::fs::remove_dir_all(&directory).expect("clean up");
}
