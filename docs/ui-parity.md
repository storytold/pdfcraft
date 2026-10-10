# UI and interaction parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first UI/UX checklist) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

How PdfCraft's tools, panels, shortcuts and feel compare with Acrobat Pro's current interface (the
2023+ "new Acrobat": All tools pane on the left, quick-action toolbar, Home view). Behaviour is
observed clean-room (`plan/acrobat/02-ui-ux.md`, local-only; `AGENTS.md` §2); nothing from Adobe's
interface is copied. Gaps ranked in [gaps.md](gaps.md).

**UI/UX fidelity ≈ 55% ready** (estimated), **30–60 h** additional to the features total.

## Measured

- **184 registered commands** (`crates/engine/src/commands.rs`), every one in the menus, the
  command palette and the automation `command_run` tool; menus are generated from the registry
  (`view.menus-from-registry`).
- **141 automation tools** (`crates/automation/src/tools.rs`), so every UI action can be checked
  headlessly.
- **View area of the checklist: 62.5% breadth** (52 shipped, 11 partial of 92).

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
| Read mode, full screen | yes | yes | shipped; presentation options planned |
| Light, dark, system theme | yes | yes | shipped |
| Side panels: thumbnails, bookmarks, attachments, layers, comments, fields, signatures | yes | yes; panels keep their width (#704) | shipped |
| Destinations, articles, model tree, Tags, Order, Content panels | yes | no | planned |
| Message bars (restricted, signed, PDF/A, forms) | yes | restricted and signed; no PDF/A view mode | partial |

## Tools, handles and precision

| Behaviour | Acrobat | PdfCraft | Status / evidence |
|---|---|---|---|
| Zoom: wheel, pinch, keys, marquee, fit page/width/height/visible | yes | yes | shipped |
| Fixed zoom percentages match Acrobat's physical size | yes | differ (#739, documented in #736) | gap |
| Dynamic zoom, loupe, pan-and-zoom window | yes | no | planned |
| Hand tool, middle-button pan, autoscroll | yes | hand tool; Linux autoscroll | partial |
| Text selection, column select | yes, Alt-drag columns | reading-order selection; no column mode (#740) | partial |
| Select and move several objects at once | yes | no (#844) | gap |
| Object handles: corners keep proportion, edges free | yes | yes for added images and signatures | shipped |
| Arrange, align and distribute page objects | yes | fields only | partial |
| Rulers, grids, snap to grid, guides | yes | no | planned |
| Cursor coordinates, line weights | yes | no | planned |
| Measuring snap to paths, endpoints, midpoints, intersections | yes | yes | shipped |
| Nudge selected objects with arrow keys (Shift for larger steps) | yes | no: arrow keys scroll and turn pages | gap |
| Single-key tool accelerators (H, V, Z, …) | yes, opt-in | no | planned |
| Context menus on pages, thumbnails, comments, fields | yes | thumbnails, comments, fields | partial |
| Comment pop-ups as movable windows, connector lines | yes | inline in the panel | planned |
| Disabled commands explain why | no | yes | ahead |

## Shortcuts and modifiers

| Item | Acrobat | PdfCraft | Status |
|---|---|---|---|
| Platform shortcuts for registered commands | yes | yes; ⌘ on macOS, Ctrl elsewhere (#695) | shipped; find bar still shows ⌘ on Windows (#746) |
| Keyboard shortcuts reference | yes | yes | shipped |
| Full keyboard operation of the app | yes | no | planned (`a11y.keyboard-only`, P0) |
| Shift-⌘-G finds previous | yes | goes forward (#810) | bug |

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
| 2026-10-10 | major | Created from the command registry, automation tools, the View area of the checklist and open UI issues |
