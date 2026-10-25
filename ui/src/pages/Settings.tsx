import { useEffect, useState, type ReactNode } from "react";
import { api, pickPath } from "../lib/api";
import { bytes } from "../lib/format";
import { useLoad } from "../lib/hooks";
import { COMMON_KINDS, kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { EncoderChoice, Settings } from "../lib/types";
import { Icon, KindDot, ProgressBar, Toggle } from "../components/ui";
import { loadPref, savePref } from "../lib/media";

const ENCODER_LABEL: Record<string, string> = {
  auto: "Automatic",
  nvenc: "NVIDIA NVENC",
  amf: "AMD AMF",
  qsv: "Intel Quick Sync",
  x264: "x264 (CPU)",
};

const HEIGHTS: Array<[number | null, string]> = [
  [null, "Native"],
  [2160, "2160p"],
  [1440, "1440p"],
  [1080, "1080p"],
  [720, "720p"],
];

type Tab = "general" | "recording" | "audio" | "clipping" | "detection" | "hotkeys" | "storage";
const TABS: Array<[Tab, string, string]> = [
  ["general", "General", "sliders"],
  ["recording", "Recording", "video"],
  ["audio", "Audio", "volume"],
  ["clipping", "Clipping", "scissors"],
  ["detection", "Detection", "target"],
  ["hotkeys", "Hotkeys", "keyboard"],
  ["storage", "Storage", "storage"],
];

export function SettingsPage() {
  const toast = useToast();
  const [tab, setTab] = useState<Tab>(() => loadPref<Tab>("settings.tab", "general"));
  const chooseTab = (t: Tab) => {
    setTab(t);
    savePref("settings.tab", t);
  };
  const { data: loaded, error } = useLoad(() => api.getSettings(), []);
  const { data: encoders } = useLoad(() => api.listEncoders(), []);
  const { data: mics } = useLoad(() => api.listAudioInputs(), []);
  const { data: storage, reload: reloadStorage } = useLoad(() => api.getStorage(), []);
  const [s, setS] = useState<Settings | null>(null);
  const [base, setBase] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (loaded) {
      setS(loaded);
      setBase(loaded);
    }
  }, [loaded]);

  if (error) return <div className="page"><p className="error-line">Could not load settings: {error}</p></div>;
  if (!s) return <div className="page" aria-busy="true" />;

  const dirty = JSON.stringify(s) !== JSON.stringify(base);
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setS({ ...s, [k]: v });
  /** Kinds with their own built-in timing (mirrors recap-core clips::default_timing). */
  const TIMED_KINDS: Array<[string, number, number]> = [
    ["kill", 6, 3],
    ["multikill", 6, 4],
    ["ace", 8, 5],
    ["death", 7, 2],
    ["assist", 5, 2],
    ["bookmark", 20, 3],
    ["objective", 8, 4],
    ["round_win", 6, 3],
  ];
  const setClip = <K extends keyof Settings["autoClip"]>(k: K, v: Settings["autoClip"][K]) => setS({ ...s, autoClip: { ...s.autoClip, [k]: v } });

  const save = async () => {
    setSaving(true);
    try {
      const saved = await api.updateSettings(s);
      setS(saved);
      setBase(saved);
      reloadStorage();
      toast.info("Settings saved");
    } catch (e) {
      toast.error(e, "Could not save settings");
    } finally {
      setSaving(false);
    }
  };

  const limitBytes = s.storageLimitGb ? s.storageLimitGb * 1e9 : null;

  return (
    <div className="page page-settings">
      <header className="page-head">
        <h1>Settings</h1>
        <p className="lede">Configure recording, clipping and storage</p>
      </header>

      <div className="set-tabs" role="tablist" aria-label="Settings sections">
        {TABS.map(([id, label, icon]) => (
          <button key={id} role="tab" aria-selected={tab === id} className={`chip ${tab === id ? "on" : ""}`} onClick={() => chooseTab(id)}>
            <Icon name={icon} size={14} />
            {label}
          </button>
        ))}
      </div>

      <Section title="Storage" icon="storage" desc="Where recordings go and how much space they may use" show={tab === "storage"}>
        <div className="set-block wide">
          <label className="set-label" htmlFor="set-dir">
            Recordings folder
          </label>
          <div className="input-row">
            <input id="set-dir" className="input" value={s.recordingsDir} onChange={(e) => set("recordingsDir", e.target.value)} placeholder="Videos\Recap" />
            <button
              className="btn"
              onClick={async () => {
                const p = await pickPath({ directory: true, title: "Choose the recordings folder" });
                if (p) set("recordingsDir", p);
              }}
            >
              Browse
            </button>
          </div>
          {storage && (
            <div className="storage">
              <ProgressBar value={limitBytes ? storage.usedBytes / limitBytes : storage.usedBytes / (storage.usedBytes + storage.freeBytes)} label="Storage used" />
              <p>
                <span className="num">{bytes(storage.usedBytes)}</span> used
                {limitBytes ? ` of ${s.storageLimitGb} GB` : ""} · {storage.matchCount} recordings, {storage.clipCount} clips · <span className="num">{bytes(storage.freeBytes)}</span> free
              </p>
            </div>
          )}
        </div>
        <Row label="Storage limit" hint="Oldest non-favourite recordings are deleted past this size." htmlFor="set-limit">
          <div className="input-row set-num">
            <input
              id="set-limit"
              type="number"
              className="input"
              min={1}
              value={s.storageLimitGb ?? ""}
              placeholder="None"
              onChange={(e) => set("storageLimitGb", e.target.value ? Math.max(1, Number(e.target.value)) : null)}
            />
            <span className="unit">GB</span>
          </div>
        </Row>
      </Section>

      <Section title="Startup" icon="power" desc="How Recap starts with Windows" show={tab === "general"}>
        <Row on={s.launchAtStartup} label="Launch with Windows">
          <Toggle checked={s.launchAtStartup} onChange={(v) => set("launchAtStartup", v)} label="Launch with Windows" />
        </Row>
        <Row on={s.startMinimized && s.launchAtStartup} label="Start in the tray" hint="When Windows starts Recap, keep the window closed.">
          <Toggle checked={s.startMinimized} onChange={(v) => set("startMinimized", v)} label="Start in the tray" disabled={!s.launchAtStartup} />
        </Row>
      </Section>

      <Section title="Video" icon="video" desc="Quality of new recordings" show={tab === "recording"}>
        <Row label="Frame rate" htmlFor="set-fps">
          <select id="set-fps" className="input set-select" value={s.fps} onChange={(e) => set("fps", Number(e.target.value))}>
            {[30, 60, 120].map((f) => (
              <option key={f} value={f}>
                {f} fps
              </option>
            ))}
          </select>
        </Row>
        <Row label="Resolution" htmlFor="set-res">
          <select id="set-res" className="input set-select" value={s.outputHeight ?? ""} onChange={(e) => set("outputHeight", e.target.value ? Number(e.target.value) : null)}>
            {HEIGHTS.map(([h, l]) => (
              <option key={l} value={h ?? ""}>
                {l}
              </option>
            ))}
          </select>
        </Row>
        <Row
          label="Quality"
          hint={
            s.rateControl === "cbr"
              ? "The same bitrate all the time: predictable file sizes."
              : "Constant quality: small files in menus, full detail in fights. High is sharper and bigger."
          }
          htmlFor="set-q"
        >
          <select
            id="set-q"
            className="input set-select"
            value={s.rateControl === "cbr" ? "cbr" : s.quality}
            onChange={(e) => {
              const v = e.target.value;
              setS(v === "cbr" ? { ...s, rateControl: "cbr" } : { ...s, rateControl: "quality", quality: v as Settings["quality"] });
            }}
          >
            <option value="high">High</option>
            <option value="medium">Medium</option>
            <option value="low">Low (smallest files)</option>
            <option value="cbr">Constant bitrate</option>
          </select>
        </Row>
        {s.rateControl === "cbr" && (
          <Row label="Bitrate" hint="12,000 kbps at 1080p60 is about 5 GB per hour." htmlFor="set-br">
            <div className="input-row set-num">
              <input id="set-br" type="number" className="input" min={1000} max={100000} step={500} value={s.bitrateKbps} onChange={(e) => set("bitrateKbps", Number(e.target.value))} />
              <span className="unit">kbps</span>
            </div>
          </Row>
        )}
        <Row
          label="Keep the seconds before a match"
          hint="While a game runs, Recap keeps a few seconds in short temporary files next to your recordings, so a match's opening is never cut off."
          htmlFor="set-pre"
        >
          <select id="set-pre" className="input set-select" value={s.prerollSecs} onChange={(e) => set("prerollSecs", Number(e.target.value))}>
            {[0, 5, 10, 15, 20, 30].map((v) => (
              <option key={v} value={v}>
                {v === 0 ? "Off" : `${v} seconds`}
              </option>
            ))}
          </select>
        </Row>
        <Row label="Encoder" htmlFor="set-enc">
          <select id="set-enc" className="input set-select" value={s.encoder} onChange={(e) => set("encoder", e.target.value as EncoderChoice)} title="Automatic picks your graphics card's encoder and falls back to the CPU.">
            {[...new Set(["auto", ...(encoders ?? [])])].map((e) => (
              <option key={e} value={e}>
                {ENCODER_LABEL[e] ?? e}
              </option>
            ))}
          </select>
        </Row>
        <Row
          on={s.gpuPreference}
          label="Record on the high-performance GPU"
          hint="Laptops with two graphics chips: record on the one games use, so frames are not copied between them. Applies after restarting Recap."
        >
          <Toggle checked={s.gpuPreference} onChange={(v) => set("gpuPreference", v)} label="High-performance GPU" />
        </Row>
      </Section>

      <Section title="Audio" icon="volume" desc="What sound is recorded" show={tab === "audio"}>
        <Row on={s.recordDesktopAudio} label="Game and desktop sound">
          <Toggle checked={s.recordDesktopAudio} onChange={(v) => set("recordDesktopAudio", v)} label="Record desktop audio" />
        </Row>
        <Row on={s.recordMicrophone} label="Microphone" hint="Mixed in with the game sound.">
          <Toggle checked={s.recordMicrophone} onChange={(v) => set("recordMicrophone", v)} label="Record microphone" />
        </Row>
        {s.recordMicrophone && (
          <Row label="Input device" htmlFor="set-mic">
            <select id="set-mic" className="input set-select" value={s.microphoneDevice ?? ""} onChange={(e) => set("microphoneDevice", e.target.value || null)}>
              <option value="">System default</option>
              {(mics ?? [])
                .filter((m) => m.id !== "default")
                .map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
            </select>
          </Row>
        )}
        {s.recordMicrophone && (
          <Row on={s.separateMicTrack} label="Also keep your voice on its own track" hint="Track 2, for video editors. Recap and clips play the full mix.">
            <Toggle checked={s.separateMicTrack} onChange={(v) => set("separateMicTrack", v)} label="Separate microphone track" />
          </Row>
        )}
      </Section>

      <Section title="Hotkeys" icon="keyboard" desc="Keys that work while you play" show={tab === "hotkeys"}>
        <Row label="Bookmark this moment" htmlFor="set-hk-b">
          <HotkeyInput id="set-hk-b" value={s.bookmarkHotkey} onChange={(v) => set("bookmarkHotkey", v)} />
        </Row>
        <Row label="Start or stop recording" htmlFor="set-hk-r">
          <HotkeyInput id="set-hk-r" value={s.manualRecordHotkey} onChange={(v) => set("manualRecordHotkey", v)} />
        </Row>
      </Section>

      <Section title="Notifications" icon="bell" desc="Small pop-ups over the game" show={tab === "general"}>
        <Row on={s.notifications} label="Show notifications over the game" hint="Recording started and saved, clips ready, skipped matches. Visible in borderless and windowed games, not in exclusive fullscreen.">
          <Toggle checked={s.notifications} onChange={(v) => set("notifications", v)} label="In-game notifications" />
        </Row>
        {s.notifications && (
          <>
            <Row label="Corner" htmlFor="set-corner">
              <select
                id="set-corner"
                className="input set-select"
                value={s.notificationCorner}
                onChange={(e) => set("notificationCorner", e.target.value as Settings["notificationCorner"])}
              >
                <option value="top-right">Top right</option>
                <option value="top-left">Top left</option>
                <option value="bottom-right">Bottom right</option>
                <option value="bottom-left">Bottom left</option>
              </select>
            </Row>
            <Row on={s.notifyOnBookmark} label="Include bookmarks">
              <Toggle checked={s.notifyOnBookmark} onChange={(v) => set("notifyOnBookmark", v)} label="Bookmark notifications" />
            </Row>
          </>
        )}
      </Section>

      <Section title="Auto-clips" icon="scissors" desc="Clips made for you after each match" show={tab === "clipping"}>
        <Row on={s.autoClip.enabled} label="Make clips after each match">
          <Toggle checked={s.autoClip.enabled} onChange={(v) => setClip("enabled", v)} label="Auto-clips" />
        </Row>
        <Row label="Seconds before / after" hint="For plays without their own timing below." htmlFor="set-cb">
          <div className="input-row set-pair">
            <input id="set-cb" type="number" className="input" min={0} max={60} step={0.5} value={s.autoClip.secondsBefore} onChange={(e) => setClip("secondsBefore", Number(e.target.value))} aria-label="Seconds before" />
            <input type="number" className="input" min={0} max={60} step={0.5} value={s.autoClip.secondsAfter} onChange={(e) => setClip("secondsAfter", Number(e.target.value))} aria-label="Seconds after" />
          </div>
        </Row>
        <Row label="Join plays closer than" hint="Back-to-back kills become one clip." htmlFor="set-cg">
          <div className="input-row set-num">
            <input id="set-cg" type="number" className="input" min={0} max={30} step={0.5} value={s.autoClip.mergeGapSeconds} onChange={(e) => setClip("mergeGapSeconds", Number(e.target.value))} />
            <span className="unit">s</span>
          </div>
        </Row>
        <Row on={s.autoClip.preciseCuts} label="Frame-accurate cuts" hint="Slower; re-encodes each clip.">
          <Toggle checked={s.autoClip.preciseCuts} onChange={(v) => setClip("preciseCuts", v)} label="Frame-accurate cuts" />
        </Row>
        <div className="set-block wide">
          <p className="set-label">Seconds around each kind of play</p>
          <p className="field-hint">A death is mostly what led to it; a bookmark keeps what just happened. Multikills run from the first kill to the last. Empty = default.</p>
          <div className="timing-grid" role="table" aria-label="Clip timing per play">
            <span className="timing-h" role="columnheader">Play</span>
            <span className="timing-h" role="columnheader">Before</span>
            <span className="timing-h" role="columnheader">After</span>
            {TIMED_KINDS.map(([k, db, da]) => {
              const t = s.autoClip.timing?.[k];
              const setT = (field: "before" | "after", raw: string) => {
                const next = { ...(s.autoClip.timing ?? {}) };
                const cur = next[k] ?? { before: db, after: da };
                if (raw === "") {
                  const other = field === "before" ? t?.after : t?.before;
                  if (other === undefined || other === (field === "before" ? da : db)) delete next[k];
                  else next[k] = { ...cur, [field]: field === "before" ? db : da };
                } else next[k] = { ...cur, [field]: Math.max(0, Math.min(120, Number(raw))) };
                setClip("timing", next);
              };
              return (
                <div key={k} className="timing-row" role="row">
                  <span className="timing-kind" role="cell">
                    <KindDot kind={k} tone="color" size={13} /> {kindStyle(k).label}
                  </span>
                  <input className="input input-small" type="number" min={0} max={120} step={0.5} placeholder={String(db)} value={t?.before ?? ""} onChange={(e) => setT("before", e.target.value)} aria-label={`${kindStyle(k).label}: seconds before`} />
                  <input className="input input-small" type="number" min={0} max={120} step={0.5} placeholder={String(da)} value={t?.after ?? ""} onChange={(e) => setT("after", e.target.value)} aria-label={`${kindStyle(k).label}: seconds after`} />
                </div>
              );
            })}
          </div>
        </div>
        <div className="set-block wide">
          <p className="set-label">Clip these plays in every game</p>
          <p className="field-hint">Leave empty to use each game's own choice.</p>
          <div className="check-grid">
            {COMMON_KINDS.map((k) => (
              <label key={k} className="check">
                <input
                  type="checkbox"
                  checked={s.autoClip.eventsOverride.includes(k)}
                  onChange={(e) => setClip("eventsOverride", e.target.checked ? [...s.autoClip.eventsOverride, k] : s.autoClip.eventsOverride.filter((x) => x !== k))}
                />
                {kindStyle(k).plural}
              </label>
            ))}
          </div>
        </div>
      </Section>

      <Section title="Detection" icon="target" desc="Finding plays and matches in recordings" show={tab === "detection"}>
        <Row on={s.scanAfterMatch} label="Scan recordings when a match ends">
          <Toggle checked={s.scanAfterMatch} onChange={(v) => set("scanAfterMatch", v)} label="Scan after match" />
        </Row>
        <Row on={s.splitMatches ?? true} label="Split sessions into matches" hint="Uses Match start / end markers.">
          <Toggle checked={s.splitMatches ?? true} onChange={(v) => set("splitMatches", v)} label="Split sessions into matches" />
        </Row>
        <Row label="Analysis resolution" hint="Lower is faster; 720p suits most HUDs." htmlFor="set-ah">
          <select id="set-ah" className="input set-select" value={s.analysisHeight} onChange={(e) => set("analysisHeight", Number(e.target.value))}>
            {[540, 720, 1080].map((h) => (
              <option key={h} value={h}>
                {h}p
              </option>
            ))}
          </select>
        </Row>
      </Section>

      <Section title="Window" icon="monitor" desc="What closing the window does" show={tab === "general"}>
        <Row on={s.closeToTray ?? true} label="Keep running when the window is closed" hint="Recap stays in the tray and keeps recording games. Off: closing the window quits Recap.">
          <Toggle checked={s.closeToTray ?? true} onChange={(v) => set("closeToTray", v)} label="Keep running in the tray" />
        </Row>
      </Section>

      <p className="set-foot muted small">Everything stays on this computer.</p>

      <div className={`save-bar ${dirty ? "show" : ""}`} aria-hidden={!dirty}>
        <span>Unsaved changes</span>
        <button className="btn btn-ghost" onClick={() => base && setS(base)} tabIndex={dirty ? 0 : -1}>
          Discard
        </button>
        <button className="btn btn-primary" onClick={save} disabled={saving} tabIndex={dirty ? 0 : -1}>
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}

function Section({ title, icon, desc, show, children }: { title: string; icon: string; desc: string; show: boolean; children: ReactNode }) {
  if (!show) return null;
  return (
    <section className="set-section">
      <header className="set-head card">
        <span className="icon-tile">
          <Icon name={icon} size={16} />
        </span>
        <div>
          <h2>{title}</h2>
          <p className="field-hint">{desc}</p>
        </div>
      </header>
      <div className="set-grid">{children}</div>
    </section>
  );
}

function Row({ label, hint, htmlFor, on, wide, children }: { label: string; hint?: string; htmlFor?: string; on?: boolean; wide?: boolean; children: ReactNode }) {
  return (
    <div className={`set-row ${on ? "on" : ""} ${wide ? "wide" : ""}`}>
      <div className="set-row-text">
        {htmlFor ? (
          <label className="set-label" htmlFor={htmlFor}>
            {label}
          </label>
        ) : (
          <p className="set-label">{label}</p>
        )}
        {hint && <p className="field-hint">{hint}</p>}
      </div>
      {children}
    </div>
  );
}

const KEY_NAMES: Record<string, string> = { " ": "Space", ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right" };

/** Click, then press a key combination. Produces strings like "Ctrl+Shift+F8". */
function HotkeyInput({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const [listening, setListening] = useState(false);
  return (
    <button
      id={id}
      type="button"
      className={`input hotkey ${listening ? "listening" : ""}`}
      onClick={() => setListening(true)}
      onBlur={() => setListening(false)}
      onKeyDown={(e) => {
        if (!listening) return;
        e.preventDefault();
        if (e.key === "Escape") return setListening(false);
        if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
        const parts = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift"].filter(Boolean) as string[];
        const key = KEY_NAMES[e.key] ?? (e.key.length === 1 ? e.key.toUpperCase() : e.key);
        onChange([...parts, key].join("+"));
        setListening(false);
      }}
      aria-label={`Hotkey ${value}. Click and press a new key to change it.`}
    >
      {listening ? "Press a key…" : <kbd>{value}</kbd>}
    </button>
  );
}
