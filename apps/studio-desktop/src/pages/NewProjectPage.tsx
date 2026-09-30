import { useCallback, useRef, useState } from "react";
import { CloudDownload, FolderOpen, FolderPlus } from "lucide-react";
import { Badge, Banner, Button, Card, PageHeader, SegmentedControl, TextField, navigate, useLocation, type SpaceCatalogEntry } from "@layrs/ui";
import { nativeWorkspace, selectFolder } from "../tauri";
import { RemoteProvider, useRemote } from "../app/remote";
import { rememberProject } from "../app/recents";
import { ConnectFlow } from "../sync/ConnectFlow";
import { NoticeView, ServerSummary, TransferView } from "../sync/parts";
import { TeamCatalog } from "../sync/TeamCatalog";
import { openProject } from "./ProjectsPage";

type Mode = "initialize" | "clone";

/** P04: start working, adapted to the chosen intention. */
export function NewProjectPage() {
  const { query } = useLocation();
  const [mode, setMode] = useState<Mode>(query.get("mode") === "clone" ? "clone" : "initialize");
  const [root, setRoot] = useState(query.get("root") ?? "");
  const [error, setError] = useState("");

  async function browse() {
    setError("");
    try {
      const selected = await selectFolder(root);
      if (selected) setRoot(selected);
    } catch (cause) {
      setError(String(cause));
    }
  }

  const folderField = (hint: string) => (
    <div className="new-project__folder">
      <TextField label="Dossier" value={root} placeholder="Chemin du dossier" hint={hint} onChange={(event) => setRoot(event.target.value)} />
      <Button icon={<FolderOpen aria-hidden size={16} />} onClick={() => void browse()}>
        Parcourir…
      </Button>
    </div>
  );

  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader title="Nouveau projet" description="Que voulez-vous faire ?" />
      <SegmentedControl<Mode>
        label="Type de projet"
        value={mode}
        onChange={(next) => {
          setMode(next);
          setError("");
        }}
        options={[
          { value: "initialize", label: "Commencer dans un dossier", icon: <FolderPlus aria-hidden size={16} /> },
          { value: "clone", label: "Récupérer un projet de l’équipe", icon: <CloudDownload aria-hidden size={16} /> },
        ]}
      />
      {error && (
        <Banner tone="danger" title="Le dossier n’a pas pu être choisi">
          {error}
        </Banner>
      )}
      {mode === "initialize" ? (
        <Initialize root={root} folderField={folderField("Les fichiers déjà présents sont conservés.")} />
      ) : (
        <CloneProvider root={root}>
          <CloneWizard root={root} folderField={folderField("Choisissez un dossier vide : rien n’y est écrit avant la confirmation.")} />
        </CloneProvider>
      )}
    </div>
  );
}

function Initialize({ root, folderField }: { root: string; folderField: JSX.Element }) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function initialize() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await nativeWorkspace(root, { action: "initialize", name: name.trim() });
      rememberProject({ root, name: name.trim(), kind: "complete" });
      openProject(root);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Card className="new-project__card">
      {folderField}
      <TextField label="Nom du nouveau projet" value={name} onChange={(event) => setName(event.target.value)} />
      <p className="new-project__note">Layrs suit ce dossier et capture son état actuel. Aucun fichier n’est envoyé à un serveur.</p>
      {error && (
        <Banner tone="danger" title="Le projet n’a pas pu être préparé">
          <code className="sync-detail">{error}</code>
        </Banner>
      )}
      <div className="new-project__actions">
        <Button onClick={() => navigate("/projects")}>Annuler</Button>
        <Button
          variant="primary"
          busy={busy}
          busyLabel="Initialisation…"
          disabledReason={!root ? "Choisissez un dossier." : !name.trim() ? "Donnez un nom au projet." : undefined}
          onClick={() => void initialize()}
        >
          Initialiser
        </Button>
      </div>
    </Card>
  );
}

/** Connection state for a folder that is not a project yet. */
function CloneProvider({ root, children }: { root: string; children: JSX.Element }) {
  const running = useRef(false);
  const acquire = useCallback(() => {
    if (running.current) return false;
    running.current = true;
    return true;
  }, []);
  const release = useCallback(() => {
    running.current = false;
  }, []);
  // The project frame records the folder in recents once it reads its real kind.
  const onChanged = useCallback((next: string) => openProject(next), []);
  return (
    <RemoteProvider root={root} acquire={acquire} release={release} onChanged={onChanged}>
      {children}
    </RemoteProvider>
  );
}

function CloneWizard({ root, folderField }: { root: string; folderField: JSX.Element }) {
  const remote = useRemote();
  const { query } = useLocation();
  const [project, setProject] = useState<SpaceCatalogEntry | null>(null);
  const [reference, setReference] = useState("");
  const [step, setStep] = useState("");
  const [limited, setLimited] = useState(false);
  const connected = remote.session.state === "connected";
  const space = reference.trim() || project?.spaceId || "";
  const projection = limited || project?.access === "limited";
  const reason = !connected
    ? "Connectez-vous d’abord au serveur."
    : !space
      ? "Choisissez le projet à récupérer."
      : !root
        ? "Choisissez un dossier vide."
        : remote.transfer
          ? "Un transfert est déjà en cours."
          : projection && !/^blake3:[a-f0-9]{64}$/.test(step.trim())
            ? "Indiquez la capture publiée à récupérer."
            : undefined;

  function start() {
    if (projection) {
      rememberProject({ root, name: project?.name ?? "", kind: "projection" });
      void remote.operate(
        { kind: "clone", label: `Récupération des fichiers autorisés de ${project?.name ?? "ce projet"}`, busy: "Récupération…", context: "clone" },
        { action: "cloneProjection", space, step: step.trim() },
      );
    } else {
      void remote.operate(
        { kind: "clone", label: `Récupération de ${project?.name ?? "ce projet"}`, busy: "Récupération…", context: "clone", cancellable: true },
        { action: "clone", space, name: project?.name || space },
      );
    }
  }

  return (
    <>
      <section className="settings-section" aria-labelledby="clone-server">
        <h2 id="clone-server">1. Serveur de l’équipe</h2>
        {connected ? (
          <Card className="sync-card">
            <ServerSummary />
          </Card>
        ) : (
          <ConnectFlow />
        )}
      </section>
      <section className="settings-section" aria-labelledby="clone-project">
        <h2 id="clone-project">2. Projet</h2>
        <Card className="new-project__card">
          {connected ? (
            <TeamCatalog
              preselect={query.get("space")}
              selected={project?.spaceId}
              onSelect={(entry) => {
                setProject(entry);
                setReference("");
              }}
            />
          ) : (
            <p className="sync-muted">Connectez-vous pour voir les projets que votre compte peut ouvrir.</p>
          )}
          {project && (
            <p>
              Projet choisi : <strong>{project.name}</strong> <span className="sync-muted">· {project.workspace.name}</span>{" "}
              {project.access === "limited" && <Badge tone="warning">Accès limité</Badge>}
            </p>
          )}
          <details className="merge__advanced">
            <summary>Référence du projet (avancé)</summary>
            <TextField
              label="Identifiant du projet distant"
              value={reference}
              placeholder="space:…"
              hint="Remplace le choix dans la liste, par exemple pour une référence fournie par votre équipe."
              onChange={(event) => setReference(event.target.value)}
            />
          </details>
        </Card>
      </section>
      <section className="settings-section" aria-labelledby="clone-folder">
        <h2 id="clone-folder">3. Dossier de cet ordinateur</h2>
        <Card className="new-project__card">
          {folderField}
          {project?.access !== "limited" && (
            <details className="merge__advanced" open={limited || undefined}>
              <summary>Récupérer seulement les fichiers autorisés (avancé)</summary>
              <p className="sync-muted">
                Crée un accès limité : seuls les fichiers autorisés sont écrits ; les chemins interdits restent visibles et réservés.
              </p>
              <label className="catalog__check">
                <input type="checkbox" checked={limited} onChange={(event) => setLimited(event.target.checked)} /> Récupérer en accès limité
              </label>
            </details>
          )}
          {projection && (
            <>
              <Banner tone="info" title="Accès limité">
                Seuls les fichiers autorisés sont téléchargés ; les chemins interdits restent visibles et réservés. Le serveur ne fournit pas encore la
                capture publiée à ce type d’accès : indiquez celle que vous a transmise votre équipe.
              </Banner>
              <TextField label="Capture publiée à récupérer" value={step} placeholder="blake3:…" onChange={(event) => setStep(event.target.value)} />
            </>
          )}
          {remote.transfer && <TransferView transfer={remote.transfer} />}
          <NoticeView notice={remote.notice} onClose={remote.clearNotice} />
          <div className="new-project__actions">
            <Button onClick={() => navigate("/projects")} disabled={Boolean(remote.transfer)}>
              Annuler
            </Button>
            <Button variant="primary" icon={<CloudDownload aria-hidden size={16} />} busy={Boolean(remote.transfer)} busyLabel="Récupération…" disabledReason={reason} onClick={start}>
              {projection ? "Récupérer les fichiers autorisés" : "Récupérer le projet"}
            </Button>
          </div>
          <p className="new-project__note">Le dossier doit être vide. En cas d’annulation ou d’échec, aucun dossier existant n’est effacé.</p>
        </Card>
      </section>
    </>
  );
}
