import { formatDateTime, formatRelativeTime } from "../foundation/format";

const POSTGRES = /^(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2}:\d{2})(\.\d+)?([+-]\d{2})(?::?(\d{2}))?$/;

/**
 * Some administration responses carry PostgreSQL text timestamps
 * ("2026-09-30 18:12:33.123456+00"); others are RFC 3339. Both become ISO.
 */
export function normalizeTime(value: string | number | null | undefined): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "number") return new Date(value).toISOString();
  const match = POSTGRES.exec(value.trim());
  if (!match) return value;
  const [, date, time, fraction = "", hours, minutes = "00"] = match;
  return `${date}T${time}${fraction.slice(0, 4)}${hours}:${minutes}`;
}

export function timeOf(value: string | null | undefined) {
  return new Date(normalizeTime(value)).getTime();
}

/** "30 sept. 2026, 20:12", or the raw text when unreadable. */
export function when(value: string | null | undefined) {
  return value ? formatDateTime(normalizeTime(value)) : "—";
}

export function ago(value: string | null | undefined) {
  return value ? formatRelativeTime(normalizeTime(value)) : "—";
}
