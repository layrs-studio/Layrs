import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { LensCatalogItem, LensConfiguration, LensPacket, LensTransport, LensViewInput } from "./types";
import { pinKey } from "./types";
import { verifyViewer } from "./verify";
import { readCache, writeCache } from "./cache";

type Loaded = Awaited<ReturnType<typeof verifyViewer>>;
type Runtime = {
  scope: string; hostUrl?: string; configuration: LensConfiguration | null; error: string;
  offline: boolean;
  load: (item: LensCatalogItem) => Promise<Loaded>;
  refresh: () => Promise<void>;
};
const Context = createContext<Runtime | null>(null);
export const useLensRuntime = () => useContext(Context);

/** A provider instance is scoped to one server, account and project. */
export function ProjectLensProvider({ scope, hostUrl, transport, children, offline = false, persist = false }: {
  scope: string; hostUrl?: string; transport: LensTransport; children: ReactNode; offline?: boolean; persist?: boolean;
}) {
  return <ScopedProvider key={`${scope}|${offline}`} scope={scope} hostUrl={hostUrl} transport={transport} offline={offline} persist={persist}>{children}</ScopedProvider>;
}
function ScopedProvider({ scope, hostUrl, transport, children, offline, persist }: {
  scope: string; hostUrl?: string; transport: LensTransport; children: ReactNode; offline: boolean; persist: boolean;
}) {
  const [configuration, setConfiguration] = useState<LensConfiguration | null>(null);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const revalidation = useRef<{ generation: number; promise: Promise<LensConfiguration> } | null>(null);
  const writes = useRef<Promise<void>>(Promise.resolve());
  const cache = useMemo(() => new Map<string, Promise<Loaded>>(), [scope]);
  const readConfiguration = useCallback(async () => {
    if (offline && persist) {
      const saved = await readCache<LensConfiguration>(`${scope}|configuration`);
      if (!saved) throw Error("Aucune configuration conservée pour ce projet.");
      return { ...saved, canManage: false };
    }
    const next = await transport<LensConfiguration>({ action: "projectLenses" });
    return next;
  }, [scope, transport, offline, persist]);
  const acceptConfiguration = useCallback((next: LensConfiguration, request: number) => {
    if (request !== generation.current) return;
    setConfiguration(next);
    if (persist && !offline) {
      // Serialize accepted observations; a late older response must never
      // resurrect a removed/revoked assignment in the offline cache.
      writes.current = writes.current.then(async () => {
        if (request === generation.current) await writeCache(`${scope}|configuration`, next);
      });
    }
  }, [scope, offline, persist]);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    // Stop using the previous assignment as soon as revalidation begins.
    setConfiguration(null);
    setError("");
    try { const next = await readConfiguration(); acceptConfiguration(next, request); }
    catch { if (request === generation.current) setError("Les lenses du projet ne sont pas disponibles. Affichage de secours."); }
  }, [readConfiguration, acceptConfiguration]);
  useEffect(() => { let current = true;
    const request = ++generation.current;
    setConfiguration(null);
    readConfiguration().then(value => { if (current) acceptConfiguration(value, request); }, () => {
      if (current && request === generation.current) setError("Les lenses du projet ne sont pas disponibles. Affichage de secours.");
    });
    return () => { current = false; generation.current++; cache.clear(); };
  }, [readConfiguration, acceptConfiguration, cache]);
  useEffect(() => {
    const revalidate = () => { if (document.visibilityState === "visible") void refresh(); };
    document.addEventListener("visibilitychange", revalidate);
    return () => document.removeEventListener("visibilitychange", revalidate);
  }, [refresh]);
  const load = useCallback(async (item: LensCatalogItem) => {
    const key = pinKey(item.pin);
    if (!offline) {
      const request = generation.current;
      // Cached code is an optimisation, never an online authorization grant.
      // All viewers opened together share one authority observation. They must
      // not cancel one another merely because a compare contains several files.
      if (!revalidation.current || revalidation.current.generation !== request) {
        const pending = { generation: request, promise: transport<LensConfiguration>({ action: "projectLenses" }) };
        revalidation.current = pending;
        void pending.promise.finally(() => {
          if (revalidation.current === pending) revalidation.current = null;
        }).catch(() => undefined);
      }
      const latest = await revalidation.current.promise;
      if (request !== generation.current) throw Error("La configuration a été actualisée. Réouvrez le fichier.");
      if (!latest.pins.some(pin => pinKey(pin) === key) || !latest.availableCatalog.some(candidate => pinKey(candidate.pin) === key && candidate.available)) {
        cache.delete(key); acceptConfiguration(latest, request);
        await writes.current;
        throw Error("Cette version n’est plus autorisée dans ce projet.");
      }
      if (persist) {
        writes.current = writes.current.then(async () => {
          if (request === generation.current) await writeCache(`${scope}|configuration`, latest);
        });
        await writes.current;
      }
    }
    if (!cache.has(key)) {
      const loading = (async () => {
        let packet = persist ? await readCache<LensPacket>(`${scope}|packet|${key}`) : null;
        if (!packet && !offline) packet = await transport<LensPacket>({ action: "lensPacket", pin: item.pin });
        if (!packet) throw Error("Cette lens n’est pas dans le cache local.");
        const verified = await verifyViewer(packet, item.pin);
        if (!offline && persist) await writeCache(`${scope}|packet|${key}`, packet);
        return verified;
      })();
      cache.set(key, loading);
      void loading.catch(() => { cache.delete(key); });
    }
    return cache.get(key)!;
  }, [cache, transport, offline, scope, persist, acceptConfiguration]);
  return <Context.Provider value={{ scope, hostUrl, configuration, error, load, refresh, offline }}>{children}</Context.Provider>;
}
export function selectProjectLens(configuration: LensConfiguration, input: LensViewInput): LensCatalogItem | undefined {
  const enabled = configuration.availableCatalog.filter(item => item.available && configuration.pins.some(pin => pinKey(pin) === pinKey(item.pin)));
  const extension = input.path.split("/").pop()?.split(".").slice(1).pop()?.toLowerCase();
  const assigned = configuration.associations.find(rule => rule.extension.replace(/^\./, "").toLowerCase() === extension);
  if (assigned) return enabled.find(item => item.pin.lensId === assigned.lensId);
  const matches = enabled.filter(item => extension && item.manifest.analyzer.fileExtensions.includes(extension));
  if (matches.length === 1) return matches[0];
  if (matches.length > 1) return undefined;
  return enabled.find(item => item.pin.lensId === "layrs.raw");
}
