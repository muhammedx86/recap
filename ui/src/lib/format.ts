export function timecode(ms: number, withTenths = false): string {
  const neg = ms < 0;
  const total = Math.max(0, Math.abs(ms));
  const s = Math.floor(total / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const base = h > 0 ? `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}` : `${m}:${String(sec).padStart(2, "0")}`;
  const tenth = withTenths ? `.${Math.floor((total % 1000) / 100)}` : "";
  return `${neg ? "-" : ""}${base}${tenth}`;
}

export function duration(ms: number): string {
  const min = Math.round(ms / 60000);
  if (ms < 60_000) return `${Math.round(ms / 1000)}s`;
  if (min < 60) return `${min} min`;
  return `${Math.floor(min / 60)} h ${min % 60} min`;
}

export function bytes(n: number): string {
  if (n < 1e6) return `${Math.round(n / 1e3)} KB`;
  if (n < 1e9) return `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} MB`;
  return `${(n / 1e9).toFixed(n < 1e10 ? 1 : 0)} GB`;
}

export function clockTime(unixMs: number): string {
  return new Date(unixMs).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** "Today", "Yesterday", "Wednesday", or "Monday 12 October". */
export function dayLabel(unixMs: number, now = Date.now()): string {
  const d = new Date(unixMs);
  const start = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((start(new Date(now)) - start(d)) / 86_400_000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  if (diff < 7) return d.toLocaleDateString(undefined, { weekday: "long" });
  return d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
}

export function dayKey(unixMs: number): string {
  const d = new Date(unixMs);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "just now", "5 minutes ago", "10 hours ago", "3 days ago", then a date. */
export function timeAgo(unixMs: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - unixMs) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} ${m === 1 ? "minute" : "minutes"} ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} ${h === 1 ? "hour" : "hours"} ago`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d} ${d === 1 ? "day" : "days"} ago`;
  return new Date(unixMs).toLocaleDateString(undefined, { month: "short", day: "numeric", year: d > 300 ? "numeric" : undefined });
}
