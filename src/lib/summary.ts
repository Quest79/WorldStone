// Turns raw backend state into the status shown on dashboard cards. Pure, so it is tested.

import type {
  BotChoice,
  BotsStatus,
  BuildStatus,
  Check,
  ClientStatus,
  ConfigStatus,
  ConnectStatus,
  DatabaseStatus,
  ExtractStatus,
  RepoStatus,
  ServerStatus,
  ServiceStatus,
  Status,
} from "../api/types";

/** What a summary says while the backend has not answered yet. Not worth a "why is this disabled" line. */
export const CHECKING = "Checking…";

export interface Summary {
  status: Status;
  detail: string;
}

export function summarizeEnvironment(checks: Check[] | null): Summary {
  if (!checks) return { status: "unknown", detail: "Checking…" };
  const needsAttention = checks.filter((c) => c.status === "missing" || c.status === "error");
  if (needsAttention.length > 0) {
    return {
      status: "missing",
      detail: `${needsAttention[0].label}: ${needsAttention[0].detail}`,
    };
  }
  const warnings = checks.filter((c) => c.status === "warning");
  if (warnings.length > 0) {
    return { status: "warning", detail: `${warnings[0].label}: ${warnings[0].detail}` };
  }
  if (checks.every((c) => c.status === "unknown")) return { status: "unknown", detail: "Checking…" };
  return { status: "ready", detail: "Git and Docker are ready" };
}

/** Downloading source needs only Git on the host; everything else runs in containers. */
export function gitReady(checks: Check[] | null): boolean {
  return checks?.find((c) => c.id === "git")?.status === "ready";
}

export interface RepoSummaryState extends Summary {
  /** What the primary button should offer. */
  action: "download" | "update";
}

export function summarizeRepos(repos: RepoStatus[] | null): RepoSummaryState {
  if (!repos) return { status: "unknown", detail: "Checking…", action: "download" };
  const missing = repos.filter((r) => r.state === "missing").length;
  const incomplete = repos.filter((r) => r.state === "incomplete");
  const ready = repos.length - missing - incomplete.length;

  if (incomplete.length > 0) {
    return { status: "warning", detail: `${incomplete[0].id}: ${incomplete[0].detail}`, action: "download" };
  }
  if (missing === repos.length) {
    return { status: "missing", detail: "Not downloaded yet", action: "download" };
  }
  if (missing > 0) {
    return { status: "warning", detail: `${missing} of ${repos.length} not downloaded`, action: "download" };
  }
  const local = repos.filter((r) => r.dirty).length;
  const detail = `${ready} downloaded${local > 0 ? `, ${local} with local changes` : ""}`;
  return { status: "ready", detail, action: "update" };
}

export interface BuildSummary extends Summary {
  /** What the button should offer: a build is needed, or the current one can be redone. */
  action: "build" | "rebuild";
  /** Why the button is disabled, or null when a build can start. */
  blocked: string | null;
}

export function summarizeBuild(build: BuildStatus | null): BuildSummary {
  if (!build) return { status: "unknown", detail: "Checking…", action: "build", blocked: CHECKING };
  const { blocked, commit } = build;
  switch (build.state) {
    case "unknown":
      return { status: "unknown", detail: blocked ?? "Docker could not be asked.", action: "build", blocked: blocked ?? "Docker could not be asked." };
    case "notBuilt":
      return { status: "missing", detail: "Not built yet", action: "build", blocked };
    case "outdated":
      return { status: "warning", detail: `The source has changed since the last build (${commit ?? "unknown commit"})`, action: "build", blocked };
    case "ready":
      if (build.playerbots !== true) {
        return { status: "warning", detail: "Built, but playerbots are not proven to be included. Rebuild.", action: "rebuild", blocked };
      }
      return { status: "ready", detail: `Playerbots compiled in · ${commit ?? "unknown commit"}`, action: "rebuild", blocked };
  }
}

export function formatBytes(bytes: number): string {
  const mib = bytes / (1024 * 1024);
  return mib >= 1024 ? `${(mib / 1024).toFixed(1)} GiB` : `${Math.round(mib)} MiB`;
}

export function summarizeClient(client: ClientStatus | null): Summary {
  if (!client) return { status: "unknown", detail: "Checking…" };
  if (client.info) {
    const { version, build, dataBytes } = client.info;
    return { status: "ready", detail: `Client ${version} (build ${build}) · ${formatBytes(dataBytes)} of data` };
  }
  if (client.problem) return { status: "warning", detail: client.problem };
  return { status: "missing", detail: "No game client chosen yet" };
}

export interface ExtractSummary extends Summary {
  /** What the button should offer. */
  action: "extract" | "resume" | "again";
  /** Why the button is disabled, or null when extraction can start. */
  blocked: string | null;
}

export function summarizeExtract(extract: ExtractStatus | null, client: ClientStatus | null): ExtractSummary {
  if (!extract) return { status: "unknown", detail: "Checking…", action: "extract", blocked: CHECKING };
  const { blocked, record } = extract;
  const action = extract.resumable ? "resume" : "extract";
  switch (extract.state) {
    case "unknown": {
      const why = blocked ?? extract.note ?? "Docker could not be asked.";
      return { status: "unknown", detail: why, action, blocked: why };
    }
    case "notExtracted": {
      if (extract.resumable) {
        return { status: "warning", detail: "An earlier extraction was interrupted. Start it again to continue where it stopped.", action, blocked };
      }
      return { status: extract.note ? "warning" : "missing", detail: extract.note ?? "Not extracted yet", action, blocked };
    }
    case "ready": {
      if (!record) return { status: "unknown", detail: "The data is there but its record could not be read.", action: "again", blocked };
      const chosen = client?.info?.build;
      if (chosen !== undefined && chosen !== record.clientBuild) {
        return {
          status: "warning",
          detail: `Extracted from client ${record.clientVersion}, but the chosen client is build ${chosen}. Extract again to match it.`,
          action: "again",
          blocked,
        };
      }
      const bytes = record.outputs.reduce((sum, o) => sum + o.bytes, 0);
      return { status: "ready", detail: `From client ${record.clientVersion} · ${formatBytes(bytes)} extracted`, action: "again", blocked };
    }
  }
}

export interface DatabaseSummary extends Summary {
  /** What the button should offer. */
  action: "setup" | "start" | "finish" | "update" | "check";
  /** Why the button is disabled, or null when setting up can start. */
  blocked: string | null;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** What is missing, in one line, from the facts the backend read out of the database. */
export function describeGaps(database: DatabaseStatus): string {
  const gaps: string[] = [];
  for (const s of database.schemas) {
    if (s.unmanaged) gaps.push(`${s.name} already holds tables this app did not create`);
    else if (!s.setupDone) gaps.push(`${s.name} is not set up`);
    else if (s.populatePending > 0) gaps.push(`${s.name} still has ${plural(s.populatePending, "data file")} to load`);
    else if (s.missingTables.length > 0) gaps.push(`${s.name} lacks ${s.missingTables.join(", ")}`);
  }
  if ((database.realms ?? 0) === 0) gaps.push("no realm is listed for players to join");
  return gaps.join("; ");
}

export function summarizeDatabase(database: DatabaseStatus | null): DatabaseSummary {
  if (!database) return { status: "unknown", detail: "Checking…", action: "setup", blocked: CHECKING };
  const { blocked } = database;
  switch (database.state) {
    case "unknown": {
      const why = blocked ?? database.note ?? "Docker could not be asked.";
      return { status: "unknown", detail: why, action: "setup", blocked: why };
    }
    case "notProvisioned":
      return { status: "missing", detail: "Not set up yet", action: "setup", blocked };
    case "stopped":
      return { status: "warning", detail: "Stopped. Start it to check its contents.", action: "start", blocked };
    case "starting":
      return { status: "warning", detail: "Starting…", action: "start", blocked: "The database is still starting." };
    case "incomplete": {
      // An unmanaged schema is never touched, so offering to "finish" would only fail again.
      const refused = database.schemas.some((s) => s.unmanaged);
      return { status: "warning", detail: describeGaps(database), action: "finish", blocked: refused ? describeGaps(database) : blocked };
    }
    case "outdated": {
      const waiting = database.schemas.filter((s) => s.updatesPending > 0);
      const total = waiting.reduce((sum, s) => sum + s.updatesPending, 0);
      const where = waiting.map((s) => `${s.name} ${s.updatesPending}`).join(", ");
      return { status: "warning", detail: `${plural(total, "update")} waiting (${where}). A backup is taken first.`, action: "update", blocked };
    }
    case "ready": {
      const versions = database.schemas.map((s) => `${s.name} ${s.version ?? "?"}`).join(" · ");
      return { status: "ready", detail: `${versions} · ${plural(database.realms ?? 0, "realm")}`, action: "check", blocked };
    }
  }
}

export interface ConfigSummary extends Summary {
  /** What the button should offer. */
  action: "generate" | "update" | "again";
  /** Why the button is disabled, or null when the files can be written. */
  blocked: string | null;
}

/** The files that are not right, and what is wrong with each, in one line. */
export function describeConfigGaps(config: ConfigStatus): string {
  return config.files
    .filter((f) => f.state !== "ready")
    .map((f) => `${f.label}: ${f.state === "missing" ? "not created" : f.staleKeys.join(", ")}`)
    .join("; ");
}

export function summarizeConfig(config: ConfigStatus | null, build: BuildStatus | null): ConfigSummary {
  if (!config) return { status: "unknown", detail: "Checking…", action: "generate", blocked: CHECKING };
  // The sample files come out of the built image, so writing them needs one.
  const blocked =
    config.blocked ??
    (build === null ? CHECKING : build.state === "notBuilt" ? "Build the server first: its sample configuration files come from the built image." : build.state === "unknown" ? (build.blocked ?? "Docker could not be asked.") : null);
  switch (config.state) {
    case "unknown": {
      const why = config.note ?? config.blocked ?? "The configuration cannot be read.";
      return { status: "unknown", detail: why, action: "generate", blocked: blocked ?? why };
    }
    case "notConfigured":
      return { status: "missing", detail: "Not generated yet", action: "generate", blocked };
    case "outdated":
      return { status: "warning", detail: `Out of date: ${describeConfigGaps(config)}`, action: "update", blocked };
    case "ready": {
      const bots = config.bots;
      const roster = bots === null ? "" : bots.enabled ? ` · ${bots.min}–${bots.max} bots` : " · bots off";
      return { status: "ready", detail: `${plural(config.files.length, "file")} in place${roster}`, action: "again", blocked };
    }
  }
}

export interface ServerSummary extends Summary {
  /** What the badge says instead of the generic word for the status. */
  label: string;
  /** Why Start is disabled, or null when the servers can be started. */
  startBlocked: string | null;
  /** Why Stop is disabled, or null. */
  stopBlocked: string | null;
  /** Why Restart is disabled, or null. */
  restartBlocked: string | null;
  /** Why Pause is disabled, or null when the servers can be frozen. */
  pauseBlocked: string | null;
  /** Why Resume is disabled, or null when frozen servers can carry on. */
  resumeBlocked: string | null;
}

export interface ServerGates {
  build: BuildStatus | null;
  extract: ExtractStatus | null;
  database: DatabaseStatus | null;
  config: ConfigStatus | null;
}

/** What has to be in place before the servers can start, in the order a person fixes it. */
export function whatStartNeeds({ build, extract, database, config }: ServerGates): string | null {
  if (!build || !extract || !database || !config) return CHECKING;
  if (build.state === "unknown") return build.blocked ?? "Docker could not be asked.";
  if (build.state === "notBuilt") return "Build the server first.";
  if (extract.state === "unknown") return extract.blocked ?? extract.note ?? "Docker could not be asked.";
  if (extract.state !== "ready") return "Extract the client data first.";
  switch (database.state) {
    case "unknown":
      return database.blocked ?? database.note ?? "Docker could not be asked.";
    case "notProvisioned":
      return "Set up the database first.";
    case "incomplete":
      return "Finish setting up the database first.";
    case "outdated":
      return "Update the database first.";
    default:
      break;
  }
  switch (config.state) {
    case "unknown":
      return config.note ?? config.blocked ?? "The configuration cannot be read.";
    case "notConfigured":
      return "Generate the configuration first.";
    case "outdated":
      return "The configuration is out of date. Generate it again first.";
    default:
      return null;
  }
}

/** Why the servers cannot start although everything is in place: another instance's servers hold their ports. */
export function portsTaken(server: ServerStatus): string | null {
  if (server.heldBy.length === 0) return null;
  const names = server.heldBy.join(" and ");
  return `The ${names} ${server.heldBy.length === 1 ? "server is" : "servers are"} running and use${server.heldBy.length === 1 ? "s" : ""} the same ports. Stop ${server.heldBy.length === 1 ? "it" : "them"} first.`;
}

/** Shown while the servers still work from a configuration that has since been replaced. */
export const RESTART_HINT = "The configuration changed after the servers started. Restart them to apply it.";

const SERVICE_WORD = {
  missing: "not created",
  stopped: "stopped",
  starting: "starting",
  running: "running",
  paused: "paused",
  unresponsive: "not answering",
  crashed: "crashed",
} as const;

/** `Realm server crashed (exit 1)`: one service, in words. */
export function describeService(s: ServiceStatus): string {
  const code = s.state === "crashed" && s.exitCode !== null ? ` (exit ${s.exitCode})` : "";
  return `${s.label} ${SERVICE_WORD[s.state]}${code}`;
}

/** Pause and resume from the state alone; the rest of the summary is [`describeServer`]. */
export function summarizeServer(server: ServerStatus | null, gates: ServerGates): ServerSummary {
  const core = describeServer(server, gates);
  if (!server) return { ...core, pauseBlocked: CHECKING, resumeBlocked: CHECKING };
  const frozen = server.services.some((s) => s.state === "paused");
  const why = server.note ?? "Docker could not be asked.";
  const pauseBlocked =
    server.state === "running"
      ? null
      : server.state === "unknown"
        ? why
        : server.state === "paused"
          ? "The servers are already paused."
          : server.state === "starting"
            ? "The servers are still starting."
            : "The servers are not running.";
  const resumeBlocked = frozen ? null : server.state === "unknown" ? why : "The servers are not paused.";
  return { ...core, pauseBlocked, resumeBlocked };
}

type ServerCore = Omit<ServerSummary, "pauseBlocked" | "resumeBlocked">;

function describeServer(server: ServerStatus | null, gates: ServerGates): ServerCore {
  if (!server) {
    return { status: "unknown", label: "Checking", detail: "Checking…", startBlocked: CHECKING, stopBlocked: CHECKING, restartBlocked: CHECKING };
  }
  const gate = whatStartNeeds(gates);
  const parts = server.services.map(describeService);
  // The database container may still be up after the servers were stopped, and Stop closes it too.
  const databaseUp = gates.database !== null && ["starting", "incomplete", "outdated", "ready"].includes(gates.database.state);
  const nothingToStop = databaseUp ? null : "Nothing is running.";
  switch (server.state) {
    case "unknown": {
      const why = server.note ?? "Docker could not be asked.";
      return { status: "unknown", label: "Unknown", detail: why, startBlocked: why, stopBlocked: why, restartBlocked: why };
    }
    case "stopped":
      return {
        status: "missing",
        label: "Stopped",
        detail: portsTaken(server) ?? "Not running",
        startBlocked: gate ?? portsTaken(server),
        stopBlocked: nothingToStop,
        restartBlocked: "Nothing is running. Use Start.",
      };
    case "starting":
      return { status: "warning", label: "Starting", detail: parts.join(" · "), startBlocked: "The servers are already starting.", stopBlocked: null, restartBlocked: gate };
    case "running": {
      const where = server.services.map((s) => `${s.label} on ${server.host}:${s.port}`).join(" · ");
      const trouble = server.note !== null;
      const stale = server.restartNeeded;
      return {
        status: trouble || stale ? "warning" : "ready",
        label: "Running",
        detail: trouble ? (server.note ?? where) : stale ? RESTART_HINT : where,
        startBlocked: "The servers are already running.",
        stopBlocked: null,
        restartBlocked: gate,
      };
    }
    case "paused":
      return {
        status: "warning",
        label: "Paused",
        detail: "Frozen: nobody can play until you resume. The database is still running.",
        startBlocked: "The servers are paused. Resume them instead.",
        stopBlocked: null,
        restartBlocked: gate,
      };
    case "partial":
      return { status: "warning", label: "Partly running", detail: parts.join(" · "), startBlocked: gate, stopBlocked: null, restartBlocked: gate };
    case "failing":
      return { status: "error", label: "Failing", detail: parts.join(" · "), startBlocked: gate, stopBlocked: null, restartBlocked: gate };
  }
}

/** Why the game cannot be launched now, or null when it can: a usable client and servers that answer. */
export function launchBlocked(client: ClientStatus | null, server: ServerStatus | null): string | null {
  if (!client || !server) return CHECKING;
  if (!client.info) return client.problem ?? "Choose your game client first.";
  switch (server.state) {
    case "running":
      return null;
    case "paused":
      return "The servers are paused. Resume them first.";
    case "starting":
      return "The servers are still starting.";
    case "unknown":
      return server.note ?? "Docker could not be asked whether the servers are running.";
    default:
      return "Start the servers first.";
  }
}

export interface BotsSummary extends Summary {
  /** What the badge says instead of the generic word for the status. */
  label: string;
}

/** Whether the playerbots are really in the world, from the counts the backend read. */
export function summarizeBots(bots: BotsStatus | null): BotsSummary {
  if (!bots) return { status: "unknown", label: "Checking", detail: "Checking…" };
  const restart = bots.restartNeeded ? " Restart the servers to apply the saved settings." : "";
  switch (bots.state) {
    case "unknown":
      return { status: "unknown", label: "Unknown", detail: bots.note };
    case "off":
      return { status: "warning", label: "Off", detail: bots.note + restart };
    case "stopped":
      return { status: "missing", label: "Stopped", detail: bots.note + restart };
    case "starting":
      return { status: "warning", label: "Waiting for the world", detail: bots.note };
    case "creating":
      return { status: "warning", label: "Creating bots", detail: bots.note };
    case "loggingIn":
      return { status: "warning", label: "Logging in", detail: bots.note };
    case "online": {
      const players = bots.players ?? 0;
      const also = players > 0 ? ` · ${plural(players, "player")} besides` : "";
      return { status: "ready", label: "Online", detail: `${bots.online ?? 0} of up to ${bots.max} bots in the world${also}.${restart}` };
    }
    case "stalled":
      return { status: "error", label: "Stalled", detail: bots.note };
  }
}

/** Why the bot settings cannot be saved as typed, or null. Mirrors the backend's rules. */
export function botChoiceProblem(choice: BotChoice): string | null {
  const whole = (n: number) => Number.isInteger(n) && n >= 0;
  if (!whole(choice.min) || !whole(choice.max)) return "Enter whole numbers, zero or more.";
  if (choice.max > 1000) return "More than 1000 bots is not supported.";
  if (choice.min > choice.max) return "The minimum number of bots cannot be above the maximum.";
  return null;
}

export interface ConnectSummary extends Summary {
  /** Why the client cannot be pointed here, or null. */
  pointBlocked: string | null;
  /** Why the saved address cannot be put back, or null. */
  restoreBlocked: string | null;
}

/** Where the game client logs in, and what the two buttons can do about it. */
export function summarizeConnect(connect: ConnectStatus | null): ConnectSummary {
  if (!connect) return { status: "unknown", detail: "Checking…", pointBlocked: CHECKING, restoreBlocked: CHECKING };
  const nothingSaved = "There is no saved address to go back to.";
  switch (connect.state) {
    case "noClient": {
      const why = "Choose your game client first.";
      return { status: "missing", detail: "No game client chosen yet", pointBlocked: why, restoreBlocked: why };
    }
    case "unknown": {
      const why = connect.note ?? "The client's realm list file cannot be read.";
      return { status: "unknown", detail: why, pointBlocked: why, restoreBlocked: why };
    }
    case "noFile":
      return { status: "warning", detail: "The client has no realm list file yet. Pointing it creates one.", pointBlocked: null, restoreBlocked: connect.backup === null ? nothingSaved : null };
    case "pointed": {
      const saved = connect.backup === null ? nothingSaved : connect.backupAddress === connect.address ? "The saved copy holds the same address, so there is nothing to go back to." : null;
      return { status: "ready", detail: `The client logs in to ${connect.address ?? connect.wanted}`, pointBlocked: `The client already logs in to ${connect.wanted}.`, restoreBlocked: saved };
    }
    case "elsewhere": {
      const detail = connect.address ? `The client logs in to ${connect.address}, not to this server` : "The client's realm list file names no address";
      return { status: "warning", detail, pointBlocked: null, restoreBlocked: `The client does not log in to ${connect.wanted}, so there is nothing to undo.` };
    }
  }
}
