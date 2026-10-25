// Highlight mode: play only the moments, each padded by a few seconds, skipping everything between.
import type { Moment } from "./moments";

export interface HighlightWindow {
  startMs: number;
  endMs: number;
  /** Index of the first moment in this window (overlapping moments share a window). */
  first: number;
  /** How many moments the window covers. */
  count: number;
}

export const HL_BEFORE_DEFAULT = 4;
export const HL_AFTER_DEFAULT = 2;

/** Pad each moment by before/after seconds and merge windows that touch. */
export function highlightWindows(moments: Moment[], beforeS: number, afterS: number, durationMs: number): HighlightWindow[] {
  const out: HighlightWindow[] = [];
  const end = durationMs > 0 ? durationMs : Infinity;
  moments.forEach((m, i) => {
    const s = Math.max(0, m.startMs - beforeS * 1000);
    const e = Math.min(end, m.endMs + afterS * 1000);
    const last = out[out.length - 1];
    if (last && s <= last.endMs) {
      last.endMs = Math.max(last.endMs, e);
      last.count++;
    } else {
      out.push({ startMs: s, endMs: e, first: i, count: 1 });
    }
  });
  return out;
}

/** The window holding `t`, else -1. */
export function windowAt(windows: HighlightWindow[], t: number): number {
  return windows.findIndex((w) => t >= w.startMs && t < w.endMs);
}

/** The first window starting after `t`, else -1. */
export function nextWindow(windows: HighlightWindow[], t: number): number {
  return windows.findIndex((w) => w.startMs > t);
}
