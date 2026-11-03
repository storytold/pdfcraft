# PdfCraft Android Native Port — System Architecture

**Document:** `docs/android/ARCHITECTURE.md`  
**Date:** November 2026  
**Status:** Approved Architecture  
**Package:** `io.github.dikshant363.pdfcraft`  
**Target Platform:** Android 12+ (API level 31+)  
**Primary Architectures:** `arm64-v8a`, `x86_64`  

---

## 1. Architectural Philosophy

PdfCraft Android is a **local-first, offline-by-default, native document engineering workstation**. It departs strictly from cross-platform web wrappers (such as WebView, Cordova, Electron) and hybrid runtimes (Flutter, React Native). 

The application couples two specialized runtimes:
1. **Deterministic Rust Systems Core**: High-throughput PDF parsing, vector mathematics, font rasterization, appearance stream generation, digital signature cryptography, and headless tool automation.
2. **Modern Android Presentation Layer**: 100% Kotlin, Jetpack Compose, Material 3, AndroidX Lifecycle, and Kotlin Coroutines.

```
┌─────────────────────────────────────────────────────────────────┐
│               Jetpack Compose Android UI (Material 3)           │
│   ┌──────────────┐   ┌───────────────┐   ┌──────────────────┐   │
│   │  HomeScreen  │   │  ViewerScreen │   │  ToolScreens/Edit│   │
│   └──────┬───────┘   └───────┬───────┘   └────────┬─────────┘   │
└──────────┼───────────────────┼────────────────────┼─────────────┘
           ▼                   ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                 AppViewModel & StateFlow State Layer            │
│  • Document Lifecycle    • Selection State    • Tool State      │
│  • Undo / Redo Stacks    • Page Image LRU     • Auto-Save Timer │
└──────────────────────────────┬──────────────────────────────────┘
                               │ Coroutine Dispatchers (Default/IO)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│         UniFFI Auto-Generated Kotlin Native Bridge Interface    │
│            io.github.dikshant363.pdfcraft.core                  │
│       • MobileSession             • DocumentInfo & PageInfo     │
│       • RenderedImage & TileRect  • SearchHit & ReadingBlock    │
└──────────────────────────────┬──────────────────────────────────┘
                               │ C-ABI Dynamic Link (JNA)
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│          pdfcraft-android (Rust cdylib: libpdfcraft_android.so) │
│  ┌───────────────────────┐       ┌───────────────────────────┐  │
│  │ MobileSession (Mutex) │       │   Renderers (Per-thread)  │  │
│  └───────────┬───────────┘       └─────────────┬─────────────┘  │
└──────────────┼─────────────────────────────────┼────────────────┘
               ▼                                 ▼
┌──────────────────────────────┐   ┌──────────────────────────────┐
│     pdfcraft-automation      │   │       pdfcraft-render        │
│  Headless Tool Dispatcher    │   │   CPU Rasterizer (hayro)     │
│  (comment, form, edit, page) │   │   Text layer, font fallback  │
└──────────────┬───────────────┘   └─────────────┬────────────────┘
               ▼                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│           pdfcraft-engine & Core Workspace Crates (33 crates)   │
│  cos, crypt, annot, forms, sign, edit, organize, ocr, filters   │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Native Interface Boundary (UniFFI + cdylib)

Communication between JVM and native Rust code occurs through **UniFFI 0.32.2** generating type-safe Kotlin bindings.

### Core Interface: `MobileSession`
The Rust engine wraps a document in an atomic session:
- **Thread Safety**: All state-modifying operations hold an internal `std::sync::Mutex`.
- **Concurrent Rasterization**: Page rendering releases the session lock once page bytes and objects are verified, enabling multiple worker threads to draw tiles in parallel without blocking the UI or tool queue.
- **Panic Boundary**: Rust panics are trapped using `std::panic::catch_unwind` and converted to `MobileError::InternalException`, preventing JVM process termination.

### Exported Bridge Methods
```text
// Lifecycle
MobileSession::new() -> MobileSession
session.open(path: String, password: Option<String>) -> Result<DocumentInfo, MobileError>
session.save_to(doc: u64, path: String) -> Result<(), MobileError>
session.save_copy_to(doc: u64, path: String) -> Result<(), MobileError>
session.close(doc: u64)

// Document Metadata
session.document_info(doc: u64) -> Result<DocumentInfo, MobileError>
session.page_info(doc: u64, page: u32) -> Result<PageInfo, MobileError>
session.outline(doc: u64) -> Result<Vec<OutlineEntry>, MobileError>

// Rendering & Text
session.render_page(doc: u64, request: RenderRequest) -> Result<RenderedImage, MobileError>
session.search(doc: u64, query: String) -> Result<Vec<SearchHit>, MobileError>
session.text_rects(doc: u64, range: TextRange) -> Result<Vec<f32>, MobileError>
session.reading_blocks(doc: u64, page: u32) -> Result<Vec<ReadingBlock>, MobileError>

// Tool Execution (Atomic mutations)
session.tool(doc: u64, name: String, args_json: String) -> Result<String, MobileError>

// Global & Utility
file_thumbnail(path: String, max_side: u32) -> Result<RenderedImage, MobileError>
save_sound(doc: u64, page: u32, index: u32, path: String) -> Result<(), MobileError>
```
digital_id_info(path: String, password: String) -> Result<DigitalIdInfo, MobileError>
engine_version() -> String
init_logging()
```

---

## 3. High-Performance Mobile Rendering Pipeline

Mobile PDF viewing demands fluid 60/120 fps panning, pinching, and zooming across arbitrarily complex vector pages while staying within strict memory constraints (typically 128 MB heap budget on entry devices).

### Multi-Scale Tiling Engine
1. **Fit-Screen Preview**: When a page enters the viewport, the entire page is rendered at "fit scale" (`scale <= 1.0`). This base bitmap stays resident as a low-frequency preview.
2. **Dynamic 512px Tiles**: As the user pinches or zooms in beyond fit scale:
   - The viewport is subdivided into 512x512 device-pixel tiles aligned to the zoom level.
   - Zoom levels are snapped in half-octave steps ($\sqrt{2}$ factors) to maximize tile reuse.
3. **Prioritized Work Queue**:
   - Tiles intersecting the active screen center receive highest priority.
   - Off-screen tiles are immediately discarded or canceled.
4. **Memory-Budgeted LRU Cache**:
   - `PageImages` maintains an LRU cache with an adaptive heap memory budget (default 64MB - 128MB).
   - Bitmaps are allocated directly as native `ARGB_8888` buffers. When memory pressure mounts, oldest non-visible tiles are evicted.

---

## 4. Headless Automation & Tool Dispatch

Every document mutation—adding a comment, drawing ink, rotating a page, filling a form field, inserting an image, or applying digital signatures—is executed via `pdfcraft_automation::Automation`.

- **Atomic Transactions**: Each tool execution is transactional. If argument validation fails, the document remains in its exact prior state.
- **Full Undo / Redo**: The engine maintains a COS revision journal. `MobileSession::undo()` and `MobileSession::redo()` revert and re-apply changes without full document re-serialization.
- **Incremental Serializer**: Saves are performed incrementally wherever possible, ensuring digital signatures over previous byte ranges remain cryptographically valid.

---

## 5. Storage Access Framework (SAF) & Security

Android 11+ enforces scoped storage. The application interacts with external files using Android's `Intent.ACTION_OPEN_DOCUMENT` and `Intent.ACTION_CREATE_DOCUMENT`:

1. **Working Copies**: Files opened via `content://` URIs are copied into the app's secure private directory (`context.filesDir/Documents/<sha>/<name>`).
2. **In-Place Mutations**: The Rust engine operates directly on the local sandbox working copy with fast seekable POSIX I/O.
3. **Write-Back**:
   - When saving back to a writable `content://` URI, the application opens an output stream with mode `"wt"` (write/truncate) and copies the working file into the provider.
   - If an I/O error or process interruption occurs during write-back, the remote file may be left truncated. The local working copy in `context.filesDir/Documents/` is preserved, allowing the user to save a copy or recover their changes via the read-only / copy prompt.
4. **Offline Isolation**: The Android app requests **zero network permissions** in `AndroidManifest.xml` (`android.permission.INTERNET` is omitted). No user document or telemetry ever leaves the device.

---

## 6. Design System & User Experience

- **Theme**: Material 3 with dynamic color support (`DynamicLightColorScheme` / `DynamicDarkColorScheme` on Android 12+), gracefully falling back to customized brand palettes.
- **Edge-to-Edge**: Full screen immersion using Android 15 `enableEdgeToEdge()` API, drawing under status and navigation bars with WindowInsets padding.
- **Typography**: Inter (Open Font License) bundled in assets, providing crisp typographic hierarchy across all densities.
- **Icons**: Lucide icons compiled at build time into pure Compose `ImageVector` objects, ensuring zero vector-drawable inflation overhead.
