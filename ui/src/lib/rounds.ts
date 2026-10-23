// Round markers → rounds with boundaries and results (see docs/API.md, "Rounds").

import type { GameEvent } from "./types";

export interface Round {
  /** Round number shown to the user (meta.round when known, else timeline order). */
  n: number;
  startMs: number;
  endMs: number;
  result: "win" | "loss" | null;
}

const isResult = (e: GameEvent) => e.kind === "round_win" || e.kind === "round_loss";
const metaRound = (e: GameEvent): number | null => (typeof e.meta?.round === "number" && e.meta.round > 0 ? e.meta.round : null);

/**
 * Build rounds from round_start / round_win / round_loss events.
 * With round_start markers, each round runs to the next start (or the end of the match).
 * Without them, each round runs from the previous result to its own result.
 */
export function computeRounds(events: GameEvent[], durationMs: number): Round[] {
  const starts = events.filter((e) => e.kind === "round_start").sort((a, b) => a.tMs - b.tMs);
  const results = events.filter(isResult).sort((a, b) => a.tMs - b.tMs);
  const dur = Math.max(durationMs, ...events.map((e) => e.tMs), 0);

  if (starts.length) {
    return starts.map((s, i) => {
      const end = starts[i + 1]?.tMs ?? dur;
      // The result normally lands before the next round starts; allow a little slack.
      const res = results.find((r) => r.tMs >= s.tMs && r.tMs < end + 1500);
      return {
        n: metaRound(s) ?? (res && metaRound(res)) ?? i + 1,
        startMs: s.tMs,
        endMs: end,
        result: res ? (res.kind === "round_win" ? "win" : "loss") : null,
      };
    });
  }
  let prev = 0;
  return results.map((r, i) => {
    const round: Round = { n: metaRound(r) ?? i + 1, startMs: prev, endMs: r.tMs, result: r.kind === "round_win" ? "win" : "loss" };
    prev = r.tMs;
    return round;
  });
}

/** Won–lost score from event counts, or null when the match has no round results. */
export function roundScore(counts: Record<string, number>): { won: number; lost: number } | null {
  const won = counts.round_win ?? 0;
  const lost = counts.round_loss ?? 0;
  return won || lost ? { won, lost } : null;
}

export function hasRoundEvents(counts: Record<string, number>): boolean {
  return (counts.round_start ?? 0) + (counts.round_win ?? 0) + (counts.round_loss ?? 0) > 0;
}
