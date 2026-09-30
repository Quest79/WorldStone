import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, isNative, onTaskLog, onTaskUpdate } from "../api/client";
import type {
  AccountsStatus,
  AppInfo,
  BackupsStatus,
  BotChoice,
  BotsStatus,
  BuildStatus,
  Check,
  ClientStatus,
  ConfigStatus,
  ConnectStatus,
  Dashboard,
  DatabaseStatus,
  ExtractStatus,
  ImagesStatus,
  LogEvent,
  RecipeSummary,
  RepairStatus,
  RepoStatus,
  ResetPreview,
  ServerStatus,
  TaskEvent,
} from "../api/types";
import { appendLog } from "../lib/logs";
import { planSetup, SETUP_STEPS, type SetupStep } from "../lib/setup";

/** How often the environment is re-checked while nothing is running. Each read asks Docker directly and takes milliseconds. */
const ENVIRONMENT_POLL_MS = 10_000;
/** How often the servers are re-checked while nothing is running, to notice one that stopped. */
const SERVER_POLL_MS = 3_000;
/** Coming back to the window re-reads everything, but not more often than this. */
const REFOCUS_MIN_MS = 2_000;
/** Where the chosen server is remembered between runs. */
const CHOSEN_KEY = "worldstone.server";

function rememberedChoice(): string | null {
  try {
    return window.localStorage.getItem(CHOSEN_KEY);
  } catch {
    return null;
  }
}

function remember(id: string) {
  try {
    window.localStorage.setItem(CHOSEN_KEY, id);
  } catch {
    // Not remembered; the next run starts with the first server.
  }
}

type Action =
  | "source"
  | "build"
  | "extract"
  | "database"
  | "configure"
  | "start"
  | "stop"
  | "restart"
  | "pause"
  | "resume"
  | "launch"
  | "setup"
  | "point"
  | "restore"
  | "account"
  | "password"
  | "bots"
  | "images"
  | "backup"
  | "restoreBackup"
  | "update"
  | "repair"
  | "reset";

/** All launcher state. Components render this; they never talk to the backend themselves. */
export function useWorldstone() {
  const native = isNative();
  const [info, setInfo] = useState<AppInfo | null>(null);
  // Every server the app can set up, and which one the pages show.
  const [recipes, setRecipes] = useState<RecipeSummary[]>([]);
  const [chosenId, setChosenId] = useState<string | null>(null);
  const [environment, setEnvironment] = useState<Check[] | null>(null);
  const [repos, setRepos] = useState<RepoStatus[] | null>(null);
  const [build, setBuild] = useState<BuildStatus | null>(null);
  const [client, setClient] = useState<ClientStatus | null>(null);
  const [extract, setExtract] = useState<ExtractStatus | null>(null);
  const [database, setDatabase] = useState<DatabaseStatus | null>(null);
  const [config, setConfig] = useState<ConfigStatus | null>(null);
  const [server, setServer] = useState<ServerStatus | null>(null);
  const [connect, setConnect] = useState<ConnectStatus | null>(null);
  const [accounts, setAccounts] = useState<AccountsStatus | null>(null);
  const [bots, setBots] = useState<BotsStatus | null>(null);
  const [images, setImages] = useState<ImagesStatus | null>(null);
  const [backups, setBackups] = useState<BackupsStatus | null>(null);
  const [dashboard, setDashboard] = useState<Dashboard | null>(null);
  const [repair, setRepair] = useState<RepairStatus | null>(null);
  const [resetPreview, setResetPreview] = useState<ResetPreview | null>(null);
  // The system folder picker is open, so the button says so instead of looking idle.
  const [choosingClient, setChoosingClient] = useState(false);
  const [task, setTask] = useState<TaskEvent | null>(null);
  const [logs, setLogs] = useState<LogEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  // A click was accepted but the backend has not reported the task yet.
  const [starting, setStarting] = useState(false);
  // The one-click Set up chain is between or inside its steps; nothing else may start meanwhile.
  const [settingUp, setSettingUp] = useState(false);
  const [setupNote, setSetupNote] = useState<string | null>(null);
  // The latest account that was created, for a brief confirmation; `count` tells two of them apart.
  const [accountCreated, setAccountCreated] = useState<{ name: string; passwordIsName: boolean; count: number } | null>(null);
  const createdCount = useRef(0);
  // Which button started the running operation, so only that one shows a spinner.
  const [activeAction, setActiveAction] = useState<Action | null>(null);

  const recipe = recipes.find((r) => r.id === chosenId) ?? null;
  const recipeId = recipe?.id ?? null;
  // The server the pages show now: an answer that arrives after a switch belongs to the other one.
  const shownRef = useRef(recipeId);
  shownRef.current = recipeId;
  const running = starting || settingUp || task?.state === "running";
  const runningRef = useRef(running);
  runningRef.current = running;
  const settingUpRef = useRef(false);
  settingUpRef.current = settingUp;
  // Tasks that ended, and callers waiting for one to end, by task id.
  const endedRef = useRef(new Map<string, TaskEvent>());
  const waitersRef = useRef(new Map<string, (event: TaskEvent) => void>());
  // Whether the world may have bots in it, so their count is worth re-reading while nothing else happens.
  const serverState = server?.state ?? null;
  const worldUpRef = useRef(false);
  worldUpRef.current = serverState === "running" || serverState === "starting";

  const refreshEnvironment = useCallback(async () => {
    try {
      setEnvironment(await api.checkEnvironment());
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  const refreshRepos = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.repoStatus(recipeId);
      if (shownRef.current === recipeId) setRepos(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshBuild = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.buildStatus(recipeId);
      if (shownRef.current === recipeId) setBuild(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshClient = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.clientStatus(recipeId);
      if (shownRef.current === recipeId) setClient(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshExtract = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.extractStatus(recipeId);
      if (shownRef.current === recipeId) setExtract(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshDatabase = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.databaseStatus(recipeId);
      if (shownRef.current === recipeId) setDatabase(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshConfig = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.configStatus(recipeId);
      if (shownRef.current === recipeId) setConfig(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshServer = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.serverStatus(recipeId);
      if (shownRef.current === recipeId) setServer(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshConnect = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.connectStatus(recipeId);
      if (shownRef.current === recipeId) setConnect(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshAccounts = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.accountsStatus(recipeId);
      if (shownRef.current === recipeId) setAccounts(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshBots = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.botsStatus(recipeId);
      if (shownRef.current === recipeId) setBots(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshImages = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.imagesStatus(recipeId);
      if (shownRef.current === recipeId) setImages(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  // The Maintenance page's own statuses. They are read when that page is open and after every
  // operation, not on a timer of their own: some of them ask Docker several things at once.
  const refreshBackups = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.backupsStatus(recipeId);
      if (shownRef.current === recipeId) setBackups(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  // Reading CPU and memory takes seconds, and reads overlap (the timer, an operation ending), so only the
  // newest one may replace what is shown: an older answer arriving late would show servers as they were.
  const dashboardTicket = useRef(0);
  const refreshDashboard = useCallback(async () => {
    if (!recipeId) return;
    const ticket = ++dashboardTicket.current;
    try {
      const next = await api.dashboardStatus(recipeId);
      if (ticket === dashboardTicket.current && shownRef.current === recipeId) setDashboard(next);
    } catch (e) {
      if (ticket === dashboardTicket.current) setError(errorText(e));
    }
  }, [recipeId]);

  const refreshRepair = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.repairStatus(recipeId);
      if (shownRef.current === recipeId) setRepair(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const refreshReset = useCallback(async () => {
    if (!recipeId) return;
    try {
      const next = await api.resetPreview(recipeId);
      if (shownRef.current === recipeId) setResetPreview(next);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  // Load static facts once.
  useEffect(() => {
    if (!native) return;
    api.appInfo().then(setInfo).catch((e) => setError(errorText(e)));
    api
      .listRecipes()
      .then((list) => {
        setRecipes(list);
        const remembered = rememberedChoice();
        setChosenId((list.find((r) => r.id === remembered) ?? list[0])?.id ?? null);
      })
      .catch((e) => setError(errorText(e)));
    void refreshEnvironment();
    // If the page was reloaded while an operation ran, show it again. An event that already
    // arrived is newer than this answer, so it wins.
    api
      .currentTask()
      .then((current) => current && setTask((event) => event ?? current))
      .catch((e) => setError(errorText(e)));
  }, [native, refreshEnvironment]);

  useEffect(() => {
    void refreshRepos();
    void refreshBuild();
    void refreshClient();
    void refreshExtract();
    void refreshDatabase();
    void refreshConfig();
    void refreshServer();
    void refreshConnect();
    void refreshAccounts();
    void refreshBots();
    void refreshImages();
    // The status bar shows the age of the last backup on every page.
    void refreshBackups();
  }, [refreshRepos, refreshBuild, refreshClient, refreshExtract, refreshDatabase, refreshConfig, refreshServer, refreshConnect, refreshAccounts, refreshBots, refreshImages, refreshBackups]);

  // Backend events.
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    const unlisten: Array<() => void> = [];
    const keep = (p: Promise<() => void>) =>
      p.then((u) => (disposed ? u() : unlisten.push(u))).catch((e) => setError(errorText(e)));

    keep(
      onTaskUpdate((event) => {
        setTask(event);
        setStarting(false);
        if (event.state !== "running") {
          endedRef.current.set(event.id, event);
          waitersRef.current.get(event.id)?.(event);
          waitersRef.current.delete(event.id);
          if (!settingUpRef.current) setActiveAction(null);
          void refreshRepos();
          void refreshBuild();
          void refreshClient();
          void refreshExtract();
          void refreshDatabase();
          void refreshConfig();
          void refreshServer();
          void refreshConnect();
          void refreshAccounts();
          void refreshBots();
          void refreshImages();
          void refreshBackups();
          void refreshEnvironment();
        }
      }),
    );
    keep(onTaskLog((entry) => setLogs((current) => appendLog(current, entry))));
    return () => {
      disposed = true;
      unlisten.forEach((u) => u());
    };
  }, [
    native,
    refreshRepos,
    refreshBuild,
    refreshClient,
    refreshExtract,
    refreshDatabase,
    refreshConfig,
    refreshServer,
    refreshConnect,
    refreshAccounts,
    refreshBots,
    refreshImages,
    refreshBackups,
    refreshEnvironment,
  ]);

  // Keep the environment fresh while idle.
  useEffect(() => {
    if (!native) return;
    const timer = setInterval(() => {
      if (runningRef.current) return;
      void refreshEnvironment();
      void refreshBuild();
      void refreshClient();
      void refreshExtract();
      void refreshDatabase();
      void refreshConfig();
      void refreshConnect();
      void refreshAccounts();
    }, ENVIRONMENT_POLL_MS);
    return () => clearInterval(timer);
  }, [native, refreshEnvironment, refreshBuild, refreshClient, refreshExtract, refreshDatabase, refreshConfig, refreshConnect, refreshAccounts]);

  // A server can stop on its own, so its state is read far more often than the rest. The bots come
  // online over minutes, so while the world is up their count is followed too.
  useEffect(() => {
    if (!native) return;
    const timer = setInterval(() => {
      if (runningRef.current) return;
      void refreshServer();
      if (worldUpRef.current) void refreshBots();
    }, SERVER_POLL_MS);
    return () => clearInterval(timer);
  }, [native, refreshServer, refreshBots]);

  // Whatever changed while the window was somewhere else is read again as soon as it is looked at.
  useEffect(() => {
    if (!native) return;
    let last = 0;
    const again = () => {
      if (document.visibilityState !== "visible" || runningRef.current || Date.now() - last < REFOCUS_MIN_MS) return;
      last = Date.now();
      void refreshEnvironment();
      void refreshRepos();
      void refreshBuild();
      void refreshClient();
      void refreshExtract();
      void refreshDatabase();
      void refreshConfig();
      void refreshServer();
      void refreshConnect();
      void refreshAccounts();
      if (worldUpRef.current) void refreshBots();
      void refreshImages();
      void refreshBackups();
    };
    window.addEventListener("focus", again);
    document.addEventListener("visibilitychange", again);
    return () => {
      window.removeEventListener("focus", again);
      document.removeEventListener("visibilitychange", again);
    };
  }, [native, refreshEnvironment, refreshRepos, refreshBuild, refreshClient, refreshExtract, refreshDatabase, refreshConfig, refreshServer, refreshConnect, refreshAccounts, refreshBots, refreshImages, refreshBackups]);

  /** Starts a background operation: the button reacts at once, and errors are shown, not swallowed. Gives the task id, or null when it could not start. */
  const start = useCallback(
    async (action: Action, begin: () => Promise<string>): Promise<string | null> => {
      setError(null);
      setStarting(true);
      setActiveAction(action);
      try {
        return await begin();
      } catch (e) {
        setError(errorText(e));
        setStarting(false);
        setActiveAction(null);
        return null;
      }
    },
    [],
  );

  /** Resolves when the task with this id has ended, whether that was before or after asking. */
  const taskEnded = useCallback(
    (id: string) =>
      new Promise<TaskEvent>((resolve) => {
        const ended = endedRef.current.get(id);
        if (ended) resolve(ended);
        else waitersRef.current.set(id, resolve);
      }),
    [],
  );

  const setupRepositories = useCallback(async () => {
    if (recipeId) await start("source", () => api.setupRepositories(recipeId));
  }, [recipeId, start]);

  const buildServer = useCallback(
    async (force: boolean) => {
      if (recipeId) await start("build", () => api.buildServer(recipeId, force));
    },
    [recipeId, start],
  );

  const extractClientData = useCallback(async () => {
    if (recipeId) await start("extract", () => api.extractClientData(recipeId));
  }, [recipeId, start]);

  const provisionDatabase = useCallback(async () => {
    if (recipeId) await start("database", () => api.provisionDatabase(recipeId));
  }, [recipeId, start]);

  const configureServer = useCallback(async () => {
    if (recipeId) await start("configure", () => api.configureServer(recipeId));
  }, [recipeId, start]);

  const startServer = useCallback(async () => {
    if (recipeId) await start("start", () => api.startServer(recipeId));
  }, [recipeId, start]);

  const stopServer = useCallback(async () => {
    if (recipeId) await start("stop", () => api.stopServer(recipeId));
  }, [recipeId, start]);

  const restartServer = useCallback(async () => {
    if (recipeId) await start("restart", () => api.restartServer(recipeId));
  }, [recipeId, start]);

  const pauseServer = useCallback(async () => {
    if (recipeId) await start("pause", () => api.pauseServer(recipeId));
  }, [recipeId, start]);

  const resumeServer = useCallback(async () => {
    if (recipeId) await start("resume", () => api.resumeServer(recipeId));
  }, [recipeId, start]);

  const launchGame = useCallback(async () => {
    if (recipeId) await start("launch", () => api.launchGame(recipeId));
  }, [recipeId, start]);

  const pointClient = useCallback(async () => {
    if (recipeId) await start("point", () => api.pointClient(recipeId));
  }, [recipeId, start]);

  const restoreClient = useCallback(async () => {
    if (recipeId) await start("restore", () => api.restoreClient(recipeId));
  }, [recipeId, start]);

  const createAccount = useCallback(
    async (name: string, password: string, gmLevel: number) => {
      if (!recipeId) return;
      const id = await start("account", () => api.createAccount(recipeId, name, password, gmLevel));
      if (id === null) return;
      // Only a task that ended well is announced; a failure is the task panel's and the error banner's.
      if ((await taskEnded(id)).state === "succeeded") {
        createdCount.current += 1;
        setAccountCreated({ name, passwordIsName: password === "", count: createdCount.current });
      }
    },
    [recipeId, start, taskEnded],
  );

  const changePassword = useCallback(
    async (name: string, password: string) => {
      if (recipeId) await start("password", () => api.changePassword(recipeId, name, password));
    },
    [recipeId, start],
  );

  const removeOldImages = useCallback(async () => {
    if (recipeId) await start("images", () => api.removeOldImages(recipeId));
  }, [recipeId, start]);

  const createBackup = useCallback(async () => {
    if (recipeId) await start("backup", () => api.createBackup(recipeId));
  }, [recipeId, start]);

  const restoreBackup = useCallback(
    async (setId: string) => {
      if (recipeId) await start("restoreBackup", () => api.restoreBackup(recipeId, setId));
    },
    [recipeId, start],
  );

  const updateServer = useCallback(async () => {
    if (recipeId) await start("update", () => api.updateServer(recipeId));
  }, [recipeId, start]);

  const repairInstance = useCallback(async () => {
    if (recipeId) await start("repair", () => api.repairInstance(recipeId));
  }, [recipeId, start]);

  /** The backend checks the confirmation again and refuses a wrong one before anything starts. */
  const resetInstance = useCallback(
    async (confirmation: string, gameData: boolean) => {
      if (recipeId) await start("reset", () => api.resetInstance(recipeId, confirmation, gameData));
    },
    [recipeId, start],
  );

  const openBackupsFolder = useCallback(async () => {
    if (!recipeId) return;
    setError(null);
    try {
      await api.openBackupsFolder(recipeId);
    } catch (e) {
      setError(errorText(e));
    }
  }, [recipeId]);

  const saveBotSettings = useCallback(
    async (choice: BotChoice) => {
      if (recipeId) await start("bots", () => api.saveBotSettings(recipeId, choice));
    },
    [recipeId, start],
  );

  /** Opens the folder picker. Nothing is remembered unless the folder passes every check. */
  const chooseClient = useCallback(async () => {
    if (!recipeId) return;
    setError(null);
    setChoosingClient(true);
    try {
      await api.chooseClient(recipeId);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setChoosingClient(false);
      void refreshClient();
      void refreshExtract();
      void refreshConnect();
    }
  }, [recipeId, refreshClient, refreshExtract, refreshConnect]);

  /** Checks a typed or pasted folder. Returns whether it was accepted; the reason is shown as the error. */
  const setClientPath = useCallback(
    async (path: string): Promise<boolean> => {
      if (!recipeId) return false;
      setError(null);
      try {
        await api.setClient(recipeId, path);
        return true;
      } catch (e) {
        setError(errorText(e));
        return false;
      } finally {
        void refreshClient();
        void refreshExtract();
        void refreshConnect();
      }
    },
    [recipeId, refreshClient, refreshExtract, refreshConnect],
  );

  /**
   * Does every step that is still missing, one after the other, using the same commands as the
   * step buttons. It re-reads the real state before each step, stops at the first failure or
   * cancel (the task panel shows why), and never runs a step twice.
   */
  const setUp = useCallback(async () => {
    if (!recipeId || settingUpRef.current) return;
    settingUpRef.current = true;
    setError(null);
    setSetupNote(null);
    setSettingUp(true);
    setActiveAction("setup");
    try {
      const ran = new Set<SetupStep>();
      for (;;) {
        const [environment, repos, build, client, extract, database, config] = await Promise.all([
          api.checkEnvironment(),
          api.repoStatus(recipeId),
          api.buildStatus(recipeId),
          api.clientStatus(recipeId),
          api.extractStatus(recipeId),
          api.databaseStatus(recipeId),
          api.configStatus(recipeId),
        ]);
        setEnvironment(environment);
        setRepos(repos);
        setBuild(build);
        setClient(client);
        setExtract(extract);
        setDatabase(database);
        setConfig(config);
        const plan = planSetup({ environment, repos, build, client, extract, database, config });
        if (plan.blocked) {
          setError(plan.blocked);
          return;
        }
        if (plan.next === null) {
          setSetupNote("Everything is set up. Start the servers when you are ready.");
          return;
        }
        const label = SETUP_STEPS.find((s) => s.id === plan.next)?.label ?? plan.next;
        if (ran.has(plan.next)) {
          setError(`${label} finished, but it is still not in place: ${plan.detail}`);
          return;
        }
        ran.add(plan.next);
        const position = SETUP_STEPS.findIndex((s) => s.id === plan.next) + 1;
        setSetupNote(`Set up, step ${position} of ${SETUP_STEPS.length}: ${label}`);
        if (plan.next === "client") {
          setChoosingClient(true);
          try {
            const chosen = await api.chooseClient(recipeId);
            if (!chosen) {
              setSetupNote("Set up stopped: no game client was chosen.");
              return;
            }
          } finally {
            setChoosingClient(false);
          }
          continue;
        }
        const id = await {
          source: () => api.setupRepositories(recipeId),
          build: () => api.buildServer(recipeId, plan.force),
          extract: () => api.extractClientData(recipeId),
          database: () => api.provisionDatabase(recipeId),
          configure: () => api.configureServer(recipeId),
        }[plan.next]();
        const ended = await taskEnded(id);
        if (ended.state !== "succeeded") {
          setSetupNote(`Set up stopped at: ${label}.`);
          return;
        }
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      settingUpRef.current = false;
      setSettingUp(false);
      setActiveAction(null);
      void refreshRepos();
      void refreshBuild();
      void refreshClient();
      void refreshExtract();
      void refreshDatabase();
      void refreshConfig();
      void refreshServer();
      void refreshConnect();
    }
  }, [recipeId, taskEnded, refreshRepos, refreshBuild, refreshClient, refreshExtract, refreshDatabase, refreshConfig, refreshServer, refreshConnect]);

  /** Shows another server. Not while an operation runs: it belongs to the server that started it. */
  const selectRecipe = useCallback((id: string) => {
    if (runningRef.current || id === shownRef.current) return;
    if (!recipes.some((r) => r.id === id)) return;
    // Nothing of the old server is left on screen; every status is read again for the new one.
    shownRef.current = id;
    dashboardTicket.current += 1;
    setRepos(null);
    setBuild(null);
    setClient(null);
    setExtract(null);
    setDatabase(null);
    setConfig(null);
    setServer(null);
    setConnect(null);
    setAccounts(null);
    setBots(null);
    setImages(null);
    setBackups(null);
    setDashboard(null);
    setRepair(null);
    setResetPreview(null);
    setSetupNote(null);
    setAccountCreated(null);
    setError(null);
    remember(id);
    setChosenId(id);
  }, [recipes]);

  const cancel = useCallback(async () => {
    try {
      await api.cancelTask();
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  return {
    native,
    info,
    recipe,
    recipes,
    selectRecipe,
    environment,
    repos,
    build,
    client,
    extract,
    database,
    config,
    server,
    connect,
    accounts,
    bots,
    images,
    backups,
    dashboard,
    repair,
    resetPreview,
    choosingClient,
    task,
    logs,
    error,
    running,
    starting,
    settingUp,
    setupNote,
    accountCreated,
    activeAction,
    dismissError: () => setError(null),
    refreshEnvironment,
    refreshRepos,
    refreshBuild,
    refreshClient,
    refreshExtract,
    refreshDatabase,
    refreshConfig,
    refreshServer,
    refreshConnect,
    refreshAccounts,
    refreshBots,
    refreshImages,
    refreshBackups,
    refreshDashboard,
    refreshRepair,
    refreshReset,
    setupRepositories,
    buildServer,
    chooseClient,
    setClientPath,
    extractClientData,
    provisionDatabase,
    configureServer,
    startServer,
    stopServer,
    restartServer,
    pauseServer,
    resumeServer,
    launchGame,
    setUp,
    pointClient,
    restoreClient,
    createAccount,
    changePassword,
    saveBotSettings,
    removeOldImages,
    createBackup,
    restoreBackup,
    updateServer,
    repairInstance,
    resetInstance,
    openBackupsFolder,
    cancel,
  };
}

export type Worldstone = ReturnType<typeof useWorldstone>;
