import { Archive, FolderOpen, History } from "lucide-react";
import { useState } from "react";
import type { BackupSet } from "../../api/types";
import type { Worldstone } from "../../hooks/useWorldstone";
import { Button } from "../../components/Button";
import { Section, Waiting } from "../../components/Section";
import { backupBlocked, databasesIn, describeSet, restoreBlocked, restoreWarning, setTitle } from "../../lib/operations";
import { CHECKING } from "../../lib/summary";

function SetRow({ ws, set, restoreWhy }: { ws: Worldstone; set: BackupSet; restoreWhy: string | null }) {
  const [confirming, setConfirming] = useState(false);
  const idle = ws.native && ws.recipe !== null && !ws.running;
  return (
    <li className="space-y-2 px-4 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-mono text-sm">{setTitle(set)}</span>
        <span className="text-xs text-muted">{describeSet(set)}</span>
        <span className="ml-auto">
          {!confirming && (
            <Button
              onClick={() => setConfirming(true)}
              disabled={!idle || restoreWhy !== null || !set.files.length}
              title={restoreWhy ?? undefined}
              className="h-8"
            >
              <History className="size-4" aria-hidden /> Restore…
            </Button>
          )}
        </span>
      </div>
      <p className="text-xs text-muted">
        {set.what}. Holds {databasesIn(set).join(", ")}.
      </p>
      {confirming && (
        <div className="space-y-2 rounded-md border border-bad/40 bg-bad/5 p-3">
          <p className="text-sm">{restoreWarning(set)}</p>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="danger"
              busy={ws.activeAction === "restoreBackup"}
              disabled={!idle}
              onClick={() => {
                setConfirming(false);
                void ws.restoreBackup(set.id);
              }}
            >
              Replace with this backup
            </Button>
            <Button onClick={() => setConfirming(false)}>Keep what I have</Button>
          </div>
        </div>
      )}
    </li>
  );
}

/** Making, listing and restoring database backups. */
export function BackupsSection({ ws }: { ws: Worldstone }) {
  const backups = ws.backups;
  const makeWhy = backupBlocked(backups);
  const restoreWhy = restoreBlocked(backups);
  const idle = ws.native && ws.recipe !== null && !ws.running;
  return (
    <Section title="Backups" hint="Copies of every database (accounts, characters and the world), kept in the Backups folder.">
      {backups === null ? (
        <Waiting native={ws.native} />
      ) : (
        <div>
          <div className="flex flex-wrap items-center gap-3 border-b border-line px-4 py-3">
            <Button onClick={ws.createBackup} busy={ws.activeAction === "backup"} disabled={!idle || makeWhy !== null} title={makeWhy ?? undefined}>
              {ws.activeAction !== "backup" && <Archive className="size-4" aria-hidden />}
              Back up now
            </Button>
            <Button onClick={ws.openBackupsFolder} disabled={!ws.native || ws.recipe === null}>
              <FolderOpen className="size-4" aria-hidden /> Open folder
            </Button>
            {makeWhy && makeWhy !== CHECKING && <span className="text-xs text-muted">{makeWhy}</span>}
          </div>
          {backups.note && <p className="px-4 py-3 text-sm text-bad">{backups.note}</p>}
          {backups.sets.length === 0 ? (
            <p className="px-4 py-3 text-sm text-muted">No backup has been made yet.</p>
          ) : (
            <>
              {restoreWhy && restoreWhy !== CHECKING && <p className="border-b border-line px-4 py-2 text-xs text-muted">Restoring is unavailable: {restoreWhy}</p>}
              <ul className="divide-y divide-line">
                {backups.sets.map((set) => (
                  <SetRow key={set.id} ws={ws} set={set} restoreWhy={restoreWhy} />
                ))}
              </ul>
            </>
          )}
          <p className="border-t border-line px-4 py-2 text-xs text-muted">
            Backups are also taken automatically before a database update, a restore and a reset, and they are never deleted by the app. A restore
            needs the servers stopped, checks every file first, and takes a backup of what it replaces.
          </p>
        </div>
      )}
    </Section>
  );
}
