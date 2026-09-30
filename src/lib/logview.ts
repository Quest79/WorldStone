// The log viewer: which sources there are, how many lines to ask for, and how a line is found and shown.
// Pure, so it is tested. What the backend returns is already cleaned and bounded; this only arranges it.

import type { LogSource, LogSourceKind, ViewLine } from "../api/types";
import { formatStamp } from "./format";

/** The choices for how many of the newest lines to read. The backend keeps any request within its own limits. */
export const LINE_CHOICES = [200, 500, 1000, 2000, 5000] as const;
export const DEFAULT_LINE_CHOICE = 500;

/** How often a followed log is read again. */
export const FOLLOW_MS = 2000;

export const KIND_HEADING: Record<LogSourceKind, string> = {
  launcher: "WorldStone",
  container: "Servers (Docker)",
  file: "Server log files",
};

export interface SourceGroup {
  kind: LogSourceKind;
  heading: string;
  sources: LogSource[];
}

/** The sources grouped for a select, in a fixed order, with empty groups left out. */
export function groupSources(sources: LogSource[]): SourceGroup[] {
  return (["launcher", "container", "file"] as const)
    .map((kind) => ({ kind, heading: KIND_HEADING[kind], sources: sources.filter((s) => s.kind === kind) }))
    .filter((g) => g.sources.length > 0);
}

/** The source to show first: the one already chosen if it is still there, else the first readable one. */
export function pickSource(sources: LogSource[], chosen: string | null): string | null {
  if (chosen !== null && sources.some((s) => s.id === chosen)) return chosen;
  return sources.find((s) => s.available)?.id ?? sources[0]?.id ?? null;
}

/** Lines that contain the text, in any letter case. An empty search keeps everything. */
export function filterLines(lines: ViewLine[], query: string): ViewLine[] {
  const needle = query.trim().toLowerCase();
  return needle === "" ? lines : lines.filter((l) => l.text.toLowerCase().includes(needle));
}

/** When a line was written, or blank when the source does not say. */
export function lineStamp(line: ViewLine): string {
  return line.tsMs === null ? "" : formatStamp(line.tsMs);
}

/** `12 of 500 lines match`, or how many are shown. */
export function countText(shown: number, total: number, query: string): string {
  if (query.trim() === "") return `${total} line${total === 1 ? "" : "s"}`;
  return `${shown} of ${total} line${total === 1 ? "" : "s"} match`;
}

/** The lines as one text, to copy. */
export function asText(lines: ViewLine[]): string {
  return lines.map((l) => (l.tsMs === null ? l.text : `${formatStamp(l.tsMs)} ${l.text}`)).join("\n");
}
