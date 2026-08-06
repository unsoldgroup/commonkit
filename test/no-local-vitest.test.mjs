import assert from "node:assert/strict";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const hook = new URL("../scripts/no-local-vitest.mjs", import.meta.url).pathname;

function invoke(command, cwd) {
  return spawnSync(process.execPath, [hook], {
    cwd,
    encoding: "utf8",
    input: JSON.stringify({ tool_name: "Bash", tool_input: { command } }),
  });
}

test("blocks direct local Vitest and points to rtest", async () => {
  const cwd = await mkdtemp(join(tmpdir(), "no-local-vitest-"));
  const result = invoke("pnpm exec vitest run test/example.test.ts", cwd);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /rtest pnpm exec vitest run/);
});

test("blocks pnpm scripts that resolve to Vitest", async () => {
  const cwd = await mkdtemp(join(tmpdir(), "no-local-vitest-package-"));
  await writeFile(
    join(cwd, "package.json"),
    JSON.stringify({
      scripts: {
        test: "vitest run",
        "test:unit": "vitest run unit",
        unit: "pnpm run test:unit",
      },
    }),
  );

  assert.equal(invoke("pnpm test", cwd).status, 2);
  assert.equal(invoke("pnpm test:unit -- --runInBand", cwd).status, 2);
  assert.equal(invoke("pnpm run unit", cwd).status, 2);
});

test("allows rtest, non-Vitest tests, and search commands", async () => {
  const cwd = await mkdtemp(join(tmpdir(), "allow-local-tests-"));
  await mkdir(join(cwd, "nested"));
  await writeFile(join(cwd, "package.json"), JSON.stringify({ scripts: { test: "node --test" } }));

  assert.equal(invoke("rtest pnpm exec vitest run test/example.test.ts", cwd).status, 0);
  assert.equal(invoke("pnpm test", cwd).status, 0);
  assert.equal(invoke("rg -n 'vitest' package.json", cwd).status, 0);
});
