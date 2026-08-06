# Portable context vendor scan

## Key takeaways

- **Sourced:** Mature packages exist for document extraction, structured
  interviews, encryption, authorization, local-first synchronization, schema
  validation, search, and MCP transport.
- **Reasoned recommendation:** CommonKit should own the domain model:
  `ContextItem`, immutable `Revision`, explicit `Grant`, revocation
  `Tombstone`, deterministic injection selection, and context receipts.
- Integrate upstreams as version-pinned providers or narrow libraries. Provider
  output remains staged and target mutation remains inside CommonKit
  transactions.
- Adopt narrow primitives now; delay CRDTs, external policy engines, and
  distributed search until a concrete product requirement triggers them.

## Flow-by-flow matrix

| Flow | Candidate upstreams | Recommendation |
|---|---|---|
| 1. Organization initializes | [`jsonschema`](https://github.com/Stranger6667/jsonschema) (MIT), [`Cedar`](https://github.com/cedar-policy/cedar) (Apache-2.0) | Pin `jsonschema` for manifests. Keep authorization behind an interface; adopt Cedar when typed rules become difficult to audit or evolve. |
| 2. Documentation onboarding | [`Docling`](https://github.com/docling-project/docling) (MIT), [`pulldown-cmark`](https://github.com/pulldown-cmark/pulldown-cmark) (MIT), [`Pandoc`](https://github.com/jgm/pandoc) (GPL-2.0-or-later) | Build `probe → extract → normalize → validate → receipt`. Parse Markdown natively. Offer Docling as an isolated, pinned provider for rich formats. Keep Pandoc an optional external fallback, not a linked or copied dependency. |
| 3. User ownership setup | [`age`/`rage`](https://github.com/str4d/rage) (MIT/Apache-2.0), [Community Solid Server](https://github.com/CommunitySolidServer/CommunitySolidServer) (MIT) | Use `age` for encrypted portable bundles and key envelopes. Keep the ownership model in CommonKit. Consider Solid only as a future interoperability adapter. |
| 4. Structured profile interview | [SurveyJS Form Library](https://github.com/surveyjs/survey-library) (MIT), [RJSF](https://github.com/rjsf-team/react-jsonschema-form) (Apache-2.0) | Choose one after the interview schema is defined: SurveyJS for branching interview UX; RJSF for a direct JSON Schema model. Survey Creator has separate commercial licensing. |
| 5. Project context | [`jsonschema`](https://github.com/Stranger6667/jsonschema), [`gix`](https://github.com/GitoxideLabs/gitoxide) (MIT/Apache-2.0), [Tantivy](https://github.com/quickwit-oss/tantivy) (MIT) | Schema first. Continue using the existing Git boundary until portability requires narrow `gix` embedding. Add Tantivy only if simpler local FTS cannot satisfy retrieval quality. |
| 6. Context injection | Official tokenizer libraries; experimental [ContextBudget](https://github.com/yw-0311/ContextBudget) | Build the resolver in CommonKit: candidates → authorization → precedence → relevance → cost → deterministic selection → render → receipt. Treat learned compression as research, not a core dependency. |
| 7. MCP availability | Official [MCP SDKs](https://github.com/modelcontextprotocol), [Docker MCP Gateway](https://github.com/docker/mcp-gateway), [Cloudflare MCP server portals](https://developers.cloudflare.com/cloudflare-one/access-controls/ai-controls/mcp-portals/), [Microsoft MCP Gateway](https://github.com/microsoft/mcp-gateway) | Use official protocol SDKs. Keep CommonKit's device relay; add optional hosted portal and Docker adapters. Defer the Kubernetes gateway until enterprise self-hosting is demanded. |
| 8. Profile updates | [Automerge](https://github.com/automerge/automerge) (MIT) | Start with immutable revisions and JSON Patch. Spike Automerge only if offline concurrent editing across devices is required. |
| 9. Upstream publication | [Cedar](https://github.com/cedar-policy/cedar), [OpenFGA](https://github.com/openfga/openfga) (Apache-2.0) | Build publication grants as first-class records. Cedar is the likely embedded authorization engine; defer OpenFGA until hosted relationship authorization is required. |
| 10. Organization updates | [`notify`](https://github.com/notify-rs/notify) (CC0-1.0), Automerge, `gix` | Use immutable Git revisions, three-way reconciliation, and publication receipts. `notify` may detect changes; it must not become source of truth. |
| 11. Cross-device operation | Automerge/Automerge Repo, [Yjs](https://github.com/yjs/yjs) | Keep Git and encrypted snapshot behavior for current v1. If structured user context needs offline multi-device writes, prefer an Automerge spike. Use Yjs only if collaborative rich-text editing becomes core. |
| 12. Context inspection | [MCP Inspector](https://github.com/modelcontextprotocol/inspector), [OpenTelemetry semantic conventions](https://github.com/open-telemetry/semantic-conventions) | Vendor Inspector for developer protocol testing and OpenTelemetry conventions for redacted traces. Build CommonKit's context-receipt explanation UI. |
| 13. Revoke/delete | Cedar or explicit typed rules | Build revocation tombstones and encryption-envelope destruction. State clearly that revocation cannot erase plaintext already exported to an uncontrolled recipient. |
| 14. Leave/export | [RO-Crate](https://github.com/ResearchObject/ro-crate), [Data Transfer Project](https://github.com/dtinit/data-transfer-project) | Borrow the self-describing crate and adapter patterns. Build a smaller CommonKit bundle with schema versions, hashes, provenance, grants, revocations, and JSON/Markdown payloads. |
| 15. Solo organization | Existing schema and policy primitives | No separate dependency or product mode. An organization with one user is cardinality one and must migrate to multiple users without conversion. |

## Proposed adoption sequence

1. Define the CommonKit-owned context, revision, grant, tombstone, and receipt
   contracts.
2. Pin JSON Schema validation and one interview form runtime.
3. Add Markdown ingestion plus a provider contract for optional Docling.
4. Add `age`-encrypted import/export bundles.
5. Implement deterministic context injection and inspection receipts.
6. Add the hybrid MCP data-plane adapters.
7. Re-evaluate Cedar and Automerge against measured authorization and
   concurrent-editing complexity.
