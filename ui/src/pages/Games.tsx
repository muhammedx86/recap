import { useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../lib/api";
import { useLoad } from "../lib/hooks";
import { COMMON_KINDS, kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { ApiSource, CaptureMode, GameInfo, GameInput } from "../lib/types";
import { Empty, Field, KindDot, Modal, Toggle } from "../components/ui";
import { gameColor } from "./Library";

const CAPTURE_LABEL: Record<CaptureMode, string> = {
  game: "Game capture",
  window: "Window capture",
  monitor: "Whole screen",
};
const API_LABEL: Record<ApiSource, string> = {
  "league-live-client": "League Live Client API",
  "cs2-gsi": "CS2 Game State Integration",
};

export function Games() {
  const { data: games, setData, error, loading } = useLoad(() => api.listGames(), []);
  const [adding, setAdding] = useState(false);
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
    <div className="page">
      <header className="page-head page-head-row">
        <div>
          <h1>Games</h1>
          <p className="lede">Recap records a game as soon as one of its processes starts. Plays are found by detectors you can edit, plus official local APIs where a game has one.</p>
        </div>
        <button className="btn btn-primary" onClick={() => setAdding(true)}>
          Add a game
        </button>
      </header>

      {error && <p className="error-line">Could not load games: {error}</p>}
      {!loading && games?.length === 0 && <Empty title="No games found">Game definitions live in the games folder. Add one here to get started.</Empty>}

      <ul className="game-list">
        {(games ?? []).map((g) => {
          const ready = g.detectors.filter((d) => d.ready);
          return (
            <li key={g.id} className={`game ${g.enabled ? "" : "is-off"}`}>
              <div className="game-head">
                <span className="game-bar" style={{ background: gameColor(g.id, g.color) }} aria-hidden />
                <div className="game-title">
                  <h2>{g.name}</h2>
                  <p className="muted small">
                    Watches for {g.processes.join(", ")}
                    {g.userModified ? ". Customised by you" : ""}
                  </p>
                </div>
                <Toggle checked={g.enabled} onChange={(v) => setEnabled(g, v)} label={`Record ${g.name}`} />
              </div>
              <div className="game-facts">
                <div>
                  <p className="fact-label">Capture</p>
                  <p>{CAPTURE_LABEL[g.capture]}</p>
                </div>
                <div>
                  <p className="fact-label">Event source</p>
                  <p>
                    {g.api ? API_LABEL[g.api] : ready.length ? "Vision detectors" : "Bookmarks only"}
                    {g.api && ready.length ? " and vision" : ""}
                    {g.hasPostMatch ? ", plus replay files after the match" : ""}
                  </p>
                </div>
                <div>
                  <p className="fact-label">Auto-clips on</p>
                  <p className="kind-inline">
                    {g.autoClipEvents.length
                      ? g.autoClipEvents.map((k) => (
                          <span key={k} className="kind-chip">
                            <KindDot kind={k} />
                            {kindStyle(k).plural}
                          </span>
                        ))
                      : "Nothing"}
                  </p>
                </div>
              </div>
              <div className="detectors">
                <p className="fact-label">Detectors</p>
                {g.detectors.length === 0 ? (
                  <p className="muted small">
                    {g.api ? "None needed, the game's API reports plays. To catch something it misses, " : "No detectors yet. "}
                    <Link to={`/studio?game=${g.id}`}>{g.api ? "add one in Detector Studio" : "Create one in Detector Studio"}</Link>
                  </p>
                ) : (
                  <ul className="det-list">
                    {g.detectors.map((d) => (
                      <li key={d.id} className="det">
                        <KindDot kind={d.event} />
                        <span className="det-name">{d.label || d.id}</span>
                        <span className="muted small">{kindStyle(d.event).label}</span>
                        {d.ready ? (
                          <span className="det-state ok">Ready, {d.templates.length} {d.templates.length === 1 ? "image" : "images"}</span>
                        ) : (
                          <Link className="det-state todo" to={`/studio?game=${g.id}&detector=${d.id}`}>
                            {d.enabled ? "Needs a reference image" : "Turned off"}
                          </Link>
                        )}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </li>
          );
        })}
      </ul>

      {adding && (
        <AddGame
          onClose={() => setAdding(false)}
          onSaved={(g) => {
            setData((l) => [...(l ?? []).filter((x) => x.id !== g.id), g].sort((a, b) => a.name.localeCompare(b.name)));
            setAdding(false);
            toast.info(`${g.name} added`);
          }}
        />
      )}
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
    <Modal
      title="Add a game"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save} disabled={busy || !id || !procList.length}>
            Add game
          </button>
        </>
      }
    >
      <Field label="Name" htmlFor="ag-name">
        <input id="ag-name" className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Overwatch 2" autoFocus />
      </Field>
      <Field label="Process names" htmlFor="ag-proc" hint="The game's .exe file names, separated by commas. Find them in Task Manager under Details.">
        <input id="ag-proc" className="input" value={processes} onChange={(e) => setProcesses(e.target.value)} placeholder="Overwatch.exe" />
      </Field>
      <Field label="Capture" htmlFor="ag-cap" hint="Game capture is best. Use window or whole screen if the recording comes out black.">
        <select id="ag-cap" className="input" value={capture} onChange={(e) => setCapture(e.target.value as CaptureMode)}>
          {(Object.keys(CAPTURE_LABEL) as CaptureMode[]).map((c) => (
            <option key={c} value={c}>
              {CAPTURE_LABEL[c]}
            </option>
          ))}
        </select>
      </Field>
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
      <p className="field-hint">After adding it, create detectors for the game in Detector Studio so Recap can find plays.</p>
    </Modal>
  );
}
