// Library card thumbnail that previews the recording: moving across it scrubs a strip of stills,
// resting on it plays the video (muted) from that spot. Nothing loads until the pointer arrives,
// and the video lets go of its file as soon as the pointer leaves.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, fileSrc, previewVideoSrc } from "../lib/api";
import { timecode } from "../lib/format";
import { MEDIA_SLEEP } from "../lib/media";
import type { ScrubStrip } from "../lib/types";
import { Thumb } from "./ui";

/** Rest this long before the video starts. */
const PLAY_DELAY_MS = 650;
/** Wait this long after the pointer arrives before asking for the strip (skips fly-overs). */
const STRIP_DELAY_MS = 120;

const strips = new Map<string, Promise<ScrubStrip | null>>();
function loadStrip(matchId: string): Promise<ScrubStrip | null> {
  let p = strips.get(matchId);
  if (!p) {
    p = api.matchScrubStrip(matchId).catch(() => null);
    strips.set(matchId, p);
    // A failure is not cached forever: try again on a later hover.
    p.then((s) => s === null && strips.delete(matchId));
  }
  return p;
}

interface Props {
  matchId: string;
  path: string;
  thumbnail: string | null;
  durationMs: number;
  /** Off while recording or processing: the file is still being written. */
  enabled: boolean;
  className?: string;
  children?: ReactNode;
}

export function PreviewThumb({ matchId, path, thumbnail, durationMs, enabled, className = "", children }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const video = useRef<HTMLVideoElement>(null);
  const [hovering, setHovering] = useState(false);
  const [strip, setStrip] = useState<ScrubStrip | null>(null);
  const [frac, setFrac] = useState<number | null>(null);
  const [playing, setPlaying] = useState(false);
  const [liveMs, setLiveMs] = useState<number | null>(null);
  const timers = useRef<{ strip?: number; play?: number }>({});

  const clearTimers = () => {
    window.clearTimeout(timers.current.strip);
    window.clearTimeout(timers.current.play);
  };

  const unload = () => {
    const v = video.current;
    if (v) {
      v.pause();
      v.removeAttribute("src");
      v.load();
    }
    setPlaying(false);
  };

  // Let go of the file when the app sleeps its media (window hidden) or the card goes away.
  useEffect(() => {
    const onSleep = () => unload();
    window.addEventListener(MEDIA_SLEEP, onSleep);
    return () => {
      window.removeEventListener(MEDIA_SLEEP, onSleep);
      clearTimers();
      unload();
    };
  }, []);

  const slotTime = (f: number, frames: number) => {
    const i = Math.min(frames - 1, Math.floor(f * frames));
    return Math.round(((i + 0.5) / frames) * durationMs);
  };

  const schedulePlay = (f: number) => {
    window.clearTimeout(timers.current.play);
    timers.current.play = window.setTimeout(() => {
      const v = video.current;
      const src = previewVideoSrc(path);
      if (!v || !src) return;
      if (v.getAttribute("src") !== src) v.src = src;
      // Seek once the file's length is known (the file's own length wins over the library's).
      const seekTo = () => {
        const len = Number.isFinite(v.duration) && v.duration > 0 ? v.duration : durationMs / 1000;
        try {
          v.currentTime = Math.max(0, Math.min(len - 0.5, len * f));
        } catch {
          /* not seekable: plays from where it is */
        }
      };
      if (v.readyState >= 1) seekTo();
      else v.addEventListener("loadedmetadata", seekTo, { once: true });
      v.play().then(
        () => setPlaying(true),
        () => setPlaying(false),
      );
    }, PLAY_DELAY_MS);
  };

  const onEnter = () => {
    if (!enabled) return;
    setHovering(true);
    timers.current.strip = window.setTimeout(() => {
      loadStrip(matchId).then((s) => s && setStrip(s));
    }, STRIP_DELAY_MS);
  };

  const onMove = (e: React.MouseEvent) => {
    if (!enabled || !box.current) return;
    const r = box.current.getBoundingClientRect();
    const f = Math.max(0, Math.min(0.9999, (e.clientX - r.left) / r.width));
    setFrac(f);
    // Moving scrubs the stills; the video waits until the pointer rests again.
    if (playing) {
      video.current?.pause();
      setPlaying(false);
    }
    schedulePlay(f);
  };

  const onLeave = () => {
    clearTimers();
    setHovering(false);
    setFrac(null);
    unload();
  };

  const frames = strip?.frames ?? 0;
  const stripSrc = strip ? fileSrc(strip.path) : undefined;
  const showStrip = hovering && frac !== null && !!stripSrc && frames > 0 && !playing;
  const idx = frac !== null && frames > 0 ? Math.min(frames - 1, Math.floor(frac * frames)) : 0;
  const t = frac === null ? null : playing && liveMs !== null ? liveMs : frames > 0 ? slotTime(frac, frames) : Math.round(frac * durationMs);

  return (
    <div ref={box} className={`preview-thumb ${hovering ? "hovering" : ""} ${playing ? "playing" : ""}`} onMouseEnter={onEnter} onMouseMove={onMove} onMouseLeave={onLeave}>
      <Thumb path={thumbnail} alt="" className={className}>
        {showStrip && (
          <div
            className="scrub-frame"
            aria-hidden
            style={{
              backgroundImage: `url("${stripSrc}")`,
              backgroundSize: `${frames * 100}% 100%`,
              backgroundPosition: `${frames > 1 ? (idx / (frames - 1)) * 100 : 0}% 0`,
            }}
          />
        )}
        <video
          ref={video}
          className="preview-video"
          muted
          playsInline
          preload="none"
          aria-hidden
          onError={() => setPlaying(false)}
          onEnded={() => setPlaying(false)}
          onTimeUpdate={(e) => setLiveMs(e.currentTarget.currentTime * 1000)}
        />
        {children}
        {hovering && frac !== null && (
          <>
            <span className="scrub-bar" aria-hidden>
              <span className="scrub-bar-fill" style={{ width: `${frac * 100}%` }} />
            </span>
            {t !== null && durationMs > 0 && <span className="scrub-time num">{timecode(t)}</span>}
          </>
        )}
      </Thumb>
    </div>
  );
}
