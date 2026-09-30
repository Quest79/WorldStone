import { Castle, Crown, Flame, Leaf, Mountain, Shield, Skull, Snowflake, Sun, Swords, type LucideIcon } from "lucide-react";
import type { CSSProperties } from "react";
import type { RecipeSummary } from "../api/types";
import { backdropFor, emblemFor, themeVars, type Emblem } from "../lib/serverTheme";
import { ServerArt } from "./ServerArt";

const ICONS: Record<Emblem, LucideIcon> = {
  swords: Swords,
  snowflake: Snowflake,
  flame: Flame,
  skull: Skull,
  shield: Shield,
  crown: Crown,
  mountain: Mountain,
  castle: Castle,
  leaf: Leaf,
  sun: Sun,
};

/** One server in the sidebar, painted in the colours and emblem its recipe gives its expansion. */
export function ServerButton({ recipe, chosen, locked, onSelect }: { recipe: RecipeSummary; chosen: boolean; locked: boolean; onSelect: () => void }) {
  const Emblem = ICONS[emblemFor(recipe.theme)];
  const scene = backdropFor(recipe.theme);
  return (
    <button
      type="button"
      onClick={onSelect}
      disabled={locked}
      aria-pressed={chosen}
      title={locked ? "Wait for the running operation to finish before switching servers." : `${recipe.name} · ${recipe.game}`}
      style={themeVars(recipe.theme) as CSSProperties}
      className="server-btn relative flex min-h-[3.75rem] w-full items-center gap-2.5 overflow-hidden rounded-md px-2 py-1.5 text-left disabled:cursor-not-allowed disabled:opacity-50"
    >
      {scene && <ServerArt scene={scene} />}
      <span className="server-btn-emblem relative flex size-8 shrink-0 items-center justify-center rounded-md">
        <Emblem className="size-[18px]" aria-hidden />
      </span>
      <span className="relative min-w-0">
        <span className="server-btn-name block text-sm font-semibold leading-tight">{recipe.name}</span>
        <span className="server-btn-game block text-xs leading-snug">{recipe.game}</span>
      </span>
    </button>
  );
}
