import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import {
  Camera,
  CheckCircle2,
  ChevronDown,
  File,
  FileCode2,
  FileImage,
  FileJson,
  FileText,
  History,
  MoreVertical,
} from "lucide-react";
import {
  Badge,
  Banner,
  Button,
  DiffView,
  LensViewer,
  EmptyState,
  Kbd,
  Link,
  Menu,
  PageHeader,
  SegmentedControl,
  SelectField,
  StatusDot,
  Switch,
  diffStats,
  formatBytes,
  formatRelativeTime,
  navigate,
  segment,
  useToast,
  type DiffMode,
} from "@layrs/ui";
import { useWorkspace, type FileChange } from "../app/workspace";
import { useCapturedChanges } from "./useCapturedChanges";

const codeExtensions = /\.(tsx?|jsx?|mjs|cjs|rs|py|go|java|kt|swift|c|cc|cpp|h|hpp|cs|rb|php|css|scss|html?|vue|svelte|sh|ps1|sql|toml|ya?ml)$/i;

function FileIcon({ path }: { path: string }) {
  if (/\.(png|jpe?g|gif|webp|svg|ico|bmp|avif)$/i.test(path)) return <FileImage aria-hidden size={20} />;
  if (/\.json$/i.test(path)) return <FileJson aria-hidden size={20} />;
  if (codeExtensions.test(path)) return <FileCode2 aria-hidden size={20} />;
  if (/\.(md|txt|rst|adoc)$/i.test(path)) return <FileText aria-hidden size={20} />;
  return <File aria-hidden size={20} />;
}

const statusLabel: Record<string, { label: string; tone: "success" | "danger" | "info" }> = {
  added: { label: "Ajouté", tone: "success" },
  deleted: { label: "Supprimé", tone: "danger" },
  modified: { label: "Modifié", tone: "info" },
};

function splitPath(path: string) {
  const index = path.lastIndexOf("/");
  return index < 0 ? { name: path, directory: "" } : { name: path.slice(index + 1), directory: path.slice(0, index) };
}

export function ChangesPage() {
  const workspace = useWorkspace();
  const { data, busy, conflicted, activeLayer, automatic, setAutomatic, autoActivity } = workspace;
  const toast = useToast();
  const pendingChanges = useMemo(() => data?.changes ?? [], [data]);
  const own = data?.state.view.worktrees.find((item) => item.worktreeId === data.state.worktree.worktreeId);
  const captured = useCapturedChanges(workspace.root, own?.worktreeId, own?.activeLayerId, own?.baseStepId);
  const showingCapture = Boolean(own?.baseStepId && !pendingChanges.length && !conflicted && !data?.needsRepair);
  const changes = showingCapture ? captured.changes ?? [] : pendingChanges;
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [mode, setMode] = useState<DiffMode>("split");
  const listRef = useRef<HTMLUListElement>(null);
  const selected = changes.find((change) => change.path === selectedPath) ?? changes[0];
  const projectKey = segment(workspace.root);

  // Keep a valid selection when the list changes underneath (automatic capture, edits).
  useEffect(() => {
    if (selectedPath && !changes.some((change) => change.path === selectedPath)) setSelectedPath(null);
  }, [changes, selectedPath]);

  const lastCapture = captured.observation;
  const captureTitle = lastCapture ? "Dernière capture" : "Capture de référence";
  const autoEnabled = automatic && activeLayer?.settings.automaticCapture;

  const captureBlocked = !data
    ? "Le projet n’est pas encore ouvert."
    : data.needsRepair
      ? "Une opération interrompue doit d’abord être reprise."
      : conflicted
        ? "Résolvez d’abord les conflits de la fusion en cours."
        : !pendingChanges.length
          ? "Aucun changement à capturer."
          : undefined;

  async function capture() {
    if (captureBlocked || busy) return;
    const next = await workspace.run({ action: "capture" }, "Capture…");
    if (next) {
      toast.show({
        tone: "success",
        title: "Capture locale créée",
        description: "Elle est enregistrée sur cet ordinateur. Publiez-la depuis Synchronisation pour la partager.",
        action: { label: "Voir l’historique", onSelect: () => navigate(`/projects/${projectKey}/history`) },
      });
    }
  }

  useEffect(() => {
    function onKeyDown(event: globalThis.KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key === "Enter" && !document.querySelector("dialog[open]")) {
        event.preventDefault();
        void capture();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  function onListKeyDown(event: KeyboardEvent<HTMLUListElement>) {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const index = changes.findIndex((change) => change.path === selected?.path);
    const next = changes[Math.max(0, Math.min(changes.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)))];
    if (!next) return;
    setSelectedPath(next.path);
    listRef.current?.querySelector<HTMLButtonElement>(`[data-path="${CSS.escape(next.path)}"]`)?.focus();
  }

  const layerName = activeLayer?.name ?? "ce Layer";
  const description = !data
    ? "Lecture de l’état local…"
    : pendingChanges.length
      ? `${pendingChanges.length} fichier${pendingChanges.length > 1 ? "s" : ""} modifié${pendingChanges.length > 1 ? "s" : ""} dans ${layerName}`
      : `Votre travail local est capturé dans ${layerName}`;

  return (
    <div className="changes">
      <div className="changes__header">
        <PageHeader
          title="Changements"
          description={description}
          aside={
            <div className="changes__auto">
              <Switch
                label="Capture automatique"
                hideLabel
                checked={automatic}
                onChange={setAutomatic}
              />
              <div className="changes__auto-status">
                <StatusDot tone={automatic && activeLayer?.settings.automaticCapture ? "accent" : "info"}>
                  <span className="changes__auto-title">
                    {autoActivity === "capturing"
                      ? "Capture automatique en cours…"
                      : automatic && activeLayer?.settings.automaticCapture
                        ? "Capture automatique active"
                        : activeLayer?.settings.automaticCapture === false
                          ? "Capture automatique désactivée pour ce Layer"
                          : "Capture automatique en pause"}
                  </span>
                </StatusDot>
                <span className="changes__auto-meta">
                  {lastCapture ? `Dernière capture ${formatRelativeTime(lastCapture.observedAt)}` : own?.baseStepId ? "État capturé disponible" : "Aucune capture pour l’instant"}
                </span>
              </div>
            </div>
          }
        />
      </div>

      <p className="changes__composer-hint">
        {autoEnabled
          ? "Layrs capture automatiquement vos fichiers après une pause dans vos modifications."
          : "La capture automatique est en pause : capturez vos modifications avec le bouton ci-dessous."}
        {" "}« En cours » montre les différences depuis la capture la plus récente, automatique ou manuelle.
        {" "}Pour retrouver une étape importante, <Link to={`/projects/${projectKey}/versions`}>nommez une version</Link>.
      </p>

      {conflicted && (
        <Banner
          tone="warning"
          title="Fusion en cours avec des conflits"
          actions={
            <Button size="sm" onClick={() => navigate(`/projects/${projectKey}/merge`)}>
              Résoudre les conflits
            </Button>
          }
        >
          Les fichiers affichés correspondent encore à l’état précédent. Choisissez les variantes avant de capturer.
        </Banner>
      )}
      {data?.needsRepair && (
        <Banner
          tone="danger"
          title="Une écriture a été interrompue"
          actions={
            <Button size="sm" onClick={() => navigate(`/projects/${projectKey}/health`)}>
              Ouvrir le diagnostic
            </Button>
          }
        >
          Vos sauvegardes sont conservées. Reprenez l’opération avant de continuer.
        </Banner>
      )}

      {showingCapture && <Banner tone="info" title="Aucun changement en attente de capture">
        {lastCapture
          ? "La dernière capture a enregistré ces modifications. Elles restent visibles ci-dessous jusqu’à vos prochaines modifications."
          : "Le dossier correspond à sa capture de référence. Les différences de cette capture sont affichées ci-dessous ; elle n’a pas été créée dans ce dossier."}
        {" "}<Link to={`/projects/${projectKey}/sync`}>Ouvrir la synchronisation pour publier ou vérifier l’état du serveur</Link>.
      </Banner>}
      {showingCapture && captured.loading && <p role="status">Lecture des fichiers de la dernière capture…</p>}
      {showingCapture && captured.error && <Banner tone="warning" title="La dernière capture n’a pas pu être affichée" actions={<Button onClick={captured.retry}>Réessayer</Button>}>{captured.error}</Banner>}
      {showingCapture && (captured.parents?.length ?? 0) > 1 && <SelectField label="Comparer la fusion à une capture parente" value={captured.parent} onChange={(event) => captured.setParent(event.target.value)}>
        <option value="">Choisissez la capture de référence</option>
        {captured.parents?.map((parent, index) => <option key={parent} value={parent}>Parent {index + 1} · {parent.slice(7, 19)}</option>)}
      </SelectField>}
      <div className="changes__body">
        <section className="changes__files" aria-label={showingCapture ? lastCapture ? "Fichiers de la dernière capture" : "Fichiers de la capture de référence" : "Fichiers modifiés"}>
          <h2 className="changes__files-title">
            <ChevronDown aria-hidden size={16} />
            {showingCapture ? captureTitle : "En cours"} <span className="changes__count">({changes.length})</span>
            {showingCapture && lastCapture && <Badge tone="info">{lastCapture.captureReason === "automatic_idle" ? "Automatique" : "Locale"}</Badge>}
          </h2>
          {changes.length ? (
            <ul ref={listRef} className="changes__list" onKeyDown={onListKeyDown}>
              {changes.map((change) => (
                <FileRow key={change.path} change={change} selected={change.path === selected?.path} onSelect={() => setSelectedPath(change.path)} />
              ))}
            </ul>
          ) : (
            <EmptyState compact icon={<CheckCircle2 size={22} />} title={showingCapture ? "Aucune différence affichée." : "Aucune modification en attente."}>
              {showingCapture ? captured.parents && captured.parents.length > 1 && !captured.parent ? "Choisissez un parent pour comparer cette fusion." : "Les captures précédentes restent disponibles dans l’historique." : "Après capture automatique, les fichiers passent dans la dernière capture."}
            </EmptyState>
          )}
        </section>

        <section className="changes__diff" aria-label="Comparaison">
          {selected ? (
            <>
              <header className="changes__diff-head">
                <FileIcon path={selected.path} />
                <h2 className="changes__diff-path" title={selected.path}>
                  {selected.path}
                </h2>
                <Badge tone={statusLabel[selected.status]?.tone ?? "info"}>{statusLabel[selected.status]?.label ?? selected.status}</Badge>
                <div className="changes__diff-tools">
                  <SegmentedControl<DiffMode>
                    label="Présentation de la comparaison"
                    size="sm"
                    value={mode}
                    onChange={setMode}
                    options={[
                      { value: "split", label: "Côte à côte" },
                      { value: "inline", label: "En ligne" },
                    ]}
                  />
                  <Menu
                    label="Actions sur le fichier"
                    trigger={<MoreVertical aria-hidden size={18} />}
                    items={[
                      {
                        id: "history",
                        label: "Historique du fichier",
                        icon: <History aria-hidden size={16} />,
                        onSelect: () => navigate(`/projects/${projectKey}/history`, { query: { path: selected.path } }),
                      },
                      {
                        id: "copy",
                        label: "Copier le chemin",
                        onSelect: () =>
                          void navigator.clipboard?.writeText(selected.path).then(
                            () => toast.show({ tone: "success", title: "Chemin copié" }),
                            () => toast.show({ tone: "warning", title: "Copie impossible", description: "Sélectionnez le chemin manuellement." }),
                          ),
                      },
                    ]}
                  />
                </div>
              </header>
              <DiffContent change={selected} mode={mode} />
            </>
          ) : (
            <EmptyState
              icon={<Camera size={22} />}
              title={data ? "Votre travail local est capturé" : "Lecture de l’état local…"}
            >
              {data && (
                <>
                  {showingCapture ? "Cette vue montre la capture enregistrée, sans modifier vos fichiers. " : <>Modifiez des fichiers dans <code>{workspace.root}</code> pour les comparer. </>}
                  <Link to={`/projects/${projectKey}/history`}>Voir l’historique</Link>
                </>
              )}
            </EmptyState>
          )}
        </section>
      </div>

      <footer className="changes__composer" aria-label="Créer une capture">
        <div className="changes__composer-text">
          <p className="changes__composer-title">{autoEnabled ? "Capturer maintenant, sans attendre" : `Capturer les modifications de ${layerName}`}</p>
          <p className="changes__composer-hint">
            Une capture enregistre l’état du dossier sur cet ordinateur. La publication se fait depuis Synchronisation.
          </p>
        </div>
        <span className="changes__composer-keys" aria-hidden>
          <Kbd>Ctrl</Kbd>
          <Kbd>Entrée</Kbd>
        </span>
        <Button
          variant="primary"
          size="lg"
          icon={<Camera aria-hidden size={18} />}
          busy={busy === "Capture…"}
          busyLabel="Capture…"
          disabledReason={busy && busy !== "Capture…" ? `Opération en cours : ${busy}` : captureBlocked}
          aria-keyshortcuts="Control+Enter"
          onClick={() => void capture()}
        >
          Créer une capture
        </Button>
      </footer>
    </div>
  );
}

function FileRow({ change, selected, onSelect }: { change: FileChange; selected: boolean; onSelect: () => void }) {
  const { name, directory } = splitPath(change.path);
  const stats = change.textDiff ? diffStats(change.textDiff) : null;
  const status = statusLabel[change.status];
  return (
    <li>
      <button type="button" data-path={change.path} className="changes__file" aria-current={selected ? "true" : undefined} onClick={onSelect}>
        <span className={`changes__file-icon changes__file-icon--${change.status}`}>
          <FileIcon path={change.path} />
        </span>
        <span className="changes__file-text">
          <span className="changes__file-name">{name}</span>
          <span className="changes__file-dir">{directory || "Racine du projet"}</span>
        </span>
        <span className="layrs-visually-hidden">{status?.label ?? change.status}</span>
        {stats ? (
          <span className="changes__file-stats" aria-label={`${stats.added} lignes ajoutées, ${stats.removed} supprimées`}>
            <span className="changes__added">+{stats.added}</span>
            <span className="changes__removed">−{stats.removed}</span>
          </span>
        ) : (
          <span className="changes__file-kind">{change.binary ? "Binaire" : "Volumineux"}</span>
        )}
      </button>
    </li>
  );
}

function DiffContent({ change, mode }: { change: FileChange; mode: DiffMode }) {
  if (change.binary)
    return (
      <EmptyState compact icon={<FileImage size={22} />} title="Contenu binaire modifié">
        Pas d’aperçu pour ce format. Taille : {formatBytes(change.beforeBytes)} → {formatBytes(change.afterBytes)}.
      </EmptyState>
    );
  if (change.textOmitted || !change.textDiff)
    return (
      <EmptyState compact icon={<FileText size={22} />} title="Fichier trop volumineux pour cet aperçu">
        Taille : {formatBytes(change.beforeBytes)} → {formatBytes(change.afterBytes)}. Ouvrez-le dans votre éditeur pour le comparer.
      </EmptyState>
    );
  return <LensViewer input={{ kind: "diff", path: change.path, lines: change.textDiff }} fallback={<DiffView lines={change.textDiff} mode={mode} />} />;
}
