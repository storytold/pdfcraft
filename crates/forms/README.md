# pdfcraft-forms

Interactive forms (AcroForm, ISO 32000-2 §12.7). Layer L3; depends on `pdfcraft-cos` and
`pdfcraft-fonts`.

## API

```rust
let all: Vec<Field> = fields(&doc);                         // terminal fields with widgets, options, flags, /DA
let snapshot = fields_with_adopted(&doc);                     // fields and page-only adoption count in one pass
let all = snapshot.items;
let adopted = snapshot.adopted_page_fields;
set_value(&mut doc, "name", &FieldValue::Text("Ada".into()))?;
set_value(&mut doc, "agree", &FieldValue::Check(true))?;
set_value(&mut doc, "size", &FieldValue::Radio(Some("L".into())))?;
set_value(&mut doc, "toppings", &FieldValue::Choice(vec!["Ham".into()]))?;
reset(&mut doc, None)?;                                      // Clear form (back to /DV)

// Prepare a form
let name = add_field(&mut doc, 0, [72.0, 700.0, 216.0, 722.0], &NewField::Text { multiline: false }, None)?; // "Text1"
set_props(&mut doc, &name, &FieldProps { name: Some("email".into()), required: Some(true), ..Default::default() })?;
// A partial appearance edit keeps each widget's other styles and custom /DA font.
set_props(&mut doc, "email", &FieldProps {
    appearance: Some(LookPatch { width: Some(3.0), fill: Some(None), ..Default::default() }),
    ..Default::default()
})?;
delete_field(&mut doc, "email")?;
```

## Behaviour

- Inheritable attributes (`/FT`, `/Ff`, `/V`, `/DV`, `/DA`, `/Q`, `/MaxLen`) are resolved down the
  field tree; the form's `/DA` and `/Q` are the last fallback.
- Text and choice fields get new appearance streams (`/Tx BMC … EMC`): auto-size (`0 Tf`), comb,
  multiline wrapping, password masking, quadding, `/MK` background and border, list-box selection.
  The `/DA` font is used when `/DR` has it as a simple font or a composite font with a predefined
  Unicode CMap; otherwise Helvetica (WinAnsi). A value or button caption the font can't hold
  (Chinese in a Helvetica field, a character beyond U+FFFF in a UCS-2 CMap) is kept, as Acrobat
  keeps it: `/V` is stored as typed, the appearance draws `?` for those characters, the widget is
  marked with the first one (`pdfcraft_fonts::UNDRAWN_MARK`, cleared once all of it can be drawn)
  and the form sets `/NeedAppearances`, so other viewers draw the field from its value with fonts
  that have them. Not in a document that declares PDF/A, which forbids that flag (`declares_pdfa`).
  `undrawn_fields` lists the marked fields; the engine reports each edit that draws one, and
  flattening refuses to make that `?` permanent.
- Check boxes and radio buttons switch `/V` and each widget's `/AS`. Widgets with no appearances
  get PdfCraft's own (a drawn check mark or dot; no symbol font needed).
- Values are validated (options, `MaxLen`, single vs multi-select, NoToggleToOff, read-only) and a
  failed call changes nothing.
- `FieldProps::appearance` applies a `LookPatch` independently to each widget. `None` keeps
  a property; `Some(None)` removes a border/fill colour. Unchanged `/DA`, custom font resources,
  widget-specific `/DA` overrides, indirect `/MK` and `/BS` contents, dash arrays and unknown
  dictionary keys are preserved. It is mutually exclusive with the complete `look` replacement.
  Use the engine's `Edit::Batch` to change several fields atomically in one undo step.
- `/RV` (rich text) is removed when a value is set so it can't contradict `/V`.
- New fields follow Acrobat: names Text1, Check Box1, Group1 (radio groups), Dropdown1, List Box1,
  Button1, Date1, Signature1; `/Helv` is added to `/DR`; appearances are drawn at once. Radio
  buttons joining a group become kid widgets with their own export state. Renaming changes only
  the last part of a dotted name.
- `Widget::caption` reads a push button's normal label (`/MK /CA`, including indirect values).
  `FieldProps { caption: Some((widget_index, text)), ..Default::default() }` changes or clears
  that label, independently of the field name. The index is 0-based. A label-only edit redraws
  just that widget, copying shared/indirect `/MK` and appearance dictionaries before editing.
  Other widgets, actions, icons, rotation and unknown entries are preserved. Existing icon-only
  layouts keep hiding the caption; caption rendering retains the existing Helvetica/WinAnsi
  limits and reports unavailable characters as above.

## Not yet

Rich-text values and exact appearance metrics/layout remain incomplete. JavaScript
execution and FDF/XFDF exchange are integrated through the engine; this crate provides
field actions and native AF rules. `/MK /R` rotation (0, 90, 180, 270) is drawn into the
appearance.
