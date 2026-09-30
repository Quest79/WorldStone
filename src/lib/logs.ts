import type { LogEvent } from "../api/types";

/** The in-memory log keeps the newest entries; the full history is in the log file. */
export const MAX_LOG_ENTRIES = 3000;

export function appendLog(logs: LogEvent[], entry: LogEvent, max = MAX_LOG_ENTRIES): LogEvent[] {
  const next = logs.length >= max ? logs.slice(logs.length - max + 1) : logs.slice();
  next.push(entry);
  return next;
}

export function formatTime(tsMs: number): string {
  return new Date(tsMs).toLocaleTimeString([], { hour12: false });
}
