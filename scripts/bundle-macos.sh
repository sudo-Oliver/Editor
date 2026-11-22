#!/bin/sh
# Builds Editor.app for Apple silicon and Intel in one, and packs it into a .dmg.
#
# Without signing details the app is signed ad hoc. It runs on this Mac, but on
# others macOS warns that it comes from an unidentified developer. For a release,
# set these in the environment (in CI they come from repository secrets):
#
#   EDITOR_SIGN_IDENTITY     "Developer ID Application: Name (TEAMID)", from the keychain
#   EDITOR_NOTARY_PROFILE    a profile stored with `xcrun notarytool store-credentials`
#     or all three of
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#
# Signing details never go into this repository.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
bundle_id="io.github.sudo-oliver.editor"
out="$root/target/bundle"
app="$out/Editor.app"
dmg="$out/Editor-$version.dmg"

echo "Building Editor $version"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$target" >/dev/null
  cargo build --release -p app --target "$target"
done

for arch in arm64 x86_64; do
  if [ ! -f "vendor/pdfium-$arch/lib/libpdfium.dylib" ]; then
    scripts/fetch-pdfium.sh "$arch"
  fi
done

rm -rf "$out"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources"
lipo -create \
  target/aarch64-apple-darwin/release/editor \
  target/x86_64-apple-darwin/release/editor \
  -output "$app/Contents/MacOS/editor"
lipo -create \
  vendor/pdfium-arm64/lib/libpdfium.dylib \
  vendor/pdfium-x86_64/lib/libpdfium.dylib \
  -output "$app/Contents/Frameworks/libpdfium.dylib"

# The icon: the SVG drawn once at full size, then every size macOS asks for.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
qlmanage -t -s 1024 -o "$work" packaging/macos/AppIcon.svg >/dev/null 2>&1
iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$work/AppIcon.svg.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$work/AppIcon.svg.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

# The `editor` command for the terminal, linked into the PATH from the app's Help menu or by Homebrew.
mkdir -p "$app/Contents/Resources/bin"
cp packaging/macos/editor "$app/Contents/Resources/bin/editor"
chmod 755 "$app/Contents/Resources/bin/editor"

sed -e "s/@VERSION@/$version/g" -e "s/@BUNDLE_ID@/$bundle_id/g" \
  packaging/macos/Info.plist > "$app/Contents/Info.plist"

identity="${EDITOR_SIGN_IDENTITY:--}"
if [ "$identity" = "-" ]; then
  echo "No EDITOR_SIGN_IDENTITY: signing ad hoc, for this Mac only."
fi
sign() {
  codesign --force --options runtime --timestamp --sign "$identity" "$@"
}
if [ "$identity" = "-" ]; then
  sign() {
    codesign --force --sign - "$@"
  }
fi
sign "$app/Contents/Frameworks/libpdfium.dylib"
sign "$app"
codesign --verify --strict --deep "$app"

# The dmg window: Editor, an arrow, Applications, and what to do about the
# warning macOS shows for an app that is not notarized.
staging="$work/dmg"
mkdir -p "$staging/.background"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
cp "packaging/macos/First open.txt" "$staging/First open.txt"
# Quick Look draws the square SVG; the middle band is the 640 by 400 window.
qlmanage -t -s 1280 -o "$work" packaging/macos/DmgBackground.svg >/dev/null 2>&1
sips -c 800 1280 "$work/DmgBackground.svg.png" --out "$work/background@2x.png" >/dev/null
sips -z 400 640 "$work/background@2x.png" --out "$work/background.png" >/dev/null
tiffutil -cathidpicheck "$work/background.png" "$work/background@2x.png" -out "$staging/.background/background.tiff" >/dev/null 2>&1

writable_dmg="$work/Editor-rw.dmg"
hdiutil create -volname "Editor" -srcfolder "$staging" -fs HFS+ -format UDRW -ov "$writable_dmg" >/dev/null
volume="$(hdiutil attach -readwrite -noverify -noautoopen "$writable_dmg" | sed -n 's|^/dev/.*\(/Volumes/.*\)$|\1|p' | head -n 1)"
# Finder sets the layout. Without a desktop session (or the permission to
# script Finder) the dmg keeps Finder's default layout, which still works.
osascript - "$(basename "$volume")" <<'APPLESCRIPT' || echo "Cannot lay out the dmg window: it keeps Finder's default." >&2
on run argv
  tell application "Finder"
    tell disk (item 1 of argv)
      open
      set current view of container window to icon view
      set toolbar visible of container window to false
      set statusbar visible of container window to false
      set the bounds of container window to {200, 120, 840, 548}
      set options to the icon view options of container window
      set arrangement of options to not arranged
      set icon size of options to 96
      set text size of options to 13
      set background picture of options to file ".background:background.tiff"
      set position of item "Editor.app" of container window to {160, 170}
      set position of item "Applications" of container window to {480, 170}
      set position of item "First open.txt" of container window to {320, 290}
      update without registering applications
      delay 1
      close
    end tell
  end tell
end run
APPLESCRIPT
sync
hdiutil detach -quiet "$volume" || hdiutil detach -force -quiet "$volume"
hdiutil convert "$writable_dmg" -format UDZO -ov -o "$dmg" >/dev/null

if [ "$identity" != "-" ]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
  if [ -n "${EDITOR_NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$dmg" --keychain-profile "$EDITOR_NOTARY_PROFILE" --wait
  elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
    xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
  else
    echo "No notary details: the dmg is signed but not notarized." >&2
    echo "$dmg"
    exit 0
  fi
  xcrun stapler staple "$dmg"
  spctl --assess --type open --context context:primary-signature -vv "$dmg"
fi
echo "$dmg"
