#!/bin/sh
# Installing this separate optional package opts in to a restricted system service.
set -eu
account=pdfcraft-hot-folder
if ! id "$account" >/dev/null 2>&1; then
  useradd --system --user-group --home-dir /var/lib/pdfcraft-hot-folder --shell /usr/sbin/nologin "$account"
fi
[ "$(id -u "$account")" -ne 0 ] || { echo 'Hot Folder must not run as root.' >&2; exit 1; }
directory=/var/lib/pdfcraft-hot-folder
[ ! -L "$directory" ] || { echo 'The service state directory must not be a symbolic link.' >&2; exit 1; }
install -d -m 700 -o "$account" -g "$account" "$directory"
runuser -u "$account" -- /usr/bin/pdfcraft-hot-folder --initialize-config --config "$directory/tasks.json"
/usr/bin/pdfcraft-hot-folder --service-action install --account "$account" --config "$directory/tasks.json"
