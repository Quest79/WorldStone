// The only module that talks to the native backend. Components never call `invoke` directly.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AccountsStatus,
  AppInfo,
  Appearance,
  AppearanceState,
  BackupsStatus,
  BotChoice,
  BotsStatus,
  BuildStatus,
  Check,
  ClientInfo,
  ClientStatus,
  ConfigStatus,
  ConnectStatus,
  Dashboard,
  DatabaseStatus,
  ExtractStatus,
  GoogleCatalogue,
  GoogleFace,
  ImagesStatus,
  LogEvent,
  LogSource,
  LogView,
  RecipeSummary,
  RepairStatus,
  RepoStatus,
  ResetPreview,
  ServerStatus,
  TaskEvent,
} from "./types";

/** True inside the desktop app; false when the page is opened in a plain browser. */
export const isNative = (): boolean => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Backend errors arrive as plain strings; anything else gets a readable fallback. */
export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Something went wrong. See the log for details.";
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listRecipes: () => invoke<RecipeSummary[]>("list_recipes"),
  checkEnvironment: () => invoke<Check[]>("check_environment"),
  repoStatus: (recipeId: string) => invoke<RepoStatus[]>("repo_status", { recipeId }),
  setupRepositories: (recipeId: string) => invoke<string>("setup_repositories", { recipeId }),
  buildStatus: (recipeId: string) => invoke<BuildStatus>("build_status", { recipeId }),
  buildServer: (recipeId: string, force: boolean) => invoke<string>("build_server", { recipeId, force }),
  clientStatus: (recipeId: string) => invoke<ClientStatus>("client_status", { recipeId }),
  setClient: (recipeId: string, path: string) => invoke<ClientInfo>("set_client", { recipeId, path }),
  /** Opens the system folder picker; null when it was closed without a choice. */
  chooseClient: (recipeId: string) => invoke<ClientInfo | null>("choose_client", { recipeId }),
  extractStatus: (recipeId: string) => invoke<ExtractStatus>("extract_status", { recipeId }),
  extractClientData: (recipeId: string) => invoke<string>("extract_client_data", { recipeId }),
  databaseStatus: (recipeId: string) => invoke<DatabaseStatus>("database_status", { recipeId }),
  provisionDatabase: (recipeId: string) => invoke<string>("provision_database", { recipeId }),
  configStatus: (recipeId: string) => invoke<ConfigStatus>("config_status", { recipeId }),
  configureServer: (recipeId: string) => invoke<string>("configure_server", { recipeId }),
  serverStatus: (recipeId: string) => invoke<ServerStatus>("server_status", { recipeId }),
  startServer: (recipeId: string) => invoke<string>("start_server", { recipeId }),
  stopServer: (recipeId: string) => invoke<string>("stop_server", { recipeId }),
  restartServer: (recipeId: string) => invoke<string>("restart_server", { recipeId }),
  pauseServer: (recipeId: string) => invoke<string>("pause_server", { recipeId }),
  resumeServer: (recipeId: string) => invoke<string>("resume_server", { recipeId }),
  launchGame: (recipeId: string) => invoke<string>("launch_game", { recipeId }),
  connectStatus: (recipeId: string) => invoke<ConnectStatus>("connect_status", { recipeId }),
  pointClient: (recipeId: string) => invoke<string>("point_client", { recipeId }),
  restoreClient: (recipeId: string) => invoke<string>("restore_client", { recipeId }),
  accountsStatus: (recipeId: string) => invoke<AccountsStatus>("accounts_status", { recipeId }),
  createAccount: (recipeId: string, name: string, password: string, gmLevel: number) =>
    invoke<string>("create_account", { recipeId, name, password, gmLevel }),
  changePassword: (recipeId: string, name: string, password: string) => invoke<string>("change_password", { recipeId, name, password }),
  botsStatus: (recipeId: string) => invoke<BotsStatus>("bots_status", { recipeId }),
  saveBotSettings: (recipeId: string, choice: BotChoice) => invoke<string>("save_bot_settings", { recipeId, choice }),
  imagesStatus: (recipeId: string) => invoke<ImagesStatus>("images_status", { recipeId }),
  removeOldImages: (recipeId: string) => invoke<string>("remove_old_images", { recipeId }),
  backupsStatus: (recipeId: string) => invoke<BackupsStatus>("backups_status", { recipeId }),
  createBackup: (recipeId: string) => invoke<string>("create_backup", { recipeId }),
  restoreBackup: (recipeId: string, setId: string) => invoke<string>("restore_backup", { recipeId, setId }),
  openBackupsFolder: (recipeId: string) => invoke<void>("open_backups_folder", { recipeId }),
  dashboardStatus: (recipeId: string) => invoke<Dashboard>("dashboard_status", { recipeId }),
  logSources: (recipeId: string) => invoke<LogSource[]>("log_sources", { recipeId }),
  readLog: (recipeId: string, source: string, lines: number) => invoke<LogView>("read_log", { recipeId, source, lines }),
  updateServer: (recipeId: string) => invoke<string>("update_server", { recipeId }),
  repairStatus: (recipeId: string) => invoke<RepairStatus>("repair_status", { recipeId }),
  repairInstance: (recipeId: string) => invoke<string>("repair_instance", { recipeId }),
  resetPreview: (recipeId: string) => invoke<ResetPreview>("reset_preview", { recipeId }),
  resetInstance: (recipeId: string, confirmation: string, gameData: boolean) =>
    invoke<string>("reset_instance", { recipeId, confirmation, gameData }),
  getAppearance: () => invoke<AppearanceState>("get_appearance"),
  saveAppearance: (appearance: Appearance) => invoke<void>("save_appearance", { appearance }),
  installedFonts: (refresh: boolean) => invoke<string[]>("installed_fonts", { refresh }),
  googleCatalogue: (refresh: boolean) => invoke<GoogleCatalogue>("google_catalogue", { refresh }),
  googleDownloaded: () => invoke<string[]>("google_downloaded"),
  /** Downloads a Google family and returns its exact name. */
  downloadGoogleFont: (family: string) => invoke<string>("download_google_font", { family }),
  googleFontFaces: (family: string) => invoke<GoogleFace[]>("google_font_faces", { family }),
  currentTask: () => invoke<TaskEvent | null>("current_task"),
  cancelTask: () => invoke<boolean>("cancel_task"),
};

export const onTaskUpdate = (handler: (event: TaskEvent) => void): Promise<UnlistenFn> =>
  listen<TaskEvent>("task-update", (e) => handler(e.payload));

export const onTaskLog = (handler: (event: LogEvent) => void): Promise<UnlistenFn> =>
  listen<LogEvent>("task-log", (e) => handler(e.payload));
