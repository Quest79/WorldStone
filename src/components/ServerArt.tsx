import type { ReactNode } from "react";
import type { Backdrop } from "../lib/serverTheme";

// Scenes drawn behind a server's button, one per expansion. They are plain shapes in
// `currentColor` (the button sets it to the recipe's accent), so they follow the theme and need
// no bundled artwork. Each is 200 x 64; the button shows the right-hand part and fades the rest.

/** Azeroth: hills under a low sun, a keep on the far ridge, birds. */
function Azeroth() {
  return (
    <>
      <defs>
        <radialGradient id="sv-azeroth-sun">
          <stop offset="0" stopColor="currentColor" stopOpacity=".5" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle cx="112" cy="30" r="26" fill="url(#sv-azeroth-sun)" />
      <circle cx="112" cy="30" r="8" fill="currentColor" opacity=".5" />
      <path d="M128 10q3-4 6 0q3-4 6 0M146 18q2.5-3.5 5 0q2.5-3.5 5 0" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" opacity=".45" />
      <path d="M0 46L24 32L44 42L76 26L100 40L128 34L156 44L200 32V64H0Z" fill="currentColor" opacity=".17" />
      <g fill="currentColor" opacity=".4">
        <rect x="158" y="36" width="26" height="28" />
        <rect x="151" y="30" width="9" height="34" />
        <rect x="182" y="30" width="9" height="34" />
        <rect x="165" y="24" width="14" height="40" />
        <path d="M150 30L155.5 19L161 30Z" />
        <path d="M181 30L186.5 19L192 30Z" />
        <path d="M164 24L172 9L180 24Z" />
        <rect x="171.5" y="1" width="1" height="9" />
        <path d="M172.5 1.5L179 3.5L172.5 5.5Z" />
        <path d="M158 36h3v-3h3v3h3v-3h3v3h3v-3h3v3h3v-3h3v3h3v-3h3v3" />
      </g>
      <g style={{ fill: "var(--sv-deep)" }} opacity=".85">
        <rect x="170" y="32" width="4" height="7" rx="2" />
        <rect x="153.5" y="38" width="3" height="6" rx="1.5" />
        <rect x="184.5" y="38" width="3" height="6" rx="1.5" />
        <rect x="168" y="46" width="8" height="18" rx="4" />
      </g>
      <path d="M0 58Q28 47 60 54T122 52T200 57V64H0Z" fill="currentColor" opacity=".3" />
    </>
  );
}

/** Northrend: aurora and stars over ice peaks, Icecrown's spires with a glow behind them. */
function Northrend() {
  return (
    <>
      <defs>
        <radialGradient id="sv-northrend-glow">
          <stop offset="0" stopColor="currentColor" stopOpacity=".5" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </radialGradient>
      </defs>
      <path d="M0 24Q40 4 84 18T200 8" fill="none" stroke="currentColor" strokeWidth="10" strokeLinecap="round" opacity=".1" />
      <path d="M0 32Q46 14 92 26T200 18" fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" opacity=".13" />
      <g fill="currentColor" opacity=".6">
        <circle cx="96" cy="8" r="1" />
        <circle cx="122" cy="20" r=".8" />
        <circle cx="70" cy="14" r=".8" />
        <circle cx="140" cy="6" r="1.1" />
        <circle cx="112" cy="36" r=".7" />
        <circle cx="52" cy="6" r=".7" />
        <circle cx="194" cy="8" r=".9" />
      </g>
      <path d="M133 12v5M130.5 14.5h5M131.2 12.7l3.6 3.6M134.8 12.7l-3.6 3.6" fill="none" stroke="currentColor" strokeWidth=".8" strokeLinecap="round" opacity=".5" />
      <circle cx="172" cy="26" r="28" fill="url(#sv-northrend-glow)" />
      <path d="M0 48L20 30L34 41L58 18L80 43L100 34L126 47L158 38L200 42V64H0Z" fill="currentColor" opacity=".17" />
      <g fill="currentColor" opacity=".42">
        <path d="M166 64V32L171 3L176 32V64Z" />
        <path d="M155 64V40L159.5 19L164 40V64Z" />
        <path d="M178 64V38L182.5 15L187 38V64Z" />
        <path d="M146 64V51L150 42L154 51V64Z" />
        <path d="M188 64V48L192 38L196 48V64Z" />
        <rect x="146" y="54" width="50" height="10" />
      </g>
      <g fill="#fff" opacity=".55">
        <rect x="170" y="20" width="2" height="7" rx="1" />
        <rect x="158.7" y="30" width="1.6" height="5" rx=".8" />
        <rect x="181.7" y="28" width="1.6" height="5" rx=".8" />
      </g>
      <path d="M0 57Q34 51 70 56T140 55T200 57V64H0Z" fill="currentColor" opacity=".25" />
    </>
  );
}

const SCENES: Record<Backdrop, () => ReactNode> = { azeroth: Azeroth, northrend: Northrend };

/** The scene for a recipe's backdrop, sized to fill the button behind its content. */
export function ServerArt({ scene }: { scene: Backdrop }) {
  const Scene = SCENES[scene];
  return (
    <svg className="server-btn-art pointer-events-none absolute inset-0 h-full w-full" viewBox="0 0 200 64" preserveAspectRatio="xMaxYMax slice" aria-hidden focusable="false">
      <Scene />
    </svg>
  );
}
