import { createContext, useContext, type ReactNode } from "react";

export const SESSION_EXPIRED_MESSAGE = "Your Layrs session expired. Sign in again to continue.";

export class PrivateRequestCancelledError extends Error {
  constructor() {
    super("Private request was cancelled.");
    this.name = "PrivateRequestCancelledError";
  }
}

export type PrivateJsonFetcher = (url: string, init?: RequestInit) => Promise<unknown>;

export interface PrivateRequestContext {
  fetchJson: PrivateJsonFetcher;
  signal: AbortSignal;
}

interface ActiveRequest {
  controller: AbortController;
  generation: number;
  key: string;
}

export class StudioPrivateApiController {
  private active = true;
  private nextGeneration = 0;
  private onUnauthorized: (message: string) => void;
  private requests = new Map<string, ActiveRequest>();
  private sessionGeneration = 0;

  constructor(onUnauthorized: (message: string) => void = () => undefined) {
    this.onUnauthorized = onUnauthorized;
  }

  setUnauthorizedHandler(handler: (message: string) => void) {
    this.onUnauthorized = handler;
  }

  reset() {
    this.abortAll();
    this.active = true;
    this.sessionGeneration += 1;
  }

  cancel(scope: string, key?: string) {
    const current = this.requests.get(scope);
    if (!current || (key !== undefined && current.key !== key)) {
      return;
    }
    current.controller.abort();
    this.requests.delete(scope);
  }

  async request<T>(
    scope: string,
    key: string,
    task: (context: PrivateRequestContext) => Promise<T>
  ): Promise<T> {
    if (!this.active) {
      throw new PrivateRequestCancelledError();
    }

    this.cancel(scope);
    const request: ActiveRequest = {
      controller: new AbortController(),
      generation: ++this.nextGeneration,
      key
    };
    const sessionGeneration = this.sessionGeneration;
    this.requests.set(scope, request);

    try {
      const result = await task({
        fetchJson: (url, init) => this.fetchJson(url, request.controller.signal, init),
        signal: request.controller.signal
      });
      if (!this.isCurrent(scope, request, sessionGeneration)) {
        throw new PrivateRequestCancelledError();
      }
      return result;
    } catch (error) {
      if (isUnauthorizedError(error)) {
        this.expireSession();
      }
      if (
        error instanceof PrivateRequestCancelledError ||
        isAbortError(error) ||
        !this.isCurrent(scope, request, sessionGeneration)
      ) {
        throw new PrivateRequestCancelledError();
      }
      throw error;
    } finally {
      if (this.requests.get(scope) === request) {
        this.requests.delete(scope);
      }
    }
  }

  private abortAll() {
    for (const request of this.requests.values()) {
      request.controller.abort();
    }
    this.requests.clear();
  }

  private expireSession() {
    if (!this.active) {
      return;
    }
    this.active = false;
    this.sessionGeneration += 1;
    this.abortAll();
    this.onUnauthorized(SESSION_EXPIRED_MESSAGE);
  }

  private async fetchJson(url: string, signal: AbortSignal, init: RequestInit = {}): Promise<unknown> {
    const headers = new Headers(init.headers);
    if (init.body !== undefined && !(init.body instanceof FormData) && !headers.has("Content-Type")) {
      headers.set("Content-Type", "application/json");
    }
    const response = await fetch(url, {
      ...init,
      credentials: "include",
      headers,
      signal
    });
    const payload = await readOptionalJson(response);
    if (!response.ok) {
      const error = Object.assign(
        new Error(errorMessageFromPayload(payload) ?? response.statusText),
        { status: response.status }
      );
      if (response.status === 401) {
        this.expireSession();
        throw new PrivateRequestCancelledError();
      }
      throw error;
    }
    return payload;
  }

  private isCurrent(scope: string, request: ActiveRequest, sessionGeneration: number) {
    return (
      this.active &&
      this.sessionGeneration === sessionGeneration &&
      this.requests.get(scope) === request &&
      !request.controller.signal.aborted
    );
  }
}

const StudioPrivateApiContext = createContext<StudioPrivateApiController | null>(null);

export function StudioPrivateApiProvider({
  api,
  children
}: {
  api: StudioPrivateApiController;
  children: ReactNode;
}) {
  return <StudioPrivateApiContext.Provider value={api}>{children}</StudioPrivateApiContext.Provider>;
}

export function useStudioPrivateApi(): StudioPrivateApiController {
  const api = useContext(StudioPrivateApiContext);
  if (!api) {
    throw new Error("Studio private API is unavailable outside its provider.");
  }
  return api;
}

export function isPrivateRequestCancelled(error: unknown): boolean {
  return error instanceof PrivateRequestCancelledError || isAbortError(error);
}

export function privateRequestKey(...parts: Array<string | number | null | undefined>): string {
  return parts.map((part) => String(part ?? "-")).join("::");
}

function isUnauthorizedError(error: unknown): boolean {
  if (!error || typeof error !== "object") {
    return false;
  }
  return (error as { status?: unknown }).status === 401;
}

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

async function readOptionalJson(response: Response): Promise<unknown> {
  if (response.status === 204) {
    return undefined;
  }
  const text = await response.text();
  if (!text) {
    return undefined;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

function errorMessageFromPayload(payload: unknown): string | undefined {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    return undefined;
  }
  const record = payload as Record<string, unknown>;
  const nested = record.error && typeof record.error === "object" ? (record.error as Record<string, unknown>) : undefined;
  for (const value of [record.message, nested?.message, record.code, nested?.code]) {
    if (typeof value === "string" && value.trim()) {
      return value;
    }
  }
  return undefined;
}
