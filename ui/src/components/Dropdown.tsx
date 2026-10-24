import { useEffect, useRef, useState, type ReactNode } from "react";
import { Icon } from "./ui";

export interface DropdownOption {
  value: string;
  label: string;
  icon?: ReactNode;
}

/**
 * Filter dropdown: a quiet button showing the current choice and a menu of options.
 * `value` null means "all" (shows `allLabel`); choosing the all item or "Clear filter" resets it.
 */
export function Dropdown({
  value,
  options,
  allLabel,
  onChange,
  label,
}: {
  value: string | null;
  options: DropdownOption[];
  allLabel: string;
  onChange: (v: string | null) => void;
  label: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  const current = options.find((o) => o.value === value);
  const pick = (v: string | null) => {
    onChange(v);
    setOpen(false);
  };
  return (
    <div className={`dd ${open ? "open" : ""}`} ref={ref}>
      <button type="button" className={`dd-btn ${current ? "set" : ""}`} aria-haspopup="listbox" aria-expanded={open} aria-label={label} onClick={() => setOpen(!open)}>
        <span className="dd-value">
          {current?.icon}
          {current?.label ?? allLabel}
        </span>
        <Icon name="chev-down" size={13} />
      </button>
      {open && (
        <div className="dd-menu" role="listbox" aria-label={label}>
          <button type="button" role="option" aria-selected={!current} className={`dd-item ${!current ? "on" : ""}`} onClick={() => pick(null)}>
            {allLabel}
          </button>
          {options.map((o) => (
            <button key={o.value} type="button" role="option" aria-selected={o.value === value} className={`dd-item ${o.value === value ? "on" : ""}`} onClick={() => pick(o.value)}>
              {o.icon}
              {o.label}
            </button>
          ))}
          {current && (
            <>
              <div className="dd-sep" />
              <button type="button" className="dd-item dd-clear" onClick={() => pick(null)}>
                Clear filter
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** A small "⋮" menu of actions. */
export function MoreMenu({ items, label = "More actions" }: { items: Array<{ label: string; icon?: string; onClick: () => void; danger?: boolean; disabled?: boolean }>; label?: string }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);
  return (
    <div className={`dd more ${open ? "open" : ""}`} ref={ref}>
      <button
        type="button"
        className="icon-btn"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setOpen(!open);
        }}
      >
        <Icon name="more" size={16} />
      </button>
      {open && (
        <div className="dd-menu dd-menu-right" role="menu">
          {items.map((it) => (
            <button
              key={it.label}
              type="button"
              role="menuitem"
              disabled={it.disabled}
              className={`dd-item ${it.danger ? "dd-danger" : ""}`}
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setOpen(false);
                it.onClick();
              }}
            >
              {it.icon && <Icon name={it.icon} size={14} />}
              {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
