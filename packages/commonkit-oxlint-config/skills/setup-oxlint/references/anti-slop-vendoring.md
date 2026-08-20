# Reviewed anti-slop vendoring

Treat anti-slop as repository source, not a CommonKit default. Start only after
the repository owner selects rules and accepts the JavaScript-plugin execution
surface.

1. Stage upstream commit `6d538555cb151d4121ed51a27db81890eacf8ae9`
   outside the repository. Verify the commit, MIT license, and upstream
   `LICENSE` notice.
2. Review the complete source and rule tests. Add focused valid and invalid
   cases for `no-chained-type-assertions`, `no-shape-in-symbol-names`, and
   `no-unknown-parameters` before considering those rules.
3. Copy reviewed files into a new project-local directory. Stop if the
   destination exists. Compare updates as a three-way reviewed change; preserve
   local edits.
4. Copy the upstream `LICENSE` notice and
   `../assets/anti-slop.provenance.json` beside the vendored source. Set both
   destinations, record the notice digest and each retained source file as
   `sha256:<digest>`, and record the reviewer, date, enabled rules, and
   classification counts.
5. Pin `oxlint`, `@oxlint/plugins`, and the upstream commit as one compatibility
   tuple. The template records upstream's `1.78.0` tuple and CommonKit's native
   `1.79.0` baseline separately; prove the selected tuple with project canaries
   before marking the review complete.
6. Enable a small reviewed subset at warning severity. Run without automatic
   fixes. Classify every finding as defect, useful policy, acceptable exception,
   or false positive before promotion.

Keep project exceptions in the project config. Keep upstream source unchanged
so provenance and updates remain reviewable.
