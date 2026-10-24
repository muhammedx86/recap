import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { api } from "../lib/api";
import { bytes, clockTime, dayLabel, duration, timecode } from "../lib/format";
import { useLibraryChanged, useLoad, useProcessing } from "../lib/hooks";
import { kindStyle, sortKinds, sourceLabel } from "../lib/kinds";
import { useToast } from "../lib/toast";
import { computeRounds, type Round } from "../lib/rounds";
import type { ClipRecord, GameEvent } from "../lib/types";
import { Player, type PlayerHandle } from "../components/Player";
import { Timeline } from "../components/Timeline";
import { MatchDetailsStrip } from "../components/MatchDetails";
import { Empty, Icon, KindDot, ProgressBar, RoundScore, Thumb } from "../components/ui";
import { matchHeading } from "./Library";

const LEAD_IN_MS = 3000;

export function MatchPage() {
  const { id = "" } = useParams();
  const { data, error, loading, reload, setData } = useLoad(() => api.getMatch(id), [id]);
  const processing = useProcessing();
  const toast = useToast();
  const player = useRef<PlayerHandle>(null);
  const [time, setTime] = useState(0);
  const [inMs, setIn] = useState<number | null>(null);
  const [outMs, setOut] = useState<number | null>(null);
  const [clipTitle, setClipTitle] = useState("");
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  useLibraryChanged(reload, id);

  // Moving between matches reuses this page: start each one fresh.
  useEffect(() => {
    setTime(0);
    setIn(null);
    setOut(null);
    setClipTitle("");
    setHidden(new Set());
  }, [id]);

  const events = data?.events ?? [];
  const m = data?.match;
  const kinds = useMemo(() => sortKinds([...new Set(events.map((e) => e.kind))]), [events]);
  // Round starts are drawn as timeline dividers and the rounds strip; listing each one adds noise.
  const visible = useMemo(() => events.filter((e) => !hidden.has(e.kind) && e.kind !== "round_start"), [events, hidden]);
  const rounds = useMemo(() => computeRounds(events, m?.durationMs ?? 0), [events, m?.durationMs]);

  const addMarker = useCallback(async () => {
    if (!m) return;
    try {
      const ev = await api.addEvent(m.id, "bookmark", Math.round(player.current?.time() ?? time));
      setData((d) => (d ? { ...d, events: [...d.events, ev].sort((a, b) => a.tMs - b.tMs) } : d));
      toast.info(`Marker added at ${timecode(ev.tMs)}`);
    } catch (e) {
      toast.error(e, "Could not add marker");
    }
  }, [m, time, setData, toast]);

  // Previous / next marker in the list (ignores markers within a second of the playhead).
  const stepEvent = useCallback(
    (dir: -1 | 1) => {
      const p = player.current;
      if (!p) return;
      const now = p.time() + LEAD_IN_MS;
      const list = visible.map((e) => e.tMs).sort((a, b) => a - b);
      const target = dir > 0 ? list.find((t) => t > now + 1000) : [...list].reverse().find((t) => t < now - 1000);
      if (target !== undefined) p.seek(Math.max(0, target - LEAD_IN_MS));
    },
    [visible],
  );

  // Keyboard: Space/K play, J/L ±10s, arrows ±5s (Shift 1s), Ctrl+arrows previous/next marker,
  // ↑/↓ speed, Backspace normal speed, , . frame step, F full screen, I/O in/out, M marker.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.closest("input, textarea, select, [contenteditable=true]") || e.metaKey || e.altKey) return;
      const p = player.current;
      if (!p) return;
      if (e.ctrlKey) {
        if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
          e.preventDefault();
          stepEvent(e.key === "ArrowLeft" ? -1 : 1);
        }
        return;
      }
      switch (e.key.toLowerCase()) {
        case "arrowup":
          e.preventDefault();
          p.changeRate(0.25);
          break;
        case "arrowdown":
          e.preventDefault();
          p.changeRate(-0.25);
          break;
        case "backspace":
          e.preventDefault();
          p.changeRate(0);
          break;
        case ",":
          p.frameStep(-1);
          break;
        case ".":
          p.frameStep(1);
          break;
        case "f":
          p.toggleFullscreen();
          break;
        case " ":
        case "k":
          e.preventDefault();
          p.toggle();
          break;
        case "j":
          p.nudge(-10_000);
          break;
        case "l":
          p.nudge(10_000);
          break;
        case "arrowleft":
          e.preventDefault();
          p.nudge(e.shiftKey ? -1000 : -5000);
          break;
        case "arrowright":
          e.preventDefault();
          p.nudge(e.shiftKey ? 1000 : 5000);
          break;
        case "i":
          setIn(Math.round(p.time()));
          setOut(null);
          break;
        case "o":
          setOut(Math.round(p.time()));
          break;
        case "m":
          addMarker();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [addMarker, stepEvent]);

  const jump = (e: GameEvent) => player.current?.seek(Math.max(0, e.tMs - LEAD_IN_MS));

  const deleteEvent = async (e: GameEvent) => {
    if (!m || e.id === undefined) return;
    try {
      await api.deleteEvent(m.id, e.id);
      setData((d) => (d ? { ...d, events: d.events.filter((x) => x.id !== e.id) } : d));
    } catch (err) {
      toast.error(err, "Could not delete marker");
    }
  };

  const makeClip = async () => {
    if (!m || inMs === null) return;
    const end = outMs ?? Math.round(time);
    if (end <= inMs) {
      toast.error("Set the out point after the in point.");
      return;
    }
    setBusy(true);
    try {
      const c = await api.createClip(m.id, inMs, end, clipTitle.trim() || undefined);
      setData((d) => (d ? { ...d, clips: [c, ...d.clips] } : d));
      setIn(null);
      setOut(null);
      setClipTitle("");
      toast.info("Clip saved");
    } catch (e) {
      toast.error(e, "Could not make clip");
    } finally {
      setBusy(false);
    }
  };

  const rescan = async () => {
    if (!m) return;
    try {
      await api.rescanMatch(m.id);
      toast.info("Rescanning this recording for plays");
    } catch (e) {
      toast.error(e, "Could not rescan");
    }
  };

  if (error)
    return (
      <div className="page">
        <BackLink />
        <Empty title="This recording could not be opened">{error}</Empty>
      </div>
    );
  if (loading && !data) return <div className="page" aria-busy="true" />;
  if (!m) return null;

  const live = m.status === "recording";
  const progress = processing[m.id];
  const current = [...visible].reverse().find((e) => e.tMs <= time + 500);

  const customTitle = !!m.title?.trim();

  return (
    <div className="page page-match">
      <header className="match-top">
        <BackLink />
        <div className="match-head">
          <div className="match-head-text">
            <h1 className="match-h1">{matchHeading(m)}</h1>
            <p className="match-meta">
              {customTitle && <span>{m.gameName}</span>}
              <span>
                {dayLabel(m.startedUnixMs)}, {clockTime(m.startedUnixMs)}
              </span>
              {m.durationMs > 0 && <span className="num">{duration(m.durationMs)}</span>}
              {/* The details strip shows the score when one is known. */}
              {!m.details?.score && <RoundScore counts={m.eventCounts} />}
              {m.sizeBytes > 0 && <span className="num">{bytes(m.sizeBytes)}</span>}
            </p>
            {!live && <MatchDetailsStrip key={m.id} m={m} onSaved={(rec) => setData((d) => (d ? { ...d, match: rec } : d))} />}
          </div>
          <div className="head-actions">
            <button className="btn btn-ghost" onClick={() => api.openPath(m.path).catch((e) => toast.error(e))}>
              <Icon name="folder" size={15} />
              Show file
            </button>
            <button className="btn btn-ghost" onClick={rescan} disabled={live || m.status === "processing"} title="Run this game's detectors over the recording again">
              <Icon name="refresh" size={15} />
              Rescan
            </button>
          </div>
        </div>
        {m.status === "processing" && (
          <div className="banner">
            <span>Finding plays{progress !== undefined ? ` · ${Math.round(progress * 100)}%` : "…"}</span>
            <ProgressBar value={progress ?? 0.05} />
          </div>
        )}
        {m.status === "error" && <p className="banner banner-err">{m.error ?? "This recording failed."}</p>}
        {live && <p className="banner">Still recording. Markers and clips appear when the match ends.</p>}
      </header>

      <div className="match-grid">
        <div className="match-main">
          <Player key={m.id} ref={player} path={m.path} poster={m.thumbnail} durationMs={m.durationMs} onTime={setTime} resume={!live} />
          <Timeline
            events={events}
            rounds={rounds}
            durationMs={m.durationMs}
            timeMs={time}
            inMs={inMs}
            outMs={outMs}
            hiddenKinds={hidden}
            onSeek={(ms) => player.current?.seek(ms)}
            onEventClick={jump}
          />
          {rounds.length > 0 && <RoundsStrip rounds={rounds} timeMs={time} onJump={(r) => player.current?.seek(Math.max(0, r.startMs - 2000))} />}
          <div className="tools">
            <div className="clipper" role="group" aria-label="Make a clip">
              <button className="btn btn-small" onClick={() => (setIn(Math.round(time)), setOut(null))} title="Set in point (I)">
                In <span className="num">{inMs !== null ? timecode(inMs) : "–"}</span>
              </button>
              <button className="btn btn-small" onClick={() => setOut(Math.round(time))} disabled={inMs === null} title="Set out point (O)">
                Out <span className="num">{outMs !== null ? timecode(outMs) : inMs !== null ? "playhead" : "–"}</span>
              </button>
              <input className="input input-small" placeholder="Clip title" value={clipTitle} onChange={(e) => setClipTitle(e.target.value)} aria-label="Clip title" />
              <button className="btn btn-primary btn-small" disabled={inMs === null || busy || live} onClick={makeClip}>
                <Icon name="scissors" size={13} />
                {busy ? "Saving…" : "Save clip"}
              </button>
            </div>
            <div className="tools-end">
              <button className="btn btn-small btn-ghost" onClick={addMarker} disabled={live} title="Add a marker at the playhead (M)">
                <Icon name="marker" size={13} />
                Marker
              </button>
              <details className="shortcuts" key={`k-${m.id}`}>
                <summary className="btn btn-small btn-ghost">Shortcuts</summary>
                <dl className="shortcuts-panel">
                  <dt>
                    <kbd>K</kbd> <kbd>Space</kbd>
                  </dt>
                  <dd>Play / pause</dd>
                  <dt>
                    <kbd>J</kbd> <kbd>L</kbd>
                  </dt>
                  <dd>Back / forward 10 s</dd>
                  <dt>
                    <kbd>←</kbd> <kbd>→</kbd>
                  </dt>
                  <dd>Back / forward 5 s (Shift: 1 s)</dd>
                  <dt>
                    <kbd>Ctrl</kbd> <kbd>←</kbd> <kbd>→</kbd>
                  </dt>
                  <dd>Previous / next marker</dd>
                  <dt>
                    <kbd>,</kbd> <kbd>.</kbd>
                  </dt>
                  <dd>Previous / next frame</dd>
                  <dt>
                    <kbd>↑</kbd> <kbd>↓</kbd>
                  </dt>
                  <dd>Faster / slower (Backspace: normal)</dd>
                  <dt>
                    <kbd>F</kbd>
                  </dt>
                  <dd>Full screen (double-click the video too)</dd>
                  <dt>
                    <kbd>I</kbd> <kbd>O</kbd>
                  </dt>
                  <dd>Clip in / out</dd>
                  <dt>
                    <kbd>M</kbd>
                  </dt>
                  <dd>Add marker</dd>
                </dl>
              </details>
            </div>
          </div>
          <MatchClips clips={data!.clips} />
        </div>

        <aside className="events-panel" aria-label="Events">
          <div className="events-head">
            <h2>
              Events <span className="muted num">{visible.length}</span>
            </h2>
            {kinds.length > 1 && (
              <details className="ev-filter" key={`f-${m.id}`}>
                <summary className="btn btn-small btn-ghost">
                  <FilterGlyph />
                  {hidden.size > 0 ? `Filter · ${kinds.length - hidden.size}/${kinds.length}` : "Filter"}
                </summary>
                <div className="ev-filter-panel" role="group" aria-label="Show or hide event kinds">
                  {kinds.map((k) => (
                    <label key={k} className="ev-filter-item">
                      <input
                        type="checkbox"
                        checked={!hidden.has(k)}
                        onChange={() =>
                          setHidden((h) => {
                            const n = new Set(h);
                            if (n.has(k)) n.delete(k);
                            else n.add(k);
                            return n;
                          })
                        }
                      />
                      <KindDot kind={k} tone="color" size={13} />
                      <span>{kindStyle(k).plural}</span>
                      <span className="num muted">{events.filter((e) => e.kind === k).length}</span>
                    </label>
                  ))}
                </div>
              </details>
            )}
          </div>
          {events.length === 0 && (
            <p className="muted small events-empty">
              {live ? "Events show up here after the match." : "No plays found. Press M to add a marker, or teach Recap this game in Studio."}
            </p>
          )}
          <ol className="events-list">
            {visible.map((e, i) => {
              const src = `${sourceLabel(e.source)}${e.confidence !== undefined ? `, ${Math.round(e.confidence * 100)}% match` : ""}`;
              return (
                <li key={e.id ?? i} className={`ev ${current === e ? "current" : ""}`}>
                  <button className="ev-main" onClick={() => jump(e)} title={`${kindStyle(e.kind).label} at ${timecode(e.tMs)} · ${src}`}>
                    <span className="ev-time num">{timecode(e.tMs)}</span>
                    <KindDot kind={e.kind} size={13} />
                    <span className="ev-kind">
                      {kindStyle(e.kind).label}
                      {e.meta?.count ? ` ×${e.meta.count}` : ""}
                    </span>
                  </button>
                  {e.id !== undefined && (
                    <button className="icon-btn ev-del" aria-label={`Delete ${kindStyle(e.kind).label} at ${timecode(e.tMs)}`} onClick={() => deleteEvent(e)}>
                      <Icon name="close" size={13} />
                    </button>
                  )}
                </li>
              );
            })}
          </ol>
        </aside>
      </div>
    </div>
  );
}

/** A compact row of round ticks: result as a short bar, number on hover/focus. Click to jump to just before the round. */
function RoundsStrip({ rounds, timeMs, onJump }: { rounds: Round[]; timeMs: number; onJump: (r: Round) => void }) {
  const current = [...rounds].reverse().find((r) => r.startMs <= timeMs + 500);
  return (
    <section className="rounds" aria-label="Rounds">
      <span className="rounds-label">Rounds</span>
      <ol className="round-ticks">
        {rounds.map((r) => {
          const res = r.result === "win" ? "won" : r.result === "loss" ? "lost" : "no result";
          return (
            <li key={`${r.n}-${r.startMs}`}>
              <button
                className={`round-tick ${r.result ?? "none"} ${current === r ? "current" : ""}`}
                onClick={() => onJump(r)}
                aria-current={current === r ? "true" : undefined}
                aria-label={`Round ${r.n}, ${res}, starts ${timecode(r.startMs)}`}
                title={`Round ${r.n} · ${res} · ${timecode(r.startMs)}`}
              >
                <span className="round-tick-n num" aria-hidden>
                  {r.n}
                </span>
                <span className="round-tick-bar" aria-hidden />
              </button>
            </li>
          );
        })}
      </ol>
    </section>
  );
}

/** Funnel-lines glyph, drawn like the shared Icon set (1.7 stroke). */
function FilterGlyph() {
  return (
    <svg width={13} height={13} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} strokeLinecap="round" aria-hidden className="icon">
      <path d="M4 6h16M7 12h10M10 18h4" />
    </svg>
  );
}

function BackLink() {
  return (
    <Link to="/" className="back">
      <Icon name="back" size={14} />
      Library
    </Link>
  );
}

function MatchClips({ clips }: { clips: ClipRecord[] }) {
  if (!clips.length) return null;
  return (
    <section className="match-clips">
      <h2>
        Clips <span className="muted num">{clips.length}</span>
      </h2>
      <ul className="clip-strip">
        {clips.map((c) => (
          <li key={c.id}>
            <Link to={`/clips?open=${c.id}`} className="clip-mini">
              <Thumb path={c.thumbnail} alt="">
                <span className="thumb-duration num">{timecode(c.durationMs)}</span>
              </Thumb>
              <span className="clip-mini-title ellipsis">{c.title}</span>
              <span className="clip-mini-meta num">
                {timecode(c.startMs)}–{timecode(c.endMs)}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
