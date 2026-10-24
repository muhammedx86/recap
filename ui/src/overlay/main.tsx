// The in-game notification window (src-tauri/src/overlay.rs): a transparent, click-through
// window that stacks short toasts in one corner. Open /overlay.html?demo in a browser to see it.
import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Bookmark, Circle, Film, SkipForward, Check } from "lucide-react";
import "./overlay.css";
import type { OverlayToast } from "../lib/types";

const SHOW_MS = 4000;
const isTauri = "__TAURI_INTERNALS__" in window;

interface Item extends OverlayToast {
  id: number;
  leaving: boolean;
}

const ICONS = {
  recording: Circle,
  saved: Check,
  bookmark: Bookmark,
  clips: Film,
  skipped: SkipForward,
} as const;

const DEMO: OverlayToast[] = [
  { kind: "recording", title: "Recording", detail: "VALORANT", corner: "top-right" },
  { kind: "bookmark", title: "Bookmark added", detail: "12:31", corner: "top-right" },
  { kind: "clips", title: "3 clips saved", detail: "VALORANT", corner: "top-right" },
];

function Overlay() {
  const [items, setItems] = useState<Item[]>([]);
  const demo = !isTauri && new URLSearchParams(location.search).has("demo");

  useEffect(() => {
    let next = 1;
    const push = (t: OverlayToast) => {
      const id = next++;
      setItems((xs) => [...xs.slice(-3), { ...t, id, leaving: false }]);
      if (demo) return;
      setTimeout(() => setItems((xs) => xs.map((x) => (x.id === id ? { ...x, leaving: true } : x))), SHOW_MS);
      setTimeout(() => setItems((xs) => xs.filter((x) => x.id !== id)), SHOW_MS + 400);
    };
    if (demo) {
      DEMO.forEach(push);
      return;
    }
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/event").then(({ listen }) =>
      listen<OverlayToast>("overlay-toast", (e) => push(e.payload)).then((u) => (unlisten = u)),
    );
    return () => unlisten?.();
  }, [demo]);

  const corner = items[items.length - 1]?.corner ?? "top-right";
  return (
    <div className={`ov-stack ${corner}`} aria-live="polite">
      {items.map((t) => {
        const Icon = ICONS[t.kind];
        return (
          <div key={t.id} className={`ov-toast k-${t.kind} ${t.leaving ? "leaving" : ""}`}>
            <span className="ov-icon">
              <Icon size={14} strokeWidth={2.25} fill={t.kind === "recording" ? "currentColor" : "none"} />
            </span>
            <span className="ov-text">
              <span className="ov-title">{t.title}</span>
              {t.detail && <span className="ov-detail">{t.detail}</span>}
            </span>
          </div>
        );
      })}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Overlay />
  </StrictMode>,
);
