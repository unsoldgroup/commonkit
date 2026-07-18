# ADR 0007: Limit chezmoi to isolation-safe semantics

Status: Accepted

## Decision

CommonKit pins chezmoi 2.70.4 and enables only semantics whose desired state can be materialized in an isolated destination without consulting or mutating the live target.

The initial provider supports ordinary files, directories, portable modes, safe relative symlinks, and deterministic destination-independent templates. It rejects scripts, hooks, command interpreters, externals, `modify_`, `create_`, exact directories, explicit removals, encrypted sources, destination-path variables, host-filesystem inspection, process execution, network access, and secret-manager functions before provider execution.

Chezmoi runs with an empty environment, private workspace-scoped home/cache/state/working/destination paths, external refresh disabled, and scripts explicitly excluded. CommonKit scans staged output into normalized resources; only CommonKit adapters may mutate the target.

## Rationale

Chezmoi defines target state partly from the current destination and does not expose a documented “compute against live A, emit into isolated B” interface. Supporting destination-dependent features would either change their meaning or require live `chezmoi apply`, violating CommonKit transactionality.

Unsupported features may graduate only after fixtures prove staged/live semantic parity and rollback through CommonKit operations. A mismatch never permits provider-native live apply.

## Consequences

- Existing advanced chezmoi sources receive source-linked remediation instead of partial output.
- Native CommonKit resources remain the fallback for denied capabilities.
- CommonKit does not redistribute chezmoi in the current design. If that changes, its MIT notice must ship in every artifact.
