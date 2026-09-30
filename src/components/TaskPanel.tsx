import { CircleCheck, CircleX, Square } from "lucide-react";
import type { TaskEvent } from "../api/types";
import { Button } from "./Button";

function Bar({ percent }: { percent: number | null }) {
  return (
    <div
      className="h-2 overflow-hidden rounded-full bg-panel-2"
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent ?? undefined}
      aria-label={percent === null ? "Working" : `${Math.round(percent)} percent`}
    >
      {percent === null ? (
        <div className="indeterminate-bar h-full rounded-full bg-accent" />
      ) : (
        <div className="h-full rounded-full bg-accent transition-[width]" style={{ width: `${percent}%` }} />
      )}
    </div>
  );
}

export function TaskPanel({
  task,
  starting,
  onCancel,
}: {
  task: TaskEvent | null;
  starting: boolean;
  onCancel: () => void;
}) {
  const active = task?.state === "running";

  let body;
  if (starting && !active) {
    body = (
      <>
        <div className="text-sm font-medium">Starting…</div>
        <Bar percent={null} />
      </>
    );
  } else if (!task) {
    body = <p className="text-sm text-muted">No operation running.</p>;
  } else if (active) {
    const p = task.progress;
    body = (
      <>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="truncate text-sm font-medium">{task.label}</div>
            {task.step && <div className="truncate text-xs text-muted">{task.step}</div>}
          </div>
          {task.cancellable && (
            <Button onClick={onCancel} className="h-8 shrink-0">
              <Square className="size-3.5" aria-hidden /> Cancel
            </Button>
          )}
        </div>
        <Bar percent={p ? p.percent : null} />
        <div className="text-xs text-muted">{p ? `${p.label}: ${Math.round(p.percent)}%` : "Working…"}</div>
      </>
    );
  } else if (task.state === "succeeded") {
    body = (
      <div className="space-y-0.5">
        <div className="flex items-center gap-2 text-sm text-ok">
          <CircleCheck className="size-4" aria-hidden /> Finished: {task.label}
        </div>
        {task.message && <p className="pl-6 text-xs text-muted">{task.message}</p>}
      </div>
    );
  } else if (task.state === "cancelled") {
    body = <div className="text-sm text-muted">Cancelled: {task.label}</div>;
  } else {
    body = (
      <div className="flex items-start gap-2 text-sm text-bad">
        <CircleX className="mt-0.5 size-4 shrink-0" aria-hidden />
        <span>{task.message ?? "The operation failed. See the log for details."}</span>
      </div>
    );
  }

  return (
    <section className="space-y-2.5 rounded-lg border border-line bg-panel p-4" aria-live="polite">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-muted">Current operation</h2>
      {body}
    </section>
  );
}
