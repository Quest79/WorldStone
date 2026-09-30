import { FolderOpen, RefreshCw } from "lucide-react";
import { useState } from "react";
import type {
  BuildStatus,
  ClientStatus,
  ConfigFileState,
  ConfigStatus,
  DatabaseStatus,
  ExtractStatus,
  RecipeSummary,
  RepoState,
  SchemaStatus,
  ServerStatus,
  Status,
} from "../api/types";
import type { Worldstone } from "../hooks/useWorldstone";
import { Button } from "../components/Button";
import { ErrorBanner } from "../components/ErrorBanner";
import { StatusBadge } from "../components/StatusBadge";
import {
  formatBytes,
  summarizeBuild,
  summarizeClient,
  summarizeConfig,
  summarizeDatabase,
  summarizeExtract,
  summarizeServer,
} from "../lib/summary";

const REPO_STATUS: Record<RepoState, Status> = { ready: "ready", missing: "missing", incomplete: "warning" };
const REPO_LABEL: Record<RepoState, string> = { ready: "Downloaded", missing: "Not downloaded", incomplete: "Needs attention" };

function Section({ title, action, children }: { title: string; action?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section className="overflow-hidden rounded-lg border border-line bg-panel">
      <div className="flex items-center justify-between border-b border-line px-4 py-3">
        <h2 className="text-sm font-semibold">{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}

function BuildDetails({ build }: { build: BuildStatus }) {
  const summary = summarizeBuild(build);
  const rows: Array<[string, string]> = [["Images", build.image]];
  if (build.commit) rows.push(["Built from commit", build.commit]);
  if (build.builtAt) rows.push(["Built", new Date(build.builtAt).toLocaleString()]);
  if (build.sizeBytes !== null) rows.push(["Size", formatBytes(build.sizeBytes)]);
  if (build.playerbots !== null) rows.push(["Playerbots", build.playerbots ? "Compiled in (verified in the image)" : "Not proven"]);
  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} className="shrink-0" />
      </div>
      {build.state !== "notBuilt" && build.state !== "unknown" && (
        <dl className="grid grid-cols-[9rem_1fr] gap-x-4 gap-y-1 text-xs">
          {rows.map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="text-muted">{label}</dt>
              <dd className="min-w-0 break-all font-mono">{value}</dd>
            </div>
          ))}
        </dl>
      )}
      {summary.blocked && <p className="text-xs text-muted">Build is unavailable: {summary.blocked}</p>}
    </div>
  );
}

function Facts({ rows }: { rows: Array<[string, string]> }) {
  return (
    <dl className="grid grid-cols-[9rem_1fr] gap-x-4 gap-y-1 text-xs">
      {rows.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-muted">{label}</dt>
          <dd className="min-w-0 break-all font-mono">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function ClientDetails({ ws, client, recipe }: { ws: Worldstone; client: ClientStatus; recipe: RecipeSummary | null }) {
  const [typed, setTyped] = useState("");
  const summary = summarizeClient(client);
  const info = client.info;
  const rows: Array<[string, string]> = [];
  if (info) {
    rows.push(["Folder", info.path], ["Version", `${info.version} (build ${info.build})`]);
    rows.push(["Languages", info.locales.length > 0 ? info.locales.join(", ") : "Merged into the Data folder"]);
    rows.push(["Data size", formatBytes(info.dataBytes)]);
  } else if (client.path) {
    rows.push(["Remembered folder", client.path]);
  }
  const busy = ws.running || ws.choosingClient;
  const accepted = recipe?.clientBuilds.map((b) => `${b.name} (${b.number})`).join(", ");
  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} className="shrink-0" />
      </div>
      {rows.length > 0 && <Facts rows={rows} />}
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={async (event) => {
          event.preventDefault();
          if (await ws.setClientPath(typed)) setTyped("");
        }}
      >
        <Button variant="secondary" className="h-8" onClick={ws.chooseClient} busy={ws.choosingClient} disabled={!ws.native || busy}>
          {!ws.choosingClient && <FolderOpen className="size-3.5" aria-hidden />} Browse…
        </Button>
        <input
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          placeholder="or paste the folder that contains WoW.exe"
          aria-label="Game client folder"
          spellCheck={false}
          className="h-8 min-w-0 flex-1 rounded-md border border-line bg-bg px-2.5 font-mono text-xs text-fg placeholder:text-muted"
        />
        <Button type="submit" variant="secondary" className="h-8" disabled={!ws.native || busy || typed.trim() === ""}>
          Use folder
        </Button>
      </form>
      {accepted && <p className="text-xs text-muted">Supported client versions: {accepted}.</p>}
    </div>
  );
}

function ExtractDetails({ ws, extract }: { ws: Worldstone; extract: ExtractStatus }) {
  const summary = summarizeExtract(extract, ws.client);
  const record = extract.record;
  const rows: Array<[string, string]> = [["Data volume", extract.volume]];
  if (record) {
    rows.push(["Client", `${record.clientVersion} (build ${record.clientBuild})`]);
    rows.push(["Extractor commit", record.extractorCommit || "unknown"]);
    rows.push(["Finished", new Date(record.finishedUnix * 1000).toLocaleString()]);
    for (const o of record.outputs) rows.push([o.name, `${o.files.toLocaleString()} files · ${formatBytes(o.bytes)}`]);
  }
  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} className="shrink-0" />
      </div>
      {extract.state !== "unknown" && <Facts rows={rows} />}
      {summary.blocked && <p className="text-xs text-muted">Extraction is unavailable: {summary.blocked}</p>}
    </div>
  );
}

/** One line per database: its version and how much of it is in place. Only facts the backend read. */
function schemaLine(s: SchemaStatus): string {
  if (!s.exists) return "Not created";
  if (s.unmanaged) return `${s.tables} tables this app did not create (left untouched)`;
  if (!s.setupDone) return s.tables > 0 ? `${s.tables} tables, setup unfinished` : "Empty";
  const parts = [`${s.tables} tables`, `version ${s.version ?? "unknown"}`];
  if (s.populatePending > 0) parts.push(`${s.populatePending} of ${s.populateTotal} data files not loaded`);
  if (s.updatesPending > 0) parts.push(`${s.updatesPending} of ${s.updatesTotal} updates waiting`);
  if (s.missingTables.length > 0) parts.push(`missing ${s.missingTables.join(", ")}`);
  return parts.join(" · ");
}

function DatabaseDetails({ database }: { database: DatabaseStatus }) {
  const summary = summarizeDatabase(database);
  const rows: Array<[string, string]> = [["Container", database.container], ["Data volume", database.volume], ["Image", database.image]];
  const read = database.schemas.length > 0;
  if (read) {
    for (const s of database.schemas) rows.push([s.name, schemaLine(s)]);
    rows.push(["Realms", database.realms === null ? "unknown" : String(database.realms)]);
  }
  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} className="shrink-0" />
      </div>
      {database.state !== "unknown" && <Facts rows={rows} />}
      {database.note && <p className="text-xs text-muted">{database.note}</p>}
      {summary.blocked && <p className="text-xs text-muted">Setting up is unavailable: {summary.blocked}</p>}
    </div>
  );
}

const FILE_STATUS: Record<ConfigFileState, Status> = { ready: "ready", outdated: "warning", missing: "missing" };
const FILE_LABEL: Record<ConfigFileState, string> = { ready: "In place", outdated: "Out of date", missing: "Not created" };

function ConfigDetails({ ws, config }: { ws: Worldstone; config: ConfigStatus }) {
  const summary = summarizeConfig(config, ws.build);
  const bots = config.bots;
  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} className="shrink-0" />
      </div>
      <ul className="divide-y divide-line rounded-md border border-line">
        {config.files.map((file) => (
          <li key={file.id} className="flex items-start justify-between gap-4 px-3 py-2">
            <div className="min-w-0">
              <div className="text-sm font-medium">{file.label}</div>
              <div className="break-all font-mono text-xs text-muted">{file.path}</div>
              {file.staleKeys.length > 0 && <div className="text-xs text-muted">Not as it should be: {file.staleKeys.join(", ")}</div>}
            </div>
            <StatusBadge status={FILE_STATUS[file.state]} label={FILE_LABEL[file.state]} className="shrink-0" />
          </li>
        ))}
      </ul>
      {bots && (
        <Facts
          rows={[
            ["Playerbots", bots.enabled ? "Enabled" : "Off"],
            ["Bots online", `${bots.min} to ${bots.max}`],
            ["Bot accounts", String(bots.accounts)],
            ["Player limit", String(bots.playerLimit)],
          ]}
        />
      )}
      <p className="text-xs text-muted">
        Only the settings this app manages are written; anything else you change in these files is kept. Passwords are generated and stay in
        the Config folder.
      </p>
      {config.note && <p className="text-xs text-muted">{config.note}</p>}
      {summary.blocked && <p className="text-xs text-muted">Generating is unavailable: {summary.blocked}</p>}
    </div>
  );
}

function ServerDetails({ ws, server }: { ws: Worldstone; server: ServerStatus }) {
  const summary = summarizeServer(server, { build: ws.build, extract: ws.extract, database: ws.database, config: ws.config });
  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} label={summary.label} className="shrink-0" />
      </div>
      {server.services.length > 0 && (
        <Facts
          rows={server.services.map((s): [string, string] => {
            const answers = s.reachable === null ? "" : s.reachable ? ` · answers on ${server.host}:${s.port}` : ` · does not answer on ${server.host}:${s.port}`;
            return [s.label, `${s.state}${s.exitCode !== null ? ` (exit ${s.exitCode})` : ""} · ${s.container}${answers}`];
          })}
        />
      )}
      {server.note && <p className="text-xs text-muted">{server.note}</p>}
      {summary.startBlocked && summary.startBlocked !== "Checking…" && server.state !== "running" && (
        <p className="text-xs text-muted">Starting is unavailable: {summary.startBlocked}</p>
      )}
    </div>
  );
}

export function SetupPage({ ws }: { ws: Worldstone }) {
  return (
    <div className="space-y-4">
      <ErrorBanner message={ws.error} onDismiss={ws.dismissError} />
      <Section
        title="Dependencies"
        action={
          <Button variant="ghost" className="h-8" onClick={ws.refreshEnvironment} disabled={!ws.native}>
            <RefreshCw className="size-3.5" aria-hidden /> Re-check
          </Button>
        }
      >
        {ws.environment === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <ul className="divide-y divide-line">
            {ws.environment.map((check) => (
              <li key={check.id} className="flex items-start justify-between gap-4 px-4 py-3">
                <div className="min-w-0">
                  <div className="text-sm font-medium">{check.label}</div>
                  <div className="text-xs text-muted">{check.detail}</div>
                </div>
                <StatusBadge status={check.status} />
              </li>
            ))}
          </ul>
        )}
      </Section>

      <Section title="Source repositories">
        {ws.repos === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <ul className="divide-y divide-line">
            {ws.repos.map((repo) => (
              <li key={repo.id} className="flex items-start justify-between gap-4 px-4 py-3">
                <div className="min-w-0">
                  <div className="text-sm font-medium capitalize">{repo.id}</div>
                  <div className="truncate text-xs text-muted" title={repo.url}>
                    {repo.url} · {repo.branch}
                  </div>
                  {(repo.commit || repo.state !== "missing") && (
                    <div className="mt-1 truncate font-mono text-xs text-muted">
                      {repo.commit ? `${repo.commit} ${repo.summary ?? ""}` : repo.detail}
                    </div>
                  )}
                </div>
                <StatusBadge status={REPO_STATUS[repo.state]} label={REPO_LABEL[repo.state]} className="shrink-0" />
              </li>
            ))}
          </ul>
        )}
      </Section>

      <Section title="Server build">
        {ws.build === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <BuildDetails build={ws.build} />
        )}
      </Section>

      <Section title="Game client">
        {ws.client === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <ClientDetails ws={ws} client={ws.client} recipe={ws.recipe} />
        )}
      </Section>

      <Section title="Client data">
        {ws.extract === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <ExtractDetails ws={ws} extract={ws.extract} />
        )}
      </Section>

      <Section title="Database">
        {ws.database === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <DatabaseDetails database={ws.database} />
        )}
      </Section>

      <Section title="Configuration">
        {ws.config === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <ConfigDetails ws={ws} config={ws.config} />
        )}
      </Section>

      <Section title="Servers">
        {ws.server === null ? (
          <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>
        ) : (
          <ServerDetails ws={ws} server={ws.server} />
        )}
      </Section>
    </div>
  );
}
