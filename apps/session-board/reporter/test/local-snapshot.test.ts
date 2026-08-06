import { expect, test } from "bun:test";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeLocalSessionSnapshot } from "../src/local-snapshot.js";

test("publishes a daemon-readable session snapshot without credentials", async () => {
  const directory = await mkdtemp(join(tmpdir(), "commonkit-sessions-"));
  const path = join(directory, "nested", "sessions.json");
  await writeLocalSessionSnapshot(path, [], 1234);

  expect(JSON.parse(await readFile(path, "utf8"))).toEqual({
    contractVersion: "commonkit.agent-sessions/v1",
    observedAtUnixMs: 1234,
    sessions: [],
  });
});
