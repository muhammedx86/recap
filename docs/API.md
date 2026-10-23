# Recap app contract (UI ↔ Rust)

This file is the contract between the React UI (`ui/`) and the Tauri backend
(`src-tauri/`). Both sides must match it exactly. All JSON is **camelCase**.
Times are integers: `*Ms` = milliseconds, `*UnixMs` = Unix epoch milliseconds.

## Data types (TypeScript notation)

```ts
type CaptureMode = "game" | "window" | "monitor";
type ApiSource = "league-live-client" | "cs2-gsi";

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
  eventCounts: Record<string, number>;  // kind -> count
  error: string | null;
}

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
  method: "ncc";
  threshold: number; cooldownMs: number; minConsecutive: number; sampleFps: number;
  scales: number[]; enabled: boolean; notes?: string;
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
  matchDetection: "api" | "detectors" | "process"; // how match boundaries are found ("process" = recording is the whole game session)             // the game has round markers (round detectors or an API that reports rounds)
  eventTypes: EventTypeDef[];     // built-ins first, then this game's custom types
}

// Settings: see crates/recap-core/src/settings.rs (serde camelCase). Key fields:
interface Settings {
  recordingsDir: string; fps: number; bitrateKbps: number; outputHeight: number | null;
  encoder: "auto" | "nvenc" | "amf" | "qsv" | "x264";
  recordDesktopAudio: boolean; recordMicrophone: boolean; microphoneDevice: string | null;
  bookmarkHotkey: string; manualRecordHotkey: string;
  autoClip: { enabled: boolean; secondsBefore: number; secondsAfter: number; mergeGapSeconds: number; eventsOverride: string[]; preciseCuts: boolean };
  scanAfterMatch: boolean; analysisHeight: number;
  gamesEnabled: Record<string, boolean>;
  storageLimitGb: number | null; notifyOnBookmark: boolean;
  splitMatches: boolean;   // split a game session into one Library entry per match using match_start/match_end markers
}

interface RecorderStatus {
  state: "idle" | "starting" | "recording" | "stopping" | "unavailable";
  matchId: string | null; gameId: string | null; gameName: string | null;
  startedUnixMs: number | null; encoder: string | null; capture: CaptureMode | null;
  bookmarks: number; message: string | null;
}

interface StorageInfo { recordingsDir: string; usedBytes: number; freeBytes: number; matchCount: number; clipCount: number }

// Detector Studio
type FrameSource = { kind: "video"; path: string; tMs: number } | { kind: "image"; path: string };
interface FrameResult { dataUrl: string; width: number; height: number }   // PNG data URL at full resolution
interface TestResult {
  events: GameEvent[];
  samples: Array<[number, number]>;  // [tMs, score]
  durationMs: number;
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
| `delete_match` | `{ id, deleteFiles: boolean }` | `null` |
| `add_event` | `{ matchId, kind, tMs }` | `GameEvent` (manual marker from the player) |
| `delete_event` | `{ matchId, eventId }` | `null` |
| `rescan_match` | `{ id }` | `null` (progress via `processing-progress` events) |
| `list_clips` | `{ matchId?: string }` | `ClipRecord[]` (newest first) |
| `create_clip` | `{ matchId, startMs, endMs, title?: string }` | `ClipRecord` |
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
| `studio_load_frame` | `{ source: FrameSource }` | `FrameResult` |
| `studio_save_detector` | `{ gameId, detector: DetectorDef, source: FrameSource, templateBox: Region \| null }` | `GameInfo` — when `templateBox` is set, the backend cuts that box from the frame at full resolution, saves it as a new template PNG, appends it to `detector.templates`, and sets `templateFrameHeight` to the frame height |
| `studio_test_detector` | `{ gameId, detector: DetectorDef, matchId }` | `TestResult` (runs the unsaved detector over the recording; templates are read from the game folders) |
| `studio_delete_detector` | `{ gameId, detectorId }` | `GameInfo` |
| `open_path` | `{ path }` | `null` (reveal in Explorer) |
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
