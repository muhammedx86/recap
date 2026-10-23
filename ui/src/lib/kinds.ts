// Event kinds: labels and colours. The accent (amber) is reserved for the
// player's own bookmarks, the playhead and selection.

export interface KindStyle {
  label: string;
  plural: string;
  color: string;
}

export const KINDS: Record<string, KindStyle> = {
  kill: { label: "Kill", plural: "Kills", color: "#6fd6a8" },
  death: { label: "Death", plural: "Deaths", color: "#f07a6a" },
  assist: { label: "Assist", plural: "Assists", color: "#7fa7ff" },
  multikill: { label: "Multikill", plural: "Multikills", color: "#c99bff" },
  ace: { label: "Ace", plural: "Aces", color: "#ff9bd2" },
  objective: { label: "Objective", plural: "Objectives", color: "#5cc8d6" },
  round_win: { label: "Round won", plural: "Rounds won", color: "#a6d86b" },
  round_loss: { label: "Round lost", plural: "Rounds lost", color: "#9a7f86" },
  mvp: { label: "MVP", plural: "MVPs", color: "#e9d38a" },
  bomb_plant: { label: "Bomb planted", plural: "Bomb plants", color: "#ff9a62" },
  bomb_defuse: { label: "Bomb defused", plural: "Bomb defuses", color: "#59c3ff" },
  bookmark: { label: "Bookmark", plural: "Bookmarks", color: "var(--accent)" },
  match_start: { label: "Match start", plural: "Match starts", color: "#8d9099" },
  match_end: { label: "Match end", plural: "Match ends", color: "#8d9099" },
};

/** Kinds offered in pickers (custom kinds can be typed in). */
export const COMMON_KINDS = ["kill", "death", "assist", "multikill", "ace", "objective", "round_win", "round_loss", "mvp", "bomb_plant", "bomb_defuse", "bookmark"];

/** Display order for summaries. */
const ORDER = Object.keys(KINDS);

export function kindStyle(kind: string): KindStyle {
  if (KINDS[kind]) return KINDS[kind];
  const pretty = kind.replace(/[_-]+/g, " ").replace(/^\w/, (c) => c.toUpperCase());
  // Stable hue from the name for custom kinds.
  const hue = [...kind].reduce((a, c) => (a * 31 + c.charCodeAt(0)) % 360, 17);
  return { label: pretty, plural: pretty, color: `hsl(${hue} 55% 68%)` };
}

export function sortKinds(kinds: string[]): string[] {
  return [...kinds].sort((a, b) => {
    const ia = ORDER.indexOf(a);
    const ib = ORDER.indexOf(b);
    return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
  });
}

export function sourceLabel(s: { type: string; detector?: string; name?: string; processor?: string }): string {
  switch (s.type) {
    case "vision":
      return "Vision";
    case "api":
      return s.name === "cs2-gsi" ? "CS2 GSI" : s.name === "league-live-client" ? "Live Client API" : "Game API";
    case "postMatch":
      return "Replay file";
    default:
      return "Manual";
  }
}
