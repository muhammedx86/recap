import { useEffect } from "react";
import { HashRouter, Route, Routes, useLocation } from "react-router-dom";
import { useMatchPanel } from "./lib/layout";
import { refreshEventTypes, useEventTypes } from "./lib/kinds";
import { Sidebar } from "./components/Sidebar";
import { ToastProvider } from "./lib/toast";
import { Library } from "./pages/Library";
import { MatchPage } from "./pages/Match";
import { Clips } from "./pages/Clips";
import { Studio } from "./pages/Studio";
import { Games } from "./pages/Games";
import { SettingsPage } from "./pages/Settings";
import { SelfTestTour } from "./components/SelfTestTour";
import { useMediaGuard } from "./lib/media";

export function App() {
  // Subscribing here re-renders every screen when event types load or change.
  useEventTypes();
  useMediaGuard();
  useEffect(() => {
    refreshEventTypes();
  }, []);
  return (
    <ToastProvider>
      <HashRouter>
        <SelfTestTour />
        <Shell />
      </HashRouter>
    </ToastProvider>
  );
}

/** The Match page gives the video the room: the app sidebar is hidden there unless you bring it back. */
function Shell() {
  const onMatch = useLocation().pathname.startsWith("/match/");
  const navOpen = useMatchPanel("nav");
  const hideNav = onMatch && !navOpen;
  return (
    <div className={`shell ${hideNav ? "nav-hidden" : ""}`}>
      {!hideNav && <Sidebar />}
      <main className="main">
        <Routes>
          <Route path="/" element={<Library />} />
          <Route path="/match/:id" element={<MatchPage />} />
          <Route path="/clips" element={<Clips />} />
          <Route path="/studio" element={<Studio />} />
          <Route path="/games" element={<Games />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="*" element={<Library />} />
        </Routes>
      </main>
    </div>
  );
}
