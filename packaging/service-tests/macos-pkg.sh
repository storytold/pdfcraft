#!/usr/bin/env bash
# Disposable CI machine only: test the actual optional Installer choice and postinstall.
set -euo pipefail
work="$RUNNER_TEMP/hot-folder-pkg"
hot="$work/PdfCraft Hot Folder.app"
main="$work/PdfCraft.app"
mkdir -p "$hot/Contents/MacOS" "$main/Contents/MacOS" "$work/components" "$work/scripts"
cp target/debug/examples/service_probe "$hot/Contents/MacOS/PdfCraft Hot Folder"
cp target/debug/examples/service_probe "$main/Contents/MacOS/PdfCraft"
sed -e 's/@SHORT_VERSION@/0.6.0/g' -e 's/@VERSION@/0.6.0/g' -e 's/@BUILD_SHA@/service-test/g' \
  packaging/macos/HotFolder-Info.plist.in > "$hot/Contents/Info.plist"
sed -e 's/PdfCraft Hot Folder/PdfCraft/g' -e 's/ai.storyteller.pdfcraft.hot-folder/ai.storyteller.pdfcraft/g' \
  "$hot/Contents/Info.plist" > "$main/Contents/Info.plist"
cp packaging/macos/hot-folder-scripts/* "$work/scripts/"
chmod 755 "$work/scripts/"*
codesign --force --sign - "$hot"
codesign --force --sign - "$main"
pkgbuild --component "$main" --install-location /Applications --identifier ai.storyteller.pdfcraft.pkg \
  --version 0.6.0 "$work/components/PdfCraft-component.pkg"
pkgbuild --component "$hot" --install-location /Applications --identifier ai.storyteller.pdfcraft.hot-folder.pkg \
  --version 0.6.0 --scripts "$work/scripts" "$work/components/HotFolder-component.pkg"
sed 's/@SHORT_VERSION@/0.6.0/g' packaging/macos/Distribution.xml.in > "$work/Distribution.xml"
productbuild --distribution "$work/Distribution.xml" --package-path "$work/components" "$work/PdfCraft.pkg"
sudo installer -pkg "$work/PdfCraft.pkg" -target /
test -d /Applications/PdfCraft.app
test ! -d '/Applications/PdfCraft Hot Folder.app'
cat > "$work/choices.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><array><dict><key>choiceIdentifier</key><string>hot-folder</string><key>choiceAttribute</key><string>selected</string><key>attributeSetting</key><integer>1</integer></dict></array></plist>
EOF
sudo installer -pkg "$work/PdfCraft.pkg" -target / -applyChoiceChangesXML "$work/choices.plist"
probe='/Applications/PdfCraft Hot Folder.app/Contents/MacOS/PdfCraft Hot Folder'
config="$HOME/Library/Application Support/PdfCraft/HotFolder/tasks.json"
for _attempt in $(seq 1 40); do
  [ -f "${config%.json}.heartbeat" ] && break
  sleep 0.25
done
test -f "${config%.json}.heartbeat"
"$probe" --service-action status --config "$config" | grep "installed=true enabled=true running=true account=$(id -un)"
# Updating the selected component must keep the existing task file and restart the daemon.
printf '{"tasks":[],"test_marker":"preserved"}' > "$config"
sudo installer -pkg "$work/PdfCraft.pkg" -target / -applyChoiceChangesXML "$work/choices.plist"
grep preserved "$config"
sudo "$probe" --service-action uninstall --config "$config"
"$probe" --service-action status --config "$config" | grep 'installed=false'
grep preserved "$config"
