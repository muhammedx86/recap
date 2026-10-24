// Built-in event types (mirrors the backend list in docs/API.md) and the icon set.
// No imports from api/mock so both can use this file.

import type { EventIcon, EventTypeDef } from "./types";

/** The 15 icons an event type can use, in picker order. */
export const EVENT_ICONS: EventIcon[] = [
  "crosshair",
  "skull",
  "handshake",
  "flame",
  "crown",
  "gem",
  "flag",
  "trophy",
  "circle-x",
  "star",
  "bomb",
  "shield",
  "bookmark",
  "zap",
  "heart",
];

/** Colours offered when creating a custom type (auto = backend picks). */
export const EVENT_COLORS = ["#7cc3a3", "#d68b80", "#8fa7d6", "#ad9bd3", "#cfa1bd", "#7cb6be", "#c9b98a", "#d39b74", "#9dbb7c", "#80acc9"];

const b = (id: string, label: string, icon: EventIcon, color: string): EventTypeDef => ({ id, label, icon, color, builtin: true });

export const BUILTIN_EVENT_TYPES: EventTypeDef[] = [
  b("kill", "Kill", "crosshair", "#7cc3a3"),
  b("death", "Death", "skull", "#d68b80"),
  b("assist", "Assist", "handshake", "#8fa7d6"),
  b("multikill", "Multikill", "flame", "#ad9bd3"),
  b("ace", "Ace", "crown", "#cfa1bd"),
  b("objective", "Objective", "gem", "#7cb6be"),
  b("round_start", "Round start", "flag", "#7d818a"),
  b("round_win", "Round won", "trophy", "#9dbb7c"),
  b("round_loss", "Round lost", "circle-x", "#a08a90"),
  b("mvp", "MVP", "star", "#c9b98a"),
  b("bomb_plant", "Bomb planted", "bomb", "#d39b74"),
  b("bomb_defuse", "Bomb defused", "shield", "#80acc9"),
  b("bookmark", "Bookmark", "bookmark", "#f2a93b"),
  b("match_start", "Match start", "flag", "#7d818a"),
  b("match_end", "Match end", "flag", "#7d818a"),
];

/** Kinds that mark rounds; round_start is drawn as dividers rather than a lane. */
export const ROUND_KINDS = ["round_start", "round_win", "round_loss"];

/** Kinds that are bookkeeping and not shown as plays in pickers. */
export const HIDDEN_IN_PICKERS = ["match_start", "match_end"];

/** Turn a display name into an event id, matching the backend: lowercase, `_` separated. */
export function eventId(label: string): string {
  return label
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 40);
}
