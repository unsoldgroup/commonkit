#!/usr/bin/env bash
set -euo pipefail
root=${1:?artifact directory required}

(
  cd "$root"
  find . -maxdepth 1 -type f ! -name SHA256SUMS ! -name SHA256SUMS.sig -print0 \
    | sort -z \
    | xargs -0 shasum -a 256 \
    | sed 's#  \./#  #' > SHA256SUMS
)
