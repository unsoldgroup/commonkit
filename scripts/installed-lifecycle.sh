#!/usr/bin/env bash
# End-to-end CI proof using only executables installed into the test prefix.
set -euo pipefail

scratch=${1:?usage: installed-lifecycle.sh SCRATCH}
installed_bin=${2:?usage: installed-lifecycle.sh SCRATCH INSTALLED_BIN}
real_git=$(command -v git)
mkdir -p "$scratch/source/layers" "$scratch/source/portable" "$scratch/tools" "$scratch/target"

commonkit="$installed_bin/commonkit"
commonkitd="$installed_bin/commonkitd"
target_helper="$installed_bin/commonkit-target-helper"
recovery_fixture="$installed_bin/commonkit-snapshot-recovery-fixture"
if [[ "${RUNNER_OS:-}" == Windows ]]; then
  commonkit="$commonkit.exe"
  commonkitd="$commonkitd.exe"
  target_helper="$target_helper.exe"
  recovery_fixture="$recovery_fixture.exe"
fi
for executable in "$commonkit" "$commonkitd" "$target_helper" "$recovery_fixture"; do test -x "$executable"; done

# Keep every installed smoke artifact and service definition inside the runner scratch root.
export HOME="$scratch/home"
export USERPROFILE="$scratch/home"
export XDG_CONFIG_HOME="$scratch/home/.config"
export XDG_DATA_HOME="$scratch/home/.local/share"
export XDG_CACHE_HOME="$scratch/home/.cache"
export COMMONKIT_SERVICE_BACKEND=process_fallback
export COMMONKIT_SERVICE_ROOT="$scratch/service"
mkdir -p "$HOME"

zero_digest=sha256:0000000000000000000000000000000000000000000000000000000000000000
cat > "$scratch/source/layers/public-base.json" <<JSON
{"schemaVersion":1,"id":"public-base","kind":"public_base","source":{"path":"layers/public-base.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"$zero_digest"},"spec":{}}
JSON
cat > "$scratch/source/layers/organization-policy.json" <<JSON
{"schemaVersion":1,"id":"organization-policy","kind":"organization_policy","source":{"path":"layers/organization-policy.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"$zero_digest"},"spec":{}}
JSON
printf 'installed lifecycle fixture\n' > "$scratch/source/portable/editor.conf"
cat > "$scratch/source/layers/personal.json" <<JSON
{"schemaVersion":1,"id":"personal","kind":"personal_kit","source":{"path":"layers/personal.json","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","contentDigest":"$zero_digest"},"spec":{"files":[{"path":"portable/editor.conf","source":"portable/editor.conf"}]}}
JSON
git -C "$scratch/source" init --quiet
git -C "$scratch/source" config user.email commonkit-ci@example.invalid
git -C "$scratch/source" config user.name CommonKit-CI
git -C "$scratch/source" remote add origin https://github.com/owner/installed-fixture.git
git -C "$scratch/source" add .
git -C "$scratch/source" commit --quiet -m fixture

cat > "$scratch/tools/gh.mjs" <<'JS'
import { cpSync } from "node:fs";
const args = process.argv.slice(2);
if (args[0] === "auth" && args[1] === "status") process.exit(0);
if (args[0] === "repo" && args[1] === "clone" && args[3]) {
  cpSync(process.env.COMMONKIT_FIXTURE_REPO, args[3], { recursive: true });
  process.exit(0);
}
process.exit(91);
JS
cat > "$scratch/tools/gh" <<'SH'
#!/usr/bin/env bash
exec node "$COMMONKIT_FIXTURE_TOOLS/gh.mjs" "$@"
SH
chmod +x "$scratch/tools/gh"
cat > "$scratch/tools/gh.cmd" <<'CMD'
@node "%COMMONKIT_FIXTURE_TOOLS%\gh.mjs" %*
CMD
cat > "$scratch/tools/git-proxy.mjs" <<'JS'
import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
const args = process.argv.slice(2);
appendFileSync(process.env.COMMONKIT_GIT_HELPER_LOG, `${JSON.stringify(args)}\n`);
const commandIndex = args[0] === "-C" ? 2 : 0;
const command = args[commandIndex];
const tail = args.slice(commandIndex + 1);
if (command === "fetch") process.exit(0);
if (command === "push") process.exit(0);
if (command === "rev-parse" && tail.some((value) => value.startsWith("refs/remotes/origin/"))) {
  const result = spawnSync(process.env.COMMONKIT_REAL_GIT, [...args.slice(0, commandIndex), "rev-parse", "HEAD"], { stdio: "inherit" });
  process.exit(result.status ?? 1);
}
if (command === "rev-list" && tail.includes("--left-right")) {
  process.stdout.write("0\t0\n");
  process.exit(0);
}
const result = spawnSync(process.env.COMMONKIT_REAL_GIT, args, { stdio: "inherit" });
process.exit(result.status ?? 1);
JS
cat > "$scratch/tools/git" <<'SH'
#!/usr/bin/env bash
exec node "$COMMONKIT_FIXTURE_TOOLS/git-proxy.mjs" "$@"
SH
chmod +x "$scratch/tools/git"
cat > "$scratch/tools/git.cmd" <<'CMD'
@node "%COMMONKIT_FIXTURE_TOOLS%\git-proxy.mjs" %*
CMD
export COMMONKIT_FIXTURE_REPO="$scratch/source"
export COMMONKIT_FIXTURE_TOOLS="$scratch/tools"
export COMMONKIT_REAL_GIT="$real_git"
export COMMONKIT_GIT_HELPER_LOG="$scratch/git-helper.log"
export PATH="$scratch/tools:$installed_bin:$PATH"

stop_daemon() {
  "$commonkit" daemon uninstall >/dev/null 2>&1 || true
}
start_daemon() {
  "$commonkit" daemon install >/dev/null
  "$commonkit" daemon start >/dev/null
  for _ in $(seq 1 150); do
    if "$commonkit" relay status >/dev/null 2>&1; then return; fi
    sleep 0.1
  done
  cat "$scratch/service/commonkitd.log" >&2
  return 1
}
trap stop_daemon EXIT
start_daemon
"$commonkit" daemon status | node -e 'let b="";process.stdin.on("data",c=>b+=c);process.stdin.on("end",()=>{const s=JSON.parse(b);if(!s.installed||!s.running)process.exit(1)})'

init_json="$scratch/init.json"
"$commonkit" init connect \
  --repository owner/installed-fixture \
  --kit-directory "$scratch/kit" \
  --loadout personal \
  --target local \
  --publish-registration \
  --target-root "$scratch/target" > "$init_json"
headless=$(node -e 'const fs=require("fs");process.stdout.write(JSON.parse(fs.readFileSync(process.argv[1])).headlessConfig)' "$init_json")

# Add a real encrypted mutable-state domain to the onboarding-generated provider fixture.
snapshot_root="$scratch/snapshots"
database="$snapshot_root/database.bin"
key_file="$scratch/snapshot.key"
mkdir -p "$snapshot_root"
printf 'database before snapshot' > "$database"
node -e 'require("fs").writeFileSync(process.argv[1], Buffer.alloc(32,47))' "$key_file"
node - "$headless" "$snapshot_root" "$database" "$key_file" "$commonkit" "$scratch/kit" <<'JS'
const fs = require("fs");
const [configPath, root, database, key, executable, portableState] = process.argv.slice(2);
const config = JSON.parse(fs.readFileSync(configPath));
const lifecycle = { executable, args: ["status"] };
config.snapshots = {
  root,
  portableState,
  keyReference: `file://${key}`,
  objectStore: { type: "local" },
  databases: [{ id: "context-mode", path: database, targetId: "local", format: "file",
    lifecycle: { stop: lifecycle, start: lifecycle } }],
};
fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
JS

# The same authenticated atomic reload used by Desktop must adopt onboarding
# state without replacing either an owned or service-manager-owned process.
"$commonkit" daemon reload-domains --confirmed >/dev/null

"$commonkit" status >/dev/null
"$commonkit" compose >/dev/null
plan_json="$scratch/plan.json"
"$commonkit" sync --confirmed > "$plan_json"
plan_id=$(node -e 'const fs=require("fs");const p=JSON.parse(fs.readFileSync(process.argv[1]));process.stdout.write(p.planId ?? p.id)' "$plan_json")
"$commonkit" diff "$plan_id" >/dev/null
"$commonkit" apply "$plan_id" --confirmed > "$scratch/apply.json"
for _ in $(seq 1 150); do
  if "$commonkit" verify >/dev/null 2>&1; then verified=1; break; fi
  sleep 0.1
done
test "${verified:-}" = 1
cmp "$scratch/source/portable/editor.conf" "$scratch/target/portable/editor.conf"

snapshot_json="$scratch/snapshot.json"
"$commonkit" snapshots create context-mode --confirmed > "$snapshot_json"
snapshot_id=$(node -e 'const fs=require("fs");process.stdout.write(JSON.parse(fs.readFileSync(process.argv[1])).snapshotId)' "$snapshot_json")
printf 'database changed after snapshot' > "$database"
"$commonkit" snapshots restore "$snapshot_id" --confirmed >/dev/null
test "$(cat "$database")" = 'database before snapshot'

# The installed helper stops after the atomic swap. A new daemon process discovers the
# authenticated transaction and rolls it back without re-running a provider or secret lookup.
stop_daemon
"$recovery_fixture" interrupt "$snapshot_root"
test "$(cat "$database")" = 'staged restored database'
start_daemon
for _ in $(seq 1 100); do
  [[ "$(cat "$database")" == 'pre-interruption database' ]] && break
  sleep 0.1
done
test "$(cat "$database")" = 'pre-interruption database'
stop_daemon
"$recovery_fixture" recover "$snapshot_root"

# Exercise the actual installed, typed stdin-only ssh remote helper protocol.
helper_target="$scratch/helper-target"
helper_state="$scratch/helper-state"
mkdir -p "$HOME/.config/commonkit" "$helper_target"
node - "$HOME/.config/commonkit/target-helper.json" "$helper_state" "$helper_target" <<'JS'
const fs = require("fs");
fs.writeFileSync(process.argv[2], JSON.stringify({
  stateRoot: process.argv[3],
  roots: [{ id: "home", path: process.argv[4], access: "read_write" }],
}));
JS
printf '%s' '{"operation":"write_file","root_id":"home","path":"portable/helper-proof.txt","content":[104,101,108,112,101,114,10]}' |
  "$target_helper" --stdio-v1 > "$scratch/helper-response.json"
node -e 'const r=require(process.argv[1]);if(r.result!=="applied")process.exit(1)' "$scratch/helper-response.json"
test "$(cat "$helper_target/portable/helper-proof.txt")" = helper

stop_daemon
"$commonkit" daemon status | node -e 'let b="";process.stdin.on("data",c=>b+=c);process.stdin.on("end",()=>{const s=JSON.parse(b);if(s.installed||s.running)process.exit(1)})'
