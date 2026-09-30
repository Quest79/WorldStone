import { RotateCcw } from "lucide-react";
import { useState } from "react";
import type { FontSettings, FontStyle } from "../../api/types";
import type { AppearanceApi } from "../../hooks/useAppearance";
import { FONT_ROLES, sameStyle, textStyle } from "../../lib/appearance";
import { cn } from "../../lib/utils";
import { Button } from "../Button";
import { FontPicker } from "./FontPicker";
import { TextEffectsEditor } from "./TextEffectsEditor";

function SaveNote({ ap }: { ap: AppearanceApi }) {
  let text = "";
  if (ap.saveState === "saving") text = "Saving…";
  else if (ap.saveState === "saved") text = "Saved";
  else if (ap.saveState === "error") text = `Not saved: ${ap.saveError}`;
  return (
    <span aria-live="polite" className={cn("text-xs", ap.saveState === "error" ? "text-bad" : "text-muted")}>
      {text}
    </span>
  );
}

/**
 * The Fonts tab. Each role in `FONT_ROLES` gets the same controls: which font, an outline and a
 * shadow. Changes show at once everywhere and are saved as they are made.
 */
export function FontsTab({ ap }: { ap: AppearanceApi }) {
  const [role, setRole] = useState<keyof FontSettings>(FONT_ROLES[0].id);

  if (!ap.native) return <p className="text-sm text-muted">Fonts are chosen and saved in the desktop app.</p>;
  if (!ap.current || !ap.defaults) return <p className="text-sm text-muted">Reading your settings…</p>;

  const meta = FONT_ROLES.find((r) => r.id === role) ?? FONT_ROLES[0];
  const style = ap.current.fonts[meta.id];
  const standard = ap.defaults.fonts[meta.id];
  const change = (fn: (s: FontStyle) => FontStyle) => ap.update(meta.id, fn);

  return (
    <div className="space-y-4">
      {ap.problem && (
        <p role="alert" className="rounded-md border border-warn/40 bg-warn/10 p-2.5 text-xs text-warn">
          {ap.problem}
        </p>
      )}

      <div role="tablist" aria-label="Font role" className="flex flex-wrap gap-1 border-b border-line">
        {FONT_ROLES.map((r) => (
          <button
            key={r.id}
            id={`font-role-${r.id}`}
            type="button"
            role="tab"
            aria-selected={r.id === meta.id}
            aria-controls="font-role-panel"
            onClick={() => setRole(r.id)}
            className={cn(
              "-mb-px h-9 border-b-2 px-3 text-sm transition-colors",
              r.id === meta.id ? "border-accent text-fg" : "border-transparent text-muted hover:text-fg",
            )}
          >
            {r.label}
          </button>
        ))}
      </div>

      <div id="font-role-panel" role="tabpanel" aria-labelledby={`font-role-${meta.id}`} className="space-y-4">
        <div className="flex items-start justify-between gap-3">
          <p className="text-sm text-muted">{meta.hint}</p>
          <div className="flex shrink-0 items-center gap-2">
            <SaveNote ap={ap} />
            <Button
              variant="ghost"
              className="h-8 px-2 text-xs"
              disabled={sameStyle(style, standard)}
              title={sameStyle(style, standard) ? "Already the standard: Segoe UI with no outline or shadow." : "Go back to Segoe UI with no outline or shadow."}
              onClick={() => ap.reset(meta.id)}
            >
              <RotateCcw className="size-3.5" aria-hidden /> Reset
            </Button>
          </div>
        </div>

        <div className="rounded-lg border border-line bg-bg p-4" aria-label={`Preview of the ${meta.label.toLowerCase()} font`}>
          <p className="mb-2 text-[11px] font-medium uppercase tracking-wide text-muted">Preview</p>
          <div className="space-y-1" style={textStyle(style)}>
            {meta.samples.map((sample, i) => (
              <p key={sample} className={i === 0 ? "text-2xl font-semibold leading-tight" : "text-sm"}>
                {sample}
              </p>
            ))}
          </div>
          <p className="mt-2 text-xs text-muted">
            {style.family} · {style.source === "google" ? "Google Fonts" : "installed"}
          </p>
        </div>

        <div className="grid gap-4 md:grid-cols-2">
          <FontPicker role={meta.label.toLowerCase()} style={style} ap={ap} onPick={(source, family) => change((s) => ({ ...s, source, family }))} />
          <TextEffectsEditor
            outline={style.outline}
            shadow={style.shadow}
            onOutline={(o) => change((s) => ({ ...s, outline: { ...s.outline, ...o } }))}
            onShadow={(o) => change((s) => ({ ...s, shadow: { ...s.shadow, ...o } }))}
          />
        </div>
      </div>
    </div>
  );
}
