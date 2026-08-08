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

const blankStatus = (mode: CodexAuthMode, status: CodexAuthStatus["status"], checkedAt = new Date().toISOString(), detail: string | null = null, hints: { loginUrl?: string | null; deviceCode?: string | null } = {}): CodexAuthStatus =>
  codexAuthStatusSchema.parse({ mode, status, checkedAt, detail, account: null, ...hints });

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

async function readPreview(stream: ReadableStream<Uint8Array> | string | number | undefined, timeoutMs = 1500) {
  if (!stream || typeof stream === "number") return "";
  if (typeof stream === "string") return stream;
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let text = "";
  const deadline = Date.now() + timeoutMs;
  try {
    while (Date.now() < deadline) {
      const result = await Promise.race([
        reader.read(),
        new Promise<ReadableStreamReadResult<Uint8Array> | null>((resolve) => setTimeout(() => resolve(null), Math.max(1, deadline - Date.now()))),
      ]);
      if (!result || result.done) break;
      text += decoder.decode(result.value, { stream: true });
      if (/https:\/\/auth\.openai\.com\/codex\/device/i.test(text) && /\b[A-Z0-9]{4,8}-[A-Z0-9]{4,8}\b/.test(text)) break;
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
  return text + decoder.decode();
}

function deviceHints(output: string) {
  const clean = output.replace(/\u001b\[[0-9;]*m/g, "").replace(/\s+/g, " ").trim();
  const loginUrl = clean.match(/https:\/\/auth\.openai\.com\/codex\/device/i)?.[0] ?? null;
  const deviceCode = clean.match(/\b[A-Z0-9]{4,8}-[A-Z0-9]{4,8}\b/)?.[0] ?? null;
  const detail = loginUrl && deviceCode
    ? `Open ${loginUrl} and enter code ${deviceCode}. It expires in 15 minutes.`
    : safeOutput(clean) || "Device login started. Follow the Codex sign-in instructions.";
  return { loginUrl, deviceCode, detail };
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
  let loginUrl: string | null = null;
  let deviceCode: string | null = null;
  let loginDetail: string | null = null;

  const status = async (): Promise<CodexAuthStatus> => {
    const checkedAt = new Date().toISOString();
    if (mode === "api-key") return blankStatus(mode, options.apiKeyConfigured ? "configured" : "missing", checkedAt, options.apiKeyConfigured ? null : "CODEX_API_KEY is not configured");
    if (loginInFlight && !loginInFlight.killed) return blankStatus(mode, "starting", checkedAt, loginDetail ?? "Device login is in progress", { loginUrl, deviceCode });
    let child: Bun.Subprocess;
    try {
      child = spawn([executable, "login", "status"], { cwd: options.cwd, env: envFor(options), stdin: "ignore", stdout: "pipe", stderr: "pipe" });
      const stdout = child.stdout && typeof child.stdout !== "number" ? streamText(child.stdout) : Promise.resolve("");
      const stderr = child.stderr && typeof child.stderr !== "number" ? streamText(child.stderr) : Promise.resolve("");
      const [exitCode, out, err] = await Promise.all([child.exited, stdout, stderr]);
      const detail = safeOutput((out || err).trim()) || null;
      const combined = `${out}\n${err}`;
      if (exitCode === 0 && /logged\s+in|authenticated/i.test(combined)) {
        loginUrl = null; deviceCode = null; loginDetail = null;
        return blankStatus(mode, "authenticated", checkedAt, detail);
      }
      if (/not\s+logged|not\s+authenticated|no\s+login/i.test(combined)) return blankStatus(mode, "not_authenticated", checkedAt, detail);
      return blankStatus(mode, exitCode === 0 ? "unknown" : "failed", checkedAt, detail ?? `codex login status exited with ${exitCode}`);
    } catch (caught) {
      return blankStatus(mode, "failed", checkedAt, safeOutput(caught instanceof Error ? caught.message : "Unable to check Codex login status"));
    }
  };

  const startDeviceLogin = async (): Promise<CodexAuthStatus> => {
    const startedAt = new Date().toISOString();
    if (mode === "api-key") return blankStatus(mode, options.apiKeyConfigured ? "configured" : "missing", startedAt, "Device login is disabled in api-key mode");
    if (loginInFlight && !loginInFlight.killed) return blankStatus(mode, "starting", startedAt, loginDetail ?? "Device login is already in progress", { loginUrl, deviceCode });
    try {
      const child = spawn([executable, "login", "--device-auth"], { cwd: options.cwd, env: envFor(options), stdin: "ignore", stdout: "pipe", stderr: "pipe" });
      loginInFlight = child;
      const stdout = child.stdout && typeof child.stdout !== "number" ? readPreview(child.stdout) : Promise.resolve("");
      const stderr = child.stderr && typeof child.stderr !== "number" ? readPreview(child.stderr) : Promise.resolve("");
      const output = (await Promise.all([stdout, stderr])).join("\n");
      const hints = deviceHints(output);
      loginUrl = hints.loginUrl; deviceCode = hints.deviceCode; loginDetail = hints.detail;
      void child.exited.then(() => { if (loginInFlight === child) loginInFlight = undefined; });
      return blankStatus(mode, "starting", startedAt, loginDetail, { loginUrl, deviceCode });
    } catch (caught) {
      loginInFlight = undefined;
      return blankStatus(mode, "failed", startedAt, safeOutput(caught instanceof Error ? caught.message : "Unable to start Codex device login"));
    }
  };

  return { status, startDeviceLogin };
}
