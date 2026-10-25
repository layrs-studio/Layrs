import { useEffect, useRef, useState } from "react";
import { FolderOpen, GitBranch } from "lucide-react";
import { Banner, Button, Card, PageHeader, SegmentedControl, SelectField, TextAreaField, TextField } from "@layrs/ui";
import { useWorkspace } from "../app/workspace";
import { nativeGit, selectFolder, type GitExportMode } from "../tauri";

type ImportSummary = { source: string; commitCount: number; references: string[]; warnings: string[]; sourceBytes: number };
type ExportSummary = { destination: string; mode: string; layerCount: number; markerCount: number; references: string[]; stepId: string | null };
type Preview<T> = { previewId: string; summary: T };
type ImportResult = { import: { alreadyImported: boolean; layerIds: string[]; markerIds: string[] }; report: { commitCount: number; references: { gitRef: string; gitId: string; stepId: string }[]; warnings: string[]; sourceBytes: number } };
type ExportResult = { export: { commits: Record<string, string>; references: Record<string, string>; warnings: string[] }; destination: string };
type Completion = { message: string; warnings: string[]; references: { name: string; target: string }[] };

/** Explicit two-stage Git import/export for a complete local checkout. */
export function GitPage() {
  const workspace = useWorkspace();
  const [tab, setTab] = useState<"import" | "export">("import");
  const [source, setSource] = useState("");
  const [importRefs, setImportRefs] = useState("");
  const [destination, setDestination] = useState("");
  const [exportRefs, setExportRefs] = useState("");
  const [exportKind, setExportKind] = useState<"history" | "selected_history" | "snapshot">("history");
  const [stepId, setStepId] = useState("");
  const [preview, setPreview] = useState<(Preview<ImportSummary> & { kind: "import" }) | (Preview<ExportSummary> & { kind: "export" }) | null>(null);
  const [result, setResult] = useState<Completion | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const activeToken = useRef<string | null>(null);

  useEffect(() => () => {
    generation.current++;
    if (activeToken.current) void nativeGit(workspace.root, { action: "discardPreview", previewId: activeToken.current }).catch(() => undefined);
  }, [workspace.root]);

  function invalidate() {
    generation.current++;
    if (activeToken.current) void nativeGit(workspace.root, { action: "discardPreview", previewId: activeToken.current }).catch(() => undefined);
    activeToken.current = null;
    setPreview(null); setResult(null); setError("");
  }
  function references(text: string) { return text.split(/\r?\n/).map((value) => value.trim()).filter(Boolean); }

  async function chooseSource() {
    try {
      const folder = await selectFolder(source || undefined);
      if (folder) { setSource(folder); invalidate(); }
    } catch (cause) { setError(String(cause)); }
  }

  async function chooseDestinationParent() {
    try {
      const folder = await selectFolder(destination || undefined);
      if (folder) {
        const separator = folder.includes("\\") ? "\\" : "/";
        setDestination(`${folder.replace(/[\\/]$/, "")}${separator}layrs-export`);
        invalidate();
      }
    } catch (cause) { setError(String(cause)); }
  }

  async function inspect() {
    if (busy || workspace.busy) return;
    invalidate();
    const current = generation.current;
    setBusy(true);
    try {
      if (tab === "import") {
        const next = await workspace.task("Analyse Git…", () => nativeGit<Preview<ImportSummary>>(workspace.root, {
          action: "previewImport", source: source.trim(), references: references(importRefs),
        }));
        if (next && generation.current === current) { activeToken.current = next.previewId; setPreview({ ...next, kind: "import" }); }
        else if (next) void nativeGit(workspace.root, { action: "discardPreview", previewId: next.previewId }).catch(() => undefined);
      } else {
        const mode: GitExportMode = exportKind === "snapshot" ? { kind: "snapshot", stepId: stepId.trim() }
          : exportKind === "selected_history" ? { kind: "selected_history", references: references(exportRefs) }
          : { kind: "history" };
        const next = await workspace.task("Préparation de l’export Git…", () => nativeGit<Preview<ExportSummary>>(workspace.root, {
          action: "previewExport", destination: destination.trim(), mode,
        }));
        if (next && generation.current === current) { activeToken.current = next.previewId; setPreview({ ...next, kind: "export" }); }
        else if (next) void nativeGit(workspace.root, { action: "discardPreview", previewId: next.previewId }).catch(() => undefined);
      }
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }

  async function apply() {
    if (!preview || busy || workspace.busy) return;
    const selected = preview;
    setBusy(true); setError(""); setPreview(null); activeToken.current = null;
    try {
      if (selected.kind === "import") {
        const next = await workspace.task("Import Git…", () => nativeGit<ImportResult>(workspace.root, { action: "applyImport", previewId: selected.previewId }));
        if (next) {
          setResult({
            message: next.import.alreadyImported ? "Cet historique Git était déjà importé." : `${next.report.commitCount} commit(s), ${next.import.layerIds.length} branche(s) et ${next.import.markerIds.length} tag(s) importés. Les fichiers du projet n’ont pas été remplacés.`,
            warnings: next.report.warnings,
            references: next.report.references.map((item) => ({ name: item.gitRef, target: `${item.gitId} → ${item.stepId}` })),
          });
          await workspace.refresh();
        }
      } else {
        const next = await workspace.task("Export Git…", () => nativeGit<ExportResult>(workspace.root, { action: "applyExport", previewId: selected.previewId }));
        if (next) setResult({
          message: `${Object.keys(next.export.references).length} référence(s) Git exportées dans ${next.destination}. Le dépôt créé est bare (sans dossier de travail).`,
          warnings: next.export.warnings,
          references: Object.entries(next.export.references).map(([name, target]) => ({ name, target })),
        });
      }
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }

  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader title="Git" description="Importer un historique Git dans ce projet ou exporter son historique vers un nouveau dépôt local." />
      <SegmentedControl label="Opération Git" value={tab} onChange={(value) => { setTab(value); invalidate(); }} options={[
        { value: "import", label: "Importer", icon: <GitBranch aria-hidden size={16} /> },
        { value: "export", label: "Exporter", icon: <GitBranch aria-hidden size={16} /> },
      ]} />
      <Card className="settings-card">
        {tab === "import" ? <>
          <h2>Importer un dépôt</h2>
          <p>L’historique, les branches et les tags sont ajoutés au projet. Les fichiers présents dans ce dossier restent en place.</p>
          <div className="new-project__folder">
            <TextField label="Dossier Git source" value={source} onChange={(event) => { setSource(event.target.value); invalidate(); }} placeholder="Chemin du dépôt Git" />
            <Button icon={<FolderOpen aria-hidden size={16} />} onClick={() => void chooseSource()}>Parcourir…</Button>
          </div>
          <TextAreaField label="Références précises (facultatif, une par ligne)" value={importRefs} onChange={(event) => { setImportRefs(event.target.value); invalidate(); }} placeholder="refs/heads/main" hint="Laisser vide pour toutes les branches et tous les tags locaux." />
        </> : <>
          <h2>Exporter vers Git</h2>
          <p>Choisissez un dossier qui n’existe pas encore. L’export y crée un dépôt Git, sans modifier ce projet.</p>
          <div className="new-project__folder">
            <TextField label="Nouveau dossier Git" value={destination} onChange={(event) => { setDestination(event.target.value); invalidate(); }} placeholder="Chemin d’un nouveau dossier" />
            <Button icon={<FolderOpen aria-hidden size={16} />} onClick={() => void chooseDestinationParent()}>Choisir le parent…</Button>
          </div>
          <SelectField label="Contenu exporté" value={exportKind} onChange={(event) => { setExportKind(event.target.value as typeof exportKind); invalidate(); }}>
            <option value="history">Tout l’historique visible</option>
            <option value="selected_history">Références précises</option>
            <option value="snapshot">Une capture seule</option>
          </SelectField>
          {exportKind === "selected_history" && <TextAreaField label="Références Git (une par ligne)" value={exportRefs} onChange={(event) => { setExportRefs(event.target.value); invalidate(); }} placeholder="refs/heads/layrs/main" />}
          {exportKind === "snapshot" && <TextField label="Identifiant de la capture" value={stepId} onChange={(event) => { setStepId(event.target.value); invalidate(); }} placeholder="step:…" hint="La capture sera exportée sur refs/heads/main, sans ses parents." />}
        </>}
        <div className="new-project__actions">
          <Button variant="primary" busy={busy} busyLabel="Préparation…" disabledReason={workspace.kind !== "complete" ? "Ouvrez un projet complet." : workspace.busy ? "Une opération est en cours." : tab === "import" && !source.trim() ? "Choisissez le dépôt source." : tab === "export" && !destination.trim() ? "Indiquez un nouveau dossier." : undefined} onClick={() => void inspect()}>Prévisualiser</Button>
        </div>
      </Card>
      {preview && <Card className="settings-card">
        <h2>Aperçu avant confirmation</h2>
        {preview.kind === "import" ? <>
          <p>{preview.summary.commitCount} commit(s) depuis <code>{preview.summary.source}</code>, {preview.summary.references.length} référence(s).</p>
          <ul>{preview.summary.references.map((reference) => <li key={reference}><code>{reference}</code></li>)}</ul>
          {preview.summary.warnings.map((warning) => <p key={warning}>{warning}</p>)}
        </> : <>
          <p>Destination : <code>{preview.summary.destination}</code> (nouveau dépôt bare).</p>
          {preview.summary.mode === "history" && <p>Historique visible de {preview.summary.layerCount} layer(s) et {preview.summary.markerCount} marqueur(s).</p>}
          {preview.summary.mode === "selected_history" && <p>Références demandées : {preview.summary.references.map((reference) => <code key={reference}>{reference} </code>)}</p>}
          {preview.summary.mode === "snapshot" && <p>Capture seule : <code>{preview.summary.stepId}</code> sur <code>refs/heads/main</code>.</p>}
          <p>La destination et l’état du projet sont vérifiés ici. La compatibilité des objets et des références est vérifiée pendant l’export ; une erreur peut laisser un dépôt incomplet à reprendre en ligne de commande.</p>
        </>}
        <div className="new-project__actions">
          <Button onClick={invalidate}>Annuler</Button>
          <Button variant="primary" busy={busy} busyLabel="Exécution…" disabledReason={workspace.busy ? "Une opération est en cours." : undefined} onClick={() => void apply()}>Confirmer {preview.kind === "import" ? "l’import" : "l’export"}</Button>
        </div>
      </Card>}
      {error && <Banner tone="danger" title="Opération Git impossible">{error}</Banner>}
      {result && <Card className="settings-card">
        <h2>Rapport Git</h2>
        <Banner tone="success" title="Opération Git terminée">{result.message}</Banner>
        {result.warnings.length > 0 && <Banner tone="warning" title="Conversions et limites"><ul>{result.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul></Banner>}
        <details><summary>Références et identifiants ({result.references.length})</summary><ul>{result.references.map((entry) => <li key={entry.name}><code>{entry.name}</code> → <code>{entry.target}</code></li>)}</ul></details>
      </Card>}
    </div>
  );
}
