#!/usr/bin/env bash
# Run on a disposable native Linux CUPS 2.x test machine after building the app and CLI.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug"
sudo install -Dm700 "$BIN/pdfcraft-cups" /usr/lib/cups/backend/pdfcraft
sudo install -Dm644 "$ROOT/packaging/linux/pdfcraft.ppd" /usr/share/cups/model/pdfcraft.ppd
cupstestppd -W all "$ROOT/packaging/linux/pdfcraft.ppd"
before=$(lpstat -d || true)
sudo sh "$ROOT/packaging/linux/printer-queue.sh" install
test "$(lpstat -d || true)" = "$before"
lpstat -v PdfCraft_Printer | grep -F 'pdfcraft:/'
cat >"$WORK/create.json" <<'JSON'
[
  {"tool":"doc_create","args":{"from":"text","text":"PdfCraft Printer keeps these words selectable."}},
  {"tool":"doc_save","args":{"doc":1,"path":"source.pdf"}}
]
JSON
"$BIN/pdfcraft-cli" run --root "$WORK" --script "$WORK/create.json"
output="/var/spool/pdfcraft-printer/$(id -u)"
# Authenticated local submission, through the real CUPS queue and PDF pass-through PPD.
timeout 30 lp -h /run/cups/cups.sock -d PdfCraft_Printer "$WORK/source.pdf"
found=''
for attempt in $(seq 1 50); do
  if [ -d "$output" ]; then
    for file in "$output"/*.pdf; do
      if [ -f "$file" ] && cmp -s "$WORK/source.pdf" "$file"; then found="$file"; break; fi
    done
  fi
  [ -n "$found" ] && break
  sleep 0.2
done
test -n "$found"
test "$(stat -c %a "$found")" = 600
test "$(stat -c %u "$found")" = "$(id -u)"
pdftotext "$found" - | grep -F 'PdfCraft Printer keeps these words selectable.'
sudo sh "$ROOT/packaging/linux/printer-queue.sh" remove
test -f "$found" # uninstall preserves printed documents
test "$(lpstat -d || true)" = "$before"
