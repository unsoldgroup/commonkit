#!/usr/bin/env bash
set -euo pipefail

runner=${1:?runner OS required}
output=${2:?output directory required}
bundle_root=apps/desktop/src-tauri/target
mkdir -p "$output"

case "$runner" in
  macOS) extensions='dmg|app.tar.gz|app.tar.gz.sig' ;;
  Windows) extensions='exe|nsis.zip|nsis.zip.sig' ;;
  Linux) extensions='AppImage|AppImage.sig|deb' ;;
  *) echo "unsupported release runner: $runner" >&2; exit 1 ;;
esac

found=0
while IFS= read -r -d '' file; do
  case "$file" in
    *.dmg|*.app.tar.gz|*.app.tar.gz.sig|*-setup.exe|*.nsis.zip|*.nsis.zip.sig|*.AppImage|*.AppImage.sig|*.deb)
      cp "$file" "$output/"
      found=1
      ;;
  esac
done < <(find "$bundle_root" -type f -path '*/release/bundle/*' -print0)

if [[ "$found" != 1 ]]; then
  echo "no $runner desktop release payloads found under $bundle_root ($extensions)" >&2
  exit 1
fi
