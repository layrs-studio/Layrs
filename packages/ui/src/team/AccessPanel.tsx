import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FileLock, ShieldCheck } from "lucide-react";
import { Button } from "../components/Button";
import { Avatar, Badge, Banner, Card, EmptyState, Loading } from "../components/Feedback";
import { SelectField } from "../components/Field";
import {
  validateNativeProjectionContext,
  validateNativeProjectionPage,
  type NativeProjectionContext,
  type NativeProjectionPage,
  type NativeProjectionPageEntry,
  type NativeProjectionPageRequest,
} from "../native-remote-project-files-model";
import {
  accessLabels,
  accessLevel,
  applyPathChange,
  describeReason,
  editablePathPolicy,
  pathAccess,
  withReplacedRules,
  type AccessLevel,
  type Effect,
  type PathAccess,
  type ReadPolicy,
} from "./access-model";
import { AccessTree, levelIcons, type EntryAccess, type TreeEntry } from "./AccessTree";
import { PolicyChangeDialog, type PreparedPolicyChange } from "./PolicyChangeDialog";
import { personName, type Person, type PolicyPreview, type PolicyTransport } from "./types";
import type { Directory } from "./useDirectory";
import { useTask } from "./useTask";

const PAGE = 200;

type Change = { kind: "exact" | "subtree"; access: AccessLevel; others: Effect };

/**
 * P16 · Accès aux fichiers: one person, the project's paths and their
 * effective rights. Your own rights come from the server's projection; other
 * people's are explained from the rules the server returned. Changes go
 * through a server-bound preview, then an explicit confirmation.
 */
export function AccessPanel({
  scope,
  directory,
  policy,
  loadContext,
  loadPage,
  self,
  projectName,
  canChange,
  onChanged,
}: {
  scope: string;
  directory: Directory;
  policy?: PolicyTransport;
  loadContext: () => Promise<unknown>;
  loadPage: (request: NativeProjectionPageRequest) => Promise<unknown>;
  self: Person | null;
  projectName: string;
  /** Capability hint `managePolicy`; null when unknown. */
  canChange: boolean | null;
  onChanged?: () => void;
}) {
  const task = useTask(scope);
  const [context, setContext] = useState<NativeProjectionContext | null>(null);
  const [layerId, setLayerId] = useState("");
  const [stepId, setStepId] = useState("");
  const [page, setPage] = useState<NativeProjectionPage | null>(null);
  const [entries, setEntries] = useState<NativeProjectionPageEntry[]>([]);
  const [subject, setSubject] = useState(self?.principalId ?? "");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState("");
  const [change, setChange] = useState<Change>({ kind: "exact", access: "read", others: "allow" });
  const [prepared, setPrepared] = useState<PreparedPolicyChange | null>(null);
  const self_ = self?.principalId ?? "";
  const { policies, canAdminister, management } = directory;

  const readPaths = useCallback(
    async (live: () => boolean, current: NativeProjectionContext, step: string, previous: NativeProjectionPage | null) => {
      const request: NativeProjectionPageRequest = {
        stepId: step,
        expectedPolicySetId: current.currentPolicySetId,
        expectedAuthorizationEpoch: current.authorizationEpoch,
        cursor: previous?.nextCursor ?? null,
        limit: PAGE,
      };
      const next = validateNativeProjectionPage(await loadPage(request), current, request, previous);
      if (!live()) return;
      setPage(next);
      setEntries((known) => (previous ? [...known, ...next.entries] : next.entries));
    },
    [loadPage],
  );

  const open = useCallback(
    (keepSelection = false) =>
      task.run("context", async (live) => {
        const current = validateNativeProjectionContext(await loadContext());
        if (!live()) return;
        setContext(current);
        const layer = current.layers.find((item) => item.record.layerId === layerId) ?? current.layers.find((item) => item.headStepIds.length) ?? null;
        const step = layer && (layer.headStepIds.includes(stepId) ? stepId : layer.headStepIds[0]);
        setLayerId(layer?.record.layerId ?? "");
        setStepId(step ?? "");
        setPage(null);
        setEntries([]);
        if (!keepSelection) setSelected("");
        if (step) await readPaths(live, current, step, null);
      }, "Les chemins du projet n’ont pas pu être lus."),
    [task, loadContext, readPaths, layerId, stepId],
  );

  const started = useRef("");
  useEffect(() => {
    if (started.current === scope) return;
    started.current = scope;
    setSubject(self_);
    void open();
  }, [scope, open, self_]);

  const byPath = useMemo(() => new Map(entries.map((entry) => [entry.path, entry])), [entries]);
  const tree: TreeEntry[] = useMemo(
    () => entries.filter((entry) => entry.target.kind !== "tombstone").map((entry) => ({ path: entry.path, directory: entry.target.kind === "tree" })),
    [entries],
  );
  const subjects: Person[] = useMemo(() => {
    const list = canAdminister ? [...directory.members] : [];
    if (self && !list.some((person) => person.principalId === self.principalId)) list.push(self);
    return list.sort((a, b) => (a.principalId === self_ ? -1 : b.principalId === self_ ? 1 : a.displayName.localeCompare(b.displayName, "fr")));
  }, [canAdminister, directory.members, self, self_]);
  const subjectName = subject === self_ ? "vous" : personName(directory.people, subject);
  const noPathRules = policies !== null && !policies.some((item) => item.parsed && item.parsed.version !== 1);

  const evaluate = useCallback(
    (list: ReadPolicy[] | null, path: string): PathAccess | null => (list ? pathAccess(list, subject, path) : null),
    [subject],
  );
  const access = useCallback(
    (path: string): EntryAccess => {
      const explained = evaluate(policies, path);
      const entry = byPath.get(path);
      if (subject === self_ && entry)
        return { level: entry.writable ? "write" : entry.contentReadable ? "read" : "none", reason: explained?.readReason ?? null, source: "server" };
      if (!explained) return null;
      return { level: accessLevel(explained), reason: explained.read ? explained.writeReason : explained.readReason, source: "rules" };
    },
    [evaluate, policies, byPath, subject, self_],
  );

  const selectedEntry = byPath.get(selected);
  const selectedIsDirectory = selectedEntry ? selectedEntry.target.kind === "tree" : tree.some((item) => item.path.startsWith(`${selected}/`));
  const current = selected ? evaluate(policies, selected) : null;
  const target = policies ? editablePathPolicy(policies, subject) : null;
  const custom = management?.mode === "custom";
  const changeBlocked = !policy
    ? "Modification des règles indisponible dans cette application."
    : !canAdminister || canChange === false
      ? "Réservé aux administrateurs du projet."
      : !policies
        ? "Les règles actuelles n’ont pas pu être lues."
        : undefined;

  useEffect(() => {
    if (!selected) return;
    setChange((value) => ({ ...value, kind: selectedIsDirectory ? "subtree" : "exact", access: current ? accessLevel(current) : value.access }));
    // Only when the selection or the person changes.
  }, [selected, subject]);

  async function prepare() {
    if (!policy || !policies || !directory.rules || !selected) return;
    await task.run("prepare", async (live) => {
      const original = target ? directory.rules!.policies.find((item) => item.policyId === target.policyId)?.rules ?? null : null;
      const rules = applyPathChange(original, { principalId: subject, path: selected, kind: change.kind, access: change.access, others: change.others });
      const after = withReplacedRules(policies, target?.policyId ?? null, rules);
      const before = pathAccess(policies, subject, selected);
      const next = pathAccess(after, subject, selected);
      const moved = tree.filter((item) => {
        const a = accessLevel(pathAccess(policies, subject, item.path));
        const b = accessLevel(pathAccess(after, subject, item.path));
        return item.path !== selected && a !== b;
      }).length;
      const { preview } = await policy<{ preview: PolicyPreview }>(
        target ? { action: "previewReplacePolicyRules", policyId: target.policyId, rules } : { action: "previewPolicyRules", rules },
      );
      if (!live()) return;
      if (preview.expectedPolicySetId !== directory.rules!.policySetId || preview.expectedAuthorizationEpoch !== directory.rules!.authorizationEpoch)
        throw new Error("Les règles ont changé depuis leur lecture. Rechargez la page avant de préparer ce changement.");
      const reducing = accessLevel(next) !== "write" && (accessLevel(before) === "write" || (accessLevel(before) === "read" && accessLevel(next) === "none"));
      const name = subject === self_ ? "Vous" : personName(directory.people, subject);
      setPrepared({
        title: "Confirmer le changement d’accès",
        subject: `${name} · ${change.kind === "subtree" ? `dossier « ${selected} » et son contenu` : `« ${selected} »`} · projet ${projectName}`,
        effects: [
          `Avant : ${accessLabels[accessLevel(before)]}. Après : ${accessLabels[accessLevel(next)]}.`,
          moved > 0
            ? `${moved} autre${moved > 1 ? "s" : ""} chemin${moved > 1 ? "s" : ""} affiché${moved > 1 ? "s" : ""} change${moved > 1 ? "nt" : ""} aussi pour cette personne.`
            : "Aucun autre chemin affiché ne change pour cette personne.",
          target ? `La règle « ${target.label} » est remplacée ; ses autres personnes et chemins sont conservés.` : `Une nouvelle règle de fichiers est ajoutée ; pour les autres chemins, elle ${change.others === "allow" ? "conserve l’accès" : "interdit tout"}.`,
          ...(management?.mode === "managed"
            ? ["Le projet passera en règles personnalisées : les rôles, équipes et invitations seront suspendus jusqu’au retour à la gestion par équipes (Réglages → Avancé), qui retirerait aussi cette règle."]
            : []),
          ...(reducing ? ["La réduction interdit l’accès futur ; elle n’efface pas une copie déjà obtenue par la personne."] : []),
          ...(subject === self_ ? ["Cette modification concerne votre propre accès."] : []),
          "Le serveur refusera le changement si les règles ont été modifiées entre-temps.",
        ],
        preview,
        confirmLabel: "Appliquer ce changement",
        successTitle: "Accès appliqué",
        danger: reducing,
      });
    }, "L’aperçu n’a pas pu être préparé.");
  }

  const layer = context?.layers.find((item) => item.record.layerId === layerId);
  return (
    <div className="access-layout">
      <Card className="access-subjects">
        <h2 className="team-section-title">Personne</h2>
        {!canAdminister && <p className="team-muted">Vous voyez vos propres accès. Les administrateurs du projet voient ceux de chaque membre.</p>}
        <ul className="team-picker__list" aria-label="Personne dont afficher les accès">
          {subjects.map((person) => (
            <li key={person.principalId}>
              <button type="button" className="team-picker__option" aria-pressed={subject === person.principalId} onClick={() => setSubject(person.principalId)}>
                <Avatar name={person.displayName} size={28} />
                <span className="team-picker__name">
                  {person.displayName}
                  {person.principalId === self_ && <small> · vous</small>}
                </span>
              </button>
            </li>
          ))}
        </ul>
        <p className="team-muted team-small">Les règles de fichiers s’appliquent aux personnes. Une équipe donne un rôle sur tout le projet (onglet Équipes).</p>
      </Card>

      <div className="access-main">
        {task.error && <Banner tone="danger">{task.error}</Banner>}
        {directory.rulesError && <Banner tone="warning">{directory.rulesError}</Banner>}
        {noPathRules && (
          <Banner tone="info" title="Aucune règle de fichier">
            Chaque personne accède aux fichiers selon son rôle dans le projet.
          </Banner>
        )}
        {context && context.layers.length > 0 && (
          <div className="team-grid-2">
            <SelectField
              label="Layer"
              value={layerId}
              onChange={(event) => {
                const next = context.layers.find((item) => item.record.layerId === event.target.value);
                setLayerId(event.target.value);
                setStepId(next?.headStepIds[0] ?? "");
                if (next?.headStepIds[0]) void task.run("paths", (live) => readPaths(live, context, next.headStepIds[0], null));
              }}
            >
              {context.layers.map((item) => (
                <option key={item.record.layerId} value={item.record.layerId} disabled={!item.headStepIds.length}>
                  {item.record.name}
                </option>
              ))}
            </SelectField>
            {layer && layer.headStepIds.length > 1 && (
              <SelectField
                label="Version publiée"
                hint="Ce Layer a plusieurs têtes : une intégration est nécessaire."
                value={stepId}
                onChange={(event) => {
                  setStepId(event.target.value);
                  void task.run("paths", (live) => readPaths(live, context, event.target.value, null));
                }}
              >
                {layer.headStepIds.map((id, index) => (
                  <option key={id} value={id}>
                    Tête {index + 1} · {id.slice(7, 15)}
                  </option>
                ))}
              </SelectField>
            )}
          </div>
        )}
        <Card className="access-tree-card">
          <div className="access-tree__head">
            <h2 className="team-section-title">Chemins et droits {subject === self_ ? "(vous)" : `de ${subjectName}`}</h2>
            <span className="team-legend">
              <span className="access-level access-level--write">{levelIcons.write} Écriture</span>
              <span className="access-level access-level--read">{levelIcons.read} Lecture</span>
              <span className="access-level access-level--none">{levelIcons.none} Interdit</span>
            </span>
          </div>
          {!context && task.pending === "context" ? (
            <Loading label="Lecture des chemins…" />
          ) : !tree.length ? (
            <EmptyState compact icon={<FileLock size={22} />} title="Aucun chemin publié à afficher">
              Publiez une première capture sur ce Layer pour régler les accès fichier par fichier.
            </EmptyState>
          ) : (
            <AccessTree
              entries={tree}
              access={access}
              expanded={expanded}
              onToggle={(path) =>
                setExpanded((value) => {
                  const next = new Set(value);
                  if (next.has(path)) next.delete(path);
                  else next.add(path);
                  return next;
                })
              }
              selected={selected}
              onSelect={setSelected}
              label={`Chemins du projet et droits ${subject === self_ ? "de votre compte" : `de ${subjectName}`}`}
            />
          )}
          {page?.nextCursor && (
            <Button busy={task.pending === "paths"} busyLabel="Lecture…" onClick={() => context && void task.run("paths", (live) => readPaths(live, context, stepId, page))}>
              Charger plus de chemins
            </Button>
          )}
          <p className="team-muted team-small">
            {subject === self_
              ? "Vos droits sont ceux confirmés par le serveur pour cette version. Les chemins interdits gardent leur nom visible ; leur contenu reste protégé."
              : "Droits calculés à partir des règles lues sur le serveur ; le serveur reste seul juge à chaque action."}
          </p>
        </Card>
      </div>

      <Card className="access-detail" aria-live="polite">
        {!selected ? (
          <EmptyState compact icon={<ShieldCheck size={22} />} title="Choisissez un fichier ou un dossier">
            Le détail explique d’où vient le droit : règle explicite, héritage d’un dossier ou refus prioritaire.
          </EmptyState>
        ) : (
          <>
            <h2 className="team-section-title access-detail__path">{selected}</h2>
            <p className="team-muted">{subject === self_ ? "Vos droits" : `Droits de ${subjectName}`}</p>
            <AccessFacts access={access(selected)} explained={current} />
            <hr className="team-separator" />
            <h3 className="team-section-title">Modifier l’accès</h3>
            {changeBlocked ? (
              <p className="team-muted">{changeBlocked === "Réservé aux administrateurs du projet." ? "Demandez à un administrateur du projet de modifier cet accès." : changeBlocked}</p>
            ) : (
              <div className="team-form">
                {custom && <p className="team-muted team-small">Règles personnalisées actives : ce changement les complète.</p>}
                <SelectField label="Portée" value={change.kind} onChange={(event) => setChange({ ...change, kind: event.target.value as Change["kind"] })}>
                  <option value="exact">Ce chemin exact</option>
                  <option value="subtree">Ce chemin et tout son contenu</option>
                </SelectField>
                <fieldset className="team-choices team-choices--compact">
                  <legend className="layrs-field__label">Accès au contenu</legend>
                  {(["none", "read", "write"] as AccessLevel[]).map((level) => (
                    <label key={level} className="team-choice">
                      <input type="radio" name="path-access" checked={change.access === level} onChange={() => setChange({ ...change, access: level })} />
                      <span>
                        <strong>
                          {levelIcons[level]} {accessLabels[level]}
                        </strong>
                        <small>{level === "none" ? "Nom visible, contenu protégé." : level === "read" ? "Consulter sans publier de modification." : "Consulter et publier des modifications."}</small>
                      </span>
                    </label>
                  ))}
                </fieldset>
                {!target && (
                  <SelectField
                    label="Autres chemins pour cette personne"
                    value={change.others}
                    onChange={(event) => setChange({ ...change, others: event.target.value as Effect })}
                    hint="Une nouvelle règle de fichiers est créée pour cette personne."
                  >
                    <option value="allow">Conserver l’accès selon son rôle</option>
                    <option value="deny">Tout interdire sauf ce chemin</option>
                  </SelectField>
                )}
                <Button variant="primary" busy={task.pending === "prepare"} busyLabel="Préparation…" onClick={() => void prepare()}>
                  Prévisualiser le changement
                </Button>
              </div>
            )}
          </>
        )}
      </Card>

      {policy && (
        <PolicyChangeDialog
          value={prepared}
          policy={policy}
          onClose={() => setPrepared(null)}
          onApplied={async () => {
            await directory.reload();
            onChanged?.();
            await open(true);
          }}
        />
      )}
    </div>
  );
}

function AccessFacts({ access, explained }: { access: EntryAccess; explained: PathAccess | null }) {
  if (!access) return <p className="team-muted">Droits non évalués : les règles n’ont pas pu être lues.</p>;
  return (
    <dl className="access-facts">
      <dt>Résultat</dt>
      <dd>
        <span className={`access-level access-level--${access.level}`}>
          {levelIcons[access.level]} {accessLabels[access.level]}
        </span>{" "}
        {access.source === "server" ? <Badge tone="success">Confirmé par le serveur</Badge> : <Badge>Calculé depuis les règles</Badge>}
      </dd>
      {explained && (
        <>
          <dt>Lecture</dt>
          <dd>
            {explained.read ? "Autorisée" : "Interdite"}. {describeReason(explained.readReason, "read")}
          </dd>
          <dt>Écriture</dt>
          <dd>
            {explained.write ? "Autorisée" : "Interdite"}. {describeReason(explained.writeReason, "write")}
          </dd>
        </>
      )}
    </dl>
  );
}
