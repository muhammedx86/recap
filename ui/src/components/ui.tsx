import { useEffect, useRef, useState, type ReactNode } from "react";
import { fileSrc } from "../lib/api";
import { kindStyle, sortKinds } from "../lib/kinds";
import { ROUND_KINDS } from "../lib/eventTypes";
import { hasRoundEvents, roundScore } from "../lib/rounds";
import { EventIcon } from "./EventIcon";

/** The event kind's icon in its colour. (Named for history: it used to be a dot.) */
/** Event icon. Lists use the quiet muted tone; pass `tone="color"` where colour carries meaning (timeline). */
export function KindDot({ kind, size = 14, tone = "muted" }: { kind: string; size?: number; tone?: "muted" | "color" }) {
  const st = kindStyle(kind);
  return (
    <span className="kind-icon" style={{ color: tone === "color" ? st.color : "var(--muted)" }} aria-hidden>
      <EventIcon icon={st.icon} size={size} />
    </span>
  );
}

/** Won–lost round score, e.g. 13–9. Renders nothing when the match has no round results. */
export function RoundScore({ counts, large }: { counts: Record<string, number>; large?: boolean }) {
  const s = roundScore(counts);
  if (!s) return null;
  const outcome = s.won > s.lost ? "win" : s.won < s.lost ? "loss" : "draw";
  return (
    <span className={`round-score ${outcome} ${large ? "large" : ""}`} title={`${s.won} rounds won, ${s.lost} lost`}>
      <span className="num">{s.won}</span>
      <span className="round-score-dash" aria-hidden>
        –
      </span>
      <span className="num">{s.lost}</span>
    </span>
  );
}

/** Compact event count summary, e.g. 24 kills 15 deaths, each with its icon. Round markers are shown as a score instead. */
export function KindCounts({ counts, max = 4 }: { counts: Record<string, number>; max?: number }) {
  const kinds = sortKinds(Object.keys(counts).filter((k) => counts[k] > 0 && k !== "match_start" && k !== "match_end" && !ROUND_KINDS.includes(k)));
  if (!kinds.length) return hasRoundEvents(counts) ? null : <span className="muted small">No events</span>;
  return (
    <span className="kind-counts">
      {kinds.slice(0, max).map((k) => (
        <span key={k} className="kind-count" title={kindStyle(k).plural}>
          <KindDot kind={k} />
          <span className="num">{counts[k]}</span>
          <span className="kind-count-label">{(counts[k] === 1 ? kindStyle(k).label : kindStyle(k).plural).toLowerCase()}</span>
        </span>
      ))}
      {kinds.length > max && <span className="muted small">+{kinds.length - max} more</span>}
    </span>
  );
}

export function KindChip({ kind }: { kind: string }) {
  return (
    <span className="kind-chip">
      <KindDot kind={kind} />
      {kindStyle(kind).label}
    </span>
  );
}

export function Thumb({ path, alt, className = "", children }: { path: string | null; alt: string; className?: string; children?: ReactNode }) {
  const [failed, setFailed] = useState(false);
  const src = fileSrc(path);
  return (
    <div className={`thumb ${className}`}>
      {src && !failed ? <img src={src} alt={alt} loading="lazy" onError={() => setFailed(true)} /> : <div className="thumb-empty" aria-label={alt} />}
      {children}
    </div>
  );
}

export function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button type="button" role="switch" aria-checked={checked} aria-label={label} disabled={disabled} className={`toggle ${checked ? "on" : ""}`} onClick={() => onChange(!checked)}>
      <span className="toggle-knob" />
    </button>
  );
}

export function Field({ label, hint, children, htmlFor }: { label: string; hint?: ReactNode; children: ReactNode; htmlFor?: string }) {
  return (
    <div className="field">
      <label className="field-label" htmlFor={htmlFor}>
        {label}
      </label>
      <div className="field-control">{children}</div>
      {hint && <p className="field-hint">{hint}</p>}
    </div>
  );
}

export function Modal({ title, onClose, children, wide, footer }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean; footer?: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      prev?.focus?.();
    };
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`modal ${wide ? "modal-wide" : ""}`} role="dialog" aria-modal="true" aria-label={title} tabIndex={-1} ref={ref}>
        <header className="modal-head">
          <h2>{title}</h2>
          <button className="icon-btn" aria-label="Close" onClick={onClose}>
            <Icon name="close" />
          </button>
        </header>
        <div className="modal-body">{children}</div>
        {footer && <footer className="modal-foot">{footer}</footer>}
      </div>
    </div>
  );
}

export function Empty({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      <h3>{title}</h3>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

export function ProgressBar({ value, label }: { value: number; label?: string }) {
  return (
    <div className="progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(value * 100)} aria-label={label}>
      <div className="progress-fill" style={{ width: `${Math.max(2, Math.min(100, value * 100))}%` }} />
    </div>
  );
}

const ICONS: Record<string, string> = {
  library: "M4 5h16M4 12h16M4 19h10",
  clips: "M5 4h14v16H5zM9 9l6 3-6 3z",
  studio: "M4 4h6v6H4zM14 14h6v6h-6zM10 7h7v7",
  games: "M7 9v6M4 12h6M15 11h.01M18 13h.01M3 7h18v10H3z",
  settings: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1",
  play: "M7 4l13 8-13 8z",
  pause: "M7 4h4v16H7zM13 4h4v16h-4z",
  back: "M15 5l-7 7 7 7",
  close: "M6 6l12 12M18 6 6 18",
  star: "M12 3l2.8 5.9 6.2.8-4.6 4.3 1.2 6.2L12 17.3 6.4 20.2l1.2-6.2L3 9.7l6.2-.8z",
  trash: "M5 7h14M10 11v6M14 11v6M7 7l1 13h8l1-13M9 7V4h6v3",
  folder: "M3 6h7l2 2h9v11H3z",
  scissors: "M6 6a2 2 0 1 0 0 .01M6 18a2 2 0 1 0 0 .01M8 7l12 10M8 17 20 7",
  marker: "M12 3v18M7 8l5-5 5 5",
  refresh: "M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7",
  plus: "M12 5v14M5 12h14",
  edit: "M4 20h4L19 9l-4-4L4 16zM14 6l4 4",
  volume: "M4 9h4l5-4v14l-5-4H4zM16 9a4 4 0 0 1 0 6",
  mute: "M4 9h4l5-4v14l-5-4H4zM16 9l5 6M21 9l-5 6",
  up: "M12 19V5M6 11l6-6 6 6",
  "chev-down": "M6 9l6 6 6-6",
  "chev-up": "M6 15l6-6 6 6",
  down: "M12 5v14M6 13l6 6 6-6",
  record: "M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10z",
  stop: "M7 7h10v10H7z",
  maximize: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  minimize: "M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5",
  warning: "M12 4 2.5 20h19zM12 10v4M12 17h.01",
};

export function Icon({ name, size = 18 }: { name: string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.7} strokeLinecap="round" strokeLinejoin="round" aria-hidden className="icon">
      <path d={ICONS[name] ?? ""} />
    </svg>
  );
}

/** Confirmation dialog with an optional checkbox (e.g. "also delete the file"). */
export function Confirm({
  title,
  body,
  confirmLabel,
  danger,
  checkboxLabel,
  onConfirm,
  onClose,
}: {
  title: string;
  body: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  checkboxLabel?: string;
  onConfirm: (checked: boolean) => void | Promise<void>;
  onClose: () => void;
}) {
  const [checked, setChecked] = useState(true);
  const [busy, setBusy] = useState(false);
  return (
    <Modal
      title={title}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className={`btn ${danger ? "btn-danger" : "btn-primary"}`}
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await onConfirm(checked);
                onClose();
              } finally {
                setBusy(false);
              }
            }}
          >
            {confirmLabel}
          </button>
        </>
      }
    >
      <div className="confirm-body">{body}</div>
      {checkboxLabel && (
        <label className="check">
          <input type="checkbox" checked={checked} onChange={(e) => setChecked(e.target.checked)} />
          {checkboxLabel}
        </label>
      )}
    </Modal>
  );
}
