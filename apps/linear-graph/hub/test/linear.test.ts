import { describe, expect, test } from "bun:test";
import { createLinearSource } from "../src/linear.js";

describe("Linear source", () => {
  test("is snapshot read-only and exposes no project or issue mutation client", () => {
    const source = createLinearSource({ token: "secret", fetcher: (async () => new Response(JSON.stringify({ data: { issues: { nodes: [], pageInfo: { hasNextPage: false } } } }))) as unknown as typeof fetch });
    expect("mutate" in source).toBe(false);
  });
});
