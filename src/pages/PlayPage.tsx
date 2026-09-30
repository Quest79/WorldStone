import { Check, Clipboard, Dices, KeyRound, LogIn, RotateCcw, RotateCw, Save, UserPlus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { AccountsStatus, BotChoice, BotsStatus, ConnectStatus } from "../api/types";
import type { Worldstone } from "../hooks/useWorldstone";
import { Button } from "../components/Button";
import { ErrorBanner } from "../components/ErrorBanner";
import { StatusBadge } from "../components/StatusBadge";
import { TaskPanel } from "../components/TaskPanel";
import { accountCreatedText, accountsBlocked, formatWhen, formProblem, generatePassword, gmWord, type AccountForm } from "../lib/accounts";
import { botChoiceProblem, CHECKING, summarizeBots, summarizeConnect } from "../lib/summary";
import { cn } from "../lib/utils";

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="overflow-hidden rounded-lg border border-line bg-panel">
      <div className="border-b border-line px-4 py-3">
        <h2 className="text-sm font-semibold">{title}</h2>
      </div>
      {children}
    </section>
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

/** How long the "account created" confirmation stays fully visible, and then takes to fade. */
const CONFIRM_SHOWN_MS = 2000;
const CONFIRM_FADE_MS = 700;

const INPUT = "h-8 rounded-md border border-line bg-bg px-2.5 text-sm text-fg placeholder:text-muted";

function Waiting({ ws }: { ws: Worldstone }) {
  return <p className="px-4 py-3 text-sm text-muted">{ws.native ? "Checking…" : "Available in the desktop app."}</p>;
}

// ---------------------------------------------------------------------------------------
// Connect
// ---------------------------------------------------------------------------------------

function ConnectDetails({ ws, connect }: { ws: Worldstone; connect: ConnectStatus }) {
  const summary = summarizeConnect(connect);
  const rows: Array<[string, string]> = [];
  if (connect.file) rows.push(["Realm list file", connect.file]);
  if (connect.address !== null) rows.push(["Logs in to", connect.address === "" ? "(no address)" : connect.address]);
  rows.push(["This server is at", connect.wanted]);
  if (connect.backup) rows.push(["Saved copy", `${connect.backup} · ${connect.backupAddress || "no address"}`]);
  const idle = ws.native && ws.recipe !== null && !ws.running;
  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} className="shrink-0" />
      </div>
      {connect.state !== "noClient" && connect.state !== "unknown" && <Facts rows={rows} />}
      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant={connect.state === "elsewhere" || connect.state === "noFile" ? "primary" : "secondary"}
          onClick={ws.pointClient}
          busy={ws.activeAction === "point"}
          disabled={!idle || summary.pointBlocked !== null}
          title={summary.pointBlocked ?? undefined}
        >
          {ws.activeAction !== "point" && <LogIn className="size-4" aria-hidden />}
          Point client at this server
        </Button>
        <Button
          onClick={ws.restoreClient}
          busy={ws.activeAction === "restore"}
          disabled={!idle || summary.restoreBlocked !== null}
          title={summary.restoreBlocked ?? undefined}
        >
          {ws.activeAction !== "restore" && <RotateCcw className="size-4" aria-hidden />}
          Restore previous address
        </Button>
      </div>
      <p className="text-xs text-muted">
        Only the realm list line of that file changes. The address it had is saved next to it first, and a saved copy is never overwritten.
        Your other lines stay as they are.
      </p>
      {summary.pointBlocked && connect.state !== "pointed" && <p className="text-xs text-muted">Pointing is unavailable: {summary.pointBlocked}</p>}
    </div>
  );
}

// ---------------------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------------------

function AccountsDetails({ ws, accounts }: { ws: Worldstone; accounts: AccountsStatus }) {
  const [form, setForm] = useState<AccountForm>({ name: "", password: "", mode: "create" });
  const [gm, setGm] = useState(0);
  const [copied, setCopied] = useState(false);
  const blocked = accountsBlocked(accounts, ws.server);
  const problem = formProblem(form, accounts);
  const touched = form.name !== "" || form.password !== "";
  const idle = ws.native && ws.recipe !== null && !ws.running;
  const changing = form.mode === "password";
  const defaults = accounts.accounts.filter((a) => a.defaultPassword);

  // A green confirmation when an account has been created: shown for a moment, then faded out.
  // The count starts at what was already there, so coming back to this page does not replay it.
  const created = ws.accountCreated;
  const confirmed = useRef(created?.count ?? 0);
  const [confirmation, setConfirmation] = useState<"off" | "shown" | "fading">("off");
  useEffect(() => {
    if (!created || created.count === confirmed.current) return;
    confirmed.current = created.count;
    setConfirmation("shown");
    const fade = setTimeout(() => setConfirmation("fading"), CONFIRM_SHOWN_MS);
    const gone = setTimeout(() => setConfirmation("off"), CONFIRM_SHOWN_MS + CONFIRM_FADE_MS);
    return () => {
      clearTimeout(fade);
      clearTimeout(gone);
    };
  }, [created]);

  // A copied password is confirmed for a moment, then the button goes back to normal.
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);

  const submit = async () => {
    if (changing) await ws.changePassword(form.name, form.password);
    else await ws.createAccount(form.name, form.password, gm);
  };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(form.password);
      setCopied(true);
    } catch {
      // The password is in plain view in the field; selecting it by hand still works.
    }
  };
  const choosePassword = (name: string) => {
    setForm({ name, password: generatePassword(), mode: "password" });
    setCopied(false);
  };

  return (
    <div className="space-y-4 px-4 py-3">
      {accounts.state !== "ready" ? (
        <p className="text-sm text-muted">{accounts.note ?? "The accounts cannot be read."}</p>
      ) : (
        <>
          {defaults.length > 0 && (
            <div className="rounded-md border border-warn/40 bg-warn/10 p-3 text-xs text-warn">
              {defaults.map((a) => a.name).join(", ")} {defaults.length === 1 ? "has" : "have"} a password everyone who has read the emulator&apos;s notes
              knows: the account&apos;s own name. Change {defaults.length === 1 ? "it" : "them"} before anyone else can reach this server.
            </div>
          )}
          {accounts.accounts.length === 0 ? (
            <p className="text-sm text-muted">There are no accounts yet. Create one below to log in.</p>
          ) : (
            <table className="w-full text-left text-xs">
              <thead className="text-muted">
                <tr>
                  <th className="py-1 pr-3 font-medium">Account</th>
                  <th className="py-1 pr-3 font-medium">Level</th>
                  <th className="py-1 pr-3 font-medium">Created</th>
                  <th className="py-1 pr-3 font-medium">Last login</th>
                  <th className="py-1 text-right font-medium" />
                </tr>
              </thead>
              <tbody className="divide-y divide-line">
                {accounts.accounts.map((a) => (
                  <tr key={a.name}>
                    <td className="py-1.5 pr-3 font-mono">
                      {a.name}
                      {a.defaultPassword && <span className="ml-2 rounded bg-warn/15 px-1.5 py-0.5 font-sans text-warn">default password</span>}
                    </td>
                    <td className="py-1.5 pr-3">{gmWord(a.gmLevel)}</td>
                    <td className="py-1.5 pr-3">{formatWhen(a.created)}</td>
                    <td className="py-1.5 pr-3">{formatWhen(a.lastLogin)}</td>
                    <td className="py-1.5 text-right">
                      <Button
                        variant="ghost"
                        className="h-7 px-2 text-xs"
                        onClick={() => choosePassword(a.name)}
                        disabled={!idle || blocked !== null}
                        title={blocked ?? `Choose a new password for ${a.name}`}
                      >
                        <KeyRound className="size-3.5" aria-hidden /> Change password
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {accounts.botAccounts > 0 && <p className="text-xs text-muted">{accounts.botAccounts} more accounts belong to the playerbots and are not listed.</p>}
        </>
      )}

      <form
        className={cn("space-y-2 rounded-md border p-3 transition-colors", confirmation === "shown" ? "border-ok/60" : "border-line")}
        style={{ transitionDuration: `${CONFIRM_FADE_MS}ms` }}
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <div className="text-sm font-medium">{changing ? `New password for ${form.name}` : "Create an account"}</div>
        <div className="flex flex-wrap items-center gap-2">
          {!changing && (
            <input
              value={form.name}
              onChange={(event) => setForm({ ...form, name: event.target.value })}
              placeholder="Account name"
              aria-label="Account name"
              autoComplete="off"
              spellCheck={false}
              maxLength={accounts.nameMax}
              className={cn(INPUT, "w-44 font-mono")}
            />
          )}
          <input
            value={form.password}
            onChange={(event) => {
              setForm({ ...form, password: event.target.value });
              setCopied(false);
            }}
            placeholder={changing ? "New password" : "Password (optional)"}
            aria-label="Password"
            autoComplete="off"
            spellCheck={false}
            maxLength={accounts.passwordMax}
            className={cn(INPUT, "w-44 font-mono")}
          />
          <Button
            variant="secondary"
            className="h-8"
            onClick={() => {
              setForm({ ...form, password: generatePassword() });
              setCopied(false);
            }}
          >
            <Dices className="size-3.5" aria-hidden /> Generate
          </Button>
          <Button variant="secondary" className="h-8" onClick={copy} disabled={form.password === ""}>
            <Clipboard className="size-3.5" aria-hidden /> {copied ? "Copied" : "Copy"}
          </Button>
          {!changing && (
            <label className="flex items-center gap-2 text-xs text-muted">
              Level
              <select value={gm} onChange={(event) => setGm(Number(event.target.value))} className={cn(INPUT, "w-36")} aria-label="Game master level">
                {Array.from({ length: accounts.maxGmLevel + 1 }, (_, level) => (
                  <option key={level} value={level}>
                    {level} · {gmWord(level)}
                  </option>
                ))}
              </select>
            </label>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <Button
            type="submit"
            variant="primary"
            className="h-8"
            busy={ws.activeAction === "account" || ws.activeAction === "password"}
            disabled={!idle || blocked !== null || problem !== null}
            title={blocked ?? problem ?? undefined}
          >
            {ws.activeAction !== "account" && ws.activeAction !== "password" && (changing ? <KeyRound className="size-3.5" aria-hidden /> : <UserPlus className="size-3.5" aria-hidden />)}
            {changing ? "Change password" : "Create account"}
          </Button>
          {changing && (
            <Button className="h-8" onClick={() => setForm({ name: "", password: "", mode: "create" })}>
              Cancel
            </Button>
          )}
          <span role="status" aria-live="polite" className="contents">
            {created && confirmation !== "off" && (
              <span
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md border border-ok/40 bg-ok/15 px-2.5 py-1 text-xs font-medium text-ok transition-opacity",
                  confirmation === "fading" ? "opacity-0" : "opacity-100",
                )}
                style={{ transitionDuration: `${CONFIRM_FADE_MS}ms` }}
              >
                <Check className="size-3.5" aria-hidden /> {accountCreatedText(created.name, created.passwordIsName)}
              </span>
            )}
          </span>
        </div>
        {blocked && blocked !== CHECKING && <p className="text-xs text-muted">Unavailable: {blocked}</p>}
        {!blocked && touched && problem && <p className="text-xs text-muted">{problem}</p>}
        {!changing && form.name !== "" && form.password === "" && problem === null && (
          <p className="text-xs text-muted">No password entered: {form.name.toUpperCase()} will log in with its name as the password. You can change it later.</p>
        )}
        <p className="text-xs text-muted">
          The password is shown here so you can read it and type it into the game. The app does not keep it, and it is never written to the log.
          {!changing && " A new account does not need one."}
        </p>
      </form>
    </div>
  );
}

// ---------------------------------------------------------------------------------------
// Playerbots
// ---------------------------------------------------------------------------------------

function BotsDetails({ ws, bots }: { ws: Worldstone; bots: BotsStatus }) {
  const summary = summarizeBots(bots);
  const [choice, setChoice] = useState<BotChoice>({ enabled: bots.enabled, min: bots.min, max: bots.max });
  // Take up what the backend says when it changes (a save finished), but never while someone is typing a different value.
  const [seen, setSeen] = useState(`${bots.enabled}/${bots.min}/${bots.max}`);
  const now = `${bots.enabled}/${bots.min}/${bots.max}`;
  if (now !== seen) {
    setSeen(now);
    setChoice({ enabled: bots.enabled, min: bots.min, max: bots.max });
  }
  const problem = botChoiceProblem(choice);
  const same = choice.enabled === bots.enabled && choice.min === bots.min && choice.max === bots.max;
  const idle = ws.native && ws.recipe !== null && !ws.running;
  const settingsMissing = bots.state === "unknown" && bots.max === 0 && !bots.enabled;
  const rows: Array<[string, string]> = [];
  const count = (n: number | null) => (n === null ? "unknown" : String(n));
  if (bots.characters !== null) rows.push(["Bot characters", count(bots.characters)], ["Bot accounts", count(bots.botAccounts)]);
  if (bots.online !== null) rows.push(["In the world now", count(bots.online)], ["Real players", count(bots.players)]);
  const restartNeeded = bots.restartNeeded || ws.server?.restartNeeded === true;
  const canRestart = ws.native && ws.recipe !== null && !ws.running;

  return (
    <div className="space-y-3 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <p className="min-w-0 text-sm text-muted">{summary.detail}</p>
        <StatusBadge status={summary.status} label={summary.label} className="shrink-0" />
      </div>
      {rows.length > 0 && <Facts rows={rows} />}
      {restartNeeded && (
        <div className="flex flex-wrap items-center gap-3 rounded-md border border-warn/40 bg-warn/10 p-3 text-xs text-warn">
          <span className="min-w-0 flex-1">The saved settings are not what the running servers were started with.</span>
          <Button className="h-8" onClick={ws.restartServer} busy={ws.activeAction === "restart"} disabled={!canRestart}>
            {ws.activeAction !== "restart" && <RotateCw className="size-3.5" aria-hidden />} Restart servers
          </Button>
        </div>
      )}
      <form
        className="space-y-2 rounded-md border border-line p-3"
        onSubmit={(event) => {
          event.preventDefault();
          void ws.saveBotSettings(choice);
        }}
      >
        <div className="text-sm font-medium">How many bots</div>
        <div className="flex flex-wrap items-center gap-4 text-xs text-muted">
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={choice.enabled} onChange={(event) => setChoice({ ...choice, enabled: event.target.checked })} />
            Playerbots on
          </label>
          <label className="flex items-center gap-2">
            At least
            <input
              type="number"
              min={0}
              max={1000}
              value={Number.isNaN(choice.min) ? "" : choice.min}
              onChange={(event) => setChoice({ ...choice, min: event.target.value === "" ? Number.NaN : Number(event.target.value) })}
              aria-label="Fewest bots online"
              className={cn(INPUT, "w-20")}
            />
          </label>
          <label className="flex items-center gap-2">
            at most
            <input
              type="number"
              min={0}
              max={1000}
              value={Number.isNaN(choice.max) ? "" : choice.max}
              onChange={(event) => setChoice({ ...choice, max: event.target.value === "" ? Number.NaN : Number(event.target.value) })}
              aria-label="Most bots online"
              className={cn(INPUT, "w-20")}
            />
            online
          </label>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <Button
            type="submit"
            variant="primary"
            className="h-8"
            busy={ws.activeAction === "bots"}
            disabled={!idle || settingsMissing || problem !== null || same}
            title={settingsMissing ? "Generate the configuration first." : (problem ?? (same ? "Nothing has changed." : undefined))}
          >
            {ws.activeAction !== "bots" && <Save className="size-3.5" aria-hidden />} Save bot settings
          </Button>
        </div>
        {problem && <p className="text-xs text-muted">{problem}</p>}
        {settingsMissing && <p className="text-xs text-muted">Generate the configuration first; that also chooses how many bots suit this machine.</p>}
        <p className="text-xs text-muted">
          Bots share the server&apos;s player limit ({bots.playerLimit || "not set yet"}), which follows the maximum. Saving rewrites the configuration; the servers pick it up
          when they are restarted.
        </p>
      </form>
    </div>
  );
}

export function PlayPage({ ws }: { ws: Worldstone }) {
  return (
    <div className="space-y-4">
      <ErrorBanner message={ws.error} onDismiss={ws.dismissError} />
      <TaskPanel task={ws.task} starting={ws.starting} onCancel={ws.cancel} />
      <Section title="Connect the game client">{ws.connect === null ? <Waiting ws={ws} /> : <ConnectDetails ws={ws} connect={ws.connect} />}</Section>
      <Section title="Accounts">{ws.accounts === null ? <Waiting ws={ws} /> : <AccountsDetails ws={ws} accounts={ws.accounts} />}</Section>
      <Section title="Playerbots">{ws.bots === null ? <Waiting ws={ws} /> : <BotsDetails ws={ws} bots={ws.bots} />}</Section>
    </div>
  );
}
