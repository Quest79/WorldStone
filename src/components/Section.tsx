import { cn } from "../lib/utils";

/** A titled panel. `danger` marks the one that can delete data. */
export function Section({
  title,
  hint,
  danger = false,
  children,
}: {
  title: string;
  /** One line under the title that says what the panel is for. */
  hint?: string;
  danger?: boolean;
  children: React.ReactNode;
}) {
  return (
    <section className={cn("overflow-hidden rounded-lg border bg-panel", danger ? "border-bad/50" : "border-line")}>
      <div className={cn("border-b px-4 py-3", danger ? "border-bad/30" : "border-line")}>
        <h2 className={cn("text-sm font-semibold", danger && "text-bad")}>{title}</h2>
        {hint && <p className="mt-0.5 text-xs text-muted">{hint}</p>}
      </div>
      {children}
    </section>
  );
}

/** What a panel says while its status has not arrived, or when it cannot be read outside the desktop app. */
export function Waiting({ native }: { native: boolean }) {
  return <p className="px-4 py-3 text-sm text-muted">{native ? "Checking…" : "Available in the desktop app."}</p>;
}
