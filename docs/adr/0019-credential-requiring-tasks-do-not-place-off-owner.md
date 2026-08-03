# A credential-requiring Declared task does not place off-owner

A **Declared task** that requires credentials places only on an **Execution
Target** its submitter owns. On a target owned by someone else it does not
place at all, so a shared target fails closed rather than quietly borrowing an
identity. This keeps CommonKit's standing rule intact — **Reconciliation**
never treats secrets or machine identity as portable content, and secret values
are provisioned per target from the operator's own password manager — and it
requires nothing to be built, because refusing placement is a capability check
of the kind ADR 0012's scheduler already performs. Rejected: running the
**Attempt** under the target owner's credentials, which is immediately useful
and means a teammate's declared task executes holding the owner's production
tokens, with the owner's receipt as the only trace. Also rejected: brokering
the submitter's credentials onto another person's machine for the attempt
lifetime, which is correct attribution and is precisely the secret-portability
CommonKit has refused to build; nothing here is urgent enough to reverse that.
The capability accepted as lost is real: a shared **Execution Target** is useful
only for hermetic work. That is also the work the support matrix needs it for,
since **Validation evidence** proves a platform and a **Loadout**, and a check
that reaches a credentialed service proves neither. Widening this is a
deliberate later decision about shared credential references, not a default.
