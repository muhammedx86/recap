import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { fileSrc, isTauri } from "../lib/api";
import { timecode } from "../lib/format";
import { Icon } from "./ui";

export interface PlayerHandle {
  seek: (ms: number) => void;
  toggle: () => void;
  nudge: (deltaMs: number) => void;
  time: () => number;
}

interface Props {
  path: string;
  poster: string | null;
  durationMs: number;
  onTime: (ms: number) => void;
  /** Range to highlight, e.g. while making a clip. */
  autoPlay?: boolean;
  className?: string;
}

const RATES = [0.25, 0.5, 1, 1.5, 2];

/**
 * Video player with custom controls. In the browser mock there is no real file, so
 * playback is simulated with a clock over the poster image; the controls behave the same.
 */
export const Player = forwardRef<PlayerHandle, Props>(function Player({ path, poster, durationMs, onTime, autoPlay, className = "" }, ref) {
  const video = useRef<HTMLVideoElement>(null);
  const simulate = !isTauri;
  const [time, setTime] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [rate, setRate] = useState(1);
  const [muted, setMuted] = useState(false);
  const [dur, setDur] = useState(durationMs);
  const [failed, setFailed] = useState(false);
  const sim = useRef({ t: 0, playing: false, last: 0 });

  const report = useCallback(
    (ms: number) => {
      setTime(ms);
      onTime(ms);
    },
    [onTime],
  );

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

  useImperativeHandle(
    ref,
    () => ({
      seek,
      toggle,
      nudge: (d) => seek((simulate ? sim.current.t : (video.current?.currentTime ?? 0) * 1000) + d),
      time: () => (simulate ? sim.current.t : (video.current?.currentTime ?? 0) * 1000),
    }),
    [seek, toggle, simulate],
  );

  useEffect(() => {
    if (video.current) video.current.playbackRate = rate;
  }, [rate]);

  useEffect(() => {
    if (autoPlay && simulate) {
      sim.current.playing = true;
      setPlaying(true);
    }
  }, [autoPlay, simulate]);

  const posterSrc = fileSrc(poster);
  const total = dur || durationMs;

  return (
    <div className={`player ${className}`}>
      <div className="player-stage" onClick={toggle}>
        {simulate ? (
          <div className="player-sim" style={posterSrc ? { backgroundImage: `url("${posterSrc}")` } : undefined} aria-label="Video preview" />
        ) : (
          <video
            ref={video}
            src={fileSrc(path)}
            poster={posterSrc}
            preload="metadata"
            autoPlay={autoPlay}
            onTimeUpdate={(e) => report(e.currentTarget.currentTime * 1000)}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onLoadedMetadata={(e) => Number.isFinite(e.currentTarget.duration) && setDur(e.currentTarget.duration * 1000)}
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
      </div>
      <div className="player-bar">
        <button className="icon-btn" onClick={toggle} aria-label={playing ? "Pause (K)" : "Play (K)"}>
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
        />
        <label className="player-rate" title="Playback speed">
          <span className="sr-only">Playback speed</span>
          <select value={rate} onChange={(e) => setRate(Number(e.target.value))}>
            {RATES.map((r) => (
              <option key={r} value={r}>
                {r}×
              </option>
            ))}
          </select>
        </label>
        {!simulate && (
          <button className="icon-btn" onClick={() => setMuted(!muted)} aria-label={muted ? "Unmute" : "Mute"}>
            <Icon name={muted ? "mute" : "volume"} size={16} />
          </button>
        )}
      </div>
    </div>
  );
});
