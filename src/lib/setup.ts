// The one-click "Set up" chain: which step comes next, decided from the same state the cards show.
// Pure, so it is tested. The steps themselves are the ordinary buttons' backend commands.

import type { BuildStatus, Check, ClientStatus, ConfigStatus, DatabaseStatus, ExtractStatus, RepoStatus } from "../api/types";
import {
  CHECKING,
  summarizeBuild,
  summarizeClient,
  summarizeConfig,
  summarizeDatabase,
  summarizeEnvironment,
  summarizeExtract,
  summarizeRepos,
} from "./summary";

export type SetupStep = "source" | "build" | "client" | "extract" | "database" | "configure";

/** What each step is called while it runs, in the order Set up does them. */
export const SETUP_STEPS: Array<{ id: SetupStep; label: string }> = [
  { id: "source", label: "Download source" },
  { id: "build", label: "Build server" },
  { id: "client", label: "Choose game client" },
  { id: "extract", label: "Extract client data" },
  { id: "database", label: "Set up database" },
  { id: "configure", label: "Generate configuration" },
];

export interface SetupState {
  environment: Check[] | null;
  repos: RepoStatus[] | null;
  build: BuildStatus | null;
  client: ClientStatus | null;
  extract: ExtractStatus | null;
  database: DatabaseStatus | null;
  config: ConfigStatus | null;
}

export interface SetupPlan {
  /** The step to run now; null when nothing is left or the chain cannot go on. */
  next: SetupStep | null;
  /** How that step (or the whole chain) is doing, for saying why a step is being run again. */
  detail: string;
  /** Why the chain cannot go on, or null. */
  blocked: string | null;
  /** Whether the build should ignore its cache (built, but playerbots are not proven in it). */
  force: boolean;
}

/** The first step whose result is not in place yet. */
export function planSetup(s: SetupState): SetupPlan {
  const stop = (blocked: string): SetupPlan => ({ next: null, detail: blocked, blocked, force: false });
  const run = (next: SetupStep, detail: string, force = false): SetupPlan => ({ next, detail, blocked: null, force });

  const env = summarizeEnvironment(s.environment);
  if (env.status === "unknown" && env.detail === CHECKING) return stop(CHECKING);
  if (env.status === "missing" || env.status === "error") return stop(env.detail);

  const repos = summarizeRepos(s.repos);
  if (repos.status === "unknown") return stop(repos.detail);
  if (repos.status !== "ready") return run("source", repos.detail);

  const build = summarizeBuild(s.build);
  if (build.status === "unknown") return stop(build.detail);
  if (build.status !== "ready") {
    if (build.blocked && build.blocked !== CHECKING) return stop(build.blocked);
    return run("build", build.detail, build.action === "rebuild");
  }

  const client = summarizeClient(s.client);
  if (client.status === "unknown") return stop(client.detail);
  if (client.status !== "ready") return run("client", client.detail);

  const extract = summarizeExtract(s.extract, s.client);
  if (extract.status === "unknown") return stop(extract.detail);
  if (extract.status !== "ready") {
    if (extract.blocked && extract.blocked !== CHECKING) return stop(extract.blocked);
    return run("extract", extract.detail);
  }

  const database = summarizeDatabase(s.database);
  if (database.status === "unknown") return stop(database.detail);
  if (database.status !== "ready") {
    if (database.blocked && database.blocked !== CHECKING) return stop(database.blocked);
    return run("database", database.detail);
  }

  const config = summarizeConfig(s.config, s.build);
  if (config.status === "unknown") return stop(config.detail);
  if (config.status !== "ready") {
    if (config.blocked && config.blocked !== CHECKING) return stop(config.blocked);
    return run("configure", config.detail);
  }

  return { next: null, detail: "Everything is set up", blocked: null, force: false };
}
