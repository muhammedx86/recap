// Anchored regions: a HUD element pinned to a corner or edge of the picture, measured on a
// reference frame (16:9 unless the detector says otherwise). Mirrors recap-core games.rs
// Region::place / unplace so the Studio draws boxes where the scan looks.
import type { Anchor, HsvRange, Region } from "./types";

export const DEFAULT_ANCHOR_ASPECT = 16 / 9;

export const ANCHORS: Array<{ id: Anchor; label: string }> = [
  { id: "top-left", label: "Top left" },
  { id: "top", label: "Top" },
  { id: "top-right", label: "Top right" },
  { id: "left", label: "Left" },
  { id: "center", label: "Centre" },
  { id: "right", label: "Right" },
  { id: "bottom-left", label: "Bottom left" },
  { id: "bottom", label: "Bottom" },
  { id: "bottom-right", label: "Bottom right" },
];

function point(a: Anchor): [number, number] {
  const x = a.endsWith("left") ? 0 : a.endsWith("right") ? 1 : 0.5;
  const y = a.startsWith("top") ? 0 : a.startsWith("bottom") ? 1 : 0.5;
  return [x, y];
}

/** HUD size on this frame relative to the reference frame (1 on it and on wider screens). */
export function hudScale(refAspect: number, w: number, h: number): number {
  return Math.min(w / Math.max(h, 1) / Math.max(refAspect, 0.1), 1);
}

function clamped(r: Region): Region {
  const x = Math.min(Math.max(r.x, 0), 0.999);
  const y = Math.min(Math.max(r.y, 0), 0.999);
  return {
    x,
    y,
    w: Math.min(Math.max(r.w, 1e-3), 1 - x),
    h: Math.min(Math.max(r.h, 1e-3), 1 - y),
  };
}

/** A region stored on the reference frame, moved onto a w x h frame. */
export function placeRegion(r: Region, anchor: Anchor | null | undefined, refAspect: number | null | undefined, w: number, h: number): Region {
  if (!anchor || !w || !h) return r;
  const ref = refAspect || DEFAULT_ANCHOR_ASPECT;
  const aspect = w / h;
  const s = hudScale(ref, w, h);
  const [ax, ay] = point(anchor);
  const fx = ax * aspect + (r.x * ref - ax * ref) * s;
  return clamped({
    x: fx / aspect,
    y: ay + (r.y - ay) * s,
    w: (r.w * ref * s) / aspect,
    h: r.h * s,
  });
}

/** A box drawn on a w x h frame, expressed on the reference frame. */
export function unplaceRegion(r: Region, anchor: Anchor | null | undefined, refAspect: number | null | undefined, w: number, h: number): Region {
  if (!anchor || !w || !h) return r;
  const ref = refAspect || DEFAULT_ANCHOR_ASPECT;
  const aspect = w / h;
  const s = hudScale(ref, w, h);
  const [ax, ay] = point(anchor);
  const rx = ax * ref + (r.x * aspect - ax * aspect) / s;
  return clamped({
    x: rx / ref,
    y: ay + (r.y - ay) / s,
    w: (r.w * aspect) / s / ref,
    h: r.h / s,
  });
}

/** "#rrggbb" -> [hue degrees, saturation 0-1, value 0-1] (same formula as recap-core). */
export function hexToHsv(hex: string): [number, number, number] {
  const n = parseInt(hex.replace("#", ""), 16);
  const r = ((n >> 16) & 255) / 255;
  const g = ((n >> 8) & 255) / 255;
  const b = (n & 255) / 255;
  const max = Math.max(r, g, b);
  const d = max - Math.min(r, g, b);
  let hue = 0;
  if (d > 0) {
    if (max === r) hue = 60 * (((g - b) / d) % 6);
    else if (max === g) hue = 60 * ((b - r) / d + 2);
    else hue = 60 * ((r - g) / d + 4);
  }
  if (hue < 0) hue += 360;
  return [hue, max > 0 ? d / max : 0, max];
}

/** A range around a sampled colour: ±15° hue, generous saturation and brightness. */
export function rangeAround(hex: string): HsvRange {
  const [h, s, v] = hexToHsv(hex);
  const wrap = (x: number) => Math.round((x + 360) % 360);
  const c = (x: number) => Math.round(Math.min(Math.max(x, 0), 1) * 100) / 100;
  return {
    hue: [wrap(h - 15), wrap(h + 15)],
    sat: [c(s - 0.3), 1],
    val: [c(v - 0.35), 1],
  };
}

/** CSS colour of the middle of a range, for a swatch. */
export function rangeSwatch(r: HsvRange): string {
  const span = (r.hue[1] - r.hue[0] + 360) % 360;
  const h = (r.hue[0] + span / 2) % 360;
  const s = (r.sat[0] + r.sat[1]) / 2;
  const v = (r.val[0] + r.val[1]) / 2;
  // HSV -> HSL for CSS.
  const l = v * (1 - s / 2);
  const sl = l === 0 || l === 1 ? 0 : (v - l) / Math.min(l, 1 - l);
  return `hsl(${Math.round(h)} ${Math.round(sl * 100)}% ${Math.round(l * 100)}%)`;
}
