import { describe, expect, it } from "vitest";
import type { BackupSet, BackupsStatus, BotsStatus, Check, DatabaseStatus, ServerStatus, ServiceStatus, TaskEvent } from "../api/types";
import { formatAge } from "./format";
import { barAddress, barBackup, barBots, barDocker, barOperation, barServer, restartNeeded, STALE_BACKUP_SECS, type OperationState } from "./statusbar";

const service = (over: Partial<ServiceStatus> = {}): ServiceStatus => ({
  id: "realmd",
  label: "Realm server",
  container: "worldstone-mangos-zero-realmd",
  port: 3724,
  state: "running",
  exitCode: null,
  reachable: true,
  uptimeSecs: 11_100,
  ...over,
});

const server = (over: Partial<ServerStatus> = {}): ServerStatus => ({
  state: "running",
  note: null,
  host: "127.0.0.1",
  services: [service(), service({ id: "mangosd", label: "World server", port: 8085, uptimeSecs: 11_040 })],
  restartNeeded: false,
  heldBy: [],
  ...over,
});

const bots = (over: Partial<BotsStatus> = {}): BotsStatus => ({
  state: "online",
  note: "84 bots are in the world.",
  enabled: true,
  min: 50,
  max: 100,
  playerLimit: 1000,
  botAccounts: 100,
  characters: 1000,
  online: 84,
  players: 0,
  restartNeeded: false,
  ...over,
});

const task = (over: Partial<TaskEvent> = {}): TaskEvent => ({
  id: "t1",
  label: "Build server",
  state: "running",
  step: null,
  progress: null,
  message: null,
  cancellable: true,
  ...over,
});

const quiet: OperationState = { task: null, starting: false, settingUp: false, setupNote: null, choosingClient: false, restartNeeded: false };

describe("barServer", () => {
  it("says running with the run's uptime, taken from the newest start", () => {
    const item = barServer(server());
    expect(item.tone).toBe("ok");
    expect(item.text).toBe("Running");
    expect(item.extra).toBe("3 h 04 min");
  });

  it("shows no uptime when none was read, rather than zero", () => {
    const none = server({ services: [service({ uptimeSecs: null })] });
    expect(barServer(none).extra).toBeNull();
  });

  it("does not colour a stopped server as a fault", () => {
    expect(barServer(server({ state: "stopped" }))).toMatchObject({ tone: "idle", text: "Stopped" });
  });

  it("warns for a note, pause, start and partial run, and names what is not running", () => {
    expect(barServer(server({ note: "The realm server does not answer." })).tone).toBe("warn");
    expect(barServer(server({ state: "paused" })).text).toBe("Paused");
    expect(barServer(server({ state: "starting" })).text).toBe("Starting");
    const partial = barServer(server({ state: "partial", services: [service(), service({ id: "mangosd", label: "World server", state: "stopped" })] }));
    expect(partial).toMatchObject({ tone: "warn", text: "Partly running", extra: "World server stopped" });
  });

  it("calls a crashing server failing and gives the exit code", () => {
    const item = barServer(server({ state: "failing", services: [service({ state: "crashed", exitCode: 139 })] }));
    expect(item).toMatchObject({ tone: "bad", text: "Failing", extra: "Realm server crashed (exit 139)" });
  });

  it("shows unknown as unknown, and waiting as waiting", () => {
    expect(barServer(server({ state: "unknown", note: "Docker is not running." }))).toMatchObject({ tone: "idle", text: "Server state unknown", title: "Docker is not running." });
    expect(barServer(null).text).toBe("Checking…");
  });
});

describe("barBots", () => {
  it("counts the bots and the real players", () => {
    expect(barBots(bots())).toMatchObject({ tone: "ok", text: "84 bots · 0 players" });
    expect(barBots(bots({ online: 1, players: 1 }))?.text).toBe("1 bot · 1 player");
  });

  it("leaves out the players when they could not be counted", () => {
    expect(barBots(bots({ players: null }))?.text).toBe("84 bots");
  });

  it("says nothing while the servers are stopped or before the first answer", () => {
    expect(barBots(bots({ state: "stopped" }))).toBeNull();
    expect(barBots(null)).toBeNull();
  });

  it("follows the bots coming online, and flags off and stalled", () => {
    expect(barBots(bots({ state: "loggingIn", online: 30 }))).toMatchObject({ tone: "warn", text: "Bots logging in · 30 of 100" });
    expect(barBots(bots({ state: "creating" }))?.text).toBe("Creating bots");
    expect(barBots(bots({ state: "off" }))?.tone).toBe("warn");
    expect(barBots(bots({ state: "stalled" }))?.tone).toBe("bad");
    expect(barBots(bots({ state: "unknown" }))).toMatchObject({ tone: "idle", text: "Bots unknown" });
  });

  it("carries the saved-settings hint into the hover text", () => {
    expect(barBots(bots({ restartNeeded: true }))?.title).toContain("Restart the servers");
  });
});

describe("restartNeeded", () => {
  it("is true when either the configuration or the bot settings are newer than the run", () => {
    expect(restartNeeded(server(), bots())).toBe(false);
    expect(restartNeeded(server({ restartNeeded: true }), bots())).toBe(true);
    expect(restartNeeded(server(), bots({ restartNeeded: true }))).toBe(true);
    expect(restartNeeded(null, null)).toBe(false);
  });
});

describe("barOperation", () => {
  it("shows the running task with its step, real percentage and Cancel", () => {
    const op = barOperation({ ...quiet, task: task({ step: "Compiling", progress: { label: "Build", percent: 42 } }) });
    expect(op).toMatchObject({ busy: true, percent: 42, cancellable: true, text: "Build server · Compiling" });
  });

  it("has no percentage when the task reports none, and no Cancel when it cannot be cancelled", () => {
    const op = barOperation({ ...quiet, task: task({ cancellable: false }) });
    expect(op).toMatchObject({ busy: true, percent: null, cancellable: false });
  });

  it("leads with the step of Set up while it is inside one", () => {
    const op = barOperation({ ...quiet, settingUp: true, setupNote: "Set up, step 3 of 5: Build server", task: task({ step: "Compiling" }) });
    expect(op.text).toBe("Set up, step 3 of 5: Build server · Compiling");
  });

  it("stays busy between the steps of Set up and right after a click", () => {
    expect(barOperation({ ...quiet, settingUp: true, setupNote: "Set up, step 4 of 5: Extract" })).toMatchObject({ busy: true, text: "Set up, step 4 of 5: Extract" });
    expect(barOperation({ ...quiet, starting: true })).toMatchObject({ busy: true, text: "Starting…" });
  });

  it("says when it is waiting for the person, without a progress bar", () => {
    expect(barOperation({ ...quiet, choosingClient: true })).toMatchObject({ busy: false, text: "Waiting for you to choose the game client" });
  });

  it("is idle with nothing to report", () => {
    expect(barOperation(quiet)).toMatchObject({ busy: false, tone: "idle", text: "Idle" });
    expect(barOperation({ ...quiet, task: task({ state: "succeeded", message: "Backed up." }) })).toMatchObject({ text: "Idle", title: "Last operation finished: Build server. Backed up." });
  });

  it("puts a failure first, then a needed restart, then a cancel", () => {
    const failed = barOperation({ ...quiet, restartNeeded: true, task: task({ state: "failed", message: "Docker said no." }) });
    expect(failed).toMatchObject({ tone: "bad", text: "Build server failed", title: "Docker said no." });
    expect(barOperation({ ...quiet, restartNeeded: true, task: task({ state: "cancelled" }) })).toMatchObject({ tone: "warn" });
    expect(barOperation({ ...quiet, task: task({ state: "cancelled" }) })).toMatchObject({ tone: "idle", text: "Build server cancelled" });
  });

  it("gives a failure without a message a pointer to the log", () => {
    expect(barOperation({ ...quiet, task: task({ state: "failed" }) }).title).toContain("log");
  });
});

const check = (id: string, status: Check["status"], detail = ""): Check => ({ id, label: id, status, detail });
const allReady = [check("git", "ready"), check("docker", "ready"), check("docker-engine", "ready"), check("docker-compose", "ready")];

describe("barDocker", () => {
  it("is ready when Git and Docker are", () => {
    expect(barDocker(allReady)).toMatchObject({ tone: "ok", text: "Docker ready" });
  });

  it("names what is wrong, worst first", () => {
    const stopped = allReady.map((c) => (c.id === "docker-engine" ? check("docker-engine", "missing", "Start Docker Desktop.") : c));
    expect(barDocker(stopped)).toMatchObject({ tone: "bad", text: "Docker not running", title: "docker-engine: Start Docker Desktop." });
    const noGit = allReady.map((c) => (c.id === "git" ? check("git", "missing", "Install Git.") : c));
    expect(barDocker(noGit).text).toBe("Git missing");
    const windows = allReady.map((c) => (c.id === "docker-engine" ? check("docker-engine", "warning") : c));
    expect(barDocker(windows)).toMatchObject({ tone: "warn", text: "Docker not on Linux containers" });
  });

  it("does not claim readiness before the first answer", () => {
    expect(barDocker(null).text).toBe("Checking Docker…");
    expect(barDocker(allReady.map((c) => check(c.id, "unknown"))).tone).toBe("idle");
  });
});

const NOW = 1_800_000_000;
const set = (over: Partial<BackupSet> = {}): BackupSet => ({
  id: "20260929-101500-manual",
  taken: NOW - 7200,
  label: "manual",
  what: "",
  files: [
    { schema: "mangos0", file: "a", bytes: 1 },
    { schema: "characters0", file: "b", bytes: 1 },
  ],
  bytes: 2,
  complete: true,
  ...over,
});
const backups = (sets: BackupSet[], note: string | null = null): BackupsStatus => ({ folder: "x", sets, note, backupBlocked: null, restoreBlocked: null });
const database = (state: DatabaseStatus["state"] = "ready"): DatabaseStatus => ({
  state,
  blocked: null,
  note: null,
  container: "c",
  volume: "v",
  image: "i",
  schemas: [],
  realms: 1,
});

describe("barBackup", () => {
  it("says how long ago the newest complete backup was made", () => {
    expect(barBackup(backups([set()]), database(), NOW)).toMatchObject({ tone: "idle", text: "Backed up 2 h ago" });
  });

  it("skips a backup that lacks a database when it looks for the newest one", () => {
    const partial = set({ taken: NOW - 60, complete: false });
    expect(barBackup(backups([partial, set()]), database(), NOW)?.text).toBe("Backed up 2 h ago");
    expect(barBackup(backups([partial]), database(), NOW)).toMatchObject({ tone: "warn", text: "No complete backup" });
  });

  it("warns when there is none, and when the newest is over a week old", () => {
    expect(barBackup(backups([]), database(), NOW)).toMatchObject({ tone: "warn", text: "No backup yet" });
    const old = set({ taken: NOW - STALE_BACKUP_SECS - 3600 });
    expect(barBackup(backups([old]), database(), NOW)).toMatchObject({ tone: "warn", text: "Backed up 7 days ago" });
  });

  it("says nothing before there is a database, or before the first answer", () => {
    expect(barBackup(backups([]), database("notProvisioned"), NOW)).toBeNull();
    expect(barBackup(null, database(), NOW)).toBeNull();
    expect(barBackup(backups([]), null, NOW)).toBeNull();
  });

  it("shows unknown when the folder could not be read", () => {
    expect(barBackup(backups([], "Access denied."), database(), NOW)).toMatchObject({ tone: "idle", text: "Backups unknown", title: "Access denied." });
  });
});

describe("barAddress", () => {
  it("is the address the launcher writes into the client, else the address the servers are on", () => {
    expect(barAddress(server({ host: "127.0.0.1" }), null)).toBe("127.0.0.1");
    expect(barAddress(null, null)).toBeNull();
  });
});

describe("formatAge", () => {
  it("uses the largest unit that fits", () => {
    expect(formatAge(5)).toBe("just now");
    expect(formatAge(-30)).toBe("just now");
    expect(formatAge(5 * 60)).toBe("5 min ago");
    expect(formatAge(2 * 3600 + 900)).toBe("2 h ago");
    expect(formatAge(86_400)).toBe("1 day ago");
    expect(formatAge(9 * 86_400)).toBe("9 days ago");
    expect(formatAge(null)).toBe("—");
  });
});
