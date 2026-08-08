import { mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { Database } from "bun:sqlite";
import {
  analysisRunSchema, campaignSchema, drainMetricsSchema, executionRunSchema, focusBriefSchema, graphSnapshotSchema, issueSchema, triageDecisionSchema,
  type AnalysisRun, type Campaign, type DrainMetrics, type ExecutionRun, type FocusBrief, type GraphSnapshot, type TriageDecision, type ZoneId,
} from "@commonkit/linear-graph-protocol";
import { summarizeTeams } from "./normalizer.js";

export class GraphStore {
  readonly db: Database;
  constructor(path: string) {
    if (path !== ":memory:") mkdirSync(dirname(path), { recursive: true });
    this.db = new Database(path);
    this.db.exec("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");
    this.db.exec(`CREATE TABLE IF NOT EXISTS snapshots (id INTEGER PRIMARY KEY CHECK (id = 1), payload TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS topic_overrides (issue_id TEXT PRIMARY KEY, zone TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS focus_brief (id INTEGER PRIMARY KEY CHECK (id = 1), payload TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS analysis_runs (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS triage_decisions (issue_id TEXT PRIMARY KEY, payload TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS campaigns (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS drain_metrics (id INTEGER PRIMARY KEY CHECK (id = 1), payload TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS execution_runs (id TEXT PRIMARY KEY, payload TEXT NOT NULL);`);
  }

  saveSnapshot(snapshot: GraphSnapshot) {
    const value = graphSnapshotSchema.parse(snapshot);
    this.db.query("INSERT INTO snapshots (id, payload) VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload").run(JSON.stringify(value));
  }

  loadSnapshot(): GraphSnapshot | null {
    const row = this.db.query("SELECT payload FROM snapshots WHERE id=1").get() as { payload: string } | null;
    if (!row) return null;
    const payload = JSON.parse(row.payload) as Record<string, unknown>;
    // Snapshots written before the team-summary contract lack `teams`. Derive it
    // from their already-normalized nodes so a restart never discards the last good view.
    if (payload.teams === undefined) {
      const nodes = issueSchema.array().parse(payload.nodes ?? []);
      payload.teams = summarizeTeams(nodes);
    }
    return graphSnapshotSchema.parse(payload);
  }

  setTopicOverride(issueId: string, zone: ZoneId) {
    this.db.query("INSERT INTO topic_overrides(issue_id, zone) VALUES(?, ?) ON CONFLICT(issue_id) DO UPDATE SET zone=excluded.zone").run(issueId, zone);
  }

  topicOverrides(): Map<string, ZoneId> {
    const rows = this.db.query("SELECT issue_id, zone FROM topic_overrides").all() as Array<{ issue_id: string; zone: ZoneId }>;
    return new Map(rows.map((row) => [row.issue_id, row.zone]));
  }

  saveBrief(brief: FocusBrief) {
    const value = focusBriefSchema.parse(brief);
    this.db.query("INSERT INTO focus_brief(id, payload) VALUES(1, ?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload").run(JSON.stringify(value));
  }

  loadBrief(): FocusBrief | null {
    const row = this.db.query("SELECT payload FROM focus_brief WHERE id=1").get() as { payload: string } | null;
    return row ? focusBriefSchema.parse(JSON.parse(row.payload)) : null;
  }

  saveAnalysisRun(run: AnalysisRun) {
    const value = analysisRunSchema.parse(run);
    this.db.query("INSERT OR REPLACE INTO analysis_runs(id, payload) VALUES(?, ?)").run(value.id, JSON.stringify(value));
  }

  latestAnalysisRun(): AnalysisRun | null {
    const row = this.db.query("SELECT payload FROM analysis_runs ORDER BY rowid DESC LIMIT 1").get() as { payload: string } | null;
    return row ? analysisRunSchema.parse(JSON.parse(row.payload)) : null;
  }

  saveTriageDecision(decision: TriageDecision) {
    const value = triageDecisionSchema.parse(decision);
    this.db.query("INSERT INTO triage_decisions(issue_id, payload) VALUES(?, ?) ON CONFLICT(issue_id) DO UPDATE SET payload=excluded.payload").run(value.issueId, JSON.stringify(value));
  }

  loadTriageDecisions(): TriageDecision[] {
    const rows = this.db.query("SELECT payload FROM triage_decisions ORDER BY rowid ASC").all() as Array<{ payload: string }>;
    return rows.map((row) => triageDecisionSchema.parse(JSON.parse(row.payload)));
  }

  loadTriageDecision(issueId: string): TriageDecision | null {
    const row = this.db.query("SELECT payload FROM triage_decisions WHERE issue_id=?").get(issueId) as { payload: string } | null;
    return row ? triageDecisionSchema.parse(JSON.parse(row.payload)) : null;
  }

  saveCampaign(campaign: Campaign) {
    const value = campaignSchema.parse(campaign);
    this.db.query("INSERT INTO campaigns(id, payload) VALUES(?, ?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload").run(value.id, JSON.stringify(value));
  }

  loadCampaign(id: string): Campaign | null {
    const row = this.db.query("SELECT payload FROM campaigns WHERE id=?").get(id) as { payload: string } | null;
    return row ? campaignSchema.parse(JSON.parse(row.payload)) : null;
  }

  loadCampaigns(): Campaign[] {
    const rows = this.db.query("SELECT payload FROM campaigns ORDER BY rowid DESC").all() as Array<{ payload: string }>;
    return rows.map((row) => campaignSchema.parse(JSON.parse(row.payload)));
  }

  saveDrainMetrics(metrics: DrainMetrics) {
    const value = drainMetricsSchema.parse(metrics);
    this.db.query("INSERT INTO drain_metrics(id, payload) VALUES(1, ?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload").run(JSON.stringify(value));
  }

  loadDrainMetrics(): DrainMetrics | null {
    const row = this.db.query("SELECT payload FROM drain_metrics WHERE id=1").get() as { payload: string } | null;
    return row ? drainMetricsSchema.parse(JSON.parse(row.payload)) : null;
  }

  saveExecutionRun(run: ExecutionRun) {
    const value = executionRunSchema.parse(run);
    this.db.query("INSERT INTO execution_runs(id, payload) VALUES(?, ?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload").run(value.id, JSON.stringify(value));
  }

  loadExecutionRun(id: string): ExecutionRun | null {
    const row = this.db.query("SELECT payload FROM execution_runs WHERE id=?").get(id) as { payload: string } | null;
    return row ? executionRunSchema.parse(JSON.parse(row.payload)) : null;
  }

  loadExecutionRuns(): ExecutionRun[] {
    const rows = this.db.query("SELECT payload FROM execution_runs ORDER BY rowid DESC").all() as Array<{ payload: string }>;
    return rows.map((row) => executionRunSchema.parse(JSON.parse(row.payload)));
  }

  close() { this.db.close(); }
}
