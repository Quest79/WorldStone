// How sizes, durations and times are written on the Maintenance and Logs pages. Pure, so it is tested.

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const;

/** `1.5 MiB`. Docker and the file system both count in binary units, so this does too. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)} B`;
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${UNITS[unit]}`;
}

/** How long something has been up: `45 s`, `12 min`, `3 h 05 min`, `2 d 4 h`. */
export function formatUptime(secs: number | null): string {
  if (secs === null || !Number.isFinite(secs) || secs < 0) return "—";
  const s = Math.floor(secs);
  if (s < 60) return `${s} s`;
  const minutes = Math.floor(s / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ${String(minutes % 60).padStart(2, "0")} min`;
  return `${Math.floor(hours / 24)} d ${hours % 24} h`;
}

/** How long ago something happened: `just now`, `5 min ago`, `2 h ago`, `3 days ago`. */
export function formatAge(secs: number | null): string {
  if (secs === null || !Number.isFinite(secs)) return "—";
  const s = Math.floor(secs);
  if (s < 60) return "just now";
  const minutes = Math.floor(s / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  return `${days} ${days === 1 ? "day" : "days"} ago`;
}

/** Share of one processor core. Unknown is a dash, never zero. */
export function formatPercent(percent: number | null): string {
  if (percent === null || !Number.isFinite(percent) || percent < 0) return "—";
  return `${percent < 10 ? percent.toFixed(1) : Math.round(percent)}%`;
}

/** `312 MiB of 7.6 GiB`, or just the use when Docker gave no limit. */
export function formatMemory(used: number | null, limit: number | null): string {
  if (used === null) return "—";
  return limit === null || limit === 0 ? formatSize(used) : `${formatSize(used)} of ${formatSize(limit)}`;
}

const two = (n: number) => String(n).padStart(2, "0");

/** `2026-09-29 10:15:00` in the local time zone. */
export function formatStamp(tsMs: number): string {
  const d = new Date(tsMs);
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

/** `2026-09-29 10:15` for a time in seconds since the epoch. */
export function formatMoment(seconds: number | null): string {
  return seconds === null ? "—" : formatStamp(seconds * 1000).slice(0, 16);
}
