import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { useSearchParams } from "react-router-dom";
import { api, fileSrc, pickPath } from "../lib/api";
import { timecode } from "../lib/format";
import { useLoad } from "../lib/hooks";
import { kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { DetectorDef, FrameResult, FrameSource, GameInfo, MatchRecord, Region, TestResult } from "../lib/types";
import { ScoreChart, computeHits } from "../components/ScoreChart";
import { Confirm, Empty, Field, Icon, KindDot, Toggle } from "../components/ui";
import { EventTypePicker } from "../components/EventTypePicker";
import { matchTitle } from "./Library";

const NEW = "__new__";

function blankDetector(): DetectorDef {
  return {
    id: "",
    label: "",
    event: "kill",
    region: { x: 0.4, y: 0.65, w: 0.2, h: 0.15 },
    templates: [],
    templateFrameHeight: 1080,
    method: "ncc",
    threshold: 0.8,
    cooldownMs: 1500,
    minConsecutive: 2,
    sampleFps: 6,
    scales: [],
    enabled: true,
  };
}

function expand(b: Region, m: number): Region {
  const x = Math.max(0, b.x - b.w * m);
  const y = Math.max(0, b.y - b.h * m);
  const r = Math.min(1, b.x + b.w * (1 + m));
  const bt = Math.min(1, b.y + b.h * (1 + m));
  return { x, y, w: r - x, h: bt - y };
}

const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 40);

export function Studio() {
  const toast = useToast();
  const [params] = useSearchParams();
  const { data: games, setData: setGames, error: gamesError } = useLoad(() => api.listGames(), []);
  const { data: matches } = useLoad(() => api.listMatches(), []);
  const [gameId, setGameId] = useState<string>(params.get("game") ?? "");
  const [detId, setDetId] = useState<string>(params.get("detector") ?? NEW);
  const [draft, setDraft] = useState<DetectorDef>(blankDetector());
  const [box, setBox] = useState<Region | null>(null);
  const [margin, setMargin] = useState(0.25);
  const [sourceKind, setSourceKind] = useState<"video" | "image">("video");
  const [matchId, setMatchId] = useState<string>("");
  const [tMs, setTMs] = useState(30_000);
  const [imagePath, setImagePath] = useState<string | null>(null);
  const [frame, setFrame] = useState<FrameResult | null>(null);
  const [frameSource, setFrameSource] = useState<FrameSource | null>(null);
  const [loadingFrame, setLoadingFrame] = useState(false);
  const [test, setTest] = useState<TestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const game: GameInfo | undefined = games?.find((g) => g.id === gameId);
  const existing = game?.detectors.find((d) => d.id === detId);
  const gameMatches = useMemo(() => (matches ?? []).filter((m) => m.gameId === gameId && m.status !== "recording"), [matches, gameId]);
  const match: MatchRecord | undefined = gameMatches.find((m) => m.id === matchId);

  // Default selections once data arrives.
  useEffect(() => {
    if (games && !gameId && games.length) setGameId(games[0].id);
  }, [games, gameId]);
  useEffect(() => {
    if (!matchId && gameMatches.length) setMatchId(gameMatches[0].id);
    if (matchId && !gameMatches.some((m) => m.id === matchId)) setMatchId(gameMatches[0]?.id ?? "");
  }, [gameMatches, matchId]);

  // Load the selected detector into the form.
  useEffect(() => {
    setTest(null);
    setBox(null);
    if (existing) {
      const { ready: _r, user: _u, templateUrls: _t, ...def } = existing;
      setDraft(def);
    } else {
      setDraft(blankDetector());
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [gameId, detId, existing?.id]);

  const loadFrame = async (src: FrameSource) => {
    setLoadingFrame(true);
    try {
      setFrame(await api.studioLoadFrame(src));
      setFrameSource(src);
    } catch (e) {
      toast.error(e, "Could not load the frame");
    } finally {
      setLoadingFrame(false);
    }
  };

  // Reload the frame when the recording changes.
  useEffect(() => {
    if (sourceKind === "video" && match) loadFrame({ kind: "video", path: match.path, tMs: Math.min(tMs, Math.max(0, match.durationMs - 1000)) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sourceKind, match?.id]);

  const set = <K extends keyof DetectorDef>(k: K, v: DetectorDef[K]) => setDraft((d) => ({ ...d, [k]: v }));

  const shownBox = box ?? draft.templateBox ?? null;
  const shownRegion = box ? expand(box, margin) : draft.region;
  const isNew = detId === NEW;
  const hits = useMemo(
    () => (test ? computeHits(test.samples, draft.threshold, draft.minConsecutive, draft.cooldownMs, draft.rearmDrop ?? 0) : []),
    [test, draft.threshold, draft.minConsecutive, draft.cooldownMs, draft.rearmDrop],
  );

  const save = async () => {
    if (!game) return;
    const id = draft.id || slug(draft.label);
    if (!id) {
      toast.error("Give the detector a name first.");
      return;
    }
    if (isNew && !box) {
      toast.error("Draw a box around the thing to detect on the frame first.");
      return;
    }
    if (box && !frameSource) {
      toast.error("Load a frame before saving a reference image.");
      return;
    }
    const det: DetectorDef = {
      ...draft,
      id,
      label: draft.label || kindStyle(draft.event).label,
      region: box ? expand(box, margin) : draft.region,
      templateBox: box ?? draft.templateBox,
    };
    setSaving(true);
    try {
      const updated = await api.studioSaveDetector(game.id, det, frameSource ?? { kind: "image", path: "" }, box);
      setGames((gs) => (gs ?? []).map((g) => (g.id === updated.id ? updated : g)));
      setDetId(id);
      setBox(null);
      toast.info(box ? "Detector saved with a new reference image" : "Detector saved");
    } catch (e) {
      toast.error(e, "Could not save the detector");
    } finally {
      setSaving(false);
    }
  };

  const runTest = async () => {
    if (!game || !matchId) return;
    setTesting(true);
    try {
      setTest(await api.studioTestDetector(game.id, { ...draft, id: draft.id || slug(draft.label) || "draft" }, matchId));
    } catch (e) {
      toast.error(e, "Test failed");
    } finally {
      setTesting(false);
    }
  };

  const remove = async () => {
    if (!game || !existing) return;
    try {
      const updated = await api.studioDeleteDetector(game.id, existing.id);
      setGames((gs) => (gs ?? []).map((g) => (g.id === updated.id ? updated : g)));
      setDetId(NEW);
      toast.info("Detector deleted");
    } catch (e) {
      toast.error(e, "Could not delete");
    }
  };

  if (gamesError) return <div className="page"><Empty title="Games could not be loaded">{gamesError}</Empty></div>;

  const hasTemplates = draft.templates.length > 0;

  return (
    <div className="page page-studio">
      <header className="page-head">
        <h1>Detector Studio</h1>
      </header>

      <div className="studio">
        <nav className="studio-rail" aria-label="Detectors">
          <select id="st-game" className="input" value={gameId} onChange={(e) => (setGameId(e.target.value), setDetId(NEW))} aria-label="Game">
            {(games ?? []).map((g) => (
              <option key={g.id} value={g.id}>
                {g.name}
              </option>
            ))}
          </select>
          {game?.api && (
            <p className="rail-note" title={`${game.name} also reports events through its official local API. Detectors add anything the API misses.`}>
              Also uses the game's API
            </p>
          )}
          <ul className="det-list" role="tablist" aria-label={`Detectors for ${game?.name ?? "this game"}`}>
            {(game?.detectors ?? []).map((d) => (
              <li key={d.id}>
                <button role="tab" aria-selected={detId === d.id} className={`det-item ${detId === d.id ? "on" : ""}`} onClick={() => setDetId(d.id)}>
                  <KindDot kind={d.event} size={14} />
                  <span className="det-name">{d.label || d.id}</span>
                  {!d.ready && (
                    <span className={`det-state ${d.enabled ? "todo" : ""}`} title={d.enabled ? "Needs a reference image" : "Turned off"}>
                      {d.enabled ? <span className="dot" aria-label="Needs a reference image" /> : "Off"}
                    </span>
                  )}
                </button>
              </li>
            ))}
          </ul>
          <button role="tab" aria-selected={isNew} className={`det-add ${isNew ? "on" : ""}`} onClick={() => setDetId(NEW)}>
            <Icon name="plus" size={14} />
            New detector
          </button>
        </nav>

        <section className="studio-stage" aria-label="Frame">
          <div className="source-bar">
            <div className="seg" role="group" aria-label="Frame source">
              <button className={sourceKind === "video" ? "on" : ""} onClick={() => setSourceKind("video")} aria-pressed={sourceKind === "video"}>
                Recording
              </button>
              <button className={sourceKind === "image" ? "on" : ""} onClick={() => setSourceKind("image")} aria-pressed={sourceKind === "image"}>
                Image
              </button>
            </div>
            {sourceKind === "video" ? (
              gameMatches.length ? (
                <select className="input input-small" value={matchId} onChange={(e) => setMatchId(e.target.value)} aria-label="Recording">
                  {gameMatches.map((m) => (
                    <option key={m.id} value={m.id}>
                      {matchTitle(m)} ({timecode(m.durationMs)})
                    </option>
                  ))}
                </select>
              ) : (
                <span className="muted small">No recordings of this game yet.</span>
              )
            ) : (
              <>
                <button
                  className="btn btn-small"
                  onClick={async () => {
                    const p = await pickPath({ filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "bmp", "webp"] }], title: "Choose a screenshot" });
                    if (p) {
                      setImagePath(p);
                      loadFrame({ kind: "image", path: p });
                    }
                  }}
                >
                  Choose screenshot
                </button>
                {imagePath && <span className="muted small ellipsis">{imagePath}</span>}
              </>
            )}
          </div>

          <FrameCanvas frame={frame} loading={loadingFrame} box={shownBox} region={shownRegion} drafting={!!box} onBox={setBox} />

          <div className="stage-foot">
            {sourceKind === "video" && match ? (
              <div className="scrub">
                <input
                  type="range"
                  min={0}
                  max={match.durationMs}
                  step={200}
                  value={tMs}
                  onChange={(e) => setTMs(Number(e.target.value))}
                  onPointerUp={() => loadFrame({ kind: "video", path: match.path, tMs })}
                  onKeyUp={(e) => (e.key.startsWith("Arrow") ? loadFrame({ kind: "video", path: match.path, tMs }) : undefined)}
                  aria-label="Frame time"
                />
                <span className="num">{timecode(tMs, true)}</span>
                <button className="icon-btn" aria-label="Back one second" onClick={() => { const t = Math.max(0, tMs - 1000); setTMs(t); loadFrame({ kind: "video", path: match.path, tMs: t }); }}>
                  <Icon name="back" size={15} />
                </button>
                <button className="icon-btn flip" aria-label="Forward one second" onClick={() => { const t = Math.min(match.durationMs, tMs + 1000); setTMs(t); loadFrame({ kind: "video", path: match.path, tMs: t }); }}>
                  <Icon name="back" size={15} />
                </button>
              </div>
            ) : null}
            <p className="stage-help" title="The dashed outline is where Recap searches for it.">
              Drag a tight box around what only appears for this play.
            </p>
          </div>

          <section className="test" aria-label="Test">
            <div className="test-head">
              <h2>Test</h2>
              {test && (
                <p className="test-summary">
                  <span className="num">{hits.length}</span> {hits.length === 1 ? "detection" : "detections"}
                  {match ? ` in ${matchTitle(match)}` : ""}
                </p>
              )}
              <button className="btn btn-small" disabled={!matchId || !hasTemplates || testing} onClick={runTest} title={!hasTemplates ? "Save a reference image first" : "Run this detector over the whole recording"}>
                {testing ? "Scanning…" : test ? "Run again" : "Run on recording"}
              </button>
            </div>
            {!hasTemplates && <p className="muted small">Save a reference image first, then test it on a whole recording.</p>}
            {box && hasTemplates && <p className="muted small">Uses the saved reference images; your new box is not saved yet.</p>}
            {test && (
              <>
                <ScoreChart
                  samples={test.samples}
                  durationMs={test.durationMs}
                  threshold={draft.threshold}
                  hits={hits}
                  onPick={(t) => match && (setSourceKind("video"), setTMs(t), loadFrame({ kind: "video", path: match.path, tMs: t }))}
                />
                {hits.length > 0 && (
                  <ul className="hits" aria-label="Detections">
                    {hits.map((t) => (
                      <li key={t}>
                        <button className="hit" onClick={() => match && (setTMs(t), loadFrame({ kind: "video", path: match.path, tMs: t }))}>
                          <span className="num">{timecode(t)}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </>
            )}
          </section>
        </section>

        <aside className="studio-form" aria-label="Detector settings">
          <Field label="Name" htmlFor="st-label">
            <input id="st-label" className="input" value={draft.label} placeholder="Kill banner" onChange={(e) => set("label", e.target.value)} title={isNew ? `Saved as ${draft.id || slug(draft.label) || "…"}` : `Id ${draft.id}`} />
          </Field>
          <div className="field">
            <p className="field-label">Marks</p>
            {game && (
              <EventTypePicker
                game={game}
                value={draft.event}
                onChange={(id) => set("event", id)}
                onGameUpdated={(g) => setGames((gs) => (gs ?? []).map((x) => (x.id === g.id ? g : x)))}
              />
            )}
          </div>
          <Field label="Threshold" htmlFor="st-thr">
            <div className="range-row" title="How closely the frame must match. Higher means fewer false alarms but more misses.">
              <input id="st-thr" type="range" min={0.5} max={0.99} step={0.01} value={draft.threshold} onChange={(e) => set("threshold", Number(e.target.value))} />
              <span className="num range-val">{Math.round(draft.threshold * 100)}%</span>
            </div>
          </Field>

          <details className="advanced">
            <summary>Advanced</summary>
            <Field label="Search margin" htmlFor="st-margin" hint={box ? undefined : "Draw a box to change it."}>
              <div className="range-row" title="How far around your box to look, in case the element shifts a little.">
                <input id="st-margin" type="range" min={0} max={1} step={0.05} value={margin} onChange={(e) => setMargin(Number(e.target.value))} disabled={!box} />
                <span className="num range-val">{Math.round(margin * 100)}%</span>
              </div>
            </Field>
            <div className="field-row">
              <Field label="Cooldown (ms)" htmlFor="st-cd">
                <input id="st-cd" type="number" className="input" min={0} step={100} value={draft.cooldownMs} onChange={(e) => set("cooldownMs", Math.max(0, Number(e.target.value)))} title="Ignore repeats this soon after a hit." />
              </Field>
              <Field label="Frames in a row" htmlFor="st-mc">
                <input id="st-mc" type="number" className="input" min={1} max={10} value={draft.minConsecutive} onChange={(e) => set("minConsecutive", Math.max(1, Number(e.target.value)))} title="Matching samples needed before it counts." />
              </Field>
            </div>
            <div className="field-row">
              <Field label="Checks / second" htmlFor="st-fps">
                <input id="st-fps" type="number" className="input" min={1} max={30} step={1} value={draft.sampleFps} onChange={(e) => set("sampleFps", Math.min(30, Math.max(1, Number(e.target.value))))} title="Higher catches short flashes." />
              </Field>
              <Field label="Extra scales" htmlFor="st-sc">
                <input
                  id="st-sc"
                  className="input"
                  value={draft.scales.join(", ")}
                  placeholder="0.9, 1.1"
                  title="For HUD scale settings, e.g. 0.9, 1.1"
                  onChange={(e) =>
                    set(
                      "scales",
                      e.target.value
                        .split(/[,\s]+/)
                        .map(Number)
                        .filter((n) => n > 0.2 && n < 5),
                    )
                  }
                />
              </Field>
            </div>
            <div className="field-row">
              <Field label="Match on" htmlFor="st-method">
                <select
                  id="st-method"
                  className="input"
                  value={draft.method}
                  onChange={(e) => set("method", e.target.value as DetectorDef["method"])}
                  title="Outlines ignores what is behind the element (better for icons over gameplay). Test again after changing."
                >
                  <option value="ncc">Brightness</option>
                  <option value="edges">Outlines</option>
                </select>
              </Field>
              <Field label="Re-fire on pulse" htmlFor="st-rearm">
                <input
                  id="st-rearm"
                  type="number"
                  className="input"
                  min={0}
                  max={0.5}
                  step={0.05}
                  value={draft.rearmDrop ?? 0}
                  onChange={(e) => set("rearmDrop", Math.min(0.5, Math.max(0, Number(e.target.value))))}
                  title="If the element stays up and pulses once per event, count each pulse: the score drop that separates two events (try 0.1). 0 = off."
                />
              </Field>
            </div>
            <div className="field field-inline">
              <span className="field-label">Enabled</span>
              <Toggle checked={draft.enabled} onChange={(v) => set("enabled", v)} label="Detector enabled" />
            </div>
          </details>

          <div className="templates">
            <p className="field-label">Reference images</p>
            {existing?.templateUrls.length ? (
              <ul className="template-list">
                {existing.templateUrls.map((u) => (
                  <li key={u}>
                    <img src={fileSrc(u)} alt="Reference" />
                  </li>
                ))}
              </ul>
            ) : (
              <p className="muted small">None yet. Draw a box on a frame, then save.</p>
            )}
            {box && <p className="small accent-text">Saving adds the box as a reference image.</p>}
          </div>

          <div className="form-actions">
            <button className="btn btn-primary" onClick={save} disabled={saving || !game}>
              {saving ? "Saving…" : isNew ? "Save detector" : "Save changes"}
            </button>
            {!isNew && existing && (
              <button className="btn btn-ghost btn-danger-text" onClick={() => setConfirmDelete(true)}>
                Delete
              </button>
            )}
          </div>
        </aside>
      </div>

      {confirmDelete && existing && (
        <Confirm
          title="Delete detector"
          danger
          confirmLabel="Delete detector"
          body={
            <p>
              <strong>{existing.label || existing.id}</strong> will stop marking {kindStyle(existing.event).plural.toLowerCase()} in {game?.name}. Markers it already made stay.
            </p>
          }
          onClose={() => setConfirmDelete(false)}
          onConfirm={remove}
        />
      )}
    </div>
  );
}

function FrameCanvas({
  frame,
  loading,
  box,
  region,
  drafting,
  onBox,
}: {
  frame: FrameResult | null;
  loading: boolean;
  box: Region | null;
  region: Region;
  drafting: boolean;
  onBox: (r: Region | null) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const start = useRef<{ x: number; y: number } | null>(null);
  const [live, setLive] = useState<Region | null>(null);

  const frac = (e: ReactPointerEvent) => {
    const r = ref.current!.getBoundingClientRect();
    return { x: Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)), y: Math.max(0, Math.min(1, (e.clientY - r.top) / r.height)) };
  };
  const rect = (a: { x: number; y: number }, b: { x: number; y: number }): Region => ({
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  });
  const pos = (r: Region) => ({ left: `${r.x * 100}%`, top: `${r.y * 100}%`, width: `${r.w * 100}%`, height: `${r.h * 100}%` });

  if (!frame)
    return (
      <div className="frame frame-empty" aria-busy={loading}>
        <p>{loading ? "Loading frame…" : "Pick a recording or a screenshot to show a frame here."}</p>
      </div>
    );

  const shown = live ?? box;
  return (
    <div
      ref={ref}
      className={`frame ${loading ? "is-loading" : ""}`}
      style={{ aspectRatio: `${frame.width} / ${frame.height}` }}
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        start.current = frac(e);
        setLive(null);
      }}
      onPointerMove={(e) => start.current && setLive(rect(start.current, frac(e)))}
      onPointerUp={(e) => {
        if (!start.current) return;
        const r = rect(start.current, frac(e));
        start.current = null;
        setLive(null);
        if (r.w > 0.006 && r.h > 0.006) onBox(r);
      }}
      role="img"
      aria-label="Frame. Drag to draw a box around the element to detect."
    >
      <img src={frame.dataUrl} alt="" draggable={false} />
      {!live && <div className="frame-region" style={pos(region)} aria-hidden />}
      {shown && <div className={`frame-box ${drafting || live ? "is-new" : ""}`} style={pos(shown)} aria-hidden />}
      <span className="frame-res num">
        {frame.width}×{frame.height}
      </span>
    </div>
  );
}
