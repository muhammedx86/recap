import { NavLink } from "react-router-dom";
import { api } from "../lib/api";
import { timecode } from "../lib/format";
import { useNow, useStatus } from "../lib/hooks";
import { useToast } from "../lib/toast";
import { Icon } from "./ui";

const NAV = [
  { to: "/", label: "Library", icon: "library", end: true },
  { to: "/clips", label: "Clips", icon: "clips" },
  { to: "/studio", label: "Studio", icon: "studio" },
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
            <Icon name={n.icon} size={16} />
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
      <div className="rec">
        <div className="rec-row">
          <span className="rec-dot idle" aria-hidden />
          <span className="rec-title">Recorder unavailable</span>
        </div>
        {status.message && <p className="rec-meta">{status.message}</p>}
      </div>
    );
  }

  if (!recording && status.state !== "starting" && status.state !== "stopping") {
    return (
      <div className="rec">
        <div
          className="rec-row"
          title={
            status.waitingFor
              ? "Each match is recorded on its own; recording starts when the next match begins"
              : "Recording starts on its own when an enabled game launches"
          }
        >
          <span className={`rec-dot ${status.waitingFor ? "armed" : "idle"}`} aria-hidden />
          <span className="rec-title">{status.waitingFor ? "Waiting for a match" : "Waiting for a game"}</span>
        </div>
        {status.waitingFor && <p className="rec-meta">{status.waitingFor} is open</p>}
        <div className="rec-actions">
          <button className="btn btn-small btn-ghost" onClick={() => api.startManualRecording().catch((e) => toast.error(e, "Could not start recording"))}>
            <Icon name="record" size={13} />
            Record now
          </button>
        </div>
      </div>
    );
  }

  const elapsed = status.startedUnixMs ? now - status.startedUnixMs : 0;
  const label = status.state === "recording" ? (status.gameName ?? "Recording") : status.state === "starting" ? "Starting…" : "Stopping…";
  return (
    <div className="rec" aria-live="polite">
      <div className="rec-row">
        <span className="rec-dot" aria-hidden />
        <span className="rec-title ellipsis">{label}</span>
        <span className="rec-time num">{timecode(elapsed)}</span>
      </div>
      {status.bookmarks > 0 && (
        <p className="rec-meta">
          {status.bookmarks} {status.bookmarks === 1 ? "bookmark" : "bookmarks"}
        </p>
      )}
      <div className="rec-actions">
        <button className="btn btn-small btn-ghost" onClick={() => api.addBookmark().catch((e) => toast.error(e))} title="Bookmark this moment">
          <Icon name="marker" size={13} />
          Bookmark
        </button>
        <button className="btn btn-small btn-ghost" onClick={() => api.stopRecording().catch((e) => toast.error(e, "Could not stop"))} title="Stop recording">
          <Icon name="stop" size={13} />
          Stop
        </button>
      </div>
    </div>
  );
}
