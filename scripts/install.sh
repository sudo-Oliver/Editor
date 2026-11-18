#!/bin/sh
# Installs the latest Editor from its GitHub releases:
#
#   curl -fsSL https://raw.githubusercontent.com/sudo-Oliver/Editor/main/scripts/install.sh | sh
#
# What it does, and nothing else:
#   1. downloads Editor.zip and its checksum from the latest release,
#   2. checks the checksum and the app's code signature,
#   3. puts Editor.app into /Applications (or ~/Applications), replacing an
#      older one, and opens it.
#
# Editor is not registered with Apple, so a download in the browser makes
# macOS warn before the first start. A download with curl carries no such mark,
# so the app opens without the warning. Nothing here needs an administrator.
#
# For a test of this script, EDITOR_RELEASE overrides where the release lies,
# and EDITOR_INSTALL_DIR where the app goes.
set -eu

release="${EDITOR_RELEASE:-https://github.com/sudo-Oliver/Editor/releases/latest/download}"

say() { printf '%s\n' "$*"; }
fail() {
  printf 'Editor was not installed: %s\n' "$*" >&2
  exit 1
}

[ "$(uname -s)" = "Darwin" ] || fail "Editor runs on macOS only."
major="$(sw_vers -productVersion | cut -d. -f1)"
[ "$major" -ge 11 ] || fail "Editor needs macOS 11 or later."

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

say "Downloading Editor…"
curl -fL --progress-bar --proto '=https,file' -o "$work/Editor.zip" "$release/Editor.zip" \
  || fail "the download did not work. Check the internet connection and try again."
curl -fsSL --proto '=https,file' -o "$work/Editor.zip.sha256" "$release/Editor.zip.sha256" \
  || fail "its checksum could not be downloaded."

say "Checking the download…"
expected="$(cut -d' ' -f1 < "$work/Editor.zip.sha256")"
actual="$(shasum -a 256 "$work/Editor.zip" | cut -d' ' -f1)"
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "the download is not the released file."
ditto -x -k "$work/Editor.zip" "$work/unpacked" || fail "the download could not be unpacked."
app="$work/unpacked/Editor.app"
[ -d "$app" ] || fail "the download holds no Editor.app."
codesign --verify --strict --deep "$app" 2>/dev/null || fail "the app's signature is broken."

# /Applications when the user may write there, as admins can, else their own.
target="${EDITOR_INSTALL_DIR:-/Applications}"
if [ ! -w "$target" ]; then
  target="$HOME/Applications"
  mkdir -p "$target"
fi

# A running Editor would keep the old version open.
if pgrep -xq editor; then
  say "Closing the running Editor…"
  osascript -e 'tell application id "io.github.sudo-oliver.editor" to quit' >/dev/null 2>&1 || true
  sleep 2
fi

say "Installing into $target…"
rm -rf "$target/Editor.app.new"
ditto "$app" "$target/Editor.app.new"
rm -rf "$target/Editor.app"
mv "$target/Editor.app.new" "$target/Editor.app"

version="$(defaults read "$target/Editor.app/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo "")"
say "Editor $version is installed. It opens now; its setup installs what your documents need."
open "$target/Editor.app"
