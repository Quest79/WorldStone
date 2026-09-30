import { useLayoutEffect, useRef, useState } from "react";
import type { LogEvent, LogLevel } from "../api/types";
import { formatTime } from "../lib/logs";
import { cn } from "../lib/utils";

// stderr is not "warning": git, Docker and compilers write ordinary progress there, so it is
// only slightly dimmed. Real failures arrive as `error` from the launcher itself.
const LEVEL_CLASS: Record<LogLevel, string> = {
  info: "text-accent",
  stdout: "text-fg",
  stderr: "text-fg/75",
  error: "text-bad",
};

export function LogPanel({ logs, className }: { logs: LogEvent[]; className?: string }) {
  const [snap, setSnap] = useState(true);
  const viewport = useRef<HTMLDivElement>(null);

  // Snap to the newest line only while enabled, so scrolling up to read is never yanked away.
  useLayoutEffect(() => {
    const el = viewport.current;
    if (snap && el) el.scrollTop = el.scrollHeight;
  }, [logs, snap]);

  return (
    <div className={cn("flex min-h-0 flex-col overflow-hidden rounded-lg border border-line bg-panel", className)}>
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted">Activity log</h2>
        <label className="flex items-center gap-2 text-xs text-muted">
          <input type="checkbox" checked={snap} onChange={(e) => setSnap(e.target.checked)} className="accent-accent" />
          Snap to bottom
        </label>
      </div>
      <div ref={viewport} className="min-h-0 flex-1 overflow-auto px-3 py-2 font-mono text-xs leading-5">
        {logs.length === 0 ? (
          <p className="text-muted">Output from operations appears here.</p>
        ) : (
          logs.map((entry, i) => (
            <div key={i} className="flex gap-3 whitespace-pre-wrap break-words">
              <span className="shrink-0 text-idle">{formatTime(entry.tsMs)}</span>
              <span className={LEVEL_CLASS[entry.level]}>{entry.text}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
