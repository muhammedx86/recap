import { useMemo, useRef, useState } from "react";
import { timecode } from "../lib/format";
import { kindStyle, sortKinds } from "../lib/kinds";
import type { GameEvent } from "../lib/types";

interface Props {
  events: GameEvent[];
  durationMs: number;
  timeMs: number;
  inMs: number | null;
  outMs: number | null;
  hiddenKinds: Set<string>;
  onSeek: (ms: number) => void;
  onEventClick: (e: GameEvent) => void;
}

/**
 * One lane per event kind, ticks at each event, an amber playhead across all lanes.
 * Click empty space to seek there; click a tick to jump a few seconds before it.
 */
export function Timeline({ events, durationMs, timeMs, inMs, outMs, hiddenKinds, onSeek, onEventClick }: Props) {
  const track = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<{ x: number; ms: number } | null>(null);
  const lanes = useMemo(() => {
    const kinds = sortKinds([...new Set(events.map((e) => e.kind))]).filter((k) => !hiddenKinds.has(k));
    return kinds.map((k) => ({ kind: k, events: events.filter((e) => e.kind === k) }));
  }, [events, hiddenKinds]);

  const dur = Math.max(1, durationMs);
  const pct = (ms: number) => `${(Math.max(0, Math.min(dur, ms)) / dur) * 100}%`;

  const msAt = (clientX: number) => {
    const r = track.current!.getBoundingClientRect();
    return Math.round(((clientX - r.left) / r.width) * dur);
  };

  // Minute ruler: pick a step that gives ~8–14 marks.
  const step = [1, 2, 5, 10, 15, 30].map((m) => m * 60_000).find((s) => dur / s <= 14) ?? 3_600_000;
  const marks: number[] = [];
  for (let t = step; t < dur; t += step) marks.push(t);

  return (
    <div className="timeline">
      <div className="tl-labels" aria-hidden>
        <div className="tl-ruler-label" />
        {lanes.map((l) => (
          <div key={l.kind} className="tl-label">
            <span className="kind-dot" style={{ background: kindStyle(l.kind).color }} />
            <span>{kindStyle(l.kind).plural}</span>
            <span className="num muted">{l.events.length}</span>
          </div>
        ))}
      </div>
      <div
        className="tl-track"
        ref={track}
        onMouseMove={(e) => setHover({ x: e.clientX - track.current!.getBoundingClientRect().left, ms: msAt(e.clientX) })}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          if ((e.target as HTMLElement).closest(".tl-tick")) return;
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
        {lanes.map((l) => (
          <div key={l.kind} className="tl-lane">
            {l.events.map((e, i) => (
              <button
                key={e.id ?? `${e.kind}-${e.tMs}-${i}`}
                className="tl-tick"
                style={{ left: pct(e.tMs), background: kindStyle(e.kind).color }}
                onClick={() => onEventClick(e)}
                aria-label={`${kindStyle(e.kind).label} at ${timecode(e.tMs)}`}
                title={`${kindStyle(e.kind).label}, ${timecode(e.tMs)}`}
              />
            ))}
          </div>
        ))}
        {lanes.length === 0 && <div className="tl-lane tl-lane-empty">No markers yet. Press M to add one at the playhead.</div>}
        {inMs !== null && (
          <div className="tl-range" style={{ left: pct(inMs), width: `calc(${pct(outMs ?? timeMs)} - ${pct(inMs)})` }} aria-hidden />
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
