import { describe, expect, it } from "vitest";
import type {
  BotsStatus,
  BuildStatus,
  Check,
  ClientStatus,
  ConfigStatus,
  ConnectStatus,
  DatabaseStatus,
  ExtractStatus,
  RepoStatus,
  SchemaStatus,
  ServerStatus,
  ServiceStatus,
} from "../api/types";
import {
  botChoiceProblem,
  describeConfigGaps,
  describeGaps,
  formatBytes,
  gitReady,
  summarizeBots,
  summarizeBuild,
  summarizeClient,
  summarizeConfig,
  summarizeConnect,
  summarizeDatabase,
  summarizeEnvironment,
  summarizeExtract,
  summarizeRepos,
  summarizeServer,
  launchBlocked,
  RESTART_HINT,
  whatStartNeeds,
} from "./summary";

const check = (id: string, status: Check["status"], detail = ""): Check => ({ id, label: id, status, detail });

const repo = (id: string, state: RepoStatus["state"], extra: Partial<RepoStatus> = {}): RepoStatus => ({
  id,
  url: `https://example.com/${id}.git`,
  branch: "master",
  state,
  commit: null,
  summary: null,
  dirty: false,
  detail: "",
  ...extra,
});

describe("summarizeEnvironment", () => {
  it("is unknown until the first check returns", () => {
    expect(summarizeEnvironment(null).status).toBe("unknown");
  });

  it("is ready when every check passes", () => {
    const s = summarizeEnvironment([check("git", "ready"), check("docker", "ready")]);
    expect(s.status).toBe("ready");
  });

  it("reports the first missing dependency and how to fix it", () => {
    const s = summarizeEnvironment([check("git", "ready"), check("docker-engine", "missing", "Start Docker Desktop.")]);
    expect(s.status).toBe("missing");
    expect(s.detail).toContain("Start Docker Desktop.");
  });

  it("ranks a missing dependency above a warning", () => {
    const s = summarizeEnvironment([check("a", "warning", "w"), check("b", "missing", "m")]);
    expect(s.status).toBe("missing");
  });

  it("surfaces warnings when nothing is missing", () => {
    expect(summarizeEnvironment([check("a", "ready"), check("b", "warning", "Use Linux containers")]).status).toBe("warning");
  });
});

describe("gitReady", () => {
  it("needs a ready git check specifically", () => {
    expect(gitReady([check("git", "ready"), check("docker", "missing")])).toBe(true);
    expect(gitReady([check("git", "missing"), check("docker", "ready")])).toBe(false);
    expect(gitReady(null)).toBe(false);
  });
});

describe("summarizeRepos", () => {
  it("offers download when nothing is there", () => {
    const s = summarizeRepos([repo("server", "missing"), repo("database", "missing")]);
    expect(s).toMatchObject({ status: "missing", action: "download" });
  });

  it("offers update when everything is downloaded", () => {
    const s = summarizeRepos([repo("server", "ready"), repo("database", "ready")]);
    expect(s).toMatchObject({ status: "ready", action: "update" });
    expect(s.detail).toBe("2 downloaded");
  });

  it("mentions local changes without treating them as a problem", () => {
    const s = summarizeRepos([repo("server", "ready", { dirty: true }), repo("database", "ready")]);
    expect(s.status).toBe("ready");
    expect(s.detail).toContain("1 with local changes");
  });

  it("flags a partial download as needing attention and keeps offering download", () => {
    const s = summarizeRepos([repo("server", "ready"), repo("database", "missing")]);
    expect(s).toMatchObject({ status: "warning", action: "download", detail: "1 of 2 not downloaded" });
  });

  it("explains an incomplete checkout", () => {
    const s = summarizeRepos([repo("server", "incomplete", { detail: "Submodules not downloaded: dep" })]);
    expect(s.status).toBe("warning");
    expect(s.detail).toContain("Submodules not downloaded");
  });

  it("is unknown until repositories have been inspected", () => {
    expect(summarizeRepos(null).status).toBe("unknown");
  });
});

const build = (extra: Partial<BuildStatus> = {}): BuildStatus => ({
  state: "ready",
  blocked: null,
  image: "worldstone/mangos-zero:latest",
  builtAt: "2026-09-29T05:22:11Z",
  sizeBytes: 412 * 1024 * 1024,
  commit: "a1b2c3d4e5f6",
  playerbots: true,
  ...extra,
});

describe("summarizeBuild", () => {
  it("is unknown and not startable until the first answer arrives", () => {
    const s = summarizeBuild(null);
    expect(s.status).toBe("unknown");
    expect(s.blocked).not.toBeNull();
  });

  it("offers a build when nothing has been built", () => {
    expect(summarizeBuild(build({ state: "notBuilt", commit: null, playerbots: null }))).toMatchObject({
      status: "missing",
      action: "build",
      blocked: null,
    });
  });

  it("says why a build cannot start, and still reports whether one exists", () => {
    const s = summarizeBuild(build({ state: "notBuilt", blocked: "Download the source first." }));
    expect(s.status).toBe("missing");
    expect(s.blocked).toBe("Download the source first.");
  });

  it("reports a build made from older source as needing attention and offers to build again", () => {
    expect(summarizeBuild(build({ state: "outdated" }))).toMatchObject({ status: "warning", action: "build" });
  });

  it("is ready when current, names the commit, and states that playerbots are included", () => {
    const s = summarizeBuild(build());
    expect(s).toMatchObject({ status: "ready", action: "rebuild" });
    expect(s.detail).toContain("a1b2c3d4e5f6");
    expect(s.detail).toContain("Playerbots compiled in");
  });

  it("never claims playerbots that were not proven", () => {
    expect(summarizeBuild(build({ playerbots: false })).status).toBe("warning");
    expect(summarizeBuild(build({ playerbots: null })).status).toBe("warning");
  });

  it("shows Docker trouble as unknown, with the reason", () => {
    const s = summarizeBuild(build({ state: "unknown", blocked: "Docker is not running. Start Docker Desktop." }));
    expect(s.status).toBe("unknown");
    expect(s.detail).toContain("Start Docker Desktop");
    expect(s.blocked).toContain("Start Docker Desktop");
  });
});

describe("formatBytes", () => {
  it("uses MiB below a gigabyte and GiB above", () => {
    expect(formatBytes(412 * 1024 * 1024)).toBe("412 MiB");
    expect(formatBytes(3 * 1024 * 1024 * 1024)).toBe("3.0 GiB");
  });
});

const goodClient: ClientStatus = {
  path: "C:/Games/WoW",
  problem: null,
  info: { path: "C:/Games/WoW", dataDir: "C:/Games/WoW/Data", build: 5875, version: "1.12.1", locales: ["enUS"], dataBytes: 2 * 1024 ** 3 },
};

const extractStatus = (extra: Partial<ExtractStatus> = {}): ExtractStatus => ({
  state: "notExtracted",
  blocked: null,
  note: null,
  volume: "worldstone-mangos-zero-gamedata",
  record: null,
  resumable: false,
  ...extra,
});

const record = { clientBuild: 5875, clientVersion: "1.12.1", extractorCommit: "abc1234", finishedUnix: 1, seconds: 60, outputs: [{ name: "dbc", files: 3, bytes: 1024 ** 3 }] };

describe("summarizeClient", () => {
  it("is unknown until the backend answers", () => {
    expect(summarizeClient(null).status).toBe("unknown");
  });

  it("asks for a client when none was chosen", () => {
    const s = summarizeClient({ path: null, info: null, problem: null });
    expect(s.status).toBe("missing");
  });

  it("reports the version and size of a usable client", () => {
    const s = summarizeClient(goodClient);
    expect(s.status).toBe("ready");
    expect(s.detail).toContain("1.12.1 (build 5875)");
    expect(s.detail).toContain("2.0 GiB");
  });

  it("shows why a remembered folder no longer works", () => {
    const s = summarizeClient({ path: "D:/gone", info: null, problem: "WoW.exe was not found in D:/gone." });
    expect(s.status).toBe("warning");
    expect(s.detail).toContain("WoW.exe was not found");
  });
});

describe("summarizeExtract", () => {
  it("cannot start until the first answer arrives", () => {
    const s = summarizeExtract(null, goodClient);
    expect(s.status).toBe("unknown");
    expect(s.blocked).not.toBeNull();
  });

  it("offers extraction, and passes on why it cannot start", () => {
    const s = summarizeExtract(extractStatus({ blocked: "Choose your game client first." }), null);
    expect(s.status).toBe("missing");
    expect(s.action).toBe("extract");
    expect(s.blocked).toBe("Choose your game client first.");
  });

  it("offers to resume an interrupted extraction instead of pretending nothing happened", () => {
    const s = summarizeExtract(extractStatus({ resumable: true }), goodClient);
    expect(s.status).toBe("warning");
    expect(s.action).toBe("resume");
    expect(s.blocked).toBeNull();
  });

  it("is ready with the client version and the size on disk", () => {
    const s = summarizeExtract(extractStatus({ state: "ready", record }), goodClient);
    expect(s.status).toBe("ready");
    expect(s.action).toBe("again");
    expect(s.detail).toContain("1.12.1");
    expect(s.detail).toContain("1.0 GiB");
  });

  it("warns when the data came from a different client than the one chosen", () => {
    const other: ClientStatus = { ...goodClient, info: { ...goodClient.info!, build: 6005, version: "1.12.2" } };
    const s = summarizeExtract(extractStatus({ state: "ready", record }), other);
    expect(s.status).toBe("warning");
    expect(s.detail).toContain("6005");
  });

  it("shows Docker trouble as unknown, with the reason", () => {
    const s = summarizeExtract(extractStatus({ state: "unknown", blocked: "Docker is not running." }), goodClient);
    expect(s.status).toBe("unknown");
    expect(s.detail).toBe("Docker is not running.");
  });

  it("surfaces a note about an unreadable record", () => {
    const s = summarizeExtract(extractStatus({ note: "The record of the last extraction is unreadable." }), goodClient);
    expect(s.status).toBe("warning");
    expect(s.detail).toContain("unreadable");
  });
});

const schema = (name: string, extra: Partial<SchemaStatus> = {}): SchemaStatus => ({
  name,
  exists: true,
  tables: 50,
  setupDone: true,
  unmanaged: false,
  populateTotal: 0,
  populatePending: 0,
  version: "22.5.1",
  updatesTotal: 3,
  updatesPending: 0,
  missingTables: [],
  ...extra,
});

const database = (state: DatabaseStatus["state"], extra: Partial<DatabaseStatus> = {}): DatabaseStatus => ({
  state,
  blocked: null,
  note: null,
  container: "worldstone-mangos-zero-db",
  volume: "worldstone-mangos-zero-dbdata",
  image: "mariadb:11.4",
  schemas: [schema("realmd"), schema("mangos0", { version: "22.6.4" })],
  realms: 1,
  ...extra,
});

describe("summarizeDatabase", () => {
  it("says it is checking until the backend answers, and blocks the button meanwhile", () => {
    const s = summarizeDatabase(null);
    expect(s.status).toBe("unknown");
    expect(s.blocked).toBe("Checking…");
  });

  it("shows why Docker could not be asked and does not offer to set up", () => {
    const s = summarizeDatabase(database("unknown", { blocked: "Docker is not running. Start Docker Desktop.", schemas: [], realms: null }));
    expect(s.status).toBe("unknown");
    expect(s.blocked).toBe("Docker is not running. Start Docker Desktop.");
  });

  it("offers to set up a database that does not exist yet, unless the scripts are missing", () => {
    expect(summarizeDatabase(database("notProvisioned", { schemas: [], realms: null }))).toMatchObject({ status: "missing", action: "setup", blocked: null });
    const s = summarizeDatabase(database("notProvisioned", { blocked: "Download the source first." }));
    expect(s.blocked).toBe("Download the source first.");
  });

  it("treats a stopped database as unread, never as healthy", () => {
    const s = summarizeDatabase(database("stopped", { schemas: [], realms: null }));
    expect(s.status).toBe("warning");
    expect(s.action).toBe("start");
  });

  it("names what is missing from an incomplete database", () => {
    const db = database("incomplete", {
      schemas: [schema("realmd"), schema("mangos0", { populatePending: 3, populateTotal: 120 })],
      realms: 0,
    });
    const s = summarizeDatabase(db);
    expect(s.status).toBe("warning");
    expect(s.detail).toBe("mangos0 still has 3 data files to load; no realm is listed for players to join");
    expect(s.action).toBe("finish");
    expect(s.blocked).toBeNull();
  });

  it("refuses to offer a setup that would only fail on tables it did not create", () => {
    const db = database("incomplete", { schemas: [schema("realmd", { unmanaged: true, setupDone: false })] });
    const s = summarizeDatabase(db);
    expect(s.blocked).toContain("realmd already holds tables this app did not create");
  });

  it("counts waiting updates per database and says a backup comes first", () => {
    const s = summarizeDatabase(database("outdated", { schemas: [schema("realmd", { updatesPending: 1 }), schema("mangos0", { updatesPending: 2 })] }));
    expect(s.detail).toBe("3 updates waiting (realmd 1, mangos0 2). A backup is taken first.");
    expect(s.action).toBe("update");
    expect(summarizeDatabase(database("outdated", { schemas: [schema("realmd", { updatesPending: 1 })] })).detail).toContain("1 update waiting");
  });

  it("shows the real version of each database when it is ready", () => {
    const s = summarizeDatabase(database("ready"));
    expect(s).toMatchObject({ status: "ready", action: "check", detail: "realmd 22.5.1 · mangos0 22.6.4 · 1 realm" });
    expect(summarizeDatabase(database("ready", { realms: 2 })).detail).toContain("2 realms");
  });

  it("shows an unknown version as unknown, not as a made-up number", () => {
    expect(summarizeDatabase(database("ready", { schemas: [schema("realmd", { version: null })] })).detail).toContain("realmd ?");
  });
});

describe("describeGaps", () => {
  it("lists missing tables and unset schemas", () => {
    const db = database("incomplete", { schemas: [schema("realmd", { setupDone: false }), schema("mangos0", { missingTables: ["creature_template", "item_template"] })] });
    expect(describeGaps(db)).toBe("realmd is not set up; mangos0 lacks creature_template, item_template");
  });
});

const configFile = (id: string, label: string, state: ConfigStatus["files"][number]["state"] = "ready", staleKeys: string[] = []) => ({
  id,
  label,
  path: `C:\\WorldStone\\Config\\server\\${id}.conf`,
  state,
  staleKeys,
});

const config = (state: ConfigStatus["state"], extra: Partial<ConfigStatus> = {}): ConfigStatus => ({
  state,
  blocked: null,
  note: null,
  files: [configFile("realmd", "Realm server"), configFile("mangosd", "World server"), configFile("aiplayerbot", "Playerbots")],
  bots: { enabled: true, min: 48, max: 96, accounts: 16, playerLimit: 196 },
  ...extra,
});

describe("summarizeConfig", () => {
  it("cannot write anything until the backend answers", () => {
    expect(summarizeConfig(null, build())).toMatchObject({ status: "unknown", blocked: "Checking…" });
  });

  it("offers to generate when nothing exists, and needs a built image for the samples", () => {
    expect(summarizeConfig(config("notConfigured"), build())).toMatchObject({ status: "missing", action: "generate", blocked: null });
    const s = summarizeConfig(config("notConfigured"), build({ state: "notBuilt" }));
    expect(s.blocked).toContain("Build the server first");
    expect(summarizeConfig(config("notConfigured"), null).blocked).toBe("Checking…");
  });

  it("puts the reason the backend gives first, such as a database that is not set up", () => {
    const s = summarizeConfig(config("notConfigured", { blocked: "Set up the database first: its passwords are written into the configuration." }), build());
    expect(s.blocked).toContain("Set up the database first");
  });

  it("names what is out of date without ever showing a value", () => {
    const files = [
      configFile("realmd", "Realm server"),
      configFile("mangosd", "World server", "outdated", ["PlayerLimit", "RealmID"]),
      configFile("aiplayerbot", "Playerbots", "missing"),
    ];
    const c = config("outdated", { files });
    expect(describeConfigGaps(c)).toBe("World server: PlayerLimit, RealmID; Playerbots: not created");
    expect(summarizeConfig(c, build())).toMatchObject({ status: "warning", action: "update" });
  });

  it("is ready with the bot roster, and says when the bots are off", () => {
    expect(summarizeConfig(config("ready"), build())).toMatchObject({ status: "ready", action: "again", detail: "3 files in place · 48–96 bots" });
    const off = config("ready", { bots: { enabled: false, min: 0, max: 0, accounts: 5, playerLimit: 100 } });
    expect(summarizeConfig(off, build()).detail).toContain("bots off");
  });

  it("reports an unreadable configuration as unknown, never as missing", () => {
    const s = summarizeConfig(config("unknown", { note: "settings.json cannot be read" }), build());
    expect(s.status).toBe("unknown");
    expect(s.detail).toContain("cannot be read");
  });
});

const service = (id: string, label: string, port: number, state: ServiceStatus["state"], extra: Partial<ServiceStatus> = {}): ServiceStatus => ({
  id,
  label,
  container: `worldstone-mangos-zero-${id}`,
  port,
  state,
  exitCode: null,
  reachable: state === "running" ? true : null,
  uptimeSecs: state === "running" ? 120 : null,
  ...extra,
});

const server = (state: ServerStatus["state"], services: ServiceStatus[], extra: Partial<ServerStatus> = {}): ServerStatus => ({
  state,
  note: null,
  host: "127.0.0.1",
  services,
  restartNeeded: false,
  heldBy: [],
  ...extra,
});

const pair = (a: ServiceStatus["state"], b: ServiceStatus["state"], extra: Partial<ServiceStatus> = {}) => [
  service("realmd", "Realm server", 3724, a),
  service("mangosd", "World server", 8085, b, extra),
];

const ready = {
  build: build(),
  extract: extractStatus({ state: "ready", record }),
  database: database("ready"),
  config: config("ready"),
};

describe("whatStartNeeds", () => {
  it("is nothing when everything is in place", () => {
    expect(whatStartNeeds(ready)).toBeNull();
  });

  it("waits for every answer, then names the first thing missing in the order it is fixed", () => {
    expect(whatStartNeeds({ ...ready, config: null })).toBe("Checking…");
    expect(whatStartNeeds({ ...ready, build: build({ state: "notBuilt" }) })).toBe("Build the server first.");
    expect(whatStartNeeds({ ...ready, extract: extractStatus() })).toBe("Extract the client data first.");
    expect(whatStartNeeds({ ...ready, database: database("notProvisioned") })).toBe("Set up the database first.");
    expect(whatStartNeeds({ ...ready, database: database("outdated") })).toBe("Update the database first.");
    expect(whatStartNeeds({ ...ready, config: config("notConfigured") })).toBe("Generate the configuration first.");
    expect(whatStartNeeds({ ...ready, config: config("outdated") })).toContain("out of date");
    const many = { build: build({ state: "notBuilt" }), extract: extractStatus(), database: database("notProvisioned"), config: config("notConfigured") };
    expect(whatStartNeeds(many)).toBe("Build the server first.");
  });

  it("does not hold a stopped database against the start, since starting brings it up", () => {
    expect(whatStartNeeds({ ...ready, database: database("stopped", { schemas: [], realms: null }) })).toBeNull();
  });
});

describe("pausing and launching", () => {
  it("can pause only servers that are running, and resume only servers that are paused", () => {
    const up = summarizeServer(server("running", pair("running", "running")), ready);
    expect(up).toMatchObject({ pauseBlocked: null, resumeBlocked: "The servers are not paused." });
    const frozen = summarizeServer(server("paused", pair("paused", "paused")), ready);
    expect(frozen).toMatchObject({ status: "warning", label: "Paused", pauseBlocked: "The servers are already paused.", resumeBlocked: null, stopBlocked: null });
    expect(frozen.startBlocked).toContain("Resume");
    const down = summarizeServer(server("stopped", pair("missing", "missing")), ready);
    expect(down).toMatchObject({ pauseBlocked: "The servers are not running.", resumeBlocked: "The servers are not paused." });
    expect(summarizeServer(null, ready)).toMatchObject({ pauseBlocked: "Checking…", resumeBlocked: "Checking…" });
  });

  it("offers resume when only one server is frozen", () => {
    const mixed = summarizeServer(server("partial", pair("running", "paused")), ready);
    expect(mixed.resumeBlocked).toBeNull();
    expect(mixed.detail).toBe("Realm server running · World server paused");
  });

  it("launches the game only with a usable client and servers that answer", () => {
    const client: ClientStatus = { path: "C:/WoW", problem: null, info: { path: "C:/WoW", dataDir: "C:/WoW/Data", build: 5875, version: "1.12.1", locales: [], dataBytes: 1 } };
    const running = server("running", pair("running", "running"));
    expect(launchBlocked(client, running)).toBeNull();
    expect(launchBlocked(null, running)).toBe("Checking…");
    expect(launchBlocked(client, null)).toBe("Checking…");
    expect(launchBlocked({ path: null, info: null, problem: null }, running)).toBe("Choose your game client first.");
    expect(launchBlocked({ path: "D:/gone", info: null, problem: "The folder no longer exists." }, running)).toBe("The folder no longer exists.");
    expect(launchBlocked(client, server("stopped", pair("missing", "missing")))).toBe("Start the servers first.");
    expect(launchBlocked(client, server("paused", pair("paused", "paused")))).toContain("Resume");
    expect(launchBlocked(client, server("starting", pair("running", "starting")))).toContain("still starting");
  });
});

describe("summarizeServer", () => {
  it("cannot be started, stopped or restarted before the first answer", () => {
    expect(summarizeServer(null, ready)).toMatchObject({ status: "unknown", startBlocked: "Checking…", stopBlocked: "Checking…", restartBlocked: "Checking…" });
  });

  it("is stopped and startable once everything is in place, with nothing to stop", () => {
    const s = summarizeServer(server("stopped", pair("missing", "missing")), { ...ready, database: database("stopped", { schemas: [], realms: null }) });
    expect(s).toMatchObject({ label: "Stopped", startBlocked: null, stopBlocked: "Nothing is running." });
    expect(s.restartBlocked).toContain("Use Start");
  });

  it("does not offer a start while another instance's servers hold the same ports, and says which", () => {
    const stopped = { ...ready, database: database("stopped", { schemas: [], realms: null }) };
    const s = summarizeServer(server("stopped", pair("missing", "missing"), { heldBy: ["mangos-zero"] }), stopped);
    expect(s.startBlocked).toBe("The mangos-zero server is running and uses the same ports. Stop it first.");
    expect(s.detail).toBe(s.startBlocked);
    const two = summarizeServer(server("stopped", pair("missing", "missing"), { heldBy: ["a", "b"] }), stopped);
    expect(two.startBlocked).toBe("The a and b servers are running and use the same ports. Stop them first.");
    const unbuilt = summarizeServer(server("stopped", pair("missing", "missing"), { heldBy: ["a"] }), { ...stopped, config: config("notConfigured") });
    expect(unbuilt.startBlocked).toBe("Generate the configuration first.");
  });

  it("still lets the database be stopped when only it is left running", () => {
    expect(summarizeServer(server("stopped", pair("stopped", "stopped")), ready).stopBlocked).toBeNull();
  });

  it("says what is missing instead of offering a start that would fail", () => {
    const s = summarizeServer(server("stopped", pair("missing", "missing")), { ...ready, config: config("notConfigured") });
    expect(s.startBlocked).toBe("Generate the configuration first.");
  });

  it("shows where the servers listen only when they are running, and offers stop and restart", () => {
    const s = summarizeServer(server("running", pair("running", "running")), ready);
    expect(s).toMatchObject({ status: "ready", label: "Running", startBlocked: "The servers are already running.", stopBlocked: null, restartBlocked: null });
    expect(s.detail).toBe("Realm server on 127.0.0.1:3724 · World server on 127.0.0.1:8085");
  });

  it("asks for a restart when the configuration changed after the servers started", () => {
    const s = summarizeServer(server("running", pair("running", "running"), { restartNeeded: true }), ready);
    expect(s).toMatchObject({ status: "warning", label: "Running", detail: RESTART_HINT, restartBlocked: null });
    // A problem with the ports is the more urgent thing to say.
    const both = summarizeServer(server("running", pair("running", "running"), { restartNeeded: true, note: "World server answers inside Docker but not on 127.0.0.1." }), ready);
    expect(both.detail).toContain("not on 127.0.0.1");
  });

  it("does not call a server that answers only inside Docker healthy", () => {
    const s = summarizeServer(server("running", pair("running", "running"), { note: "World server answers inside Docker but not on 127.0.0.1." }), ready);
    expect(s.status).toBe("warning");
    expect(s.detail).toContain("not on 127.0.0.1");
  });

  it("shows a server that is still loading as starting, with what each one is doing", () => {
    const s = summarizeServer(server("starting", pair("running", "starting")), ready);
    expect(s).toMatchObject({ status: "warning", label: "Starting", stopBlocked: null });
    expect(s.detail).toBe("Realm server running · World server starting");
  });

  it("shows a crash as an error with its exit code, and offers a restart", () => {
    const s = summarizeServer(server("failing", pair("running", "crashed", { exitCode: 139 })), ready);
    expect(s).toMatchObject({ status: "error", label: "Failing", restartBlocked: null });
    expect(s.detail).toBe("Realm server running · World server crashed (exit 139)");
  });

  it("shows one server up and one down as partly running", () => {
    expect(summarizeServer(server("partial", pair("running", "stopped")), ready)).toMatchObject({ status: "warning", label: "Partly running" });
  });

  it("passes on why Docker could not be asked", () => {
    const s = summarizeServer(server("unknown", [], { note: "Docker is not running. Start Docker Desktop." }), ready);
    expect(s).toMatchObject({ status: "unknown", startBlocked: "Docker is not running. Start Docker Desktop." });
  });
});

const botsStatus = (state: BotsStatus["state"], extra: Partial<BotsStatus> = {}): BotsStatus => ({
  state,
  note: "note",
  enabled: true,
  min: 48,
  max: 96,
  playerLimit: 196,
  botAccounts: 16,
  characters: 144,
  online: 77,
  players: 0,
  restartNeeded: false,
  ...extra,
});

describe("summarizeBots", () => {
  it("waits for the first answer", () => {
    expect(summarizeBots(null)).toMatchObject({ status: "unknown", label: "Checking" });
  });

  it("is ready only when bots are counted in the world, and says how many", () => {
    const s = summarizeBots(botsStatus("online"));
    expect(s).toMatchObject({ status: "ready", label: "Online" });
    expect(s.detail).toBe("77 of up to 96 bots in the world.");
    expect(summarizeBots(botsStatus("online", { players: 2 })).detail).toContain("2 players besides");
    expect(summarizeBots(botsStatus("online", { players: 1 })).detail).toContain("1 player besides");
  });

  it("is not ready while the bots are still being made or logging in", () => {
    for (const state of ["starting", "creating", "loggingIn"] as const) {
      expect(summarizeBots(botsStatus(state)).status, state).toBe("warning");
    }
    expect(summarizeBots(botsStatus("creating", { note: "Creating the bots' characters: 45 of 144." })).detail).toContain("45 of 144");
  });

  it("shows a stalled world as an error with the reason the backend found", () => {
    expect(summarizeBots(botsStatus("stalled", { note: "Only 0 of at least 48 bots are online." }))).toMatchObject({ status: "error", label: "Stalled", detail: "Only 0 of at least 48 bots are online." });
  });

  it("never turns an unknown into a number", () => {
    expect(summarizeBots(botsStatus("unknown", { note: "The database is not answering." }))).toMatchObject({ status: "unknown", detail: "The database is not answering." });
  });

  it("keeps stopped and off apart, and mentions a restart that would change things", () => {
    expect(summarizeBots(botsStatus("stopped"))).toMatchObject({ status: "missing", label: "Stopped" });
    expect(summarizeBots(botsStatus("off"))).toMatchObject({ status: "warning", label: "Off" });
    expect(summarizeBots(botsStatus("off", { restartNeeded: true })).detail).toContain("Restart the servers");
    expect(summarizeBots(botsStatus("online", { restartNeeded: true })).detail).toContain("Restart the servers");
  });
});

describe("botChoiceProblem", () => {
  it("accepts a sensible range and explains the rest", () => {
    expect(botChoiceProblem({ enabled: true, min: 48, max: 96 })).toBeNull();
    expect(botChoiceProblem({ enabled: false, min: 0, max: 0 })).toBeNull();
    expect(botChoiceProblem({ enabled: true, min: 50, max: 40 })).toContain("minimum");
    expect(botChoiceProblem({ enabled: true, min: 0, max: 1001 })).toContain("1000");
    expect(botChoiceProblem({ enabled: true, min: Number.NaN, max: 10 })).toContain("whole numbers");
    expect(botChoiceProblem({ enabled: true, min: 1.5, max: 10 })).toContain("whole numbers");
    expect(botChoiceProblem({ enabled: true, min: -1, max: 10 })).toContain("whole numbers");
  });
});

const connectStatus = (state: ConnectStatus["state"], extra: Partial<ConnectStatus> = {}): ConnectStatus => ({
  state,
  note: null,
  client: "C:/Games/WoW",
  file: "C:/Games/WoW/realmlist.wtf",
  address: "logon.example.com",
  wanted: "127.0.0.1",
  backup: null,
  backupAddress: null,
  ...extra,
});

describe("summarizeConnect", () => {
  it("waits for the first answer, and needs a client first", () => {
    expect(summarizeConnect(null)).toMatchObject({ status: "unknown", pointBlocked: "Checking…", restoreBlocked: "Checking…" });
    expect(summarizeConnect(connectStatus("noClient"))).toMatchObject({ status: "missing", pointBlocked: "Choose your game client first." });
  });

  it("offers to point a client that logs in elsewhere, and has nothing to undo yet", () => {
    const s = summarizeConnect(connectStatus("elsewhere"));
    expect(s).toMatchObject({ status: "warning", pointBlocked: null });
    expect(s.detail).toContain("logon.example.com");
    expect(s.restoreBlocked).toContain("nothing to undo");
  });

  it("says when the file names no address", () => {
    expect(summarizeConnect(connectStatus("elsewhere", { address: null })).detail).toContain("names no address");
  });

  it("is ready once the client logs in here, and restores only a different saved address", () => {
    const pointed = connectStatus("pointed", { address: "127.0.0.1" });
    expect(summarizeConnect(pointed)).toMatchObject({ status: "ready", pointBlocked: "The client already logs in to 127.0.0.1." });
    expect(summarizeConnect(pointed).restoreBlocked).toContain("no saved address");
    const saved = { ...pointed, backup: "realmlist.wtf.worldstone-backup", backupAddress: "logon.example.com" };
    expect(summarizeConnect(saved).restoreBlocked).toBeNull();
    expect(summarizeConnect({ ...saved, backupAddress: "127.0.0.1" }).restoreBlocked).toContain("same address");
  });

  it("lets a client without the file be pointed, which creates it", () => {
    expect(summarizeConnect(connectStatus("noFile", { address: null }))).toMatchObject({ status: "warning", pointBlocked: null });
  });

  it("passes on why the file cannot be read", () => {
    expect(summarizeConnect(connectStatus("unknown", { note: "realmlist.wtf is not plain text" }))).toMatchObject({ status: "unknown", pointBlocked: "realmlist.wtf is not plain text" });
  });
});
