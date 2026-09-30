import type { LucideIcon } from "lucide-react";
import type { Status } from "../api/types";
import { StatusBadge } from "./StatusBadge";

export function StatusCard({
  icon: Icon,
  title,
  status,
  detail,
  label,
}: {
  icon: LucideIcon;
  title: string;
  status: Status;
  detail: string;
  /** Replaces the generic word for the status, such as "Running" for a ready server. */
  label?: string;
}) {
  return (
    <div className="min-w-0 rounded-lg border border-line bg-panel px-3 py-2.5">
      <div className="flex items-center gap-2">
        <Icon className="size-4 shrink-0 text-muted" aria-hidden />
        <span className="truncate text-sm text-muted">{title}</span>
        <StatusBadge status={status} label={label} className="ml-auto shrink-0" />
      </div>
      <p className="mt-1.5 line-clamp-2 text-xs text-muted" title={detail}>
        {detail}
      </p>
    </div>
  );
}
