import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import test from "node:test";

const harnessUrl = new URL("../apps/desktop/harness/harness.ts", import.meta.url);

test("the screenshot harness contains only synthetic target configuration", async () => {
  const harness = await readFile(harnessUrl, "utf8");

  assert.match(harness, /local-workstation/);
  assert.match(harness, /example-org\/my-commonkit/);
  assert.doesNotMatch(harness, /machineToken/i);
  assert.match(harness, /contractVersion: "commonkit\.panel\/v1"/);
  assert.match(harness, /runtimeVersion: "0\.2\.0"/);
});

test("the marketing site publishes responsive, privacy-safe product proof", async () => {
  const siteUrl = new URL("../docs/index.html", import.meta.url);
  const site = await readFile(siteUrl, "utf8");

  for (const marker of ["/Users/", "example-user", "machineToken"]) {
    assert.doesNotMatch(site, new RegExp(marker.replaceAll("/", "\\/"), "i"));
  }
  assert.match(site, /<picture>/);
  assert.match(site, /srcset="[^"]+640\.webp 640w,[^"]+960\.webp 960w,[^"]+\.webp 1535w"/s);
  assert.match(site, /loading="lazy"/);
  assert.match(site, /alt="CommonKit 0\.2\.0 status panel[^"<>]+"/);

  for (const asset of [
    "../docs/.nojekyll",
    "../docs/assets/commonkit-status-0.2.0.png",
    "../docs/assets/commonkit-status-0.2.0.webp",
    "../docs/assets/commonkit-status-0.2.0-960.webp",
    "../docs/assets/commonkit-status-0.2.0-640.webp",
  ]) {
    await access(new URL(asset, import.meta.url));
  }
});
