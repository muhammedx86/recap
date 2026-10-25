// In-memory backend used when the UI runs outside Tauri (browser dev, screenshots).
// It implements every command in docs/API.md with realistic data so each screen can be exercised.

import { BUILTIN_EVENT_TYPES, EVENT_COLORS, EVENT_ICONS, ROUND_KINDS, eventId } from "./eventTypes";
import type {
  AudioInput,
  ClipRecord,
  DetectorDef,
  EventMap,
  EventTypeDef,
  EventTypeInput,
  DetailValue,
  FrameReadResult,
  FrameResult,
  ReaderDef,
  ReaderInfo,
  ReaderTestResult,
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
  VideoInfo,
} from "./types";

const ROOT = "C:\\Users\\William\\Videos\\Recap";
const MIN = 60_000;
const DAY = 24 * 60 * MIN;

// ---------- deterministic randomness ----------
function rng(seed: number) {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return ((s >>> 0) % 100000) / 100000;
  };
}

// ---------- events bus ----------
type Handler = (p: any) => void;
const listeners = new Map<string, Set<Handler>>();

export function listen<K extends keyof EventMap>(name: K, handler: (p: EventMap[K]) => void): () => void {
  if (!listeners.has(name)) listeners.set(name, new Set());
  listeners.get(name)!.add(handler as Handler);
  return () => listeners.get(name)?.delete(handler as Handler);
}

function emit<K extends keyof EventMap>(name: K, payload: EventMap[K]) {
  listeners.get(name)?.forEach((h) => h(payload));
}

// ---------- games ----------
const GAME_COLORS: Record<string, string> = {
  valorant: "#ff5a5f",
  "league-of-legends": "#c8a14b",
  cs2: "#e8913a",
  "marvel-rivals": "#8f6bff",
  fortnite: "#4fb3ff",
  "r6-siege": "#4fd1c5",
  manual: "#8d9099",
};

function detector(partial: Partial<DetectorDef> & Pick<DetectorDef, "id" | "label" | "event">, gameId: string, ready: boolean): GameInfo["detectors"][number] {
  const box: Region = partial.templateBox ?? { x: 0.455, y: 0.735, w: 0.09, h: 0.075 };
  const def: DetectorDef = {
    region: { x: box.x - box.w * 0.25, y: box.y - box.h * 0.25, w: box.w * 1.5, h: box.h * 1.5 },
    templateBox: box,
    templates: ready ? [`templates/${partial.id}-1.png`, `templates/${partial.id}-2.png`] : [],
    templateFrameHeight: 1080,
    method: "ncc",
    threshold: 0.82,
    cooldownMs: 1500,
    minConsecutive: 2,
    sampleFps: 6,
    scales: [],
    enabled: true,
    ...partial,
  };
  return {
    ...def,
    ready: ready && def.enabled,
    user: ready,
    templateUrls: def.templates.map((t) => `${ROOT}\\..\\games\\${gameId}\\${t.replace(/\//g, "\\")}`),
  };
}

/** Stored game shape; event types, rounds and match detection are derived in gameInfo(). */
type BaseGame = Omit<GameInfo, "eventTypes" | "hasRounds" | "matchDetection" | "readers">;

// ---------- screen readers (match details) ----------
const px = (x: number, y: number, w: number, h: number): Region => ({ x: x / 1920, y: y / 1080, w: w / 1920, h: h / 1080 });
const VAL_MAPS = ["Abyss", "Ascent", "Bind", "Breeze", "Corrode", "Fracture", "Haven", "Icebox", "Lotus", "Pearl", "Split", "Sunset"];
const CARD_STEP = 311 / 1920;

function reader(partial: Partial<ReaderDef> & Pick<ReaderDef, "id" | "label" | "field" | "region">, gameId: string): ReaderInfo {
  const def: ReaderDef = { kind: "text", when: "end", sampleFps: 1, threshold: 0.75, method: "ncc", templateFrameHeight: 1080, enabled: true, ...partial };
  const optionUrls: Record<string, string[]> = {};
  for (const o of def.options ?? []) optionUrls[o.value] = o.templates.map((t) => `${ROOT}\\..\\games\\${gameId}\\${t.replace(/\//g, "\\")}`);
  const ready = def.enabled && (def.kind === "text" || (def.options ?? []).some((o) => o.templates.length > 0));
  return { ...def, ready, user: false, optionUrls };
}

const readersByGame: Record<string, ReaderInfo[]> = {
  valorant: [
    reader({ id: "end-map", label: "Map (end screen)", field: "map", region: px(92, 12, 170, 17), values: VAL_MAPS }, "valorant"),
    reader(
      {
        id: "end-kda",
        label: "Your K/D/A",
        field: "kda",
        region: px(200, 608, 278, 58),
        slots: { count: 5, stepX: CARD_STEP, stepY: 0 },
        highlight: { box: { x: 0.1, y: -1.17, w: 0.8, h: 0.95 }, color: "#e2e88f", tolerance: 40 },
        pattern: String.raw`(?P<kills>\d+)\D{1,4}(?P<deaths>\d+)\D{1,4}(?P<assists>\d+)`,
      },
      "valorant",
    ),
    reader(
      {
        id: "agent-portrait",
        label: "Your agent",
        field: "character",
        kind: "choice",
        when: "any",
        region: px(1180, 960, 96, 96),
        options: [
          { value: "Jett", templates: ["templates/agent-jett-1.png", "templates/agent-jett-2.png"] },
          { value: "Sage", templates: ["templates/agent-sage-1.png"] },
        ],
      },
      "valorant",
    ),
  ],
};

/** Custom event types per game (built-ins are added in gameInfo()). */
const customTypes: Record<string, EventTypeDef[]> = {
  valorant: [{ id: "ult_used", label: "Ult used", icon: "zap", color: "#c99bff", builtin: false }],
  cs2: [{ id: "clutch", label: "Clutch", icon: "crown", color: "#e9d38a", builtin: false }],
  "marvel-rivals": [{ id: "team_up", label: "Team-up", icon: "heart", color: "#ff9bd2", builtin: false }],
};

let games: BaseGame[] = [
  {
    id: "valorant",
    name: "VALORANT",
    processes: ["VALORANT-Win64-Shipping.exe"],
    capture: "game",
    api: "riot-client",
    preferApi: true,
    autoClipEvents: ["kill"],
    color: GAME_COLORS.valorant,
    enabled: true,
    userModified: true,
    hasPostMatch: false,
    detectors: [
      detector({ id: "kill-banner", label: "Kill banner", event: "kill" }, "valorant", true),
      detector({ id: "death-recap", label: "Death recap panel", event: "death", templateBox: { x: 0.36, y: 0.08, w: 0.28, h: 0.06 } }, "valorant", false),
      detector({ id: "buy-phase", label: "Buy phase banner", event: "round_start", templateBox: { x: 0.42, y: 0.12, w: 0.16, h: 0.05 } }, "valorant", true),
      detector({ id: "round-won", label: "Round won banner", event: "round_win", templateBox: { x: 0.38, y: 0.22, w: 0.24, h: 0.09 } }, "valorant", true),
      detector({ id: "round-lost", label: "Round lost banner", event: "round_loss", templateBox: { x: 0.38, y: 0.22, w: 0.24, h: 0.09 } }, "valorant", true),
      detector({ id: "ult-cast", label: "Ultimate cast", event: "ult_used", templateBox: { x: 0.47, y: 0.88, w: 0.06, h: 0.07 } }, "valorant", true),
      detector({ id: "match-found", label: "Agent select ends", event: "match_start", templateBox: { x: 0.4, y: 0.45, w: 0.2, h: 0.06 } }, "valorant", true),
      detector(
        {
          id: "spectating",
          label: "Spectating (hides kills)",
          event: "_blocker",
          anchor: "bottom-left",
          region: { x: 0.047, y: 0.771, w: 0.109, h: 0.056 },
          templateBox: { x: 0.061, y: 0.785, w: 0.083, h: 0.025 },
          templates: ["templates/spectating-1.png", "templates/spectating-2.png"],
          method: "edges",
          blocks: ["kill", "assist"],
          blockMarginMs: 1500,
        },
        "valorant",
        true,
      ),
      detector(
        {
          id: "multikill-badge",
          label: "Multikill badge",
          event: "multikill",
          anchor: "bottom",
          region: { x: 0.46, y: 0.8, w: 0.08, h: 0.06 },
          templateBox: { x: 0.47, y: 0.81, w: 0.06, h: 0.04 },
          templates: ["templates/2x.png", "templates/3x.png", "templates/4x.png"],
          values: ["2", "3", "4"],
          valueRule: { votes: 3, window: 5, fire: "change" },
          colorFilter: { ranges: [{ hue: [40, 70], sat: [0.45, 1], val: [0.55, 1] }] },
        },
        "valorant",
        true,
      ),
    ],
  },
  {
    id: "league-of-legends",
    name: "League of Legends",
    processes: ["League of Legends.exe"],
    capture: "game",
    api: "league-live-client",
    preferApi: true,
    autoClipEvents: ["kill", "multikill", "objective"],
    color: GAME_COLORS["league-of-legends"],
    enabled: true,
    userModified: false,
    hasPostMatch: false,
    detectors: [],
  },
  {
    id: "cs2",
    name: "Counter-Strike 2",
    processes: ["cs2.exe"],
    capture: "game",
    api: "cs2-gsi",
    preferApi: true,
    autoClipEvents: ["kill", "ace"],
    color: GAME_COLORS.cs2,
    enabled: true,
    userModified: false,
    hasPostMatch: false,
    detectors: [],
  },
  {
    id: "marvel-rivals",
    name: "Marvel Rivals",
    processes: ["Marvel-Win64-Shipping.exe"],
    capture: "game",
    api: null,
    preferApi: true,
    autoClipEvents: ["kill"],
    color: GAME_COLORS["marvel-rivals"],
    enabled: true,
    userModified: false,
    hasPostMatch: false,
    detectors: [
      detector({ id: "elimination", label: "Elimination callout", event: "kill", templateBox: { x: 0.44, y: 0.6, w: 0.12, h: 0.05 } }, "marvel-rivals", false),
      detector({ id: "knocked-out", label: "You were knocked out", event: "death", templateBox: { x: 0.4, y: 0.3, w: 0.2, h: 0.08 } }, "marvel-rivals", false),
      detector({ id: "round-start", label: "Round start countdown", event: "round_start", templateBox: { x: 0.44, y: 0.18, w: 0.12, h: 0.06 } }, "marvel-rivals", true),
      detector({ id: "round-won", label: "Round victory", event: "round_win", templateBox: { x: 0.36, y: 0.3, w: 0.28, h: 0.1 } }, "marvel-rivals", true),
      detector({ id: "round-lost", label: "Round defeat", event: "round_loss", templateBox: { x: 0.36, y: 0.3, w: 0.28, h: 0.1 } }, "marvel-rivals", true),
    ],
  },
  {
    id: "r6-siege",
    name: "Rainbow Six Siege",
    processes: ["RainbowSix.exe", "RainbowSix_Vulkan.exe"],
    capture: "game",
    api: null,
    preferApi: true,
    autoClipEvents: ["kill", "ace"],
    color: GAME_COLORS["r6-siege"],
    enabled: true,
    userModified: true,
    hasPostMatch: true,
    detectors: [
      detector({ id: "kill-feed", label: "Your kill in the feed", event: "kill", templateBox: { x: 0.78, y: 0.18, w: 0.2, h: 0.035 } }, "r6-siege", true),
      detector({ id: "action-phase", label: "Action phase starts", event: "round_start", templateBox: { x: 0.45, y: 0.04, w: 0.1, h: 0.04 } }, "r6-siege", true),
      detector({ id: "round-won", label: "Round won", event: "round_win", templateBox: { x: 0.38, y: 0.32, w: 0.24, h: 0.08 } }, "r6-siege", true),
      detector({ id: "round-lost", label: "Round lost", event: "round_loss", templateBox: { x: 0.38, y: 0.32, w: 0.24, h: 0.08 } }, "r6-siege", false),
    ],
  },
  {
    id: "fortnite",
    name: "Fortnite",
    processes: ["FortniteClient-Win64-Shipping.exe"],
    capture: "game",
    api: null,
    preferApi: true,
    autoClipEvents: ["kill"],
    color: GAME_COLORS.fortnite,
    enabled: false,
    userModified: false,
    hasPostMatch: true,
    detectors: [],
  },
];

// ---------- settings ----------
let settings: Settings = {
  recordingsDir: ROOT,
  fps: 60,
  bitrateKbps: 12000,
  rateControl: "quality",
  quality: "medium",
  prerollSecs: 10,
  gpuPreference: true,
  outputHeight: null,
  encoder: "auto",
  recordDesktopAudio: true,
  recordMicrophone: false,
  microphoneDevice: null,
  bookmarkHotkey: "F8",
  manualRecordHotkey: "F9",
  autoClip: { enabled: true, secondsBefore: 5, secondsAfter: 3, mergeGapSeconds: 2, eventsOverride: [], preciseCuts: false },
  scanAfterMatch: true,
  analysisHeight: 720,
  gamesEnabled: { fortnite: false },
  storageLimitGb: 200,
  closeToTray: true,
  notifyOnBookmark: true,
  separateMicTrack: true,
  launchAtStartup: false,
  startMinimized: true,
  notifications: true,
  notificationCorner: "top-right",
  splitMatches: true,
};

// ---------- matches, events, clips ----------
const now = Date.now();
const startOfToday = new Date(new Date().setHours(0, 0, 0, 0)).getTime();

interface MatchSeed {
  id: string;
  gameId: string;
  start: number;
  durMin: number;
  status: MatchRecord["status"];
  title?: string;
  favorite?: boolean;
  error?: string;
  kills: number;
  deaths: number;
  assists: number;
  extra?: Array<[string, number]>;
  /** Round results; `starts` adds round_start markers, `meta` sets meta.round (API games). */
  rounds?: { won: number; lost: number; starts: boolean; meta: boolean };
}

const seeds: MatchSeed[] = [
  { id: "m-live", gameId: "valorant", start: now - 23 * MIN, durMin: 23, status: "recording", kills: 0, deaths: 0, assists: 0 },
  { id: "m-val-1", gameId: "valorant", start: startOfToday + 13.5 * 60 * MIN, durMin: 41.2, status: "ready", kills: 24, deaths: 15, assists: 7, extra: [["bookmark", 2], ["ult_used", 6]], rounds: { won: 13, lost: 9, starts: true, meta: false } },
  { id: "m-val-2", gameId: "valorant", start: startOfToday + 12.6 * 60 * MIN, durMin: 38.4, status: "ready", kills: 19, deaths: 20, assists: 11, extra: [["ult_used", 4]], rounds: { won: 14, lost: 16, starts: true, meta: false } },
  { id: "m-val-3", gameId: "valorant", start: startOfToday + 9.2 * 60 * MIN, durMin: 33.1, status: "ready", kills: 26, deaths: 14, assists: 4, extra: [["ace", 1], ["ult_used", 5]], rounds: { won: 13, lost: 7, starts: true, meta: false } },
  { id: "m-lol-1", gameId: "league-of-legends", start: startOfToday + 11 * 60 * MIN, durMin: 31.4, status: "processing", kills: 9, deaths: 4, assists: 13, extra: [["objective", 5], ["multikill", 1]] },
  { id: "m-cs-1", gameId: "cs2", start: startOfToday - DAY + 21 * 60 * MIN, durMin: 47.8, status: "ready", kills: 27, deaths: 18, assists: 4, extra: [["mvp", 4], ["bomb_plant", 3], ["ace", 1], ["clutch", 2]], rounds: { won: 13, lost: 11, starts: true, meta: true } },
  { id: "m-mr-1", gameId: "marvel-rivals", start: startOfToday - DAY + 19 * 60 * MIN, durMin: 18.6, status: "ready", kills: 0, deaths: 0, assists: 0, extra: [["bookmark", 3], ["team_up", 4]], rounds: { won: 2, lost: 1, starts: true, meta: false } },
  { id: "m-r6-1", gameId: "r6-siege", start: startOfToday - 2 * DAY + 20.5 * 60 * MIN, durMin: 33.5, status: "ready", kills: 11, deaths: 6, assists: 3, extra: [["ace", 1]], rounds: { won: 5, lost: 4, starts: true, meta: false } },
  { id: "m-val-4", gameId: "valorant", start: startOfToday - 2 * DAY + 22.4 * 60 * MIN, durMin: 29.5, status: "ready", kills: 12, deaths: 17, assists: 6, rounds: { won: 6, lost: 13, starts: true, meta: false } },
  { id: "m-val-5", gameId: "valorant", start: startOfToday - 2 * DAY + 21.6 * 60 * MIN, durMin: 44.2, status: "ready", favorite: true, kills: 31, deaths: 18, assists: 9, extra: [["clutch", 2], ["ult_used", 7]], rounds: { won: 13, lost: 11, starts: true, meta: false } },
  { id: "m-lol-2", gameId: "league-of-legends", start: startOfToday - 3 * DAY + 22 * 60 * MIN, durMin: 36.1, status: "ready", title: "Kai'Sa pentakill game", favorite: true, kills: 17, deaths: 3, assists: 9, extra: [["multikill", 3], ["objective", 7]] },
  { id: "m-man-1", gameId: "manual", start: startOfToday - 3 * DAY + 17 * 60 * MIN, durMin: 12.3, status: "ready", title: "Slay the Spire 2 run", kills: 0, deaths: 0, assists: 0, extra: [["bookmark", 4]] },
  { id: "m-cs-2", gameId: "cs2", start: startOfToday - 4 * DAY + 20 * 60 * MIN, durMin: 2.1, status: "error", error: "The recorder stopped unexpectedly. The file may be incomplete.", kills: 1, deaths: 1, assists: 0 },
];

function gameName(id: string) {
  if (id === "manual") return "Manual recording";
  return games.find((g) => g.id === id)?.name ?? id;
}

function makeEvents(seed: MatchSeed): GameEvent[] {
  const r = rng(seed.id.split("").reduce((a, c) => a + c.charCodeAt(0) * 31, 7));
  const dur = seed.durMin * MIN;
  const evs: GameEvent[] = [];
  const isApi = seed.gameId === "league-of-legends" || seed.gameId === "cs2";
  const VISION: Record<string, string> = {
    death: "death-recap",
    round_start: "round-start",
    round_win: "round-won",
    round_loss: "round-lost",
    ult_used: "ult-cast",
    clutch: "clutch-banner",
    team_up: "team-up",
  };
  const apiKinds = ["kill", "death", "assist", "mvp", "bomb_plant", "bomb_defuse", "ace", "multikill", "objective", ...ROUND_KINDS];
  const src = (kind: string): GameEvent["source"] =>
    kind === "bookmark"
      ? { type: "manual" }
      : isApi && apiKinds.includes(kind)
        ? { type: "api", name: seed.gameId === "cs2" ? "cs2-gsi" : "league-live-client" }
        : { type: "vision", detector: VISION[kind] ?? "kill-banner" };
  const add = (kind: string, n: number) => {
    for (let i = 0; i < n; i++) {
      const tMs = Math.round(40_000 + r() * (dur - 60_000));
      const e: GameEvent = { kind, tMs, source: src(kind) };
      if (e.source.type === "vision") e.confidence = 0.8 + r() * 0.19;
      if (kind === "multikill") e.meta = { count: 2 + Math.floor(r() * 3) };
      evs.push(e);
    }
  };
  add("kill", seed.kills);
  add("death", seed.deaths);
  add("assist", seed.assists);
  for (const [k, n] of seed.extra ?? []) add(k, n);
  if (seed.rounds) {
    const { won, lost, starts, meta } = seed.rounds;
    const n = won + lost;
    // Spread wins and losses so the score swings a little, like a real match.
    const results: Array<"win" | "loss"> = [];
    let w = won;
    let l = lost;
    for (let i = 0; i < n; i++) {
      const pickWin = w > 0 && (l === 0 || r() < w / (w + l) + (i % 5 === 0 ? 0.15 : -0.05));
      results.push(pickWin ? "win" : "loss");
      if (pickWin) w--;
      else l--;
    }
    const first = 25_000;
    const len = (dur - first - 20_000) / n;
    results.forEach((res, i) => {
      const s = Math.round(first + i * len + r() * 2000);
      const end = Math.round(first + (i + 1) * len - 3000 - r() * 4000);
      const m = meta ? { round: i + 1 } : undefined;
      if (starts) evs.push({ kind: "round_start", tMs: s, source: src("round_start"), ...(m ? { meta: m } : {}), ...(isApi ? {} : { confidence: 0.86 + r() * 0.12 }) });
      const kind = res === "win" ? "round_win" : "round_loss";
      evs.push({ kind, tMs: end, source: src(kind), ...(m ? { meta: m } : {}), ...(isApi ? {} : { confidence: 0.84 + r() * 0.14 }) });
    });
  }
  evs.sort((a, b) => a.tMs - b.tMs);
  return evs.map((e, i) => ({ ...e, id: i + 1 }));
}

const eventsByMatch = new Map<string, GameEvent[]>();
let nextEventId = 10_000;

function countEvents(evs: GameEvent[]): Record<string, number> {
  const out: Record<string, number> = {};
  for (const e of evs) out[e.kind] = (out[e.kind] ?? 0) + 1;
  return out;
}

const pathFor = (gameId: string, start: number, ext = "mp4") => {
  const d = new Date(start);
  const stamp = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}_${String(d.getHours()).padStart(2, "0")}-${String(d.getMinutes()).padStart(2, "0")}`;
  return `${ROOT}\\${gameId}\\${stamp}.${ext}`;
};

const dv = (value: string, source: DetailValue["source"], confidence?: number): DetailValue => ({ value, source, ...(confidence !== undefined ? { confidence } : {}) });
const seedDetails: Record<string, Record<string, DetailValue>> = {
  "m-val-1": {
    character: dv("Jett", "screen", 0.94),
    map: dv("Ascent", "api"),
    mode: dv("Competitive", "api"),
    rank: dv("Gold 2", "api"),
    player: dv("11lumin#1111", "api"),
    kills: dv("24", "screen", 0.9),
    deaths: dv("15", "screen", 0.9),
    assists: dv("7", "screen", 0.9),
    result: dv("win", "api"),
    score: dv("13-9", "api"),
  },
  "m-val-2": {
    character: dv("Clove", "screen", 0.91),
    map: dv("Lotus", "api"),
    mode: dv("Competitive", "api"),
    rank: dv("Gold 2", "api"),
    player: dv("11lumin#1111", "api"),
    kills: dv("19", "screen", 0.9),
    deaths: dv("20", "screen", 0.9),
    assists: dv("11", "screen", 0.9),
    result: dv("loss", "api"),
    score: dv("14-16", "api"),
  },
  "m-val-3": {
    character: dv("Reyna", "screen", 0.95),
    map: dv("Haven", "api"),
    mode: dv("Competitive", "api"),
    rank: dv("Gold 2", "api"),
    player: dv("11lumin#1111", "api"),
    kills: dv("26", "screen", 0.9),
    deaths: dv("14", "screen", 0.9),
    assists: dv("4", "screen", 0.9),
    result: dv("win", "api"),
    score: dv("13-7", "api"),
  },
  "m-val-4": {
    character: dv("Omen", "screen", 0.93),
    map: dv("Sunset", "api"),
    mode: dv("Competitive", "api"),
    rank: dv("Gold 1", "api"),
    player: dv("11lumin#1111", "api"),
    kills: dv("12", "screen", 0.9),
    deaths: dv("17", "screen", 0.9),
    assists: dv("6", "screen", 0.9),
    result: dv("loss", "api"),
    score: dv("6-13", "api"),
  },
  "m-val-5": {
    character: dv("Neon", "screen", 0.92),
    map: dv("Bind", "api"),
    mode: dv("Unrated", "api"),
    player: dv("11lumin#1111", "api"),
    kills: dv("31", "screen", 0.9),
    deaths: dv("18", "screen", 0.9),
    assists: dv("9", "screen", 0.9),
    result: dv("win", "api"),
    score: dv("13-11", "api"),
  },
  "m-lol-2": {
    character: dv("Kai'Sa", "api"),
    map: dv("Summoner's Rift", "api"),
    mode: dv("Ranked Solo", "manual"),
    kills: dv("17", "api"),
    deaths: dv("3", "api"),
    assists: dv("9", "api"),
    result: dv("win", "api"),
  },
  "m-cs-1": {
    map: dv("Mirage", "api"),
    mode: dv("Premier", "api"),
    kills: dv("27", "api"),
    deaths: dv("18", "api"),
    assists: dv("4", "api"),
    result: dv("win", "api"),
    score: dv("13-11", "api"),
  },
  "m-mr-1": { character: dv("Magik", "manual"), kills: dv("21", "events"), deaths: dv("9", "events"), result: dv("win", "events"), score: dv("2-1", "events") },
  "m-r6-1": { map: dv("Clubhouse", "screen", 0.82), kills: dv("11", "events"), deaths: dv("6", "events"), assists: dv("3", "events"), result: dv("loss", "screen", 0.77), score: dv("4-5", "screen", 0.8) },
};

let matches: MatchRecord[] = seeds.map((s) => {
  const evs = s.status === "recording" ? [] : makeEvents(s);
  eventsByMatch.set(s.id, evs);
  return {
    id: s.id,
    gameId: s.gameId,
    gameName: gameName(s.gameId),
    path: pathFor(s.gameId, s.start),
    startedUnixMs: s.start,
    durationMs: Math.round(s.durMin * MIN),
    sizeBytes: Math.round(s.durMin * 60 * 1.5e6),
    status: s.status,
    title: s.title ?? null,
    favorite: s.favorite ?? false,
    thumbnail: s.status === "recording" ? null : pathFor(s.gameId, s.start, "jpg"),
    eventCounts: countEvents(evs),
    error: s.error ?? null,
    details: structuredClone(seedDetails[s.id] ?? {}),
  };
});

function kindsIn(matchId: string, a: number, b: number) {
  return [...new Set((eventsByMatch.get(matchId) ?? []).filter((e) => e.tMs >= a && e.tMs <= b).map((e) => e.kind))];
}

let clips: ClipRecord[] = [];
function seedClips() {
  for (const m of matches.filter((m) => m.status === "ready")) {
    const evs = (eventsByMatch.get(m.id) ?? []).filter((e) => ["kill", "multikill", "ace", "bookmark"].includes(e.kind)).slice(0, 4);
    evs.forEach((e, i) => {
      const startMs = Math.max(0, e.tMs - 5000);
      const endMs = e.tMs + 3000;
      clips.push({
        id: `c-${m.id}-${i}`,
        matchId: m.id,
        path: `${ROOT}\\clips\\${m.gameId}-${m.id}-${i}.mp4`,
        startMs,
        endMs,
        durationMs: endMs - startMs,
        kind: e.kind === "bookmark" ? "manual" : "auto",
        title: `${m.gameName} ${e.kind === "bookmark" ? "bookmark" : e.kind} at ${fmt(e.tMs)}`,
        eventKinds: kindsIn(m.id, startMs, endMs),
        createdUnixMs: m.startedUnixMs + m.durationMs + 60_000 + i * 1000,
        thumbnail: `${ROOT}\\clips\\${m.gameId}-${m.id}-${i}.jpg`,
        sizeBytes: 12_000_000 + i * 900_000,
      });
    });
  }
  clips.push({
    id: "c-montage-1",
    matchId: null,
    path: `${ROOT}\\montages\\best-of-the-week.mp4`,
    startMs: 0,
    endMs: 64_000,
    durationMs: 64_000,
    kind: "montage",
    title: "Best of the week",
    eventKinds: ["kill", "multikill", "ace"],
    createdUnixMs: now - 2 * 60 * MIN,
    thumbnail: `${ROOT}\\montages\\league-of-legends-best.jpg`,
    sizeBytes: 98_000_000,
  });
}
seedClips();

function fmt(ms: number) {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

// ---------- recorder status ----------
let status: RecorderStatus = {
  state: "recording",
  matchId: "m-live",
  gameId: "valorant",
  gameName: "VALORANT",
  startedUnixMs: now - 23 * MIN,
  encoder: "NVENC H.264",
  capture: "game",
  bookmarks: 2,
  message: null,
};

// Simulated post-match processing for the League match.
let lolProgress = 0.34;
const processingTimer = setInterval(() => {
  lolProgress = Math.min(1, lolProgress + 0.004);
  const m = matches.find((x) => x.id === "m-lol-1");
  if (!m) return;
  if (lolProgress >= 1) {
    m.status = "ready";
    emit("processing-progress", { matchId: m.id, stage: "done", progress: 1 });
    emit("library-changed", { matchId: m.id });
    clearInterval(processingTimer);
  } else {
    emit("processing-progress", { matchId: m.id, stage: lolProgress < 0.8 ? "scanning" : "clipping", progress: lolProgress });
  }
}, 400);

export function processingSnapshot(): Record<string, number> {
  return matches.some((m) => m.id === "m-lol-1" && m.status === "processing") ? { "m-lol-1": lolProgress } : {};
}

// ---------- media ----------
function gameFromPath(path: string): string {
  const id = Object.keys(GAME_COLORS).find((g) => path.toLowerCase().includes(g));
  return id ?? "manual";
}

/** Abstract poster art for a file path (deterministic). */
export function mediaUrl(path: string): string {
  // Demo art served from ui/public.
  if (path.startsWith("/demo-assets/")) return path;
  const g = gameFromPath(path);
  const c = GAME_COLORS[g] ?? "#8d9099";
  const r = rng(path.split("").reduce((a, ch) => a + ch.charCodeAt(0) * 13, 3));
  const hz = 52 + r() * 14;
  const shapes = Array.from({ length: 7 }, () => {
    const x = r() * 1600;
    const w = 60 + r() * 260;
    const h = 120 + r() * 380;
    return `<rect x="${x.toFixed(0)}" y="${(hz * 9 - h).toFixed(0)}" width="${w.toFixed(0)}" height="${h.toFixed(0)}" fill="#0b0d10" opacity="${(0.35 + r() * 0.5).toFixed(2)}"/>`;
  }).join("");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1600 900" preserveAspectRatio="xMidYMid slice">
<defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${c}" stop-opacity=".55"/><stop offset=".62" stop-color="#1b1f27"/><stop offset="1" stop-color="#0d0f13"/></linearGradient>
<radialGradient id="l" cx="${(20 + r() * 60).toFixed(0)}%" cy="35%" r="55%"><stop offset="0" stop-color="#fff" stop-opacity=".22"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient></defs>
<rect width="1600" height="900" fill="url(#s)"/><rect width="1600" height="900" fill="url(#l)"/>${shapes}
<rect y="${(hz * 9).toFixed(0)}" width="1600" height="${(900 - hz * 9).toFixed(0)}" fill="#0d0f13" opacity=".85"/>
<circle cx="800" cy="450" r="3" fill="#fff" opacity=".7"/></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

/** A 1920x1080 end-of-match scoreboard: map, score and five player cards (yours has a yellow name). */
function endScreenFrame(): string {
  const cards = [
    ["BRIMSTONE", "DOMA", "12 / 14 / 6"],
    ["SAGE", "TSACK", "8 / 13 / 11"],
    ["JETT", "WILLIAM", "24 / 15 / 7"],
    ["OMEN", "MISTIC", "15 / 12 / 9"],
    ["PHOENIX", "PLAYER", "10 / 16 / 4"],
  ]
    .map(([agent, name, kda], i) => {
      const x = 188 + i * 311;
      const me = i === 2;
      return `<g><rect x="${x}" y="495" width="300" height="260" fill="#11161c" opacity=".82"/>
<text x="${x + 150}" y="538" text-anchor="middle" font-family="sans-serif" font-size="22" letter-spacing="3" fill="#c9ccd1">${agent}</text>
<text x="${x + 150}" y="582" text-anchor="middle" font-family="sans-serif" font-weight="800" font-size="38" fill="${me ? "#e2e88f" : "#f1efe9"}">${name}</text>
<rect x="${x + 50}" y="598" width="200" height="2" fill="#f1efe9" opacity=".3"/>
<text x="${x + 150}" y="650" text-anchor="middle" font-family="sans-serif" font-size="36" fill="#f1efe9">${kda}</text>
<text x="${x + 150}" y="676" text-anchor="middle" font-family="sans-serif" font-size="18" letter-spacing="2" fill="#9aa0a8">KDA</text></g>`;
    })
    .join("");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1920" height="1080" viewBox="0 0 1920 1080">
<defs><linearGradient id="v" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2b8a78"/><stop offset="1" stop-color="#0d2a2a"/></linearGradient></defs>
<rect width="1920" height="1080" fill="url(#v)"/>
<text x="960" y="430" text-anchor="middle" font-family="sans-serif" font-weight="900" font-size="420" fill="#7fe0c4" opacity=".55">VICTORY</text>
<text x="96" y="27" font-family="sans-serif" font-size="15" letter-spacing="1.5" fill="#e8ece9">ASCENT</text>
<text x="96" y="43" font-family="sans-serif" font-size="13" letter-spacing="1.5" fill="#c4ccc8">COMPETITIVE</text>
<text x="228" y="132" font-family="sans-serif" font-weight="900" font-size="96" fill="#7fe0c4">13</text>
<text x="1622" y="132" font-family="sans-serif" font-weight="900" font-size="96" fill="#ff5a5f">9</text>
${cards}
<rect x="830" y="855" width="260" height="56" fill="#e9e7e1"/><text x="960" y="892" text-anchor="middle" font-family="sans-serif" font-size="22" letter-spacing="3" fill="#111">CONTINUE</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

/**
 * Detector Studio in the browser shows these drawn frames instead of a video. To try the real
 * video player, set localStorage "studio-mock-video" to {"url": "...mp4", "info": VideoInfo}.
 */
export function mockStudioVideo(): { url: string; info: VideoInfo } | null {
  try {
    const raw = localStorage.getItem("studio-mock-video");
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

/** A 1920x1080 HUD-like frame with a kill banner, for Detector Studio. */
export function studioFrame(t: number, path: string): string {
  const m = matches.find((x) => x.path === path);
  if (m && m.gameId === "valorant" && t >= m.durationMs - 60_000) return endScreenFrame();
  const g = gameFromPath(path);
  const c = GAME_COLORS[g] ?? "#8d9099";
  const showBanner = Math.floor(t / 7000) % 3 !== 2;
  const banner = showBanner
    ? `<g transform="translate(960 865)"><polygon points="-86,-40 0,-82 86,-40 0,2" fill="none" stroke="#f3f0e8" stroke-width="7"/><polygon points="-46,-40 0,-62 46,-40 0,-18" fill="${c}"/><rect x="-120" y="16" width="240" height="22" fill="#f3f0e8" opacity=".9"/></g>`
    : "";
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1920" height="1080" viewBox="0 0 1920 1080">
<defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${c}" stop-opacity=".45"/><stop offset=".55" stop-color="#2a2f38"/><stop offset="1" stop-color="#14171c"/></linearGradient></defs>
<rect width="1920" height="1080" fill="url(#s)"/>
<rect x="0" y="600" width="1920" height="480" fill="#1a1d23"/>
<rect x="220" y="260" width="380" height="380" fill="#101318" opacity=".8"/><rect x="1320" y="300" width="300" height="320" fill="#101318" opacity=".7"/>
<rect x="760" y="380" width="160" height="240" fill="#0e1014" opacity=".9"/>
<g fill="#f3f0e8" opacity=".85"><rect x="40" y="40" width="320" height="18"/><rect x="40" y="70" width="220" height="12" opacity=".6"/>
<rect x="840" y="28" width="240" height="44" opacity=".25"/><rect x="1560" y="40" width="320" height="14"/><rect x="1600" y="64" width="280" height="14"/><rect x="1640" y="88" width="240" height="14"/>
<rect x="40" y="990" width="260" height="46" opacity=".3"/><rect x="1620" y="990" width="260" height="46" opacity=".3"/></g>
<circle cx="960" cy="540" r="4" fill="#7fffd4"/>${banner}
<text x="40" y="1060" font-family="sans-serif" font-size="18" fill="#f3f0e8" opacity=".5">${fmt(t)}</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

export function pickPath(directory: boolean): string {
  return directory ? "D:\\Recordings\\Recap" : "C:\\Users\\William\\Pictures\\Screenshots\\valorant-kill.png";
}

// ---------- command dispatch ----------
const delay = (ms = 120) => new Promise((r) => setTimeout(r, ms));

function gameInfo(id: string): GameInfo {
  const g = games.find((x) => x.id === id);
  if (!g) throw `Unknown game "${id}"`;
  const events = g.detectors.map((d) => d.event);
  return {
    ...g,
    enabled: settings.gamesEnabled[id] ?? true,
    eventTypes: [...BUILTIN_EVENT_TYPES, ...(customTypes[id] ?? [])],
    hasRounds: g.api === "cs2-gsi" || events.some((e) => ROUND_KINDS.includes(e)),
    readers: structuredClone(readersByGame[id] ?? []),
    assetsDir: id === "valorant" ? "/demo-assets/valorant" : null,
    matchDetection: g.api && g.api !== "riot-client" ? "api" : events.some((e) => e === "match_start" || e === "match_end") ? "detectors" : "process",
  };
}

function allEventTypes(): EventTypeDef[] {
  const out = new Map(BUILTIN_EVENT_TYPES.map((t) => [t.id, t]));
  Object.values(customTypes).forEach((list) => list.forEach((t) => out.has(t.id) || out.set(t.id, t)));
  return [...out.values()];
}

function matchDetail(id: string): MatchDetail {
  const m = matches.find((x) => x.id === id);
  if (!m) throw `Recording not found`;
  const evs = eventsByMatch.get(id) ?? [];
  m.eventCounts = countEvents(evs);
  return { match: { ...m }, events: [...evs], clips: clips.filter((c) => c.matchId === id).sort((a, b) => b.createdUnixMs - a.createdUnixMs) };
}

let studioCancelled = false;

function testDetector(det: DetectorDef, matchId: string, upTo = 1): TestResult {
  const m = matches.find((x) => x.id === matchId);
  if (!m) throw "Recording not found";
  const scanned = m.durationMs * upTo;
  const kills = (eventsByMatch.get(matchId) ?? []).filter((e) => e.kind === det.event);
  const step = Math.max(1000 / det.sampleFps, m.durationMs / 1800);
  const r = rng(Math.round(det.threshold * 1000) + matchId.length);
  const samples: Array<[number, number]> = [];
  for (let t = 0; t < scanned; t += step) {
    let s = 0.12 + r() * 0.22;
    for (const k of kills) {
      const d = t - k.tMs;
      if (d >= 0 && d < 2200) s = Math.max(s, 0.86 + r() * 0.12 - d / 40000);
    }
    if (r() > 0.995) s = 0.7 + r() * 0.2; // near misses
    samples.push([Math.round(t), Math.min(0.99, s)]);
  }
  const events: GameEvent[] = [];
  let run = 0;
  let runStart = 0;
  let peak = 0;
  let fired = false;
  let last = -Infinity;
  for (const [t, s] of samples) {
    if (s >= det.threshold) {
      if (run === 0) {
        runStart = t;
        peak = s;
        fired = false;
      }
      run++;
      peak = Math.max(peak, s);
      if (!fired && run >= det.minConsecutive) {
        fired = true;
        if (runStart - last >= det.cooldownMs) {
          last = runStart;
          events.push({ kind: det.event, tMs: runStart, source: { type: "vision", detector: det.id }, confidence: peak });
        }
      }
    } else run = 0;
  }
  const at = det.templateBox ?? det.region;
  const hits = events.map((e, i) => ({ tMs: e.tMs, score: e.confidence ?? 0.9, region: at, scale: 1, reference: i % Math.max(1, det.templates.length) }));
  return { events, samples, durationMs: m.durationMs, hits, cancelled: upTo < 1 };
}

const handlers: Record<string, (a: any) => unknown> = {
  get_status: () => ({ ...status }),
  list_matches: ({ gameId }) =>
    matches
      .filter((m) => !gameId || m.gameId === gameId)
      .map((m) => ({ ...m, eventCounts: countEvents(eventsByMatch.get(m.id) ?? []) }))
      .sort((a, b) => b.startedUnixMs - a.startedUnixMs),
  get_match: ({ id }) => matchDetail(id),
  update_match: ({ id, title, favorite }) => {
    const m = matches.find((x) => x.id === id);
    if (!m) throw "Recording not found";
    if (title !== undefined) m.title = title === "" ? null : title;
    if (favorite !== undefined) m.favorite = favorite;
    emit("library-changed", { matchId: id });
    return { ...m };
  },
  update_match_detail: ({ id, field, value }: { id: string; field: string; value: string }) => {
    const m = matches.find((x) => x.id === id);
    if (!m) throw "Recording not found";
    const v = value.trim();
    const details = { ...m.details };
    if (v) details[field] = { value: v, source: "manual" };
    else delete details[field];
    m.details = details;
    emit("library-changed", { matchId: id });
    return { ...m };
  },
  delete_match: ({ id }) => {
    matches = matches.filter((m) => m.id !== id);
    clips = clips.map((c) => (c.matchId === id ? { ...c, matchId: null } : c));
    emit("library-changed", {});
    return null;
  },
  add_event: ({ matchId, kind, tMs }) => {
    const e: GameEvent = { id: nextEventId++, kind, tMs: Math.round(tMs), source: { type: "manual" } };
    const list = eventsByMatch.get(matchId) ?? [];
    list.push(e);
    list.sort((a, b) => a.tMs - b.tMs);
    eventsByMatch.set(matchId, list);
    emit("library-changed", { matchId });
    return e;
  },
  delete_event: ({ matchId, eventId }) => {
    eventsByMatch.set(matchId, (eventsByMatch.get(matchId) ?? []).filter((e) => e.id !== eventId));
    emit("library-changed", { matchId });
    return null;
  },
  rescan_match: ({ id }) => {
    const m = matches.find((x) => x.id === id);
    if (!m) throw "Recording not found";
    m.status = "processing";
    let p = 0;
    const t = setInterval(() => {
      p += 0.1;
      if (p >= 1) {
        clearInterval(t);
        m.status = "ready";
        emit("processing-progress", { matchId: id, stage: "done", progress: 1 });
        emit("library-changed", { matchId: id });
      } else emit("processing-progress", { matchId: id, stage: "scanning", progress: p });
    }, 250);
    emit("library-changed", { matchId: id });
    return null;
  },
  list_clips: ({ matchId }) => clips.filter((c) => !matchId || c.matchId === matchId).sort((a, b) => b.createdUnixMs - a.createdUnixMs),
  create_clip: ({ matchId, startMs, endMs, title }) => {
    const m = matches.find((x) => x.id === matchId);
    if (!m) throw "Recording not found";
    if (endMs <= startMs) throw "The clip end must be after its start.";
    const c: ClipRecord = {
      id: `c-${Date.now()}`,
      matchId,
      path: `${ROOT}\\clips\\${m.gameId}-${Date.now()}.mp4`,
      startMs: Math.round(startMs),
      endMs: Math.round(endMs),
      durationMs: Math.round(endMs - startMs),
      kind: "manual",
      title: title || `${m.gameName} clip at ${fmt(startMs)}`,
      eventKinds: kindsIn(matchId, startMs, endMs),
      createdUnixMs: Date.now(),
      thumbnail: `${ROOT}\\clips\\${m.gameId}-${Date.now()}.jpg`,
      sizeBytes: Math.round(((endMs - startMs) / 1000) * 1.5e6),
    };
    clips.push(c);
    emit("library-changed", { matchId });
    return c;
  },
  create_montage: ({ clipIds, title }) => {
    const parts = clipIds.map((id: string) => clips.find((c) => c.id === id)).filter(Boolean) as ClipRecord[];
    if (parts.length < 2) throw "Pick at least two clips for a montage.";
    const dur = parts.reduce((a, c) => a + c.durationMs, 0);
    const c: ClipRecord = {
      id: `c-mont-${Date.now()}`,
      matchId: null,
      path: `${ROOT}\\montages\\${title.replace(/\W+/g, "-").toLowerCase()}.mp4`,
      startMs: 0,
      endMs: dur,
      durationMs: dur,
      kind: "montage",
      title,
      eventKinds: [...new Set(parts.flatMap((p) => p.eventKinds))],
      createdUnixMs: Date.now(),
      thumbnail: parts[0].thumbnail,
      sizeBytes: parts.reduce((a, p) => a + p.sizeBytes, 0),
    };
    clips.push(c);
    emit("library-changed", {});
    return c;
  },
  update_clip: ({ id, title }) => {
    const c = clips.find((x) => x.id === id);
    if (!c) throw "Clip not found";
    c.title = title;
    return { ...c };
  },
  delete_clip: ({ id }) => {
    clips = clips.filter((c) => c.id !== id);
    emit("library-changed", {});
    return null;
  },
  list_games: () => games.map((g) => gameInfo(g.id)),
  set_game_enabled: ({ gameId, enabled }) => {
    settings = { ...settings, gamesEnabled: { ...settings.gamesEnabled, [gameId]: enabled } };
    return gameInfo(gameId);
  },
  save_game: ({ game }: { game: GameInput }) => {
    if (!/^[a-z0-9_-]+$/i.test(game.id)) throw "Game id can only use letters, digits, - and _.";
    if (!game.processes.length) throw "Add at least one process name.";
    const existing = games.find((g) => g.id === game.id);
    const info: BaseGame = { ...game, enabled: true, userModified: true, detectors: existing?.detectors ?? [], hasPostMatch: false };
    games = existing ? games.map((g) => (g.id === game.id ? info : g)) : [...games, info];
    return gameInfo(game.id);
  },
  list_event_types: () => allEventTypes(),
  save_event_type: ({ gameId, eventType }: { gameId: string; eventType: EventTypeInput }) => {
    if (!games.some((g) => g.id === gameId)) throw "Unknown game";
    const label = eventType.label.trim();
    if (!label) throw "Give the event type a name.";
    if (!EVENT_ICONS.includes(eventType.icon)) throw `Unknown icon "${eventType.icon}"`;
    const list = (customTypes[gameId] ??= []);
    const id = eventType.id ?? eventId(label);
    if (!id) throw "The name needs at least one letter or digit.";
    if (BUILTIN_EVENT_TYPES.some((t) => t.id === id)) throw `“${label}” is a built-in event type. Pick it from the list instead.`;
    const existing = list.find((t) => t.id === id);
    if (!eventType.id && existing) throw `${gameInfo(gameId).name} already has an event type called “${existing.label}”.`;
    const color = eventType.color ?? existing?.color ?? EVENT_COLORS[(list.length + 3) % EVENT_COLORS.length];
    const def: EventTypeDef = { id, label, icon: eventType.icon, color, builtin: false };
    customTypes[gameId] = existing ? list.map((t) => (t.id === id ? def : t)) : [...list, def];
    return gameInfo(gameId);
  },
  delete_event_type: ({ gameId, eventTypeId }) => {
    const g = games.find((x) => x.id === gameId);
    if (!g) throw "Unknown game";
    if (BUILTIN_EVENT_TYPES.some((t) => t.id === eventTypeId)) throw "Built-in event types cannot be deleted.";
    const users = g.detectors.filter((d) => d.event === eventTypeId);
    if (users.length) throw `Detector “${users[0].label || users[0].id}” still marks this event. Change it or delete it first.`;
    customTypes[gameId] = (customTypes[gameId] ?? []).filter((t) => t.id !== eventTypeId);
    return gameInfo(gameId);
  },
  get_settings: () => structuredClone(settings),
  update_settings: ({ settings: s }) => {
    settings = structuredClone(s);
    return structuredClone(settings);
  },
  list_encoders: () => ["auto", "nvenc", "x264"],
  list_audio_inputs: (): AudioInput[] => [
    { id: "default", name: "Default microphone" },
    { id: "{0.0.1.00000000}.{a1}", name: "Microphone (Shure MV7)" },
    { id: "{0.0.1.00000000}.{b2}", name: "Headset Microphone (Arctis 7)" },
  ],
  start_manual_recording: () => {
    status = { ...status, state: "recording", matchId: `m-${Date.now()}`, gameId: "manual", gameName: "Manual recording", startedUnixMs: Date.now(), bookmarks: 0 };
    emit("status-changed", { ...status });
    return { ...status };
  },
  stop_recording: () => {
    // Stopping a game recording skips that match; the game stays open, waiting for the next one.
    const waitingFor = status.gameId && status.gameId !== "manual" ? status.gameName : null;
    status = { ...status, state: "idle", matchId: null, gameId: null, gameName: null, startedUnixMs: null, bookmarks: 0, waitingFor };
    emit("status-changed", { ...status });
    return { ...status };
  },
  add_bookmark: () => {
    const { matchId, startedUnixMs } = status;
    if (status.state !== "recording" || !matchId || !startedUnixMs) throw "Nothing is recording right now.";
    status = { ...status, bookmarks: status.bookmarks + 1 };
    emit("bookmark-added", { matchId, tMs: Date.now() - startedUnixMs });
    emit("status-changed", { ...status });
    return null;
  },
  get_storage: (): StorageInfo => ({
    recordingsDir: settings.recordingsDir,
    usedBytes: matches.reduce((a, m) => a + m.sizeBytes, 0) + clips.reduce((a, c) => a + c.sizeBytes, 0),
    freeBytes: 412e9,
    matchCount: matches.length,
    clipCount: clips.length,
  }),
  studio_load_frame: ({ source }: { source: FrameSource }): FrameResult => ({
    dataUrl: studioFrame(source.kind === "video" ? source.tMs : 3000, source.path),
    width: 1920,
    height: 1080,
  }),
  studio_save_detector: ({ gameId, detector: det, templateBox }: { gameId: string; detector: DetectorDef; templateBox: Region | null }) => {
    const g = games.find((x) => x.id === gameId);
    if (!g) throw "Unknown game";
    if (!/^[a-z0-9_-]+$/i.test(det.id)) throw "Detector id can only use letters, digits, - and _.";
    const templates = templateBox ? [...det.templates, `templates/${det.id}-${det.templates.length + 1}.png`] : det.templates;
    const saved = {
      ...det,
      templates,
      templateFrameHeight: templateBox ? 1080 : det.templateFrameHeight,
      ready: det.enabled && templates.length > 0,
      user: true,
      templateUrls: templates.map((t) => `${ROOT}\\..\\games\\${gameId}\\${t}`),
    };
    g.detectors = [...g.detectors.filter((d) => d.id !== det.id), saved].sort((a, b) => a.id.localeCompare(b.id));
    g.userModified = true;
    return gameInfo(gameId);
  },
  studio_video_info: ({ path }: { path: string }): VideoInfo => {
    const custom = mockStudioVideo();
    if (custom) return custom.info;
    const m = matches.find((x) => x.path === path);
    if (!m) throw "Recording not found";
    return { width: 1920, height: 1080, durationMs: m.durationMs, fps: 60, content: null };
  },
  studio_cancel: () => {
    studioCancelled = true;
    return null;
  },
  studio_delete_detector: ({ gameId, detectorId }) => {
    const g = games.find((x) => x.id === gameId);
    if (!g) throw "Unknown game";
    g.detectors = g.detectors.filter((d) => d.id !== detectorId);
    return gameInfo(gameId);
  },
  studio_save_reader: ({ gameId, reader: def, crops }: { gameId: string; reader: ReaderDef; crops: Array<{ value: string; region: Region }> }) => {
    if (!games.some((g) => g.id === gameId)) throw "Unknown game";
    if (!/^[a-z0-9_-]+$/i.test(def.id)) throw "Reader id can only use letters, digits, - and _.";
    if (!def.field.trim()) throw "Pick what this reader fills in.";
    const options = structuredClone(def.options ?? []);
    for (const c of crops) {
      const value = c.value.trim();
      if (!value) throw "Give the reference a value (e.g. the agent's name).";
      let o = options.find((x) => x.value.toLowerCase() === value.toLowerCase());
      if (!o) options.push((o = { value, templates: [] }));
      o.templates.push(`templates/${def.id}-${value.toLowerCase().replace(/\W+/g, "-")}-${o.templates.length + 1}.png`);
    }
    const saved = { ...reader({ ...def, options }, gameId), user: true };
    const list = readersByGame[gameId] ?? [];
    readersByGame[gameId] = [...list.filter((x) => x.id !== def.id), saved].sort((a, b) => a.id.localeCompare(b.id));
    return gameInfo(gameId);
  },
  studio_read_frame: ({ reader: def, source }: { reader: ReaderDef; source: FrameSource }): FrameReadResult => {
    const m = source.kind === "video" ? matches.find((x) => x.path === source.path) : undefined;
    const onEnd = !m || source.kind !== "video" || source.tMs >= m.durationMs - 60_000;
    if (def.kind === "choice") {
      const opt = def.options?.find((o) => o.templates.length);
      return { slot: def.slots ? 0 : 0, readings: opt ? [{ field: def.field, value: opt.value, confidence: 0.91 }] : [], ocrAvailable: true };
    }
    if (!onEnd) return { slot: def.highlight ? null : 0, readings: [], ocrAvailable: true };
    const slot = def.highlight ? 2 : 0;
    if (def.field === "kda" || /\?P<kills>/.test(def.pattern ?? ""))
      return {
        slot,
        readings: [
          { field: "kills", value: "24", confidence: 1 },
          { field: "deaths", value: "15", confidence: 1 },
          { field: "assists", value: "7", confidence: 1 },
        ],
        ocrAvailable: true,
      };
    const sample: Record<string, string> = { map: "Ascent", mode: "Competitive", character: "Jett", result: "win", team_score: "13", enemy_score: "9" };
    return { slot, readings: [{ field: def.field, value: sample[def.field] ?? "Ascent", confidence: 1 }], ocrAvailable: true };
  },
  studio_test_reader: ({ reader: def, matchId }: { reader: ReaderDef; matchId: string }): ReaderTestResult => {
    const m = matches.find((x) => x.id === matchId);
    if (!m) throw "Recording not found";
    const window: [number, number] = def.when === "start" ? [0, 60_000] : def.when === "any" ? [0, m.durationMs] : [Math.max(0, m.durationMs - 65_000), m.durationMs];
    const fps = def.sampleFps || 1;
    const frames = Math.round(((window[1] - window[0]) / 1000) * fps);
    const readings: Array<[number, string, string]> = [];
    const fields = def.field === "kda" ? [["kills", "24"], ["deaths", "15"], ["assists", "7"]] : [[def.field, def.kind === "choice" ? def.options?.[0]?.value ?? "Jett" : def.field === "map" ? "Ascent" : "13"]];
    const start = def.when === "end" ? m.durationMs - 58_000 : window[0];
    for (let i = 0; i < Math.min(frames, 18); i++) {
      const t = Math.round(start + i * (1000 / fps));
      for (const [f, v] of fields) readings.push([t, f, i === 3 && f === fields[0][0] ? (f === "map" ? "Breeze" : "21") : v]);
    }
    const details: Record<string, DetailValue> = {};
    for (const [f, v] of fields) details[f] = { value: v, source: "screen", confidence: 0.94 };
    return { details, readings, frames, window };
  },
  studio_delete_reader: ({ gameId, readerId }) => {
    readersByGame[gameId] = (readersByGame[gameId] ?? []).filter((r) => r.id !== readerId);
    return gameInfo(gameId);
  },
  open_path: () => null,
  set_fullscreen: () => null,
};

export async function invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  if (cmd === "studio_test_detector") {
    // Report progress like the backend, and stop early on studio_cancel.
    studioCancelled = false;
    const steps = 16;
    for (let i = 1; i <= steps; i++) {
      await delay(110);
      if (studioCancelled) {
        const a = args as { detector: DetectorDef; matchId: string };
        return structuredClone(testDetector(a.detector, a.matchId, (i - 1) / steps)) as T;
      }
      emit("studio-progress", { task: "test", progress: i / steps });
    }
    const a = args as { detector: DetectorDef; matchId: string };
    return structuredClone(testDetector(a.detector, a.matchId)) as T;
  }
  await delay(cmd.startsWith("studio_test") ? 700 : 90);
  const h = handlers[cmd];
  if (!h) throw `Unknown command ${cmd}`;
  return structuredClone(h(args)) as T;
}
