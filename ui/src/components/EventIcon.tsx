import { Bomb, Bookmark, CircleX, Crosshair, Crown, Flag, Flame, Gem, Handshake, Heart, Shield, Skull, Star, Trophy, Zap, type LucideIcon } from "lucide-react";
import type { EventIcon as EventIconName } from "../lib/types";

const MAP: Record<EventIconName, LucideIcon> = {
  crosshair: Crosshair,
  skull: Skull,
  handshake: Handshake,
  flame: Flame,
  crown: Crown,
  gem: Gem,
  flag: Flag,
  trophy: Trophy,
  "circle-x": CircleX,
  star: Star,
  bomb: Bomb,
  shield: Shield,
  bookmark: Bookmark,
  zap: Zap,
  heart: Heart,
};

/** One of the 15 event icons, drawn in `color` (defaults to the current text colour). */
export function EventIcon({ icon, size = 14, color, title }: { icon: EventIconName; size?: number; color?: string; title?: string }) {
  const C = MAP[icon] ?? Star;
  return <C size={size} color={color ?? "currentColor"} strokeWidth={2} aria-hidden={title ? undefined : true} aria-label={title} className="ev-icon" />;
}
