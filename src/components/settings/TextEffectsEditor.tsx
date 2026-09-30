import { useId } from "react";
import type { TextOutline, TextShadow } from "../../api/types";

/** A slider with its label and current value, so the number is never a guess. */
function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  unit = "",
  disabled,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  unit?: string;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  const id = useId();
  return (
    <div className="grid grid-cols-[4.5rem_1fr_3.25rem] items-center gap-2">
      <label htmlFor={id} className="text-xs text-muted">
        {label}
      </label>
      <input
        id={id}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
        className="h-1.5 w-full accent-accent disabled:opacity-40"
      />
      <output htmlFor={id} className="text-right text-xs tabular-nums text-muted">
        {value}
        {unit}
      </output>
    </div>
  );
}

function ColourField({ label, value, disabled, onChange }: { label: string; value: string; disabled: boolean; onChange: (value: string) => void }) {
  const id = useId();
  return (
    <div className="grid grid-cols-[4.5rem_1fr_3.25rem] items-center gap-2">
      <label htmlFor={id} className="text-xs text-muted">
        {label}
      </label>
      <input
        id={id}
        type="color"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        className="h-6 w-12 cursor-pointer rounded border border-line bg-panel-2 p-0.5 disabled:cursor-not-allowed disabled:opacity-40"
      />
      <span className="text-right font-mono text-[11px] uppercase text-muted">{value}</span>
    </div>
  );
}

/** One effect: a switch and the controls it opens. Controls are disabled while the effect is off, and say why. */
function Effect({ title, enabled, onToggle, children }: { title: string; enabled: boolean; onToggle: (on: boolean) => void; children: React.ReactNode }) {
  const id = useId();
  return (
    <fieldset className="rounded-md border border-line bg-panel-2/40 p-3">
      <legend className="sr-only">{title}</legend>
      <div className="mb-2 flex items-center justify-between">
        <label htmlFor={id} className="flex cursor-pointer items-center gap-2 text-sm font-medium">
          <input id={id} type="checkbox" checked={enabled} onChange={(e) => onToggle(e.target.checked)} className="size-4 accent-accent" />
          {title}
        </label>
        {!enabled && <span className="text-xs text-muted">Off: tick to change</span>}
      </div>
      <div className="space-y-1.5" title={enabled ? undefined : `Turn the ${title.toLowerCase()} on to change these.`}>
        {children}
      </div>
    </fieldset>
  );
}

/** The outline and shadow controls. Every font role uses this same editor. */
export function TextEffectsEditor({
  outline,
  shadow,
  onOutline,
  onShadow,
}: {
  outline: TextOutline;
  shadow: TextShadow;
  onOutline: (change: Partial<TextOutline>) => void;
  onShadow: (change: Partial<TextShadow>) => void;
}) {
  const o = !outline.enabled;
  const s = !shadow.enabled;
  return (
    <div className="space-y-3">
      <Effect title="Outline" enabled={outline.enabled} onToggle={(on) => onOutline({ enabled: on })}>
        <Slider label="Width" value={outline.width} min={0} max={8} step={0.5} unit="px" disabled={o} onChange={(width) => onOutline({ width })} />
        <ColourField label="Colour" value={outline.color} disabled={o} onChange={(color) => onOutline({ color })} />
        <Slider label="Opacity" value={outline.opacity} min={0} max={100} unit="%" disabled={o} onChange={(opacity) => onOutline({ opacity })} />
      </Effect>
      <Effect title="Shadow" enabled={shadow.enabled} onToggle={(on) => onShadow({ enabled: on })}>
        <Slider label="Across" value={shadow.x} min={-40} max={40} unit="px" disabled={s} onChange={(x) => onShadow({ x })} />
        <Slider label="Down" value={shadow.y} min={-40} max={40} unit="px" disabled={s} onChange={(y) => onShadow({ y })} />
        <Slider label="Blur" value={shadow.blur} min={0} max={60} unit="px" disabled={s} onChange={(blur) => onShadow({ blur })} />
        <ColourField label="Colour" value={shadow.color} disabled={s} onChange={(color) => onShadow({ color })} />
        <Slider label="Opacity" value={shadow.opacity} min={0} max={100} unit="%" disabled={s} onChange={(opacity) => onShadow({ opacity })} />
      </Effect>
    </div>
  );
}
