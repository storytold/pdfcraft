#!/usr/bin/env bash
# Runs only on disposable CI machines. Verifies real boot-service registration and termination.
set -euo pipefail
service_dir=/Library/PdfCraftServiceTest
if [ "$(uname -s)" = Linux ]; then service_dir=/usr/lib/pdfcraft-service-test; fi
sudo install -d -m 755 "$service_dir"
sudo install -m 755 target/debug/examples/service_probe "$service_dir/pdfcraft-hot-folder"
probe="$service_dir/pdfcraft-hot-folder"
directory="$RUNNER_TEMP/Hot Folder service test"
mkdir -m 700 "$directory"
config="$directory/tasks.json"
(umask 077; printf '{}\n' > "$config")
# The diagnostic log intentionally belongs to the unprivileged CI account.
# shellcheck disable=SC2024
if sudo "$probe" --service --config "$config" > "$directory/root-run.log" 2>&1; then
  echo 'The processing worker must reject root execution' >&2; exit 1
fi
grep 'non-root account' "$directory/root-run.log"
test ! -e "$directory/tasks.heartbeat"
cleanup() { sudo "$probe" --service-action uninstall --config "$config" || true; }
trap cleanup EXIT
sudo "$probe" --service-action install --account "$(id -un)" --config "$config"
for _attempt in $(seq 1 40); do
  [ -f "$directory/tasks.heartbeat" ] && break
  sleep 0.25
done
test -f "$directory/tasks.heartbeat"
"$probe" --service-action status --config "$config" | grep 'installed=true enabled=true running=true'
sudo "$probe" --service-action disable --config "$config" | grep 'enabled=false'
sudo "$probe" --service-action enable --config "$config" | grep 'enabled=true'
sudo "$probe" --service-action stop --config "$config"
for _attempt in $(seq 1 40); do
  [ -f "$directory/tasks.stopped" ] && break
  sleep 0.25
done
test -f "$directory/tasks.stopped"
"$probe" --service-action status --config "$config" | grep 'running=false'
sudo "$probe" --service-action start --config "$config"
sudo "$probe" --service-action uninstall --config "$config"
"$probe" --service-action status --config "$config" | grep 'installed=false'
trap - EXIT
