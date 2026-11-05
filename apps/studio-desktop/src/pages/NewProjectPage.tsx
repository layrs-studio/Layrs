import { useCallback, useEffect, useRef, useState } from "react";
import { CloudDownload, FolderOpen, FolderPlus } from "lucide-react";
import { Badge, Banner, Button, Card, PageHeader, SegmentedControl, TextField, navigate, useLocation, type SpaceCatalogEntry } from "@layrs/ui";
import { nativeWorkspace, selectFolder } from "../tauri";
import { RemoteProvider, useRemote } from "../app/remote";
import { rememberProject } from "../app/recents";
import { ConnectFlow } from "../sync/ConnectFlow";
import { NoticeView, ServerSummary, TransferView } from "../sync/parts";
import { TeamCatalog } from "../sync/TeamCatalog";
import { openProject } from "./ProjectsPage";
import { SelectionFields } from "../app/SelectionFields";
import { cloneSelectedRequest, selectionFromRules, type SelectionRule } from "../app/selection";
import { useDesktopContexts } from "../app/contexts";
import { rememberRecentContext } from "../app/recentContext";
import { explainError } from "../app/errors";
import { CloneDestinationNotice, useCloneDestination } from "./CloneDestination";

type Mode = "initialize" | "clone" | "server";
const validSlug = /^[a-z0-9][a-z0-9-]{0,62}$/;
function slugFromName(name: string) {
  return name.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 63).replace(/-+$/g, "");
}
type LensPin = { lensId: string; version: string; digest: string };
type LensCatalogItem = { manifest: { id: string; name: string; analyzer: { fileExtensions: string[] } }; pin: LensPin; source: string; state: string; available: boolean };
type LensCatalog = { items: LensCatalogItem[] } | LensCatalogItem[];

/** P04: start working, adapted to the chosen intention. */
export function NewProjectPage() {
  const { query } = useLocation();
  const [mode, setMode] = useState<Mode>(query.get("mode") === "clone" ? "clone" : query.get("mode") === "server" ? "server" : "initialize");
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
          { value: "server", label: "Créer sur le serveur", icon: <FolderPlus aria-hidden size={16} /> },
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
      ) : mode === "server" ? (
        <CloneProvider root={root}><ServerProjectWizard root={root} folderField={folderField("Choisissez un dossier vide pour récupérer le projet créé sur le serveur.")} /></CloneProvider>
      ) : (
        <CloneProvider root={root}>
          <CloneWizard root={root} folderField={folderField("Choisissez un dossier : Layrs vérifie son contenu avant de proposer une action.")} />
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
    <RemoteProvider root={root} acquire={acquire} release={release} onChanged={onChanged} announceConnection>
      {children}
    </RemoteProvider>
  );
}

function ServerProjectWizard({ root, folderField }: { root: string; folderField: JSX.Element }) {
  const remote = useRemote();
  const contexts = useDesktopContexts();
  const destination = useCloneDestination();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugEdited, setSlugEdited] = useState(false);
  const [lenses, setLenses] = useState<LensCatalogItem[]>([]);
  const [pins, setPins] = useState<Record<string, string>>({});
  const [loadingLenses, setLoadingLenses] = useState(false);
  const [lensLoaded, setLensLoaded] = useState(false);
  const [catalogAttempt, setCatalogAttempt] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const connected = remote.session.state === "connected" && remote.origin === contexts.origin && remote.session.principalId === contexts.principalId;
  const identity = JSON.stringify([root, remote.origin, remote.session.state === "connected" ? remote.session.principalId : null, contexts.selected?.id, remote.epoch]);
  const currentIdentity = useRef(identity);
  currentIdentity.current = identity;
  useEffect(() => () => { currentIdentity.current = ""; }, []);
  useEffect(() => {
    if (!connected) { setLenses([]); setLensLoaded(false); return; }
    let active = true;
    setLoadingLenses(true);
    setLensLoaded(false);
    void remote.transport<LensCatalog>({ action: "administration", command: { action: "lensCatalog" } }, { root: remote.root, origin: remote.origin, principalId: contexts.principalId }).then((result) => {
      if (!active) return;
      const available = (Array.isArray(result) ? result : result.items).filter((item) => item.available && item.state === "active");
      setLenses(available);
      setLensLoaded(true);
      const defaults: Record<string, string> = {};
      for (const item of available) if (item.source === "builtin" && !defaults[item.pin.lensId]) defaults[item.pin.lensId] = item.pin.digest;
      setPins(defaults);
    }).catch((cause) => { if (active) setError(explainError(cause, "remote").detail); })
      .finally(() => { if (active) setLoadingLenses(false); });
    return () => { active = false; };
  }, [connected, remote.transport, remote.origin, remote.root, contexts.principalId, catalogAttempt]);
  const chosen = lenses.filter((item) => pins[item.pin.lensId] === item.pin.digest);
  const associations = new Map<string, string>();
  for (const item of chosen) for (const raw of item.manifest.analyzer.fileExtensions) {
    const extension = raw.startsWith(".") ? raw : `.${raw}`;
    if (!associations.has(extension)) associations.set(extension, item.pin.lensId);
  }
  async function create() {
    if (!connected || !contexts.selected?.canCreateProject || !root || !name.trim() || !validSlug.test(slug) || busy || !lensLoaded || destination.result?.kind !== "empty") return;
    setBusy(true);
    setError("");
    try {
      const request = { contextId: contexts.selected.id, spaceSlug: slug, name: name.trim(), lensConfiguration: { pins: chosen.map((item) => item.pin), associations: [...associations].map(([extension, lensId]) => ({ extension, lensId })) } };
      const result = await remote.transport<{ spaceId: string }>({ action: "createProject", request }, { root: remote.root, origin: remote.origin, principalId: contexts.principalId });
      if (currentIdentity.current !== identity) throw new Error("Le projet a été créé sur le serveur, mais le dossier ou la connexion a changé. Choisissez Récupérer un projet pour continuer avec la destination souhaitée.");
      const cloned = await remote.operate({ kind: "clone", label: `Récupération de ${name.trim()}`, busy: "Récupération…", context: "clone", cancellable: true }, { action: "clone", space: result.spaceId, name: name.trim() });
      if (!cloned) throw new Error("Le projet a été créé sur le serveur, mais sa récupération a échoué. Choisissez Récupérer un projet pour reprendre.");
      if (cloned.state?.worktree.checkoutRoot) rememberRecentContext(cloned.state.worktree.checkoutRoot, remote.origin, contexts.principalId, request.contextId, result.spaceId);
    } catch (cause) { setError(explainError(cause, "remote").detail); }
    finally { setBusy(false); }
  }
  const groups = new Map<string, LensCatalogItem[]>();
  for (const item of lenses) groups.set(item.pin.lensId, [...(groups.get(item.pin.lensId) ?? []), item]);
  return <>
    <section className="settings-section"><h2>Serveur</h2>{connected ? <Card className="sync-card"><ServerSummary /><p>Espace : <strong>{contexts.selected?.kind === "personal" ? "Personnel" : contexts.selected?.name}</strong></p></Card> : <ConnectFlow />}</section>
    <section className="settings-section"><h2>Projet sur le serveur</h2><Card className="new-project__card">
      <TextField label="Nom du projet" value={name} maxLength={255} onChange={(event) => { setName(event.target.value); if (!slugEdited) setSlug(slugFromName(event.target.value)); }} />
      <TextField label="Identifiant du projet" value={slug} maxLength={63} hint="Unique dans l’espace choisi ; lettres minuscules, chiffres et tirets." onChange={(event) => { setSlugEdited(true); setSlug(event.target.value); }} />
      {loadingLenses ? <p>Chargement des lenses…</p> : !lensLoaded && connected ? <Button onClick={() => { setError(""); setCatalogAttempt((value) => value + 1); }}>Réessayer le chargement des lenses</Button> : groups.size > 0 && <fieldset className="new-project-form__lenses"><legend>Lenses du projet</legend>{[...groups].map(([lensId, versions]) => <label className="new-project-form__field" key={lensId}>{versions[0].manifest.name}<select value={pins[lensId] ?? ""} onChange={(event) => setPins((current) => ({ ...current, [lensId]: event.target.value }))}><option value="">Non activée</option>{versions.map((item) => <option key={`${item.pin.version}:${item.pin.digest}`} value={item.pin.digest}>Version {item.pin.version}</option>)}</select></label>)}</fieldset>}
    </Card></section>
    <section className="settings-section"><h2>Dossier de cet ordinateur</h2><Card className="new-project__card">{folderField}<p>Le dossier est vérifié avant la création du projet sur le serveur.</p><CloneDestinationNotice inspection={destination} />{error && <Banner tone="danger" title="Création impossible">{error}</Banner>}<NoticeView notice={remote.notice} onClose={remote.clearNotice} /><div className="new-project__actions"><Button onClick={() => navigate("/projects")}>Annuler</Button><Button variant="primary" busy={busy} disabledReason={!connected ? "Connectez-vous au serveur." : !contexts.selected?.canCreateProject ? "Votre compte ne peut pas créer de projet dans cet espace." : !root || !name.trim() || !validSlug.test(slug) ? "Complétez le projet et choisissez un dossier vide." : destination.result?.kind !== "empty" ? "La création d’un nouveau projet nécessite un dossier vérifié et vide." : !lensLoaded ? "Attendez le catalogue des lenses." : undefined} onClick={() => void create()}>Créer et récupérer</Button></div></Card></section>
  </>;
}

function CloneWizard({ root, folderField }: { root: string; folderField: JSX.Element }) {
  const remote = useRemote();
  const contexts = useDesktopContexts();
  const { query } = useLocation();
  const [project, setProject] = useState<SpaceCatalogEntry | null>(null);
  const [projectContextId, setProjectContextId] = useState("");
  const [reference, setReference] = useState("");
  useEffect(() => { setProject(null); setProjectContextId(""); setReference(""); }, [contexts.selected?.id, remote.origin, contexts.principalId]);
  const [step, setStep] = useState("");
  const [limited, setLimited] = useState(false);
  const [selectionMode, setSelectionMode] = useState<"all" | "selected">("all");
  const [rules, setRules] = useState<SelectionRule[]>([]);
  const connected = remote.session.state === "connected" && remote.origin === contexts.origin && remote.session.principalId === contexts.principalId;
  const currentProject = projectContextId === contexts.selected?.id ? project : null;
  const space = reference.trim() || currentProject?.spaceId || "";
  const destination = useCloneDestination(space);
  const projection = limited || currentProject?.access === "limited" || selectionMode === "selected";
  let selectedRequest: ReturnType<typeof cloneSelectedRequest> | undefined;
  let selectionError: string | undefined;
  if (selectionMode === "selected") {
    try {
      const selection = selectionFromRules(rules, navigator.platform.startsWith("Win") ? "portable_windows" : "case_sensitive");
      selectedRequest = cloneSelectedRequest(space, step, selection);
    } catch (cause) { selectionError = cause instanceof Error ? cause.message : String(cause); }
  }
  const reason = !connected
    ? "Connectez-vous d’abord au serveur."
    : !space
      ? "Choisissez le projet à récupérer."
      : !root
        ? "Choisissez un dossier."
        : remote.transfer
          ? "Un transfert est déjà en cours."
          : destination.result?.kind !== "empty"
            ? "Choisissez un dossier vide et vérifié, ou utilisez l’action proposée pour le projet existant."
          : projection && !/^blake3:[a-f0-9]{64}$/.test(step.trim())
            ? "Indiquez la capture publiée à récupérer."
            : selectionError;

  function start() {
    if (reason) return;
    if (projection) {
      rememberProject({ root, name: currentProject?.name ?? "", kind: "projection" });
      void remote.operate(
        { kind: "clone", label: `Récupération des fichiers autorisés de ${currentProject?.name ?? "ce projet"}`, busy: "Récupération…", context: "clone" },
        selectedRequest ?? { action: "cloneProjection", space, step: step.trim() },
      ).then((result) => { if (result && currentProject && contexts.selected && remote.session.state === "connected") rememberRecentContext(root, remote.origin, remote.session.principalId, contexts.selected.id, currentProject.spaceId); });
    } else {
      void remote.operate(
        { kind: "clone", label: `Récupération de ${currentProject?.name ?? "ce projet"}`, busy: "Récupération…", context: "clone", cancellable: true },
        { action: "clone", space, name: currentProject?.name || space },
      ).then((result) => { if (result && currentProject && contexts.selected && remote.session.state === "connected") rememberRecentContext(root, remote.origin, remote.session.principalId, contexts.selected.id, currentProject.spaceId); });
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
              selected={currentProject?.spaceId}
              onSelect={(entry) => {
                setProject(entry);
                setProjectContextId(contexts.selected?.id ?? "");
                setReference("");
              }}
            />
          ) : (
            <p className="sync-muted">Connectez-vous pour voir les projets que votre compte peut ouvrir.</p>
          )}
          {currentProject && (
            <p>
              Projet choisi : <strong>{currentProject.name}</strong> <span className="sync-muted">· {currentProject.workspace.name}</span>{" "}
              {currentProject.access === "limited" && <Badge tone="warning">Accès limité</Badge>}
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
          <CloneDestinationNotice inspection={destination} projection={projection} />
          <SegmentedControl<"all" | "selected"> label="Fichiers à récupérer" value={selectionMode} onChange={setSelectionMode} options={[
            { value: "all", label: "Tous les fichiers autorisés" },
            { value: "selected", label: "Choisir des chemins" },
          ]} />
          {selectionMode === "selected" && <SelectionFields rules={rules} onChange={setRules} disabled={Boolean(remote.transfer)} error={selectionError} />}
          {currentProject?.access !== "limited" && (
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
              <Banner tone="info" title={selectionMode === "selected" ? "Sélection partielle" : "Accès limité"}>
                Seuls les fichiers sélectionnés et autorisés sont téléchargés ; les chemins interdits restent réservés. Les chemins exclus par votre choix
                restent identifiés et peuvent être ajoutés ensuite. Indiquez l’identifiant exact de la capture publiée à récupérer.
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
          <p className="new-project__note">Un clonage utilise un dossier vide. Pour un projet Layrs existant, choisissez le lien, la synchronisation ou la publication proposés ci-dessus. Aucun dossier existant n’est effacé.</p>
        </Card>
      </section>
    </>
  );
}
