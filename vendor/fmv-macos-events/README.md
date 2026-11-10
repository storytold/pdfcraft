# Mac document-event adapter

This small local crate registers handlers for standard open-documents and quit
Apple events. It keeps winit's NSApplication delegate intact. Registration occurs
before the event loop and again at will-finish-launching so Finder's cold launch
event reaches the queue. Later events wake the root egui viewport; the viewer
uses its existing loading, deduplication, and normal-exit paths.

The adapter also adds standard responder-chain Edit shortcuts for native file
dialog text fields. An Apple-event quit aborts an active modal panel before
queuing the viewer's normal exit, so the nested dialog loop cannot strand it.

Objective-C interoperation requires unsafe declarations and message sends. They
are isolated here behind a safe main-thread-only API, with safety comments at
each boundary. The application crate still forbids unsafe Rust. Retained handlers
outlive the event loop and unregister before release. No polling, socket, helper
process, or persistent event queue is introduced.

Native packaged-app checks in scripts/test-macos-package.py cover actual
LaunchServices delivery rather than substituting CLI arguments for Finder
events. This crate uses the existing objc2 stack and is MIT OR Apache-2.0 licensed.

Published as fmv-macos-events. Requires macOS; other targets intentionally expose no API.
