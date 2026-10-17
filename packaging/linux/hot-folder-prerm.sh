#!/bin/sh
set -eu
# Debian passes "upgrade"; RPM passes 1 for an upgrade. Leave registration for the next version.
case "${1:-remove}" in upgrade|1) systemctl stop pdfcraft-hot-folder.service; exit 0 ;; esac
if [ -f /etc/systemd/system/pdfcraft-hot-folder.service ]; then
  /usr/bin/pdfcraft-hot-folder --service-action uninstall --config /var/lib/pdfcraft-hot-folder/tasks.json
fi
# Keep task definitions, receipts and the service account, so a reinstall cannot reprocess files.
