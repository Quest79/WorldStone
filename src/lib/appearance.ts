import type { CSSProperties } from "react";
import type { FontSettings, FontStyle, TextOutline, TextShadow } from "../api/types";

/**
 * The font roles the app has. Every role is edited by the same controls (font, outline, shadow),
 * so a new role is one entry here, one field in the backend's `FontSettings`, and the CSS that
 * reads its variables (`--font-<role>`, `--stroke-<role>`, `--shadow-<role>`).
 */
export const FONT_ROLES: ReadonlyArray<{ id: keyof FontSettings; label: string; hint: string; samples: string[] }> = [
  {
    id: "ui",
    label: "General UI",
    hint: "Everything in the window: pages, buttons, cards, logs.",
    samples: ["WorldStone: Start servers", "The quick brown fox jumps over the lazy dog 0123456789"],
  },
  {
    id: "serverName",
    label: "Server names",
    hint: "The names on the server buttons in the sidebar.",
    samples: ["MaNGOS Zero", "AzerothCore"],
  },
];

/** What the page falls back to when the chosen font is missing or lacks a character. */
const FALLBACKS = '"Segoe UI", system-ui, sans-serif';
const HEX = /^#[0-9a-f]{6}$/i;

/** `--font-server-name` for `serverName`. */
export function varSuffix(role: keyof FontSettings): string {
  return role.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
}

/** `#rrggbb` and an opacity in percent as `#rrggbbaa`; anything that is not a colour becomes black. */
export function hexAlpha(color: string, percent: number): string {
  const base = HEX.test(color) ? color.toLowerCase() : "#000000";
  const alpha = Math.round((Math.min(100, Math.max(0, percent)) / 100) * 255);
  return `${base}${alpha.toString(16).padStart(2, "0")}`;
}

/** The CSS `font-family` value: the chosen font first, then what a missing character falls back to. */
export function fontFamilyValue(style: FontStyle): string {
  const name = style.family.replace(/["\\;{}<>\r\n]/g, "");
  return name && name.toLowerCase() !== "segoe ui" ? `"${name}", ${FALLBACKS}` : FALLBACKS;
}

/**
 * `-webkit-text-stroke` for an outline. The stroke is drawn behind the letters (`paint-order`), so
 * only its outer half shows; it is doubled here so the width means what it looks like.
 */
export function strokeValue(outline: TextOutline): string {
  if (!outline.enabled || !(outline.width > 0)) return "0 transparent";
  return `${+(outline.width * 2).toFixed(2)}px ${hexAlpha(outline.color, outline.opacity)}`;
}

export function shadowValue(shadow: TextShadow): string {
  if (!shadow.enabled) return "none";
  const n = (v: number) => `${+v.toFixed(2)}px`;
  return `${n(shadow.x)} ${n(shadow.y)} ${n(shadow.blur)} ${hexAlpha(shadow.color, shadow.opacity)}`;
}

/** The style of a piece of text in this role: for previews, and the same values the page-wide variables carry. */
export function textStyle(style: FontStyle): CSSProperties {
  return {
    fontFamily: fontFamilyValue(style),
    WebkitTextStroke: strokeValue(style.outline),
    paintOrder: "stroke fill",
    textShadow: shadowValue(style.shadow),
  };
}

/** The variables set on the page's root, one trio per role. */
export function appearanceVars(fonts: FontSettings): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const { id } of FONT_ROLES) {
    const style = fonts[id];
    const suffix = varSuffix(id);
    vars[`--font-${suffix}`] = fontFamilyValue(style);
    vars[`--stroke-${suffix}`] = strokeValue(style.outline);
    vars[`--shadow-${suffix}`] = shadowValue(style.shadow);
  }
  return vars;
}

/** The Google families the current settings use, each once. */
export function googleFamiliesInUse(fonts: FontSettings): string[] {
  const families = FONT_ROLES.map(({ id }) => fonts[id]).filter((s) => s.source === "google").map((s) => s.family);
  return [...new Set(families)];
}

/** Whether two styles are the same choice, so "reset" can say there is nothing to reset. */
export function sameStyle(a: FontStyle, b: FontStyle): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Installed font names that contain the search text, in the order given. */
export function matchNames(names: string[], search: string): string[] {
  const needle = search.trim().toLowerCase();
  return needle ? names.filter((n) => n.toLowerCase().includes(needle)) : names;
}
