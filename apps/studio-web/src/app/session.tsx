import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from "react";

/*
 * Web session. The access token lives only in memory: a reload or a new tab
 * asks to sign in again. Every response is bound to the session generation it
 * was requested in, so late answers never leak into another account.
 */

export type Principal = { principalId: string; displayName: string; kind: string };

export type ApiOptions = {
  anonymous?: boolean;
  /** A 401 is an expected answer (e.g. wrong current password), not a lost session. */
  unauthorizedIsAnswer?: boolean;
};

export type Api = <T>(path: string, method?: string, body?: unknown, options?: ApiOptions) => Promise<T>;

type SessionApi = {
  principal: Principal | null;
  api: Api;
  signIn: (login: string, password: string) => Promise<void>;
  signOut: () => Promise<void>;
  /** Drops the session locally, e.g. after a password change revoked it. */
  forget: () => void;
  /** Incremented on every sign-in/sign-out; use as a React key to purge views. */
  generation: number;
};

const SessionContext = createContext<SessionApi | null>(null);

export function useSession() {
  const value = useContext(SessionContext);
  if (!value) throw new Error("useSession must be used inside <SessionProvider>");
  return value;
}

const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string | null,
  ) {
    super(message);
  }
}

export function SessionProvider({ children }: { children: ReactNode }) {
  const secret = useRef("");
  const generationRef = useRef(0);
  const [generation, setGeneration] = useState(0);
  const [principal, setPrincipal] = useState<Principal | null>(null);

  const forget = useCallback(() => {
    secret.current = "";
    generationRef.current++;
    setGeneration(generationRef.current);
    setPrincipal(null);
  }, []);

  const api = useCallback<Api>(
    async <T,>(path: string, method = "GET", body?: unknown, options: ApiOptions = {}) => {
      if (location.protocol !== "https:") throw new Error("Ouvrez Studio sur l’adresse HTTPS de votre instance Layrs.");
      const requestGeneration = generationRef.current;
      const headers: Record<string, string> = { Accept: "application/json" };
      if (!options.anonymous) {
        if (!secret.current) throw new Error("Connectez-vous à cette instance.");
        headers.Authorization = `Bearer ${secret.current}`;
      }
      if (body !== undefined) headers["Content-Type"] = "application/json";
      const response = await fetch(path, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: "error",
        credentials: "omit",
        cache: "no-store",
        signal: AbortSignal.timeout(120_000),
      });
      if (requestGeneration !== generationRef.current) throw new Error("La session a changé pendant la lecture.");
      if (response.status === 401 && !options.anonymous && !options.unauthorizedIsAnswer) {
        forget();
        throw new ApiError("Session expirée ou révoquée. Reconnectez-vous.", 401, "unauthorized");
      }
      const reader = response.body?.getReader();
      if (!reader) throw new Error("Réponse serveur absente.");
      const chunks: Uint8Array[] = [];
      let size = 0;
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          size += value.length;
          if (size > MAX_RESPONSE_BYTES) throw new Error("Réponse trop volumineuse pour cette vue.");
          chunks.push(value);
        }
      } catch (error) {
        await reader.cancel();
        throw error;
      }
      const bytes = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) {
        bytes.set(chunk, offset);
        offset += chunk.length;
      }
      const result = size ? JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) : {};
      if (requestGeneration !== generationRef.current) throw new Error("La session a changé pendant la lecture.");
      if (!response.ok) {
        const code = result?.error?.code;
        const safeCode = typeof code === "string" && /^[a-zA-Z0-9_-]{1,96}$/.test(code) ? code : null;
        throw new ApiError(safeCode ?? `Erreur serveur ${response.status}`, response.status, safeCode);
      }
      return result as T;
    },
    [forget],
  );

  const signIn = useCallback(
    async (login: string, password: string) => {
      const session = await api<{ principal_id: string; access_token: string }>("/v3/auth/login", "POST", { login, password }, { anonymous: true });
      secret.current = session.access_token;
      generationRef.current++;
      setGeneration(generationRef.current);
      let identity: Principal = { principalId: session.principal_id, displayName: "", kind: "" };
      try {
        const who = await api<Record<string, unknown>>("/v3/auth/whoami");
        identity = {
          principalId: String(who.principal_id ?? who.principalId ?? session.principal_id),
          displayName: String(who.display_name ?? who.displayName ?? ""),
          kind: String(who.kind ?? ""),
        };
      } catch {
        // The profile is only for display; the session stays valid without it.
      }
      setPrincipal(identity);
    },
    [api],
  );

  const signOut = useCallback(async () => {
    try {
      await api("/v3/auth/logout", "POST", {});
    } finally {
      forget();
    }
  }, [api, forget]);

  const value = useMemo(() => ({ principal, api, signIn, signOut, forget, generation }), [principal, api, signIn, signOut, forget, generation]);
  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

/** Human message for an error thrown by `api`. */
export function describeError(error: unknown) {
  if (error instanceof ApiError) {
    if (error.status === 403) return "Vous n’avez pas accès à cette ressource.";
    if (error.status === 404) return "Élément introuvable ou inaccessible.";
    return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}
