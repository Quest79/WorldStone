import { CircleAlert, CircleCheck, CircleHelp, CircleX, TriangleAlert, type LucideIcon } from "lucide-react";
import type { Status } from "../api/types";
import { cn } from "../lib/utils";

/** One place that decides what every status looks like, so colours mean the same everywhere. */
export const STATUS_STYLE: Record<Status, { label: string; icon: LucideIcon; text: string; dot: string }> = {
  ready: { label: "Ready", icon: CircleCheck, text: "text-ok", dot: "bg-ok" },
  missing: { label: "Missing", icon: CircleX, text: "text-bad", dot: "bg-bad" },
  warning: { label: "Attention", icon: TriangleAlert, text: "text-warn", dot: "bg-warn" },
  error: { label: "Error", icon: CircleAlert, text: "text-bad", dot: "bg-bad" },
  unknown: { label: "Unknown", icon: CircleHelp, text: "text-idle", dot: "bg-idle" },
};

export function StatusBadge({ status, label, className }: { status: Status; label?: string; className?: string }) {
  const style = STATUS_STYLE[status];
  const Icon = style.icon;
  return (
    <span className={cn("inline-flex items-center gap-1.5 text-sm font-medium", style.text, className)}>
      <Icon className="size-4" aria-hidden />
      {label ?? style.label}
    </span>
  );
}
