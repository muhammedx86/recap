// Mirrors docs/API.md exactly. All JSON is camelCase.

export type CaptureMode = "game" | "window" | "monitor";
export type ApiSource = "league-live-client" | "cs2-gsi" | "riot-client" | "marvel-state-file" | "rocket-league-stats";

/** Rocket League's Stats API on this computer. */
export interface RlApiStatus {
  installs: string[];
  enabled: boolean;
  needsRestart: boolean;
  needsAdmin: boolean;
  port: number;
  connected: boolean;
  error?: string;
}

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
  /** Recording health; every field optional, missing = fine / unknown. */
  health?: MatchHealth;
}

/** How well a recording went. The recorder fills what it knows; the UI badges anything set. */
export interface MatchHealth {
  /** Recording was interrupted and resumed, or started after the match did. */
  partial?: boolean;
  /** The game's live API never answered, so plays came from the screen only. */
  apiUnavailable?: boolean;
  /** Share of frames dropped while encoding, 0..1. */
  droppedFrames?: number;
  /** Disk space ran low while recording. */
  lowDisk?: boolean;
  /** Encoder that was used, e.g. "NVENC H.264" (shown when it differs from the one picked). */
  encoder?: string;
}

/** Where a clip of the requested range will really start and end. */
export interface CutPoints {
  startMs: number;
  endMs: number;
  /** True when frame-accurate cuts are on (the range is cut exactly). */
  exact: boolean;
}

export interface ScrubStrip {
  /** One JPG holding `frames` stills side by side; still i shows the middle of slot i. */
  path: string;
  frames: number;
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
  holdMargin?: number;
  /** Corner/edge the HUD element is pinned to; the region is then measured on a 16:9 frame (or anchorAspect). */
  anchor?: Anchor;
  anchorAspect?: number;
  /** Keep only these colours before comparing (HUD colours), as HSV ranges. */
  colorFilter?: ColorFilter;
  /** One value per reference image: the detector reads which one is on screen (a counter, a state). */
  values?: string[];
  valueRule?: ValueRule;
  /** Event kinds dropped while this element is on screen (spectating, killcam...). */
  blocks?: string[];
  blockMarginMs?: number;
  enabled: boolean;
  notes?: string;
}

export type Anchor = "top-left" | "top" | "top-right" | "left" | "center" | "right" | "bottom-left" | "bottom" | "bottom-right";

export interface HsvRange {
  /** Degrees 0–360; from > to wraps through red. */
  hue: [number, number];
  sat: [number, number];
  val: [number, number];
}

export interface ColorFilter {
  ranges: HsvRange[];
}

export interface ValueRule {
  votes: number;
  window: number;
  fire: "change" | "increase";
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
  | "heart"
  | "goal"
  | "car"
  | "target";

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
  /** Absolute path of the game's bundled art (agents/, maps/, ranks/), when it has any. */
  assetsDir?: string | null;
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
  /** Your seconds before/after per kind of play (over the game's and built-in defaults). */
  timing?: Record<string, ClipTiming>;
}

export interface ClipTiming {
  before: number;
  after: number;
}

export interface Settings {
  recordingsDir: string;
  fps: number;
  bitrateKbps: number;
  /** Constant quality (small files in menus, sharp fights) or constant bitrate. */
  rateControl: "quality" | "cbr";
  quality: "high" | "medium" | "low";
  /** Seconds kept from before a recording starts (0 = off). */
  prerollSecs: number;
  /** Laptops with two GPUs: record on the high-performance one. */
  gpuPreference: boolean;
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

/** Where a detector matched: box as fractions of the game picture, template size and reference image. */
export interface MatchLocation {
  score: number;
  region: Region;
  /** Template size relative to the reference (1 = as cut). */
  scale: number;
  /** Index into the detector's reference images. */
  reference: number;
}

export interface TestHit extends MatchLocation {
  tMs: number;
}

export interface TestResult {
  events: GameEvent[];
  samples: Array<[number, number]>;
  durationMs: number;
  /** Where each detection matched. */
  hits: TestHit[];
  /** Stopped early with `studio_cancel`; samples cover only part of the recording. */
  cancelled: boolean;
}

/** A recording as Studio shows it: size, length, frame rate and the game picture inside any black bars. */
export interface VideoInfo {
  width: number;
  height: number;
  durationMs: number;
  /** Frames per second; 0 when unknown. */
  fps: number;
  /** The game picture in video pixels, or null when it fills the frame. */
  content: { x: number; y: number; w: number; h: number } | null;
}

export interface ProcessingProgress {
  matchId: string;
  stage: "waiting" | "scanning" | "postMatch" | "details" | "splitting" | "clipping" | "done" | "error";
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
  /** A Studio test is running: progress 0..1. */
  "studio-progress": { task: "test"; progress: number };
}
