// Match page layout: the app sidebar and the Plays panel start hidden so the video gets the room.
import { useSyncExternalStore } from "react";
import { loadPref, savePref } from "./media";

type Panel = "nav" | "side";
const KEY: Record<Panel, string> = {
  nav: "match.panel.nav",
  side: "match.panel.side",
};

const state: Record<Panel, boolean> = {
  nav: loadPref<boolean>(KEY.nav, false),
  side: loadPref<boolean>(KEY.side, false),
};
const listeners = new Set<() => void>();
const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};

export function setMatchPanel(p: Panel, open: boolean) {
  if (state[p] === open) return;
  state[p] = open;
  savePref(KEY[p], open);
  listeners.forEach((fn) => fn());
}
export const toggleMatchPanel = (p: Panel) => setMatchPanel(p, !state[p]);

/** Whether a Match-page panel is shown (remembered; hidden by default). */
export function useMatchPanel(p: Panel): boolean {
  return useSyncExternalStore(subscribe, () => state[p]);
}
