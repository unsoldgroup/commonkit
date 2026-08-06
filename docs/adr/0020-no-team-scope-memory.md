# Memory stays single-owner; team knowledge is Git-owned content

CommonKit's multiplayer model adds no shared memory. An **About Me Profile**
remains one person's encrypted, single-writer store under ADR 0010, and there is
no `team` **Scope** inside it and no second profile store beside it. What a team
actually wants to share — conventions, endpoints, deploy rules, house style — is
knowledge that should be read, reviewed, and versioned, so it belongs in the
shared team repository as a skill, a document, or a **Styleguide**, where
**Reconciliation** already plans, applies, verifies, and rolls it back, and
where a **Grant** already circulates it. `yc-software/qm` layers memory across
scopes because its agent lives in a room with other people and captures what is
said there; CommonKit's agent works on a target for one person, and the analogous
need is met by content that is already portable. Rejected: a `team` scope inside
the profile database, which is qm parity and requires multiple writers on a
database ADR 0003 and ADR 0010 made single-writer deliberately. Also rejected: a
separate team profile store with one designated writer, which preserves that rule
and buys a second memory system whose contents are shared team knowledge living
outside review. This narrows the multiplayer scope on purpose: shared knowledge
in CommonKit is reviewable by construction, and anything that must stay
unreviewable is personal, encrypted, and therefore not shared.
