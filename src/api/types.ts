// Shapes that cross the Tauri boundary. They mirror the serde models in src-tauri.

export type Status = "unknown" | "ready" | "missing" | "warning" | "error";

export interface Check {
  id: string;
  label: string;
  status: Status;
  detail: string;
}

export interface AppInfo {
  name: string;
  version: string;
  root: string;
}

export interface RepoSummary {
  id: string;
  url: string;
  branch: string;
}

export interface ClientBuild {
  number: number;
  name: string;
}

/** How a server looks in the app: the colours and emblem of its expansion, from its recipe. */
export interface RecipeTheme {
  accent: string;
  deep: string;
  emblem: string;
  backdrop: string | null;
}

export interface RecipeSummary {
  id: string;
  name: string;
  game: string;
  clientBuild: number;
  clientBuilds: ClientBuild[];
  theme: RecipeTheme | null;
  repos: RepoSummary[];
}

export type RepoState = "missing" | "ready" | "incomplete";

export interface RepoStatus {
  id: string;
  url: string;
  branch: string;
  state: RepoState;
  commit: string | null;
  summary: string | null;
  dirty: boolean;
  detail: string;
}

export type TaskState = "running" | "succeeded" | "failed" | "cancelled";

export interface Progress {
  label: string;
  percent: number;
}

export interface TaskEvent {
  id: string;
  label: string;
  state: TaskState;
  step: string | null;
  progress: Progress | null;
  message: string | null;
  cancellable: boolean;
}

export type LogLevel = "info" | "stdout" | "stderr" | "error";

export interface LogEvent {
  taskId: string | null;
  level: LogLevel;
  text: string;
  tsMs: number;
}

export type BuildState = "unknown" | "notBuilt" | "ready" | "outdated";

export interface BuildStatus {
  state: BuildState;
  /** Why a build cannot start right now, if it cannot. */
  blocked: string | null;
  image: string;
  builtAt: string | null;
  sizeBytes: number | null;
  /** The source commit the existing image was built from. */
  commit: string | null;
  /** Whether the image was verified to contain playerbots; null when there is no image. */
  playerbots: boolean | null;
}

/** A game client folder that passed every check. */
export interface ClientInfo {
  path: string;
  dataDir: string;
  build: number;
  version: string;
  locales: string[];
  dataBytes: number;
}

export interface ClientStatus {
  /** The remembered folder, even when it no longer passes. */
  path: string | null;
  info: ClientInfo | null;
  /** Why the remembered folder cannot be used, if it cannot. */
  problem: string | null;
}

export type ExtractState = "unknown" | "notExtracted" | "ready";

export interface OutputStat {
  name: string;
  files: number;
  bytes: number;
}

export interface ExtractRecord {
  clientBuild: number;
  clientVersion: string;
  extractorCommit: string;
  finishedUnix: number;
  seconds: number;
  outputs: OutputStat[];
}

export interface ExtractStatus {
  state: ExtractState;
  /** Why extraction cannot start right now, if it cannot. */
  blocked: string | null;
  /** Something worth knowing that does not stop extraction. */
  note: string | null;
  volume: string;
  record: ExtractRecord | null;
  /** An interrupted extraction is waiting to be resumed. */
  resumable: boolean;
}

export type DatabaseState = "unknown" | "notProvisioned" | "stopped" | "starting" | "incomplete" | "outdated" | "ready";

export interface SchemaStatus {
  name: string;
  exists: boolean;
  tables: number;
  /** The tables were created by this app and setup ran to the end. */
  setupDone: boolean;
  /** Holds tables this app did not create, so it will not touch them. */
  unmanaged: boolean;
  populateTotal: number;
  populatePending: number;
  /** The newest database version, such as "22.6.4". */
  version: string | null;
  updatesTotal: number;
  updatesPending: number;
  missingTables: string[];
}

export interface DatabaseStatus {
  state: DatabaseState;
  /** Why setting up cannot start right now, if it cannot. */
  blocked: string | null;
  /** Something worth knowing that does not stop setting up. */
  note: string | null;
  container: string;
  volume: string;
  image: string;
  schemas: SchemaStatus[];
  /** How many realms players can pick; null when it could not be read. */
  realms: number | null;
}

export type ConfigState = "unknown" | "notConfigured" | "outdated" | "ready";
export type ConfigFileState = "missing" | "outdated" | "ready";

export interface ConfigFileStatus {
  id: string;
  label: string;
  /** Where the file is, so it can be opened and edited. */
  path: string;
  state: ConfigFileState;
  /** Managed settings that are not as they should be. Names only, never values. */
  staleKeys: string[];
}

export interface BotsConfigured {
  enabled: boolean;
  min: number;
  max: number;
  accounts: number;
  playerLimit: number;
}

export interface ConfigStatus {
  state: ConfigState;
  /** Why generating cannot run right now, if it cannot. */
  blocked: string | null;
  note: string | null;
  files: ConfigFileStatus[];
  /** The bot settings as configured; null until the settings exist. */
  bots: BotsConfigured | null;
}

export type ServiceState = "missing" | "stopped" | "starting" | "running" | "paused" | "unresponsive" | "crashed";
export type ServerState = "unknown" | "stopped" | "starting" | "running" | "paused" | "partial" | "failing";

export interface ServiceStatus {
  id: string;
  label: string;
  container: string;
  port: number;
  state: ServiceState;
  exitCode: number | null;
  /** Whether the port answers on this machine; null when it was not asked. */
  reachable: boolean | null;
  /** How long this run has been going, for a server that is up. */
  uptimeSecs: number | null;
}

export interface ServerStatus {
  state: ServerState;
  note: string | null;
  /** The address the servers are published on. */
  host: string;
  services: ServiceStatus[];
  /** A configuration file was changed after a server started, so it still works from the old one. */
  restartNeeded: boolean;
  /** Other instances whose servers are up on a port these need; they have to be stopped first. */
  heldBy: string[];
}

export type ConnectState = "noClient" | "unknown" | "noFile" | "pointed" | "elsewhere";

/** Where the game client logs in, read from its realm list file. */
export interface ConnectStatus {
  state: ConnectState;
  note: string | null;
  client: string | null;
  /** The file that holds the address. */
  file: string | null;
  /** The address the file sets now. */
  address: string | null;
  /** The address this server is reached at from this machine. */
  wanted: string;
  /** The newest saved copy of the file, and the address in it. */
  backup: string | null;
  backupAddress: string | null;
}

export type AccountsState = "unknown" | "blocked" | "ready";

export interface Account {
  name: string;
  gmLevel: number;
  /** Seconds since the epoch; null when the database has no date. */
  created: number | null;
  lastLogin: number | null;
  /** The password is the account's own name. */
  defaultPassword: boolean;
}

export interface AccountsStatus {
  state: AccountsState;
  note: string | null;
  accounts: Account[];
  /** Accounts the playerbots made for themselves; they are not listed. */
  botAccounts: number;
  /** Names starting with this (in any letter case) belong to the bots. */
  botPrefix: string;
  maxGmLevel: number;
  nameMax: number;
  passwordMax: number;
}

export type BotsState = "unknown" | "off" | "stopped" | "starting" | "creating" | "loggingIn" | "online" | "stalled";

export interface BotsStatus {
  state: BotsState;
  note: string;
  enabled: boolean;
  min: number;
  max: number;
  playerLimit: number;
  /** Null when the database could not be asked. */
  botAccounts: number | null;
  characters: number | null;
  online: number | null;
  /** Real players in the world, not counting bots. */
  players: number | null;
  /** The saved settings differ from what the running world server was started with. */
  restartNeeded: boolean;
}

export interface BotChoice {
  enabled: boolean;
  min: number;
  max: number;
}

export type ImageKind = "current" | "old" | "database";

export interface ImageRow {
  reference: string;
  name: string;
  tag: string;
  /** The first 12 characters of the image id, as Docker Desktop shows it. */
  id: string;
  kind: ImageKind;
  /** The heading: which server this is ("Realm server"), or "Old build". */
  title: string;
  /** What this image is, in words. */
  what: string;
  sizeBytes: number;
  created: string;
  /** The containers made from this image. */
  usedBy: string[];
  removable: boolean;
}

export interface ImagesStatus {
  /** Set when Docker could not be asked; the list is then empty and says nothing about the images. */
  note: string | null;
  images: ImageRow[];
}

export interface BackupFile {
  schema: string;
  file: string;
  bytes: number;
}

/** The dumps one run made, shown as one backup. */
export interface BackupSet {
  /** The time and label in the file names, such as `20260929-101500-manual`. */
  id: string;
  /** Seconds since the epoch. */
  taken: number;
  label: string;
  /** Why it exists, in words. */
  what: string;
  files: BackupFile[];
  bytes: number;
  /** Every one of this server's databases is in it. */
  complete: boolean;
}

export interface BackupsStatus {
  folder: string;
  /** Newest first. */
  sets: BackupSet[];
  /** Why the folder could not be read. */
  note: string | null;
  /** Why a backup cannot be made right now. */
  backupBlocked: string | null;
  /** Why a backup cannot be restored right now. */
  restoreBlocked: string | null;
}

export interface DashboardRow {
  id: string;
  label: string;
  container: string;
  state: ServiceState;
  port: number;
  /** The port is published on this machine; the database is reachable only by the servers. */
  published: boolean;
  uptimeSecs: number | null;
  /** Share of one processor core, in percent; a busy container can pass 100. */
  cpuPercent: number | null;
  memoryBytes: number | null;
  memoryLimitBytes: number | null;
}

export interface Dashboard {
  host: string;
  /** Set when Docker could not be asked, or could not give the usage. */
  note: string | null;
  rows: DashboardRow[];
}

export type LogSourceKind = "launcher" | "container" | "file";

export interface LogSource {
  id: string;
  label: string;
  kind: LogSourceKind;
  /** Something can be read from it now. */
  available: boolean;
  /** What it is, or why it cannot be read. */
  detail: string | null;
  bytes: number | null;
  /** Seconds since the epoch. */
  modified: number | null;
}

export interface ViewLine {
  tsMs: number | null;
  level: LogLevel;
  text: string;
}

export interface LogView {
  source: string;
  lines: ViewLine[];
  /** The source holds older lines than the ones shown. */
  truncated: boolean;
  note: string | null;
}

export type RepairFix = "source" | "build" | "extract" | "database" | "configure" | "leftovers" | "servers";

export interface RepairFinding {
  title: string;
  detail: string;
  /** What repairs it; null when only the person can. */
  fix: RepairFix | null;
  action: string | null;
}

export interface RepairStatus {
  findings: RepairFinding[];
  canRepair: boolean;
}

export interface ResetItem {
  id: string;
  label: string;
  detail: string;
  exists: boolean;
  bytes: number | null;
}

export interface ResetPreview {
  /** What has to be typed to confirm. */
  phrase: string;
  items: ResetItem[];
  /** What a reset never touches, in words. */
  kept: string[];
  blocked: string | null;
}

export type FontSource = "installed" | "google";

/** A line round each letter; `width` is how thick it looks, in pixels. */
export interface TextOutline {
  enabled: boolean;
  width: number;
  color: string;
  opacity: number;
}

export interface TextShadow {
  enabled: boolean;
  x: number;
  y: number;
  blur: number;
  color: string;
  opacity: number;
}

/** One font role: which font, and how its letters are dressed. */
export interface FontStyle {
  source: FontSource;
  family: string;
  outline: TextOutline;
  shadow: TextShadow;
}

export interface FontSettings {
  ui: FontStyle;
  serverName: FontStyle;
}

export interface Appearance {
  fonts: FontSettings;
}

export interface AppearanceState {
  current: Appearance;
  defaults: Appearance;
  /** Why the saved file could not be read; the defaults are in use then. */
  problem: string | null;
}

export interface GoogleFamily {
  family: string;
  category: string;
  weights: number[];
}

export interface GoogleCatalogue {
  fetchedAt: number;
  fonts: GoogleFamily[];
  /** Why the list is not a fresh one, when a refresh failed. */
  note?: string;
}

export interface GoogleFace {
  weight: number;
  /** The font file, base64 encoded. */
  data: string;
}
