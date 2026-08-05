/* The six-pack.
 *
 * Six instruments, each owning exactly one truth about this kit. Every needle is
 * driven from a real daemon channel; a channel the daemon does not yet publish
 * renders unpowered rather than inventing a value. `source` records which channel
 * feeds each instrument so the contract stays visible from the UI side. */

export type Signal = "live" | "caution" | "alert" | "cold";

export interface Reading {
  /** Position of the pointer, 0..1 of the instrument's travel. */
  fraction: number;
  /** What the instrument prints under the dial. Real value or "NO SIGNAL". */
  readout: string;
  signal: Signal;
  /** One sentence for the screen-reader panel summary. */
  spoken: string;
}

export type InstrumentKind = "counter" | "rose" | "activity" | "attitude" | "rate";

export interface InstrumentSpec {
  id: string;
  /** Engraved label plate under the dial. */
  tag: string;
  kind: InstrumentKind;
  /** The daemon channel this instrument reads. */
  source: string;
  /** Full-scale deflection for counting instruments. */
  fullScale?: number;
}

export const SIX_PACK: readonly InstrumentSpec[] = [
  { id: "skills",   tag: "Skills",       kind: "counter",  source: "kit.skills",       fullScale: 40 },
  { id: "servers",  tag: "MCP Servers",  kind: "rose",     source: "relay.servers",    fullScale: 12 },
  { id: "tools",    tag: "Tools",        kind: "counter",  source: "kit.tools",        fullScale: 60 },
  { id: "agents",   tag: "Agent Sessions", kind: "activity", source: "relay.sessions", fullScale: 8 },
  { id: "drift",    tag: "Drift",        kind: "attitude", source: "status.state" },
  { id: "plan",     tag: "Plan",         kind: "rate",     source: "plans.operations", fullScale: 24 },
];

/** A channel the daemon does not publish yet. Unpowered, never fabricated. */
export function noSignal(what: string): Reading {
  return { fraction: 0, readout: "NO SIGNAL", signal: "cold", spoken: `${what} is not reporting.` };
}

const TRAVEL = 270;
const START = -135;

function clamp(value: number): number {
  return value < 0 ? 0 : value > 1 ? 1 : value;
}

export function countReading(count: number | null, fullScale: number, noun: string): Reading {
  if (count === null) return noSignal(noun);
  const fraction = clamp(count / fullScale);
  return {
    fraction,
    readout: String(count).padStart(2, "0"),
    signal: count > 0 ? "live" : "cold",
    spoken: `${count} ${noun}.`,
  };
}

function polar(angleDegrees: number, radius: number): [number, number] {
  const radians = ((angleDegrees - 90) * Math.PI) / 180;
  return [50 + radius * Math.cos(radians), 50 + radius * Math.sin(radians)];
}

function ticks(count: number, major: number, fullScale?: number): string {
  let marks = "";
  for (let index = 0; index <= count; index += 1) {
    const angle = START + (index / count) * TRAVEL;
    const isMajor = index % major === 0;
    const [x1, y1] = polar(angle, isMajor ? 30 : 33);
    const [x2, y2] = polar(angle, 38);
    marks += `<line x1="${x1.toFixed(1)}" y1="${y1.toFixed(1)}" x2="${x2.toFixed(1)}" y2="${y2.toFixed(1)}" stroke="currentColor" stroke-width="${isMajor ? 1.6 : .8}" opacity="${isMajor ? .95 : .5}"/>`;
    if (isMajor && fullScale !== undefined) {
      const [tx, ty] = polar(angle, 22);
      marks += `<text x="${tx.toFixed(1)}" y="${(ty + 2.6).toFixed(1)}" text-anchor="middle" font-size="8.5" fill="currentColor" opacity=".85" font-family="B612 Mono, ui-monospace, monospace">${Math.round((index / count) * fullScale)}</text>`;
    }
  }
  return marks;
}

/** A coloured range band on the outer scale, drawn in the instrument's own travel. */
function arc(from: number, to: number, colour: string): string {
  const radius = 35.5;
  const [x1, y1] = polar(START + from * TRAVEL, radius);
  const [x2, y2] = polar(START + to * TRAVEL, radius);
  const large = (to - from) * TRAVEL > 180 ? 1 : 0;
  return `<path d="M${x1.toFixed(1)} ${y1.toFixed(1)} A${radius} ${radius} 0 ${large} 1 ${x2.toFixed(1)} ${y2.toFixed(1)}" fill="none" stroke="${colour}" stroke-width="2.6" opacity=".55" stroke-linecap="butt"/>`;
}

const NEEDLE = `<g class="needle"><path d="M50 50 L48.1 46 L50 14 L51.9 46 Z" fill="var(--lum)"/><circle cx="50" cy="50" r="4.2" fill="#1c2228" stroke="#3a434a" stroke-width=".8"/></g>`;

function face(kind: InstrumentKind, fullScale: number): string {
  if (kind === "attitude") {
    // Horizon: level means the target matches its desired state.
    return `<defs><clipPath id="hz"><circle cx="50" cy="50" r="37"/></clipPath></defs>
      <g clip-path="url(#hz)" class="needle">
        <rect x="-10" y="-10" width="120" height="60" fill="#16303f"/>
        <rect x="-10" y="50" width="120" height="70" fill="#3a2a15"/>
        <line x1="-10" y1="50" x2="110" y2="50" stroke="var(--lum)" stroke-width="1.6"/>
      </g>
      <circle cx="50" cy="50" r="37" fill="none" stroke="#0a0d10" stroke-width="2"/>
      <path d="M30 50 L42 50 M58 50 L70 50 M50 44 L50 50" stroke="var(--green)" stroke-width="2" fill="none" stroke-linecap="square"/>
      <path d="M50 11 L46.5 17 L53.5 17 Z" fill="var(--green)"/>`;
  }
  if (kind === "rose") {
    // One lit segment per connected server, read like a compass card.
    let segments = "";
    for (let index = 0; index < fullScale; index += 1) {
      const angle = (index / fullScale) * 360;
      const [x1, y1] = polar(angle, 26);
      const [x2, y2] = polar(angle, 37);
      segments += `<line data-slot="${index}" x1="${x1.toFixed(1)}" y1="${y1.toFixed(1)}" x2="${x2.toFixed(1)}" y2="${y2.toFixed(1)}" stroke="currentColor" stroke-width="3.4" opacity=".18" stroke-linecap="butt"/>`;
    }
    return `${segments}<circle cx="50" cy="50" r="19" fill="none" stroke="currentColor" stroke-width=".8" opacity=".35"/>`;
  }
  if (kind === "activity") {
    // Turn-coordinator geometry: a bar that banks with concurrent sessions.
    return `${ticks(8, 4, fullScale)}
      <g class="needle"><rect x="22" y="48.6" width="56" height="2.8" rx="1.4" fill="var(--lum)"/><circle cx="50" cy="50" r="3.4" fill="#1c2228" stroke="#3a434a" stroke-width=".8"/></g>
      <path d="M31 62 L69 62" stroke="currentColor" stroke-width=".8" opacity=".45"/>`;
  }
  if (kind === "rate") {
    // Vertical-speed geometry: centre is zero, climb means operations are waiting.
    // A green band up to a quarter scale: a small plan is a normal plan.
    return `${ticks(10, 5, fullScale)}${arc(0, .25, "var(--green)")}${arc(.6, 1, "var(--amber)")}${NEEDLE}`;
  }
  return `${ticks(10, 5, fullScale)}<circle cx="50" cy="50" r="38" fill="none" stroke="currentColor" stroke-width=".6" opacity=".3"/>${NEEDLE}`;
}

export function instrumentMarkup(spec: InstrumentSpec): string {
  const scale = spec.fullScale ?? 1;
  return `<figure class="instrument" data-instrument="${spec.id}" data-lit="off" data-signal="cold" data-source="${spec.source}">
    <div class="dial"><svg viewBox="0 0 100 100" role="img" aria-labelledby="dial-${spec.id}" focusable="false" style="color: var(--lum-faint)">${face(spec.kind, scale)}</svg></div>
    <output class="reading" data-role="readout">----</output>
    <figcaption class="tag" id="dial-${spec.id}">${spec.tag}</figcaption>
  </figure>`;
}

export function sixPackMarkup(specs: readonly InstrumentSpec[] = SIX_PACK, railed = false): string {
  return `<div class="sixpack${railed ? " is-rail" : ""}">${specs.map(instrumentMarkup).join("")}</div>`;
}

/** Move one instrument to its reading. The SVG stays put; only the pointer moves. */
export function driveInstrument(root: ParentNode, spec: InstrumentSpec, reading: Reading): void {
  const figure = root.querySelector<HTMLElement>(`[data-instrument="${spec.id}"]`);
  if (!figure) return;
  const lit = reading.signal !== "cold";
  figure.dataset.lit = lit ? "on" : "off";
  figure.dataset.signal = reading.signal;
  const readout = figure.querySelector<HTMLElement>('[data-role="readout"]');
  if (readout) readout.textContent = reading.readout;
  const svg = figure.querySelector<SVGSVGElement>("svg");
  if (svg) svg.style.color = lit ? "var(--lum-dim)" : "var(--lum-faint)";

  if (spec.kind === "rose") {
    const active = Math.round(reading.fraction * (spec.fullScale ?? 1));
    figure.querySelectorAll<SVGLineElement>("[data-slot]").forEach((slot, index) => {
      const on = index < active;
      slot.setAttribute("opacity", on ? "1" : ".18");
      slot.setAttribute("stroke", on ? "var(--green)" : "currentColor");
    });
    return;
  }
  const needle = figure.querySelector<SVGGElement>(".needle");
  if (!needle) return;
  if (spec.kind === "attitude") {
    // Converged sits level; drift pitches the horizon away from the wings.
    needle.style.transform = `translateY(${(reading.fraction * 26).toFixed(1)}px) rotate(${(reading.fraction * 9).toFixed(1)}deg)`;
    return;
  }
  if (spec.kind === "activity") {
    needle.style.transform = `rotate(${(reading.fraction * 34 - 17).toFixed(1)}deg)`;
    return;
  }
  needle.style.transform = `rotate(${(START + reading.fraction * TRAVEL).toFixed(1)}deg)`;
}

/** The panel read aloud, because a wall of dials is not a screen-reader experience. */
export function panelSummary(entries: Array<[InstrumentSpec, Reading]>): string {
  return entries.map(([spec, reading]) => `${spec.tag}: ${reading.spoken}`).join(" ");
}
