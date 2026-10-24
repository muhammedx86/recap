import { useEffect, useRef, useState, type ReactNode } from "react";
import { fileSrc } from "../lib/api";
import { kindStyle, sortKinds } from "../lib/kinds";
import { ROUND_KINDS } from "../lib/eventTypes";
import { hasRoundEvents, roundScore } from "../lib/rounds";
import { EventIcon } from "./EventIcon";
import {
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  Bell,
  Bookmark,
  Calendar,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Circle,
  Clock,
  Crosshair,
  Download,
  EllipsisVertical,
  Eye,
  Film,
  FolderOpen,
  Gamepad2,
  HardDrive,
  Info,
  Keyboard,
  LayoutGrid,
  List,
  ListFilter,
  Maximize,
  Mic,
  Minimize,
  Monitor,
  Pause,
  Pencil,
  Play,
  Plus,
  Power,
  RefreshCw,
  ScanSearch,
  Scissors,
  Search,
  Settings,
  SkipBack,
  SkipForward,
  SlidersHorizontal,
  Sparkles,
  Square,
  Star,
  StepBack,
  StepForward,
  Trash2,
  TriangleAlert,
  Video,
  Volume2,
  VolumeX,
  X,
  type LucideIcon,
} from "lucide-react";

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

const ICONS: Record<string, LucideIcon> = {
  library: Gamepad2,
  clips: Film,
  studio: ScanSearch,
  games: LayoutGrid,
  settings: Settings,
  play: Play,
  pause: Pause,
  back: ArrowLeft,
  close: X,
  star: Star,
  trash: Trash2,
  folder: FolderOpen,
  scissors: Scissors,
  marker: Bookmark,
  refresh: RefreshCw,
  plus: Plus,
  edit: Pencil,
  volume: Volume2,
  mute: VolumeX,
  up: ArrowUp,
  "chev-down": ChevronDown,
  "chev-up": ChevronUp,
  "chev-right": ChevronRight,
  down: ArrowDown,
  record: Circle,
  stop: Square,
  maximize: Maximize,
  minimize: Minimize,
  warning: TriangleAlert,
  "frame-back": StepBack,
  "frame-fwd": StepForward,
  "skip-back": SkipBack,
  "skip-fwd": SkipForward,
  search: Search,
  list: List,
  grid: LayoutGrid,
  more: EllipsisVertical,
  check: Check,
  calendar: Calendar,
  clock: Clock,
  storage: HardDrive,
  video: Video,
  monitor: Monitor,
  keyboard: Keyboard,
  bell: Bell,
  mic: Mic,
  power: Power,
  info: Info,
  sliders: SlidersHorizontal,
  download: Download,
  filter: ListFilter,
  sparkles: Sparkles,
  target: Crosshair,
  eye: Eye,
};

export function Icon({ name, size = 18 }: { name: string; size?: number }) {
  const C = ICONS[name] ?? Circle;
  return <C size={size} strokeWidth={1.75} aria-hidden className="icon" />;
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
