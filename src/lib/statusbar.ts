// What the thin bar at the bottom of the window says. Pure, so it is tested. Every item comes from state the
// launcher already reads; unknown is shown as unknown, and an item with nothing true to say is left out.

import type { BackupsStatus, BotsStatus, Check, ConnectStatus, DatabaseStatus, ServerStatus, TaskEvent } from "../api/types";
import { formatAge, formatUptime } from "./format";
import { describeService, summarizeBots } from "./summary";

/** ok and idle are the normal colours; warn and bad are for what needs a person. */
export type Tone = "ok" | "warn" | "bad" | "idle";

export interface BarItem {
  tone: Tone;
  text: string;
  /** Quieter words after the text, such as how long the servers have been up. */
  extra: string | null;
  /** The hover text: the full explanation the bar has no room for. */
  title: string;
}

const item = (tone: Tone, text: string, title: string, extra: string | null = null): BarItem => ({ tone, text, extra, title });

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** A backup older than this is worth a nudge. */
export const STALE_BACKUP_SECS = 7 * 24 * 3600;

/** The state of the servers, and how long this run has been going. */
export function barServer(server: ServerStatus | null): BarItem {
  if (!server) return item("idle", "Checking…", "Reading the state of the servers.");
  const trouble = server.services.find((s) => s.state === "crashed" || s.state === "unresponsive" || s.state === "missing" || s.state === "stopped");
  const which = trouble ? describeService(trouble) : null;
  switch (server.state) {
    case "unknown":
      return item("idle", "Server state unknown", server.note ?? "Docker could not be asked.");
    case "stopped":
      return item("idle", "Stopped", "The servers are not running.");
    case "starting":
      return item("warn", "Starting", "The servers are starting.");
    case "running": {
      // The most recently started one, so a server that restarted by itself shows its new run.
      const ups = server.services.filter((s) => s.state === "running" && s.uptimeSecs !== null).map((s) => s.uptimeSecs as number);
      const up = ups.length > 0 ? formatUptime(Math.min(...ups)) : null;
      return item(server.note === null ? "ok" : "warn", "Running", server.note ?? "The realm and world servers are running.", up);
    }
    case "paused":
      return item("warn", "Paused", "Frozen: nobody can play until you resume. The database is still running.");
    case "partial":
      return item("warn", "Partly running", which ?? "One of the servers is not running.", which);
    case "failing":
      return item("bad", "Failing", which ?? "A server keeps stopping.", which);
  }
}

/** The bots in the world, or nothing while the servers are stopped (the server item says so). */
export function barBots(bots: BotsStatus | null): BarItem | null {
  if (!bots || bots.state === "stopped") return null;
  const title = summarizeBots(bots).detail;
  switch (bots.state) {
    case "unknown":
      return item("idle", "Bots unknown", title);
    case "off":
      return item("warn", "Bots off", title);
    case "starting":
      return item("warn", "Bots waiting for the world", title);
    case "creating":
      return item("warn", "Creating bots", title);
    case "loggingIn":
      return item("warn", `Bots logging in · ${bots.online ?? 0} of ${bots.max}`, title);
    case "online": {
      const players = bots.players === null ? "" : ` · ${plural(bots.players, "player")}`;
      return item("ok", `${plural(bots.online ?? 0, "bot")}${players}`, title);
    }
    case "stalled":
      return item("bad", "Bots stalled", title);
  }
}

/** Something the servers should be restarted for: the settings changed after they started. */
export function restartNeeded(server: ServerStatus | null, bots: BotsStatus | null): boolean {
  return server?.restartNeeded === true || bots?.restartNeeded === true;
}

export interface BarOperation {
  tone: Tone;
  text: string;
  /** An operation is going on, so the bar shows progress. */
  busy: boolean;
  /** Its own percentage; null while it only reports that it is working. */
  percent: number | null;
  cancellable: boolean;
  title: string;
}

export interface OperationState {
  task: TaskEvent | null;
  starting: boolean;
  settingUp: boolean;
  setupNote: string | null;
  choosingClient: boolean;
  restartNeeded: boolean;
}

/** The middle of the bar: what is running now, or else the last thing worth knowing. */
export function barOperation({ task, starting, settingUp, setupNote, choosingClient, restartNeeded: restart }: OperationState): BarOperation {
  const idle = (tone: Tone, text: string, title: string): BarOperation => ({ tone, text, busy: false, percent: null, cancellable: false, title });
  if (task?.state === "running") {
    // Inside the one-click Set up, the step's own name says less than "step 3 of 5".
    const lead = settingUp && setupNote ? setupNote : task.label;
    const text = task.step ? `${lead} · ${task.step}` : lead;
    return { tone: "idle", text, busy: true, percent: task.progress?.percent ?? null, cancellable: task.cancellable, title: text };
  }
  if (choosingClient) return idle("idle", "Waiting for you to choose the game client", "The folder picker is open.");
  if (settingUp) {
    const text = setupNote ?? "Setting up…";
    return { tone: "idle", text, busy: true, percent: null, cancellable: false, title: text };
  }
  if (starting) return { tone: "idle", text: "Starting…", busy: true, percent: null, cancellable: false, title: "The operation is starting." };
  if (task?.state === "failed") return idle("bad", `${task.label} failed`, task.message ?? "The operation failed. See the log for details.");
  if (restart) return idle("warn", "Restart needed to apply the changed settings", "The settings changed after the servers started. Restart them to apply.");
  if (task?.state === "cancelled") return idle("idle", `${task.label} cancelled`, `${task.label} was cancelled.`);
  if (task?.state === "succeeded") return idle("idle", "Idle", `Last operation finished: ${task.label}${task.message ? `. ${task.message}` : ""}`);
  return idle("idle", "Idle", "No operation is running.");
}

/** Git and Docker, from the environment checks: the first thing that is wrong, else that Docker is ready. */
export function barDocker(checks: Check[] | null): BarItem {
  if (!checks) return item("idle", "Checking Docker…", "Checking Git and Docker.");
  const words: Record<string, { missing: string; warning: string }> = {
    git: { missing: "Git missing", warning: "Git needs attention" },
    docker: { missing: "Docker not installed", warning: "Docker needs attention" },
    "docker-engine": { missing: "Docker not running", warning: "Docker not on Linux containers" },
    "docker-compose": { missing: "Compose missing", warning: "Compose needs attention" },
  };
  const wrong = (c: Check) => c.status === "missing" || c.status === "error";
  const bad = checks.find(wrong);
  if (bad) return item("bad", words[bad.id]?.missing ?? `${bad.label} missing`, `${bad.label}: ${bad.detail}`);
  const warn = checks.find((c) => c.status === "warning");
  if (warn) return item("warn", words[warn.id]?.warning ?? `${warn.label} needs attention`, `${warn.label}: ${warn.detail}`);
  if (checks.every((c) => c.status === "unknown")) return item("idle", "Checking Docker…", "Checking Git and Docker.");
  return item("ok", "Docker ready", "Git and Docker are ready.");
}

/** How long ago the databases were last backed up. Nothing is said before there is a database to back up. */
export function barBackup(backups: BackupsStatus | null, database: DatabaseStatus | null, nowSecs: number): BarItem | null {
  if (!backups || !database || database.state === "notProvisioned") return null;
  if (backups.note) return item("idle", "Backups unknown", backups.note);
  const newest = backups.sets.find((s) => s.complete);
  if (!newest) {
    return backups.sets.length === 0
      ? item("warn", "No backup yet", "Nothing has been backed up. Make one on the Maintenance page.")
      : item("warn", "No complete backup", "The backups that exist do not hold every database. Make a new one on the Maintenance page.");
  }
  const age = nowSecs - newest.taken;
  const stale = age > STALE_BACKUP_SECS;
  return item(stale ? "warn" : "idle", `Backed up ${formatAge(age)}`, stale ? "The newest backup is more than a week old." : `The newest complete backup (${newest.label}) of ${plural(newest.files.length, "database")}.`);
}

/** The address to put in the game client's realm list: the one the launcher would write. */
export function barAddress(server: ServerStatus | null, connect: ConnectStatus | null): string | null {
  return connect?.wanted ?? server?.host ?? null;
}
