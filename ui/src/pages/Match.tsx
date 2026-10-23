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
import { Empty, Icon, KindDot, ProgressBar, RoundScore, Thumb } from "../components/ui";
import { gameColor, matchHeading } from "./Library";

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

  const events = data?.events ?? [];
  const m = data?.match;
  const kinds = useMemo(() => sortKinds([...new Set(events.map((e) => e.kind))]), [events]);
  const visible = useMemo(() => events.filter((e) => !hidden.has(e.kind)), [events, hidden]);
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

  // Keyboard: K/space play, J/L ±10s, arrows ±5s, I/O in/out, M marker.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.closest("input, textarea, select, [contenteditable=true]") || e.metaKey || e.ctrlKey || e.altKey) return;
      const p = player.current;
      if (!p) return;
      switch (e.key.toLowerCase()) {
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
  }, [addMarker]);

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

  return (
    <div className="page page-match">
      <div className="match-top">
        <BackLink />
        <div className="match-head">
          <div>
            <p className="match-game">
              <span className="game-swatch" style={{ background: gameColor(m.gameId) }} aria-hidden />
              {m.title?.trim() ? m.gameName : null}
              <span className={m.title?.trim() ? "muted" : ""}>
                {dayLabel(m.startedUnixMs)}, {clockTime(m.startedUnixMs)}
              </span>
            </p>
            <div className="match-h1-row">
              <h1 className="match-h1">{matchHeading(m)}</h1>
              <RoundScore counts={m.eventCounts} large />
            </div>
          </div>
          <div className="head-actions">
            <button className="btn" onClick={() => api.openPath(m.path).catch((e) => toast.error(e))}>
              <Icon name="folder" size={16} />
              Show file
            </button>
            <button className="btn" onClick={rescan} disabled={live || m.status === "processing"}>
              <Icon name="refresh" size={16} />
              Rescan for plays
            </button>
          </div>
        </div>
        {m.status === "processing" && (
          <div className="banner">
            <span>Finding plays in this recording{progress !== undefined ? `, ${Math.round(progress * 100)}%` : ""}.</span>
            <ProgressBar value={progress ?? 0.05} />
          </div>
        )}
        {m.status === "error" && <div className="banner banner-err">{m.error ?? "This recording failed."}</div>}
        {live && <div className="banner">This match is still recording. Markers and clips appear when it ends.</div>}
      </div>

      <div className="match-grid">
        <div className="match-main">
          <Player ref={player} path={m.path} poster={m.thumbnail} durationMs={m.durationMs} onTime={setTime} />
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
            <div className="clipper" aria-label="Make a clip">
              <span className="clipper-label">Clip</span>
              <button className="btn btn-small" onClick={() => (setIn(Math.round(time)), setOut(null))} title="Set in point (I)">
                In <span className="num">{inMs !== null ? timecode(inMs) : "–"}</span>
              </button>
              <button className="btn btn-small" onClick={() => setOut(Math.round(time))} disabled={inMs === null} title="Set out point (O)">
                Out <span className="num">{outMs !== null ? timecode(outMs) : inMs !== null ? "playhead" : "–"}</span>
              </button>
              <input className="input input-small" placeholder="Clip title (optional)" value={clipTitle} onChange={(e) => setClipTitle(e.target.value)} aria-label="Clip title" />
              <button className="btn btn-primary btn-small" disabled={inMs === null || busy || live} onClick={makeClip}>
                <Icon name="scissors" size={14} />
                {busy ? "Saving…" : "Save clip"}
              </button>
            </div>
            <button className="btn btn-small" onClick={addMarker} disabled={live} title="Add a marker at the playhead (M)">
              <Icon name="marker" size={14} />
              Add marker
            </button>
          </div>
          <p className="keys muted small">
            <kbd>K</kbd> play/pause <kbd>J</kbd>
            <kbd>L</kbd> 10 s back/forward <kbd>←</kbd>
            <kbd>→</kbd> 5 s <kbd>I</kbd>
            <kbd>O</kbd> clip in/out <kbd>M</kbd> marker
          </p>
          <MatchClips clips={data!.clips} />
        </div>

        <aside className="events-panel" aria-label="Events">
          <div className="events-head">
            <h2>Events</h2>
            <span className="muted num">{visible.length}</span>
          </div>
          {kinds.length > 0 && (
            <div className="legend" role="group" aria-label="Show or hide event kinds">
              {kinds.map((k) => {
                const off = hidden.has(k);
                return (
                  <button
                    key={k}
                    className={`legend-item ${off ? "off" : ""}`}
                    aria-pressed={!off}
                    onClick={() =>
                      setHidden((h) => {
                        const n = new Set(h);
                        if (n.has(k)) n.delete(k);
                        else n.add(k);
                        return n;
                      })
                    }
                  >
                    <KindDot kind={k} />
                    {kindStyle(k).plural}
                    <span className="num muted">{events.filter((e) => e.kind === k).length}</span>
                  </button>
                );
              })}
            </div>
          )}
          {events.length === 0 && (
            <p className="muted small events-empty">
              {live ? "Events will show up here after the match." : "No plays were found. Add markers with M, or teach Recap this game in Detector Studio."}
            </p>
          )}
          <ol className="events-list">
            {visible.map((e, i) => (
              <li key={e.id ?? i} className={`ev ${current === e ? "current" : ""}`}>
                <button className="ev-main" onClick={() => jump(e)}>
                  <span className="ev-time num">{timecode(e.tMs)}</span>
                  <KindDot kind={e.kind} />
                  <span className="ev-kind">
                    {kindStyle(e.kind).label}
                    {e.meta?.count ? ` ×${e.meta.count}` : ""}
                  </span>
                  <span className="ev-src">
                    {sourceLabel(e.source)}
                    {e.confidence !== undefined ? ` ${Math.round(e.confidence * 100)}%` : ""}
                  </span>
                </button>
                {e.id !== undefined && (
                  <button className="icon-btn ev-del" aria-label={`Delete ${kindStyle(e.kind).label} at ${timecode(e.tMs)}`} onClick={() => deleteEvent(e)}>
                    <Icon name="close" size={14} />
                  </button>
                )}
              </li>
            ))}
          </ol>
          <p className="muted small events-foot">
            {duration(m.durationMs)}, {bytes(m.sizeBytes)}
          </p>
        </aside>
      </div>
    </div>
  );
}

/** One pill per round: number and result. Click to jump to just before the round starts. */
function RoundsStrip({ rounds, timeMs, onJump }: { rounds: Round[]; timeMs: number; onJump: (r: Round) => void }) {
  const current = [...rounds].reverse().find((r) => r.startMs <= timeMs + 500);
  const won = rounds.filter((r) => r.result === "win").length;
  const lost = rounds.filter((r) => r.result === "loss").length;
  return (
    <section className="rounds" aria-label="Rounds">
      <div className="rounds-head">
        <h2>Rounds</h2>
        <span className="muted small">
          {won || lost ? (
            <>
              <span className="num">{won}</span> won, <span className="num">{lost}</span> lost
            </>
          ) : (
            `${rounds.length} rounds`
          )}
        </span>
      </div>
      <ol className="round-pills">
        {rounds.map((r) => (
          <li key={`${r.n}-${r.startMs}`}>
            <button
              className={`round-pill ${r.result ?? "none"} ${current === r ? "current" : ""}`}
              onClick={() => onJump(r)}
              aria-current={current === r ? "true" : undefined}
              title={`Round ${r.n}: ${r.result === "win" ? "won" : r.result === "loss" ? "lost" : "no result"}, starts ${timecode(r.startMs)}`}
            >
              <span className="num">{r.n}</span>
              <span className="round-pill-res" aria-hidden>
                {r.result === "win" ? "W" : r.result === "loss" ? "L" : "·"}
              </span>
              <span className="sr-only">{r.result === "win" ? "won" : r.result === "loss" ? "lost" : "no result"}</span>
            </button>
          </li>
        ))}
      </ol>
    </section>
  );
}

function BackLink() {
  return (
    <Link to="/" className="back">
      <Icon name="back" size={16} />
      Library
    </Link>
  );
}

function MatchClips({ clips }: { clips: ClipRecord[] }) {
  if (!clips.length) return null;
  return (
    <section className="match-clips">
      <h2>Clips from this match</h2>
      <ul className="clip-strip">
        {clips.map((c) => (
          <li key={c.id}>
            <Link to={`/clips?open=${c.id}`} className="clip-mini">
              <Thumb path={c.thumbnail} alt="">
                <span className="thumb-duration num">{timecode(c.durationMs)}</span>
              </Thumb>
              <span className="clip-mini-title">{c.title}</span>
              <span className="muted small num">
                {timecode(c.startMs)}–{timecode(c.endMs)}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
