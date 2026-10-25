// Detector Studio: what a detector does (marks plays / reads a value / hides other detections),
// where its HUD element is pinned, and which colours it compares.
import type { Anchor, DetectorDef, HsvRange } from "../lib/types";
import { ANCHORS, rangeSwatch } from "../lib/anchor";
import { COMMON_KINDS, kindStyle } from "../lib/kinds";
import { fileSrc } from "../lib/api";
import { Field, Icon, KindDot } from "../components/ui";

export type DetectorRole = "mark" | "value" | "block";

export const roleOf = (d: DetectorDef): DetectorRole => (d.valueRule || d.values?.length ? "value" : d.blocks?.length ? "block" : "mark");

/** The draft with its role switched; fields of the other roles are cleared. */
export function withRole(d: DetectorDef, role: DetectorRole): DetectorDef {
  const base: DetectorDef = {
    ...d,
    values: undefined,
    valueRule: undefined,
    blocks: undefined,
    blockMarginMs: undefined,
  };
  if (role === "value")
    return {
      ...base,
      event: d.event.startsWith("_") ? "kill" : d.event,
      values: d.templates.map((_, i) => d.values?.[i] ?? ""),
      valueRule: d.valueRule ?? { votes: 3, window: 5, fire: "increase" },
    };
  if (role === "block")
    return {
      ...base,
      event: "_blocker",
      blocks: d.blocks?.length ? d.blocks : ["kill", "assist"],
      blockMarginMs: d.blockMarginMs ?? 1000,
    };
  return { ...base, event: d.event.startsWith("_") ? "kill" : d.event };
}

const ROLES: Array<{ id: DetectorRole; label: string; title: string }> = [
  {
    id: "mark",
    label: "Marks plays",
    title: "Adds a marker each time the element appears",
  },
  {
    id: "value",
    label: "Reads a value",
    title: "Each reference stands for a value (a kill counter's digits, a 2X/3X badge); marks when it changes",
  },
  {
    id: "block",
    label: "Hides plays",
    title: "While it is on screen (spectating, a killcam), other detections of the chosen kinds are dropped",
  },
];

export function RolePicker({ draft, onChange }: { draft: DetectorDef; onChange: (d: DetectorDef) => void }) {
  const role = roleOf(draft);
  return (
    <div className="field">
      <p className="field-label">What it does</p>
      <div className="seg seg-full" role="radiogroup" aria-label="What the detector does">
        {ROLES.map((r) => (
          <button
            key={r.id}
            role="radio"
            aria-checked={role === r.id}
            className={role === r.id ? "on" : ""}
            title={r.title}
            onClick={() => role !== r.id && onChange(withRole(draft, r.id))}
          >
            {r.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Value per reference + voting, for "Reads a value". */
export function ValueSettings({ draft, templateUrls, set }: { draft: DetectorDef; templateUrls: string[]; set: (d: Partial<DetectorDef>) => void }) {
  const rule = draft.valueRule ?? {
    votes: 3,
    window: 5,
    fire: "increase" as const,
  };
  const values = draft.templates.map((_, i) => draft.values?.[i] ?? "");
  return (
    <div className="field value-settings">
      <p className="field-label">Value of each reference</p>
      {templateUrls.length ? (
        <ul className="value-list">
          {templateUrls.map((u, i) => (
            <li key={u}>
              <img src={fileSrc(u)} alt={`Reference ${i + 1}`} />
              <input
                className="input input-small"
                value={values[i]}
                placeholder="e.g. 3"
                aria-label={`Value of reference ${i + 1}`}
                onChange={(e) =>
                  set({
                    values: values.map((v, j) => (j === i ? e.target.value : v)),
                  })
                }
              />
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted small">Save one reference per value (each digit, each badge), then name them here.</p>
      )}
      <div className="field-row">
        <Field label="Marks when" htmlFor="st-vfire">
          <select
            id="st-vfire"
            className="input"
            value={rule.fire}
            onChange={(e) =>
              set({
                valueRule: {
                  ...rule,
                  fire: e.target.value as "change" | "increase",
                },
              })
            }
          >
            <option value="increase">The number goes up</option>
            <option value="change">The value changes</option>
          </select>
        </Field>
        <Field label="Agreeing checks" htmlFor="st-votes">
          <div className="inline-pair">
            <input
              id="st-votes"
              type="number"
              className="input"
              min={1}
              max={rule.window}
              value={rule.votes}
              onChange={(e) =>
                set({
                  valueRule: {
                    ...rule,
                    votes: Math.min(rule.window, Math.max(1, Number(e.target.value))),
                  },
                })
              }
              title="How many of the recent checks must read the same value before it counts"
            />
            <span className="muted small">of</span>
            <input
              type="number"
              className="input"
              aria-label="Recent checks"
              min={rule.votes}
              max={60}
              value={rule.window}
              onChange={(e) =>
                set({
                  valueRule: {
                    ...rule,
                    window: Math.min(60, Math.max(rule.votes, Number(e.target.value))),
                  },
                })
              }
            />
          </div>
        </Field>
      </div>
      <p className="field-hint">A counter going from 1 to 3 marks two plays. The first value seen is only the starting point.</p>
    </div>
  );
}

/** Kinds to hide + margin, for "Hides plays". */
export function BlockSettings({ draft, set }: { draft: DetectorDef; set: (d: Partial<DetectorDef>) => void }) {
  const blocks = draft.blocks ?? [];
  const toggle = (k: string) => {
    const next = blocks.includes(k) ? blocks.filter((b) => b !== k) : [...blocks, k];
    if (next.length) set({ blocks: next });
  };
  return (
    <div className="field">
      <p className="field-label">Hide these while it is on screen</p>
      <div className="chip-row">
        {COMMON_KINDS.map((k) => (
          <button key={k} className={`chip ${blocks.includes(k) ? "on" : ""}`} aria-pressed={blocks.includes(k)} onClick={() => toggle(k)}>
            <KindDot kind={k} size={12} tone={blocks.includes(k) ? "color" : "muted"} /> {kindStyle(k).label}
          </button>
        ))}
      </div>
      <Field label="Also hide for (s) before and after" htmlFor="st-bm">
        <input
          id="st-bm"
          type="number"
          className="input input-small"
          min={0}
          max={30}
          step={0.5}
          value={(draft.blockMarginMs ?? 1000) / 1000}
          onChange={(e) =>
            set({
              blockMarginMs: Math.round(Math.min(30, Math.max(0, Number(e.target.value))) * 1000),
            })
          }
        />
      </Field>
      <p className="field-hint">Use it for something only shown when you are not playing yourself, like the spectating bar after you die.</p>
    </div>
  );
}

/** Anchor + colour filter, inside "More settings". */
export function PlacementSettings({
  draft,
  set,
  picking,
  onPick,
}: {
  draft: DetectorDef;
  set: (d: Partial<DetectorDef>) => void;
  picking: boolean;
  onPick: (on: boolean) => void;
}) {
  const ranges = draft.colorFilter?.ranges ?? [];
  const setRanges = (rs: HsvRange[]) => set({ colorFilter: rs.length ? { ranges: rs } : undefined });
  const setRange = (i: number, r: HsvRange) => setRanges(ranges.map((x, j) => (j === i ? r : x)));
  const num = (v: string, max: number) => Math.min(max, Math.max(0, Number(v) || 0));
  return (
    <>
      <Field
        label="Pinned to"
        htmlFor="st-anchor"
        hint="Where the game keeps this element on ultrawide or 16:10 screens. The box is measured on a 16:9 picture; on 16:9 recordings nothing changes."
      >
        <select
          id="st-anchor"
          className="input"
          value={draft.anchor ?? ""}
          onChange={(e) => set({ anchor: (e.target.value || undefined) as Anchor | undefined })}
        >
          <option value="">Stretches with the picture</option>
          {ANCHORS.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}
            </option>
          ))}
        </select>
      </Field>
      <div className="field">
        <p className="field-label">Only compare these colours</p>
        {ranges.length > 0 && (
          <ul className="hsv-list">
            {ranges.map((r, i) => (
              <li key={i}>
                <span className="hsv-swatch" style={{ background: rangeSwatch(r) }} aria-hidden />
                <label>
                  <span className="muted small">Hue</span>
                  <input
                    className="input input-small"
                    type="number"
                    min={0}
                    max={360}
                    value={r.hue[0]}
                    onChange={(e) =>
                      setRange(i, {
                        ...r,
                        hue: [num(e.target.value, 360), r.hue[1]],
                      })
                    }
                    aria-label="Hue from"
                  />
                  <span className="muted small">–</span>
                  <input
                    className="input input-small"
                    type="number"
                    min={0}
                    max={360}
                    value={r.hue[1]}
                    onChange={(e) =>
                      setRange(i, {
                        ...r,
                        hue: [r.hue[0], num(e.target.value, 360)],
                      })
                    }
                    aria-label="Hue to"
                  />
                </label>
                <label title="Lowest saturation (0–100%): lower takes in greyer shades">
                  <span className="muted small">Sat ≥</span>
                  <input
                    className="input input-small"
                    type="number"
                    min={0}
                    max={100}
                    value={Math.round(r.sat[0] * 100)}
                    onChange={(e) =>
                      setRange(i, {
                        ...r,
                        sat: [num(e.target.value, 100) / 100, r.sat[1]],
                      })
                    }
                    aria-label="Lowest saturation"
                  />
                </label>
                <label title="Lowest brightness (0–100%)">
                  <span className="muted small">Bright ≥</span>
                  <input
                    className="input input-small"
                    type="number"
                    min={0}
                    max={100}
                    value={Math.round(r.val[0] * 100)}
                    onChange={(e) =>
                      setRange(i, {
                        ...r,
                        val: [num(e.target.value, 100) / 100, r.val[1]],
                      })
                    }
                    aria-label="Lowest brightness"
                  />
                </label>
                <button className="icon-btn" aria-label="Remove colour" onClick={() => setRanges(ranges.filter((_, j) => j !== i))}>
                  <Icon name="close" size={12} />
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className="inline-pair">
          <button className={`btn btn-small ${picking ? "btn-primary" : ""}`} onClick={() => onPick(!picking)} aria-pressed={picking}>
            {picking ? "Click the colour on the frame…" : "Pick a colour from the frame"}
          </button>
        </div>
        <p className="field-hint">
          {ranges.length
            ? "Everything else is blacked out before comparing, in the references too. Save and test again after changing."
            : "For a HUD colour that stands out (a red kill icon, a yellow badge): the scene behind it stops mattering."}
        </p>
      </div>
    </>
  );
}
