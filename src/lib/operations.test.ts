import { describe, expect, it } from "vitest";
import type {
  BackupSet,
  BackupsStatus,
  BuildStatus,
  DashboardRow,
  DatabaseStatus,
  RepairFinding,
  RepoStatus,
  ResetItem,
  ResetPreview,
} from "../api/types";
import {
  backupBlocked,
  databasesIn,
  describeRow,
  describeSet,
  repairLabel,
  resetBlocked,
  resetConfirmed,
  resetTargets,
  restoreBlocked,
  restoreWarning,
  summarizeRepair,
  updateBlocked,
} from "./operations";
import { CHECKING } from "./summary";

const set = (extra: Partial<BackupSet> = {}): BackupSet => ({
  id: "20260929-101500-manual",
  taken: new Date(2026, 8, 29, 10, 15, 0).getTime() / 1000,
  label: "manual",
  what: "Made with Back up now",
  files: [
    { schema: "character0", file: "character0-20260929-101500-manual.sql.gz", bytes: 3_000_000 },
    { schema: "realmd", file: "realmd-20260929-101500-manual.sql.gz", bytes: 1_200_000 },
  ],
  bytes: 4_200_000,
  complete: true,
  ...extra,
});

const backups = (extra: Partial<BackupsStatus> = {}): BackupsStatus => ({
  folder: "C:/x/Backups",
  sets: [set()],
  note: null,
  backupBlocked: null,
  restoreBlocked: null,
  ...extra,
});

describe("backups", () => {
  it("wait for the backend, then pass on its reasons", () => {
    expect(backupBlocked(null)).toBe(CHECKING);
    expect(restoreBlocked(null)).toBe(CHECKING);
    expect(backupBlocked(backups())).toBeNull();
    expect(restoreBlocked(backups({ restoreBlocked: "Stop the servers first." }))).toBe("Stop the servers first.");
  });

  it("describe a set by its databases and size, and say when it is not whole", () => {
    expect(describeSet(set())).toBe("2 databases · 4.0 MiB");
    expect(describeSet(set({ files: [set().files[0]], bytes: 3_000_000, complete: false }))).toBe("1 database · 2.9 MiB · not every database");
    expect(databasesIn(set())).toEqual(["character0", "realmd"]);
  });

  it("warn about exactly which databases a restore replaces and that it can be undone", () => {
    const text = restoreWarning(set());
    expect(text).toContain("replaces character0, realmd");
    expect(text).toContain("2026-09-29 10:15");
    expect(text).toContain("can be undone");
  });
});

const repo = (state: RepoStatus["state"]): RepoStatus => ({ id: "server", url: "", branch: "master", state, commit: "abc", summary: null, dirty: false, detail: "" });
const build = (state: BuildStatus["state"], blocked: string | null = null): BuildStatus =>
  ({ state, blocked, commit: null, playerbots: true }) as unknown as BuildStatus;
const database = (state: DatabaseStatus["state"], note: string | null = null): DatabaseStatus => ({ state, note }) as unknown as DatabaseStatus;

describe("updateBlocked", () => {
  const ready = { repos: [repo("ready")], build: build("ready"), database: database("ready") };

  it("waits for every status", () => {
    expect(updateBlocked({ ...ready, repos: null })).toBe(CHECKING);
    expect(updateBlocked({ ...ready, build: null })).toBe(CHECKING);
    expect(updateBlocked({ ...ready, database: null })).toBe(CHECKING);
  });

  it("is for a server that is already set up", () => {
    expect(updateBlocked(ready)).toBeNull();
    expect(updateBlocked({ ...ready, repos: [repo("ready"), repo("missing")] })).toContain("Set up first");
    expect(updateBlocked({ ...ready, build: build("notBuilt") })).toContain("not been built");
    expect(updateBlocked({ ...ready, database: database("notProvisioned") })).toContain("no database");
  });

  it("can go ahead when the build is old, the database has updates waiting or it is stopped", () => {
    expect(updateBlocked({ ...ready, build: build("outdated") })).toBeNull();
    expect(updateBlocked({ ...ready, database: database("outdated") })).toBeNull();
    expect(updateBlocked({ ...ready, database: database("stopped") })).toBeNull();
  });

  it("passes on why Docker or the database cannot be asked", () => {
    expect(updateBlocked({ ...ready, build: build("unknown", "Docker is not running.") })).toBe("Docker is not running.");
    expect(updateBlocked({ ...ready, database: database("unknown", "No answer.") })).toBe("No answer.");
  });
});

const finding = (extra: Partial<RepairFinding> = {}): RepairFinding => ({ title: "A problem", detail: "", fix: "build", action: "Build the server", ...extra });

describe("repair", () => {
  it("has nothing to do when nothing is wrong", () => {
    const summary = summarizeRepair({ findings: [], canRepair: false });
    expect(summary.status).toBe("ready");
    expect(summary.blocked).toBe("Nothing needs repairing.");
    expect(summarizeRepair(null).blocked).toBe(CHECKING);
  });

  it("counts the problems and offers the repair when the app can do any of it", () => {
    const summary = summarizeRepair({ findings: [finding(), finding({ fix: null, action: null })], canRepair: true });
    expect(summary).toMatchObject({ status: "warning", headline: "2 problems found", blocked: null, manual: 1 });
    expect(repairLabel(summary, 2)).toBe("Repair 2 problems");
    expect(repairLabel(summarizeRepair({ findings: [finding()], canRepair: true }), 1)).toBe("Repair 1 problem");
  });

  it("says so when only the person can put it right", () => {
    const summary = summarizeRepair({ findings: [finding({ fix: null, action: null })], canRepair: false });
    expect(summary.blocked).toContain("cannot repair this by itself");
    expect(repairLabel(summary, 1)).toBe("Repair");
  });
});

const item = (id: string, exists: boolean, label = id): ResetItem => ({ id, label, detail: "", exists, bytes: null });
const preview = (extra: Partial<ResetPreview> = {}): ResetPreview => ({
  phrase: "mangos-zero",
  items: [item("database", true, "Database"), item("credentials", true, "Database credentials"), item("logs", false, "Server logs"), item("gameData", true, "Extracted game data")],
  kept: [],
  blocked: null,
  ...extra,
});

describe("reset", () => {
  it("is confirmed only by the instance id, as the backend also insists", () => {
    expect(resetConfirmed("mangos-zero", "mangos-zero")).toBe(true);
    expect(resetConfirmed("mangos-zero", "  mangos-zero ")).toBe(true);
    for (const wrong of ["", "mangos", "MANGOS-ZERO", "reset"]) expect(resetConfirmed("mangos-zero", wrong)).toBe(false);
    expect(resetConfirmed("", "")).toBe(false);
  });

  it("lists what exists, and the game data only when it is ticked", () => {
    expect(resetTargets(preview(), false)).toEqual(["Database", "Database credentials"]);
    expect(resetTargets(preview(), true)).toEqual(["Database", "Database credentials", "Extracted game data"]);
    expect(resetTargets(null, true)).toEqual([]);
  });

  it("stays disabled until it is safe and confirmed, and says which", () => {
    expect(resetBlocked(null, false, "")).toBe(CHECKING);
    expect(resetBlocked(preview({ blocked: "Docker is not running." }), false, "mangos-zero")).toBe("Docker is not running.");
    expect(resetBlocked(preview({ items: [item("gameData", true)] }), false, "mangos-zero")).toBe("There is nothing to reset yet.");
    expect(resetBlocked(preview(), false, "")).toBe("Type mangos-zero to confirm.");
    expect(resetBlocked(preview(), false, "mangos-zero")).toBeNull();
  });
});

const row = (extra: Partial<DashboardRow> = {}): DashboardRow => ({
  id: "world",
  label: "World server",
  container: "worldstone-mangos-zero-mangosd",
  state: "running",
  port: 8085,
  published: true,
  uptimeSecs: 3 * 3600 + 300,
  cpuPercent: 3.25,
  memoryBytes: 536_870_912,
  memoryLimitBytes: 8_160_437_862,
  ...extra,
});

describe("describeRow", () => {
  it("writes a running server's figures", () => {
    expect(describeRow(row(), "127.0.0.1")).toEqual({
      id: "world",
      label: "World server",
      container: "worldstone-mangos-zero-mangosd",
      state: "Running",
      tone: "ready",
      port: "127.0.0.1:8085",
      uptime: "3 h 05 min",
      cpu: "3.3%",
      memory: "512 MiB of 7.6 GiB",
    });
  });

  it("shows what Docker did not give as dashes, never zeros", () => {
    const line = describeRow(row({ state: "stopped", uptimeSecs: null, cpuPercent: null, memoryBytes: null, memoryLimitBytes: null }), "127.0.0.1");
    expect(line).toMatchObject({ state: "Stopped", tone: "missing", uptime: "—", cpu: "—", memory: "—" });
  });

  it("says the database is not on the host's network", () => {
    expect(describeRow(row({ published: false, port: 3306 }), "127.0.0.1").port).toBe("3306 (only the servers)");
  });

  it("marks a crashed or unresponsive server as an error", () => {
    expect(describeRow(row({ state: "crashed" }), "h").tone).toBe("error");
    expect(describeRow(row({ state: "unresponsive" }), "h")).toMatchObject({ tone: "error", state: "Not answering" });
  });
});
