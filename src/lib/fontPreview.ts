/**
 * Previews for the Google Fonts list: a family is shown in its own letterforms before it is
 * downloaded. Each preview is Google's own tiny stylesheet asking for just the letters of the
 * family's name (a few hundred bytes of CSS and a few KB of font), fetched only for the rows on
 * screen. It is registered under a different name than the real download (`ws-preview <family>`),
 * so the two can never shadow each other. Offline, or if Google says no, the row stays in the UI font.
 */
const NAME = /^[A-Za-z0-9 -]{1,60}$/;
const FONT_URL = /url\((https:\/\/fonts\.gstatic\.com\/[A-Za-z0-9._~/%?=&-]+)\)/;

/** The name the preview is registered under. */
export const previewName = (family: string): string => `ws-preview ${family}`;

/** Google's stylesheet for just the letters of `family`; null for a name that is not a plain family name. */
export function previewCssUrl(family: string): string | null {
  if (!NAME.test(family)) return null;
  return `https://fonts.googleapis.com/css2?family=${family.replace(/ /g, "+")}&text=${encodeURIComponent(family)}`;
}

/** The first font file a stylesheet points at, only if it is on Google's font host. */
export function firstFontUrl(css: string): string | null {
  return FONT_URL.exec(css)?.[1] ?? null;
}

const started = new Map<string, Promise<boolean>>();

/** Makes the preview for a family available to the page. Each family is fetched at most once per session; resolves false when it could not be. */
export function loadPreview(family: string): Promise<boolean> {
  const known = started.get(family);
  if (known) return known;
  const run = (async () => {
    const cssUrl = previewCssUrl(family);
    if (!cssUrl) return false;
    try {
      const css = await fetch(cssUrl).then((r) => (r.ok ? r.text() : Promise.reject(new Error(String(r.status)))));
      const file = firstFontUrl(css);
      if (!file) return false;
      const face = new FontFace(previewName(family), `url(${file})`);
      await face.load();
      document.fonts.add(face);
      return true;
    } catch {
      return false;
    }
  })();
  started.set(family, run);
  return run;
}
