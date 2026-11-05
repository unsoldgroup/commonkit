import { describe, expect, test } from "bun:test";
import { briefExcerpt, briefPresentation, renderBrief } from "../src/brief.js";
import { demoPayload, type GraphPayload } from "../src/protocol.js";

function fixture(): GraphPayload {
  const payload = structuredClone(demoPayload);
  payload.brief = { text: "Original manual brief\n".repeat(200), source: "manual", status: "stale", snapshotAt: "2026-01-01T00:00:00Z" };
  return payload;
}

describe("concise daily brief", () => {
  test("uses the first eligible recommendation in server order, even with history", () => {
    const payload = fixture();
    const first = payload.recommendations[0]!;
    payload.snapshot.nodes.find((node) => node.id === first.issueId)!.statusType = "completed";
    payload.recommendations.unshift({ ...first, issueId: "missing" });
    const expected = payload.recommendations.find((rec) => payload.snapshot.nodes.some((node) => node.id === rec.issueId && node.statusType !== "completed" && node.statusType !== "canceled"))!;
    expect(briefPresentation(payload).task?.id).toBe(expected.issueId);
    expect(payload.recommendations[0]!.issueId).toBe("missing");
  });

  test("bounds title, reasons and unbroken strings without changing originals", () => {
    const payload = fixture();
    const rec = payload.recommendations[0]!;
    const node = payload.snapshot.nodes.find((node) => node.id === rec.issueId)!;
    node.title = "Long title ".repeat(200);
    rec.whyNow = "reason ".repeat(200);
    rec.nextAction = "x".repeat(2000);
    const result = briefPresentation(payload);
    const task = result.task!;
    expect(`${task.title} Why now: ${task.why} Next: ${task.next}`.split(/\s+/).length).toBeLessThanOrEqual(50);
    expect(task.title.length + task.why.length + task.next.length).toBeLessThan(350);
    expect(task.full).toContain(node.title);
    expect(task.full).toContain(rec.whyNow);
    expect(task.full).toContain(rec.nextAction);
    expect(result.analysis).toBe(payload.brief!.text);
    expect(briefExcerpt("😀".repeat(200), 14, 10)).toBe("😀".repeat(10) + "…");
  });

  test("empty, loading and unavailable states never invent a task", () => {
    const payload = fixture();
    expect(briefPresentation(payload, true).task).toBeUndefined();
    expect(briefPresentation(payload, false, true).task).toBeUndefined();
    payload.recommendations = [];
    expect(briefPresentation(payload).empty).toContain("No eligible recommendation");
    expect(briefPresentation(payload).task).toBeUndefined();
  });

  test("retains original manual analysis and stale/failure provenance", () => {
    const payload = fixture();
    payload.snapshot.stale = true;
    payload.analysis = { id: "failed", startedAt: "2026-01-01T00:00:00Z", status: "failed", error: "Enrichment failed" };
    const result = briefPresentation(payload);
    expect(result.status).toContain("Snapshot stale");
    expect(result.status).toContain("Analysis stale");
    expect(result.status).toContain("Last analysis failed");
    expect(result.provenance).toContain("Manual focus brief (original)");
    expect(result.provenance).toContain(payload.brief!.snapshotAt!);
    expect(result.analysis).toBe(payload.brief!.text);
  });

  test("renders hostile values exclusively through text, preserving full Details", () => {
    const payload = fixture();
    const hostile = '<img src=x onerror="alert(1)"><script>alert(1)</script>';
    const rec = payload.recommendations[0]!;
    payload.snapshot.nodes.find((node) => node.id === rec.issueId)!.title = hostile;
    rec.whyNow = hostile;
    payload.brief!.text = hostile.repeat(100);
    const elements = new Map<string, any>();
    const element = (): any => ({ textContent: "", dataset: {}, children: [], set innerHTML(_value: string) { throw new Error("Untrusted HTML sink"); }, replaceChildren() { this.children = []; }, append(child: any) { this.children.push(child); } });
    for (const id of ["#brief-copy", "#brief-status", "#brief-task-full", "#brief-provenance", "#brief-analysis"]) elements.set(id, element());
    const root = { ownerDocument: { createElement: element }, querySelector: (id: string) => elements.get(id) };
    renderBrief(root as unknown as HTMLElement, briefPresentation(payload));
    expect(elements.get("#brief-analysis").textContent).toBe(payload.brief!.text);
    expect(elements.get("#brief-task-full").textContent).toContain(hostile);
    expect(elements.get("#brief-copy").children[0].textContent).toContain("<img");
  });
});
