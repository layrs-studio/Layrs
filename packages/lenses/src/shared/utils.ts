export type FieldRecord = Record<string, unknown>;

export function joinClassNames(...classNames: Array<string | undefined>): string | undefined {
  return classNames.filter(Boolean).join(" ") || undefined;
}

export function getStringField(fields: FieldRecord, key: string): string | undefined {
  const value = fields[key];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

export function getNumberValue(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function stringifyValue(value: unknown): string | undefined {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }

  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }

  return JSON.stringify(value);
}

export function shortenHash(value: string | undefined): string | undefined {
  return value && value.length > 16 ? `${value.slice(0, 16)}...` : value;
}

export function formatBytes(bytes: number | undefined): string | undefined {
  if (bytes === undefined) {
    return undefined;
  }

  if (bytes < 1024) {
    return `${bytes} B`;
  }

  const units = ["KB", "MB", "GB"];
  let size = bytes / 1024;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}
