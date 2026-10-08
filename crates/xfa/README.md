# pdfcraft-xfa

XFA forms (XFA 3.3, the template and layout parts). Layer L3; depends on `pdfcraft-cos` and
`pdfcraft-fonts` only.

A dynamic XFA form is a PDF shell around an XML template (`/AcroForm /XFA`): one placeholder page
("requires Adobe Reader") and `/NeedsRendering true`. Viewers without an XFA engine show the
placeholder. This crate reads the template, lays it out, and writes ordinary pages and AcroForm
fields into the document, so everything else in PdfCraft (rendering, filling, saving, printing,
the automation tools) works on it unchanged. The engine does this when it opens such a form.

## What it does

- **Packets** (`read_packets`): the XDP from one stream or the `(name, stream)` array; UTF-8
  and UTF-16; capped at 64 MB.
- **Template** (`parse`): subforms, areas, fields, draws, exclusion groups, page sets with
  page areas, content areas and media; measurements in mm, cm, in, pt and px; fonts, paragraphs,
  margins, borders (edge order rules), captions with reserves, check-button items, pictures,
  rich text (`exData` XHTML: paragraphs, `br`, bold/italic/underline/size spans, embedded fields),
  inline JPEG images, lines, rectangles, occurrences, `breakBefore`/`breakAfter`, overflow
  leaders, `columnWidths` and `colSpan`.
- **Layout** (`layout`): positioned, `tb`, `lr-tb`/`rl-tb`, `table` and `row` layouts; a
  container without a width is as wide as its content; flow across content areas and pages with
  page masters chosen by occurrence; a table's header row repeats after a break; presence
  `hidden` takes no space, `invisible` takes space; repeating subforms get their initial
  instances; fields that compute the page number or count on layout show the numbers.
- **PDF** (`pdf::write_form`): pages with content streams in the standard 14 fonts (Arial and
  friends → Helvetica, Courier New → Courier, serif faces → Times; nothing is embedded), JPEG
  XObjects, and widgets: text (multi-line, max length, alignment), date (Acrobat's `AFDate`
  format and keystroke actions from the picture clause), check boxes and radio groups (from
  `exclGroup`), push buttons with Reset / Print / Save As / URI actions recognised from their
  scripts. Appearance streams are left empty for the forms layer to generate. Each field keeps
  its SOM path in `/PCSom`; the AcroForm gets `/PCXfaLayout` so a saved form is not laid out
  twice. The XFA packets stay, so Adobe's viewers keep rendering the form from them.

- **Data** (`data`): the `datasets` packet. On layout, a field takes its value from the data
  node at its SOM path (dates in ISO form, check and radio states by their on values), and a
  repeating subform or row gets as many instances as the data has. `write_datasets` merges the
  AcroForm fields' values (by `/PCSom`, or by the Designer field names of a static form) into
  the existing `xfa:data`: only the bound nodes' text changes and missing nodes are added, so
  unbound data, other namespaces, attributes and comments stay byte for byte. Values it can't
  write (a node holding structured content, absurd SOM indices, a packet that is neither UTF-8
  nor UTF-16) come back as warnings; UTF-16 packets are written back as UTF-16. `read_values`
  goes the other way, so a form filled here shows its values in Adobe's viewers and a form
  filled there shows them here. The engine does both: values on open (recorded in the
  document's warnings), the packet after every form edit.

Layout measures each container once per (node, width): width-less containers would otherwise
be measured exponentially often in their nesting. A measurement budget backs this up.

## API sketch

```rust
if pdfcraft_xfa::is_dynamic(&doc) {
    let report = pdfcraft_xfa::render_into(&mut doc)?;   // pages, fields, warnings
    for f in pdfcraft_forms::fields(&doc) { pdfcraft_forms::redraw_field(&mut doc, &f.name)?; }
}
let form = pdfcraft_xfa::layout_xml(template_xml)?;       // pages of items, for tests
```

## Deliberately not done (yet)

- **Data binding** is the default one only: explicit `bind ref` expressions, global binding and
  data descriptions are not followed, and no standalone XML or XDP data file is imported or
  exported.
- **Scripting.** No FormCalc or XFA JavaScript (`xfa.host`, `xfa.layout`, `instanceManager`);
  rows are not added by button, validations and calculations don't run. Buttons map only the
  common idioms (reset, print, save as, launchURL).
- **Static XFA forms** (`/NeedsRendering` absent, AcroForm fields present) keep their AcroForm;
  only their data is read and written.
- Choice lists are text fields; signature, image, barcode and password fields are left blank;
  PNG and GIF images, `keep` constraints, `subformSet` relations, `rl-tb` is mirrored `lr-tb`,
  font metrics are the approximate Helvetica ones, and line heights are 1.15 × size.
- Acrobat cannot be an oracle here (clean-room rules): layout follows the specification, and
  fidelity has been checked by eye on real government forms, not pixel-compared.
