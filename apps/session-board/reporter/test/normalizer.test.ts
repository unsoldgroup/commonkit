import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { sessionSchema } from "@commonkit/session-board-protocol";
import { normalizeWorktreePs } from "../src/normalizer.js";

const fixturePath = join(import.meta.dir, "../fixtures/worktree-ps.json");

describe("Orca worktree normalizer", () => {
  test("maps captured worktree and agent state into protocol sessions", async () => {
    const payload = await Bun.file(fixturePath).json();
    const sessions = normalizeWorktreePs(payload, "studio");

    expect(sessions).toHaveLength(2);
    expect(sessions[0]).toEqual({
      machineId: "studio",
      worktreeId: "commonkit::/Users/al/code/commonkit",
      repo: "commonkit",
      project: "commonkit",
      agent: "codex",
      paneKey: "pane-codex",
      state: "waiting",
      stateSince: "2026-07-21T19:59:59.000Z",
      title: "Build Mac reporter",
      lastOutputAt: "2026-07-21T20:00:01.000Z",
    });
    expect(sessions.every((session) => sessionSchema.safeParse(session).success)).toBe(true);
  });

  test("uses safe protocol fallbacks for unknown agents and states", async () => {
    const payload = await Bun.file(fixturePath).json();
    const session = normalizeWorktreePs(payload, "studio")[1]!;
    expect(session.agent).toBe("other");
    expect(session.state).toBe("idle");
    expect(session.stateSince).toBe("2026-07-21T20:00:02.000Z");
  });

  test("rejects an unsuccessful Orca envelope", () => {
    expect(() => normalizeWorktreePs({ ok: false, error: { code: "runtime_unavailable" } }, "studio"))
      .toThrow("Orca worktree ps failed");
  });
});
