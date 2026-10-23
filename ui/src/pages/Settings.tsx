import { useEffect, useState, type ReactNode } from "react";
import { api, pickPath } from "../lib/api";
import { bytes } from "../lib/format";
import { useLoad } from "../lib/hooks";
import { COMMON_KINDS, kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { EncoderChoice, Settings } from "../lib/types";
import { Field, ProgressBar, Toggle } from "../components/ui";

const ENCODER_LABEL: Record<string, string> = {
  auto: "Automatic (best available)",
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
        <p className="lede">Everything stays on this computer. Nothing is uploaded.</p>
      </header>

      <Section title="Recordings" note="Where matches and clips are saved.">
        <Field label="Folder" htmlFor="set-dir">
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
        </Field>
        {storage && (
          <div className="storage">
            <ProgressBar value={limitBytes ? storage.usedBytes / limitBytes : storage.usedBytes / (storage.usedBytes + storage.freeBytes)} label="Storage used" />
            <p className="small">
              <span className="num">{bytes(storage.usedBytes)}</span> used by {storage.matchCount} recordings and {storage.clipCount} clips
              {limitBytes ? ` of a ${s.storageLimitGb} GB limit` : ""}. <span className="num">{bytes(storage.freeBytes)}</span> free on the drive.
            </p>
          </div>
        )}
        <Field label="Storage limit" htmlFor="set-limit" hint="When recordings pass this size, the oldest ones that are not favourites are deleted. Leave empty for no limit.">
          <div className="input-row input-row-narrow">
            <input
              id="set-limit"
              type="number"
              className="input"
              min={1}
              value={s.storageLimitGb ?? ""}
              placeholder="No limit"
              onChange={(e) => set("storageLimitGb", e.target.value ? Math.max(1, Number(e.target.value)) : null)}
            />
            <span className="unit">GB</span>
          </div>
        </Field>
      </Section>

      <Section title="Video" note="Higher bitrate looks better and takes more space. 12,000 kbps at 1080p60 is about 5 GB per hour.">
        <div className="field-row">
          <Field label="Frame rate" htmlFor="set-fps">
            <select id="set-fps" className="input" value={s.fps} onChange={(e) => set("fps", Number(e.target.value))}>
              {[30, 60, 120].map((f) => (
                <option key={f} value={f}>
                  {f} fps
                </option>
              ))}
            </select>
          </Field>
          <Field label="Resolution" htmlFor="set-res">
            <select id="set-res" className="input" value={s.outputHeight ?? ""} onChange={(e) => set("outputHeight", e.target.value ? Number(e.target.value) : null)}>
              {HEIGHTS.map(([h, l]) => (
                <option key={l} value={h ?? ""}>
                  {l}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Bitrate" htmlFor="set-br">
            <div className="input-row">
              <input id="set-br" type="number" className="input" min={1000} max={100000} step={500} value={s.bitrateKbps} onChange={(e) => set("bitrateKbps", Number(e.target.value))} />
              <span className="unit">kbps</span>
            </div>
          </Field>
        </div>
        <Field label="Encoder" htmlFor="set-enc" hint="Automatic picks your graphics card's encoder and falls back to the CPU.">
          <select id="set-enc" className="input" value={s.encoder} onChange={(e) => set("encoder", e.target.value as EncoderChoice)}>
            {[...new Set(["auto", ...(encoders ?? [])])].map((e) => (
              <option key={e} value={e}>
                {ENCODER_LABEL[e] ?? e}
              </option>
            ))}
          </select>
        </Field>
      </Section>

      <Section title="Audio" note="Your microphone is saved on its own track, so you can turn it down later.">
        <Row label="Record game and desktop sound">
          <Toggle checked={s.recordDesktopAudio} onChange={(v) => set("recordDesktopAudio", v)} label="Record desktop audio" />
        </Row>
        <Row label="Record microphone">
          <Toggle checked={s.recordMicrophone} onChange={(v) => set("recordMicrophone", v)} label="Record microphone" />
        </Row>
        {s.recordMicrophone && (
          <Field label="Microphone" htmlFor="set-mic">
            <select id="set-mic" className="input" value={s.microphoneDevice ?? ""} onChange={(e) => set("microphoneDevice", e.target.value || null)}>
              <option value="">System default</option>
              {(mics ?? [])
                .filter((m) => m.id !== "default")
                .map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
            </select>
          </Field>
        )}
      </Section>

      <Section title="Hotkeys" note="These work while you are in a game.">
        <div className="field-row">
          <Field label="Bookmark this moment" htmlFor="set-hk-b">
            <HotkeyInput id="set-hk-b" value={s.bookmarkHotkey} onChange={(v) => set("bookmarkHotkey", v)} />
          </Field>
          <Field label="Start or stop a manual recording" htmlFor="set-hk-r">
            <HotkeyInput id="set-hk-r" value={s.manualRecordHotkey} onChange={(v) => set("manualRecordHotkey", v)} />
          </Field>
        </div>
        <Row label="Show a small notice in game when a bookmark is saved">
          <Toggle checked={s.notifyOnBookmark} onChange={(v) => set("notifyOnBookmark", v)} label="Bookmark notice" />
        </Row>
      </Section>

      <Section title="Auto-clips" note="When a match ends, Recap cuts a short clip around each play.">
        <Row label="Make clips automatically">
          <Toggle checked={s.autoClip.enabled} onChange={(v) => setClip("enabled", v)} label="Auto-clips" />
        </Row>
        <div className="field-row">
          <Field label="Seconds before" htmlFor="set-cb">
            <input id="set-cb" type="number" className="input" min={0} max={60} step={0.5} value={s.autoClip.secondsBefore} onChange={(e) => setClip("secondsBefore", Number(e.target.value))} />
          </Field>
          <Field label="Seconds after" htmlFor="set-ca">
            <input id="set-ca" type="number" className="input" min={0} max={60} step={0.5} value={s.autoClip.secondsAfter} onChange={(e) => setClip("secondsAfter", Number(e.target.value))} />
          </Field>
          <Field label="Join plays closer than" htmlFor="set-cg" hint="Seconds. Back-to-back kills become one clip.">
            <input id="set-cg" type="number" className="input" min={0} max={30} step={0.5} value={s.autoClip.mergeGapSeconds} onChange={(e) => setClip("mergeGapSeconds", Number(e.target.value))} />
          </Field>
        </div>
        <div className="field">
          <p className="field-label">Clip these plays in every game</p>
          <p className="field-hint">Leave all unticked to use each game's own choice from the Games page.</p>
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
        <Row label="Frame-accurate cuts" hint="Re-encodes each clip. Slower, but clips start exactly where you set them instead of at the nearest keyframe.">
          <Toggle checked={s.autoClip.preciseCuts} onChange={(v) => setClip("preciseCuts", v)} label="Frame-accurate cuts" />
        </Row>
      </Section>

      <Section title="Finding plays" note="Detectors look for each game's on-screen signals after the match ends.">
        <Row label="Scan each recording when the match ends">
          <Toggle checked={s.scanAfterMatch} onChange={(v) => set("scanAfterMatch", v)} label="Scan after match" />
        </Row>
        <Field label="Analysis resolution" htmlFor="set-ah" hint="Frames are scaled to this height before matching. Lower is faster; 720p suits most HUDs.">
          <select id="set-ah" className="input" value={s.analysisHeight} onChange={(e) => set("analysisHeight", Number(e.target.value))}>
            {[540, 720, 1080].map((h) => (
              <option key={h} value={h}>
                {h}p
              </option>
            ))}
          </select>
        </Field>
      </Section>

      <div className={`save-bar ${dirty ? "show" : ""}`} aria-hidden={!dirty}>
        <span>You have unsaved changes.</span>
        <button className="btn btn-ghost" onClick={() => base && setS(base)} tabIndex={dirty ? 0 : -1}>
          Discard
        </button>
        <button className="btn btn-primary" onClick={save} disabled={saving} tabIndex={dirty ? 0 : -1}>
          {saving ? "Saving…" : "Save settings"}
        </button>
      </div>
    </div>
  );
}

function Section({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <section className="set-section">
      <div className="set-aside">
        <h2>{title}</h2>
        {note && <p className="muted small">{note}</p>}
      </div>
      <div className="set-body">{children}</div>
    </section>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="set-row">
      <div>
        <p>{label}</p>
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
