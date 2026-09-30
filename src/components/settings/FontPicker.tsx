import { Check, Download, LoaderCircle, RefreshCw, Search, TriangleAlert } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { FontSource, FontStyle } from "../../api/types";
import type { AppearanceApi } from "../../hooks/useAppearance";
import { matchNames } from "../../lib/appearance";
import { loadPreview, previewName } from "../../lib/fontPreview";
import { cn } from "../../lib/utils";
import { Button } from "../Button";

/** Google's list has well over a thousand families; the most popular are shown and search reaches the rest. */
const GOOGLE_SHOWN = 100;

const SOURCES: Array<{ id: FontSource; label: string }> = [
  { id: "installed", label: "Installed on this PC" },
  { id: "google", label: "Google Fonts" },
];

function Row({ selected, onClick, disabled, children, fontFamily }: { selected: boolean; onClick: () => void; disabled?: boolean; children: React.ReactNode; fontFamily?: string }) {
  return (
    <li className="font-row">
      <button
        type="button"
        onClick={onClick}
        disabled={disabled}
        aria-pressed={selected}
        style={fontFamily ? { fontFamily } : undefined}
        className={cn(
          "flex h-8 w-full items-center justify-between gap-2 rounded px-2 text-left text-sm transition-colors disabled:cursor-wait disabled:opacity-60",
          selected ? "bg-accent/20 text-fg" : "text-muted hover:bg-panel-2 hover:text-fg",
        )}
      >
        {children}
      </button>
    </li>
  );
}

/** A Google family's name drawn in its own font. The preview is fetched when the row scrolls into view; until then, or offline, it is in the UI font. */
function PreviewName({ family }: { family: string }) {
  const el = useRef<HTMLSpanElement>(null);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const node = el.current;
    if (!node) return;
    const watch = new IntersectionObserver(
      (seen) => {
        if (!seen.some((s) => s.isIntersecting)) return;
        watch.disconnect();
        void loadPreview(family).then(setReady);
      },
      { rootMargin: "120px" },
    );
    watch.observe(node);
    return () => watch.disconnect();
  }, [family]);
  return (
    <span ref={el} style={ready ? { fontFamily: `"${previewName(family)}", "Segoe UI", sans-serif` } : undefined}>
      {family}
    </span>
  );
}

/**
 * Chooses a font for one role from the fonts installed on this Windows install or from Google
 * Fonts. A Google font is downloaded to this PC the first time it is chosen, then works offline.
 */
export function FontPicker({ role, style, ap, onPick }: { role: string; style: FontStyle; ap: AppearanceApi; onPick: (source: FontSource, family: string) => void }) {
  const [source, setSource] = useState<FontSource>(style.source);
  const [search, setSearch] = useState("");
  const [byName, setByName] = useState("");

  // The lists load when they are first looked at.
  const { loadInstalled, loadCatalogue } = ap;
  const installedStatus = ap.installed.status;
  const catalogueStatus = ap.catalogue.status;
  useEffect(() => {
    if (source === "installed" && installedStatus === "idle") void loadInstalled();
    if (source === "google" && catalogueStatus === "idle") void loadCatalogue();
  }, [source, installedStatus, catalogueStatus, loadInstalled, loadCatalogue]);

  const installedMatches = useMemo(() => matchNames(ap.installed.data, search), [ap.installed.data, search]);
  const googleMatches = useMemo(() => {
    const all = ap.catalogue.data?.fonts ?? [];
    const needle = search.trim().toLowerCase();
    return needle ? all.filter((f) => f.family.toLowerCase().includes(needle)) : all;
  }, [ap.catalogue.data, search]);

  const chosen = style.family;
  const installedLower = useMemo(() => new Set(ap.installed.data.map((n) => n.toLowerCase())), [ap.installed.data]);
  const gone = style.source === "installed" && installedStatus === "ready" && chosen !== "" && !installedLower.has(chosen.toLowerCase());
  const lostFiles = style.source === "google" ? ap.missing[chosen] : undefined;
  const busy = ap.downloading !== null;

  const chooseGoogle = async (family: string) => {
    const exact = ap.downloaded.includes(family) ? family : await ap.download(family);
    if (exact) onPick("google", exact);
  };

  const list = source === "installed" ? ap.installed : ap.catalogue;
  const shownGoogle = googleMatches.slice(0, GOOGLE_SHOWN);

  return (
    <div className="flex min-h-0 flex-col gap-2">
      <div role="group" aria-label={`Where the ${role} font comes from`} className="grid grid-cols-2 gap-1 rounded-md border border-line bg-panel-2/40 p-1">
        {SOURCES.map(({ id, label }) => (
          <button
            key={id}
            type="button"
            aria-pressed={source === id}
            onClick={() => {
              setSource(id);
              setSearch("");
            }}
            className={cn("h-7 rounded text-xs font-medium transition-colors", source === id ? "bg-accent text-accent-fg" : "text-muted hover:text-fg")}
          >
            {label}
          </button>
        ))}
      </div>

      <div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-2.5 size-3.5 text-muted" aria-hidden />
        <input
          type="search"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={source === "installed" ? "Search installed fonts" : "Search Google Fonts"}
          aria-label={source === "installed" ? "Search installed fonts" : "Search Google Fonts"}
          className="h-8 w-full rounded-md border border-line bg-panel-2 pl-8 pr-2 text-sm text-fg placeholder:text-muted"
        />
      </div>

      <div className="h-64 overflow-auto rounded-md border border-line bg-bg/60 p-1">
        {list.status === "loading" && (
          <p className="flex items-center gap-2 p-2 text-sm text-muted">
            <LoaderCircle className="size-4 animate-spin" aria-hidden /> {source === "installed" ? "Reading the fonts installed on this PC…" : "Loading the Google Fonts list…"}
          </p>
        )}
        {list.status === "error" && <p className="p-2 text-sm text-bad">{list.error}</p>}

        {source === "installed" && installedStatus === "ready" && (
          <>
            <ul>
              {installedMatches.map((name) => (
                <Row key={name} selected={style.source === "installed" && chosen.toLowerCase() === name.toLowerCase()} onClick={() => onPick("installed", name)} fontFamily={`"${name}", "Segoe UI", sans-serif`}>
                  <span className="truncate">{name}</span>
                  {style.source === "installed" && chosen.toLowerCase() === name.toLowerCase() && <Check className="size-3.5 shrink-0 text-accent" aria-hidden />}
                </Row>
              ))}
            </ul>
            {installedMatches.length === 0 && <p className="p-2 text-sm text-muted">{ap.installed.data.length === 0 ? "No fonts were found." : "No installed font matches."}</p>}
          </>
        )}

        {source === "google" && catalogueStatus !== "idle" && ap.catalogue.data && (
          <>
            <ul>
              {shownGoogle.map((f) => {
                const have = ap.downloaded.includes(f.family);
                const selected = style.source === "google" && chosen === f.family;
                return (
                  <Row key={f.family} selected={selected} disabled={busy} onClick={() => void chooseGoogle(f.family)}>
                    <span className="min-w-0 truncate">
                      <PreviewName family={f.family} /> <span className="text-[11px] text-muted">{f.category}</span>
                    </span>
                    {ap.downloading === f.family ? (
                      <span className="flex shrink-0 items-center gap-1 text-xs text-muted">
                        <LoaderCircle className="size-3.5 animate-spin" aria-hidden /> Downloading
                      </span>
                    ) : selected ? (
                      <Check className="size-3.5 shrink-0 text-accent" aria-hidden />
                    ) : have ? (
                      <span className="shrink-0 text-[11px] text-ok">Downloaded</span>
                    ) : (
                      <Download className="size-3.5 shrink-0 text-muted" aria-hidden />
                    )}
                  </Row>
                );
              })}
            </ul>
            {googleMatches.length === 0 && <p className="p-2 text-sm text-muted">No Google font matches. Use “Add by name” below if you know its exact name.</p>}
            {googleMatches.length > GOOGLE_SHOWN && (
              <p className="p-2 text-xs text-muted">
                Showing the {GOOGLE_SHOWN} most popular of {googleMatches.length}. Type to narrow the list.
              </p>
            )}
          </>
        )}
      </div>

      {source === "google" && (
        <>
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              const name = byName.trim();
              if (name) void chooseGoogle(name).then(() => setByName(""));
            }}
          >
            <input
              type="text"
              value={byName}
              onChange={(e) => setByName(e.target.value)}
              placeholder="Add by name, e.g. Cinzel"
              aria-label="Add a Google font by name"
              className="h-8 min-w-0 flex-1 rounded-md border border-line bg-panel-2 px-2 text-sm text-fg placeholder:text-muted"
            />
            <Button type="submit" className="h-8" disabled={!byName.trim() || busy} busy={ap.downloading !== null && ap.downloading === byName.trim()}>
              Download
            </Button>
          </form>
          <div className="flex items-center justify-between gap-2 text-xs text-muted">
            <span>
              {ap.catalogue.data
                ? `List fetched ${new Date(ap.catalogue.data.fetchedAt * 1000).toLocaleDateString()}. `
                : ""}
              Names are drawn in their own font (a small preview comes from Google). A font is downloaded once, then works offline.
            </span>
            <Button variant="ghost" className="h-7 px-2 text-xs" busy={catalogueStatus === "loading"} onClick={() => void ap.loadCatalogue(true)} title="Fetch the current list from Google">
              <RefreshCw className="size-3.5" aria-hidden /> Refresh
            </Button>
          </div>
          {ap.catalogue.data?.note && <p className="text-xs text-warn">{ap.catalogue.data.note}</p>}
          {ap.downloadError && (
            <p role="alert" className="text-xs text-bad">
              {ap.downloadError}
            </p>
          )}
        </>
      )}

      {source === "installed" && (
        <div className="flex items-center justify-between gap-2 text-xs text-muted">
          <span>{installedStatus === "ready" ? `${ap.installed.data.length} fonts installed.` : ""}</span>
          <Button variant="ghost" className="h-7 px-2 text-xs" busy={installedStatus === "loading"} onClick={() => void ap.loadInstalled(true)} title="Read the installed fonts again">
            <RefreshCw className="size-3.5" aria-hidden /> Refresh
          </Button>
        </div>
      )}

      {gone && (
        <p role="alert" className="flex items-start gap-1.5 text-xs text-warn">
          <TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden /> “{chosen}” is not installed on this PC, so text uses Segoe UI instead.
        </p>
      )}
      {lostFiles !== undefined && (
        <div role="alert" className="flex items-start justify-between gap-2 text-xs text-warn">
          <span className="flex items-start gap-1.5">
            <TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden /> The files for “{chosen}” could not be loaded, so text uses Segoe UI instead. {lostFiles}
          </span>
          <Button className="h-7 shrink-0 px-2 text-xs" busy={ap.downloading === chosen} disabled={busy} onClick={() => void ap.download(chosen)}>
            Download again
          </Button>
        </div>
      )}
    </div>
  );
}
