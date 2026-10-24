import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../lib/api";
import { bytes, clockTime, duration, timeAgo } from "../lib/format";
import { useLibraryChanged, useLoad, useProcessing } from "../lib/hooks";
import { useToast } from "../lib/toast";
import type { DetailValue, MatchRecord } from "../lib/types";
import { Confirm, Empty, Icon, KindDot, Modal, ProgressBar, Thumb } from "../components/ui";
import { Dropdown, MoreMenu } from "../components/Dropdown";
import { AssetImg } from "../components/AssetImg";
import { kindStyle, sortKinds } from "../lib/kinds";
import { ROUND_KINDS } from "../lib/eventTypes";
import { roundScore } from "../lib/rounds";
import { kdaRatio, scoreParts } from "../lib/details";
import { useAssetDirs } from "../lib/assets";
import { loadPref, savePref } from "../lib/media";

/** Full name for pickers and dialogs: the custom title, or game and start time. */
export function matchTitle(m: MatchRecord): string {
  return m.title?.trim() || `${m.gameName}, ${clockTime(m.startedUnixMs)}`;
}

/** Heading for rows and the match page: custom title, or just the game (the time is shown beside it). */
export function matchHeading(m: MatchRecord): string {
  return m.title?.trim() || m.gameName;
}

/** What a game calls its playable characters, for filters and labels. */
export function characterNoun(gameId: string | null, plural = true): string {
  const one =
    gameId === "valorant" ? "agent" : gameId === "league-of-legends" ? "champion" : gameId === "marvel-rivals" || gameId === "overwatch-2" ? "hero" : gameId === "apex-legends" ? "legend" : gameId === "r6-siege" ? "operator" : "character";
  return plural ? `${one}s` : one;
}

/** Short tag for a game tab. */
const CODES: Record<string, string> = {
  valorant: "VAL",
  "league-of-legends": "LOL",
  cs2: "CS2",
  "marvel-rivals": "MR",
  "r6-siege": "R6",
  "apex-legends": "APX",
  fortnite: "FN",
  manual: "REC",
};
export function gameCode(id: string, name: string): string {
  return CODES[id] ?? name.replace(/[^A-Za-z0-9 ]/g, "").split(/\s+/).filter(Boolean).map((w) => w[0]).join("").slice(0, 3).toUpperCase();
}

type View = "list" | "grid";
type Result = "win" | "loss";

export function Library() {
  const { data: matches, error, loading, reload, setData } = useLoad(() => api.listMatches(), []);
  const processing = useProcessing();
  const dirs = useAssetDirs();
  const [game, setGame] = useState<string>(() => loadPref("library.game", "all"));
  const [view, setView] = useState<View>(() => loadPref<View>("library.view", "list"));
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<Result | null>(null);
  const [map, setMap] = useState<string | null>(null);
  const [character, setCharacter] = useState<string | null>(null);
  const [mode, setMode] = useState<string | null>(null);
  const [favOnly, setFavOnly] = useState(false);
  const [renaming, setRenaming] = useState<MatchRecord | null>(null);
  const [deleting, setDeleting] = useState<MatchRecord | null>(null);
  const toast = useToast();
  useLibraryChanged(reload);

  const gamesPresent = useMemo(() => {
    const m = new Map<string, string>();
    (matches ?? []).forEach((x) => m.set(x.gameId, x.gameName));
    return [...m.entries()];
  }, [matches]);
  const activeGame = game !== "all" && gamesPresent.some(([id]) => id === game) ? game : "all";

  const inGame = useMemo(() => (matches ?? []).filter((m) => activeGame === "all" || m.gameId === activeGame), [matches, activeGame]);
  const values = (field: string) => [...new Set(inGame.map((m) => m.details?.[field]?.value).filter((v): v is string => !!v))].sort((a, b) => a.localeCompare(b));
  const maps = values("map");
  const characters = values("character");
  const modes = values("mode");
  const assetsDir = activeGame === "all" ? null : (dirs[activeGame] ?? null);

  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    const has = (m: MatchRecord, f: string, v: string | null) => v === null || m.details?.[f]?.value === v;
    return inGame.filter(
      (m) =>
        (!favOnly || m.favorite) &&
        (result === null || m.details?.result?.value.toLowerCase() === result || (!m.details?.result && outcomeFromEvents(m) === result)) &&
        has(m, "map", map) &&
        has(m, "character", character) &&
        has(m, "mode", mode) &&
        (!q || [m.title, m.gameName, ...Object.values(m.details ?? {}).map((d) => d.value)].some((v) => v && v.toLowerCase().includes(q))),
    );
  }, [inGame, favOnly, result, map, character, mode, query]);

  const filtered = favOnly || result !== null || map !== null || character !== null || mode !== null || !!query.trim();
  const clearFilters = () => {
    setFavOnly(false);
    setResult(null);
    setMap(null);
    setCharacter(null);
    setMode(null);
    setQuery("");
  };

  const chooseGame = (id: string) => {
    setGame(id);
    savePref("library.game", id);
    setMap(null);
    setCharacter(null);
    setMode(null);
  };
  const chooseView = (v: View) => {
    setView(v);
    savePref("library.view", v);
  };

  const patch = (m: MatchRecord) => setData((l) => (l ?? []).map((x) => (x.id === m.id ? m : x)));
  const toggleFav = async (m: MatchRecord) => {
    try {
      patch(await api.updateMatch(m.id, { favorite: !m.favorite }));
    } catch (e) {
      toast.error(e, "Could not update");
    }
  };
  const actions = (m: MatchRecord) => ({
    onFav: () => toggleFav(m),
    onRename: () => setRenaming(m),
    onDelete: () => setDeleting(m),
    onShow: () => api.openPath(m.path).catch((e) => toast.error(e)),
  });
  const noun = characterNoun(activeGame === "all" ? null : activeGame);
  const any = (matches?.length ?? 0) > 0;

  return (
    <div className="page page-library">
      <header className="page-head">
        <h1>Match Library</h1>
        <p className="lede">Browse and manage your recorded matches</p>
      </header>

      {any && (
        <>
          <div className="lib-search-row">
            <label className="search">
              <Icon name="search" size={14} />
              <input className="input" type="search" placeholder="Search maps, agents, titles…" value={query} onChange={(e) => setQuery(e.target.value)} aria-label="Search recordings" />
            </label>
          </div>

          <div className="lib-tabs-row">
            <div className="game-tabs" role="tablist" aria-label="Game">
              <button role="tab" aria-selected={activeGame === "all"} className={`chip ${activeGame === "all" ? "on" : ""}`} onClick={() => chooseGame("all")}>
                <Icon name="library" size={14} />
                All games
              </button>
              {gamesPresent.map(([id, name]) => (
                <button key={id} role="tab" aria-selected={activeGame === id} className={`chip ${activeGame === id ? "on" : ""}`} onClick={() => chooseGame(id)}>
                  <span className="game-glyph" aria-hidden>
                    {gameCode(id, name)}
                  </span>
                  {name}
                </button>
              ))}
            </div>
            <div className="seg icons" role="group" aria-label="View">
              <button className={view === "list" ? "on" : ""} aria-pressed={view === "list"} aria-label="List view" title="List view" onClick={() => chooseView("list")}>
                <Icon name="list" size={14} />
              </button>
              <button className={view === "grid" ? "on" : ""} aria-pressed={view === "grid"} aria-label="Grid view" title="Grid view" onClick={() => chooseView("grid")}>
                <Icon name="grid" size={14} />
              </button>
            </div>
          </div>

          <div className="lib-filters" role="toolbar" aria-label="Filter recordings">
            <Dropdown
              label="Result"
              allLabel="All results"
              value={result}
              onChange={(v) => setResult(v as Result | null)}
              options={[
                { value: "win", label: "Wins" },
                { value: "loss", label: "Losses" },
              ]}
            />
            {maps.length > 0 && (
              <Dropdown
                label="Map"
                allLabel="All maps"
                value={map}
                onChange={setMap}
                options={maps.map((v) => ({ value: v, label: v, icon: assetsDir ? <AssetImg dir={assetsDir} kind="maps" name={v} /> : undefined }))}
              />
            )}
            {characters.length > 0 && (
              <Dropdown
                label={noun}
                allLabel={`All ${noun}`}
                value={character}
                onChange={setCharacter}
                options={characters.map((v) => ({ value: v, label: v, icon: assetsDir ? <AssetImg dir={assetsDir} kind="agents" name={v} fallback="none" /> : undefined }))}
              />
            )}
            {modes.length > 1 && <Dropdown label="Mode" allLabel="All modes" value={mode} onChange={setMode} options={modes.map((v) => ({ value: v, label: v }))} />}
            <Dropdown
              label="Favourites"
              allLabel="All matches"
              value={favOnly ? "fav" : null}
              onChange={(v) => setFavOnly(v === "fav")}
              options={[{ value: "fav", label: "Favourites" }]}
            />
            {filtered && (
              <button className="btn btn-ghost btn-small" onClick={clearFilters}>
                Clear
              </button>
            )}
            <span className="lib-count num">
              {list.length} {list.length === 1 ? "match" : "matches"}
            </span>
          </div>
        </>
      )}

      {error && <p className="error-line">Could not load recordings: {error}</p>}
      {!loading && !error && matches?.length === 0 && (
        <Empty title="No recordings yet">Launch an enabled game and Recap records it. Press F9 to record anything else.</Empty>
      )}
      {!loading && any && list.length === 0 && (
        <Empty
          title="No matches found"
          action={
            filtered ? (
              <button className="btn" onClick={clearFilters}>
                Clear filters
              </button>
            ) : undefined
          }
        />
      )}

      {view === "list" ? (
        <ul className="match-list">
          {list.map((m) => (
            <MatchRow key={m.id} m={m} dir={dirs[m.gameId] ?? null} progress={processing[m.id]} showGame={activeGame === "all"} {...actions(m)} />
          ))}
        </ul>
      ) : (
        <ul className="match-grid-view">
          {list.map((m) => (
            <MatchCard key={m.id} m={m} dir={dirs[m.gameId] ?? null} progress={processing[m.id]} showGame={activeGame === "all"} {...actions(m)} />
          ))}
        </ul>
      )}

      {renaming && <RenameDialog m={renaming} onClose={() => setRenaming(null)} onSaved={patch} />}
      {deleting && (
        <Confirm
          title="Delete recording"
          danger
          confirmLabel="Delete recording"
          checkboxLabel="Also delete the video file from disk"
          body={
            <p>
              <strong>{matchTitle(deleting)}</strong> and its markers will be removed from the library. Clips you already made are kept.
            </p>
          }
          onClose={() => setDeleting(null)}
          onConfirm={async (deleteFiles) => {
            try {
              await api.deleteMatch(deleting.id, deleteFiles);
              setData((l) => (l ?? []).filter((x) => x.id !== deleting.id));
              toast.info("Recording deleted");
            } catch (e) {
              toast.error(e, "Could not delete");
            }
          }}
        />
      )}
    </div>
  );
}

/** Win/loss from round markers when no result detail is known. */
function outcomeFromEvents(m: MatchRecord): Result | null {
  const s = roundScore(m.eventCounts);
  if (!s || s.won === s.lost) return null;
  return s.won > s.lost ? "win" : "loss";
}

interface RowData {
  live: boolean;
  processing: boolean;
  done: boolean;
  outcome: Result | "draw" | null;
  heading: string;
  score: [number, number] | null;
  mode: string | null;
  character: string | null;
  rank: string | null;
  player: string | null;
  kda: { k: string; d: string; a: string } | null;
  ratio: string | null;
  plays: Array<{ kind: string; n: number }>;
}

function rowData(m: MatchRecord): RowData {
  const live = m.status === "recording";
  const processing = m.status === "processing";
  const done = !live && !processing && m.status !== "error";
  const d: Record<string, DetailValue> = m.details ?? {};
  const res = d.result?.value.toLowerCase();
  const outcome = done ? (res === "win" || res === "loss" || res === "draw" ? res : outcomeFromEvents(m)) : null;
  const evScore = roundScore(m.eventCounts);
  const score = scoreParts(d.score?.value) ?? (evScore ? [evScore.won, evScore.lost] : null);
  const hasKda = !!(d.kills || d.deaths);
  const kda = hasKda ? { k: d.kills?.value ?? "–", d: d.deaths?.value ?? "–", a: d.assists?.value ?? "–" } : null;
  const skip = new Set(["match_start", "match_end", ...ROUND_KINDS, ...(hasKda ? ["kill", "death", "assist"] : [])]);
  const plays = sortKinds(Object.keys(m.eventCounts).filter((k) => m.eventCounts[k] > 0 && !skip.has(k))).map((k) => ({ kind: k, n: m.eventCounts[k] }));
  return {
    live,
    processing,
    done,
    outcome,
    heading: m.title?.trim() || d.map?.value || m.gameName,
    score: done ? score : null,
    mode: d.mode?.value ?? null,
    character: d.character?.value ?? null,
    rank: d.rank?.value ?? null,
    player: d.player?.value ?? null,
    kda: done ? kda : null,
    ratio: done ? kdaRatio(d) : null,
    plays: done ? plays : [],
  };
}

interface RowProps {
  m: MatchRecord;
  dir: string | null;
  progress?: number;
  showGame: boolean;
  onFav: () => void;
  onRename: () => void;
  onDelete: () => void;
  onShow: () => void;
}

function menuItems(p: RowProps, live: boolean) {
  return [
    { label: p.m.favorite ? "Remove from favourites" : "Add to favourites", icon: "star", onClick: p.onFav },
    { label: "Rename", icon: "edit", onClick: p.onRename },
    { label: "Show file", icon: "folder", onClick: p.onShow },
    { label: "Delete", icon: "trash", onClick: p.onDelete, danger: true, disabled: live },
  ];
}

function Score({ score, outcome }: { score: [number, number]; outcome: RowData["outcome"] }) {
  return (
    <span className={`score ${outcome ?? ""}`}>
      <span className="num">{score[0]}</span>
      <span className="score-dash">–</span>
      <span className="num">{score[1]}</span>
    </span>
  );
}

function Kda({ r }: { r: RowData }) {
  if (!r.kda) return null;
  return (
    <div className="kda" title="Kills / deaths / assists">
      <span className="kda-line num">
        {r.kda.k}
        <span className="kda-sep">/</span>
        <span className="kda-d">{r.kda.d}</span>
        <span className="kda-sep">/</span>
        {r.kda.a}
      </span>
      {r.ratio && <span className="kda-ratio num">{r.ratio} KDA</span>}
    </div>
  );
}

function Plays({ plays, max = 3 }: { plays: RowData["plays"]; max?: number }) {
  if (!plays.length) return null;
  return (
    <div className="plays">
      {plays.slice(0, max).map((p) => (
        <span key={p.kind} className="play" title={kindStyle(p.kind).plural}>
          <KindDot kind={p.kind} size={12} />
          <span className="num">{p.n}</span>
          <span className="play-label">{(p.n === 1 ? kindStyle(p.kind).label : kindStyle(p.kind).plural).toLowerCase()}</span>
        </span>
      ))}
      {plays.length > max && <span className="play play-more">+{plays.length - max}</span>}
    </div>
  );
}

function StatusLine({ m, r, progress }: { m: MatchRecord; r: RowData; progress?: number }) {
  if (r.live)
    return (
      <span className="row-status live">
        <span className="rec-dot" aria-hidden /> Recording
      </span>
    );
  if (r.processing)
    return (
      <span className="row-status">
        <ProgressBar value={progress ?? 0.05} label="Processing" />
        <span className="num">Finding plays{progress !== undefined ? ` ${Math.round(progress * 100)}%` : "…"}</span>
      </span>
    );
  if (m.status === "error") return <span className="row-status err">{m.error ?? "This recording failed."}</span>;
  return null;
}

function MatchRow(p: RowProps) {
  const { m, dir, showGame, progress } = p;
  const r = rowData(m);
  const hasStatus = r.live || r.processing || m.status === "error";
  return (
    <li className={`mrow ${r.outcome ?? ""} ${r.live ? "is-live" : ""}`}>
      <Link to={`/match/${m.id}`} className="mrow-link" aria-label={`Open ${matchTitle(m)}`}>
        <div className="mrow-main">
          <div className="mrow-title">
            <span className="ellipsis">{r.heading}</span>
            {m.favorite && (
              <span className="fav-star" aria-label="Favourite">
                <Icon name="star" size={11} />
              </span>
            )}
          </div>
          {r.score && <Score score={r.score} outcome={r.outcome} />}
          <div className="mrow-sub ellipsis">
            {[(showGame || !r.mode) && r.heading !== m.gameName ? m.gameName : null, r.mode].filter(Boolean).join(" · ") || (r.live ? "Live" : "")}
          </div>
          <div className="mrow-time">
            <span title={new Date(m.startedUnixMs).toLocaleString()}>{timeAgo(m.startedUnixMs)}</span>
            {!r.live && m.durationMs > 0 && <span className="num"> · {duration(m.durationMs)}</span>}
          </div>
        </div>

        {hasStatus ? (
          <div className="mrow-status">
            <StatusLine m={m} r={r} progress={progress} />
          </div>
        ) : (
          <>
            <div className="mrow-player">
              {r.player && <PlayerName id={r.player} />}
              {r.rank && (
                <span className="rank">
                  <AssetImg dir={dir} kind="ranks" name={r.rank} />
                  {r.rank}
                </span>
              )}
            </div>
            <div className="mrow-agent">{r.character && <AssetImg dir={dir} kind="agents" name={r.character} className="portrait" />}</div>
            <div className="mrow-kda">
              <Kda r={r} />
              {!r.kda && r.character && <span className="mrow-char">{r.character}</span>}
            </div>
            <div className="mrow-plays">
              <Plays plays={r.plays} />
              {r.done && !r.plays.length && !r.kda && !r.score && <span className="faint small">No plays found</span>}
            </div>
          </>
        )}
        <div className="mrow-end">
          <span className="mrow-size num">{m.sizeBytes ? bytes(m.sizeBytes) : ""}</span>
        </div>
      </Link>
      <div className="mrow-menu">
        <MoreMenu items={menuItems(p, r.live)} />
      </div>
    </li>
  );
}

function PlayerName({ id }: { id: string }) {
  const [name, tag] = id.split("#");
  return (
    <span className="player-id ellipsis">
      {name}
      {tag && <span className="player-tag">#{tag}</span>}
    </span>
  );
}

function MatchCard(p: RowProps) {
  const { m, dir, showGame, progress } = p;
  const r = rowData(m);
  return (
    <li className={`mcard ${r.outcome ?? ""} ${r.live ? "is-live" : ""}`}>
      <Link to={`/match/${m.id}`} className="mcard-link" aria-label={`Open ${matchTitle(m)}`}>
        <Thumb path={m.thumbnail} alt="" className="mcard-thumb">
          {r.outcome && r.outcome !== "draw" && <span className={`mcard-badge ${r.outcome}`}>{r.outcome === "win" ? "Victory" : "Defeat"}</span>}
          {!r.live && m.durationMs > 0 && <span className="thumb-duration num">{duration(m.durationMs)}</span>}
          {r.live && (
            <span className="thumb-tag live">
              <span className="rec-dot" aria-hidden /> Recording
            </span>
          )}
        </Thumb>
        <div className="mcard-body">
          <div className="mcard-top">
            {r.character && <AssetImg dir={dir} kind="agents" name={r.character} className="portrait small" />}
            <div className="mcard-text">
              <div className="mrow-title">
                <span className="ellipsis">{r.heading}</span>
                {m.favorite && (
                  <span className="fav-star" aria-label="Favourite">
                    <Icon name="star" size={11} />
                  </span>
                )}
              </div>
              <div className="mrow-sub ellipsis">{[(showGame || !r.mode) && r.heading !== m.gameName ? m.gameName : null, r.mode, timeAgo(m.startedUnixMs)].filter(Boolean).join(" · ")}</div>
            </div>
            {r.score && <Score score={r.score} outcome={r.outcome} />}
          </div>
          <StatusLine m={m} r={r} progress={progress} />
          {r.done && (r.kda || r.rank || r.plays.length > 0) && (
            <div className="mcard-stats">
              <Kda r={r} />
              {r.rank && (
                <span className="rank">
                  <AssetImg dir={dir} kind="ranks" name={r.rank} />
                  {r.rank}
                </span>
              )}
              {!r.kda && <Plays plays={r.plays} max={2} />}
            </div>
          )}
        </div>
      </Link>
      <div className="mcard-menu">
        <MoreMenu items={menuItems(p, r.live)} />
      </div>
    </li>
  );
}

const COLORS: Record<string, string> = {
  valorant: "#ff5a5f",
  "league-of-legends": "#c8a14b",
  cs2: "#e8913a",
  "marvel-rivals": "#8f6bff",
  fortnite: "#4fb3ff",
  "r6-siege": "#4fd1c5",
};
export function gameColor(id: string, fallback?: string | null): string {
  return fallback || COLORS[id] || "#8d9099";
}

function RenameDialog({ m, onClose, onSaved }: { m: MatchRecord; onClose: () => void; onSaved: (m: MatchRecord) => void }) {
  const [title, setTitle] = useState(m.title ?? "");
  const toast = useToast();
  const save = async () => {
    try {
      onSaved(await api.updateMatch(m.id, { title: title.trim() }));
      onClose();
    } catch (e) {
      toast.error(e, "Could not rename");
    }
  };
  return (
    <Modal
      title="Rename recording"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save}>
            Save name
          </button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <input className="input input-large" autoFocus value={title} placeholder={matchTitle({ ...m, title: null })} onChange={(e) => setTitle(e.target.value)} aria-label="Title" />
        <p className="field-hint">Leave empty to use the game name and start time.</p>
      </form>
    </Modal>
  );
}
