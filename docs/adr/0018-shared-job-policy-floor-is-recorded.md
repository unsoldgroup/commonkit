# A shared Job's policy floor is the two-principal intersection, and it is recorded

A **Job** submitted by one person onto an **Execution Target** owned by another
resolves its policy as the intersection of exactly two principals, the
submitter and the target owner: allowed egress hosts intersect, denied hosts
union, and the same narrowing applies to every other bounded control. Neither
person's policy can be widened by the other, which is the only property that
makes lending a target safe in both directions — the owner's machine cannot be
opened up by whoever submits to it, and the submitter's content cannot be run
under a policy looser than they accepted. `yc-software/qm` computes the same
floor over every participant in a conversation; CommonKit has no room, and the
two principals are the whole trust relationship. Rejected: intersecting over
all members of the submitting **Scope**, which imports qm's room semantics into
a system that has none and makes adding a teammate silently tighten everyone
else's jobs. Also rejected: letting the target owner's policy govern alone,
which is already safe for the owner and not for the submitter. Because the
resolved floor varies with who submitted, it is recorded in the terminal
**Attempt** receipt exactly as **Isolation level** is under ADR 0013:
**Validation evidence** produced under different floors states so, and is
therefore never silently compared. Recording rather than requiring is what
keeps a shared target useful without making its evidence dishonest.
