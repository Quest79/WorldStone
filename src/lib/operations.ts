// What the Maintenance page says and allows for backups, updating, repairing, resetting and the
// service dashboard. Pure, so every rule is tested. A rule that is only enforced here is not a rule:
// the backend checks the same things again before it changes anything.

import type {
  BackupSet,
  BackupsStatus,
  BuildStatus,
  DashboardRow,
  DatabaseStatus,
  RepairStatus,
  RepoStatus,
  ResetPreview,
  ServiceState,
  Status,
} from "../api/types";
import { formatMemory, formatMoment, formatPercent, formatSize, formatUptime } from "./format";
import { CHECKING } from "./summary";

// ---------------------------------------------------------------------------------------
// Backups
// ---------------------------------------------------------------------------------------

/** Why *Back up now* is disabled, or null. */
export function backupBlocked(backups: BackupsStatus | null): string | null {
  return backups === null ? CHECKING : backups.backupBlocked;
}

/** Why no backup can be restored right now, or null. */
export function restoreBlocked(backups: BackupsStatus | null): string | null {
  return backups === null ? CHECKING : backups.restoreBlocked;
}

/** The databases in a backup, in the order they are stored. */
export function databasesIn(set: BackupSet): string[] {
  return set.files.map((f) => f.schema);
}

/** `3 databases · 4.1 MiB`, and whether it is a whole backup. */
export function describeSet(set: BackupSet): string {
  const count = set.files.length;
  const size = formatSize(set.bytes);
  return `${count} database${count === 1 ? "" : "s"} · ${size}${set.complete ? "" : " · not every database"}`;
}

export function setTitle(set: BackupSet): string {
  return formatMoment(set.taken);
}

/** What restoring this backup does, said before the person confirms. */
export function restoreWarning(set: BackupSet): string {
  return (
    `This replaces ${databasesIn(set).join(", ")} with the copy from ${setTitle(set)}. Everything written since then is lost from those databases, ` +
    "but a backup of what is there now is taken first, so this can be undone."
  );
}

// ---------------------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------------------

export interface UpdateGates {
  repos: RepoStatus[] | null;
  build: BuildStatus | null;
  database: DatabaseStatus | null;
}

/** Why *Update* is disabled, or null. It refreshes a server that is already set up. */
export function updateBlocked({ repos, build, database }: UpdateGates): string | null {
  if (repos === null || build === null || database === null) return CHECKING;
  if (repos.some((r) => r.state !== "ready")) return "The source is not fully downloaded. Use Set up first.";
  if (build.state === "unknown") return build.blocked ?? "Docker could not be asked about the server build.";
  if (build.state === "notBuilt") return "The server has not been built yet. Use Set up first.";
  if (database.state === "notProvisioned") return "There is no database yet. Use Set up first.";
  if (database.state === "unknown") return database.note ?? "The database could not be read.";
  return null;
}

// ---------------------------------------------------------------------------------------
// Repair
// ---------------------------------------------------------------------------------------

export interface RepairSummary {
  status: Status;
  headline: string;
  /** Why the button is disabled, or null. */
  blocked: string | null;
  /** The findings only the person can put right. */
  manual: number;
}

export function summarizeRepair(repair: RepairStatus | null): RepairSummary {
  if (repair === null) return { status: "unknown", headline: CHECKING, blocked: CHECKING, manual: 0 };
  if (repair.findings.length === 0) {
    return { status: "ready", headline: "Nothing needs repairing.", blocked: "Nothing needs repairing.", manual: 0 };
  }
  const manual = repair.findings.filter((f) => f.fix === null).length;
  const headline = `${repair.findings.length} problem${repair.findings.length === 1 ? "" : "s"} found`;
  return {
    status: "warning",
    headline,
    blocked: repair.canRepair ? null : "The app cannot repair this by itself. See what each problem needs.",
    manual,
  };
}

/** The button's words. */
export function repairLabel(summary: RepairSummary, findings: number): string {
  return summary.blocked === null ? `Repair ${findings} problem${findings === 1 ? "" : "s"}` : "Repair";
}

// ---------------------------------------------------------------------------------------
// Reset
// ---------------------------------------------------------------------------------------

/** Whether what was typed confirms the reset. The backend checks the same rule. */
export function resetConfirmed(phrase: string, typed: string): boolean {
  return phrase !== "" && typed.trim() === phrase;
}

/** What a reset would delete with the current choices, in the order they are listed. */
export function resetTargets(preview: ResetPreview | null, gameData: boolean): string[] {
  if (!preview) return [];
  return preview.items.filter((i) => i.exists && (i.id !== "gameData" || gameData)).map((i) => i.label);
}

/** Why *Reset* is disabled, or null when it can run. */
export function resetBlocked(preview: ResetPreview | null, gameData: boolean, typed: string): string | null {
  if (preview === null) return CHECKING;
  if (preview.blocked) return preview.blocked;
  if (resetTargets(preview, gameData).length === 0) return "There is nothing to reset yet.";
  if (!resetConfirmed(preview.phrase, typed)) return `Type ${preview.phrase} to confirm.`;
  return null;
}

// ---------------------------------------------------------------------------------------
// The service dashboard
// ---------------------------------------------------------------------------------------

const STATE_WORD: Record<ServiceState, string> = {
  missing: "Not created",
  stopped: "Stopped",
  starting: "Starting",
  running: "Running",
  paused: "Paused",
  unresponsive: "Not answering",
  crashed: "Crashed",
};

const STATE_TONE: Record<ServiceState, Status> = {
  missing: "missing",
  stopped: "missing",
  starting: "warning",
  running: "ready",
  paused: "warning",
  unresponsive: "error",
  crashed: "error",
};

export interface DashboardLine {
  id: string;
  label: string;
  container: string;
  state: string;
  tone: Status;
  /** `127.0.0.1:8085`, or that it is only reachable by the servers. */
  port: string;
  uptime: string;
  cpu: string;
  memory: string;
}

/** A row as the page writes it. Anything Docker did not give is a dash, never a zero. */
export function describeRow(row: DashboardRow, host: string): DashboardLine {
  return {
    id: row.id,
    label: row.label,
    container: row.container,
    state: STATE_WORD[row.state],
    tone: STATE_TONE[row.state],
    port: row.published ? `${host}:${row.port}` : `${row.port} (only the servers)`,
    uptime: formatUptime(row.uptimeSecs),
    cpu: formatPercent(row.cpuPercent),
    memory: formatMemory(row.memoryBytes, row.memoryLimitBytes),
  };
}
