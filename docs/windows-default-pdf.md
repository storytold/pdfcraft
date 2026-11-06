# Windows default PDF application

After an existing PDF has opened successfully, the Windows desktop app offers to open Default Apps settings once per session. Failed opens, a pending password, creating a new PDF, other platforms, and `--control` automation sessions do not trigger the offer. A read-only background registry query suppresses the offer when `PdfCraft.Document` is the user's selected PDF ProgID; the Classes default is used only when no per-user choice exists. An Open With registration for `Applications\pdfcraft.exe` is also recognized when its parsed command names the current executable exactly, without probing or executing that path. A failed query suppresses the offer and records a diagnostic.

**Yes** opens Windows Settings. The user chooses the association there; PdfCraft never writes the protected `UserChoice` key or its hash. An existing HKCU registration uses `registeredAppUser=PdfCraft`; the MSI's HKLM registration uses `registeredAppMachine=PdfCraft`. On older Windows, missing registrations or an unavailable app-specific URI, the general Default Apps page is used. The installer remains responsible for registration; portable launches do not create it.

**Do not ask again** is saved with the normal application preferences on Yes, Later, Close and Escape. Preferences provides **Ask to make PdfCraft the default PDF app** and an explicit **Open default app settings** action. Re-enabling the preference does not restart a prompt already used during the current session. Opening settings failures leave the document open and report the error.

The opt-in UI control channel supports:

```text
pdfcraft-cli ui --control control.json set key=ask-default-pdf value=off
pdfcraft-cli ui --control control.json command id=app.default_pdf_settings
pdfcraft-cli ui --control control.json state
```

`ui.state` includes `ask_default_pdf` and `default_pdf_prompt`. The command is disabled when the frontend has no Windows integration. Automated tests inject callbacks and synthetic PDF bytes; they neither install an application nor change or open host Windows settings.

Protocol reference: [Microsoft: Launch the Default Apps settings page](https://learn.microsoft.com/windows/apps/develop/launch/launch-default-apps-settings). App-specific links require Windows 11's April 2023 cumulative update or later (22000.1817 / 22621.1555); Windows 10 uses the general page.
