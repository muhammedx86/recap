import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorMessage, isTauri, on } from "./api";
import * as mock from "./mock";
import type { RecorderStatus } from "./types";

/** Load async data with loading/error state and a reload function. */
export function useLoad<T>(fn: () => Promise<T>, deps: unknown[] = []) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const seq = useRef(0);
  const reload = useCallback(() => {
    const id = ++seq.current;
    setLoading(true);
    return fn()
      .then((d) => {
        if (id === seq.current) {
          setData(d);
          setError(null);
        }
      })
      .catch((e) => {
        if (id === seq.current) setError(errorMessage(e));
      })
      .finally(() => {
        if (id === seq.current) setLoading(false);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  useEffect(() => {
    reload();
  }, [reload]);
  return { data, setData, error, loading, reload };
}

export function useStatus(): RecorderStatus | null {
  const [status, setStatus] = useState<RecorderStatus | null>(null);
  useEffect(() => {
    api.getStatus().then(setStatus).catch(() => setStatus(null));
    return on("status-changed", setStatus);
  }, []);
  return status;
}

/** Ticks every `ms` while `active`, returning Date.now(). */
export function useNow(active: boolean, ms = 1000): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [active, ms]);
  return now;
}

/** Map of matchId -> processing progress (0..1), kept live from processing-progress events. */
export function useProcessing(): Record<string, number> {
  const [map, setMap] = useState<Record<string, number>>(() => (isTauri ? {} : mock.processingSnapshot()));
  useEffect(
    () =>
      on("processing-progress", (p) => {
        setMap((m) => {
          const next = { ...m };
          if (p.stage === "done" || p.stage === "error") delete next[p.matchId];
          else next[p.matchId] = p.progress;
          return next;
        });
      }),
    [],
  );
  return map;
}

/** Reload when the library changes (optionally only for one match). */
export function useLibraryChanged(cb: () => void, matchId?: string) {
  const ref = useRef(cb);
  ref.current = cb;
  useEffect(
    () =>
      on("library-changed", (p) => {
        if (!matchId || !p.matchId || p.matchId === matchId) ref.current();
      }),
    [matchId],
  );
}
