import { Trash2 } from "lucide-react";
import { useState } from "react";
import type { Worldstone } from "../../hooks/useWorldstone";
import { Button } from "../../components/Button";
import { Section, Waiting } from "../../components/Section";
import { resetBlocked, resetTargets } from "../../lib/operations";
import { formatSize } from "../../lib/format";
import { CHECKING } from "../../lib/summary";
import { cn } from "../../lib/utils";

/** The only place in the app that deletes data. Typing the instance's id is what enables it. */
export function ResetSection({ ws }: { ws: Worldstone }) {
  const preview = ws.resetPreview;
  const [typed, setTyped] = useState("");
  const [gameData, setGameData] = useState(false);
  const idle = ws.native && ws.recipe !== null && !ws.running;
  const why = resetBlocked(preview, gameData, typed);
  const gameDataItem = preview?.items.find((i) => i.id === "gameData");
  const targets = resetTargets(preview, gameData);

  return (
    <Section
      danger
      title="Reset this server"
      hint="Deletes the database and the generated files and returns the server to how it was before it had a database."
    >
      {preview === null ? (
        <Waiting native={ws.native} />
      ) : (
        <div className="space-y-3 px-4 py-3">
          <ul className="space-y-1.5">
            {preview.items.map((item) => (
              <li key={item.id} className={cn("text-sm", !item.exists && "text-muted line-through decoration-muted/50")}>
                <span className="font-medium">{item.label}</span>
                {item.bytes !== null && item.exists && <span className="text-xs text-muted"> · {formatSize(item.bytes)}</span>}
                {!item.exists && <span className="text-xs no-underline"> · nothing there</span>}
                <p className="text-xs text-muted no-underline">{item.detail}</p>
              </li>
            ))}
          </ul>
          <p className="text-xs text-muted">
            <span className="font-medium text-fg">Never deleted:</span> {preview.kept.join("; ").toLowerCase()}.
          </p>
          {gameDataItem?.exists && (
            <label className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={gameData} onChange={(e) => setGameData(e.target.checked)} className="accent-bad" />
              Also delete the extracted game data
            </label>
          )}
          <div className="flex flex-wrap items-center gap-3">
            <input
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              placeholder={preview.phrase}
              aria-label={`Type ${preview.phrase} to confirm the reset`}
              autoComplete="off"
              spellCheck={false}
              className="h-9 w-56 rounded-md border border-line bg-bg px-2.5 font-mono text-sm text-fg placeholder:text-muted"
            />
            <Button
              variant="danger"
              busy={ws.activeAction === "reset"}
              disabled={!idle || why !== null}
              title={why ?? undefined}
              onClick={() => {
                const confirmation = typed;
                setTyped("");
                setGameData(false);
                void ws.resetInstance(confirmation, gameData);
              }}
            >
              {ws.activeAction !== "reset" && <Trash2 className="size-4" aria-hidden />}
              {targets.length > 0 ? `Delete ${targets.length} thing${targets.length === 1 ? "" : "s"} and reset` : "Reset"}
            </Button>
            {why && why !== CHECKING && <span className="text-xs text-muted">{why}</span>}
          </div>
          <p className="text-xs text-muted">
            The servers are stopped first so the world is saved, then every database is backed up to the Backups folder. If that backup cannot be
            made, nothing is deleted. After a reset, set the server up again from the Setup page, and restore a backup if you want your data back.
          </p>
        </div>
      )}
    </Section>
  );
}
