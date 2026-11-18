#!/bin/sh
# Builds a release on this Mac and publishes it on GitHub:
#
#   1. set the version in Cargo.toml and commit it,
#   2. scripts/release.sh
#
# It builds the app (scripts/bundle-macos.sh), packs it as Editor.zip for the
# install command and as Editor-<version>.dmg for the browser, writes their
# checksums and the Homebrew cask, and creates the release v<version> with all
# of it. The fixed name Editor.zip lets the install command always find the
# newest release.
#
# Needs: gh, signed in (gh auth login).
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
tag="v$version"
out="$root/target/bundle"

command -v gh >/dev/null || { echo "Install the GitHub CLI first: brew install gh" >&2; exit 1; }
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "Commit or stash your changes first, so that the release matches the code." >&2
  exit 1
fi
if gh release view "$tag" >/dev/null 2>&1; then
  echo "$tag exists already. Raise the version in Cargo.toml first." >&2
  exit 1
fi

cargo test --workspace --quiet
scripts/bundle-macos.sh

dmg="$out/Editor-$version.dmg"
zip="$out/Editor.zip"
ditto -c -k --keepParent "$out/Editor.app" "$zip"
(cd "$out" && shasum -a 256 "Editor.zip" > "Editor.zip.sha256" && shasum -a 256 "Editor-$version.dmg" > "Editor-$version.dmg.sha256")
checksum="$(cut -d' ' -f1 < "$dmg.sha256")"
sed -e "s/@VERSION@/$version/" -e "s/@SHA256@/$checksum/" packaging/homebrew/editor.rb > "$out/editor.rb"

notes="$out/notes.md"
cat > "$notes" <<EOF
## Install

Open **Terminal** (press Cmd + Space, type Terminal, press Return), paste this line and press Return:

\`\`\`sh
curl -fsSL https://raw.githubusercontent.com/sudo-Oliver/Editor/main/scripts/install.sh | sh
\`\`\`

It downloads this release, checks it, puts Editor into Applications and opens it. You can [read the script](https://github.com/sudo-Oliver/Editor/blob/main/scripts/install.sh) first.

### Or with the dmg

Download \`Editor-$version.dmg\` below, open it and drag Editor into Applications. Editor is free software and not registered with Apple (that costs a yearly fee), so macOS warns before the first start. That warning is about the missing registration, not about anything found in the app. To open it once:

1. Double-click Editor and click **Done** on the warning.
2. Open **System Settings > Privacy & Security**, scroll down and click **Open Anyway** next to Editor.
3. Confirm with your password. From then on, Editor opens normally.

Checksums: \`Editor.zip.sha256\`, \`Editor-$version.dmg.sha256\`.
EOF

git push origin HEAD
gh release create "$tag" "$dmg" "$dmg.sha256" "$zip" "$zip.sha256" "$out/editor.rb" \
  --title "Editor $version" --notes-file "$notes" --target "$(git rev-parse HEAD)"
echo "Published $tag. For Homebrew, copy target/bundle/editor.rb to Casks/editor.rb in sudo-Oliver/homebrew-tap."
