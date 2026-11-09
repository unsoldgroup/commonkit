import type { Epic, EpicChild } from "./linear.js";

export interface FocusMetricTile { key: string; label: string; value: string; detail: string }
export interface FocusMetrics { generatedAt: string; tiles: FocusMetricTile[] }

const TIME_ZONE = "Europe/Madrid";
// Tile order and labels when metrics.json is absent or lacks a key.
const DEFAULT_TILES = [
  { key: "first_response", label: "First response, median" },
  { key: "paid_to_bound", label: "Paid → bound, median" },
  { key: "prs_without_al", label: "PRs merged without Al" },
];

export const esc = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, (char) => `&#${char.charCodeAt(0)};`);

function madridParts(now: Date) {
  const parts = Object.fromEntries(new Intl.DateTimeFormat("en-GB", {
    timeZone: TIME_ZONE, weekday: "short", day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23",
  }).formatToParts(now).map((part) => [part.type, part.value]));
  const iso = new Intl.DateTimeFormat("en-CA", { timeZone: TIME_ZONE, year: "numeric", month: "2-digit", day: "2-digit" }).format(now);
  return { ...parts, iso } as Record<string, string>;
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const weekdayOf = (isoDate: string) => WEEKDAYS[new Date(`${isoDate}T12:00:00Z`).getUTCDay()];

export function checklist(child: EpicChild) {
  let done = 0; let open = 0;
  for (const comment of child.comments.nodes) {
    done += (comment.body.match(/^\s*[-*] \[[xX]\]/gm) ?? []).length;
    open += (comment.body.match(/^\s*[-*] \[ \]/gm) ?? []).length;
  }
  const total = done + open;
  const percent = total ? Math.round((done / total) * 100) : child.state.type === "completed" ? 100 : 0;
  return { done, total, percent };
}

/** "[Autonomy] A1. Send policy core" -> { code: "A1", letter: "A", title: "Send policy core" } */
export function parseTitle(title: string) {
  const match = /^\[[^\]]*\]\s*([A-Z])(\d*)\.\s*(.*)$/.exec(title);
  return match ? { code: `${match[1]}${match[2]}`, letter: match[1], title: match[3] } : { code: "·", letter: "", title };
}

function column(child: EpicChild): "ops" | "eng" {
  const labels = child.labels.nodes.map((label) => label.name.toLowerCase());
  if (labels.includes("ops")) return "ops";
  if (labels.includes("eng")) return "eng";
  const { letter } = parseTitle(child.title);
  return letter >= "G" && letter <= "J" ? "eng" : "ops";
}

export interface FocusPill { label: string; tone: "ok" | "go" | "wait" | "shadow" | "" }
export interface FocusStream {
  code: string; title: string; identifier: string; due: string | null; weekday: string | null;
  done: number; total: number; percent: number; status: string; pills: FocusPill[];
}
export interface FocusModel {
  schema: "focus/1"; identifier: string; title: string; dayLabel: string; refreshedAt: string; generatedAt: string;
  tiles: Array<{ key: string; label: string; value: string | null; detail: string }>;
  today: FocusStream[]; ops: FocusStream[]; eng: FocusStream[];
  decisionOwed: { code: string; title: string; identifier: string } | null;
}

function stream(child: EpicChild): FocusStream {
  const parsed = parseTitle(child.title);
  const weekday = child.dueDate ? weekdayOf(child.dueDate) : null;
  const type = child.state.type;
  const main: FocusPill = type === "completed" ? { label: "done", tone: "ok" }
    : type === "started" ? { label: "in progress", tone: "go" }
    : type === "canceled" ? { label: "canceled", tone: "" }
    : { label: weekday ?? "todo", tone: "wait" };
  const shadow = child.labels.nodes.some((label) => label.name.toLowerCase() === "shadow");
  return { code: parsed.code, title: parsed.title, identifier: child.identifier, due: child.dueDate ?? null, weekday, ...checklist(child), status: type, pills: shadow ? [main, { label: "shadow", tone: "shadow" }] : [main] };
}

function modelTiles(metrics: FocusMetrics | null): FocusModel["tiles"] {
  const byKey = new Map((metrics?.tiles ?? []).map((tile) => [tile.key, tile]));
  const extra = (metrics?.tiles ?? []).filter((tile) => !DEFAULT_TILES.some((known) => known.key === tile.key));
  const slots = [...DEFAULT_TILES.map((known) => byKey.get(known.key) ?? { ...known, value: null, detail: "awaiting events" }), ...extra];
  return slots.slice(0, 3).map(({ key, label, value, detail }) => ({ key, label, value, detail }));
}

const NOT_STARTED = new Set(["triage", "backlog", "unstarted"]);

/** Pure week-focus model: the single source for the /focus HTML and the /api/focus JSON. */
export function buildFocusModel(epic: Epic, metrics: FocusMetrics | null, now: Date): FocusModel {
  const today = madridParts(now);
  const children = epic.children.nodes;
  const todays = children.filter((child) => child.dueDate === today.iso);
  const rest = children.filter((child) => child.dueDate !== today.iso);
  const weekday = WEEKDAYS.indexOf(today.weekday);
  const dayOfWeek = weekday >= 1 && weekday <= 5 ? `day ${weekday} of 5` : "weekend";
  const owed = todays.find((child) => NOT_STARTED.has(child.state.type));
  const owedTitle = owed ? parseTitle(owed.title) : null;
  return {
    schema: "focus/1", identifier: epic.identifier, title: epic.title,
    dayLabel: `${today.weekday} ${today.day} ${today.month} ${today.year} · ${dayOfWeek}`,
    refreshedAt: `${today.hour}:${today.minute}`, generatedAt: now.toISOString(),
    tiles: modelTiles(metrics),
    today: todays.map(stream),
    ops: rest.filter((child) => column(child) === "ops").map(stream),
    eng: rest.filter((child) => column(child) === "eng").map(stream),
    decisionOwed: owed && owedTitle ? { code: owedTitle.code, title: owedTitle.title, identifier: owed.identifier } : null,
  };
}

function pillsHtml(item: FocusStream) {
  return `<div class="pills">${item.pills.map((pill) => `<span class="st${pill.tone ? ` ${pill.tone}` : ""}">${esc(pill.label)}</span>`).join("")}</div>`;
}

function row(item: FocusStream) {
  const meta = [item.identifier, item.weekday, item.total ? `${item.done}/${item.total} checked` : null].filter(Boolean).join(" · ");
  return `<div class="row"><div class="l">${esc(item.code)}</div><div class="t">${esc(item.title)}<small>${esc(meta)}</small></div><div class="bar"><i style="width:${item.percent}%"></i></div><div class="pct">${item.percent}%</div>${pillsHtml(item)}</div>`;
}

function tiles(model: FocusModel) {
  return model.tiles.map((tile) => tile.value !== null
    ? `<div class="tile"><div class="k">${esc(tile.label)}</div><div class="v">${esc(tile.value)}</div><div class="d">${esc(tile.detail)}</div></div>`
    : `<div class="tile"><div class="k">${esc(tile.label)}</div><div class="v muted">—</div><div class="d">${esc(tile.detail)}</div></div>`).join("");
}

export function renderFocusHtml(epic: Epic, metrics: FocusMetrics | null, now: Date, pageUrl = `graph.unsold.cloud/focus/${epic.identifier}?tv=1`) {
  return renderFocusModelHtml(buildFocusModel(epic, metrics, now), pageUrl);
}

export function renderFocusModelHtml(model: FocusModel, pageUrl = `graph.unsold.cloud/focus/${model.identifier}?tv=1`) {
  const empty = `<p class="none">Nothing here.</p>`;
  const owed = model.decisionOwed;
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta http-equiv="refresh" content="300">
<meta name="viewport" content="width=1920,height=1080"><title>${esc(model.title)} · ${esc(model.identifier)}</title><style>
*{box-sizing:border-box}
html,body{margin:0;width:1920px;height:1080px;overflow:hidden;background:#0b0f14;color:#e6edf3;font:22px/1.3 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
body{padding:36px 48px;display:flex;flex-direction:column;gap:20px}
.top{display:flex;justify-content:space-between;align-items:baseline}
.top h1{font-size:40px;margin:0;font-weight:700}.top h1 span{color:#008578}
.day{font-size:24px;color:#8b98a5}
.tiles{display:grid;grid-template-columns:repeat(3,1fr);gap:20px}
.tile,.col{background:#141a22;border-radius:14px;padding:20px 24px}
.k,h2{font-size:18px;text-transform:uppercase;letter-spacing:.06em;color:#8b98a5;margin:0}
.v{font-size:52px;font-weight:700;margin:4px 0}.v.muted{color:#8b98a5}
.d{font-size:18px;color:#8b98a5}
.today{background:#0f2a2a;border:2px solid #008578;border-radius:14px;padding:16px 24px}
.grid{display:grid;grid-template-columns:1fr 1fr;gap:20px;flex:1;min-height:0}
.col{overflow:hidden}
.row{display:grid;grid-template-columns:56px 1fr 180px 64px 200px;gap:14px;align-items:center;padding:10px 0;border-top:1px solid #222b36}
h2+.row{border-top:0}
.l{font-weight:700;font-size:26px;color:#008578}
.t{font-size:22px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.t small{display:block;color:#8b98a5;font-size:16px}
.bar{height:12px;background:#222b36;border-radius:6px;overflow:hidden}.bar i{display:block;height:100%;background:#008578}
.pct{font-size:18px;color:#8b98a5;text-align:right}
.pills{display:flex;gap:6px;justify-content:flex-end}
.st{font-size:16px;padding:4px 12px;border-radius:999px;background:#222b36;color:#e6edf3;white-space:nowrap}
.st.ok{background:#2ecc71;color:#0b0f14}.st.go{background:#008578;color:#e6edf3}.st.wait{background:#f0a020;color:#0b0f14}.st.shadow{background:#3b82f6;color:#e6edf3}
.none{color:#8b98a5;margin:8px 0}
.foot{display:flex;justify-content:space-between;font-size:20px;color:#8b98a5}.foot b{color:#f0a020}
</style></head><body>
<div class="top"><h1>${esc(model.title)} · <span>${esc(model.identifier)}</span></h1><div class="day">${esc(model.dayLabel)} · refreshed ${esc(model.refreshedAt)}</div></div>
<div class="tiles">${tiles(model)}</div>
<div class="today"><h2>Today · ${model.today.length} due</h2>${model.today.map(row).join("") || empty}</div>
<div class="grid"><div class="col"><h2>Ops · ${model.ops.length} streams</h2>${model.ops.map(row).join("") || empty}</div><div class="col"><h2>Eng · ${model.eng.length} streams</h2>${model.eng.map(row).join("") || empty}</div></div>
<div class="foot"><div>Decision owed: <b>${owed ? esc(`${owed.code} ${owed.title} (${owed.identifier})`) : "none"}</b></div><div>${esc(pageUrl)}</div></div>
</body></html>`;
}
