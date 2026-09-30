import { Check, Copy, RefreshCw } from "lucide-react";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { LogLevel } from "../api/types";
import type { Worldstone } from "../hooks/useWorldstone";
import { useLogViewer } from "../hooks/useLogViewer";
import { Button } from "../components/Button";
import { LogPanel } from "../components/LogPanel";
import { asText, countText, filterLines, groupSources, LINE_CHOICES, lineStamp } from "../lib/logview";
import { cn } from "../lib/utils";

const LEVEL_CLASS: Record<LogLevel, string> = {
  info: "text-accent",
  stdout: "text-fg",
  stderr: "text-fg/75",
  error: "text-bad",
};

const TABS = [
  { id: "activity", label: "Live activity" },
  { id: "browse", label: "Browse logs" },
] as const;
type Tab = (typeof TABS)[number]["id"];

const CONTROL = "h-8 rounded-md border border-line bg-bg px-2 text-sm text-fg";

/** Any log the server keeps: the launcher's own, each container's, and the log files the servers write. */
function Browse({ ws }: { ws: Worldstone }) {
  const viewer = useLogViewer(ws.recipe?.id ?? null, ws.running);
  const [query, setQuery] = useState("");
  const [copied, setCopied] = useState(false);
  const [snap, setSnap] = useState(true);
  const viewport = useRef<HTMLDivElement>(null);

  const groups = useMemo(() => groupSources(viewer.sources ?? []), [viewer.sources]);
  const chosen = viewer.sources?.find((s) => s.id === viewer.source) ?? null;
  const lines = useMemo(() => filterLines(viewer.view?.lines ?? [], query), [viewer.view, query]);
  const total = viewer.view?.lines.length ?? 0;

  // Follow the newest line only while enabled, so scrolling up to read is never yanked away.
  useLayoutEffect(() => {
    const el = viewport.current;
    if (snap && el) el.scrollTop = el.scrollHeight;
  }, [lines, snap]);

  async function copy() {
    try {
      await navigator.clipboard.writeText(asText(lines));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // The clipboard can be refused; the text is still on screen to select.
    }
  }

  const note = viewer.problem ?? viewer.view?.note ?? chosen?.detail ?? null;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-line bg-panel">
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2">
        <select
          value={viewer.source ?? ""}
          onChange={(e) => viewer.setSource(e.target.value)}
          disabled={viewer.sources === null || viewer.sources.length === 0}
          aria-label="Log"
          className={cn(CONTROL, "min-w-52")}
        >
          {groups.map((group) => (
            <optgroup key={group.kind} label={group.heading}>
              {group.sources.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.label}
                  {source.available ? "" : " (nothing to read)"}
                </option>
              ))}
            </optgroup>
          ))}
        </select>
        <select
          value={viewer.lines}
          onChange={(e) => viewer.setLines(Number(e.target.value))}
          aria-label="How many lines"
          className={CONTROL}
        >
          {LINE_CHOICES.map((n) => (
            <option key={n} value={n}>
              Last {n} lines
            </option>
          ))}
        </select>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter lines"
          aria-label="Filter lines"
          spellCheck={false}
          className={cn(CONTROL, "w-44 placeholder:text-muted")}
        />
        <label className="flex items-center gap-1.5 text-xs text-muted">
          <input type="checkbox" checked={viewer.follow} onChange={(e) => viewer.setFollow(e.target.checked)} className="accent-accent" />
          Follow
        </label>
        <label className="flex items-center gap-1.5 text-xs text-muted">
          <input type="checkbox" checked={snap} onChange={(e) => setSnap(e.target.checked)} className="accent-accent" />
          Snap to bottom
        </label>
        <div className="ml-auto flex items-center gap-2">
          <Button onClick={() => void viewer.reload()} className="h-8" disabled={viewer.source === null} title="Read the log again">
            <RefreshCw className={cn("size-4", viewer.loading && "animate-spin")} aria-hidden /> Reload
          </Button>
          <Button onClick={() => void copy()} className="h-8" disabled={lines.length === 0}>
            {copied ? <Check className="size-4" aria-hidden /> : <Copy className="size-4" aria-hidden />} {copied ? "Copied" : "Copy"}
          </Button>
        </div>
      </div>
      <div ref={viewport} className="min-h-0 flex-1 overflow-auto px-3 py-2 font-mono text-xs leading-5">
        {!ws.native ? (
          <p className="text-muted">Available in the desktop app.</p>
        ) : viewer.sources === null ? (
          <p className="text-muted">Checking…</p>
        ) : viewer.sources.length === 0 ? (
          <p className="text-muted">There are no logs yet.</p>
        ) : viewer.view === null && viewer.problem === null ? (
          <p className="text-muted">Reading…</p>
        ) : lines.length === 0 ? (
          <p className="text-muted">{query.trim() === "" ? "This log is empty." : "No line matches the filter."}</p>
        ) : (
          lines.map((line, i) => (
            <div key={i} className="flex gap-3 whitespace-pre-wrap break-words">
              {lineStamp(line) !== "" && <span className="shrink-0 text-idle">{lineStamp(line)}</span>}
              <span className={LEVEL_CLASS[line.level]}>{line.text}</span>
            </div>
          ))
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-line px-3 py-1.5 text-xs text-muted">
        <span>{countText(lines.length, total, query)}</span>
        {viewer.view?.truncated && <span>Older lines exist; raise the number of lines to see more.</span>}
        {note && <span className={cn(viewer.problem !== null && "text-bad")}>{note}</span>}
      </div>
    </div>
  );
}

export function LogsPage({ ws }: { ws: Worldstone }) {
  const [tab, setTab] = useState<Tab>("activity");
  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div role="tablist" aria-label="Logs" className="flex shrink-0 gap-1 border-b border-line">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            className={cn(
              "-mb-px border-b-2 px-3 py-2 text-sm",
              tab === t.id ? "border-accent font-medium text-fg" : "border-transparent text-muted hover:text-fg",
            )}
          >
            {t.label}
          </button>
        ))}
      </div>
      {tab === "activity" ? <LogPanel logs={ws.logs} className="flex-1" /> : <Browse ws={ws} />}
    </div>
  );
}
