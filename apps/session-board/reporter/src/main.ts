import { CodexTerminalExecutor, DecisionExecutor, GatePoller } from "./actions.js";
import { ClaudeHookServer } from "./claude-hook-server.js";
import { loadConfig } from "./config.js";
import { HubClient } from "./hub-client.js";
import { FallbackStateSource, OrcaCliPollingStateSource, OrcaWebSocketStateSource } from "./state-source.js";
import { TailReader } from "./tail.js";
import { OrcaSteerer } from "./steer.js";

const config = await loadConfig();
let client: HubClient;
let hook: ClaudeHookServer;
const codex = new CodexTerminalExecutor(undefined, ({ actionId, outcome }) => client.closeAction(actionId, outcome));
const decisions = new DecisionExecutor(() => client.actions, codex);
const tails = new TailReader();
const steerer = new OrcaSteerer();
client = new HubClient({
  url: config.hubUrl,
  machineToken: config.machineToken,
  machineId: config.machineId,
  machineName: config.machineName,
  onDecision: async (decision) => {
    const action = client.actions.get(decision.actionId);
    if (action?.kind === "claude-permission" && hook.resolve(decision)) return;
    await decisions.execute(decision);
  },
  onTailRequest: (session) => tails.read(session),
});
hook = new ClaudeHookServer({
  machineId: config.machineId,
  port: config.hookPort,
  openAction: (action) => client.openAction(action),
  closeAction: (actionId, outcome) => client.closeAction(actionId, outcome),
  steer: (cwd, text) => steerer.send(cwd, text),
});
const source = new FallbackStateSource(
  new OrcaWebSocketStateSource({ machineId: config.machineId }),
  new OrcaCliPollingStateSource({ machineId: config.machineId }),
);
const gates = new GatePoller(config.machineId, (actions) => client.setActions(actions));

client.start();
hook.start();
await Promise.all([source.start((sessions) => client.setSessions(sessions)), gates.start()]);

async function shutdown() {
  gates.stop();
  await source.stop();
  client.stop();
  await hook.stop();
}
process.on("SIGINT", () => void shutdown().finally(() => process.exit(0)));
process.on("SIGTERM", () => void shutdown().finally(() => process.exit(0)));
