---
title: Install PdfCraft
description: Download PdfCraft for Windows 11, macOS or Linux from getartcraft.com or GitHub, or install it with ArtCraft Launcher.
order: 10
verified: 0.7.1
---

Download PdfCraft from the [PdfCraft page on getartcraft.com](https://getartcraft.com/apps/pdfcraft), or pick a specific file from the [latest GitHub release](https://github.com/storytold/pdfcraft/releases/latest). In the file names, `<version>` stands for the version number, for example `0.7.1`.

## Windows 11

1. Open the [latest release](https://github.com/storytold/pdfcraft/releases/latest).
2. Download `pdfcraft-<version>-windows-x64.msi`. It fits most PCs (64-bit Intel and AMD).
3. Run the installer. It installs for all users and needs administrator rights.
4. Start PdfCraft from the Start menu. The installer adds a Start menu shortcut and, unless you untick it, a desktop shortcut.

Pick the file that matches your PC:

| Your PC                      | Installer                              | Portable zip                                    |
|------------------------------|----------------------------------------|-------------------------------------------------|
| 64-bit Intel or AMD          | `pdfcraft-<version>-windows-x64.msi`   | `pdfcraft-<version>-windows-x64-portable.zip`   |
| Snapdragon and other ARM PCs | `pdfcraft-<version>-windows-arm64.msi` | `pdfcraft-<version>-windows-arm64-portable.zip` |

The release also has `x86` files. Do not use them: users who installed the `x86` build on a 64-bit PC reported that PdfCraft exited at launch (see [PdfCraft does not start](../troubleshooting/pdfcraft-does-not-start.md)).

The installer adds PdfCraft to **Open with** for PDF files. It does not change which app opens PDFs by default. Unless you untick it, it also adds **Edit with PdfCraft** to the right-click menu of PDF files (on Windows 11, under **Show more options**). See [PdfCraft is missing from Open with](../faq/open-with-menu.md).

### Portable version

The portable zip runs from any folder, including a USB stick. Unzip it and run `pdfcraft.exe`. A `portable.txt` file next to the program keeps settings, logs and crash recovery in a `PdfCraftData` folder beside `pdfcraft.exe`, so nothing is written to `%APPDATA%`. Delete `portable.txt` to use the normal per-user folders instead.

## macOS

Download `pdfcraft-<version>-macos-universal.dmg`. It is signed and notarized, and works on Apple silicon and Intel Macs.

## Linux

Every Linux build except RISC-V needs glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or newer). Replace `x86_64` with `aarch64` on ARM.

| Format           | File                                       |
|------------------|--------------------------------------------|
| AppImage         | `pdfcraft-<version>-linux-x86_64.AppImage` |
| Flatpak          | `pdfcraft-<version>-linux-x86_64.flatpak`  |
| Debian, Ubuntu   | `pdfcraft-<version>-linux-x86_64.deb`      |
| Fedora, openSUSE | `pdfcraft-<version>-linux-x86_64.rpm`      |
| Tarball          | `pdfcraft-<version>-linux-x86_64.tar.gz`   |

The release also has FreeBSD, RISC-V Linux and web (`pdfcraft-web-<version>.zip`) builds, and `pdfcraft-cli` command-line downloads.

## Install with ArtCraft Launcher

ArtCraft Launcher lists PdfCraft among the Craft apps. See [ArtCraft Launcher](https://getartcraft.com/docs/craft-launcher) for how to install and update apps with it.

## Check the download

Every release has a `SHA256SUMS.txt` file with a checksum for every download.
