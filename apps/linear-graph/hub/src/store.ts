import { mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { Database } from "bun:sqlite";
import {
  analysisRunSchema, focusBriefSchema, graphSnapshotSchema, issueSchema, type AnalysisRun, type FocusBrief, type GraphSnapshot,
  type ZoneId,
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
      CREATE TABLE IF NOT EXISTS analysis_runs (id TEXT PRIMARY KEY, payload TEXT NOT NULL);`);
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

  close() { this.db.close(); }
}
