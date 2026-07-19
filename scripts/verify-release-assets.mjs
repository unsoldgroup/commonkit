import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const root = process.argv[2];
if (!root) throw new Error("usage: verify-release-assets.mjs <artifact-directory>");

const names = await readdir(root);
const required = ["SHA256SUMS", "SHA256SUMS.sig", "latest.json"];
for (const name of required) {
  if (!names.includes(name)) throw new Error(`release artifact missing: ${name}`);
}
if (!names.some((name) => name.endsWith(".spdx.json"))) throw new Error("release SBOM missing");
for (const [label, predicate] of [
  ["macOS DMG", (name) => name.endsWith(".dmg")],
  ["macOS updater", (name) => name.endsWith(".app.tar.gz")],
  ["macOS CLI", (name) => name === "commonkit-macos-universal"],
  ["macOS daemon", (name) => name === "commonkitd-macos-universal"],
  ["Linux AppImage", (name) => name.endsWith(".AppImage")],
  ["Linux Debian package", (name) => name.endsWith(".deb")],
  ["Linux CLI", (name) => name === "commonkit-linux-x86_64"],
  ["Linux daemon", (name) => name === "commonkitd-linux-x86_64"],
  ["Windows NSIS installer", (name) => name.endsWith("-setup.exe")],
  ["Windows updater", (name) => name.endsWith(".nsis.zip")],
  ["Windows CLI", (name) => name === "commonkit-windows-x86_64.exe"],
  ["Windows daemon", (name) => name === "commonkitd-windows-x86_64.exe"],
]) {
  if (!names.some(predicate)) throw new Error(`${label} release payload missing`);
}

const updater = JSON.parse(await readFile(join(root, "latest.json"), "utf8"));
const expectedPlatforms = ["darwin-aarch64", "darwin-x86_64", "linux-x86_64", "windows-x86_64"];
if (!updater.version || !updater.notes?.trim() || !updater.platforms ||
    JSON.stringify(Object.keys(updater.platforms).sort()) !== JSON.stringify(expectedPlatforms)) {
  throw new Error("latest.json is incomplete");
}
for (const platform of expectedPlatforms) {
  const entry = updater.platforms[platform];
  if (!entry.url?.startsWith("https://") || !entry.signature?.trim()) {
    throw new Error(`latest.json entry is invalid: ${platform}`);
  }
}
for (const payload of names.filter((name) => !name.startsWith("SHA256SUMS"))) {
  if (!names.includes(`${payload}.sig`) && /(?:app\.tar\.gz|AppImage|nsis\.zip)$/.test(payload)) {
    throw new Error(`updater signature missing: ${payload}.sig`);
  }
}
const checksums = await readFile(join(root, "SHA256SUMS"), "utf8");
for (const payload of names.filter((name) => !name.startsWith("SHA256SUMS"))) {
  if (!checksums.split("\n").some((line) => line.endsWith(`  ${payload}`))) {
    throw new Error(`checksum missing for release payload: ${payload}`);
  }
}
