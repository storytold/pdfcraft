# Removing a signature

Use a certificate ▸ **Remove signature…**, or **Remove signature…** under a signature in the
Signatures panel, takes one signature out of a document and saves the result as a new file, so
the document can be edited and signed again (Acrobat calls this Clear Signature). It is meant
for documents you own or are authorised to change. It never makes a document look signed, or a
signature look valid, when it isn't.

Headless: `sign_removable` lists what can be removed and why not, and `sign_clear` removes one
(then `doc_save`, preferably to a new `path`).

## What can be removed

| Kind | What it is | What goes |
|---|---|---|
| Digital signature | the value (`/V`) of a signature field: `/ByteRange`, `/Contents`, the signature dictionary | the field and its widgets, or only the value and the appearance (**Keep an empty signature field to sign again**) |
| Certification signature | a digital signature with DocMDP permissions (level 1–3) | as above, and the catalog's `/Perms /DocMDP` when it names this signature: the document is no longer certified |
| Visual signature or initials | a Fill & Sign mark PdfCraft recognises with certainty: a typed, drawn or image signature or initials annotation | that annotation; it has no cryptography behind it |

The list shows each one's kind, signer, field, page, signing time and the verdict validation
gave (valid, validity unknown, invalid). That verdict is PdfCraft's validation, shown as it is;
that a signature exists says nothing about whether it is valid.

Never listed, never removed:

- **A signature drawn into a page's content**, such as a scanned page or a picture placed in the
  content stream. It is not a field or an annotation, so it can't be told apart from the rest of
  the page with certainty, and PdfCraft doesn't erase regions, cover them with white boxes or
  rasterise pages to hide it. To take it out, redact that area (Redact) or, when the signature is
  an image of its own, delete it with Edit PDF. The dialog says so when there is nothing to list.
- **Other text or pictures that look like a signature.** Only Fill & Sign marks are recognised.

## When it is refused

The removal is refused, and nothing changes, when:

- **Another signature certifies the document.** No DocMDP level permits removing a signature, so
  removing an approval signature from a certified document would break the certification.
  Removing the certification signature itself is allowed: the document is then no longer
  certified, and the dialog says so.
- **Another signature locks the field** (FieldMDP `/Lock`, actions All, Include or Exclude).
- **It is a document timestamp** (`/ETSI.RFC3161`, in a field or not): it covers the whole file
  and is not a signer's signature to clear.
- **The document can't be changed** (read-only, or its security doesn't allow changes).

PdfCraft never works around a certification, a lock or a document's security.

## What the removal changes

For a digital signature, in the working copy (undoable):

1. the field leaves `/AcroForm /Fields` (or its parent's `/Kids`) and its widgets leave their
   pages' `/Annots`; parent fields left without kids go too. Other fields, their values and
   their widgets stay. With **Keep an empty signature field**, only `/V` and the widgets'
   `/AP` and `/AS` go; the field's `/Lock` and seed values (`/SV`) stay.
2. the catalog's `/Perms /DocMDP` goes when it names this signature (`/UR3` stays).
3. `/SigFlags` loses AppendOnly once nothing is signed, and SignaturesExist once there is no
   signature field.
4. the pages' annotations, the field tree and the certification entry are checked for references
   to missing objects before and after; if the removal added one, it is refused. Damage the file
   already had is not blamed on it.

Page content, resources, fonts, images, other annotations, bookmarks, metadata, attachments,
layers and links are not touched.

## Other signatures

Every other signature in the document will report that the document changed after it was signed.
The dialog names them, and names those signed **after** the removed one: what they signed
includes it. PdfCraft doesn't say they stay valid; check them again afterwards.

## Saving

Nothing is written until you choose where the new version goes:

1. **Remove and Save As…** asks for a file; the name offered is `<name>_unsigned.pdf`. The
   original file can't be chosen (the dialog asks for another one), and the system's save dialog
   asks before replacing an existing file, as in any Save As. In the browser the new version is
   downloaded instead.
2. The signature is removed, and PdfCraft checks that it is gone.
3. The file is written as Save As writes it: the bytes are checked to reopen with every page,
   written to a temporary file next to the destination and moved into place, so a failure never
   leaves a half-written file.
4. If any step fails, the removal is undone and the document is as it was. The message says
   what was removed only after the file was written and checked.

Afterwards the document is the new file: edit it and sign it again as usual. Unsaved changes made
before the removal are saved in the new file too.

### Is the removed signature still in the file?

- **No signature left:** the new file is a full rewrite with no earlier revisions. The removed
  signature is not in it.
- **Other signatures left:** a full rewrite would break them, so the new file is the signed file
  plus an incremental update. Its first bytes are the earlier revision, byte for byte, and the
  removed signature can be recovered from there (cutting the file at that revision gives back the
  signed document). The dialog says so before you continue.

The original file is never changed, and keeps its signatures.

## Not yet

- Acrobat's Clear Signature for a signature not yet saved (PdfCraft signing always saves).
- Clear All Signature Fields, and removing from a right-click on the signature on the page.
- Removing a document timestamp, or the security store (`/DSS`) entries of a removed signature
  (they stay, unused).
- Structure-tree references to a removed widget stay (the widget object is kept for them).
