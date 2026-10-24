#!/bin/sh
# Local CUPS 2.x only. Never changes the default printer or removes saved PDFs.
set -eu
export LC_ALL=C
export CUPS_SERVER=/run/cups/cups.sock
queue=PdfCraft_Printer
uri=$(lpstat -v "$queue" 2>/dev/null || true)
if [ -n "$uri" ] && [ "$uri" != "device for $queue: pdfcraft:/" ]; then
  echo "PdfCraft: a different printer already uses $queue; it was not changed" >&2
  exit 1
fi
case "${1:-install}" in
  install)
    [ "$(id -u)" = 0 ] || { echo 'Run printer setup as root' >&2; exit 1; }
    lpstat -r >/dev/null
    install -d -o root -g root -m 0755 /var/spool/pdfcraft-printer
    lpadmin -p "$queue" -E -D 'PdfCraft Printer' -v pdfcraft:/ \
      -P /usr/share/cups/model/pdfcraft.ppd \
      -o printer-is-shared=false -o printer-op-policy=authenticated \
      -o printer-error-policy=abort-job -o job-sheets-default=none,none
    ;;
  remove)
    if [ -n "$uri" ]; then
      [ -z "$(lpstat -o "$queue")" ] || { echo 'Finish or cancel pending PdfCraft print jobs before uninstalling' >&2; exit 1; }
      lpadmin -x "$queue"
    fi
    ;;
  *) echo 'Usage: printer-queue.sh install|remove' >&2; exit 2 ;;
esac
