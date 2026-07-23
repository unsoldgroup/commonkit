import { expect, test } from "bun:test";
import type { Session } from "@commonkit/session-board-protocol";
import { FallbackStateSource, type StateSource } from "../src/state-source.js";

test("automatically selects the CLI fallback when WebSocket framing fails", async () => {
  const calls: string[] = [];
  const primary: StateSource = {
    name: "primary",
    async start() { calls.push("primary:start"); throw new Error("bad framing"); },
    async stop() { calls.push("primary:stop"); },
  };
  const fallback: StateSource = {
    name: "fallback",
    async start(listener) { calls.push("fallback:start"); await listener([] as Session[]); },
    async stop() { calls.push("fallback:stop"); },
  };
  const warnings: string[] = [];
  const source = new FallbackStateSource(primary, fallback, (warning) => warnings.push(warning));
  await source.start(() => undefined);
  expect(calls).toEqual(["primary:start", "primary:stop", "fallback:start"]);
  expect(warnings[0]).toContain("bad framing");
});
