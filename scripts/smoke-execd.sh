#!/usr/bin/env bash
# Submit one declared task at one revision and wait for it to reach a terminal
# state. Run this ON the target, or anywhere that can reach the execution API.
#
#   ./smoke-execd.sh ci-pnpm-test 9cd2e5d29e599342264ff92f8760e61666325132
#
# The manifest is the committed declaration with its revision replaced, which is
# exactly what the webhook does, so a green run here is evidence the webhook
# path will run the same thing.
set -euo pipefail

task=${1:?task ID required}
revision=${2:?revision required}
api=${COMMONKIT_EXECD_API:-http://127.0.0.1:7341}
tasks_file=${COMMONKIT_EXECD_TASKS:-/etc/commonkit/execution-context.json}
timeout_seconds=${COMMONKIT_EXECD_SMOKE_TIMEOUT:-3600}

token=${COMMONKIT_EXECD_CLIENT_TOKEN:-$(bws secret list |
  jq -r '.[] | select(.key=="commonkit/EXECD_CLIENT_TOKEN") | .value' | head -1)}
if [[ -z "$token" ]]; then
  echo "no client token: set COMMONKIT_EXECD_CLIENT_TOKEN or store commonkit/EXECD_CLIENT_TOKEN" >&2
  exit 1
fi

tag=${COMMONKIT_EXECD_SMOKE_TAG:-}
request=$(jq -c --arg task "$task" --arg revision "$revision" --arg tag "$tag" \
  '{manifest: (.tasks[$task] | .repositoryRevision = $revision),
    idempotencyKey: ("smoke:" + $task + ":" + $revision + $tag)}' "$tasks_file")
if [[ "$request" == *"null"* && "$(jq -r --arg task "$task" '.tasks | has($task)' "$tasks_file")" != true ]]; then
  echo "no declared task named $task in $tasks_file" >&2
  exit 1
fi

submitted=$(curl -sS -X POST -H "Authorization: Bearer $token" \
  -H 'content-type: application/json' --data "$request" "$api/execution/v1/jobs")
job=$(jq -r '.jobId // empty' <<<"$submitted")
if [[ -z "$job" ]]; then
  echo "submit failed: $submitted" >&2
  exit 1
fi
echo "job $job submitted for $task at $revision"

deadline=$((SECONDS + timeout_seconds))
state=
while ((SECONDS < deadline)); do
  snapshot=$(curl -sS -H "Authorization: Bearer $token" "$api/execution/v1/jobs/$job")
  state=$(jq -r '.attempt.state // empty' <<<"$snapshot")
  case "$state" in
  succeeded | failed | cancelled | canceled | timedOut | timed_out)
    jq -c '{state: .attempt.state, exitCode: .receipt.exitCode, artifacts: (.receipt.artifactIds | length)}' <<<"$snapshot"
    [[ "$state" == succeeded ]] || exit 1
    exit 0
    ;;
  esac
  sleep 5
done
echo "job $job did not finish within ${timeout_seconds}s (last state: ${state:-unknown})" >&2
exit 1
