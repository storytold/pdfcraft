//! macOS open-documents and quit Apple events (#73).
//!
//! Finder double-clicks, Open With, drops on the Dock icon and `open -a PdfCraft file.pdf` don't
//! pass paths on the command line: LaunchServices sends the running (or just-launched) app a
//! `kAEOpenDocuments` ('odoc') Apple event. winit 0.30 doesn't handle it and owns the
//! `NSApplicationDelegate`, so AppKit answered "PdfCraft cannot open files in the PDF document
//! format". Handling it ourselves needs Objective-C class declarations, i.e. `unsafe`, which this
//! workspace forbids. The audited `fmv-macos-events` crate (also used by PhotoCraft) wraps exactly
//! that, an `NSAppleEventManager` handler registered before Finder's launch event that leaves
//! winit's delegate alone, behind a safe main-thread API. This module forwards its events to the
//! app's file intake and quit handling.

use craft::platform::open::IntakeSender;
use fmv_macos_events::{Event, Inbox, Registration};

/// Keeps the Apple-event handlers registered; hold it until the event loop returns.
pub struct AppleEvents {
    _registration: Registration,
    inbox: Inbox,
}

impl AppleEvents {
    /// Register the handlers. Call on the main thread before the event loop starts, so the event
    /// that launched the app (a Finder double-click) is caught too.
    pub fn install() -> Self {
        let (registration, inbox) = Registration::install();
        Self { _registration: registration, inbox }
    }

    /// Forwards the events to the app: opened documents go to `sender`, a quit goes to the quit
    /// menu's handler on `ctx`. Events arriving later wake `ctx`.
    ///
    /// The inbox belongs to the main thread (it is not `Send`, and drains inside its own wake
    /// would borrow it twice), so the returned function does the forwarding: the frame calls it
    /// once per frame on the main thread.
    // PLAN: pdf-hub-w1 calls the returned function once per frame (it replaces the `OsEventsFn` poll).
    pub fn connect(&self, sender: IntakeSender, ctx: &egui::Context) -> Box<dyn Fn()> {
        let wake = ctx.clone();
        self.inbox.set_wake(move || wake.request_repaint());
        let inbox = self.inbox.clone();
        let ctx = ctx.clone();
        Box::new(move || {
            for event in inbox.drain() {
                match event {
                    Event::Open(paths) => sender.open(paths),
                    Event::Quit => {
                        craft::quit::on_menu_event(&ctx, craft::quit::QUIT_MENU_ID);
                    }
                }
            }
        })
    }
}
