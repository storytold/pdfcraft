# User docs: rules for agents and contributors

Everything in this folder (except this file, CLAUDE.md and README.md) is published at
https://getartcraft.com/docs/pdfcraft. The file path is the URL:
`troubleshooting/no-printers-listed.md` → `/docs/pdfcraft/troubleshooting/no-printers-listed`.
Renaming a file breaks links, so add the old path to `redirect_from` when you rename.

## Who reads this

Artists using PdfCraft, Discord mods answering them, and AI assistants looking things up for
either. Write for an artist, not a developer.

## Page rules

- One question or problem per page. Lead with the answer or the fix in the first two lines.
- Plain words, short sentences, numbered steps for anything the reader does.
- Start the body at `##`. The page title comes from frontmatter.
- Link to other pages instead of repeating them. Use relative links to the `.md` file:
  `[Install PdfCraft](../getting-started/install.md)`.
- No marketing copy, no "simply", no claims you have not checked in the code, a release or an issue.
- Call the product "PdfCraft", as the app, its downloads and getartcraft.com do. Do not call it PrintCraft, its old name.
- Name UI labels exactly as the app shows them, in **bold**. Check them in `crates/engine/src/commands.rs` and the `tl!(...)` strings in `crates/ui-egui/src/`.
- Windows 11 is the only supported Windows version.
- Troubleshooting pages: `## Symptoms`, `## Fix`, `## Why this happens` (optional),
  `## Still stuck?` (link the issue tracker or Discord).
- Say when a fix is merged but not yet in a release.

## Frontmatter

    ---
    title: Combine PDF files
    description: One sentence for search results, link previews and AI summaries.
    order: 10                 # optional, sorts within the folder (lower first)
    verified: 0.6.0           # optional, app version you last checked this page against
    redirect_from: [old/path] # optional, old URLs (relative to /docs/pdfcraft)
    ---

Only `key: value` and `key: [a, b]` lines work. Quote values that start with a quote or contain ` #`.

Set `verified` to the latest release (the newest release tag) after checking the page against that tag with `git show <tag>:<path>`. Pages describe the latest release users download, not unreleased code on main.

## Folders

- `getting-started/` install and first run
- `guides/` how to do one thing
- `troubleshooting/` symptom → fix
- `faq/` short answers
- `reference/` lists and settings

A folder's `index.md` names the folder in the sidebar. Files or folders starting with `_` are
drafts and are not published.

## Check before you push

    node <path to your artcraft-services checkout>/frontend/apps/artcraft-website-next/scripts/docs/core.mjs docs/user

The checker lives in storytold/artcraft-services. A CI check will be added once it is on that repo's main branch.

## Support PRs

Pages written to answer a user question come in as PRs titled `docs(support): <page title>`.
Review them like code: check every claim against the source. Issues for PdfCraft go to
https://github.com/storytold/pdfcraft/issues.
