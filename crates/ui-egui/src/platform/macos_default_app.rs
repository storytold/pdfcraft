//! The app that opens PDFs on macOS, and making this PdfCraft that app when the user asks (#8573).
//!
//! A default left on an app that was since removed makes macOS fall back to Preview, and Finder's
//! Get Info ▸ Change All may not replace it. LaunchServices' content-type binding does.
//! Nothing here runs on its own: the default changes only from the Preferences button, and macOS
//! asks the user to confirm (#2363 is about an app taking over the default unasked).

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use super::macos_association::BUNDLE_ID;

/// The app LaunchServices opens PDFs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Opener {
    /// This running copy of PdfCraft.
    ThisApp,
    /// Another copy of PdfCraft, by its folder. macOS binds the default to the bundle identifier,
    /// so pressing the button again from this copy would change nothing.
    OtherCopy(String),
    /// Another app, by the name the Finder shows.
    Other(String),
    /// LaunchServices names no app.
    Nothing,
    /// This macOS (before 12) has no way to ask which app opens PDFs.
    Unknown,
}

/// Where the last "make PdfCraft the default" request stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    /// None was made since Preferences opened.
    Idle,
    /// macOS hasn't answered yet (it may be asking the user).
    Waiting,
    /// macOS answered; `Opener` says what it did.
    Answered,
}

/// What the Preferences row shows: refreshed when Preferences opens and when macOS answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub opener: Opener,
    pub request: Request,
    /// Whether this process is an installed app bundle LaunchServices can bind (a `cargo run` build isn't).
    pub can_set: bool,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);

/// This running bundle, when it is the PdfCraft app.
fn this_bundle() -> Option<PathBuf> {
    use objc2_foundation::NSBundle;
    let bundle = NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none_or(|id| id.to_string() != BUNDLE_ID) {
        return None;
    }
    bundle.bundleURL().to_file_path()
}

/// The PDF content type, by the identifier PdfCraft's Info.plist declares.
fn pdf_type() -> Option<objc2::rc::Retained<objc2_uniform_type_identifiers::UTType>> {
    objc2_uniform_type_identifiers::UTType::typeWithIdentifier(&objc2_foundation::NSString::from_str("com.adobe.pdf"))
}

/// Ask LaunchServices which app opens PDFs now.
fn query() -> Opener {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSFileManager, NSString};
    // `URLForApplicationToOpenContentType:` is macOS 12 and later; the app runs from macOS 11.
    if !objc2::available!(macos = 12.0) {
        return Opener::Unknown;
    }
    let Some(pdf) = pdf_type() else { return Opener::Nothing };
    let Some(url) = NSWorkspace::sharedWorkspace().URLForApplicationToOpenContentType(&pdf) else { return Opener::Nothing };
    let Some(path) = url.to_file_path() else { return Opener::Nothing };
    if this_bundle().is_some_and(|me| me == path) {
        return Opener::ThisApp;
    }
    let same_id = objc2_foundation::NSBundle::bundleWithURL(&url).and_then(|b| b.bundleIdentifier()).is_some_and(|id| id.to_string() == BUNDLE_ID);
    if same_id {
        let folder = path.parent().map_or_else(String::new, |p| p.to_string_lossy().into_owned());
        return Opener::OtherCopy(folder);
    }
    let name = NSFileManager::defaultManager().displayNameAtPath(&NSString::from_str(&path.to_string_lossy())).to_string();
    Opener::Other(name.strip_suffix(".app").unwrap_or(&name).to_owned())
}

/// Read the current opener again, keeping the request's state.
pub(crate) fn refresh(request: Request) {
    let status = Status { opener: query(), request, can_set: this_bundle().is_some() };
    *STATUS.lock().unwrap_or_else(PoisonError::into_inner) = Some(status);
}

/// Read the current opener again when Preferences opens. A request macOS hasn't answered stays
/// waiting, so reopening Preferences can't enable the button for a second request.
pub(crate) fn reread() {
    let opener = query();
    let can_set = this_bundle().is_some();
    let mut slot = STATUS.lock().unwrap_or_else(PoisonError::into_inner);
    let request = match slot.as_ref().map(|s| s.request) {
        Some(Request::Waiting) => Request::Waiting,
        _ => Request::Idle,
    };
    *slot = Some(Status { opener, request, can_set });
}

/// The last status read, reading it the first time.
pub(crate) fn status() -> Status {
    if let Some(status) = STATUS.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        return status;
    }
    refresh(Request::Idle);
    STATUS.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or(Status {
        opener: Opener::Nothing,
        request: Request::Idle,
        can_set: false,
    })
}

/// Ask macOS to open PDFs with this PdfCraft. macOS may ask the user first; when it answers, the
/// status is read again (an error, such as the user keeping the other app, leaves that app shown).
/// Returns false when the request couldn't be made (not an installed bundle, or macOS before 12).
pub(crate) fn make_default(ctx: &egui::Context) -> bool {
    use block2::RcBlock;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSError, NSURL};
    if !objc2::available!(macos = 12.0) {
        return false;
    }
    let (Some(app), Some(pdf)) = (this_bundle(), pdf_type()) else { return false };
    let Some(app_url) = NSURL::from_file_path(&app) else { return false };
    refresh(Request::Waiting);
    let ctx = ctx.clone();
    let completion = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            log::info!("PdfCraft is now the app that opens PDFs");
        } else {
            log::info!("macOS kept the app that opens PDFs");
        }
        refresh(Request::Answered);
        ctx.request_repaint();
    });
    NSWorkspace::sharedWorkspace().setDefaultApplicationAtURL_toOpenContentType_completionHandler(&app_url, &pdf, Some(&completion));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build that isn't the installed PdfCraft bundle (this test binary, `cargo run`) never asks
    /// LaunchServices to change the developer's PDF app, and the row says why its button is off.
    #[test]
    fn a_process_outside_the_app_bundle_never_changes_the_default() {
        refresh(Request::Idle);
        let before = status();
        assert!(!before.can_set);
        assert!(!make_default(&egui::Context::default()));
        let after = status();
        assert_eq!(after.request, Request::Idle, "no request was made");
        assert_eq!(after.opener, before.opener);
    }
}
