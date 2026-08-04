# Scope is an ownership axis, not a precedence layer

CommonKit adds **Scope** — personal, team, org — as the axis naming who owns a
contributing piece of a kit and who may lend it, and keeps ADR 0001's ordered
chain of public base, organization policy, personal kit, project loadout, and
target overrides as the separate axis deciding which contribution wins. The
two answer different questions: precedence answers which value applies, and
ownership answers who may change it and grant it away. Rejected: collapsing
both into one ordered scope chain with a single writable layer, as
`yc-software/qm` does. That model cannot express a non-overridable floor
structurally, and qm accordingly enforces its organization floor in prose,
concatenating the organization instructions with a sentence telling the model
that lower-scope instructions must not override them. CommonKit's floor is a
composition rule that holds whether or not a model cooperates, and trading it
for an instruction to a model would be a downgrade no amount of model quality
repairs. The cost accepted is two models where qm has one: a reader must learn
that a team scope owning a skill says nothing on its own about where that skill
lands in precedence, and every place that composes must be explicit about which
axis it is consulting.
