import { useEffect, useState } from "react";
import { NavLink } from "react-router-dom";
import { api, on } from "../lib/api";
import { bytes, timecode } from "../lib/format";
import { useNow, useStatus } from "../lib/hooks";
import { useToast } from "../lib/toast";
import type { StorageInfo } from "../lib/types";
import { Icon } from "./ui";

const SECTIONS = [
  {
    label: "Library",
    links: [
      { to: "/", label: "Matches", icon: "library", end: true },
      { to: "/clips", label: "Clips", icon: "clips" },
    ],
  },
  {
    label: "Tools",
    links: [
      { to: "/studio", label: "Detector Studio", icon: "studio" },
      { to: "/games", label: "Games", icon: "games" },
    ],
  },
  {
    label: "System",
    links: [{ to: "/settings", label: "Settings", icon: "settings" }],
  },
];

export function Sidebar() {
  const storage = useStorage();
  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-mark" aria-hidden />
        <span className="brand-name">Recap</span>
      </div>
      <nav aria-label="Main">
        {SECTIONS.map((s) => (
          <div key={s.label} className="nav-section">
            <div className="nav-label">{s.label}</div>
            <div className="nav">
              {s.links.map((n) => (
                <NavLink key={n.to} to={n.to} end={n.end} className={({ isActive }) => `nav-link ${isActive ? "active" : ""}`}>
                  <Icon name={n.icon} size={15} />
                  {n.label}
                </NavLink>
              ))}
            </div>
          </div>
        ))}
      </nav>
      {storage && <StorageStats s={storage} />}
      <RecordingCard />
    </aside>
  );
}

/** Below this much free space on the recordings drive, warn before recordings start failing. */
const LOW_DISK_BYTES = 15e9;

function useStorage(): StorageInfo | null {
  const [s, setS] = useState<StorageInfo | null>(null);
  useEffect(() => {
    let alive = true;
    const check = () =>
      api
        .getStorage()
        .then((x) => alive && setS(x))
        .catch(() => {});
    check();
    const timer = setInterval(check, 5 * 60_000);
    const off = on("library-changed", check);
    return () => {
      alive = false;
      clearInterval(timer);
      off();
    };
  }, []);
  return s;
}

function StorageStats({ s }: { s: StorageInfo }) {
  const low = s.freeBytes < LOW_DISK_BYTES;
  const total = s.usedBytes + s.freeBytes;
  return (
    <div className="side-stats">
      <div className="nav-label">Storage</div>
      <div className="side-stat">
        <span>Recordings</span>
        <span className="num">{bytes(s.usedBytes)}</span>
      </div>
      <div className={`side-meter ${low ? "low" : ""}`} aria-hidden>
        <span style={{ width: `${total > 0 ? Math.max(2, (s.usedBytes / total) * 100) : 0}%` }} />
      </div>
      <div className="side-hint num">{bytes(s.freeBytes)} free on this drive</div>
      <div className="side-stat">
        <span>Matches</span>
        <span className="num">{s.matchCount}</span>
      </div>
      <div className="side-stat">
        <span>Clips</span>
        <span className="num">{s.clipCount}</span>
      </div>
      {low && (
        <NavLink to="/settings" className="disk-warn" title="Recordings stop when the drive is full. Free some space or set a storage limit in Settings.">
          <Icon name="warning" size={14} />
          <span>Low disk space</span>
        </NavLink>
      )}
    </div>
  );
}

function RecordingCard() {
  const status = useStatus();
  const toast = useToast();
  const recording = status?.state === "recording";
  const now = useNow(recording);
  if (!status) return null;

  if (status.state === "unavailable") {
    return (
      <div className="rec">
        <div className="rec-row">
          <span className="rec-icon">
            <Icon name="warning" size={15} />
          </span>
          <div className="rec-text">
            <span className="rec-title">Recorder unavailable</span>
            {status.message && <span className="rec-meta">{status.message}</span>}
          </div>
        </div>
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
          <span className="rec-icon">
            <Icon name="video" size={15} />
          </span>
          <div className="rec-text">
            <span className="rec-title">{status.waitingFor ? "Waiting for a match" : "Ready to record"}</span>
            <span className="rec-meta ellipsis">{status.waitingFor ? `${status.waitingFor} is open` : "Starts with your game"}</span>
          </div>
          <span className={`rec-dot ${status.waitingFor ? "armed" : "idle"}`} aria-hidden />
        </div>
        <div className="rec-actions">
          <button className="btn btn-small" onClick={() => api.startManualRecording().catch((e) => toast.error(e, "Could not start recording"))}>
            <Icon name="record" size={12} />
            Record now
          </button>
        </div>
      </div>
    );
  }

  const elapsed = status.startedUnixMs ? now - status.startedUnixMs : 0;
  const label = status.state === "recording" ? (status.gameName ?? "Recording") : status.state === "starting" ? "Starting…" : "Stopping…";
  return (
    <div className="rec live" aria-live="polite">
      <div className="rec-row">
        <span className="rec-icon">
          <span className="rec-dot" aria-hidden />
        </span>
        <div className="rec-text">
          <span className="rec-title ellipsis">{label}</span>
          <span className="rec-meta">
            {status.bookmarks > 0 ? `${status.bookmarks} ${status.bookmarks === 1 ? "bookmark" : "bookmarks"}` : "Recording"}
          </span>
        </div>
        <span className="rec-time num">{timecode(elapsed)}</span>
      </div>
      <div className="rec-actions">
        <button className="btn btn-small" onClick={() => api.addBookmark().catch((e) => toast.error(e))} title="Bookmark this moment">
          <Icon name="marker" size={12} />
          Bookmark
        </button>
        <button className="btn btn-small" onClick={() => api.stopRecording().catch((e) => toast.error(e, "Could not stop"))} title="Stop recording">
          <Icon name="stop" size={12} />
          Stop
        </button>
      </div>
    </div>
  );
}
