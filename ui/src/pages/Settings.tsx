import { useEffect, useState, type ReactNode } from "react";
import { api, pickPath } from "../lib/api";
import { bytes } from "../lib/format";
import { useLoad } from "../lib/hooks";
import { COMMON_KINDS, kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { EncoderChoice, Settings } from "../lib/types";
import { ProgressBar, Toggle } from "../components/ui";

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

export function SettingsPage() {
  const toast = useToast();
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
      </header>

      <Section title="Storage">
        <div className="set-block">
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

      <Section title="Video">
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
        <Row label="Bitrate" hint="12,000 kbps at 1080p60 is about 5 GB per hour." htmlFor="set-br">
          <div className="input-row set-num">
            <input id="set-br" type="number" className="input" min={1000} max={100000} step={500} value={s.bitrateKbps} onChange={(e) => set("bitrateKbps", Number(e.target.value))} />
            <span className="unit">kbps</span>
          </div>
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
      </Section>

      <Section title="Audio">
        <Row label="Game and desktop sound">
          <Toggle checked={s.recordDesktopAudio} onChange={(v) => set("recordDesktopAudio", v)} label="Record desktop audio" />
        </Row>
        <Row label="Microphone" hint="Saved on its own track.">
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
      </Section>

      <Section title="Hotkeys">
        <Row label="Bookmark this moment" htmlFor="set-hk-b">
          <HotkeyInput id="set-hk-b" value={s.bookmarkHotkey} onChange={(v) => set("bookmarkHotkey", v)} />
        </Row>
        <Row label="Start or stop recording" htmlFor="set-hk-r">
          <HotkeyInput id="set-hk-r" value={s.manualRecordHotkey} onChange={(v) => set("manualRecordHotkey", v)} />
        </Row>
        <Row label="Notice in game when bookmarking">
          <Toggle checked={s.notifyOnBookmark} onChange={(v) => set("notifyOnBookmark", v)} label="Bookmark notice" />
        </Row>
      </Section>

      <Section title="Auto-clips">
        <Row label="Make clips after each match">
          <Toggle checked={s.autoClip.enabled} onChange={(v) => setClip("enabled", v)} label="Auto-clips" />
        </Row>
        <Row label="Seconds before / after" htmlFor="set-cb">
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
        <Row label="Frame-accurate cuts" hint="Slower; re-encodes each clip.">
          <Toggle checked={s.autoClip.preciseCuts} onChange={(v) => setClip("preciseCuts", v)} label="Frame-accurate cuts" />
        </Row>
        <div className="set-block">
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

      <Section title="Detection">
        <Row label="Scan recordings when a match ends">
          <Toggle checked={s.scanAfterMatch} onChange={(v) => set("scanAfterMatch", v)} label="Scan after match" />
        </Row>
        <Row label="Split sessions into matches" hint="Uses Match start / end markers.">
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

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="set-section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function Row({ label, hint, htmlFor, children }: { label: string; hint?: string; htmlFor?: string; children: ReactNode }) {
  return (
    <div className="set-row">
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
