// In-memory backend used when the UI runs outside Tauri (browser dev, screenshots).
// It implements every command in docs/API.md with realistic data so each screen can be exercised.

import type {
  AudioInput,
  ClipRecord,
  DetectorDef,
  EventMap,
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

let games: GameInfo[] = [
  {
    id: "valorant",
    name: "VALORANT",
    processes: ["VALORANT-Win64-Shipping.exe"],
    capture: "game",
    api: null,
    preferApi: true,
    autoClipEvents: ["kill"],
    color: GAME_COLORS.valorant,
    enabled: true,
    userModified: true,
    hasPostMatch: false,
    detectors: [
      detector({ id: "kill-banner", label: "Kill banner", event: "kill" }, "valorant", true),
      detector({ id: "death-recap", label: "Death recap panel", event: "death", templateBox: { x: 0.36, y: 0.08, w: 0.28, h: 0.06 } }, "valorant", false),
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
  notifyOnBookmark: true,
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
}

const seeds: MatchSeed[] = [
  { id: "m-live", gameId: "valorant", start: now - 23 * MIN, durMin: 23, status: "recording", kills: 0, deaths: 0, assists: 0 },
  { id: "m-val-1", gameId: "valorant", start: startOfToday + 13.5 * 60 * MIN, durMin: 41.2, status: "ready", kills: 24, deaths: 15, assists: 7, extra: [["bookmark", 2]] },
  { id: "m-lol-1", gameId: "league-of-legends", start: startOfToday + 11 * 60 * MIN, durMin: 31.4, status: "processing", kills: 9, deaths: 4, assists: 13, extra: [["objective", 5], ["multikill", 1]] },
  { id: "m-cs-1", gameId: "cs2", start: startOfToday - DAY + 21 * 60 * MIN, durMin: 47.8, status: "ready", kills: 27, deaths: 18, assists: 4, extra: [["round_win", 13], ["round_loss", 11], ["mvp", 4], ["bomb_plant", 3], ["ace", 1]] },
  { id: "m-mr-1", gameId: "marvel-rivals", start: startOfToday - DAY + 19 * 60 * MIN, durMin: 18.6, status: "ready", kills: 0, deaths: 0, assists: 0, extra: [["bookmark", 3]] },
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
  const src = (kind: string): GameEvent["source"] =>
    kind === "bookmark"
      ? { type: "manual" }
      : isApi
        ? { type: "api", name: seed.gameId === "cs2" ? "cs2-gsi" : "league-live-client" }
        : { type: "vision", detector: kind === "death" ? "death-recap" : "kill-banner" };
  const add = (kind: string, n: number) => {
    for (let i = 0; i < n; i++) {
      const tMs = Math.round(40_000 + r() * (dur - 60_000));
      const e: GameEvent = { kind, tMs, source: src(kind) };
      if (!isApi && kind !== "bookmark") e.confidence = 0.8 + r() * 0.19;
      if (kind === "multikill") e.meta = { count: 2 + Math.floor(r() * 3) };
      evs.push(e);
    }
  };
  add("kill", seed.kills);
  add("death", seed.deaths);
  add("assist", seed.assists);
  for (const [k, n] of seed.extra ?? []) add(k, n);
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

/** A 1920x1080 HUD-like frame with a kill banner, for Detector Studio. */
function studioFrame(t: number, path: string): string {
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
  return { ...g, enabled: settings.gamesEnabled[id] ?? true };
}

function matchDetail(id: string): MatchDetail {
  const m = matches.find((x) => x.id === id);
  if (!m) throw `Recording not found`;
  const evs = eventsByMatch.get(id) ?? [];
  m.eventCounts = countEvents(evs);
  return { match: { ...m }, events: [...evs], clips: clips.filter((c) => c.matchId === id).sort((a, b) => b.createdUnixMs - a.createdUnixMs) };
}

function testDetector(det: DetectorDef, matchId: string): TestResult {
  const m = matches.find((x) => x.id === matchId);
  if (!m) throw "Recording not found";
  const kills = (eventsByMatch.get(matchId) ?? []).filter((e) => e.kind === det.event);
  const step = Math.max(1000 / det.sampleFps, m.durationMs / 1800);
  const r = rng(Math.round(det.threshold * 1000) + matchId.length);
  const samples: Array<[number, number]> = [];
  for (let t = 0; t < m.durationMs; t += step) {
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
  return { events, samples, durationMs: m.durationMs };
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
    const info: GameInfo = { ...game, enabled: true, userModified: true, detectors: existing?.detectors ?? [], hasPostMatch: false };
    games = existing ? games.map((g) => (g.id === game.id ? info : g)) : [...games, info];
    return gameInfo(game.id);
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
    status = { ...status, state: "idle", matchId: null, gameId: null, gameName: null, startedUnixMs: null, bookmarks: 0 };
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
  studio_test_detector: ({ detector: det, matchId }: { detector: DetectorDef; matchId: string }) => testDetector(det, matchId),
  studio_delete_detector: ({ gameId, detectorId }) => {
    const g = games.find((x) => x.id === gameId);
    if (!g) throw "Unknown game";
    g.detectors = g.detectors.filter((d) => d.id !== detectorId);
    return gameInfo(gameId);
  },
  open_path: () => null,
};

export async function invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  await delay(cmd.startsWith("studio_test") ? 700 : 90);
  const h = handlers[cmd];
  if (!h) throw `Unknown command ${cmd}`;
  return structuredClone(h(args)) as T;
}
