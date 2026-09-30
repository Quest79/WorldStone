import { Type, X, type LucideIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { AppearanceApi } from "../../hooks/useAppearance";
import { cn } from "../../lib/utils";
import { FontsTab } from "./FontsTab";

/**
 * The settings tabs. A new kind of setting is one entry here and one component; there are no
 * entries for settings that do not exist yet.
 */
const TABS: Array<{ id: string; label: string; icon: LucideIcon; render: (ap: AppearanceApi) => React.ReactNode }> = [
  { id: "fonts", label: "Fonts", icon: Type, render: (ap) => <FontsTab ap={ap} /> },
];

/** The settings window: a tab rail on the left and the chosen tab on the right. Esc, the X and a click outside close it. */
export function SettingsDialog({ ap, onClose }: { ap: AppearanceApi; onClose: () => void }) {
  const [tabId, setTabId] = useState(TABS[0].id);
  const tab = TABS.find((t) => t.id === tabId) ?? TABS[0];
  const panel = useRef<HTMLDivElement>(null);
  // The caller may pass a new function every render; the window must not re-focus itself when it does.
  const close = useRef(onClose);
  close.current = onClose;

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    panel.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close.current();
      if (e.key === "Tab" && panel.current) {
        // Keep focus inside the window while it is open.
        const items = [...panel.current.querySelectorAll<HTMLElement>('button, input, select, textarea, [href], [tabindex]:not([tabindex="-1"])')].filter((el) => !el.hasAttribute("disabled"));
        if (items.length === 0) return;
        const first = items[0];
        const last = items[items.length - 1];
        if (e.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      opener?.focus();
    };
  }, []);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        tabIndex={-1}
        className="flex h-[min(46rem,100%)] w-[min(60rem,100%)] flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-2xl outline-none"
      >
        <header className="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 id="settings-title" className="text-base font-semibold">
            Settings
          </h2>
          <button type="button" onClick={onClose} aria-label="Close settings" title="Close" className="rounded-md p-1.5 text-muted hover:bg-panel-2 hover:text-fg">
            <X className="size-4" aria-hidden />
          </button>
        </header>
        <div className="flex min-h-0 flex-1">
          <nav aria-label="Settings" className="w-44 shrink-0 space-y-1 border-r border-line p-2">
            {TABS.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                type="button"
                onClick={() => setTabId(id)}
                aria-current={tab.id === id ? "page" : undefined}
                className={cn(
                  "flex h-9 w-full items-center gap-2.5 rounded-md px-3 text-sm transition-colors",
                  tab.id === id ? "bg-accent/15 text-accent" : "text-muted hover:bg-panel-2 hover:text-fg",
                )}
              >
                <Icon className="size-4" aria-hidden /> {label}
              </button>
            ))}
          </nav>
          <div className="min-w-0 flex-1 overflow-auto p-5">{tab.render(ap)}</div>
        </div>
      </div>
    </div>
  );
}
