import { RefreshCw } from "lucide-react";
import type { Worldstone } from "../../hooks/useWorldstone";
import { Button } from "../../components/Button";
import { Section } from "../../components/Section";
import { updateBlocked } from "../../lib/operations";
import { CHECKING } from "../../lib/summary";

/** Pull the newest source, rebuild, apply database updates and restart, in one operation. */
export function UpdateSection({ ws }: { ws: Worldstone }) {
  const why = updateBlocked({ repos: ws.repos, build: ws.build, database: ws.database });
  const idle = ws.native && ws.recipe !== null && !ws.running;
  return (
    <Section title="Update" hint="Bring the server up to date with the newest source.">
      <div className="space-y-3 px-4 py-3">
        <div className="flex flex-wrap items-center gap-3">
          <Button variant="primary" onClick={ws.updateServer} busy={ws.activeAction === "update"} disabled={!idle || why !== null} title={why ?? undefined}>
            {ws.activeAction !== "update" && <RefreshCw className="size-4" aria-hidden />}
            Update
          </Button>
          {why && why !== CHECKING && <span className="text-xs text-muted">{why}</span>}
        </div>
        <ol className="list-decimal space-y-0.5 pl-5 text-xs text-muted">
          <li>Everything is checked first, so a problem stops the update before anything changes.</li>
          <li>The newest source is downloaded. Your local edits to it are never overwritten.</li>
          <li>The server is rebuilt only if the source changed, while the running servers stay up.</li>
          <li>If the database has updates waiting, the servers are stopped, every database is backed up, and the updates are applied.</li>
          <li>Servers that were running are started again on the new version. Servers that were stopped stay stopped, and if nothing changed nothing is restarted.</li>
        </ol>
      </div>
    </Section>
  );
}
