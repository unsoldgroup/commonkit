import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

describe("Linear graph service sandbox", () => {
  test("allows only explicit graph state and git metadata writes under ProtectHome", () => {
    const service = readFileSync(new URL("../../deploy/templates/linear-graph.service", import.meta.url), "utf8");
    expect(service).toContain("ProtectHome=read-only");
    expect(service).toContain("ReadWritePaths=%h/.local/share/commonkit/linear-graph @@DATA_DIR@@ @@REPO_DIR@@/.git");
  });
});
