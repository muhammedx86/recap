// Match details: labels, display formatting and source wording shared by the Match page,
// Library rows and Detector Studio.

import type { DetailValue, MatchRecord } from "./types";

/** Built-in fields in display order. */
export const DETAIL_FIELDS: Array<{ id: string; label: string; placeholder: string }> = [
  { id: "character", label: "Character", placeholder: "Jett" },
  { id: "map", label: "Map", placeholder: "Ascent" },
  { id: "mode", label: "Mode", placeholder: "Competitive" },
  { id: "rank", label: "Rank", placeholder: "Gold 2" },
  { id: "player", label: "Player", placeholder: "name#tag" },
  { id: "kills", label: "Kills", placeholder: "18" },
  { id: "deaths", label: "Deaths", placeholder: "6" },
  { id: "assists", label: "Assists", placeholder: "8" },
  { id: "result", label: "Result", placeholder: "win" },
  { id: "score", label: "Score", placeholder: "13-11" },
];

/** Fields a reader can target besides the built-ins. "kda" fills kills/deaths/assists from one box. */
export const READER_FIELDS: Array<{ id: string; label: string }> = [
  { id: "character", label: "Character" },
  { id: "map", label: "Map" },
  { id: "mode", label: "Mode" },
  { id: "kda", label: "K/D/A (one box)" },
  { id: "kills", label: "Kills" },
  { id: "deaths", label: "Deaths" },
  { id: "assists", label: "Assists" },
  { id: "result", label: "Result" },
  { id: "team_score", label: "Your team's score" },
  { id: "enemy_score", label: "Enemy team's score" },
];

export const KDA_PATTERN = String.raw`(?P<kills>\d+)\D{1,4}(?P<deaths>\d+)\D{1,4}(?P<assists>\d+)`;

const KDA = ["kills", "deaths", "assists"];

export function detailLabel(field: string): string {
  return (
    DETAIL_FIELDS.find((f) => f.id === field)?.label ??
    READER_FIELDS.find((f) => f.id === field)?.label ??
    field.replace(/[_-]+/g, " ").replace(/^\w/, (c) => c.toUpperCase())
  );
}

export function sourceText(v: DetailValue): string {
  switch (v.source) {
    case "manual":
      return "Edited";
    case "api":
      return "From the game";
    case "screen":
      return v.confidence !== undefined ? `Read from screen, ${Math.round(v.confidence * 100)}% agreement` : "Read from screen";
    case "events":
      return "Counted from events";
  }
}

export function resultLabel(v: string): string {
  const s = v.toLowerCase();
  return s === "win" ? "Win" : s === "loss" ? "Loss" : s === "draw" ? "Draw" : v;
}

export function displayValue(field: string, value: string): string {
  if (field === "result") return resultLabel(value);
  if (field === "score") return value.replace(/\s*[-:]\s*/, "–");
  return value;
}

/** "18 / 6 / 8" when at least kills or deaths are known. */
export function kdaText(d: Record<string, DetailValue>, sep = " / "): string | null {
  if (!d.kills && !d.deaths) return null;
  return KDA.map((k) => d[k]?.value ?? "–").join(sep);
}

/** The weakest source among K/D/A values (for the strip's source marker). */
export function kdaSource(d: Record<string, DetailValue>): DetailValue | null {
  const order = ["manual", "api", "screen", "events"];
  const vals = KDA.map((k) => d[k]).filter(Boolean) as DetailValue[];
  if (!vals.length) return null;
  return vals.reduce((a, b) => (order.indexOf(b.source) > order.indexOf(a.source) ? b : a));
}

/** Custom fields (not built-in, not internal helpers). */
export function customFields(m: MatchRecord): string[] {
  return Object.keys(m.details ?? {}).filter((k) => !DETAIL_FIELDS.some((f) => f.id === k) && k !== "team_score" && k !== "enemy_score");
}

/** Kills+assists over deaths, e.g. "1.50" (deaths of 0 count as 1). Null without kills or deaths. */
export function kdaRatio(d: Record<string, DetailValue>): string | null {
  const n = (k: string) => {
    const v = parseInt(d[k]?.value ?? "", 10);
    return Number.isFinite(v) ? v : null;
  };
  const k = n("kills");
  const de = n("deaths");
  if (k === null || de === null) return null;
  return ((k + (n("assists") ?? 0)) / Math.max(1, de)).toFixed(2);
}

/** "13-9" -> [13, 9]. */
export function scoreParts(v: string | undefined): [number, number] | null {
  const m = v?.match(/(\d+)\s*[-:–—]\s*(\d+)/);
  return m ? [Number(m[1]), Number(m[2])] : null;
}
