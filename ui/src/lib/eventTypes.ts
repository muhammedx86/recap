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
export const EVENT_COLORS = ["#6fd6a8", "#f07a6a", "#7fa7ff", "#c99bff", "#ff9bd2", "#5cc8d6", "#e9d38a", "#ff9a62", "#a6d86b", "#59c3ff"];

const b = (id: string, label: string, icon: EventIcon, color: string): EventTypeDef => ({ id, label, icon, color, builtin: true });

export const BUILTIN_EVENT_TYPES: EventTypeDef[] = [
  b("kill", "Kill", "crosshair", "#6fd6a8"),
  b("death", "Death", "skull", "#f07a6a"),
  b("assist", "Assist", "handshake", "#7fa7ff"),
  b("multikill", "Multikill", "flame", "#c99bff"),
  b("ace", "Ace", "crown", "#ff9bd2"),
  b("objective", "Objective", "gem", "#5cc8d6"),
  b("round_start", "Round start", "flag", "#8d9099"),
  b("round_win", "Round won", "trophy", "#a6d86b"),
  b("round_loss", "Round lost", "circle-x", "#b07f8a"),
  b("mvp", "MVP", "star", "#e9d38a"),
  b("bomb_plant", "Bomb planted", "bomb", "#ff9a62"),
  b("bomb_defuse", "Bomb defused", "shield", "#59c3ff"),
  b("bookmark", "Bookmark", "bookmark", "#f2a93b"),
  b("match_start", "Match start", "flag", "#8d9099"),
  b("match_end", "Match end", "flag", "#8d9099"),
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
