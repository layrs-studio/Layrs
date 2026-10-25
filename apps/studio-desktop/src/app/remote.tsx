import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { nativeRemote } from "../tauri";
import { explainError, type ErrorContext, type Explained } from "./errors";
import {
  lastOrigin,
  normalizeOrigin,
  readActivity,
  rememberAuthority,
  rememberOrigin,
  rememberedAuthority,
  rememberedOrigin,
  saveActivity,
  type ActivityEntry,
  type ActivityKind,
} from "./serverMemory";

export type RemoteAction = { action: string; [key: string]: unknown };

export type Session =
  | { state: "none" }
  | { state: "unknown" }
  | { state: "checking" }
  | { state: "connected"; principalId: string; displayName?: string }
  | { state: "signedOut"; reason?: string }
  | { state: "unreachable"; reason: string };

export type Login = {
  userCode: string;
  verificationUri: string;
  expiresAt: number;
  interval: number;
  status: "waiting" | "checking" | "expired" | "refused";
  error?: string;
};

export type Transfer = {
  entryId: string;
  transferId: string | null;
  kind: ActivityKind;
  label: string;
  startedAt: string;
  steps: string[];
  cancelling: boolean;
};

export type Notice = { tone: "success" | "info" | "warning" | "danger"; title: string; body?: string; detail?: string };

type Reply = {
  status?: string;
  principalId?: string;
  principal_id?: string;
  display_name?: string;
  userCode?: string;
  verificationUri?: string;
  expiresIn?: number;
  interval?: number;
  receipt?: { status: string };
  received?: { divergent: boolean };
  state?: { worktree: { checkoutRoot: string } };
};

type OperateSpec = {
  kind: ActivityKind;
  label: string;
  /** Label of the foreground lock while it runs. */
  busy: string;
  context: ErrorContext;
  cancellable?: boolean;
};

type RemoteApi = {
  root: string;
  origin: string;
  /** True for limited-access folders, bound to the server that issued them. */
  fixed: boolean;
  authority: string;
  session: Session;
  /** Changes whenever the authenticated identity may have changed. */
  epoch: number;
  login: Login | null;
  transfer: Transfer | null;
  activity: ActivityEntry[];
  notice: Notice | null;
  clearNotice: () => void;
  chooseServer: (origin: string) => void;
  bindOrigin: (origin: string) => void;
  saveAuthority: (pem: string) => void;
  check: () => Promise<void>;
  startLogin: () => Promise<void>;
  pollLogin: () => Promise<void>;
  cancelLogin: () => void;
  logout: () => Promise<void>;
  /** Server operation under the foreground lock, recorded in the activity log. */
  operate: <T extends Reply>(spec: OperateSpec, action: RemoteAction) => Promise<T | null>;
  /** Records an operation driven by another lock owner (limited access). Rethrows failures. */
  track: <T>(spec: Omit<OperateSpec, "busy">, work: () => Promise<T>, describe: (result: T) => Pick<Notice, "tone" | "title" | "body"> & { unconfirmed?: boolean }) => Promise<T>;
  cancelTransfer: () => Promise<void>;
  /** Raw authenticated call for embedded team surfaces. */
  transport: <T>(action: RemoteAction) => Promise<T>;
};

const RemoteContext = createContext<RemoteApi | null>(null);

export function useRemote() {
  const value = useContext(RemoteContext);
  if (!value) throw new Error("useRemote must be used inside <RemoteProvider>");
  return value;
}

function newId() {
  return crypto.randomUUID();
}

/** Per-folder server connection, transfers and their activity log. */
export function RemoteProvider({
  root,
  acquire,
  release,
  onChanged,
  children,
}: {
  root: string;
  acquire: (label: string) => boolean;
  release: () => void;
  /** Called after an operation that changed a local folder (its root). */
  onChanged: (root: string) => void;
  children: ReactNode;
}) {
  const [origin, setOrigin] = useState(() => rememberedOrigin(root) || lastOrigin());
  const [fixed, setFixed] = useState(false);
  const [authority, setAuthority] = useState(() => rememberedAuthority(rememberedOrigin(root) || lastOrigin()));
  const [session, setSession] = useState<Session>(() => ((rememberedOrigin(root) || lastOrigin()) ? { state: "unknown" } : { state: "none" }));
  const [epoch, setEpoch] = useState(0);
  const [login, setLogin] = useState<Login | null>(null);
  const [transfer, setTransfer] = useState<Transfer | null>(null);
  const [activity, setActivity] = useState<ActivityEntry[]>(() => readActivity(root));
  const [notice, setNotice] = useState<Notice | null>(null);
  const generation = useRef(0);
  const inFlight = useRef(false);
  const polling = useRef(false);
  const current = useRef<Transfer | null>(null);
  const config = useRef({ root, origin, authority });
  config.current = { root, origin, authority };

  const call = useCallback(<T,>(action: RemoteAction, target = config.current) => {
    return nativeRemote<T>(target.root, { origin: target.origin, caPem: target.authority || null, action });
  }, []);

  const record = useCallback((entry: ActivityEntry) => {
    const key = config.current.root;
    saveActivity(key, entry);
    setActivity(readActivity(key));
  }, []);

  // A transfer still marked running belongs to a Layrs process that stopped.
  useEffect(() => {
    const entries = readActivity(root);
    for (const entry of entries.filter((item) => item.outcome === "running")) {
      saveActivity(root, {
        ...entry,
        outcome: "interrupted",
        endedAt: entry.endedAt ?? new Date().toISOString(),
        message:
          entry.kind === "publish" || entry.kind === "publishIntegrate" || entry.kind === "candidates"
            ? "Layrs s’est arrêté pendant cette publication. Relancez Publier : la même publication est rejouée pour connaître son résultat, sans doublon. Vos captures locales sont conservées."
            : "Layrs s’est arrêté pendant ce transfert. Les fichiers locaux n’ont été remplacés qu’après vérification ; relancez l’opération si besoin.",
      });
    }
    setActivity(readActivity(root));
  }, [root]);

  const invalidate = useCallback(() => {
    generation.current++;
    polling.current = false;
    setEpoch((value) => value + 1);
  }, []);

  const check = useCallback(async () => {
    const target = config.current;
    if (!target.origin) {
      setSession({ state: "none" });
      return;
    }
    const version = generation.current;
    setSession({ state: "checking" });
    try {
      const reply = await call<Reply>({ action: "whoami" }, target);
      if (version !== generation.current) return;
      setSession({ state: "connected", principalId: reply.principalId || reply.principal_id || "", displayName: reply.display_name || undefined });
      rememberOrigin(target.root, target.origin);
    } catch (cause) {
      if (version !== generation.current) return;
      const explained = explainError(cause, "session");
      setSession(explained.signedOut ? { state: "signedOut" } : { state: "unreachable", reason: explained.detail });
    }
  }, [call]);

  const chooseServer = useCallback(
    (value: string) => {
      const next = normalizeOrigin(value);
      if (fixed || next === config.current.origin) return;
      // Callers may start a login right away, before the next render.
      config.current = { ...config.current, origin: next, authority: rememberedAuthority(next) };
      invalidate();
      setLogin(null);
      setNotice(null);
      setOrigin(next);
      setAuthority(rememberedAuthority(next));
      setSession(next ? { state: "unknown" } : { state: "none" });
    },
    [fixed, invalidate],
  );

  const bindOrigin = useCallback(
    (value: string) => {
      const next = normalizeOrigin(value);
      setFixed(true);
      if (next === config.current.origin) return;
      config.current = { ...config.current, origin: next, authority: rememberedAuthority(next) };
      invalidate();
      setLogin(null);
      setOrigin(next);
      setAuthority(rememberedAuthority(next));
      setSession(next ? { state: "unknown" } : { state: "none" });
    },
    [invalidate],
  );

  const saveAuthority = useCallback(
    (pem: string) => {
      rememberAuthority(config.current.origin, pem);
      config.current = { ...config.current, authority: pem.trim() ? pem : "" };
      setAuthority(pem.trim() ? pem : "");
      invalidate();
      setSession(config.current.origin ? { state: "unknown" } : { state: "none" });
    },
    [invalidate],
  );

  // Read-only session check whenever the server or its certificate changes.
  useEffect(() => {
    if (session.state === "unknown" && config.current.origin) void check();
  }, [session.state, check]);

  const startLogin = useCallback(async () => {
    const target = config.current;
    if (!target.origin || inFlight.current) return;
    inFlight.current = true;
    invalidate();
    setNotice(null);
    setSession({ state: "signedOut" });
    try {
      const reply = await call<Reply>({ action: "startLogin" }, target);
      rememberOrigin(target.root, target.origin);
      setLogin({
        userCode: reply.userCode ?? "",
        verificationUri: reply.verificationUri ?? "",
        expiresAt: Date.now() + Math.max(30, reply.expiresIn ?? 600) * 1000,
        interval: Math.max(5, reply.interval ?? 5),
        status: "waiting",
      });
    } catch (cause) {
      const explained = explainError(cause, "login");
      setNotice({ tone: "danger", title: explained.title, body: explained.body, detail: explained.detail });
      setSession({ state: "unreachable", reason: explained.detail });
    } finally {
      inFlight.current = false;
    }
  }, [call, invalidate]);

  const pollLogin = useCallback(async () => {
    if (polling.current) return;
    const pending = login;
    if (!pending || pending.status === "expired" || pending.status === "refused") return;
    if (Date.now() > pending.expiresAt) {
      setLogin({ ...pending, status: "expired" });
      return;
    }
    polling.current = true;
    const version = generation.current;
    setLogin((value) => value && { ...value, status: "checking", error: undefined });
    try {
      const reply = await call<Reply>({ action: "pollLogin" });
      if (version !== generation.current) return;
      if (reply.status === "authorized") {
        setLogin(null);
        setEpoch((value) => value + 1);
        setSession({ state: "connected", principalId: reply.principalId || reply.principal_id || "" });
        setNotice({ tone: "success", title: "Connexion autorisée.", body: "Le jeton est conservé dans le coffre du système." });
        rememberOrigin(config.current.root, config.current.origin);
        void check();
      } else {
        setLogin((value) => value && { ...value, status: "waiting", interval: Math.max(value.interval, reply.interval ?? value.interval) });
      }
    } catch (cause) {
      if (version !== generation.current) return;
      const detail = String(cause);
      setLogin((value) =>
        value && {
          ...value,
          status: /expired/i.test(detail) ? "expired" : /denied|refus/i.test(detail) ? "refused" : "waiting",
          error: explainError(cause, "login").detail,
        },
      );
    } finally {
      if (version === generation.current) polling.current = false;
    }
  }, [call, check, login]);

  // Bounded automatic check at the interval requested by the server.
  useEffect(() => {
    if (!login || login.status !== "waiting" || login.error) return;
    const delay = Math.min(Math.max(login.expiresAt - Date.now(), 0), login.interval * 1000);
    const timer = window.setTimeout(() => void pollLogin(), delay);
    return () => window.clearTimeout(timer);
  }, [login, pollLogin]);

  const cancelLogin = useCallback(() => {
    generation.current++;
    polling.current = false;
    setLogin(null);
  }, []);

  const logout = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      await call({ action: "logout" });
      invalidate();
      setSession({ state: "signedOut" });
      setNotice({ tone: "info", title: "Session révoquée.", body: "Le jeton de cet ordinateur a été supprimé du coffre du système." });
    } catch (cause) {
      const explained = explainError(cause, "session");
      setNotice({ tone: "danger", title: "La déconnexion n’a pas abouti.", body: "Le jeton est conservé pour réessayer la révocation.", detail: explained.detail });
    } finally {
      inFlight.current = false;
    }
  }, [call, invalidate]);

  const track = useCallback<RemoteApi["track"]>(
    async (spec, work, describe) => {
      const entry: ActivityEntry = {
        id: newId(),
        kind: spec.kind,
        label: spec.label,
        origin: config.current.origin,
        startedAt: new Date().toISOString(),
        outcome: "running",
        steps: ["Démarré"],
      };
      record(entry);
      setNotice(null);
      const view: Transfer = { entryId: entry.id, transferId: null, kind: spec.kind, label: spec.label, startedAt: entry.startedAt, steps: entry.steps, cancelling: false };
      current.current = view;
      setTransfer(view);
      try {
        const result = await work();
        const described = describe(result);
        record({ ...entry, endedAt: new Date().toISOString(), outcome: described.unconfirmed ? "unconfirmed" : "success", message: described.title, steps: [...(current.current?.steps ?? entry.steps), "Terminé"] });
        setNotice({ tone: described.tone, title: described.title, body: described.body });
        return result;
      } catch (cause) {
        const explained = explainError(cause, spec.context);
        record({ ...entry, endedAt: new Date().toISOString(), outcome: explained.unconfirmed ? "unconfirmed" : "failed", message: explained.title, steps: [...(current.current?.steps ?? entry.steps), "Échec"] });
        setNotice({ tone: explained.unconfirmed ? "warning" : "danger", title: explained.title, body: explained.body, detail: explained.detail });
        throw cause;
      } finally {
        current.current = null;
        setTransfer(null);
      }
    },
    [record],
  );

  const operate = useCallback<RemoteApi["operate"]>(
    async <T extends Reply>(spec: OperateSpec, action: RemoteAction) => {
      const target = { ...config.current };
      if (!target.origin || inFlight.current) return null;
      if (!acquire(spec.busy)) return null;
      inFlight.current = true;
      setNotice(null);
      const transferId = spec.cancellable ? newId() : null;
      const entry: ActivityEntry = {
        id: newId(),
        kind: spec.kind,
        label: spec.label,
        origin: target.origin,
        startedAt: new Date().toISOString(),
        outcome: "running",
        steps: [spec.cancellable ? "Transfert démarré, annulable" : "Démarré"],
      };
      record(entry);
      const view: Transfer = { entryId: entry.id, transferId, kind: spec.kind, label: spec.label, startedAt: entry.startedAt, steps: entry.steps, cancelling: false };
      current.current = view;
      setTransfer(view);
      let changed: string | undefined;
      let result: T | null = null;
      try {
        const request = transferId ? { ...action, action: `${action.action}Cancellable`, transferId } : action;
        const reply = await call<T>(request, target);
        if (reply.receipt && reply.receipt.status !== "applied")
          throw new Error(
            "Publication refusée. Recevez la vue actuelle, puis vérifiez les droits, les protections et les têtes avant un nouvel essai.",
          );
        const title =
          action.action === "publishCandidates"
            ? "Captures candidates admises. Les têtes distantes restent inchangées ; prévisualisez maintenant votre choix."
            : action.action === "publish"
              ? "Publication confirmée par le serveur."
              : reply.received?.divergent
                ? "Réception terminée. Plusieurs têtes sont conservées ; une intégration reste nécessaire."
                : action.action === "receive"
                  ? "Réception terminée et vérifiée."
                  : action.action === "clone" || action.action === "cloneProjection" || action.action === "cloneProjectionSelected"
                    ? "Projet récupéré et vérifié."
                    : action.action === "provision"
                      ? "Projet créé sur le serveur."
                      : action.action === "migrateProjection"
                        ? "Accès limité créé. Ce dossier et son historique sont conservés."
                        : "Opération terminée.";
        const body =
          action.action === "publish"
            ? "Le reçu du serveur est appliqué sur cet ordinateur."
            : reply.received?.divergent
              ? "Préparez une fusion pour réunir les têtes."
              : undefined;
        record({ ...entry, endedAt: new Date().toISOString(), outcome: "success", message: title, steps: [...(current.current?.steps ?? entry.steps), "Terminé"] });
        setNotice({ tone: reply.received?.divergent ? "warning" : "success", title, body });
        if (["clone", "cloneProjection", "cloneProjectionSelected", "receive", "publish", "provision"].includes(action.action))
          changed = reply.state?.worktree.checkoutRoot || target.root;
        if (action.action === "migrateProjection") changed = String(action.destination);
        result = reply;
      } catch (cause) {
        const explained = explainError(cause, spec.context);
        if (explained.signedOut) {
          invalidate();
          setSession({ state: "signedOut" });
        }
        record({
          ...entry,
          endedAt: new Date().toISOString(),
          outcome: explained.cancelled ? "cancelled" : explained.unconfirmed ? "unconfirmed" : "failed",
          message: explained.title,
          steps: [...(current.current?.steps ?? entry.steps), explained.cancelled ? "Annulé" : "Échec"],
        });
        setNotice({
          tone: explained.cancelled ? "info" : explained.unconfirmed ? "warning" : "danger",
          title: explained.title,
          body: explained.body,
          detail: explained.cancelled ? undefined : explained.detail,
        });
      } finally {
        current.current = null;
        setTransfer(null);
        inFlight.current = false;
        release();
      }
      if (changed) onChanged(changed);
      return result;
    },
    [acquire, call, invalidate, onChanged, record, release],
  );

  const cancelTransfer = useCallback(async () => {
    const active = current.current;
    if (!active?.transferId || active.cancelling) return;
    const update = (step: string, cancelling: boolean) => {
      if (current.current?.entryId !== active.entryId) return;
      const next = { ...current.current, cancelling, steps: [...current.current.steps, step] };
      current.current = next;
      setTransfer(next);
    };
    update("Arrêt demandé", true);
    try {
      const result = await call<{ status: string }>({ action: "cancelTransfer", transferId: active.transferId });
      const publication = active.kind === "publish" || active.kind === "publishIntegrate" || active.kind === "candidates";
      update(
        result.status === "tooLate"
          ? publication
            ? "La publication a déjà été engagée ; attendez son reçu."
            : "L’application locale a déjà commencé ; attendez son résultat."
          : result.status === "notFound"
            ? "Annulation indisponible pour le moment. Réessayez si le transfert reste actif."
            : "Arrêt demandé. Le résultat du transfert précisera si l’annulation a pu être effectuée.",
        result.status !== "notFound",
      );
    } catch (cause) {
      update(`Demande d’arrêt non transmise : ${explainError(cause, "remote").title}`, false);
    }
  }, [call]);

  const transport = useCallback(<T,>(action: RemoteAction) => call<T>(action), [call]);

  const value = useMemo<RemoteApi>(
    () => ({
      root,
      origin,
      fixed,
      authority,
      session,
      epoch,
      login,
      transfer,
      activity,
      notice,
      clearNotice: () => setNotice(null),
      chooseServer,
      bindOrigin,
      saveAuthority,
      check,
      startLogin,
      pollLogin,
      cancelLogin,
      logout,
      operate,
      track,
      cancelTransfer,
      transport,
    }),
    [root, origin, fixed, authority, session, epoch, login, transfer, activity, notice, chooseServer, bindOrigin, saveAuthority, check, startLogin, pollLogin, cancelLogin, logout, operate, track, cancelTransfer, transport],
  );

  return <RemoteContext.Provider value={value}>{children}</RemoteContext.Provider>;
}

export function explainNotice(explained: Explained): Notice {
  return { tone: "danger", title: explained.title, body: explained.body, detail: explained.detail };
}

export function sessionName(session: Session) {
  return session.state === "connected" ? session.displayName || session.principalId : "";
}
