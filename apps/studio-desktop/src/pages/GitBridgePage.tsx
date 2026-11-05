import { useEffect, useRef, useState } from "react";
import { Banner, Button, Card, PageHeader, SelectField, TechnicalId, TextAreaField, TextField } from "@layrs/ui";
import { useWorkspace } from "../app/workspace";
import { currentWorktree } from "../app/worktrees";
import { gitExportMode, gitExportRequest, gitImportRequest, gitReferences, type GitExportMode, type GitExportResult, type GitExportReview, type GitImportReview, type GitTarget } from "../app/gitBridge";
import { nativeGitBridge } from "../tauri";

export function GitBridgePage() {
  return <div className="layrs-page layrs-page--narrow">
    <PageHeader title="Pont Git" description="Importer un checkpoint Git vérifié ou exporter explicitement une version native. Aucun transfert n’est lancé à l’ouverture de cette page." />
    <Banner tone="info" title="Interopérabilité locale et explicite">
      Le moteur conserve ses refus pour Git LFS, les submodules et les formats non pris en charge. Les fichiers de travail ne sont pas remplacés par un import. Les exports sont des dépôts Git nus, sans export continu.
    </Banner>
    <GitImport />
    <GitExport />
  </div>;
}

function useGitOperation() {
  const workspace = useWorkspace();
  const [error, setError] = useState("");
  const revision = useRef(0);
  useEffect(() => () => { revision.current++; }, []);
  async function perform<T>(label: string, work: () => Promise<T>, accept: (result: T) => void) {
    const current = revision.current;
    const result = await workspace.task(label, async () => {
      setError("");
      try { return await work(); }
      catch (cause) { if (current === revision.current) setError(String(cause)); throw cause; }
    });
    if (result !== null && current === revision.current) accept(result);
  }
  const blocked = workspace.busy ? `Opération en cours : ${workspace.busy}` : !workspace.data ? "Actualisez l’état du Worktree."
    : workspace.data.needsRepair || ["stale", "needs_repair"].includes(currentWorktree(workspace.data.state)?.status ?? "") ? "Terminez la reprise du Worktree." : undefined;
  return { workspace, error, perform, blocked };
}

function Target({ target }: { target: GitTarget }) {
  return <dl className="settings-facts">
    <dt>Dossier natif</dt><dd><code>{target.path}</code></dd>
    <dt>Worktree</dt><dd><TechnicalId value={target.worktreeId} /></dd>
    <dt>Store</dt><dd><TechnicalId value={target.storeId} /></dd>
    <dt>Opération vérifiée</dt><dd><TechnicalId value={target.operationId} /></dd>
    <dt>View vérifiée</dt><dd><TechnicalId value={target.viewId} /></dd>
  </dl>;
}
function NativeError({ error }: { error: string }) {
  return error ? <Banner tone="danger" title="Le moteur Git n’a pas confirmé l’opération"><code className="sync-detail">{error}</code></Banner> : null;
}

function GitImport() {
  const { workspace, error, perform, blocked } = useGitOperation();
  const [source, setSource] = useState("");
  const [checkpoint, setCheckpoint] = useState("");
  const [digest, setDigest] = useState("");
  const [references, setReferences] = useState("");
  const [review, setReview] = useState<GitImportReview | null>(null);
  const [imported, setImported] = useState<string | null>(null);
  const state = workspace.data?.state;
  const stale = review && (review.review.target.worktreeId !== state?.worktree.worktreeId || review.review.target.operationId !== state?.operation.operationId);
  function changed() { setReview(null); setImported(null); }

  async function prepare() {
    if (blocked || !source.trim() || !checkpoint.trim()) return;
    changed();
    await perform("Préparation du checkpoint Git…", () => nativeGitBridge<{ checkpoint: { path: string; digest: string } }>(workspace.root, {
      action: "prepareImport", source: source.trim(), checkpoint: checkpoint.trim(), references: gitReferences(references),
    }), result => { setCheckpoint(result.checkpoint.path); setDigest(result.checkpoint.digest); });
  }
  async function inspect() {
    if (blocked) return;
    changed();
    await perform("Vérification du checkpoint Git…", () => nativeGitBridge<GitImportReview>(workspace.root, { action: "reviewImport", checkpoint: checkpoint.trim(), digest: digest.trim() }), result => {
      setCheckpoint(result.review.checkpoint); setDigest(result.review.digest); setReview(result);
    });
  }
  async function apply() {
    if (blocked || !review || !state || stale) return;
    setReview(null);
    await perform("Import du checkpoint Git…", () => {
      const request = gitImportRequest(review, checkpoint, digest, state.worktree.worktreeId, state.operation.operationId);
      return nativeGitBridge<{ import: { importId: string; alreadyImported: boolean } }>(workspace.root, request);
    }, result => {
      setImported(result.import.alreadyImported ? "Cet import était déjà installé ; aucune référence n’a été dupliquée." : "Import confirmé. Les références Git sont disponibles dans les Layers et points natifs.");
      void workspace.refresh();
    });
  }

  return <section className="settings-section" aria-labelledby="git-import">
    <h2 id="git-import">Importer un checkpoint Git</h2>
    <Card className="settings-card">
      <p className="sync-muted">La préparation écrit un nouveau fichier checkpoint contenant l’historique choisi. Conservez-le comme votre dépôt source. La vérification suivante est en lecture seule.</p>
      <TextField label="Dépôt Git source" value={source} disabled={Boolean(workspace.busy)} hint="Chemin local du dépôt Git ; aucune URL réseau." onChange={event => { setSource(event.target.value); changed(); }} />
      <TextAreaField label="Références Git à préparer (optionnel)" value={references} rows={3} disabled={Boolean(workspace.busy)} hint="Une référence complète par ligne, par exemple refs/heads/main. Vide : toutes les références locales prises en charge." onChange={event => { setReferences(event.target.value); changed(); }} />
      <TextField label="Fichier checkpoint Git" value={checkpoint} disabled={Boolean(workspace.busy)} hint="Pour préparer : nouveau fichier hors du dépôt source, dans un dossier existant. Pour reprendre : checkpoint existant." onChange={event => { setCheckpoint(event.target.value); setDigest(""); changed(); }} />
      <div><Button disabledReason={blocked ?? (!source.trim() || !checkpoint.trim() ? "Indiquez le dépôt source et le nouveau checkpoint." : undefined)} onClick={() => void prepare()}>Créer le checkpoint Git</Button></div>
      <TextField label="Empreinte SHA-256 du checkpoint" value={digest} disabled={Boolean(workspace.busy)} placeholder="sha256:…" hint="Utilisez l’empreinte rendue lors de sa préparation. Une autre empreinte ou un fichier modifié sera refusé." onChange={event => { setDigest(event.target.value); changed(); }} />
      <div><Button disabledReason={blocked ?? (!checkpoint.trim() || !/^sha256:[a-f0-9]{64}$/.test(digest.trim()) ? "Indiquez le checkpoint et son empreinte exacte." : undefined)} onClick={() => void inspect()}>Vérifier le checkpoint et la cible</Button></div>
      <NativeError error={error} />
      {review && <section aria-label="Import Git vérifié">
        <h3>Checkpoint et cible à confirmer</h3>
        <p><code>{review.review.checkpoint}</code></p>
        <TechnicalId value={review.review.digest} label="Copier l’empreinte" />
        <Target target={review.review.target} />
        <p>{review.review.report.commits} commit(s), {review.review.report.references.length} référence(s) · format {review.review.report.objectFormat}</p>
        <details><summary>Références et captures exactes</summary><ul>{review.review.report.references.slice(0, 100).map(item => <li key={item.gitRef}><code>{item.gitRef}</code> · <code>{item.gitId}</code> → <code>{item.stepId}</code></li>)}</ul>
          {review.review.report.references.length > 100 && <p>Affichage limité aux 100 premières références ; le checkpoint vérifié contient l’ensemble.</p>}
        </details>
        {review.review.report.warnings.map((warning, index) => <Banner key={index} tone="warning" title={warning} />)}
        <div><Button variant="primary" disabledReason={blocked ?? (stale ? "Le Worktree a changé. Vérifiez de nouveau le checkpoint et la cible." : undefined)} onClick={() => void apply()}>Importer ce checkpoint dans ce Worktree</Button></div>
      </section>}
      {imported && <Banner tone="success" title={imported} />}
    </Card>
  </section>;
}

function GitExport() {
  const { workspace, error, perform, blocked } = useGitOperation();
  const [destination, setDestination] = useState("");
  const [kind, setKind] = useState<GitExportMode["kind"]>("history");
  const [references, setReferences] = useState("");
  const [step, setStep] = useState("");
  const [reference, setReference] = useState("refs/heads/main");
  const [review, setReview] = useState<GitExportReview | null>(null);
  const [result, setResult] = useState<GitExportResult | null>(null);
  let mode: GitExportMode | undefined, invalid: string | undefined;
  try { mode = gitExportMode(kind, references, step, reference); }
  catch (cause) { invalid = cause instanceof Error ? cause.message : String(cause); }
  const state = workspace.data?.state;
  const stale = review && (review.review.target.worktreeId !== state?.worktree.worktreeId || review.review.target.operationId !== state?.operation.operationId);
  function changed() { setReview(null); setResult(null); }
  async function inspect() {
    if (blocked || !mode || !destination.trim()) return;
    changed();
    await perform("Vérification de la cible Git…", () => nativeGitBridge<GitExportReview>(workspace.root, { action: "reviewExport", destination: destination.trim(), mode }), result => {
      setDestination(result.review.destination);
      if (result.review.mode.kind === "selected_history") setReferences(result.review.mode.references.join("\n"));
      setReview(result);
    });
  }
  async function apply(resume: boolean) {
    if (blocked || !mode || !review || !state || stale) return;
    setReview(null);
    await perform(resume ? "Reprise de l’export Git…" : "Export Git…", () => {
      const request = gitExportRequest(review, destination, mode, state.worktree.worktreeId, state.operation.operationId, resume);
      return nativeGitBridge<GitExportResult>(workspace.root, request);
    }, setResult);
  }
  return <section className="settings-section" aria-labelledby="git-export">
    <h2 id="git-export">Exporter vers un dépôt Git</h2>
    <Card className="settings-card">
      <TextField label="Dossier Git de destination" value={destination} disabled={Boolean(workspace.busy)} hint="Nouveau dossier inexistant pour un export ; dossier exact de l’export interrompu pour une reprise." onChange={event => { setDestination(event.target.value); changed(); }} />
      <SelectField label="Contenu de l’export" value={kind} disabled={Boolean(workspace.busy)} onChange={event => { setKind(event.target.value as GitExportMode["kind"]); changed(); }}>
        <option value="history">Historique des références natives visibles</option>
        <option value="selected_history">Historique de références Git exactes</option>
        <option value="snapshot">Une Step exacte, sans historique</option>
      </SelectField>
      {kind === "selected_history" && <TextAreaField label="Références Git à exporter" value={references} rows={3} disabled={Boolean(workspace.busy)} hint="Une référence Git complète par ligne. Les noms doivent correspondre à ceux du moteur d’export." onChange={event => { setReferences(event.target.value); changed(); }} />}
      {kind === "snapshot" && <>
        <TextField label="Step exacte à exporter" value={step} disabled={Boolean(workspace.busy)} placeholder="blake3:…" onChange={event => { setStep(event.target.value); changed(); }} />
        <TextField label="Référence Git du snapshot" value={reference} disabled={Boolean(workspace.busy)} onChange={event => { setReference(event.target.value); changed(); }} />
      </>}
      <div><Button disabledReason={blocked ?? invalid ?? (!destination.trim() ? "Indiquez le dossier de destination." : undefined)} onClick={() => void inspect()}>Vérifier la cible de l’export</Button></div>
      <NativeError error={error} />
      {review && <section aria-label="Cible Git vérifiée">
        <Target target={review.review.target} />
        <p>Destination : <code>{review.review.destination}</code></p>
        <Banner tone="info" title="Aperçu du contenu indisponible dans le moteur">
          Cette vérification fixe la cible native, sa View et le mode demandé. Les références, les contenus et les refus seront vérifiés par l’export ; aucun fichier de destination n’a été écrit par cette vérification.
        </Banner>
        <p className="sync-muted">Une reprise exige le marqueur d’export incomplet et exactement la même opération, View, destination et sélection. Un dossier ordinaire ne peut pas être repris.</p>
        <div className="sync-card__actions">
          <Button variant="primary" disabledReason={blocked ?? (stale ? "Le Worktree a changé. Vérifiez de nouveau la cible." : undefined)} onClick={() => void apply(false)}>Créer cet export Git</Button>
          <Button disabledReason={blocked ?? (stale ? "Le Worktree a changé. Vérifiez de nouveau la cible." : undefined)} onClick={() => void apply(true)}>Reprendre cet export interrompu</Button>
        </div>
      </section>}
      {result && <Banner tone="success" title="Export Git confirmé par le moteur"><code>{result.destination}</code> · {Object.keys(result.export.commits).length} commit(s), {Object.keys(result.export.references).length} référence(s).</Banner>}
      {result?.export.warnings.map((warning, index) => <Banner key={index} tone="warning" title={warning} />)}
    </Card>
  </section>;
}
