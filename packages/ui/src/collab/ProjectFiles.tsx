import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronRight, File, FileWarning, Folder, FolderOpen, History, Link2, Lock, RefreshCw, Search } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, EmptyState } from "../components/Feedback";
import { SelectField, TextField } from "../components/Field";
import {
  validateNativeProjectionContext,
  validateNativeProjectionPage,
  type NativeProjectionContext,
  type NativeProjectionPage,
  type NativeProjectionPageEntry,
  type NativeProjectionPageRequest,
} from "../native-remote-project-files-model";
import { accessLabel, denied, findFile, folderChildren, kindLabel, searchFiles, type FileRow } from "./model";
import type { CollabCapabilities } from "./types";
import { CollabError, CopyButton, DelayedRows, LockedContent, TechnicalDetails } from "./parts";
import { useScopedTasks } from "./useScopedTasks";

const PAGE_SIZE = 100;
const MAX_ENTRIES = 100_000;

/**
 * P06 — published files of a project, as the server authorizes them for this
 * account. Pages are observations at one authorization epoch: they are
 * validated, appended in order and never reused after a refresh.
 */
export function ProjectFiles({
  scope,
  loadContext,
  loadPage,
  onOpenHistory,
  capabilities,
}: {
  scope: string;
  capabilities?: CollabCapabilities;
  loadContext: () => Promise<NativeProjectionContext>;
  loadPage: (request: NativeProjectionPageRequest) => Promise<NativeProjectionPage>;
  onOpenHistory?: (path: string) => void;
}) {
  const tasks = useScopedTasks(scope);
  const [context, setContext] = useState<NativeProjectionContext | null>(null);
  const [step, setStep] = useState("");
  const [entries, setEntries] = useState<NativeProjectionPageEntry[]>([]);
  const [last, setLast] = useState<NativeProjectionPage | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [folder, setFolder] = useState("");
  const [selected, setSelected] = useState("");
  const [query, setQuery] = useState("");
  const [lostSelection, setLostSelection] = useState("");
  const seenCursors = useRef(new Set<string>());
  const seenPaths = useRef(new Set<string>());

  function forget() {
    setEntries([]);
    setLast(null);
    seenCursors.current = new Set();
    seenPaths.current = new Set();
  }

  async function read(current: NativeProjectionContext, stepId: string, previous: NativeProjectionPage | null, alive: () => boolean) {
    const request: NativeProjectionPageRequest = {
      stepId,
      expectedPolicySetId: current.currentPolicySetId,
      expectedAuthorizationEpoch: current.authorizationEpoch,
      cursor: previous?.nextCursor ?? null,
      limit: PAGE_SIZE,
    };
    const page = validateNativeProjectionPage(await loadPage(request), current, request, previous);
    if (!alive()) return null;
    if (page.nextCursor && seenCursors.current.has(page.nextCursor)) throw new Error("Curseur répété : actualisez le projet.");
    if (seenPaths.current.size + page.entries.length > MAX_ENTRIES || page.entries.some((entry) => seenPaths.current.has(entry.path)))
      throw new Error("Liste incomplète ou répétée : actualisez le projet.");
    if (page.nextCursor) seenCursors.current.add(page.nextCursor);
    for (const entry of page.entries) seenPaths.current.add(entry.path);
    return page;
  }

  function refresh(preferredStep = step) {
    const previousSelection = selected;
    return tasks.run(
      "read",
      async (alive) => {
        setError(null);
        forget();
        setContext(null);
        const next = validateNativeProjectionContext(await loadContext());
        if (!alive()) return;
        const heads = next.layers.flatMap((layer) => layer.headStepIds);
        const chosen = heads.includes(preferredStep) ? preferredStep : (heads[0] ?? "");
        setContext(next);
        setStep(chosen);
        if (!chosen) return;
        const page = await read(next, chosen, null, alive);
        if (!page) return;
        setEntries(page.entries);
        setLast(page);
        if (previousSelection && !findFile(page.entries, previousSelection) && !page.nextCursor) {
          setLostSelection(previousSelection);
          setSelected("");
        }
      },
      (cause) => {
        forget();
        setContext(null);
        setError(cause);
      },
    );
  }

  function choose(stepId: string) {
    if (!context) return;
    setStep(stepId);
    setSelected("");
    setFolder("");
    setLostSelection("");
    void tasks.run(
      "read",
      async (alive) => {
        setError(null);
        forget();
        const page = await read(context, stepId, null, alive);
        if (!page) return;
        setEntries(page.entries);
        setLast(page);
      },
      (cause) => {
        forget();
        setError(cause);
      },
    );
  }

  function more() {
    if (!context || !last?.nextCursor) return;
    void tasks.run(
      "read",
      async (alive) => {
        setError(null);
        const page = await read(context, step, last, alive);
        if (!page) return;
        setEntries((current) => [...current, ...page.entries]);
        setLast(page);
      },
      (cause) => {
        forget();
        setError(cause);
      },
    );
  }

  useEffect(() => {
    setContext(null);
    setStep("");
    forget();
    setError(null);
    setFolder("");
    setSelected("");
    setQuery("");
    setLostSelection("");
    if (!denied(capabilities, "browseFiles")) void refresh("");
    // A new scope is a new account/project: start from scratch.
  }, [scope]);

  const loading = tasks.isBusy("read");
  const rows = useMemo(() => (query.trim() ? searchFiles(entries, query) : folderChildren(entries, folder)), [entries, folder, query]);
  const file = selected ? findFile(entries, selected) : null;
  const incomplete = Boolean(last?.nextCursor);
  const heads = context?.layers.flatMap((layer) => layer.headStepIds.map((head, index) => ({ layer, head, index }))) ?? [];
  const crumbs = folder ? folder.split("/") : [];

  if (denied(capabilities, "browseFiles"))
    return (
      <section className="collab-files" aria-label="Fichiers du projet">
        <Banner tone="warning" title="Fichiers non accessibles">
          Votre compte ne peut pas parcourir les fichiers publiés de ce projet. Demandez l’accès à un responsable du projet.
        </Banner>
      </section>
    );

  function open(row: FileRow) {
    setLostSelection("");
    if (row.kind === "tree") {
      setFolder(row.path);
      setQuery("");
      setSelected(row.path);
    } else setSelected(row.path);
  }

  return (
    <section className="collab-files" aria-label="Fichiers du projet">
      <div className="collab-toolbar">
        {context && heads.length > 0 && (
          <SelectField label="Référence affichée" value={step} disabled={loading} onChange={(event) => choose(event.target.value)}>
            {heads.map(({ layer, head, index }) => (
              <option key={`${layer.record.layerId}:${head}`} value={head}>
                {layer.headStepIds.length > 1 ? `${layer.record.name} · tête ${index + 1} sur ${layer.headStepIds.length}` : `${layer.record.name} · dernière capture publiée`}
              </option>
            ))}
          </SelectField>
        )}
        <TextField
          label="Rechercher un nom"
          value={query}
          disabled={!entries.length}
          trailing={<Search aria-hidden size={16} className="collab-field-icon" />}
          placeholder={incomplete ? "Parmi les chemins chargés (liste incomplète)" : "Parmi les chemins chargés"}
          onChange={(event) => setQuery(event.target.value)}
        />
        <Button icon={<RefreshCw aria-hidden size={16} />} busy={loading} busyLabel="Actualisation…" onClick={() => void refresh()}>
          Actualiser
        </Button>
      </div>

      {error ? <CollabError error={error} title="Fichiers non chargés" kept="Aucune ancienne liste n’est affichée." onRetry={() => void refresh()} /> : null}
      {lostSelection && (
        <Banner tone="info" title="Sélection disparue">
          « {lostSelection} » n’est plus présent dans cette référence ou n’est plus visible avec vos droits.
        </Banner>
      )}
      {context && !heads.length && !loading && (
        <EmptyState icon={<FolderOpen size={22} />} title="Aucune capture publiée">
          Ce projet n’a pas encore de Layer publié. Les fichiers apparaîtront après la première publication depuis Desktop.
        </EmptyState>
      )}

      {(entries.length > 0 || (loading && !error)) && (
        <div className="collab-split">
          <div className="collab-split__list">
            <nav aria-label="Dossier affiché" className="collab-path">
              <button type="button" className="collab-path__part" onClick={() => { setFolder(""); setQuery(""); }} aria-current={!folder && !query ? "location" : undefined}>
                Racine
              </button>
              {crumbs.map((part, index) => {
                const path = crumbs.slice(0, index + 1).join("/");
                return (
                  <span key={path} className="collab-path__item">
                    <ChevronRight aria-hidden size={14} />
                    <button type="button" className="collab-path__part" onClick={() => { setFolder(path); setQuery(""); }} aria-current={path === folder && !query ? "location" : undefined}>
                      {part}
                    </button>
                  </span>
                );
              })}
            </nav>
            <DelayedRows active={loading && !entries.length} label="Chargement des fichiers…" />
            {query.trim() && <p className="collab-muted">{rows.length ? `${rows.length} résultat${rows.length > 1 ? "s" : ""} parmi les chemins chargés.` : "Aucun nom correspondant parmi les chemins chargés."}</p>}
            {!query.trim() && entries.length > 0 && !rows.length && <EmptyState compact title="Dossier vide">Aucun élément chargé dans ce dossier.</EmptyState>}
            <ul className="collab-rows" aria-label={query.trim() ? "Résultats de recherche" : "Contenu du dossier"}>
              {rows.map((row) => {
                const access = accessLabel(row);
                return (
                  <li key={row.path}>
                    <button type="button" className="collab-row" aria-current={row.path === selected ? "true" : undefined} onClick={() => open(row)}>
                      <span className="collab-row__icon" aria-hidden>
                        {!row.readable ? <Lock size={18} /> : row.kind === "tree" ? <Folder size={18} /> : row.kind === "symlink" ? <Link2 size={18} /> : row.kind === "file" ? <File size={18} /> : <FileWarning size={18} />}
                      </span>
                      <span className="collab-row__text">
                        <span className="collab-row__title">{row.name}</span>
                        <span className="collab-row__meta">{query.trim() ? row.path : kindLabel(row.kind)}</span>
                      </span>
                      {!row.implicit && (row.kind !== "tree" || !row.readable) && <Badge tone={access.tone}>{access.label}</Badge>}
                    </button>
                  </li>
                );
              })}
            </ul>
            {last && (
              <div className="collab-pager">
                <p className="collab-muted" role="status">
                  {entries.length ? `${entries.length} chemin${entries.length > 1 ? "s" : ""} chargé${entries.length > 1 ? "s" : ""}` : "Aucun chemin"}
                  {incomplete ? " · liste incomplète" : " · fin de la liste"}
                </p>
                {incomplete && (
                  <Button size="sm" busy={loading} busyLabel="Chargement…" onClick={more}>
                    Charger les chemins suivants
                  </Button>
                )}
              </div>
            )}
          </div>
          <aside className="collab-split__detail" aria-label="Détail de la sélection">
            {file ? (
              <FileDetail file={file} step={step} epoch={last?.authorizationEpoch} onOpenHistory={onOpenHistory} />
            ) : (
              <EmptyState compact icon={<File size={20} />} title="Aucune sélection">
                Choisissez un fichier pour voir ses droits, copier son chemin ou ouvrir son historique.
              </EmptyState>
            )}
          </aside>
        </div>
      )}
      {last && entries.length === 0 && !loading && (
        <EmptyState compact icon={<FolderOpen size={20} />} title="Aucun chemin visible">
          Cette capture ne contient aucun chemin visible avec vos droits.
        </EmptyState>
      )}
    </section>
  );
}

function FileDetail({ file, step, epoch, onOpenHistory }: { file: FileRow; step: string; epoch?: number; onOpenHistory?: (path: string) => void }) {
  const access = accessLabel(file);
  return (
    <div className="collab-file">
      <div className="collab-file__head">
        <h2 className="collab-file__name">{file.name}</h2>
        <p className="collab-muted collab-break">{file.path}</p>
      </div>
      <dl className="collab-facts">
        <div>
          <dt>Type</dt>
          <dd>{kindLabel(file.kind)}{file.executable && file.readable ? " · exécutable" : ""}</dd>
        </div>
        {!file.implicit && (
          <div>
            <dt>Vos droits</dt>
            <dd>{file.readable ? <Badge tone={access.tone}>{access.label}</Badge> : <LockedContent />}</dd>
          </div>
        )}
      </dl>
      {!file.readable ? (
        <Banner tone="warning" title="Contenu non accessible">
          Le chemin est réservé dans ce projet mais son contenu ne vous est pas ouvert. Aucune taille, empreinte ni aperçu n’est affiché.
        </Banner>
      ) : file.kind === "file" ? (
        <p className="collab-muted">Aperçu non disponible sur le Web pour l’instant : les contenus restent consultables depuis Desktop, selon vos droits.</p>
      ) : null}
      <div className="collab-actions">
        <CopyButton value={file.path} label="Copier le chemin" done="Chemin copié" />
        {onOpenHistory && file.kind !== "tree" && (
          <Button size="sm" icon={<History aria-hidden size={14} />} onClick={() => onOpenHistory(file.path)}>
            Historique du fichier
          </Button>
        )}
      </div>
      <TechnicalDetails
        items={[
          { label: "Capture affichée", value: step, copy: "Copier la capture" },
          { label: "Révision des droits", value: epoch === undefined ? null : String(epoch), copy: "Copier" },
        ]}
      />
    </div>
  );
}
