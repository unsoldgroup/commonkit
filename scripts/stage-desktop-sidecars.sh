#!/usr/bin/env bash
set -euo pipefail

target=${1:?target required}
artifacts=${2:?release artifact directory required}
destination=${3:-apps/desktop/src-tauri/binaries}
mkdir -p "$destination"

case "$target" in
  universal-apple-darwin)
    suffix=macos-universal
    extension=
    ;;
  x86_64-pc-windows-msvc)
    suffix=windows-x86_64
    extension=.exe
    ;;
  x86_64-unknown-linux-gnu)
    suffix=linux-x86_64
    extension=
    ;;
  *)
    echo "unsupported desktop sidecar target: $target" >&2
    exit 64
    ;;
esac

for binary in commonkit commonkitd commonkit-target-helper; do
  source_path="$artifacts/$binary-$suffix$extension"
  destination_path="$destination/$binary-$target$extension"
  test -f "$source_path"
  cp "$source_path" "$destination_path"
  chmod 0755 "$destination_path"
done
