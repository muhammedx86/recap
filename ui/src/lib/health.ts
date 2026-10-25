// Recording health: what to badge on a match, from the optional `health` fields.
import type { MatchHealth } from "./types";

export interface HealthIssue {
  key: string;
  /** Short badge text. */
  label: string;
  /** One sentence for the tooltip. */
  detail: string;
  /** "warn" = the recording itself may be affected; "info" = worth knowing. */
  tone: "warn" | "info";
}

/** Dropped-frame share below this is not worth a badge. */
const DROP_MIN = 0.005;
/** Encoder names that mean the GPU encoder was not used. */
const SOFTWARE = /x264|software|cpu/i;

export function healthIssues(h: MatchHealth | undefined | null): HealthIssue[] {
  if (!h) return [];
  const out: HealthIssue[] = [];
  if (h.partial) out.push({ key: "partial", label: "Partial", detail: "Recording was interrupted or started after the match did; part of the match is missing.", tone: "warn" });
  if (h.droppedFrames !== undefined && h.droppedFrames >= DROP_MIN) {
    const pct = h.droppedFrames * 100;
    out.push({
      key: "dropped",
      label: `${pct < 10 ? pct.toFixed(1) : Math.round(pct)}% dropped`,
      detail: `${pct.toFixed(1)}% of frames were dropped while encoding, so playback may stutter.`,
      tone: h.droppedFrames >= 0.02 ? "warn" : "info",
    });
  }
  if (h.lowDisk) out.push({ key: "disk", label: "Low disk", detail: "Disk space ran low while this was recording.", tone: "warn" });
  if (h.apiUnavailable) out.push({ key: "api", label: "Screen only", detail: "The game's live data never answered, so plays were found from the screen alone.", tone: "info" });
  if (h.encoder && SOFTWARE.test(h.encoder)) out.push({ key: "encoder", label: "CPU encoder", detail: `Recorded with ${h.encoder}; the GPU encoder was not available.`, tone: "info" });
  return out;
}
