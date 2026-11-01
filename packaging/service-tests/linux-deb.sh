#!/usr/bin/env bash
# Disposable CI machine only: exercise real package scripts and restricted service identity.
set -euo pipefail
work="$RUNNER_TEMP/hot-folder-deb"
mkdir -p "$work/main/DEBIAN" "$work/stage/usr/bin"
cat > "$work/main/DEBIAN/control" <<'EOF'
Package: pdfcraft
Version: 0.6.0
Architecture: all
Maintainer: PdfCraft contributors
Description: Synthetic main-package dependency for installer integration testing
EOF
dpkg-deb --build "$work/main" "$work/main.deb"
sudo dpkg -i "$work/main.deb"
if systemctl is-enabled pdfcraft-hot-folder.service 2>/dev/null; then
  echo 'Installing only the main package unexpectedly enabled Hot Folder' >&2; exit 1
fi
cp target/debug/examples/service_probe "$work/stage/usr/bin/pdfcraft-hot-folder"
export NFPM_ARCH=amd64 VERSION=0.6.0 PDFCRAFT_MAINTAINER='PdfCraft contributors'
sed "s|\${HOT_STAGE}|$work/stage|g" packaging/linux/hot-folder-nfpm.yaml > "$work/nfpm.yaml"
nfpm package -f "$work/nfpm.yaml" -p deb -t "$work/hot-folder.deb"
sudo dpkg -i "$work/hot-folder.deb"
probe=/usr/bin/pdfcraft-hot-folder
config=/var/lib/pdfcraft-hot-folder/tasks.json
for _attempt in $(seq 1 40); do
  sudo test -f /var/lib/pdfcraft-hot-folder/tasks.heartbeat && break
  sleep 0.25
done
sudo test -f /var/lib/pdfcraft-hot-folder/tasks.heartbeat
"$probe" --service-action status --config "$config" | grep 'installed=true enabled=true running=true account=pdfcraft-hot-folder'
sudo runuser -u pdfcraft-hot-folder -- sh -c 'printf "{\"tasks\":[],\"test_marker\":\"preserved\"}" > /var/lib/pdfcraft-hot-folder/tasks.json'
sudo dpkg -i "$work/hot-folder.deb"
sudo grep preserved "$config"
sudo dpkg -r pdfcraft-hot-folder
test ! -e /etc/systemd/system/pdfcraft-hot-folder.service
sudo grep preserved "$config"
sudo dpkg -r pdfcraft
