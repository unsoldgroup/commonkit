import { describe, expect, test } from "bun:test";
import { createCodexAuthManager } from "../src/codex-auth.js";

const stream = (text: string) => new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode(text)); controller.close(); } });

describe("Codex auth manager", () => {
  test("reports api-key mode without exposing the key and refuses device login", async () => {
    const manager = createCodexAuthManager({ mode: "api-key", apiKeyConfigured: false });
    const status = await manager.status();
    expect(status).toMatchObject({ mode: "api-key", status: "missing", detail: "CODEX_API_KEY is not configured" });
    expect(JSON.stringify(status)).not.toContain("sk-");
    expect((await manager.startDeviceLogin()).detail).toContain("disabled");
  });

  test("runs redacted subscription status and starts one device login at a time", async () => {
    const commands: string[][] = [];
    let login: any;
    const manager = createCodexAuthManager({ mode: "subscription", codexHome: "/tmp/codex-test", spawn(command, options) {
      commands.push(command);
      expect(options.env).toMatchObject({ HOME: "/tmp/codex-test", CODEX_HOME: "/tmp/codex-test" });
      expect(options.env).not.toHaveProperty("CODEX_API_KEY");
      if (command[2] === "status") return { exited: Promise.resolve(0), stdout: stream("Logged in using ChatGPT\naccess-sess-123456789012345\n"), stderr: stream("") } as any;
      login = { exited: new Promise<number>(() => undefined), killed: false, stdout: stream("Open https://auth.openai.com/codex/device\nEnter code GERY-PHROH\n"), stderr: stream("") };
      return login;
    } });
    expect((await manager.status()).status).toBe("authenticated");
    const firstLogin = manager.startDeviceLogin();
    const secondLogin = manager.startDeviceLogin();
    const started = await firstLogin;
    expect(started.status).toBe("starting");
    expect(started.loginUrl).toBe("https://auth.openai.com/codex/device");
    expect(started.deviceCode).toBe("GERY-PHROH");
    expect((await secondLogin).detail).toContain("already in progress");
    expect(commands).toEqual([["codex", "login", "status"], ["codex", "login", "--device-auth"]]);
    expect(JSON.stringify(started)).not.toContain("access-sess");
    login.killed = true;
  });
});
