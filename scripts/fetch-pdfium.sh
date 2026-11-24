#!/bin/sh
# Downloads the pinned PDFium build and verifies its checksum.
#
#   scripts/fetch-pdfium.sh            the build for this machine, into vendor/pdfium
#   scripts/fetch-pdfium.sh windows    the Windows x64 build, into vendor/pdfium-windows
#   scripts/fetch-pdfium.sh x86_64     a macOS build for that architecture, into
#                                      vendor/pdfium-x86_64 (used by bundle-macos.sh)
set -eu

release="chromium/8125"
root="$(cd "$(dirname "$0")/.." && pwd)"
if [ "${1:-}" = "windows" ]; then
  platform="Windows-x86_64"
  target="$root/vendor/pdfium-windows"
elif [ $# -gt 0 ]; then
  platform="Darwin-$1"
  target="$root/vendor/pdfium-$1"
else
  platform="$(uname -s)-$(uname -m)"
  target="$root/vendor/pdfium"
fi

case "$platform" in
  Darwin-arm64)
    asset="pdfium-mac-arm64.tgz"
    checksum="4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d"
    ;;
  Darwin-x86_64)
    asset="pdfium-mac-x64.tgz"
    checksum="8a5193535b80c7215d8932f6647ae5097465b6dba2811602c21e120c2af112b0"
    ;;
  # Git Bash on Windows, as the CI runner has it.
  MINGW64_NT*-x86_64 | MSYS_NT*-x86_64 | Windows-x86_64)
    asset="pdfium-win-x64.tgz"
    checksum="9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b"
    ;;
  Linux-x86_64)
    asset="pdfium-linux-x64.tgz"
    checksum="e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659"
    ;;
  *)
    echo "Unsupported platform: $platform" >&2
    exit 1
    ;;
esac

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
