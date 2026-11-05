# Linear work graph

The graph is a private, read-only focus dashboard for Linear issues. It groups issues into
stable topical zones, renders parent and relationship edges, and adds only high-confidence
Codex suggestions as dashed edges. The UI has Focus and Universe views, filters, an accessible
issue list, editable focus brief, and a primary-topic correction control.

## Local development

```sh
pnpm install
pnpm --filter @commonkit/linear-graph-protocol test
pnpm --filter @commonkit/linear-graph-hub typecheck
pnpm --filter @commonkit/linear-graph-web build
LINEAR_API_TOKEN=... CODEX_API_KEY=... LINEAR_GRAPH_ACTION_TOKEN=... \
  pnpm --filter @commonkit/linear-graph-hub start
```

The hub listens on `127.0.0.1:8790` by default. `POST /api/analysis-runs` is protected by the
action token; Linear and Codex credentials are never exposed to the browser. The scheduled run
is daily at 07:00 Europe/Madrid by default (override with `LINEAR_GRAPH_TIMEZONE`), with a startup
run when no completed snapshot exists.

## Structure, ranking and freshness (USG-383)

Linear is the primary structure. Every daily run syncs Linear, assigns zones from Linear's own
links (issues joined by parent, blocks or related share the zone that wins a vote of manual
overrides and keyword inference; components above 40 issues keep per-issue zones), ranks work
deterministically, and publishes that snapshot before Codex runs. Codex is optional enrichment
(brief, dashed semantic edges, triage); its failure is recorded on the run as
`Codex enrichment skipped: …` and never freezes the graph.

Ranking order: issues named in Al's manual focus brief (in brief order), then unblocked before
blocked, Linear priority (urgent first, none last), number of open issues it unblocks, in progress
before todo before backlog, due date, most recently updated. A generated brief never pins.

Read endpoints (tailnet-only through Caddy; FR consumers use `http://127.0.0.1:$LINEAR_GRAPH_PORT`):

- `GET /api/freshness` returns `{ syncedAt, generatedAt, ageSeconds, staleAfterHours: 36, stale, lastRun }`.
  `stale` is true when there is no snapshot or it is older than 36 hours. `/health` carries the
  same `ageSeconds` and `stale`.
- `GET /api/next-up?limit=10` (1 to 50) returns the freshness fields plus `items`:
  `{ ref, short_description, why_now, next_action, priority, pinned, zone, relations: [{ ref, kind, short_description }] }`.
  `priority` is Linear's number (0 none, 1 urgent … 4 low). `kind` is one of `blocked_by`,
  `blocks`, `parent`, `child`, `related`, `duplicate`; only open related issues are listed (max 6).
  Descriptions are issue titles with emails and quote ids replaced; they are not otherwise
  screened for customer names.

On open, the store marks any analysis run still `running` as failed ("Interrupted"): those were
cut off by a hub exit. A restart within an hour of the last attempt does not re-sync Linear, and
Linear rate limits (429, or 400 `RATELIMITED`) back off and retry.

### Bounded daily analysis (USG-399)

Codex reviews every active issue in sequential batches of at most 25 issues and
256,000 UTF-8 prompt bytes, including JSON escaping and instructions. Descriptions
are redacted but not truncated. Each batch receives all incident active-issue
relations and description-free metadata for endpoints outside that batch. New
semantic suggestions are therefore limited to issues visible in that batch;
this is not an exhaustive search of every possible pair across the universe.
No issue is selected or excluded based on a top-N cutoff. An individual issue
plus its relationship context that cannot fit fails planning explicitly, before
any provider call. It must be resolved before claiming complete enrichment.

Each batch must return exactly one valid assignment and triage decision per
issue. Coverage errors report both collections' expected/actual row counts and
missing, duplicate and foreign ID counts. Duplicate counts mean distinct IDs
appearing more than once. Each category includes at most two ID samples: UUIDs
are shown directly and other strings are SHA-256 fingerprints, never arbitrary
provider text. Reference failures identify the field and array index, total
invalid reference count, allowed domain, and at most two UUID/fingerprint
samples. They never include arbitrary model text.

The provider's JSON schema derives from the runtime schema. Assignment,
triage and recommendation subjects use primary-batch ID enums. Semantic edge
endpoints and evidence IDs use primary-plus-related enums when that set has at
most 100 IDs. Each enum also requires IDs of at most 100 characters. Larger
sets retain string schemas plus strict runtime membership validation; context
is never removed to fit an enum. At most 475 ID enum entries and 47,500 ID
characters are repeated across the seven reference fields, leaving headroom
under OpenAI's [schema limits](https://developers.openai.com/api/docs/guides/structured-outputs):
1,000 aggregate enum values and 120,000 aggregate string characters. Each enum
stays below the 250-entry threshold for the additional 15,000-character limit.
The prompt explicitly distinguishes IDs from human identifiers and restricts
recommendation subjects to primary issues. Every batch brief participates in
bounded hierarchical synthesis. Existing limits of 1,000 semantic suggestions
and 50 Codex recommendations apply to the combined result; these suggestion
limits do not cap assignment or triage coverage. The published ranking remains
the deterministic Linear ranking, with only the manual brief supplying pins.

`lastRun.enrichment` on `/api/freshness` and `enrichment` on
`/api/analysis-runs/latest` report status (`running`, `completed`, `failed`), phase
(`planning`, `analyzing`, `summarizing`), `totalIssues`, `analyzedIssues`,
`totalBatches` and `completedBatches`. Analyzed counts describe validated batches,
not published partial results. No new enrichment is published until all batches
and synthesis succeed. The outer run can be `completed` because Linear synced
while enrichment is `failed`; its error explains the failure. Interrupted runs
retain their counts and become failed on restart. Old records without coverage
remain readable, but do not prove complete enrichment.

Fallback briefs regenerate their active count and snapshot date on every sync.
A retained Codex brief keeps its original generation/snapshot date and becomes
stale when enrichment fails. Manual brief text remains untouched. Failed
enrichment is also recorded on the brief. The model, provider, login, sandbox,
action-token policy, frame feed and daily schedule are unchanged. The existing
`LINEAR_GRAPH_CODEX_TIMEOUT_MS` (default 600,000) applies to each bounded provider
request. A structurally valid response with missing, duplicate or foreign
assignment/triage IDs, or an invalid reference, shares **one correction request
per batch**. It receives the same full payload plus the safe diagnostic and a request for
a complete replacement. Both collections and all references must pass the
original validation; no rows from the invalid response are kept. Counts advance
only after validation. A second invalid response fails enrichment without
publishing partial results, even if the second failure is a different kind.
Transport, timeout, schema and synthesis
failures are not retried. If correction instructions exceed the existing prompt
budget, the batch fails explicitly without truncating its context.

Batches remain serial: at most twice the planned batch count in analysis calls,
plus the existing synthesis calls. For 87 batches, that is at most 174 analysis
calls, with at most 87 additional per-request timeouts. Large universes can take
substantially longer than one request; there is no automatic whole-run retry.

The 2026-11-05 11:37 sync failed batch 4/87 after 75/2160 active issues on
`f4a03309bf9aaf1e009cd46b442a78b249e2812f`. Its response was not retained, so the
original missing/duplicate/foreign breakdown is unknown. Three targeted
replays of snapshot batch 4 using the existing provider/model/subscription
settings each returned exactly 25 assignments and 25 triage decisions with
zero coverage discrepancies. The snapshot reconstructed 87 batches; batch 4
had six related endpoints, a 49,595-byte prompt, and no descriptions at the
snapshot's 5,000-character storage limit. These are successful replays, not a
reproduction of the original bad response. Intermittent output drift is an
inference. Synthetic seam regressions cover each discrepancy independently;
real-provider correction after an actual bad response and complete-universe
success still require validation after parent review and authorized rollout.

The subsequent 12:09:25–12:12:20 run
`6c150a9a-890b-4f33-aafe-45b407d918d3` passed schema/coverage validation but
failed reference validation in batch 1/87 after 0/2162 issues. The offending
reference was not retained, so its exact mismatch remains unknown. The
reference correction and bounded enum constraints address the observed failure
class without claiming to reconstruct that response.

At 12:20:29–12:23:14 UTC, a read-only replay of batch 1 from the
12:09:58 snapshot passed with the bounded schema: 25 primary issues, six
related endpoints, a 29,955-byte prompt, and no descriptions at the snapshot
storage limit. The existing subscription/default model returned 25 assignments,
25 triage decisions, nine semantic edges and five recommendations in one call.
**No correction was exercised by this provider replay.** Synthetic regressions
verify reference correction and mixed coverage/reference exhaustion. This is
targeted acceptance evidence, not full-universe success; no service restart,
production run, deployment or publication occurred during the replay.

### USG-399 rollout handoff

No production rollout or Linear closure is part of this patch. Once separately
authorized:

1. Integrate the reviewed USG-399 patch into the deployment's tracked upstream.
   `/root/commonkit` currently uses `linear-graph-deploy` tracking `origin/main`.
   Preserve unrelated `.agents/` work; do not run the updater against uncommitted
   implementation files. It resets to upstream and will not deploy a local-only
   commit.
2. Run `pnpm run test:linear-graph` and `pnpm run typecheck:linear-graph` on the VPS.
   Run `bash /root/commonkit/apps/linear-graph/deploy/update-vps.sh` only after
   authorization and integration. Its protocol/web build and health probe are
   required; the hub runs TypeScript directly. No credential or model changes
   are needed. Old analysis-run JSON needs no database migration. New run records
   add `enrichment`; an older hub's strict schema cannot read that property.
   Prefer fix-forward recovery. A code rollback needs a compatible reader or an
   explicitly planned database restore/projection, not a blind service restart.
3. Check `/health`, then allow the daily run or explicitly authorize one normal
   authenticated `POST /api/analysis-runs`. Do not bypass the existing action
   authorization. Inspect `/api/freshness` until the run finishes.
4. Require fresh `syncedAt`, null run error, `enrichment.status=completed`,
   `analyzedIssues=totalIssues` and `completedBatches=totalBatches`. For a generated
   brief, `/api/focus-brief` must be ready with the new `snapshotAt`; a manual
   brief remains manual. A healthy HTTP response alone does not prove analysis.
5. If planning, a batch, validation or synthesis fails, leave the issue open and
   record its phase/counts/error. The fresh Linear graph remains usable. Do not
   declare success, drop remaining issues, change provider/auth, or raise limits
   to conceal the failure. Live provider acceptance and runtime are rollout
   verification, not covered by the mocked regression suite.

## VPS deployment

On the VPS, install Bun, pnpm, Caddy, and acme.sh first. Then run the installer with secrets
provided from the existing secrets manager (do not put them in the repository):

```sh
LINEAR_API_TOKEN='…' LINEAR_GRAPH_ACTION_TOKEN='…' \
CF_Token='…' LINEAR_GRAPH_TAILSCALE_IP='<tailnet-ip>' \
  bash apps/linear-graph/deploy/install-vps.sh
```

The installer writes a mode-600 environment file, a user systemd service, and a Caddy site
restricted to Tailscale source addresses. Create a DNS-only `graph.unsold.cloud` A record for that address,
then verify from a tailnet device:

```sh
curl -fsS https://graph.unsold.cloud/health
systemctl --user status linear-graph.service
```

### Updating a running VPS hub

Every deploy after the first is one command on the VPS:

```sh
bash apps/linear-graph/deploy/update-vps.sh
```

It fetches, resets to the tracked upstream, rebuilds `protocol` and `web` only when those
changed (the hub runs TypeScript directly under Bun), restarts the service, and polls
`/health` on the configured port before reporting the deployed commit. It refuses to run if
the checkout has uncommitted changes to tracked files, or is not on a branch with an upstream.

The deploy checkout must be able to see the branch it deploys. A clone created with a
single-branch refspec can only fetch `main`, which is how `/root/commonkit` ended up serving
hand-copied, untracked files until EXP-2820. Fix the refspec, then track the branch:

```sh
git -C ~/commonkit config remote.origin.fetch "+refs/heads/*:refs/remotes/origin/*"
git -C ~/commonkit fetch origin
git -C ~/commonkit checkout -B linear-graph-deploy origin/<deploy-branch>
```

Codex uses the existing subscription login copied into the graph’s private `CODEX_HOME` by default. Set `LINEAR_GRAPH_CODEX_AUTH=api-key` together with
`CODEX_API_KEY` only when API-key mode is intended; subscription mode never forwards that variable
to the Codex child process. The installer copies only the existing `~/.codex/auth.json` once; future
device logins stay inside the graph’s private home. Override `LINEAR_GRAPH_CODEX_HOME` only when you
intentionally want a separate account. The authenticated VPS user can start device login from the graph with
`POST /api/codex/login` and inspect redacted state at `GET /api/codex/status`. `LINEAR_GRAPH_REPO_MAP`
is optional JSON such as
`{"teams":{"UNS":"unsold"},"projects":{"project-id":"repo-name"}}`; it provides bounded
repository context without sending repository source code to Codex. Codex runs as a read-only,
non-interactive child process with a strict output schema and no tools or network access.
