import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmod,
  copyFile,
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import test from "node:test";

const execFileAsync = promisify(execFile);

test("the public commonkit package contains only the native launcher and public metadata", async () => {
  const output = await mkdtemp(join(tmpdir(), "commonkit-npm-pack-"));
  try {
    const { stdout } = await execFileAsync(
      "pnpm",
      ["pack", "--pack-destination", output, "--json"],
      {
        cwd: new URL("../packages/commonkit-npm", import.meta.url),
        maxBuffer: 4 * 1024 * 1024,
      },
    );
    const packed = JSON.parse(stdout);
    const paths = packed.files.map(({ path }) => path).sort();
    const [archive] = await readdir(output);
    const { stdout: manifestText } = await execFileAsync(
      "tar",
      ["-xOf", join(output, archive), "package/package.json"],
    );
    const manifest = JSON.parse(manifestText);

    assert.deepEqual(paths, [
      "LICENSE",
      "README.md",
      "bin/commonkit-native.mjs",
      "package.json",
    ]);
    assert.deepEqual(manifest.bin, {
      commonkit: "bin/commonkit-native.mjs",
      commonkitd: "bin/commonkit-native.mjs",
      "commonkit-target-helper": "bin/commonkit-native.mjs",
    });
    assert.equal(manifest.scripts, undefined);
    assert.equal(manifest.packageManager, undefined);
  } finally {
    await rm(output, { recursive: true, force: true });
  }
});

test("the installed commonkit command executes the matching native binary and forwards its result", async () => {
  const prefix = await mkdtemp(join(tmpdir(), "commonkit-npm-install-"));
  try {
    const packageRoot = join(prefix, "node_modules", "commonkit");
    const nativeRoot = join(
      prefix,
      "node_modules",
      `commonkit-${process.platform}-${process.arch}`,
    );
    await mkdir(join(packageRoot, "bin"), { recursive: true });
    await mkdir(join(nativeRoot, "bin"), { recursive: true });
    await mkdir(join(prefix, "bin"), { recursive: true });
    await copyFile(
      new URL("../packages/commonkit-npm/bin/commonkit-native.mjs", import.meta.url),
      join(packageRoot, "bin", "commonkit-native.mjs"),
    );
    await writeFile(
      join(nativeRoot, "package.json"),
      JSON.stringify({ name: `commonkit-${process.platform}-${process.arch}` }),
    );
    const native = join(nativeRoot, "bin", "commonkit");
    const nativeContent = "#!/bin/sh\nprintf 'native:%s\\n' \"$*\"\nexit 23\n";
    await writeFile(native, nativeContent);
    await writeFile(
      join(nativeRoot, "checksums.json"),
      JSON.stringify({
        commonkit: `sha256:${createHash("sha256").update(nativeContent).digest("hex")}`,
      }),
    );
    await chmod(native, 0o755);
    const command = join(prefix, "bin", "commonkit");
    await symlink(join(packageRoot, "bin", "commonkit-native.mjs"), command);

    await assert.rejects(
      execFileAsync(command, ["--version", "--verbose"]),
      (error) => {
        assert.equal(error.code, 23);
        assert.equal(error.stdout, "native:--version --verbose\n");
        return true;
      },
    );
  } finally {
    await rm(prefix, { recursive: true, force: true });
  }
});

test("the launcher refuses a native executable that does not match its published digest", async () => {
  const prefix = await mkdtemp(join(tmpdir(), "commonkit-npm-tampered-"));
  try {
    const packageRoot = join(prefix, "node_modules", "commonkit");
    const nativeRoot = join(
      prefix,
      "node_modules",
      `commonkit-${process.platform}-${process.arch}`,
    );
    await mkdir(join(packageRoot, "bin"), { recursive: true });
    await mkdir(join(nativeRoot, "bin"), { recursive: true });
    await mkdir(join(prefix, "bin"), { recursive: true });
    await copyFile(
      new URL("../packages/commonkit-npm/bin/commonkit-native.mjs", import.meta.url),
      join(packageRoot, "bin", "commonkit-native.mjs"),
    );
    await writeFile(
      join(nativeRoot, "package.json"),
      JSON.stringify({ name: `commonkit-${process.platform}-${process.arch}` }),
    );
    const native = join(nativeRoot, "bin", "commonkit");
    await writeFile(native, "#!/bin/sh\nexit 0\n");
    await chmod(native, 0o755);
    await writeFile(
      join(nativeRoot, "checksums.json"),
      JSON.stringify({
        commonkit: `sha256:${createHash("sha256").update("expected").digest("hex")}`,
      }),
    );
    const command = join(prefix, "bin", "commonkit");
    await symlink(join(packageRoot, "bin", "commonkit-native.mjs"), command);

    await assert.rejects(execFileAsync(command), (error) => {
      assert.equal(error.code, 1);
      assert.match(error.stderr, /failed its SHA-256 integrity check/);
      return true;
    });
  } finally {
    await rm(prefix, { recursive: true, force: true });
  }
});

test("the launcher explains when npm did not install a supported native package", async () => {
  const prefix = await mkdtemp(join(tmpdir(), "commonkit-npm-missing-"));
  try {
    const packageRoot = join(prefix, "node_modules", "commonkit");
    await mkdir(join(packageRoot, "bin"), { recursive: true });
    await mkdir(join(prefix, "bin"), { recursive: true });
    await copyFile(
      new URL("../packages/commonkit-npm/bin/commonkit-native.mjs", import.meta.url),
      join(packageRoot, "bin", "commonkit-native.mjs"),
    );
    const command = join(prefix, "bin", "commonkit");
    await symlink(join(packageRoot, "bin", "commonkit-native.mjs"), command);

    await assert.rejects(execFileAsync(command), (error) => {
      assert.equal(error.code, 1);
      assert.match(
        error.stderr,
        new RegExp(`Expected optional dependency commonkit-${process.platform}-${process.arch}`),
      );
      return true;
    });
  } finally {
    await rm(prefix, { recursive: true, force: true });
  }
});

test("npm installs only the native payload matching the host platform and architecture", async () => {
  const root = JSON.parse(
    await readFile(new URL("../packages/commonkit-npm/package.json", import.meta.url)),
  );
  const cargoWorkspace = await readFile(
    new URL("../Cargo.toml", import.meta.url),
    "utf8",
  );
  const cargoVersion = cargoWorkspace.match(
    /\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/,
  )?.[1];
  assert.equal(root.version, cargoVersion);
  const platforms = [
    ["commonkit-darwin-arm64", "darwin", "arm64"],
    ["commonkit-linux-x64", "linux", "x64"],
  ];

  for (const [name, os, cpu] of platforms) {
    const manifest = JSON.parse(
      await readFile(new URL(`../packages/${name}/package.json`, import.meta.url)),
    );
    assert.equal(manifest.name, name);
    assert.equal(manifest.version, root.version);
    assert.deepEqual(manifest.os, [os]);
    assert.deepEqual(manifest.cpu, [cpu]);
    assert.deepEqual(manifest.files, ["bin", "checksums.json", "LICENSE", "README.md"]);
    assert.equal(root.optionalDependencies[name], `workspace:${root.version}`);
  }
});

test("native package staging records every executable by SHA-256 digest", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "commonkit-npm-binaries-"));
  const output = await mkdtemp(join(tmpdir(), "commonkit-npm-stage-"));
  try {
    const contents = new Map([
      ["commonkit", "cli"],
      ["commonkitd", "daemon"],
      ["commonkit-target-helper", "helper"],
    ]);
    for (const [name, content] of contents) {
      await writeFile(join(fixture, name), content);
      await chmod(join(fixture, name), 0o755);
    }
    await execFileAsync(process.execPath, [
      new URL("../scripts/stage-npm-native-package.mjs", import.meta.url).pathname,
      "commonkit-darwin-arm64",
      fixture,
      output,
    ]);

    const checksums = JSON.parse(
      await readFile(join(output, "checksums.json"), "utf8"),
    );
    assert.deepEqual(
      checksums,
      Object.fromEntries(
        [...contents].map(([name, content]) => [
          name,
          `sha256:${createHash("sha256").update(content).digest("hex")}`,
        ]),
      ),
    );
    for (const name of contents.keys()) {
      assert.equal(await readFile(join(output, "bin", name), "utf8"), contents.get(name));
    }
  } finally {
    await rm(fixture, { recursive: true, force: true });
    await rm(output, { recursive: true, force: true });
  }
});
