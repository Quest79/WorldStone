import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, isNative } from "../api/client";
import type { Appearance, FontSettings, FontStyle, GoogleCatalogue } from "../api/types";
import { appearanceVars, googleFamiliesInUse } from "../lib/appearance";

/** A list that is fetched when it is first needed. */
export interface Loadable<T> {
  status: "idle" | "loading" | "ready" | "error";
  data: T;
  error: string | null;
}

export type SaveState = "idle" | "saving" | "saved" | "error";

/** Sliders send many values a second; the file is written once they pause. */
const SAVE_DELAY_MS = 350;

/**
 * The person's appearance choices: read at start, applied to the whole page as CSS variables, saved
 * as they change. It also holds the font lists the settings window picks from and the Google
 * families that have been downloaded and registered with the page.
 */
export function useAppearance() {
  const native = isNative();
  const [current, setCurrent] = useState<Appearance | null>(null);
  const [defaults, setDefaults] = useState<Appearance | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<Loadable<string[]>>({ status: "idle", data: [], error: null });
  const [catalogue, setCatalogue] = useState<Loadable<GoogleCatalogue | null>>({ status: "idle", data: null, error: null });
  const [downloaded, setDownloaded] = useState<string[]>([]);
  const [downloading, setDownloading] = useState<string | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  /** Google families in use whose files could not be loaded, with the reason. */
  const [missing, setMissing] = useState<Record<string, string>>({});
  const registered = useRef(new Set<string>());
  const latest = useRef<Appearance | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const writing = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    if (!native) return;
    api
      .getAppearance()
      .then((state) => {
        latest.current = state.current;
        setCurrent(state.current);
        setDefaults(state.defaults);
        setProblem(state.problem);
      })
      .catch((e) => setProblem(errorText(e)));
    api.googleDownloaded().then(setDownloaded).catch(() => setDownloaded([]));
  }, [native]);

  /** Loads a downloaded family's files into the page, once. */
  const registerGoogle = useCallback(async (family: string): Promise<boolean> => {
    if (registered.current.has(family)) return true;
    try {
      const faces = await api.googleFontFaces(family);
      for (const face of faces) {
        const bytes = Uint8Array.from(atob(face.data), (c) => c.charCodeAt(0));
        const font = new FontFace(family, bytes.buffer, { weight: String(face.weight) });
        await font.load();
        document.fonts.add(font);
      }
      registered.current.add(family);
      setMissing(({ [family]: _gone, ...rest }) => rest);
      return true;
    } catch (e) {
      setMissing((m) => ({ ...m, [family]: errorText(e) }));
      return false;
    }
  }, []);

  // What is chosen becomes the page's look, and the Google families it uses are loaded.
  useEffect(() => {
    if (!current) return;
    const root = document.documentElement.style;
    for (const [name, value] of Object.entries(appearanceVars(current.fonts))) root.setProperty(name, value);
    for (const family of googleFamiliesInUse(current.fonts)) void registerGoogle(family);
  }, [current, registerGoogle]);

  const write = useCallback(() => {
    writing.current = writing.current.then(async () => {
      const next = latest.current;
      if (!next) return;
      setSaveState("saving");
      try {
        await api.saveAppearance(next);
        setSaveState("saved");
        setSaveError(null);
      } catch (e) {
        setSaveState("error");
        setSaveError(errorText(e));
      }
    });
  }, []);

  /** Changes one font role; the page follows at once and the file is written shortly after. */
  const update = useCallback(
    (role: keyof FontSettings, change: (style: FontStyle) => FontStyle) => {
      setCurrent((prev) => {
        if (!prev) return prev;
        const next: Appearance = { ...prev, fonts: { ...prev.fonts, [role]: change(prev.fonts[role]) } };
        latest.current = next;
        return next;
      });
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(write, SAVE_DELAY_MS);
    },
    [write],
  );

  const reset = useCallback(
    (role: keyof FontSettings) => {
      if (defaults) update(role, () => defaults.fonts[role]);
    },
    [defaults, update],
  );

  const loadInstalled = useCallback(async (refresh = false) => {
    setInstalled((s) => ({ ...s, status: "loading", error: null }));
    try {
      setInstalled({ status: "ready", data: await api.installedFonts(refresh), error: null });
    } catch (e) {
      setInstalled({ status: "error", data: [], error: errorText(e) });
    }
  }, []);

  // Reading the installed fonts takes about two seconds, so it starts with the app and is ready when the window opens.
  useEffect(() => {
    if (native) void loadInstalled();
  }, [native, loadInstalled]);

  const loadCatalogue = useCallback(async (refresh = false) => {
    setCatalogue((s) => ({ ...s, status: "loading", error: null }));
    try {
      setCatalogue({ status: "ready", data: await api.googleCatalogue(refresh), error: null });
    } catch (e) {
      setCatalogue((s) => ({ status: "error", data: s.data, error: errorText(e) }));
    }
  }, []);

  /** Downloads a Google family and loads it; returns its exact name, or null when it failed (the reason is in `downloadError`). */
  const download = useCallback(
    async (family: string): Promise<string | null> => {
      setDownloading(family);
      setDownloadError(null);
      try {
        const exact = await api.downloadGoogleFont(family);
        registered.current.delete(exact);
        await registerGoogle(exact);
        setDownloaded(await api.googleDownloaded());
        return exact;
      } catch (e) {
        setDownloadError(errorText(e));
        return null;
      } finally {
        setDownloading(null);
      }
    },
    [registerGoogle],
  );

  return {
    native,
    current,
    defaults,
    problem,
    saveState,
    saveError,
    installed,
    catalogue,
    downloaded,
    downloading,
    downloadError,
    missing,
    update,
    reset,
    loadInstalled,
    loadCatalogue,
    download,
  };
}

export type AppearanceApi = ReturnType<typeof useAppearance>;
