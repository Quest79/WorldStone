import { X } from "lucide-react";

/** The last failed action, with the backend's own explanation. Shown on every page that can start one. */
export function ErrorBanner({ message, onDismiss }: { message: string | null; onDismiss: () => void }) {
  if (!message) return null;
  return (
    <div role="alert" className="flex items-start justify-between gap-3 rounded-lg border border-bad/40 bg-bad/10 p-3 text-sm text-bad">
      <span className="min-w-0 break-words">{message}</span>
      <button type="button" onClick={onDismiss} aria-label="Dismiss" className="shrink-0 hover:text-fg">
        <X className="size-4" />
      </button>
    </div>
  );
}
