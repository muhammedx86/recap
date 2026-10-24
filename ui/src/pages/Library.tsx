import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../lib/api";
import { bytes, clockTime, dayKey, dayLabel, duration, plural } from "../lib/format";
import { useLibraryChanged, useLoad, useProcessing } from "../lib/hooks";
import { useToast } from "../lib/toast";
import type { MatchRecord } from "../lib/types";
import { Confirm, Empty, Icon, Modal, ProgressBar, RoundScore, Thumb } from "../components/ui";
import { kindStyle, sortKinds } from "../lib/kinds";
import { ROUND_KINDS } from "../lib/eventTypes";
import { roundScore } from "../lib/rounds";

/** Full name for pickers and dialogs: the custom title, or game and start time. */
export function matchTitle(m: MatchRecord): string {
  return m.title?.trim() || `${m.gameName}, ${clockTime(m.startedUnixMs)}`;
}

/** Heading for rows and the match page: custom title, or just the game (the time is shown beside it). */
export function matchHeading(m: MatchRecord): string {
  return m.title?.trim() || m.gameName;
}

export function Library() {
  const { data: matches, error, loading, reload, setData } = useLoad(() => api.listMatches(), []);
  const processing = useProcessing();
  const [game, setGame] = useState<string>("all");
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

  const groups = useMemo(() => {
    const list = (matches ?? []).filter((m) => (game === "all" || m.gameId === game) && (!favOnly || m.favorite));
    const out: Array<{ key: string; label: string; items: MatchRecord[] }> = [];
    for (const m of list) {
      const k = dayKey(m.startedUnixMs);
      const g = out.find((x) => x.key === k);
      if (g) g.items.push(m);
      else out.push({ key: k, label: dayLabel(m.startedUnixMs), items: [m] });
    }
    return out;
  }, [matches, game, favOnly]);

  const patch = (m: MatchRecord) => setData((list) => (list ?? []).map((x) => (x.id === m.id ? m : x)));

  const toggleFav = async (m: MatchRecord) => {
    try {
      patch(await api.updateMatch(m.id, { favorite: !m.favorite }));
    } catch (e) {
      toast.error(e, "Could not update");
    }
  };

  return (
    <div className="page">
      <header className="page-head page-head-row">
        <h1>Library</h1>
        {(matches?.length ?? 0) > 0 && (
          <button className={`chip ${favOnly ? "on" : ""}`} onClick={() => setFavOnly(!favOnly)} aria-pressed={favOnly}>
            <Icon name="star" size={13} />
            Favourites
          </button>
        )}
      </header>

      {(matches?.length ?? 0) > 0 && (
        <div className="filters" role="toolbar" aria-label="Filter recordings">
          <button className={`chip ${game === "all" ? "on" : ""}`} onClick={() => setGame("all")}>
            All games
          </button>
          {gamesPresent.map(([id, name]) => (
            <button key={id} className={`chip ${game === id ? "on" : ""}`} onClick={() => setGame(id)}>
              {name}
            </button>
          ))}
        </div>
      )}

      {error && <p className="error-line">Could not load recordings: {error}</p>}
      {!loading && !error && matches?.length === 0 && (
        <Empty title="No recordings yet">Launch an enabled game and Recap records it. Press F9 to record anything else.</Empty>
      )}
      {!loading && matches && matches.length > 0 && groups.length === 0 && <Empty title="Nothing matches this filter" />}

      {groups.map((g) => (
        <section key={g.key} className="day">
          <div className="day-head">
            <h2 className="day-title">{g.label}</h2>
            <p className="day-meta num">
              {plural(g.items.length, "match", "matches")} · {duration(g.items.reduce((a, m) => a + m.durationMs, 0))}
            </p>
          </div>
          <ul className="match-list">
            {g.items.map((m) => (
              <MatchRow
                key={m.id}
                m={m}
                progress={processing[m.id]}
                onFav={() => toggleFav(m)}
                onRename={() => setRenaming(m)}
                onDelete={() => setDeleting(m)}
              />
            ))}
          </ul>
        </section>
      ))}

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

/** Plain-text play summary for a row, e.g. "24 kills · 15 deaths · 7 assists". Round markers show as the score instead. */
function playSummary(counts: Record<string, number>, max = 4): string[] {
  const kinds = sortKinds(Object.keys(counts).filter((k) => counts[k] > 0 && k !== "match_start" && k !== "match_end" && !ROUND_KINDS.includes(k)));
  const parts = kinds.slice(0, max).map((k) => `${counts[k]} ${(counts[k] === 1 ? kindStyle(k).label : kindStyle(k).plural).toLowerCase()}`);
  if (kinds.length > max) parts.push(`+${kinds.length - max} more`);
  return parts;
}

function MatchRow({ m, progress, onFav, onRename, onDelete }: { m: MatchRecord; progress?: number; onFav: () => void; onRename: () => void; onDelete: () => void }) {
  const live = m.status === "recording";
  const processing = m.status === "processing";
  const done = !live && !processing && m.status !== "error";
  const titled = !!m.title?.trim();
  const plays = done ? playSummary(m.eventCounts) : [];
  return (
    <li className={`match ${live ? "is-live" : ""}`}>
      <Link to={`/match/${m.id}`} className="match-link" aria-label={`Open ${matchTitle(m)}`}>
        <Thumb path={m.thumbnail} alt="" className="match-thumb" />
        <div className="match-info">
          <h3 className="match-title">
            {matchHeading(m)}
            {m.favorite && <Icon name="star" size={12} />}
          </h3>
          <p className="row-meta">
            {titled && <span>{m.gameName}</span>}
            <span className="num">{clockTime(m.startedUnixMs)}</span>
            {!live && <span className="num">{duration(m.durationMs)}</span>}
            {done && roundScore(m.eventCounts) && (
              <span>
                <RoundScore counts={m.eventCounts} />
              </span>
            )}
            {plays.map((p) => (
              <span key={p} className="num">
                {p}
              </span>
            ))}
            {done && !plays.length && !roundScore(m.eventCounts) && <span>No plays found</span>}
          </p>
          {live && (
            <p className="match-status live">
              <span className="rec-dot" aria-hidden /> Recording
            </p>
          )}
          {processing && (
            <div className="match-status">
              <ProgressBar value={progress ?? 0.05} label="Processing" />
              <span className="num">Finding plays{progress !== undefined ? ` ${Math.round(progress * 100)}%` : "…"}</span>
            </div>
          )}
          {m.status === "error" && <p className="match-status err">{m.error ?? "This recording failed."}</p>}
        </div>
      </Link>
      <div className="match-side">
        <div className="row-actions">
          <button className={`icon-btn ${m.favorite ? "is-fav" : ""}`} aria-pressed={m.favorite} aria-label={m.favorite ? "Remove from favourites" : "Add to favourites"} onClick={onFav}>
            <Icon name="star" size={16} />
          </button>
          <button className="icon-btn" aria-label="Rename" onClick={onRename}>
            <Icon name="edit" size={16} />
          </button>
          <button className="icon-btn" aria-label="Delete" onClick={onDelete} disabled={live}>
            <Icon name="trash" size={16} />
          </button>
        </div>
        <span className="match-size num">{m.sizeBytes ? bytes(m.sizeBytes) : ""}</span>
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
