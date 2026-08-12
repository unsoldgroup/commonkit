# Share Engram chunks; keep personal memory private

This decision supersedes ADR 0020's blanket rejection of shared memory.
CommonKit may carry Engram's portable, compressed chunks as an opaque managed
artifact set while every live Engram SQLite store remains target-local and
single-writer. Owner-device synchronization accepts the current mixed-scope
chunk format because all targets have the same principal. Cross-principal
transport requires Engram-produced, machine-verifiable project-scope
attestation and fails closed without it; CommonKit never opens or redacts a
chunk payload. Session summaries are personal-only by default, and team facts
are deliberately promoted into typed project observations. A Grant authorizes
future delivery but cannot erase an observation already imported into a
grantee's memory, so revocation is prospective and disclosed before enrolment.
Rejected: Engram Cloud, which creates a second authority beside CommonKit;
manual Git transport, which is repository-bound and unscheduled; and
CommonKit-side filtering, which would violate the boundary that CommonKit
observes declarations without inspecting memory contents. The accepted cost is
that team sync remains unavailable until Engram emits scope-separated,
attested chunks. The attestation contract is `engram.scope-export.v1`: exporter
version, portable project identity, project scope, exact manifest digest, and
the ID, compressed size, and digest of every chunk. CommonKit verifies and
unions this metadata but does not originate the classification claim.
