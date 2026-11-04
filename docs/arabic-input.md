# Arabic page editing and input

Status: candidate implementation, 2026-11-02. Eight distinct native shell checks
pass. A user confirmed real Windows Arabic typing, with joined multiline text
observed in the desktop editor. Browser and provider-specific IME acceptance remain pending.

PdfCraft passes logical Unicode to egui 0.37. Its native editor owns bidi caret
mapping, selection and shaping. This contribution protects that editor's input
lifecycle; it does not add a second visual-order buffer or a new shaping engine.
It builds on the Auto/LTR/RTL controls in #5693. Related work in #1572 and #1851
remains separate; their source-font and custom-editor changes are not imported here.

## Editing behavior

- Left/Right and Shift-selection follow the native visual caret mapping. Copy and
  replacement keep logical text, including mixed Arabic, Latin and numbers.
- Backspace can correct a haraka without deleting its base letter. Delete removes
  the following grapheme. Enter and deletion across a newline retain typing order.
- While an input method has preedit text, Enter, Escape and Tab belong to that
  composition. They do not apply, discard or leave the PDF editor. Ordinary editor
  commands resume after the composition's final frame.
- Save/finish actions refuse an active composition, retaining the draft. Registry
  commands from page editors wait for the current frame's input, as form commands
  already do, so a same-frame commit is not omitted.

The composition guard stores only a focused-widget identifier and a boolean. It
does not clone preedit text, reorder characters or alter native caret geometry.
It resets when another widget takes focus. Native and web hosts use the same guard.
When native caret clicks or drags promote a preedit to ordinary editor text, the
guard follows that native transition so commands do not remain blocked afterward.

## Driving composition headlessly

The existing opt-in, token-authenticated UI control channel accepts `ui.ime`:

```sh
pdfcraft-cli ui --control control.json ime phase=preedit text=مرح selection='[0,3]'
pdfcraft-cli ui --control control.json ime phase=commit text=مرحبا
pdfcraft-cli ui --control control.json ime phase=cancel
```

`selection` is optional for preedit and counts Unicode scalar values, not bytes.
It must be ordered and inside the preedit. Commit accepts text only; cancel accepts
no text or selection. Text is capped at 262144 UTF-8 bytes before copying it into
the input queue. Invalid arguments leave the editor unchanged. Replies do not echo
the text. No transport, authentication, root-confinement or network default changes.

These calls exercise application delivery. They do not establish that a particular
operating-system IME provider or keyboard was tested.

## Qualification boundaries

The selected live-check target is Windows Arabic keyboard input and Chrome.
`crates/ui-egui/tests/arabic_input.rs` contains the bounded native shell journeys;
composition events in that harness are synthetic. Browser and native-window
results must be recorded separately after they are observed.

The native target covers both existing-paragraph and added-text drafts: visual
arrows and Shift-selection with logical copy; haraka Backspace and grapheme Delete;
joining/splitting Arabic and Latin lines; pointer insertion inside a joined word;
composition cancellation and Enter/Tab ownership; refusal to save a candidate;
and malformed input preserving text and focus. Added text also survives document
undo/redo and save/reopen. Five cases passed in the first bounded run; the two
composition-key cases passed after their harness explicitly invoked the real
`raw_input_hook`, which kittest's eframe adapter otherwise omits. The focused
rerun also included the malformed-input case.

One additional pointer-interruption regression passed after reproducing the stuck
guard before its fix. It checks the native committed-text snapshot before and after
a caret click, then requires ordinary Enter and Escape to work in both editors.

The cancellation regression fails with the two application input-guard hooks
removed: the source draft closes on the composition's Escape. Restoring the hooks
keeps it open; a subsequent ordinary Escape still closes it. Native screenshots
were inspected for joined text and the insertion caret. These checks establish
synthetic shell behavior, not a particular OS keyboard or IME provider.

Source-paragraph typing tests do not qualify every font or PDF producer. Existing
Arabic source-font saving remains dependent on the supported-font engine work in
#5623. Added-text history and save/reopen are checked separately. Full Arabic form
appearances, OCR, export interoperability and broad mixed-style source editing are
outside this input contribution. Readiness and parity statuses remain unchanged
until the corresponding workflows have evidence. The Windows computer-use helper
was unavailable during this run; the user instead typed directly with the Windows
Arabic keyboard. This establishes that observed typing example, not every keyboard
workflow or an OS composition provider. The initial standalone input build lacked
#5623 and refused Apply when it selected a variable fallback before the document's
static font. Combined local acceptance must include that existing source-font fix.

## Revision history

- 2026-11-02: document the input lifecycle candidate, headless composition API and
  separate native/browser/provider qualification boundaries.
