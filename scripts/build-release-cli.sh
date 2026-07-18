#!/usr/bin/env bash
set -euo pipefail

target=${1:?target required}
output=${2:?output directory required}
mkdir -p "$output"

if [[ "$target" == universal-apple-darwin ]]; then
  cargo build --release --locked --target aarch64-apple-darwin -p commonkit-cli
  cargo build --release --locked --target x86_64-apple-darwin -p commonkit-cli
  lipo -create \
    target/aarch64-apple-darwin/release/commonkit \
    target/x86_64-apple-darwin/release/commonkit \
    -output "$output/commonkit-macos-universal"
  chmod 0755 "$output/commonkit-macos-universal"
elif [[ "$target" == x86_64-pc-windows-msvc ]]; then
  cargo build --release --locked --target "$target" -p commonkit-cli
  cp "target/$target/release/commonkit.exe" "$output/commonkit-windows-x86_64.exe"
else
  cargo build --release --locked --target "$target" -p commonkit-cli
  cp "target/$target/release/commonkit" "$output/commonkit-linux-x86_64"
  chmod 0755 "$output/commonkit-linux-x86_64"
fi
