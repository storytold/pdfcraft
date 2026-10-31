#!/usr/bin/env bash
set -euo pipefail

mode="${1:?usage: scripts/gtk-qa.sh build|test-engine|smoke|viewer|document|palette|automation|form|reconcile|coverage|editing|screens|organizer|text-tools|redact|recents-open|fill-sign|sign|page-ops|panels-edit|export-ops|parity-ops|captures|parity}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
export PATH="$HOME/.cargo/bin:$PATH"

case "$mode" in
  captures)
    for sub in smoke viewer document palette automation form reconcile editing screens organizer text-tools redact recents-open fill-sign sign page-ops panels-edit export-ops parity-ops; do
      "$0" "$sub" || exit 1
    done
    echo "GTK_GATE_OK captures"
    exit 0
    ;;
esac

case "$mode" in
  parity)
    "$0" coverage
    if grep -qE '^\|.*\|[[:space:]]*Pendiente' docs/gtk-parity.md; then
      echo "GTK_QA_FAIL parity: quedan filas Pendiente en docs/gtk-parity.md" >&2
      exit 1
    fi
    echo "GTK_GATE_OK parity"
    ;;
  build)
    cargo check -p pdfcraft-gtk
    echo "GTK_GATE_OK build"
    ;;
  test-engine)
    cargo test -p pdfcraft-engine --lib
    echo "GTK_GATE_OK test-engine"
    ;;
  smoke|viewer|document|palette|automation|form|reconcile|coverage|editing|screens|organizer|text-tools|redact|recents-open|fill-sign|sign|page-ops|panels-edit|export-ops|parity-ops)
    cargo build -p pdfcraft-gtk
    qa_dir="${PDFCRAFT_GTK_QA_DIR:-/tmp/opencode/pdfcraft-gtk-qa}"
    mkdir -p "$qa_dir"
    capture="$qa_dir/$mode.png"
    if [[ "$mode" == organizer ]]; then
      capture="$qa_dir/organizer-ops.png"
    elif [[ "$mode" == sign ]]; then
      capture="$qa_dir/sign-applied.png"
    fi
    log="$qa_dir/$mode.log"
    if [[ "$mode" == screens ]]; then
      rm -f "$qa_dir/organizer.png" "$qa_dir/recents.png" "$qa_dir/redact.png" "$qa_dir/panels.png" "$qa_dir/redact-apply.png" "$log"
    elif [[ "$mode" == redact ]]; then
      rm -f "$qa_dir/redact-apply.png" "$log"
    elif [[ "$mode" == sign ]]; then
      rm -f "$qa_dir/sign-dialog.png" "$qa_dir/sign-applied.png" "$qa_dir/sign-output.pdf" "$log"
    elif [[ "$mode" == page-ops ]]; then
      rm -f "$qa_dir/reemplazo.pdf" "$log"
      rm -rf "$qa_dir/parts"
    elif [[ "$mode" == export-ops ]]; then
      rm -f "$qa_dir/export-ops.txt" "$qa_dir/export-ops.docx" "$log"
      rm -rf "$qa_dir/export-imgs" "$qa_dir/export-imgs-all" "$qa_dir/export-imgs-made"
    else
      rm -f "$capture" "$log"
    fi
    qa_file="${PDFCRAFT_GTK_QA_FILE:-$root/crates/ui-egui/tests/data/form.pdf}"
    if [[ "$mode" == parity-ops ]]; then
      qa_file="$qa_dir/parity-fixture.pdf"
      python3 - "$qa_file" <<'PYEOF'
import sys
path = sys.argv[1]
stream = b"BT /F1 24 Tf 72 700 Td (Hola mundo) Tj ET"
objects = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
]
with open(path, "wb") as fh:
    fh.write(b"%PDF-1.7\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(fh.tell())
        fh.write(f"{number} 0 obj\n".encode() + body + b"\nendobj\n")
    start = fh.tell()
    fh.write(b"xref\n0 " + str(len(objects) + 1).encode() + b"\n0000000000 65535 f \n")
    for offset in offsets:
        fh.write(f"{offset:010d} 00000 n \n".encode())
    fh.write(b"trailer\n<< /Size " + str(len(objects) + 1).encode() + b" /Root 1 0 R >>\nstartxref\n" + str(start).encode() + b"\n%%EOF\n")
PYEOF
    fi
    if [[ "$mode" == viewer || "$mode" == document || "$mode" == editing ]] && [[ -z "${PDFCRAFT_GTK_QA_FILE:-}" ]]; then
      qa_file="$qa_dir/two-pages.pdf"
      pdfunite "$root/crates/ui-egui/tests/data/form.pdf" "$root/crates/ui-egui/tests/data/form.pdf" "$qa_file"
    fi
    if [[ "$mode" == form ]]; then
      rm -f "$qa_dir/exported.txt" "$qa_dir/form-result.png"
    fi
    if [[ "$mode" == reconcile ]]; then
      rm -f "$qa_dir/reconcile-output.pdf"
    fi
    set +e
    # Ejecución bajo Xvfb: determinista. En Wayland nativo el frame clock se
    # suspende cuando ninguna ventana está visible en pantalla y las capturas
    # devuelven 0 nodos (flakes); en Xvfb siempre hay frames. PDFCRAFT_QA_NATIVE=1
    # para forzar la sesión gráfica real.
    if [[ "${PDFCRAFT_QA_NATIVE:-0}" == "1" ]]; then
      timeout 45s env \
        PDFCRAFT_GTK_QA="$mode" \
        PDFCRAFT_GTK_QA_DIR="$qa_dir" \
        PDFCRAFT_GTK_QA_FILE="$qa_file" \
        cargo run --quiet -p pdfcraft-gtk >"$log" 2>&1
    else
      timeout 45s xvfb-run -a -s "-screen 0 1280x900x24" env \
        GDK_BACKEND=x11 \
        PDFCRAFT_GTK_QA="$mode" \
        PDFCRAFT_GTK_QA_DIR="$qa_dir" \
        PDFCRAFT_GTK_QA_FILE="$qa_file" \
        cargo run --quiet -p pdfcraft-gtk >"$log" 2>&1
    fi
    status=$?
    set -e
    cat "$log"
    if [[ $status -ne 0 ]]; then
      echo "GTK_QA_FAIL $mode exit=$status" >&2
      exit "$status"
    fi
    grep -F "GTK_QA_OK $mode" "$log" >/dev/null
    if [[ "$mode" == screens ]]; then
      for name in organizer recents redact panels; do test -s "$qa_dir/$name.png"; done
    elif [[ "$mode" == redact ]]; then
      test -s "$qa_dir/redact-apply.png"
    elif [[ "$mode" == sign ]]; then
      test -s "$qa_dir/sign-dialog.png"
      test -s "$qa_dir/sign-applied.png"
      test -s "$qa_dir/sign-output.pdf"
    else
      test -s "$capture"
    fi
    if [[ "$mode" == sign ]]; then
      pdfinfo "$qa_dir/sign-output.pdf" | grep -E '^Pages:[[:space:]]+1$'
    fi
    if [[ "$mode" == page-ops ]]; then
      test -s "$qa_dir/reemplazo.pdf"
      mapfile -t parts < <(find "$qa_dir/parts" -maxdepth 1 -name '*.pdf' | sort)
      [[ ${#parts[@]} -eq 2 ]]
      for part in "${parts[@]}"; do
        pdfinfo "$part" | grep -E '^Pages:[[:space:]]+1$'
      done
    fi
    if [[ "$mode" == export-ops ]]; then
      test -s "$qa_dir/export-ops.txt"
      test -s "$qa_dir/export-ops.docx"
      mapfile -t exported_pages < <(find "$qa_dir/export-imgs" -maxdepth 1 -name '*.png' | sort)
      [[ ${#exported_pages[@]} -eq 1 ]]
      mapfile -t made_images < <(find "$qa_dir/export-imgs-made" -maxdepth 1 -name '*_Page_1_Image_0001.png' | sort)
      [[ ${#made_images[@]} -eq 1 ]]
      head -c 4 "${made_images[0]}" | od -An -tx1 | grep -q '89 50 4e 47'
    fi
    if [[ "$mode" == document ]]; then
      pdfinfo "$qa_dir/document-output.pdf" | grep -E '^Pages:[[:space:]]+3$'
    fi
    if [[ "$mode" == form ]]; then
      test -s "$qa_dir/exported.txt"
      test -s "$qa_dir/form-result.png"
    fi
    if [[ "$mode" == reconcile ]]; then
      pdfinfo "$qa_dir/reconcile-output.pdf" | grep -E '^Pages:[[:space:]]+1$'
    fi
    echo "GTK_GATE_OK $mode capture=$capture"
    ;;
  *)
    echo "unknown QA mode: $mode" >&2
    exit 2
    ;;
esac
