# A Principal is a GitHub login, brokered by the installed GitHub CLI

CommonKit's multiplayer model needs a **Principal**, which version 1 did not
have on any surface, and takes the identity it already depends on: a principal
is a GitHub login, proven through the installed GitHub CLI, which v1 already
resolved as the credential broker and repository-provisioning client. The CLI,
MCP, the desktop application, and the Session Board all resolve the acting
principal the same way, so who is driving a surface is one answer rather than
four. Choosing GitHub also collapses a cost ADR 0016 accepted: a **Grant** edge
between personal **Scopes** confers composition while Git read access comes
from elsewhere, and when the **Grant graph** keys on the same login that grants
repository access, the two systems that had to agree become one system with two
views. Rejected: extending `commonkit-execd`'s per-target worker tokens to
people, which is self-contained and provider-neutral and makes CommonKit own
principal enrollment, rotation, and revocation for no benefit at the scale it
serves. Also rejected: a first-party account system, which is the cleanest
long-term boundary and the most to build and operate for a handful of people.
The constraint accepted, stated plainly because it is a product boundary rather
than an implementation detail: multiplayer CommonKit requires GitHub. Solo
CommonKit does not, and nothing here adds an identity requirement to a
single-owner kit. Replacing the broker later — a first-party GitHub App, a
device flow, another provider — changes how a principal is proven and not what
a principal is, which is why the **Grant graph** stores the identity rather than
the proof.
