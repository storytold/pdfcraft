# pdfcraft-forms

Interactive forms (AcroForm, ISO 32000-2 §12.7). Layer L3; depends on `pdfcraft-cos` and
`pdfcraft-fonts`.

## API

```rust
let all: Vec<Field> = fields(&doc);                         // terminal fields with widgets, options, flags, /DA
set_value(&mut doc, "name", &FieldValue::Text("Ada".into()))?;
set_value(&mut doc, "agree", &FieldValue::Check(true))?;
set_value(&mut doc, "size", &FieldValue::Radio(Some("L".into())))?;
set_value(&mut doc, "toppings", &FieldValue::Choice(vec!["Ham".into()]))?;
reset(&mut doc, None)?;                                      // Clear form (back to /DV)

// Prepare a form
let name = add_field(&mut doc, 0, [72.0, 700.0, 216.0, 722.0], &NewField::Text { multiline: false }, None)?; // "Text1"
set_props(&mut doc, &name, &FieldProps { name: Some("email".into()), required: Some(true), ..Default::default() })?;
delete_field(&mut doc, "email")?;
```

## Behaviour

- Inheritable attributes (`/FT`, `/Ff`, `/V`, `/DV`, `/DA`, `/Q`, `/MaxLen`) are resolved down the
  field tree; the form's `/DA` and `/Q` are the last fallback.
- Text and choice fields get new appearance streams (`/Tx BMC … EMC`): auto-size (`0 Tf`), comb,
  multiline wrapping, password masking, quadding, `/MK` background and border, list-box selection.
  The `/DA` font is used when `/DR` has it as a simple font; otherwise Helvetica (WinAnsi).
- Check boxes and radio buttons switch `/V` and each widget's `/AS`. Widgets with no appearances
  get PdfCraft's own (a drawn check mark or dot; no symbol font needed).
- Values are validated (options, `MaxLen`, single vs multi-select, NoToggleToOff, read-only) and a
  failed call changes nothing.
- `/RV` (rich text) is removed when a value is set so it can't contradict `/V`.
- New fields follow Acrobat: names Text1, Check Box1, Group1 (radio groups), Dropdown1, List Box1,
  Button1, Date1, Signature1; `/Helv` is added to `/DR`; appearances are drawn at once. Radio
  buttons joining a group become kid widgets with their own export state. Renaming changes only
  the last part of a dotted name.

## Not yet

JavaScript actions (format, keystroke, validate, calculate; M6.4/M6.5), rich-text values,
`/MK /R` rotation, the Appearance (colours, fonts), Actions, Format, Validate and Calculate
property tabs, FDF/XFDF import and export (M6.7).
