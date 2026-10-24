// Detector Studio, Details mode: screen readers that fill match details (map, character, K/D/A…).
// Studio owns the frame and source; this hook supplies the rail list, canvas overlays, the test
// panel and the settings form.

import { useEffect, useState, type ReactNode } from "react";
import { api, fileSrc } from "../lib/api";
import { timecode } from "../lib/format";
import { KDA_PATTERN, READER_FIELDS, detailLabel, displayValue, sourceText } from "../lib/details";
import type { FrameReadResult, FrameResult, FrameSource, GameInfo, MatchRecord, ReaderDef, ReaderTestResult, Region } from "../lib/types";
import { Confirm, Field, Icon, Toggle } from "../components/ui";

export const NEW_READER = "__new_reader__";

type Target = "region" | "slot2" | "option" | "pick";

export type CanvasProps = {
  box: Region | null;
  region: Region | null;
  overlays: Array<{ region: Region; className: string }>;
  drafting: boolean;
  onBox: (r: Region | null) => void;
  onPoint?: (p: { x: number; y: number }) => void;
  hint: string;
};

function blankReader(): ReaderDef {
  return {
    id: "",
    label: "",
    field: "map",
    kind: "text",
    region: { x: 0.04, y: 0.01, w: 0.1, h: 0.025 },
    when: "end",
    sampleFps: 1,
    values: [],
    options: [],
    threshold: 0.75,
    method: "ncc",
    templateFrameHeight: 1080,
    enabled: true,
  };
}

const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 40);

function slotRegion(r: ReaderDef, i: number): Region {
  const dx = (r.slots?.stepX ?? 0) * i;
  const dy = (r.slots?.stepY ?? 0) * i;
  return { ...r.region, x: Math.min(Math.max(0, r.region.x + dx), 1 - r.region.w), y: Math.min(Math.max(0, r.region.y + dy), 1 - r.region.h) };
}

function grow(b: Region, m: number): Region {
  const x = Math.max(0, b.x - b.w * m);
  const y = Math.max(0, b.y - b.h * m);
  return { x, y, w: Math.min(1, b.x + b.w * (1 + m)) - x, h: Math.min(1, b.y + b.h * (1 + m)) - y };
}

/** Average colour of a 5×5 patch of the frame at (x, y) fractions, as #rrggbb. */
async function sampleColour(frame: FrameResult, p: { x: number; y: number }): Promise<string> {
  const img = new Image();
  img.src = frame.dataUrl;
  await img.decode();
  const c = document.createElement("canvas");
  c.width = frame.width;
  c.height = frame.height;
  const ctx = c.getContext("2d")!;
  ctx.drawImage(img, 0, 0, frame.width, frame.height);
  const cx = Math.round(p.x * frame.width);
  const cy = Math.round(p.y * frame.height);
  const data = ctx.getImageData(Math.max(0, cx - 2), Math.max(0, cy - 2), 5, 5).data;
  const sum = [0, 0, 0];
  for (let i = 0; i < data.length; i += 4) for (let k = 0; k < 3; k++) sum[k] += data[i + k];
  const n = data.length / 4;
  return "#" + sum.map((v) => Math.round(v / n).toString(16).padStart(2, "0")).join("");
}

const NUMERIC = ["kda", "kills", "deaths", "assists", "team_score", "enemy_score"];

const WHEN_LABEL: Record<ReaderDef["when"], string> = { end: "End of match", start: "Start of match", any: "Throughout" };

export function useReaderWorkspace(o: {
  game: GameInfo | undefined;
  setGames: (f: (gs: GameInfo[] | null) => GameInfo[] | null) => void;
  frame: FrameResult | null;
  frameSource: FrameSource | null;
  match: MatchRecord | undefined;
  readerId: string;
  setReaderId: (id: string) => void;
  toast: { error: (e: unknown, title?: string) => void; info: (m: string) => void };
  showFrameAt: (tMs: number) => void;
}) {
  const { game, frame, frameSource, match, readerId, setReaderId, toast } = o;
  const existing = game?.readers.find((r) => r.id === readerId);
  const isNew = readerId === NEW_READER;
  const [draft, setDraft] = useState<ReaderDef>(blankReader());
  const [drawn, setDrawn] = useState(false);
  const [target, setTarget] = useState<Target>("region");
  const [optionValue, setOptionValue] = useState("");
  const [crops, setCrops] = useState<Array<{ value: string; region: Region }>>([]);
  const [customField, setCustomField] = useState(false);
  const [read, setRead] = useState<FrameReadResult | null>(null);
  const [test, setTest] = useState<ReaderTestResult | null>(null);
  const [reading, setReading] = useState(false);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  useEffect(() => {
    setRead(null);
    setTest(null);
    setCrops([]);
    setTarget("region");
    setDrawn(false);
    if (existing) {
      const { ready: _r, user: _u, optionUrls: _o, ...def } = existing;
      setDraft(def);
      setCustomField(!READER_FIELDS.some((f) => f.id === def.field));
    } else {
      setDraft(blankReader());
      setCustomField(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [game?.id, readerId, existing?.id]);

  const set = <K extends keyof ReaderDef>(k: K, v: ReaderDef[K]) => setDraft((d) => ({ ...d, [k]: v }));

  const setField = (f: string) => {
    if (f === "__custom") {
      setCustomField(true);
      set("field", "");
      return;
    }
    setCustomField(false);
    setDraft((d) => ({
      ...d,
      field: f,
      // K/D/A from one box: three numbers into kills, deaths and assists.
      pattern: f === "kda" ? KDA_PATTERN : d.pattern === KDA_PATTERN ? undefined : d.pattern,
      label: d.label || (isNew ? "" : d.label),
    }));
  };

  const onBox = (r: Region | null) => {
    if (!r) return;
    if (target === "slot2") {
      setDraft((d) => ({ ...d, slots: { count: Math.max(2, d.slots?.count ?? 5), stepX: +(r.x - d.region.x).toFixed(4), stepY: +(r.y - d.region.y).toFixed(4) } }));
      setTarget("region");
      return;
    }
    if (target === "option") {
      const value = optionValue.trim();
      if (!value) {
        toast.error("Type the value this picture means first, e.g. the agent's name.");
        return;
      }
      setCrops((c) => [...c, { value, region: r }]);
      // The first reference also sets where to look (with a margin).
      if (!drawn && !(draft.options ?? []).some((x) => x.templates.length)) {
        set("region", grow(r, 0.25));
        setDrawn(true);
      }
      setOptionValue("");
      setTarget("region");
      return;
    }
    set("region", r);
    setDrawn(true);
    setRead(null);
  };

  const onPoint =
    target === "pick" && frame
      ? async (p: { x: number; y: number }) => {
          try {
            const color = await sampleColour(frame, p);
            setDraft((d) => ({ ...d, highlight: { box: d.highlight?.box ?? { x: 0, y: 0, w: 1, h: 1 }, tolerance: d.highlight?.tolerance ?? 40, color } }));
          } catch {
            toast.error("Could not read the colour here. Type it instead.");
          }
          setTarget("region");
        }
      : undefined;

  const readerForCall = (): ReaderDef => ({
    ...draft,
    id: draft.id || slug(draft.label) || "draft",
    label: draft.label || detailLabel(draft.field),
    values: (draft.values ?? []).map((v) => v.trim()).filter(Boolean),
  });

  const save = async () => {
    if (!game) return;
    const id = draft.id || slug(draft.label || detailLabel(draft.field));
    if (!id) return toast.error("Give the reader a name first.");
    if (!draft.field.trim()) return toast.error("Pick what this reader fills in.");
    if (isNew && !drawn && !crops.length) return toast.error("Draw a box on the frame around what to read.");
    if (crops.length && !frameSource) return toast.error("Load a frame before adding a reference.");
    setSaving(true);
    try {
      const updated = await api.studioSaveReader(game.id, { ...readerForCall(), id }, crops.length ? frameSource : null, crops);
      o.setGames((gs) => (gs ?? []).map((g) => (g.id === updated.id ? updated : g)));
      setReaderId(id);
      setCrops([]);
      toast.info(crops.length ? `Reader saved with ${crops.length} new ${crops.length === 1 ? "reference" : "references"}` : "Reader saved");
    } catch (e) {
      toast.error(e, "Could not save the reader");
    } finally {
      setSaving(false);
    }
  };

  const readFrame = async () => {
    if (!game || !frameSource) return;
    setReading(true);
    try {
      setRead(await api.studioReadFrame(game.id, readerForCall(), frameSource));
    } catch (e) {
      toast.error(e, "Could not read the frame");
    } finally {
      setReading(false);
    }
  };

  const runTest = async () => {
    if (!game || !match) return;
    setTesting(true);
    try {
      setTest(await api.studioTestReader(game.id, readerForCall(), match.id));
    } catch (e) {
      toast.error(e, "Test failed");
    } finally {
      setTesting(false);
    }
  };

  const remove = async () => {
    if (!game || !existing) return;
    try {
      const updated = await api.studioDeleteReader(game.id, existing.id);
      o.setGames((gs) => (gs ?? []).map((g) => (g.id === updated.id ? updated : g)));
      setReaderId(NEW_READER);
      toast.info("Reader deleted");
    } catch (e) {
      toast.error(e, "Could not delete");
    }
  };

  // ---- canvas ----
  const count = draft.slots?.count ?? 1;
  const overlays: CanvasProps["overlays"] = [];
  for (let i = 1; i < count; i++) overlays.push({ region: slotRegion(draft, i), className: "frame-slot" });
  if (read?.slot !== null && read?.slot !== undefined && count > 1) overlays.push({ region: slotRegion(draft, read.slot), className: "frame-slot is-picked" });
  for (const c of crops) overlays.push({ region: c.region, className: "frame-crop" });
  const hasRefs = (draft.options ?? []).some((x) => x.templates.length > 0);
  const canvas: CanvasProps = {
    box: draft.kind === "choice" && !hasRefs && !drawn && !crops.length && isNew ? null : slotRegion(draft, 0),
    region: null,
    overlays,
    drafting: drawn,
    onBox,
    onPoint,
    hint:
      target === "slot2"
        ? "Draw the box around the same thing in the next card or row."
        : target === "option"
          ? `Draw a tight box around “${optionValue.trim() || "the picture"}”.`
          : target === "pick"
            ? "Click the colour that marks your own card or row."
            : draft.kind === "choice"
              ? "Drag a box over the area where the picture appears."
              : "Drag a box around the text to read.",
  };

  // ---- rail ----
  const rail: ReactNode = (
    <>
      <ul className="det-list" role="tablist" aria-label={`Readers for ${game?.name ?? "this game"}`}>
        {(game?.readers ?? []).map((r) => (
          <li key={r.id}>
            <button role="tab" aria-selected={readerId === r.id} className={`det-item ${readerId === r.id ? "on" : ""}`} onClick={() => setReaderId(r.id)}>
              <span className="reader-glyph" aria-hidden>
                {r.kind === "choice" ? "◩" : "Aa"}
              </span>
              <span className="det-name">{r.label || r.id}</span>
              {!r.ready && (
                <span className={`det-state ${r.enabled ? "todo" : ""}`} title={r.enabled ? "Needs a reference picture" : "Turned off"}>
                  {r.enabled ? <span className="dot" aria-label="Needs a reference picture" /> : "Off"}
                </span>
              )}
            </button>
          </li>
        ))}
      </ul>
      <button role="tab" aria-selected={isNew} className={`det-add ${isNew ? "on" : ""}`} onClick={() => setReaderId(NEW_READER)}>
        <Icon name="plus" size={14} />
        New reader
      </button>
    </>
  );

  // ---- try it ----
  const readerName = draft.label || detailLabel(draft.field);
  const testPanel: ReactNode = (
    <section className="test" aria-label="Try the reader">
      <div className="test-head">
        <h2>Try it</h2>
        <div className="test-actions">
          <button className="btn btn-small" disabled={!frameSource || reading} onClick={readFrame} title="Read the frame shown above">
            {reading ? "Reading…" : "Read this frame"}
          </button>
          <button className="btn btn-small" disabled={!match || testing} onClick={runTest} title="Read the whole recording the way Recap does after a match">
            {testing ? "Reading…" : "Test on recording"}
          </button>
        </div>
      </div>
      {!read && !test && <p className="muted small">Read the frame above, or run {readerName.toLowerCase()} over a whole recording.</p>}
      {read && (
        <div className="read-result">
          {read.readings.length ? (
            <ul className="readings">
              {read.readings.map((r) => (
                <li key={r.field}>
                  <span className="muted">{detailLabel(r.field)}</span>
                  <strong>{displayValue(r.field, r.value)}</strong>
                </li>
              ))}
            </ul>
          ) : (
            <p className="small">Nothing read on this frame.</p>
          )}
          {count > 1 && (
            <p className="muted small">
              {read.slot === null ? "No card showed the highlight colour." : `Read card ${read.slot + 1} of ${count}${draft.highlight ? " (highlighted)" : ""}.`}
            </p>
          )}
          {!read.ocrAvailable && draft.kind === "text" && <p className="muted small">Text recognition is not available on this computer, so text readers only run on Windows.</p>}
        </div>
      )}
      {test && (
        <div className="read-result">
          <p className="muted small num">
            Looked from {timecode(test.window[0])} to {timecode(test.window[1])} · {test.frames} {test.frames === 1 ? "frame" : "frames"}
          </p>
          {Object.keys(test.details).length ? (
            <ul className="readings">
              {Object.entries(test.details).map(([f, v]) => (
                <li key={f} title={sourceText(v)}>
                  <span className="muted">{detailLabel(f)}</span>
                  <strong>{displayValue(f, v.value)}</strong>
                  {v.confidence !== undefined && <span className="muted num small">{Math.round(v.confidence * 100)}%</span>}
                </li>
              ))}
            </ul>
          ) : (
            <p className="small">Nothing was read in that part of the recording.</p>
          )}
          {test.readings.length > 0 && (
            <details className="advanced">
              <summary>Every reading ({test.readings.length})</summary>
              <ol className="reading-log">
                {test.readings.map(([t, f, v], i) => (
                  <li key={i}>
                    <button className="hit" onClick={() => o.showFrameAt(t)}>
                      <span className="num">{timecode(t)}</span>
                    </button>
                    <span className="muted">{detailLabel(f)}</span>
                    <span>{displayValue(f, v)}</span>
                  </li>
                ))}
              </ol>
            </details>
          )}
        </div>
      )}
    </section>
  );

  // ---- form ----
  const fieldSelectValue = customField ? "__custom" : READER_FIELDS.some((f) => f.id === draft.field) ? draft.field : "__custom";
  const optionUrls = existing?.optionUrls ?? {};
  const form: ReactNode = (
    <>
      <Field label="Name" htmlFor="rd-label">
        <input
          id="rd-label"
          className="input"
          value={draft.label}
          placeholder={detailLabel(draft.field || "map")}
          onChange={(e) => set("label", e.target.value)}
          title={isNew ? `Saved as ${draft.id || slug(draft.label || detailLabel(draft.field)) || "…"}` : `Id ${draft.id}`}
        />
      </Field>
      <Field label="Fills in" htmlFor="rd-field">
        <select id="rd-field" className="input" value={fieldSelectValue} onChange={(e) => setField(e.target.value)}>
          {READER_FIELDS.map((f) => (
            <option key={f.id} value={f.id}>
              {f.label}
            </option>
          ))}
          <option value="__custom">Something else…</option>
        </select>
      </Field>
      {fieldSelectValue === "__custom" && (
        <Field label="Detail name" htmlFor="rd-custom">
          <input id="rd-custom" className="input" value={draft.field} placeholder="rank" onChange={(e) => set("field", e.target.value.toLowerCase().replace(/[^a-z0-9_]+/g, "_"))} />
        </Field>
      )}
      <div className="field-row">
        <div className="field">
          <span className="field-label">Read as</span>
          <div className="seg" role="group" aria-label="Read as">
            <button className={draft.kind === "text" ? "on" : ""} aria-pressed={draft.kind === "text"} onClick={() => set("kind", "text")}>
              Text
            </button>
            <button className={draft.kind === "choice" ? "on" : ""} aria-pressed={draft.kind === "choice"} onClick={() => set("kind", "choice")} title="Match against labelled pictures, e.g. agent portraits">
              Pictures
            </button>
          </div>
        </div>
        <Field label="When" htmlFor="rd-when">
          <select id="rd-when" className="input" value={draft.when} onChange={(e) => set("when", e.target.value as ReaderDef["when"])}>
            {(Object.keys(WHEN_LABEL) as ReaderDef["when"][]).map((w) => (
              <option key={w} value={w}>
                {WHEN_LABEL[w]}
              </option>
            ))}
          </select>
        </Field>
      </div>

      {draft.kind === "text" && NUMERIC.includes(draft.field) ? (
        <p className="muted small reader-notes">
          {draft.field === "kda" ? "Reads three numbers, like 18 / 6 / 8, into kills, deaths and assists." : "Reads a number."}
        </p>
      ) : draft.kind === "text" ? (
        <Field label="Allowed values" htmlFor="rd-values" hint="One per line. Readings snap to the closest; leave empty to keep any text.">
          <textarea
            id="rd-values"
            className="input reader-values"
            rows={4}
            value={(draft.values ?? []).join("\n")}
            placeholder={draft.field === "map" ? "Ascent\nBind\nHaven" : ""}
            onChange={(e) => set("values", e.target.value.split("\n"))}
          />
        </Field>
      ) : (
        <div className="field">
          <span className="field-label">Values</span>
          {(draft.options ?? []).length === 0 && crops.length === 0 && <p className="muted small">None yet. Type a value, then draw a box around its picture.</p>}
          <ul className="option-list">
            {(draft.options ?? []).map((opt) => (
              <li key={opt.value}>
                <span className="option-value">{opt.value}</span>
                <span className="option-thumbs">
                  {(optionUrls[opt.value] ?? []).slice(0, 4).map((u) => (
                    <img key={u} src={fileSrc(u)} alt="" />
                  ))}
                  <span className="muted small num">{opt.templates.length}</span>
                </span>
              </li>
            ))}
            {crops.map((c, i) => (
              <li key={`new-${i}`} className="is-new">
                <span className="option-value">{c.value}</span>
                <span className="muted small">new</span>
                <button className="icon-btn" aria-label={`Remove new reference for ${c.value}`} onClick={() => setCrops((cs) => cs.filter((_, j) => j !== i))}>
                  <Icon name="close" size={12} />
                </button>
              </li>
            ))}
          </ul>
          <div className="option-add">
            <input
              className="input input-small"
              value={optionValue}
              placeholder="Value, e.g. Jett"
              aria-label="Value for the new reference"
              onChange={(e) => setOptionValue(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && optionValue.trim() && setTarget("option")}
            />
            <button className={`btn btn-small ${target === "option" ? "btn-primary" : ""}`} disabled={!optionValue.trim() || !frame} onClick={() => setTarget(target === "option" ? "region" : "option")}>
              {target === "option" ? "Drawing…" : "Draw it"}
            </button>
          </div>
        </div>
      )}

      <details className="advanced">
        <summary>More</summary>
        <div className="field field-inline">
          <span className="field-label">Repeated boxes</span>
          <Toggle
            checked={!!draft.slots}
            onChange={(v) => set("slots", v ? { count: 5, stepX: draft.region.w * 1.1, stepY: 0 } : undefined)}
            label="One box per player card or row"
          />
        </div>
        {draft.slots && (
          <>
            <div className="field-row slot-row">
              <Field label="How many" htmlFor="rd-count">
                <input
                  id="rd-count"
                  type="number"
                  className="input"
                  min={2}
                  max={16}
                  value={draft.slots.count}
                  onChange={(e) => set("slots", { ...draft.slots!, count: Math.min(16, Math.max(2, Number(e.target.value))) })}
                />
              </Field>
              <Field label="Across %" htmlFor="rd-sx">
                <input
                  id="rd-sx"
                  type="number"
                  className="input"
                  step={0.1}
                  value={+(draft.slots.stepX * 100).toFixed(2)}
                  onChange={(e) => set("slots", { ...draft.slots!, stepX: Number(e.target.value) / 100 })}
                />
              </Field>
              <Field label="Down %" htmlFor="rd-sy">
                <input
                  id="rd-sy"
                  type="number"
                  className="input"
                  step={0.1}
                  value={+(draft.slots.stepY * 100).toFixed(2)}
                  onChange={(e) => set("slots", { ...draft.slots!, stepY: Number(e.target.value) / 100 })}
                />
              </Field>
            </div>
            <button className={`btn btn-small form-btn ${target === "slot2" ? "btn-primary" : ""}`} onClick={() => setTarget(target === "slot2" ? "region" : "slot2")} disabled={!frame} title="Or set how far apart the boxes are above, as a share of the frame">
              {target === "slot2" ? "Drawing the second box…" : "Draw the second box"}
            </button>
            <div className="field field-inline">
              <span className="field-label">Pick the highlighted one</span>
              <Toggle
                checked={!!draft.highlight}
                onChange={(v) => set("highlight", v ? { box: { x: 0, y: 0, w: 1, h: 1 }, color: "#e2e88f", tolerance: 40 } : undefined)}
                label="Read only the card or row showing a colour"
              />
            </div>
            {draft.highlight && (
              <div className="highlight-row">
                <input
                  type="color"
                  className="swatch-input"
                  value={draft.highlight.color}
                  onChange={(e) => set("highlight", { ...draft.highlight!, color: e.target.value })}
                  aria-label="Highlight colour"
                />
                <input
                  className="input input-small num"
                  value={draft.highlight.color}
                  onChange={(e) => set("highlight", { ...draft.highlight!, color: e.target.value })}
                  aria-label="Highlight colour (hex)"
                />
                <button className={`btn btn-small ${target === "pick" ? "btn-primary" : ""}`} disabled={!frame} onClick={() => setTarget(target === "pick" ? "region" : "pick")}>
                  {target === "pick" ? "Click the frame…" : "Pick from frame"}
                </button>
              </div>
            )}
          </>
        )}
        {draft.kind === "text" ? (
          <Field label="Pattern" htmlFor="rd-pattern" hint="Optional regular expression. Named groups such as (?P<kills>\d+) fill those details.">
            <input id="rd-pattern" className="input num" value={draft.pattern ?? ""} placeholder="^\d{1,2}$" onChange={(e) => set("pattern", e.target.value || undefined)} />
          </Field>
        ) : (
          <Field label="Match needed" htmlFor="rd-thr">
            <div className="range-row">
              <input id="rd-thr" type="range" min={0.5} max={0.99} step={0.01} value={draft.threshold} onChange={(e) => set("threshold", Number(e.target.value))} />
              <span className="num range-val">{Math.round(draft.threshold * 100)}%</span>
            </div>
          </Field>
        )}
        <Field label="Checks / second" htmlFor="rd-fps">
          <input id="rd-fps" type="number" className="input" min={0.2} max={10} step={0.2} value={draft.sampleFps} onChange={(e) => set("sampleFps", Math.min(10, Math.max(0.2, Number(e.target.value))))} />
        </Field>
        <div className="field field-inline">
          <span className="field-label">Enabled</span>
          <Toggle checked={draft.enabled} onChange={(v) => set("enabled", v)} label="Reader enabled" />
        </div>
      </details>

      {draft.notes && <p className="muted small reader-notes">{draft.notes}</p>}

      <div className="form-actions">
        <button className="btn btn-primary" onClick={save} disabled={saving || !game}>
          {saving ? "Saving…" : isNew ? "Save reader" : "Save changes"}
        </button>
        {!isNew && existing && (
          <button className="btn btn-ghost btn-danger-text" onClick={() => setConfirmDelete(true)}>
            Delete
          </button>
        )}
      </div>
    </>
  );

  const dialogs: ReactNode =
    confirmDelete && existing ? (
      <Confirm
        title="Delete reader"
        danger
        confirmLabel="Delete reader"
        body={
          <p>
            <strong>{existing.label || existing.id}</strong> will stop filling in {detailLabel(existing.field).toLowerCase()} for {game?.name}. Details already saved stay.
          </p>
        }
        onClose={() => setConfirmDelete(false)}
        onConfirm={remove}
      />
    ) : null;

  return { rail, canvas, testPanel, form, dialogs, when: draft.when };
}
