import { useEffect, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api } from "../lib/api";
import { bytes, dayLabel, timecode } from "../lib/format";
import { useLibraryChanged, useLoad } from "../lib/hooks";
import { kindStyle } from "../lib/kinds";
import { useToast } from "../lib/toast";
import type { ClipKind, ClipRecord } from "../lib/types";
import { Player } from "../components/Player";
import { Confirm, Empty, Icon, Modal, Thumb } from "../components/ui";

const FILTERS: Array<{ id: "all" | ClipKind; label: string }> = [
  { id: "all", label: "All" },
  { id: "auto", label: "Auto" },
  { id: "manual", label: "Made by you" },
  { id: "montage", label: "Montages" },
];

export function Clips() {
  const { data: clips, error, loading, reload, setData } = useLoad(() => api.listClips(), []);
  const [params, setParams] = useSearchParams();
  const [filter, setFilter] = useState<"all" | ClipKind>("all");
  const [selected, setSelected] = useState<string[]>([]);
  const [playing, setPlaying] = useState<ClipRecord | null>(null);
  const [renaming, setRenaming] = useState<ClipRecord | null>(null);
  const [deleting, setDeleting] = useState<ClipRecord | null>(null);
  const [montage, setMontage] = useState(false);
  const toast = useToast();
  useLibraryChanged(reload);

  // Deep link from the match page: /clips?open=<id>
  useEffect(() => {
    const open = params.get("open");
    if (open && clips) {
      const c = clips.find((x) => x.id === open);
      if (c) setPlaying(c);
      setParams({}, { replace: true });
    }
  }, [params, clips, setParams]);

  const shown = useMemo(() => (clips ?? []).filter((c) => filter === "all" || c.kind === filter), [clips, filter]);
  const byId = useMemo(() => new Map((clips ?? []).map((c) => [c.id, c])), [clips]);

  const toggle = (id: string) => setSelected((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]));
  const replace = (c: ClipRecord) => setData((l) => (l ?? []).map((x) => (x.id === c.id ? c : x)));

  return (
    <div className="page">
      <header className="page-head">
        <h1>Clips</h1>
        <p className="lede">Your highlights and montages. Select clips to join them into a montage.</p>
      </header>

      <div className="filters" role="toolbar" aria-label="Filter clips">
        {FILTERS.map((f) => (
          <button key={f.id} className={`chip ${filter === f.id ? "on" : ""}`} onClick={() => setFilter(f.id)} aria-pressed={filter === f.id}>
            {f.label}
          </button>
        ))}
      </div>

      {error && <p className="error-line">Could not load clips: {error}</p>}
      {!loading && !error && shown.length === 0 && (
        <Empty title={clips?.length ? "No clips of this kind" : "No clips yet"}>
          {clips?.length ? "Try another filter." : "Clips are cut around your plays when a match ends, or press I and O on any recording."}
        </Empty>
      )}

      <ul className={`clip-grid ${selected.length ? "selecting" : ""}`}>
        {shown.map((c) => {
          const sel = selected.includes(c.id);
          const kinds = c.eventKinds.filter((k) => k !== "match_start" && k !== "match_end").slice(0, 3);
          return (
            <li key={c.id} className={`clip ${sel ? "selected" : ""}`}>
              <button className="clip-play" onClick={() => setPlaying(c)} aria-label={`Play ${c.title}`}>
                <Thumb path={c.thumbnail} alt="">
                  <span className="thumb-duration num">{timecode(c.durationMs)}</span>
                  {c.kind === "montage" && <span className="thumb-tag">Montage</span>}
                </Thumb>
              </button>
              <label className="clip-select" title={sel ? "Deselect" : "Select for a montage"}>
                <input type="checkbox" checked={sel} onChange={() => toggle(c.id)} aria-label={`Select ${c.title}`} />
                {sel && <span className="clip-order num">{selected.indexOf(c.id) + 1}</span>}
              </label>
              <div className="clip-body">
                <div className="clip-text">
                  <h3 className="clip-title">{c.title}</h3>
                  <p className="clip-meta">
                    <span>{dayLabel(c.createdUnixMs)}</span>
                    <span className="num">{bytes(c.sizeBytes)}</span>
                    {kinds.length > 0 && <span>{kinds.map((k) => kindStyle(k).label).join(", ")}</span>}
                  </p>
                </div>
                <div className="row-actions">
                  {c.matchId && (
                    <Link className="icon-btn" to={`/match/${c.matchId}`} aria-label="Open the full match" title="Open the full match">
                      <Icon name="library" size={16} />
                    </Link>
                  )}
                  <button className="icon-btn" aria-label="Show in folder" title="Show in folder" onClick={() => api.openPath(c.path).catch((e) => toast.error(e))}>
                    <Icon name="folder" size={16} />
                  </button>
                  <button className="icon-btn" aria-label="Rename" title="Rename" onClick={() => setRenaming(c)}>
                    <Icon name="edit" size={16} />
                  </button>
                  <button className="icon-btn" aria-label="Delete" title="Delete" onClick={() => setDeleting(c)}>
                    <Icon name="trash" size={16} />
                  </button>
                </div>
              </div>
            </li>
          );
        })}
      </ul>

      {selected.length > 0 && (
        <div className="selection-bar" role="region" aria-label="Selection">
          <span className="num">
            {selected.length} {selected.length === 1 ? "clip" : "clips"} · {timecode(selected.reduce((a, id) => a + (byId.get(id)?.durationMs ?? 0), 0))}
          </span>
          <button className="btn btn-ghost" onClick={() => setSelected([])}>
            Clear
          </button>
          <button className="btn btn-primary" disabled={selected.length < 2} onClick={() => setMontage(true)} title={selected.length < 2 ? "Select at least two clips" : undefined}>
            Make montage
          </button>
        </div>
      )}

      {playing && (
        <Modal title={playing.title} wide onClose={() => setPlaying(null)}>
          <Player path={playing.path} poster={playing.thumbnail} durationMs={playing.durationMs} onTime={() => {}} autoPlay className="player-modal" />
        </Modal>
      )}
      {renaming && <RenameClip clip={renaming} onClose={() => setRenaming(null)} onSaved={replace} />}
      {deleting && (
        <Confirm
          title="Delete clip"
          danger
          confirmLabel="Delete clip"
          checkboxLabel="Also delete the video file from disk"
          body={
            <p>
              <strong>{deleting.title}</strong> will be removed from Recap.
            </p>
          }
          onClose={() => setDeleting(null)}
          onConfirm={async (deleteFile) => {
            try {
              await api.deleteClip(deleting.id, deleteFile);
              setData((l) => (l ?? []).filter((x) => x.id !== deleting.id));
              setSelected((s) => s.filter((x) => x !== deleting.id));
            } catch (e) {
              toast.error(e, "Could not delete");
            }
          }}
        />
      )}
      {montage && (
        <MontageDialog
          clips={selected.map((id) => byId.get(id)).filter((c): c is ClipRecord => !!c)}
          onClose={() => setMontage(false)}
          onCreated={(c) => {
            setData((l) => [c, ...(l ?? [])]);
            setSelected([]);
            setMontage(false);
            toast.info(`Montage “${c.title}” saved`);
          }}
        />
      )}
    </div>
  );
}

function RenameClip({ clip, onClose, onSaved }: { clip: ClipRecord; onClose: () => void; onSaved: (c: ClipRecord) => void }) {
  const [title, setTitle] = useState(clip.title);
  const toast = useToast();
  const save = async () => {
    if (!title.trim()) return;
    try {
      onSaved(await api.updateClip(clip.id, title.trim()));
      onClose();
    } catch (e) {
      toast.error(e, "Could not rename");
    }
  };
  return (
    <Modal
      title="Rename clip"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save} disabled={!title.trim()}>
            Save name
          </button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <input className="input input-large" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} aria-label="Clip title" />
      </form>
    </Modal>
  );
}

function MontageDialog({ clips: initial, onClose, onCreated }: { clips: ClipRecord[]; onClose: () => void; onCreated: (c: ClipRecord) => void }) {
  const [order, setOrder] = useState(initial);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const toast = useToast();
  const move = (i: number, d: number) =>
    setOrder((o) => {
      const n = [...o];
      const j = i + d;
      if (j < 0 || j >= n.length) return o;
      [n[i], n[j]] = [n[j], n[i]];
      return n;
    });
  const create = async () => {
    setBusy(true);
    try {
      onCreated(await api.createMontage(order.map((c) => c.id), title.trim() || "Montage"));
    } catch (e) {
      toast.error(e, "Could not make montage");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      title="Make a montage"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={create} disabled={busy || order.length < 2}>
            {busy ? "Stitching…" : "Make montage"}
          </button>
        </>
      }
    >
      <input className="input input-large" placeholder="Montage title" value={title} onChange={(e) => setTitle(e.target.value)} aria-label="Montage title" autoFocus />
      <p className="field-hint">Clips play in this order. Total length {timecode(order.reduce((a, c) => a + c.durationMs, 0))}.</p>
      <ol className="montage-list">
        {order.map((c, i) => (
          <li key={c.id}>
            <span className="num muted">{i + 1}</span>
            <Thumb path={c.thumbnail} alt="" className="montage-thumb" />
            <span className="montage-title">{c.title}</span>
            <span className="num muted small">{timecode(c.durationMs)}</span>
            <button className="icon-btn" aria-label="Move up" disabled={i === 0} onClick={() => move(i, -1)}>
              <Icon name="up" size={16} />
            </button>
            <button className="icon-btn" aria-label="Move down" disabled={i === order.length - 1} onClick={() => move(i, 1)}>
              <Icon name="down" size={16} />
            </button>
          </li>
        ))}
      </ol>
    </Modal>
  );
}
