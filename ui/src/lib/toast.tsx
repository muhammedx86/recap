import { createContext, useCallback, useContext, useState, type ReactNode } from "react";
import { errorMessage } from "./api";

interface Toast {
  id: number;
  text: string;
  tone: "info" | "error";
}

interface ToastApi {
  info: (text: string) => void;
  error: (e: unknown, prefix?: string) => void;
}

const Ctx = createContext<ToastApi>({ info: () => {}, error: () => {} });

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((text: string, tone: Toast["tone"]) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t.slice(-3), { id, text, tone }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), tone === "error" ? 7000 : 3500);
  }, []);
  const api: ToastApi = {
    info: (text) => push(text, "info"),
    error: (e, prefix) => push(prefix ? `${prefix}: ${errorMessage(e)}` : errorMessage(e), "error"),
  };
  return (
    <Ctx.Provider value={api}>
      {children}
      <div className="toasts" role="status" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast-${t.tone}`}>
            {t.text}
            <button className="toast-close" aria-label="Dismiss" onClick={() => setToasts((x) => x.filter((y) => y.id !== t.id))}>
              ×
            </button>
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}

export const useToast = () => useContext(Ctx);
