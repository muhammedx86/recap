import { useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "../lib/api";

/**
 * End-to-end self-test only (docs/E2E.md): when the backend finishes its checks it emits
 * `selftest-tour`; walk every page, holding each for a few seconds and reporting it so the
 * driver script can screenshot the real app. Renders nothing and does nothing otherwise.
 */
export function SelfTestTour() {
  const navigate = useNavigate();
  useEffect(() => {
    if (!isTauri) return;
    let stop = () => {};
    let cancelled = false;
    invoke<boolean>("selftest_enabled")
      .then((on) => {
        if (!on || cancelled) return;
        return listen<{ matchId: string }>("selftest-tour", async (e) => {
          const pages: [string, string][] = [
            ["library", "/"],
            ["match", e.payload.matchId ? `/match/${e.payload.matchId}` : "/"],
            ["clips", "/clips"],
            ["studio", "/studio"],
            ["games", "/games"],
            ["settings", "/settings"],
          ];
          for (const [name, path] of pages) {
            navigate(path);
            await new Promise((r) => setTimeout(r, 1500));
            await invoke("selftest_tour_step", { page: name });
            await new Promise((r) => setTimeout(r, 3500));
          }
          await invoke("selftest_tour_step", { page: "done" });
        }).then((un) => {
          stop = un;
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop();
    };
  }, [navigate]);
  return null;
}
