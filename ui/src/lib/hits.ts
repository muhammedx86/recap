// Detection rule shared by Detector Studio's live preview.

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
