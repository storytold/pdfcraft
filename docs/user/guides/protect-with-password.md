---
title: Protect a PDF with a password
description: Add a password to a PDF in PdfCraft, open a protected PDF, and lift or remove restrictions.
order: 50
verified: 0.7.1
---

Choose **File**, **Protect using password…**, pick what the password is for, type it twice and click **Apply**. The password is added when you save.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl.

## Add a password

1. Open the PDF.
2. Click the **Menu** button, then **File**, then **Protect using password…**. The **Protect Using Password** dialog opens.
3. Under "Requires user to enter a password for:", choose one:
   - **Viewing**: anyone opening the document needs the password.
   - **Editing**: the document opens without a password, and the password is needed to change the restrictions.
4. Type the password in **Type Password** and again in **Re-type Password**. A strength note shows Weak, Medium or Strong.
5. Optional: open **Advanced Options** to pick the encryption level (**Compatibility**), what is encrypted (**Encrypt**), whether **Printing allowed** and **Changes allowed**, and whether copying text and screen-reader access are enabled.
6. Click **Apply**. PdfCraft says "Password protection will be applied when you save".
7. Choose **File**, **Save** (`Ctrl+S`).

**Apply** stays off until the password is filled in and both entries match. If you pick an older **Compatibility** level than "Acrobat X and later (256-bit AES)", the password may use plain ASCII characters only.

## Open a protected PDF

1. Open the file. PdfCraft shows "Password required" and says the file is protected.
2. Type the password and click **Open**.

PdfCraft opens every encryption level from 40-bit RC4 to AES-256. Edits to an encrypted document are saved encrypted, under the same keys.

## Lift restrictions

A document restricted by its author shows "This document is secured. Some changes are restricted by its security settings." PdfCraft honors the restrictions, and some page and editing commands stay off.

To lift them while the document is open:

1. Click **Unlock…** in that notice. **File**, **Security properties…** has the same button, and `Ctrl+K` finds it as **Enter permissions password…**.
2. Type the permissions password and click **Unlock**.

Opening the document with its permissions password also lifts them. Version 0.6.0 and earlier have no **Unlock…** button.

## Remove the password

Choose **File**, **Remove security**. If the document is restricted, PdfCraft first asks for its permissions password. PdfCraft says "Security will be removed when you save", so save the file afterwards. **File**, **Security properties…** shows the encryption method and each permission.
