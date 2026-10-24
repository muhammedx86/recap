// Bundled per-game art (agent portraits, map banners, rank icons) under a game's `assets/`
// folder: assets/agents/<slug>.png, assets/maps/<slug>.jpg, assets/ranks/<slug>.png.

import { useEffect, useState } from "react";
import { api } from "./api";

/** "KAY/O" -> "kay-o", "Gold 2" -> "gold-2". */
export function slug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export type AssetKind = "agents" | "maps" | "ranks";

const EXT: Record<AssetKind, string[]> = { agents: ["png"], maps: ["jpg", "png"], ranks: ["png"] };

/** Candidate file paths for one asset, best first. Empty when the game has no assets folder. */
export function assetPaths(dir: string | null | undefined, kind: AssetKind, name: string | null | undefined): string[] {
  if (!dir || !name?.trim()) return [];
  const sep = dir.includes("\\") ? "\\" : "/";
  const base = dir.replace(/[\\/]+$/, "");
  return EXT[kind].map((e) => `${base}${sep}${kind}${sep}${slug(name)}.${e}`);
}

let cache: Record<string, string | null> | null = null;
let pending: Promise<Record<string, string | null>> | null = null;
const subs = new Set<(m: Record<string, string | null>) => void>();

function load(): Promise<Record<string, string | null>> {
  pending ??= api
    .listGames()
    .then((gs) => {
      cache = Object.fromEntries(gs.map((g) => [g.id, g.assetsDir ?? null]));
      subs.forEach((f) => f(cache!));
      return cache;
    })
    .catch(() => (cache = {}))
    .finally(() => {
      pending = null;
    });
  return pending;
}

/** gameId -> absolute assets folder (or null), loaded once and shared. */
export function useAssetDirs(): Record<string, string | null> {
  const [dirs, setDirs] = useState<Record<string, string | null>>(cache ?? {});
  useEffect(() => {
    subs.add(setDirs);
    if (!cache) load();
    return () => {
      subs.delete(setDirs);
    };
  }, []);
  return dirs;
}
