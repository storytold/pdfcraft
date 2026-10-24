#!/bin/sh
# dpkg remove / rpm final erase only; upgrades keep the queue.
case "${1:-}" in
  remove|0) exec /usr/share/pdfcraft/printer/printer-queue.sh remove ;;
esac
exit 0
