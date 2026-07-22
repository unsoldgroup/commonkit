import { readFile } from "node:fs/promises";
import { homedir, hostname } from "node:os";
import { join } from "node:path";

interface FileConfig {
  hubUrl?: string;
  machineToken?: string;
  machineId?: string;
  machineName?: string;
}

export interface ReporterConfig {
  hubUrl: string;
  machineToken: string;
  machineId: string;
  machineName: string;
}

export async function loadConfig(env: NodeJS.ProcessEnv = process.env): Promise<ReporterConfig> {
  const path = env.SESSION_BOARD_CONFIG ?? join(homedir(), ".config/commonkit/session-board-reporter.json");
  let file: FileConfig = {};
  try { file = JSON.parse(await readFile(path, "utf8")) as FileConfig; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const hubUrl = env.SESSION_BOARD_HUB_URL ?? file.hubUrl;
  const machineToken = env.SESSION_BOARD_MACHINE_TOKEN ?? file.machineToken;
  if (!hubUrl || !machineToken) throw new Error("SESSION_BOARD_HUB_URL and SESSION_BOARD_MACHINE_TOKEN are required (env or config file)");
  return {
    hubUrl,
    machineToken,
    machineId: env.SESSION_BOARD_MACHINE_ID ?? file.machineId ?? hostname(),
    machineName: env.SESSION_BOARD_MACHINE_NAME ?? file.machineName ?? hostname(),
  };
}
