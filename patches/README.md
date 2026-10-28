# PdfCraft EVO Contributions & Patches

This directory contains standalone, rebase-ready patches and git bundles for contributing the UI enhancements and milestone upgrades upstream to [storytold/pdfcraft](https://github.com/storytold/pdfcraft).

---

## 1. Patches Overview

| File | Purpose | Applies To |
|---|---|---|
| [`pdfcraft-ui-capsule-fullscreen.patch`](file:///D:/Coding/Pdf%20craft%20development/patches/pdfcraft-ui-capsule-fullscreen.patch) | **The Core UI Enhancement Patch**:<br>• Read mode integrated capsule bar (all 16 controls & buttons in exact Acrobat-style layout)<br>• Restored bottom floating pill bar for all other modes<br>• Full Screen mode with 100% clean canvas (zero toolbars/overlays)<br>• Custom PC monitor Full Screen icon matching Acrobat<br>• Crisp single-border page number input with auto-selection & Esc/Enter handling<br>• Instant Read / Edit mode toggle buttons | Any clean PdfCraft branch (`crates/ui-egui`) |
| [`0004-feat-ui-integrate-horizontal-capsule-bar-into-Read-m.patch`](file:///D:/Coding/Pdf%20craft%20development/patches/0004-feat-ui-integrate-horizontal-capsule-bar-into-Read-m.patch) | Git format-patch: Capsule top bar integration and zoom slider | Upstream git repository |
| [`0005-feat-ui-match-full-screen-mode-icon-hide-all-overlay.patch`](file:///D:/Coding/Pdf%20craft%20development/patches/0005-feat-ui-match-full-screen-mode-icon-hide-all-overlay.patch) | Git format-patch: Full screen mode icon, overlay hiding & mode switcher | Upstream git repository |
| [`pdfcraft-evo-contributions.bundle`](file:///D:/Coding/Pdf%20craft%20development/patches/pdfcraft-evo-contributions.bundle) | Standalone Git Bundle containing all commit history and metadata | Direct git import / clone |

---

## 2. How to Apply to Any Future Upstream Version

To apply the clean UI patch to any future updated clone or branch:

```bash
# Check if the patch applies cleanly
git apply --check patches/pdfcraft-ui-capsule-fullscreen.patch

# Apply the patch directly
git apply patches/pdfcraft-ui-capsule-fullscreen.patch
```

---

## 3. How to Push to Your GitHub & Contribute Upstream

Since the upstream repository is `storytold/pdfcraft`, contributions are submitted via your personal GitHub fork:

1. **Fork the Repository**:
   Visit [https://github.com/storytold/pdfcraft](https://github.com/storytold/pdfcraft) and click **Fork** (creating `https://github.com/Manideepvelchuri/pdfcraft`).

2. **Add Your Fork as Remote & Push**:
   ```bash
   git remote add myfork https://github.com/Manideepvelchuri/pdfcraft.git
   git push -u myfork checkpoint-stable:feature/read-mode-capsule-fullscreen
   ```

3. **Open a Pull Request**:
   Visit your fork on GitHub and click **Compare & pull request** targeting `storytold/pdfcraft:main`.
