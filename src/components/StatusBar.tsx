import { Check, Copy, Square } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { barAddress, barBackup, barBots, barDocker, barOperation, barServer, restartNeeded, type Tone } from "../lib/statusbar";
import { cn } from "../lib/utils";
import type { Worldstone } from "../hooks/useWorldstone";

/** How often the age of the last backup is worked out again. */
const CLOCK_MS = 30_000;

const TEXT: Record<Tone, string> = { ok: "text-fg", idle: "text-muted", warn: "text-warn", bad: "text-bad" };
const DOT: Record<Tone, string> = { ok: "bg-ok", idle: "bg-idle", warn: "bg-warn", bad: "bg-bad" };

function useNowSecs(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), CLOCK_MS);
    return () => clearInterval(timer);
  }, []);
  return now;
}

function Segment({ children, className, title }: { children: ReactNode; className?: string; title?: string }) {
  return (
    <div title={title} className={cn("flex h-full shrink-0 items-center gap-1.5 whitespace-nowrap border-l border-line px-3 first:border-l-0 first:pl-0", className)}>
      {children}
    </div>
  );
}

function CopyAddress({ address }: { address: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(address);
      setState("copied");
    } catch {
      setState("failed");
    }
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setState("idle"), 1500);
  };
  return (
    <button
      type="button"
      onClick={copy}
      title={state === "failed" ? "The address could not be copied." : "The address to put in the game client's realm list. Click to copy."}
      aria-label={`Copy the realm list address ${address}`}
      className="flex items-center gap-1.5 rounded px-1 hover:bg-panel-2 hover:text-fg"
    >
      <span>Realmlist</span>
      <span className="font-mono text-fg">{address}</span>
      {state === "copied" ? <Check className="size-3 text-ok" aria-hidden /> : <Copy className="size-3" aria-hidden />}
      <span className="sr-only" aria-live="polite">
        {state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : ""}
      </span>
    </button>
  );
}

/** One line at the bottom of the window, always visible: the servers, the bots, what is running, and what is wrong. */
export function StatusBar({ ws, onOpenLogs }: { ws: Worldstone; onOpenLogs: () => void }) {
  const now = useNowSecs();
  const shell = "flex h-7 shrink-0 items-center border-t border-line bg-panel px-3 text-xs text-muted";

  if (!ws.native) {
    return (
      <footer className={shell} aria-label="Status">
        <Segment>
          <span className={cn("size-2 rounded-full", DOT.idle)} aria-hidden /> Desktop app only
        </Segment>
      </footer>
    );
  }

  const server = barServer(ws.server);
  const bots = barBots(ws.bots);
  const op = barOperation({
    task: ws.task,
    starting: ws.starting,
    settingUp: ws.settingUp,
    setupNote: ws.setupNote,
    choosingClient: ws.choosingClient,
    restartNeeded: restartNeeded(ws.server, ws.bots),
  });
  const docker = barDocker(ws.environment);
  const backup = barBackup(ws.backups, ws.database, now);
  const address = barAddress(ws.server, ws.connect);

  return (
    <footer className={shell} aria-label="Status">
      <Segment title={server.title}>
        <span className={cn("size-2 shrink-0 rounded-full", DOT[server.tone])} aria-hidden />
        <span className={cn("font-medium", TEXT[server.tone === "idle" ? "ok" : server.tone])}>{server.text}</span>
        {server.extra && <span>{server.extra}</span>}
      </Segment>
      {bots && (
        <Segment title={bots.title}>
          <span className={TEXT[bots.tone]}>{bots.text}</span>
        </Segment>
      )}

      <div className="flex h-full min-w-0 flex-1 items-center justify-center gap-2 px-3">
        <button
          type="button"
          onClick={onOpenLogs}
          title={`${op.title} — click to open the logs`}
          className={cn("flex min-w-0 items-center gap-2 rounded px-1.5 py-0.5 hover:bg-panel-2", TEXT[op.tone])}
        >
          <span className="truncate">{op.text}</span>
          {op.busy && (
            <span
              className="h-1 w-24 shrink-0 overflow-hidden rounded-full bg-panel-2"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={op.percent ?? undefined}
              aria-label={op.percent === null ? "Working" : `${Math.round(op.percent)} percent`}
            >
              {op.percent === null ? (
                <span className="indeterminate-bar block h-full rounded-full bg-accent" />
              ) : (
                <span className="block h-full rounded-full bg-accent transition-[width]" style={{ width: `${op.percent}%` }} />
              )}
            </span>
          )}
          {op.busy && op.percent !== null && <span className="shrink-0 text-fg">{Math.round(op.percent)}%</span>}
        </button>
        {op.cancellable && (
          <button
            type="button"
            onClick={() => void ws.cancel()}
            className="flex h-5 shrink-0 items-center gap-1 rounded border border-line px-1.5 text-[11px] text-fg hover:bg-panel-2"
          >
            <Square className="size-2.5" aria-hidden /> Cancel
          </button>
        )}
      </div>

      <Segment title={docker.title}>
        <span className={cn("size-2 shrink-0 rounded-full", DOT[docker.tone])} aria-hidden />
        <span className={docker.tone === "ok" || docker.tone === "idle" ? undefined : TEXT[docker.tone]}>{docker.text}</span>
      </Segment>
      {backup && (
        <Segment title={backup.title} className="hidden min-[1000px]:flex">
          <span className={backup.tone === "warn" || backup.tone === "bad" ? TEXT[backup.tone] : undefined}>{backup.text}</span>
        </Segment>
      )}
      {address && (
        <Segment className="hidden min-[1100px]:flex">
          <CopyAddress address={address} />
        </Segment>
      )}
    </footer>
  );
}
