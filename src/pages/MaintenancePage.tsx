import { Trash2 } from "lucide-react";
import { useEffect } from "react";
import type { ImageRow } from "../api/types";
import type { Worldstone } from "../hooks/useWorldstone";
import { Button } from "../components/Button";
import { ErrorBanner } from "../components/ErrorBanner";
import { Section, Waiting } from "../components/Section";
import { TaskPanel } from "../components/TaskPanel";
import { removeLabel, summarizeImages } from "../lib/images";
import { formatBytes } from "../lib/summary";
import { cn } from "../lib/utils";
import { BackupsSection } from "./maintenance/BackupsSection";
import { RepairSection } from "./maintenance/RepairSection";
import { ResetSection } from "./maintenance/ResetSection";
import { ServicesSection } from "./maintenance/ServicesSection";
import { UpdateSection } from "./maintenance/UpdateSection";

/** How often the service table is read while the page is open. Docker is asked for CPU and memory, which takes a moment. */
const DASHBOARD_MS = 5000;

const KIND_STYLE: Record<ImageRow["kind"], string> = {
  current: "bg-ok/15 text-ok",
  old: "bg-warn/15 text-warn",
  database: "bg-accent/15 text-accent",
};

function ImageItem({ image }: { image: ImageRow }) {
  return (
    <li className="space-y-1 px-4 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className={cn("rounded px-2 py-0.5 text-xs font-medium", KIND_STYLE[image.kind])}>{image.title}</span>
        <span className="font-mono text-sm">{image.reference}</span>
        <span className="text-xs text-muted">
          image {image.id} · {formatBytes(image.sizeBytes)}
        </span>
        <span className={cn("ml-auto text-xs", image.usedBy.length > 0 ? "text-ok" : "text-muted")}>
          {image.usedBy.length > 0 ? `Running: ${image.usedBy.join(", ")}` : "Not running"}
        </span>
      </div>
      <p className="text-xs text-muted">{image.what}</p>
    </li>
  );
}

function ImagesDetails({ ws }: { ws: Worldstone }) {
  const summary = summarizeImages(ws.images);
  const idle = ws.native && ws.recipe !== null && !ws.running;
  return (
    <div>
      {ws.images === null ? (
        <Waiting native={ws.native} />
      ) : summary.note ? (
        <p className="px-4 py-3 text-sm text-muted">{summary.note}</p>
      ) : summary.rows.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted">No image has been built or downloaded yet.</p>
      ) : (
        <ul className="divide-y divide-line">
          {summary.rows.map((image) => (
            <ImageItem key={image.reference} image={image} />
          ))}
        </ul>
      )}
      <div className="space-y-2 border-t border-line px-4 py-3">
        <div className="flex flex-wrap items-center gap-3">
          <Button
            onClick={ws.removeOldImages}
            busy={ws.activeAction === "images"}
            disabled={!idle || summary.removeBlocked !== null}
            title={summary.removeBlocked ?? undefined}
          >
            {ws.activeAction !== "images" && <Trash2 className="size-4" aria-hidden />}
            {removeLabel(summary.removable)}
          </Button>
          {summary.removeBlocked && <span className="text-xs text-muted">{summary.removeBlocked}</span>}
        </div>
        <p className="text-xs text-muted">
          Each server has its own image, named after the edition and the server. Only old images that no container uses can be removed; the
          current images, the database server and your data are never touched.
        </p>
      </div>
    </div>
  );
}

export function MaintenancePage({ ws }: { ws: Worldstone }) {
  const { native, recipe, running, refreshBackups, refreshDashboard, refreshRepair, refreshReset, refreshImages } = ws;
  const serverState = ws.server?.state;
  const recipeId = recipe?.id ?? null;

  // These are read when the page opens, when an operation ends and when the servers change state.
  // Not while an operation runs: they would describe a moment that is about to be out of date.
  useEffect(() => {
    if (!native || recipeId === null || running) return;
    void refreshBackups();
    void refreshRepair();
    void refreshReset();
    void refreshImages();
  }, [native, recipeId, running, serverState, refreshBackups, refreshRepair, refreshReset, refreshImages]);

  // The service table is live, so it is read on a timer for as long as the page is open.
  useEffect(() => {
    if (!native || recipeId === null) return;
    void refreshDashboard();
    const timer = setInterval(() => void refreshDashboard(), DASHBOARD_MS);
    return () => clearInterval(timer);
  }, [native, recipeId, refreshDashboard, running]);

  return (
    <div className="space-y-4">
      <ErrorBanner message={ws.error} onDismiss={ws.dismissError} />
      <TaskPanel task={ws.task} starting={ws.starting} onCancel={ws.cancel} />
      <ServicesSection ws={ws} />
      <UpdateSection ws={ws} />
      <BackupsSection ws={ws} />
      <RepairSection ws={ws} />
      <Section title="Docker images">
        <ImagesDetails ws={ws} />
      </Section>
      <ResetSection ws={ws} />
    </div>
  );
}
