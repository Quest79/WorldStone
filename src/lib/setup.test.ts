import { describe, expect, it } from "vitest";
import type { BuildStatus, ClientStatus, ConfigStatus, DatabaseStatus, ExtractStatus, RepoStatus } from "../api/types";
import { planSetup, type SetupState } from "./setup";

const repo = (id: string, state: RepoStatus["state"]): RepoStatus => ({ id, url: "https://example.com/x.git", branch: "master", state, commit: null, summary: null, dirty: false, detail: "" });
const build = (extra: Partial<BuildStatus> = {}): BuildStatus => ({ state: "ready", blocked: null, image: "worldstone/vanilla-world:latest", builtAt: null, sizeBytes: null, commit: "a1b2c3d", playerbots: true, ...extra });
const goodClient: ClientStatus = { path: "C:/WoW", problem: null, info: { path: "C:/WoW", dataDir: "C:/WoW/Data", build: 5875, version: "1.12.1", locales: ["enUS"], dataBytes: 1024 ** 3 } };
const extract = (extra: Partial<ExtractStatus> = {}): ExtractStatus => ({
  state: "ready",
  blocked: null,
  note: null,
  volume: "v",
  record: { clientBuild: 5875, clientVersion: "1.12.1", extractorCommit: "abc", finishedUnix: 1, seconds: 1, outputs: [{ name: "dbc", files: 1, bytes: 1 }] },
  resumable: false,
  ...extra,
});
const database = (state: DatabaseStatus["state"], extra: Partial<DatabaseStatus> = {}): DatabaseStatus => ({
  state,
  blocked: null,
  note: null,
  container: "c",
  volume: "v",
  image: "worldstone/vanilla-database:11.4",
  schemas: [{ name: "realmd", exists: true, tables: 5, setupDone: true, unmanaged: false, populateTotal: 0, populatePending: 0, version: "1", updatesTotal: 1, updatesPending: 0, missingTables: [] }],
  realms: 1,
  ...extra,
});
const config = (state: ConfigStatus["state"], extra: Partial<ConfigStatus> = {}): ConfigStatus => ({
  state,
  blocked: null,
  note: null,
  files: [{ id: "mangosd", label: "World server", path: "p", state: "ready", staleKeys: [] }],
  bots: { enabled: true, min: 1, max: 2, accounts: 1, playerLimit: 10 },
  ...extra,
});

const done = (): SetupState => ({
  environment: [
    { id: "git", label: "Git", status: "ready", detail: "" },
    { id: "docker", label: "Docker", status: "ready", detail: "" },
  ],
  repos: [repo("server", "ready"), repo("database", "ready")],
  build: build(),
  client: goodClient,
  extract: extract(),
  database: database("ready"),
  config: config("ready"),
});

describe("planSetup", () => {
  it("has nothing to do when every step is in place", () => {
    expect(planSetup(done())).toMatchObject({ next: null, blocked: null, detail: "Everything is set up" });
  });

  it("waits for the first answers instead of guessing a step", () => {
    expect(planSetup({ ...done(), environment: null })).toMatchObject({ next: null, blocked: "Checking…" });
    expect(planSetup({ ...done(), repos: null }).next).toBeNull();
    expect(planSetup({ ...done(), config: null }).next).toBeNull();
  });

  it("does the steps in the order they depend on each other", () => {
    const fresh: SetupState = {
      ...done(),
      repos: [repo("server", "missing"), repo("database", "missing")],
      build: build({ state: "notBuilt", commit: null, playerbots: null }),
      client: { path: null, info: null, problem: null },
      extract: extract({ state: "notExtracted", record: null }),
      database: database("notProvisioned", { schemas: [], realms: null }),
      config: config("notConfigured", { files: [] }),
    };
    expect(planSetup(fresh).next).toBe("source");
    const withSource = { ...fresh, repos: done().repos };
    expect(planSetup(withSource).next).toBe("build");
    const withBuild = { ...withSource, build: build() };
    expect(planSetup(withBuild).next).toBe("client");
    const withClient = { ...withBuild, client: goodClient };
    expect(planSetup(withClient).next).toBe("extract");
    const withData = { ...withClient, extract: extract() };
    expect(planSetup(withData).next).toBe("database");
    const withDatabase = { ...withData, database: database("ready") };
    expect(planSetup(withDatabase).next).toBe("configure");
  });

  it("resumes an interrupted extraction and finishes a half-made database rather than starting over", () => {
    expect(planSetup({ ...done(), extract: extract({ state: "notExtracted", record: null, resumable: true }) }).next).toBe("extract");
    expect(planSetup({ ...done(), database: database("stopped") }).next).toBe("database");
  });

  it("rebuilds without the cache only when the image is not proven to hold the bots", () => {
    expect(planSetup({ ...done(), build: build({ state: "outdated" }) })).toMatchObject({ next: "build", force: false });
    expect(planSetup({ ...done(), build: build({ playerbots: false }) })).toMatchObject({ next: "build", force: true });
  });

  it("stops with the reason when a step cannot run, and when Docker cannot be asked", () => {
    const blocked = planSetup({ ...done(), extract: extract({ state: "notExtracted", record: null, blocked: "Docker is not running." }) });
    expect(blocked).toMatchObject({ next: null, blocked: "Docker is not running." });
    const noDocker = planSetup({ ...done(), environment: [{ id: "docker-engine", label: "Docker", status: "missing", detail: "Start Docker Desktop." }] });
    expect(noDocker).toMatchObject({ next: null, blocked: "Docker: Start Docker Desktop." });
    const unknown = planSetup({ ...done(), build: build({ state: "unknown", blocked: "Docker could not be asked." }) });
    expect(unknown).toMatchObject({ next: null, blocked: "Docker could not be asked." });
  });

  it("refuses to touch a database that holds tables this app did not create", () => {
    const foreign = database("incomplete", { schemas: [{ ...database("ready").schemas[0], unmanaged: true }] });
    expect(planSetup({ ...done(), database: foreign }).blocked).toContain("did not create");
  });
});
