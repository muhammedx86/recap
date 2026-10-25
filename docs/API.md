# Recap app contract (UI ↔ Rust)

This file is the contract between the React UI (`ui/`) and the Tauri backend
(`src-tauri/`). Both sides must match it exactly. All JSON is **camelCase**.
Times are integers: `*Ms` = milliseconds, `*UnixMs` = Unix epoch milliseconds.

## Data types (TypeScript notation)

```ts
type CaptureMode = "game" | "window" | "monitor";
type ApiSource = "league-live-client" | "cs2-gsi" | "riot-client" | "marvel-state-file";

type EventSource =
  | { type: "vision"; detector: string }
  | { type: "api"; name: string }
  | { type: "postMatch"; processor: string }
  | { type: "manual" };

interface GameEvent {
  id?: number;            // present when read from the library
  kind: string;           // "kill" | "death" | "assist" | "multikill" | "ace" | "objective" | "round_win" | "round_loss" | "mvp" | "bomb_plant" | "bomb_defuse" | "bookmark" | "match_start" | "match_end" | custom
  tMs: number;            // offset from recording start
  source: EventSource;
  confidence?: number;    // 0..1, vision only
  meta?: any;
}

type MatchStatus = "recording" | "processing" | "ready" | "error";

interface MatchRecord {
  id: string;             // uuid
  gameId: string;         // "manual" for manual recordings
  gameName: string;
  path: string;           // absolute .mp4 path
  startedUnixMs: number;
  durationMs: number;
  sizeBytes: number;
  status: MatchStatus;
  title: string | null;
  favorite: boolean;
  thumbnail: string | null;   // absolute .jpg path
  eventCounts: Record<string, number>;  // kind -> count (hidden kinds starting with "_" excluded)
  error: string | null;
  details: Record<string, DetailValue>; // field -> value, e.g. character, map, mode, kills, deaths, assists, result, score
  health?: {            // how the recording went; every field optional (missing = fine/unknown)
    partial?: boolean;        // interrupted and resumed, or started after the match (derived from a `_resumed` event)
    apiUnavailable?: boolean; // the game's live API never answered; plays came from the screen only
    droppedFrames?: number;   // share of frames dropped while encoding, 0..1
    lowDisk?: boolean;        // disk space ran low while recording
    encoder?: string;         // encoder actually used, e.g. "NVENC H.264"
  };                          // backend: Library::set_health(match_id, &MatchHealth) merges (bools OR'ed, values overwrite)
}

// Where a detail came from, best first. A value is only replaced by an equal or better source;
// "manual" (typed by the player) is never replaced by processing.
type DetailSource = "manual" | "api" | "screen" | "events";
interface DetailValue { value: string; source: DetailSource; confidence?: number }
// Built-in fields in display order: character, map, mode, kills, deaths, assists,
// result ("win" | "loss" | "draw"), score ("13-11", yours first). Readers may add custom fields.
// Events of kind "_details" carry live-API snapshots (meta = {field: value}); the UI hides kinds starting with "_".

type ClipKind = "auto" | "manual" | "montage";
interface ClipRecord {
  id: string;
  matchId: string | null;   // null for montages
  path: string;
  startMs: number;          // within the source match (0 for montages)
  endMs: number;
  durationMs: number;
  kind: ClipKind;
  title: string;
  eventKinds: string[];     // kinds inside the clip window
  createdUnixMs: number;
  thumbnail: string | null;
  sizeBytes: number;
}

interface Region { x: number; y: number; w: number; h: number }   // fractions 0..1

interface DetectorDef {
  id: string; label: string; event: string;
  region: Region; templateBox?: Region;
  templates: string[];            // relative to game folder
  templateFrameHeight: number;
  method: "ncc" | "edges";
  threshold: number; cooldownMs: number; minConsecutive: number; sampleFps: number;
  scales: number[];
  rearmDrop?: number; // 0/absent = off enabled: boolean; notes?: string;
}

// Event types: every event kind has a label, one of 15 icons and a colour.
// Built-in types always exist; games can add custom ones (created in Detector Studio).
type EventIcon =
  | "crosshair" | "skull" | "handshake" | "flame" | "crown"
  | "gem" | "flag" | "trophy" | "circle-x" | "star"
  | "bomb" | "shield" | "bookmark" | "zap" | "heart";
interface EventTypeDef {
  id: string;          // the event `kind`, e.g. "kill", "ult_used"
  label: string;       // "Kill", "Ult used"
  icon: EventIcon;
  color: string;       // hex, e.g. "#6fd6a8"
  builtin: boolean;
}
// Built-ins: kill(crosshair) death(skull) assist(handshake) multikill(flame) ace(crown) objective(gem)
// round_start(flag) round_win(trophy) round_loss(circle-x) mvp(star) bomb_plant(bomb) bomb_defuse(shield)
// bookmark(bookmark) match_start(flag) match_end(flag).
//
// Rounds: `round_start`, `round_win` and `round_loss` are round markers. `meta.round` (1-based) is set
// when the source knows the round number (CS2 GSI); otherwise number rounds in timeline order.
// The round score of a match is eventCounts.round_win – eventCounts.round_loss.

interface GameInfo {
  id: string; name: string; processes: string[];
  capture: CaptureMode; api: ApiSource | null; preferApi: boolean;
  autoClipEvents: string[]; color: string | null;
  enabled: boolean;               // from settings
  userModified: boolean;
  detectors: Array<DetectorDef & { ready: boolean; user: boolean; templateUrls: string[] }>; // templateUrls = absolute file paths of templates
  hasPostMatch: boolean;
  hasRounds: boolean;
  matchDetection: "api" | "detectors" | "process"; // how match boundaries are found ("process" = recording is the whole game session)
  assetsDir: string | null;      // the game's bundled assets/ folder (agents/<slug>.png, maps/<slug>.jpg, ranks/<slug>.png), if any
  eventTypes: EventTypeDef[];     // built-ins first, then this game's custom types
  readers: Array<ReaderDef & { ready: boolean; user: boolean; optionUrls: Record<string, string[]> }>; // optionUrls: value -> absolute reference paths
}

// Screen readers (games/<id>/readers/*.json): fill match details from the screen.
interface ReaderDef {
  id: string; label: string;
  field: string;                       // detail field; named groups in `pattern` fill those fields instead
  kind: "text" | "choice";             // text = OCR; choice = best-matching labelled reference
  region: Region;                      // box to read (first slot), fractions of the game picture
  slots?: { count: number; stepX: number; stepY: number };            // repeated boxes (one per player)
  highlight?: { box: Region; color: string; tolerance: number };      // pick the slot showing this colour; box is relative to the slot
  when: "end" | "start" | "any";       // end: from just before match_end until the next match
  sampleFps: number;                   // default 1
  pattern?: string;                    // text: regex; e.g. "(?P<kills>\\d+)\\D+(?P<deaths>\\d+)\\D+(?P<assists>\\d+)"
  values?: string[];                   // text: allowed values (readings snap to the closest, or are dropped)
  options?: Array<{ value: string; templates: string[] }>;            // choice
  threshold: number; method: "ncc" | "edges"; templateFrameHeight: number;  // choice matching
  enabled: boolean; notes?: string;
}
// Readers filling "team_score" and "enemy_score" are combined into "score" and "result".

// Settings: see crates/recap-core/src/settings.rs (serde camelCase). Key fields:
interface Settings {
  recordingsDir: string; fps: number; bitrateKbps: number; outputHeight: number | null;
  encoder: "auto" | "nvenc" | "amf" | "qsv" | "x264";
  recordDesktopAudio: boolean; recordMicrophone: boolean; microphoneDevice: string | null;
  bookmarkHotkey: string; manualRecordHotkey: string;
  autoClip: { enabled: boolean; secondsBefore: number; secondsAfter: number; mergeGapSeconds: number; eventsOverride: string[]; preciseCuts: boolean };
  scanAfterMatch: boolean; analysisHeight: number;
  gamesEnabled: Record<string, boolean>;
  storageLimitGb: number | null;
  notifyOnBookmark: boolean;       // include bookmarks in the in-game notifications
  separateMicTrack: boolean;       // default true: track 1 = full mix (what players play), track 2 = microphone alone
  launchAtStartup: boolean;        // default false: start Recap when Windows starts (autostart entry passes --minimized)
  startMinimized: boolean;         // default true: when launched by that entry, stay in the tray
  notifications: boolean;          // default true: in-game notifications (overlay window)
  notificationCorner: "top-right" | "top-left" | "bottom-right" | "bottom-left";
  splitMatches: boolean;   // split a game session into one Library entry per match using match_start/match_end markers
  closeToTray: boolean;    // closing the window keeps Recap running in the tray (default true); false quits
}

interface RecorderStatus {
  state: "idle" | "starting" | "recording" | "stopping" | "unavailable";
  matchId: string | null; gameId: string | null; gameName: string | null;
  startedUnixMs: number | null; encoder: string | null; capture: CaptureMode | null;
  bookmarks: number; message: string | null;
  waitingFor?: string | null;  // idle between matches of a per-match game (its name)
}

interface StorageInfo { recordingsDir: string; usedBytes: number; freeBytes: number; matchCount: number; clipCount: number }

// Detector Studio
type FrameSource = { kind: "video"; path: string; tMs: number } | { kind: "image"; path: string };
interface FrameResult { dataUrl: string; width: number; height: number }   // PNG data URL at full resolution
interface MatchLocation {
  score: number;
  region: Region;       // where it matched, fractions of the game picture
  scale: number;        // template size relative to the reference (1 = as cut)
  reference: number;    // index into the detector's templates
}
interface TestResult {
  events: GameEvent[];
  samples: Array<[number, number]>;  // [tMs, score]
  durationMs: number;
  hits: Array<MatchLocation & { tMs: number }>;  // where each detection matched
  cancelled: boolean;                 // stopped by studio_cancel; samples cover part of the recording
}
interface VideoInfo {
  width: number; height: number; durationMs: number;
  fps: number;                        // 0 when unknown
  content: { x: number; y: number; w: number; h: number } | null;  // game picture inside black bars, video pixels; null = fills the frame
}
interface ProcessingProgress { matchId: string; stage: "scanning" | "postMatch" | "clipping" | "done" | "error"; progress: number; message?: string }
```

## Commands (`invoke(name, args)`)

Arguments are passed as an object with these camelCase keys.

| Command | Args | Returns |
|---|---|---|
| `get_status` | – | `RecorderStatus` |
| `list_matches` | `{ gameId?: string }` | `MatchRecord[]` (newest first) |
| `get_match` | `{ id }` | `{ match: MatchRecord; events: GameEvent[]; clips: ClipRecord[] }` |
| `update_match` | `{ id, title?: string, favorite?: boolean }` (title `""` clears it) | `MatchRecord` |
| `update_match_detail` | `{ id, field, value }` (`""` clears; saved as source `manual`) | `MatchRecord` |
| `delete_match` | `{ id, deleteFiles: boolean }` | `null` |
| `add_event` | `{ matchId, kind, tMs }` | `GameEvent` (manual marker from the player) |
| `delete_event` | `{ matchId, eventId }` | `null` |
| `rescan_match` | `{ id }` | `null` (progress via `processing-progress` events) |
| `list_clips` | `{ matchId?: string }` | `ClipRecord[]` (newest first) |
| `create_clip` | `{ matchId, startMs, endMs, title?: string }` | `ClipRecord` |
| `clip_cut_points` | `{ matchId, startMs, endMs }` | `{ startMs, endMs, exact }`: where a clip of that range will really start (the keyframe a stream-copy cut uses) and end; `exact` when frame-accurate cuts are on |
| `match_waveform` | `{ matchId }` | `number[]` (peaks 0..255, one per bucket across the recording; empty without audio). Cached as `<thumbnails>/<id>.wave` |
| `match_scrub_strip` | `{ matchId }` | `{ path, frames }`: one JPG with `frames` evenly spaced 16:9 frames side by side, for hover scrubbing. Cached as `<thumbnails>/<id>.strip.jpg` |
| `create_montage` | `{ clipIds: string[], title: string }` | `ClipRecord` |
| `update_clip` | `{ id, title }` | `ClipRecord` |
| `delete_clip` | `{ id, deleteFile: boolean }` | `null` |
| `list_games` | – | `GameInfo[]` |
| `set_game_enabled` | `{ gameId, enabled }` | `GameInfo` |
| `save_game` | `{ game: { id, name, processes, capture, api, preferApi, autoClipEvents, color } }` | `GameInfo` (adds a custom game in the user folder) |
| `list_event_types` | – | `EventTypeDef[]` (built-ins plus every game's custom types, deduplicated by id) |
| `save_event_type` | `{ gameId, eventType: { id?: string; label: string; icon: EventIcon; color?: string \| null } }` | `GameInfo` — creates a custom type for that game (id derived from the label when omitted: lowercase, `_` separated) or edits an existing custom one; built-in ids cannot be changed; colour auto-assigned when omitted |
| `delete_event_type` | `{ gameId, eventTypeId }` | `GameInfo` — fails if a detector of that game still uses it |
| `get_settings` | – | `Settings` |
| `update_settings` | `{ settings: Settings }` | `Settings` |
| `list_encoders` | – | `string[]` |
| `list_audio_inputs` | – | `Array<{ id: string; name: string }>` |
| `start_manual_recording` | – | `RecorderStatus` |
| `stop_recording` | – | `RecorderStatus` |
| `add_bookmark` | – | `null` (same as the hotkey) |
| `get_storage` | – | `StorageInfo` |
| `studio_load_frame` | `{ source: FrameSource }` | `FrameResult` (game picture only, black bars removed). Studio uses it for screenshots and as a fallback for colour picking; recordings are played directly |
| `studio_save_detector` | `{ gameId, detector: DetectorDef, source: FrameSource, templateBox: Region \| null }` | `GameInfo` — when `templateBox` is set, the backend cuts that box from the frame at full resolution, saves it as a new template PNG, appends it to `detector.templates`, and sets `templateFrameHeight` to the frame height. With `templateBox: null` the detector is saved as given (Studio removes a reference by saving `templates` without it) |
| `studio_video_info` | `{ path }` | `VideoInfo`. Studio plays the recording itself (asset URL) and uses `content` to crop black bars so boxes are fractions of the game picture. Black-bar detection decodes a few seconds of video, so results are cached per file until it changes |
| `studio_test_detector` | `{ gameId, detector: DetectorDef, matchId }` | `TestResult` (runs the unsaved detector over the recording; templates are read from the game folders). Reports `studio-progress` events; `studio_cancel` stops it early |
| `studio_cancel` | – | `null` — stop the running Studio test; it returns what it has so far with `cancelled: true` |
| `studio_delete_detector` | `{ gameId, detectorId }` | `GameInfo` |
| `studio_save_reader` | `{ gameId, reader: ReaderDef, source: FrameSource \| null, crops: Array<{ value, region }> }` (each crop becomes a reference for that choice value) | `GameInfo` |
| `studio_read_frame` | `{ gameId, reader: ReaderDef, source: FrameSource }` | `{ slot: number \| null; readings: Array<{ field; value; confidence }>; ocrAvailable: boolean }` |
| `studio_test_reader` | `{ gameId, reader: ReaderDef, matchId }` | `{ details: Record<string, DetailValue>; readings: Array<[tMs, field, value]>; frames: number; window: [startMs, endMs] }` |
| `studio_delete_reader` | `{ gameId, readerId }` | `GameInfo` |
| `open_path` | `{ path }` | `null` (reveal in Explorer) |
| `set_fullscreen` | `{ on: boolean }` | `null` (the app window; used by the video player's full screen) |
| `refresh` | – | `null` (re-broadcasts `status-changed`) |

Media files are shown with `convertFileSrc(path)` from `@tauri-apps/api/core` (asset protocol); there is no command for it.

Errors are returned as rejected promises with a string message.

## Events (`listen(name)`)

| Event | Payload |
|---|---|
| `status-changed` | `RecorderStatus` |
| `library-changed` | `{ matchId?: string }` |
| `processing-progress` | `ProcessingProgress` |
| `bookmark-added` | `{ matchId: string; tMs: number }` |
| `window-hidden` | `{ reason: "closed" \| "minimized" }` — the window went to the tray or was minimized; the UI pauses all media |
| `studio-progress` | `{ task: "test"; progress: number }` — a Studio detector test is running (0..1) |
| `overlay-toast` | `{ kind: "recording" \| "saved" \| "bookmark" \| "clips" \| "skipped"; title: string; detail?: string; corner }`, sent only to the `overlay` window (`overlay.html`). Not sent while the main window has focus. |
