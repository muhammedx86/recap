import { useMemo, useRef, useState } from "react";
import { timecode } from "../lib/format";
import { kindStyle, sortKinds } from "../lib/kinds";
import type { Round } from "../lib/rounds";
import type { GameEvent } from "../lib/types";
import { KindDot } from "./ui";

interface Props {
  events: GameEvent[];
  rounds: Round[];
  durationMs: number;
  timeMs: number;
  inMs: number | null;
  outMs: number | null;
  hiddenKinds: Set<string>;
  onSeek: (ms: number) => void;
  onEventClick: (e: GameEvent) => void;
}

/** Kinds drawn as round dividers instead of their own lane. */
const DIVIDER_KINDS = new Set(["round_start"]);

/**
 * One lane per event kind, ticks at each event, an amber playhead across all lanes.
 * Rounds are drawn as dividers across every lane (R1, R2…) with a faint win/loss wash.
 * Click empty space to seek there; click a tick to jump a few seconds before it.
 */
export function Timeline({ events, rounds, durationMs, timeMs, inMs, outMs, hiddenKinds, onSeek, onEventClick }: Props) {
  const track = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<{ x: number; ms: number } | null>(null);
  const lanes = useMemo(() => {
    const kinds = sortKinds([...new Set(events.map((e) => e.kind))]).filter((k) => !hiddenKinds.has(k) && !DIVIDER_KINDS.has(k));
    return kinds.map((k) => ({ kind: k, events: events.filter((e) => e.kind === k) }));
  }, [events, hiddenKinds]);

  const dur = Math.max(1, durationMs);
  const frac = (ms: number) => Math.max(0, Math.min(dur, ms)) / dur;
  const pct = (ms: number) => `${frac(ms) * 100}%`;

  const msAt = (clientX: number) => {
    const r = track.current!.getBoundingClientRect();
    return Math.round(((clientX - r.left) / r.width) * dur);
  };

  // Minute ruler: pick a step that gives ~8–14 marks.
  const step = [1, 2, 5, 10, 15, 30].map((m) => m * 60_000).find((s) => dur / s <= 14) ?? 3_600_000;
  const marks: number[] = [];
  for (let t = step; t < dur; t += step) marks.push(t);

  // With many short rounds, label only some so numbers never collide.
  const labelEvery = rounds.length > 36 ? 5 : rounds.length > 18 ? 2 : 1;
  const showRounds = rounds.length > 0 && !hiddenKinds.has("round_start");

  return (
    <div className={`timeline ${showRounds ? "has-rounds" : ""}`}>
      <div className="tl-labels" aria-hidden>
        <div className="tl-ruler-label" />
        {showRounds && (
          <div className="tl-label tl-round-label">
            <KindDot kind="round_start" />
            <span>Rounds</span>
            <span className="num muted">{rounds.length}</span>
          </div>
        )}
        {lanes.map((l) => (
          <div key={l.kind} className="tl-label">
            <KindDot kind={l.kind} />
            <span className="tl-label-text">{kindStyle(l.kind).plural}</span>
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
          if ((e.target as HTMLElement).closest(".tl-tick, .tl-round-num")) return;
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
        {showRounds && <div className="tl-lane tl-lane-rounds" />}
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
        {lanes.length === 0 && !showRounds && <div className="tl-lane tl-lane-empty">No markers yet. Press M to add one at the playhead.</div>}
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
