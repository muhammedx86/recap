import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { DETAIL_FIELDS, customFields, detailLabel, displayValue, kdaSource, kdaText, sourceText } from "../lib/details";
import { useToast } from "../lib/toast";
import type { DetailValue, MatchRecord } from "../lib/types";
import { Icon } from "./ui";

type Item = { key: string; label: string; value: string | null; source: DetailValue | null; fields: string[] };

/** The quiet row of match details under the title. Every value is click-to-edit. */
export function MatchDetailsStrip({ m, onSaved }: { m: MatchRecord; onSaved: (m: MatchRecord) => void }) {
  const d = m.details ?? {};
  const toast = useToast();
  const [editing, setEditing] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  const single = (id: string): Item => ({ key: id, label: detailLabel(id), value: d[id] ? displayValue(id, d[id].value) : null, source: d[id] ?? null, fields: [id] });
  const all: Item[] = [
    single("character"),
    single("map"),
    single("mode"),
    { key: "kda", label: "K/D/A", value: kdaText(d), source: kdaSource(d), fields: ["kills", "deaths", "assists"] },
    single("result"),
    single("score"),
    ...customFields(m).map(single),
  ];
  const shown = all.filter((i) => i.value !== null || i.key === editing);
  const missing = all.filter((i) => i.value === null && i.key !== editing && DETAIL_FIELDS.some((f) => f.id === i.key || (i.key === "kda" && f.id === "kills")));

  const save = async (item: Item, text: string) => {
    setEditing(null);
    const value = text.trim();
    try {
      let rec = m;
      if (item.key === "kda") {
        const parts = value ? value.split(/[^0-9]+/).filter(Boolean) : ["", "", ""];
        if (value && parts.length !== 3) {
          toast.error("Write K/D/A as three numbers, like 18/6/8.");
          return;
        }
        for (let i = 0; i < 3; i++) if ((d[item.fields[i]]?.value ?? "") !== parts[i]) rec = await api.updateMatchDetail(m.id, item.fields[i], parts[i]);
      } else {
        const stored = item.key === "result" ? normaliseResult(value) : value;
        if ((d[item.key]?.value ?? "") === stored) return;
        rec = await api.updateMatchDetail(m.id, item.key, stored);
      }
      onSaved(rec);
    } catch (e) {
      toast.error(e, "Could not save");
    }
  };

  if (!shown.length && !adding)
    return (
      <div className="details details-empty">
        <button className="details-add" onClick={() => setAdding(true)}>
          <Icon name="plus" size={12} />
          Add details
        </button>
      </div>
    );

  return (
    <dl className="details" aria-label="Match details">
      {shown.map((item) => (
        <div key={item.key} className="detail">
          <dt>{item.label}</dt>
          <dd>
            {editing === item.key ? (
              <InlineEdit
                initial={item.key === "kda" ? (kdaText(d, "/") ?? "") : item.key === "result" ? (item.value ?? "") : (d[item.key]?.value ?? "")}
                placeholder={item.key === "kda" ? "18/6/8" : DETAIL_FIELDS.find((f) => f.id === item.key)?.placeholder ?? ""}
                label={item.label}
                onDone={(v) => (v === null ? setEditing(null) : save(item, v))}
              />
            ) : (
              <button
                className={`detail-value ${item.key === "result" ? `result-${(d.result?.value ?? "").toLowerCase()}` : ""}`}
                onClick={() => setEditing(item.key)}
                title={`${item.source ? sourceText(item.source) : ""}. Click to edit.`}
              >
                <span className={item.key === "kda" || item.key === "score" ? "num" : undefined}>{item.value}</span>
                {item.source && <span className={`src-dot src-${item.source.source}`} aria-label={sourceText(item.source)} />}
              </button>
            )}
          </dd>
        </div>
      ))}
      {missing.length > 0 && (
        <div className="detail detail-more">
          <dt className="sr-only">Add</dt>
          <dd>
            {adding ? (
              <select
                className="input input-small details-picker"
                autoFocus
                defaultValue=""
                onBlur={() => setAdding(false)}
                onChange={(e) => {
                  setAdding(false);
                  if (e.target.value) setEditing(e.target.value);
                }}
                aria-label="Add a detail"
              >
                <option value="" disabled>
                  Add…
                </option>
                {missing.map((i) => (
                  <option key={i.key} value={i.key}>
                    {i.label}
                  </option>
                ))}
              </select>
            ) : (
              <button className="details-add" onClick={() => setAdding(true)} aria-label="Add a detail" title="Add a detail">
                <Icon name="plus" size={12} />
              </button>
            )}
          </dd>
        </div>
      )}
    </dl>
  );
}

function normaliseResult(v: string): string {
  const s = v.trim().toLowerCase();
  if (!s) return "";
  if (/^(w|win|won|victory)/.test(s)) return "win";
  if (/^(l|loss|lost|lose|defeat)/.test(s)) return "loss";
  if (/^(d|draw|tie)/.test(s)) return "draw";
  return v.trim();
}

function InlineEdit({ initial, placeholder, label, onDone }: { initial: string; placeholder: string; label: string; onDone: (v: string | null) => void }) {
  const [v, setV] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (val: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(val);
  };
  return (
    <input
      ref={ref}
      className="detail-input"
      value={v}
      placeholder={placeholder}
      aria-label={label}
      size={Math.max(4, Math.min(18, (v || placeholder).length + 1))}
      onChange={(e) => setV(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") finish(v);
        if (e.key === "Escape") finish(null);
      }}
      onBlur={() => finish(v)}
    />
  );
}
