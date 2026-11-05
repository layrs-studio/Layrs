import { useState } from "react";
import { Banner, Button, Card, Link, TechnicalId, formatBytes, segment } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useWorkspace } from "../../app/workspace";
import { useOfflineTask } from "../../app/useOfflineTask";
import { versionLabel } from "../../app/offline";
import { localRestoreRequest, localRestorePreview, localRestoreReceipt, type LocalRestorePreview, type LocalRestoreReceipt, type RestoreTarget } from "../../app/offlineRestore";

export function OfflineRestore({ blocked, resolve }: { blocked?: string; resolve: () => Promise<RestoreTarget> }) {
  const projection = useProjection(), { root } = useWorkspace(), task = useOfflineTask();
  const [preview, setPreview] = useState<LocalRestorePreview | null>(null), [receipt, setReceipt] = useState<LocalRestoreReceipt | null>(null);
  const data = projection.data;
  const reason = task.blocked ?? blocked ?? (!projection.ready || data?.capabilities.restore !== true || data?.pendingPublication ? "Actualisez ou reprenez le dossier avant de restaurer." : undefined);
  async function prepare() {
    if (reason || !data) return;
    setPreview(null); setReceipt(null);
    const result = await task.execute("Préparation de la restauration locale…", async () => {
      const target = await resolve();
      return localRestorePreview(await task.local(localRestoreRequest(target, data)), target, data);
    });
    if (result) setPreview(result);
  }
  async function apply() {
    if (reason || !data || !preview) return;
    const reviewed = preview;
    setPreview(null); setReceipt(null);
    const result = await task.execute("Restauration locale…", async () => {
      localRestorePreview({ preview: reviewed }, reviewed.source, data);
      return localRestoreReceipt(await task.local({ action: "applyRestore", preview: reviewed }), reviewed, data);
    });
    if (result) {
      setReceipt(result);
      // The local capture changed. This read runs after the foreground queue
      // releases; it cannot fetch content and must not deadlock that queue.
      await projection.refresh();
    }
  }
  return <section className="settings-section" aria-labelledby="offline-restore">
    <h3 id="offline-restore">Restaurer les chemins sélectionnés</h3>
    <Card className="settings-card">
      <p>La version et les chemins ci-dessus définissent cette restauration locale. Les contenus doivent déjà être disponibles ; cette action ne télécharge rien. Les octets remplacés restent conservés par le moteur.</p>
      <Button disabledReason={reason} onClick={() => void prepare()}>Préparer l’aperçu local</Button>
      {preview && <div aria-label="Aperçu de restauration locale">
        <p>Destination : <code>{root}</code> · Worktree <TechnicalId value={preview.destination.identity.worktreeId} /></p>
        <p>Source : {versionLabel(preview.source.version)} · génération courante {preview.projectionGeneration}</p>
        <p>Empreinte de la version : <TechnicalId value={preview.source.version.kind === "remote" ? preview.source.version.observationDigest : preview.source.version.digest} /></p>
        <p>Empreinte de l’aperçu : <TechnicalId value={preview.digest} /></p>
        <ul>{preview.source.paths.map(path => <li key={path}>
          <strong><code>{path}</code></strong>
          <dl className="settings-facts"><dt>Avant</dt><dd><FileMetadata file={preview.before[path]} /></dd><dt>Après</dt><dd><FileMetadata file={preview.after[path]} /></dd></dl>
        </li>)}</ul>
        <p>Vérifiez ces chemins avant d’appliquer. Le moteur refusera si le dossier, la version ou les droits ont changé depuis cet aperçu.</p>
        <Button variant="primary" disabledReason={reason} onClick={() => void apply()}>Appliquer cette restauration locale</Button>
      </div>}
      {receipt && <p role="status">Restauration locale confirmée · capture {receipt.capture.sequence}. Les octets précédents sont conservés. Aucune publication n’a été effectuée.</p>}
      <p><Link to={`/projects/${segment(root)}/health`}>Diagnostic et reprise d’une restauration interrompue</Link></p>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
    </Card>
  </section>;
}

function FileMetadata({ file }: { file: LocalRestorePreview["before"][string] }) {
  return file === null ? <>Absence enregistrée</> : <>{formatBytes(file.bytes)} · {file.executable ? "Exécutable" : "Non exécutable"} · <TechnicalId value={file.fileId} /></>;
}
