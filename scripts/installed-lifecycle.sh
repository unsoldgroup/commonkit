#!/usr/bin/env bash
# End-to-end CI proof using only executables installed into the test prefix.
set -euo pipefail

scratch=${1:?usage: installed-lifecycle.sh SCRATCH}
installed_bin=${2:?usage: installed-lifecycle.sh SCRATCH INSTALLED_BIN}
real_git=$(command -v git)
mkdir -p "$scratch/source/layers" "$scratch/source/portable" "$scratch/tools" "$scratch/target"

commonkit="$installed_bin/commonkit"
commonkitd="$installed_bin/commonkitd"
recovery_fixture="$installed_bin/commonkit-snapshot-recovery-fixture"
if [[ "${RUNNER_OS:-}" == Windows ]]; then
  commonkit="$commonkit.exe"
  commonkitd="$commonkitd.exe"
  recovery_fixture="$recovery_fixture.exe"
fi
for executable in "$commonkit" "$commonkitd" "$recovery_fixture"; do test -x "$executable"; done

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

daemon_pid=
stop_daemon() {
  if [[ -n "$daemon_pid" ]]; then kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; fi
  daemon_pid=
}
start_daemon() {
  "$commonkitd" --port 0 --relay-port 0 > "$scratch/commonkitd.log" 2>&1 &
  daemon_pid=$!
  for _ in $(seq 1 150); do
    if "$commonkit" relay status >/dev/null 2>&1; then return; fi
    sleep 0.1
  done
  cat "$scratch/commonkitd.log" >&2
  return 1
}
trap stop_daemon EXIT
start_daemon

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

# Portable SSH-helper simulation: assert a helper can receive an argv-safe, stdin-only payload.
ssh_log="$scratch/ssh-helper.log"
printf '%s\n' '{"operation":"inspect","path":"portable/editor.conf"}' |
  node -e 'let b="";process.stdin.on("data",c=>b+=c);process.stdin.on("end",()=>require("fs").writeFileSync(process.argv[1],b))' "$ssh_log"
grep -q 'portable/editor.conf' "$ssh_log"
