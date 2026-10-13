# Mobile Security Review (iOS & Android)

This document details the security review findings and mitigations across the mobile architecture (MOB-7.4).

---

## 1. Sandbox Path Confinement

### Threat
Malicious PDF documents, tools, or inputs attempting to read or overwrite files outside the app's sandboxed container directory (e.g. via `../` traversal, symlink traversal, or absolute host paths).

### Mitigations
1. **Engine Sandbox Confinement (`crates/ffi/src/engine.rs`):**
   - Implemented in `open_file(path_str, password)` and tested in `crates/ffi/tests/open_close.rs` (`test_open_file_with_sandbox`).
   - If a sandbox root is configured, all file paths are canonicalized and verified using `std::path::Path::starts_with(&sandbox_root)`.
   - Any path escaping the sandbox produces `FfiError::PathEscape { path }` and is blocked immediately.
2. **Platform Sandboxing:**
   - **iOS:** App Sandbox isolates container storage (`Documents`, `Library/Caches`, `tmp`). All user document access from outside the container goes through security-scoped bookmarks or file coordination.
   - **Android:** App-specific internal storage (`context.filesDir`, `context.cacheDir`) is inaccessible to other apps. Outside documents are accessed exclusively via content URIs from the Storage Access Framework (`Scoped Storage`).

---

## 2. Intent Filters & Inter-Process Communication

### Threat
Unauthorized external applications invoking internal activities, hijacking document handles, or injecting malformed payloads into unexported components.

### Mitigations
- In `mobile/android/app/src/main/AndroidManifest.xml`, only the `MainActivity` launcher is exported (`android:exported="true"`).
- All internal receivers, services, and content providers are strictly non-exported (`android:exported="false"`).
- Document sharing and viewing intents use read-only content URIs with `FLAG_GRANT_READ_URI_PERMISSION`.

---

## 3. URL and Link Handling

### Threat
PDF documents embedding malicious action URIs (e.g., `javascript:`, `file:///`, `content://`, or local IPC schemes) attempting code execution or unauthorized local file reads.

### Mitigations
- PDF links are parsed as typed `LinkTarget::Uri(String)`.
- Platform UI bridges sanitize URLs prior to dispatch:
  - Only `https://` and `http://` schemes are permitted for external browser launch.
  - Dangerous pseudo-schemes (`javascript:`, `data:`, `file:`, `app:`) are filtered out and blocked.

---

## 4. Archive & Attachment Extraction

### Threat
Zip bombs, path traversal in embedded files, or directory traversal attacks via malicious `/F` or `/UF` names in embedded file streams.

### Mitigations
- Embedded file names are sanitized: path separators (`/`, `\`, `:`) and directory traversal segments (`..`) are stripped, extracting files strictly as flat basenames into a sandboxed destination directory.
- Decompression limits (Budget B3) are enforced across all stream filters (Flate, LZW, Crypt) in `pdfcraft-cos` and `pdfcraft-engine`.
