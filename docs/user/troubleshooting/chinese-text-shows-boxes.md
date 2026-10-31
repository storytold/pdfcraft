---
title: Chinese text in the interface shows as boxes
description: Why the Chinese interface of PdfCraft can show empty boxes for some characters, and how to fix it.
order: 30
verified: 0.7.1
---

In PdfCraft 0.6.0 and earlier, some Chinese characters in the menus and dialogs, mostly Simplified-only ones, appear as empty boxes. Since 0.7.0, PdfCraft draws them with a Chinese font already installed on your computer. Update to the latest release. If you still see boxes, switch the interface language to English or another language.

Shortcuts on this page use Ctrl. On macOS, use Cmd instead of Ctrl.

## Symptoms

- With the interface in Simplified Chinese, characters such as 欢, 签 and 导 show as boxes. Only characters shared with the Japanese fonts draw correctly ([issue #984](https://github.com/storytold/pdfcraft/issues/984)).
- It affects the installer and the portable zip alike (issue #984, Windows with a Simplified Chinese locale).

## Fix

1. Check your version in **Help**, **About PdfCraft**. If it is 0.6.0 or earlier, install the latest release. See [Install PdfCraft](../getting-started/install.md).
2. If boxes remain, check that your computer has one of the Chinese fonts PdfCraft looks for:
   - Windows: Microsoft YaHei or Microsoft JhengHei (SimSun and MingLiU also work).
   - macOS: PingFang, Hiragino Sans GB or STHeiti.
   - Linux: WenQuanYi Micro Hei or Zen Hei, Droid Sans Fallback, or AR PL UMing.

If none helps, change the interface language:

1. Click the **Menu** button, then **Edit**, then **Preferences…** (`Ctrl+,`).
2. Set **Interface language** to a language you can read. **Auto** follows your system language.

## Why this happens

PdfCraft's release builds embed Japanese fonts from craft-fonts but no Chinese interface font. Changing that is on the project's list ([ROADMAP](https://github.com/storytold/pdfcraft/blob/main/ROADMAP.md), issue #826).

Since 0.7.0 ([#925](https://github.com/storytold/pdfcraft/pull/925)), when the interface is in Simplified or Traditional Chinese, PdfCraft reads one Chinese font that is already installed on your computer and uses it before the Japanese fonts. It does not copy or ship that font. The web build does not do this.

## Still stuck?

Comment on [issue #984](https://github.com/storytold/pdfcraft/issues/984) or ask in [Discord](https://discord.gg/artcraft). Say your Windows or macOS version and the PdfCraft version from **Help**, **About PdfCraft**.
