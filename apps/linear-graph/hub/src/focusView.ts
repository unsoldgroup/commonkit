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

function pills(child: EpicChild) {
  const type = child.state.type;
  const main = type === "completed" ? `<span class="st ok">done</span>`
    : type === "started" ? `<span class="st go">in progress</span>`
    : type === "canceled" ? `<span class="st">canceled</span>`
    : `<span class="st wait">${esc(child.dueDate ? weekdayOf(child.dueDate) : "todo")}</span>`;
  const shadow = child.labels.nodes.some((label) => label.name.toLowerCase() === "shadow") ? `<span class="st shadow">shadow</span>` : "";
  return `<div class="pills">${main}${shadow}</div>`;
}

function row(child: EpicChild) {
  const parsed = parseTitle(child.title);
  const progress = checklist(child);
  const meta = [child.identifier, child.dueDate ? weekdayOf(child.dueDate) : null, progress.total ? `${progress.done}/${progress.total} checked` : null].filter(Boolean).join(" · ");
  return `<div class="row"><div class="l">${esc(parsed.code)}</div><div class="t">${esc(parsed.title)}<small>${esc(meta)}</small></div><div class="bar"><i style="width:${progress.percent}%"></i></div><div class="pct">${progress.percent}%</div>${pills(child)}</div>`;
}

function tiles(metrics: FocusMetrics | null) {
  const byKey = new Map((metrics?.tiles ?? []).map((tile) => [tile.key, tile]));
  const extra = (metrics?.tiles ?? []).filter((tile) => !DEFAULT_TILES.some((known) => known.key === tile.key));
  const slots: Array<{ label: string; tile?: FocusMetricTile }> = [...DEFAULT_TILES.map((known) => ({ label: known.label, tile: byKey.get(known.key) })), ...extra.map((tile) => ({ label: tile.label, tile }))];
  return slots.slice(0, 3).map(({ label, tile }) => tile
    ? `<div class="tile"><div class="k">${esc(tile.label)}</div><div class="v">${esc(tile.value)}</div><div class="d">${esc(tile.detail)}</div></div>`
    : `<div class="tile"><div class="k">${esc(label)}</div><div class="v muted">—</div><div class="d">awaiting events</div></div>`).join("");
}

const NOT_STARTED = new Set(["triage", "backlog", "unstarted"]);

export function renderFocusHtml(epic: Epic, metrics: FocusMetrics | null, now: Date, pageUrl = `graph.unsold.cloud/focus/${epic.identifier}?tv=1`) {
  const today = madridParts(now);
  const children = epic.children.nodes;
  const todays = children.filter((child) => child.dueDate === today.iso);
  const rest = children.filter((child) => child.dueDate !== today.iso);
  const ops = rest.filter((child) => column(child) === "ops");
  const eng = rest.filter((child) => column(child) === "eng");
  const weekday = WEEKDAYS.indexOf(today.weekday);
  const dayOfWeek = weekday >= 1 && weekday <= 5 ? `day ${weekday} of 5` : "weekend";
  const owed = todays.find((child) => NOT_STARTED.has(child.state.type));
  const empty = `<p class="none">Nothing here.</p>`;
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta http-equiv="refresh" content="300">
<meta name="viewport" content="width=1920,height=1080"><title>${esc(epic.title)} · ${esc(epic.identifier)}</title><style>
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
<div class="top"><h1>${esc(epic.title)} · <span>${esc(epic.identifier)}</span></h1><div class="day">${esc(`${today.weekday} ${today.day} ${today.month} ${today.year}`)} · ${dayOfWeek} · refreshed ${esc(`${today.hour}:${today.minute}`)}</div></div>
<div class="tiles">${tiles(metrics)}</div>
<div class="today"><h2>Today · ${todays.length} due</h2>${todays.map(row).join("") || empty}</div>
<div class="grid"><div class="col"><h2>Ops · ${ops.length} streams</h2>${ops.map(row).join("") || empty}</div><div class="col"><h2>Eng · ${eng.length} streams</h2>${eng.map(row).join("") || empty}</div></div>
<div class="foot"><div>Decision owed: <b>${owed ? esc(`${parseTitle(owed.title).code} ${parseTitle(owed.title).title} (${owed.identifier})`) : "none"}</b></div><div>${esc(pageUrl)}</div></div>
</body></html>`;
}
