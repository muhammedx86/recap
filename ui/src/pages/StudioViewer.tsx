// Detector Studio's frame viewer: plays the recording directly (no per-frame round trips),
// crops away black bars so boxes are fractions of the game picture, lets you draw, move and
// resize a box with a magnifier, and shows a timeline with match events and test results.

import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api, fileSrc, isTauri, on } from "../lib/api";
import { mockStudioVideo, studioFrame } from "../lib/mock";
import { timecode } from "../lib/format";
import type { FrameResult, FrameSource, Region, VideoInfo } from "../lib/types";
import { Icon } from "../components/ui";

export type Media = { kind: "video"; path: string; durationMs: number } | { kind: "image"; path: string };

export interface Marker {
  tMs: number;
  color: string;
  title: string;
}

export interface Overlay {
  region: Region;
  className: string;
  label?: string;
}

export interface ViewerHandle {
  /** Pause and show this moment. */
  seek: (tMs: number) => void;
  /** The exact frame on screen, for the backend to cut at full resolution. */
  source: () => FrameSource | null;
  /** Average colour of a 5×5 patch at (x, y) fractions of the game picture, as #rrggbb. */
  sampleColour: (p: { x: number; y: number }) => Promise<string>;
}

export interface ViewerProps {
  media: Media | null;
  /** The box being edited (solid outline) and the dashed search area. */
  box: Region | null;
  region?: Region | null;
  overlays?: Overlay[];
  /** The box is a new, unsaved one (filled). */
  drafting?: boolean;
  /** Drawing is allowed: a drag draws a box; dragging the box moves it, its corners resize it. */
  onBox?: (r: Region) => void;
  /** When set, a click without dragging reports the point (colour picking). */
  onPoint?: (p: { x: number; y: number }) => void;
  onEscape?: () => void;
  /** Called (throttled) when the shown moment changes. */
  onTime?: (tMs: number) => void;
  onInfo?: (info: { width: number; height: number } | null) => void;
  markers?: Marker[];
  /** Test results on the timeline. */
  detections?: number[];
  samples?: Array<[number, number]>;
  threshold?: number;
  /** Part of the recording scanned so far while a test runs (0..1). */
  progress?: number | null;
  /** Times [ and ] jump between, with what to call them. */
  jumps?: { times: number[]; noun: string };
  emptyText?: string;
}

const infoCache = new Map<string, Promise<VideoInfo>>();
function videoInfo(path: string): Promise<VideoInfo> {
  let p = infoCache.get(path);
  if (!p) {
    p = api.studioVideoInfo(path);
    infoCache.set(path, p);
    p.catch(() => infoCache.delete(path));
  }
  return p;
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
type Pt = { x: number; y: number };
type DragMode = "new" | "move" | "nw" | "ne" | "sw" | "se";
const rectOf = (a: Pt, b: Pt): Region => ({ x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), w: Math.abs(a.x - b.x), h: Math.abs(a.y - b.y) });
const pos = (r: Region) => ({ left: `${r.x * 100}%`, top: `${r.y * 100}%`, width: `${r.w * 100}%`, height: `${r.h * 100}%` });
const LOUPE = 136;
const ZOOM = 4;

function isTyping(t: EventTarget | null) {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

export const StudioViewer = forwardRef<ViewerHandle, ViewerProps>(function StudioViewer(props, ref) {
  const { media, box, region, overlays = [], drafting, onBox, onPoint, onEscape, onTime, onInfo, markers = [], detections = [], samples, threshold, progress, jumps } = props;
  const isVideo = media?.kind === "video";
  // In the browser preview there is no file to play: draw the mock frames instead (unless a sample video is configured).
  const mockVideo = !isTauri ? mockStudioVideo() : null;
  const simulate = isVideo && !isTauri && !mockVideo;
  const videoUrl = isVideo ? (mockVideo ? mockVideo.url : fileSrc(media.path)) : undefined;
  const durationMs = isVideo ? media.durationMs : 0;

  const [info, setInfo] = useState<VideoInfo | null>(null);
  const [infoError, setInfoError] = useState<string | null>(null);
  const [image, setImage] = useState<FrameResult | null>(null);
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
  const [t, setT] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [live, setLive] = useState<Region | null>(null);
  const [loupe, setLoupe] = useState<Pt | null>(null);
  const [cursor, setCursor] = useState<string>("crosshair");

  const videoRef = useRef<HTMLVideoElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);
  const loupeRef = useRef<HTMLCanvasElement>(null);
  const pending = useRef<number | null>(null);
  const drag = useRef<{ mode: DragMode; start: Pt; orig: Region | null } | null>(null);
  const tRef = useRef(0);
  tRef.current = t;
  /** Where to show the recording once it has loaded. */
  const startAt = useRef<number | null>(null);

  // ---- load the media ----
  useEffect(() => {
    setInfo(null);
    setInfoError(null);
    setImage(null);
    setNatural(null);
    setPlaying(false);
    setLive(null);
    pending.current = null;
    if (!media) return;
    let stale = false;
    if (media.kind === "video") {
      videoInfo(media.path).then(
        (i) => !stale && setInfo(i),
        (e) => !stale && setInfoError(String(e)),
      );
    } else {
      api.studioLoadFrame({ kind: "image", path: media.path }).then(
        (f) => !stale && setImage(f),
        (e) => !stale && setInfoError(String(e)),
      );
    }
    return () => {
      stale = true;
    };
  }, [media?.kind, media?.path]);

  // Size of the full picture (video pixels) and the game picture inside it.
  const full = isVideo ? (info ? { w: info.width, h: info.height } : natural) : image ? { w: image.width, h: image.height } : null;
  const content = useMemo(() => {
    if (!full) return null;
    const c = isVideo ? info?.content : null;
    return c ?? { x: 0, y: 0, w: full.w, h: full.h };
  }, [full?.w, full?.h, info, isVideo]);
  // Without the backend's info (it failed), fall back to the whole video picture.
  const ready = !!content && (!isVideo || !!info || (!!infoError && !!natural));

  useEffect(() => {
    onInfo?.(ready && content ? { width: content.w, height: content.h } : null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, content?.w, content?.h]);

  // ---- time ----
  const fps = info?.fps && info.fps > 1 ? info.fps : 60;
  // Tell the page about the shown moment at most ~12 times a second (it re-renders a lot).
  const timeNotify = useRef<{ timer: number | null; latest: number }>({ timer: null, latest: 0 });
  const onTimeRef = useRef(onTime);
  onTimeRef.current = onTime;
  const notify = useCallback((ms: number) => {
    const n = timeNotify.current;
    n.latest = ms;
    if (n.timer !== null) return;
    n.timer = window.setTimeout(() => {
      n.timer = null;
      onTimeRef.current?.(n.latest);
    }, 80);
  }, []);

  const seek = useCallback(
    (ms: number) => {
      if (!isVideo) return;
      const target = clamp(ms, 0, Math.max(0, durationMs - 1));
      startAt.current = target;
      setT(target);
      notify(target);
      const v = videoRef.current;
      if (simulate || !v) return;
      // While the decoder is busy, keep only the latest request (drags stay smooth).
      if (v.seeking) pending.current = target;
      else v.currentTime = target / 1000;
    },
    [isVideo, durationMs, simulate, notify],
  );

  const pause = useCallback(() => {
    videoRef.current?.pause();
    setPlaying(false);
  }, []);

  // Stop playing when the window goes to the tray or is minimized.
  useEffect(() => on("window-hidden", pause), [pause]);

  const togglePlay = useCallback(() => {
    if (!isVideo) return;
    if (simulate) return setPlaying((p) => !p);
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) v.play().catch(() => {});
    else v.pause();
  }, [isVideo, simulate]);

  const frameIndex = (ms: number) => Math.floor((ms / 1000) * fps + 1e-3);
  const stepFrames = useCallback(
    (n: number) => {
      pause();
      // Land in the middle of the frame so the browser and ffmpeg agree on which one it is.
      seek(((frameIndex(tRef.current) + n + 0.5) / fps) * 1000);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [fps, pause, seek],
  );

  // Simulated playback in the browser preview.
  useEffect(() => {
    if (!simulate || !playing) return;
    const id = window.setInterval(() => {
      const next = tRef.current + 100;
      if (next >= durationMs) setPlaying(false);
      seek(next);
    }, 100);
    return () => window.clearInterval(id);
  }, [simulate, playing, durationMs, seek]);

  // Smooth playhead while a real video plays.
  useEffect(() => {
    if (simulate || !playing) return;
    let raf = 0;
    const tick = () => {
      const v = videoRef.current;
      if (v) {
        const ms = v.currentTime * 1000;
        setT(ms);
        notify(ms);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [simulate, playing, notify]);

  // Start each recording paused a little way in (the first frames are often a black load screen).
  useEffect(() => {
    if (!isVideo) return;
    const start = Math.min(30_000, Math.max(0, durationMs / 4));
    startAt.current = start;
    setT(start);
    notify(start);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [media?.kind, media?.path]);

  const currentSource = useCallback((): FrameSource | null => {
    if (!media) return null;
    if (media.kind === "image") return { kind: "image", path: media.path };
    const ms = simulate ? tRef.current : (videoRef.current?.currentTime ?? tRef.current / 1000) * 1000;
    // The frame shown starts at frame n; ffmpeg seeking to that start returns the same frame.
    const tMs = simulate ? Math.round(ms) : Math.floor((frameIndex(ms) * 1000) / fps);
    return { kind: "video", path: media.path, tMs };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [media, simulate, fps]);

  const mediaEl = (): CanvasImageSource | null => (isVideo && !simulate ? videoRef.current : imgRef.current);

  const sampleColour = useCallback(
    async (p: Pt): Promise<string> => {
      const avg = (data: Uint8ClampedArray) => {
        const sum = [0, 0, 0];
        for (let i = 0; i < data.length; i += 4) for (let k = 0; k < 3; k++) sum[k] += data[i + k];
        const n = data.length / 4;
        return "#" + sum.map((v) => Math.round(v / n).toString(16).padStart(2, "0")).join("");
      };
      const c = document.createElement("canvas");
      c.width = c.height = 5;
      const ctx = c.getContext("2d", { willReadFrequently: true })!;
      const el = mediaEl();
      if (el && content) {
        try {
          ctx.drawImage(el, content.x + p.x * content.w - 2, content.y + p.y * content.h - 2, 5, 5, 0, 0, 5, 5);
          return avg(ctx.getImageData(0, 0, 5, 5).data);
        } catch {
          // The video can't be read back in this webview: ask the backend for the frame.
        }
      }
      const src = currentSource();
      if (!src) throw new Error("No frame");
      const f = await api.studioLoadFrame(src);
      const img = new Image();
      img.src = f.dataUrl;
      await img.decode();
      ctx.drawImage(img, p.x * f.width - 2, p.y * f.height - 2, 5, 5, 0, 0, 5, 5);
      return avg(ctx.getImageData(0, 0, 5, 5).data);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [content, currentSource, isVideo, simulate],
  );

  useImperativeHandle(ref, () => ({ seek: (ms) => (pause(), seek(ms)), source: currentSource, sampleColour }), [pause, seek, currentSource, sampleColour]);

  // ---- jumps ----
  const jumpTimes = jumps?.times ?? [];
  const jump = useCallback(
    (dir: 1 | -1) => {
      const now = tRef.current;
      const list = dir > 0 ? jumpTimes.filter((x) => x > now + 250) : jumpTimes.filter((x) => x < now - 250);
      const target = dir > 0 ? list[0] : list[list.length - 1];
      if (target !== undefined) {
        pause();
        seek(target);
      }
    },
    [jumpTimes, pause, seek],
  );
  const jumpIndex = jumpTimes.findIndex((x) => Math.abs(x - t) <= 250);

  // ---- keyboard ----
  useEffect(() => {
    if (!isVideo) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || isTyping(e.target)) return;
      const k = e.key;
      let handled = true;
      if (k === " " || k === "k" || k === "K") togglePlay();
      else if (k === ",") stepFrames(-1);
      else if (k === ".") stepFrames(1);
      else if (k === "ArrowLeft") (pause(), seek(tRef.current - (e.shiftKey ? 5000 : 1000)));
      else if (k === "ArrowRight") (pause(), seek(tRef.current + (e.shiftKey ? 5000 : 1000)));
      else if (k === "j" || k === "J") (pause(), seek(tRef.current - 5000));
      else if (k === "l" || k === "L") (pause(), seek(tRef.current + 5000));
      else if (k === "[") jump(-1);
      else if (k === "]") jump(1);
      else if (k === "Escape" && onEscape) onEscape();
      else handled = false;
      if (handled) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isVideo, togglePlay, stepFrames, pause, seek, jump, onEscape]);

  // ---- drawing ----
  const frac = (e: { clientX: number; clientY: number }): Pt => {
    const r = frameRef.current!.getBoundingClientRect();
    return { x: clamp((e.clientX - r.left) / r.width, 0, 1), y: clamp((e.clientY - r.top) / r.height, 0, 1) };
  };
  const hitTest = (p: Pt): DragMode | null => {
    if (!box || !onBox || !frameRef.current) return null;
    const r = frameRef.current.getBoundingClientRect();
    const near = (a: number, b: number, size: number) => Math.abs(a - b) * size <= 7;
    const corners: Array<[DragMode, number, number]> = [
      ["nw", box.x, box.y],
      ["ne", box.x + box.w, box.y],
      ["sw", box.x, box.y + box.h],
      ["se", box.x + box.w, box.y + box.h],
    ];
    for (const [m, cx, cy] of corners) if (near(p.x, cx, r.width) && near(p.y, cy, r.height)) return m;
    if (p.x > box.x && p.x < box.x + box.w && p.y > box.y && p.y < box.y + box.h) return "move";
    return null;
  };
  const dragTo = (p: Pt): Region | null => {
    const d = drag.current;
    if (!d) return null;
    if (d.mode === "new" || !d.orig) return rectOf(d.start, p);
    const o = d.orig;
    if (d.mode === "move") {
      const x = clamp(o.x + p.x - d.start.x, 0, 1 - o.w);
      const y = clamp(o.y + p.y - d.start.y, 0, 1 - o.h);
      return { ...o, x, y };
    }
    const anchor = { x: d.mode.includes("w") ? o.x + o.w : o.x, y: d.mode.includes("n") ? o.y + o.h : o.y };
    return rectOf(anchor, p);
  };
  const canDraw = ready && (!!onBox || !!onPoint);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!canDraw || e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    pause();
    const p = frac(e);
    const mode = onPoint ? null : hitTest(p);
    drag.current = { mode: mode ?? "new", start: p, orig: box };
    setLive(null);
    if (!onPoint) setLoupe(p);
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!canDraw) return;
    const p = frac(e);
    if (!drag.current) {
      const m = onPoint ? null : hitTest(p);
      setCursor(onPoint ? "copy" : m === "move" ? "move" : m === "nw" || m === "se" ? "nwse-resize" : m ? "nesw-resize" : "crosshair");
      return;
    }
    setLive(dragTo(p));
    if (!onPoint) setLoupe(p);
  };
  const onPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    const p = frac(e);
    const r = dragTo(p);
    drag.current = null;
    setLive(null);
    setLoupe(null);
    if (!r) return;
    if (d.mode === "new") {
      if (onPoint && r.w < 0.004 && r.h < 0.004) onPoint(d.start);
      else if (onBox && r.w > 0.004 && r.h > 0.004) onBox(r);
    } else if (onBox && (r.x !== d.orig?.x || r.y !== d.orig?.y || r.w !== d.orig?.w || r.h !== d.orig?.h) && r.w > 0.004 && r.h > 0.004) {
      onBox(r);
    }
  };

  // Magnifier: redraw from the picture itself while drawing.
  useEffect(() => {
    const cv = loupeRef.current;
    const el = mediaEl();
    if (!loupe || !cv || !el || !content || !frameRef.current) return;
    const dpr = window.devicePixelRatio || 1;
    cv.width = cv.height = Math.round(LOUPE * dpr);
    const ctx = cv.getContext("2d")!;
    const shown = frameRef.current.getBoundingClientRect();
    const span = (LOUPE / ZOOM) * (content.w / shown.width); // source pixels across the loupe
    const sx = content.x + loupe.x * content.w - span / 2;
    const sy = content.y + loupe.y * content.h - span / 2;
    ctx.imageSmoothingEnabled = false;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, cv.width, cv.height);
    try {
      ctx.drawImage(el, sx, sy, span, span, 0, 0, cv.width, cv.height);
    } catch {
      /* not decodable yet */
    }
    const k = cv.width / span;
    const r = live ?? box;
    if (r) {
      ctx.strokeStyle = "#f2a93b";
      ctx.lineWidth = 1.5 * dpr;
      ctx.strokeRect((content.x + r.x * content.w - sx) * k, (content.y + r.y * content.h - sy) * k, r.w * content.w * k, r.h * content.h * k);
    }
    ctx.strokeStyle = "rgba(255,255,255,0.55)";
    ctx.lineWidth = 1 * dpr;
    const m = cv.width / 2;
    ctx.beginPath();
    ctx.moveTo(m, m - 10 * dpr);
    ctx.lineTo(m, m + 10 * dpr);
    ctx.moveTo(m - 10 * dpr, m);
    ctx.lineTo(m + 10 * dpr, m);
    ctx.stroke();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loupe, live, box, content]);

  // ---- render ----
  if (!media)
    return (
      <div className="viewer">
        <div className="frame frame-empty">
          <p>{props.emptyText ?? "Pick a recording or a screenshot to show a frame here."}</p>
        </div>
      </div>
    );

  const aspect = content ? `${content.w} / ${content.h}` : "16 / 9";
  // Position the full picture so only the game picture shows inside the frame.
  const mediaStyle =
    full && content
      ? {
          width: `${(full.w / content.w) * 100}%`,
          height: `${(full.h / content.h) * 100}%`,
          left: `${(-content.x / content.w) * 100}%`,
          top: `${(-content.y / content.h) * 100}%`,
        }
      : { width: "100%", height: "100%", left: 0, top: 0 };
  const shown = live ?? box;
  // The magnifier sits in the corner away from the pointer, so it never hides what you are boxing.
  const fw = frameRef.current?.clientWidth ?? 0;
  const fh = frameRef.current?.clientHeight ?? 0;
  const loupeStyle = loupe
    ? { left: loupe.x > 0.5 ? 10 : fw - LOUPE - 10, top: loupe.y > 0.5 ? 10 : Math.max(10, fh - LOUPE - 10), width: LOUPE, height: LOUPE }
    : undefined;

  return (
    <div className="viewer">
      <div
        ref={frameRef}
        className={`frame ${!ready ? "is-loading" : ""}`}
        style={{ aspectRatio: aspect, cursor: canDraw ? cursor : "default" }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => ((drag.current = null), setLive(null), setLoupe(null))}
        onDoubleClick={() => isVideo && !onBox && togglePlay()}
        role="img"
        aria-label={onPoint ? "Frame. Click a colour." : "Frame. Drag to draw a box around the element."}
      >
        <div className="frame-media" style={mediaStyle}>
          {isVideo && !simulate ? (
            <video
              ref={videoRef}
              src={videoUrl}
              muted
              playsInline
              preload="auto"
              onLoadedMetadata={(e) => {
                const v = e.currentTarget;
                setNatural({ w: v.videoWidth, h: v.videoHeight });
                if (startAt.current !== null) v.currentTime = startAt.current / 1000;
              }}
              onSeeked={(e) => {
                const v = e.currentTarget;
                if (pending.current !== null) {
                  const next = pending.current;
                  pending.current = null;
                  v.currentTime = next / 1000;
                  return;
                }
                if (!playing) {
                  setT(v.currentTime * 1000);
                  notify(v.currentTime * 1000);
                }
              }}
              onPlay={() => setPlaying(true)}
              onPause={(e) => {
                setPlaying(false);
                setT(e.currentTarget.currentTime * 1000);
                notify(e.currentTarget.currentTime * 1000);
              }}
              onError={() => setInfoError("This recording can't be played here.")}
            />
          ) : isVideo ? (
            <img ref={imgRef} src={studioFrame(Math.round(t), media.path)} alt="" draggable={false} />
          ) : image ? (
            <img ref={imgRef} src={image.dataUrl} alt="" draggable={false} />
          ) : null}
        </div>
        {ready && !live && region && <div className="frame-region" style={pos(region)} aria-hidden />}
        {ready &&
          !live &&
          overlays.map((o, i) => (
            <div key={i} className={o.className} style={pos(o.region)} aria-hidden>
              {o.label && <span className="frame-tag">{o.label}</span>}
            </div>
          ))}
        {ready && shown && <div className={`frame-box ${drafting || live ? "is-new" : ""}`} style={pos(shown)} aria-hidden />}
        {loupe && (
          <canvas
            ref={loupeRef}
            className="loupe"
            style={loupeStyle}
            aria-hidden
          />
        )}
        {!ready && <span className="frame-chip">{infoError ?? (isVideo ? "Finding the game picture…" : "Loading…")}</span>}
        {ready && content && (
          <span
            className="frame-res num"
            title={info?.content ? "Black bars are left out: boxes are measured on the game picture." : infoError ? `Black bars were not checked: ${infoError}` : undefined}
          >
            {content.w}×{content.h}
            {info?.content ? " · bars cropped" : infoError ? " · bars not checked" : ""}
          </span>
        )}
      </div>

      {isVideo && (
        <>
          <div className="transport">
            <button className="icon-btn" onClick={togglePlay} aria-label={playing ? "Pause (K)" : "Play (K)"} title={playing ? "Pause (K or Space)" : "Play (K or Space)"}>
              <Icon name={playing ? "pause" : "play"} size={15} />
            </button>
            <button className="icon-btn" onClick={() => stepFrames(-1)} aria-label="Previous frame (,)" title="Previous frame ( , )">
              <Icon name="frame-back" size={15} />
            </button>
            <button className="icon-btn" onClick={() => stepFrames(1)} aria-label="Next frame (.)" title="Next frame ( . )">
              <Icon name="frame-fwd" size={15} />
            </button>
            <span className="transport-time num">
              {timecode(t, true)} <span className="muted">/ {timecode(durationMs)}</span>
            </span>
            <span className="transport-spacer" />
            {jumps && jumpTimes.length > 0 && (
              <span className="transport-jumps">
                <button className="icon-btn" onClick={() => jump(-1)} aria-label={`Previous ${jumps.noun} ([)`} title={`Previous ${jumps.noun} ( [ )`}>
                  <Icon name="skip-back" size={15} />
                </button>
                <span className="num small">
                  {jumpIndex >= 0 ? `${jumpIndex + 1} of ${jumpTimes.length}` : `${jumpTimes.length} ${jumpTimes.length === 1 ? jumps.noun : jumps.noun + "s"}`}
                </span>
                <button className="icon-btn" onClick={() => jump(1)} aria-label={`Next ${jumps.noun} (])`} title={`Next ${jumps.noun} ( ] )`}>
                  <Icon name="skip-fwd" size={15} />
                </button>
              </span>
            )}
            <span className="keys muted" title={"Space or K  play / pause\n,  .  one frame back / forward\n←  →  one second (Shift: five)\nJ  L  five seconds\n[  ]  previous / next marker\nEsc  clear the new box"}>
              Keys
            </span>
          </div>
          <Timeline
            durationMs={durationMs}
            t={t}
            onSeek={(ms) => (pause(), seek(ms))}
            markers={markers}
            detections={detections}
            samples={samples}
            threshold={threshold}
            progress={progress ?? null}
          />
        </>
      )}
    </div>
  );
});

// ---------- timeline ----------

const TW = 1000;

function Timeline({
  durationMs,
  t,
  onSeek,
  markers,
  detections,
  samples,
  threshold,
  progress,
}: {
  durationMs: number;
  t: number;
  onSeek: (tMs: number) => void;
  markers: Marker[];
  detections: number[];
  samples?: Array<[number, number]>;
  threshold?: number;
  progress: number | null;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const [hover, setHover] = useState<number | null>(null);
  const dur = Math.max(1, durationMs);
  const hasCurve = !!samples?.length;
  const H = 40;
  const y = (s: number) => 3 + (1 - clamp(s, 0, 1)) * (H - 6);

  const path = useMemo(() => {
    if (!samples?.length) return "";
    // At most ~TW points, keeping the highest score in each column so short spikes survive.
    const cols = new Map<number, number>();
    for (const [ts, s] of samples) {
      const c = Math.round((ts / dur) * TW);
      cols.set(c, Math.max(cols.get(c) ?? -1, s));
    }
    return [...cols.entries()]
      .sort((a, b) => a[0] - b[0])
      .map(([c, s], i) => `${i ? "L" : "M"}${c},${y(s).toFixed(1)}`)
      .join("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [samples, dur]);

  const tAt = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect();
    return clamp(((clientX - r.left) / r.width) * dur, 0, dur);
  };
  const pct = (ms: number) => `${(ms / dur) * 100}%`;

  return (
    <div
      ref={ref}
      className={`timeline ${hasCurve ? "has-curve" : ""}`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        dragging.current = true;
        onSeek(tAt(e.clientX));
      }}
      onPointerMove={(e) => {
        const ms = tAt(e.clientX);
        setHover(ms);
        if (dragging.current) onSeek(ms);
      }}
      onPointerUp={() => (dragging.current = false)}
      onPointerCancel={() => (dragging.current = false)}
      onPointerLeave={() => setHover(null)}
      role="slider"
      aria-label="Time in the recording"
      aria-valuemin={0}
      aria-valuemax={Math.round(dur / 1000)}
      aria-valuenow={Math.round(t / 1000)}
      aria-valuetext={timecode(t)}
    >
      {hasCurve && (
        <svg className="tl-curve" viewBox={`0 0 ${TW} ${H}`} preserveAspectRatio="none" aria-hidden>
          {threshold !== undefined && (
            <>
              <rect x={0} y={0} width={TW} height={y(threshold)} className="tl-zone" />
              <line x1={0} x2={TW} y1={y(threshold)} y2={y(threshold)} className="tl-threshold" vectorEffect="non-scaling-stroke" />
            </>
          )}
          <path d={path} className="tl-line" vectorEffect="non-scaling-stroke" />
        </svg>
      )}
      <div className="tl-track" aria-hidden>
        {progress !== null && <div className="tl-progress" style={{ width: pct(progress * dur) }} />}
        <div className="tl-played" style={{ width: pct(t) }} />
      </div>
      {detections.map((d) => (
        <span key={d} className="tl-hit" style={{ left: pct(d) }} aria-hidden />
      ))}
      <div className="tl-markers" aria-hidden>
        {markers.map((m, i) => (
          <span key={i} className="tl-marker" style={{ left: pct(m.tMs), background: m.color }} title={`${m.title} · ${timecode(m.tMs)}`} />
        ))}
      </div>
      <span className="tl-head" style={{ left: pct(t) }} aria-hidden />
      {hover !== null && (
        <span className="tl-hover num" style={{ left: pct(hover) }} aria-hidden>
          {timecode(hover)}
        </span>
      )}
    </div>
  );
}
