import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("desktop window opens at a useful management size", async () => {
  const config = JSON.parse(
    await readFile(new URL("src-tauri/tauri.conf.json", root), "utf8"),
  );
  const window = config.app.windows[0];
  assert.ok(window.width >= 1180);
  assert.ok(window.height >= 760);
});

test("desktop shell uses a full-height sidebar and independently scrolling content", async () => {
  const css = await readFile(new URL("src/style.css", root), "utf8");
  assert.match(css, /#app\s*\{[^}]*height:\s*100(?:dvh|vh)/s);
  assert.match(css, /\.rail\s*\{[^}]*height:\s*100(?:dvh|vh)/s);
  assert.match(css, /\.deck\s*\{[^}]*overflow-y:\s*auto/s);
});
