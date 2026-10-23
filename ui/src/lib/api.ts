// Typed wrapper around every Tauri command and event in docs/API.md.
// Outside Tauri (plain browser / screenshots) every call goes to the in-memory mock.

import { invoke as tauriInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import * as mock from "./mock";
import type {
  AudioInput,
  ClipRecord,
  DetectorDef,
  EventMap,
  EventTypeDef,
  EventTypeInput,
  FrameResult,
  FrameSource,
  GameEvent,
  GameInfo,
  GameInput,
  MatchDetail,
  MatchRecord,
  RecorderStatus,
  Region,
  Settings,
  StorageInfo,
  TestResult,
} from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri) return tauriInvoke<T>(cmd, args);
  return mock.invoke<T>(cmd, args ?? {});
}

/** Subscribe to a backend event. Returns an unsubscribe function. */
export function on<K extends keyof EventMap>(name: K, handler: (payload: EventMap[K]) => void): () => void {
  if (!isTauri) return mock.listen(name, handler);
  let unlisten: (() => void) | null = null;
  let cancelled = false;
  tauriListen<EventMap[K]>(name, (e) => handler(e.payload)).then((u) => {
    if (cancelled) u();
    else unlisten = u;
  });
  return () => {
    cancelled = true;
    unlisten?.();
  };
}

/** URL the webview can load for a local file (video, thumbnail, template). */
export function fileSrc(path: string | null | undefined): string | undefined {
  if (!path) return undefined;
  if (isTauri) return convertFileSrc(path);
  return mock.mediaUrl(path);
}

/** Pick a file or folder. Uses the native dialog in Tauri, a fixed sample path in the browser. */
export async function pickPath(opts: { directory?: boolean; filters?: { name: string; extensions: string[] }[]; title?: string }): Promise<string | null> {
  if (!isTauri) return mock.pickPath(opts.directory ?? false);
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({ directory: opts.directory ?? false, multiple: false, filters: opts.filters, title: opts.title });
  return typeof res === "string" ? res : null;
}

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return "Something went wrong";
  }
}

export const api = {
  getStatus: () => call<RecorderStatus>("get_status"),
  listMatches: (gameId?: string) => call<MatchRecord[]>("list_matches", gameId ? { gameId } : {}),
  getMatch: (id: string) => call<MatchDetail>("get_match", { id }),
  /** `title: ""` clears the custom title. */
  updateMatch: (id: string, patch: { title?: string; favorite?: boolean }) => call<MatchRecord>("update_match", { id, ...patch }),
  deleteMatch: (id: string, deleteFiles: boolean) => call<null>("delete_match", { id, deleteFiles }),
  addEvent: (matchId: string, kind: string, tMs: number) => call<GameEvent>("add_event", { matchId, kind, tMs }),
  deleteEvent: (matchId: string, eventId: number) => call<null>("delete_event", { matchId, eventId }),
  rescanMatch: (id: string) => call<null>("rescan_match", { id }),

  listClips: (matchId?: string) => call<ClipRecord[]>("list_clips", matchId ? { matchId } : {}),
  createClip: (matchId: string, startMs: number, endMs: number, title?: string) =>
    call<ClipRecord>("create_clip", { matchId, startMs, endMs, ...(title ? { title } : {}) }),
  createMontage: (clipIds: string[], title: string) => call<ClipRecord>("create_montage", { clipIds, title }),
  updateClip: (id: string, title: string) => call<ClipRecord>("update_clip", { id, title }),
  deleteClip: (id: string, deleteFile: boolean) => call<null>("delete_clip", { id, deleteFile }),

  listGames: () => call<GameInfo[]>("list_games"),
  setGameEnabled: (gameId: string, enabled: boolean) => call<GameInfo>("set_game_enabled", { gameId, enabled }),
  saveGame: (game: GameInput) => call<GameInfo>("save_game", { game }),

  listEventTypes: () => call<EventTypeDef[]>("list_event_types"),
  saveEventType: (gameId: string, eventType: EventTypeInput) => call<GameInfo>("save_event_type", { gameId, eventType }),
  deleteEventType: (gameId: string, eventTypeId: string) => call<GameInfo>("delete_event_type", { gameId, eventTypeId }),

  getSettings: () => call<Settings>("get_settings"),
  updateSettings: (settings: Settings) => call<Settings>("update_settings", { settings }),
  listEncoders: () => call<string[]>("list_encoders"),
  listAudioInputs: () => call<AudioInput[]>("list_audio_inputs"),

  startManualRecording: () => call<RecorderStatus>("start_manual_recording"),
  stopRecording: () => call<RecorderStatus>("stop_recording"),
  addBookmark: () => call<null>("add_bookmark"),
  getStorage: () => call<StorageInfo>("get_storage"),

  studioLoadFrame: (source: FrameSource) => call<FrameResult>("studio_load_frame", { source }),
  studioSaveDetector: (gameId: string, detector: DetectorDef, source: FrameSource, templateBox: Region | null) =>
    call<GameInfo>("studio_save_detector", { gameId, detector, source, templateBox }),
  studioTestDetector: (gameId: string, detector: DetectorDef, matchId: string) =>
    call<TestResult>("studio_test_detector", { gameId, detector, matchId }),
  studioDeleteDetector: (gameId: string, detectorId: string) => call<GameInfo>("studio_delete_detector", { gameId, detectorId }),

  openPath: (path: string) => call<null>("open_path", { path }),
};
