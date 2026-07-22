import { DecisionExecutor, GatePoller } from "./actions.js";
import { loadConfig } from "./config.js";
import { HubClient } from "./hub-client.js";
import { FallbackStateSource, OrcaCliPollingStateSource, OrcaWebSocketStateSource } from "./state-source.js";
import { TailReader } from "./tail.js";

const config = await loadConfig();
let client: HubClient;
const decisions = new DecisionExecutor(() => client.actions);
const tails = new TailReader();
client = new HubClient({
  url: config.hubUrl,
  machineToken: config.machineToken,
  machineId: config.machineId,
  machineName: config.machineName,
  onDecision: (decision) => decisions.execute(decision),
  onTailRequest: (session) => tails.read(session),
});
const source = new FallbackStateSource(
  new OrcaWebSocketStateSource({ machineId: config.machineId }),
  new OrcaCliPollingStateSource({ machineId: config.machineId }),
);
const gates = new GatePoller(config.machineId, (actions) => client.setActions(actions));

client.start();
await Promise.all([source.start((sessions) => client.setSessions(sessions)), gates.start()]);

async function shutdown() {
  gates.stop();
  await source.stop();
  client.stop();
}
process.on("SIGINT", () => void shutdown().finally(() => process.exit(0)));
process.on("SIGTERM", () => void shutdown().finally(() => process.exit(0)));
