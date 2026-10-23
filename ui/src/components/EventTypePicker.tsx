import { useState } from "react";
import { api } from "../lib/api";
import { EVENT_COLORS, EVENT_ICONS, ROUND_KINDS } from "../lib/eventTypes";
import { kindStyle, mergeEventTypes, refreshEventTypes } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { EventIcon as EventIconName, EventTypeDef, GameInfo } from "../lib/types";
import { EventIcon } from "./EventIcon";
import { Icon } from "./ui";

const MATCH_KINDS = ["match_start", "match_end"];

/**
 * Pick which event a detector marks. Lists the game's event types in groups (plays, rounds,
 * match boundaries, your own) and lets you create, edit or delete custom types inline.
 */
export function EventTypePicker({
  game,
  value,
  onChange,
  onGameUpdated,
}: {
  game: GameInfo;
  value: string;
  onChange: (id: string) => void;
  onGameUpdated: (g: GameInfo) => void;
}) {
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<EventTypeDef | "new" | null>(null);
  const types = game.eventTypes;
  const current = kindStyle(value);

  const builtinPlays = types.filter((t) => t.builtin && !ROUND_KINDS.includes(t.id) && !MATCH_KINDS.includes(t.id));
  const rounds = ROUND_KINDS.map((id) => types.find((t) => t.id === id)).filter(Boolean) as EventTypeDef[];
  const match = MATCH_KINDS.map((id) => types.find((t) => t.id === id)).filter(Boolean) as EventTypeDef[];
  const custom = types.filter((t) => !t.builtin);

  const pick = (id: string) => {
    onChange(id);
    setOpen(false);
    setEditing(null);
  };

  const remove = async (t: EventTypeDef) => {
    try {
      const g = await api.deleteEventType(game.id, t.id);
      onGameUpdated(g);
      refreshEventTypes();
      if (value === t.id) onChange("kill");
      toast.info(`“${t.label}” deleted`);
    } catch (e) {
      toast.error(e, "Could not delete this event type");
    }
  };

  const group = (title: string, list: EventTypeDef[], hint?: string, editable = false) =>
    list.length > 0 && (
      <div className="et-group">
        <p className="et-group-title">
          {title}
          {hint && <span className="muted"> · {hint}</span>}
        </p>
        <ul className={`et-grid ${editable ? "et-grid-wide" : ""}`}>
          {list.map((t) => (
            <li key={t.id} className={`et-item ${value === t.id ? "on" : ""}`}>
              <button type="button" className="et-option" onClick={() => pick(t.id)} aria-pressed={value === t.id}>
                <span className="et-icon" style={{ color: t.color }}>
                  <EventIcon icon={t.icon} size={15} />
                </span>
                <span className="et-label">{t.label}</span>
              </button>
              {editable && (
                <span className="et-actions">
                  <button type="button" className="icon-btn" aria-label={`Edit ${t.label}`} onClick={() => setEditing(t)}>
                    <Icon name="edit" size={13} />
                  </button>
                  <button type="button" className="icon-btn" aria-label={`Delete ${t.label}`} onClick={() => remove(t)}>
                    <Icon name="trash" size={13} />
                  </button>
                </span>
              )}
            </li>
          ))}
        </ul>
      </div>
    );

  return (
    <div className="et-picker">
      <button type="button" className={`et-current input ${open ? "open" : ""}`} aria-expanded={open} onClick={() => (setOpen(!open), setEditing(null))}>
        <span className="et-icon" style={{ color: current.color }}>
          <EventIcon icon={current.icon} size={16} />
        </span>
        <span className="et-label">{current.label}</span>
        <span className="et-chev" aria-hidden>
          <Icon name={open ? "chev-up" : "chev-down"} size={14} />
        </span>
      </button>

      {open && (
        <div className="et-panel">
          {editing ? (
            <EventTypeForm
              game={game}
              initial={editing === "new" ? null : editing}
              onCancel={() => setEditing(null)}
              onSaved={(g, id) => {
                onGameUpdated(g);
                mergeEventTypes(g.eventTypes);
                pick(id);
              }}
            />
          ) : (
            <>
              {group("Plays", builtinPlays)}
              {group("Rounds", rounds, "shown as round dividers")}
              {group("Match", match, "splits a session into matches")}
              {group(`Your types for ${game.name}`, custom, undefined, true)}
              <button type="button" className="btn btn-small et-new" onClick={() => setEditing("new")}>
                <Icon name="plus" size={14} />
                New event type
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}

function EventTypeForm({
  game,
  initial,
  onCancel,
  onSaved,
}: {
  game: GameInfo;
  initial: EventTypeDef | null;
  onCancel: () => void;
  onSaved: (g: GameInfo, id: string) => void;
}) {
  const toast = useToast();
  const [label, setLabel] = useState(initial?.label ?? "");
  const [icon, setIcon] = useState<EventIconName>(initial?.icon ?? "zap");
  const [color, setColor] = useState<string | null>(initial?.color ?? null);
  const [busy, setBusy] = useState(false);

  const save = async () => {
    if (!label.trim()) {
      toast.error("Give the event type a name.");
      return;
    }
    setBusy(true);
    try {
      const before = new Set(game.eventTypes.map((t) => t.id));
      const g = await api.saveEventType(game.id, { ...(initial ? { id: initial.id } : {}), label: label.trim(), icon, color });
      const id = initial?.id ?? g.eventTypes.find((t) => !t.builtin && !before.has(t.id))?.id ?? g.eventTypes.find((t) => t.label === label.trim())?.id;
      if (!id) throw "The new event type was not returned";
      toast.info(initial ? "Event type updated" : `“${label.trim()}” created`);
      onSaved(g, id);
    } catch (e) {
      toast.error(e, "Could not save the event type");
    } finally {
      setBusy(false);
    }
  };

  const shownColor = color ?? "var(--ink-2)";
  return (
    <form
      className="et-form"
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <p className="et-group-title">{initial ? `Edit “${initial.label}”` : `New event type for ${game.name}`}</p>
      <label className="field-label" htmlFor="et-name">
        Name
      </label>
      <input id="et-name" className="input" value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Ult used" autoFocus maxLength={40} />

      <p className="field-label">Icon</p>
      <div className="icon-grid" role="radiogroup" aria-label="Icon">
        {EVENT_ICONS.map((i) => (
          <button
            key={i}
            type="button"
            role="radio"
            aria-checked={icon === i}
            aria-label={i.replace("-", " ")}
            className={`icon-choice ${icon === i ? "on" : ""}`}
            onClick={() => setIcon(i)}
            style={icon === i ? { color: shownColor } : undefined}
          >
            <EventIcon icon={i} size={18} />
          </button>
        ))}
      </div>

      <p className="field-label">Colour</p>
      <div className="swatches" role="radiogroup" aria-label="Colour">
        <button type="button" role="radio" aria-checked={color === null} className={`swatch swatch-auto ${color === null ? "on" : ""}`} onClick={() => setColor(null)}>
          Auto
        </button>
        {EVENT_COLORS.map((c) => (
          <button key={c} type="button" role="radio" aria-checked={color === c} aria-label={c} className={`swatch ${color === c ? "on" : ""}`} style={{ background: c }} onClick={() => setColor(c)} />
        ))}
      </div>

      <div className="et-preview">
        <span className="et-icon" style={{ color: shownColor }}>
          <EventIcon icon={icon} size={16} />
        </span>
        <span>{label.trim() || "Your event"}</span>
        <span className="muted small">on the timeline</span>
      </div>

      <div className="et-form-actions">
        <button type="button" className="btn btn-small" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="btn btn-primary btn-small" disabled={busy}>
          {busy ? "Saving…" : initial ? "Save type" : "Create type"}
        </button>
      </div>
    </form>
  );
}
