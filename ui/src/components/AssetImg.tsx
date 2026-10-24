import { useEffect, useState } from "react";
import { fileSrc } from "../lib/api";
import { assetPaths, type AssetKind } from "../lib/assets";

/**
 * A bundled game image (agent portrait, map banner, rank icon). Tries each candidate file in
 * turn; with none left it shows `fallback` (or initials for portraits, nothing otherwise).
 */
export function AssetImg({
  dir,
  kind,
  name,
  className = "",
  fallback,
}: {
  dir: string | null | undefined;
  kind: AssetKind;
  name: string | null | undefined;
  className?: string;
  fallback?: "initials" | "none";
}) {
  const paths = assetPaths(dir, kind, name);
  const key = paths.join("|");
  const [i, setI] = useState(0);
  useEffect(() => setI(0), [key]);
  const src = i < paths.length ? fileSrc(paths[i]) : undefined;
  if (src) return <img className={`asset asset-${kind} ${className}`} src={src} alt="" loading="lazy" draggable={false} onError={() => setI(i + 1)} />;
  if ((fallback ?? (kind === "agents" ? "initials" : "none")) === "initials" && name)
    return (
      <span className={`asset asset-${kind} asset-initials ${className}`} aria-hidden>
        {initials(name)}
      </span>
    );
  return null;
}

function initials(name: string): string {
  const words = name.replace(/[^\p{L}\p{N} ]/gu, " ").split(/\s+/).filter(Boolean);
  if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
  return (words[0] ?? name).slice(0, 2).toUpperCase();
}
