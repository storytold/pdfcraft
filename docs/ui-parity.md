# UI and interaction parity

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-24 · **Change:** minor (vector paths in the Edit tool, align, distribute and arrange page objects from the selection's right-click menu, #1540; commands, tools and the View area recounted 2026-10-24; earlier: column select #740, multi-object move #844, Shift-⌘-G #810 fixed) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-21 · **Last updated:** 2026-10-21 · **Change:** minor (Pages inspector thumbnail size slider and buttons #3808; full screen presents one page at a time #1459; the selected field nudges with the arrow keys in Prepare a form #2608) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

How PdfCraft's tools, panels, shortcuts and feel compare with Acrobat Pro's current interface (the
2023+ "new Acrobat": All tools pane on the left, quick-action toolbar, Home view). Behaviour is
observed clean-room (`plan/acrobat/02-ui-ux.md`, local-only; `AGENTS.md` §2); nothing from Adobe's
interface is copied. Gaps ranked in [gaps.md](gaps.md).

**UI/UX fidelity ≈ 55% ready** (estimated), **30–60 h** additional to the features total.

## Measured

- **205 registered commands** (`crates/engine/src/commands.rs`), every one in the menus, the
  command palette and the automation `command_run` tool; menus are generated from the registry
  (`view.menus-from-registry`).
- **184 automation tools** (`crates/automation/src/tools.rs`), so every UI action can be checked
  headlessly.
- **View area of the checklist: 64.2% breadth** (54 shipped, 14 partial of 95). All three counted on
  2026-10-24.

No dump of Acrobat's menu tree has been diffed against the registry yet (gap 24).

## Shell and navigation

| Element | Acrobat | PdfCraft | Status |
|---|---|---|---|
| Home view (recent files, recommended tools) | yes | yes | shipped |
| All tools pane, tool sets | yes | yes, with milestone badges | shipped |
| Quick-action toolbar, customisable | yes | fixed | partial (`view.customize-quick-tools` planned) |
| Command search | "Search tools" | command palette (⌘K), matches translated and English labels | shipped |
| Document tabs | yes | yes, with unsaved marker | shipped |
| Multiple windows, drag a tab out | yes | no | planned |
| Split window, spreadsheet split | yes | no | planned |
| Read mode, full screen | yes | yes; full screen is one fitted page at a time on black, click/keys turn pages (#1459) | shipped; auto-advance, loop and transitions planned |
| Light, dark, system theme | yes | yes | shipped |
| Side panels: thumbnails, bookmarks, attachments, layers, comments, fields, signatures | yes | yes; panels keep their width (#704); the Pages inspector has a 50–150% thumbnail size slider and buttons (#3808) | shipped |
| Destinations, articles, model tree, Tags, Order, Content panels | yes | no | planned |
| Message bars (restricted, signed, PDF/A, forms) | yes | restricted and signed; no PDF/A view mode | partial |

## Tools, handles and precision

| Behaviour | Acrobat | PdfCraft | Status / evidence |
|---|---|---|---|
| Zoom: wheel, pinch, keys, marquee, fit page/width/height/visible | yes | yes | shipped |
| Fixed zoom percentages match Acrobat's physical size | yes | differ (#739, documented in #736) | gap |
| Dynamic zoom, loupe, pan-and-zoom window | yes | no | planned |
| Hand tool, middle-button pan, autoscroll | yes | hand tool; Linux autoscroll | partial |
| Text selection, column select | yes, Alt-drag columns | reading-order selection; Alt/Option-drag or Edit ▸ Column select (#740) | shipped |
| Select and move several objects at once | yes | on one page: Shift/Ctrl/⌘-click and marquee, one undo step (#844), vector paths included (#1540); not across pages, no group resize | partial |
| Object handles: corners keep proportion, edges free | yes | yes for added images and signatures; Shift on a corner keeps the proportions of comments, stamps and form fields (#2199); a selected vector path's corners resize it freely (#1540); no Alt resize from the centre | shipped |
| Arrange, align and distribute page objects | yes | fields; in Edit text & images, the selection's right-click menu aligns and distributes any mix of objects and arranges one vector path or image (#1540); text isn't arranged | shipped |
| Rulers, grids, snap to grid, guides | yes | no | planned |
| Cursor coordinates, line weights | yes | no | planned |
| Measuring snap to paths, endpoints, midpoints, intersections | yes | yes | shipped |
| Nudge selected objects with arrow keys (Shift for larger steps) | yes | yes in Edit text: a point, Shift ten points, for selected objects, images and an open paragraph's box (#1520); and in Prepare a form, for the selected field (#2608); a selected comment: a screen pixel, ⌘/Ctrl or Shift ten | shipped |
| Single-key tool accelerators (H, V, Z, …) | yes, opt-in | no | planned |
| Context menus on pages, thumbnails, comments, fields | yes | thumbnails, comments, fields, an Edit-tool selection (Arrange, Align, Distribute, Delete; #1540) | partial |
| Comment pop-ups as movable windows, connector lines | yes | inline in the panel | planned |
| Disabled commands explain why | no | yes | ahead |

## Shortcuts and modifiers

| Item | Acrobat | PdfCraft | Status |
|---|---|---|---|
| Platform shortcuts for registered commands | yes | yes; ⌘ on macOS, Ctrl elsewhere (#695) | shipped; find bar still shows ⌘ on Windows (#746) |
| Keyboard shortcuts reference | yes | yes | shipped |
| Full keyboard operation of the app | yes | no | planned (`a11y.keyboard-only`, P0) |
| Shift-⌘-G finds previous | yes | yes (#810 fixed) | shipped |

## Dialogs and preferences

| Item | Acrobat | PdfCraft | Status |
|---|---|---|---|
| Print dialog with preview | yes | yes | shipped |
| Document Properties (description, security, fonts, initial view, custom, advanced) | yes | all but Custom and XMP | partial |
| Field Properties (General, Appearance, Position, Options, Actions, Format, Validate, Calculate, Signed) | yes | all but Signed | partial |
| Comment properties and properties bar | yes | yes | shipped |
| Preferences | ≈ 20 pages | Interface language, Documents and view, Identity, Fill & Sign, JavaScript | partial (gap 19) |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-24 | minor | After merging `upstream/main` at d4454108: commands (205), automation tools (184) and the View area (64.2%, unchanged, `cargo xtask parity`) recounted |
| 2026-10-22 | minor | After merging `upstream/main` at 80b7d215: commands (204), automation tools (180) and the View area (64.2%, `cargo xtask parity`) recounted |
| 2026-10-21 | minor | Pages inspector thumbnail sizing (#3808), separate from page and Organize zoom; view setting retained on reload and exposed through UI control. Not persisted between sessions. |
| 2026-10-20 | minor | After merging `upstream/main` at 22bedbf2: commands (201), automation tools (176) and the View area (63.2%, `cargo xtask parity`) recounted |
| 2026-10-15 | minor | A selected field nudges with the arrow keys in Prepare a form: a point, Shift ten points, one undo step per press (#2608) |
| 2026-10-14 | minor | Full screen shows one fitted page (or spread) at a time on black with click and key paging, as Acrobat's Full Screen Mode (#379, #1459, #1353) |
| 2026-10-13 | minor | Vector paths join the Edit tool's selection with corner-handle resize (#1540); the selection's right-click menu aligns, distributes and arranges page objects (arrange: one vector path or image), so that row is shipped; object handles partial (vector path corners don't keep proportion). Commands (192), automation tools (168) and the View area (62.8%) recounted |
| 2026-10-11 | minor | Column select shipped (#740, PR #765); several objects move together on one page (#844, PR #907); Shift-⌘-G goes back (#810, PR #835) |
| 2026-10-10 | major | Created from the command registry, automation tools, the View area of the checklist and open UI issues |
