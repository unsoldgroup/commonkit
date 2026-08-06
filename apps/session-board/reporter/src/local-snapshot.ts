import { mkdir, rename, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import type { Session } from "@commonkit/session-board-protocol";

export async function writeLocalSessionSnapshot(
  path: string,
  sessions: Session[],
  observedAtUnixMs = Date.now(),
): Promise<void> {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${process.pid}.tmp`;
  const snapshot = {
    contractVersion: "commonkit.agent-sessions/v1",
    observedAtUnixMs,
    sessions,
  };
  await writeFile(temporary, `${JSON.stringify(snapshot)}\n`, { mode: 0o600 });
  await rename(temporary, path);
}
