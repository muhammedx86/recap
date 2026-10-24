import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { api } from "../lib/api";
import { bytes, clockTime, duration, timecode } from "../lib/format";
import { useLibraryChanged, useLoad, useProcessing } from "../lib/hooks";
import { kindStyle, sortKinds, sourceLabel } from "../lib/kinds";
import { useToast } from "../lib/toast";
import { computeRounds, type Round } from "../lib/rounds";
import { groupMoments, momentTitle, type Moment } from "../lib/moments";
import type { ClipRecord, GameEvent, MatchRecord } from "../lib/types";
import { AssetImg } from "../components/AssetImg";
import { useAssetDirs } from "../lib/assets";
import { kdaRatio, scoreParts } from "../lib/details";
import { roundScore } from "../lib/rounds";
import { loadPref, savePref } from "../lib/media";
import { Player, type PlayerHandle } from "../components/Player";
import { Timeline } from "../components/Timeline";
import { MatchDetailsStrip } from "../components/MatchDetails";
import { Empty, Icon, KindCounts, KindDot, ProgressBar, Thumb } from "../components/ui";
import { matchHeading } from "./Library";

const LEAD_IN_MS = 3000;

type SideTab = "overview" | "plays";

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
  const [tab, setTab] = useState<SideTab>(() => loadPref<SideTab>("match.tab", "overview"));
  const chooseTab = (t: SideTab) => {
    setTab(t);
    savePref("match.tab", t);
  };
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
  const moments = useMemo(() => groupMoments(visible), [visible]);
  const [grouped, setGrouped] = useState(() => loadPref<boolean>("match.grouped", true));
  const [onlyBig, setOnlyBig] = useState(false);
  const [open, setOpen] = useState<Set<number>>(new Set());
  const shownMoments = onlyBig ? moments.filter((x) => momentTitle(x)) : moments;

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

  const renderEvent = (e: GameEvent, i: number) => {
            const src = `${sourceLabel(e.source)}${e.confidence !== undefined ? `, ${Math.round(e.confidence * 100)}% match` : ""}`;
            return (
              <li key={e.id ?? i} className={`ev ${current === e ? "current" : ""}`}>
                <button className="ev-main" onClick={() => jump(e)} title={`${kindStyle(e.kind).label} at ${timecode(e.tMs)} · ${src}`}>
                  <span className="ev-time num">{timecode(e.tMs)}</span>
                  <KindDot kind={e.kind} tone="color" size={13} />
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
  };

  const customTitle = !!m.title?.trim();

  const d = m.details ?? {};
  const tabs: Array<[SideTab, string]> = [
    ["overview", "Overview"],
    ["plays", `Plays`],
  ];

  return (
    <div className="match-page">
      <div className="match-col">
        <header className="match-top">
          <BackLink />
          <div className="match-head-text">
            <h1 className="match-h1">
              {m.title?.trim() || m.details?.map?.value || matchHeading(m)}
              {m.favorite && (
                <span className="fav-star" aria-label="Favourite">
                  <Icon name="star" size={13} />
                </span>
              )}
            </h1>
            <p className="match-meta">
              <span>
                {[customTitle || d.map ? m.gameName : null, d.mode?.value].filter(Boolean).join(" · ")}
                {customTitle || d.map || d.mode ? " · " : ""}Recorded {recordedDate(m.startedUnixMs)}, {clockTime(m.startedUnixMs)}
              </span>
              {m.durationMs > 0 && <span className="num">{duration(m.durationMs)}</span>}
              {m.sizeBytes > 0 && <span className="num">{bytes(m.sizeBytes)}</span>}
            </p>
          </div>
          <div className="head-actions">
            <button className="btn btn-ghost" onClick={() => api.openPath(m.path).catch((e) => toast.error(e))}>
              <Icon name="folder" size={14} />
              Show file
            </button>
            <button className="btn" onClick={rescan} disabled={live || m.status === "processing"} title="Run this game's detectors over the recording again">
              <Icon name="refresh" size={14} />
              Rescan
            </button>
          </div>
        </header>
        {m.status === "processing" && (
          <div className="banner">
            <span>{progress !== undefined && progress < 0 ? "Plays are found once the match you are playing ends, so recording never costs frames." : `Finding plays${progress !== undefined ? ` · ${Math.round(progress * 100)}%` : "…"}`}</span>
            <ProgressBar value={progress !== undefined && progress >= 0 ? progress : 0.05} />
          </div>
        )}
        {m.status === "error" && <p className="banner banner-err">{m.error ?? "This recording failed."}</p>}
        {live && <p className="banner">Still recording. Markers and clips appear when the match ends.</p>}

        <div className="video-card">
          <Player key={m.id} ref={player} path={m.path} poster={m.thumbnail} durationMs={m.durationMs} onTime={setTime} resume={!live} />
        </div>

        <section className="timeline-card" aria-label="Timeline">
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
                <summary className="btn btn-small btn-ghost">
                  <Icon name="keyboard" size={13} />
                  Shortcuts
                </summary>
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
        </section>
        <MatchClips clips={data!.clips} />
      </div>

      <aside className="match-side" aria-label="Match">
        <div className="side-tabs" role="tablist">
          {tabs.map(([k, label]) => (
            <button key={k} role="tab" aria-selected={tab === k} className={`side-tab ${tab === k ? "on" : ""}`} onClick={() => chooseTab(k)}>
              {label}
              {k === "plays" && <span className="side-tab-n num">{visible.length}</span>}
            </button>
          ))}
        </div>

        {tab === "overview" && (
          <div className="side-body">
            <MatchSummary m={m} />
            {!live && (
              <section className="side-section">
                <h2 className="side-h">Details</h2>
                <MatchDetailsStrip key={m.id} m={m} onSaved={(rec) => setData((x) => (x ? { ...x, match: rec } : x))} />
              </section>
            )}
            <section className="side-section">
              <h2 className="side-h">Plays</h2>
              <KindCounts counts={m.eventCounts} max={8} />
            </section>
          </div>
        )}

        {tab === "plays" && (
          <div className="events-panel">
            <div className="events-head">
              <div className="seg" role="group" aria-label="List plays">
                <button className={grouped ? "on" : ""} onClick={() => (setGrouped(true), savePref("match.grouped", true))} title="Plays close together as one moment">
                  Moments
                </button>
                <button className={!grouped ? "on" : ""} onClick={() => (setGrouped(false), savePref("match.grouped", false))} title="Every play on its own">
                  All
                </button>
              </div>
              {grouped && moments.some((x) => momentTitle(x)) && (
                <label className="check-inline small">
                  <input type="checkbox" checked={onlyBig} onChange={(e) => setOnlyBig(e.target.checked)} />
                  Multikills
                </label>
              )}
              <span className="muted small events-count">{grouped ? `${shownMoments.length}` : `${visible.length}`}</span>
              {kinds.length > 1 && (
                <details className="ev-filter" key={`f-${m.id}`}>
                  <summary className="btn btn-small btn-ghost">
                    <Icon name="filter" size={13} />
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
                {live ? "Events show up here after the match." : "No plays found. Press M to add a marker, or teach Recap this game in Detector Studio."}
              </p>
            )}
            {grouped ? (
              <ol className="events-list moments-list">
                {shownMoments.map((mo) => {
                  const title = momentTitle(mo);
                  const isOpen = open.has(mo.startMs);
                  const isCurrent = time + 500 >= mo.startMs && time <= mo.endMs + 2000;
                  return (
                    <li key={mo.startMs} className={`moment ${title ? "big" : ""} ${!title && mo.events.length === 1 ? "single" : ""} ${isCurrent ? "current" : ""}`}>
                      <div className="moment-row">
                        <button className="ev-main" onClick={() => player.current?.seek(Math.max(0, mo.startMs - LEAD_IN_MS))} title={`Play from ${timecode(mo.startMs)}`}>
                          <span className="ev-time num">{timecode(mo.startMs)}</span>
                          <span className="moment-body">
                            <span className="moment-title">{title ?? momentLabel(mo)}</span>
                            <span className="moment-kinds">
                              {mo.counts.map(([k, n]) => (
                                <span key={k} className="moment-kind" title={`${n} ${n === 1 ? kindStyle(k).label : kindStyle(k).plural}`}>
                                  <KindDot kind={k} tone="color" size={12} />
                                  {n > 1 && <span className="num">{n}</span>}
                                </span>
                              ))}
                              {mo.endMs - mo.startMs >= 1000 && <span className="moment-len num">{Math.round((mo.endMs - mo.startMs) / 1000)} s</span>}
                            </span>
                          </span>
                        </button>
                        {mo.events.length > 1 && (
                          <button
                            className="icon-btn moment-toggle"
                            aria-expanded={isOpen}
                            aria-label={isOpen ? "Hide the plays in this moment" : "Show the plays in this moment"}
                            onClick={() =>
                              setOpen((o) => {
                                const n = new Set(o);
                                if (n.has(mo.startMs)) n.delete(mo.startMs);
                                else n.add(mo.startMs);
                                return n;
                              })
                            }
                          >
                            <Icon name={isOpen ? "chev-up" : "chev-down"} size={13} />
                          </button>
                        )}
                        {mo.events.length === 1 && mo.events[0].id !== undefined && (
                          <button className="icon-btn ev-del" aria-label={`Delete ${kindStyle(mo.events[0].kind).label} at ${timecode(mo.startMs)}`} onClick={() => deleteEvent(mo.events[0])}>
                            <Icon name="close" size={13} />
                          </button>
                        )}
                      </div>
                      {isOpen && <ol className="moment-children">{mo.events.map((e, i) => renderEvent(e, i))}</ol>}
                    </li>
                  );
                })}
              </ol>
            ) : (
              <ol className="events-list">{visible.map((e, i) => renderEvent(e, i))}</ol>
            )}
          </div>
        )}
      </aside>
    </div>
  );
}

function recordedDate(unixMs: number): string {
  return new Date(unixMs).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

/** The headline card: result, score, map, character, K/D/A and rank. */
function MatchSummary({ m }: { m: MatchRecord }) {
  const dirs = useAssetDirs();
  const dir = dirs[m.gameId] ?? null;
  const d = m.details ?? {};
  const res = d.result?.value.toLowerCase();
  const evScore = roundScore(m.eventCounts);
  const score = scoreParts(d.score?.value) ?? (evScore ? ([evScore.won, evScore.lost] as [number, number]) : null);
  const outcome = res === "win" || res === "loss" || res === "draw" ? res : evScore ? (evScore.won > evScore.lost ? "win" : evScore.won < evScore.lost ? "loss" : "draw") : null;
  const ratio = kdaRatio(d);
  if (!outcome && !score && !d.map && !d.character && !d.kills) return null;
  const [name, tag] = (d.player?.value ?? "").split("#");
  return (
    <section className={`summary ${outcome ?? ""}`}>
      <div className="summary-banner">
        <AssetImg dir={dir} kind="maps" name={d.map?.value} className="summary-map" />
        <div className="summary-banner-text">
          {outcome && <span className={`summary-result ${outcome}`}>{outcome === "win" ? "Victory" : outcome === "loss" ? "Defeat" : "Draw"}</span>}
          {score && (
            <span className="summary-score num">
              {score[0]}
              <span className="score-dash"> – </span>
              {score[1]}
            </span>
          )}
          <span className="summary-sub">{[d.map?.value, d.mode?.value].filter(Boolean).join(" · ")}</span>
        </div>
      </div>
      <div className="summary-row">
        {d.character && <AssetImg dir={dir} kind="agents" name={d.character.value} className="portrait" />}
        <div className="summary-who">
          {d.character && <span className="summary-char">{d.character.value}</span>}
          {d.player && (
            <span className="player-id">
              {name}
              {tag && <span className="player-tag">#{tag}</span>}
            </span>
          )}
          {d.rank && (
            <span className="rank">
              <AssetImg dir={dir} kind="ranks" name={d.rank.value} />
              {d.rank.value}
            </span>
          )}
        </div>
        {(d.kills || d.deaths) && (
          <div className="kda summary-kda">
            <span className="kda-line num">
              {d.kills?.value ?? "–"}
              <span className="kda-sep">/</span>
              <span className="kda-d">{d.deaths?.value ?? "–"}</span>
              <span className="kda-sep">/</span>
              {d.assists?.value ?? "–"}
            </span>
            {ratio && <span className="kda-ratio num">{ratio} KDA</span>}
          </div>
        )}
      </div>
    </section>
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

function BackLink() {
  return (
    <Link to="/" className="back icon-btn" aria-label="Back to the library" title="Back to the library">
      <Icon name="back" size={17} />
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

/** "Kill + assist", "Death", "2 bookmarks"... for a moment without a multikill. */
function momentLabel(mo: Moment): string {
  return mo.counts
    .slice(0, 3)
    .map(([k, n]) => (n > 1 ? `${n} ${kindStyle(k).plural.toLowerCase()}` : kindStyle(k).label))
    .join(" + ");
}
