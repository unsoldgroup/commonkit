#!/usr/bin/env bash
# Produce portable, secret-free evidence for CommonKit's eight headless v1 flows.
set -euo pipefail

evidence_root=${1:?usage: qualify-eight-flows.sh EVIDENCE_ROOT INSTALLED_BIN}
installed_bin=${2:?usage: qualify-eight-flows.sh EVIDENCE_ROOT INSTALLED_BIN}
commit=${COMMONKIT_QUALIFICATION_COMMIT:?set COMMONKIT_QUALIFICATION_COMMIT to the exact 40-character source commit}

case "$commit" in
  *[!0-9a-f]*|"") echo "COMMONKIT_QUALIFICATION_COMMIT must be a lowercase hexadecimal commit" >&2; exit 2 ;;
esac
test "${#commit}" -eq 40 || {
  echo "COMMONKIT_QUALIFICATION_COMMIT must contain exactly 40 characters" >&2
  exit 2
}

script_root=$(cd "$(dirname "$0")" && pwd -P)
installed_bin=$(cd "$installed_bin" && pwd -P)
mkdir -p "$evidence_root"
evidence_root=$(cd "$evidence_root" && pwd -P)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/commonkit-eight-flow.XXXXXX")
cleanup() {
  case "$scratch" in
    "${TMPDIR:-/tmp}"/commonkit-eight-flow.*) rm -rf -- "$scratch" ;;
  esac
}
trap cleanup EXIT

log_path="$evidence_root/installed-lifecycle.log"
result=failed
if "$script_root/installed-lifecycle.sh" "$scratch" "$installed_bin" >"$log_path" 2>&1; then
  result=passed
fi

COMMONKIT_EVIDENCE_ROOT="$evidence_root" \
COMMONKIT_EVIDENCE_BIN="$installed_bin" \
COMMONKIT_EVIDENCE_COMMIT="$commit" \
COMMONKIT_EVIDENCE_RESULT="$result" \
node <<'JS'
const { createHash } = require("node:crypto");
const { readFileSync, writeFileSync } = require("node:fs");
const { join } = require("node:path");
const { execFileSync } = require("node:child_process");

const root = process.env.COMMONKIT_EVIDENCE_ROOT;
const bin = process.env.COMMONKIT_EVIDENCE_BIN;
const suffix = process.platform === "win32" ? ".exe" : "";
const names = [
  "commonkit",
  "commonkitd",
  "commonkit-target-helper",
  "commonkit-snapshot-recovery-fixture",
];
const binaries = Object.fromEntries(names.map((name) => {
  const path = join(bin, `${name}${suffix}`);
  const digest = createHash("sha256").update(readFileSync(path)).digest("hex");
  return [name, `sha256:${digest}`];
}));
let kernel = "";
try {
  kernel = execFileSync("uname", ["-a"], { encoding: "utf8" }).trim();
} catch {
  kernel = `${process.platform} ${process.arch}`;
}
const evidence = {
  schemaVersion: 1,
  commit: process.env.COMMONKIT_EVIDENCE_COMMIT,
  platform: process.platform,
  architecture: process.arch,
  kernel,
  nodeVersion: process.version,
  result: process.env.COMMONKIT_EVIDENCE_RESULT,
  command: "scripts/installed-lifecycle.sh <isolated-scratch> <installed-bin>",
  binaries,
};
writeFileSync(join(root, "eight-flow-evidence.json"), `${JSON.stringify(evidence, null, 2)}\n`);
JS

test "$result" = passed
