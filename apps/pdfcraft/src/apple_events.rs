//! macOS open-documents and quit Apple events (#73).
//!
//! Finder double-clicks, Open With, drops on the Dock icon and `open -a PdfCraft file.pdf` don't
//! pass paths on the command line: LaunchServices sends the running (or just-launched) app a
//! `kAEOpenDocuments` ('odoc') Apple event. winit 0.30 doesn't handle it and owns the
//! `NSApplicationDelegate`, so AppKit answered "PdfCraft cannot open files in the PDF document
//! format". Handling it ourselves needs Objective-C class declarations, i.e. `unsafe`, which this
//! workspace forbids. The audited `fmv-macos-events` crate (also used by PhotoCraft) wraps exactly
//! that, an `NSAppleEventManager` handler registered before Finder's launch event that leaves
//! winit's delegate alone, behind a safe main-thread API. This module adapts it to the
//! [`OsEvent`]s that the UI polls every frame.

use std::sync::{Arc, Mutex};

use fmv_macos_events::{Event, Inbox, Registration};
use pdfcraft_ui_egui::{OsEvent, OsEventsFn};

/// Keeps the Apple-event handlers registered; hold it until the event loop returns.
pub struct AppleEvents {
    _registration: Registration,
    inbox: Inbox,
    menu_inbox: Arc<Mutex<Vec<String>>>,
}

impl AppleEvents {
    /// Register the handlers. Call on the main thread before the event loop starts, so the event
    /// that launched the app (a Finder double-click) is caught too.
    pub fn install() -> Self {
        let (registration, inbox) = Registration::install();
        Self { _registration: registration, inbox, menu_inbox: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Access the shared menu inbox for delivering native menu events.
    pub fn menu_inbox(&self) -> Arc<Mutex<Vec<String>>> {
        self.menu_inbox.clone()
    }

    /// The queue the UI drains (`PdfCraftApp::os_events`); events arriving later wake `ctx`.
    pub fn connect(&self, ctx: &egui::Context) -> OsEventsFn {
        let ctx = ctx.clone();
        self.inbox.set_wake(move || ctx.request_repaint());
        let inbox = self.inbox.clone();
        let menu_inbox = self.menu_inbox.clone();
        Box::new(move || {
            let mut events: Vec<OsEvent> = inbox
                .drain()
                .into_iter()
                .map(|e| match e {
                    Event::Open(paths) => OsEvent::Open(paths.into_iter().map(|p| p.to_string_lossy().into_owned()).collect()),
                    Event::Quit => OsEvent::Quit,
                })
                .collect();
            if let Ok(mut menu) = menu_inbox.lock() {
                for cmd in menu.drain(..) {
                    events.push(OsEvent::Command(cmd));
                }
            }
            events
        })
    }
}
