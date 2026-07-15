export const DESKTOP_AUTH_EXPIRED_EVENT = "layrs:desktop-auth-expired";
export const DESKTOP_AUTH_EXPIRED_MESSAGE =
  "Layrs Desktop session expired or was revoked. Reconnect this device to continue.";

export class DesktopPrivateRequestCancelledError extends Error {
  constructor() {
    super(DESKTOP_AUTH_EXPIRED_MESSAGE);
    this.name = "DesktopPrivateRequestCancelledError";
  }
}

let generation = 0;
let sessionActive = true;

export async function guardDesktopPrivateRequest<T>(request: () => Promise<T>): Promise<T> {
  if (!sessionActive) {
    throw new DesktopPrivateRequestCancelledError();
  }
  const requestGeneration = generation;
  try {
    const result = await request();
    if (requestGeneration !== generation) {
      throw new DesktopPrivateRequestCancelledError();
    }
    return result;
  } catch (error) {
    if (error instanceof DesktopPrivateRequestCancelledError) {
      throw error;
    }
    if (isDesktopAuthExpiredError(error)) {
      invalidateDesktopPrivateRequests();
      throw new DesktopPrivateRequestCancelledError();
    }
    if (requestGeneration !== generation) {
      throw new DesktopPrivateRequestCancelledError();
    }
    throw error;
  }
}

export function invalidateDesktopPrivateRequests() {
  if (!sessionActive) {
    return;
  }
  sessionActive = false;
  generation += 1;
  globalThis.dispatchEvent?.(
    new CustomEvent(DESKTOP_AUTH_EXPIRED_EVENT, {
      detail: { message: DESKTOP_AUTH_EXPIRED_MESSAGE }
    })
  );
}

export function resetDesktopPrivateRequests() {
  if (sessionActive) {
    return;
  }
  sessionActive = true;
  generation += 1;
}

export function isDesktopPrivateRequestCancelled(error: unknown): boolean {
  return error instanceof DesktopPrivateRequestCancelledError;
}

export function isDesktopAuthExpiredError(error: unknown): boolean {
  const record = error && typeof error === "object" ? (error as Record<string, unknown>) : undefined;
  if (record?.status === 401 || record?.statusCode === 401 || record?.status_code === 401) {
    return true;
  }
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : safeJson(error);
  const lower = message.toLowerCase();
  return (
    lower.includes("http 401") ||
    lower.includes('"status":401') ||
    lower.includes('"code":"unauthorized"') ||
    lower.includes("session expired or was revoked")
  );
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value) ?? "";
  } catch {
    return "";
  }
}
