import { useEffect, useId, useMemo, useRef, useState } from "react";
import { timecode } from "../lib/format";
import { kindStyle, sortKinds } from "../lib/kinds";
import { groupMoments, momentTitle } from "../lib/moments";
import type { Round } from "../lib/rounds";
import type { CutPoints, GameEvent } from "../lib/types";
import { Icon, KindDot } from "./ui";

interface Props {
  events: GameEvent[];
  rounds: Round[];
  durationMs: number;
  timeMs: number;
  inMs: number | null;
  outMs: number | null;
  /** Where the clip of in..out will really be cut (keyframe-snapped), when known. */
  cut?: CutPoints | null;
  /** Audio peaks 0..255 across the recording; drawn as a strip under the lanes. */
  waveform?: number[] | null;
  /** Highlight mode: only these windows play; everything else is dimmed. */
  windows?: Array<{ startMs: number; endMs: number }> | null;
  hiddenKinds: Set<string>;
  /** "track": every play on one line (default); "lanes": one lane per kind. */
  view?: TimelineView;
  /** The thin curve behind the single track. */
  curve?: TimelineCurve;
  onSeek: (ms: number) => void;
  onEventClick: (e: GameEvent) => void;
}

export type TimelineView = "track" | "lanes";
export type TimelineCurve = "activity" | "audio" | "none";

/** Summary kinds: on the single track they highlight the plays they cover instead of adding a marker. */
const SUMMARY_KINDS = new Set(["multikill", "ace"]);

/** Kinds drawn as round dividers instead of their own lane. */
const DIVIDER_KINDS = new Set(["round_start"]);
/** When rounds are drawn, results show as the round wash; no separate lanes. */
const ROUND_RESULT_KINDS = new Set(["round_win", "round_loss"]);
/** Markers closer than this on screen become one chip with a count. */
export const CLUSTER_PX = 24;

interface Cluster {
  events: GameEvent[];
  startMs: number;
  endMs: number;
}

/**
 * Group events that land within `px` of a cluster's first marker on screen (given the track width).
 * Anchoring at the first marker keeps chips about `px` wide instead of chaining across the match.
 */
export function clusterEvents(events: GameEvent[], durationMs: number, widthPx: number, px = CLUSTER_PX): Cluster[] {
  const sorted = [...events].sort((a, b) => a.tMs - b.tMs);
  const gapMs = widthPx > 0 ? (px / widthPx) * Math.max(1, durationMs) : 0;
  const out: Cluster[] = [];
  for (const e of sorted) {
    const last = out[out.length - 1];
    if (last && e.tMs - last.startMs < gapMs) {
      last.events.push(e);
      last.endMs = e.tMs;
    } else {
      out.push({ events: [e], startMs: e.tMs, endMs: e.tMs });
    }
  }
  return out;
}

/**
 * One lane per event kind, ticks at each event, an accent playhead across all lanes.
 * Rounds are drawn as dividers across every lane (R1, R2…) with a faint win/loss wash.
 * Markers that would overlap merge into a chip with a count; hovering it fans the members out.
 * Click empty space to seek there; click a tick to jump a few seconds before it.
 */
export function Timeline(props: Props) {
  return props.view === "lanes" ? <LanesTimeline {...props} /> : <TrackTimeline {...props} />;
}

function LanesTimeline({ events, rounds, durationMs, timeMs, inMs, outMs, cut, waveform, windows, hiddenKinds, onSeek, onEventClick }: Props) {
  const track = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [hover, setHover] = useState<{ x: number; ms: number } | null>(null);
  /** The cluster whose members are fanned out: "kind:index". `pinned` keeps it open after the pointer leaves. */
  const [fan, setFan] = useState<{ key: string; pinned: boolean } | null>(null);
  const closeTimer = useRef<number | undefined>(undefined);
  const showRounds = rounds.length > 0 && !hiddenKinds.has("round_start");

  useEffect(() => {
    const el = track.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  useEffect(() => () => window.clearTimeout(closeTimer.current), []);

  // A pinned fan closes on Escape or a click anywhere else.
  useEffect(() => {
    if (!fan?.pinned) return;
    const onDown = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest(".tl-cluster, .tl-fan")) setFan(null);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setFan(null);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [fan?.pinned]);

  const lanes = useMemo(() => {
    const kinds = sortKinds([...new Set(events.map((e) => e.kind))]).filter((k) => !hiddenKinds.has(k) && !DIVIDER_KINDS.has(k) && !(showRounds && ROUND_RESULT_KINDS.has(k)));
    return kinds.map((k) => {
      const list = events.filter((e) => e.kind === k);
      return {
        kind: k,
        count: list.length,
        clusters: clusterEvents(list, durationMs, width),
      };
    });
  }, [events, hiddenKinds, showRounds, durationMs, width]);

  const dur = Math.max(1, durationMs);
  const frac = (ms: number) => Math.max(0, Math.min(dur, ms)) / dur;
  const pct = (ms: number) => `${frac(ms) * 100}%`;

  const msAt = (clientX: number) => {
    const r = track.current!.getBoundingClientRect();
    return Math.round(((clientX - r.left) / r.width) * dur);
  };

  const openFan = (key: string) => {
    window.clearTimeout(closeTimer.current);
    setFan((f) => (f?.key === key ? f : { key, pinned: false }));
  };
  const leaveFan = () => {
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setFan((f) => (f && !f.pinned ? null : f)), 220);
  };

  // Minute ruler: pick a step that gives ~8–14 marks.
  const step = [1, 2, 5, 10, 15, 30].map((m) => m * 60_000).find((s) => dur / s <= 14) ?? 3_600_000;
  const marks: number[] = [];
  for (let t = step; t < dur; t += step) marks.push(t);

  // With many short rounds, label only some so numbers never collide.
  const labelEvery = rounds.length > 36 ? 5 : rounds.length > 18 ? 2 : 1;
  const hasWave = !!waveform && waveform.length > 0;

  // Highlight mode: the gaps between windows are dimmed.
  const gaps = useMemo(() => {
    if (!windows) return [];
    const out: Array<[number, number]> = [];
    let t = 0;
    for (const w of windows) {
      if (w.startMs > t) out.push([t, w.startMs]);
      t = Math.max(t, w.endMs);
    }
    if (t < dur) out.push([t, dur]);
    return out;
  }, [windows, dur]);

  const rangeEnd = outMs ?? timeMs;

  return (
    <div className={`timeline ${showRounds ? "has-rounds" : ""}`}>
      <div className="tl-labels" aria-hidden>
        <div className="tl-ruler-label" />
        {showRounds && (
          <div className="tl-label tl-round-label">
            <KindDot kind="round_start" size={12} />
            <span className="tl-label-text">Rounds</span>
            <span className="num muted">{rounds.length}</span>
          </div>
        )}
        {lanes.map((l) => (
          <div key={l.kind} className="tl-label">
            <KindDot kind={l.kind} size={12} tone="color" />
            <span className="tl-label-text">{kindStyle(l.kind).plural}</span>
            <span className="num muted">{l.count}</span>
          </div>
        ))}
        {hasWave && (
          <div className="tl-label tl-wave-label">
            <span className="tl-wave-icon">
              <Icon name="wave" size={12} />
            </span>
            <span className="tl-label-text">Audio</span>
          </div>
        )}
      </div>
      <div
        className="tl-track"
        ref={track}
        onMouseMove={(e) =>
          setHover({
            x: e.clientX - track.current!.getBoundingClientRect().left,
            ms: msAt(e.clientX),
          })
        }
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          if ((e.target as HTMLElement).closest(".tl-tick, .tl-round-num, .tl-cluster, .tl-fan")) return;
          onSeek(msAt(e.clientX));
        }}
        role="slider"
        aria-label="Timeline"
        aria-valuemin={0}
        aria-valuemax={dur}
        aria-valuenow={Math.round(timeMs)}
        aria-valuetext={timecode(timeMs)}
        tabIndex={0}
      >
        <div className="tl-ruler">
          {marks.map((m) => (
            <span key={m} className="tl-mark num" style={{ left: pct(m) }}>
              {timecode(m)}
            </span>
          ))}
        </div>
        {showRounds && (
          <div className="tl-rounds" aria-hidden>
            {rounds.map((r, i) => (
              <div
                key={`${r.n}-${r.startMs}`}
                className={`tl-round ${r.result ?? ""}`}
                style={{
                  left: pct(r.startMs),
                  width: `${(frac(r.endMs) - frac(r.startMs)) * 100}%`,
                }}
              >
                <span className="tl-round-line" />
                {(i % labelEvery === 0 || r.n === 1) && (
                  <button
                    className="tl-round-num num"
                    tabIndex={-1}
                    onClick={() => onSeek(Math.max(0, r.startMs - 2000))}
                    title={`Round ${r.n}${r.result ? (r.result === "win" ? ", won" : ", lost") : ""}, ${timecode(r.startMs)}`}
                  >
                    R{r.n}
                  </button>
                )}
              </div>
            ))}
          </div>
        )}
        {windows?.map((w) => (
          <div
            key={`w${w.startMs}`}
            className="tl-hl-win"
            style={{
              left: pct(w.startMs),
              width: `max(2px, ${(frac(w.endMs) - frac(w.startMs)) * 100}%)`,
            }}
            aria-hidden
          />
        ))}
        {gaps.map(([a, b]) => (
          <div key={a} className="tl-hl-gap" style={{ left: pct(a), width: `${(frac(b) - frac(a)) * 100}%` }} aria-hidden />
        ))}
        {showRounds && <div className="tl-lane tl-lane-rounds" />}
        {lanes.map((l) => (
          <div key={l.kind} className={`tl-lane ${fan?.key.startsWith(`${l.kind}:`) ? "fanned" : ""}`}>
            {l.clusters.map((c, i) => {
              const color = kindStyle(l.kind).color;
              if (c.events.length === 1) {
                const e = c.events[0];
                return (
                  <button
                    key={e.id ?? `${e.kind}-${e.tMs}-${i}`}
                    className="tl-tick"
                    style={{ left: pct(e.tMs), background: color }}
                    onClick={() => onEventClick(e)}
                    aria-label={`${kindStyle(e.kind).label} at ${timecode(e.tMs)}`}
                    title={`${kindStyle(e.kind).label}, ${timecode(e.tMs)}`}
                  />
                );
              }
              const key = `${l.kind}:${i}`;
              const mid = (c.startMs + c.endMs) / 2;
              const open = fan?.key === key;
              return (
                <ClusterChip
                  key={key}
                  cluster={c}
                  kind={l.kind}
                  color={color}
                  leftPct={frac(mid) * 100}
                  spanPct={(frac(c.endMs) - frac(c.startMs)) * 100}
                  centerPx={frac(mid) * width}
                  trackPx={width}
                  open={open}
                  pinned={!!(open && fan?.pinned)}
                  onEnter={() => openFan(key)}
                  onLeave={leaveFan}
                  onToggle={() => setFan((f) => (f?.key === key && f.pinned ? null : { key, pinned: true }))}
                  onPick={(e) => {
                    onEventClick(e);
                  }}
                />
              );
            })}
          </div>
        ))}
        {lanes.length === 0 && !showRounds && <div className="tl-lane tl-lane-empty">No markers yet. Press M to add one at the playhead.</div>}
        {hasWave && <Waveform peaks={waveform!} widthPx={width} playedFrac={frac(timeMs)} />}
        {inMs !== null && (
          <div
            className="tl-range"
            style={{
              left: pct(inMs),
              width: `calc(${pct(rangeEnd)} - ${pct(inMs)})`,
            }}
            aria-hidden
          />
        )}
        {inMs !== null && cut && !cut.exact && cut.startMs < inMs - 40 && (
          <div
            className="tl-cut-lead"
            style={{
              left: pct(cut.startMs),
              width: `calc(${pct(inMs)} - ${pct(cut.startMs)})`,
            }}
            title={`The clip starts at the keyframe at ${timecode(cut.startMs)}`}
            aria-hidden
          />
        )}
        {inMs !== null && cut && (
          <span className={`tl-cut-flag num ${cut.exact ? "exact" : ""}`} style={{ left: pct(cut.startMs) }} aria-hidden>
            {cut.exact ? "In" : "Keyframe"} {timecode(cut.startMs, true)}
          </span>
        )}
        <div className="tl-playhead" style={{ left: pct(timeMs) }} aria-hidden />
        {hover && (
          <div className="tl-hover num" style={{ left: hover.x }} aria-hidden>
            {timecode(hover.ms)}
          </div>
        )}
      </div>
    </div>
  );
}

function ClusterChip({
  cluster,
  kind,
  color,
  leftPct,
  spanPct,
  centerPx,
  trackPx,
  open,
  pinned,
  onEnter,
  onLeave,
  onToggle,
  onPick,
}: {
  cluster: Cluster;
  kind: string;
  color: string;
  leftPct: number;
  spanPct: number;
  centerPx: number;
  trackPx: number;
  open: boolean;
  pinned: boolean;
  onEnter: () => void;
  onLeave: () => void;
  onToggle: () => void;
  onPick: (e: GameEvent) => void;
}) {
  const n = cluster.events.length;
  const style = kindStyle(kind);
  const label = `${n} ${style.plural.toLowerCase()}, ${timecode(cluster.startMs)}–${timecode(cluster.endMs)}`;
  // Keep the fan inside the track: shift it when the chip sits near an edge.
  const fanW = Math.min(trackPx, n * 64 + 12);
  const shift = Math.max(fanW / 2 - centerPx, Math.min(0, trackPx - centerPx - fanW / 2));
  return (
    <>
      <button
        className={`tl-cluster ${open ? "open" : ""}`}
        style={{
          left: `${leftPct}%`,
          ["--span" as string]: `${spanPct}%`,
          ["--kind" as string]: color,
        }}
        onMouseEnter={onEnter}
        onMouseLeave={onLeave}
        onFocus={onEnter}
        onBlur={onLeave}
        onClick={onToggle}
        aria-expanded={open}
        aria-label={label}
        title={open ? undefined : label}
      >
        <span className="tl-cluster-count num">{n}</span>
      </button>
      {open && (
        <div
          className={`tl-fan ${pinned ? "pinned" : ""}`}
          style={{
            left: `${leftPct}%`,
            transform: `translateX(calc(-50% + ${shift}px))`,
          }}
          onMouseEnter={onEnter}
          onMouseLeave={onLeave}
          role="menu"
          aria-label={label}
        >
          {cluster.events.map((e, i) => (
            <button key={e.id ?? `${e.tMs}-${i}`} className="tl-fan-item" role="menuitem" onClick={() => onPick(e)} title={`${style.label} at ${timecode(e.tMs)}`}>
              <span className="tl-fan-dot" style={{ background: color }} />
              <span className="num">{timecode(e.tMs)}</span>
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** Mirrored peak bars, one per ~3 px; the part already played is tinted with the accent. */
function Waveform({ peaks, widthPx, playedFrac }: { peaks: number[]; widthPx: number; playedFrac: number }) {
  const clip = useId().replace(/:/g, "");
  const bars = useMemo(() => resample(peaks, Math.max(40, Math.floor(widthPx / 3))), [peaks, widthPx]);
  const d = useMemo(() => {
    let p = "";
    bars.forEach((v, i) => {
      const h = Math.max(2, (v / 255) * 92);
      p += `M${i + 0.5} ${50 - h / 2}V${50 + h / 2}`;
    });
    return p;
  }, [bars]);
  const n = bars.length;
  return (
    <div className="tl-lane tl-wave" aria-hidden>
      <svg viewBox={`0 0 ${n} 100`} preserveAspectRatio="none" width="100%" height="100%">
        <defs>
          <clipPath id={`played-${clip}`}>
            <rect x="0" y="0" width={playedFrac * n} height="100" />
          </clipPath>
        </defs>
        <path d={d} className="tl-wave-bars" strokeWidth={0.62} />
        <path d={d} className="tl-wave-bars played" strokeWidth={0.62} clipPath={`url(#played-${clip})`} />
      </svg>
    </div>
  );
}

/** Max-pool `peaks` into `n` buckets (or return them as-is when there are fewer). */
function resample(peaks: number[], n: number): number[] {
  if (peaks.length <= n) return peaks;
  const out = new Array<number>(n).fill(0);
  for (let i = 0; i < peaks.length; i++) {
    const b = Math.min(n - 1, Math.floor((i * n) / peaks.length));
    if (peaks[i] > out[b]) out[b] = peaks[i];
  }
  return out;
}

/** Shared track plumbing: width tracking, hover time, the fanned-out cluster. */
function useTrack(durationMs: number) {
  const track = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [hover, setHover] = useState<{ x: number; ms: number } | null>(null);
  const [fan, setFan] = useState<{ key: string; pinned: boolean } | null>(null);
  const closeTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    const el = track.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  useEffect(() => () => window.clearTimeout(closeTimer.current), []);
  useEffect(() => {
    if (!fan?.pinned) return;
    const onDown = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest(".tl-cluster, .tl-fan, .tl1-cluster")) setFan(null);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setFan(null);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [fan?.pinned]);
  const dur = Math.max(1, durationMs);
  const frac = (ms: number) => Math.max(0, Math.min(dur, ms)) / dur;
  const pct = (ms: number) => `${frac(ms) * 100}%`;
  const msAt = (clientX: number) => {
    const r = track.current!.getBoundingClientRect();
    return Math.max(0, Math.min(dur, Math.round(((clientX - r.left) / r.width) * dur)));
  };
  const openFan = (key: string) => {
    window.clearTimeout(closeTimer.current);
    setFan((f) => (f?.key === key ? f : { key, pinned: false }));
  };
  const leaveFan = () => {
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setFan((f) => (f && !f.pinned ? null : f)), 220);
  };
  return {
    track,
    width,
    hover,
    setHover,
    fan,
    setFan,
    openFan,
    leaveFan,
    dur,
    frac,
    pct,
    msAt,
  };
}

/** Ruler step giving roughly 6–12 labels across the track. */
function rulerMarks(dur: number, widthPx: number): number[] {
  const want = Math.max(4, Math.min(12, Math.floor(widthPx / 110)));
  const step = [0.5, 1, 2, 5, 10, 15, 30, 60].map((m) => m * 60_000).find((s) => dur / s <= want) ?? 3_600_000;
  const out: number[] = [];
  for (let t = 0; t < dur - step * 0.4; t += step) out.push(t);
  return out;
}

/** Event density over time, smoothed with a Gaussian, normalised to 0..1. */
export function activityCurve(events: GameEvent[], durationMs: number, n: number): number[] {
  const dur = Math.max(1, durationMs);
  const sigma = Math.max(6000, dur / 160);
  const out = new Array<number>(n).fill(0);
  for (const e of events) {
    const w = e.kind === "kill" ? 1 : e.kind === "death" ? 0.7 : 0.5;
    const c = (e.tMs / dur) * (n - 1);
    const r = Math.ceil(((3 * sigma) / dur) * (n - 1)) + 1;
    for (let i = Math.max(0, Math.floor(c - r)); i <= Math.min(n - 1, Math.ceil(c + r)); i++) {
      const dt = ((i - c) / (n - 1)) * dur;
      out[i] += w * Math.exp(-(dt * dt) / (2 * sigma * sigma));
    }
  }
  const max = Math.max(...out, 1e-9);
  return out.map((v) => v / max);
}

/** The audio envelope: average-pooled peaks, lightly smoothed, normalised to 0..1. */
export function audioCurve(peaks: number[], n: number): number[] {
  if (peaks.length === 0) return [];
  const pooled = new Array<number>(n).fill(0);
  const cnt = new Array<number>(n).fill(0);
  for (let i = 0; i < peaks.length; i++) {
    const b = Math.min(n - 1, Math.floor((i * n) / peaks.length));
    pooled[b] += peaks[i];
    cnt[b]++;
  }
  const avg = pooled.map((v, i) => (cnt[i] ? v / cnt[i] : 0));
  const sm = avg.map((_, i) => {
    let s = 0;
    let k = 0;
    for (let j = i - 2; j <= i + 2; j++) if (j >= 0 && j < n) ((s += avg[j]), k++);
    return s / k;
  });
  const max = Math.max(...sm, 1e-9);
  return sm.map((v) => v / max);
}

/** One full-width track: every play as an icon on the same line, a soft activity curve behind them,
 * faint round ticks above and a time ruler underneath. */
function TrackTimeline({ events, rounds, durationMs, timeMs, inMs, outMs, cut, waveform, windows, hiddenKinds, curve = "activity", onSeek, onEventClick }: Props) {
  const { track, width, hover, setHover, fan, setFan, openFan, leaveFan, dur, frac, pct, msAt } = useTrack(durationMs);
  const showRounds = rounds.length > 0 && !hiddenKinds.has("round_start");

  const shown = useMemo(
    () => events.filter((e) => !hiddenKinds.has(e.kind) && !DIVIDER_KINDS.has(e.kind) && !SUMMARY_KINDS.has(e.kind) && !(showRounds && ROUND_RESULT_KINDS.has(e.kind))),
    [events, hiddenKinds, showRounds],
  );
  // Multikills and aces (reported, or kills grouped into one moment): drawn as one highlighted moment.
  const summaries = useMemo(
    () =>
      groupMoments(events.filter((e) => !hiddenKinds.has(e.kind)))
        .map((mo) => ({ startMs: mo.startMs, endMs: mo.endMs, title: momentTitle(mo) }))
        .filter((x): x is { startMs: number; endMs: number; title: string } => !!x.title),
    [events, hiddenKinds],
  );
  const summaryAt = (a: number, b: number) => summaries.find((x) => x.startMs <= b && x.endMs >= a);

  const clusters = useMemo(() => clusterEvents(shown, durationMs, width, 34), [shown, durationMs, width]);
  // Lay the markers out so none overlap: nudge each right of its neighbour, then back from the right edge.
  const xs = useMemo(() => {
    const w = clusters.map((c) => (c.events.length === 1 ? 24 : c.events.length >= 10 ? 44 : 38));
    const x = clusters.map((c) => ((c.startMs + c.endMs) / 2 / Math.max(1, durationMs)) * width);
    for (let i = 1; i < x.length; i++) x[i] = Math.max(x[i], x[i - 1] + (w[i - 1] + w[i]) / 2 + 2);
    if (x.length) x[x.length - 1] = Math.min(x[x.length - 1], width - w[w.length - 1] / 2);
    for (let i = x.length - 2; i >= 0; i--) x[i] = Math.min(x[i], x[i + 1] - (w[i + 1] + w[i]) / 2 - 2);
    if (x.length) x[0] = Math.max(x[0], w[0] / 2);
    return x;
  }, [clusters, durationMs, width]);

  const n = Math.max(60, Math.floor(width / 4));
  const curveVals = useMemo(() => {
    if (curve === "none" || width === 0) return [];
    if (curve === "audio") return waveform && waveform.length ? audioCurve(waveform, n) : [];
    return activityCurve(shown, durationMs, n);
  }, [curve, width, waveform, n, shown, durationMs]);

  const marks = rulerMarks(dur, width);
  const labelEvery = width > 0 ? Math.max(1, Math.ceil(rounds.length / Math.max(1, width / 34))) : 1;
  const gaps = useMemo(() => {
    if (!windows) return [];
    const out: Array<[number, number]> = [];
    let t = 0;
    for (const w of windows) {
      if (w.startMs > t) out.push([t, w.startMs]);
      t = Math.max(t, w.endMs);
    }
    if (t < dur) out.push([t, dur]);
    return out;
  }, [windows, dur]);
  const rangeEnd = outMs ?? timeMs;

  return (
    <div className={`timeline single ${showRounds ? "has-rounds" : ""}`}>
      <div
        className="tl-track tl1"
        ref={track}
        onMouseMove={(e) =>
          setHover({
            x: e.clientX - track.current!.getBoundingClientRect().left,
            ms: msAt(e.clientX),
          })
        }
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          if ((e.target as HTMLElement).closest(".tl1-marker, .tl1-cluster, .tl-fan, .tl1-round-num")) return;
          onSeek(msAt(e.clientX));
        }}
        role="slider"
        aria-label="Timeline"
        aria-valuemin={0}
        aria-valuemax={dur}
        aria-valuenow={Math.round(timeMs)}
        aria-valuetext={timecode(timeMs)}
        tabIndex={0}
      >
        <div className="tl1-rounds" aria-hidden={!showRounds}>
          {showRounds &&
            rounds.map((r, i) => (
              <div
                key={`${r.n}-${r.startMs}`}
                className={`tl1-round ${r.result ?? ""}`}
                style={{
                  left: pct(r.startMs),
                  width: `${(frac(r.endMs) - frac(r.startMs)) * 100}%`,
                }}
              >
                <span className="tl1-round-line" />
                {(i % labelEvery === 0 || r.n === 1) && (
                  <button
                    className="tl1-round-num num"
                    tabIndex={-1}
                    onClick={() => onSeek(Math.max(0, r.startMs - 2000))}
                    title={`Round ${r.n}${r.result ? (r.result === "win" ? ", won" : ", lost") : ""}, ${timecode(r.startMs)}`}
                  >
                    R{r.n}
                  </button>
                )}
              </div>
            ))}
        </div>
        <div className="tl1-band">
          {curveVals.length > 1 && <Curve values={curveVals} playedFrac={frac(timeMs)} kind={curve} />}
          {windows?.map((w) => (
            <div
              key={`w${w.startMs}`}
              className="tl1-hl-win"
              style={{
                left: pct(w.startMs),
                width: `max(2px, ${(frac(w.endMs) - frac(w.startMs)) * 100}%)`,
              }}
              aria-hidden
            />
          ))}
          {gaps.map(([a, b]) => (
            <div key={a} className="tl1-hl-gap" style={{ left: pct(a), width: `${(frac(b) - frac(a)) * 100}%` }} aria-hidden />
          ))}
          {inMs !== null && (
            <div
              className="tl1-range"
              style={{
                left: pct(inMs),
                width: `calc(${pct(rangeEnd)} - ${pct(inMs)})`,
              }}
              aria-hidden
            />
          )}
          {inMs !== null && cut && !cut.exact && cut.startMs < inMs - 40 && (
            <div
              className="tl1-cut-lead"
              style={{
                left: pct(cut.startMs),
                width: `calc(${pct(inMs)} - ${pct(cut.startMs)})`,
              }}
              title={`The clip starts at the keyframe at ${timecode(cut.startMs)}`}
              aria-hidden
            />
          )}
          {shown.length === 0 && <span className="tl1-empty">No plays yet. Press M to add a marker at the playhead.</span>}
          {clusters.map((c, i) => {
            const big = summaryAt(c.startMs, c.endMs);
            if (c.events.length === 1) {
              const e = c.events[0];
              const st = kindStyle(e.kind);
              return (
                <button
                  key={e.id ?? `${e.kind}-${e.tMs}-${i}`}
                  className={`tl1-marker ${big ? "big" : ""}`}
                  style={{ left: `${xs[i]}px`, ["--kind" as string]: st.color }}
                  onClick={() => onEventClick(e)}
                  aria-label={`${st.label} at ${timecode(e.tMs)}${big ? `, ${big.title}` : ""}`}
                  title={`${big ? `${big.title} · ` : ""}${st.label}, ${timecode(e.tMs)}`}
                >
                  <KindDot kind={e.kind} tone="color" size={12} />
                </button>
              );
            }
            const key = `all:${i}`;
            const open = fan?.key === key;
            return (
              <IconCluster
                key={key}
                cluster={c}
                title={big?.title ?? null}
                leftPx={xs[i]}
                centerPx={xs[i]}
                trackPx={width}
                open={open}
                pinned={!!(open && fan?.pinned)}
                onEnter={() => openFan(key)}
                onLeave={leaveFan}
                onToggle={() => setFan((f) => (f?.key === key && f.pinned ? null : { key, pinned: true }))}
                onPick={onEventClick}
              />
            );
          })}
        </div>
        <div className="tl1-ruler" aria-hidden>
          {marks.map((m) => (
            <span key={m} className={`tl1-mark num ${m === 0 ? "first" : ""}`} style={{ left: pct(m) }}>
              {timecode(m)}
            </span>
          ))}
        </div>
        {inMs !== null && cut && (
          <span className={`tl-cut-flag num ${cut.exact ? "exact" : ""}`} style={{ left: pct(cut.startMs) }} aria-hidden>
            {cut.exact ? "In" : "Keyframe"} {timecode(cut.startMs, true)}
          </span>
        )}
        <div className="tl1-playhead" style={{ left: pct(timeMs) }} aria-hidden />
        {hover && (
          <div className="tl-hover num" style={{ left: hover.x }} aria-hidden>
            {timecode(hover.ms)}
          </div>
        )}
      </div>
    </div>
  );
}

/** Several plays close together: the leading kind's icon with a count; hover or click fans them out. */
function IconCluster({
  cluster,
  title,
  leftPx,
  centerPx,
  trackPx,
  open,
  pinned,
  onEnter,
  onLeave,
  onToggle,
  onPick,
}: {
  cluster: Cluster;
  title: string | null;
  leftPx: number;
  centerPx: number;
  trackPx: number;
  open: boolean;
  pinned: boolean;
  onEnter: () => void;
  onLeave: () => void;
  onToggle: () => void;
  onPick: (e: GameEvent) => void;
}) {
  const n = cluster.events.length;
  const counts = new Map<string, number>();
  for (const e of cluster.events) counts.set(e.kind, (counts.get(e.kind) ?? 0) + 1);
  const kinds = sortKinds([...counts.keys()]);
  const lead = [...kinds].sort((a, b) => (counts.get(b) ?? 0) - (counts.get(a) ?? 0))[0];
  const mixed = kinds.length > 1;
  const desc = kinds.map((k) => `${counts.get(k)} ${(counts.get(k) === 1 ? kindStyle(k).label : kindStyle(k).plural).toLowerCase()}`).join(", ");
  const label = `${title ? `${title}: ` : ""}${desc}, ${timecode(cluster.startMs)}–${timecode(cluster.endMs)}`;
  const fanW = Math.min(trackPx, n * 76 + 12);
  const shift = Math.max(fanW / 2 - centerPx, Math.min(0, trackPx - centerPx - fanW / 2));
  return (
    <>
      <button
        className={`tl1-cluster ${open ? "open" : ""} ${title ? "big" : ""} ${mixed ? "mixed" : ""}`}
        style={{
          left: `${leftPx}px`,
          ["--kind" as string]: kindStyle(lead).color,
        }}
        onMouseEnter={onEnter}
        onMouseLeave={onLeave}
        onFocus={onEnter}
        onBlur={onLeave}
        onClick={onToggle}
        aria-expanded={open}
        aria-label={label}
        title={open ? undefined : label}
      >
        <KindDot kind={lead} tone="color" size={12} />
        <span className="tl1-count num">{n}</span>
      </button>
      {open && (
        <div
          className={`tl-fan tl1-fan ${pinned ? "pinned" : ""}`}
          style={{
            left: `${leftPx}px`,
            transform: `translateX(calc(-50% + ${shift}px))`,
          }}
          onMouseEnter={onEnter}
          onMouseLeave={onLeave}
          role="menu"
          aria-label={label}
        >
          {title && <span className="tl1-fan-title">{title}</span>}
          {cluster.events.map((e, i) => (
            <button key={e.id ?? `${e.tMs}-${i}`} className="tl-fan-item" role="menuitem" onClick={() => onPick(e)} title={`${kindStyle(e.kind).label} at ${timecode(e.tMs)}`}>
              <KindDot kind={e.kind} tone="color" size={12} />
              <span className="num">{timecode(e.tMs)}</span>
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** A soft filled curve behind the markers; the played part is brighter. */
function Curve({ values, playedFrac, kind }: { values: number[]; playedFrac: number; kind: TimelineCurve }) {
  const clip = useId().replace(/:/g, "");
  const n = values.length;
  const { line, area } = useMemo(() => {
    const y = (v: number) => 96 - v * 80;
    let l = `M0 ${y(values[0]).toFixed(1)}`;
    for (let i = 1; i < n; i++) {
      // Midpoint quadratic smoothing keeps the curve soft at any width.
      const xm = i - 0.5;
      const ym = (y(values[i - 1]) + y(values[i])) / 2;
      l += `Q${i - 1} ${y(values[i - 1]).toFixed(1)} ${xm} ${ym.toFixed(1)}`;
    }
    l += `L${n - 1} ${y(values[n - 1]).toFixed(1)}`;
    return { line: l, area: `${l}L${n - 1} 100L0 100Z` };
  }, [values, n]);
  return (
    <svg className={`tl1-curve ${kind}`} viewBox={`0 0 ${n - 1} 100`} preserveAspectRatio="none" aria-hidden>
      <defs>
        <clipPath id={`cp-${clip}`}>
          <rect x="0" y="0" width={playedFrac * (n - 1)} height="100" />
        </clipPath>
      </defs>
      <path d={area} className="tl1-curve-area" />
      <path d={line} className="tl1-curve-line" vectorEffect="non-scaling-stroke" />
      <g clipPath={`url(#cp-${clip})`}>
        <path d={area} className="tl1-curve-area played" />
        <path d={line} className="tl1-curve-line played" vectorEffect="non-scaling-stroke" />
      </g>
    </svg>
  );
}
