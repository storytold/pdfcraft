#!/usr/bin/env bash
# Native Installer resources; build-time only, no installer script or network.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
destination="${1:?usage: prepare-resources.sh NEW_RESOURCE_DIRECTORY}"
if [ -e "$destination" ]; then echo "resource destination already exists: $destination" >&2; exit 1; fi
mkdir -p "$destination"
cp -R "$here/resources/"* "$destination/"
# Installer follows the system language preference list. English copies for the
# other macOS display languages prevent falling through to a supported *secondary*
# preference (e.g. Polish, then German). These are fallbacks, not translations.
# Ask the build SDK for ISO language codes rather than maintaining a stale list
# of macOS languages. Only directory names are generated; every fallback is the
# same contributor-original English catalog. Script-specific Chinese aliases
# ensure regional preferences resolve through the matching native script.
# Keep AppleScript outside shell substitutions for macOS /bin/bash 3.2.
fallbacks="$(osascript "$here/list-languages.applescript")"
while IFS= read -r language; do
  if [[ ! "$language" =~ ^[a-z]{2,3}$ ]]; then echo "Unexpected ISO language code: $language" >&2; exit 1; fi
  # Never overwrite a real catalog copied above.
  if [ -f "$destination/$language.lproj/Localizable.strings" ]; then continue; fi
  # Chinese resolution must go through its script-specific translated resources.
  if [ "$language" = zh ]; then continue; fi
  mkdir -p "$destination/$language.lproj"
  cp "$here/resources/en.lproj/Localizable.strings" "$destination/$language.lproj/Localizable.strings"
done <<< "$fallbacks"
# A non-localized English table also covers missing/unknown locale resources.
cp "$here/resources/en.lproj/Localizable.strings" "$destination/Localizable.strings"
find "$destination" -name Localizable.strings -exec plutil -lint {} \;
