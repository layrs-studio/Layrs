const relative = new Intl.RelativeTimeFormat("fr", { numeric: "auto" });
const dateTime = new Intl.DateTimeFormat("fr", { dateStyle: "medium", timeStyle: "short" });

const units: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["week", 7 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
];

/** "il y a 12 minutes"; invalid input is returned unchanged rather than guessed. */
export function formatRelativeTime(value: string | number | Date, now = Date.now()) {
  const time = new Date(value).getTime();
  if (Number.isNaN(time)) return String(value);
  const seconds = Math.round((time - now) / 1000);
  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) return relative.format(Math.round(seconds / size), unit);
  }
  return Math.abs(seconds) < 30 ? "à l’instant" : relative.format(seconds, "second");
}

export function formatDateTime(value: string | number | Date) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : dateTime.format(date);
}

export function formatBytes(value: number | null | undefined) {
  if (value === null || value === undefined) return "absent";
  if (value < 1024) return `${value} o`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1).replace(".", ",")} Kio`;
  if (value < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(1).replace(".", ",")} Mio`;
  return `${(value / 1024 ** 3).toFixed(1).replace(".", ",")} Gio`;
}
