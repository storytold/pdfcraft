# Policies for administrators

An organisation can set some of PdfCraft's settings for everyone on a machine, in the places its
management tools already write: Group Policy, Intune or an RMM's `reg add` on Windows, a
configuration profile from an MDM on macOS, and a file under `/etc` on Linux and the BSDs. A
policy wins over Preferences, and people can't change it: the setting shows as off and locked,
with a note saying it was set by their administrator and where.

PdfCraft reads policies when it starts, so a change applies once every PdfCraft window has been
closed and it is started again (opening another file while it runs hands the file to the running
copy). With no policy, nothing changes. The desktop app and `pdfcraft-cli` (including `run` and
`mcp`) both follow them, and so do files processed in the background: the Action Wizard,
multi-file OCR and `doc_batch`.

## Policies

| Name | Type | Effect |
|---|---|---|
| `DisableJavaScript` | flag | `1` (or `true`) turns JavaScript in documents off and keeps it off: field scripts, calculations, validation, button and link scripts, the Action Wizard's "Run a JavaScript" step, the JavaScript console, and the scripts of XFA forms (JavaScript and FormCalc). Acrobat's standard format, validate and calculate functions (`AFNumber_Format` and the rest) keep working, as they do with JavaScript turned off in Preferences. `0` (or `false`) leaves the choice to the person using the app. It is the equivalent of Acrobat's `bDisableJavaScript`. |

A flag is the number `0` or `1`, or a boolean where the store has one.

## Where policies go

### Windows

Values under `HKEY_LOCAL_MACHINE\SOFTWARE\Policies\PdfCraft` for the whole machine, or
`HKEY_CURRENT_USER\SOFTWARE\Policies\PdfCraft` for one user (Group Policy's User
Configuration). Flags are `REG_DWORD`. When both are set, the one that restricts more wins.

```powershell
New-Item -Path 'HKLM:\SOFTWARE\Policies\PdfCraft' -Force | Out-Null
New-ItemProperty -Path 'HKLM:\SOFTWARE\Policies\PdfCraft' -Name 'DisableJavaScript' -PropertyType DWord -Value 1 -Force | Out-Null
```

or, from `cmd` or an RMM component:

```
reg add "HKLM\SOFTWARE\Policies\PdfCraft" /v DisableJavaScript /t REG_DWORD /d 1 /f
```

Only administrators can write either key, `HKCU\SOFTWARE\Policies` included.

### macOS

Managed preferences for the domain `ai.storyteller.pdfcraft`, delivered by a configuration profile.
Most MDMs take the keys as a preference file (plist) for that domain, under a name such as
Application & Custom Settings or Preference file. Flags are booleans. A value someone sets
themselves with `defaults write` is not a policy and is ignored.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>DisableJavaScript</key>
    <true/>
</dict>
</plist>
```

`defaults read "/Library/Managed Preferences/ai.storyteller.pdfcraft"` shows what reached a Mac.

### Linux and the BSDs

A JSON object in `/etc/pdfcraft/policy.json` (`/usr/local/etc/pdfcraft/policy.json` on FreeBSD):

```json
{ "DisableJavaScript": true }
```

Keys PdfCraft doesn't know are ignored, so one file can serve several versions. The Flatpak build
can't see the host's `/etc` and doesn't read this file yet.

## When a policy can't be read

A policy that is there but can't be read, such as a `REG_SZ` where a `REG_DWORD` belongs, a value
other than 0 or 1, a damaged or unreadable `policy.json`, one larger than 64 KB, something at
that path that isn't a regular file (a folder, a pipe, a device, or a link that leads nowhere), or
a `pdfcraft` folder that is a link leading nowhere, counts as the most restrictive setting:
JavaScript stays off. Preferences says the policy couldn't be read, shows where it is and what was
wrong, and the same goes to the log. An administrator who put something there meant to manage the
setting, and falling back to the default would leave JavaScript on where they believe it is off,
with nothing to show it.
