import type { GraphPayload } from "./protocol.js";

/** Bound words AND characters: identifiers and unbroken text also need a limit. */
export function briefExcerpt(value: string, words: number, characters: number): string {
  const text = value.replace(/\s+/gu, " ").trim();
  const excerpt = Array.from(text.split(" ").slice(0, words).join(" ")).slice(0, characters).join("");
  return excerpt.length < text.length ? `${excerpt.trimEnd()}…` : excerpt;
}

export function briefPresentation(payload: GraphPayload, loading = false, unavailable = false) {
  // Preserve the server's deterministic order. History never makes a closed task eligible.
  const nodes = new Map(payload.snapshot.nodes.filter((node) => node.statusType !== "completed" && node.statusType !== "canceled").map((node) => [node.id, node]));
  const recommendation = payload.recommendations.find((rec) => nodes.has(rec.issueId));
  const node = recommendation ? nodes.get(recommendation.issueId) : undefined;
  const task = !loading && !unavailable && node && recommendation ? {
    id: node.id,
    title: `${briefExcerpt(node.identifier, 1, 24)} · ${briefExcerpt(node.title, 14, 100)}`,
    why: briefExcerpt(recommendation.whyNow, 14, 110) || "No reason provided.",
    next: briefExcerpt(recommendation.nextAction, 14, 110) || "No next action provided.",
    full: `${node.identifier} · ${node.title}\n\nWhy now: ${recommendation.whyNow}\n\nNext action: ${recommendation.nextAction}`,
  } : undefined;
  const brief = payload.brief;
  return {
    task,
    empty: loading ? "Loading today’s brief…" : unavailable ? "Recommendations unavailable. Refresh to retry." : "No eligible recommendation in this snapshot. Review the open issues for your next task.",
    status: [payload.snapshot.stale ? "Snapshot stale" : "", brief?.status && brief.status !== "ready" ? `Analysis ${brief.status}` : "", payload.analysis?.status === "running" ? "Analysis running" : "", payload.analysis?.status === "failed" ? "Last analysis failed" : ""].filter(Boolean).join(" · "),
    analysis: brief?.text ?? "No saved analysis yet.",
    provenance: [brief?.source === "manual" ? "Manual focus brief (original)" : brief?.source === "codex" ? "Generated analysis" : brief?.source === "fallback" ? "Fallback analysis" : "Saved focus brief", brief?.generatedAt ? `Generated: ${brief.generatedAt}` : "", brief?.snapshotAt ? `Snapshot: ${brief.snapshotAt}` : "", brief?.updatedAt ? `Updated: ${brief.updatedAt}` : ""].filter(Boolean).join(" · "),
  };
}

/** All remote values cross the textContent boundary, including the full originals. */
export function renderBrief(root: HTMLElement, presentation: ReturnType<typeof briefPresentation>) {
  const doc = root.ownerDocument;
  const copy = root.querySelector<HTMLElement>("#brief-copy")!;
  copy.replaceChildren();
  if (presentation.task) {
    const button = doc.createElement("button");
    button.type = "button";
    button.className = "brief-task";
    button.dataset.node = presentation.task.id;
    button.textContent = presentation.task.title;
    copy.append(button);
    for (const [label, value] of [["Why now", presentation.task.why], ["Next", presentation.task.next]]) {
      const line = doc.createElement("p");
      line.textContent = `${label}: ${value}`;
      copy.append(line);
    }
  } else copy.textContent = presentation.empty;
  root.querySelector("#brief-status")!.textContent = presentation.status;
  root.querySelector("#brief-task-full")!.textContent = presentation.task?.full ?? "";
  root.querySelector("#brief-provenance")!.textContent = presentation.provenance;
  root.querySelector("#brief-analysis")!.textContent = presentation.analysis;
}
