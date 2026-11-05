import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  nativeProjectionReady,
  projectionCaptureMatches,
  projectionStoreFiles,
  type NativeCapturedProjectionPatch,
  type NativeNamespacePreviewRequest,
  type NativeProjectFilesSnapshot,
  type NativeProjectionStatus,
  type NativeProjectionSelection,
} from "@layrs/ui";
import { expandSelectionRequest } from "./selection";
import { filesystemRecoveryRequest, type ConversionInspection } from "./offline";
import { nativeProjection } from "../tauri";
import { useRemote } from "./remote";
import { rememberTarget, rememberedTarget, type RemoteTarget } from "./serverMemory";
import { useWorkspace } from "./workspace";

export type NamespacePreview = { mutation: { kind: "create" | "rename" | "move"; path: string; source?: string }; local: { projectionGeneration: number } };
export type RestorePreview = { local: { before: Record<string, unknown>; after: Record<string, unknown>; operationId: string } };
export type DirectoryPreview = { local: { source: string; destination: string; sourceFiles: Record<string, unknown> } };
export type ResumeKind = "resumeInstallation" | "resumeNamespace" | "resumeRefresh" | "resumeRestore" | "resumeRebase";
export type ProjectionNotice = { tone: "success" | "info" | "warning"; text: string };

type PublishReply = {
  receipt: { candidateId?: string; receipt: { status: string; rejections?: { code: string }[] } };
  pendingLocalAcceptance: boolean;
};

type ProjectionApi = {
  data: NativeProjectionStatus | null;
  files: NativeProjectFilesSnapshot | null;
  /** Registry valid, no repair pending: local writes may be prepared. */
  ready: boolean;
  patch: NativeCapturedProjectionPatch | null;
  busy: boolean;
  notice: ProjectionNotice | null;
  clearNotice: () => void;
  target: RemoteTarget | null;
  setTarget: (target: RemoteTarget | null) => void;
  candidateId: string;
  history: NativeCapturedProjectionPatch[];
  namespacePreview: NamespacePreview | null;
  directoryPreview: DirectoryPreview | null;
  restorePreview: RestorePreview | null;
  lastRestore: string;
  clearPreviews: () => void;
  perform: (label: string, work: (revision: number) => Promise<void>) => Promise<void>;
  isCurrent: (revision: number) => boolean;
  /** Runs one native call after the previous ones: the local store admits one opener per process. */
  serial: <T>(work: () => Promise<T>) => Promise<T>;
  refresh: () => Promise<void>;
  resume: (kind: ResumeKind, inspection: ConversionInspection) => Promise<void>;
  capture: () => Promise<void>;
  publish: (candidate?: boolean) => Promise<void>;
  receive: () => Promise<void>;
  expandSelection: (expectedGeneration: number, selection: NativeProjectionSelection) => Promise<void>;
  previewNamespace: (request: NativeNamespacePreviewRequest) => Promise<void>;
  applyNamespace: () => Promise<void>;
  previewDirectory: (source: string, destination: string) => Promise<void>;
  applyDirectory: () => Promise<void>;
  loadHistory: () => Promise<void>;
  prepareRestore: (selection: { sequence: number; paths: string[] } | "inverse") => Promise<void>;
  applyRestore: () => Promise<void>;
};

const ProjectionContext = createContext<ProjectionApi | null>(null);

export function useProjection() {
  const value = useContext(ProjectionContext);
  if (!value) throw new Error("useProjection must be used inside <ProjectionProvider>");
  return value;
}

/** Limited-access folder: only authorized files, reserved names kept visible without content. */
export function ProjectionProvider({ children, mode = "ordinary" }: { children: ReactNode; mode?: "ordinary" | "inspection" }) {
  const workspace = useWorkspace();
  const remote = useRemote();
  const { root, acquire, release, setError } = workspace;
  const [data, setData] = useState<NativeProjectionStatus | null>(null);
  const [patch, setPatch] = useState<NativeCapturedProjectionPatch | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<ProjectionNotice | null>(null);
  const [target, setTargetState] = useState<RemoteTarget | null>(() => rememberedTarget(root));
  const [candidateId, setCandidateId] = useState("");
  const [history, setHistory] = useState<NativeCapturedProjectionPatch[]>([]);
  const [namespacePreview, setNamespacePreview] = useState<NamespacePreview | null>(null);
  const [directoryPreview, setDirectoryPreview] = useState<DirectoryPreview | null>(null);
  const [restorePreview, setRestorePreview] = useState<RestorePreview | null>(null);
  const [lastRestore, setLastRestore] = useState("");
  const generation = useRef(0);
  const active = useRef(false);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const serial = useCallback(<T,>(work: () => Promise<T>): Promise<T> => {
    const run = queue.current.then(work, work);
    queue.current = run.catch(() => undefined);
    return run;
  }, []);
  const { bindOrigin, transport, track } = remote;

  const clearPreviews = useCallback(() => {
    setNamespacePreview(null);
    setDirectoryPreview(null);
    setRestorePreview(null);
  }, []);

  const perform = useCallback(
    async (label: string, work: (revision: number) => Promise<void>) => {
      if (active.current || !acquire(label)) return;
      const revision = generation.current;
      active.current = true;
      setBusy(true);
      setNotice(null);
      setError("");
      try {
        if (revision === generation.current) await serial(() => work(revision));
      } catch (cause) {
        if (revision === generation.current) {
          // The local state may have moved: require a fresh read before any write.
          setData(null);
          clearPreviews();
          setError(String(cause));
        }
      } finally {
        if (revision === generation.current) {
          active.current = false;
          setBusy(false);
        }
        release();
      }
    },
    [acquire, release, setError, clearPreviews],
  );

  const isCurrent = useCallback((revision: number) => revision === generation.current, []);

  const accept = useCallback(
    (next: NativeProjectionStatus) => {
      projectionStoreFiles(next);
      setData(next);
      setPatch(next.latestCapture && projectionCaptureMatches(next, next.latestCapture) ? next.latestCapture : null);
      bindOrigin(next.identity.origin);
      if (next.pendingPublication) {
        const pending = next.pendingPublication.layerId;
        setTargetState((current) => (current?.layerId === pending ? current : { layerId: pending }));
      }
    },
    [bindOrigin],
  );

  // Inside `perform` the queue is already held: read directly there.
  const status = useCallback(() => {
    // Ordinary open can migrate a legacy source. Conversion and Diagnostic
    // must use read-only inspection, including refreshes after explicit recovery.
    if (modeRef.current === "inspection") return Promise.reject(new Error("Utilisez l’inspection en lecture seule de la source sur cette page."));
    return nativeProjection<NativeProjectionStatus>(root, { action: "status" });
  }, [root]);

  // A read: allowed while another view works, guarded against late answers.
  const refresh = useCallback(async () => {
    const revision = generation.current;
    setError("");
    try {
      const next = await serial(() => revision === generation.current ? status() : Promise.resolve(null));
      if (next && revision === generation.current) {
        accept(next);
        setNamespacePreview(null);
      }
    } catch (cause) {
      if (revision === generation.current) {
        setData(null);
        setError(String(cause));
      }
    }
  }, [serial, status, accept, setError]);

  useEffect(() => {
    generation.current++;
    active.current = false;
    setData(null);
    setPatch(null);
    clearPreviews();
    setHistory([]);
    setLastRestore("");
    setNotice(null);
    setTargetState(rememberedTarget(root));
    if (mode === "ordinary") void refresh();
    return () => {
      generation.current++;
    };
    // Changing read mode invalidates ordinary status without replacing the
    // native queue or permitting inspection routes to open/migrate the source.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root, mode]);

  let files: NativeProjectFilesSnapshot | null = null;
  try {
    if (data && mode === "ordinary") files = projectionStoreFiles(data);
  } catch {
    // A malformed registry stays read-only.
  }
  const ready = files !== null && nativeProjectionReady(files) && data?.needsRepair === false;

  const setTarget = useCallback(
    (next: RemoteTarget | null) => {
      rememberTarget(root, next);
      setTargetState(next);
    },
    [root],
  );

  const resume = useCallback(
    (kind: ResumeKind, inspection: ConversionInspection) =>
      perform("Reprise…", async (revision) => {
        await nativeProjection(root, filesystemRecoveryRequest(inspection, kind));
        const next = modeRef.current === "ordinary" ? await status() : null;
        if (revision === generation.current) {
          if (next) accept(next);
          clearPreviews();
          setNotice({ tone: "success", text: next ? "Opération locale reprise et vérifiée." : "Reprise exécutée. Relisez l’inspection du dossier pour vérifier son état." });
        }
      }),
    [perform, root, status, accept, clearPreviews],
  );

  const capture = useCallback(
    () =>
      perform("Capture…", async (revision) => {
        if (!data || !ready || data.capabilities.capture !== true) return;
        const result = await nativeProjection<NativeCapturedProjectionPatch>(root, { action: "capture", expectedGeneration: data.projection.generation });
        if (!projectionCaptureMatches(data, result)) throw new Error("Capture incohérente avec l’accès limité ouvert. Actualisez avant de poursuivre.");
        // The first capture after conversion changes provenance and publication
        // capabilities. Ordinary captures already return their exact local patch.
        const next = data.latestCaptureImported === true ? await status() : null;
        if (revision === generation.current) {
          if (next) accept(next);
          else setPatch(result);
          setNamespacePreview(null);
          setNotice({
            tone: "success",
            text: result.changes.length
              ? "Modifications autorisées capturées localement. Aucune publication n’a été effectuée."
              : "Aucune modification autorisée à capturer. Votre travail local est capturé.",
          });
        }
      }),
    [perform, data, ready, root, status, accept],
  );

  const publish = useCallback(
    (candidate = false) =>
      perform(candidate ? "Envoi pour revue…" : "Publication…", async (revision) => {
        if (!data || !patch || !ready || data.latestCaptureImported === true || data.capabilities.publish !== true || !target?.layerId) return;
        const result = await track(
          { kind: candidate ? "candidates" : "publish", label: candidate ? "Envoi pour revue" : "Publication de la capture", context: "publish" },
          async () => {
            const reply = await transport<PublishReply>({
              action: candidate ? "publishProjectionCandidate" : "publishProjection",
              patchId: patch.sequence,
              layerId: target.layerId,
            });
            const receipt = reply.receipt?.receipt;
            if (receipt?.status !== "applied")
              throw new Error(
                `Publication refusée : ${receipt?.rejections?.map((item) => item.code).join(", ") || receipt?.status || "reçu absent"}. La capture locale est conservée.`,
              );
            return reply;
          },
          (reply) =>
            candidate
              ? { tone: "success", title: "Candidat reçu par le serveur.", body: "Le Layer reste à sa version actuelle ; proposez une revue pour demander son intégration." }
              : reply.pendingLocalAcceptance
                ? {
                    tone: "warning",
                    unconfirmed: true,
                    title: "Publication acceptée par le serveur ; confirmation locale en attente.",
                    body: "Relancez la publication de la même capture vers la même cible pour terminer. Rien n’est publié deux fois.",
                  }
                : { tone: "success", title: "Publication confirmée par le serveur." },
        );
        if (revision !== generation.current) return;
        if (candidate && result.receipt.candidateId) setCandidateId(result.receipt.candidateId);
        try {
          const next = await status();
          if (revision === generation.current) {
            accept(next);
            setNamespacePreview(null);
            setNotice({
              tone: result.pendingLocalAcceptance ? "warning" : "success",
              text: candidate
                ? "Candidat reçu par le serveur. Le Layer reste à sa version actuelle ; proposez une revue pour demander son intégration."
                : result.pendingLocalAcceptance
                  ? "Publication confirmée par le serveur ; l’acceptation locale reste en attente. Réessayez la même capture et la même cible pour terminer."
                  : "Publication confirmée par le serveur.",
            });
          }
        } catch {
          if (revision === generation.current) {
            setData(null);
            setNotice({ tone: "warning", text: "Publication confirmée ; la relecture de l’état local est indisponible. Actualisez avant de poursuivre." });
          }
        }
      }),
    [perform, data, patch, ready, target, track, transport, status, accept],
  );

  const receive = useCallback(
    () =>
      perform("Réception…", async (revision) => {
        if (!data || !ready || data.capabilities.receive !== true || !target?.layerId) return;
        await track(
          { kind: "receive", label: "Réception des fichiers autorisés", context: "receive" },
          () => transport({ action: "receiveProjection", layerId: target.layerId }),
          () => ({ tone: "success", title: "Réception vérifiée.", body: "Les chemins réservés sont conservés." }),
        );
        const next = await status();
        if (revision === generation.current) {
          accept(next);
          setNamespacePreview(null);
          setNotice({ tone: "success", text: "Réception vérifiée. Les chemins réservés sont conservés." });
        }
      }),
    [perform, data, ready, target, track, transport, status, accept],
  );

  const expandSelection = useCallback(
    (expectedGeneration: number, selection: NativeProjectionSelection) =>
      perform("Élargissement de la sélection…", async (revision) => {
        // Re-read under the foreground queue, while retaining the generation the user saw.
        const before = await status();
        const request = expandSelectionRequest(before, expectedGeneration, selection);
        await track(
          { kind: "receive", label: "Téléchargement des chemins ajoutés", context: "receive" },
          () => transport(request),
          () => ({ tone: "success", title: "Sélection élargie et fichiers vérifiés." }),
        );
        const next = await status();
        if (revision === generation.current) {
          accept(next);
          clearPreviews();
          setNotice({ tone: "success", text: "Sélection élargie. Les fichiers disponibles et autorisés ont été téléchargés ; les autres chemins restent identifiés." });
        }
      }),
    [perform, status, track, transport, accept, clearPreviews],
  );

  const previewNamespace = useCallback(
    async (request: NativeNamespacePreviewRequest) => {
      if (!data || !ready || data.capabilities.namespace !== true) throw new Error("Modification de chemins indisponible dans cet état.");
      await perform("Préparation de l’aperçu…", async (revision) => {
        const { mutation } = request;
        const result = await transport<NamespacePreview>({
          action: "previewProjectionNamespace",
          expectedGeneration: request.anchor.generation,
          mutation: { kind: mutation.kind, path: mutation.destination, ...(mutation.kind !== "create" ? { source: mutation.source } : {}) },
        });
        const next = await status();
        if (revision === generation.current) {
          accept(next);
          setNamespacePreview(result);
        }
      });
    },
    [perform, data, ready, transport, status, accept],
  );

  const applyNamespace = useCallback(
    () =>
      perform("Application…", async (revision) => {
        if (!data || !ready || !namespacePreview || data.capabilities.namespace !== true) return;
        const captured = await transport<NativeCapturedProjectionPatch>({ action: "applyProjectionNamespace", preview: namespacePreview });
        const next = await status();
        if (!projectionCaptureMatches(next, captured)) throw new Error("La capture du changement ne correspond plus à l’accès limité. Actualisez avant de poursuivre.");
        if (revision === generation.current) {
          accept(next);
          setPatch(captured);
          setNamespacePreview(null);
          setNotice({ tone: "success", text: "Changement appliqué et capturé localement. Publiez cette capture pour modifier le projet distant." });
        }
      }),
    [perform, data, ready, namespacePreview, transport, status, accept],
  );

  const previewDirectory = useCallback(
    (source: string, destination: string) =>
      perform("Préparation de l’aperçu…", async (revision) => {
        if (!data || !ready) return;
        const result = await transport<DirectoryPreview>({
          action: "previewProjectionDirectoryMove",
          expectedGeneration: data.projection.generation,
          source: source.trim(),
          destination: destination.trim(),
        });
        const next = await status();
        if (revision === generation.current) {
          accept(next);
          setDirectoryPreview(result);
        }
      }),
    [perform, data, ready, transport, status, accept],
  );

  const applyDirectory = useCallback(
    () =>
      perform("Déplacement…", async (revision) => {
        if (!data || !ready || !directoryPreview) return;
        const result = await transport<NativeCapturedProjectionPatch>({ action: "applyProjectionDirectoryMove", preview: directoryPreview });
        const next = await status();
        if (!projectionCaptureMatches(next, result)) throw new Error("La capture du dossier ne correspond plus à l’accès limité.");
        if (revision === generation.current) {
          accept(next);
          setDirectoryPreview(null);
          setPatch(result);
          setNotice({ tone: "success", text: "Dossier déplacé et capturé localement." });
        }
      }),
    [perform, data, ready, directoryPreview, transport, status, accept],
  );

  const loadHistory = useCallback(
    () =>
      perform("Lecture de l’historique…", async (revision) => {
        const result = await nativeProjection<{ captures: NativeCapturedProjectionPatch[] }>(root, { action: "history", before: null, limit: 50 });
        if (revision === generation.current) setHistory(result.captures);
      }),
    [perform, root],
  );

  const prepareRestore = useCallback(
    (selection: { sequence: number; paths: string[] } | "inverse") =>
      perform("Préparation de l’aperçu…", async (revision) => {
        if (!data || !ready || data.capabilities.restore !== true) return;
        if (selection !== "inverse" && (!Number.isSafeInteger(selection.sequence) || selection.sequence < 1 || !selection.paths.length))
          throw new Error("Sélectionnez une capture et au moins un chemin exact.");
        const source = selection === "inverse" ? { kind: "inverse", operationId: lastRestore } : { kind: "capture", sequence: selection.sequence, paths: selection.paths };
        const result = await transport<RestorePreview>({ action: "previewProjectionRestore", expectedGeneration: data.projection.generation, source });
        if (revision === generation.current) {
          setNamespacePreview(null);
          setRestorePreview(result);
        }
      }),
    [perform, data, ready, lastRestore, transport],
  );

  const applyRestore = useCallback(
    () =>
      perform("Restauration…", async (revision) => {
        if (!data || !ready || !restorePreview || data.capabilities.restore !== true) return;
        const result = await transport<{ operationId: string; capture: NativeCapturedProjectionPatch }>({ action: "applyProjectionRestore", preview: restorePreview });
        const next = await status();
        if (!projectionCaptureMatches(next, result.capture)) throw new Error("La capture restaurée ne correspond plus à l’accès limité.");
        if (revision === generation.current) {
          accept(next);
          setPatch(result.capture);
          setLastRestore(result.operationId);
          setRestorePreview(null);
          setNotice({
            tone: "success",
            text: "Fichiers restaurés localement. Les octets précédents sont conservés pour annuler ; publiez la capture pour changer le projet distant.",
          });
        }
      }),
    [perform, data, ready, restorePreview, transport, status, accept],
  );

  const value = useMemo<ProjectionApi>(
    () => ({
      data: mode === "ordinary" ? data : null,
      files,
      ready,
      patch,
      busy,
      notice,
      clearNotice: () => setNotice(null),
      target,
      setTarget,
      candidateId,
      history,
      namespacePreview,
      directoryPreview,
      restorePreview,
      lastRestore,
      clearPreviews,
      perform,
      isCurrent,
      serial,
      refresh,
      resume,
      capture,
      publish,
      receive,
      expandSelection,
      previewNamespace,
      applyNamespace,
      previewDirectory,
      applyDirectory,
      loadHistory,
      prepareRestore,
      applyRestore,
    }),
    // `files` and `ready` derive from `data`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [mode, data, patch, busy, notice, target, setTarget, candidateId, history, namespacePreview, directoryPreview, restorePreview, lastRestore, clearPreviews, perform, isCurrent, refresh, resume, capture, publish, receive, expandSelection, previewNamespace, applyNamespace, previewDirectory, applyDirectory, loadHistory, prepareRestore, applyRestore],
  );

  return <ProjectionContext.Provider value={value}>{children}</ProjectionContext.Provider>;
}
