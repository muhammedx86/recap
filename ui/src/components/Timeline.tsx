import { useEffect, useId, useMemo, useRef, useState } from "react";
import { timecode } from "../lib/format";
import { kindStyle, sortKinds } from "../lib/kinds";
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
  onSeek: (ms: number) => void;
  onEventClick: (e: GameEvent) => void;
}

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
export function Timeline({ events, rounds, durationMs, timeMs, inMs, outMs, cut, waveform, windows, hiddenKinds, onSeek, onEventClick }: Props) {
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
    const kinds = sortKinds([...new Set(events.map((e) => e.kind))]).filter(
      (k) => !hiddenKinds.has(k) && !DIVIDER_KINDS.has(k) && !(showRounds && ROUND_RESULT_KINDS.has(k)),
    );
    return kinds.map((k) => {
      const list = events.filter((e) => e.kind === k);
      return { kind: k, count: list.length, clusters: clusterEvents(list, durationMs, width) };
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
        onMouseMove={(e) => setHover({ x: e.clientX - track.current!.getBoundingClientRect().left, ms: msAt(e.clientX) })}
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
                style={{ left: pct(r.startMs), width: `${(frac(r.endMs) - frac(r.startMs)) * 100}%` }}
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
          <div key={`w${w.startMs}`} className="tl-hl-win" style={{ left: pct(w.startMs), width: `max(2px, ${(frac(w.endMs) - frac(w.startMs)) * 100}%)` }} aria-hidden />
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
        {inMs !== null && <div className="tl-range" style={{ left: pct(inMs), width: `calc(${pct(rangeEnd)} - ${pct(inMs)})` }} aria-hidden />}
        {inMs !== null && cut && !cut.exact && cut.startMs < inMs - 40 && (
          <div
            className="tl-cut-lead"
            style={{ left: pct(cut.startMs), width: `calc(${pct(inMs)} - ${pct(cut.startMs)})` }}
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
        style={{ left: `${leftPct}%`, ["--span" as string]: `${spanPct}%`, ["--kind" as string]: color }}
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
          style={{ left: `${leftPct}%`, transform: `translateX(calc(-50% + ${shift}px))` }}
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
