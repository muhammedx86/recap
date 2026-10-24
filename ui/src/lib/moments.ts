// Group plays that happen close together into "moments" (a triple kill, a trade, a clutch...).
import type { GameEvent } from "./types";

/** Plays this close to the previous one join the same moment. */
export const MOMENT_GAP_MS = 6000;

/** Kinds that mark structure, not plays: never grouped. */
const STRUCTURE = new Set(["round_start", "round_win", "round_loss", "match_start", "match_end"]);
/** Summary kinds derived from the kills themselves: shown as the moment's title, not as extra plays. */
const SUMMARY = new Set(["multikill", "ace"]);

export interface Moment {
  startMs: number;
  endMs: number;
  /** The plays in time order (structure and summary kinds excluded). */
  events: GameEvent[];
  /** kind -> count, in first-seen order. */
  counts: Array<[string, number]>;
  kills: number;
  /** A multikill/ace event inside the moment, if the game or Recap reported one. */
  highlight?: GameEvent;
}

const KILL_NAMES = ["", "", "Double kill", "Triple kill", "Quadra kill", "Penta kill"];

export function groupMoments(events: GameEvent[], gapMs = MOMENT_GAP_MS): Moment[] {
  const sorted = [...events].filter((e) => !STRUCTURE.has(e.kind)).sort((a, b) => a.tMs - b.tMs);
  const out: Moment[] = [];
  let cur: Moment | null = null;
  for (const e of sorted) {
    if (!cur || e.tMs - cur.endMs > gapMs) {
      cur = { startMs: e.tMs, endMs: e.tMs, events: [], counts: [], kills: 0 };
      out.push(cur);
    }
    cur.endMs = Math.max(cur.endMs, e.tMs);
    if (SUMMARY.has(e.kind)) {
      // Keep the strongest summary (an ace beats a multikill).
      if (!cur.highlight || e.kind === "ace") cur.highlight = e;
      continue;
    }
    cur.events.push(e);
    if (e.kind === "kill") cur.kills++;
    const c = cur.counts.find(([k]) => k === e.kind);
    if (c) c[1]++;
    else cur.counts.push([e.kind, 1]);
  }
  return out.filter((m) => m.events.length > 0 || m.highlight);
}

/** "Triple kill", "Ace", or null for an ordinary moment. */
export function momentTitle(m: Moment): string | null {
  if (m.highlight?.kind === "ace") return "Ace";
  const n = Math.max(m.kills, Number(m.highlight?.meta?.count ?? 0));
  if (n >= 2) return n < KILL_NAMES.length ? KILL_NAMES[n] : `${n} kills`;
  return null;
}
