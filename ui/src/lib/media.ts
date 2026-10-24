// Stop media when the app is not on screen.
//
// Closing the window only hides it to the tray (Recap keeps running to record games), so a
// <video> would otherwise keep playing, with sound, in the background. The backend emits
// "window-hidden" when the window is closed to the tray or minimized; visibilitychange is the
// backup. Hidden pages also stop decoding: players listen for MEDIA_SLEEP and release their file.

import { useEffect } from "react";
import { on } from "./api";

/** Fired on window when every player should pause and let go of its file. */
export const MEDIA_SLEEP = "recap:media-sleep";

export function pauseAllMedia() {
  for (const el of Array.from(document.querySelectorAll<HTMLMediaElement>("video, audio"))) {
    if (!el.paused) el.pause();
  }
  window.dispatchEvent(new Event(MEDIA_SLEEP));
}

/** Mount once at the app root. */
export function useMediaGuard() {
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState === "hidden") pauseAllMedia();
    };
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("pagehide", pauseAllMedia);
    const off = on("window-hidden", () => pauseAllMedia());
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("pagehide", pauseAllMedia);
      off();
    };
  }, []);
}

/** Small typed localStorage helpers: storage can be unavailable, so every access is guarded. */
export function loadPref<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(`recap.${key}`);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

export function savePref(key: string, value: unknown) {
  try {
    localStorage.setItem(`recap.${key}`, JSON.stringify(value));
  } catch {
    /* storage unavailable: preferences just don't persist */
  }
}

/** Where playback stopped per recording, so reopening a match resumes there. */
const RESUME_KEY = "resume";
const RESUME_MAX = 60;

export function loadResume(path: string): number | null {
  const all = loadPref<Record<string, [number, number]>>(RESUME_KEY, {});
  return all[path]?.[0] ?? null;
}

export function saveResume(path: string, ms: number | null) {
  const all = loadPref<Record<string, [number, number]>>(RESUME_KEY, {});
  if (ms === null) delete all[path];
  else all[path] = [Math.round(ms), Date.now()];
  // Keep the most recent entries only.
  const keep = Object.entries(all)
    .sort((a, b) => b[1][1] - a[1][1])
    .slice(0, RESUME_MAX);
  savePref(RESUME_KEY, Object.fromEntries(keep));
}
