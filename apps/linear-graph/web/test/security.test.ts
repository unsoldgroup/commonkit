import { describe, expect, test } from "bun:test";
import { setTextContent } from "../src/security.js";

describe("untrusted brief rendering", () => {
  test("keeps malicious markup and action tokens as text", () => {
    const element = { textContent: "", innerHTML: "" };
    const token = "graph-action-token";
    setTextContent(element, `<img src=x onerror=\"window.exfiltrate('${token}')\">`);
    expect(element.textContent).toContain(token);
    expect(element.innerHTML).toBe("");
  });
});
