# CommonKit

CommonKit is a portable, versioned collection of shared developer capabilities that can be materialized consistently across local machines, remote hosts, projects, and coding agents.

## Product positioning

**Primary audience**:
AI-native developers and small engineering teams using multiple coding agents across local and remote development environments.

**Primary promise**:
Your best development setup, everywhere. CommonKit makes hard-won agent instructions, skills, hooks, tools, and policies portable so every machine and coding agent starts capable, consistent, and ready.

**Supporting benefits**:
Control, safety, and speed substantiate the primary capability promise; they do not compete with it as the lead message.

**Continuous improvement**:
CommonKit natively uses SkillOpt to produce evidence-backed skill improvement candidates from evaluations and approved, redacted usage evidence. Improvements are computed in isolation, reviewed by a human, promoted into Git-owned source, and then reconciled across targets. CommonKit never allows the optimizer to mutate active skills directly.

**Secret provisioning**:
CommonKit keeps secret values out of portable configuration and integrates with password managers to provision them independently on each target. Bitwarden Secrets Manager is the native first provider, not the permanent product boundary.

_Avoid_: Claims that CommonKit synchronizes secrets, guarantees all machine identity remains local, or is exclusively coupled to Bitwarden.

**Brand voice**:
Concise, confident craftsperson. Use plain claims, concrete verbs, short sentences, and technically exact supporting evidence. Be opinionated without hype. Avoid flowery language, cute metaphors, enterprise jargon, and inflated AI-product claims.

## Language

**CommonKit**:
The complete portable collection of shared developer capabilities and policies.
_Avoid_: Toolbox, toolchain, environment sync

**Organization CommonKit**:
An organization-provisioned CommonKit that supplies shared policy, documentation, capabilities, and project context to its users.
_Avoid_: Team profile, company account

**User context**:
User-owned, structured personal information and working preferences selected for injection into agent work.
_Avoid_: User data, memory dump

**User profile**:
The structured, user-owned portion of **User context** initialized through an interview and revised under the user's control.
_Avoid_: Loadout, account profile

**Core profile schema**:
The stable CommonKit-owned set of universal fields available in every structured user-profile interview.
_Avoid_: Organization questionnaire, profile template

**Profile extension**:
A namespaced set of organization-supplied interview questions whose source remains visible to the user.
_Avoid_: Core field, hidden policy

**Organization onboarding record**:
Organization-owned information explicitly required for access, compliance, or employment and collected outside the user-owned profile.
_Avoid_: Required profile field, personal context

**Project context source**:
The project-owned documentation, terminology, decisions, and agent instructions authored beside the project's code or working artifacts.
_Avoid_: Organization context copy, personal profile

**Project context map**:
An editor-approved manifest of project context sources, ownership, scopes, conflicts, duplicates, and known gaps.
_Avoid_: Generated project summary, repository file list

**Project editor**:
An organization-designated repository maintainer authorized to approve and publish project context through CommonKit.
_Avoid_: Repository writer, organization administrator

**Profile draft**:
The structured, field-by-field representation of interview answers that the user reviews before values enter encrypted storage.
_Avoid_: Interview transcript, inferred profile

**Profile revision proposal**:
An agent-suggested, evidence-explained change to a user-profile field that remains a draft until the user confirms it.
_Avoid_: Learned preference, automatic profile update

**Personal context repository**:
A user-controlled private GitHub repository containing client-side encrypted user-context values and plaintext canonical schema field names for reviewable history and pre-authorization discovery.
_Avoid_: Organization repository, plaintext profile

**Context descriptor**:
A plaintext schema field name indicating that a category of user-owned context exists without revealing its encrypted value.
_Avoid_: Profile value, authorization grant

**Context grant**:
A user-issued authorization permitting selected encrypted context to be injected for a defined organization, project, eligible agents, purpose, sensitivity, and duration.
_Avoid_: Device trust, repository access

**Agent trust class**:
A policy-defined set of identity, execution, isolation, and data-handling requirements an agent runtime must satisfy before it can receive granted context.
_Avoid_: Agent product, agent instance

**Recovery identity**:
A user-controlled identity capable of restoring access to encrypted **User context** after authorized devices are lost.
_Avoid_: Organization recovery key, account password

**Deletion tombstone**:
A signed portable record revoking future use of deleted user context and directing CommonKit-controlled devices to remove active copies, caches, and encryption envelopes.
_Avoid_: Empty profile field, Git deletion

**Profile conflict**:
Two concurrent encrypted revisions of the same profile field that remain preserved until the user explicitly selects or replaces their value.
_Avoid_: Latest-write-wins, automatic merge

**Organization documentation**:
Organization-owned source material onboarded as curated context for agent work.
_Avoid_: Knowledge dump, shared memory

**Documentation artifact**:
A versioned organization- or project-owned source plus its normalized agent-ready representation, provenance, integrity hash, and scope.
_Avoid_: Live web page, untracked extraction

**Documentation revision candidate**:
An inactive proposed revision containing the upstream diff, normalized output, extraction warnings, and affected-context preview for editor review.
_Avoid_: Automatic documentation update, live revision

**Context section**:
An addressable agent-ready portion derived from semantic boundaries in an immutable **Documentation artifact** and retaining exact source provenance.
_Avoid_: Detached chunk, generated summary

**Documentation conflict**:
Two published context claims that disagree and must remain source-attributed until an authorized editor resolves them.
_Avoid_: Layer override, newest-wins merge

**Task context brief**:
The compact initial context injected for agent work, containing critical policy, authorized profile values, relevant source descriptors, and selected documentation within budget.
_Avoid_: Complete project context, static handbook

**Context receipt**:
An immutable, redacted explanation of which context was considered, authorized, selected, omitted, injected, or retrieved for agent work and why.
_Avoid_: Prompt log, plaintext transcript

**Receipt view**:
An audience-specific projection of a **Context receipt** that reveals only the sources and decisions its viewer is authorized to inspect.
_Avoid_: Shared audit log, full prompt trace

**Project memory store**:
A target-local database holding indexed project content and session memory for one agent-context tool, at a path CommonKit declares, observed and snapshotted as mutable state and never carried in portable state.
_Avoid_: Cache, index, knowledge base

**Engram chunk set**:
An opaque, append-only, content-addressed set exported from one target-local
Engram store for a declared **Engram project identity**. CommonKit may inventory
and carry compressed chunks but never inspect their payloads.
_Avoid_: Shared database, memory snapshot

**Engram project identity**:
A portable, explicitly declared subject identifier shared by the same project
on different targets and independent of repository basename.
_Avoid_: Project directory name, database key

**Organization membership**:
The GitHub-backed association and role that authorizes a user to access an **Organization CommonKit** in version 1.
_Avoid_: CommonKit account, repository collaborator

**Context injection**:
The policy- and scope-bound delivery of selected organization, project, and user context into agent work.
_Avoid_: Prompt stuffing, synchronization

**Context plane**:
The CommonKit-owned selection, budgeting, rendering, and provenance of context injected into agent work.
_Avoid_: MCP gateway, knowledge base

**Capability control plane**:
The CommonKit-owned desired state for MCP servers, tools, grants, and credential references.
_Avoid_: MCP relay, tool registry

**Capability data plane**:
The replaceable local or hosted transport through which agents discover and invoke authorized capabilities.
_Avoid_: Context plane, source of truth

**Upstream publication**:
An explicit user-authorized act that shares selected user-owned context with the organization.
_Avoid_: Sync back, telemetry

**Generalized contribution**:
A user-reviewed reusable practice, template, style rule, or documentation improvement derived from personal context without exposing the source profile field or value.
_Avoid_: Shared profile field, anonymized telemetry

**Contribution attribution**:
The per-contribution user choice to identify a **Generalized contribution** by name, pseudonym, or no public identity.
_Avoid_: Organization attribution policy, profile identity

**Contribution license**:
The explicit durable permission an author grants the organization to use, modify, and redistribute an accepted **Generalized contribution** while retaining authorship.
_Avoid_: Ownership assignment, revocable access grant

**Loadout**:
A selected subset and set of overrides from a CommonKit for a particular target.
_Avoid_: Profile, bundle

**Target**:
A local or remote machine where a loadout is materialized.
_Avoid_: Box, VPS, environment

**Adapter**:
A translator between CommonKit's normalized model and an agent or service's native configuration.
_Avoid_: Plugin, integration

**Agent-context provider**:
A package system that resolves, locks, audits, and compiles agent instructions, skills, prompts, hooks, plugins, and MCP declarations.
_Avoid_: CommonKit package manager

**Desired-state provider**:
A version-pinned, non-mutating resolver that converts provider-owned inputs into normalized desired resources and provenance.
_Avoid_: Adapter, installer

**Operation adapter**:
A CommonKit-controlled mutator that prepares, applies, verifies, and rolls back normalized operations on a target.
_Avoid_: Provider, package manager

**Reconciliation**:
The process of planning, applying, and verifying a target against its selected loadout.
_Avoid_: File copy, deployment

**Job**:
An immutable, content-addressed request for durable execution that outlives its submitting client.
_Avoid_: Task process, remote command

**Attempt**:
A revisioned execution of a **Job** with its own lease, fencing token, and terminal receipt.
_Avoid_: Job retry

**Execution Profile**:
Execution-only resource, capability, isolation, and comparability requirements for a **Job**, pinning operating system, architecture, and machine class. It does not pin the toolchain (ADR 0010).
_Avoid_: Loadout

**Validation evidence**:
A terminal **Attempt** receipt binding an exact commit, platform, Loadout digest, and **Execution Profile** digest, recorded as a support-matrix row's proof that a platform is supported.
_Avoid_: Test run, CI result, green build

**Declared task**:
A repository-owned, immutable **ExecutionManifest** named by a stable task ID, the only thing a remote agent or webhook may submit.
_Avoid_: CI job, pipeline step, workflow

**Investigation**:
A **Declared task** that runs a coding agent on the **Execution Target** that produced a failed **Attempt**, inside its retained workspace, and emits findings as an artifact (ADR 0011).
_Avoid_: Auto-fix, self-healing CI, agent remediation

**Isolation level**:
The enforced execution boundary a target actually provides — namespace sandbox with cgroup ceilings, process group with resource limits, or process only — recorded in the **Execution Profile** rather than required of every target (ADR 0013).
_Avoid_: Sandbox, security level

**Implementor**:
The adapter contract through which CommonKit drives a coding-agent engine as a **Job** — start, events, send, cancel, resume, result — keeping engine references opaque.
_Avoid_: Claude adapter, Codex integration

**Execution Target**:
A managed **Target** advertising durable-execution readiness, capacity, and an exact Loadout digest.
_Avoid_: Worker box

**Context budget**:
The per-**Loadout** accounting of agent context a target pays for on every turn, split into always-on text and router text.
_Avoid_: Token limit, context window

**Always-on text**:
Agent instruction content injected on every turn regardless of task, such as compiled `AGENTS.md`, `CLAUDE.md`, and hook prose. Reducible only by lossy rewriting.
_Avoid_: System prompt, preamble

**Router text**:
The single description line each skill contributes so an agent can decide whether to load it. Reducible only by admitting fewer skills.
_Avoid_: Skill metadata, frontmatter

**Distillation**:
A lossy rewrite of an imported source into the minimal form that preserves its behavior, producing a distilled artifact and a **Retention map**.
_Avoid_: Summarization, compression

**Retention map**:
The provenance record binding each digested section of an imported upstream source to whether distillation kept or dropped it, and where kept content landed.
_Avoid_: Source map, diff

**Material change**:
An upstream revision that touches or removes a section the **Retention map** marks kept, or adds a section matching no known digest or heading. Only a material change warrants re-distillation.
_Avoid_: Upstream drift, breaking change

**About Me Profile**:
The kit owner’s encrypted, portable collection of approved personal facts and preferences.
_Avoid_: Project memory, transcript archive, user account

**Claim**:
One approved, versioned fact or preference in an **About Me Profile**.
_Avoid_: Observation, project fact

**Scoped View**:
The subset of profile claims available to one Loadout and trusted project.
_Avoid_: Separate profile, persona

**Suggestion**:
A proposed claim based on words the user stated directly; it is not approved memory.
_Avoid_: Automatic learning, inference

**Styleguide**:
An optional, single-valued **Loadout** capability that routes one selected
writing skill for declared prose scopes and modes. It is not always-on
instruction text and is not a public lint gate.
_Avoid_: Default voice, prose policy

**Scope**:
The ownership axis naming who owns a contributing piece of a **CommonKit** and
who may lend it — `personal`, `team`, or `org`. It is not a precedence layer
(ADR 0015).
_Avoid_: Layer, tier, namespace, workspace

**Principal**:
An acting person, identified by a GitHub login and proven through the installed
GitHub CLI, resolved the same way on every surface (ADR 0021).
_Avoid_: User, account, actor, identity

**Policy floor**:
The resolved intersection of the submitter's and target owner's policy for a
shared **Job**, recorded in the terminal **Attempt** receipt (ADR 0018).
_Avoid_: Audience floor, least privilege, effective policy

**Grant graph**:
The live record of scope membership and which scope lends which capability to
which, owned by the authoritative scheduler and never stored in Git (ADR 0014).
_Avoid_: Permissions file, ACL config, share list

**Grant**:
One edge in the **Grant graph**: an owning scope lending one named capability
to one grantee scope at one permission.
_Avoid_: Share, invite, copy

## Relationships

- A **CommonKit** defines one or more **Loadouts**.
- An organization provisions an **Organization CommonKit** for one or more users; an organization with one user follows the same model.
- An **Organization CommonKit** contains one or more projects.
- Each project owns a **Project context source** in its project repository.
- The organization CommonKit repository registers the project, supplies governing policy, and pins the accepted **Project context source** revision without duplicating its content.
- Existing-project onboarding scans known documentation, manifests, ADRs, agent instructions, and repository metadata to propose a **Project context map**.
- A proposed **Project context map** identifies ownership, duplicates, conflicts, gaps, and suggested scopes but remains inactive until a project editor approves it.
- A **Project editor** must have both qualifying GitHub repository access and the CommonKit role assigned by organization policy.
- Repository write access permits proposing project-context changes but does not by itself permit publication.
- GitHub organization or repository access establishes **Organization membership** in version 1 through a replaceable identity-provider boundary.
- A single-user organization may establish **Organization membership** with a personal GitHub account and private repository.
- A user owns their **User context**, including their **User profile**.
- A user's portable **User context** is encrypted before it enters their **Personal context repository**.
- Each user owns a separate **Personal context repository**; the organization owns its distinct organization CommonKit repository.
- CommonKit guides the user to create or connect the **Personal context repository** under the user's GitHub account after organization invitation acceptance.
- An organization may require that a **Personal context repository** exists before personal-context injection, but cannot create, administer, transfer, or delete it.
- CommonKit composes organization, project, and encrypted personal context locally without copying the complete personal record into organization storage.
- Decryption identities remain user-controlled and never enter GitHub, portable configuration, plans, receipts, logs, or organization-owned storage.
- A **Personal context repository** uses partially encrypted structured files: all canonical schema field names and document structure remain plaintext for review, reconciliation, and pre-authorization discovery, while all personal values remain encrypted.
- A **Context descriptor** allows an agent to determine that potentially relevant context exists before it is authorized to decrypt or inject the value.
- An existing **Context grant** permits automatic injection only when its organization, project, agent, purpose, sensitivity, and duration cover the current work.
- Without a matching **Context grant**, an agent sees only the **Context descriptor** and may request just-in-time user approval.
- A just-in-time approval creates a project-and-purpose **Context grant** by default, persists across sessions, and remains active until the user revokes it.
- Before issuing a persistent **Context grant**, CommonKit shows the selected context fields, project, stated purpose, eligible agents, and revocation path.
- A persistent **Context grant** targets one or more **Agent trust classes**, not a product name or individual installation.
- A new agent runtime receives granted context only after CommonKit verifies that it satisfies the selected **Agent trust class**.
- Organization policy defines available **Agent trust classes** and their minimum requirements; users select which qualifying classes receive each personal-context grant.
- A project may narrow eligible **Agent trust classes** but cannot widen them beyond the organization security floor.
- Possession of an authorized device or repository access does not by itself authorize context injection.
- The personal-context schema is closed; unknown or newly introduced fields fail closed until the schema explicitly classifies them.
- Repository access can reveal field names, section presence, edit timing, and approximate encrypted value sizes even though it cannot reveal the values.
- Each user has two independent **Recovery identities**: an offline user-held recovery key and an encrypted recovery identity provisioned through the user's configured password-manager provider.
- An organization is never a recovery recipient for a user's complete **User context**.
- Deleting user context revokes its grants, removes active ciphertext and plaintext caches, destroys active CommonKit-controlled encryption envelopes, and publishes a **Deletion tombstone** to authorized devices.
- Deletion receipts retain only opaque audit metadata and never retain deleted plaintext.
- CommonKit guarantees that deleted context cannot be used for future CommonKit-controlled decryption or injection after a target has observed the valid tombstone.
- CommonKit does not claim cryptographic erasure of historical Git ciphertext from clones or backups when an actor retains a formerly valid recovery identity.
- CommonKit does not claim to erase plaintext previously exported to an uncontrolled recipient or an organization artifact already incorporated under a **Contribution license**.
- Concurrent edits to different profile fields merge automatically from their immutable revisions.
- Concurrent edits to the same field create a **Profile conflict**; both revisions are decrypted only on an authorized user device and remain unresolved until the user chooses or writes a replacement.
- A structured interview initializes a **User profile**.
- A structured interview is composed from the **Core profile schema** plus zero or more visibly namespaced **Profile extensions**.
- CommonKit owns and versions the **Core profile schema**; organizations own and version their **Profile extensions**.
- Every question identifies whether it came from CommonKit or an organization extension.
- Every personal-profile question is optional, including questions from a **Profile extension**.
- Information required for access, compliance, or employment belongs in a separate **Organization onboarding record** with explicit purpose, ownership, and retention.
- The profile interview is an adaptive conversation that asks one question at a time and maps answers into the selected schema.
- The interview produces a **Profile draft** showing every field name and proposed stored value for user review and editing before encryption.
- Interview transcripts and unconfirmed inferences do not become **User context**.
- The **Core profile schema** is work-focused: identity and role, communication, collaboration, feedback, decision-making, planning, technical preferences, requested accessibility accommodations, boundaries, and agent-interaction preferences.
- The **Core profile schema** excludes lifestyle, health history or diagnoses, family, financial, and demographic profiling.
- An eligible agent may create a **Profile revision proposal** from observed collaboration, but cannot update the **User profile** automatically.
- A **Profile revision proposal** explains the observed pattern without retaining raw conversations as profile evidence by default.
- A user must review and confirm a **Profile revision proposal** before its values enter encrypted storage.
- CommonKit suggests profile review after material role or project changes, conflicting agent feedback, and on a lightweight six-month cadence.
- Users may dismiss review reminders permanently or for individual profile sections.
- **Organization documentation** remains organization-owned and is onboarded separately from user-owned context.
- Initial documentation sources include Git repositories, ordinary files, rich-document uploads processed through an isolated extractor, and selected web pages captured as pinned snapshots.
- Every **Documentation artifact** preserves the original source, normalized content, owner, source revision or capture time, integrity hash, and organization or project scope.
- Imported documentation is immutable at a revision; a changed source produces a new candidate revision rather than silently changing injected context.
- Scheduled read-only checks detect changed documentation sources and create **Documentation revision candidates**.
- Only an authorized organization editor may publish a **Documentation revision candidate** for subsequent context injection.
- Large **Documentation artifacts** are divided into derived **Context sections** using headings and semantic boundaries while the original and normalized revision remain immutable.
- A **Context section** inherits its source owner, scope, revision, and hash linkage; authorized editors may adjust its title, tags, and boundaries before publication.
- Project documentation receives higher task relevance than organization-wide documentation but does not silently override it.
- When published sources disagree, CommonKit injects both source-attributed claims as a **Documentation conflict** and queues it for authorized editor resolution.
- **Context injection** composes only the selected organization, project, and user context authorized for the current agent work.
- Agent work begins with a budgeted **Task context brief** and retrieves complete **Context sections** on demand.
- A **Context receipt** covers both initial injection and subsequent retrieval without retaining secret or personal plaintext.
- The user-facing **Receipt view** identifies the user's injected fields, grants, and relevant organization or project sources.
- The organization-facing **Receipt view** identifies organization and project sources and policy outcomes but represents personal-context participation only as an opaque authorized indicator.
- Personal field names and values never appear in an organization-facing **Receipt view**.
- Authorization, mandatory policy, scope, precedence, and context-budget enforcement deterministically define the eligible **Task context brief**.
- A replaceable relevance ranker may order eligible context but cannot authorize, override, or silently omit mandatory material.
- Every included and omitted candidate has a reproducible reason in the **Context receipt**.
- When mandatory explanatory context exceeds the initial budget, the **Task context brief** contains its descriptors and requires retrieval during work rather than blocking task start.
- Deferred retrieval never defers enforceable policy: CommonKit must block an affected operation until its governing control is enforced and any required instructions have been retrieved.
- Machine-enforceable organization and project controls are compiled into CommonKit policy and apply independently of prompt contents.
- Human-readable procedures may be retrieved on demand; an operation governed only by a procedure remains blocked until the agent retrieves that procedure.
- The **Context plane** and **Capability control plane** remain owned by CommonKit and independent of any relay or hosted gateway.
- The **Capability data plane** uses a persistent device relay as the default agent endpoint for local, stdio, offline, and private capabilities.
- An organization may add a hosted MCP portal adapter for remote, organization-governed capabilities without making that portal the canonical registry.
- The device relay may route organization-governed remote calls through the selected hosted portal while keeping device-private calls local.
- User-owned context remains private unless the user performs an explicit **Upstream publication**.
- **Upstream publication** creates a separate, reviewed **Generalized contribution** and never transfers ownership of the source personal record.
- The user reviews the exact **Generalized contribution** before submission; the source profile field and value remain private.
- Before submission, the user selects a **Contribution attribution** of named, pseudonymous, or anonymous.
- Private derivation provenance remains visible only to the user unless the user explicitly authorizes its disclosure.
- Submission presents the applicable **Contribution license** before confirmation; repository placement alone never implies consent silently.
- Withdrawing a contribution stops new publication and may remove future attribution links but cannot retract organization versions already incorporated under the accepted **Contribution license**.
- A **CommonKit** composes configuration in this precedence order: public base, organization policy, personal kit, project loadout, then target overrides.
- Organization security policy is a non-overridable floor. Later layers may tighten it but cannot weaken it.
- A **Loadout** is materialized on one or more **Targets**.
- A **Job** has one or more ordered **Attempts**, but at most one active leased **Attempt**.
- An **Execution Target** references a managed **Target** and exact Loadout and **Execution Profile** digests.
- An **Execution Target**'s Loadout digest is the composed loadout digest of that **Target**'s last successful **Reconciliation**, not an independently authored value. A **Job** whose digests differ does not place, so a drifted target fails closed instead of producing incomparable results.
- A repository declares its own checks as **Declared tasks**. CommonKit submits them; it never accepts arbitrary argv, and a shell string is not a task.
- **Validation evidence** is commit-bound. It proves the platform and the CommonKit-managed environment; the toolchain is proven by the repository's own commit-pinned files (ADR 0010).
- An **Investigation** inherits the **Loadout** of the target it runs on, so it reasons with the same skills and hooks the developer's own agent would. Its output is artifacts only; an **Execution Target** produces evidence and analysis but never authors repository history (ADR 0011).
- One authoritative scheduler serves many target-resident workers; a scheduler per machine with federation is not v1 (ADR 0012). The single-writer database limit is deliberate and is not high availability.
- "Run this on every platform" is one **Declared task** per platform, each capability-labelled. The scheduler has no matrix or fan-out semantics.
- **Validation evidence** states the **Isolation level** it was produced under, so results from unlike targets are never silently compared.
- A **Job** submitted onto an **Execution Target** owned by someone else runs under a **Policy floor**: allowed hosts intersect, denied hosts union, over the submitter and the target owner only. Neither principal's policy can be widened by the other (ADR 0018).
- **Validation evidence** states the **Policy floor** it was produced under, for the same reason it states **Isolation level**.
- A **Job** places on an **Execution Target** owned by someone else only when the submitter holds a **Grant** edge to that target's **Scope**, and a **Declared task** requiring credentials does not place off-owner at all (ADR 0019).
- A terminal **Attempt** receipt on a shared **Execution Target** is readable by the submitter and the target owner.
- The Session Board shows every **Principal**'s sessions in a shared **Scope**, and every verb stays bound to the session's owning **Principal**. A teammate never resolves another **Principal**'s pending action, so an "Always" rule is only ever written by the owner of the project it lands in (ADR 0008, ADR 0009).
- A shared **Scope** has no shared credentials in version 1. A credential a target needs is provisioned on that target from its owner's password manager, as for a single-owner kit.
- Onboarding a **Principal** is: add them to the **Grant graph**, extend read on the shared team repository, and let their first **Reconciliation** materialize team and org **Scope** content plus every resolved **Grant**. Their personal kit stays their own repository and may lend back.
- The CLI is the primary surface for durable execution. The Session Board mirrors runs read-only for the glanceable case; it consumes the execution API as a client and never fronts it.
- MCP exposes context and durable execution controls; `commonkit-execd` owns lifecycle persistence independently of MCP sessions.
- An **Adapter** participates in **Reconciliation** for one agent or service.
- A **Loadout** selects a version-pinned **Agent-context provider** for portable agent content.
- A **Desired-state provider** computes resources in isolated staging; it never mutates a managed live target.
- An **Operation adapter** is the only component allowed to mutate, verify, or roll back a target.
- APM is CommonKit's preferred **Agent-context provider**; CommonKit does not compete with APM's package resolution, distribution, compilation, or package-audit responsibilities.
- Chezmoi is CommonKit's preferred home-configuration provider for the subset of semantics proven equivalent under isolated materialization; unsupported destination-dependent or executable features fail closed.
- A **Loadout** has exactly one **Context budget**; admitting an import that exceeds it forces an explicit eviction choice rather than silent growth.
- **Distillation** never mutates a live target. It computes a distilled artifact and **Retention map** in isolated staging, a human promotes them into canonical Git source, and CommonKit **Reconciliation** applies the result.
- An imported skill retains its upstream reference and revision as provenance. The upstream reference is decoupled from the distilled form; re-pulling upstream is a three-way merge against the distilled fork, not a replacement.
- Scheduled read-only upstream checks classify revisions against the **Retention map**. Non-material changes advance the recorded upstream revision silently; only a **Material change** notifies a human and queues re-distillation.
- Imported skills are admitted by human review against the **Context budget**, not by automated evaluation. An admitted skill may afterward enter the SkillOpt evaluation and promotion path unchanged.
- A **Loadout** can select at most one **Styleguide**. Later layers replace the
  complete selection, while organization policy can deny or pin it. CommonKit
  binds its descriptor, APM manifest and lock, evaluation suite, and retention
  map into normalized state and plan provenance.
- A **Styleguide** activates through skill routing. It does not enter public-base
  instructions, and its deterministic writing-form metric is internal
  evaluation evidence rather than a reconciliation gate.
- SkillOpt is an exact-version external candidate-computation provider, not a target mutator. CommonKit independently evaluates held-out evidence and policy, requires human promotion into canonical Git source, then uses APM and CommonKit reconciliation for compilation and named-canary deployment.
- Native providers remain available for migration, fallback, and capabilities not safely delegated upstream.
- CommonKit stages provider output and applies it through CommonKit **Reconciliation** so target mutation remains plan-bound, receipted, verifiable, and recoverable.
- APM policy governs which agent packages and primitives may be installed; CommonKit policy governs targets, paths, permissions, services, credentials, schedules, relay exposure, and mutation authorization.
- The APM lockfile owns the resolved agent-package graph and content integrity; the CommonKit lockfile references its digest and owns composed loadout, target, and adapter state.
- The ordered composition chain decides precedence; **Scope** decides ownership and lending authority. The two axes are orthogonal, and a **Scope** owning content says nothing on its own about where that content lands in precedence (ADR 0015).
- A `team` **Scope** exists to own and lend. Organization security policy remains the non-overridable floor, enforced structurally by composition rather than by instructions to a model.
- Shared content — skills, loadouts, policy, styleguides — is Git-owned and applied by **Reconciliation**. The **Grant graph** is owned by the authoritative scheduler and is never Git-owned (ADR 0014).
- The **Grant graph** keys on **Principals**, and every surface resolves the acting **Principal** identically. Multiplayer CommonKit requires GitHub; a single-owner kit does not (ADR 0021).
- Team and org **Scopes** live in one shared team repository; each personal **Scope** is a private repository of its own (ADR 0016).
- A **Grant** materializes the owner's content at a handle path on the grantee's target and is removed on revoke. It is a mount, never a copy.
- A **Grant** between personal **Scopes** confers composition, not Git read access. The owner extends repository access out of band, and an edge the grantee's target cannot read stays unresolved.
- Content arriving through a **Grant** composes beneath the grantee's personal kit, so the grantee's own content always wins a name collision and the lent contribution is recorded as shadowed in the plan and receipt (ADR 0017).
- A `team` **Scope** lends; it does not bind. A binding rule is organization policy, which is a non-overridable floor and is not a **Grant**.
- Promoting content to a higher **Scope** is a pull request into the shared team repository. Organization policy governs whether it requires review.
- The grantee's **Loadout** pays the **Context budget** for lent content. A **Grant** that would exceed it stays unresolved and is reported by **Reconciliation**; the grantee admits it by evicting something explicitly.
- A **Grant** resolves to Git-owned content plus a live graph edge. Content reachable in Git is not granted without an edge, and an edge whose content is absent is unresolved rather than an error.
- Revoking a **Grant** takes effect at the graph, not at the grantee's next pull.
- **Reconciliation** never treats secrets or machine identity as portable CommonKit content.
- **mcp-local-relay** is an independently publishable package in the CommonKit repository. It remains the MCP data plane; CommonKit owns desired-state composition and reconciliation.
- A GitHub repository is the durable store for portable, reviewable CommonKit state. Secrets and mutable database files do not belong in Git.
- A local CommonKit service exposes peer interfaces for the macOS status bar, CLI, and MCP tools. The status bar does not communicate through MCP.
- The version 1 runtime is implemented in Rust and shared by the CLI, local service, MCP server, and Tauri 2 desktop application. The existing TypeScript reconciliation engine and `mcp-local-relay` runtime are migration sources, not permanent sidecars.
- Database adapters create consistent, integrity-checked, encrypted snapshots in S3-compatible object storage. Git records only snapshot descriptors and content hashes.
- Version 1 uses one authoritative writer per database. Cross-machine database portability is snapshot and restore, not binary merging; multi-writer synchronization requires a later application-level export/import model.
- An **About Me Profile** is stored in a dedicated encrypted SQLite database with one writer; context-mode and Engram remain project-memory systems.
- CommonKit owns a **Project memory store**'s declared path, directory, and environment, never its contents, schema, keys, or retention. An **About Me Profile** is owner memory and is encrypted by CommonKit; a **Project memory store** is project memory, is not encrypted by CommonKit, and holds no **Claims**.
- A **Project memory store** is per-**Target** and is never **Scope**-shareable. Cross-machine movement is operator-initiated snapshot and restore, never reconciliation, and never a merge of stores belonging to different targets.
- There is no team **About Me Profile**. Engram's live store remains
  target-local, while project-scoped **Engram chunk sets** may circulate by
  **Grant** only after Engram attests their scope; personal observations and
  session summaries never cross a principal boundary (ADR 0022).
- A **Scoped View** controls disclosure by Loadout and trusted project. Organization policy may narrow access but never broaden it.
- Agents may create **Suggestions**, but only direct user edits or explicit contradiction clarifications create approved **Claims**.

## Example dialogue

> **Developer:** "Does my remote Codex session have the same hooks and skills as my Mac?"
> **Domain expert:** "Apply the remote-development **Loadout** from your **CommonKit** to that **Target**, then verify its **Adapters**."

## Flagged ambiguities

- "Toolbox" was the initial metaphor; resolved: the product and domain object are **CommonKit**.
- "Profile" described a selected subset; resolved: use **Loadout** unless later user research favors a more conventional term.
- "Profile" is now valid only as **User profile**, the user-owned structured context produced by interview; it does not mean **Loadout**.
- Organization initiation does not imply organization ownership of personal context; resolved: users own their **User context** and sharing upstream requires explicit publication.
- MCP scaling was initially framed as local relay versus hosted gateway; resolved: CommonKit uses a hybrid **Capability data plane**, with the device relay as the default endpoint and hosted portals as optional organization adapters.
- CommonKit-owned accounts versus external identity was initially open; resolved for version 1: GitHub bootstraps **Organization membership**, behind a replaceable identity-provider boundary.
- Personal-context recovery was initially open; resolved: users receive both an offline recovery key and a password-manager-provisioned encrypted recovery identity, while organizations receive neither.
- Personal-context encryption granularity was initially open; resolved: use partially encrypted structured files with canonical schema field names visible for reviewable Git history and pre-authorization discovery while acknowledging metadata leakage.
- Pre-authorization discovery was initially open; resolved: apply existing scope- and purpose-bound **Context grants** automatically, otherwise expose only the descriptor and request just-in-time approval.
- Just-in-time grant duration was initially open; resolved: approvals persist for the selected project and purpose across sessions until revoked.
- Eligible agents were initially open; resolved: persistent grants target policy-defined **Agent trust classes**, allowing portability without automatically trusting new runtimes.
- Agent-class authority was initially open; resolved: organizations define minimum requirements, users select eligible classes per grant, and projects may only narrow them.
- Personal-context repository ownership was initially open; resolved: each user owns a separate encrypted repository that CommonKit composes locally with the organization's repository.
- Personal-repository provisioning was initially open; resolved: creation or connection is a user-authorized guided step under the user's GitHub account, never an organization-administered operation.
- Interview ownership was initially open; resolved: CommonKit supplies a stable core schema and organizations may add visibly namespaced extensions.
- Required extension questions were initially open; resolved: personal-profile answers are always optional, and genuinely required information belongs in a separate organization-owned onboarding record.
- Interview experience was initially open; resolved: use an adaptive conversation followed by mandatory structured review before encrypted storage.
- Core profile breadth was initially open; resolved: capture work-focused context and requested accommodations while excluding broad personal and sensitive-person profiling.
- Profile learning was initially open; resolved: agents may propose evidence-explained revisions, but only explicit user confirmation changes the profile and raw conversations are not retained by default.
- Profile review cadence was initially open; resolved: combine material-change prompts with a six-month checkup while allowing user-controlled dismissal.
- Initial documentation sources were open; resolved: support Git repositories, files, isolated rich-document extraction, and pinned web snapshots with originals and full provenance.
- Documentation update behavior was open; resolved: detect changes automatically, but require editor review of diffs, extraction warnings, and affected projects before publication.
- Documentation selection granularity was open; resolved: derive editor-reviewable semantic sections that retain exact provenance to immutable source revisions.
- Documentation conflict handling was open; resolved: surface both claims with scope and revision, prioritize project relevance without silent override, and require editor resolution.
- Documentation delivery was open; resolved: inject a compact task-specific brief first, retrieve full sections on demand, and account for both in the context receipt.
- Initial context selection was open; resolved: use deterministic policy and budget gates with replaceable explainable relevance ranking over eligible candidates only.
- Mandatory-context overflow was open; resolved: start with descriptors and retrieve full sections during work, while never allowing retrieval deferral to bypass enforceable controls.
- Deferred-policy enforcement was open; resolved: compile machine-enforceable controls outside the prompt and block operations governed only by human-readable procedures until retrieval.
- Context-receipt visibility was open; resolved: provide audience-specific views, with complete user-owned references for the user and opaque personal-context participation for the organization.
- Project-context location was open; resolved: author it beside project artifacts and let the organization kit register, govern, and pin the accepted revision.
- Existing-project onboarding was open; resolved: discover existing sources and propose an editor-reviewed context map instead of generating or activating content automatically.
- Project-context publication authority was open; resolved: organization-designated project editors require both repository access and a CommonKit role.
- Upstream publication form was open; resolved: derive a user-reviewed reusable contribution rather than publishing an exact personal-profile field.
- Contribution attribution was open; resolved: users choose named, pseudonymous, or anonymous attribution separately for each contribution.
- Contribution rights were open; resolved: the user retains authorship and explicitly grants the organization a durable license for accepted artifacts.
- Personal-context deletion was open; resolved: provide bounded deletion, grant revocation, cache removal, and portable tombstones within CommonKit's control while explicitly disclaiming erasure of historical ciphertext decryptable with retained recovery identities, previously exported plaintext, and licensed contributions.
- Multi-device profile conflicts were open; resolved: merge non-conflicting fields, preserve both same-field revisions, and require explicit user resolution rather than latest-write-wins or CRDT guessing.
- "Agent package manager" overlapped with CommonKit's initial adapter scope; resolved: package management belongs to the selected **Agent-context provider**, with APM preferred, while CommonKit orchestrates the complete developer environment.
- APM MCP translation was initially open; resolved: CommonKit reads only APM's documented `dependencies.mcp` manifest shape and isolated staged `.mcp.json`, requires the two representations to agree, normalizes declarations with manifest/lock provenance, rejects ambiguous ownership or unsupported transports, and sends the resulting relay desired state and stable loopback client configuration through CommonKit's confirmation-bound plan. It does not import APM internals or invoke provider-native live apply.
- GitHub authentication was initially open; resolved for v1: the installed GitHub CLI is the credential broker and repository-provisioning client. OAuth credentials remain outside CommonKit and the kit. A first-party GitHub App/device flow may replace this boundary later without changing portable state.

## Open — not yet resolved

- Which S3-compatible object-store provider is the default for encrypted snapshots.
- How context selection, precedence, conflicts, and context budgets work during **Context injection**.
- Which fields belong in the initial **User profile** interview and which are optional extensions.
- What review, redaction, withdrawal, and provenance rules govern **Upstream publication**.

## Portable context first-release scope

The first portable-context release delivers the complete private-context loop:

1. Organization and project registration through GitHub-backed membership.
2. Provenance-preserving organization and project documentation onboarding.
3. A user-owned, partially encrypted personal-context repository.
4. The adaptive core profile interview and structured review.
5. Multi-device encrypted revision synchronization and explicit conflict resolution.
6. Deterministic task briefs, on-demand context retrieval, and compiled policy enforcement.
7. Project-and-purpose grants targeting organization-defined agent trust classes.
8. Audience-specific context receipts, revocation, and bounded deletion.
9. The persistent device relay as the default capability endpoint.

Generalized upstream contributions and the hosted Cloudflare MCP portal adapter are designed but deferred to the next release.

## Version 1 contract

Version 1 is complete only when it supports all of these flows:

1. Initialize a target.
2. Preview drift.
3. Apply safely.
4. Verify parity.
5. Provision credential references independently.
6. Recover or roll back.
7. Manage multiple targets and layered configuration.
8. Run scheduled read-only drift checks.
9. Manage optional `mcp-local-relay` state.
10. Add coding-agent adapters.
11. Export redacted diagnostics.
12. Provide open-source onboarding, schema, threat model, and CI.

Version 1 supports macOS, Linux, and Windows as first-class managed targets. Every platform must support headless CLI/service operation; the desktop status application is an additional interface, not a runtime requirement.

## Session Board

The Session Board is CommonKit's glanceable approval surface: a hub on the always-on VPS (`board.unsold.cloud`, tailnet-only) fed by per-machine reporters, showing every Claude/Codex/Orca session and the decisions they are waiting on.

**Glossary**
- **Pending action**: a decision a human owes a session — a Claude permission prompt, a Codex numbered option prompt, or an Orca gate.
- **Approval card**: the board's rendering of one pending action. A card answers who ([project] + agent), what (verb + relative path or command), why (**intent** — the agent's last transcript message before asking), and shows the exact payload (diff, content, or command).
- **Verbs**: Allow (once), Always (persist a project-local allowlist rule — ADR 0009), Deny with an optional steer message. Codex cards keep their numbered options.
- **Intent**: transcript-derived context on a card; read from the session's JSONL transcript at prompt time, never stored beyond the action.
- **Stale**: an action whose 55s window lapsed; the terminal prompt took over (fail-open, ADR 0008) and the card must show that outcome rather than live buttons.
- **Decision log**: the recent history of human decisions, kept by the hub for glanceable audit (7-day retention).
- **Push opt-in**: per-device web-push subscription so a new pending action reaches the iPad when the board is closed; notification delivery never affects the decision path.
