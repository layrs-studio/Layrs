import { useState } from "react";
import { Banner, Button, Card, Link, SelectField, TechnicalId, TextAreaField, TextField, formatBytes, segment } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useWorkspace } from "../../app/workspace";
import { useRemote } from "../../app/remote";
import { useOfflineTask } from "../../app/useOfflineTask";
import * as offline from "../../app/offline";
import { OfflinePreparations } from "./OfflinePreparations";
import { OfflineRestore } from "./OfflineRestore";

export function OfflineFiles() {
  const { data } = useProjection(), remote = useRemote(), { root } = useWorkspace();
  return <section className="settings-section" aria-labelledby="offline-files">
    <h2 id="offline-files">Disponibilité hors ligne</h2>
    {data?.offline?.shared === true
      ? <OfflineQuery key={`${data.identity.worktreeId}:${data.projection.generation}:${data.projection.digest}:${remote.epoch}`} />
      : <Card className="settings-card"><p>La disponibilité par version et les préparations nécessitent un stockage partagé.</p><Link to={`/projects/${segment(root)}/worktrees`}>Créer une copie avec stockage partagé</Link></Card>}
  </section>;
}

function OfflineQuery() {
  const projection = useProjection(), remote = useRemote(), task = useOfflineTask();
  const { root } = useWorkspace(), data = projection.data!;
  const [paths, setPaths] = useState("");
  const [selected, setSelected] = useState("current"), [other, setOther] = useState("");
  const [operation, setOperation] = useState<offline.Operation>("read");
  const [observations, setObservations] = useState<offline.Version[]>([]);
  const [step, setStep] = useState(""), [observationPaths, setObservationPaths] = useState("");
  const [report, setReport] = useState<offline.Availability | null>(null);
  const [read, setRead] = useState<{ bytes: number; text: string | null } | null>(null);
  const [diff, setDiff] = useState<offline.Difference[] | null>(null);
  const [resolved, setResolved] = useState<offline.AvailabilityRequest | null>(null);
  const [preparation, setPreparation] = useState<offline.Preparation | null>(null);
  const [refreshPreparations, setRefreshPreparations] = useState(0);
  let versions: offline.Version[] = [], historyError: string | undefined;
  try { versions = [...offline.historyVersions(data, projection.history), ...observations]; }
  catch (cause) { historyError = String(cause); }
  versions = [...new Map(versions.map(v => [offline.versionKey(v), v])).values()];
  let requestPaths: string[] = [], inputError: string | undefined;
  try { requestPaths = offline.exactPaths(paths, data.projection.snapshot.pathSemantics); }
  catch (cause) { inputError = cause instanceof Error ? cause.message : String(cause); }
  const comparison = operation.startsWith("compare");
  const blocked = task.blocked ?? inputError ?? (comparison && !other ? "Choisissez la seconde version." : undefined) ?? historyError;
  const networkBlocked = task.blocked ?? (!projection.ready ? "Actualisez ou réparez ce dossier." : data.pendingPublication ? "Terminez la publication en attente." : remote.session.state !== "connected" ? "Connectez-vous pour cette action réseau." : remote.session.principalId !== data.identity.principalId ? "Connectez-vous avec le compte de ce dossier." : undefined);
  let observationError: string | undefined;
  try { offline.exactStep(step); offline.exactPaths(observationPaths, data.projection.snapshot.pathSemantics); }
  catch (cause) { observationError = cause instanceof Error ? cause.message : String(cause); }
  function clear() { setReport(null); setRead(null); setDiff(null); setResolved(null); setPreparation(null); }
  async function query(): Promise<offline.AvailabilityRequest> {
    // Current is always supplied by the native API, never reconstructed from UI state.
    const current = selected === "current" || comparison && other === "current"
      ? offline.currentVersion(await task.local({ action: "currentVersion" }), data) : null;
    const pick = (key: string) => key === "current" && current ? current : versions.find(v => offline.versionKey(v) === key);
    const v = pick(selected), b = comparison ? pick(other) : null;
    if (!v || comparison && !b) throw new Error("Rechargez les versions exactes avant de poursuivre.");
    return offline.availabilityRequest(v, b ?? null, requestPaths, operation);
  }
  async function run(kind: "availability" | "compare" | "readFile" | "prepare") {
    if (blocked || kind === "prepare" && networkBlocked) return;
    clear();
    const result = await task.execute("Vérification hors ligne…", async () => {
      const request = await query();
      if (kind === "availability") return { request, report: offline.availability(await task.local({ action: kind, request }), request) };
      if (kind === "compare") return { request, differences: offline.differences(await task.local({ action: kind, request }), request) };
      if (kind === "readFile") return { request, preview: offline.preview(await task.local({ action: kind, version: request.version, path: request.paths[0], maxBytes: offline.previewLimit }), request.version, request.paths[0]) };
      const reply = await remote.transport({ action: "prepareProjectionOffline", ...offline.networkRequest(data, data.projection.generation), request });
      return { request, preparation: offline.preparedReply(reply, request) };
    });
    if (!result) return;
    setResolved(result.request);
    if ("report" in result) setReport(result.report!);
    if ("differences" in result) setDiff(result.differences!);
    if ("preview" in result) setRead(result.preview!);
    if ("preparation" in result) { setPreparation(result.preparation!); setRefreshPreparations(n => n + 1); }
  }
  async function observe() {
    if (networkBlocked || observationError) return;
    const result = await task.execute("Observation de la version distante…", async () => {
      const exact = offline.exactStep(step);
      return offline.observedReply(await remote.transport({ action: "observeProjectionVersion", ...offline.networkRequest(data, data.projection.generation), step: exact, paths: offline.exactPaths(observationPaths, data.projection.snapshot.pathSemantics) }), exact);
    });
    if (result) { setObservations(v => [...v, result]); setSelected(offline.versionKey(result)); clear(); }
  }
  const options = <><option value="current">Version courante du dossier</option>{versions.map(v => <option key={offline.versionKey(v)} value={offline.versionKey(v)}>{offline.versionLabel(v)} · {(v.kind === "remote" ? v.observationDigest : v.digest).slice(-12)}</option>)}</>;
  return <>
    <Card className="settings-card">
      <p className="sync-muted">Vérifiez une version exacte sans connexion. Une préparation conserve ses contenus ; elle ne donne aucun droit supplémentaire et ne garantit pas une opération sur un dossier modifié.</p>
      <SelectField label="Version à vérifier" value={selected} disabled={task.busy} onChange={e => { setSelected(e.target.value); clear(); }}>{options}</SelectField>
      <Button disabledReason={task.blocked} onClick={() => void projection.loadHistory()}>Charger les versions de l’historique</Button>
      <TextAreaField label="Chemins à vérifier" hint="Un chemin exact par ligne, sans motif. Maximum : 1 024 chemins." value={paths} disabled={task.busy} error={paths ? inputError : undefined} onChange={e => { setPaths(e.target.value); clear(); }} />
      <SelectField label="Opération hors ligne" value={operation} disabled={task.busy} onChange={e => { setOperation(e.target.value as offline.Operation); clear(); }}>
        <option value="read">Lire</option><option value="edit">Modifier</option><option value="compareMetadata">Comparer les métadonnées</option><option value="compareContent">Comparer les contenus</option>
      </SelectField>
      {comparison && <SelectField label="Seconde version" value={other} disabled={task.busy} onChange={e => { setOther(e.target.value); clear(); }}><option value="">Choisir une version exacte</option>{options}</SelectField>}
      <p className="sync-muted">Les reçus de préparation pour Capture et Restauration ne sont pas pris en charge. La restauration locale ci-dessous vérifie séparément le dossier ; utilisez Lire pour préparer les octets de sa cible.</p>
      <div className="sync-card__actions">
        <Button disabledReason={blocked} onClick={() => void run("availability")}>Vérifier la disponibilité</Button>
        <Button disabledReason={blocked ?? networkBlocked} onClick={() => void run("prepare")}>Télécharger et préparer</Button>
        {comparison ? <Button disabledReason={blocked} onClick={() => void run("compare")}>Comparer ces versions</Button>
          : <Button disabledReason={blocked ?? (requestPaths.length !== 1 || operation !== "read" ? "Choisissez Lire et un seul fichier." : undefined)} onClick={() => void run("readFile")}>Lire l’aperçu (1 Mio maximum)</Button>}
      </div>
      <OfflineRestore key={JSON.stringify([selected, other, paths, operation])} blocked={blocked ?? (comparison ? "Choisissez une seule version pour restaurer." : undefined)} resolve={async () => { const request = await query(); return { version: request.version, paths: request.paths }; }} />
      {resolved && <div aria-label="Versions vérifiées"><VersionDetails value={resolved.version} />{resolved.other && <VersionDetails value={resolved.other} />}</div>}
      {report && <div role="status">
        <p>{report.available ? "Opération disponible selon cette vérification locale." : "Opération indisponible selon cette vérification locale."} {report.prepared ? "Préparation enregistrée." : "Aucune préparation confirmée pour cette demande."}</p>
        {report.blockers.length > 0 && <p>{report.blockers.map(b => offline.blockers[b]).join(" · ")}</p>}
        <ul>{report.paths.map(row => <li key={`${row.operand}:${row.path}`}>
          <strong>{row.path}</strong> · version {row.operand + 1}
          <dl className="settings-facts">
            <dt>Sélection</dt><dd>{row.selected ? "Inclus" : "Exclu par choix"}</dd>
            <dt>Droits observés</dt><dd>{row.blockers.includes("protectedPath") ? "Accès protégé" : row.observedWritable ? "Écriture autorisée lors de l’observation" : "Écriture non autorisée lors de l’observation"}</dd>
            <dt>Présence dans cette version</dt><dd>{row.blockers.includes("unknownPath") ? "Chemin inconnu" : row.absent ? "Absence enregistrée" : "Chemin enregistré"}</dd>
            <dt>Matérialisation</dt><dd>{row.materialized ? "Matérialisé dans le dossier observé" : "Non matérialisé"}</dd>
            <dt>Contenu local</dt><dd>{row.absent ? "Sans contenu" : row.blockers.includes("missingContent") ? "Octets indisponibles" : row.manifestId ? "Manifeste local disponible" : "Présence locale non établie"}</dd>
            <dt>Conservation</dt><dd>{row.retained ? "Retenu par une protection" : "Aucune protection indiquée"}</dd>
          </dl>
          {row.blockers.length > 0 && <p>{row.blockers.map(b => offline.blockers[b]).join(" · ")}</p>}
        </li>)}</ul>
      </div>}
      {read && <div role="status"><p>{formatBytes(read.bytes)} · version exacte lue localement</p>{read.text === null ? <p>Contenu binaire ou non UTF-8 : aucun aperçu textuel.</p> : <pre tabIndex={0} aria-label="Aperçu UTF-8" style={{ maxHeight: "24rem", overflow: "auto", whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{read.text}</pre>}</div>}
      {diff && <div role="status"><p>Comparaison des identifiants de contenu et des modes exécutables.</p>{diff.length === 0 ? <p>Aucune différence sur ces chemins.</p> : <ul>{diff.map(d => <li key={d.path}><strong>{d.path}</strong><p>Avant : {d.before ? <TechnicalId value={d.before} /> : "Absent"} · exécutable : {d.beforeExecutable ? "oui" : "non"}</p><p>Après : {d.after ? <TechnicalId value={d.after} /> : "Absent"} · exécutable : {d.afterExecutable ? "oui" : "non"}</p></li>)}</ul>}</div>}
      {preparation && <p role="status">Préparation {preparation.status === "ready" ? "prête" : "en cours, à reprendre"} · <TechnicalId value={preparation.id} /></p>}
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      <details><summary>Observer une version distante exacte</summary>
        <p>Cette action réseau observe les métadonnées des chemins demandés. Elle ne change pas la sélection du dossier et ne prépare pas les contenus.</p>
        <TextField label="Step distant exact" value={step} disabled={task.busy} onChange={e => setStep(e.target.value)} />
        <TextAreaField label="Chemins de l’observation distante" hint="Sélection propre à cette observation, un chemin exact par ligne." value={observationPaths} disabled={task.busy} onChange={e => setObservationPaths(e.target.value)} />
        <Button disabledReason={networkBlocked ?? observationError} onClick={() => void observe()}>Observer les métadonnées</Button>
      </details>
      {remote.session.state !== "connected" && <Link to={`/projects/${segment(root)}/connect`}>Connexion au serveur</Link>}
    </Card>
    <OfflinePreparations key={refreshPreparations} networkBlocked={networkBlocked} />
  </>;
}

export function VersionDetails({ value }: { value: offline.Version }) {
  return <details><summary>{offline.versionLabel(value)}</summary><dl className="settings-facts">{Object.entries(value).filter(([key]) => key !== "kind").map(([key, item]) => <div key={key}><dt>{({ worktreeId: "Worktree", generation: "Génération", sequence: "Capture", digest: "Empreinte", stepId: "Step", rootTreeId: "Arbre", policySetId: "Politique", authorizationEpoch: "Époque des droits", observationDigest: "Empreinte de l’observation" } as Record<string, string>)[key]}</dt><dd><code>{item}</code></dd></div>)}</dl></details>;
}
