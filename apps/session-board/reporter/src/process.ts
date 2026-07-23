export interface CommandResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export type RunCommand = (argv: string[]) => Promise<CommandResult>;

export const runCommand: RunCommand = async (argv) => {
  try {
    const { stdout, stderr } = await executeFile(argv[0]!, argv.slice(1), { encoding: "utf8" });
    return { exitCode: 0, stdout, stderr };
  } catch (error) {
    const failure = error as Error & { code?: number; stdout?: string; stderr?: string };
    return { exitCode: typeof failure.code === "number" ? failure.code : 1, stdout: failure.stdout ?? "", stderr: failure.stderr ?? failure.message };
  }
};

export function parseCommandJson(result: CommandResult, label: string): unknown {
  if (result.exitCode !== 0) throw new Error(`${label} failed: ${result.stderr.trim() || `exit ${result.exitCode}`}`);
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(`${label} returned invalid JSON`);
  }
}
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const executeFile = promisify(execFile);
