import { useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../lib/api";
import { useLoad } from "../lib/hooks";
import { COMMON_KINDS, kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { ApiSource, CaptureMode, GameInfo, GameInput, MatchDetection } from "../lib/types";
import { Empty, Field, Icon, KindDot, Toggle } from "../components/ui";
import { gameColor } from "./Library";

const CAPTURE_LABEL: Record<CaptureMode, string> = {
  game: "Game capture",
  window: "Window capture",
  monitor: "Whole screen",
};
const MATCH_LABEL: Record<MatchDetection, string> = {
  api: "Matches from API",
  detectors: "Matches from detectors",
  process: "Whole sessions",
};
const MATCH_HINT: Record<MatchDetection, string> = {
  api: "The game's API says when each match starts and ends.",
  detectors: "Match start / Match end detectors split each session into matches.",
  process: "One recording per game session. Add Match start / Match end detectors in Studio to split sessions into matches.",
};

const API_LABEL: Record<ApiSource, string> = {
  "league-live-client": "League Live Client API",
  "cs2-gsi": "CS2 Game State Integration",
  "riot-client": "Riot Client (local)",
  "marvel-state-file": "game's match state file",
  "rocket-league-stats": "Rocket League Stats API",
};

/** APIs that only say when a match is on (and details), not plays: events still come from vision. */
const MATCH_ONLY: ApiSource[] = ["riot-client", "marvel-state-file"];

export function Games() {
  const { data: games, setData, error, loading } = useLoad(() => api.listGames(), []);
  const [adding, setAdding] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const toast = useToast();

  const setEnabled = async (g: GameInfo, enabled: boolean) => {
    setData((l) => (l ?? []).map((x) => (x.id === g.id ? { ...x, enabled } : x)));
    try {
      const u = await api.setGameEnabled(g.id, enabled);
      setData((l) => (l ?? []).map((x) => (x.id === u.id ? u : x)));
    } catch (e) {
      setData((l) => (l ?? []).map((x) => (x.id === g.id ? { ...x, enabled: !enabled } : x)));
      toast.error(e, "Could not update");
    }
  };

  return (
    <div className="page page-games">
      <header className="page-head page-head-row">
        <div>
          <h1>Games</h1>
          <p className="lede">Which games Recap records, and how it finds their matches and plays</p>
        </div>
        {!adding && (
          <button className="btn btn-primary" onClick={() => setAdding(true)}>
            <Icon name="plus" size={14} />
            Add a game
          </button>
        )}
      </header>

      {adding && (
        <AddGame
          onClose={() => setAdding(false)}
          onSaved={(g) => {
            setData((l) => [...(l ?? []).filter((x) => x.id !== g.id), g].sort((a, b) => a.name.localeCompare(b.name)));
            setAdding(false);
            setOpen(g.id);
            toast.info(`${g.name} added`);
          }}
        />
      )}

      {error && <p className="error-line">Could not load games: {error}</p>}
      {!loading && games?.length === 0 && <Empty title="No games yet">Add a game to start recording it.</Empty>}

      <ul className="game-list">
        {(games ?? []).map((g) => {
          const ready = g.detectors.filter((d) => d.ready);
          const isOpen = open === g.id;
          const custom = g.eventTypes.filter((t) => !t.builtin);
          const needs = g.detectors.filter((d) => !d.ready && d.enabled).length;
          return (
            <li key={g.id} className={`game ${g.enabled ? "" : "is-off"} ${isOpen ? "is-open" : ""}`}>
              <div className="game-row">
                <button className="game-toggle" aria-expanded={isOpen} onClick={() => setOpen(isOpen ? null : g.id)}>
                  <span className="game-dot" style={{ background: gameColor(g.id, g.color) }} aria-hidden />
                  <span className="game-name">{g.name}</span>
                  <span className="game-meta">
                    <span className="ellipsis">{g.processes.join(", ")}</span>
                    {g.hasRounds && <span>Rounds</span>}
                    <span title={MATCH_HINT[g.matchDetection]}>{MATCH_LABEL[g.matchDetection]}</span>
                    {needs > 0 && (
                      <span className="game-todo">
                        <span className="dot" aria-hidden />
                        {needs} {needs === 1 ? "detector needs" : "detectors need"} an image
                      </span>
                    )}
                  </span>
                  <span className={`game-chev ${isOpen ? "up" : ""}`} aria-hidden />
                </button>
                <Toggle checked={g.enabled} onChange={(v) => setEnabled(g, v)} label={`Record ${g.name}`} />
              </div>

              {isOpen && (
                <div className="game-detail">
                  <dl className="game-facts">
                    <div>
                      <dt>Capture</dt>
                      <dd>{CAPTURE_LABEL[g.capture]}</dd>
                    </div>
                    <div>
                      <dt>Events from</dt>
                      <dd>
                        {g.api && !MATCH_ONLY.includes(g.api) ? API_LABEL[g.api] : ready.length ? "Vision detectors" : "Bookmarks only"}
                        {g.api && !MATCH_ONLY.includes(g.api) && ready.length ? " and vision" : ""}
                        {g.api === "riot-client" ? `; match details from the ${API_LABEL[g.api]}` : ""}
                        {g.api === "marvel-state-file" ? `; matches from the ${API_LABEL[g.api]}` : ""}
                        {g.hasPostMatch ? ", plus replay files" : ""}
                      </dd>
                    </div>
                    <div>
                      <dt>Auto-clips</dt>
                      <dd>{g.autoClipEvents.length ? g.autoClipEvents.map((k) => kindStyle(k).plural).join(", ") : "None"}</dd>
                    </div>
                    {custom.length > 0 && (
                      <div>
                        <dt>Custom events</dt>
                        <dd className="kind-inline">
                          {custom.map((t) => (
                            <span key={t.id} className="kind-chip">
                              <KindDot kind={t.id} size={12} />
                              {t.label}
                            </span>
                          ))}
                        </dd>
                      </div>
                    )}
                  </dl>
                  {g.api === "rocket-league-stats" && <RocketLeagueSetup />}
                  {g.matchDetection === "process" && (
                    <p className="game-hint">
                      Each recording is a whole game session. <Link to={`/studio?game=${g.id}`}>Add Match start / end detectors</Link> to split it into matches.
                    </p>
                  )}

                  <div className="gd">
                    <div className="gd-head">
                      <h3>Detectors</h3>
                      <Link className="gd-link" to={`/studio?game=${g.id}`}>
                        Open in Studio
                      </Link>
                    </div>
                    {g.detectors.length === 0 ? (
                      <p className="muted small">{g.api ? "None needed; the game's API reports plays." : "No detectors yet."}</p>
                    ) : (
                      <ul className="gd-list">
                        {g.detectors.map((d) => (
                          <li key={d.id}>
                            <Link className="gd-item" to={`/studio?game=${g.id}&detector=${d.id}`}>
                              <KindDot kind={d.event} size={13} />
                              <span className="gd-name">{d.label || d.id}</span>
                              <span className="gd-kind">{kindStyle(d.event).label}</span>
                              <span className={`gd-state ${d.ready ? "" : d.enabled ? "todo" : ""}`}>
                                {d.ready ? `${d.templates.length} ${d.templates.length === 1 ? "image" : "images"}` : d.enabled ? "Needs an image" : "Off"}
                              </span>
                            </Link>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function AddGame({ onClose, onSaved }: { onClose: () => void; onSaved: (g: GameInfo) => void }) {
  const toast = useToast();
  const [name, setName] = useState("");
  const [processes, setProcesses] = useState("");
  const [capture, setCapture] = useState<CaptureMode>("game");
  const [events, setEvents] = useState<string[]>(["kill"]);
  const [busy, setBusy] = useState(false);
  const id = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  const procList = processes
    .split(/[,\n]+/)
    .map((p) => p.trim())
    .filter(Boolean);

  const save = async () => {
    const game: GameInput = { id, name: name.trim(), processes: procList, capture, api: null, preferApi: true, autoClipEvents: events, color: null };
    setBusy(true);
    try {
      onSaved(await api.saveGame(game));
    } catch (e) {
      toast.error(e, "Could not add the game");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="add-game"
      onSubmit={(e) => {
        e.preventDefault();
        if (id && procList.length) save();
      }}
    >
      <h2>Add a game</h2>
      <div className="add-game-grid">
        <Field label="Name" htmlFor="ag-name">
          <input id="ag-name" className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Overwatch 2" autoFocus />
        </Field>
        <Field label="Process" htmlFor="ag-proc" hint="Recap records while this .exe runs. Find it in Task Manager → Details.">
          <input id="ag-proc" className="input" value={processes} onChange={(e) => setProcesses(e.target.value)} placeholder="Overwatch.exe" />
        </Field>
        <Field label="Capture" htmlFor="ag-cap">
          <select id="ag-cap" className="input" value={capture} onChange={(e) => setCapture(e.target.value as CaptureMode)} title="Use window or whole screen if game capture records black.">
            {(Object.keys(CAPTURE_LABEL) as CaptureMode[]).map((c) => (
              <option key={c} value={c}>
                {CAPTURE_LABEL[c]}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <div className="field">
        <p className="field-label">Make clips of</p>
        <div className="check-grid">
          {COMMON_KINDS.filter((k) => k !== "round_loss").map((k) => (
            <label key={k} className="check">
              <input type="checkbox" checked={events.includes(k)} onChange={(e) => setEvents((ev) => (e.target.checked ? [...ev, k] : ev.filter((x) => x !== k)))} />
              {kindStyle(k).plural}
            </label>
          ))}
        </div>
      </div>
      <p className="field-hint">To split sessions into matches or find plays, add detectors in Studio afterwards.</p>
      <div className="add-game-actions">
        <button type="submit" className="btn btn-primary" disabled={busy || !id || !procList.length}>
          Add game
        </button>
        <button type="button" className="btn btn-ghost" onClick={onClose}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/** Rocket League reports plays through its Stats API, which is off until a setting in the game's
 * folder is switched on. Recap does that itself; this shows where things stand. */
function RocketLeagueSetup() {
  const { data: st, setData } = useLoad(() => api.rocketLeagueApiStatus(), []);
  const [busy, setBusy] = useState(false);
  const toast = useToast();
  if (!st) return null;
  const turnOn = async () => {
    setBusy(true);
    try {
      setData(await api.enableRocketLeagueApi());
    } catch (e) {
      toast.error(e, "Could not switch on the Stats API");
    } finally {
      setBusy(false);
    }
  };
  let tone = "ok";
  let text: string;
  if (st.connected) text = "Stats API on: Recap is receiving goals, saves and the score live.";
  else if (st.enabled && st.needsRestart) {
    tone = "warn";
    text = "Stats API switched on. Restart Rocket League once for it to take effect.";
  } else if (st.enabled) text = "Stats API on. Plays and the score come from the game during matches.";
  else if (!st.installs.length) {
    tone = "warn";
    text = "Rocket League wasn't found yet. Start the game once and Recap switches its Stats API on.";
  } else {
    tone = "warn";
    text = st.needsAdmin
      ? "Rocket League is installed in a protected folder. Switching its Stats API on needs administrator permission once."
      : "Rocket League's Stats API is off.";
  }
  return (
    <div className={`game-api-setup ${tone}`}>
      <Icon name={tone === "ok" ? "check" : "warning"} size={16} />
      <p>
        {text}
        {st.error && <span className="muted small"> ({st.error})</span>}
      </p>
      {!st.enabled && st.installs.length > 0 && (
        <button className="btn btn-small" onClick={turnOn} disabled={busy}>
          {busy ? "Switching on…" : "Switch on"}
        </button>
      )}
    </div>
  );
}
