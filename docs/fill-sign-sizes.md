# Saved Fill & Sign appearance sizes

To reuse a particular signature size, place a saved Fill & Sign signature, select it and resize it with its handles.
Right-click the selected appearance and choose **Use this size for saved signature**.
The Comments panel offers the same action.
Choose **Use this size for saved initials** to update the initials preference instead.
The action names the saved target explicitly; the selected appearance can supply a size for either target.

Future placements use the chosen width and height, including after restarting PdfCraft.
Existing placed appearances remain unchanged, and setting the preference does not change the PDF or its undo history.
The signature and initials preferences are independent.
Accepting an unchanged saved graphic preserves its size preference.
Accepting a changed saved graphic or removing it clears its size preference.
Cancelling the dialog or an unsuccessful image import leaves the preference intact.

Sizes describe the full displayed annotation rectangle in physical PDF points, with 72 points per inch.
They are independent of zoom, display density and temporary view rotation.
Both dimensions are retained, so copying a size from another appearance can intentionally change its proportions.
The source can be a suitable typed, drawn or imported image appearance, including one reopened from disk.
The saved target must exist and accept the chosen size when the action is invoked.
For drawn targets, the stroke width and padding retain their physical size across pages with different scales.
The box must leave enough room for the strokes and their padding; an incompatible size action is disabled with a reason.
If an older stored preference cannot fit the drawing or its position cannot be represented on the target page, placement reports the problem and preserves the preference and PDF.
Choose a larger placed appearance as the size source, or change the saved graphic to clear its previous size preference.

Without a chosen preference, the existing signature placement sizes continue to apply.
Saved appearance sizes affect Fill & Sign placements; visible cryptographic-signature graphics retain their existing behavior.

## Automation

The opt-in UI control channel exposes `sign.fill.signature.size` and `sign.fill.initials.size` through `ui.command`.
These commands use the current selected appearance and the same validation as the context menu.
`ui.state` reports the optional `signature_size` and `initials_size` pairs.

The headless `fill_sign_add` tool accepts an optional `size: [width, height]` for a signature or initials placement.
It applies to that call and does not read or change the desktop profile.
Omitting `size` retains the existing placement behavior.
Each dimension must be finite and between 0.01 and 14,400 physical points.
Invalid or unsupported geometry is refused atomically, leaving no partially placed annotation.
