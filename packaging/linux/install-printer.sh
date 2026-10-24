#!/bin/sh
# Install the printer from a native package, extracted tar, or an AppImage's usr/ tree.
set -eu
[ "$(id -u)" = 0 ] || { echo 'Run PdfCraft Printer installation with sudo' >&2; exit 1; }
base=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
if [ "$base" != /usr ]; then
  install -Dm700 "$base/lib/cups/backend/pdfcraft" /usr/lib/cups/backend/pdfcraft
  install -Dm644 "$base/share/cups/model/pdfcraft.ppd" /usr/share/cups/model/pdfcraft.ppd
  install -Dm755 "$base/share/pdfcraft/printer/printer-queue.sh" /usr/share/pdfcraft/printer/printer-queue.sh
fi
exec /usr/share/pdfcraft/printer/printer-queue.sh install
