import type React from "react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { api, fileSrc, isTauri } from "../lib/api";
import { timecode } from "../lib/format";
import { MEDIA_SLEEP, loadPref, loadResume, savePref, saveResume } from "../lib/media";
import { Icon } from "./ui";

export interface PlayerHandle {
  seek: (ms: number) => void;
  toggle: () => void;
  pause: () => void;
  play: () => void;
  nudge: (deltaMs: number) => void;
  time: () => number;
  /** Change speed by `delta` (e.g. ±0.25); 0 resets to 1×. */
  changeRate: (delta: number) => void;
  /** Step one frame back (-1) or forward (+1); pauses. */
  frameStep: (dir: -1 | 1) => void;
  toggleFullscreen: () => void;
  toggleMute: () => void;
}

interface Props {
  path: string;
  poster: string | null;
  durationMs: number;
  onTime: (ms: number) => void;
  autoPlay?: boolean;
  /** Pick up where playback last stopped for this file. */
  resume?: boolean;
  className?: string;
}

const RATES = [0.25, 0.5, 1, 1.5, 2];
const MIN_RATE = 0.25;
const MAX_RATE = 4;
/** One frame at 30 fps (recordings are 30 or 60 fps; this steps cleanly either way). */
const FRAME_MS = 1000 / 30;

const clampRate = (r: number) => Math.round(Math.min(MAX_RATE, Math.max(MIN_RATE, r)) * 100) / 100;

/**
 * Video player with custom controls. In the browser mock there is no real file, so
 * playback is simulated with a clock over the poster image; the controls behave the same.
 *
 * Volume, mute and speed are remembered across sessions; with `resume`, so is the position.
 * Playback pauses whenever the window is hidden (see lib/media), and the file is released on
 * unmount so nothing keeps decoding in the background.
 */
export const Player = forwardRef<PlayerHandle, Props>(function Player(
  { path, poster, durationMs, onTime, autoPlay, resume, className = "" },
  ref,
) {
  const video = useRef<HTMLVideoElement>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const simulate = !isTauri;
  const [time, setTime] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [rate, setRate] = useState(() => loadPref("player.rate", 1));
  const [muted, setMuted] = useState(() => loadPref("player.muted", false));
  const [volume, setVolume] = useState(() => loadPref("player.volume", 1));
  const [dur, setDur] = useState(durationMs);
  const [failed, setFailed] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [resumedAt, setResumedAt] = useState<number | null>(null);
  const sim = useRef({ t: 0, playing: false, last: 0 });
  const lastSaved = useRef(0);

  const report = useCallback(
    (ms: number) => {
      setTime(ms);
      onTime(ms);
    },
    [onTime],
  );

  const current = useCallback(() => (simulate ? sim.current.t : (video.current?.currentTime ?? 0) * 1000), [simulate]);

  // Simulated clock for mock mode.
  useEffect(() => {
    if (!simulate) return;
    let raf = 0;
    const tick = (now: number) => {
      const s = sim.current;
      if (s.playing) {
        const dt = s.last ? (now - s.last) * rate : 0;
        s.t = Math.min(durationMs, s.t + dt);
        if (s.t >= durationMs) {
          s.playing = false;
          setPlaying(false);
        }
        report(s.t);
      }
      s.last = now;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [simulate, rate, durationMs, report]);

  const seek = useCallback(
    (ms: number) => {
      const t = Math.max(0, Math.min(dur || durationMs, ms));
      if (simulate) {
        sim.current.t = t;
        report(t);
      } else if (video.current) {
        video.current.currentTime = t / 1000;
        report(t);
      }
    },
    [simulate, dur, durationMs, report],
  );

  const pause = useCallback(() => {
    if (simulate) {
      sim.current.playing = false;
      setPlaying(false);
    } else if (video.current && !video.current.paused) {
      video.current.pause();
    }
  }, [simulate]);

  const play = useCallback(() => {
    if (simulate) {
      sim.current.playing = true;
      setPlaying(true);
    } else if (video.current?.paused) {
      video.current.play().catch(() => {});
    }
  }, [simulate]);

  const toggle = useCallback(() => {
    if (simulate) {
      sim.current.playing = !sim.current.playing;
      setPlaying(sim.current.playing);
      return;
    }
    const v = video.current;
    if (!v) return;
    if (v.paused) v.play().catch(() => {});
    else v.pause();
  }, [simulate]);

  const changeRate = useCallback((delta: number) => setRate((r) => (delta === 0 ? 1 : clampRate(r + delta))), []);

  const frameStep = useCallback(
    (dir: -1 | 1) => {
      pause();
      seek(current() + dir * FRAME_MS);
    },
    [pause, seek, current],
  );

  const toggleFullscreen = useCallback(() => setFullscreen((f) => !f), []);
  const toggleMute = useCallback(() => setMuted((m) => !m), []);

  useImperativeHandle(
    ref,
    () => ({
      seek,
      toggle,
      pause,
      play,
      nudge: (d) => seek(current() + d),
      time: current,
      changeRate,
      frameStep,
      toggleFullscreen,
      toggleMute,
    }),
    [seek, toggle, pause, play, current, changeRate, frameStep, toggleFullscreen, toggleMute],
  );

  // Remember preferences; apply them to the element.
  useEffect(() => {
    if (video.current) video.current.playbackRate = rate;
    savePref("player.rate", rate);
  }, [rate]);
  useEffect(() => {
    if (video.current) {
      video.current.muted = muted;
      video.current.volume = volume;
    }
    savePref("player.muted", muted);
    savePref("player.volume", volume);
  }, [muted, volume]);

  // The whole window becomes the player in fullscreen (Esc leaves it).
  useEffect(() => {
    if (isTauri) api.setFullscreen(fullscreen).catch(() => {});
    else if (fullscreen) wrap.current?.requestFullscreen?.().catch(() => {});
    else if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    if (!fullscreen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFullscreen(false);
    };
    const onExit = () => !document.fullscreenElement && !isTauri && setFullscreen(false);
    window.addEventListener("keydown", onKey);
    document.addEventListener("fullscreenchange", onExit);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.removeEventListener("fullscreenchange", onExit);
    };
  }, [fullscreen]);

  // Leaving the page: never leave the window fullscreen, and remember where we were.
  useEffect(() => {
    const v = video.current;
    return () => {
      if (isTauri) api.setFullscreen(false).catch(() => {});
      if (resume) {
        const t = simulate ? sim.current.t : (v?.currentTime ?? 0) * 1000;
        rememberPosition(path, t, dur || durationMs);
      }
      // Release the file so nothing keeps decoding or holding it open.
      if (v) {
        v.pause();
        v.removeAttribute("src");
        v.load();
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  // Window hidden or minimized: pause (lib/media also pauses every media element).
  useEffect(() => {
    const sleep = () => pause();
    window.addEventListener(MEDIA_SLEEP, sleep);
    return () => window.removeEventListener(MEDIA_SLEEP, sleep);
  }, [pause]);

  useEffect(() => {
    if (autoPlay && simulate) {
      sim.current.playing = true;
      setPlaying(true);
    }
  }, [autoPlay, simulate]);

  // Resume where playback last stopped (shown briefly with a way back to the start).
  const applyResume = useCallback(
    (total: number) => {
      if (!resume) return;
      const at = loadResume(path);
      if (at !== null && at > 5000 && at < total - 5000) {
        seek(at);
        setResumedAt(at);
      }
    },
    [resume, path, seek],
  );
  useEffect(() => {
    if (simulate) applyResume(durationMs);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);
  useEffect(() => {
    if (resumedAt === null) return;
    const t = setTimeout(() => setResumedAt(null), 6000);
    return () => clearTimeout(t);
  }, [resumedAt]);

  const onTimeUpdate = (ms: number) => {
    report(ms);
    if (resume && Math.abs(ms - lastSaved.current) > 2000) {
      lastSaved.current = ms;
      rememberPosition(path, ms, dur || durationMs);
    }
  };

  const posterSrc = fileSrc(poster);
  const total = dur || durationMs;
  const rates = RATES.includes(rate) ? RATES : [...RATES, rate].sort((a, b) => a - b);

  return (
    <div ref={wrap} className={`player ${fullscreen ? "is-fullscreen" : ""} ${className}`}>
      <div className="player-stage" onClick={toggle} onDoubleClick={toggleFullscreen}>
        {simulate ? (
          <div className="player-sim" style={posterSrc ? { backgroundImage: `url("${posterSrc}")` } : undefined} aria-label="Video preview" />
        ) : (
          <video
            ref={video}
            src={fileSrc(path)}
            poster={posterSrc}
            preload="metadata"
            autoPlay={autoPlay}
            onTimeUpdate={(e) => onTimeUpdate(e.currentTarget.currentTime * 1000)}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onEnded={() => resume && saveResume(path, null)}
            onLoadedMetadata={(e) => {
              const v = e.currentTarget;
              v.playbackRate = rate;
              v.volume = volume;
              const d = Number.isFinite(v.duration) ? v.duration * 1000 : durationMs;
              setDur(d);
              applyResume(d);
            }}
            onError={() => setFailed(true)}
            muted={muted}
          />
        )}
        {failed && <p className="player-error">This video file could not be opened. It may have been moved or deleted.</p>}
        {!playing && !failed && (
          <span className="player-bigplay" aria-hidden>
            <Icon name="play" size={22} />
          </span>
        )}
        {resumedAt !== null && (
          <div className="player-resumed" onClick={(e) => e.stopPropagation()}>
            Resumed at <span className="num">{timecode(resumedAt)}</span>
            <button
              className="link-btn"
              onClick={() => {
                seek(0);
                setResumedAt(null);
              }}
            >
              Start over
            </button>
          </div>
        )}
      </div>
      <div className="player-bar">
        <button className="icon-btn" onClick={toggle} aria-label={playing ? "Pause (Space)" : "Play (Space)"} title={playing ? "Pause (Space)" : "Play (Space)"}>
          <Icon name={playing ? "pause" : "play"} size={16} />
        </button>
        <span className="player-time num">
          {timecode(time)} <span className="muted">/ {timecode(total)}</span>
        </span>
        <input
          className="player-scrub"
          type="range"
          min={0}
          max={Math.max(1, total)}
          step={100}
          value={time}
          onChange={(e) => seek(Number(e.target.value))}
          aria-label="Seek"
          style={{ "--p": `${total > 0 ? Math.min(100, (time / total) * 100) : 0}%` } as React.CSSProperties}
        />
        <label className="player-rate" title="Playback speed (↑ ↓, Backspace resets)">
          <span className="sr-only">Playback speed</span>
          <select value={rate} onChange={(e) => setRate(Number(e.target.value))}>
            {rates.map((r) => (
              <option key={r} value={r}>
                {r}×
              </option>
            ))}
          </select>
        </label>
        <div className="player-volume">
          <button className="icon-btn" onClick={toggleMute} aria-label={muted ? "Unmute" : "Mute"} title={muted ? "Unmute" : "Mute"}>
            <Icon name={muted || volume === 0 ? "mute" : "volume"} size={16} />
          </button>
          <input
            className="player-vol"
            type="range"
            min={0}
            max={1}
            step={0.05}
            value={muted ? 0 : volume}
            style={{ "--p": `${(muted ? 0 : volume) * 100}%` } as React.CSSProperties}
            onChange={(e) => {
              const v = Number(e.target.value);
              setVolume(v);
              setMuted(v === 0);
            }}
            aria-label="Volume"
          />
        </div>
        <button
          className="icon-btn"
          onClick={toggleFullscreen}
          aria-label={fullscreen ? "Exit full screen (F)" : "Full screen (F)"}
          title={fullscreen ? "Exit full screen (F, Esc)" : "Full screen (F)"}
        >
          <Icon name={fullscreen ? "minimize" : "maximize"} size={15} />
        </button>
      </div>
    </div>
  );
});

/** Save the position unless it is at the very start or end (then start fresh next time). */
function rememberPosition(path: string, ms: number, total: number) {
  if (ms < 5000 || ms > total - 5000) saveResume(path, null);
  else saveResume(path, ms);
}
