#!/bin/sh
set -eu
# RPM passes 1 to the old package's %postun on upgrade, after the new files are in place:
# restart a running service so it loads the new binary. Debian never passes 1 here.
if [ "${1:-0}" = 1 ]; then systemctl try-restart pdfcraft-hot-folder.service || true; fi
