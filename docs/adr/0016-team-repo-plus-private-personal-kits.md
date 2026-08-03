# One team repository, private personal kits

Team and organization **Scopes** live in one shared CommonKit repository that
every member reads, and each person's personal kit stays a private repository
of their own. Personal content therefore remains confidential between
coworkers, which a single shared repository could never provide: Git access
control is repository-level, so within one repository a **Grant** could only
ever control what is materialized and what costs **Context budget**, never what
is secret. Rejected: one repository with scopes as directories, which is
simpler and cheaper but makes every member's personal kit readable by every
other member. Also rejected: a repository per scope, which extends real
confidentiality to team scopes too, at the cost of cross-repository
authentication on every target and a much heavier onboarding for the case
CommonKit actually has, which is a handful of people and one team. The cost
accepted is two distribution paths — the shared team repository and N private
personal repositories — and a boundary where two systems must agree: a
**Grant** edge from one personal scope to another confers composition, not Git
read access, which the owner still extends out of band through the GitHub CLI
credential broker. CommonKit does not paper over the gap. An edge whose content
the grantee's target cannot read is an unresolved edge under ADR 0014, so a
grant made without the matching repository access degrades to a no-op that
**Reconciliation** reports, rather than to a failed apply or a silent partial
materialization.
