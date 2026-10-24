// Mirrors docs/API.md exactly. All JSON is camelCase.

export type CaptureMode = "game" | "window" | "monitor";
export type ApiSource = "league-live-client" | "cs2-gsi" | "riot-client";

export type EventSource =
  | { type: "vision"; detector: string }
  | { type: "api"; name: string }
  | { type: "postMatch"; processor: string }
  | { type: "manual" };

export interface GameEvent {
  id?: number;
  kind: string;
  tMs: number;
  source: EventSource;
  confidence?: number;
  meta?: any;
}

export type MatchStatus = "recording" | "processing" | "ready" | "error";

export interface MatchRecord {
  id: string;
  gameId: string;
  gameName: string;
  path: string;
  startedUnixMs: number;
  durationMs: number;
  sizeBytes: number;
  status: MatchStatus;
  title: string | null;
  favorite: boolean;
  thumbnail: string | null;
  eventCounts: Record<string, number>;
  error: string | null;
  /** field -> value: character, map, mode, kills, deaths, assists, result, score, or custom fields. */
  details: Record<string, DetailValue>;
}

/** Where a detail came from, best first. "manual" values are never replaced by processing. */
export type DetailSource = "manual" | "api" | "screen" | "events";

export interface DetailValue {
  value: string;
  source: DetailSource;
  confidence?: number;
}

export interface ReaderDef {
  id: string;
  label: string;
  /** Detail field; named groups in `pattern` fill those fields instead. */
  field: string;
  kind: "text" | "choice";
  /** Box to read (the first slot when `slots` is set), fractions of the game picture. */
  region: Region;
  slots?: { count: number; stepX: number; stepY: number };
  /** Pick the slot showing this colour; `box` is relative to the slot. */
  highlight?: { box: Region; color: string; tolerance: number };
  when: "end" | "start" | "any";
  sampleFps: number;
  pattern?: string;
  values?: string[];
  options?: Array<{ value: string; templates: string[] }>;
  threshold: number;
  method: "ncc" | "edges";
  templateFrameHeight: number;
  enabled: boolean;
  notes?: string;
}

/** optionUrls: choice value -> absolute reference image paths. */
export type ReaderInfo = ReaderDef & { ready: boolean; user: boolean; optionUrls: Record<string, string[]> };

export interface Reading {
  field: string;
  value: string;
  confidence: number;
}

export interface FrameReadResult {
  slot: number | null;
  readings: Reading[];
  ocrAvailable: boolean;
}

export interface ReaderTestResult {
  details: Record<string, DetailValue>;
  /** [tMs, field, value] for every frame that produced something. */
  readings: Array<[number, string, string]>;
  frames: number;
  window: [number, number];
}

export type ClipKind = "auto" | "manual" | "montage";

export interface ClipRecord {
  id: string;
  matchId: string | null;
  path: string;
  startMs: number;
  endMs: number;
  durationMs: number;
  kind: ClipKind;
  title: string;
  eventKinds: string[];
  createdUnixMs: number;
  thumbnail: string | null;
  sizeBytes: number;
}

export interface Region {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface DetectorDef {
  id: string;
  label: string;
  event: string;
  region: Region;
  templateBox?: Region;
  templates: string[];
  templateFrameHeight: number;
  /** "ncc" compares brightness; "edges" compares outlines (ignores the scene behind a HUD element). */
  method: "ncc" | "edges";
  threshold: number;
  cooldownMs: number;
  minConsecutive: number;
  sampleFps: number;
  scales: number[];
  /** Fire again when the score dips by this much and recovers while still on screen. 0 = off. */
  rearmDrop?: number;
  enabled: boolean;
  notes?: string;
}

export type EventIcon =
  | "crosshair"
  | "skull"
  | "handshake"
  | "flame"
  | "crown"
  | "gem"
  | "flag"
  | "trophy"
  | "circle-x"
  | "star"
  | "bomb"
  | "shield"
  | "bookmark"
  | "zap"
  | "heart";

export interface EventTypeDef {
  id: string;
  label: string;
  icon: EventIcon;
  color: string;
  builtin: boolean;
}

export interface EventTypeInput {
  id?: string;
  label: string;
  icon: EventIcon;
  color?: string | null;
}

export type GameDetector = DetectorDef & { ready: boolean; user: boolean; templateUrls: string[] };

export interface GameInfo {
  id: string;
  name: string;
  processes: string[];
  capture: CaptureMode;
  api: ApiSource | null;
  preferApi: boolean;
  autoClipEvents: string[];
  color: string | null;
  enabled: boolean;
  userModified: boolean;
  detectors: GameDetector[];
  hasPostMatch: boolean;
  hasRounds: boolean;
  eventTypes: EventTypeDef[];
  readers: ReaderInfo[];
  /** How Recap finds where a match starts and ends inside a game session. */
  matchDetection: MatchDetection;
}

export type MatchDetection = "api" | "detectors" | "process";

export interface GameInput {
  id: string;
  name: string;
  processes: string[];
  capture: CaptureMode;
  api: ApiSource | null;
  preferApi: boolean;
  autoClipEvents: string[];
  color: string | null;
}

export type EncoderChoice = "auto" | "nvenc" | "amf" | "qsv" | "x264";

export interface AutoClipSettings {
  enabled: boolean;
  secondsBefore: number;
  secondsAfter: number;
  mergeGapSeconds: number;
  eventsOverride: string[];
  preciseCuts: boolean;
}

export interface Settings {
  recordingsDir: string;
  fps: number;
  bitrateKbps: number;
  outputHeight: number | null;
  encoder: EncoderChoice;
  recordDesktopAudio: boolean;
  recordMicrophone: boolean;
  microphoneDevice: string | null;
  bookmarkHotkey: string;
  manualRecordHotkey: string;
  autoClip: AutoClipSettings;
  scanAfterMatch: boolean;
  analysisHeight: number;
  gamesEnabled: Record<string, boolean>;
  storageLimitGb: number | null;
  /** Closing the window keeps Recap running in the tray (true) or quits it (false). */
  closeToTray: boolean;
  /** Include bookmarks in the in-game notifications. */
  notifyOnBookmark: boolean;
  /** Also keep the microphone alone on audio track 2 (track 1 is always the full mix). */
  separateMicTrack: boolean;
  /** Start Recap when Windows starts. */
  launchAtStartup: boolean;
  /** When started with Windows, stay in the tray. */
  startMinimized: boolean;
  /** Small notifications over the game. */
  notifications: boolean;
  notificationCorner: NotificationCorner;
  /** Split game sessions into separate matches using Match start / Match end markers. */
  splitMatches: boolean;
}

export type NotificationCorner = "top-right" | "top-left" | "bottom-right" | "bottom-left";

/** Payload of the "overlay-toast" event (the in-game notification window). */
export interface OverlayToast {
  kind: "recording" | "saved" | "bookmark" | "clips" | "skipped";
  title: string;
  detail?: string;
  corner: NotificationCorner;
}

export interface RecorderStatus {
  state: "idle" | "starting" | "recording" | "stopping" | "unavailable";
  matchId: string | null;
  gameId: string | null;
  gameName: string | null;
  startedUnixMs: number | null;
  encoder: string | null;
  capture: CaptureMode | null;
  bookmarks: number;
  message: string | null;
  /** Idle between matches of a game that records each match (its name). */
  waitingFor?: string | null;
}

export interface StorageInfo {
  recordingsDir: string;
  usedBytes: number;
  freeBytes: number;
  matchCount: number;
  clipCount: number;
}

export type FrameSource = { kind: "video"; path: string; tMs: number } | { kind: "image"; path: string };

export interface FrameResult {
  dataUrl: string;
  width: number;
  height: number;
}

export interface TestResult {
  events: GameEvent[];
  samples: Array<[number, number]>;
  durationMs: number;
}

export interface ProcessingProgress {
  matchId: string;
  stage: "scanning" | "postMatch" | "details" | "splitting" | "clipping" | "done" | "error";
  progress: number;
  message?: string;
}

export interface MatchDetail {
  match: MatchRecord;
  events: GameEvent[];
  clips: ClipRecord[];
}

export interface AudioInput {
  id: string;
  name: string;
}

export interface EventMap {
  "status-changed": RecorderStatus;
  "library-changed": { matchId?: string };
  "processing-progress": ProcessingProgress;
  "bookmark-added": { matchId: string; tMs: number };
  /** The window was closed to the tray or minimized: stop playing media. */
  "window-hidden": { reason: "closed" | "minimized" };
}
