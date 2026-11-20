---
title: PdfCraft does not start
description: What to do when PdfCraft closes right after launch on Windows 11, with a graphics error in its log.
order: 10
verified: 0.7.1
---

On Windows 11, first check that you installed the build that matches your PC (`x64` for most) and that you have the latest release. If it still closes at launch, start PdfCraft with OpenGL: set `PDFCRAFT_RENDERER` to `gl`. The steps are below.

## Symptoms

- PdfCraft closes at launch or never shows a window.
- The log file `pdfcraft.log` contains lines like these:

      Shader translation error for stage ShaderStages(VERTEX | FRAGMENT): The parameter is incorrect. (0x80070057)
      In Device::create_render_pipeline, label = 'egui_pipeline'

This was reported for versions 0.4 to 0.6.0 on NVIDIA and Intel graphics ([issue #1046](https://github.com/storytold/pdfcraft/issues/1046)).

The log is in `%APPDATA%\PdfCraft\data\logs\pdfcraft.log`. With the portable zip it is in the `PdfCraftData` folder next to `pdfcraft.exe`.

## Fix

### 1. Use the right build

Two users in the issue had installed the `x86` build on a 64-bit PC by mistake. If that is you, uninstall it and install `pdfcraft-<version>-windows-x64.msi` instead. See [Install PdfCraft](../getting-started/install.md). One user in the issue hit the same error on the x64 portable build, so if you already have the right build, go to step 2.

### 2. Update to the latest release

Since 0.7.0, when Direct3D 12 fails like this at launch, PdfCraft starts again with OpenGL by itself. The log then says "the GPU renderer (wgpu) didn't start" and "Starting with OpenGL instead". Version 0.6.0 and earlier close instead. Check your version in **Help**, **About PdfCraft**, or look at the file name you downloaded.

### 3. Start with OpenGL

If the latest release still closes at launch, choose OpenGL yourself. A user in the issue got PdfCraft 0.6.0 running this way.

1. Close PdfCraft.
2. Open PowerShell.
3. Run these two commands. Use the path of your own `pdfcraft.exe`; the reporter's x64 guess was `C:\Program Files\PdfCraft\pdfcraft.exe`, but check yours.

       $env:PDFCRAFT_RENDERER = 'gl'
       & 'C:\Program Files\PdfCraft\pdfcraft.exe'

PdfCraft should open. The setting applies only to that PowerShell window.

To make it permanent for your Windows account, run this in PowerShell, then sign out of Windows and sign back in:

    [Environment]::SetEnvironmentVariable('PDFCRAFT_RENDERER', 'gl', 'User')

To undo it, run the same command with `$null` instead of `'gl'`, and sign out and in again.

## Why this happens

On Windows, PdfCraft draws with Direct3D 12 by default. On some PCs that fails to compile PdfCraft's shaders. In 0.6.0 and earlier the error ended the program before PdfCraft could fall back to OpenGL. Since 0.7.0 ([#1042](https://github.com/storytold/pdfcraft/pull/1042)) PdfCraft retries with OpenGL on its own. This does not fix Direct3D 12 itself, and the issue is still open. `PDFCRAFT_RENDERER=gl` starts with OpenGL from the beginning.

## Still stuck?

Ask in [Discord](https://discord.gg/artcraft) or comment on [issue #1046](https://github.com/storytold/pdfcraft/issues/1046). Say your Windows version, graphics card, which PdfCraft file you installed, and attach `pdfcraft.log`.
