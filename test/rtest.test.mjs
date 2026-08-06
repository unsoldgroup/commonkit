import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, realpath, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const repositoryRoot = new URL("..", import.meta.url).pathname;
const rtest = join(repositoryRoot, "scripts", "rtest");
const worker = join(repositoryRoot, "scripts", "commonkit-rtest-worker");

async function executable(path, source) {
  await writeFile(path, source);
  await chmod(path, 0o755);
}

async function localHarness() {
  const directory = await mkdtemp(join(tmpdir(), "commonkit-rtest-"));
  const bin = join(directory, "bin");
  const calls = join(directory, "calls");
  await mkdir(bin);
  await executable(
    join(bin, "git"),
    `#!/bin/sh\nprintf '%s\\n' '${directory}/fixture repo'\n`,
  );
  await executable(
    join(bin, "rsync"),
    `#!/bin/sh\nprintf '%s\\0' \"$@\" > '${calls}.rsync'\n`,
  );
  await executable(
    join(bin, "ssh"),
    `#!/bin/sh\nprintf '%s\\0' \"$@\" > '${calls}.ssh'\ncat > '${calls}.stdin'\n`,
  );
  return { directory, bin, calls };
}

test("rtest sends pnpm arguments as NUL-delimited stdin instead of shell text", async () => {
  const harness = await localHarness();
  const result = spawnSync(rtest, ["pnpm", "exec", "vitest", "run", "test/a b.test.ts"], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${harness.bin}:/usr/bin:/bin`,
      RTEST_LOCK_ROOT: harness.directory,
      RTEST_VPS: "developer@build.example.test",
    },
  });

  assert.equal(result.status, 0, result.stderr ?? result.error?.message);
  const ssh = (await readFile(`${harness.calls}.ssh`)).toString().split("\0").filter(Boolean);
  assert.equal(ssh.at(-2), "/root/bin/commonkit-rtest-worker");
  assert.match(ssh.at(-1), /^\/root\/rtest\/fixture-repo-[a-f0-9]{12}$/);
  assert.deepEqual(
    (await readFile(`${harness.calls}.stdin`)).toString().split("\0").filter(Boolean),
    ["pnpm", "exec", "vitest", "run", "test/a b.test.ts"],
  );
});

test("rtest defaults to pnpm test and rejects non-pnpm commands", async () => {
  const harness = await localHarness();
  const base = {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${harness.bin}:/usr/bin:/bin`,
      RTEST_LOCK_ROOT: harness.directory,
      RTEST_VPS: "developer@build.example.test",
    },
  };

  const defaultRun = spawnSync(rtest, [], base);
  assert.equal(defaultRun.status, 0, defaultRun.stderr ?? defaultRun.error?.message);
  assert.deepEqual(
    (await readFile(`${harness.calls}.stdin`)).toString().split("\0").filter(Boolean),
    ["pnpm", "test"],
  );

  const denied = spawnSync(rtest, ["bash", "-c", "echo unsafe"], base);
  assert.equal(denied.status, 64);
  assert.match(denied.stderr, /only pnpm commands are allowed/);
});

test("rtest excludes generated and platform-specific build artifacts", async () => {
  const source = await readFile(rtest, "utf8");

  for (const excluded of [".build/", "target/", "src-tauri/binaries/", ".cache/"]) {
    assert.ok(source.includes(`--exclude='${excluded}'`), `missing ${excluded} exclusion`);
  }
});

test("remote worker serializes jobs and applies the memory and CPU envelope", async () => {
  const directory = await mkdtemp(join(tmpdir(), "commonkit-rtest-worker-"));
  const bin = join(directory, "bin");
  const calls = join(directory, "systemd.args");
  const workspace = join(directory, "example-a1b2c3d4e5f6");
  await mkdir(bin);
  await mkdir(workspace);
  await executable(join(bin, "flock"), "#!/bin/sh\nexit 0\n");
  await executable(
    join(bin, "systemd-run"),
    `#!/bin/sh\nprintf '%s\\0' \"$@\" > '${calls}'\n`,
  );

  const result = spawnSync(worker, [workspace], {
    input: Buffer.from("pnpm\0exec\0vitest\0run\0"),
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:/usr/bin:/bin`,
      RTEST_LOCK_FILE: join(directory, "lock"),
      RTEST_WORKSPACE_ROOT: directory,
    },
  });

  assert.equal(result.status, 0, result.stderr ?? result.error?.message);
  const args = (await readFile(calls)).toString().split("\0").filter(Boolean);
  assert.ok(args.includes("MemoryMax=8G"));
  assert.ok(args.includes("CPUQuota=600%"));
  assert.ok(args.includes("NODE_OPTIONS=--max-old-space-size=6144"));
  assert.ok(args.includes(await realpath(workspace)));
  assert.deepEqual(args.slice(-4), ["pnpm", "exec", "vitest", "run"]);
});

test("remote worker rejects invalid workspaces and commands", () => {
  const badWorkspace = spawnSync(worker, ["/tmp/not-rtest"], {
    input: Buffer.from("pnpm\0test\0"),
    encoding: "utf8",
  });
  assert.equal(badWorkspace.status, 64);
  assert.match(badWorkspace.stderr, /invalid workspace/);

  const directory = spawnSync("mktemp", ["-d"], { encoding: "utf8" }).stdout.trim();
  const workspace = join(directory, "example-a1b2c3d4e5f6");
  spawnSync("mkdir", [workspace]);
  const badCommand = spawnSync(worker, [workspace], {
    input: Buffer.from("bash\0-c\0echo unsafe\0"),
    encoding: "utf8",
    env: { ...process.env, RTEST_WORKSPACE_ROOT: directory },
  });
  assert.equal(badCommand.status, 64);
  assert.match(badCommand.stderr, /only pnpm commands are allowed/);
});
