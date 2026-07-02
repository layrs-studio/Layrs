export async function readJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) {
    return undefined;
  }

  try {
    return JSON.parse(text) as unknown;
  } catch {
    return { message: text };
  }
}

export function getErrorMessage(payload: unknown, fallback: string): string {
  if (typeof payload === "object" && payload) {
    if ("message" in payload && typeof payload.message === "string") {
      return payload.message;
    }
    if ("error" in payload && typeof payload.error === "string") {
      return payload.error;
    }
    if ("error" in payload && typeof payload.error === "object" && payload.error) {
      const error = payload.error as { message?: unknown };
      if (typeof error.message === "string") {
        return error.message;
      }
    }
  }

  return fallback;
}

export function getErrorCode(payload: unknown): string | undefined {
  if (typeof payload !== "object" || !payload) {
    return undefined;
  }

  if ("code" in payload && typeof payload.code === "string") {
    return payload.code;
  }

  if ("error" in payload && typeof payload.error === "object" && payload.error) {
    const error = payload.error as { code?: unknown };
    if (typeof error.code === "string") {
      return error.code;
    }
  }

  return undefined;
}

export function unwrapItems<T>(value: T[] | { items: T[] }): T[] {
  return Array.isArray(value) ? value : value.items;
}

export function slugify(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}
