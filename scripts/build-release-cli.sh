#!/usr/bin/env bash
set -euo pipefail

target=${1:?target required}
output=${2:?output directory required}
mkdir -p "$output"

if [[ "$target" == universal-apple-darwin ]]; then
  for architecture in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --target "$architecture" \
      -p commonkit-cli -p commonkit-service --bin commonkit --bin commonkitd
  done
  for binary in commonkit commonkitd; do
    lipo -create \
      "target/aarch64-apple-darwin/release/$binary" \
      "target/x86_64-apple-darwin/release/$binary" \
      -output "$output/$binary-macos-universal"
    chmod 0755 "$output/$binary-macos-universal"
  done
elif [[ "$target" == x86_64-pc-windows-msvc ]]; then
  cargo build --release --locked --target "$target" \
    -p commonkit-cli -p commonkit-service --bin commonkit --bin commonkitd
  for binary in commonkit commonkitd; do
    cp "target/$target/release/$binary.exe" "$output/$binary-windows-x86_64.exe"
  done
else
  cargo build --release --locked --target "$target" \
    -p commonkit-cli -p commonkit-service --bin commonkit --bin commonkitd
  for binary in commonkit commonkitd; do
    cp "target/$target/release/$binary" "$output/$binary-linux-x86_64"
    chmod 0755 "$output/$binary-linux-x86_64"
  done
fi
