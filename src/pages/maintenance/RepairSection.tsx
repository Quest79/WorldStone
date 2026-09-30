import { Wrench } from "lucide-react";
import type { Worldstone } from "../../hooks/useWorldstone";
import { Button } from "../../components/Button";
import { Section, Waiting } from "../../components/Section";
import { StatusBadge } from "../../components/StatusBadge";
import { repairLabel, summarizeRepair } from "../../lib/operations";
import { CHECKING } from "../../lib/summary";

/** Finds what is wrong with the instance and puts right what the app can, using the same steps as Setup. */
export function RepairSection({ ws }: { ws: Worldstone }) {
  const repair = ws.repair;
  const summary = summarizeRepair(repair);
  const idle = ws.native && ws.recipe !== null && !ws.running;
  return (
    <Section title="Repair" hint="Checks every part of the server and puts right what is missing or broken, without deleting anything.">
      {repair === null ? (
        <Waiting native={ws.native} />
      ) : (
        <div>
          <div className="flex flex-wrap items-center gap-3 px-4 py-3">
            <StatusBadge status={summary.status} label={summary.headline} />
            <Button
              onClick={ws.repairInstance}
              busy={ws.activeAction === "repair"}
              disabled={!idle || summary.blocked !== null}
              title={summary.blocked ?? undefined}
              className="ml-auto"
            >
              {ws.activeAction !== "repair" && <Wrench className="size-4" aria-hidden />}
              {repairLabel(summary, repair.findings.length)}
            </Button>
          </div>
          {repair.findings.length > 0 && (
            <ul className="divide-y divide-line border-t border-line">
              {repair.findings.map((finding) => (
                <li key={finding.title} className="space-y-0.5 px-4 py-3">
                  <div className="text-sm font-medium">{finding.title}</div>
                  <p className="text-xs text-muted">{finding.detail}</p>
                  <p className="text-xs text-accent">{finding.action ? `Repair will: ${finding.action.toLowerCase()}.` : "Only you can put this right."}</p>
                </li>
              ))}
            </ul>
          )}
          {summary.blocked && summary.blocked !== CHECKING && repair.findings.length > 0 && (
            <p className="border-t border-line px-4 py-2 text-xs text-muted">{summary.blocked}</p>
          )}
          <p className="border-t border-line px-4 py-2 text-xs text-muted">
            Repair rebuilds what is missing, completes a database that is missing tables (it never resets one), generates the configuration again,
            removes backup files that were never finished, and restarts servers that crashed. It checks again afterwards and says if something is still wrong.
            A server that is merely out of date is for Update.
          </p>
        </div>
      )}
    </Section>
  );
}
