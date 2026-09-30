import { LoaderCircle } from "lucide-react";
import type { ButtonHTMLAttributes } from "react";
import { cn } from "../lib/utils";

type Variant = "primary" | "secondary" | "ghost" | "danger";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  /** Shows a spinner and disables the button: the click was accepted, work is under way. */
  busy?: boolean;
}

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg hover:bg-accent/90 disabled:bg-panel-2 disabled:text-muted",
  secondary: "border border-line bg-panel-2 text-fg hover:bg-line/60 disabled:text-muted",
  ghost: "text-muted hover:bg-panel-2 hover:text-fg disabled:opacity-50",
  danger: "bg-bad text-white hover:bg-bad/90 disabled:bg-panel-2 disabled:text-muted",
};

export function Button({ variant = "secondary", busy = false, className, children, disabled, ...rest }: Props) {
  return (
    <button
      type="button"
      className={cn(
        "inline-flex h-9 items-center justify-center gap-2 rounded-md px-3.5 text-sm font-medium transition-colors",
        VARIANTS[variant],
        className,
      )}
      disabled={disabled || busy}
      {...rest}
    >
      {busy && <LoaderCircle className="size-4 animate-spin" aria-hidden />}
      {children}
    </button>
  );
}
