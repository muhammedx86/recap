import { HashRouter, Route, Routes } from "react-router-dom";
import { Sidebar } from "./components/Sidebar";
import { ToastProvider } from "./lib/toast";
import { Library } from "./pages/Library";
import { MatchPage } from "./pages/Match";
import { Clips } from "./pages/Clips";
import { Studio } from "./pages/Studio";
import { Games } from "./pages/Games";
import { SettingsPage } from "./pages/Settings";

export function App() {
  return (
    <ToastProvider>
      <HashRouter>
        <div className="shell">
          <Sidebar />
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
      </HashRouter>
    </ToastProvider>
  );
}
