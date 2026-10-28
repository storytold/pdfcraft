# PR: Control channel: Wait After a Failed Accept Instead of Spinning at Once (#3631)

## Summary
- **Issue Reference:** Resolves #3631
- **Crate:** `crates/ui-egui`
- **Scope:** Socket accept loop in `serve_with` (`crates/ui-egui/src/control.rs`).

## Root Cause
The UI control channel's accepting thread previously consumed `listener.incoming().flatten()`. If `accept()` continuously failed (for example, when file descriptors were exhausted under heavy load or OS constraints), `flatten()` dropped the error and immediately invoked `accept()` in a tight loop, pegging an entire CPU core at 100% utilization.

## Fix
- Replaced `listener.incoming().flatten()` with explicit matching on each incoming `std::io::Result<TcpStream>`.
- On error, log a warning and sleep 100ms before retrying, matching the robust backoff behavior in `apps/pdfcraft/src/single_instance.rs`.
- Successfully accepted connections and throughput are unaffected.
