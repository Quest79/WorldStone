import { Container, Gem, Gamepad2, House, ScrollText, Settings, Wrench, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { ServerButton } from "./components/ServerButton";
import { SettingsDialog } from "./components/settings/SettingsDialog";
import { StatusBar } from "./components/StatusBar";
import { useAppearance } from "./hooks/useAppearance";
import { useWorldstone } from "./hooks/useWorldstone";
import { cn } from "./lib/utils";
import { HomePage } from "./pages/HomePage";
import { LogsPage } from "./pages/LogsPage";
import { MaintenancePage } from "./pages/MaintenancePage";
import { PlayPage } from "./pages/PlayPage";
import { SetupPage } from "./pages/SetupPage";

type Page = "home" | "setup" | "play" | "maintenance" | "logs";

const NAV: Array<{ id: Page; label: string; icon: LucideIcon }> = [
  { id: "home", label: "Home", icon: House },
  { id: "setup", label: "Setup", icon: Wrench },
  { id: "play", label: "Play", icon: Gamepad2 },
  { id: "maintenance", label: "Maintenance", icon: Container },
  { id: "logs", label: "Logs", icon: ScrollText },
];

export default function App() {
  const ws = useWorldstone();
  const appearance = useAppearance();
  const [page, setPage] = useState<Page>("home");
  const [settingsOpen, setSettingsOpen] = useState(false);

  return (
    <div className="flex h-full flex-col">
      <div className="flex min-h-0 flex-1">
        <nav className="flex w-52 shrink-0 flex-col border-r border-line bg-panel p-2" aria-label="Main">
          <div className="mb-2 flex items-center gap-2 border-b border-line px-3 pb-3 pt-2">
            <Gem className="size-5 text-accent" aria-hidden />
            <span className="font-semibold tracking-tight">WorldStone</span>
            <span className="text-xs text-muted">{ws.info ? `v${ws.info.version}` : ""}</span>
          </div>
          {ws.recipes.length > 0 && (
            <div className="mb-3 border-b border-line px-1 pb-3" role="group" aria-label="Server">
              <p className="px-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-muted">Server</p>
              <ul className="space-y-0.5">
                {ws.recipes.map((r) => {
                  const chosen = r.id === ws.recipe?.id;
                  const locked = ws.running && !chosen;
                  return (
                    <li key={r.id}>
                      <ServerButton recipe={r} chosen={chosen} locked={locked} onSelect={() => ws.selectRecipe(r.id)} />
                    </li>
                  );
                })}
              </ul>
            </div>
          )}
          <ul className="space-y-1">
            {NAV.map(({ id, label, icon: Icon }) => (
              <li key={id}>
                <button
                  type="button"
                  onClick={() => setPage(id)}
                  aria-current={page === id ? "page" : undefined}
                  className={cn(
                    "flex h-9 w-full items-center gap-2.5 rounded-md px-3 text-sm transition-colors",
                    page === id ? "bg-accent/15 text-accent" : "text-muted hover:bg-panel-2 hover:text-fg",
                  )}
                >
                  <Icon className="size-4" aria-hidden /> {label}
                </button>
              </li>
            ))}
          </ul>
          <div className="mt-auto flex justify-end border-t border-line px-1 pt-2">
            <button
              type="button"
              onClick={() => setSettingsOpen(true)}
              aria-label="Settings"
              title="Settings"
              className="rounded-md p-2 text-muted transition-colors hover:bg-panel-2 hover:text-fg"
            >
              <Settings className="size-4" aria-hidden />
            </button>
          </div>
        </nav>

        <main className="min-w-0 flex-1 overflow-auto p-5">
          {!ws.native && (
            <div className="mb-4 rounded-lg border border-warn/40 bg-warn/10 p-3 text-sm text-warn">
              You are viewing WorldStone in a browser. Operations need the desktop app: run <code>npm run desktop</code>.
            </div>
          )}
          {page === "home" && <HomePage ws={ws} />}
          {page === "setup" && <SetupPage ws={ws} />}
          {page === "play" && <PlayPage ws={ws} />}
          {page === "maintenance" && <MaintenancePage ws={ws} />}
          {page === "logs" && <LogsPage ws={ws} />}
        </main>
      </div>
      <StatusBar ws={ws} onOpenLogs={() => setPage("logs")} />
      {settingsOpen && <SettingsDialog ap={appearance} onClose={() => setSettingsOpen(false)} />}
    </div>
  );
}
