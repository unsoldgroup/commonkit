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

test("allows commands that only describe a Vitest run in prose", async () => {
  const cwd = await mkdtemp(join(tmpdir(), "no-local-vitest-prose-"));

  // The exact shape that blocked `gh pr create` on 2026-09-03: a heredoc PR body
  // documenting the command that was run on the remote runner.
  const heredoc = [
    "gh pr create --title 'Fix' --body \"$(cat <<'EOF'",
    "## Verification",
    "- `rtest pnpm exec vitest related --run` -- 116 tests passed",
    "EOF",
    ')"',
  ].join("\n");
  assert.equal(invoke(heredoc, cwd).status, 0);

  // Same mention carried by a text flag rather than a heredoc.
  assert.equal(
    invoke('git commit -m "note: run vitest via rtest, never locally"', cwd).status,
    0,
  );
  assert.equal(
    invoke("gh pr create --body 'ran pnpm exec vitest on the VPS'", cwd).status,
    0,
  );
});

test("still blocks a real Vitest run that appears alongside prose", async () => {
  const cwd = await mkdtemp(join(tmpdir(), "no-local-vitest-mixed-"));

  // Stripping prose must not become a way to smuggle a real invocation past the
  // hook -- the command outside the quoted body still runs.
  assert.equal(
    invoke('git commit -m "describing vitest" && pnpm exec vitest run', cwd).status,
    2,
  );
  // A quoted string that is NOT a text-carrying flag value is still scanned.
  assert.equal(invoke('ssh box "pnpm exec vitest run"', cwd).status, 2);
});
