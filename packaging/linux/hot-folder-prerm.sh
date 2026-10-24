#!/bin/sh
set -eu
# Upgrades leave registration for the next version. Debian runs the old prerm ("upgrade")
# before the new postinst, so stopping here is safe. RPM runs the old %preun (1) after the new
# %post: stopping then would leave the service down, so hot-folder-postrm.sh restarts it instead.
case "${1:-remove}" in
  upgrade) systemctl stop pdfcraft-hot-folder.service; exit 0 ;;
  1) exit 0 ;;
esac
if [ -f /etc/systemd/system/pdfcraft-hot-folder.service ]; then
  /usr/bin/pdfcraft-hot-folder --service-action uninstall --config /var/lib/pdfcraft-hot-folder/tasks.json
fi
# Keep task definitions, receipts and the service account, so a reinstall cannot reprocess files.
