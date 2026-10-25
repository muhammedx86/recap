// Event kinds: labels, icons and colours, backed by list_event_types.
// A tiny module-level store keeps kindStyle() synchronous; App subscribes so the tree
// re-renders when the list changes (after loading, or after Studio edits a type).

import { useSyncExternalStore } from "react";
import { api } from "./api";
import { BUILTIN_EVENT_TYPES, HIDDEN_IN_PICKERS } from "./eventTypes";
import type { EventIcon, EventTypeDef } from "./types";

export interface KindStyle {
  label: string;
  plural: string;
  color: string;
  icon: EventIcon;
}

const BUILTIN_PLURALS: Record<string, string> = {
  kill: "Kills",
  death: "Deaths",
  assist: "Assists",
  multikill: "Multikills",
  ace: "Aces",
  objective: "Objectives",
  round_start: "Rounds",
  round_win: "Rounds won",
  round_loss: "Rounds lost",
  mvp: "MVPs",
  bomb_plant: "Bomb plants",
  bomb_defuse: "Bomb defuses",
  bookmark: "Bookmarks",
  match_start: "Match starts",
  match_end: "Match ends",
};

let types: EventTypeDef[] = BUILTIN_EVENT_TYPES;
let byId = new Map(types.map((t) => [t.id, t]));
const subs = new Set<() => void>();

function setTypes(list: EventTypeDef[]) {
  // Built-ins always exist even if the backend list is older.
  const merged = new Map(BUILTIN_EVENT_TYPES.map((t) => [t.id, t]));
  for (const t of list) merged.set(t.id, t);
  types = [...merged.values()];
  byId = merged;
  subs.forEach((f) => f());
}

/** Fetch the latest event types from the backend. Safe to call any time. */
export async function refreshEventTypes(): Promise<void> {
  try {
    setTypes(await api.listEventTypes());
  } catch {
    // Keep the built-ins; the UI still renders.
  }
}

/** Merge types that arrived with a GameInfo (e.g. after save_event_type) without a round trip. */
export function mergeEventTypes(list: EventTypeDef[]) {
  setTypes([...types, ...list]);
}

/** Subscribe to the type list (re-renders when it changes). */
export function useEventTypes(): EventTypeDef[] {
  return useSyncExternalStore(
    (f) => {
      subs.add(f);
      return () => subs.delete(f);
    },
    () => types,
  );
}

function pluralOf(t: EventTypeDef): string {
  if (BUILTIN_PLURALS[t.id] && t.builtin) return BUILTIN_PLURALS[t.id];
  const l = t.label.trim();
  // Single words pluralise simply; phrases ("Ult used") read fine as they are.
  if (!/\s/.test(l) && !/s$/i.test(l)) return /(x|z|ch|sh)$/i.test(l) ? `${l}es` : `${l}s`;
  return l;
}

export function kindStyle(kind: string): KindStyle {
  const t = byId.get(kind);
  if (t) return { label: t.label, plural: pluralOf(t), color: t.color, icon: t.icon };
  const pretty = kind.replace(/[_-]+/g, " ").replace(/^\w/, (c) => c.toUpperCase());
  // Stable hue from the name for kinds nobody has defined.
  const hue = [...kind].reduce((a, c) => (a * 31 + c.charCodeAt(0)) % 360, 17);
  return { label: pretty, plural: pretty, color: `hsl(${hue} 55% 68%)`, icon: "star" };
}

/** Built-in kinds offered in clip/settings pickers. */
export const COMMON_KINDS = BUILTIN_EVENT_TYPES.map((t) => t.id).filter((k) => !HIDDEN_IN_PICKERS.includes(k) && k !== "round_start");

/** Game-defined kinds that are the headline play of their game, listed first (before kills). */
const HEADLINE = ["goal"];

export function sortKinds(kinds: string[]): string[] {
  const order = [...HEADLINE, ...types.map((t) => t.id).filter((id) => !HEADLINE.includes(id))];
  return [...kinds].sort((a, b) => {
    const ia = order.indexOf(a);
    const ib = order.indexOf(b);
    return (ia < 0 ? 999 : ia) - (ib < 0 ? 999 : ib) || a.localeCompare(b);
  });
}

export function sourceLabel(s: { type: string; detector?: string; name?: string; processor?: string }): string {
  switch (s.type) {
    case "vision":
      return "Vision";
    case "api":
      return s.name === "cs2-gsi" ? "CS2 GSI" : s.name === "league-live-client" ? "Live Client API" : s.name === "rocket-league-stats" ? "Stats API" : "Game API";
    case "postMatch":
      return "Replay file";
    default:
      return "Manual";
  }
}
