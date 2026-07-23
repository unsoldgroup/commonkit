import type { SessionRef } from "@commonkit/session-board-protocol";
import { parseCommandJson, runCommand, type RunCommand } from "./process.js";

type Value = Record<string, unknown>;
const object = (value: unknown) => value && typeof value === "object" ? value as Value : undefined;

export class TailReader {
  constructor(private readonly run: RunCommand = runCommand) {}

  async read(ref: SessionRef): Promise<string[]> {
    const listed = object(parseCommandJson(await this.run(["orca", "terminal", "list", "--json"]), "orca terminal list"));
    const terminals = object(listed?.result)?.terminals;
    if (!Array.isArray(terminals)) return [];
    const terminal = terminals.map(object).find((item) => item?.worktreeId === ref.worktreeId && (item.paneKey === undefined || item.paneKey === ref.paneKey));
    if (typeof terminal?.handle !== "string") return [];
    const read = object(parseCommandJson(await this.run(["orca", "terminal", "read", "--terminal", terminal.handle, "--json"]), "orca terminal read"));
    const tail = object(object(read?.result)?.terminal)?.tail;
    return Array.isArray(tail) ? tail.filter((line): line is string => typeof line === "string").slice(-120) : [];
  }
}
