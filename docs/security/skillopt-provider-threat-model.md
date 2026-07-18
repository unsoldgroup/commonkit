# SkillOpt external provider threat model

Status: v1 security contract

## Boundary and assets

SkillOpt is an untrusted, version-pinned external computation provider. It is
not linked into CommonKit, does not share CommonKit's Rust process, and is not
allowed to own promotion. The protected assets are the Git-owned `SKILL.md`,
unselected repository content, raw sessions and credentials, held-out
evaluations, provider locks, candidate receipts, and active target state.

The provider receives a private, disposable staging directory containing only
the selected skill, explicitly reviewed tasks, approved redacted evidence, and
bounded configuration. It may return a JSON report, a staged manifest, and
`proposed_SKILL.md`. CommonKit treats all three as hostile input.

## Trust assumptions

- The CommonKit binary, repository policy, human approver, and OS account are
  trusted.
- PyPI, the downloaded wheel, its dependencies, the model backend, and the
  `skillopt-sleep` process are not trusted to preserve CommonKit invariants.
- A SHA-256 package lock establishes expected package identity; it does not
  make the package safe.
- Filesystem permissions provide same-host process isolation only. A uv
  environment is dependency isolation, not a complete sandbox. Use a container
  or OS sandbox when provider network/filesystem isolation is required.
- Validation establishes protocol compatibility, not candidate safety or
  correctness. Human review and CommonKit promotion gates remain mandatory.

## Threats and controls

| Threat | Required control | Failure behavior |
|---|---|---|
| Dependency substitution or version drift | Exact provider version, wheel digest, source, adapter contract, capabilities, marker, and runtime version probe | Unsupported provider; retain prior environment |
| CLI drift or argument injection | Invoke a fixed executable with an argument vector; allowlist backend and credential environment names; no shell | Reject invalid configuration |
| Unattended adoption | Adapter exposes only `skillopt-sleep run`; never passes `adopt`, `schedule`, `unschedule`, or `--auto-adopt`; adopted output must be empty | Reject candidate |
| Source mutation | Copy source into staging, compare staged input byte-for-byte after execution, and never pass a live source path | Reject candidate; live source remains untouched |
| Staging escape or symlink attack | Private `0700` staging, canonical containment check, regular-file/no-symlink checks, bounded reads | Reject output |
| Secret or transcript disclosure | Explicitly reviewed non-empty task corpus, secret scan, approved/redacted evidence only, cleared environment with credential allowlist | Reject input |
| Held-out evaluation leakage | Held-out identities stay in CommonKit contracts and are not copied into provider staging | Invalidate run if manifest/suite digests differ |
| Disk or memory denial of service | Timeout, task/edit bounds, 256 KiB proposal limit, 2 MiB stdout/stderr limits, 8 MiB task limit; kill child on live output overflow | Terminate provider and reject output |
| Malformed or ambiguous result | Strict JSON structs deny unknown fields; require accepted gate, reviewed tasks, consistent counts, staged manifest, and valid score range | Reject output |
| Malicious candidate content | Candidate validation, policy/regression gates, immutable artifacts, explicit human approval, digest-bound promotion plan | Candidate cannot promote |
| Upgrade changes behavior without breaking CLI | Disposable install, hashed requirements, synthetic contract fixture, source-mutation check, fixed behavioral replay, separate activation approval | Keep current lock active |
| Lock/report tampering | Canonical digest-bound upgrade plan/report and provider re-check during activation | Reject activation |

## Residual risks

- A provider with allowed model credentials can exfiltrate staged inputs through
  its configured backend. Operators must approve the exact evidence preview and
  choose a backend permitted by policy.
- A uv environment alone cannot prevent a malicious process from reading files
  accessible to the same OS user or opening arbitrary network connections.
  High-sensitivity runs require a container/OS sandbox with a read-only root,
  the staging directory as the only writable mount, and destination allowlists.
- Transitive dependencies are trusted only to the extent of the generated
  hash-locked requirements. Upgrade review must retain the full lock artifact.
- A compatible provider can produce a low-quality or adversarial skill. Human
  review, held-out evaluation, APM compilation, canary reconciliation, and
  rollback are separate defenses and must not be collapsed into provider check.

## Security invariants

1. Provider success can create only an immutable candidate; it cannot edit a
   Git source, provider lock, APM package, or active target.
2. Provider activation and candidate promotion are distinct human-approved
   actions bound to immutable digests.
3. Unknown versions, fields, capabilities, paths, or mutation evidence fail
   closed.
4. The previously pinned environment remains usable after an incompatible or
   failed upgrade.


