import assert from "node:assert/strict";
import test from "node:test";
import { withDeadline } from "../src/async-deadline.ts";

test("setup operations fail visibly instead of leaving the desktop spinner hung", async () => {
  await assert.rejects(
    withDeadline(new Promise(() => {}), 5, "Setup timed out"),
    /Setup timed out/,
  );
});

test("setup operations return normally before the deadline", async () => {
  assert.equal(await withDeadline(Promise.resolve("ready"), 50, "timed out"), "ready");
});
