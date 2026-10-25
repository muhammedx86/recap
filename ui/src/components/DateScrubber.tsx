// A rail on the right edge of the library: month (or day) ticks placed where those matches sit in
// the list, a hover label with the date under the pointer, and click/drag to jump there.
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

interface Item {
  id: string;
  startedUnixMs: number;
}

interface Props {
  /** The list element; each child must carry `data-id` matching an item. */
  listRef: RefObject<HTMLElement | null>;
  items: Item[];
}

interface Tick {
  key: string;
  label: string;
  /** Fraction down the list where the group's first match sits. */
  at: number;
  major: boolean;
}

const MONTH = (d: Date) => d.toLocaleDateString(undefined, { month: "short" });
/** Scroll the target this far below the top of the scroller, so it isn't flush with the edge. */
const LANDING_PX = 16;

function scroller(el: HTMLElement | null): HTMLElement | null {
  for (let p = el?.parentElement ?? null; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if ((o === "auto" || o === "scroll") && p.scrollHeight > p.clientHeight) return p;
  }
  return null;
}

export function DateScrubber({ listRef, items }: Props) {
  const rail = useRef<HTMLDivElement>(null);
  const [ticks, setTicks] = useState<Tick[]>([]);
  const [view, setView] = useState<{ top: number; height: number } | null>(null);
  const [hover, setHover] = useState<{ y: number; label: string } | null>(null);
  const [usable, setUsable] = useState(false);
  const dragging = useRef(false);

  /** Each item's top within the list, and the list height. */
  const layout = useCallback(() => {
    const list = listRef.current;
    if (!list) return null;
    const byId = new Map(items.map((i) => [i.id, i]));
    const tops: Array<{ top: number; item: Item }> = [];
    for (const child of Array.from(list.children) as HTMLElement[]) {
      const it = byId.get(child.dataset.id ?? "");
      if (it) tops.push({ top: child.offsetTop - list.offsetTop, item: it });
    }
    return { tops, height: Math.max(1, list.scrollHeight) };
  }, [listRef, items]);

  const measure = useCallback(() => {
    const l = layout();
    const sc = scroller(listRef.current);
    const list = listRef.current;
    if (!l || !list || l.tops.length < 2) {
      setUsable(false);
      return;
    }
    const first = l.tops[0].item.startedUnixMs;
    const last = l.tops[l.tops.length - 1].item.startedUnixMs;
    const spanDays = Math.abs(first - last) / 86_400_000;
    const byMonth = spanDays > 45;
    const out: Tick[] = [];
    let prevKey = "";
    let prevYear = -1;
    for (const { top, item } of l.tops) {
      const d = new Date(item.startedUnixMs);
      const key = byMonth ? `${d.getFullYear()}-${d.getMonth()}` : d.toDateString();
      if (key === prevKey) continue;
      prevKey = key;
      const newYear = d.getFullYear() !== prevYear;
      prevYear = d.getFullYear();
      out.push({
        key,
        label: byMonth ? (newYear && out.length > 0 ? String(d.getFullYear()) : MONTH(d)) : d.toLocaleDateString(undefined, { month: "short", day: "numeric" }),
        at: top / l.height,
        major: byMonth ? newYear : d.getDate() === 1,
      });
    }
    // Drop labels that would collide on the rail (keep year changes).
    const h = rail.current?.clientHeight ?? 400;
    let lastY = -Infinity;
    const spaced = out.filter((t) => {
      const y = t.at * h;
      if (y - lastY < 18 && !t.major) return false;
      lastY = y;
      return true;
    });
    setTicks(spaced);
    // Worth showing only when the list is longer than the viewport.
    setUsable(!!sc && sc.scrollHeight > sc.clientHeight * 1.35 && out.length >= 2);
    if (sc) {
      const listTop = list.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
      const top = (sc.scrollTop - listTop) / l.height;
      setView({ top: Math.max(0, top), height: Math.min(1, sc.clientHeight / l.height) });
    }
  }, [layout, listRef]);

  useLayoutEffect(() => {
    measure();
  }, [measure]);

  useEffect(() => {
    const list = listRef.current;
    const sc = scroller(list) ?? list?.closest<HTMLElement>(".main") ?? null;
    if (!list) return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(list);
    if (sc) ro.observe(sc);
    const onScroll = () => measure();
    sc?.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      ro.disconnect();
      sc?.removeEventListener("scroll", onScroll);
    };
  }, [listRef, measure]);

  const fracAt = (clientY: number) => {
    const r = rail.current!.getBoundingClientRect();
    return Math.max(0, Math.min(1, (clientY - r.top) / r.height));
  };

  /** The match at a fraction down the list. */
  const itemAt = (f: number) => {
    const l = layout();
    if (!l || !l.tops.length) return null;
    const y = f * l.height;
    let pick = l.tops[0];
    for (const t of l.tops) {
      if (t.top <= y) pick = t;
      else break;
    }
    return { ...pick, height: l.height };
  };

  const jump = (f: number, smooth: boolean) => {
    const list = listRef.current;
    const sc = scroller(list);
    const hit = itemAt(f);
    if (!list || !sc || !hit) return;
    const listTop = list.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
    sc.scrollTo({ top: listTop + hit.top - LANDING_PX, behavior: smooth ? "smooth" : "auto" });
  };

  const label = (f: number) => {
    const hit = itemAt(f);
    if (!hit) return "";
    return new Date(hit.item.startedUnixMs).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
  };

  if (!usable) return null;

  return (
    <div
      ref={rail}
      className="date-scrubber"
      role="scrollbar"
      aria-orientation="vertical"
      aria-label="Jump to a date"
      aria-valuenow={Math.round((view?.top ?? 0) * 100)}
      onMouseMove={(e) => {
        const f = fracAt(e.clientY);
        setHover({ y: f * rail.current!.clientHeight, label: label(f) });
        if (dragging.current) jump(f, false);
      }}
      onMouseLeave={() => {
        setHover(null);
        dragging.current = false;
      }}
      onMouseDown={(e) => {
        e.preventDefault();
        dragging.current = true;
        jump(fracAt(e.clientY), true);
      }}
      onMouseUp={() => (dragging.current = false)}
    >
      <div className="ds-line" />
      {view && <div className="ds-view" style={{ top: `${view.top * 100}%`, height: `${Math.max(2, view.height * 100)}%` }} />}
      {ticks.map((t) => (
        <div key={t.key} className={`ds-tick ${t.major ? "major" : ""}`} style={{ top: `${t.at * 100}%` }}>
          <span className="ds-tick-label">{t.label}</span>
        </div>
      ))}
      {hover && (
        <div className="ds-hover num" style={{ top: hover.y }}>
          {hover.label}
        </div>
      )}
    </div>
  );
}
