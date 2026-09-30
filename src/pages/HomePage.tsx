import {
  Container,
  Database,
  Download,
  FileCog,
  FolderOpen,
  Bot,
  Gamepad2,
  Gem,
  GitBranch,
  Hammer,
  PackageOpen,
  Pause,
  Play,
  RefreshCw,
  RotateCw,
  Server,
  Square,
  Swords,
  Wrench,
} from "lucide-react";
import type { Worldstone } from "../hooks/useWorldstone";
import { planSetup } from "../lib/setup";
import {
  CHECKING,
  gitReady,
  launchBlocked,
  summarizeBots,
  summarizeBuild,
  summarizeClient,
  summarizeConfig,
  summarizeDatabase,
  summarizeEnvironment,
  summarizeExtract,
  summarizeRepos,
  summarizeServer,
} from "../lib/summary";
import { Button } from "../components/Button";
import { ErrorBanner } from "../components/ErrorBanner";
import { LogPanel } from "../components/LogPanel";
import { StatusCard } from "../components/StatusCard";
import { TaskPanel } from "../components/TaskPanel";

export function HomePage({ ws }: { ws: Worldstone }) {
  // Outside the desktop app nothing can be checked, so say that instead of "Checking…".
  const unavailable = { status: "unknown", detail: "Available in the desktop app" } as const;
  const env = ws.native ? summarizeEnvironment(ws.environment) : unavailable;
  const repos = ws.native ? summarizeRepos(ws.repos) : { ...unavailable, action: "download" as const };
  const build = ws.native ? summarizeBuild(ws.build) : { ...unavailable, action: "build" as const, blocked: "Available in the desktop app" };
  const client = ws.native ? summarizeClient(ws.client) : unavailable;
  const extract = ws.native
    ? summarizeExtract(ws.extract, ws.client)
    : { ...unavailable, action: "extract" as const, blocked: "Available in the desktop app" };
  const db = ws.native
    ? summarizeDatabase(ws.database)
    : { ...unavailable, action: "setup" as const, blocked: "Available in the desktop app" };
  const gates = { build: ws.build, extract: ws.extract, database: ws.database, config: ws.config };
  const config = ws.native
    ? summarizeConfig(ws.config, ws.build)
    : { ...unavailable, action: "generate" as const, blocked: "Available in the desktop app" };
  const server = ws.native
    ? summarizeServer(ws.server, gates)
    : {
        ...unavailable,
        label: "Unavailable",
        startBlocked: "Available in the desktop app",
        stopBlocked: "Available in the desktop app",
        restartBlocked: "Available in the desktop app",
        pauseBlocked: "Available in the desktop app",
        resumeBlocked: "Available in the desktop app",
      };
  const bots = ws.native ? summarizeBots(ws.bots) : { ...unavailable, label: "Unavailable" };
  const plan = planSetup({ environment: ws.environment, repos: ws.repos, build: ws.build, client: ws.client, extract: ws.extract, database: ws.database, config: ws.config });
  const setupDisabled = !ws.native ? "Available in the desktop app" : plan.blocked ?? (plan.next === null ? "Everything is already set up." : null);
  const launchWhy = ws.native ? launchBlocked(ws.client, ws.server) : "Available in the desktop app";
  const paused = ws.server?.state === "paused" || (ws.server?.services.some((s) => s.state === "paused") ?? false);
  const canSetup = ws.native && ws.recipe !== null && gitReady(ws.environment);
  const canBuild = ws.native && ws.recipe !== null && build.blocked === null;
  const canExtract = ws.native && ws.recipe !== null && extract.blocked === null;
  const canDatabase = ws.native && ws.recipe !== null && db.blocked === null;
  const canConfigure = ws.native && ws.recipe !== null && config.blocked === null;
  const configLabel = { generate: "Generate configuration", update: "Update configuration", again: "Regenerate configuration" }[config.action];
  const databaseLabel = {
    setup: "Set up database",
    start: "Start database",
    finish: "Finish database setup",
    update: "Update database",
    check: "Check database",
  }[db.action];
  const extractLabel = { extract: "Extract client data", resume: "Resume extraction", again: "Extract again" }[extract.action];
  const Icon = repos.action === "update" ? RefreshCw : Download;

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      <section className="relative isolate shrink-0 overflow-hidden rounded-lg border border-line p-6">
        <div
          aria-hidden
          className="absolute inset-0 -z-10"
          style={{
            backgroundImage: [
              "radial-gradient(60% 130% at 90% 0%, color-mix(in srgb, var(--color-accent) 30%, transparent), transparent 70%)",
              "radial-gradient(45% 110% at 5% 140%, color-mix(in srgb, var(--color-warn) 16%, transparent), transparent 70%)",
              "repeating-linear-gradient(135deg, rgb(255 255 255 / 0.018) 0 1px, transparent 1px 22px)",
              "linear-gradient(135deg, var(--color-panel-2), var(--color-panel))",
            ].join(", "),
          }}
        />
        <Gem
          aria-hidden
          strokeWidth={1}
          className="absolute -right-6 top-1/2 -z-10 size-64 -translate-y-1/2 rotate-12 text-accent/30 [mask-image:linear-gradient(to_left,black,transparent_80%)]"
        />
        <h1 className="text-2xl font-semibold tracking-tight">WorldStone</h1>
        <p className="mt-1 text-sm text-muted">
          {ws.recipe ? `${ws.recipe.name} · ${ws.recipe.game}` : "Build and run World of Warcraft emulator servers."}
        </p>
        <div className="mt-5 flex flex-wrap items-center gap-2">
          <Button
            variant={plan.next !== null && plan.blocked === null ? "primary" : "secondary"}
            onClick={ws.setUp}
            busy={ws.activeAction === "setup"}
            disabled={setupDisabled !== null || ws.recipe === null || ws.running}
            title={setupDisabled ?? "Runs every step that is still missing, in order: download source, build, choose your game client, extract its data, database, configuration."}
          >
            {ws.activeAction !== "setup" && <Wrench className="size-4" aria-hidden />}
            Set up
          </Button>
          <Button
            variant={config.status === "ready" && server.status !== "ready" ? "primary" : "secondary"}
            onClick={ws.startServer}
            busy={ws.activeAction === "start"}
            disabled={!ws.native || ws.recipe === null || server.startBlocked !== null || ws.running}
            title={server.startBlocked ?? undefined}
          >
            {ws.activeAction !== "start" && <Play className="size-4" aria-hidden />}
            Start servers
          </Button>
          {paused ? (
            <Button
              onClick={ws.resumeServer}
              busy={ws.activeAction === "resume"}
              disabled={!ws.native || ws.recipe === null || server.resumeBlocked !== null || ws.running}
              title={server.resumeBlocked ?? "Let the frozen servers carry on."}
            >
              {ws.activeAction !== "resume" && <Play className="size-4" aria-hidden />}
              Resume servers
            </Button>
          ) : (
            <Button
              onClick={ws.pauseServer}
              busy={ws.activeAction === "pause"}
              disabled={!ws.native || ws.recipe === null || server.pauseBlocked !== null || ws.running}
              title={server.pauseBlocked ?? "Freeze the servers where they are. Nobody can play until you resume; nothing is saved or closed."}
            >
              {ws.activeAction !== "pause" && <Pause className="size-4" aria-hidden />}
              Pause servers
            </Button>
          )}
          <Button
            onClick={ws.restartServer}
            busy={ws.activeAction === "restart"}
            disabled={!ws.native || ws.recipe === null || server.restartBlocked !== null || ws.running}
            title={server.restartBlocked ?? undefined}
          >
            {ws.activeAction !== "restart" && <RotateCw className="size-4" aria-hidden />}
            Restart servers
          </Button>
          <Button
            onClick={ws.stopServer}
            busy={ws.activeAction === "stop"}
            disabled={!ws.native || ws.recipe === null || server.stopBlocked !== null || ws.running}
            title={server.stopBlocked ?? undefined}
          >
            {ws.activeAction !== "stop" && <Square className="size-4" aria-hidden />}
            Stop servers
          </Button>
          <Button
            className="ml-auto"
            variant={launchWhy === null ? "primary" : "secondary"}
            onClick={ws.launchGame}
            busy={ws.activeAction === "launch"}
            disabled={!ws.native || ws.recipe === null || launchWhy !== null || ws.running}
            title={launchWhy ?? "Starts your game client and points it at this server first (the address it had is saved)."}
          >
            {ws.activeAction !== "launch" && <Swords className="size-4" aria-hidden />}
            Launch game
          </Button>
        </div>
        {ws.setupNote && <p className="mt-3 text-xs text-muted">{ws.setupNote}</p>}
      </section>

      <ErrorBanner message={ws.error} onDismiss={ws.dismissError} />

      <div className="grid shrink-0 grid-cols-[repeat(auto-fill,minmax(17rem,1fr))] gap-2">
        <StatusCard icon={Container} title="Dependencies" status={env.status} detail={env.detail} />
        <StatusCard icon={GitBranch} title="Source repositories" status={repos.status} detail={repos.detail} />
        <StatusCard icon={Hammer} title="Server build" status={build.status} detail={build.detail} />
        <StatusCard icon={Gamepad2} title="Game client" status={client.status} detail={client.detail} />
        <StatusCard icon={PackageOpen} title="Client data" status={extract.status} detail={extract.detail} />
        <StatusCard icon={Database} title="Database" status={db.status} detail={db.detail} />
        <StatusCard icon={FileCog} title="Configuration" status={config.status} detail={config.detail} />
        <StatusCard icon={Server} title="Servers" status={server.status} label={server.label} detail={server.detail} />
        <StatusCard icon={Bot} title="Playerbots" status={bots.status} label={bots.label} detail={bots.detail} />
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant={repos.action === "download" ? "primary" : "secondary"}
          onClick={ws.setupRepositories}
          busy={ws.activeAction === "source"}
          disabled={!canSetup || ws.running}
          title={canSetup ? undefined : "Git must be installed, and the desktop app must be running."}
        >
          {ws.activeAction !== "source" && <Icon className="size-4" aria-hidden />}
          {repos.action === "update" ? "Update source" : "Download source"}
        </Button>
        <Button
          variant={repos.action === "update" && build.action === "build" ? "primary" : "secondary"}
          onClick={() => ws.buildServer(build.action === "rebuild")}
          busy={ws.activeAction === "build"}
          disabled={!canBuild || ws.running}
          title={build.blocked ?? undefined}
        >
          {ws.activeAction !== "build" && <Hammer className="size-4" aria-hidden />}
          {build.action === "rebuild" ? "Rebuild server" : "Build server"}
        </Button>
        <Button
          variant={client.status === "ready" ? "secondary" : "primary"}
          onClick={ws.chooseClient}
          busy={ws.choosingClient}
          disabled={!ws.native || ws.recipe === null || ws.running}
          title="Pick the folder that contains WoW.exe"
        >
          {!ws.choosingClient && <FolderOpen className="size-4" aria-hidden />}
          {client.status === "ready" ? "Change client" : "Choose client"}
        </Button>
        <Button
          variant={client.status === "ready" && extract.status !== "ready" ? "primary" : "secondary"}
          onClick={ws.extractClientData}
          busy={ws.activeAction === "extract"}
          disabled={!canExtract || ws.running}
          title={extract.blocked ?? undefined}
        >
          {ws.activeAction !== "extract" && <PackageOpen className="size-4" aria-hidden />}
          {extractLabel}
        </Button>
        <Button
          variant={db.status === "ready" ? "secondary" : "primary"}
          onClick={ws.provisionDatabase}
          busy={ws.activeAction === "database"}
          disabled={!canDatabase || ws.running}
          title={db.blocked ?? undefined}
        >
          {ws.activeAction !== "database" && <Database className="size-4" aria-hidden />}
          {databaseLabel}
        </Button>
        <Button
          variant={db.status === "ready" && config.status !== "ready" ? "primary" : "secondary"}
          onClick={ws.configureServer}
          busy={ws.activeAction === "configure"}
          disabled={!canConfigure || ws.running}
          title={config.blocked ?? undefined}
        >
          {ws.activeAction !== "configure" && <FileCog className="size-4" aria-hidden />}
          {configLabel}
        </Button>
      </div>
      {ws.native && (
        <ul className="-mt-2 space-y-0.5 text-xs text-muted">
          {build.blocked && build.blocked !== CHECKING && <li>Build: {build.blocked}</li>}
          {extract.blocked && extract.blocked !== CHECKING && <li>Extraction: {extract.blocked}</li>}
          {db.blocked && db.blocked !== CHECKING && <li>Database: {db.blocked}</li>}
          {config.blocked && config.blocked !== CHECKING && <li>Configuration: {config.blocked}</li>}
          {server.startBlocked && server.startBlocked !== CHECKING && server.status !== "ready" && server.label !== "Paused" && <li>Start: {server.startBlocked}</li>}
          {launchWhy && launchWhy !== CHECKING && <li>Launch game: {launchWhy}</li>}
        </ul>
      )}

      <TaskPanel task={ws.task} starting={ws.starting} onCancel={ws.cancel} />
      <LogPanel logs={ws.logs} className="min-h-40 flex-1" />
    </div>
  );
}
