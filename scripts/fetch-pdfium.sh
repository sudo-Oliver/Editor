#!/bin/sh
# Downloads the pinned PDFium build into vendor/pdfium and verifies its checksum.
set -eu

release="chromium/8125"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    asset="pdfium-mac-arm64.tgz"
    checksum="4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d"
    ;;
  Linux-x86_64)
    asset="pdfium-linux-x64.tgz"
    checksum="e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659"
    ;;
  *)
    echo "Unsupported platform: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
target="$root/vendor/pdfium"
archive="$(mktemp)"
trap 'rm -f "$archive"' EXIT

curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/$release/$asset" -o "$archive"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$archive" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$archive" | cut -d' ' -f1)"
fi
if [ "$actual" != "$checksum" ]; then
  echo "Checksum mismatch for $asset: $actual" >&2
  exit 1
fi

rm -rf "$target"
mkdir -p "$target"
tar -xzf "$archive" -C "$target"
echo "PDFium $release installed in $target"
