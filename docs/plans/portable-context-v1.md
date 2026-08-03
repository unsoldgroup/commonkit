# Portable context v1 implementation plan

Tracks USG-60 and the decisions in `CONTEXT.md`, ADR 0010, and ADR 0011.

## Outcome

An organization can register projects and curated documentation, invite a user
through GitHub-backed membership, and let that user create an independently
owned encrypted profile. Authorized agents receive deterministic task briefs
and retrieve additional sections through the device relay. Users can inspect,
revoke, synchronize, resolve conflicts, and delete active CommonKit-controlled
personal context without granting the organization recovery access.

## Release boundary

Included:

- GitHub-backed organization/project registration and roles.
- Organization/project document ingestion from Git, files, rich uploads, and
  pinned web snapshots.
- User-owned partially encrypted personal-context repositories.
- Adaptive work-profile interview with structured confirmation.
- Encrypted multi-device revisions, recovery, conflicts, and bounded deletion.
- Context grants, agent trust classes, deterministic briefs, retrieval, and
  audience-specific receipts.
- Device-relay projection and retrieval tools.

Deferred:

- Generalized upstream contribution submission and licensing UI.
- Cloudflare MCP server portal provisioning.
- CRDT-backed editing, broad SaaS document connectors, and hosted CommonKit
  accounts.

## Domain contracts

Add versioned schemas under `schemas/` and matching Rust types in
`crates/commonkit-contracts` for:

- `OrganizationRegistration`, `ProjectRegistration`, `AgentTrustClass`.
- `DocumentationArtifact`, `DocumentationRevisionCandidate`, `ContextSection`,
  `ProjectContextMap`, `DocumentationConflict`.
- `ProfileSchema`, `ProfileExtension`, `EncryptedProfileDocument`,
  `ProfileRevision`, `ProfileConflict`, `RecoveryRecipient`.
- `ContextDescriptor`, `ContextGrant`, `ContextGrantRevocation`,
  `DeletionTombstone`, `TaskContextRequest`, `TaskContextBrief`,
  `ContextReceipt`, chained `ContextReceiptEvent`, and audience-specific
  `ReceiptView`.
- `Principal`, `OrganizationMembership`, `RoleBinding`,
  `RepositoryAccessEvidence`, `RuntimeAttestation`, `RuntimeSessionPrincipal`,
  `OrganizationOnboardingRecord`, `ProfileDraft`, and
  `ProfileRevisionProposal`.
- `GitHubOnboardingAttempt` and `GitHubOrphanReceipt`, binding repository role,
  immutable account/repository IDs, confirmation digest, idempotency key,
  current step, recoverable failure, registration ID, and terminal state.

Every portable record has a schema version, stable opaque ID, owner and scope,
content/revision hash, creation time, and provenance. Personal values, key
material, secret values, and decrypted context are forbidden in plans,
receipts, diagnostics, and logs.

## Implementation sequence

### 1. Contracts and policy invariants

Define canonical serialization, redaction, compatibility, ownership, and
authorization contracts first. Extend organization policy with project-editor
roles and agent trust classes. Add closed-schema validation so unknown personal
fields fail before persistence or publication.

Verification: schema snapshots, canonical round trips, redaction tests,
organization-floor tests, and fixtures proving personal values never appear in
receipts or diagnostics.

### 2. Personal repository cryptography

Add a provider that stages partially encrypted structured documents. Canonical
field names and shape remain plaintext; every value is authenticated
ciphertext. Use interoperable age recipients through a narrow Rust boundary.
Use a random data-encryption key per profile revision and bind encrypted inner
payloads to profile, schema, field, revision, parent hashes, and recipient-set
digest. Wrap data keys to device, offline recovery, and password-manager-
provisioned recovery recipients. Keys never enter Git,
CommonKit portable metadata, or organization storage.

Verification: known-answer encryption tests, tamper rejection, wrong-recipient
failure, two-path recovery, recipient rotation, and scans proving fixtures and
diagnostics contain no plaintext.

### 3. GitHub-backed onboarding

Replace direct GitHub mutation in onboarding with a shared inspect/plan/apply
operation adapter, using GitHub CLI only as its replaceable transport. Register organization
membership, create/connect a user-owned private repository with explicit
publication consent, and register project repositories. The organization can
require a personal repository but cannot administer it. Initialization creates
a preview and never applies or publishes silently.

Verification: mocked GitHub boundary tests for solo and multi-user
organizations, ownership rejection, stale authorization, and no-side-effect
preview behavior.

### 4. Documentation ingestion and project discovery

Implement `probe -> extract -> normalize -> validate -> receipt` as a desired-
state provider. Markdown is native. Rich files use an isolated, exact-version
Docling adapter. Web sources become pinned snapshots. Preserve originals and
derive editor-reviewable sections. Scan existing projects to propose context
maps. Changed sources create inactive candidates; only project editors can
publish.

Verification: deterministic Markdown fixtures, pinned HTML fixtures, isolated
extractor contract fixtures, provenance/hash linkage, duplicate/conflict
detection, and role-gated publication tests.

### 5. Profile interview and revision UI

Add the CommonKit core work-profile schema and namespaced organization
extensions. Use an adaptive one-question-at-a-time interview that produces a
structured draft; no transcript or inference is persisted. The user confirms
every stored value before encryption. Add agent revision proposals and
event-driven plus six-month review reminders.

Verification: branching interview tests, optional-field behavior, extension
source labels, draft confirmation, cancellation/no-write behavior, and UI tests
proving plaintext never crosses the Tauri command boundary except for the
authorized local operation that encrypts it.

### 6. Revision sync, recovery, and deletion

Sync immutable encrypted revisions through the personal repository. Merge
different-field changes; preserve same-field conflicts for local user
resolution. Implement recovery-recipient onboarding and rotation. Deletion
emits signed grant revocations and tombstones, removes active ciphertext and
controlled caches/envelopes, and prevents future CommonKit injection after
observation. It does not claim erasure of historical Git ciphertext decryptable
with a retained recovery identity.

Verification: two-device offline histories, conflict fixtures, lost-device
recovery, revoked-recipient denial, tombstone replay/idempotency, old-device
reconnection, and opaque-only deletion receipts.

### 7. Context resolver, grants, and receipts

Build the deterministic pipeline: enumerate candidates, authorize, apply scope
and precedence, enforce compiled policy, rank eligible context, cost tokens,
construct the initial brief, and issue a receipt. Existing project-and-purpose
grants apply across sessions to qualifying agent trust classes. Missing grants
expose descriptors only and create just-in-time requests. Mandatory explanatory
overflow becomes retrievable descriptors; affected operations remain blocked
until non-machine-enforceable procedures are retrieved.

Verification: table-driven policy matrices, context-budget boundaries,
deterministic replay, ranker substitution, conflict inclusion, grant
revocation, trust-class eligibility, and user/org receipt projections.

### 8. Device relay retrieval and end-to-end release gate

Project desired context into the persistent device relay and add bounded tools
for descriptor search, section retrieval, access request, receipt inspection,
and profile-revision proposals. Keep stdio/local/private operations local. Add
an installed lifecycle fixture covering organization registration through
deletion on macOS, Linux, and Windows; each supported platform requires native
recorded evidence.

Verification: Rust MCP compatibility tests, relay restart/recovery, auth and
size bounds, offline local retrieval, degraded remote behavior, installed
lifecycle, complete local Rust checks, `rtest` JavaScript checks, desktop tests,
and typecheck. GitHub-hosted workflows are not evidence.

## Rollout

1. Land schemas and read-only inspection behind an experimental capability
   flag.
2. Enable personal repository creation for test users; retain export and
   complete bounded deletion and recovery disclosures before accepting real profile values.
3. Enable documentation candidates and project-editor publication.
4. Enable context injection only after receipt/redaction and grant tests pass.
5. Record native macOS, Linux, and Windows lifecycle evidence before declaring
   support.

No migration of existing agent instructions is automatic. Existing APM-backed
context remains active until a reviewed project context map explicitly admits
or references it.

## Cold-audit contract clarifications

These constraints resolve the first independent audit and are normative for
the tickets above.

### Record integrity and compatibility

- Portable signed records use a canonical envelope containing domain tag,
  record kind/version, opaque ID, owner/scope, generation, parent hashes,
  payload hash, signer ID, created-at time, and signature.
- Profile revisions, device authorizations, grants, revocations, and tombstones
  form authenticated parent-linked histories. A valid tombstone dominates all
  earlier revisions in its deletion generation. Re-creation uses a new opaque
  profile-item ID.
- Top-level source records carry ownership and provenance metadata; projections
  and views reference their source record rather than duplicating universal
  metadata. Hash definitions exclude the hash/signature fields themselves and
  use record-kind domain separation.
- Each schema kind has explicit version dispatch, prior-version read fixtures,
  upgrade functions, newer-version quarantine, and downgrade refusal. Schema
  snapshots remain change detectors, not compatibility evidence by themselves.
- Contracts live in a dedicated `portable_context` module and one schema
  registry drives generation, checked-in schemas, snapshots, and catalog
  publication.

### Identity, repositories, and authorization evidence

- GitHub identity evidence binds provider, immutable account/node ID, login,
  repository node ID/owner/visibility/permission, checked-at, expiry, and
  revocation status to an authenticated principal.
- Runtime-session principals bind user, organization, project, purpose, agent
  runtime identity, verified trust-class evidence, issue/expiry times, and
  revocation generation. Every resolution and retrieval call re-authorizes the
  session principal.
- Repository roles are explicit: organization kit, project context, and
  personal context. Solo mode maps organization kit ownership to the user's
  account without collapsing these roles.
- GitHub onboarding is a resumable inspect/plan/apply state machine. If remote
  repository creation succeeds before later registration fails, CommonKit
  reports a recoverable user-owned orphan and never deletes it automatically.
- The daemon owns registrations, authorization evidence, refresh state, and
  transactional publication; CLI and desktop share the same Rust adapter.

### Encryption and recovery

- A dedicated personal-context crypto/store module owns canonical structured
  traversal, secret types, zeroization, and envelope transforms.
- Every encrypted leaf has a canonical inner payload binding profile ID,
  schema/namespace version, field ID, revision ID, parents, operation, and
  recipient-set digest. The outer document stores an authenticated ciphertext
  envelope with bounded sizes and no plaintext value.
- Random per-revision data keys are wrapped to age recipients. Cryptographic
  primitives and wrapper transformations belong to the crypto ticket; device
  enrollment, rotation workflows, and synchronization belong to the sync
  ticket.
- `commonkit-personal-context` owns envelope grammar, secret types, crypto
  transformations, and provider-neutral recovery-material interfaces only.
  `commonkit-adapters` owns provider-specific consent-bound
  create/read/update/delete operations; the daemon owns plan-confirm-apply,
  durable state, and redacted receipts. Password-manager version-history limits
  are included in recovery and deletion disclosures.
- Secret values use non-serializable redacted types, private temporary storage,
  zeroization, and tested panic/error paths; generic JSON values never carry
  decrypted profile content.

### Profile history and deletion

- Profile history is a signed revision DAG with canonical parent ordering and
  field-level `set`/`delete` operations. Different-field operations merge;
  concurrent operations on one field create a conflict record. Resolution is a
  new signed revision referencing both parents.
- A user-controlled profile signing authority certifies device signing keys;
  its recovery material follows the same offline and password-manager recovery
  paths. Device certificates and revocations are signed and generation-bound.
- Deletion and grant-revocation records are signed, monotonic, replay-safe, and
  processed before decryption or injection. Offline use is bounded by an
  authorization lease; reconnect must refresh revocation state before further
  access.
- The deletion guarantee is bounded: after observing a valid tombstone,
  CommonKit removes active controlled state and prevents future controlled
  injection. It does not promise erasure of historical Git ciphertext when an
  actor retains a formerly valid recovery identity.

### Profile field catalog

The v1 core catalog is closed and every field is optional. Fields are strings
or string lists unless noted:

- `identity.display_name`, `identity.pronouns`, `identity.role`,
  `identity.responsibilities[]`, `identity.expertise[]`.
- `communication.tone`, `communication.detail_level`,
  `communication.preferred_channels[]`, `communication.async_expectations`.
- `collaboration.working_style`, `collaboration.handoff_preferences`,
  `collaboration.meeting_preferences`, `collaboration.escalation_preferences`.
- `feedback.preferred_style`, `feedback.correction_preferences`,
  `feedback.praise_preferences`.
- `decisions.decision_style`, `decisions.evidence_expectations`,
  `decisions.risk_tolerance`, `decisions.approval_thresholds`.
- `planning.planning_horizon`, `planning.task_breakdown`,
  `planning.status_update_preferences`, `planning.definition_of_done`.
- `technical.languages[]`, `technical.frameworks[]`,
  `technical.package_managers[]`, `technical.tool_preferences[]`,
  `technical.code_review_preferences`.
- `accessibility.requested_accommodations[]` and
  `accessibility.presentation_preferences[]`; these capture requested working
  adjustments only, never diagnoses or medical history.
- `boundaries.do_not_do[]`, `boundaries.ask_before[]`,
  `boundaries.availability_notes`.
- `agents.response_style`, `agents.autonomy_preferences`,
  `agents.clarification_preferences`, `agents.memory_preferences`.

Core and extension validators reject lifestyle, diagnosis/health-history,
family, financial, and demographic profiling. Extensions are namespace-
versioned and an older client quarantines only the unknown namespace; it cannot
publish or silently discard it. Material changes are explicit user-reported
role/project changes or two conflicting confirmed revision proposals within 30
days. Reminder state uses an injected clock and durable global/per-section
dismissals.

### Ingestion security and publication surfaces

- V1 pins Docling `2.115.0` and records the executable, Python environment,
  model, and artifact digests. Rich extraction runs without network access in
  private scratch space with CPU, memory, wall-time, input, archive expansion,
  page, and output limits. Native qualification is required on macOS, Linux,
  and Windows before enabling that format there.
- Web capture permits HTTPS only, strips credentials, limits redirects/time/
  bytes/content types, resolves and revalidates every hop, rejects loopback,
  private, link-local, metadata, and organization-denied ranges, and never
  executes page scripts.
- Originals and normalized output pass secret classification before portable
  persistence; suspected secrets are quarantined with redacted warnings.
- The daemon owns immutable artifact/candidate storage and publication
  transactions. CLI and desktop expose review, boundary/tag edit, diff, and
  publish surfaces over the same service API.

### Resolver, receipts, and enforcement

- Purpose, sensitivity, field IDs, trust classes, grant duration, and matching
  use canonical typed identifiers and table-driven comparison rules.
- Evaluation time, candidate digest, ranker version/configuration, deterministic
  tie-breaker, renderer version, tokenizer/budget algorithm, and model budget
  are explicit resolver inputs recorded in receipts.
- Receipts consist of an immutable root selection receipt plus hash-chained
  retrieval/access events. The daemon generates audience projections; the
  user-private fragment is encrypted separately and never stored in
  organization-readable state.
- Procedure gates cover only named CommonKit-controlled operations and relay
  tool calls. CommonKit makes no claim to intercept arbitrary shell/editor
  activity outside those boundaries.
- JIT grant approval/revocation and receipt inspection have both desktop and
  headless CLI/service surfaces with content-addressed confirmation bindings.

### Runtime and release ownership

- The daemon owns canonical context, grant, receipt, tombstone, and encrypted
  projection state. `commonkit-mcp` owns official-SDK tool contracts.
  `commonkit-relay` transports and caches calls but never becomes canonical.
- Every tool call carries the verified runtime-session principal and is
  re-authorized by the daemon before output construction.
- The relay adds a bounded local/stdio upstream transport with allowlisted
  executable identity, isolated environment, lifecycle limits, secret
  references, and restart recovery.
- Daemon durable state has versioned migrations, private permissions,
  transactional writes, integrity checks, downgrade refusal, and redacted
  corruption recovery.
- `rtest` runs JavaScript/Vitest suites remotely; installed lifecycle and
  platform/provider qualification run natively on each named OS. A versioned
  redacted evidence schema records commands, revisions, environment/provider
  digests, outcomes, and artifact hashes without tokens, keys, plaintext, or
  personal ciphertext.
