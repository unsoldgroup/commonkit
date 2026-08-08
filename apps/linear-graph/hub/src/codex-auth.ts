import { codexAuthModeSchema, codexAuthStatusSchema, type CodexAuthMode, type CodexAuthStatus } from "@commonkit/linear-graph-protocol";

export interface CodexAuthOptions {
  mode?: CodexAuthMode;
  executable?: string;
  cwd?: string;
  codexHome?: string;
  apiKeyConfigured?: boolean;
  spawn?: (command: string[], options: { cwd?: string; env: Record<string, string>; stdin: "ignore"; stdout: "pipe"; stderr: "pipe" }) => Bun.Subprocess;
}

export interface CodexAuthManager {
  status(): Promise<CodexAuthStatus>;
  startDeviceLogin(): Promise<CodexAuthStatus>;
}

const blankStatus = (mode: CodexAuthMode, status: CodexAuthStatus["status"], checkedAt = new Date().toISOString(), detail: string | null = null): CodexAuthStatus =>
  codexAuthStatusSchema.parse({ mode, status, checkedAt, detail, account: null });

function safeOutput(value: string) {
  return value.replace(/(?:sk|rk|sess|access|refresh)-[A-Za-z0-9._-]{12,}/gi, "[redacted]")
    .replace(/(?:Bearer\s+)[A-Za-z0-9._-]+/gi, "Bearer [redacted]")
    .slice(-1200);
}

async function streamText(stream: ReadableStream<Uint8Array> | string | number | undefined) {
  if (!stream || typeof stream === "number") return "";
  if (typeof stream === "string") return stream;
  return new Response(stream).text();
}

function envFor(options: CodexAuthOptions) {
  return {
    PATH: process.env.PATH ?? "/usr/local/bin:/usr/bin:/bin",
    ...(options.codexHome ? { CODEX_HOME: options.codexHome } : {}),
  };
}

export function createCodexAuthManager(options: CodexAuthOptions = {}): CodexAuthManager {
  const mode = codexAuthModeSchema.parse(options.mode ?? "subscription");
  const executable = options.executable ?? "codex";
  const spawn = options.spawn ?? ((command, spawnOptions) => Bun.spawn(command, spawnOptions));
  let loginInFlight: Bun.Subprocess | undefined;

  const status = async (): Promise<CodexAuthStatus> => {
    const checkedAt = new Date().toISOString();
    if (mode === "api-key") return blankStatus(mode, options.apiKeyConfigured ? "configured" : "missing", checkedAt, options.apiKeyConfigured ? null : "CODEX_API_KEY is not configured");
    if (loginInFlight && !loginInFlight.killed) return blankStatus(mode, "starting", checkedAt, "Device login is in progress");
    let child: Bun.Subprocess;
    try {
      child = spawn([executable, "login", "status"], { cwd: options.cwd, env: envFor(options), stdin: "ignore", stdout: "pipe", stderr: "pipe" });
      const stdout = child.stdout && typeof child.stdout !== "number" ? streamText(child.stdout) : Promise.resolve("");
      const stderr = child.stderr && typeof child.stderr !== "number" ? streamText(child.stderr) : Promise.resolve("");
      const [exitCode, out, err] = await Promise.all([child.exited, stdout, stderr]);
      const detail = safeOutput((out || err).trim()) || null;
      if (exitCode === 0 && /logged\s+in|authenticated/i.test(out)) return blankStatus(mode, "authenticated", checkedAt, detail);
      if (/not\s+logged|not\s+authenticated|no\s+login/i.test(`${out}\n${err}`)) return blankStatus(mode, "not_authenticated", checkedAt, detail);
      return blankStatus(mode, exitCode === 0 ? "unknown" : "failed", checkedAt, detail ?? `codex login status exited with ${exitCode}`);
    } catch (caught) {
      return blankStatus(mode, "failed", checkedAt, safeOutput(caught instanceof Error ? caught.message : "Unable to check Codex login status"));
    }
  };

  const startDeviceLogin = async (): Promise<CodexAuthStatus> => {
    const startedAt = new Date().toISOString();
    if (mode === "api-key") return blankStatus(mode, options.apiKeyConfigured ? "configured" : "missing", startedAt, "Device login is disabled in api-key mode");
    if (loginInFlight && !loginInFlight.killed) return blankStatus(mode, "starting", startedAt, "Device login is already in progress");
    try {
      const child = spawn([executable, "login", "--device-auth"], { cwd: options.cwd, env: envFor(options), stdin: "ignore", stdout: "pipe", stderr: "pipe" });
      loginInFlight = child;
      const stdout = child.stdout && typeof child.stdout !== "number" ? streamText(child.stdout) : Promise.resolve("");
      const stderr = child.stderr && typeof child.stderr !== "number" ? streamText(child.stderr) : Promise.resolve("");
      void Promise.all([child.exited, stdout, stderr]).then(() => { if (loginInFlight === child) loginInFlight = undefined; });
      return blankStatus(mode, "starting", startedAt, "Device login started. Complete the Codex device flow on the VPS or use the displayed device instructions.");
    } catch (caught) {
      loginInFlight = undefined;
      return blankStatus(mode, "failed", startedAt, safeOutput(caught instanceof Error ? caught.message : "Unable to start Codex device login"));
    }
  };

  return { status, startDeviceLogin };
}
