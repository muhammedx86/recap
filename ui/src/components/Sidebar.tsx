import { NavLink } from "react-router-dom";
import { api } from "../lib/api";
import { timecode } from "../lib/format";
import { useNow, useStatus } from "../lib/hooks";
import { useToast } from "../lib/toast";
import { Icon } from "./ui";

const NAV = [
  { to: "/", label: "Library", icon: "library", end: true },
  { to: "/clips", label: "Clips", icon: "clips" },
  { to: "/studio", label: "Detector Studio", icon: "studio" },
  { to: "/games", label: "Games", icon: "games" },
  { to: "/settings", label: "Settings", icon: "settings" },
];

export function Sidebar() {
  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-mark" aria-hidden />
        <span className="brand-name">Recap</span>
      </div>
      <nav className="nav" aria-label="Main">
        {NAV.map((n) => (
          <NavLink key={n.to} to={n.to} end={n.end} className={({ isActive }) => `nav-link ${isActive ? "active" : ""}`}>
            <Icon name={n.icon} />
            {n.label}
          </NavLink>
        ))}
      </nav>
      <RecordingIndicator />
    </aside>
  );
}

function RecordingIndicator() {
  const status = useStatus();
  const toast = useToast();
  const recording = status?.state === "recording";
  const now = useNow(recording);
  if (!status) return null;

  if (status.state === "unavailable") {
    return (
      <div className="rec rec-off">
        <p className="rec-title">Recorder unavailable</p>
        <p className="rec-meta">{status.message ?? "The recording engine did not start."}</p>
      </div>
    );
  }

  if (!recording && status.state !== "starting" && status.state !== "stopping") {
    return (
      <div className="rec rec-idle">
        <p className="rec-title">Waiting for a game</p>
        <p className="rec-meta">Recording starts on its own when an enabled game launches.</p>
        <button
          className="btn btn-small"
          onClick={() => api.startManualRecording().catch((e) => toast.error(e, "Could not start recording"))}
        >
          <Icon name="record" size={14} />
          Record now
        </button>
      </div>
    );
  }

  const elapsed = status.startedUnixMs ? now - status.startedUnixMs : 0;
  return (
    <div className="rec rec-live" aria-live="polite">
      <div className="rec-head">
        <span className="rec-dot" aria-hidden />
        <span className="rec-state">{status.state === "recording" ? "Recording" : status.state === "starting" ? "Starting" : "Stopping"}</span>
      </div>
      <p className="rec-game">{status.gameName ?? "Recording"}</p>
      <p className="rec-time num">{timecode(elapsed)}</p>
      <p className="rec-meta">
        {status.bookmarks} {status.bookmarks === 1 ? "bookmark" : "bookmarks"}
        {status.encoder ? `, ${status.encoder}` : ""}
      </p>
      <div className="rec-actions">
        <button className="btn btn-small" onClick={() => api.addBookmark().catch((e) => toast.error(e))} title="Bookmark this moment">
          <Icon name="marker" size={14} />
          Bookmark
        </button>
        <button className="btn btn-small btn-ghost" onClick={() => api.stopRecording().catch((e) => toast.error(e, "Could not stop"))} title="Stop recording">
          <Icon name="stop" size={14} />
          Stop
        </button>
      </div>
    </div>
  );
}
