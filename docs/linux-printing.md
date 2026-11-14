# Printer settings missing on Linux

## Symptom

On a Linux desktop the Print dialog lists the CUPS queue, but **Printer properties…** says
"This printer offers no settings of its own." The same queue shows its full option set (trays,
paper type, finishing, duplex, color mode) in GTK print dialogs such as Chrome or Evince, and
`lpoptions -p <queue> -l` prints every option.

## Cause

`printer_options_via_within` in `crates/print/src/spool.rs` reads the queue's PPD straight from
disk, at `/etc/cups/ppd/<queue>.ppd`. CUPS writes those files as root with the scheduler's group
and mode `0640`. On Arch Linux that is `root:cups`; other distributions use `lp` or another
group. A user who is not in that group cannot open the file. The open error is discarded, so the
option list comes back empty and the dialog reports a printer with no settings. The subsequent
`lpoptions -p <queue> -l` call still succeeds, because it asks the CUPS scheduler over its
socket rather than reading the file, but it is only used to update defaults on options that
were already parsed.

The Flatpak build is affected on every system: its sandbox has no `/etc/cups`, only the CUPS
socket and `~/.cups`.

## Workaround

Find the group that owns the PPDs and join it, then log out and back in so the new group
membership applies:

```sh
ls -l /etc/cups/ppd/                 # note the group, e.g. cups or lp
sudo usermod -aG <group> "$USER"
```

Check with `test -r /etc/cups/ppd/<queue>.ppd && echo readable` after logging in again.
Printer properties then lists the PPD's options.

Without changing groups, queue defaults can still be set for your user with CUPS itself, for
example `lpoptions -p <queue> -o Duplex=DuplexNoTumble`. They are saved in `~/.cups/lpoptions`
and `lp` applies them to every job, including PdfCraft's. Note that some vendor PPDs name
duplex with their own keyword (for example a `Print Type` option) rather than the standard
`Duplex`, and the generic `sides=` job option does not drive those.

## Why the package installer cannot fix it

The `.deb`/`.rpm` post-install script runs as root at install time. It does not know which
desktop users will print, and adding users to the scheduler's group would widen their access to
CUPS's files beyond what PdfCraft needs. The Flatpak cannot read the directory at all.

The durable fix is in the print code: obtain the PPD from the scheduler rather than the
filesystem. The scheduler serves `GET /printers/<queue>.ppd` to any local client (this is what
libcups's `cupsGetPPD` does), and it works over the CUPS socket the Flatpak already has. Failing
that, the option list can be built from `lpoptions -p <queue> -l`, whose lines carry the
keyword, label, choices and current default of every option.

Verified on Arch Linux with CUPS 2.4.20 and PdfCraft 0.19.0 against a Konica Minolta
PostScript queue: before joining the `cups` group the dialog showed no settings; after joining
and logging in again it listed the PPD's options.
