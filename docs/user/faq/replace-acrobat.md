---
title: Can PdfCraft replace Adobe Acrobat?
description: What PdfCraft does well today, what is missing compared with Adobe Acrobat, and what is planned.
order: 20
verified: 0.7.1
---

For viewing, organizing, combining, splitting, commenting, form filling and passwords, often yes. For converting files and some Pro workflows, not yet. PdfCraft is in alpha.

## Works today

The project lists these as good today:

- viewing and search
- organizing, combining and splitting pages
- most kinds of comment
- filling and authoring forms
- passwords, redaction and sanitizing
- basic digital signatures
- printing (if no printers are listed, see [No printers are listed](../troubleshooting/no-printers-listed.md))
- the Accessibility Checker

It opens files protected with every standard encryption level, from 40-bit RC4 to AES-256.

## Thin or missing

The project lists these as thin or missing:

- reliable editing of existing text, especially Chinese, Japanese, Korean and right-to-left text
- OCR beyond Latin script
- Office import and export; there is no Office-to-PDF, and Word export loses layout
- signature tokens and online long-term validation
- Preflight and PDF/X or PDF/UA
- printing from the web build
- physical printer fidelity
- full XFA forms

The project's own estimate, from October 2026, is that PdfCraft is about 55% of the way to replacing Acrobat for everyday work, such as viewing, commenting, filling and signing forms, and organizing pages. See the [roadmap](https://github.com/storytold/pdfcraft/blob/main/ROADMAP.md) for what comes next.

## Related

- [Install PdfCraft](../getting-started/install.md)
- [Guides](../guides/index.md)
