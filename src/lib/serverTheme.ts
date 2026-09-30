import type { RecipeTheme } from "../api/types";

/** The emblems the UI can draw. A recipe names one; anything else falls back to a shield. */
export const EMBLEMS = ["swords", "snowflake", "flame", "skull", "shield", "crown", "mountain", "castle", "leaf", "sun"] as const;
export type Emblem = (typeof EMBLEMS)[number];

/** The scenes the UI can draw behind a server's button. A recipe names one; anything else draws none. */
export const BACKDROPS = ["azeroth", "northrend"] as const;
export type Backdrop = (typeof BACKDROPS)[number];

export function backdropFor(theme: RecipeTheme | null): Backdrop | null {
  return (BACKDROPS as readonly string[]).includes(theme?.backdrop ?? "") ? (theme!.backdrop as Backdrop) : null;
}

const HEX = /^#[0-9a-f]{6}$/i;

export function emblemFor(theme: RecipeTheme | null): Emblem {
  return (EMBLEMS as readonly string[]).includes(theme?.emblem ?? "") ? (theme!.emblem as Emblem) : "shield";
}

/**
 * The colours a server's button paints with, as CSS variables. A recipe without a theme, or with a
 * colour that is not `#rrggbb`, sets nothing, so the button keeps the app's own accent.
 */
export function themeVars(theme: RecipeTheme | null): Record<string, string> {
  if (!theme) return {};
  const vars: Record<string, string> = {};
  if (HEX.test(theme.accent)) vars["--sv-accent"] = theme.accent;
  if (HEX.test(theme.deep)) vars["--sv-deep"] = theme.deep;
  return vars;
}
