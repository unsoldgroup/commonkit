# The work graph owns the taxonomy

The linear-graph hub assigns every issue a **Zone** and a set of **Topic tags**
once per sync, from a Codex pass with a keyword fallback in `normalizer.ts`.
Consumers render that assignment and never classify issues themselves.

A consumer that re-derives its own grouping produces a second taxonomy that
silently disagrees with the first. This was not hypothetical: a backlog index
built its own twelve keyword buckets, and each bucket smeared across three to
six zones — its "scraper" bucket was 28% platform, 27% product, 23% operations.
Two classifiers over one backlog means neither answer can be trusted, and
neither is visibly wrong.

Where the graph's assignment is worse than a local guess, that is a defect in
the hub — a missing fallback keyword, or an issue left `unsorted` — and it gets
fixed in the hub, where every consumer benefits. Staleness is the same: a
consumer must be able to ask the hub to resync rather than patch around a stale
snapshot with hardcoded issue lists.

## Consequences

- A consumer's grouping code is a rendering of `zone` and `topicTags`. Keyword
  lists in consumers are a bug.
- `unsorted` is a visible defect queue, not a resting place. An issue with no
  **Topic tag** is invisible to every consumer that groups by them.
- **Cluster** stays independent of this: it comes from tracker relationships,
  not subject, so it is derived from edges by anyone who needs it.
