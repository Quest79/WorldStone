import type { Worldstone } from "../../hooks/useWorldstone";
import { Section, Waiting } from "../../components/Section";
import { StatusBadge } from "../../components/StatusBadge";
import { describeRow } from "../../lib/operations";

/** Every container of the instance: its state, address, how long it has been up, and what it uses right now. */
export function ServicesSection({ ws }: { ws: Worldstone }) {
  const dashboard = ws.dashboard;
  return (
    <Section title="Services" hint="What each part of the server is doing right now, read from Docker every few seconds.">
      {dashboard === null ? (
        <Waiting native={ws.native} />
      ) : (
        <div>
          {dashboard.rows.length === 0 ? (
            <p className="px-4 py-3 text-sm text-muted">{dashboard.note ?? "There are no services to show."}</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[40rem] text-left text-sm">
                <thead className="text-xs text-muted">
                  <tr className="border-b border-line">
                    <th className="px-4 py-2 font-medium">Service</th>
                    <th className="px-2 py-2 font-medium">State</th>
                    <th className="px-2 py-2 font-medium">Address</th>
                    <th className="px-2 py-2 font-medium">Up for</th>
                    <th className="px-2 py-2 font-medium">Processor</th>
                    <th className="px-4 py-2 font-medium">Memory</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-line">
                  {dashboard.rows.map((row) => {
                    const line = describeRow(row, dashboard.host);
                    return (
                      <tr key={line.id}>
                        <td className="px-4 py-2">
                          <div className="font-medium">{line.label}</div>
                          <div className="font-mono text-xs text-muted">{line.container}</div>
                        </td>
                        <td className="px-2 py-2">
                          <StatusBadge status={line.tone} label={line.state} />
                        </td>
                        <td className="px-2 py-2 font-mono text-xs">{line.port}</td>
                        <td className="px-2 py-2 tabular-nums">{line.uptime}</td>
                        <td className="px-2 py-2 tabular-nums">{line.cpu}</td>
                        <td className="px-4 py-2 tabular-nums">{line.memory}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
          {dashboard.rows.length > 0 && dashboard.note && <p className="border-t border-line px-4 py-2 text-xs text-muted">{dashboard.note}</p>}
          <p className="border-t border-line px-4 py-2 text-xs text-muted">
            Processor is a share of one core, so a busy server can pass 100%. A dash means Docker gave no figure, usually because the container is not running.
          </p>
        </div>
      )}
    </Section>
  );
}
