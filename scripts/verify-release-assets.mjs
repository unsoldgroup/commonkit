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
if (!names.some((name) => /commonkit/.test(name) && !name.endsWith(".sig"))) {
  throw new Error("CommonKit release payload missing");
}

const updater = JSON.parse(await readFile(join(root, "latest.json"), "utf8"));
if (!updater.version || !updater.platforms || Object.keys(updater.platforms).length === 0) {
  throw new Error("latest.json is incomplete");
}
