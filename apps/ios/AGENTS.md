# Desktop-first iPhone and iPad workflow

These rules supplement the repository-root AGENTS.md. The Swift shell remains the
isolated experiment described in README.md, proposed upstream but not an accepted
upstream language-policy exception.

## Before changing a mobile workflow

1. Read the corresponding desktop implementation first:
   - `../../crates/ui-egui/src/home.rs`: Home, recommendations and pinned tools.
   - `../../crates/ui-egui/src/panels/tools.rs` and `all_tools_ui.rs`: tool discovery.
   - `../../crates/engine/src/catalog.rs` and `commands.rs`: names, command IDs,
     capability/permission requirements and actions.
   - `../../crates/ui-egui/src/theme.rs`: neutral chrome, blue accent, light/dark
     behavior and white PDF pages.
2. Reuse an existing engine/automation operation through DocumentBridge. New reusable
   document behavior belongs in the existing Rust engine/automation layer first,
   then crosses the generated bridge. Do not copy desktop PDF rules into Swift or
   introduce a second PDF implementation. Keep UI toolkit dependencies out of the engine.
3. Keep the desktop's task order, terminology and safety behavior where supported.
   A tool may ask for a PDF first, then resume its intended workflow. Choosing
   Organize pages does not rotate or otherwise change the document automatically.
4. Adapt presentation rather than rebuilding the desktop toolkit: use native
   SwiftUI layout, sheets, Files, sharing and accessibility. An iPad sidebar becomes
   a tools sheet on compact layouts; primary opening and document actions must
   remain reachable on iPhone, split views and larger text sizes.

## One small mobile presentation layer

- `PdfCraftMobilePOC/WorkspaceView.swift` owns the supported mobile tool descriptors.
  Home cards, pins, sidebar and command search all consume that list. Keep stable
  desktop command IDs when semantics match; use `mobile.*` IDs for mobile-only
  adaptations, not misleading aliases for different desktop operations.
- `ContentView` dispatches those intents to the existing document workflow.
  `PageView` and `PageTextView` remain the common page and text presentations.
- The native palette mirrors only the desktop tokens it needs. Keep it aligned
  when changing desktop tokens; it is not generated or automatically synchronized.
  Follow system appearance by default and retain native light/dark accessibility.
- Reuse the licensed macOS lion artwork and compatible package metadata, following
  README.md's shared identity section and `../../packaging/macos/Info.plist.in`.
  Common metadata is generated from that template; do not hand-copy its values into
  Swift or the iOS configuration. Keep only platform-specific overrides local.
  Regenerate the mobile icon with `../../packaging/ios-icon.swift` when the macOS
  artwork changes; update its attribution hash. Generate the project through
  `../../scripts/generate-ios-project.sh` so workspace version/build metadata stays
  aligned. Keep the unregistered POC bundle ID defined once in project.yml and read
  the compiled identifier in launch commands; do not copy desktop-only
  handlers, translation claims or production signing identities.
- Show only working mobile capabilities. Do not add inert Edit/Convert/E-Sign tabs,
  copied community branding, placeholder tools or claims of desktop feature parity.
- Prefer Files' existing recent/provider/folder UI over a second bookmark or file
  history store. Preserve security-scoped access, private Rust snapshots, save-copy
  semantics, permission checks and unsaved-change confirmation.
- Do not migrate to a proposed shared mobile host, add a new library or generalize
  across repositories merely for visual consistency. That requires an accepted
  upstream direction and a separately validated integration.

## Validation and documentation

Run `bash scripts/test-ios.sh <uuid>` on an iPad and an iPhone, sequentially.
Extend existing native workflow tests for changed behavior; retain real Files,
rotate/export/reopen/share and text-selection coverage. Inspect the actual UI for
layout and theme changes, not only accessibility labels. Update README.md with
the desktop sources reused, intentional native differences and exact test results.
Never describe a simulator run as physical-device or full accessibility validation.
