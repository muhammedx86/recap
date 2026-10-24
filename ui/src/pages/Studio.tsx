import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { api, fileSrc, on, pickPath } from "../lib/api";
import { timecode } from "../lib/format";
import { useLoad } from "../lib/hooks";
import { kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { DetectorDef, FrameSource, GameDetector, GameEvent, GameInfo, MatchRecord, Region, TestHit, TestResult } from "../lib/types";
import { computeHits } from "../lib/hits";
import { Confirm, Empty, Field, Icon, KindDot, Toggle } from "../components/ui";
import { EventTypePicker } from "../components/EventTypePicker";
import { matchTitle } from "./Library";
import { NEW_READER, useReaderWorkspace } from "./StudioReaders";
import { StudioViewer, type Marker, type Media, type Overlay, type ViewerHandle } from "./StudioViewer";

const NEW = "__new__";

/** Sensible timing for each kind of play: banners for kills flash briefly, round screens linger. */
const EVENT_DEFAULTS: Record<string, Pick<DetectorDef, "cooldownMs" | "minConsecutive" | "sampleFps">> = {
  kill: { cooldownMs: 1500, minConsecutive: 2, sampleFps: 6 },
  assist: { cooldownMs: 1500, minConsecutive: 2, sampleFps: 6 },
  death: { cooldownMs: 4000, minConsecutive: 2, sampleFps: 4 },
  multikill: { cooldownMs: 3000, minConsecutive: 2, sampleFps: 4 },
  ace: { cooldownMs: 10000, minConsecutive: 2, sampleFps: 4 },
  objective: { cooldownMs: 5000, minConsecutive: 2, sampleFps: 3 },
  bomb_plant: { cooldownMs: 10000, minConsecutive: 2, sampleFps: 3 },
  bomb_defuse: { cooldownMs: 10000, minConsecutive: 2, sampleFps: 3 },
  mvp: { cooldownMs: 20000, minConsecutive: 3, sampleFps: 2 },
  round_start: { cooldownMs: 20000, minConsecutive: 3, sampleFps: 2 },
  round_win: { cooldownMs: 20000, minConsecutive: 3, sampleFps: 2 },
  round_loss: { cooldownMs: 20000, minConsecutive: 3, sampleFps: 2 },
  match_start: { cooldownMs: 60000, minConsecutive: 3, sampleFps: 1 },
  match_end: { cooldownMs: 60000, minConsecutive: 3, sampleFps: 1 },
};
const defaultsFor = (event: string) => EVENT_DEFAULTS[event] ?? { cooldownMs: 2000, minConsecutive: 2, sampleFps: 4 };

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
    ...defaultsFor("kill"),
    scales: [],
    enabled: true,
  };
}

function defOf(d: GameDetector): DetectorDef {
  const { ready: _r, user: _u, templateUrls: _t, ...def } = d;
  return def;
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

/** Size of a box on the game picture, and a warning when it is likely to work badly. */
function boxAdvice(b: Region, pic: { width: number; height: number } | null): { size: string; warn?: string } | null {
  if (!pic) return null;
  const w = Math.round(b.w * pic.width);
  const h = Math.round(b.h * pic.height);
  const size = `${w}×${h} px`;
  if (w < 12 || h < 12) return { size, warn: "Very small. Include the whole icon or a few letters, or it can match by chance." };
  if (b.w * b.h > 0.05 || b.w > 0.35 || b.h > 0.35)
    return { size, warn: "Large. Whatever changes behind it lowers the match. Box only the part that always looks the same." };
  return { size };
}

const pct = (v: number) => `${Math.round(v * 100)}%`;

export function Studio() {
  const toast = useToast();
  const [params] = useSearchParams();
  const { data: games, setData: setGames, error: gamesError } = useLoad(() => api.listGames(), []);
  const { data: matches } = useLoad(() => api.listMatches(), []);
  const [gameId, setGameId] = useState<string>(params.get("game") ?? "");
  const [detId, setDetId] = useState<string>(params.get("detector") ?? NEW);
  const [draft, setDraft] = useState<DetectorDef>(blankDetector());
  const [box, setBox] = useState<Region | null>(null);
  /** The frame the new box was drawn on: its reference is cut from there. */
  const [boxAt, setBoxAt] = useState<FrameSource | null>(null);
  const [margin, setMargin] = useState(0.25);
  const [sourceKind, setSourceKind] = useState<"video" | "image">("video");
  const [matchId, setMatchId] = useState<string>("");
  const [imagePath, setImagePath] = useState<string | null>(null);
  const [picture, setPicture] = useState<{ width: number; height: number } | null>(null);
  const [now, setNow] = useState(0);
  const [matchEvents, setMatchEvents] = useState<GameEvent[]>([]);
  const [test, setTest] = useState<TestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [removeRef, setRemoveRef] = useState<number | null>(null);
  const [mode, setMode] = useState<"detectors" | "details">(params.get("mode") === "details" ? "details" : "detectors");
  const [readerId, setReaderId] = useState<string>(params.get("reader") ?? NEW_READER);
  const viewer = useRef<ViewerHandle>(null);

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

  // Events already in the recording: jump targets and timeline marks.
  useEffect(() => {
    setMatchEvents([]);
    setTest(null);
    if (!matchId) return;
    let stale = false;
    api.getMatch(matchId).then(
      (d) => !stale && setMatchEvents(d.events),
      () => {},
    );
    return () => {
      stale = true;
    };
  }, [matchId]);

  // Load the selected detector into the form.
  useEffect(() => {
    setTest(null);
    setBox(null);
    setDraft(existing ? defOf(existing) : blankDetector());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [gameId, detId, existing?.id]);

  const media: Media | null = useMemo(
    () =>
      sourceKind === "video"
        ? match
          ? { kind: "video", path: match.path, durationMs: match.durationMs }
          : null
        : imagePath
          ? { kind: "image", path: imagePath }
          : null,
    [sourceKind, match?.path, match?.durationMs, imagePath],
  );

  const showFrameAt = useCallback(
    (t: number) => {
      setSourceKind("video");
      viewer.current?.seek(t);
    },
    [],
  );

  const viewerApi = useMemo(
    () => ({
      ready: !!picture,
      source: () => viewer.current?.source() ?? null,
      sampleColour: (p: { x: number; y: number }) => viewer.current!.sampleColour(p),
    }),
    [picture],
  );
  const rw = useReaderWorkspace({ game, setGames, viewer: viewerApi, match, matchEvents, readerId, setReaderId, toast, showFrameAt });

  // Details are usually read from the end screen: entering Details mode jumps there.
  useEffect(() => {
    if (mode === "details" && rw.when === "end" && sourceKind === "video" && match) showFrameAt(Math.max(0, match.durationMs - 40_000));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, match?.id]);

  const set = <K extends keyof DetectorDef>(k: K, v: DetectorDef[K]) => setDraft((d) => ({ ...d, [k]: v }));
  const isNew = detId === NEW;
  const hasTemplates = draft.templates.length > 0;
  const kind = kindStyle(draft.event);

  const hits = useMemo(
    () => (test ? computeHits(test.samples, draft.threshold, draft.minConsecutive, draft.cooldownMs, draft.rearmDrop ?? 0, draft.holdMargin ?? 0) : []),
    [test, draft.threshold, draft.minConsecutive, draft.cooldownMs, draft.rearmDrop, draft.holdMargin],
  );
  const sameKind = useMemo(() => matchEvents.filter((e) => e.kind === draft.event).map((e) => e.tMs).sort((a, b) => a - b), [matchEvents, draft.event]);
  // How many of the plays already marked in this recording the test found (within 2 s).
  const missed = useMemo(() => (test ? sameKind.filter((t) => !hits.some((h) => Math.abs(h - t) <= 2000)) : []), [test, sameKind, hits]);
  const found = sameKind.length - missed.length;
  // Where it matched, when the shown moment is on a detection.
  const nearHit: TestHit | null = useMemo(() => {
    if (!test?.hits.length || sourceKind !== "video") return null;
    let best: TestHit | null = null;
    for (const h of test.hits) if (now >= h.tMs - 200 && now <= h.tMs + 3000 && (!best || Math.abs(h.tMs - now) < Math.abs(best.tMs - now))) best = h;
    return best;
  }, [test, now, sourceKind]);

  const nameForSave = () => {
    const label = draft.label.trim() || kind.label;
    let id = draft.id || slug(label);
    if (isNew && game) {
      const taken = new Set(game.detectors.map((d) => d.id));
      for (let i = 2; taken.has(id); i++) id = `${slug(label)}-${i}`;
    }
    return { id, label };
  };

  const applySaved = (updated: GameInfo, id: string) => {
    setGames((gs) => (gs ?? []).map((g) => (g.id === updated.id ? updated : g)));
    const d = updated.detectors.find((x) => x.id === id);
    if (d) setDraft(defOf(d));
    setDetId(id);
  };

  const save = async (withBox: "new" | "same" | "none") => {
    if (!game) return;
    const { id, label } = nameForSave();
    if (!id) return toast.error("Give the detector a name first.");
    const cut = withBox === "new" ? box : withBox === "same" ? (draft.templateBox ?? null) : null;
    const source = withBox === "new" ? boxAt : withBox === "same" ? viewer.current?.source() : null;
    if (cut && !source) return toast.error("Show a frame first.");
    const det: DetectorDef = {
      ...draft,
      id,
      label,
      region: withBox === "new" && box ? expand(box, margin) : draft.region,
      templateBox: cut ?? draft.templateBox,
    };
    setSaving(true);
    try {
      const updated = await api.studioSaveDetector(game.id, det, source ?? { kind: "image", path: "" }, cut);
      applySaved(updated, id);
      setBox(null);
      setBoxAt(null);
      toast.info(withBox === "same" ? "Reference added" : withBox === "new" ? (isNew ? "Detector saved. Now test it." : "New reference saved") : "Detector saved");
    } catch (e) {
      toast.error(e, "Could not save the detector");
    } finally {
      setSaving(false);
    }
  };

  const dropReference = async (i: number) => {
    if (!game || !existing) return;
    const def = { ...defOf(existing), templates: existing.templates.filter((_, j) => j !== i) };
    try {
      const updated = await api.studioSaveDetector(game.id, def, { kind: "image", path: "" }, null);
      // Keep unsaved form edits; only the reference list changes.
      setGames((gs) => (gs ?? []).map((g) => (g.id === updated.id ? updated : g)));
      setDraft((d) => ({ ...d, templates: def.templates }));
      setTest(null);
      toast.info("Reference removed");
    } catch (e) {
      toast.error(e, "Could not remove the reference");
    }
  };

  const runTest = async () => {
    if (!game || !matchId) return;
    setTesting(true);
    setProgress(0);
    const stop = on("studio-progress", (p) => p.task === "test" && setProgress(p.progress));
    try {
      const res = await api.studioTestDetector(game.id, { ...draft, id: draft.id || slug(draft.label) || "draft" }, matchId);
      setTest(res);
      if (res.cancelled) toast.info("Test stopped: results cover part of the recording");
    } catch (e) {
      toast.error(e, "Test failed");
    } finally {
      stop();
      setTesting(false);
      setProgress(null);
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

  const onBoxDrawn = (r: Region) => {
    setBox(r);
    setBoxAt(viewer.current?.source() ?? null);
  };

  if (gamesError) return <div className="page"><Empty title="Games could not be loaded">{gamesError}</Empty></div>;

  // ---- what the viewer shows ----
  const markers: Marker[] = matchEvents.map((e) => ({ tMs: e.tMs, color: kindStyle(e.kind).color, title: kindStyle(e.kind).label }));
  const detectorOverlays: Overlay[] = nearHit
    ? [{ region: nearHit.region, className: "frame-match", label: `${pct(nearHit.score)} match` }]
    : [];
  // On a detection, the green match box replaces the saved box.
  const shownBox = box ?? (nearHit ? null : (draft.templateBox ?? null));
  const shownRegion = box ? expand(box, margin) : hasTemplates ? draft.region : null;
  const advice = box ? boxAdvice(box, picture) : null;
  const boxMoved = box && boxAt?.kind === "video" && sourceKind === "video" && Math.abs(boxAt.tMs - now) > 250;

  // ---- the three steps ----
  const step = box ? 2 : !hasTemplates ? 1 : 3;
  const steps = [
    { n: 1, text: `Find a moment with a ${kind.label.toLowerCase()} on screen`, done: !!box || hasTemplates },
    { n: 2, text: "Draw a tight box around it", done: hasTemplates && !box },
    { n: 3, text: "Test on the whole recording", done: !!test },
  ];

  const jumpChips = (times: number[], noun: string, lead = 300, max = 14) =>
    times.length ? (
      <div className="jumps">
        <span className="muted small">{noun}</span>
        <ul>
          {times.slice(0, max).map((t) => (
            <li key={t}>
              <button className={`hit ${Math.abs(now - t - lead) < 400 ? "on" : ""}`} onClick={() => showFrameAt(t + lead)}>
                <span className="num">{timecode(t)}</span>
              </button>
            </li>
          ))}
          {times.length > max && <li className="muted small">+{times.length - max} more on the timeline</li>}
        </ul>
      </div>
    ) : null;

  return (
    <div className="page page-studio">
      <header className="page-head">
        <h1>Detector Studio</h1>
      </header>

      <div className="studio">
        <nav className="studio-rail" aria-label="Detectors">
          <select id="st-game" className="input" value={gameId} onChange={(e) => (setGameId(e.target.value), setDetId(NEW), setReaderId(NEW_READER))} aria-label="Game">
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
          <div className="seg rail-mode" role="tablist" aria-label="What to edit">
            <button role="tab" aria-selected={mode === "detectors"} className={mode === "detectors" ? "on" : ""} onClick={() => setMode("detectors")} title="Spot plays such as kills on screen">
              Detectors
            </button>
            <button role="tab" aria-selected={mode === "details"} className={mode === "details" ? "on" : ""} onClick={() => setMode("details")} title="Readers that fill in match details such as the map or your K/D/A">
              Details
            </button>
          </div>
          {mode === "details" ? (
            rw.rail
          ) : (
            <>
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
            </>
          )}
        </nav>

        <section className="studio-stage" aria-label="Frame">
          <div className="source-bar">
            <div className="seg" role="group" aria-label="Frame source">
              <button className={sourceKind === "video" ? "on" : ""} onClick={() => setSourceKind("video")} aria-pressed={sourceKind === "video"}>
                Recording
              </button>
              <button className={sourceKind === "image" ? "on" : ""} onClick={() => setSourceKind("image")} aria-pressed={sourceKind === "image"}>
                Screenshot
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
                <span className="muted small">No recordings of this game yet. Use a screenshot instead.</span>
              )
            ) : (
              <>
                <button
                  className="btn btn-small"
                  onClick={async () => {
                    const p = await pickPath({ filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "bmp", "webp"] }], title: "Choose a screenshot" });
                    if (p) setImagePath(p);
                  }}
                >
                  Choose screenshot
                </button>
                {imagePath && <span className="muted small ellipsis">{imagePath}</span>}
              </>
            )}
          </div>

          {mode === "detectors" && (
            <ol className="steps" aria-label="Steps">
              {steps.map((s) => (
                <li key={s.n} className={`${s.n === step ? "on" : ""} ${s.done ? "done" : ""}`} aria-current={s.n === step ? "step" : undefined}>
                  <span className="step-n">{s.done ? "✓" : s.n}</span>
                  {s.text}
                </li>
              ))}
            </ol>
          )}

          <StudioViewer
            ref={viewer}
            media={media}
            onTime={setNow}
            onInfo={setPicture}
            markers={mode === "details" ? [...markers, ...rw.markers] : markers}
            {...(mode === "details"
              ? {
                  box: rw.canvas.box,
                  region: rw.canvas.region,
                  overlays: rw.canvas.overlays,
                  drafting: rw.canvas.drafting,
                  onBox: rw.canvas.onBox,
                  onPoint: rw.canvas.onPoint,
                  jumps: rw.jumps,
                }
              : {
                  box: shownBox,
                  region: shownRegion,
                  overlays: detectorOverlays,
                  drafting: !!box,
                  onBox: onBoxDrawn,
                  onEscape: () => (setBox(null), setBoxAt(null)),
                  detections: hits,
                  samples: test?.samples,
                  threshold: test ? draft.threshold : undefined,
                  progress,
                  jumps: hits.length ? { times: hits, noun: "detection" } : sameKind.length ? { times: sameKind, noun: kind.label.toLowerCase() } : undefined,
                })}
            emptyText={sourceKind === "video" ? "No recording of this game yet. Choose a screenshot instead." : "Choose a screenshot to show it here."}
          />

          {mode === "details" ? (
            <div className="stage-bar">
              <p className="stage-help">{rw.canvas.hint}</p>
              {rw.stageActions}
            </div>
          ) : box ? (
            <div className="stage-bar box-bar">
              <p className={`stage-help ${advice?.warn ? "warn-text" : ""}`}>
                {advice && <span className="num">{advice.size}</span>}
                {advice?.warn ? ` · ${advice.warn}` : " · Drag the box to move it, its corners to resize."}
                {boxMoved && boxAt?.kind === "video" && (
                  <>
                    {" "}
                    Drawn at{" "}
                    <button className="link-btn num" onClick={() => showFrameAt(boxAt.tMs)}>
                      {timecode(boxAt.tMs, true)}
                    </button>
                    ; the reference is cut there.
                  </>
                )}
              </p>
              <div className="stage-actions">
                <button className="btn btn-ghost btn-small" onClick={() => (setBox(null), setBoxAt(null))}>
                  Cancel
                </button>
                <button className="btn btn-primary btn-small" onClick={() => save("new")} disabled={saving || !game}>
                  {saving ? "Saving…" : isNew ? "Save detector" : "Save as new reference"}
                </button>
              </div>
            </div>
          ) : (
            <div className="stage-bar">
              {step === 1 ? (
                <p className="stage-help">
                  {sameKind.length
                    ? `Jump to a ${kind.label.toLowerCase()} below, or scrub the timeline. Then drag a box around what only appears for it, like a banner or icon.`
                    : `Scrub to a ${kind.label.toLowerCase()}, then drag a box around what only appears for it, like a banner or icon.`}
                </p>
              ) : (
                <p className="stage-help">Drag on the frame to draw a new box. Space plays, , and . step one frame.</p>
              )}
              {!isNew && draft.templateBox && (
                <button className="btn btn-small" onClick={() => save("same")} disabled={saving || !picture} title="Cut the same box from the frame shown now">
                  Add this frame as a reference
                </button>
              )}
            </div>
          )}
          {mode === "detectors" && sourceKind === "video" && !box && !test && jumpChips(sameKind, `${kind.plural} already marked`)}

          {mode === "details" ? (
            rw.testPanel
          ) : (
            <section className="test" aria-label="Test">
              <div className="test-head">
                <h2>Test</h2>
                {test && !testing && (
                  <p className="test-summary">
                    <span className="num">{hits.length}</span> {hits.length === 1 ? "detection" : "detections"}
                    {sameKind.length > 0 && (
                      <>
                        {" · found "}
                        <span className="num">{found}</span> of {sameKind.length} {kind.plural.toLowerCase()} already marked
                      </>
                    )}
                    {test.cancelled && " · stopped early"}
                  </p>
                )}
                <div className="test-actions">
                  {testing ? (
                    <button className="btn btn-small" onClick={() => api.studioCancel()}>
                      Stop
                    </button>
                  ) : (
                    <button
                      className={`btn btn-small ${step === 3 && !test ? "btn-primary" : ""}`}
                      disabled={!matchId || !hasTemplates}
                      onClick={runTest}
                      title={!hasTemplates ? "Save a reference image first" : "Run this detector over the whole recording"}
                    >
                      {test ? "Run again" : "Test on recording"}
                    </button>
                  )}
                </div>
              </div>
              {testing && (
                <div className="test-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round((progress ?? 0) * 100)}>
                  <div style={{ width: pct(progress ?? 0) }} />
                  <span className="muted small num">Scanning… {pct(progress ?? 0)}</span>
                </div>
              )}
              {!testing && !hasTemplates && <p className="muted small">Save a reference first, then run it over a whole recording to see what it finds.</p>}
              {!testing && hasTemplates && !test && <p className="muted small">Runs over the whole recording. Results appear on the timeline above; click one to check it.</p>}
              {box && hasTemplates && !testing && <p className="muted small">Tests use the saved references; save the new box to include it.</p>}
              {test && !testing && (
                <>
                  {nearHit ? (
                    <p className="small match-note">
                      At <span className="num">{timecode(nearHit.tMs, true)}</span> it matched reference {nearHit.reference + 1}
                      {Math.abs(nearHit.scale - 1) > 0.01 ? ` at ${pct(nearHit.scale)} size` : ""}, {pct(nearHit.score)} alike. The green box shows where.
                    </p>
                  ) : (
                    <p className="muted small">
                      Click a detection to check it. Move “Match needed” to trade misses for false alarms; results update as you drag.
                    </p>
                  )}
                  {missed.length > 0 && jumpChips(missed, `Missed ${kind.plural.toLowerCase()}`)}
                  {jumpChips(hits, "Detections", 0, 60)}
                </>
              )}
            </section>
          )}
        </section>

        {mode === "details" ? (
          <aside className="studio-form" aria-label="Reader settings">
            {rw.form}
          </aside>
        ) : (
          <aside className="studio-form" aria-label="Detector settings">
            <Field label="Name" htmlFor="st-label">
              <input
                id="st-label"
                className="input"
                value={draft.label}
                placeholder={kind.label}
                onChange={(e) => set("label", e.target.value)}
                title={isNew ? `Saved as ${nameForSave().id || "…"}` : `Id ${draft.id}`}
              />
            </Field>
            <div className="field">
              <p className="field-label">Marks</p>
              {game && (
                <EventTypePicker
                  game={game}
                  value={draft.event}
                  onChange={(id) => setDraft((d) => ({ ...d, event: id, ...(isNew ? defaultsFor(id) : {}) }))}
                  onGameUpdated={(g) => setGames((gs) => (gs ?? []).map((x) => (x.id === g.id ? g : x)))}
                />
              )}
            </div>
            <Field label="Match needed" htmlFor="st-thr">
              <div className="range-row" title="How alike the screen must look to the reference. Higher: fewer false alarms, more misses.">
                <input id="st-thr" type="range" min={0.5} max={0.99} step={0.01} value={draft.threshold} onChange={(e) => set("threshold", Number(e.target.value))} />
                <span className="num range-val">{pct(draft.threshold)}</span>
              </div>
            </Field>

            <div className="templates">
              <p className="field-label">
                References <span className="muted num">{existing?.templateUrls.length ?? 0}</span>
              </p>
              {existing?.templateUrls.length ? (
                <ul className="template-list">
                  {existing.templateUrls.map((u, i) => (
                    <li key={u} className={nearHit?.reference === i ? "is-matched" : ""} title={nearHit?.reference === i ? "Matched here" : `Reference ${i + 1}`}>
                      <img src={fileSrc(u)} alt={`Reference ${i + 1}`} />
                      <button className="ref-remove" aria-label={`Remove reference ${i + 1}`} onClick={() => setRemoveRef(i)}>
                        <Icon name="close" size={11} />
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="muted small">None yet. Draw a box on the frame and save it.</p>
              )}
              {!isNew && draft.templateBox && (
                <p className="muted small ref-help">
                  {draft.templates.length >= 2
                    ? `Recap compares the ${draft.templates.length} references and ignores what differs between them, like the background.`
                    : "Add 2–3 references from different scenes (other maps, lighting) so the background behind it is ignored."}{" "}
                  Show another {kind.label.toLowerCase()}, then use “Add this frame as a reference”.
                </p>
              )}
            </div>

            <details className="advanced">
              <summary>More settings</summary>
              <Field label="Search area around the box" htmlFor="st-margin" hint={box ? undefined : "Draw a box to change it."}>
                <div className="range-row" title="How far around your box to look, in case it moves a little.">
                  <input id="st-margin" type="range" min={0} max={1} step={0.05} value={margin} onChange={(e) => setMargin(Number(e.target.value))} disabled={!box} />
                  <span className="num range-val">{pct(margin)}</span>
                </div>
              </Field>
              <div className="field-row">
                <Field label="Ignore repeats for (s)" htmlFor="st-cd">
                  <input
                    id="st-cd"
                    type="number"
                    className="input"
                    min={0}
                    step={0.5}
                    value={draft.cooldownMs / 1000}
                    onChange={(e) => set("cooldownMs", Math.max(0, Math.round(Number(e.target.value) * 1000)))}
                    title="After a detection, ignore the same thing for this long."
                  />
                </Field>
                <Field label="Checks in a row" htmlFor="st-mc">
                  <input id="st-mc" type="number" className="input" min={1} max={10} value={draft.minConsecutive} onChange={(e) => set("minConsecutive", Math.max(1, Number(e.target.value)))} title="How many checks in a row must match before it counts." />
                </Field>
              </div>
              <div className="field-row">
                <Field label="Checks per second" htmlFor="st-fps">
                  <input id="st-fps" type="number" className="input" min={1} max={30} step={1} value={draft.sampleFps} onChange={(e) => set("sampleFps", Math.min(30, Math.max(1, Number(e.target.value))))} title="Higher catches short flashes but scans slower." />
                </Field>
                <Field label="Also try sizes (%)" htmlFor="st-sc">
                  <input
                    id="st-sc"
                    className="input"
                    defaultValue={draft.scales.map((s) => Math.round(s * 100)).join(", ")}
                    key={`${draft.id}-${draft.scales.join(",")}`}
                    placeholder="90, 110"
                    title="For players who change the HUD size, e.g. 90, 110"
                    onBlur={(e) =>
                      set(
                        "scales",
                        e.target.value
                          .split(/[,\s]+/)
                          .map((n) => Number(n) / 100)
                          .filter((n) => n > 0.2 && n < 5),
                      )
                    }
                  />
                </Field>
              </div>
              <div className="field-row">
                <Field label="Compare" htmlFor="st-method">
                  <select
                    id="st-method"
                    className="input"
                    value={draft.method}
                    onChange={(e) => set("method", e.target.value as DetectorDef["method"])}
                    title="Outlines ignores colours and brightness behind the element (better for icons over gameplay). Test again after changing."
                  >
                    <option value="ncc">Pixels</option>
                    <option value="edges">Outlines</option>
                  </select>
                </Field>
                <Field label="Count pulses" htmlFor="st-rearm">
                  <input
                    id="st-rearm"
                    type="number"
                    className="input"
                    min={0}
                    max={0.5}
                    step={0.05}
                    value={draft.rearmDrop ?? 0}
                    onChange={(e) => set("rearmDrop", Math.min(0.5, Math.max(0, Number(e.target.value))))}
                    title="For an element that stays up and pulses once per play: how much the match must dip between two plays (try 0.1). 0 = off."
                  />
                </Field>
                <Field label="Steady margin" htmlFor="st-hold">
                  <input
                    id="st-hold"
                    type="number"
                    className="input"
                    min={0}
                    max={0.5}
                    step={0.05}
                    value={draft.holdMargin ?? 0}
                    onChange={(e) => set("holdMargin", Math.min(0.5, Math.max(0, Number(e.target.value))))}
                    title="Once found, keep treating the element as on screen until the match drops this far below 'Match needed', so a flickering match isn't counted twice (try 0.15). 0 = off."
                  />
                </Field>
              </div>
              <div className="field field-inline">
                <span className="field-label">Enabled</span>
                <Toggle checked={draft.enabled} onChange={(v) => set("enabled", v)} label="Detector enabled" />
              </div>
            </details>

            <div className="form-actions">
              <button className="btn btn-primary" onClick={() => save(box ? "new" : "none")} disabled={saving || !game || (isNew && !box)} title={isNew && !box ? "Draw a box on the frame first" : undefined}>
                {saving ? "Saving…" : isNew ? "Save detector" : "Save changes"}
              </button>
              {!isNew && existing && (
                <button className="btn btn-ghost btn-danger-text" onClick={() => setConfirmDelete(true)}>
                  Delete
                </button>
              )}
            </div>
          </aside>
        )}
      </div>

      {rw.dialogs}
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
      {removeRef !== null && existing && (
        <Confirm
          title="Remove reference"
          danger
          confirmLabel="Remove"
          body={
            <p>
              Reference {removeRef + 1} will no longer be used by <strong>{existing.label || existing.id}</strong>.
              {existing.templates.length === 1 ? " It is the only one: the detector stops working until you add another." : ""}
            </p>
          }
          onClose={() => setRemoveRef(null)}
          onConfirm={() => dropReference(removeRef)}
        />
      )}
    </div>
  );
}
