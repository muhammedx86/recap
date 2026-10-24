import { useMemo, useRef, useState } from "react";
import { timecode } from "../lib/format";

interface Props {
  samples: Array<[number, number]>;
  durationMs: number;
  threshold: number;
  hits: number[];
  onPick: (tMs: number) => void;
}

const W = 1000;
const H = 140;
const PAD_T = 10;
const PAD_B = 6;

/** Match score over time with the threshold line and fired events. Click anywhere to preview that moment. */
export function ScoreChart({ samples, durationMs, threshold, hits, onPick }: Props) {
  const svg = useRef<SVGSVGElement>(null);
  const [hover, setHover] = useState<number | null>(null);
  const dur = Math.max(1, durationMs);
  const x = (t: number) => (t / dur) * W;
  const y = (s: number) => PAD_T + (1 - Math.max(0, Math.min(1, s))) * (H - PAD_T - PAD_B);

  const path = useMemo(() => {
    if (!samples.length) return "";
    // Downsample to at most ~W points (keeping the max in each bucket so spikes survive).
    const buckets = new Map<number, number>();
    for (const [t, s] of samples) {
      const b = Math.round(x(t));
      buckets.set(b, Math.max(buckets.get(b) ?? -1, s));
    }
    return [...buckets.entries()]
      .sort((a, b) => a[0] - b[0])
      .map(([bx, s], i) => `${i ? "L" : "M"}${bx},${y(s).toFixed(1)}`)
      .join("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [samples, dur]);

  const tAt = (clientX: number) => {
    const r = svg.current!.getBoundingClientRect();
    return Math.max(0, Math.min(dur, ((clientX - r.left) / r.width) * dur));
  };

  return (
    <div className="chart">
      <svg
        ref={svg}
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={`Match score over time with ${hits.length} detections`}
        onMouseMove={(e) => setHover(tAt(e.clientX))}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => onPick(Math.round(tAt(e.clientX)))}
      >
        <rect x={0} y={y(1)} width={W} height={y(threshold) - y(1)} className="chart-zone" />
        <path d={path} className="chart-line" vectorEffect="non-scaling-stroke" />
        <line x1={0} x2={W} y1={y(threshold)} y2={y(threshold)} className="chart-threshold" vectorEffect="non-scaling-stroke" />
        {hits.map((t) => (
          <line key={t} x1={x(t)} x2={x(t)} y1={y(1)} y2={H - PAD_B} className="chart-hit" vectorEffect="non-scaling-stroke" />
        ))}
        {hover !== null && <line x1={x(hover)} x2={x(hover)} y1={0} y2={H - PAD_B} className="chart-hover" vectorEffect="non-scaling-stroke" />}
      </svg>
      <div className="chart-axis num muted small" aria-hidden>
        <span>0:00</span>
        <span>{hover !== null ? timecode(hover) : "Click to preview a moment"}</span>
        <span>{timecode(dur)}</span>
      </div>
    </div>
  );
}

/**
 * Same trigger rule as the backend (vision.rs `Trigger`): rising edge after N matching samples,
 * with a cooldown; with `rearmDrop` > 0 a dip-and-recover while still matching fires again.
 */
export function computeHits(
  samples: Array<[number, number]>,
  threshold: number,
  minConsecutive: number,
  cooldownMs: number,
  rearmDrop = 0,
): number[] {
  const out: number[] = [];
  let run = 0;
  let start = 0;
  let peak = 0;
  let fired = false;
  let last = -Infinity;
  let high = 0;
  let trough = 0;
  let armed = false;
  for (const [t, s] of samples) {
    if (s >= threshold) {
      if (run === 0) {
        start = t;
        peak = s;
        fired = false;
      }
      run++;
      peak = Math.max(peak, s);
      if (fired && rearmDrop > 0) {
        if (!armed) {
          high = Math.max(high, s);
          if (high - s >= rearmDrop) {
            armed = true;
            trough = s;
          }
        } else {
          trough = Math.min(trough, s);
          if (s - trough >= rearmDrop && t - last >= cooldownMs) {
            last = t;
            out.push(t);
            high = trough = s;
            armed = false;
            continue;
          }
        }
      }
      if (!fired && run >= Math.max(1, minConsecutive)) {
        fired = true;
        high = trough = peak;
        armed = false;
        if (start - last >= cooldownMs) {
          last = start;
          out.push(start);
        }
      }
    } else run = 0;
  }
  return out;
}
